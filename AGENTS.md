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
