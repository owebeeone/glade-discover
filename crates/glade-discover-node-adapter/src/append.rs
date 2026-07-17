use glade_discover_core::{Event, VerificationResult};
use glade_discover_protocol::Generation;
use glade_discover_protocol::{
    ClaimDraft, ClaimId, ClaimIdentity, DecodeError, DirectoryRecord, IntentId, OpEnvelope,
    Principal, RecordId, ServeClaim, ServiceInstanceClaim, Shape, SignedOp, Slot, StreamId,
    decode_directory_record, decode_signed_op, encode_directory_record, encode_signed_op,
    record_id, unsigned_canonical_bytes,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Allocation {
    pub stream: StreamId,
    pub seq: u64,
    pub prev: Option<[u8; 32]>,
    pub lamport: u64,
    pub shape: Shape,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AppendKey {
    pub slot: Slot,
    pub generation: Generation,
    pub intent: IntentId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableAccepted {
    pub draft: ClaimDraft,
    pub canonical: Vec<u8>,
}

pub trait AppendHost {
    type Error;

    fn accepted(&mut self, key: &AppendKey) -> Result<Option<DurableAccepted>, Self::Error>;
    fn allocate(&mut self, draft: &ClaimDraft) -> Result<Allocation, Self::Error>;
    fn signer(&self) -> Principal;
    fn sign(&mut self, unsigned: &[u8]) -> Result<Vec<u8>, Self::Error>;
    /// Verifies canonical signed bytes using the trusted node's B5 verifier.
    fn verify(&mut self, op: &SignedOp) -> Result<bool, Self::Error>;
    fn persist_accepted(
        &mut self,
        key: &AppendKey,
        draft: &ClaimDraft,
        canonical: &[u8],
    ) -> Result<(), Self::Error>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppendError<E> {
    Host(E),
    Protocol(DecodeError),
    DraftMismatch,
    SignerMismatch,
    PayloadMismatch,
    SignatureInvalid,
}

pub fn handle_append<H: AppendHost>(
    host: &mut H,
    slot: &Slot,
    generation: Generation,
    intent: &IntentId,
    draft: &ClaimDraft,
) -> Result<Event, AppendError<H::Error>> {
    let key = AppendKey {
        slot: slot.clone(),
        generation,
        intent: intent.clone(),
    };
    if let Some(accepted) = host.accepted(&key).map_err(AppendError::Host)? {
        if accepted.draft != *draft {
            return Err(AppendError::DraftMismatch);
        }
        let op = decode_signed_op(&accepted.canonical).map_err(AppendError::Protocol)?;
        let signer = host.signer();
        if op.envelope().origin != signer {
            return Err(AppendError::SignerMismatch);
        }
        let stored_record =
            decode_directory_record(&op.envelope().payload).map_err(AppendError::Protocol)?;
        if stored_record != finalize(draft, record_id(&op)) {
            return Err(AppendError::PayloadMismatch);
        }
        if !host.verify(&op).map_err(AppendError::Host)? {
            return Err(AppendError::SignatureInvalid);
        }
        return Ok(accepted_event(intent, op, signer));
    }

    let allocation = host.allocate(draft).map_err(AppendError::Host)?;
    let origin = host.signer();
    let identity = RecordId {
        stream: allocation.stream.clone(),
        origin: origin.clone(),
        seq: allocation.seq,
    };
    let record = finalize(draft, identity);
    let envelope = OpEnvelope {
        stream: allocation.stream,
        origin: origin.clone(),
        seq: allocation.seq,
        prev: allocation.prev,
        lamport: allocation.lamport,
        refs: Vec::new(),
        shape: allocation.shape,
        payload: encode_directory_record(&record).map_err(AppendError::Protocol)?,
    };
    let unsigned = encode_signed_op(&envelope, &[]).map_err(AppendError::Protocol)?;
    let signature = host
        .sign(&unsigned_canonical_bytes(&unsigned))
        .map_err(AppendError::Host)?;
    let op = encode_signed_op(&envelope, &signature).map_err(AppendError::Protocol)?;
    if !host.verify(&op).map_err(AppendError::Host)? {
        return Err(AppendError::SignatureInvalid);
    }
    host.persist_accepted(&key, draft, op.canonical_bytes())
        .map_err(AppendError::Host)?;
    Ok(accepted_event(intent, op, origin))
}

fn finalize(draft: &ClaimDraft, allocated: RecordId) -> DirectoryRecord {
    match draft {
        ClaimDraft::Workspace {
            node,
            share,
            identity,
            grant_ref,
            lease_expiry_ms,
            epoch,
        } => DirectoryRecord::ServeClaim(ServeClaim {
            node: node.clone(),
            share: share.clone(),
            claim_id: finalized_identity(identity, allocated),
            grant_ref: grant_ref.clone(),
            lease_expiry_ms: *lease_expiry_ms,
            epoch: *epoch,
        }),
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
        } => DirectoryRecord::ServiceInstanceClaim(ServiceInstanceClaim {
            node: node.clone(),
            share: share.clone(),
            glade_id: glade_id.clone(),
            key: key.clone(),
            claim_id: finalized_identity(identity, allocated),
            def_ref: def_ref.clone(),
            exec_grant_ref: exec_grant_ref.clone(),
            compute_key: compute_key.clone(),
            lease_expiry_ms: *lease_expiry_ms,
            epoch: *epoch,
        }),
    }
}

fn finalized_identity(identity: &ClaimIdentity, allocated: RecordId) -> ClaimId {
    match identity {
        ClaimIdentity::Mint => ClaimId::from(allocated),
        ClaimIdentity::Existing(existing) => existing.clone(),
    }
}

fn accepted_event(intent: &IntentId, op: SignedOp, signer: Principal) -> Event {
    Event::OpAccepted {
        intent: intent.clone(),
        op: Box::new(op),
        verification: VerificationResult::Valid { signer },
    }
}
