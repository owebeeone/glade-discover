use std::path::{Path, PathBuf};

fn scenario_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).expect("read scenario directory") {
            let path = entry.expect("read scenario entry").path();
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

fn decode_all_scenarios() -> usize {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    let files = scenario_files(&root);
    for file in &files {
        let data = std::fs::read_to_string(file).expect("read scenario data");
        glade_discover_sim::Scenario::from_json(&data)
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
    }
    files.len()
}

#[test]
fn discovers_and_decodes_every_scenario_file() {
    assert!(
        decode_all_scenarios() > 0,
        "scenario corpus must not be empty"
    );
}

#[test]
fn every_scenario_passes_the_generic_runner() {
    assert!(decode_all_scenarios() > 0);
    let result = glade_discover_sim::run_all_scenarios();
    assert!(result.is_ok(), "{result:?}");
}
