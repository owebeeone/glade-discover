# Draft host contracts — first interface-only tranche

Date: 2026-09-05. Status: **executable draft contracts and reusable tests; no production adapters or cutover**. Owner review is still required before implementation. This is not the complete discovery/placement/trust architecture.

The owner requested interface traits and canonical TDD tests while async lifecycle research proceeds elsewhere. This tranche covers stable host seams without choosing a lifecycle framework, channels, macros, consensus, sharding, or new wire semantics. Existing production crates and the working demo are unchanged.

## Packages and scope

| Contract crate | Required traits/methods | What success means |
| --- | --- | --- |
| [glade-discover-transport-api](../crates/glade-discover-transport-api/src/lib.rs) | `Transport::send` | Local transport acceptance only, not remote receipt or persistence |
| [glade-discover-signature-api](../crates/glade-discover-signature-api/src/lib.rs) | `Signer::{principal, sign}`, `Verifier::verify` | Signature and claimed-origin binding; not authorization, payload validity or freshness |
| [glade-discover-operation-store-api](../crates/glade-discover-operation-store-api/src/lib.rs) | `DurableOperationStore::{load, persist}` | Exact local canonical record persisted under a declared durability profile; not complete append acceptance |

Each crate depends **only** on `glade-discover-protocol`; there are no dependencies between the three, no node/core imports, and no new third-party dependencies. The existing protocol dependency includes its codec and SHA-256 implementation: these are small contracts, not dependency-free crates. No runtime or macro dependency is introduced.

Each package is classified as `contract` in `architecture-policy.json`. Existing classifications/allowlists are unchanged. No implementation crate is claimed to exist; the test fixtures are private to integration tests.

## Draft async and ownership choices

- Methods use standard `Future` return types with `Send`; services require `Send + Sync` and shared `&self` access. This permits overlapping borrowed calls without requiring spawned tasks, channels, or threads. It excludes local-only/non-Send implementations in this draft.
- Dispatch is generic; these return-position future traits are not a `dyn Trait` API. No compulsory boxing/allocation is introduced. A reviewed dynamic-dispatch adapter MAY be added later.
- Borrowed requests MUST remain alive while their futures exist. Independently spawned work needs an owned-lifetime arrangement at the integration layer; no `'static` requirement is imposed on every call.
- Constructing/dropping an **unpolled** future MUST initiate no backend I/O. Once polled, dropping it MUST NOT be interpreted as proof of rollback, non-delivery, non-persistence, or completed cleanup. Side effects may already have happened.
- Known rejection/unavailability is separate from an uncertain write/send outcome. The caller MUST reconcile or retry idempotently where an effect may have happened; this port does not choose retry policy.
- No ordering is promised across overlapping operations. The caller MUST await required prerequisites. Scheduling limits, cancellation coordination, cleanup ownership, shutdown, and supervision remain with the lifecycle workstream.
- Error enums currently classify outcomes; richer diagnostics and generic/dynamic/local-only variants remain review questions, not selected production requirements.

## Critical storage boundary

The existing protocol `op_hash` hashes the **unsigned envelope**, excluding signature bytes. The new port does not change that protocol identity. OS-006 rejects a different canonical signed-byte variant at an existing hash with `CanonicalConflict`, preserving the stored bytes. Different unsigned envelopes, including equivocation at the same stream/origin/sequence, have distinct hashes and MUST coexist.

This first-variant rule is a **draft store policy**, not a global signature canonicalization rule. The caller/admission layer MUST prevent unverified data from reserving a variant in the accepted store. The store deliberately does not verify signatures itself. Quarantine/retention of untrusted signature variants needs a separate reviewed boundary; the owner MAY choose another storage representation before implementing this port.

An operation receipt is not an accepted-intent transaction. Allocation, intent deduplication, exact canonical operation, kernel state, and clock/sequence protections still need their appropriate atomic integration contract. Existing discovery v3.1 persistence-before-acceptance/gossip remains mandatory. Nothing here authorizes memory-only acceptance or replaces the existing host adapter.

## Canonical requirement-to-test map

Reusable assertions live in each crate's `src/conformance.rs`; fixtures and negative witnesses live in `tests/public_contract.rs`. Enable the opt-in `conformance` feature through an implementation's **dev-dependency** when reusing the suite. Production default builds omit the helper code.

| ID | Required observation | Canonical helper / executable evidence |
| --- | --- | --- |
| TR-001 | Exact destination/message submitted; honest local acceptance | `exact_submission`; `tr_001_exact_destination_and_message`; wrong-destination mutant |
| TR-002 | Known rejection causes no submission; uncertainty may follow submission | `failure` with independent observer; known-rejection mutant; uncertain-result positive fixture |
| TR-003 | No effect during construction/unpolled drop | `unpolled`; `tr_003_unpolled_call_has_no_submission` |
| TR-004 | Two borrowed calls can coexist; futures can suspend | `tr_004_calls_can_overlap_without_exclusive_borrow`; `canonical_async_suite_can_suspend_without_runtime_or_threads` |
| SG-001 | Sign/verify round trip returns the exact stable identity | `round_trip_and_tamper`; wrong-identity mutant |
| SG-002 | Modified payload, signature and origin do not authenticate | `round_trip_and_tamper`; accept-anything mutant; unknown-origin unavailable regression |
| SG-003 | Backend inability remains an error, not validity/invalidity | `unavailable`; `sg_003_backend_failure_is_not_invalid_signature_or_success` |
| SG-004 | No signing/verifier I/O during construction/unpolled drop | `unpolled`; independent eager-signer/eager-verifier mutants |
| OS-001 | Missing and failed reads are distinguishable | `missing`, `read_unavailable` |
| OS-002 | Receipt and loaded bytes match the canonical operation | `round_trip`; false-success/discarding-store mutant |
| OS-003 | Identical retries are idempotent; distinct hashes remain available | `round_trip` uses replay and same-record-identity/different-envelope evidence |
| OS-004 | Known write failure changes no stored record; uncertainty may persist | `write_failure`, `read_unavailable`; persistence-then-rejection mutant and uncertain-result positive fixture |
| OS-005 | Successfully persisted record survives actual backing-store reopen | `survives_reopen`; snapshot and record-loss fixtures exercise the assertion only; actual durability evidence PENDING |
| OS-006 | Same hash/different signature cannot silently replace stored bytes | `signature_variant`; overwrite mutant |
| OS-007 | No load/persist I/O during construction/unpolled drop | `unpolled`; independent eager-load/eager-persist mutants |

Four compile-fail doctests reject missing required trait methods. Implementing fixtures provide compiler witnesses; the ready-only driver requires `Future + Send`. One manually polled transport probe suspends and resumes deterministically. None of these are an executor or a lifecycle implementation.

### What is and is not proved

The private signing fixture is **non-cryptographic**, not a permissive development signer. The private store is **volatile**, not a durable implementation. Positive and deliberately broken fixtures validate the executable contract/test assertions. They MUST NOT be presented as production conformance.

Actual adapters MUST run applicable helpers with independent observers and explicit fixture setup. They also MUST supply algorithm/key-resolution vectors, broader envelope mutation coverage, real storage reopen and crash/fault injection, ambiguity after interrupted writes, and real transport acceptance/backpressure tests. Production durability profiles MUST specify process/OS/device failure assumptions. Callback-driven reopen only establishes durability when the callback actually closes and reopens the backing store.

Full lifecycle cancellation/shutdown correctness, trust authorization, distributed consistency, performance under load and demo integration remain unimplemented. No full-system assurance is inferred from this fast suite.

## TDD and adversarial review record

1. Tests and empty crate shells were created first. The initial run failed with unresolved trait/type/helper imports in all three crates. Contracts and reusable helpers were then added, without production adapters.
2. The architecture gate rejected all three unclassified libraries (ARCH-001). Exact contract-role entries, required method lists and the one allowed protocol dependency were added; no existing rule was relaxed.
3. The unknown-origin regression failed because the original helper demanded `Invalid` for unavailable key material. It now accepts a fail-closed `Unavailable` for the altered origin, while requiring the original valid signature to authenticate.
4. An independent adversarial reviewer found the unsigned-hash/signed-byte ambiguity (P1), known-failure side-effect checks (P2), and incomplete lazy-I/O checks (P2). Added failing tests, fixed the contract/assertions, and retained rejecting mutants. The signature-variant regression specifically failed on the fixture's original silent overwrite.
5. Re-review confirmed all three findings addressed and no remaining blocker **for draft contracts**. The reviewer also checked the new architecture classifications; no existing allowlist relaxation was observed. The reviewed snapshot passed 26 integration tests and four compile-fail tests; additional second-operation laziness and pending-future probes followed.

## Fast commands and CI

Run from `glade-discover`:

```sh
# Architecture gate plus only the requested package and its conformance feature.
sh scripts/check-contracts.sh transport
sh scripts/check-contracts.sh signature
sh scripts/check-contracts.sh store

# All draft contracts (now seven), not the simulator or node application.
sh scripts/check-contracts.sh
```

The subsequent [registry contract tranche](RegistryContractDraft.md) adds four
contracts. Verification figures below describe this original three-crate tranche.

`cargo test` without `conformance` does not run these feature-gated integration targets. The script enables it explicitly. CI's `contract-interfaces` job runs the script and all-feature clippy on Rust 1.85; the existing verification job depends on it. Required merge-check settings remain a separate hosting configuration.

No existing code imports these new APIs yet. When implementations/consumers arrive, interface changes MUST test those affected consumers; internal adapter edits SHOULD use the narrow owning suite first. The fast script is not an exhaustive assurance run. Reusable policy: [LibraryBoundaryAndTestingPolicy.md](../../dev-docs/LibraryBoundaryAndTestingPolicy.md).

### Local verification and timings

On the local macOS arm64 host, Rust 1.85.0 (MSRV) and 1.96.0:

- **29 integration tests and four compile-fail doctests passed** for the new contracts on both toolchains. Formatting and all-target/all-feature clippy passed on both.
- The architecture gate passed. Its 14 existing negative/positive tests passed; 45 existing node-adapter tests, four core public-contract tests, and six protocol public-contract tests also passed.
- Pinned design snapshot and requirement-trace checks passed. CI YAML/job dependencies, script syntax, bad-argument rejection, document links and whitespace were checked.
- Test-body execution reported `0.00s` at the Rust harness resolution for each suite. Separately launching the already-built transport/signature/store test binaries measured `0.00s`, `0.00s`, and `0.54s` wall time respectively, including process-launch overhead; these rounded values are not zero-cost claims.
- A warm all-contract command including architecture gate and doctests measured **1.72s wall time** on Rust 1.96. An earlier incremental compile-plus-check run measured **4.51s** (Cargo reported `0.67s` compilation), while another build was active.
- A fresh target-directory build plus all new tests/doctests measured **6.01s wall time** on Rust 1.85 (Cargo reported `3.53s` compilation). Registry sources were cached/offline; this excludes downloads and a cold architecture-tool build. The isolated target directory was `/tmp/glade-contract-cold.LTgbEl`.

These are observations from one development host, not enforced budgets or comparable benchmark samples. Full simulator/fuzz suites, actual adapters, demo execution and hosted CI were not run. No production code, pinned protocol definitions, or existing runtime dependencies changed; no commits were made.

## Next review decisions

- Approve or revise the four trait surfaces, generic/Send choices, error detail, and signature-variant store policy.
- Define the atomic accepted-intent/state/clock commit boundary before creating a real durable append adapter.
- Separately settle public publish/renew/resolve receipts, trust-policy evidence, shard/referral interfaces and clock domains. This tranche does not pretend those open semantics are frozen.
- Choose lifecycle/cleanup orchestration after the independent research/API proposals. Then extend canonical tests before implementations, keeping the demo on its existing path until an explicit integration/cutover review.
