#![cfg_attr(
    not(test),
    allow(dead_code, reason = "P4 runner consumes the P2 network foundation")
)]

use std::collections::BTreeMap;
use std::fmt;

use crate::faults::SplitMix64;
use crate::queue::MsgRef;
use crate::schema::{LinkFault, LinkSpec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeliveryPlan {
    pub(crate) at_ms: u64,
    pub(crate) msg_ref: MsgRef,
    pub(crate) copy_index: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkError {
    DuplicateLink,
    UnknownLink,
    ArithmeticOverflow,
    CopyIndexExhausted,
    DeliveryBudgetExceeded,
}

impl fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateLink => formatter.write_str("duplicate bidirectional link"),
            Self::UnknownLink => formatter.write_str("unknown link"),
            Self::ArithmeticOverflow => formatter.write_str("network time arithmetic overflow"),
            Self::CopyIndexExhausted => formatter.write_str("network copy index exhausted"),
            Self::DeliveryBudgetExceeded => {
                formatter.write_str("network delivery expansion exceeds event budget")
            }
        }
    }
}

pub(crate) struct Network {
    links: BTreeMap<(String, String), LinkSpec>,
}

impl Network {
    pub(crate) fn new(links: Vec<LinkSpec>) -> Result<Self, NetworkError> {
        let mut indexed = BTreeMap::new();
        for link in links {
            let key = link_key(&link.a, &link.b);
            if indexed.insert(key, link).is_some() {
                return Err(NetworkError::DuplicateLink);
            }
        }
        Ok(Self { links: indexed })
    }

    pub(crate) fn plan(
        &self,
        from: &str,
        to: &str,
        sent_at_ms: u64,
        msg_ref: MsgRef,
        rng: &mut SplitMix64,
        delivery_budget: u64,
    ) -> Result<Vec<DeliveryPlan>, NetworkError> {
        let link = self
            .links
            .get(&link_key(from, to))
            .ok_or(NetworkError::UnknownLink)?;
        let mut latency_ms = link.latency_ms;
        let mut reorder_delay_ms = 0_u64;
        let mut copy_offsets = vec![0_u64];
        let mut dropped = false;

        for fault in &link.faults {
            if !is_active(fault, sent_at_ms) {
                continue;
            }
            match *fault {
                LinkFault::Loss {
                    probability_ppm, ..
                } => dropped |= rng.loss_occurs(probability_ppm),
                LinkFault::Partition { .. } => dropped = true,
                LinkFault::Reorder { extra_delay_ms, .. } => {
                    reorder_delay_ms = reorder_delay_ms
                        .checked_add(extra_delay_ms)
                        .ok_or(NetworkError::ArithmeticOverflow)?;
                }
                LinkFault::Duplicate {
                    copies, spacing_ms, ..
                } => {
                    let expanded = u64::try_from(copy_offsets.len())
                        .ok()
                        .and_then(|current| {
                            current.checked_mul(u64::from(copies.get()).checked_add(1)?)
                        })
                        .ok_or(NetworkError::DeliveryBudgetExceeded)?;
                    if expanded > delivery_budget {
                        return Err(NetworkError::DeliveryBudgetExceeded);
                    }
                    let originals = copy_offsets.clone();
                    for offset in originals {
                        for copy in 1..=copies.get() {
                            let spacing = spacing_ms
                                .checked_mul(u64::from(copy))
                                .ok_or(NetworkError::ArithmeticOverflow)?;
                            copy_offsets.push(
                                offset
                                    .checked_add(spacing)
                                    .ok_or(NetworkError::ArithmeticOverflow)?,
                            );
                        }
                    }
                }
                LinkFault::Latency {
                    latency_ms: replacement,
                    ..
                } => latency_ms = replacement,
            }
        }

        if dropped {
            return Ok(Vec::new());
        }

        let base_delivery_ms = sent_at_ms
            .checked_add(latency_ms)
            .and_then(|value| value.checked_add(reorder_delay_ms))
            .ok_or(NetworkError::ArithmeticOverflow)?;
        copy_offsets
            .into_iter()
            .enumerate()
            .map(|(copy_index, offset)| {
                Ok(DeliveryPlan {
                    at_ms: base_delivery_ms
                        .checked_add(offset)
                        .ok_or(NetworkError::ArithmeticOverflow)?,
                    msg_ref,
                    copy_index: u32::try_from(copy_index)
                        .map_err(|_| NetworkError::CopyIndexExhausted)?,
                })
            })
            .collect()
    }
}

fn link_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

const fn is_active(fault: &LinkFault, at_ms: u64) -> bool {
    let (start_ms, end_ms) = match *fault {
        LinkFault::Loss {
            start_ms, end_ms, ..
        }
        | LinkFault::Partition { start_ms, end_ms }
        | LinkFault::Reorder {
            start_ms, end_ms, ..
        }
        | LinkFault::Duplicate {
            start_ms, end_ms, ..
        }
        | LinkFault::Latency {
            start_ms, end_ms, ..
        } => (start_ms, end_ms),
    };
    start_ms <= at_ms && at_ms < end_ms
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use crate::faults::SplitMix64;
    use crate::queue::{EventId, MsgRef};
    use crate::schema::{LinkFault, LinkSpec};

    use super::{DeliveryPlan, Network};

    fn reference() -> MsgRef {
        MsgRef {
            producing_event_id: EventId(4),
            emission_index: 2,
        }
    }

    #[test]
    fn links_are_bidirectional() {
        let network = Network::new(vec![LinkSpec {
            a: "a".to_owned(),
            b: "b".to_owned(),
            latency_ms: 5,
            faults: Vec::new(),
        }])
        .unwrap();
        let mut rng = SplitMix64::new(0);

        assert_eq!(
            network
                .plan("b", "a", 10, reference(), &mut rng, 10)
                .unwrap(),
            vec![DeliveryPlan {
                at_ms: 15,
                msg_ref: reference(),
                copy_index: 0,
            }]
        );
    }

    #[test]
    fn active_faults_apply_in_frozen_declaration_order() {
        let network = Network::new(vec![LinkSpec {
            a: "a".to_owned(),
            b: "b".to_owned(),
            latency_ms: 5,
            faults: vec![
                LinkFault::Latency {
                    start_ms: 0,
                    end_ms: 100,
                    latency_ms: 20,
                },
                LinkFault::Reorder {
                    start_ms: 0,
                    end_ms: 100,
                    extra_delay_ms: 3,
                },
                LinkFault::Latency {
                    start_ms: 0,
                    end_ms: 100,
                    latency_ms: 7,
                },
                LinkFault::Duplicate {
                    start_ms: 0,
                    end_ms: 100,
                    copies: NonZeroU16::new(2).unwrap(),
                    spacing_ms: 4,
                },
            ],
        }])
        .unwrap();
        let mut rng = SplitMix64::new(1);

        assert_eq!(
            network
                .plan("a", "b", 10, reference(), &mut rng, 10)
                .unwrap(),
            vec![
                DeliveryPlan {
                    at_ms: 20,
                    msg_ref: reference(),
                    copy_index: 0,
                },
                DeliveryPlan {
                    at_ms: 24,
                    msg_ref: reference(),
                    copy_index: 1,
                },
                DeliveryPlan {
                    at_ms: 28,
                    msg_ref: reference(),
                    copy_index: 2,
                },
            ]
        );
    }

    #[test]
    fn partition_drops_but_all_active_loss_faults_still_draw() {
        let network = Network::new(vec![LinkSpec {
            a: "a".to_owned(),
            b: "b".to_owned(),
            latency_ms: 0,
            faults: vec![
                LinkFault::Partition {
                    start_ms: 0,
                    end_ms: 10,
                },
                LinkFault::Loss {
                    start_ms: 0,
                    end_ms: 10,
                    probability_ppm: 0,
                },
                LinkFault::Loss {
                    start_ms: 0,
                    end_ms: 10,
                    probability_ppm: 0,
                },
            ],
        }])
        .unwrap();
        let mut with_plan = SplitMix64::new(13);

        assert!(
            network
                .plan("a", "b", 1, reference(), &mut with_plan, 10)
                .unwrap()
                .is_empty()
        );

        let mut direct = SplitMix64::new(13);
        direct.next_u64();
        direct.next_u64();
        assert_eq!(with_plan.next_u64(), direct.next_u64());
    }

    #[test]
    fn duplicate_expansion_is_rejected_before_exceeding_the_delivery_budget() {
        let network = Network::new(vec![LinkSpec {
            a: "a".to_owned(),
            b: "b".to_owned(),
            latency_ms: 0,
            faults: vec![
                LinkFault::Duplicate {
                    start_ms: 0,
                    end_ms: 10,
                    copies: NonZeroU16::new(100).unwrap(),
                    spacing_ms: 0,
                },
                LinkFault::Duplicate {
                    start_ms: 0,
                    end_ms: 10,
                    copies: NonZeroU16::new(100).unwrap(),
                    spacing_ms: 0,
                },
            ],
        }])
        .unwrap();
        let mut rng = SplitMix64::new(1);

        assert_eq!(
            network.plan("a", "b", 0, reference(), &mut rng, 10),
            Err(super::NetworkError::DeliveryBudgetExceeded)
        );
    }

    #[test]
    fn half_open_fault_is_inactive_at_end() {
        let network = Network::new(vec![LinkSpec {
            a: "a".to_owned(),
            b: "b".to_owned(),
            latency_ms: 2,
            faults: vec![LinkFault::Partition {
                start_ms: 1,
                end_ms: 5,
            }],
        }])
        .unwrap();
        let mut rng = SplitMix64::new(0);

        assert_eq!(
            network
                .plan("a", "b", 5, reference(), &mut rng, 10)
                .unwrap()[0]
                .at_ms,
            7
        );
    }
}
