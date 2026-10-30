# Verified data packages and immutable plan storage

This local slice adds package loading and host-only plan preparation, save and
currentness functions. It adds no HTTP/MCP route, evaluation, credentials,
indicator calculations, broker effects or activation.

## Package compatibility

Data package V2 is a new strict envelope with `packageContractVersion: "2"`:
`package` contains the unchanged V1 descriptor; `dataContracts` maps operation IDs
to digested contract documents; `dataSchemas` maps schema references to digested
schema documents. Each document entry carries both the byte-level artifact digest
and its canonical JSON digest. These are distinct hashes and are checked separately.
The nested descriptor continues to specify wire and SDK compatibility. Legacy V1
parsing rejects V2 fields, and unsupported outer versions fail closed.

Install and installed-record reads validate all declared data documents offline.
Paths must be package-relative, without symlink components. Documents are capped
at 1 MiB; packages allow at most 64 contracts and 256 schemas. Schema references
must be local fragments. Port and state schemas must have verified artifacts,
and input/output schema references must be declared by the manifest operation.
A contract is returned only after the package is revalidated through the existing
package store. Contract document loading does not admit invocation.

## Plan persistence

The host supplies strategy scope and an ordinary pipeline request. It cannot
supply verified contracts through this service. Preparation requires strategy and
plugin-instance object access, a current saved capability revision for the exact
strategy/version/spec hash/mode, and verified active package/manifest bindings.
Currentness uses the runtime clock, not a request-selected historical timestamp.

The record pins the compiler plan and exact package digests. Its content digest
identifies an immutable revision. The revision and authority/strategy-scoped
idempotency key are written with storage expectations in one atomic batch.
Conflicting keys or immutable content fail. Matching retries are idempotent;
partial records are rejected. The storage implementation must support atomic
batches; there is no sequential-write fallback.

Currentness verifies stored identity, rechecks object access and graph admission,
reloads package contracts, recompiles the same request and compares complete
content. Neither a cached plan nor a cached idempotency record skips these checks.
Changed packages, contracts, strategy facts or revoked bindings require a new
explicit preparation. There is no migration or silent rebinding.

The service integration test installs separate provider and calculation fixture
packages, saves and checks a plan, then disables the provider binding and verifies
that currentness and idempotent save both fail. Executable fixture bytes are never
invoked. Existing graph-currentness contracts remain the owner of entitlement and
credential checks. Public route integration, explicit package-replacement cases,
host evaluation and the default pack remain separate follow-up work.
