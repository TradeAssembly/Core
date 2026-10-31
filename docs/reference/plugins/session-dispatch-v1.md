# Session occurrence dispatch

`tradeassembly.execution.scheduler.run` and `POST /scheduler/run` accept the
additive `sessionDispatch` object. Existing activation ticks remain unchanged.
The closed request has `kind:enqueue` with `job`, `kind:advance` or `inspect` with
`jobId`, `kind:pause` with `jobId` and explicit `paused`,
`kind:start_worker`/`stop_worker` with `jobId`, or `kind:resume_workers`. Each bounded public
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

Explicit bounded ticks and explicitly enabled local background workers share this
same durable state machine. Background intent records only the owned job/source
binding and enabled state; it is not a principal or grant. Each phase rechecks
current identity, source and C3. Up to sixteen workers may run; ephemeral stop
status is bounded separately from durable evidence. Inspection exposes actual
running state and last refusal. Stop disables future phases; a current bounded
calculation may finish. Pause/revision contention retains the original child for
reconciliation. A pause/unpause or stop/start race cannot lose a requested wake.

Local startup may resume enabled intent only through the existing protected
installation identity and current IdentityPort/policy, never by creating an
identity or reading a principal from the job. Other verified sessions may call
resume_workers explicitly. Expired identity, policy or source refusals stop and
remain visible; no authentication or provider substitution is attempted.
Persistent agent delegation has no defined contract yet and refuses background
start. No new key, account, permission, OS service or cron registration is made.

Native regressions exercise real authored plugin processes over loopback with
durable local storage: before-due waiting, competing lease refusal, due wake,
progress-write recovery, closed/missed calendars, empty/incomplete coverage,
worker stop/service recreation/unchanged-intent resume, and untouched managed
positions/orders. Whole compiled Core process restart remains a separate gate.
Remote Reach dispatch negotiation, selection-to-order admission, continuing exit
management and point-in-time research accounting remain separate integrations.
This is not production authentication, market-data or brokerage acceptance.
