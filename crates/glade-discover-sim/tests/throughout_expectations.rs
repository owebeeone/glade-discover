use std::path::Path;

use glade_discover_sim::{RunnerError, Scenario, run_scenario};
use serde_json::{Value, json};

fn scenario_value(relative: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scenarios")
        .join(relative);
    serde_json::from_str(&std::fs::read_to_string(path).expect("scenario JSON"))
        .expect("scenario value")
}

fn scenario(value: &Value) -> Scenario {
    Scenario::from_json(&serde_json::to_string(value).expect("scenario JSON"))
        .expect("strict scenario")
}

fn minimal(inputs: Value, expect: Value) -> Value {
    json!({
        "seed": 1,
        "event_budget": 20,
        "nodes": [
            {
                "id": "node-a",
                "principal": "principal-a",
                "owns": [],
                "seed_grants": [],
                "clock": {
                    "initial_wall_ms": 1000,
                    "initial_mono_ms": 0,
                    "changes": []
                },
                "restarts": []
            },
            {
                "id": "node-b",
                "principal": "principal-b",
                "owns": [],
                "seed_grants": [],
                "clock": {
                    "initial_wall_ms": 1000,
                    "initial_mono_ms": 0,
                    "changes": []
                },
                "restarts": []
            }
        ],
        "links": [],
        "inputs": inputs,
        "verifier_fixtures": [],
        "stop": {"kind": "at_ms", "at_ms": 2},
        "expect": expect
    })
}

fn route(input_id: &str, at_ms: u64, node: &str, ingress: &str, corr: &str) -> Value {
    json!({
        "input_id": input_id,
        "at_ms": at_ms,
        "node": node,
        "event": {
            "kind": "route",
            "ingress": ingress,
            "principal": "reader",
            "authenticated_context_hex": "",
            "corr": corr,
            "query": {"kind": "workspace", "share": "workspace-a"}
        }
    })
}

fn same_record_ids_with_different_signed_values(when: Value) -> Value {
    let seed =
        scenario_value("ingest/delayed-sign.json")["inputs"][0]["event"]["msg"]["op"].clone();
    let mut different = seed.clone();
    let mut canonical = different["canonical_hex"]
        .as_str()
        .expect("canonical hex")
        .to_owned();
    assert!(canonical.ends_with("5a"), "fixture signature suffix");
    let suffix = canonical.len() - 2;
    canonical.replace_range(suffix.., "5b");
    different["canonical_hex"] = json!(canonical);

    let mut value = minimal(
        json!([route("observe", 0, "node-a", "ingress", "corr")]),
        json!([{
            "kind": "converged",
            "when": when,
            "nodes": ["node-a", "node-b"]
        }]),
    );
    for node in value["nodes"].as_array_mut().expect("nodes") {
        node["principal"] = json!("owner-a");
        node["owns"] = json!(["workspace-a"]);
    }
    value["nodes"][0]["seed_grants"] = json!([seed]);
    value["nodes"][1]["seed_grants"] = json!([different]);
    value
}

#[test]
fn at_ms_converged_compares_only_retained_record_id_sets() {
    let value = same_record_ids_with_different_signed_values(json!({
        "kind": "at_ms",
        "at_ms": 0
    }));

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn throughout_converged_compares_only_retained_record_id_sets() {
    let value = same_record_ids_with_different_signed_values(json!({
        "kind": "throughout",
        "start_ms": 0,
        "end_ms": 1
    }));

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn throughout_converged_is_checked_after_every_event_in_the_half_open_interval() {
    let value = minimal(
        json!([
            route("route-a", 0, "node-a", "ingress-a", "corr-a"),
            route("route-b", 1, "node-b", "ingress-b", "corr-b")
        ]),
        json!([{
            "kind": "converged",
            "when": {"kind": "throughout", "start_ms": 0, "end_ms": 2},
            "nodes": ["node-a", "node-b"]
        }]),
    );

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn throughout_route_requires_one_matching_reply_from_each_event() {
    let value = minimal(
        json!([
            route("route-0", 0, "node-a", "ingress", "corr"),
            route("route-1", 1, "node-a", "ingress", "corr")
        ]),
        json!([{
            "kind": "route",
            "when": {"kind": "throughout", "start_ms": 0, "end_ms": 2},
            "node": "node-a",
            "ingress": "ingress",
            "corr": "corr",
            "ans": {"kind": "no_claim"}
        }]),
    );

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn throughout_route_rejects_an_event_without_the_matching_reply() {
    let value = minimal(
        json!([
            route("matching", 0, "node-a", "ingress", "corr"),
            route("different", 1, "node-a", "other-ingress", "other-corr")
        ]),
        json!([{
            "kind": "route",
            "when": {"kind": "throughout", "start_ms": 0, "end_ms": 2},
            "node": "node-a",
            "ingress": "ingress",
            "corr": "corr",
            "ans": {"kind": "no_claim"}
        }]),
    );

    assert_eq!(
        run_scenario(&scenario(&value)),
        Err(RunnerError::OracleMismatch)
    );
}

#[test]
fn throughout_clock_and_storage_are_checked_after_each_event_and_exclude_end_ms() {
    let inputs = json!([
        route("same-time-a", 0, "node-a", "ingress-a", "corr-a"),
        route("same-time-b", 0, "node-a", "ingress-b", "corr-b"),
        route("end-boundary", 1, "node-a", "ingress-c", "corr-c")
    ]);
    let value = minimal(
        inputs.clone(),
        json!([
            {
                "kind": "state",
                "when": {"kind": "throughout", "start_ms": 0, "end_ms": 1},
                "node": "node-a",
                "assertion": {
                    "kind": "clock",
                    "value": {"kind": "ready", "watermark": 1000}
                }
            },
            {
                "kind": "state",
                "when": {"kind": "throughout", "start_ms": 0, "end_ms": 2},
                "node": "node-a",
                "assertion": {
                    "kind": "storage",
                    "retained_bytes": 0,
                    "exhausted": false
                }
            }
        ]),
    );
    assert!(run_scenario(&scenario(&value)).is_ok());

    let outside_half_open = minimal(
        inputs,
        json!([{
            "kind": "state",
            "when": {"kind": "throughout", "start_ms": 0, "end_ms": 2},
            "node": "node-a",
            "assertion": {
                "kind": "clock",
                "value": {"kind": "ready", "watermark": 1000}
            }
        }]),
    );
    assert_eq!(
        run_scenario(&scenario(&outside_half_open)),
        Err(RunnerError::OracleMismatch)
    );
}

#[test]
fn throughout_retained_assertion_is_supported() {
    let mut value = scenario_value("ingest/proof-late.json");
    let assertion = value["expect"][4]["assertion"].clone();
    value["expect"] = json!([{
        "kind": "state",
        "when": {"kind": "throughout", "start_ms": 2, "end_ms": 4},
        "node": "router",
        "assertion": assertion
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn throughout_unresolved_assertion_is_supported() {
    let mut value = scenario_value("ingest/proof-late.json");
    value["expect"] = json!([{
        "kind": "state",
        "when": {"kind": "throughout", "start_ms": 0, "end_ms": 2},
        "node": "router",
        "assertion": {
            "kind": "unresolved",
            "grant_id": {
                "stream": {
                    "share": "home",
                    "glade_id": "directory",
                    "key_hex": "73657276652d6772616e74733a776f726b73706163652d61"
                },
                "origin": "owner-a",
                "seq": 0
            },
            "claim_id": {
                "stream": {
                    "share": "workspace-a",
                    "glade_id": "directory",
                    "key_hex": "73657276652d636c61696d73"
                },
                "origin": "node-principal-a",
                "seq": 0
            },
            "present": true
        }
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn throughout_mine_assertion_is_supported() {
    let mut value = scenario_value("ingest/delayed-sign.json");
    value["inputs"].as_array_mut().expect("inputs").push(route(
        "observe-pending",
        10,
        "node-a",
        "ingress",
        "corr",
    ));
    let assertion = value["expect"][1]["assertion"].clone();
    value["expect"] = json!([{
        "kind": "state",
        "when": {"kind": "throughout", "start_ms": 1, "end_ms": 20},
        "node": "node-a",
        "assertion": assertion
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn throughout_sync_assertion_is_supported() {
    let mut value = scenario_value("sync/sync-round.json");
    value["expect"] = json!([{
        "kind": "state",
        "when": {"kind": "throughout", "start_ms": 1, "end_ms": 4},
        "node": "node-a",
        "assertion": {
            "kind": "sync",
            "peer": "node-b",
            "sync_id": "node-a:0",
            "status": "active"
        }
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}
