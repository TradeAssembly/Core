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

## Current implementation and remaining release gaps

The Rust distribution crate and npm shim implement packing, byte verification,
safe extraction, private per-user roots, source-free local setup, real-authority
offline plugin registration, durable transaction guards, and stable launch paths.
Do not mark the five-target release complete from these local results.

Checkpoint (2026-09-28): branch `codex/f2-npm-distribution`, based on Core
`a16d4a769258fb7354d9102017533850020f8f2c`. Local implementation commits
`e1557c9` and `890eb5c` precede the configuration-preservation checkpoint.
No push, merge, npm publication, or namespace-ownership verification has occurred.
The local candidate is `target/npm-preserved-candidate`, version `0.1.0-beta.1`;
its installer SHA-256 is
`8d03610fddb22e3c4b5f1d7dcc5d41845b68a59e08d6e7d76b7c2e74082faaae`,
archive SHA-256 is
`7c0a4d6b55555a4a0e0a39bd9b65621879c0240e8b6ca663411973fb2dbdd8a4`,
and release-manifest SHA-256 is
`3f71fb0b87eb1f1bd1eb77fef54755fe045b262dfe1651052ae590a8f909e486`.
The original bundle and parent-lock hashes above remain unchanged; original
Core/Warden strict ad-hoc signature checks pass.

Passing local evidence: seven Rust unit tests; distribution strict Clippy;
actual frozen-binary installer acceptance (including edited-config preservation,
external-state denial, running/active-state denial and interrupted recovery);
actual offline local-tarball npm/pnpm acceptance with lifecycle scripts disabled;
format/diff checks, dependency deny and machete checks, and current source
whitelist/public-boundary/architecture checks. Native test logs are
`target/distribution-native-install.log` and
`target/distribution-package-managers.log`. These are Mac arm64 evidence, not
five-target native or registry qualification; the local candidate remains
`publishable: false`.

The required full `just verify` / `cargo xtask verify` gate is **not passing**.
Its workspace Clippy and pinned sandbox dependency setup passed, but workspace
test linking exhausted disk (`errno=28`, including `backend_parity`) even with
serialized compilation, disabled incremental/debug output, and stripped symbols.
Evidence is preserved in `target/core-verification.log`; only this task's bulky
build cache was cleaned. Workspace tests/nextest and subsequent full-gate stages
were not completed. Do not claim merge or release readiness from targeted checks.
No owned test processes remain running.

Native target production remains a release dependency. The existing
`xtask/src/bundle_local.rs` is Mac arm64 only; the frozen
`runtime-rs/src/bin/tradeassembly_sandbox.rs` rejects non-Mac platforms. New target
payloads need native Node, Warden, Alpaca and sandbox prerequisite packaging,
including Windows SRT installation/ACL checks, and actual controlled-sink
qualification. The distribution wrapper cannot cure a missing native runtime.
Authenticated npm namespace ownership, package publication/provenance, and public
registry installation tests are still required. Apple notarization remains
excluded. Keep one current-state section here rather than per-repair plans.
