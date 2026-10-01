# F2 npm prerelease distribution

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
the alternate is GitHub Actions using ephemeral AWS CodeBuild-hosted runners
under the existing AWS budget, selected and proven separately. It is not the
old direct CodeBuild path. Public visibility is only possible after the
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
Next: assemble the versioned four-target candidate from these exact outputs,
check each binary architecture/inventory and notices, run the Mac acceptance,
then satisfy candidate and registry verifiers. Do not mark those gates passed
from green GitHub jobs alone.

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

The Core, Warden and Alpaca native GitHub workflows are opt-in diagnostics, not
release producers or qualification gates. Their branch-push triggers were removed
to avoid unplanned native builds while the CodeBuild path is prepared. Manual
dispatch is not assumed available from an unmerged branch; no native artifact
receipt may be inferred from these workflows. CodeBuild configuration, real
builds, artifact capture and the separate Mac qualification remain required.

Current implementation status: **plan amended; candidate/registry verifier code
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

The parent unsigned-cohort lock SHA-256 is
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
on all four targets, and three CodeBuild experimental receipts pass. Package
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
`distribution/src/lib.rs`. Frozen Warden source is `e5b926c`; changes use a new
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

The approved first-release scope above supersedes this section's original
proposal: CodeBuild is selected, Windows/Linux acceptance is deferred, and only
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
AWS CodeBuild is the selected first-release build route.
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
Public standard GitHub runners remain a future option if those repositories
are published. Use eligible AWS credits for private producer builds; verify credit
eligibility and bound compute before dispatch. AppVeyor is another hosted
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
have successful pinned CodeBuild artifacts and integrity evidence, explicitly
marked experimental and not platform-qualified.
Files/repos: Core distribution verifier, candidate/installer and npm packages;
the existing Core, Warden and Alpaca producer worktrees; private coordination
CodeBuild configuration and readback capture. The GitHub native workflows are
manual diagnostics only, not this release's producer or acceptance gate.
Before dispatch, read AWS identity, credit/budget applicability and project
configuration; pin producer commits and build images, bound build timeout and
concurrency, and preserve sanitized build IDs/logs. Inspect each artifact's
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
