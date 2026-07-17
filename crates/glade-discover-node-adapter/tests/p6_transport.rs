use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::{
    Effect, Event, KernelConfig, MonoInstant, PersistedState, StepCtx, VerificationBatch,
    VerificationResult, WallMs, WatermarkLoad, restore_fresh, step,
};
use glade_discover_node_adapter::transport::{
    GossipSend, Transport, dispatch_gossip, inbound_deliver,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, DirectoryRecord, GrantId, NodeId, OpEnvelope, Principal,
    RecordId, Shape, StreamId, WireMsg, encode_directory_record, encode_signed_op,
};

#[derive(Default)]
struct RecordingTransport {
    sent: Vec<(NodeId, WireMsg)>,
}

impl Transport for RecordingTransport {
    type Error = &'static str;

    fn send(&mut self, to: &NodeId, message: &WireMsg) -> Result<(), Self::Error> {
        self.sent.push((to.clone(), message.clone()));
        Ok(())
    }
}

struct FailingTransport;

impl Transport for FailingTransport {
    type Error = &'static str;

    fn send(&mut self, _to: &NodeId, _message: &WireMsg) -> Result<(), Self::Error> {
        Err("link-down")
    }
}

fn stream() -> StreamId {
    StreamId {
        share: "workspace-a".to_owned(),
        glade_id: "directory".to_owned(),
        key: vec![0x61],
    }
}

fn grant_op() -> glade_discover_protocol::SignedOp {
    let record_id = RecordId {
        stream: stream(),
        origin: Principal::from("owner-a"),
        seq: 0,
    };
    let payload = encode_directory_record(&DirectoryRecord::CapabilityGrant(CapabilityGrant {
        grant_id: GrantId::from(record_id),
        issuer: Principal::from("owner-a"),
        principal: Principal::from("node-a"),
        share: "workspace-a".to_owned(),
        verbs: vec![CapabilityVerb::Serve],
        scope: None,
    }))
    .expect("canonical grant");
    encode_signed_op(
        &OpEnvelope {
            stream: stream(),
            origin: Principal::from("owner-a"),
            seq: 0,
            prev: None,
            lamport: 1,
            refs: Vec::new(),
            shape: Shape::Log,
            payload,
        },
        &[0xa5, 0x5a],
    )
    .expect("canonical signed op")
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("owner-a"),
        peers: BTreeSet::from([NodeId::from("node-b")]),
        workspace_owner_roots: BTreeMap::from([(
            "workspace-a".to_owned(),
            Principal::from("owner-a"),
        )]),
        node_principal_bindings: BTreeSet::new(),
        skew_margin_ms: 5_000,
        max_lease_ms: 3_600_000,
        clock_resync_ms: 30_000,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(10_000).expect("non-zero"),
        gossip_fan: 8,
        max_retained_bytes: NonZeroU64::new(1_048_576).expect("non-zero"),
    }
}

fn ctx() -> StepCtx {
    StepCtx {
        mono: MonoInstant(0),
        wall: WallMs(100_000),
    }
}

#[test]
fn verified_inbound_message_maps_exactly_to_deliver_event() {
    let op = grant_op();
    let message = WireMsg::DirOp {
        op: Box::new(op.clone()),
    };
    let verification = VerificationBatch(vec![VerificationResult::Valid {
        signer: Principal::from("owner-a"),
    }]);

    assert_eq!(
        inbound_deliver(
            NodeId::from("node-b"),
            message.clone(),
            verification.clone(),
        ),
        Event::Deliver {
            from: NodeId::from("node-b"),
            msg: Box::new(message),
            verification,
        }
    );
}

#[test]
fn gossip_sends_the_exact_typed_message_and_canonical_op_bytes() {
    let op = grant_op();
    let canonical = op.canonical_bytes().to_vec();
    let effect = Effect::Gossip {
        to: NodeId::from("node-b"),
        msg: Box::new(WireMsg::DirOp {
            op: Box::new(op.clone()),
        }),
    };
    let mut transport = RecordingTransport::default();

    assert_eq!(
        dispatch_gossip(&mut transport, &effect),
        Some(GossipSend::Sent {
            to: NodeId::from("node-b"),
        })
    );
    assert_eq!(transport.sent.len(), 1);
    assert_eq!(transport.sent[0].0, NodeId::from("node-b"));
    let WireMsg::DirOp { op: sent } = &transport.sent[0].1 else {
        panic!("gossip changed the message variant");
    };
    assert_eq!(sent.as_ref(), &op);
    assert_eq!(sent.canonical_bytes(), canonical);
}

#[test]
fn transport_loss_is_reported_without_reverting_the_already_folded_state() {
    let op = grant_op();
    let initial = restore_fresh(
        config(),
        PersistedState::default(),
        WatermarkLoad::Readable(WallMs(100_000)),
        ctx(),
    );
    let event = inbound_deliver(
        NodeId::from("node-b"),
        WireMsg::DirOp {
            op: Box::new(op.clone()),
        },
        VerificationBatch(vec![VerificationResult::Valid {
            signer: Principal::from("owner-a"),
        }]),
    );
    let (committed, _) = step(initial, ctx(), event);
    let persisted_after_step = committed.persisted().clone();
    assert_eq!(
        persisted_after_step.retained_bytes,
        op.canonical_bytes().len() as u64
    );

    let effect = Effect::Gossip {
        to: NodeId::from("node-b"),
        msg: Box::new(WireMsg::DirOp { op: Box::new(op) }),
    };
    let mut transport = FailingTransport;
    assert_eq!(
        dispatch_gossip(&mut transport, &effect),
        Some(GossipSend::Failed {
            to: NodeId::from("node-b"),
            error: "link-down",
        })
    );

    assert_eq!(committed.persisted(), &persisted_after_step);
    assert_eq!(committed.persisted().retained.len(), 1);
}
