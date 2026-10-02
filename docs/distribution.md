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

## Current checkpoint

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
Relay has an active monthly catalog entry and both existing clients, but the
fulfillment origin still names the older F2 gateway. AWS CLI confirms the actual
production gateway is distinct; correct the normal Hub binding before purchase
proof. Legacy dashboard edits correctly reject this manifest-managed product;
use versioned plan/apply, not a guard bypass. Hub `ba45db1` checkpoints the
operator review panel and signed-assertion route aliases. Hub Rust formatting,
strict workspace Clippy and all workspace tests passed (two explicit skips),
log `/tmp/tradeassembly-hub-registration-full-gates.log`, SHA-256
`100f24403c323119f5e5668995d7576a66a121c4c1c4bba3610984b5ad1729e2`.
Edge check and dashboard build passed; four ARM64 Lambda packages built.
These are local checkpoints, not deployed acceptance. SSO refresh `54230`
completed successfully; AWS CLI verified the intended `HubProductionDeploy`
assumed role in account `056319544861`. Bitwarden CLI supplied credentials and
TOTP directly to the requesting Chrome process without logging them. No root
bypass or MFA disable. Plan `25950` failed safely before apply: omitted
observability inputs would remove existing alerts and violate the outbox
precondition. Readback confirms the existing confirmed SNS subscriber and $25
Hub budget. Final plan `54589` also preserves the existing enabled recovery
timer. Reviewed plan `dist/hub-release-evidence/20261002T172002Z-38417/hub.plan`
has no deletions/replacements: four package/alias updates and Stripe-webhook
outbox-wake permission/configuration. Guarded apply `55336` is running; log
`/tmp/tradeassembly-hub-manifest-runtime-apply.log`. Next: verify that apply,
deploy the AWS-origin Worker, then review the actual registry
revision and update Relay's production manifest. No paid grant is fabricated.
Temporary CLI credential files are removed
after each run; refreshed credentials return to Bitwarden.

An earlier Mac candidate passed native qualification; later source/profile bytes
cannot inherit that pass. Reuse unaffected GitHub producer outputs/evidence.
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
CI orchestration. Build once, qualify exact artifacts, promote/deploy the same
artifacts, publish npm beta, verify registry installs/upgrades and integrate.
Success requires current owning gates, both distribution verifier modes, the
unsigned production launch verifier and successful actual CI receipts. No Apple,
five-platform acceptance, F3/F4 or real trading. Root fixes named failures; CI
owns routine execution. Do not silently expand scope or count partial passes.

The replacement L1–L4 goal is active in the main thread. Its previous
five-target/paid-exclusion objective has been replaced, not completed. Do not
start a second competing implementation goal.

## L1 — Production service, purchasing and staging acceptance

Outcome: paying customers can use production Relay, ordinary customers cannot
use staging. Owners: private Relay runtime/IaC/xtask/profile; existing Hub identity,
catalog/billing/entitlement; Core profile/onboarding. Product owns aggregation.
No Hub rewrite unless a specific failing contract proves it necessary.

Freeze one finite acceptance manifest: exact deployments/profile, named routes,
owner/controlled nonowner, approved live product/price and expected effects.
Reuse existing journal/OAuth drivers and M4/M5/M6 runners where they actually
cover selected production resources. Old test receipts are not production proof.
Fill uncovered cases only; no bespoke framework per check.

1. Actual packaged production Hub/Alpaca Paper signup/connect/read/status, without
   manual keys, forced diagnostic tenant or staging URLs. One initial browser
   consent is allowed; stored test grants support later automated smoke tests.
2. Approved production Hub offer, checkout/return and subscription management;
   signed webhook/receipt processing, purchased entitlement, allowance checks,
   cancellation/expiry and denied access. A test grant does not prove live billing.
   Require a genuine provider-confirmed production purchase/entitlement record,
   existing or from an explicitly authorized operator purchase; no arbitrary
   charge or fabricated payment. Reuse Hub; price is not selected by this plan.
3. Production enrollment/journal ingest/replay/query/export, entitlement and
   workspace/environment denials. Watch/Reach prove production owner/node
   authority, not an acceptance shared-secret bypass. Isolated diagnostic state;
   no broker orders, customer-account changes or new trading activation.
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
Exit: `cargo xtask distribution-verify --candidate --evidence ABSOLUTE_DIR`
returns 0 under the four-target policy with correct production profile/L1 bindings.

## L3 — CI deployment, npm publication and registry acceptance

Outcome: one authorized release input drives the existing tools without an agent
loop. Private Product owns `.github/workflows/f2-release.yml` and Rust/owning
`./sdlc` orchestration. Core/Warden/Alpaca expose pinned reusable native jobs or
their existing dispatch/artifact interface; Relay owns service deployment recipes.
Public Core must not fetch private source or hold hosted deployment credentials.

Pipeline:
`preflight -> build/reuse -> stage/package -> native qualify -> staging smoke ->
reviewed production plan/deploy -> production smoke -> candidate gate -> npm beta
publish -> registry bytes/install/upgrade checks -> aggregate launch gate`.

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
Proposed command, NOT IMPLEMENTED yet:
`./sdlc f2 launch-verify --manifest PATH --evidence ABSOLUTE_DIR`.
It requires L1–L3, stable owning gates, integrated/pushed source, unchanged
baseline and genuine CI receipts. It derives `UNSIGNED_CORE_BETA_READY` and
`RELAY_PRODUCTION_SELLABLE`; both must be true. It cannot assert historical
M7/M8 completion or notarized readiness. No readiness from an implemented
verifier alone, hand-filled booleans, grants without billing or mock-only proof.

## Next action and discipline

Capture the existing pinned Core native builds when successful; clean owning
gates have passed. Then execute L1 production packaged onboarding and
purchase/service acceptance. Preserve the passed revoked-owner proof. Resolve
npm ownership/trusted-publisher bootstrap through independent deterministic
setup, not another agent/reviewer loop.

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
