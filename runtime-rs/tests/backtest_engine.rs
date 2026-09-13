use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tradeassembly_runtime::backtest_contracts::*;
use tradeassembly_runtime::backtest_engine::{execute, replay};
use tradeassembly_runtime::historical_data::*;
use tradeassembly_runtime::ports::AuthorityContext;

const UNIT: i64 = 1_000_000;

fn strategy(entry: &str, exit: Option<&str>) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3/valid/static-equity.json");
    let mut spec: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.expression":entry,"tradeassembly.quantity":UNIT});
    if let Some(exit) = exit {
        spec["stages"]["exit_policy"]["substeps"][0]["config"]["tradeassembly.expression"] =
            json!(exit);
    }
    spec
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
                corporate_action_gaps: QualityDisposition::Reject,
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
    .unwrap()
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

fn version(spec: Value) -> Value {
    let hash = canonical_hash(&spec, "hash").unwrap();
    json!({
        "id":"version-1",
        "strategyId":"strategy-1",
        "specHash":hash,
        "spec":spec,
        "immutable":true
    })
}

fn manifest(
    snapshot: &DatasetSnapshot,
    version: &Value,
    end_of_data: EndOfDataPolicy,
) -> BacktestRunManifest {
    let configuration = BacktestConfiguration {
        schema: BACKTEST_CONFIGURATION_SCHEMA.to_string(),
        strategy: StrategyVersionBinding {
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: "version-1".to_string(),
            strategy_spec_hash: version["specHash"].as_str().unwrap().to_string(),
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
            reporting_currency: "USD".to_string(),
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
        end_of_data,
        evaluator_version: "kernel-1".to_string(),
        compiler_version: "compiler-1".to_string(),
        plugin_fingerprints: BTreeMap::new(),
        variable_overrides: BTreeMap::new(),
    };
    BacktestRunManifest::build(
        BacktestRunManifestContent {
            schema: BACKTEST_MANIFEST_SCHEMA.to_string(),
            run_id: "run-1".to_string(),
            request_hash: "sha256:request".to_string(),
            idempotency_key: "run-1".to_string(),
            configuration: configuration.clone(),
            validation: configuration.validate(),
            authority: AuthorityContext::local_cli(),
            client: "self".to_string(),
            purpose: "research".to_string(),
            evidence_refs: vec![],
            first_attempt_id: "run-1:attempt:1".to_string(),
        },
        1,
    )
    .unwrap()
}

#[test]
fn strategy_dataset_and_accounting_drive_a_deterministic_replayable_result() {
    let snapshot = snapshot();
    let version = version(strategy(
        "bar.close > 101000000",
        Some("bar.close < 100000000"),
    ));
    let manifest = manifest(&snapshot, &version, EndOfDataPolicy::MarkToMarket);
    let first = execute(&manifest, &version, &snapshot, 10).unwrap();
    let second = execute(&manifest, &version, &snapshot, 20).unwrap();
    assert_eq!(first.result_hash, second.result_hash);
    assert_eq!(first.content.fills.len(), 2);
    assert_eq!(first.content.fills[0]["side"], "buy");
    assert_eq!(first.content.fills[1]["side"], "sell");
    assert_eq!(first.content.accounting["endingEquityMicros"], 996 * UNIT);
    assert_eq!(
        replay(&manifest, &version, &snapshot, &first)
            .unwrap()
            .result_hash,
        first.result_hash
    );
}

#[test]
fn no_signal_creates_no_orders_and_force_close_is_explicit_and_costed() {
    let snapshot = snapshot();
    let quiet = version(strategy("bar.close > 200000000", None));
    let quiet_result = execute(
        &manifest(&snapshot, &quiet, EndOfDataPolicy::MarkToMarket),
        &quiet,
        &snapshot,
        10,
    )
    .unwrap();
    assert!(quiet_result.content.orders.is_empty());
    assert!(quiet_result.content.fills.is_empty());

    let open = version(strategy("bar.close > 101000000", None));
    let mut force = manifest(&snapshot, &open, EndOfDataPolicy::ForceClose);
    force.content.configuration.costs.fixed_fee_micros = UNIT;
    force = BacktestRunManifest::build(force.content, 1).unwrap();
    let result = execute(&force, &open, &snapshot, 10).unwrap();
    assert_eq!(result.content.fills.len(), 2);
    assert_eq!(result.content.fills[1]["endOfData"], true);
    assert_eq!(result.content.fills[1]["feeMicros"], UNIT);
}

#[test]
fn immutable_dataset_can_be_reused_by_a_newer_version_of_the_same_strategy() {
    let snapshot = snapshot();
    let mut newer_version = version(strategy("bar.close > 101000000", None));
    newer_version["id"] = json!("version-2");
    let mut newer_manifest = manifest(&snapshot, &newer_version, EndOfDataPolicy::MarkToMarket);
    newer_manifest
        .content
        .configuration
        .strategy
        .strategy_version_id = "version-2".to_string();
    newer_manifest = BacktestRunManifest::build(newer_manifest.content, 1).unwrap();

    let result = execute(&newer_manifest, &newer_version, &snapshot, 10).unwrap();

    assert_eq!(
        newer_manifest
            .content
            .configuration
            .strategy
            .strategy_version_id,
        "version-2"
    );
    assert_eq!(result.content.run_id, newer_manifest.content.run_id);
}

#[test]
fn tampered_or_unavailable_inputs_and_replay_mismatch_fail_closed() {
    let snapshot = snapshot();
    let version = version(strategy("bar.close > 101000000", None));
    let manifest = manifest(&snapshot, &version, EndOfDataPolicy::MarkToMarket);
    let result = execute(&manifest, &version, &snapshot, 10).unwrap();

    let mut bad_version = version.clone();
    bad_version["spec"]["name"] = json!("tampered");
    assert_eq!(
        execute(&manifest, &bad_version, &snapshot, 10).unwrap_err(),
        "backtest_strategy_integrity_failed"
    );
    let mut bad_snapshot = snapshot.clone();
    let HistoricalObservationData::Bar(bar) = &mut bad_snapshot.content.observations[0].data else {
        unreachable!()
    };
    bar.close = 500.0;
    assert_eq!(
        execute(&manifest, &version, &bad_snapshot, 10).unwrap_err(),
        "dataset_snapshot_integrity_failed"
    );
    let mut mismatched = result.clone();
    mismatched.content.accounting["endingEquityMicros"] = json!(0);
    mismatched = BacktestResult::build(mismatched.content, 10).unwrap();
    assert_eq!(
        replay(&manifest, &version, &snapshot, &mismatched).unwrap_err(),
        "backtest_replay_mismatch"
    );
}
