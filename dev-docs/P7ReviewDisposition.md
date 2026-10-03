# P7 Review Disposition

Status: implementation review and initial repository packaging complete
Date: 2026-07-18
Design baseline: vendored Glade discovery v3.1 semantic freeze, SHA-256
`d73b765c76e4c3bebb98bd7bce1f6fc2e7f99d2f9215c3bcd13ed683cab864af`

Packaging baseline: commit `65fc18b`, tag `glade/discover-impl-stage1`, on
`main` and published to `origin/main`.

## Review method

P7 reviewed the implementation, concrete scenario data, deterministic replay,
and trusted-node adapters together. Findings were fixed TDD-first and then
checked by agents independent of the implementation lane. A requirement is
closed only when the relevant regression test and the full workspace gate pass.

## Closed findings

| Area | Review finding | Disposition and executable evidence |
| --- | --- | --- |
| Ingress | Cached `Matched`/`NoClaim` results could bypass current INV-7 policy. | Every replay MUST reauthorize the original source closure. Revocation/regrant and left/right/neither/both vectors are in `crates/glade-discover-node-adapter/tests/p6_ingress.rs`. |
| Ingress | Definition candidates could substitute a closure after authorization. | Authorization evidence binds principal, query, closure, and policy revision; candidate closure MUST match before D5 placement. Covered by `p6_ingress.rs`. |
| Ingress | Admission failure semantics and cache growth were underspecified. | Admission failures are non-terminal and MUST NOT be cached. Global, per-ingress, and request-key byte limits are tested in `p6_ingress.rs`. |
| Append | Durable replay and fresh signed bytes were not reverified at B5. | The adapter MUST validate exact draft/origin/payload/canonical bytes and signature before persistence or callback. Corruption and cross-generation vectors are in `crates/glade-discover-node-adapter/tests/p6_append.rs`. |
| Core append | Callback matching was ambiguous for one intent across slots and uncertain-clock callbacks could retain/gossip. | Acceptance MUST match the exact finalized tuple and payload. Uncertain callbacks remain pending and inert; exact ready duplicates complete once. Covered by `crates/glade-discover-core/tests/p3d_append_sync.rs`. |
| Sync | `SyncOps` could fold outside the active `(peer, SyncId)` round. | Only the correlated active round folds operations. Covered by `p3d_append_sync.rs`. |
| Sync | Count-only chunks, complete head vectors, repeated unbounded ids, or incompatible retained/outbox configuration could prevent convergence. | `SyncOps` MUST satisfy byte and item ceilings; `SyncId` is limited to 256 bytes, configured node ids to 235 bytes, and only configured peers receive responses. `SyncStart` sends the largest fitting conservative head prefix, while a response remains the complete gap followed by `SyncEnd`. Construction/restore MUST reject outbox capacity below the conservative requirement computed from the larger of the configured ceiling and authenticated actual retained bytes; restore MUST reject a canonical retained-byte accounting mismatch before effects execute. The default 32 MiB/16,384-entry outbox admits a response near the 16 MiB retained ceiling. Protocol/core boundary tests and driver admission/integration cover the closure. |
| Claims | A revoked, expired, or losing local derived service was not reconciled. | Ready-clock reconciliation emits teardown and marks the local claim lost once. Covered by `crates/glade-discover-core/tests/p3a_claims.rs`. |
| Driver | An executor failure stopped later effects and exposed only an index. | Every queued effect is attempted independently; failures retain the exact effect and durable outbox id. Covered by `crates/glade-discover-node-adapter/tests/p6_driver.rs`. |
| Simulator | Minted appends, restart recovery, quiescence, seed grants, and duplicate expansion were not all environment-enforced. | The generic environment now finalizes and exact-replays accepted appends, enqueues restore recovery, validates seed grants, classifies quiescence exactly, and bounds expansion before allocation. Covered by simulator unit, environment, quiescence, and adversarial-sweep tests. |
| Simulator | The schema accepted `Throughout` forms the oracle returned as unimplemented, and convergence compared stored op values rather than frozen `RecordId` sets. | Convergence, route, and all state assertions MUST be checked after every event in the half-open interval; event ids disambiguate equal timestamps. Convergence MUST compare only retained identity sets. Covered by `crates/glade-discover-sim/tests/throughout_expectations.rs`. |
| Provenance/fuzz | CI did not compile nested fuzz targets and the design check trusted only the vendored header. | CI MUST compile both fuzz binaries; the snapshot script independently checks the parent body and `Decisions.md` hash. |
| Reproducibility | CI installed a floating fuzz nightly. | CI and the documented command MUST use `nightly-2026-02-28`. |

## Crash-safety closure

The final correctness pass found that commit-before-effect alone did not close
the crash window. A process could commit `Lost` and crash before teardown, or
commit `Pending` and lose a failed append effect. It also found that callers
could discard restore recovery or retain an old in-memory clock after wall
scheduling.

The trusted driver now owns a bounded durable outbox:

- kernel persisted state, effective-wall watermark, outbox entries, payload
  byte count, and the next monotone outbox id MUST commit as one versioned
  snapshot before execution;
- restore MUST authenticate and integrity-check that atomic snapshot before a
  loaded effect executes; first creation MUST be create-if-absent, and every
  replacement/acknowledgement MUST compare the expected monotone revision;
- restore MUST recompute canonical retained bytes, reject stored accounting
  mismatches, and admit sync capacity against the larger of the configured
  retained ceiling and authenticated actual retained bytes before execution;
- successful execution MUST be acknowledged by another durable commit;
- executor failure or acknowledgement failure MUST leave the exact effect
  replayable;
- normal and recovery backlogs MUST satisfy both entry and stable structural
  payload-byte budgets, with normal limits no greater than recovery limits; a
  full valid backlog MUST be drained with revision-checked acknowledgements
  until reconstructed recovery fits, or restore returns an explicit blocked
  result retaining its capacity cause;
- restore MUST merge kernel recovery with the loaded durable outbox and commit
  that merged snapshot before returning;
- wall scheduling MUST advance the pure clock, enqueue `Schedule`, and commit
  both through `NodeDriver` before timer execution;
- `restore_fresh` MUST reject durable pending recovery work; production restore
  uses `restore_with_recovery` through authenticated `NodeDriver::from_snapshot`;
- restore and wall scheduling MUST reconcile accepted derived services at the
  effective wall, marking expired/revoked losers `Lost` and queueing Teardown
  without waiting for another event;
- persisted monotonic schedules MUST rebase to the new process epoch;
- the outbox MUST reject a transition before state commit when either its
  configured entry or payload-byte limit would be exceeded.

`crates/glade-discover-node-adapter/tests/p6_driver.rs` covers executor failure,
restart replay, acknowledgement and later-effect failure, authentication,
create-if-absent and revisioned atomic snapshots, recovery under a full or
zero-capacity backlog, entry/byte limit validation, stable fixed-width sizing,
retained-byte accounting/config-downsize rejection, revision exhaustion,
monotonic rebasing, and driver-owned wall scheduling.
`crates/glade-discover-core/tests/p3a_claims.rs` covers downtime and scheduling
expiry reconciliation. The vertical append-to-gossip path in
`p6_vertical_integration.rs` asserts both enqueue and acknowledgement commits.

At-least-once delivery permits a crash-window duplicate. The trusted executor
MUST therefore treat Append as idempotent by `(slot, generation, intent)` and
Teardown as idempotent by `(slot, generation)`. Gossip is duplicate-safe,
scheduled wake tokens are stale-safe, and replies retain their ingress and
correlation identity.

## Verification gates

The release gate MUST include:

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

## Deliberate residuals

- V1 chain-prefix compaction/checkpoints and locality-aware rendezvous remain
  deferred exactly as frozen; retained history is bounded by configuration.
- Append persistence assumes one serialized writer per durable append key. A
  future multi-writer host MUST replace the read-then-persist pair with an
  atomic compare-and-set that returns the durable winner.
- `SourceClosure` provenance is a trusted policy/service-manager contract. The
  adapter checks equality with authorization evidence but cannot derive the
  closure from opaque definition bytes.
- Outbox delivery is at least once. Every host executor MUST preserve the named
  effect idempotency contracts; exactly-once external side effects are not
  claimed.
- The stable structural outbox sizing algorithm is part of snapshot format
  version 1. Any incompatible sizing change MUST use an explicit snapshot
  format migration and version bump.
- The durable store MUST implement the authentication, integrity, atomicity,
  and revision-CAS obligations of `DurableCommit`; the adapter cannot make an
  untrusted storage engine trustworthy.
