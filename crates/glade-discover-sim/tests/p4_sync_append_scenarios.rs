use std::fs;
use std::path::Path;

use glade_discover_sim::{Scenario, run_scenario};

const SCENARIOS: &[&str] = &[
    "sync/sync-round.json",
    "sync/sync-drop.json",
    "sync/sync-retry.json",
    "ingest/append-restart.json",
    "ingest/delayed-sign.json",
];

#[test]
fn sync_and_append_scenarios_execute_through_the_generic_runner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let failures = SCENARIOS
        .iter()
        .filter_map(|relative| {
            let input = fs::read_to_string(root.join(relative)).expect("scenario data");
            let scenario = Scenario::from_json(&input).expect("strict scenario decode");
            run_scenario(&scenario)
                .err()
                .map(|error| format!("{relative}: {error:?}"))
        })
        .collect::<Vec<_>>();

    assert!(
        failures.is_empty(),
        "P4.3 runner failures:\n{}",
        failures.join("\n")
    );
}
