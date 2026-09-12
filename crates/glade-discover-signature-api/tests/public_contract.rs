use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use glade_discover_protocol::{Principal, SignedOp, unsigned_canonical_bytes};
use glade_discover_signature_api::{
    SignError, SignatureStatus, Signer, VerificationError, Verifier, conformance,
};

// NON-CRYPTOGRAPHIC fixture. Never export or use this as a development signer.
#[derive(Default)]
struct TestCrypto {
    unavailable: bool,
    accept_anything: bool,
    wrong_signer: bool,
    unknown_origin_unavailable: bool,
}
impl Signer for TestCrypto {
    fn principal(&self) -> Principal {
        Principal::from("fixture-only")
    }
    async fn sign(&self, unsigned: &[u8]) -> Result<Vec<u8>, SignError> {
        if self.unavailable {
            Err(SignError::Unavailable)
        } else {
            Ok(unsigned.to_vec())
        }
    }
}
impl Verifier for TestCrypto {
    async fn verify(&self, op: &SignedOp) -> Result<SignatureStatus, VerificationError> {
        if self.unavailable
            || (self.unknown_origin_unavailable && op.envelope().origin != self.principal())
        {
            return Err(VerificationError::Unavailable);
        }
        if self.accept_anything
            || (op.envelope().origin == self.principal()
                && op.signature() == unsigned_canonical_bytes(op))
        {
            Ok(SignatureStatus::Valid {
                signer: if self.wrong_signer {
                    Principal::from("wrong")
                } else {
                    self.principal()
                },
            })
        } else {
            Ok(SignatureStatus::Invalid)
        }
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
fn sg_001_002_round_trip_tamper_and_origin_binding() {
    let port = TestCrypto::default();
    ready(conformance::round_trip_and_tamper(&port, &port));
}

#[test]
fn sg_003_backend_failure_is_not_invalid_signature_or_success() {
    let port = TestCrypto {
        unavailable: true,
        ..Default::default()
    };
    ready(conformance::unavailable(&port, &port));
}

#[test]
#[should_panic(expected = "SG-002")]
fn canonical_suite_rejects_accept_anything_verifier() {
    let signer = TestCrypto::default();
    let verifier = TestCrypto {
        accept_anything: true,
        ..Default::default()
    };
    ready(conformance::round_trip_and_tamper(&signer, &verifier));
}

#[test]
#[should_panic(expected = "SG-001")]
fn canonical_suite_rejects_wrong_verified_identity() {
    let signer = TestCrypto::default();
    let verifier = TestCrypto {
        wrong_signer: true,
        ..Default::default()
    };
    ready(conformance::round_trip_and_tamper(&signer, &verifier));
}

#[test]
fn unknown_origin_key_can_fail_closed_without_claiming_invalid_signature() {
    let signer = TestCrypto::default();
    let verifier = TestCrypto {
        unknown_origin_unavailable: true,
        ..Default::default()
    };
    ready(conformance::round_trip_and_tamper(&signer, &verifier));
}

#[derive(Default)]
struct ObservedCrypto {
    calls: AtomicUsize,
    eager: bool,
    eager_verify_only: bool,
}
impl Signer for ObservedCrypto {
    fn principal(&self) -> Principal {
        Principal::from("fixture-only")
    }
    fn sign(&self, _: &[u8]) -> impl Future<Output = Result<Vec<u8>, SignError>> + Send {
        if self.eager {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
        async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(SignError::Rejected)
        }
    }
}
impl Verifier for ObservedCrypto {
    fn verify(
        &self,
        _: &SignedOp,
    ) -> impl Future<Output = Result<SignatureStatus, VerificationError>> + Send {
        if self.eager || self.eager_verify_only {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
        async {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(VerificationError::Unavailable)
        }
    }
}

#[test]
fn sg_004_sign_and_verify_are_lazy() {
    let port = ObservedCrypto::default();
    conformance::unpolled(&port, &port, || port.calls.load(Ordering::SeqCst));
}

#[test]
#[should_panic(expected = "SG-004")]
fn canonical_suite_rejects_eager_crypto_io() {
    let port = ObservedCrypto {
        eager: true,
        ..Default::default()
    };
    conformance::unpolled(&port, &port, || port.calls.load(Ordering::SeqCst));
}

#[test]
#[should_panic(expected = "SG-004 eager verification")]
fn canonical_suite_independently_checks_verifier_laziness() {
    let port = ObservedCrypto {
        eager_verify_only: true,
        ..Default::default()
    };
    conformance::unpolled(&port, &port, || port.calls.load(Ordering::SeqCst));
}
