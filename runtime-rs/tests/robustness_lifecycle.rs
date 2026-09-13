use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tradeassembly_runtime::domain::RobustnessRunState;
use tradeassembly_runtime::ports::{
    AuthorityContext, ClockPort, IdempotencyKey, PortDescriptor, PortKind, SideEffectContext,
    VersionedPort,
};
use tradeassembly_runtime::robustness_contracts::*;
use tradeassembly_runtime::service::{robustness_lifecycle, TradeAssemblyService};

struct TestClock(AtomicI64);
impl TestClock {
    fn new(now: i64) -> Self {
        Self(AtomicI64::new(now))
    }
    fn advance(&self, delta: i64) {
        self.0.fetch_add(delta, Ordering::SeqCst);
    }
}
impl VersionedPort for TestClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![PortDescriptor::new(
            PortKind::Clock,
            "test.robustness-clock",
        )]
    }
}
impl ClockPort for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("key"),
    )
}
fn source() -> RobustnessSourceBinding {
    RobustnessSourceBinding {
        source_run_id: "backtest-1".to_string(),
        source_manifest_hash: "sha256:manifest".to_string(),
        source_result_hash: "sha256:result".to_string(),
        source_report_hash: "sha256:report".to_string(),
        source_dataset_id: "dataset-1".to_string(),
        source_dataset_hash: "sha256:dataset".to_string(),
    }
}
fn manifest(run_id: &str, now: i64) -> RobustnessRunManifest {
    RobustnessRunManifest::build(
        RobustnessRunManifestContent {
            schema: ROBUSTNESS_MANIFEST_SCHEMA.to_string(),
            run_id: run_id.to_string(),
            request_hash: "sha256:request".to_string(),
            idempotency_key: format!("key-{run_id}"),
            study_kind: RobustnessStudyKind::MonteCarlo,
            engine_version: "robustness-engine-v1".to_string(),
            deterministic_seed: 7,
            source: source(),
            supporting_sources: Vec::new(),
            assumptions: json!({"method":"trade_bootstrap"}),
            budget: RobustnessBudget {
                maximum_samples: 100,
                maximum_grid_points: 10,
                maximum_windows: 10,
                maximum_scenarios: 10,
                maximum_output_bytes: 1024,
                maximum_attempts: 2,
            },
            authority: AuthorityContext::local_cli(),
            client: "test".to_string(),
            purpose: "research".to_string(),
            first_attempt_id: format!("{run_id}:attempt:1"),
        },
        now,
    )
    .expect("manifest")
}
fn attempt(run_id: &str, now: i64) -> RobustnessAttempt {
    RobustnessAttempt {
        schema: ROBUSTNESS_ATTEMPT_SCHEMA.to_string(),
        attempt_id: format!("{run_id}:attempt:1"),
        run_id: run_id.to_string(),
        ordinal: 1,
        state: RobustnessAttemptState::Queued,
        lease_owner: None,
        lease_until_ms: None,
        fencing_token: 0,
        heartbeat_at_ms: None,
        created_at_ms: now,
        updated_at_ms: now,
        failure_code: None,
    }
}
fn queued(manifest: &RobustnessRunManifest, now: i64) -> RobustnessRunRecord {
    RobustnessRunRecord {
        schema: ROBUSTNESS_RUN_SCHEMA.to_string(),
        run_id: manifest.content.run_id.clone(),
        manifest_hash: manifest.manifest_hash.clone(),
        request_hash: manifest.content.request_hash.clone(),
        idempotency_key: manifest.content.idempotency_key.clone(),
        state: RobustnessRunState::Queued,
        sequence: 0,
        current_attempt_id: manifest.content.first_attempt_id.clone(),
        fencing_token: 0,
        result_hash: None,
        result: None,
        failure_code: None,
        events: vec![],
        created_at_ms: now,
        updated_at_ms: now,
    }
}
fn runtime(
    now: i64,
) -> (
    NamedTempFile,
    tradeassembly_runtime::ports::ServiceRuntime,
    Arc<TestClock>,
) {
    let file = NamedTempFile::new().expect("db");
    let base = TradeAssemblyService::test_local(file.path().to_string_lossy());
    let mut runtime = (*base.runtime()).clone();
    let clock = Arc::new(TestClock::new(now));
    runtime.clock = clock.clone();
    (file, runtime, clock)
}

#[test]
fn immutable_contracts_reject_tamper_and_idempotency_conflict() {
    let (_file, runtime, _) = runtime(100);
    let manifest = manifest("r1", 100);
    runtime
        .robustness
        .put_manifest(&manifest, &context("one"))
        .expect("put");
    let mut tampered = manifest.clone();
    tampered.content.deterministic_seed += 1;
    assert_eq!(
        tampered.verify(),
        Err("robustness_manifest_integrity_failed".to_string())
    );
    runtime
        .robustness
        .create_run(&queued(&manifest, 100), &context("one"))
        .expect("run");
    assert!(runtime
        .robustness
        .find_run_by_idempotency_key("key-r1")
        .expect("find")
        .is_some());
    assert_eq!(
        runtime.robustness.put_manifest(&tampered, &context("one")),
        Err("robustness_manifest_integrity_failed".to_string())
    );
}

#[test]
fn claimed_attempt_heartbeats_and_commits_once_with_fencing() {
    let (_file, runtime, clock) = runtime(100);
    let manifest = manifest("r2", 100);
    let run = queued(&manifest, 100);
    runtime
        .robustness
        .put_manifest(&manifest, &context("two"))
        .expect("manifest");
    runtime
        .robustness
        .create_run(&run, &context("two"))
        .expect("run");
    runtime
        .robustness
        .put_attempt(&attempt("r2", 100), &context("two"))
        .expect("attempt");
    let (running, claimed) =
        robustness_lifecycle::claim(&runtime, "r2", "worker-a", &context("claim")).expect("claim");
    assert_eq!(running.state, RobustnessRunState::Running);
    assert_eq!(claimed.fencing_token, 1);
    clock.advance(1);
    let claimed = robustness_lifecycle::heartbeat(
        &runtime,
        "r2",
        &claimed.attempt_id,
        "worker-a",
        1,
        &context("heartbeat"),
    )
    .expect("heartbeat");
    let result = RobustnessResult::build(
        RobustnessResultContent {
            schema: ROBUSTNESS_RESULT_SCHEMA.to_string(),
            run_id: "r2".to_string(),
            manifest_hash: manifest.manifest_hash.clone(),
            source: source(),
            engine_version: "robustness-engine-v1".to_string(),
            output: json!({"available":false,"reason":"engine_not_in_f01"}),
            diagnostics: vec![],
        },
        101,
    )
    .expect("result");
    let completed = robustness_lifecycle::complete(
        &runtime,
        "r2",
        &claimed.attempt_id,
        "worker-a",
        1,
        result.clone(),
        &context("complete"),
    )
    .expect("complete");
    assert_eq!(completed.state, RobustnessRunState::Completed);
    assert_eq!(
        completed.result_hash.as_deref(),
        Some(result.result_hash.as_str())
    );
    assert_eq!(
        robustness_lifecycle::complete(
            &runtime,
            "r2",
            &claimed.attempt_id,
            "worker-a",
            1,
            result,
            &context("late")
        ),
        Err("robustness_worker_lease_lost".to_string())
    );
}

#[test]
fn cancellation_blocks_completion_and_recovery_fails_expired_lease() {
    let (_file, runtime, clock) = runtime(100);
    let first_manifest = manifest("r3", 100);
    let run = queued(&first_manifest, 100);
    runtime
        .robustness
        .put_manifest(&first_manifest, &context("three"))
        .expect("manifest");
    runtime
        .robustness
        .create_run(&run, &context("three"))
        .expect("run");
    runtime
        .robustness
        .put_attempt(&attempt("r3", 100), &context("three"))
        .expect("attempt");
    let (_, claimed) =
        robustness_lifecycle::claim(&runtime, "r3", "worker-a", &context("claim-three"))
            .expect("claim");
    robustness_lifecycle::cancel(&runtime, "r3", &context("cancel-three")).expect("cancel");
    assert_eq!(
        robustness_lifecycle::heartbeat(
            &runtime,
            "r3",
            &claimed.attempt_id,
            "worker-a",
            1,
            &context("late-heartbeat")
        ),
        Err("robustness_worker_lease_lost".to_string())
    );
    let manifest = manifest("r4", 100);
    let mut run = queued(&manifest, 100);
    run.state = RobustnessRunState::Running;
    run.fencing_token = 2;
    let mut expired = attempt("r4", 100);
    expired.state = RobustnessAttemptState::Running;
    expired.lease_owner = Some("worker-b".to_string());
    expired.fencing_token = 2;
    expired.lease_until_ms = Some(101);
    runtime
        .robustness
        .put_manifest(&manifest, &context("four"))
        .expect("manifest");
    runtime
        .robustness
        .create_run(&run, &context("four"))
        .expect("run");
    runtime
        .robustness
        .put_attempt(&expired, &context("four"))
        .expect("attempt");
    clock.advance(2);
    let recovered = robustness_lifecycle::recover(&runtime, &context("recover")).expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, RobustnessRunState::Queued);
    assert_eq!(recovered[0].events.len(), 2);
    assert_eq!(
        runtime
            .robustness
            .list_attempts("r4")
            .expect("attempts")
            .len(),
        2
    );
}

#[test]
fn queued_recovery_is_idempotent_and_worker_completes_from_durable_queue() {
    let (_file, runtime, _) = runtime(100);
    let manifest = manifest("r5", 100);
    let run = queued(&manifest, 100);
    runtime
        .robustness
        .put_manifest(&manifest, &context("five"))
        .expect("manifest");
    runtime
        .robustness
        .create_run(&run, &context("five"))
        .expect("run");
    runtime
        .robustness
        .put_attempt(&attempt("r5", 100), &context("five"))
        .expect("attempt");

    assert_eq!(
        robustness_lifecycle::recover(&runtime, &context("recover-five"))
            .expect("first recovery")
            .len(),
        1
    );
    assert_eq!(
        robustness_lifecycle::recover(&runtime, &context("recover-five-again"))
            .expect("second recovery")
            .len(),
        1
    );

    let completed = robustness_lifecycle::process_with(&runtime, "worker-a", &|manifest, _| {
        RobustnessResult::build(
            RobustnessResultContent {
                schema: ROBUSTNESS_RESULT_SCHEMA.to_string(),
                run_id: manifest.content.run_id.clone(),
                manifest_hash: manifest.manifest_hash.clone(),
                source: manifest.content.source.clone(),
                engine_version: manifest.content.engine_version.clone(),
                output: json!({"status":"available","study":"monte_carlo"}),
                diagnostics: vec![],
            },
            100,
        )
    })
    .expect("process")
    .expect("delivery");
    assert_eq!(completed.state, RobustnessRunState::Completed);
    assert!(completed.result_hash.is_some());
    assert_eq!(
        robustness_lifecycle::process_with(&runtime, "worker-a", &|_, _| {
            unreachable!("deduplicated queue must be empty")
        })
        .expect("idle"),
        None
    );
}

#[test]
fn worker_failure_is_terminal_and_queue_delivery_is_acknowledged() {
    let (_file, runtime, _) = runtime(100);
    let manifest = manifest("r6", 100);
    let run = queued(&manifest, 100);
    runtime
        .robustness
        .put_manifest(&manifest, &context("six"))
        .expect("manifest");
    runtime
        .robustness
        .create_run(&run, &context("six"))
        .expect("run");
    runtime
        .robustness
        .put_attempt(&attempt("r6", 100), &context("six"))
        .expect("attempt");
    robustness_lifecycle::recover(&runtime, &context("recover-six")).expect("recover");

    let failed = robustness_lifecycle::process_with(&runtime, "worker-b", &|_, _| {
        Err("robustness_fixture_failure".to_string())
    })
    .expect("process")
    .expect("delivery");
    assert_eq!(failed.state, RobustnessRunState::Failed);
    assert_eq!(
        failed.failure_code.as_deref(),
        Some("robustness_fixture_failure")
    );
    assert_eq!(
        robustness_lifecycle::process_with(&runtime, "worker-b", &|_, _| {
            unreachable!("acknowledged failure must not redeliver")
        })
        .expect("idle"),
        None
    );
}
