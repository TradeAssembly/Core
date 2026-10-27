# Scheduler service projection: focused qualification preparation

Starting producer checkpoint: `825b105`, implementation `ac891f5` on
`codex/core-scheduler-observation-20261026`. No producer pin or publication.

## First unqualified boundary

The existing source-bound harness qualifies the pure timing projection and
SQLite observation query. It does not compile the owning runtime assembly or
exercise `service::execution::scheduler_for_activation`, which binds a selected
execution's next cycle to that durable scheduler port. The consumer's passing
local owner journey does not qualify this newer producer code.

Two test-only cases are now prepared beside the actual service projection:

- `scheduler_projection_retains_due_time_and_fencing_across_reopen`: constructs
  the owning fixture service and SQLite ports, seeds paused deterministic state,
  ignores a foreign cached schedule key, preserves original due time after
  duplicate scheduling and repeated reads, reopens the service, replaces an
  expired lease, rejects stale completion and observes terminal work. Foreign
  work remains unchanged. Fixed logical timestamps drive scheduler claims;
  wall time is used only to name an owned disposable database.
- `scheduler_projection_suppresses_external_terminal_and_unbound_evidence`:
  suppresses external-agent and stopped timing despite a stale running flag,
  keeps cycle overflow unavailable, rejects an unbound activation and verifies
  reads create no deterministic schedule.

The paused fixture prevents background worker resume on reopen. These cases test
service/port projection and durable schedule behavior, not worker liveness or a
supervised process restart. No broker operation or authority change is involved.

## Actual checks

- `just --command rustfmt --edition 2021 --check
  runtime-rs/src/service/execution.rs`: passed (syntax/format only).
- `git diff --check`: passed.
- Reused the existing source-bound harness executable with the
  `local_schedule_observation::tests::` filter and one test thread: **3 passed**,
  covering read-side fencing preservation, missing schema and SQLite reopen /
  terminal timing. No compilation occurred. Its SHA-256 is
  `4f8187c2949907a23f29d699e3f83c5b3b3572ad26b163522856c97ac4a5c342`.
- Unchanged production module SHA-256 values: timing projection
  `2295d0db692154dba45d9af12789cb3daaacc1ed9f73ba9ef1a619eb214e546a`;
  SQLite query
  `8691152a9889f0bf35cea24519a2e680878185ee22d1853f9109271f612f47f9`.

**The two new owning service tests have not been compiled or executed.** The
cached adapter results do not substitute for them. No runtime defect is claimed
fixed and no worker liveness, PostgreSQL, full gate or merge readiness is claimed.

## Admission blocker and exact resume

The existing resource guard reports 158.00 GiB available (17.06%), below its 20%
floor. Admission with a 4 GiB incremental build estimate and 4 GiB concurrent
reserve returns `enough_space: false`: required available bytes 207,511,965,696;
observed available bytes 169,648,173,056; shortfall 37,863,792,640 bytes
(approximately 35.3 GiB). The floor and reserve were not lowered. No cleanup or
large build was attempted. This is a resource admission failure, not an approval
denial or an executed failing service test.

After admission succeeds, acquire/renew the existing worktree lease and run:

```sh
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 \
  just --command cargo test --locked --offline \
  -p tradeassembly-runtime --lib scheduler_projection_ -- --test-threads=1
```

Then qualify the existing `scheduler_recovery` owning integration target against
these producer bytes. Reuse the native fixture and ports; do not repeat consumer
acceptance or adopt a pending identity dependency candidate. Consumer pin changes
and publication still require their existing qualification and approval gates.
