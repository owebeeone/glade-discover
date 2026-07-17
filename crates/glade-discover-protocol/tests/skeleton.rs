#[test]
fn exposes_protocol_crate_identity() {
    assert_eq!(glade_discover_protocol::crate_name(), "protocol");
}
