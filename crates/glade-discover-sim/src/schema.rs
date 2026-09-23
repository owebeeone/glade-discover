use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::{NonZeroU16, NonZeroU64};

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub seed: u64,
    pub event_budget: NonZeroU64,
    pub nodes: Vec<NodeSpec>,
    pub links: Vec<LinkSpec>,
    pub inputs: Vec<ScenarioInput>,
    pub verifier_fixtures: Vec<VerifierFixture>,
    pub stop: StopSpec,
    pub expect: Vec<Expectation>,
}

impl Scenario {
    pub fn from_json(input: &str) -> Result<Self, ScenarioDecodeError> {
        let scenario: Self = serde_json::from_str(input).map_err(ScenarioDecodeError::Json)?;
        scenario.validate()?;
        Ok(scenario)
    }

    fn validate(&self) -> Result<(), ScenarioDecodeError> {
        let mut nodes = BTreeSet::new();
        for node in &self.nodes {
            if node.id.is_empty() || !nodes.insert(node.id.as_str()) {
                return invalid("node ids must be non-empty and unique");
            }
            validate_clock(&node.clock)?;
        }
        if nodes.is_empty() {
            return invalid("scenario must declare at least one node");
        }
        if self.expect.is_empty() {
            return invalid("scenario must declare at least one expectation");
        }

        for link in &self.links {
            for fault in &link.faults {
                validate_fault(fault)?;
            }
            if !nodes.contains(link.a.as_str()) || !nodes.contains(link.b.as_str()) {
                return invalid("link references an unknown node");
            }
        }

        let horizon = match self.stop {
            StopSpec::AtMs { at_ms } => at_ms,
            StopSpec::QuiescentForMs {
                quiet_ms,
                deadline_ms,
            } => {
                if quiet_ms.get() > deadline_ms {
                    return invalid("quiet interval must fit before its deadline");
                }
                deadline_ms
            }
        };
        let mut inputs = BTreeMap::new();
        for input in &self.inputs {
            if input.input_id.is_empty() || inputs.insert(input.input_id.as_str(), input).is_some()
            {
                return invalid("duplicate input_id or empty input_id");
            }
            if !nodes.contains(input.node.as_str()) {
                return invalid("input references an unknown node");
            }
            if input.at_ms > horizon {
                return invalid("input occurs beyond the stop deadline");
            }
            validate_event(&input.event)?;
        }

        let mut fixture_keys = BTreeSet::new();
        for fixture in &self.verifier_fixtures {
            if let Some(reader) = fixture.reader.as_deref() {
                if !nodes.contains(reader) {
                    return invalid("verifier fixture reader references an unknown node");
                }
            }
            if !fixture_keys.insert((&fixture.selector, fixture.reader.as_deref())) {
                return invalid("duplicate verifier fixture key");
            }
            match &fixture.selector {
                OpSelector::Input {
                    input_id,
                    op_ordinal,
                } => {
                    let Some(input) = inputs.get(input_id.as_str()) else {
                        return invalid("op selector references an unknown input");
                    };
                    if *op_ordinal >= input.event.op_count() {
                        return invalid("op selector is op-free or out of range");
                    }
                }
                OpSelector::Minted { node, slot, .. } => {
                    if !nodes.contains(node.as_str()) || !slot.is_valid() {
                        return invalid("invalid minted op selector");
                    }
                }
            }
        }

        for expectation in &self.expect {
            validate_expectation(expectation, horizon, &nodes)?;
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum ScenarioDecodeError {
    Json(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for ScenarioDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
            Self::Invalid(message) => message.fmt(formatter),
        }
    }
}

impl std::error::Error for ScenarioDecodeError {}

fn invalid<T>(message: &str) -> Result<T, ScenarioDecodeError> {
    Err(ScenarioDecodeError::Invalid(message.to_owned()))
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    pub id: String,
    pub principal: String,
    pub owns: Vec<String>,
    pub seed_grants: Vec<SignedOpSpec>,
    #[serde(default = "default_max_retained_bytes")]
    pub max_retained_bytes: NonZeroU64,
    pub clock: ClockSpec,
    pub restarts: Vec<RestartSpec>,
}

fn default_max_retained_bytes() -> NonZeroU64 {
    NonZeroU64::new(16 * 1024 * 1024).expect("constant is non-zero")
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClockSpec {
    pub initial_wall_ms: i64,
    pub initial_mono_ms: u64,
    pub changes: Vec<ClockChange>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockChange {
    SetOffset {
        at_ms: u64,
        offset_ms: i64,
    },
    Drift {
        start_ms: u64,
        end_ms: u64,
        rate_ppm: i32,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RestartSpec {
    pub at_ms: u64,
    pub watermark: RestartWatermark,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RestartWatermark {
    Preserve,
    Unreadable,
    Replace { value: i64 },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LinkSpec {
    pub a: String,
    pub b: String,
    pub latency_ms: u64,
    pub faults: Vec<LinkFault>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LinkFault {
    Loss {
        start_ms: u64,
        end_ms: u64,
        probability_ppm: u32,
    },
    Partition {
        start_ms: u64,
        end_ms: u64,
    },
    Reorder {
        start_ms: u64,
        end_ms: u64,
        extra_delay_ms: u64,
    },
    Duplicate {
        start_ms: u64,
        end_ms: u64,
        copies: NonZeroU16,
        spacing_ms: u64,
    },
    Latency {
        start_ms: u64,
        end_ms: u64,
        latency_ms: u64,
    },
}

impl LinkFault {
    const fn interval(&self) -> (u64, u64) {
        match *self {
            Self::Loss {
                start_ms, end_ms, ..
            }
            | Self::Partition { start_ms, end_ms }
            | Self::Reorder {
                start_ms, end_ms, ..
            }
            | Self::Duplicate {
                start_ms, end_ms, ..
            }
            | Self::Latency {
                start_ms, end_ms, ..
            } => (start_ms, end_ms),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScenarioInput {
    pub input_id: String,
    pub at_ms: u64,
    pub node: String,
    pub event: ScenarioEvent,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScenarioEvent {
    Deliver {
        from: String,
        msg: WireMsgSpec,
    },
    Route {
        ingress: String,
        principal: String,
        authenticated_context_hex: String,
        corr: String,
        query: SlotSpec,
    },
    Advertise {
        command: Box<ClaimCommandSpec>,
    },
    OpAccepted {
        intent: String,
        op: SignedOpSpec,
    },
    Wakeup {
        token: WakeTokenSpec,
    },
    ClockReseed {
        watermark: i64,
    },
}

impl ScenarioEvent {
    fn op_count(&self) -> u32 {
        match self {
            Self::Deliver { msg, .. } => msg.op_count(),
            Self::OpAccepted { .. } => 1,
            Self::Route { .. }
            | Self::Advertise { .. }
            | Self::Wakeup { .. }
            | Self::ClockReseed { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireMsgSpec {
    DirOp {
        op: SignedOpSpec,
    },
    SyncStart {
        sync_id: String,
        heads: Vec<StreamHeadSpec>,
    },
    SyncOps {
        sync_id: String,
        ops: Vec<SignedOpSpec>,
    },
    SyncEnd {
        sync_id: String,
    },
}

impl WireMsgSpec {
    fn op_count(&self) -> u32 {
        match self {
            Self::DirOp { .. } => 1,
            Self::SyncOps { ops, .. } => u32::try_from(ops.len()).unwrap_or(u32::MAX),
            Self::SyncStart { .. } | Self::SyncEnd { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SignedOpSpec {
    pub canonical_hex: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StreamHeadSpec {
    pub stream: StreamIdSpec,
    pub origin: String,
    pub seq: u64,
    pub hash_hex: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Ord, PartialOrd)]
#[serde(deny_unknown_fields)]
pub struct StreamIdSpec {
    pub share: String,
    pub glade_id: String,
    pub key_hex: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Ord, PartialOrd)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SlotSpec {
    Workspace {
        share: String,
    },
    Binding {
        share: String,
        glade_id: String,
        key_hex: String,
    },
}

impl SlotSpec {
    fn is_valid(&self) -> bool {
        match self {
            Self::Workspace { share } => !share.is_empty(),
            Self::Binding {
                share, glade_id, ..
            } => !share.is_empty() && !glade_id.is_empty(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClaimCommandSpec {
    pub intent: String,
    pub slot: SlotSpec,
    pub generation: u64,
    pub mode: ClaimModeSpec,
    pub draft: ClaimDraftSpec,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClaimModeSpec {
    Initial,
    Renew,
    Takeover { authority_ref: RecordIdSpec },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClaimDraftSpec {
    Workspace {
        node: String,
        share: String,
        identity: ClaimIdentitySpec,
        grant_ref: RecordIdSpec,
        lease_expiry_ms: i64,
        epoch: u64,
    },
    Service {
        node: String,
        share: String,
        glade_id: String,
        key_hex: String,
        identity: ClaimIdentitySpec,
        def_ref: RecordIdSpec,
        exec_grant_ref: RecordIdSpec,
        compute_key_hex: String,
        lease_expiry_ms: i64,
        epoch: u64,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClaimIdentitySpec {
    Mint,
    Existing { claim_id: RecordIdSpec },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Ord, PartialOrd)]
#[serde(deny_unknown_fields)]
pub struct RecordIdSpec {
    pub stream: StreamIdSpec,
    pub origin: String,
    pub seq: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WakeTokenSpec {
    ClaimRenew {
        slot: SlotSpec,
        generation: u64,
    },
    AppendRetry {
        slot: SlotSpec,
        generation: u64,
        intent: String,
    },
    GossipTick {
        peer: String,
    },
    SyncTimeout {
        peer: String,
        sync_id: String,
        attempt: u8,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Ord, PartialOrd)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpSelector {
    Input {
        input_id: String,
        op_ordinal: u32,
    },
    Minted {
        node: String,
        slot: SlotSpec,
        generation: u64,
        append_ordinal: u32,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VerifierFixture {
    pub selector: OpSelector,
    pub reader: Option<String>,
    pub outcome: VerificationOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VerificationOutcome {
    Valid { signer: String },
    BadSignature,
}

pub(crate) struct VerifierTable {
    entries: BTreeMap<(OpSelector, Option<String>), VerificationOutcome>,
}

impl VerifierTable {
    pub(crate) fn from_fixtures(fixtures: &[VerifierFixture]) -> Self {
        let entries = fixtures
            .iter()
            .map(|fixture| {
                (
                    (fixture.selector.clone(), fixture.reader.clone()),
                    fixture.outcome.clone(),
                )
            })
            .collect();
        Self { entries }
    }

    pub(crate) fn outcome(
        &self,
        selector: &OpSelector,
        reader: &str,
        envelope_signer: &str,
    ) -> VerificationOutcome {
        self.entries
            .get(&(selector.clone(), Some(reader.to_owned())))
            .or_else(|| self.entries.get(&(selector.clone(), None)))
            .cloned()
            .unwrap_or_else(|| VerificationOutcome::Valid {
                signer: envelope_signer.to_owned(),
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SelectorError {
    IntentMismatch,
    UnknownIntent,
    EmptyCanonicalBytes,
    BindingMismatch,
    OrdinalExhausted,
}

pub(crate) struct MintedSelectorRegistry<P> {
    next_ordinals: BTreeMap<(String, SlotSpec, u64), u32>,
    intents: BTreeMap<String, (OpSelector, P)>,
    intent_bindings: BTreeMap<String, Vec<u8>>,
    bindings: BTreeMap<Vec<u8>, OpSelector>,
}

impl<P> Default for MintedSelectorRegistry<P> {
    fn default() -> Self {
        Self {
            next_ordinals: BTreeMap::new(),
            intents: BTreeMap::new(),
            intent_bindings: BTreeMap::new(),
            bindings: BTreeMap::new(),
        }
    }
}

impl<P: Clone + Eq> MintedSelectorRegistry<P> {
    pub(crate) fn observe_append(
        &mut self,
        node: &str,
        slot: &SlotSpec,
        generation: u64,
        intent: &str,
        payload: &P,
    ) -> Result<OpSelector, SelectorError> {
        if let Some((selector, established_payload)) = self.intents.get(intent) {
            let matches_scope = matches!(
                selector,
                OpSelector::Minted {
                    node: established_node,
                    slot: established_slot,
                    generation: established_generation,
                    ..
                } if established_node == node
                    && established_slot == slot
                    && *established_generation == generation
            );
            return if matches_scope && established_payload == payload {
                Ok(selector.clone())
            } else {
                Err(SelectorError::IntentMismatch)
            };
        }

        let ordinal_key = (node.to_owned(), slot.clone(), generation);
        let append_ordinal = self.next_ordinals.get(&ordinal_key).copied().unwrap_or(0);
        let next = append_ordinal
            .checked_add(1)
            .ok_or(SelectorError::OrdinalExhausted)?;
        let selector = OpSelector::Minted {
            node: node.to_owned(),
            slot: slot.clone(),
            generation,
            append_ordinal,
        };
        self.next_ordinals.insert(ordinal_key, next);
        self.intents
            .insert(intent.to_owned(), (selector.clone(), payload.clone()));
        Ok(selector)
    }

    pub(crate) fn bind_persisted(
        &mut self,
        intent: &str,
        canonical_bytes: &[u8],
    ) -> Result<OpSelector, SelectorError> {
        if canonical_bytes.is_empty() {
            return Err(SelectorError::EmptyCanonicalBytes);
        }
        let (selector, _) = self
            .intents
            .get(intent)
            .ok_or(SelectorError::UnknownIntent)?;
        if let Some(established_bytes) = self.intent_bindings.get(intent) {
            return if established_bytes.as_slice() == canonical_bytes {
                Ok(selector.clone())
            } else {
                Err(SelectorError::BindingMismatch)
            };
        }
        if let Some(established_selector) = self.bindings.get(canonical_bytes) {
            if established_selector != selector {
                return Err(SelectorError::BindingMismatch);
            }
        }
        self.intent_bindings
            .insert(intent.to_owned(), canonical_bytes.to_vec());
        self.bindings
            .insert(canonical_bytes.to_vec(), selector.clone());
        Ok(selector.clone())
    }

    pub(crate) fn selector_for_bytes(&self, canonical_bytes: &[u8]) -> Option<&OpSelector> {
        self.bindings.get(canonical_bytes)
    }

    pub(crate) fn unfulfilled<'a>(
        &'a self,
        fixtures: &'a [VerifierFixture],
    ) -> Vec<&'a OpSelector> {
        let fulfilled: BTreeSet<&OpSelector> = self.bindings.values().collect();
        fixtures
            .iter()
            .filter_map(|fixture| match &fixture.selector {
                selector @ OpSelector::Minted { .. } if !fulfilled.contains(selector) => {
                    Some(selector)
                }
                OpSelector::Input { .. } | OpSelector::Minted { .. } => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StopSpec {
    AtMs {
        at_ms: u64,
    },
    QuiescentForMs {
        quiet_ms: NonZeroU64,
        deadline_ms: u64,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectationTime {
    AtMs { at_ms: u64 },
    Throughout { start_ms: u64, end_ms: u64 },
}

impl ExpectationTime {
    const fn interval(&self) -> Option<(u64, u64)> {
        match *self {
            Self::AtMs { .. } => None,
            Self::Throughout { start_ms, end_ms } => Some((start_ms, end_ms)),
        }
    }

    const fn end(&self) -> u64 {
        match *self {
            Self::AtMs { at_ms } => at_ms,
            Self::Throughout { end_ms, .. } => end_ms,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expectation {
    Converged {
        when: ExpectationTime,
        nodes: Vec<String>,
    },
    Route {
        when: ExpectationTime,
        node: String,
        ingress: String,
        corr: String,
        ans: RouteAnsSpec,
    },
    Effect {
        when: ExpectationTime,
        node: String,
        effect: Box<EffectSpec>,
        count: u32,
    },
    State {
        when: ExpectationTime,
        node: String,
        assertion: StateAssertion,
    },
}

impl Expectation {
    const fn when(&self) -> &ExpectationTime {
        match self {
            Self::Converged { when, .. }
            | Self::Route { when, .. }
            | Self::Effect { when, .. }
            | Self::State { when, .. } => when,
        }
    }

    fn referenced_nodes(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Converged { nodes, .. } => Box::new(nodes.iter().map(String::as_str)),
            Self::Route { node, .. } | Self::Effect { node, .. } | Self::State { node, .. } => {
                Box::new(std::iter::once(node.as_str()))
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteAnsSpec {
    Matched { node: String },
    NoClaim,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectSpec {
    Gossip {
        to: String,
        msg: WireMsgSpec,
    },
    Append {
        intent: String,
        slot: SlotSpec,
        generation: u64,
        draft: Box<ClaimDraftSpec>,
    },
    Reply {
        ingress: String,
        corr: String,
        ans: RouteAnsSpec,
    },
    Schedule {
        token: WakeTokenSpec,
        at_mono: u64,
    },
    Teardown {
        slot: SlotSpec,
        generation: u64,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateAssertion {
    Clock {
        value: ClockStateSpec,
    },
    Retained {
        record_id: RecordIdSpec,
        present: bool,
    },
    Unresolved {
        grant_id: RecordIdSpec,
        claim_id: RecordIdSpec,
        present: bool,
    },
    Mine {
        slot: SlotSpec,
        generation: u64,
        status: MineStatusSpec,
    },
    Sync {
        peer: String,
        sync_id: String,
        status: SyncStatusSpec,
    },
    Storage {
        retained_bytes: u64,
        exhausted: bool,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockStateSpec {
    Ready { watermark: i64 },
    Uncertain { floor: Option<i64> },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MineStatusSpec {
    Pending,
    Accepted,
    Lost,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatusSpec {
    Active,
    Complete,
    Stale,
}

fn validate_clock(clock: &ClockSpec) -> Result<(), ScenarioDecodeError> {
    let mut drift_intervals = Vec::new();
    for change in &clock.changes {
        if let ClockChange::Drift {
            start_ms, end_ms, ..
        } = *change
        {
            validate_interval(start_ms, end_ms)?;
            if drift_intervals
                .iter()
                .any(|&(start, end)| start_ms < end && start < end_ms)
            {
                return invalid("clock drift intervals must not overlap");
            }
            drift_intervals.push((start_ms, end_ms));
        }
    }
    Ok(())
}

fn validate_fault(fault: &LinkFault) -> Result<(), ScenarioDecodeError> {
    let (start, end) = fault.interval();
    validate_interval(start, end)?;
    if let LinkFault::Loss {
        probability_ppm, ..
    } = fault
    {
        if *probability_ppm > 1_000_000 {
            return invalid("loss probability_ppm exceeds one million");
        }
    }
    Ok(())
}

fn validate_interval(start: u64, end: u64) -> Result<(), ScenarioDecodeError> {
    if start >= end {
        return invalid("half-open interval must satisfy start_ms < end_ms");
    }
    Ok(())
}

fn validate_event(event: &ScenarioEvent) -> Result<(), ScenarioDecodeError> {
    match event {
        ScenarioEvent::Route {
            authenticated_context_hex,
            query,
            ..
        } => {
            if !is_hex(authenticated_context_hex) || !query.is_valid() {
                return invalid("route contains invalid hex or slot data");
            }
        }
        ScenarioEvent::Deliver { msg, .. } => validate_wire(msg)?,
        ScenarioEvent::OpAccepted { op, .. } => validate_signed_op(op)?,
        ScenarioEvent::Advertise { command } => {
            if !command.slot.is_valid() {
                return invalid("advertise command has invalid slot");
            }
        }
        ScenarioEvent::Wakeup { .. } | ScenarioEvent::ClockReseed { .. } => {}
    }
    Ok(())
}

fn validate_wire(message: &WireMsgSpec) -> Result<(), ScenarioDecodeError> {
    match message {
        WireMsgSpec::DirOp { op } => validate_signed_op(op),
        WireMsgSpec::SyncOps { ops, .. } => {
            for op in ops {
                validate_signed_op(op)?;
            }
            Ok(())
        }
        WireMsgSpec::SyncStart { heads, .. } => {
            for head in heads {
                if head.hash_hex.len() != 64 || !is_hex(&head.hash_hex) {
                    return invalid("stream head hash must be exactly 32 bytes of hex");
                }
            }
            Ok(())
        }
        WireMsgSpec::SyncEnd { .. } => Ok(()),
    }
}

fn validate_signed_op(op: &SignedOpSpec) -> Result<(), ScenarioDecodeError> {
    if op.canonical_hex.is_empty() || !is_hex(&op.canonical_hex) {
        return invalid("signed op must contain non-empty canonical hex");
    }
    Ok(())
}

fn validate_expectation(
    expectation: &Expectation,
    horizon: u64,
    nodes: &BTreeSet<&str>,
) -> Result<(), ScenarioDecodeError> {
    let when = expectation.when();
    if let Some((start, end)) = when.interval() {
        validate_interval(start, end)?;
    }
    if when.end() > horizon {
        return invalid("expectation occurs beyond the stop deadline");
    }
    for node in expectation.referenced_nodes() {
        if !nodes.contains(node) {
            return invalid("expectation references an unknown node");
        }
    }
    Ok(())
}

fn is_hex(value: &str) -> bool {
    value.len() % 2 == 0 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{
        MintedSelectorRegistry, OpSelector, Scenario, SelectorError, SlotSpec, VerificationOutcome,
        VerifierFixture, VerifierTable,
    };

    fn scenario_with(overrides: &str) -> String {
        format!(
            r#"{{
                "seed": 7,
                "event_budget": 100,
                "nodes": [{{
                    "id": "node-a",
                    "principal": "principal-a",
                    "owns": [],
                    "seed_grants": [],
                    "clock": {{
                        "initial_wall_ms": 1000,
                        "initial_mono_ms": 0,
                        "changes": []
                    }},
                    "restarts": []
                }}],
                "links": [],
                "inputs": [{{
                    "input_id": "op-1",
                    "at_ms": 1,
                    "node": "node-a",
                    "event": {{
                        "kind": "deliver",
                        "from": "node-a",
                        "msg": {{"kind": "dir_op", "op": {{"canonical_hex": "00"}}}}
                    }}
                }}],
                "verifier_fixtures": [],
                "stop": {{"kind": "at_ms", "at_ms": 10}},
                "expect": [{{
                    "when": {{"kind": "at_ms", "at_ms": 10}},
                    "kind": "converged",
                    "nodes": ["node-a"]
                }}]
                {overrides}
            }}"#
        )
    }

    #[test]
    fn rejects_dangling_fixture_reader() {
        let json = scenario_with("").replace(
            r#""verifier_fixtures": []"#,
            r#""verifier_fixtures": [{
                "selector": {"kind": "input", "input_id": "op-1", "op_ordinal": 0},
                "reader": "node-missing",
                "outcome": {"kind": "bad_signature"}
            }]"#,
        );

        let error = Scenario::from_json(&json).expect_err("reader must name a scenario node");
        assert!(error.to_string().contains("reader"));
    }

    #[test]
    fn valid_fixture_requires_an_explicit_signer() {
        let json = scenario_with("").replace(
            r#""verifier_fixtures": []"#,
            r#""verifier_fixtures": [{
                "selector": {"kind": "input", "input_id": "op-1", "op_ordinal": 0},
                "reader": null,
                "outcome": {"kind": "valid"}
            }]"#,
        );

        assert!(Scenario::from_json(&json).is_err());
    }

    #[test]
    fn quiescent_stop_must_fit_before_its_deadline() {
        let json = scenario_with("").replace(
            r#""stop": {"kind": "at_ms", "at_ms": 10}"#,
            r#""stop": {"kind": "quiescent_for_ms", "quiet_ms": 11, "deadline_ms": 10}"#,
        );

        let error = Scenario::from_json(&json).expect_err("impossible quiet interval must fail");
        assert!(error.to_string().contains("quiet"));
    }

    #[test]
    fn rejects_overlapping_clock_drift_intervals() {
        let json = scenario_with("").replace(
            r#""changes": []"#,
            r#""changes": [
                {"kind": "drift", "start_ms": 1, "end_ms": 5, "rate_ppm": 10},
                {"kind": "drift", "start_ms": 4, "end_ms": 8, "rate_ppm": -10}
            ]"#,
        );

        let error = Scenario::from_json(&json).expect_err("drift intervals must not overlap");
        assert!(error.to_string().contains("must not overlap"));
    }

    fn workspace_selector(append_ordinal: u32) -> OpSelector {
        OpSelector::Minted {
            node: "node-a".to_owned(),
            slot: SlotSpec::Workspace {
                share: "share-a".to_owned(),
            },
            generation: 3,
            append_ordinal,
        }
    }

    #[test]
    fn reader_specific_fixture_replaces_global_fixture() {
        let selector = workspace_selector(0);
        let table = VerifierTable::from_fixtures(&[
            VerifierFixture {
                selector: selector.clone(),
                reader: None,
                outcome: VerificationOutcome::BadSignature,
            },
            VerifierFixture {
                selector: selector.clone(),
                reader: Some("node-b".to_owned()),
                outcome: VerificationOutcome::Valid {
                    signer: "specific-signer".to_owned(),
                },
            },
        ]);

        assert_eq!(
            table.outcome(&selector, "node-b", "envelope"),
            VerificationOutcome::Valid {
                signer: "specific-signer".to_owned(),
            }
        );
        assert_eq!(
            table.outcome(&selector, "node-c", "envelope"),
            VerificationOutcome::BadSignature
        );
        assert_eq!(
            table.outcome(&workspace_selector(9), "node-c", "envelope"),
            VerificationOutcome::Valid {
                signer: "envelope".to_owned(),
            }
        );
    }

    #[test]
    fn minted_selector_allocates_once_per_distinct_intent() {
        let mut registry = MintedSelectorRegistry::default();
        let slot = SlotSpec::Workspace {
            share: "share-a".to_owned(),
        };

        let first = registry
            .observe_append("node-a", &slot, 3, "intent-a", &"payload-a")
            .unwrap();
        assert_eq!(
            registry
                .observe_append("node-a", &slot, 3, "intent-a", &"payload-a")
                .unwrap(),
            first
        );
        assert_eq!(
            registry
                .observe_append("node-a", &slot, 3, "intent-b", &"payload-b")
                .unwrap(),
            workspace_selector(1)
        );
        assert_eq!(
            registry.observe_append("node-a", &slot, 3, "intent-a", &"changed"),
            Err(SelectorError::IntentMismatch)
        );
    }

    #[test]
    fn minted_selector_resolves_only_on_persisted_byte_binding() {
        let mut registry = MintedSelectorRegistry::default();
        let slot = SlotSpec::Workspace {
            share: "share-a".to_owned(),
        };
        let selector = registry
            .observe_append("node-a", &slot, 3, "intent-a", &"payload-a")
            .unwrap();
        let fixtures = vec![VerifierFixture {
            selector: selector.clone(),
            reader: None,
            outcome: VerificationOutcome::BadSignature,
        }];

        assert_eq!(registry.unfulfilled(&fixtures), vec![&selector]);
        assert_eq!(
            registry.bind_persisted("intent-a", b"canonical").unwrap(),
            selector
        );
        assert_eq!(registry.selector_for_bytes(b"canonical"), Some(&selector));
        assert!(registry.unfulfilled(&fixtures).is_empty());
    }
}
