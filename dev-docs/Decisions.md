# Glade Discover Implementation Decisions

This log records implementation choices only. It MUST NOT revise the vendored
v3.1 semantic freeze. A semantic conflict blocks the affected lane until the
parent design is amended and explicitly re-vendored.

## DISC-IMP-001 — Snapshot provenance for an untracked parent design

- Status: accepted
- Date: 2026-07-17
- Context: the v3 source document was untracked at workspace base revision
  `803f3656736900633b7106bf8e8c40d4d6bd9283` when P0 began.
- Decision: record the base revision and source status, but identify the exact
  semantic freeze by SHA-256
  `d73b765c76e4c3bebb98bd7bce1f6fc2e7f99d2f9215c3bcd13ed683cab864af`
  after the accepted v3.1 P1 amendment.
  The snapshot checker MUST validate both the vendored body and the parent body
  when the parent workspace is present.
- Consequence: provenance does not falsely claim the untracked content exists
  in the named Git commit; the content hash remains reproducible.

## DISC-IMP-002 — Rust language baseline

- Status: accepted
- Date: 2026-07-17
- Decision: use Rust edition 2024 with MSRV 1.85.0, the first stable release
  supporting that edition. The workspace MUST deny warnings in its verification
  gate and MUST commit `Cargo.lock`.
- Consequence: the core may use edition-2024 language rules but MUST remain free
  of runtime, OS, network, filesystem, and ambient-randomness dependencies.

## DISC-IMP-003 — Simulator verifier fixture addressing

- Status: accepted
- Date: 2026-07-17
- Decision: scenario inputs use stable input ids plus an op ordinal;
  runtime-minted operations use `(node, slot, generation, append_ordinal)`, with
  reader-specific fixtures taking precedence over global fixtures. A minted
  selector resolves only when persistence binds canonical signed bytes. Missing
  fixtures are simulator-valid by default. The verdict never enters a wire or
  production record.
- Consequence: forged and reader-dependent verification schedules are data, not
  scenario-specific branches, and runtime `(origin, seq)` allocation remains an
  environment concern.

## DISC-IMP-004 — P1 implementation-discovered semantic amendments

- Status: accepted in parent v3.1
- Date: 2026-07-17
- Decision: apply the dispositions recorded in
  [`P1ContractBlockers.md`](P1ContractBlockers.md). In particular, the trusted
  ingress owns duplicate suppression, the host atomically commits durable state
  before effects, and a required `MAX_RETAINED_BYTES` ceiling bounds v1 storage.
- Consequence: P1 may freeze implementable public contracts without hidden I/O,
  unbounded retained history, or a route-response cache in the kernel.

## DISC-IMP-005 — P2 deterministic encodings and fault application

- Status: accepted in parent v3.1
- Date: 2026-07-17
- Decision: the version-1 directory payload uses the exact versioned
  canonical-CBOR integer-tag layout frozen in parent §4. Simulator links are
  bidirectional. Active link faults apply in declaration order: the last
  latency replaces the current latency, reorder delays add, duplicate `copies`
  are additional deliveries, and every active loss consumes one SplitMix64
  draw and drops when `draw % 1_000_000 < probability_ppm`.
- Consequence: protocol corpus bytes and simulator event logs are portable
  executable oracles rather than implementation-dependent fixtures.

## DISC-IMP-006 — P3/P4 executable timing and bounds amendments

- Status: accepted in parent v3.1
- Date: 2026-07-17
- Decision: expose protocol `op_hash` as SHA-256 over exact canonical unsigned
  fields 1–10; add required non-zero `KernelConfig.sync_timeout_ms` with a
  10,000 ms default; treat `ClaimRenew` as a stale-safe notification while
  actual renewal remains an explicit `Advertise::Renew`; and allow scenarios to
  override a node's non-zero retained-byte ceiling (16 MiB default).
- Consequence: chain links, retry scheduling, renewal responsibility, and the
  storage-bound oracle have one deterministic implementation-independent
  meaning.

## DISC-IMP-007 — P5 reproducible hardening profiles

- Status: accepted
- Date: 2026-07-17
- Decision: keep bounded deterministic mutation, property, model, seed, and
  restart sweeps in ordinary workspace tests so CI always runs a reproducible
  smoke profile. Keep protocol and scenario libFuzzer entry points in the
  nested `fuzz` workspace for longer local or scheduled campaigns. Any failure
  MUST be reduced to a fixed seed or fixture and promoted to the ordinary test
  suite before its production fix.
- Consequence: the required smoke gate does not depend on nightly or ambient
  randomness, while the same decoder boundaries remain available to a
  coverage-guided engine.

## DISC-IMP-008 — P6 durable driver and bounded trusted ingress

- Status: accepted
- Date: 2026-07-17
- Decision: the node driver MUST atomically commit the next kernel persisted
  state and effective-wall watermark before interpreting any emitted effect.
  Append acceptance is durably keyed by `(slot, generation, intent)` and bound
  to the exact draft and canonical bytes. Trusted-ingress terminal replay MUST
  have non-zero global and per-ingress entry limits plus a request-key byte
  limit. Admission failures are non-terminal `AdmissionError` values and MUST
  NOT enter duplicate replay state; ingress teardown MUST evict its entries.
- Consequence: effect or transport failure cannot roll back a pure fold,
  restart cannot remint an accepted intent, and trusted duplicate suppression
  cannot grow memory without a configured ceiling or ingress lifecycle release.

## DISC-IMP-009 — P7 crash-safe effect delivery and restore composition

- Status: accepted
- Date: 2026-07-17
- Decision: the trusted node driver MUST atomically commit kernel state, the
  effective-wall watermark, and an adapter-owned durable effect outbox before
  executing any effect. Restore MUST accept one versioned atomic snapshot and
  MUST authenticate and integrity-check it before executing a loaded effect.
  Snapshot creation MUST use create-if-absent; replacement and incremental
  effect acknowledgement MUST compare an expected monotone revision. Outbox
  entry ids, their next allocation value, and stable structural payload-byte
  accounting MUST survive restart. Normal limits MUST NOT exceed recovery
  limits. The driver MUST enforce finite normal and recovery entry/byte limits
  and reject invalid limits or overflow before committing a kernel transition.
  If a valid loaded backlog prevents recovery from fitting, restore MUST drain
  authenticated entries with revision-checked acknowledgements until recovery
  fits or return an explicit blocked result that retains the capacity cause.
  Successful effects MUST be durably acknowledged; failed or interrupted
  effects MUST remain replayable. Append execution MUST be
  idempotent by `(slot, generation, intent)` and teardown execution MUST be
  idempotent by `(slot, generation)`. Production restore MUST compose loaded
  outbox entries with kernel recovery effects, and wall scheduling MUST install
  and commit the observed clock state through the driver before the timer
  effect executes. Persisted monotonic schedules MUST be rebased to the new
  process monotonic epoch during authenticated restore.
- Consequence: append and teardown are delivered at least once across executor
  failure and process crash, recovery effects cannot be silently discarded,
  and a caller cannot persist a forward watermark while continuing with a
  stale driver clock. Duplicate execution remains possible only at the named
  idempotent trusted-host boundaries.

## DISC-IMP-010 — P7 outbound sync wire bounds

- Status: accepted
- Date: 2026-07-17
- Decision: every outbound sync body MUST fit `MAX_MESSAGE_BYTES` in addition
  to the `OPS_PER_CHUNK` item ceiling. `SyncOps` MUST use deterministic greedy
  byte-aware chunks in missing-operation order. `SyncId` UTF-8 bytes MUST NOT
  exceed 256 at decode or the direct pure-core boundary. The responder MUST
  plan the complete configured retained gap and append `SyncEnd`; it MUST emit
  neither a partial operation nor `SyncEnd` if one missing operation cannot fit
  with the supplied `SyncId`. `SyncStart` MUST include the largest deterministic
  head prefix that fits; omitted heads are conservative because they cause the
  peer to return extra duplicate-safe operations. A locally generated
  empty-head `SyncStart` that cannot fit MUST mark the round stale without a
  timer. A node MUST answer `SyncStart` only for a configured peer, and
  configured local/peer node ids MUST fit the 235-byte bound that leaves room
  for the largest generated `SyncId`. The default durable outbox has 16,384
  normal entries and a 32 MiB payload budget. Driver construction and restore
  MUST conservatively validate entry/byte capacity against the configured
  retained ceiling or authenticated actual retained bytes, whichever is
  larger, plus maximum record/frame sizes, the correlation bound, and actual
  configured peer-id length. Restore MUST recompute the canonical retained-byte
  sum and reject a mismatch before loaded effects execute. Incompatible custom
  configurations fail before traffic or loaded effects.
- Consequence: a valid retained set cannot generate messages the conforming
  receiver rejects solely for size or overflow the default durable outbox.
  Correlation-id repetition cannot amplify response allocation without a small
  independent bound, and every successful response retains the frozen complete
  gap-then-terminal semantics. A node cannot enter service with a retained
  ceiling its durable outbox can never synchronize.

## DISC-IMP-011 — P7 complete throughout-oracle and pinned fuzz CI

- Status: accepted
- Date: 2026-07-17
- Decision: every schema-valid `Throughout` expectation MUST be evaluated
  after every processed event in its half-open interval. Observed effects MUST
  retain the producing event id so same-timestamp route expectations remain
  distinguishable. `Converged` MUST compare exactly the retained `RecordId`
  sets, not their locally stored operation values. CI MUST compile the nested
  fuzz workspace with the pinned `nightly-2026-02-28` toolchain used by every
  documented fuzz command.
- Consequence: the strict schema has no accepted-but-unimplemented expectation
  surface, and fuzz-target compilation is reproducible across repository runs.

## Open decisions

None through the P7 review gate.
