use glade_discover_protocol::{
    ClaimDraft, ClaimIdentity, DirectoryRecord, Generation, IntentId, SignedOp, Slot, WireMsg,
    decode_directory_record, record_id,
};

#[must_use]
pub fn on_retry(
    state: &State,
    slot: &Slot,
    generation: Generation,
    intent: &IntentId,
) -> Transition {
    let Some(own) = state.persisted().mine.get(&(slot.clone(), generation)) else {
        return unchanged(state);
    };
    if own.status != MineStatus::Pending || own.intent != *intent {
        return unchanged(state);
    }
    Transition {
        persisted: state.persisted().clone(),
        effects: vec![Effect::Append {
            intent: intent.clone(),
            slot: slot.clone(),
            generation,
            draft: Box::new(own.draft.clone()),
        }],
    }
}

use crate::{
    ClockState, Effect, IngestDisposition, MineStatus, MonoInstant, State, StepCtx, Transition,
    VerificationResult, WakeToken, ingest,
};

#[must_use]
pub fn on_accepted(
    state: &State,
    ctx: StepCtx,
    intent: &IntentId,
    op: &SignedOp,
    verification: &VerificationResult,
) -> Transition {
    let VerificationResult::Valid { signer } = verification else {
        return unchanged(state);
    };
    if signer != &op.envelope().origin || signer != &state.config().local_principal {
        return unchanged(state);
    }
    let Ok(record) = decode_directory_record(&op.envelope().payload) else {
        return unchanged(state);
    };
    let Some((mine_key, _)) = state.persisted().mine.iter().find(|(key, own)| {
        own.intent == *intent
            && own.status == MineStatus::Pending
            && finalizes(&own.draft, &record, op)
            && state
                .persisted()
                .mine
                .keys()
                .filter(|(slot, _)| slot == &key.0)
                .map(|(_, generation)| *generation)
                .max()
                == Some(key.1)
    }) else {
        return unchanged(state);
    };
    if matches!(state.clock(), ClockState::Uncertain { .. }) {
        let effects = ctx
            .mono
            .0
            .checked_add(state.config().clock_resync_ms)
            .map(MonoInstant)
            .map_or_else(Vec::new, |at_mono| {
                vec![Effect::Schedule {
                    token: WakeToken::AppendRetry {
                        slot: mine_key.0.clone(),
                        generation: mine_key.1,
                        intent: intent.clone(),
                    },
                    at_mono,
                }]
            });
        return Transition {
            persisted: state.persisted().clone(),
            effects,
        };
    }

    let outcome = ingest::ingest(
        op,
        verification,
        state.persisted(),
        state.config(),
        state.clock(),
    );
    let accepted = match outcome.disposition {
        IngestDisposition::Folded => true,
        IngestDisposition::Duplicate => exact_duplicate(state, op),
        IngestDisposition::Quarantined
        | IngestDisposition::Rejected
        | IngestDisposition::TimeDeferred
        | IngestDisposition::StorageExhausted => false,
    };
    if !accepted {
        return unchanged(state);
    }
    let mut persisted = outcome.persisted;
    let Some(accepted) = persisted.mine.get_mut(mine_key) else {
        return unchanged(state);
    };
    accepted.status = MineStatus::Accepted;
    if matches!(draft_identity(&accepted.draft), ClaimIdentity::Mint) {
        if let Some(claim_id) = finalized_claim_id(&record) {
            set_draft_identity(&mut accepted.draft, ClaimIdentity::Existing(claim_id));
        }
    }

    let effects = state
        .config()
        .peers
        .iter()
        .take(usize::from(state.config().gossip_fan))
        .cloned()
        .map(|to| Effect::Gossip {
            to,
            msg: Box::new(WireMsg::DirOp {
                op: Box::new(op.clone()),
            }),
        })
        .collect();
    Transition { persisted, effects }
}

fn exact_duplicate(state: &State, op: &SignedOp) -> bool {
    let id = record_id(op);
    state
        .persisted()
        .retained
        .get(&id.stream)
        .and_then(|records| records.get(&id))
        .is_some_and(|existing| existing.canonical_bytes() == op.canonical_bytes())
}

fn unchanged(state: &State) -> Transition {
    Transition {
        persisted: state.persisted().clone(),
        effects: Vec::new(),
    }
}

fn finalizes(draft: &ClaimDraft, record: &DirectoryRecord, op: &SignedOp) -> bool {
    match (draft, record) {
        (
            ClaimDraft::Workspace {
                node,
                share,
                identity,
                grant_ref,
                lease_expiry_ms,
                epoch,
            },
            DirectoryRecord::ServeClaim(claim),
        ) => {
            claim.node == *node
                && claim.share == *share
                && claim.grant_ref == *grant_ref
                && claim.lease_expiry_ms == *lease_expiry_ms
                && claim.epoch == *epoch
                && identity_matches(identity, claim.claim_id.record(), op)
        }
        (
            ClaimDraft::Service {
                node,
                share,
                glade_id,
                key,
                identity,
                def_ref,
                exec_grant_ref,
                compute_key,
                lease_expiry_ms,
                epoch,
            },
            DirectoryRecord::ServiceInstanceClaim(claim),
        ) => {
            claim.node == *node
                && claim.share == *share
                && claim.glade_id == *glade_id
                && claim.key == *key
                && claim.def_ref == *def_ref
                && claim.exec_grant_ref == *exec_grant_ref
                && claim.compute_key == *compute_key
                && claim.lease_expiry_ms == *lease_expiry_ms
                && claim.epoch == *epoch
                && identity_matches(identity, claim.claim_id.record(), op)
        }
        _ => false,
    }
}

fn identity_matches(
    identity: &ClaimIdentity,
    finalized: &glade_discover_protocol::RecordId,
    op: &SignedOp,
) -> bool {
    match identity {
        ClaimIdentity::Mint => finalized == &record_id(op),
        ClaimIdentity::Existing(existing) => finalized == existing.record(),
    }
}

fn draft_identity(draft: &ClaimDraft) -> &ClaimIdentity {
    match draft {
        ClaimDraft::Workspace { identity, .. } | ClaimDraft::Service { identity, .. } => identity,
    }
}

fn set_draft_identity(draft: &mut ClaimDraft, next: ClaimIdentity) {
    match draft {
        ClaimDraft::Workspace { identity, .. } | ClaimDraft::Service { identity, .. } => {
            *identity = next;
        }
    }
}

fn finalized_claim_id(record: &DirectoryRecord) -> Option<glade_discover_protocol::ClaimId> {
    match record {
        DirectoryRecord::ServeClaim(claim) => Some(claim.claim_id.clone()),
        DirectoryRecord::ServiceInstanceClaim(claim) => Some(claim.claim_id.clone()),
        DirectoryRecord::CapabilityGrant(_) | DirectoryRecord::CapabilityRevocation(_) => None,
    }
}
