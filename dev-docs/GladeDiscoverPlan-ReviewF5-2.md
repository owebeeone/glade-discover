# Review F5-2 — `GladeDiscoverPlan.md` (re-review after revision)

Reviewer: Claude Fable 5 (authored the v3 freeze + ReviewF5 — adversarial, not
independent). Date: 2026-07-17. Scope: the revised plan; (1) closure audit of
PF5-01..09, (2) fresh hunt for issues INTRODUCED by the revisions.

## Verdict

All nine PF5 findings are properly closed — none by weakening. One (PF5-04) was
closed **better than the review asked**: the `VerifierFixture` design keeps the
verdict out of production records entirely, where my suggestion (`sig_valid` on
`SignedOp`) would have contaminated the wire shape. The revision introduces
**one new HIGH** (the fixture cannot address ops the kernel/environment MINTS at
runtime — which blocks two wave-1 scenarios the same way PF5-04 blocked the
authority ones) and three MEDIUM clarifications. After freezing the two fixture
rules, the plan is ready to execute.

## Closure audit (PF5-01..09)

| PF5 | Status | Where / note |
| --- | --- | --- |
| 01 §5/§7 ordering | **Closed** | §5 lines 194–197 (P1 coordinator-only gate); §7 W1a/W1b split; W1a others review-only. |
| 02 walking skeleton | **Closed** | New P2.5 (lines 403–435): `s-disc-route-terminal` end-to-end, coordinator-owned, "P3 MUST NOT start until green", real frozen types, TDD ladder, invalid-authorization sibling. W2a in the schedule. As recommended, plus the sibling guard. |
| 03 RED honesty | **Closed** | P2C lines 375–379: decode-valid + "runner not implemented" is the P2 bar; named-invariant failure becomes mandatory at P2.5/P4. Honest. |
| 04 verifier seam | **Closed — improved** | P1.1 lines 271–276: sim-only `VerifierFixture{op_ref, reader?, signer, verdict}` + verifier table; verdicts NEVER on wire/production records. Cleaner than the reviewed suggestion; adopt as precedent (test-control data stays out of production shapes). |
| 05 vendored copy | **Closed** | P0.1: read-only pinned snapshot, DO-NOT-EDIT banner with revision+sha256, hash-check, rule 6 extended (lines 37–39). |
| 06 Cargo.lock | **Closed** | Committed (line 55); CI `--locked` on every §9 command. |
| 07 stop/horizon | **Closed (data)** | P1.3 freezes `stop: AtMs \| QuiescentForMs` as required scenario data. The RULE semantics still land at P4.1 — see PF5-2-04 below for the one wrinkle. |
| 08 cross-lane invariants | **Closed** | P3 exit gate lines 515–517 (lanes MUST NOT claim D2/D3/D6) + P4.2 lines 551–553 (explicit integration-gate assertions). |
| 09 direct purity | **Closed** | P2D lines 395–396: step twice, byte-identical. Sim replay test retained separately. |

## New findings (introduced or exposed by the revisions)

### PF5-2-01 — HIGH — `VerifierFixture` cannot address runtime-MINTED ops

- **The gap:** `VerifierFixture{op_ref, …}` addresses ops that exist in the
  scenario's `inputs`. But the kernel MINTS ops at runtime — renewals and
  takeovers via `Append` → environment signs → `OpAccepted` (P4.1 "models
  signing/persistence as environment behavior"). Those ops have no authorable
  `op_ref`, yet OTHER nodes' verifiers must judge them when they gossip.
- **What it blocks:** `s-disc-no-ping-pong` (renewals under partition MUST NOT
  oscillate — renewals are kernel-minted) and `s-disc-epoch-tie` (takeover
  claims), both **P2C wave-1**; later every append/sync scenario
  (`-append-restart`, `-delayed-sign`, `-sync-round`). Same failure class as
  PF5-04: the frozen fixture schema can't express scenarios already assigned to
  the first wave.
- **Disposition (freeze with the fixture in P1):** (a) a **default rule** —
  environment-minted ops verify VALID unless overridden; (b) an **addressing
  form for minted ops** — a fixture selector by `(node, slot, generation)` or
  `(origin, seq-range)` pattern, so a scenario can declare e.g. "node B's
  takeover claim verifies invalid at node C". Reject scenarios whose fixtures
  address nothing.

### PF5-2-02 — MEDIUM — fixture resolution + `op_ref` addressing are unspecified

- `reader?` is optional: when absent, does the verdict bind ALL readers? When a
  specific `(op, reader)` fixture and a global `(op)` fixture both match, which
  wins? Duplicate `(op_ref, reader)` pairs are not in P1.3's decoder-rejection
  list ("duplicate ids" reads as scenario ids).
- `op_ref` itself needs a frozen form: scenario `inputs` need **stable input
  ids** (the analogue of the effect `msg-ref = (producing-event-id,
  emission-index)`), or `op_ref` is an index that silently shifts when a
  scenario is edited.
- **Disposition:** freeze — most-specific match wins (reader-specific >
  global); duplicate `(op_ref, reader)` is a decode rejection; inputs carry
  explicit ids and `op_ref` names them. One paragraph in P1.1/P1.3.

### PF5-2-03 — MEDIUM — post-P2.5 module ownership handoff is unstated

- P2.5 has the coordinator writing production code in `ingest.rs`,
  `projection.rs`, `routing.rs`, `runner.rs` — modules that P3B/P3C/P4.1 assign
  to lane agents. Temporally safe (W2a is coordinator-only), but the HANDOFF is
  unstated: at W2b, do lanes inherit the skeleton code (including its
  `NotImplemented` branches) as owned code they may refactor?
- **Risk:** an unowned-feeling `NotImplemented` branch invites exactly the
  "opportunistically fix another lane's files" violation §4.1 forbids, or its
  inverse (a lane treating skeleton code as untouchable coordinator property).
- **Disposition:** one line in P2.5's exit gate: "on P3 start, each lane
  inherits full ownership of its module including skeleton code; the
  coordinator stops editing those modules." Also assign `runner.rs` skeleton
  pieces to the P4.1 simulator agent the same way.

### PF5-2-04 — MEDIUM — `QuiescentForMs` never fires under periodic wakeups

- P3D schedules a recurring **gossip tick** (self-rescheduling). A scenario
  with gossip enabled therefore ALWAYS has a pending wakeup — literal
  quiescence ("no pending events for X virtual ms") never occurs, and
  `QuiescentForMs` scenarios hang until… nothing. The stop DATA is frozen
  (PF5-07) but this rule detail decides whether wave-2 sync scenarios are
  even expressible with quiescence.
- **Disposition:** define quiescence as "no pending events excluding
  self-rescheduling periodic ticks" (tick wakeups carry a `periodic` marker in
  `WakeToken`), OR mandate `AtMs` for gossip-enabled scenarios. Decide at P1
  (it shapes `WakeToken`), implement at P4.1.

### PF5-2-05 — LOW — W1b load imbalance

- Agent 4 carries P2C (12 scenarios) + P2D (9 test families) while agents 2/3
  hold narrower lanes. If wall-clock matters, move P2D to agent 2 or 3 (corpus
  and sim-foundation are lighter). Advisory only.

## What held (revision-quality notes)

- The **W1a/W2a "review-only" waves** are the honest cost of the serialization
  fixes — three agents idle-but-reviewing twice. Correct trade; the reviews are
  named (adversarial / fixture / runner-oracle), not busywork.
- **P2.5's anti-cheat guards** are complete: no allow-all authority stub, no
  scenario-name branch, `NotImplemented` shortcuts that "cannot satisfy later
  scenarios," and the invalid-authorization sibling (the mutation test proving
  the happy path's verdict is live). The sibling may legitimately fail via
  `NotImplemented` until its owning lane — acceptable, and explicitly stated.
- The P2 exit-gate line "no test passes through a stubbed allow-all verdict"
  now composes correctly with P2.5's fixture-driven verifier.
- `--locked` discipline is consistent across §0/§9 (fmt correctly lacks it —
  fmt does not resolve the graph).

## Disposition summary

Nine of nine prior findings closed cleanly; one closed better than asked
(PF5-04 — adopt its production/test-data separation as precedent). Before W1:
freeze the two fixture rules (PF5-2-01 minted-op verdicts + addressing;
PF5-2-02 resolution/ids) — they are the same freeze-completeness class that
already bit once, and they block wave-1 scenarios. PF5-2-03/04 are one-line
clarifications (ownership handoff; quiescence vs periodic ticks — the latter
touches the frozen `WakeToken`, so decide it at P1 too). PF5-2-05 is advisory.
After those, execute — further prose review has hit diminishing returns; the
next findings will come from the walking skeleton.
