# Explicit session calendars and occurrence plans V1

The SDK session_calendar module defines `tradeassembly.session_calendar.v1`
(`schema://tradeassembly/session-calendar@1`). A bounded document names its calendar,
timezone, primary-session scope, source revision, actual provider knowledge time
and inclusive covered date range. Every covered date has exactly one explicit
closed reason or UTC open/close pair with early-close metadata. Missing dates are
uncovered, not inferred holidays. The host preserves supplied anchors across DST
and half-days; it does not derive a timezone schedule from a label. This does not
assert brokerage eligibility or coverage of another product's session.

An optional `session_triggers` list in ClockPolicy is additive. Empty lists retain
the old wire form. Each `tradeassembly.session_trigger.v1` declares:

- unique trigger_id; mandatory source calendar capability, ref, timezone, scope
  and maximum age;
- open/close anchor and signed offset_ms bounded to one day;
- missed_run skip or bounded catch_up; finite retry max_attempts/backoff_ms;
- separate execution_window calendar with explicit inward open/close offsets;
- explicit existing signal-requirement/selector targets. Static, expression and
  screened selectors remain distinct valid targets.

Both calendar sources must be mandatory, without fallback, with the exact calendar
response schema and market_clock stage binding. Their current graph operations
must match the declared capabilities. Old evaluators reject session-trigger
strategies until the occurrence-aware lifecycle is integrated; they must not
silently ignore the added policy.

## Host preparation interface

HTTP POST `/product/strategies/session-schedules/prepare`, MCP
`tradeassembly.strategy.session_schedule.prepare`, accept `schedule` plus
`idempotencyKey` under existing owner/command authority. ScheduleCommand is closed:
`scope` (strategyId/strategyVersionId/strategySpecHash), capabilityGraphRevision,
mode, triggerRef, sessionDate, calendarCollectionId,
executionCalendarCollectionId, knowledgeCutoffMs. Calendar documents come only
from current host-admitted collections, not caller-authored provider responses.
The host checks package/schema identities, owner/version/graph/mode/capability,
exact calendar identity, covered date, cutoff, provider-known time, host-known
time and both configured age limits.

HTTP POST `/product/strategies/session-schedules/get`, MCP
`tradeassembly.strategy.session_schedule.get`, accept occurrenceId and recheck
current bindings. Response `schedule` is a
`tradeassembly.session_schedule_summary.v1` with immutable receipt hash,
scope/graph/mode, preparedAtMs and `tradeassembly.session_occurrence.v1`.
The occurrence preserves trigger and source identities, due/latest-start times,
separate permissible execution boundaries and original preparation status.
`queued:false` and `executionAuthorityGranted:false` are explicit: this interface
prepares a durable plan; it does not enqueue work, run indicators, submit an order
or report that a scheduled job executed. Preparation status is not current
execution state. A runner must apply trusted current time before every claim and
effect, and continue managing existing positions independently of entry schedules.

Logical occurrence identity includes strategy/version/spec, trigger and session
date; it excludes changing calendar revision. A different calendar receipt for
that logical occurrence conflicts and requires explicit reconciliation, rather
than creating a second entry opportunity. Exact retry returns the original
receipt and preparation times. Receipt creation uses atomic conditional storage;
currentness checks precede outer command-cache replay. Raw provider payloads remain
private. HTTP domain/currentness errors are conflicts and MCP errors retain
isError:true. This surface does not expand legacy runner V1 wire enums.

## Integration gates still open

Queue dispatch/fenced occurrence claims, retry-attempt lifecycle, selection and
indicator warmup, typed runner transport, sizing/risk/orders, managed-position
exits and public research/journal projections are separate unfinished stages.
Production providers must explicitly implement the calendar schema; existing
single-timestamp calendar-resolution responses are a different contract and are
not silently promoted to complete session coverage. Synthetic fixture calendars
prove contract behavior only; they are not authoritative exchange schedules.
