# Typed provider bars

The SDK's `market_data` module defines `schema://tradeassembly/ohlcv-series@1`
and a bounded codec for provider-normalized OHLCV observations. This is a data
contract, not a provider implementation, brokerage eligibility check or indicator.

`OhlcvBar` requires open, high, low, close and volume. Provider-reported VWAP,
trade count and session evidence are optional and omitted when unavailable.
The codec preserves them without calculating substitutes. Prices may be negative
for applicable instruments; OHLC ordering, finite values, nonnegative volume and
valid session bounds are checked. Asset-specific constraints still belong to the
selected provider and instrument context.

`ProviderBar` carries instrument ID, source event ID and the existing explicit
provenance fields. `bar_batch` validates requested instruments, a positive fixed
interval, ordered unique observations, bounded row count (1–256 requested maximum),
window bounds, known/arrival times and as-of cutoff. Empty batches are explicit
no-data results. Rows are not silently sorted, filled, adjusted or discarded.
Content identity is computed using the shared exact-range canonicalization.

## Provider adoption

Use a separately versioned provider operation rather than changing an existing
operation's response shape. Its verified operation-data contract declares no
inputs and an OHLCV output from `ohlcv_output_port(interval_ms)`, along with exact
applicability, parameters and semantics. Declare the OHLCV and evaluation-response
schema references in the manifest, and package their verified schema/contract
artifacts through data package V2. The existing wire V1 envelope, capability
registry, graph admission, entitlements and scoped credential custody continue to
own invocation. This metadata grants no execution authority.

The operation receives `DataEvaluationRequest`. The SDK's
`decode_operation_payload` separates existing host-injected configuration from
the closed evaluation request. A provider wraps the returned `DataBatch` in its
declared `DataEvaluationResponse` output port. Root provider state is currently
unsupported: use explicit bounded windows/pinned source revisions rather than
uncommitted pagination or process-global progress.

Existing host payload selection uses HistoricalData for operation IDs containing
`bars` and Operation for ordinary data operations. An adapter must decode its
actual declared operation's carrier; the codec itself has no broker/network code.

## Availability and historical data

An event's closing time does not establish its publication or observation time.
Providers must supply factual `knownAtMs`, `arrivedAtMs`, source event and revision
metadata. The codec never assigns closing time to missing availability fields.
A fetch completed after the requested cutoff cannot be relabelled as earlier
knowledge. An explicit acquisition/pinned-snapshot phase followed by evaluation
at/after acquisition avoids that race. Current historical bars alone do not
establish point-in-time availability; research must retain its dataset-as-of and
limitations. A modeled publication-time policy would need separate explicit
pinned semantics rather than an undocumented substitution.

## Consumer integration evidence

The independent local default calculation package provides `data.ohlcv_field`
for an explicit open/high/low/close/volume projection to the numeric series
schema. This calculation remains outside Core. Optional provider fields are not
implicitly advertised as required projection fields. A direct OHLCV-to-numeric
schema mismatch continues to fail compilation.

The opt-in saved-plan process proof uses typed synthetic OHLCV acquisition,
verified packages, the ordinary graph-save HTTP route, immutable plans, the
shared evaluator and real calculation processes for projection→EMA→EMA. Two-bar
batches and single-bar steps match values, semantic hashes and final state; no
broker dispatch claim is created. It uses a test process sandbox and explicit
fixtures, not a real market-data account or production OS sandbox qualification.
Real provider packaging/acquisition, authentication, product strategy paths and
trading acceptance remain separate integration work.
