use glade_discover_core::{Effect, RouteAns, projection::ProjectedClaim, routing};
use glade_discover_protocol::{
    ClaimId, Corr, IngressId, NodeId, Principal, RecordId, RouteQuery, Slot, StreamId,
};

fn workspace(share: &str) -> Slot {
    Slot::Workspace {
        share: share.to_owned(),
    }
}

fn binding(share: &str, glade_id: &str, key: u8) -> Slot {
    Slot::Binding {
        share: share.to_owned(),
        glade_id: glade_id.to_owned(),
        key: vec![key],
    }
}

fn claim_id(origin: &str, seq: u64) -> ClaimId {
    ClaimId::from(RecordId {
        stream: StreamId {
            share: "directory".to_owned(),
            glade_id: "claims".to_owned(),
            key: vec![0],
        },
        origin: Principal::from(origin),
        seq,
    })
}

fn claim(slot: Slot, node: &str, epoch: u64, origin: &str, seq: u64) -> ProjectedClaim {
    ProjectedClaim {
        slot,
        node: NodeId::from(node),
        claim_id: claim_id(origin, seq),
        epoch,
    }
}

#[test]
fn exact_slot_matching_prevents_cross_scope_shadowing() {
    let claims = vec![
        claim(workspace("share-a"), "workspace-node", 1, "a", 0),
        claim(binding("share-a", "glade-a", 1), "exact-node", 1, "b", 0),
        claim(binding("share-a", "glade-a", 2), "other-key", 99, "z", 0),
        claim(binding("share-a", "glade-b", 1), "other-glade", 99, "z", 1),
    ];

    assert_eq!(
        routing::resolve(
            &RouteQuery {
                slot: workspace("share-a"),
            },
            &claims,
        ),
        RouteAns::Matched {
            node: NodeId::from("workspace-node"),
        }
    );
    assert_eq!(
        routing::resolve(
            &RouteQuery {
                slot: binding("share-a", "glade-a", 1),
            },
            &claims,
        ),
        RouteAns::Matched {
            node: NodeId::from("exact-node"),
        }
    );
}

#[test]
fn winner_orders_by_epoch_then_stable_claim_id() {
    let slot = workspace("share-a");
    let lower_epoch_higher_id = claim(slot.clone(), "old", 7, "z", 99);
    let higher_epoch_lower_id = claim(slot.clone(), "takeover", 8, "a", 0);

    assert_eq!(
        routing::resolve(
            &RouteQuery { slot: slot.clone() },
            &[lower_epoch_higher_id, higher_epoch_lower_id],
        ),
        RouteAns::Matched {
            node: NodeId::from("takeover"),
        }
    );

    let lower_id = claim(slot.clone(), "lower-id", 8, "a", 0);
    let higher_id = claim(slot.clone(), "higher-id", 8, "z", 99);
    assert_eq!(
        routing::resolve(&RouteQuery { slot }, &[higher_id, lower_id]),
        RouteAns::Matched {
            node: NodeId::from("higher-id"),
        }
    );
}

#[test]
fn reply_is_one_direct_authz_blind_terminal_effect() {
    let ingress = IngressId::from("ingress-a");
    let corr = Corr::from("corr-a");
    let query = RouteQuery {
        slot: workspace("share-a"),
    };

    assert_eq!(
        routing::reply(ingress.clone(), corr.clone(), &query, &[]),
        Effect::Reply {
            ingress,
            corr,
            ans: RouteAns::NoClaim,
        }
    );
}

#[test]
fn repeated_route_identity_resolves_the_current_claims_without_a_cache() {
    let ingress = IngressId::from("ingress-a");
    let corr = Corr::from("same-corr");
    let slot = workspace("share-a");
    let query = RouteQuery { slot: slot.clone() };

    assert_eq!(
        routing::reply(ingress.clone(), corr.clone(), &query, &[]),
        Effect::Reply {
            ingress: ingress.clone(),
            corr: corr.clone(),
            ans: RouteAns::NoClaim,
        }
    );
    assert_eq!(
        routing::reply(
            ingress.clone(),
            corr.clone(),
            &query,
            &[claim(slot, "new-node", 0, "claimant", 0)],
        ),
        Effect::Reply {
            ingress,
            corr,
            ans: RouteAns::Matched {
                node: NodeId::from("new-node"),
            },
        }
    );
}

#[test]
fn equal_correlations_on_distinct_ingresses_remain_distinct_replies() {
    let corr = Corr::from("shared-corr");
    let query = RouteQuery {
        slot: workspace("share-a"),
    };

    let first = routing::reply(IngressId::from("ingress-a"), corr.clone(), &query, &[]);
    let second = routing::reply(IngressId::from("ingress-b"), corr, &query, &[]);

    assert_ne!(first, second);
}
