use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::clock::{ClockDecision, EffectiveClock};
use glade_discover_core::{ClockState, KernelConfig, MonoInstant, StepCtx, WallMs, clock};
use glade_discover_protocol::{NodeId, Principal};

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("principal-a"),
        peers: BTreeSet::new(),
        workspace_owner_roots: BTreeMap::new(),
        node_principal_bindings: BTreeSet::new(),
        skew_margin_ms: 5_000,
        max_lease_ms: 3_600_000,
        clock_resync_ms: 30_000,
        sync_timeout_ms: NonZeroU64::new(10_000).expect("nonzero"),
        sync_retries: 3,
        gossip_fan: 8,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("nonzero"),
    }
}

fn ctx(mono: u64, wall: i64) -> StepCtx {
    StepCtx {
        mono: MonoInstant(mono),
        wall: WallMs(wall),
    }
}

#[test]
fn ready_watermark_advances_forward_and_ignores_runtime_rollback() {
    assert_eq!(
        clock::observe(
            ClockState::Ready {
                watermark: WallMs(1_000),
            },
            ctx(5, 1_500),
            &config(),
        ),
        EffectiveClock::Ready {
            state: ClockState::Ready {
                watermark: WallMs(1_500),
            },
            effective_wall: WallMs(1_500),
        }
    );
    assert_eq!(
        clock::observe(
            ClockState::Ready {
                watermark: WallMs(1_500),
            },
            ctx(6, 900),
            &config(),
        ),
        EffectiveClock::Ready {
            state: ClockState::Ready {
                watermark: WallMs(1_500),
            },
            effective_wall: WallMs(1_500),
        }
    );
}

#[test]
fn readable_uncertainty_recovers_only_at_the_checked_threshold() {
    let uncertain = ClockState::Uncertain {
        floor: Some(WallMs(1_000)),
    };
    assert_eq!(
        clock::observe(uncertain, ctx(1, 30_999), &config()),
        EffectiveClock::Uncertain { state: uncertain }
    );
    assert_eq!(
        clock::observe(uncertain, ctx(2, 31_000), &config()),
        EffectiveClock::Ready {
            state: ClockState::Ready {
                watermark: WallMs(31_000),
            },
            effective_wall: WallMs(31_000),
        }
    );
}

#[test]
fn recovery_overflow_and_unknown_floor_remain_uncertain() {
    let overflow = ClockState::Uncertain {
        floor: Some(WallMs(i64::MAX - 10)),
    };
    assert_eq!(
        clock::observe(overflow, ctx(1, i64::MAX), &config()),
        EffectiveClock::Uncertain { state: overflow }
    );
    assert_eq!(
        clock::observe(
            ClockState::Uncertain { floor: None },
            ctx(1, i64::MAX),
            &config(),
        ),
        EffectiveClock::Uncertain {
            state: ClockState::Uncertain { floor: None },
        }
    );
    let mut oversized_resync = config();
    oversized_resync.clock_resync_ms = u64::MAX;
    let negative_floor = ClockState::Uncertain {
        floor: Some(WallMs(-100)),
    };
    assert_eq!(
        clock::observe(negative_floor, ctx(1, i64::MAX), &oversized_resync),
        EffectiveClock::Uncertain {
            state: negative_floor,
        }
    );
}

#[test]
fn trusted_reseed_only_recovers_an_unknown_floor_and_never_moves_below_wall() {
    assert_eq!(
        clock::reseed(
            ClockState::Uncertain { floor: None },
            ctx(3, 5_000),
            WallMs(4_000),
        ),
        ClockState::Ready {
            watermark: WallMs(5_000),
        }
    );
    let known = ClockState::Uncertain {
        floor: Some(WallMs(9_000)),
    };
    assert_eq!(clock::reseed(known, ctx(3, 5_000), WallMs(6_000)), known);
    let ready = ClockState::Ready {
        watermark: WallMs(9_000),
    };
    assert_eq!(clock::reseed(ready, ctx(3, 5_000), WallMs(6_000)), ready);
}

#[test]
fn wall_deadline_projection_is_checked_and_never_schedules_in_the_past() {
    assert_eq!(
        clock::deadline_at(ctx(100, 0), WallMs(1_000), WallMs(900)),
        ClockDecision::Ready(MonoInstant(100))
    );
    assert_eq!(
        clock::deadline_at(ctx(100, 0), WallMs(1_000), WallMs(1_025)),
        ClockDecision::Ready(MonoInstant(125))
    );
    assert_eq!(
        clock::deadline_at(ctx(u64::MAX - 2, 0), WallMs(0), WallMs(i64::MAX),),
        ClockDecision::Uncertain
    );
    assert_eq!(
        clock::deadline_at(ctx(0, 0), WallMs(i64::MIN), WallMs(i64::MAX)),
        ClockDecision::Uncertain
    );
}

#[test]
fn skew_expiry_and_maximum_lease_boundaries_are_inclusive_and_checked() {
    assert_eq!(
        clock::is_expired(WallMs(6_000), WallMs(1_000), &config()),
        ClockDecision::Ready(true)
    );
    assert_eq!(
        clock::is_expired(WallMs(6_001), WallMs(1_000), &config()),
        ClockDecision::Ready(false)
    );
    assert_eq!(
        clock::lease_within_limit(WallMs(3_606_000), WallMs(1_000), &config()),
        ClockDecision::Ready(true)
    );
    assert_eq!(
        clock::lease_within_limit(WallMs(3_606_001), WallMs(1_000), &config()),
        ClockDecision::Ready(false)
    );
    assert_eq!(
        clock::is_expired(WallMs(i64::MAX), WallMs(i64::MAX), &config()),
        ClockDecision::Uncertain
    );
    assert_eq!(
        clock::lease_within_limit(WallMs(i64::MAX), WallMs(i64::MAX), &config()),
        ClockDecision::Uncertain
    );
}
