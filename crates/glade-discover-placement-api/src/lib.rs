//! Draft shard-location seam. No DHT, consensus or namespace proof wire schema.
use glade_discover_protocol::{NodeId, Principal, SignedOp};
use std::{
    future::Future,
    num::{NonZeroU16, NonZeroUsize},
};
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocateRequest {
    pub namespace: String,
    pub key: Vec<u8>,
    pub limit: NonZeroUsize,
    pub max_referrals: NonZeroU16,
    pub prefer: Option<NodeId>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryCandidate {
    pub node: NodeId,
    pub authority: Vec<SignedOp>,
}
/// Trusted locator output, NOT a transferable proof or permission to write a source.
/// Evidence is preserved for inspection; remote serialized structs are untrusted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Location {
    pub namespace: String,
    pub key: Vec<u8>,
    pub shard: String,
    pub authority: Principal,
    pub mapping_epoch: u64,
    pub registries: Vec<RegistryCandidate>,
    pub observed_at_ms: i64,
    pub valid_until_ms: i64,
    pub referrals_used: u16,
    pub truncated: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocateError {
    UnknownNamespace,
    Unavailable,
    Denied,
    StaleMapping,
    Unverifiable,
    ClockUncertain,
    BudgetExceeded,
}
/// ```compile_fail
/// use glade_discover_placement_api::ShardLocator;
/// struct Missing;
/// impl ShardLocator for Missing {}
/// ```
pub trait ShardLocator: Send + Sync {
    /// PL-001: bind exact canonical namespace/key, return 1..=limit distinct nodes,
    /// and enforce referral budget (including cycles/retries). Preference is a
    /// locality hint, not authority. Truncation is explicit; no global index implied.
    /// PL-002: MUST verify namespace/shard/node delegation against configured
    /// authoritative fold/anchor, not just any valid signature. Authority profile
    /// and canonical namespace/proof schema MUST be settled before implementation;
    /// this trait does not invent new v3.1 grant verbs or a consensus protocol.
    /// PL-003: current trusted clock must precede valid_until, bounded by every
    /// delegation's expiry; stale/unverifiable/uncertain results fail closed.
    /// PL-004: unavailable is not unknown namespace or revocation. Listed nodes
    /// are authorized candidates, not proof of reachability, leadership or fencing.
    /// Futures MUST be lazy; budgets bound work, not an implicit wall-clock timeout.
    fn locate(
        &self,
        request: &LocateRequest,
    ) -> impl Future<Output = Result<Location, LocateError>> + Send;
}
