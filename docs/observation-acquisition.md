# Bounded financial observation acquisition

Typed paper configurations can explicitly opt in under
`typedExecution.automaticExecution`:

```json
{"observationAcquisition":"bounded_independent_reads_v1"}
```

Absence preserves legacy single-phase acquisition and its serialized configuration
bytes. Unknown mode versions refuse. Opting in changes the configuration hash;
an existing cycle cannot silently adopt a changed configuration.

The new mode handles only account, instrument metadata and quote collection.
Inputs remain explicit literals or the current traded instrument ID; none can
consume another observation's output. Reads run sequentially through their
original authorized C3 collection endpoints, source bindings and credentials.
Each keeps its actual provider/host timestamps and immutable receipt. This is
not a provider-atomic snapshot or a provider eligibility guarantee. Normal plan
validation still checks account, instrument, currency, coverage and freshness.

Every successful read commits its original phase-specific key and cycle cursor
before the next read. A resumed cycle uses the retained cursor, rather than
reacquiring completed observations. An uncertain outer transport command still
requires the existing supported C3 recovery; this mode does not invent a receipt
or bypass an unresolved command. Source/configuration, entry authority, health,
worker stop/pause and execution window are checked between reads.

A turn starts at most three reads. It stops starting reads after a ten-second
monotonic budget; an already-started read retains its existing fifteen-second
operation timeout. Partial progress is retained. A batch never crosses into plan
preparation, approval or submission. Those remain separately authorized phases.
The source's thirty-second freshness policy is unchanged. Old observations that
expire during a partial turn refuse normally; their timestamps are never renewed.

This mode acquires new inputs before any plan exists. It never reprices or mutates
an approved plan, replaces a reservation, or retries a denied/unknown broker order.
Synthetic fixture qualification and real provider coverage are separate gates.

Worker health validation shares immutable verified package metadata only within
one pure binding read, one provider-selection read, or one post-effect
readiness/due-time read. These are separate rounds: a provider call, next turn,
or independent admission cannot inherit their package snapshots. Registry,
credential, ownership, health and clock reads remain current in every round.
For a persisted, unattempted, due-now execution successor the worker returns to
its normal top-of-loop health gate directly, without first repeating an advisory
health probe. Future wakes, attempted-child backoff and management scheduling
retain their existing checks.
