# Authenticated local factsheet snapshots (v1)

This local capability shares an immutable **redacted factsheet**, optionally with a completed backtest summary, on one configured installation. It does not export an executable strategy, create a public/bearer link, send invitations, discover users, provision identities, or change strategy/plugin/broker/account permissions. Hosted delivery and cross-issuer recipients are not supported.

## Identity and ownership

Use the existing authenticated Studio transport and canonical issuer/subject `ObjectOwner`. Snapshot creation, management, and reads require a human signed-in principal; anonymous/CLI ambient authority and delegated agent access are unavailable in v1. The owner must own the source strategy. A new `share_snapshot` scope inherits that strategy owner through `ObjectAuthorizationPort`.

Recipients are explicit, snapshot-only read relationships: immutable creation records contain at most ten exact case-sensitive subjects under the creator's issuer. The recipient must authenticate through that same issuer on this installation. Email/display-name resolution is not performed. The relationship grants only factsheet view/download, never original strategy, dataset, research run, journal, account, or management access. The existing ownership port has no generalized grant mutation API; these narrow read relationships are feature-scoped and do not add global roles or bypass transport/control-plane authority preparation.

## Preview and creation

`POST /product/strategies/share-workspace` with `strategyId` and optional `researchRunId` returns `sharingVersion: local_snapshot_v1`, explicit capabilities, the redacted preview/content hash, owned snapshots, and at most twenty verified completed research choices matching the latest published version.

`POST /product/strategies/share-snapshots/create` requires:

- `strategyId`, optional `researchRunId`, and the exact reviewed `expectedContentHash`;
- a fresh intent `idempotencyKey` (reuse it for an uncertain retry);
- `acknowledgeSnapshot: true`;
- optional `recipientSubjects` (omitted/empty means private);
- `acknowledgeRecipients: true` when naming recipients.

A changed preview or reused key with a changed request is rejected. The latest published immutable version must have a valid canonical spec hash. Optional research must be completed, owned, and bound to that exact strategy/version/hash; immutable manifest/result verification is required. Drafts and foreign/mismatched research are rejected.

The exported allowlist contains the redacted strategy name, version, sanitized instrument labels, definition hash, fixed research/paper notice and optional simulation counts/numeric accounting/provenance hashes. It never serializes the original StrategySpec, metadata, rules, plugin configuration, credentials, broker account references, orders, recipient roster or principal identity. Free text is bounded and sensitive/token/address/identity-like labels are replaced. The complete preview is available for owner review. A factsheet is capped at 32 KiB. Simulation data is identified as such; it establishes no live performance claim.

## Integrity, audit and access

Creation atomically writes content, private access state and the first immutable audit entry under absence expectations. Each later access verifies the canonical owner scope, content hash, complete record hash and access-state hash against durable audit records before returning data. Source changes do not mutate existing snapshot content.

`POST /product/share-snapshots/view` and `/product/share-snapshots/export` require `snapshotId` and current authenticated authorization. Only exact IDs are resolved; `latest` and fixed demo records are not substituted. Responses expose the factsheet and `RFC8785+SHA-256` integrity manifest, not ACL/identity internals. Unknown/unauthorized/revoked-recipient reads return the same 404. The owner can see revoked metadata in management; revoked content is unavailable.

These routes retain normal transport authentication and control-plane authority preparation. Repeated reads re-evaluate current grants/integrity instead of replaying a cached successful response. Download responses must be private/no-store. Reads do not create or extend grants.

## Revocation

`POST /product/strategies/share-snapshots/revoke` requires owner authority and `snapshotId`. Omit `recipientSubject` to revoke the complete snapshot, or provide one initial recipient to revoke that relationship. State and a new immutable audit entry update atomically under a compare-and-set expectation. Concurrent conflicts are explicit; repeat revocation is idempotent. A retried creation cannot restore access. No operation adds recipients or changes content after creation; create a new reviewed snapshot instead.

Revocation blocks subsequent authenticated reads/downloads, including repeated identical requests and after restart. It does not recall previously downloaded bytes or cancel a read already authorized before revocation. Raw journal export is not included. Existing research/report exports are separate owner-controlled operations.

## Deployment limits

This does not make the Studio listener public or deliver a link to anyone. A recipient needs an existing sign-in and network access to the configured installation. No hosted identity, invitation, email, public-link, synchronization, commercial profile or production rollout is implied. A consumer must require the explicit v1 capability; older runtimes returning fixed demo acknowledgments/factsheets remain unavailable.
