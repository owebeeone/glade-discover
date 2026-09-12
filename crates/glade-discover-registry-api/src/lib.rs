//! Draft discovery-facing API. No registry implementation or wire schema change.
use glade_discover_acceptance_api::{Accepted, RequestKey};
use glade_discover_protocol::{ClaimDraft, NodeId, Principal, RouteQuery, SignedOp};
use std::{future::Future, num::NonZeroUsize};
#[cfg(feature = "conformance")]
pub mod conformance;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Publication {
    pub key: RequestKey,
    pub draft: ClaimDraft,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationReceipt {
    pub registry: NodeId,
    pub accepted: Accepted,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteError {
    Denied,
    Expired,
    InvalidRequest,
    RequestMismatch,
    Unavailable,
    OutcomeUnknown,
    ClockUncertain,
    Capacity,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveRequest {
    pub principal: Principal,
    pub query: RouteQuery,
    pub limit: NonZeroUsize,
}
/// ALWAYS partial local knowledge, even when empty and not truncated. Never global
/// absence, provider reachability, source fencing, or replication confirmation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolution {
    pub registry: NodeId,
    pub observed_at_ms: i64,
    pub claims: Vec<SignedOp>,
    pub truncated: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveError {
    Denied,
    Unavailable,
    ClockUncertain,
}
/// Trusted in-process port: authenticated principals come from ingress, not wire
/// assertions. Futures MUST be lazy; dropping polled writes leaves unknown outcome.
/// ```compile_fail
/// use glade_discover_registry_api::RegistryWriter;
/// struct Missing;
/// impl RegistryWriter for Missing {}
/// ```
pub trait RegistryWriter: Send + Sync {
    /// RG-001: Mint identity required. Validate exact slot/draft/requester binding,
    /// current authority and trusted clock. Success MUST be backed by the atomic
    /// journal before gossip. Identical key+draft retry returns original bytes even
    /// after expiry/restart, subject to current authorization, without a new lease.
    /// Changed draft => RequestMismatch. Known errors MUST NOT consume the key.
    fn publish(
        &self,
        request: &Publication,
    ) -> impl Future<Output = Result<PublicationReceipt, WriteError>> + Send;
    /// RG-002: Existing identity required, with a NEW intent for a new renewal.
    /// Identity, target and publisher authority MUST bind the previous claim;
    /// MUST NOT silently mint/retarget it. Expiry <= trusted now rejects NEW
    /// acceptance. Replay/forwarding never extends expiry. Re-evaluate policy.
    fn renew(
        &self,
        request: &Publication,
    ) -> impl Future<Output = Result<PublicationReceipt, WriteError>> + Send;
}
/// ```compile_fail
/// use glade_discover_registry_api::RegistryReader;
/// struct Missing;
/// impl RegistryReader for Missing {}
/// ```
pub trait RegistryReader: Send + Sync {
    /// RG-002/003: current authorized fold; omit superseded/expired claims (expiry
    /// == now is expired), bind exact query and complete source closure, enforce
    /// limit/report truncation. Validate signatures and authority before exposing
    /// topology. Unknown time/unavailability/denial MUST NOT be empty success.
    /// Uses trusted local time, not caller time. Performs no service instantiation.
    /// Futures are lazy; results can become stale immediately after evaluation.
    fn resolve(
        &self,
        request: &ResolveRequest,
    ) -> impl Future<Output = Result<Resolution, ResolveError>> + Send;
}
