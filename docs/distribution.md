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
completion. Matrix receipts use `tradeassembly.distribution-native.v1`, bind the
exact release-manifest digest, and identify nonempty hashed native evidence for
each check. They must be generated from actual acceptance, not filled with
assertions based on compilation or mocked brokers.

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
No push, merge, npm publication, or namespace-ownership verification has occurred.
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
