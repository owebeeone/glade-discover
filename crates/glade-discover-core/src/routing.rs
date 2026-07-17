use glade_discover_protocol::{Corr, IngressId, RouteQuery};

use crate::{Effect, RouteAns, projection::ProjectedClaim};

#[must_use]
pub fn resolve(query: &RouteQuery, claims: &[ProjectedClaim]) -> RouteAns {
    claims
        .iter()
        .filter(|claim| claim.slot == query.slot)
        .max_by(|left, right| (left.epoch, &left.claim_id).cmp(&(right.epoch, &right.claim_id)))
        .map_or(RouteAns::NoClaim, |claim| RouteAns::Matched {
            node: claim.node.clone(),
        })
}

/// Resolves one delivered route into its single terminal, ingress-addressed
/// effect. Consumer authorization and duplicate suppression belong to the
/// trusted ingress and therefore are deliberately absent from this boundary.
#[must_use]
pub fn reply(
    ingress: IngressId,
    corr: Corr,
    query: &RouteQuery,
    claims: &[ProjectedClaim],
) -> Effect {
    Effect::Reply {
        ingress,
        corr,
        ans: resolve(query, claims),
    }
}
