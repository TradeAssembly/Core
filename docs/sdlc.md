# Core SDLC

Use `just` / `cargo xtask` for local Core commands. `scripts/sdlc/verify` is a thin
wrapper; Product owns the top-level release/control-plane SDLC.

Distribution work uses `cargo xtask distribution-pack` and
`cargo xtask distribution-verify`. Targeted installer tests include real opt-in
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
