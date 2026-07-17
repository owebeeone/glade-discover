//! Trusted ingress boundary for discovery routing and derived placement.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroUsize;

use glade_discover_core::{PrincipalCtx, RouteAns};
use glade_discover_protocol::{Corr, IngressId, NodeId, RouteQuery};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedRequest {
    pub ingress: IngressId,
    pub corr: Corr,
    pub principal: PrincipalCtx,
    pub query: RouteQuery,
}

/// Canonical source set whose read authority guards a target or derivation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceClosure(BTreeSet<String>);

impl SourceClosure {
    #[must_use]
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(names.into_iter().map(Into::into).collect())
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.0.iter()
    }
}

/// INV-7 evidence bound to the authenticated request and exact source closure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizationEvidence {
    pub principal: PrincipalCtx,
    pub query: RouteQuery,
    pub source_closure: SourceClosure,
    pub policy_revision: u64,
}

pub trait ConsumerPolicy {
    /// Evaluates the current policy fold. `required` is supplied for replay so
    /// the cached result can be exposed only under its original source closure.
    fn authorize(
        &mut self,
        principal: &PrincipalCtx,
        query: &RouteQuery,
        required: Option<&SourceClosure>,
    ) -> Option<AuthorizationEvidence>;
}

pub trait DiscoveryRouter {
    fn resolve(
        &mut self,
        ingress: &IngressId,
        corr: &Corr,
        principal: &PrincipalCtx,
        query: &RouteQuery,
    ) -> RouteAns;
}

/// Internal definition candidate. Its opaque identity is never returned to the
/// ingress caller; its source closure must equal the authorization evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceCandidate {
    pub opaque_definition: Vec<u8>,
    pub source_closure: SourceClosure,
}

pub trait ServiceManager {
    /// Matches a definition but does not instantiate it.
    fn match_candidate(
        &mut self,
        principal: &PrincipalCtx,
        query: &RouteQuery,
    ) -> Option<ServiceCandidate>;

    /// Runs D5 and placement only with closure-bound INV-7 evidence.
    fn place(
        &mut self,
        principal: &PrincipalCtx,
        query: &RouteQuery,
        candidate: &ServiceCandidate,
        evidence: &AuthorizationEvidence,
    ) -> ServiceResolution;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServiceResolution {
    Ready { node: NodeId },
    NoClaim,
    Denied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrustedResult {
    Denied,
    Matched { node: NodeId },
    NoClaim,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IngressLimits {
    pub completed_total: NonZeroUsize,
    pub completed_per_ingress: NonZeroUsize,
    pub max_key_bytes: NonZeroUsize,
}

/// Admission failures are not terminal route results and are never cached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    KeyTooLarge,
    IngressCapacity,
    TotalCapacity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Completed {
    result: TrustedResult,
    source_closure: Option<SourceClosure>,
}

pub struct TrustedIngress<P, D, S> {
    policy: P,
    discovery: D,
    service_manager: S,
    limits: IngressLimits,
    completed: BTreeMap<(IngressId, Corr), Completed>,
}

impl<P, D, S> TrustedIngress<P, D, S>
where
    P: ConsumerPolicy,
    D: DiscoveryRouter,
    S: ServiceManager,
{
    #[must_use]
    pub fn new(limits: IngressLimits, policy: P, discovery: D, service_manager: S) -> Self {
        Self {
            policy,
            discovery,
            service_manager,
            limits,
            completed: BTreeMap::new(),
        }
    }

    pub fn close_ingress(&mut self, ingress: &IngressId) -> usize {
        let before = self.completed.len();
        self.completed
            .retain(|(entry_ingress, _), _| entry_ingress != ingress);
        before - self.completed.len()
    }

    /// Handles one admitted request. A topology-bearing or `NoClaim` replay
    /// re-evaluates current INV-7, while discovery and service placement remain
    /// exactly-once for the completed correlation.
    pub fn handle(&mut self, request: TrustedRequest) -> Result<TrustedResult, AdmissionError> {
        let key_bytes = request
            .ingress
            .as_str()
            .len()
            .checked_add(request.corr.as_str().len())
            .ok_or(AdmissionError::KeyTooLarge)?;
        if key_bytes > self.limits.max_key_bytes.get() {
            return Err(AdmissionError::KeyTooLarge);
        }

        let key = (request.ingress.clone(), request.corr.clone());
        if let Some(completed) = self.completed.get(&key).cloned() {
            return Ok(self.replay(&request, &completed));
        }
        if self.completed.len() >= self.limits.completed_total.get() {
            return Err(AdmissionError::TotalCapacity);
        }
        if self.completed_for(&request.ingress) >= self.limits.completed_per_ingress.get() {
            return Err(AdmissionError::IngressCapacity);
        }

        let Some(evidence) = self
            .policy
            .authorize(&request.principal, &request.query, None)
        else {
            let completed = Completed {
                result: TrustedResult::Denied,
                source_closure: None,
            };
            self.completed.insert(key, completed.clone());
            return Ok(completed.result);
        };
        if !evidence_matches(&evidence, &request, None) {
            let completed = Completed {
                result: TrustedResult::Denied,
                source_closure: None,
            };
            self.completed.insert(key, completed.clone());
            return Ok(completed.result);
        }

        let completed = match self.discovery.resolve(
            &request.ingress,
            &request.corr,
            &request.principal,
            &request.query,
        ) {
            RouteAns::Matched { node } => Completed {
                result: TrustedResult::Matched { node },
                source_closure: Some(evidence.source_closure),
            },
            RouteAns::NoClaim => self.resolve_service(&request, evidence),
        };
        self.completed.insert(key, completed.clone());
        Ok(completed.result)
    }

    fn replay(&mut self, request: &TrustedRequest, completed: &Completed) -> TrustedResult {
        let Some(required) = &completed.source_closure else {
            return completed.result.clone();
        };
        self.policy
            .authorize(&request.principal, &request.query, Some(required))
            .filter(|evidence| evidence_matches(evidence, request, Some(required)))
            .map_or(TrustedResult::Denied, |_| completed.result.clone())
    }

    fn resolve_service(
        &mut self,
        request: &TrustedRequest,
        evidence: AuthorizationEvidence,
    ) -> Completed {
        let Some(candidate) = self
            .service_manager
            .match_candidate(&request.principal, &request.query)
        else {
            return Completed {
                result: TrustedResult::NoClaim,
                source_closure: Some(evidence.source_closure),
            };
        };
        if candidate.source_closure != evidence.source_closure {
            return Completed {
                result: TrustedResult::Denied,
                source_closure: None,
            };
        }
        let source_closure = candidate.source_closure.clone();
        let resolution =
            self.service_manager
                .place(&request.principal, &request.query, &candidate, &evidence);
        match resolution {
            ServiceResolution::Ready { node } => Completed {
                result: TrustedResult::Matched { node },
                source_closure: Some(source_closure),
            },
            ServiceResolution::NoClaim => Completed {
                result: TrustedResult::NoClaim,
                source_closure: Some(source_closure),
            },
            ServiceResolution::Denied => Completed {
                result: TrustedResult::Denied,
                source_closure: None,
            },
        }
    }

    fn completed_for(&self, ingress: &IngressId) -> usize {
        self.completed
            .keys()
            .filter(|(candidate, _)| candidate == ingress)
            .count()
    }
}

fn evidence_matches(
    evidence: &AuthorizationEvidence,
    request: &TrustedRequest,
    required: Option<&SourceClosure>,
) -> bool {
    evidence.principal == request.principal
        && evidence.query == request.query
        && required.is_none_or(|required| evidence.source_closure == *required)
}
