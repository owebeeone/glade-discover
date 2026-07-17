use glade_discover_protocol::{
    ClaimId, DecodeError, DirectoryRecord, GrantId, SignedOp, decode_directory_record, op_hash,
    record_id,
};

use crate::{
    ClockState, KernelConfig, PersistedState, VerificationResult, WallMs,
    authority::{AuthorityView, publish_allowed, referenced_grant, revocation_is_authorized},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralVerdict {
    Malformed,
    UnsupportedVersion,
    BadSignature,
    BadChain,
    Duplicate,
    StructurallyValid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GovernanceVerdict {
    Authorized,
    Unauthorized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveCapabilityVerdict {
    Live,
    UnresolvedProof,
    Revoked,
    Expired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestDisposition {
    Folded,
    Duplicate,
    Quarantined,
    Rejected,
    TimeDeferred,
    StorageExhausted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestOutcome {
    pub structural: StructuralVerdict,
    pub governance: Option<GovernanceVerdict>,
    pub capability: Option<LiveCapabilityVerdict>,
    pub disposition: IngestDisposition,
    pub persisted: PersistedState,
}

pub fn ingest(
    op: &SignedOp,
    verification: &VerificationResult,
    persisted: &PersistedState,
    config: &KernelConfig,
    clock: ClockState,
) -> IngestOutcome {
    let record = match decode_directory_record(&op.envelope().payload) {
        Ok(record) => record,
        Err(DecodeError::UnsupportedVersion(_)) => {
            return rejected(persisted, StructuralVerdict::UnsupportedVersion);
        }
        Err(_) => return rejected(persisted, StructuralVerdict::Malformed),
    };

    let signer = match verification {
        VerificationResult::Valid { signer } if signer == &op.envelope().origin => signer,
        VerificationResult::Valid { .. } | VerificationResult::BadSignature => {
            return rejected(persisted, StructuralVerdict::BadSignature);
        }
    };

    let id = record_id(op);
    if let Some(existing) = persisted
        .retained
        .get(&id.stream)
        .and_then(|records| records.get(&id))
    {
        return if op_hash(existing) == op_hash(op) {
            IngestOutcome {
                structural: StructuralVerdict::Duplicate,
                governance: None,
                capability: None,
                disposition: IngestDisposition::Duplicate,
                persisted: persisted.clone(),
            }
        } else {
            rejected(persisted, StructuralVerdict::BadChain)
        };
    }
    if !chain_position_is_valid(op, persisted) || !identity_is_consistent(&record, &id, persisted) {
        return rejected(persisted, StructuralVerdict::BadChain);
    }

    let governance = publish_allowed(op, &record, signer, &AuthorityView { config, persisted });
    if governance == GovernanceVerdict::Unauthorized {
        return IngestOutcome {
            structural: StructuralVerdict::StructurallyValid,
            governance: Some(governance),
            capability: None,
            disposition: IngestDisposition::Rejected,
            persisted: persisted.clone(),
        };
    }

    let capability = live_capability(&record, persisted, config, clock);
    if lease_exceeds_maximum(&record, config, clock) {
        return IngestOutcome {
            structural: StructuralVerdict::StructurallyValid,
            governance: Some(governance),
            capability,
            disposition: IngestDisposition::Rejected,
            persisted: persisted.clone(),
        };
    }

    let Ok(record_bytes) = u64::try_from(op.canonical_bytes().len()) else {
        return storage_exhausted(persisted, governance);
    };
    let Some(retained_bytes) = persisted.retained_bytes.checked_add(record_bytes) else {
        return storage_exhausted(persisted, governance);
    };
    if retained_bytes > config.max_retained_bytes.get() {
        return storage_exhausted(persisted, governance);
    }

    let is_claim = matches!(
        record,
        DirectoryRecord::ServeClaim(_) | DirectoryRecord::ServiceInstanceClaim(_)
    );
    let mut next = persisted.clone();
    next.retained
        .entry(id.stream.clone())
        .or_default()
        .insert(id.clone(), op.clone());
    next.retained_bytes = retained_bytes;
    match &record {
        DirectoryRecord::ServeClaim(claim) => {
            update_unresolved(&mut next, &claim.grant_ref, &claim.claim_id, capability);
        }
        DirectoryRecord::ServiceInstanceClaim(claim) => {
            update_unresolved(
                &mut next,
                &claim.exec_grant_ref,
                &claim.claim_id,
                capability,
            );
        }
        DirectoryRecord::CapabilityGrant(grant) => {
            next.unresolved.remove(&grant.grant_id);
        }
        DirectoryRecord::CapabilityRevocation(_) => {}
    }
    let disposition = if is_claim && matches!(clock, ClockState::Uncertain { .. }) {
        next.time_deferred.insert(id);
        IngestDisposition::TimeDeferred
    } else {
        IngestDisposition::Folded
    };

    IngestOutcome {
        structural: StructuralVerdict::StructurallyValid,
        governance: Some(governance),
        capability,
        disposition,
        persisted: next,
    }
}

/// Revalidate claims retained while the wall watermark was unavailable.
///
/// Claims within the checked lease ceiling become ordinary retained claims.
/// Claims beyond it, malformed deferred entries, and every deferred claim when
/// the ceiling arithmetic is uncertain are permanently removed and
/// byte-deaccounted.
#[must_use]
pub fn revalidate_time_deferred(
    persisted: &PersistedState,
    config: &KernelConfig,
    effective_wall: WallMs,
) -> PersistedState {
    let ceiling = i64::try_from(config.max_lease_ms)
        .ok()
        .and_then(|max_lease| effective_wall.0.checked_add(max_lease))
        .and_then(|value| {
            i64::try_from(config.skew_margin_ms)
                .ok()
                .and_then(|skew| value.checked_add(skew))
        });
    let mut next = persisted.clone();
    let deferred: Vec<_> = next.time_deferred.iter().cloned().collect();
    for id in deferred {
        let expiry =
            next.retained
                .get(&id.stream)
                .and_then(|records| records.get(&id))
                .and_then(|op| decode_directory_record(&op.envelope().payload).ok())
                .and_then(|record| match record {
                    DirectoryRecord::ServeClaim(claim) => Some(claim.lease_expiry_ms),
                    DirectoryRecord::ServiceInstanceClaim(claim) => Some(claim.lease_expiry_ms),
                    DirectoryRecord::CapabilityGrant(_)
                    | DirectoryRecord::CapabilityRevocation(_) => None,
                });
        let admitted = ceiling
            .zip(expiry)
            .is_some_and(|(ceiling, expiry)| expiry <= ceiling);
        if admitted {
            next.time_deferred.remove(&id);
        } else {
            remove_retained(&mut next, &id);
        }
    }
    rebuild_unresolved(&mut next);
    next
}

fn remove_retained(persisted: &mut PersistedState, id: &glade_discover_protocol::RecordId) {
    let (removed_bytes, stream_empty) =
        persisted
            .retained
            .get_mut(&id.stream)
            .map_or((None, false), |records| {
                let bytes = records
                    .remove(id)
                    .map(|op| u64::try_from(op.canonical_bytes().len()).unwrap_or(u64::MAX));
                (bytes, records.is_empty())
            });
    if stream_empty {
        persisted.retained.remove(&id.stream);
    }
    if let Some(bytes) = removed_bytes {
        persisted.retained_bytes = persisted.retained_bytes.saturating_sub(bytes);
    }
    persisted.time_deferred.remove(id);
}

fn rebuild_unresolved(persisted: &mut PersistedState) {
    let grants: std::collections::BTreeSet<_> = persisted
        .retained
        .values()
        .flat_map(|records| records.values())
        .filter_map(
            |op| match decode_directory_record(&op.envelope().payload).ok()? {
                DirectoryRecord::CapabilityGrant(grant) => Some(grant.grant_id),
                _ => None,
            },
        )
        .collect();
    let mut unresolved =
        std::collections::BTreeMap::<GrantId, std::collections::BTreeSet<ClaimId>>::new();
    for op in persisted
        .retained
        .values()
        .flat_map(|records| records.values())
    {
        let Ok(record) = decode_directory_record(&op.envelope().payload) else {
            continue;
        };
        let dependency = match record {
            DirectoryRecord::ServeClaim(claim) => Some((claim.grant_ref, claim.claim_id)),
            DirectoryRecord::ServiceInstanceClaim(claim) => {
                Some((claim.exec_grant_ref, claim.claim_id))
            }
            DirectoryRecord::CapabilityGrant(_) | DirectoryRecord::CapabilityRevocation(_) => None,
        };
        if let Some((grant_id, claim_id)) = dependency {
            if !grants.contains(&grant_id) {
                unresolved.entry(grant_id).or_default().insert(claim_id);
            }
        }
    }
    persisted.unresolved = unresolved;
}

fn rejected(persisted: &PersistedState, structural: StructuralVerdict) -> IngestOutcome {
    IngestOutcome {
        structural,
        governance: None,
        capability: None,
        disposition: IngestDisposition::Quarantined,
        persisted: persisted.clone(),
    }
}

fn storage_exhausted(persisted: &PersistedState, governance: GovernanceVerdict) -> IngestOutcome {
    IngestOutcome {
        structural: StructuralVerdict::StructurallyValid,
        governance: Some(governance),
        capability: None,
        disposition: IngestDisposition::StorageExhausted,
        persisted: persisted.clone(),
    }
}

fn chain_position_is_valid(op: &SignedOp, persisted: &PersistedState) -> bool {
    let envelope = op.envelope();
    let head = persisted
        .retained
        .get(&envelope.stream)
        .and_then(|records| {
            records
                .iter()
                .filter(|(id, _)| id.origin == envelope.origin)
                .max_by_key(|(id, _)| id.seq)
        });
    match head {
        None => envelope.seq == 0 && envelope.prev.is_none(),
        Some((head_id, head_op)) => {
            head_id.seq.checked_add(1) == Some(envelope.seq)
                && envelope.prev == Some(op_hash(head_op))
        }
    }
}

fn identity_is_consistent(
    record: &DirectoryRecord,
    op_id: &glade_discover_protocol::RecordId,
    persisted: &PersistedState,
) -> bool {
    match record {
        DirectoryRecord::CapabilityGrant(grant) => grant.grant_id.record() == op_id,
        DirectoryRecord::ServeClaim(claim) => {
            claim.claim_id.record() == op_id
                || original_claim(&claim.claim_id, persisted).is_some_and(|original| {
                    matches!(
                        original,
                        DirectoryRecord::ServeClaim(anchor)
                            if anchor.node == claim.node
                                && anchor.share == claim.share
                                && anchor.claim_id == claim.claim_id
                                && anchor.grant_ref == claim.grant_ref
                                && anchor.epoch == claim.epoch
                    )
                })
        }
        DirectoryRecord::ServiceInstanceClaim(claim) => {
            claim.claim_id.record() == op_id
                || original_claim(&claim.claim_id, persisted).is_some_and(|original| {
                    matches!(
                        original,
                        DirectoryRecord::ServiceInstanceClaim(anchor)
                            if anchor.node == claim.node
                                && anchor.share == claim.share
                                && anchor.glade_id == claim.glade_id
                                && anchor.key == claim.key
                                && anchor.claim_id == claim.claim_id
                                && anchor.def_ref == claim.def_ref
                                && anchor.exec_grant_ref == claim.exec_grant_ref
                                && anchor.compute_key == claim.compute_key
                                && anchor.epoch == claim.epoch
                    )
                })
        }
        DirectoryRecord::CapabilityRevocation(_) => true,
    }
}

fn original_claim(
    claim_id: &glade_discover_protocol::ClaimId,
    persisted: &PersistedState,
) -> Option<DirectoryRecord> {
    persisted
        .retained
        .get(&claim_id.record().stream)
        .and_then(|records| records.get(claim_id.record()))
        .and_then(|op| decode_directory_record(&op.envelope().payload).ok())
}

fn update_unresolved(
    persisted: &mut PersistedState,
    grant_id: &GrantId,
    claim_id: &ClaimId,
    capability: Option<LiveCapabilityVerdict>,
) {
    if capability == Some(LiveCapabilityVerdict::UnresolvedProof) {
        persisted
            .unresolved
            .entry(grant_id.clone())
            .or_default()
            .insert(claim_id.clone());
    }
}

fn live_capability(
    record: &DirectoryRecord,
    persisted: &PersistedState,
    config: &KernelConfig,
    clock: ClockState,
) -> Option<LiveCapabilityVerdict> {
    let (grant_ref, expiry) = match record {
        DirectoryRecord::ServeClaim(claim) => (&claim.grant_ref, claim.lease_expiry_ms),
        DirectoryRecord::ServiceInstanceClaim(claim) => {
            (&claim.exec_grant_ref, claim.lease_expiry_ms)
        }
        DirectoryRecord::CapabilityGrant(_) | DirectoryRecord::CapabilityRevocation(_) => {
            return None;
        }
    };
    let Some(grant) = referenced_grant(persisted, grant_ref) else {
        return Some(LiveCapabilityVerdict::UnresolvedProof);
    };
    if revocation_is_authorized(persisted, config, &grant) {
        return Some(LiveCapabilityVerdict::Revoked);
    }
    if let ClockState::Ready { watermark } = clock {
        let Some(skew) = i64::try_from(config.skew_margin_ms).ok() else {
            return Some(LiveCapabilityVerdict::Expired);
        };
        let Some(expiry_floor) = watermark.0.checked_add(skew) else {
            return Some(LiveCapabilityVerdict::Expired);
        };
        if expiry <= expiry_floor {
            return Some(LiveCapabilityVerdict::Expired);
        }
    }
    Some(LiveCapabilityVerdict::Live)
}

fn lease_exceeds_maximum(
    record: &DirectoryRecord,
    config: &KernelConfig,
    clock: ClockState,
) -> bool {
    let expiry = match record {
        DirectoryRecord::ServeClaim(claim) => claim.lease_expiry_ms,
        DirectoryRecord::ServiceInstanceClaim(claim) => claim.lease_expiry_ms,
        DirectoryRecord::CapabilityGrant(_) | DirectoryRecord::CapabilityRevocation(_) => {
            return false;
        }
    };
    let ClockState::Ready { watermark } = clock else {
        return false;
    };
    let Some(max_lease) = i64::try_from(config.max_lease_ms).ok() else {
        return true;
    };
    let Some(skew) = i64::try_from(config.skew_margin_ms).ok() else {
        return true;
    };
    let Some(limit) = watermark
        .0
        .checked_add(max_lease)
        .and_then(|value| value.checked_add(skew))
    else {
        return true;
    };
    expiry > limit
}
