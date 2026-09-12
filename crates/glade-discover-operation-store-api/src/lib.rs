//! Review-stage canonical operation storage contract; no database implementation.
//! This is NOT an accepted-intent journal or an atomic kernel-state commit API.
//! It alone cannot satisfy the complete v3.1 append/restart transaction boundary.

use glade_discover_protocol::SignedOp;
use std::future::Future;

#[cfg(feature = "conformance")]
pub mod conformance;

/// Protocol op_hash: hashes the unsigned envelope, NOT the signature bytes.
pub type OperationHash = [u8; 32];

/// A local durable record receipt, NOT gossip/replica confirmation or permission
/// to advertise acceptance before the remaining required transaction is committed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Persisted {
    pub hash: OperationHash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteError {
    /// Known not persisted because storage was unavailable.
    Unavailable,
    /// Known not persisted because admission rejected this operation.
    Rejected,
    /// Persistence may have happened; reconcile by hash or retry identical bytes.
    OutcomeUnknown,
    /// OS-006: this hash already has DIFFERENT canonical signed bytes. The
    /// previously stored bytes MUST remain unchanged; the new variant is rejected.
    CanonicalConflict,
}

/// ```compile_fail
/// use glade_discover_operation_store_api::DurableOperationStore;
/// struct Missing;
/// impl DurableOperationStore for Missing {}
/// ```
pub trait DurableOperationStore: Send + Sync {
    /// OS-001: None means absent from this store, not unknown or globally absent.
    /// OS-002: loaded data MUST preserve exact canonical bytes and match the hash.
    /// OS-004: read failure MUST NOT be translated into absence.
    /// Future construction MUST NOT initiate I/O. No ordering across calls is
    /// assumed until the caller observes their completion.
    fn load(
        &self,
        hash: &OperationHash,
    ) -> impl Future<Output = Result<Option<SignedOp>, ReadError>> + Send;

    /// OS-002: success MUST follow durable local persistence of the exact bytes,
    /// returning their protocol op_hash. Atomic all-or-nothing record visibility;
    /// successful records survive reopen under the adapter's stated durability
    /// profile (which MUST describe filesystem/device failure assumptions).
    /// OS-003: identical retries MUST be idempotent. Distinct hashes MUST coexist,
    /// including evidence of equivocation at the same RecordId; no overwrite by ID.
    /// No signature verification, authorization, retention/GC, intent allocation,
    /// sequence advancement or clock-watermark transaction is performed here.
    /// OS-006: the protocol hash excludes the signature. A different signed-byte
    /// variant at an existing hash MUST return CanonicalConflict without replacing
    /// it. Do not redefine op_hash. The caller/admission layer MUST prevent
    /// unverified data from reserving a canonical variant in the accepted store;
    /// this store performs no verification. Quarantining untrusted
    /// variants is a separate boundary, not implemented by this draft port.
    /// OS-004: ambiguous persistence MUST be OutcomeUnknown, not known rejection.
    /// Future construction MUST NOT initiate I/O. Dropping a polled future leaves
    /// persistence unknown and does NOT guarantee rollback or completed cleanup.
    fn persist(&self, op: &SignedOp) -> impl Future<Output = Result<Persisted, WriteError>> + Send;
}
