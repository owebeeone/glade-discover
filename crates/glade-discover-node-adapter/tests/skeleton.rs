#[test]
fn exposes_node_adapter_crate_identity() {
    assert_eq!(glade_discover_node_adapter::crate_name(), "node-adapter");
}
