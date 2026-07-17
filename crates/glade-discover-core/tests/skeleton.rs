#[test]
fn exposes_core_crate_identity() {
    assert_eq!(glade_discover_core::crate_name(), "core");
}
