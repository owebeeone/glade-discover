# glade-discover

`glade-discover` is the pure discovery state machine for Glade. Its contract is
the pinned v3.1 semantic freeze in
[`dev-docs/GladeDiscoveryDesign.md`](dev-docs/GladeDiscoveryDesign.md).

The core boundary is:

```text
step(state, ctx, event) -> (state', effects[])
```

The core MUST NOT read clocks, perform I/O, access signing keys, use ambient
randomness, or call the network. Those actions are represented only by typed
events and effects. The simulator supplies deterministic contexts and delivery
schedules; `glade-discover-node-adapter` performs trusted external actions
through injected host traits and commits durable state before effects.

## Development

New opt-in, runtime-independent host traits and reusable tests are described in
[Draft host contracts](dev-docs/DraftHostContracts.md). They do not replace the
existing adapter or demo. The follow-up [registry contract draft](dev-docs/RegistryContractDraft.md)
adds acceptance, registry, trust and placement boundaries. Run
`sh scripts/check-contracts.sh` for all seven draft contract crates, or pass
`transport`, `signature`, `store`, `acceptance`, `registry`, `trust`, or `placement`
for one package. Also test affected consumers when changing a contract (for
example, run `registry` after changing `acceptance`).
The script enables the otherwise opt-in conformance tests explicitly.

Run the fast architecture gate when changing a public boundary, library, or
dependency:

```sh
./scripts/check-architecture.sh
```

The gate rejects unclassified workspace libraries, unexpected dependency edges,
missing named trait contracts/implementations, and missing conformance targets.
It is a separate tool workspace: normal core/package tests do not compile it.
See [Library boundary checks](dev-docs/LibraryBoundaryChecks.md) for coverage,
exceptions, fast-test selection, and the shared policy reference.

For the ordinary edit loop, select the affected package/test, for example:

```sh
cargo test --locked -p glade-discover-core --test public_contract
cargo test --locked -p glade-discover-node-adapter --test p6_transport
```

These narrow examples are not a substitute for affected-consumer tests after a
contract change. The workspace commands below remain the full verification gate.

Run the P0 provenance and trace checks:

```sh
./scripts/check-design-snapshot.sh
./scripts/check-requirement-trace.sh
```

Run the standard Rust gate:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

Run the same workspace gate on the declared MSRV:

```sh
cargo +1.85.0 fmt --all -- --check
cargo +1.85.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.85.0 test --locked --workspace
```

P5 hardening tests run as part of the workspace suite. The reusable
coverage-guided targets live in the nested `fuzz` workspace. With `cargo-fuzz`
installed, run:

```sh
cargo +nightly-2026-02-28 fuzz run protocol_decode
cargo +nightly-2026-02-28 fuzz run scenario_decode
```

CI compiles every nested fuzz target without starting an unbounded campaign:

```sh
cargo +nightly-2026-02-28 check --locked --manifest-path fuzz/Cargo.toml --bins
```

Development is TDD-first. Every behavior begins with a failing test or typed RED
scenario. See [`dev-docs/GladeDiscoverPlan.md`](dev-docs/GladeDiscoverPlan.md)
for phased ownership and integration gates.
