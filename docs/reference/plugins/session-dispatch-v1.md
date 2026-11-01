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
positions/orders. Whole compiled Core process restart is a separate opt-in gate.
Remote Reach dispatch negotiation, selection-to-order admission, continuing exit
management and point-in-time research accounting remain separate integrations.
This is not production authentication, market-data or brokerage acceptance.


## Compiled process gate

The ignored `session_actual_compiled_core_process_restart_and_lost_mcp_response`
gate uses an explicitly frozen Core CLI and the Rust Cargo example
`session-process-driver` (`tests/fixtures/session_process_driver.rs`). The old
Python driver is removed from the source lane; historical Python evidence is
not a runtime dependency. Build/freeze the example with the owning Cargo lane,
then run its helper tests separately from the actual process qualification:

```sh
cargo build --offline -p tradeassembly-runtime --example session-process-driver
cargo test --offline -p tradeassembly-runtime --example session-process-driver
```

Inputs are explicit absolute paths `SESSION_PROCESS_TEST_BINARY`,
`SESSION_PROCESS_TEST_DRIVER`, `SESSION_PROCESS_TEST_SRT`, their corresponding
`_SHA256` values, `SESSION_PROCESS_TEST_CORE_REVISION` (the frozen CLI's exact
40-character source revision), and a new `SCREEN_INTERFACE_EVIDENCE_DIR`.
The wrapper verifies all frozen bytes and launches the driver binary directly.
The driver independently verifies Core/SRT bytes and actual stored coreRevision;
there is no implicit historical revision or `SCRIPT`/Python fallback.

After normal MCP initialization it drops the owned response pipe before sending
enqueue, verifies the actual domain and C3 commit, and replays the exact enqueue
in a fresh Core process. It enables the same owned worker intent and explicitly
kills that Core child before due. Normal `api` startup must resume that unchanged
intent, produce native warming/ready frames through the configured SRT, and keep
the completed job unchanged across another API restart without provider replay.
The exact seeded unrelated pending order and existing position must remain
unchanged throughout; absent, extra or modified seed records refuse before
starting Core. Financial activation is explicitly excluded from this computation
fixture.

Execution is explicitly Unix/SRT-only; other platforms refuse the driver rather
than claim an unqualified sandbox boundary. MCP input/output use owned nonblocking
pipes and finite deadlines, with no reader threads retaining a supposedly closed
reply pipe. Database reads use the existing read-only SQLite adapter dependency,
closed namespace names and row/byte limits. Child kill/join and driver lifetime
are bounded; failure leaves isolated synthetic state and logs for reconciliation.
A protected synthetic marker and private files are required, Warden is loopback,
and Core uses normal local_owner startup with synthetic fixture authority only.

Helper tests and compilation do not qualify this new process driver. The actual
ignored gate must run against the declared frozen bytes and retain its result.
This is not an installer or release qualification, does not establish real-provider
entitlement/credentials, and does not establish Relay HTTP record-ack loss handling.
