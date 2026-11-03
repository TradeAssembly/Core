# Explicit no-trade research warmup

The optional typed backtest binding remains versioned independently inside the
canonical backtest configuration. Existing `tradeassembly.typed-backtest-binding.v1`
requests keep their exact serialized fields and behavior. V1 rejects a supplied
`warmupObservations`, including null. The new closed
`tradeassembly.typed-backtest-binding.v2` requires that integer field and a
closed `calendarEvidence` policy; null, unknown fields and implicit defaults are
refused. The calendar fields were coupled before V2 publication; see
[the explicit research calendar contract](typed-backtest-calendar-v2.md).

V2 permits 0–255 warmup observations and requires the count to be smaller than
the actual full-window frame count. The current full capture/research window is
still at most256 observations, and each evaluator turn remains at most64 frames.
A150-observation warmup therefore leaves at most106 eligible decision bars.
The ordinary next-bar rule still needs a later bar to execute an order.

For example, retain all existing binding fields but set:

```json
{
  "schemaVersion": "tradeassembly.typed-backtest-binding.v2",
  "warmupObservations": 150,
  "calendarEvidence": {
    "kind": "declared_dataset_calendar",
    "calendar": "crypto_24x7",
    "timezone": "UTC",
    "purpose": "research_simulation_only",
    "acknowledgeResearchAssumption": true
  }
}
```

This is a fragment, not a complete request. Pin the actual owned plan, captures,
scope, event bounds, interval, quantity and retrospective cutoff as usual.
The field is part of the immutable configuration/manifest hash. It does not
modify or retarget an admitted run. A different value creates a distinct normal
run; keyed recomputation uses the original admitted value and generation.

All warmup source frames are acquired/replayed, evaluated by the actual typed
publisher, and atomically checkpointed by the existing evaluator. Original
publisher true/false/unavailable signals, clocks and state are retained. The
pure simulation suppresses order admission for zero-based decision indices
0 through149. Index150 is the first eligible decision; its earliest fill is
the open of index151. No warmup decision creates a pending order for the first
eligible bar. Evaluation errors still fail, and publisher unavailability remains
unavailability. The report separately describes no-trade warmup in its provenance;
`warmingFrames` continues to count actual unavailable publisher signal frames.

This research admission rule grants no trading permission, does not infer a
market session/calendar, does not add sessions to source bars, and does not relax
price/quantity/volume precision or current source/owner checks. It does not make
historical point-in-time availability qualified. Provider-session mode retains existing validation. The separately explicit
declared-dataset calendar mode supports only the documented daily research
profile; other model limitations remain unchanged.
