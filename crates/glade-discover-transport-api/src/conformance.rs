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
