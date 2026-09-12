//! Review-stage asynchronous transport contract. No executor or implementation.
//!
//! `conformance` is opt-in test support; enable it through a dev-dependency.
//! These traits use generic dispatch and Send futures, not `dyn Transport`.

use glade_discover_protocol::{NodeId, WireMsg};
use std::future::Future;

#[cfg(feature = "conformance")]
pub mod conformance;

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
