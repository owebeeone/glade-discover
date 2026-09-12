use glade_discover_acceptance_api::Accepted;
use glade_discover_protocol::{
    ClaimIdentity, DirectoryRecord, NodeId, decode_directory_record, encode_signed_op, op_hash,
};
use glade_discover_registry_api::*;
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

#[derive(Default)]
struct Registry {
    accepted: Mutex<Vec<Accepted>>,
    now: Mutex<i64>,
    deny: bool,
    unavailable: bool,
    stale: bool,
    forget_retry: bool,
    mixed_denied: bool,
}
impl RegistryWriter for Registry {
    async fn publish(&self, p: &Publication) -> Result<PublicationReceipt, WriteError> {
        self.write(p, false)
    }
    async fn renew(&self, p: &Publication) -> Result<PublicationReceipt, WriteError> {
        self.write(p, true)
    }
}
impl Registry {
    fn write(&self, p: &Publication, renew: bool) -> Result<PublicationReceipt, WriteError> {
        if self.deny {
            return Err(WriteError::Denied);
        }
        if self.unavailable {
            return Err(WriteError::Unavailable);
        }
        let glade_discover_protocol::ClaimDraft::Workspace {
            identity,
            lease_expiry_ms,
            ..
        } = &p.draft
        else {
            return Err(WriteError::InvalidRequest);
        };
        if renew != matches!(identity, ClaimIdentity::Existing(_)) {
            return Err(WriteError::InvalidRequest);
        }
        let mut accepted = self.accepted.lock().unwrap();
        if !self.forget_retry {
            if let Some(a) = accepted.iter().find(|a| a.key == p.key) {
                if a.draft != p.draft {
                    return Err(WriteError::RequestMismatch);
                }
                return Ok(PublicationReceipt {
                    registry: NodeId::from("registry"),
                    accepted: a.clone(),
                });
            }
        }
        if *lease_expiry_ms <= *self.now.lock().unwrap() {
            return Err(WriteError::Expired);
        }
        if let ClaimIdentity::Existing(id) = identity {
            let previous=accepted.iter().rev().find(|old| matches!(decode_directory_record(&old.op.envelope().payload),Ok(DirectoryRecord::ServeClaim(ref c)) if &c.claim_id==id));
            let Some(previous) = previous else {
                return Err(WriteError::InvalidRequest);
            };
            let glade_discover_protocol::ClaimDraft::Workspace {
                node: prior_node,
                share: prior_share,
                ..
            } = &previous.draft
            else {
                return Err(WriteError::InvalidRequest);
            };
            let glade_discover_protocol::ClaimDraft::Workspace { node, share, .. } = &p.draft
            else {
                return Err(WriteError::InvalidRequest);
            };
            if node != prior_node
                || share != prior_share
                || p.key.requester != previous.key.requester
                || p.key.slot != previous.key.slot
            {
                return Err(WriteError::InvalidRequest);
            }
        }
        let mut a = conformance::accepted(p, accepted.len() as u64 + 1);
        let mut envelope = a.op.envelope().clone();
        envelope.prev = accepted.last().map(|old| op_hash(&old.op));
        a.op = encode_signed_op(&envelope, &[]).unwrap();
        accepted.push(a.clone());
        Ok(PublicationReceipt {
            registry: NodeId::from("registry"),
            accepted: a,
        })
    }
}
impl RegistryReader for Registry {
    async fn resolve(&self, q: &ResolveRequest) -> Result<Resolution, ResolveError> {
        if self.mixed_denied && q.query.slot == conformance::mixed_query().query.slot {
            return Err(ResolveError::Denied);
        }
        if self.deny {
            return Err(ResolveError::Denied);
        }
        if self.unavailable {
            return Err(ResolveError::Unavailable);
        }
        let now = *self.now.lock().unwrap();
        let a = self.accepted.lock().unwrap();
        let mut current = std::collections::BTreeMap::new();
        for entry in a.iter() {
            let DirectoryRecord::ServeClaim(claim) =
                decode_directory_record(&entry.op.envelope().payload).unwrap()
            else {
                continue;
            };
            current.insert(claim.claim_id, entry);
        }
        let all: Vec<_> = current
            .values()
            .filter(|a| {
                a.key.slot == q.query.slot && (self.stale || conformance::expiry(&a.draft) > now)
            })
            .map(|a| a.op.clone())
            .collect();
        Ok(Resolution {
            registry: NodeId::from("registry"),
            observed_at_ms: now,
            truncated: all.len() > q.limit.get(),
            claims: all.into_iter().take(q.limit.get()).collect(),
        })
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

// Model snapshot transfer, NOT a durable registry or real restart implementation.
async fn reopen_registry(r: Registry) -> Registry {
    Registry {
        accepted: Mutex::new(r.accepted.into_inner().unwrap()),
        now: Mutex::new(r.now.into_inner().unwrap()),
        ..Default::default()
    }
}
#[test]
fn rg_007_restart_preserves_acceptance_and_retry() {
    ready(conformance::reopen_retry(
        Registry::default(),
        reopen_registry,
    ));
}
#[test]
#[should_panic(expected = "RG-007")]
fn rejects_registry_that_forgets_receipts_on_restart() {
    ready(conformance::reopen_retry(Registry::default(), |_| async {
        Registry::default()
    }));
}
#[test]
fn rg_001_publish_retry_and_changed_request() {
    ready(conformance::publish_retry(&Registry::default()));
}
#[test]
fn rg_002_renewal_and_expiry() {
    let r = Registry::default();
    ready(conformance::renew_and_expire(&r, &r, |now| {
        *r.now.lock().unwrap() = now
    }));
}
#[test]
fn rg_003_absence_is_not_unavailability_or_denial() {
    ready(conformance::empty(&Registry::default()));
    ready(conformance::read_error(
        &Registry {
            unavailable: true,
            ..Default::default()
        },
        ResolveError::Unavailable,
    ));
    ready(conformance::read_error(
        &Registry {
            deny: true,
            ..Default::default()
        },
        ResolveError::Denied,
    ));
}
#[test]
#[should_panic(expected = "RG-001")]
fn rejects_non_idempotent_publisher() {
    ready(conformance::publish_retry(&Registry {
        forget_retry: true,
        ..Default::default()
    }));
}
#[test]
#[should_panic(expected = "RG-002")]
fn rejects_expired_candidates() {
    let r = Registry {
        stale: true,
        ..Default::default()
    };
    ready(conformance::renew_and_expire(&r, &r, |now| {
        *r.now.lock().unwrap() = now
    }));
}

#[test]
fn rg_004_bounds_and_query_scope() {
    let r = Registry::default();
    ready(conformance::bounded(&r, &r));
}
#[test]
fn rg_005_invalid_renewals_and_expired_new_requests() {
    let r = Registry::default();
    ready(conformance::invalid_writes(&r, &r));
}
#[test]
fn rg_006_mixed_source_query_is_denied() {
    ready(conformance::mixed_source_denied(&Registry {
        mixed_denied: true,
        ..Default::default()
    }));
}
#[test]
#[should_panic(expected = "RG-006")]
fn rejects_any_source_authorized_registry() {
    ready(conformance::mixed_source_denied(&Registry::default()));
}
