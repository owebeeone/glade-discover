//! Review-stage canonical operation storage contract; no database implementation.
//! This is NOT an accepted-intent journal or an atomic kernel-state commit API.
//! It alone cannot satisfy the complete v3.1 append/restart transaction boundary.

use glade_discover_protocol::SignedOp;
use std::future::Future;

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

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Reusable suite for an empty isolated store. Reopen/crash evidence MUST also
    //! come from the actual adapter; these assertions alone cannot prove durability.

    use crate::{DurableOperationStore, Persisted, ReadError, WriteError};
    use glade_discover_protocol::{
        OpEnvelope, Principal, Shape, SignedOp, StreamId, encode_signed_op, op_hash,
    };

    /// Codec-valid unsigned probe; storage MUST NOT invent an authorization policy.
    pub fn operation(payload: Vec<u8>) -> SignedOp {
        encode_signed_op(
            &OpEnvelope {
                stream: StreamId::default(),
                origin: Principal::from("fixture-only"),
                seq: 1,
                prev: None,
                lamport: 1,
                refs: vec![],
                shape: Shape::Log,
                payload,
            },
            &[],
        )
        .unwrap()
    }

    pub async fn missing<S: DurableOperationStore>(store: &S) {
        assert_eq!(
            store.load(&op_hash(&operation(vec![0]))).await,
            Ok(None),
            "OS-001 absence"
        );
    }

    pub async fn round_trip<S: DurableOperationStore>(store: &S) {
        let first = operation(vec![1, 2, 3]);
        let hash = op_hash(&first);
        for _ in 0..2 {
            assert_eq!(
                store.persist(&first).await,
                Ok(Persisted { hash }),
                "OS-002 receipt / OS-003 retry"
            );
            assert_eq!(
                store.load(&hash).await,
                Ok(Some(first.clone())),
                "OS-002 exact record"
            );
        }
        // Same origin/stream/sequence, different signed bytes: keep both as evidence.
        let second = operation(vec![9]);
        let other_hash = op_hash(&second);
        assert_ne!(hash, other_hash);
        assert_eq!(
            store.persist(&second).await,
            Ok(Persisted { hash: other_hash }),
            "OS-003 second record"
        );
        assert_eq!(
            store.load(&other_hash).await,
            Ok(Some(second)),
            "OS-003 second record retained"
        );
        assert_eq!(
            store.load(&hash).await,
            Ok(Some(first)),
            "OS-003 no overwrite by identity"
        );
    }

    pub async fn read_unavailable<S: DurableOperationStore>(store: &S) {
        assert_eq!(
            store.load(&op_hash(&operation(vec![]))).await,
            Err(ReadError::Unavailable),
            "OS-004 read failure"
        );
    }

    pub async fn write_failure<S: DurableOperationStore>(store: &S, error: WriteError) {
        let op = operation(vec![]);
        let before = store
            .load(&op_hash(&op))
            .await
            .expect("OS-004 fixture read must work");
        assert_eq!(store.persist(&op).await, Err(error), "OS-004 write failure");
        if error != WriteError::OutcomeUnknown {
            assert_eq!(
                store.load(&op_hash(&op)).await,
                Ok(before),
                "OS-004 known failure changed storage"
            );
        }
    }

    /// Reopen MUST close the original adapter and return an independently reopened
    /// handle to the same backing store. A snapshot fixture only tests this assertion.
    pub async fn survives_reopen<S, F>(store: S, reopen: impl FnOnce(S) -> F)
    where
        S: DurableOperationStore,
        F: std::future::Future<Output = S>,
    {
        let op = operation(vec![5]);
        let hash = op_hash(&op);
        assert_eq!(
            store.persist(&op).await,
            Ok(Persisted { hash }),
            "OS-005 persist"
        );
        let reopened = reopen(store).await;
        assert_eq!(
            reopened.load(&hash).await,
            Ok(Some(op)),
            "OS-005 reopen retained exact bytes"
        );
    }

    /// Same unsigned operation, different signature: protocol hash stays unchanged.
    pub async fn signature_variant<S: DurableOperationStore>(store: &S) {
        let first = operation(vec![6]);
        let second = encode_signed_op(first.envelope(), &[9]).unwrap();
        let hash = op_hash(&first);
        assert_eq!(hash, op_hash(&second));
        assert_ne!(first.canonical_bytes(), second.canonical_bytes());
        assert_eq!(
            store.persist(&first).await,
            Ok(Persisted { hash }),
            "OS-006 first variant"
        );
        assert_eq!(
            store.persist(&second).await,
            Err(WriteError::CanonicalConflict),
            "OS-006 signature conflict"
        );
        assert_eq!(
            store.load(&hash).await,
            Ok(Some(first)),
            "OS-006 original variant retained"
        );
    }

    pub fn unpolled<S: DurableOperationStore>(store: &S, effects: impl Fn() -> usize) {
        let op = operation(vec![]);
        let hash = op_hash(&op);
        let before = effects();
        let reading = store.load(&hash);
        assert_eq!(effects(), before, "OS-007 eager load");
        drop(reading);
        assert_eq!(effects(), before, "OS-007 load drop");
        let writing = store.persist(&op);
        assert_eq!(effects(), before, "OS-007 eager persist");
        drop(writing);
        assert_eq!(effects(), before, "OS-007 persist drop");
    }
}
