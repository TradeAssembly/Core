# Session occurrence dispatch

`tradeassembly.execution.scheduler.run` and `POST /scheduler/run` accept the
additive `sessionDispatch` object. Existing activation ticks remain unchanged.
The closed request has `kind:enqueue` with `job`, `kind:advance` or `inspect` with
`jobId`, or `kind:pause` with `jobId` and explicit `paused`. Each bounded public
poll needs its own transport idempotency key; it never changes the domain job ID.

A job explicitly provides occurrenceId, exact owned scope/graph, paper mode,
subjects (`all_selected` with maxMembers, or `explicit` with instruments),
eventStartMs/eventEndMs, intervalMs, maxFramesPerStep, and
sourceCapturesBySubject. No strategy allocation, price, order or exit parameter is
inferred. A verified current human and compiled Core revision are mandatory.

The admitted calendar occurrence fixes session date, versioned timezone/calendar
semantics, due/latest-start and permissible execution boundaries. Closed/missed
occurrences do not acquire data. Each template selector must be an explicit target
of the published trigger. Newly due work uses published collection queries and
pinned capabilities, then screens, binds each selected subject and evaluates
bounded completed-bar windows. Static selectors remain supported. Unsupported
selector producers and mixed-interval chunking refuse explicitly. Historical
universe dispatch refuses until point-in-time acquisition/replay is qualified;
current chains must not masquerade as historical membership.

Jobs use an owned queue partition, lease/fence, revision CAS and integrity hash.
The original child request/cutoff and key are durable before its effect. Restart
recovers original receipts after domain success and progress-storage failure.
The queue message ID is retained, including across availability-time changes.
Retry limits/backoff come from the published trigger. Exhausted interrupted work
retains its pending request as uncertain, requiring reconciliation. It cannot
restart with a replacement source or reset its attempt count.

Results distinguish running, complete, complete_empty, partial, failed, uncertain,
closed and skipped_missed. Complete means this computation finished; individual
results retain readiness. It grants no entry or execution authority. Inspection
is original owner-local evidence, not fresh provider or trading eligibility.
Candidate changes never delete managed positions, pending orders or their exits.

Current worker support is explicit bounded ticks. Automatic wake/startup resume,
remote Reach dispatch negotiation, selection-to-order admission, continuing exit
management and historical research accounting are separate integration gates.
The native regression exercises real authored plugin processes over loopback and
durable local storage, including before-due waiting, original-result restart,
empty/incomplete coverage and untouched managed positions/orders. It is not
production authentication, market-data or brokerage acceptance.
