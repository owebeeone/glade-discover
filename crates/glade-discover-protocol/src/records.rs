use crate::{ClaimId, ComputeKey, DefRevId, GrantId, Head, NodeId, Principal, Slot, StreamId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Shape {
    Value,
    Log,
    Stream,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OpEnvelope {
    pub stream: StreamId,
    pub origin: Principal,
    pub seq: u64,
    pub prev: Option<[u8; 32]>,
    pub lamport: u64,
    pub refs: Vec<Head>,
    pub shape: Shape,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SignedOp {
    pub(crate) envelope: OpEnvelope,
    pub(crate) signature: Vec<u8>,
    pub(crate) canonical_bytes: Vec<u8>,
}

impl SignedOp {
    #[must_use]
    pub fn envelope(&self) -> &OpEnvelope {
        &self.envelope
    }

    #[must_use]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    #[must_use]
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DirectoryRecord {
    ServeClaim(ServeClaim),
    ServiceInstanceClaim(ServiceInstanceClaim),
    CapabilityGrant(CapabilityGrant),
    CapabilityRevocation(CapabilityRevocation),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ServeClaim {
    pub node: NodeId,
    pub share: String,
    pub claim_id: ClaimId,
    pub grant_ref: GrantId,
    pub lease_expiry_ms: i64,
    pub epoch: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ServiceInstanceClaim {
    pub node: NodeId,
    pub share: String,
    pub glade_id: String,
    pub key: Vec<u8>,
    pub claim_id: ClaimId,
    pub def_ref: DefRevId,
    pub exec_grant_ref: GrantId,
    pub compute_key: ComputeKey,
    pub lease_expiry_ms: i64,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CapabilityVerb {
    Serve,
    Execute,
    Takeover,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExecutionScope {
    pub def_ref: DefRevId,
    pub compute_key: ComputeKey,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GrantScope {
    Execution(ExecutionScope),
    Takeover { slot: Slot, supersedes: ClaimId },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CapabilityGrant {
    pub grant_id: GrantId,
    pub issuer: Principal,
    pub principal: Principal,
    pub share: String,
    pub verbs: Vec<CapabilityVerb>,
    pub scope: Option<GrantScope>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CapabilityRevocation {
    pub revokes: GrantId,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ClaimIdentity {
    Mint,
    Existing(ClaimId),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ClaimDraft {
    Workspace {
        node: NodeId,
        share: String,
        identity: ClaimIdentity,
        grant_ref: GrantId,
        lease_expiry_ms: i64,
        epoch: u64,
    },
    Service {
        node: NodeId,
        share: String,
        glade_id: String,
        key: Vec<u8>,
        identity: ClaimIdentity,
        def_ref: DefRevId,
        exec_grant_ref: GrantId,
        compute_key: ComputeKey,
        lease_expiry_ms: i64,
        epoch: u64,
    },
}
