# Selected-plan creation over the data runner

This candidate adds exactly `prepare_selection_plan` with nested closed
`selectionPlan` to the finite computation channel. Its fields and product behavior
are documented in [selection-bound data plans](../../selection-bound-data-plans.md).
It is not an arbitrary HTTP/tool proxy, scheduler activation or order operation.

## Version negotiation

Existing six operations continue to require Core command schema
`tradeassembly.core-data-runner.v1`, mutation-binding and receipt V1, Reach command
`tradeassembly.relay.reach-data-command/v1`, and signed preparation
`tradeassembly.relay.reach-data-preparation/v1`.

Selected-plan creation requires all of:

- Core command: `tradeassembly.core-data-runner.v2`
- Core mutation binding: `tradeassembly.core-data-mutation-binding.v2`
- Core receipt: `tradeassembly.core-data-runner.receipt.v2`
- Reach command: `tradeassembly.relay.reach-data-command/v2`
- Signed preparation: `tradeassembly.relay.reach-data-preparation/v2`

V1 plus the new operation, V2 plus an old operation, or mixed coupled versions
reject. Existing source-registration and signed node claim/accept/record protocols
are unchanged; they do not grant the new mutation. The transport delivery retains
its explicitly versioned nested command and receipt. Counterpart capability/source
adoption is required; there is no downgrade to a guessed old operation.

## Authority and identity

V2 mutation binding includes `resourceKind:strategy_selection_plan`, computed from
the current Core action definition. V1 omits that field and requires it absent.
The action is `strategy.selection_plan.prepare`. Scope and graph must match the
nested command. The actual compiled revision, current verified owner/source,
canonical command/domain/policy digests, and false execution-authority flag still
apply. Unsupported policy-identity adapters refuse. `authorityRef` remains
provenance, never permission.

Only paper and backtest modes are accepted by the data runner. The nested mode
must match; both use paper-account authority. No live-account promotion, C5 grant,
credential material or broker input is introduced. Existing command/document
bounds, closed fields, original IDs/keys and signature domain remain enforced.

The domain digest includes the command, current selection/static-definition
hashes, resolved program, compiled graph and package/binding/source-authentication
pins. Host preparation time is a later effect result; generating a new time cannot
change the signed command binding on retry. Expired/revoked/changed sources refuse
rather than silently refresh under the original key.

## Results and recovery

The completed V2 result has `kind:selected_plan`: scope, revisionId,
revisionSha256, planSha256, capabilityGraphRevision, mode, typed subject,
selectionSha256 map, preparedAtMs and knownAtMs. It remains non-authorizing. The
original host times/lineage belong to the original committed plan.

Existing accepted/uncertain/final history, leases, fencing and compare-and-swap
storage apply. Loss of final runner-receipt storage after successful plan creation
must recover that original domain plan; it cannot create a replacement with a new
host time. Changed commands cannot reuse the original command/idempotency binding.
Current source/owner/policy checks remain before and after effects. Historical
readback does not assert current freshness or create authority.

Existing V1 `evaluate_strategy` may consume the resulting prepared plan under its
original `strategy.data_run.step` binding. That does not imply automatic scheduler
execution, sizing/risk/order admission, exit management or a released remote
workflow. Actual Core/Relay process qualification is a separate owning gate.
