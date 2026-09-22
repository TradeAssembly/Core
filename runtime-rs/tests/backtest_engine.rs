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
            vwap: None,
            session: None,
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

fn portfolio_case() -> (DatasetSnapshot, Value, BacktestRunManifest) {
    let mut content = snapshot().content;
    content.instruments = vec!["QQQ".into(), "SPY".into()];
    content.granularity = "1m".into();
    content.timezone = "America/New_York".into();
    content.time_slice = DatasetTimeSlice {
        start: "2026-01-02T14:29:00Z".into(),
        end: "2026-01-02T14:33:00Z".into(),
    };
    content.observations.clear();
    for symbol in ["QQQ", "SPY"] {
        for (minute, open, close, volume) in [
            (29, 300.0, 300.0, 100.0),
            (30, 100.0, 100.0, 100.0),
            (31, 100.0, 99.0, 200.0),
            (32, 98.0, 100.0, 100.0),
            (33, 101.0, 100.0, 100.0),
        ] {
            let mut row = bar(&format!("2026-01-02T14:{minute}:00Z"), open, close);
            row.instrument_id = symbol.into();
            let HistoricalObservationData::Bar(bar) = &mut row.data else {
                unreachable!()
            };
            bar.volume = volume;
            bar.vwap = Some(if minute == 29 { 300.0 } else { 100.0 });
            bar.session = Some(BarSession {
                calendar: "XNYS".into(),
                start: "2026-01-02T09:30:00-05:00".into(),
                end: "2026-01-02T16:00:00-05:00".into(),
                source_ref: "controlled-calendar:2026-01-02".into(),
            });
            content.observations.push(row);
        }
    }
    let snapshot = DatasetSnapshot::build(content, 1).unwrap();
    let mut spec = strategy("bar.close > 0", None);
    spec["signal_market_requirements"][0]["timeframes"] = json!(["1m"]);
    spec["signal_market_requirements"][0]["selectors"] = json!([
        {"selector_id":"q","type":"normalized_instrument","normalized_id":"QQQ"},
        {"selector_id":"s","type":"normalized_instrument","normalized_id":"SPY"}
    ]);
    spec["stages"]["evaluate"]["substeps"][0]["config"] = json!({"tradeassembly.portfolio_vwap": {
        "version":1,"price_basis":"provider_bar_vwap","fills":"exact_next_minute_open_expire_remainder",
        "policy":{"volume_window":1,"entry_notional_micros":100*UNIT,"maximum_positions":1,"maximum_exposure_micros":500*UNIT,
        "rules":{"entry_discount_bps":50,"minimum_volume_ratio_bps":12000,"exit_proximity_bps":10,"maximum_holding_ms":900000,"maximum_loss_bps":50,"entry_cooldown_ms":600000}}
    }});
    spec["stages"]["exit_policy"]["policy"]["monitoring"] = json!(["synthetic"]);
    spec["stages"]["exit_policy"]["policy"]["triggers"] = json!([
        {"id":"vwap_proximity","kind":"indicator","scope":"full_position"},
        {"id":"maximum_holding","kind":"time","scope":"full_position"},
        {"id":"maximum_loss","kind":"profit_loss","scope":"full_position"}
    ]);
    spec["stages"]["order_strategy"]["policy"]["allowed_order_types"] = json!(["market"]);
    let validation = tradeassembly_runtime::spec::validate_strategy_spec_report(&spec);
    assert!(validation.valid, "{:?}", validation.diagnostics);
    let version = version(spec);
    let mut content = manifest(&snapshot, &version, EndOfDataPolicy::MarkToMarket).content;
    let config = &mut content.configuration;
    config.dataset.instruments = snapshot.content.instruments.clone();
    config.dataset.granularity = "1m".into();
    config.dataset.timezone = "America/New_York".into();
    config.execution.fill_timing = FillTiming::NextBar;
    config.execution.fill_price_source = PriceSource::Open;
    let mut qqq = config.instruments[0].clone();
    qqq.instrument_id = "QQQ".into();
    config.instruments.insert(0, qqq);
    content.validation = config.validate();
    let manifest = BacktestRunManifest::build(content, 1).unwrap();
    (snapshot, version, manifest)
}

#[test]
fn portfolio_compiler_accounting_and_replay_keep_original_source_indices() {
    let (snapshot, version, manifest) = portfolio_case();
    let result = execute(&manifest, &version, &snapshot, 10).unwrap();
    assert_eq!(result.content.fills.len(), 2);
    assert_eq!(result.content.fills[0]["symbol"], "QQQ");
    assert_eq!(result.content.fills[0]["decisionBarIndex"], 2);
    assert_eq!(result.content.fills[0]["executionBarIndex"], 3);
    assert_eq!(
        result.content.fills[0]["executionTimestamp"],
        "2026-01-02T14:32:00Z"
    );
    assert_eq!(result.content.fills[0]["priceMicros"], 98 * UNIT);
    assert_eq!(
        result.content.fills[0]["decisionTimestamp"],
        "2026-01-02T14:32:00Z"
    );
    assert_eq!(result.content.orders[0]["notionalMicros"], 100 * UNIT);
    assert!(result.content.orders[0].get("quantityMicros").is_none());
    assert_eq!(
        result.content.accounting["endingEquityMicros"],
        1_003_061_224_i64
    );
    assert_eq!(
        replay(&manifest, &version, &snapshot, &result)
            .unwrap()
            .result_hash,
        result.result_hash
    );
    assert!(result
        .content
        .decisions
        .iter()
        .all(|d| d["barIndex"] != 0 && d["barIndex"] != 5));
}

#[test]
fn portfolio_missing_evidence_and_conflicting_risk_fail_closed() {
    let (snapshot, version, manifest) = portfolio_case();
    let mut wrong = manifest.content.clone();
    wrong.configuration.risk.maximum_open_positions = 2;
    wrong.validation = wrong.configuration.validate();
    let wrong = BacktestRunManifest::build(wrong, 1).unwrap();
    assert_eq!(
        execute(&wrong, &version, &snapshot, 10).unwrap_err(),
        "backtest_portfolio_policy_mismatch"
    );
    for missing in ["vwap", "session"] {
        let mut data = snapshot.content.clone();
        let HistoricalObservationData::Bar(bar) = &mut data.observations[2].data else {
            unreachable!()
        };
        if missing == "vwap" {
            bar.vwap = None;
        } else {
            bar.session = None;
        }
        let data = DatasetSnapshot::build(data, 1).unwrap();
        let mut rebound = manifest.content.clone();
        rebound.configuration.dataset.dataset_id = data.dataset_id.clone();
        rebound.configuration.dataset.snapshot_ref = data.snapshot_ref.clone();
        rebound.configuration.dataset.content_hash = data.content_hash.clone();
        rebound.validation = rebound.configuration.validate();
        let rebound = BacktestRunManifest::build(rebound, 1).unwrap();
        assert_eq!(
            execute(&rebound, &version, &data, 10).unwrap_err(),
            if missing == "vwap" {
                "backtest_provider_vwap_required"
            } else {
                "backtest_session_evidence_required"
            }
        );
    }
}

#[test]
fn portfolio_durable_worker_restart_report_and_replay_use_real_engine() {
    use tradeassembly_runtime::ports::{IdempotencyKey, SideEffectContext};
    use tradeassembly_runtime::service::{backtest_lifecycle, TradeAssemblyService};
    let (snapshot, mut version, manifest) = portfolio_case();
    let db = tempfile::NamedTempFile::new().unwrap();
    let path = db.path().to_str().unwrap();
    let service = TradeAssemblyService::test_local(path);
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("portfolio-fixture").unwrap(),
    );
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context)
        .unwrap();
    version["published"] = json!(true);
    service
        .runtime()
        .storage
        .put_json("strategy_versions", "strategy-1:1", version, &context)
        .unwrap();
    let mut configuration = manifest.content.configuration;
    configuration.capability_graph.revision_id.clear();
    configuration.capability_graph.graph_fingerprint.clear();
    let created = backtest_lifecycle::create(
        &service,
        json!({"idempotencyKey":"portfolio-run","client":"test","purpose":"research","configuration":configuration}),
    );
    assert_eq!(created.status, 202, "{}", created.body);
    let run_id = created.body["runId"].as_str().unwrap().to_string();
    drop(service);
    let service = TradeAssemblyService::test_local(path);
    // No process_with callback or prebuilt result: invoke the production worker.
    let completed = backtest_lifecycle::process(&service, "portfolio-worker");
    assert_eq!(completed.status, 200, "{}", completed.body);
    assert_eq!(completed.body["status"], "completed", "{}", completed.body);
    let report = backtest_lifecycle::report(&service, &run_id);
    assert_eq!(report.status, 200, "{}", report.body);
    let replayed = backtest_lifecycle::replay(&service, &run_id);
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    let export = backtest_lifecycle::export(&service, &run_id, "ordersFillsCsv");
    assert_eq!(export.status, 200, "{}", export.body);
    assert!(
        export.body.to_string().contains("notional_micros"),
        "{}",
        export.body
    );
}

#[test]
fn portfolio_contract_and_compilation_are_discoverable_over_mcp() {
    use tradeassembly_runtime::service::TradeAssemblyService;
    let (_, version, _) = portfolio_case();
    let directory = tempfile::tempdir().unwrap();
    let service =
        TradeAssemblyService::test_local(directory.path().join("discovery.db").to_string_lossy());
    let schema = service.call_mcp_tool("tradeassembly.strategy.schema", json!({}));
    let evaluator = &schema["structuredContent"]["evaluators"][0];
    assert_eq!(evaluator["id"], "portfolio-vwap-v1");
    assert_eq!(evaluator["configSchema"]["type"], "object");
    assert!(!evaluator["configSchema"]
        .to_string()
        .contains("\"default\""));
    assert_eq!(evaluator["statelessExecutionSupported"], false);
    let validated = service.call_mcp_tool(
        "tradeassembly.strategy.validate",
        json!({"spec":version["spec"]}),
    );
    assert_eq!(validated["structuredContent"]["valid"], true, "{validated}");
    assert_eq!(
        validated["structuredContent"]["compilation"]["backtest"]["supported"],
        true
    );
    assert_eq!(
        validated["structuredContent"]["compilation"]["statelessExecution"]["supported"],
        false
    );
    assert_eq!(
        validated["structuredContent"]["compilation"]["authorizationGranted"],
        false
    );
    let mut unsupported = version["spec"].clone();
    unsupported["stages"]["evaluate"]["substeps"][0]["config"]["tradeassembly.portfolio_vwap"]
        ["price_basis"] = json!("invented");
    let rejected = service.call_mcp_tool(
        "tradeassembly.strategy.validate",
        json!({"spec":unsupported}),
    );
    assert_eq!(rejected["structuredContent"]["valid"], true);
    assert_eq!(
        rejected["structuredContent"]["compilation"]["backtest"]["supported"],
        false
    );
    assert!(rejected["structuredContent"]["compilation"]["backtest"]["reason"].is_string());
    assert!(service
        .runtime()
        .storage
        .list_json("strategies")
        .unwrap()
        .is_empty());
}

#[test]
fn ten_symbol_twenty_bar_example_enforces_shared_four_position_limit() {
    let (base_data, base_version, base_manifest) = portfolio_case();
    let mut content = base_data.content;
    content.instruments = [
        "AAPL", "AMD", "AMZN", "IWM", "META", "MSFT", "NVDA", "QQQ", "SPY", "TSLA",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    content.time_slice = DatasetTimeSlice {
        start: "2026-01-02T14:30:00Z".into(),
        end: "2026-01-02T14:53:00Z".into(),
    };
    content.observations.clear();
    let start = chrono::DateTime::parse_from_rfc3339(&content.time_slice.start).unwrap();
    for symbol in &content.instruments {
        for minute in 0..24 {
            let (open, close, volume) = match minute {
                20 => (100.0, 99.0, 120.0),
                21 => (98.0, 100.0, 100.0),
                22 => (101.0, 100.0, 100.0),
                _ => (100.0, 100.0, 100.0),
            };
            let timestamp = (start + chrono::Duration::minutes(minute))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let mut row = bar(&timestamp, open, close);
            row.instrument_id = symbol.clone();
            let HistoricalObservationData::Bar(bar) = &mut row.data else {
                unreachable!()
            };
            bar.volume = volume;
            bar.vwap = Some(100.0);
            bar.session = Some(BarSession {
                calendar: "XNYS".into(),
                start: "2026-01-02T09:30:00-05:00".into(),
                end: "2026-01-02T16:00:00-05:00".into(),
                source_ref: "controlled-calendar:2026-01-02".into(),
            });
            content.observations.push(row);
        }
    }
    let data = DatasetSnapshot::build(content, 1).unwrap();
    let mut spec = base_version["spec"].clone();
    spec["signal_market_requirements"][0]["selectors"] = data.content.instruments.iter().enumerate().map(|(i,s)|json!({"selector_id":format!("symbol_{i}"),"type":"normalized_instrument","normalized_id":s})).collect::<Vec<_>>().into();
    spec["stages"]["evaluate"]["substeps"][0]["config"]["tradeassembly.portfolio_vwap"]["policy"]
        ["volume_window"] = json!(20);
    spec["stages"]["evaluate"]["substeps"][0]["config"]["tradeassembly.portfolio_vwap"]["policy"]
        ["maximum_positions"] = json!(4);
    let version = version(spec);
    let mut content = base_manifest.content;
    let config = &mut content.configuration;
    config.strategy.strategy_spec_hash = version["specHash"].as_str().unwrap().into();
    config.dataset.dataset_id = data.dataset_id.clone();
    config.dataset.snapshot_ref = data.snapshot_ref.clone();
    config.dataset.content_hash = data.content_hash.clone();
    config.dataset.selected_time_slice = data.content.time_slice.clone();
    config.dataset.instruments = data.content.instruments.clone();
    let template = config.instruments[0].clone();
    config.instruments = data
        .content
        .instruments
        .iter()
        .map(|symbol| {
            let mut instrument = template.clone();
            instrument.instrument_id = symbol.clone();
            instrument
        })
        .collect();
    config.risk.maximum_open_positions = 4;
    content.validation = config.validate();
    let manifest = BacktestRunManifest::build(content, 1).unwrap();
    let result = execute(&manifest, &version, &data, 10).unwrap();
    assert_eq!(result.content.fills.len(), 8);
    let buys = result
        .content
        .fills
        .iter()
        .filter(|f| f["side"] == "buy")
        .collect::<Vec<_>>();
    assert_eq!(
        buys.iter()
            .map(|f| f["symbol"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["AAPL", "AMD", "AMZN", "IWM"]
    );
    for buy in buys {
        assert_eq!(buy["decisionTimestamp"], "2026-01-02T14:51:00Z");
        assert_eq!(buy["executionTimestamp"], "2026-01-02T14:51:00Z");
    }
    assert_eq!(
        result.content.accounting["endingEquityMicros"],
        1_012_244_896_i64
    );
    assert_eq!(
        replay(&manifest, &version, &data, &result)
            .unwrap()
            .result_hash,
        result.result_hash
    );
}
