# P1 Contract Blockers

Status: resolved by the parent v3.1 amendment on 2026-07-17.

These issues were found while writing the first RED public-contract tests. They
could not be resolved by private implementation choices without changing or
completing v3 semantics. The accepted dispositions below were applied to v3.1
before affected public types landed.

| Blocker | Accepted disposition |
| --- | --- |
| P1-B01 | full `{stream,origin,seq}` record identity |
| P1-B02 | initial/takeover `ClaimDraft`; adapter finalizes identity |
| P1-B03 | duplicate suppression/replay at trusted ingress |
| P1-B04 | explicit verification batch plus immutable `KernelConfig` |
| P1-B05 | atomic durable commit before effects plus hard retained-byte ceiling |
| P1-B06 | `Ready/Uncertain{floor}` and trusted reseed |
| P1-B07 | `SignedOp` owns exact canonical accepted bytes |
| P1-B08 | ingest verdict assertions restricted to direct module/corpus tests |
| P1-B09 | explicit advertise/reseed inputs, teardown effect, config/query types |
| P1-B10 | strict selector, clock, restart, deadline, and event-budget schema |

## P1-B01 — Record identity is not unique in the current store

V3 defines `GrantId` and `ClaimId` as `(origin, seq)`. The current store allocates
`seq` independently for every `(share, glade_id, key, origin)` chain, so one
origin can have the same sequence in several chains.

Recommended ruling: define the immutable record identity as:

```text
RecordId = { stream: {share, glade_id, key}, origin, seq }
GrantId = RecordId
ClaimId = RecordId
DefRevId = RecordId
```

Discovery SHOULD retain the existing per-stream store model rather than invent
one global discovery chain per origin.

Evidence: v3 §4; `glade/node/src/store.rs` chain identity and `chain_of`.

## P1-B02 — Initial claim payload is self-referential

An initial claim payload must contain its `claim_id`, but v3 says the node
allocates the sequence—and therefore the `claim_id`—only after receiving
`Append { payload }`. The kernel cannot put an unknown record identity into the
payload it emits.

Recommended ruling: `Append` MUST carry a typed `ClaimDraft`. The node allocates
the record identity, inserts it into the finalized claim, signs, persists, and
returns `OpAccepted`. Pre-accept idempotency MUST use `(slot, generation,
intent)`, not `claim_id`. Renewals carry the already-established `claim_id`.

Evidence: v3 §4 record shapes and §8 append lifecycle.

## P1-B03 — Duplicate-route idempotency requires omitted state

V3 requires a duplicate `(IngressId, Corr)` to return the same answer. Re-running
resolution after the retained fold changes can return a different answer, while
the frozen State has no completed-reply ledger. “No pending routes” does not
solve completed-response deduplication.

Ruling required: either:

1. add a completed-reply cache and define its persistence, size, eviction, and
   idempotency horizon; or
2. move duplicate suppression to the trusted ingress and change the kernel rule
   to resolve every delivered `Route` against current state.

An unbounded core cache MUST NOT be introduced implicitly.

Evidence: v3 §9 State and §11 failure outcomes.

## P1-B04 — Verification and authority have no explicit kernel input

Ingest must verify B5 signatures and publication authority, but frozen
`step(state, ctx, event)` receives clocks plus unverified wire bytes only.
Reader-specific simulator verifier outcomes, ownership roots, and
node-to-derived-principal bindings cannot affect the kernel without hidden
callbacks, which would violate INV-D0.

Recommended ruling:

- `Deliver`/`OpAccepted` MUST carry local non-wire verification results for each
  contained op, or `step` MUST receive an immutable verification view;
- State or an explicit immutable `KernelConfig` MUST carry local node identity,
  peers, owner roots, derived-principal bindings, and frozen constants;
- verifier fixtures MUST remain simulator-only and MUST produce those explicit
  local inputs rather than mutate `SignedOp`.

## P1-B05 — Durable state has no persistence ordering contract

V3 marks retained records, unresolved proofs, and the wall watermark durable,
but the API has no persistence effect or acknowledgement. A crash after an
`Append` effect but before the advanced watermark is durable can violate INV-D1.

Ruling required: either add a `PersistState -> StateAccepted` lifecycle, or
normatively require the driver to atomically persist returned durable state
before executing any dependent effects. Restart restoration MUST have an
explicit constructor/event contract.

## P1-B06 — Clock-uncertain recovery is incomplete

`resync` has no frozen value, an unreadable watermark has no known recovery
floor, State has no uncertainty variant, and the event API has no restart/init
input.

Recommended ruling:

```text
ClockState = Ready { watermark }
           | Uncertain { floor: Option<WallMs> }
```

A readable backward wall MAY recover after a frozen checked
`CLOCK_RESYNC_MS`. An unknown floor MUST remain fail-closed until an explicit
trusted reset/reseed input; it cannot recover from a comparison to missing data.

## P1-B07 — Exact accepted bytes are not represented

`OpAccepted { SignedOp }` cannot prove P6's exact-byte requirement if decoding
and re-encoding can change representation. The current `Op` also has fields
1–10 but no B5 signature field.

Recommended ruling: `SignedOp` MUST own validated canonical bytes. `Gossip`
MUST forward that same blob. The canonical unsigned envelope SHOULD preserve
the current `Op` fields 1–10, with a ruled signature envelope/field whose
coverage and op-hash calculation are fixed before codec work.

## P1-B08 — Ingest outcomes are not observable from `step`

The scenario schema permits `expect.kind = ingest`, and v3 names distinct
structural/governance verdicts. All rejected inputs currently produce the same
observable result: unchanged state and no effect.

Ruling required: add typed deterministic observations to the step result, or
remove ingest-verdict expectations from end-to-end scenarios and assign them
only to direct ingest-module tests. Hidden simulator inspection MUST NOT become
the oracle.

## P1-B09 — The frozen event/config surface cannot initiate required behavior

The State/API omits local node identity and peers, although gossip is addressed
and periodic per peer. It also has no local publish/takeover input, so initial
claims and authorized takeovers cannot be initiated through the stated “ONLY
inputs.” Append retry after restart similarly lacks a boot/reconstruction rule.
The exact `Route` query/slot fields are not frozen.

Recommended ruling: freeze `KernelConfig`, the canonical binding query/slot,
and explicit local lifecycle inputs (or a separate pure command API) before P3.

## P1-B10 — The frozen scenario sketch cannot express required tests strictly

The P1 schema needs these corrections:

- `Input { input_id }` MUST add `op_ordinal` because `SyncOps` carries several
  ops. `OpAccepted` and `DirOp` use ordinal zero; `SyncOps` uses vector order.
- A minted verifier selector resolves only when signing/persistence creates and
  binds canonical op bytes, not when `Append` is merely emitted.
- `QuiescentForMs` MUST include a failure deadline, and each scenario MUST have
  an event budget to stop zero-time loops with typed `DidNotQuiesce` failure.
- wall offset/drift MUST be typed per-node clock schedules, not link faults.
- restart entries MUST distinguish preserved, unreadable, and explicitly
  replaced/corrupted watermarks.
- generic fault `param` MUST become a strict tagged union with per-kind fields.

These are scenario-contract changes because v3 §12 labels the sketch FROZEN.

## Closure

The parent v3.1 semantic freeze rules P1-B01 through P1-B10. The amended design
MUST be re-vendored with new provenance before P1 public-contract tests resume.
