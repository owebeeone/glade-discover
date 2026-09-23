//! Consumer tests for the first-slice profile, glade-wz
//! `dev-docs/glade/GladeFirstSliceProfile.md` (plan Step 2.10).
//!
//! Each test is named for the profile statement (`SP-…`) it pins and runs that
//! statement's helper on contract-faithful deterministic providers: no clock
//! read, network, executor or node. Each `*_rejects_*` test runs the same
//! helper on a deliberately wrong fixture and must panic with the statement's
//! ID. The append host is NON-CRYPTOGRAPHIC (its signature is the signed
//! bytes) and volatile: nothing here is evidence of cryptography or durability.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use glade_discover_core::ingest::{self, StructuralVerdict};
use glade_discover_core::{
    ClaimCommand, ClaimMode, ClockState, Effect, Event, KernelConfig, MonoInstant,
    NodePrincipalBinding, PersistedState, PrincipalCtx, PrincipalPlane, RouteAns, State, StepCtx,
    VerificationResult, WallMs, WatermarkLoad, restore_fresh, step,
};
use glade_discover_node_adapter::append::{
    self, Allocation, AppendHost, AppendKey, DurableAccepted,
};
use glade_discover_protocol::{
    CapabilityGrant, CapabilityVerb, ClaimDraft, ClaimId, ClaimIdentity, Corr, DecodeError,
    DirectoryRecord, Generation, GrantId, IngressId, IntentId, NodeId, OpEnvelope, Principal,
    RecordId, RouteQuery, ServeClaim, Shape, SignedOp, Slot, StreamId, WireMsg,
    decode_directory_record, decode_signed_op, encode_directory_record, encode_signed_op, op_hash,
    record_id, unsigned_canonical_bytes,
};

const SHARE: &str = "workspace";
const OWNER: &str = "owner";
const NODE_A: &str = "node-a";
const PRINCIPAL_A: &str = "principal-a";
const NODE_B: &str = "node-b";
const LEASE: i64 = 5_000;

// ---- fixtures: node-a, its owner grant, its clock -----------------------------

fn stream(share: &str, glade_id: &str, key: &[u8]) -> StreamId {
    StreamId {
        share: share.into(),
        glade_id: glade_id.into(),
        key: key.to_vec(),
    }
}

fn envelope(at: StreamId, origin: &str, seq: u64, shape: Shape, payload: Vec<u8>) -> OpEnvelope {
    OpEnvelope {
        stream: at,
        origin: Principal::from(origin),
        seq,
        prev: None,
        lamport: seq,
        refs: Vec::new(),
        shape,
        payload,
    }
}

/// The first op of `origin`'s chain on `at`, echo-signed like the host below.
fn first_op(at: StreamId, origin: &str, payload: Vec<u8>) -> SignedOp {
    let envelope = envelope(at, origin, 0, Shape::Log, payload);
    let unsigned = unsigned_canonical_bytes(&encode_signed_op(&envelope, &[]).expect("envelope"));
    encode_signed_op(&envelope, &unsigned).expect("signed op")
}

/// The owner's serve grant to principal-a.
fn grant() -> (GrantId, SignedOp) {
    let at = stream(SHARE, "grants", PRINCIPAL_A.as_bytes());
    let id = GrantId::from(RecordId {
        stream: at.clone(),
        origin: Principal::from(OWNER),
        seq: 0,
    });
    let record = DirectoryRecord::CapabilityGrant(CapabilityGrant {
        grant_id: id.clone(),
        issuer: Principal::from(OWNER),
        principal: Principal::from(PRINCIPAL_A),
        share: SHARE.into(),
        verbs: vec![CapabilityVerb::Serve],
        scope: None,
    });
    let payload = encode_directory_record(&record).expect("grant");
    (id, first_op(at, OWNER, payload))
}

fn at(mono: u64, wall: i64) -> StepCtx {
    StepCtx {
        mono: MonoInstant(mono),
        wall: WallMs(wall),
    }
}

/// node-a under the slice's fixed development trust configuration.
fn config() -> KernelConfig {
    let bound = NodePrincipalBinding {
        node: NodeId::from(NODE_A),
        principal: Principal::from(PRINCIPAL_A),
        plane: PrincipalPlane::Workspace,
    };
    KernelConfig {
        local_node: NodeId::from(NODE_A),
        local_principal: Principal::from(PRINCIPAL_A),
        peers: BTreeSet::from([NodeId::from(NODE_B)]),
        workspace_owner_roots: BTreeMap::from([(SHARE.to_owned(), Principal::from(OWNER))]),
        node_principal_bindings: BTreeSet::from([bound]),
        skew_margin_ms: 5,
        max_lease_ms: 10_000,
        clock_resync_ms: 30,
        sync_retries: 3,
        sync_timeout_ms: NonZeroU64::new(1_000).expect("non-zero"),
        gossip_fan: 1,
        max_retained_bytes: NonZeroU64::new(1_000_000).expect("non-zero"),
    }
}

/// node-a's first boot: its owner grant folded, the watermark read at `restore`.
fn node_a(restore: StepCtx) -> State {
    let grant = grant().1;
    let chain = (
        grant.envelope().stream.clone(),
        BTreeMap::from([(record_id(&grant), grant.clone())]),
    );
    let persisted = PersistedState {
        retained: BTreeMap::from([chain]),
        retained_bytes: u64::try_from(grant.canonical_bytes().len()).expect("small"),
        ..PersistedState::default()
    };
    restore_fresh(
        config(),
        persisted,
        WatermarkLoad::Readable(restore.wall),
        restore,
    )
}

fn slot() -> Slot {
    Slot::Workspace {
        share: SHARE.into(),
    }
}

fn advertise() -> Event {
    let draft = ClaimDraft::Workspace {
        node: NodeId::from(NODE_A),
        share: SHARE.into(),
        identity: ClaimIdentity::Mint,
        grant_ref: grant().0,
        lease_expiry_ms: LEASE,
        epoch: 0,
    };
    let command = ClaimCommand {
        intent: IntentId::from("intent-1"),
        slot: slot(),
        generation: Generation(1),
        mode: ClaimMode::Initial,
        draft,
    };
    Event::Advertise {
        command: Box::new(command),
    }
}

/// One route step at `now`: the state a driver would commit, and the answer.
fn routed(state: State, now: StepCtx) -> (State, RouteAns) {
    let event = Event::Route {
        ingress: IngressId::from("in"),
        principal: PrincipalCtx {
            principal: Principal::from("reader"),
            authenticated_context: Vec::new(),
        },
        corr: Corr::from("c"),
        query: RouteQuery { slot: slot() },
    };
    let (state, effects) = step(state, now, event);
    let [Effect::Reply { ans, .. }] = effects.as_slice() else {
        panic!("one reply per route: {effects:?}");
    };
    let ans = ans.clone();
    (state, ans)
}

fn matched() -> RouteAns {
    RouteAns::Matched {
        node: NodeId::from(NODE_A),
    }
}

/// NON-CRYPTOGRAPHIC append host: its signature echoes the signed bytes.
/// `volatile` acknowledges without persisting (a wrong fixture for SP-L1).
#[derive(Default)]
struct Host {
    volatile: bool,
    signed: Vec<Vec<u8>>,
    persisted: Option<Vec<u8>>,
}

impl AppendHost for Host {
    type Error = &'static str;

    fn accepted(&mut self, _key: &AppendKey) -> Result<Option<DurableAccepted>, Self::Error> {
        Ok(None)
    }

    fn allocate(&mut self, _draft: &ClaimDraft) -> Result<Allocation, Self::Error> {
        Ok(Allocation {
            stream: stream(SHARE, "claims", b""),
            seq: 0,
            prev: None,
            lamport: 0,
            shape: Shape::Log,
        })
    }

    fn signer(&self) -> Principal {
        Principal::from(PRINCIPAL_A)
    }

    fn sign(&mut self, unsigned: &[u8]) -> Result<Vec<u8>, Self::Error> {
        self.signed.push(unsigned.to_vec());
        Ok(unsigned.to_vec())
    }

    fn verify(&mut self, op: &SignedOp) -> Result<bool, Self::Error> {
        Ok(op.envelope().origin == self.signer() && op.signature() == unsigned_canonical_bytes(op))
    }

    fn persist_accepted(
        &mut self,
        _: &AppendKey,
        _: &ClaimDraft,
        op: &[u8],
    ) -> Result<(), Self::Error> {
        self.persisted = (!self.volatile).then(|| op.to_vec());
        Ok(())
    }
}

/// Advertise at `now`; the adapter signs and persists; the kernel folds its
/// `OpAccepted`. Returns the state and that last step's effects.
fn register(state: State, now: StepCtx, host: &mut Host) -> (State, Vec<Effect>) {
    let (state, effects) = step(state, now, advertise());
    let [
        Effect::Append {
            intent,
            slot,
            generation,
            draft,
        },
    ] = effects.as_slice()
    else {
        return (state, effects);
    };
    let accepted = append::handle_append(host, slot, *generation, intent, draft).expect("append");
    step(state, now, accepted)
}

fn registration() -> Host {
    let mut host = Host::default();
    register(node_a(at(0, 1_000)), at(0, 1_000), &mut host);
    host
}

// ---- Glade's taut CBOR (glade-wire `cbor.rs`), for bytes discovery cannot make --

fn head(major: u8, len: usize) -> Vec<u8> {
    let [high, low] = u16::try_from(len).expect("small fixture").to_be_bytes();
    match len {
        0..=23 => vec![(major << 5) | low],
        24..=0xff => vec![(major << 5) | 24, low],
        _ => vec![(major << 5) | 25, high, low],
    }
}

fn text(value: &str) -> Vec<u8> {
    [head(3, value.len()), value.as_bytes().to_vec()].concat()
}

fn bytes(value: &[u8]) -> Vec<u8> {
    [head(2, value.len()), value.to_vec()].concat()
}

fn array(items: &[Vec<u8>]) -> Vec<u8> {
    [head(4, items.len()), items.concat()].concat()
}

/// A map keyed 1, 2, 3, … in order, as every record below is.
fn map(fields: &[Vec<u8>]) -> Vec<u8> {
    let entries = fields.iter().enumerate();
    let entries: Vec<Vec<u8>> = entries
        .map(|(at, field)| [head(0, at + 1), field.clone()].concat())
        .collect();
    [head(5, fields.len()), entries.concat()].concat()
}

/// The home-share kinds as `glade/node/src/sysdata.rs` (glade 831eded) encodes them.
fn glade_home_share_kinds() -> [(&'static str, Vec<u8>); 9] {
    let (node, share, principal) = (text(NODE_A), text(SHARE), text(PRINCIPAL_A));
    let (app, glade_id) = (text("grazel"), text("ws.diff"));
    let tail = ["log", "share", "private", "from_cursor"].map(text);
    let hosts = array(std::slice::from_ref(&node));
    let workspace = map(&[share.clone(), text("name"), hosts]);
    let serve = map(&[node.clone(), share.clone(), head(0, 5_000), head(0, 0)]);
    let grant = map(&[principal.clone(), share.clone(), array(&[text("serve")])]);
    let binding = map(&[&[app.clone(), glade_id.clone()][..], &tail].concat());
    [
        ("NodeRecord", map(&[node, text("operator")])),
        ("WorkspaceEntry", workspace),
        ("ServeClaim", serve),
        ("CapabilityGrant", grant),
        ("CapabilityRevocation", map(&[principal.clone(), share])),
        ("BindingDecl", binding),
        ("BindingRetraction", map(&[app.clone(), glade_id.clone()])),
        ("ServiceDefinition", map(&[app, text("git"), glade_id])),
        ("PrincipalRecord", map(&[principal])),
    ]
}

/// taut vector `chain/a0` as a glade-wire `Op` (fields 1–10) with the given
/// shape, plus a field 11 when one is given.
fn glade_wire_op(shape: usize, field_11: Option<&[u8]>) -> Vec<u8> {
    let (share, glade_id, key, origin) = (text("sh"), text("g"), bytes(b""), text("a"));
    let (seq, prev, lamport, refs) = (head(0, 0), vec![0xf6], head(0, 0), array(&[]));
    let mut fields = vec![share, glade_id, key, origin, seq, prev, lamport, refs];
    fields.extend([head(0, shape), bytes(b"p0")]);
    fields.extend(field_11.map(bytes));
    map(&fields)
}

// ---- SP-R3 + SP-P2: what a registration writes, and what its signer signs ----

fn assert_sp_r3_p2(signed: &[u8], persisted: &[u8]) {
    let op = decode_signed_op(persisted).expect("SP-R3: a canonical signed op");
    let claim = ServeClaim {
        node: NodeId::from(NODE_A),
        share: SHARE.into(),
        claim_id: ClaimId::from(record_id(&op)),
        grant_ref: grant().0,
        lease_expiry_ms: LEASE,
        epoch: 0,
    };
    let decoded = decode_directory_record(&op.envelope().payload);
    let expected = Ok(DirectoryRecord::ServeClaim(claim));
    assert_eq!(
        decoded, expected,
        "SP-R3: a v1 ServeClaim named by its record id"
    );
    let unsigned = unsigned_canonical_bytes(&op);
    assert_eq!(
        signed, unsigned,
        "SP-P2: the signer gets the unsigned bytes, not a digest"
    );
    let parts: [&[u8]; 4] = [&[0xab], &unsigned[1..], &[0x0b], &bytes(op.signature())];
    assert_eq!(
        persisted,
        parts.concat(),
        "SP-P2: the signed op is fields 1-10 plus field 11"
    );
}

#[test]
fn sp_r3_p2_registration_signs_the_unsigned_bytes_of_a_v1_serve_claim() {
    let host = registration();
    let [signed] = host.signed.as_slice() else {
        panic!("one signature per registration");
    };
    assert_sp_r3_p2(signed, host.persisted.as_deref().expect("persisted"));
}

#[test]
#[should_panic(expected = "SP-R3: a v1 ServeClaim named by its record id")]
fn sp_r3_rejects_a_glade_shaped_serve_claim() {
    let glade_claim = glade_home_share_kinds()[2].1.clone();
    let glade = first_op(stream(SHARE, "claims", b""), PRINCIPAL_A, glade_claim);
    assert_sp_r3_p2(&unsigned_canonical_bytes(&glade), glade.canonical_bytes());
}

#[test]
#[should_panic(expected = "SP-P2: the signer gets the unsigned bytes, not a digest")]
fn sp_p2_rejects_a_signer_handed_the_op_hash() {
    let persisted = registration().persisted.expect("persisted");
    let digest = op_hash(&decode_signed_op(&persisted).expect("canonical"));
    assert_sp_r3_p2(&digest, &persisted);
}

// ---- SP-P2: the unsigned bytes are Glade's own taut Op bytes -------------------

// taut `corpus/glade_hashes.json` (taut d98ff68), Glade's op-hash oracle: share
// "sh", glade id "g", empty key, origin "a", shape "value", lamport = seq.
const A0: &str = "8a87b62f11deec6937b784dd4bada44d6473aa304c9cfd9c8974b139539e6873";
const A1: &str = "aee9d38f848a822fc9dec97e549645cd27400316f277468ad9e19040f9ea26a9";
const A2: &str = "ee0b8923ff7281a153e805df961ca05e58ff5bc87ac19fdf2fd96ad0aceff94b";
const FORK: &str = "c2862c82d40044a4166ac6c19c50425744c73a1825a7db784ae3392aac4f3134";
/// (vector, seq, prev, payload, op_hash)
type TautVector = (
    &'static str,
    u64,
    Option<&'static str>,
    &'static [u8],
    &'static str,
);
const TAUT_OP_HASHES: [TautVector; 4] = [
    ("chain/a0", 0, None, b"p0", A0),
    ("chain/a1", 1, Some(A0), b"p1", A1),
    ("chain/a2", 2, Some(A1), b"p2", A2),
    ("fork/a0", 0, None, b"p0-fork", FORK),
];

fn unhex(digits: &str) -> Vec<u8> {
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).expect("hex");
    (0..digits.len()).step_by(2).map(byte).collect()
}

fn assert_sp_p2_taut_bytes(value: Shape) {
    for (vector, seq, prev, payload, hash) in TAUT_OP_HASHES {
        let mut taut_op = envelope(stream("sh", "g", b""), "a", seq, value, payload.to_vec());
        taut_op.prev = prev.map(|prev| unhex(prev).try_into().expect("32 bytes"));
        let op = encode_signed_op(&taut_op, &[]).expect("representable vector");
        let hashed = op_hash(&op).map(|byte| format!("{byte:02x}")).concat();
        assert_eq!(
            hashed, hash,
            "SP-P2 {vector}: the unsigned bytes are taut's"
        );
    }
}

#[test]
fn sp_p2_unsigned_bytes_are_taut_op_fields_1_to_10() {
    assert_sp_p2_taut_bytes(Shape::Value);
}

#[test]
#[should_panic(expected = "SP-P2 chain/a0: the unsigned bytes are taut's")]
fn sp_p2_rejects_a_wrong_shape_mapping() {
    assert_sp_p2_taut_bytes(Shape::Log);
}

// ---- SP-P3: Glade ops outside the discovery envelope are explicit blockers -----

fn assert_sp_p3(cases: &[(&str, Vec<u8>, Result<(), DecodeError>)]) {
    for (case, encoded, expected) in cases {
        let decoded = decode_signed_op(encoded).map(|_| ());
        assert_eq!(&decoded, expected, "SP-P3 {case}");
    }
}

fn unknown_shape(value: u64) -> Result<(), DecodeError> {
    let name = "Shape";
    Err(DecodeError::UnknownEnum { name, value })
}

#[test]
fn sp_p3_glade_ops_outside_the_envelope_are_explicit_blockers() {
    let (sig, no_field_11): (&[u8], _) = (&[7; 64], Err(DecodeError::MissingField(11)));
    assert_sp_p3(&[
        ("no field 11", glade_wire_op(0, None), no_field_11),
        ("Swmr", glade_wire_op(3, Some(sig)), unknown_shape(3)),
        ("Crdt", glade_wire_op(4, Some(sig)), unknown_shape(4)),
        ("64-byte signature", glade_wire_op(0, Some(sig)), Ok(())),
    ]);
    let op = decode_signed_op(&glade_wire_op(0, Some(sig))).expect("representable");
    let unsigned = unsigned_canonical_bytes(&op);
    assert_eq!(
        unsigned,
        glade_wire_op(0, None),
        "SP-P3: glade-wire bytes are the unsigned"
    );
}

#[test]
#[should_panic(expected = "SP-P3 Swmr fits")]
fn sp_p3_rejects_a_fixture_that_calls_swmr_representable() {
    assert_sp_p3(&[("Swmr fits", glade_wire_op(3, Some(&[7; 64])), Ok(()))]);
}

// ---- SP-R4: the node's home-share kinds are not discovery records --------------

fn assert_sp_r4(kind: &str, payload: Vec<u8>) {
    let decodes = decode_directory_record(&payload).is_ok();
    assert!(!decodes, "SP-R4 {kind}: not a discovery directory record");
    let pushed = first_op(stream(SHARE, "claims", b""), PRINCIPAL_A, payload);
    let signer = Principal::from(PRINCIPAL_A);
    let (verified, fresh) = (
        VerificationResult::Valid { signer },
        PersistedState::default(),
    );
    let ready = ClockState::Ready {
        watermark: WallMs(1_000),
    };
    let verdict = ingest::ingest(&pushed, &verified, &fresh, &config(), ready).structural;
    assert_eq!(
        verdict,
        StructuralVerdict::Malformed,
        "SP-R4 {kind}: quarantined"
    );
}

#[test]
fn sp_r4_glade_home_share_kinds_are_not_discovery_records() {
    for (kind, payload) in glade_home_share_kinds() {
        assert_sp_r4(kind, payload);
    }
}

#[test]
#[should_panic(expected = "SP-R4 CapabilityGrant: not a discovery directory record")]
fn sp_r4_rejects_a_discovery_grant_offered_as_a_glade_kind() {
    assert_sp_r4("CapabilityGrant", grant().1.envelope().payload.clone());
}

// ---- SP-C1 + SP-C2: one clock, and the watermark committed with each step ------

/// What a host commits with each state delta (the driver's `durable_watermark`).
fn committed(state: &State) -> WatermarkLoad {
    match state.clock() {
        ClockState::Ready { watermark } => WatermarkLoad::Readable(watermark),
        ClockState::Uncertain { floor } => {
            floor.map_or(WatermarkLoad::Unreadable, WatermarkLoad::Readable)
        }
    }
}

/// node-a restores at `restore`, registers at 1 000 and is routed at 3 000 on
/// its one clock; the process restarts at 2 000 from what `commit` recorded.
fn assert_sp_c1_c2(restore: StepCtx, commit: fn(&State) -> WatermarkLoad) {
    let (state, _) = register(node_a(restore), at(0, 1_000), &mut Host::default());
    let (state, live) = routed(state, at(2_000, 3_000));
    assert_eq!(
        live,
        matched(),
        "SP-C1: registered and routed on the one clock"
    );
    let restart = at(0, 2_000);
    let restarted = restore_fresh(config(), state.persisted().clone(), commit(&state), restart);
    let (_, behind) = routed(restarted.clone(), restart);
    assert_eq!(
        behind,
        RouteAns::NoClaim,
        "SP-C2: a wall behind the watermark fails closed"
    );
    let floor = ClockState::Uncertain {
        floor: Some(WallMs(3_000)),
    };
    assert_eq!(
        restarted.clock(),
        floor,
        "SP-C2: only the clock tells this from absence"
    );
    let retained = &restarted.persisted().retained;
    assert!(
        retained.contains_key(&stream(SHARE, "claims", b"")),
        "SP-C2: the claim is kept"
    );
    let (_, resynced) = routed(restarted, at(1, 3_030));
    assert_eq!(
        resynced,
        matched(),
        "SP-C2: routed again at floor + CLOCK_RESYNC_MS"
    );
}

#[test]
fn sp_c1_c2_one_clock_and_the_watermark_committed_with_each_step() {
    assert_sp_c1_c2(at(0, 1_000), committed);
}

#[test]
#[should_panic(expected = "SP-C1: registered and routed on the one clock")]
fn sp_c1_rejects_a_second_clock_at_restore() {
    assert_sp_c1_c2(at(0, 11_000), committed);
}

#[test]
#[should_panic(expected = "SP-C2: a wall behind the watermark fails closed")]
fn sp_c2_rejects_a_store_that_keeps_its_first_watermark() {
    assert_sp_c1_c2(at(0, 1_000), |_| WatermarkLoad::Readable(WallMs(1_000)));
}

// ---- SP-L1: the sync effect follows durable local acceptance -------------------

fn assert_sp_l1(host: &mut Host) {
    let (_, pushed) = register(node_a(at(0, 1_000)), at(0, 1_000), host);
    let [Effect::Gossip { to, msg }] = pushed.as_slice() else {
        panic!("SP-L1: one push, only after acceptance: {pushed:?}");
    };
    let WireMsg::DirOp { op } = msg.as_ref() else {
        panic!("SP-L1: the push is the accepted op");
    };
    let (sent, durable) = (
        (to.as_str(), Some(op.canonical_bytes())),
        (NODE_B, host.persisted.as_deref()),
    );
    assert_eq!(
        sent, durable,
        "SP-L1: the push carries the bytes persisted first"
    );
}

#[test]
fn sp_l1_sync_effect_follows_durable_local_acceptance() {
    assert_sp_l1(&mut Host::default());
}

#[test]
#[should_panic(expected = "SP-L1: the push carries the bytes persisted first")]
fn sp_l1_rejects_a_host_that_accepts_without_persisting() {
    let mut volatile = Host {
        volatile: true,
        ..Host::default()
    };
    assert_sp_l1(&mut volatile);
}
