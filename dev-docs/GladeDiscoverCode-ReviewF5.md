# Review F5 — glade-discover implementation quality (code review)

Reviewer: Claude Fable 5 (authored the v3 freeze + plan reviews F5/F5-2 —
adversarial, not independent). Date: 2026-07-18. Subject: commit `65fc18b`
(tag `glade/discover-impl-stage1`), all four crates, full source read
(~9.0k LOC src), checked line-by-line against the vendored v3.1 freeze
(§1 API, §2 clocks, §6 order, §8 append, §10 invariants). Gates re-run
before review: fmt, clippy `-D warnings`, 211 tests green on 1.96.0 and on
the 1.85.0 MSRV pin, snapshot + trace scripts, fuzz nightly compile.

## Verdict

This is a high-quality, freeze-faithful implementation. I found **no
violation of the frozen semantics** — every §6/§8/§10 rule I traced exists
in code where the design says it should. The real findings are of a
different class: **two unbounded-growth defects** a short-scenario suite
cannot see (local claim bookkeeping grows per renewal forever; sync-round
bookkeeping grows per gossip tick forever), one **scan-abort inconsistency**
that turns a single undecodable retained record into total authorization
loss, and a family of **at-the-frozen-bound algorithmic costs** (full
re-decode/re-scan per event, whole-snapshot rewrite per event) that are
fine for stage 1 and simulator scale but must be re-visited before any
long-running or loaded node. Plus hygiene items. Ranked below.

## What held (verified, keep)

- **Purity discipline is complete.** Core state is BTreeMap/BTreeSet
  throughout (no HashMap iteration nondeterminism anywhere), `unsafe_code =
  "forbid"` workspace-wide, core depends only on protocol (+sha2), no
  time/rand/io in the kernel, arithmetic is checked/saturating pervasively,
  and I found no reachable panic on hostile input (the few `expect`s are
  genuinely unreachable, e.g. usize→u64 on 64-bit).
- **The codec is fail-closed for real** ([codec.rs](../crates/glade-discover-protocol/src/codec.rs)):
  canonical CBOR with minimal-length integer enforcement, exact map
  arities, ascending field keys, trailing-data rejection, bounds on every
  dimension (record 16 KiB, message 1 MiB, key 4 KiB, sync-id 256, SyncOps
  ≤256, verbs ≤3 strict-sorted), and encode paths that re-decode their own
  output so encoder drift cannot ship. B2 as designed.
- **Frozen semantics are traceable line-for-line**: `(epoch, claim_id)` max
  in both winner reconciliation ([model.rs:508](../crates/glade-discover-core/src/model.rs))
  and routing ([routing.rs:10](../crates/glade-discover-core/src/routing.rs));
  renewal reuses epoch+claim_id with strictly-greater expiry and identical
  lineage; takeover requires owner-rooted `Takeover` scope, exact
  `supersedes` epoch+1, checked issuer per plane; `ClaimRenew` wakeup inert
  per §6; effective wall = max(watermark, ctx.wall) monotone; deferred
  claims revalidated (and violators permanently removed + de-accounted) on
  Uncertain→Ready; retain-then-project three-layer ingest with the
  pending-proof index and lenient-publish/exact-effect revocation handling
  (the revoke-then-grant ordering works because retention is permissive and
  effect is checked against the grant's actual share owner).
- **INV-D5 is visible in code**: `Effect::Gossip` for own ops is emitted
  only in `append::on_accepted` after `ingest` folds the op; the sync
  responder serves only retained ops; the driver executes effects only from
  the durable outbox after the snapshot commit.
- **The driver implements the crash-safety closure faithfully**
  ([driver.rs](../crates/glade-discover-node-adapter/src/driver.rs)):
  whole-snapshot commit with revision CAS *before* any effect executes,
  per-effect acknowledge, restore that re-validates byte accounting and
  outbox invariants, a sync-capacity floor derived from config vs actual
  retained bytes, stale-safe schedule rebase, and a capacity-drain recovery
  loop with honest `RecoveryBlocked` failure.
- **Ingress enforces INV-7 ordering** ([ingress.rs](../crates/glade-discover-node-adapter/src/ingress.rs)):
  authorize before resolve, replays re-authorize under the original source
  closure, candidate closure must equal evidence closure, Denied cached
  without closure (no leak channel), admission failures never cached.
- **The simulator is oracle-grade**: exact-count effect matching,
  per-event `Throughout` windows, seed grants validated by byte-exact fold,
  the minted-op selector registry + default-valid rule + reader-specific >
  global precedence + duplicate-fixture and duplicate-input-id decode
  rejection (PF5-2-01/02 fully landed, with unit tests), quiescence that
  excludes self-rescheduling periodic ticks by checking the tick was a pure
  self-reschedule (PF5-2-04, done precisely), rng draws consumed even for
  inactive faults so editing a fault set does not shift the stream, and
  duplicate-fault expansion budget-gated before allocation.

## Findings (ranked)

### CQ-01 — HIGH — `mine` grows without bound across renewals

- `PersistedState.mine` ([model.rs:82](../crates/glade-discover-core/src/model.rs))
  gains one `(slot, generation)` entry per accepted advertise
  ([claims.rs:260-268](../crates/glade-discover-core/src/claims.rs)) — and a
  renewal is a new generation. Nothing ever removes an entry (verified: no
  `mine.remove`/`retain` in any src). `on_slot_winner` only flips losing
  entries to `Lost`; **winning** generations accumulate forever: a claim
  renewed every 30s adds ~2880 entries/day/slot.
- Compounding: `mine` is inside `PersistedState`, so every entry is
  re-serialized into every `DurableSnapshot` on **every event** (CQ-06),
  and `latest_generation` / `latest_own_claim` / `on_accepted`'s
  max-generation guard are O(|mine|) scans — cost grows linearly with
  uptime.
- This is **not** covered by the frozen compaction deferral: that deferral
  is about the replicated record set; `mine` is local implementation
  bookkeeping the freeze does not require to be append-only.
- **Fix (pure, local):** on acceptance of generation N for a slot, drop
  that slot's non-`Pending` entries with generation < N (keep the latest
  Accepted and any Pending). One function in claims.rs; no wire or freeze
  impact.

### CQ-02 — HIGH at the frozen bound — full re-decode/re-scan per event, quadratic in projection

- Every `step` ends in `reconcile_service_winners`
  ([model.rs:454](../crates/glade-discover-core/src/model.rs)) →
  `projection::live_claims` decodes **every retained op**; then for **each**
  candidate claim `revocation_is_authorized`
  ([projection.rs:66,96](../crates/glade-discover-core/src/projection.rs) →
  [authority.rs:81](../crates/glade-discover-core/src/authority.rs))
  re-scans all retained ops — O(claims × records) decodes per event.
  `live_grant`, `claim_epoch`, `finalized_claim_id`, `referenced_grant` are
  each full scans per lookup, and ingest calls two of them per op. `op_hash`
  recomputes SHA-256 from canonical bytes on every call (per retained op
  per `SyncStart` in `missing_ops`, per chain check).
- At the frozen 16 MiB retained bound this is prohibitive; at sim scale it
  is invisible, which is exactly why it needs a written flag now.
- The **shape is deliberate and good** (fold-from-retained, no cache
  invalidation bugs) — the fix is a *derived index*, not a semantic change:
  decoded records + grant/revocation maps + per-op hash memoized beside
  `retained` (rebuilt on restore, maintained on fold — still pure and
  deterministic). Cheapest immediate win: build the revocation set once per
  projection instead of per claim.

### CQ-03 — MEDIUM — completed/stale sync rounds are never evicted

- `State.sync` gains an entry per gossip tick per peer (`start_round`) and
  `finish_round`/`retry_round`
  ([sync.rs:316-327](../crates/glade-discover-core/src/sync.rs)) only ever
  overwrite to `Complete`/`Stale` — nothing removes (verified). In-memory
  only (reset on restart), but a daemon gossiping every few seconds grows
  this monotonically for its whole uptime.
- Removing a round on `Complete`/`Stale` is behavior-identical today: late
  `SyncOps` require an `Active` entry either way, and a late `SyncEnd`
  no-ops either way. Evict on completion; if a tombstone window is wanted
  for diagnostics, cap it.

### CQ-04 — MEDIUM — `live_grant` aborts the whole scan on one undecodable record

- [claims.rs:375](../crates/glade-discover-core/src/claims.rs): inside the
  `for` loop, `decode_directory_record(...).ok()?` early-returns **None
  from the function** on the first retained op that fails to decode — every
  grant lookup then fails, so every authorization on the node denies. The
  sibling scanners (`claim_epoch`, `finalized_claim_id`, `referenced_grant`,
  `rebuild_unresolved`) all skip-and-continue.
- Unreachable while ingest gates hold and the store is honest — but a
  version-skewed or corrupted retained record turns fail-closed into
  fail-everything, and the inconsistency between five sibling scanners is a
  defect regardless. Make `live_grant` skip like the others.

### CQ-05 — MEDIUM — the `"svc"` literal is scattered across three crates

- Seven occurrences in non-test source: the codec decode gate
  ([codec.rs:675](../crates/glade-discover-protocol/src/codec.rs)), claims
  authorization, authority publish gate, projection, and winner
  reconciliation. One `pub const` in protocol (e.g. `DERIVED_SERVICE_SHARE`)
  referenced everywhere; today a rename or a second derived plane is a
  scattered, missable edit of a security-relevant value.

### CQ-06 — MEDIUM — whole-snapshot-per-event commit model

- `commit_transition` ([driver.rs:559-592](../crates/glade-discover-node-adapter/src/driver.rs))
  serializes the **entire** `PersistedState` (all retained bytes + all of
  `mine`) through `DurableCommit::commit` on every event — at the frozen
  bound, a 16 MiB atomic write per folded DirOp. Correct, and the trait
  shape permits a diffing/log+checkpoint implementation, but nothing
  in-tree demonstrates one and the contract docs don't say incremental
  impls are expected. Acceptable stage-1; name it as the scaling seam so
  the first real deployment doesn't discover it in production.

### CQ-07 — MEDIUM — the simulator cannot express late timers

- `sample_clock` ([faults.rs:52-56](../crates/glade-discover-sim/src/faults.rs))
  models mono as exactly `initial + sim_ms`; drift and offsets apply to
  wall only, and `Schedule` wakeups fire punctually (the runner's inverse
  conversion [runner.rs:668-671](../crates/glade-discover-sim/src/runner.rs)
  is exact *only because of this*). A GC-paused/overloaded node missing its
  renewal window — the classic lease-loss cause — is inexpressible except
  partially via wall offsets. Adding a per-node wake-delay fault later will
  also require reworking that inverse conversion; note both together.

### CQ-08 — LOW/MEDIUM — one Teardown for many losing generations

- `on_slot_winner` ([claims.rs:136-164](../crates/glade-discover-core/src/claims.rs))
  marks **all** losing Accepted generations `Lost` but emits a single
  `Teardown{slot, generation=max}`. The `EffectExecutor` contract says
  Teardown is keyed `(slot, generation)` — an embedder that keys running
  instances per generation leaks the lower ones. Either emit one Teardown
  per losing generation or amend the contract to state teardown is
  slot-level (generation informational).

### CQ-09 — LOW — gossip fan is a fixed BTreeSet prefix

- [append.rs:118-122](../crates/glade-discover-core/src/append.rs) (core):
  `peers.iter().take(gossip_fan)` — always the same lexicographically-first
  peers; peers late in sort order never receive eager gossip and converge
  only via anti-entropy. Deterministic rotation (offset by persisted
  `next_sync` or the op seq) spreads load without breaking purity.

### CQ-10 — LOW — stale blanket `allow(dead_code)` masking real dead code

- [queue.rs:1-4](../crates/glade-discover-sim/src/queue.rs) and
  [faults.rs:1-4](../crates/glade-discover-sim/src/faults.rs) carry
  module-wide allows with reason "P4 runner consumes the P2 foundation" —
  P4 exists now, and the allows hide that `WakeQueue` and `message_refs`
  are used by nothing but their own tests (verified). Remove the allows;
  delete or wire the items. Same family:
  `RunnerError::NotImplemented` is a never-constructed variant.

### CQ-11 — LOW — layout and file-size hygiene

- Core [append.rs:30](../crates/glade-discover-core/src/append.rs) has a
  `use crate::{...}` block **between two functions** (a botched insertion;
  compiles, reads wrong). The runner's `Input` and `Generated` arms are a
  duplicated ~45-line step-execution block — extract one helper. And by
  this program's own hysteresis rule (split at >1000, target <500),
  [runner.rs](../crates/glade-discover-sim/src/runner.rs) (1562),
  [codec.rs](../crates/glade-discover-protocol/src/codec.rs) (1187) and
  [schema.rs](../crates/glade-discover-sim/src/schema.rs) (1157) are split
  candidates.

### CQ-12 — LOW — undocumented recovery runbook for a poisoned watermark

- `reseed` deliberately ignores `Uncertain{floor: Some}` — only wall
  progression past `floor + clock_resync_ms` heals a detected rollback
  ([clock.rs:32-74](../crates/glade-discover-core/src/clock.rs)). Correctly
  fail-closed, but if the watermark was poisoned *forward* by a wrongly-fast
  clock, the node stays unavailable until real time re-crosses it. The
  implicit operator path — remove the watermark record → `Unreadable` →
  `floor: None` → explicit `ClockReseed` — works but is written nowhere.
  One paragraph on `ClockHost` ends a future 3 a.m. debugging session.

### CQ-13 — LOW — ingress cache lifecycle sharp edges (document, don't change)

- A `Denied` correlation is cached forever without re-evaluation (correct —
  corrs are single-use; say so). The completed cache is freed only by
  `close_ingress`; an embedder that never closes ingresses will hit the
  admission caps and stall new requests by design. Both behaviors are
  correct per P7 — each needs one doc line where embedders will read it.

### Notes (no action required)

- §6's vestigial "most-specific wins" phrase: the kernel is exact-slot
  match (correct per "candidates = winners exactly matching the canonical
  Slot"; the specificity ladder lives in the service manager per R2-09).
  The phrase invites a future implementer to add kernel fallback — consider
  deleting it from the design text at the next re-vendor.
- Retained-set growth via renewal records is the **known, ruled** compaction
  deferral — at ~300 B/renewal against 16 MiB, a single 30s-renewed slot
  saturates a directory in ~19 days. Fine for stage 1; worth having the
  number in view when compaction is scheduled.
- `ClockHost::persist_watermark` and `DurableSnapshot.watermark` are two
  parallel persistence seams for the same value; both are correct, but a
  line saying which is canonical when both are wired would prevent drift.
- Quarantine is a verdict, not a store: an honest-but-out-of-order DirOp
  gets the same `Quarantined` disposition as an equivocation and is simply
  dropped (sync convergence recovers it). Behavior is sound; the naming
  overloads "hostile" onto "not-yet-foldable".

## Disposition summary

Nothing here invalidates the stage-1 claim: the frozen semantics are
implemented, the gates are green on both toolchains, and the adversarial
surfaces (codec, ingress, driver) are genuinely defended. Before any
**long-running** node: CQ-01 (mine pruning), CQ-03 (round eviction), CQ-04
(scan-abort consistency) — all small, pure, freeze-neutral fixes. Before
any **loaded** node: CQ-02 and CQ-06 (the derived-index and incremental-
commit seams). The remainder is hygiene and documentation. The two HIGHs
share one lesson worth carrying forward: the scenario suite proves
semantics, not asymptotics — growth and at-bound costs need either
long-horizon scenarios or explicit budget assertions in a future pass.
