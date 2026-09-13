# Stored journal evidence

`GET /journal/events` reads durable journal records owned by the authenticated
caller. New events carry the authenticated owner (issuer, subject and tenant)
separately from the acting worker. Incoming payload fields cannot choose this
owner. Existing REST authentication requirements apply.

Legacy events without a stored owner use a conservative strategy-owner projection.
Legacy records without a verifiable strategy binding are not exposed. Background
events with no authenticated ownership are not automatically claimed by a reader.

`GET /journal/export` returns schemaVersion1, the visible events, an explicit legacy
projection limitation, and eventsSha256 over canonical JSON of the events array.
This hash verifies exported content, not trading performance or completeness of
all historical unowned activity. `tradeassembly journal list`, `journal export`
and `journal replay` use the same local authenticated installation. Export emits
JSON on stdout for the user or calling agent to save where they choose.

`POST /journal/replay` and `/journal/replay-report` inspect the same stored rows
and return counts by event type. `strategyEvaluationPerformed` is false and
`deterministic` is null: these endpoints do not re-execute a strategy or prove
backtest reproducibility.

`POST /journal/replay-harness` compares these counts with a request shaped as
`{"expected":{"counts":{"strategy.created":1}}}`. Exact counts match with
HTTP200; mismatches return HTTP409. Missing or invalid expected counts return
HTTP400. This comparison is only journal evidence inspection.

Unauthenticated reads return HTTP401. Journal corruption/read failure returns
HTTP503 with a redacted diagnostic, not a successful empty journal. Nested
workspace journal fields preserve an explicit error object on failure.
