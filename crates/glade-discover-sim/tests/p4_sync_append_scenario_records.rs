use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimId, DirectoryRecord, GrantId, NodeId, OpEnvelope,
    Principal, RecordId, ServeClaim, Shape, SignedOp, StreamId, decode_directory_record,
    decode_signed_op, encode_directory_record, encode_signed_op, record_id,
};
use serde_json::Value;

fn stream(share: &str, key: u8) -> StreamId {
    StreamId {
        share: share.into(),
        glade_id: "directory".into(),
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
    let payload = encode_directory_record(&record).expect("canonical directory record");
    encode_signed_op(
        &OpEnvelope {
            stream,
            origin: Principal::from(origin),
            seq: 0,
            prev: None,
            lamport: 1,
            refs: Vec::new(),
            shape: Shape::Log,
            payload,
        },
        &[0xa5, 0x5a],
    )
    .expect("canonical signed op")
}

fn sync_op() -> SignedOp {
    let op_stream = stream("workspace-sync", 0x71);
    let op_id = id(&op_stream, "owner-sync");
    signed(
        op_stream,
        "owner-sync",
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: GrantId::from(op_id),
            issuer: Principal::from("owner-sync"),
            principal: Principal::from("owner-sync"),
            share: "workspace-sync".into(),
            verbs: vec![CapabilityVerb::Serve],
            scope: None,
        }),
    )
}

fn append_ops(grant_key: u8) -> (SignedOp, SignedOp) {
    let grant_stream = stream("workspace-a", grant_key);
    let claim_stream = stream("workspace-a", 0x01);
    let grant_record_id = id(&grant_stream, "owner-a");
    let claim_record_id = id(&claim_stream, "owner-a");
    let grant = signed(
        grant_stream,
        "owner-a",
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: GrantId::from(grant_record_id.clone()),
            issuer: Principal::from("owner-a"),
            principal: Principal::from("owner-a"),
            share: "workspace-a".into(),
            verbs: vec![CapabilityVerb::Serve],
            scope: None,
        }),
    );
    let claim = signed(
        claim_stream,
        "owner-a",
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("node-a"),
            share: "workspace-a".into(),
            claim_id: ClaimId::from(claim_record_id),
            grant_ref: GrantId::from(grant_record_id),
            lease_expiry_ms: 500_000,
            epoch: 0,
        }),
    );
    (grant, claim)
}

fn scenario_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
}

fn collect_hex(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            if let Some(hex) = fields.get("canonical_hex").and_then(Value::as_str) {
                output.push(hex.to_owned());
            }
            for child in fields.values() {
                collect_hex(child, output);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_hex(child, output);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn decode_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("ASCII hex");
            u8::from_str_radix(pair, 16).expect("valid hex")
        })
        .collect()
}

#[test]
fn sync_and_append_scenarios_embed_only_canonical_self_consistent_ops() {
    let sync = sync_op();
    let (delayed_grant, delayed_claim) = append_ops(0xa1);
    let (restart_grant, restart_claim) = append_ops(0xa2);
    let cases = [
        ("sync/sync-round.json", vec![(sync, 3_usize)]),
        (
            "ingest/delayed-sign.json",
            vec![(delayed_grant, 1), (delayed_claim, 3)],
        ),
        (
            "ingest/append-restart.json",
            vec![(restart_grant, 1), (restart_claim, 3)],
        ),
    ];

    let mut mismatches = Vec::new();
    for (file, expected) in cases {
        let scenario: Value = serde_json::from_str(
            &fs::read_to_string(scenario_root().join(file)).expect("scenario data"),
        )
        .expect("scenario JSON");
        let mut actual = Vec::new();
        collect_hex(&scenario, &mut actual);
        let expected = expected
            .into_iter()
            .map(|(op, count)| (encode_hex(op.canonical_bytes()), (op, count)))
            .collect::<BTreeMap<_, _>>();
        let actual_counts = actual.into_iter().fold(BTreeMap::new(), |mut counts, hex| {
            *counts.entry(hex).or_insert(0_usize) += 1;
            counts
        });
        let expected_counts = expected
            .iter()
            .map(|(hex, (_, count))| (hex.clone(), *count))
            .collect::<BTreeMap<_, _>>();
        if actual_counts != expected_counts {
            mismatches.push(format!(
                "{file}\tactual={actual_counts:?}\texpected={expected_counts:?}"
            ));
            continue;
        }

        for (hex, (expected_op, _)) in expected {
            let decoded = decode_signed_op(&decode_hex(&hex)).expect("canonical signed op");
            assert_eq!(decoded, expected_op);
            let record = decode_directory_record(&decoded.envelope().payload)
                .expect("typed directory record");
            match record {
                DirectoryRecord::CapabilityGrant(grant) => {
                    assert_eq!(grant.grant_id.record(), &record_id(&decoded));
                    assert_eq!(grant.issuer, decoded.envelope().origin);
                }
                DirectoryRecord::ServeClaim(claim) => {
                    assert_eq!(claim.claim_id.record(), &record_id(&decoded));
                    assert_eq!(claim.grant_ref.record().stream.share, claim.share);
                }
                DirectoryRecord::ServiceInstanceClaim(_)
                | DirectoryRecord::CapabilityRevocation(_) => {
                    panic!("unexpected scenario record kind")
                }
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "canonical fixture replacements required:\n{}",
        mismatches.join("\n")
    );
}
