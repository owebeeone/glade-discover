//! Review-stage signature contracts. Integrity is NOT authorization or freshness.
//! No algorithm, trust-policy engine, executor, or permissive signer is provided.

use glade_discover_protocol::{Principal, SignedOp};
use std::future::Future;

#[cfg(feature = "conformance")]
pub mod conformance;

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
