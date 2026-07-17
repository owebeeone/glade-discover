use std::fs;
use std::path::{Path, PathBuf};

use glade_discover_protocol::{decode_directory_record, decode_signed_op};
use glade_discover_sim::{Scenario, run_scenario};
use serde_json::Value;

const ASSIGNED: &[&str] = &[
    "routing/noclaim-handoff.json",
    "routing/hostile-duplicate-route.json",
    "routing/hostile-route-cache-collision.json",
    "bounds/dos-bound.json",
];

fn scenario_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
}

fn read(relative: &str) -> String {
    fs::read_to_string(scenario_root().join(relative)).expect("read assigned scenario")
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0, "canonical hex must have byte pairs");
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).expect("ASCII hex");
            u8::from_str_radix(pair, 16).expect("valid canonical hex")
        })
        .collect()
}

fn canonical_ops(value: &Value) -> impl Iterator<Item = &[u8]> {
    value["inputs"]
        .as_array()
        .expect("inputs array")
        .iter()
        .filter_map(|input| input["event"]["msg"]["op"]["canonical_hex"].as_str())
        .map(str::as_bytes)
}

#[test]
fn assigned_scenarios_strictly_decode() {
    for relative in ASSIGNED {
        Scenario::from_json(&read(relative)).unwrap_or_else(|error| panic!("{relative}: {error}"));
    }
}

#[test]
fn embedded_ops_are_exact_canonical_typed_records() {
    for relative in ASSIGNED {
        let value: Value = serde_json::from_str(&read(relative)).expect("scenario JSON");
        for encoded_hex in canonical_ops(&value) {
            let encoded_hex = std::str::from_utf8(encoded_hex).expect("ASCII hex");
            let bytes = decode_hex(encoded_hex);
            let op = decode_signed_op(&bytes)
                .unwrap_or_else(|error| panic!("{relative}: invalid signed op: {error:?}"));
            assert_eq!(op.canonical_bytes(), bytes, "{relative}: exact op bytes");
            decode_directory_record(&op.envelope().payload)
                .unwrap_or_else(|error| panic!("{relative}: invalid directory record: {error:?}"));
        }
    }
}

#[test]
fn noclaim_handoff_exposes_only_the_internal_terminal_answer() {
    let value: Value =
        serde_json::from_str(&read("routing/noclaim-handoff.json")).expect("noclaim scenario JSON");
    let route_answers = value["expect"]
        .as_array()
        .expect("expectations")
        .iter()
        .filter(|expectation| expectation["kind"] == "route")
        .map(|expectation| &expectation["ans"])
        .collect::<Vec<_>>();

    assert_eq!(route_answers.len(), 2);
    for answer in route_answers {
        assert_eq!(answer, &serde_json::json!({"kind": "no_claim"}));
    }
}

#[test]
fn dos_bound_hits_the_declared_ceiling_with_two_small_real_ops() {
    let value: Value =
        serde_json::from_str(&read("bounds/dos-bound.json")).expect("bounds scenario JSON");
    let ceiling = value["nodes"][0]["max_retained_bytes"]
        .as_u64()
        .expect("explicit retained-byte ceiling");
    let lengths = canonical_ops(&value)
        .map(|hex| decode_hex(std::str::from_utf8(hex).expect("ASCII hex")).len() as u64)
        .collect::<Vec<_>>();

    assert_eq!(lengths.len(), 2);
    assert_eq!(lengths[0], ceiling, "first op exactly fills the ceiling");
    assert!(
        lengths[0].checked_add(lengths[1]).expect("small fixture") > ceiling,
        "second op must exceed the retained-byte ceiling"
    );
}

#[test]
fn routing_handoff_and_hostile_variants_are_green() {
    for relative in [
        "routing/noclaim-handoff.json",
        "routing/hostile-duplicate-route.json",
        "routing/hostile-route-cache-collision.json",
    ] {
        let scenario = Scenario::from_json(&read(relative)).expect("strict scenario");
        run_scenario(&scenario).unwrap_or_else(|error| panic!("{relative}: {error:?}"));
    }
}

#[test]
fn dos_bound_is_green_end_to_end() {
    let scenario = Scenario::from_json(&read("bounds/dos-bound.json")).expect("strict scenario");
    assert!(run_scenario(&scenario).is_ok());
}
