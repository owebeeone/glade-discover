use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    ClockState, GovernanceVerdict, IngestDisposition, KernelConfig, LiveCapabilityVerdict,
    NodePrincipalBinding, PersistedState, PrincipalPlane, StructuralVerdict, VerificationResult,
    WallMs, ingest, projection,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityRevocation, CapabilityVerb, ClaimId, ComputeKey, DefRevId,
    DirectoryRecord, ExecutionScope, GrantId, GrantScope, NodeId, OpEnvelope, Principal, RecordId,
    ServeClaim, ServiceInstanceClaim, Shape, SignedOp, Slot, StreamId, encode_directory_record,
    encode_signed_op, op_hash,
};

fn stream(glade_id: &str) -> StreamId {
    StreamId {
        share: "home".into(),
        glade_id: glade_id.into(),
        key: Vec::new(),
    }
}

fn record_id(stream: &StreamId, origin: &str, seq: u64) -> RecordId {
    RecordId {
        stream: stream.clone(),
        origin: Principal::from(origin),
        seq,
    }
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("router"),
        local_principal: Principal::from("owner-a"),
        peers: BTreeSet::new(),
        workspace_owner_roots: BTreeMap::from([
            ("workspace-a".into(), Principal::from("owner-a")),
            ("workspace-b".into(), Principal::from("owner-b")),
        ]),
        node_principal_bindings: BTreeSet::from([
            NodePrincipalBinding {
                node: NodeId::from("node-a"),
                principal: Principal::from("node-principal-a"),
                plane: PrincipalPlane::Workspace,
            },
            NodePrincipalBinding {
                node: NodeId::from("svc-node"),
                principal: Principal::from("svc-principal"),
                plane: PrincipalPlane::Derived,
            },
        ]),
        skew_margin_ms: 5,
        max_lease_ms: 1_000,
        clock_resync_ms: 30,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(10_000).expect("non-zero"),
        gossip_fan: 8,
        max_retained_bytes: NonZeroU64::new(1_000_000).expect("non-zero"),
    }
}

fn signed(
    envelope_stream: StreamId,
    origin: &str,
    seq: u64,
    prev: Option<[u8; 32]>,
    record: DirectoryRecord,
) -> SignedOp {
    let payload = encode_directory_record(&record).expect("canonical directory record");
    encode_signed_op(
        &OpEnvelope {
            stream: envelope_stream,
            origin: Principal::from(origin),
            seq,
            prev,
            lamport: seq,
            refs: Vec::new(),
            shape: Shape::Log,
            payload,
        },
        &[0xAA],
    )
    .expect("canonical signed op")
}

fn grant(
    origin: &str,
    principal: &str,
    share: &str,
    verbs: Vec<CapabilityVerb>,
    scope: Option<GrantScope>,
) -> (GrantId, SignedOp) {
    let grant_stream = stream("dir.grants");
    let id = GrantId::from(record_id(&grant_stream, origin, 0));
    let op = signed(
        grant_stream,
        origin,
        0,
        None,
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: id.clone(),
            issuer: Principal::from(origin),
            principal: Principal::from(principal),
            share: share.into(),
            verbs,
            scope,
        }),
    );
    (id, op)
}

fn workspace_claim(grant_ref: GrantId, expiry: i64) -> SignedOp {
    let claim_stream = stream("dir.claims");
    let id = ClaimId::from(record_id(&claim_stream, "node-principal-a", 0));
    signed(
        claim_stream,
        "node-principal-a",
        0,
        None,
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            claim_id: id,
            grant_ref,
            lease_expiry_ms: expiry,
            epoch: 1,
        }),
    )
}

fn derived_grant(subject: &str, def_ref: DefRevId, compute_key: ComputeKey) -> (GrantId, SignedOp) {
    grant(
        "owner-a",
        subject,
        "workspace-a",
        vec![CapabilityVerb::Execute],
        Some(GrantScope::Execution(ExecutionScope {
            def_ref,
            compute_key,
        })),
    )
}

fn derived_claim(
    grant_ref: GrantId,
    origin: &str,
    node: &str,
    def_ref: DefRevId,
    compute_key: ComputeKey,
) -> SignedOp {
    let claim_stream = stream("dir.instances");
    signed(
        claim_stream.clone(),
        origin,
        0,
        None,
        DirectoryRecord::ServiceInstanceClaim(ServiceInstanceClaim {
            node: NodeId::from(node),
            share: "svc".into(),
            glade_id: "diff".into(),
            key: vec![7],
            claim_id: ClaimId::from(record_id(&claim_stream, origin, 0)),
            def_ref,
            exec_grant_ref: grant_ref,
            compute_key,
            lease_expiry_ms: 1_500,
            epoch: 2,
        }),
    )
}

fn ready() -> ClockState {
    ClockState::Ready {
        watermark: WallMs(1_000),
    }
}

fn apply(
    persisted: &PersistedState,
    op: &SignedOp,
    signer: &str,
) -> glade_discover_core::ingest::IngestOutcome {
    ingest::ingest(
        op,
        &VerificationResult::Valid {
            signer: Principal::from(signer),
        },
        persisted,
        &config(),
        ready(),
    )
}

#[test]
fn structural_governance_and_live_capability_verdicts_are_separate() {
    let (grant_id, grant_op) = grant(
        "attacker",
        "node-principal-a",
        "workspace-a",
        vec![CapabilityVerb::Serve],
        None,
    );
    let bad_signature = ingest::ingest(
        &grant_op,
        &VerificationResult::BadSignature,
        &PersistedState::default(),
        &config(),
        ready(),
    );
    assert_eq!(bad_signature.structural, StructuralVerdict::BadSignature);
    assert_eq!(bad_signature.governance, None);
    assert_eq!(bad_signature.disposition, IngestDisposition::Quarantined);

    let unauthorized = apply(&PersistedState::default(), &grant_op, "attacker");
    assert_eq!(
        unauthorized.structural,
        StructuralVerdict::StructurallyValid
    );
    assert_eq!(
        unauthorized.governance,
        Some(GovernanceVerdict::Unauthorized)
    );
    assert_eq!(unauthorized.capability, None);
    assert_eq!(unauthorized.disposition, IngestDisposition::Rejected);

    let claim = workspace_claim(grant_id.clone(), 1_500);
    let unresolved = apply(&PersistedState::default(), &claim, "node-principal-a");
    assert_eq!(unresolved.structural, StructuralVerdict::StructurallyValid);
    assert_eq!(unresolved.governance, Some(GovernanceVerdict::Authorized));
    assert_eq!(
        unresolved.capability,
        Some(LiveCapabilityVerdict::UnresolvedProof)
    );
    assert_eq!(unresolved.disposition, IngestDisposition::Folded);
    assert_eq!(
        unresolved.persisted.unresolved.get(&grant_id),
        Some(&BTreeSet::from([ClaimId::from(record_id(
            &stream("dir.claims"),
            "node-principal-a",
            0,
        ))]))
    );
}

#[test]
fn proof_arrival_clears_pending_and_revocation_suppresses_projection() {
    let (grant_id, grant_op) = grant(
        "owner-a",
        "node-principal-a",
        "workspace-a",
        vec![CapabilityVerb::Serve],
        None,
    );
    let claim_op = workspace_claim(grant_id.clone(), 1_500);
    let claim_out = apply(&PersistedState::default(), &claim_op, "node-principal-a");
    assert!(claim_out.persisted.unresolved.contains_key(&grant_id));

    let grant_out = apply(&claim_out.persisted, &grant_op, "owner-a");
    assert!(!grant_out.persisted.unresolved.contains_key(&grant_id));
    assert_eq!(
        projection::live_claims(&grant_out.persisted, &config(), ready()).len(),
        1
    );

    let revoke_stream = stream("dir.revocations");
    let revocation = signed(
        revoke_stream,
        "owner-a",
        0,
        None,
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation { revokes: grant_id }),
    );
    let revoked = apply(&grant_out.persisted, &revocation, "owner-a");
    assert_eq!(revoked.governance, Some(GovernanceVerdict::Authorized));
    assert!(projection::live_claims(&revoked.persisted, &config(), ready()).is_empty());
}

#[test]
fn revocation_before_grant_is_retained_and_applies_only_for_the_exact_owner() {
    let (grant_id, grant_op) = grant(
        "owner-a",
        "node-principal-a",
        "workspace-a",
        vec![CapabilityVerb::Serve],
        None,
    );
    let revoke_stream = stream("dir.revocations");
    let revocation = signed(
        revoke_stream.clone(),
        "owner-a",
        0,
        None,
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
            revokes: grant_id.clone(),
        }),
    );
    let first = apply(&PersistedState::default(), &revocation, "owner-a");
    assert_eq!(first.governance, Some(GovernanceVerdict::Authorized));
    assert_eq!(first.disposition, IngestDisposition::Folded);

    let claim_out = apply(
        &first.persisted,
        &workspace_claim(grant_id, 1_500),
        "node-principal-a",
    );
    let grant_out = apply(&claim_out.persisted, &grant_op, "owner-a");
    assert!(projection::live_claims(&grant_out.persisted, &config(), ready()).is_empty());

    let wrong_owner = signed(
        revoke_stream,
        "owner-b",
        0,
        None,
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
            revokes: GrantId::from(record_id(&stream("dir.grants"), "owner-a", 0)),
        }),
    );
    let wrong_first = apply(&PersistedState::default(), &wrong_owner, "owner-b");
    let claim_out = apply(
        &wrong_first.persisted,
        &workspace_claim(
            GrantId::from(record_id(&stream("dir.grants"), "owner-a", 0)),
            1_500,
        ),
        "node-principal-a",
    );
    let grant_out = apply(&claim_out.persisted, &grant_op, "owner-a");
    assert_eq!(
        projection::live_claims(&grant_out.persisted, &config(), ready()).len(),
        1
    );
}

#[test]
fn derived_claim_requires_exact_execution_scope_subject_and_node_binding() {
    let def_ref = DefRevId::from(record_id(&stream("dir.definitions"), "owner-a", 9));
    let compute_key = ComputeKey::from(vec![1, 2, 3]);
    let (grant_id, grant_op) = grant(
        "owner-a",
        "svc-principal",
        "workspace-a",
        vec![CapabilityVerb::Execute],
        Some(GrantScope::Execution(ExecutionScope {
            def_ref: def_ref.clone(),
            compute_key: compute_key.clone(),
        })),
    );
    let claim_stream = stream("dir.instances");
    let claim_id = ClaimId::from(record_id(&claim_stream, "svc-principal", 0));
    let claim = signed(
        claim_stream,
        "svc-principal",
        0,
        None,
        DirectoryRecord::ServiceInstanceClaim(ServiceInstanceClaim {
            node: NodeId::from("svc-node"),
            share: "svc".into(),
            glade_id: "diff".into(),
            key: vec![7],
            claim_id,
            def_ref,
            exec_grant_ref: grant_id,
            compute_key,
            lease_expiry_ms: 1_500,
            epoch: 2,
        }),
    );
    let granted = apply(&PersistedState::default(), &grant_op, "owner-a");
    let claimed = apply(&granted.persisted, &claim, "svc-principal");
    assert_eq!(claimed.governance, Some(GovernanceVerdict::Authorized));
    assert_eq!(claimed.capability, Some(LiveCapabilityVerdict::Live));
    assert_eq!(
        projection::live_claims(&claimed.persisted, &config(), ready()),
        vec![projection::ProjectedClaim {
            slot: Slot::Binding {
                share: "svc".into(),
                glade_id: "diff".into(),
                key: vec![7],
            },
            node: NodeId::from("svc-node"),
            claim_id: ClaimId::from(record_id(&stream("dir.instances"), "svc-principal", 0,)),
            epoch: 2,
        }]
    );
}

#[test]
fn every_derived_authority_equality_is_independently_necessary() {
    let def_ref = DefRevId::from(record_id(&stream("dir.definitions"), "owner-a", 9));
    let other_def = DefRevId::from(record_id(&stream("dir.definitions"), "owner-a", 10));
    let compute_key = ComputeKey::from(vec![1, 2, 3]);

    let (wrong_def_id, wrong_def_grant) =
        derived_grant("svc-principal", other_def, compute_key.clone());
    let granted = apply(&PersistedState::default(), &wrong_def_grant, "owner-a");
    let claimed = apply(
        &granted.persisted,
        &derived_claim(
            wrong_def_id,
            "svc-principal",
            "svc-node",
            def_ref.clone(),
            compute_key.clone(),
        ),
        "svc-principal",
    );
    assert!(projection::live_claims(&claimed.persisted, &config(), ready()).is_empty());

    let (wrong_compute_id, wrong_compute_grant) =
        derived_grant("svc-principal", def_ref.clone(), ComputeKey::from(vec![9]));
    let granted = apply(&PersistedState::default(), &wrong_compute_grant, "owner-a");
    let claimed = apply(
        &granted.persisted,
        &derived_claim(
            wrong_compute_id,
            "svc-principal",
            "svc-node",
            def_ref.clone(),
            compute_key.clone(),
        ),
        "svc-principal",
    );
    assert!(projection::live_claims(&claimed.persisted, &config(), ready()).is_empty());

    let (wrong_subject_id, wrong_subject_grant) =
        derived_grant("other-principal", def_ref.clone(), compute_key.clone());
    let granted = apply(&PersistedState::default(), &wrong_subject_grant, "owner-a");
    let claimed = apply(
        &granted.persisted,
        &derived_claim(
            wrong_subject_id,
            "svc-principal",
            "svc-node",
            def_ref.clone(),
            compute_key.clone(),
        ),
        "svc-principal",
    );
    assert!(projection::live_claims(&claimed.persisted, &config(), ready()).is_empty());

    let (grant_id, grant_op) = derived_grant("svc-principal", def_ref.clone(), compute_key.clone());
    let granted = apply(&PersistedState::default(), &grant_op, "owner-a");
    let wrong_node = apply(
        &granted.persisted,
        &derived_claim(
            grant_id.clone(),
            "svc-principal",
            "other-node",
            def_ref.clone(),
            compute_key.clone(),
        ),
        "svc-principal",
    );
    assert_eq!(wrong_node.governance, Some(GovernanceVerdict::Unauthorized));

    let wrong_verifier = ingest::ingest(
        &derived_claim(grant_id, "svc-principal", "svc-node", def_ref, compute_key),
        &VerificationResult::Valid {
            signer: Principal::from("other-principal"),
        },
        &granted.persisted,
        &config(),
        ready(),
    );
    assert_eq!(wrong_verifier.structural, StructuralVerdict::BadSignature);
}

#[test]
fn expiry_and_retained_byte_ceiling_fail_closed() {
    let (grant_id, grant_op) = grant(
        "owner-a",
        "node-principal-a",
        "workspace-a",
        vec![CapabilityVerb::Serve],
        None,
    );
    let granted = apply(&PersistedState::default(), &grant_op, "owner-a");
    let expired = apply(
        &granted.persisted,
        &workspace_claim(grant_id.clone(), 1_005),
        "node-principal-a",
    );
    assert_eq!(expired.capability, Some(LiveCapabilityVerdict::Expired));
    assert!(projection::live_claims(&expired.persisted, &config(), ready()).is_empty());

    let too_far = apply(
        &granted.persisted,
        &workspace_claim(grant_id, 2_006),
        "node-principal-a",
    );
    assert_eq!(too_far.disposition, IngestDisposition::Rejected);

    let mut tiny = config();
    tiny.max_retained_bytes = NonZeroU64::new(
        u64::try_from(grant_op.canonical_bytes().len() - 1).expect("small fixture"),
    )
    .expect("non-zero");
    let exhausted = ingest::ingest(
        &grant_op,
        &VerificationResult::Valid {
            signer: Principal::from("owner-a"),
        },
        &PersistedState::default(),
        &tiny,
        ready(),
    );
    assert_eq!(exhausted.disposition, IngestDisposition::StorageExhausted);
    assert_eq!(exhausted.persisted, PersistedState::default());
}

#[test]
fn full_chain_validation_handles_append_duplicate_gap_fork_and_stable_renewal() {
    let (grant_id, grant_op) = grant(
        "owner-a",
        "node-principal-a",
        "workspace-a",
        vec![CapabilityVerb::Serve],
        None,
    );
    let first = apply(&PersistedState::default(), &grant_op, "owner-a");
    let duplicate = apply(&first.persisted, &grant_op, "owner-a");
    assert_eq!(duplicate.structural, StructuralVerdict::Duplicate);
    assert_eq!(duplicate.disposition, IngestDisposition::Duplicate);
    let resigned = encode_signed_op(grant_op.envelope(), &[0xBB]).expect("alternate signature");
    assert_eq!(
        apply(&first.persisted, &resigned, "owner-a").structural,
        StructuralVerdict::Duplicate
    );

    let grant_stream = stream("dir.grants");
    let fork = signed(
        grant_stream.clone(),
        "owner-a",
        0,
        None,
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: grant_id.clone(),
            issuer: Principal::from("owner-a"),
            principal: Principal::from("other-subject"),
            share: "workspace-a".into(),
            verbs: vec![CapabilityVerb::Serve],
            scope: None,
        }),
    );
    assert_eq!(
        apply(&first.persisted, &fork, "owner-a").structural,
        StructuralVerdict::BadChain
    );

    let gap = signed(
        grant_stream.clone(),
        "owner-a",
        2,
        Some(op_hash(&grant_op)),
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
            revokes: grant_id.clone(),
        }),
    );
    assert_eq!(
        apply(&first.persisted, &gap, "owner-a").structural,
        StructuralVerdict::BadChain
    );

    let next = signed(
        grant_stream,
        "owner-a",
        1,
        Some(op_hash(&grant_op)),
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
            revokes: grant_id.clone(),
        }),
    );
    assert_eq!(
        apply(&first.persisted, &next, "owner-a").disposition,
        IngestDisposition::Folded
    );

    let initial_claim = workspace_claim(grant_id, 1_500);
    let initial = apply(&first.persisted, &initial_claim, "node-principal-a");
    let claim_stream = stream("dir.claims");
    let stable_id = ClaimId::from(record_id(&claim_stream, "node-principal-a", 0));
    let renewal = signed(
        claim_stream.clone(),
        "node-principal-a",
        1,
        Some(op_hash(&initial_claim)),
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            claim_id: stable_id.clone(),
            grant_ref: GrantId::from(record_id(&stream("dir.grants"), "owner-a", 0)),
            lease_expiry_ms: 1_600,
            epoch: 1,
        }),
    );
    assert_eq!(
        apply(&initial.persisted, &renewal, "node-principal-a").disposition,
        IngestDisposition::Folded
    );

    let changed_epoch = signed(
        claim_stream,
        "node-principal-a",
        1,
        Some(op_hash(&initial_claim)),
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            claim_id: stable_id,
            grant_ref: GrantId::from(record_id(&stream("dir.grants"), "owner-a", 0)),
            lease_expiry_ms: 1_600,
            epoch: 2,
        }),
    );
    assert_eq!(
        apply(&initial.persisted, &changed_epoch, "node-principal-a").structural,
        StructuralVerdict::BadChain
    );
}

#[test]
fn clock_recovery_revalidates_and_deaccounts_time_deferred_claims() {
    let missing_grant = GrantId::from(record_id(&stream("dir.grants"), "owner-a", 0));
    let valid_claim = workspace_claim(missing_grant.clone(), 1_500);
    let valid = ingest::ingest(
        &valid_claim,
        &VerificationResult::Valid {
            signer: Principal::from("node-principal-a"),
        },
        &PersistedState::default(),
        &config(),
        ClockState::Uncertain {
            floor: Some(WallMs(1_000)),
        },
    );
    assert_eq!(valid.disposition, IngestDisposition::TimeDeferred);
    let recovered = ingest::revalidate_time_deferred(&valid.persisted, &config(), WallMs(1_000));
    assert!(recovered.time_deferred.is_empty());
    assert_eq!(recovered.retained, valid.persisted.retained);
    assert!(recovered.unresolved.contains_key(&missing_grant));

    let excessive_claim = workspace_claim(missing_grant.clone(), 2_006);
    let excessive = ingest::ingest(
        &excessive_claim,
        &VerificationResult::Valid {
            signer: Principal::from("node-principal-a"),
        },
        &PersistedState::default(),
        &config(),
        ClockState::Uncertain {
            floor: Some(WallMs(1_000)),
        },
    );
    let rejected = ingest::revalidate_time_deferred(&excessive.persisted, &config(), WallMs(1_000));
    assert!(rejected.retained.is_empty());
    assert_eq!(rejected.retained_bytes, 0);
    assert!(rejected.time_deferred.is_empty());
    assert!(rejected.unresolved.is_empty());

    let overflowed =
        ingest::revalidate_time_deferred(&excessive.persisted, &config(), WallMs(i64::MAX));
    assert!(overflowed.retained.is_empty());
    assert_eq!(overflowed.retained_bytes, 0);
}
