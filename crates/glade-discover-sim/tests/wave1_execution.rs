use std::path::{Path, PathBuf};

use glade_discover_sim::{Scenario, run_scenario};

fn scenario_files(directory: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scenarios")
        .join(directory);
    let mut files = std::fs::read_dir(root)
        .expect("scenario directory")
        .map(|entry| entry.expect("scenario entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[test]
fn wave_one_scenarios_execute_through_the_generic_runner() {
    for directory in ["authz", "clock", "claims", "routing"] {
        for path in scenario_files(directory) {
            let input = std::fs::read_to_string(&path).expect("scenario JSON");
            let scenario = Scenario::from_json(&input).expect("strict scenario");
            run_scenario(&scenario).unwrap_or_else(|error| panic!("{}: {error:?}", path.display()));
        }
    }
}
