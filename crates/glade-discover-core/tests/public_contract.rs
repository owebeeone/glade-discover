use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    ClockState, Effect, Event, KernelConfig, MonoInstant, NodePrincipalBinding, PersistedState,
    PrincipalCtx, PrincipalPlane, RouteAns, StepCtx, VerificationResult, WakeClass, WakeToken,
    WallMs, WatermarkLoad, restore_fresh,
};
use glade_discover_protocol::{Corr, Generation, IngressId, NodeId, Principal, RouteQuery, Slot};

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("principal-a"),
        peers: BTreeSet::from([NodeId::from("node-b")]),
        workspace_owner_roots: BTreeMap::from([("workspace-a".into(), Principal::from("owner-a"))]),
        node_principal_bindings: BTreeSet::from([NodePrincipalBinding {
            node: NodeId::from("node-a"),
            principal: Principal::from("principal-a"),
            plane: PrincipalPlane::Workspace,
        }]),
        skew_margin_ms: 5_000,
        max_lease_ms: 3_600_000,
        clock_resync_ms: 30_000,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(10_000).expect("non-zero fixture"),
        gossip_fan: 8,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("non-zero fixture"),
    }
}

#[test]
fn restore_derives_ready_and_uncertain_clock_states() {
    let persisted = PersistedState::default();

    let ready = restore_fresh(
        config(),
        persisted.clone(),
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(10),
            wall: WallMs(1_001),
        },
    );
    let rollback = restore_fresh(
        config(),
        persisted.clone(),
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(10),
            wall: WallMs(999),
        },
    );
    let unreadable = restore_fresh(
        config(),
        persisted,
        WatermarkLoad::Unreadable,
        StepCtx {
            mono: MonoInstant(10),
            wall: WallMs(999),
        },
    );

    assert_eq!(
        ready.clock(),
        ClockState::Ready {
            watermark: WallMs(1_001)
        }
    );
    assert_eq!(
        rollback.clock(),
        ClockState::Uncertain {
            floor: Some(WallMs(1_000))
        }
    );
    assert_eq!(unreadable.clock(), ClockState::Uncertain { floor: None });
}

#[test]
fn wake_class_is_derived_from_the_typed_variant() {
    let periodic = WakeToken::GossipTick {
        peer: NodeId::from("node-b"),
    };
    let one_shot = WakeToken::ClaimRenew {
        slot: Slot::Workspace {
            share: "workspace-a".into(),
        },
        generation: Generation(2),
    };

    assert_eq!(periodic.class(), WakeClass::Periodic);
    assert_eq!(one_shot.class(), WakeClass::OneShot);
    assert!(one_shot < periodic);
}

#[test]
fn route_event_and_effect_are_direct_and_authz_blind() {
    let ingress = IngressId::from("ingress-a");
    let corr = Corr::from("corr-a");
    let query = RouteQuery {
        slot: Slot::Workspace {
            share: "workspace-a".into(),
        },
    };
    let event = Event::Route {
        ingress: ingress.clone(),
        principal: PrincipalCtx {
            principal: Principal::from("reader-a"),
            authenticated_context: vec![0x01],
        },
        corr: corr.clone(),
        query,
    };
    let effect = Effect::Reply {
        ingress,
        corr,
        ans: RouteAns::NoClaim,
    };

    assert!(matches!(event, Event::Route { .. }));
    assert!(matches!(
        effect,
        Effect::Reply {
            ans: RouteAns::NoClaim,
            ..
        }
    ));
}

#[test]
fn verifier_result_is_explicit_local_input() {
    let valid = VerificationResult::Valid {
        signer: Principal::from("principal-a"),
    };

    assert_ne!(valid, VerificationResult::BadSignature);
}
