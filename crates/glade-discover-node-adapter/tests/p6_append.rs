use std::collections::BTreeMap;

use glade_discover_core::Event;
use glade_discover_node_adapter::append::{
    self, Allocation, AppendHost, AppendKey, DurableAccepted,
};
use glade_discover_protocol::{
    ClaimDraft, ClaimIdentity, DirectoryRecord, Generation, GrantId, IntentId, NodeId, Principal,
    RecordId, Shape, SignedOp, Slot, StreamId, decode_directory_record, decode_signed_op,
    encode_directory_record, encode_signed_op, unsigned_canonical_bytes,
};

struct FakeHost {
    accepted: BTreeMap<AppendKey, DurableAccepted>,
    calls: Vec<&'static str>,
    signed: Vec<Vec<u8>>,
    signer: Principal,
    signature_valid: bool,
}

impl FakeHost {
    fn fresh() -> Self {
        Self {
            accepted: BTreeMap::new(),
            calls: Vec::new(),
            signed: Vec::new(),
            signer: Principal::from("node-principal"),
            signature_valid: true,
        }
    }
}

impl AppendHost for FakeHost {
    type Error = &'static str;

    fn accepted(&mut self, key: &AppendKey) -> Result<Option<DurableAccepted>, Self::Error> {
        self.calls.push("load");
        Ok(self.accepted.get(key).cloned())
    }

    fn allocate(&mut self, _draft: &ClaimDraft) -> Result<Allocation, Self::Error> {
        self.calls.push("allocate");
        Ok(Allocation {
            stream: claim_stream(),
            seq: 0,
            prev: None,
            lamport: 7,
            shape: Shape::Log,
        })
    }

    fn signer(&self) -> Principal {
        self.signer.clone()
    }

    fn sign(&mut self, unsigned: &[u8]) -> Result<Vec<u8>, Self::Error> {
        self.calls.push("sign");
        self.signed.push(unsigned.to_vec());
        Ok(vec![0xa5, 0x5a])
    }

    fn verify(&mut self, op: &SignedOp) -> Result<bool, Self::Error> {
        self.calls.push("verify");
        Ok(self.signature_valid
            && op.envelope().origin == self.signer
            && op.signature() == [0xa5, 0x5a])
    }

    fn persist_accepted(
        &mut self,
        key: &AppendKey,
        draft: &ClaimDraft,
        canonical: &[u8],
    ) -> Result<(), Self::Error> {
        self.calls.push("persist");
        self.accepted.insert(
            key.clone(),
            DurableAccepted {
                draft: draft.clone(),
                canonical: canonical.to_vec(),
            },
        );
        Ok(())
    }
}

fn claim_stream() -> StreamId {
    StreamId {
        share: "workspace".to_owned(),
        glade_id: "directory".to_owned(),
        key: vec![1],
    }
}

fn grant_id() -> GrantId {
    GrantId::from(RecordId {
        stream: StreamId {
            share: "workspace".to_owned(),
            glade_id: "directory".to_owned(),
            key: vec![0x10],
        },
        origin: Principal::from("owner"),
        seq: 0,
    })
}

fn draft() -> ClaimDraft {
    ClaimDraft::Workspace {
        node: NodeId::from("node-a"),
        share: "workspace".to_owned(),
        identity: ClaimIdentity::Mint,
        grant_ref: grant_id(),
        lease_expiry_ms: 50_000,
        epoch: 0,
    }
}

fn slot() -> Slot {
    Slot::Workspace {
        share: "workspace".to_owned(),
    }
}

#[test]
fn restart_after_persist_before_callback_reuses_the_exact_accepted_op() {
    let intent = IntentId::from("intent-a");
    let generation = Generation(1);
    let mut first_host = FakeHost::fresh();

    let first = append::handle_append(&mut first_host, &slot(), generation, &intent, &draft())
        .expect("first append");
    let Event::OpAccepted { op: first_op, .. } = &first else {
        panic!("adapter must return OpAccepted");
    };
    assert_eq!(
        first_host.calls,
        ["load", "allocate", "sign", "verify", "persist"],
        "the durable write precedes the returned callback"
    );
    assert_eq!(first_host.signed, [unsigned_canonical_bytes(first_op)]);
    assert_eq!(
        first_host
            .accepted
            .values()
            .next()
            .expect("durable entry")
            .canonical,
        first_op.canonical_bytes(),
        "the exact returned bytes are durable"
    );

    // The first callback is deliberately not delivered to the kernel. A new
    // adapter instance sees only the durable intent index after restart.
    let mut restarted = FakeHost {
        accepted: first_host.accepted,
        ..FakeHost::fresh()
    };
    let replay = append::handle_append(&mut restarted, &slot(), generation, &intent, &draft())
        .expect("retry append");

    assert_eq!(replay, first);
    assert_eq!(restarted.calls, ["load", "verify"]);
    assert!(restarted.signed.is_empty(), "retry must not sign again");
    let Event::OpAccepted { op: replayed, .. } = replay else {
        panic!("adapter must replay OpAccepted");
    };
    assert_eq!(
        replayed.canonical_bytes(),
        restarted
            .accepted
            .values()
            .next()
            .expect("durable entry")
            .canonical
    );
}

#[test]
fn allocation_finalizes_the_minted_identity_before_signing() {
    let intent = IntentId::from("intent-b");
    let mut host = FakeHost::fresh();
    let accepted = append::handle_append(&mut host, &slot(), Generation(1), &intent, &draft())
        .expect("append");
    let Event::OpAccepted {
        intent: accepted_intent,
        op,
        ..
    } = accepted
    else {
        panic!("adapter must return OpAccepted");
    };

    assert_eq!(accepted_intent, intent);
    assert_eq!(op.envelope().stream, claim_stream());
    assert_eq!(op.envelope().origin, Principal::from("node-principal"));
    assert_eq!(op.envelope().seq, 0);
    assert_eq!(op.envelope().prev, None);

    let expected_id = RecordId {
        stream: claim_stream(),
        origin: Principal::from("node-principal"),
        seq: 0,
    };
    let record = glade_discover_protocol::decode_directory_record(&op.envelope().payload)
        .expect("workspace claim");
    let glade_discover_protocol::DirectoryRecord::ServeClaim(claim) = record else {
        panic!("workspace draft must finalize as ServeClaim");
    };
    assert_eq!(claim.claim_id.record(), &expected_id);
    assert_eq!(claim.node, NodeId::from("node-a"));
    assert_eq!(claim.grant_ref, grant_id());
}

#[test]
fn the_same_intent_under_another_generation_cannot_replay_the_durable_entry() {
    let intent = IntentId::from("intent-c");
    let mut first = FakeHost::fresh();
    append::handle_append(&mut first, &slot(), Generation(1), &intent, &draft())
        .expect("first append");

    let mut other_generation = FakeHost {
        accepted: first.accepted,
        ..FakeHost::fresh()
    };
    append::handle_append(
        &mut other_generation,
        &slot(),
        Generation(2),
        &intent,
        &draft(),
    )
    .expect("distinct append key");

    assert_eq!(
        other_generation.calls,
        ["load", "allocate", "sign", "verify", "persist"],
        "another generation must not hit the prior durable entry"
    );
    assert_eq!(other_generation.accepted.len(), 2);
}

#[test]
fn durable_replay_fails_closed_for_a_changed_draft_or_foreign_origin() {
    let intent = IntentId::from("intent-d");
    let generation = Generation(1);
    let mut first = FakeHost::fresh();
    append::handle_append(&mut first, &slot(), generation, &intent, &draft())
        .expect("first append");

    let mut changed_draft = draft();
    let ClaimDraft::Workspace {
        lease_expiry_ms, ..
    } = &mut changed_draft
    else {
        unreachable!()
    };
    *lease_expiry_ms += 1;
    let mut restarted = FakeHost {
        accepted: first.accepted.clone(),
        ..FakeHost::fresh()
    };
    assert!(matches!(
        append::handle_append(&mut restarted, &slot(), generation, &intent, &changed_draft,),
        Err(append::AppendError::DraftMismatch)
    ));
    assert_eq!(restarted.calls, ["load"]);

    let mut foreign = FakeHost {
        accepted: first.accepted,
        signer: Principal::from("other-principal"),
        ..FakeHost::fresh()
    };
    assert!(matches!(
        append::handle_append(&mut foreign, &slot(), generation, &intent, &draft()),
        Err(append::AppendError::SignerMismatch)
    ));
    assert_eq!(foreign.calls, ["load"]);
}

#[test]
fn durable_replay_rejects_same_signer_canonical_bytes_with_a_changed_payload() {
    let intent = IntentId::from("intent-e");
    let generation = Generation(1);
    let mut first = FakeHost::fresh();
    append::handle_append(&mut first, &slot(), generation, &intent, &draft())
        .expect("first append");

    let durable = first.accepted.values_mut().next().expect("durable entry");
    let stored = decode_signed_op(&durable.canonical).expect("stored op");
    let DirectoryRecord::ServeClaim(mut substituted) =
        decode_directory_record(&stored.envelope().payload).expect("stored record")
    else {
        unreachable!()
    };
    substituted.lease_expiry_ms += 1;
    let mut envelope = stored.envelope().clone();
    envelope.payload = encode_directory_record(&DirectoryRecord::ServeClaim(substituted))
        .expect("substituted record");
    durable.canonical = encode_signed_op(&envelope, &[0xde, 0xad])
        .expect("substituted signed op")
        .canonical_bytes()
        .to_vec();

    let mut restarted = FakeHost {
        accepted: first.accepted,
        ..FakeHost::fresh()
    };
    assert!(matches!(
        append::handle_append(&mut restarted, &slot(), generation, &intent, &draft()),
        Err(append::AppendError::PayloadMismatch)
    ));
    assert_eq!(restarted.calls, ["load"]);
}

#[test]
fn durable_replay_rejects_signature_only_corruption_before_op_accepted() {
    let intent = IntentId::from("intent-f");
    let generation = Generation(1);
    let mut first = FakeHost::fresh();
    append::handle_append(&mut first, &slot(), generation, &intent, &draft())
        .expect("first append");

    let durable = first.accepted.values_mut().next().expect("durable entry");
    let stored = decode_signed_op(&durable.canonical).expect("stored op");
    durable.canonical = encode_signed_op(stored.envelope(), &[0xa4, 0x5a])
        .expect("canonical corrupt signature")
        .canonical_bytes()
        .to_vec();

    let mut restarted = FakeHost {
        accepted: first.accepted,
        ..FakeHost::fresh()
    };
    assert!(matches!(
        append::handle_append(&mut restarted, &slot(), generation, &intent, &draft()),
        Err(append::AppendError::SignatureInvalid)
    ));
    assert_eq!(restarted.calls, ["load", "verify"]);
}

#[test]
fn freshly_signed_bytes_are_verified_before_the_durable_acceptance_write() {
    let mut host = FakeHost {
        signature_valid: false,
        ..FakeHost::fresh()
    };
    assert!(matches!(
        append::handle_append(
            &mut host,
            &slot(),
            Generation(1),
            &IntentId::from("intent-g"),
            &draft(),
        ),
        Err(append::AppendError::SignatureInvalid)
    ));
    assert_eq!(host.calls, ["load", "allocate", "sign", "verify"]);
    assert!(
        host.accepted.is_empty(),
        "invalid signature must not persist"
    );
}
