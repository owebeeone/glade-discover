//! Draft policy seam, separate from signature integrity. No policy implementation.
use glade_discover_protocol::{Principal, RecordId, StreamId};
use std::{collections::BTreeSet, future::Future};
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Read,
    Publish,
    Renew,
    ServeRegistry,
}
/// Exact canonical scope, NOT naive text-prefix matching. Namespace delegation
/// needs a versioned policy profile before production implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resource {
    Sources(BTreeSet<StreamId>),
    Namespace(String),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustRequest {
    pub subject: Principal,
    pub action: Action,
    pub resource: Resource,
}
/// Trusted evaluator output, NOT an unforgeable capability or transferable JWT.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Authorization {
    pub request: TrustRequest,
    pub policy_revision: u64,
    pub evaluated_at_ms: i64,
    pub valid_until_ms: i64,
    pub basis: Vec<RecordId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    Permit(Authorization),
    Deny,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyError {
    Unavailable,
    EvidenceUnavailable,
    ClockUncertain,
}
/// ```compile_fail
/// use glade_discover_trust_api::TrustPolicy;
/// struct Missing;
/// impl TrustPolicy for Missing {}
/// ```
pub trait TrustPolicy: Send + Sync {
    /// TP-001: bind exact subject/action and COMPLETE resource/source closure.
    /// TP-002: valid signatures alone MUST NOT confer permission. Missing/stale
    /// authority/revocation evidence and uncertain time MUST NOT produce Permit.
    /// Distinguish determinate Deny from inability to decide (PolicyError).
    /// TP-003: re-evaluate current authoritative fold on every call, including retries.
    /// Permit MUST have evaluated_at < valid_until, bounded by authority freshness.
    /// A permit is an evaluation snapshot: callers MUST evaluate again for each
    /// security-sensitive operation. This port provides no revision subscription;
    /// caching until expiry is unsafe without a separately specified invalidation
    /// mechanism. A permit is NOT revocation immunity. Basis is not a wire proof.
    /// TP-004: error => fail closed, never fallback to permissive development mode.
    /// Authenticated identities come from ingress; futures MUST be lazy.
    fn evaluate(
        &self,
        request: &TrustRequest,
    ) -> impl Future<Output = Result<Decision, PolicyError>> + Send;
}
