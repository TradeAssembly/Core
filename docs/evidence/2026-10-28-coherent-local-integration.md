# Coherent local Core integration

Local branch `codex/core-studio-local-integration-20261028` starts at dependency remediation `0c2c9314914e959e7225f071a627691ff5c8394a`. It carries scheduler/sharing changes through `b95b081`, with additive test/module conflicts resolved by retaining existing redaction tests and Windows support. The approved RSA issuer policy is 2048–8192 bits; smaller keys are rejected. No provider, credential, persisted identity or verification algorithm policy was weakened.

The retained checksum-bound dependency preparation was run from the existing cache without fetching. Focused locked/offline qualification on the combined source passes:

- 12 actual runtime CLI identity tests (real loopback OIDC, callback/bootstrap, encrypted persistence/reopen, tamper/config/expiry, redaction).
- 20 custody tests; desktop keychain round-trip remains intentionally ignored because it accesses the OS store. Native Apple adapter compiles; no real credentials are read or migrated.
- 8 local owner installation/permissions/concurrency tests.
- 2 actual binary browser-onboarding tests, including restart/cancel; split core binary targets compile.
- 9 sharing tests, 6 worker observation tests, 2 scheduler projections and 5 recovery tests.
- 40 distribution tests covering declared-target inventories, integrity, private bootstrap serialization and source-bound replacement/qualification receipts.
- Runtime and core-binaries all-target Clippy with warnings denied, formatting and changed-path public-boundary scan.

This is 104 passing focused tests and one intentionally ignored OS-store test, not full owning CI, cross-platform certification or installation/release readiness. All data and identities are disposable fixtures. Default custody and caller contracts remain intact. Source dependency lock is the prepared approved dependency lock, without advisory suppression or further version changes.

No public push/PR retry, merge, artifact release, consumer pin update, deployment or live activation occurred. The prior publication denial's compatibility decision and actual caller compilation now have new evidence; the full owning gate condition remains pending. Report those facts before any publication attempt. Existing source/checkouts/binaries are preserved; builds serialized under byte-budget policy.
