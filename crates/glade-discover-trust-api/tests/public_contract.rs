use glade_discover_protocol::Principal;
use glade_discover_trust_api::*;
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

#[derive(Default)]
struct Policy {
    revoked: Mutex<bool>,
    unavailable: bool,
    permissive: bool,
    wrong_binding: bool,
    any_source: bool,
    elapsed: Mutex<i64>,
    ignore_expiry: bool,
    clock_uncertain: bool,
}
impl TrustPolicy for Policy {
    async fn evaluate(&self, r: &TrustRequest) -> Result<Decision, PolicyError> {
        if self.clock_uncertain {
            return Err(PolicyError::ClockUncertain);
        }
        if self.unavailable {
            return Err(PolicyError::EvidenceUnavailable);
        }
        let base = conformance::request();
        let partial = self.any_source
            && r.subject == base.subject
            && r.action == base.action
            && matches!((&r.resource,&base.resource),(Resource::Sources(all),Resource::Sources(allowed)) if allowed.iter().any(|a|all.contains(a)));
        if self.permissive
            || (!*self.revoked.lock().unwrap()
                && (*r == base || partial)
                && (self.ignore_expiry || *self.elapsed.lock().unwrap() < 10))
        {
            let mut binding = r.clone();
            if self.wrong_binding {
                binding.subject = Principal::from("wrong");
            }
            Ok(Decision::Permit(Authorization {
                request: binding,
                policy_revision: 1,
                evaluated_at_ms: 10 + *self.elapsed.lock().unwrap(),
                valid_until_ms: 20,
                basis: vec![conformance::basis()],
            }))
        } else {
            Ok(Decision::Deny)
        }
    }
}
fn ready<F: Future + Send>(f: F) -> F::Output {
    match std::pin::pin!(f)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(x) => x,
        Poll::Pending => panic!("fixture pending"),
    }
}
#[test]
fn tp_001_002_exact_subject_action_scope() {
    ready(conformance::scoped(&Policy::default()));
}
#[test]
fn tp_003_revocation_is_reevaluated() {
    let p = Policy::default();
    ready(conformance::revoked(&p, || {
        *p.revoked.lock().unwrap() = true
    }));
}
#[test]
fn tp_004_unknown_evidence_fails_closed() {
    ready(conformance::unavailable(&Policy {
        unavailable: true,
        ..Default::default()
    }));
}
#[test]
#[should_panic(expected = "TP-002")]
fn rejects_allow_everything_policy() {
    ready(conformance::scoped(&Policy {
        permissive: true,
        ..Default::default()
    }));
}
#[test]
#[should_panic(expected = "TP-001")]
fn rejects_unbound_permission() {
    ready(conformance::scoped(&Policy {
        wrong_binding: true,
        ..Default::default()
    }));
}

#[test]
#[should_panic(expected = "TP-002 mixed")]
fn rejects_any_source_instead_of_complete_closure() {
    ready(conformance::scoped(&Policy {
        any_source: true,
        ..Default::default()
    }));
}

#[test]
fn tp_005_expiry_and_uncertain_clock_fail_closed() {
    let p = Policy::default();
    ready(conformance::expires(&p, || *p.elapsed.lock().unwrap() = 10));
    ready(conformance::clock_uncertain(&Policy {
        clock_uncertain: true,
        ..Default::default()
    }));
}
#[test]
#[should_panic(expected = "TP-005")]
fn rejects_permit_forever() {
    let p = Policy {
        ignore_expiry: true,
        ..Default::default()
    };
    ready(conformance::expires(&p, || *p.elapsed.lock().unwrap() = 10));
}
