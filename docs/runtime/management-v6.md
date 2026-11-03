# Finite managed-position transport V6

This is the engineering contract implemented by Core. It is not a statement of
release, provider coverage, hosted deployment, or full product qualification.
The existing V1–V5 operation families keep their wire versions and behavior.

V6 carries `manage_position` with one closed management variant: `register`,
`advance`, `submit_close`, `reconcile_close`, `settle_close`, or `inspect`. It
accepts paper mode only. Every variant binds the original strategy scope and
owned entry. Quantity, price, provider/account identity, posting data and
execution authority are derived by Core; callers cannot override them.

| Variant | Action | Authority |
| --- | --- | --- |
| register | strategy.position.register | Existing order-plan management policy |
| advance | strategy.position.advance | Existing order-plan management policy |
| submit_close | strategy.order_plan.submit | Existing submission policy and separate current C5 |
| reconcile_close | strategy.order_plan.reconcile | Existing order-plan management policy; original-order lookup only |
| settle_close | strategy.order_plan.settle | Existing management policy plus exact provider settlement-read capability |
| inspect | strategy.position.inspect | Authenticated owned historical read; no financial C3/C5 grant |

The command, Reach envelope, preparation, binding and receipt versions must all
be V6. The binding resource kind is `strategy_managed_position`. Results use
`tradeassembly.management_snapshot.v1`; they report historical outcomes and
never grant execution authority. `core.runner.management@6.0.0` is the additive
compatibility descriptor. The older data-only descriptor remains unchanged.

## Original position and current effects

Registration requires the entry's original C5 activation, exact strategy version,
graph, account, published exit policy and final attributable fill. A different
activation sharing the same configuration cannot adopt that entry implicitly.
Candidate membership and entry pause do not remove ownership of an open lot.

Effects recheck current run controls, original source identity and permissions,
owned remaining quantity, fresh account/metadata/quote inputs and the actual
execution window. Close admission uses separate current C5 authorization and
normal reservation/submission handling. A pending close retains its original
collection and occurrence bindings. An existing ambiguous/open submission is
reconciled using the separate original-order lookup operation, never submitted
again under a fresh outer key.

Historical inspection checks authenticated ownership, exact scope, position,
original entry intent and its pinned settlement receipt. It does not renew
trading permission, quotes or active execution authority. A stopped run or a
revoked broker connection does not by itself prevent an owned historical read.
Signed transport still requires the enrolled Core source and exact owner/resource
custody; a caller-provided source hash is not source verification.

## Immutable outcomes and uncertainty

Original command bindings and sealed outcome snapshots are stored separately.
Same-command reporting reuses the sealed original result without acquiring new
provider data. If the final receipt head is lost after sealing, recovery preserves
the original snapshot timestamp and receipt hash.

An expired dispatch fence without a sealed outcome reports Uncertain with
`management_runner_original_outcome_missing_reconciliation_required`. A successful
child whose projection or snapshot cannot be sealed reports Uncertain with
`management_runner_outcome_seal_failed_reconciliation_required:<reason>`.
Neither condition proves that no effect occurred. Retrying the command must not
repeat its child effect. Separate authorized reconciliation can observe the
original close and settle its actual final outcome.

Transport implementations must protect **all** recorded V6 Uncertain outcomes,
including reads, from ordinary claim redelivery. Retrying the original record
acknowledgment is permitted. A recorded uncertain close must not block a separate
lookup/settlement command. Accepted live leases without a recorded outcome retain
their explicit resume behavior; they are not fabricated no-effect results.

## Journal scope and limits

Mutation snapshots contain an empty journal, `journalIncluded:false` and
`journalCompleteness:not_included`. Inspection includes the complete local
managed-position journal only when it contains at most 100 entries. Larger
journals refuse with `management_runner_journal_paging_unsupported`; they are
never truncated. Cursor/limit fields are unsupported. This is not a complete
Core trade, research, notification or hosted archive projection.

V6 does not itself add automatic remote background management, next-session
rollover, broker cancel/amend, model-estimated indicators, or live execution.
Those paths require their own explicit contracts and qualification.
