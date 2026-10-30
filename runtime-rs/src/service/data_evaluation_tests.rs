use super::*;
use crate::ports::{
    AuthorityContext, PluginOperationPort, PluginOperationResponse, PortDescriptor, VersionedPort,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
const INTERVAL: i64 = 86_400_000;
type InvocationHook = Box<dyn Fn(&PluginOperationRequest) + Send + Sync>;

#[derive(Default)]
struct FixtureOperations {
    requests: Mutex<Vec<DataEvaluationRequest>>,
    revised: AtomicBool,
    delayed_first: AtomicBool,
    research_run: AtomicBool,
    fail: AtomicBool,
    incomplete: AtomicBool,
    premature: AtomicBool,
    gap: AtomicBool,
    future: AtomicBool,
    foreign_state: AtomicBool,
    nondeterministic: AtomicBool,
    hook: Mutex<Option<InvocationHook>>,
}
impl VersionedPort for FixtureOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginOperationPort for FixtureOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        _context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        assert!(!request.operation_id.starts_with("broker."));
        if let Some(hook) = self.hook.lock().unwrap().take() {
            hook(request);
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err("fixture_calculation_failed".into());
        }
        let payload: DataEvaluationRequest = serde_json::from_value(request.input.clone()).unwrap();
        self.requests.lock().unwrap().push(payload.clone());
        let schema = SchemaIdentity {
            schema_ref: "schema://test/series@1".into(),
            sha256: hash(&json!({"type":"array","items":{"type":"number"}}))?,
        };
        let mut points = vec![];
        let mut state = BTreeMap::new();
        if request.operation_id == "provide" {
            let mut start = payload.window.event_start_ms;
            while start + INTERVAL <= payload.window.event_end_ms
                && points.len() < payload.max_points as usize
            {
                let end = start + INTERVAL;
                let mut point = DataPoint {
                    instrument_id: "TEST".into(),
                    source_event_id: format!("fixture:{end}"),
                    provenance: BatchProvenance {
                        content_sha256: String::new(),
                        upstream_content_sha256: vec![],
                        event_start_ms: start,
                        event_end_ms: end,
                        known_at_ms: end,
                        arrived_at_ms: payload.window.as_of_ms,
                        completed: true,
                        source_revision: if self.revised.load(Ordering::SeqCst) {
                            "revision.2"
                        } else {
                            "revision.1"
                        }
                        .into(),
                    },
                    result: PointValue::Ready(json!(end / INTERVAL)),
                };
                if self.incomplete.load(Ordering::SeqCst) {
                    point.provenance.completed = false;
                }
                if self.future.load(Ordering::SeqCst) {
                    point.provenance.known_at_ms = payload.window.as_of_ms + 1;
                    point.provenance.arrived_at_ms = payload.window.as_of_ms + 1;
                }
                if self.delayed_first.load(Ordering::SeqCst) && start == 10 * INTERVAL {
                    point.provenance.known_at_ms = 15 * INTERVAL;
                }
                point.provenance.content_sha256 = point_sha256(&point).unwrap();
                if self.gap.load(Ordering::SeqCst) && end == 12 * INTERVAL {
                    start = end;
                    continue;
                }
                points.push(point);
                start = end;
            }
        } else {
            assert_eq!(request.operation_id, "derive");
            for input in &payload.inputs["source"].points {
                assert!(input.provenance.event_end_ms <= payload.window.event_end_ms);
                let slot = if self.research_run.load(Ordering::SeqCst) {
                    RESEARCH_RUN_STATE_SLOT
                } else {
                    &input.instrument_id
                };
                let seen = payload
                    .prior_state
                    .get(slot)
                    .and_then(|s| s["seen"].as_u64())
                    .unwrap_or(0)
                    + 1;
                state.insert(slot.to_owned(), json!({"seen":seen}));
                let mut point = input.clone();
                point.result = if self.premature.load(Ordering::SeqCst)
                    || seen >= payload.parameters["window"].as_u64().unwrap()
                {
                    input.result.clone()
                } else {
                    PointValue::WarmingUp
                };
                points.push(point);
            }
        }
        if request.operation_id == "derive" && self.foreign_state.load(Ordering::SeqCst) {
            state = BTreeMap::from([("OTHER".into(), json!({"seen":1}))]);
        }
        let response = DataEvaluationResponse {
            schema_version: EvaluationVersion::V1,
            outputs: BTreeMap::from([(
                "value".into(),
                DataBatch {
                    schema,
                    shape: DataShape::Series,
                    points,
                },
            )]),
            state,
        };
        let value = serde_json::to_value(response).unwrap();
        Ok(PluginOperationResponse {
            correlation_id: request.correlation_id.clone(),
            schema_ref: EVALUATION_RESPONSE_SCHEMA.into(),
            payload: value.clone(),
            observed_at_ms: payload.window.as_of_ms,
            source_event_id: request.correlation_id.clone(),
            content_hash: hash(&value)?,
            freshness_state: "fresh".into(),
            evidence_refs: vec![],
            deterministic: request.operation_id == "provide"
                || !self.nondeterministic.load(Ordering::SeqCst),
            replayable: true,
            provider_outcome_id: None,
            reconciliation_required: false,
        })
    }
}
fn context(id: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(id).unwrap(),
    )
}
fn rig() -> (
    tempfile::TempDir,
    TradeAssemblyService,
    data_plans::StoredDataPlan,
    Arc<FixtureOperations>,
) {
    let operations = Arc::new(FixtureOperations::default());
    let (directory, service, scope, request) =
        super::super::data_plans::tests::fixture_service(Some(operations.clone()));
    let plan = data_plans::save(&service, scope, request, &context("create-plan")).unwrap();
    (directory, service, plan, operations)
}
fn request(
    plan: &data_plans::StoredDataPlan,
    run: &str,
    start: i64,
    end: i64,
    prior: Option<String>,
) -> EvaluationStep {
    EvaluationStep {
        run_id: run.into(),
        plan_revision_id: plan.revision_id.clone(),
        window: EvaluationWindow {
            event_start_ms: start * INTERVAL,
            event_end_ms: end * INTERVAL,
            as_of_ms: end * INTERVAL,
        },
        expected_checkpoint_sha256: prior,
    }
}
fn terminal(receipt: &EvaluationReceipt) -> Vec<DataPoint> {
    receipt
        .outputs
        .get("derived")
        .into_iter()
        .flatten()
        .flat_map(|r| r.outputs["value"].points.clone())
        .collect()
}

#[test]
fn data_evaluation_batch_and_incremental_share_state_readiness_and_causal_outputs() {
    let (_directory, service, plan, operations) = rig();
    let batch = step(
        &service,
        request(&plan, "batch", 10, 15, None),
        &context("batch"),
    )
    .unwrap();
    assert_eq!(batch.status, EvaluationStatus::Ready);
    assert_eq!(batch.frames_processed, 5);
    let points = terminal(&batch);
    assert!(points[..4]
        .iter()
        .all(|p| matches!(p.result, PointValue::WarmingUp)));
    assert!(matches!(points[4].result, PointValue::Ready(_)));
    let mut prior = None;
    let mut streamed = vec![];
    let mut last = None;
    for index in 10..15 {
        let receipt = step(
            &service,
            request(&plan, "stream", index, index + 1, prior),
            &context(&format!("stream-{index}")),
        )
        .unwrap();
        prior = Some(receipt.checkpoint.checkpoint_sha256.clone());
        streamed.extend(terminal(&receipt));
        last = Some(receipt);
    }
    assert_eq!(
        points
            .iter()
            .map(|p| &p.provenance.content_sha256)
            .collect::<Vec<_>>(),
        streamed
            .iter()
            .map(|p| &p.provenance.content_sha256)
            .collect::<Vec<_>>()
    );
    assert_eq!(batch.checkpoint.states, last.unwrap().checkpoint.states);
    assert!(operations
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| !r.inputs.is_empty())
        .all(|r| r.inputs.values().all(
            |b| b.points.len() == 1 && b.points[0].provenance.known_at_ms == r.window.as_of_ms
        )));
}
#[test]
fn data_evaluation_duplicate_is_noop_and_revision_requires_explicit_replay() {
    let (_directory, service, plan, operations) = rig();
    let first = step(
        &service,
        request(&plan, "run", 10, 15, None),
        &context("initial"),
    )
    .unwrap();
    let mut duplicate = request(
        &plan,
        "run",
        10,
        15,
        Some(first.checkpoint.checkpoint_sha256.clone()),
    );
    duplicate.window.as_of_ms += 1;
    let replay = step(&service, duplicate.clone(), &context("duplicate")).unwrap();
    assert_eq!(replay.status, EvaluationStatus::NoNewData);
    assert_eq!(replay.checkpoint, first.checkpoint);
    operations.revised.store(true, Ordering::SeqCst);
    duplicate.window.as_of_ms += 1;
    assert_eq!(
        step(&service, duplicate, &context("revision")).unwrap_err(),
        "data_evaluation_revision_requires_replay"
    );
    let runtime = service.runtime();
    let saved = runtime.storage.list_json(RUNS).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0].1["checkpointSha256"],
        first.checkpoint.checkpoint_sha256
    );
}
#[test]
fn data_evaluation_failure_and_atomic_commit_failure_do_not_advance_state_or_cursor() {
    let (directory, service, plan, operations) = rig();
    operations.fail.store(true, Ordering::SeqCst);
    let input = request(&plan, "recover", 10, 15, None);
    assert_eq!(
        step(&service, input.clone(), &context("retry")).unwrap_err(),
        "fixture_calculation_failed"
    );
    assert!(service
        .runtime()
        .storage
        .list_json(RUNS)
        .unwrap()
        .is_empty());
    operations.fail.store(false, Ordering::SeqCst);
    let db = directory.path().join("service.db");
    let connection = rusqlite::Connection::open(db).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_evaluation BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='data_evaluation_checkpoints_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(step(&service, input.clone(), &context("retry")).is_err());
    for namespace in [RUNS, SOURCES, RECEIPTS] {
        assert!(service
            .runtime()
            .storage
            .list_json(namespace)
            .unwrap()
            .is_empty());
    }
    connection
        .execute_batch("DROP TRIGGER fail_evaluation;")
        .unwrap();
    let recovered = step(&service, input.clone(), &context("retry")).unwrap();
    assert_eq!(recovered.frames_processed, 5);
    assert_eq!(recovered.checkpoint.states["derived"]["TEST"]["seen"], 5);
    let calls = operations.requests.lock().unwrap().len();
    assert_eq!(step(&service, input, &context("retry")).unwrap(), recovered);
    assert_eq!(operations.requests.lock().unwrap().len(), calls);
}

#[test]
fn data_evaluation_incomplete_source_waits_without_consuming_observations() {
    let (_dir, service, plan, operations) = rig();
    operations.incomplete.store(true, Ordering::SeqCst);
    let waiting = step(
        &service,
        request(&plan, "waiting", 10, 15, None),
        &context("waiting"),
    )
    .unwrap();
    assert_eq!(waiting.status, EvaluationStatus::WaitingForInput);
    assert_eq!(waiting.frames_processed, 0);
    assert!(waiting.checkpoint.cursors.is_empty());
    assert!(waiting.checkpoint.states.is_empty());
    assert!(operations
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.inputs.is_empty()));
}
#[test]
fn data_evaluation_invalid_inputs_and_premature_outputs_fail_without_state_progress() {
    for fault in [
        "gap",
        "future",
        "premature",
        "foreign_state",
        "nondeterministic",
    ] {
        let (_dir, service, plan, operations) = rig();
        let (flag, expected) = match fault {
            "gap" => (&operations.gap, "data_evaluation_gap_requires_policy"),
            "future" => (&operations.future, "data_evaluation_event_contract_invalid"),
            "premature" => (&operations.premature, "data_evaluation_premature_output"),
            "foreign_state" => (
                &operations.foreign_state,
                "data_evaluation_state_partition_invalid",
            ),
            _ => (
                &operations.nondeterministic,
                "data_evaluation_response_contract_invalid",
            ),
        };
        flag.store(true, Ordering::SeqCst);
        assert_eq!(
            step(
                &service,
                request(&plan, fault, 10, 15, None),
                &context(fault)
            )
            .unwrap_err(),
            expected
        );
        for namespace in [RUNS, SOURCES, RECEIPTS] {
            assert!(service
                .runtime()
                .storage
                .list_json(namespace)
                .unwrap()
                .is_empty());
        }
    }
}
#[test]
fn data_evaluation_lease_takeover_prevents_stale_worker_commit() {
    let (_dir, service, plan, operations) = rig();
    let runtime = service.runtime();
    *operations.hook.lock().unwrap() = Some(Box::new(move |request| {
        let old = runtime
            .leases
            .current(&request.activation_id)
            .unwrap()
            .unwrap();
        runtime.leases.release(&old).unwrap();
        runtime
            .leases
            .acquire(
                &request.activation_id,
                "replacement",
                runtime.clock.now_ms(),
                60_000,
            )
            .unwrap()
            .unwrap();
    }));
    assert_eq!(
        step(
            &service,
            request(&plan, "fenced", 10, 15, None),
            &context("fenced")
        )
        .unwrap_err(),
        "data_evaluation_lease_stale"
    );
    assert!(service
        .runtime()
        .storage
        .list_json(RUNS)
        .unwrap()
        .is_empty());
    let resources = service.runtime().storage.list_json(FENCES).unwrap();
    let resource = format!("data-evaluation:{}", resources[0].0);
    assert_eq!(
        service
            .runtime()
            .leases
            .current(&resource)
            .unwrap()
            .unwrap()
            .owner,
        "replacement"
    );
}
#[test]
fn data_evaluation_cached_receipt_is_checked_and_idempotency_cannot_change_inputs() {
    let (_dir, service, plan, operations) = rig();
    let input = request(&plan, "cache", 10, 15, None);
    let original = step(&service, input.clone(), &context("cache")).unwrap();
    let calls = operations.requests.lock().unwrap().len();
    let mut conflicting = input.clone();
    conflicting.window.as_of_ms += 1;
    assert_eq!(
        step(&service, conflicting, &context("cache")).unwrap_err(),
        "data_evaluation_idempotency_conflict"
    );
    assert_eq!(operations.requests.lock().unwrap().len(), calls);
    let runtime = service.runtime();
    let row = runtime.storage.list_json(RECEIPTS).unwrap().pop().unwrap();
    let mut value = row.1;
    value["framesProcessed"] = json!(0);
    runtime
        .storage
        .put_json(RECEIPTS, &row.0, value, &context("tamper"))
        .unwrap();
    assert_eq!(
        step(&service, input, &context("cache")).unwrap_err(),
        "data_evaluation_receipt_invalid"
    );
    assert_eq!(
        runtime.storage.list_json(RUNS).unwrap()[0].1["checkpointSha256"],
        original.checkpoint.checkpoint_sha256
    );
}

#[test]
fn data_evaluation_missing_initial_coverage_cannot_silently_shift_the_sample() {
    let (_dir, service, plan, operations) = rig();
    operations.gap.store(true, Ordering::SeqCst);
    assert_eq!(
        step(
            &service,
            request(&plan, "initial-gap", 11, 15, None),
            &context("initial-gap")
        )
        .unwrap_err(),
        "data_evaluation_gap_requires_policy"
    );
    assert!(service
        .runtime()
        .storage
        .list_json(RUNS)
        .unwrap()
        .is_empty());
}

#[test]
fn data_evaluation_prior_state_availability_survives_batch_and_restart() {
    let operations = Arc::new(FixtureOperations::default());
    operations.delayed_first.store(true, Ordering::SeqCst);
    let (_dir, service, scope, mut pipeline) =
        super::super::data_plans::tests::fixture_service(Some(operations.clone()));
    pipeline
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "derived")
        .unwrap()
        .parameters = json!({"window":1});
    let plan = data_plans::save(&service, scope, pipeline, &context("delayed-plan")).unwrap();
    let delayed_request = |run: &str, start, end, prior| {
        let mut r = request(&plan, run, start, end, prior);
        r.window.as_of_ms = 15 * INTERVAL;
        r
    };
    let batch = step(
        &service,
        delayed_request("batch-delayed", 10, 12, None),
        &context("batch-delayed"),
    )
    .unwrap();
    let first = step(
        &service,
        delayed_request("stream-delayed", 10, 11, None),
        &context("stream-delayed-a"),
    )
    .unwrap();
    let mut before_available = request(
        &plan,
        "stream-delayed",
        11,
        12,
        Some(first.checkpoint.checkpoint_sha256.clone()),
    );
    before_available.window.as_of_ms = 12 * INTERVAL;
    assert_eq!(
        step(&service, before_available, &context("too-early-state")).unwrap_err(),
        "data_evaluation_state_not_available_at_cutoff"
    );
    let resumed = step(
        &service,
        delayed_request(
            "stream-delayed",
            11,
            12,
            Some(first.checkpoint.checkpoint_sha256),
        ),
        &context("stream-delayed-b"),
    )
    .unwrap();
    for point in [terminal(&batch)[1].clone(), terminal(&resumed)[0].clone()] {
        assert!(
            point.provenance.known_at_ms >= 15 * INTERVAL,
            "state became available later than this output: {point:?}"
        );
    }
    let calls = operations.requests.lock().unwrap();
    assert!(calls
        .iter()
        .filter(|r| r.node_id == "derived" && r.window.event_end_ms == 12 * INTERVAL)
        .all(|r| r.window.as_of_ms >= 15 * INTERVAL));
}

#[test]
fn data_evaluation_research_run_state_has_a_known_slot_and_is_isolated_on_resume() {
    let operations = Arc::new(FixtureOperations::default());
    operations.research_run.store(true, Ordering::SeqCst);
    let (_dir, service, scope, pipeline) =
        super::super::data_plans::tests::fixture_service_with_state(
            Some(operations.clone()),
            StatePartition::ResearchRun,
            "backtest",
        );
    let plan = data_plans::save(&service, scope, pipeline, &context("research-plan")).unwrap();
    let first = step(
        &service,
        request(&plan, "research-a", 10, 12, None),
        &context("research-a"),
    )
    .unwrap();
    let other = step(
        &service,
        request(&plan, "research-b", 10, 11, None),
        &context("research-b"),
    )
    .unwrap();
    let resumed = step(
        &service,
        request(
            &plan,
            "research-a",
            12,
            13,
            Some(first.checkpoint.checkpoint_sha256),
        ),
        &context("research-a-resume"),
    )
    .unwrap();
    assert_eq!(
        other.checkpoint.states["derived"][RESEARCH_RUN_STATE_SLOT]["seen"],
        1
    );
    assert_eq!(
        resumed.checkpoint.states["derived"][RESEARCH_RUN_STATE_SLOT]["seen"],
        3
    );
    assert_eq!(
        operations
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.node_id == "derived" && r.prior_state.is_empty())
            .count(),
        2
    );
}
