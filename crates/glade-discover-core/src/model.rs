use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_protocol::{
    ClaimDraft, ClaimId, Corr, Generation, GrantId, IngressId, IntentId, NodeId, Principal,
    RecordId, RouteQuery, SignedOp, Slot, StreamId, SyncId, WireMsg,
};

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct WallMs(pub i64);

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonoInstant(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StepCtx {
    pub mono: MonoInstant,
    pub wall: WallMs,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PrincipalPlane {
    Workspace,
    Derived,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NodePrincipalBinding {
    pub node: NodeId,
    pub principal: Principal,
    pub plane: PrincipalPlane,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KernelConfig {
    pub local_node: NodeId,
    pub local_principal: Principal,
    pub peers: BTreeSet<NodeId>,
    pub workspace_owner_roots: BTreeMap<String, Principal>,
    pub node_principal_bindings: BTreeSet<NodePrincipalBinding>,
    pub skew_margin_ms: u64,
    pub max_lease_ms: u64,
    pub clock_resync_ms: u64,
    pub sync_retries: u8,
    pub sync_timeout_ms: NonZeroU64,
    pub gossip_fan: u8,
    pub max_retained_bytes: NonZeroU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockState {
    Ready { watermark: WallMs },
    Uncertain { floor: Option<WallMs> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatermarkLoad {
    Readable(WallMs),
    Unreadable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MineStatus {
    Pending,
    Accepted,
    Lost,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnClaim {
    pub intent: IntentId,
    pub draft: ClaimDraft,
    pub status: MineStatus,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PersistedState {
    pub retained: BTreeMap<StreamId, BTreeMap<RecordId, SignedOp>>,
    pub retained_bytes: u64,
    pub time_deferred: BTreeSet<RecordId>,
    pub unresolved: BTreeMap<GrantId, BTreeSet<ClaimId>>,
    pub mine: BTreeMap<(Slot, Generation), OwnClaim>,
    pub next_sync: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoundProgress {
    Active { attempt: u8 },
    Complete,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    config: KernelConfig,
    persisted: PersistedState,
    sync: BTreeMap<(NodeId, SyncId), RoundProgress>,
    clock: ClockState,
}

impl State {
    #[must_use]
    pub const fn clock(&self) -> ClockState {
        self.clock
    }

    #[must_use]
    pub const fn config(&self) -> &KernelConfig {
        &self.config
    }

    #[must_use]
    pub const fn persisted(&self) -> &PersistedState {
        &self.persisted
    }

    #[must_use]
    pub const fn sync(&self) -> &BTreeMap<(NodeId, SyncId), RoundProgress> {
        &self.sync
    }
}

#[must_use]
pub fn restore_fresh(
    config: KernelConfig,
    persisted: PersistedState,
    watermark_load: WatermarkLoad,
    ctx: StepCtx,
) -> State {
    let (state, recovery) = restore_with_recovery(config, persisted, watermark_load, ctx);
    assert!(
        recovery.is_empty(),
        "restore_fresh cannot discard durable pending recovery work"
    );
    state
}

/// Restores durable state and reconstructs deterministic local retry schedules.
///
/// Pending appends are already durable in `mine`; the returned one-shot
/// schedules are idempotent because every token retains the exact
/// `(slot,generation,intent)` key. Ready-clock restore also reconciles accepted
/// derived services, marks expired or losing instances `Lost`, and returns the
/// corresponding teardown effects. An uncertain clock remains fail-closed and
/// does not tear down solely because no live winner can be projected.
#[must_use]
pub fn restore_with_recovery(
    config: KernelConfig,
    persisted: PersistedState,
    watermark_load: WatermarkLoad,
    ctx: StepCtx,
) -> (State, Vec<Effect>) {
    let clock = match watermark_load {
        WatermarkLoad::Readable(watermark) if ctx.wall >= watermark => ClockState::Ready {
            watermark: ctx.wall,
        },
        WatermarkLoad::Readable(watermark) => ClockState::Uncertain {
            floor: Some(watermark),
        },
        WatermarkLoad::Unreadable => ClockState::Uncertain { floor: None },
    };

    let append_recovery = persisted
        .mine
        .iter()
        .filter(|(_, claim)| claim.status == MineStatus::Pending)
        .map(|((slot, generation), claim)| Effect::Schedule {
            token: WakeToken::AppendRetry {
                slot: slot.clone(),
                generation: *generation,
                intent: claim.intent.clone(),
            },
            at_mono: ctx.mono,
        })
        .collect::<Vec<_>>();

    let mut state = State {
        config,
        persisted,
        sync: BTreeMap::new(),
        clock,
    };
    let mut recovery = reconcile_service_winners(&mut state);
    recovery.extend(append_recovery);
    (state, recovery)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrincipalCtx {
    pub principal: Principal,
    pub authenticated_context: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerificationResult {
    Valid { signer: Principal },
    BadSignature,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VerificationBatch(pub Vec<VerificationResult>);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WakeToken {
    ClaimRenew {
        slot: Slot,
        generation: Generation,
    },
    AppendRetry {
        slot: Slot,
        generation: Generation,
        intent: IntentId,
    },
    GossipTick {
        peer: NodeId,
    },
    SyncTimeout {
        peer: NodeId,
        sync_id: SyncId,
        attempt: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeClass {
    OneShot,
    Periodic,
}

impl WakeToken {
    #[must_use]
    pub const fn class(&self) -> WakeClass {
        match self {
            Self::GossipTick { .. } => WakeClass::Periodic,
            Self::ClaimRenew { .. } | Self::AppendRetry { .. } | Self::SyncTimeout { .. } => {
                WakeClass::OneShot
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimMode {
    Initial,
    Renew,
    Takeover { authority_ref: GrantId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimCommand {
    pub intent: IntentId,
    pub slot: Slot,
    pub generation: Generation,
    pub mode: ClaimMode,
    pub draft: ClaimDraft,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Deliver {
        from: NodeId,
        msg: Box<WireMsg>,
        verification: VerificationBatch,
    },
    Route {
        ingress: IngressId,
        principal: PrincipalCtx,
        corr: Corr,
        query: RouteQuery,
    },
    Advertise {
        command: Box<ClaimCommand>,
    },
    OpAccepted {
        intent: IntentId,
        op: Box<SignedOp>,
        verification: VerificationResult,
    },
    Wakeup {
        token: WakeToken,
    },
    ClockReseed {
        watermark: WallMs,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteAns {
    Matched { node: NodeId },
    NoClaim,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Gossip {
        to: NodeId,
        msg: Box<WireMsg>,
    },
    Append {
        intent: IntentId,
        slot: Slot,
        generation: Generation,
        draft: Box<ClaimDraft>,
    },
    Reply {
        ingress: IngressId,
        corr: Corr,
        ans: RouteAns,
    },
    Schedule {
        token: WakeToken,
        at_mono: MonoInstant,
    },
    Teardown {
        slot: Slot,
        generation: Generation,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    pub persisted: PersistedState,
    pub effects: Vec<Effect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WallScheduleError {
    ClockUncertain,
}

/// Advances the pure clock and prepares a timer for durable effect delivery.
pub fn prepare_wall_schedule(
    state: State,
    ctx: StepCtx,
    target: WallMs,
    token: WakeToken,
) -> Result<(State, Vec<Effect>), WallScheduleError> {
    let mut state = observe_clock(state, ctx);
    let ClockState::Ready {
        watermark: effective_wall,
    } = state.clock
    else {
        return Err(WallScheduleError::ClockUncertain);
    };
    let crate::clock::ClockDecision::Ready(at_mono) =
        crate::clock::deadline_at(ctx, effective_wall, target)
    else {
        return Err(WallScheduleError::ClockUncertain);
    };
    let mut effects = reconcile_service_winners(&mut state);
    effects.push(Effect::Schedule { token, at_mono });
    Ok((state, effects))
}

#[must_use]
pub fn step(state: State, ctx: StepCtx, event: Event) -> (State, Vec<Effect>) {
    let mut state = observe_clock(state, ctx);
    let (mut state, mut effects) = match event {
        Event::Deliver {
            from,
            msg,
            verification,
        } => match *msg {
            WireMsg::DirOp { op } if matches!(verification.0.as_slice(), [_]) => {
                let verification = &verification.0[0];
                let outcome = crate::ingest::ingest(
                    &op,
                    verification,
                    &state.persisted,
                    &state.config,
                    state.clock,
                );
                state.persisted = outcome.persisted;
                (state, Vec::new())
            }
            WireMsg::DirOp { .. } => (state, Vec::new()),
            message @ (WireMsg::SyncStart { .. }
            | WireMsg::SyncOps { .. }
            | WireMsg::SyncEnd { .. }) => {
                let transition =
                    crate::sync::on_message(&state, ctx, &from, &message, &verification);
                state.persisted = transition.persisted;
                state.sync = transition.rounds;
                (state, transition.effects)
            }
        },
        Event::Route {
            ingress,
            corr,
            query,
            ..
        } => {
            let claims =
                crate::projection::live_claims(&state.persisted, &state.config, state.clock);
            (
                state,
                vec![crate::routing::reply(ingress, corr, &query, &claims)],
            )
        }
        Event::ClockReseed { watermark } => {
            let previous = state.clock;
            state.clock = crate::clock::reseed(state.clock, ctx, watermark);
            if matches!(previous, ClockState::Uncertain { .. }) {
                if let ClockState::Ready {
                    watermark: effective_wall,
                } = state.clock
                {
                    state.persisted = crate::ingest::revalidate_time_deferred(
                        &state.persisted,
                        &state.config,
                        effective_wall,
                    );
                }
            }
            (state, Vec::new())
        }
        Event::Advertise { command } => {
            let outcome = crate::claims::on_advertise(&state, ctx, &command);
            state.persisted = outcome.transition.persisted;
            (state, outcome.transition.effects)
        }
        Event::OpAccepted {
            intent,
            op,
            verification,
        } => {
            let transition = crate::append::on_accepted(&state, ctx, &intent, &op, &verification);
            state.persisted = transition.persisted;
            (state, transition.effects)
        }
        Event::Wakeup { token } => match &token {
            WakeToken::ClaimRenew { .. } => {
                let transition = crate::claims::on_wakeup(&state, ctx, &token);
                state.persisted = transition.persisted;
                (state, transition.effects)
            }
            WakeToken::AppendRetry {
                slot,
                generation,
                intent,
            } => {
                let transition = crate::append::on_retry(&state, slot, *generation, intent);
                state.persisted = transition.persisted;
                (state, transition.effects)
            }
            WakeToken::GossipTick { .. } | WakeToken::SyncTimeout { .. } => {
                let transition = crate::sync::on_wakeup(&state, ctx, &token);
                state.persisted = transition.persisted;
                state.sync = transition.rounds;
                (state, transition.effects)
            }
        },
    };
    effects.extend(reconcile_service_winners(&mut state));
    (state, effects)
}

fn observe_clock(mut state: State, ctx: StepCtx) -> State {
    let previous_clock = state.clock;
    let observed = crate::clock::observe(state.clock, ctx, &state.config);
    state.clock = match observed {
        crate::clock::EffectiveClock::Ready {
            state: clock,
            effective_wall,
        } => {
            if matches!(previous_clock, ClockState::Uncertain { .. }) {
                state.persisted = crate::ingest::revalidate_time_deferred(
                    &state.persisted,
                    &state.config,
                    effective_wall,
                );
            }
            clock
        }
        crate::clock::EffectiveClock::Uncertain { state } => state,
    };
    state
}

fn reconcile_service_winners(state: &mut State) -> Vec<Effect> {
    let projected = crate::projection::live_claims(&state.persisted, &state.config, state.clock);
    let mut slots = projected
        .iter()
        .filter_map(|claim| match &claim.slot {
            slot @ Slot::Binding { share, .. } if share == "svc" => Some(slot.clone()),
            Slot::Workspace { .. } | Slot::Binding { .. } => None,
        })
        .collect::<BTreeSet<_>>();
    slots.extend(
        state
            .persisted
            .mine
            .iter()
            .filter_map(|((slot, _), claim)| match slot {
                slot @ Slot::Binding { share, .. }
                    if share == "svc" && claim.status == MineStatus::Accepted =>
                {
                    Some(slot.clone())
                }
                Slot::Workspace { .. } | Slot::Binding { .. } => None,
            }),
    );
    let mut effects = Vec::new();
    for slot in slots {
        let winner = projected
            .iter()
            .filter(|claim| claim.slot == slot)
            .max_by(|left, right| (left.epoch, &left.claim_id).cmp(&(right.epoch, &right.claim_id)))
            .map(|claim| claim.claim_id.clone());
        let transition = crate::claims::on_slot_winner(state, &slot, winner.as_ref());
        state.persisted = transition.persisted;
        effects.extend(transition.effects);
    }
    effects
}
