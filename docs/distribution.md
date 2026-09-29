# F2 npm prerelease distribution

The canonical F2 plan remains Product's
`product/release-plans/2026-09-23-f2-implementation-completion-plan.md`.
This document specifies its separately qualified distribution layer, not a new
runtime release lock. No Apple payment, notarization, broker orders, or trading
activation is authorized by an install or upgrade.

## Frozen input and proof matrix

The parent unsigned-cohort lock SHA-256 is
`222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639`.
The Mac arm64 bundle manifest SHA-256 is
`6740c7d7f4e8a692ff005dd18b6dc2656883eb8e366253e4e8e6aa054108f5dc`.
Packaging must not rebuild, re-sign, or edit its 846 payload files. New targets
must have separate native qualification, never copied Mac receipts.

| Requirement | Owner / deterministic acceptance | Evidence |
| --- | --- | --- |
| Frozen bytes and parent identity | distribution verifier: manifest, every file, target, parent-lock digest | package release manifest |
| npm/pnpm delivery with scripts disabled | packed and published install tests | native command outcomes |
| Safe extraction | traversal, escaping links, extras, corruption, mode tests | Rust tests |
| Persistent source-free installation | actual frozen binary setup, Warden and offline Alpaca | native installer acceptance |
| Reinstall, compatible update and rollback | actual versioned installer, preserved state and authority | installation receipt |
| Reject running/incompatible rigs | real SQLite state and native process inspection | negative acceptance |
| Platform enforcement | real Warden and controlled broker sink per new target | native target qualification |
| Five-target public prerelease | exact npm versions and all target receipts | distribution matrix gate |

Supported targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`x86_64-pc-windows-msvc`. Windows ARM and musl are excluded. Platform dependencies
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
and native enforcement evidence is present for all five targets. Package tests,
builds and a Mac-only installer are not whole-plan completion. The original
M0–M6 lock and false M7/M8 verdicts remain unchanged. Run owning Core gates before
any merge; publish only a `beta` tag after authenticated namespace ownership and
native qualification. Ad-hoc signing and npm provenance are not notarization.

Publication has two gates, not a circular pre-publication registry requirement:
`distribution-verify --candidate --evidence DIRECTORY` requires all five native
targets and local npm/pnpm tarball tests before publishing. Default
`distribution-verify --evidence DIRECTORY` additionally requires installs from
the actual public registry. Candidate qualification is never customer-release
completion. Matrix receipts use `tradeassembly.distribution-native.v2`, bind the
exact release manifest, installer descriptor/binary and npm launcher/platform
tarballs, and identify nonempty hashed native evidence for each check. V1
manifest-only receipts fail closed. They must be generated from actual
acceptance, not filled with assertions based on compilation or mocked brokers.

## Maintainer acceptance commands

Use an absolute, new output directory and the actual locked bundle and parent
lock, not a fresh runtime build:

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

This cohort deliberately fixes the current Mac arm64 payload. Later qualified
runtime locks can update the installer's candidate pins without invalidating
structurally valid previous installation records. Changed Warden or state
compatibility still requires an explicit, separately qualified migration.

For a new target, native freezing requires an explicit schema-1 input descriptor
(`distribution/src/native.rs`), the actual target host, three source revisions,
four binary digests, and the Alpaca archive/manifest digests. It checks binary
architecture, Node `22.23.2`, locked and installed SRT `0.0.67`, contained links,
and complete payload inventory. Windows staged links are rejected. This is
trusted-build-input validation, not source attestation or runtime qualification;
the command explicitly reports `qualified: false`. It refuses to recreate the
original Mac arm64 bundle.

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
Reuse the qualified Mac candidate and recorded checks. Preserve the frozen Mac
inventory and parent lock byte-for-byte. New targets receive independent native
qualification; they do not inherit Mac proof or require rerunning unchanged
M0–M6 captures. Revalidate only evidence whose actual inputs change.

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
frozen Mac arm64 bundle. Producer workflows run in their owning private repos;
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

### Current execution checkpoint

**Current step: D2; D1 decisions are recorded above.** Core source checkpoints
are `7782118` and `48eb1d2` (local, not pushed). Warden checkpoint is `d2bd10d`
(local, not pushed); Alpaca candidate branch is committed/pushed at `592bd8f`.
Core remote PR #2 remains at `520be81`. No native Windows success or five-target
qualification is claimed. Prior turn was a status-only/no-progress turn; this
continuation implemented recovery/read fixes and obtained new verification.

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
preserving package artifacts, frozen payloads and evidence logs. Core retry is
genuinely running under process handle **76254**, with debug/incremental disabled,
output `target/native-core-owning-verify-retry.log`. Revalidate this handle before
doing anything else; do not restart because output is quiet. No other worker,
build or routing-operation handle remains live. Core's full gate is not yet green.

**Exact next action:** resume Core verify handle 76254 and inspect its actual
terminal result; scoped source is checkpointed and producer caches are reclaimed.
Resolve GitHub's verified
native-runner billing dependency through the mandatory operations route without
inventing success or silently raising an unbounded spending limit. Execute owning
native builders and real Windows authority tests. Resolve native failures against the
recorded ACL/atomic-write contract, not by weakening validation. Implement the
qualification capture/composite controlled-MCP proof before creating receipts.
D3–D5 remain unqualified. Do not rebuild the frozen Mac payload, restart M1–M6,
expand Studio/Hub/Relay features, or reopen M7/M8.
