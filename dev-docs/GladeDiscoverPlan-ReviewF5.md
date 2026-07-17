# Review F5 — `GladeDiscoverPlan.md`

Reviewer: Claude Fable 5 (the session that authored the v3 semantic freeze —
adversarial but NOT independent; an independent pass is worth running too).
Date: 2026-07-17. Scope: the implementation plan, checked against
`GladeDiscoveryDesign.md` v3 (the frozen §1 API, §10 invariants, §13 scenarios)
and against parallel-execution / integration-risk practice.

## Verdict

The plan is faithful to the freeze and its parallel-work discipline is sound —
crate isolation (core depends only on protocol, no tokio/fs/net/rand/keys),
single-writer file ownership, TDD-first, no scenario-specific branches, complete
scenario coverage (§13's 24 `s-disc-*` all appear across P2C+P4.3). Those are
real strengths and should stay. Two issues would bite during execution: a
**phase-dependency contradiction between §5 and §7** that makes the first wave
either serial or type-unstable, and the **absence of a walking-skeleton milestone**
before the four-lane fan-out — the classic integration-risk trap for a spec that
has never been executed. Plus a **frozen-schema gap** (no signature-verdict seam)
that blocks authoring the authority scenarios as data, and the recurring
**design-doc-copy dual-maintenance smell**. None invalidate the plan; all are
cheaper to fix now than mid-wave.

## Blockers (fix before W1 starts)

### PF5-01 — BLOCKER — §5 and §7 disagree on P1→P2 ordering

- **§5 (lines 172–188)** makes P2A–P2D depend on P1 (`P1 ... └─ P2A..P2D`).
  **§7 (line 586, W1)** runs P1 (coordinator) and P2A/P2B/P2C/P2D (agents 2–4)
  **concurrently in the same wave**.
- **Conflict:** P2C authors scenarios as **typed data** and P2D writes core
  unit tests — both compile against P1's frozen `model.rs`/protocol types
  (`Event`, `Effect`, records, `Scenario`). If they start with P1, they compile
  against types still in flux, and the whole "freeze first, then implement
  against stable contracts" premise (the plan's own §4.2 gate) is violated. If
  they wait for P1, W1 is serial at the front and the wave table is misleading.
- **Disposition:** make the **shared-contract gate (§4.2) its own wave W1a**
  (coordinator only: P1.1 + P1.2 land + compile), THEN W1b fans out P2A–P2D.
  The plan already names §4.2 as "before the first parallel wave" — the wave
  table just contradicts it. Align §7 to §5.

### PF5-02 — BLOCKER — no walking skeleton before the four-lane fan-out

- The plan goes P2 (all RED) → **P3 four concurrent kernel lanes** (clock/claims,
  ingest/authority/projection, routing, sync/append) → P4 (first integration).
  The four lanes build independently against a **frozen-but-never-executed**
  contract, and the pieces first meet at P4.
- **Failure mode:** the v3 invariants most likely to be wrong-on-paper are
  emergent across modules — INV-D2 (authorized-before-specificity spans
  projection **and** routing), INV-D3 (no-oscillation spans claims **and**
  routing), INV-D6 (convergence spans ingest **and** sync). Rule 6 ("a
  discovered contradiction stops the lane, files a decision issue") means a
  contradiction found at P4 in a **shared frozen type** (e.g. the
  claim_id/epoch/generation interaction, or the pending-proof index shape)
  retroactively invalidates work in multiple lanes built on it.
- **Disposition:** insert a **P2.5 vertical slice** — ONE trivial scenario
  (`s-disc-route-terminal`: one `DirOp` claim in → `Route` → `Reply{Matched}`)
  driven end-to-end through a minimal ingest→retained→project→route→reply→runner
  path, single-node, no faults, before P3 fans out. It exercises the §1 API, the
  runner loop, and the oracle harness against real (if trivial) data, so a
  contract flaw surfaces at day 2, not at P4. Parallelize AFTER one green
  end-to-end, not before.

## High-priority findings

### PF5-03 — HIGH — "RED for the named invariant" is unachievable in W1

- **P2C (lines 351–352):** "Every scenario MUST fail against the unimplemented
  runner for the named missing invariant." But the runner (P4.1) and invariant
  oracle (P4.2) do not exist until P4. In W1 every scenario fails identically
  with P1.3's "runner not implemented" (line 288) — NOT for its named invariant.
- **Why it matters:** the value of RED-first is proving the test is meaningful
  (fails for the right reason). A generic "runner absent" failure proves only
  that the decoder ran. The genuinely-RED-for-the-right-reason state is first
  reachable at P4, so the P2C claim overstates the W1 guarantee.
- **Disposition:** split the assertion. W1 target = "scenario decodes as valid
  typed data + fails "runner absent"" (a schema/typing gate). Re-assert
  "fails for the named invariant, then goes green" at P4 when the oracle exists.
  Say so in P2C so an agent doesn't chase an impossible W1 state. (This is why
  PF5-02's walking skeleton matters — it's the first point a scenario can fail
  for a real reason.)

### PF5-04 — HIGH — the frozen scenario schema cannot express the authority scenarios

- **P3B (line 416):** "Crypto verification MUST use a deterministic verifier seam
  supplied as data." **P1/P2B** freeze the `Scenario` schema. But v3 §12's
  schema (`nodes/links/inputs/expect` + `seed_grants`) has **no per-op signer or
  signature-validity field**. `s-disc-forged-node` (signer ≠ derived principal),
  `s-disc-unauth-revoke` (revoker not authorized), `s-disc-owner-proof` all
  require the scenario to encode *which key signed each op* and *whether the
  stubbed verifier accepts it* — that is the "verifier seam supplied as data."
- **Failure:** with the schema frozen in W1 and this field absent, the authority
  scenarios (P2C wave-1, five of them) can't be authored; adding the field later
  is exactly the change-controlled frozen-type edit §4.2 makes expensive.
- **Disposition:** add to the frozen `SignedOp`/scenario-input representation a
  test-controlled `signer: Principal` + `sig_valid: bool` (the verifier-seam
  verdict), and a scenario `verifier_table` if verdicts vary by reader. Freeze it
  in P1, before the authority scenarios are authored. This is a freeze-completeness
  fix, not a semantic change.

### PF5-05 — HIGH — copying the v3 design into the repo re-creates the dual-maintenance smell

- **P0.1 (lines 205, 214):** "Copy the v3 semantic freeze into
  `dev-docs/GladeDiscoveryDesign.md` ... byte-identical or carries a provenance
  note." The canonical design lives in `glade-wz/dev-docs/glade/`. Two editable
  copies drift — this is the exact `grazel-app.glade` dual-maintenance problem
  the program already hit (glade/apps vs grazel/apps diverged silently).
- **Tension:** a member repo that travels (gwz capture/clone) genuinely may not
  carry the parent's dev-docs, so vendoring has a real reason.
- **Disposition:** vendor as an explicit **pinned snapshot** — parent is the
  sole source of truth, the vendored copy carries the source hash in a header
  banner, and rule 6 is extended: the vendored copy is **read-only**; a
  semantic-freeze change happens in the parent and re-vendors. Never edit the
  member copy. (Cheap: a one-line "DO NOT EDIT — vendored from glade-wz@<hash>"
  banner + a check.)

## Medium-priority findings

### PF5-06 — MEDIUM — `Cargo.lock` ignored contradicts the deterministic-replay headline

- **Line 53:** "`Cargo.lock` ignored for this library workspace" (the sibling
  lib convention). But byte-identical deterministic replay (a top-3 goal,
  INV-D0) can be perturbed by a utility-dependency bump (a hashmap-iteration or
  float-format change in a transitive dep). The kernel determinism is internal,
  but the SIM harness and its assertions run through deps.
- **Disposition:** commit `Cargo.lock` for the workspace (or at minimum pin the
  sim crate's dev-deps), OR state explicitly that determinism is guaranteed only
  against the checked-in lock and CI pins it. A replay oracle whose reproducibility
  depends on an unpinned graph is a latent flake source.

### PF5-07 — MEDIUM — the runner horizon / quiescence rule must be in the FROZEN schema

- **P4.1 (line 469):** the runner "stops deterministically at the declared
  horizon or quiescence rule" — designed in P4. But the `Scenario` schema is
  frozen in W1 (P1/P2B), and "declared horizon" is a scenario field. If the
  horizon/quiescence representation is designed at P4, the W1-frozen schema
  lacks it → a late change-controlled edit + re-touch of every authored scenario.
- **Disposition:** freeze the horizon/quiescence fields (e.g. `stop: {at_ms |
  quiescent_for_ms}`) in P1 with the rest of the `Scenario` schema. The RULE can
  be implemented at P4; the DATA SHAPE must exist at W1.

### PF5-08 — MEDIUM — cross-lane invariants need an explicit integration-gate owner

- INV-D2 (projection P3B + routing P3C), INV-D3 (claims P3A + routing P3C),
  INV-D6 (ingest P3B + sync P3D) are **emergent across lanes** — no single P3
  lane can prove them, and each lane's green tests can pass while the joint
  property fails.
- **Disposition:** name these three as **P4-gate assertions** owned by the
  coordinator/oracle (the plan puts the oracle in P4.2 — good), and add to the P3
  exit gate (line 450) a note that D2/D3/D6 are NOT lane-closable, so no lane
  claims them. Prevents a lane marking "INV-D3 covered" from its half.

### PF5-09 — MEDIUM — INV-D0 (purity) is only tested via sim replay

- **P4.2** tests INV-D0 as "deterministic replay." But purity — `step(state,
  ctx, event)` returns identical `(state', effects)` for identical inputs — is a
  **core-level** property, testable directly without the sim (call `step` twice,
  compare). Testing it only through the sim couples the strongest correctness
  guarantee to the heaviest harness.
- **Disposition:** add a core-crate property test (P3, any lane, or P2D shell):
  `step` is a pure function. Keep the sim replay test too; they cover different
  failure modes (pure-fn violation vs. runner-ordering nondeterminism).

## What held (checked, sound — keep)

- **Crate boundary** (§3, lines 117–122): core depends only on protocol; no
  tokio/fs/net/OS-rand/keys/node — faithful to v3 §1's pure-kernel + §2's
  clock-as-input. The node adapter as a separate final-phase crate is correct.
- **Scenario coverage:** every v3 §13 scenario (24 across 11 findings) is
  assigned exactly once across P2C (01,02,04,06,10) and P4.3 (03,05,07,08,09,11).
  No gap, no duplication.
- **INV-D4 placement** (P4.2, lines 484–486): correctly kept OUT of the
  authz-blind core sim and assigned to the P6 node-wrapper — matches v3 §7's
  trust boundary. Good discipline, not an oversight.
- **Rule 6** (line 36): "the semantic freeze MUST NOT change implicitly in code;
  a contradiction stops the lane + files a decision issue" — the right guard for
  a freeze-then-build model. (PF5-05 extends it to the vendored copy.)
- **Rule 7** (no scenario-specific branches): the essential anti-cheat for a
  data-driven oracle; keep it load-bearing in the handoff checklist (§8).
- **Deferred list** (§10): compaction, locality, observability, HLC, in-core
  transport/keys — matches v3's deferrals exactly; nothing half-invented.

## Disposition summary

Two must-fix-before-W1: **PF5-01** (align §5/§7 — gate P1 as its own sub-wave)
and **PF5-04** (add the signature-verdict seam to the frozen schema, else the
authority scenarios can't be authored). One structural must-fix: **PF5-02**
(a walking-skeleton P2.5 before the four-lane fan-out — the single highest-value
change; it converts P4 integration risk into a day-2 check). PF5-03/05/07 are
freeze-completeness + honesty fixes cheap to land in W1. PF5-06/08/09 are
quality hardening. None require reopening the v3 semantics — they are plan-shape
and freeze-completeness, not design. After these, the plan is ready to execute.
