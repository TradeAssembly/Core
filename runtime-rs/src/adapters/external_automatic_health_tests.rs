// Explicit native account-health expiry/retry proof. No injectable clock and no
// timestamps are edited: actual trusted host time must pass the unchanged TTL.
fn native_health_reads(root: &std::path::Path, store: &str) -> usize {
    fs::read_to_string(root.join(store).join("health.log"))
        .unwrap_or_default()
        .lines()
        .count()
}
fn native_health_source_pin(s: &crate::service::TradeAssemblyService, id: &str) -> Value {
    let r = s.runtime().plugins.get_instance(id).unwrap().unwrap();
    json!({"instanceRef":r["instanceRef"],"pluginRef":r["pluginRef"],"activePackageSha256":r["activePackageSha256"],
 "configuration":r["configuration"],"credentialRef":r["credentialRef"],"credentialRevision":r["credentialRevision"],
 "credentialRevisionRef":r["credentialStatus"]["revisionRef"],"accountRef":r["accountRef"],"accountMode":r["accountMode"],"healthBinding":r["health"]["binding"]})
}
fn native_health_state(s: &crate::service::TradeAssemblyService) -> Option<Value> {
    let rows = s
        .runtime()
        .storage
        .list_json("automatic_account_health_v1")
        .unwrap();
    assert!(rows.len() <= 1);
    rows.first().map(|(_, v)| v.clone())
}
fn native_health_facts_requests(s: &crate::service::TradeAssemblyService) -> usize {
    s.runtime()
        .storage
        .list_json("plugin_operation_requests")
        .unwrap()
        .iter()
        .filter(|(_, r)| {
            r["binding"]["pluginInstanceRef"] == "fixture.execution.facts"
                && r["binding"]["operationId"] == "account.health"
        })
        .count()
}
fn native_health_financial_state(s: &crate::service::TradeAssemblyService) -> Value {
    let namespaces = [
        "execution_order_plans_v1",
        "execution_order_plan_submissions_v2",
        "broker_order_intents",
        "execution_cash_reservations_v1",
        "execution_managed_positions_v1",
        "execution_managed_position_journal_v1",
        "positions",
        "orders",
        "data_evaluation_checkpoints_v1",
        "data_evaluation_receipts_v1",
    ];
    json!(namespaces
        .iter()
        .map(|ns| (*ns, s.runtime().storage.list_json(ns).unwrap()))
        .collect::<BTreeMap<_, _>>())
}
fn native_health_scheduler(
    s: &crate::service::TradeAssemblyService,
    kind: &str,
    id: &str,
    key: &str,
) -> Value {
    let response=s.handle_http("POST","/scheduler/run",json!({"accountMode":"paper","sessionDispatch":{"kind":kind,"jobId":id},"idempotencyKey":key}));
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(response.body["ok"], true, "{}", response.body);
    response.body
}
fn native_health_wait_worker_stopped(s: &crate::service::TradeAssemblyService, id: &str) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if crate::service::session_dispatch::worker_status_for_test(s, id)["running"] == false {
            return;
        }
        assert!(
            std::time::Instant::now() < end,
            "health worker failed to stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}
struct NativeHealthAvailabilityFault {
    visible: std::path::PathBuf,
    retained: std::path::PathBuf,
    bytes: Vec<u8>,
    active: bool,
}
impl NativeHealthAvailabilityFault {
    fn new(root: &std::path::Path) -> Self {
        let visible = root.join("facts-health/SYNTHETIC_BROKER_ONLY");
        let retained = root.join("facts-health/HEALTH_FIXTURE_UNAVAILABLE");
        let bytes = fs::read(&visible).unwrap();
        assert_eq!(bytes.as_slice(), b"synthetic only\n");
        assert!(!retained.exists());
        fs::rename(&visible, &retained).unwrap();
        Self {
            visible,
            retained,
            bytes,
            active: true,
        }
    }
    fn restore(&mut self) {
        if self.active {
            assert_eq!(fs::read(&self.retained).unwrap(), self.bytes);
            fs::rename(&self.retained, &self.visible).unwrap();
            self.active = false;
        }
    }
}
impl Drop for NativeHealthAvailabilityFault {
    fn drop(&mut self) {
        self.restore();
    }
}
struct NativeHealthStopOnDrop {
    service: crate::service::TradeAssemblyService,
    job: String,
}
impl Drop for NativeHealthStopOnDrop {
    fn drop(&mut self) {
        let _=self.service.handle_http("POST","/scheduler/run",json!({"accountMode":"paper","sessionDispatch":{"kind":"stop_worker","jobId":self.job},"idempotencyKey":"health-fixture-drop-stop"}));
    }
}
fn exercise_native_automatic_health_refresh(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    revision: &str,
    snapshot: &Value,
    root: &std::path::Path,
) {
    use crate::service::{
        data_collections as collections, session_dispatch as dispatch,
        session_schedules as schedules,
    };
    let before_pins = [
        native_health_source_pin(s, "fixture.broker.v2"),
        native_health_source_pin(s, "fixture.execution.facts"),
    ];
    let run = s
        .runtime()
        .storage
        .get_json("execution_runs", "run_fixture-v2-activation")
        .unwrap()
        .unwrap();
    let config_id = run["configId"].as_str().unwrap();
    let config = s
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        config["typedExecution"]["automaticExecution"]["accountHealthRefresh"],
        json!({"schemaVersion":"tradeassembly.automatic_account_health_refresh.v1","refreshIntervalMs":60000})
    );
    let graph = s
        .runtime()
        .storage
        .get_json("capability_graph_revisions", revision)
        .unwrap()
        .unwrap();
    let calendar = collections::acquire(
        s,
        collections::CollectionCommand {
            scope: scope.clone(),
            capability_graph_revision: revision.into(),
            acquisition_node: "calendar".into(),
            mode: "paper".into(),
            response_schema_ref:
                tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA.into(),
            input: json!({}),
        },
        &ctx("health-calendar"),
    )
    .unwrap();
    let now = s.runtime().clock.trusted_now_ms().unwrap();
    let date = chrono::DateTime::from_timestamp_millis(now)
        .unwrap()
        .date_naive()
        .to_string();
    let schedule=s.handle_http("POST","/product/strategies/session-schedules/prepare",json!({"schedule":schedules::ScheduleCommand{scope:scope.clone(),capability_graph_revision:revision.into(),mode:"paper".into(),trigger_ref:"preopen".into(),session_date:date,calendar_collection_id:calendar.collection_id.clone(),execution_calendar_collection_id:calendar.collection_id,knowledge_cutoff_ms:now},"idempotencyKey":"health-schedule"}));
    assert_eq!(schedule.status, 200, "{}", schedule.body);
    let due = schedule.body["schedule"]["occurrence"]["dueAtMs"]
        .as_i64()
        .unwrap();
    assert!(
        due - now > 500000,
        "entry must genuinely remain scheduled after the whole health-only proof"
    );
    let request = dispatch::DispatchRequest {
        occurrence_id: schedule.body["schedule"]["occurrence"]["occurrenceId"]
            .as_str()
            .unwrap()
            .into(),
        scope: scope.clone(),
        capability_graph_revision: revision.into(),
        mode: "paper".into(),
        subjects: dispatch::SubjectPolicy::AllSelected { max_members: 1 },
        event_start_ms: snapshot["start"].as_i64().unwrap(),
        event_end_ms: snapshot["start"].as_i64().unwrap() + 60000,
        interval_ms: 60000,
        max_frames_per_step: 1,
        source_captures_by_subject: BTreeMap::new(),
        execution_activation_id: Some("fixture-v2-activation".into()),
    };
    let enqueued=s.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"enqueue","job":request},"idempotencyKey":"health-enqueue"}));
    assert_eq!(enqueued.status, 200, "{}", enqueued.body);
    assert_eq!(enqueued.body["ok"], true, "{}", enqueued.body);
    let id = enqueued.body["body"]["job"]["jobId"].as_str().unwrap();
    let _stop = NativeHealthStopOnDrop {
        service: s.clone(),
        job: id.into(),
    };
    let finance = native_health_financial_state(s);
    native_health_scheduler(s, "start_worker", id, "health-start-initial");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    loop {
        let status = dispatch::worker_status_for_test(s, id);
        if status["iterations"].as_u64().unwrap_or(0) > 0 {
            break;
        }
        assert_eq!(status["running"], true, "{status}");
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not reach future-entry wait"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let stopped = native_health_scheduler(s, "stop_worker", id, "health-stop-before-expiry");
    native_health_wait_worker_stopped(s, id);
    let baseline_reads = [
        native_health_reads(root, "synthetic-broker"),
        native_health_reads(root, "facts-health"),
    ];
    assert!(baseline_reads.iter().all(|n| *n >= 1));
    let observations = [
        s.runtime()
            .plugins
            .get_instance("fixture.broker.v2")
            .unwrap()
            .unwrap(),
        s.runtime()
            .plugins
            .get_instance("fixture.execution.facts")
            .unwrap()
            .unwrap(),
    ];
    let last_checked = observations
        .iter()
        .map(|r| r["health"]["checkedAtMs"].as_i64().unwrap())
        .max()
        .unwrap();
    let expire_at = last_checked.checked_add(120001).unwrap();
    let mut waits = Vec::new();
    loop {
        let now = s.runtime().clock.trusted_now_ms().unwrap();
        if now >= expire_at {
            break;
        }
        let ms = (expire_at - now).min(45000);
        eprintln!("native automatic health: worker STOPPED; trusted now {now}, genuine observations expire after {expire_at}; wait {ms}ms");
        waits.push(json!({"waitStartedAtMs":now,"sleepMs":ms,"targetMs":expire_at}));
        write_screen_interface_evidence(
            "automatic_health",
            &format!("expiry-wait-{}.json", waits.len()),
            &json!({"lastCheckedAtMs":last_checked,"strictlyExpiredAtMs":expire_at,"waits":waits,"stoppedWorker":stopped}),
        );
        std::thread::sleep(std::time::Duration::from_millis(u64::try_from(ms).unwrap()));
    }
    let expired_now = s.runtime().clock.trusted_now_ms().unwrap();
    assert!(expired_now - last_checked > 120000);
    assert_eq!(
        [
            native_health_reads(root, "synthetic-broker"),
            native_health_reads(root, "facts-health")
        ],
        baseline_reads,
        "disabled worker performed a health read"
    );
    for node in ["exec_broker", "exec_account"] {
        let selected = graph["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["nodeId"] == node)
            .unwrap()["selected"]
            .clone();
        let refused = crate::broker_submission::account_identity::current(
            s.runtime().plugins.as_ref(),
            s.runtime().plugin_packages.as_ref(),
            s.runtime().credentials.as_ref(),
            &selected,
            expired_now,
            "paper",
        );
        assert_eq!(
            refused.err().unwrap(),
            "canonical_account_observation_invalid"
        );
    }
    assert_eq!(native_health_financial_state(s), finance);
    write_screen_interface_evidence(
        "automatic_health",
        "strict-expiry-before-refresh.json",
        &json!({"observations":observations,"expiredAtMs":expired_now,"unchangedTtlMs":120000,"financialStateUnchanged":true,"readCounts":baseline_reads}),
    );
    let mut unavailable = NativeHealthAvailabilityFault::new(root);
    native_health_scheduler(s, "start_worker", id, "health-start-expired-with-fault");
    // Four genuine failed provider attempts give a natural eight-second backoff,
    // long enough to prove restart does not turn a future timer into permission.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let failed = loop {
        if let Some(state) = native_health_state(s) {
            if state["targets"].as_array().unwrap().iter().any(|t| {
                t["instanceRef"] == "fixture.execution.facts"
                    && t["failures"].as_u64().unwrap_or(0) >= 4
            }) {
                break state;
            }
        }
        let status = dispatch::worker_status_for_test(s, id);
        assert_eq!(status["running"], true, "{status}");
        assert!(
            std::time::Instant::now() < deadline,
            "native degraded/null backoff did not occur"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    dispatch::halt_worker_for_test(s, id);
    native_health_wait_worker_stopped(s, id);
    let halted = dispatch::worker_status_for_test(s, id);
    assert_eq!(
        halted["lastError"],
        "fixture_worker_stopped_without_disabling_intent"
    );
    let retained_intent = s
        .runtime()
        .storage
        .get_json("session_dispatch_worker_intents_v1", id)
        .unwrap()
        .unwrap();
    assert_eq!(retained_intent["enabled"], true);
    let target = failed["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["instanceRef"] == "fixture.execution.facts")
        .unwrap();
    assert!(
        target["pending"].is_null(),
        "known sealed failed read must permit a later NEW attempt"
    );
    assert_eq!(target["lastOutcome"]["classification"], "failed");
    let retry_at = target["nextAttemptAtMs"].as_i64().unwrap();
    let degraded = s
        .runtime()
        .plugins
        .get_instance("fixture.execution.facts")
        .unwrap()
        .unwrap();
    assert_eq!(degraded["health"]["state"], "degraded");
    assert!(degraded["health"]["account"].is_null());
    assert_eq!(
        dispatch::worker_status_for_test(s, id)["accountHealthRefresh"]["status"],
        "blocked"
    );
    assert_eq!(
        [
            native_health_source_pin(s, "fixture.broker.v2"),
            native_health_source_pin(s, "fixture.execution.facts")
        ],
        before_pins
    );
    assert_eq!(native_health_financial_state(s), finance);
    let requests_before = native_health_facts_requests(s);
    unavailable.restore();
    let restored =
        crate::service::TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone())
            .for_authenticated_invocation(
                &s.invocation_owner().unwrap().issuer,
                &s.invocation_owner().unwrap().subject,
                None,
                None,
            );
    assert_eq!(
        restored
            .runtime()
            .storage
            .get_json("session_dispatch_worker_intents_v1", id)
            .unwrap()
            .unwrap(),
        retained_intent
    );
    let resume = restored.handle_http(
        "POST",
        "/scheduler/run",
        json!({"sessionDispatch":{"kind":"resume_workers"},"idempotencyKey":"health-resume-owned"}),
    );
    assert_eq!(resume.status, 200, "{}", resume.body);
    assert_eq!(resume.body["ok"], true, "{}", resume.body);
    assert_eq!(resume.body["body"]["workersResumed"], 1, "{}", resume.body);
    let mut backoff_checks = 0;
    loop {
        let before = restored.runtime().clock.trusted_now_ms().unwrap();
        if before >= retry_at {
            break;
        }
        let count = native_health_facts_requests(&restored);
        let after = restored.runtime().clock.trusted_now_ms().unwrap();
        if after < retry_at {
            assert_eq!(
                count, requests_before,
                "read dispatched before natural backoff deadline"
            );
            backoff_checks += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        backoff_checks > 0,
        "restart consumed the entire natural backoff; cannot claim a timer gate observation"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let fresh = loop {
        let status = dispatch::worker_status_for_test(&restored, id);
        if status["accountHealthRefresh"]["status"] == "ready" {
            break status;
        }
        assert_eq!(status["running"], true, "{status}");
        assert!(
            std::time::Instant::now() < deadline,
            "native provider did not recover after restored availability"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    native_health_scheduler(&restored, "stop_worker", id, "health-stop-final");
    native_health_wait_worker_stopped(&restored, id);
    let final_state = native_health_state(&restored).unwrap();
    let final_target = final_state["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["instanceRef"] == "fixture.execution.facts")
        .unwrap();
    assert_eq!(final_target["failures"], 0);
    assert!(final_target["error"].is_null());
    assert!(final_target["pending"].is_null());
    assert_eq!(final_target["lastOutcome"]["classification"], "fresh");
    assert_ne!(
        target["lastOutcome"]["recordId"],
        final_target["lastOutcome"]["recordId"]
    );
    let final_reads = [
        native_health_reads(root, "synthetic-broker"),
        native_health_reads(root, "facts-health"),
    ];
    assert_eq!(final_reads, [baseline_reads[0] + 1, baseline_reads[1] + 1]);
    assert!(native_health_facts_requests(&restored) > requests_before);
    assert_eq!(
        [
            native_health_source_pin(&restored, "fixture.broker.v2"),
            native_health_source_pin(&restored, "fixture.execution.facts")
        ],
        before_pins
    );
    assert_eq!(
        restored
            .runtime()
            .storage
            .get_json("execution_configs", config_id)
            .unwrap()
            .unwrap(),
        config
    );
    assert_eq!(
        restored
            .runtime()
            .storage
            .get_json("capability_graph_revisions", revision)
            .unwrap()
            .unwrap(),
        graph
    );
    let epoch =
        chrono::DateTime::from_timestamp_millis(restored.runtime().clock.trusted_now_ms().unwrap())
            .unwrap()
            .to_rfc3339();
    let current = restored.handle_http(
        "POST",
        "/capability-graph-revisions/currentness",
        json!({"revisionId":revision,"evaluationEpoch":epoch}),
    );
    assert_eq!(current.status, 200, "{}", current.body);
    assert_eq!(current.body["current"], true, "{}", current.body);
    assert_eq!(native_health_financial_state(&restored), finance);
    assert_eq!(
        fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap_or_default(),
        ""
    );
    let inspected = native_health_scheduler(&restored, "inspect", id, "health-job-final");
    assert_eq!(inspected["body"]["job"]["status"], "pending");
    assert!(inspected["body"]["job"]["startedAtMs"].is_null());
    assert!(restored.runtime().clock.trusted_now_ms().unwrap() < due);
    write_screen_interface_evidence(
        "automatic_health",
        "native-expiry-backoff-service-recreation.json",
        &json!({"sourcePins":before_pins,"scope":scope,"graphRevision":revision,"configId":config_id,"job":inspected,"expiredAtMs":expired_now,"genuineLastCheckedAtMs":last_checked,"ttlMs":120000,"failureState":failed,"degradedObservation":degraded,"retainedIntent":retained_intent,"resume":resume.body,"backoffChecks":backoff_checks,"naturalRetryAtMs":retry_at,"ready":fresh,"finalState":final_state,"baselineReadCounts":baseline_reads,"finalReadCounts":final_reads,"graphCurrentness":current.body,"financialStateUnchanged":true,"brokerCalls":0,"realProviderCalls":0,"clockInjected":false,"manuallyRedatedObservations":0,"serviceRecreationQualified":true,"actualProcessRestartQualified":false,"terminalOwnedPositionHealthQualified":false,"automaticEntryOrCloseQualified":false,"osSandboxQualified":false,"qualification":"native authored same-source account.health through normal Core HTTP/C3/worker; real TTL wait, genuine degraded/null failure, natural backoff, retained owned intent and service recreation; mock Warden/traced fixture sandbox; no real provider or broker"}),
    );
}
#[test]
#[ignore = "explicit native genuine >120s account-health expiry/backoff/service-recreation fixture; synthetic provider only"]
fn native_automatic_account_health_expiry_backoff_and_service_recreation() {
    for name in [
        "CAPTURE_FIXTURE_BINARY",
        "CAPTURE_FIXTURE_SHA256",
        "SCREEN_INTERFACE_EVIDENCE_DIR",
    ] {
        assert!(
            std::env::var_os(name).is_some(),
            "explicit existing frozen native fixture/evidence input required: {name}"
        );
    }
    let evidence =
        std::path::PathBuf::from(std::env::var_os("SCREEN_INTERFACE_EVIDENCE_DIR").unwrap());
    assert!(
        !evidence.join("financial_screen_auto_health").exists(),
        "use a NEW owned health evidence case; no unchanged retries"
    );
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    native_execution_plan_fixture(Some("financial_screen_auto_health"));
}
