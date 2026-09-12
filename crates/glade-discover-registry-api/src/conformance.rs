//! Fixture: admitted workspace/publisher, trusted controllable clock initially 0.
//! Real adapters must additionally prove journal integration and durable restart.
use crate::*;
pub async fn reopen_retry<W, F>(w: W, reopen: impl FnOnce(W) -> F)
where
    W: RegistryWriter + RegistryReader,
    F: Future<Output = W>,
{
    let p = publication();
    let first = w.publish(&p).await.expect("RG-007 first acceptance");
    let before = w.resolve(&query()).await.unwrap();
    assert_eq!(
        before.claims,
        vec![first.accepted.op.clone()],
        "RG-007 initial visibility"
    );
    let w = reopen(w).await;
    assert_eq!(
        w.resolve(&query()).await.unwrap().claims,
        before.claims,
        "RG-007 retained before retry"
    );
    assert_eq!(
        w.publish(&p).await,
        Ok(first),
        "RG-007 exact retry after restart"
    );
    assert_eq!(
        w.resolve(&query()).await.unwrap().claims,
        before.claims,
        "RG-007 no duplicate"
    );
}
pub fn mixed_query() -> ResolveRequest {
    let mut q = query();
    q.query.slot = glade_discover_protocol::Slot::Binding {
        share: "workspace".into(),
        glade_id: "derived".into(),
        key: b"mixed-source".to_vec(),
    };
    q
}
/// Fixture maps this derived binding to sources A+B; only A is authorized.
pub async fn mixed_source_denied<R: RegistryReader>(r: &R) {
    assert_eq!(
        r.resolve(&mixed_query()).await,
        Err(ResolveError::Denied),
        "RG-006 complete source closure required"
    );
}
pub async fn bounded<W: RegistryWriter, R: RegistryReader>(w: &W, r: &R) {
    let p = publication();
    w.publish(&p).await.unwrap();
    let mut second = p.clone();
    second.key.intent = "second".into();
    if let ClaimDraft::Workspace { node, .. } = &mut second.draft {
        *node = "provider-two".into();
    }
    w.publish(&second).await.unwrap();
    let q = query();
    let result = r.resolve(&q).await.unwrap();
    assert_eq!(result.claims.len(), 1, "RG-004 limit");
    assert!(result.truncated, "RG-004 truncation");
    let mut other = q;
    other.query.slot = glade_discover_protocol::Slot::Workspace {
        share: "other".into(),
    };
    assert!(
        r.resolve(&other).await.unwrap().claims.is_empty(),
        "RG-004 query isolation"
    );
}
pub async fn invalid_writes<W: RegistryWriter, R: RegistryReader>(w: &W, r: &R) {
    let p = publication();
    let a = w.publish(&p).await.unwrap();
    let before = r.resolve(&query()).await.unwrap();
    assert_eq!(
        w.renew(&p).await,
        Err(WriteError::InvalidRequest),
        "RG-005 cannot renew Mint"
    );
    let DirectoryRecord::ServeClaim(claim) =
        decode_directory_record(&a.accepted.op.envelope().payload).unwrap()
    else {
        panic!("fixture claim")
    };
    let mut changed = p.clone();
    changed.key.intent = "retarget".into();
    if let ClaimDraft::Workspace { node, identity, .. } = &mut changed.draft {
        *identity = ClaimIdentity::Existing(claim.claim_id);
        *node = "wrong-provider".into();
    }
    assert_eq!(
        w.renew(&changed).await,
        Err(WriteError::InvalidRequest),
        "RG-005 cannot retarget renewal"
    );
    assert_eq!(
        w.publish(&changed).await,
        Err(WriteError::InvalidRequest),
        "RG-005 cannot publish Existing"
    );
    let mut expired = p;
    expired.key.intent = "expired-new".into();
    if let ClaimDraft::Workspace {
        lease_expiry_ms, ..
    } = &mut expired.draft
    {
        *lease_expiry_ms = 0;
    }
    assert_eq!(
        w.publish(&expired).await,
        Err(WriteError::Expired),
        "RG-005 expired new request"
    );
    assert_eq!(
        r.resolve(&query()).await,
        Ok(before),
        "RG-005 no state change on rejection"
    );
}
use glade_discover_protocol::{
    ClaimId, ClaimIdentity, DirectoryRecord, RecordId, ServeClaim, decode_directory_record,
    encode_directory_record, encode_signed_op,
};
pub fn publication() -> Publication {
    let p = glade_discover_acceptance_api::conformance::proposal();
    Publication {
        key: p.key,
        draft: p.draft,
    }
}
/// Synthetic fixture record constructor, NOT a signer or registry implementation.
pub fn accepted(p: &Publication, seq: u64) -> Accepted {
    let base = glade_discover_acceptance_api::conformance::proposal();
    let mut e = base.op.envelope().clone();
    e.seq = seq;
    e.lamport = seq;
    let ClaimDraft::Workspace {
        node,
        share,
        identity,
        grant_ref,
        lease_expiry_ms,
        epoch,
    } = &p.draft
    else {
        panic!("workspace fixture only")
    };
    let claim_id = match identity {
        ClaimIdentity::Mint => ClaimId::from(RecordId {
            stream: e.stream.clone(),
            origin: e.origin.clone(),
            seq,
        }),
        ClaimIdentity::Existing(id) => id.clone(),
    };
    e.payload = encode_directory_record(&DirectoryRecord::ServeClaim(ServeClaim {
        node: node.clone(),
        share: share.clone(),
        claim_id,
        grant_ref: grant_ref.clone(),
        lease_expiry_ms: *lease_expiry_ms,
        epoch: *epoch,
    }))
    .unwrap();
    Accepted {
        key: p.key.clone(),
        draft: p.draft.clone(),
        op: encode_signed_op(&e, &[]).unwrap(),
        commit_revision: seq,
    }
}
pub fn expiry(d: &ClaimDraft) -> i64 {
    match d {
        ClaimDraft::Workspace {
            lease_expiry_ms, ..
        }
        | ClaimDraft::Service {
            lease_expiry_ms, ..
        } => *lease_expiry_ms,
    }
}
pub fn query() -> ResolveRequest {
    ResolveRequest {
        principal: "consumer".into(),
        query: RouteQuery {
            slot: publication().key.slot,
        },
        limit: NonZeroUsize::new(1).unwrap(),
    }
}
pub async fn publish_retry<W: RegistryWriter>(w: &W) {
    let p = publication();
    let first = w.publish(&p).await.expect("RG-001 publish");
    assert_eq!(first.accepted.key, p.key, "RG-001 key");
    assert_eq!(first.accepted.draft, p.draft, "RG-001 draft");
    assert_eq!(w.publish(&p).await, Ok(first), "RG-001 exact retry");
    let mut c = p;
    if let ClaimDraft::Workspace { epoch, .. } = &mut c.draft {
        *epoch += 1;
    }
    assert_eq!(
        w.publish(&c).await,
        Err(WriteError::RequestMismatch),
        "RG-001 changed request"
    );
}
pub async fn renew_and_expire<W: RegistryWriter, R: RegistryReader>(
    w: &W,
    r: &R,
    set_time: impl Fn(i64),
) {
    let mut p = publication();
    if let ClaimDraft::Workspace {
        lease_expiry_ms, ..
    } = &mut p.draft
    {
        *lease_expiry_ms = 10;
    }
    let first = w.publish(&p).await.unwrap();
    let DirectoryRecord::ServeClaim(original) =
        decode_directory_record(&first.accepted.op.envelope().payload).unwrap()
    else {
        panic!("RG-002 kind")
    };
    let mut renewed = p.clone();
    renewed.key.intent = "renew".into();
    if let ClaimDraft::Workspace {
        identity,
        lease_expiry_ms,
        ..
    } = &mut renewed.draft
    {
        *identity = ClaimIdentity::Existing(original.claim_id.clone());
        *lease_expiry_ms = 20;
    }
    let receipt = w.renew(&renewed).await.expect("RG-002 renewal");
    let DirectoryRecord::ServeClaim(next) =
        decode_directory_record(&receipt.accepted.op.envelope().payload).unwrap()
    else {
        panic!("RG-002 kind")
    };
    assert_eq!(next.claim_id, original.claim_id, "RG-002 identity");
    assert_eq!(next.lease_expiry_ms, 20, "RG-002 exact expiry");
    let fresh = r.resolve(&query()).await.unwrap();
    assert_eq!(
        fresh.claims,
        vec![receipt.accepted.op.clone()],
        "RG-002 superseded claim omitted before expiry"
    );
    set_time(10);
    let result = r.resolve(&query()).await.unwrap();
    assert_eq!(result.observed_at_ms, 10, "RG-002 trusted clock");
    assert_eq!(
        result.claims,
        vec![receipt.accepted.op],
        "RG-002 renewed claim"
    );
    set_time(20);
    assert!(
        r.resolve(&query()).await.unwrap().claims.is_empty(),
        "RG-002 expiry boundary"
    );
    assert_eq!(w.publish(&p).await, Ok(first), "RG-002 retry never renews");
}
pub async fn empty<R: RegistryReader>(r: &R) {
    let a = r.resolve(&query()).await.expect("RG-003 known local empty");
    assert!(a.claims.is_empty(), "RG-003 empty");
    assert!(!a.truncated, "RG-003 empty not truncated");
}
pub async fn read_error<R: RegistryReader>(r: &R, error: ResolveError) {
    assert_eq!(
        r.resolve(&query()).await,
        Err(error),
        "RG-003 error not absence"
    );
}
