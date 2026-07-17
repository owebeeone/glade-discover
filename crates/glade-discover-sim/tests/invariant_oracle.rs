use std::path::Path;

use glade_discover_sim::{Invariant, InvariantCase, Scenario, verify_invariants};

fn load(relative: &str) -> Scenario {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scenarios")
        .join(relative);
    Scenario::from_json(&std::fs::read_to_string(path).expect("scenario JSON"))
        .expect("strict scenario")
}

#[test]
fn cross_lane_invariants_have_executable_replay_stable_evidence() {
    let cases = [
        (
            Invariant::DeterministicPurity,
            load("routing/corr-collision.json"),
        ),
        (
            Invariant::NoLeaseResurrection,
            load("clock/wall-rollback.json"),
        ),
        (
            Invariant::AuthorizedBeforeSpecificity,
            load("authz/inst-forged-node.json"),
        ),
        (
            Invariant::NoClaimOscillation,
            load("claims/no-ping-pong.json"),
        ),
        (
            Invariant::PersistBeforeGossip,
            load("ingest/append-restart.json"),
        ),
        (Invariant::PostHealConvergence, load("sync/sync-round.json")),
    ];
    let borrowed = cases
        .iter()
        .map(|(invariant, scenario)| InvariantCase {
            invariant: *invariant,
            scenario,
        })
        .collect::<Vec<_>>();

    let evidence = verify_invariants(&borrowed).expect("invariant suite");
    assert_eq!(evidence.len(), cases.len());
    assert!(evidence.iter().all(|item| item.event_count > 0));
}
