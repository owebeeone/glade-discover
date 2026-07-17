use std::path::Path;

use glade_discover_sim::{Scenario, run_scenario};

#[test]
fn route_terminal_executes_through_the_generic_runner() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/routing/route-terminal.json");
    let input = std::fs::read_to_string(path).expect("read route-terminal scenario");
    let scenario = Scenario::from_json(&input).expect("strict scenario decode");

    let first = run_scenario(&scenario).expect("walking skeleton succeeds");
    let replay = run_scenario(&scenario).expect("deterministic replay succeeds");
    assert_eq!(first, replay);
}
