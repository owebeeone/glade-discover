//! Trusted node adapters for the pure Glade discovery kernel.

pub mod append;
pub mod clock;
pub mod driver;
pub mod ingress;
pub mod transport;

/// Returns the stable role name used by workspace smoke tests.
#[must_use]
pub const fn crate_name() -> &'static str {
    "node-adapter"
}
