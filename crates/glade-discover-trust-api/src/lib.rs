//! Draft policy seam, separate from signature integrity. No policy implementation.
use glade_discover_protocol::{Principal, RecordId, StreamId};
use std::{collections::BTreeSet, future::Future};
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

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Fixture permits exactly request() at time 10 until 20. Real adapters install
    //! authenticated grants/revocations themselves; these probes are not crypto tests.
    use crate::*;
    pub async fn expires<P: TrustPolicy>(p: &P, expire: impl FnOnce()) {
        assert!(
            matches!(p.evaluate(&request()).await, Ok(Decision::Permit(_))),
            "TP-005 initially authorized"
        );
        expire();
        assert_eq!(
            p.evaluate(&request()).await,
            Ok(Decision::Deny),
            "TP-005 expiry boundary"
        );
    }
    pub async fn clock_uncertain<P: TrustPolicy>(p: &P) {
        assert_eq!(
            p.evaluate(&request()).await,
            Err(PolicyError::ClockUncertain),
            "TP-005 uncertain time"
        );
    }
    pub fn request() -> TrustRequest {
        TrustRequest {
            subject: "consumer".into(),
            action: Action::Read,
            resource: Resource::Sources(
                [StreamId {
                    share: "workspace".into(),
                    glade_id: "source".into(),
                    key: vec![],
                }]
                .into(),
            ),
        }
    }
    pub fn basis() -> RecordId {
        RecordId {
            stream: StreamId::default(),
            origin: "authority".into(),
            seq: 1,
        }
    }
    pub async fn scoped<P: TrustPolicy>(p: &P) {
        let mut mixed = request();
        if let Resource::Sources(sources) = &mut mixed.resource {
            sources.insert(StreamId::default());
        }
        assert_eq!(
            p.evaluate(&mixed).await,
            Ok(Decision::Deny),
            "TP-002 mixed source closure"
        );
        let r = request();
        let Decision::Permit(a) = p.evaluate(&r).await.expect("TP-001 evaluation") else {
            panic!("TP-001 permit")
        };
        assert_eq!(a.request, r, "TP-001 binding");
        assert_eq!(a.evaluated_at_ms, 10, "TP-001 time");
        assert_eq!(a.valid_until_ms, 20, "TP-001 expiry");
        assert!(!a.basis.is_empty(), "TP-001 basis");
        let mut wrong = r.clone();
        wrong.subject = "other".into();
        assert_eq!(
            p.evaluate(&wrong).await,
            Ok(Decision::Deny),
            "TP-002 subject"
        );
        wrong = r.clone();
        wrong.action = Action::Publish;
        assert_eq!(
            p.evaluate(&wrong).await,
            Ok(Decision::Deny),
            "TP-002 action"
        );
        wrong = r;
        wrong.resource = Resource::Sources([StreamId::default()].into());
        assert_eq!(p.evaluate(&wrong).await, Ok(Decision::Deny), "TP-002 scope");
    }
    pub async fn revoked<P: TrustPolicy>(p: &P, revoke: impl FnOnce()) {
        assert!(
            matches!(p.evaluate(&request()).await, Ok(Decision::Permit(_))),
            "TP-003 initial permit"
        );
        revoke();
        assert_eq!(
            p.evaluate(&request()).await,
            Ok(Decision::Deny),
            "TP-003 revocation"
        );
    }
    pub async fn unavailable<P: TrustPolicy>(p: &P) {
        assert_eq!(
            p.evaluate(&request()).await,
            Err(PolicyError::EvidenceUnavailable),
            "TP-004 fail closed"
        );
    }
}
