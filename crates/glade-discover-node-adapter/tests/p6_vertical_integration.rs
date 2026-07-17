use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::rc::Rc;

use glade_discover_core::{
    ClaimCommand, ClaimMode, ClockState, Effect, Event, KernelConfig, MineStatus, MonoInstant,
    NodePrincipalBinding, PersistedState, PrincipalPlane, StepCtx, WallMs, WatermarkLoad,
    restore_fresh,
};
use glade_discover_node_adapter::append::{
    self, Allocation, AppendHost, AppendKey, DurableAccepted,
};
use glade_discover_node_adapter::driver::{
    DurableCommit, DurableSnapshot, EffectExecutor, NodeDriver,
};
use glade_discover_node_adapter::transport::{GossipSend, Transport, dispatch_gossip};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimDraft, ClaimIdentity, DirectoryRecord, Generation,
    GrantId, IntentId, NodeId, OpEnvelope, Principal, RecordId, Shape, Slot, StreamId, WireMsg,
    encode_directory_record, encode_signed_op,
};

fn grant_stream() -> StreamId {
    StreamId {
        share: "workspace".into(),
        glade_id: "directory".into(),
        key: vec![0x10],
    }
}

fn claim_stream() -> StreamId {
    StreamId {
        share: "workspace".into(),
        glade_id: "directory".into(),
        key: vec![0x20],
    }
}

fn grant_id() -> GrantId {
    GrantId::from(RecordId {
        stream: grant_stream(),
        origin: Principal::from("owner"),
        seq: 0,
    })
}

fn persisted_with_grant() -> PersistedState {
    let op = encode_signed_op(
        &OpEnvelope {
            stream: grant_stream(),
            origin: Principal::from("owner"),
            seq: 0,
            prev: None,
            lamport: 0,
            refs: Vec::new(),
            shape: Shape::Log,
            payload: encode_directory_record(&DirectoryRecord::CapabilityGrant(CapabilityGrant {
                grant_id: grant_id(),
                issuer: Principal::from("owner"),
                principal: Principal::from("node-principal"),
                share: "workspace".into(),
                verbs: vec![CapabilityVerb::Serve],
                scope: None,
            }))
            .expect("canonical grant"),
        },
        &[0x01],
    )
    .expect("signed grant");
    let mut persisted = PersistedState {
        retained_bytes: u64::try_from(op.canonical_bytes().len()).expect("small fixture"),
        ..PersistedState::default()
    };
    persisted
        .retained
        .entry(grant_stream())
        .or_default()
        .insert(grant_id().record().clone(), op);
    persisted
}

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("node-principal"),
        peers: BTreeSet::from([NodeId::from("node-b")]),
        workspace_owner_roots: BTreeMap::from([("workspace".into(), Principal::from("owner"))]),
        node_principal_bindings: BTreeSet::from([NodePrincipalBinding {
            node: NodeId::from("node-a"),
            principal: Principal::from("node-principal"),
            plane: PrincipalPlane::Workspace,
        }]),
        skew_margin_ms: 5,
        max_lease_ms: 10_000,
        clock_resync_ms: 30,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(1_000).expect("non-zero"),
        gossip_fan: 1,
        max_retained_bytes: NonZeroU64::new(1_000_000).expect("non-zero"),
    }
}

fn slot() -> Slot {
    Slot::Workspace {
        share: "workspace".into(),
    }
}

fn draft() -> ClaimDraft {
    ClaimDraft::Workspace {
        node: NodeId::from("node-a"),
        share: "workspace".into(),
        identity: ClaimIdentity::Mint,
        grant_ref: grant_id(),
        lease_expiry_ms: 5_000,
        epoch: 0,
    }
}

fn advertise() -> Event {
    Event::Advertise {
        command: Box::new(ClaimCommand {
            intent: IntentId::from("intent-a"),
            slot: slot(),
            generation: Generation(1),
            mode: ClaimMode::Initial,
            draft: draft(),
        }),
    }
}

struct Store {
    log: Rc<RefCell<Vec<&'static str>>>,
    snapshots: Vec<DurableSnapshot>,
}

impl DurableCommit for Store {
    type Error = &'static str;

    fn authenticate(&mut self, _snapshot: &DurableSnapshot) -> Result<bool, Self::Error> {
        Ok(true)
    }

    fn commit(
        &mut self,
        expected_revision: Option<u64>,
        snapshot: &DurableSnapshot,
    ) -> Result<(), Self::Error> {
        self.log.borrow_mut().push("state-commit");
        assert_eq!(
            self.snapshots.last().map(|current| current.revision),
            expected_revision
        );
        self.snapshots.push(snapshot.clone());
        Ok(())
    }

    fn acknowledge(
        &mut self,
        expected_revision: u64,
        revision: u64,
        id: u64,
    ) -> Result<(), Self::Error> {
        self.log.borrow_mut().push("state-commit");
        let mut snapshot = self.snapshots.last().expect("snapshot before ack").clone();
        assert_eq!(snapshot.revision, expected_revision);
        snapshot.revision = revision;
        snapshot.outbox.remove(id).expect("valid outbox accounting");
        self.snapshots.push(snapshot);
        Ok(())
    }
}

struct Host {
    log: Rc<RefCell<Vec<&'static str>>>,
    accepted: BTreeMap<AppendKey, DurableAccepted>,
}

impl AppendHost for Host {
    type Error = &'static str;

    fn accepted(&mut self, key: &AppendKey) -> Result<Option<DurableAccepted>, Self::Error> {
        Ok(self.accepted.get(key).cloned())
    }

    fn allocate(&mut self, _draft: &ClaimDraft) -> Result<Allocation, Self::Error> {
        Ok(Allocation {
            stream: claim_stream(),
            seq: 0,
            prev: None,
            lamport: 1,
            shape: Shape::Log,
        })
    }

    fn signer(&self) -> Principal {
        Principal::from("node-principal")
    }

    fn sign(&mut self, _unsigned: &[u8]) -> Result<Vec<u8>, Self::Error> {
        Ok(vec![0xAA, 0x55])
    }

    fn verify(&mut self, op: &glade_discover_protocol::SignedOp) -> Result<bool, Self::Error> {
        Ok(op.envelope().origin == self.signer() && op.signature() == [0xAA, 0x55])
    }

    fn persist_accepted(
        &mut self,
        key: &AppendKey,
        draft: &ClaimDraft,
        canonical: &[u8],
    ) -> Result<(), Self::Error> {
        self.log.borrow_mut().push("append-persist");
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

struct Wire {
    log: Rc<RefCell<Vec<&'static str>>>,
    sent: Vec<(NodeId, WireMsg)>,
}

impl Transport for Wire {
    type Error = &'static str;

    fn send(&mut self, to: &NodeId, message: &WireMsg) -> Result<(), Self::Error> {
        self.log.borrow_mut().push("send");
        self.sent.push((to.clone(), message.clone()));
        Ok(())
    }
}

struct Effects {
    append: Host,
    wire: Wire,
}

impl EffectExecutor for Effects {
    type Error = &'static str;

    fn execute(&mut self, effect: &Effect) -> Result<Option<Event>, Self::Error> {
        match effect {
            Effect::Append {
                intent,
                slot,
                generation,
                draft,
            } => append::handle_append(&mut self.append, slot, *generation, intent, draft)
                .map(Some)
                .map_err(|_| "append failed"),
            Effect::Gossip { .. } => match dispatch_gossip(&mut self.wire, effect) {
                Some(GossipSend::Sent { .. }) => Ok(None),
                Some(GossipSend::Failed { .. }) => Err("transport failed"),
                None => unreachable!("matched gossip"),
            },
            Effect::Schedule { .. } | Effect::Reply { .. } | Effect::Teardown { .. } => Ok(None),
        }
    }
}

#[test]
fn advertise_to_exact_gossip_commits_each_durable_boundary_first() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let state = restore_fresh(
        config(),
        persisted_with_grant(),
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(1_000),
        },
    );
    let mut driver = NodeDriver::new_fresh(
        state,
        Store {
            log: Rc::clone(&log),
            snapshots: Vec::new(),
        },
        Effects {
            append: Host {
                log: Rc::clone(&log),
                accepted: BTreeMap::new(),
            },
            wire: Wire {
                log: Rc::clone(&log),
                sent: Vec::new(),
            },
        },
    )
    .expect("valid default capacity");

    let report = driver
        .handle(
            StepCtx {
                mono: MonoInstant(0),
                wall: WallMs(1_000),
            },
            advertise(),
        )
        .expect("advertise drive");
    assert!(report.failures.is_empty());
    let [accepted] = report.followups.try_into().expect("one accepted callback");
    let Event::OpAccepted { op, .. } = &accepted else {
        panic!("append must return OpAccepted");
    };
    let accepted_bytes = op.canonical_bytes().to_vec();

    assert_eq!(
        &*log.borrow(),
        &["state-commit", "append-persist", "state-commit"]
    );
    assert_eq!(
        driver
            .state()
            .persisted()
            .mine
            .get(&(slot(), Generation(1)))
            .expect("durable pending")
            .status,
        MineStatus::Pending
    );

    let report = driver
        .handle(
            StepCtx {
                mono: MonoInstant(1),
                wall: WallMs(1_001),
            },
            accepted,
        )
        .expect("accepted drive");
    assert!(report.followups.is_empty());
    assert!(report.failures.is_empty());
    assert_eq!(
        &*log.borrow(),
        &[
            "state-commit",
            "append-persist",
            "state-commit",
            "state-commit",
            "send",
            "state-commit",
        ]
    );

    let (store, effects) = driver.adapters();
    assert_eq!(store.snapshots.len(), 4);
    assert_eq!(
        store.snapshots[3]
            .persisted
            .mine
            .get(&(slot(), Generation(1)))
            .expect("accepted own claim")
            .status,
        MineStatus::Accepted
    );
    assert_eq!(effects.wire.sent.len(), 1);
    assert_eq!(effects.wire.sent[0].0, NodeId::from("node-b"));
    let WireMsg::DirOp { op: sent } = &effects.wire.sent[0].1 else {
        panic!("accepted claim must gossip as DirOp");
    };
    assert_eq!(sent.canonical_bytes(), accepted_bytes);
    assert!(
        store.snapshots[3]
            .persisted
            .retained
            .values()
            .flat_map(|records| records.values())
            .any(|retained| retained.canonical_bytes() == accepted_bytes)
    );

    let key = AppendKey {
        slot: slot(),
        generation: Generation(1),
        intent: IntentId::from("intent-a"),
    };
    assert_eq!(
        effects.append.accepted[&key].canonical, accepted_bytes,
        "durable append bytes, folded bytes, and transport bytes must be identical"
    );
    assert!(matches!(driver.state().clock(), ClockState::Ready { .. }));
}
