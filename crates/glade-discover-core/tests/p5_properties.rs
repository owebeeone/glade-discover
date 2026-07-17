use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    ClockState, IngestDisposition, KernelConfig, NodePrincipalBinding, PersistedState,
    PrincipalPlane, RouteAns, VerificationResult, WallMs, ingest, projection::ProjectedClaim,
    routing,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimId, DirectoryRecord, GrantId, NodeId, OpEnvelope,
    Principal, RecordId, RouteQuery, ServeClaim, Shape, SignedOp, Slot, StreamId,
    encode_directory_record, encode_signed_op, record_id,
};

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("router"),
        local_principal: Principal::from("owner"),
        peers: BTreeSet::new(),
        workspace_owner_roots: BTreeMap::from([("workspace".to_owned(), Principal::from("owner"))]),
        node_principal_bindings: BTreeSet::from([
            NodePrincipalBinding {
                node: NodeId::from("node-a"),
                principal: Principal::from("principal-a"),
                plane: PrincipalPlane::Workspace,
            },
            NodePrincipalBinding {
                node: NodeId::from("node-b"),
                principal: Principal::from("principal-b"),
                plane: PrincipalPlane::Workspace,
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

fn stream(key: u8) -> StreamId {
    StreamId {
        share: "workspace".to_owned(),
        glade_id: "directory".to_owned(),
        key: vec![key],
    }
}

fn id(stream: &StreamId, origin: &str) -> RecordId {
    RecordId {
        stream: stream.clone(),
        origin: Principal::from(origin),
        seq: 0,
    }
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
        &[0x50, 0x05],
    )
    .expect("canonical signed op")
}

fn records() -> Vec<SignedOp> {
    let grant = |key, principal: &str| {
        let grant_stream = stream(key);
        let grant_id = GrantId::from(id(&grant_stream, "owner"));
        let op = signed(
            grant_stream,
            "owner",
            DirectoryRecord::CapabilityGrant(CapabilityGrant {
                grant_id: grant_id.clone(),
                issuer: Principal::from("owner"),
                principal: Principal::from(principal),
                share: "workspace".to_owned(),
                verbs: vec![CapabilityVerb::Serve],
                scope: None,
            }),
        );
        (grant_id, op)
    };
    let (grant_a_id, grant_a) = grant(0x10, "principal-a");
    let (grant_b_id, grant_b) = grant(0x11, "principal-b");

    let claim = |key, origin: &str, node: &str, grant_ref| {
        let claim_stream = stream(key);
        signed(
            claim_stream.clone(),
            origin,
            DirectoryRecord::ServeClaim(ServeClaim {
                node: NodeId::from(node),
                share: "workspace".to_owned(),
                claim_id: ClaimId::from(id(&claim_stream, origin)),
                grant_ref,
                lease_expiry_ms: 5_000,
                epoch: 7,
            }),
        )
    };

    vec![
        grant_a,
        grant_b,
        claim(0x20, "principal-a", "node-a", grant_a_id),
        claim(0x21, "principal-b", "node-b", grant_b_id),
    ]
}

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    fn visit<T: Clone>(remaining: &mut Vec<T>, prefix: &mut Vec<T>, output: &mut Vec<Vec<T>>) {
        if remaining.is_empty() {
            output.push(prefix.clone());
            return;
        }
        for index in 0..remaining.len() {
            let item = remaining.remove(index);
            prefix.push(item.clone());
            visit(remaining, prefix, output);
            prefix.pop();
            remaining.insert(index, item);
        }
    }

    if items.is_empty() {
        return vec![Vec::new()];
    }
    let mut remaining = items.to_vec();
    let mut output = Vec::new();
    visit(&mut remaining, &mut Vec::new(), &mut output);
    output
}

fn ingest_order(records: &[SignedOp]) -> PersistedState {
    let mut persisted = PersistedState::default();
    for op in records {
        let outcome = ingest::ingest(
            op,
            &VerificationResult::Valid {
                signer: op.envelope().origin.clone(),
            },
            &persisted,
            &config(),
            ClockState::Ready {
                watermark: WallMs(1_000),
            },
        );
        assert_eq!(outcome.disposition, IngestDisposition::Folded);
        persisted = outcome.persisted;
    }
    persisted
}

fn retained_ids(persisted: &PersistedState) -> BTreeSet<RecordId> {
    persisted
        .retained
        .values()
        .flat_map(|records| records.keys().cloned())
        .collect()
}

#[test]
fn retained_records_converge_to_set_union_for_every_small_subset_and_order() {
    let universe = records();

    for mask in 0_u8..(1 << universe.len()) {
        let subset = universe
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, op)| op.clone())
            .collect::<Vec<_>>();
        let expected_ids = subset.iter().map(record_id).collect::<BTreeSet<_>>();
        let mut canonical = None;

        for order in permutations(&subset) {
            let persisted = ingest_order(&order);
            assert_eq!(retained_ids(&persisted), expected_ids, "mask {mask:03b}");
            if let Some(expected) = &canonical {
                assert_eq!(&persisted, expected, "mask {mask:03b}, order differs");
            } else {
                canonical = Some(persisted.clone());
            }

            let mut replayed = persisted.clone();
            for op in order.iter().rev() {
                let duplicate = ingest::ingest(
                    op,
                    &VerificationResult::Valid {
                        signer: op.envelope().origin.clone(),
                    },
                    &replayed,
                    &config(),
                    ClockState::Ready {
                        watermark: WallMs(1_000),
                    },
                );
                assert_eq!(duplicate.disposition, IngestDisposition::Duplicate);
                replayed = duplicate.persisted;
            }
            assert_eq!(replayed, persisted, "duplicate replay changes the union");
        }
    }
}

fn projected(origin: &str, node: &str, epoch: u64) -> ProjectedClaim {
    let claim_stream = stream(0x30);
    ProjectedClaim {
        slot: Slot::Workspace {
            share: "workspace".to_owned(),
        },
        node: NodeId::from(node),
        claim_id: ClaimId::from(id(&claim_stream, origin)),
        epoch,
    }
}

#[test]
fn every_candidate_permutation_agrees_on_the_total_order_winner() {
    let candidates = [
        projected("z-low-epoch", "low-epoch", 6),
        projected("a-high-epoch", "tie-low-id", 7),
        projected("z-high-epoch", "tie-high-id", 7),
    ];
    let query = RouteQuery {
        slot: Slot::Workspace {
            share: "workspace".to_owned(),
        },
    };

    for mask in 0_u8..(1 << candidates.len()) {
        let subset = candidates
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, claim)| claim.clone())
            .collect::<Vec<_>>();
        let expected = if mask & 0b100 != 0 {
            RouteAns::Matched {
                node: NodeId::from("tie-high-id"),
            }
        } else if mask & 0b010 != 0 {
            RouteAns::Matched {
                node: NodeId::from("tie-low-id"),
            }
        } else if mask & 0b001 != 0 {
            RouteAns::Matched {
                node: NodeId::from("low-epoch"),
            }
        } else {
            RouteAns::NoClaim
        };

        for order in permutations(&subset) {
            assert_eq!(
                routing::resolve(&query, &order),
                expected,
                "mask {mask:03b} depends on candidate order"
            );
        }
    }
}
