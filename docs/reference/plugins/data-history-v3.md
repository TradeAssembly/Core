# Versioned history and partition continuity

`tradeassembly.operation_data.v3` is an explicit outer record containing an
unchanged, closed V2 `operation` and a `history` contract. V1/V2 documents and
plan/checkpoint serialization remain unchanged when the new fields are empty.
Legacy contract access rejects V3 instead of discarding its metadata. Package
and canonical contract hashes cover the whole V3 record.

`history.firstReady` maps each output port to every required input port. Values
are bounded observation expressions: `literal`, `parameter`, `add`, `subtract`,
`multiply`, `maximum`, `floor_divide` by a positive constant, and `floor_sqrt`.
Expressions have at most eight nested levels, 64 nodes and 32 terms per list;
intermediate values and resolved history are bounded to 1,048,576 observations.
Missing, fractional, negative, zero final counts and overflowing expressions
reject preparation. They do not execute calculation code.

For aligned intervals, a consumer requiring C ready observations of a selected
producer output with H observations of source history requires H + C - 1 source
observations. Convergent paths take the maximum for each shared source. Each
output keeps its own source-history map; selecting another output can change
the immutable plan identity. Interval conversions remain unsupported. Planning
history does not enlarge invocation, frame, state, graph or time budgets.

`reconstruction` distinguishes `finite_window` from
`checkpoint_or_explicit_origin`. First readiness is not a guarantee that
truncating recursive state to that many observations reproduces an established
run. A run starts at its explicit requested origin or resumes its exact pinned
checkpoint. This contract does not introduce automatic history fetching or
implicit reconstruction of missing checkpoints.

`partitioning` is explicit: `instrument_independent` invokes each instrument
separately, with only that instrument's input points and prior state.
Stateless source acquisition also uses singleton instrument requests and divides
the existing source point budget across them. Stateful V3 source declarations
are rejected during preparation until acquisition state is supported.
Instrument state, required-input counters, availability time and lineage remain
isolated. Explicit input contexts must match that independent context;
cross-context calculations must declare `all_declared_context` instead.
The latter retains one invocation over the full declared context.

The initial unavailable policy is `invoke_complete_frame`. A complete frame
containing unavailable required input clears the affected partition's prior
seed and all its required-input counters before invocation. The plugin receives
the typed unavailable frame, returns its reset state and unavailable outputs,
and cannot emit ready outputs for that frame. A deterministic reset identity is
persisted with the checkpoint. Subsequent readiness is checked separately for
each output against its resolved input counts. Other instruments retain their
state, counters, availability and lineage. State, counters, reset identities,
outputs, source cursors and receipts use the existing fenced atomic commit;
a failed validation, invocation or commit advances none of them.

A missing source event is different from a complete unavailable frame. Missing
coverage waits or rejects with an explicit gap/replay requirement. There is no
forward fill, calendar inference, provider substitution or error-to-false rule.
Existing authority, package freshness, schema and state-partition checks still
apply. This source contract is not evidence of provider coverage, plugin-pack
adoption, released installation or broker execution.
