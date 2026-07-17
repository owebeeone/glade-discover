use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::claims::{ClaimDisposition, ClaimOutcome};
use glade_discover_core::{
    ClaimCommand, ClaimMode, ClockState, Effect, Event, KernelConfig, MineStatus, MonoInstant,
    NodePrincipalBinding, OwnClaim, PersistedState, PrincipalPlane, StepCtx, VerificationBatch,
    VerificationResult, WakeToken, WallMs, WatermarkLoad, claims, prepare_wall_schedule,
    restore_fresh, restore_with_recovery, step,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityRevocation, CapabilityVerb, ClaimDraft, ClaimId, ClaimIdentity,
    DefRevId, DirectoryRecord, ExecutionScope, GrantId, GrantScope, NodeId, OpEnvelope, Principal,
    RecordId, ServeClaim, Shape, Slot, StreamId, WireMsg, encode_directory_record,
    encode_signed_op,
};

fn slot() -> Slot {
    Slot::Workspace {
        share: "workspace-a".into(),
    }
}

fn service_slot() -> Slot {
    Slot::Binding {
        share: "svc".into(),
        glade_id: "glade-a".into(),
        key: vec![0xaa],
    }
}

fn record_id(key: u8, origin: &str) -> RecordId {
    RecordId {
        stream: StreamId {
            share: "workspace-a".into(),
            glade_id: "directory".into(),
            key: vec![key],
        },
        origin: Principal::from(origin),
        seq: 0,
    }
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("node-principal-a"),
        peers: BTreeSet::new(),
        workspace_owner_roots: BTreeMap::from([
            ("workspace-a".into(), Principal::from("owner-a")),
            ("svc".into(), Principal::from("owner-a")),
        ]),
        node_principal_bindings: BTreeSet::from([
            NodePrincipalBinding {
                node: NodeId::from("node-a"),
                principal: Principal::from("node-principal-a"),
                plane: PrincipalPlane::Workspace,
            },
            NodePrincipalBinding {
                node: NodeId::from("node-a"),
                principal: Principal::from("node-principal-a"),
                plane: PrincipalPlane::Derived,
            },
        ]),
        skew_margin_ms: 5_000,
        max_lease_ms: 3_600_000,
        clock_resync_ms: 30_000,
        sync_timeout_ms: NonZeroU64::new(10_000).expect("nonzero"),
        sync_retries: 3,
        gossip_fan: 8,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("nonzero"),
    }
}

fn ctx(wall: i64) -> StepCtx {
    StepCtx {
        mono: MonoInstant(10),
        wall: WallMs(wall),
    }
}

fn signed_record(record: DirectoryRecord, id: &RecordId) -> glade_discover_protocol::SignedOp {
    let payload = encode_directory_record(&record).expect("record payload");
    encode_signed_op(
        &OpEnvelope {
            stream: id.stream.clone(),
            origin: id.origin.clone(),
            seq: id.seq,
            prev: None,
            lamport: 1,
            refs: Vec::new(),
            shape: Shape::Log,
            payload,
        },
        &[0xa5],
    )
    .expect("signed op")
}

fn insert_record(persisted: &mut PersistedState, id: RecordId, record: DirectoryRecord) {
    let op = signed_record(record, &id);
    persisted.retained_bytes += u64::try_from(op.canonical_bytes().len()).expect("record size");
    persisted
        .retained
        .entry(id.stream.clone())
        .or_default()
        .insert(id, op);
}

fn serve_grant(persisted: &mut PersistedState, key: u8) -> GrantId {
    let id = record_id(key, "owner-a");
    let grant_id = GrantId::from(id.clone());
    insert_record(
        persisted,
        id,
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: grant_id.clone(),
            issuer: Principal::from("owner-a"),
            principal: Principal::from("node-principal-a"),
            share: "workspace-a".into(),
            verbs: vec![CapabilityVerb::Serve],
            scope: None,
        }),
    );
    grant_id
}

fn state(persisted: PersistedState, clock: ClockState) -> glade_discover_core::State {
    let load = match clock {
        ClockState::Ready { watermark } => WatermarkLoad::Readable(watermark),
        ClockState::Uncertain { floor: Some(floor) } => WatermarkLoad::Readable(floor),
        ClockState::Uncertain { floor: None } => WatermarkLoad::Unreadable,
    };
    let restore_wall = match clock {
        ClockState::Ready { watermark } => watermark.0,
        ClockState::Uncertain { floor: Some(floor) } => floor.0 - 1,
        ClockState::Uncertain { floor: None } => 1_000,
    };
    restore_fresh(config(), persisted, load, ctx(restore_wall))
}

fn initial_command(grant_ref: GrantId, expiry: i64) -> ClaimCommand {
    ClaimCommand {
        intent: "intent-initial".into(),
        slot: slot(),
        generation: glade_discover_protocol::Generation(1),
        mode: ClaimMode::Initial,
        draft: ClaimDraft::Workspace {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            identity: ClaimIdentity::Mint,
            grant_ref,
            lease_expiry_ms: expiry,
            epoch: 0,
        },
    }
}

fn assert_accepted(outcome: &ClaimOutcome) {
    assert_eq!(outcome.disposition, ClaimDisposition::Accepted);
    assert!(matches!(
        outcome.transition.effects.as_slice(),
        [Effect::Append { .. }]
    ));
}

#[test]
fn valid_initial_advertisement_becomes_durable_pending_before_append() {
    let mut persisted = PersistedState::default();
    let grant = serve_grant(&mut persisted, 0xa1);
    let command = initial_command(grant, 60_000);
    let outcome = claims::on_advertise(
        &state(
            persisted,
            ClockState::Ready {
                watermark: WallMs(1_000),
            },
        ),
        ctx(1_000),
        &command,
    );

    assert_accepted(&outcome);
    assert_eq!(
        outcome
            .transition
            .persisted
            .mine
            .get(&(slot(), glade_discover_protocol::Generation(1)))
            .expect("pending own claim")
            .status,
        MineStatus::Pending
    );
}

#[test]
fn uncertain_expired_and_overlong_advertisements_fail_closed() {
    let mut persisted = PersistedState::default();
    let grant = serve_grant(&mut persisted, 0xa1);
    let unknown = state(persisted.clone(), ClockState::Uncertain { floor: None });
    assert_eq!(
        claims::on_advertise(
            &unknown,
            ctx(1_000),
            &initial_command(grant.clone(), 60_000)
        )
        .disposition,
        ClaimDisposition::ClockUncertain
    );
    let ready = state(
        persisted,
        ClockState::Ready {
            watermark: WallMs(10_000),
        },
    );
    assert_eq!(
        claims::on_advertise(&ready, ctx(10_000), &initial_command(grant.clone(), 15_000))
            .disposition,
        ClaimDisposition::Expired
    );
    assert_eq!(
        claims::on_advertise(&ready, ctx(10_000), &initial_command(grant, 3_615_001)).disposition,
        ClaimDisposition::LeaseTooLong
    );
}

#[test]
fn renewal_requires_a_new_generation_but_preserves_claim_id_and_epoch() {
    let mut persisted = PersistedState::default();
    let grant = serve_grant(&mut persisted, 0xa1);
    let claim_id = ClaimId::from(record_id(0x01, "node-principal-a"));
    insert_record(
        &mut persisted,
        claim_id.record().clone(),
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            claim_id: claim_id.clone(),
            grant_ref: grant.clone(),
            lease_expiry_ms: 60_000,
            epoch: 4,
        }),
    );
    let old_draft = ClaimDraft::Workspace {
        node: NodeId::from("node-a"),
        share: "workspace-a".into(),
        identity: ClaimIdentity::Mint,
        grant_ref: grant.clone(),
        lease_expiry_ms: 60_000,
        epoch: 4,
    };
    persisted.mine.insert(
        (slot(), glade_discover_protocol::Generation(1)),
        OwnClaim {
            intent: "intent-old".into(),
            draft: old_draft,
            status: MineStatus::Accepted,
        },
    );
    let ready = restore_with_recovery(
        config(),
        persisted,
        WatermarkLoad::Readable(WallMs(10_000)),
        ctx(10_000),
    )
    .0;
    let mut renew = ClaimCommand {
        intent: "intent-renew".into(),
        slot: slot(),
        generation: glade_discover_protocol::Generation(2),
        mode: ClaimMode::Renew,
        draft: ClaimDraft::Workspace {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            identity: ClaimIdentity::Existing(claim_id.clone()),
            grant_ref: grant,
            lease_expiry_ms: 70_000,
            epoch: 4,
        },
    };
    assert_accepted(&claims::on_advertise(&ready, ctx(10_000), &renew));

    renew.generation = glade_discover_protocol::Generation(1);
    assert_eq!(
        claims::on_advertise(&ready, ctx(10_000), &renew).disposition,
        ClaimDisposition::StaleGeneration
    );
    renew.generation = glade_discover_protocol::Generation(2);
    if let ClaimDraft::Workspace { epoch, .. } = &mut renew.draft {
        *epoch = 5;
    }
    assert_eq!(
        claims::on_advertise(&ready, ctx(10_000), &renew).disposition,
        ClaimDisposition::IdentityMismatch
    );
    if let ClaimDraft::Workspace {
        identity, epoch, ..
    } = &mut renew.draft
    {
        *epoch = 4;
        *identity = ClaimIdentity::Existing(ClaimId::from(record_id(0x02, "node-principal-a")));
    }
    assert_eq!(
        claims::on_advertise(&ready, ctx(10_000), &renew).disposition,
        ClaimDisposition::IdentityMismatch
    );
}

#[test]
fn late_renewal_does_not_resurrect_an_expired_claim() {
    let mut persisted = PersistedState::default();
    let grant = serve_grant(&mut persisted, 0xa1);
    let claim_id = ClaimId::from(record_id(0x01, "node-principal-a"));
    persisted.mine.insert(
        (slot(), glade_discover_protocol::Generation(1)),
        OwnClaim {
            intent: "intent-old".into(),
            draft: ClaimDraft::Workspace {
                node: NodeId::from("node-a"),
                share: "workspace-a".into(),
                identity: ClaimIdentity::Existing(claim_id.clone()),
                grant_ref: grant.clone(),
                lease_expiry_ms: 15_000,
                epoch: 4,
            },
            status: MineStatus::Accepted,
        },
    );
    let renew = ClaimCommand {
        intent: "intent-renew".into(),
        slot: slot(),
        generation: glade_discover_protocol::Generation(2),
        mode: ClaimMode::Renew,
        draft: ClaimDraft::Workspace {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            identity: ClaimIdentity::Existing(claim_id),
            grant_ref: grant,
            lease_expiry_ms: 80_000,
            epoch: 4,
        },
    };
    let outcome = claims::on_advertise(
        &state(
            persisted,
            ClockState::Ready {
                watermark: WallMs(10_000),
            },
        ),
        ctx(10_000),
        &renew,
    );
    assert_eq!(outcome.disposition, ClaimDisposition::Expired);
}

#[test]
fn takeover_requires_exact_scope_and_checked_epoch_increment() {
    let mut persisted = PersistedState::default();
    let local_serve_grant = serve_grant(&mut persisted, 0xa1);
    let superseded_id = record_id(0x01, "node-principal-b");
    let superseded_claim_id = ClaimId::from(superseded_id.clone());
    let serve_grant_id = GrantId::from(record_id(0xa2, "owner-a"));
    insert_record(
        &mut persisted,
        superseded_id.clone(),
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("node-b"),
            share: "workspace-a".into(),
            claim_id: superseded_claim_id.clone(),
            grant_ref: serve_grant_id,
            lease_expiry_ms: 60_000,
            epoch: 4,
        }),
    );
    let takeover_record_id = record_id(0xf1, "owner-a");
    let takeover_grant = GrantId::from(takeover_record_id.clone());
    insert_record(
        &mut persisted,
        takeover_record_id,
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: takeover_grant.clone(),
            issuer: Principal::from("owner-a"),
            principal: Principal::from("node-principal-a"),
            share: "workspace-a".into(),
            verbs: vec![CapabilityVerb::Takeover],
            scope: Some(GrantScope::Takeover {
                slot: slot(),
                supersedes: superseded_claim_id,
            }),
        }),
    );
    let ready = state(
        persisted,
        ClockState::Ready {
            watermark: WallMs(10_000),
        },
    );
    let mut takeover = initial_command(local_serve_grant.clone(), 80_000);
    takeover.intent = "intent-takeover".into();
    takeover.mode = ClaimMode::Takeover {
        authority_ref: takeover_grant,
    };
    if let ClaimDraft::Workspace { epoch, .. } = &mut takeover.draft {
        *epoch = 5;
    }
    assert_accepted(&claims::on_advertise(&ready, ctx(10_000), &takeover));

    if let ClaimDraft::Workspace { epoch, .. } = &mut takeover.draft {
        *epoch = 6;
    }
    assert_eq!(
        claims::on_advertise(&ready, ctx(10_000), &takeover).disposition,
        ClaimDisposition::UnauthorizedTakeover
    );
    if let ClaimDraft::Workspace { epoch, .. } = &mut takeover.draft {
        *epoch = 5;
    }
    takeover.mode = ClaimMode::Takeover {
        authority_ref: local_serve_grant,
    };
    assert_eq!(
        claims::on_advertise(&ready, ctx(10_000), &takeover).disposition,
        ClaimDisposition::UnauthorizedTakeover
    );
}

#[test]
fn stale_wakeup_is_inert_and_losing_service_instance_tears_down_once() {
    let winner = ClaimId::from(record_id(0xc2, "node-principal-b"));
    let (persisted, _) = accepted_local_service();
    let ready = state(
        persisted,
        ClockState::Ready {
            watermark: WallMs(10_000),
        },
    );
    let stale = claims::on_wakeup(
        &ready,
        ctx(10_000),
        &glade_discover_core::WakeToken::ClaimRenew {
            slot: service_slot(),
            generation: glade_discover_protocol::Generation(99),
        },
    );
    assert!(stale.effects.is_empty());
    assert_eq!(stale.persisted, *ready.persisted());

    let lost = claims::on_slot_winner(&ready, &service_slot(), Some(&winner));
    assert_eq!(
        lost.effects,
        vec![Effect::Teardown {
            slot: service_slot(),
            generation: glade_discover_protocol::Generation(3),
        }]
    );
    assert_eq!(
        lost.persisted
            .mine
            .get(&(service_slot(), glade_discover_protocol::Generation(3)))
            .expect("lost own claim")
            .status,
        MineStatus::Lost
    );
    let already_lost = state(
        lost.persisted,
        ClockState::Ready {
            watermark: WallMs(10_000),
        },
    );
    assert!(
        claims::on_slot_winner(&already_lost, &service_slot(), Some(&winner))
            .effects
            .is_empty()
    );
}

#[test]
fn revoked_or_expired_local_service_is_torn_down_once_but_uncertain_clock_is_inert() {
    let (persisted, revocation) = accepted_local_service();
    let ready = state(
        persisted.clone(),
        ClockState::Ready {
            watermark: WallMs(1_000),
        },
    );
    let (revoked, effects) = step(
        ready,
        ctx(1_001),
        Event::Deliver {
            from: NodeId::from("owner-node"),
            msg: Box::new(WireMsg::DirOp {
                op: Box::new(revocation.clone()),
            }),
            verification: VerificationBatch(vec![VerificationResult::Valid {
                signer: Principal::from("owner-a"),
            }]),
        },
    );
    assert_eq!(effects, vec![service_teardown()]);
    assert_eq!(
        revoked
            .persisted()
            .mine
            .get(&(service_slot(), glade_discover_protocol::Generation(3)))
            .expect("local service")
            .status,
        MineStatus::Lost
    );
    let (_, duplicate_effects) = step(
        revoked,
        ctx(1_002),
        Event::Deliver {
            from: NodeId::from("owner-node"),
            msg: Box::new(WireMsg::DirOp {
                op: Box::new(revocation),
            }),
            verification: VerificationBatch(vec![VerificationResult::Valid {
                signer: Principal::from("owner-a"),
            }]),
        },
    );
    assert!(duplicate_effects.is_empty(), "teardown is one-shot");

    let ready = state(
        persisted.clone(),
        ClockState::Ready {
            watermark: WallMs(1_000),
        },
    );
    let (expired, effects) = step(
        ready,
        ctx(15_000),
        Event::Wakeup {
            token: glade_discover_core::WakeToken::ClaimRenew {
                slot: service_slot(),
                generation: glade_discover_protocol::Generation(3),
            },
        },
    );
    assert_eq!(effects, vec![service_teardown()]);
    assert_eq!(
        expired
            .persisted()
            .mine
            .get(&(service_slot(), glade_discover_protocol::Generation(3)))
            .expect("expired local service")
            .status,
        MineStatus::Lost
    );

    let uncertain = state(persisted, ClockState::Uncertain { floor: None });
    let (uncertain, effects) = step(
        uncertain,
        ctx(i64::MAX),
        Event::Wakeup {
            token: glade_discover_core::WakeToken::ClaimRenew {
                slot: service_slot(),
                generation: glade_discover_protocol::Generation(3),
            },
        },
    );
    assert!(effects.is_empty());
    assert_eq!(
        uncertain
            .persisted()
            .mine
            .get(&(service_slot(), glade_discover_protocol::Generation(3)))
            .expect("uncertain local service")
            .status,
        MineStatus::Accepted
    );
}

#[test]
fn restore_marks_a_service_expired_during_downtime_lost_and_recovers_teardown() {
    let (persisted, _) = accepted_local_service();

    let (restored, recovery) = restore_with_recovery(
        config(),
        persisted,
        WatermarkLoad::Readable(WallMs(1_000)),
        ctx(15_000),
    );

    assert_eq!(recovery, vec![service_teardown()]);
    assert_eq!(
        restored
            .persisted()
            .mine
            .get(&(service_slot(), glade_discover_protocol::Generation(3)))
            .expect("expired local service")
            .status,
        MineStatus::Lost
    );
}

#[test]
fn wall_schedule_crossing_service_expiry_reconciles_before_timer_effect() {
    let (persisted, _) = accepted_local_service();
    let ready = state(
        persisted,
        ClockState::Ready {
            watermark: WallMs(1_000),
        },
    );

    let (scheduled, effects) = prepare_wall_schedule(
        ready,
        ctx(15_000),
        WallMs(16_000),
        WakeToken::GossipTick {
            peer: NodeId::from("peer"),
        },
    )
    .expect("ready wall schedule");

    assert_eq!(
        effects,
        vec![
            service_teardown(),
            Effect::Schedule {
                token: WakeToken::GossipTick {
                    peer: NodeId::from("peer"),
                },
                at_mono: MonoInstant(1_010),
            },
        ]
    );
    assert_eq!(
        scheduled
            .persisted()
            .mine
            .get(&(service_slot(), glade_discover_protocol::Generation(3)))
            .expect("expired local service")
            .status,
        MineStatus::Lost
    );
}

fn accepted_local_service() -> (PersistedState, glade_discover_protocol::SignedOp) {
    let mut persisted = PersistedState::default();
    let def_ref = DefRevId::from(record_id(0xd0, "def-a"));
    let compute_key: glade_discover_protocol::ComputeKey = vec![0xc0].into();
    let grant_record_id = record_id(0xe0, "owner-a");
    let grant_id = GrantId::from(grant_record_id.clone());
    insert_record(
        &mut persisted,
        grant_record_id,
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: grant_id.clone(),
            issuer: Principal::from("owner-a"),
            principal: Principal::from("node-principal-a"),
            share: "svc".into(),
            verbs: vec![CapabilityVerb::Execute],
            scope: Some(GrantScope::Execution(ExecutionScope {
                def_ref: def_ref.clone(),
                compute_key: compute_key.clone(),
            })),
        }),
    );
    let claim_record_id = record_id(0xc1, "node-principal-a");
    let claim_id = ClaimId::from(claim_record_id.clone());
    insert_record(
        &mut persisted,
        claim_record_id,
        DirectoryRecord::ServiceInstanceClaim(glade_discover_protocol::ServiceInstanceClaim {
            node: NodeId::from("node-a"),
            share: "svc".into(),
            glade_id: "glade-a".into(),
            key: vec![0xaa],
            claim_id: claim_id.clone(),
            def_ref: def_ref.clone(),
            exec_grant_ref: grant_id.clone(),
            compute_key: compute_key.clone(),
            lease_expiry_ms: 20_000,
            epoch: 4,
        }),
    );
    persisted.mine.insert(
        (service_slot(), glade_discover_protocol::Generation(3)),
        OwnClaim {
            intent: "intent-service".into(),
            draft: ClaimDraft::Service {
                node: NodeId::from("node-a"),
                share: "svc".into(),
                glade_id: "glade-a".into(),
                key: vec![0xaa],
                identity: ClaimIdentity::Existing(claim_id),
                def_ref,
                exec_grant_ref: grant_id.clone(),
                compute_key,
                lease_expiry_ms: 20_000,
                epoch: 4,
            },
            status: MineStatus::Accepted,
        },
    );
    let revocation_id = record_id(0xf0, "owner-a");
    let revocation = signed_record(
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
            revokes: grant_id.clone(),
        }),
        &revocation_id,
    );
    (persisted, revocation)
}

fn service_teardown() -> Effect {
    Effect::Teardown {
        slot: service_slot(),
        generation: glade_discover_protocol::Generation(3),
    }
}
