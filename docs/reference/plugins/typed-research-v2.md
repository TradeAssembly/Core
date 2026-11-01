# Captured typed research and explicit recomputation

Typed research evaluates an owned strategy data plan against admitted captured
inputs, then passes the resulting signals to the shared simulator. Results retain
the evaluator receipts, checkpoints, signal history and equity series. Comparison
and robustness consume those verified results rather than constructed trade rows.

Availability depends on the released Core build, compatible plugin packages,
current permissions and actual data coverage. Installing a calculation package
does not establish provider access, historical coverage or trading eligibility.

## Supported simulation profile

The initial V2 profile is one instrument, long or flat, with an explicit quantity,
cash capital, no borrowing, and a unit contract multiplier. It supports equity,
ETF and fund quantities in shares or units, and a restricted `crypto_spot` profile
in `base_asset` units. Instrument, dataset and data-plan families must agree.

Completed-bar signals execute at the next bar's open. The configuration declares
whether fill-series timestamps denote bar starts or ends. Original timestamps,
source receipts and host availability times remain evidence. Warmup remains
unavailable signal data and cannot create an order.

Prices, quantities and the fill-series volume use six decimal places. Crypto
uses full fills and requires exactly representable cash, adverse prices and fee
products. Finer volume and unsupported fractional cash products refuse. This
profile does not establish compatibility with a particular venue's precision or
historical response. Nonpositive crypto fill prices refuse.

Fees, spread and slippage are explicit simulation assumptions. Results are
simulated, not broker observations. Retrospective capture replay does not prove
that every input was available at its historical event time.

Required custom sizing, risk, stop, screened-selection and session-trigger
semantics outside this profile refuse. Full financial-stage parity needs separate
qualification. Sharpe requires a qualified return-sampling and annualization
policy; a benchmark requires an explicitly supported verified series. Missing
metrics remain unavailable.

## Immutable inputs and result versions

A V2 backtest binds the strategy version/specification, capability graph, data
plan, admitted source captures, fill dataset, time grid and simulation model.
Source replacement, arbitrary executable configuration and silently ignored
parameter overrides are not accepted.

V1 configuration, manifest, result and report contracts retain their original
wire behavior. Typed fields require their explicit V2 configuration/manifest/
result versions and report V3. Explicit nulls or new fields cannot silently
upgrade an old wire version.

Report V3 exposes the genuine frame equity and drawdown series. Comparison uses
every verified frame. Robustness binds the original manifest, result, dataset and
report, and uses actual completed supporting runs for declared studies. The
current robustness trade-return projection refuses marked open trades because
its trade contract does not distinguish an open mark from a realized return.

## Recompute is an explicit keyed mutation

Recomputation invokes plugins and saves isolated evaluation progress. It requires
the actual current owner and normal `backtest.run.execute` authority for the
original `backtest_run`. Paper account authority and backtest computation context
are separate concepts.

| Operation | HTTP | MCP |
| --- | --- | --- |
| Start | `POST /backtests/{runId}/recomputations` | `tradeassembly.backtest.recompute.start` |
| Advance one bounded step | `POST /backtests/{runId}/recomputations/{id}/advance` | `tradeassembly.backtest.recompute.advance` |
| Inspect without computation | `GET /backtests/{runId}/recomputations/{id}` | `tradeassembly.backtest.recompute.inspect` |

Mutation bodies contain one `request` object. Its closed command schema is
`tradeassembly.typed_backtest_recompute_command.v1`:

* Both commands require `schemaVersion`, `kind`, `scope`, `runId` and an explicit
  stable `idempotencyKey`. Scope contains the exact owned `strategyId`,
  `strategyVersionId` and `strategySpecHash`.
* Start uses `kind: "start"` and the original `expectedManifestHash` and
  `expectedResultHash`.
* Advance uses `kind: "advance"`, `recomputationId` and `expectedStateSha256`
  from the current status. Each admitted turn uses its own stable key.

The command accepts no caller identity, credential, replacement source, price,
quantity, window or clock override. Start records a stable generation. Advance
retains the exact inner step before invocation and performs at most one bounded
step. Original results and their evaluation generations stay immutable.

The CLI accepts the closed command DTO in a bounded JSON file and adds the HTTP
wrapper. It does not mint replacement keys:

```sh
tradeassembly backtests recompute start --request-file start.json
tradeassembly backtests recompute inspect BACKTEST_ID RECOMPUTATION_ID
tradeassembly backtests recompute advance --request-file advance.json
```

Old typed replay entry points return
`typed_backtest_recompute_command_required` before plugin or checkpoint work.
Legacy V1 replay retains its existing behavior. Delegated agent contexts and
Relay research transport require their own verified authority and finite
transport contract before enablement.

## Recovery and refusal

Retries recover the original generation, inner request, committed receipt and
sealed outcome. A lost response cannot authorize another window or replacement
source. A changed payload under the same key conflicts. A fresh key with stale
state conflicts. Foreign ownership refuses before cache or receipt recovery.

A permission denial remains denied. Once current permission is restored, an
explicit fresh Advance with the current state hash can continue the same retained
inner step. It does not rewrite the denied command. Inspect supplies the current
state and pending request without invoking a plugin. This also supports recovery
when an earlier outer error has already been sealed.

Busy, stale source, expired lease, mismatched checkpoint and semantic mismatch
remain explicit failures. Completion requires the recomputed semantic and
economic content to match the original. Its proof retains the new evaluation
receipts without replacing the original result or pretending identity-bearing
hashes are interchangeable.

Synthetic subprocess tests establish their stated calculation and recovery
paths. They do not establish real provider access, historical entitlement,
production authentication, venue execution, investment merit or a published
release. Strategy examples and simulation results are educational, not investment
recommendations.
