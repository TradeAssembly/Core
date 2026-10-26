# Read-only deterministic schedule observation checkpoint

Local implementation checkpoint on public Core base a16d4a769258fb7354d9102017533850020f8f2c. Not merge/release qualification; no producer pin or publication.

Branch codex/core-scheduler-observation-20261026 is isolated. Existing onboarding, installer, dependency remediation and consumer worktrees remain untouched. Retain this checkout for owning validation; source is not integrated or pushed.

## Change

SchedulerPort gains an optional read-only observe operation, returning unavailable by default. Local SQLite and self-hosted PostgreSQL implementations select only an existing schedule's state and due_at_ms. They do not create, claim, renew, complete, cancel or advance work. Existing idempotency/fencing/authority behavior is unchanged.

The execution workspace projects nextEvaluationAt from the selected activation's actual next durable tick. It reports timingStatus scheduled, not_scheduled or unavailable. Overdue work retains its original due time. Terminal execution states, external_agent ownership and a disabled scheduler never claim a next deterministic evaluation. Missing/error/unknown evidence remains unavailable. Schedule keys are derived from the selected run's activation and cycle rather than trusting another activation's cached key.

No worker liveness claim was added: a saved running flag, queue lease and market timestamp are not worker heartbeat proof. Worker freshness and real supervised restart qualification remain a following increment using existing observations and LeaseRepository.current; no new scheduler or timer service.

## Validation

- cargo fmt --all --check and git diff --check pass.
- Source-bound small Cargo harness imports the exact new production projection and SQLite query modules without copying their implementations: 8 tests pass. Cases cover pending/in-flight original due time; terminal/invalid records; stopped/external-agent suppression; unavailable/missing/unknown evidence; paused-entry timing; no read-side lease/fencing changes; missing schema; persisted SQLite reopen; completed/canceled work.
- Harness uses rusqlite 0.32.1 with bundled SQLite, matching the owning lock. It occupies 77 MiB. This validates those exact modules, not the complete runtime or service assembly.
- Added an owning scheduler_recovery integration test for observe/reopen/claim/fencing/complete. Attempted cargo test --locked --offline -p tradeassembly-runtime --test scheduler_recovery stops before building: keyring 4.2.0 is missing from the local cache. No repeated large build or alternate dependency resolution.
- Owning workspace tests, PostgreSQL integration, full security gates and real API restart remain pending. No Rust lock/manifests, identity code, provider configuration or activation authority changed.

## Resume

Qualify the selected Core dependency candidate and build slot, run the owning recovery/adapter/service tests and full gates, then qualify immutable producer bytes before Studio consumer pin changes. This checkpoint contains no hosted/private application source and performs no broker activity.
