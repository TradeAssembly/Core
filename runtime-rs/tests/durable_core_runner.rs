use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::NamedTempFile;
use tradeassembly_runtime::backtest_contracts::*;
use tradeassembly_runtime::historical_data::*;
use tradeassembly_runtime::ports::{
    AuthorityContext, CoreRunnerBridgePort, IdempotencyKey, RunnerBridgeCommand,
    RunnerBridgeErrorCode, RunnerBridgeReceiptState, RunnerBridgeWorkload, SideEffectContext,
    CORE_RUNNER_BRIDGE_SCHEMA, CORE_RUNNER_BRIDGE_VERSION,
};
use tradeassembly_runtime::runtime_config::{
    RuntimeBuilder, RuntimeConfig, RuntimeConfigLayer, SecretResolver,
};
use tradeassembly_runtime::service::core_runner::DurableCoreRunnerBridge;
use tradeassembly_runtime::service::TradeAssemblyService;

const CORE_RELEASE: &str = "test-core-release";

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("key"),
    )
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
            observations: vec![
                HistoricalObservation {
                    instrument_id: "BTC/USD".to_string(),
                    timestamp: "2026-01-01T00:00:00Z".to_string(),
                    data: HistoricalObservationData::Bar(BarObservation {
                        open: 1.0,
                        high: 1.1,
                        low: 0.9,
                        close: 1.0,
                        volume: 10.0,
                    }),
                },
                HistoricalObservation {
                    instrument_id: "BTC/USD".to_string(),
                    timestamp: "2026-01-01T01:00:00Z".to_string(),
                    data: HistoricalObservationData::Bar(BarObservation {
                        open: 1.0,
                        high: 1.2,
                        low: 1.0,
                        close: 1.1,
                        volume: 10.0,
                    }),
                },
            ],
        },
        1,
    )
    .expect("snapshot")
}

fn configuration(snapshot: &DatasetSnapshot, spec_hash: &str) -> BacktestConfiguration {
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
            corporate_action_policy: "none".to_string(),
            benchmark: None,
        },
        capability_graph: CapabilityGraphBinding {
            revision_id: "graph-1".to_string(),
            graph_fingerprint: "sha256:graph".to_string(),
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

fn seed(service: &TradeAssemblyService, key: &str) -> Value {
    let snapshot = snapshot();
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
        "tradeassembly.quantity": 100_000
    });
    let spec_hash = canonical_hash(&spec, "strategy hash").expect("strategy hash");
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context(key))
        .expect("snapshot stored");
    service
        .runtime()
        .storage
        .put_json(
            "strategy_versions",
            "strategy-1:1",
            json!({
                "id": "version-1",
                "strategyId": "strategy-1",
                "specHash": spec_hash,
                "spec": spec,
                "published": true
            }),
            &context(key),
        )
        .expect("version stored");
    json!({
        "runId": "run-hosted-1",
        "idempotencyKey": "manifest-source",
        "client": "self",
        "purpose": "strategy_backtest_research",
        "configuration": configuration(&snapshot, &spec_hash)
    })
}

fn command_from_source_manifest() -> (RunnerBridgeCommand, NamedTempFile, String) {
    let target_file = NamedTempFile::new().expect("target db");
    let target_path = target_file.path().to_string_lossy().to_string();
    let target = TradeAssemblyService::test_local(&target_path);
    let seeded = seed(&target, "target-seed");
    let configuration: BacktestConfiguration =
        serde_json::from_value(seeded["configuration"].clone()).expect("configuration");
    let manifest = BacktestRunManifest::build(
        BacktestRunManifestContent {
            schema: BACKTEST_MANIFEST_SCHEMA.to_string(),
            run_id: "run-hosted-1".to_string(),
            request_hash: canonical_hash(&configuration, "configuration hash")
                .expect("configuration hash"),
            idempotency_key: "manifest-source".to_string(),
            validation: configuration.validate(),
            configuration,
            authority: AuthorityContext {
                actor: "tradeassembly://identity/user-1".to_string(),
                surface: "studio_personal".to_string(),
                account_mode: "research".to_string(),
            },
            client: "self".to_string(),
            purpose: "strategy_backtest_research".to_string(),
            evidence_refs: vec!["tradeassembly://datasets/hosted-test".to_string()],
            first_attempt_id: "run-hosted-1:attempt:1".to_string(),
        },
        100,
    )
    .expect("manifest");
    (
        RunnerBridgeCommand {
            schema: CORE_RUNNER_BRIDGE_SCHEMA.to_string(),
            bridge_version: CORE_RUNNER_BRIDGE_VERSION.to_string(),
            core_release: CORE_RELEASE.to_string(),
            workspace_id: "workspace-1".to_string(),
            command_id: "command-1".to_string(),
            authority_ref: "tradeassembly://authority/receipt-1".to_string(),
            idempotency_key: IdempotencyKey::new("runner-command-1").expect("idempotency"),
            workload: RunnerBridgeWorkload::HostedDeterministicBacktest,
            submitted_at_ms: manifest.created_at_ms,
            backtest_manifest: manifest,
        },
        target_file,
        target_path,
    )
}

#[test]
fn durable_bridge_executes_replays_and_reconciles_missing_receipt_after_restart() {
    let (command, _target_file, target_path) = command_from_source_manifest();
    let service = TradeAssemblyService::test_local(&target_path);
    let bridge =
        DurableCoreRunnerBridge::new(service.clone(), CORE_RELEASE).expect("runner bridge");
    let receipt = bridge.dispatch(&command).expect("dispatch");
    let run = service
        .runtime()
        .backtests
        .get_run("run-hosted-1")
        .expect("run read")
        .expect("run");
    assert_eq!(
        receipt.state,
        RunnerBridgeReceiptState::Completed,
        "receipt={receipt:#?} run={run:#?}"
    );
    assert_eq!(receipt.result_sha256, run.result_hash);
    assert_eq!(
        service.runtime().backtests.list_runs().expect("runs").len(),
        1
    );
    assert_eq!(bridge.dispatch(&command).expect("replay"), receipt);

    service
        .runtime()
        .storage
        .clear_namespace("core_runner_bridge_receipts_v1")
        .expect("simulate crash before receipt");
    drop(bridge);
    drop(service);
    let restarted = TradeAssemblyService::test_local(&target_path);
    let restarted_bridge =
        DurableCoreRunnerBridge::new(restarted.clone(), CORE_RELEASE).expect("restart bridge");
    assert_eq!(
        restarted_bridge.dispatch(&command).expect("reconcile"),
        receipt
    );
    assert_eq!(
        restarted
            .runtime()
            .backtests
            .list_runs()
            .expect("restart runs")
            .len(),
        1
    );
}

#[test]
fn altered_replay_and_workspace_crossing_fail_closed() {
    let (command, _target_file, target_path) = command_from_source_manifest();
    let service = TradeAssemblyService::test_local(&target_path);
    let bridge = DurableCoreRunnerBridge::new(service, CORE_RELEASE).expect("runner bridge");
    bridge.dispatch(&command).expect("dispatch");

    let mut altered = command.clone();
    altered.submitted_at_ms += 1;
    let altered_error = bridge.dispatch(&altered).expect_err("altered replay");
    assert_eq!(
        altered_error.code,
        RunnerBridgeErrorCode::IdempotencyConflict,
        "{altered_error:#?}"
    );
    assert!(bridge
        .receipt("workspace-2", &command.command_id)
        .expect("cross-workspace lookup")
        .is_none());

    let mut reused_idempotency = command.clone();
    reused_idempotency.command_id = "command-2".to_string();
    let reused_error = bridge
        .dispatch(&reused_idempotency)
        .expect_err("reused idempotency");
    assert_eq!(
        reused_error.code,
        RunnerBridgeErrorCode::IdempotencyConflict,
        "{reused_error:#?}"
    );
}

#[test]
fn preexisting_run_id_cannot_be_substituted_into_another_command() {
    let (command, _target_file, target_path) = command_from_source_manifest();
    let service = TradeAssemblyService::test_local(&target_path);
    let bridge =
        DurableCoreRunnerBridge::new(service.clone(), CORE_RELEASE).expect("runner bridge");
    bridge.dispatch(&command).expect("first dispatch");

    let mut substituted = command.clone();
    substituted.workspace_id = "workspace-2".to_string();
    substituted.command_id = "command-2".to_string();
    substituted.idempotency_key =
        IdempotencyKey::new("runner-command-2").expect("second idempotency");
    let mut content = substituted.backtest_manifest.content.clone();
    content.client = "different-client".to_string();
    substituted.backtest_manifest =
        BacktestRunManifest::build(content, substituted.backtest_manifest.created_at_ms)
            .expect("substituted manifest");
    let error = bridge
        .dispatch(&substituted)
        .expect_err("run substitution must fail");
    assert_eq!(error.code, RunnerBridgeErrorCode::ReceiptMismatch);
    assert!(bridge
        .receipt(&substituted.workspace_id, &substituted.command_id)
        .is_err());
    assert_eq!(
        service.runtime().backtests.list_runs().expect("runs").len(),
        1
    );
}

#[test]
fn credential_shaped_top_level_identifiers_fail_before_admission_without_echo() {
    let (mut command, _target_file, target_path) = command_from_source_manifest();
    let service = TradeAssemblyService::test_local(&target_path);
    let bridge = DurableCoreRunnerBridge::new(service, CORE_RELEASE).expect("runner bridge");
    let marker = ["sk", "not-a-real-credential"].join("-");
    command.idempotency_key = IdempotencyKey::new(&marker).expect("shaped idempotency");
    let error = bridge
        .dispatch(&command)
        .expect_err("credential-shaped top-level value");
    assert_eq!(
        error.code,
        RunnerBridgeErrorCode::CredentialMaterialNotAllowed
    );
    assert!(!error.message.contains(&marker));
}

#[test]
fn concurrent_dispatch_is_duplicate_safe() {
    let (command, _target_file, target_path) = command_from_source_manifest();
    let service = TradeAssemblyService::test_local(&target_path);
    let bridge = Arc::new(
        DurableCoreRunnerBridge::new(service.clone(), CORE_RELEASE).expect("runner bridge"),
    );
    let barrier = Arc::new(Barrier::new(4));
    let workers = (0..4)
        .map(|_| {
            let bridge = Arc::clone(&bridge);
            let barrier = Arc::clone(&barrier);
            let command = command.clone();
            thread::spawn(move || {
                barrier.wait();
                bridge.dispatch(&command)
            })
        })
        .collect::<Vec<_>>();
    let receipts = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker").expect("dispatch"))
        .collect::<Vec<_>>();
    assert!(
        receipts.iter().all(|receipt| {
            receipt.receipt_id == receipts[0].receipt_id
                && receipt.command_sha256 == receipts[0].command_sha256
                && matches!(
                    receipt.state,
                    RunnerBridgeReceiptState::Accepted | RunnerBridgeReceiptState::Completed
                )
        }),
        "{receipts:#?}"
    );
    assert_eq!(
        service.runtime().backtests.list_runs().expect("runs").len(),
        1
    );
    let durable = bridge
        .receipt(&command.workspace_id, &command.command_id)
        .expect("receipt")
        .expect("durable receipt");
    assert_eq!(durable.receipt_id, receipts[0].receipt_id);
    assert_eq!(durable.command_sha256, receipts[0].command_sha256);
    assert!(matches!(
        durable.state,
        RunnerBridgeReceiptState::Accepted | RunnerBridgeReceiptState::Completed
    ));
}

#[test]
fn corrupt_durable_command_and_receipt_records_fail_closed() {
    let (command, _target_file, target_path) = command_from_source_manifest();
    let service = TradeAssemblyService::test_local(&target_path);
    let bridge =
        DurableCoreRunnerBridge::new(service.clone(), CORE_RELEASE).expect("runner bridge");
    bridge.dispatch(&command).expect("dispatch");
    let key = format!(
        "command-{:x}",
        Sha256::digest(format!("{}\0{}", command.workspace_id, command.command_id))
    );

    service
        .runtime()
        .storage
        .clear_namespace("core_runner_bridge_commands_v1")
        .expect("clear command");
    service
        .runtime()
        .storage
        .put_json(
            "core_runner_bridge_commands_v1",
            &key,
            json!({"schema": "corrupt"}),
            &context("corrupt-command"),
        )
        .expect("corrupt command stored");
    let command_error = bridge
        .receipt(&command.workspace_id, &command.command_id)
        .expect_err("corrupt command");
    assert_eq!(command_error.code, RunnerBridgeErrorCode::AdapterFailure);

    let (command, _target_file, target_path) = command_from_source_manifest();
    let service = TradeAssemblyService::test_local(&target_path);
    let bridge =
        DurableCoreRunnerBridge::new(service.clone(), CORE_RELEASE).expect("runner bridge");
    bridge.dispatch(&command).expect("dispatch");
    service
        .runtime()
        .storage
        .clear_namespace("core_runner_bridge_receipts_v1")
        .expect("clear receipt");
    service
        .runtime()
        .storage
        .put_json(
            "core_runner_bridge_receipts_v1",
            &key,
            json!({"schema": "corrupt"}),
            &context("corrupt-receipt"),
        )
        .expect("corrupt receipt stored");
    let receipt_error = bridge
        .receipt(&command.workspace_id, &command.command_id)
        .expect_err("corrupt receipt");
    assert_eq!(receipt_error.code, RunnerBridgeErrorCode::AdapterFailure);
}

#[test]
fn process_protocol_is_strict_bounded_and_does_not_echo_input() {
    let db = NamedTempFile::new().expect("protocol db");
    let process_root = tempfile::tempdir().expect("process root");
    fs::create_dir_all(process_root.path().join(".tradeassembly"))
        .expect("TradeAssembly directory");
    fs::write(
        process_root.path().join(".tradeassembly/warden.token"),
        "test-only-runner-token-at-least-32-bytes",
    )
    .expect("Warden token");
    let mut child = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .args([
            "--db",
            db.path().to_str().expect("db path"),
            "--core-release",
            CORE_RELEASE,
        ])
        .current_dir(process_root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("runner process");
    let mut stdin = child.stdin.take().expect("stdin");
    writeln!(
        stdin,
        "{}",
        json!({
            "operation": "receipt",
            "workspace_id": "workspace-1",
            "command_id": "missing",
            "unknown": "must-not-survive"
        })
    )
    .expect("strict line");
    writeln!(
        stdin,
        "{}",
        json!({
            "operation": "receipt",
            "workspace_id": "workspace-1",
            "command_id": "missing"
        })
    )
    .expect("valid line");
    writeln!(stdin, "{}", "x".repeat(301 * 1024)).expect("oversized line");
    drop(stdin);
    let output = child.wait_with_output().expect("runner output");
    assert!(output.status.success());
    let lines = String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("response JSON"))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["error"]["code"], "malformed_command");
    assert_eq!(lines[1], json!({"ok": true, "receipt": Value::Null}));
    assert_eq!(lines[2]["error"]["code"], "payload_out_of_bounds");
    assert!(!serde_json::to_string(&lines)
        .expect("responses")
        .contains("must-not-survive"));
}

#[test]
fn process_configured_mode_rejects_ambiguous_relative_and_non_self_hosted_inputs() {
    let process_root = tempfile::tempdir().expect("process root");
    let relative = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .args([
            "--runtime-config",
            "runtime.json",
            "--core-release",
            CORE_RELEASE,
        ])
        .current_dir(process_root.path())
        .output()
        .expect("relative config process");
    assert_eq!(relative.status.code(), Some(2));
    assert_generic_adapter_failure(&relative.stdout);

    let local_config = process_root.path().join("local.json");
    fs::write(
        &local_config,
        serde_json::to_vec(&json!({"profile": "local"})).expect("local config JSON"),
    )
    .expect("local config");
    let local = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .args([
            "--runtime-config",
            local_config.to_str().expect("config path"),
            "--core-release",
            CORE_RELEASE,
        ])
        .current_dir(process_root.path())
        .output()
        .expect("local configured process");
    assert_eq!(local.status.code(), Some(2));
    assert_generic_adapter_failure(&local.stdout);

    let mixed = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .args([
            "--db",
            "/tmp/tradeassembly-runner-test.db",
            "--runtime-config",
            local_config.to_str().expect("config path"),
            "--core-release",
            CORE_RELEASE,
        ])
        .output()
        .expect("mixed mode process");
    assert!(!mixed.status.success());
}

#[test]
fn process_configured_mode_rejects_unknown_fields_without_echoing_values() {
    let process_root = tempfile::tempdir().expect("process root");
    let config = process_root.path().join("runtime.json");
    let marker = "must-not-echo-runtime-config-value";
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "profile": "self_hosted",
            "inlineSecret": marker
        }))
        .expect("invalid config JSON"),
    )
    .expect("invalid config");
    let output = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .args([
            "--runtime-config",
            config.to_str().expect("config path"),
            "--core-release",
            CORE_RELEASE,
        ])
        .output()
        .expect("invalid config process");
    assert_eq!(output.status.code(), Some(2));
    assert_generic_adapter_failure(&output.stdout);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(marker));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(marker));
}

fn assert_generic_adapter_failure(stdout: &[u8]) {
    let response: Value = serde_json::from_slice(stdout).expect("runner error response");
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "adapter_failure");
    assert_eq!(
        response["error"]["message"],
        "runner bridge runtime configuration is invalid"
    );
}

struct ManagedRunnerSecrets {
    postgres_url: String,
    nats_url: String,
}

impl SecretResolver for ManagedRunnerSecrets {
    fn resolve(&self, reference: &str) -> Result<String, String> {
        match reference {
            "env://TRADEASSEMBLY_TEST_POSTGRES_URL" => Ok(self.postgres_url.clone()),
            "env://TRADEASSEMBLY_TEST_NATS_URL" => Ok(self.nats_url.clone()),
            "env://TRADEASSEMBLY_TEST_WARDEN_TOKEN" => {
                Ok("test-only-managed-runner-warden-token".to_string())
            }
            _ => Err("test secret reference is unsupported".to_string()),
        }
    }
}

#[test]
#[ignore = "requires Docker-backed Postgres and NATS JetStream"]
fn configured_process_is_duplicate_safe_and_recovers_across_replacement() {
    let postgres_url = std::env::var("TRADEASSEMBLY_TEST_POSTGRES_URL")
        .expect("TRADEASSEMBLY_TEST_POSTGRES_URL is required");
    let nats_url = std::env::var("TRADEASSEMBLY_TEST_NATS_URL")
        .expect("TRADEASSEMBLY_TEST_NATS_URL is required");
    let process_root = tempfile::tempdir().expect("process root");
    let artifact_root = process_root.path().join("artifacts");
    fs::create_dir_all(&artifact_root).expect("artifact root");
    let config_path = process_root.path().join("runtime.json");
    let config_layer = RuntimeConfigLayer {
        profile: Some("self_hosted".to_string()),
        oidc_profile: Some("oidc_test".to_string()),
        oidc_issuer: Some("http://127.0.0.1:18080/realms/tradeassembly-test".to_string()),
        oidc_audience: Some("tradeassembly-managed-test".to_string()),
        oidc_client_id: Some("tradeassembly-managed-test".to_string()),
        oidc_redirect_uri: Some("http://127.0.0.1:18976/callback".to_string()),
        postgres_url_ref: Some("env://TRADEASSEMBLY_TEST_POSTGRES_URL".to_string()),
        nats_url_ref: Some("env://TRADEASSEMBLY_TEST_NATS_URL".to_string()),
        object_store_endpoint: Some(format!("file://{}", artifact_root.display())),
        warden_token_ref: Some("env://TRADEASSEMBLY_TEST_WARDEN_TOKEN".to_string()),
        telemetry_enabled: Some(false),
        ..RuntimeConfigLayer::default()
    };
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config_layer).expect("runtime config JSON"),
    )
    .expect("runtime config");
    let config = RuntimeConfig::resolve(
        Some(config_layer),
        RuntimeConfigLayer::default(),
        RuntimeConfigLayer::default(),
    )
    .expect("self-hosted config");
    let (runtime, manifest) = RuntimeBuilder::new(config)
        .with_secret_resolver(Arc::new(ManagedRunnerSecrets {
            postgres_url: postgres_url.clone(),
            nats_url: nats_url.clone(),
        }))
        .build()
        .expect("self-hosted runtime");
    assert_eq!(manifest.profile, "self_hosted");
    assert!(manifest
        .adapters
        .iter()
        .any(|adapter| adapter.adapter_id == "self_hosted.postgres"));
    assert!(manifest
        .adapters
        .iter()
        .any(|adapter| adapter.adapter_id == "self_hosted.nats-jetstream"));
    assert!(!manifest
        .adapters
        .iter()
        .any(|adapter| adapter.adapter_id.contains("sqlite")));

    let service = TradeAssemblyService::from_runtime("self_hosted", runtime);
    seed(&service, "managed-target-seed");
    drop(service);
    let (command, _target_file, _target_path) = command_from_source_manifest();

    let first = run_configured_process(
        process_root.path(),
        &config_path,
        &postgres_url,
        &nats_url,
        &[json!({"operation": "dispatch", "command": command})],
    );
    assert!(
        first.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_rows = response_rows(&first.stdout);
    assert_eq!(first_rows.len(), 1);
    assert_eq!(first_rows[0]["ok"], true);
    assert_eq!(first_rows[0]["receipt"]["state"], "completed");

    let second = run_configured_process(
        process_root.path(),
        &config_path,
        &postgres_url,
        &nats_url,
        &[
            json!({"operation": "dispatch", "command": command}),
            json!({
                "operation": "receipt",
                "workspace_id": command.workspace_id,
                "command_id": command.command_id
            }),
        ],
    );
    assert!(
        second.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_rows = response_rows(&second.stdout);
    assert_eq!(second_rows.len(), 2);
    assert_eq!(second_rows[0]["receipt"], first_rows[0]["receipt"]);
    assert_eq!(second_rows[1]["receipt"], first_rows[0]["receipt"]);

    let mut altered = command;
    altered.submitted_at_ms += 1;
    let conflict = run_configured_process(
        process_root.path(),
        &config_path,
        &postgres_url,
        &nats_url,
        &[json!({"operation": "dispatch", "command": altered})],
    );
    assert!(conflict.status.success());
    let conflict_rows = response_rows(&conflict.stdout);
    assert_eq!(conflict_rows[0]["ok"], false);
    assert_eq!(conflict_rows[0]["error"]["code"], "idempotency_conflict");
}

fn run_configured_process(
    working_directory: &Path,
    config_path: &Path,
    postgres_url: &str,
    nats_url: &str,
    requests: &[Value],
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"));
    child
        .args([
            "--runtime-config",
            config_path.to_str().expect("config path"),
            "--core-release",
            CORE_RELEASE,
        ])
        .current_dir(working_directory)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("TRADEASSEMBLY_TEST_POSTGRES_URL", postgres_url)
        .env("TRADEASSEMBLY_TEST_NATS_URL", nats_url)
        .env(
            "TRADEASSEMBLY_TEST_WARDEN_TOKEN",
            "test-only-managed-runner-warden-token",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = child.spawn().expect("configured runner process");
    {
        let input = child.stdin.as_mut().expect("runner stdin");
        for request in requests {
            writeln!(input, "{request}").expect("runner request");
        }
    }
    drop(child.stdin.take());
    child.wait_with_output().expect("configured runner output")
}

fn response_rows(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8(stdout.to_vec())
        .expect("runner UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("runner response JSON"))
        .collect()
}
