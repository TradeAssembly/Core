# Local supervised worker observations

Local source checkpoint only. This is not producer publication, consumer pin qualification, merge approval or release readiness.

The existing local scheduler loop now persists immutable, per-activation worker observations and a latest-record pointer. Observations use the runtime clock and the process boot identity already used for restart receipts. A worker session has a distinct lease owner. Each check-in is valid for two configured polling intervals plus five seconds; the observed time and expiry are explicit. Recording/reading observations does not grant execution authority, create schedules or acquire/renew leases.

The activation read model binds observations to the selected activation and current process. It reports previous-process evidence as recovering, expired check-ins as stale, and unsupported/malformed evidence as unavailable. A current matching unexpired evaluation lease is required for the evaluating status. No tick lease between ticks is normal. External-agent and terminal executions return their explicit non-deterministic/stopped states without reading worker or lease evidence. IDs and fencing remain diagnostic fields.

The loop's registration now releases on unwind so a failed thread cannot permanently retain an in-process registry entry. A crashed worker's last check-in expires; existing scheduler/lease fencing continues to own recovery. Reads do not restart a worker. Process restart reconstructs the existing execution and supervised loop without creating a parallel scheduler. No broker, credentials, hosted infrastructure or live activation is added.

Focused source checks passed:

- 8 worker-observation tests: fixed-clock expiry/rollback, missing/invalid/foreign/previous-process evidence, lease matching, actual SQLite immutable history/read-only inspection, stale-owner fencing, and reopen recovery.
- 1 worker registration unwind cleanup test.
- 5 existing scheduler recovery tests: checkpoint-before-ack, exact schedule/logical fencing, invalid delivery retry, outbox idempotency, read-only schedule reopen.
- 2 existing activation timing projection tests, including external/terminal/unbound suppression.
- Scoped `cargo xtask scan-public` passes for the changed source/evidence, including repository copyright inventory.
- `cargo fmt --all -- --check` and `cargo clippy --locked --offline -p tradeassembly-runtime --lib -- -D warnings`.

No full workspace/release-gate claim. Binary/process-restart consumer qualification is recorded separately after the source checkpoint. Observations are bounded evidence of recent loop activity, not a guarantee of uninterrupted operation or a new admission authority.
