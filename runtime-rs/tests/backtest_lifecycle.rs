use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tradeassembly_runtime::backtest_contracts::*;
use tradeassembly_runtime::historical_data::*;
use tradeassembly_runtime::ports::{
    AuthorityContext, ClockPort, IdempotencyKey, PortDescriptor, PortKind, SideEffectContext,
    VersionedPort,
};
use tradeassembly_runtime::service::{backtest_lifecycle, TradeAssemblyService};

fn config(snapshot: &DatasetSnapshot, spec_hash: &str) -> BacktestConfiguration {
    BacktestConfiguration {
        schema: BACKTEST_CONFIGURATION_SCHEMA.to_string(),
        strategy: StrategyVersionBinding {
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: "version-1".to_string(),
            strategy_spec_hash: spec_hash.to_string(),
        },
        dataset: DatasetBinding {
            dataset_id: snapshot.dataset_id.clone(),
            snapshot_ref: snapshot.snapshot_ref.clone(),
            content_hash: snapshot.content_hash.clone(),
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            source_plugin_ref: "tradeassembly.local-data".to_string(),
            source_plugin_instance_ref: "local-data".to_string(),
            source_operation_id: "marketdata.bars.read_v1".to_string(),
            source_manifest_fingerprint: "sha256:plugin".to_string(),
            source_class: "test".to_string(),
            capability_graph_revision_id: "graph-1".to_string(),
            capability_graph_fingerprint: "sha256:graph".to_string(),
            instruments: vec!["BTC/USD".to_string()],
            selected_time_slice: DatasetTimeSlice {
                start: "2026-01-01T00:00:00Z".to_string(),
                end: "2026-01-03T00:00:00Z".to_string(),
            },
            granularity: "1h".to_string(),
            calendar: "CRYPTO_24X7".to_string(),
            timezone: "UTC".to_string(),
            corporate_action_policy: snapshot
                .content
                .normalization_policy
                .price_adjustment
                .clone(),
            benchmark: None,
        },
        capability_graph: CapabilityGraphBinding {
            revision_id: String::new(),
            graph_fingerprint: String::new(),
        },
        capital: CapitalConfiguration {
            starting_cash_micros: 1_000_000,
            reporting_currency: "USD".to_string(),
            account_model: AccountModel::Cash,
            maximum_leverage_micros: 1_000_000,
            margin_policy: "none".to_string(),
        },
        execution: ExecutionConfiguration {
            evaluation_timing: EvaluationTiming::BarClose,
            fill_timing: FillTiming::NextBar,
            fill_price_source: PriceSource::Open,
            order_type: BacktestOrderType::Market,
            time_in_force: "day".to_string(),
            fill_policy: FillPolicy::Full,
            reject_on_insufficient_capital: true,
            reject_on_insufficient_liquidity: true,
            stale_data_policy: MissingDataPolicy::Reject,
            missing_data_policy: MissingDataPolicy::Reject,
        },
        costs: CostConfiguration {
            fixed_fee_micros: 0,
            per_unit_fee_micros: 0,
            notional_fee_bps: 0,
            spread_bps: 0,
            slippage_bps: 0,
        },
        liquidity: LiquidityConfiguration {
            maximum_participation_bps: 1_000,
            minimum_volume_micros: 1,
        },
        instruments: vec![InstrumentConfiguration {
            instrument_id: "BTC/USD".to_string(),
            instrument_family: "crypto_spot".to_string(),
            quantity_unit: "base_asset".to_string(),
            quantity_scale: 6,
            price_scale: 6,
            contract_multiplier_micros: 1_000_000,
            settlement: "cash".to_string(),
            metadata: BTreeMap::new(),
        }],
        risk: BacktestRiskConfiguration {
            maximum_position_notional_micros: 1_000_000,
            maximum_order_quantity_micros: 1_000_000,
            maximum_loss_micros: 1_000_000,
            maximum_open_positions: 1,
        },
        end_of_data: EndOfDataPolicy::MarkToMarket,
        evaluator_version: "kernel-1".to_string(),
        compiler_version: "compiler-1".to_string(),
        plugin_fingerprints: BTreeMap::new(),
        variable_overrides: BTreeMap::new(),
    }
}

fn snapshot() -> DatasetSnapshot {
    DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: Some("version-1".to_string()),
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: "local-data".to_string(),
                plugin_ref: "tradeassembly.local-data".to_string(),
                operation_id: "marketdata.bars.read_v1".to_string(),
                plugin_manifest_fingerprint: "sha256:plugin".to_string(),
                capability_graph_revision_id: "graph-1".to_string(),
                capability_graph_fingerprint: "sha256:graph".to_string(),
                source_request_hash: "sha256:request".to_string(),
                upstream_refs: BTreeMap::new(),
            },
            asset_classes: vec![AssetClass::CryptoSpot],
            instruments: vec!["BTC/USD".to_string()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1h".to_string(),
            time_slice: DatasetTimeSlice {
                start: "2026-01-01T00:00:00Z".to_string(),
                end: "2026-01-03T00:00:00Z".to_string(),
            },
            calendar: "CRYPTO_24X7".to_string(),
            timezone: "UTC".to_string(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".to_string(),
                timezone: "UTC".to_string(),
                duplicate_policy: "reject".to_string(),
                price_adjustment: "none".to_string(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: QualityDisposition::Reject,
                stale_observations: QualityDisposition::Reject,
                outliers: QualityDisposition::Allow,
                invalid_markets: QualityDisposition::Reject,
                calendar_mismatch: QualityDisposition::Reject,
                corporate_action_gaps: QualityDisposition::Allow,
                missing_derivative_fields: QualityDisposition::Allow,
            },
            quality_findings: vec![],
            observations: vec![HistoricalObservation {
                instrument_id: "BTC/USD".to_string(),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                data: HistoricalObservationData::Bar(BarObservation {
                    open: 1.0,
                    high: 1.0,
                    low: 1.0,
                    close: 1.0,
                    volume: 1.0,
                }),
            }],
        },
        1,
    )
    .expect("snapshot")
}

fn prepare(service: &TradeAssemblyService, key: &str) -> Value {
    let mut spec: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/strategy-spec/v3/valid/static-equity.json"),
        )
        .expect("strategy fixture"),
    )
    .expect("strategy JSON");
    spec["stages"]["evaluate"]["substeps"][0]["config"] = json!({
        "tradeassembly.expression": "bar.close > 0",
        "tradeassembly.quantity": 1_000_000
    });
    prepare_with_spec(service, key, spec)
}

fn prepare_with_spec(service: &TradeAssemblyService, key: &str, spec: Value) -> Value {
    let snapshot = snapshot();
    let spec_hash = canonical_hash(&spec, "strategy hash").expect("strategy hash");
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context(key))
        .expect("snapshot stored");
    service.runtime().storage.put_json("strategy_versions", "strategy-1:1", json!({"id":"version-1","strategyId":"strategy-1","specHash":spec_hash,"spec":spec,"published":true}), &context(key)).expect("version stored");
    json!({"idempotencyKey":key,"client":"test","purpose":"research","configuration":config(&snapshot, &spec_hash)})
}

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("key"),
    )
}
fn service_path() -> (NamedTempFile, String) {
    let file = NamedTempFile::new().expect("db");
    let path = file.path().to_string_lossy().to_string();
    (file, path)
}
fn result(manifest: &BacktestRunManifest) -> BacktestResult {
    result_at(manifest, 2)
}

fn result_at(manifest: &BacktestRunManifest, created_at_ms: i64) -> BacktestResult {
    BacktestResult::build(
        BacktestResultContent {
            schema: BACKTEST_RESULT_SCHEMA.to_string(),
            run_id: manifest.content.run_id.clone(),
            manifest_hash: manifest.manifest_hash.clone(),
            evaluator_version: manifest.content.configuration.evaluator_version.clone(),
            compiler_version: manifest.content.configuration.compiler_version.clone(),
            decisions: vec![],
            orders: vec![],
            fills: vec![],
            positions: vec![],
            ledger: vec![],
            accounting: json!({
                "fills": [],
                "ledger": [],
                "positions": {},
                "endingCashMicros": 1_000_000,
                "marketValueMicros": 0,
                "endingEquityMicros": 1_000_000,
                "realizedPnlMicros": 0,
                "unrealizedPnlMicros": 0,
                "incomeMicros": 0,
                "costsMicros": 0,
                "reconciliationResidualMicros": 0
            }),
            diagnostics: vec![],
        },
        created_at_ms,
    )
    .expect("real result")
}

struct TestClock {
    now_ms: AtomicI64,
}

impl TestClock {
    fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
        }
    }

    fn advance(&self, delta_ms: i64) {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl VersionedPort for TestClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![PortDescriptor::new(PortKind::Clock, "test.clock")]
    }
}

impl ClockPort for TestClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

fn service_with_clock(path: &str, now_ms: i64) -> (TradeAssemblyService, Arc<TestClock>) {
    let base = TradeAssemblyService::test_local(path);
    let runtime = base.runtime();
    let mut runtime = (*runtime).clone();
    let clock = Arc::new(TestClock::new(now_ms));
    runtime.clock = clock.clone();
    (TradeAssemblyService::from_runtime(path, runtime), clock)
}

#[test]
fn create_is_fsm_driven_and_idempotency_is_request_equivalent() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path);
    let body = prepare(&service, "create-1");
    let created = backtest_lifecycle::create(&service, body.clone());
    assert_eq!(created.status, 202, "{:#}", created.body);
    assert_eq!(created.body["status"], "queued");
    assert_eq!(created.body["sequence"], 2);
    let duplicate = backtest_lifecycle::create(&service, body.clone());
    assert_eq!(duplicate.status, 201);
    assert_eq!(duplicate.body["runId"], created.body["runId"]);
    let mut conflict = body;
    conflict["configuration"]["costs"]["slippageBps"] = json!(1);
    assert_eq!(backtest_lifecycle::create(&service, conflict).status, 409);
}

#[test]
fn restart_redelivery_failure_retry_and_real_result_completion() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path);
    let created = backtest_lifecycle::create(&service, prepare(&service, "retry-1"));
    let run_id = created.body["runId"]
        .as_str()
        .unwrap_or_else(|| panic!("run: {:#}", created.body))
        .to_string();
    drop(service);
    let restarted = TradeAssemblyService::test_local(&path);
    let failed = backtest_lifecycle::process_with(&restarted, "worker-a", &|_, _| {
        Err("backtest_data_insufficient".to_string())
    });
    assert_eq!(failed.body["status"], "failed");
    let retried = backtest_lifecycle::retry(
        &restarted,
        &run_id,
        json!({"idempotencyKey":"retry-command"}),
    );
    assert_eq!(retried.status, 202);
    assert_eq!(retried.body["status"], "queued");
    assert_eq!(retried.body["failureCode"], Value::Null);
    let completed = backtest_lifecycle::process_with(&restarted, "worker-b", &|manifest, _| {
        Ok(Some(result(manifest)))
    });
    assert_eq!(completed.body["status"], "completed");
    let stored = restarted
        .runtime()
        .backtests
        .get_run(&run_id)
        .expect("run read")
        .expect("run");
    assert!(stored.result.is_some());
    assert_eq!(stored.failure_code, None);
    assert_eq!(stored.events.len(), 7);
    assert_eq!(
        restarted
            .runtime()
            .backtests
            .list_events(&run_id)
            .expect("events"),
        stored.events
    );
}

#[test]
fn completed_report_accepts_an_immutable_dataset_from_an_earlier_strategy_version() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path)
        .with_studio_base_url("http://127.0.0.1:3002/")
        .expect("studio origin");
    let mut request = prepare(&service, "dataset-reuse-report");
    let spec = request["configuration"]["strategy"]["strategySpecHash"]
        .as_str()
        .expect("strategy spec hash")
        .to_string();
    let mut version_spec: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/strategy-spec/v3/valid/static-equity.json"),
        )
        .expect("strategy fixture"),
    )
    .expect("strategy JSON");
    version_spec["stages"]["evaluate"]["substeps"][0]["config"] = json!({
        "tradeassembly.expression": "bar.close > 0",
        "tradeassembly.quantity": 1_000_000
    });
    service
        .runtime()
        .storage
        .put_json(
            "strategy_versions",
            "strategy-1:2",
            json!({
                "id": "version-2",
                "strategyId": "strategy-1",
                "specHash": spec,
                "spec": version_spec,
                "published": true,
            }),
            &context("dataset-reuse-version"),
        )
        .expect("newer version stored");
    request["configuration"]["strategy"]["strategyVersionId"] = json!("version-2");

    let expected_spec_hash = request["configuration"]["strategy"]["strategySpecHash"].clone();
    let created = backtest_lifecycle::create(&service, request);
    assert_eq!(created.status, 202, "{:#}", created.body);
    let completed = backtest_lifecycle::process(&service, "worker-a");
    assert_eq!(completed.status, 200, "{:#}", completed.body);
    assert_eq!(completed.body["status"], "completed", "{}", completed.body);
    assert_eq!(
        completed.body["report"]["run"]["strategyVersionId"],
        "version-2"
    );
    assert_eq!(
        completed.body["report"]["provenance"]["strategyVersionId"],
        "version-2"
    );
    assert_eq!(
        completed.body["report"]["provenance"]["strategySpecHash"],
        expected_spec_hash
    );
    assert_eq!(
        completed.body["report"]["provenance"]["sourceClass"],
        "test"
    );
    assert_eq!(
        completed.body["report"]["provenance"]["sourcePluginInstanceRef"],
        "local-data"
    );
    assert_eq!(
        completed.body["report"]["provenance"]["sourceOperationId"],
        "marketdata.bars.read_v1"
    );
    assert_eq!(completed.body["report"]["provenance"]["granularity"], "1h");
    assert_eq!(
        completed.body["report"]["provenance"]["strategyExecutionSummary"]["status"],
        "verified"
    );
    let run_id = created.body["runId"].as_str().expect("created run ID");
    let read = backtest_lifecycle::get(&service, run_id);
    assert_eq!(read.status, 200, "{}", read.body);
    let navigation = read.body["navigation"].clone();
    assert_eq!(
        navigation["href"],
        format!(
            "http://127.0.0.1:3002/app/strategies/strategy-1/research?view=backtest&runId={run_id}"
        )
    );
    assert_eq!(
        backtest_lifecycle::report(&service, run_id).body["navigation"],
        navigation
    );
    assert_eq!(
        backtest_lifecycle::export(&service, run_id, "reportJson").body["navigation"],
        navigation
    );
    let replayed = backtest_lifecycle::replay(&service, run_id);
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    assert_eq!(replayed.body["navigation"], navigation);
}

#[test]
fn cancellation_wins_over_a_late_worker_result() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path);
    let created = backtest_lifecycle::create(&service, prepare(&service, "cancel-1"));
    let run_id = created.body["runId"]
        .as_str()
        .unwrap_or_else(|| panic!("run: {:#}", created.body))
        .to_string();
    let outcome = backtest_lifecycle::process_with(&service, "worker-c", &|manifest, _| {
        let canceled = backtest_lifecycle::cancel(
            &service,
            &manifest.content.run_id,
            json!({"idempotencyKey":"cancel-command"}),
        );
        assert_eq!(canceled.body["status"], "canceled");
        Ok(Some(result(manifest)))
    });
    assert_eq!(outcome.status, 409);
    assert_eq!(outcome.body["detail"], "backtest_worker_lease_lost");
    assert_eq!(
        backtest_lifecycle::get(&service, &run_id).body["status"],
        "canceled"
    );
    let stored = service
        .runtime()
        .backtests
        .get_run(&run_id)
        .expect("run read")
        .expect("run");
    assert!(stored.result.is_none());
    assert!(service
        .runtime()
        .backtests
        .get_result(&run_id)
        .expect("result read")
        .is_none());
}

#[test]
fn unsupported_strategy_semantics_are_rejected_before_manifest_or_queue_creation() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path);
    prepare(&service, "unsupported-1");
    let version = service
        .runtime()
        .storage
        .get_json("strategy_versions", "strategy-1:1")
        .expect("version read")
        .expect("version");
    let mut spec = version["spec"].clone();
    spec["stages"]["order_strategy"]["policy"]["allowed_order_types"] = json!(["limit"]);
    let body = prepare_with_spec(&service, "unsupported-1", spec);

    let response = backtest_lifecycle::create(&service, body);

    assert_eq!(response.status, 400, "{:#}", response.body);
    assert_eq!(
        response.body["detail"],
        "backtest_strategy_semantics_unsupported"
    );
    assert!(service
        .runtime()
        .backtests
        .list_runs()
        .expect("runs")
        .is_empty());
    assert!(service
        .runtime()
        .storage
        .list_json("backtest_manifests_v1")
        .expect("manifests")
        .is_empty());
}

#[test]
fn conditional_order_semantics_are_rejected_before_manifest_or_queue_creation() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path);
    prepare(&service, "unsupported-conditional-1");
    let version = service
        .runtime()
        .storage
        .get_json("strategy_versions", "strategy-1:1")
        .expect("version read")
        .expect("version");
    let mut spec = version["spec"].clone();
    spec["stages"]["order_strategy"]["policy"]["conditional_orders_allowed"] = json!(true);
    let body = prepare_with_spec(&service, "unsupported-conditional-1", spec);

    let response = backtest_lifecycle::create(&service, body);

    assert_eq!(response.status, 400, "{:#}", response.body);
    assert_eq!(
        response.body["detail"],
        "backtest_strategy_semantics_unsupported"
    );
    assert!(service
        .runtime()
        .backtests
        .list_runs()
        .expect("runs")
        .is_empty());
    assert!(service
        .runtime()
        .storage
        .list_json("backtest_manifests_v1")
        .expect("manifests")
        .is_empty());
}

#[test]
fn a_redelivered_higher_fence_prevents_the_stale_worker_from_committing() {
    let (_file, path) = service_path();
    let (service, clock) = service_with_clock(&path, 1_000);
    let created = backtest_lifecycle::create(&service, prepare(&service, "fence-1"));
    let run_id = created.body["runId"].as_str().expect("run id").to_string();

    let stale = backtest_lifecycle::process_with(&service, "worker-a", &|manifest, _| {
        clock.advance(30_001);
        let current = backtest_lifecycle::process_with(&service, "worker-b", &|manifest, _| {
            Ok(Some(result_at(manifest, 2)))
        });
        assert_eq!(current.status, 200, "{:#}", current.body);
        Ok(Some(result_at(manifest, 3)))
    });

    assert_eq!(stale.status, 409, "{:#}", stale.body);
    assert_eq!(stale.body["detail"], "backtest_worker_lease_lost");
    let run = service
        .runtime()
        .backtests
        .get_run(&run_id)
        .expect("run read")
        .expect("run");
    assert_eq!(
        run.state,
        tradeassembly_runtime::domain::BacktestRunState::Completed
    );
    assert_eq!(run.result.expect("result").created_at_ms, 2);
    let attempt = service
        .runtime()
        .backtests
        .get_attempt(&run.current_attempt_id)
        .expect("attempt read")
        .expect("attempt");
    assert_eq!(attempt.fencing_token, 2);
}

#[test]
fn immutable_result_write_conflict_cannot_mark_the_run_completed() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path);
    let created = backtest_lifecycle::create(&service, prepare(&service, "result-conflict-1"));
    let run_id = created.body["runId"].as_str().expect("run id").to_string();
    service
        .runtime()
        .storage
        .put_json(
            "backtest_results_v1",
            &run_id,
            json!({"conflicting": true}),
            &context("poison-result"),
        )
        .expect("conflicting result stored");

    let response = backtest_lifecycle::process_with(&service, "worker-a", &|manifest, _| {
        Ok(Some(result(manifest)))
    });

    assert_eq!(response.status, 409, "{:#}", response.body);
    assert_eq!(response.body["detail"], "backtest_result_write_conflict");
    let run = service
        .runtime()
        .backtests
        .get_run(&run_id)
        .expect("run read")
        .expect("run");
    assert_eq!(
        run.state,
        tradeassembly_runtime::domain::BacktestRunState::Failed
    );
    assert!(run.result.is_none());
    assert_eq!(
        run.failure_code.as_deref(),
        Some("backtest_result_write_conflict")
    );
}

#[test]
fn tampered_embedded_result_is_rejected_on_run_reads() {
    let (_file, path) = service_path();
    let service = TradeAssemblyService::test_local(&path);
    let created = backtest_lifecycle::create(&service, prepare(&service, "tamper-run-1"));
    let run_id = created.body["runId"].as_str().expect("run id").to_string();
    let completed = backtest_lifecycle::process_with(&service, "worker-a", &|manifest, _| {
        Ok(Some(result(manifest)))
    });
    assert_eq!(completed.status, 200, "{:#}", completed.body);
    let mut stored = service
        .runtime()
        .storage
        .get_json("backtest_runs_v2", &run_id)
        .expect("raw run read")
        .expect("raw run");
    stored["result"]["content"]["accounting"]["startingCashMicros"] = json!(999);
    service
        .runtime()
        .storage
        .put_json("backtest_runs_v2", &run_id, stored, &context("tamper-run"))
        .expect("tampered run stored");

    assert_eq!(
        service.runtime().backtests.get_run(&run_id),
        Err("backtest_result_integrity_failed".to_string())
    );
    let response = backtest_lifecycle::get(&service, &run_id);
    assert_eq!(response.status, 500, "{:#}", response.body);
}
