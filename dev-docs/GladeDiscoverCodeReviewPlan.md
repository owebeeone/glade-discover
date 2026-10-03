# Glade Discover Code-Review Remediation Plan

Status: ready for execution
Date: 2026-07-18
Baseline: commit `65fc18b`, tag `glade/discover-impl-stage1`
Input review: `dev-docs/GladeDiscoverCode-ReviewF5.md`

## 1. Objective

Close CQ-01 through CQ-13 without weakening the v3.1 semantic freeze. This is
an implementation-hardening phase, not a benchmark-and-defer exercise.
Unbounded local state, nested retained-set scans, and whole-state persistence
amplification MUST be removed before the implementation is treated as suitable
for a long-running or loaded node.

The work remains TDD-first:

1. every defect MUST first have a failing regression or structural-cost test;
2. production code MUST make that test pass with the smallest coherent change;
3. refactoring MUST occur only with the affected suite green;
4. behavior or contract changes MUST be recorded in `Decisions.md` before code;
5. every lane MUST finish with current-Rust and Rust 1.85 warnings denied.

## 2. Mandatory closure decisions

The coordinator owns these decisions. Parallel implementation MUST NOT begin
until they are recorded.

### CR-D01 — bounded local claim bookkeeping

`PersistedState.mine` MUST have a finite slot-relative bound. The target
invariant is at most one newest `Pending` entry and one newest terminal
(`Accepted` or `Lost`) entry per slot. A newer command supersedes an older
pending generation; accepting a generation removes older terminal history that
is no longer required for recovery.

Pruning MUST preserve:

- append retry for the newest pending intent;
- renewal validation against the currently accepted claim;
- restart recovery for a losing accepted service;
- stale-generation and stale-callback rejection;
- claim-id and epoch continuity across renewal.

### CR-D02 — teardown coverage

The host contract MUST state whether teardown is slot-level or generation-level.
The conservative implementation target is generation-level: every distinct
losing accepted generation present during legacy-state reconciliation emits one
durable `Teardown{slot,generation}`. After CR-D01 normalization, ordinary state
remains bounded and normally contains only one relevant accepted generation.

### CR-D03 — bounded sync-round bookkeeping

`State.sync` MUST be bounded by configured peers, not process uptime. A peer MAY
have one active round and one most-recent terminal observation. Starting a new
round MUST supersede/evict older state for that peer; completing or exhausting a
round MUST evict older terminal entries. Recently completed/stale state remains
observable so existing round-status contracts and tests are not silently
removed.

### CR-D04 — scalable durable mutation contract

CQ-06 is not closed by noting that a future store could diff snapshots. The
adapter MUST define an incremental authenticated transition/checkpoint contract,
and at least one durable implementation or integration with the Glade durable
store MUST demonstrate it.

The contract MUST provide:

- authenticated load of one checkpoint plus ordered mutations;
- create-if-absent and revision compare-and-swap;
- atomic state mutation plus outbox enqueue before effect execution;
- atomic acknowledgement of one outbox id;
- bounded journal growth through authenticated checkpoint replacement;
- crash recovery at every checkpoint/mutation/acknowledgement boundary;
- format/version migration and tamper-failure behavior.

An in-memory mock alone MUST NOT close CQ-06. If the wider Glade workspace has
no selected durable engine, this lane first produces a short storage decision
covering engine, keying, canonical encoding, authentication, fsync/transaction
semantics, and error mapping, then implements that decision.

## 3. Finding-to-work map

| Finding | Required change | Primary executable evidence | Closure |
| --- | --- | --- | --- |
| CQ-01 | Normalize `mine` on command acceptance, append acceptance, loss, and restore. | 100,000-renewal and superseded-pending tests; restart and stale-callback regressions. | `mine` cardinality is bounded by slots, independent of renewal count. |
| CQ-02 | Introduce a deterministic derived fold/index view for decoded records, grants, authorized revocations, claims, and op hashes; rebuild on restore and maintain on fold. | Indexed-vs-reference property tests and structural decode/scan counters at the retained ceiling. | No claim-by-record nested scans; record decode/hash work is bounded per fold/projection. |
| CQ-03 | Enforce the per-peer active/terminal retention rule in sync transitions. | 100,000 completed/failed rounds and late-message/timeout regressions. | Round-map cardinality is bounded by peer count. |
| CQ-04 | Make `live_grant` skip an undecodable retained payload consistently with sibling scanners. | Corrupt/version-skewed retained-record regression before and after a valid grant. | One bad payload cannot erase unrelated authorization. |
| CQ-05 | Add one protocol-owned derived-service namespace constant and replace every source literal. | Protocol/core constant-use tests and zero non-test literal scan. | Security-relevant namespace has one definition. |
| CQ-06 | Implement CR-D04 and stop requiring a full retained snapshot rewrite for an ordinary transition. | Byte-accounted durable-store tests, checkpoint+mutation replay equivalence, tamper and crash matrix. | Post-checkpoint append/ack cost is proportional to its mutation; journal size is bounded. |
| CQ-07 | Add deterministic per-node wake-delay/late-timer faults and correct monotonic-to-simulation scheduling. | On-time, delayed-renewal, lease-loss, restart, and deterministic-replay scenarios. | Simulator can express missed renewal windows without abusing wall offset. |
| CQ-08 | Implement CR-D02 together with CR-D01 normalization. | Multiple legacy losing generations and repeated-delivery idempotency tests. | Every contractually distinct running generation is covered exactly as specified. |
| CQ-09 | Replace fixed peer-prefix eager gossip with deterministic rotation. | Distribution test across all peers plus identical replay test. | Every configured peer is selected over a bounded sequence when fan is non-zero. |
| CQ-10 | Remove stale module-wide dead-code allowances; delete or integrate dead queue/error items. | Clippy with warnings denied and dead-code search. | No blanket allowance hides production dead code. |
| CQ-11 | Move imports, extract duplicated runner execution, and split oversized modules along cohesive boundaries. | Existing tests unchanged; file-level dependency and formatting checks. | No semantic changes; reviewability improves without new blanket allows. |
| CQ-12 | Publish a poisoned-watermark recovery runbook and define the canonical persistence seam. | Runbook test/fixture for authenticated administrative reset to `Unreadable`, then explicit reseed. | Recovery does not require unauthenticated snapshot editing. |
| CQ-13 | Document single-use correlations, cached denial behavior, ingress closure, and admission-cap recovery. | Ingress lifecycle examples and existing bounded-cache tests. | Host obligations are explicit at the public adapter boundary. |

## 4. Structural scalability gates

Wall-clock benchmarks MAY supplement these gates but MUST NOT replace them.
CI closure uses deterministic cardinality and operation-count assertions.

### SG-01 — claim cardinality

After 100,000 successful renewals of one slot, `mine.len()` MUST remain within
the CR-D01 bound. Alternating delayed acceptance, rejection, takeover, and loss
MUST NOT increase the bound. Restoring the resulting state MUST emit all and
only required recovery effects.

### SG-02 — sync cardinality

After 100,000 terminal rounds across `P` peers, `sync.len()` MUST be O(P) under
the exact CR-D03 bound. Partition, absent `SyncEnd`, retry exhaustion, late
`SyncOps`, and late timeout delivery MUST preserve it.

### SG-03 — fold/projection work

For `R` retained records and `C` projected claims:

- derived-index rebuild MUST decode each retained payload at most once;
- one projection MUST NOT decode retained payloads or scan all records per
  claim;
- revocation evaluation MUST be O(R + C), not O(R × C);
- index results MUST equal a simple test-only reference fold under record-order
  permutations, grant/revoke arrival order, and corrupt payloads;
- per-op hashes MUST be computed once per retained entry lifecycle, not once per
  lookup.

### SG-04 — durable write amplification

For a checkpoint containing a near-16-MiB retained set:

- appending one bounded record MUST write one bounded mutation plus fixed
  framing, not the checkpoint body;
- acknowledging one effect MUST write one bounded acknowledgement mutation;
- recovery from checkpoint plus mutations MUST reproduce the same logical
  `DurableSnapshot` and outbox accounting;
- checkpoint replacement MUST be atomic and MUST bound accumulated journal
  bytes under a configured policy;
- tampering, truncation, revision gaps, and duplicate revisions MUST fail
  closed before effects execute.

## 5. Parallel execution waves

At most one lane owns a source file at a time. Shared contracts, public types,
`Decisions.md`, and final integration remain coordinator-owned.

| Wave | Coordinator | Claims lane | Sync/index lane | Adapter/simulator lane | Gate |
| --- | --- | --- | --- | --- | --- |
| W0 decisions | Record CR-D01..D04; freeze public changes | Review claim/teardown invariants | Review round/index invariants | Review durable-store and timer contracts | Decisions accepted; no production edits |
| W1 RED | Integrate shared test utilities | CQ-01/CQ-04/CQ-08 failing tests | CQ-02/CQ-03 structural failing tests | CQ-06 failing mutation/crash tests | Intended RED failures demonstrated |
| W2 bounded core | CQ-05 shared constant and merge control | Bound `mine`, fix scan abort, teardown coverage | Bound rounds; build and verify derived index | Durable-store design/API implementation begins | CQ-01..CQ-05/CQ-08 green; SG-01..03 green |
| W3 persistence/sim | Cross-crate integration and trace updates | Adversarial lifecycle review | Index/reference differential sweep | Finish CQ-06; implement CQ-07/CQ-09 | SG-04 and simulator scenarios green |
| W4 hygiene/docs | CQ-10/CQ-11 integration | Claim-code review | Core performance review | CQ-12/CQ-13 runbooks and adapter docs | All CQ rows have evidence |
| W5 independent review | Full gates and disposition | Read-only lifecycle audit | Read-only asymptotic audit | Read-only crash/security audit | No blocker; clean review package |

CQ-02 and CQ-06 are integration-critical. Their lanes MUST land before layout
splits so performance work is not obscured by mechanical file movement.

## 6. Required verification

The final gate MUST include:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo +1.85.0 fmt --all -- --check
cargo +1.85.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.85.0 test --locked --workspace
cargo +nightly-2026-02-28 check --locked --manifest-path fuzz/Cargo.toml --bins
./scripts/check-design-snapshot.sh
./scripts/check-requirement-trace.sh
git diff --check
```

The review package MUST also contain:

- SG-01 through SG-04 results;
- an updated requirement trace and decisions log;
- a CQ-01..CQ-13 disposition table with executable evidence;
- the durable-store format/API decision and crash matrix;
- confirmation that no ignored tests, new blanket allowances, TODOs, or
  scenario-specific implementation branches were introduced;
- an independent correctness, asymptotic, and crash-safety re-review.

## 7. Completion rule

This remediation is complete only when every CQ finding has an implemented or
explicitly inapplicable disposition backed by evidence. CQ-01, CQ-02, CQ-03,
and CQ-06 MUST NOT close as documentation-only, benchmark-only, or deferred
work. A green semantic scenario suite without SG-01 through SG-04 is
insufficient.
