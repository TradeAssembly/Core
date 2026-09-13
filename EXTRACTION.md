# Isolated F2 Core extraction experiment

Not a published or qualified release. Source snapshot includes authorized dirty
F2 work from /Users/davidjbeveridge/.codex/worktrees/f2-onboarding-repair at base
2c9b1469adc4afbfdfbdaa240db52736896a06a4. The build revision environment records
that base, not a claim that the extracted bytes equal the original commit.

Frozen selection: Git cached and untracked/nonignored files under Cargo.toml,
Cargo.lock, LICENSE, .cargo/config.toml, identity-sdk, plugin-sdk, runtime-rs,
sightline-sidecar, xtask, plugin-contracts, packaging/alpaca.json. Copied with
git ls-files and tar, preserving the original checkout and excluding ignored state.
No webapp, Product, Hub, deployment infrastructure or sibling provider source.

Compile-time asset closure adds docs/reference/contracts/strategy-spec.schema.json
and examples/strategy-spec/v3/valid/{crypto-spot-24x7,multi-leg-option-spread,
static-equity}.json. Initial isolated build85128 exposed these four parent-relative
includes in runtime-rs/src/spec.rs; copy only the required schema/fixtures.

The existing xtask is retained initially to preserve lockfile and harness code;
its Studio-specific commands must be separated before this becomes public Core.
Standalone packaging closure additionally includes packaging/sandbox and
docs/reference/local-binary-setup.md. Existing bundle-local uses those pinned
inputs without webapp. Candidate target/f2-local-candidate was assembled in
session22135, explicitly unsigned/not notarized/not release ready. Its manifest
SHA256 is efb60698f9caa93977276a1738e80c01c145cf2c1cd7096b40fde62b533e2f9c.
Real packaged-sandbox test5025 passed: preserved arguments, denied private-file
read/write and loopback network, real Alpaca capability discovery without host
Node or broker credentials. Fresh empty-env setup89938 reports prepared and
sandboxConfigured, while policyRunning/automationReady remain false.
The owner-approved Apache-2.0 grant is now applied to Core LICENSE with attribution
in NOTICE. docs/licensing.md scopes the grant and remaining distribution audit.
No other repository or third-party component is relicensed; no publication.

Core xtask now routes verify/setup through a fixed Core registry; original mixed
router is preserved outside Core at /private/tmp/tradeassembly-core-preserved-studio-router-20260913.rs.
Twenty-four other uncompiled tooling copies were compared byte-for-byte against
their preserved Studio originals before removal; docs/removed-studio-tooling.json
records their hashes. Core keeps only compiled owning validators. Studio-owned
full_parity_gap_closure and UI contract checks remain unchanged in Studio;
docs/test-ownership.md records passing owner checks and the split. Core retains
runtime payload compatibility. Candidate nextest24780 and cargo test58831 each
pass1170 tests,45 ignored; nextest reports one leaky result requiring follow-up.
Formatting, strict workspace Clippy, deny/audit/machete also pass with documented
dependency warnings. M2 relocation real controlled-boundary suite passes25/0/0.
Whitelist remains red for the uncommitted extraction; full gates not complete.

Fixture closure includes tests/fixtures/parity and the complete strategy-spec/v3
valid/invalid/migration corpus, all JSON. CLI/HTTP/GraphQL/MCP parity checks remain
enabled. MCP fixture explicitly adds existing live_mandate.issue/revoke/status
tools; no automatic snapshot regeneration. Parity4 and strategy corpus11 pass.
Public-boundary negative fixtures now exercise permitted Core test/packaging paths
because webapp/product paths are rejected earlier as forbidden ownership. All11
boundary tests pass; no forbidden-marker or path protection removed.

Remaining qualification includes clean producer revision, Core-only archive smoke,
actual provider and user-agent journeys, Studio pinned consumption, and release
artifact/source/notices qualification. No individual green gate is a release verdict.
