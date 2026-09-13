# Plugin System

Status: public contract and local lifecycle implementation.

Plugins are capability-bounded extensions that can live outside Core once the public contract is stable. Core owns the manifest, operation, capability, permission, resolver, lockfile, and default-bundle contracts. Product may provide hosted configuration or private adapters, but Core must not depend on Product.

## Locator Grammar

Supported locators:

```text
core://plugins/<plugin-id>
git+https://<host>/<org>/<repo>.git?ref=<branch-or-tag-or-sha>#<manifest-path>
git+ssh://<host>/<org>/<repo>.git?ref=<branch-or-tag-or-sha>#<manifest-path>
git+file://<absolute-path>?ref=<commit-sha>#<manifest-path>
```

Rules:

- `<plugin-id>` uses lowercase dotted names such as `tradeassembly.simbroker`.
- Git locators must include `?ref=` and a manifest fragment.
- Manifest paths must be relative `.yaml` or `.yml` paths with no `..`.
- Lockfiles must resolve Git locators to immutable 40-character commit SHAs.

## Manifest Contract

The public manifest schema lives at `plugin-contracts/schemas/plugin-manifest.schema.json`.

Every manifest declares:

- stable plugin id, provider metadata, name, and version,
- runtime protocol and entrypoint,
- capabilities,
- permissions,
- host APIs,
- declarative configuration fields and storage classes,
- health prerequisites and connectivity-check posture,
- trust, sandbox, source-provenance, and integrity facts,
- operation input and output schema format,
- operation capability mapping,
- typed operation traits for supported modes, instrument families, data shapes,
  fields, schemas, timeframes, freshness, latency, determinism, and replay,
- zero or more typed transitive operation dependencies with stable local ids,
  required/optional posture, and optional exact operation constraints,
- effect and risk class,
- credential-grant and account-binding requirements,
- APF finance action id and resource type,
- mandate, purpose, evidence, PEP coverage, check-pack, receipt, and redaction metadata,
- supported protocol, mode, asset, and resource tags for resolver matching,
- forbidden declarations,
- no-advice and no-secret boundary posture.

Forbidden declarations include trade advice, raw secret output, hosted OAuth credentials, billing, payouts, and enterprise SSO/RBAC.

## Local Lifecycle

The implemented local lifecycle separates immutable manifests from durable,
independently configured plugin instances. One manifest can own multiple
instances with distinct account refs, configuration, credential handles,
enabled state, and factual health. Declarative manifest fields drive Studio
configuration without plugin-specific React branches or arbitrary plugin code.

Credential fields are write-only through `CredentialPort`; no public result or
durable lifecycle record contains submitted secret material. Canonical HTTP,
GraphQL, and Studio operations share command handlers. Provider-named routes
remain compatibility aliases for the default instance.

See [plugin-lifecycle.md](plugin-lifecycle.md) for the complete route, state,
credential, health, entitlement, and interface-parity contract.

## Capability Resolver And Entitlements

The runtime exposes a plugin capability resolver through:

- `POST /plugins/capabilities/resolve`
- `POST /capabilities/resolve`
- `POST /strategy/contracts/capability:resolve`
- GraphQL `ResolveCapability`
- MCP `tradeassembly.plugin.capability_resolve`

The normalized resolver requirement preserves the complete StrategySpec
contract: capability and requirement ids, purpose, readiness levels, stage and
substep refs, typed constraints, dependency refs, fallback policy, policy tags,
requested mode, strategy/version context, and any explicit plugin-instance,
operation, or account binding. Legacy unversioned capability declarations remain
parseable during migration; new plugin contracts should use namespaced `@version`
capability refs.

The compatibility candidate response returns:

- matching plugin operation candidates,
- per-candidate blockers,
- top-level blockers,
- local entitlement decisions,
- APF action metadata needed for receipts,
- `noSilentFallback: true`,
- `noAdvice: true`.

The canonical resolver evaluates the complete StrategySpec requirement set as
one deterministic graph. Each selected node identifies an exact plugin instance
and operation; plugin-operation dependencies may recursively resolve through a
different plugin. Ambiguous candidates remain blocked until an execution config
records an explicit binding or permitted configured fallback. Immutable graph
revisions carry the complete requirement set, selected operations, manifest and
credential-generation facts, entitlement decisions, authority actions, and a
stable fingerprint. Backtest and activation consumers reject missing,
incomplete, ambiguous, blocked, or stale revisions. Candidate ordering is never
a binding decision.

Public Core ships with a `local_full` entitlement profile that grants broad local capability access by default. Explicit local deny grants override defaults and must fail closed. Overrides are durable, inspectable, and revocable. Public callers cannot self-create allow grants. Credential-backed operations remain blocked until a local/customer-managed credential handle is configured.

## Lockfile And Default Bundle

The default bundle lives at `plugin-contracts/default-bundle.yaml`.

The lockfile lives at `plugin-contracts/tradeassembly.plugins.lock.yaml`.

The lockfile records source type, locator, resolved commit for Git plugins,
manifest path, and manifest digest. Git source locks must use real,
non-placeholder commits. Extraction candidates remain checked-in `core`
manifests until their external repositories can publish validating manifests
and immutable Git pins. Default local usability comes from a declared bundle
and lock, not from copying hosted Product code into Core.

## Extraction Path

Alpaca is the first intended first-party extraction candidate. It should move only after the contract gate passes and the external repo can publish a manifest that validates under this contract.

Verification:

```bash
cargo xtask plugin-contract
```
