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
at 1 MiB and 16 MiB in aggregate; packages allow at most 256 contracts and
256 schemas. Schema references
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

## Bounded V2 catalog capacity

V2 packages admit up to 256 operation-data contracts, 256 data-schema documents
and 256 base response-schema documents.
The embedded wire V1 format is unchanged; unknown descriptor versions/fields
remain rejected. Older hosts reject catalogs above 64 contracts, so packages
using the larger capacity require a matching qualified host/source pin. This
capacity change does not enlarge a runtime DAG, frame, state or evaluation budget.

Descriptors are read with a 1 MiB cap before decoding. Typed manifests and each
contract/schema document (including base response schemas) are capped at 1 MiB; aggregate document reads at 16 MiB;
manifest operations at 512. Metadata trees have at most 32 nesting levels,
32,768 visited nodes, 2,048 members per collection, 16 KiB strings and 256-byte
keys. JSON Schema compilation uses the same bounded work accounting expanded
through local JSON-pointer references: at most eight reference levels, no cycles,
anchors, remote, dynamic or recursive references. Regex patterns have a 512-byte
length and 64-pattern work limit. Structural limits bound validation work; they
are not a preemptive CPU timer. Existing package archive/expansion limits,
artifact/canonical hashes, safe paths, symlink rejection and offline resolution
remain in force. The manifest operation index avoids repeated linear searches.
Legacy descriptor decoding retains its closed schema, with the common 1 MiB
resource cap; legacy manifests are not reinterpreted as typed metadata.

Catalog admission is separate from availability, exact history planning,
calculation readiness, state reconstruction, installed instance authority and
broker eligibility. V1/V2 warmup metadata still has its existing meaning; a
capacity increase does not make undeclared composite histories or reset policies
executable. Such semantics require a separately versioned operation contract.

## Bounded verification reuse

A local package store retains at most one parsed, validated data-metadata entry.
Its identity covers the package root, complete descriptor and manifest. A hit
rereads and hashes every declared response schema, data schema and operation
contract, including unrelated operations, through the same path, symlink,
per-document and aggregate limits. Package reads still validate installed-record,
manifest and executable identity. Failed reads cannot produce a trusted snapshot.
The cache contains no ownership, credentials, configuration, graph or account
authorization; a cached schema or contract grants no invocation permission.

Plan preparation uses one verified package-and-requested-contract snapshot per
unique package within that preparation call. Every node still checks its current
object access, instance, manifest and configuration/credential bindings. The
snapshot is discarded after the current-validation call. Evaluation dispatch and
final atomic commit retain their current-plan checks; the external plugin host
still revalidates the exact executable and current invocation authority. No
cross-step plan cache, process reuse, deadline increase or sandbox change is
introduced. Existing package adapters retain a conservative default snapshot
implementation. The snapshot is an in-process port value, not a new wire format.

During one evaluation step, at most 64 compiled schema validators are retained,
keyed by package digest, schema reference and canonical schema digest. Eviction
revalidates and recompiles on the next use, so it does not reject larger valid
plans. Every returned value is still validated. The cache is discarded with the
step, including on failure; it is never checkpoint state. Schema reuse cannot
skip the full artifact revalidation before dispatch or before the atomic commit.
Changes made after the last dispatch still reject the transaction.
