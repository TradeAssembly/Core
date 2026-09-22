use chrono::{TimeZone, Utc};
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};
use tradeassembly_runtime::agent_runner::{
    self, AgentDeployment, AgentRun, AgentRuntimePort, ACTIVE_RUNS_NS,
};
use tradeassembly_runtime::cli::{execute_command, AgentCommand, Command};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

struct FakeAdapter {
    calls: AtomicUsize,
    resumes: AtomicUsize,
}
impl AgentRuntimePort for FakeAdapter {
    fn start_or_resume(
        &self,
        run: &AgentRun,
        _trigger: &serde_json::Value,
    ) -> Result<Option<String>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if run.codex_session_ref.is_some() {
            self.resumes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Some(format!("codex-session:{}", run.run_id)))
    }
}
struct FailingAdapter;
impl AgentRuntimePort for FailingAdapter {
    fn start_or_resume(
        &self,
        _run: &AgentRun,
        _trigger: &serde_json::Value,
    ) -> Result<Option<String>, String> {
        Err("fake_adapter_failed".to_string())
    }
}
struct SelectiveAdapter {
    attempts: AtomicUsize,
}
impl AgentRuntimePort for SelectiveAdapter {
    fn start_or_resume(
        &self,
        run: &AgentRun,
        _trigger: &serde_json::Value,
    ) -> Result<Option<String>, String> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if run.deployment_id == "deploy-a" {
            Err("fake_adapter_failed".to_string())
        } else {
            Ok(Some(format!("codex-session:{}", run.run_id)))
        }
    }
}
struct PolicyCapturingAdapter {
    policy: Mutex<Option<String>>,
}

struct LongRunningAdapter {
    started: AtomicBool,
    calls: AtomicUsize,
    completion: Arc<(Mutex<bool>, Condvar)>,
}

impl AgentRuntimePort for LongRunningAdapter {
    fn start_or_resume(
        &self,
        run: &AgentRun,
        _trigger: &serde_json::Value,
    ) -> Result<Option<String>, String> {
        self.started.store(true, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (completed, wake) = &*self.completion;
        let completed = completed.lock().unwrap();
        let (completed, _) = wake
            .wait_timeout_while(completed, Duration::from_secs(5), |done| !*done)
            .unwrap();
        if !*completed {
            return Err("test_adapter_completion_timed_out".into());
        }
        Ok(Some(format!("codex-session:{}", run.run_id)))
    }
}
impl AgentRuntimePort for PolicyCapturingAdapter {
    fn start_or_resume(
        &self,
        _run: &AgentRun,
        trigger: &serde_json::Value,
    ) -> Result<Option<String>, String> {
        *self.policy.lock().unwrap() = trigger["sideEffectPolicy"].as_str().map(str::to_string);
        Ok(Some("codex-session:policy".to_string()))
    }
}
struct DeniedEntitlement {
    calls: AtomicUsize,
}
impl agent_runner::AgentEntitlementPort for DeniedEntitlement {
    fn authorize(&self, _deployment: &AgentDeployment) -> agent_runner::AgentEntitlementDecision {
        self.calls.fetch_add(1, Ordering::SeqCst);
        agent_runner::AgentEntitlementDecision {
            allowed: false,
            reference: "denied-test".into(),
        }
    }
}

fn db(name: &str) -> String {
    format!(
        ".tradeassembly/test-agent-runner-{name}-{}.db",
        std::process::id()
    )
}
fn deployment(id: &str) -> AgentDeployment {
    AgentDeployment {
        executor: Default::default(),
        deployment_id: id.to_string(),
        system_project_id: "system-1".to_string(),
        agent_definition_version_id: "agent-v1".to_string(),
        execution_config_version_id: "config-v1".to_string(),
        studio_tool_allowlist: vec![
            "studio.deployment.inspect".to_string(),
            "studio.execution.external_receipt.append".to_string(),
        ],
        desired_state: "active".to_string(),
        interval_seconds: 60,
        cron_utc: None,
        mode: "live".to_string(),
        prompt: "inspect the supplied redacted trigger and use Studio MCP tools".to_string(),
        workspace: std::env::temp_dir().display().to_string(),
        runtime_profile: "local-read-only".to_string(),
    }
}

fn cron_deployment(id: &str, expression: &str) -> AgentDeployment {
    let mut configured = deployment(id);
    configured.cron_utc = Some(expression.to_string());
    configured
}

fn utc_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .expect("fixed UTC test timestamp")
        .timestamp_millis()
}

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).unwrap(),
    )
}

#[test]
fn external_client_deployment_never_starts_a_supervised_agent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external.db");
    let service = TradeAssemblyService::test_local(path.to_str().unwrap());
    let mut external = deployment("external-client");
    let legacy_digest = agent_runner::deployment_binding_digest(&external);
    let mut legacy = serde_json::to_value(&external).unwrap();
    legacy.as_object_mut().unwrap().remove("executor");
    let restored: AgentDeployment = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.executor, agent_runner::AgentExecutor::Supervised);
    assert_eq!(
        agent_runner::deployment_binding_digest(&restored),
        legacy_digest
    );
    external.executor = agent_runner::AgentExecutor::ExternalClient;
    external.prompt.clear();
    external.workspace.clear();
    external.interval_seconds = 0;
    assert_ne!(
        agent_runner::deployment_binding_digest(&external),
        legacy_digest
    );
    agent_runner::put_deployment(&service.runtime(), &external).unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    for now in [120_000, 180_000] {
        assert!(
            agent_runner::supervise_once(&service.runtime(), "supervisor", now, &adapter)
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert!(service
        .runtime()
        .storage
        .list_json(agent_runner::SCHEDULES_NS)
        .unwrap()
        .is_empty());
    assert!(service
        .runtime()
        .storage
        .list_json(agent_runner::RUNS_NS)
        .unwrap()
        .is_empty());
    let restarted = TradeAssemblyService::test_local(path.to_str().unwrap());
    assert!(
        agent_runner::supervise_once(&restarted.runtime(), "restart", 240_000, &adapter)
            .unwrap()
            .is_empty()
    );
    external.executor = agent_runner::AgentExecutor::Supervised;
    external.prompt = "Read owner-provided evidence.".into();
    external.workspace = temp.path().display().to_string();
    external.interval_seconds = 60;
    assert_eq!(
        agent_runner::put_deployment(&restarted.runtime(), &external).unwrap_err(),
        "agent_deployment_binding_immutable"
    );
}

#[test]
fn all_active_deployments_run_without_singleton_scheduler_state() {
    let db = db("all-active");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-b")).unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    let receipts =
        agent_runner::supervise_once(&service.runtime(), "runner-a", 120_000, &adapter).unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    assert!(service
        .runtime()
        .storage
        .get_json("scheduler_state", "current")
        .unwrap()
        .is_none());
    let _ = std::fs::remove_file(db);
}

#[test]
fn quarantined_deployment_does_not_stop_other_active_deployments() {
    let db = db("quarantine-isolation");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-b")).unwrap();
    let adapter = SelectiveAdapter {
        attempts: AtomicUsize::new(0),
    };

    let first =
        agent_runner::supervise_once(&service.runtime(), "runner", 60_000, &adapter).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(
        first
            .iter()
            .find(|receipt| receipt["deploymentId"] == "deploy-a")
            .unwrap()["outcome"],
        "pending_reconcile"
    );
    assert_eq!(
        first
            .iter()
            .find(|receipt| receipt["deploymentId"] == "deploy-b")
            .unwrap()["outcome"],
        "completed"
    );
    assert_eq!(adapter.attempts.load(Ordering::SeqCst), 2);

    let second =
        agent_runner::supervise_once(&service.runtime(), "runner", 61_000, &adapter).unwrap();
    assert_eq!(
        second
            .iter()
            .find(|receipt| receipt["deploymentId"] == "deploy-a")
            .unwrap()["outcome"],
        "pending_reconcile"
    );
    assert_eq!(
        second
            .iter()
            .find(|receipt| receipt["deploymentId"] == "deploy-b")
            .unwrap()["outcome"],
        "not_due"
    );
    assert_eq!(adapter.attempts.load(Ordering::SeqCst), 2);
    let _ = std::fs::remove_file(db);
}

#[test]
fn overlapping_tick_is_coalesced_and_restart_recovers_once() {
    let db = db("coalesce-recover");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let configured = deployment("deploy-a");
    let binding_digest = agent_runner::deployment_binding_digest(&configured);
    agent_runner::put_deployment(&service.runtime(), &configured).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            ACTIVE_RUNS_NS,
            "deploy-a",
            json!({"runId":"run-inflight","state":"running","leaseExpiresAtMs": 200_000}),
            &context("inflight"),
        )
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            agent_runner::RUNS_NS,
            "run-inflight",
            json!({"run":{"runId":"run-inflight","state":"running","codexSessionRef":"opaque"},"bindingDigest":binding_digest}),
            &context("inflight-run"),
        )
        .unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    let coalesced =
        agent_runner::supervise_once(&service.runtime(), "runner-a", 120_000, &adapter).unwrap();
    assert_eq!(coalesced[0]["outcome"], "coalesced");
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    drop(service);
    let restarted = TradeAssemblyService::test_local(&db);
    let recovered =
        agent_runner::supervise_once(&restarted.runtime(), "runner-b", 200_001, &adapter).unwrap();
    assert_eq!(recovered[0]["outcome"], "completed");
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    let events = restarted
        .runtime()
        .events
        .replay("agent-deployment:deploy-a", 0, 10)
        .unwrap();
    assert!(events
        .iter()
        .any(|event| event.event_type == "agent_run.recovered"));
    let _ = std::fs::remove_file(db);
}

#[test]
fn long_adapter_run_renews_the_lease_before_another_runner_can_acquire_it() {
    let db = db("lease-heartbeat");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    let adapter = Arc::new(LongRunningAdapter {
        started: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        completion: Arc::new((Mutex::new(false), Condvar::new())),
    });
    let completion = Arc::clone(&adapter.completion);
    let primary_service = service.clone();
    let primary_adapter = Arc::clone(&adapter);
    let primary = std::thread::spawn(move || {
        agent_runner::supervise_once_with_timing_and_entitlement(
            &primary_service.runtime(),
            "runner-a",
            1_000,
            primary_adapter.as_ref(),
            &agent_runner::LocalEntitlement,
            agent_runner::AgentRunnerTiming {
                lease_ms: 5_000,
                heartbeat_ms: 10,
            },
        )
    });
    for _ in 0..100 {
        if adapter.started.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(adapter.started.load(Ordering::SeqCst));
    let deadline = Instant::now() + Duration::from_secs(2);
    let active = loop {
        let active = service
            .runtime()
            .storage
            .get_json(ACTIVE_RUNS_NS, "deploy-a")
            .unwrap()
            .unwrap();
        let original_expiry = active["leaseExpiresAtMs"].as_i64().unwrap();
        if original_expiry > 1_000 {
            break (active, original_expiry);
        }
        assert!(Instant::now() < deadline, "active lease was not persisted");
        std::thread::sleep(Duration::from_millis(5));
    };
    let (active, original_expiry) = active;
    let renewed_expiry = loop {
        let current = service
            .runtime()
            .storage
            .get_json(ACTIVE_RUNS_NS, "deploy-a")
            .unwrap()
            .unwrap();
        let expiry = current["leaseExpiresAtMs"].as_i64().unwrap();
        if expiry > original_expiry + 1 {
            break expiry;
        }
        assert!(Instant::now() < deadline, "active lease was not renewed");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(renewed_expiry > active["leaseExpiresAtMs"].as_i64().unwrap());
    let competing = agent_runner::supervise_once_with_timing_and_entitlement(
        &service.runtime(),
        "runner-b",
        original_expiry + 1,
        adapter.as_ref(),
        &agent_runner::LocalEntitlement,
        agent_runner::AgentRunnerTiming {
            lease_ms: 5_000,
            heartbeat_ms: 10,
        },
    )
    .unwrap();
    assert_eq!(competing[0]["outcome"], "lease_held");
    let (done, wake) = &*completion;
    *done.lock().unwrap() = true;
    wake.notify_all();
    let completed = primary.join().unwrap().unwrap();
    assert_eq!(completed[0]["outcome"], "completed");
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    let active = service
        .runtime()
        .storage
        .get_json(ACTIVE_RUNS_NS, "deploy-a")
        .unwrap()
        .unwrap();
    assert_eq!(active["leaseFence"], 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn interval_skip_then_resumes_the_same_opaque_session() {
    let db = db("interval-resume");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 60_000, &adapter).unwrap()[0]
            ["outcome"],
        "completed"
    );
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 61_000, &adapter).unwrap()[0]
            ["outcome"],
        "not_due"
    );
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 120_000, &adapter).unwrap()[0]
            ["outcome"],
        "completed"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(adapter.resumes.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn cron_schedule_is_utc_minute_resolution_and_honors_standard_dom_dow_or() {
    let db = db("cron-utc-boundary");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    // 2026-09-01 is a Tuesday (2 when Sunday is zero), while the day-of-month
    // selector is two. Standard five-field cron treats two restricted day
    // selectors as OR, so this runs because the weekday matches.
    agent_runner::put_deployment(
        &service.runtime(),
        &cron_deployment("deploy-a", "0 14 2 * 2"),
    )
    .unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    assert_eq!(
        agent_runner::supervise_once(
            &service.runtime(),
            "runner",
            utc_ms(2026, 9, 1, 13, 59, 30),
            &adapter,
        )
        .unwrap()[0]["outcome"],
        "not_due"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        agent_runner::supervise_once(
            &service.runtime(),
            "runner",
            utc_ms(2026, 9, 1, 14, 0, 0),
            &adapter,
        )
        .unwrap()[0]["outcome"],
        "completed"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    let schedule = service
        .runtime()
        .storage
        .get_json(agent_runner::SCHEDULES_NS, "deploy-a")
        .unwrap()
        .unwrap();
    assert_eq!(schedule["scheduleKind"], "cron_utc");
    assert_eq!(schedule["lastDueAtMs"], utc_ms(2026, 9, 1, 14, 0, 0));
    assert_eq!(schedule["nextDueAtMs"], utc_ms(2026, 9, 2, 14, 0, 0));
    let _ = std::fs::remove_file(db);
}

#[test]
fn cron_validation_rejects_seconds_timezone_and_out_of_range_fields() {
    let db = db("cron-validation");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    for (index, expression) in [
        "0 */5 * * * *",
        "0 14 * * * UTC",
        "*/0 * * * *",
        "60 * * * *",
        "0 24 * * *",
        "0 0 0 * *",
        "0 0 * 13 *",
        "0 0 * * 8",
        "0 14 ? * 1-5",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            agent_runner::put_deployment(
                &service.runtime(),
                &cron_deployment(&format!("invalid-{index}"), expression),
            )
            .unwrap_err(),
            "agent_deployment_invalid"
        );
    }
    let mut invalid_interval = deployment("invalid-interval");
    invalid_interval.interval_seconds = 59;
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &invalid_interval).unwrap_err(),
        "agent_deployment_invalid"
    );
    let mut cron_only = cron_deployment("cron-only", "*/15 * * * *");
    cron_only.interval_seconds = 0;
    agent_runner::put_deployment(&service.runtime(), &cron_only).unwrap();
    let _ = std::fs::remove_file(db);
}

#[test]
fn cron_schedule_change_recomputes_the_durable_next_due() {
    let db = db("cron-change");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    agent_runner::put_deployment(
        &service.runtime(),
        &cron_deployment("deploy-a", "*/10 * * * *"),
    )
    .unwrap();
    let at_one = utc_ms(2026, 9, 1, 10, 1, 0);
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", at_one, &adapter).unwrap()[0]
            ["outcome"],
        "not_due"
    );
    let first_schedule = service
        .runtime()
        .storage
        .get_json(agent_runner::SCHEDULES_NS, "deploy-a")
        .unwrap()
        .unwrap();
    assert_eq!(first_schedule["nextDueAtMs"], utc_ms(2026, 9, 1, 10, 10, 0));

    agent_runner::put_deployment(
        &service.runtime(),
        &cron_deployment("deploy-a", "*/5 * * * *"),
    )
    .unwrap();
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", at_one, &adapter).unwrap()[0]
            ["outcome"],
        "not_due"
    );
    let recomputed = service
        .runtime()
        .storage
        .get_json(agent_runner::SCHEDULES_NS, "deploy-a")
        .unwrap()
        .unwrap();
    assert_eq!(recomputed["nextDueAtMs"], utc_ms(2026, 9, 1, 10, 5, 0));
    assert_ne!(
        recomputed["scheduleDigest"],
        first_schedule["scheduleDigest"]
    );
    assert_eq!(
        agent_runner::supervise_once(
            &service.runtime(),
            "runner",
            utc_ms(2026, 9, 1, 10, 5, 0),
            &adapter,
        )
        .unwrap()[0]["outcome"],
        "completed"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn missed_cron_ticks_coalesce_once_and_next_due_survives_restart() {
    let db = db("cron-missed");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(
        &service.runtime(),
        &cron_deployment("deploy-a", "*/5 * * * *"),
    )
    .unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    let first_due = utc_ms(2026, 9, 1, 10, 0, 0);
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner-a", first_due, &adapter).unwrap()
            [0]["outcome"],
        "completed"
    );
    drop(service);

    let restarted = TradeAssemblyService::test_local(&db);
    let delayed = utc_ms(2026, 9, 1, 10, 16, 0);
    assert_eq!(
        agent_runner::supervise_once(&restarted.runtime(), "runner-b", delayed, &adapter).unwrap()
            [0]["outcome"],
        "completed"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    let schedule = restarted
        .runtime()
        .storage
        .get_json(agent_runner::SCHEDULES_NS, "deploy-a")
        .unwrap()
        .unwrap();
    assert_eq!(schedule["lastDueAtMs"], utc_ms(2026, 9, 1, 10, 5, 0));
    assert_eq!(schedule["missedTickCount"], 2);
    assert_eq!(schedule["missedTickCountCapped"], false);
    assert_eq!(schedule["nextDueAtMs"], utc_ms(2026, 9, 1, 10, 20, 0));
    drop(restarted);

    let after_restart = TradeAssemblyService::test_local(&db);
    assert_eq!(
        agent_runner::supervise_once(
            &after_restart.runtime(),
            "runner-c",
            utc_ms(2026, 9, 1, 10, 17, 0),
            &adapter,
        )
        .unwrap()[0]["outcome"],
        "not_due"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    let _ = std::fs::remove_file(db);
}

#[test]
fn overdue_interval_with_a_valid_active_run_is_coalesced_once() {
    let db = db("interval-coalesced");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let configured = deployment("deploy-a");
    agent_runner::put_deployment(&service.runtime(), &configured).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            agent_runner::SCHEDULES_NS,
            "deploy-a",
            json!({"nextDueAtMs": 60_000}),
            &context("legacy-overdue-schedule"),
        )
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            ACTIVE_RUNS_NS,
            "deploy-a",
            json!({"runId":"run-inflight","state":"running","leaseExpiresAtMs":300_000}),
            &context("active-overdue-schedule"),
        )
        .unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 180_000, &adapter).unwrap()[0]
            ["outcome"],
        "coalesced"
    );
    let schedule = service
        .runtime()
        .storage
        .get_json(agent_runner::SCHEDULES_NS, "deploy-a")
        .unwrap()
        .unwrap();
    assert_eq!(schedule["lastDueAtMs"], 60_000);
    assert_eq!(schedule["missedTickCount"], 2);
    assert_eq!(schedule["nextDueAtMs"], 240_000);
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 181_000, &adapter).unwrap()[0]
            ["outcome"],
        "not_due"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    let coalesced_events = service
        .runtime()
        .events
        .replay("agent-deployment:deploy-a", 0, 10)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "agent_run.coalesced")
        .count();
    assert_eq!(coalesced_events, 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn adapter_failure_is_quarantined_until_explicit_operator_recovery() {
    let db = db("failure");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner-a", 60_000, &FailingAdapter)
            .unwrap()[0]["outcome"],
        "pending_reconcile"
    );
    let quarantined_run_id = service
        .runtime()
        .storage
        .get_json(ACTIVE_RUNS_NS, "deploy-a")
        .unwrap()
        .unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(agent_runner::RUNS_NS, &quarantined_run_id)
            .unwrap()
            .unwrap()["run"]["state"],
        "pending_reconcile"
    );
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner-b", 120_000, &adapter).unwrap()[0]
            ["outcome"],
        "pending_reconcile"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        agent_runner::recover_pending_run(&service.runtime(), "deploy-a", 120_000).unwrap()
            ["outcome"],
        "reconciled"
    );
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner-b", 120_000, &adapter).unwrap()[0]
            ["outcome"],
        "completed"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn codex_adapter_builds_workspace_profile_args_and_parses_jsonl_session() {
    let run = AgentRun {
        run_id: "run-1".into(),
        deployment_id: "deploy-a".into(),
        trigger_id: "tick".into(),
        state: "running".into(),
        lease_fence: 1,
        deployment_binding_digest: "binding".into(),
        codex_session_ref: None,
    };
    let trigger = json!({"workspace":"/tmp/workspace", "runtimeProfile":"tradeassembly-local", "sideEffectPolicy":"TradeAssembly MCP only"});
    let args = agent_runner::codex_command_args(&run, &trigger).unwrap();
    assert_eq!(args[..4], ["exec", "--json", "-C", "/tmp/workspace"]);
    assert!(args
        .windows(2)
        .any(|pair| pair == ["-p", "tradeassembly-local"]));
    assert!(args.iter().any(|arg| arg == "--skip-git-repo-check"));
    let resumed = AgentRun {
        codex_session_ref: Some("opaque-session".into()),
        ..run
    };
    let resume_args = agent_runner::codex_command_args(&resumed, &trigger).unwrap();
    assert_eq!(
        resume_args[..4],
        ["exec", "resume", "--skip-git-repo-check", "opaque-session"]
    );
    for invocation in [&args, &resume_args] {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(invocation.last().unwrap()).unwrap(),
            trigger
        );
        assert!(!invocation.iter().any(|arg| arg.contains("bypass")));
    }
    assert_eq!(
        agent_runner::extract_session_ref(
            "{\"type\":\"thread.started\",\"thread_id\":\"opaque-session\"}\n"
        ),
        Some("opaque-session".into())
    );
}

#[test]
fn runner_prompt_keeps_broker_execution_user_configured_and_off_platform() {
    let db = db("broker-off-platform");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    let adapter = PolicyCapturingAdapter {
        policy: Mutex::new(None),
    };
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 60_000, &adapter).unwrap()[0]
            ["outcome"],
        "completed"
    );
    let policy = adapter.policy.lock().unwrap().clone().unwrap();
    assert!(policy.contains("broker-off-platform"));
    assert!(policy.contains("Never put credentials"));
    let _ = std::fs::remove_file(db);
}

#[test]
fn deployment_rejects_secret_shaped_prompt_and_preserves_same_trigger_identity() {
    let db = db("secret");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let mut unsafe_deployment = deployment("unsafe");
    unsafe_deployment.prompt = "authorization: Bearer secret".into();
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &unsafe_deployment).unwrap_err(),
        "agent_deployment_invalid"
    );
    unsafe_deployment.prompt = "apiKey: must-not-persist".into();
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &unsafe_deployment).unwrap_err(),
        "agent_deployment_invalid"
    );
    let mut unscoped_tools = deployment("unscoped-tools");
    unscoped_tools.studio_tool_allowlist.clear();
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &unscoped_tools).unwrap_err(),
        "agent_deployment_invalid"
    );
    let mut unsafe_tool = deployment("unsafe-tool");
    unsafe_tool.studio_tool_allowlist = vec!["broker.submit_order".to_string()];
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &unsafe_tool).unwrap_err(),
        "agent_deployment_invalid"
    );
    let mut missing_workspace = deployment("missing-workspace");
    missing_workspace.workspace = std::env::temp_dir()
        .join(format!(
            "tradeassembly-missing-workspace-{}",
            std::process::id()
        ))
        .display()
        .to_string();
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &missing_workspace).unwrap_err(),
        "agent_deployment_invalid"
    );
    let _ = std::fs::remove_file(db);
}

#[test]
fn denied_entitlement_never_invokes_the_adapter() {
    let db = db("entitlement");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    let entitlement = DeniedEntitlement {
        calls: AtomicUsize::new(0),
    };
    assert_eq!(
        agent_runner::supervise_once_with_entitlement(
            &service.runtime(),
            "runner",
            60_000,
            &adapter,
            &entitlement
        )
        .unwrap()[0]["outcome"],
        "entitlement_denied"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(entitlement.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent_runner::supervise_once_with_entitlement(
            &service.runtime(),
            "runner",
            61_000,
            &adapter,
            &entitlement
        )
        .unwrap()[0]["outcome"],
        "not_due"
    );
    assert_eq!(entitlement.calls.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn expired_run_without_session_is_pending_reconcile_not_a_new_run() {
    let db = db("expired-no-session");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    agent_runner::put_deployment(&service.runtime(), &deployment("deploy-a")).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            agent_runner::RUNS_NS,
            "run-old",
            json!({"run":{"runId":"run-old","state":"running","codexSessionRef":null}}),
            &context("old"),
        )
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            ACTIVE_RUNS_NS,
            "deploy-a",
            json!({"runId":"run-old","state":"running","leaseExpiresAtMs":0}),
            &context("active-old"),
        )
        .unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 180_000, &adapter).unwrap()[0]
            ["outcome"],
        "pending_reconcile"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(agent_runner::RUNS_NS, "run-old")
            .unwrap()
            .unwrap()["run"]["state"],
        "pending_reconcile"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(ACTIVE_RUNS_NS, "deploy-a")
            .unwrap()
            .unwrap()["state"],
        "pending_reconcile"
    );
    assert_eq!(
        agent_runner::supervise_once(&service.runtime(), "runner", 181_000, &adapter).unwrap()[0]
            ["outcome"],
        "pending_reconcile"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    let _ = std::fs::remove_file(db);
}

#[test]
fn expired_run_with_session_resumes_the_exact_run_id() {
    let db = db("expired-session");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let configured = deployment("deploy-a");
    let binding_digest = agent_runner::deployment_binding_digest(&configured);
    agent_runner::put_deployment(&service.runtime(), &configured).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            agent_runner::RUNS_NS,
            "run-old",
            json!({"run":{"runId":"run-old","state":"running","codexSessionRef":"opaque"},"bindingDigest":binding_digest}),
            &context("old-session"),
        )
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            ACTIVE_RUNS_NS,
            "deploy-a",
            json!({"runId":"run-old","state":"running","leaseExpiresAtMs":0}),
            &context("active-session"),
        )
        .unwrap();
    let adapter = FakeAdapter {
        calls: AtomicUsize::new(0),
        resumes: AtomicUsize::new(0),
    };
    let result =
        agent_runner::supervise_once(&service.runtime(), "runner", 180_000, &adapter).unwrap();
    assert_eq!(result[0]["runId"], "run-old");
    assert_eq!(adapter.resumes.load(Ordering::SeqCst), 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn active_deployment_binding_rewrite_is_rejected_and_preserves_the_original_binding() {
    let db = db("binding-mismatch");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let configured = deployment("deploy-a");
    let old_binding = agent_runner::deployment_binding_digest(&configured);
    agent_runner::put_deployment(&service.runtime(), &configured).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            agent_runner::RUNS_NS,
            "run-old",
            json!({
                "run":{"runId":"run-old","state":"running","codexSessionRef":"opaque"},
                "bindingDigest": old_binding,
            }),
            &context("binding-old-run"),
        )
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            ACTIVE_RUNS_NS,
            "deploy-a",
            json!({"runId":"run-old","state":"running","leaseExpiresAtMs":0}),
            &context("binding-old-active"),
        )
        .unwrap();
    let mut changed = configured;
    changed.execution_config_version_id = "config-v2".to_string();
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &changed).unwrap_err(),
        "agent_deployment_binding_immutable"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(agent_runner::DEPLOYMENTS_NS, "deploy-a")
            .unwrap()
            .unwrap()["bindingDigest"],
        old_binding
    );
    let _ = std::fs::remove_file(db);
}

#[test]
fn agent_cli_creates_lists_and_changes_a_deployment_lifecycle() {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory
        .path()
        .join("tradeassembly.db")
        .display()
        .to_string();
    let deployment_file = directory.path().join("deployment.json");
    let mut configured = deployment("deploy-cli");
    configured.desired_state = "paused".to_string();
    configured.workspace = directory.path().display().to_string();
    std::fs::write(&deployment_file, serde_json::to_vec(&configured).unwrap()).unwrap();
    let service = TradeAssemblyService::test_local(&db_path);

    assert_eq!(
        execute_command(
            &service,
            Command::Agent {
                command: AgentCommand::Create { deployment_file },
            },
        )["ok"],
        true
    );
    assert_eq!(
        execute_command(
            &service,
            Command::Agent {
                command: AgentCommand::List,
            },
        )["deployments"][0]["desiredState"],
        "paused"
    );
    assert_eq!(
        execute_command(
            &service,
            Command::Agent {
                command: AgentCommand::Start {
                    deployment_id: "deploy-cli".into(),
                },
            },
        )["desiredState"],
        "active"
    );
    assert_eq!(
        execute_command(
            &service,
            Command::Agent {
                command: AgentCommand::Pause {
                    deployment_id: "deploy-cli".into(),
                },
            },
        )["desiredState"],
        "paused"
    );
    assert_eq!(
        execute_command(
            &service,
            Command::Agent {
                command: AgentCommand::Recover {
                    deployment_id: "deploy-cli".into(),
                    acknowledge_reconciled: false,
                },
            },
        )["error"]["code"],
        "agent_recovery_ack_required"
    );
}
