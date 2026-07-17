use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimDraft, ClaimId, ClaimIdentity, DirectoryRecord,
    Generation, GrantId, GrantScope, Slot, decode_directory_record,
};

use crate::clock::{ClockDecision, EffectiveClock};
use crate::{
    ClaimCommand, ClaimMode, ClockState, Effect, MineStatus, NodePrincipalBinding, OwnClaim,
    PersistedState, PrincipalPlane, State, StepCtx, Transition, WakeToken, WallMs,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimDisposition {
    Accepted,
    InvalidCommand,
    ClockUncertain,
    Expired,
    LeaseTooLong,
    Unauthorized,
    UnauthorizedTakeover,
    IdentityMismatch,
    StaleGeneration,
    StorageExhausted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimOutcome {
    pub disposition: ClaimDisposition,
    pub transition: Transition,
}

#[must_use]
pub fn on_advertise(state: &State, ctx: StepCtx, command: &ClaimCommand) -> ClaimOutcome {
    let persisted = state.persisted().clone();
    let effective_wall = match crate::clock::observe(state.clock(), ctx, state.config()) {
        EffectiveClock::Ready { effective_wall, .. } => effective_wall,
        EffectiveClock::Uncertain { .. } => {
            return rejected(persisted, ClaimDisposition::ClockUncertain);
        }
    };
    if state.persisted().retained_bytes >= state.config().max_retained_bytes.get() {
        return rejected(persisted, ClaimDisposition::StorageExhausted);
    }
    if !slot_matches_draft(&command.slot, &command.draft) || !draft_is_local(state, &command.draft)
    {
        return rejected(persisted, ClaimDisposition::InvalidCommand);
    }

    let expiry = WallMs(draft_expiry(&command.draft));
    match crate::clock::is_expired(expiry, effective_wall, state.config()) {
        ClockDecision::Ready(true) => return rejected(persisted, ClaimDisposition::Expired),
        ClockDecision::Ready(false) => {}
        ClockDecision::Uncertain => {
            return rejected(persisted, ClaimDisposition::ClockUncertain);
        }
    }
    match crate::clock::lease_within_limit(expiry, effective_wall, state.config()) {
        ClockDecision::Ready(false) => {
            return rejected(persisted, ClaimDisposition::LeaseTooLong);
        }
        ClockDecision::Ready(true) => {}
        ClockDecision::Uncertain => {
            return rejected(persisted, ClaimDisposition::ClockUncertain);
        }
    }

    if let Some(existing) = persisted
        .mine
        .get(&(command.slot.clone(), command.generation))
    {
        if existing.intent == command.intent
            && existing.draft == command.draft
            && existing.status == MineStatus::Pending
        {
            return accepted(persisted, command);
        }
        return rejected(persisted, ClaimDisposition::StaleGeneration);
    }
    if latest_generation(&persisted, &command.slot)
        .is_some_and(|generation| command.generation <= generation)
    {
        return rejected(persisted, ClaimDisposition::StaleGeneration);
    }

    let disposition = match &command.mode {
        ClaimMode::Initial => validate_initial(state, command),
        ClaimMode::Renew => validate_renewal(state, command, effective_wall),
        ClaimMode::Takeover { authority_ref } => validate_takeover(state, command, authority_ref),
    };
    if disposition != ClaimDisposition::Accepted {
        return rejected(persisted, disposition);
    }
    accepted(persisted, command)
}

#[must_use]
pub fn on_wakeup(state: &State, _ctx: StepCtx, token: &WakeToken) -> Transition {
    let persisted = state.persisted().clone();
    match token {
        WakeToken::ClaimRenew { slot, generation }
            if persisted.mine.contains_key(&(slot.clone(), *generation)) =>
        {
            // Renewal needs a fresh expiry and therefore enters through a typed
            // Advertise command. A wakeup is only a stale-safe notification;
            // it cannot invent a renewal draft.
            Transition {
                persisted,
                effects: Vec::new(),
            }
        }
        WakeToken::ClaimRenew { .. }
        | WakeToken::AppendRetry { .. }
        | WakeToken::GossipTick { .. }
        | WakeToken::SyncTimeout { .. } => Transition {
            persisted,
            effects: Vec::new(),
        },
    }
}

#[must_use]
pub fn on_slot_winner(state: &State, slot: &Slot, winner: Option<&ClaimId>) -> Transition {
    let mut persisted = state.persisted().clone();
    if !matches!(slot, Slot::Binding { share, .. } if share == "svc") {
        return Transition {
            persisted,
            effects: Vec::new(),
        };
    }
    if winner.is_none() && matches!(state.clock(), ClockState::Uncertain { .. }) {
        return Transition {
            persisted,
            effects: Vec::new(),
        };
    }
    let losing = persisted
        .mine
        .iter()
        .filter(|((candidate, _), claim)| candidate == slot && claim.status == MineStatus::Accepted)
        .filter_map(|((_, generation), claim)| {
            let is_losing = winner.is_none_or(|winner| {
                finalized_claim_id(&persisted, &claim.draft).as_ref() != Some(winner)
            });
            is_losing.then_some(*generation)
        })
        .collect::<Vec<_>>();
    let Some(generation) = losing.iter().copied().max() else {
        return Transition {
            persisted,
            effects: Vec::new(),
        };
    };
    for losing_generation in losing {
        if let Some(claim) = persisted.mine.get_mut(&(slot.clone(), losing_generation)) {
            claim.status = MineStatus::Lost;
        }
    }
    Transition {
        persisted,
        effects: vec![Effect::Teardown {
            slot: slot.clone(),
            generation,
        }],
    }
}

fn validate_initial(state: &State, command: &ClaimCommand) -> ClaimDisposition {
    if !matches!(draft_identity(&command.draft), ClaimIdentity::Mint)
        || latest_generation(state.persisted(), &command.slot).is_some()
    {
        return ClaimDisposition::IdentityMismatch;
    }
    if claim_authorized(state, &command.draft) {
        ClaimDisposition::Accepted
    } else {
        ClaimDisposition::Unauthorized
    }
}

fn validate_renewal(
    state: &State,
    command: &ClaimCommand,
    effective_wall: WallMs,
) -> ClaimDisposition {
    let ClaimIdentity::Existing(wanted_id) = draft_identity(&command.draft) else {
        return ClaimDisposition::IdentityMismatch;
    };
    let Some((_, previous)) = latest_own_claim(state.persisted(), &command.slot) else {
        return ClaimDisposition::IdentityMismatch;
    };
    if previous.status != MineStatus::Accepted
        || finalized_claim_id(state.persisted(), &previous.draft).as_ref() != Some(wanted_id)
        || draft_epoch(&previous.draft) != draft_epoch(&command.draft)
        || !same_claim_lineage(&previous.draft, &command.draft)
    {
        return ClaimDisposition::IdentityMismatch;
    }
    match crate::clock::is_expired(
        WallMs(draft_expiry(&previous.draft)),
        effective_wall,
        state.config(),
    ) {
        ClockDecision::Ready(true) => return ClaimDisposition::Expired,
        ClockDecision::Ready(false) => {}
        ClockDecision::Uncertain => return ClaimDisposition::ClockUncertain,
    }
    if draft_expiry(&command.draft) <= draft_expiry(&previous.draft) {
        return ClaimDisposition::IdentityMismatch;
    }
    if claim_authorized(state, &command.draft) {
        ClaimDisposition::Accepted
    } else {
        ClaimDisposition::Unauthorized
    }
}

fn validate_takeover(
    state: &State,
    command: &ClaimCommand,
    authority_ref: &GrantId,
) -> ClaimDisposition {
    if !matches!(draft_identity(&command.draft), ClaimIdentity::Mint)
        || !claim_authorized(state, &command.draft)
    {
        return ClaimDisposition::UnauthorizedTakeover;
    }
    let Some(grant) = live_grant(state.persisted(), authority_ref) else {
        return ClaimDisposition::UnauthorizedTakeover;
    };
    let Some(GrantScope::Takeover { slot, supersedes }) = &grant.scope else {
        return ClaimDisposition::UnauthorizedTakeover;
    };
    if !grant.verbs.contains(&CapabilityVerb::Takeover)
        || grant.principal != state.config().local_principal
        || slot != &command.slot
        || grant.share != slot_share(&command.slot)
    {
        return ClaimDisposition::UnauthorizedTakeover;
    }
    let issuer_valid = match &command.slot {
        Slot::Workspace { share } => {
            state.config().workspace_owner_roots.get(share) == Some(&grant.issuer)
        }
        Slot::Binding { .. } => execution_grant_issuer(state, &command.draft)
            .is_some_and(|issuer| issuer == grant.issuer),
    };
    let Some(superseded_epoch) = claim_epoch(state.persisted(), supersedes) else {
        return ClaimDisposition::UnauthorizedTakeover;
    };
    if !issuer_valid
        || superseded_epoch
            .checked_add(1)
            .is_none_or(|epoch| epoch != draft_epoch(&command.draft))
    {
        return ClaimDisposition::UnauthorizedTakeover;
    }
    ClaimDisposition::Accepted
}

fn accepted(mut persisted: PersistedState, command: &ClaimCommand) -> ClaimOutcome {
    persisted.mine.insert(
        (command.slot.clone(), command.generation),
        OwnClaim {
            intent: command.intent.clone(),
            draft: command.draft.clone(),
            status: MineStatus::Pending,
        },
    );
    ClaimOutcome {
        disposition: ClaimDisposition::Accepted,
        transition: Transition {
            persisted,
            effects: vec![Effect::Append {
                intent: command.intent.clone(),
                slot: command.slot.clone(),
                generation: command.generation,
                draft: Box::new(command.draft.clone()),
            }],
        },
    }
}

fn rejected(persisted: PersistedState, disposition: ClaimDisposition) -> ClaimOutcome {
    ClaimOutcome {
        disposition,
        transition: Transition {
            persisted,
            effects: Vec::new(),
        },
    }
}

fn latest_generation(persisted: &PersistedState, slot: &Slot) -> Option<Generation> {
    persisted
        .mine
        .keys()
        .filter_map(|(candidate, generation)| (candidate == slot).then_some(*generation))
        .max()
}

fn latest_own_claim<'a>(
    persisted: &'a PersistedState,
    slot: &Slot,
) -> Option<(Generation, &'a OwnClaim)> {
    persisted
        .mine
        .iter()
        .filter_map(|((candidate, generation), claim)| {
            (candidate == slot).then_some((*generation, claim))
        })
        .max_by_key(|(generation, _)| *generation)
}

fn claim_authorized(state: &State, draft: &ClaimDraft) -> bool {
    match draft {
        ClaimDraft::Workspace {
            node,
            share,
            grant_ref,
            ..
        } => {
            let binding = NodePrincipalBinding {
                node: node.clone(),
                principal: state.config().local_principal.clone(),
                plane: PrincipalPlane::Workspace,
            };
            let Some(grant) = live_grant(state.persisted(), grant_ref) else {
                return false;
            };
            state.config().node_principal_bindings.contains(&binding)
                && state.config().workspace_owner_roots.get(share) == Some(&grant.issuer)
                && grant.principal == state.config().local_principal
                && grant.share == *share
                && grant.verbs.contains(&CapabilityVerb::Serve)
                && grant.scope.is_none()
        }
        ClaimDraft::Service {
            node,
            share,
            def_ref,
            exec_grant_ref,
            compute_key,
            ..
        } => {
            let binding = NodePrincipalBinding {
                node: node.clone(),
                principal: state.config().local_principal.clone(),
                plane: PrincipalPlane::Derived,
            };
            let Some(grant) = live_grant(state.persisted(), exec_grant_ref) else {
                return false;
            };
            state.config().node_principal_bindings.contains(&binding)
                && share == "svc"
                && grant.principal == state.config().local_principal
                && grant.share == *share
                && grant.verbs.contains(&CapabilityVerb::Execute)
                && matches!(
                    grant.scope,
                    Some(GrantScope::Execution(ref scope))
                        if scope.def_ref == *def_ref && scope.compute_key == *compute_key
                )
        }
    }
}

fn live_grant(persisted: &PersistedState, wanted: &GrantId) -> Option<CapabilityGrant> {
    let mut grant = None;
    let mut revoked = false;
    for op in persisted
        .retained
        .values()
        .flat_map(|records| records.values())
    {
        match decode_directory_record(&op.envelope().payload).ok()? {
            DirectoryRecord::CapabilityGrant(candidate) if candidate.grant_id == *wanted => {
                grant = Some(candidate);
            }
            DirectoryRecord::CapabilityRevocation(revocation) if revocation.revokes == *wanted => {
                revoked = true;
            }
            _ => {}
        }
    }
    (!revoked).then_some(grant).flatten()
}

fn claim_epoch(persisted: &PersistedState, wanted: &ClaimId) -> Option<u64> {
    persisted
        .retained
        .values()
        .flat_map(|records| records.values())
        .find_map(
            |op| match decode_directory_record(&op.envelope().payload).ok()? {
                DirectoryRecord::ServeClaim(claim) if claim.claim_id == *wanted => {
                    Some(claim.epoch)
                }
                DirectoryRecord::ServiceInstanceClaim(claim) if claim.claim_id == *wanted => {
                    Some(claim.epoch)
                }
                _ => None,
            },
        )
}

fn finalized_claim_id(persisted: &PersistedState, draft: &ClaimDraft) -> Option<ClaimId> {
    if let ClaimIdentity::Existing(claim_id) = draft_identity(draft) {
        return Some(claim_id.clone());
    }
    persisted
        .retained
        .values()
        .flat_map(|records| records.values())
        .find_map(
            |op| match decode_directory_record(&op.envelope().payload).ok()? {
                DirectoryRecord::ServeClaim(claim) if matches_workspace_draft(&claim, draft) => {
                    Some(claim.claim_id)
                }
                DirectoryRecord::ServiceInstanceClaim(claim)
                    if matches_service_draft(&claim, draft) =>
                {
                    Some(claim.claim_id)
                }
                _ => None,
            },
        )
}

fn matches_workspace_draft(
    claim: &glade_discover_protocol::ServeClaim,
    draft: &ClaimDraft,
) -> bool {
    matches!(
        draft,
        ClaimDraft::Workspace {
            node,
            share,
            grant_ref,
            lease_expiry_ms,
            epoch,
            ..
        } if claim.node == *node
            && claim.share == *share
            && claim.grant_ref == *grant_ref
            && claim.lease_expiry_ms == *lease_expiry_ms
            && claim.epoch == *epoch
    )
}

fn matches_service_draft(
    claim: &glade_discover_protocol::ServiceInstanceClaim,
    draft: &ClaimDraft,
) -> bool {
    matches!(
        draft,
        ClaimDraft::Service {
            node,
            share,
            glade_id,
            key,
            def_ref,
            exec_grant_ref,
            compute_key,
            lease_expiry_ms,
            epoch,
            ..
        } if claim.node == *node
            && claim.share == *share
            && claim.glade_id == *glade_id
            && claim.key == *key
            && claim.def_ref == *def_ref
            && claim.exec_grant_ref == *exec_grant_ref
            && claim.compute_key == *compute_key
            && claim.lease_expiry_ms == *lease_expiry_ms
            && claim.epoch == *epoch
    )
}

fn execution_grant_issuer(
    state: &State,
    draft: &ClaimDraft,
) -> Option<glade_discover_protocol::Principal> {
    let ClaimDraft::Service { exec_grant_ref, .. } = draft else {
        return None;
    };
    live_grant(state.persisted(), exec_grant_ref).map(|grant| grant.issuer)
}

fn slot_matches_draft(slot: &Slot, draft: &ClaimDraft) -> bool {
    match (slot, draft) {
        (Slot::Workspace { share: slot_share }, ClaimDraft::Workspace { share, .. }) => {
            slot_share == share
        }
        (
            Slot::Binding {
                share: slot_share,
                glade_id: slot_glade,
                key: slot_key,
            },
            ClaimDraft::Service {
                share,
                glade_id,
                key,
                ..
            },
        ) => slot_share == share && slot_glade == glade_id && slot_key == key,
        _ => false,
    }
}

fn draft_is_local(state: &State, draft: &ClaimDraft) -> bool {
    match draft {
        ClaimDraft::Workspace { node, .. } | ClaimDraft::Service { node, .. } => {
            node == &state.config().local_node
        }
    }
}

fn same_claim_lineage(previous: &ClaimDraft, next: &ClaimDraft) -> bool {
    match (previous, next) {
        (
            ClaimDraft::Workspace {
                node: previous_node,
                share: previous_share,
                grant_ref: previous_grant,
                ..
            },
            ClaimDraft::Workspace {
                node,
                share,
                grant_ref,
                ..
            },
        ) => previous_node == node && previous_share == share && previous_grant == grant_ref,
        (
            ClaimDraft::Service {
                node: previous_node,
                share: previous_share,
                glade_id: previous_glade,
                key: previous_key,
                def_ref: previous_def,
                exec_grant_ref: previous_grant,
                compute_key: previous_compute,
                ..
            },
            ClaimDraft::Service {
                node,
                share,
                glade_id,
                key,
                def_ref,
                exec_grant_ref,
                compute_key,
                ..
            },
        ) => {
            previous_node == node
                && previous_share == share
                && previous_glade == glade_id
                && previous_key == key
                && previous_def == def_ref
                && previous_grant == exec_grant_ref
                && previous_compute == compute_key
        }
        _ => false,
    }
}

const fn draft_identity(draft: &ClaimDraft) -> &ClaimIdentity {
    match draft {
        ClaimDraft::Workspace { identity, .. } | ClaimDraft::Service { identity, .. } => identity,
    }
}

const fn draft_expiry(draft: &ClaimDraft) -> i64 {
    match draft {
        ClaimDraft::Workspace {
            lease_expiry_ms, ..
        }
        | ClaimDraft::Service {
            lease_expiry_ms, ..
        } => *lease_expiry_ms,
    }
}

const fn draft_epoch(draft: &ClaimDraft) -> u64 {
    match draft {
        ClaimDraft::Workspace { epoch, .. } | ClaimDraft::Service { epoch, .. } => *epoch,
    }
}

fn slot_share(slot: &Slot) -> &str {
    match slot {
        Slot::Workspace { share } | Slot::Binding { share, .. } => share,
    }
}
