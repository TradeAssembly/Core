use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::NamedTempFile;
use tradeassembly_runtime::backtest_contracts::*;
use tradeassembly_runtime::backtest_engine::execute;
use tradeassembly_runtime::backtest_report;
use tradeassembly_runtime::domain::{BacktestRunState, RobustnessRunState};
use tradeassembly_runtime::historical_data::*;
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::research_comparison::{
    durable::{ComparisonService, CreateComparisonRequest},
    ComparisonScope, CompatibilityPolicy,
};
use tradeassembly_runtime::robustness_contracts::*;
use tradeassembly_runtime::robustness_engine::{
    MonteCarloInput, MonteCarloMode, StudyInput, ROBUSTNESS_ENGINE_VERSION,
};
use tradeassembly_runtime::robustness_projection;
use tradeassembly_runtime::service::TradeAssemblyService;

const UNIT: i64 = 1_000_000;

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("idempotency key"),
    )
}

fn strategy() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3/valid/static-equity.json");
    let mut spec: Value =
        serde_json::from_slice(&fs::read(path).expect("strategy fixture")).expect("strategy JSON");
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.expression":"bar.close > 101000000","tradeassembly.quantity":UNIT});
    spec
}

fn version(spec: Value) -> Value {
    let hash = canonical_hash(&spec, "strategy hash").expect("hash");
    json!({
        "id":"version-1",
        "strategyId":"strategy-1",
        "specHash":hash,
        "spec":spec,
        "immutable":true
    })
}

fn snapshot() -> DatasetSnapshot {
    DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: Some("version-1".to_string()),
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: "local-data-1".to_string(),
                plugin_ref: "local-data".to_string(),
                operation_id: "marketdata.bars.read_v1".to_string(),
                plugin_manifest_fingerprint: "sha256:plugin".to_string(),
                capability_graph_revision_id: "graph-1".to_string(),
                capability_graph_fingerprint: "sha256:graph".to_string(),
                source_request_hash: "sha256:request".to_string(),
                upstream_refs: BTreeMap::new(),
            },
            asset_classes: vec![AssetClass::Equity],
            instruments: vec!["SPY".to_string()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1h".to_string(),
            time_slice: DatasetTimeSlice {
                start: "2026-01-01T00:00:00Z".to_string(),
                end: "2026-01-01T03:00:00Z".to_string(),
            },
            calendar: "XNYS".to_string(),
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
            observations: vec![
                bar("2026-01-01T00:00:00Z", 100.0, 102.0),
                bar("2026-01-01T01:00:00Z", 103.0, 104.0),
                bar("2026-01-01T02:00:00Z", 99.0, 98.0),
            ],
        },
        1,
    )
    .expect("snapshot")
}

fn bar(timestamp: &str, open: f64, close: f64) -> HistoricalObservation {
    HistoricalObservation {
        instrument_id: "SPY".to_string(),
        timestamp: timestamp.to_string(),
        data: HistoricalObservationData::Bar(BarObservation {
            open,
            high: open.max(close),
            low: open.min(close),
            close,
            volume: 100.0,
        }),
    }
}

fn manifest(
    snapshot: &DatasetSnapshot,
    version: &Value,
    run_id: &str,
    currency: &str,
) -> BacktestRunManifest {
    let configuration = BacktestConfiguration {
        schema: BACKTEST_CONFIGURATION_SCHEMA.to_string(),
        strategy: StrategyVersionBinding {
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: "version-1".to_string(),
            strategy_spec_hash: version["specHash"].as_str().expect("spec hash").to_string(),
        },
        dataset: DatasetBinding {
            dataset_id: snapshot.dataset_id.clone(),
            snapshot_ref: snapshot.snapshot_ref.clone(),
            content_hash: snapshot.content_hash.clone(),
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            source_plugin_ref: "local-data".to_string(),
            source_plugin_instance_ref: "local-data-1".to_string(),
            source_operation_id: "marketdata.bars.read_v1".to_string(),
            source_manifest_fingerprint: "sha256:plugin".to_string(),
            source_class: "test".to_string(),
            capability_graph_revision_id: "graph-1".to_string(),
            capability_graph_fingerprint: "sha256:graph".to_string(),
            instruments: vec!["SPY".to_string()],
            selected_time_slice: snapshot.content.time_slice.clone(),
            granularity: "1h".to_string(),
            calendar: "XNYS".to_string(),
            timezone: "UTC".to_string(),
            corporate_action_policy: "none".to_string(),
            benchmark: None,
        },
        capability_graph: CapabilityGraphBinding {
            revision_id: "caprev-1".to_string(),
            graph_fingerprint: "sha256:capability-graph".to_string(),
        },
        capital: CapitalConfiguration {
            starting_cash_micros: 1_000 * UNIT,
            reporting_currency: currency.to_string(),
            account_model: AccountModel::Cash,
            maximum_leverage_micros: UNIT,
            margin_policy: "none".to_string(),
        },
        execution: ExecutionConfiguration {
            evaluation_timing: EvaluationTiming::BarClose,
            fill_timing: FillTiming::SameBar,
            fill_price_source: PriceSource::Close,
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
            maximum_participation_bps: 10_000,
            minimum_volume_micros: UNIT,
        },
        instruments: vec![InstrumentConfiguration {
            instrument_id: "SPY".to_string(),
            instrument_family: "equity".to_string(),
            quantity_unit: "share".to_string(),
            quantity_scale: 6,
            price_scale: 6,
            contract_multiplier_micros: UNIT,
            settlement: "cash".to_string(),
            metadata: BTreeMap::new(),
        }],
        risk: BacktestRiskConfiguration {
            maximum_position_notional_micros: 1_000 * UNIT,
            maximum_order_quantity_micros: 10 * UNIT,
            maximum_loss_micros: 1_000 * UNIT,
            maximum_open_positions: 1,
        },
        end_of_data: EndOfDataPolicy::MarkToMarket,
        evaluator_version: "kernel-1".to_string(),
        compiler_version: "compiler-1".to_string(),
        plugin_fingerprints: BTreeMap::new(),
        variable_overrides: BTreeMap::new(),
    };
    BacktestRunManifest::build(
        BacktestRunManifestContent {
            schema: BACKTEST_MANIFEST_SCHEMA.to_string(),
            run_id: run_id.to_string(),
            request_hash: format!("sha256:request-{run_id}"),
            idempotency_key: run_id.to_string(),
            configuration: configuration.clone(),
            validation: configuration.validate(),
            authority: AuthorityContext::local_cli(),
            client: "self".to_string(),
            purpose: "research".to_string(),
            evidence_refs: vec![],
            first_attempt_id: format!("{run_id}:attempt:1"),
        },
        1,
    )
    .expect("manifest")
}

fn complete_source(
    service: &TradeAssemblyService,
    snapshot: &DatasetSnapshot,
    version: &Value,
    run_id: &str,
    currency: &str,
) {
    let runtime = service.runtime();
    let manifest = manifest(snapshot, version, run_id, currency);
    let result = execute(&manifest, version, snapshot, 10).expect("result");
    let attempt_id = format!("{run_id}:attempt:1");
    let event = event(run_id, &attempt_id);
    let run = BacktestRunRecord {
        schema: BACKTEST_RUN_SCHEMA.to_string(),
        run_id: run_id.to_string(),
        manifest_hash: manifest.manifest_hash.clone(),
        request_hash: manifest.content.request_hash.clone(),
        idempotency_key: manifest.content.idempotency_key.clone(),
        state: BacktestRunState::Completed,
        sequence: 1,
        current_attempt_id: attempt_id.clone(),
        fencing_token: 1,
        result_hash: Some(result.result_hash.clone()),
        result: Some(result.clone()),
        failure_code: None,
        events: vec![event],
        created_at_ms: 1,
        updated_at_ms: 10,
    };
    let attempt = BacktestAttempt {
        schema: BACKTEST_ATTEMPT_SCHEMA.to_string(),
        attempt_id,
        run_id: run_id.to_string(),
        ordinal: 1,
        state: BacktestAttemptState::Completed,
        stage: "completed".to_string(),
        progress_bps: 10_000,
        queue_message_id: None,
        lease_owner: Some("test".to_string()),
        fencing_token: 1,
        created_at_ms: 1,
        updated_at_ms: 10,
        failure_code: None,
        diagnostics: vec![],
    };
    let write_context = context(&format!("source-{run_id}"));
    runtime
        .storage
        .put_json(
            "strategy_versions",
            "version-1",
            version.clone(),
            &write_context,
        )
        .expect("strategy version stored");
    runtime
        .backtests
        .put_manifest(&manifest, &write_context)
        .expect("manifest stored");
    runtime
        .backtests
        .put_result(&result, &write_context)
        .expect("result stored");
    runtime
        .backtests
        .create_run(&run, &write_context)
        .expect("run stored");
    runtime
        .backtests
        .put_attempt(&attempt, &write_context)
        .expect("attempt stored");
}

fn complete_robustness_source(
    service: &TradeAssemblyService,
    snapshot: &DatasetSnapshot,
    version: &Value,
    source_run_id: &str,
    robustness_run_id: &str,
) {
    let runtime = service.runtime();
    let source_manifest = runtime
        .backtests
        .get_manifest(source_run_id)
        .expect("source manifest read")
        .expect("source manifest");
    let source_result = runtime
        .backtests
        .get_result(source_run_id)
        .expect("source result read")
        .expect("source result");
    let source_run = runtime
        .backtests
        .get_run(source_run_id)
        .expect("source run read")
        .expect("source run");
    let source_report = backtest_report::project_with_strategy_spec(
        &source_run,
        &source_manifest,
        &source_result,
        snapshot,
        &runtime
            .backtests
            .list_attempts(source_run_id)
            .expect("source attempts"),
        &runtime
            .backtests
            .list_events(source_run_id)
            .expect("source events"),
        version.get("spec"),
    )
    .expect("source report");
    let assumptions = serde_json::to_value(StudyInput::MonteCarlo(MonteCarloInput {
        path_count: 32,
        confidence_level: 0.9,
        ruin_equity_ratio: 0.5,
        mode: MonteCarloMode::TradeBootstrap,
    }))
    .expect("study assumptions");
    let manifest = RobustnessRunManifest::build(
        RobustnessRunManifestContent {
            schema: ROBUSTNESS_MANIFEST_SCHEMA.to_string(),
            run_id: robustness_run_id.to_string(),
            request_hash: format!("sha256:request-{robustness_run_id}"),
            idempotency_key: robustness_run_id.to_string(),
            study_kind: RobustnessStudyKind::MonteCarlo,
            engine_version: ROBUSTNESS_ENGINE_VERSION.to_string(),
            deterministic_seed: 23,
            source: RobustnessSourceBinding {
                source_run_id: source_run_id.to_string(),
                source_manifest_hash: source_manifest.manifest_hash.clone(),
                source_result_hash: source_result.result_hash.clone(),
                source_report_hash: source_report.report_hash.clone(),
                source_dataset_id: snapshot.dataset_id.clone(),
                source_dataset_hash: snapshot.content_hash.clone(),
            },
            supporting_sources: vec![],
            assumptions,
            budget: RobustnessBudget {
                maximum_samples: 32,
                maximum_grid_points: 8,
                maximum_windows: 8,
                maximum_scenarios: 8,
                maximum_output_bytes: 100_000,
                maximum_attempts: 2,
            },
            authority: AuthorityContext::local_cli(),
            client: "self".to_string(),
            purpose: "research".to_string(),
            first_attempt_id: format!("{robustness_run_id}:attempt:1"),
        },
        20,
    )
    .expect("robustness manifest");
    let result = robustness_projection::execute(
        &manifest,
        &source_manifest,
        &source_result,
        &source_report,
        snapshot,
        version,
        21,
    )
    .expect("robustness result");
    let run = RobustnessRunRecord {
        schema: ROBUSTNESS_RUN_SCHEMA.to_string(),
        run_id: robustness_run_id.to_string(),
        manifest_hash: manifest.manifest_hash.clone(),
        request_hash: manifest.content.request_hash.clone(),
        idempotency_key: manifest.content.idempotency_key.clone(),
        state: RobustnessRunState::Completed,
        sequence: 0,
        current_attempt_id: manifest.content.first_attempt_id.clone(),
        fencing_token: 1,
        result_hash: Some(result.result_hash.clone()),
        result: Some(result.clone()),
        failure_code: None,
        events: vec![],
        created_at_ms: 20,
        updated_at_ms: 21,
    };
    let write_context = context(&format!("robustness-{robustness_run_id}"));
    runtime
        .robustness
        .put_manifest(&manifest, &write_context)
        .expect("robustness manifest stored");
    runtime
        .robustness
        .put_result(&result, &write_context)
        .expect("robustness result stored");
    runtime
        .robustness
        .create_run(&run, &write_context)
        .expect("robustness run stored");
}

fn event(run_id: &str, attempt_id: &str) -> BacktestLifecycleEvent {
    let event_hash = canonical_hash(
        &json!({
            "runId":run_id,
            "sequence":1,
            "from":BacktestRunState::Queued,
            "to":BacktestRunState::Completed,
            "attemptId":attempt_id,
            "previous":Value::Null,
            "detail":"completed"
        }),
        "event hash",
    )
    .expect("event hash");
    BacktestLifecycleEvent {
        event_id: format!("{run_id}:event:1"),
        run_id: run_id.to_string(),
        sequence: 1,
        event_type: "backtest.completed".to_string(),
        from_state: BacktestRunState::Queued,
        to_state: BacktestRunState::Completed,
        attempt_id: attempt_id.to_string(),
        occurred_at_ms: 10,
        previous_event_hash: None,
        event_hash,
        detail: "completed".to_string(),
    }
}

fn request(run_ids: Vec<&str>, key: &str) -> CreateComparisonRequest {
    CreateComparisonRequest {
        source_run_ids: run_ids.into_iter().map(str::to_string).collect(),
        scope: ComparisonScope {
            strategy_id: "strategy-1".to_string(),
            allow_cross_version: true,
            compatible_strategy_ids: vec![],
        },
        policy: CompatibilityPolicy::default(),
        idempotency_key: key.to_string(),
    }
}

#[test]
fn comparison_is_order_independent_idempotent_and_persists_exports_across_restart() {
    let file = NamedTempFile::new().expect("database");
    let path = file.path().to_string_lossy().to_string();
    let service = TradeAssemblyService::test_local(path.clone());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot"))
        .expect("snapshot stored");
    complete_source(&service, &snapshot, &version, "backtest-a", "USD");
    complete_source(&service, &snapshot, &version, "backtest-b", "USD");

    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    let first = comparisons
        .create(
            request(vec!["backtest-b", "backtest-a"], "comparison-idempotent"),
            20,
            &context("comparison-idempotent"),
        )
        .expect("comparison created");
    let second = comparisons
        .create(
            request(vec!["backtest-a", "backtest-b"], "comparison-idempotent"),
            21,
            &context("comparison-idempotent"),
        )
        .expect("idempotent retry");
    assert_eq!(first, second);
    let mut conflicting = request(vec!["backtest-a", "backtest-b"], "comparison-idempotent");
    conflicting.scope.allow_cross_version = false;
    assert_eq!(
        comparisons
            .create(conflicting, 21, &context("comparison-idempotent"))
            .expect_err("changed request must fail"),
        "comparison_request_conflict"
    );
    let artifact = comparisons
        .get_artifact(&first.comparison_id)
        .expect("artifact read")
        .expect("artifact exists");
    assert!(!artifact.artifact.blocked);
    assert_eq!(comparisons.list().expect("list"), vec![first.clone()]);
    assert!(comparisons
        .get_export(&first.comparison_id, "json")
        .expect("json read")
        .expect("json exists")
        .content
        .contains(&artifact.artifact.comparison_hash));
    assert!(comparisons
        .get_export(&first.comparison_id, "csv")
        .expect("csv read")
        .expect("csv exists")
        .content
        .starts_with("run_hash,metric,unit,value,unavailable_reason\n"));

    let restarted = TradeAssemblyService::test_local(path);
    let runtime = restarted.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    assert_eq!(
        comparisons.get(&first.comparison_id).expect("restart read"),
        Some(first)
    );
}

#[test]
fn rejected_scope_binding_leaves_no_comparison_residue() {
    let file = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(file.path().to_string_lossy());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot-scope-rejection"))
        .expect("snapshot stored");
    complete_source(&service, &snapshot, &version, "backtest-a", "USD");
    complete_source(&service, &snapshot, &version, "backtest-b", "USD");

    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    assert_eq!(
        comparisons
            .create_with_prewrite(
                request(
                    vec!["backtest-a", "backtest-b"],
                    "comparison-scope-rejected"
                ),
                20,
                &context("comparison-scope-rejected"),
                |_| Err("object_not_available".to_string()),
            )
            .expect_err("scope rejection"),
        "object_not_available"
    );
    for namespace in [
        "research_comparison_manifests_v1",
        "research_comparison_artifacts_v1",
        "research_comparison_exports_v1",
        "research_comparison_idempotency_v1",
    ] {
        assert!(
            runtime
                .storage
                .list_json(namespace)
                .expect("comparison namespace")
                .is_empty(),
            "{namespace} must remain empty"
        );
    }
}

#[test]
fn incompatible_currency_persists_an_explicit_blocked_comparison() {
    let file = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(file.path().to_string_lossy());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot-currency"))
        .expect("snapshot stored");
    complete_source(&service, &snapshot, &version, "backtest-usd", "USD");
    complete_source(&service, &snapshot, &version, "backtest-eur", "EUR");
    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    let record = comparisons
        .create(
            request(vec!["backtest-usd", "backtest-eur"], "comparison-currency"),
            20,
            &context("comparison-currency"),
        )
        .expect("blocked comparison persisted");
    let artifact = comparisons
        .get_artifact(&record.comparison_id)
        .expect("artifact")
        .expect("artifact exists");
    assert!(artifact.artifact.blocked);
    assert!(artifact
        .artifact
        .aligned_metrics
        .iter()
        .all(|metric| metric.values_by_run.values().all(Option::is_none)));
}

#[test]
fn source_substitution_fails_closed_before_a_comparison_manifest_is_written() {
    let file = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(file.path().to_string_lossy());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot-tamper"))
        .expect("snapshot stored");
    complete_source(&service, &snapshot, &version, "backtest-a", "USD");
    complete_source(&service, &snapshot, &version, "backtest-b", "USD");
    let runtime = service.runtime();
    let mut tampered = runtime
        .backtests
        .get_run("backtest-a")
        .expect("run read")
        .expect("run exists");
    tampered.result_hash = Some("sha256:substituted".to_string());
    runtime
        .storage
        .put_json(
            "backtest_runs_v2",
            "backtest-a",
            serde_json::to_value(tampered).expect("serialized tamper"),
            &context("tamper"),
        )
        .expect("tamper write");
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    assert!(comparisons
        .create(
            request(vec!["backtest-a", "backtest-b"], "comparison-tamper"),
            20,
            &context("comparison-tamper")
        )
        .is_err());
    assert!(comparisons.list().expect("list").is_empty());
}

#[test]
fn robustness_only_comparison_is_order_independent_and_preserves_unavailable_series() {
    let file = NamedTempFile::new().expect("database");
    let path = file.path().to_string_lossy().to_string();
    let service = TradeAssemblyService::test_local(path.clone());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot-robustness"))
        .expect("snapshot stored");
    complete_source(&service, &snapshot, &version, "backtest-robust-a", "USD");
    complete_source(&service, &snapshot, &version, "backtest-robust-b", "USD");
    complete_robustness_source(
        &service,
        &snapshot,
        &version,
        "backtest-robust-a",
        "robustness-a",
    );
    complete_robustness_source(
        &service,
        &snapshot,
        &version,
        "backtest-robust-b",
        "robustness-b",
    );
    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    let first = comparisons
        .create(
            request(
                vec!["robustness-b", "robustness-a"],
                "robustness-idempotent",
            ),
            30,
            &context("robustness-idempotent"),
        )
        .expect("robustness comparison");
    let second = comparisons
        .create(
            request(
                vec!["robustness-a", "robustness-b"],
                "robustness-idempotent",
            ),
            31,
            &context("robustness-idempotent"),
        )
        .expect("robustness idempotent retry");
    assert_eq!(first, second);
    let manifest = comparisons
        .get_manifest(&first.comparison_id)
        .expect("manifest")
        .expect("manifest exists");
    assert!(manifest.content.sources.iter().all(|source| matches!(
        source.kind,
        tradeassembly_runtime::research_comparison::durable::ComparisonSourceKind::Robustness
    )));
    let artifact = comparisons
        .get_artifact(&first.comparison_id)
        .expect("artifact")
        .expect("artifact exists");
    assert!(!artifact.artifact.blocked);
    assert_eq!(artifact.artifact.absolute_equity.unavailable.len(), 2);
    assert_eq!(artifact.artifact.drawdown.unavailable.len(), 2);
    assert!(artifact
        .artifact
        .aligned_metrics
        .iter()
        .any(|metric| metric.metric == "robustness.monte_carlo.loss_probability"));

    let restarted = TradeAssemblyService::test_local(path);
    let runtime = restarted.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    assert_eq!(
        comparisons.get(&first.comparison_id).expect("restart"),
        Some(first)
    );
}

#[test]
fn mixed_backtest_and_robustness_sources_follow_study_semantics_policy() {
    let file = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(file.path().to_string_lossy());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot-mixed"))
        .expect("snapshot stored");
    complete_source(&service, &snapshot, &version, "backtest-mixed-a", "USD");
    complete_source(&service, &snapshot, &version, "backtest-mixed-b", "USD");
    complete_robustness_source(
        &service,
        &snapshot,
        &version,
        "backtest-mixed-a",
        "robustness-mixed-a",
    );
    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    let blocked = comparisons
        .create(
            request(
                vec!["backtest-mixed-b", "robustness-mixed-a"],
                "mixed-semantics-block",
            ),
            30,
            &context("mixed-semantics-block"),
        )
        .expect("blocked comparison persists");
    assert!(
        comparisons
            .get_artifact(&blocked.comparison_id)
            .expect("blocked artifact")
            .expect("blocked artifact exists")
            .artifact
            .blocked
    );

    let mut warning = request(
        vec!["backtest-mixed-b", "robustness-mixed-a"],
        "mixed-semantics-warning",
    );
    warning.policy.study_semantics =
        tradeassembly_runtime::research_comparison::ComparisonPolicy::Warn;
    let warned = comparisons
        .create(warning, 31, &context("mixed-semantics-warning"))
        .expect("warning comparison persists");
    let artifact = comparisons
        .get_artifact(&warned.comparison_id)
        .expect("warning artifact")
        .expect("warning artifact exists");
    assert!(!artifact.artifact.blocked);
    assert!(artifact
        .artifact
        .warnings
        .iter()
        .any(|warning| warning == "comparison_warning:studySemantics"));
}

#[test]
fn tampered_robustness_result_fails_closed_before_comparison_persistence() {
    let file = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(file.path().to_string_lossy());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot-robustness-tamper"))
        .expect("snapshot stored");
    complete_source(
        &service,
        &snapshot,
        &version,
        "backtest-robust-tamper-a",
        "USD",
    );
    complete_source(
        &service,
        &snapshot,
        &version,
        "backtest-robust-tamper-b",
        "USD",
    );
    complete_robustness_source(
        &service,
        &snapshot,
        &version,
        "backtest-robust-tamper-a",
        "robustness-tamper-a",
    );
    complete_robustness_source(
        &service,
        &snapshot,
        &version,
        "backtest-robust-tamper-b",
        "robustness-tamper-b",
    );
    let runtime = service.runtime();
    let mut tampered = runtime
        .robustness
        .get_result("robustness-tamper-a")
        .expect("robustness result read")
        .expect("robustness result");
    tampered.content.output["resultHash"] = json!("sha256:substituted");
    runtime
        .storage
        .put_json(
            "robustness_results_v1",
            "robustness-tamper-a",
            serde_json::to_value(tampered).expect("serialized tamper"),
            &context("robustness-tamper"),
        )
        .expect("tamper write");
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    assert!(comparisons
        .create(
            request(
                vec!["robustness-tamper-a", "robustness-tamper-b"],
                "robustness-tamper-comparison",
            ),
            30,
            &context("robustness-tamper-comparison"),
        )
        .is_err());
    assert!(comparisons.list().expect("list").is_empty());
}

#[test]
fn comparison_http_surface_exposes_durable_artifact_and_exports_after_restart() {
    let file = NamedTempFile::new().expect("database");
    let path = file.path().to_string_lossy().to_string();
    let service = TradeAssemblyService::test_local(path.clone());
    let snapshot = snapshot();
    let version = version(strategy());
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("snapshot-http"))
        .expect("snapshot stored");
    complete_source(&service, &snapshot, &version, "backtest-http-a", "USD");
    complete_source(&service, &snapshot, &version, "backtest-http-b", "USD");
    let request = json!({
        "sourceRunIds":["backtest-http-b","backtest-http-a"],
        "scope":{
            "strategyId":"strategy-1",
            "allowCrossVersion":true,
            "compatibleStrategyIds":[]
        },
        "idempotencyKey":"comparison-http-create",
        "actor":"local-user",
        "surface":"http"
    });
    let created = service.handle_http("POST", "/research-comparisons", request.clone());
    assert_eq!(created.status, 201, "{created:#?}");
    let comparison_id = created.body["record"]["comparisonId"]
        .as_str()
        .expect("comparison id")
        .to_string();
    assert!(!created.body["artifact"]["artifact"]["blocked"]
        .as_bool()
        .expect("blocked status"));
    let mut bound_sources = created.body["manifest"]["content"]["sources"]
        .as_array()
        .expect("source bindings")
        .iter()
        .filter_map(|value| value["sourceRunId"].as_str())
        .collect::<Vec<_>>();
    bound_sources.sort_unstable();
    assert_eq!(bound_sources, vec!["backtest-http-a", "backtest-http-b"]);

    let replay = service.handle_http("POST", "/research-comparisons", request);
    assert_eq!(replay.status, 201, "{replay:#?}");
    assert_eq!(replay.body["record"]["comparisonId"], comparison_id);

    let restarted = TradeAssemblyService::test_local(path);
    let fetched = restarted.handle_http(
        "GET",
        &format!("/research-comparisons/{comparison_id}"),
        json!({}),
    );
    assert_eq!(fetched.status, 200, "{fetched:#?}");
    assert_eq!(
        fetched.body["artifact"]["artifact"]["comparisonHash"],
        created.body["artifact"]["artifact"]["comparisonHash"]
    );
    let listed = restarted.handle_http("GET", "/research-comparisons", json!({}));
    assert_eq!(listed.status, 200, "{listed:#?}");
    assert_eq!(listed.body["comparisons"].as_array().map(Vec::len), Some(1));
    let graphql = restarted.execute_graphql(json!({
        "operationName":"ResearchComparison",
        "query":"query ResearchComparison($comparisonId: String!) { researchComparison(comparisonId: $comparisonId) }",
        "variables":{"comparisonId":comparison_id}
    }));
    assert!(graphql.get("errors").is_none(), "{graphql:#?}");
    assert_eq!(
        graphql["data"]["researchComparison"]["artifact"]["artifact"]["comparisonHash"],
        created.body["artifact"]["artifact"]["comparisonHash"]
    );
    for format in ["json", "csv"] {
        let exported = restarted.handle_http(
            "GET",
            &format!("/research-comparisons/{comparison_id}/exports/{format}"),
            json!({}),
        );
        assert_eq!(exported.status, 200, "{exported:#?}");
        assert_eq!(exported.body["export"]["exportKind"], format);
        assert!(exported.body["export"]["contentHash"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:")));
    }
}
