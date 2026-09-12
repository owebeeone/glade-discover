use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use glade_discover_operation_store_api::{
    DurableOperationStore, OperationHash, Persisted, ReadError, WriteError, conformance,
};
use glade_discover_protocol::{SignedOp, op_hash};

// Volatile fixture validates the test contract; it is NOT a durable implementation.
#[derive(Default)]
struct TestStore {
    records: Mutex<BTreeMap<OperationHash, SignedOp>>,
    read_failure: bool,
    write_failure: Option<WriteError>,
    discard: bool,
    effect_before_failure: bool,
    overwrite_variants: bool,
}
impl DurableOperationStore for TestStore {
    async fn load(&self, hash: &OperationHash) -> Result<Option<SignedOp>, ReadError> {
        if self.read_failure {
            Err(ReadError::Unavailable)
        } else {
            Ok(self.records.lock().unwrap().get(hash).cloned())
        }
    }
    async fn persist(&self, op: &SignedOp) -> Result<Persisted, WriteError> {
        if let Some(error) = self.write_failure {
            if self.effect_before_failure {
                self.records.lock().unwrap().insert(op_hash(op), op.clone());
            }
            return Err(error);
        }
        let hash = op_hash(op);
        if !self.discard {
            let mut records = self.records.lock().unwrap();
            if !self.overwrite_variants && records.get(&hash).is_some_and(|old| old != op) {
                return Err(WriteError::CanonicalConflict);
            }
            records.insert(hash, op.clone());
        }
        Ok(Persisted { hash })
    }
}
fn ready<F: Future + Send>(future: F) -> F::Output {
    let waker = Waker::noop();
    match std::pin::pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(waker))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fixture unexpectedly pending"),
    }
}

#[test]
fn os_001_missing_is_explicit() {
    ready(conformance::missing(&TestStore::default()));
}

#[test]
fn os_002_003_canonical_round_trip_replay_and_equivocation_evidence() {
    ready(conformance::round_trip(&TestStore::default()));
}

#[test]
fn os_004_errors_do_not_masquerade_as_missing_or_durable() {
    let port = TestStore {
        read_failure: true,
        ..Default::default()
    };
    ready(conformance::read_unavailable(&port));
    for error in [
        WriteError::Unavailable,
        WriteError::Rejected,
        WriteError::OutcomeUnknown,
    ] {
        let port = TestStore {
            write_failure: Some(error),
            ..Default::default()
        };
        ready(conformance::write_failure(&port, error));
    }
}

#[test]
#[should_panic(expected = "OS-002")]
fn canonical_suite_rejects_false_success() {
    ready(conformance::round_trip(&TestStore {
        discard: true,
        ..Default::default()
    }));
}

#[test]
fn os_005_reopen_assertion_with_explicit_volatile_snapshot_fixture() {
    ready(conformance::survives_reopen(
        TestStore::default(),
        |store| async move {
            // Models a reopen boundary, not a crash or actual durable media.
            TestStore {
                records: Mutex::new(store.records.into_inner().unwrap()),
                ..Default::default()
            }
        },
    ));
}

#[test]
#[should_panic(expected = "OS-005")]
fn canonical_suite_rejects_lost_record_after_reopen() {
    ready(conformance::survives_reopen(
        TestStore::default(),
        |_| async { TestStore::default() },
    ));
}

#[test]
fn os_006_same_envelope_different_signature_is_explicit_conflict() {
    ready(conformance::signature_variant(&TestStore::default()));
}

#[test]
#[should_panic(expected = "OS-006")]
fn canonical_suite_rejects_signature_variant_overwrite() {
    ready(conformance::signature_variant(&TestStore {
        overwrite_variants: true,
        ..Default::default()
    }));
}

#[test]
#[should_panic(expected = "OS-004")]
fn canonical_suite_rejects_persistence_followed_by_known_rejection() {
    let port = TestStore {
        write_failure: Some(WriteError::Rejected),
        effect_before_failure: true,
        ..Default::default()
    };
    ready(conformance::write_failure(&port, WriteError::Rejected));
}

#[test]
fn uncertain_result_may_have_persisted() {
    let port = TestStore {
        write_failure: Some(WriteError::OutcomeUnknown),
        effect_before_failure: true,
        ..Default::default()
    };
    ready(conformance::write_failure(
        &port,
        WriteError::OutcomeUnknown,
    ));
    assert_eq!(port.records.lock().unwrap().len(), 1);
}

#[derive(Default)]
struct ObservedStore {
    calls: AtomicUsize,
    eager: bool,
    eager_write_only: bool,
}
impl DurableOperationStore for ObservedStore {
    fn load(
        &self,
        _: &OperationHash,
    ) -> impl Future<Output = Result<Option<SignedOp>, ReadError>> + Send {
        if self.eager {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
        async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }
    fn persist(&self, _: &SignedOp) -> impl Future<Output = Result<Persisted, WriteError>> + Send {
        if self.eager || self.eager_write_only {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
        async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(WriteError::Rejected)
        }
    }
}

#[test]
fn os_007_load_and_persist_are_lazy() {
    let port = ObservedStore::default();
    conformance::unpolled(&port, || port.calls.load(Ordering::SeqCst));
}

#[test]
#[should_panic(expected = "OS-007")]
fn canonical_suite_rejects_eager_storage_io() {
    let port = ObservedStore {
        eager: true,
        ..Default::default()
    };
    conformance::unpolled(&port, || port.calls.load(Ordering::SeqCst));
}

#[test]
#[should_panic(expected = "OS-007 eager persist")]
fn canonical_suite_independently_checks_persist_laziness() {
    let port = ObservedStore {
        eager_write_only: true,
        ..Default::default()
    };
    conformance::unpolled(&port, || port.calls.load(Ordering::SeqCst));
}
