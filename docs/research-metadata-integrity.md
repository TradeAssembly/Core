# Research metadata integrity

Legacy local dataset and universe descriptors are declarations, not acquired
market data or research results. Their responses are explicitly unverified and
do not infer row counts, content hashes, snapshot locations, symbols, time
ranges, calendar, capital, costs, or execution defaults. Caller-supplied records
remain declarations. They cannot qualify a canonical backtest.

The explicit `immutableSnapshotId` selector returns metadata only after current ownership,
strategy identity and immutable snapshot integrity checks. The content digest
identifies those bytes; it does not by itself establish provider authenticity
or the suitability of the dataset for a research claim.

Completed legacy metadata commands may retain historical fabricated metadata.
The finite HTTP, GraphQL and MCP legacy routes return a sanitized projection
after current ownership and exact retained command binding checks, before the
generic duplicate handler can mutate or replay the old result. The original
command, response, receipt and bytes remain unchanged. The projection digest
identifies the original response, not a dataset. Foreign and missing strategies
receive the same opaque refusal. Canonical dataset ingestion keeps its own
existing durable command path.

This correction does not acquire historical data or produce research results.
Typed backtests still require a verified immutable dataset, pinned evaluator
inputs, explicit execution and cost assumptions, and reproducible outputs.

Legacy broad Studio payload fields such as `snapshotId: latest` and
`datasetId: demo` keep their existing ignored-selector behavior. They cannot
select or verify an immutable snapshot. Conflicting explicit immutable selector
aliases refuse.
