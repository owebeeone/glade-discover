//! Pure deterministic state machine for Glade discovery.

mod model;

pub mod append;
pub mod authority;
pub mod claims;
pub mod clock;
pub mod ingest;
pub mod projection;
pub mod routing;
pub mod sync;

pub use ingest::{GovernanceVerdict, IngestDisposition, LiveCapabilityVerdict, StructuralVerdict};
pub use model::{
    ClaimCommand, ClaimMode, ClockState, Effect, Event, KernelConfig, MineStatus, MonoInstant,
    NodePrincipalBinding, OwnClaim, PersistedState, PrincipalCtx, PrincipalPlane, RoundProgress,
    RouteAns, State, StepCtx, Transition, VerificationBatch, VerificationResult, WakeClass,
    WakeToken, WallMs, WallScheduleError, WatermarkLoad, prepare_wall_schedule, restore_fresh,
    restore_with_recovery, step,
};

/// Returns the stable role name used by workspace smoke tests.
#[must_use]
pub const fn crate_name() -> &'static str {
    "core"
}
