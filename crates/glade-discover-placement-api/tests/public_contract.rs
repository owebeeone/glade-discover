use glade_discover_placement_api::*;
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

#[derive(Default)]
struct Locator {
    now: Mutex<i64>,
    unknown: bool,
    unavailable: bool,
    stale: bool,
    unbounded: bool,
    wrong_node: bool,
    wrong_epoch: bool,
    unverified: bool,
}
impl ShardLocator for Locator {
    async fn locate(&self, q: &LocateRequest) -> Result<Location, LocateError> {
        if self.unverified {
            return Err(LocateError::Unverifiable);
        }
        if self.unknown {
            return Err(LocateError::UnknownNamespace);
        }
        if self.unavailable {
            return Err(LocateError::Unavailable);
        }
        if !self.stale && *self.now.lock().unwrap() >= 20 {
            return Err(LocateError::StaleMapping);
        }
        let mut result = conformance::location(q);
        result.observed_at_ms = *self.now.lock().unwrap();
        if self.wrong_node {
            result.registries[0].node = "attacker".into();
        }
        if self.wrong_epoch {
            result.mapping_epoch = 0;
        }
        if self.unbounded {
            result.registries.push(result.registries[0].clone());
        }
        Ok(result)
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
fn pl_001_002_scoped_bounded_authority_evidence() {
    ready(conformance::bounded(&Locator::default()));
}
#[test]
fn pl_003_expired_mapping_is_not_returned_as_current() {
    let p = Locator::default();
    ready(conformance::expires(&p, || *p.now.lock().unwrap() = 20));
}
#[test]
fn pl_004_unknown_and_unreachable_are_distinct() {
    ready(conformance::error(
        &Locator {
            unknown: true,
            ..Default::default()
        },
        LocateError::UnknownNamespace,
    ));
    ready(conformance::error(
        &Locator {
            unavailable: true,
            ..Default::default()
        },
        LocateError::Unavailable,
    ));
}
#[test]
#[should_panic(expected = "PL-001")]
fn rejects_unbounded_candidates() {
    ready(conformance::bounded(&Locator {
        unbounded: true,
        ..Default::default()
    }));
}
#[test]
#[should_panic(expected = "PL-003")]
fn rejects_stale_mapping() {
    let p = Locator {
        stale: true,
        ..Default::default()
    };
    ready(conformance::expires(&p, || *p.now.lock().unwrap() = 20));
}

#[test]
#[should_panic(expected = "PL-002")]
fn rejects_unauthorized_node_with_copied_evidence() {
    ready(conformance::bounded(&Locator {
        wrong_node: true,
        ..Default::default()
    }));
}
#[test]
#[should_panic(expected = "PL-002")]
fn rejects_stale_mapping_epoch() {
    ready(conformance::bounded(&Locator {
        wrong_epoch: true,
        ..Default::default()
    }));
}
#[test]
fn unverifiable_delegation_returns_no_candidates() {
    ready(conformance::error(
        &Locator {
            unverified: true,
            ..Default::default()
        },
        LocateError::Unverifiable,
    ));
}
