use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use glade_discover_protocol::{NodeId, SyncId, WireMsg};
use glade_discover_transport_api::{LocalAcceptance, SendError, Transport, conformance};

#[derive(Default)]
struct TestTransport {
    submissions: Mutex<Vec<(NodeId, WireMsg)>>,
    failure: Option<SendError>,
    corrupt: bool,
    effect_before_failure: bool,
}

impl Transport for TestTransport {
    async fn send(&self, to: &NodeId, message: &WireMsg) -> Result<LocalAcceptance, SendError> {
        if let Some(error) = self.failure {
            if self.effect_before_failure {
                self.submissions
                    .lock()
                    .unwrap()
                    .push((to.clone(), message.clone()));
            }
            return Err(error);
        }
        self.submissions.lock().unwrap().push((
            if self.corrupt {
                NodeId::from("wrong")
            } else {
                to.clone()
            },
            message.clone(),
        ));
        Ok(LocalAcceptance)
    }
}

// Deliberately NOT an executor: a deterministic ready-only fixture driver.
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

fn message() -> WireMsg {
    WireMsg::SyncEnd {
        sync_id: SyncId::from("test"),
    }
}

#[test]
fn tr_001_exact_destination_and_message() {
    let port = TestTransport::default();
    ready(conformance::exact_submission(
        &port,
        &NodeId::from("peer"),
        &message(),
        || port.submissions.lock().unwrap().clone(),
    ));
}

#[test]
fn tr_002_failures_are_not_acceptance() {
    for error in [
        SendError::Unavailable,
        SendError::Rejected,
        SendError::OutcomeUnknown,
    ] {
        let port = TestTransport {
            failure: Some(error),
            ..Default::default()
        };
        ready(conformance::failure(
            &port,
            &NodeId::from("peer"),
            &message(),
            error,
            || port.submissions.lock().unwrap().clone(),
        ));
        assert!(port.submissions.lock().unwrap().is_empty());
    }
}

#[test]
fn tr_003_unpolled_call_has_no_submission() {
    let port = TestTransport::default();
    conformance::unpolled(&port, &NodeId::from("peer"), &message(), || {
        port.submissions.lock().unwrap().len()
    });
}

#[test]
fn tr_004_calls_can_overlap_without_exclusive_borrow() {
    let port = TestTransport::default();
    let a = NodeId::from("a");
    let b = NodeId::from("b");
    let msg = message();
    let first = port.send(&a, &msg);
    let second = port.send(&b, &msg);
    assert_eq!(ready(second), Ok(LocalAcceptance));
    assert_eq!(ready(first), Ok(LocalAcceptance));
}

#[test]
#[should_panic(expected = "TR-001")]
fn canonical_suite_rejects_wrong_destination() {
    let port = TestTransport {
        corrupt: true,
        ..Default::default()
    };
    ready(conformance::exact_submission(
        &port,
        &NodeId::from("peer"),
        &message(),
        || port.submissions.lock().unwrap().clone(),
    ));
}

#[test]
#[should_panic(expected = "TR-002")]
fn canonical_suite_rejects_effect_followed_by_known_rejection() {
    let port = TestTransport {
        failure: Some(SendError::Rejected),
        effect_before_failure: true,
        ..Default::default()
    };
    ready(conformance::failure(
        &port,
        &NodeId::from("peer"),
        &message(),
        SendError::Rejected,
        || port.submissions.lock().unwrap().clone(),
    ));
}

#[test]
fn uncertain_result_may_have_submitted() {
    let port = TestTransport {
        failure: Some(SendError::OutcomeUnknown),
        effect_before_failure: true,
        ..Default::default()
    };
    ready(conformance::failure(
        &port,
        &NodeId::from("peer"),
        &message(),
        SendError::OutcomeUnknown,
        || port.submissions.lock().unwrap().clone(),
    ));
    assert_eq!(port.submissions.lock().unwrap().len(), 1);
}

#[test]
fn canonical_async_suite_can_suspend_without_runtime_or_threads() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Deferred {
        paused: AtomicBool,
        inner: TestTransport,
    }
    impl Transport for Deferred {
        async fn send(&self, to: &NodeId, message: &WireMsg) -> Result<LocalAcceptance, SendError> {
            std::future::poll_fn(|cx| {
                if !self.paused.swap(true, Ordering::SeqCst) {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            self.inner.send(to, message).await
        }
    }
    let port = Deferred {
        paused: AtomicBool::new(false),
        inner: TestTransport::default(),
    };
    let to = NodeId::from("peer");
    let msg = message();
    let mut check = std::pin::pin!(conformance::exact_submission(&port, &to, &msg, || port
        .inner
        .submissions
        .lock()
        .unwrap()
        .clone()));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(check.as_mut().poll(&mut cx).is_pending());
    assert!(port.inner.submissions.lock().unwrap().is_empty());
    assert!(check.as_mut().poll(&mut cx).is_ready());
}
