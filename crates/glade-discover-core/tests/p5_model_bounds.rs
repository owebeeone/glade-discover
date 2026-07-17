use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    ClockState, Event, IngestDisposition, KernelConfig, MonoInstant, NodePrincipalBinding,
    PersistedState, PrincipalPlane, RoundProgress, StepCtx, VerificationResult, WakeToken, WallMs,
    WatermarkLoad, ingest, restore_fresh, step,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityRevocation, CapabilityVerb, DirectoryRecord, GrantId, NodeId,
    OpEnvelope, Principal, RecordId, Shape, StreamId, encode_directory_record, encode_signed_op,
    op_hash,
};

fn stream() -> StreamId {
    StreamId {
        share: "workspace-a".into(),
        glade_id: "directory".into(),
        key: Vec::new(),
    }
}

fn config(max_retained_bytes: u64, sync_retries: u8) -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("owner-a"),
        peers: BTreeSet::from([NodeId::from("node-b")]),
        workspace_owner_roots: BTreeMap::from([("workspace-a".into(), Principal::from("owner-a"))]),
        node_principal_bindings: BTreeSet::from([NodePrincipalBinding {
            node: NodeId::from("node-a"),
            principal: Principal::from("owner-a"),
            plane: PrincipalPlane::Workspace,
        }]),
        skew_margin_ms: 5,
        max_lease_ms: 1_000,
        clock_resync_ms: 30,
        sync_retries,
        sync_timeout_ms: NonZeroU64::new(10).expect("non-zero"),
        gossip_fan: 1,
        max_retained_bytes: NonZeroU64::new(max_retained_bytes).expect("non-zero"),
    }
}

fn signed_record(
    seq: u64,
    prev: Option<[u8; 32]>,
    record: DirectoryRecord,
) -> glade_discover_protocol::SignedOp {
    let payload = encode_directory_record(&record).expect("canonical record");
    encode_signed_op(
        &OpEnvelope {
            stream: stream(),
            origin: Principal::from("owner-a"),
            seq,
            prev,
            lamport: seq,
            refs: Vec::new(),
            shape: Shape::Log,
            payload,
        },
        &[0xAA],
    )
    .expect("canonical op")
}

fn grant() -> (GrantId, glade_discover_protocol::SignedOp) {
    let id = GrantId::from(RecordId {
        stream: stream(),
        origin: Principal::from("owner-a"),
        seq: 0,
    });
    let op = signed_record(
        0,
        None,
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: id.clone(),
            issuer: Principal::from("owner-a"),
            principal: Principal::from("node-a"),
            share: "workspace-a".into(),
            verbs: vec![CapabilityVerb::Serve],
            scope: None,
        }),
    );
    (id, op)
}

fn ingest_valid(
    op: &glade_discover_protocol::SignedOp,
    persisted: &PersistedState,
    limit: u64,
) -> glade_discover_core::ingest::IngestOutcome {
    ingest::ingest(
        op,
        &VerificationResult::Valid {
            signer: Principal::from("owner-a"),
        },
        persisted,
        &config(limit, 3),
        ClockState::Ready {
            watermark: WallMs(1_000),
        },
    )
}

#[test]
fn retained_byte_ceiling_accepts_exactly_at_limit_and_never_grows_past_it() {
    let (grant_id, first) = grant();
    let first_len = u64::try_from(first.canonical_bytes().len()).expect("small fixture");

    let below = ingest_valid(&first, &PersistedState::default(), first_len - 1);
    assert_eq!(below.disposition, IngestDisposition::StorageExhausted);
    assert_eq!(below.persisted, PersistedState::default());

    let exact = ingest_valid(&first, &PersistedState::default(), first_len);
    assert_eq!(exact.disposition, IngestDisposition::Folded);
    assert_eq!(exact.persisted.retained_bytes, first_len);

    let duplicate = ingest_valid(&first, &exact.persisted, first_len);
    assert_eq!(duplicate.disposition, IngestDisposition::Duplicate);
    assert_eq!(duplicate.persisted, exact.persisted);

    let second = signed_record(
        1,
        Some(op_hash(&first)),
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation { revokes: grant_id }),
    );
    let over = ingest_valid(&second, &exact.persisted, first_len);
    assert_eq!(over.disposition, IngestDisposition::StorageExhausted);
    assert_eq!(over.persisted, exact.persisted);
    assert!(over.persisted.retained_bytes <= first_len);
}

fn sync_state(sync_retries: u8) -> glade_discover_core::State {
    restore_fresh(
        config(1_000_000, sync_retries),
        PersistedState::default(),
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(1_000),
        },
    )
}

#[test]
fn sync_round_transition_model_holds_for_every_small_retry_budget() {
    let peer = NodeId::from("node-b");
    for retry_budget in 0..=5 {
        let (mut state, start_effects) = step(
            sync_state(retry_budget),
            StepCtx {
                mono: MonoInstant(0),
                wall: WallMs(1_000),
            },
            Event::Wakeup {
                token: WakeToken::GossipTick { peer: peer.clone() },
            },
        );
        assert_eq!(start_effects.len(), 2);
        let sync_id = state.sync().keys().next().expect("round").1.clone();

        for attempt in 0..=retry_budget {
            let (next, effects) = step(
                state,
                StepCtx {
                    mono: MonoInstant(10 * (u64::from(attempt) + 1)),
                    wall: WallMs(1_001 + i64::from(attempt)),
                },
                Event::Wakeup {
                    token: WakeToken::SyncTimeout {
                        peer: peer.clone(),
                        sync_id: sync_id.clone(),
                        attempt,
                    },
                },
            );
            let expected = if attempt < retry_budget {
                RoundProgress::Active {
                    attempt: attempt + 1,
                }
            } else {
                RoundProgress::Stale
            };
            assert_eq!(
                next.sync().get(&(peer.clone(), sync_id.clone())),
                Some(&expected)
            );
            assert_eq!(effects.len(), usize::from(attempt < retry_budget) * 2);
            state = next;
        }

        let stale_sync = state.sync().clone();
        let stale_persisted = state.persisted().clone();
        let (unchanged, effects) = step(
            state,
            StepCtx {
                mono: MonoInstant(100),
                wall: WallMs(1_100),
            },
            Event::Wakeup {
                token: WakeToken::SyncTimeout {
                    peer: peer.clone(),
                    sync_id,
                    attempt: 0,
                },
            },
        );
        assert_eq!(unchanged.sync(), &stale_sync);
        assert_eq!(unchanged.persisted(), &stale_persisted);
        assert!(effects.is_empty());
    }
}

#[test]
fn completed_round_is_terminal_and_timer_overflow_fails_stale() {
    let peer = NodeId::from("node-b");
    let (state, _) = step(
        sync_state(3),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(1_000),
        },
        Event::Wakeup {
            token: WakeToken::GossipTick { peer: peer.clone() },
        },
    );
    let sync_id = state.sync().keys().next().expect("round").1.clone();
    let (complete, _) = step(
        state,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(1_001),
        },
        Event::Deliver {
            from: peer.clone(),
            msg: Box::new(glade_discover_protocol::WireMsg::SyncEnd {
                sync_id: sync_id.clone(),
            }),
            verification: glade_discover_core::VerificationBatch::default(),
        },
    );
    let (still_complete, effects) = step(
        complete,
        StepCtx {
            mono: MonoInstant(10),
            wall: WallMs(1_010),
        },
        Event::Wakeup {
            token: WakeToken::SyncTimeout {
                peer: peer.clone(),
                sync_id: sync_id.clone(),
                attempt: 0,
            },
        },
    );
    assert_eq!(
        still_complete.sync().get(&(peer.clone(), sync_id)),
        Some(&RoundProgress::Complete)
    );
    assert!(effects.is_empty());

    let (overflow, effects) = step(
        sync_state(3),
        StepCtx {
            mono: MonoInstant(u64::MAX),
            wall: WallMs(1_000),
        },
        Event::Wakeup {
            token: WakeToken::GossipTick { peer: peer.clone() },
        },
    );
    assert!(effects.is_empty());
    let (_, progress) = overflow.sync().iter().next().expect("overflowed round");
    assert_eq!(progress, &RoundProgress::Stale);
}
