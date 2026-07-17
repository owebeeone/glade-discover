use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    ClaimCommand, ClaimMode, ClockState, Effect, Event, KernelConfig, MineStatus, MonoInstant,
    NodePrincipalBinding, OwnClaim, PersistedState, PrincipalPlane, WakeToken, WallMs,
    WatermarkLoad, step,
};
use glade_discover_node_adapter::clock::{self, ClockHost, ClockSample};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimDraft, ClaimIdentity, DirectoryRecord, Generation,
    GrantId, IntentId, NodeId, OpEnvelope, Principal, RecordId, Shape, Slot, StreamId,
    encode_directory_record, encode_signed_op,
};

struct FakeClock {
    load: WatermarkLoad,
    persisted: Vec<WallMs>,
}

impl ClockHost for FakeClock {
    type Error = &'static str;

    fn load_watermark(&mut self) -> Result<WatermarkLoad, Self::Error> {
        Ok(self.load)
    }

    fn persist_watermark(&mut self, watermark: WallMs) -> Result<(), Self::Error> {
        self.persisted.push(watermark);
        Ok(())
    }
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("node-principal"),
        peers: BTreeSet::new(),
        workspace_owner_roots: BTreeMap::from([("workspace".to_owned(), Principal::from("owner"))]),
        node_principal_bindings: BTreeSet::from([NodePrincipalBinding {
            node: NodeId::from("node-a"),
            principal: Principal::from("node-principal"),
            plane: PrincipalPlane::Workspace,
        }]),
        skew_margin_ms: 5,
        max_lease_ms: 10_000,
        clock_resync_ms: 30,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(1_000).expect("non-zero"),
        gossip_fan: 2,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("non-zero"),
    }
}

fn grant_stream() -> StreamId {
    StreamId {
        share: "workspace".to_owned(),
        glade_id: "directory".to_owned(),
        key: vec![0x10],
    }
}

fn grant_id() -> GrantId {
    GrantId::from(RecordId {
        stream: grant_stream(),
        origin: Principal::from("owner"),
        seq: 0,
    })
}

fn persisted_with_grant() -> PersistedState {
    let record = DirectoryRecord::CapabilityGrant(CapabilityGrant {
        grant_id: grant_id(),
        issuer: Principal::from("owner"),
        principal: Principal::from("node-principal"),
        share: "workspace".to_owned(),
        verbs: vec![CapabilityVerb::Serve],
        scope: None,
    });
    let op = encode_signed_op(
        &OpEnvelope {
            stream: grant_stream(),
            origin: Principal::from("owner"),
            seq: 0,
            prev: None,
            lamport: 1,
            refs: Vec::new(),
            shape: Shape::Log,
            payload: encode_directory_record(&record).expect("grant record"),
        },
        &[0xa5],
    )
    .expect("signed grant");
    let retained_bytes = u64::try_from(op.canonical_bytes().len()).expect("small fixture");
    let mut persisted = PersistedState {
        retained_bytes,
        ..PersistedState::default()
    };
    persisted
        .retained
        .entry(grant_stream())
        .or_default()
        .insert(grant_id().record().clone(), op);
    persisted
}

fn advertise() -> Event {
    Event::Advertise {
        command: Box::new(ClaimCommand {
            intent: IntentId::from("intent-a"),
            slot: Slot::Workspace {
                share: "workspace".to_owned(),
            },
            generation: Generation(1),
            mode: ClaimMode::Initial,
            draft: ClaimDraft::Workspace {
                node: NodeId::from("node-a"),
                share: "workspace".to_owned(),
                identity: ClaimIdentity::Mint,
                grant_ref: grant_id(),
                lease_expiry_ms: 5_000,
                epoch: 0,
            },
        }),
    }
}

#[test]
fn unreadable_or_backward_watermark_restores_uncertain_and_blocks_append() {
    let sample = ClockSample {
        wall: WallMs(1_000),
        mono: MonoInstant(50),
    };
    let cases = [
        (
            WatermarkLoad::Unreadable,
            ClockState::Uncertain { floor: None },
        ),
        (
            WatermarkLoad::Readable(WallMs(2_000)),
            ClockState::Uncertain {
                floor: Some(WallMs(2_000)),
            },
        ),
    ];

    for (load, expected) in cases {
        let mut host = FakeClock {
            load,
            persisted: Vec::new(),
        };
        let state = clock::restore(&mut host, config(), persisted_with_grant(), sample)
            .expect("clock restore");
        assert_eq!(state.state.clock(), expected);
        assert!(state.recovery.is_empty());

        let (_, effects) = step(state.state, sample.ctx(), advertise());
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Append { .. })),
            "clock-uncertain state must not publish"
        );
    }

    let mut ready_host = FakeClock {
        load: WatermarkLoad::Readable(WallMs(900)),
        persisted: Vec::new(),
    };
    let ready = clock::restore(&mut ready_host, config(), persisted_with_grant(), sample)
        .expect("ready restore");
    assert!(ready.recovery.is_empty());
    let (_, effects) = step(ready.state, sample.ctx(), advertise());
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Append { .. })),
        "control: the same authorized claim publishes when the clock is ready"
    );
}

#[test]
fn restore_durably_records_a_forward_wall_sample_before_returning_ready() {
    let mut host = FakeClock {
        load: WatermarkLoad::Readable(WallMs(100)),
        persisted: Vec::new(),
    };
    let restored = clock::restore(
        &mut host,
        config(),
        PersistedState::default(),
        ClockSample {
            wall: WallMs(120),
            mono: MonoInstant(10),
        },
    )
    .expect("restore");

    assert_eq!(
        restored.state.clock(),
        ClockState::Ready {
            watermark: WallMs(120)
        }
    );
    assert_eq!(host.persisted, [WallMs(120)]);
}

#[test]
fn adapter_restore_surfaces_durable_pending_append_recovery_effects() {
    let mut persisted = persisted_with_grant();
    let slot = Slot::Workspace {
        share: "workspace".to_owned(),
    };
    persisted.mine.insert(
        (slot.clone(), Generation(4)),
        OwnClaim {
            intent: IntentId::from("recover-me"),
            draft: match advertise() {
                Event::Advertise { command } => command.draft,
                _ => unreachable!(),
            },
            status: MineStatus::Pending,
        },
    );
    let mut host = FakeClock {
        load: WatermarkLoad::Readable(WallMs(1_000)),
        persisted: Vec::new(),
    };
    let restored = clock::restore(
        &mut host,
        config(),
        persisted,
        ClockSample {
            wall: WallMs(1_000),
            mono: MonoInstant(77),
        },
    )
    .expect("restore pending state");

    assert_eq!(
        restored.recovery,
        vec![Effect::Schedule {
            token: WakeToken::AppendRetry {
                slot,
                generation: Generation(4),
                intent: IntentId::from("recover-me"),
            },
            at_mono: MonoInstant(77),
        }]
    );
}
