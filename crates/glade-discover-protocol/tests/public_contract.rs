use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimDraft, ClaimId, ClaimIdentity, DefRevId, DirectoryRecord,
    GrantId, Head, NodeId, OpEnvelope, Principal, RecordId, ServeClaim, Shape, SignedOp, Slot,
    StreamHead, StreamId, SyncId, WireMsg,
};

fn stream() -> StreamId {
    StreamId {
        share: "workspace-a".into(),
        glade_id: "directory".into(),
        key: vec![0x01],
    }
}

fn record_id(origin: &str, seq: u64) -> RecordId {
    RecordId {
        stream: stream(),
        origin: Principal::from(origin),
        seq,
    }
}

#[test]
fn record_identity_contains_the_full_stream_axis() {
    let first = record_id("origin-a", 1);
    let different_stream = RecordId {
        stream: StreamId {
            key: vec![0x02],
            ..stream()
        },
        origin: Principal::from("origin-a"),
        seq: 1,
    };

    assert_ne!(first, different_stream);
    assert_eq!(GrantId::from(first.clone()).record(), &first);
    assert_eq!(ClaimId::from(first.clone()).record(), &first);
    assert_eq!(DefRevId::from(first.clone()).record(), &first);
}

#[test]
fn claim_draft_can_request_identity_finalization_without_a_claim_id() {
    let draft = ClaimDraft::Workspace {
        node: NodeId::from("node-a"),
        share: "workspace-a".into(),
        identity: ClaimIdentity::Mint,
        grant_ref: GrantId::from(record_id("owner-a", 4)),
        lease_expiry_ms: 50_000,
        epoch: 3,
    };

    assert!(matches!(
        draft,
        ClaimDraft::Workspace {
            identity: ClaimIdentity::Mint,
            ..
        }
    ));
}

#[test]
fn finalized_claim_and_capability_are_typed_records() {
    let claim_id = ClaimId::from(record_id("origin-a", 1));
    let grant_id = GrantId::from(record_id("owner-a", 4));
    let claim = DirectoryRecord::ServeClaim(ServeClaim {
        node: NodeId::from("node-a"),
        share: "workspace-a".into(),
        claim_id,
        grant_ref: grant_id.clone(),
        lease_expiry_ms: 50_000,
        epoch: 3,
    });
    let grant = DirectoryRecord::CapabilityGrant(CapabilityGrant {
        grant_id,
        issuer: Principal::from("owner-a"),
        principal: Principal::from("node-principal-a"),
        share: "workspace-a".into(),
        verbs: vec![CapabilityVerb::Serve],
        scope: None,
    });

    assert!(matches!(claim, DirectoryRecord::ServeClaim(_)));
    assert!(matches!(grant, DirectoryRecord::CapabilityGrant(_)));
}

#[test]
fn slot_and_stream_head_preserve_canonical_binding_axes() {
    let workspace = Slot::Workspace {
        share: "workspace-a".into(),
    };
    let binding = Slot::Binding {
        share: "svc".into(),
        glade_id: "glade-a".into(),
        key: vec![0x01, 0x02],
    };
    let head = StreamHead {
        stream: stream(),
        origin: Principal::from("origin-a"),
        seq: 7,
        hash: [0xAB; 32],
    };

    assert_ne!(workspace, binding);
    assert_eq!(head.stream.key, [0x01]);
    assert_eq!(head.hash, [0xAB; 32]);
}

#[test]
fn wire_contract_has_correlated_rounds_and_exact_byte_ops() {
    fn assert_value_traits<T: Clone + core::fmt::Debug + Eq + Ord>() {}
    assert_value_traits::<SignedOp>();

    let messages = [
        WireMsg::SyncStart {
            sync_id: SyncId::from("round-1"),
            heads: Vec::new(),
        },
        WireMsg::SyncOps {
            sync_id: SyncId::from("round-1"),
            ops: Vec::new(),
        },
        WireMsg::SyncEnd {
            sync_id: SyncId::from("round-1"),
        },
    ];

    assert_eq!(messages.len(), 3);
}

#[test]
fn op_envelope_uses_legacy_compatible_causal_heads() {
    let envelope = OpEnvelope {
        stream: stream(),
        origin: Principal::from("origin-a"),
        seq: 2,
        prev: Some([0x11; 32]),
        lamport: 3,
        refs: vec![Head {
            origin: Principal::from("origin-b"),
            seq: 8,
            hash: None,
        }],
        shape: Shape::Value,
        payload: vec![0xA0],
    };

    assert_eq!(envelope.refs[0].seq, 8);
    assert!(envelope.refs[0].hash.is_none());
}
