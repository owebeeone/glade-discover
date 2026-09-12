use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn run() -> Result<(), String> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--locked",
            "--offline",
        ])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let metadata = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let policy = std::fs::read(root.join("architecture-policy.json")).map_err(|e| e.to_string())?;
    let policy = serde_json::from_slice(&policy).map_err(|e| e.to_string())?;
    let errors = glade_architecture_check::check(metadata, policy, |p| {
        std::fs::read_to_string(p).map_err(|e| e.to_string())
    })?;
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    println!(
        "Architecture boundaries: PASS (workspace libraries, declared dependencies, traits and conformance targets)"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
