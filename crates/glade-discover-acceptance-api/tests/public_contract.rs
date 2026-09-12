use glade_discover_acceptance_api::*;
use glade_discover_protocol::{
    ClaimDraft, ClaimIdentity, DirectoryRecord, Slot, decode_directory_record, op_hash, record_id,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

#[derive(Clone, Default)]
struct State {
    revision: u64,
    cursor: Option<Cursor>,
    watermark: Option<i64>,
    entries: BTreeMap<RequestKey, Accepted>,
    pending: Vec<RequestKey>,
}
#[derive(Default)]
struct Journal {
    state: Mutex<State>,
    fail_before: bool,
    lose_reply: bool,
    split_commit: bool,
    lose_ack: bool,
    resurrect_ack: bool,
}
impl AcceptanceJournal for Journal {
    fn scope(&self) -> StreamScope {
        conformance::scope()
    }
    async fn recover(&self) -> Result<Recovery, ReadError> {
        let s = self.state.lock().unwrap();
        Ok(Recovery {
            revision: s.revision,
            cursor: s.cursor.clone(),
            watermark_ms: s.watermark,
            pending: s.pending.iter().map(|k| s.entries[k].clone()).collect(),
        })
    }
    async fn lookup(&self, key: &RequestKey) -> Result<Option<Accepted>, ReadError> {
        Ok(self.state.lock().unwrap().entries.get(key).cloned())
    }
    async fn commit(&self, p: &Proposal) -> Result<Accepted, CommitError> {
        let mut s = self.state.lock().unwrap();
        if let Some(old) = s.entries.get(&p.key) {
            return if old.draft == p.draft && old.op == p.op {
                Ok(old.clone())
            } else {
                Err(CommitError::RequestMismatch)
            };
        }
        if s.revision != p.expected_revision {
            return Err(CommitError::Conflict);
        }
        if s.watermark.is_some_and(|w| p.watermark_ms < w) {
            return Err(CommitError::ClockRegression);
        }
        let e = p.op.envelope();
        let next_seq = match &s.cursor {
            Some(c) => c.seq.checked_add(1).ok_or(CommitError::Exhausted)?,
            None => 1,
        };
        if e.stream != self.scope().stream
            || e.origin != self.scope().origin
            || e.seq != next_seq
            || e.prev != s.cursor.as_ref().map(|c| c.hash)
            || e.lamport <= s.cursor.as_ref().map_or(0, |c| c.lamport)
        {
            return Err(CommitError::InvalidOperation);
        }
        let ClaimDraft::Workspace {
            node,
            share,
            identity,
            grant_ref,
            lease_expiry_ms,
            epoch,
        } = &p.draft
        else {
            return Err(CommitError::InvalidOperation);
        };
        let Ok(DirectoryRecord::ServeClaim(record)) = decode_directory_record(&e.payload) else {
            return Err(CommitError::InvalidOperation);
        };
        let identity_ok = match identity {
            ClaimIdentity::Mint => record.claim_id.record() == &record_id(&p.op),
            ClaimIdentity::Existing(id) => record.claim_id == *id,
        };
        if p.key.slot
            != (Slot::Workspace {
                share: share.clone(),
            })
            || share != &self.scope().stream.share
            || !identity_ok
            || record.node != *node
            || record.share != *share
            || record.grant_ref != *grant_ref
            || record.lease_expiry_ms != *lease_expiry_ms
            || record.epoch != *epoch
        {
            return Err(CommitError::InvalidOperation);
        }
        if self.fail_before {
            return Err(CommitError::Unavailable);
        }
        let revision = s.revision.checked_add(1).ok_or(CommitError::Exhausted)?;
        let a = Accepted {
            key: p.key.clone(),
            draft: p.draft.clone(),
            op: p.op.clone(),
            commit_revision: revision,
        };
        s.entries.insert(p.key.clone(), a.clone());
        if !self.split_commit {
            s.revision = revision;
            s.watermark = Some(p.watermark_ms);
            s.cursor = Some(Cursor {
                seq: e.seq,
                hash: op_hash(&p.op),
                lamport: e.lamport,
            });
            s.pending.push(p.key.clone());
        }
        if self.lose_reply {
            Err(CommitError::OutcomeUnknown)
        } else {
            Ok(a)
        }
    }
    async fn acknowledge(
        &self,
        key: &RequestKey,
        expected_revision: u64,
    ) -> Result<u64, CommitError> {
        let mut s = self.state.lock().unwrap();
        if !s.entries.contains_key(key) {
            return Err(CommitError::InvalidOperation);
        }
        if !s.pending.contains(key) {
            return Ok(s.revision);
        }
        if s.revision != expected_revision {
            return Err(CommitError::Conflict);
        }
        let revision = s.revision.checked_add(1).ok_or(CommitError::Exhausted)?;
        s.pending.retain(|k| k != key);
        s.revision = revision;
        if self.lose_ack {
            Err(CommitError::OutcomeUnknown)
        } else {
            Ok(revision)
        }
    }
}
fn ready<F: Future + Send>(f: F) -> F::Output {
    match std::pin::pin!(f)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(x) => x,
        Poll::Pending => panic!("fixture pending"),
    }
}

#[test]
fn ac_008_competing_intents_have_one_cas_winner() {
    ready(conformance::competing(&Journal::default()));
}
// Snapshot copying is a fixture, NOT a real crash/durable backing store.
async fn reopen(j: Journal) -> Journal {
    let mut state = j.state.into_inner().unwrap();
    if j.resurrect_ack {
        state.pending = state.entries.keys().cloned().collect();
    }
    Journal {
        state: Mutex::new(state),
        ..Default::default()
    }
}

#[test]
fn ac_006_acknowledgement_survives_reopen() {
    ready(conformance::ack_recover(Journal::default(), reopen, false));
}
#[test]
fn ac_006_lost_ack_reply_is_reconciled() {
    ready(conformance::ack_recover(
        Journal {
            lose_ack: true,
            ..Default::default()
        },
        reopen,
        true,
    ));
}
#[test]
#[should_panic(expected = "AC-006")]
fn rejects_resurrected_handoff() {
    ready(conformance::ack_recover(
        Journal {
            resurrect_ack: true,
            ..Default::default()
        },
        reopen,
        false,
    ));
}
#[test]
fn ac_007_new_proposal_validation() {
    ready(conformance::invalid_proposals(&Journal::default()));
}
#[test]
fn ac_001_atomic_acceptance_retry_and_reopen() {
    ready(conformance::accept_retry_recover(
        Journal::default(),
        reopen,
    ));
}
#[test]
fn ac_002_precommit_failure_is_empty() {
    ready(conformance::interrupted(
        Journal {
            fail_before: true,
            ..Default::default()
        },
        reopen,
        false,
    ));
}
#[test]
fn ac_003_lost_reply_is_recovered_without_duplicate() {
    ready(conformance::interrupted(
        Journal {
            lose_reply: true,
            ..Default::default()
        },
        reopen,
        true,
    ));
}
#[test]
fn ac_004_conflicts_clock_regression_and_changed_retry() {
    ready(conformance::rejections(&Journal::default()));
}
#[test]
fn ac_005_acknowledgement_retains_deduplication() {
    ready(conformance::acknowledge(&Journal::default()));
}
#[test]
#[should_panic(expected = "AC-001")]
fn rejects_split_transaction() {
    ready(conformance::accept_retry_recover(
        Journal {
            split_commit: true,
            ..Default::default()
        },
        reopen,
    ));
}
