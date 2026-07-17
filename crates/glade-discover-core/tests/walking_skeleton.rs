use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    Effect, Event, KernelConfig, MonoInstant, NodePrincipalBinding, PersistedState, PrincipalCtx,
    PrincipalPlane, RouteAns, StepCtx, VerificationBatch, VerificationResult, WallMs,
    WatermarkLoad, restore_fresh, step,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimId, Corr, DirectoryRecord, GrantId, IngressId, NodeId,
    OpEnvelope, Principal, RecordId, RouteQuery, ServeClaim, Shape, Slot, StreamId, WireMsg,
    encode_directory_record, encode_signed_op,
};

fn stream() -> StreamId {
    StreamId {
        share: "workspace-a".into(),
        glade_id: "directory".into(),
        key: vec![1],
    }
}

fn record_id(origin: &str) -> RecordId {
    RecordId {
        stream: stream(),
        origin: Principal::from(origin),
        seq: 0,
    }
}

fn signed_record(origin: &str, record: DirectoryRecord) -> glade_discover_protocol::SignedOp {
    let payload = encode_directory_record(&record).expect("canonical directory record");
    encode_signed_op(
        &OpEnvelope {
            stream: stream(),
            origin: Principal::from(origin),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: Vec::new(),
            shape: Shape::Value,
            payload,
        },
        &[0xAA],
    )
    .expect("canonical signed op")
}

fn grant() -> glade_discover_protocol::SignedOp {
    signed_record(
        "owner-a",
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: GrantId::from(record_id("owner-a")),
            issuer: Principal::from("owner-a"),
            principal: Principal::from("node-principal-b"),
            share: "workspace-a".into(),
            verbs: vec![CapabilityVerb::Serve],
            scope: None,
        }),
    )
}

fn claim(origin: &str, lease_expiry_ms: i64) -> glade_discover_protocol::SignedOp {
    signed_record(
        origin,
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("node-b"),
            share: "workspace-a".into(),
            claim_id: ClaimId::from(record_id(origin)),
            grant_ref: GrantId::from(record_id("owner-a")),
            lease_expiry_ms,
            epoch: 0,
        }),
    )
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("router"),
        local_principal: Principal::from("owner-a"),
        peers: BTreeSet::new(),
        workspace_owner_roots: BTreeMap::from([("workspace-a".into(), Principal::from("owner-a"))]),
        node_principal_bindings: BTreeSet::from([NodePrincipalBinding {
            node: NodeId::from("node-b"),
            principal: Principal::from("node-principal-b"),
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

fn deliver(
    state: glade_discover_core::State,
    at: u64,
    op: glade_discover_protocol::SignedOp,
    signer: &str,
) -> glade_discover_core::State {
    step(
        state,
        StepCtx {
            mono: MonoInstant(at),
            wall: WallMs(100_000 + i64::try_from(at).expect("small fixture")),
        },
        Event::Deliver {
            from: NodeId::from("node-b"),
            msg: Box::new(WireMsg::DirOp { op: Box::new(op) }),
            verification: VerificationBatch(vec![VerificationResult::Valid {
                signer: Principal::from(signer),
            }]),
        },
    )
    .0
}

fn route(state: glade_discover_core::State) -> (glade_discover_core::State, Vec<Effect>) {
    step(
        state,
        StepCtx {
            mono: MonoInstant(2),
            wall: WallMs(100_002),
        },
        Event::Route {
            ingress: IngressId::from("ingress-a"),
            principal: PrincipalCtx {
                principal: Principal::from("reader-a"),
                authenticated_context: Vec::new(),
            },
            corr: Corr::from("corr-a"),
            query: RouteQuery {
                slot: Slot::Workspace {
                    share: "workspace-a".into(),
                },
            },
        },
    )
}

fn initial() -> glade_discover_core::State {
    restore_fresh(
        config(),
        PersistedState::default(),
        WatermarkLoad::Readable(WallMs(100_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(100_000),
        },
    )
}

#[test]
fn valid_grant_and_claim_route_to_the_serving_node() {
    let state = deliver(initial(), 0, grant(), "owner-a");
    let state = deliver(
        state,
        1,
        claim("node-principal-b", 200_000),
        "node-principal-b",
    );
    let (state, effects) = route(state);

    assert_eq!(state.persisted().retained.len(), 1);
    assert_eq!(state.persisted().retained[&stream()].len(), 2);
    assert_eq!(
        effects,
        vec![Effect::Reply {
            ingress: IngressId::from("ingress-a"),
            corr: Corr::from("corr-a"),
            ans: RouteAns::Matched {
                node: NodeId::from("node-b"),
            },
        }]
    );
}

#[test]
fn forged_claim_cannot_produce_a_match() {
    let state = deliver(initial(), 0, grant(), "owner-a");
    let state = deliver(state, 1, claim("attacker", 200_000), "attacker");
    let (state, effects) = route(state);

    assert_eq!(state.persisted().retained[&stream()].len(), 1);
    assert_eq!(
        effects,
        vec![Effect::Reply {
            ingress: IngressId::from("ingress-a"),
            corr: Corr::from("corr-a"),
            ans: RouteAns::NoClaim,
        }]
    );
}

#[test]
fn claim_inside_the_skew_expiry_floor_is_not_routable() {
    let state = deliver(initial(), 0, grant(), "owner-a");
    let state = deliver(
        state,
        1,
        claim("node-principal-b", 105_001),
        "node-principal-b",
    );
    let (_, effects) = route(state);

    assert_eq!(
        effects,
        vec![Effect::Reply {
            ingress: IngressId::from("ingress-a"),
            corr: Corr::from("corr-a"),
            ans: RouteAns::NoClaim,
        }]
    );
}
