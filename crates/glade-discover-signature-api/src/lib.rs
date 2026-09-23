//! Review-stage signature contracts. Integrity is NOT authorization or freshness.
//! No algorithm, trust-policy engine, executor, or permissive signer is provided.

use glade_discover_protocol::{Principal, SignedOp};
use std::future::Future;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignError {
    Unavailable,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationError {
    /// Unable to establish validity, e.g. verifier/key material unavailable.
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignatureStatus {
    /// Cryptographically authenticated identity, bound to the exact unsigned
    /// canonical bytes AND the envelope origin. Grants no permission by itself.
    Valid { signer: Principal },
    /// Verification completed and found the signature/origin binding invalid.
    Invalid,
}

/// ```compile_fail
/// use glade_discover_signature_api::Signer;
/// struct Missing;
/// impl Signer for Missing {}
/// ```
pub trait Signer: Send + Sync {
    /// SG-001: stable identity for this handle; key rotation uses a new handle.
    fn principal(&self) -> Principal;
    /// SG-001: signs the supplied unsigned canonical bytes as `principal()`.
    /// No default authorization policy is implied. Callers MUST construct and
    /// validate the intended envelope; an HSM/policy adapter MAY reject a request.
    /// The signature algorithm/key resolution is supplied by the implementation.
    /// Constructing the future MUST NOT initiate I/O; dropping a polled future
    /// does not roll back an HSM/audit effect or guarantee completed cleanup.
    fn sign(&self, unsigned: &[u8]) -> impl Future<Output = Result<Vec<u8>, SignError>> + Send;
}

/// ```compile_fail
/// use glade_discover_signature_api::Verifier;
/// struct Missing;
/// impl Verifier for Missing {}
/// ```
pub trait Verifier: Send + Sync {
    /// SG-002: MUST verify exact canonical bytes, signature, and origin binding.
    /// SG-003: inability to verify MUST be an error, never Valid or Invalid.
    /// A caller MUST fail closed on errors and still check authorization,
    /// freshness and payload validity separately after a Valid result.
    /// Constructing the future MUST NOT initiate I/O. No cleanup/rollback is
    /// promised by dropping a polled future. Uses generic, not dynamic dispatch.
    fn verify(
        &self,
        op: &SignedOp,
    ) -> impl Future<Output = Result<SignatureStatus, VerificationError>> + Send;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Reusable suite; the fixture signer MUST permit the probe envelope and the
    //! verifier MUST have its authentic key. No real cryptographic backend is shipped.
    //! Implementations also need algorithm vectors and key-resolution security tests.

    use crate::{SignError, SignatureStatus, Signer, VerificationError, Verifier};
    use glade_discover_protocol::{
        OpEnvelope, Principal, Shape, StreamId, encode_signed_op, unsigned_canonical_bytes,
    };

    fn envelope(origin: Principal) -> OpEnvelope {
        OpEnvelope {
            stream: StreamId::default(),
            origin,
            seq: 1,
            prev: None,
            lamport: 1,
            refs: vec![],
            shape: Shape::Log,
            payload: vec![1, 2, 3],
        }
    }

    pub async fn round_trip_and_tamper<S: Signer, V: Verifier>(signer: &S, verifier: &V) {
        let identity = signer.principal();
        let envelope = envelope(identity.clone());
        let unsigned = encode_signed_op(&envelope, &[]).unwrap();
        let signature = signer
            .sign(&unsigned_canonical_bytes(&unsigned))
            .await
            .expect("SG-001 signing");
        assert_eq!(signer.principal(), identity, "SG-001 stable identity");
        let op = encode_signed_op(&envelope, &signature).unwrap();
        assert_eq!(
            verifier.verify(&op).await,
            Ok(SignatureStatus::Valid { signer: identity }),
            "SG-001 authenticated identity"
        );

        let mut changed = envelope.clone();
        changed.payload.push(99);
        let tampered = encode_signed_op(&changed, &signature).unwrap();
        assert_eq!(
            verifier.verify(&tampered).await,
            Ok(SignatureStatus::Invalid),
            "SG-002 payload binding"
        );

        changed = envelope.clone();
        changed.origin = Principal::from(format!("{}-other", envelope.origin));
        let forged_origin = encode_signed_op(&changed, &signature).unwrap();
        assert!(
            matches!(
                verifier.verify(&forged_origin).await,
                Ok(SignatureStatus::Invalid) | Err(VerificationError::Unavailable)
            ),
            "SG-002 origin binding must fail closed; an unknown key is not proof of invalidity"
        );

        let mut bad_signature = signature;
        if let Some(first) = bad_signature.first_mut() {
            *first ^= 0xff;
        } else {
            bad_signature.push(0xff);
        }
        let forged = encode_signed_op(&envelope, &bad_signature).unwrap();
        assert_eq!(
            verifier.verify(&forged).await,
            Ok(SignatureStatus::Invalid),
            "SG-002 signature binding"
        );
    }

    /// Both backends MUST be configured as unavailable, not merely invalid inputs.
    pub async fn unavailable<S: Signer, V: Verifier>(signer: &S, verifier: &V) {
        let op = encode_signed_op(&envelope(signer.principal()), &[]).unwrap();
        assert_eq!(
            signer.sign(&unsigned_canonical_bytes(&op)).await,
            Err(SignError::Unavailable),
            "SG-003 signer error"
        );
        assert_eq!(
            verifier.verify(&op).await,
            Err(VerificationError::Unavailable),
            "SG-003 verifier error"
        );
    }

    /// An independent observer MUST count started backend effects (not just successes).
    pub fn unpolled<S: Signer, V: Verifier>(signer: &S, verifier: &V, effects: impl Fn() -> usize) {
        let op = encode_signed_op(&envelope(signer.principal()), &[]).unwrap();
        let bytes = unsigned_canonical_bytes(&op);
        let before = effects();
        let signing = signer.sign(&bytes);
        assert_eq!(effects(), before, "SG-004 eager signing");
        drop(signing);
        assert_eq!(effects(), before, "SG-004 signing drop");
        let verification = verifier.verify(&op);
        assert_eq!(effects(), before, "SG-004 eager verification");
        drop(verification);
        assert_eq!(effects(), before, "SG-004 verification drop");
    }
}
