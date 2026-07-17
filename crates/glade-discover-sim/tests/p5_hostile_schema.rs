use std::panic::{AssertUnwindSafe, catch_unwind};

use glade_discover_sim::Scenario;

const VALID: &str = r#"{
  "seed": 1,
  "event_budget": 10,
  "nodes": [{
    "id": "node-a",
    "principal": "principal-a",
    "owns": [],
    "seed_grants": [],
    "clock": {"initial_wall_ms": 1000, "initial_mono_ms": 0, "changes": []},
    "restarts": []
  }],
  "links": [],
  "inputs": [{
    "input_id": "route-a",
    "at_ms": 0,
    "node": "node-a",
    "event": {
      "kind": "route",
      "ingress": "ingress-a",
      "principal": "reader-a",
      "authenticated_context_hex": "",
      "corr": "corr-a",
      "query": {"kind": "workspace", "share": "workspace-a"}
    }
  }],
  "verifier_fixtures": [],
  "stop": {"kind": "at_ms", "at_ms": 10},
  "expect": [{
    "kind": "route",
    "when": {"kind": "at_ms", "at_ms": 0},
    "node": "node-a",
    "ingress": "ingress-a",
    "corr": "corr-a",
    "ans": {"kind": "no_claim"}
  }]
}"#;

#[test]
fn bounded_deterministic_hostile_json_never_panics() {
    let mut random = SplitMix64::new(0x5035_5C43_3A41_0001);
    for case in 0..1_024 {
        let len = (random.next() as usize) % 4_097;
        let mut bytes = vec![0_u8; len];
        for byte in &mut bytes {
            *byte = random.next() as u8;
        }
        let hostile = String::from_utf8_lossy(&bytes);
        let result = catch_unwind(AssertUnwindSafe(|| Scenario::from_json(&hostile)));
        assert!(result.is_ok(), "scenario decoder panicked for case {case}");
    }
}

#[test]
fn strict_schema_rejects_unknown_wrong_type_and_extreme_invalid_values() {
    let hostile = [
        VALID.replace("\"seed\": 1,", "\"seed\": 1, \"unknown\": null,"),
        VALID.replace("\"seed\": 1", "\"seed\": \"one\""),
        VALID.replace("\"event_budget\": 10", "\"event_budget\": 0"),
        VALID.replace("\"event_budget\": 10", "\"event_budget\": -1"),
        VALID.replace(
            "\"event_budget\": 10",
            "\"event_budget\": 18446744073709551616",
        ),
        VALID.replace("\"nodes\": [{", "\"nodes\": [{\"surprise\":true,"),
        VALID.replace("\"nodes\": [{", "\"nodes\": \"node-a\", \"discard\": [{"),
        VALID.replace(
            "\"initial_wall_ms\": 1000",
            "\"initial_wall_ms\": 9223372036854775808",
        ),
        VALID.replace("\"at_ms\": 0", "\"at_ms\": -1"),
        VALID.replace("\"kind\": \"route\"", "\"kind\": \"remote_code\""),
        VALID.replace("\"kind\": \"workspace\"", "\"kind\": \"binding\""),
        VALID.replace(
            "\"input_id\": \"route-a\",\n    \"at_ms\": 0",
            "\"input_id\": \"route-a\",\n    \"at_ms\": 11",
        ),
        VALID.replace("\"expect\": [{", "\"expect\": [{\"unexpected\":false,"),
        VALID.replace("\"expect\": [{", "\"expect\": [] , \"discard_expect\": [{"),
    ];

    for (case, hostile) in hostile.iter().enumerate() {
        let result = catch_unwind(AssertUnwindSafe(|| Scenario::from_json(hostile)));
        assert!(result.is_ok(), "mutation {case} panicked");
        assert!(
            result.expect("already checked").is_err(),
            "mutation {case} must fail closed"
        );
    }
}

#[test]
fn single_character_mutations_are_either_rejected_or_strictly_decodable() {
    for (index, byte) in VALID.bytes().enumerate().step_by(7) {
        if !byte.is_ascii() {
            continue;
        }
        let mut mutated = VALID.as_bytes().to_vec();
        mutated[index] = byte ^ 0x01;
        let Ok(text) = std::str::from_utf8(&mutated) else {
            continue;
        };
        let result = catch_unwind(AssertUnwindSafe(|| Scenario::from_json(text)));
        assert!(result.is_ok(), "single-byte mutation {index} panicked");
    }
}

struct SplitMix64(u64);

impl SplitMix64 {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
}
