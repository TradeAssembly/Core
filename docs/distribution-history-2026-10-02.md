# Historical F2 distribution plan and evidence — archived 2026-10-02

This preserves the previous plan and evidence below; it is not an execution
queue. Current scope and next action are in [distribution.md](distribution.md).
Superseded five-target, CodeBuild, packaging-only, paid-launch-exclusion and
signed-release instructions here do not override the current unsigned launch.

The canonical F2 plan remains Product's
`product/release-plans/2026-09-23-f2-implementation-completion-plan.md`.
This document specifies its separately qualified distribution layer and the
bounded runtime repairs required to qualify that distribution. The amendment
below controls older packaging-only instructions. No Apple payment, notarization,
broker orders, or trading activation is authorized by an install or upgrade.

## Approved first-release scope — 2026-09-30

### Build-provider amendment — 2026-10-01

The user selected **builds on GitHub**. Native producer builds and release
artifacts must be produced by GitHub Actions workflows; the AWS CodeBuild
project/source-archive path below is historical planning, not the execution
route. This amendment does not change the four-target acceptance matrix or
authorize making private producer repositories public. Build receipts must bind
the exact Core/Warden/Alpaca commits, GitHub run and attempt, native runner,
artifact identity and digest, downloaded archive bytes, and extracted binary
inventory; the existing CodeBuild-only receipt schema is not acceptable as a
substitute. No release gate may be marked passed by merely dispatching a run.

Core is public and its standard GitHub-hosted runners are free. Warden and
Alpaca are private, and the organization has a $0 Actions overage budget with
`prevent_further_usage=true`. Preserve that cap. The September private jobs
failed before a runner started, but an October 1 Warden job reached a real
GitHub-hosted runner after the monthly allowance reset. Use standard
GitHub-hosted runners within the included allowance for this release. Stop
before any overage; do not silently raise the cap. If included minutes run out,
stop and make a separate, explicit build-runner decision. Do not silently
switch to CodeBuild or raise the Actions cap. Public visibility is only possible
after the
mandatory secret/history and binary/license/hosted-boundary review below.
Record real runner and billing readbacks before claiming a producer build.

October 1 checkpoint: pinned GitHub Actions native builds passed on actual
GitHub-hosted runners for all three experimental first-release targets and
all three producers:

| Target | Core run | Warden run | Alpaca run |
| --- | --- | --- | --- |
| GNU Linux x64 | `36907697699` | `36907884275` | `36907966575` |
| GNU Linux arm64 | `36909322770` | `36909194724` | `36909194587` |
| Windows x64 | `36909322597` | `36914759668` | `36909194467` |

Core source is `6637e6f7ebe21189bbde090f66d83ad8e6ed70a1` on build-only
branch `codex/f2-native-build-input-6637e6f`; Alpaca source is `908b5e3` and
pins that exact Core SDK. Warden source is `dade4e6` on Linux and `0b75f20`
on Windows after native Windows private-file and export-path repairs. The
private Rust capture tool validated GitHub run/job/artifact records and actual
downloaded ZIP digests. Core's deterministic `verify-github-builds` passed
against the captured readbacks and archives for all three targets, including
a wrong-source negative check. Core `just verify` and Warden's owning local
gate passed at their current source heads. GitHub's October Actions usage
readback showed $0 net charges; the $0 overage stop remains in force.

This is **native producer build provenance only**. It does not qualify a
source-free installation, Windows/Linux runtime behavior, the assembled
candidate, Mac arm64 acceptance, npm registry bytes, or a customer release.
The new `cargo xtask distribution-extract-github-builds` path was run against
all three recorded experimental target archives. It validated pinned run and
archive provenance, extracted only five named outputs per target, checked
executable architecture, and wrote hash inventories under ignored
`target/github-build-outputs-{linux-x64,linux-arm64,windows-x64}`. A wrong-Core
revision exited 1 without creating an output directory. These are unqualified
build inputs, not package or install evidence. The original frozen Mac payload
and parent lock were not changed.
The three opt-in producer workflows now offer the standard `macos-15` arm64
GitHub runner. Warden run `36918705772`, Alpaca run `36918717106`, and
corrected Core run `36921808794` passed. Their actual archives were captured
with GitHub run/job/artifact readbacks and matching ZIP digests. Alpaca's package hash
is `828a71a4c563650da4312799cb3b8ee8adf1a26ed4c846429754b0167ff53d27`.
The first Core run `36918694751` predates this pin and is diagnostic only:
its embedded default-plugin hash cannot install that Alpaca package. The
corrected Core commit `a274deeae4426b6c1cb4a33849aa458da7da0b24` embeds
the matching pin and passed clean-worktree `just verify` before GitHub build.
The Alpaca workflow uses Core SDK commit `d3e536f`; the verifier established
both commits have the identical `plugin-sdk` tree
`1e97d7e09ff4c20bc8c099474dfd5dbdcddd72b7`.
`cargo xtask distribution-extract-github-builds` passed for arm64 Mac and
materialized seven named, hash-inventoried outputs under ignored
`target/github-build-outputs-macos-arm64`; native binaries were identified as
arm64 Mach-O. This is binary transport with `qualified: false`. The Mac GitHub
build inputs are distinct from the
existing locally built and qualified `0.1.0-beta.2` Mac candidate; changing
the Mac binary inputs requires a fresh native Mac candidate and qualification.
The GitHub receipt verifier accepts `macos-15` only for arm64 Mac and rejects
that runner for Linux. No replacement qualification is claimed by completing
producer builds.
October 1 source-free diagnostic: `cargo xtask bundle-local` assembled a
separate unsigned Mac bundle from the verified GitHub Core, Warden and Alpaca
outputs plus the SHA-pinned Node archive. With no connection profile it is a
**local diagnostic**, not the production Relay candidate. The explicit ignored
`real_binary_setup_and_policy_service_work_without_source_or_path` test passed
against its actual bundled Core and Warden binaries: local setup, real policy
service, offline Alpaca installation and stdio MCP tool inventory succeeded.
No broker order was sent. The frozen baseline was not modified.

The Mac upgrade path is not qualified yet. The baseline Warden digest is
`84738d401b7442c743de7fe736d61199a82fe1f89c405a0e0135cc1bd915acce`;
the GitHub-built Warden digest is
`3717756a40393f88c2cb7c5edd64d5f30747bf56befb4d53340f97d85cf789de`.
The attempted transition to `3717756a...` is **not compatible**: the actual
GitHub authority exits with `database_integrity_witness_missing` on the frozen
state. The frozen binary predates that external witness/key-head format. The
previous claim that its source was `e5b926c` was incorrect; the Core source lock
pins `ffa6889b141f881fb594c1c46e3c83180c2998ef`. Do not synthesize a witness,
reset policy state, relax the new storage checks, or qualify that failed pair.

For this bounded packaging release, build the locked Mac authority on GitHub,
with only four iterator-style Clippy repairs in its storage source, preserving
its schema and integrity formats. The build-only producer branch is
`codex/f2-mac-frozen-authority`; its first native run `36928230162` failed on
those four Clippy diagnostics and is terminal. The amended workflow binds the
storage file SHA `1899cf62b7d77f0061937fe4fc68dd371842dd1a474e59412bb12e601f7b97cd`,
rejects other source differences from the lock, and runs the owning full gate.
The existing experimental Linux/Windows outputs remain separately pinned.
Adopting the newer authority storage format needs its own migration and is not
required to recreate the frozen Mac contract in a GitHub build.
The amended source is `be9ac371e1e27d04ef08169ec6eef49e2bafd521`.
Its local `cargo --locked xtask verify` exited 0, including 115 Rust tests,
22 SDK tests, strict Clippy, dependency advisories/licenses and the public
boundary gate; full output is `target/warden-frozen-authority-verify.log` in
this Core worktree. GitHub run `36928911139` completed successfully. The private
capture tool verified its source, native runner and artifact `11194953937`;
the downloaded ZIP SHA-256 is
`7750f77a9d3d232da2314b51cc1dc045782785325c982570c586d51cffa9c6ae`.
Core's `distribution-extract-github-builds` passed and identified the extracted
authority as arm64 Mach-O with SHA-256
`1fb42d13f2ef46f3232daddfa6358553218978832501df9c8299aa3ee02878f8`.
Its hash inventory is `target/github-build-outputs-macos-compatible/build-outputs.json`.
This is verified build provenance, not upgrade qualification.

R3 still needs an exact-pair, stopped-rig, resumable binary transition with real
state/rollback proof. The source now pins the retrieved `1fb42d13...` digest;
`3717756a...` is not an enabled release pair. Admit
only the frozen and qualified replacement digest, in either direction, after
stopped-rig checks and full verification of both immutable
payloads. Journal the target in `pending.json` before changing the stable
authority binary; on retry accept only the pinned old or new digest and complete
the same transaction. Update the local installation hash and sandbox binding
without resetting identity, keys, journal or Warden SQLite state. Verify old
Warden -> new Warden -> old Warden against the same disposable state, including
interrupted swap/retry and active-rig denial, before enabling the pair in a
candidate. No cross-platform authority migration is implied. Do not weaken the
general digest gate or call a fresh install an upgrade. The Warden source delta
is insufficient evidence of compatibility. The real test also exposed a
macOS `/var` versus `/private/var` executable-path alias: process inspection now
resolves both sides before comparing them. Recovery after the pointer commit
and before pending-journal cleanup must retain the previous slot; both upgrade
and rollback have explicit acceptance for that crash boundary. These installer
changes remain unqualified until the real tests pass with bound package bytes.
Current installer-development evidence: both ignored `packaged_install`
drivers pass against the separately repacked `target/f2-npm-mac-recovery-diagnostic`
fixture, with unchanged frozen Warden bytes. They preserve Core state,
credential fingerprints and original signed Warden receipts, including the
post-pointer crash cleanup. All 30 distribution unit tests and strict
distribution Clippy pass. This is recovery evidence, not acceptance of the new
authority digest or a customer candidate. The coherent Core repair is in
`distribution/src/install.rs`, `distribution/tests/packaged_install.rs` and
this document. The exact-pair real-binary test now passes against
`target/f2-npm-mac-package-github-compatible/tradeassembly-darwin-arm64`:
baseline install, running-policy/active-execution denial, interrupted authority
and metadata writes, retry, upgrade, rollback, post-pointer cleanup, retained
Core row, owner configuration, credential fingerprints and original signed
Warden receipts. `target/github-compatible-upgrade.log` records 1/1 passing in
19.95 seconds (SHA-256
`28c669b7c7dec9f66f78d1d8e6d3ae48c4c2742fd7013379769230508dbc55f2`).
The package is local diagnostic beta.3: manifest
`4650620f56bb3b0cf2272773c997fd95d3eaa5c84a9bcb4d163ae0315e77aacd`,
archive `7df3c8adaa44d8bb732df681ae3910ee37afdf451cf468785a5f407e2bb11264`,
descriptor `001defa070027526f815d704ce67a3f72c75f803ee1fea1b7c8e941ad709c61f`,
locally built installer
`ef14241cf6b978af78461b367d210a32380cf9315ad63b8031ae5bb789e71c3b`.
The runtime, authority and Alpaca bytes are GitHub-built, but this local installer
does not satisfy customer build provenance. The final package must bind the
GitHub-built committed installer and rerun qualification. The frozen parent
lock remains `222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`.
All 30 distribution unit tests and strict Clippy pass after pinning the verified
authority. Install/reinstall, packaging-version upgrade/rollback and running-rig
denial also pass (1/1 in 9.02 seconds), recorded in
`target/github-compatible-install.log` (SHA-256
`99f53bd64666a0f3faa433a19ae371717b958e91df0a1ff32977bf27eef3f429`).
The repair is committed and pushed at
`6b0aa2e37a722f1db861b2725567961cf4de01f0`. The owning full clean gate
`CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 just verify`
exited 0, including all 1,336 Nextest tests. Log:
`target/github-compatible-core-verify-lowdisk.log`, SHA-256
`45119f0fe6c2dc6a4c4fcb6bb2d26ab934e0bdfbb192a1214d0be47a94dd5a59`.
Earlier attempts exhausted local compiler-cache disk space; only reproducible
Cargo debug outputs were removed. Frozen payloads and evidence were preserved.
The default gate reports `archiveSmoke: not_requested`; the separate
`cargo xtask foss-core-boundary --archive-smoke` exited 0 against the clean
`6b0aa2e` archive, reporting `archiveSmoke: passed`, including isolated archive
compilation and runtime MCP discovery. Log:
`target/github-compatible-archive-smoke.log`, SHA-256
`b82cc57d1929f30091c84b605b75139303974c9f424fd2798f694a9bb3e53c8f`.
GitHub native Mac Core run `36932664901` passed. Capture verified artifact
`11196253815`, ZIP SHA-256
`e1e61625c877fa4d4998be18975f227222a3bb1f98cda48f636224e7bd608d91`;
Core extraction and SDK-tree checks passed. Installer SHA-256 is
`8e710d33a2b693776fab5190e262e866763935e4da4f94ddea2eff3f0a07904e`.
The private capture tool is checkpointed at `8e8b321` with this exact source pin;
2/2 capture tests pass. The fresh local-profile beta.3 package is
`target/f2-npm-mac-package-github-installer`; manifest
`d1bbac6831464b4a26bcd08d5c7fdce1188f81d3058c9bb782f5b4c5254f4b6b`,
archive `cda53e873b5ab6354ab1e8c7865c9f804ad8cc9257879c17f541b9421583d2ad`,
descriptor `8db8d8881ea1e55ecf57597ccef69be60df5ee4e3b51403a5754db3e3ad6dc8e`.
Its complete native qualifier failed the Paper ambiguous-outcome case (3/4
order cases passed); no qualification receipt was created. Setup, baseline
upgrade/rollback, npm/pnpm and sandbox checks passed. Failed evidence is retained
under `target/f2-npm-github-mac-evidence/aarch64-apple-darwin` and
`target/github-mac-qualification.log`. This package is **not qualified**.

Bounded diagnosis reproduced the final distinct-order denial against those
same GitHub runtime/authority bytes. Its durable intent and C5 allow decision
exist, but post-C5 admission rejects a same-owner, same-fence lease renewal:
intent expiry `1790893681127`, current expiry `1790893682488`. The isolated
diagnostic database remains at the path recorded in
`target/github-mac-paper-diagnostic.log`; no customer state was used.
Decision: renewal may extend the live lease, never the lifetime of the signed
intent. Compare every intent field exactly except normalize a same-generation
renewed expiry to the original; reject shortened leases, changed owner/fence/
resource, expired original deadline, or any other configuration/risk/mandate/
package/order change. Both pre-C5 and post-C5 rechecks use this rule. The new
focused negative-matrix unit test passes. The corrected local runtime also
passes the formerly failing actual Paper stdio MCP/real Warden/controlled-sink
case (1/1 in 111.34 seconds), with duplicate, crash, absent-outcome and durable
receipt assertions unchanged. Log: `target/github-mac-paper-renewal-fix.log`,
SHA-256 `f9b477d8d2214ea19da756e7d78d7441bb980169d1f9396c288e7d9198c78b71`.
Strict runtime all-targets Clippy, `cargo xtask scan-public` and
`git diff --check` exit 0. This local diagnostic does not qualify the previous
GitHub package or any changed shipping bytes.
Only `runtime-rs/src/broker_submission/admission.rs` is changed; the temporary
test-state retention edit was removed. Next: checkpoint this coherent repair,
run the clean owning gate and separate source-archive smoke, then push and
rebuild on GitHub, assemble a separate package and rerun full qualification.
Do not reuse the failed package's component logs to qualify changed bytes.
The repair is committed and pushed at
`ee33757f98a9781cf18c6fc36037dccecda549b3`. Clean `just verify` exited 0
(1,337/1,337 Nextest tests), followed by a separate successful isolated
`cargo xtask foss-core-boundary --archive-smoke`. Logs and SHA-256:
`target/github-lease-renewal-core-verify.log` =
`b99f993cf7718a2dd859c0ae8c08d65b00201641b8c7d5cecd951d0b7f2414c7`;
`target/github-lease-renewal-archive-smoke.log` =
`99e8e699d44520cf477eb0ca9a8d585bd69c519c0ff03c2abe85b8342c6a0f45`.
GitHub native Mac build `36937535332` passed against this revision. Capture
verified artifact `11198879853`, ZIP SHA-256
`b862f25d10205cc40498d5e515bf13380cbd38f1da0e0ac002d9feb0ae41023b`.
The private capture tool is checkpointed at `322e7de`; its two tests pass.
Core extraction and SDK binding passed under
`target/github-build-outputs-macos-lease-renewal`: Core SHA-256
`38915b9a372b8eb5da92d64cc1966802717ba575517b3cb47d26d0752c107d34`;
installer, sandbox, compatible Warden and Alpaca bytes match the prior verified
inputs. The separate `target/f2-npm-mac-package-lease-renewal` local-profile
beta.3 has manifest `1afedb0ee042d1866d17b706ff3d40eece1f8a2bbd7d1beacdd6a83333118023`,
archive `885115fec2d4f605ca460b90c770adc627b9ee5e3b5da1826cdebc728434ce41`,
descriptor `cb41d41897477b2a48961006559e4aee8460278f580739eb0250a570b8699422`.
Its **complete native Mac qualifier passes**, including the four real stdio
MCP/Warden/controlled-sink cases (4/4 in 132.86 seconds), source-free setup,
scripts-disabled npm/pnpm, baseline upgrade/rollback, state preservation,
interruption/active-rig denial and sandbox checks. No actual broker orders or
shipping Live activation occurred. Receipt:
`target/f2-npm-evidence-lease-renewal/aarch64-apple-darwin/receipt.json`, SHA-256
`fc3c410069666e863c04749385738ec72b8dc298570ea94506393e612f5961ab`;
qualifier log `target/github-lease-renewal-qualification.log`, SHA-256
`4eeeb9d76df2333e11f19158d052d94bdac2e9af9a37929e0d7a0e5288f14690`.
Qualification source is committed `2370ea11a6f89f0734f6e199bc84872a97c05755`.
The native evidence was moved without modification into the matrix's actual
target directory; its receipt hash is unchanged. Watches/tests/extraction are
terminal; do not rerun them without changed inputs. The original frozen parent
lock remains unchanged.

The actual `cargo xtask distribution-verify --candidate --evidence
/Users/davidjbeveridge/.codex/worktrees/f2-npm-distribution/target/f2-npm-evidence-lease-renewal`
exits 1 with
`first_release_production_profile_required:aarch64-apple-darwin`, recorded in
`target/github-lease-renewal-candidate-matrix-gate.log`. This is Mac component
qualification, **not customer release qualification**. Next: E0–E5 real
production/staging isolation and the customer profile, then bind that profile
to a separate bundle using these verified binaries and qualify those exact
bytes. Complete experimental native packages/receipts before the full matrix.
Experimental packaging follow-up is separate from that Mac qualification.
Use native GitHub runners for staged-input freezing and package production;
retain `native::freeze`'s `native_host_required` and `package::pack`'s
`pack_on_native_target_required` checks. The present assembler
`xtask/src/bundle_local.rs` only supports Mac arm64; native Windows/Linux
staging still needs target-bound Node/plugin pins, runtime dependencies and
notices. Cross-target **read-only verification**, however, must select paths
from the declared release target. The former host-dependent paths would reject
Windows payloads on a Mac verifier. The target-path helper is implemented in
`distribution/src/lib.rs` at local commit `2370ea1`; it does not bypass
installation or packing host gates:
all 31 distribution unit tests and strict all-targets distribution Clippy pass,
including five-target file inventory, hash tampering, wrong authority digest,
and a self-consistent Windows manifest with missing `.exe` paths. These are
parser/integrity tests, not Windows/Linux runtime qualification. They do not
change the existing GitHub run's source pin or qualify new installer bytes.
The next full owning gate remains required at the integration checkpoint.
Acceptance remains the full four-target candidate matrix, with native
package bytes and receipts, not extracted build ZIP inventories alone.
The npm access preflight remains unauthenticated; registry 404 does not prove
namespace ownership. Bitwarden credential use reached actual npm browser MFA:
the page requests a six-digit **authenticator device** code. Its Bitwarden item
has no TOTP/custom fields; authorized Gmail has no recent npm verification mail.
The Chrome Bitwarden overlay was handled once through bounded native controls;
no reset/recovery/security weakening was attempted. User input was requested
once and the MFA tab was marked for handoff. npm CLI web session `98779` is
terminal (exit 1); the browser sign-in can still establish its normal session,
after which a new CLI authorization must be verified. No npm token was
obtained or saved. Isolated temporary userconfig directory is
`/private/tmp/tradeassembly-npm-auth.qJw5BG`. Namespace verification remains open.
Nothing was published. Production Relay isolation,
experimental package assembly, candidate/registry verification, publication
and final integration remain open.

The October 1 AWS CLI readback still lists only the test Relay DSQL cluster and
the separate production Hub cluster, with the F2 conformance Relay Lambdas and
test archive bucket; it did not establish production Relay storage. The E0–E5
production/staging isolation gate below remains open. Do not freeze a
customer-facing Relay profile from the diagnostic bundle.
Next: close the production profile/isolation gate, assemble the versioned
four-target candidate from the selected outputs, check binary
architecture/inventory and notices, qualify the exact customer Mac bundle,
then satisfy candidate and registry verifiers. Do not mark those gates passed
from green GitHub jobs alone.

E0/E1 continuation, October 1: the current AWS CLI inventory confirms the
gateway points at the `test`/`f2-conformance` DSQL cluster and archive bucket;
the only other DSQL cluster belongs to production Hub. The staging OAuth
Lambda has no owner-policy configuration. `tradeassembly-prod` and
`tradeassembly-f2-conformance` SSO sessions are expired; existing `default`
credentials successfully completed the bounded read-only inventory, without
an auth restart. Partial sanitized inventory is excluded from Git under
`f2-relay-watch-remember/.operator/relay-environments/inventory.json`;
**E0 is not complete** (verified owner/nonowner identities, edge/IAM readbacks
and production resource/profile references remain).

The existing private `codex/f2-relay-watch-remember` worktree has committed
E1 changes at `841c0bd` in `packages/relay-environment-admission`, workspace/package locks,
OAuth library and both entrypoints, Gateway identity/enrollment/configuration,
Reach configuration/node authority, route-inventory/executable tests, OAuth
and conformance Gateway/Reach IaC, and owning `justfile`. Settled
decision: exact Hub-verified tenant/subject pairs, mandatory nonempty staging
allowlist, no caller email/header authority, no production owner list, and
callback recheck against the current deployed policy before claim/exchange.
Start/status/ack apply admission after identity verification but before store
access. The staging IaC plan and application startup fail on missing/empty
owner configuration; deployment requires an explicit private operator var-file.
`just relay-admission-test` exits 0: policy 4/4, OAuth 9/9, strict targeted
Clippy. Its actual router/SQLite regression observes no state creation for a
denied identity and no claim/delete/provider exchange after owner revocation.
Log `target/relay-environment-admission.log` SHA-256:
`a368a63c790cb916a88d6e79f948f5f05c4919bd34b99c2706be8f05439f63d9`.
OAuth/OpenTofu format, validation and `git diff --check` pass after installing
the already locked provider. These are local policy tests, not live deployment
proof. Gateway/Reach hosted configurations now require explicit
`environment_admission`; absent or empty staging configuration fails before
provider initialization. Verified Hub identity is checked before bindings,
commercial admission, enrollment and quota effects. Reach's node-authority
wrapper first verifies the existing Hub installation signature/owner binding,
then checks the bound canonical Hub principal before nonce/command mutation.
No caller-supplied principal is accepted in place of that proof. Existing
conformance IaC explicitly selects `test`; this does not qualify staging or
production deployment. Watch remains a separately configured operator-secret
surface with issued browser-ack proofs, not a newly added Hub-token surface.
The route inventory accounts for all 35 existing routes and their exact
handlers/boundaries or no-effect public exceptions; new/unclassified routes and
handler swaps fail its deterministic test. This source inventory does not
replace actual authentication/effect tests.

Local `aws-gateway,aws-reach` library suite passes 16/16, including isolated
real PostgreSQL enrollment/auth tests observing zero enrollment/quota effects
for valid same-tenant and different-tenant nonowners despite spoofed headers.
The node policy unit test observes zero Reach store mutations on denied
claim/accept/finish; its identity verifier is explicitly a fixture, not claimed
live or cryptographic acceptance. Route inventory 3/3, Reach suite 5/5 and
existing Reach package tests 2/2, policy 4/4 and strict combined-feature Clippy
pass. Required `just hosted-gateway-test hosted-hub-test relay-verify` exited 0;
OAuth all-feature tests pass 10/10 (two DynamoDB Local tests remain explicitly
ignored, not claimed passed). Logs and SHA-256:
`target/relay-environment-hosted-tests.log` =
`7ecfd68a46b7d334334f9fce8116c09ad5f84c19eaa6fd4e83e25398128d0c25`;
`target/relay-environment-route-inventory.log` =
`ec6509b1e1acbc01de658f76f14a96e8c3fb7d903247ab3bea530bd3522e802e`;
`target/relay-environment-owning-gates.log` =
`d3ed40d267f9c5fb2e80d578d816e7e0eeeca6f98d63220bc2f28c1a5e432814`;
`target/relay-environment-reach-gate.log` =
`9d01b562b8910de40ed4c682f4a34cfa9b22c3fd903586da5c546430dda06f7e`.
The first full `just verify` process `52310` is terminal exit 1: its preceding
checks passed, then the existing language gate rejected the uncommitted source.
No language gate was weakened. Failed log SHA-256:
`target/relay-environment-full-verify.log` =
`a9247481d86d4546d32adcdefd42dc76542bdf4ba9e27318e0c60199d96c116a`.
The coherent batch is now committed locally at `841c0bd`, and the worktree is
clean. Full `just verify` process `37106` is now terminal exit 1. Workspace
Clippy and tests passed; `dependency-smoke` rejected the pre-existing mismatch
between the declared Core pin `30ac3ff33e68da7aca9e666cbf1b8e59de411d64`
and the actual Cargo dependency/lock revision
`a16d4a769258fb7354d9102017533850020f8f2c`. The gate is not weakened or
skipped. Log `target/relay-environment-clean-verify.log` SHA-256:
`2b78076208fa61a1864c873c1a70d56306677b4f1715d9db812b1aafa032348e`.
The mismatch is repaired at local Relay commits `67ac02c` (manifest) and
`99d8a99` (the exact-pin regression assertion). These preserve the existing
qualified Cargo revision; no runtime dependency was upgraded. The actual
`just dependency-smoke` against a detached checkout of that exact Core commit,
all eight dependency-smoke tests and targeted Clippy passed. Log
`target/relay-pin-smoke.log` SHA-256:
`10bebf00f0fcf1e2c353197e92192b7f6e6ab932175237d527583f2859e094dd`.
The intermediate full gate `18440` is terminal exit 1, with 140 xtask tests
passing and the old exact-pin assertion failing; its log hash is
`6ffba5b1c9923114e4df5aee1e19b0ecc796a7926476183a72a44ccac8cfa02c`.
That assertion now passes without relaxing equality. Full `just verify`
process `81835` is terminal exit 0 at runtime revision `99d8a99`, log
`target/relay-environment-pin-verify.log` SHA-256:
`06a46f63e487da5247d8fe003c08f5ae9f4a006a10949480f8228129cfbffe95`.
This closes the E1 owning runtime gate. The standard full gate reports its
optional Core checkout smoke as skipped; the separate actual exact-checkout
smoke above passed and supplies that evidence. Do not claim a skipped check
as passed or restart the terminal process.

E2 configuration slice is committed locally at `5f0fe6b`: new pure
`infra/aws/relay-environment` module, Gateway/Reach/Watch IaC and owning
`just relay-component-infra-{init,plan,apply}` commands. Existing test resource
names and addresses remain unchanged; actual AWS state-key inventory confirmed
`f2/relay-{gateway,reach,watch}/terraform.tfstate`, which the recipes preserve.
Staging/production use separate state keys and names. Guards require staging
owners; reject production conformance/Hub databases, default test roles and
archives; bind endpoint to scoped cluster ARN; prevent granting hosted inventory
access to the test role; and require environment-specific Watch secret/owner
scope. Apply requires the reviewed saved-plan digest and refuses deletion or
replacement. All three roots pass locked-provider initialization, formatting
and validation. Seven provider-free positive/negative module tests pass under
the new owning test target, which is included in `just verify`. Log
`target/relay-environment-iac-tests.log` SHA-256:
`dab44b99fddb53a8fd43e62f09f3e496e469fa87e3855944d757fcf39c87b631`.
Recipe syntax checks passed; invalid component and relative artifact/file paths
exit 2 before cloud work. These are configuration tests, not live IAM or E4
acceptance. No saved real deployment plan or apply has run. The E1 runtime
gate predates this IaC-only commit; the complete owning gate is still required
at the next integration/deployment checkpoint, alongside real resource plans,
readbacks, database-role grants and rollback bindings.
Additional E0 readbacks confirmed Reach's 13 and Watch's 8 direct-origin routes
use application authorization, and the Gateway IAM role is scoped to the test
cluster/archive prefix and its own logs. A real POST to Hub identity with the
sampled stored session returned `unauthorized`; all six WorkOS session records
have expired access claims. Do not treat their stored tenant/subject as verified
owners. Use supported refresh or fresh sign-in before deployment. Details are
in the existing ignored operator inventory. No apply, resource creation, broker
order, source push or deployment proof occurred. Next after owning verification:
resolve exact owner/nonowner identities and remaining E0 route/IAM/profile
readbacks before staging deployment. The operator identity is now verified:
the existing source-free Core MCP process `73016` returned authenticated from
`tradeassembly.auth.status`, and a direct production Hub
`POST /v1/identity/session` returned HTTP 200 with its tenant/subject. Exact
principal identifiers remain in the existing ignored operator inventory,
not this public document. New session material is stored in Bitwarden, not
Keychain. No broker credential, broker order or Live activation was involved.
The first bootstrap attempt reached a confirmed callback timeout; the retry
reused the same MCP process and authenticated browser session and its bootstrap
record is `succeeded`. The browser callback subsequently displayed a refused
connection after the one-shot listener closed; persisted bootstrap and the
independent Hub response, not that browser error, prove authentication. Returning
to the same onboarding page visibly shows `Signed in. Connect your broker to
continue.` Screenshot evidence is the ignored Relay operator artifact
`.operator/relay-environments/operator-auth/authenticated-page.png`.
A separate CLI initialization failure (`file-backed secret is unavailable`)
was traced to the default relative Warden token reference: the working MCP
uses the Studio directory, while the isolated CLI uses the Relay directory.
Both isolated operator/nonowner configurations now explicitly reference the
same existing mode-0600 Studio Warden token file, without copying its secret
or changing authority. Fresh standalone CLI `auth status` handle `3959`
exited 0 and returned the authenticated owner. Independent valid nonowner
verification remains open. The original nonowner attempt's callback port
8977 was rejected by the actual WorkOS error URL `redirect-uri-invalid`.
That MCP `61054` was gracefully stopped (handle now absent); the isolated
configuration now uses the already-registered callback on port 8976.
Replacement MCP `44286` initializes and authenticates successfully, but
WorkOS reused the existing owner browser SSO: the returned stable identity
is the same owner, so this is explicitly **not** nonowner acceptance evidence.
Current live handle: MCP `73016`, isolated config
`.operator/relay-environments/operator-auth/runtime.json` in the Relay worktree.
Independent nonowner identity is now verified: isolated browser email login
completed with stable identity `oidc:XCeRD2Wu_MKIivtlCHO_BVuXICIhxgFBxNd-ZvXJtyQ`.
Actual production Hub POST `/v1/identity/session` returned HTTP 200, tenant
`org_01M1DJ93D2WZMB65VRCAS17GJ9`, subject
`user_01M1DHVPNM3XYWX3TKV95W0V76`, distinct from the owner subject above.
Fresh standalone CLI `24077` exited 0 with `authenticated: true` for this
second identity. Sanitized readback is in the ignored inventory, observed
2026-10-02T01:13:53Z. This proves identity only, not deployed staging denial.
Next: finish E0 Cloudflare route/origin and production profile readbacks before
deployment, then use this real nonowner in E4. The normal
Hub dashboard sign-out in Chrome tab `1003168191` currently displays
`ERR_BLOCKED_BY_CLIENT` at `/auth/sign-out`; preserve the page, do not disable
browser protections. Screenshot is the ignored nonowner operator artifact
`browser-account-switch-blocked.png`. Source pointers: Hub
`apps/hub-edge/src/index.ts` POST `/auth/sign-out` and
`apps/hub-edge/src/dashboard-auth.ts::endDashboardSession` (302 to WorkOS);
actual Hub response headers include `form-action 'self'`. A cross-origin form
redirect/CSP conflict is a hypothesis to verify, not yet a proven cause.
Account switching succeeded using WorkOS's documented session logout URL
with the isolated session's `sid`, then Core CLI logout of only that isolated
local session. WorkOS reported an unset app homepage after logout, but the
next normal login did show the email screen and accepted the second identity.
No browser protection or authority policy was weakened, and the dirty Hub
checkout was not modified. The successful callback still displayed Chrome
`ERR_BLOCKED_BY_CLIENT`; the persisted bootstrap reports `succeeded`, and
actual Hub plus fresh CLI evidence prove authentication despite that UI error.
Keep this user-facing sign-out/callback friction open for diagnosis rather
than claiming browser onboarding is clean. MCP `44286` retains its previous
owner binding and reports unauthenticated for the changed account; use a fresh
process if needed for the nonowner, not a bypass of identity binding.
Original operator MCP `73016` and its local session were not modified.
E0 Cloudflare readback: installed Wrangler 4.125.0 and the Cloudflare connector
currently authenticate only the older OptionLab identity/accounts. Explicit
Worker queries in account `efdb13d2d6f8a0124f0ec3d27cdd3aab` return 10007
(Worker absent); inherited target account `2e778d60d9f80f40c6af6f499c52acc8`
returns 10000 with those credentials. Connector account/domain/zone readbacks
confirm the same visibility mismatch, not absence of the deployed service.
Existing Hub `docs/operations/deployment-access.md` and Relay
`infra/aws/broker-oauth-relay/deployment-2026-09-09.md` identify the personal
operator identity as the correct existing account. Do not move infrastructure
to a legacy account or overwrite the current default Wrangler profile.
Cloudflare access repair is complete, observed 2026-10-02T01:33Z. After the
user confirmed the final consent, fresh no-browser authentication `18412`
exited 0 and created `tradeassembly-release`. Earlier `34927` and `5244`
expired; they are not live processes. The profile is bound only to the Relay
worktree, leaving the default profile unchanged. `whoami --json` exited 0
and verified the exact target account `2e778d60d9f80f40c6af6f499c52acc8`.
Wrangler 4.125.0 rejects `whoami --profile`; use the directory binding for
that command. Scopes omit unneeded KV/Pages/AI access and select only the
existing target account, not all future accounts. Browser success screenshot
`cloudflare-auth-success.png` is ignored operator evidence.
Actual deployment-list and version-view commands exited 0 for both workers:
staging version `832ef9b8-29fb-4b21-8489-7e7edf2bf56d` at 100%, with
`PUBLIC_HOST=staging.tradeassembly.ai` and origin
`https://6ve4rs0fda.execute-api.us-east-1.amazonaws.com`; production version
`5503bc2d-5469-4e61-bef4-f5606577f327` at 100%, with
`PUBLIC_HOST=connect.tradeassembly.ai` and origin
`https://2q1d6wr0f8.execute-api.us-east-1.amazonaws.com`.
This is current deployed-binding evidence, not staging isolation acceptance.
Actual supported Worker Domains GET returned HTTP 200 / success for both exact
hostnames, bound to those worker names in zone `67d9ad08d4969496fab457fed5d84bf4`.
Domain IDs and public mappings are recorded in the ignored inventory. Connector
credentials remain the older account; bounded API readback captured the official
CLI token in process memory only, never logs or repository files.
Actual authenticated Alpaca dashboard readback verifies production app
`TradeAssembly` is Published, website `https://tradeassembly.ai`, and exactly
one redirect `https://connect.tradeassembly.ai/oauth/alpaca/callback`. Its public
client ID matches deployed `591d18539bcf3d8e37b3560d5c95a100`. Existing Terms and
Privacy URL fields are empty; no form was submitted or secret regenerated.
Ignored screenshot `alpaca-production-registration.png` captures the public
form, not the credential modal. Existing password/authenticator login completed
through Bitwarden; MFA required native paste because segmented DOM fill did not
advance correctly. Staging registration readback remains separate.
Next: separate Relay storage deployment and the remaining E1–E5 gates.
No Cloudflare Worker, route, deployment, or infrastructure policy was changed.
E0 AWS readback is refreshed: actual STS account is `056319544861`; complete
DSQL/S3 listings contain only Hub and conformance DSQL plus the test/archive
and existing state buckets. There is no separate production Relay DSQL/archive
or production Watch SSM parameter. Do not relabel the conformance resources as
production. Actual gateway/Reach/Watch configurations all select
`relay_conformance` on `bnucpdh6dc6o4nutdrgu2eq464`; deployment hashes and
sanitized mappings are recorded in the ignored inventory.
Production OAuth selects `https://connect.tradeassembly.ai`, its own DynamoDB
table and `/tradeassembly/broker-oauth/production`; its client ID is public
metadata, not proof of current Alpaca redirect registration. Both OAuth roles
are scoped to their own tables, parameters and logs; KMS decrypt requires their
exact parameter encryption context and SSM service. Reach/Watch roles select
only the test DSQL cluster and their own logs; Watch's secret is the acceptance
parameter, and its SES wildcard is constrained by the sender condition.
All five HTTP APIs' actual routes and Lambda proxy bindings were inspected:
OAuth staging/production 4 each, gateway 10, Reach 13, Watch 8; each uses
application authorization (`NONE` at API Gateway) and `$default` auto-deploy.
These readbacks close route/IAM discovery, not application-boundary acceptance.
Next after Cloudflare readback: freeze those origin bindings and current Alpaca
registration, then provision the separate staging/production storage and
verification-role boundaries through reviewed IaC. Existing pure environment
validation creates no database, archive or verification role; do not confuse its
passing tests with deployed resources. No secret values were retrieved.
E2 storage bootstrap is now implemented locally in private Relay
`infra/aws/relay-storage/` and its owning `justfile` recipes. It creates new
environment-named DSQL/archive resources, explicitly rejecting test/unknown
environments. DSQL deletion protection and both resources' `prevent_destroy`
retain data during code rollback. Archive configuration enables versioning,
Object Lock, KMS encryption, owner-enforced ACLs, blocked public access and TLS.
No test lifecycle expiry is copied. State keys are distinct
`ENV/relay-storage/terraform.tfstate`; apply requires the exact saved-plan digest
and rejects any delete/replacement action. Committed locally at Relay `5eadefa`.
Mocked OpenTofu checks pass 4/4;
these are configuration evidence only. No storage apply, SQL role/schema/grant,
verification IAM role or Watch secret has been created. Initial full gate
`99721` exited 1 solely because the language scanner requires committed source;
the actual report lists the new files/justfile as uncommitted, not a language
violation. After committing, full `just verify` `72489` exited 1 due to local
disk exhaustion during compilation (`No space left on device`), log
`target/relay-storage-integration-verify-committed.log`. This is not a passing
integration gate. Disk readback showed 152 MiB free and 43 GiB in this Relay
worktree's regenerable `target/debug`; no process executes a binary there.
Scoped `cargo clean --profile dev` `37756` exited 0, removing 45.4 GiB of
regenerable build data while preserving `target/release`, top-level evidence,
Core frozen bundles and other worktrees. Disk readback now shows 42 GiB free.
The Core frozen lock digest remains
`222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`.
The same full `just verify` is now live as `83875`, with
`CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0` to
bound cache growth; log `target/relay-storage-integration-verify-compact.log`.
These flags change debug/cache size, not acceptance behavior. Reuse the live
handle. Next: its terminal result, then inspected live storage plans and
remaining E2 schema/role/Watch boundaries. Overall release remains incomplete.
Both live storage plans are prepared and inspected: each has exactly eight
create actions, zero changes/deletes/replacements, account `056319544861` and
region `us-east-1`. Staging saved-plan SHA-256 is
`4652e39947f5d8a1e36b57088a9814627c10fcd20ba08273ed9d442d1de422f4`;
production is `cca7219b271f1cc194cf914d515b19b69106f84d310433ce062f7039faf5e430`.
Plans select their own `relay_ENV` database role, `ENV/gateway` prefix and
`tradeassembly-relay-ENV-056319544861-use1` bucket. Full gate `83875` is terminal
exit 0, log SHA-256
`ba66dbbfc608a9993d3928b96b6fddfb5d2e384f8bdb94f4d6dd89438272c8e3`.
Its optional Core checkout smoke is skipped, not a new pass.
Staging apply `71278` and production apply `74516` both exited 0: eight added,
zero changed/destroyed each. Actual AWS readbacks verify distinct ACTIVE DSQL
clusters (`xnud5czqxjrmah57yldkinncem` staging,
`xjud5dniplgc6e6varrnvvrxze` production), deletion protection and enabled
AWS-owned KMS encryption. Both actual archive buckets have KMS encryption,
versioning, Object Lock, BucketOwnerEnforced, all four public-access blocks and
the TLS-only deny policy. The ignored inventory records this separately from
application admission. SQL schema/grants, verification roles, Watch secrets
and component deployment remain next; storage alone is not E2 completion.
These processes are terminal. Do not restart or reapply unchanged plans.
Deterministic live storage boundary gate `49002` also exited 0. It captures
actual AWS readbacks for each environment under ignored
`.operator/relay-environments/ENV/*-readback.json`, checks all nine required
boundary predicates with `jq -e`, and emits `storage-boundary.json`; all are
true in both environments. This is deployed storage evidence, not mocks or
application E4 proof. Frozen Core lock hash was reverified unchanged afterward.
Next action: provision the versioned SQL schemas/grants and environment-specific
verification IAM roles using existing adapter schema constants and exact
storage outputs. The existing conformance role trusts only its bootstrap user
and HubProductionDeploy SSO role; do not grant it customer-environment access
or copy its test permissions. Then finish Watch secret references and deploy
the environment-specific components through their owning plan/apply recipes.
E2 schema bootstrap is committed locally at Relay `f095e5b`: private
`packages/hosted-adapters/aws-dsql/src/bin/relay-schema-bootstrap.rs`, owning
`just relay-schema-{plan,apply,test}` and storage README. It consumes the five
existing adapter SCHEMA arrays (37 statements), validates actual AWS account,
cluster environment/protection/encryption, and binds environment/endpoint/DDL/
role grants into a required apply digest. SQL values in catalog reads use
driver parameter bindings; all executable DDL/grants are checked-in static
Rust strings. No caller-supplied SQL, DDL in runtime startup, automatic write
retry, schema repair/delete or application admin role is introduced.
Resumption reads the actual catalog and requires all existing schema validators
to pass after async index creation. Grants provide only private-schema USAGE
and table DML. IAM mappings remain explicitly absent from its receipt.
Targeted tests pass 4/4 and strict Clippy; actual invalid-digest CLI exits 1
before AWS/SQL. Full owning integration gate `73316` is live, log
`target/relay-schema-integration-verify.log`; do not restart it. Staging plan
`17365` is terminal exit 0, digest
`86b726ca7028e8617fc76248b990621f3df00cd6a8eaca85ec77efb56ef7b1ee`;
production plan digest
`a85a50eabc39c1de5f5a139d33d16b6873157509b7cb2bc37a3d09dace748c7c`.
Both plans select exact newly deployed endpoints and only their own database
role. Full integration gate `73316` is terminal exit 0, log SHA-256
`0989e8120a21ca7292762451003f9e46b396c6b34b7e22045b97b55a2a460c4e`;
optional Core checkout smoke remains skipped, not a new pass. Staging read-only
IAM/admin preflight `47480` exited 0 with full TLS verification; actual
`to_regclass` read also returned absent before bootstrap. Staging SQL apply
`47469`, staging resumption `11181` and production SQL apply `19164` all exited
0. Both receipts report actual schema verification and the exact environment
application role; IAM mappings are explicitly false. Staging resumption finds
37 existing objects, zero created objects. Actual catalog readbacks for both
roles report LOGIN true, SUPERUSER/CREATEROLE/CREATEDB false. A fail-fast shell
gate rechecks both receipts, all resumption steps and both role readbacks.
Ignored evidence log SHA-256s:
staging apply `4e853cb73cf27015a3daa7150e4f4a7405a822f57f9f809707508b494c69ad35`,
staging resume `9d7d5ea42ceea4e14bb8539be9053cfabc8ebc7c4745dacc37210a39ef14ed3d`,
production apply `14d78f127918d0f55d387016e7115468c8bc9388f97f2d46474d210758d91fa9`.
These handles are terminal. Do not restart full gates or recreate storage/schema.
Next: environment-specific verification IAM roles and runtime IAM/DB mappings,
then the already-specified component deployment and E4 customer authorization
acceptance. No customer data or broker orders were created by this bootstrap.
Deployment caution from targeted source inspection: existing
`apps/studio-f2-gateway/src/hosted_watch.rs` identifies itself as an acceptance
surface with a fixed configured owner/workspace/node and private acceptance
bearer; `infra/aws/f2-relay-watch/main.tf` does not include environment admission
in that configuration. Do not equate this test surface with customer-bound
production Watch or deploy it as a universal customer authorization bypass.
Resolve its actual customer/node authority binding under the existing E1/E2
requirements before claiming production Watch availability.
E2 verification authority checkpoint: Relay commits `ba93813`, `13e507e`
and `1c57a8f` add isolated read-only verification roles, fix the KMS encryption
context condition to StringLike, and pin trust only to the existing
HubProductionDeploy SSO role. Targeted infrastructure tests pass 3/3 and both
actual AWS policy validations return zero findings. Full owning gate `57025`
exited 0 (log `target/relay-verification-integration-verify.log`, SHA-256
`99133f3c6550410aca592a46b3ef3e4899aca88255471e49d386f1e04dcbb21f`);
it began before the later Terraform-only corrections, which have targeted
validation, not a claimed fresh full release qualification.
Both roles were created with two additions each. Root cannot AssumeRole;
the existing `tradeassembly-prod` SSO profile was refreshed successfully and
actual caller identity confirms account `056319544861` and the intended SSO
role. No new password, recovery path, long-term key or default profile change.
The reviewed trust-only update plans were applied successfully: staging
`5ec2ac7319436eec2b84ed46423a5e75beeded32dd17c6a57afcf6bc6de347d0`,
production `fc3f2d95bc7e46b366d9e6cec501abb158e1cb6ce24a79d883700cb3f66bca8e`.
Each changes exactly one trust policy with no delete/replacement; actual IAM
readback confirms only the exact SSO principal. Live boundary gate `29836`
exited 0: both actual assumed verification roles can read only their own
cluster, bucket encryption and archive-prefix listing. Cross-environment
cluster/bucket reads and out-of-prefix lists all fail with AccessDenied.
Ignored evidence is under `.operator/relay-environments/ENV/verification-*`.
This proves IAM readback isolation, not SQL access, application admission or
customer release readiness. No broker orders or Live activation occurred.
Next: create read-only SQL verification roles and exact IAM/SQL mappings;
runtime component IAM/DB authority and customer-bound Watch deployment remain
in scope. Reuse the completed storage/schema/build evidence; do not rerun
those gates merely to refresh authentication.
SQL authority follow-up extends the existing private Rust schema-bootstrap
utility with `authority-plan`/`authority-apply` and owning
`just relay-authority-{plan,apply}`. Closed checked-in staging/production
bindings select only verification/gateway/reach/watch IAM names. All grants
are literal SQL accepted by SQLx's static SQL guard; catalog values remain
parameter-bound. Exact live IAM ARN/environment tags and existing cluster
boundaries are verified before grants. Existing conflicting mappings or
elevated/inherited/write authority stop the apply, without automatic repair.
Verification gets its own read-only SQL role, never the frozen `relay_ENV`
runtime role. Original schema-bootstrap digest is unchanged. Targeted gate
`81783` exited 0: six tests and strict Clippy pass.
Actual staging apply and resume, then production apply (`2826`, exit 0)
confirm exact verifier mappings, with resume reporting `existing`. Plans:
staging `c58c93a0337839ab7cfd5e3f7a6d4a63e7540c5e7e6256da46c8f686795bd3fb`,
production `cf6e89f8be5f5fec4ce8cbdf647203fbf90f3e3e71cca3f003df255c270c4e27`.
Live non-admin SQL gate `67708` exited 0: both actual assumed IAM verifier
roles connect as their own SQL role with verified TLS, read tables in all five
private schemas, have SELECT on all private tables, no DML or schema-CREATE
privilege, and are denied a connection as the runtime role with actual
`FATAL: unable to accept connection, access denied`. Evidence remains ignored
under each environment's `authority-verification-*`,
`verification-sql-privileges.json` and `verification-runtime-role-denial.txt`.
Initial full owning `just verify` `8643` exited 1 at the repository-clean
language invariant, after its preceding leaf gates passed. Relay `a770cfb`
now commits the existing bootstrap Rust file, justfile and storage README;
no gate was bypassed. The committed full rerun `21056` is terminal exit 0,
log `target/relay-sql-authority-integration-verify-committed.log`, SHA-256
`aa1c5d6d0ff7ad71d24b9c0fc834d417d441ecef7a44043066a6bfd1a67e1508`.
Its optional Core dependency smoke is explicitly skipped; no new claim of
cross-repository or customer release qualification follows from that skip.
Relay `acd86bf` adds owning `just relay-component-build` with closed component
selection, locked dependencies and ARM64 Lambda ZIP output. Invalid component
selection is rejected. Fresh Gateway build `66222` and Reach build `54908`
exited 0. Archive digests: Gateway
`559803ce4668abedc1f410bd3076b420eb5a1cb7fc74d0142704b316792814b5`, Reach
`f3c18f98c4342f1b9ad5be5d61d64b6b4e9db3d0a232b9e4a734ad08de60af35`.
Staging Gateway's initial SSO plan `1747` exited 1 on S3 state access;
the existing deploy profile works without expanding Hub SSO permissions.
Reviewed staging plans (Gateway `1b9c9430e5609382e57c67ccd81eb1f37302c5ddc99d33c2b8a9ed377317c54c`,
Reach `c5621e370808271093849d4c8668c32756d5cff49581e434a99eea469bc4bf60`)
and production plans (Gateway `b4a351cb18aa7b8189c65c1aa0f7872aab4cad906d835695ad77f57c7c203f36`,
Reach `9b3943fcf637d06a44dcffda6178e1e249e7fe37ed48121853f453377f2cb33e`)
create only isolated resources, no replacements/deletes. Applies `95700`,
`29379`, `63597`, `52961` all exited 0. Actual AWS code/configuration readbacks
`33148` and `40668` exited 0 and match exact built ZIPs and reviewed config
digests. Staging origins: Gateway `https://9l27k1uz14.execute-api.us-east-1.amazonaws.com`,
Reach `https://0bxei6rn16.execute-api.us-east-1.amazonaws.com`.
Production origins: Gateway `https://22msecn6h1.execute-api.us-east-1.amazonaws.com`,
Reach `https://v7bqv2qykb.execute-api.us-east-1.amazonaws.com`.
Exact Gateway/Reach runtime IAM→SQL role mappings are verified in both
environments (`3694` and `45351`, exit 0), without admin runtime access.
Actual assumed verifier boundary `43300` exited 0: each can read both own
Lambda configurations and is denied both cross-environment configurations.
Endpoint gates `71335` and `75249` exit 0: both Gateway health endpoints 200,
both Reach anonymous scopes 401. Fresh browser token gate `27483` exits 0:
Hub verifies the distinct same-tenant nonowner (200), then staging Gateway
`/v1/access` and Reach `/v1/reach/scope` both deny it (401). Public `/v1/setup`
returns generic instructions by design and is not a protected admission test.
No zero-side-effect or all-route E4 proof is claimed by these narrow checks.
Evidence is ignored under `.operator/relay-environments/ENV/{gateway,reach}-*`
and `authority-{gateway,reach}-*`. All listed build/deployment/readback gates
are terminal. Reuse them; do not restart or rebuild unchanged artifacts.
Owner-positive verification initially stalled because isolated owner refresh
expired and Chrome reused the nonowner AuthKit session; actual subject
inspection caught that mismatch. This is now resolved as recorded below:
`operator-auth` belongs to the exact Hub-verified owner. Keep independent
nonowner state isolated; authenticated status alone is still not owner proof.
Existing owner MCP `73016` remains live. Watch customer contract is now settled:
`customer_watch.rs` owns hosted configuration v2 and protected HTTP routes;
legacy `hosted_watch.rs` v1 is test-only. Use existing HubWorkspaceAuthenticator
(identity, current environment admission, stored workspace binding), fresh
`tradeassembly-relay`/`relay.use` entitlement and Reach HubNodeAuthority over
the same durable installations. Callers select a node, never an owner/workspace.
Heartbeat signatures bind the verified scope, timestamp and local cursor;
reject stale/future/revoked proofs before mutation. DSQL monotonic heartbeats
provide duplicate/ambiguous-commit recovery without a separate nonce write.
Loss detection must never manufacture a heartbeat. Remote archive cursor is
conservatively zero until verified archive integration; do not claim catch-up.
Store authenticated customer notification preferences in existing subscription
JSON (backward compatible, no DDL). Browser ACK is a delivery-specific signed,
expiring capability binding environment/tenant/workspace/principal/node, with
current admission and installation ownership rechecked. No universal customer
token, fixed operator recipient, browser fragment login, or new runner/GUI.
Proof matrix: source route inventory and unit negatives/signature binding;
owning Watch compile/clippy/tests; real isolated DSQL duplicate/recovery and
zero-effect denial; deployed owner/nonowner/anonymous and email/push receipts.
Local checks alone do not close E2/E4. Relay `e7910a2` commits this customer
Watch contract and preserves the old test deployment. Targeted gates are
terminal green: `just hosted-watch-test` (2 binding tests, 3 route inventory
tests, executable startup boundary, clippy and OpenTofu validation),
`just customer-watch-local-test` (actual HTTP handlers + real SQL adapters +
explicit local Hub fixture, zero Watch/installation effects on denials),
local-feature clippy, and all six `just dsql-local-test` tests. These are not
actual production Hub/DSQL/provider evidence. Heartbeat duplicate read-back,
cross-node delivery denials and pre-outage replay recovery are covered. Logs:
`target/customer-watch-focused.log` SHA-256
`9e062375b8c3bb4531b53edff8e8952d5fffb6973b4bf8f5aefefd5469955a8a`,
`target/customer-watch-boundary.log` SHA-256
`b5278ccd66e519dc1dbb6b73e14b72298cef06dfbca800014326325b66a0185a`.
Committed `just verify` integration `98667` is terminal exit 0, log
`target/customer-watch-integration.log`, SHA-256
`076d174f8a048d04501fb5869365ecef706481201f54ffa22866ef922a874054`.
Its optional Core dependency smoke is explicitly skipped, not new Core proof.
Legacy Watch's three regression tests and all six local SQL tests also pass.
Independent staging/production Watch credentials are now saved in Bitwarden
and environment-specific Standard SSM SecureStrings (AWS-managed key).
No raw secret value entered a command argument, local file, log or receipt.
The deploy sender `alerts@tradeassembly.ai` is actually verified in SES.
Watch is deployed in staging and production using exact ARM64 ZIP readback,
without changing the test deployment. IaC follow-up `4c0654a` fixes the
environment JSON type mismatch exposed by the real staging plan and adds
exact own-environment Watch configuration read access for verification roles.
Staging plan SHA-256 `3091ea5a49f7fb8944e565f70e3400f2d9c06f215ffb0d6ff757ef8514a91545`;
production `008d146efecbbad64ca53e34f174f62b4844bb7135ae6012e9a5ac2b26e5936b`.
Both reviewed plans create 17 resources, no replacement/deletion. Applies
`90330`/`61042` are terminal exit 0. Origins: staging
`https://cvpv36l9e1.execute-api.us-east-1.amazonaws.com`; production
`https://mubijupjx2.execute-api.us-east-1.amazonaws.com`.
Actual code/configuration readbacks match v2, exact DSQL/SQL role, distinct
SSM namespace and staging owner/production empty override. Config digests:
staging `2bc210c4cb09f6d3292590248b9a07d8bf4374331f1022ec7d571688355d0ba8`,
production `072af0a412114db1a9444d984c47d94eac8c3cb3ea124cb7db1e4f35c3593cbc`.
Exact Watch runtime IAM→SQL mappings `9597`/`58208` verify non-admin runtime
roles in their own clusters. Actual assumed verifier gate `84555` exits 0:
own Watch configuration readable, cross-environment denied, both environments.
Both actual anonymous node-status endpoints return 401, proving successful
startup through SSM/DSQL. Fresh Hub nonowner identity gate `67910` returns 200
for the independently verified nonowner and staging Watch node-status 401.
Neither narrow check proves all-route zero effects or owner-positive access.
Vault/SSM parity is verified in both environments. Post-IaC owning focused
gate `38111` exits 0; environment infra tests pass. Reuse these receipts and
ignored `.operator/relay-environments/ENV/watch-*`, `authority-watch-*` evidence.
ARM64 Watch build `90457` is terminal exit 0; the new ZIP SHA-256 is
`b58a8d7adbd292466452ab50f56634efee9e9d047cd85800354a598ea7d25ab7`.
Old test ZIP is preserved as `target/relay-watch-test-baseline-20261001.zip`,
SHA-256 `2a961bc5c58f53c7407802dd77828a3ba1c8940387b3bb1821c43adfc973ab86`.
The build's deprecated linker optimization warning is nonfatal; no gate was
weakened. Customer browser onboarding and
automatic loss scheduling are not implemented by this API and remain explicit
integration gaps; conservative remote cursor zero is not archive catch-up proof.
Staging OAuth admission is deployed from Relay `77376c4` (locked standalone
Lambda build). Build `94408` exits 0; ZIP SHA-256
`51c7041ed06de52ceb8bd8bf1635a8945e1c322910ccab396a1a3314808a50d6`.
Reviewed plan `b331dd7e4ad50b23f0ef7bfa79774fe12ac1a5ef95ebf043f4b891a4a3dd7255`
changes only the staging OAuth Lambda code/environment, no create/delete.
Apply `93285` exits 0; actual AWS readback verifies exact code and owner tuple.
Anonymous gate `40248` and fresh Hub-verified nonowner gate `81196` deny start,
status and ACK on both public URL and direct origin (all HTTP 401); actual
DynamoDB count confirms zero probe connection rows. This is not callback or
provider-effect proof. Exact owner sign-in is now recovered through the
existing isolated Core MCP `73016`: only AuthKit sign-in cookies were cleared
in Chrome to leave the cached nonowner, preserving anti-bot/security cookies;
existing email-code login authenticates `david.beveridge@tradeassembly.ai`.
An initial CLI status raced local Bitwarden persistence; do not restart the
successful flow because Chrome's callback tab showed an error. Actual MCP
status and Hub identity confirm the required owner. Gate `48921` exits 0:
owner creates a disposable data-only Paper pending OAuth connection, recovers
it through public/direct status routes, then ACK removes it. Actual DynamoDB
readback `97723` confirms zero remaining rows for that isolated instance.
No Alpaca token exchange, account connection or broker order was attempted;
full broker connection/provider acceptance remains pending. Full owning
`just verify` on `77376c4`, handle `6333`, exits 0; log
`target/oauth-admission-integration.log`, SHA-256
`88f94a3e6ce7acfc1383b287152443896fa3232975fcce8437391ed60a75f95d`.
Its optional Core dependency smoke is explicitly skipped, not Core proof.
No new agent was spawned. Existing Core MCP handles remain untouched.
Owner Relay entitlement/enrollment is now actually verified (2026-10-02):
Hub first returned `commerceMode: test`, `permitted: false`, no subscription
paid period; staging enrollment returned 403. This was not a login failure.
The existing Hub operator account inspected the actual registered manifest
through the dedicated dashboard API (HTTP 200), without granting the Relay
owner catalog-admin authority. Manifest revision 1/digest
`sha256:f71a327d73792a8657d2a162b9afc4bf737cd202eb8c3362cf212c4c75c716aa`
registers `tradeassembly-relay.relay-f2-monthly`, the existing test client and
test Stripe price. The required HTTPS return path is `/checkout/return`,
not `/purchase/return`; its existing gateway return page actually works.
Corrected owner test checkout `15517` returns 201 (subscription
`9530c5ed-e138-4217-a4db-b962dd7533f9`). Stripe visibly labels it Sandbox;
official synthetic test-card checkout completed without saving payment details
or moving real funds. No grant was fabricated, no paid gate disabled, and no
Hub source/deployment or registry mutation was needed. Existing Hub paid-period
branch `50c2420` remains clean and must not be reimplemented or deployed blindly.
Actual Hub follow-up `51731` returns `permitted: true`, `state: granted`, test
commerce, revision 1, and a durable paid period spanning
2026-10-02T05:04:40Z through 2026-11-02T05:04:40Z. Actual staging enrollment
returns 200. Duplicate gate `83599` returns the identical workspace binding,
revision and paid period; `/v1/access` returns 200 with granted entitlement.
Ignored receipts live in Relay `.operator/relay-environments/staging/`:
`owner-entitlement.json` SHA-256
`41639658fe33f3e63116a2e15414f79f3fc3e8ba7914b7ff621e5b621724ab72`;
`owner-enrollment-response.json` and `owner-enrollment-replay.json` both
`addf3f0bbaf3f36398174d3c9b7252c53122c46a26195a129f6f58aad0786615`;
`gateway-owner-positive-response.json`
`85d7802db9a627912874ba67e1bae856492488c525bac29d7a9bb7cc456ebdfa`.
The isolated staging workspace/test subscription is intentionally retained
for the remaining acceptance, not production billing proof. Native Chrome
cleared only Hub/AuthKit sign-in cookies to select the existing operator;
security cookies and independent Core owner/nonowner stores were preserved.
Browser clipboard and native system clipboard are separate; temporary
same-origin dashboard authentication was kept in memory and cleared after
the API read. No raw authentication was logged or written to disk.
Next: finish E4 owner/nonowner, zero-effect and broker-connection acceptance
before E3/E5 release assembly. Reuse the now-owner `operator-auth` session;
the independent `nonowner-auth` remains available for negative verification.
E4 immediate proof slice: private Relay `xtask/src/relay_environment.rs`,
`xtask/src/main.rs` and `justfile` own a deployed journal probe, not a full E4
verdict. Settled contract: HTTPS-only configured staging gateway and Hub;
owner/nonowner identities must be verified by Hub, owner must have fresh
`relay.use`; credentials arrive through bounded stdin and never enter evidence.
One fixed diagnostic request is configured before first submission and reused
after interruption; read-back, duplicate receipts and conflicting-key rejection
prove recovery. No scheduler tick, broker order, production write or new grant.

| Proof | Deterministic check and durable evidence |
| --- | --- |
| Actual identity and entitlement | Hub identity equals both configured actors; owner entitlement permits relay.use. |
| Durable journal and recovery | Ingest and exact replay return identical receipt; query/export contain exactly one matching request and digest. |
| Conflicting key | Changed payload with the original key returns 409. |
| Boundary isolation | Anonymous and valid nonowner ingest/read/export denied; owner wrong-workspace ingest returns 403; archive before/after denial attempts is unchanged. |
| Fail-closed evidence | Probe exits nonzero on missing observations; receipt binds public configuration and candidate SHA-256 and says staging-journal only. |

Acceptance: `just relay-environment-journal-test`, then the actual
`just relay-environment-journal-probe CONFIG CANDIDATE EVIDENCE` with tokens
on stdin. Owning `just verify` is required at integration, not on each edit.
Remaining OAuth, Watch, production-profile and registry cases stay incomplete;
this slice cannot satisfy `relay-environment-verify` or the release gate.
Immediate slice implemented at Relay `cba99ec`. Actual deployed gate `65434`
exits 0: actual Hub verifies both actors and owner entitlement; staging returns
the same ingest/replay receipt and exactly one durable matching diagnostic event,
query/export parity, conflicting key 409, wrong workspace 403, and anonymous
plus verified nonowner ingest/read/export 401. Archive is unchanged across all
denial attempts. This also resumes the partially completed first run `92966`
with the exact original key and timestamp; no additional event is introduced.
First run's read used unsupported limit 1000; owning route requires at most
100. Probe now uses 100 and rejects truncated/non-isolated page evidence.
The single diagnostic event is intentionally retained under normal immutable
archive policy. No customer strategy, credentials or broker orders are stored.
Ignored receipt: Relay
`.operator/relay-environments/staging/journal-proof.json`; includes public
config/candidate/probe-source/request/receipt hashes, observation time and hashed
actors, explicitly `fullE4Complete: false`. Tokens enter bounded stdin only.
Focused tests pass 3/3 and strict Clippy passes. First integration `38769`
fails only because the whitelist requires a clean committed checkout; no
gate was waived. Local commit checkpoints the code before rerun `14178`.
Rerun `14178` is now terminal exit 0: full owning `just verify` passes against
clean committed `cba99ec`. Log: Relay `target/staging-journal-integration.log`.
Log SHA-256:
`9b807407c4b2a502f28f3801c665d464a7ae3a69ad3514d92b22ec4a19114cb3`.
Optional Core checkout composition smoke is skipped because its environment
path is unset; this pass is Relay owning integration, not new Core proof.
The actual deployed journal receipt SHA-256 is
`53c9c2507d768c512344726f78fc43249379309c526c16231cedd90122f1abdc`.
This closes the immediate staging journal slice, not the complete E4 matrix.
Next action: continue actual broker connection acceptance. Existing MCP processes,
Hub edits and frozen binary remain untouched. No push or npm publication yet.
E4 broker follow-up found an actual render-order defect: a resumed attempt
with `instanceRef` and required new profile-scoped sign-in returned HTTP 404,
because rendering called `bound_service` before authentication to obtain broker
permission controls. Immediate Core fix scope is
`runtime-rs/src/cli/browser_onboarding.rs` and
`runtime-rs/tests/browser_onboarding_stdio.rs`; the existing
`.github/workflows/native-core.yml` additionally runs this exact regression
suite and archives its log before producing native binaries. No new workflow,
provider or qualification bypass is introduced. Proof matrix: unauthenticated
existing-instance page must return 200 with Sign in and without broker permission
controls; the existing cross-origin denial/restart/cancel tests must stay green.
Readiness remains false and ownership/authorization still run before any broker
action. Targeted acceptance: the real stdio/HTTP `browser_onboarding_stdio` test
suite. Frozen artifacts are not patched; release needs a new pinned producer
candidate and its required qualification before this source fix can ship.
The frozen Core lock hash remains unchanged. No broker transaction or Live
policy was changed; this is not full E2 or customer release completion.
Core regression `61401` is terminal exit 0: both actual stdio/HTTP tests pass,
including unauthenticated existing-instance HTTP 200 with sign-in and no broker
permission form, plus restart/cancel/cross-origin rejection. The one-condition
render fix defers descriptor lookup until Hub authentication; it does not bypass
`bound_service` or broker authorization. Full owning `just verify` `5598` is
terminal exit 1 solely at the clean-checkout whitelist requirement. Formatting,
strict Clippy, workspace tests, Nextest (1,339 passed, 61 skipped), deny, audit
and machete passed beforehand. Log: `target/onboarding-integration-verify.log`.
Contract, architecture, public-source and FOSS-boundary checks `85757` pass.
The full gate must pass again on the checkpointed clean revision; none is waived.
Relay dev-cache cleanup `65760` is terminal 0 and
removed 45.5 GiB of rebuildable caches only; preserved frozen release, ignored
operator state and receipts. Reuse owner MCP `27109` and real Warden `8186`.
The current profile-scoped owner needs normal Hub sign-in, not copied session
tokens. The old loopback URL still belongs to the unchanged release binary;
shipping the repair requires a separately pinned GitHub-built Core candidate.
Clean owning gate `82967` is terminal exit 0 against committed Core
`5de65946ca531302616b539d039537fdbeada0ce`, including the full required
`just verify` sequence. Log `target/onboarding-clean-integration-verify.log`
SHA-256: `d548be0c4a299ab2a09cd1920864a73bc39bcd65271e9533d77c63f886450086`.
The unchanged frozen lock remains
`222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`.
That exact Core source is pushed to `codex/f2-npm-distribution`. Actual GitHub
Mac arm64 native run `36971256125` succeeded against that SHA with the new
browser stdio regression included in the existing producer workflow. Public
Core uses the standard free `macos-15` runner; no private allowance cap changed.
Pinned capture and extraction passed: artifact `11212346113`, ZIP SHA-256
`a0157bb40c1818edbfdfcd918aae3ad6cebb8ad827cb5570cbd0e5a3f2965186`,
Core binary SHA-256
`6ac19384ffe3df5ed6404b326d1f63ac5b700692b8a0e7b5126b3f51bea698f3`.
The extracted inventory remains `qualified:false`; this is not candidate
release qualification. Existing owner MCP `27109` was deliberately stopped
after verification and replaced by repaired binary MCP `39130` (PID `54884`),
using preserved isolated state and real Warden `8186` (PID `34090`).
Actual business Hub sign-in and Relay entitlement are verified. Expired v4 was
replaced by v5 onboarding `c50fcd26d2d2dd3c511f9ed7c7cc576324a8d2f6694d528ed2f4e79be695967f`.
The repaired browser flow reaches actual Alpaca consent for TradeAssembly
Staging, `env=paper`, requested `data trading`, without pasted keys.
Chrome CDP navigation timed out; bounded native Chrome navigation succeeded.
The user approved the exact paper-account consent and provider terms. Actual
Alpaca callback completed; OAuth credentials reached the isolated Core store.
The former MCP process had exited, so fresh owner MCP `80253` now uses the same
GitHub-built Core bytes, persisted state and real Warden `8186`. The isolated
runtime now explicitly selects the GitHub-built sandbox launcher plus the
preserved pinned Node/SRT accoutrements; no sandbox bypass or host Node dependency.
Actual MCP broker verification succeeded with connectivity checked and a durable
`tradeassembly.broker_onboarding_receipt.v1` receipt (credential revision 1,
instance revision `1790946478814`, checked-at `1790946626289`). No orders.
Setup status exposed a distinct retry defect: its fixed verification key replayed
the earlier sandbox-unavailable failure, producing `idempotency_replay_failed`
despite successful fresh account verification. The source repair assigns a fresh
random key to each read-only status verification; explicit broker command replay
semantics remain unchanged. Its targeted key/account/mode regression passes.
Actual repaired source-build MCP `40470`, request `601`, resumed that same durable
OAuth attempt without another grant: `ready:true`, Hub authenticated, Relay
permitted, broker connected, real Warden ready. Fresh account health persisted
receipt checked-at `1790947141355`; the restart observation reports `passed`
with current and prior process observations. This uses the real sandboxed Alpaca
account read, not a mock. The unchanged GitHub binary MCP `80253` remains separate.
Next: full owning gates, separately pinned GitHub-built repaired bytes, then
repeat actual status/restart proof on those exact bytes. Source-build evidence
does not qualify the GitHub candidate or complete E4 acceptance.
The actual persisted receipt/restart rows were exported read-only to isolated
`onboarding-retry-source-receipts.json`, SHA-256
`cae768ab83a1cc06a3f5338fe683a7f07d69a37492806b5e168624a91864a1ad`.
Source-probe executable SHA-256:
`0a363d5fa1b4a31ed306e4ea9b2aef5bc99ee5132d0dcd26bef572ad308abd6c`.
Owning verify `43749` passed tests/dependency checks but exited 1 at the clean-tree
language invariant. Checkpoint these scoped changes, then rerun the unchanged
full gate on that clean revision; this is not a waived or passing full gate.
The clean owning gate `49742` is terminal exit 0 against checkpoint
`677693fffce399db22ee467a336fb0deebbd5aa4`, including the full `just verify`
sequence. Log `target/onboarding-retry-clean-verify.log` SHA-256:
`f4e09163bac72ccb9d378b1d2ff5ccd209ff3f3eceb78bbca6bb3154dd7690f2`.
That exact repair is pushed to `codex/f2-npm-distribution`; Mac ARM GitHub run
`37013112271` was dispatched against it. Do not mistake prior run `36971256125`
for this repair. Next: await that run, capture digest-bound native outputs,
update the Mac producer pin only, and repeat real resumed status on those bytes.
The source-only probe `40470` is intentionally stopped. Owner `80253` and Warden
`8186` are preserved. Frozen lock hash remains unchanged; no merge/npm release.
GitHub run `37013112271` completed successfully. Pinned capture passed for artifact
`11228864137`, ZIP SHA-256
`fa2dc5341f8e270f99c511f5512c527e88e1da915524ca753855dc77579a98a5`.
Deterministic extraction passed to `target/github-build-outputs-macos-onboarding-retry`
with `qualified:false`. Core binary SHA-256:
`bb6a2842130c316e71856bb910c37582001c12e299a082ec1424b1e9a297d088`.
Its sandbox launcher is byte-identical to the already pinned launcher in the rig.
Native probe MCP `59508` remains live, using the same isolated persisted state.
Actual requests `701`–`704` prove: the expired onboarding attempt remains expired;
fresh sandboxed paper-account verification succeeds and persists a receipt;
the same explicit command key replays `duplicate:true` with unchanged checked-at;
requesting Live against that paper instance rejects `account_mode_mismatch`.
Native receipt checked-at: `1790948672076`; restart observation: `passed`.
Read-only receipt export `onboarding-retry-native-receipts.json` SHA-256:
`6c1ac80bb27db39b4647ab427f566e481302dd755bebcfef0e23dcd52bb075a3`.
The two actual durable command rows are exported in
`onboarding-retry-native-commands.json`, SHA-256
`90b14b88aafe2640aceedfe9d2551639c31ec544fa996d630de169dea9f42c98`.
No orders, credential replacement, expiry mutation or Live activation occurred.
Next acceptance: a fresh finite browser setup through this exact native process
must reach `ready` before its natural expiry; the expired prior attempt cannot
substitute for this positive proof. Then complete the remaining E4 negatives and
candidate/registry release gates. Do not claim full E4 or npm qualification.
Fresh native attempt v7 `e616bf47a0f19ba58c2081f5bf7ecb7641bc1833bebbb3fc15980563eb3161f0`
completed actual staging browser consent and OAuth exchange on the exact new
GitHub-built bytes. Native MCP `59508`, request `706`, reported `ready:true`,
Hub authenticated, Relay permitted, broker connected and real Warden ready.
The renewed same-account grant is credential revision 2, instance revision
`1790949212495`; its first native receipt checked-at is `1790949214086`.
The browser visibly showed “Connection verified. You can return to your agent.”
Screenshot `onboarding-native-v7-ready.png` SHA-256:
`61b0e2fe6fad704eac557a5f4cbf0be2b612cde97804c9419592a82137cccc7e`.
Observed friction: the original scope-selection tab did not refresh after its
`target=_blank` authorization form opened the provider tab. One explicit refresh
rendered success; MCP readiness itself did not require that refresh. Record this
honestly; automatic browser completion is not proven. Local page/status requests
also incur credential-store lookup latency; do not restart loading requests.
Only the isolated native probe was intentionally restarted. Current native MCP
`37062`, request `711`, again reports `ready:true` with credential revision 2 and
restart observation `passed`; checked-at `1790949385190`. Old MCP `80253` and
real Warden `8186` remain untouched. New browser capability is returned by `711`.
Actual persisted revision-2 restart evidence is exported in
`onboarding-native-v7-restart-receipts.json`, SHA-256
`78f91468a9f74771365e5744614658c6f71fa4fcdd6abc167c98059f52d78eba`.
Positive native OAuth/readiness/restart proof is now closed, not full E4.
Next: extend the existing private Relay `xtask/src/relay_environment.rs` driver
to capture/verify the remaining finite E4 matrix (defined below), retaining its
real owner/nonowner and zero-effect bindings. Record the stale-tab friction in
the customer handoff; no automatic browser-refresh proof is claimed. Production
packaged onboarding, callback/state negatives and production Relay cases remain
required before candidate/registry release acceptance.
Do not restart the live rig or reinterpret partial onboarding as full E4 proof.
No merged, published npm, new broker-order or complete E4 result is claimed.
E4 staging journal both-origin slice is now verified. Private Relay commits
`a5dc56e` (strict v2 capture/verifier and exact edge journal routing), `02321ad`
(isolated configuration-test backend data), and `aac3df2` (Wrangler package cwd)
are local checkpoints; not merged or pushed. The custom host previously sent
journal requests to the OAuth-only service and returned 404. Staging now pins
the existing journal gateway separately, on only three exact method/path pairs;
OAuth backend selection is unchanged. Production edge configuration/deployment
is untouched. Cloudflare staging version is
`c833f37e-6cbe-4f31-9ed1-4709a90f8cdf`, deployed by successful
`just edge-deploy-staging` handle `20557` from `aac3df2`.
Actual deployed probe `57210` and `just relay-environment-journal-verify` both
exit 0. Hub verified the actual owner and nonowner. Both custom and direct
origins reject anonymous/nonowner ingest/read/export (12 actual 401 responses),
owner reads agree, replay has the same receipt and exactly one existing event,
conflicting key rejects 409, wrong workspace rejects 403, query/export agree,
and actual archive hashes are unchanged across denial attempts. The original
event key/timestamp were reused; no extra event, broker order or Live activation.
Ignored evidence: private Relay
`.operator/relay-environments/staging/journal-proof-both-origins.json`, SHA-256
`46c1a83b2bf75ee6a9816b1d6ea2746da90cc964b6decb151a4e6771c9e351b6`.
It binds exact native Core binary SHA-256
`bb6a2842130c316e71856bb910c37582001c12e299a082ec1424b1e9a297d088`,
config, probe source, request and hashed actors. Scope is explicitly
`staging-journal-only`, `fullE4Complete:false`; stale/incomplete/different-input
evidence is rejected by the verifier. This is not candidate/package qualification.
Full owning `just verify` handle `92426` exits 0 on `02321ad`; the later commit
changes only the Wrangler invocation directory, and its actual deploy reruns
edge format/Clippy/tests/build successfully. Integration log is private Relay
`target/staging-journal-both-origins-integration.log`, SHA-256
`0020763f907227ec100c48aab69f62b4452c28dbbcc7e70e59ce1b1402a0984b`.
The optional Core checkout smoke is explicitly skipped, not new passing evidence.
Previous gate attempts exhausted disk or reused expired deployment-backend data.
Only regenerable Cargo dev caches were cleaned; release bundles, receipts and
live processes were preserved. Pure existing mock-provider OpenTofu tests now
use isolated working data and a shared provider cache; no test was weakened.
The original frozen release-lock SHA-256 remains
`222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`.
Next acceptance: remaining finite OAuth callback/admission negatives, then
production packaged onboarding/Relay evidence and candidate/registry gates.
Do not rerun this completed slice or count it as complete E4.
OAuth denial driver is committed locally in private Relay `238538c`. Actual
probe `37653` is terminal exit 0: both custom/direct origins reject anonymous
and nonowner start/status/ack, wrong-environment start, tampered callbacks, and
naturally expired callbacks; the production callback rejects the valid staging
transaction. All 19 checks passed. The isolated owner transaction remained
pending with no grant before natural expiry. No provider consent was opened;
only a synthetic non-grant code was used. This does not claim a measured
provider-call counter or live revoked-owner proof.
Ignored evidence `staging/oauth-denial-proof.json` under private Relay
`.operator/relay-environments/`, SHA-256
`239dbed0b6bda6b92a0ca5db5e8ef64fd80a75b259b762c6cc650434366906b9`.
It binds the same exact native candidate, source, configuration and hashed
actors. Scope is `staging-oauth-denials-only`; revoked-owner and full E4 remain
explicitly false. Deterministic `just relay-environment-oauth-denial-verify`
process `3508` exited 0 against the real evidence and exact native binary in
private Relay `.operator/relay-environments/operator-auth/bin-retry-native/`.
An initial verifier invocation used a nonexistent Core-local path and correctly
failed `candidate unavailable`; only the corrected digest-bound run is passing.
Owning `just verify` process `77746` exited 0 on `238538c`;
log `/tmp/tradeassembly-oauth-denial-integration.log`, SHA-256
`3ef26e23fedabeab23a7ddbd48aac4b1f6b6a53725ca22a3c4dfc2343c5d6e6b`.
The optional Core-checkout smoke is skipped, not passing evidence.
Because the source binding changed, the actual journal probe was repeated on
the same existing event, not a new record. Probe `75451` and verifier passed;
`staging/journal-proof-both-origins-238538c.json` SHA-256
`59b654e9871b044e7dd2d8e08468a1f2196db70afb908111f86bc8f92a024bf0`.
AWS CLI renewal completed through the same remote login `33605` in Chrome,
using the correct corporate Bitwarden entry and MFA. STS confirms account
`056319544861`. CLI readback confirms the staging OAuth function is Active,
last update Successful, revision `ab34e70e-e9e2-4eea-8879-b79469f15bbe`, code
SHA-256/base64 `UccEHtBt5Szri9i/FjWolF4cMikQzKs5ahozFICKUNY=`. Staging policy
was unchanged at that checkpoint. The subsequent revoked-owner proof is recorded
below. Next: production packaged onboarding/Relay and the full candidate/registry
gates. Preserve native MCP `37062`, owner MCP `80253`, and
real Warden `8186`; none of this qualifies or publishes npm packages.
Revoked-owner design is settled in private Relay `xtask/src/oauth_revocation.rs`:
keep every original staging owner, briefly append the verified controlled
nonowner, start one paper/data-only pending transaction, restore the original
policy, then test its unexpired callback through both origins. Require actual
401/`unauthorized`, an unchanged consistent-read DynamoDB row with no grant,
and exact restored environment/code. The deployed callback source is
`apps/broker-oauth-relay/src/lib.rs::callback`: admission is checked before
state claim and provider exchange. This is not a provider-call counter claim.
AWS updates are fixed to staging function/account, fenced by revision, bounded
in time, and reject concurrent unrelated edits. A mode-0600 immutable recovery
file precedes mutation; an independent process restores after 120 seconds if the
probe is interrupted. Normal/error exits also restore before returning. Remote
AWS outage can still require the explicit restore command; do not claim an
absolute restoration guarantee during provider outage. Existing owner remains
admitted, no real provider code or consent is used, production is untouched.
Focused restoration/evidence tests (3/3) and strict Clippy passed. Local Relay
commits `5f0c48a`, `6c5522a` own this driver; not pushed or merged. First actual
probe rejected AWS CLI stdin JSON before any configuration mutation. Exact
original policy was read back, then the explicit restore command `74665` passed.
The corrected driver uses AWS CLI `--environment` for validated non-secret
variables. Second probe `4499` created only a pending transaction, then correctly
failed its overly strict hex digest check and restored admission on the error
path. The deployed digest is base64url; `4d60852` fixes that exact binding, not
the gate. Third probe `91068` failed before mutation because its Hub session
expired while Cargo waited for the integration build lock. Both actors were
refreshed through native CLI. Final `bb336ab` also binds restoration to the exact
AWS account/region/function ARN. Do not reuse stale credentials across build
waits; build the xtask first, then supply credentials to its ready executable.
Actual final probe `48188` exits 0 on `bb336ab`, using that already built Rust
xtask. Both custom/direct unexpired revoked-owner callbacks returned actual
401/`unauthorized`; the consistently read pending/no-grant row was identical
before and after. Row SHA-256:
`5c745d5fd2d704bc8412aeda94fd7ad0e04eef4d71594627d5734f87f118a43a`.
Original owner admission and every other variable were restored exactly; code
SHA remained unchanged. Independent CLI readback confirms Successful update,
revision `d147cb5d-0241-4d84-a048-1d1112e87028`, original allowlist only.
The watchdog was stopped only after successful restoration. Durable recovery
files remain mode-0600 under ignored operator state; no raw credentials/provider
codes were written. One failed-probe pending row and the successful probe row
expire normally; neither has a grant. No broker orders or production mutation.
Ignored actual evidence `staging/oauth-revocation-proof-v4.json` SHA-256
`b853076acba796d6a63169aa1aecbb20b7bf203959bdecc3496168176bd8581a`.
`just relay-environment-oauth-revocation-verify` process `71753` exits 0 and binds
the exact native candidate/config/source/actors, both-origin 401 observations,
unchanged durable row and restored policy. Scope remains explicitly
`staging-oauth-revocation-only`, `fullE4Complete:false`. This closes only the live
revoked-owner case. Earlier journal/denial source hashes remain valid because
their source module is unchanged; this driver is a separate private module.
Full owning `just verify` process `29803` exits 0. Its run began on `5f0c48a`
and continued while the diagnostic fixes were committed; final `bb336ab` has
separate 3/3 targeted tests and strict all-target xtask Clippy (`17710`, exit 0).
No claim of a single pristine-final-revision release gate or merge readiness is
made. Final integration/release checkpoints still require stable owning gates.
Integration log `/tmp/tradeassembly-oauth-revocation-integration.log` SHA-256
`026087230647e91e3137a926266bd81db39e8aa2b212abe1bc4d92a994cc05ec`;
targeted log `/tmp/tradeassembly-oauth-revocation-tests.log` SHA-256
`7653648bac3333ebd218cbaa5acadcf8eb621ac5b4eaac3a1048571589986e8d`.
Optional Core-checkout smoke remains explicitly skipped, not passing evidence.
Next acceptance: exact native packaged production Hub/Alpaca Paper onboarding,
production Relay entitlement/read-write/export and denials, then full private
E4 aggregation and candidate/registry gates. Staging-owner-only and production
configuration overall verdicts remain false until their complete matrix passes.
Production profile inputs were refreshed through AWS CLI: production Hub code
`jRx4TzRaWTBnAFrsolv4/5avoSxDwn0g2mB7J8e+nMg=` declares production, the expected
issuer and `https://hub.tradeassembly.ai`; its dashboard client is distinct from
the already Hub-verified native client. Production OAuth code
`6O8BCylGsrROkipnzYRs1otj04dElCfqhwx6lAtDYb4=` declares production, public base
`https://connect.tradeassembly.ai`, expected production Alpaca client and Hub.
Production gateway is Active/Successful, code
`VZgDzkZoq+3B9BC9MHa0IOtaHLf8dNAUJwSzFnkoFLU=`; gateway/Reach origins remain
the existing isolated production outputs, not the staging endpoints.
Private Relay `cc694fd` creates `deployment-profiles/relay-production-connection.json`
and `xtask/src/relay_production_profile.rs`, with owning verify/test recipes.
Profile SHA-256 `1beb89e9afd501697470ca21d824e28050e117153305b57175c9e7322a4511f1`.
The exact private validator rejects mixed identity, staging origins, unsafe
callbacks, missing entitlement checks, unexpected checkout and secret fields;
2/2 tests and strict all-target xtask Clippy pass. Actual profile verifier
`13427` exits 0 but explicitly reports deployed onboarding and full E4 false.
Log `/tmp/tradeassembly-relay-production-profile-tests.log` SHA-256
`1b6632ac14d5d2279aae11fd881f43729baa59d906807b41102a78fa85f8d415`.
Customer isolation gap discovered: Core required a fixed `organizationId` and
forced it into sign-in. Publishing the diagnostic operator organization would
not produce general customer onboarding. Core `connection_profile.rs` now
accepts null/omitted organization, preserves explicit nonempty organization
binding, rejects blank values, and lets WorkOS/Hub derive verified tenant or
documented personal scope. The production asset uses null; no test tenant is
shipped. Source pointers: Core `ConnectionProfile::{validate,configure}` and
existing WorkOS configured-organization claim enforcement; Hub
`docs/integrations/v1/identity.md` and ADR-001 personal/tenant derivation.
Targeted Core process `50836` exits 0 (3 connection-profile tests). Remaining
owning gates and repaired native-byte qualification are not inferred from that
test. This changes runtime source: the existing `677693f` native binary remains
preserved, but cannot be relabeled as supporting the new null-organization
profile. Next: finish source/boundary/owning gates, commit this scoped repair,
obtain the separately pinned GitHub native build, then perform production
source-free onboarding and required byte-invalidated qualification. Do not
rebuild or requalify unchanged Warden/Alpaca producers, touch the staging rig,
or substitute a manually forced test organization to make onboarding pass.
Strict all-target runtime Clippy `13001` exits 0. Public source scan passed;
whitelist correctly rejected the uncommitted checkpoint. Test log SHA-256
`eb68ae3a71f98ed5b3c54e56fbbdebf6ec87c14017ef82700dceda83ce58e704`;
Clippy log SHA-256
`7113e8bbd7ddcbfc7c4db3567fc597d5d68a43b197a5651e99adc012ac5b396f`.
Clean owning gates are still required before pushing/dispatching new Core bytes.
E2 follow-up is committed locally at Relay `da8b639`: OpenTofu's owner
principal hash now uses the same UTF-8 JSON tuple as runtime `serde_json`,
instead of HTML-escaped `jsonencode` bytes. Quote/backslash escaping preserves
literal escape sequences; no runtime authority rule changed. Fixed digest
vectors cover HTML metacharacters, literal backslashes, quotes and Unicode
line separators in both actual OpenTofu evaluation and Rust admission tests.
`just relay-environment-infra-test` passes 8/8; owning
`just relay-environment-policy-test` passes 5/5 and strict Clippy.
Full `just verify` handle `14785` is terminal exit 0 against this committed
revision. Log `target/relay-environment-integration-verify.log` SHA-256:
`4c767f8326743e8149dc649c7e75bd6119714828ce73993f05b76abaa54f890c`.
This closes the owning integration gate for the E2 configuration slice, not
live deployment or E4 acceptance. Its optional Core checkout smoke reports
skipped; the prior actual exact-pin smoke remains separately recorded above.
Do not restart this completed process or claim the skipped check passed.
E0 tooling: the official WorkOS CLI is available through `npx workos`; its
real API command accepts a Bitwarden-sourced key in the process environment,
with no new Keychain storage. The migrated configuration identifies a sandbox
environment despite its historical `Production` display name. A lookup of the
verified production operator returns `entity_not_found`; do not treat that
credential as production administration or manufacture a valid nonowner.
Exact non-secret environment/client references are in the ignored inventory.
Resolve the correct existing environment access before provisioning/verification.
Do not rerun the completed GitHub Mac
qualifier or reset the frozen baseline.

The September 30 scope amendment deferred Windows/Linux platform acceptance
testing until after the first release. Its former AWS CodeBuild producer choice
is superseded by the GitHub build-provider amendment above. The four-target
scope controls older five-target completion language throughout this document
and the saved goal objective. Intel Mac remains deferred.

| First-release target | Required evidence and customer status |
| --- | --- |
| macOS arm64 | Full existing candidate and public-registry acceptance against exact artifacts; qualified beta platform. |
| Windows x64 | Successful pinned build, target/architecture verification, complete artifact inventory, hashes, notices and published-byte verification; **experimental, platform acceptance not performed**. Intended client OS is current supported Windows 11; a Windows Server build host is acceptable. |
| GNU Linux x64 / arm64 | Same build and artifact-integrity evidence; **experimental, platform acceptance not performed**. Ubuntu 24.04 LTS is the intended initial qualification baseline, not a claimed tested environment. |
| Intel Mac / Windows arm64 | Deferred; no first-release package or support promise. |

Windows/Linux source-free install, npm/pnpm execution, upgrade/rollback, recovery,
controlled-order and OS sandbox/authority acceptance are follow-up release work.
Existing passing component results remain recorded as component evidence. No
missing receipt may be synthesized, marked passed, or borrowed from macOS.
Runtime authority, account/ACL, sandbox, credential, idempotency and fail-closed
checks stay enabled on every shipped platform. A failed runtime prerequisite
must still prevent operation. This deferral does not authorize fixing failures
by disabling enforcement. All actual known build/test failures remain explicit.

Original September 30 implementation sequence (CodeBuild steps superseded by
the GitHub amendment above):

1. Update the release manifest/verifier and npm target mapping together: four
   distributed targets, full acceptance required only for macOS arm64, explicit
   experimental/unqualified status for Windows/Linux, Intel Mac excluded.
   Validate the selected release policy rather than add a generic skip flag.
   Test that missing Mac evidence fails, experimental status cannot claim full
   qualification, and wrong/missing/tampered platform artifacts still fail.
2. Configure short-lived CodeBuild jobs with pinned Core/Warden/Alpaca sources,
   locked dependencies, target-specific artifacts and bounded build concurrency
   and timeouts. Verify AWS credit eligibility and use the existing approved
   operating budget; retain artifact hashes and sanitized logs. Avoid an idle
   reserved fleet. Reuse valid immutable artifacts where source/input hashes
   still match. The cross-repository coordination root owns private build-project
   configuration and source pins; public Core retains provider-neutral
   build/pack commands. Studio Cloud does not own OSS distribution builds.
3. Publish under npm `beta` with visible platform status in package metadata,
   installation output and release instructions. Verify registry package bytes
   for every published target; run actual npm/pnpm install/upgrade acceptance on
   macOS arm64. Do not report Windows/Linux installs as verified.
4. Close only after the revised candidate and registry verifier modes pass,
   applicable owning integration gates pass, scoped source is integrated/pushed,
   and the deferred platform test matrix is linked from the release notes.

The secret/history review below remains mandatory before making Warden or Alpaca
public; CodeBuild can consume their private sources, so changing visibility is
not a prerequisite for this release. Apple payment/signing/notarization and
M7/M8 remain excluded. Original frozen artifacts and prior evidence stay intact.

Historical September 30 checkpoint (superseded by the October 1 GitHub amendment):
the Core, Warden and Alpaca native GitHub workflows are opt-in diagnostics, not
release producers or qualification gates. Their branch-push triggers were removed
to avoid unplanned native builds while the CodeBuild path is prepared. Manual
dispatch is not assumed available from an unmerged branch; no native artifact
receipt may be inferred from these workflows. CodeBuild configuration, real
builds, artifact capture and the separate Mac qualification remain required.

Historical September 30 implementation status: **plan amended; candidate/registry verifier code
in progress; CodeBuild configuration and live qualification still pending**.
The saved goal is active and still
contains its older five-target wording; that wording is superseded here, not
fulfilled. No platform acceptance or release success is claimed by this edit.

2026-09-30 local checkpoint: `cargo test --locked -p tradeassembly-distribution`
passed (21 unit tests; the three native acceptance tests remained ignored).
With `TRADEASSEMBLY_DISTRIBUTION_CANDIDATE` set to the existing
`target/f2-npm-mac-package-beta2` directory, the explicit ignored
`package_managers` test passed: npm and pnpm installed its local tarballs with
lifecycle scripts disabled. This is local Mac package evidence for the existing
beta.2 staging-profile candidate, **not** production Relay, registry, or the
amended release-matrix qualification. Do not reuse this result for changed
candidate bytes.

First-release package-label slice (committed at `eaa61fccbe0d683b5c3a33e8fca72e45e2f2a120`, 2026-09-30): replacement
candidate packaging now excludes Intel macOS from launcher dependencies, marks
Mac arm64 qualification required and Windows/Linux acceptance deferred, and
requires the same policy metadata at the package/launcher integrity boundary.
The new launcher rejects policy mismatches and warns on experimental platforms;
the npm README discloses this support status. Legacy schema-v1 dependency
validation remains unchanged for frozen artifacts. `cargo test --locked -p
tradeassembly-distribution` passed 24 unit tests after the verifier edit;
targeted Clippy, formatting, `node --check packaging/npm/cli.cjs`, and
`git diff --check` passed. The new schema-v2 candidate verifier requires an
exact four-target policy, full Mac arm64 native receipt, and separate
digest-bound experimental build receipts for Windows/Linux. Those receipts
bind sanitized CodeBuild success readbacks and all five producer outputs to
the candidate descriptor. The published verifier now requires downloaded npm
tarballs matching the qualified SHA-256 bytes, npm SHA-512 integrity, fixed
registry URLs, beta tags, Mac npm/pnpm install proofs, and a separate Mac
registry upgrade/rollback proof. The read-only `cargo xtask
distribution-capture-registry --evidence ABSOLUTE_DIR` command fetches the five
package tarballs after candidate qualification; it does not publish. Schema-v1
legacy verification remains unchanged. No schema-v2 candidate or public-registry
receipt has been captured or passed.

Registry-verifier checkpoint: `cargo test --locked -p
tradeassembly-distribution` passed 25 unit tests after adding exact candidate
tarball, SHA-512 integrity, URL and beta-tag checks; read-only npm capture
fails before network access if candidate evidence is missing. The capture
command is wired through Core `cargo xtask` but has not contacted npm or
produced a real registry receipt. Published status also requires a separate
Mac registry upgrade/rollback proof. Targeted Clippy passed and `cargo check
--locked --manifest-path xtask/Cargo.toml` passed. The revised published
verifier has not passed on actual artifacts.

Core integration checkpoint (2026-09-30): `just verify` exited 0 on
`eaa61fccbe0d683b5c3a33e8fca72e45e2f2a120`, including workspace tests,
nextest, language/public-boundary gates and the FOSS publication scan. The
branch has not been pushed and no candidate or registry qualification is
claimed. A separate coordination worktree on `codex/f2-codebuild-distribution`
was created for private CodeBuild configuration; no build configuration or AWS
resource has yet been created. A fresh CLI SSO device authorization required
interactive browser approval; the official AWS integration again returned
`Unknown tool`, which the operations route could not record as a completed
integration attempt. The pending CLI login was cancelled, not restarted.

Frozen-scope proof matrix for this release gate:

| Required behavior | Owner and targeted check | Evidence status |
| --- | --- | --- |
| Mac arm64 remains fully qualified; missing native receipt fails | Core `package::verify_native_targets`; schema-v2 negative unit test | Missing-receipt denial passes; real new-candidate receipt pending. |
| Intel Mac excluded; Windows/Linux never reported runtime-qualified | Core pack/launcher and `first_release_matrix_valid`; policy tests | Static policy tests pass; new package and live install pending. |
| Experimental artifacts bind target, version, source and build outputs | Core `verify_experimental_target`; CodeBuild readback tests | Readback negative tests pass; actual CodeBuild and candidate evidence pending. |
| Published package bytes and installs match the candidate | Core `distribution-capture-registry`, published verifier and Mac install/upgrade probes | Offline byte/integrity negative test passes; actual registry capture and Mac probes pending. |

Next action: resolve B0/B1 source availability, provision the reviewed
CodeBuild resources, run and capture real native builds, then assemble the new
Mac candidate and experimental matrix. Read-only npm capture follows candidate
qualification. Do not classify experimental evidence as native runtime
acceptance.

CodeBuild access checkpoint (2026-09-30): the `tradeassembly-prod` SSO profile
was refreshed, but its only assigned role (`HubProductionDeploy`) cannot list
CodeBuild projects. The separate `default` AWS CLI profile was authenticated
and `sts get-caller-identity` verified account `056319544861`; its read-only
inventory found zero CodeBuild projects and zero CodeConnections connections.
Existing budgets cover Hub production, broker OAuth and Relay, not distribution
builds. The September Cost Explorer result shows applied credits, **not** the
remaining credit balance or CodeBuild credit eligibility. No project, build or
connection was created. Do not dispatch a paid build until its finite maximum
compute exposure and applicable credit/budget headroom are verified.

#### CodeBuild execution contract B0–B5

This closes the source/provenance design; the steps below are not completed
evidence. Private coordination root owns build configuration and capture;
Core owns provider-neutral package verification. Use the existing branch and
producer worktrees. Do not broaden the four-target release policy.

**B0 — Freeze inputs and economics.** Record the three committed producer SHAs,
Core SDK SHA used by Alpaca, exact target/image map, Rust and Node versions,
lockfile hashes, and five expected output roles (Core CLI/runner, Warden,
sandbox, Node runtime, Alpaca plugin). Verify GitHub has each source SHA before
dispatch. Read AWS credit applicability and a budget/cost bound; configure
on-demand builds with no reserved fleet, no webhook, one concurrent build per
project, a finite timeout and queued timeout. Exit: a reviewed private input
manifest and finite maximum build-cost estimate. Stop before dispatch if either
the sources or spending bound cannot be proven.

**B1 — Establish least-privilege source access.** Use one AWS CodeConnections
GitHub App connection restricted to `TradeAssembly/Core`, `TradeAssembly/Warden`
and `TradeAssembly/Alpaca`; authorize read-only repository contents and no
webhook/status-write permissions. CodeBuild projects use that connection as a
source-level credential, an IAM role scoped to the connection and their own
artifact/log resources, and `source.type=GITHUB`. Do not put a personal token
in a project, environment variable, buildspec or release receipt. Exit:
CodeConnections reports `AVAILABLE`, the project role can fetch each exact
private commit, and a wrong-repository probe is denied. An uncompleted GitHub
App authorization is a real handoff, not a reason to invent a source receipt.

**B2 — Create bounded producer projects.** For each of Windows x64, GNU Linux
x64 and GNU Linux arm64, create Core, Warden and Alpaca builds (nine jobs total)
using pinned, target-appropriate CodeBuild images and explicit release build
commands. Alpaca additionally fetches the same Core producer commit as a
`CORE_SDK` GitHub secondary source; the sanitized build readback and Core
candidate verifier must both bind that exact SHA.
Each job uses `cargo --locked`, retains sanitized command logs and emits only
its intended native outputs. Node and the sandbox runtime are separately
inventoried and hash-bound in the assembled target even when sourced from a
pinned upstream package rather than a Rust producer. Exit: project readback
matches the reviewed manifest; dry validation of buildspecs and output paths
passes. Do not call a project definition a successful build.

**B3 — Run and capture genuine builds.** Start builds against exact source
SHAs, one target at a time; stop on failure rather than fan out nine retries.
For every success, run the private `f2-codebuild capture` against the actual
CodeBuild ID. Its sanitized receipt must bind project, image, GitHub repo,
resolved commit, completion and status; extend it to bind the Alpaca secondary
SDK before Alpaca dispatch. Download each immutable artifact, inspect binary
format/architecture, inventory files and notices, and calculate SHA-256 from
the downloaded bytes. Exit: three real producer receipts and all five output
hashes per experimental target. Missing, failed or ambiguous output is a fail.

**B4 — Assemble and verify the new candidate.** Preserve frozen beta.2 bytes.
Assemble a new four-target candidate with production Relay profile binding,
full Mac arm64 acceptance evidence and Windows/Linux experimental receipts.
Run `cargo xtask distribution-verify --candidate --evidence ABSOLUTE_DIR` in
Core. Any missing platform, wrong SHA, stale build, SDK mismatch, artifact
tamper or Mac acceptance gap must exit nonzero. Successful compilation alone
never becomes Windows/Linux runtime qualification.

**B5 — Publish only the qualified bytes.** After owning Core/Warden/Alpaca
gates and the candidate verifier pass, publish exact tarballs under npm `beta`.
Capture real registry bytes and Mac npm/pnpm install and upgrade/rollback
evidence; run `cargo xtask distribution-verify --evidence ABSOLUTE_DIR`.
Integrate and push scoped source under repository gates. Exit: both verifiers
pass on genuine receipts and the public status remains Mac-qualified,
Windows/Linux experimental, Intel Mac deferred. No Apple payment/signing or
Windows/Linux platform acceptance is part of this gate.

B1/B3 contract checkpoint (2026-09-30): the private capture tool emits
`tradeassembly.codebuild-sanitized.v2`, requires an exact `CORE_SDK` secondary
source and revision for Alpaca, and rejects unrequested secondary sources for
Core/Warden. Core's verifier requires the Alpaca SDK SHA to equal its Core
producer SHA. `just f2-codebuild-test` passed three tests; Core
`cargo test --locked -p tradeassembly-distribution` passed 25 unit tests and
`cargo clippy --locked -p tradeassembly-distribution --all-targets -- -D warnings`
passed. This is contract evidence only: the connection created on 2026-09-30
remains `PENDING`, and there is no CodeBuild project, real build, candidate or
registry receipt. AWS billing readback reported $4,966.26 estimated credit
remaining with CodeBuild applicable; the separate $25 monthly pre-credit
CodeBuild budget has 50%, 75% and 90% actual-spend email alerts. At observed
us-east-1 medium rates, nine 60-minute jobs have a $6.30 compute ceiling for
one pass before artifacts, logs or retries. The AWS Connector for GitHub
authorization page is open in Chrome awaiting action-time confirmation before
any app grant or repository selection. No source access is claimed.

B3 artifact-provenance repair (2026-09-30, local only): the capture schema
is `tradeassembly.codebuild-sanitized.v3`. It binds CodeBuild's S3 ZIP location
and reported SHA-256 to the pinned project, source and image. Experimental
receipts now require `buildArchives` for Core, Warden and Alpaca. The Core
verifier checks each downloaded ZIP digest against that readback and checks
that the selected Core, sandbox, Warden and Alpaca output bytes actually occur
inside the matching ZIP; Node remains a separate pinned upstream input. The
private capture tool's eight tests and Core's 26 distribution unit tests,
targeted Clippy, Cargo audit and Cargo deny passed locally. The clean Core
repair commit `641c9d37107a46f1fee6c379410b866dcf42a6fc` also passed
`CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 just verify`, including 1,332
nextest cases. Private CodeBuild inputs were pinned to that revision and lock
digest at this checkpoint; recheck them against the current Core HEAD before
dispatch. The private `capture` command now fetches
the exact S3 ZIP and verifies its checksum before persisting the readback. No
ZIP has been downloaded from AWS, and this does not qualify a platform. The
connection remains `PENDING`, and authenticated GitHub commit lookups still
cannot find the pinned Core, Warden or Alpaca commits as of 2026-09-30.

### Relay production configuration and private staging — release gate

User requirement (2026-09-30): the shipped Relay integration must use production
services. Staging must permit David's identity and necessary scoped service
identities only, not ordinary customer accounts. This gate is independent of the
Windows/Linux acceptance deferral. Production deployment environment does not
select an Alpaca Live account; Paper/Live remains an explicit user choice.

Audit evidence captured 2026-09-30:

- The existing `target/f2-npm-mac-frozen-beta2/bin/connection-profile.json` selects
  `environment: staging`, `relayOrigins: [https://staging.tradeassembly.ai]` and
  `hubBaseUrl: https://hub.tradeassembly.ai`. Preserve this candidate as evidence;
  it must not be advertised as a production-configured Relay release.
- An unauthenticated connection-start probe returned 401. The staging OAuth
  callback returned the application's missing-code 400 without an edge access
  challenge, both through its custom domain and its documented direct AWS API
  Gateway origin. The production callback origin also returned 400. These prove
  routing and anonymous rejection at start, not successful production OAuth or
  owner-only staging access.
- Reviewed Relay source in `f2-relay-reach-handoff`:
  `apps/broker-oauth-relay/src/lib.rs` (`HubIdentityVerifier::verify`, `start`)
  accepts a nonempty Hub tenant/subject with no owner allowlist in that path.
  Both environment stacks configure the same Hub base in
  `infra/aws/broker-oauth-relay/main.tf`. Paid Relay gateway IaC is tagged test,
  names `tradeassembly-relay-gateway-f2`, and uses F2 conformance storage. This
  source inspection is not a readback of the currently deployed Lambda version.
- AWS CLI readback on 2026-09-30 resolved account `056319544861` in `us-east-1`.
  Staging and production broker-OAuth Lambdas and API Gateway endpoints exist,
  but both Lambdas reported the same deployed code hash. The enumerated API
  Gateway routes use `AuthorizationType NONE`; application-level authorization
  must protect both custom domains and direct AWS origins. This readback does
  not prove the currently deployed application's owner policy.
- The deployed Relay gateway/reach/watch functions are tagged F2 conformance
  and reference the test DSQL cluster and test archive bucket. The DSQL
  inventory showed that test Relay cluster and a separate production Hub
  cluster, but no distinct production Relay cluster; no production Relay
  archive bucket was identified. Do not relabel or reuse test resources as
  production. Operator-only detail is in the ignored Relay
  `.operator/relay-environments/inventory.json`.
- Production Hub issuer, public dashboard client and Hub base were read from
  deployed configuration. Cloudflare route ownership and the registered Alpaca
  redirect were not established. Stored WorkOS sessions exist in Bitwarden,
  but a sampled session's asserted identity had expired; it is not current
  verification of David's `(tenant_id, subject_id)`. A separate valid
  non-owner test identity has not been verified. E0 remains open.

#### Executable closure plan E0–E5

This is an implementation plan, not completed evidence. Exact existing source
owners were inspected on 2026-09-30. Continue in the existing Core worktree
`/Users/davidjbeveridge/.codex/worktrees/f2-npm-distribution` and Relay worktree
`/Users/davidjbeveridge/.codex/worktrees/f2-relay-reach-handoff`; check their current
heads/dirty state before editing. Hub remains a dependency through its published
identity/entitlement interfaces. No Hub code change is planned. A missing Hub
contract must be recorded as an explicit dependency, not improvised in Core.

Settled decisions:

- Keep the existing Hub identity service. Sharing a verified identity issuer is
  permissible; it does not grant staging access. Environment admission belongs
  in Relay after identity verification and before enrollment, read/write, OAuth
  provider exchange or node-command effects. Use exact `(tenant_id, subject_id)`
  pairs for David, never caller-supplied email or an entire email-domain rule.
- Add one small shared Rust policy in private Relay
  `packages/relay-environment-policy/{Cargo.toml,src/lib.rs}`. Staging requires a
  nonempty allowlist and fails startup on missing/invalid configuration. A
  request from a valid non-owner receives a denial before side effects.
  Production uses existing Hub entitlement/ownership rules. Existing node and
  service credentials must resolve to an allowed owner and environment through
  their existing verifier; no new universal bypass credential is introduced.
- Apply the policy at the application boundary, including direct AWS origins.
  Edge protection is supplementary. Minimal sign-in/health and OAuth callback
  routes may remain reachable; callback state must reference an allowed owner,
  the same environment and an unexpired transaction, rechecked before exchange.
  Revoking an owner also blocks that owner's pending callback/status/ack flows.
- A shared Hub user token is not inherently cross-environment invalid. Reject
  wrong issuer/audience where configured and cross-environment transaction IDs,
  handoffs, installation/node authority and data references. Do not introduce
  a fake token rejection claim solely from the hostname used to obtain it.
- Create separate production Relay storage and IAM scope from F2 conformance
  resources: new production DSQL database/cluster resources, archive bucket and
  environment-specific state/secret references. Preserve existing test data and
  infrastructure. Reuse versioned schema initialization and adapters; no data
  migration, new database technology or broker credential relocation.
- Production deployment is independent of broker Paper/Live. Acceptance uses
  Paper account discovery and disposable journal data, with no broker orders.

**E0 — Resolve access and freeze the deployment inventory.**
Outcome: one exact mapping from each user-facing route to deployed code, policy
and data resources. Read AWS Lambda configuration/code hashes, API Gateway routes,
IAM resource scopes, DSQL/S3 references and Cloudflare route/origin configuration;
read production Hub issuer/client/capability and Alpaca redirect registration.
Never dump secret environment values or SSM contents. Resolve the AWS CLI session
through the existing SSO profile and operations route. If the recorder again
rejects a completed integration assessment, diagnose that specific evidence
failure; do not fabricate a tier result or restart the whole login repeatedly.
Owning files: Relay `deployment-profiles/f2-relay-aws-dsql.yaml`,
`infra/aws/{broker-oauth-relay,f2-relay-gateway,f2-relay-reach,f2-relay-watch}/`,
`apps/broker-oauth-edge/wrangler.toml`; operator inventory under private
`.operator/relay-environments/inventory.json` (new, excluded from Git).
Exit: verified David tenant/subject, a separate non-owner test identity, actual
environment endpoints/deployment hashes, and distinct production storage/secret
references are recorded. Remaining *values* are discovery inputs, not unsettled
architecture. Stop dependent deployment if any identity or resource is ambiguous.

**E1 — Implement staging admission and deterministic environment checks.**
Outcome: all staging operations reject valid non-owner identities before effects.
Files: new policy package and Relay `Cargo.toml`; OAuth
`apps/broker-oauth-relay/{Cargo.toml,src/lib.rs,src/main.rs,src/bin/broker-oauth-relay-lambda.rs}`;
gateway `apps/studio-f2-gateway/{Cargo.toml,src/hosted_identity.rs,src/hosted_enrollment.rs,src/hosted_gateway.rs,src/hosted_reach.rs}`;
node adapters only where necessary to preserve their verified owner/environment.
Tests must cover missing/empty allowlist, valid owner, same-tenant different
subject, different tenant, spoofed email/headers, invalid bearer, removed owner
with pending OAuth state, and denied enrollment/data/node operations. A router
inventory test must identify every public route and its policy/exception so a
new route cannot silently bypass admission. Assert zero store/provider effects
for denial cases. Production entitlement checks must remain enforced.
Existing acceptance commands (run in Relay): `just relay-verify`,
`just hosted-gateway-test`, `just hosted-hub-test`; use the smallest affected
test filter during iteration and these complete targets at this checkpoint.
Exit: actual tests pass, every exposed route is accounted for, and no authority
or credential is supplied by a caller in place of verified identity.

**E2 — Deploy isolated environments with a rollback boundary.**
Outcome: staging enforces the owner policy; production Relay is deployed with
its own resources and current production Hub/broker configuration.
Files: Relay IaC modules listed in E0 (`main.tf`, `variables.tf`, `outputs.tf`),
new hosted `infra/aws/relay-storage/{main.tf,tests/storage.tftest.hcl,README.md}`
and its provider lock (separate environment DSQL/archive bootstrap),
`apps/broker-oauth-edge/{wrangler.toml,src/lib.rs}` only if routing changes,
`deployment-profiles/f2-relay-aws-dsql.yaml`, and owning `justfile`.
Parameterize gateway/reach/watch environment and resource references, preserving
existing F2 resource names and state addresses by default. Do not apply a rename
that destroys test resources. Add explicit production inputs and module outputs
for function/code/configuration digests and origins. No implicit test defaults
may be used in a production plan. Staging policy must be mandatory in the plan.
Use existing `just relay-build`, `just relay-infra-validate`,
`just relay-infra-init ENV`, `just relay-infra-plan ENV ARTIFACT_ZIP`, and
`just relay-infra-apply ENV` for OAuth. Add equivalent scoped gateway/reach/watch
plan/apply recipes to the same owning `justfile` (these recipes do not exist yet).
Run `just edge-verify` if edge changes. Inspect saved plans for resource isolation
and unexpected replacement before applying. Deploy staging policy first, then
production resources. Capture readback, not just a successful apply exit.
Rollback: retain previous deployment artifacts/configuration; production traffic
can return to its last verified version. Never roll staging back to unrestricted
access: on failure, retain its policy or temporarily deny staging operations.
No schema/data deletion is part of rollback. Exit: deployment readback matches
the reviewed inputs and IAM limits access to the selected environment resources.

**E3 — Produce the customer production profile and reject mixed bundles.**
Outcome: clean customer installation selects production Relay without manual
configuration, while local-only Core use remains available without Relay.
Files: private Relay `deployment-profiles/relay-production-connection.json`
(new public-data-only asset) and owning packaging assembly; Core
`runtime-rs/src/connection_profile.rs`, `distribution/src/{candidate,native,package}.rs`,
`distribution/tests/{packaged_install,package_managers}.rs`,
`runtime-rs/tests/{browser_onboarding_stdio,local_binary_setup}.rs`, and
`packaging/npm/README.md` only as needed. Bind a declared deployment environment
and profile digest into release metadata. Core validates the generic contract;
private production assembly validates the exact approved production endpoints.
Reject production bundles containing staging origins, missing profile hashes,
unexpected identity/callback configuration or mismatched profile/manifest mode.
Preserve immutable baseline bytes. Build a new candidate with new configuration
hashes, reusing unchanged executable bytes and their applicable test evidence.
Commands: `cargo test --locked -p tradeassembly-distribution`,
`cargo test --locked -p tradeassembly-runtime connection_profile`, and affected
existing source-free installation/onboarding tests with the new candidate paths.
Exit: negative cases fail deterministically and clean installation reports the
production profile without changing the user's Paper/Live choice.

E3 Core binding checkpoint (2026-09-30): schema-2 release and candidate
descriptors now bind `deploymentEnvironment` and
`connectionProfileSha256`. Bundle verification checks the inventoried profile
bytes against those fields, and the first-release matrix rejects anything but
production. Legacy schema-1 frozen payloads remain unchanged; generic local-only
schema-2 bundles remain possible. Targeted distribution tests (25/25), Clippy,
format and diff checks pass. This is not the completed E3 gate: the private
approved production profile, exact endpoint validation, new native candidate,
source-free setup and live deployed readback remain outstanding.

**E4 — Exercise real production and staging boundaries.**
Outcome: evidence covers actual deployments and real authenticated identities.
Add one bounded driver under private Relay `xtask/src/relay_environment.rs`,
register in `xtask/src/main.rs`, and expose owning `just` recipes. Planned
commands (must be implemented, not claimed to exist):
`just relay-environment-probe INVENTORY CANDIDATE EVIDENCE` and
`just relay-environment-verify INVENTORY CANDIDATE EVIDENCE`.
The probe performs the calls and captures sanitized responses, status codes,
operation IDs, profile/deployment digests and hashed identity references. Tokens
arrive from the established credential provider/private file descriptors, never
CLI arguments or receipts. The verifier requires every named case, rejects
missing/skipped/stale/mismatched evidence and exits nonzero on any unmet case.
Finite live matrix:

| Case | Required observation |
| --- | --- |
| Anonymous staging operation, custom and direct origin | Rejected; no enrollment, provider exchange or data effect. |
| Real valid non-owner Hub account, custom and direct origin | Hub first verifies the identity; staging then denies it. A missing entitlement or invalid token alone is insufficient proof of the owner restriction. |
| David on staging | Allowed sign-in, OAuth setup and scoped disposable read/write through CLI/MCP; necessary node operations preserve owner binding. |
| Callback/state isolation | Expired, tampered, wrong-environment or revoked-owner transaction rejected before exchange; allowed owner's valid callback succeeds. |
| Production onboarding from packaged Mac candidate | Real Hub sign-in, production Alpaca OAuth consent, Paper account read and status refresh succeed without copying keys. |
| Production Relay access | Existing valid production entitlement allows enrollment and disposable journal ingest/read/export; denied entitlement fails; cross-workspace and cross-environment reads fail. |
| Profile/local behavior | Default Relay setup uses production; local-only Core starts without signing in to Relay. |

Create/reuse one controlled non-owner test account, using normal Hub identity
flows. Do not alter a customer's account. For production Relay positive tests,
use an existing valid entitlement or the existing authorized operator test-grant
mechanism with its provenance recorded; never add an authorization bypass. Such
a grant proves service behavior, not successful production billing. If no valid
test identity/entitlement is available, the corresponding case stays incomplete.
No real order or new charge is required. Dispose of diagnostic records according
to retention policy; record intentionally retained records rather than bypass
immutability. Exit: the live verifier passes the complete finite matrix.

**E5 — Bind evidence to the release and integrate.**
Outcome: a changed package/configuration/deployment cannot inherit the old pass.
Files: Core `distribution/src/package.rs` and release tests for generic external
evidence references; private Relay driver from E4 for deployment-specific proof;
this plan and owning Product release-packet references only. Keep hosted policy
code out of Core. Bind E4 evidence to the exact npm version, candidate/profile
hashes, deployment code/configuration digests and environment policy revision.
Require the private Relay verifier for Relay launch. Core distribution readiness
must separately validate its declared profile; it must not imply Relay billing
or production-service readiness merely because the OSS binary passed.
Run `just verify` in each changed owning repo and Core `cargo xtask verify` at
integration, plus revised candidate/registry distribution verification. Recapture
only invalidated proofs; do not restart unchanged M0–M6 work. Merge/push under
existing repo gates and retain rollback references. Done means actual verifier
passes and integrated source, not merely implemented tests or planned commands.

Execution stays on the selected root model; no automatic agent/reviewer chain.
Dependencies are E0 -> E1 -> E2 -> E4 -> E5, with E3 after E0 and before E4.
Stop at unresolved access, identity, mandatory human login handoff or provider
configuration that prevents the next required observation; checkpoint the exact
failed command and continue only independent in-scope work. No broad auth-tool
rewrite, billing redesign, UI overhaul, Windows/Linux acceptance, broker orders,
Apple signing or data migration is included. No new spending limit is implied.

Exit: E0–E5 meet their stated acceptance and both environment verdicts derive
from the actual bound evidence. Until then report
`RELAY_PRODUCTION_CONFIG_VERIFIED=false` and `STAGING_OWNER_ONLY_VERIFIED=false`.
The current audit has not established either condition or production paid-launch
readiness. This work does not reopen Apple signing/notarization or unrelated UI.

## Frozen input and proof matrix

The preserved parent lock is available at
`target/f2-unsigned-cohort-release-lock.json` in this worktree, copied unchanged
from `/private/tmp/f2-unsigned-cohort-release-lock.json`. Its SHA-256 is
`222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`.
The Mac arm64 bundle manifest SHA-256 is
`6740c7d7f4e8a692ff005dd18b6dc2656883eb8e366253e4e8e6aa054108f5dc`.
Preserve these baseline artifacts and their 846 payload files unchanged. A new
candidate may be built in a separate directory under the amendment below; it
receives its own hashes and qualification. The first-release four-target matrix
requires evidence for each target's actual bytes, with full native acceptance
only on Mac arm64 and explicit build-only evidence on Windows/Linux.

| Requirement | Owner / deterministic acceptance | Evidence |
| --- | --- | --- |
| Frozen bytes and parent identity | distribution verifier: manifest, every file, target, parent-lock digest | package release manifest |
| npm/pnpm delivery with scripts disabled | packed and published install tests | native command outcomes |
| Safe extraction | traversal, escaping links, extras, corruption, mode tests | Rust tests |
| Persistent source-free installation | actual frozen binary setup, Warden and offline Alpaca | native installer acceptance |
| Reinstall, compatible update and rollback | actual versioned installer, preserved state and authority | installation receipt |
| Reject running/incompatible rigs | real SQLite state and native process inspection | negative acceptance |
| Platform enforcement | real Warden and controlled broker sink per new target | native target qualification |
| Four-target first-release beta | exact npm versions, Mac qualification and three experimental build receipts | schema-v2 distribution matrix gate |

First-release targets: `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`. Intel Mac, Windows ARM
and musl are excluded. Platform dependencies
and any elevation must be explicit; no global security downgrade or unsandboxed
fallback. No target is publishable merely because it cross-compiles.

## Installation contract

`npx --yes tradeassembly@VERSION install` and
`pnpm dlx tradeassembly@VERSION install` install a verified payload into an
owned per-user root, not an npm cache. The root contains immutable `versions/`,
stable `authority/bin/warden`, persistent `state/local`, stable `bin/`, and an
atomic current/previous installation record. Node/npm or pnpm is a prerequisite.
No dependency lifecycle scripts or customer Cargo builds are needed.

`install --upgrade` requires no active activation, agent session, unexpired
execution lease, or running owned runtime process. It does not stop a strategy,
kill a process, reset credentials, or place an order. First-version upgrades
support unchanged state compatibility and Warden digest only. An incompatible
authority or database migration must be supplied and qualified by a future
release; blindly changing installation metadata is forbidden.
External Postgres or NATS state is not covered by this local upgrade path and
fails closed pending a qualified migration. Existing configuration is preserved;
only the verified versioned sandbox path is updated. Frozen Core setup verifies
bootstrap JSON byte-for-byte, so upgrades must not replay it against user-edited
configuration.

An interrupted setup leaves a resumable transaction, never a successful pointer.
Rollback preserves state and is subject to the same compatibility and stopped
checks. No rollback is represented as reversal of a database migration.

## Completion boundary

`cargo xtask distribution-verify` must fail until published npm/pnpm installation
and native enforcement evidence are present on Mac arm64, registry bytes match
on all four targets, and three GitHub Actions experimental receipts pass. Package
tests, builds and a Mac-only installer are not whole-plan completion. The original
M0–M6 lock and false M7/M8 verdicts remain unchanged. Run owning Core gates before
any merge; publish only a `beta` tag after authenticated namespace ownership and
native qualification. Ad-hoc signing and npm provenance are not notarization.

Publication has two gates, not a circular pre-publication registry requirement:
`distribution-verify --candidate --evidence DIRECTORY` requires Mac native
qualification, three experimental build receipts and local Mac npm/pnpm tests
before publishing. Default
`distribution-verify --evidence DIRECTORY` additionally requires installs from
the actual public registry. Candidate qualification is never customer-release
completion. Matrix receipts use `tradeassembly.distribution-native.v2`, bind the
exact release manifest, installer descriptor/binary and npm launcher/platform
tarballs, and identify nonempty hashed native evidence for each check. V1
manifest-only receipts fail closed. They must be generated from actual
acceptance, not filled with assertions based on compilation or mocked brokers.

## Maintainer acceptance commands

Use absolute, new output directories and the selected candidate's qualified
bundle and evidence. The commands below describe the original baseline; the
amendment adds candidate qualification and supersedes its fixed version/pins:

```text
cargo build --release --locked -p tradeassembly-distribution
cargo xtask distribution-pack --bundle BUNDLE --parent LOCK --version 0.1.0-beta.1 --installer INSTALLER --out NEW_DIRECTORY
cargo xtask distribution-freeze-native --input STAGING --metadata INPUTS_JSON --parent LOCK --out NEW_NATIVE_BUNDLE
cargo test --locked -p tradeassembly-distribution
TRADEASSEMBLY_DISTRIBUTION_PACKAGE=NEW_DIRECTORY/tradeassembly-darwin-arm64 cargo test --locked -p tradeassembly-distribution --test packaged_install -- --ignored
TRADEASSEMBLY_DISTRIBUTION_CANDIDATE=NEW_DIRECTORY cargo test --locked -p tradeassembly-distribution --test package_managers -- --ignored
just verify
```

The opt-in tests create isolated temporary state, use actual bundled Core and
Warden, register offline Alpaca through authorization, and reap only owned test
processes. They cover stopped compatible packaging-version update/rollback,
active-state and actual process denial, partial-transaction recovery, identity
preservation, and continued operation after package-manager directories are
deleted. They do not connect Alpaca accounts or place broker orders. The npm/pnpm
test is offline local-tarball evidence, explicitly not registry evidence.

The original cohort fixes its Mac arm64 payload. The amended candidate contract
updates candidate pins without invalidating structurally valid previous
installation records. Changed Warden or state
compatibility still requires an explicit, separately qualified migration.

For a new target, native freezing requires an explicit schema-1 input descriptor
(`distribution/src/native.rs`), the actual target host, three source revisions,
four binary digests, and the Alpaca archive/manifest digests. It checks binary
architecture, Node `22.23.2`, locked and installed SRT `0.0.67`, contained links,
and complete payload inventory. Windows staged links are rejected. This is
trusted-build-input validation, not source attestation or runtime qualification;
the command explicitly reports `qualified: false`. Preserve the original Mac
arm64 bundle; extend this command for a separately identified new Mac candidate.

## Current implementation and remaining release gaps

Latest continuation checkpoint (2026-10-02): staging journal, real OAuth owner
readiness/restart, actor/state/expiry negatives and revoked-owner callbacks have
their partial live proofs above; full E4/registry acceptance is still open.
Private Relay `cc694fd` owns the tenant-neutral production profile and exact
validator. Core `connection_profile.rs` has the required optional-organization
repair; focused tests (3/3) and strict all-target runtime Clippy passed. Source
scan `18325` passed; its whitelist command then correctly rejected uncommitted
changes. Commit this repair before clean owning gates; do not waive that check.
Core owning log: `/tmp/tradeassembly-customer-profile-core-verify.log`.
Relay owning log: `/tmp/tradeassembly-customer-profile-relay-verify.log`.
Reuse their live processes rather than starting duplicate gate runs.
No other test/build agents are being started. Keep MCP `37062`/`80253` and Warden
`8186` untouched. Next: finish this source checkpoint and stable owning gates,
then GitHub-built repaired Core bytes, production source-free onboarding,
remaining production Relay matrix and candidate/registry qualification. The
current `677693f` binary cannot accept the new null-organization profile. No
producer rebuild, Apple step, broker order, merge or npm publication is implied.

The Rust distribution crate and npm shim implement packing, byte verification,
safe extraction, private per-user roots, source-free local setup, real-authority
offline plugin registration, durable transaction guards, and stable launch paths.
Do not mark the five-target release complete from these local results.

Checkpoint (2026-09-28): branch `codex/f2-npm-distribution`, based on Core
`a16d4a769258fb7354d9102017533850020f8f2c`. Local implementation commits
`e1557c9`, `890eb5c`, `3abcafb`, and `36dd8aa` cover the installer and native
portability checkpoints. The latter is the source revision qualified below.
The implementation branch is pushed as draft PR
https://github.com/TradeAssembly/Core/pull/2. No merge, npm publication, or
namespace-ownership verification has occurred. GitHub CI is not claimed green.
The local candidate is `target/npm-qualified-candidate`, version `0.1.0-beta.1`;
its installer SHA-256 is
`15a5703a0431c412061630f5ed5e435f45416eea1c5783a3fb7f5289866d53aa`,
archive SHA-256 is
`7c0a4d6b55555a4a0e0a39bd9b65621879c0240e8b6ca663411973fb2dbdd8a4`,
and release-manifest SHA-256 is
`3f71fb0b87eb1f1bd1eb77fef54755fe045b262dfe1651052ae590a8f909e486`.
The original bundle and parent-lock hashes above remain unchanged; original
Core/Warden strict ad-hoc signature checks pass.

Passing local evidence: eleven Rust unit tests; distribution strict Clippy;
actual frozen-binary installer acceptance (including edited-config preservation,
external-state denial, running/active-state denial and interrupted recovery);
actual offline local-tarball npm/pnpm acceptance with lifecycle scripts disabled;
format/diff checks, dependency deny and machete checks, and current source
whitelist/public-boundary/architecture checks. Native test logs are
`target/distribution-install-final.log` (SHA-256
`a47466630195064a30e9e14ee75f6dc4092810c7cad14478cf16f660f7a8befb`) and
`target/distribution-package-managers-final.log` (SHA-256
`bc4fc43d0656f85450ecf61d6c62d3e466a0a245b7e9d2d9f82bcdaebb1af251`).
Both npm tarballs are retained in the candidate directory. The current installer
passes strict ad-hoc signature verification. Five launcher tests and all four
other-target compilation checks pass (`target/native-launcher-verification.log`,
SHA-256 `b0cc3322a0a1daaf9191793e58408e0032bde6fa0100135a7961fe9ee22ad06e`).
These are Mac arm64 evidence and cross-compilation checks, not
five-target native or registry qualification; the local candidate remains
`publishable: false`.

The required full `just verify` (which invokes `cargo xtask verify`) **passed at
commit `3abcafb`** after reclaiming disposable package caches and using serialized,
stripped compilation. Workspace tests, strict Clippy, dependency/security/source
gates and nextest passed (1301 passed, 51 explicitly skipped). Log:
`target/core-verification.log`, SHA-256
`674bd7e78065ad7b12809f6a31f600cf38062c44df19ff7f3555ed59edf55e41`.
Clean-revision source archive qualification also passed at `3abcafb`; log
`target/source-archive-verification.log`, SHA-256
`4a19e81fe8ce983f81ac9bcbed02013b5f300a4d147b8f890d6e930e1ac64ce0`.
The complete gate also **passed at `36dd8aa`** for the native portability edits:
1306 nextest tests passed, 51 explicitly skipped; all subsequent gates passed.
Log `target/core-native-verification.log`, SHA-256
`e8671539bc739561fe4e5ab20845512b16a038d51a97b4100bf5ac5baa52b630`.
Archive smoke remains evidence for `3abcafb`, not the later source revision.
No skip is claimed as native enforcement, and the candidate matrix still fails
closed without all native target receipts. No owned test processes remain.

Native target production remains a release dependency. The existing
`xtask/src/bundle_local.rs` is Mac arm64 only. The new source sandbox launcher
supports Linux/Windows dispatch and Windows `.exe` lookup, but the frozen Mac
launcher remains unchanged. New target
payloads need native Node, Warden, Alpaca and sandbox prerequisite packaging,
including Windows SRT installation/ACL checks, and actual controlled-sink
qualification. The distribution wrapper cannot cure a missing native runtime.
The pinned Warden SQLite authority explicitly rejects non-Unix secure integrity
locking and integrity-key creation/loading (`warden-storage-sqlite/src/lib.rs`).
Thus native Windows requires a separately qualified authority-storage port; it
cannot pass by changing the package name or using permissive file access.
GitHub Core has no native workflows/self-hosted runners; Warden and Alpaca have
no published native sidecar releases. Alpaca's package builder also needs native
Windows `.exe` path handling. These are concrete producer/qualification gaps,
not npm installer test failures.
Authenticated npm namespace ownership, package publication/provenance, and public
registry installation tests are still required. Apple notarization remains
excluded. Keep one current-state section here rather than per-repair plans.

## Bounded completion goal (2026-09-29)

Complete this distribution layer across the five targets above. **M7 and M8
remain excluded and false.** This goal is not Apple enrollment, Developer ID
signing, notarization, Gatekeeper certification, a paid Relay launch, or the
Product full/public/commercial release lock. An explicitly labelled npm `beta`
distribution is in scope; it does not certify M7/M8 or a production SLA.

Resume Core branch `codex/f2-npm-distribution` and draft PR #2 at `520be81`.
The worktree was clean when this goal was defined; no worker agents remain live.
Reuse the original Mac candidate as baseline/upgrade input and preserve its
inventory and parent lock byte-for-byte. Follow the amendment below for the
replacement candidate and runtime boundary repairs. Targets receive independent
native qualification; revalidate evidence whose actual inputs change.

| Step | Bounded outcome and owning surfaces | Deterministic exit |
| --- | --- | --- |
| D1: settle remaining contracts | One recorded design for Windows private authority storage, native builder/artifact handoffs and receipt generation. Inspect Warden `warden-storage-sqlite` / `warden-service`, Alpaca `xtask`, Core `distribution`, `packaging`, `runtime-rs/src/local_install.rs` and sandbox launcher only. Specify exact native test commands and proof artifacts before edits. | Update this document with settled contracts, source pointers and each target's executable qualification commands; no unresolved authority/receipt binding. |
| D2: native producers | Implement only secure Windows storage/locking/ACL support necessary for Warden, native Alpaca executable packaging, and reproducible native Core/Node/SRT/artifact production. Changes belong to the owning repo's issue branch; preserve Unix behavior and the frozen Mac artifacts. | Owning producer gates plus actual native positive/negative Windows authority tests pass. Target-native inputs freeze and pack with explicit digests, notices and versions. Compilation alone fails this exit. |
| D3: five-target candidate qualification | Run actual source-free setup, stopped upgrade/rollback, active/running denial, interrupted recovery, real Warden controlled sink, duplicate/ambiguous-outcome recovery, and OS filesystem/egress denial. Use disposable state; no real broker endpoints or orders. Include native Windows SRT elevation/account/WFP and private ACL checks. | Required owning Core gates and `cargo xtask distribution-verify --candidate --evidence ABSOLUTE_DIRECTORY` exit 0 from genuine, digest-bound native receipts for all five targets. No skipped or mock-only proof substitutes. |
| D4: npm beta delivery | Authenticate intended npm account, verify namespace ownership, publish the exact-qualified platform packages and launcher only under `beta`, with appropriate integrity/provenance evidence. Verify actual npm and pnpm registry installs with lifecycle scripts disabled on every target and the compatible upgrade path. | `cargo xtask distribution-verify --evidence ABSOLUTE_DIRECTORY` exits 0, binding the published exact version and real native registry-install artifacts. Default/latest and M7/M8 readiness remain excluded. |
| D5: integration and handoff | Finish the scoped Core PR and necessary producer PRs under each repo's merge/CI rules; retain durable non-secret evidence, hashes, source pins, install/update/rollback instructions and prerelease/platform disclosures. No unrelated refactor or product work. | Required merge gates pass; published registry bytes match qualified artifacts; final heads are committed/integrated/pushed; clean worktrees and a reconciled proof matrix. Existing M0–M6 lock hashes remain unchanged and M7/M8 stay false. |

Stop successfully only after D1–D5 have actual passing evidence. Do not redefine
completion as a draft PR, Mac-only delivery, cross-compilation, or a boolean
claim. Missing native runtime support is implementation work, not automatic
permission to drop a target. Preserve progress on real dependencies; no fabricated
receipts, weakened sandboxing, substituted WSL, trading activation or broker
orders. If a required action needs a materially different scope or mandatory
human handoff, checkpoint the precise decision and affected acceptance instead
of claiming completion. Unchanged optional improvements stay deferred.

Execution uses the active root model and deterministic tools first. No automatic
review chains, mandatory delegation or full-history forks. Respect applicable
producer review gates. Run the smallest relevant tests while editing, then full
owning gates at integration boundaries. Reuse live builds and cache/evidence;
do not repeat full gates or rediscover contracts after every repair. Keep the
current step, decisions, files, exact passing/failing checks, process handles
and next action in this existing current-state document. Use CLI, then integration,
then supported API, then Chrome for external operations. Standing authorization
covers necessary in-scope routine access; mandatory safety/human gates still apply.

### D1 decisions and proof contract

Windows authority storage stays in the owning producers. Warden owns a generic
private-files module in `warden-storage-sqlite`, reused by `warden-service`.
Core owns its own small internal private-files adapter shared by distribution
and runtime; neither imports private Warden source or a hosted service. Use
safe typed `windows-permissions` APIs on open handles and `atomicwrites`
write-through moves; no project unsafe FFI or PowerShell secret handling.
Unix formats, HMAC/anchor bindings and locking remain unchanged.

Open Windows paths without following reparse points; reject reparse components,
network paths, null/broad/unknown ACLs and ownership mismatches. Hold private
directory handles without delete sharing while creating children. Newly created
empty directories receive a protected, owner-only inheritable DACL before any
secret is written. Validate handle ownership and DACL before reading secrets or
locking. Existing private ancestors may grant the current user, SYSTEM and
Administrators (privileged OS principals); private secret files grant only the
current user. Do not silently tighten an existing unsafe secret. Synchronize
content then use same-filesystem write-through rename for publication; durable
pending-marker removal uses a write-through rename to a harmless tombstone.
Do not claim directory `sync_all` works on Windows or treat it as a successful
no-op. Native tests must cover ACL denial, reparse denial, competing locks,
interrupted writes and stable identity across restart.

Source pointers: pinned Warden `warden-storage-sqlite/src/lib.rs`
(`IntegrityLock`, private read/write/remove) and `warden-service/src/main.rs`
(private seed/token/export files); Core `runtime-rs/src/local_install.rs`,
`runtime-rs/src/local_owner_identity.rs`, `distribution/src/install.rs` and
`distribution/src/lib.rs`. The frozen Mac Warden source lock is `ffa6889b`
(the older `e5b926c` claim was disproved by the real storage transition test);
changes use a new
producer branch, not the modern detached checkout or frozen release copies.
Alpaca changes branch from `e4ea4c`, limited initially to
`xtask/src/main.rs`: native `.exe` selection, matching descriptor paths and
rejecting a foreign `TARGET` label on a host-built binary.

The exact SDK `0.1.0` is not yet published to crates.io. Native producer builds
use the already documented command-line Cargo patch to the pinned public Core
SDK tree (`1e97d7e09ff4c20bc8c099474dfd5dbdcddd72b7`), without changing
Alpaca's registry-addressable declaration or committing a local path dependency.
Builder provenance records that source tree and override. SDK publication is
not required for customer binary/npm installation and is not added to this goal.

Native builders use standard GitHub runner labels `macos-15-intel`,
`ubuntu-22.04`, `ubuntu-22.04-arm`, and `windows-2022`. Preserve the already
frozen Mac arm64 bundle and qualify its replacement on a native arm64 Mac.
Producer workflows run in their owning private repos;
Core never receives private source or a broad personal token. Root downloads
producer artifacts with authenticated `gh run download`, verifies run/head SHA,
target and digests, then stages only non-secret binaries and notices as candidate
Core release assets. These assets are input transport, not release qualification.
Each workflow has explicit timeouts, bounded retries and maximum parallelism;
actual runner/billing rejection is a dependency, not an assumed limitation.

`distribution-freeze-native` must bind `plugins/alpaca.json` target, version,
source revision, package name/digest and manifest digest to `native-inputs.json`.
Never copy the Mac lock into another target or overwrite the frozen Mac lock.
Receipt v2 binds the release manifest, installer descriptor **and actual
installer**, npm launcher and platform tarballs, source revisions and native
host. The old manifest-only receipt is insufficient: changing the installer
does not change `release.json`. Altered installer/package bytes must invalidate
qualification. A capture command records actual process exits and hashed output;
it does not accept user-provided success booleans as end-to-end evidence.
Its `artifacts` object contains `installerDescriptor`, `installer`, `npmLauncher`
and `npmPlatform`, each with `artifact` (contained relative path) and `sha256`.
`launcherSha256` binds the extracted `cli.cjs`. Platform tarball contents must
match the qualified installer, descriptor, release and bundle archive; launcher
metadata must pin all five optional dependencies to the exact version and have
no lifecycle scripts. `host` records `os`, `arch` and `target` from the executing
capture process. In addition to the original checks, receipts require
`interruptedRecovery`, `preservedConfigurationIdentityState`, and on Windows
`windowsPrivateAcl`, `windowsSandboxAccount`, `windowsSandboxElevation`, and
`windowsSandboxWfp`. A receipt field is a binding, not proof by itself.

Native acceptance entrypoints to implement/run are:

- Owning Warden: `cargo test --workspace`, including native private-file ACL,
  no-follow, lock-contention and crash-recovery tests; `cargo xtask verify`.
- Owning Alpaca: `cargo test --workspace`, `cargo build --release`,
  `cargo xtask package`, `cargo xtask verify`; inspect/execute the actual native
  descriptor binary, including `.exe` on Windows.
- Core: `cargo xtask distribution-freeze-native --input ABSOLUTE_STAGE
  --metadata ABSOLUTE_INPUTS --parent ABSOLUTE_PARENT --out ABSOLUTE_BUNDLE`,
  then `cargo xtask distribution-pack` using that native bundle/installer.
- Core qualification capture (new): `cargo xtask distribution-qualify
  --candidate ABSOLUTE_CANDIDATE --out ABSOLUTE_EVIDENCE`. This runs source-free
  installation and actual npm/pnpm `--ignore-scripts` packages, upgrade/rollback,
  active denial and interrupted recovery against isolated private state.
  Run the actual stdio MCP driver through Core and real Warden to the controlled
  broker executable; capture duplicate and lost-response reconciliation with
  submission count exactly one. No scheduler tick or real broker endpoint.
- Native sandbox checks use bundled Node helpers, not Mac `/bin` commands.
  They demonstrate a reachable controlled endpoint outside sandbox and denial
  inside, plus protected-file denial. Windows additionally captures actual SRT
  account/elevation/WFP/ACL behavior; unsupported/skipped is a failed target.
- Matrix exits remain `cargo xtask distribution-verify --candidate --evidence
  ABSOLUTE_DIRECTORY` and, after beta publication/native registry installs,
  `cargo xtask distribution-verify --evidence ABSOLUTE_DIRECTORY`.

The capture command and composite controlled-boundary tests are implementation
requirements, not existing passing evidence. Direct sink tests alone, current
Warden prepare/receipt tests alone, and cross-compilation do not satisfy them.

### Runtime repair and candidate amendment — active goal

This amendment supersedes packaging-only restrictions and the historical
Paper-only interpretation below. The user activated the amended goal on
2026-09-29 with **GPT-6 Sol Medium**. The goal is active; the text below is its
durable execution contract.

#### Settled scope and design

- Preserve the original bundle and M0–M6 lock at the hashes above. Treat them
  as immutable provenance and upgrade inputs. Build a new candidate separately;
  old evidence does not certify changed runtime bytes. No M7/M8 work is added.
- Add explicit `tradeassembly.order.submit` and
  `tradeassembly.order.reconcile` MCP tools. These names are the planned API,
  not existing functionality. Submission accepts execution/run identity,
  selected broker instance, caller-defined order, and an idempotency key.
  Reconciliation accepts an existing submission/receipt identity and its own
  idempotency key. Never accept a caller-authored owner, lease, Warden permit,
  risk verdict, or capability grant as authority. Reuse existing order schema
  and stored configuration; reject conflicting mode/account overrides.
- Bind both tools to the existing authenticated MCP execution context from
  `cli/external_mcp.rs` / `agent_runner/external_session.rs`. Check identity,
  tool grant, current lease/fencing token, deployment/run/activation binding,
  strategy/configuration revision, broker/account binding, current controls,
  risk limits and authorization before dispatch. Recheck mutable authority at
  the existing commit/dispatch boundary. A bare setup connection cannot submit.
- Route through the shared service and broker submission/recovery machinery.
  Preserve the deterministic evaluator's rejection of externally driven ticks;
  agent submission requires no scheduler tick. The deterministic path retains
  its existing behavior and reaches the same applicable risk/authority checks.
- Keep explicit Paper and Live modes. Extend the shared admission/recovery
  boundary to Paper where currently bypassed, with mode-appropriate policy.
  Live continues to require its existing mandate, account and C5 checks.
  Do not copy Live policy into Paper or let mode changes bypass authorization.
- An isolated controlled sink may exercise both modes using a test-only
  authority configuration and disposable activation/session state. It must be
  a separate executable with no broker credentials or real broker endpoint;
  enforce that isolation in the driver. Never change the installed user's
  policy, activate a real trading rig, or send a real broker order. This
  supersedes the older prohibition's overly broad interpretation of isolated
  test activation; production Live activation remains outside this work.
- Persist an intent/dispatch claim before effects. Same key/same input returns
  the original result or reconciliation-required; changed input with the same
  key is rejected. Recovery observes and records the broker outcome without
  blind resubmission. Unknown remains quarantined until a trusted observation
  resolves it. `studio.agent_run.recover` is not broker-order reconciliation.
- Correct misleading status documentation: only advertise implemented tools.
  Do not solve this by exposing arbitrary plugin invocation or by trusting MCP
  request flags such as `allow_mcp_order_submission` as identity/authority.

#### Ordered implementation and acceptance

Execute these six finite work packages within D1–D5. Finish each package's
acceptance and record evidence in the existing checkpoint before advancing.
No separate plan/ledger per repair. The code audit above is the starting point;
only inspect additional source needed to resolve an actual implementation gap.

**R1 — Define and implement the agent order boundary (D1/D2).**
Outcome: an attached external agent can submit its chosen order through the
same enforced machinery and obtain a durable submission identity.
Files: Core `runtime-rs/src/mcp.rs`, `service.rs`, `cli/mod.rs`,
`cli/external_mcp.rs`, `agent_runner/external_session.rs`,
`service/agent_deployment.rs`, new `service/agent_orders.rs`,
`broker_submission.rs`, `broker_submission/{admission,risk,recovery}.rs`,
`adapters/plugin_operations.rs`, `service/broker_recovery.rs`, and existing
policy/schema files only where required by these explicit tools.
First trace the existing attach-to-service execution context and record the
exact reused binding types; settle schema fields and error codes before coding.
Reuse existing storage namespaces and claims where possible. If a schema or
Warden protocol migration is actually needed, specify it before implementation;
do not silently declare `f2-local-v1` compatibility.
Acceptance: targeted Rust cases reject missing attachment, identity mismatch,
expired/replaced lease, revoked grant, stale configuration, mode/account
mismatch, pause/kill control, risk violation, missing Live mandate, and Warden
denial. Each rejection produces zero sink submissions. Accepted Paper and
isolated Live paths retain exact authority/receipt bindings. Existing
deterministic-path tests remain green. Test the behavior, not just tool names.
Commands: `cargo test --locked -p tradeassembly-runtime agent_order` (new named
test group; require nonzero executed cases), then existing affected admission,
recovery and deterministic execution tests identified from their exact names.
Stop this package on passing boundary behavior; no new scheduler, strategy
language, risk model, UI, agent hosting, or generic plugin API.

**R2 — Prove actual MCP submission and fault recovery (D3).**
Outcome: a source-free installed binary demonstrates the promised agent loop
interface, including recovery after a crash or lost response.
Files: new Core `runtime-rs/tests/agent_order_mcp.rs`, existing
`tests/common/controlled_warden.rs`, `tests/common/controlled_broker_package.rs`,
`examples/f2_controlled_broker.rs`, and `tests/local_binary_setup.rs` only as
needed for shared real-process setup. The driver starts actual packaged Core
stdio, authenticates/attaches using the supported session path, starts real
Warden and a controlled sink, and drives the public tools. No direct service
injection may stand in for the final end-to-end assertion. A simulated
external-agent driver is sufficient; paid model inference is not required.
Cases: successful submit; sequential and concurrent duplicate; conflicting-key
payload; sink commits then drops response; Core restart during uncertain
outcome; reconciliation with found/absent/unknown outcomes; stale/replaced
session; revoked authority; risk rejection; clean detach/re-attach. Prove one
sink effect for an accepted order across duplicate/recovery cases and zero for
denials. Persist ordered journal/receipt references and observe current state;
do not prepopulate success evidence or enable a shipping policy exception.
Command: with explicit new-candidate Core/Warden paths,
`cargo test --locked -p tradeassembly-runtime --test agent_order_mcp -- --ignored`.
Require actual executed tests and captured exits/transcripts/sink counts.
Stop when this finite matrix passes in both applicable modes. Unknown outcomes
must fail closed; no new automatic strategy promotion or autonomous recovery
policy beyond existing user-configured behavior.

**R3 — Qualify a replacement candidate and compatibility (D2/D3).**
Outcome: a new exact-version candidate can be installed, upgraded to and rolled
back under the existing stopped-rig and state-compatibility rules.
Files: Core `distribution/src/{lib,native,package,install}.rs`,
`distribution/tests/{packaged_install,package_managers}.rs`, packaging pins,
and the existing distribution xtask routing/verification implementation.
Replace the old Mac-only hardcoded candidate restriction with a versioned,
explicit candidate descriptor pinned to approved source/artifact hashes and
the immutable baseline digest. Do not simply remove hash verification. Retain
strict target, inventory, parent/provenance, installer, tarball and receipt
bindings; reject baseline-receipt reuse for replacement bytes and tampering.
Extend native freezing for a new Mac arm64 output without overwriting baseline.
Select an unused beta version after checking the registry; do not assume a
specific version remains unpublished. Every target uses that exact version.
Test baseline-to-candidate upgrade on Mac arm64 with unchanged state/identity/
configuration and compatible authority, then rollback. On other targets test
compatible version upgrades of their actual qualified native payloads. Where
Warden bytes differ, do not falsely call them compatible: either keep that
target's authority stable or design and qualify an explicit migration. No
cross-platform authority migration is required.
Commands: `cargo test --locked -p tradeassembly-distribution`, explicit ignored
`packaged_install` and `package_managers` tests, plus candidate manifest/digest
negative tests. Retain the existing no-running-rig and interruption cases.

**R4 planning refinement — platform scope and public producers (2026-09-30).**

The approved first-release scope and October 1 build-provider amendment above
supersede this section's original proposal: GitHub Actions native builds are
selected, Windows/Linux acceptance is deferred, and only
macOS arm64 is qualified for the initial beta. Intended later qualification
covers GNU Linux x64/arm64 on Ubuntu 24.04 LTS and current supported Windows
11 x64. Windows x64 includes both Intel and AMD processors. Intel Mac is a future
candidate; the user's home Intel Mac is not a pipeline dependency. Windows ARM,
other Linux distributions and source installation remain separately qualified
extensions. The schema-v2 executable verifier now requires four targets with
distinct Mac qualification and Windows/Linux experimental evidence. Schema-v1
retains the old five-target contract only for the frozen baseline. Launcher
metadata and negative tests must remain aligned with schema-v2. This does not
constitute a passing four-target release or invalidate saved evidence.

Build method and runtime qualification are separate fields of the evidence.
Cross-compilation and compilation under emulation are acceptable artifact
production methods when toolchain, target, source and resulting hashes are
recorded. Bare-metal ownership is not required for acceptance: a VM running the
target OS/kernel can exercise real filesystem, account and network controls.
Record guest OS/build, CPU architecture, translation/emulation and sandbox
configuration. An ARM Linux VM is suitable for Linux arm64 proof. Windows ARM
running translated x64 applications proves that configuration; it does not by
itself prove Windows 11 x64. Windows Server builds can produce Windows 11
executables, but client-OS qualification must also run on Windows 11. Retain all
actual install, upgrade, recovery, authority and sandbox acceptance cases.

Before making **either Warden or Alpaca public**, complete one bounded disclosure
review per repository:

- Inventory every intended public branch/tag and its reachable Git history,
  submodules/LFS objects, tracked binaries/archives, and repository metadata and
  release/Actions assets that would become public. Pin reviewed refs and hashes.
- Run an existing deterministic history scanner such as Gitleaks with redacted
  output over all intended refs, plus the owning current-tree secret/public
  boundary gates. Inspect binary/archive contents and configuration/fixtures for
  embedded credentials, personal/customer data and hosted-only implementation.
- Verify license/notice coverage and that hosted OAuth client secrets, relay
  operations and private infrastructure remain outside the public package.
  Apache-2.0 declarations are useful evidence, not a completed disclosure audit.
- Resolve every finding before visibility changes. If a real credential is
  found, revoke/rotate it and remove the exposed material from the proposed
  publication; do not print it in the report. History remediation requires its
  own exact-ref checkpoint before destructive rewriting.
- Exit evidence: reviewed ref/object inventory, scanner versions and commands,
  redacted results, finding resolutions, boundary/license verdict and the final
  audited commit. No unresolved secret or private-content finding may pass.

The review is required planning scope; preliminary scanning has run, but the
full disclosure verdict remains open. Repository visibility has not changed.
GitHub Actions on standard GitHub-hosted runners is the selected first-release
build route. The CodeBuild discussion below is historical analysis, not an
implementation instruction.
Preliminary disclosure scan (2026-09-30, Gitleaks 8.30.0): exact intended
producer heads Warden `dade4e616656fa1a0e891ef06886838e913b532c` (82
reachable commits) and Alpaca `7ee2842c4de9767e2556bbb873f66c65bd1552f8`
(42 reachable commits) were scanned with `gitleaks git --log-opts=HEAD
--redact=100`; their exact tracked `git archive HEAD` trees were separately
scanned with archive depth 2. Alpaca returned zero findings in both scans;
Warden returned one `generic-api-key` match in the test fixture
`warden-gateway/tests/gateway.rs`'s `idempotency_key`, in both history and
current tree. Inspection of the exact historical/current lines shows a
13-character fixture identifier inside `fn invocation()`, not an
authentication secret or external credential; this specific finding is
classified as a false positive. Neither producer has submodule
or LFS entries at these heads. Alpaca has nine tracked historical distribution
tarballs containing native executables. Each extracted executable's printable
strings passed a separate Gitleaks 8.30.0 scan with zero findings. This does
not review all binary content, personal/customer data, licensing or hosted-only
boundaries; those checks remain open before any public visibility change. No
publication approval or public-repository verdict follows from this
preliminary scan. A first worktree-directory scan was contaminated by ignored
`target/` build outputs and is not used as evidence.
The private producers already use standard GitHub-hosted runners; keep the $0
overage cap and verify included minutes before new runs. AppVeyor is another hosted
Windows builder. GitHub larger runners offer a Windows 11 Desktop image for
client-OS qualification and are paid even for public repositories; check account
eligibility and a finite budget before selecting that route. No home machine
needs to become an always-on runner. Source builds on Linux remain feasible,
but a supported source installer must resolve the pinned SDK dependency and
run the same behavioral checks; compilation alone does not qualify support.

Provider references checked 2026-09-30:
[GitHub runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[Windows 11 larger runners](https://docs.github.com/en/actions/reference/runners/larger-runners),
[Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions),
[CodeBuild images](https://docs.aws.amazon.com/codebuild/latest/userguide/ec2-compute-images.html),
[AppVeyor](https://www.appveyor.com/pricing/).

**R4 — Produce the four-target candidate and qualify macOS arm64 (D2/D3).**
Outcome: the exact macOS arm64 candidate has genuine source-free installation,
authority, recovery and sandbox evidence; Windows x64 and GNU Linux x64/arm64
have successful pinned GitHub Actions artifacts and integrity evidence, explicitly
marked experimental and not platform-qualified.
Files/repos: Core distribution verifier, candidate/installer and npm packages;
the existing Core, Warden and Alpaca producer worktrees; GitHub native workflow
definitions and digest-bound run/artifact readbacks. Those workflows supply
build inputs, not by themselves a release acceptance result.
Before dispatch, verify the Actions usage cap and runner availability; pin
producer commits and native runner labels, bound job timeout and concurrency,
and preserve GitHub run IDs, attempts, job IDs, artifact IDs and ZIP digests.
Inspect each artifact's
target, architecture, complete inventory, notices and hashes. Missing, failed,
wrong-source or tampered output fails the experimental receipt; compilation
must never be described as Windows/Linux runtime acceptance. Build and run the
real macOS arm64 acceptance drivers against the new candidate; unavailable or
skipped Mac cases fail qualification. Preserve the D1 authority contract and
the baseline bytes. Run owning producer gates, Core `just verify` and archive
qualification once at the coherent integration checkpoint.
Exit: `cargo xtask distribution-verify --candidate --evidence ABSOLUTE_DIR`
returns 0 for the four-target policy with full Mac evidence, three bound
experimental build receipts and R2 proof. Windows/Linux native acceptance and
Intel Mac remain follow-up work, not implied passes.

**R5 — Publish and verify the npm beta (D4).**
Outcome: the documented one-command install works from the actual registry on
macOS arm64, and a compatible next packaging version demonstrates the Mac
upgrade path. Windows/Linux packages are published with experimental status,
verified bytes, and no claim of tested installation.
Files: existing distribution launcher/package metadata, registry acceptance
driver and this document's install/update/rollback instructions.
Resolve npm authentication through CLI first, Bitwarden and the supported
account MFA flow. Inspect the actual challenge; use the authorized available
factor, and preserve an unavoidable human handoff without retry/recovery loops.
Verify ownership of the launcher and all platform package names, unused version,
package contents, licenses/notices, and exact optional-dependency versions.
Publish only the R4-qualified bytes under `beta`, with supported provenance and
integrity evidence; never claim provenance exists if only checksums exist.
If a trusted CI publication path requires additional configuration, make it
explicit and complete it before publishing, without exposing producer secrets.
Run actual npm and pnpm registry installation with scripts disabled on macOS
arm64, compare all four platform registry tarballs and launcher bytes to the
candidate, then exercise Mac compatible upgrade/rollback. No rebuild between
qualification and publication. A second
packaging version must receive changed-input qualification before publication;
unchanged runtime artifacts may retain valid evidence, but package receipts
must bind the new package version/bytes. If a platform fails, preserve the beta
failure record, remediate and publish a new immutable version; do not overwrite
or report matrix completion from partial success.
Exit: `cargo xtask distribution-verify --evidence ABSOLUTE_DIR` returns 0 from
genuine four-target registry-byte evidence and Mac install/upgrade proof;
Windows/Linux remain explicitly experimental.

**R6 — Integrate and hand off (D5).**
Outcome: scoped changes are committed, merged and pushed under owning repo
rules, with reproducible installation instructions and an auditable candidate.
Keep existing worktrees/PRs, preserve unrelated edits, and complete mandatory
producer review where required. Do not add automatic Core reviewer chains.
Record exact merged source, artifact hashes, native receipts, package versions,
baseline lineage, platform prerequisites and unsigned/not-notarized caveats.
Classify existing M0–M6 evidence by actual input dependencies: unchanged service
deployments retain their valid evidence; changed Core/artifact bindings need
new evidence and any dependent aggregate must be regenerated separately.
Never alter the old lock or imply it qualifies new bytes. Update Product plan
links/receipts only where needed; do not reopen its full commercial M8 lock.
Finish with both matrix commands passing for the published candidate, owning
integration gates, archive/review evidence, source integrated/pushed, and no
uncommitted task edits. Account for unrelated dirty state rather than deleting it.
M7/M8 and public/paid Product readiness verdicts stay false.

#### Resource and execution controls

- Root orchestration/implementation/integration: **GPT-6 Sol Medium**, selected
  by the user before execution. No model change or delegation starts from this
  planning request. Use deterministic commands for builds, test selection,
  hashing, receipts and status. No mandatory helper agents or reviewer chains.
- Before large builds, check disk and bound concurrency. Reclaim only verified
  disposable caches owned by these worktrees using Cargo; preserve frozen
  payloads, candidate assets, evidence and user files. The archive gate clears
  inherited profile options: if needed, give its isolated build explicit fixed
  stripped-debug/two-job settings in `xtask/src/foss_core_boundary.rs`, with
  tests that secret-environment isolation and all-target build remain intact.
  Update `docs/rust-rewrite-harness.md` for changed harness settings. A fresh
  archive still compiles extracted sources and executes runtime smoke. Use an
  available larger native build host if local space cannot support that build.
- Run focused tests while changing code; full required gates at integration.
  Reuse live process handles. Do not rerun unchanged full suites, authentication
  attempts, failed unpaid CI dispatches or immutable-baseline captures.
- Unknown implementation details are resolved within R1, not by indefinitely
  extending the roadmap. No new product features or unrelated cleanup. After
  two equivalent failures of an approach, diagnose/change the approach and
  report the concrete unresolved dependency; do not blindly retry.
- There is no user-imposed token/time budget. Do not invent one. Keep one compact
  checkpoint containing package R1–R6, changed files, decisions, evidence,
  handles and next action. Actual account handoffs/spending decisions can block
  their dependent work; continue only independent work that advances this plan.
- The original release contract conflict is resolved in this proposed plan by
  qualifying a new candidate. The user activated the amended goal; do not
  re-ask the frozen-candidate question as routine permission.

#### Updated goal text — activated 2026-09-29

Historical record only: the saved goal objective below still names five fully
qualified targets. The approved 2026-09-30 first-release amendment and the
R4/R5 exit criteria above supersede that platform count and acceptance level.
The goal API has not rewritten its objective; do not report the old text as a
passing release gate or claim it has been fulfilled.

```text
Complete the amended D1–D5 F2 npm beta distribution plan in
/Users/davidjbeveridge/.codex/worktrees/f2-npm-distribution/docs/distribution.md,
section “Runtime repair and candidate amendment”, using its ordered R1–R6
work packages and deterministic acceptance. Execute with GPT-6 Sol Medium.
Resume existing Core branch codex/f2-npm-distribution at its current verified
HEAD (planning baseline 3594b6a), and existing Warden/Alpaca producer worktrees.
Read the compact checkpoint first; preserve existing work and live processes.

Preserve the original Mac bundle and M0–M6 lock byte-for-byte as immutable
baseline/provenance. Build and separately qualify a new versioned candidate,
including Mac arm64, for the bounded runtime repairs. Implement explicit
authenticated MCP order submission and broker reconciliation using existing
session/lease, strategy/configuration, risk, Warden, idempotency and receipt
machinery. Preserve deterministic evaluation and mode-specific Paper/Live
constraints. Agent submission must not depend on scheduler ticks. No arbitrary
plugin execution or caller-authored authority may bypass the shared boundary.

Prove actual source-free stdio MCP → Core → real Warden → controlled broker
executable, including denials, concurrent duplicates, changed-input keys,
crashes/lost responses and durable reconciliation. Isolated test activations
and test-only authority for a credential-free controlled sink are permitted;
real broker orders, real rig activation and shipping policy weakening are not.
Never substitute direct service mocks or prewritten success receipts.

Finish native qualification for Mac arm64/x64, GNU Linux x64/arm64, and native
Windows x64 MSVC/SRT alpha. Preserve private storage/ACL/reparse/locking and
sandbox guarantees. Complete source-free install, npm/pnpm scripts-disabled
delivery, identity/config/state preservation, stopped upgrade/rollback,
running-rig denial, interrupted recovery and Windows account/elevation/WFP
evidence. Resolve build resources, native CI and npm authentication through
the required CLI → integration → supported API → Chrome route. Use established
spending authorization; do not invent a cap or bypass mandatory human handoffs.

Publish exact-qualified packages only under npm beta after candidate matrix
acceptance and namespace verification. Verify actual registry bytes and npm/
pnpm install/compatible upgrade on all five targets. Complete owning gates,
archive proof, mandatory producer reviews and scoped integration/push. Preserve
the old lock; requalify changed inputs and regenerate affected aggregates in
new artifacts, reusing unchanged evidence only where its bindings remain valid.

Completion requires both cargo xtask distribution-verify --candidate --evidence
ABSOLUTE_DIR and cargo xtask distribution-verify --evidence ABSOLUTE_DIR to exit
0 from genuine digest-bound evidence for all five targets and the exact
published versions; owning integration/archive/review gates pass; scoped
source is integrated/pushed; task edits are committed; baseline hashes remain
unchanged. Narrow passing tests, draft PRs and local-only installs are not
completion. Keep the full scope; checkpoint actual blockers without weakening
acceptance or repeatedly consuming tokens on unchanged failures.

M7/M8 remain excluded and false: no Apple payment/enrollment, Developer ID
signing, notarization, Gatekeeper certification, full/public/commercial Product
release lock, paid Relay launch or SLA claims. No new scheduler, hosted agent
runtime, pricing, UI or unrelated refactor. Use the selected root model and
deterministic tools; no automatic reviewer chains or full-history forks.
```

#### Current amended-goal checkpoint

**R1 in progress.** Core source HEAD at goal start: `7e975eb`; Warden
`d2bd10d`; Alpaca `592bd8f`; all three worktrees were clean. No live build
or worker process was found at startup. Exact source audit confirms:

- `cli/external_mcp.rs` owns attach/detach and renewable lease; the capability
  is verified by `agent_runner.rs` against durable run, deployment, tool
  allowlist, mode and current fencing lease. `service.rs::call_mcp_tool` binds
  that context, and `ports::SideEffectContext` can carry it into plugin dispatch.
- `service/execution.rs::evaluate_tick_response` correctly rejects agent ticks.
  Preserve that invariant. `broker_submission.rs::load_current_state` already
  accepts agent provenance without a deterministic attempt, but its current
  `BrokerCurrentState`, mandate, actor and recovery validation require Live.
- `adapters/plugin_operations.rs` already writes a request/dispatch claim before
  broker effects and checks duplicate hashes; only Live calls the common
  `LocalBrokerSubmissionBoundary`. Paper needs mode-appropriate admission.
  `service/broker_recovery.rs` accepts an agent observer but hardcodes a Live
  recovery `SideEffectContext`. Extend these shared paths rather than create a
  second unaudited submission engine.
- `finance_authority.rs` already defines `order.submit.paper` and
  `paper_order`, but its specialized broker envelope validation accepts only
  `order.submit.live`; `broker_submission/admission.rs` builds only the Live
  envelope. Implement a Paper admission branch using the existing Paper action
  and selected account, while preserving the Live C5 path and its mandate.
  The old status helper advertised generic `plugin.invoke` despite its absence
  from MCP definitions; derive advertised names from the real registry.
- The R1 public tool arguments will carry only an existing activation ID,
  selected broker instance, order fields and idempotency key. The service
  derives plugin/package/strategy/configuration/lease/account/mode bindings
  from durable state. The per-run allowlist must explicitly include each new
  tool; attachment alone does not imply a submission grant. Recovery derives
  the original operation from persisted intent/request bindings.

R1 proof matrix: absent or stale connection and missing tool grant -> MCP
denial/zero dispatch; stale lease/config/controls or changed mode/account ->
broker admission denial/zero dispatch; accepted Paper and isolated Live ->
Warden/appropriate risk verdict plus durable receipt; duplicate and lost
response -> one durable claim, one sink effect, trusted reconciliation.
R2's actual packaged stdio/real-Warden driver will prove the full path.

Current local edit: `runtime-rs/src/mcp.rs` derives plugin status names from
real MCP definitions, removing the advertised nonexistent generic invoke.
The first broad-filter `cargo test` handle `60384` failed at the link step with
ENOSPC while compiling unrelated integration test targets; no assertion ran.
`cargo clean --profile dev` removed only this worktree's 10.0 GiB generated
debug cache; no candidate, frozen bundle or evidence was removed. The narrower
`--lib` test handle `68501` exited 0: one focused test passed. Handle `62930`
exited 0 for `cargo fmt --check` and strict library Clippy. This proves the
status fix, not order submission. No runtime boundary or new-candidate
qualification is claimed yet. Next: implement explicit tool definitions and
service handler with enforced attached-session binding; then extend the shared
mode-specific admission/recovery path before enabling any broker effect.

2026-09-29 R1 continuation: `bff7cba` committed the status fix; `bd05ecd`
committed the initial Core order boundary. Those edits add `tradeassembly.order.submit` and `.reconcile`
definitions in `runtime-rs/src/mcp.rs`, dispatch in `service.rs`, and a narrow
`service/agent_orders.rs`. The submit adapter takes only activation ID,
selected broker instance, canonical market order and idempotency key. It
derives strategy/configuration/revision/account and operation bindings from
durable records, verifies the attached run and lease, acquires a fresh quote
through the pinned market-data plugin (with its own durable receipt), then
calls the existing duplicate-safe broker operation and Warden boundary.
Reconcile calls the existing observation-only recovery boundary.
Both reject bare MCP connections; Paper explicitly fails closed for now. No
generic plugin invocation, scheduler tick, caller-authored permit or owner
identity has been added. The narrow `cargo test --locked -p
tradeassembly-runtime --lib agent_order` ran two cases successfully, covering
discovery/standalone-mock denial and bare-connection denial with zero dispatch
claims. This is not accepted-order or real-Warden proof. Strict Clippy handle
`36930` exited 0 for the initial narrow route. Subsequent shared-boundary
edits generalize account observation, mode-specific broker admission and
stored Warden validation to Paper while preserving the Live mandate branch.
Recovery now derives its mode and original operation from durable intent, not
caller parameters. `cargo test --locked -p tradeassembly-runtime --lib
broker_submission` passed 16 cases (27 real-process cases ignored), and the
new Paper Warden-envelope unit test passed. The changed code has not yet
passed real-Warden/controlled-sink proof. Disk free was 3.9 GiB after focused
builds; do not start broad link-heavy suites blindly.

Current R1 follow-up: Core now selects the mode-specific Paper submit
capability and operation, retains the legacy Live lookup ID, and requires a
distinct Paper lookup ID. Core narrow broker-submission tests passed 16/16
(27 real-process ignored), agent-order tests 2/2, strict library Clippy passed.
The Alpaca producer worktree has an explicit read-only Paper and Live lookup
contract and complete host-bound receipt identity; mock-provider tests passed
3/3, including found and absent observations. A verified 404 is classified
as `absent` in durable Core recovery outcomes, while unknown failures remain
`unresolved`; neither outcome permits blind original resubmission. This is
not yet an integrated package or R1 acceptance. Local producer verification
uses a noncommitted Cargo patch to Core's unpublished plugin SDK; the
registry-exact dependency is unchanged. The Alpaca producer follow-up is
committed on its isolated branch as `e1d8385`; no publication is claimed.

Next R1 work: execute attached-agent real-Warden controlled-sink tests in both
modes, then run deterministic parity and the specified R1 acceptance matrix.

Follow-up checkpoint: Core commits `bd4fa1a` and `36a0220` now include the
Paper/Live lookup split, Paper agent submit selection, durable `absent` versus
`unresolved` recovery observations, and a controlled broker fixture that
declares the Paper submit/lookup operations. Alpaca producer commit `e1d8385`
adds found/absent lookup and exact receipt identity. Focused Core checks:
`cargo test --offline --lib broker_submission::` 16 passed, 27 ignored;
`cargo test --offline --lib agent_order` 2 passed; library strict Clippy and
`cargo check --offline --example f2_controlled_broker` passed. Alpaca
`cargo test --offline --workspace` with a local uncommitted SDK source
override passed. No real-Warden attached-agent MCP path has passed yet. For
reattachment, preserve the existing quarantine and owner-acknowledged
`agent_run.recover` transition rather than automatically marking a disconnected
run reconciled. The R2 matrix must show original-order observation before
acknowledgement and a fresh attached session after acknowledgement; absent or
unknown observation must not authorize resubmission.
Until those pass, R1 and the overall release gate remain open. No R2–R6
acceptance is claimed.

2026-09-29 real-process boundary checkpoint: four ignored Core library tests
were explicitly executed (not merely compiled) with real Warden, SRT, Node,
and an isolated controlled broker: Paper submit, Paper lost-response recovery,
Live submit, and Live lost-response recovery all passed (`cargo test --offline
--lib attached_agent_mcp_ -- --ignored --nocapture`: 4 passed). Each accepted
case asserted exactly one durable broker sink row/submission, duplicate-safe
replay, and no scheduler ticks. The service-level test uses a verified attached
agent context, not the packaged stdio interface, so this is **not R2 proof**.
The Paper path exposed and fixed a missing quote requirement for external-agent
Paper activations and missing Paper action registration on the C5 broker PEP;
the fixture-only Warden policy continues to isolate its allow decision. The
shipping Warden Paper rule remains unchanged. `cargo test --offline --lib
agent_order` passed 2/2 after the change. R1 remains open for its denial
matrix, deterministic parity, and release-boundary gates; R2–R6 remain open.
Exact next action: run policy/admission regressions, add the R1 zero-sink
negative matrix, then build the source-free stdio driver without treating these
service tests as a substitute.

R1 negative-matrix continuation: `cargo test --offline --lib
never_reaches_sink -- --ignored` passed 6/6, and `prevents_real_dispatch
-- --ignored` passed 10/10 with the same real-process environment. The
attached-agent MCP service tests now also assert zero sink submissions for an
unattached caller, wrong activation, wrong plugin binding, and excess quantity
before the accepted order. A same-key/different-quantity attempt is explicitly
reported as `agent_order_idempotency_conflict` and leaves the sink at one
submission; this required mapping immutable storage's conflict result at the
plugin request boundary. The expanded Paper/Live MCP group passed 4/4;
`cargo test --offline --lib broker_submission::` passed 16 nonignored cases
(31 explicitly ignored). Revoked MCP grant, source-free stdio, and
deterministic-path parity remain unclaimed. R1 is not yet closed.

The missing-order-tool-grant case also executed with real Warden and a
controlled broker: `attached_agent_missing_order_grant_never_reaches_sink`
passed 1/1. The existing verified MCP context binder returns
`agent_mcp_tool_not_allowed` before any dispatch. R2 can reuse the real-stdio
attach/quarantine pattern in `runtime-rs/tests/external_mcp_process.rs`, but
that fixture is inspection-only. The packaged binary uses the production SRT
sandbox and shipping Warden policy, unlike the injected service fixture; the
controlled broker's local SQLite sink may be blocked by that sandbox. Do not
count service-level proof as source-free stdio proof. Probe the actual
candidate binary with an isolated Paper sink first, then settle a fixture
transport that preserves production sandbox rules before extending to Live.

2026-09-29 R2 stdio checkpoint: the new opt-in
`runtime-rs/tests/agent_order_mcp.rs` runs the actual candidate Core binary
over MCP stdio with real Warden and production SRT. The controlled broker
fixture declares a narrow loopback HTTP destination to an isolated SQLite sink
outside the sandbox; production sandbox rules were not disabled. With explicit
`F2_TEST_RUNTIME_BINARY`, `F2_TEST_WARDEN_BINARY`,
`F2_TEST_CONTROLLED_BROKER_BINARY`, and `F2_TEST_SRT_CLI`, `cargo test --offline
--test agent_order_mcp -- --ignored --nocapture` executed 1/1 passing. It
proves Paper attach, accepted order, sequential duplicate, conflicting-key
rejection, process restart fail-closed as `agent_run_pending_reconcile`, owner
recovery via MCP, fresh attach, same-order replay, one sink row/submission, and
zero scheduler ticks. `cargo clippy --offline --test agent_order_mcp -- -D
warnings` passed. This is candidate-binary evidence, not a frozen packaged
artifact qualification. R2 remains open for concurrent duplicate, dropped
broker response, found/absent/unknown reconciliation, stale/replaced session,
revocation/risk denials, and isolated Live stdio proof. R1 and R3–R6 remain
open. Next: extend the single stdio fixture with the remaining finite matrix;
do not mark a disconnected run reconciled without order observation.

R2 fault-path continuation: the same real-stdio test now has a controlled
broker response-body failure *after* the isolated sink commits. The first
submit and identical retry both report reconciliation required; the public
`tradeassembly.order.reconcile` tool observes the original order through the
broker lookup, persists its receipt, and the sink records one submission for
that order. A quantity above configured risk limits is denied before any sink
file exists; pausing the deployment revokes further order authority and adds
no sink row. The test asserts both durable intent keys and operation receipts,
two accepted orders with exactly two total sink effects, and zero scheduler
ticks. Explicit `cargo test --offline --test agent_order_mcp -- --ignored
--nocapture` passed 1/1 after these assertions; targeted strict Clippy passed.
The test was renamed after the run to describe its broader behavior, with no
semantic change. R2 still needs separate crash-during-uncertainty,
found/absent/unknown, concurrent duplicate, stale-session and Live cases;
the current owner recovery followed a known successful receipt, not an
unobserved ambiguous outcome.

R2 crash continuation: a third controlled order commits at the sink and loses
its response, then the candidate MCP process is killed without graceful
detach. A restarted candidate refuses immediate attachment, the authenticated
owner observes the order through `tradeassembly.order.observe`, and a recovery
attempt before the durable 90-second lease expires returns
`agent_recovery_lease_held`. After expiry, a fresh owner recovery attempt
acknowledges the quarantined run, a new session attaches, and replay of the
same order leaves exactly one sink submission. The complete opt-in stdio test
executed 1/1 passing in 116.19 seconds with real Warden and production SRT;
targeted strict Clippy passed. The first implementation of the test reused a
Warden authorization idempotency key after a retryable lease-held response and
hit `warden_request_failed:409:idempotency_conflict`. The test now uses a new
key for the post-expiry *recovery attempt*; order submission/recovery retain
their original stable order key. This interaction deserves a separate explicit
retry contract and regression before R2 closes. The test was renamed after
the passing run only to reflect crash coverage.

Crash-recovery contract decision: an MCP connection that dies cannot use its
agent-scoped `tradeassembly.order.reconcile`, and `studio.agent_run.recover`
must not acknowledge reconciliation before the order outcome is observed.
Expose one owner-scoped MCP observation tool backed by the existing
`POST /orders/broker-recovery` path and its authenticated `RecoveryObserver::Owner`.
It accepts only the original and distinct recovery keys, never submits or
replays, and has no independent authority grant. It is unavailable from an
attached agent's restricted allowlist. The crash test must call this through
the restarted candidate binary, observe found/absent/unknown, then acknowledge
only a genuinely reconciled run. Source pointers: `runtime-rs/src/service/
broker_recovery.rs`, `runtime-rs/src/agent_runner/external_session.rs`,
`runtime-rs/src/service/agent_deployment.rs`, and `runtime-rs/src/mcp.rs`.
The candidate implementation of `tradeassembly.order.observe` now reuses
that owner HTTP boundary; the real-stdio test confirms a bare authenticated
owner connection can observe the existing order before run acknowledgement,
while the attached agent's allowlist denies this owner tool. Candidate binary
rebuild, the explicit R2 test (1/1), and targeted strict Clippy passed. This
does not yet prove the crash/unknown variants, and no final release gate ran.

R2 explicit-absence continuation: the controlled broker now reports an
explicit 404 lookup as a failed, bounded `provider_order_absent` response.
The host retains envelope/output-policy, declared-schema and installed-schema
digest checks, but does not require a failed response to contain a successful
order payload. A focused schema regression passed, including digest tampering.
The actual candidate stdio → real Warden → production SRT → controlled sink
test passed 1/1 in 130.38 seconds: an order rejected before sink commit left
no sink row, lookup persisted `plugin_broker_recovery_outcomes.state=absent`,
same-key replay stayed fail-closed, and a subsequent distinct order was denied
before sink dispatch. Strict targeted Clippy passed. This does not establish
unavailable/unknown lookup behavior: that case needs a fresh isolated rig,
because the absent-order rig denied the next distinct submit. Nor does this
qualify a packaged artifact or close the remaining R1/R2 matrix or R3–R6.
Next: isolate the unknown-lookup case, then concurrent duplicate and
stale/replaced-session proof; settle isolated Live source-free policy injection
without enabling the shipping Live policy.

R2 unknown-lookup continuation: a second opt-in stdio test uses a fresh
private database, real Warden, production SRT and the same controlled sink.
The sink commits one order and loses its response; the subsequent lookup
returns unavailable, not absent. The public MCP reconcile call fails closed,
the durable outcome is `unresolved`, original-key replay remains blocked, no
successful order receipt appears, and sink counts stay exactly (1 row, 1
submission). The targeted command filtered to
`installed_stdio_unknown_lookup` passed 1/1 in 22.46 seconds after correcting
its isolated fixture to use the existing test strategy ID. Targeted strict
Clippy passed before that fixture-ID correction. This is still a candidate
binary test, not packaged-artifact or five-target qualification. Next: run the
combined R2 tests at an integration checkpoint, prove concurrent duplicate and
stale/replaced session paths, then settle isolated Live proof without changing
shipping Live policy.

Combined R2 Paper checkpoint: with explicit candidate Core, real Warden,
controlled broker and SRT paths, the full opt-in `agent_order_mcp` binary ran
2/2 passing in 129.32 seconds. Both tests use separate disposable state and
controlled loopback sinks; neither invokes a real broker. The candidate still
uses the embedded shipping Live rules, which deny
`execution.activate.live` and `order.submit.live`. A positive source-free
Live case therefore needs an isolated test-only authority route (for example,
a private Warden policy-rewriting proxy with real Warden-signed decisions),
not a shipping runtime flag or relaxation of the packaged policy. This design
is not yet implemented or accepted as Live proof.

R2 isolated Live continuation: the candidate binary stays unchanged and keeps
its embedded deny rules. An opt-in test-only loopback adapter substitutes
`allow` only for `execution.activate.live` and `order.submit.live` while
installing the policy into a disposable real Warden. All authorization and
receipt calls still go to that Warden; the test asserts a signed Ed25519
receipt and C5 decision for the Live order. A separate synthetic signed legal
receipt is bound to the disposable local owner, exact saved strategy version,
spec hash and local-live environment, and is verified by the ordinary
file-backed verifier before activation. No Hub key, real user acknowledgement,
broker credential, production account, or real broker endpoint is used. The
new source-free stdio Live test passed 1/1 in 29.04 seconds: attached MCP
submit and sequential duplicate returned the same result, the sink recorded
one row/one submission, and no scheduler tick existed. This is candidate
binary evidence, not final packaged-artifact or five-target qualification.
The Live test now also proves that activation without a mandate, a competing
unattached client, and an over-limit order cannot reach the sink. Targeted
strict Clippy and the expanded Live test passed. The combined opt-in stdio
suite then passed 3/3 in 129.91 seconds with distinct disposable state for
Paper crash/absence recovery, Paper unknown lookup, and Live C5 submit. The
remaining R2 concurrent duplicate and full stale/replaced-session matrix
still need proof; shipping-policy denial must remain part of the final
negative matrix. This run does not close R1, qualify a packaged artifact, or
complete R3–R6.

Concurrency boundary check: the existing real-Warden controlled-broker unit
case `named_agent_real_plugin_quote_and_c5_order_are_duplicate_safe` passed
1/1 with simultaneous calls into the plugin-operation adapter and one
controlled sink effect. This is useful lower-layer evidence, not a claim that
the synchronous stdio MCP transport dispatched concurrent calls. The installed
binary suite separately proves a second client cannot attach or submit while
the first deployment session is active. A subsequent Live source-free stdio
run passed 1/1 after the test replaced the active deployment lease: two calls
from the old attached process failed, the replacement lease remained current,
and the sink stayed at one row/one submission. This proves lease replacement
denial for the already-attached client. The Live test then added an explicit
MCP detach: the detached client could not submit, owner recovery using the
required authority context succeeded, and the same connection re-attached
before lease replacement. That targeted test passed 1/1 in 39.93 seconds.
The test also sends two identical JSON-RPC order requests before reading
either response. Both returned the same accepted result and the sink still
recorded one effect; that source-free Live test passed 1/1 in 40.84 seconds.
This is concurrent in-flight MCP input, not simultaneous handler execution:
the stdio server serializes its dispatch, and a second process cannot attach
to the active deployment. The lower adapter's simultaneous-call test covers
the underlying race. These source-free tests use an unpackaged candidate
binary.

Shipping Live negative checkpoint: the same disposable setup now runs without
the test-only policy adapter. A valid local mandate permits activation and
agent session attachment, but the embedded shipping policy denies
`order.submit.live` at the real Warden C5 broker boundary. The source-free
stdio order returns `agent_order_submission_denied`, a durable Warden deny
decision exists, and the controlled sink is untouched. The targeted test
passed 1/1 in 27.60 seconds. Activation itself is not a shipping-policy deny
gate; an earlier test assumption to that effect failed and was corrected.
The combined opt-in `agent_order_mcp` regression passed 4/4 in 130.88
seconds with separate disposable state. This is an unpackaged candidate
binary, not packaged-artifact or five-target evidence, so R2 is not closed.

R1 command checkpoint: the exact `cargo test --offline --locked -p
tradeassembly-runtime agent_order` invocation failed at link time with
`No space left on device`, before it could execute its intended test matrix.
Four identified, regenerable non-candidate Rust test executables were
removed from this worktree's `target/debug/deps`, recovering about 500 MB.
The narrower `cargo test --offline --locked -p tradeassembly-runtime --lib
agent_order` then passed 2/2. This does not substitute for the plan's exact
R1 command or prove its complete negative matrix. Remaining R1 proof must
bind each named denial to zero controlled-sink submissions and preserve the
deterministic path; rerun the exact command after sufficient build space is
available.

R1 continuation: after two disk-limited link attempts, the exact named command
passed with `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0` (2 `agent_order` tests
executed, other targets filtered). Space was recovered by deleting only Cargo
incremental and `deps` cache files from the inactive
`rename-tradeassembly-studio-f2` worktree; its binaries, source and the active
F2 extraction rig were preserved. These caches can be regenerated. The
following targeted R1 proof matrix also passed:

| Boundary | Executed evidence | Result |
| --- | --- | --- |
| No attached/unauthenticated agent; wrong owner and mode binding | named `agent_order` 2/2; `mcp_parity::external_agent_attachment_requires_authenticated_persistent_transport` 1/1; isolated Live fixture rejects other owner and Paper deployment against Live activation before sink | Pass |
| Current run and lease | `external_session::tests` 3/3; isolated Live fixture rejects released and replaced lease with sink unchanged; `event_replay_checkpoint_inbox_and_lease_survive_restart` 1/1 acquires a replacement only after the recorded expiry and rejects stale release | Pass; released lease models lost ownership, while the local port test proves elapsed expiry and fencing |
| Revocation, configuration/package/credential drift, mandate and risk | real-Warden `prevents_real_dispatch` 10/10 and `never_reaches_sink` 7/7, including missing grant, paused/stopped agent, wrong account, excess quantity/notional, revoked/expired mandate, stale quote, changed config, replaced lease and Warden denial | Pass; zero controlled sink effects |
| Paper/Live order outcome, signed decision, and duplicate/recovery | opt-in source-free MCP suite 4/4 in separate rigs; `owner_recovers_lost_response_after_stop_and_revocation_without_resubmit` 1/1 against real Warden and controlled broker | Pass for unpackaged candidate only |
| Deterministic runtime retained | `execution_parity` persisted scheduler/worker and activation/tick/control/APF cases 2/2 | Pass |

The two Live fixture variants passed individually again after wrong-owner,
mode-mismatch and released-lease assertions were added. R1 source behavior is
substantially evidenced, but no packaged candidate or five-target result is
inferred from this matrix. R2's final installed-artifact rerun and R3–R6
remain open.

R3 prerequisite: native freezing now accepts a separately staged Mac arm64
input with an arm64 Mach-O header, while retaining the host-target and
explicit input-hash checks. The targeted binary-header test passed 1/1. The
baseline Mac manifest hash remains enforced for schema-1 releases.

R3 descriptor implementation is now in progress: schema-2 releases require a
SHA-256-bound `candidate-descriptor.json` naming the exact version, target,
immutable parent lock and baseline Mac manifest, replacement bundle manifest,
three source revisions and native binary/plugin input digests. The new
`cargo xtask distribution-describe-candidate --bundle FROZEN --parent LOCK
--version VERSION --out NEW_DIRECTORY/candidate-descriptor.json` command
derives it from a native-frozen bundle, checks the actual bytes, and will not
overwrite an existing output. `distribution-pack` takes that exact file with
`--candidate-descriptor`; the installer and delivery verifier check its hash
and content. The old schema-1 Mac bundle is still accepted only at its frozen
manifest hash. Targeted descriptor tamper and baseline-receipt reuse tests,
the distribution crate suite (17/17), strict distribution Clippy and xtask
check passed. Registry CLI returned 404 for five names; a
bounded curl retry confirmed 404 for the sixth Windows package after the npm
request timed out. `0.1.0-beta.2` is provisionally unused, subject to a fresh
pre-publish ownership/version check.

2026-09-29 Mac arm64 replacement checkpoint: the
`e72cf9519e9a4498b08b201adfea64b450ffcf27` Core source was built as a
native release binary and staged beside the unchanged frozen Warden
(`84738d401b7442c743de7fe736d61199a82fe1f89c405a0e0135cc1bd915acce`),
Alpaca plugin, Node and SRT. `distribution-describe-native-inputs` generated
metadata SHA `a8a0acbf5b5f0f7731d0e37555cf2c1985bf25a6d1082c34127404b72c9cd79a`;
`distribution-freeze-native` generated a separate Mac arm64 bundle;
`distribution-describe-candidate` generated descriptor SHA
`3a737ac17bbd0938bee0a76d9995a46136502c3a69eb6316cbbee5702a8f1ee5`;
`distribution-pack` produced schema-2 `0.1.0-beta.2` with manifest SHA
`1ece8ec55b12b8739fa4afd858ced688ef649beb6cea9585b24f9ea1d3ce679f`
and archive SHA `ad4edfdb3fe2d60e802ee23e60774e04bef98beb48becf70790deea7073b9ec1`.
This version remains provisional until a fresh registry ownership/version check.
All outputs live only under this worktree's ignored `target/`; no source-free
receipt or public registry package has been emitted.

The actual candidate's ignored `packaged_install` and `package_managers`
tests each passed 1/1. A separately repacked **unmodified frozen baseline**
(`0.1.0-beta.1`, archive SHA
`7c0a4d6b55555a4a0e0a39bd9b65621879c0240e8b6ca663411973fb2dbdd8a4`)
passed the new explicit baseline → replacement → rollback test 1/1, retaining
database row, owner configuration, identity/credential file hashes, the stable
command and the original Warden digest. The installed replacement's immutable
Core binary, packaged Warden/Node/SRT, and controlled broker passed the four
ignored real-stdio MCP tests 4/4: Paper crash and ambiguous recovery, unknown
lookup fail-closed, isolated Live C5 submission, and shipping Live denial.
The Paper fixture now explicitly rejects a **distinct** post-absence order;
absence itself fences the original key only, and the broker's rejected response
is correctly classified as reconciliation-required, not a terminal admission
denial. No actual broker order was placed. Distribution unit tests 17/17 and
strict distribution Clippy passed. The immutable parent lock remains SHA
`222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`;
the frozen baseline manifest remains SHA
`6740c7d7f4e8a692ff005dd18b6dc2656883eb8e366253e4e8e6aa054108f5dc`.
Mac arm64 R2/R3 behavior is evidenced, but R4's native capture/receipt and
five-target matrix, R5 registry publication, and R6 integration are open.
The `packaged_sandbox` test also passed 1/1 against this exact frozen
replacement bundle: bundled Node and sandbox denied filesystem and loopback
access while the outside controls succeeded, and the pinned Alpaca binary
completed local capability discovery. Its manifest assertion now verifies the
pin and plugin package through the candidate's inventory digests rather than
an obsolete embedded `alpaca` object.
R4 capture continuation: `cargo xtask distribution-qualify --candidate DIR
--baseline-package DIR --controlled-broker BINARY --source CORE_CHECKOUT
--out NEW_EVIDENCE_DIR` now runs the real package, baseline upgrade, npm/pnpm,
MCP and sandbox acceptance drivers and retains their outputs. The first Mac
arm64 execution completed with all drivers passing and produced a provisional
receipt under `target/native-qualify-mac-arm64-beta2`, but it ran while the
capture source was uncommitted. The command was subsequently tightened to
require a clean source commit, bind the harness/source revisions and exact
controlled-broker binary, and reverify the npm archives against the descriptor.
That provisional receipt is **not** the release receipt; rerun into a new
directory after committing the capture implementation. Windows currently fails
closed before receipt generation until its native ACL/account/elevation/WFP
drivers exist. The other four native hosts, five-target matrix, registry
publication and R6 integration remain open. Do not fabricate a matrix receipt
from this note.
The clean-source negative probe returned `qualification_source_not_clean`
before creating an evidence directory. This preserves the captured test
harness revision as an actual committed input, rather than treating an
uncommitted qualification script as release evidence.

Clean Mac arm64 native capture passed from committed qualification source
`444a7959784348cf2afb2051ddd7e1138bc574e3`. The sealed receipt at
`target/native-qualify-mac-arm64-beta2-committed/receipt.json` has SHA-256
`223e82b65ac82093bdc464b091f2c8bb3f2c18ac50f7e67c1392592816071f42`;
it binds the candidate descriptor SHA
`3a737ac17bbd0938bee0a76d9995a46136502c3a69eb6316cbbee5702a8f1ee5`,
controlled broker SHA
`9e53ac690f841ccf4fe8c9e7288ada3afe8174af417226829ccf132ded373508`,
exact installer and npm tarballs, source revisions, and passing real-process
logs. A partial five-target matrix probe accepted this Mac receipt and failed
at the next target with the exact expected code
`native_qualification_missing:x86_64-apple-darwin`. That is **not** a passing
`distribution-verify --candidate` gate. The original parent lock and frozen
baseline manifest still hash to their recorded values. Next: publish the
locally gated Core branch, then stage native producer/Core artifacts for Mac x64,
Linux x64/arm64 and Windows x64. Windows still needs its specific security
drivers before capture can emit a receipt.

Core gate checkpoint, 2026-09-29: committed source `8ac76d9` passed
`just verify` (which runs `cargo xtask verify`), including workspace tests,
1,319 nextest tests, dependency/audit checks, the clean-tree language
whitelist, plugin contract, architecture, public scan, and FOSS boundary.
The first parallel nextest attempt had one intermittent failure in the
virtual-time soak; it passed in isolation and on the next two full nextest
runs. The MCP parity fixture now includes the three registered order tools,
and the controlled-Warden test helper asserts readiness and health. This
local gate does not establish the other four native receipts, five-target
matrix, registry publication, or Relay integration.

Native builder checkpoint: Core run `36662130016` started on Windows and
failed strict Clippy on Windows-only imports, return expressions and an unused
Unix-only test sandbox. The subsequent Core repair also routes local credential
files through the private Windows ACL/atomic-file adapter instead of treating
Windows permissions as a no-op; this must be retested on the actual Windows
runner. Warden run `36663034275` and Alpaca run `36663412517` did not start:
GitHub reported account-payment or spending-limit admission rejection. Neither
is native qualification evidence. Do not raise the recorded $0 Actions cap
without an explicit finite budget decision; use another approved native host
if GitHub remains unavailable. The CLI confirms Core is public and its native
Windows job did start, while Warden and Alpaca are private repositories and
their jobs received no runner steps. The organization Actions budget remains
$0 with `prevent_further_usage=true` and alerts enabled. A change to repo
visibility is a separate publication/security decision, not a CI workaround.
Core rerun `36665473745` reached native compilation but caught a Windows-only
result-expression semicolon and unused import. Commit `274260e` corrected those
two lines; native run `36665941341` then passed that compile stage but found
an unguarded Unix symlink test in `agent_launchd`. The corresponding
`warden_launchd` symlink tests are likewise Unix-only and now carry explicit
gates. The previous full Core gate passed on `1f58f73`; targeted format,
distribution unit, launchd test and strict Clippy checks passed for the
follow-ups. Run the full gate again before any integration or release claim.
The pinned SRT Windows alpha requires a one-time elevated `windows-install`
that provisions its sandbox account and machine WFP filters. A native test must
observe that setup plus restricted account, elevation, ACL and WFP behavior;
mere compilation or a mock receipt cannot open the Windows qualification gate.
Native run `36666676094` compiled the branch and reached distribution unit
tests. Several tests failed from one Windows private-file primitive:
`OpenOptions::create_new` was rejected because the Rust API requires an
explicit write flag even when custom Windows access bits grant write access.
The primitive now requests `.write(true)` before its owner-ACL protection and
validation; this still needs an actual Windows retest. No native release
receipt was emitted.
Native run `36667206056` passed the distribution unit and Windows-private
adapter tests, then failed six Core local-install tests at the protected state
root (`local_install_state_insecure`). This may be a canonicalized Windows
temporary-path or ACL traversal defect; the new narrow native test compares
raw and canonicalized private temp roots and reports the underlying error
before changing security behavior. The full Core `just verify` gate passed on
clean commit `61dd2f4`. No Windows receipt exists yet.
Native Core run `36668630926` on `0e97c7b` isolated the defect further:
raw private temp-root creation passed, while the canonicalized `\\?\` Windows
path failed with OS code 1 (`Incorrect function`) before the local-install
tests ran. The follow-up native diagnostic separates local path parsing,
canonical-parent metadata, parent-handle traversal, and directory creation.
Do not loosen owner/DACL or reparse-point validation to make this pass.
Run `36669098721` confirms path parsing and canonical-parent metadata passed;
the failing call is `private_parent` opening a verbatim Windows volume-root
handle (`Incorrect function`). The candidate repair opens only that volume
root through its equivalent ordinary drive spelling, retaining validated
verbatim descendant paths and all private ACL/reparse checks. Core native
Windows run `36669540407` on clean commit `643a933` passed the complete
`native-core.yml` builder job in 15m44s: strict Windows Clippy, distribution
and private-file tests, local-install/owner tests, release build, and artifact
upload. The same commit passed local `just verify` (including `cargo xtask
verify` and 1,320 nextest cases). This is a builder checkpoint, not a Windows
qualification receipt: native SRT account/elevation/WFP, controlled
install/upgrade evidence and private Warden/Alpaca binaries remain due.
Parallel Core builder runs also passed for Mac x64 (`36670747388`), Linux x64
(`36670759083`), and Linux arm64 (`36670768977`), all at `643a933`.
The exact Core builder artifacts were downloaded into
`target/native-core-builders/{windows-x64,macos-x64,linux-x64,linux-arm64}`;
all four contain their native release executables and six proof logs. `file`
identifies the main binaries as PE32+ x86-64, Mach-O x86_64, ELF x86-64, and
ELF aarch64 respectively. Their main `tradeassembly` SHA-256 digests, in that
order, are `aa0519a486dfcbbc4a18ffdb66fe95fa7d08d6b002b254e6aab35a5c9642505c`,
`ac2217ba5e8e528e155015e384dd1d0221102fe348b6687ca2dc8275cf0574cb`,
`499d6543ce1c26ff741bb9a701d005b8d00b385724b8c337bce61caff298b63a`,
and `71f27b12b59c1570f3595819c62e95000c30ccc50c80c104fbf542ccc4212881`.
These are Core-only builder outputs, not complete candidate bundles or native
qualification receipts. The other producers and platform-specific SRT proofs
remain mandatory.
Windows SRT setup repair: pushed commit `fe2701a` probes the bundled native
helper before activation, runs a bounded one-time `windows-install` only for
absent account state, rechecks readiness, and fails before the installed
pointer if UAC/setup fails. It does not rotate an already provisioned shared
machine account on upgrade. Core Windows native builder run `36674209758`
passed strict Clippy, owner/private-file tests, distribution tests and the
release build on that exact revision. This is still **builder evidence only**.
That committed batch adds the production Windows environment handoff to the
SRT launcher, dedicated-account identity and real installed-secret denial
probes, Windows owner-ACL validation, a fresh-provisioning signal, and
digest-bound Windows qualifier checks. The qualified candidate still requires
real Windows account/elevation/WFP/ACL behavior and all five complete native
receipts; no such receipt has been emitted. Core Windows builder run
`36676382267` passed on clean commit `6eceeea`, including strict Windows
Clippy and native Core tests. A subsequent local change makes installation
exercise SRT's behavioral WFP readiness check before the installed pointer
is written and binds that outcome into the qualifier. Local distribution
tests and strict Clippy passed, and clean commit `fe70cef` passed `just verify`
(including 1,322 nextest cases). Native Core Windows builder run
`36678558698` passed its strict Clippy, 25 distribution tests, private-file,
owner, setup and release-build checks. Its exact PE x64 Core artifact is
retained at `target/native-core-builders/windows-x64-fe70cef/` with main
`tradeassembly.exe` SHA-256
`aede410da6ca3eecab7255348afd32caf735fa6ee92458a7af271857340cab3d`.
This is still **builder evidence, not fresh-Windows qualification**. The
separate fresh-host SRT component probe on Windows runner
[`36682896292`](https://github.com/TradeAssembly/Core/actions/runs/36682896292)
installed the sandbox account and four WFP filters, then failed behavioral
startup with `listen EACCES 127.0.0.1:60080`. Its downloaded setup/status/
behavior logs are retained at `target/native-srt-probe-36682896292/`.
The probe is not a TradeAssembly installer receipt. A follow-up probe tests a
pre-bindable nondefault port range and binds it to both SRT installation and
runtime configuration. If that passes, Core installation must select/persist
the same safe range before activation; do not simply change the probe and
declare Windows fixed. The follow-up run
[`36683453953`](https://github.com/TradeAssembly/Core/actions/runs/36683453953)
confirmed `40080–40089` was bindable and WFP installed for that exact range,
but the quoted Node child failed with `The filename, directory name, or volume
label syntax is incorrect`. Its logs are retained at
`target/native-srt-probe-36683453953/`. The next component run
[`36684073949`](https://github.com/TradeAssembly/Core/actions/runs/36684073949)
passed a real sandboxed `cmd.exe` child with the same WFP configuration, then
failed only for the quoted absolute Node path. That localizes the remaining
behavioral issue to the SRT CLI's argv-to-Windows-shell boundary, not WFP
startup. Follow-up run
[`36684724250`](https://github.com/TradeAssembly/Core/actions/runs/36684724250)
passed `node.exe` via PATH, but an unquoted absolute path under `C:/Program
Files` was split and the default argv path still failed. Logs are retained at
`target/native-srt-probe-36684724250/`. Explicit quoted-path run
[`36685026656`](https://github.com/TradeAssembly/Core/actions/runs/36685026656)
reported `path=0 forward=0 argv=1`: the pinned SRT backend and a Windows-quoted
absolute Node path work, but its default POSIX-quoted argv path does not.
The current unqualified Core repair uses the tested raw-command interface from
the Rust launcher with fail-closed argument validation. Its Windows installer
chooses a bindable proxy range, verifies SRT uses that exact range, and records
it in local runtime configuration; a native mini-bundle probe and complete
source-free candidate qualification remain required. The partial candidate
repair at `4e05566` passed `just verify` locally and the native Windows Core
builder [`36689234686`](https://github.com/TradeAssembly/Core/actions/runs/36689234686).
The component probe initially encountered SRT's 30-second WFP verification
timeout twice after account/filter installation; a bounded same-host retry is
retained, with either failure logged. Probe
[`36690353541`](https://github.com/TradeAssembly/Core/actions/runs/36690353541)
then built the Rust launcher natively but its bundled Node child failed with
`EISDIR ... lstat 'D:'`. Diagnostic probe
[`36692125087`](https://github.com/TradeAssembly/Core/actions/runs/36692125087)
ran that same bundled Node through SRT directly from both workspace and bundle
working directories (`workspace=0`, `bundle=0`), isolating the failure to the
Rust launcher path. Commit `7dad7c6` removes Windows verbatim-drive prefixes
from bundled Node/CLI argv paths only after canonical containment checks;
its 7 launcher unit tests, strict targeted Clippy, and full `just verify` pass
locally. Native Windows component probe
[`36693766675`](https://github.com/TradeAssembly/Core/actions/runs/36693766675)
passed on commit `046cc1b`: SRT installation/account/WFP, direct and Rust
launcher bundled-Node execution, exact argument-with-metacharacters echo, and
sandbox child SID matching the installed account. Logs are retained at
`target/native-srt-probe-36693766675/`. This closes the launcher-specific
failure, but it is **not** a source-free installer/upgrade/rollback/security
qualification receipt. The exact-source native Core builder
[`36693766617`](https://github.com/TradeAssembly/Core/actions/runs/36693766617)
also passed on `046cc1b`; its retained Windows x64 PE artifacts are under
`target/native-core-builders/windows-x64-046cc1b/`. The main
`tradeassembly.exe` SHA-256 is
`16bf542dab764db6f76fe3be91d9018a05dcdc47391e8289e0ec62931452b533`,
and `tradeassembly-sandbox.exe` SHA-256 is
`179436520ff29952c1f66d932e7894499ee5c65d4f742ffd86f18199e4d9edaa`.
These are builder bytes, not a complete private-producer candidate. The partial candidate
matrix still accepts the sealed
Mac arm64 receipt and
fails precisely at `native_qualification_missing:x86_64-apple-darwin`.
Private Warden/Alpaca native jobs remain
runner-admission blocked by the organization's $0 Actions budget. The
existing `zoidberg` host was probed without changing trust settings:
its known host key matches the saved fully qualified host, but SSH
authentication is denied and its operating system/architecture were not
verified, so it cannot presently replace those jobs.
An unauthenticated npm CLI probe on 2026-09-30 received 404/unavailable for
`0.1.0-beta.2` on all six package names, but `npm whoami` has no session;
ownership and version availability must be rechecked after authentication
immediately before any publish.

On 2026-09-30 a bounded npm CLI sign-in using the existing vault credential
and an isolated temporary npm config reached npm's one-time-code prompt.
The vault item has no TOTP, and the connected mailbox had no fresh npm code.
Chrome had no existing npm session; its extension UI then blocked further
automation. The temporary config was removed and `npm whoami` still reports
`ENEEDAUTH`. No token was retained and no package was published. Public
registry metadata for the unscoped launcher returned 404; that does not
establish namespace ownership. R5 still requires an authenticated ownership
and unused-version check after R4 qualifies the candidate.

After the full local gate, `cargo clean
--profile dev` removed this worktree's disposable debug build cache and
restored about 41 GiB of disk availability; release binaries, frozen bundles,
candidate data and native evidence under `target/` were preserved. Cargo can
rebuild the debug cache when needed.

GitHub runner budget check, 2026-09-29: the organization billing budgets API
returns an Actions organization budget of **$0** with
`prevent_further_usage=true`; the current usage API reports Actions net charges
of $0, and the Core repository permits Actions. There are no self-hosted Core
runners registered. This resolves the prior unknown-cap pre-dispatch question,
but it does not guarantee that a new job can run inside included minutes or
that GitHub's enforcement is instantaneous. Keep native jobs timed and do not
silently raise the budget if a target is rejected.

### Historical execution checkpoint — before the amendment

The records below preserve prior evidence and failures. Statements that the
runtime amendment is awaiting approval, that the shipping Mac candidate must
retain the baseline hash, or that controlled proof must be Paper-only are
superseded by the plan above when it is activated. No activation occurred as
part of writing this amendment.

**Current step: D2; D1 decisions are recorded above.** Core source checkpoints
include `7782118`, `48eb1d2`, and `b711ffc` (local, not pushed). Warden checkpoint is `d2bd10d`
(local, not pushed); Alpaca candidate branch is committed/pushed at `592bd8f`.
Core remote PR #2 remains at `520be81`. No native Windows success or five-target
qualification is claimed. The current continuation corrected the workflow's
required copyright notice and obtained a passing complete Core gate.

- Core Windows adapter is now shared by distribution and runtime through
  `platform/windows_private.rs`, explicitly classified as public Core source.
  Private-root setup, stable identity, no-clobber publication, installer locks,
  facade replacement, staged directory publication and metadata removal use it.
  Secret/config/installation reads validate the actual read handle. Existing
  Warden `.integrity-pending` evidence is preserved for Warden to validate, not
  adopted as authority. Only exact Windows `.tradeassembly-local-write-` plus
  32 lowercase hexadecimal digits plus `.tmp` scratch files may be preserved
  after owner/DACL/no-reparse validation; they are never adopted or deleted by
  retry. Unknown state and unmarked stable authority files still fail closed.
  Owner fixture setup uses the same private-directory API on Windows.
  Mac owner integration tests (8) and setup tests (10) pass; one additional
  orphan-file test is Windows-only and not yet executed. Public source/boundary
  scans pass after classifying the adapter and avoiding the private-package name
  reserved by the source scanner; no scanner exemption was added. Native helper
  MSVC cross-Clippy passes, but this is compilation, not Windows execution.
  Current logs: `target/native-core-install-tests.log` SHA
  `702ef3206059b7160ac0c233b2a2be8eac6d807027934d28ffbed1165db43306`;
  `target/native-core-owner-tests.log` SHA
  `95dc20aaf5e053e15a2b53e51e71952ed317155f290df281ba1f40b7ce3ef2ae`.
- Owning Alpaca format check covers all three Rust source files with direct
  `rustfmt --check --edition 2021`: cargo-fmt's metadata subprocess drops the
  command-line SDK patch. Actual strict Clippy, all 12 workspace tests and release
  build pass with the pinned SDK patch; previous package verification is reused
  because its source is unchanged. `native-plugin.yml` and Warden's pending
  `native-authority.yml` pass actionlint. GitHub Actions actually rejected Alpaca
  Windows candidate run `36577743556` (head `592bd8f`, job `109437623511`)
  before executing steps: recent account payments failed or spending limit must
  increase. Actions is enabled. No native receipt/artifact exists for this run.
  This is a verified native-runner dependency, not a build failure or assumption.
  Do not retry unchanged jobs or publish unqualified packages.
  Authenticated CLI billing inspection confirmed the organization Actions budget
  is **0**, with `prevent_further_usage: true`. No budget change was made. The
  retired `/orgs/TradeAssembly/settings/billing/actions` endpoint returns 410;
  the current `/organizations/TradeAssembly/settings/billing/budgets` API works.
  Core's `native-core.yml` is now committed and passes actionlint and source/
  publication-boundary checks; it supplies actual native compile/storage tests
  and binaries only, never a qualification receipt or private producer source.
- Full owning Warden `cargo xtask verify` exits 0 on Mac: formatting, strict
  workspace Clippy, 245 Rust tests (one ignored PostgreSQL test), boundary,
  dependency/license checks, machete and all 25 SDK tests. Evidence:
  `target/native-owning-verify.log` SHA
  `68ea3bc80940241491bb1dd2a41f4fec85499c393366457ef33f0fbbca4676de`.
  Native Windows proof and the required independent maker-checker remain due.

- Core branch `codex/f2-npm-distribution`: `distribution/src/native.rs` now
  rejects stale/foreign Alpaca locks and changed package bytes;
  `distribution/src/package.rs` requires v2 installer/tarball bindings, inspects
  actual npm tarball contents, rejects v1 receipts, and requires missing recovery
  and Windows-specific evidence. `docs/distribution.md` is the sole plan/state.
  Fifteen distribution unit tests and strict all-target distribution Clippy pass.
  These are verifier/packaging tests, **not** native runtime proof. Logs:
  `target/distribution-binding-tests.log` SHA
  `b21abc0e91c522a01a81d7a015d2d52a5f9e5b664bd2173707a61ab3edde51da`;
  `target/distribution-binding-clippy.log` SHA
  `c60b7ae6bedb85c2d1f4c202fef06e25ab596256da0d81aaae683c6a4efe0cf5`.
- Alpaca branch `codex/f2-native-distribution` in sibling worktree
  `/Users/davidjbeveridge/.codex/worktrees/f2-alpaca-native-distribution`:
  `xtask/src/main.rs` and native producer workflow changes. Native suffix/build-root/host-label tests
  pass; workspace tests (12), strict Clippy, release build and actual
  `cargo --config 'patch.crates-io.tradeassembly-plugin-sdk.path="/Users/davidjbeveridge/.codex/worktrees/f2-npm-distribution/plugin-sdk"'
  --offline --locked xtask verify` pass on Mac arm64. This is producer smoke,
  not a replacement for the frozen Mac plugin. Actual new smoke archive and
  paired sidecars are retained in `target/native-smoke-package/`; archive SHA
  `49b25731235a6bc3afcdde680c9f338687e07a390fb41146df86ddd142faf0d8`.
  Logs `target/native-alpaca-tests.log` SHA
  `9ceb31bc7b7a77823a6130eff8d1e5ae26f62db38e84ae2c939466f9497703ab`
  and `target/native-package-verify.log` SHA
  `d63896e2b0b766fb12e3e9c92805fc4ea2df5a8d480888e5e1f18f809f4e0cb6`.
- Warden branch `codex/f2-native-authority` in sibling worktree
  `/Users/davidjbeveridge/.codex/worktrees/f2-warden-native-distribution`:
  Windows private-file adapter in `warden-storage-sqlite/src/windows_private.rs`,
  storage integration in `src/lib.rs`, service seed/token/export integration,
  Windows-only dependencies and lockfile. Mac storage tests (60), CLI tests (3),
  strict affected-crate Clippy and audit-sequence regression pass. Four existing
  explicit-counter loops were converted to `zip` to satisfy the current Clippy
  gate, without suppressing it. The actual Windows adapter and its four native
  tests compile/lint for MSVC through ignored `target/windows-private-check`;
  they **have not run on Windows**, nor has the full Windows service built.
  Logs `target/native-storage-tests.log` SHA
  `7c7f9c74eaf476dae918294837053bb074ae1365764d9b7b2302f18937bb561a`,
  `target/native-service-tests.log` SHA
  `10361b8bd43c56885698b605062fc3db387884ad7c4a8108927602b1b2e22cda`,
  `target/native-authority-clippy.log` SHA
  `572e6dd6b641a749f84fd633c7b5984c644ed886ee7135826a553780879704c8`,
  and `target/windows-private-crosscheck.log` SHA
  `c599a6e26ad883be9eebd239804e79bb9536358acb84ea0c18b602ea1b6cb77d`.
  This earlier crosscheck hash predates the fifth lock-race test. Full owning
  gates now pass as recorded above; required independent maker-checker remains due.

Frozen Mac manifest and parent-lock hashes were rechecked unchanged. Only the
owned, disposable `target/core-gates` build cache was reclaimed (6.5 GiB); logs,
release bytes and other worktrees were preserved. The prior xtask binary was
copied and byte-compared to `target/native-distribution-checks-xtask` first.
Prior full Core gates are evidence for `36dd8aa`, **not** the new edits.

Core's new full `cargo xtask verify` attempt failed during test linking with
`errno=28 (No space left on device)`; retain `target/native-core-owning-verify.log`
as failed evidence. Reclaimed only the two new producer worktrees' disposable
debug caches with `cargo clean --profile dev` (1.6 GiB Alpaca, 2.9 GiB Warden),
preserving package artifacts, frozen payloads and evidence logs. Core retry handle
**76254** is terminal, exit 101: another linker `errno=28` in
`target/native-core-owning-verify-retry.log`. Reclaimed only this worktree's
generated runtime dev artifacts with `cargo clean -p tradeassembly-runtime
--profile dev` (7.1 GiB). A third actual full attempt, handle **55626**, reached
workspace tests but exited 101: the bounded virtual-time soak failed at
`service/execution.rs:6341` with `local database schema operation failed`;
free space was 180 MiB. Preserve `target/native-core-owning-verify-clean.log`.
Disk pressure is suspected, not a proven substitute for rerunning that test.

A generated executable probe went from 137,699,856 to 83,178,960 bytes with
system symbol stripping and still ran `--help` successfully. Reclaimed only
this worktree's disposable release/dev caches with Cargo (197.7 MiB / 9.3 GiB),
preserving all candidate/frozen artifacts and logs, and removed the temporary
probe. Next gate uses consistent dev/test `DEBUG=0`, `STRIP=symbols`,
`INCREMENTAL=0`, two build jobs, and the exact committed source revision.
These are build settings, not changed assertions or a weakened gate. The
stripped-profile gate (handle 80962, committed source `edca828`) exited 1 after
all 1,312 nextest cases passed (51 skipped), the previously failing soak passed,
and dependency, whitelist, plugin-contract and architecture checks passed.
`scan-public` then rejected the new workflow's missing copyright header. The
header is corrected; retain `target/native-core-owning-verify-stripped.log` as
failed whole-gate evidence. No process remains live; do not reuse terminal
handles. The composite proof must use Paper: the existing
controlled-broker helper/example are Live-only and cannot be substituted.

The complete rerun **passed**, exit 0: `just verify` invokes the complete
`cargo xtask verify`, pinned to source
`b711ffcee2a525888cf7e7a9255571add7eab73c` with the stripped dev/test profile.
All 1,312 nextest tests passed (51 skipped), as did the workspace test suite,
strict workspace Clippy, formatting, deny/audit/machete, whitelist,
plugin-contract, architecture, public scan and FOSS boundary. Evidence:
`target/native-core-owning-verify-final.log` SHA
`e234d547bfaf9bcdabb577c8dde2374e6acae69f7eb201b3d5c7d6b8eb98dc39`.
Handle 90265 is terminal; no build/test process remains live. Archive smoke is
not yet qualified on this revision. The original frozen manifest and parent
lock hashes remain unchanged. This is local Core acceptance, not five-target
distribution or registry qualification.

The supported npm legacy CLI reached an OTP challenge. The existing Bitwarden
login has no TOTP field; notes indicate MFA/recovery information but no TOTP URI.
No secret was printed or authentication claimed. A bounded Gmail CLI search
found no npmjs.com message in the last day; this does not prove an email challenge
or permit assuming the challenge type. Resolve the actual supported MFA path,
not a recovery/reset loop. The native Actions dollar-cap question is pending;
the verified $0 cutoff has not been changed.

The next bounded audit identified a frozen-contract conflict. Actual frozen
binary stdio `tools/list` returned `pluginInvoke:false`; its only order/recovery
named tool was `studio.agent_run.recover`. Source pointers: `mcp.rs` lists
`tradeassembly.plugin.invoke` in help, not the tool dispatcher;
`service/execution.rs` rejects external-agent evaluation;
`adapters/plugin_operations.rs` applies submission permits only to Live;
`broker_submission.rs` and `service/broker_recovery.rs` bind Live authority.
The existing controlled tests use a Live allow override, not the required
agent-driven Paper MCP boundary. They cannot qualify D3. A packaging wrapper
cannot supply missing connection-owned authority semantics in frozen bytes.

Independent portability checks passed using the actual frozen Mac bundle:
`packaged_sandbox` exercised bundled Node, positive outside controls, denied
reads/writes/egress, and offline invocation of the pinned Alpaca plugin
(`target/native-portable-sandbox-test.log`, SHA
`676c9ee8fd227c44623b9b23ca1382c029aea9e1162651225cb84e698a23d7cc`).
Actual local npm/pnpm installs with lifecycle scripts disabled passed
(`target/native-portable-package-managers.log`, SHA
`2815f8dc5298ebe5bbb8b296f1fc1248deb0738926ddecbc42852bbff23d2685`).
Actual source-free setup, real Warden and stdio inventory passed
(`target/native-frozen-mcp-inventory.log`, SHA
`a531453dc69c1aaca3a249ce4958516f442edae7d229065e85ea25d203fe9d5e`).
These are Mac results, not Windows, registry or controlled-order qualification.
Handles 24167, 5676 and 7821 are terminal, exit 0. Test-only changes are in
`distribution/tests/package_managers.rs`, `runtime-rs/tests/packaged_sandbox.rs`
and `runtime-rs/tests/local_binary_setup.rs`; full owning gates predate them.
Frozen manifest and parent lock hashes were rechecked unchanged.
Local checkpoint commit: `6f9edc4`; clean worktree before this status update.
The checkpoint's targeted validation (handle 89575, terminal exit 0) passed
`cargo fmt --check`, strict Clippy for those three test targets,
`cargo xtask scan-public`, and `cargo xtask foss-core-boundary` (411 files,
archive smoke not requested). This is a local checkpoint, not merge/release
qualification or a newly passed full owning gate. No live process remains.
The archive preflight confirmed that `run_archive_smoke` builds the extracted
workspace from scratch in a separate target directory, and
`configure_isolated_environment` clears the caller's stripped-profile settings.
Only 2 GiB is currently available; no doomed full archive build was started,
and its gate remains unsatisfied. This resource observation is separate from
the required runtime amendment, not a reason to weaken archive acceptance.

Execution is blocked pending the explicit frozen-runtime contract amendment.
The same conflict has persisted across at least three consecutive goal turns;
independent portable-test progress is checkpointed, not overall completion.
Native build spending approval and supported npm MFA remain pending external
dependencies. Do not resume automatic implementation merely to repeat these
checks or add more planning artifacts; resume when an actual dependency changes.

**Exact next action:** checkpoint these targeted tests; resolve the pending
explicit amendment question before any runtime rebuild: may a new compatible
candidate be built/qualified while preserving the original frozen payload and
M0–M6 lock? Keeping packaging-only leaves the composite D3 proof unsatisfied;
do not redefine success or enable Live to avoid that conflict. Independent
native qualification and authentication work may continue within existing
scope. Do not rerun unchanged full gates just to duplicate evidence.
Resolve GitHub's verified
native-runner billing dependency through the mandatory operations route without
inventing success or silently raising an unbounded spending limit. Execute owning
native builders and real Windows authority tests. Resolve native failures against the
recorded ACL/atomic-write contract, not by weakening validation. Implement the
qualification capture/composite controlled-MCP proof before creating receipts.
D3–D5 remain unqualified. Do not rebuild the frozen Mac payload, restart M1–M6,
expand Studio/Hub/Relay features, or reopen M7/M8.

## October 3 recovery adoption - full prior canonical snapshot

Historical snapshot below. Superseded execution instructions are not an active work queue.

# F2 unsigned Core beta + sellable production Relay

User-approved scope reset: 2026-10-02. This is the active execution plan and
replacement-goal contract. It supersedes older distribution instructions,
including their paid-Relay-launch exclusion. Historical Product M0–M8 contracts
and the frozen baseline remain unchanged provenance, not this unsigned launch's
completion definition. Source pointers, hashes and prior evidence remain in
[distribution-history-2026-10-02.md](distribution-history-2026-10-02.md).

Standing authorization: David reapproved necessary in-scope technical, legal and
financial operations on 2026-10-02. Do not ask for routine authorization again.
This does not expand L1–L4, establish an arbitrary purchase price or spending
limit, authorize Apple payment, or replace mandatory platform confirmations.

## Frozen scope

- One version-pinned unsigned/ad-hoc-signed npm beta on macOS ARM64, Windows
  x64, GNU Linux x64 and GNU Linux ARM64; scripts-disabled npm/pnpm installation
  and a compatible upgrade path. Fully qualify Mac; Windows/Linux require native
  build, architecture/inventory/license/integrity and registry-byte evidence and
  explicit experimental status, not runtime qualification.
- A clean Mac customer installs, signs into production Hub, connects Alpaca via
  OAuth and uses existing agent submit/reconcile, deterministic risk/authority,
  research/backtest/journal and promotion tools. Their external agent owns the
  loop. No new hosted agent runner or strategy/risk language.
- Relay is sellable in production: signup/login, an approved purchasable Hub
  plan, production checkout/return/subscription management, purchased entitlement
  and existing allowance enforcement, enrollment, durable journal ingest/read/
  export and the promised F2 Watch/Reach operations. Broker execution remains
  local/customer-managed with Warden controls.
- Owner-only staging protection on application boundaries, including direct
  origins; isolated production storage, IAM and secrets; verified rollback.
- CI owns repeatable build, qualification, deployment, publication and release
  verification. An agent fixes named failures, not schedules the release loop.

Excluded: Apple payment/Developer ID/notarization, Intel Mac, Windows ARM,
Windows/Linux native acceptance, F3/F4, UI redesign, other brokers, repository
visibility changes, new infrastructure architecture, cross-provider migration,
pricing redesign and automatic review chains. No actual broker orders or
production Live activation in acceptance. No weakened shipping controls, mock-
only end-to-end claims, fabricated receipts or test resources relabeled production.
Do not reopen unchanged M0–M6 evidence. Historical M7/M8 and signed/full-release
verdicts remain unfulfilled; this launch needs its own explicit unsigned-paid
contract and truthful verdicts, not a waiver of the historical requirements.

## Execution contract — 2026-10-02 reset

This section and the replacement goal below govern execution. Dated entries in
the checkpoint history preserve evidence; their old `Next:` instructions are
not a work queue. The frozen scope and all L1–L4 success criteria remain intact.

**Accepted checkpoint:** L2's four-target candidate verifier passed locally and
in GitHub run `37062163171`. The production-storefront Mac candidate passed all
14 native checks. Preserve its bytes, receipt and unchanged Warden/Alpaca/SDK
inputs. Windows/Linux remain experimental. This checkpoint does not establish
production service acceptance, registry installation or a sellable release.

**Reconciled stop checkpoint:** the user stopped the main thread and cleared its
goal. No new goal is started by this reconciliation. Relay `ad11765` is clean and
committed locally. The repaired clean-tree integration gate passed; staging
deployment completed. Logs are `/tmp/tradeassembly-relay-watch-ack-integration-clean-space.log`
(SHA-256 `a5422811d9c9c2469a5dcf4e72f7b9392c9a49505a1ab1cfdb4c9667e6a152ce`)
and `/tmp/tradeassembly-watch-ack-staging-apply.log`
(`6bb5e354e34e8530b17fa8748ad14c76ceae3c05380ff4bccda864d8f03d6553`).
Retain prior unchanged-Core composition evidence; the newest gate explicitly
skipped that optional local checkout check. Do not rebuild/redeploy this fix.

The actual repaired probe returned 200 for delivery and duplicate delivery, then
failed `actual browser notification not received`; cleanup revoked its node.
Private `e4-watch-custom-repaired.json` SHA-256 is
`f57d470657374f414f912883b0dc77f340a8f5cc947b584ecc79495db5e92223`.
The existing private browser capture has a subscription and zero notifications.
Collector PID 73204 subsequently expired; no release build/probe/collector was
running at reconciliation. Preserve the capture and Chrome tab; inspect current
state before a bounded resume. No accepted Watch/ACK proof or coverage increase.
Staging remains 24/35; enrollment, six Watch routes, four positive OAuth routes,
deployment/rollback evidence and separate production acceptance remain open.

**Current execution, superseding the stop checkpoint:** the replacement L1–L4
goal is active. Relay `b669dbe` rejects revoked installations instead of treating
Hub history reads as active authority; the real registration/revocation regression
failed before repair and passes after it. `7bc9225` adds the owning offline Watch
verifier and E4 leaf. Both actual staging surfaces passed all 19 cases each,
including real Chrome notification, issued/duplicate/durable ACK, recovery and
revoked denial. Custom proof SHA-256
`ba9d2af63b9dd36898784113e2b94fd2be3ef2530b6d9b1e5e00f89c6284a6aa`;
direct `8b033cf8bff838a1305aaafa49096f596bf4f953ebc30ca026e13d722a927560`.
The owning verifier, 18 targeted E4 tests and strict Clippy pass. The aggregate
accepts six leaves and covers 30/35 routes; report
`ababd1efab74f9382ef9f48a072d5fa114427e16e4d4953d3d9c07d1d7d829c4`
correctly rejects readiness for enrollment, four positive OAuth routes and both
deployment/rollback proofs. Staging repair deployment/readback passed with only
the Lambda code hash changed; runtime and harness-inclusive full gates passed.
The final full-gate log SHA-256 is
`d23f0962b6cf4facebdfa58eec081b9441f44e5a4cea0d739dbcb0f1271bfccc`.
Exact artifact/plan/log hashes and live handles are in the
compact coordinator checkpoint. The earlier failed resumed diagnostic remains
unaccepted. Core candidate/native bytes, production, and broker order state are
unchanged; paid-production acceptance, CI/npm/registry and L4 remain mandatory.

Enrollment continuation: Relay `fe416c0` + `7756cba` add the existing E4
workspace driver/verifier. Actual custom/direct surfaces pass all 16 checks:
owner re-enrollment/retry, unchanged pre/post quota, anonymous/nonowner denials,
exact cost/schema checks and active Hub paid-period bindings. This is an existing
operator workspace, not a new customer purchase. Proof SHA-256
`167cb7e397e2374837a10e2bcedb3f1108ddbdd65e07cbe92328cb9770fde635`.
21 targeted E4 tests and strict Clippy pass. The qualified Cargo.lock and Watch
proofs remain unchanged and valid. Aggregate SHA-256
`247e34ddf2733af1d125f2c206ec53138924079e889f6a4244199d5d3cc32eeb`
accepts seven leaves and covers 31/35 routes; exit 1 correctly requires the four
positive OAuth routes and deployment/rollback proof. The compact checkpoint
records the passing full integration gate (log SHA-256
`d95b59504e40438011228603f0e4e9cf8ae1f447c860999d5cad389c68d111db`;
optional Core composition explicitly skipped) and exact next action. All L1–L4 production,
CI/publication and registry requirements remain intact.
The exact frozen production package also installs into isolated private local
test state and its browser/MCP Hub login succeeds. Alpaca consent is pending the
specific action-time access/terms confirmation; only `data` is requested on the
existing Codex-managed Paper account. There is no trading scope, order, payment
or new production entitlement proof. Preserve the live handles in the compact
checkpoint and complete callback/status/ACK evidence before counting connection
acceptance. The earlier bare-producer/profile mismatch is not a native defect.

**Production remains a separate mandatory gate:** prove the exact deployed
production package/profile's Hub login and Alpaca OAuth; actual production
purchase/webhook/entitlement/allowance and subscription management; enrollment,
journal ingest/replay/read/export; promised Watch/Reach effects and denials;
deployment/storage/IAM/secret separation and data-preserving rollback. Bind
observations to production resources, actors and artifacts. Staging receipts,
test-mode purchases and a matching source revision cannot substitute for this.
No real broker order is required or authorized by this acceptance plan.

### Bounded architecture findings and decisions

- `runtime-rs/src/workos_identity.rs::current_identity_and_token` deletes the
  stored session on any refresh error. That can turn a transient provider failure
  into another browser login. Defer changing the qualified Core binary unless
  this defect blocks customer use or required acceptance. If it does, approve a
  candidate amendment through existing gates; preserve prior evidence and
  requalify changed bytes. Never silently inherit a binary qualification.
- Product `.github/workflows/f2-release.yml` already runs native packaging and
  the exact candidate verifier. It does not yet perform the entire deployment,
  production acceptance, npm publication or final launch chain. Extend that
  workflow and existing owner commands. Do not build a generic release engine,
  agent scheduler, new credential service or another acceptance framework.
- The workflow previously registered push events while all executable jobs
  required manual dispatch. Relevant pushes must execute a real check; an
  all-skipped workflow must never be counted as successful release verification.
- `tools/f2-github/src/evidence_cache.rs` already transports only digest-bound
  evidence and rejects unsafe/mismatched files. Reuse it. Candidate verification
  after extraction is cheap and remains required; native qualification is not
  repeated just to move evidence between machines.
- E4 currently binds candidate/configuration and probe/router source hashes.
  Preserve those checks. Source selection may be narrowed only when an owning
  validator proves the exact relevant dependency set and tampering/invalidation
  regressions pass. Do not hand-edit receipts to force a newer source hash.
- Product harness instructions still prescribed a permanent coordinator model
  and automatic review rounds. The active root owns this bounded execution;
  existing user model selection wins. No routine reviewers, delegation, history
  forks, issue fanout or planning packet per repair. The focused escalation below
  is explicitly authorized for the main thread.

### Main-thread model and escalation contract

Recommended root: `gpt-6.1-sol` with `high` reasoning. This is an engineering
judgment for the remaining integration/debugging work, not a measured claim
about model superiority. Honor a different model explicitly selected by David.
CI/CLI execute routine steps; the root owns decisions, integration and failures.

After two substantive failed repairs of the same acceptance failure, repeated
rework of the same fix, or an unresolved consequential authority/concurrency
decision, stop that approach and obtain one focused diagnosis. Ordinary build
waiting, missing credentials, disk exhaustion and known provider throttling are
operational conditions to resolve with tools, not reasons to buy more reasoning.
Do not count unrelated errors or polling turns as failed repair attempts.

The main thread may use one fresh-context `default` subagent with
`model=gpt-6-astra`, `reasoning_effort=high`, `fork_turns=none`. Provide the exact
failure, expected behavior, relevant source pointers, attempted fixes, evidence
and a bounded question. Request a root cause, smallest coherent repair and
discriminating acceptance check. The root validates and integrates the result;
deterministic gates still decide success. If the root is already Astra High,
a focused Astra `xhigh` diagnosis is the escalation. Do not create reviewer
chains or bounce indefinitely between models. This authorization is for the
main thread; side conversations keep their own no-subagent boundary.

### Tool choice before each operation

Use existing CLI -> callable MCP/Codex integration -> bounded script over a
supported API -> Chrome. Follow `ops_route.py` for the concrete operation and
record real availability/blocker evidence; preserve and reuse its valid route
assessment instead of redoing unrelated setup. Do not infer that no API exists
from a single failed command. Repair routine authentication/configuration first.

For console/network/Service Worker/DOM diagnostics use Chrome DevTools MCP when
callable, or the supported protocol/API route. Do not manipulate DevTools panels
through screen coordinates to obtain data an existing diagnostic interface
provides. Native Computer Use is the last fallback for a specifically identified
unsupported operation. A browser-only login fallback ends when login completes;
return to the CLI/API for the original operation.

At reconciliation, `chrome-devtools` was present but disabled in Codex config.
It is now enabled and `codex mcp get chrome-devtools --json` confirms that state.
This side conversation still has no callable DevTools tools: configuration is
not connection proof. At main-thread startup discover callable tools; reload
that MCP connection if needed and verify it before debugging. Do not repeatedly
navigate Chrome settings/DevTools to compensate for a disabled connector.

### Evidence reuse and invalidation

| Change | Required action | Evidence retained |
| --- | --- | --- |
| Docs, status text or CI wiring only | Validate that change and its owning integration gates; run actual CI job before claiming CI success | Unchanged native artifacts and qualification; valid service observations |
| Hosted Relay/Hub implementation | Owner tests, affected real staging/production behavior, deployment and compatibility checks | Core/Warden/Alpaca native qualification when their actual inputs match |
| Production connection profile or packaged assets | New candidate descriptor; existing package/native gates and affected production onboarding | Unchanged producer build outputs, historical receipts |
| Core/Warden/plugin native inputs | Rebuild affected outputs; owning gates and required candidate qualification | Unchanged other targets/components only where their validators accept them |
| Environment/IAM/storage/provider configuration | Fresh affected deployment, isolation, authority and rollback observations | Native qualification with unchanged payload/profile |
| Proof driver or verifier | Tests for false positives and dependency invalidation; owning verifier determines whether old raw observations remain usable | Original raw observations and historical results, never rewritten verdicts |

Immutable qualification may be reused by matching inputs. Live facts such as
current access, provider connectivity and deployed health require a fresh
production smoke at promotion; they are not cached indefinitely. Scope/source
changes invalidate affected evidence. Each owner's broader required gate still
runs at its integration boundary. There is no general exemption from gates.

### Execution order and browser bootstrap

1. Resume from the stop checkpoint above. Inspect the existing browser capture
   and receiver with the appropriate diagnostic tool; the hosted fix and gate
   already passed. Resume the expired collector only when ready for the next
   bounded actual delivery/ACK check, using its existing origin/subscription.
2. Resolve the already identified first-launch dependencies once: production
   Stripe credential repair and genuine purchase/access/portal evidence; npm
   namespace/MFA/trusted-publisher bootstrap; initial Hub/Alpaca consent and the
   real notification receiver. CLI/API checks follow supported authentication.
   When a dependency genuinely requires human input, record the exact handoff
   once and continue only independent in-scope work. No repeated navigation.
3. Finish the finite missing L1 service/isolation/rollback observations. Use the
   existing E4 adapter and owning probes. Production effects are explicit; a
   passing staging aggregate cannot complete L1. Keep browser journey coverage
   for changed customer auth/checkout surfaces and actual WebPush delivery.
4. Complete one L3 workflow around existing commands and the L4 verifier. This
   wiring is independent work while provider bootstrap is pending; do not wait
   to publish before building the pipeline. First-launch proofs feed that same
   pipeline. CI controls jobs, retries, artifact retention and dependency order.
5. Run the pipeline against the frozen manifest, integrate qualified source,
   publish beta, verify actual registry bytes/install/update, and obtain both
   unsigned Core readiness and production Relay sellability verdicts. Then stop.

Routine deployment uses scoped CI identities and designated acceptance actors,
not an operator's browser cookies or a Bitwarden session uploaded into CI.
Customer refresh grants remain scoped and revocable; synthetic grants do not
prove paid entitlement. A browser receiver may remain running for real push
delivery, with structured observations collected automatically. Reauthenticate
only when the supported provider flow requires it. Never bypass consent/MFA.

One immutable release input and one generated CI summary replace manual release
shepherding. Use GitHub's job dependencies, concurrency and rerun-failed-jobs
mechanisms; use existing digest verification for retained artifacts. After an
ambiguous dispatch/deploy/publish response, inspect the existing operation or
immutable version before retrying. CI stops on failure and emits its exact failed
step, input identities, report/artifact links and bootstrap dependency. The root
fixes that failure and resumes the affected job. An agent does not poll, click
through accounts, repeatedly regenerate plans or orchestrate each passing step.

Detailed receipts stay in retained CI artifacts. Update the canonical compact
checkpoint only when a gate passes, the next action changes, or a dependency is
identified. Model success statements and manually entered booleans are not proof.

## Checkpoint history and evidence

### Actual staging rollback qualification — 2026-10-03

Coordinator source `52ec6abd8cd0a6b037d625ce32137d936fa12787` adds an explicit
default-off staging-Watch-only changed-code qualification switch. Reviewed
trust-only repinning leaves existing permissions/resources/configuration intact.
Actual GitHub plan `37106573790` attempt 2, deploy `37106704530` and rollback
`37106818511` succeeded with intent archived before effects and exact provider
readbacks. Production run `37106705426` was plan-only; no production update.
The existing owning deployment validator accepts the actual retained different
executable deployment and restoration, preserving nonempty journal data,
archive bytes, storage protections, configuration and unrelated functions.
This closes staging rollback proof only, not production or overall readiness.
The retained E4 `CANDIDATE` argument is the exact packaging status file
`target/f2-npm-mac-package-production-storefront/candidate.json`, SHA-256
`4820ab91e67bf8cecb1755366a3a1a0c04d34f0e069db88a2753b58d3410f950`.
It is not `tradeassembly-darwin-arm64/candidate-descriptor.json` (`51da2114...`),
which packaged-connection and IAM commands explicitly require. The former's
`release.candidateDescriptorSha256` binds the latter and both refer to the same
frozen `30dcbd46...` archive/profile/native bytes. Never relabel a receipt or
redeploy to repair this input-type error. Current owning deployment verification
revalidated the original three snapshots/plan/deploy/rollback with the correct
status file (`6b12e7`, terminal 0); all source/config/input hashes are unchanged.
Coordinator targeted 55 tests, fmt/strict Clippy/actionlint and `just verify` pass.
Candidate CI `37106550626` passed; deliberately skipped release jobs do not count.

Private Relay corrected aggregate
`.operator/relay-environments/staging/e4-current-with-actual-rollback-corrected-20261003.json`
SHA-256 `3ffec1a943d9243d635500c690fb4a220f31795adf8052d98d1edf14059f2ca9`
passes all eight supplied owning leaves with no deployment gaps, 31/35 routes
proven, four positive OAuth routes outstanding. Exit 1, `fullE4Complete=false`
and `releaseReady=false` are required. Correction selected the retained exact
route proof, not new observations or effects. Compact source/process/artifact
bindings are in coordinator `RECONCILIATION.md`; do not rerun accepted work.
Read-only production Hub Stripe checks still return HTTP 401 for the existing
correctly formatted live restricted key; no credential or entitlement mutation.
The user added an npm Bitwarden passkey. On 2026-10-03 the existing browser
account completed the supported npm CLI web login using that passkey; the CLI
exited 0 and a separate registry `npm whoami` returned the expected publisher.
The temporary mode-0600 token was saved/read back in the existing Bitwarden npm
item without changing its password or passkey. This proves publisher login,
not publication permission or recovery-hold clearance for a publish operation.
No package was published and no account recovery was repeated. Bootstrap,
actual production commerce/lifecycle/Watch/Reach, eleven-job release integration,
exact npm publication and registry install/upgrade/rollback remain mandatory.

Core worktree `/Users/davidjbeveridge/.codex/worktrees/f2-npm-distribution`, branch
`codex/f2-npm-distribution`: pushed source
`3abf3e052b932dff5c0a0c18bf5673691842e58b` permits customer tenant selection
instead of a fixed diagnostic organization. Clean Core gate `6610` passed
(1341 nextest passed, 61 explicit skips); clean Relay gate `93560` passed at
`cc694fdedb92f0e2a80ad579c1a40fd078e9ba7e`. Optional Core-checkout Relay smoke
was skipped, not passed. Both processes are terminal; do not restart them.
Core log `/tmp/tradeassembly-customer-profile-core-verify.log` SHA-256
`79c4a3d8b23493d8a269160f5a2c51165c53a69e2faae98d4a7b056190532393`;
Relay log `/tmp/tradeassembly-customer-profile-relay-verify.log` SHA-256
`b1adca7c1d8a2d5fa63a598c1b3e688d603b3ce7508d81fa049e4a53912db1aa`.
GitHub native builds at exact source `3abf3e0`: Mac ARM `37029189820`, Linux
x64 `37029195635`, Linux ARM `37029200333`, Windows x64 `37029205949`.
Mac and both Linux runs passed and their archives were digest-validated by the
existing capture tool. Windows failed only the two real browser-onboarding tests
with `onboarding_listener_unavailable`; preserve its failed log
`/tmp/tradeassembly-core-3ab-windows-failure.log` (SHA-256
`247bcd44297bfa24f6f65186ab6ccff2f71c192abe30f7b0f289f2a64d970638`).
The Windows fixture cleared even `SystemRoot`; a test-only repair now preserves
that OS-owned variable while leaving auth/broker/project configuration absent.
Local stdio/HTTP/restart tests pass (2/2), log
`/tmp/tradeassembly-onboarding-windows-env-fixture.log` SHA-256
`d2819d18c4aabaa7199a5dd7579cfbb33c88bad6a3b5bbacf7810c33ecf1c6d7`.
Windows retry `37030939714` at `296d603` passed both real stdio onboarding tests;
it then failed the unit fixture's intentional non-Unix database-ownership panic.
Its failed log is `/tmp/tradeassembly-core-296-windows-failure.log`. The repair
uses the existing locked `winapi-util` safe file-information API for Windows
volume/file identity, preserving rejection of foreign/replaced fixtures. Native
CI also runs the database-ownership tests; no test is skipped or weakened.
Local ownership tests (2/2), onboarding unit tests (5/5), and public scan pass.
Second repair `08ddf1859eceb022c2a91f93d295831294697af7` is pushed after clean
full Core gate `60452` exited 0; log
`/tmp/tradeassembly-windows-test-db-core-verify.log` SHA-256
`45a1f8ee0d86ad6565d5c3a6e34731fa4dce8825de78515661343dd6b53fc485`.
Windows-native GitHub `37034024144` passed at the exact `08ddf18` pin; watch
`17348` is terminal 0. Deterministic capture passed, artifact `11239686980`,
ZIP SHA-256 `3afa22688ed8c1275d458f7235c280790768579804a2691a2b95cdee1ddbbd19`.
Its archive/readback are preserved under coordinator
`artifacts/f2-github-v2/windows-x64-customer-profile/`; capture is not package
qualification or Windows runtime acceptance.
Windows extraction `95946` also exited 0 through the owning Rust verifier;
exact Core/launcher/installer, unchanged Warden/Alpaca and SDK-tree bindings
were checked. Outputs are `target/github-build-outputs-windows-x64-customer-profile/`.
No native Windows customer acceptance is claimed; experimental packaging remains.
Mac watch `71877` is terminal success. Private coordinator
`35bbfda` pins all four Core targets; capture tests (2/2)/strict Clippy passed.
Producer and SDK pins are unchanged. Captured Core ZIPs under coordinator
`artifacts/f2-github-v2/{macos-arm64,linux-x64,linux-arm64}-customer-profile/`:
Mac `f42cddd7e34213aa4b60ea455a01033261edebc2c90a1a1d978609c4a2da5210`,
Linux x64 `1d730670ff5c24e1fcfff9092036fae1f6a15819ac0edb639039a8d0c458a750`,
Linux ARM `e1cd0338c8624e9766a7dd5ffcd75eeeb04a5081e642c5ca303d1e8f1ac38854`.
Their extraction processes and public scan are terminal success. Do not
redownload unchanged producers. Clean full Core gate `34942` passed at `296d603`
(1341 nextest passed, 61 explicit skips), log
`/tmp/tradeassembly-296d603-core-verify.log`, SHA-256
`e76b797a1866bbe2a36d811c9051c575358b5586ac05e777dea69cb2de8ebc84`.
Mac package `target/f2-npm-mac-package-production-customer` is now packed as
`0.1.0-beta.3`, binding the exact captured `3abf3e0` Core bytes and private
production profile SHA-256
`1beb89e9afd501697470ca21d824e28050e117153305b57175c9e7322a4511f1`.
Native package qualification `93761` exited 0: source-free install, npm/pnpm
ignore-scripts, upgrade/rollback and preserved state, sandbox denials, real
Warden controlled sink, duplicate and ambiguous recovery. Receipt
`target/f2-npm-evidence-production-customer-v3/aarch64-apple-darwin/receipt.json`
SHA-256 `05adfd223864fc17504cd9d4ce150512df60dda1abe8e935f60c5a235285b6d3`.
No actual broker orders or production Live activation. Prior v1/v2 invocations
failed on operator-supplied paths; preserve them, do not relabel as passes.
That Mac receipt binds the older test-commerce profile, not the corrected
live-commerce profile below. Preserve it as historical package proof; repack
and requalify the final profile without rebuilding unchanged native payloads.
Next: actual production paid Relay onboarding and final package qualification.

Private Relay worktree `/Users/davidjbeveridge/.codex/worktrees/f2-relay-watch-remember`:
real staging OAuth setup/restart, both-origin journal proof, 19 OAuth negatives,
and revoked-owner proof/verifier passed. Revocation evidence is
`staging/oauth-revocation-proof-v4.json`, SHA-256
`b853076acba796d6a63169aa1aecbb20b7bf203959bdecc3496168176bd8581a`.
Original admission was restored exactly. Do not repeat that mutation for a newer
timestamp; complete route/node coverage is still required.

Private Relay `d11524a` corrects the production profile to the registered live
commerce client; its manifest-bound regression rejects the registered test
client. Three targeted tests and strict all-target xtask Clippy passed, log
`/tmp/tradeassembly-production-live-client-profile-gates.log`, SHA-256
`8e9b3331e60f762f054823484667ab78275037298bcfd92238bcd55a4bcd2fcb`.
Corrected profile SHA-256 is
`33edb356f2ed7d505c2c01c8d149193d06f0196f810948ca0550ad893aad0973`.
The private production profile validator passes, but real packaged production
onboarding and aggregate service acceptance do not. Its `checkoutUrl:null` is
an open purchasable-onboarding gap: use the actual existing Hub catalog path,
not a new billing system, test tenant, invented price or entitlement bypass.
Actual Chrome production Hub sign-in now has operator catalog authority after
normal WorkOS administration repaired the intended existing membership; do not
bypass Hub through DSQL or fabricate a paid entitlement. Official WorkOS CLI
`0.23.0` accessed the dashboard through
existing Bitwarden credentials without Keychain. Production environment identity
was verified and targeted explicitly; existing `hub-operator` permissions are
present. The intended user's organization membership was `member`; only that
membership was assigned the existing Hub Operator role, with CLI readback and
a fresh signed-in operator registry. No global default/customer role changed.
Relay has an active monthly catalog entry and both existing clients. The older
F2 gateway binding was corrected through the versioned manifest review below.
Legacy dashboard edits correctly reject this manifest-managed product;
use versioned plan/apply, not a guard bypass. Hub `ba45db1` checkpoints the
operator review panel and signed-assertion route aliases. Hub Rust formatting,
strict workspace Clippy and all workspace tests passed (two explicit skips),
log `/tmp/tradeassembly-hub-registration-full-gates.log`, SHA-256
`100f24403c323119f5e5668995d7576a66a121c4c1c4bba3610984b5ad1729e2`.
Edge check and dashboard build passed; four ARM64 Lambda packages built.
These gates prove the scoped Hub repair, not paid Relay acceptance. SSO refresh `54230`
completed successfully; AWS CLI verified the intended `HubProductionDeploy`
assumed role in account `056319544861`. Bitwarden CLI supplied credentials and
TOTP directly to the requesting Chrome process without logging them. No root
bypass or MFA disable. Plan `25950` failed safely before apply: omitted
observability inputs would remove existing alerts and violate the outbox
precondition. Readback confirms the existing confirmed SNS subscriber and $25
Hub budget. Final plan `54589` also preserves the existing enabled recovery
timer. Reviewed plan `dist/hub-release-evidence/20261002T172002Z-38417/hub.plan`
has no deletions/replacements: four package/alias updates and Stripe-webhook
outbox-wake permission/configuration. Guarded apply `55336` exited 0;
`/tmp/tradeassembly-hub-manifest-runtime-apply.log` SHA-256
`dffa68933b3fca5c27716b29d95e4406bcaf8dc7858b56ac5550a9585d880202`.
DSQL/KMS positive and negative authorization checks passed; API live alias is
version `42`. Worker deploy `59489` failed before upload because Wrangler was
signed into a different account. Scoped refresh `98563` succeeded for the
existing Hub owner, account `2e778d60d9f80f40c6af6f499c52acc8`, with no all-account
or unrelated AI/email/container access. Retry deploy `37739` exited 0; Worker
version `51a5e5f8-a5fb-43f6-b3b0-b3dc421a0f7b` serves the existing custom domain; log
`/tmp/tradeassembly-hub-cloudflare-dashboard-deploy-correct-account.log`.
Worker `/readyz` returned `{"status":"ready"}`. The deployed panel exposed a
missing read path for the current registry revision. Hub `0e84418` adds only
that read alias/panel operation through the existing operator authority, plus
anonymous and signed-nonoperator denial tests. Full Rust gates and Edge checks
passed: logs `/tmp/tradeassembly-hub-registry-read-rust-gates.log` and
`/tmp/tradeassembly-hub-registry-read-edge-check.log`. Package `14972`, reviewed
runtime apply `67458` and Worker deploy `77035` all exited 0. Worker version
`6b876e22-0580-459f-be60-44d2f91f9bf4`; apply plan had only code/alias changes.
The real operator read proved revision 1, then a reviewed plan changed only
`relay-web` to the production origin. Private Relay `2ffb2a7` binds revision 2,
mutation `relay.f2.production.0002`, manifest digest
`sha256:173cd8b1ffb3ba7445a1c061972307b71f4c2cafa34951ca587499a0fc1291f3`.
Plan digest `sha256:f31a14c2219e9ef27dd4d9ec5dcfc9491d13de50be31fffb1e9726512220b8b9`;
actual apply and fresh readback both prove registry revision 2. Receipt screenshot
is coordinator `artifacts/f2-production-registry/relay-revision-2.png`.
Existing live/test clients, price mappings and entitlement policy are unchanged.
Purchase-start decision: use a product-neutral Hub buyer storefront, not a new
Relay-hosted OAuth/session BFF and not billing in public Core. The existing
product-bearer checkout keeps its mapped-product authority unchanged. A new,
separate dashboard buyer contract must require the verified Hub session plus
`hub:commerce:checkout:write`, bind the account to its verified tenant/subject,
and resolve an active account-subscription offer and one exact cloud-web return
template from the current registered product configuration. A registered live
client must exist; test-only registrations are not sellable. Caller-selectable
prices, customer IDs, identity, commerce mode, entitlements and free-form return
URLs remain forbidden. The storefront URL carries product/offer/surface IDs
only. Its signed login continuation accepts only the bounded storefront path,
not arbitrary redirects. The request carries an opaque idempotency key and
expected manifest digest; stale configuration is 409, and checkout continues
through the existing durable reservation/customer/session/replay machinery.
Stripe's hosted checkout displays the actual price before purchase. Neither a
return nor an open Checkout session grants access; real webhook/entitlement
evidence is still required. Reuse the existing Hub purchases/portal interface.

Hub `a43a55e` locally implements the missing AWS dashboard billing-portal route
with live customer/subscription ownership, signed buyer permission, strict
return target and Stripe-only redirect validation. Four targeted tests pass;
full fmt/strict workspace Clippy/workspace tests passed (242 passed, 2 existing
ignored). Test log `/tmp/tradeassembly-hub-portal-rust-tests.log`, SHA-256
`f138bee9925eaf6a005e8b7c5a091cdf2c8213d9add589490b2588fa853fac6b`.
This is adapter/route test evidence, not a real paid Portal/E2E receipt. It is
committed locally, not yet pushed/deployed. No paid grant is fabricated.
Hub `1519033` now implements the separate buyer API, bounded signed storefront
continuation and thin purchase view. It preserves the product-bearer route,
rejects caller-selected provider/identity/mode/return authority, checks the
manifest at reservation, and persists the same browser attempt across ambiguity
and reload. Real disposable PostgreSQL checks found and fixed an existing
subscription query missing its commerce-mode bind, and a replay projection
that selected the live price for test subscriptions. The new reservation returns
its selected price from the transaction snapshot. Full Rust fmt/strict Clippy/
workspace tests passed: 246 passed, 3 ignored; the two database commerce tests
were separately executed and passed against isolated PostgreSQL on port 15473.
This is real database/adapter evidence, not Stripe or paid-production evidence.
Logs and SHA-256:
`/tmp/tradeassembly-hub-buyer-rust-tests.log`
`09cf6a7e958ca915c9905efebbc2a205c2afa315b54ca62756c9acaab96fa610`;
`/tmp/tradeassembly-hub-buyer-database-tests.log`
`67d9c2dd1bcf5c032324f3194ec0afe60b25216ac834566b73c358f3201a35c5`;
`/tmp/tradeassembly-hub-buyer-edge-check.log`
`4c9478012ab06946efa0b1224c80c524f0ba7ee552410de4b7c435dc7fe0b054`
(95 passing tests, bootstrap/type checks); dashboard build passed with unchanged
CSS and asset `index-DxtxgvWs.js`. Hub has no UIS manifest/DESIGN binding;
`uis validate --workspace . --json` reported that absence, so this slice reuses
existing components/styles rather than adopting a new interface system.
Disposable server `/tmp/tradeassembly-hub-buyer-db.NsiVzL` was stopped cleanly;
no user rig was stopped and its test state remains recoverable.
Private Relay `f7286b0` sets the exact Hub storefront checkout URL. Profile
SHA-256 `6c73b9e50ce898d11a49fa62903fba984f59e4af2fa5a49aa417eb9e556a2869`;
owning three tests and strict xtask Clippy pass, log
`/tmp/tradeassembly-relay-buyer-profile-tests.log`, SHA-256
`2e4d56852cd25d2320e637e9751d067f03a792cd9698d58965970f8f9381c41a`.
Hub source is committed locally through `93a82df`; private Relay remains
`f7286b0`. AWS code-only deployment of `1519033` completed from reviewed plan
`Hub/dist/hub-release-evidence/20261002T181709Z-62694/hub.plan` (four Lambda
code hashes and four live aliases only). Existing DSQL/KMS positive and
negative authorization checks passed. Cloudflare Hub Worker version
`3ddae085-beff-4ce6-871d-abb795cd52c6` deploys the storefront plus `93a82df`
account-binding repair: Edge now matches Rust's `org_id` or `user:<sub>`;
nonstandard aliases cannot choose account authority, and legacy bare-subject
personal sessions require fresh sign-in rather than silent account migration.
Owning Edge check passed with 100 tests; log
`/tmp/tradeassembly-hub-identity-edge-check.log`, SHA-256
`b23f84a8b5d2560552c02bfeaf61ec7a180f71cbec8297c6750b7b3cf6dce7e6`.
Dashboard build passed, log `/tmp/tradeassembly-hub-identity-dashboard-build.log`,
SHA-256 `70390a3c12776d553b58da93870d01260e90445a0bf033605e2274266bc71e69`.
Actual production probes: storefront 200, anonymous buyer API 401, readiness
200. Sources are not yet pushed. No actual charge, webhook grant, browser
rendering or final native/profile qualification is claimed.
Actual Chrome storefront now renders the signed-in buyer's registered Relay
monthly offer and enables checkout, proving the real buyer read permission.
Checkout and duplicate-safe retry both return 503. Authoritative custom log
group `/tradeassembly/hub/production/api` records Stripe `/customers` HTTP 401
at `2026-10-02T18:26:24Z` and `18:26:43Z`; the Lambda live alias is version 44.
This is a provider credential rejection, not a frontend timeout or failed
reservation. Preserve the existing browser attempt and durable reservation.
Immediate next action: repair the approved production Stripe credential
workflow, validate its provider access without exposing the value, update the
existing secret if needed, then recover the same attempt. Hub instructions
require `aws-secrets-manager` and `asm-exec` for operational secret handling;
neither was found locally. Official AWS skill search returned
`creating-secrets-using-best-practices` (loaded), not the required helper/skill.
No direct Secrets Manager value reads, payment or grant were performed.
Credential investigation: Bitwarden's default Stripe CLI credential validates
against a different account with no Relay price (registered price returns 404);
it was not reassigned or modified. The local `tradeassembly-staging` CLI live
credential returns 401. Chrome Bitwarden autofill completed normal sign-in to
the intended Stripe account `acct_1U5pZ0FGQTzcfc3P` (Trade Assembly). Its
existing `Hub Worker Commerce Production` restricted key is present but its
secret cannot be revealed through that row. No full-access key substitution,
permission expansion or rotation was performed. The existing-key rotation
dialog is open in agent-owned `stripeRecoveryTab`, Chrome group
`Hub Stripe recovery`, awaiting the required credential-change human handoff.
Choose old-key expiration in 24 hours and submit rotation, then leave the new
key display open (never paste its value into chat). Resume by securely saving
the replacement in Bitwarden, validating the registered price/provider access,
updating only the existing runtime secret and recovering the same checkout.
Verify permissions and production runtime refresh before claiming repair.
After credential repair verify hosted storefront
login, actual registered price and real purchase/access/portal. Check the
dashboard client's buyer permission and cross-client subject/tenant binding
from actual verified sessions; do not add synthetic entitlement or bypass
authority to make the check pass. In-scope owners: Hub API/manifest/dashboard auth,
dashboard API/view/tests and existing commerce contract; private Relay profile
and its existing profile tests; canonical release documentation. No Core native
rebuild, new database migration, new hosted OAuth client, price change, actual
broker order, Apple payment or unrelated UI redesign is needed for this slice.
Before deployment require authority/CSRF/ownership/mode/return/stale/replay
negative tests and the owning full Hub Rust/Edge gates. Review normal deploy
plans for code-only updates; then obtain real production purchase/access/portal
evidence and the protected-staging matrix before final profile packaging.
Temporary CLI credential files are removed
after each run; refreshed credentials return to Bitwarden.

An earlier Mac candidate passed native qualification; later source/profile bytes
cannot inherit that pass. Reuse unaffected GitHub producer outputs/evidence.
Current Mac candidate has now been repackaged with the exact production Hub
storefront profile, using existing unchanged native inputs and installer.
`target/f2-npm-mac-package-production-storefront/candidate.json` binds version
`0.1.0-beta.3`, archive
`30dcbd465e5964ded291328f3e8bbd92b7cfac8d663abfe01945bd21e8120c9b`,
descriptor `51da2114d1e8ec86de4089734b8f165d76fc80e98fefbd660f7586c150fa7fea`,
profile `6c73b9e50ce898d11a49fa62903fba984f59e4af2fa5a49aa417eb9e556a2869`.
Freeze/describe/pack exited 0; candidate remains explicitly not publishable.
Fresh native qualification completed (terminal `18900`, exit 0), source revision
`03e60da` at preflight, log `/tmp/tradeassembly-production-storefront-qualification.log`
SHA-256 `c00207314b9417cc3f48930ac1035d085daf678b071889c37673312da9fe71eb`.
All 14 required local native checks passed, including real Warden/controlled
sink, duplicate and ambiguous recovery, source-free setup, sandbox denials,
scripts-disabled npm/pnpm and state-preserving upgrade/rollback. Receipt
`target/f2-npm-evidence-production-storefront/aarch64-apple-darwin/receipt.json`
SHA-256 `1133c3949ed404d205e8ba5fa5b72c63bf38184ce96094a55649d6ee42b6fa53`
binds the new descriptor; actual broker orders are false. Artifact/check hashes
were independently recomputed, not inferred from the pass label.
Next: preserve this qualified Mac artifact; finish equivalent native packaging
for the experimental targets, real L1 production acceptance, CI-owned publication
and the full aggregate gates. Stripe's human key rotation dialog remains pending
(verified this turn). L1/L3/L4 and overall L2 candidate gate remain open.
npm ownership/authentication, complete candidate/registry gates and integration
remain open. No publication or production paid-readiness claim.

Preserve native MCP `37062`, original MCP `80253`, Warden on `8186` and user rigs;
verify these historical handles before action, never restart on observation
timeout alone. This plan update changes no runtime or deployment. Immutable
baseline lock SHA-256:
`222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`.

## Runtime repair and candidate amendment — replacement goal

Complete finite L1–L4 below: unsigned four-target Core npm beta plus sellable
production Relay and a CI-owned release path. Fully qualify Mac ARM64; publish
Windows x64/Linux x64+ARM64 as explicitly experimental build/integrity-verified
targets. Preserve frozen artifacts, broker authority and unchanged valid proof.
Prove real production signup, Hub purchase/entitlement, Alpaca OAuth without
copied keys, enrollment, durable data, Watch/Reach and denial/isolation boundaries.
Use existing deterministic acceptance tools; add only missing aggregation and
CI orchestration. Preserve accepted L2 candidate evidence, the reconciled Watch
state and its private browser capture. Resolve first-launch identity/commerce/npm bootstrap once. Defer the
Core refresh-session change unless required for customer use or acceptance.
Extend Product's existing workflow; no generic release framework or new runner.
Build only invalidated outputs, qualify exact artifacts, promote/deploy those
artifacts, publish npm beta, verify registry installs/upgrades and integrate.
Success requires current owning gates, both distribution verifier modes, the
unsigned production launch verifier and successful actual CI receipts. No Apple,
five-platform acceptance, F3/F4 or real trading. Root fixes named failures; CI
owns routine execution. Resume digest-matching completed steps, report missing
bootstrap dependencies once, and preserve failed evidence. Run affected checks
while editing and owning full gates at integration. Production behavior and
production payment-backed entitlement remain required even when staging passes.
No routine reviewer chains, delegation or per-repair planning. Focused main-thread
Astra escalation is authorized under the contract above. Use CLI/MCP/API before
Chrome and protocol diagnostics before visual DevTools manipulation. Do not
silently expand scope or count partial passes. Finish when the gates pass.

The user cleared the main-thread goal after stopping execution. The text above
is the reconciled replacement objective, ready for an explicit start in that
thread. This reconciliation does not start a goal or resume release operations.

## L1 — Production service, purchasing and staging acceptance

### Named candidate amendment: personal signup authority — 2026-10-03

The actual isolated frozen-binary signup failed after the production callback
with `workos_hub_identity_permission_missing`. Accepted owner sessions are
organization-backed; ordinary personal AuthKit signup has no organization
permissions. Execute this named repair under the replacement goal's runtime
repair/candidate-amendment scope. The original candidate is retained, not edited
or relabeled. A changed client is unqualified until the existing gates qualify
its exact new native/archive/descriptor bytes and dependent captures.

Decision: do not add environment-wide JWT scopes, manufacture permissions,
assign customers to a shared organization, or substitute Connect authentication
for the existing AuthKit public-client flow. Core verifies signature/issuer/
client/subject/time as before. Only absent-organization sessions may defer the
Hub identity permission decision to the real Hub projection; configured
organization bindings and organization permission denials remain enforced.
The projection must match the signed subject and exact `user:{sub}` personal
tenant (or signed organization tenant) before custody, restart or refresh.

Hub owns explicit personal self-service policy after active-client admission.
Use a verified personal-session marker, never a caller-supplied tenant or role.
Permit identity projection, own entitlement checks and mapped-product checkout
only. Preserve product binding, commerce mode, account ownership, idempotency,
administrative denials and payment-backed entitlement. Apply consistently to
Edge's `auth.ts`/`service.ts` and Rust API's verifier/authenticated routes. Do
not grant generic permissions during token verification. Dashboard account
management remains a separately checked own-account route, not an operator grant.

Finite acceptance: signed-token invalid signature/issuer/client/subject/time
denials; absent versus blank/invalid organization discrimination; registered,
inactive, unknown and cross-product client cases; personal own-account success
without role claims; unchanged organization permission denials; no entitlement
grant/operator access/other-account read; unpaid access stays denied. Source
tests do not close real signup, custody, distinct-process restart, refresh,
purchase/webhook/paid-period or Relay acceptance. Rebuild only invalidated
outputs through existing owners and retain old successful/failing evidence.

Source pointers: Core `runtime-rs/src/workos_identity.rs::{finish,
validate_hub_claims,current_identity_and_token}`, `hub_identity::project_session`;
Hub `apps/hub-edge/src/{auth,service,contracts}.ts`,
`apps/hub-api/src/lib.rs::{WorkosTokenVerifier,authenticated,require_permission}`.
No provider permission/configuration mutation is part of this repair.

Outcome: paying customers can use production Relay, ordinary customers cannot
use staging. Owners: private Relay runtime/IaC/xtask/profile; existing Hub identity,
catalog/billing/entitlement; Core profile/onboarding. Product owns aggregation.
No Hub rewrite unless a specific failing contract proves it necessary.

Freeze one finite acceptance manifest: exact deployments/profile, named routes,
owner/controlled nonowner, approved live product/price and expected effects.
Reuse existing journal/OAuth drivers and M4/M5/M6 runners where they actually
cover selected production resources. Old test receipts are not production proof.
Fill uncovered cases only; no bespoke framework per check.

The coordinator's `just f2-launch-inputs-preflight ABSOLUTE_MANIFEST
ABSOLUTE_INPUT_ROOT ABSOLUTE_NEW_REPORT` checks the closed manifest, fixed
archive references and all 25 proof-family paths/digests. It is input-only:
owning/provider/production acceptance remains `not_evaluated` and every launch
verdict remains false. It is not an implementation of `f2 launch-verify` and
does not prove source integration or semantic receipt validity. New owning
verifier pins require explicit contract review, never relabeling old receipts.

The source audit fixes four remaining proof ownership boundaries. Implement
packaged production onboarding first, using Core's existing
`runtime-rs/src/cli/browser_onboarding.rs` and
`runtime-rs/src/service/plugin_oauth.rs`, plus Relay's existing broker handoff.
Capture actual signup/login, identity continuity, Paper callback, local custody
before ACK and restart readback; distinguish replay/wrong-owner/canceled denial
tests from deployed positive evidence. Control the browser handoff through
Chrome. Do not build another identity/OAuth implementation or use fixture-only
stdio tests as production proof.

The complementary Core source fault owner is `cargo xtask oauth-fault-proof
--descriptor ABSOLUTE_DESCRIPTOR --evidence ABSOLUTE_NEW_DIRECTORY`. It requires
the unchanged frozen Mac descriptor, archives its exact Core revision and lock,
and overlays only two `cfg(test)` boundary hooks plus isolated test support.
Removing the exact hooks must reconstruct the frozen OAuth file byte-for-byte.
It compiles that archived tree, not the current checkout's later repairs, then
runs five exact named behavioral tests with nonzero execution checks and raw
logs. Real local credential/SQLite adapters are independently reopened before
the scripted sink consumes ACK. Failures in custody/receipt writes must prevent
ACK; consumed ACK with a lost response must recover in a distinct test process
without fetching status or rewriting credentials, revoke its verifier, and
replay without transport. Credential revision drift must block recovery.
The archived source, dependency lock, overlay/test source, test executable,
toolchain and logs are hash-bound. This is source-only fault evidence, explicitly
not WorkOS/HTTP/provider authentication, frozen-binary fault execution, fresh
signup, full onboarding or release readiness. It complements deployed positive
proofs; it cannot silently replace a requirement for deployed observations.

The fourth owner is staging positive OAuth, distinct from production packaged
onboarding and from existing staging denial/revocation proofs. Private Relay's
`xtask/src/oauth_positive.rs` now implements begin/complete/paired verification
over the existing deployed broker service; E4 accepts its custom/direct pair
only after that owning gate passes. It requires real provider callbacks and
Paper account reads, identical ready retries, wrong verifier/instance/nonowner
denials, unchanged credentials after denials, ACK/retry and consumed status.
Local discrimination tests are not deployed positives. Browser callback
interception must be verified before a short-lived capture begins; codes go
process-to-process, never copied by the user or saved in exported evidence.
Preserve the current production consent independently. No shipping binary,
identity service, entitlement or broker order change is implied.

The final coordinator must consume semantic raw inputs, not the 25 report
hashes alone: closed configs/descriptor, paired leaves and deployment chains.
Include staging journal as a mandatory supporting input to E4. Keep accepted
staging leaves at their own `ac3f4ba` validator, production leaves at their
reviewed owner, and add reviewed pins for new owners rather than silently
reinterpreting old proofs. Existing 24-hour checks remain enforced. Implement
the missing full onboarding/commerce owners before wiring the eleven-job chain;
do not turn incomplete constituent jobs into the final acceptance definition.

The additive coordinator login owner is `just f2-packaged-login-capture CONFIG
DESCRIPTOR ABSOLUTE_NEW_DIRECTORY`, with matching `f2-packaged-login-verify`.
Its closed config selects an isolated installation and expected hashed owner.
It requires the exact qualified Mac bytes/profile/installed record, the existing
Bitwarden custody choice, and an actual initially unauthenticated packaged MCP.
It initiates the normal loopback companion; its private handoff must be opened
in Chrome and the normal sign-in button used. It never calls the default-browser
login convenience or injects credentials. A twenty-minute capture bound requires
the provider/Hub transition and same live identity in a distinct packaged process.
Redacted per-phase observations survive partial failure; handoff capabilities
are not exportable evidence. Unit tests discriminate malformed receipts only.
Terminal bootstrap failures must stop immediately with a bounded machine code,
not spend the capture deadline silently waiting for an impossible transition.
Actual isolated production signup on 2026-10-03 completed provider email
verification and delivered the loopback response, but Core denied it with
`workos_hub_identity_permission_missing`. The new owning capture reports that
failure directly. Existing organization-backed sessions are not evidence that
personal signup works. Resolve the production native-client/personal-user
authority binding without bypassing Core/Hub checks or altering frozen shipping
bytes silently; a positive login or full-signup verdict remains unproven.
This is login/restart evidence, explicitly not fresh signup, broker OAuth, full
onboarding, CI provenance or release readiness. Those remaining proof families
are still mandatory. The preceding packaged-connection producer stays unchanged.
Fresh signup must include actual provider creation facts bound to this journey;
an old user or a new local session is not signup. WorkOS's supported user API
exposes creation/sign-in timestamps; a production-authorized reader is required.
The saved WorkOS CLI staging/sandbox API key cannot read the verified production
user (`entity_not_found` observed), and must not be used as production evidence.

The coordinator now provides `just f2-packaged-connection-probe CONFIG
DESCRIPTOR NEW_PROOF` / `-verify` for actual frozen packaged MCP account/status
readback and a second process restart. Config pins the isolated installation,
onboarding reference and hashed expected owner. Inputs, real production profile,
native binary and installed record are verified; response projection excludes
credentials, personal fields and browser capabilities. This proves only a live
same-owner connection/ACK/account/Warden readback, never fresh signup, login
transition or the complete OAuth fault/denial contract. All broader readiness
flags remain false; it cannot substitute for `production-onboarding` in L4.

Local `-verify` keeps installation revalidation. The separate coordinator
`f2-packaged-connection-export` emits a closed hash-only reference only after
that check; `f2-packaged-connection-portable-verify` consumes reference,
descriptor and proof without local installation/vault access. Exact artifact
provenance and fresh provider acceptance remain separate CI obligations; neither
this export nor the portable result upgrades the partial scope or establishes
complete onboarding. Do not upload an operator installation or session to CI.

The separate `-prepare` command requires a verified existing Hub session and
an authoritatively ended local attempt; it reuses the broker instance under a
deterministic new setup key and holds only its own product companion for 20
minutes. A new private handoff validates the exact returned loopback URL/id.
No grant, orders, activation or agent runner is created. Chrome still completes
the browser flow, subject to existing action-time consent boundaries. Inspect
live handles and actual expiry before renewal; never replace an active flow.

Subscription lifecycle belongs to current Hub commerce source
`apps/hub-api/src/lib.rs` and the deployed Stripe webhook ingress, with a
separate private Relay capture around the existing paid-period leaf. Require
actual event delivery/persistence, canonical portal return, owner isolation,
period-end and effective cancellation, expiry and stale/replayed denial.
Provider event existence alone does not prove webhook delivery. Test clocks or
seeded state cannot prove natural production expiry. Identify the deployed Hub
revision before binding proof; the older SDK checkout is not the commerce owner.

Deployed policy parity belongs to coordinator `tools/f2-github/src/ci_inventory.rs`
and reviewed Relay/Hub IaC: execution/CI role trust, inline and attached policy
versions/boundaries, filtered configured secret references and datastore/KMS
conditions, plus actual own-role success/opposite-environment denial. Existing
function metadata is insufficient. Resolve the existing CI-role source before
asserting parity; do not broaden readback privileges for capture convenience or
read secret values. These three proofs remain separate mandatory obligations.

Relay now exposes the bounded offline assessor `just
relay-environment-iam-plan-parity PRIVATE_INPUT_DIRECTORY CANDIDATE_DESCRIPTOR
NEW_PRIVATE_REPORT` (`xtask/src/relay_e4/iam_plans.rs`). It consumes twelve actual
provider-refreshed plans and filtered function metadata, rejects IAM drift and
missing/additional policies, and checks nineteen source-owned roles (including
Hub backup) and twelve Lambda configurations. This is only plan/inventory
parity: provider freshness, reviewed source provenance and actual
role-admission/separation remain unverified. Managed-policy documents must match
reviewed defaults exactly: Logs `v1`, Backup `v30` plus its canonical SHA-256.
This retains the existing AWS-owned Backup service policy on the backup role
only; it grants no new permissions and rejects upstream document/version drift.
The strengthened receipt is `tradeassembly.f2.iam-plan-parity/v2`. It is
not the full `deployed-iam-secret-separation` release proof and cannot satisfy
L4 on its own. Preserve failed observations; do not apply a broad plan just to
make this assessor green.

Actual code-role admission can now be revalidated by coordinator `just
f2-ci-code-authority-verify ENVIRONMENT RUN_ID ABSOLUTE_NEW_REPORT`. It binds
retained real CI run/attempt/ordered steps to compiled approved `5df29c` wrapper
and reusable workflow bytes, and reads both exact deployed OIDC trusts afresh.
Skipped/missing denial, foreign source/environment/attempt, extra trust and
wildcard subjects reject. Reusing the passed production job does not repeat its
deployment. The source-bound private result proves CI code-role admission and
current trust only; all remaining readback/execution/secret/storage and full
release obligations remain mandatory. Management credentials only read trust,
never stand in for the admitted CI execution.

The full IAM-family composition now has an owning coordinator command:
`just f2-iam-separation-verify ABSOLUTE_CONFIG ABSOLUTE_CANDIDATE
ABSOLUTE_NEW_REPORT`. `tools/f2-github/src/iam_separation.rs` freezes the approved
source/live receipts (`cf3eb789...`/`cef6f500...`) and candidate `51da2114...`,
retains the original source pins, and requires the live capture to remain within
24 hours. It independently revalidates both actual readback jobs/attempt and
unexpired provider ZIP/member digests at `4ca4737...`, then reuses the existing
fresh code-role admission/current-trust checker for both environments at
`5df29c88...`. Config cannot select authority, commands or receipt digests.
Processes, input bytes, capture interval and retries are bounded; changed inputs,
wrong source/attempt/role, skipped or missing denial, expired/replaced artifacts
and hand-filled success flags cannot pass. Private new-only partial receipts
remain diagnostic on failure; full release and deployment flags remain false.
Actual execution accepted four constituents and correctly rejected historical
staging run `37106818511` with `ci_authority_run_binding_invalid`. This is an
implemented gate and an actual negative observation, **not** full IAM acceptance.
The missing reviewed staging admission remains required after controller freeze;
do not repeat deployments or change trust merely to make this result green.
The owning README defines the fixed input and receipt contract. A replacement
live capture requires an explicit reviewed digest amendment, not a caller flag.

Controller closure decision (2026-10-03): use the two existing coordinator
branches, not an advancing self-pinned controller. Freeze code controller C on
`codex/f2-codebuild-distribution` after its owning gates; keep the environment
wrappers, IAM subject and original inventory bindings on that branch. Complete
final release source R on `codex/f2-release-execution-reset`, preserving its
unfinished edits. Amend only the release workflow branch, release-chain branch
and npm publication's exact ref/workflow-ref checks. R may compile already-known
C and approved evidence digests; its own source comes from the immutable actual
GitHub run context. Do not globally rename branches, force-reset the controller,
assume SHA/tag dispatch, or repeatedly repin authority for unrelated tooling.

Replace the existing controller trust once using the owning saved-plan/digest
procedure. Require exactly four trust-only updates (two code and two readback
roles), with an exact old-controller-to-C substitution and every other policy,
subject, principal, action, resource, boundary and session limit unchanged.
Preserve the original source/live receipts as historical baseline; they cannot
describe current trusts after this replacement. An owning bounded successor
assessor must verify that exact substitution against the original source-bound
plan inputs, then re-run full fresh live parity. Only afterward can both C
wrappers run `operation=plan` with rollback qualification false to establish
current admissions/denials without deployments. Preserve prior deployment
evidence at its actual producer source, never relabel it as a C admission.

Final jobs must not receive operator credentials or new IAM read privileges.
The existing management read route produces the fresh successor proof before
R; R pins its exact digest. Add a closed offline consumption adapter that
revalidates constituent hashes, source bindings, the existing 24-hour freshness
bound and actual GitHub admissions/artifacts, and emits same-run/R evidence.
Copying an accepted receipt or manifest flags is insufficient. The successor
assessor now exists at Relay `4af40dfa...` in
`xtask/src/relay_e4/iam_successor.rs`, with owning
`relay-environment-iam-controller-{plan-verify,live-parity}` commands. It fixes
C to `9bc6d450...`, reuses original source/lock and full live checks, and rejects
any changed permission, resource/module/output envelope, extra/stale/wildcard
authority, unknown value, changed original input or unreviewed controller.
Four targeted fault tests and strict Clippy pass. Actual plan-only proof accepts
saved plan `b6012617...` with exactly four trust replacements; the original
unreconciled Hub-plan input was correctly rejected, not applied. Use the original
hash-bound reconciled input directory, preserving all historical proof bytes.
Fresh live successor parity passed 130 actual metadata reads; C staging
`37121530138` and production Gateway `37121751022` plan-only admissions passed
without another deployment. Release tooling R `abb592a18d3687cfe296433bf8ff9ccb2e2d3162`
now consumes exact source `cf3eb789...` and successor `061f001a...`, checks all
source/input/plan/lock/verifier bindings and 24-hour freshness, then re-reads
actual GitHub admissions, workflow bytes, original inventory/artifact facts and
retained ZIP/member hashes. No CI GetRole privilege or operator credentials
were added. The operator-only authority command retains fresh AWS reads.

Actual local full IAM composition passed (`98215`, terminal 0), private receipt
`f2-release-execution-reset/.operator/iam-successor-composition-20261003/iam-separation-final.json`,
SHA-256 `1b8667c4598cb8187a4d58b0dc0712418b6f2c48e2dfebb21906b60563f4d31d`.
It proves the IAM family only: `releaseReady=false`, deployment acceptance is
not evaluated and `executionContext.sameRunReleaseSourceVerified=false`.
The adapter also implements actual R run/attempt/source/workflow checks for a
CI invocation; that same-run invocation is not yet proven. Targeted authority
and composition fault tests, strict Clippy, 85 provider-tool tests, actionlint
and harness checks pass (one unrelated opt-in retained-publication test ignored).
Coordinator owning `just verify` passed on committed R (`1116`, terminal 0),
including its required harness, locked dependencies, strict Clippy/tests and
product contracts. Full-family source integration and the actual same-run CI
receipt remain required, as do production onboarding/commerce/effects/data-preserving rollback,
all eleven successful jobs, exact npm publication and real registry journeys.
Full L1–L4 scope is unchanged; an IAM leaf or local composition is not launch
acceptance. Original dirty release-reset edits were preserved in stash
`f6d688ac8e4fedc2eeef02cafa18576ecb69e0ab` and reconciled as newer upstream
supersets; frozen controller C and accepted native/package bytes were unchanged.

The finite CI transport is now implemented on R: `just f2-iam-inputs-export`
exports only the five exact projected inputs and admission run IDs to a closed,
bounded credential-free bundle; `just f2-iam-bundle-verify` invokes the existing
owning composition from private scratch. The private immutable cache asset is
`iam-inputs-20261003.json`, SHA-256 `c80314c24c6476d742cf512ff2aa61d6922a77dd0de5946d02b877cac6ba9aa0`.
The explicit `iam-separation` CI selector has only contents/actions read
authority, no AWS/OIDC/vault/operator credential transport. Actual same-run
execution remains required; this constituent is not the eleven-job chain.

Reviewed IaC provenance now uses private Relay `just
relay-environment-iam-source-parity SAVED_PLAN_ROOT PARITY_INPUTS HUB_SOURCE
CANDIDATE NEW_REPORT` (`xtask/src/relay_e4/iam_source.rs`). It compares each of
the twelve retained saved-plan archives' complete embedded `.tf` files/modules
and provider locks against compiled reviewed Relay `1ca5fb` and Hub `048c107`
Git bytes, and requires actual `tofu show -json` equality with the parity input.
Use only the corrected no-op Hub plan; no new plan/apply occurs. Input/candidate
hashes are rechecked, subprocess output/time are bounded, and receipts exclude
raw plan/state values. This closes saved-plan source provenance and reruns the
existing nineteen-role/twelve-function inventory parity. It does not substitute
for fresh provider reads, actual admission or full secret/storage separation.
Accepted older receipts retain their original owning source/verifier pins.

Fresh provider parity uses private Relay `just
relay-environment-iam-live-parity PARITY_INPUTS ACCEPTED_SOURCE_PROOF CANDIDATE
NEW_REPORT` (`xtask/src/relay_e4/iam_live.rs`). Its fixed accepted source receipt
binds the twelve saved plans and all original inputs. Read-only AWS CLI checks
compare all nineteen roles' trust, session limits, boundaries, inline policy
inventories/documents and attachments; twelve healthy functions' roles and
projected secret/storage references; exact managed-policy defaults; protected
secret metadata; and both isolated Relay clusters and encrypted, versioned,
Object-Lock-enabled, private archive buckets. Raw function environments stay
inside the subprocess/verifier and are never retained or logged. Secret values
are never retrieved. Calls, output and runtime are bounded; input hashes are
rechecked after capture. The private receipt contains only projected metadata
digests and verdicts. A passing live-parity constituent is not actual CI
admission, the full IAM-separation family, or release acceptance: those flags
remain false until the approved proof composition verifies them.
IAM document comparison normalizes only legal single-string versus one-string
array grammar positions; it never collapses arbitrary arrays, deduplicates
values, or tolerates extra principals/actions/resources/conditions. Negative
tests preserve rejection of each authority change.
Secrets Manager's documented omitted `KmsKeyId` resolves to
`alias/aws/secretsmanager` and requires actual enabled AWS-managed encryption
key metadata; an omitted field alone is never encryption acceptance.

1. Actual packaged production Hub/Alpaca Paper signup/connect/read/status, without
   manual keys, forced diagnostic tenant or staging URLs. One initial browser
   consent is allowed; stored test grants support later automated smoke tests.
2. Approved production Hub offer, checkout/return and subscription management;
   signed webhook/receipt processing, purchased entitlement, allowance checks,
   cancellation/expiry and denied access. A test grant does not prove live billing.
   Require a genuine provider-confirmed production purchase/entitlement record,
   existing or from an explicitly authorized operator purchase; no arbitrary
   charge or fabricated payment. Reuse Hub; price is not selected by this plan.
   The private owning read-only leaf is now
   `just relay-production-paid-period-probe CONFIG CANDIDATE-DESCRIPTOR NEW-PROOF`
   with matching `-verify` and `-test` commands. Ten actual company Stripe CLI
   reads plus two actual Hub reads must bind the same live checkout/customer/
   subscription/Price, positive-value invoice, payment event, collected invoice
   payment, succeeded PaymentIntent and captured/unrefunded/undisputed charge to
   Hub's exact payment-backed active period. Closed source/input-bound private
   receipts retain partial failures and explicitly keep commercial-lifecycle and
   full-release false. No purchases or credential/account changes are made by
   this command. Portal/return, actual signed-webhook delivery, cancellation,
   expiry and denials remain required separate evidence. Fixture tests are not
   actual production purchase proof. The owning gateway README defines safe
   credential input, the twelve-read inventory and bounded capture/retry contract.
3. Production enrollment/journal ingest/replay/query/export, entitlement and
   workspace/environment denials. Watch/Reach prove production owner/node
   authority, not an acceptance shared-secret bypass. Isolated diagnostic state;
   no broker orders, customer-account changes or new trading activation.
   The private owning command is now `just relay-production-workspace-journal-probe`
   with matching `-verify` and `-test` commands. Bind the exact qualified beta.3
   Mac descriptor, real production identities/live decisions, the original fixed
   diagnostic request and active Hub paid period. Require 46 actual gateway and
   four Hub observations, both origins, duplicate/conflict recovery, single-event
   replay/export and unchanged quotas after denials. Keep accepted staging sources
   untouched. Closed source-bound evidence stays purchase/full-release false;
   fixture unit tests are not hosted proof. The owning gateway README specifies
   config, safe credential input, 300-second bound and same-request recovery.
   Reach/Watch reuse their existing private E4 command interfaces, extended to
   closed production targets and production heartbeat signing. Production
   admission must persist actual Hub identity/live payment-backed period, cover
   the whole observation interval and bind the hosted tenant/subject principal
   to the workspace. Existing Inspect, duplicate/recovery, delivery/issued-ACK
   and revoked-denial cases remain required on both origins; no broker orders.
   Local fixture tests establish verifier discrimination only, not positive
   production observations. Accepted staging node/Watch receipts retain their
   original owning source/verifier pin `ac3f4ba176a3bf3dd1f070c94e5b1a4a0f0bda78`;
   final aggregation must validate each proof family through its approved pin,
   never relabel old source hashes or require new observations for unchanged
   valid evidence. The retained node receipt is `e4-node-effects-accepted.json`,
   not the stale similarly named `e4-node-effects-current.json`.
4. Every staging route classified protected/explicit exception; owner, verified
   nonowner and anonymous coverage on custom/direct surfaces. Callback tamper,
   expiry, environment and revocation before effects. Reuse valid current proof.
5. Deployment, storage, IAM, secrets and rollback match reviewed IaC. Rollback
   preserves data and cannot reopen staging.

Existing commands: private `just relay-environment-journal-verify`,
`just relay-environment-oauth-denial-verify`, and
`just relay-environment-oauth-revocation-verify` with exact config/candidate/
evidence arguments; owning policy/Watch/Reach/Hub tests. Implement the missing
full E4 adapter over those proofs, not hand-written success assertions.
October 2 continuation: private Relay now has the initial Rust coverage adapter
`just relay-environment-e4-assess JOURNAL_CONFIG OAUTH_CONFIG CANDIDATE
JOURNAL_PROOF OAUTH_DENIAL_PROOF REVOCATION_PROOF NEW_REPORT`. Four unit tests
and strict xtask Clippy pass. It inventories 35 current routes, reuses owning
leaf validators and reports missing live coverage with a nonzero exit. Actual
assessment against the current storefront package candidate rejects the three
existing receipts, which bind different candidate bytes; report SHA-256
`873a78a4bee7246fa8e9d37099d825a0d4b333542e76e2738982e7e6503cbff3`.
This is a real rejection, not full E4 proof or a replacement of prior accepted
evidence. Adapter completion still requires actual missing route/deployment/
rollback proofs. Keep this same adapter and contract; no new parallel verifier.
Actual staging routing checks subsequently found direct Gateway/Reach/Watch
admission returning 401 while custom-domain equivalents returned 404. Private
Relay `f92b53e` fixes this finite edge gap with exact 35-route classification,
distinct service origins and source-derived parity tests; production Connect
remains OAuth-only. Final Relay `e40ef23` passed `just verify` (Core composition
smoke explicitly skipped for unset local path) and the owning staging deployment.
Deployed Worker `3410cd51-3eb0-430b-be66-625f8fbb1ff6` returns 401 for Gateway
access, Reach scope, Watch node-status and journal query on both custom/direct
origins; invalid callback state returns 400 on both. Public pages return 200;
misverbs/unknown paths and production non-OAuth access remain 404. Private
diagnostic report SHA-256
`ecab04b9b816c80fabed55fd16f65a83bcd7ec8859a61cb8885d119ae667cf78`.
This closes the finite routing repair, not full E4 actor/deployment/rollback proof.
Production follow-through, October 3: private Relay
`ac3f4ba176a3bf3dd1f070c94e5b1a4a0f0bda78` enables the same exact service
routes at `connect.tradeassembly.ai`, with separately pinned production Gateway,
Reach, Watch and OAuth origins and the existing Worker account. The regression
test first failed on the missing flag; all seven edge tests and clean-source
`just verify` pass. Optional Core composition smoke was explicitly skipped for
an unset local Core path. Reviewed production-only Wrangler deployment through
`just edge-deploy-production` activated Worker
`6e694083-84bf-40f4-bf04-413496226751`; previous `5503bc2d…` remains retained.
Actual provider readback verifies all six bindings and unchanged staging Worker
`3410cd51…`. All 35 custom/direct route pairs match; 35 misverbs and 35 extra
paths return 404 (140 observations total). HTTP 400/422 validation responses
are not authenticated admission proof. Private evidence bundle
`edge-routing-repair-20261003/evidence.json` SHA-256
`37c50bdb2b8b9d16681d04f406ce6f5816403fe10596c6f056023337e0c6f155`
keeps commercial/release readiness false. Staging configuration, shipping edge
handler source, AWS backends, secrets, native packages and prior actor proof
inputs remain unchanged. This operator repair is not the required full CI chain,
positive OAuth, production purchase/lifecycle, Watch delivery/ACK or Reach proof.
Current continuation: Relay `7a37a35` adds the finite 118-observation live route
admission probe and exact-set verifier; nine E4 tests and strict xtask Clippy pass.
Fresh current-candidate journal replay/query/export and OAuth denial/real-expiry
proofs pass their owning validators. Journal replay does not establish fresh
paid admission. The route probe preserved a real owner-console 503 failure:
deployed Hub returned a legacy entitlement without identity/product/mode or a
payment-backed period. Relay's strict admission contract remains unchanged.
Existing paid-period implementation was merged into Hub's deployment branch as
`048c107`; formatting, strict workspace Clippy and workspace tests pass. Guarded
AWS deployment completed with zero additions/deletions/replacements; Hub API
alias is version 45 and additive DSQL migration passed. Live Hub returns all 12
required fields with the existing payment-backed test period. Fresh unique
journal admission, duplicate replay, query/export and denials pass the owning
validator. Relay `dc69f40` corrects the probe's enrollment schema and exact 403
nonowner workspace denial expectations; nine tests and strict xtask Clippy pass.
The actual 118-route admission probe and separate offline validator now pass;
the clean-tree full Relay integration gate subsequently passed for `2f6f715`,
and that scoped branch is pushed. The initially skipped Core composition smoke
also passed separately through the owning `just dependency-smoke`, with the
exact pinned Core `a16d4a769258fb7354d9102017533850020f8f2c` in a disposable
detached checkout. It proves dependency/profile/source bindings, required files,
architecture and verify dry-run, not a replacement native qualification.
Composition log SHA-256
`069f7046978b315da25fea311f00d90d9047c2bb251cc35ba39df47d8bff70fe`.
Frozen shipping bytes remain unchanged. Current-candidate OAuth
revocation now also passes: both callbacks denied before durable changes and
original admission restored. Relay `2f6f715` counts exact workspace 403 denials;
ten targeted tests/strict xtask Clippy pass. The actual aggregate accepts all four
current leaves but still rejects readiness: 19/35 routes require positive owner,
node or issued-capability evidence, and two deployment/rollback proofs are absent.
Relay `44ff774` now implements the isolated node-effect leaf using the existing
Hub/Reach SDK signing contracts and Inspect-only commands. Both actual staging
surfaces passed 25 cases each; owning merge and offline verification passed.
Proof SHA-256 `47e81a2e165c197fc97a64dff16eccf487cc9183c77963fd3df7cad60e014942`.
Durable pairing, enqueue/idempotency conflicts, signed claim/accept/finish,
duplicate and ambiguous-outcome recovery, invalid/stale/replayed authority and
revocation cleanup are verified. Failed probes remain diagnostic only; the
owning recovery tool durably revoked the isolated node whose cleanup was
throttled. The actual cause was API Gateway's five-request-per-second limit;
requests are now paced below it without changing infrastructure or acceptance.
Operator browser authentication was restored through the packaged MCP flow.
The aggregate accepts five leaves, covers 24/35 routes and exits 1 correctly:
11 routes and both deployment/rollback proofs remain unproven. Report SHA-256
`db09d8e896d251b2f228a18e64f8fdc9e69e6cd0d2845eec2700074a82063dff`.
The new clean-tree Relay `just verify` integration gate passed (terminal 34978,
exit 0); the previously accepted pinned Core composition smoke is retained.
Next: positive workspace enrollment, six Watch routes and four OAuth routes.
Watch must use its actual
signed heartbeat and issued ACK capability, not inferred tokens or synthetic
successful delivery. Existing handler source owns the contract; preserve native
candidate bytes. Partial failures remain diagnostic only; no actual broker orders.
ACK/positive callback effects, deployment/rollback and commerce proof remain open.
Historical Watch repair checkpoint: private Relay's isolated live probe established
pairing, subscription, signed heartbeat, durable node readback and real staleness.
Delivery returned 503; actual scoped DSQL readback proves SES delivered once,
but WebPush failed. Cleanup durably revoked the isolated node. The adapter was
incorrectly applying its 256-character subscription-key bound to the longer
issued ACK capability. The regression fails before repair and passes with a
separate 4096-character ACK bound matching the receiver; subscription-key limits
are unchanged. Four adapter tests, strict adapter Clippy, sixteen E4 harness
tests and strict xtask Clippy pass. The staging ARM64 Lambda build passes; its
reviewed plan changes only the Lambda code hash, with no add/delete/config/IAM
or data changes. Relay `ad11765` checkpoints the fix/diagnostic harness locally.
The initial full gate rejected the uncommitted tree and the next attempt hit
disk exhaustion. The clean-space rerun subsequently passed and the narrow
staging deployment completed; see the reconciled stop checkpoint above. The
remaining failure is real browser receipt/ACK, not the already repaired ACK
length bound. Prove actual delivery/issued ACK, duplicate recovery and revoked
denial on both surfaces, then pass the owning offline verifier. Native candidate
bytes remain unchanged. No production mutation or broker orders are part of this repair.
Exit: actual bound observations for every L1 case; deterministic aggregate passes.
Missing approved live plan or billing access is a named dependency, not scope
for redesign. Continue independent candidate work while that dependency resolves.

## L2 — Exact candidate and four native packages

Outcome: one immutable version and four complete platform packages. Existing
Core/Warden/Alpaca workflows and distribution tooling own their outputs;
private Product coordination owns cross-repo inputs/receipts.

Pin source SHAs, locks/toolchains/images, compatible Warden, SDK/plugin/Node and
production-profile bytes. Build only invalidated outputs on native GitHub runners.
Keep the $0 Actions-overage cap; no CodeBuild detour or new runner/visibility
project. Existing native freeze/describe/pack commands bind architecture,
inventory, notices, binary/profile/descriptor/manifest/tarball hashes and provenance.

Mac acceptance: source-free stdio MCP -> real Warden -> credential-free controlled
sink; submit, duplicates/conflicts, ambiguous recovery/reconcile, risk/authority
denials and retained deterministic path; scripts-disabled npm/pnpm; compatible
upgrade/rollback/state preservation, active-rig denial, interruption and sandbox.
Windows/Linux need build/inventory/architecture/license/digest proof only with
experimental disclosure. Run owning gates, Core `just verify`, `cargo xtask
verify` and required archive checks once on stable revisions. No rebuild between
qualification and promotion.

Native assembly implementation checkpoint (October 2): private coordinator
`tools/f2-github stage-native` now stages all four supported native-host layouts
without changing Core or its already-qualified Mac package. It rejects wrong
host/producer pins, modified extracted-output digests, unsafe Node archive paths,
links, duplicates, unsupported inputs and existing destinations. It uses the
official Node 22.23.2 archive checksums, that archive's npm CLI, scripts-disabled
lock installation with an isolated npm configuration/environment, and packaging
assets read by `git show` from the exact producer revision. Production profile
bytes are bound to the explicitly supplied digest. Its receipt is staging only;
Core GitHub provenance verification remains required before accepting inputs,
and Core freeze/describe/pack/qualification remain authoritative afterward.
Seven focused tests, strict Clippy, dependency audit and secret scan pass.
Actual Mac-input staging and Core describe-native-inputs/freeze pass in separate
ignored `target/f2-coordinator-stage-smoke*` paths. Staging receipt SHA-256
`db8437e9cfe0c04142577879a46dd9c3e88dbca66362f514042f62026a7bb84c`;
frozen smoke manifest `435e809658049c4389a1369581caab2c3af61e51cd8beb6ff7d6e69b0dc8853d`.
This is not Windows/Linux execution or qualification evidence. Existing qualified
Mac archive remains `30dcbd465e5964ded291328f3e8bbd92b7cfac8d663abfe01945bd21e8120c9b`.
Private coordinator now composes the captured native Core installer with these
inputs through `package-native`; no producer bytes are rebuilt. Digest-bound
private prerelease cache assets solve cross-repository transport with the
coordinator's existing read-only job token, not a personal/browser/Bitwarden token.
Actual GitHub run `37052707743`, attempt 2, at `c108c20` passed both Linux jobs.
Downloaded x64 artifact ZIP SHA-256
`668443d39c037e6b5710c2b6d9b16609090d3a7f7d2e8810955661e28913d857`;
arm64 `40703709420058a98486738b6f30a79c48a386ff5030d327059d1df8d8218341`.
These are packaged trees and receipts, not npm delivery archives or runtime
acceptance. Windows run `37053299398` passed all 11 orchestration tests but
failed the actual Core extractor with `metadata_replace_failed`. The retained
log is authoritative; Core's Windows private-file writer rejects inherited broad
workspace ACLs. Coordinator `3c68760` allocates and validates only a fresh
owner-only packaging parent; pinned binaries and security gates remain unchanged.
Windows retry `37054136260` passed at `3c68760`. Artifact `11247459470` downloaded
and ZIP SHA-256 independently recomputed:
`d66f2aea8870e37a3474e6ade9ee2dcc721e2b01eb66e1f2bf5ea1150ece3bf0`.
Windows bundle archive SHA-256
`87cda25b4ae6c6a49a9f5da742ebe226c2edf69fb94992daacab760edfb8a3d8`;
descriptor `36c9db674071376fca058525b6b5cfbabdb4bde3b0af11f0d7366c8c7f11b379`.
Delivery checkpoint: coordinator `53c6ef9` is pushed. Its packer uses exact pinned
Node/npm, offline mode, no lifecycle scripts or inherited credentials. All delivered
files receive recursive hashes. The common launcher is the exact qualified Mac
archive (`a5322a786f940171e8034ff6303e1b7ce769f5e20986ffe531a1045fadc129bd`),
after comparing the complete manifest and all other file contents. Only JSON
serialization order and CRLF-only cli/README differences are normalized; added
behavior fails. Sixteen targeted tests, strict Clippy and dependency audit pass.
Actual Linux delivery jobs passed in run `37055677256` at `7537b18`; downloaded
x64 ZIP `a3e06366e317e64bd09a627208eae783257e3687deb6c3fc1654a82ced033fc7`
and arm64 ZIP `596a6c798da5f40f17f8059908483a0ac8f51c2f94f5bab452562f311fb3857f`.
Windows-only run `37056091877` passed at `53c6ef9`; artifact `11248323678` ZIP
`d81c4592d54819227dfdbc9868f9d04f67c4ea8bf07663495465bc638858a720`.
All downloaded ZIP hashes were independently recomputed. No native producer
rebuild or Mac qualification replacement occurred.
`assemble-experimental` builds Core's existing receipt schema from the actual
archives, readbacks, outputs and delivery bytes. The assembled evidence root is
`target/f2-npm-evidence-production-storefront`; Mac receipt is unchanged.
First four-target candidate check failed at Windows `executable_permission_missing`:
the verifier required POSIX execute bits on a foreign Windows payload. Decision:
permission validation must follow the declared target, not the verifier host.
Windows retains regular-file, full inventory, digest, PE machine/architecture,
source and provenance checks; POSIX payloads still require executable bits.
`distribution/src/lib.rs::executable_file` implements that correction with an
explicit negative POSIX/link regression. All 32 distribution tests and strict
distribution Clippy pass (three native process tests remain explicitly skipped
here; their unchanged Mac qualification evidence is retained). Clean `f96371a`
passed `just verify`, explicit `cargo xtask verify`, and the actual four-target
candidate verifier in terminal `76483` (exit 0), and is pushed on the existing
distribution branch. Owning-gate log SHA-256
`0b71b341a0c3e4e287aa443e7878ea2f0f3e51db1767f0989695b404acd1f28b`;
candidate log `fccfd64682b7fa5d8251da91b6f288f67afe5ac15ac3ea0ed7a7121d13510c78`.
The candidate is qualified under this policy with Mac ARM64 as the only qualified
target; Windows/Linux remain experimental and `releaseReady` remains false.
Next: CI-owned aggregate/publication steps. Private evidence transport must
preserve exact receipt-referenced bytes, exclude unrelated local state, check
all digests and the externally pinned archive hash, and rerun the existing
candidate verifier on extracted evidence. It must not synthesize test results,
rerun native producer builds, or replace completed Mac qualification.
L1 commercial proof
and full L3/L4 remain open.
The coordinator's whole-family `./repos check` still fails
because its isolated checkout has no `Hub` child; no locks were refreshed and
no family integration/merge readiness is claimed.

Exit: `cargo xtask distribution-verify --candidate --evidence ABSOLUTE_DIR`
returns 0 under the four-target policy with correct production profile/L1 bindings.

## L3 — CI deployment, npm publication and registry acceptance

Outcome: one authorized release input drives the existing tools without an agent
loop. Private Product owns `.github/workflows/f2-release.yml` and Rust/owning
`./sdlc` orchestration. Core/Warden/Alpaca expose pinned reusable native jobs or
their existing dispatch/artifact interface; Relay owns service deployment recipes.
Public Core must not fetch private source or hold hosted deployment credentials.

The existing native/package and candidate jobs are implemented and accepted;
extend them. A complete release run is still missing. Required remaining jobs:
preflight immutable inputs and bootstrap availability; invoke reviewed owning
staging deploy/smoke commands; invoke reviewed production plan/deploy/smoke;
verify actual production commercial/service receipts; run the candidate gate;
publish the exact npm tarballs; capture/compare registry bytes and run the Mac
registry install/upgrade/rollback checks; run the unsigned launch aggregate.
Use fixed reviewed commands, not commands supplied by an untrusted manifest.
The manifest selects immutable revisions/artifacts/environments/receipt inputs.
Privileged jobs execute only on approved protected release inputs; ordinary
pushes run unprivileged checks. Existing concurrency/timeouts remain in force;
new deploy/smoke jobs have bounded timeouts and preserve logs on failure.

October 2 implementation checkpoint: private coordinator `eb78501` adds the
finite `tools/f2-github/release-inputs.json` and owning Rust
`preflight-release-inputs` command. The existing workflow requires this
unprivileged input-consistency job before candidate/native packaging. It binds
frozen asset hashes, verifier/archive, package names, support levels and required
downstream gates; no manifest-provided executable commands are accepted.
All 25 standalone tool tests, strict Clippy, fmt, actionlint and agent-harness
checks pass locally. Actual CLI report is accepted with `releaseReady=false`;
its digest/path are in the existing coordinator checkpoint. Commit is pushed;
GitHub run `37089479807` preflight job `111106544760` passed; its downloaded
report matches the local report byte-for-byte. Candidate job `111106683782`
passed and run `37089479807` completed successfully. Coordinator `4ca4737`
then added opt-in metadata-only OIDC inventory jobs. Actual run `37090769349`
passed both staging and production: STS identity, four healthy own functions,
and actual denial of cross-environment reads. Relay owning IaC `10283c1` is
pushed and its full `just verify` passed. Exact receipt paths/hashes are in
the existing coordinator checkpoint. This is deployment-inventory-only,
`releaseReady=false`, not privileged deployment/rollback or launch acceptance.
This is not bootstrap availability or a full release pipeline: all required
service/deploy/publish/registry/launch jobs and actual receipts remain mandatory.

Private Relay `3dba197` implements reviewed code-only plan/apply/rollback through
`just relay-code-*`, with fsynced intent, exclusive receipt lock, one revision-
fenced submission, bounded ambiguous-outcome recovery and exact previous ZIP.
Ten targeted tests, strict Clippy, fmt and the standalone advisory audit pass.
Actual staging Watch reuse accepted unchanged qualified bytes without an update;
the current checkpoint records exact plan/receipt digests. The same receipt replay
also passed unchanged. It is not changed-code deployment, rollback or durable-data
preservation proof. The owning gate stopped on expired AWS auth; login was repaired
and replacement `34859` passed (exit 0), with optional Core composition explicitly
skipped. Relay `0190fa0` adds a pinned read-only private operator-build workflow;
actionlint passes and both scoped commits are pushed. Actual GitHub run
`37093153663` and driver job `111117580008` passed all steps on ubuntu-22.04.
Actual run/job/artifact readbacks and downloaded ZIP/member/driver digests agree;
the exact private archive is retained in the coordinator cache for CI consumption.
The current checkpoint records all bindings. This proves operator build/transport,
not deployment authority, service acceptance, rollback or release readiness.
Runtime/proof dependencies and all native qualifications are unchanged.
Privileged CI will pin immutable repo IDs, context, workflow_ref, reviewed
workflow_sha and dispatch event in its OIDC subject, with STS audience and
finite own-environment actions. Prepare matching IAM before changing subject
format. At that operator-build checkpoint, no write role, subject customization
or CI deployment had been applied. The current execution section records later
reviewed authority and actual acceptance; deployment/rollback proof is separate.

Completion evidence for this implementation task is an actual CI run that
executes every required job with real inputs, a failed-job rerun that preserves
successful artifact bindings, and deterministic rejection of absent/stale/
tampered production and registry evidence. A workflow file, stub, mock runner,
all-skipped run or successful candidate-only job cannot satisfy L3/L4.

Pipeline:
`preflight -> build/reuse -> stage/package -> native qualify -> staging smoke ->
reviewed production plan/deploy -> production smoke -> candidate gate -> npm beta
publish -> registry bytes/install/upgrade checks -> aggregate launch gate`.

Registry delivery now has an owning implementation:
`cargo xtask distribution-qualify-registry --evidence ABSOLUTE_DIR
--baseline-package ABSOLUTE_FROZEN_MAC_PACKAGE --source ABSOLUTE_CORE_CHECKOUT`.
After `distribution-capture-registry`, it verifies the unchanged candidate and
all five captured registry tarballs before running actual Mac ARM64 npm/pnpm
installs with lifecycle scripts disabled and isolated credential-free config/
caches. It checks installed native/launcher bytes, the stable source-free rig
after removing the package-manager client, and the existing real frozen-baseline
upgrade/rollback/state-preservation test against the captured registry package.
A separate source/input/log-bound `registry/qualification.json` is written only
after both exact opt-in tests pass; it never edits accepted native receipts.
Failed attempt logs remain diagnostic, completed valid receipts may be reused,
and a new check source or changed registry/native inputs invalidates reuse.
The schema-v2 published gate requires this receipt; arbitrary exit-zero log
attachments do not establish registry acceptance. Schema-v1 remains historical.
This is implementation, not a passing registry journey: actual publication and
registry execution are unproven. The earlier 72-hour recovery hold is historical,
not a freshly verified publishing blocker; passkey-backed CLI login now passes.
The actual gated publisher must establish publication acceptance without a
speculative publish or weakened production/CI predecessors.

The final-job restoration component now exists in coordinator
`tools/f2-github/src/registry_restore.rs`, exposed through
`just f2-release-registry-restore RUN SOURCE ABSOLUTE_CORE_VERIFIER
ABSOLUTE_CANDIDATE_EVIDENCE ABSOLUTE_PRELAUNCH_EVIDENCE ABSOLUTE_NEW_OUTPUT`.
It requires fresh actual ten-predecessor/current-final-step provider acceptance,
clean pinned sources, snapshots the small proof ZIP against its provider digest,
downloads the unique unexpired same-run/source large registry-byte artifact,
checks exact inventories, all five frozen delivery hashes and the registry
manifest, then invokes the unchanged owning Core published gate. Original native
evidence is re-hashed and never overwritten. Duplicate central-directory names
are rejected even when the ZIP library normalizes them away. Size/time, links,
traversal, stale/foreign source, partial input and overwrite guards are enforced.
It keeps readiness false and proves no production commerce. Transport fixture
tests and actual candidate-only refusal are not positive registry proof.
Positive restoration/final-job wiring still require the real release run and
published registry packages; no placeholder inputs or gate bypass.

The coordinator now implements the missing exact-byte publication component:
`just f2-release-publish RUN SOURCE ABSOLUTE_CORE_VERIFIER
ABSOLUTE_CANDIDATE_EVIDENCE ABSOLUTE_NEW_OUTPUT_DIR`. It requires the actual
owning CI publisher step and the eight successful predecessor gates, their
same-run source-bound provider archives and the clean pinned Core candidate
verifier. A distinct prepublish readback is never full-chain acceptance. All
five accepted beta.3 hashes are pinned; bytes are privately snapshotted without
repacking, native packages precede the launcher, and all existing versions are
checked for conflicts before writes. One fsynced intent precedes each single
submission. Exact observed metadata/tag/downloaded registry bytes settle success
or ambiguous recovery; unresolved outcomes stop, while reruns reuse exact
published versions. No overwrite, unpublish, automatic tag repair or placeholder
package is supported. Existing scoped npm authorization and pinned Node/npm are
required; no account setup, MFA/session/credential arguments or new credential
service are introduced. Publisher bootstrap remains unproven and frozen metadata
must not be amended to manufacture OIDC eligibility. The command is not wired
into a complete release workflow yet; actual publication, registry qualification,
production acceptance and the eleven-job/unsigned launch gates remain open.
Fixture tests and an actual retained-tarball input check are not hosted proof.

Reuse already verified unchanged deployments; no redeploy for unchanged inputs.
First L1 setup uses the same reviewed deployment commands. Publish after candidate
acceptance; registry acceptance follows publishing, never a circular prerequisite.

CI contract: immutable finite release manifest, pinned actions/toolchains/locks,
native target verification, strict exit codes, bounded retries/jobs, per-release/
environment concurrency and durable receipts. Bind workflow/run/attempt, source,
artifacts, profile, deployments and observed cases. Resume only digest-matching
completed steps. Missing/tampered/skipped required cases fail. Record actual proof
dependencies, not an entire unrelated shared file or documentation tree hash.

AWS OIDC roles are least-privilege and repository/ref/environment scoped; verify
the account's actual subject-claim format. Use npm trusted publishing on supported
GitHub-hosted runners after namespace/package trust setup. Initial package/MFA
bootstrap is a one-time dependency. Verify GitHub plan support before relying on
private environment approvals; otherwise protected refs + approved dispatch +
restricted OIDC trust, not a broad token. Cloudflare uses its existing scoped
supported deploy credential. Never upload root credentials, browser sessions or
Bitwarden session tokens into CI. No browser sign-in on each routine release.

Deploy exact qualified artifacts by digest; inspect saved IaC plans, stop on
unexpected deletes/replacements. Roll back to previous verified code/config,
preserving data and staging admission. npm versions are immutable: checkpoint
partial publication, resume exact-byte checks; fixes get a new version, not
overwrite/unpublish to manufacture success. Cache by actual input dependencies.
Deterministic release decisions/provenance are required; bit-identical rebuilds
across mutable hosted images are not claimed without separate proof.

Exit: actual CI run, candidate gate, launcher plus all four registry tarball comparisons and
Mac registry npm/pnpm install/upgrade/rollback pass; default
`cargo xtask distribution-verify --evidence ABSOLUTE_DIR` exits 0. Tag stays beta.

## L4 — Integration and unsigned paid launch verdict

Outcome: integrated/pushed scoped source, versioned install/update instructions,
unsigned/experimental disclosures, purchase/manage flow, operated production
Relay and rollback references. No unrelated cleanup or full signed release work.

Product adds `f2-unsigned-production/v1` manifest/verifier profile to existing
Rust release machinery, preserving historical M0–M8 and baseline bytes.
The coordinator's existing `tools/f2-github` now has a local provider readback
component, `capture-release-chain RUN_ID REVIEWED_SOURCE_COMMIT
ABSOLUTE_NEW_OUTPUT_DIR`. Its fixed contract requires the eleven L3 gate names
as actual successful jobs at the reviewed workflow bytes, their exact successful
owning gate steps and unexpired same-run/source archives with provider digests.
Earlier successful attempts may be reused only within that same run/source.
Consumption additionally requires `just f2-release-chain-verify RUN SOURCE
ABSOLUTE_EVIDENCE_DIR`: fresh GitHub run/job/step/artifact/workflow readback,
exact closed saved report and actual local ZIP digests/inventory. The matching
prelaunch command keeps its ten-completed-plus-running-final distinction and
cannot satisfy the eleven-completed-gate verdict. No report flag is an authority.
Its scope is provider CI completion, with `releaseReady=false`; it does not
replace owning acceptance or the final profile. The complete release jobs and
actual passing chain remain open. Candidate-only runs deterministically fail.
Proposed command, NOT IMPLEMENTED yet:
`./sdlc f2 launch-verify --manifest PATH --evidence ABSOLUTE_DIR`.
It requires L1–L3, stable owning gates, integrated/pushed source, unchanged
baseline and genuine CI receipts. It derives `UNSIGNED_CORE_BETA_READY` and
`RELAY_PRODUCTION_SELLABLE`; both must be true. It cannot assert historical
M7/M8 completion or notarized readiness. No readiness from an implemented
verifier alone, hand-filled booleans, grants without billing or mock-only proof.

## Next action and discipline

Durable rollback continuation: private Relay `7169b50` adds the owning
read-only `relay-data-sentinel` binary and
`xtask/src/relay_e4/deployment.rs`; `just relay-environment-e4-deployment-snapshot`
and `...-verify` feed the existing E4 aggregate's optional closed deployment
bundle. No runtime dependency, frozen native byte, controller trust pin, schema,
grant, or deployed code changed. Design is settled: existing environment-scoped
verification roles, existing DSQL connector, native SQL parameters, an enabled
owner-bound nonempty manifest, exact retained S3 version/body/COMPLIANCE retention,
four healthy function/configuration readbacks, and three ordered observations.
The reviewed plan and distinct accepted CI deploy/rollback receipts must bind
changed code, exact revisions and rollback-origin digest. Reuse/empty data/mock
fixtures cannot satisfy the gate. Actual CI run/job/artifact readback remains
mandatory; the offline chain validator does not attest provider provenance.
Staging's real pre-effect capture passed. The corresponding production owner
currently has no enrolled workspace/archive, proved by the read-only scoped
discovery. Do not fabricate a paid grant or seed SQL to solve that dependency;
complete normal production purchase/enrollment/archive acceptance before claiming
production data preservation. Do not deploy a deliberately degraded Watch build
or alter ZIP metadata merely to manufacture a changed-code test. Existing E4
route observations are reused, not rerun; the actual deploy/rollback gate remains
open until its accepted receipt chain exists. Exact checks/handles are in the
existing compact coordinator checkpoint.

Current CI slice: Relay `39f6f9a` and coordinator `3a79700` are pushed. The
intent-capable operator passed its owning gates and actual GitHub driver build;
its exact archive/member/source digests are retained in the private cache.
Reviewed environment-specific code roles are applied and pinned to the exact
coordinator commit/wrapper/dispatch subject. Repository-only OIDC customization
retains immutable IDs. Actual staging/production plan-only runs pass own-role
authentication and opposite-role/function denials. Unchanged staging execution
passes after an empty-artifact-selection repair. Actual intent archive readback,
fresh-run accepted-receipt recovery and same-job rerun all pass; accepted receipt
bytes and staging code/revision remain unchanged. These are reuse/replay proofs,
not changed-code timeout or durable-data-preserving rollback proof. No update
occurred in the failed probe. Never repin a controller
with unresolved ambiguous effects; recover under its original reviewed version
first. Exact identifiers, artifact bindings, checks and next action are in the
compact
coordinator checkpoint. This is not changed-code deployment, data-preserving
rollback, paid-production acceptance or launch completion. The full coordinator
gate remains red for its missing isolated Hub checkout. All L1–L4 criteria and
the frozen shipping/native qualification inputs remain unchanged.

Follow the execution contract at the top of this file. Native builds, four-target
packaging, Mac qualification and actual CI candidate verification have passed;
do not execute their historical `Next:` instructions again. Preserve the current
Watch repair/probe, close missing L1 production and staging observations, resolve
the named Stripe/npm bootstrap dependencies, and complete L3/L4 automation.

Keep this checkpoint compact: milestone, revision/changed files, exact gate/
evidence, live handle and next action. Detailed history belongs in the archive.
Targeted checks while editing; broad gates at stable integration. Failed CI stops
that release and preserves evidence; diagnose the named failure, do not expand
requirements or rerun unaffected jobs. Existing spending limits still apply.

## Provider references verified 2026-10-02

- [npm trusted publishing](https://docs.npmjs.com/trusted-publishers/): hosted
  GitHub runners supported; self-hosted currently unsupported.
- [GitHub deployment environments](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments): private environment
  and approval capabilities depend on the account plan; verify before use.
- [AWS OIDC](https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-aws): short-lived role trust; newer repositories may
  use immutable owner/repository IDs in subject claims.
