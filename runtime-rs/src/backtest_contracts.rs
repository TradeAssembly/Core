// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::domain::BacktestRunState;
use crate::historical_data::DatasetTimeSlice;
use crate::ports::AuthorityContext;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const BACKTEST_CONFIGURATION_SCHEMA: &str = "tradeassembly.backtest-configuration.v1";
pub const BACKTEST_MANIFEST_SCHEMA: &str = "tradeassembly.backtest-run-manifest.v1";
pub const BACKTEST_RESULT_SCHEMA: &str = "tradeassembly.backtest-result.v1";
pub const BACKTEST_RUN_SCHEMA: &str = "tradeassembly.backtest-run.v2";
pub const BACKTEST_ATTEMPT_SCHEMA: &str = "tradeassembly.backtest-attempt.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountModel {
    Cash,
    Margin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationTiming {
    BarClose,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FillTiming {
    SameBar,
    NextBar,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    Open,
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FillPolicy {
    Full,
    ParticipationCappedPartial,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MissingDataPolicy {
    Reject,
    SkipObservation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndOfDataPolicy {
    MarkToMarket,
    ForceClose,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BacktestOrderType {
    Market,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StrategyVersionBinding {
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub strategy_spec_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetBinding {
    pub dataset_id: String,
    pub snapshot_ref: String,
    pub content_hash: String,
    pub schema: String,
    pub source_plugin_ref: String,
    pub source_plugin_instance_ref: String,
    pub source_operation_id: String,
    pub source_manifest_fingerprint: String,
    pub source_class: String,
    pub capability_graph_revision_id: String,
    pub capability_graph_fingerprint: String,
    pub instruments: Vec<String>,
    pub selected_time_slice: DatasetTimeSlice,
    pub granularity: String,
    pub calendar: String,
    pub timezone: String,
    pub corporate_action_policy: String,
    pub benchmark: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGraphBinding {
    pub revision_id: String,
    pub graph_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapitalConfiguration {
    pub starting_cash_micros: i64,
    pub reporting_currency: String,
    pub account_model: AccountModel,
    pub maximum_leverage_micros: i64,
    pub margin_policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionConfiguration {
    pub evaluation_timing: EvaluationTiming,
    pub fill_timing: FillTiming,
    pub fill_price_source: PriceSource,
    pub order_type: BacktestOrderType,
    pub time_in_force: String,
    pub fill_policy: FillPolicy,
    pub reject_on_insufficient_capital: bool,
    pub reject_on_insufficient_liquidity: bool,
    pub stale_data_policy: MissingDataPolicy,
    pub missing_data_policy: MissingDataPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CostConfiguration {
    pub fixed_fee_micros: i64,
    pub per_unit_fee_micros: i64,
    pub notional_fee_bps: u32,
    pub spread_bps: u32,
    pub slippage_bps: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiquidityConfiguration {
    pub maximum_participation_bps: u32,
    pub minimum_volume_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstrumentConfiguration {
    pub instrument_id: String,
    pub instrument_family: String,
    pub quantity_unit: String,
    pub quantity_scale: u32,
    pub price_scale: u32,
    pub contract_multiplier_micros: i64,
    pub settlement: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestRiskConfiguration {
    pub maximum_position_notional_micros: i64,
    pub maximum_order_quantity_micros: i64,
    pub maximum_loss_micros: i64,
    pub maximum_open_positions: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestConfiguration {
    pub schema: String,
    pub strategy: StrategyVersionBinding,
    pub dataset: DatasetBinding,
    pub capability_graph: CapabilityGraphBinding,
    pub capital: CapitalConfiguration,
    pub execution: ExecutionConfiguration,
    pub costs: CostConfiguration,
    pub liquidity: LiquidityConfiguration,
    pub instruments: Vec<InstrumentConfiguration>,
    pub risk: BacktestRiskConfiguration,
    pub end_of_data: EndOfDataPolicy,
    pub evaluator_version: String,
    pub compiler_version: String,
    #[serde(default)]
    pub plugin_fingerprints: BTreeMap<String, String>,
    #[serde(default)]
    pub variable_overrides: BTreeMap<String, serde_json::Value>,
}

impl BacktestConfiguration {
    pub fn validate(&self) -> BacktestValidationReport {
        let mut diagnostics = Vec::new();
        let mut push_error = |field: String, detail: String| {
            diagnostics.push(BacktestDiagnostic {
                code: "backtest_config_invalid".to_string(),
                field: Some(field),
                severity: DiagnosticSeverity::Error,
                detail,
            });
        };
        if self.schema != BACKTEST_CONFIGURATION_SCHEMA {
            push_error("/schema".to_string(), "unsupported schema".to_string());
        }
        for (field, value) in [
            ("/strategy/strategyId", self.strategy.strategy_id.as_str()),
            (
                "/strategy/strategyVersionId",
                self.strategy.strategy_version_id.as_str(),
            ),
            (
                "/strategy/strategySpecHash",
                self.strategy.strategy_spec_hash.as_str(),
            ),
            ("/dataset/datasetId", self.dataset.dataset_id.as_str()),
            ("/dataset/contentHash", self.dataset.content_hash.as_str()),
            (
                "/dataset/capabilityGraphRevisionId",
                self.dataset.capability_graph_revision_id.as_str(),
            ),
            (
                "/dataset/capabilityGraphFingerprint",
                self.dataset.capability_graph_fingerprint.as_str(),
            ),
            (
                "/capabilityGraph/revisionId",
                self.capability_graph.revision_id.as_str(),
            ),
            (
                "/capabilityGraph/graphFingerprint",
                self.capability_graph.graph_fingerprint.as_str(),
            ),
            (
                "/capital/reportingCurrency",
                self.capital.reporting_currency.as_str(),
            ),
            ("/evaluatorVersion", self.evaluator_version.as_str()),
            ("/compilerVersion", self.compiler_version.as_str()),
        ] {
            if value.trim().is_empty() {
                push_error(field.to_string(), "value is required".to_string());
            }
        }
        if self.dataset.instruments.is_empty() || self.instruments.is_empty() {
            push_error(
                "/instruments".to_string(),
                "at least one instrument is required".to_string(),
            );
        }
        if self.dataset.selected_time_slice.start >= self.dataset.selected_time_slice.end {
            push_error(
                "/dataset/selectedTimeSlice".to_string(),
                "time slice must be increasing".to_string(),
            );
        }
        if self.capital.starting_cash_micros <= 0
            || self.capital.maximum_leverage_micros < 1_000_000
            || self.risk.maximum_position_notional_micros <= 0
            || self.risk.maximum_order_quantity_micros <= 0
            || self.risk.maximum_loss_micros <= 0
            || self.risk.maximum_open_positions == 0
        {
            push_error(
                "/capital".to_string(),
                "capital and risk limits must be positive".to_string(),
            );
        }
        if self.costs.fixed_fee_micros < 0
            || self.costs.per_unit_fee_micros < 0
            || self.costs.notional_fee_bps > 10_000
            || self.costs.spread_bps > 10_000
            || self.costs.slippage_bps > 10_000
            || self.liquidity.maximum_participation_bps == 0
            || self.liquidity.maximum_participation_bps > 10_000
            || self.liquidity.minimum_volume_micros < 0
        {
            push_error(
                "/execution".to_string(),
                "cost or liquidity value is outside supported bounds".to_string(),
            );
        }
        for (index, instrument) in self.instruments.iter().enumerate() {
            if instrument.instrument_id.trim().is_empty()
                || instrument.instrument_family.trim().is_empty()
                || instrument.quantity_unit.trim().is_empty()
                || instrument.contract_multiplier_micros <= 0
                || instrument.quantity_scale > 12
                || instrument.price_scale > 12
            {
                push_error(
                    format!("/instruments/{index}"),
                    "instrument configuration is incomplete".to_string(),
                );
            }
            if !self.dataset.instruments.contains(&instrument.instrument_id) {
                push_error(
                    format!("/instruments/{index}/instrumentId"),
                    "instrument is absent from dataset".to_string(),
                );
            }
        }
        BacktestValidationReport {
            ready: diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != DiagnosticSeverity::Error),
            diagnostics,
        }
    }

    pub fn canonical_hash(&self) -> Result<String, String> {
        canonical_hash(self, "backtest_config_serialization_failed")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestDiagnostic {
    pub code: String,
    pub field: Option<String>,
    pub severity: DiagnosticSeverity,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestValidationReport {
    pub ready: bool,
    pub diagnostics: Vec<BacktestDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestRunManifestContent {
    pub schema: String,
    pub run_id: String,
    pub request_hash: String,
    pub idempotency_key: String,
    pub configuration: BacktestConfiguration,
    pub validation: BacktestValidationReport,
    pub authority: AuthorityContext,
    pub client: String,
    pub purpose: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    pub first_attempt_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestRunManifest {
    pub manifest_hash: String,
    pub manifest_ref: String,
    pub byte_length: u64,
    pub created_at_ms: i64,
    pub content: BacktestRunManifestContent,
}

impl BacktestRunManifest {
    pub fn build(content: BacktestRunManifestContent, created_at_ms: i64) -> Result<Self, String> {
        if content.schema != BACKTEST_MANIFEST_SCHEMA
            || content.run_id.trim().is_empty()
            || content.request_hash.trim().is_empty()
            || content.idempotency_key.trim().is_empty()
            || content.client.trim().is_empty()
            || content.purpose.trim().is_empty()
            || content.first_attempt_id.trim().is_empty()
            || !content.validation.ready
            || !content.configuration.validate().ready
        {
            return Err("backtest_manifest_invalid".to_string());
        }
        let bytes = canonical_bytes(&content, "backtest_manifest_serialization_failed")?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        Ok(Self {
            manifest_ref: format!("tradeassembly://backtests/{}/manifest", content.run_id),
            manifest_hash: format!("sha256:{digest}"),
            byte_length: bytes.len() as u64,
            created_at_ms,
            content,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        let rebuilt = Self::build(self.content.clone(), self.created_at_ms)
            .map_err(|_| "backtest_manifest_integrity_failed".to_string())?;
        if rebuilt.manifest_hash != self.manifest_hash
            || rebuilt.manifest_ref != self.manifest_ref
            || rebuilt.byte_length != self.byte_length
        {
            return Err("backtest_manifest_integrity_failed".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestRunRecord {
    pub schema: String,
    pub run_id: String,
    pub manifest_hash: String,
    pub request_hash: String,
    pub idempotency_key: String,
    pub state: BacktestRunState,
    pub sequence: i64,
    pub current_attempt_id: String,
    #[serde(default)]
    pub fencing_token: i64,
    pub result_hash: Option<String>,
    pub result: Option<BacktestResult>,
    pub failure_code: Option<String>,
    #[serde(default)]
    pub events: Vec<BacktestLifecycleEvent>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktestAttemptState {
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestAttempt {
    pub schema: String,
    pub attempt_id: String,
    pub run_id: String,
    pub ordinal: u32,
    pub state: BacktestAttemptState,
    pub stage: String,
    pub progress_bps: u32,
    pub queue_message_id: Option<String>,
    pub lease_owner: Option<String>,
    pub fencing_token: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub failure_code: Option<String>,
    #[serde(default)]
    pub diagnostics: Vec<BacktestDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestLifecycleEvent {
    pub event_id: String,
    pub run_id: String,
    pub sequence: i64,
    pub event_type: String,
    pub from_state: BacktestRunState,
    pub to_state: BacktestRunState,
    pub attempt_id: String,
    pub occurred_at_ms: i64,
    pub previous_event_hash: Option<String>,
    pub event_hash: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestResultContent {
    pub schema: String,
    pub run_id: String,
    pub manifest_hash: String,
    pub evaluator_version: String,
    pub compiler_version: String,
    pub decisions: Vec<serde_json::Value>,
    pub orders: Vec<serde_json::Value>,
    pub fills: Vec<serde_json::Value>,
    pub positions: Vec<serde_json::Value>,
    pub ledger: Vec<serde_json::Value>,
    pub accounting: serde_json::Value,
    #[serde(default)]
    pub diagnostics: Vec<BacktestDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestResult {
    pub result_hash: String,
    pub result_ref: String,
    pub byte_length: u64,
    pub created_at_ms: i64,
    pub content: BacktestResultContent,
}

impl BacktestResult {
    pub fn build(content: BacktestResultContent, created_at_ms: i64) -> Result<Self, String> {
        if content.schema != BACKTEST_RESULT_SCHEMA
            || content.run_id.trim().is_empty()
            || content.manifest_hash.trim().is_empty()
            || content.evaluator_version.trim().is_empty()
            || content.compiler_version.trim().is_empty()
        {
            return Err("backtest_result_invalid".to_string());
        }
        let bytes = canonical_bytes(&content, "backtest_result_serialization_failed")?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        Ok(Self {
            result_ref: format!("tradeassembly://backtests/{}/result", content.run_id),
            result_hash: format!("sha256:{digest}"),
            byte_length: bytes.len() as u64,
            created_at_ms,
            content,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        let rebuilt = Self::build(self.content.clone(), self.created_at_ms)
            .map_err(|_| "backtest_result_integrity_failed".to_string())?;
        if rebuilt.result_hash != self.result_hash
            || rebuilt.result_ref != self.result_ref
            || rebuilt.byte_length != self.byte_length
        {
            return Err("backtest_result_integrity_failed".to_string());
        }
        Ok(())
    }
}

pub fn canonical_hash<T: Serialize>(value: &T, error: &str) -> Result<String, String> {
    let bytes = canonical_bytes(value, error)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonical_bytes<T: Serialize>(value: &T, error: &str) -> Result<Vec<u8>, String> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| error.to_string())
}
