use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::rc::Rc;

use glade_discover_core::{
    ClockState, Effect, Event, KernelConfig, MineStatus, MonoInstant, OwnClaim, PersistedState,
    StepCtx, VerificationBatch, WakeToken, WallMs, WatermarkLoad, restore_fresh,
};
use glade_discover_protocol::{
    ClaimDraft, ClaimIdentity, Generation, GrantId, IntentId, MAX_RECORD_BYTES, NodeId, OpEnvelope,
    Principal, RecordId, Shape, Slot, StreamId, SyncId, WireMsg, encode_signed_op, record_id,
};

use glade_discover_node_adapter::driver::{
    DEFAULT_OUTBOX_BYTES, DriveError, DriveReport, DurableCommit, DurableOutbox, DurableSnapshot,
    EffectExecutor, EffectFailure, NodeDriver, OutboxError, OutboxLimits, RestoreError,
    SNAPSHOT_FORMAT_VERSION,
};

fn config() -> KernelConfig {
    KernelConfig {
        local_node: NodeId::from("node-a"),
        local_principal: Principal::from("principal-a"),
        peers: BTreeSet::from([NodeId::from("node-b")]),
        workspace_owner_roots: BTreeMap::new(),
        node_principal_bindings: BTreeSet::new(),
        skew_margin_ms: 5,
        max_lease_ms: 1_000,
        clock_resync_ms: 30,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(10).expect("non-zero"),
        gossip_fan: 1,
        max_retained_bytes: NonZeroU64::new(1_000_000).expect("non-zero"),
    }
}

fn config_with_retained_limit(limit: u64) -> KernelConfig {
    let mut value = config();
    value.max_retained_bytes = NonZeroU64::new(limit).expect("non-zero retained limit");
    value.peers.clear();
    value
}

fn state() -> glade_discover_core::State {
    restore_fresh(
        config(),
        PersistedState::default(),
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(1_000),
        },
    )
}

fn snapshot(persisted: PersistedState, outbox: DurableOutbox) -> DurableSnapshot {
    DurableSnapshot {
        format_version: SNAPSHOT_FORMAT_VERSION,
        revision: 0,
        persisted,
        watermark: Some(WallMs(1_000)),
        outbox,
    }
}

fn restore_ctx() -> StepCtx {
    StepCtx {
        mono: MonoInstant(8),
        wall: WallMs(1_000),
    }
}

fn pending_workspace_state(intent: &str, generation: u64) -> PersistedState {
    let slot = Slot::Workspace {
        share: "workspace".to_owned(),
    };
    let mut persisted = PersistedState::default();
    persisted.mine.insert(
        (slot, Generation(generation)),
        OwnClaim {
            intent: IntentId::from(intent),
            draft: ClaimDraft::Workspace {
                node: NodeId::from("node-a"),
                share: "workspace".to_owned(),
                identity: ClaimIdentity::Mint,
                grant_ref: GrantId::from(RecordId {
                    stream: StreamId {
                        share: "workspace".to_owned(),
                        glade_id: "directory".to_owned(),
                        key: vec![3],
                    },
                    origin: Principal::from("owner"),
                    seq: 0,
                }),
                lease_expiry_ms: 2_000,
                epoch: 0,
            },
            status: MineStatus::Pending,
        },
    );
    persisted
}

fn two_pending_workspace_claims() -> PersistedState {
    let mut persisted = pending_workspace_state("pending-first", 5);
    let second = pending_workspace_state("pending-second", 6)
        .mine
        .into_values()
        .next()
        .expect("second pending claim");
    persisted.mine.insert(
        (
            Slot::Workspace {
                share: "workspace-second".to_owned(),
            },
            Generation(6),
        ),
        second,
    );
    persisted
}

fn state_near_the_default_retained_ceiling() -> glade_discover_core::State {
    const RETAINED_LIMIT: u64 = 16 * 1024 * 1024;
    let mut persisted = PersistedState::default();
    for seq in 0_u64.. {
        let op = encode_signed_op(
            &OpEnvelope {
                stream: StreamId {
                    share: "workspace".to_owned(),
                    glade_id: "directory".to_owned(),
                    key: vec![7],
                },
                origin: Principal::from("principal-a"),
                seq,
                prev: None,
                lamport: seq,
                refs: Vec::new(),
                shape: Shape::Value,
                payload: vec![0x5a; 15_000],
            },
            &[0xaa],
        )
        .expect("bounded canonical op");
        let bytes = u64::try_from(op.canonical_bytes().len()).expect("bounded fixture");
        if persisted.retained_bytes + bytes > RETAINED_LIMIT {
            break;
        }
        persisted.retained_bytes += bytes;
        persisted
            .retained
            .entry(op.envelope().stream.clone())
            .or_default()
            .insert(record_id(&op), op);
    }
    assert!(RETAINED_LIMIT - persisted.retained_bytes < MAX_RECORD_BYTES as u64);
    let mut large_config = config();
    large_config.max_retained_bytes = NonZeroU64::new(RETAINED_LIMIT).expect("non-zero");
    restore_fresh(
        large_config,
        persisted,
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(1_000),
        },
    )
}

#[derive(Clone)]
struct Store {
    log: Rc<RefCell<Vec<&'static str>>>,
    fail: bool,
    committed: Option<DurableSnapshot>,
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
        self.log.borrow_mut().push("commit");
        if self.fail {
            return Err("commit failed");
        }
        if self.committed.as_ref().map(|current| current.revision) != expected_revision {
            return Err("revision mismatch");
        }
        self.committed = Some(snapshot.clone());
        Ok(())
    }

    fn acknowledge(
        &mut self,
        expected_revision: u64,
        revision: u64,
        id: u64,
    ) -> Result<(), Self::Error> {
        self.log.borrow_mut().push("commit");
        if self.fail {
            return Err("commit failed");
        }
        let snapshot = self.committed.as_mut().expect("snapshot before ack");
        if snapshot.revision != expected_revision {
            return Err("revision mismatch");
        }
        snapshot.revision = revision;
        snapshot.outbox.remove(id).expect("durable pending effect");
        Ok(())
    }
}

struct UnauthenticatedStore;

impl DurableCommit for UnauthenticatedStore {
    type Error = &'static str;

    fn authenticate(&mut self, _snapshot: &DurableSnapshot) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn commit(
        &mut self,
        _expected_revision: Option<u64>,
        _snapshot: &DurableSnapshot,
    ) -> Result<(), Self::Error> {
        panic!("unauthenticated snapshot must not commit")
    }

    fn acknowledge(
        &mut self,
        _expected_revision: u64,
        _revision: u64,
        _id: u64,
    ) -> Result<(), Self::Error> {
        panic!("unauthenticated snapshot must not execute or acknowledge")
    }
}

struct FailSelectedAckStore {
    inner: Store,
    fail_id: Option<u64>,
}

impl DurableCommit for FailSelectedAckStore {
    type Error = &'static str;

    fn authenticate(&mut self, snapshot: &DurableSnapshot) -> Result<bool, Self::Error> {
        self.inner.authenticate(snapshot)
    }

    fn commit(
        &mut self,
        expected_revision: Option<u64>,
        snapshot: &DurableSnapshot,
    ) -> Result<(), Self::Error> {
        self.inner.commit(expected_revision, snapshot)
    }

    fn acknowledge(
        &mut self,
        expected_revision: u64,
        revision: u64,
        id: u64,
    ) -> Result<(), Self::Error> {
        if self.fail_id == Some(id) {
            self.inner.log.borrow_mut().push("commit");
            return Err("selected ack failed");
        }
        self.inner.acknowledge(expected_revision, revision, id)
    }
}

struct Executor {
    log: Rc<RefCell<Vec<&'static str>>>,
    fail_on: Option<&'static str>,
    followup_on: Option<&'static str>,
}

impl EffectExecutor for Executor {
    type Error = &'static str;

    fn execute(&mut self, effect: &Effect) -> Result<Option<Event>, Self::Error> {
        let kind = match effect {
            Effect::Gossip { .. } => "gossip",
            Effect::Schedule { .. } => "schedule",
            Effect::Append { .. } => "append",
            Effect::Reply { .. } => "reply",
            Effect::Teardown { .. } => "teardown",
        };
        self.log.borrow_mut().push(kind);
        if self.fail_on == Some(kind) {
            Err("effect failed")
        } else if self.followup_on == Some(kind) {
            Ok(Some(Event::ClockReseed {
                watermark: WallMs(9_999),
            }))
        } else {
            Ok(None)
        }
    }
}

fn gossip_tick() -> Event {
    Event::Wakeup {
        token: WakeToken::GossipTick {
            peer: NodeId::from("node-b"),
        },
    }
}

#[test]
fn durable_state_and_watermark_commit_before_dependent_effects() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let store = Store {
        log: Rc::clone(&log),
        fail: false,
        committed: None,
    };
    let executor = Executor {
        log: Rc::clone(&log),
        fail_on: None,
        followup_on: None,
    };
    let mut driver = NodeDriver::new_fresh(state(), store, executor).expect("valid capacity");

    let report = driver
        .handle(
            StepCtx {
                mono: MonoInstant(1),
                wall: WallMs(1_001),
            },
            gossip_tick(),
        )
        .expect("drive event");

    assert_eq!(
        report,
        DriveReport {
            followups: Vec::new(),
            failures: Vec::new(),
        }
    );
    assert_eq!(
        &*log.borrow(),
        &["commit", "gossip", "commit", "schedule", "commit"]
    );
    assert_eq!(driver.state().persisted().next_sync, 1);
    let (store, _) = driver.adapters();
    let snapshot = store.committed.as_ref().expect("durable snapshot");
    assert_eq!(snapshot.persisted.next_sync, 1);
    assert_eq!(snapshot.watermark, Some(WallMs(1_001)));
    assert!(snapshot.outbox.pending.is_empty());
}

#[test]
fn failed_commit_leaves_state_unchanged_and_executes_no_effect() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut driver = NodeDriver::new_fresh(
        state(),
        Store {
            log: Rc::clone(&log),
            fail: true,
            committed: None,
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    )
    .expect("valid capacity");

    let error = driver
        .handle(
            StepCtx {
                mono: MonoInstant(1),
                wall: WallMs(1_001),
            },
            gossip_tick(),
        )
        .expect_err("commit must fail");

    assert_eq!(error, DriveError::Commit("commit failed"));
    assert_eq!(&*log.borrow(), &["commit"]);
    assert_eq!(driver.state().persisted().next_sync, 0);
}

#[test]
fn fresh_create_does_not_overwrite_an_existing_revision_zero_snapshot() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let existing = snapshot(PersistedState::default(), DurableOutbox::default());
    let mut driver = NodeDriver::new_fresh(
        state(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(existing.clone()),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    )
    .expect("valid capacity");

    let error = driver
        .handle(
            StepCtx {
                mono: MonoInstant(1),
                wall: WallMs(1_001),
            },
            gossip_tick(),
        )
        .expect_err("create-if-absent must reject an existing record");

    assert_eq!(error, DriveError::Commit("revision mismatch"));
    assert_eq!(&*log.borrow(), &["commit"]);
    assert_eq!(driver.state().persisted().next_sync, 0);
    assert_eq!(driver.adapters().0.committed, Some(existing));
}

#[test]
fn failed_gossip_does_not_rollback_or_skip_successor_schedule_and_followup() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut driver = NodeDriver::new_fresh(
        state(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: None,
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: Some("gossip"),
            followup_on: Some("schedule"),
        },
    )
    .expect("valid capacity");

    let report = driver
        .handle(
            StepCtx {
                mono: MonoInstant(1),
                wall: WallMs(1_001),
            },
            gossip_tick(),
        )
        .expect("commit succeeds despite effect failure");

    assert_eq!(
        report,
        DriveReport {
            followups: vec![Event::ClockReseed {
                watermark: WallMs(9_999),
            }],
            failures: vec![EffectFailure {
                id: 0,
                effect: Effect::Gossip {
                    to: NodeId::from("node-b"),
                    msg: Box::new(WireMsg::SyncStart {
                        sync_id: SyncId::from("node-a:0"),
                        heads: Vec::new(),
                    }),
                },
                source: "effect failed",
            }],
        }
    );
    assert_eq!(&*log.borrow(), &["commit", "gossip", "schedule", "commit"]);
    assert_eq!(driver.state().persisted().next_sync, 1);
    let (store, _) = driver.adapters();
    assert_eq!(
        store
            .committed
            .as_ref()
            .expect("durable snapshot")
            .persisted
            .next_sync,
        1
    );
    assert_eq!(
        store
            .committed
            .as_ref()
            .expect("durable snapshot")
            .outbox
            .pending
            .len(),
        1
    );
}

#[test]
fn failed_teardown_survives_restart_and_is_acknowledged_after_retry() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let effect = Effect::Teardown {
        slot: Slot::Binding {
            share: "svc".to_owned(),
            glade_id: "cache".to_owned(),
            key: vec![1],
        },
        generation: Generation(7),
    };
    let outbox = DurableOutbox::from_effects(vec![effect.clone()]).expect("bounded outbox");
    let loaded = snapshot(PersistedState::default(), outbox);
    let mut driver = NodeDriver::from_snapshot(
        config_with_retained_limit(1),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits::default(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: Some("teardown"),
            followup_on: None,
        },
    )
    .expect("restore commits the loaded outbox");

    let failed = driver.retry_pending().expect("report failed teardown");
    assert_eq!(failed.failures.len(), 1);
    assert_eq!(driver.outbox().pending.len(), 1);

    driver.adapters_mut().1.fail_on = None;
    let report = driver.retry_pending().expect("retry pending teardown");

    assert!(report.failures.is_empty());
    assert_eq!(
        &*log.borrow(),
        &["commit", "teardown", "teardown", "commit"]
    );
    let (store, _) = driver.adapters();
    assert!(
        store
            .committed
            .as_ref()
            .expect("durable snapshot")
            .outbox
            .pending
            .is_empty()
    );
}

#[test]
fn driver_owned_wall_schedule_installs_forward_clock_before_timer_effect() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut driver = NodeDriver::new_fresh(
        state(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: None,
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    )
    .expect("valid capacity");

    driver
        .schedule_wall(
            StepCtx {
                mono: MonoInstant(20),
                wall: WallMs(1_200),
            },
            WallMs(1_300),
            WakeToken::GossipTick {
                peer: NodeId::from("node-b"),
            },
        )
        .expect("durable wall schedule");

    assert_eq!(
        driver.state().clock(),
        ClockState::Ready {
            watermark: WallMs(1_200)
        }
    );
    assert_eq!(&*log.borrow(), &["commit", "schedule", "commit"]);
    let (store, _) = driver.adapters();
    assert_eq!(
        store
            .committed
            .as_ref()
            .expect("durable snapshot")
            .watermark,
        Some(WallMs(1_200))
    );
}

#[test]
fn restore_commits_kernel_recovery_before_the_schedule_executor_runs() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let slot = Slot::Workspace {
        share: "workspace".to_owned(),
    };
    let intent = IntentId::from("pending");
    let mut persisted = PersistedState::default();
    persisted.mine.insert(
        (slot.clone(), Generation(3)),
        OwnClaim {
            intent: intent.clone(),
            draft: ClaimDraft::Workspace {
                node: NodeId::from("node-a"),
                share: "workspace".to_owned(),
                identity: ClaimIdentity::Mint,
                grant_ref: GrantId::from(RecordId {
                    stream: StreamId {
                        share: "workspace".to_owned(),
                        glade_id: "directory".to_owned(),
                        key: vec![1],
                    },
                    origin: Principal::from("owner"),
                    seq: 0,
                }),
                lease_expiry_ms: 2_000,
                epoch: 0,
            },
            status: MineStatus::Pending,
        },
    );
    let loaded = snapshot(persisted, DurableOutbox::default());
    let mut driver = NodeDriver::from_snapshot(
        config(),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits::default(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    )
    .expect("restore durably merges recovery");

    assert_eq!(&*log.borrow(), &["commit"]);
    assert_eq!(driver.outbox().pending.len(), 1);
    driver.retry_pending().expect("execute recovery schedule");
    assert_eq!(&*log.borrow(), &["commit", "schedule", "commit"]);
    assert!(driver.outbox().pending.is_empty());
}

#[test]
fn acknowledgement_failure_keeps_effect_replayable_until_ack_commits() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let effect = Effect::Teardown {
        slot: Slot::Workspace {
            share: "workspace".to_owned(),
        },
        generation: Generation(9),
    };
    let loaded = snapshot(
        PersistedState::default(),
        DurableOutbox::from_effects(vec![effect]).expect("outbox"),
    );
    let mut driver = NodeDriver::from_snapshot(
        config(),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits::default(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    )
    .expect("restore outbox");
    driver.adapters_mut().0.fail = true;

    assert_eq!(
        driver.retry_pending(),
        Err(DriveError::Acknowledge {
            id: 0,
            source: "commit failed"
        })
    );
    assert_eq!(driver.outbox().pending.len(), 1);

    driver.adapters_mut().0.fail = false;
    driver.retry_pending().expect("at-least-once retry");
    assert!(driver.outbox().pending.is_empty());
    assert_eq!(
        &*log.borrow(),
        &["commit", "teardown", "commit", "teardown", "commit"]
    );
}

#[test]
fn insufficient_sync_entry_capacity_is_rejected_before_driver_creation() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let result = NodeDriver::new_fresh_with_limit(
        state(),
        1,
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: None,
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(
        result,
        Err(OutboxError::InsufficientSyncCapacity {
            normal_entries: 1,
            ..
        })
    ));
    assert!(log.borrow().is_empty());
}

#[test]
fn insufficient_sync_byte_capacity_is_rejected_before_driver_creation() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let result = NodeDriver::new_fresh_with_limits(
        state(),
        OutboxLimits {
            normal_entries: 8,
            total_entries: 9,
            normal_bytes: 1,
            total_bytes: 2,
        },
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: None,
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(
        result,
        Err(OutboxError::InsufficientSyncCapacity {
            normal_bytes: 1,
            ..
        })
    ));
    assert!(log.borrow().is_empty());
}

#[test]
fn restore_rejects_config_downsize_that_cannot_admit_authenticated_retained_set() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let large_state = state_near_the_default_retained_ceiling();
    let loaded = snapshot(large_state.persisted().clone(), DurableOutbox::default());

    let result = NodeDriver::from_snapshot(
        config(),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits {
            normal_entries: 1_000,
            total_entries: 1_256,
            normal_bytes: 2_000_000,
            total_bytes: 3_000_000,
        },
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(
        result,
        Err(RestoreError::Outbox(
            OutboxError::InsufficientSyncCapacity { .. }
        ))
    ));
    assert!(log.borrow().is_empty());
}

#[test]
fn restore_rejects_authenticated_retained_byte_count_mismatch_before_effects_execute() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let persisted = PersistedState {
        retained_bytes: 1,
        ..PersistedState::default()
    };
    let loaded = snapshot(
        persisted,
        DurableOutbox::from_effects(vec![Effect::Teardown {
            slot: Slot::Workspace {
                share: "must-not-run".to_owned(),
            },
            generation: Generation(1),
        }])
        .expect("loaded outbox"),
    );

    let result = NodeDriver::from_snapshot(
        config(),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits::default(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(
        result,
        Err(RestoreError::Outbox(OutboxError::InvalidRetainedByteCount))
    ));
    assert!(log.borrow().is_empty());
}

#[test]
fn oversized_configured_node_id_is_rejected_before_driver_creation() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut oversized = config();
    oversized.peers = BTreeSet::from([NodeId::from("p".repeat(236))]);
    let state = restore_fresh(
        oversized,
        PersistedState::default(),
        WatermarkLoad::Readable(WallMs(1_000)),
        StepCtx {
            mono: MonoInstant(0),
            wall: WallMs(1_000),
        },
    );

    let result = NodeDriver::new_fresh(
        state,
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: None,
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(result, Err(OutboxError::NodeIdTooLarge)));
    assert!(log.borrow().is_empty());
}

#[test]
fn full_loaded_outbox_is_drained_until_kernel_recovery_fits() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let slot = Slot::Workspace {
        share: "workspace".to_owned(),
    };
    let intent = IntentId::from("pending-full");
    let mut persisted = PersistedState::default();
    persisted.mine.insert(
        (slot, Generation(4)),
        OwnClaim {
            intent,
            draft: ClaimDraft::Workspace {
                node: NodeId::from("node-a"),
                share: "workspace".to_owned(),
                identity: ClaimIdentity::Mint,
                grant_ref: GrantId::from(RecordId {
                    stream: StreamId {
                        share: "workspace".to_owned(),
                        glade_id: "directory".to_owned(),
                        key: vec![2],
                    },
                    origin: Principal::from("owner"),
                    seq: 0,
                }),
                lease_expiry_ms: 2_000,
                epoch: 0,
            },
            status: MineStatus::Pending,
        },
    );
    let loaded = snapshot(
        persisted,
        DurableOutbox::from_effects(vec![Effect::Teardown {
            slot: Slot::Workspace {
                share: "old".to_owned(),
            },
            generation: Generation(1),
        }])
        .expect("loaded outbox"),
    );
    let limits = OutboxLimits {
        normal_entries: 1,
        total_entries: 1,
        normal_bytes: 1_000_000,
        total_bytes: 1_000_000,
    };
    let mut driver = NodeDriver::from_snapshot(
        config_with_retained_limit(1),
        restore_ctx(),
        loaded.clone(),
        limits,
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    )
    .expect("restore drains authenticated backlog to make recovery capacity");

    assert_eq!(&*log.borrow(), &["teardown", "commit", "commit"]);
    assert_eq!(driver.outbox().pending.len(), 1);
    assert!(matches!(
        driver.outbox().pending.values().next(),
        Some(Effect::Schedule {
            token: WakeToken::AppendRetry { .. },
            ..
        })
    ));
    driver.retry_pending().expect("execute reconstructed retry");
    assert!(driver.outbox().pending.is_empty());
}

#[test]
fn recovery_that_cannot_fit_an_empty_outbox_reports_the_capacity_cause() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let loaded = snapshot(two_pending_workspace_claims(), DurableOutbox::default());

    let result = NodeDriver::from_snapshot(
        config_with_retained_limit(1),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits {
            normal_entries: 1,
            total_entries: 1,
            normal_bytes: 1_000_000,
            total_bytes: 1_000_000,
        },
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(
        result,
        Err(RestoreError::RecoveryBlocked {
            capacity: OutboxError::Full {
                limit: 1,
                pending: 0,
                additional: 2,
            },
            failures,
        }) if failures.is_empty()
    ));
    assert!(log.borrow().is_empty());
}

#[test]
fn revision_exhaustion_prevents_recovery_backlog_execution() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut loaded = snapshot(
        pending_workspace_state("pending-revision-exhausted", 6),
        DurableOutbox::from_effects(vec![Effect::Teardown {
            slot: Slot::Workspace {
                share: "old".to_owned(),
            },
            generation: Generation(1),
        }])
        .expect("loaded outbox"),
    );
    loaded.revision = u64::MAX;

    let result = NodeDriver::from_snapshot(
        config_with_retained_limit(1),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits {
            normal_entries: 1,
            total_entries: 1,
            normal_bytes: 1_000_000,
            total_bytes: 1_000_000,
        },
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(result, Err(RestoreError::RevisionExhausted)));
    assert!(log.borrow().is_empty());
}

#[test]
fn unauthenticated_atomic_snapshot_cannot_execute_loaded_effects() {
    let loaded = snapshot(
        PersistedState::default(),
        DurableOutbox::from_effects(vec![Effect::Teardown {
            slot: Slot::Workspace {
                share: "forged".to_owned(),
            },
            generation: Generation(99),
        }])
        .expect("forged outbox"),
    );
    let log = Rc::new(RefCell::new(Vec::new()));

    let result = NodeDriver::from_snapshot(
        config(),
        restore_ctx(),
        loaded,
        OutboxLimits::default(),
        UnauthenticatedStore,
        Executor {
            log: Rc::clone(&log),
            fail_on: None,
            followup_on: None,
        },
    );

    assert!(matches!(result, Err(RestoreError::Unauthenticated)));
    assert!(log.borrow().is_empty());
}

#[test]
fn loaded_monotonic_schedule_is_rebased_to_the_new_process_epoch() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let loaded = snapshot(
        PersistedState::default(),
        DurableOutbox::from_effects(vec![Effect::Schedule {
            token: WakeToken::GossipTick {
                peer: NodeId::from("node-b"),
            },
            at_mono: MonoInstant(u64::MAX),
        }])
        .expect("old schedule"),
    );

    let driver = NodeDriver::from_snapshot(
        config(),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits::default(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: Some(loaded),
        },
        Executor {
            log,
            fail_on: None,
            followup_on: None,
        },
    )
    .expect("authenticated restore");

    assert!(matches!(
        driver.outbox().pending.values().next(),
        Some(Effect::Schedule {
            at_mono: MonoInstant(8),
            ..
        })
    ));
}

#[test]
fn earlier_followup_survives_a_later_acknowledgement_failure() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let loaded = snapshot(
        PersistedState::default(),
        DurableOutbox::from_effects(vec![
            Effect::Schedule {
                token: WakeToken::GossipTick {
                    peer: NodeId::from("node-b"),
                },
                at_mono: MonoInstant(5),
            },
            Effect::Teardown {
                slot: Slot::Workspace {
                    share: "old".to_owned(),
                },
                generation: Generation(1),
            },
        ])
        .expect("two effects"),
    );
    let mut driver = NodeDriver::from_snapshot(
        config(),
        restore_ctx(),
        loaded.clone(),
        OutboxLimits::default(),
        FailSelectedAckStore {
            inner: Store {
                log: Rc::clone(&log),
                fail: false,
                committed: Some(loaded),
            },
            fail_id: Some(1),
        },
        Executor {
            log,
            fail_on: None,
            followup_on: Some("schedule"),
        },
    )
    .expect("authenticated restore");

    assert_eq!(
        driver.retry_pending(),
        Err(DriveError::Acknowledge {
            id: 1,
            source: "selected ack failed",
        })
    );
    driver.adapters_mut().0.fail_id = None;
    let report = driver.retry_pending().expect("retry later effect");

    assert_eq!(
        report.followups,
        vec![Event::ClockReseed {
            watermark: WallMs(9_999),
        }]
    );
    assert!(driver.outbox().pending.is_empty());
}

#[test]
fn inverted_entry_or_byte_limits_are_rejected_before_driver_creation() {
    let log = Rc::new(RefCell::new(Vec::new()));
    for limits in [
        OutboxLimits {
            normal_entries: 2,
            total_entries: 1,
            normal_bytes: 10,
            total_bytes: 10,
        },
        OutboxLimits {
            normal_entries: 1,
            total_entries: 1,
            normal_bytes: 11,
            total_bytes: 10,
        },
    ] {
        let result = NodeDriver::new_fresh_with_limits(
            state(),
            limits,
            Store {
                log: Rc::clone(&log),
                fail: false,
                committed: None,
            },
            Executor {
                log: Rc::clone(&log),
                fail_on: None,
                followup_on: None,
            },
        );
        assert!(matches!(result, Err(OutboxError::InvalidLimits)));
    }
    assert!(log.borrow().is_empty());
}

#[test]
fn stable_effect_sizing_uses_fixed_width_for_numeric_fields() {
    let effect = |at_mono| Effect::Schedule {
        token: WakeToken::GossipTick {
            peer: NodeId::from("node-b"),
        },
        at_mono: MonoInstant(at_mono),
    };
    let single_digit = DurableOutbox::from_effects(vec![effect(9)]).expect("outbox");
    let maximum = DurableOutbox::from_effects(vec![effect(u64::MAX)]).expect("outbox");

    assert_eq!(single_digit.pending_bytes, maximum.pending_bytes);
}

#[test]
fn default_outbox_admits_a_sync_response_near_the_retained_ceiling() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut driver = NodeDriver::new_fresh(
        state_near_the_default_retained_ceiling(),
        Store {
            log: Rc::clone(&log),
            fail: false,
            committed: None,
        },
        Executor {
            log,
            fail_on: Some("gossip"),
            followup_on: None,
        },
    )
    .expect("valid default capacity");

    let report = driver
        .handle(
            StepCtx {
                mono: MonoInstant(1),
                wall: WallMs(1_001),
            },
            Event::Deliver {
                from: NodeId::from("node-b"),
                msg: Box::new(WireMsg::SyncStart {
                    sync_id: SyncId::from("full-store-round"),
                    heads: Vec::new(),
                }),
                verification: VerificationBatch::default(),
            },
        )
        .expect("aggregate-bounded response commits before delivery");

    assert!(!report.failures.is_empty());
    assert!(driver.outbox().pending_bytes <= DEFAULT_OUTBOX_BYTES);
    assert!(driver.outbox().pending_bytes > 0);
    assert!(driver.outbox().pending.values().any(|effect| matches!(
        effect,
        Effect::Gossip { msg, .. } if matches!(msg.as_ref(), WireMsg::SyncEnd { .. })
    )));
}
