#[test]
fn exposes_simulator_crate_identity() {
    assert_eq!(glade_discover_sim::crate_name(), "sim");
}
