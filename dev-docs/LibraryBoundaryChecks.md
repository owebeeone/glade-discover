# Library boundary checks

Date: 2026-09-05. Status: initial architecture guard; no production discovery behavior changed.

This is the local adoption record for the workspace's reusable [Library Boundary and Testing Policy](../../dev-docs/LibraryBoundaryAndTestingPolicy.md). The Glade package proposal is [GladePackageArchitecture.md](../../dev-docs/GladePackageArchitecture.md). Those relative links resolve in the GWZ checkout; standalone users should obtain the root documents from the workspace owner rather than assume the files live in this repository.

## Scope and command

```sh
./scripts/check-architecture.sh
```

The gate covers **library members of this repository's primary Cargo workspace**. It uses `cargo metadata --no-deps --locked --offline`, reads `architecture-policy.json`, and parses reachable Rust modules with `syn`. It does not build the discovery libraries or run their tests. The checker itself is an independent, locked Cargo workspace under `tools/architecture-check`, so core/package tests do not compile its dependencies. Its first build may fetch its own dependencies; subsequent metadata checks require no application dependency downloads.

CI runs the guard's tests and gate in the `Architecture boundaries` job, before the existing full verification job. A failed check fails CI. Plain `cargo build` does not run this gate. Repository hosting branch protection must separately mark the job required for merge; this change does not configure that setting.

Nested standalone workspaces (including fuzz and the checker), other GWZ repositories, binaries without library targets, and non-Rust packages are not classified by this initial gate. A newly added primary-workspace library fails until classified. Extending scope requires explicit adoption rather than claiming blanket coverage.

## Checked rules

| Error | Shared policy | Check / test evidence |
|---|---|---|
| ARCH-001 | LBT-001, LBT-012 | Reject unclassified libraries and classifications for libraries absent from metadata. `unclassified_library_fails_closed`, `exemptions_need_a_reason_and_stale_policy_entries_fail` |
| ARCH-002 | LBT-003, LBT-005 | Exact direct-dependency allowlist by real package name and normal/build/dev kind, including optional/target-specific declarations. `normal_optional_target_build_and_dev_dependencies_are_checked_by_real_name` |
| ARCH-003 | LBT-002 | Required public trait path and non-empty required-method set; comments, private/conditional/default-only/empty declarations do not satisfy it. `missing_private_empty_default_only_and_comment_traits_do_not_satisfy_contract`, `an_empty_trait_policy_is_not_an_exemption`, `public_out_of_line_modules_are_followed_but_orphan_files_are_not` |
| ARCH-004 | LBT-002 | An implementation classification must name a contract package/trait and public implementation type, with a normal dependency and explicit production trait impl. `implementation_must_implement_the_named_contract`, `missing_contract_declaration_or_conformance_suite_fails`, `a_renamed_contract_dependency_can_be_implemented_explicitly` |
| ARCH-005 | LBT-007, LBT-009 | Implementation/integration libraries must name conformance targets; each declared target must exist and contain an unignored, unconditional `#[test]`. `missing_contract_declaration_or_conformance_suite_fails`, `ignored_tests_and_conditional_impls_are_not_evidence` |
| ARCH-006 | LBT-003 | Contract/pure/protocol/implementation libraries cannot depend on classified implementation/integration/harness packages, even through an allowlisted dev/build edge. `contract_cannot_depend_on_implementation_even_if_allowlisted`, `implementation_to_implementation_dependency_is_rejected_even_if_allowlisted` |
| ARCH-007 | LBT-001, LBT-012 | A non-service role needs a non-empty rationale. Exact exceptions and review ownership remain a review obligation. `exemptions_need_a_reason_and_stale_policy_entries_fail` |

The positive fixture is `valid_interface_implementation_and_conformance_target_pass`; malformed policy/source is covered by `malformed_policy_and_source_are_errors_not_silent_success`. Tests live in `tools/architecture-check/tests/boundaries.rs`.

## Policy format and review

Each library is keyed by its Cargo package name. Dependencies use `normal:package`, `build:package`, or `dev:package`. Renames are normalized using Cargo metadata, so aliases cannot hide a dependency. The allowlist covers declared direct dependencies, not only currently enabled features.

For a new replaceable service, use a `contract` library with a `traits` map from public module-qualified path to required method names. Use an `implementation` library with an `implements` entry naming `package`, `trait`, and `type`, plus `conformance_tests`. Implement the trait with an explicit qualified path, for example `impl reader_api::Reader for Engine`. Required methods must have no default body. An empty trait/method list is not an exception.

The syntax checker intentionally supports ordinary unconditional module declarations and explicit public struct/enum implementations. It does not resolve imported trait aliases, public re-exports, generated macro implementations, feature-conditional witnesses, or `#[path]` modules. Use supported explicit declarations or add tested checker/compiler support; do not disable the check to accommodate them. A required trait must be reachable through public modules from the actual library root, not merely present in an orphan file.

Current classifications preserve the frozen design:

- Protocol is a codec/data boundary, with corpus tests; no marker trait is required.
- Core is a pure typed state-machine boundary, with deterministic tests; no artificial trait is required.
- Node adapter is a transitional integration library containing eight consumer-owned port traits and existing conformance tests. Port extraction is proposed, not performed here.
- Simulator is a harness, not a production dependency.

Rationales and retirement/re-review conditions are recorded per package in the policy. The discovery maintainer owns review. Agents MUST NOT reclassify a service as `pure`, `protocol`, `integration`, or `harness`, weaken a required-method list, remove a suite, or add an allowed dependency merely to make CI green. Those changes require an explicit architectural review.

## Compiler and behavioral evidence

This lint does not prove Rust type resolution, public API non-leakage, conformance-test assertions, semantic usefulness, or third-party transitive dependencies. A syntactic `impl` must still compile against the actual trait. A test target containing `#[test]` must still execute meaningful assertions. Existing `p6_*` tests compile concrete test hosts against the actual ports and exercise their behavior, but do not establish that production Iroh/storage adapters exist.

Source scanning deliberately does not count conditional contracts/implementations; feature-specific APIs need explicit compiler witnesses and a feature matrix. Cargo also rejects dependency cycles in the actual build. More complete API/transitive graph checks are future adoption work, not capabilities of this initial lint.

No CI file can prevent an authorized writer from deleting both a rule and its checks. Required-check settings and review of policy/workflow changes are separate administrative safeguards.

## Fast test workflow

Examples, run from the repository root:

```sh
# Structure/dependency guard, without application compilation.
./scripts/check-architecture.sh

# A changed library or one targeted suite.
cargo test --locked -p glade-discover-core
cargo test --locked -p glade-discover-node-adapter --test p6_transport

# Only when working on the guard itself.
cargo test --locked --manifest-path tools/architecture-check/Cargo.toml
cargo clippy --locked --manifest-path tools/architecture-check/Cargo.toml --all-targets -- -D warnings
```

Use the owning package's relevant tests during RED/GREEN. Add affected-consumer tests for contract changes. Existing full-workspace, adversarial and fuzz jobs remain separate assurance gates, not the required command for every local edit. The checker does not yet select affected consumers or enforce wall-time budgets; measure warm test execution, incremental compile+test, and cold build independently before setting package-specific thresholds.

## Initial verification evidence

On 2026-09-05 in the local macOS workspace:

- TDD: initial guard tests failed before the checker existed; the later implementation-to-implementation dependency regression also failed before its check was added.
- All 14 guard tests passed on Rust 1.85.0 and the local 1.96.0 toolchain. Guard clippy and formatting checks passed on both.
- The actual workspace policy passed; a warm `check-architecture.sh` run measured 0.58 seconds wall time with the local default toolchain. The Rust 1.85 tool dependency build reported 6.38 seconds. These are observations, not universal budgets or CI measurements.
- 45 existing node-adapter tests and four core public-contract tests passed. The pinned design hash and requirement trace checks passed.
- The primary-workspace metadata step succeeded with an empty temporary Cargo cache and offline mode; no application dependencies were fetched for metadata inspection.

The full simulation/fuzz suite and hosted CI were not run for this tooling/documentation change. No production implementation was changed. Required branch-protection settings and wider repository adoption remain separate work.
