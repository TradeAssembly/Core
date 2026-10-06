# Core qualification harness

Core's owning commands are `just` and `cargo xtask`. Product owns integrated
release decisions; Studio owns its frontend and presentation contracts.

The additive `cargo xtask oauth-fault-proof` gate archives the exact frozen Mac
Core revision and Cargo lock, permits only an exactly removable two-hook
`cfg(test)` overlay and isolated test support, and runs five exact non-skipped
behavioral assertions. It binds raw logs/source/toolchain and reports only
source-level faults. Production identity/HTTP, packaged fault execution, full
onboarding and release readiness remain false. Existing integration/release
gates are unchanged.

`cargo xtask distribution-freeze-native` validates and inventories explicitly
staged native inputs on the target host. It preserves the original Mac arm64
bundle and reports `qualified: false`; runtime enforcement receipts are separate.

`cargo xtask distribution-pack` packages an explicitly supplied frozen bundle
and parent lock; it never recompiles or signs that payload. The Rust
`tradeassembly-distribution` crate owns verification and installation. The only
additional executable JavaScript exemption is `packaging/npm/cli.cjs`, a thin
exact-version native-package selector without lifecycle hooks or business logic.
`cargo xtask distribution-verify --evidence DIRECTORY` reads the versioned
matrix. Historical schema-v1 requires five native receipts. The first-release
schema-v2 policy requires full Mac arm64 native and published npm/pnpm evidence;
Windows x64 and GNU Linux x64/arm64 require exact-target GitHub Actions, source,
artifact and registry-byte bindings but remain experimental and explicitly
unqualified for runtime acceptance. Intel Mac is excluded. Unit tests or local
tarballs are not publication evidence. `cargo xtask distribution-capture-registry
--evidence DIRECTORY` fetches the five published beta package tarballs and
metadata read-only from npm after candidate qualification, compares their bytes
to qualified artifacts, and stores sanitized evidence. It does not publish.
`cargo xtask distribution-extract-github-builds` verifies pinned GitHub run,
artifact and archive digests, plus equal Core `plugin-sdk` Git tree IDs at the
runtime and Alpaca SDK revisions in the explicit `--source` checkout, before
materializing only named Core, Warden and Alpaca outputs into a new directory.
Its output is binary transport marked
`qualified: false`; it does not produce a platform package or receipt.
`cargo xtask distribution-qualify` is the native-host capture entrypoint for
clean-source, exact-candidate installed-package, npm/pnpm, MCP and sandbox
process evidence. It seals the actual tested tarballs, installer, descriptor,
controlled broker and source revisions; an interrupted or failed run has no
receipt. The current Windows implementation fails closed until native
ACL/account/elevation/WFP checks are implemented and executed.

`cargo xtask verify` (also `just verify`) runs the fixed fail-fast registry in
xtask/src/core_verify.rs: formatting, strict workspace Clippy, pinned standalone
sandbox prerequisite installation, explicit workspace doctests and one full
nextest unit/integration execution, deny, audit,
machete, language/source-cleanliness, plugin contracts, Core architecture,
public-source scanning and Core publication boundary/license checks.

October 3 approved coverage amendment: `cargo test --workspace --doc --locked`
covers doctests; `cargo nextest run --workspace --locked` covers the same workspace
unit/integration binaries previously run twice. Ignored provider/sandbox/archive
release opt-ins still require their separate actual invocations. No coverage is
claimed from a skipped test. Cheap required-input, tool, committed-revision,
clean-source and Unix disk-headroom preflight runs before formatting/compilation.
The practical 2 GiB free-space floor catches known pressure, not cold-build sizing.
`cargo xtask verify --dev` preserves scans and test coverage while explicitly
allowing dirty source; it is not release qualification. Default `verify` requires
clean committed source, including nonignored untracked files. Auth/provider
availability preflight belongs to the owning release job, not public Core.

`cargo xtask onboarding-verify` is the targeted local browser-onboarding gate:
protected OAuth polling freshness/authority/redaction, OAuth binding/replay,
public connection profiles, browser state/HTTP controls, identity bootstrap,
and real stdio process restart/cancel. It does not prove deployed OAuth or
customer installation; those require the actual packaged browser journey.

No skipped, zero-test or mock-only result is end-to-end evidence. Ignored real
M2 controlled-broker/Warden and packaged-sandbox cases require their explicit
invocations and exact artifact-bound receipts. Never place actual broker orders
or activate Live policy during qualification.

`cargo xtask foss-core-boundary --archive-smoke` additionally requires a clean
committed revision and independently extracted source build/runtime proof. A
normal boundary scan does not imply the archive has passed. Apache-2.0 readiness
is scoped to Core's owned grant; exact third-party distribution notices and M7
signing/notarization remain separate requirements.

Source extraction preserves Studio's original tooling. Removed uncompiled copies
are enumerated in docs/removed-studio-tooling.json with SHA256 values; Studio
test ownership is in docs/test-ownership.md. Do not copy private deployment or
Studio lifecycle implementations back into Core to satisfy a Core gate.

No merge/release claim until all owning gates and the Product F2 locked release
verifier pass. A local checkpoint commit needed for archive/source-cleanliness
qualification is not a merge, pin, publication or release decision.
# Registry delivery acceptance

The unsigned first-release published gate requires separate process-derived
registry qualification; candidate-only evidence never suffices. Owning command:
`cargo xtask distribution-qualify-registry --evidence ABSOLUTE_DIR
--baseline-package ABSOLUTE_FROZEN_MAC_PACKAGE --source ABSOLUTE_CORE_CHECKOUT`.
It requires Mac ARM64, exact prior registry capture, a clean committed test
checkout, actual npm/pnpm installs with scripts disabled, and real frozen-baseline
upgrade/rollback/state preservation against the captured native registry package.
Fixed tests must execute and pass; ignored/skipped/zero-test summaries fail.
The separate receipt binds native/registry inputs, test sources and logs.
Full Core gates and production/CI acceptance remain separate mandatory gates.

## Local CI setup phase

`just setup` prepares locked dependencies; `just check` runs developer feedback;
`just build` produces local native binaries; `just verify` preserves the complete
clean-source gate. None requires Codex or a CI-provider run. The coordinator's
local CI manifest owns separate integration tests and records gaps; no deployment
or product repair is authorized by a passing local baseline.

## Launch compatibility gate

`just contract-test` checks the owning interface behavior.
`contracts/compatibility.json` declares independently versioned launch contracts.
Compatibility does not waive artifact integrity, expected deployment identity,
job/receipt replay bindings, or existing state/authority migration restrictions.
The coordinator owns the finite cross-component gate and exact release lock;
its policy is `docs/compatibility.md`. No provider or agent acceptance is required
for this local gate, and passing it is not production release readiness.

The Core runner exposes read-only `--compatibility` without altering the existing
`--build-identity` response. Core remains independent of private repositories.
