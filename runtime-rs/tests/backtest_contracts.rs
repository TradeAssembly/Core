use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tradeassembly_runtime::adapters::backtest_repository::StorageBacktestRunRepository;
use tradeassembly_runtime::adapters::local::sqlite::LocalSqliteStorage;
use tradeassembly_runtime::backtest_contracts::*;
use tradeassembly_runtime::domain::BacktestRunState;
use tradeassembly_runtime::historical_data::DatasetTimeSlice;
use tradeassembly_runtime::ports::{
    AuthorityContext, BacktestImmutableWrite, BacktestRunRepository, IdempotencyKey,
    SideEffectContext, StoragePort,
};

fn context() -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("gc-11-contract-test").expect("idempotency key"),
    )
}

#[test]
fn mcp_backtest_schema_validates_nested_contract_without_source_inspection() {
    let tools = tradeassembly_runtime::mcp::tool_definitions();
    let schema = &tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "tradeassembly.backtest.run")
        .unwrap()["inputSchema"];
    let validator =
        jsonschema::validator_for(schema).expect("all nested schema references resolve");
    let mut request =
        json!({"idempotency_key":"schema-test", "request":{"configuration":configuration()}});
    assert!(validator.is_valid(&request));
    request["request"]["configuration"]["instruments"][0]["metadata"] =
        json!({"testAssumption":true});
    assert!(
        !validator.is_valid(&request),
        "metadata values must be strings"
    );
    request["request"]["configuration"]["instruments"][0]["metadata"] =
        json!({"testAssumption":"true"});
    request["request"]["configuration"]["execution"]["fillTiming"] = json!("invented");
    assert!(
        !validator.is_valid(&request),
        "fill timing must match the typed enum"
    );
}

fn configuration() -> BacktestConfiguration {
    BacktestConfiguration {
        schema: BACKTEST_CONFIGURATION_SCHEMA.to_string(),
        strategy: StrategyVersionBinding {
            strategy_id: "strategy_fixture".to_string(),
            strategy_version_id: "version_fixture_1".to_string(),
            strategy_spec_hash: "sha256:spec".to_string(),
        },
        dataset: DatasetBinding {
            dataset_id: "dataset_fixture".to_string(),
            snapshot_ref: "tradeassembly://datasets/dataset_fixture".to_string(),
            content_hash: "sha256:dataset".to_string(),
            schema: "tradeassembly.dataset-snapshot.v1".to_string(),
            source_plugin_ref: "tradeassembly.local-data".to_string(),
            source_plugin_instance_ref: "local-data".to_string(),
            source_operation_id: "marketdata.bars.read_v1".to_string(),
            source_manifest_fingerprint: "sha256:plugin".to_string(),
            source_class: "test".to_string(),
            capability_graph_revision_id: "graph_fixture".to_string(),
            capability_graph_fingerprint: "sha256:graph".to_string(),
            instruments: vec!["BTC/USD".to_string()],
            selected_time_slice: DatasetTimeSlice {
                start: "2026-01-01T00:00:00Z".to_string(),
                end: "2026-01-03T00:00:00Z".to_string(),
            },
            granularity: "1h".to_string(),
            calendar: "CRYPTO_24X7".to_string(),
            timezone: "UTC".to_string(),
            corporate_action_policy: "not_applicable".to_string(),
            benchmark: None,
        },
        capability_graph: CapabilityGraphBinding {
            revision_id: "caprev-1".to_string(),
            graph_fingerprint: "sha256:capability-graph".to_string(),
        },
        capital: CapitalConfiguration {
            starting_cash_micros: 10_000_000_000,
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
            fixed_fee_micros: 100_000,
            per_unit_fee_micros: 10,
            notional_fee_bps: 1,
            spread_bps: 2,
            slippage_bps: 3,
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
            maximum_position_notional_micros: 5_000_000_000,
            maximum_order_quantity_micros: 1_000_000,
            maximum_loss_micros: 1_000_000_000,
            maximum_open_positions: 1,
        },
        end_of_data: EndOfDataPolicy::MarkToMarket,
        evaluator_version: "strategy-kernel-v1".to_string(),
        compiler_version: "strategy-spec-v3-compiler-v1".to_string(),
        plugin_fingerprints: BTreeMap::from([(
            "local-data".to_string(),
            "sha256:plugin".to_string(),
        )]),
        variable_overrides: BTreeMap::new(),
    }
}

fn manifest(run_id: &str) -> BacktestRunManifest {
    let configuration = configuration();
    BacktestRunManifest::build(
        BacktestRunManifestContent {
            schema: BACKTEST_MANIFEST_SCHEMA.to_string(),
            run_id: run_id.to_string(),
            request_hash: configuration.canonical_hash().expect("config hash"),
            idempotency_key: format!("create:{run_id}"),
            validation: configuration.validate(),
            configuration,
            authority: AuthorityContext::local_cli(),
            client: "self".to_string(),
            purpose: "strategy_backtest_research".to_string(),
            evidence_refs: vec!["tradeassembly://datasets/dataset_fixture".to_string()],
            first_attempt_id: format!("attempt_{run_id}_1"),
        },
        100,
    )
    .expect("manifest")
}

#[test]
fn configuration_is_closed_validated_and_semantically_hashed() {
    let config = configuration();
    assert!(config.validate().ready);
    let hash = config.canonical_hash().expect("hash");

    let round_trip: BacktestConfiguration =
        serde_json::from_value(serde_json::to_value(&config).expect("serialize")).expect("parse");
    assert_eq!(hash, round_trip.canonical_hash().expect("round-trip hash"));

    let mut distinct = config.clone();
    distinct.costs.slippage_bps += 1;
    assert_ne!(hash, distinct.canonical_hash().expect("distinct hash"));

    let mut invalid = serde_json::to_value(config).expect("value");
    invalid["hiddenDefault"] = json!(true);
    assert!(serde_json::from_value::<BacktestConfiguration>(invalid).is_err());
}

#[test]
fn repository_keeps_manifest_and_result_immutable_across_restart() {
    let file = NamedTempFile::new().expect("db file");
    let path = file.path().to_string_lossy().to_string();
    let storage: Arc<dyn StoragePort> = Arc::new(LocalSqliteStorage::new(&path));
    let repository = StorageBacktestRunRepository::new(Arc::clone(&storage));
    let manifest = manifest("run_fixture");
    assert_eq!(
        repository
            .put_manifest(&manifest, &context())
            .expect("manifest create"),
        BacktestImmutableWrite::Created
    );
    assert_eq!(
        repository
            .put_manifest(&manifest, &context())
            .expect("manifest duplicate"),
        BacktestImmutableWrite::AlreadyPresent
    );

    let run = BacktestRunRecord {
        schema: BACKTEST_RUN_SCHEMA.to_string(),
        run_id: "run_fixture".to_string(),
        manifest_hash: manifest.manifest_hash.clone(),
        request_hash: manifest.content.request_hash.clone(),
        idempotency_key: manifest.content.idempotency_key.clone(),
        state: BacktestRunState::Queued,
        sequence: 1,
        current_attempt_id: "attempt_run_fixture_1".to_string(),
        fencing_token: 0,
        result_hash: None,
        result: None,
        failure_code: None,
        events: Vec::new(),
        created_at_ms: 100,
        updated_at_ms: 100,
    };
    repository.create_run(&run, &context()).expect("run create");
    assert_eq!(
        repository
            .find_run_by_idempotency_key("create:run_fixture")
            .expect("idempotency lookup"),
        Some(run.clone())
    );

    let result = BacktestResult::build(
        BacktestResultContent {
            schema: BACKTEST_RESULT_SCHEMA.to_string(),
            run_id: run.run_id.clone(),
            manifest_hash: manifest.manifest_hash.clone(),
            evaluator_version: "strategy-kernel-v1".to_string(),
            compiler_version: "strategy-spec-v3-compiler-v1".to_string(),
            decisions: vec![],
            orders: vec![],
            fills: vec![],
            positions: vec![],
            ledger: vec![],
            accounting: json!({"startingCashMicros": 10_000_000_000_i64}),
            diagnostics: vec![],
        },
        200,
    )
    .expect("result");
    repository
        .put_result(&result, &context())
        .expect("result create");
    drop(repository);

    let restarted = StorageBacktestRunRepository::new(Arc::clone(&storage));
    assert_eq!(
        restarted
            .get_manifest("run_fixture")
            .expect("manifest read"),
        Some(manifest.clone())
    );
    assert_eq!(
        restarted.get_result("run_fixture").expect("result read"),
        Some(result.clone())
    );

    let mut substituted_content = result.content.clone();
    substituted_content.run_id = "run_other".to_string();
    let substituted = BacktestResult::build(substituted_content, 200).expect("substituted result");
    storage
        .put_json(
            "backtest_results_v1",
            "run_fixture",
            serde_json::to_value(substituted).expect("substituted value"),
            &context(),
        )
        .expect("simulate cross-run substitution");
    assert_eq!(
        restarted
            .get_result("run_fixture")
            .expect_err("cross-run substitution rejected"),
        "backtest_result_integrity_failed"
    );
    storage
        .put_json(
            "backtest_results_v1",
            "run_fixture",
            serde_json::to_value(&result).expect("result value"),
            &context(),
        )
        .expect("restore result");

    let mut conflicting = result.clone();
    conflicting.content.accounting = json!({"startingCashMicros": 1_i64});
    conflicting = BacktestResult::build(conflicting.content, 200).expect("conflicting result");
    assert_eq!(
        restarted
            .put_result(&conflicting, &context())
            .expect_err("conflicting result rejected"),
        "backtest_result_write_conflict"
    );

    let mut tampered = serde_json::to_value(&manifest).expect("manifest value");
    tampered["content"]["purpose"] = json!("other");
    storage
        .put_json("backtest_manifests_v1", "run_fixture", tampered, &context())
        .expect("simulate disk-owner tamper");
    assert_eq!(
        restarted
            .get_manifest("run_fixture")
            .expect_err("tamper rejected"),
        "backtest_manifest_integrity_failed"
    );
}
