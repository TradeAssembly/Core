# Core SDLC

Use `just` / `cargo xtask` for local Core commands. `scripts/sdlc/verify` is a thin
wrapper; Product owns the top-level release/control-plane SDLC.

Work on an issue branch, preserve unrelated changes, and use bounded packets with
explicit acceptance evidence. Runtime changes need targeted behavior tests;
integration requires the full registry in docs/rust-rewrite-harness.md. Review
subprocesses are manual-only. Deterministic checks do not spend model review turns.

Record failures honestly. Ignored tests, synthetic fixtures and account-specific
setup cannot stand in for the required real-boundary/customer/release evidence.
Local checkpoint commits may precede clean-source gates; all owning merge and
release gates remain mandatory before publication or consumer pin updates.
