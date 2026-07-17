//! Canonical, fail-closed CBOR codec for discovery records and messages.

use crate::{
    CapabilityGrant, CapabilityRevocation, CapabilityVerb, ClaimId, DefRevId, DirectoryRecord,
    ExecutionScope, GrantId, GrantScope, Head, NodeId, OpEnvelope, Principal, RecordId, ServeClaim,
    ServiceInstanceClaim, Shape, SignedOp, Slot, StreamHead, StreamId, SyncId, WireMsg,
};
use sha2::{Digest, Sha256};

pub const MAX_RECORD_BYTES: usize = 16 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const MAX_SYNC_ID_BYTES: usize = 256;
pub const MIN_SIGNED_OP_BYTES: usize = 23;
const MAX_KEY_BYTES: usize = 4 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    RecordTooLarge,
    MessageTooLarge,
    SyncIdTooLarge,
    Truncated,
    TrailingData,
    NonCanonical,
    WrongType,
    InvalidUtf8,
    IntegerOutOfRange,
    MissingField(u64),
    UnknownField(u64),
    UnknownEnum {
        name: &'static str,
        value: u64,
    },
    UnknownWireKind(u8),
    UnsupportedVersion(u64),
    InvalidValue(&'static str),
    InvalidLength {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    TooManyItems {
        field: &'static str,
        max: usize,
        actual: usize,
    },
    KeyTooLarge,
}

#[must_use]
pub fn record_id(op: &SignedOp) -> RecordId {
    RecordId {
        stream: op.envelope.stream.clone(),
        origin: op.envelope.origin.clone(),
        seq: op.envelope.seq,
    }
}

#[must_use]
pub fn unsigned_canonical_bytes(op: &SignedOp) -> Vec<u8> {
    encode_unsigned_envelope(&op.envelope)
}

/// Hash the exact canonical unsigned envelope used by signatures and chain
/// links.
#[must_use]
pub fn op_hash(op: &SignedOp) -> [u8; 32] {
    Sha256::digest(unsigned_canonical_bytes(op)).into()
}

pub fn decode_signed_op(bytes: &[u8]) -> Result<SignedOp, DecodeError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(DecodeError::RecordTooLarge);
    }
    let mut decoder = Decoder::new(bytes);
    let op = decoder.signed_op()?;
    decoder.finish()?;
    Ok(op)
}

/// Construct a structurally valid canonical signed operation.
///
/// Cryptographic validity remains the caller/verifier's responsibility. This
/// function provides the one canonical representation used by adapters and
/// deterministic scenario fixtures.
pub fn encode_signed_op(envelope: &OpEnvelope, signature: &[u8]) -> Result<SignedOp, DecodeError> {
    let unsigned = encode_unsigned_envelope(envelope);
    let mut encoder = Encoder::new();
    encoder.map(11);
    // The unsigned representation is a ten-entry map. Its initial map byte is
    // replaced by the eleven-entry map above; all field bytes remain exact.
    encoder.raw(unsigned.get(1..).ok_or(DecodeError::Truncated)?);
    encoder.uint(11);
    encoder.bytes(signature);
    decode_signed_op(&encoder.finish())
}

pub fn decode_directory_record(bytes: &[u8]) -> Result<DirectoryRecord, DecodeError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(DecodeError::RecordTooLarge);
    }
    let mut decoder = Decoder::new(bytes);
    decoder.exact_map(3)?;
    decoder.field(1)?;
    let version = decoder.uint()?;
    if version != 1 {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    decoder.field(2)?;
    let kind = decoder.uint()?;
    decoder.field(3)?;
    let record = match kind {
        0 => DirectoryRecord::ServeClaim(decoder.serve_claim()?),
        1 => DirectoryRecord::ServiceInstanceClaim(decoder.service_instance_claim()?),
        2 => DirectoryRecord::CapabilityGrant(decoder.capability_grant()?),
        3 => DirectoryRecord::CapabilityRevocation(decoder.capability_revocation()?),
        value => {
            return Err(DecodeError::UnknownEnum {
                name: "DirectoryRecordKind",
                value,
            });
        }
    };
    decoder.finish()?;
    Ok(record)
}

pub fn encode_directory_record(record: &DirectoryRecord) -> Result<Vec<u8>, DecodeError> {
    let mut encoder = Encoder::new();
    encoder.map(3);
    encoder.uint(1);
    encoder.uint(1);
    encoder.uint(2);
    let kind = match record {
        DirectoryRecord::ServeClaim(_) => 0,
        DirectoryRecord::ServiceInstanceClaim(_) => 1,
        DirectoryRecord::CapabilityGrant(_) => 2,
        DirectoryRecord::CapabilityRevocation(_) => 3,
    };
    encoder.uint(kind);
    encoder.uint(3);
    match record {
        DirectoryRecord::ServeClaim(value) => encoder.serve_claim(value),
        DirectoryRecord::ServiceInstanceClaim(value) => encoder.service_instance_claim(value),
        DirectoryRecord::CapabilityGrant(value) => encoder.capability_grant(value)?,
        DirectoryRecord::CapabilityRevocation(value) => encoder.capability_revocation(value),
    }
    let bytes = encoder.finish();
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(DecodeError::RecordTooLarge);
    }
    // Keeping encode and decode on one structural path prevents a future
    // encoder extension from producing bytes this version would not accept.
    decode_directory_record(&bytes)?;
    Ok(bytes)
}

pub fn decode_stream_head(bytes: &[u8]) -> Result<StreamHead, DecodeError> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(DecodeError::MessageTooLarge);
    }
    let mut decoder = Decoder::new(bytes);
    let head = decoder.stream_head()?;
    decoder.finish()?;
    Ok(head)
}

#[must_use]
pub fn encode_stream_head(head: &StreamHead) -> Vec<u8> {
    let mut encoder = Encoder::new();
    encoder.stream_head(head);
    encoder.finish()
}

/// Encode a wire body and return its external `WireKind` discriminator.
#[must_use]
pub fn encode_wire_msg(message: &WireMsg) -> (u8, Vec<u8>) {
    let mut encoder = Encoder::new();
    let tag = match message {
        WireMsg::DirOp { op } => {
            encoder.map(1);
            encoder.uint(1);
            encoder.raw(op.canonical_bytes());
            0
        }
        WireMsg::SyncStart { sync_id, heads } => {
            encoder.map(2);
            encoder.uint(1);
            encoder.text(sync_id.as_str());
            encoder.uint(2);
            encoder.array(heads.len());
            for head in heads {
                encoder.stream_head(head);
            }
            1
        }
        WireMsg::SyncOps { sync_id, ops } => {
            encoder.map(2);
            encoder.uint(1);
            encoder.text(sync_id.as_str());
            encoder.uint(2);
            encoder.array(ops.len());
            for op in ops {
                encoder.raw(op.canonical_bytes());
            }
            2
        }
        WireMsg::SyncEnd { sync_id } => {
            encoder.map(1);
            encoder.uint(1);
            encoder.text(sync_id.as_str());
            3
        }
    };
    (tag, encoder.finish())
}

/// Exact encoded body length for the common `SyncStart`/`SyncOps` list shape.
///
/// `item_bytes` is the checked sum of the raw canonical head or operation
/// encodings. The external wire-kind discriminator is not part of the body.
#[must_use]
pub fn sync_list_body_len(sync_id: &SyncId, item_count: usize, item_bytes: usize) -> Option<usize> {
    3_usize
        .checked_add(cbor_head_bytes(sync_id.as_str().len()))?
        .checked_add(sync_id.as_str().len())?
        .checked_add(cbor_head_bytes(item_count))?
        .checked_add(item_bytes)
}

const fn cbor_head_bytes(value: usize) -> usize {
    match value {
        0..=23 => 1,
        24..=255 => 2,
        256..=65_535 => 3,
        65_536..=4_294_967_295 => 5,
        _ => 9,
    }
}

pub fn decode_wire_msg(tag: u8, bytes: &[u8]) -> Result<WireMsg, DecodeError> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(DecodeError::MessageTooLarge);
    }
    let mut decoder = Decoder::new(bytes);
    let message = match tag {
        0 => {
            decoder.exact_map(1)?;
            decoder.field(1)?;
            WireMsg::DirOp {
                op: Box::new(decoder.signed_op()?),
            }
        }
        1 => {
            decoder.exact_map(2)?;
            decoder.field(1)?;
            let sync_id = decoder.sync_id()?;
            decoder.field(2)?;
            let count = decoder.array()?;
            let mut heads = Vec::new();
            for _ in 0..count {
                heads.push(decoder.stream_head()?);
            }
            WireMsg::SyncStart { sync_id, heads }
        }
        2 => {
            decoder.exact_map(2)?;
            decoder.field(1)?;
            let sync_id = decoder.sync_id()?;
            decoder.field(2)?;
            let count = decoder.array()?;
            if count > 256 {
                return Err(DecodeError::TooManyItems {
                    field: "ops",
                    max: 256,
                    actual: count,
                });
            }
            let mut ops = Vec::new();
            for _ in 0..count {
                ops.push(decoder.signed_op()?);
            }
            WireMsg::SyncOps { sync_id, ops }
        }
        3 => {
            decoder.exact_map(1)?;
            decoder.field(1)?;
            WireMsg::SyncEnd {
                sync_id: decoder.sync_id()?,
            }
        }
        other => return Err(DecodeError::UnknownWireKind(other)),
    };
    decoder.finish()?;
    Ok(message)
}

struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn finish(&self) -> Result<(), DecodeError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(DecodeError::TrailingData)
        }
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        let value = *self
            .bytes
            .get(self.position)
            .ok_or(DecodeError::Truncated)?;
        self.position += 1;
        Ok(value)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or(DecodeError::IntegerOutOfRange)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(DecodeError::Truncated)?;
        self.position = end;
        Ok(value)
    }

    fn argument(&mut self, expected_major: u8) -> Result<u64, DecodeError> {
        let initial = self.byte()?;
        if initial >> 5 != expected_major {
            return Err(DecodeError::WrongType);
        }
        let additional = initial & 0x1f;
        match additional {
            value @ 0..=23 => Ok(u64::from(value)),
            24 => {
                let value = u64::from(self.byte()?);
                if value < 24 {
                    Err(DecodeError::NonCanonical)
                } else {
                    Ok(value)
                }
            }
            25 => {
                let bytes: [u8; 2] = self
                    .take(2)?
                    .try_into()
                    .map_err(|_| DecodeError::Truncated)?;
                let value = u64::from(u16::from_be_bytes(bytes));
                if value <= u64::from(u8::MAX) {
                    Err(DecodeError::NonCanonical)
                } else {
                    Ok(value)
                }
            }
            26 => {
                let bytes: [u8; 4] = self
                    .take(4)?
                    .try_into()
                    .map_err(|_| DecodeError::Truncated)?;
                let value = u64::from(u32::from_be_bytes(bytes));
                if value <= u64::from(u16::MAX) {
                    Err(DecodeError::NonCanonical)
                } else {
                    Ok(value)
                }
            }
            27 => {
                let bytes: [u8; 8] = self
                    .take(8)?
                    .try_into()
                    .map_err(|_| DecodeError::Truncated)?;
                let value = u64::from_be_bytes(bytes);
                if value <= u64::from(u32::MAX) {
                    Err(DecodeError::NonCanonical)
                } else {
                    Ok(value)
                }
            }
            _ => Err(DecodeError::NonCanonical),
        }
    }

    fn length(&mut self, major: u8) -> Result<usize, DecodeError> {
        usize::try_from(self.argument(major)?).map_err(|_| DecodeError::IntegerOutOfRange)
    }

    fn uint(&mut self) -> Result<u64, DecodeError> {
        self.argument(0)
    }

    fn signed_i64(&mut self) -> Result<i64, DecodeError> {
        let initial = *self
            .bytes
            .get(self.position)
            .ok_or(DecodeError::Truncated)?;
        match initial >> 5 {
            0 => {
                let value = self.uint()?;
                i64::try_from(value).map_err(|_| DecodeError::IntegerOutOfRange)
            }
            1 => {
                let magnitude = self.argument(1)?;
                let magnitude =
                    i64::try_from(magnitude).map_err(|_| DecodeError::IntegerOutOfRange)?;
                Ok(-1 - magnitude)
            }
            _ => Err(DecodeError::WrongType),
        }
    }

    fn map(&mut self) -> Result<usize, DecodeError> {
        self.length(5)
    }

    fn exact_map(&mut self, expected: usize) -> Result<(), DecodeError> {
        let actual = self.map()?;
        match actual.cmp(&expected) {
            std::cmp::Ordering::Equal => Ok(()),
            std::cmp::Ordering::Less => Err(DecodeError::MissingField(
                u64::try_from(actual + 1).map_err(|_| DecodeError::IntegerOutOfRange)?,
            )),
            std::cmp::Ordering::Greater => Err(DecodeError::UnknownField(
                u64::try_from(expected + 1).map_err(|_| DecodeError::IntegerOutOfRange)?,
            )),
        }
    }

    fn field(&mut self, expected: u64) -> Result<(), DecodeError> {
        let actual = self.uint()?;
        if actual == expected {
            Ok(())
        } else if actual < expected {
            Err(DecodeError::NonCanonical)
        } else if actual <= expected + 10 {
            Err(DecodeError::MissingField(expected))
        } else {
            Err(DecodeError::UnknownField(actual))
        }
    }

    fn array(&mut self) -> Result<usize, DecodeError> {
        self.length(4)
    }

    fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let length = self.length(2)?;
        self.take(length)
    }

    fn text(&mut self) -> Result<String, DecodeError> {
        let length = self.length(3)?;
        let bytes = self.take(length)?;
        core::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| DecodeError::InvalidUtf8)
    }

    fn sync_id(&mut self) -> Result<SyncId, DecodeError> {
        let value = self.text()?;
        if value.len() > MAX_SYNC_ID_BYTES {
            return Err(DecodeError::SyncIdTooLarge);
        }
        Ok(SyncId::from(value))
    }

    fn optional_hash(&mut self, field: &'static str) -> Result<Option<[u8; 32]>, DecodeError> {
        if self.bytes.get(self.position) == Some(&0xF6) {
            self.position += 1;
            return Ok(None);
        }
        let bytes = self.bytes()?;
        if bytes.len() != 32 {
            return Err(DecodeError::InvalidLength {
                field,
                expected: 32,
                actual: bytes.len(),
            });
        }
        Ok(Some(bytes.try_into().expect("length checked")))
    }

    fn required_hash(&mut self, field: &'static str) -> Result<[u8; 32], DecodeError> {
        let bytes = self.bytes()?;
        if bytes.len() != 32 {
            return Err(DecodeError::InvalidLength {
                field,
                expected: 32,
                actual: bytes.len(),
            });
        }
        Ok(bytes.try_into().expect("length checked"))
    }

    fn null(&mut self) -> Result<(), DecodeError> {
        if self.byte()? == 0xF6 {
            Ok(())
        } else {
            Err(DecodeError::WrongType)
        }
    }

    fn stream_id(&mut self) -> Result<StreamId, DecodeError> {
        self.exact_map(3)?;
        self.field(1)?;
        let share = self.text()?;
        self.field(2)?;
        let glade_id = self.text()?;
        self.field(3)?;
        let key = self.bytes()?.to_vec();
        if key.len() > MAX_KEY_BYTES {
            return Err(DecodeError::KeyTooLarge);
        }
        Ok(StreamId {
            share,
            glade_id,
            key,
        })
    }

    fn record_id(&mut self) -> Result<RecordId, DecodeError> {
        self.exact_map(3)?;
        self.field(1)?;
        let stream = self.stream_id()?;
        self.field(2)?;
        let origin = Principal::from(self.text()?);
        self.field(3)?;
        let seq = self.uint()?;
        Ok(RecordId {
            stream,
            origin,
            seq,
        })
    }

    fn slot(&mut self) -> Result<Slot, DecodeError> {
        let fields = self.map()?;
        self.field(1)?;
        match self.uint()? {
            0 => {
                if fields != 2 {
                    return if fields < 2 {
                        Err(DecodeError::MissingField(2))
                    } else {
                        Err(DecodeError::UnknownField(3))
                    };
                }
                self.field(2)?;
                Ok(Slot::Workspace {
                    share: self.text()?,
                })
            }
            1 => {
                if fields != 4 {
                    return if fields < 4 {
                        Err(DecodeError::MissingField(
                            u64::try_from(fields + 1)
                                .map_err(|_| DecodeError::IntegerOutOfRange)?,
                        ))
                    } else {
                        Err(DecodeError::UnknownField(5))
                    };
                }
                self.field(2)?;
                let share = self.text()?;
                self.field(3)?;
                let glade_id = self.text()?;
                self.field(4)?;
                let key = self.bytes()?.to_vec();
                if key.len() > MAX_KEY_BYTES {
                    return Err(DecodeError::KeyTooLarge);
                }
                Ok(Slot::Binding {
                    share,
                    glade_id,
                    key,
                })
            }
            value => Err(DecodeError::UnknownEnum {
                name: "SlotKind",
                value,
            }),
        }
    }

    fn grant_scope(&mut self) -> Result<Option<GrantScope>, DecodeError> {
        if self.bytes.get(self.position) == Some(&0xF6) {
            self.null()?;
            return Ok(None);
        }
        self.exact_map(3)?;
        self.field(1)?;
        let scope = match self.uint()? {
            0 => {
                self.field(2)?;
                let def_ref = DefRevId::from(self.record_id()?);
                self.field(3)?;
                let compute_key = self.bytes()?.to_vec();
                if compute_key.len() > MAX_KEY_BYTES {
                    return Err(DecodeError::KeyTooLarge);
                }
                GrantScope::Execution(ExecutionScope {
                    def_ref,
                    compute_key: compute_key.into(),
                })
            }
            1 => {
                self.field(2)?;
                let slot = self.slot()?;
                self.field(3)?;
                let supersedes = ClaimId::from(self.record_id()?);
                GrantScope::Takeover { slot, supersedes }
            }
            value => {
                return Err(DecodeError::UnknownEnum {
                    name: "GrantScopeKind",
                    value,
                });
            }
        };
        Ok(Some(scope))
    }

    fn capability_verb(&mut self) -> Result<CapabilityVerb, DecodeError> {
        match self.uint()? {
            0 => Ok(CapabilityVerb::Serve),
            1 => Ok(CapabilityVerb::Execute),
            2 => Ok(CapabilityVerb::Takeover),
            value => Err(DecodeError::UnknownEnum {
                name: "CapabilityVerb",
                value,
            }),
        }
    }

    fn serve_claim(&mut self) -> Result<ServeClaim, DecodeError> {
        self.exact_map(6)?;
        self.field(1)?;
        let node = NodeId::from(self.text()?);
        self.field(2)?;
        let share = self.text()?;
        self.field(3)?;
        let claim_id = ClaimId::from(self.record_id()?);
        self.field(4)?;
        let grant_ref = GrantId::from(self.record_id()?);
        self.field(5)?;
        let lease_expiry_ms = self.signed_i64()?;
        self.field(6)?;
        let epoch = self.uint()?;
        Ok(ServeClaim {
            node,
            share,
            claim_id,
            grant_ref,
            lease_expiry_ms,
            epoch,
        })
    }

    fn service_instance_claim(&mut self) -> Result<ServiceInstanceClaim, DecodeError> {
        self.exact_map(10)?;
        self.field(1)?;
        let node = NodeId::from(self.text()?);
        self.field(2)?;
        let share = self.text()?;
        if share != "svc" {
            return Err(DecodeError::InvalidValue("service_instance.share"));
        }
        self.field(3)?;
        let glade_id = self.text()?;
        self.field(4)?;
        let key = self.bytes()?.to_vec();
        if key.len() > MAX_KEY_BYTES {
            return Err(DecodeError::KeyTooLarge);
        }
        self.field(5)?;
        let claim_id = ClaimId::from(self.record_id()?);
        self.field(6)?;
        let def_ref = DefRevId::from(self.record_id()?);
        self.field(7)?;
        let exec_grant_ref = GrantId::from(self.record_id()?);
        self.field(8)?;
        let compute_key = self.bytes()?.to_vec();
        if compute_key.len() > MAX_KEY_BYTES {
            return Err(DecodeError::KeyTooLarge);
        }
        self.field(9)?;
        let lease_expiry_ms = self.signed_i64()?;
        self.field(10)?;
        let epoch = self.uint()?;
        Ok(ServiceInstanceClaim {
            node,
            share,
            glade_id,
            key,
            claim_id,
            def_ref,
            exec_grant_ref,
            compute_key: compute_key.into(),
            lease_expiry_ms,
            epoch,
        })
    }

    fn capability_grant(&mut self) -> Result<CapabilityGrant, DecodeError> {
        self.exact_map(6)?;
        self.field(1)?;
        let grant_id = GrantId::from(self.record_id()?);
        self.field(2)?;
        let issuer = Principal::from(self.text()?);
        self.field(3)?;
        let principal = Principal::from(self.text()?);
        self.field(4)?;
        let share = self.text()?;
        self.field(5)?;
        let count = self.array()?;
        if count > 3 {
            return Err(DecodeError::TooManyItems {
                field: "verbs",
                max: 3,
                actual: count,
            });
        }
        let mut verbs = Vec::new();
        let mut previous = None;
        for _ in 0..count {
            let verb = self.capability_verb()?;
            if previous.is_some_and(|prior| prior >= verb) {
                return Err(DecodeError::NonCanonical);
            }
            previous = Some(verb);
            verbs.push(verb);
        }
        self.field(6)?;
        let scope = self.grant_scope()?;
        Ok(CapabilityGrant {
            grant_id,
            issuer,
            principal,
            share,
            verbs,
            scope,
        })
    }

    fn capability_revocation(&mut self) -> Result<CapabilityRevocation, DecodeError> {
        self.exact_map(1)?;
        self.field(1)?;
        Ok(CapabilityRevocation {
            revokes: GrantId::from(self.record_id()?),
        })
    }

    fn head(&mut self) -> Result<Head, DecodeError> {
        self.exact_map(3)?;
        self.field(1)?;
        let origin = Principal::from(self.text()?);
        self.field(2)?;
        let seq = self.uint()?;
        self.field(3)?;
        let hash = self.optional_hash("head.hash")?;
        Ok(Head { origin, seq, hash })
    }

    fn stream_head(&mut self) -> Result<StreamHead, DecodeError> {
        self.exact_map(6)?;
        self.field(1)?;
        let share = self.text()?;
        self.field(2)?;
        let glade_id = self.text()?;
        self.field(3)?;
        let key = self.bytes()?.to_vec();
        if key.len() > MAX_KEY_BYTES {
            return Err(DecodeError::KeyTooLarge);
        }
        self.field(4)?;
        let origin = Principal::from(self.text()?);
        self.field(5)?;
        let seq = self.uint()?;
        self.field(6)?;
        let hash = self.required_hash("hash")?;
        Ok(StreamHead {
            stream: StreamId {
                share,
                glade_id,
                key,
            },
            origin,
            seq,
            hash,
        })
    }

    fn shape(&mut self) -> Result<Shape, DecodeError> {
        match self.uint()? {
            0 => Ok(Shape::Value),
            1 => Ok(Shape::Log),
            2 => Ok(Shape::Stream),
            value => Err(DecodeError::UnknownEnum {
                name: "Shape",
                value,
            }),
        }
    }

    fn signed_op(&mut self) -> Result<SignedOp, DecodeError> {
        let start = self.position;
        let fields = self.map()?;
        if fields > 11 {
            // Consume known fields first so the exact unexpected field is
            // reported below instead of trusting only the map length.
        }

        self.field(1)?;
        let share = self.text()?;
        self.field(2)?;
        let glade_id = self.text()?;
        self.field(3)?;
        let key = self.bytes()?.to_vec();
        if key.len() > MAX_KEY_BYTES {
            return Err(DecodeError::KeyTooLarge);
        }
        self.field(4)?;
        let origin = Principal::from(self.text()?);
        self.field(5)?;
        let seq = self.uint()?;
        self.field(6)?;
        let prev = self.optional_hash("prev")?;
        self.field(7)?;
        let lamport = self.uint()?;
        self.field(8)?;
        let refs_count = self.array()?;
        let mut refs = Vec::new();
        for _ in 0..refs_count {
            refs.push(self.head()?);
        }
        self.field(9)?;
        let shape = self.shape()?;
        self.field(10)?;
        let payload = self.bytes()?.to_vec();

        if fields < 11 {
            return Err(DecodeError::MissingField(11));
        }
        self.field(11)?;
        let signature = self.bytes()?.to_vec();
        if fields > 11 {
            let field = self.uint()?;
            return Err(DecodeError::UnknownField(field));
        }

        let canonical_bytes = self.bytes[start..self.position].to_vec();
        if canonical_bytes.len() > MAX_RECORD_BYTES {
            return Err(DecodeError::RecordTooLarge);
        }
        Ok(SignedOp {
            envelope: OpEnvelope {
                stream: StreamId {
                    share,
                    glade_id,
                    key,
                },
                origin,
                seq,
                prev,
                lamport,
                refs,
                shape,
                payload,
            },
            signature,
            canonical_bytes,
        })
    }
}

struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn argument(&mut self, major: u8, value: u64) {
        let prefix = major << 5;
        match value {
            0..=23 => self
                .bytes
                .push(prefix | u8::try_from(value).expect("bounded")),
            24..=0xFF => {
                self.bytes.push(prefix | 24);
                self.bytes.push(u8::try_from(value).expect("bounded"));
            }
            0x100..=0xFFFF => {
                self.bytes.push(prefix | 25);
                self.bytes
                    .extend_from_slice(&u16::try_from(value).expect("bounded").to_be_bytes());
            }
            0x1_0000..=0xFFFF_FFFF => {
                self.bytes.push(prefix | 26);
                self.bytes
                    .extend_from_slice(&u32::try_from(value).expect("bounded").to_be_bytes());
            }
            _ => {
                self.bytes.push(prefix | 27);
                self.bytes.extend_from_slice(&value.to_be_bytes());
            }
        }
    }

    fn uint(&mut self, value: u64) {
        self.argument(0, value);
    }

    fn signed_i64(&mut self, value: i64) {
        if value >= 0 {
            self.uint(u64::try_from(value).expect("non-negative i64 fits u64"));
        } else {
            let magnitude = u64::try_from(-1_i128 - i128::from(value))
                .expect("negative i64 CBOR magnitude fits u64");
            self.argument(1, magnitude);
        }
    }

    fn bytes(&mut self, value: &[u8]) {
        self.argument(2, u64::try_from(value.len()).expect("usize fits u64"));
        self.raw(value);
    }

    fn text(&mut self, value: &str) {
        self.argument(3, u64::try_from(value.len()).expect("usize fits u64"));
        self.raw(value.as_bytes());
    }

    fn array(&mut self, length: usize) {
        self.argument(4, u64::try_from(length).expect("usize fits u64"));
    }

    fn map(&mut self, length: usize) {
        self.argument(5, u64::try_from(length).expect("usize fits u64"));
    }

    fn optional_hash(&mut self, value: Option<&[u8; 32]>) {
        if let Some(hash) = value {
            self.bytes(hash);
        } else {
            self.bytes.push(0xF6);
        }
    }

    fn null(&mut self) {
        self.bytes.push(0xF6);
    }

    fn stream_id(&mut self, stream: &StreamId) {
        self.map(3);
        self.uint(1);
        self.text(&stream.share);
        self.uint(2);
        self.text(&stream.glade_id);
        self.uint(3);
        self.bytes(&stream.key);
    }

    fn record_id(&mut self, record: &RecordId) {
        self.map(3);
        self.uint(1);
        self.stream_id(&record.stream);
        self.uint(2);
        self.text(record.origin.as_str());
        self.uint(3);
        self.uint(record.seq);
    }

    fn slot(&mut self, slot: &Slot) {
        match slot {
            Slot::Workspace { share } => {
                self.map(2);
                self.uint(1);
                self.uint(0);
                self.uint(2);
                self.text(share);
            }
            Slot::Binding {
                share,
                glade_id,
                key,
            } => {
                self.map(4);
                self.uint(1);
                self.uint(1);
                self.uint(2);
                self.text(share);
                self.uint(3);
                self.text(glade_id);
                self.uint(4);
                self.bytes(key);
            }
        }
    }

    fn grant_scope(&mut self, scope: Option<&GrantScope>) {
        match scope {
            None => self.null(),
            Some(GrantScope::Execution(scope)) => {
                self.map(3);
                self.uint(1);
                self.uint(0);
                self.uint(2);
                self.record_id(scope.def_ref.record());
                self.uint(3);
                self.bytes(scope.compute_key.as_bytes());
            }
            Some(GrantScope::Takeover { slot, supersedes }) => {
                self.map(3);
                self.uint(1);
                self.uint(1);
                self.uint(2);
                self.slot(slot);
                self.uint(3);
                self.record_id(supersedes.record());
            }
        }
    }

    fn capability_verb(&mut self, verb: CapabilityVerb) {
        self.uint(match verb {
            CapabilityVerb::Serve => 0,
            CapabilityVerb::Execute => 1,
            CapabilityVerb::Takeover => 2,
        });
    }

    fn serve_claim(&mut self, claim: &ServeClaim) {
        self.map(6);
        self.uint(1);
        self.text(claim.node.as_str());
        self.uint(2);
        self.text(&claim.share);
        self.uint(3);
        self.record_id(claim.claim_id.record());
        self.uint(4);
        self.record_id(claim.grant_ref.record());
        self.uint(5);
        self.signed_i64(claim.lease_expiry_ms);
        self.uint(6);
        self.uint(claim.epoch);
    }

    fn service_instance_claim(&mut self, claim: &ServiceInstanceClaim) {
        self.map(10);
        self.uint(1);
        self.text(claim.node.as_str());
        self.uint(2);
        self.text(&claim.share);
        self.uint(3);
        self.text(&claim.glade_id);
        self.uint(4);
        self.bytes(&claim.key);
        self.uint(5);
        self.record_id(claim.claim_id.record());
        self.uint(6);
        self.record_id(claim.def_ref.record());
        self.uint(7);
        self.record_id(claim.exec_grant_ref.record());
        self.uint(8);
        self.bytes(claim.compute_key.as_bytes());
        self.uint(9);
        self.signed_i64(claim.lease_expiry_ms);
        self.uint(10);
        self.uint(claim.epoch);
    }

    fn capability_grant(&mut self, grant: &CapabilityGrant) -> Result<(), DecodeError> {
        if grant.verbs.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(DecodeError::NonCanonical);
        }
        if grant.verbs.len() > 3 {
            return Err(DecodeError::TooManyItems {
                field: "verbs",
                max: 3,
                actual: grant.verbs.len(),
            });
        }
        self.map(6);
        self.uint(1);
        self.record_id(grant.grant_id.record());
        self.uint(2);
        self.text(grant.issuer.as_str());
        self.uint(3);
        self.text(grant.principal.as_str());
        self.uint(4);
        self.text(&grant.share);
        self.uint(5);
        self.array(grant.verbs.len());
        for verb in &grant.verbs {
            self.capability_verb(*verb);
        }
        self.uint(6);
        self.grant_scope(grant.scope.as_ref());
        Ok(())
    }

    fn capability_revocation(&mut self, revocation: &CapabilityRevocation) {
        self.map(1);
        self.uint(1);
        self.record_id(revocation.revokes.record());
    }

    fn head(&mut self, head: &Head) {
        self.map(3);
        self.uint(1);
        self.text(head.origin.as_str());
        self.uint(2);
        self.uint(head.seq);
        self.uint(3);
        self.optional_hash(head.hash.as_ref());
    }

    fn stream_head(&mut self, head: &StreamHead) {
        self.map(6);
        self.uint(1);
        self.text(&head.stream.share);
        self.uint(2);
        self.text(&head.stream.glade_id);
        self.uint(3);
        self.bytes(&head.stream.key);
        self.uint(4);
        self.text(head.origin.as_str());
        self.uint(5);
        self.uint(head.seq);
        self.uint(6);
        self.bytes(&head.hash);
    }
}

fn encode_unsigned_envelope(envelope: &OpEnvelope) -> Vec<u8> {
    let mut encoder = Encoder::new();
    encoder.map(10);
    encoder.uint(1);
    encoder.text(&envelope.stream.share);
    encoder.uint(2);
    encoder.text(&envelope.stream.glade_id);
    encoder.uint(3);
    encoder.bytes(&envelope.stream.key);
    encoder.uint(4);
    encoder.text(envelope.origin.as_str());
    encoder.uint(5);
    encoder.uint(envelope.seq);
    encoder.uint(6);
    encoder.optional_hash(envelope.prev.as_ref());
    encoder.uint(7);
    encoder.uint(envelope.lamport);
    encoder.uint(8);
    encoder.array(envelope.refs.len());
    for head in &envelope.refs {
        encoder.head(head);
    }
    encoder.uint(9);
    encoder.uint(match envelope.shape {
        Shape::Value => 0,
        Shape::Log => 1,
        Shape::Stream => 2,
    });
    encoder.uint(10);
    encoder.bytes(&envelope.payload);
    encoder.finish()
}
