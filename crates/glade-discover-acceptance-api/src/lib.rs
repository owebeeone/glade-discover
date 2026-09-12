//! Draft per-stream atomic acceptance journal, NOT a database implementation.
//! Trusted admission MUST validate signature, payload, authority and clock first.
use glade_discover_protocol::{
    ClaimDraft, Generation, IntentId, Principal, SignedOp, Slot, StreamId,
};
use std::future::Future;
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamScope {
    pub stream: StreamId,
    pub origin: Principal,
}
/// Requester MUST come from authenticated ingress, not an asserted wire identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RequestKey {
    pub requester: Principal,
    pub slot: Slot,
    pub generation: Generation,
    pub intent: IntentId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub seq: u64,
    pub hash: [u8; 32],
    pub lamport: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub expected_revision: u64,
    pub key: RequestKey,
    pub draft: ClaimDraft,
    pub op: SignedOp,
    pub watermark_ms: i64,
}
/// Local durable receipt, not replica confirmation or current discoverability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Accepted {
    pub key: RequestKey,
    pub draft: ClaimDraft,
    pub op: SignedOp,
    pub commit_revision: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    pub revision: u64,
    pub cursor: Option<Cursor>,
    pub watermark_ms: Option<i64>,
    pub pending: Vec<Accepted>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    Unavailable,
    Corrupt,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitError {
    Conflict,
    RequestMismatch,
    InvalidOperation,
    ClockRegression,
    Unavailable,
    OutcomeUnknown,
    Capacity,
    Exhausted,
}
/// Mutations MUST be serialized per scope. Futures MUST be lazy; dropping a polled
/// mutation means unknown outcome, not rollback or completed cleanup.
/// ```compile_fail
/// use glade_discover_acceptance_api::AcceptanceJournal;
/// struct Missing;
/// impl AcceptanceJournal for Missing {}
/// ```
pub trait AcceptanceJournal: Send + Sync {
    /// Immutable local stream/origin ownership for this handle.
    fn scope(&self) -> StreamScope;
    /// AC-001/003: authenticated consistent snapshot. Unreadable/corrupt MUST NOT
    /// become empty. Readable fresh scope has revision 0, no cursor/watermark.
    /// Initial watermark absence is not clock certainty: trusted admission must
    /// establish it. Implementations MUST document bounded pending/retained storage
    /// and reject new work at capacity; this draft permits no silent eviction/GC.
    fn recover(&self) -> impl Future<Output = Result<Recovery, ReadError>> + Send;
    /// Deduplication survives reopen and acknowledgement; errors are not absence.
    fn lookup(
        &self,
        key: &RequestKey,
    ) -> impl Future<Output = Result<Option<Accepted>, ReadError>> + Send;
    /// AC-001: atomically persist key+draft+EXACT signed bytes, increment revision,
    /// advance cursor and non-regressing watermark, and enqueue a durable handoff.
    /// Success MUST follow the specified local durability barrier, before gossip.
    /// AC-004: exact key+draft+op replay returns original Accepted BEFORE checking
    /// CAS, even after ack/clock advance. Changed draft/bytes => RequestMismatch.
    /// New keys require matching expected_revision and valid scope, next seq
    /// (initial 1), prev hash, increasing lamport, finalized draft/slot binding.
    /// Overflow fails closed. Every known error MUST leave the scope unchanged.
    /// AC-002/003: crashes expose whole commit or none. Lost replies are recovered
    /// by lookup/recovery; never blindly allocate another operation on retry.
    fn commit(
        &self,
        proposal: &Proposal,
    ) -> impl Future<Output = Result<Accepted, CommitError>> + Send;
    /// AC-005: AFTER downstream durable fold/outbox commit, atomically remove
    /// pending handoff and advance revision; retain dedup/cursor/watermark.
    /// Already acknowledged key => current revision, no write. Pending key requires
    /// matching revision; unknown key => InvalidOperation. Delivery is at least
    /// once; consumer dedup is mandatory. Derived kernel snapshot is a separate
    /// downstream transaction, not part of this write-ahead journal's schema.
    fn acknowledge(
        &self,
        key: &RequestKey,
        expected_revision: u64,
    ) -> impl Future<Output = Result<u64, CommitError>> + Send;
}
