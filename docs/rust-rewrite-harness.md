# Core qualification harness

Core's owning commands are `just` and `cargo xtask`. Product owns integrated
release decisions; Studio owns its frontend and presentation contracts.

`cargo xtask verify` (also `just verify`) runs the fixed fail-fast registry in
xtask/src/core_verify.rs: formatting, strict workspace Clippy, pinned standalone
sandbox prerequisite installation, workspace tests and nextest, deny, audit,
machete, language/source-cleanliness, plugin contracts, Core architecture,
public-source scanning and Core publication boundary/license checks.

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
