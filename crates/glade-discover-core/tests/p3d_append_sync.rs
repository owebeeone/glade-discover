use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    ClockState, Effect, Event, KernelConfig, MineStatus, MonoInstant, NodePrincipalBinding,
    OwnClaim, PersistedState, PrincipalPlane, RoundProgress, StepCtx, VerificationBatch,
    VerificationResult, WakeToken, WallMs, append, restore_with_recovery, step, sync,
};
use glade_discover_protocol::{
    ClaimDraft, ClaimId, ClaimIdentity, DecodeError, DirectoryRecord, Generation, GrantId,
    IntentId, MAX_MESSAGE_BYTES, MAX_SYNC_ID_BYTES, NodeId, OpEnvelope, Principal, RecordId,
    ServeClaim, Shape, Slot, StreamHead, StreamId, SyncId, WireMsg, decode_wire_msg,
    encode_directory_record, encode_signed_op, encode_wire_msg, op_hash, record_id,
};

fn stream() -> StreamId {
    StreamId {
        share: "workspace-a".into(),
        glade_id: "directory".into(),
        key: vec![1],
    }
}

fn claim_record_id() -> RecordId {
    RecordId {
        stream: stream(),
        origin: Principal::from("principal-a"),
        seq: 0,
    }
}

fn grant_id() -> GrantId {
    GrantId::from(RecordId {
        stream: stream(),
        origin: Principal::from("owner-a"),
        seq: 0,
    })
}

fn draft() -> ClaimDraft {
    ClaimDraft::Workspace {
        node: NodeId::from("node-a"),
        share: "workspace-a".into(),
        identity: ClaimIdentity::Mint,
        grant_ref: grant_id(),
        lease_expiry_ms: 200_000,
        epoch: 0,
    }
}

fn signed_claim() -> glade_discover_protocol::SignedOp {
    let payload = encode_directory_record(&DirectoryRecord::ServeClaim(ServeClaim {
        node: NodeId::from("node-a"),
        share: "workspace-a".into(),
        claim_id: ClaimId::from(claim_record_id()),
        grant_ref: grant_id(),
        lease_expiry_ms: 200_000,
        epoch: 0,
    }))
    .expect("canonical claim");
    encode_signed_op(
        &OpEnvelope {
            stream: stream(),
            origin: Principal::from("principal-a"),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: Vec::new(),
            shape: Shape::Value,
            payload,
        },
        &[0xAA],
    )
    .expect("canonical signed claim")
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("principal-a"),
        peers: BTreeSet::from([NodeId::from("node-b"), NodeId::from("node-c")]),
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
        sync_timeout_ms: NonZeroU64::new(10_000).expect("non-zero"),
        gossip_fan: 8,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("non-zero"),
    }
}

fn state_with_pending_append() -> glade_discover_core::State {
    state_with_pending_append_and_watermark(glade_discover_core::WatermarkLoad::Readable(WallMs(
        100_000,
    )))
}

fn state_with_pending_append_and_watermark(
    watermark: glade_discover_core::WatermarkLoad,
) -> glade_discover_core::State {
    let slot = Slot::Workspace {
        share: "workspace-a".into(),
    };
    let mut persisted = PersistedState::default();
    persisted.mine.insert(
        (slot, Generation(1)),
        OwnClaim {
            intent: IntentId::from("intent-a"),
            draft: draft(),
            status: MineStatus::Pending,
        },
    );
    restore_state_for_unit_test(
        config(),
        persisted,
        watermark,
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    )
}

fn restore_state_for_unit_test(
    config: KernelConfig,
    persisted: PersistedState,
    watermark: glade_discover_core::WatermarkLoad,
    ctx: StepCtx,
) -> glade_discover_core::State {
    restore_with_recovery(config, persisted, watermark, ctx).0
}

#[test]
fn accepted_append_is_indexed_before_exact_byte_gossip() {
    let op = signed_claim();
    let transition = append::on_accepted(
        &state_with_pending_append(),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &IntentId::from("intent-a"),
        &op,
        &VerificationResult::Valid {
            signer: Principal::from("principal-a"),
        },
    );

    assert_eq!(
        transition.persisted.retained[&stream()][&claim_record_id()],
        op
    );
    let accepted = transition
        .persisted
        .mine
        .values()
        .next()
        .expect("own claim");
    assert_eq!(accepted.status, MineStatus::Accepted);
    assert!(matches!(
        &accepted.draft,
        ClaimDraft::Workspace {
            identity: ClaimIdentity::Existing(id),
            ..
        } if id.record() == &claim_record_id()
    ));
    assert_eq!(
        transition.effects,
        ["node-b", "node-c"]
            .into_iter()
            .map(|peer| Effect::Gossip {
                to: NodeId::from(peer),
                msg: Box::new(WireMsg::DirOp {
                    op: Box::new(op.clone()),
                }),
            })
            .collect::<Vec<_>>()
    );
    for effect in &transition.effects {
        let Effect::Gossip { msg, .. } = effect else {
            panic!("only gossip is expected");
        };
        let WireMsg::DirOp { op: forwarded } = msg.as_ref() else {
            panic!("exact directory op is expected");
        };
        assert_eq!(forwarded.canonical_bytes(), op.canonical_bytes());
    }
}

#[test]
fn unknown_append_intent_is_never_indexed_or_gossiped() {
    let transition = append::on_accepted(
        &state_with_pending_append(),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &IntentId::from("wrong-intent"),
        &signed_claim(),
        &VerificationResult::Valid {
            signer: Principal::from("principal-a"),
        },
    );

    assert!(transition.persisted.retained.is_empty());
    assert!(transition.effects.is_empty());
}

#[test]
fn superseded_generation_acceptance_is_dropped() {
    let state = state_with_pending_append();
    let mut persisted = state.persisted().clone();
    persisted.mine.insert(
        (
            Slot::Workspace {
                share: "workspace-a".into(),
            },
            Generation(2),
        ),
        OwnClaim {
            intent: IntentId::from("intent-new"),
            draft: draft(),
            status: MineStatus::Pending,
        },
    );
    let state = restore_state_for_unit_test(
        config(),
        persisted,
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    );
    let transition = append::on_accepted(
        &state,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &IntentId::from("intent-a"),
        &signed_claim(),
        &VerificationResult::Valid {
            signer: Principal::from("principal-a"),
        },
    );

    assert!(transition.persisted.retained.is_empty());
    assert!(transition.effects.is_empty());
}

#[test]
fn pending_append_retry_replays_the_same_durable_intent() {
    let transition = append::on_retry(
        &state_with_pending_append(),
        &Slot::Workspace {
            share: "workspace-a".into(),
        },
        Generation(1),
        &IntentId::from("intent-a"),
    );

    assert_eq!(
        transition.effects,
        vec![Effect::Append {
            intent: IntentId::from("intent-a"),
            slot: Slot::Workspace {
                share: "workspace-a".into(),
            },
            generation: Generation(1),
            draft: Box::new(draft()),
        }]
    );
}

#[test]
fn restore_reconstructs_pending_append_retries_in_deterministic_key_order() {
    let state = state_with_pending_append();
    let mut persisted = state.persisted().clone();
    persisted.mine.insert(
        (
            Slot::Workspace {
                share: "workspace-b".into(),
            },
            Generation(2),
        ),
        OwnClaim {
            intent: IntentId::from("intent-b"),
            draft: workspace_draft("workspace-b"),
            status: MineStatus::Pending,
        },
    );
    persisted.mine.insert(
        (
            Slot::Workspace {
                share: "workspace-c".into(),
            },
            Generation(3),
        ),
        OwnClaim {
            intent: IntentId::from("accepted-is-inert"),
            draft: workspace_draft("workspace-c"),
            status: MineStatus::Accepted,
        },
    );
    let restore_ctx = StepCtx {
        mono: MonoInstant(77),
        wall: WallMs(100_000),
    };

    let (_, effects) = restore_with_recovery(
        config(),
        persisted,
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        restore_ctx,
    );

    assert_eq!(
        effects,
        vec![
            Effect::Schedule {
                token: WakeToken::AppendRetry {
                    slot: Slot::Workspace {
                        share: "workspace-a".into(),
                    },
                    generation: Generation(1),
                    intent: IntentId::from("intent-a"),
                },
                at_mono: restore_ctx.mono,
            },
            Effect::Schedule {
                token: WakeToken::AppendRetry {
                    slot: Slot::Workspace {
                        share: "workspace-b".into(),
                    },
                    generation: Generation(2),
                    intent: IntentId::from("intent-b"),
                },
                at_mono: restore_ctx.mono,
            },
        ]
    );
}

#[test]
fn same_intent_on_two_slots_accepts_reverse_order_callbacks_by_finalized_payload() {
    let intent = IntentId::from("shared-intent");
    let draft_a = workspace_draft("workspace-a");
    let draft_b = workspace_draft("workspace-b");
    let slot_a = Slot::Workspace {
        share: "workspace-a".into(),
    };
    let slot_b = Slot::Workspace {
        share: "workspace-b".into(),
    };
    let mut persisted = PersistedState::default();
    for (slot, draft) in [
        (slot_a.clone(), draft_a.clone()),
        (slot_b.clone(), draft_b.clone()),
    ] {
        persisted.mine.insert(
            (slot, Generation(1)),
            OwnClaim {
                intent: intent.clone(),
                draft,
                status: MineStatus::Pending,
            },
        );
    }
    let mut two_slot_config = config();
    two_slot_config
        .workspace_owner_roots
        .insert("workspace-b".into(), Principal::from("owner-a"));
    let state = restore_state_for_unit_test(
        two_slot_config,
        persisted,
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    );
    let op_a = signed_workspace_claim("workspace-a", 0x0a);
    let op_b = signed_workspace_claim("workspace-b", 0x0b);
    let verification = VerificationResult::Valid {
        signer: Principal::from("principal-a"),
    };

    let reverse = append::on_accepted(
        &state,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &intent,
        &op_b,
        &verification,
    );
    assert_eq!(
        reverse
            .persisted
            .mine
            .get(&(slot_b.clone(), Generation(1)))
            .expect("workspace-b pending tuple")
            .status,
        MineStatus::Accepted
    );
    assert_eq!(
        reverse.persisted.retained[&op_b.envelope().stream]
            [&glade_discover_protocol::record_id(&op_b)],
        op_b
    );

    let state = restore_state_for_unit_test(
        config(),
        reverse.persisted,
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_001)),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
    );
    let forward = append::on_accepted(
        &state,
        StepCtx {
            mono: MonoInstant(2),
            wall: WallMs(100_002),
        },
        &intent,
        &op_a,
        &verification,
    );
    assert_eq!(
        forward
            .persisted
            .mine
            .get(&(slot_a, Generation(1)))
            .expect("workspace-a pending tuple")
            .status,
        MineStatus::Accepted
    );
}

#[test]
fn uncertain_acceptance_defers_without_retaining_then_exact_ready_duplicate_completes_once() {
    let op = signed_claim();
    let intent = IntentId::from("intent-a");
    let retry = WakeToken::AppendRetry {
        slot: Slot::Workspace {
            share: "workspace-a".into(),
        },
        generation: Generation(1),
        intent: intent.clone(),
    };
    let verification = VerificationResult::Valid {
        signer: Principal::from("principal-a"),
    };
    let (uncertain, effects) = step(
        state_with_pending_append_and_watermark(glade_discover_core::WatermarkLoad::Unreadable),
        StepCtx {
            mono: MonoInstant(10),
            wall: WallMs(100_000),
        },
        Event::OpAccepted {
            intent: intent.clone(),
            op: Box::new(op.clone()),
            verification: verification.clone(),
        },
    );
    assert!(uncertain.persisted().retained.is_empty());
    assert!(uncertain.persisted().time_deferred.is_empty());
    assert_eq!(
        uncertain
            .persisted()
            .mine
            .values()
            .next()
            .expect("pending mine")
            .status,
        MineStatus::Pending
    );
    assert_eq!(
        effects,
        vec![Effect::Schedule {
            token: retry.clone(),
            at_mono: MonoInstant(30_010),
        }]
    );

    let (ready, effects) = step(
        uncertain,
        StepCtx {
            mono: MonoInstant(11),
            wall: WallMs(100_000),
        },
        Event::ClockReseed {
            watermark: WallMs(100_000),
        },
    );
    assert!(effects.is_empty());
    assert!(matches!(ready.clock(), ClockState::Ready { .. }));

    let (retained_elsewhere, effects) = step(
        ready,
        StepCtx {
            mono: MonoInstant(12),
            wall: WallMs(100_000),
        },
        Event::Deliver {
            from: NodeId::from("node-b"),
            msg: Box::new(WireMsg::DirOp {
                op: Box::new(op.clone()),
            }),
            verification: VerificationBatch(vec![verification.clone()]),
        },
    );
    assert!(effects.is_empty());
    assert_eq!(
        retained_elsewhere
            .persisted()
            .mine
            .values()
            .next()
            .expect("still pending")
            .status,
        MineStatus::Pending
    );

    let (retrying, effects) = step(
        retained_elsewhere,
        StepCtx {
            mono: MonoInstant(30_010),
            wall: WallMs(100_001),
        },
        Event::Wakeup {
            token: retry.clone(),
        },
    );
    assert_eq!(
        effects,
        vec![Effect::Append {
            intent: intent.clone(),
            slot: Slot::Workspace {
                share: "workspace-a".into(),
            },
            generation: Generation(1),
            draft: Box::new(draft()),
        }]
    );

    let (accepted, effects) = step(
        retrying,
        StepCtx {
            mono: MonoInstant(30_011),
            wall: WallMs(100_001),
        },
        Event::OpAccepted {
            intent: intent.clone(),
            op: Box::new(op.clone()),
            verification: verification.clone(),
        },
    );
    assert_eq!(
        accepted
            .persisted()
            .mine
            .values()
            .next()
            .expect("accepted mine")
            .status,
        MineStatus::Accepted
    );
    assert_eq!(effects.len(), 2, "exact duplicate gossips once per peer");

    let (_, repeated_effects) = step(
        accepted,
        StepCtx {
            mono: MonoInstant(30_012),
            wall: WallMs(100_002),
        },
        Event::OpAccepted {
            intent,
            op: Box::new(op),
            verification,
        },
    );
    assert!(repeated_effects.is_empty(), "Accepted mine never regossips");
}

#[test]
fn uncertain_acceptance_retry_deadline_overflow_fails_closed() {
    let state =
        state_with_pending_append_and_watermark(glade_discover_core::WatermarkLoad::Unreadable);
    let (state, effects) = step(
        state,
        StepCtx {
            mono: MonoInstant(u64::MAX),
            wall: WallMs(100_000),
        },
        Event::OpAccepted {
            intent: IntentId::from("intent-a"),
            op: Box::new(signed_claim()),
            verification: VerificationResult::Valid {
                signer: Principal::from("principal-a"),
            },
        },
    );

    assert!(effects.is_empty());
    assert!(state.persisted().retained.is_empty());
    assert!(state.persisted().time_deferred.is_empty());
    assert_eq!(
        state
            .persisted()
            .mine
            .values()
            .next()
            .expect("pending mine")
            .status,
        MineStatus::Pending
    );
}

#[test]
fn ready_duplicate_requires_exact_canonical_accepted_bytes() {
    let retained = signed_claim();
    let substituted_signature =
        encode_signed_op(retained.envelope(), &[0xbb]).expect("canonical alternate signature");
    let pending = state_with_pending_append();
    let mut persisted = pending.persisted().clone();
    persisted
        .retained
        .entry(retained.envelope().stream.clone())
        .or_default()
        .insert(
            glade_discover_protocol::record_id(&retained),
            retained.clone(),
        );
    persisted.retained_bytes =
        u64::try_from(retained.canonical_bytes().len()).expect("small fixture");
    let ready = restore_state_for_unit_test(
        config(),
        persisted,
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    );

    let (ready, effects) = step(
        ready,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        Event::OpAccepted {
            intent: IntentId::from("intent-a"),
            op: Box::new(substituted_signature),
            verification: VerificationResult::Valid {
                signer: Principal::from("principal-a"),
            },
        },
    );

    assert!(effects.is_empty());
    assert_eq!(
        ready
            .persisted()
            .mine
            .values()
            .next()
            .expect("pending exact replay")
            .status,
        MineStatus::Pending
    );
}

fn workspace_draft(share: &str) -> ClaimDraft {
    ClaimDraft::Workspace {
        node: NodeId::from("node-a"),
        share: share.into(),
        identity: ClaimIdentity::Mint,
        grant_ref: grant_id(),
        lease_expiry_ms: 200_000,
        epoch: 0,
    }
}

fn signed_workspace_claim(share: &str, key: u8) -> glade_discover_protocol::SignedOp {
    let stream = StreamId {
        share: share.into(),
        glade_id: "directory".into(),
        key: vec![key],
    };
    let id = RecordId {
        stream: stream.clone(),
        origin: Principal::from("principal-a"),
        seq: 0,
    };
    let payload = encode_directory_record(&DirectoryRecord::ServeClaim(ServeClaim {
        node: NodeId::from("node-a"),
        share: share.into(),
        claim_id: ClaimId::from(id),
        grant_ref: grant_id(),
        lease_expiry_ms: 200_000,
        epoch: 0,
    }))
    .expect("canonical claim");
    encode_signed_op(
        &OpEnvelope {
            stream,
            origin: Principal::from("principal-a"),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: Vec::new(),
            shape: Shape::Value,
            payload,
        },
        &[0xaa],
    )
    .expect("canonical op")
}

fn state_with_retained_op(op: &glade_discover_protocol::SignedOp) -> glade_discover_core::State {
    let mut persisted = PersistedState::default();
    persisted
        .retained
        .entry(stream())
        .or_default()
        .insert(claim_record_id(), op.clone());
    persisted.retained_bytes = u64::try_from(op.canonical_bytes().len()).expect("small fixture");
    restore_state_for_unit_test(
        config(),
        persisted,
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    )
}

fn opaque_signed_op(
    stream: StreamId,
    seq: u64,
    payload_len: usize,
) -> glade_discover_protocol::SignedOp {
    encode_signed_op(
        &OpEnvelope {
            stream,
            origin: Principal::from("principal-a"),
            seq,
            prev: None,
            lamport: seq,
            refs: Vec::new(),
            shape: Shape::Value,
            payload: vec![0x5a; payload_len],
        },
        &[0xaa],
    )
    .expect("bounded canonical op")
}

fn state_with_retained_ops(
    ops: impl IntoIterator<Item = glade_discover_protocol::SignedOp>,
) -> glade_discover_core::State {
    let mut persisted = PersistedState::default();
    for op in ops {
        persisted.retained_bytes = persisted
            .retained_bytes
            .checked_add(u64::try_from(op.canonical_bytes().len()).expect("bounded op"))
            .expect("bounded fixture");
        persisted
            .retained
            .entry(op.envelope().stream.clone())
            .or_default()
            .insert(record_id(&op), op);
    }
    let mut large_config = config();
    large_config.max_retained_bytes =
        NonZeroU64::new(persisted.retained_bytes.saturating_add(1)).expect("non-zero bound");
    restore_state_for_unit_test(
        large_config,
        persisted,
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    )
}

#[test]
fn sync_start_streams_the_gap_then_the_correlated_terminal() {
    let op = signed_claim();
    let transition = sync::on_message(
        &state_with_retained_op(&op),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &NodeId::from("node-b"),
        &WireMsg::SyncStart {
            sync_id: SyncId::from("round-a"),
            heads: Vec::new(),
        },
        &VerificationBatch::default(),
    );

    assert_eq!(
        transition.effects,
        vec![
            Effect::Gossip {
                to: NodeId::from("node-b"),
                msg: Box::new(WireMsg::SyncOps {
                    sync_id: SyncId::from("round-a"),
                    ops: vec![op],
                }),
            },
            Effect::Gossip {
                to: NodeId::from("node-b"),
                msg: Box::new(WireMsg::SyncEnd {
                    sync_id: SyncId::from("round-a"),
                }),
            },
        ]
    );
}

#[test]
fn sync_start_from_an_unconfigured_peer_is_inert() {
    let transition = sync::on_message(
        &state_with_retained_op(&signed_claim()),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &NodeId::from("unknown-node"),
        &WireMsg::SyncStart {
            sync_id: SyncId::from("unknown-round"),
            heads: Vec::new(),
        },
        &VerificationBatch::default(),
    );

    assert!(transition.effects.is_empty());
}

#[test]
fn every_sync_ops_chunk_fits_the_wire_limit_and_receiver_decodes() {
    let ops = (0..100)
        .map(|seq| opaque_signed_op(stream(), seq, 15_000))
        .collect::<Vec<_>>();
    let transition = sync::on_message(
        &state_with_retained_ops(ops.clone()),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &NodeId::from("node-b"),
        &WireMsg::SyncStart {
            sync_id: SyncId::from("large-round"),
            heads: Vec::new(),
        },
        &VerificationBatch::default(),
    );

    let mut emitted = 0;
    let mut chunks = 0;
    for effect in &transition.effects {
        let Effect::Gossip { msg, .. } = effect else {
            panic!("sync response emits only gossip");
        };
        let (tag, body) = encode_wire_msg(msg);
        assert!(body.len() <= MAX_MESSAGE_BYTES, "{} byte body", body.len());
        assert_eq!(decode_wire_msg(tag, &body), Ok(msg.as_ref().clone()));
        if let WireMsg::SyncOps { ops, .. } = msg.as_ref() {
            assert!(ops.len() <= 256);
            emitted += ops.len();
            chunks += 1;
        }
    }
    assert_eq!(emitted, ops.len());
    assert!(chunks >= 2, "large legal gap must be byte-chunked");
    assert!(matches!(
        transition.effects.last(),
        Some(Effect::Gossip { msg, .. }) if matches!(msg.as_ref(), WireMsg::SyncEnd { .. })
    ));
}

#[test]
fn a_complete_near_ceiling_sync_response_fits_the_default_outbox_budget() {
    const DEFAULT_OUTBOX_BYTES: usize = 32 * 1024 * 1024;
    let ops = (0..1_050)
        .map(|seq| opaque_signed_op(stream(), seq, 15_000))
        .collect::<Vec<_>>();
    let transition = sync::on_message(
        &state_with_retained_ops(ops.clone()),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &NodeId::from("node-b"),
        &WireMsg::SyncStart {
            sync_id: SyncId::from("aggregate-round"),
            heads: Vec::new(),
        },
        &VerificationBatch::default(),
    );

    let aggregate = transition
        .effects
        .iter()
        .map(|effect| {
            let Effect::Gossip { to, msg } = effect else {
                panic!("sync response emits only gossip");
            };
            9 + to.as_str().len() + encode_wire_msg(msg).1.len()
        })
        .sum::<usize>();
    let emitted = transition
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Gossip { msg, .. } => match msg.as_ref() {
                WireMsg::SyncOps { ops, .. } => Some(ops.len()),
                WireMsg::SyncStart { .. } | WireMsg::SyncEnd { .. } | WireMsg::DirOp { .. } => None,
            },
            Effect::Append { .. }
            | Effect::Reply { .. }
            | Effect::Schedule { .. }
            | Effect::Teardown { .. } => None,
        })
        .sum::<usize>();

    assert!(aggregate <= DEFAULT_OUTBOX_BYTES, "{aggregate} bytes");
    assert_eq!(emitted, ops.len());
    assert!(matches!(
        transition.effects.last(),
        Some(Effect::Gossip { msg, .. }) if matches!(msg.as_ref(), WireMsg::SyncEnd { .. })
    ));
}

#[test]
fn an_overlong_direct_sync_id_is_inert_at_the_pure_core_boundary() {
    let sync_id = SyncId::from("x".repeat(MAX_SYNC_ID_BYTES + 1));
    let request = WireMsg::SyncStart {
        sync_id,
        heads: Vec::new(),
    };
    let (tag, body) = encode_wire_msg(&request);
    assert!(body.len() < MAX_MESSAGE_BYTES);
    assert_eq!(
        decode_wire_msg(tag, &body),
        Err(DecodeError::SyncIdTooLarge)
    );

    let transition = sync::on_message(
        &state_with_retained_op(&signed_claim()),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &NodeId::from("node-b"),
        &request,
        &VerificationBatch::default(),
    );

    assert!(
        transition.effects.is_empty(),
        "the direct pure boundary must mirror fail-closed wire validation"
    );
}

#[test]
fn an_unrepresentable_local_sync_start_marks_the_round_stale_without_a_timer() {
    let mut oversized = config();
    oversized.local_node = NodeId::from("n".repeat(MAX_MESSAGE_BYTES));
    let state = restore_state_for_unit_test(
        oversized,
        PersistedState::default(),
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    );
    let peer = NodeId::from("node-b");
    let transition = sync::on_wakeup(
        &state,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &WakeToken::GossipTick { peer: peer.clone() },
    );

    assert!(transition.effects.is_empty());
    assert_eq!(transition.persisted.next_sync, 1);
    assert_eq!(
        transition.rounds.get(&(
            peer,
            SyncId::from(format!("{}:0", state.config().local_node))
        )),
        Some(&RoundProgress::Stale)
    );
}

#[test]
fn sync_start_truncates_heads_to_a_receiver_decodable_conservative_summary() {
    let ops = (0_u16..300)
        .map(|ordinal| {
            let mut key = vec![0x44; 4_096];
            key[..2].copy_from_slice(&ordinal.to_be_bytes());
            opaque_signed_op(
                StreamId {
                    share: "workspace-a".into(),
                    glade_id: "directory".into(),
                    key,
                },
                0,
                1,
            )
        })
        .collect::<Vec<_>>();
    let state = state_with_retained_ops(ops);
    let transition = sync::on_wakeup(
        &state,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &WakeToken::GossipTick {
            peer: NodeId::from("node-b"),
        },
    );

    let Effect::Gossip { msg, .. } = &transition.effects[0] else {
        panic!("round starts with gossip");
    };
    let WireMsg::SyncStart { heads, .. } = msg.as_ref() else {
        panic!("round starts with SyncStart");
    };
    let (tag, body) = encode_wire_msg(msg);
    assert!(body.len() <= MAX_MESSAGE_BYTES, "{} byte body", body.len());
    assert_eq!(decode_wire_msg(tag, &body), Ok(msg.as_ref().clone()));
    assert!(!heads.is_empty());
    assert!(heads.len() < 300, "oversized summary must omit only heads");
}

#[test]
fn matching_peer_head_sends_only_the_correlated_terminal() {
    let op = signed_claim();
    let transition = sync::on_message(
        &state_with_retained_op(&op),
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        &NodeId::from("node-b"),
        &WireMsg::SyncStart {
            sync_id: SyncId::from("round-a"),
            heads: vec![StreamHead {
                stream: stream(),
                origin: Principal::from("principal-a"),
                seq: 0,
                hash: op_hash(&op),
            }],
        },
        &VerificationBatch::default(),
    );

    assert_eq!(
        transition.effects,
        vec![Effect::Gossip {
            to: NodeId::from("node-b"),
            msg: Box::new(WireMsg::SyncEnd {
                sync_id: SyncId::from("round-a"),
            }),
        }]
    );
}

#[test]
fn retained_sync_fixture_starts_with_a_ready_clock() {
    assert!(matches!(
        state_with_retained_op(&signed_claim()).clock(),
        ClockState::Ready { .. }
    ));
}

#[test]
fn sync_retry_is_correlated_and_sync_end_is_terminal() {
    let peer = NodeId::from("node-b");
    let (state, effects) = step(
        state_with_retained_op(&signed_claim()),
        StepCtx {
            mono: MonoInstant(10),
            wall: WallMs(100_010),
        },
        Event::Wakeup {
            token: WakeToken::GossipTick { peer: peer.clone() },
        },
    );
    let ((round_peer, sync_id), progress) = state.sync().iter().next().expect("active round");
    assert_eq!(round_peer, &peer);
    assert_eq!(progress, &RoundProgress::Active { attempt: 0 });
    assert!(matches!(
        effects.as_slice(),
        [Effect::Gossip { .. }, Effect::Schedule { .. }]
    ));

    let sync_id = sync_id.clone();
    let (state, retry_effects) = step(
        state,
        StepCtx {
            mono: MonoInstant(10_010),
            wall: WallMs(110_010),
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
        state.sync().get(&(peer.clone(), sync_id.clone())),
        Some(&RoundProgress::Active { attempt: 1 })
    );
    assert!(matches!(
        retry_effects.as_slice(),
        [Effect::Gossip { .. }, Effect::Schedule { .. }]
    ));

    let (state, terminal_effects) = step(
        state,
        StepCtx {
            mono: MonoInstant(10_011),
            wall: WallMs(110_011),
        },
        Event::Deliver {
            from: peer.clone(),
            msg: Box::new(WireMsg::SyncEnd {
                sync_id: sync_id.clone(),
            }),
            verification: VerificationBatch::default(),
        },
    );
    assert!(terminal_effects.is_empty());
    assert_eq!(
        state.sync().get(&(peer, sync_id)),
        Some(&RoundProgress::Complete)
    );
}

#[test]
fn sync_timeout_budget_ends_in_stale_without_assuming_completion() {
    let peer = NodeId::from("node-b");
    let (mut state, _) = step(
        state_with_retained_op(&signed_claim()),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
        Event::Wakeup {
            token: WakeToken::GossipTick { peer: peer.clone() },
        },
    );
    let sync_id = state.sync().keys().next().expect("round key").1.clone();
    for attempt in 0..=3 {
        let (next, effects) = step(
            state,
            StepCtx {
                mono: MonoInstant(10_000 * u64::from(attempt + 1)),
                wall: WallMs(110_000 + i64::from(attempt)),
            },
            Event::Wakeup {
                token: WakeToken::SyncTimeout {
                    peer: peer.clone(),
                    sync_id: sync_id.clone(),
                    attempt,
                },
            },
        );
        if attempt < 3 {
            assert_eq!(
                next.sync().get(&(peer.clone(), sync_id.clone())),
                Some(&RoundProgress::Active {
                    attempt: attempt + 1,
                })
            );
            assert_eq!(effects.len(), 2);
        } else {
            assert_eq!(
                next.sync().get(&(peer.clone(), sync_id.clone())),
                Some(&RoundProgress::Stale)
            );
            assert!(effects.is_empty());
        }
        state = next;
    }
}

#[test]
fn sync_ops_fold_only_for_the_active_peer_qualified_round() {
    let peer = NodeId::from("node-b");
    let op = signed_claim();

    let (active, sync_id) = active_round(config(), &peer);
    let (folded, _) = deliver_sync_ops(active, &peer, &sync_id, op.clone());
    assert!(
        folded
            .persisted()
            .retained
            .get(&op.envelope().stream)
            .is_some_and(|records| records.contains_key(&glade_discover_protocol::record_id(&op))),
        "control: active peer-qualified SyncOps folds"
    );

    let (active, _) = active_round(config(), &peer);
    let (unknown, effects) =
        deliver_sync_ops(active, &peer, &SyncId::from("unknown-round"), op.clone());
    assert!(unknown.persisted().retained.is_empty());
    assert!(effects.is_empty());

    let (active, sync_id) = active_round(config(), &peer);
    let (wrong_peer, effects) =
        deliver_sync_ops(active, &NodeId::from("node-c"), &sync_id, op.clone());
    assert!(wrong_peer.persisted().retained.is_empty());
    assert!(effects.is_empty());

    let (active, sync_id) = active_round(config(), &peer);
    let (complete, _) = step(
        active,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        Event::Deliver {
            from: peer.clone(),
            msg: Box::new(WireMsg::SyncEnd {
                sync_id: sync_id.clone(),
            }),
            verification: VerificationBatch::default(),
        },
    );
    let (post_end, effects) = deliver_sync_ops(complete, &peer, &sync_id, op.clone());
    assert!(post_end.persisted().retained.is_empty());
    assert!(effects.is_empty());

    let mut no_retries = config();
    no_retries.sync_retries = 0;
    let (active, sync_id) = active_round(no_retries, &peer);
    let (stale, _) = step(
        active,
        StepCtx {
            mono: MonoInstant(10_000),
            wall: WallMs(110_000),
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
        stale.sync().get(&(peer.clone(), sync_id.clone())),
        Some(&RoundProgress::Stale)
    );
    let (post_stale, effects) = deliver_sync_ops(stale, &peer, &sync_id, op);
    assert!(post_stale.persisted().retained.is_empty());
    assert!(effects.is_empty());
}

fn active_round(config: KernelConfig, peer: &NodeId) -> (glade_discover_core::State, SyncId) {
    let state = restore_state_for_unit_test(
        config,
        PersistedState::default(),
        glade_discover_core::WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    );
    let (state, _) = step(
        state,
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
        Event::Wakeup {
            token: WakeToken::GossipTick { peer: peer.clone() },
        },
    );
    let sync_id = state
        .sync()
        .keys()
        .find(|(round_peer, _)| round_peer == peer)
        .expect("active round")
        .1
        .clone();
    (state, sync_id)
}

fn deliver_sync_ops(
    state: glade_discover_core::State,
    from: &NodeId,
    sync_id: &SyncId,
    op: glade_discover_protocol::SignedOp,
) -> (glade_discover_core::State, Vec<Effect>) {
    step(
        state,
        StepCtx {
            mono: MonoInstant(1),
            wall: WallMs(100_001),
        },
        Event::Deliver {
            from: from.clone(),
            msg: Box::new(WireMsg::SyncOps {
                sync_id: sync_id.clone(),
                ops: vec![op],
            }),
            verification: VerificationBatch(vec![VerificationResult::Valid {
                signer: Principal::from("principal-a"),
            }]),
        },
    )
}
