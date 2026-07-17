use std::fs;
use std::path::{Path, PathBuf};

use glade_discover_sim::{RunReport, Scenario, run_scenario};
use serde_json::{Value, json};

const FAULT_SEEDS: &[u64] = &[
    0,
    1,
    2,
    3,
    5,
    8,
    13,
    21,
    34,
    55,
    89,
    144,
    233,
    377,
    610,
    987,
    u64::MAX,
];

// At equal timestamps scripted inputs are inserted before restart events. These
// cuts therefore exercise: post-grant/pre-advertise, post-Append, pending,
// immediately pre-accept, and post-accept restoration. Pending restoration must
// itself schedule the retry; the base scenario contains no authored wakeup.
const APPEND_RESTART_CUTS_MS: &[u64] = &[0, 1, 5, 9, 10];

fn scenario_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")
}

fn load_value(relative: &str) -> Value {
    let path = scenario_root().join(relative);
    serde_json::from_str(&fs::read_to_string(path).expect("scenario JSON"))
        .expect("typed JSON value")
}

fn parse(value: &Value) -> Scenario {
    Scenario::from_json(&serde_json::to_string(value).expect("serialize generated scenario"))
        .expect("strict generated scenario")
}

fn run_twice(scenario: &Scenario) -> RunReport {
    let first = run_scenario(scenario).expect("first adversarial execution");
    let replay = run_scenario(scenario).expect("deterministic replay");
    assert_eq!(first, replay, "same seed must produce an identical report");
    assert!(first.event_count() > 0);
    first
}

fn fault_scenario(seed: u64, faults: Value) -> Scenario {
    let mut value = load_value("sync/sync-round.json");
    value["seed"] = json!(seed);
    value["event_budget"] = json!(2_000);
    value["links"] = json!([{
        "a": "node-a",
        "b": "node-b",
        "latency_ms": 1,
        "faults": faults,
    }]);
    value["inputs"] = Value::Array(
        value["inputs"]
            .as_array()
            .expect("inputs")
            .iter()
            .filter(|input| matches!(input["input_id"].as_str(), Some("seed-op" | "start-round")))
            .cloned()
            .collect(),
    );
    value["verifier_fixtures"] = Value::Array(
        value["verifier_fixtures"]
            .as_array()
            .expect("fixtures")
            .iter()
            .filter(|fixture| fixture["selector"]["input_id"] == "seed-op")
            .cloned()
            .collect(),
    );
    value["stop"] = json!({"kind": "at_ms", "at_ms": 25_000});
    value["expect"] = json!([
        {
            "when": {"kind": "at_ms", "at_ms": 25_000},
            "kind": "state",
            "node": "node-a",
            "assertion": {
                "kind": "retained",
                "record_id": {
                    "stream": {
                        "share": "workspace-sync",
                        "glade_id": "directory",
                        "key_hex": "71"
                    },
                    "origin": "owner-sync",
                    "seq": 0
                },
                "present": true
            }
        },
        {
            "when": {"kind": "at_ms", "at_ms": 25_000},
            "kind": "converged",
            "nodes": ["node-a", "node-b"]
        }
    ]);
    parse(&value)
}

#[test]
fn declared_fault_seed_sweep_is_replay_stable_and_converges_after_heal() {
    let profiles = [
        (
            "loss_then_heal",
            json!([{
                "kind": "loss",
                "start_ms": 0,
                "end_ms": 2,
                "probability_ppm": 500_000
            }]),
        ),
        (
            "reorder_and_duplicate",
            json!([
                {
                    "kind": "reorder",
                    "start_ms": 0,
                    "end_ms": 100,
                    "extra_delay_ms": 5
                },
                {
                    "kind": "duplicate",
                    "start_ms": 0,
                    "end_ms": 100,
                    "copies": 2,
                    "spacing_ms": 1
                }
            ]),
        ),
        (
            "partition_then_heal",
            json!([{
                "kind": "partition",
                "start_ms": 0,
                "end_ms": 2
            }]),
        ),
    ];

    for seed in FAULT_SEEDS {
        for (profile, faults) in &profiles {
            let scenario = fault_scenario(*seed, faults.clone());
            let report = run_twice(&scenario);
            assert!(
                !report.event_log().is_empty(),
                "seed {seed}, profile {profile} must execute the generic runner"
            );
        }
    }
}

#[test]
fn restart_sweep_covers_every_expressible_append_transition() {
    for restart_at in APPEND_RESTART_CUTS_MS {
        let mut value = load_value("ingest/append-restart.json");
        value["nodes"][0]["restarts"] = json!([{
            "at_ms": restart_at,
            "watermark": {"kind": "preserve"}
        }]);
        let expectations = value["expect"].as_array_mut().expect("expectations");
        let retry_template = expectations
            .iter()
            .find(|expectation| {
                expectation["kind"] == "effect"
                    && expectation["effect"]["kind"] == "append"
                    && expectation["when"]["at_ms"] == 5
            })
            .cloned()
            .expect("base recovery expectation");
        expectations.retain(|expectation| {
            !(expectation["kind"] == "effect"
                && expectation["effect"]["kind"] == "append"
                && expectation["when"]["at_ms"] == 5)
        });
        if matches!(*restart_at, 1 | 5 | 9) {
            if *restart_at == 1 {
                let initial = expectations
                    .iter_mut()
                    .find(|expectation| {
                        expectation["kind"] == "effect"
                            && expectation["effect"]["kind"] == "append"
                            && expectation["when"]["at_ms"] == 1
                    })
                    .expect("initial append expectation");
                initial["count"] = json!(2);
            } else {
                let mut recovery = retry_template;
                recovery["when"]["at_ms"] = json!(restart_at);
                expectations.push(recovery);
            }
        }
        let scenario = parse(&value);
        let report = run_twice(&scenario);
        assert!(
            report
                .event_log()
                .iter()
                .any(|entry| entry.contains("Restart")),
            "restart cut {restart_at} must be observed"
        );
    }
}
