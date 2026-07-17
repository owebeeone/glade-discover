use std::path::{Path, PathBuf};

use glade_discover_sim::{Scenario, run_scenario};

fn scenario_files() -> Vec<PathBuf> {
    let mut pending = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios")];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("scenario directory") {
            let path = entry.expect("scenario entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn every_scenario_has_a_byte_identical_replay_log() {
    for path in scenario_files() {
        let input = std::fs::read_to_string(&path).expect("scenario JSON");
        let scenario = Scenario::from_json(&input).expect("strict scenario");
        let first = run_scenario(&scenario).expect("first execution");
        let replay = run_scenario(&scenario).expect("replay execution");
        assert_eq!(first, replay, "{}", path.display());
    }
}
