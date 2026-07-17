"""Canonical Glade discovery protocol schema (v3.1).

The signed operation deliberately preserves the existing Glade ``Op`` field
numbers 1 through 10.  Field 11 adds the required B5 signature.  Signatures and
chain hashes cover the canonical encoding of fields 1 through 10; field 11 is
carried and forwarded but is not part of that unsigned encoding.

``WireKind`` is the discovery transport discriminator.  The four message bodies
remain separate taut messages, matching the Rust ``WireMsg`` enum.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "taut" / "src"))

from taut.ir.dsl import BYTES, INT, STR, Enum, F, List, Msg, Ref, schema


SCHEMA = schema(
    Enum("Shape", value=0, log=1, stream=2),
    Enum("WireKind", dir_op=0, sync_start=1, sync_ops=2, sync_end=3),
    Enum(
        "DirectoryRecordKind",
        serve_claim=0,
        service_instance_claim=1,
        capability_grant=2,
        capability_revocation=3,
    ),
    Enum("CapabilityVerb", serve=0, execute=1, takeover=2),

    # Existing Glade causal head used by Op.refs.  It is scoped by the
    # containing operation and therefore does not repeat the stream axis.
    Msg(
        "Head",
        F("origin", 1, STR),
        F("seq", 2, INT),
        F("hash", 3, BYTES, optional=True),
    ),

    # Flattened per-origin stream head used by discovery anti-entropy.
    Msg(
        "StreamHead",
        F("share", 1, STR),
        F("glade_id", 2, STR),
        F("key", 3, BYTES),
        F("origin", 4, STR),
        F("seq", 5, INT),
        F("hash", 6, BYTES),
    ),

    Msg(
        "SignedOp",
        F("share", 1, STR),
        F("glade_id", 2, STR),
        F("key", 3, BYTES),
        F("origin", 4, STR),
        F("seq", 5, INT),
        F("prev", 6, BYTES, optional=True),
        F("lamport", 7, INT),
        F("refs", 8, List(Ref("Head"))),
        F("shape", 9, Ref("Shape")),
        F("payload", 10, BYTES),
        F("sig", 11, BYTES),
    ),

    # ---- versioned directory payload -------------------------------------
    Msg(
        "RecordStreamId",
        F("share", 1, STR),
        F("glade_id", 2, STR),
        F("key", 3, BYTES),
    ),
    Msg(
        "RecordId",
        F("stream", 1, Ref("RecordStreamId")),
        F("origin", 2, STR),
        F("seq", 3, INT),
    ),
    Msg(
        "WorkspaceSlot",
        F("kind", 1, INT),
        F("share", 2, STR),
    ),
    Msg(
        "BindingSlot",
        F("kind", 1, INT),
        F("share", 2, STR),
        F("glade_id", 3, STR),
        F("key", 4, BYTES),
    ),
    Msg(
        "ExecutionScope",
        F("kind", 1, INT),
        F("def_ref", 2, Ref("RecordId")),
        F("compute_key", 3, BYTES),
    ),
    # ``slot`` is represented as a canonical nested map selected by its own
    # kind. The Rust fail-closed codec enforces WorkspaceSlot|BindingSlot.
    Msg(
        "TakeoverWorkspaceScope",
        F("kind", 1, INT),
        F("slot", 2, Ref("WorkspaceSlot")),
        F("supersedes", 3, Ref("RecordId")),
    ),
    Msg(
        "TakeoverBindingScope",
        F("kind", 1, INT),
        F("slot", 2, Ref("BindingSlot")),
        F("supersedes", 3, Ref("RecordId")),
    ),
    Msg(
        "ServeClaimBody",
        F("node", 1, STR),
        F("share", 2, STR),
        F("claim_id", 3, Ref("RecordId")),
        F("grant_ref", 4, Ref("RecordId")),
        F("lease_expiry_ms", 5, INT),
        F("epoch", 6, INT),
    ),
    Msg(
        "ServiceInstanceClaimBody",
        F("node", 1, STR),
        F("share", 2, STR),
        F("glade_id", 3, STR),
        F("key", 4, BYTES),
        F("claim_id", 5, Ref("RecordId")),
        F("def_ref", 6, Ref("RecordId")),
        F("exec_grant_ref", 7, Ref("RecordId")),
        F("compute_key", 8, BYTES),
        F("lease_expiry_ms", 9, INT),
        F("epoch", 10, INT),
    ),
    # Scope is a canonical nested map or null. Taut currently has no tagged
    # union primitive, so the legal members are declared as concrete body
    # variants; the executable Rust codec owns their single-union dispatch.
    Msg(
        "CapabilityGrantBodyNoScope",
        F("grant_id", 1, Ref("RecordId")),
        F("issuer", 2, STR),
        F("principal", 3, STR),
        F("share", 4, STR),
        F("verbs", 5, List(Ref("CapabilityVerb"))),
        # This slot MUST be absent/null in the no-scope corpus.
        F("scope", 6, Ref("ExecutionScope"), optional=True),
    ),
    Msg(
        "CapabilityGrantBodyExecutionScope",
        F("grant_id", 1, Ref("RecordId")),
        F("issuer", 2, STR),
        F("principal", 3, STR),
        F("share", 4, STR),
        F("verbs", 5, List(Ref("CapabilityVerb"))),
        F("scope", 6, Ref("ExecutionScope")),
    ),
    Msg(
        "CapabilityGrantBodyTakeoverWorkspaceScope",
        F("grant_id", 1, Ref("RecordId")),
        F("issuer", 2, STR),
        F("principal", 3, STR),
        F("share", 4, STR),
        F("verbs", 5, List(Ref("CapabilityVerb"))),
        F("scope", 6, Ref("TakeoverWorkspaceScope")),
    ),
    Msg(
        "CapabilityGrantBodyTakeoverBindingScope",
        F("grant_id", 1, Ref("RecordId")),
        F("issuer", 2, STR),
        F("principal", 3, STR),
        F("share", 4, STR),
        F("verbs", 5, List(Ref("CapabilityVerb"))),
        F("scope", 6, Ref("TakeoverBindingScope")),
    ),
    Msg(
        "CapabilityRevocationBody",
        F("revokes", 1, Ref("RecordId")),
    ),
    # Four concrete wrappers describe the same frozen outer map. The kind value
    # is fixed and cross-checked by the Rust union decoder.
    Msg(
        "ServeClaimPayloadV1",
        F("schema_version", 1, INT),
        F("record_kind", 2, INT),
        F("body", 3, Ref("ServeClaimBody")),
    ),
    Msg(
        "ServiceInstanceClaimPayloadV1",
        F("schema_version", 1, INT),
        F("record_kind", 2, INT),
        F("body", 3, Ref("ServiceInstanceClaimBody")),
    ),
    Msg(
        "CapabilityGrantPayloadV1NoScope",
        F("schema_version", 1, INT),
        F("record_kind", 2, INT),
        F("body", 3, Ref("CapabilityGrantBodyNoScope")),
    ),
    Msg(
        "CapabilityGrantPayloadV1ExecutionScope",
        F("schema_version", 1, INT),
        F("record_kind", 2, INT),
        F("body", 3, Ref("CapabilityGrantBodyExecutionScope")),
    ),
    Msg(
        "CapabilityGrantPayloadV1TakeoverWorkspaceScope",
        F("schema_version", 1, INT),
        F("record_kind", 2, INT),
        F("body", 3, Ref("CapabilityGrantBodyTakeoverWorkspaceScope")),
    ),
    Msg(
        "CapabilityGrantPayloadV1TakeoverBindingScope",
        F("schema_version", 1, INT),
        F("record_kind", 2, INT),
        F("body", 3, Ref("CapabilityGrantBodyTakeoverBindingScope")),
    ),
    Msg(
        "CapabilityRevocationPayloadV1",
        F("schema_version", 1, INT),
        F("record_kind", 2, INT),
        F("body", 3, Ref("CapabilityRevocationBody")),
    ),

    Msg("DirOp", F("op", 1, Ref("SignedOp"))),
    Msg(
        "SyncStart",
        F("sync_id", 1, STR),
        F("heads", 2, List(Ref("StreamHead"))),
    ),
    Msg(
        "SyncOps",
        F("sync_id", 1, STR),
        F("ops", 2, List(Ref("SignedOp"))),
    ),
    Msg("SyncEnd", F("sync_id", 1, STR)),
)
