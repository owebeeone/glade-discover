use glade_discover_sim::Scenario;

const VALID: &str = r#"
{
  "seed": 1,
  "event_budget": 100,
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
    "input_id": "route-1",
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
    "when": {"kind": "at_ms", "at_ms": 0},
    "kind": "route",
    "node": "node-a",
    "ingress": "ingress-a",
    "corr": "corr-a",
    "ans": {"kind": "no_claim"}
  }]
}
"#;

#[test]
fn decodes_the_strict_minimal_scenario() {
    let scenario = Scenario::from_json(VALID).expect("valid strict scenario");

    assert_eq!(scenario.seed, 1);
    assert_eq!(scenario.inputs.len(), 1);
}

#[test]
fn rejects_unknown_fields() {
    let hostile = VALID.replace("\"seed\": 1,", "\"seed\": 1, \"surprise\": true,");

    let error = Scenario::from_json(&hostile).expect_err("unknown field must fail closed");
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn rejects_zero_event_budget() {
    let hostile = VALID.replace("\"event_budget\": 100", "\"event_budget\": 0");

    assert!(Scenario::from_json(&hostile).is_err());
}

#[test]
fn rejects_duplicate_input_ids() {
    let duplicated_input = r#",{
      "input_id": "route-1",
      "at_ms": 1,
      "node": "node-a",
      "event": {
        "kind": "clock_reseed",
        "watermark": 1001
      }
    }"#;
    let hostile = VALID.replace(
        "}],\n  \"verifier_fixtures\"",
        &format!("}}{duplicated_input}],\n  \"verifier_fixtures\""),
    );

    let error = Scenario::from_json(&hostile).expect_err("duplicate id must fail");
    assert!(error.to_string().contains("duplicate input_id"));
}

#[test]
fn rejects_input_op_selector_out_of_range() {
    let fixture = r#"{
      "selector": {"kind": "input", "input_id": "route-1", "op_ordinal": 0},
      "reader": null,
      "outcome": {"kind": "bad_signature"}
    }"#;
    let hostile = VALID.replace(
        "\"verifier_fixtures\": []",
        &format!("\"verifier_fixtures\": [{fixture}]"),
    );

    let error = Scenario::from_json(&hostile).expect_err("route has no embedded op");
    assert!(error.to_string().contains("op selector"));
}

#[test]
fn rejects_invalid_half_open_fault_interval() {
    let link = r#"{
      "a": "node-a",
      "b": "node-a",
      "latency_ms": 0,
      "faults": [{"kind": "partition", "start_ms": 5, "end_ms": 5}]
    }"#;
    let hostile = VALID.replace("\"links\": []", &format!("\"links\": [{link}]"));

    let error = Scenario::from_json(&hostile).expect_err("empty interval must fail");
    assert!(error.to_string().contains("half-open interval"));
}
