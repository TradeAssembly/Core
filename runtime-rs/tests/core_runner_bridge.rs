use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use tradeassembly_runtime::backtest_contracts::*;
use tradeassembly_runtime::historical_data::DatasetTimeSlice;
use tradeassembly_runtime::ports::{
    verify_core_runner_bridge, AuthorityContext, CoreRunnerBridgePort, IdempotencyKey,
    PortDescriptor, PortKind, RunnerBridgeCommand, RunnerBridgeError, RunnerBridgeErrorCode,
    RunnerBridgeReceipt, RunnerBridgeWorkload, VersionedPort, CORE_RUNNER_BRIDGE_SCHEMA,
    CORE_RUNNER_BRIDGE_VERSION,
};

const CORE_RELEASE: &str = "core-test-release";

#[derive(Default)]
struct ReferenceBridge {
    entries: Mutex<HashMap<(String, String), (String, RunnerBridgeReceipt)>>,
    receipts: Mutex<HashMap<(String, String), RunnerBridgeReceipt>>,
    executions: Mutex<usize>,
}

impl ReferenceBridge {
    fn executions(&self) -> usize {
        *self.executions.lock().expect("execution lock")
    }
}

impl VersionedPort for ReferenceBridge {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::CoreRunnerBridge, "reference-runner-bridge")
                .for_profiles(&["test"])
                .with_capabilities(&["hosted_deterministic_backtest"]),
        ]
    }
}

impl CoreRunnerBridgePort for ReferenceBridge {
    fn dispatch(
        &self,
        command: &RunnerBridgeCommand,
    ) -> Result<RunnerBridgeReceipt, RunnerBridgeError> {
        command.validate(CORE_RELEASE)?;
        let digest = command.canonical_hash()?;
        let key = (
            command.workspace_id.clone(),
            command.idempotency_key.as_str().to_string(),
        );
        let mut entries = self.entries.lock().map_err(|_| RunnerBridgeError {
            code: RunnerBridgeErrorCode::AdapterFailure,
            message: "runner bridge state is unavailable".to_string(),
        })?;
        if let Some((existing_digest, receipt)) = entries.get(&key) {
            if existing_digest == &digest {
                return Ok(receipt.clone());
            }
            return Err(RunnerBridgeError {
                code: RunnerBridgeErrorCode::IdempotencyConflict,
                message: "runner bridge idempotency key was reused".to_string(),
            });
        }
        *self.executions.lock().expect("execution lock") += 1;
        let receipt = RunnerBridgeReceipt::accepted(
            command,
            CORE_RELEASE,
            format!(
                "receipt-{}",
                command.command_id.trim_start_matches("command-")
            ),
            command.submitted_at_ms + 1,
        )?;
        entries.insert(key, (digest, receipt.clone()));
        self.receipts.lock().expect("receipt lock").insert(
            (command.workspace_id.clone(), command.command_id.clone()),
            receipt.clone(),
        );
        Ok(receipt)
    }

    fn receipt(
        &self,
        workspace_id: &str,
        command_id: &str,
    ) -> Result<Option<RunnerBridgeReceipt>, RunnerBridgeError> {
        Ok(self
            .receipts
            .lock()
            .map_err(|_| RunnerBridgeError {
                code: RunnerBridgeErrorCode::AdapterFailure,
                message: "runner bridge state is unavailable".to_string(),
            })?
            .get(&(workspace_id.to_string(), command_id.to_string()))
            .cloned())
    }
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

fn command() -> RunnerBridgeCommand {
    let configuration = configuration();
    let manifest = BacktestRunManifest::build(
        BacktestRunManifestContent {
            schema: BACKTEST_MANIFEST_SCHEMA.to_string(),
            run_id: "run-hosted-1".to_string(),
            request_hash: configuration.canonical_hash().expect("config hash"),
            idempotency_key: "backtest-create-1".to_string(),
            validation: configuration.validate(),
            configuration,
            authority: AuthorityContext {
                actor: "tradeassembly://identity/user".to_string(),
                surface: "studio_personal".to_string(),
                account_mode: "research".to_string(),
            },
            client: "self".to_string(),
            purpose: "strategy_backtest_research".to_string(),
            evidence_refs: vec!["tradeassembly://datasets/dataset_fixture".to_string()],
            first_attempt_id: "attempt-hosted-1".to_string(),
        },
        100,
    )
    .expect("manifest");
    RunnerBridgeCommand {
        schema: CORE_RUNNER_BRIDGE_SCHEMA.to_string(),
        bridge_version: CORE_RUNNER_BRIDGE_VERSION.to_string(),
        core_release: CORE_RELEASE.to_string(),
        workspace_id: "workspace-1".to_string(),
        command_id: "command-1".to_string(),
        authority_ref: "tradeassembly://authority/receipt-1".to_string(),
        idempotency_key: IdempotencyKey::new("runner-command-1").expect("idempotency"),
        workload: RunnerBridgeWorkload::HostedDeterministicBacktest,
        submitted_at_ms: 200,
        backtest_manifest: manifest,
    }
}

#[test]
fn conformance_enforces_replay_conflict_and_workspace_scope() {
    let bridge = ReferenceBridge::default();
    let report = verify_core_runner_bridge(&bridge, &command(), CORE_RELEASE).expect("conformance");
    assert_eq!(
        report.suite_id,
        "tradeassembly.core-runner-bridge.conformance.v1"
    );
    assert_eq!(bridge.executions(), 2);
}

#[test]
fn strict_json_release_and_receipt_bindings_fail_closed() {
    let command = command();
    let bytes = serde_json::to_vec(&command).expect("command json");
    assert_eq!(
        RunnerBridgeCommand::parse_json(&bytes, CORE_RELEASE).expect("strict command"),
        command
    );

    let mut unknown = serde_json::to_value(&command).expect("command value");
    unknown["unknownField"] = json!(true);
    assert_eq!(
        RunnerBridgeCommand::parse_json(
            &serde_json::to_vec(&unknown).expect("unknown json"),
            CORE_RELEASE
        )
        .expect_err("unknown field")
        .code,
        RunnerBridgeErrorCode::MalformedCommand
    );

    let mut nested_unknown = serde_json::to_value(&command).expect("command value");
    nested_unknown["backtestManifest"]["content"]["authority"]["hostedCredentialRef"] =
        json!("credential://must-not-be-discarded");
    assert_eq!(
        RunnerBridgeCommand::parse_json(
            &serde_json::to_vec(&nested_unknown).expect("nested unknown json"),
            CORE_RELEASE
        )
        .expect_err("nested unknown authority field")
        .code,
        RunnerBridgeErrorCode::MalformedCommand
    );
    assert_eq!(
        command
            .validate("different-release")
            .expect_err("release mismatch")
            .code,
        RunnerBridgeErrorCode::CoreReleaseMismatch
    );

    let mut receipt =
        RunnerBridgeReceipt::accepted(&command, CORE_RELEASE, "receipt-1", 201).expect("receipt");
    receipt.workspace_id = "workspace-2".to_string();
    assert_eq!(
        receipt
            .verify(&command, CORE_RELEASE)
            .expect_err("workspace mutation")
            .code,
        RunnerBridgeErrorCode::ReceiptMismatch
    );
}

#[test]
fn live_credentials_and_unbounded_payloads_are_rejected_without_echo() {
    let mut live = command();
    live.backtest_manifest.content.authority.account_mode = "live".to_string();
    let error = live.validate(CORE_RELEASE).expect_err("live rejection");
    assert_eq!(error.code, RunnerBridgeErrorCode::LiveModeNotAllowed);

    let mut credential = command();
    credential
        .backtest_manifest
        .content
        .configuration
        .variable_overrides
        .insert("apiKey".to_string(), json!("DO_NOT_ECHO"));
    let error = credential
        .validate(CORE_RELEASE)
        .expect_err("credential rejection");
    assert_eq!(
        error.code,
        RunnerBridgeErrorCode::CredentialMaterialNotAllowed
    );
    assert!(!format!("{error:?}").contains("DO_NOT_ECHO"));

    let mut oversized = command();
    oversized
        .backtest_manifest
        .content
        .configuration
        .variable_overrides
        .insert("parameter".to_string(), json!("x".repeat(17 * 1024)));
    assert_eq!(
        oversized
            .validate(CORE_RELEASE)
            .expect_err("oversized string")
            .code,
        RunnerBridgeErrorCode::PayloadOutOfBounds
    );
}

#[test]
fn safe_variable_overrides_remain_supported_and_semantically_bound() {
    let mut command = command();
    command
        .backtest_manifest
        .content
        .configuration
        .variable_overrides
        .insert("entryThreshold".to_string(), json!(42));
    let rebuilt = BacktestRunManifest::build(
        command.backtest_manifest.content.clone(),
        command.backtest_manifest.created_at_ms,
    )
    .expect("rebuilt manifest");
    command.backtest_manifest = rebuilt;
    command.validate(CORE_RELEASE).expect("safe override");
    let first = command.canonical_hash().expect("first hash");
    command
        .backtest_manifest
        .content
        .configuration
        .variable_overrides
        .insert("entryThreshold".to_string(), json!(43));
    let rebuilt = BacktestRunManifest::build(
        command.backtest_manifest.content.clone(),
        command.backtest_manifest.created_at_ms,
    )
    .expect("rebuilt manifest");
    command.backtest_manifest = rebuilt;
    assert_ne!(first, command.canonical_hash().expect("changed hash"));
}
