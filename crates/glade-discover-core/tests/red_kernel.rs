use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::clock::EffectiveClock;
use glade_discover_core::projection::ProjectedClaim;
use glade_discover_core::{
    ClockState, Effect, Event, KernelConfig, MonoInstant, PersistedState, PrincipalCtx, RouteAns,
    StepCtx, VerificationBatch, WakeToken, WallMs, WatermarkLoad, claims, clock, restore_fresh,
    routing, step, sync,
};
use glade_discover_protocol::{
    ClaimId, Corr, Generation, IngressId, NodeId, Principal, RecordId, RouteQuery, Slot, StreamId,
    SyncId, WireMsg,
};

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("principal-a"),
        peers: BTreeSet::from([NodeId::from("node-b")]),
        workspace_owner_roots: BTreeMap::new(),
        node_principal_bindings: BTreeSet::new(),
        skew_margin_ms: 5_000,
        max_lease_ms: 3_600_000,
        clock_resync_ms: 30_000,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(10_000).expect("non-zero fixture"),
        gossip_fan: 8,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("non-zero fixture"),
    }
}

fn state() -> glade_discover_core::State {
    restore_fresh(
        config(),
        PersistedState::default(),
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(1_000),
        },
    )
}

fn query() -> RouteQuery {
    RouteQuery {
        slot: Slot::Workspace {
            share: "workspace-a".into(),
        },
    }
}

fn claim_id(key: u8, origin: &str, seq: u64) -> ClaimId {
    ClaimId::from(RecordId {
        stream: StreamId {
            share: "workspace-a".into(),
            glade_id: "directory".into(),
            key: vec![key],
        },
        origin: Principal::from(origin),
        seq,
    })
}

#[test]
fn effective_wall_never_moves_backward() {
    let observed = clock::observe(
        ClockState::Ready {
            watermark: WallMs(1_000),
        },
        StepCtx {
            mono: MonoInstant(5),
            wall: WallMs(900),
        },
        &config(),
    );

    assert_eq!(
        observed,
        EffectiveClock::Ready {
            state: ClockState::Ready {
                watermark: WallMs(1_000)
            },
            effective_wall: WallMs(1_000),
        }
    );
}

#[test]
fn epoch_then_stable_claim_id_selects_one_winner() {
    let claims = vec![
        ProjectedClaim {
            slot: query().slot,
            node: NodeId::from("node-a"),
            claim_id: claim_id(1, "origin-a", 1),
            epoch: 4,
        },
        ProjectedClaim {
            slot: query().slot,
            node: NodeId::from("node-b"),
            claim_id: claim_id(2, "origin-b", 1),
            epoch: 4,
        },
    ];

    assert_eq!(
        routing::resolve(&query(), &claims),
        RouteAns::Matched {
            node: NodeId::from("node-b")
        }
    );
}

#[test]
fn a_route_emits_exactly_one_direct_reply() {
    let ingress = IngressId::from("ingress-a");
    let corr = Corr::from("corr-a");
    let event = Event::Route {
        ingress: ingress.clone(),
        principal: PrincipalCtx {
            principal: Principal::from("reader-a"),
            authenticated_context: Vec::new(),
        },
        corr: corr.clone(),
        query: query(),
    };
    let (_, effects) = step(
        state(),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(1_001),
        },
        event,
    );

    assert_eq!(
        effects,
        vec![Effect::Reply {
            ingress,
            corr,
            ans: RouteAns::NoClaim,
        }]
    );
}

#[test]
fn step_is_directly_pure_for_identical_inputs() {
    let initial = state();
    let event = Event::ClockReseed {
        watermark: WallMs(1_000),
    };
    let ctx = StepCtx {
        mono: MonoInstant(1),
        wall: WallMs(1_000),
    };

    assert_eq!(
        step(initial.clone(), ctx, event.clone()),
        step(initial, ctx, event)
    );
}

#[test]
fn sync_end_is_terminal_only_for_the_correlated_round() {
    let transition = sync::on_message(
        &state(),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(1_000),
        },
        &NodeId::from("node-b"),
        &WireMsg::SyncEnd {
            sync_id: SyncId::from("round-1"),
        },
        &VerificationBatch::default(),
    );

    assert!(transition.effects.is_empty());
}

#[test]
fn stale_generation_wakeup_is_inert() {
    let transition = claims::on_wakeup(
        &state(),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(1_000),
        },
        &WakeToken::ClaimRenew {
            slot: query().slot,
            generation: Generation(99),
        },
    );

    assert!(transition.effects.is_empty());
}
