#![cfg_attr(
    not(test),
    allow(dead_code, reason = "P4 runner consumes the P2 fault foundation")
)]

use std::fmt;

use crate::schema::{ClockChange, ClockSpec};

const SPLITMIX_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(SPLITMIX_GAMMA);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    pub(crate) fn loss_occurs(&mut self, probability_ppm: u32) -> bool {
        self.next_u64() % 1_000_000 < u64::from(probability_ppm)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClockSample {
    pub(crate) wall_ms: i64,
    pub(crate) mono_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClockError {
    Overflow,
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("node clock arithmetic overflow")
    }
}

pub(crate) fn sample_clock(clock: &ClockSpec, sim_ms: u64) -> Result<ClockSample, ClockError> {
    let mono_ms = clock
        .initial_mono_ms
        .checked_add(sim_ms)
        .ok_or(ClockError::Overflow)?;

    let mut offset_ms = 0_i64;
    let mut selected_offset_at = None;
    let mut drift_adjustment = 0_i128;
    for change in &clock.changes {
        match *change {
            ClockChange::SetOffset {
                at_ms,
                offset_ms: candidate,
            } if at_ms <= sim_ms && selected_offset_at.is_none_or(|selected| at_ms >= selected) => {
                selected_offset_at = Some(at_ms);
                offset_ms = candidate;
            }
            ClockChange::Drift {
                start_ms,
                end_ms,
                rate_ppm,
            } if sim_ms > start_ms => {
                let elapsed_ms = sim_ms.min(end_ms).saturating_sub(start_ms);
                let numerator = i128::from(elapsed_ms) * i128::from(rate_ppm);
                drift_adjustment += numerator / 1_000_000;
            }
            ClockChange::SetOffset { .. } | ClockChange::Drift { .. } => {}
        }
    }

    let wall_ms = i128::from(clock.initial_wall_ms)
        .checked_add(i128::from(sim_ms))
        .and_then(|value| value.checked_add(i128::from(offset_ms)))
        .and_then(|value| value.checked_add(drift_adjustment))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(ClockError::Overflow)?;

    Ok(ClockSample { wall_ms, mono_ms })
}

#[cfg(test)]
mod tests {
    use crate::schema::{ClockChange, ClockSpec};

    use super::{ClockError, ClockSample, SplitMix64, sample_clock};

    #[test]
    fn splitmix64_matches_frozen_vectors() {
        let mut rng = SplitMix64::new(0);

        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
        assert_eq!(rng.next_u64(), 0x06c4_5d18_8009_454f);
        assert_eq!(rng.next_u64(), 0xf88b_b8a8_724c_81ec);
    }

    #[test]
    fn loss_consumes_one_draw_even_at_probability_extremes() {
        let mut with_checks = SplitMix64::new(9);
        assert!(!with_checks.loss_occurs(0));
        assert!(with_checks.loss_occurs(1_000_000));
        let after_checks = with_checks.next_u64();

        let mut direct = SplitMix64::new(9);
        direct.next_u64();
        direct.next_u64();
        assert_eq!(after_checks, direct.next_u64());
    }

    #[test]
    fn clock_applies_offset_and_half_open_integral_drift() {
        let clock = ClockSpec {
            initial_wall_ms: 1_000,
            initial_mono_ms: 50,
            changes: vec![
                ClockChange::SetOffset {
                    at_ms: 10,
                    offset_ms: -20,
                },
                ClockChange::Drift {
                    start_ms: 5,
                    end_ms: 15,
                    rate_ppm: 500_000,
                },
            ],
        };

        assert_eq!(
            sample_clock(&clock, 12),
            Ok(ClockSample {
                wall_ms: 995,
                mono_ms: 62,
            })
        );
        assert_eq!(
            sample_clock(&clock, 20),
            Ok(ClockSample {
                wall_ms: 1_005,
                mono_ms: 70,
            })
        );
    }

    #[test]
    fn negative_drift_truncates_toward_zero() {
        let clock = ClockSpec {
            initial_wall_ms: 0,
            initial_mono_ms: 0,
            changes: vec![ClockChange::Drift {
                start_ms: 0,
                end_ms: 3,
                rate_ppm: -500_000,
            }],
        };

        assert_eq!(sample_clock(&clock, 3).unwrap().wall_ms, 2);
    }

    #[test]
    fn clock_overflow_fails_closed() {
        let clock = ClockSpec {
            initial_wall_ms: i64::MAX,
            initial_mono_ms: u64::MAX,
            changes: Vec::new(),
        };

        assert_eq!(sample_clock(&clock, 1), Err(ClockError::Overflow));
    }
}
