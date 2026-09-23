//! Draft per-stream atomic acceptance journal, NOT a database implementation.
//! Trusted admission MUST validate signature, payload, authority and clock first.
use glade_discover_protocol::{
    ClaimDraft, Generation, IntentId, Principal, SignedOp, Slot, StreamId,
};
use std::future::Future;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamScope {
    pub stream: StreamId,
    pub origin: Principal,
}
/// Requester MUST come from authenticated ingress, not an asserted wire identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RequestKey {
    pub requester: Principal,
    pub slot: Slot,
    pub generation: Generation,
    pub intent: IntentId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub seq: u64,
    pub hash: [u8; 32],
    pub lamport: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub expected_revision: u64,
    pub key: RequestKey,
    pub draft: ClaimDraft,
    pub op: SignedOp,
    pub watermark_ms: i64,
}
/// Local durable receipt, not replica confirmation or current discoverability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Accepted {
    pub key: RequestKey,
    pub draft: ClaimDraft,
    pub op: SignedOp,
    pub commit_revision: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    pub revision: u64,
    pub cursor: Option<Cursor>,
    pub watermark_ms: Option<i64>,
    pub pending: Vec<Accepted>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    Unavailable,
    Corrupt,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitError {
    Conflict,
    RequestMismatch,
    InvalidOperation,
    ClockRegression,
    Unavailable,
    OutcomeUnknown,
    Capacity,
    Exhausted,
}
/// Mutations MUST be serialized per scope. Futures MUST be lazy; dropping a polled
/// mutation means unknown outcome, not rollback or completed cleanup.
/// ```compile_fail
/// use glade_discover_acceptance_api::AcceptanceJournal;
/// struct Missing;
/// impl AcceptanceJournal for Missing {}
/// ```
pub trait AcceptanceJournal: Send + Sync {
    /// Immutable local stream/origin ownership for this handle.
    fn scope(&self) -> StreamScope;
    /// AC-001/003: authenticated consistent snapshot. Unreadable/corrupt MUST NOT
    /// become empty. Readable fresh scope has revision 0, no cursor/watermark.
    /// Initial watermark absence is not clock certainty: trusted admission must
    /// establish it. Implementations MUST document bounded pending/retained storage
    /// and reject new work at capacity; this draft permits no silent eviction/GC.
    fn recover(&self) -> impl Future<Output = Result<Recovery, ReadError>> + Send;
    /// Deduplication survives reopen and acknowledgement; errors are not absence.
    fn lookup(
        &self,
        key: &RequestKey,
    ) -> impl Future<Output = Result<Option<Accepted>, ReadError>> + Send;
    /// AC-001: atomically persist key+draft+EXACT signed bytes, increment revision,
    /// advance cursor and non-regressing watermark, and enqueue a durable handoff.
    /// Success MUST follow the specified local durability barrier, before gossip.
    /// AC-004: exact key+draft+op replay returns original Accepted BEFORE checking
    /// CAS, even after ack/clock advance. Changed draft/bytes => RequestMismatch.
    /// New keys require matching expected_revision and valid scope, next seq
    /// (initial 1), prev hash, increasing lamport, finalized draft/slot binding.
    /// Overflow fails closed. Every known error MUST leave the scope unchanged.
    /// AC-002/003: crashes expose whole commit or none. Lost replies are recovered
    /// by lookup/recovery; never blindly allocate another operation on retry.
    fn commit(
        &self,
        proposal: &Proposal,
    ) -> impl Future<Output = Result<Accepted, CommitError>> + Send;
    /// AC-005: AFTER downstream durable fold/outbox commit, atomically remove
    /// pending handoff and advance revision; retain dedup/cursor/watermark.
    /// Already acknowledged key => current revision, no write. Pending key requires
    /// matching revision; unknown key => InvalidOperation. Delivery is at least
    /// once; consumer dedup is mandatory. Derived kernel snapshot is a separate
    /// downstream transaction, not part of this write-ahead journal's schema.
    fn acknowledge(
        &self,
        key: &RequestKey,
        expected_revision: u64,
    ) -> impl Future<Output = Result<u64, CommitError>> + Send;
}

// The conformance probes exist only with the `conformance` feature. The
// condition sits on this braced module, which encloses the whole conditional
// section: never a bare `#[cfg]` on a single declaration, so deleting or
// moving a declaration cannot hand the condition to the next one.
#[cfg(feature = "conformance")]
pub mod conformance {
    //! Empty admitted fixture scope, clock 10. Snapshot-copy reopen tests assertions,
    //! not real durability. Actual adapters MUST inject real commit/crash boundaries.
    use crate::*;
    pub async fn competing<J: AcceptanceJournal>(j: &J) {
        let first = proposal();
        let mut second = first.clone();
        second.key.intent = "competing".into();
        let one = j.commit(&first);
        let two = j.commit(&second);
        let winner = one.await.expect("AC-008 first CAS wins");
        assert_eq!(
            two.await,
            Err(CommitError::Conflict),
            "AC-008 stale competing intent"
        );
        assert_eq!(
            j.lookup(&second.key).await,
            Ok(None),
            "AC-008 loser key not consumed"
        );
        assert_committed(j, &first, &winner).await;
    }
    pub async fn ack_recover<J, F>(j: J, reopen: impl FnOnce(J) -> F, lose_reply: bool)
    where
        J: AcceptanceJournal,
        F: Future<Output = J>,
    {
        let p = proposal();
        let a = j.commit(&p).await.unwrap();
        let before = j.recover().await.unwrap();
        assert_eq!(
            j.acknowledge(&p.key, 1).await,
            if lose_reply {
                Err(CommitError::OutcomeUnknown)
            } else {
                Ok(2)
            },
            "AC-006 acknowledgement boundary"
        );
        let j = reopen(j).await;
        let after = j.recover().await.unwrap();
        assert_eq!(after.revision, 2, "AC-006 revision");
        assert!(after.pending.is_empty(), "AC-006 no handoff resurrection");
        assert_eq!(after.cursor, before.cursor, "AC-006 cursor retained");
        assert_eq!(
            after.watermark_ms, before.watermark_ms,
            "AC-006 watermark retained"
        );
        assert_eq!(
            j.lookup(&p.key).await,
            Ok(Some(a.clone())),
            "AC-006 dedup retained"
        );
        assert_eq!(
            j.acknowledge(&p.key, 1).await,
            Ok(2),
            "AC-006 lost ack retry"
        );
        assert_eq!(j.commit(&p).await, Ok(a), "AC-006 publish retry");
        assert!(
            j.recover().await.unwrap().pending.is_empty(),
            "AC-006 no resend on retry"
        );
    }
    pub async fn invalid_proposals<J: AcceptanceJournal>(j: &J) {
        let before = j.recover().await.unwrap();
        for mutation in 0..7 {
            let mut p = proposal();
            let mut e = p.op.envelope().clone();
            match mutation {
                0 => e.stream.share = "wrong".into(),
                1 => e.origin = "wrong".into(),
                2 => e.lamport = 0,
                3 => e.prev = Some([9; 32]),
                4 => {
                    p.key.slot = Slot::Workspace {
                        share: "wrong".into(),
                    }
                }
                5 => {
                    if let ClaimDraft::Workspace { epoch, .. } = &mut p.draft {
                        *epoch += 1;
                    }
                }
                _ => e.payload.push(99),
            }
            p.op = encode_signed_op(&e, p.op.signature()).unwrap();
            assert_eq!(
                j.commit(&p).await,
                Err(CommitError::InvalidOperation),
                "AC-007 invalid proposal {mutation}"
            );
            assert_eq!(
                j.recover().await,
                Ok(before.clone()),
                "AC-007 no partial mutation"
            );
            assert_eq!(
                j.lookup(&p.key).await,
                Ok(None),
                "AC-007 rejected key not consumed"
            );
        }
    }
    use glade_discover_protocol::{
        ClaimId, ClaimIdentity, DirectoryRecord, GrantId, OpEnvelope, RecordId, ServeClaim, Shape,
        encode_directory_record, encode_signed_op, op_hash,
    };
    pub fn scope() -> StreamScope {
        StreamScope {
            stream: StreamId {
                share: "workspace".into(),
                glade_id: "directory".into(),
                key: vec![],
            },
            origin: "writer".into(),
        }
    }
    pub fn proposal() -> Proposal {
        let s = scope();
        let id = RecordId {
            stream: s.stream.clone(),
            origin: s.origin.clone(),
            seq: 1,
        };
        let key = RequestKey {
            requester: "publisher".into(),
            slot: Slot::Workspace {
                share: "workspace".into(),
            },
            generation: Generation(1),
            intent: "publish".into(),
        };
        let grant = GrantId::from(RecordId {
            seq: 0,
            ..id.clone()
        });
        let draft = ClaimDraft::Workspace {
            node: "provider".into(),
            share: "workspace".into(),
            identity: ClaimIdentity::Mint,
            grant_ref: grant.clone(),
            lease_expiry_ms: 100,
            epoch: 1,
        };
        let payload = encode_directory_record(&DirectoryRecord::ServeClaim(ServeClaim {
            node: "provider".into(),
            share: "workspace".into(),
            claim_id: ClaimId::from(id),
            grant_ref: grant,
            lease_expiry_ms: 100,
            epoch: 1,
        }))
        .unwrap();
        let op = encode_signed_op(
            &OpEnvelope {
                stream: s.stream,
                origin: s.origin,
                seq: 1,
                prev: None,
                lamport: 1,
                refs: vec![],
                shape: Shape::Log,
                payload,
            },
            &[],
        )
        .unwrap();
        Proposal {
            expected_revision: 0,
            key,
            draft,
            op,
            watermark_ms: 10,
        }
    }
    pub async fn assert_committed<J: AcceptanceJournal>(j: &J, p: &Proposal, a: &Accepted) {
        assert_eq!(
            a,
            &Accepted {
                key: p.key.clone(),
                draft: p.draft.clone(),
                op: p.op.clone(),
                commit_revision: 1
            },
            "AC-001 receipt"
        );
        assert_eq!(
            j.recover().await,
            Ok(Recovery {
                revision: 1,
                cursor: Some(Cursor {
                    seq: 1,
                    hash: op_hash(&p.op),
                    lamport: 1
                }),
                watermark_ms: Some(10),
                pending: vec![a.clone()]
            }),
            "AC-001 atomic snapshot"
        );
        assert_eq!(j.lookup(&p.key).await, Ok(Some(a.clone())), "AC-001 lookup");
    }
    pub async fn accept_retry_recover<J, F>(j: J, reopen: impl FnOnce(J) -> F)
    where
        J: AcceptanceJournal,
        F: Future<Output = J>,
    {
        let p = proposal();
        assert_eq!(j.scope(), scope());
        let a = j.commit(&p).await.expect("AC-001 acceptance");
        assert_committed(&j, &p, &a).await;
        let j = reopen(j).await;
        assert_committed(&j, &p, &a).await;
        assert_eq!(j.commit(&p).await, Ok(a.clone()), "AC-001 retry");
        assert_committed(&j, &p, &a).await;
    }
    pub async fn interrupted<J, F>(j: J, reopen: impl FnOnce(J) -> F, committed: bool)
    where
        J: AcceptanceJournal,
        F: Future<Output = J>,
    {
        let p = proposal();
        let before = j.recover().await.unwrap();
        assert_eq!(
            j.commit(&p).await,
            Err(if committed {
                CommitError::OutcomeUnknown
            } else {
                CommitError::Unavailable
            }),
            "AC-002/003 injected boundary"
        );
        let j = reopen(j).await;
        if committed {
            let a = j.lookup(&p.key).await.unwrap().expect("AC-003 recovered");
            assert_committed(&j, &p, &a).await;
            assert_eq!(j.commit(&p).await, Ok(a), "AC-003 retry");
        } else {
            assert_eq!(j.recover().await, Ok(before), "AC-002 no partial commit");
            assert_eq!(j.lookup(&p.key).await, Ok(None), "AC-002 no residue");
        }
    }
    pub async fn rejections<J: AcceptanceJournal>(j: &J) {
        let p = proposal();
        j.commit(&p).await.unwrap();
        let before = j.recover().await.unwrap();
        let mut c = p.clone();
        if let ClaimDraft::Workspace { epoch, .. } = &mut c.draft {
            *epoch += 1;
        }
        assert_eq!(
            j.commit(&c).await,
            Err(CommitError::RequestMismatch),
            "AC-004 changed retry"
        );
        c = p.clone();
        c.key.intent = "new".into();
        assert_eq!(
            j.commit(&c).await,
            Err(CommitError::Conflict),
            "AC-004 stale CAS"
        );
        c.expected_revision = 1;
        c.watermark_ms = 9;
        assert_eq!(
            j.commit(&c).await,
            Err(CommitError::ClockRegression),
            "AC-004 clock regression"
        );
        c.watermark_ms = 10;
        assert_eq!(
            j.commit(&c).await,
            Err(CommitError::InvalidOperation),
            "AC-004 reused sequence"
        );
        assert_eq!(j.recover().await, Ok(before), "AC-004 no residue");
    }
    pub async fn acknowledge<J: AcceptanceJournal>(j: &J) {
        let p = proposal();
        let a = j.commit(&p).await.unwrap();
        assert_eq!(
            j.acknowledge(&p.key, 0).await,
            Err(CommitError::Conflict),
            "AC-005 stale ack"
        );
        assert_eq!(j.acknowledge(&p.key, 1).await, Ok(2), "AC-005 ack");
        assert_eq!(j.acknowledge(&p.key, 1).await, Ok(2), "AC-005 replay ack");
        let s = j.recover().await.unwrap();
        assert!(s.pending.is_empty(), "AC-005 pending removed");
        assert_eq!(
            s.cursor,
            Some(Cursor {
                seq: 1,
                hash: op_hash(&p.op),
                lamport: 1
            }),
            "AC-005 cursor retained"
        );
        assert_eq!(s.watermark_ms, Some(10), "AC-005 clock retained");
        assert_eq!(
            j.lookup(&p.key).await,
            Ok(Some(a.clone())),
            "AC-005 dedup retained"
        );
        assert_eq!(j.commit(&p).await, Ok(a), "AC-005 replay before CAS");
        assert!(
            j.recover().await.unwrap().pending.is_empty(),
            "AC-005 no resurrection"
        );
    }
}
