# Selection-bound data plans

This candidate connects an immutable instrument selection to the existing typed
pipeline evaluator. It produces signal evidence. It does not size or submit an
order, expand a broker allowlist, dispatch a scheduler occurrence, or reconcile
managed positions.

A published evaluate substep can use the separate
`tradeassembly.selection_strategy` configuration key. Its `schemaVersion` is
`tradeassembly.selection_strategy.v1`. The existing `tradeassembly.data_strategy`
version 1 representation is unchanged.

The template declares named `selections`, each containing
`marketRequirementRef` and `selectorRef` from the same published StrategySpec.
`tradeSelectionRef` chooses which selection supplies the traded subject. Each
node has its capability-graph node ID, parameters, output `context`, and named
`inputContexts`. Context kinds are:

- `subject`: the explicitly chosen selected member, with an explicit `intervalMs`.
- `selected`: all members of a named `selectionRef`, with an explicit interval.
- `literal`: an explicitly authored typed data context, which can differ from the
  traded subject. It does not make its instruments eligible for orders.

Rules use the existing comparisons, boolean composition and current/previous
samples. An output operand's instrument is explicitly `{ "kind": "subject" }`
or `{ "kind": "literal", "instrumentId": "SPY" }`. Indicator calculations stay
in installed plugins; the host does not add formula-specific compiler branches.

## Prepare and evaluate

`POST /product/strategies/selection-plans/prepare` and MCP tool
`tradeassembly.strategy.selection_plan.prepare` take an outer `selectionPlan`
and `idempotencyKey`. The closed command contains:

| Field | Meaning |
| --- | --- |
| `scope` | Exact owned strategy ID, version ID and spec hash |
| `capabilityGraphRevision` | Current graph for that exact version and mode |
| `mode` | Explicit paper, live or backtest data context; no trading authority |
| `subjectInstrumentId` | One member of `tradeSelectionRef` |
| `selectionReceipts` | Screened template alias to host-issued selection ID |
| `sourceCaptures` | Root graph-node ID to an admitted data capture, when used |

Screened selections must be complete and nonempty, current, untampered and bound
to the same strategy version, selector, graph and mode. Empty, partial or failed
screens cannot produce a plan. No fallback member is chosen. Explicit normalized
static selectors use the same downstream plan path without a screener receipt.
Other selector producers are unavailable through this new bridge; existing
legacy selection paths remain intact.

Each preparation chooses one subject explicitly. Batch scheduling over an entire
set and automatic re-selection are separate integration work. The host pins the
selection receipts or static definitions, subject, resolved contexts and host
availability time into plan identity. Repeating the same domain command/key
returns its original plan and original host time. Reusing that key for a changed
subject or binding is a conflict.

The returned plan is consumed by the existing data-run step HTTP/MCP surfaces.
Source capture coverage, plugin contracts, warmup, causal state, output schemas,
leases and atomic checkpoint/result storage still apply. Selection knowledge
cannot be backdated: a step whose knowledge cutoff precedes the bound selection
is refused, and emitted signals retain selection availability and lineage.
Changing selection evidence changes plan identity; this candidate does not
silently migrate indicator state between selections.

Plan preparation requires the separate C3
`strategy.selection_plan.prepare` configuration-write policy. It grants no C5
broker execution authority. Existing positions and pending orders are not removed
by preparing or evaluating a new candidate selection. Continued exit monitoring
and selection-to-order admission still require their separate runtime integration.

Current qualification is tracked by focused product/process tests. Real provider
coverage, historical point-in-time universe replay, automatic scheduler dispatch,
order sizing/risk/execution and remote Reach adoption are not established by this
bridge. Historical bar computation using today's selected membership is not a
qualified historical strategy backtest.
