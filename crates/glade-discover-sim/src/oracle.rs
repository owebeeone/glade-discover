use crate::{RunnerError, Scenario, run_scenario};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Invariant {
    DeterministicPurity,
    NoLeaseResurrection,
    AuthorizedBeforeSpecificity,
    NoClaimOscillation,
    PersistBeforeGossip,
    PostHealConvergence,
}

#[derive(Clone, Copy, Debug)]
pub struct InvariantCase<'a> {
    pub invariant: Invariant,
    pub scenario: &'a Scenario,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvariantEvidence {
    pub invariant: Invariant,
    pub event_count: u64,
}

/// Executes each typed invariant case and its deterministic replay. Scenario
/// expectations remain the invariant-specific oracle; this function prevents
/// evidence from being accepted unless both executions are byte-identical.
pub fn verify_invariants(
    cases: &[InvariantCase<'_>],
) -> Result<Vec<InvariantEvidence>, RunnerError> {
    cases
        .iter()
        .map(|case| {
            let first = run_scenario(case.scenario)?;
            let replay = run_scenario(case.scenario)?;
            if first != replay {
                return Err(RunnerError::OracleMismatch);
            }
            Ok(InvariantEvidence {
                invariant: case.invariant,
                event_count: first.event_count(),
            })
        })
        .collect()
}
