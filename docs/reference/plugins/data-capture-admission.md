# Host capture admission

`POST /product/strategies/data-captures/create` (MCP
`tradeassembly.strategy.data_capture.create`) accepts `capture` and an
`idempotencyKey`. The closed capture command contains `input` and `policy`:

- `scope`: exact owned strategy/version/spec hash for acquisition provenance.
- `capabilityGraphRevision`, `mode`, `acquisitionNode`, `replayNode`: explicit
  current graph selections. Both operations must be read-only; replay must be a
  deterministic, replayable, credential-free, stateless typed source.
- `parameterExtracts`: replay parameter names mapped to RFC 6901 pointers in the
  actual acquisition response. There is no caller-supplied response admission API.
- `context`: exact instruments, asset classes/families and positive interval.
- `eventStartMs`, `eventEndMs`, `gapPolicy: "strict_fixed_interval"`: complete
  fixed-interval coverage, bounded to 256 points per output. No calendar/session
  inference, gap filling, implicit provider replacement or partial admission.

Core invokes acquisition through the existing authorized plugin host and scoped
credential transport. It verifies the actual response schema, extracts and
normalizes replay parameters, compiles the exact typed replay contract, invokes
replay and validates every point's schema, identity, completion, interval,
coverage, ordering, hash and causal cutoff. Input and policy are bounded; native
response redaction still applies. Credentials and configuration are not copied
into the receipt: request/configuration context is hash-bound. Header, cookie,
authorization and credential-grant fields in acquisition payloads reject admission. Original response
and point evidence remain private local records, not a new dataset catalog.

The closed `tradeassembly.data_capture_receipt.v1` receipt binds acquisition and replay package/manifest/instance,
non-secret execution-context revisions, original strategy scope, actual host
response identities and canonical payload hashes, replay parameters, contract identity,
coverage and observation timestamps. Receipt and idempotency records commit in
one conditional storage transaction. Failed extraction, validation, invocation or
commit produces no admitted receipt. Cached create/get responses recheck ownership,
current graph/package/context and receipt integrity before returning data.

`POST /product/strategies/data-captures/get` (MCP
`tradeassembly.strategy.data_capture.get`) reads a `captureId` under its inherited
strategy ownership and current binding checks. A hash supplied with arbitrary JSON
is not evidence of host acquisition.

## Binding captures to strategy runs

Keep capture selection outside the immutable strategy rules. A source node in the
published typed strategy declares empty parameters. Supply `sourceCaptures` as a
root graph-node ID to capture-ID map when preparing its data plan. Core fills only
those source parameters from the admitted receipt. It cannot override transform
parameters, rules, instruments or bindings. Captures can be reused by another
version of the same owned strategy; the original acquisition scope remains in the
receipt. Different strategies require an explicit future sharing contract.

The plan records source authentication explicitly. A source without a capture
binding is `unverified_input`; imported snapshots never acquire authenticated
provider status merely by matching an unkeyed hash. A bound source is
`host_acquisition` only after exact parameter/context/current-operation/contract
checks. Missing, foreign, modified or differently bound receipts reject planning
and every freshness check. Capture-bound compiled plans use V4; older readers
reject that version. Legacy checkpoints are not rewritten. Older prepared plans
may require explicit preparation again because source-authentication metadata is
part of immutable plan identity.

Evaluation windows must remain inside admitted coverage and cannot predate the
host acquisition/replay observations. Replayed source points must match the
admitted points before Core applies the versioned host availability envelope:
known/arrival times are conservatively raised to host observation times, and the
original point and receipt hashes join upstream lineage. Original provider timestamps and canonical point evidence remain in the receipt. This is never a backdate to
historical bar close. Batch and incremental replay retain the same envelope and
checkpoint/receipt transaction semantics.

A host-witnessed capture proves an invocation of the pinned plugin, not that a
provider has supplied historical publication/revision evidence. Historical
research availability assumptions remain separate from factual observation times.
The current path grants no broker order, activation or execution authority.

## Qualification

The owning fixture uses separate native processes, loopback HTTP and fake
credential canaries. It covers product acquisition, strategy authoring/publishing,
plan preparation, replay, fresh service/adapters over stored state, causal output
parity, gaps/future data/missing extraction, atomic failure, receipt tampering,
foreign owners and revoked cached retries. It is not production OS-sandbox or
live-provider qualification. An opt-in test accepts an independently pinned
external package fixture through `DATA_CAPTURE_PROVIDER_CONFIG`; it rejects
non-local service URLs and non-fixture credential values. Native package and
provider coverage remain separate qualification evidence.

V3 instrument-independent replay with multiple capture instruments is currently
rejected before acquisition (`data_capture_multi_instrument_independent_replay_unsupported`).
Use separate single-instrument captures. The host does not admit a combined replay
under semantics that later singleton evaluation cannot reproduce. All-declared-context
contracts retain their explicit declared batching semantics.

The local subprocess fixture is Rust, compiled against the current locked test
artifacts without a nested Cargo command or network access. It grants no production
sandbox qualification and does not require a Python runtime or language-policy exception.
