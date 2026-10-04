# Core SDLC

Use `just` / `cargo xtask` for local Core commands. `scripts/sdlc/verify` is a thin
wrapper; Product owns the top-level release/control-plane SDLC.

The October 3 recovery uses one full nextest unit/integration pass plus explicit
workspace doctests instead of executing that full suite twice. Release `verify`
first requires clean committed source, required inputs/tools and practical Unix
storage headroom; `verify --dev` keeps coverage/scans but permits dirty development
and cannot establish release qualification. CI/deterministic runners own logs,
exit status and resume. The selected Sol High owners diagnose and repair; one
Astra Medium diagnosis is available for stuck work. Core docs/distribution.md is
canonical; coordinator RELEASE_CURRENT.md is the only live checkpoint.

`cargo xtask oauth-fault-proof --descriptor ABSOLUTE_DESCRIPTOR --evidence
ABSOLUTE_NEW_DIRECTORY` owns the finite frozen-source OAuth fault complement.
It runs exact named behavioral assertions against archived source and writes
hash-bound raw logs. It never rebuilds a shipping payload or replaces production
signup/login/OAuth evidence. Time/output bounds and a failed named assertion
stop capture without a passing proof.

Distribution work uses `cargo xtask distribution-pack`, native-host
`cargo xtask distribution-qualify`, and `cargo xtask distribution-verify`.
Qualification requires a clean committed test harness, runs the actual installed
package, npm/pnpm, MCP and sandbox drivers, and writes a receipt only after
their nonzero executed-test gates pass. Windows additionally needs dedicated
ACL/account/elevation/WFP proof before a receipt can exist. Targeted installer tests include real opt-in
frozen-binary acceptance; its environment variable and exact command are in
docs/distribution.md. Packaging a local npm tarball is not public registry delivery
or another platform's runtime/sandbox evidence. Keep the parent lock unchanged.
`cargo xtask distribution-freeze-native` inventories new native inputs on their
actual target host; freezing is not qualification or permission to publish.

Work on an issue branch, preserve unrelated changes, and use bounded packets with
explicit acceptance evidence. Runtime changes need targeted behavior tests;
integration requires the full registry in docs/rust-rewrite-harness.md. Review
subprocesses are manual-only. Deterministic checks do not spend model review turns.

Record failures honestly. Ignored tests, synthetic fixtures and account-specific
setup cannot stand in for the required real-boundary/customer/release evidence.
Local checkpoint commits may precede clean-source gates; all owning merge and
release gates remain mandatory before publication or consumer pin updates.

# Registry release checkpoint

After publishing exact qualified tarballs, use `cargo xtask
distribution-capture-registry --evidence ABSOLUTE_DIR`, then `cargo xtask
distribution-qualify-registry --evidence ABSOLUTE_DIR --baseline-package
ABSOLUTE_FROZEN_MAC_PACKAGE --source ABSOLUTE_CORE_CHECKOUT` on Mac ARM64.
The latter runs actual opt-in registry and frozen-baseline installer tests,
not mock evidence. Require the default `distribution-verify` afterward.
Keep failed logs, native receipts and frozen bytes; no overwrite or automatic
republish is part of qualification. Release CI must retain these distinct proofs.
