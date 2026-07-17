use glade_discover_core::{
    ClockState, Effect, KernelConfig, MonoInstant, PersistedState, State, StepCtx, WallMs,
    WatermarkLoad,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockSample {
    pub wall: WallMs,
    pub mono: MonoInstant,
}

impl ClockSample {
    #[must_use]
    pub const fn ctx(self) -> StepCtx {
        StepCtx {
            wall: self.wall,
            mono: self.mono,
        }
    }
}

pub trait ClockHost {
    type Error;

    fn load_watermark(&mut self) -> Result<WatermarkLoad, Self::Error>;
    fn persist_watermark(&mut self, watermark: WallMs) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClockError<E> {
    Host(E),
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClockRestore {
    pub state: State,
    pub recovery: Vec<Effect>,
}

pub fn restore<H: ClockHost>(
    host: &mut H,
    config: KernelConfig,
    persisted: PersistedState,
    sample: ClockSample,
) -> Result<ClockRestore, ClockError<H::Error>> {
    let watermark = host.load_watermark().map_err(ClockError::Host)?;
    let prior = match watermark {
        WatermarkLoad::Readable(watermark) => Some(watermark),
        WatermarkLoad::Unreadable => None,
    };
    let (state, recovery) =
        glade_discover_core::restore_with_recovery(config, persisted, watermark, sample.ctx());
    if let ClockState::Ready { watermark } = state.clock() {
        if prior.is_none_or(|prior| watermark > prior) {
            host.persist_watermark(watermark)
                .map_err(ClockError::Host)?;
        }
    }
    Ok(ClockRestore { state, recovery })
}
