use super::*;
fn request() -> DispatchRequest {
    DispatchRequest {
        occurrence_id: "occurrence_test".into(),
        scope: data_plans::PlanScope {
            strategy_id: "owned".into(),
            strategy_version_id: "v1".into(),
            strategy_spec_hash: "hash".into(),
        },
        capability_graph_revision: "graph".into(),
        mode: "paper".into(),
        subjects: SubjectPolicy::AllSelected { max_members: 1 },
        event_start_ms: 86400000,
        event_end_ms: 172800000,
        interval_ms: 86400000,
        max_frames_per_step: 1,
        source_captures_by_subject: BTreeMap::new(),
        execution_activation_id: None,
    }
}
#[test]
fn historical_universe_and_unbounded_work_reject_before_lookup_or_effect() {
    let service = TradeAssemblyService::from_runtime(
        ":memory:",
        crate::adapters::local::test_runtime(":memory:"),
    );
    let mut r = request();
    r.mode = "backtest".into();
    assert_eq!(
        validate_request(&service, &r).unwrap_err(),
        "session_dispatch_historical_universe_unsupported"
    );
    r.mode = "live".into();
    assert_eq!(
        validate_request(&service, &r).unwrap_err(),
        "session_dispatch_request_invalid"
    );
    r.mode = "paper".into();
    r.subjects = SubjectPolicy::AllSelected { max_members: 33 };
    assert_eq!(
        validate_request(&service, &r).unwrap_err(),
        "session_dispatch_subjects_invalid"
    );
    r.subjects = SubjectPolicy::AllSelected { max_members: 32 };
    r.event_end_ms = r.event_start_ms + 4096 * 86400000;
    assert_eq!(
        validate_request(&service, &r).unwrap_err(),
        "session_dispatch_work_bounds_invalid"
    );
}
#[test]
fn mcp_foreign_historical_job_read_uses_error_envelope_without_child_effects() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("service.db").to_string_lossy().into_owned();
    let service =
        TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db));
    let alice = service.for_authenticated_invocation("test", "alice", None, None);
    alice.bind_owned_object("strategy", "owned").unwrap();
    let who = hash(&alice.invocation_owner().unwrap()).unwrap();
    let r = request();
    let id = job_id(&r, &who).unwrap();
    let mut j = Job {
        schema_version: "tradeassembly.session_dispatch.v1".into(),
        job_id: id.clone(),
        request: r,
        core_revision: "1".repeat(40),
        owner_sha256: who,
        schedule_sha256: "a".repeat(64),
        created_at_ms: 1000,
        started_at_ms: None,
        status: Status::Pending,
        paused: false,
        generation: 0,
        attempts: 0,
        next_attempt_at_ms: 1000,
        error: None,
        record_sha256: String::new(),
        fencing_token: 0,
        queue_message_id: None,
        pending: None,
        results: BTreeMap::new(),
        subjects: None,
    };
    j.record_sha256 = record_hash(&j).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            JOBS,
            &id,
            value(&j).unwrap(),
            &context("seed-owned-job").unwrap(),
        )
        .unwrap();
    let bob = service.for_authenticated_invocation("test", "bob", None, None);
    let response=bob.call_mcp_tool("tradeassembly.execution.scheduler.run",json!({"sessionDispatch":{"kind":"inspect","jobId":id},"idempotencyKey":"foreign-session-job"}));
    assert_eq!(response["isError"], true, "{response}");
    assert!(response.get("job").is_none());
    for ns in [
        "compiled_data_plan_revisions_v1",
        "plugin_operation_receipts",
        "orders",
    ] {
        assert!(service.runtime().storage.list_json(ns).unwrap().is_empty());
    }
    let original = read(&alice, &id).unwrap().0;
    let mut tampered = value(&original).unwrap();
    tampered["paused"] = json!(true);
    service
        .runtime()
        .storage
        .put_json(JOBS, &id, tampered, &context("tampered-job").unwrap())
        .unwrap();
    assert!(read(&alice, &id).is_err());
}
