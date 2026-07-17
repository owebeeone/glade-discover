<!--
DO NOT EDIT — vendored from glade-wz working tree.
Source: `dev-docs/glade/GladeDiscoveryDesign.md`
Workspace base revision: `803f3656736900633b7106bf8e8c40d4d6bd9283`
Source status at vendoring: untracked; the content hash below is authoritative.
Body SHA-256: `d73b765c76e4c3bebb98bd7bce1f6fc2e7f99d2f9215c3bcd13ed683cab864af`
Semantic changes MUST be made in the parent workspace and explicitly re-vendored.
-->
<!-- BEGIN VENDORED BODY -->
# Glade Discovery Design — `glade-discover` (v3.1, semantic freeze)

Status: design v3.1 (2026-07-17) — the **semantic-freeze**. Closes
GladeDiscoveryDesign-Review56-2 (GD56R2-01..12) with Gianni's amendments.
The v3.1 amendment closes the implementation-discovered P1-B01..B10 contract
blockers without weakening INV-D0..D6.
Principle (Gianni): *the simulator proves an implementation only against the
semantics its oracle encodes — so v3 must DECIDE those semantics, freeze the
public types / state transitions / invariants / failure outcomes, and only then
are the `s-disc-*` scenarios written as RED tests before code.* The next review
evaluates **code + scenarios**, not prose. Companion: `GladeDiscoveryModel.md`.
Grounded in current `sysdata/claims/store/peer/registry.rs`; depends on B2/B3/B5,
D3/INV-7 (`GladeAuthzModel.md` §11), D5 (glade-diff sandbox), WD-8.

**Acceptance bar (Gianni):** not "closed" until R2-09 is assigned (§7), the
takeover tie-break is stable across renewal (§6), restart-safe clock behaviour is
specified (§2), and the `s-disc-*` data exists in RED form.

---

## 1. FROZEN — the kernel API

The kernel is pure; ALL I/O, crypto, configuration, and clock reads are explicit
state, effects, or inputs.

```
step(state, ctx, event) -> (state', effects[])

ctx    = { mono: MonoInstant, wall: WallMs }                 // §2, sampled together per step
event  =                                                     // the ONLY inputs
   | Deliver   { from: NodeId, msg: WireMsg, verification: VerificationBatch }
   | Route     { ingress: IngressId, principal: PrincipalCtx, corr, query }  // R2-10, R2-03: authenticated
   | Advertise { command: ClaimCommand }                     // initial/renew/takeover local command
   | OpAccepted{ intent: IntentId, op: SignedOp, verification: VerificationResult }
   | Wakeup    { token: WakeToken }                          // R2-10 tokened
   | ClockReseed { watermark: WallMs }                       // trusted recovery from unknown watermark
effect =                                                     // the ONLY outputs
   | Gossip    { to: NodeId, msg: WireMsg }                  // addressed; env delivers-or-not
   | Append    { intent: IntentId, slot, generation, draft: ClaimDraft }
   | Reply     { ingress: IngressId, corr, ans: RouteAns }   // R2-10: direct reply to the ingress
   | Schedule  { token: WakeToken, at_mono: MonoInstant }    // §2
   | Teardown  { slot, generation }                          // losing derived instance, §6
```

`KernelConfig` is immutable deterministic state and carries `local_node`,
`local_principal`, the bounded peer set, workspace-owner roots, explicit
`NodePrincipalBinding{node,principal,plane=Workspace|Derived}` entries,
`SKEW_MARGIN`, `MAX_LEASE`,
`CLOCK_RESYNC_MS`, sync/retry constants, and `MAX_RETAINED_BYTES`. `Slot` and
`RouteQuery` are the canonical binding axis
`{share, glade_id: Option<GladeId>, key: Option<CanonicalKey>}`; `key` MUST be
absent when `glade_id` is absent. A workspace slot has neither optional field.

```
ClaimCommand = { intent, slot, generation, mode, draft }
mode = Initial | Renew | Takeover{authority_ref: GrantId}
ClaimIdentity = Mint | Existing(ClaimId)
ClaimDraft = Workspace{node,share,identity,grant_ref,lease_expiry_ms,epoch}
           | Service{node,share=svc,glade_id,key,identity,def_ref,exec_grant_ref,
                     compute_key,lease_expiry_ms,epoch}
```

`Initial` and `Takeover` require `identity=Mint`; `Renew` requires `Existing`.
The command's slot MUST exactly match the draft. The core validates command
authority/configuration before emitting `Append`; the adapter only finalizes a
`Mint` identity and performs signing/persistence.

A takeover authority is a live `CapabilityGrant` with verb `takeover`,
`principal == command/draft principal`, and exact
`scope = Takeover{slot,supersedes: ClaimId}`. Its issuer MUST be the workspace
owner for a workspace slot or the issuer of the live exact-scope D5 execution
grant for a service slot. The command's epoch MUST equal the superseded winner's
checked `epoch + 1`; its identity is `Mint`. A general or differently scoped
grant authorizes no takeover.

`VerificationBatch` is local non-wire data, in canonical op traversal order
(`DirOp` ordinal 0; `SyncOps` vector order). Each result is
`Valid{signer: Principal}` or `BadSignature`. Its length MUST equal the number
of ops in the message or the event is rejected without folding. `OpAccepted`
has one corresponding result. Hidden verifier callbacks are forbidden.

```
WakeToken = ClaimRenew{slot,generation}
          | AppendRetry{slot,generation,intent}
          | GossipTick{peer}
          | SyncTimeout{peer,sync_id,attempt}
WakeClass = Periodic only for GossipTick; every other variant is OneShot
```

Wake ordering is enum discriminant in the order above, then lexicographic field
order. The class is derived from the variant and is never caller-supplied.

- **R2-01/R2-10:** `principal` is the B3 authenticated context (never a wire
  field); the reply is a direct effect to an opaque `IngressId` (multiple
  sessions per node distinguishable). The kernel API is **INTERNAL** — its
  `RouteAns` is consumed by the trusted node, not returned to a client (§7).
- **R2-03:** the kernel NEVER emits a signed record. To advertise it emits
  `Append` with a claim draft; the node finalizes identity, signs+persists, and
  returns `OpAccepted`; only THEN does the kernel `Gossip` those exact canonical
  bytes (§8). No signing key ever enters kernel state.
- **Durable-commit boundary (P1-B05):** after `step`, the host MUST atomically
  persist the returned durable-state delta before interpreting any effect. If
  persistence fails, it executes no effects. Restoration supplies the persisted
  state and `WatermarkLoad = Readable(WallMs) | Unreadable` to a pure
  `restore(config, persisted, watermark_load, ctx) -> State` constructor.
  `PersistedState` contains `retained`, `retained_bytes`, `time_deferred`,
  `unresolved`, `mine`, and `next_sync`. The forward-only watermark is a separate
  small durable cell but MUST commit atomically with that state delta. `sync`
  round progress is ephemeral and restores empty; pending append retries are
  reconstructed from durable `mine`.
- **INV-D0 (purity):** `step` is a deterministic function of `(state, ctx,
  event)`; identical inputs → identical `(state', effects)` (freezes DR-13).

## 2. FROZEN — two clocks + restart-safe watermark (R2-04)

- `WallMs` (i64 Unix-ms): cross-node absolute, in records only. `MonoInstant`:
  local monotone, scheduling only. Schedule a `WallMs W` as `at_mono =
  mono_now + max(0, W − effwall)` (§below), checked arithmetic; saturating at
  `i64`/instant limits → treat as `clock-uncertain`.
- **Effective wall (INV-D1, no resurrection):** `ClockState = Ready{watermark} |
  Uncertain{floor: Option<WallMs>}`. Ready projection uses
  `effwall = max(watermark, ctx.wall)` and advances the persisted watermark
  forward-only. A readable backward wall restores as `Uncertain{floor=watermark}`
  and recovers only when checked `ctx.wall ≥ floor + CLOCK_RESYNC_MS`. An
  unreadable watermark restores as `Uncertain{floor=None}` and remains
  fail-closed until a trusted `ClockReseed`; there is no comparison against
  missing data. Uncertain state may serve locally-held reads but MUST NOT make
  an expiry-live decision or publish/renew a claim.
- `SKEW_MARGIN`, `MAX_LEASE`, and `CLOCK_RESYNC_MS` are **configured node
  constants** (defaults: `5_000 ms`, `3_600_000 ms`, and `30_000 ms`). A claim is expired at
  projection when `W ≤ effwall + SKEW_MARGIN`; a claim is rejected at ingest when
  `W > effwall + MAX_LEASE + SKEW_MARGIN` (the margin is INCLUDED, per R2-04).
- Time never enters the fold (§3.2). The sim models inter-node wall-offset
  **separately** from timer-delivery delay, and models restart (watermark drop).
- A structurally/governance-valid remote claim received while clock-uncertain is
  retained and byte-accounted but marked `time_deferred` and is never projected.
  On recovery to Ready, the maximum-lease check runs before the record may enter
  live projection; an over-limit record is permanently rejected then. This
  preserves bounded convergence without inventing an `effwall` while uncertain.

## 3. FROZEN — pipeline: ingest → retained fold → project (R2-08)

Three layers with THREE distinct verdicts (R2-08 conflation fixed):

1. **Structural/envelope validity** (B2 fail-closed decode, B5 signature, chain
   `seq/prev`): `Malformed | UnsupportedVersion | BadSignature | BadChain |
   Duplicate | StructurallyValid`. Only `StructurallyValid` proceeds.
2. **Governance authority to PUBLISH this record kind**: may this signer publish
   a claim/grant/revocation at all? (e.g. a revocation signer must be an
   authorized revoker, §4). `PublishUnauthorized` is rejected **permanently**.
3. **Live referenced-capability state** (a claim whose serve-grant is absent or
   revoked): the record is **retained in the time-free set** and its routability
   is decided at PROJECTION, not ingest. A grant arriving later (either order)
   flips it live with no resend.

`Malformed`, `UnsupportedVersion`, `BadSignature`, `BadChain`, and
`PublishUnauthorized` are direct ingest-module verdicts. They are covered by
module/corpus tests, not end-to-end scenario expectations; `step` does not emit
a diagnostic side channel for rejected hostile input (P1-B08).

- **Retained set = the fold:** time-free, live-authz-free set-union of
  structurally-valid, publish-authorized signed records.
- **Pending-proof index (FROZEN state, R2-08):** `unresolved: Map<GrantId,
  [ClaimRef]>` — a durable index of retained claims whose `grant_ref` has not
  folded; re-evaluated on grant/revocation arrival. Survives restart with the
  retained set.
- **`project(effwall, principal_view)`** filters expired (§2) + live-authorized
  (§4), then selects the slot winner by the §6 total order. Re-run per query.

## 4. FROZEN — records + the two authority planes (R2-02, R2-07, R2-08)

`StreamId = {share, glade_id, key}` matches the current per-zone store chain.
Envelope (`GladeRecordEnvelope.md`): stream, origin, seq, prev-hash, lamport,
refs, shape, payload, **B5 sig**. `RecordId = {stream, origin, seq}` is the
immutable canonical record identity; `GrantId`, `ClaimId`, and `DefRevId` are
typed `RecordId`s. The full stream axis is required because `seq` is allocated
per `(share, glade_id, key, origin)`, not globally per origin (P1-B01).
Renewals REUSE the first claim record's `claim_id` (R2-06).

`SignedOp` is an opaque strictly-decoded value that owns its exact validated
canonical bytes plus a decoded envelope view. The unsigned envelope preserves
the current taut `Op` fields 1–10; required signature field 11 is the B5
signature over canonical fields 1–10. `op_hash`, `prev`, and head hashes cover
those same canonical unsigned bytes for compatibility. Gossip forwards the
exact accepted signed bytes; it MUST NOT decode and re-encode them (P1-B07).
Envelope field 8 uses the existing causal `Head{origin,seq,hash?}` shape. It is
distinct from sync `StreamHead{stream,origin,seq,hash}`, whose hash is required;
the two types MUST NOT be conflated.

| record | v3 shape (delta from `sysdata.rs`) |
| --- | --- |
| `ServeClaim` (anchor) | `{node, share, claim_id, grant_ref: GrantId, lease_expiry_ms: WallMs, epoch}` |
| `ServiceInstanceClaim` (NEW) | `{node, share=svc, glade_id, key (canonical, glade-diff §3), claim_id, def_ref: DefRevId, exec_grant_ref: GrantId, compute_key, lease_expiry_ms, epoch}` |
| `CapabilityGrant` | `{grant_id, issuer: Principal, principal, share, verbs, [scope]}` (+id/issuer/scope) |
| `CapabilityRevocation` | `{revokes: GrantId}` (+ signed by an authorized revoker, §below) |

The directory payload is a separately decoded, versioned canonical-CBOR map;
the envelope continues to carry it as opaque bytes. Version 1 is exactly
`{1: schema_version=1, 2: record_kind, 3: body}`. `record_kind` is
`ServeClaim=0 | ServiceInstanceClaim=1 | CapabilityGrant=2 |
CapabilityRevocation=3`. A directory decoder MUST reject an unknown schema
version distinctly as `UnsupportedVersion`, and MUST reject missing fields,
unknown fields, duplicate map keys, non-minimal integers, trailing bytes, and
non-canonical collection order. These failures do not invalidate the outer
signed-op codec's ability to preserve the exact accepted envelope bytes.

Version-1 bodies use these integer field tags:

- `ServeClaim`: `{1:node, 2:share, 3:claim_id, 4:grant_ref,
  5:lease_expiry_ms, 6:epoch}`.
- `ServiceInstanceClaim`: `{1:node, 2:share, 3:glade_id, 4:key, 5:claim_id,
  6:def_ref, 7:exec_grant_ref, 8:compute_key, 9:lease_expiry_ms, 10:epoch}`.
- `CapabilityGrant`: `{1:grant_id, 2:issuer, 3:principal, 4:share, 5:verbs,
  6:scope}`, where `verbs` is a strictly increasing, duplicate-free array using
  `serve=0 | execute=1 | takeover=2`, and `scope` is CBOR `null` or the scoped
  map below.
- `CapabilityRevocation`: `{1:revokes}`.

Nested values are also exact canonical-CBOR integer-key maps:
`RecordId={1:stream,2:origin,3:seq}`;
`StreamId={1:share,2:glade_id,3:key}`;
`WorkspaceSlot={1:kind=0,2:share}`;
`BindingSlot={1:kind=1,2:share,3:glade_id,4:key}`;
`ExecutionScope={1:kind=0,2:def_ref,3:compute_key}`; and
`TakeoverScope={1:kind=1,2:slot,3:supersedes}`. Text identifiers are CBOR text,
keys and compute keys are CBOR byte strings, `lease_expiry_ms` is a signed i64,
and sequence/epoch values are unsigned u64. Encoders MUST produce this one
representation; decoders MUST NOT accept alternate tags or representations.

Discovery-consumed verbs are `serve | execute | takeover`. Grant scope is
`Execution{def_ref,compute_key}` or
`Takeover{slot,supersedes:ClaimId}`; absence of scope never implies either
specific authority.

**Two authority planes** (R2-02 — a share-owner grant CANNOT authorize a derived
instance):

- **Workspace serve-plane:** a `ServeClaim.grant_ref` resolves to a
  `CapabilityGrant{verbs∋"serve"}` whose `issuer` is the **stable owner** of
  `share`. Ownership proof (R2-07): the issuer is the share's root authority per
  the account/workspace ownership chain (E-users-1 root fp; AZ-17/E-ws-1) — a
  valid signature proves identity, the **ownership predicate** proves authority.
  The claim also requires `claim signer == envelope origin == grant.principal`
  and the configured binding of that principal to `claim.node`; a different
  publish-authorized signer cannot reuse another principal's grant. A revocation
  is authorized iff its signer is that owner (or an admin ancestor, if
  governance defines one). Serve-grants ride the HOME serve-plane
  (self-verifying home-locally); content ACLs ride the workspace (§7).
- **Derived-service plane (R2-02):** a `ServiceInstanceClaim` is authorized iff
  `exec_grant_ref` resolves to a **D5 execution grant scoped to exactly
  `(def_ref: DefRevId, compute_key)`** AND the claim's signer/origin is the
  **derived-service principal bound to `node`** (the serving node). A GENERAL
  execution grant does NOT authorize arbitrary instance claims — the grant is
  per-`(def-revision, compute-key)`. Binding checks (frozen equality): `claim
  signer == envelope origin == derived-service principal ∧ derived-service
  principal bound-to == claim.node ∧ exec grant.scope == (claim.def_ref,
  claim.compute_key)`.

**INV-D2 (authorized-before-specificity):** projection admits a claim only after
its plane's authority verifies; a more-specific forged/unauthorized claim never
outranks a lower authorized one.

## 5. FROZEN — the taut protocol, incl. the full sync round (R2-05, R2-11, R2-12)

| msg | carries | notes |
| --- | --- | --- |
| `DirOp` | one `SignedOp` | ingest §3.1; own claims via §9 only |
| `SyncStart` | `SyncId, [StreamHead{share,glade_id,key,origin,seq,hash}]` | R2-05: a correlated round; heads are per-origin (matches `store` `Head`) |
| `SyncOps` | `SyncId, [SignedOp]` (chunked, `OPS_PER_CHUNK`) | verify-as-ingest per op |
| `SyncEnd` | `SyncId` | **explicit terminal** (replaces stream-close); completes the announced round |

**Sync round state machine (FROZEN, R2-05):** initiator sends `SyncStart(id,
myheads)`; responder computes the gap, streams `SyncOps(id, …)`, then `SyncEnd(id)`.
Per `SyncId`: dedup by id; out-of-order/gapped chain suffix is **rejected**
(verify-as-ingest, `peer.rs`), re-requested on the next round. A `Schedule`d
`sync-timeout` wakeup fires retry with a budget (`SYNC_RETRIES`); a missing
`SyncEnd` before timeout ⇒ retry, NOT "assume complete". `SYNC_TIMEOUT_MS` is
a required non-zero node configuration (default `10_000 ms`). Gossip cadence:
each node schedules `gossip-tick` per peer; a round is complete only on `SyncEnd`.
`State.sync: Map<(NodeId,SyncId), RoundProgress>`.

A timeout retry reuses the same `SyncId`; a duplicate `SyncStart` idempotently
replays/recomputes that round's response. A later gossip tick starts a fresh
deterministically allocated `SyncId`. Round state is peer-qualified.

- `Route` / `Reply` are the INTERNAL routing api (§7); `RouteAns = Matched{node}
  | NoClaim`. **No `needs-instantiation`, no `def_ref`** leaves discovery (R2-09).

## 6. FROZEN — routing + total order (R2-06)

`project` then: candidates = winners exactly matching the canonical `Slot`
`(share, glade_id?, key?)`;
most-specific wins, scoped per-binding. **Slot total order (R2-06):** `max` by
`(epoch, claim_id)` — `claim_id` is stable across renewals, so two equal-epoch
claimants CANNOT alternate. `claim_id` is the immutable record identity (§4),
total and deterministic. **Epoch discipline:** ordinary **renewal reuses its
epoch and claim_id**; only an **explicitly authorized takeover** mints
`epoch+1` (a distinct `claim_id`). Initial publish, renewal, and takeover enter
through typed `Advertise{ClaimCommand}`; takeover carries its authority
reference and is checked against `KernelConfig`. A losing global instance emits
`Teardown{slot,generation}` and does not republish (glade-diff D2) — no
counter-claim, no ping-pong.

`ClaimRenew{slot,generation}` is a stale-safe local wake notification only in
v1. It MUST NOT invent a fresh expiry or intent. A real renewal enters through
`Advertise{mode=Renew,...}` with the node's explicit fresh expiry and intent;
the trusted node owns that renewal policy/timer.

- **INV-D3 (no oscillation):** under sustained partition with both claimants
  alive, post-heal the slot converges to one winner and stays; no renewal
  triggers a counter-claim.
- Discovery does **no consumer authz** (R2-01) and **no definition match** (R2-09);
  it returns `Matched{node}` or `NoClaim`.

## 7. FROZEN — the trust boundary (R2-01, R2-09)

Discovery is internal and authz-blind. The **trusted node** wraps it:

1. the trusted ingress suppresses duplicate `(IngressId, Corr)` requests and
   replays its first completed response; only the first request reaches the
   kernel. The kernel resolves every `Route` it receives against current state
   and keeps no route-deduplication state (P1-B03).
2. discovery → `Matched{node}` | `NoClaim` (internal only).
3. the node runs **consumer authorization** — INV-7 source-closure
   (`can_read(...)` over the target/derived-source closure per D3) — BEFORE
   exposing a `NodeId`, a definition identity, or any instantiation signal.
4. only for an AUTHORIZED `NoClaim` does the **service manager** match an
   authorized `DemandServiceDefinition`, run the D5 execution check + placement,
   and (if it instantiates) publish the instance claim via §9.

**INV-D4 (no leak / no unauthorized compute):** an unauthorized caller receives
neither a `NodeId`, a definition identity, nor an instantiation — the node
suppresses before exposing. Definition governance, D5, placement, and
metadata-suppression live OUTSIDE discovery entirely (R2-09).

## 8. FROZEN — append lifecycle (R2-03)

For an initial claim, `Append{intent,slot,generation,draft}` carries a
`ClaimDraft` with no `claim_id`. The node allocates the full `RecordId`, inserts
it as `claim_id`, signs (B5), **persists the exact canonical signed bytes**, then
returns `OpAccepted{intent,op,verification}`. Renewals carry their established
`claim_id`; takeovers mint a new one. The kernel validates intent, slot,
generation, finalized payload, signer/origin, and verification before indexing,
then emits `Gossip` of the exact accepted bytes. A mismatch is rejected and
never gossiped. **Gossip strictly AFTER persist** (INV-D5).

Restart reconstructs pending intents from durable `mine` state and schedules an
idempotent retry. Pre-accept idempotency is keyed by
`(slot,generation,intent)`—not the not-yet-known `claim_id`. A stale-generation
`OpAccepted` arriving after supersession is dropped.

## 9. FROZEN — state, bounds, deferred

```
State {                                    // R2-08/10/11
  config:    KernelConfig                  // immutable authority/node/peer/bounds input
  retained:  Map<Share, RecordSet>         // §3.2 fold (persisted)
  retained_bytes: u64                      // canonical byte accounting (persisted)
  time_deferred:Set<RecordId>              // uncertain-clock claims (persisted)
  unresolved:Map<GrantId, [ClaimRef]>      // §3 pending-proof (persisted)
  mine:      Map<(Slot, Generation), OwnClaim>   // slot+gen keyed (many svc instances)
  sync:      Map<(NodeId,SyncId), RoundProgress> // §5
  clock:     ClockState                    // §2 (persisted)
  next_sync: u64                           // deterministic local id allocation
}   // NO pending/completed routes — ingress dedupes (§7)
```

- **Bounds (FROZEN defaults, R2-12/DR-14):** `MAX_RECORD = 16 KiB`, `MAX_MSG =
  1 MiB`, `MAX_KEY = 4 KiB`, `OPS_PER_CHUNK = 256`, `SYNC_RETRIES = 3`,
  `SYNC_TIMEOUT_MS = 10_000`, per-node gossip fan `≤ 8`, per-ingress route rate
  limited. `MAX_RETAINED_BYTES` is a
  required non-zero node configuration (no universal default because node
  storage classes differ). Breach ⇒ typed reject.
- **Storage ceiling (P1-B05):** accepting canonical bytes that would exceed
  `MAX_RETAINED_BYTES` yields `StorageExhausted`; the op is not folded. The node
  continues serving existing state but MUST NOT publish or renew claims while
  exhausted. Both the kernel's canonical-byte accounting and the host's physical
  store quota MUST be enforced. Safety remains fail-closed; convergence/liveness
  are not claimed for explicitly exhausted nodes.
- **Compaction DEFERRED (R2-11):** v1 does NOT delete chain prefixes (dropping a
  prefix breaks `prev`-verification for a fresh peer). v1 **retains the chain**
  up to the configured byte ceiling; signed checkpoints/snapshots are later
  hardening (own `WD-`/GDL row). The ceiling bounds storage, while rate/size
  limits bound exhaustion speed.
- Locality rendezvous DEFERRED (Model §5, WD-6): the kernel stays topology-blind.

## 10. FROZEN — invariants (the oracle the scenarios assert)

- **INV-D0** purity/determinism (§1). **INV-D1** no lease resurrection under wall
  rollback/restart (§2). **INV-D2** authorized-before-specificity (§4). **INV-D3**
  no epoch oscillation under partition/heal (§6). **INV-D4** no NodeId/def/compute
  leak to an unauthorized caller (§7). **INV-D5** gossip only after persist (§8).
  **INV-D6** convergence: any connected, non-storage-exhausted topology, under
  loss/reorder/dup/partition, converges the retained fold to one directory
  (monotone set-union).

## 11. FROZEN — failure outcomes (every axis has one terminal answer)

Malformed/BadSig/BadChain → quarantine, never folded, never a routing miss.
PublishUnauthorized → rejected permanently. StorageExhausted → reject new
retained bytes, serve existing state, no publish/renew. UnresolvedProof →
retained, projects absent until proof folds. Expired/rolled-back → absent
(fail-closed). Route: exactly one `Reply` (`Matched|NoClaim`) per delivered
request; trusted ingress suppresses duplicates and replays its first response
(§7). Sync timeout → retry the same `SyncId` to budget → give up (peer marked
stale, next tick uses a fresh id). Append with no OpAccepted → idempotent retry
on restart.

## 12. Determinism + scenario schema (FROZEN typed, R2-12)

PRNG = `splitmix64(u64 seed)`; documented draw order maps to fault kinds.
Ordering: effects in emission order; gossiped records by
`(lamport, stream, origin, seq)`;
simultaneous inputs by `(t, insertion-seq)`; wakeups by `(at_mono, token)`. Every
generated effect gets a `msg-ref = (producing-event-id, emission-index)`.

```
Scenario {
  seed: u64
  event_budget: NonZeroU64
  nodes: [NodeSpec{id,principal,owns?,seed_grants?,max_retained_bytes?:NonZeroU64,
                   clock:[ClockChange],restarts:[RestartSpec]}]
  links: [LinkSpec{a,b,latency_ms,faults:[LinkFault]}]
  inputs: [ScenarioInput{input_id,at_ms,node,event}]
  verifier_fixtures: [VerifierFixture{selector,reader?,outcome}]
  stop: AtMs{at_ms} | QuiescentForMs{quiet_ms:NonZeroU64,deadline_ms}
  expect: [converged|route|effect|state]  // no end-to-end ingest verdict expectation
}

OpSelector = Input{input_id,op_ordinal}
           | Minted{node,slot,generation,append_ordinal}
RestartSpec = {at_ms, watermark: Preserve|Unreadable|Replace{value}}

ClockSpec = {initial_wall_ms:i64, initial_mono_ms:u64, changes:[ClockChange]}
ClockChange = SetOffset{at_ms,offset_ms:i64}
            | Drift{start_ms,end_ms,rate_ppm:i32}
LinkFault = Loss{start_ms,end_ms,probability_ppm:u32}
          | Partition{start_ms,end_ms}
          | Reorder{start_ms,end_ms,extra_delay_ms:u64}
          | Duplicate{start_ms,end_ms,copies:NonZeroU16,spacing_ms:u64}
          | Latency{start_ms,end_ms,latency_ms:u64}

ScenarioEvent = Deliver{from,msg:WireMsg}
              | Route{ingress,principal,corr,query}
              | Advertise{command:ClaimCommand}
              | OpAccepted{intent,op:SignedOp}
              | Wakeup{token:WakeToken}
              | ClockReseed{watermark:WallMs}

Expectation = {when: AtMs{at_ms}|Throughout{start_ms,end_ms}, kind}
kind = Converged{nodes:[NodeId]}
     | Route{node,ingress,corr,ans:RouteAns}
     | Effect{node,effect:Effect,count:u32}
     | State{node,assertion:StateAssertion}
StateAssertion = Clock{value:ClockState}
               | Retained{record_id,present:bool}
               | Unresolved{grant_id,claim_id,present:bool}
               | Mine{slot,generation,status:Pending|Accepted|Lost}
               | Sync{peer,sync_id,status:Active|Complete|Stale}
               | Storage{retained_bytes:u64,exhausted:bool}
```

An omitted `NodeSpec.max_retained_bytes` uses the simulator default of 16 MiB;
scenarios that assert storage behavior MUST set it explicitly.

`ClockChange` is a typed per-node wall-offset/drift schedule. `LinkFault` is a
strict tagged union of loss, partition, reorder, duplicate, and latency changes;
there is no generic `param`, and clocks are never link properties. All
`start_ms,end_ms` intervals are non-empty half-open `[start_ms,end_ms)`;
overlapping drift intervals are invalid. Wall time advances one ms per simulator
ms plus the current offset and the checked integral drift adjustment
`elapsed_ms * rate_ppm / 1_000_000` (truncated toward zero). Input op
ordinal is zero for `DirOp`/`OpAccepted` and vector order for `SyncOps`; op-free
or out-of-range selectors are decode errors. Scenario JSON represents an
embedded `SignedOp` only as `{canonical_hex}`; the strict protocol decoder
constructs its decoded view or rejects the scenario.

Links are bidirectional. Active faults apply in declaration order. The last
active `Latency` replaces base/current latency; each `Reorder` adds its delay;
`Duplicate.copies` is the number of additional copies (total `1 + copies`). Each
active `Loss` consumes exactly one SplitMix64 draw in declaration order and
drops when `draw % 1_000_000 < probability_ppm`, even if an earlier active loss
already selected drop; the fixed draw schedule MUST NOT depend on short-circuit
control flow.

A minted selector is allocated on the first globally ordered emission of each
distinct `IntentId`; retries reuse it. It resolves only when signing/persistence
binds it to canonical `SignedOp` bytes, and simulator-side metadata preserves
that binding through gossip/sync. Any unbound minted fixture fails scenario
completion. `VerifierFixture.outcome` is exactly `VerificationResult`.
Reader-specific fixtures replace global ones; absent fixtures yield `Valid`
with the envelope signer in simulation only.

Periodic `WakeToken` variants define their own `WakeClass`; scenario data cannot
forge it. Quiescence ignores only one later successor `Schedule` for the same
periodic token. Any state change or other effect resets quiet time. Deadline
yields typed `DidNotQuiesce`; exceeding `event_budget` yields typed
`EventBudgetExceeded`. `AtMs` processes the complete event-time bucket.
`Expectation.AtMs` evaluates after that bucket; `Throughout` must hold after
every event in its half-open interval. `Converged` means exact equality of the
listed nodes' retained `RecordId` sets. Unknown fields, dangling references,
duplicate ids/fixture keys, invalid intervals, stop-overrun inputs, and checked
arithmetic overflow are decoder errors.

## 13. Traceability (finding → frozen decision → RED scenario)

| GD56R2 | resolution (frozen) | scenario (red) |
| --- | --- | --- |
| 01 authz boundary | §7 discovery authz-blind, internal API; node authorizes first | s-disc-authz-boundary |
| 02 derived authority | §4 derived-service plane: def_ref+exec-grant scoped to (def,compute_key)+node binding | s-disc-inst-authority / -forged-node / -wrong-def / -revoked-exec |
| 03 append boundary | §1/§8 draft→finalize/sign/persist→OpAccepted→exact-byte Gossip; restart idempotent | s-disc-append-restart / -delayed-sign |
| 04 clock rollback | §2 persisted ClockState; restart uncertainty + trusted reseed fail-closed | s-disc-wall-rollback / -restart-uncertain / -skew |
| 05 sync round | §5 SyncId round SM + timeout/retry/dup + explicit SyncEnd | s-disc-sync-round / -sync-drop / -sync-retry |
| 06 takeover order | §6 (epoch, stable claim_id); renewal≠takeover; loser tears down | s-disc-epoch-tie / -no-ping-pong |
| 07 grant/revoke authority | §4 GrantId=record identity + owner proof + authorized-revoker | s-disc-owner-proof / -unauth-revoke / -regrant |
| 08 verdict/fold split | §3 three verdicts; pending-proof index; proof-arrival either order | s-disc-proof-late / -revoke-then-grant |
| 09 def matching OUT | §7 discovery returns Matched/NoClaim; service manager matches | s-disc-noclaim-handoff |
| 10 return path | §1/§7 IngressId + Reply; trusted-ingress duplicate suppression; no route state | s-disc-corr-collision / -route-terminal |
| 11 compaction | §9 DEFERRED — retain chain to hard byte ceiling | s-disc-dos-bound |
| 12 determinism/schema | §12 strict typed Scenario + PRNG + msg-ref + deadlines | (harness self-test) |

## 14. Next (not phasing)

The `s-disc-*` scenarios above are authored as RED failing tests against the
frozen §1 API + §10 invariants BEFORE the kernel; then the kernel crate, the
`glade-discover.taut.py` (§4/§5), and the sim harness (§12). The next review is
code + green scenarios.
