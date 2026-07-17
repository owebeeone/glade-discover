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

#[test]
fn append_without_an_authored_callback_is_finalized_by_the_environment() {
    let mut value = scenario_value("ingest/delayed-sign.json");
    value["inputs"]
        .as_array_mut()
        .expect("inputs")
        .retain(|input| input["input_id"] != "accepted");
    value["stop"] = json!({"kind": "at_ms", "at_ms": 3});
    value["expect"] = json!([{
        "when": {"kind": "at_ms", "at_ms": 3},
        "kind": "state",
        "node": "node-a",
        "assertion": {
            "kind": "mine",
            "slot": {"kind": "workspace", "share": "workspace-a"},
            "generation": 1,
            "status": "accepted"
        }
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn auto_finalized_append_uses_its_minted_verifier_fixture() {
    let mut value = scenario_value("ingest/delayed-sign.json");
    value["inputs"]
        .as_array_mut()
        .expect("inputs")
        .retain(|input| input["input_id"] != "accepted");
    value["verifier_fixtures"][1]["outcome"] = json!({"kind": "bad_signature"});
    value["stop"] = json!({"kind": "at_ms", "at_ms": 3});
    value["expect"] = json!([{
        "when": {"kind": "at_ms", "at_ms": 3},
        "kind": "state",
        "node": "node-a",
        "assertion": {
            "kind": "mine",
            "slot": {"kind": "workspace", "share": "workspace-a"},
            "generation": 1,
            "status": "pending"
        }
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn an_unbound_minted_fixture_fails_scenario_completion() {
    let scenario = Scenario::from_json(
        r#"{
          "seed":1,"event_budget":10,
          "nodes":[{"id":"node-a","principal":"principal-a","owns":[],"seed_grants":[],
            "clock":{"initial_wall_ms":1000,"initial_mono_ms":0,"changes":[]},"restarts":[]}],
          "links":[],
          "inputs":[{"input_id":"route","at_ms":0,"node":"node-a","event":{"kind":"route",
            "ingress":"ingress","principal":"reader","authenticated_context_hex":"",
            "corr":"corr","query":{"kind":"workspace","share":"workspace-a"}}}],
          "verifier_fixtures":[{"selector":{"kind":"minted","node":"node-a",
            "slot":{"kind":"workspace","share":"workspace-a"},"generation":1,
            "append_ordinal":0},"reader":null,"outcome":{"kind":"bad_signature"}}],
          "stop":{"kind":"at_ms","at_ms":1},
          "expect":[{"kind":"route","when":{"kind":"at_ms","at_ms":0},"node":"node-a",
            "ingress":"ingress","corr":"corr","ans":{"kind":"no_claim"}}]
        }"#,
    )
    .expect("strict scenario");

    assert_eq!(run_scenario(&scenario), Err(RunnerError::InvalidSetup));
}

#[test]
fn seed_grants_are_folded_before_the_first_scenario_event() {
    let mut value = scenario_value("ingest/delayed-sign.json");
    let seed_op = value["inputs"][0]["event"]["msg"]["op"].clone();
    value["nodes"][0]["seed_grants"] = json!([seed_op]);
    value["inputs"] = json!([]);
    value["verifier_fixtures"] = json!([]);
    value["stop"] = json!({"kind": "at_ms", "at_ms": 0});
    value["expect"] = json!([{
        "when": {"kind": "at_ms", "at_ms": 0},
        "kind": "state",
        "node": "node-a",
        "assertion": {
            "kind": "retained",
            "record_id": {
                "stream": {"share": "workspace-a", "glade_id": "directory", "key_hex": "a1"},
                "origin": "owner-a",
                "seq": 0
            },
            "present": true
        }
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn repeated_append_before_generated_callback_replays_without_reminting() {
    let mut value = scenario_value("ingest/delayed-sign.json");
    value["inputs"]
        .as_array_mut()
        .expect("inputs")
        .retain(|input| input["input_id"] != "accepted");
    let mut duplicate = value["inputs"][1].clone();
    duplicate["input_id"] = json!("advertise-duplicate");
    value["inputs"]
        .as_array_mut()
        .expect("inputs")
        .push(duplicate);
    value["stop"] = json!({"kind": "at_ms", "at_ms": 3});
    value["expect"] = json!([{
        "when": {"kind": "at_ms", "at_ms": 3},
        "kind": "state",
        "node": "node-a",
        "assertion": {
            "kind": "mine",
            "slot": {"kind": "workspace", "share": "workspace-a"},
            "generation": 1,
            "status": "accepted"
        }
    }]);

    assert!(run_scenario(&scenario(&value)).is_ok());
}

#[test]
fn unauthorized_seed_grant_fails_scenario_setup() {
    let mut value = scenario_value("ingest/delayed-sign.json");
    let seed_op = value["inputs"][0]["event"]["msg"]["op"].clone();
    value["nodes"][0]["owns"] = json!([]);
    value["nodes"][0]["seed_grants"] = json!([seed_op]);
    value["inputs"] = json!([]);
    value["verifier_fixtures"] = json!([]);
    value["stop"] = json!({"kind": "at_ms", "at_ms": 0});
    value["expect"] = json!([{
        "when": {"kind": "at_ms", "at_ms": 0},
        "kind": "state",
        "node": "node-a",
        "assertion": {
            "kind": "clock",
            "value": {"kind": "ready", "watermark": 100000}
        }
    }]);

    assert_eq!(
        run_scenario(&scenario(&value)),
        Err(RunnerError::InvalidSetup)
    );
}
