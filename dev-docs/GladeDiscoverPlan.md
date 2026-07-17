# Glade Discover Implementation Plan

Status: P0 through P7 implementation and review complete (2026-07-17);
initial repository commit pending user direction
Target: `glade-discover`, a Rust member repo of `glade-wz`
Normative design: `../dev-docs/glade/GladeDiscoveryDesign.md` in the parent
`glade-wz` workspace, v3.1 semantic freeze
Execution posture: TDD-first, scenario-first, agent-parallel

## 1. Goal

Build the pure `glade-discover` state machine, its taut protocol, and its
deterministic adversarial simulator exactly as frozen by the v3.1 design.

The delivered system MUST:

- implement the frozen `step(state, ctx, event) -> (state', effects[])` API;
- keep I/O, persistence, signing, and clock reads outside the kernel;
- encode every wire and record contract canonically and fail closed;
- execute every `s-disc-*` scenario as typed data, with deterministic replay;
- prove INV-D0 through INV-D6 under the specified failure schedules;
- integrate with the node only through explicit adapters after the standalone
  kernel and scenario suite are green.

This plan deliberately optimizes for parallel execution. It isolates work into
crates and modules with explicit ownership so agents can work concurrently
without editing the same files or deciding contracts independently.

## 2. Non-negotiable development rules

1. Every behavior change MUST begin with a failing unit, corpus, property, or
   data-driven scenario test.
2. Production code MUST be the smallest change that makes the new test pass.
3. Refactoring MUST occur only while all affected tests are green.
4. Every defect MUST gain a regression test reproducing the failure.
5. Every feature MUST cover success, rejection/failure, and boundary cases.
6. The v3.1 semantic freeze MUST NOT be changed implicitly in code. A discovered
   contradiction stops that lane and creates a named decision issue. The
   vendored design snapshot is read-only; semantic changes happen in the parent
   workspace and are then re-vendored with new provenance.
7. Scenario-specific branches in the kernel or simulator are forbidden. A
   scenario supplies data; generic state transitions produce the result.
8. No agent other than the integration owner may edit shared workspace
   manifests, public re-export files, or cross-crate dependency declarations.
9. Tests MUST NOT read real user state, real clocks, the network, or
   `~/.glade`.
10. Commits SHOULD be small, green, and limited to one work package.

## 3. Planned repository shape

The coordinator creates and owns the skeleton before parallel implementation:

```text
glade-discover/
  Cargo.toml                         # coordinator-owned workspace manifest
  Cargo.lock                        # committed: pins replay/test dependencies
  README.md
  dev-docs/
    GladeDiscoverPlan.md
    GladeDiscoveryDesign.md          # read-only pinned v3.1 snapshot
    RequirementTrace.md
    Decisions.md                     # implementation-only decisions
  protocol/
    glade-discover.taut.py
    corpus/
      valid/
      invalid/
  crates/
    glade-discover-protocol/
      src/
        lib.rs                       # coordinator-owned exports
        ids.rs
        records.rs
        wire.rs
        codec.rs
      tests/
    glade-discover-core/
      src/
        lib.rs                       # coordinator-owned exports/step dispatch
        model.rs                     # frozen public state/event/effect types
        clock.rs
        ingest.rs
        authority.rs
        projection.rs
        routing.rs
        claims.rs
        append.rs
        sync.rs
      tests/
    glade-discover-sim/
      src/
        lib.rs
        schema.rs
        queue.rs
        network.rs
        faults.rs
        runner.rs
        oracle.rs
      tests/
    glade-discover-node-adapter/      # final phase only
      src/
        lib.rs
        append.rs
        clock.rs
        ingress.rs
        transport.rs
  scenarios/
    authz/
    claims/
    clock/
    ingest/
    routing/
    sync/
    bounds/
  tests/
    all_scenarios.rs
    deterministic_replay.rs
```

`glade-discover-core` MUST depend only on the protocol crate and small,
deterministic utility dependencies. It MUST NOT depend on Tokio, filesystem,
networking, OS randomness, key stores, or the node implementation.

The node adapter is intentionally a separate crate. Its existence MUST NOT
weaken the pure core boundary.

## 4. Parallel-work protocol

### 4.1 Agent roles

Each wave supports four active roles:

| Role | Stable responsibility | Shared files allowed |
| --- | --- | --- |
| Coordinator/integrator | skeleton, manifests, frozen public types, re-exports, merge verification | yes; sole writer |
| Protocol/scenario agent | taut schema, codec corpus, record/wire tests, assigned scenario data | no |
| Kernel subsystem agent | one explicitly assigned core module and its tests | no |
| Simulator/subsystem agent | simulator module or a second disjoint core module and its tests | no |

Roles may rotate between waves, but file ownership MUST be assigned before work
starts. One agent MUST NOT opportunistically fix another lane's files.

### 4.2 Shared-contract gate

Before the first parallel implementation wave, the coordinator MUST land:

- the Cargo workspace and empty crates;
- frozen public types in `model.rs` and protocol type names;
- module files with compileable stubs;
- trait/function signatures used across lanes;
- strict dependency direction;
- test commands and formatting/lint configuration;
- `RequirementTrace.md` mapping DR/R2 findings, INV-D0..D6, and scenarios to
  owning files and work packages.

After this gate, public types are change-controlled. An agent needing a public
contract change writes a short proposal in `dev-docs/Decisions.md`; only the
coordinator applies the shared edit after checking all lanes.

### 4.3 Conflict avoidance

- The coordinator owns all `Cargo.toml`, `lib.rs`, root test aggregators, and
  generated-code registration.
- Each implementation agent owns one module plus a matching test file/directory.
- Scenario agents own disjoint scenario subdirectories.
- Generated output MUST be regenerated by the coordinator after schema changes;
  agents edit only the taut source and corpus inputs.
- Cross-lane calls use the frozen interfaces; temporary test fakes live in the
  calling lane and MUST NOT modify another lane's implementation.
- Integration happens at named wave gates, not continuously through incidental
  shared edits.

## 5. Dependency graph

```text
P0 semantic import + workspace skeleton
  └─ P1 public type/codec contract + RED harness shell
       └─ P2 parallel RED foundations
            ├─ P2A protocol corpus
            ├─ P2B deterministic queue/fault engine
            ├─ P2C typed scenario data
            └─ P2D core unit-test shells
                 └─ P2.5 walking skeleton
                      └─ P3 parallel kernel lanes
                           ├─ clock + claim lifecycle
                           ├─ ingest + authority + projection
                           ├─ routing + ordering
                           └─ sync + append effects
                                └─ P4 full scenario runner + invariant oracle
                                     └─ P5 hardening/property tests
                                          └─ P6 node adapter integration
                                               └─ P7 code + green-scenario review
```

P1 is a coordinator-only contract gate. P2A through P2D start only after P1
lands and are deliberately parallel. P2.5 then proves one real vertical path
before the four P3 kernel lanes fan out. P4 integrates the complete lanes only
after their module-local tests pass.

## 6. Phases and work packages

## P0 — Import the semantic freeze and establish the workspace

Milestone: the new repo owns enough context to build independently, and all
agents use one canonical contract.

### P0.1 — Import design artifacts

Owner: coordinator. No parallel work before completion.

- Vendor the v3.1 semantic freeze into `dev-docs/GladeDiscoveryDesign.md` as a
  read-only pinned snapshot. The parent `glade-wz` document remains the sole
  source of truth.
- Add a banner: `DO NOT EDIT — vendored from <workspace>@<revision>, sha256
  <content-hash>`.
- Add a check that fails when the body differs from the recorded content hash.
  A semantic update MUST be made in the parent and explicitly re-vendored.
- Create `RequirementTrace.md` with rows for INV-D0..D6 and every `s-disc-*`.
- Create a local `Decisions.md`; it MUST record implementation decisions only,
  not silently revise the semantic freeze.
- Add a README that names the pure-kernel boundary and standard commands.

Verification:

- Vendored design body is byte-identical to its recorded parent snapshot and
  the provenance/hash check passes.
- Every v3.1 scenario name appears in `RequirementTrace.md` exactly once as a
  primary acceptance test, with optional secondary coverage.

### P0.2 — Rust workspace skeleton

Owner: coordinator.

- Create the protocol, core, and simulator crates.
- Reserve the node-adapter crate but do not implement it.
- Pin the Rust edition/MSRV and lint policy.
- Commit `Cargo.lock`; deterministic replay is guaranteed against the checked-in
  dependency graph and CI MUST use `--locked`.
- Add `fmt`, `clippy`, unit-test, corpus-test, and all-scenario commands.
- Add CI only after the commands work locally.

Exit gate:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

All pass with skeleton tests.

## P1 — Freeze public types and create the RED test boundary

Milestone: agents can implement independently against compileable interfaces,
and scenario tests fail because behavior is absent—not because types are vague.

### P1.1 — Protocol and identity interfaces

Owner: coordinator with protocol agent review.

Freeze Rust types corresponding to:

- `NodeId`, `Principal`, `IngressId`, `Corr`, `IntentId`, `SyncId`;
- `StreamId`, full-axis `RecordId`, `GrantId`, `ClaimId`, `DefRevId`,
  `ComputeKey`, canonical `Slot`/`RouteQuery`, `Generation`;
- `WallMs`, `MonoInstant`, `StepCtx`, `WakeToken`;
- exact-byte `SignedOp`, `ClaimDraft`, `ClaimCommand`, records, `WireMsg`,
  `StreamHead`, `VerificationResult`/`VerificationBatch`;
- `KernelConfig`, `ClockState`, and `WatermarkLoad`;
- `Event::{Deliver,Route,Advertise,OpAccepted,Wakeup,ClockReseed}`;
- `Effect::{Gossip,Append,Reply,Schedule,Teardown}`;
- `RouteAns::{Matched,NoClaim}`;
- typed structural, governance, and live-capability verdicts.

Freeze the simulator-only verifier seam separately from production `SignedOp`:

- production `SignedOp` carries the real canonical envelope/signature fields;
- scenario inputs have explicit stable `input_id` values; fixtures MUST NOT use
  positional array indexes;
- scenario data carries
  `VerifierFixture { selector, reader?, outcome }`, where `selector` is
  `Input { input_id, op_ordinal } |
  Minted { node, slot, generation, append_ordinal }`;
- `append_ordinal` is zero-based in deterministic simulator event order over
  the first emission of each distinct `IntentId` for `(node, slot, generation)`;
  a retry of the same intent reuses its selector and does not increment the
  ordinal. This addresses initial, renewal, and takeover appends without
  depending on runtime `(origin,seq)`;
- the simulator resolves fixtures through a deterministic verifier table;
- a fixture without `reader` applies to every reader; a reader-specific fixture
  overrides the selector-global fixture for that reader;
- an op with no matching reader-specific or global fixture verifies
  `Valid { signer: envelope_signer }` in the simulator; this is a
  simulator default only—real verification never defaults to valid;
- duplicate `(selector, reader)` entries are a decode error; at most one global
  fixture may exist per selector;
- input selector ordinal zero addresses `DirOp`/`OpAccepted`; `SyncOps` uses
  vector order; op-free/out-of-range selectors MUST fail decode;
- every minted selector MUST bind to canonical signed bytes produced by
  signing/persistence by scenario completion; `Append` emission alone does not
  resolve it, and simulator metadata preserves the binding through gossip/sync;
- `verdict`/`sig_valid` MUST NOT become a wire or production record field.

Identifiers MUST use deterministic ordering and MUST reject non-canonical or
oversized encodings.

### P1.2 — State and subsystem interfaces

Owner: coordinator.

Freeze state partitions and narrow functions:

```text
clock.observe(ctx, persisted) -> EffectiveClock | ClockUncertain
ingest(op, retained) -> IngestOutcome
authority.publish_allowed(op, authority_view) -> verdict
projection.live_claims(retained, proof_view, effwall) -> iterator
routing.resolve(query, projected) -> Matched | NoClaim
claims.on_wakeup(...) -> effects
append.on_accepted(...) -> effects
sync.on_message(...) -> effects
```

The exact Rust signatures may use borrowed views, but module ownership and
direction MUST remain stable.

### P1.3 — RED all-scenario test

Owner: coordinator, with simulator agent review.

- Add the strict typed `Scenario` decoder.
- Freeze `event_budget: NonZeroU64` and
  `stop: AtMs{at_ms} | QuiescentForMs{quiet_ms,deadline_ms}` as required
  scenario data. The runner
  implementation comes later, but no scenario may rely on an implicit horizon.
- Freeze `WakeClass::{OneShot,Periodic}` in `WakeToken`. Quiescence means no
  pending/in-flight **non-periodic** work and no state/effect change caused by a
  periodic tick for the requested interval. Periodic ticks continue to fire;
  their mere rescheduling does not reset the interval, but any work or state
  change they produce does. This prevents recurring gossip ticks from making
  quiescence impossible without allowing unconverged sync work to terminate.
- Freeze the simulator verifier table/fixtures needed by forged-signer,
  wrong-owner, and unauthorized-revoker scenarios.
- Add a root test that discovers every scenario data file.
- Initially fail with a clear “runner not implemented” result.
- Reject unknown fields, invalid fault parameters, duplicate ids, missing
  expectations, invalid time intervals, duplicate fixture keys, unresolved
  input selectors, and structurally invalid minted selectors. A syntactically
  valid minted selector that binds no persisted signed op is rejected by the
  runner's scenario-completion validation.
- Freeze per-node typed clock schedules and typed restart watermark outcomes;
  link faults MUST NOT carry clock behavior or an untyped generic parameter.
- End-to-end scenarios MUST NOT expect internal ingest verdicts. Direct
  ingest/corpus tests own the exact rejection classification.

Exit gate: all crates compile; unit scaffolding is green; strict scenario data
decodes; the explicitly marked scenario acceptance target is RED with the
single expected “runner not implemented” reason. P1 is complete and landed
before any P2 lane begins.

## P2 — Parallel RED foundations

Milestone: independent failing tests fully describe the first implementation
wave.

All four lanes start together after P1.

### P2A — Taut schema and codec corpus

Owner paths: `protocol/`, protocol crate `codec.rs`, protocol corpus tests.

Write failing tests first for:

- round-trip of every frozen message and record;
- canonical byte equality across field order variants;
- stable `GrantId`/`ClaimId` record identity;
- malformed, unknown-version, missing-field, oversized, and trailing-data
  rejection;
- `StreamHead {share,glade_id,key,origin,seq,hash}` exactness;
- signature/envelope fields being covered by the canonical payload presented to
  the external signer.

Deliverable: taut source, hostile corpus, and RED/then-green generated binding
conformance. No kernel code.

### P2B — Deterministic queue and fault model

Owner paths: simulator `schema.rs`, `queue.rs`, `network.rs`, `faults.rs` and
their tests.

Write failing tests first for:

- `(time, insertion-sequence)` input ordering;
- `(at_mono, token)` wakeup ordering;
- effect `msg-ref = (producing-event-id, emission-index)`;
- frozen SplitMix64 vectors and draw order;
- latency, loss, partition, reorder, duplicate, drift, and per-node wall-offset;
- restart events preserving or corrupting the modeled watermark as declared;
- explicit input ids plus input/minted verifier-selector resolution;
- reader-specific fixture precedence, duplicate rejection, valid-by-default
  runtime minting, and unmatched-minted-selector failure;
- `AtMs` termination and periodic-tick-aware `QuiescentForMs` semantics;
- byte-identical event logs across repeated runs with the same seed.

Clock offset/drift schedules belong to node specifications. Restart data MUST
use `Preserve | Unreadable | Replace{value}` watermark outcomes. Link faults are
a strict tagged union of loss, partition, reorder, duplicate, and latency.

No test may call a real clock, sleep, socket, filesystem, or random source.

### P2C — Scenario corpus, wave 1

Owner paths: `scenarios/authz`, `scenarios/clock`, `scenarios/routing`, and
`scenarios/claims`.

Author concrete data for:

- `s-disc-authz-boundary`;
- `s-disc-inst-authority`, `-forged-node`, `-wrong-def`, `-revoked-exec`;
- `s-disc-wall-rollback`, `-restart-uncertain`, `-skew`;
- `s-disc-epoch-tie`, `-no-ping-pong`;
- `s-disc-corr-collision`, `-route-terminal`.

Runtime-mint scenarios MUST address verifier overrides with the frozen
`Minted {node,slot,generation,append_ordinal}` selector and consider it fulfilled
only after canonical signed bytes are persisted. Scenarios relying on
the default-valid rule MUST still assert the expected signer/origin binding so
the default cannot conceal an authority error.

Each scenario MUST declare observable effects and final state, not merely “did
not crash.” In P2 it MUST decode as valid typed data and fail only with the
expected “runner not implemented” result. Failure for the scenario's named
invariant becomes mandatory once P2.5/P4 provide the executable path and oracle;
P2 does not pretend a missing runner proves an invariant test is meaningful.

### P2D — Core unit-test shells

Owner: coordinator during W1b. Owner paths: core test directories only; no
production implementations. This balances W1b while the other agents own the
protocol, simulator foundation, and scenario-data lanes.

Write focused failing tests for:

- effective-wall monotonicity and checked arithmetic;
- readable and unknown-floor clock uncertainty plus trusted reseed;
- three-stage ingest verdict separation;
- owner and derived-service authority predicates;
- authorized-before-specificity;
- `(epoch, stable claim_id)` order across renewals;
- append-before-gossip ordering;
- retained-byte ceiling and fail-closed storage exhaustion;
- sync round correlation, timeout, retry, and terminal completion;
- exact one-reply route behavior;
- direct purity: invoking `step` twice with byte-identical state, context, and
  event produces byte-identical state/effects.

P2 exit gate: corpus and simulator foundation tests are green where their
generic machinery exists; all kernel/scenario behavior tests are RED for known,
documented reasons. There MUST be no test that passes through a stubbed
allow-all verdict.

## P2.5 — Walking skeleton before subsystem fan-out

Milestone: one real typed scenario executes through the frozen API and generic
runner, exposing contract/integration mistakes before four agents build on it.

Owner: coordinator/integrator. P3 MUST NOT start until this is green.

Implement the smallest end-to-end path for `s-disc-route-terminal`:

1. load one strict scenario data file;
2. seed a structurally valid claim **and its valid authorization proof**;
3. deliver the signed `DirOp` through ingest into retained state;
4. project and resolve one `Route`;
5. emit and observe `Reply { ingress, corr, Matched { node } }`;
6. assert the effect through the generic runner/oracle path.

The slice MUST use the real frozen `Event`, `Effect`, state, protocol, scenario,
and runner types. It MAY implement only the happy-path behavior needed for this
scenario; every shortcut MUST be replaced by an explicit `NotImplemented`
branch that cannot satisfy later scenarios. It MUST NOT use an allow-all
authority stub or scenario-name branch.

Walking-skeleton TDD sequence:

- scenario first decodes and fails because the runner/path is absent;
- minimal runner then makes it fail at missing ingest;
- minimal ingest makes it fail at missing projection/routing;
- minimal routing/reply makes the scenario green.

Exit gate: `s-disc-route-terminal` is green end-to-end. A deliberately invalid
authorization sibling MUST NOT produce `Matched`; it may remain an explicit
fail-closed/`NotImplemented` case until its owning lane. Public-contract defects
discovered here are resolved before P3 ownership splits. At P3 start, P3A/P3B/
P3C/P3D inherit full ownership of their modules, including walking-skeleton code
and `NotImplemented` branches; the coordinator stops editing those modules.
At P4 start, the simulator agent similarly inherits all `runner.rs` skeleton
code.

## P3 — Parallel kernel implementation

Milestone: each pure subsystem passes its own tests behind the P1 contracts.

Four implementation lanes run concurrently. Each lane writes a new failing edge
case before every production change.

### P3A — Clock and claim lifecycle

Owner paths: `clock.rs`, `claims.rs`, their tests, assigned clock/claim scenarios.

Implement:

- persisted effective-wall watermark projection;
- `Ready | Uncertain{floor}` clock state, recovery threshold, and trusted reseed;
- checked wall/monotonic deadline conversion;
- skew and maximum-lease rejection;
- renewal preserving epoch and stable `claim_id`;
- explicit authorized takeover only;
- stale generation wakeup no-op;
- losing global-instance teardown effect/state.

Edge tests: backward and forward wall jumps, unreadable watermark, restart,
integer limits, late renewal after expiry, and equal-epoch claimant renewal.

### P3B — Ingest, authority, retained fold, and projection

Owner paths: `ingest.rs`, `authority.rs`, `projection.rs`, their tests, assigned
authz/ingest scenarios.

Implement:

- structural/envelope verdicts;
- governance publish authorization;
- owner-root proof and authorized-revoker predicate;
- derived-service `(def_ref, compute_key)` execution scope;
- signer/origin/grant-principal/node binding equality from explicit
  `KernelConfig` authority roots and node-principal bindings;
- retained time-free set union;
- durable pending-proof index semantics;
- grant/revocation arrival in either order;
- expired and live-authorized projection before specificity.
- canonical retained-byte accounting and `StorageExhausted` rejection before
  fold mutation.

Crypto verification MUST arrive as explicit local non-wire verification data;
no hidden callback, key, or crypto I/O enters the core.

### P3C — Routing and winner selection

Owner paths: `routing.rs`, its tests, assigned routing scenarios.

Implement:

- exact advertisement slot matching;
- specificity order and scoped precedence;
- stable total order `(epoch, claim_id)`;
- authz-blind `Matched | NoClaim` result only;
- direct `Reply { ingress, corr, ans }` effect;
- no pending or completed route map; every delivered `Route` resolves current
  state because the trusted ingress owns duplicate suppression.

The kernel MUST NOT expose definition identity, run INV-7, instantiate, dial, or
forward a subscription.

### P3D — Sync and append lifecycle

Owner paths: `sync.rs`, `append.rs`, their tests, assigned sync/append scenarios.

Implement:

- `ClaimDraft` append intent creation and `(slot,generation,intent)` idempotency;
- `OpAccepted` validation, stale-generation rejection, local indexing, then
  gossip of the same exact accepted canonical bytes;
- `SyncStart`, chunked `SyncOps`, and correlated `SyncEnd`;
- per-`SyncId` progress/dedup state;
- chain-gap/suffix rejection and next-round repair;
- timeout schedule, retry budget, stale-peer result, next-tick retry;
- gossip tick bounded to configured peer fan.

P3 exit gate: all core unit tests pass; lanes have not edited each other's files;
the coordinator integrates module dispatch and runs workspace checks. Individual
lanes MUST NOT claim INV-D2, INV-D3, or INV-D6 closed: those are cross-lane
properties owned by the P4 coordinator/oracle gate.

## P4 — Simulator runner and complete scenario corpus

Milestone: all frozen semantics execute end-to-end as data.

### P4.1 — Generic runner integration

Owner: simulator agent; coordinator owns only cross-crate wiring.

Implement the event loop that:

- runs N independent kernel states;
- samples each node's wall and monotonic context;
- delivers inputs, messages, wakeups, restarts, and accepted-op callbacks;
- interprets `Gossip`, `Append`, `Reply`, and `Schedule` generically;
- interprets `Teardown` as a local service-manager effect;
- models signing/persistence as environment behavior;
- records stable effects and state observations;
- stops deterministically at `AtMs`, or at `QuiescentForMs` using the frozen
  periodic-tick rule: ticks keep firing, rescheduling alone is ignored, and any
  resulting work/state change resets the quiescence interval; deadline produces
  `DidNotQuiesce`, and `event_budget` stops zero-time loops.

### P4.2 — Invariant oracle

Owner: coordinator/oracle integration owner. Owner paths: simulator `oracle.rs`
and its tests.

Implement generic assertions for:

- INV-D0 deterministic replay;
- INV-D1 no lease resurrection;
- INV-D2 authorization before specificity;
- INV-D3 no post-heal claim oscillation;
- INV-D5 gossip strictly after accepted persistence;
- INV-D6 convergence after connected delivery resumes.

INV-D2 (projection + routing), INV-D3 (claims + routing), and INV-D6 (ingest +
sync) are explicit P4 integration-gate assertions. Their component tests are
necessary but cannot close the joint invariant.

INV-D4 belongs to the trusted node wrapper, not the authz-blind discovery core.
The standalone simulator MUST assert that discovery returns only internal
`Matched|NoClaim`; P6 owns the node-level no-leak proof.

### P4.3 — Scenario corpus, wave 2

Run three agents in parallel on disjoint directories while the coordinator
integrates the runner:

- Agent A, ingest/authority: `s-disc-owner-proof`, `-unauth-revoke`, `-regrant`,
  `-proof-late`, `-revoke-then-grant`.
- Agent B, sync/append: `s-disc-sync-round`, `-sync-drop`, `-sync-retry`,
  `-append-restart`, `-delayed-sign`.
- Agent C, routing/bounds: `s-disc-noclaim-handoff`, `-dos-bound`, plus hostile
  duplicate/collision variants.

P4 exit gate: every scenario named by v3.1 exists as concrete data and is green;
running the suite twice produces byte-identical replay logs.

## P5 — Hardening and adversarial expansion

Milestone: invariants hold beyond curated examples.

Implementation status: complete. Deterministic property/model tests, authority
mutation matrices, byte/schema mutation smoke, reusable libFuzzer targets,
fault-seed sweeps, and append restart-cut sweeps are executable in the paths
named by `RequirementTrace.md`.

Parallel lanes:

- property tests for set-union convergence under arbitrary record order;
- property tests for total-order winner agreement;
- model-based tests for sync round state transitions;
- fuzz targets for protocol and scenario decoders;
- mutation/negative tests proving each authority conjunct is necessary;
- resource-bound tests at, below, and above every configured limit;
- retained-byte ceiling tests proving bounded growth and fail-closed behavior;
- seed sweep over loss/reorder/dup/partition/heal schedules;
- restart sweep at every append lifecycle transition.

Failures MUST be minimized to a deterministic seed and promoted to a permanent
scenario or regression test before being fixed.

Exit gate:

- no panics on hostile bytes or scenarios;
- no nondeterministic replay failures across a declared seed range;
- all configured bounds have success and rejection tests;
- clippy, formatting, docs, unit, corpus, scenario, property, and fuzz smoke
  gates pass.

## P6 — Node adapter and trusted-boundary proof

Milestone: the pure component integrates without absorbing node responsibilities.

Implementation status: complete. Injected append, clock, ingress, transport,
and durable-driver boundaries are exported by the node-adapter crate. The
vertical integration test proves commit-before-effect ordering from advertise
through exact-byte gossip.

This phase begins only after P5. It MAY proceed as four parallel adapter lanes:

- append adapter: allocate seq/prev, sign, persist, return `OpAccepted`;
- clock adapter: persist/read watermark and map real monotonic timers;
- ingress adapter: B3 context + opaque `IngressId`, consume internal route
  result, suppress/replay duplicate `(IngressId,Corr)`, and run authorization
  before exposure;
- transport adapter: map node delivery to `Deliver` and `Gossip` effects.

The coordinator owns the final adapter composition.

Required integration tests:

- restart between `Append` and `OpAccepted` is idempotent;
- durable state is atomically committed before any dependent effect executes;
- exact persisted op bytes are the gossiped bytes;
- unreadable/backward watermark enters clock-uncertain and publishes no claim;
- unauthorized caller receives no node id, definition id, or instantiation;
- authorized `NoClaim` alone reaches the service-manager definition matcher;
- INV-7 is enforced outside discovery before any external response/action;
- transport loss never changes the pure fold semantics.

The adapter MUST NOT expose node internals to the core crate.

## P7 — Final review and delivery

The next formal review evaluates code plus green scenarios, as required by v3.1.

Review inputs:

- v3.1 semantic freeze and local decision log;
- requirement trace with file/test links;
- protocol corpus and generated bindings;
- all concrete `s-disc-*` data;
- deterministic replay artifacts for selected adversarial seeds;
- coverage report for INV-D0..D6;
- node-adapter boundary tests;
- known residual risks and deferred compaction/locality work.

Definition of done:

- all v3.1 scenarios exist and pass;
- every invariant has at least one direct negative test and one end-to-end
  scenario/property test;
- all affected checks pass with warnings denied;
- no scenario-specific implementation branches exist;
- no ambient I/O, clock, randomness, or crypto key enters the core;
- docs and traceability describe the shipped behavior;
- compaction and locality remain explicitly deferred, not partially invented.

## 7. Four-agent execution schedule

| Wave | Coordinator | Agent 2 | Agent 3 | Agent 4 | Gate |
| --- | --- | --- | --- | --- | --- |
| W0 | P0 design import + skeleton | idle/review | idle/review | idle/review | skeleton green |
| W1a | P1 public contracts + compile gate | contract review only | scenario-schema review only | protocol-type review only | contracts landed/frozen |
| W1b | P2D core RED test shells | P2A protocol RED corpus | P2B sim RED foundation | P2C typed scenario data | intended RED suite |
| W2a | P2.5 walking skeleton | adversarial review | fixture review | runner/oracle review | one green vertical slice |
| W2b | integrate dispatch | P3A clock/claims | P3B ingest/authority/projection | P3C routing; then P3D sync/append as slot frees | core unit suite green |
| W3 | P4 runner integration | P4 scenario authority lane | P4 scenario sync/append lane | P4 scenario routing/bounds lane | all v3.1 scenarios green |
| W4 | hardening integration | convergence/order properties | sync model + decoder fuzz | seed/restart/bounds sweep | adversarial gates green |
| W5 | adapter composition | append/clock adapters | ingress/authz adapter | transport adapter | integration gates green |
| W6 | review package | independent correctness review | security review | reproducibility/ops review | review disposition complete |

P3D is sizeable. If more than four concurrent agents are available, split it
immediately into append and sync lanes. With four slots, routing is expected to
finish first and its agent takes append while the fourth agent retains sync.

## 8. Merge and handoff checklist for every work package

An agent handoff MUST include:

- work package id and owned paths;
- failing test/scenario observed before implementation;
- implementation summary;
- exact verification commands and results;
- requirements/invariants covered;
- public-contract change requests, if any;
- residual risks or deliberately unhandled cases;
- clean `git status` limited to owned files or a commit hash.

The coordinator MUST reject a handoff that changes frozen semantics, edits
unowned shared files, lacks the prior failing test, or passes by weakening an
assertion.

## 9. Standard verification commands

The skeleton phase MUST make these the stable repo commands:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo test --locked -p glade-discover-protocol --test corpus
cargo test --locked -p glade-discover-sim --test all_scenarios
cargo test --locked -p glade-discover-sim --test deterministic_replay
```

Fuzz/property commands MAY be separate because of runtime cost, but CI MUST run
at least a bounded smoke profile. Full seed sweeps belong to the review gate.

## 10. Explicitly deferred work

The following MUST NOT leak into v1 implementation tasks:

- chain-prefix compaction/checkpoints;
- locality-aware rendezvous and `ReplicaHint`;
- observability supplier surface;
- HLC replacement for bounded wall/skew semantics;
- embedded transport, signing keys, or filesystem persistence in the core;
- definition matching or service instantiation inside discovery.

Each deferred item requires its own decision and tests before implementation.
