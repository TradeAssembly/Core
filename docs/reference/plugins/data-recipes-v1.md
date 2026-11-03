# Verified recipe data plans

Recipe documents compose admitted native data operations. Core binds ports,
parameters and contexts; plugins perform calculations. Preparing or evaluating a
recipe does not publish a strategy, activate a runner or authorize an order.

## Versions and bounded artifacts

Package envelope `3` adds `recipes` entries containing `recipeId`, a byte-digested
package-relative document and `canonicalSha256`. Package envelopes `1` and `2`
remain closed. Outer plugin request/response wire `1` is unchanged.

A `tradeassembly.data_recipe.v1` document declares `recipeId`,
`semanticsVersion`, `parameterSchema`, optional `parameterDefaults`, typed
`inputs`, native `nodes`, `bindings` and typed named `outputs`. Native nodes name
an exact plugin, operation and semantics version; `self` binds this verified
package. Parameters are tagged constants or references to declared parameters.
Ports name an external input or a node output. A bounded parameter-only `choose`
port uses `parameter`, `equals`, `when_true` and `when_false`. It can explicitly
route a zero-delay request directly to its input. There is no executable
expression, script, implicit lookup, hidden acquisition or indicator formula in
Core. Document V1 nodes are native operations; recipe instances compose through
explicit typed program dependencies.

Existing document, aggregate, schema-work and path/symlink protections cover all
recipe documents, including during an unrelated operation lookup. Packages allow
at most 256 recipes. A recipe declares at most 32 native nodes and 64 named
inputs/outputs. The final expanded plan, including base acquisition nodes and all
instances, retains the existing 32-node limit. Cycles, dangling references,
duplicate bindings, incompatible schema/context, unknown code fields, unsupported
versions and unsafe numeric constants reject preparation.

## Product contract

`tradeassembly.strategy.data_recipe.prepare` maps to
`POST /product/strategies/data-recipes/prepare`. Its body contains owned
`scope`, closed `program` and `idempotencyKey`.

The program version is `tradeassembly.recipe_plan.v1`. `baseRequest` is an
ordinary pipeline request naming a current graph revision, mode, base nodes and
capture bindings. Each instance declares its own ID, owned plugin instance,
exact package/recipe/semantics, parameters, native graph-node/context bindings,
typed input sources and selected output names. An input source names a base
pipeline node/port or another recipe instance/output. Final `outputs` choose the
requested program outputs. Core resolves dependencies; it never grants an
operation merely because a recipe names it. Two calculated instances must use
distinct capability nodes and retain independent native state. Direct aliases may
share an underlying data port without duplicating invocation.

The result contains an immutable plan and `executionAuthorityGranted: false`.
`recipeSource` records the normalized program, package/manifest/context pins,
separate document and canonical recipe hashes, and resolved named output ports,
types and instrument contexts. Currentness re-materializes the program from
verified artifacts and current bindings. It does not trust caller-authored
provider evidence or provenance claims.

Explicit pipeline `selectedOutputs` opt into compiled plan
`tradeassembly.compiled_data_plan.v5`. Selected ports determine
`selectedSourceHistory` and readiness; all selected outputs must be ready.
Per-output V3 history remains intact. Empty output selection retains the older
plan versions and readiness behavior. Selection does not remove other explicitly
provided base work; recipe materialization emits only its needed native nodes.

`tradeassembly.strategy.data_recipe.step` maps to
`POST /product/strategies/data-recipes/step`. It accepts a closed `EvaluationStep`
and `idempotencyKey`, requires a current owned plan with recipe provenance and
uses the existing bounded evaluator and atomic state/output/cursor transaction.
It returns a data receipt and `executionAuthorityGranted: false`. Use the same
transport and original key for a retry; changed authority or arguments reject.
A receipt may contain data without strategy signals. Existing published-strategy
signal preparation/evaluation remains a separate contract.

Syntax and request-bound errors use HTTP 400. Currentness, ownership,
configuration, dependency and preparation conflicts use 409. Compilation errors
use stable code `data_recipe_plan_compile_failed` with typed `diagnostics`.
Other failures preserve their stable recipe/package/plan error codes. MCP
failures have `isError: true`; absence or warming data is never converted into a
signal or selected instrument.

## Qualification boundary

The owning synthetic fixture exercises public preparation/evaluation, a real
Rust child process, host-admitted capture, fresh service/package-store recreation,
idempotent retry, bad parameters, instance cycles, contextual diagnostics and
cached-read rejection after recipe tampering. Its test sandbox is not production
OS-sandbox qualification. It proves no authenticated provider coverage or broker
execution.

Default-pack recipe adoption and composite numerical/state proofs remain separate
from this interface: selected-output Ichimoku histories, zero-delay profiles,
candle period zero, previous-candle rules, two instances, per-instrument reset,
near-limit expansion and mandatory production sandbox throughput must be exercised
with the exact adopted package. Generic screening, calendar-relative scheduling,
research accounting, activation, sizing/risk/orders and synthetic exits are not
provided by the recipe data API alone.
