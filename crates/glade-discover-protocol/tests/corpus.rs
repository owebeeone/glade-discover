use glade_discover_protocol::{
    CapabilityGrant, CapabilityRevocation, CapabilityVerb, ClaimId, DecodeError, DefRevId,
    DirectoryRecord, ExecutionScope, GrantId, GrantScope, MAX_MESSAGE_BYTES, MAX_RECORD_BYTES,
    MAX_SYNC_ID_BYTES, MIN_SIGNED_OP_BYTES, NodeId, OpEnvelope, Principal, RecordId, ServeClaim,
    ServiceInstanceClaim, Shape, Slot, StreamHead, StreamId, SyncId, WireMsg,
    decode_directory_record, decode_signed_op, decode_stream_head, decode_wire_msg,
    encode_directory_record, encode_signed_op, encode_stream_head, encode_wire_msg, op_hash,
    record_id, sync_list_body_len, unsigned_canonical_bytes,
};

fn hex(value: &str) -> Vec<u8> {
    let compact: String = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert_eq!(compact.len() % 2, 0);
    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = core::str::from_utf8(pair).expect("hex is ASCII");
            u8::from_str_radix(text, 16).expect("valid hex")
        })
        .collect()
}

fn canonical_signed_op() -> Vec<u8> {
    hex(include_str!("../../../protocol/corpus/valid/signed-op.hex"))
}

fn tiny_record_id(origin: &str, seq: u64) -> RecordId {
    RecordId {
        stream: StreamId {
            share: "h".into(),
            glade_id: "g".into(),
            key: Vec::new(),
        },
        origin: Principal::from(origin),
        seq,
    }
}

fn grant_id(origin: &str, seq: u64) -> GrantId {
    GrantId::from(tiny_record_id(origin, seq))
}

fn claim_id(origin: &str, seq: u64) -> ClaimId {
    ClaimId::from(tiny_record_id(origin, seq))
}

fn def_id(origin: &str, seq: u64) -> DefRevId {
    DefRevId::from(tiny_record_id(origin, seq))
}

#[test]
fn signed_op_preserves_exact_canonical_bytes_and_unsigned_compatibility() {
    let bytes = canonical_signed_op();
    let op = decode_signed_op(&bytes).expect("canonical signed op");

    assert_eq!(op.canonical_bytes(), bytes);
    assert_eq!(op.envelope().stream.share, "home");
    assert_eq!(op.envelope().stream.glade_id, "dir.claims");
    assert!(op.envelope().stream.key.is_empty());
    assert_eq!(op.envelope().origin.as_str(), "origin-a");
    assert_eq!(op.envelope().seq, 0);
    assert_eq!(op.envelope().prev, None);
    assert_eq!(op.envelope().lamport, 0);
    assert!(op.envelope().refs.is_empty());
    assert_eq!(op.envelope().payload, [0xA0]);
    assert_eq!(op.signature(), [0xAA, 0xBB]);

    assert_eq!(
        unsigned_canonical_bytes(&op),
        hex(
            "aa0164686f6d65026a6469722e636c61696d73034004686f726967696e2d61050006f60700088009010a41a0"
        )
    );
    let expected_hash: [u8; 32] =
        hex("34a80e05ded26d40e5b2f5e8100fa38aa98689dcb119d232908b411ca8f557a9")
            .try_into()
            .expect("32-byte digest");
    assert_eq!(op_hash(&op), expected_hash);
    let id = record_id(&op);
    assert_eq!(id.stream, op.envelope().stream);
    assert_eq!(id.origin, op.envelope().origin);
    assert_eq!(id.seq, 0);
}

#[test]
fn signed_op_rejects_missing_unknown_noncanonical_and_trailing_data() {
    let missing_signature = hex(include_str!(
        "../../../protocol/corpus/invalid/missing-signature.hex"
    ));
    assert_eq!(
        decode_signed_op(&missing_signature),
        Err(DecodeError::MissingField(11))
    );

    let unknown = hex(include_str!(
        "../../../protocol/corpus/invalid/unknown-field.hex"
    ));
    assert_eq!(
        decode_signed_op(&unknown),
        Err(DecodeError::UnknownField(12))
    );

    let noncanonical = hex(include_str!(
        "../../../protocol/corpus/invalid/noncanonical-seq.hex"
    ));
    assert_eq!(
        decode_signed_op(&noncanonical),
        Err(DecodeError::NonCanonical)
    );

    let trailing = hex(include_str!(
        "../../../protocol/corpus/invalid/trailing-data.hex"
    ));
    assert_eq!(decode_signed_op(&trailing), Err(DecodeError::TrailingData));
}

#[test]
fn signed_op_rejects_wrong_hash_lengths_and_size_limit() {
    let bad_prev = hex(include_str!(
        "../../../protocol/corpus/invalid/short-prev.hex"
    ));
    assert_eq!(
        decode_signed_op(&bad_prev),
        Err(DecodeError::InvalidLength {
            field: "prev",
            expected: 32,
            actual: 1,
        })
    );

    let oversized = vec![0; MAX_RECORD_BYTES + 1];
    assert_eq!(
        decode_signed_op(&oversized),
        Err(DecodeError::RecordTooLarge)
    );
}

#[test]
fn signed_op_rejects_malformed_types_truncation_and_unknown_enums() {
    let wrong_type = hex(include_str!(
        "../../../protocol/corpus/invalid/wrong-top-level-type.hex"
    ));
    assert_eq!(decode_signed_op(&wrong_type), Err(DecodeError::WrongType));

    let truncated = hex(include_str!(
        "../../../protocol/corpus/invalid/truncated-signature.hex"
    ));
    assert_eq!(decode_signed_op(&truncated), Err(DecodeError::Truncated));

    let invalid_utf8 = hex(include_str!(
        "../../../protocol/corpus/invalid/invalid-utf8.hex"
    ));
    assert_eq!(
        decode_signed_op(&invalid_utf8),
        Err(DecodeError::InvalidUtf8)
    );

    let unknown_shape = hex(include_str!(
        "../../../protocol/corpus/invalid/unknown-shape.hex"
    ));
    assert_eq!(
        decode_signed_op(&unknown_shape),
        Err(DecodeError::UnknownEnum {
            name: "Shape",
            value: 3,
        })
    );
}

#[test]
fn stream_head_exactness_and_required_hash_are_fail_closed() {
    let bytes = hex(include_str!(
        "../../../protocol/corpus/valid/stream-head.hex"
    ));
    let head = decode_stream_head(&bytes).expect("canonical stream head");

    assert_eq!(head.stream.share, "home");
    assert_eq!(head.origin.as_str(), "origin-a");
    assert_eq!(head.seq, 7);
    assert_eq!(head.hash, [0xAB; 32]);
    assert_eq!(encode_stream_head(&head), bytes);

    let short_hash =
        hex("a60164686f6d65026a6469722e636c61696d73034004686f726967696e2d6105070641ab");
    assert_eq!(
        decode_stream_head(&short_hash),
        Err(DecodeError::InvalidLength {
            field: "hash",
            expected: 32,
            actual: 1,
        })
    );
}

#[test]
fn every_wire_variant_round_trips_with_an_external_canonical_tag() {
    let op = decode_signed_op(&canonical_signed_op()).expect("canonical op");
    let cases = [
        glade_discover_protocol::WireMsg::DirOp {
            op: Box::new(op.clone()),
        },
        glade_discover_protocol::WireMsg::SyncStart {
            sync_id: "sync-a".into(),
            heads: Vec::new(),
        },
        glade_discover_protocol::WireMsg::SyncOps {
            sync_id: "sync-a".into(),
            ops: vec![op],
        },
        glade_discover_protocol::WireMsg::SyncEnd {
            sync_id: "sync-a".into(),
        },
    ];

    for message in cases {
        let (tag, bytes) = encode_wire_msg(&message);
        assert_eq!(decode_wire_msg(tag, &bytes), Ok(message));
    }

    assert_eq!(
        decode_wire_msg(99, &[]),
        Err(DecodeError::UnknownWireKind(99))
    );
    assert_eq!(
        decode_wire_msg(3, &vec![0; MAX_MESSAGE_BYTES + 1]),
        Err(DecodeError::MessageTooLarge)
    );

    let too_many_ops = hex("a2016673796e632d6102990101");
    assert_eq!(
        decode_wire_msg(2, &too_many_ops),
        Err(DecodeError::TooManyItems {
            field: "ops",
            max: 256,
            actual: 257,
        })
    );
}

#[test]
fn every_wire_sync_id_is_bounded_independently_of_the_message_limit() {
    let too_long = SyncId::from("s".repeat(MAX_SYNC_ID_BYTES + 1));
    for message in [
        WireMsg::SyncStart {
            sync_id: too_long.clone(),
            heads: Vec::new(),
        },
        WireMsg::SyncOps {
            sync_id: too_long.clone(),
            ops: Vec::new(),
        },
        WireMsg::SyncEnd {
            sync_id: too_long.clone(),
        },
    ] {
        let (tag, body) = encode_wire_msg(&message);
        assert!(body.len() < MAX_MESSAGE_BYTES);
        assert_eq!(
            decode_wire_msg(tag, &body),
            Err(DecodeError::SyncIdTooLarge)
        );
    }
}

#[test]
fn minimum_signed_op_size_constant_matches_the_smallest_structural_encoding() {
    let op = encode_signed_op(
        &OpEnvelope {
            stream: StreamId {
                share: String::new(),
                glade_id: String::new(),
                key: Vec::new(),
            },
            origin: Principal::from(""),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: Vec::new(),
            shape: Shape::Value,
            payload: Vec::new(),
        },
        &[],
    )
    .expect("smallest structural op");

    assert_eq!(op.canonical_bytes().len(), MIN_SIGNED_OP_BYTES);
}

#[test]
fn sync_list_size_matches_the_encoder_across_cbor_count_boundaries() {
    let sync_id = SyncId::from("size-round");
    let op = decode_signed_op(&canonical_signed_op()).expect("canonical op");
    let head = StreamHead {
        stream: op.envelope().stream.clone(),
        origin: op.envelope().origin.clone(),
        seq: op.envelope().seq,
        hash: op_hash(&op),
    };

    for count in [0, 23, 24, 255, 256] {
        let ops = vec![op.clone(); count];
        let ops_message = WireMsg::SyncOps {
            sync_id: sync_id.clone(),
            ops,
        };
        let ops_item_bytes = count * op.canonical_bytes().len();
        assert_eq!(
            sync_list_body_len(&sync_id, count, ops_item_bytes),
            Some(encode_wire_msg(&ops_message).1.len())
        );

        let heads = vec![head.clone(); count];
        let heads_message = WireMsg::SyncStart {
            sync_id: sync_id.clone(),
            heads,
        };
        let head_item_bytes = count * encode_stream_head(&head).len();
        assert_eq!(
            sync_list_body_len(&sync_id, count, head_item_bytes),
            Some(encode_wire_msg(&heads_message).1.len())
        );
    }
}

#[test]
fn every_directory_record_kind_round_trips_canonically() {
    let records = [
        DirectoryRecord::ServeClaim(ServeClaim {
            node: NodeId::from("n"),
            share: "w".into(),
            claim_id: claim_id("c", 1),
            grant_ref: grant_id("g", 2),
            lease_expiry_ms: -1,
            epoch: 3,
        }),
        DirectoryRecord::ServiceInstanceClaim(ServiceInstanceClaim {
            node: NodeId::from("n"),
            share: "svc".into(),
            glade_id: "diff".into(),
            key: vec![1],
            claim_id: claim_id("c", 1),
            def_ref: def_id("d", 2),
            exec_grant_ref: grant_id("g", 3),
            compute_key: vec![2].into(),
            lease_expiry_ms: 50,
            epoch: 4,
        }),
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: grant_id("g", 1),
            issuer: Principal::from("issuer"),
            principal: Principal::from("subject"),
            share: "w".into(),
            verbs: vec![CapabilityVerb::Serve, CapabilityVerb::Execute],
            scope: Some(GrantScope::Execution(ExecutionScope {
                def_ref: def_id("d", 2),
                compute_key: vec![3].into(),
            })),
        }),
        DirectoryRecord::CapabilityGrant(CapabilityGrant {
            grant_id: grant_id("g", 4),
            issuer: Principal::from("issuer"),
            principal: Principal::from("subject"),
            share: "w".into(),
            verbs: vec![CapabilityVerb::Takeover],
            scope: Some(GrantScope::Takeover {
                slot: Slot::Binding {
                    share: "svc".into(),
                    glade_id: "diff".into(),
                    key: vec![4],
                },
                supersedes: claim_id("c", 5),
            }),
        }),
        DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
            revokes: grant_id("g", 6),
        }),
    ];

    for record in records {
        let bytes = encode_directory_record(&record).expect("canonical typed record");
        assert_eq!(decode_directory_record(&bytes), Ok(record));
    }
}

#[test]
fn directory_payload_has_stable_bytes_and_distinct_version_rejection() {
    let canonical = hex(include_str!(
        "../../../protocol/corpus/valid/capability-revocation.hex"
    ));
    let expected = DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
        revokes: grant_id("o", 0),
    });
    assert_eq!(decode_directory_record(&canonical), Ok(expected.clone()));
    assert_eq!(encode_directory_record(&expected), Ok(canonical.clone()));

    let unsupported = hex(include_str!(
        "../../../protocol/corpus/invalid/unsupported-record-version.hex"
    ));
    assert_eq!(
        decode_directory_record(&unsupported),
        Err(DecodeError::UnsupportedVersion(2))
    );

    let noncanonical = hex(include_str!(
        "../../../protocol/corpus/invalid/noncanonical-record-version.hex"
    ));
    assert_eq!(
        decode_directory_record(&noncanonical),
        Err(DecodeError::NonCanonical)
    );

    let mut trailing = canonical;
    trailing.push(0);
    assert_eq!(
        decode_directory_record(&trailing),
        Err(DecodeError::TrailingData)
    );
}

#[test]
fn directory_payload_rejects_duplicate_unknown_missing_and_unsorted_collections() {
    let duplicate_outer_key = hex("a401010101020303a101a301a3016168026167034002616f0300");
    assert!(decode_directory_record(&duplicate_outer_key).is_err());

    let unknown_body_field = hex("a30101020303a201a301a3016168026167034002616f03000200");
    assert_eq!(
        decode_directory_record(&unknown_body_field),
        Err(DecodeError::UnknownField(2))
    );

    let missing_body = hex("a201010203");
    assert_eq!(
        decode_directory_record(&missing_body),
        Err(DecodeError::MissingField(3))
    );

    let unsorted = DirectoryRecord::CapabilityGrant(CapabilityGrant {
        grant_id: grant_id("g", 1),
        issuer: Principal::from("issuer"),
        principal: Principal::from("subject"),
        share: "w".into(),
        verbs: vec![CapabilityVerb::Execute, CapabilityVerb::Serve],
        scope: None,
    });
    assert_eq!(
        encode_directory_record(&unsorted),
        Err(DecodeError::NonCanonical)
    );

    let sorted = DirectoryRecord::CapabilityGrant(CapabilityGrant {
        verbs: vec![CapabilityVerb::Serve, CapabilityVerb::Execute],
        ..match unsorted {
            DirectoryRecord::CapabilityGrant(grant) => grant,
            _ => unreachable!(),
        }
    });
    let mut bytes = encode_directory_record(&sorted).expect("sorted verbs");
    let verbs = bytes
        .windows(3)
        .position(|window| window == [0x82, 0x00, 0x01])
        .expect("verb array");
    bytes[verbs + 1] = 1;
    bytes[verbs + 2] = 0;
    assert_eq!(
        decode_directory_record(&bytes),
        Err(DecodeError::NonCanonical)
    );
}

#[test]
fn canonical_signed_op_encoder_constructs_reusable_scenario_fixtures() {
    let record = DirectoryRecord::CapabilityRevocation(CapabilityRevocation {
        revokes: grant_id("o", 0),
    });
    let payload = encode_directory_record(&record).expect("record payload");
    let envelope = OpEnvelope {
        stream: StreamId {
            share: "home".into(),
            glade_id: "dir.revocations".into(),
            key: Vec::new(),
        },
        origin: Principal::from("owner"),
        seq: 7,
        prev: Some([0x11; 32]),
        lamport: 9,
        refs: Vec::new(),
        shape: Shape::Log,
        payload,
    };

    let op = encode_signed_op(&envelope, &[0xAA, 0xBB]).expect("canonical signed op");
    assert_eq!(decode_signed_op(op.canonical_bytes()), Ok(op.clone()));
    assert_eq!(op.envelope(), &envelope);
    assert_eq!(op.signature(), [0xAA, 0xBB]);
}

#[test]
fn service_instance_share_is_the_reserved_svc_namespace() {
    let record = DirectoryRecord::ServiceInstanceClaim(ServiceInstanceClaim {
        node: NodeId::from("n"),
        share: "bad".into(),
        glade_id: "diff".into(),
        key: vec![1],
        claim_id: claim_id("c", 1),
        def_ref: def_id("d", 2),
        exec_grant_ref: grant_id("g", 3),
        compute_key: vec![2].into(),
        lease_expiry_ms: 50,
        epoch: 4,
    });
    assert_eq!(
        encode_directory_record(&record),
        Err(DecodeError::InvalidValue("service_instance.share"))
    );
}
