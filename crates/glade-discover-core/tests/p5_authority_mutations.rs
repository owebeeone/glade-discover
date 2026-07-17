use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    ClockState, GovernanceVerdict, IngestDisposition, KernelConfig, NodePrincipalBinding,
    PrincipalPlane, StructuralVerdict, VerificationResult, WallMs, ingest,
    projection::ProjectedClaim,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimId, ComputeKey, DecodeError, DefRevId, DirectoryRecord,
    ExecutionScope, GrantId, GrantScope, NodeId, OpEnvelope, Principal, RecordId, ServeClaim,
    ServiceInstanceClaim, Shape, SignedOp, Slot, StreamId, encode_directory_record,
    encode_signed_op,
};

#[derive(Clone, Copy)]
enum ExpectedBoundary {
    GrantUnauthorized,
    ClaimBadSignature,
    ClaimUnauthorized,
    ProjectionFiltered,
}

struct RunResult {
    grant: ingest::IngestOutcome,
    claim: ingest::IngestOutcome,
    projected: Vec<ProjectedClaim>,
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("router"),
        local_principal: Principal::from("owner"),
        peers: BTreeSet::new(),
        workspace_owner_roots: BTreeMap::from([
            ("workspace".to_owned(), Principal::from("owner")),
            ("other-workspace".to_owned(), Principal::from("owner")),
        ]),
        node_principal_bindings: BTreeSet::from([
            NodePrincipalBinding {
                node: NodeId::from("workspace-node"),
                principal: Principal::from("workspace-principal"),
                plane: PrincipalPlane::Workspace,
            },
            NodePrincipalBinding {
                node: NodeId::from("service-node"),
                principal: Principal::from("service-principal"),
                plane: PrincipalPlane::Derived,
            },
        ]),
        skew_margin_ms: 5,
        max_lease_ms: 10_000,
        clock_resync_ms: 100,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(1_000).expect("non-zero"),
        gossip_fan: 2,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("non-zero"),
    }
}

fn stream(share: &str, key: u8) -> StreamId {
    StreamId {
        share: share.to_owned(),
        glade_id: "directory".to_owned(),
        key: vec![key],
    }
}

fn id(stream: &StreamId, origin: &str, seq: u64) -> RecordId {
    RecordId {
        stream: stream.clone(),
        origin: Principal::from(origin),
        seq,
    }
}

fn definition(seq: u64) -> DefRevId {
    DefRevId::from(id(&stream("definitions", 0xd0), "owner", seq))
}

fn signed(stream: StreamId, origin: &str, record: DirectoryRecord) -> SignedOp {
    encode_signed_op(
        &OpEnvelope {
            stream,
            origin: Principal::from(origin),
            seq: 0,
            prev: None,
            lamport: 1,
            refs: Vec::new(),
            shape: Shape::Log,
            payload: encode_directory_record(&record).expect("canonical directory record"),
        },
        &[0xa5],
    )
    .expect("canonical signed op")
}

fn execute_scope() -> GrantScope {
    GrantScope::Execution(ExecutionScope {
        def_ref: definition(0),
        compute_key: ComputeKey::from(vec![0xc0]),
    })
}

fn run(
    config: &KernelConfig,
    grant: SignedOp,
    grant_signer: &str,
    claim: SignedOp,
    claim_signer: &str,
) -> RunResult {
    let clock = ClockState::Ready {
        watermark: WallMs(1_000),
    };
    let grant = ingest::ingest(
        &grant,
        &VerificationResult::Valid {
            signer: Principal::from(grant_signer),
        },
        &Default::default(),
        config,
        clock,
    );
    let claim = ingest::ingest(
        &claim,
        &VerificationResult::Valid {
            signer: Principal::from(claim_signer),
        },
        &grant.persisted,
        config,
        clock,
    );
    let projected = glade_discover_core::projection::live_claims(&claim.persisted, config, clock);
    RunResult {
        grant,
        claim,
        projected,
    }
}

fn assert_mutation(name: &str, expected: ExpectedBoundary, result: &RunResult) {
    assert!(
        result.projected.is_empty(),
        "{name}: mutated authority projected a live claim"
    );
    match expected {
        ExpectedBoundary::GrantUnauthorized => {
            assert_eq!(
                result.grant.governance,
                Some(GovernanceVerdict::Unauthorized),
                "{name}"
            );
            assert_eq!(
                result.grant.disposition,
                IngestDisposition::Rejected,
                "{name}"
            );
        }
        ExpectedBoundary::ClaimBadSignature => {
            assert_eq!(
                result.claim.structural,
                StructuralVerdict::BadSignature,
                "{name}"
            );
            assert_eq!(
                result.claim.disposition,
                IngestDisposition::Quarantined,
                "{name}"
            );
        }
        ExpectedBoundary::ClaimUnauthorized => {
            assert_eq!(
                result.claim.governance,
                Some(GovernanceVerdict::Unauthorized),
                "{name}"
            );
            assert_eq!(
                result.claim.disposition,
                IngestDisposition::Rejected,
                "{name}"
            );
        }
        ExpectedBoundary::ProjectionFiltered => {
            assert_eq!(
                result.grant.disposition,
                IngestDisposition::Folded,
                "{name}"
            );
            assert_eq!(
                result.claim.disposition,
                IngestDisposition::Folded,
                "{name}"
            );
        }
    }
}

fn replace_binding(
    config: &mut KernelConfig,
    principal: &str,
    node: &str,
    replacement_principal: &str,
    replacement_node: &str,
    replacement_plane: PrincipalPlane,
) {
    let original = config
        .node_principal_bindings
        .iter()
        .find(|binding| {
            binding.principal == Principal::from(principal) && binding.node == NodeId::from(node)
        })
        .cloned()
        .expect("fixture binding");
    config.node_principal_bindings.remove(&original);
    config.node_principal_bindings.insert(NodePrincipalBinding {
        node: NodeId::from(replacement_node),
        principal: Principal::from(replacement_principal),
        plane: replacement_plane,
    });
}

#[derive(Clone)]
struct WorkspaceFixture {
    config: KernelConfig,
    grant: CapabilityGrant,
    claim: ServeClaim,
    grant_stream: StreamId,
    claim_stream: StreamId,
    claim_signer: &'static str,
}

impl WorkspaceFixture {
    fn valid() -> Self {
        let grant_stream = stream("workspace", 0x10);
        let claim_stream = stream("workspace", 0x20);
        Self {
            config: config(),
            grant: CapabilityGrant {
                grant_id: GrantId::from(id(&grant_stream, "owner", 0)),
                issuer: Principal::from("owner"),
                principal: Principal::from("workspace-principal"),
                share: "workspace".to_owned(),
                verbs: vec![CapabilityVerb::Serve],
                scope: None,
            },
            claim: ServeClaim {
                node: NodeId::from("workspace-node"),
                share: "workspace".to_owned(),
                claim_id: ClaimId::from(id(&claim_stream, "workspace-principal", 0)),
                grant_ref: GrantId::from(id(&grant_stream, "owner", 0)),
                lease_expiry_ms: 5_000,
                epoch: 1,
            },
            grant_stream,
            claim_stream,
            claim_signer: "workspace-principal",
        }
    }

    fn run(self) -> RunResult {
        run(
            &self.config,
            signed(
                self.grant_stream,
                "owner",
                DirectoryRecord::CapabilityGrant(self.grant),
            ),
            "owner",
            signed(
                self.claim_stream,
                "workspace-principal",
                DirectoryRecord::ServeClaim(self.claim),
            ),
            self.claim_signer,
        )
    }
}

type WorkspaceMutation = fn(&mut WorkspaceFixture);

fn workspace_bad_signature(fixture: &mut WorkspaceFixture) {
    fixture.claim_signer = "other-principal";
}

fn workspace_wrong_owner(fixture: &mut WorkspaceFixture) {
    fixture
        .config
        .workspace_owner_roots
        .insert("workspace".to_owned(), Principal::from("different-owner"));
}

fn workspace_wrong_issuer(fixture: &mut WorkspaceFixture) {
    fixture.grant.issuer = Principal::from("different-owner");
}

fn workspace_wrong_binding_node(fixture: &mut WorkspaceFixture) {
    replace_binding(
        &mut fixture.config,
        "workspace-principal",
        "workspace-node",
        "workspace-principal",
        "other-node",
        PrincipalPlane::Workspace,
    );
}

fn workspace_wrong_binding_principal(fixture: &mut WorkspaceFixture) {
    replace_binding(
        &mut fixture.config,
        "workspace-principal",
        "workspace-node",
        "other-principal",
        "workspace-node",
        PrincipalPlane::Workspace,
    );
}

fn workspace_wrong_binding_plane(fixture: &mut WorkspaceFixture) {
    replace_binding(
        &mut fixture.config,
        "workspace-principal",
        "workspace-node",
        "workspace-principal",
        "workspace-node",
        PrincipalPlane::Derived,
    );
}

fn workspace_wrong_subject(fixture: &mut WorkspaceFixture) {
    fixture.grant.principal = Principal::from("other-principal");
}

fn workspace_wrong_share(fixture: &mut WorkspaceFixture) {
    fixture.grant.share = "other-workspace".to_owned();
}

fn workspace_wrong_verb(fixture: &mut WorkspaceFixture) {
    fixture.grant.verbs = vec![CapabilityVerb::Execute];
}

fn workspace_wrong_scope(fixture: &mut WorkspaceFixture) {
    fixture.grant.scope = Some(execute_scope());
}

#[test]
fn workspace_authority_conjuncts_are_independently_necessary() {
    let happy = WorkspaceFixture::valid().run();
    assert_eq!(happy.projected.len(), 1, "valid workspace authority");

    let cases: [(&str, WorkspaceMutation, ExpectedBoundary); 10] = [
        (
            "signature must match operation origin",
            workspace_bad_signature,
            ExpectedBoundary::ClaimBadSignature,
        ),
        (
            "grant signer must be configured owner",
            workspace_wrong_owner,
            ExpectedBoundary::GrantUnauthorized,
        ),
        (
            "grant issuer must match signer",
            workspace_wrong_issuer,
            ExpectedBoundary::GrantUnauthorized,
        ),
        (
            "binding node must match",
            workspace_wrong_binding_node,
            ExpectedBoundary::ClaimUnauthorized,
        ),
        (
            "binding principal must match",
            workspace_wrong_binding_principal,
            ExpectedBoundary::ClaimUnauthorized,
        ),
        (
            "binding plane must be workspace",
            workspace_wrong_binding_plane,
            ExpectedBoundary::ClaimUnauthorized,
        ),
        (
            "grant subject must match claim origin",
            workspace_wrong_subject,
            ExpectedBoundary::ProjectionFiltered,
        ),
        (
            "grant share must match claim share",
            workspace_wrong_share,
            ExpectedBoundary::ProjectionFiltered,
        ),
        (
            "grant must contain serve verb",
            workspace_wrong_verb,
            ExpectedBoundary::ProjectionFiltered,
        ),
        (
            "workspace grant must be unscoped",
            workspace_wrong_scope,
            ExpectedBoundary::ProjectionFiltered,
        ),
    ];

    for (name, mutate, expected) in cases {
        let mut fixture = WorkspaceFixture::valid();
        mutate(&mut fixture);
        assert_mutation(name, expected, &fixture.run());
    }
}

#[derive(Clone)]
struct DerivedFixture {
    config: KernelConfig,
    grant: CapabilityGrant,
    claim: ServiceInstanceClaim,
    grant_stream: StreamId,
    claim_stream: StreamId,
    claim_signer: &'static str,
}

impl DerivedFixture {
    fn valid() -> Self {
        let grant_stream = stream("workspace", 0x11);
        let claim_stream = stream("svc", 0x21);
        Self {
            config: config(),
            grant: CapabilityGrant {
                grant_id: GrantId::from(id(&grant_stream, "owner", 0)),
                issuer: Principal::from("owner"),
                principal: Principal::from("service-principal"),
                share: "workspace".to_owned(),
                verbs: vec![CapabilityVerb::Execute],
                scope: Some(execute_scope()),
            },
            claim: ServiceInstanceClaim {
                node: NodeId::from("service-node"),
                share: "svc".to_owned(),
                glade_id: "directory".to_owned(),
                key: vec![0x21],
                claim_id: ClaimId::from(id(&claim_stream, "service-principal", 0)),
                def_ref: definition(0),
                exec_grant_ref: GrantId::from(id(&grant_stream, "owner", 0)),
                compute_key: ComputeKey::from(vec![0xc0]),
                lease_expiry_ms: 5_000,
                epoch: 1,
            },
            grant_stream,
            claim_stream,
            claim_signer: "service-principal",
        }
    }

    fn run(self) -> RunResult {
        run(
            &self.config,
            signed(
                self.grant_stream,
                "owner",
                DirectoryRecord::CapabilityGrant(self.grant),
            ),
            "owner",
            signed(
                self.claim_stream,
                "service-principal",
                DirectoryRecord::ServiceInstanceClaim(self.claim),
            ),
            self.claim_signer,
        )
    }
}

type DerivedMutation = fn(&mut DerivedFixture);

fn derived_bad_signature(fixture: &mut DerivedFixture) {
    fixture.claim_signer = "other-principal";
}

fn derived_wrong_owner(fixture: &mut DerivedFixture) {
    fixture
        .config
        .workspace_owner_roots
        .insert("workspace".to_owned(), Principal::from("different-owner"));
}

fn derived_wrong_issuer(fixture: &mut DerivedFixture) {
    fixture.grant.issuer = Principal::from("different-owner");
}

fn derived_wrong_binding_node(fixture: &mut DerivedFixture) {
    replace_binding(
        &mut fixture.config,
        "service-principal",
        "service-node",
        "service-principal",
        "other-node",
        PrincipalPlane::Derived,
    );
}

fn derived_wrong_binding_principal(fixture: &mut DerivedFixture) {
    replace_binding(
        &mut fixture.config,
        "service-principal",
        "service-node",
        "other-principal",
        "service-node",
        PrincipalPlane::Derived,
    );
}

fn derived_wrong_binding_plane(fixture: &mut DerivedFixture) {
    replace_binding(
        &mut fixture.config,
        "service-principal",
        "service-node",
        "service-principal",
        "service-node",
        PrincipalPlane::Workspace,
    );
}

fn derived_wrong_subject(fixture: &mut DerivedFixture) {
    fixture.grant.principal = Principal::from("other-principal");
}

fn derived_unknown_grant_share(fixture: &mut DerivedFixture) {
    fixture.grant.share = "unknown-workspace".to_owned();
}

fn derived_wrong_verb(fixture: &mut DerivedFixture) {
    fixture.grant.verbs = vec![CapabilityVerb::Serve];
}

fn derived_missing_scope(fixture: &mut DerivedFixture) {
    fixture.grant.scope = None;
}

fn derived_wrong_share(fixture: &mut DerivedFixture) {
    fixture.claim.share = "workspace".to_owned();
}

fn derived_wrong_definition(fixture: &mut DerivedFixture) {
    fixture.claim.def_ref = definition(1);
}

fn derived_wrong_compute_key(fixture: &mut DerivedFixture) {
    fixture.claim.compute_key = ComputeKey::from(vec![0xc1]);
}

#[test]
fn derived_authority_conjuncts_are_independently_necessary() {
    let happy = DerivedFixture::valid().run();
    assert_eq!(happy.projected.len(), 1, "valid derived authority");
    assert_eq!(
        happy.projected[0].slot,
        Slot::Binding {
            share: "svc".to_owned(),
            glade_id: "directory".to_owned(),
            key: vec![0x21],
        }
    );

    let mut invalid_share = DerivedFixture::valid();
    derived_wrong_share(&mut invalid_share);
    assert_eq!(
        encode_directory_record(&DirectoryRecord::ServiceInstanceClaim(invalid_share.claim)),
        Err(DecodeError::InvalidValue("service_instance.share")),
        "derived share is rejected before ingest"
    );

    let cases: [(&str, DerivedMutation, ExpectedBoundary); 12] = [
        (
            "signature must match operation origin",
            derived_bad_signature,
            ExpectedBoundary::ClaimBadSignature,
        ),
        (
            "execution grant signer must be configured owner",
            derived_wrong_owner,
            ExpectedBoundary::GrantUnauthorized,
        ),
        (
            "execution grant issuer must match signer",
            derived_wrong_issuer,
            ExpectedBoundary::GrantUnauthorized,
        ),
        (
            "binding node must match",
            derived_wrong_binding_node,
            ExpectedBoundary::ClaimUnauthorized,
        ),
        (
            "binding principal must match",
            derived_wrong_binding_principal,
            ExpectedBoundary::ClaimUnauthorized,
        ),
        (
            "binding plane must be derived",
            derived_wrong_binding_plane,
            ExpectedBoundary::ClaimUnauthorized,
        ),
        (
            "execution grant subject must match claim origin",
            derived_wrong_subject,
            ExpectedBoundary::ProjectionFiltered,
        ),
        (
            "execution grant share must be owner-rooted",
            derived_unknown_grant_share,
            ExpectedBoundary::GrantUnauthorized,
        ),
        (
            "execution grant must contain execute verb",
            derived_wrong_verb,
            ExpectedBoundary::ProjectionFiltered,
        ),
        (
            "execution grant must carry exact scope",
            derived_missing_scope,
            ExpectedBoundary::ProjectionFiltered,
        ),
        (
            "scope definition must exactly match claim",
            derived_wrong_definition,
            ExpectedBoundary::ProjectionFiltered,
        ),
        (
            "scope compute key must exactly match claim",
            derived_wrong_compute_key,
            ExpectedBoundary::ProjectionFiltered,
        ),
    ];

    for (name, mutate, expected) in cases {
        let mut fixture = DerivedFixture::valid();
        mutate(&mut fixture);
        assert_mutation(name, expected, &fixture.run());
    }
}
