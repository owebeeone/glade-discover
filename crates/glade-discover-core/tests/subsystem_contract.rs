use glade_discover_core::{
    GovernanceVerdict, IngestDisposition, LiveCapabilityVerdict, StructuralVerdict, append,
    authority, claims, clock, ingest, projection, routing, sync,
};

#[test]
fn subsystem_entry_points_are_frozen_and_disjoint() {
    let _clock = clock::observe;
    let _ingest = ingest::ingest;
    let _authority = authority::publish_allowed;
    let _projection = projection::live_claims;
    let _routing = routing::resolve;
    let _claims = claims::on_wakeup;
    let _append = append::on_accepted;
    let _sync = sync::on_message;
}

#[test]
fn ingest_verdict_layers_remain_distinct_types() {
    assert_ne!(
        StructuralVerdict::StructurallyValid,
        StructuralVerdict::BadSignature
    );
    assert_ne!(
        GovernanceVerdict::Authorized,
        GovernanceVerdict::Unauthorized
    );
    assert_ne!(
        LiveCapabilityVerdict::Live,
        LiveCapabilityVerdict::UnresolvedProof
    );
    assert_ne!(IngestDisposition::Folded, IngestDisposition::TimeDeferred);
}
