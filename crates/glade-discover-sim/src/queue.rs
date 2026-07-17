#![cfg_attr(
    not(test),
    allow(dead_code, reason = "P4 runner consumes the P2 queue foundation")
)]

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct EventId(pub(crate) u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct MsgRef {
    pub(crate) producing_event_id: EventId,
    pub(crate) emission_index: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueError {
    InsertionSequenceExhausted,
    EmissionIndexExhausted,
}

impl fmt::Display for QueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsertionSequenceExhausted => formatter.write_str("insertion sequence exhausted"),
            Self::EmissionIndexExhausted => formatter.write_str("emission index exhausted"),
        }
    }
}

pub(crate) struct InputQueue<T> {
    entries: BTreeMap<(u64, u64), T>,
    next_insertion: u64,
}

impl<T> Default for InputQueue<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_insertion: 0,
        }
    }
}

impl<T> InputQueue<T> {
    pub(crate) fn push(&mut self, at_ms: u64, value: T) -> Result<(), QueueError> {
        let insertion = self.next_insertion;
        self.next_insertion = insertion
            .checked_add(1)
            .ok_or(QueueError::InsertionSequenceExhausted)?;
        let replaced = self.entries.insert((at_ms, insertion), value);
        debug_assert!(replaced.is_none(), "insertion sequence keys are unique");
        Ok(())
    }

    pub(crate) fn pop(&mut self) -> Option<(u64, T)> {
        self.entries
            .pop_first()
            .map(|((at_ms, _insertion), value)| (at_ms, value))
    }
}

pub(crate) struct WakeQueue<K, T> {
    entries: BTreeMap<(u64, K, u64), T>,
    next_insertion: u64,
}

impl<K, T> Default for WakeQueue<K, T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_insertion: 0,
        }
    }
}

impl<K: Ord, T> WakeQueue<K, T> {
    pub(crate) fn push(&mut self, at_mono: u64, token: K, value: T) -> Result<(), QueueError> {
        let insertion = self.next_insertion;
        self.next_insertion = insertion
            .checked_add(1)
            .ok_or(QueueError::InsertionSequenceExhausted)?;
        let replaced = self.entries.insert((at_mono, token, insertion), value);
        debug_assert!(replaced.is_none(), "insertion sequence keys are unique");
        Ok(())
    }

    pub(crate) fn pop(&mut self) -> Option<(u64, K, T)> {
        self.entries
            .pop_first()
            .map(|((at_mono, token, _insertion), value)| (at_mono, token, value))
    }
}

pub(crate) fn message_refs(
    event_id: EventId,
    emission_count: usize,
) -> Result<Vec<MsgRef>, QueueError> {
    (0..emission_count)
        .map(|index| {
            let emission_index =
                u32::try_from(index).map_err(|_| QueueError::EmissionIndexExhausted)?;
            Ok(MsgRef {
                producing_event_id: event_id,
                emission_index,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{EventId, InputQueue, MsgRef, WakeQueue, message_refs};

    #[test]
    fn simultaneous_inputs_preserve_insertion_order() {
        let mut queue = InputQueue::default();
        queue.push(7, "first").unwrap();
        queue.push(3, "earlier").unwrap();
        queue.push(7, "second").unwrap();

        assert_eq!(queue.pop(), Some((3, "earlier")));
        assert_eq!(queue.pop(), Some((7, "first")));
        assert_eq!(queue.pop(), Some((7, "second")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn simultaneous_wakeups_order_by_token_then_insertion() {
        let mut queue = WakeQueue::default();
        queue.push(9, "z-token", "z").unwrap();
        queue.push(9, "a-token", "a-first").unwrap();
        queue.push(9, "a-token", "a-second").unwrap();
        queue.push(2, "z-token", "early").unwrap();

        assert_eq!(queue.pop(), Some((2, "z-token", "early")));
        assert_eq!(queue.pop(), Some((9, "a-token", "a-first")));
        assert_eq!(queue.pop(), Some((9, "a-token", "a-second")));
        assert_eq!(queue.pop(), Some((9, "z-token", "z")));
    }

    #[test]
    fn message_references_use_event_id_and_emission_index() {
        assert_eq!(
            message_refs(EventId(42), 3).unwrap(),
            vec![
                MsgRef {
                    producing_event_id: EventId(42),
                    emission_index: 0,
                },
                MsgRef {
                    producing_event_id: EventId(42),
                    emission_index: 1,
                },
                MsgRef {
                    producing_event_id: EventId(42),
                    emission_index: 2,
                },
            ]
        );
    }
}
