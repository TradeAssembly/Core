# Provider management V7

V7 is an additive, paper-only installed-node transport for provider observations
and observed-entry position management. Existing V1–V6 command families keep
their original wire versions. V7 accepts only `provider_management`, with a
closed `management` payload and these intents:

| Intent | Action | Resource kind |
| --- | --- | --- |
| `observe_settlement` | `strategy.order_plan.settle` | `strategy_order_plan` |
| `register` | `strategy.position.register` | `strategy_managed_position` |
| `advance` | `strategy.position.advance` | `strategy_managed_position` |
| `submit_close` | `strategy.order_plan.submit` | `strategy_managed_position` |
| `reconcile_close` | `strategy.order_plan.reconcile` | `strategy_managed_position` |
| `settle_close` | `strategy.order_plan.settle` | `strategy_managed_position` |
| `inspect` | `strategy.position.inspect` | `strategy_managed_position` |

The runner, Reach command, preparation, mutation binding and receipt must all use
V7 together. Original bindings have a separate namespace and schema. Scope,
owner, enrolled source, immutable original order/account and current policy
checks remain enforced. `authorityRef` is provenance only. A transport receipt
never grants execution; close submission still crosses normal independent C5.
Callers cannot submit quantities, prices, provider responses, fee finality or
account identities through these management intents.

An observation returns a historical non-atomic receipt. Reported and accepted
facts are separate: a disputed report can retain an earlier accepted projection
without making it currently usable. Raw account snapshots, account references,
provider account identifiers and raw read payloads are omitted. Account coverage
and read hashes do not establish atomicity, final settlement or zero debt.
Unavailable fees/cash remain unavailable. `cashReconciled` is false and
`releasedHold` is zero.

Position evidence uses an explicit `observed_v2` tag. Coherent legacy entries
remain on V6. Register, Advance and Inspect may carry only the sealed position
image returned by that child command. Observe and close settlement omit position
images; they do not reload a later position and label it as the original outcome.
Submit and lookup carry the original child submission projection. Historical
inspection may report a disputed entry after entry pause or broker disable; it
does not grant another read from the broker or another close.

Every protected V7 outcome uses durable original binding and dispatch state.
After dispatch, failure to seal an outcome becomes uncertain. Resume retrieves
only the original sealed snapshot or uncertainty; it does not blindly repeat an
observation or financial action. A fresh, explicitly authorized reconciliation
command is separate. Relay must protect every recorded V7 uncertainty from
redelivery and allow independent lookup to drain it. Signed preparation,
source-revision, request equivalence and acknowledgment recovery remain required.

The journal is the owned managed-position namespace, bounded to 100 entries.
Larger histories refuse explicitly; no paging or complete broker-accounting
claim is made. Its V7 projection includes tagged entry evidence and the original
journal digest.

This transport does not establish fee-aware BTC ownership. Received-asset fees
require separately verified original-entry-attributed net quantity; gross fills
and aggregate holdings alone cannot prove a fully closable lot. Conditional
cash capacity with unknown total debt also requires a separately selected policy
and sufficient evidence. V7 does not weaken V1 funding or metadata guards, fill
missing option facts, or make an unavailable provider capability available.
