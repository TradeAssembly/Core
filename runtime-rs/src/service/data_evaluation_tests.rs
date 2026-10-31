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
    unavailable_a: AtomicBool,
    fail_b: AtomicBool,
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
        if request.operation_id == "derive"
            && payload.instruments == ["B"]
            && self.fail_b.load(Ordering::SeqCst)
        {
            return Err("fixture_partition_b_failed".into());
        }
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
                for instrument in &payload.instruments {
                    let mut point = point.clone();
                    point.instrument_id = instrument.clone();
                    if instrument != "TEST" {
                        point.source_event_id = format!("fixture:{instrument}:{end}");
                    }
                    if instrument == "A"
                        && end == 13 * INTERVAL
                        && self.unavailable_a.load(Ordering::SeqCst)
                    {
                        point.result = PointValue::WarmingUp;
                        point.provenance.known_at_ms = 20 * INTERVAL;
                        point.provenance.arrived_at_ms = 20 * INTERVAL;
                    }
                    point.provenance.content_sha256 = point_sha256(&point).unwrap();
                    points.push(point);
                }
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
                    .unwrap_or(0);
                let seen = if matches!(input.result, PointValue::Ready(_)) {
                    seen + 1
                } else {
                    0
                };
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
fn data_evaluation_prior_state_availability_survives_batch_and_checkpoint_reload() {
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

#[test]
fn data_evaluation_service_recreation_preserves_causality_and_research_run_isolation() {
    let operations = Arc::new(FixtureOperations::default());
    operations.delayed_first.store(true, Ordering::SeqCst);
    operations.research_run.store(true, Ordering::SeqCst);
    let (directory, service, scope, mut pipeline) =
        super::super::data_plans::tests::fixture_service_with_state(
            Some(operations.clone()),
            StatePartition::ResearchRun,
            "backtest",
        );
    pipeline
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "derived")
        .unwrap()
        .parameters = json!({"window":1});
    let plan = data_plans::save(&service, scope, pipeline, &context("recreated-plan")).unwrap();
    let make_request = |run: &str, start, end, prior| {
        let mut r = request(&plan, run, start, end, prior);
        r.window.as_of_ms = 15 * INTERVAL;
        r
    };
    let initial_a = step(
        &service,
        make_request("persisted-a", 10, 11, None),
        &context("persisted-a-initial"),
    )
    .unwrap();
    let initial_b = step(
        &service,
        make_request("persisted-b", 10, 11, None),
        &context("persisted-b-initial"),
    )
    .unwrap();
    drop(service);
    drop(operations);

    // Fresh service, runtime, storage/registry/lease repository objects and
    // stateless fixture invoker. Only the database and verified package files
    // survive; no prior service or invoker object is reused.
    let fresh_operations = Arc::new(FixtureOperations::default());
    fresh_operations.delayed_first.store(true, Ordering::SeqCst);
    fresh_operations.research_run.store(true, Ordering::SeqCst);
    let db = directory
        .path()
        .join("service.db")
        .to_string_lossy()
        .into_owned();
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.plugin_packages = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            directory.path().join("package-store"),
        )
        .unwrap(),
    );
    runtime.plugin_operations = fresh_operations.clone();
    let reopened = TradeAssemblyService::from_runtime(&db, runtime);
    assert_eq!(
        data_plans::current(&reopened, &plan.revision_id).unwrap(),
        plan
    );
    let mut too_early = make_request(
        "persisted-a",
        11,
        12,
        Some(initial_a.checkpoint.checkpoint_sha256.clone()),
    );
    too_early.window.as_of_ms = 12 * INTERVAL;
    assert_eq!(
        step(&reopened, too_early, &context("persisted-a-early")).unwrap_err(),
        "data_evaluation_state_not_available_at_cutoff"
    );
    let resumed_a = step(
        &reopened,
        make_request(
            "persisted-a",
            11,
            12,
            Some(initial_a.checkpoint.checkpoint_sha256),
        ),
        &context("persisted-a-resume"),
    )
    .unwrap();
    let resumed_b = step(
        &reopened,
        make_request(
            "persisted-b",
            11,
            13,
            Some(initial_b.checkpoint.checkpoint_sha256),
        ),
        &context("persisted-b-resume"),
    )
    .unwrap();
    assert_eq!(
        resumed_a.checkpoint.states["derived"][RESEARCH_RUN_STATE_SLOT]["seen"],
        2
    );
    assert_eq!(
        resumed_b.checkpoint.states["derived"][RESEARCH_RUN_STATE_SLOT]["seen"],
        3
    );
    for receipt in [&resumed_a, &resumed_b] {
        assert_eq!(
            receipt.checkpoint.state_availability["derived"].known_at_ms,
            15 * INTERVAL
        );
        assert!(terminal(receipt)
            .iter()
            .all(|p| p.provenance.known_at_ms >= 15 * INTERVAL));
    }
    let a_key = key(&["persisted-a", &plan.revision_id]).unwrap();
    let stored_a = reopened
        .runtime()
        .storage
        .get_json(RUNS, &a_key)
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_a["states"]["derived"][RESEARCH_RUN_STATE_SLOT]["seen"],
        2
    );
    assert!(fresh_operations
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.node_id == "derived")
        .all(|r| !r.prior_state.is_empty() && r.window.as_of_ms >= 15 * INTERVAL));
}

#[test]
fn data_strategy_product_routes_commit_signals_and_history_then_resume_after_service_recreation() {
    use crate::data_pipeline::InstrumentContext;
    use crate::data_strategy::{Comparison, DataStrategy, Operand, Rule, Sample, SignalValue};
    let (directory, service, template, operations) = rig();
    let mut request = template.content.plan.request.clone();
    request.trade_instruments = vec![InstrumentContext {
        instrument_id: "SPY".into(),
        asset_class: "equity".into(),
        instrument_family: "equity".into(),
    }];
    let operand = |sample| Operand::Output {
        node: "derived".into(),
        port: "value".into(),
        instrument: "TEST".into(),
        sample,
    };
    let program = DataStrategy {
        version: 1,
        nodes: request.nodes.clone(),
        bindings: request.bindings.clone(),
        trade_instruments: request.trade_instruments.clone(),
        entry: Rule::Compare {
            left: operand(Sample::Current),
            op: Comparison::Greater,
            right: operand(Sample::Previous),
        },
        exit: Rule::IsTrue {
            value: Operand::Boolean { value: false },
        },
    };
    let mut spec: Value = serde_json::from_str(include_str!(
        "../../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .unwrap();
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.data_strategy":program});
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name":"Persisted data signal fixture","spec":spec}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let published=service.handle_http("POST","/product/strategies/publish",json!({"strategyId":created.body["body"]["strategy"]["id"],"expectedDraftHash":created.body["body"]["draft"]["draftHash"],"actor":{"kind":"user","id":"user.local"}}));
    assert_eq!(
        published.body["body"]["published"], true,
        "{}",
        published.body
    );
    let version = &published.body["body"]["version"];
    let scope = data_plans::PlanScope {
        strategy_id: version["strategyId"].as_str().unwrap().into(),
        strategy_version_id: version["id"].as_str().unwrap().into(),
        strategy_spec_hash: version["specHash"].as_str().unwrap().into(),
    };
    let graph=service.handle_http("POST","/capability-graph-revisions",json!({"configId":"signal-config","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper","additionalRequirements":[{"requirementId":"derived","capability":"test.derive","mode":"paper","declaration":"required","fallbackPolicy":"none"}]}));
    assert_eq!(graph.status, 200, "{}", graph.body);
    let prepared=service.handle_http("POST","/product/strategies/data-plans/prepare",json!({"scope":scope,"capabilityGraphRevisionId":graph.body["revision"]["revisionId"],"mode":"paper","idempotencyKey":"signal-prepare"}));
    assert_eq!(prepared.status, 200, "{}", prepared.body);
    let plan: data_plans::StoredDataPlan =
        serde_json::from_value(prepared.body["plan"].clone()).unwrap();
    let command = |run: &str, start, end, prior| json!({"step":request_for_signal(&plan,run,start,end,prior),"idempotencyKey":format!("{run}:{start}")});
    let body = command("signal-a", 10, 16, None);
    let initial = service.handle_http("POST", "/product/strategies/data-runs/step", body.clone());
    assert_eq!(initial.status, 200, "{}", initial.body);
    let initial: EvaluationReceipt =
        serde_json::from_value(initial.body["receipt"].clone()).unwrap();
    assert_eq!(
        initial.signals.last().unwrap().entry,
        SignalValue::Ready { value: true }
    );
    assert_eq!(
        initial.signals.last().unwrap().trade_instruments[0].instrument_id,
        "SPY"
    );
    assert!(!initial.checkpoint.last_frame.is_empty());
    let retry_body = body.clone();
    let duplicate = service.handle_http("POST", "/product/strategies/data-runs/step", body);
    let duplicate: EvaluationReceipt =
        serde_json::from_value(duplicate.body["receipt"].clone()).unwrap();
    assert_eq!(duplicate, initial);
    drop(service);
    drop(operations);
    let db = directory
        .path()
        .join("service.db")
        .to_string_lossy()
        .into_owned();
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.plugin_packages = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            directory.path().join("package-store"),
        )
        .unwrap(),
    );
    runtime.plugin_operations = Arc::new(FixtureOperations::default());
    let reopened = TradeAssemblyService::from_runtime(&db, runtime);
    let resumed = reopened.handle_http(
        "POST",
        "/product/strategies/data-runs/step",
        command(
            "signal-a",
            16,
            17,
            Some(initial.checkpoint.checkpoint_sha256),
        ),
    );
    assert_eq!(resumed.status, 200, "{}", resumed.body);
    let resumed: EvaluationReceipt =
        serde_json::from_value(resumed.body["receipt"].clone()).unwrap();
    assert_eq!(resumed.signals[0].entry, SignalValue::Ready { value: true });
    let independent = reopened.handle_http(
        "POST",
        "/product/strategies/data-runs/step",
        command("signal-b", 10, 11, None),
    );
    assert_eq!(independent.status, 200, "{}", independent.body);
    let independent: EvaluationReceipt =
        serde_json::from_value(independent.body["receipt"].clone()).unwrap();
    assert_eq!(independent.signals[0].entry, SignalValue::WarmingUp);
    let mcp_body = command("signal-mcp", 10, 11, None);
    let mcp = reopened.call_mcp_tool("tradeassembly.strategy.data_run.step", mcp_body.clone());
    assert_eq!(mcp["isError"], false, "{mcp}");
    let mut instance = reopened
        .runtime()
        .plugins
        .get_instance("instance.derive")
        .unwrap()
        .unwrap();
    instance["enabled"] = json!(false);
    reopened
        .runtime()
        .plugins
        .put_instance(
            "instance.derive",
            instance,
            &context("revoke-signal-plugin"),
        )
        .unwrap();
    let mcp_revoked = reopened.call_mcp_tool("tradeassembly.strategy.data_run.step", mcp_body);
    assert_eq!(mcp_revoked["isError"], true, "{mcp_revoked}");
    // A previously completed transport command must not bypass current package,
    // instance or ownership checks when returning its cached receipt.
    assert_eq!(
        reopened
            .handle_http("POST", "/product/strategies/data-runs/step", retry_body)
            .status,
        409
    );
    assert!(reopened
        .runtime()
        .storage
        .list_json("plugin_broker_dispatch_claims")
        .unwrap()
        .is_empty());
}
fn request_for_signal(
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

#[test]
fn data_evaluation_v3_unavailable_resets_only_affected_instrument_across_service_recreation() {
    let operations = Arc::new(FixtureOperations::default());
    operations.unavailable_a.store(true, Ordering::SeqCst);
    let (directory, service, scope, mut pipeline) =
        super::super::data_plans::tests::fixture_service_with_history(
            Some(operations.clone()),
            StatePartition::Instrument,
            "paper",
            true,
        );
    for node in &mut pipeline.nodes {
        let mut a = node.context.instruments[0].clone();
        a.instrument_id = "A".into();
        let mut b = a.clone();
        b.instrument_id = "B".into();
        node.context.instruments = vec![a, b];
        if node.graph_node_id == "derived" {
            node.parameters = json!({"window":2});
        }
    }
    let plan = data_plans::save(&service, scope, pipeline, &context("v3-plan")).unwrap();
    let make = |start, end, prior| {
        let mut r = request(&plan, "isolated", start, end, prior);
        r.window.as_of_ms = 20 * INTERVAL;
        r
    };
    operations.unavailable_a.store(false, Ordering::SeqCst);
    let mut baseline_request = make(10, 15, None);
    baseline_request.run_id = "baseline".into();
    let baseline = step(&service, baseline_request, &context("v3-baseline")).unwrap();
    operations.unavailable_a.store(true, Ordering::SeqCst);
    let initial = step(&service, make(10, 12, None), &context("v3-initial")).unwrap();
    assert_eq!(initial.checkpoint.states["derived"]["A"]["seen"], 2);
    assert_eq!(initial.checkpoint.states["derived"]["B"]["seen"], 2);
    drop(service);
    let db = directory
        .path()
        .join("service.db")
        .to_string_lossy()
        .into_owned();
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.plugin_packages = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            directory.path().join("package-store"),
        )
        .unwrap(),
    );
    runtime.plugin_operations = operations.clone();
    let reopened = TradeAssemblyService::from_runtime(&db, runtime);
    // A fails readiness first; B then fails invocation. No partial reset or source ledger may commit.
    let saved_before = reopened.runtime().storage.list_json(RUNS).unwrap();
    operations.fail_b.store(true, Ordering::SeqCst);
    assert_eq!(
        step(
            &reopened,
            make(12, 13, Some(initial.checkpoint.checkpoint_sha256.clone())),
            &context("v3-partition-failure")
        )
        .unwrap_err(),
        "fixture_partition_b_failed"
    );
    operations.fail_b.store(false, Ordering::SeqCst);
    assert_eq!(
        reopened.runtime().storage.list_json(RUNS).unwrap(),
        saved_before
    );
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_v3 BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='data_evaluation_checkpoints_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(step(
        &reopened,
        make(12, 13, Some(initial.checkpoint.checkpoint_sha256.clone())),
        &context("v3-commit-failure")
    )
    .is_err());
    assert_eq!(
        reopened.runtime().storage.list_json(RUNS).unwrap(),
        saved_before
    );
    connection.execute_batch("DROP TRIGGER fail_v3;").unwrap();
    let reset = step(
        &reopened,
        make(12, 13, Some(initial.checkpoint.checkpoint_sha256)),
        &context("v3-reset"),
    )
    .unwrap();
    assert_eq!(reset.checkpoint.states["derived"]["A"]["seen"], 0);
    assert_eq!(reset.checkpoint.states["derived"]["B"]["seen"], 3);
    let points = terminal(&reset);
    assert!(matches!(points[0].result, PointValue::WarmingUp));
    assert!(matches!(points[1].result, PointValue::Ready(_)));
    assert_eq!(points[0].provenance.known_at_ms, 20 * INTERVAL);
    assert_eq!(points[1].provenance.known_at_ms, 13 * INTERVAL);
    assert!(reset.checkpoint.partition_resets["derived"].contains_key("A"));
    assert!(!reset.checkpoint.partition_resets["derived"].contains_key("B"));
    drop(reopened);
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.plugin_packages = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            directory.path().join("package-store"),
        )
        .unwrap(),
    );
    runtime.plugin_operations = operations.clone();
    let reopened = TradeAssemblyService::from_runtime(&db, runtime);
    let resumed = step(
        &reopened,
        make(13, 15, Some(reset.checkpoint.checkpoint_sha256.clone())),
        &context("v3-resume"),
    )
    .unwrap();
    assert_eq!(resumed.checkpoint.states["derived"]["A"]["seen"], 2);
    assert_eq!(resumed.checkpoint.states["derived"]["B"]["seen"], 5);
    let points = terminal(&resumed);
    assert!(matches!(points[0].result, PointValue::WarmingUp));
    assert!(matches!(points[1].result, PointValue::Ready(_)));
    assert!(matches!(points[2].result, PointValue::Ready(_)));
    assert_eq!(points[1].provenance.known_at_ms, 14 * INTERVAL);
    assert_eq!(points[2].provenance.known_at_ms, 20 * INTERVAL);
    let expected_b: Vec<_> = terminal(&baseline)
        .into_iter()
        .filter(|p| p.instrument_id == "B" && p.provenance.event_end_ms >= 13 * INTERVAL)
        .collect();
    let actual_b: Vec<_> = terminal(&reset)
        .into_iter()
        .chain(terminal(&resumed))
        .filter(|p| p.instrument_id == "B")
        .collect();
    assert_eq!(
        actual_b, expected_b,
        "unrelated partition including provenance must match uninterrupted execution"
    );
    let captured = operations.requests.lock().unwrap();
    let source_requests: Vec<_> = captured.iter().filter(|r| r.node_id != "derived").collect();
    assert!(!source_requests.is_empty());
    assert!(
        source_requests.iter().all(|r| r.instruments.len() == 1),
        "instrument-independent sources must receive singleton requests too"
    );
    assert!(source_requests.iter().any(|r| r.instruments == ["A"]));
    assert!(source_requests.iter().any(|r| r.instruments == ["B"]));
}

#[test]
fn data_evaluation_receipt_origin_allows_owned_transport_reads_and_preserves_legacy() {
    let (_directory, service, plan, operations) = rig();
    let input = request(&plan, "origin-read", 10, 11, None);
    let origin = context("origin-step");
    let receipt = step(&service, input.clone(), &origin).unwrap();
    assert_eq!(receipt.request_authority, Some(origin.authority.clone()));
    let calls = operations.requests.lock().unwrap().len();
    let mut mcp = context("downstream-read");
    mcp.authority.surface = "mcp".into();
    assert_eq!(
        admitted_signal(&service, &input, "origin-step", &mcp)
            .unwrap()
            .1,
        receipt
    );
    assert_eq!(
        operations.requests.lock().unwrap().len(),
        calls,
        "reading an owned receipt does not invoke a provider"
    );
    let mut legacy = receipt;
    legacy.request_authority = None;
    legacy.receipt_sha256 = receipt_hash(&legacy).unwrap();
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert!(encoded.get("requestAuthority").is_none());
    let record_key = key(&[
        &key(&[&input.run_id, &input.plan_revision_id]).unwrap(),
        "origin-step",
    ])
    .unwrap();
    service
        .runtime()
        .storage
        .put_json(RECEIPTS, &record_key, encoded.clone(), &origin)
        .unwrap();
    assert_eq!(
        serde_json::to_value(
            admitted_signal(&service, &input, "origin-step", &origin)
                .unwrap()
                .1
        )
        .unwrap(),
        encoded
    );
    assert!(
        admitted_signal(&service, &input, "origin-step", &mcp).is_err(),
        "legacy receipts retain strict original-context verification"
    );
    assert_eq!(step(&service, input, &origin).unwrap(), legacy);
    assert_eq!(operations.requests.lock().unwrap().len(), calls);
}

struct ChangeAfterFinalFrame {
    inner: Arc<FixtureOperations>,
    change: Box<dyn Fn() + Send + Sync>,
    corrupt_value: bool,
}
impl VersionedPort for ChangeAfterFinalFrame {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginOperationPort for ChangeAfterFinalFrame {
    fn invoke(
        &self,
        call: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        let mut response = self.inner.invoke(call, context)?;
        let payload: DataEvaluationRequest = serde_json::from_value(call.input.clone()).unwrap();
        if call.operation_id == "derive" && payload.window.event_end_ms == 17 * INTERVAL {
            (self.change)();
            if self.corrupt_value {
                response.payload["state"]["TEST"]["seen"] = json!("invalid-after-warm-cache");
            }
        }
        Ok(response)
    }
}

#[test]
fn data_evaluation_step_schema_reuse_cannot_commit_changed_artifacts_or_authority() {
    for case in [
        "schema",
        "executable",
        "configuration",
        "revocation",
        "invalid-state-value",
    ] {
        let (_dir, service, plan, operations) = rig();
        let runtime = service.runtime();
        let package = runtime
            .plugin_packages
            .get(&plan.content.package_sha256["derived"])
            .unwrap()
            .unwrap();
        let schema = package.install_root.join("state.json");
        let executable = package.executable_path;
        let target_runtime = runtime.clone();
        let change = Box::new(move || match case {
            "schema" => std::fs::write(&schema, b"{}").unwrap(),
            "executable" => std::fs::write(&executable, b"changed-after-last-dispatch").unwrap(),
            "configuration" | "revocation" => {
                let mut instance = target_runtime
                    .plugins
                    .get_instance("instance.derive")
                    .unwrap()
                    .unwrap();
                if case == "configuration" {
                    instance["configuration"] = json!({"changed":true});
                } else {
                    instance["enabled"] = json!(false);
                }
                target_runtime
                    .plugins
                    .put_instance("instance.derive", instance, &context("change-last-frame"))
                    .unwrap();
            }
            _ => {}
        });
        let mut modified = (*runtime).clone();
        modified.plugin_operations = Arc::new(ChangeAfterFinalFrame {
            inner: operations.clone(),
            change,
            corrupt_value: case == "invalid-state-value",
        });
        let changed = TradeAssemblyService::from_runtime(service.db(), modified);
        let changed = request_service(&changed);
        let request = request(&plan, case, 10, 17, None);
        let verified = request_plan(&changed, &request).unwrap();
        remember_request_plan(&changed, &request, &verified).unwrap();
        let error = step(&changed, request, &context(case)).unwrap_err();
        let expected = match case {
            "schema" | "executable" => "data_plan_graph_stale",
            "configuration" => "data_plan_stale",
            "revocation" => "data_plan_graph_stale",
            _ => "data_evaluation_value_schema_invalid",
        };
        assert_eq!(error, expected, "{case}");
        assert_eq!(
            operations
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.node_id == "derived")
                .count(),
            7,
            "all frames must reach the final dispatch: {case}"
        );
        for namespace in [RUNS, SOURCES, RECEIPTS] {
            assert!(
                runtime.storage.list_json(namespace).unwrap().is_empty(),
                "{case} {namespace}"
            );
        }
    }
}

#[test]
fn request_plan_handoff_is_local_and_rechecks_before_dispatch() {
    let (_dir, service, plan, operations) = rig();
    let request = request(&plan, "request-handoff", 10, 11, None);
    let scoped = request_service(&service);
    let verified = request_plan(&scoped, &request).unwrap();
    remember_request_plan(&scoped, &request, &verified).unwrap();
    assert!(service.data_step_request.is_none());
    assert!(request_service(&service)
        .data_step_request
        .unwrap()
        .verified
        .lock()
        .unwrap()
        .is_none());
    assert_eq!(request_plan(&scoped, &request).unwrap(), verified);
    let mut instance = service
        .runtime()
        .plugins
        .get_instance("instance.derive")
        .unwrap()
        .unwrap();
    instance["enabled"] = json!(false);
    service
        .runtime()
        .plugins
        .put_instance(
            "instance.derive",
            instance,
            &context("revoke-after-preflight"),
        )
        .unwrap();
    // The immutable handoff is reusable, but is never dispatch authorization.
    let error = step(&scoped, request, &context("request-handoff")).unwrap_err();
    assert_eq!(error, "data_plan_graph_stale");
    assert!(operations.requests.lock().unwrap().is_empty());
    for namespace in [RUNS, SOURCES, RECEIPTS] {
        assert!(service
            .runtime()
            .storage
            .list_json(namespace)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn public_request_budget_is_not_restarted_by_evaluation() {
    let (_dir, service, plan, operations) = rig();
    let mut scoped = request_service(&service);
    Arc::get_mut(scoped.data_step_request.as_mut().unwrap())
        .unwrap()
        .started = std::time::Instant::now() - MAX_STEP_DURATION;
    let error = step(
        &scoped,
        request(&plan, "expired-public-budget", 10, 11, None),
        &context("expired-public-budget"),
    )
    .unwrap_err();
    assert_eq!(error, "data_evaluation_deadline_exceeded");
    assert!(operations.requests.lock().unwrap().is_empty());
    for namespace in [RUNS, SOURCES, RECEIPTS, FENCES] {
        assert!(service
            .runtime()
            .storage
            .list_json(namespace)
            .unwrap()
            .is_empty());
    }
}
