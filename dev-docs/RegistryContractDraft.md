# Registry contracts — interface-only steps 1–3

Date: 2026-09-05. Status: executable draft contracts, not production adapters or a
demo cutover. Owner approval of implementation remains required. These contracts
extend [DraftHostContracts.md](DraftHostContracts.md), without changing the frozen
discovery protocol or selecting an async lifecycle framework.

## Boundaries

| Crate suffix (`glade-discover-…`) | Public traits | Direct dependencies |
| --- | --- | --- |
| `acceptance-api` | `AcceptanceJournal` | protocol |
| `registry-api` | `RegistryWriter`, `RegistryReader` | protocol, acceptance-api |
| `trust-api` | `TrustPolicy` | protocol |
| `placement-api` | `ShardLocator` | protocol |

Registry reuses acceptance request keys and receipts rather than defining competing
durability semantics. Each crate has an explicit contract classification, required
trait methods and a feature-gated reusable conformance suite. No implementation,
node, runtime or new third-party dependency is introduced. Async methods return
Send futures; handles are Send + Sync. Constructing a future MUST NOT start work.
Dropping a polled write future MUST NOT be interpreted as rollback or cleanup.

## Atomic acceptance and recovery

Within one immutable stream/origin scope, a successful journal commit MUST atomically
retain the request key, draft, exact signed operation, revision, sequence/hash cursor,
non-regressing clock watermark and pending downstream handoff. Exact replay MUST
return the original receipt before checking the stale expected revision. A changed
draft or signed operation under the same key MUST fail. New intents MUST satisfy
revision comparison and operation-chain validation; known failures MUST NOT mutate
state. OutcomeUnknown requires lookup/recovery, not an assumption of failure.

An acceptance receipt means durable local acceptance at that registry, not peer
replication, current resolution visibility or successful resource access. Admission
MUST authenticate the requester and validate signatures, authority, payload and
trusted time before this internal journal port is used. Public structs are not
unforgeable authorization tokens. An unreadable/corrupt journal MUST NOT be treated
as a fresh empty scope; an absent watermark is not evidence of a trustworthy clock.

The journal is a write-ahead acceptance boundary, not the entire derived kernel
transaction. Consumers MUST durably commit the derived fold/outbox before
acknowledging the journal handoff and MUST deduplicate replayed operations.

| Interruption point | Required recovery |
| --- | --- |
| Before acceptance commit | No request, operation, cursor or watermark residue |
| After commit, before receipt delivery | Entire acceptance recoverable; retry returns original receipt |
| After downstream durable commit, before acknowledgement | Pending handoff can replay; consumer deduplicates |
| After acknowledgement commit, before its reply | Handoff stays removed; retry identity and cursor remain |

Acknowledgement MUST NOT erase request deduplication or resurrect a consumed handoff
when the original publish is retried. Retained requests and pending handoffs MUST be
bounded by configured capacity, with explicit Capacity errors rather than silent
eviction. A future retention/retirement profile needs separate review.

## Registry, trust and placement

Publish MUST mint an identity; renewal MUST preserve the existing claim identity,
target and authority. A new renewal uses a new intent. Exact retry MUST return the
original acceptance without extending the lease, including after restart/expiry,
subject to current authorization. Resolution MUST omit expired and superseded
claims, honor query/limit bounds and distinguish denial, unavailability and clock
uncertainty from an empty result. Every result is a partial local view: even an
untruncated empty result proves neither global absence nor unreachability.

Trust evaluation MUST bind the exact subject, action and complete source closure.
Permission for source A MUST NOT authorize a derived A+B result. Callers MUST
reevaluate each security-sensitive operation; a finite permit expiry is not a
revocation exemption. This interface supplies no cache-invalidation mechanism.
Composition MUST address policy/effect ordering; these independent ports do not
provide an atomic distributed revocation barrier. Cryptographic signature validity
alone is not publishing or registry authority.

Placement MUST bind the canonical namespace/key and authorized node/shard/mapping
epoch, bound candidates and referral work, and reject expired or unverifiable
delegation. Locality preference MUST NOT confer authority. Candidate nodes are not
promises of reachability, elected leadership or source fencing. Namespace authority
schema, canonicalization and proof-verification profile MUST be settled before
production implementation; the draft does not invent a new protocol grant verb.
Membership migration, tiering and consensus selection also remain open.

## Traceable canonical tests

Reusable assertions live in each crate's `src/conformance.rs`; private models and
rejecting mutants live in `tests/public_contract.rs`. Implementations SHOULD reuse
the assertions through a dev-dependency with `conformance` enabled.

| Requirement | Executable scenario |
| --- | --- |
| AC-001 | Atomic acceptance, exact retry and reopen |
| AC-002–003 | Precommit failure and lost postcommit reply |
| AC-004 | CAS conflict, changed retry, clock regression |
| AC-005–006 | Acknowledgement retains deduplication; reopen/lost reply cannot resurrect handoff |
| AC-007 | Invalid stream, origin, chain, payload and draft bindings leave no residue |
| AC-008 | Deterministic competing-intent/stale-CAS schedule has one winner |
| RG-001 | Publish receipt and retry identity |
| RG-002 | Renewal identity, supersession and expiry boundaries |
| RG-003–004 | Empty/error distinction, query and result bounds |
| RG-005–006 | Invalid/retargeted renewal and mixed-source denial |
| RG-007 | Restart preserves accepted claim and exact retry |
| TP-001–002 | Exact subject/action/complete-source scope |
| TP-003–005 | Revocation, missing evidence, expiry and uncertain clock |
| PL-001–002 | Bounded placement and exact configured authority/node/shard/epoch |
| PL-003–004 | Expiry, unknown namespace, unavailability and unverifiable evidence |

These fixtures are volatile snapshot-copy models with synthetic unsigned evidence,
not durable stores or cryptographic implementations. AC-008 awaits two lazy futures
sequentially: it is NOT an overlapping-execution race test. Workspace claim fixtures
do not establish all service-claim behavior; mixed-source probes do not implement a
projection engine. Actual crash/interleaving tests, complete profile coverage,
capacity/overflow extremes, cancellation schedules, clock acquisition and real
authority verification remain adapter obligations.

## TDD and adversarial review

Tests preceded the interfaces (unresolved-import RED). The architecture gate first
rejected unclassified new crates; exact entries were added without relaxing existing
classifications or dependency rules. The new command selectors also failed before
being added. Semantic RED runs exposed incomplete mixed-source checks, copied
placement evidence, invalid proposal acceptance, renewal supersession/retargeting
and restart/retry gaps; private models and assertions were corrected with mutants
retained.

Independent review identified five issues: complete-source denial, placement
node/shard/epoch binding, trust expiry/clock uncertainty, acknowledgement restart
safety, and ambiguous permit caching. All were addressed. Source/policy re-review
reported no remaining blocker for this draft-only tranche, expressly reserving
real storage/concurrency/cryptography verification for adapters.

## Fast verification

```sh
sh scripts/check-contracts.sh acceptance
sh scripts/check-contracts.sh registry
sh scripts/check-contracts.sh trust
sh scripts/check-contracts.sh placement
# All seven contracts plus architecture gate; not the full application workspace.
sh scripts/check-contracts.sh
```

The four new crates have 38 integration/fixture tests and five compile-fail doctests.
Together with the earlier tranche, there are 67 integration tests and nine doctests.
Normal default-feature cargo test omits the conformance targets; the script and CI
explicitly enable them. Contract changes MUST include affected-consumer checks;
acceptance changes also affect registry. Architecture and all-feature lint checks
remain required. Passing private fixtures MUST NOT be reported as production
conformance. The existing demo has not been migrated or exercised by this tranche.

Local verification on 2026-09-05:

- All seven suites passed (67 integration tests, nine compile-fail doctests).
  The four new suites also passed on MSRV Rust 1.85; current Rust was 1.96.
- The four new crates passed formatting and all-target/all-feature clippy;
  clippy also passed on Rust 1.85.
- Architecture gate and its 14 tests passed, as did 45 existing node-adapter
  tests, four core public-contract tests and six protocol public-contract tests.
- Frozen design snapshot, requirement trace, shell syntax, CI YAML parsing and
  tracked diff whitespace checks passed. Hosted CI was not run.
- A warm all-seven command including the architecture gate and doctests took
  **3.00 seconds** wall time; the isolated trust command took **0.70 seconds**.
  These are local observations, not budgets or cold-build measurements. Test
  bodies reported 0.00 seconds at the harness's displayed precision; command
  timings also include process startup, Cargo, the gate and doctests.
