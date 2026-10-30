# Shared bounded data evaluation

The host-only `service::data_evaluation::step` evaluates a verified saved plan.
It has no HTTP/MCP route or strategy activation integration in this slice. The
same step function accepts a historical batch or successive event windows. The
host owns scheduling, package validation, invocation authority, leases and atomic
progress. Calculations remain in separate plugins.

## Composition and compatibility

Operation data contract V1 retains its exact declared plugin-dependency binding
rules. V2 introduces explicit named data bindings between independently admitted
operation nodes in the existing capability graph. Invocation prerequisites remain
ordinary operation dependencies; data edges are not recursively expanded as
operation definitions. This permits shared producers and repeated calculations
with separate parameters/state, without treating `EMA(EMA(x))` as a definition
cycle. The compiler still rejects actual node cycles, incompatible schemas and
incompatible time/instrument contexts. Plans containing V2 contracts use plan V2;
V1 plans, package V1, wire V1 and legacy checkpoints retain their versions.

## Evaluation protocol

Plugins declare `schema://tradeassembly/data-evaluation-response@1` as an output
schema and receive `tradeassembly.data_evaluation.v1` inside the existing bounded
invocation envelope. Sources receive a bounded acquisition window. Calculations
receive only the current complete event frame and their own preceding state.
Inputs and outputs carry a schema/shape, instrument, source event identity,
event interval, known time, arrival time, completion flag and provenance. A
series/table schema validates the array of ready values; a scalar schema validates
one ready value. `warming_up` has no fabricated numeric or boolean value.

The host revalidates package/schema bindings and current plan admission. It
allows data read operations only; broker/order effects are rejected. Calculation
operations cannot request credentials, account binding or outbound network
permission. Every invocation continues through the existing invocation port and
its authority checks. Transform responses must be deterministic and replayable.
Data read invocation receipts can be reused across checkpoint lease generations;
checkpoint writes retain their own fencing and compare-and-swap protection.

Sources must be ordered, complete, schema-valid and available by the requested
as-of time. Duplicates are no-ops. Changed committed observations require explicit
replay; out-of-order and unexplained gaps fail. A complete common frame barrier
prevents one source/instrument advancing while another is missing. No implicit
fill, provider substitution, time aggregation or error-to-false conversion occurs.
The host counts valid required inputs and rejects ready results before declared
warmup. Derived identities include semantics and upstream/prior-state hashes;
arrival time is excluded from semantic content identity.

State, output receipt, source ledger and cursor are committed in one atomic
storage transaction, with checkpoint/receipt/fence expectations. Failed plugin
calls, rejected responses, stale workers and failed commits do not advance them.
Retry keys cannot change inputs; cached receipts are integrity checked and still
require a current plan. Leases are released only using the acquired claim.

A step accepts at most 32 nodes, 64 common frames, 256 points per batch, 256 KiB
state per operation and 8 MiB accumulated results. Each invocation is bounded to
15 seconds. A 45-second monotonic step budget is checked between calls, frames
and before commit; a call already running may finish within its invocation
budget before the step rejects. There is no partial checkpoint on budget failure.

## Remaining integration

This is exercised with real package verification/graph/storage and a controlled
invocation-port fixture, not an installed provider/calculation process. The
separate default pack, generic rule operators, product routes, strategy/research
wiring and paper execution still need integration and actual product exercise.
Provider state and activation-partitioned state are rejected until their owning
lifecycle is integrated. Instrument/research-run calculation state is supported.
Fixed equal-interval event frames are supported; session calendars, resampling,
explicit gap policies and revision replay orchestration remain separate work.
No provider coverage, brokerage eligibility or research result is inferred from
this host interface alone.

## Causal state availability and research state slots

Every evaluated calculation node persists host-owned state availability beside
its plugin state and warmup counters in the same checkpoint transaction. Later
frames use the maximum known and arrival times of current inputs and preceding
state when invoking a plugin and stamping outputs. This also applies when plugin
state is unchanged or empty: the host's readiness decision has causal inputs.
A request cutoff earlier than the retained state's availability is rejected.
Unreleased data-evaluation checkpoints/receipts missing required availability
metadata require explicit replay; no availability time is guessed. Existing
strategy checkpoints and old wire versions are unchanged.

ResearchRun-partitioned plugins read and return their state under the SDK's
`RESEARCH_RUN_STATE_SLOT` (`__research_run__`). The host scopes this stable slot by
saved plan, research run and graph node. Raw caller run IDs need not be exposed
to the plugin; distinct runs retain independent state and resume independently.
