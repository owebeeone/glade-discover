use std::collections::BTreeMap;

use glade_discover_core::{
    ClockState, Effect, Event, KernelConfig, MonoInstant, PersistedState, RouteAns, State, StepCtx,
    WakeToken, WallMs, WallScheduleError, WatermarkLoad, prepare_wall_schedule,
    restore_with_recovery, step,
};
use glade_discover_protocol::{
    ClaimDraft, ClaimIdentity, MAX_RECORD_BYTES, MAX_SYNC_ID_BYTES, MIN_SIGNED_OP_BYTES, RecordId,
    Slot, encode_wire_msg,
};

pub const SNAPSHOT_FORMAT_VERSION: u16 = 1;
pub const DEFAULT_OUTBOX_LIMIT: usize = 16_384;
pub const DEFAULT_OUTBOX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_CONFIG_NODE_ID_BYTES: usize = MAX_SYNC_ID_BYTES - 21;
const MIN_OPS_PER_SYNC_CHUNK: u64 = 63;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutboxLimits {
    pub normal_entries: usize,
    pub total_entries: usize,
    pub normal_bytes: u64,
    pub total_bytes: u64,
}

impl Default for OutboxLimits {
    fn default() -> Self {
        Self::with_normal_limit(DEFAULT_OUTBOX_LIMIT)
    }
}

impl OutboxLimits {
    #[must_use]
    pub const fn with_normal_limit(normal_entries: usize) -> Self {
        Self {
            normal_entries,
            total_entries: normal_entries.saturating_add(256),
            normal_bytes: DEFAULT_OUTBOX_BYTES,
            total_bytes: DEFAULT_OUTBOX_BYTES + 4 * 1024 * 1024,
        }
    }

    fn validate(self, config: &KernelConfig, retained_bytes: u64) -> Result<Self, OutboxError> {
        if self.normal_entries > self.total_entries || self.normal_bytes > self.total_bytes {
            return Err(OutboxError::InvalidLimits);
        }
        if config.local_node.as_str().len() > MAX_CONFIG_NODE_ID_BYTES
            || config
                .peers
                .iter()
                .any(|peer| peer.as_str().len() > MAX_CONFIG_NODE_ID_BYTES)
        {
            return Err(OutboxError::NodeIdTooLarge);
        }
        let (required_entries, required_bytes) = required_sync_capacity(config, retained_bytes)?;
        if self.normal_entries < required_entries || self.normal_bytes < required_bytes {
            return Err(OutboxError::InsufficientSyncCapacity {
                required_entries,
                required_bytes,
                normal_entries: self.normal_entries,
                normal_bytes: self.normal_bytes,
            });
        }
        Ok(self)
    }
}

/// One authenticated atomic durable snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableSnapshot {
    pub format_version: u16,
    pub revision: u64,
    pub persisted: PersistedState,
    pub watermark: Option<WallMs>,
    pub outbox: DurableOutbox,
}

/// A durable, monotonically identified and byte-accounted effect outbox.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DurableOutbox {
    pub next_id: u64,
    pub pending_bytes: u64,
    pub pending: BTreeMap<u64, Effect>,
}

impl DurableOutbox {
    pub fn from_effects(effects: Vec<Effect>) -> Result<Self, OutboxError> {
        let mut outbox = Self::default();
        outbox.enqueue(effects, usize::MAX, u64::MAX)?;
        Ok(outbox)
    }

    pub fn remove(&mut self, id: u64) -> Result<Option<Effect>, OutboxError> {
        let Some(effect) = self.pending.remove(&id) else {
            return Ok(None);
        };
        let bytes = effect_bytes(&effect)?;
        self.pending_bytes = self
            .pending_bytes
            .checked_sub(bytes)
            .ok_or(OutboxError::InvalidByteCount)?;
        Ok(Some(effect))
    }

    fn validate(&self, entry_limit: usize, byte_limit: u64) -> Result<(), OutboxError> {
        if self.pending.len() > entry_limit {
            return Err(OutboxError::Full {
                limit: entry_limit,
                pending: self.pending.len(),
                additional: 0,
            });
        }
        if self
            .pending
            .last_key_value()
            .is_some_and(|(id, _)| *id >= self.next_id)
        {
            return Err(OutboxError::InvalidNextId);
        }
        let actual = effect_set_bytes(self.pending.values())?;
        if actual != self.pending_bytes {
            return Err(OutboxError::InvalidByteCount);
        }
        if actual > byte_limit {
            return Err(OutboxError::BytesFull {
                limit: byte_limit,
                pending: actual,
                additional: 0,
            });
        }
        Ok(())
    }

    fn enqueue(
        &mut self,
        effects: Vec<Effect>,
        entry_limit: usize,
        byte_limit: u64,
    ) -> Result<(), OutboxError> {
        let target = self
            .pending
            .len()
            .checked_add(effects.len())
            .ok_or(OutboxError::Full {
                limit: entry_limit,
                pending: self.pending.len(),
                additional: effects.len(),
            })?;
        if target > entry_limit {
            return Err(OutboxError::Full {
                limit: entry_limit,
                pending: self.pending.len(),
                additional: effects.len(),
            });
        }
        let additional_bytes = effect_set_bytes(effects.iter())?;
        let target_bytes = self
            .pending_bytes
            .checked_add(additional_bytes)
            .ok_or(OutboxError::SizeOverflow)?;
        if target_bytes > byte_limit {
            return Err(OutboxError::BytesFull {
                limit: byte_limit,
                pending: self.pending_bytes,
                additional: additional_bytes,
            });
        }
        let additional = u64::try_from(effects.len()).map_err(|_| OutboxError::IdExhausted)?;
        self.next_id
            .checked_add(additional)
            .ok_or(OutboxError::IdExhausted)?;
        for effect in effects {
            let id = self.next_id;
            self.next_id = self
                .next_id
                .checked_add(1)
                .ok_or(OutboxError::IdExhausted)?;
            self.pending.insert(id, effect);
        }
        self.pending_bytes = target_bytes;
        Ok(())
    }

    fn merge_recovery(
        &mut self,
        recovery: &[Effect],
        limits: OutboxLimits,
    ) -> Result<(), OutboxError> {
        let missing = recovery
            .iter()
            .filter(|effect| !self.pending.values().any(|pending| pending == *effect))
            .cloned()
            .collect();
        self.enqueue(missing, limits.total_entries, limits.total_bytes)
    }

    fn rebase_schedules(&mut self, now: MonoInstant) -> Result<(), OutboxError> {
        for effect in self.pending.values_mut() {
            if let Effect::Schedule { at_mono, .. } = effect {
                *at_mono = now;
            }
        }
        self.pending_bytes = effect_set_bytes(self.pending.values())?;
        Ok(())
    }
}

/// Trusted authenticated snapshot storage with revision-checked atomic writes.
pub trait DurableCommit {
    type Error;

    /// MUST authenticate and integrity-check the whole snapshot loaded from the
    /// same atomic record before any contained effect can execute.
    fn authenticate(&mut self, snapshot: &DurableSnapshot) -> Result<bool, Self::Error>;

    /// MUST atomically write the whole record. `None` means create only if the
    /// record is absent; `Some(revision)` means compare-and-swap that revision.
    fn commit(
        &mut self,
        expected_revision: Option<u64>,
        snapshot: &DurableSnapshot,
    ) -> Result<(), Self::Error>;

    /// MUST atomically compare the revision and remove exactly one outbox id.
    fn acknowledge(
        &mut self,
        expected_revision: u64,
        revision: u64,
        id: u64,
    ) -> Result<(), Self::Error>;
}

/// Executes one authenticated trusted-node effect after durable enqueue.
///
/// Implementations MUST tolerate at-least-once execution. `Append` is keyed by
/// `(slot,generation,intent)`, `Teardown` by `(slot,generation)`, replies by
/// `(ingress,corr)`, and wake tokens are stale-safe.
pub trait EffectExecutor {
    type Error;

    fn execute(&mut self, effect: &Effect) -> Result<Option<Event>, Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboxError {
    Full {
        limit: usize,
        pending: usize,
        additional: usize,
    },
    BytesFull {
        limit: u64,
        pending: u64,
        additional: u64,
    },
    IdExhausted,
    SizeOverflow,
    InvalidNextId,
    InvalidByteCount,
    InvalidRetainedByteCount,
    RetainedRecordTooLarge,
    InvalidLimits,
    NodeIdTooLarge,
    InsufficientSyncCapacity {
        required_entries: usize,
        required_bytes: u64,
        normal_entries: usize,
        normal_bytes: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriveError<CommitError> {
    Commit(CommitError),
    Acknowledge { id: u64, source: CommitError },
    Outbox(OutboxError),
    Clock(WallScheduleError),
    RevisionExhausted,
    MissingRevision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestoreError<CommitError, EffectError> {
    Store(CommitError),
    Unauthenticated,
    UnsupportedFormat(u16),
    Outbox(OutboxError),
    RevisionExhausted,
    RecoveryBlocked {
        capacity: OutboxError,
        failures: Vec<EffectFailure<EffectError>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectFailure<EffectError> {
    pub id: u64,
    pub effect: Effect,
    pub source: EffectError,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriveReport<EffectError> {
    pub followups: Vec<Event>,
    pub failures: Vec<EffectFailure<EffectError>>,
}

pub struct NodeDriver<Commit, Executor> {
    state: State,
    revision: Option<u64>,
    outbox: DurableOutbox,
    limits: OutboxLimits,
    bootstrap_followups: Vec<Event>,
    commit: Commit,
    executor: Executor,
}

impl<Commit, Executor> NodeDriver<Commit, Executor>
where
    Commit: DurableCommit,
    Executor: EffectExecutor,
{
    pub fn new_fresh(
        state: State,
        commit: Commit,
        executor: Executor,
    ) -> Result<Self, OutboxError> {
        Self::new_fresh_with_limits(state, OutboxLimits::default(), commit, executor)
    }

    pub fn new_fresh_with_limit(
        state: State,
        outbox_limit: usize,
        commit: Commit,
        executor: Executor,
    ) -> Result<Self, OutboxError> {
        Self::new_fresh_with_limits(
            state,
            OutboxLimits::with_normal_limit(outbox_limit),
            commit,
            executor,
        )
    }

    pub fn new_fresh_with_limits(
        state: State,
        limits: OutboxLimits,
        commit: Commit,
        executor: Executor,
    ) -> Result<Self, OutboxError> {
        let retained_bytes = validated_retained_bytes(state.persisted())?;
        let limits = limits.validate(state.config(), retained_bytes)?;
        Ok(Self {
            state,
            revision: None,
            outbox: DurableOutbox::default(),
            limits,
            bootstrap_followups: Vec::new(),
            commit,
            executor,
        })
    }

    pub fn from_snapshot(
        config: KernelConfig,
        ctx: StepCtx,
        mut loaded: DurableSnapshot,
        limits: OutboxLimits,
        mut commit: Commit,
        mut executor: Executor,
    ) -> Result<Self, RestoreError<Commit::Error, Executor::Error>> {
        if loaded.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(RestoreError::UnsupportedFormat(loaded.format_version));
        }
        if !commit.authenticate(&loaded).map_err(RestoreError::Store)? {
            return Err(RestoreError::Unauthenticated);
        }
        let retained_bytes =
            validated_retained_bytes(&loaded.persisted).map_err(RestoreError::Outbox)?;
        let limits = limits
            .validate(&config, retained_bytes)
            .map_err(RestoreError::Outbox)?;
        loaded
            .outbox
            .validate(limits.total_entries, limits.total_bytes)
            .map_err(RestoreError::Outbox)?;
        loaded
            .outbox
            .rebase_schedules(ctx.mono)
            .map_err(RestoreError::Outbox)?;
        loaded
            .outbox
            .validate(limits.total_entries, limits.total_bytes)
            .map_err(RestoreError::Outbox)?;
        let watermark_load = loaded
            .watermark
            .map_or(WatermarkLoad::Unreadable, WatermarkLoad::Readable);
        let (state, recovery) =
            restore_with_recovery(config, loaded.persisted.clone(), watermark_load, ctx);
        let mut bootstrap_followups = Vec::new();
        loop {
            let mut merged = loaded.outbox.clone();
            match merged.merge_recovery(&recovery, limits) {
                Ok(()) => {
                    loaded.outbox = merged;
                    break;
                }
                Err(capacity @ (OutboxError::Full { .. } | OutboxError::BytesFull { .. })) => {
                    let entries = loaded
                        .outbox
                        .pending
                        .iter()
                        .map(|(id, effect)| (*id, effect.clone()))
                        .collect::<Vec<_>>();
                    let mut failures = Vec::new();
                    let mut drained = false;
                    for (id, effect) in entries {
                        let revision = loaded
                            .revision
                            .checked_add(1)
                            .ok_or(RestoreError::RevisionExhausted)?;
                        match executor.execute(&effect) {
                            Ok(followup) => {
                                commit
                                    .acknowledge(loaded.revision, revision, id)
                                    .map_err(RestoreError::Store)?;
                                loaded.revision = revision;
                                loaded.outbox.remove(id).map_err(RestoreError::Outbox)?;
                                if let Some(event) = followup {
                                    bootstrap_followups.push(event);
                                }
                                drained = true;
                                break;
                            }
                            Err(source) => failures.push(EffectFailure { id, effect, source }),
                        }
                    }
                    if !drained {
                        return Err(RestoreError::RecoveryBlocked { capacity, failures });
                    }
                }
                Err(error) => return Err(RestoreError::Outbox(error)),
            }
        }
        let revision = loaded
            .revision
            .checked_add(1)
            .ok_or(RestoreError::RevisionExhausted)?;
        let snapshot = DurableSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            revision,
            persisted: state.persisted().clone(),
            watermark: durable_watermark(state.clock()),
            outbox: loaded.outbox.clone(),
        };
        commit
            .commit(Some(loaded.revision), &snapshot)
            .map_err(RestoreError::Store)?;
        Ok(Self {
            state,
            revision: Some(revision),
            outbox: loaded.outbox,
            limits,
            bootstrap_followups,
            commit,
            executor,
        })
    }

    #[must_use]
    pub const fn state(&self) -> &State {
        &self.state
    }

    #[must_use]
    pub const fn outbox(&self) -> &DurableOutbox {
        &self.outbox
    }

    #[must_use]
    pub const fn adapters(&self) -> (&Commit, &Executor) {
        (&self.commit, &self.executor)
    }

    pub fn adapters_mut(&mut self) -> (&mut Commit, &mut Executor) {
        (&mut self.commit, &mut self.executor)
    }

    pub fn handle(
        &mut self,
        ctx: StepCtx,
        event: Event,
    ) -> Result<DriveReport<Executor::Error>, DriveError<Commit::Error>> {
        let (next, effects) = step(self.state.clone(), ctx, event);
        self.commit_transition(next, effects)?;
        self.retry_pending()
    }

    pub fn schedule_wall(
        &mut self,
        ctx: StepCtx,
        target: WallMs,
        token: WakeToken,
    ) -> Result<DriveReport<Executor::Error>, DriveError<Commit::Error>> {
        let (next, effects) = prepare_wall_schedule(self.state.clone(), ctx, target, token)
            .map_err(DriveError::Clock)?;
        self.commit_transition(next, effects)?;
        self.retry_pending()
    }

    pub fn retry_pending(
        &mut self,
    ) -> Result<DriveReport<Executor::Error>, DriveError<Commit::Error>> {
        let entries = self
            .outbox
            .pending
            .iter()
            .map(|(id, effect)| (*id, effect.clone()))
            .collect::<Vec<_>>();
        let mut followups = std::mem::take(&mut self.bootstrap_followups);
        let mut failures = Vec::new();
        for (id, effect) in entries {
            if !self.outbox.pending.contains_key(&id) {
                continue;
            }
            let Some(current_revision) = self.revision else {
                self.bootstrap_followups = followups;
                return Err(DriveError::MissingRevision);
            };
            let Some(revision) = current_revision.checked_add(1) else {
                self.bootstrap_followups = followups;
                return Err(DriveError::RevisionExhausted);
            };
            match self.executor.execute(&effect) {
                Ok(followup) => {
                    if let Err(source) = self.commit.acknowledge(current_revision, revision, id) {
                        self.bootstrap_followups = followups;
                        return Err(DriveError::Acknowledge { id, source });
                    }
                    self.revision = Some(revision);
                    if let Some(event) = followup {
                        followups.push(event);
                    }
                    if let Err(error) = self.outbox.remove(id) {
                        self.bootstrap_followups = followups;
                        return Err(DriveError::Outbox(error));
                    }
                }
                Err(source) => failures.push(EffectFailure { id, effect, source }),
            }
        }
        Ok(DriveReport {
            followups,
            failures,
        })
    }

    fn commit_transition(
        &mut self,
        next: State,
        effects: Vec<Effect>,
    ) -> Result<(), DriveError<Commit::Error>> {
        let mut outbox = self.outbox.clone();
        outbox
            .enqueue(
                effects,
                self.limits.normal_entries,
                self.limits.normal_bytes,
            )
            .map_err(DriveError::Outbox)?;
        let revision = match self.revision {
            Some(current) => current
                .checked_add(1)
                .ok_or(DriveError::RevisionExhausted)?,
            None => 0,
        };
        let snapshot = DurableSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            revision,
            persisted: next.persisted().clone(),
            watermark: durable_watermark(next.clock()),
            outbox: outbox.clone(),
        };
        self.commit
            .commit(self.revision, &snapshot)
            .map_err(DriveError::Commit)?;
        self.state = next;
        self.revision = Some(revision);
        self.outbox = outbox;
        Ok(())
    }
}

fn effect_set_bytes<'a>(effects: impl IntoIterator<Item = &'a Effect>) -> Result<u64, OutboxError> {
    effects.into_iter().try_fold(0_u64, |total, effect| {
        total
            .checked_add(effect_bytes(effect)?)
            .ok_or(OutboxError::SizeOverflow)
    })
}

fn effect_bytes(effect: &Effect) -> Result<u64, OutboxError> {
    match effect {
        Effect::Gossip { to, msg } => {
            let (_, body) = encode_wire_msg(msg);
            sum_bytes([1, text_bytes(to.as_str())?, sized_bytes(body.len())?])
        }
        Effect::Append {
            intent,
            slot,
            generation: _,
            draft,
        } => sum_bytes([
            1,
            text_bytes(intent.as_str())?,
            slot_bytes(slot)?,
            8,
            draft_bytes(draft)?,
        ]),
        Effect::Reply { ingress, corr, ans } => sum_bytes([
            1,
            text_bytes(ingress.as_str())?,
            text_bytes(corr.as_str())?,
            route_answer_bytes(ans)?,
        ]),
        Effect::Schedule { token, .. } => sum_bytes([1, wake_token_bytes(token)?, 8]),
        Effect::Teardown { slot, .. } => sum_bytes([1, slot_bytes(slot)?, 8]),
    }
}

fn draft_bytes(draft: &ClaimDraft) -> Result<u64, OutboxError> {
    match draft {
        ClaimDraft::Workspace {
            node,
            share,
            identity,
            grant_ref,
            lease_expiry_ms: _,
            epoch: _,
        } => sum_bytes([
            1,
            text_bytes(node.as_str())?,
            text_bytes(share)?,
            identity_bytes(identity)?,
            record_id_bytes(grant_ref.record())?,
            8,
            8,
        ]),
        ClaimDraft::Service {
            node,
            share,
            glade_id,
            key,
            identity,
            def_ref,
            exec_grant_ref,
            compute_key,
            lease_expiry_ms: _,
            epoch: _,
        } => sum_bytes([
            1,
            text_bytes(node.as_str())?,
            text_bytes(share)?,
            text_bytes(glade_id)?,
            blob_bytes(key.len())?,
            identity_bytes(identity)?,
            record_id_bytes(def_ref.record())?,
            record_id_bytes(exec_grant_ref.record())?,
            blob_bytes(compute_key.as_bytes().len())?,
            8,
            8,
        ]),
    }
}

fn identity_bytes(identity: &ClaimIdentity) -> Result<u64, OutboxError> {
    match identity {
        ClaimIdentity::Mint => Ok(1),
        ClaimIdentity::Existing(claim_id) => sum_bytes([1, record_id_bytes(claim_id.record())?]),
    }
}

fn wake_token_bytes(token: &WakeToken) -> Result<u64, OutboxError> {
    match token {
        WakeToken::ClaimRenew { slot, .. } => sum_bytes([1, slot_bytes(slot)?, 8]),
        WakeToken::AppendRetry {
            slot,
            generation: _,
            intent,
        } => sum_bytes([1, slot_bytes(slot)?, 8, text_bytes(intent.as_str())?]),
        WakeToken::GossipTick { peer } => sum_bytes([1, text_bytes(peer.as_str())?]),
        WakeToken::SyncTimeout {
            peer,
            sync_id,
            attempt: _,
        } => sum_bytes([
            1,
            text_bytes(peer.as_str())?,
            text_bytes(sync_id.as_str())?,
            1,
        ]),
    }
}

fn route_answer_bytes(answer: &RouteAns) -> Result<u64, OutboxError> {
    match answer {
        RouteAns::Matched { node } => sum_bytes([1, text_bytes(node.as_str())?]),
        RouteAns::NoClaim => Ok(1),
    }
}

fn slot_bytes(slot: &Slot) -> Result<u64, OutboxError> {
    match slot {
        Slot::Workspace { share } => sum_bytes([1, text_bytes(share)?]),
        Slot::Binding {
            share,
            glade_id,
            key,
        } => sum_bytes([
            1,
            text_bytes(share)?,
            text_bytes(glade_id)?,
            blob_bytes(key.len())?,
        ]),
    }
}

fn record_id_bytes(record: &RecordId) -> Result<u64, OutboxError> {
    sum_bytes([
        text_bytes(&record.stream.share)?,
        text_bytes(&record.stream.glade_id)?,
        blob_bytes(record.stream.key.len())?,
        text_bytes(record.origin.as_str())?,
        8,
    ])
}

fn text_bytes(value: &str) -> Result<u64, OutboxError> {
    sum_bytes([8, sized_bytes(value.len())?])
}

fn blob_bytes(length: usize) -> Result<u64, OutboxError> {
    sum_bytes([8, sized_bytes(length)?])
}

fn sized_bytes(value: usize) -> Result<u64, OutboxError> {
    u64::try_from(value).map_err(|_| OutboxError::SizeOverflow)
}

fn sum_bytes<const N: usize>(parts: [u64; N]) -> Result<u64, OutboxError> {
    parts.into_iter().try_fold(0_u64, |total, part| {
        total.checked_add(part).ok_or(OutboxError::SizeOverflow)
    })
}

fn validated_retained_bytes(persisted: &PersistedState) -> Result<u64, OutboxError> {
    let actual = persisted
        .retained
        .values()
        .flat_map(BTreeMap::values)
        .try_fold(0_u64, |total, op| {
            let bytes =
                u64::try_from(op.canonical_bytes().len()).map_err(|_| OutboxError::SizeOverflow)?;
            if bytes > MAX_RECORD_BYTES as u64 {
                return Err(OutboxError::RetainedRecordTooLarge);
            }
            total.checked_add(bytes).ok_or(OutboxError::SizeOverflow)
        })?;
    if actual != persisted.retained_bytes {
        return Err(OutboxError::InvalidRetainedByteCount);
    }
    Ok(actual)
}

fn required_sync_capacity(
    config: &KernelConfig,
    actual_retained_bytes: u64,
) -> Result<(usize, u64), OutboxError> {
    if config.peers.is_empty() {
        return Ok((0, 0));
    }
    let retained_bytes = config.max_retained_bytes.get().max(actual_retained_bytes);
    let record_count = retained_bytes / u64::try_from(MIN_SIGNED_OP_BYTES).unwrap_or(u64::MAX);
    let chunk_count = record_count
        .checked_add(MIN_OPS_PER_SYNC_CHUNK - 1)
        .ok_or(OutboxError::SizeOverflow)?
        / MIN_OPS_PER_SYNC_CHUNK;
    let response_entries = usize::try_from(
        chunk_count
            .checked_add(1)
            .ok_or(OutboxError::SizeOverflow)?,
    )
    .map_err(|_| OutboxError::SizeOverflow)?;
    let required_entries = response_entries.max(2);
    let peer_bytes = config
        .peers
        .iter()
        .map(|peer| u64::try_from(peer.as_str().len()).map_err(|_| OutboxError::SizeOverflow))
        .try_fold(0_u64, |maximum, length| {
            length.map(|length| maximum.max(length))
        })?;
    // A full 256-byte SyncId plus CBOR map/text/array framing costs at most
    // 265 bytes per SyncOps body. Durable Gossip framing adds 9 + peer bytes.
    // SyncEnd costs at most 261 body bytes plus the same durable framing.
    let chunk_overhead = 274_u64
        .checked_add(peer_bytes)
        .ok_or(OutboxError::SizeOverflow)?;
    let end_bytes = 270_u64
        .checked_add(peer_bytes)
        .ok_or(OutboxError::SizeOverflow)?;
    let response_bytes = retained_bytes
        .checked_add(
            chunk_count
                .checked_mul(chunk_overhead)
                .ok_or(OutboxError::SizeOverflow)?,
        )
        .and_then(|bytes| bytes.checked_add(end_bytes))
        .ok_or(OutboxError::SizeOverflow)?;
    // A locally initiated round atomically enqueues one maximum-size
    // SyncStart plus its SyncTimeout schedule.
    let initiated_bytes = u64::try_from(glade_discover_protocol::MAX_MESSAGE_BYTES)
        .map_err(|_| OutboxError::SizeOverflow)?
        .checked_add(292)
        .and_then(|bytes| bytes.checked_add(peer_bytes.checked_mul(2)?))
        .ok_or(OutboxError::SizeOverflow)?;
    let required_bytes = response_bytes.max(initiated_bytes);
    Ok((required_entries, required_bytes))
}

const fn durable_watermark(clock: ClockState) -> Option<WallMs> {
    match clock {
        ClockState::Ready { watermark } => Some(watermark),
        ClockState::Uncertain { floor } => floor,
    }
}
