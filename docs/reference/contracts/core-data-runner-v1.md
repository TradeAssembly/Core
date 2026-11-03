# Data-computation runner V1

`tradeassembly-core-runner --data-only` adds `dispatch_data` with a closed
`tradeassembly.core-data-runner.v1` command and `data_receipt` with workspace_id
and command_id. Read-only `data_binding` resolves current resource/policy facts
for an exact command, without admission or permission. Existing backtest/paper V1 command enums remain unchanged and
reject new operation kinds. `core.runner.data@1.0.0` advertises this separate
local computation protocol. It does not grant Relay mutations or brokerage access.

Command fields: schemaVersion, coreRelease, workspaceId, commandId, authorityRef,
idempotencyKey, mode (paper/backtest), submittedAtMs, operation. authorityRef is
provenance only: it never selects an authenticated principal. Current owner,
resource, source and Warden C3 authorization remain those of the configured Core
service. A results/journal source link is insufficient to dispatch a mutation;
Relay needs the separate authenticated one-command authorization adapter.

Finite operation kinds are prepare_strategy, prepare_recipe, evaluate_strategy,
evaluate_recipe, evaluate_screen and prepare_session. They carry the corresponding
owned scope/graph/capture, typed recipe, evaluation step, screen command or calendar
plan command. No arbitrary route, shell, SQL, credentials, provider response,
activation or order operation is accepted. Unknown versions/fields, unsafe numeric
forms and oversized/deep payloads reject. Maximum command bytes262144, receipt
bytes307200. Literal release matching and mode/source/window checks remain strict.

Receipts bind original workspace/command/canonical command hash, release and Core
revision where the host executable installed build identity, state, actual bounded
result/error, observation time and previous receipt revision. A library test with
no installed build identity reports null rather than inventing a source revision.
The normal executable installs its compiled identity before accepting commands.

Admission atomically records command, idempotency binding, accepted receipt and its
immutable revision before calling the owning public Core API. Final commit retains
the original accepted revision. An upstream ambiguous failure records uncertain;
it is not an empty successful result or proof of no effect. Retry preserves the
original command and underlying C3 key. Completed/failed results are stable;
current owner/package/graph/source checks still precede cached results. An accepted
or uncertain job cannot silently resume under a different Core build revision.

Plan projections preserve exact source/plan identities. Evaluation projections
preserve actual ready/warming_up/no_new_data/waiting_for_input status, checkpoint
hash/generation and processed/signal-frame counts. Completion means the bounded
computation request completed; it does not mean research success, a strategy entry
or a broker fill. Full outputs remain in the owning original receipt. Selection
and calendar projections preserve their typed original evidence. A calendar plan
still reports no queued execution. All results have executionAuthorityGranted:false.

The owning evaluator now exposes an internal read-only receipt inspector sharing
the same hash/checkpoint/request validation as normal replay. Runner summaries are
checked against original source artifacts rather than treated as independent truth.
Source tampering/currentness errors do not rewrite original historical receipts.

Still separate: authenticated Relay mutation-authority projection/readback, remote
connected-runner handoff, queued session execution/retries, selection/warmup binding,
research accounting and journal pagination, sizing/risk/order/synthetic-exit stages,
production provider coverage and signed combined release qualification. Native
fixtures and local process protocol tests do not establish these capabilities.

Data-only startup opens the configured service without resuming local scheduler
workers and refuses legacy bridge/paper operations. Conversely, legacy startup
refuses the new data operations; it retains its existing behavior. Consumers must
select the explicit mode before startup. No metadata query starts a worker.
