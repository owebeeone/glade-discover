//! Canonical records and wire protocol for Glade discovery.

mod codec;
mod ids;
mod records;
mod wire;

pub use codec::{
    DecodeError, MAX_MESSAGE_BYTES, MAX_RECORD_BYTES, MAX_SYNC_ID_BYTES, MIN_SIGNED_OP_BYTES,
    decode_directory_record, decode_signed_op, decode_stream_head, decode_wire_msg,
    encode_directory_record, encode_signed_op, encode_stream_head, encode_wire_msg, op_hash,
    record_id, sync_list_body_len, unsigned_canonical_bytes,
};
pub use ids::{
    ClaimId, ComputeKey, Corr, DefRevId, Generation, GrantId, IngressId, IntentId, NodeId,
    Principal, RecordId, RouteQuery, Slot, StreamId, SyncId,
};
pub use records::{
    CapabilityGrant, CapabilityRevocation, CapabilityVerb, ClaimDraft, ClaimIdentity,
    DirectoryRecord, ExecutionScope, GrantScope, OpEnvelope, ServeClaim, ServiceInstanceClaim,
    Shape, SignedOp,
};
pub use wire::{Head, StreamHead, WireMsg};

/// Returns the stable role name used by workspace smoke tests.
#[must_use]
pub const fn crate_name() -> &'static str {
    "protocol"
}
