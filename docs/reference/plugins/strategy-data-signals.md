# Strategy-bound data signals (development)

The versioned `tradeassembly.data_strategy` evaluate-substep configuration binds
an immutable StrategyVersion to a typed pipeline and generic entry/exit rules.
It is source functionality under development, not a released trading engine.
The existing strategy create, draft-edit and publish paths persist this configuration.
Explicit user authority is still required for publication.

The object has `version: 1`, `nodes`, `bindings`, `tradeInstruments`, `entry` and
`exit`. Nodes and bindings use the existing compiled-data-plan structures.
The graph revision and evaluation mode are selected at plan preparation rather
than embedded in the strategy, avoiding a circular strategy-hash/graph identity.
Only one enabled nonoptional evaluate substep with this configuration is accepted;
additional evaluate configuration is rejected rather than ignored.

Rules are closed tagged objects:

- `compare`: `left`, `op`, `right`; numeric greater/greater_equal/less/less_equal,
  or numeric/boolean equal. Values are never coerced between booleans and numbers.
- `is_true`: boolean `value` operand.
- `all` / `any`: nonempty `rules`; `not`: one `rule`.

Operands are `number` with a finite exact-range `value`, `boolean` with `value`,
or `output` with explicit `node`, `port`, `instrument`, and `sample`
(`current` or `previous`). Output values must be primitive numbers or booleans.
Object/array field extraction must be a declared plugin operation, not an
implicit Core conversion. Rules are bounded to 64 nodes and eight nested levels.
No indicator formula is implemented by this rule layer.

For a cross above, use ALL(previous left <= previous right, current left >
current right). A cross below reverses those comparisons. Previous means the
immediately preceding completed interval on the referenced output node. Gaps,
wrong timestamps, ambiguous points and malformed types reject the step. Missing
initial history and plugin warmup return explicit `warming_up`; NOT and boolean
short circuits cannot turn missing data or errors into false/true.

`tradeInstruments` is an explicit target list independent of the instruments
referenced by rule operands. For example a BTC data output may gate a SPY target.
This identifies signal intent only; it does not establish asset eligibility,
brokerage support, sizing, risk clearance or order authority.

## Product and agent entry points

`POST /product/strategies/data-plans/prepare` accepts:

```json
{
  "scope": {
    "strategyId": "owned-strategy",
    "strategyVersionId": "published-version",
    "strategySpecHash": "exact-version-hash"
  },
  "capabilityGraphRevisionId": "current-saved-graph",
  "mode": "paper",
  "idempotencyKey": "prepare-1"
}
```

The host loads the pipeline and rules from the exact owned version, verifies all
packages and graph bindings, and persists the immutable compiled plan. It never
accepts a caller replacement pipeline. Host-only plan/evaluator calls enforce the
same configuration match. Mode selects data evaluation context, not trading
permission. The MCP equivalent is `tradeassembly.strategy.data_plan.prepare`.

`POST /product/strategies/data-runs/step` accepts:

```json
{
  "step": {
    "runId": "explicit-isolated-run",
    "planRevisionId": "prepared-plan",
    "window": {"eventStartMs": 0, "eventEndMs": 86400000, "asOfMs": 86400000},
    "expectedCheckpointSha256": null
  },
  "idempotencyKey": "step-1"
}
```

The MCP equivalent is `tradeassembly.strategy.data_run.step`. Both use the
existing object-authorization and command paths. Every step revalidates immutable
strategy/graph/package/configuration bindings. Responses include an evaluation
receipt and `executionAuthorityGranted: false`.

The receipt's `signals` contain each frame's entry/exit readiness/value, explicit
trade targets, event/known/arrival times and contributing point hashes. Known time
includes prior samples as well as current samples. The last complete frame is
committed atomically with plugin state, input cursors, signals and the idempotent
receipt under the evaluator's existing lease/fence/CAS. Retrying an identical step
returns the original receipt; conflicting checkpoint or request identity rejects.
Older experimental checkpoints without required previous-frame history require
explicit replay. Legacy strategy checkpoints and plugin SDK wire contracts are
unchanged.

## Current integration boundary

The product/agent signal paths share the same bounded evaluator in paper, live
and backtest data contexts. Real calculation subprocesses are exercised against
explicit synthetic acquisition fixtures, including pipeline composition and
batch/incremental parity. This is not actual provider acquisition or broker
acceptance. The old price compiler explicitly rejects this configuration, even
when a legacy price expression is also present, so it cannot silently trade a
partially interpreted strategy.

Backtest accounting/manifests, durable execution activation/ticks, allocation,
sizing, pricing, risk, broker compilation and synthetic exits still need to
consume these signal receipts through their existing authorization boundaries.
Until integrated, those old paths reject typed-data strategies. No release,
automatic activation, order placement or implicit provider fallback is provided
by these routes.
