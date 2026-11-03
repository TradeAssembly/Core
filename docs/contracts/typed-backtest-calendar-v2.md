# Explicit typed research calendar evidence

Before publication, typed-backtest binding V2 couples two required declarations:
`warmupObservations` and the closed `calendarEvidence` object. Existing binding
V1 rejects either new field, including null, and preserves its bytes and provider
session semantics. V2 has no default or fallback calendar mode.

The evidence object requires exactly these fields:

```json
{
  "kind": "declared_dataset_calendar",
  "calendar": "crypto_24x7",
  "timezone": "UTC",
  "purpose": "research_simulation_only",
  "acknowledgeResearchAssumption": true
}
```

`kind` is `declared_dataset_calendar` or `provider_bar_sessions`. Purpose accepts
only `research_simulation_only`. Declared mode requires the acknowledgement to
be true. Provider mode requires it to be false, since that mode relies on the
original source session evidence rather than this assumption. Unknown fields,
missing fields, null evidence and other purposes refuse. Calendar and timezone
must match the immutable accepted dataset and the published clock policy. The
policy is included in configuration/manifest identity and retained recomputation.

## Provider bar sessions

`provider_bar_sessions` retains the existing source-session validation. Every
bar needs its original nonempty source reference and matching calendar; its full
interval must be inside the provider session. Published timezone/calendar/day/
window checks remain in force. Missing sessions refuse; they never select the
model below automatically. No new provider calendar collection or receipt is
inferred from the declaration.

## Declared dataset calendar

The initial supported model is deliberately narrow: one `crypto_spot` instrument,
an accepted CryptoSpot bars dataset with calendar `crypto_24x7`, timezone `UTC`,
RFC3339/UTC normalization, daily `1d` granularity and exact86400000ms intervals.
No other asset class, timezone, granularity or calendar model is qualified by
this version. These are explicit model inputs, not inferred defaults.

The published clock must match that calendar and timezone, list all seven days
exactly once, contain one00:00–24:00 window, and retain FollowCalendar holiday/
early-close policies. Session triggers, a schedule-capability requirement and
an eligibility expression are unsupported in this model. With that full UTC
clock every admitted bar's entire interval is covered. There is no weekday,
holiday, host-timezone or market-open inference.

The existing verified dataset/hash/source/owner and immutable plan/capture
checks remain required. Dataset window endpoints must equal the typed binding;
all bars must be present in exact increasing interval order with the declared
start/end timestamp basis and instrument. Future, shifted, missing, duplicate
or out-of-window intervals refuse. Actual acquisition/arrival clocks and the
current trusted cutoff remain unchanged. Existing contradictory provider
session evidence also refuses.

Source bars are never modified: absent `BarObservation.session` remains absent.
The report labels this an explicit research calendar assumption, identifies
whether provider bar sessions were used, and sets both venue-eligibility
qualification and trading-eligibility grants to false. This is simulation
eligibility only; it cannot be used to bypass paper/live order-plan guards or
claim venue availability or historical point-in-time knowledge.
