use std::collections::BTreeMap;

use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimId, DirectoryRecord, GrantId, GrantScope, NodeId, Slot,
    decode_directory_record,
};

use crate::{
    ClockState, KernelConfig, NodePrincipalBinding, PersistedState, PrincipalPlane,
    authority::revocation_is_authorized,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedClaim {
    pub slot: Slot,
    pub node: NodeId,
    pub claim_id: ClaimId,
    pub epoch: u64,
}

pub fn live_claims(
    persisted: &PersistedState,
    config: &KernelConfig,
    clock: ClockState,
) -> Vec<ProjectedClaim> {
    let ClockState::Ready { watermark } = clock else {
        return Vec::new();
    };
    let Some(skew) = i64::try_from(config.skew_margin_ms).ok() else {
        return Vec::new();
    };
    let Some(expiry_floor) = watermark.0.checked_add(skew) else {
        return Vec::new();
    };
    let Some(max_lease) = i64::try_from(config.max_lease_ms).ok() else {
        return Vec::new();
    };
    let Some(expiry_ceiling) = expiry_floor.checked_add(max_lease) else {
        return Vec::new();
    };

    let mut grants = BTreeMap::<GrantId, CapabilityGrant>::new();
    let mut decoded = Vec::new();
    for op in persisted
        .retained
        .values()
        .flat_map(|records| records.values())
    {
        let Ok(record) = decode_directory_record(&op.envelope().payload) else {
            continue;
        };
        if let DirectoryRecord::CapabilityGrant(grant) = &record {
            grants.insert(grant.grant_id.clone(), grant.clone());
        }
        decoded.push((op, record));
    }

    decoded
        .into_iter()
        .filter_map(|(op, record)| match record {
            DirectoryRecord::ServeClaim(claim) => {
                if claim.lease_expiry_ms <= expiry_floor || claim.lease_expiry_ms > expiry_ceiling {
                    return None;
                }
                let grant = grants.get(&claim.grant_ref)?;
                if revocation_is_authorized(persisted, config, grant) {
                    return None;
                }
                let owner = config.workspace_owner_roots.get(&claim.share)?;
                let binding = NodePrincipalBinding {
                    node: claim.node.clone(),
                    principal: op.envelope().origin.clone(),
                    plane: PrincipalPlane::Workspace,
                };
                if grant.issuer != *owner
                    || grant.principal != op.envelope().origin
                    || grant.share != claim.share
                    || !grant.verbs.contains(&CapabilityVerb::Serve)
                    || grant.scope.is_some()
                    || !config.node_principal_bindings.contains(&binding)
                {
                    return None;
                }
                Some(ProjectedClaim {
                    slot: Slot::Workspace { share: claim.share },
                    node: claim.node,
                    claim_id: claim.claim_id,
                    epoch: claim.epoch,
                })
            }
            DirectoryRecord::ServiceInstanceClaim(claim) => {
                if claim.lease_expiry_ms <= expiry_floor || claim.lease_expiry_ms > expiry_ceiling {
                    return None;
                }
                let grant = grants.get(&claim.exec_grant_ref)?;
                if revocation_is_authorized(persisted, config, grant) {
                    return None;
                }
                let binding = NodePrincipalBinding {
                    node: claim.node.clone(),
                    principal: op.envelope().origin.clone(),
                    plane: PrincipalPlane::Derived,
                };
                let exact_scope = matches!(
                    &grant.scope,
                    Some(GrantScope::Execution(scope))
                        if scope.def_ref == claim.def_ref
                            && scope.compute_key == claim.compute_key
                );
                if claim.share != "svc"
                    || grant.principal != op.envelope().origin
                    || !grant.verbs.contains(&CapabilityVerb::Execute)
                    || !exact_scope
                    || !config.node_principal_bindings.contains(&binding)
                {
                    return None;
                }
                Some(ProjectedClaim {
                    slot: Slot::Binding {
                        share: claim.share,
                        glade_id: claim.glade_id,
                        key: claim.key,
                    },
                    node: claim.node,
                    claim_id: claim.claim_id,
                    epoch: claim.epoch,
                })
            }
            DirectoryRecord::CapabilityGrant(_) | DirectoryRecord::CapabilityRevocation(_) => None,
        })
        .collect()
}
