use glade_discover_core::{Effect, Event, VerificationBatch};
use glade_discover_protocol::{NodeId, WireMsg};

/// Synchronous transport boundary injected by the trusted node. Implementors
/// may perform I/O, but transport behavior never enters the pure kernel.
pub trait Transport {
    type Error;

    fn send(&mut self, to: &NodeId, message: &WireMsg) -> Result<(), Self::Error>;
}

/// Observable outcome of interpreting one addressed gossip effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GossipSend<E> {
    Sent { to: NodeId },
    Failed { to: NodeId, error: E },
}

/// Converts a message whose verification has already completed at the trusted
/// node into the kernel's explicit, non-wire delivery input.
#[must_use]
pub fn inbound_deliver(from: NodeId, message: WireMsg, verification: VerificationBatch) -> Event {
    Event::Deliver {
        from,
        msg: Box::new(message),
        verification,
    }
}

/// Interprets only `Gossip` effects. The function deliberately has no access to
/// kernel state: send failure can be reported, but cannot roll back or mutate a
/// state that the pure step already produced and the host already committed.
pub fn dispatch_gossip<T: Transport>(
    transport: &mut T,
    effect: &Effect,
) -> Option<GossipSend<T::Error>> {
    let Effect::Gossip { to, msg } = effect else {
        return None;
    };
    Some(match transport.send(to, msg) {
        Ok(()) => GossipSend::Sent { to: to.clone() },
        Err(error) => GossipSend::Failed {
            to: to.clone(),
            error,
        },
    })
}
