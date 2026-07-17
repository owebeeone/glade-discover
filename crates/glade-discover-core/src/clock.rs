use crate::{ClockState, KernelConfig, MonoInstant, StepCtx, WallMs};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockDecision<T> {
    Ready(T),
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectiveClock {
    Ready {
        state: ClockState,
        effective_wall: WallMs,
    },
    Uncertain {
        state: ClockState,
    },
}

#[must_use]
pub fn observe(current: ClockState, ctx: StepCtx, config: &KernelConfig) -> EffectiveClock {
    match current {
        ClockState::Ready { watermark } => {
            let effective_wall = WallMs(watermark.0.max(ctx.wall.0));
            EffectiveClock::Ready {
                state: ClockState::Ready {
                    watermark: effective_wall,
                },
                effective_wall,
            }
        }
        ClockState::Uncertain { floor: Some(floor) } => {
            let Ok(resync_ms) = i64::try_from(config.clock_resync_ms) else {
                return EffectiveClock::Uncertain {
                    state: ClockState::Uncertain { floor: Some(floor) },
                };
            };
            let Some(threshold) = floor.0.checked_add(resync_ms) else {
                return EffectiveClock::Uncertain {
                    state: ClockState::Uncertain { floor: Some(floor) },
                };
            };
            if ctx.wall.0 >= threshold {
                EffectiveClock::Ready {
                    state: ClockState::Ready {
                        watermark: ctx.wall,
                    },
                    effective_wall: ctx.wall,
                }
            } else {
                EffectiveClock::Uncertain {
                    state: ClockState::Uncertain { floor: Some(floor) },
                }
            }
        }
        ClockState::Uncertain { floor: None } => EffectiveClock::Uncertain {
            state: ClockState::Uncertain { floor: None },
        },
    }
}

#[must_use]
pub const fn reseed(current: ClockState, ctx: StepCtx, watermark: WallMs) -> ClockState {
    match current {
        ClockState::Uncertain { floor: None } => ClockState::Ready {
            watermark: WallMs(if watermark.0 > ctx.wall.0 {
                watermark.0
            } else {
                ctx.wall.0
            }),
        },
        ClockState::Ready { .. } | ClockState::Uncertain { floor: Some(_) } => current,
    }
}

#[must_use]
pub fn deadline_at(
    ctx: StepCtx,
    effective_wall: WallMs,
    target_wall: WallMs,
) -> ClockDecision<MonoInstant> {
    if target_wall <= effective_wall {
        return ClockDecision::Ready(ctx.mono);
    }
    let Some(delta) = target_wall.0.checked_sub(effective_wall.0) else {
        return ClockDecision::Uncertain;
    };
    let Ok(delta) = u64::try_from(delta) else {
        return ClockDecision::Uncertain;
    };
    match ctx.mono.0.checked_add(delta) {
        Some(deadline) => ClockDecision::Ready(MonoInstant(deadline)),
        None => ClockDecision::Uncertain,
    }
}

#[must_use]
pub fn is_expired(
    lease_expiry: WallMs,
    effective_wall: WallMs,
    config: &KernelConfig,
) -> ClockDecision<bool> {
    let Some(limit) = checked_add_ms(effective_wall.0, config.skew_margin_ms) else {
        return ClockDecision::Uncertain;
    };
    ClockDecision::Ready(lease_expiry.0 <= limit)
}

#[must_use]
pub fn lease_within_limit(
    lease_expiry: WallMs,
    effective_wall: WallMs,
    config: &KernelConfig,
) -> ClockDecision<bool> {
    let Some(limit) = checked_add_ms(effective_wall.0, config.max_lease_ms)
        .and_then(|limit| checked_add_ms(limit, config.skew_margin_ms))
    else {
        return ClockDecision::Uncertain;
    };
    ClockDecision::Ready(lease_expiry.0 <= limit)
}

fn checked_add_ms(value: i64, delta: u64) -> Option<i64> {
    let delta = i64::try_from(delta).ok()?;
    value.checked_add(delta)
}
