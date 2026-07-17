use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use glade_discover_protocol::{
    DirectoryRecord, decode_directory_record, decode_signed_op, op_hash, record_id,
};

const SCENARIOS: [&str; 5] = [
    "authz/owner-proof.json",
    "authz/unauth-revoke.json",
    "authz/regrant.json",
    "ingest/proof-late.json",
    "ingest/revoke-then-grant.json",
];

#[test]
fn authority_and_ingest_scenarios_use_canonical_identity_consistent_records() {
    for name in SCENARIOS {
        let path = scenario_root().join(name);
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        glade_discover_sim::Scenario::from_json(&source)
            .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()));

        let json: serde_json::Value = serde_json::from_str(&source).expect("valid JSON");
        let mut heads: BTreeMap<(String, String, u64), [u8; 32]> = BTreeMap::new();
        for input in json["inputs"].as_array().expect("inputs array") {
            let Some(hex) = input
                .pointer("/event/msg/op/canonical_hex")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let canonical = decode_hex(hex);
            let op = decode_signed_op(&canonical).expect("strict signed-op decode");
            assert_eq!(op.canonical_bytes(), canonical, "{} is canonical", name);
            let envelope = op.envelope();
            let record = decode_directory_record(&envelope.payload)
                .unwrap_or_else(|error| panic!("decode payload in {name}: {error:?}"));
            let id = record_id(&op);
            match record {
                DirectoryRecord::CapabilityGrant(grant) => {
                    assert_eq!(grant.grant_id.record(), &id, "grant identity in {name}");
                }
                DirectoryRecord::ServeClaim(claim) => {
                    assert_eq!(claim.claim_id.record(), &id, "claim identity in {name}");
                }
                DirectoryRecord::ServiceInstanceClaim(_)
                | DirectoryRecord::CapabilityRevocation(_) => {}
            }

            let chain = (
                format!(
                    "{}\0{}\0{:x?}",
                    envelope.stream.share, envelope.stream.glade_id, envelope.stream.key
                ),
                envelope.origin.to_string(),
                envelope.seq,
            );
            if envelope.seq == 0 {
                assert_eq!(envelope.prev, None, "seq zero has no prev in {name}");
            } else {
                let prior = heads
                    .get(&(chain.0.clone(), chain.1.clone(), envelope.seq - 1))
                    .unwrap_or_else(|| panic!("missing predecessor in {name}"));
                assert_eq!(envelope.prev.as_ref(), Some(prior), "prev hash in {name}");
            }
            heads.insert(chain, op_hash(&op));
        }
    }
}

#[test]
fn authority_and_ingest_scenarios_satisfy_the_strict_oracle() {
    for name in SCENARIOS {
        let path = scenario_root().join(name);
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let scenario = glade_discover_sim::Scenario::from_json(&source)
            .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()));
        glade_discover_sim::run_scenario(&scenario)
            .unwrap_or_else(|error| panic!("run {name}: {error:?}"));
    }
}

fn scenario_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0, "hex length");
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                .expect("valid hex")
        })
        .collect()
}
