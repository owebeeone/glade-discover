use std::cell::RefCell;
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::rc::Rc;

use glade_discover_core::{PrincipalCtx, RouteAns};
use glade_discover_node_adapter::ingress::{
    AdmissionError, AuthorizationEvidence, ConsumerPolicy, DiscoveryRouter, IngressLimits,
    ServiceCandidate, ServiceManager, ServiceResolution, SourceClosure, TrustedIngress,
    TrustedRequest, TrustedResult,
};
use glade_discover_protocol::{Corr, IngressId, NodeId, Principal, RouteQuery, Slot};

#[derive(Clone)]
struct PolicyState {
    readable: BTreeSet<String>,
    allowed_principals: BTreeSet<Principal>,
    route_closure: SourceClosure,
    revision: u64,
}

struct PolicyStub {
    state: Rc<RefCell<PolicyState>>,
    calls: Rc<RefCell<Vec<String>>>,
}

impl ConsumerPolicy for PolicyStub {
    fn authorize(
        &mut self,
        principal: &PrincipalCtx,
        query: &RouteQuery,
        required: Option<&SourceClosure>,
    ) -> Option<AuthorizationEvidence> {
        let state = self.state.borrow();
        let closure = required
            .cloned()
            .unwrap_or_else(|| state.route_closure.clone());
        self.calls.borrow_mut().push(format!(
            "policy:{}:{}",
            principal.principal,
            closure.names().cloned().collect::<Vec<_>>().join("+")
        ));
        if !state.allowed_principals.contains(&principal.principal)
            || !closure
                .names()
                .all(|source| state.readable.contains(source))
        {
            return None;
        }
        Some(AuthorizationEvidence {
            principal: principal.clone(),
            query: query.clone(),
            source_closure: closure,
            policy_revision: state.revision,
        })
    }
}

struct DiscoveryStub {
    answer: RouteAns,
    calls: Rc<RefCell<Vec<String>>>,
}

impl DiscoveryRouter for DiscoveryStub {
    fn resolve(
        &mut self,
        _ingress: &IngressId,
        _corr: &Corr,
        _principal: &PrincipalCtx,
        _query: &RouteQuery,
    ) -> RouteAns {
        self.calls.borrow_mut().push("discovery".to_owned());
        self.answer.clone()
    }
}

struct ServiceStub {
    candidate: Option<ServiceCandidate>,
    resolution: ServiceResolution,
    calls: Rc<RefCell<Vec<String>>>,
}

impl ServiceManager for ServiceStub {
    fn match_candidate(
        &mut self,
        _principal: &PrincipalCtx,
        _query: &RouteQuery,
    ) -> Option<ServiceCandidate> {
        self.calls.borrow_mut().push("match".to_owned());
        self.candidate.clone()
    }

    fn place(
        &mut self,
        _principal: &PrincipalCtx,
        _query: &RouteQuery,
        candidate: &ServiceCandidate,
        evidence: &AuthorizationEvidence,
    ) -> ServiceResolution {
        assert_eq!(candidate.source_closure, evidence.source_closure);
        self.calls.borrow_mut().push("place".to_owned());
        self.resolution.clone()
    }
}

fn closure(names: &[&str]) -> SourceClosure {
    SourceClosure::new(names.iter().copied())
}

fn readable(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn policy_state(route_closure: SourceClosure, readable_sources: &[&str]) -> PolicyState {
    PolicyState {
        readable: readable(readable_sources),
        allowed_principals: BTreeSet::from([Principal::from("reader")]),
        route_closure,
        revision: 1,
    }
}

fn limits(total: usize, per_ingress: usize, key_bytes: usize) -> IngressLimits {
    IngressLimits {
        completed_total: NonZeroUsize::new(total).expect("non-zero total"),
        completed_per_ingress: NonZeroUsize::new(per_ingress).expect("non-zero per ingress"),
        max_key_bytes: NonZeroUsize::new(key_bytes).expect("non-zero key bytes"),
    }
}

fn adapter(
    state: Rc<RefCell<PolicyState>>,
    answer: RouteAns,
    candidate: Option<ServiceCandidate>,
    resolution: ServiceResolution,
    limits: IngressLimits,
    calls: &Rc<RefCell<Vec<String>>>,
) -> TrustedIngress<PolicyStub, DiscoveryStub, ServiceStub> {
    TrustedIngress::new(
        limits,
        PolicyStub {
            state,
            calls: Rc::clone(calls),
        },
        DiscoveryStub {
            answer,
            calls: Rc::clone(calls),
        },
        ServiceStub {
            candidate,
            resolution,
            calls: Rc::clone(calls),
        },
    )
}

fn request(ingress: &str, corr: &str, principal: &str, share: &str) -> TrustedRequest {
    TrustedRequest {
        ingress: IngressId::from(ingress),
        corr: Corr::from(corr),
        principal: PrincipalCtx {
            principal: Principal::from(principal),
            authenticated_context: vec![0xb3],
        },
        query: RouteQuery {
            slot: Slot::Workspace {
                share: share.to_owned(),
            },
        },
    }
}

fn candidate(source_closure: SourceClosure) -> ServiceCandidate {
    ServiceCandidate {
        opaque_definition: vec![0xd5],
        source_closure,
    }
}

#[test]
fn unauthorized_request_stops_before_discovery_and_service_without_leaks() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let state = Rc::new(RefCell::new(policy_state(closure(&["left"]), &[])));
    let mut ingress = adapter(
        state,
        RouteAns::Matched {
            node: NodeId::from("secret-node"),
        },
        Some(candidate(closure(&["left"]))),
        ServiceResolution::Ready {
            node: NodeId::from("secret-service-node"),
        },
        limits(8, 4, 128),
        &calls,
    );

    assert_eq!(
        ingress.handle(request("in", "corr", "reader", "private")),
        Ok(TrustedResult::Denied)
    );
    assert_eq!(calls.borrow().as_slice(), ["policy:reader:left"]);
}

#[test]
fn candidate_source_closure_must_equal_the_authorization_evidence_before_placement() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let state = Rc::new(RefCell::new(policy_state(
        closure(&["left"]),
        &["left", "right"],
    )));
    let mut ingress = adapter(
        state,
        RouteAns::NoClaim,
        Some(candidate(closure(&["right"]))),
        ServiceResolution::Ready {
            node: NodeId::from("must-not-place"),
        },
        limits(8, 4, 128),
        &calls,
    );

    assert_eq!(
        ingress.handle(request("in", "corr", "reader", "derived")),
        Ok(TrustedResult::Denied)
    );
    assert_eq!(
        calls.borrow().as_slice(),
        ["policy:reader:left", "discovery", "match"]
    );
}

#[test]
fn inv7_left_right_neither_both_vectors_gate_live_definition_placement() {
    for (name, sources, expected) in [
        ("neither", vec![], TrustedResult::Denied),
        ("left", vec!["left"], TrustedResult::Denied),
        ("right", vec!["right"], TrustedResult::Denied),
        (
            "both",
            vec!["left", "right"],
            TrustedResult::Matched {
                node: NodeId::from("placed-node"),
            },
        ),
    ] {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let state = Rc::new(RefCell::new(policy_state(
            closure(&["left", "right"]),
            &sources,
        )));
        let mut ingress = adapter(
            state,
            RouteAns::NoClaim,
            Some(candidate(closure(&["left", "right"]))),
            ServiceResolution::Ready {
                node: NodeId::from("placed-node"),
            },
            limits(8, 4, 128),
            &calls,
        );

        assert_eq!(
            ingress.handle(request("in", name, "reader", "diff")),
            Ok(expected),
            "{name}"
        );
        assert_eq!(calls.borrow().contains(&"place".to_owned()), name == "both");
    }
}

#[test]
fn cached_delivery_reruns_inv7_for_all_source_vectors_without_redispatch() {
    for (name, sources, expected) in [
        ("neither", vec![], TrustedResult::Denied),
        ("left", vec!["left"], TrustedResult::Denied),
        ("right", vec!["right"], TrustedResult::Denied),
        (
            "both",
            vec!["left", "right"],
            TrustedResult::Matched {
                node: NodeId::from("placed-node"),
            },
        ),
    ] {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let state = Rc::new(RefCell::new(policy_state(
            closure(&["left", "right"]),
            &["left", "right"],
        )));
        let mut ingress = adapter(
            Rc::clone(&state),
            RouteAns::NoClaim,
            Some(candidate(closure(&["left", "right"]))),
            ServiceResolution::Ready {
                node: NodeId::from("placed-node"),
            },
            limits(8, 4, 128),
            &calls,
        );
        let req = request("in", "same", "reader", "diff");
        assert!(matches!(
            ingress.handle(req.clone()),
            Ok(TrustedResult::Matched { .. })
        ));
        let before = calls.borrow().clone();
        state.borrow_mut().readable = readable(&sources);
        state.borrow_mut().revision += 1;

        assert_eq!(ingress.handle(req), Ok(expected), "cached {name}");
        let after = calls.borrow();
        assert_eq!(after.len(), before.len() + 1, "cached {name}");
        assert_eq!(
            after.last().expect("policy replay"),
            "policy:reader:left+right"
        );
        assert_eq!(
            after
                .iter()
                .filter(|call| call.as_str() == "discovery")
                .count(),
            1
        );
        assert_eq!(
            after.iter().filter(|call| call.as_str() == "match").count(),
            1
        );
        assert_eq!(
            after.iter().filter(|call| call.as_str() == "place").count(),
            1
        );
    }
}

#[test]
fn source_revocation_on_replay_suppresses_cached_node_until_authority_returns() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let state = Rc::new(RefCell::new(policy_state(
        closure(&["left", "right"]),
        &["left", "right"],
    )));
    let mut ingress = adapter(
        Rc::clone(&state),
        RouteAns::Matched {
            node: NodeId::from("warm-node"),
        },
        None,
        ServiceResolution::NoClaim,
        limits(8, 4, 128),
        &calls,
    );
    let req = request("in", "same", "reader", "diff");
    let matched = TrustedResult::Matched {
        node: NodeId::from("warm-node"),
    };
    assert_eq!(ingress.handle(req.clone()), Ok(matched.clone()));

    state.borrow_mut().readable.remove("right");
    state.borrow_mut().revision += 1;
    assert_eq!(ingress.handle(req.clone()), Ok(TrustedResult::Denied));

    state.borrow_mut().readable.insert("right".to_owned());
    state.borrow_mut().revision += 1;
    assert_eq!(ingress.handle(req), Ok(matched));
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|call| call.as_str() == "discovery")
            .count(),
        1
    );
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|call| call.starts_with("policy:"))
            .count(),
        3
    );
}

#[test]
fn cached_no_claim_and_changed_principal_are_reauthorized_without_redispatch() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let state = Rc::new(RefCell::new(policy_state(closure(&["left"]), &["left"])));
    let mut ingress = adapter(
        Rc::clone(&state),
        RouteAns::NoClaim,
        None,
        ServiceResolution::NoClaim,
        limits(8, 4, 128),
        &calls,
    );
    let req = request("in", "same", "reader", "missing");
    assert_eq!(ingress.handle(req.clone()), Ok(TrustedResult::NoClaim));
    let downstream_calls = calls.borrow().clone();

    state.borrow_mut().readable.clear();
    assert_eq!(ingress.handle(req), Ok(TrustedResult::Denied));
    assert_eq!(calls.borrow().len(), downstream_calls.len() + 1);

    state.borrow_mut().readable.insert("left".to_owned());
    assert_eq!(
        ingress.handle(request("in", "same", "other-reader", "missing")),
        Ok(TrustedResult::Denied)
    );
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|call| call.as_str() == "discovery")
            .count(),
        1
    );
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|call| call.as_str() == "match")
            .count(),
        1
    );
}

#[test]
fn admission_limits_are_non_terminal_byte_bounded_and_fair_per_ingress() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let state = Rc::new(RefCell::new(policy_state(closure(&["left"]), &["left"])));
    let mut ingress = adapter(
        state,
        RouteAns::Matched {
            node: NodeId::from("node"),
        },
        None,
        ServiceResolution::NoClaim,
        limits(2, 1, 12),
        &calls,
    );

    assert!(matches!(
        ingress.handle(request("a", "one", "reader", "slot")),
        Ok(TrustedResult::Matched { .. })
    ));
    assert_eq!(
        ingress.handle(request("a", "two", "reader", "slot")),
        Err(AdmissionError::IngressCapacity)
    );
    assert_eq!(
        ingress.handle(request("very-long", "corr-long", "reader", "slot")),
        Err(AdmissionError::KeyTooLarge)
    );
    assert!(matches!(
        ingress.handle(request("b", "one", "reader", "slot")),
        Ok(TrustedResult::Matched { .. })
    ));
    assert_eq!(
        ingress.handle(request("c", "one", "reader", "slot")),
        Err(AdmissionError::TotalCapacity)
    );

    assert_eq!(ingress.close_ingress(&IngressId::from("a")), 1);
    assert!(matches!(
        ingress.handle(request("a", "two", "reader", "slot")),
        Ok(TrustedResult::Matched { .. })
    ));
}

#[test]
fn exact_key_byte_limit_is_accepted_and_duplicate_keys_include_ingress() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let state = Rc::new(RefCell::new(policy_state(closure(&["left"]), &["left"])));
    let mut ingress = adapter(
        state,
        RouteAns::NoClaim,
        None,
        ServiceResolution::NoClaim,
        limits(3, 2, 5),
        &calls,
    );

    assert_eq!(
        ingress.handle(request("ab", "cde", "reader", "slot")),
        Ok(TrustedResult::NoClaim)
    );
    assert_eq!(
        ingress.handle(request("xy", "cde", "reader", "slot")),
        Ok(TrustedResult::NoClaim)
    );
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|call| call.as_str() == "discovery")
            .count(),
        2
    );
}
