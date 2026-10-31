# Instrument-selection data V1

This candidate adds a common candidate-set codec and host-witnessed collection
acquisition. It does not yet bind an InstrumentSet to a StrategySpec selector,
run a screener, schedule session-relative occurrences, warm selected contracts,
manage positions or submit orders. Existing static/expression selectors and old
wire/checkpoint versions retain their existing behavior.

## Candidate-set codec

`tradeassembly_plugin_sdk::instrument_set::InstrumentSet` is a closed
`tradeassembly.instrument_set.v1` value. Static, expression and screener sources
share the same envelope. Source definitions/semantics and operation/data receipts
are referenced by SHA-256. These references require host ownership and provenance
validation; possessing or deserializing a set is not authority.

A set identifies the exact universe definition, inspected membership, ranking
policy and candidate count. Ranked members have unique canonical instrument IDs,
asset class/family and explicit context instruments. Context and traded member
identities are separate. Rejection summaries reference bounded evidence rather
than embedding an unbounded universe. Reason counts may overlap when multiple
reasons reject a candidate. A summary is not a replacement for the evidence.

Statuses distinguish `complete_nonempty`, `complete_empty`, `partial` and
`failed`. Partial and failed results carry explicit issues. Failed results cannot
contain selected members. A complete empty result can represent no eligible
candidates; incomplete coverage cannot silently become that result. Completeness
is always relative to the exact universe definition, never an inferred global
market. The codec validates structural consistency, not the provider's assertion
that its pagination or scope is complete.

Evaluation, knowledge cutoff, known time and expiry are distinct. Data receipts
retain original field-effective dates, source timestamps and coverage. Host
acquisition now does not prove historical availability. `require_complete_scope`
checks exact universe identity, complete status and current expiry; consumers must
also verify the referenced evidence and policy. It grants no entry permission.

Bounds: 256 selected members, 32 context IDs per member, 64 data receipt hashes,
128 rejection summaries, 64 issue codes and 256 KiB encoded set. All bounds apply
together. A set within codec bounds is not automatically within a downstream
plan's history/frame/deadline limits. Candidate membership is not a position or
pending-order ledger; reselection must not delete managed-position exit state.

## Host collection acquisition

HTTP routes:

- `POST /product/strategies/data-collections/create`
- `POST /product/strategies/data-collections/get`

MCP tools:

- `tradeassembly.strategy.data_collection.create`
- `tradeassembly.strategy.data_collection.get`

Create accepts `collection` (closed CollectionCommand), `idempotencyKey` and
optional top-level `accountMode` (defaults to paper).
CollectionCommand contains `scope` (strategyId, strategyVersionId,
strategySpecHash), `capabilityGraphRevision`, `acquisitionNode`, `mode`,
`responseSchemaRef` and object `input`. The host resolves an existing current
read-only graph operation and its owned enabled plugin instance, verified package,
manifest, configuration and credential revision. The response schema must be an
exact declared verified package schema. There is no new registry or implicit
provider substitution. Credentials remain scoped to the existing operation host.
Broker/order operations are rejected.

Account authority must match the request: paper for paper/backtest reads and
explicit live authority for a live read. A data acquisition receipt never grants
brokerage authority. Get accepts `collectionId` and revalidates current owner,
graph, package, configuration and receipt integrity.

The result contains `collection` (a stable bounded
`tradeassembly.data_collection_summary.v1`) and
`executionAuthorityGranted:false`. The owner-local immutable `tradeassembly.data_collection_receipt.v1` binds the command/query, source
operation/package/configuration, separate response-schema and payload canonical
hashes, original provider response and host witness time. It does not invent bar
intervals, publication timestamps, entitlement, complete coverage, quotes or
selection eligibility. Field-specific freshness and incomplete-universe policies
remain explicit downstream requirements.

The entire operation response must fit 512 KiB, with at most 4,096 array elements,
65,536 visited JSON nodes, depth 24, 256 fields per object and 16,384 bytes per
string. Commands fit 64 KiB and receipts fit 640 KiB. Bounds apply together. The
existing 1 MiB plugin wire envelope is unchanged. Oversized results fail as a
whole; no truncation, hidden sampling or promotion of partial coverage occurs.
A larger provider catalog requires a separately versioned bounded paging/chunk
protocol with explicit final coverage evidence; that protocol is not implemented
by this candidate.

Receipt and idempotency index commit atomically. Same-transport retries return
the original current receipt; a changed command conflicts. Current validation
also runs before an outer HTTP/MCP cached response is returned. A retry does not
silently acquire a newer snapshot. Evidence being intact is distinct from its
freshness for an entry decision. HTTP malformed input is 400; stale/domain or
binding/commit conflicts are 409. MCP failures use `isError:true`.

Public summaries contain scope/graph/mode, opaque collection and evidence hashes,
plugin/operation/package/manifest/schema identities and observation/witness times.
They omit raw payloads, queries, pages, cursors, account/credential bindings and
provider outcome IDs. This keeps first and cached HTTP/MCP responses identical
without relaxing generic redaction. Native selectors retrieve verified raw
receipts internally by collection ID; a client-supplied summary is not evidence.
Receipts remain owner-local and can contain raw provider metadata. Do not copy
complete receipt bodies to a cloud transport by default. Relay authorization and
graph/selection/runner integration remain release requirements.

## Qualification boundary

Focused tests cover closed sets, status/scope/time distinctions, ranking/identity
bounds, evidence requirements and rejection codes. Collection process fixtures
exercise the actual operation host, HTTP/MCP routes, atomic storage failure,
service recreation, cached retries, tampering, foreign ownership and revocation
using local HTTP and fake credentials. Optional externally pinned package tests
run only with an explicit loopback fixture configuration. NativeTestSandbox is a
test adapter, not production OS sandbox qualification. No authenticated provider,
live brokerage, released distribution or completed screener is established by
these fixtures.
