use glade_discover_protocol::{DirectoryRecord, Principal, SignedOp, decode_directory_record};

use crate::{GovernanceVerdict, KernelConfig, PersistedState, PrincipalPlane};

pub struct AuthorityView<'a> {
    pub config: &'a KernelConfig,
    pub persisted: &'a PersistedState,
}

pub fn publish_allowed(
    op: &SignedOp,
    record: &DirectoryRecord,
    signer: &Principal,
    view: &AuthorityView<'_>,
) -> GovernanceVerdict {
    if signer != &op.envelope().origin {
        return GovernanceVerdict::Unauthorized;
    }

    let authorized = match record {
        DirectoryRecord::CapabilityGrant(grant) => {
            grant.issuer == *signer
                && view.config.workspace_owner_roots.get(&grant.share) == Some(signer)
        }
        DirectoryRecord::ServeClaim(claim) => {
            view.config
                .node_principal_bindings
                .contains(&crate::NodePrincipalBinding {
                    node: claim.node.clone(),
                    principal: signer.clone(),
                    plane: PrincipalPlane::Workspace,
                })
        }
        DirectoryRecord::ServiceInstanceClaim(claim) => {
            claim.share == "svc"
                && view
                    .config
                    .node_principal_bindings
                    .contains(&crate::NodePrincipalBinding {
                        node: claim.node.clone(),
                        principal: signer.clone(),
                        plane: PrincipalPlane::Derived,
                    })
        }
        DirectoryRecord::CapabilityRevocation(revocation) => {
            referenced_grant(view.persisted, &revocation.revokes).map_or_else(
                || {
                    view.config
                        .workspace_owner_roots
                        .values()
                        .any(|owner| owner == signer)
                },
                |grant| view.config.workspace_owner_roots.get(&grant.share) == Some(signer),
            )
        }
    };

    if authorized {
        GovernanceVerdict::Authorized
    } else {
        GovernanceVerdict::Unauthorized
    }
}

pub(crate) fn referenced_grant(
    persisted: &PersistedState,
    wanted: &glade_discover_protocol::GrantId,
) -> Option<glade_discover_protocol::CapabilityGrant> {
    persisted
        .retained
        .values()
        .flat_map(|records| records.values())
        .find_map(
            |op| match decode_directory_record(&op.envelope().payload).ok()? {
                DirectoryRecord::CapabilityGrant(grant) if &grant.grant_id == wanted => Some(grant),
                _ => None,
            },
        )
}

pub(crate) fn revocation_is_authorized(
    persisted: &PersistedState,
    config: &KernelConfig,
    grant: &glade_discover_protocol::CapabilityGrant,
) -> bool {
    let Some(owner) = config.workspace_owner_roots.get(&grant.share) else {
        return false;
    };
    persisted
        .retained
        .values()
        .flat_map(|records| records.values())
        .any(|op| {
            matches!(
                decode_directory_record(&op.envelope().payload),
                Ok(DirectoryRecord::CapabilityRevocation(revocation))
                    if revocation.revokes == grant.grant_id
                        && op.envelope().origin == *owner
            )
        })
}
