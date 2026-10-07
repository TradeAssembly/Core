# F2 Core extraction experiment

Use the canonical frozen F2 M0–M8 plan and M3 extraction packet in
/Users/davidjbeveridge/.codex/worktrees/f2-onboarding-repair/docs/planning/plans/.
This is the codex/f2-core-extraction issue branch. A private local checkpoint
commit is permitted after source/secret/boundary checks to enable clean-revision
archive qualification. It is not a merge/release approval. Do not publish, change
producer pins or declare release readiness until owning gates are qualified.
Use Rust and cargo xtask/just command surfaces. Preserve original source; no
broker orders, hosted implementations, secrets, generated trade advice, or Live
policy activation. Existing M2 acceptance must survive relocation unchanged.

October 3 recovery execution: the active root and independent implementation
streams use the explicitly selected GPT-6.1 Sol High; stuck diagnosis uses one
Astra Medium handoff. CI/deterministic runners own execution/results and failed-job
resume. No automatic review or model-routing chain. Core docs/distribution.md is
canonical; coordinator RELEASE_CURRENT.md is the one operational checkpoint.
`cargo xtask verify` requires clean source before expensive work and uses one
full nextest unit/integration pass plus explicit workspace doctests, preserving
all scanners/security and separate release opt-ins. `verify --dev` explicitly
permits dirty development but is never release qualification. See the harness
coverage amendment; do not restore duplicate full-suite execution.
EXTRACTION.md identifies the selected source and unresolved harness/license work.

Distribution: `cargo xtask distribution-pack` owns frozen-payload packaging;
`cargo xtask distribution-verify` owns the versioned evidence matrix. The
first-release schema-v2 policy fully qualifies Mac arm64 and requires
digest-bound experimental build evidence, not native runtime acceptance, for
Windows x64 and GNU Linux x64/arm64; Intel Mac is excluded. The historical
schema-v1 five-target contract remains readable for baseline evidence.
`cargo xtask distribution-capture-registry` obtains read-only npm registry
evidence after candidate qualification; it does not publish packages.
`cargo xtask distribution-qualify-registry --evidence ABSOLUTE_DIR
--baseline-package ABSOLUTE_FROZEN_MAC_PACKAGE --source ABSOLUTE_CORE_CHECKOUT`
runs real npm/pnpm registry installs and the existing frozen-baseline upgrade/
rollback test on Mac ARM64. Its separate `registry/qualification.json` preserves
native receipts; failed/skipped tests and changed input/source bindings fail.
`cargo xtask distribution-freeze-native` inventories staged native inputs on
their actual host; its output is explicitly not native qualification.
The narrowly whitelisted `packaging/npm/cli.cjs` is native-launch glue only.
No install scripts, source compilation, re-signing frozen bytes, unsandboxed
fallback, Apple payment, automatic strategy activation, or active-rig upgrades.

`cargo xtask oauth-fault-proof --descriptor ABSOLUTE_DESCRIPTOR --evidence
ABSOLUTE_NEW_DIRECTORY` is additive frozen-source fault evidence only. Preserve
qualified native bytes and production proof ownership; never label its test
identity/transport boundaries as deployed authentication or full onboarding.

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

## Targeted Rust iteration

Use `just build-target PACKAGE BINARY` and `just test-package PACKAGE` during
iteration. `just build-artifacts` builds the exact development outputs consumed
by local CI; `just build` retains all-workspace behavior. Cargo owns dependency
selection and caches. Full `just verify` remains required at integration.
