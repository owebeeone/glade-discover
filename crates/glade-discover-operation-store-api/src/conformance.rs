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
