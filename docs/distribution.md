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
here; their unchanged Mac qualification evidence is retained). No gate completion
claim until owning gates and the four-target candidate verifier pass. Next: those
exact gates, then the CI-owned aggregate/publication steps. L1 commercial proof
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
