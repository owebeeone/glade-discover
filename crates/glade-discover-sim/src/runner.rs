use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::num::NonZeroU64;

use glade_discover_core::{
    ClaimCommand, ClaimMode, Effect, Event, KernelConfig, MineStatus, MonoInstant,
    NodePrincipalBinding, PersistedState, PrincipalCtx, PrincipalPlane, RoundProgress, RouteAns,
    State, StepCtx, VerificationBatch, VerificationResult, WakeClass, WakeToken, WallMs,
    WatermarkLoad, restore_fresh, restore_with_recovery, step,
};
use glade_discover_protocol::{
    ClaimDraft, ClaimId, ClaimIdentity, Corr, DefRevId, DirectoryRecord, Generation, GrantId,
    IngressId, IntentId, NodeId, OpEnvelope, Principal, RecordId, RouteQuery, ServeClaim,
    ServiceInstanceClaim, Shape, Slot, StreamHead, StreamId, SyncId, WireMsg, decode_signed_op,
    encode_directory_record, encode_signed_op, op_hash, record_id,
};

use crate::faults::{SplitMix64, sample_clock};
use crate::network::Network;
use crate::queue::{EventId, InputQueue, MsgRef};
use crate::schema::{
    ClaimCommandSpec, ClaimDraftSpec, ClaimIdentitySpec, ClaimModeSpec, EffectSpec, Expectation,
    ExpectationTime, MineStatusSpec, MintedSelectorRegistry, OpSelector, RecordIdSpec, RestartSpec,
    RestartWatermark, RouteAnsSpec, Scenario, ScenarioEvent, SlotSpec, StateAssertion, StopSpec,
    SyncStatusSpec, VerificationOutcome, VerifierTable, WakeTokenSpec, WireMsgSpec,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerError {
    NotImplemented,
    DidNotQuiesce,
    EventBudgetExceeded,
    InvalidSetup,
    ProtocolDecode,
    ClockOverflow,
    QueueOverflow,
    MissingNode,
    OracleMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunReport {
    event_count: u64,
    event_log: Vec<String>,
}

impl RunReport {
    #[must_use]
    pub const fn event_count(&self) -> u64 {
        self.event_count
    }

    #[must_use]
    pub fn event_log(&self) -> &[String] {
        &self.event_log
    }
}

#[derive(Clone, Debug)]
struct ObservedEffect {
    event_id: u64,
    at_ms: u64,
    node: String,
    effect: Effect,
}

enum Scheduled<'a> {
    Input(&'a crate::schema::ScenarioInput),
    Generated {
        node: String,
        event: Event,
    },
    Restart {
        node: &'a crate::schema::NodeSpec,
        spec: &'a RestartSpec,
    },
}

impl Scheduled<'_> {
    fn is_non_periodic(&self) -> bool {
        match self {
            Self::Input(input) => match &input.event {
                ScenarioEvent::Wakeup {
                    token: WakeTokenSpec::GossipTick { .. },
                } => false,
                ScenarioEvent::Deliver { .. }
                | ScenarioEvent::Route { .. }
                | ScenarioEvent::Advertise { .. }
                | ScenarioEvent::OpAccepted { .. }
                | ScenarioEvent::Wakeup { .. }
                | ScenarioEvent::ClockReseed { .. } => true,
            },
            Self::Generated {
                event: Event::Wakeup { token },
                ..
            } => token.class() == WakeClass::OneShot,
            Self::Generated { .. } | Self::Restart { .. } => true,
        }
    }
}

#[derive(Default)]
struct SimAppendEnvironment {
    registry: MintedSelectorRegistry<ClaimDraft>,
    streams: BTreeMap<(String, StreamId), Vec<glade_discover_protocol::SignedOp>>,
    accepted_by_intent: BTreeMap<String, Event>,
    authored_callbacks: BTreeSet<String>,
}

impl SimAppendEnvironment {
    fn new(scenario: &Scenario) -> Self {
        Self {
            authored_callbacks: scenario
                .inputs
                .iter()
                .filter_map(|input| match &input.event {
                    ScenarioEvent::OpAccepted { intent, .. } => Some(intent.clone()),
                    _ => None,
                })
                .collect(),
            ..Self::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_append(
        &mut self,
        node: &str,
        intent: &IntentId,
        slot: &Slot,
        generation: Generation,
        draft: &ClaimDraft,
        origin: &Principal,
        verifier: &VerifierTable,
    ) -> Result<Option<Event>, RunnerError> {
        let slot_spec = slot_to_spec(slot);
        let selector = self
            .registry
            .observe_append(node, &slot_spec, generation.0, intent.as_str(), draft)
            .map_err(|_| RunnerError::InvalidSetup)?;
        if self.authored_callbacks.contains(intent.as_str()) {
            return Ok(None);
        }
        if let Some(accepted) = self.accepted_by_intent.get(intent.as_str()) {
            return Ok(Some(accepted.clone()));
        }

        let stream = append_stream(slot);
        let entries = self
            .streams
            .entry((node.to_owned(), stream.clone()))
            .or_default();
        let seq = u64::try_from(entries.len()).map_err(|_| RunnerError::QueueOverflow)?;
        let prev = entries.last().map(op_hash);
        let record_id = RecordId {
            stream: stream.clone(),
            origin: origin.clone(),
            seq,
        };
        let record = finalize_draft(draft, record_id);
        let envelope = OpEnvelope {
            stream,
            origin: origin.clone(),
            seq,
            prev,
            lamport: seq.saturating_add(1),
            refs: Vec::new(),
            shape: Shape::Log,
            payload: encode_directory_record(&record).map_err(|_| RunnerError::ProtocolDecode)?,
        };
        let op = encode_signed_op(&envelope, b"glade-discover-sim")
            .map_err(|_| RunnerError::ProtocolDecode)?;
        self.registry
            .bind_persisted(intent.as_str(), op.canonical_bytes())
            .map_err(|_| RunnerError::InvalidSetup)?;
        entries.push(op.clone());
        let verification =
            convert_verification(verifier.outcome(&selector, node, op.envelope().origin.as_str()));
        let accepted = Event::OpAccepted {
            intent: intent.clone(),
            op: Box::new(op),
            verification,
        };
        self.accepted_by_intent
            .insert(intent.to_string(), accepted.clone());
        Ok(Some(accepted))
    }

    fn accepted_event(
        &mut self,
        input: &crate::schema::ScenarioInput,
        intent: &str,
        spec: &crate::schema::SignedOpSpec,
        verifier: &VerifierTable,
    ) -> Result<Event, RunnerError> {
        let op = decode_op(spec)?;
        let selector = self
            .registry
            .bind_persisted(intent, op.canonical_bytes())
            .map_err(|_| RunnerError::InvalidSetup)?;
        let verification = convert_verification(verifier.outcome(
            &selector,
            &input.node,
            op.envelope().origin.as_str(),
        ));
        Ok(Event::OpAccepted {
            intent: IntentId::from(intent),
            op: Box::new(op),
            verification,
        })
    }
}

fn slot_to_spec(slot: &Slot) -> SlotSpec {
    match slot {
        Slot::Workspace { share } => SlotSpec::Workspace {
            share: share.clone(),
        },
        Slot::Binding {
            share,
            glade_id,
            key,
        } => SlotSpec::Binding {
            share: share.clone(),
            glade_id: glade_id.clone(),
            key_hex: encode_hex(key),
        },
    }
}

fn append_stream(slot: &Slot) -> StreamId {
    match slot {
        Slot::Workspace { share } => StreamId {
            share: share.clone(),
            glade_id: "directory".to_owned(),
            key: b"sim-append".to_vec(),
        },
        Slot::Binding {
            share,
            glade_id,
            key,
        } => StreamId {
            share: share.clone(),
            glade_id: glade_id.clone(),
            key: key.clone(),
        },
    }
}

fn finalize_draft(draft: &ClaimDraft, allocated: RecordId) -> DirectoryRecord {
    match draft {
        ClaimDraft::Workspace {
            node,
            share,
            identity,
            grant_ref,
            lease_expiry_ms,
            epoch,
        } => DirectoryRecord::ServeClaim(ServeClaim {
            node: node.clone(),
            share: share.clone(),
            claim_id: match identity {
                ClaimIdentity::Mint => ClaimId::from(allocated),
                ClaimIdentity::Existing(existing) => existing.clone(),
            },
            grant_ref: grant_ref.clone(),
            lease_expiry_ms: *lease_expiry_ms,
            epoch: *epoch,
        }),
        ClaimDraft::Service {
            node,
            share,
            glade_id,
            key,
            identity,
            def_ref,
            exec_grant_ref,
            compute_key,
            lease_expiry_ms,
            epoch,
        } => DirectoryRecord::ServiceInstanceClaim(ServiceInstanceClaim {
            node: node.clone(),
            share: share.clone(),
            glade_id: glade_id.clone(),
            key: key.clone(),
            claim_id: match identity {
                ClaimIdentity::Mint => ClaimId::from(allocated),
                ClaimIdentity::Existing(existing) => existing.clone(),
            },
            def_ref: def_ref.clone(),
            exec_grant_ref: exec_grant_ref.clone(),
            compute_key: compute_key.clone(),
            lease_expiry_ms: *lease_expiry_ms,
            epoch: *epoch,
        }),
    }
}

fn event_is_substantive(
    event: &Event,
    before: &State,
    after: &State,
    effects: &[Effect],
    current_mono: u64,
) -> bool {
    let Event::Wakeup { token } = event else {
        return true;
    };
    if token.class() != WakeClass::Periodic || before != after {
        return true;
    }
    !matches!(
        effects,
        [Effect::Schedule {
            token: successor,
            at_mono,
        }] if successor == token && at_mono.0 > current_mono
    )
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

pub fn run_scenario(scenario: &Scenario) -> Result<RunReport, RunnerError> {
    let (horizon, quiet_ms) = match scenario.stop {
        StopSpec::AtMs { at_ms } => (at_ms, None),
        StopSpec::QuiescentForMs {
            quiet_ms,
            deadline_ms,
        } => (deadline_ms, Some(quiet_ms.get())),
    };

    let owners = owner_roots(scenario)?;
    let bindings = node_bindings(scenario);
    let mut states = initial_states(scenario, &owners, &bindings)?;
    let verifier = VerifierTable::from_fixtures(&scenario.verifier_fixtures);
    let network = Network::new(scenario.links.clone()).map_err(|_| RunnerError::InvalidSetup)?;
    let mut rng = SplitMix64::new(scenario.seed);
    let mut append_environment = SimAppendEnvironment::new(scenario);
    let selector_by_bytes = input_selectors(scenario)?;
    let mut queue = InputQueue::default();
    for input in &scenario.inputs {
        queue
            .push(input.at_ms, Scheduled::Input(input))
            .map_err(|_| RunnerError::QueueOverflow)?;
    }
    for node in &scenario.nodes {
        for restart in &node.restarts {
            queue
                .push(
                    restart.at_ms,
                    Scheduled::Restart {
                        node,
                        spec: restart,
                    },
                )
                .map_err(|_| RunnerError::QueueOverflow)?;
        }
    }

    let mut event_count = 0_u64;
    let mut event_log = Vec::new();
    let mut observed = Vec::new();
    let mut snapshots = vec![(0, states.clone())];
    let mut last_substantive_ms = 0_u64;
    let mut pending_non_periodic = false;
    while let Some((at_ms, scheduled)) = queue.pop() {
        if at_ms > horizon {
            pending_non_periodic |= scheduled.is_non_periodic();
            while let Some((_at_ms, remaining)) = queue.pop() {
                pending_non_periodic |= remaining.is_non_periodic();
            }
            break;
        }
        event_count = event_count
            .checked_add(1)
            .ok_or(RunnerError::EventBudgetExceeded)?;
        if event_count > scenario.event_budget.get() {
            return Err(RunnerError::EventBudgetExceeded);
        }
        match scheduled {
            Scheduled::Input(input) => {
                let node_spec = scenario
                    .nodes
                    .iter()
                    .find(|node| node.id == input.node)
                    .ok_or(RunnerError::MissingNode)?;
                let sample = sample_clock(&node_spec.clock, at_ms)
                    .map_err(|_| RunnerError::ClockOverflow)?;
                let event = match &input.event {
                    ScenarioEvent::OpAccepted { intent, op } => {
                        append_environment.accepted_event(input, intent, op, &verifier)?
                    }
                    _ => convert_event(input, &verifier)?,
                };
                let state = states.remove(&input.node).ok_or(RunnerError::MissingNode)?;
                let before = state.clone();
                let event_debug = format!("{event:?}");
                let (state, effects) = step(
                    state,
                    StepCtx {
                        mono: MonoInstant(sample.mono_ms),
                        wall: WallMs(sample.wall_ms),
                    },
                    event.clone(),
                );
                if event_is_substantive(&event, &before, &state, &effects, sample.mono_ms) {
                    last_substantive_ms = at_ms;
                }
                event_log.push(format!("{at_ms}:{}:{event_debug}:{effects:?}", input.node));
                observed.extend(effects.iter().cloned().map(|effect| ObservedEffect {
                    event_id: event_count,
                    at_ms,
                    node: input.node.clone(),
                    effect,
                }));
                states.insert(input.node.clone(), state);
                enqueue_effects(
                    &mut queue,
                    &network,
                    &mut rng,
                    scenario,
                    &verifier,
                    &selector_by_bytes,
                    &mut append_environment,
                    event_count,
                    at_ms,
                    &input.node,
                    &effects,
                )?;
            }
            Scheduled::Generated { node, event } => {
                let node_spec = scenario
                    .nodes
                    .iter()
                    .find(|candidate| candidate.id == node)
                    .ok_or(RunnerError::MissingNode)?;
                let sample = sample_clock(&node_spec.clock, at_ms)
                    .map_err(|_| RunnerError::ClockOverflow)?;
                let state = states.remove(&node).ok_or(RunnerError::MissingNode)?;
                let before = state.clone();
                let event_debug = format!("{event:?}");
                let (state, effects) = step(
                    state,
                    StepCtx {
                        mono: MonoInstant(sample.mono_ms),
                        wall: WallMs(sample.wall_ms),
                    },
                    event.clone(),
                );
                if event_is_substantive(&event, &before, &state, &effects, sample.mono_ms) {
                    last_substantive_ms = at_ms;
                }
                event_log.push(format!("{at_ms}:{node}:{event_debug}:{effects:?}"));
                observed.extend(effects.iter().cloned().map(|effect| ObservedEffect {
                    event_id: event_count,
                    at_ms,
                    node: node.clone(),
                    effect,
                }));
                states.insert(node.clone(), state);
                enqueue_effects(
                    &mut queue,
                    &network,
                    &mut rng,
                    scenario,
                    &verifier,
                    &selector_by_bytes,
                    &mut append_environment,
                    event_count,
                    at_ms,
                    &node,
                    &effects,
                )?;
            }
            Scheduled::Restart { node, spec } => {
                last_substantive_ms = at_ms;
                let sample =
                    sample_clock(&node.clock, at_ms).map_err(|_| RunnerError::ClockOverflow)?;
                let old = states.remove(&node.id).ok_or(RunnerError::MissingNode)?;
                let load = match spec.watermark {
                    RestartWatermark::Preserve => match old.clock() {
                        glade_discover_core::ClockState::Ready { watermark }
                        | glade_discover_core::ClockState::Uncertain {
                            floor: Some(watermark),
                        } => WatermarkLoad::Readable(watermark),
                        glade_discover_core::ClockState::Uncertain { floor: None } => {
                            WatermarkLoad::Unreadable
                        }
                    },
                    RestartWatermark::Unreadable => WatermarkLoad::Unreadable,
                    RestartWatermark::Replace { value } => WatermarkLoad::Readable(WallMs(value)),
                };
                let (restarted, recovery_effects) = restore_with_recovery(
                    old.config().clone(),
                    old.persisted().clone(),
                    load,
                    StepCtx {
                        mono: MonoInstant(sample.mono_ms),
                        wall: WallMs(sample.wall_ms),
                    },
                );
                event_log.push(format!(
                    "{at_ms}:{}:Restart:{:?}:{recovery_effects:?}",
                    node.id, spec.watermark
                ));
                observed.extend(
                    recovery_effects
                        .iter()
                        .cloned()
                        .map(|effect| ObservedEffect {
                            event_id: event_count,
                            at_ms,
                            node: node.id.clone(),
                            effect,
                        }),
                );
                states.insert(node.id.clone(), restarted);
                enqueue_effects(
                    &mut queue,
                    &network,
                    &mut rng,
                    scenario,
                    &verifier,
                    &selector_by_bytes,
                    &mut append_environment,
                    event_count,
                    at_ms,
                    &node.id,
                    &recovery_effects,
                )?;
            }
        }
        snapshots.push((at_ms, states.clone()));
    }

    if quiet_ms.is_some_and(|quiet| {
        pending_non_periodic
            || last_substantive_ms
                .checked_add(quiet)
                .is_none_or(|settled_at| settled_at > horizon)
    }) {
        return Err(RunnerError::DidNotQuiesce);
    }

    if !append_environment
        .registry
        .unfulfilled(&scenario.verifier_fixtures)
        .is_empty()
    {
        return Err(RunnerError::InvalidSetup);
    }

    check_expectations(scenario, &snapshots, &observed)?;
    Ok(RunReport {
        event_count,
        event_log,
    })
}

fn input_selectors(scenario: &Scenario) -> Result<BTreeMap<Vec<u8>, OpSelector>, RunnerError> {
    let mut selectors = BTreeMap::new();
    for input in &scenario.inputs {
        let specs: Vec<&crate::schema::SignedOpSpec> = match &input.event {
            ScenarioEvent::Deliver {
                msg: WireMsgSpec::DirOp { op },
                ..
            }
            | ScenarioEvent::OpAccepted { op, .. } => vec![op],
            ScenarioEvent::Deliver {
                msg: WireMsgSpec::SyncOps { ops, .. },
                ..
            } => ops.iter().collect(),
            ScenarioEvent::Deliver { .. }
            | ScenarioEvent::Route { .. }
            | ScenarioEvent::Advertise { .. }
            | ScenarioEvent::Wakeup { .. }
            | ScenarioEvent::ClockReseed { .. } => Vec::new(),
        };
        for (ordinal, spec) in specs.into_iter().enumerate() {
            let ordinal = u32::try_from(ordinal).map_err(|_| RunnerError::InvalidSetup)?;
            selectors.insert(
                decode_hex(&spec.canonical_hex)?,
                OpSelector::Input {
                    input_id: input.input_id.clone(),
                    op_ordinal: ordinal,
                },
            );
        }
    }
    Ok(selectors)
}

#[allow(clippy::too_many_arguments)]
fn enqueue_effects(
    queue: &mut InputQueue<Scheduled<'_>>,
    network: &Network,
    rng: &mut SplitMix64,
    scenario: &Scenario,
    verifier: &VerifierTable,
    selector_by_bytes: &BTreeMap<Vec<u8>, OpSelector>,
    append_environment: &mut SimAppendEnvironment,
    event_id: u64,
    at_ms: u64,
    from: &str,
    effects: &[Effect],
) -> Result<(), RunnerError> {
    for (emission_index, effect) in effects.iter().enumerate() {
        match effect {
            Effect::Gossip { to, msg } => {
                let msg_ref = MsgRef {
                    producing_event_id: EventId(event_id),
                    emission_index: u32::try_from(emission_index)
                        .map_err(|_| RunnerError::QueueOverflow)?,
                };
                let plans = match network.plan(
                    from,
                    to.as_str(),
                    at_ms,
                    msg_ref,
                    rng,
                    scenario.event_budget.get(),
                ) {
                    Ok(plans) => plans,
                    Err(crate::network::NetworkError::DeliveryBudgetExceeded) => {
                        return Err(RunnerError::EventBudgetExceeded);
                    }
                    Err(_) => {
                        // A missing link means no modeled transport path; the
                        // emitted effect remains observable to the oracle.
                        continue;
                    }
                };
                for plan in plans {
                    let verification = verification_for_message(
                        msg,
                        to.as_str(),
                        verifier,
                        selector_by_bytes,
                        append_environment,
                    );
                    queue
                        .push(
                            plan.at_ms,
                            Scheduled::Generated {
                                node: to.to_string(),
                                event: Event::Deliver {
                                    from: NodeId::from(from),
                                    msg: msg.clone(),
                                    verification,
                                },
                            },
                        )
                        .map_err(|_| RunnerError::QueueOverflow)?;
                }
            }
            Effect::Schedule { token, at_mono } => {
                let node = scenario
                    .nodes
                    .iter()
                    .find(|node| node.id == from)
                    .ok_or(RunnerError::MissingNode)?;
                let sim_ms = at_mono
                    .0
                    .checked_sub(node.clock.initial_mono_ms)
                    .ok_or(RunnerError::ClockOverflow)?;
                queue
                    .push(
                        sim_ms,
                        Scheduled::Generated {
                            node: from.to_owned(),
                            event: Event::Wakeup {
                                token: token.clone(),
                            },
                        },
                    )
                    .map_err(|_| RunnerError::QueueOverflow)?;
            }
            Effect::Append {
                intent,
                slot,
                generation,
                draft,
            } => {
                if let Some(event) = append_environment.observe_append(
                    from,
                    intent,
                    slot,
                    *generation,
                    draft,
                    &Principal::from(
                        scenario
                            .nodes
                            .iter()
                            .find(|node| node.id == from)
                            .ok_or(RunnerError::MissingNode)?
                            .principal
                            .clone(),
                    ),
                    verifier,
                )? {
                    queue
                        .push(
                            at_ms,
                            Scheduled::Generated {
                                node: from.to_owned(),
                                event,
                            },
                        )
                        .map_err(|_| RunnerError::QueueOverflow)?;
                }
            }
            Effect::Reply { .. } | Effect::Teardown { .. } => {}
        }
    }
    Ok(())
}

fn verification_for_message(
    message: &WireMsg,
    reader: &str,
    verifier: &VerifierTable,
    selector_by_bytes: &BTreeMap<Vec<u8>, OpSelector>,
    append_environment: &SimAppendEnvironment,
) -> VerificationBatch {
    let ops: Vec<&glade_discover_protocol::SignedOp> = match message {
        WireMsg::DirOp { op } => vec![op],
        WireMsg::SyncOps { ops, .. } => ops.iter().collect(),
        WireMsg::SyncStart { .. } | WireMsg::SyncEnd { .. } => Vec::new(),
    };
    VerificationBatch(
        ops.into_iter()
            .map(|op| {
                append_environment
                    .registry
                    .selector_for_bytes(op.canonical_bytes())
                    .or_else(|| selector_by_bytes.get(op.canonical_bytes()))
                    .map_or_else(
                        || VerificationResult::Valid {
                            signer: op.envelope().origin.clone(),
                        },
                        |selector| {
                            convert_verification(verifier.outcome(
                                selector,
                                reader,
                                op.envelope().origin.as_str(),
                            ))
                        },
                    )
            })
            .collect(),
    )
}

pub fn run_all_scenarios() -> Result<(), RunnerError> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let mut pending = vec![root];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(|_| RunnerError::InvalidSetup)? {
            let path = entry.map_err(|_| RunnerError::InvalidSetup)?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    for path in files {
        let input = std::fs::read_to_string(path).map_err(|_| RunnerError::InvalidSetup)?;
        let scenario = Scenario::from_json(&input).map_err(|_| RunnerError::InvalidSetup)?;
        run_scenario(&scenario)?;
    }
    Ok(())
}

fn owner_roots(scenario: &Scenario) -> Result<BTreeMap<String, Principal>, RunnerError> {
    let mut owners = BTreeMap::new();
    for node in &scenario.nodes {
        for share in &node.owns {
            let principal = Principal::from(node.principal.clone());
            if owners
                .insert(share.clone(), principal.clone())
                .is_some_and(|old| old != principal)
            {
                return Err(RunnerError::InvalidSetup);
            }
        }
    }
    Ok(owners)
}

fn node_bindings(scenario: &Scenario) -> BTreeSet<NodePrincipalBinding> {
    scenario
        .nodes
        .iter()
        .flat_map(|node| {
            [PrincipalPlane::Workspace, PrincipalPlane::Derived].map(|plane| NodePrincipalBinding {
                node: NodeId::from(node.id.clone()),
                principal: Principal::from(node.principal.clone()),
                plane,
            })
        })
        .collect()
}

fn initial_states(
    scenario: &Scenario,
    owners: &BTreeMap<String, Principal>,
    bindings: &BTreeSet<NodePrincipalBinding>,
) -> Result<BTreeMap<String, State>, RunnerError> {
    let all_nodes = scenario
        .nodes
        .iter()
        .map(|node| NodeId::from(node.id.clone()))
        .collect::<BTreeSet<_>>();
    scenario
        .nodes
        .iter()
        .map(|node| {
            let sample = sample_clock(&node.clock, 0).map_err(|_| RunnerError::ClockOverflow)?;
            let local = NodeId::from(node.id.clone());
            let mut peers = all_nodes.clone();
            peers.remove(&local);
            let config = KernelConfig {
                local_node: local,
                local_principal: Principal::from(node.principal.clone()),
                peers,
                workspace_owner_roots: owners.clone(),
                node_principal_bindings: bindings.clone(),
                skew_margin_ms: 5_000,
                max_lease_ms: 3_600_000,
                clock_resync_ms: 30_000,
                sync_retries: 3,
                sync_timeout_ms: NonZeroU64::new(10_000).expect("constant is non-zero"),
                gossip_fan: 8,
                max_retained_bytes: node.max_retained_bytes,
            };
            let ctx = StepCtx {
                mono: MonoInstant(sample.mono_ms),
                wall: WallMs(sample.wall_ms),
            };
            let mut state = restore_fresh(
                config,
                PersistedState::default(),
                WatermarkLoad::Readable(WallMs(sample.wall_ms)),
                ctx,
            );
            for seed in &node.seed_grants {
                let op = decode_op(seed)?;
                let expected_id = record_id(&op);
                let expected_bytes = op.canonical_bytes().to_vec();
                let signer = op.envelope().origin.clone();
                (state, _) = step(
                    state,
                    ctx,
                    Event::Deliver {
                        from: NodeId::from(node.id.clone()),
                        msg: Box::new(WireMsg::DirOp { op: Box::new(op) }),
                        verification: VerificationBatch(vec![VerificationResult::Valid { signer }]),
                    },
                );
                let folded_exactly = state
                    .persisted()
                    .retained
                    .get(&expected_id.stream)
                    .and_then(|records| records.get(&expected_id))
                    .is_some_and(|retained| retained.canonical_bytes() == expected_bytes);
                if !folded_exactly {
                    return Err(RunnerError::InvalidSetup);
                }
            }
            Ok((node.id.clone(), state))
        })
        .collect()
}

fn convert_event(
    input: &crate::schema::ScenarioInput,
    verifier: &VerifierTable,
) -> Result<Event, RunnerError> {
    match &input.event {
        ScenarioEvent::Deliver { from, msg } => {
            let (msg, verification) =
                convert_wire_msg(msg, Some((&input.input_id, &input.node, verifier)))?;
            Ok(Event::Deliver {
                from: NodeId::from(from.clone()),
                msg: Box::new(msg),
                verification,
            })
        }
        ScenarioEvent::Route {
            ingress,
            principal,
            authenticated_context_hex,
            corr,
            query,
        } => Ok(Event::Route {
            ingress: IngressId::from(ingress.clone()),
            principal: PrincipalCtx {
                principal: Principal::from(principal.clone()),
                authenticated_context: decode_hex(authenticated_context_hex)?,
            },
            corr: Corr::from(corr.clone()),
            query: RouteQuery {
                slot: convert_slot(query)?,
            },
        }),
        ScenarioEvent::ClockReseed { watermark } => Ok(Event::ClockReseed {
            watermark: WallMs(*watermark),
        }),
        ScenarioEvent::Advertise { command } => Ok(Event::Advertise {
            command: Box::new(convert_claim_command(command)?),
        }),
        ScenarioEvent::OpAccepted { intent, op } => {
            let op = decode_op(op)?;
            let selector = OpSelector::Input {
                input_id: input.input_id.clone(),
                op_ordinal: 0,
            };
            let verification = convert_verification(verifier.outcome(
                &selector,
                &input.node,
                op.envelope().origin.as_str(),
            ));
            Ok(Event::OpAccepted {
                intent: IntentId::from(intent.clone()),
                op: Box::new(op),
                verification,
            })
        }
        ScenarioEvent::Wakeup { token } => Ok(Event::Wakeup {
            token: convert_wake_token(token)?,
        }),
    }
}

fn convert_wire_msg(
    message: &WireMsgSpec,
    input: Option<(&str, &str, &VerifierTable)>,
) -> Result<(WireMsg, VerificationBatch), RunnerError> {
    match message {
        WireMsgSpec::DirOp { op } => {
            let op = decode_op(op)?;
            let verification =
                input.map_or_else(VerificationBatch::default, |(id, reader, table)| {
                    VerificationBatch(vec![convert_verification(table.outcome(
                        &OpSelector::Input {
                            input_id: id.to_owned(),
                            op_ordinal: 0,
                        },
                        reader,
                        op.envelope().origin.as_str(),
                    ))])
                });
            Ok((WireMsg::DirOp { op: Box::new(op) }, verification))
        }
        WireMsgSpec::SyncStart { sync_id, heads } => Ok((
            WireMsg::SyncStart {
                sync_id: SyncId::from(sync_id.clone()),
                heads: heads
                    .iter()
                    .map(convert_stream_head)
                    .collect::<Result<_, _>>()?,
            },
            VerificationBatch::default(),
        )),
        WireMsgSpec::SyncOps { sync_id, ops } => {
            let ops = ops.iter().map(decode_op).collect::<Result<Vec<_>, _>>()?;
            let verification =
                input.map_or_else(VerificationBatch::default, |(id, reader, table)| {
                    VerificationBatch(
                        ops.iter()
                            .enumerate()
                            .map(|(ordinal, op)| {
                                let ordinal = u32::try_from(ordinal).unwrap_or(u32::MAX);
                                convert_verification(table.outcome(
                                    &OpSelector::Input {
                                        input_id: id.to_owned(),
                                        op_ordinal: ordinal,
                                    },
                                    reader,
                                    op.envelope().origin.as_str(),
                                ))
                            })
                            .collect(),
                    )
                });
            Ok((
                WireMsg::SyncOps {
                    sync_id: SyncId::from(sync_id.clone()),
                    ops,
                },
                verification,
            ))
        }
        WireMsgSpec::SyncEnd { sync_id } => Ok((
            WireMsg::SyncEnd {
                sync_id: SyncId::from(sync_id.clone()),
            },
            VerificationBatch::default(),
        )),
    }
}

fn decode_op(
    spec: &crate::schema::SignedOpSpec,
) -> Result<glade_discover_protocol::SignedOp, RunnerError> {
    decode_signed_op(&decode_hex(&spec.canonical_hex)?).map_err(|_| RunnerError::ProtocolDecode)
}

fn convert_verification(outcome: VerificationOutcome) -> VerificationResult {
    match outcome {
        VerificationOutcome::Valid { signer } => VerificationResult::Valid {
            signer: Principal::from(signer),
        },
        VerificationOutcome::BadSignature => VerificationResult::BadSignature,
    }
}

fn convert_stream_head(spec: &crate::schema::StreamHeadSpec) -> Result<StreamHead, RunnerError> {
    let hash: [u8; 32] = decode_hex(&spec.hash_hex)?
        .try_into()
        .map_err(|_| RunnerError::ProtocolDecode)?;
    Ok(StreamHead {
        stream: convert_stream(&spec.stream)?,
        origin: Principal::from(spec.origin.clone()),
        seq: spec.seq,
        hash,
    })
}

fn convert_claim_command(spec: &ClaimCommandSpec) -> Result<ClaimCommand, RunnerError> {
    Ok(ClaimCommand {
        intent: IntentId::from(spec.intent.clone()),
        slot: convert_slot(&spec.slot)?,
        generation: Generation(spec.generation),
        mode: match &spec.mode {
            ClaimModeSpec::Initial => ClaimMode::Initial,
            ClaimModeSpec::Renew => ClaimMode::Renew,
            ClaimModeSpec::Takeover { authority_ref } => ClaimMode::Takeover {
                authority_ref: GrantId::from(convert_record_id(authority_ref)?),
            },
        },
        draft: convert_claim_draft(&spec.draft)?,
    })
}

fn convert_claim_draft(spec: &ClaimDraftSpec) -> Result<ClaimDraft, RunnerError> {
    match spec {
        ClaimDraftSpec::Workspace {
            node,
            share,
            identity,
            grant_ref,
            lease_expiry_ms,
            epoch,
        } => Ok(ClaimDraft::Workspace {
            node: NodeId::from(node.clone()),
            share: share.clone(),
            identity: convert_claim_identity(identity)?,
            grant_ref: GrantId::from(convert_record_id(grant_ref)?),
            lease_expiry_ms: *lease_expiry_ms,
            epoch: *epoch,
        }),
        ClaimDraftSpec::Service {
            node,
            share,
            glade_id,
            key_hex,
            identity,
            def_ref,
            exec_grant_ref,
            compute_key_hex,
            lease_expiry_ms,
            epoch,
        } => Ok(ClaimDraft::Service {
            node: NodeId::from(node.clone()),
            share: share.clone(),
            glade_id: glade_id.clone(),
            key: decode_hex(key_hex)?,
            identity: convert_claim_identity(identity)?,
            def_ref: DefRevId::from(convert_record_id(def_ref)?),
            exec_grant_ref: GrantId::from(convert_record_id(exec_grant_ref)?),
            compute_key: decode_hex(compute_key_hex)?.into(),
            lease_expiry_ms: *lease_expiry_ms,
            epoch: *epoch,
        }),
    }
}

fn convert_claim_identity(spec: &ClaimIdentitySpec) -> Result<ClaimIdentity, RunnerError> {
    match spec {
        ClaimIdentitySpec::Mint => Ok(ClaimIdentity::Mint),
        ClaimIdentitySpec::Existing { claim_id } => Ok(ClaimIdentity::Existing(
            glade_discover_protocol::ClaimId::from(convert_record_id(claim_id)?),
        )),
    }
}

fn convert_wake_token(spec: &WakeTokenSpec) -> Result<WakeToken, RunnerError> {
    match spec {
        WakeTokenSpec::ClaimRenew { slot, generation } => Ok(WakeToken::ClaimRenew {
            slot: convert_slot(slot)?,
            generation: Generation(*generation),
        }),
        WakeTokenSpec::AppendRetry {
            slot,
            generation,
            intent,
        } => Ok(WakeToken::AppendRetry {
            slot: convert_slot(slot)?,
            generation: Generation(*generation),
            intent: IntentId::from(intent.clone()),
        }),
        WakeTokenSpec::GossipTick { peer } => Ok(WakeToken::GossipTick {
            peer: NodeId::from(peer.clone()),
        }),
        WakeTokenSpec::SyncTimeout {
            peer,
            sync_id,
            attempt,
        } => Ok(WakeToken::SyncTimeout {
            peer: NodeId::from(peer.clone()),
            sync_id: SyncId::from(sync_id.clone()),
            attempt: *attempt,
        }),
    }
}

fn check_expectations(
    scenario: &Scenario,
    snapshots: &[(u64, BTreeMap<String, State>)],
    observed: &[ObservedEffect],
) -> Result<(), RunnerError> {
    for expectation in &scenario.expect {
        match expectation {
            Expectation::Route {
                when: ExpectationTime::AtMs { at_ms },
                node,
                ingress,
                corr,
                ans,
            } => {
                let wanted = expected_reply(ingress, corr, ans);
                if observed
                    .iter()
                    .filter(|entry| entry.at_ms == *at_ms && entry.node == *node)
                    .filter(|entry| entry.effect == wanted)
                    .count()
                    != 1
                {
                    return Err(RunnerError::OracleMismatch);
                }
            }
            Expectation::Route {
                when: ExpectationTime::Throughout { start_ms, end_ms },
                node,
                ingress,
                corr,
                ans,
            } => {
                let wanted = expected_reply(ingress, corr, ans);
                for (event_offset, (at_ms, _)) in snapshots.iter().skip(1).enumerate() {
                    if *start_ms <= *at_ms && *at_ms < *end_ms {
                        let event_id = u64::try_from(event_offset)
                            .ok()
                            .and_then(|value| value.checked_add(1))
                            .ok_or(RunnerError::OracleMismatch)?;
                        if observed
                            .iter()
                            .filter(|entry| {
                                entry.event_id == event_id
                                    && entry.node == *node
                                    && entry.effect == wanted
                            })
                            .count()
                            != 1
                        {
                            return Err(RunnerError::OracleMismatch);
                        }
                    }
                }
            }
            Expectation::Effect {
                when: ExpectationTime::AtMs { at_ms },
                node,
                effect: expected,
                count,
            } => {
                let wanted = convert_effect_spec(expected)?;
                let actual = observed
                    .iter()
                    .filter(|entry| {
                        entry.at_ms == *at_ms && entry.node == *node && entry.effect == wanted
                    })
                    .count();
                if actual != usize::try_from(*count).map_err(|_| RunnerError::OracleMismatch)? {
                    return Err(RunnerError::OracleMismatch);
                }
            }
            Expectation::State {
                when: ExpectationTime::AtMs { at_ms },
                node,
                assertion,
            } => {
                if !state_assertion_matches(state_at(snapshots, *at_ms, node)?, assertion)? {
                    return Err(RunnerError::OracleMismatch);
                }
            }
            Expectation::State {
                when: ExpectationTime::Throughout { start_ms, end_ms },
                node,
                assertion,
            } => {
                for (at_ms, states) in snapshots.iter().skip(1) {
                    if *start_ms <= *at_ms && *at_ms < *end_ms {
                        let state = states.get(node).ok_or(RunnerError::MissingNode)?;
                        if !state_assertion_matches(state, assertion)? {
                            return Err(RunnerError::OracleMismatch);
                        }
                    }
                }
            }
            Expectation::Effect {
                when: ExpectationTime::Throughout { start_ms, end_ms },
                node,
                effect: expected,
                count,
            } => {
                let wanted = convert_effect_spec(expected)?;
                let actual = observed
                    .iter()
                    .filter(|entry| {
                        *start_ms <= entry.at_ms
                            && entry.at_ms < *end_ms
                            && entry.node == *node
                            && entry.effect == wanted
                    })
                    .count();
                if actual != usize::try_from(*count).map_err(|_| RunnerError::OracleMismatch)? {
                    return Err(RunnerError::OracleMismatch);
                }
            }
            Expectation::Converged {
                when: ExpectationTime::AtMs { at_ms },
                nodes,
            } => {
                let states = snapshots
                    .iter()
                    .rev()
                    .find(|(time, _)| *time <= *at_ms)
                    .map(|(_, states)| states)
                    .ok_or(RunnerError::MissingNode)?;
                if !states_converged(states, nodes)? {
                    return Err(RunnerError::OracleMismatch);
                }
            }
            Expectation::Converged {
                when: ExpectationTime::Throughout { start_ms, end_ms },
                nodes,
            } => {
                for (at_ms, states) in snapshots.iter().skip(1) {
                    if *start_ms <= *at_ms && *at_ms < *end_ms && !states_converged(states, nodes)?
                    {
                        return Err(RunnerError::OracleMismatch);
                    }
                }
            }
        }
    }
    Ok(())
}

fn states_converged(
    states: &BTreeMap<String, State>,
    nodes: &[String],
) -> Result<bool, RunnerError> {
    let Some(first_node) = nodes.first() else {
        return Ok(false);
    };
    let first = retained_record_ids(states.get(first_node).ok_or(RunnerError::MissingNode)?);
    for node in &nodes[1..] {
        if retained_record_ids(states.get(node).ok_or(RunnerError::MissingNode)?) != first {
            return Ok(false);
        }
    }
    Ok(true)
}

fn retained_record_ids(state: &State) -> BTreeSet<&RecordId> {
    state
        .persisted()
        .retained
        .values()
        .flat_map(BTreeMap::keys)
        .collect()
}

fn state_assertion_matches(state: &State, assertion: &StateAssertion) -> Result<bool, RunnerError> {
    match assertion {
        StateAssertion::Retained { record_id, present } => {
            let id = convert_record_id(record_id)?;
            let actual = state
                .persisted()
                .retained
                .get(&id.stream)
                .is_some_and(|records| records.contains_key(&id));
            Ok(actual == *present)
        }
        StateAssertion::Clock { value } => {
            let actual = state.clock();
            Ok(match value {
                crate::schema::ClockStateSpec::Ready { watermark } => matches!(
                    actual,
                    glade_discover_core::ClockState::Ready { watermark: WallMs(actual) }
                        if actual == *watermark
                ),
                crate::schema::ClockStateSpec::Uncertain { floor } => match actual {
                    glade_discover_core::ClockState::Uncertain { floor: actual } => {
                        actual.map(|value| value.0) == *floor
                    }
                    glade_discover_core::ClockState::Ready { .. } => false,
                },
            })
        }
        StateAssertion::Storage {
            retained_bytes,
            exhausted,
        } => Ok(state.persisted().retained_bytes == *retained_bytes
            && (state.persisted().retained_bytes >= state.config().max_retained_bytes.get())
                == *exhausted),
        StateAssertion::Unresolved {
            grant_id,
            claim_id,
            present,
        } => {
            let grant = GrantId::from(convert_record_id(grant_id)?);
            let claim = glade_discover_protocol::ClaimId::from(convert_record_id(claim_id)?);
            let actual = state
                .persisted()
                .unresolved
                .get(&grant)
                .is_some_and(|claims| claims.contains(&claim));
            Ok(actual == *present)
        }
        StateAssertion::Mine {
            slot,
            generation,
            status,
        } => {
            let actual = state
                .persisted()
                .mine
                .get(&(convert_slot(slot)?, Generation(*generation)))
                .map(|claim| claim.status.clone());
            let expected = match status {
                MineStatusSpec::Pending => MineStatus::Pending,
                MineStatusSpec::Accepted => MineStatus::Accepted,
                MineStatusSpec::Lost => MineStatus::Lost,
            };
            Ok(actual == Some(expected))
        }
        StateAssertion::Sync {
            peer,
            sync_id,
            status,
        } => {
            let actual = state
                .sync()
                .get(&(NodeId::from(peer.clone()), SyncId::from(sync_id.clone())));
            Ok(matches!(
                (status, actual),
                (SyncStatusSpec::Active, Some(RoundProgress::Active { .. }))
                    | (SyncStatusSpec::Complete, Some(RoundProgress::Complete))
                    | (SyncStatusSpec::Stale, Some(RoundProgress::Stale))
            ))
        }
    }
}

fn state_at<'a>(
    snapshots: &'a [(u64, BTreeMap<String, State>)],
    at_ms: u64,
    node: &str,
) -> Result<&'a State, RunnerError> {
    snapshots
        .iter()
        .rev()
        .find(|(time, _)| *time <= at_ms)
        .and_then(|(_, states)| states.get(node))
        .ok_or(RunnerError::MissingNode)
}

fn convert_effect_spec(effect: &EffectSpec) -> Result<Effect, RunnerError> {
    match effect {
        EffectSpec::Reply { ingress, corr, ans } => Ok(expected_reply(ingress, corr, ans)),
        EffectSpec::Gossip { to, msg } => Ok(Effect::Gossip {
            to: NodeId::from(to.clone()),
            msg: Box::new(convert_wire_msg(msg, None)?.0),
        }),
        EffectSpec::Append {
            intent,
            slot,
            generation,
            draft,
        } => Ok(Effect::Append {
            intent: IntentId::from(intent.clone()),
            slot: convert_slot(slot)?,
            generation: Generation(*generation),
            draft: Box::new(convert_claim_draft(draft)?),
        }),
        EffectSpec::Schedule { token, at_mono } => Ok(Effect::Schedule {
            token: convert_wake_token(token)?,
            at_mono: MonoInstant(*at_mono),
        }),
        EffectSpec::Teardown { slot, generation } => Ok(Effect::Teardown {
            slot: convert_slot(slot)?,
            generation: glade_discover_protocol::Generation(*generation),
        }),
    }
}

fn expected_reply(ingress: &str, corr: &str, ans: &RouteAnsSpec) -> Effect {
    Effect::Reply {
        ingress: IngressId::from(ingress),
        corr: Corr::from(corr),
        ans: match ans {
            RouteAnsSpec::Matched { node } => RouteAns::Matched {
                node: NodeId::from(node.clone()),
            },
            RouteAnsSpec::NoClaim => RouteAns::NoClaim,
        },
    }
}

fn convert_slot(slot: &SlotSpec) -> Result<Slot, RunnerError> {
    match slot {
        SlotSpec::Workspace { share } => Ok(Slot::Workspace {
            share: share.clone(),
        }),
        SlotSpec::Binding {
            share,
            glade_id,
            key_hex,
        } => Ok(Slot::Binding {
            share: share.clone(),
            glade_id: glade_id.clone(),
            key: decode_hex(key_hex)?,
        }),
    }
}

fn convert_record_id(record: &RecordIdSpec) -> Result<RecordId, RunnerError> {
    Ok(RecordId {
        stream: convert_stream(&record.stream)?,
        origin: Principal::from(record.origin.clone()),
        seq: record.seq,
    })
}

fn convert_stream(stream: &crate::schema::StreamIdSpec) -> Result<StreamId, RunnerError> {
    Ok(StreamId {
        share: stream.share.clone(),
        glade_id: stream.glade_id.clone(),
        key: decode_hex(&stream.key_hex)?,
    })
}

fn decode_hex(value: &str) -> Result<Vec<u8>, RunnerError> {
    if value.len() % 2 != 0 {
        return Err(RunnerError::ProtocolDecode);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = core::str::from_utf8(pair).map_err(|_| RunnerError::ProtocolDecode)?;
            u8::from_str_radix(text, 16).map_err(|_| RunnerError::ProtocolDecode)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use glade_discover_protocol::{
        ClaimDraft, ClaimIdentity, Generation, IntentId, NodeId, Principal, RecordId, Slot,
        StreamId,
    };

    use crate::schema::VerifierTable;

    use super::SimAppendEnvironment;

    #[test]
    fn repeated_auto_append_reuses_the_exact_persisted_event_without_reminting() {
        let slot = Slot::Workspace {
            share: "workspace-a".to_owned(),
        };
        let draft = ClaimDraft::Workspace {
            node: NodeId::from("node-a"),
            share: "workspace-a".to_owned(),
            identity: ClaimIdentity::Mint,
            grant_ref: glade_discover_protocol::GrantId::from(RecordId {
                stream: StreamId {
                    share: "workspace-a".to_owned(),
                    glade_id: "directory".to_owned(),
                    key: vec![0xa1],
                },
                origin: Principal::from("owner-a"),
                seq: 0,
            }),
            lease_expiry_ms: 500_000,
            epoch: 0,
        };
        let intent = IntentId::from("publish-a");
        let origin = Principal::from("owner-a");
        let verifier = VerifierTable::from_fixtures(&[]);
        let mut environment = SimAppendEnvironment::default();

        let first = environment
            .observe_append(
                "node-a",
                &intent,
                &slot,
                Generation(1),
                &draft,
                &origin,
                &verifier,
            )
            .expect("first append")
            .expect("generated callback");
        let replay = environment
            .observe_append(
                "node-a",
                &intent,
                &slot,
                Generation(1),
                &draft,
                &origin,
                &verifier,
            )
            .expect("retry append")
            .expect("replayed callback");

        assert_eq!(replay, first);
        assert_eq!(
            environment.streams.values().map(Vec::len).sum::<usize>(),
            1,
            "retry must not allocate another sequence or signed op"
        );
    }
}
