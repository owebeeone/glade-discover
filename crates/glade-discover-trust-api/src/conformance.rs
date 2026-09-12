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
