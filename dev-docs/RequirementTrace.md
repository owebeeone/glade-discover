# Glade Discover Requirement Trace

Status: P7 implementation and review complete. All 24 primary scenarios, P5
hardening sweeps, P6 adapter integration tests, and P7 crash-safety regressions
are green; INV-D0 through INV-D6 have executable evidence at their assigned
layer. The reviewed baseline is commit `65fc18b`, tagged
`glade/discover-impl-stage1`.

The v3.1 semantic freeze is authoritative. This file assigns one primary
acceptance scenario to each frozen scenario name and identifies the package
that MUST make it executable. Secondary unit/property coverage is added beside
the primary row as implementation lands.

## Invariant ownership

| Invariant | Frozen rule | Primary closure gate | Planned evidence |
| --- | --- | --- | --- |
| INV-D0 | `step` is deterministic and pure | P2D/P5 | direct same-input unit test plus replay sweep |
| INV-D1 | wall rollback cannot resurrect a lease | P3A/P4 | clock unit tests and rollback/restart scenarios |
| INV-D2 | authority is checked before specificity | P4 integration | authority/projection/routing joint oracle |
| INV-D3 | partition healing cannot cause claim oscillation | P4 integration | claim/routing joint oracle and seed sweep |
| INV-D4 | unauthorized callers learn no node, definition, or compute signal | P6 node adapter | `p6_ingress.rs` INV-7 ordering/no-leak matrix |
| INV-D5 | gossip occurs only after persistence acceptance | P3D/P4 | append lifecycle unit and scenario assertions |
| INV-D6 | retained folds converge after connected delivery resumes | P4/P5 | sync integration oracle and convergence properties |

## P5 hardening evidence

| Hardening requirement | Executable evidence |
| --- | --- |
| Set-union convergence and total winner order | `crates/glade-discover-core/tests/p5_properties.rs` |
| Sync transition model and retained-byte boundaries | `crates/glade-discover-core/tests/p5_model_bounds.rs` |
| Every authority conjunct is necessary | `crates/glade-discover-core/tests/p5_authority_mutations.rs` and `p3b_ingest_authority.rs` |
| Hostile protocol bytes and canonical mutations | `crates/glade-discover-protocol/tests/p5_hostile_decode.rs` |
| Hostile strict scenario documents | `crates/glade-discover-sim/tests/p5_hostile_schema.rs` |
| Reusable protocol and scenario libFuzzer entry points | `fuzz/fuzz_targets/*.rs` |
| Loss/reorder/duplicate/partition seed sweep and append restart cuts | `crates/glade-discover-sim/tests/p5_adversarial_sweeps.rs` |

## P6 trusted-boundary evidence

| Adapter requirement | Executable evidence |
| --- | --- |
| Restart-idempotent allocation/sign/persist and durable payload binding | `crates/glade-discover-node-adapter/tests/p6_append.rs` |
| Forward-only persisted watermark and uncertain-clock publish suppression | `crates/glade-discover-node-adapter/tests/p6_clock.rs` |
| Atomic state/watermark/outbox commit, acknowledgement, retry, and bounded backlog | `crates/glade-discover-node-adapter/tests/p6_driver.rs` |
| INV-7 before topology/definition action, terminal replay, bounded ingress cache | `crates/glade-discover-node-adapter/tests/p6_ingress.rs` |
| Exact message delivery and transport-loss isolation | `crates/glade-discover-node-adapter/tests/p6_transport.rs` |
| Advertise through durable append, accepted fold, and exact-byte gossip | `crates/glade-discover-node-adapter/tests/p6_vertical_integration.rs` |

## P7 review evidence

| Review closure | Executable evidence |
| --- | --- |
| Exact append callback correlation and uncertain-clock retry | `crates/glade-discover-core/tests/p3d_append_sync.rs` |
| Correlated SyncOps fold and terminal rounds | `crates/glade-discover-core/tests/p3d_append_sync.rs` |
| Outbound sync frame/id/peer bounds, complete response, canonical retained-byte validation, and capacity admission against configured/actual retained bytes | `crates/glade-discover-protocol/tests/corpus.rs`, `crates/glade-discover-core/tests/p3d_append_sync.rs`, and `crates/glade-discover-node-adapter/tests/p6_driver.rs` |
| Revoked/expired/losing local derived teardown | `crates/glade-discover-core/tests/p3a_claims.rs` |
| B5 revalidation of fresh and durable append bytes | `crates/glade-discover-node-adapter/tests/p6_append.rs` |
| Current INV-7 replay, source-closure binding, and bounded admission | `crates/glade-discover-node-adapter/tests/p6_ingress.rs` |
| Authenticated create/CAS snapshots, stable bounded crash-safe outbox, restore composition, and driver-owned wall scheduling | `crates/glade-discover-node-adapter/tests/p6_driver.rs` |
| Restore/schedule expiry reconciliation for accepted derived services | `crates/glade-discover-core/tests/p3a_claims.rs` |
| Generic append/restart environment and exact replay | `crates/glade-discover-sim/tests/environment_contract.rs` and `p5_adversarial_sweeps.rs` |
| Every schema-valid throughout oracle after every event | `crates/glade-discover-sim/tests/throughout_expectations.rs` |

## Primary acceptance scenarios

`primary` is a machine-checked marker. Each v3.1 `s-disc-*` scenario MUST occur
exactly once in this table with that marker.

| Scenario | Role | Requirement | Invariant(s) | Work package | Planned data/test path |
| --- | --- | --- | --- | --- | --- |
| s-disc-authz-boundary | primary | GD56R2-01 | INV-D4 | P2C/P6 | `scenarios/authz/authz-boundary.*` |
| s-disc-inst-authority | primary | GD56R2-02 | INV-D2 | P2C/P4 | `scenarios/authz/inst-authority.*` |
| s-disc-inst-forged-node | primary | GD56R2-02 | INV-D2 | P2C/P4 | `scenarios/authz/inst-forged-node.*` |
| s-disc-inst-wrong-def | primary | GD56R2-02 | INV-D2 | P2C/P4 | `scenarios/authz/inst-wrong-def.*` |
| s-disc-inst-revoked-exec | primary | GD56R2-02 | INV-D2 | P2C/P4 | `scenarios/authz/inst-revoked-exec.*` |
| s-disc-append-restart | primary | GD56R2-03 | INV-D5 | P4/P6 | `scenarios/ingest/append-restart.*` |
| s-disc-delayed-sign | primary | GD56R2-03 | INV-D5 | P4 | `scenarios/ingest/delayed-sign.*` |
| s-disc-wall-rollback | primary | GD56R2-04 | INV-D1 | P2C/P4 | `scenarios/clock/wall-rollback.*` |
| s-disc-restart-uncertain | primary | GD56R2-04 | INV-D1 | P2C/P4 | `scenarios/clock/restart-uncertain.*` |
| s-disc-skew | primary | GD56R2-04 | INV-D1 | P2C/P4 | `scenarios/clock/skew.*` |
| s-disc-sync-round | primary | GD56R2-05 | INV-D6 | P4 | `scenarios/sync/sync-round.*` |
| s-disc-sync-drop | primary | GD56R2-05 | INV-D6 | P4 | `scenarios/sync/sync-drop.*` |
| s-disc-sync-retry | primary | GD56R2-05 | INV-D6 | P4 | `scenarios/sync/sync-retry.*` |
| s-disc-epoch-tie | primary | GD56R2-06 | INV-D3 | P2C/P4 | `scenarios/claims/epoch-tie.*` |
| s-disc-no-ping-pong | primary | GD56R2-06 | INV-D3 | P2C/P4 | `scenarios/claims/no-ping-pong.*` |
| s-disc-owner-proof | primary | GD56R2-07 | INV-D2 | P4 | `scenarios/authz/owner-proof.*` |
| s-disc-unauth-revoke | primary | GD56R2-07 | INV-D2 | P4 | `scenarios/authz/unauth-revoke.*` |
| s-disc-regrant | primary | GD56R2-07 | INV-D2 | P4 | `scenarios/authz/regrant.*` |
| s-disc-proof-late | primary | GD56R2-08 | INV-D2, INV-D6 | P4 | `scenarios/ingest/proof-late.*` |
| s-disc-revoke-then-grant | primary | GD56R2-08 | INV-D2, INV-D6 | P4 | `scenarios/ingest/revoke-then-grant.*` |
| s-disc-noclaim-handoff | primary | GD56R2-09 | INV-D4 | P4/P6 | `scenarios/routing/noclaim-handoff.*` |
| s-disc-corr-collision | primary | GD56R2-10 | INV-D0 | P2C/P4 | `scenarios/routing/corr-collision.*` |
| s-disc-route-terminal | primary | GD56R2-10 | INV-D0 | P2.5 | `scenarios/routing/route-terminal.*` |
| s-disc-dos-bound | primary | GD56R2-11 | INV-D0, INV-D6 | P4/P5 | `scenarios/bounds/dos-bound.*` |

## Harness-only requirement

GD56R2-12 is covered by strict schema decoder tests, fixed SplitMix64 vectors,
stable queue ordering tests, and byte-identical replay tests. It has no separate
`s-disc-*` name in v3.1.
