//! Draft shard-location seam. No DHT, consensus or namespace proof wire schema.
use glade_discover_protocol::{NodeId, Principal, SignedOp};
use std::{
    future::Future,
    num::{NonZeroU16, NonZeroUsize},
};
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

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Fixture namespace/workspace, authoritative mapping epoch 1, trusted time 0,
    //! delegation expiry 20, one local registry. Synthetic evidence is NOT a grant.
    use crate::*;
    use glade_discover_protocol::{OpEnvelope, Shape, StreamId, encode_signed_op};
    pub fn request() -> LocateRequest {
        LocateRequest {
            namespace: "workspace".into(),
            key: vec![1],
            limit: NonZeroUsize::new(1).unwrap(),
            max_referrals: NonZeroU16::new(2).unwrap(),
            prefer: Some("registry".into()),
        }
    }
    pub fn location(q: &LocateRequest) -> Location {
        let op = encode_signed_op(
            &OpEnvelope {
                stream: StreamId::default(),
                origin: "authority".into(),
                seq: 1,
                prev: None,
                lamport: 1,
                refs: vec![],
                shape: Shape::Value,
                payload: b"fixture-only; not an authority schema".to_vec(),
            },
            &[],
        )
        .unwrap();
        Location {
            namespace: q.namespace.clone(),
            key: q.key.clone(),
            shard: "shard-1".into(),
            authority: "authority".into(),
            mapping_epoch: 1,
            registries: vec![RegistryCandidate {
                node: "registry".into(),
                authority: vec![op],
            }],
            observed_at_ms: 0,
            valid_until_ms: 20,
            referrals_used: 1,
            truncated: false,
        }
    }
    pub async fn bounded<L: ShardLocator>(l: &L) {
        let q = request();
        let a = l.locate(&q).await.expect("PL-001 mapping");
        assert_eq!(a.namespace, q.namespace, "PL-001 namespace");
        assert_eq!(a.key, q.key, "PL-001 key");
        assert!(
            !a.registries.is_empty() && a.registries.len() <= q.limit.get(),
            "PL-001 candidate bound"
        );
        let unique: std::collections::BTreeSet<_> = a.registries.iter().map(|r| &r.node).collect();
        assert_eq!(unique.len(), a.registries.len(), "PL-001 duplicate nodes");
        assert!(
            a.referrals_used <= q.max_referrals.get(),
            "PL-001 referral bound"
        );
        assert_eq!(
            a.authority,
            Principal::from("authority"),
            "PL-002 authority binding"
        );
        assert!(
            a.registries.iter().all(|r| !r.authority.is_empty()),
            "PL-002 retained evidence"
        );
        assert_eq!(a.shard, "shard-1", "PL-002 configured shard");
        assert_eq!(a.mapping_epoch, 1, "PL-002 configured mapping epoch");
        assert_eq!(
            a.registries[0].node,
            NodeId::from("registry"),
            "PL-002 authorized registry, not copied evidence"
        );
        assert_eq!(a.valid_until_ms, 20, "PL-003 expiry bound");
        assert!(a.observed_at_ms < a.valid_until_ms, "PL-003 freshness");
    }
    pub async fn expires<L: ShardLocator>(l: &L, expire: impl FnOnce()) {
        bounded(l).await;
        expire();
        assert_eq!(
            l.locate(&request()).await,
            Err(LocateError::StaleMapping),
            "PL-003 expiry boundary"
        );
    }
    pub async fn error<L: ShardLocator>(l: &L, error: LocateError) {
        assert_eq!(
            l.locate(&request()).await,
            Err(error),
            "PL-004 explicit failure"
        );
    }
}
