//! Review-stage asynchronous transport contract. No executor or implementation.
//!
//! `conformance` is opt-in test support; enable it through a dev-dependency.
//! These traits use generic dispatch and Send futures, not `dyn Transport`.

use glade_discover_protocol::{NodeId, WireMsg};
use std::future::Future;

/// Local transport acceptance ONLY: not remote receipt, authorization, persistence,
/// replication, or a guarantee of eventual delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalAcceptance;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendError {
    /// Known not accepted: transport unavailable.
    Unavailable,
    /// Known not accepted: local admission rejected the message.
    Rejected,
    /// Acceptance/delivery may have happened; retry may duplicate the operation.
    OutcomeUnknown,
}

/// A marker implementation cannot satisfy this contract.
/// ```compile_fail
/// use glade_discover_transport_api::Transport;
/// struct Missing;
/// impl Transport for Missing {}
/// ```
pub trait Transport: Send + Sync {
    /// TR-001: MUST submit the exact destination/message; success means ONLY
    /// local acceptance. TR-002: MUST preserve known failure versus uncertainty.
    /// TR-003: MUST NOT perform I/O before polling the returned future.
    /// After polling, dropping the future does NOT promise rollback, non-delivery,
    /// or completed cleanup. No ordering is promised between overlapping calls.
    /// TR-004: shared access permits overlap; it does not require parallelism.
    fn send(
        &self,
        to: &NodeId,
        message: &WireMsg,
    ) -> impl Future<Output = Result<LocalAcceptance, SendError>> + Send;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Reusable assertions for a configured, isolated transport fixture.
    //! The observer MUST independently inspect the adapter's submission boundary.
    //! Passing against a double is not evidence of actual network delivery.

    use crate::{LocalAcceptance, SendError, Transport};
    use glade_discover_protocol::{NodeId, WireMsg};

    /// Fixture MUST begin with an empty submission log and accept this request.
    pub async fn exact_submission<T: Transport>(
        port: &T,
        to: &NodeId,
        message: &WireMsg,
        observed: impl FnOnce() -> Vec<(NodeId, WireMsg)>,
    ) {
        assert_eq!(
            port.send(to, message).await,
            Ok(LocalAcceptance),
            "TR-001 acceptance"
        );
        assert_eq!(
            observed(),
            vec![(to.clone(), message.clone())],
            "TR-001 exact submission"
        );
    }

    /// Fixture MUST be configured to produce the specified failure.
    pub async fn failure<T: Transport>(
        port: &T,
        to: &NodeId,
        message: &WireMsg,
        error: SendError,
        observed: impl Fn() -> Vec<(NodeId, WireMsg)>,
    ) {
        let before = observed();
        assert_eq!(
            port.send(to, message).await,
            Err(error),
            "TR-002 failure classification"
        );
        if error != SendError::OutcomeUnknown {
            assert_eq!(observed(), before, "TR-002 known failure after submission");
        }
    }

    /// Observation counts effects, not only completed requests.
    pub fn unpolled<T: Transport>(
        port: &T,
        to: &NodeId,
        message: &WireMsg,
        effects: impl Fn() -> usize,
    ) {
        let before = effects();
        let future = port.send(to, message);
        assert_eq!(effects(), before, "TR-003 eager effect");
        drop(future);
        assert_eq!(effects(), before, "TR-003 unpolled drop effect");
    }
}
