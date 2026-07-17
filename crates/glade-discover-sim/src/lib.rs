//! Deterministic adversarial simulator for Glade discovery.

mod faults;
mod network;
mod oracle;
mod queue;
mod runner;
mod schema;

pub use oracle::{Invariant, InvariantCase, InvariantEvidence, verify_invariants};
pub use runner::{RunReport, RunnerError, run_all_scenarios, run_scenario};
pub use schema::{Scenario, ScenarioDecodeError};

/// Returns the stable role name used by workspace smoke tests.
#[must_use]
pub const fn crate_name() -> &'static str {
    "sim"
}
