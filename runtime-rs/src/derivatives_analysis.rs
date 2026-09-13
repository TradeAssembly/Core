//! Pure, deterministic strategy-scoped derivatives analysis.

use crate::backtest_accounting::AccountingResult;
use crate::backtest_contracts::{canonical_hash, InstrumentConfiguration};
use crate::backtest_report::BacktestReport;
use crate::historical_data::{
    DatasetSnapshot, HistoricalObservationData, OptionContractObservation, OptionGreeks,
    OptionRight,
};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

pub const DERIVATIVES_ANALYSIS_SCHEMA: &str = "tradeassembly.derivatives-analysis.v1";
pub const DERIVATIVES_REQUEST_SCHEMA: &str = "tradeassembly.derivatives-analysis-request.v1";
pub const BLACK_SCHOLES_MERTON_MODEL: &str = "black_scholes_merton";
pub const BLACK_SCHOLES_MERTON_VERSION: &str = "1.0.0";
pub const MAX_LEGS: usize = 128;
pub const MAX_SCENARIOS: usize = 16;
pub const MAX_SENSITIVITY_AXIS_VALUES: usize = 16;
pub const MAX_SENSITIVITY_POINTS: usize = 256;
pub const MAX_PRICE_SHOCK_BPS: i32 = 10_000;
pub const MAX_VOLATILITY_SHIFT_PPM: i64 = 1_000_000;
pub const MAX_ELAPSED_SECONDS: i64 = 31_536_000;
pub const MAX_EVIDENCE_AGE_MS: i64 = 31_536_000_000;
const SCALE: i64 = 1_000_000;
const YEAR_SECONDS: f64 = 31_557_600.0;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceBindings {
    pub run_id: String,
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub strategy_spec_hash: String,
    pub manifest_hash: String,
    pub result_hash: String,
    pub report_hash: String,
    pub dataset_hash: String,
    pub accounting_hash: String,
    pub instrument_configurations_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    pub source_ref: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourcedPrice {
    pub price_micros: i64,
    pub observed_at_ms: i64,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentKind {
    Underlying,
    Option,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Open,
    Closed,
    Exercised,
    Assigned,
    Expired,
    Settled,
    TerminalValued,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleEvidence {
    pub state: LifecycleState,
    pub occurred_at_ms: Option<i64>,
    pub settlement_price_micros: Option<i64>,
    pub settlement_cashflow_micros: Option<i64>,
    pub terminal_value_micros: Option<i64>,
    #[serde(default)]
    pub evidence_refs: Vec<Provenance>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StrategyLeg {
    pub leg_id: String,
    pub position_group_id: String,
    pub instrument_id: String,
    pub underlying_instrument_id: String,
    pub instrument_kind: InstrumentKind,
    pub quantity_micros: i64,
    pub entry_price_micros: Option<i64>,
    #[serde(default)]
    pub report_trade_ids: Vec<String>,
    pub lifecycle: LifecycleEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioKind {
    Current,
    HypotheticalStress,
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclaredScenario {
    pub scenario_id: String,
    pub kind: ScenarioKind,
    #[serde(default)]
    pub underlying_prices_micros: BTreeMap<String, i64>,
    #[serde(default)]
    pub volatility_shifts_ppm: BTreeMap<String, i64>,
    pub elapsed_seconds: i64,
    pub declaration: Provenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SensitivityGrid {
    pub price_shocks_bps: Vec<i32>,
    pub volatility_shifts_ppm: Vec<i64>,
    pub elapsed_seconds: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionedPricingModel {
    pub model: String,
    pub version: String,
    pub risk_free_rate_ppm: Option<i64>,
    #[serde(default)]
    pub dividend_yields_ppm: BTreeMap<String, i64>,
    #[serde(default)]
    pub implied_volatilities_ppm: BTreeMap<String, i64>,
    #[serde(default)]
    pub spot_prices_micros: BTreeMap<String, i64>,
    #[serde(default)]
    pub time_to_expiry_seconds: BTreeMap<String, i64>,
    #[serde(default)]
    pub input_provenance: Vec<Provenance>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivativesAnalysisRequest {
    pub schema: String,
    pub engine_version: String,
    pub as_of_ms: i64,
    pub max_mark_age_ms: i64,
    pub max_greek_age_ms: i64,
    pub source_bindings: SourceBindings,
    pub legs: Vec<StrategyLeg>,
    #[serde(default)]
    pub source_marks: BTreeMap<String, SourcedPrice>,
    #[serde(default)]
    pub scenarios: Vec<DeclaredScenario>,
    pub sensitivity_grid: Option<SensitivityGrid>,
    pub pricing_model: Option<VersionedPricingModel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    MissingContractMetadata,
    MissingQuote,
    MissingGreeks,
    MissingImpliedVolatility,
    MissingRiskFreeRate,
    MissingDividendYield,
    MissingMultiplier,
    MissingSettlementFacts,
    MissingLifecycleFacts,
    MissingExplicitPricingModel,
    MissingModelSpot,
    MissingTimeToExpiry,
    MissingEntryPrice,
    MissingAccountingPosition,
    MissingReportTrade,
    StaleQuote,
    StaleGreeks,
    SourceFactMismatch,
    UnsupportedPricingModel,
    AccountingMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnavailableCalculation {
    pub scope: String,
    pub calculation: String,
    pub reason: UnavailableReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AvailableValue {
    Available {
        value_micros: i64,
        provenance: String,
    },
    Unavailable {
        reason: UnavailableReason,
    },
}

impl AvailableValue {
    fn value(&self) -> Option<i64> {
        match self {
            Self::Available { value_micros, .. } => Some(*value_micros),
            Self::Unavailable { .. } => None,
        }
    }

    fn provenance(&self) -> Option<&str> {
        match self {
            Self::Available { provenance, .. } => Some(provenance),
            Self::Unavailable { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractMetadata {
    pub contract_symbol: String,
    pub underlying_instrument_id: String,
    pub expiration: String,
    pub strike_micros: i64,
    pub right: OptionRight,
    pub multiplier_micros: i64,
    pub settlement: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GreekValuesPpm {
    pub delta: Option<i64>,
    pub gamma: Option<i64>,
    pub theta: Option<i64>,
    pub vega: Option<i64>,
    pub rho: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GreekEvidence {
    pub values_ppm: GreekValuesPpm,
    pub observed_at_ms: i64,
    pub age_ms: i64,
    pub stale: bool,
    pub source_ref: String,
    pub source_content_hash: String,
    pub source_plugin_ref: String,
    pub source_operation_id: String,
    pub pathway: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PnlBound {
    Finite(i64),
    UnboundedNegative,
    UnboundedPositive,
    Unavailable(UnavailableReason),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayoffRisk {
    pub minimum_pnl_micros: PnlBound,
    pub maximum_pnl_micros: PnlBound,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegAnalysis {
    pub leg_id: String,
    pub position_group_id: String,
    pub instrument_id: String,
    pub instrument_kind: InstrumentKind,
    pub quantity_micros: i64,
    pub contract: Option<ContractMetadata>,
    pub mark: AvailableValue,
    pub source_greeks: Option<GreekEvidence>,
    pub model_greeks: Option<GreekEvidence>,
    pub realized_pnl: AvailableValue,
    pub unrealized_pnl: AvailableValue,
    pub lifecycle: LifecycleEvidence,
    pub lifecycle_complete: bool,
    pub payoff_risk: PayoffRisk,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupAnalysis {
    pub position_group_id: String,
    pub leg_ids: Vec<String>,
    pub realized_pnl: AvailableValue,
    pub unrealized_pnl: AvailableValue,
    pub payoff_risk: PayoffRisk,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioLegValue {
    pub leg_id: String,
    pub value: AvailableValue,
    pub pnl: AvailableValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScenarioAnalysis {
    pub scenario_id: String,
    pub kind: ScenarioKind,
    pub declaration: Provenance,
    pub legs: Vec<ScenarioLegValue>,
    pub aggregate_value: AvailableValue,
    pub aggregate_pnl: AvailableValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SensitivityPoint {
    pub price_shock_bps: i32,
    pub volatility_shift_ppm: i64,
    pub elapsed_seconds: i64,
    pub aggregate_value: AvailableValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Reconciled,
    Mismatch,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountingReconciliation {
    pub status: ReconciliationStatus,
    pub analyzed_realized_pnl_micros: Option<i64>,
    pub analyzed_unrealized_pnl_micros: Option<i64>,
    pub accounting_realized_pnl_micros: i64,
    pub accounting_unrealized_pnl_micros: i64,
    pub report_realized_pnl_micros: i64,
    pub report_unrealized_pnl_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivativesAnalysis {
    pub schema: String,
    pub engine_version: String,
    pub source_bindings: SourceBindings,
    pub pricing_model_hash: Option<String>,
    pub legs: Vec<LegAnalysis>,
    pub groups: Vec<GroupAnalysis>,
    pub scenarios: Vec<ScenarioAnalysis>,
    pub sensitivity: Vec<SensitivityPoint>,
    pub accounting_reconciliation: AccountingReconciliation,
    pub unavailable: Vec<UnavailableCalculation>,
    pub output_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnalysisError {
    InvalidRequest(&'static str),
    IntegrityFailure(&'static str),
    LimitExceeded(&'static str),
    ArithmeticOverflow,
    SerializationFailure,
}

pub fn analyze(
    request: &DerivativesAnalysisRequest,
    snapshot: &DatasetSnapshot,
    instruments: &[InstrumentConfiguration],
    accounting: &AccountingResult,
    report: &BacktestReport,
) -> Result<DerivativesAnalysis, AnalysisError> {
    validate_request(request)?;
    verify_sources(request, snapshot, instruments, accounting, report)?;

    let instrument_map = instruments
        .iter()
        .map(|instrument| (instrument.instrument_id.as_str(), instrument))
        .collect::<BTreeMap<_, _>>();
    let observations = latest_option_observations(snapshot, request.as_of_ms)?;
    verify_leg_source_facts(request, &instrument_map, &observations)?;
    let mut unavailable = Vec::new();
    let mut legs = Vec::with_capacity(request.legs.len());

    for leg in sorted_legs(&request.legs) {
        let observation = observations.get(leg.instrument_id.as_str()).copied();
        let configuration = instrument_map.get(leg.instrument_id.as_str()).copied();
        legs.push(analyze_leg(
            leg,
            observation,
            configuration,
            request,
            snapshot,
            accounting,
            report,
            &mut unavailable,
        )?);
    }

    let groups = analyze_groups(&request.legs, &legs)?;
    let scenarios = sorted_scenarios(&request.scenarios)
        .into_iter()
        .map(|scenario| analyze_scenario(scenario, &request.legs, &legs, request, &mut unavailable))
        .collect::<Result<Vec<_>, _>>()?;
    let sensitivity = analyze_sensitivity(request, &request.legs, &legs, &mut unavailable)?;
    let accounting_reconciliation =
        reconcile_accounting(&legs, accounting, report, &mut unavailable)?;
    unavailable.sort_by(|left, right| {
        (&left.scope, &left.calculation, left.reason).cmp(&(
            &right.scope,
            &right.calculation,
            right.reason,
        ))
    });
    unavailable.dedup();

    let mut analysis = DerivativesAnalysis {
        schema: DERIVATIVES_ANALYSIS_SCHEMA.to_string(),
        engine_version: request.engine_version.clone(),
        source_bindings: request.source_bindings.clone(),
        pricing_model_hash: request
            .pricing_model
            .as_ref()
            .map(pricing_model_hash)
            .transpose()?,
        legs,
        groups,
        scenarios,
        sensitivity,
        accounting_reconciliation,
        unavailable,
        output_hash: String::new(),
    };
    analysis.output_hash = canonical_hash(&analysis, "derivatives_analysis_hash_failed")
        .map_err(|_| AnalysisError::SerializationFailure)?;
    Ok(analysis)
}

fn validate_request(request: &DerivativesAnalysisRequest) -> Result<(), AnalysisError> {
    if request.schema != DERIVATIVES_REQUEST_SCHEMA
        || request.engine_version.trim().is_empty()
        || request.as_of_ms < 0
        || request.max_mark_age_ms < 0
        || request.max_greek_age_ms < 0
        || request.legs.is_empty()
    {
        return Err(AnalysisError::InvalidRequest("request"));
    }
    if request.max_mark_age_ms > MAX_EVIDENCE_AGE_MS
        || request.max_greek_age_ms > MAX_EVIDENCE_AGE_MS
    {
        return Err(AnalysisError::LimitExceeded("evidence_age"));
    }
    if request.legs.len() > MAX_LEGS {
        return Err(AnalysisError::LimitExceeded("legs"));
    }
    if request.scenarios.len() > MAX_SCENARIOS {
        return Err(AnalysisError::LimitExceeded("scenarios"));
    }
    if let Some(grid) = &request.sensitivity_grid {
        if grid.price_shocks_bps.is_empty()
            || grid.volatility_shifts_ppm.is_empty()
            || grid.elapsed_seconds.is_empty()
        {
            return Err(AnalysisError::InvalidRequest("sensitivity_grid"));
        }
        if grid.price_shocks_bps.len() > MAX_SENSITIVITY_AXIS_VALUES
            || grid.volatility_shifts_ppm.len() > MAX_SENSITIVITY_AXIS_VALUES
            || grid.elapsed_seconds.len() > MAX_SENSITIVITY_AXIS_VALUES
        {
            return Err(AnalysisError::LimitExceeded("sensitivity_grid_axis"));
        }
        if has_duplicates(&grid.price_shocks_bps)
            || has_duplicates(&grid.volatility_shifts_ppm)
            || has_duplicates(&grid.elapsed_seconds)
            || grid
                .price_shocks_bps
                .iter()
                .any(|value| !(-MAX_PRICE_SHOCK_BPS..=MAX_PRICE_SHOCK_BPS).contains(value))
            || grid.volatility_shifts_ppm.iter().any(|value| {
                !(-MAX_VOLATILITY_SHIFT_PPM..=MAX_VOLATILITY_SHIFT_PPM).contains(value)
            })
            || grid
                .elapsed_seconds
                .iter()
                .any(|value| !(0..=MAX_ELAPSED_SECONDS).contains(value))
        {
            return Err(AnalysisError::InvalidRequest("sensitivity_grid"));
        }
        let count = grid
            .price_shocks_bps
            .len()
            .checked_mul(grid.volatility_shifts_ppm.len())
            .and_then(|value| value.checked_mul(grid.elapsed_seconds.len()))
            .ok_or(AnalysisError::LimitExceeded("sensitivity_grid"))?;
        if count > MAX_SENSITIVITY_POINTS {
            return Err(AnalysisError::LimitExceeded("sensitivity_grid"));
        }
    }
    let mut leg_ids = BTreeSet::new();
    for leg in &request.legs {
        if leg.leg_id.trim().is_empty()
            || leg.position_group_id.trim().is_empty()
            || leg.instrument_id.trim().is_empty()
            || leg.underlying_instrument_id.trim().is_empty()
            || leg.quantity_micros == 0
            || !leg_ids.insert(&leg.leg_id)
        {
            return Err(AnalysisError::InvalidRequest("leg"));
        }
    }
    let mut scenario_ids = BTreeSet::new();
    for scenario in &request.scenarios {
        if scenario.scenario_id.trim().is_empty()
            || !(0..=MAX_ELAPSED_SECONDS).contains(&scenario.elapsed_seconds)
            || !scenario_ids.insert(&scenario.scenario_id)
            || !valid_provenance(&scenario.declaration)
            || scenario
                .underlying_prices_micros
                .values()
                .any(|value| *value < 0)
            || scenario.volatility_shifts_ppm.values().any(|value| {
                !(-MAX_VOLATILITY_SHIFT_PPM..=MAX_VOLATILITY_SHIFT_PPM).contains(value)
            })
        {
            return Err(AnalysisError::InvalidRequest("scenario"));
        }
    }
    for mark in request.source_marks.values() {
        if mark.price_micros < 0 || !valid_provenance(&mark.provenance) {
            return Err(AnalysisError::InvalidRequest("source_mark"));
        }
    }
    Ok(())
}

fn has_duplicates<T: Ord + Copy>(values: &[T]) -> bool {
    let mut seen = BTreeSet::new();
    values.iter().any(|value| !seen.insert(*value))
}

fn verify_sources(
    request: &DerivativesAnalysisRequest,
    snapshot: &DatasetSnapshot,
    instruments: &[InstrumentConfiguration],
    accounting: &AccountingResult,
    report: &BacktestReport,
) -> Result<(), AnalysisError> {
    snapshot
        .verify()
        .map_err(|_| AnalysisError::IntegrityFailure("dataset"))?;
    let mut report_without_hash = report.clone();
    report_without_hash.report_hash.clear();
    let report_hash = canonical_hash(&report_without_hash, "derivatives_report_hash_failed")
        .map_err(|_| AnalysisError::SerializationFailure)?;
    let accounting_hash = accounting_content_hash(accounting)?;
    let instrument_configurations_hash = instrument_configurations_hash(instruments)?;
    let binding = &request.source_bindings;
    if report_hash != report.report_hash
        || binding.report_hash != report.report_hash
        || binding.run_id != report.run.run_id
        || binding.strategy_id != report.run.strategy_id
        || binding.strategy_version_id != report.run.strategy_version_id
        || binding.strategy_spec_hash != report.run.strategy_spec_hash
        || binding.manifest_hash != report.metadata.manifest_hash
        || binding.result_hash != report.metadata.result_hash
        || binding.dataset_hash != report.metadata.dataset_hash
        || binding.dataset_hash != snapshot.content_hash
        || binding.accounting_hash != accounting_hash
        || binding.instrument_configurations_hash != instrument_configurations_hash
        || binding.strategy_id != snapshot.content.strategy_id
        || snapshot.content.strategy_version_id.as_deref()
            != Some(binding.strategy_version_id.as_str())
    {
        return Err(AnalysisError::IntegrityFailure("source_binding"));
    }
    Ok(())
}

fn accounting_content_hash(accounting: &AccountingResult) -> Result<String, AnalysisError> {
    canonical_hash(accounting, "derivatives_accounting_hash_failed")
        .map_err(|_| AnalysisError::SerializationFailure)
}

fn instrument_configurations_hash(
    instruments: &[InstrumentConfiguration],
) -> Result<String, AnalysisError> {
    let mut canonical_instruments = instruments.to_vec();
    canonical_instruments.sort_by(|left, right| left.instrument_id.cmp(&right.instrument_id));
    canonical_hash(
        &canonical_instruments,
        "derivatives_instrument_configurations_hash_failed",
    )
    .map_err(|_| AnalysisError::SerializationFailure)
}

fn pricing_model_hash(model: &VersionedPricingModel) -> Result<String, AnalysisError> {
    canonical_hash(model, "derivatives_model_hash_failed")
        .map_err(|_| AnalysisError::SerializationFailure)
}

fn model_scenario_hash(
    model: &VersionedPricingModel,
    declaration: &Provenance,
) -> Result<String, AnalysisError> {
    canonical_hash(
        &(pricing_model_hash(model)?, &declaration.content_hash),
        "derivatives_model_scenario_hash_failed",
    )
    .map_err(|_| AnalysisError::SerializationFailure)
}

fn latest_option_observations(
    snapshot: &DatasetSnapshot,
    as_of_ms: i64,
) -> Result<BTreeMap<&str, (&OptionContractObservation, i64)>, AnalysisError> {
    let mut latest = BTreeMap::new();
    for row in &snapshot.content.observations {
        let HistoricalObservationData::OptionContract(contract) = &row.data else {
            continue;
        };
        let timestamp = parse_timestamp_ms(&row.timestamp)?;
        if timestamp <= as_of_ms
            && latest
                .get(contract.contract_symbol.as_str())
                .is_none_or(|(_, prior)| timestamp > *prior)
        {
            latest.insert(
                contract.contract_symbol.as_str(),
                (contract.as_ref(), timestamp),
            );
        }
    }
    Ok(latest)
}

fn verify_leg_source_facts(
    request: &DerivativesAnalysisRequest,
    instruments: &BTreeMap<&str, &InstrumentConfiguration>,
    observations: &BTreeMap<&str, (&OptionContractObservation, i64)>,
) -> Result<(), AnalysisError> {
    for leg in &request.legs {
        let configuration = instruments.get(leg.instrument_id.as_str()).copied();
        let observation = observations.get(leg.instrument_id.as_str()).copied();
        let configured_as_option = configuration.is_some_and(|configuration| {
            matches!(
                configuration.instrument_family.as_str(),
                "option" | "option_contract" | "equity_option"
            )
        });
        match leg.instrument_kind {
            InstrumentKind::Underlying => {
                if observation.is_some()
                    || configured_as_option
                    || leg.underlying_instrument_id != leg.instrument_id
                {
                    return Err(AnalysisError::IntegrityFailure("instrument_kind"));
                }
            }
            InstrumentKind::Option => {
                if configuration.is_some_and(|_| !configured_as_option) {
                    return Err(AnalysisError::IntegrityFailure("instrument_kind"));
                }
                if observation.is_some_and(|(observation, _)| {
                    observation.underlying_instrument_id != leg.underlying_instrument_id
                }) {
                    return Err(AnalysisError::IntegrityFailure("underlying_instrument"));
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn analyze_leg(
    leg: &StrategyLeg,
    observation: Option<(&OptionContractObservation, i64)>,
    configuration: Option<&InstrumentConfiguration>,
    request: &DerivativesAnalysisRequest,
    snapshot: &DatasetSnapshot,
    accounting: &AccountingResult,
    report: &BacktestReport,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<LegAnalysis, AnalysisError> {
    let contract = if leg.instrument_kind == InstrumentKind::Option {
        contract_metadata(leg, observation, configuration, unavailable)?
    } else {
        None
    };
    let mark = leg_mark(leg, observation, request, snapshot, accounting, unavailable)?;
    let source_greeks = source_greeks(leg, observation, request, snapshot, unavailable)?;
    let model_greeks = if source_greeks.is_none() && leg.instrument_kind == InstrumentKind::Option {
        model_greeks(leg, contract.as_ref(), request, unavailable)?
    } else {
        None
    };
    let lifecycle_complete = validate_lifecycle(leg, configuration, request, report, unavailable);
    let (realized_pnl, unrealized_pnl) = lifecycle_pnl(leg, accounting, report, unavailable)?;
    let payoff_risk = if leg.lifecycle.state == LifecycleState::Open {
        payoff_risk(
            std::slice::from_ref(leg),
            &BTreeMap::from([(leg.leg_id.as_str(), contract.as_ref())]),
        )?
    } else if !lifecycle_complete {
        unavailable_risk(UnavailableReason::MissingLifecycleFacts)
    } else {
        match realized_pnl.value() {
            Some(value) => PayoffRisk {
                minimum_pnl_micros: PnlBound::Finite(value),
                maximum_pnl_micros: PnlBound::Finite(value),
            },
            None => unavailable_risk(UnavailableReason::MissingReportTrade),
        }
    };
    Ok(LegAnalysis {
        leg_id: leg.leg_id.clone(),
        position_group_id: leg.position_group_id.clone(),
        instrument_id: leg.instrument_id.clone(),
        instrument_kind: leg.instrument_kind,
        quantity_micros: leg.quantity_micros,
        contract,
        mark,
        source_greeks,
        model_greeks,
        realized_pnl,
        unrealized_pnl,
        lifecycle: leg.lifecycle.clone(),
        lifecycle_complete,
        payoff_risk,
    })
}

fn contract_metadata(
    leg: &StrategyLeg,
    observation: Option<(&OptionContractObservation, i64)>,
    configuration: Option<&InstrumentConfiguration>,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<Option<ContractMetadata>, AnalysisError> {
    let Some((option, _)) = observation else {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "contract",
            UnavailableReason::MissingContractMetadata,
        );
        return Ok(None);
    };
    let Some(configuration) = configuration else {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "contract",
            UnavailableReason::MissingMultiplier,
        );
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "contract",
            UnavailableReason::MissingSettlementFacts,
        );
        return Ok(None);
    };
    if configuration.contract_multiplier_micros <= 0 {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "contract",
            UnavailableReason::MissingMultiplier,
        );
        return Ok(None);
    }
    if configuration.settlement.trim().is_empty() {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "contract",
            UnavailableReason::MissingSettlementFacts,
        );
        return Ok(None);
    }
    Ok(Some(ContractMetadata {
        contract_symbol: option.contract_symbol.clone(),
        underlying_instrument_id: option.underlying_instrument_id.clone(),
        expiration: option.expiration.clone(),
        strike_micros: f64_to_scaled(option.strike)?,
        right: option.right.clone(),
        multiplier_micros: configuration.contract_multiplier_micros,
        settlement: configuration.settlement.clone(),
    }))
}

fn leg_mark(
    leg: &StrategyLeg,
    observation: Option<(&OptionContractObservation, i64)>,
    request: &DerivativesAnalysisRequest,
    snapshot: &DatasetSnapshot,
    accounting: &AccountingResult,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<AvailableValue, AnalysisError> {
    if leg.instrument_kind == InstrumentKind::Underlying {
        return Ok(match request.source_marks.get(&leg.instrument_id) {
            Some(mark)
                if mark.provenance.content_hash == request.source_bindings.accounting_hash
                    && mark.provenance.source_ref
                        == format!(
                            "tradeassembly://backtests/{}/accounting",
                            request.source_bindings.run_id
                        )
                    && accounting
                        .positions
                        .get(&leg.instrument_id)
                        .is_some_and(|position| {
                            position.market_price_micros == mark.price_micros
                        })
                    && mark.observed_at_ms <= request.as_of_ms
                    && request.as_of_ms.saturating_sub(mark.observed_at_ms)
                        <= request.max_mark_age_ms =>
            {
                AvailableValue::Available {
                    value_micros: mark.price_micros,
                    provenance: mark.provenance.content_hash.clone(),
                }
            }
            Some(mark)
                if mark.observed_at_ms <= request.as_of_ms
                    && request.as_of_ms.saturating_sub(mark.observed_at_ms)
                        > request.max_mark_age_ms =>
            {
                unavailable_value(
                    unavailable,
                    &leg.leg_id,
                    "mark",
                    UnavailableReason::StaleQuote,
                )
            }
            Some(_) => unavailable_value(
                unavailable,
                &leg.leg_id,
                "mark",
                UnavailableReason::SourceFactMismatch,
            ),
            None => unavailable_value(
                unavailable,
                &leg.leg_id,
                "mark",
                UnavailableReason::MissingQuote,
            ),
        });
    }
    let Some((option, observed_at_ms)) = observation else {
        return Ok(unavailable_value(
            unavailable,
            &leg.leg_id,
            "mark",
            UnavailableReason::MissingQuote,
        ));
    };
    if request.as_of_ms.saturating_sub(observed_at_ms) > request.max_mark_age_ms {
        return Ok(unavailable_value(
            unavailable,
            &leg.leg_id,
            "mark",
            UnavailableReason::StaleQuote,
        ));
    }
    let quote = match (option.bid, option.ask, option.last) {
        (Some(bid), Some(ask), _) => Some((bid + ask) / 2.0),
        (_, _, last) => last,
    };
    Ok(match quote {
        Some(value) => AvailableValue::Available {
            value_micros: f64_to_scaled(value)?,
            provenance: snapshot.content_hash.clone(),
        },
        None => unavailable_value(
            unavailable,
            &leg.leg_id,
            "mark",
            UnavailableReason::MissingQuote,
        ),
    })
}

fn source_greeks(
    leg: &StrategyLeg,
    observation: Option<(&OptionContractObservation, i64)>,
    request: &DerivativesAnalysisRequest,
    snapshot: &DatasetSnapshot,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<Option<GreekEvidence>, AnalysisError> {
    if leg.instrument_kind != InstrumentKind::Option {
        return Ok(None);
    }
    let Some((option, observed_at_ms)) = observation else {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "greeks",
            UnavailableReason::MissingGreeks,
        );
        return Ok(None);
    };
    let Some(greeks) = &option.greeks else {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "greeks",
            UnavailableReason::MissingGreeks,
        );
        return Ok(None);
    };
    let age_ms = request.as_of_ms.saturating_sub(observed_at_ms);
    let stale = age_ms > request.max_greek_age_ms;
    if stale {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "fresh_greeks",
            UnavailableReason::StaleGreeks,
        );
    }
    Ok(Some(GreekEvidence {
        values_ppm: greek_values(greeks)?,
        observed_at_ms,
        age_ms,
        stale,
        source_ref: snapshot.snapshot_ref.clone(),
        source_content_hash: snapshot.content_hash.clone(),
        source_plugin_ref: snapshot.content.source.plugin_ref.clone(),
        source_operation_id: snapshot.content.source.operation_id.clone(),
        pathway: "verified_source_observation".to_string(),
    }))
}

fn model_greeks(
    leg: &StrategyLeg,
    contract: Option<&ContractMetadata>,
    request: &DerivativesAnalysisRequest,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<Option<GreekEvidence>, AnalysisError> {
    let Some(contract) = contract else {
        return Ok(None);
    };
    let Some(model) = request.pricing_model.as_ref() else {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "model_greeks",
            UnavailableReason::MissingExplicitPricingModel,
        );
        return Ok(None);
    };
    match model_inputs(leg, contract, model, 0, 0, None) {
        Ok(inputs) => {
            let priced = black_scholes(&inputs, &contract.right)?;
            Ok(Some(GreekEvidence {
                values_ppm: priced.greeks,
                observed_at_ms: request.as_of_ms,
                age_ms: 0,
                stale: false,
                source_ref: format!("model:{}:{}", model.model, model.version),
                source_content_hash: pricing_model_hash(model)?,
                source_plugin_ref: String::new(),
                source_operation_id: String::new(),
                pathway: "explicit_versioned_model".to_string(),
            }))
        }
        Err(reason) => {
            push_unavailable(unavailable, &leg.leg_id, "model_greeks", reason);
            Ok(None)
        }
    }
}

fn validate_lifecycle(
    leg: &StrategyLeg,
    configuration: Option<&InstrumentConfiguration>,
    request: &DerivativesAnalysisRequest,
    report: &BacktestReport,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> bool {
    let lifecycle = &leg.lifecycle;
    let report_ref = format!(
        "tradeassembly://backtests/{}/report",
        request.source_bindings.run_id
    );
    let refs_valid = !lifecycle.evidence_refs.is_empty()
        && lifecycle.evidence_refs.iter().all(valid_provenance)
        && lifecycle.evidence_refs.iter().any(|reference| {
            reference.source_ref == report_ref && reference.content_hash == report.report_hash
        });
    let report_facts_match = lifecycle_matches_report(leg, report);
    let complete = match lifecycle.state {
        LifecycleState::Open => true,
        LifecycleState::Closed => {
            lifecycle.occurred_at_ms.is_some() && refs_valid && report_facts_match
        }
        LifecycleState::Exercised | LifecycleState::Assigned | LifecycleState::Expired => {
            lifecycle.occurred_at_ms.is_some()
                && lifecycle.settlement_price_micros.is_some()
                && refs_valid
                && report_facts_match
                && configuration.is_some_and(|value| !value.settlement.trim().is_empty())
        }
        LifecycleState::Settled => {
            lifecycle.occurred_at_ms.is_some()
                && lifecycle.settlement_price_micros.is_some()
                && lifecycle.settlement_cashflow_micros.is_some()
                && refs_valid
                && report_facts_match
        }
        LifecycleState::TerminalValued => {
            lifecycle.occurred_at_ms.is_some()
                && lifecycle.terminal_value_micros.is_some()
                && refs_valid
                && report_facts_match
        }
    };
    if !complete {
        push_unavailable(
            unavailable,
            &leg.leg_id,
            "lifecycle",
            UnavailableReason::MissingLifecycleFacts,
        );
    }
    complete
}

fn lifecycle_matches_report(leg: &StrategyLeg, report: &BacktestReport) -> bool {
    if leg.report_trade_ids.is_empty() {
        return false;
    }
    let trades = leg
        .report_trade_ids
        .iter()
        .map(|trade_id| {
            report
                .trades
                .iter()
                .find(|trade| trade.trade_id == *trade_id && trade.symbol == leg.instrument_id)
        })
        .collect::<Option<Vec<_>>>();
    let Some(trades) = trades else {
        return false;
    };
    let quantity = trades
        .iter()
        .map(|trade| i128::from(trade.quantity_micros).abs())
        .sum::<i128>();
    if quantity != i128::from(leg.quantity_micros).abs() {
        return false;
    }

    match leg.lifecycle.state {
        LifecycleState::Open => true,
        LifecycleState::Closed => trades.iter().all(|trade| {
            trade.realized
                && trade.exit.as_ref().is_some_and(|value| {
                    lifecycle_state_matches(value, LifecycleState::Closed)
                        && lifecycle_time_matches(value, leg.lifecycle.occurred_at_ms)
                })
        }),
        LifecycleState::TerminalValued => {
            let values = trades
                .iter()
                .map(|trade| {
                    if trade.realized {
                        return None;
                    }
                    let evidence = trade.terminal_valuation.as_ref()?;
                    if !lifecycle_state_matches(evidence, LifecycleState::TerminalValued)
                        || !lifecycle_time_matches(evidence, leg.lifecycle.occurred_at_ms)
                    {
                        return None;
                    }
                    let quantity = evidence.get("quantityMicros")?.as_i64()?;
                    let price = evidence.get("priceMicros")?.as_i64()?;
                    i128::from(quantity)
                        .checked_mul(i128::from(price))?
                        .checked_div(i128::from(SCALE))
                })
                .collect::<Option<Vec<_>>>();
            values.is_some_and(|values| {
                values.into_iter().sum::<i128>()
                    == i128::from(leg.lifecycle.terminal_value_micros.unwrap_or_default())
            })
        }
        state => trades.iter().all(|trade| {
            trade
                .exit
                .as_ref()
                .or(trade.terminal_valuation.as_ref())
                .is_some_and(|value| {
                    lifecycle_state_matches(value, state)
                        && lifecycle_time_matches(value, leg.lifecycle.occurred_at_ms)
                        && optional_i64_matches(
                            value,
                            "settlementPriceMicros",
                            leg.lifecycle.settlement_price_micros,
                        )
                        && optional_i64_matches(
                            value,
                            "settlementCashflowMicros",
                            leg.lifecycle.settlement_cashflow_micros,
                        )
                })
        }),
    }
}

fn lifecycle_state_matches(value: &Value, state: LifecycleState) -> bool {
    value.get("lifecycleState").and_then(Value::as_str) == Some(lifecycle_state_name(state))
}

fn lifecycle_time_matches(value: &Value, expected: Option<i64>) -> bool {
    let observed = value
        .get("occurredAtMs")
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
                .map(|timestamp| timestamp.timestamp_millis())
        });
    observed == expected
}

fn optional_i64_matches(value: &Value, field: &str, expected: Option<i64>) -> bool {
    expected.is_none_or(|expected| value.get(field).and_then(Value::as_i64) == Some(expected))
}

fn lifecycle_state_name(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Open => "open",
        LifecycleState::Closed => "closed",
        LifecycleState::Exercised => "exercised",
        LifecycleState::Assigned => "assigned",
        LifecycleState::Expired => "expired",
        LifecycleState::Settled => "settled",
        LifecycleState::TerminalValued => "terminal_valued",
    }
}

fn lifecycle_pnl(
    leg: &StrategyLeg,
    accounting: &AccountingResult,
    report: &BacktestReport,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<(AvailableValue, AvailableValue), AnalysisError> {
    let mut realized = 0_i64;
    let mut all_trades_found = true;
    for trade_id in &leg.report_trade_ids {
        match report
            .trades
            .iter()
            .find(|trade| &trade.trade_id == trade_id)
        {
            Some(trade) if trade.realized => match trade.pnl_micros {
                Some(value) => realized = checked_add(realized, value)?,
                None => all_trades_found = false,
            },
            _ => all_trades_found = false,
        }
    }
    let realized_pnl = if !leg.report_trade_ids.is_empty() && all_trades_found {
        AvailableValue::Available {
            value_micros: realized,
            provenance: report.report_hash.clone(),
        }
    } else if leg.lifecycle.state == LifecycleState::Open && leg.report_trade_ids.is_empty() {
        AvailableValue::Available {
            value_micros: 0,
            provenance: report.report_hash.clone(),
        }
    } else {
        unavailable_value(
            unavailable,
            &leg.leg_id,
            "realized_pnl",
            UnavailableReason::MissingReportTrade,
        )
    };
    let unrealized_pnl = if leg.lifecycle.state == LifecycleState::Open {
        match accounting.positions.get(&leg.instrument_id) {
            Some(position) if position.quantity == leg.quantity_micros => {
                AvailableValue::Available {
                    value_micros: position.unrealized_pnl_micros,
                    provenance: report.metadata.result_hash.clone(),
                }
            }
            _ => unavailable_value(
                unavailable,
                &leg.leg_id,
                "unrealized_pnl",
                UnavailableReason::MissingAccountingPosition,
            ),
        }
    } else {
        AvailableValue::Available {
            value_micros: 0,
            provenance: report.metadata.result_hash.clone(),
        }
    };
    Ok((realized_pnl, unrealized_pnl))
}

fn analyze_groups(
    request_legs: &[StrategyLeg],
    legs: &[LegAnalysis],
) -> Result<Vec<GroupAnalysis>, AnalysisError> {
    let mut grouped = BTreeMap::<&str, Vec<&LegAnalysis>>::new();
    for leg in legs {
        grouped.entry(&leg.position_group_id).or_default().push(leg);
    }
    let request_by_id = request_legs
        .iter()
        .map(|leg| (leg.leg_id.as_str(), leg))
        .collect::<BTreeMap<_, _>>();
    grouped
        .into_iter()
        .map(|(group_id, mut group_legs)| {
            group_legs.sort_by(|left, right| left.leg_id.cmp(&right.leg_id));
            let realized = aggregate_available(
                group_legs.iter().map(|leg| &leg.realized_pnl),
                "accepted_backtest_report",
            )?;
            let unrealized = aggregate_available(
                group_legs.iter().map(|leg| &leg.unrealized_pnl),
                "accepted_backtest_accounting",
            )?;
            let contracts = group_legs
                .iter()
                .map(|leg| (leg.leg_id.as_str(), leg.contract.as_ref()))
                .collect();
            let open_raw_legs = group_legs
                .iter()
                .filter(|leg| leg.lifecycle.state == LifecycleState::Open)
                .map(|leg| request_by_id[leg.leg_id.as_str()])
                .collect::<Vec<_>>();
            let mut risks = group_legs
                .iter()
                .filter(|leg| leg.lifecycle.state != LifecycleState::Open)
                .map(|leg| leg.payoff_risk.clone())
                .collect::<Vec<_>>();
            if !open_raw_legs.is_empty() {
                risks.push(payoff_risk_refs(&open_raw_legs, &contracts)?);
            }
            Ok(GroupAnalysis {
                position_group_id: group_id.to_string(),
                leg_ids: group_legs.iter().map(|leg| leg.leg_id.clone()).collect(),
                realized_pnl: realized,
                unrealized_pnl: unrealized,
                payoff_risk: aggregate_payoff_risks(risks.iter())?,
            })
        })
        .collect()
}

fn analyze_scenario(
    scenario: &DeclaredScenario,
    request_legs: &[StrategyLeg],
    legs: &[LegAnalysis],
    request: &DerivativesAnalysisRequest,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<ScenarioAnalysis, AnalysisError> {
    let analyses = legs
        .iter()
        .map(|leg| (leg.leg_id.as_str(), leg))
        .collect::<BTreeMap<_, _>>();
    let mut values = Vec::new();
    for leg in sorted_legs(request_legs) {
        let analysis = analyses[leg.leg_id.as_str()];
        let value = scenario_leg_value(leg, analysis, scenario, request, unavailable)?;
        let pnl = if leg.lifecycle.state != LifecycleState::Open {
            analysis.realized_pnl.clone()
        } else {
            match (value.value(), leg.entry_price_micros) {
                (Some(position_value_micros), Some(entry)) => {
                    let multiplier = analysis
                        .contract
                        .as_ref()
                        .map_or(SCALE, |contract| contract.multiplier_micros);
                    let cost_basis = position_value(entry, leg.quantity_micros, multiplier)?;
                    AvailableValue::Available {
                        value_micros: position_value_micros
                            .checked_sub(cost_basis)
                            .ok_or(AnalysisError::ArithmeticOverflow)?,
                        provenance: value
                            .provenance()
                            .unwrap_or(&scenario.declaration.content_hash)
                            .to_string(),
                    }
                }
                (_, None) => unavailable_value(
                    unavailable,
                    &leg.leg_id,
                    &format!("scenario:{}:pnl", scenario.scenario_id),
                    UnavailableReason::MissingEntryPrice,
                ),
                _ => value.clone(),
            }
        };
        values.push(ScenarioLegValue {
            leg_id: leg.leg_id.clone(),
            value,
            pnl,
        });
    }
    let aggregate_value = aggregate_available(
        values.iter().map(|value| &value.value),
        "scenario_valuation",
    )?;
    let aggregate_pnl =
        aggregate_available(values.iter().map(|value| &value.pnl), "scenario_valuation")?;
    Ok(ScenarioAnalysis {
        scenario_id: scenario.scenario_id.clone(),
        kind: scenario.kind,
        declaration: scenario.declaration.clone(),
        legs: values,
        aggregate_value,
        aggregate_pnl,
    })
}

fn scenario_leg_value(
    leg: &StrategyLeg,
    analysis: &LegAnalysis,
    scenario: &DeclaredScenario,
    request: &DerivativesAnalysisRequest,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<AvailableValue, AnalysisError> {
    let calculation = format!("scenario:{}:value", scenario.scenario_id);
    if leg.lifecycle.state != LifecycleState::Open {
        return Ok(inactive_leg_value(analysis, unavailable, &calculation));
    }
    if leg.instrument_kind == InstrumentKind::Underlying {
        return Ok(
            match scenario
                .underlying_prices_micros
                .get(&leg.instrument_id)
                .copied()
                .or_else(|| analysis.mark.value())
            {
                Some(value) => AvailableValue::Available {
                    value_micros: position_value(value, leg.quantity_micros, SCALE)?,
                    provenance: scenario.declaration.content_hash.clone(),
                },
                None => unavailable_value(
                    unavailable,
                    &leg.leg_id,
                    &calculation,
                    UnavailableReason::MissingQuote,
                ),
            },
        );
    }
    let Some(contract) = analysis.contract.as_ref() else {
        return Ok(unavailable_value(
            unavailable,
            &leg.leg_id,
            &calculation,
            UnavailableReason::MissingContractMetadata,
        ));
    };
    let spot = scenario
        .underlying_prices_micros
        .get(&leg.underlying_instrument_id)
        .copied();
    let unit_value = if scenario.kind == ScenarioKind::Terminal {
        let Some(spot) = spot else {
            return Ok(unavailable_value(
                unavailable,
                &leg.leg_id,
                &calculation,
                UnavailableReason::MissingModelSpot,
            ));
        };
        intrinsic_value(spot, contract.strike_micros, &contract.right)
    } else {
        let Some(model) = request.pricing_model.as_ref() else {
            return Ok(unavailable_value(
                unavailable,
                &leg.leg_id,
                &calculation,
                UnavailableReason::MissingExplicitPricingModel,
            ));
        };
        let vol_shift = scenario
            .volatility_shifts_ppm
            .get(&leg.instrument_id)
            .copied()
            .unwrap_or(0);
        match model_inputs(
            leg,
            contract,
            model,
            scenario.elapsed_seconds,
            vol_shift,
            spot,
        ) {
            Ok(inputs) => black_scholes(&inputs, &contract.right)?.value_micros,
            Err(reason) => {
                return Ok(unavailable_value(
                    unavailable,
                    &leg.leg_id,
                    &calculation,
                    reason,
                ));
            }
        }
    };
    let provenance = if scenario.kind == ScenarioKind::Terminal {
        scenario.declaration.content_hash.clone()
    } else {
        model_scenario_hash(
            request
                .pricing_model
                .as_ref()
                .expect("non-terminal model checked"),
            &scenario.declaration,
        )?
    };
    Ok(AvailableValue::Available {
        value_micros: position_value(unit_value, leg.quantity_micros, contract.multiplier_micros)?,
        provenance,
    })
}

fn analyze_sensitivity(
    request: &DerivativesAnalysisRequest,
    request_legs: &[StrategyLeg],
    legs: &[LegAnalysis],
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<Vec<SensitivityPoint>, AnalysisError> {
    let Some(grid) = request.sensitivity_grid.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(model) = request.pricing_model.as_ref() else {
        push_unavailable(
            unavailable,
            "strategy",
            "sensitivity",
            UnavailableReason::MissingExplicitPricingModel,
        );
        return Ok(Vec::new());
    };
    let model_hash = pricing_model_hash(model)?;
    let analyses = legs
        .iter()
        .map(|leg| (leg.leg_id.as_str(), leg))
        .collect::<BTreeMap<_, _>>();
    let mut points = Vec::new();
    let mut prices = grid.price_shocks_bps.clone();
    let mut vols = grid.volatility_shifts_ppm.clone();
    let mut times = grid.elapsed_seconds.clone();
    prices.sort();
    vols.sort();
    times.sort();
    for price_shock in prices {
        for vol_shift in &vols {
            for elapsed in &times {
                let mut values = Vec::new();
                for leg in sorted_legs(request_legs) {
                    let analysis = analyses[leg.leg_id.as_str()];
                    let value = if leg.lifecycle.state != LifecycleState::Open {
                        inactive_leg_value(analysis, unavailable, "sensitivity")
                    } else if leg.instrument_kind == InstrumentKind::Underlying {
                        model
                            .spot_prices_micros
                            .get(&leg.underlying_instrument_id)
                            .and_then(|spot| shock_price(*spot, price_shock).ok())
                            .map(|spot| position_value(spot, leg.quantity_micros, SCALE))
                            .transpose()?
                            .map(|value| AvailableValue::Available {
                                value_micros: value,
                                provenance: model_hash.clone(),
                            })
                            .unwrap_or(AvailableValue::Unavailable {
                                reason: UnavailableReason::MissingModelSpot,
                            })
                    } else if let Some(contract) = analysis.contract.as_ref() {
                        let shocked_spot = model
                            .spot_prices_micros
                            .get(&leg.underlying_instrument_id)
                            .copied()
                            .map(|spot| shock_price(spot, price_shock))
                            .transpose()?;
                        match model_inputs(leg, contract, model, *elapsed, *vol_shift, shocked_spot)
                        {
                            Ok(inputs) => AvailableValue::Available {
                                value_micros: position_value(
                                    black_scholes(&inputs, &contract.right)?.value_micros,
                                    leg.quantity_micros,
                                    contract.multiplier_micros,
                                )?,
                                provenance: model_hash.clone(),
                            },
                            Err(reason) => AvailableValue::Unavailable { reason },
                        }
                    } else {
                        AvailableValue::Unavailable {
                            reason: UnavailableReason::MissingContractMetadata,
                        }
                    };
                    values.push(value);
                }
                points.push(SensitivityPoint {
                    price_shock_bps: price_shock,
                    volatility_shift_ppm: *vol_shift,
                    elapsed_seconds: *elapsed,
                    aggregate_value: aggregate_available(values.iter(), &model_hash)?,
                });
            }
        }
    }
    Ok(points)
}

fn reconcile_accounting(
    legs: &[LegAnalysis],
    accounting: &AccountingResult,
    report: &BacktestReport,
    unavailable: &mut Vec<UnavailableCalculation>,
) -> Result<AccountingReconciliation, AnalysisError> {
    let realized = sum_available(legs.iter().map(|leg| &leg.realized_pnl))?;
    let unrealized = sum_available(legs.iter().map(|leg| &leg.unrealized_pnl))?;
    let accepted_consistent = accounting.realized_pnl_micros == report.overview.realized_pnl_micros
        && accounting.unrealized_pnl_micros == report.overview.unrealized_pnl_micros;
    let status = match (realized, unrealized) {
        (Some(realized), Some(unrealized))
            if accepted_consistent
                && realized == accounting.realized_pnl_micros
                && unrealized == accounting.unrealized_pnl_micros =>
        {
            ReconciliationStatus::Reconciled
        }
        (Some(_), Some(_)) => {
            push_unavailable(
                unavailable,
                "strategy",
                "accounting_reconciliation",
                UnavailableReason::AccountingMismatch,
            );
            ReconciliationStatus::Mismatch
        }
        _ => ReconciliationStatus::Unavailable,
    };
    Ok(AccountingReconciliation {
        status,
        analyzed_realized_pnl_micros: realized,
        analyzed_unrealized_pnl_micros: unrealized,
        accounting_realized_pnl_micros: accounting.realized_pnl_micros,
        accounting_unrealized_pnl_micros: accounting.unrealized_pnl_micros,
        report_realized_pnl_micros: report.overview.realized_pnl_micros,
        report_unrealized_pnl_micros: report.overview.unrealized_pnl_micros,
    })
}

fn payoff_risk(
    legs: &[StrategyLeg],
    contracts: &BTreeMap<&str, Option<&ContractMetadata>>,
) -> Result<PayoffRisk, AnalysisError> {
    let refs = legs.iter().collect::<Vec<_>>();
    payoff_risk_refs(&refs, contracts)
}

fn payoff_risk_refs(
    legs: &[&StrategyLeg],
    contracts: &BTreeMap<&str, Option<&ContractMetadata>>,
) -> Result<PayoffRisk, AnalysisError> {
    if legs.iter().any(|leg| leg.entry_price_micros.is_none()) {
        return Ok(unavailable_risk(UnavailableReason::MissingEntryPrice));
    }
    if legs.iter().any(|leg| {
        leg.instrument_kind == InstrumentKind::Option
            && contracts
                .get(leg.leg_id.as_str())
                .copied()
                .flatten()
                .is_none()
    }) {
        return Ok(unavailable_risk(UnavailableReason::MissingContractMetadata));
    }
    let mut by_underlying = BTreeMap::<&str, Vec<&StrategyLeg>>::new();
    for leg in legs {
        by_underlying
            .entry(&leg.underlying_instrument_id)
            .or_default()
            .push(*leg);
    }
    let components = by_underlying
        .into_values()
        .map(|component| payoff_risk_single(&component, contracts))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PayoffRisk {
        minimum_pnl_micros: aggregate_lower_bound(
            components.iter().map(|risk| &risk.minimum_pnl_micros),
        )?,
        maximum_pnl_micros: aggregate_upper_bound(
            components.iter().map(|risk| &risk.maximum_pnl_micros),
        )?,
    })
}

fn payoff_risk_single(
    legs: &[&StrategyLeg],
    contracts: &BTreeMap<&str, Option<&ContractMetadata>>,
) -> Result<PayoffRisk, AnalysisError> {
    let mut breakpoints = BTreeSet::from([0_i64]);
    for leg in legs {
        if let Some(contract) = contracts.get(leg.leg_id.as_str()).copied().flatten() {
            breakpoints.insert(contract.strike_micros);
        }
    }
    let mut values = Vec::new();
    for spot in breakpoints {
        values.push(terminal_group_pnl(legs, contracts, spot)?);
    }
    let tail_slope = legs.iter().try_fold(0_i128, |sum, leg| {
        let active = match leg.instrument_kind {
            InstrumentKind::Underlying => true,
            InstrumentKind::Option => contracts[leg.leg_id.as_str()]
                .is_some_and(|contract| contract.right == OptionRight::Call),
        };
        if active {
            sum.checked_add(
                i128::from(leg.quantity_micros)
                    .checked_mul(i128::from(
                        contracts
                            .get(leg.leg_id.as_str())
                            .copied()
                            .flatten()
                            .map_or(SCALE, |contract| contract.multiplier_micros),
                    ))
                    .ok_or(AnalysisError::ArithmeticOverflow)?,
            )
            .ok_or(AnalysisError::ArithmeticOverflow)
        } else {
            Ok(sum)
        }
    })?;
    let minimum = *values
        .iter()
        .min()
        .ok_or(AnalysisError::InvalidRequest("legs"))?;
    let maximum = *values
        .iter()
        .max()
        .ok_or(AnalysisError::InvalidRequest("legs"))?;
    Ok(PayoffRisk {
        minimum_pnl_micros: if tail_slope < 0 {
            PnlBound::UnboundedNegative
        } else {
            PnlBound::Finite(minimum)
        },
        maximum_pnl_micros: if tail_slope > 0 {
            PnlBound::UnboundedPositive
        } else {
            PnlBound::Finite(maximum)
        },
    })
}

fn aggregate_lower_bound<'a>(
    bounds: impl Iterator<Item = &'a PnlBound>,
) -> Result<PnlBound, AnalysisError> {
    let mut total = 0_i64;
    for bound in bounds {
        match bound {
            PnlBound::UnboundedNegative => return Ok(PnlBound::UnboundedNegative),
            PnlBound::Unavailable(reason) => return Ok(PnlBound::Unavailable(*reason)),
            PnlBound::Finite(value) => total = checked_add(total, *value)?,
            PnlBound::UnboundedPositive => {
                return Err(AnalysisError::InvalidRequest("minimum_pnl_bound"))
            }
        }
    }
    Ok(PnlBound::Finite(total))
}

fn aggregate_upper_bound<'a>(
    bounds: impl Iterator<Item = &'a PnlBound>,
) -> Result<PnlBound, AnalysisError> {
    let mut total = 0_i64;
    for bound in bounds {
        match bound {
            PnlBound::UnboundedPositive => return Ok(PnlBound::UnboundedPositive),
            PnlBound::Unavailable(reason) => return Ok(PnlBound::Unavailable(*reason)),
            PnlBound::Finite(value) => total = checked_add(total, *value)?,
            PnlBound::UnboundedNegative => {
                return Err(AnalysisError::InvalidRequest("maximum_pnl_bound"))
            }
        }
    }
    Ok(PnlBound::Finite(total))
}

fn aggregate_payoff_risks<'a>(
    risks: impl Iterator<Item = &'a PayoffRisk>,
) -> Result<PayoffRisk, AnalysisError> {
    let risks = risks.collect::<Vec<_>>();
    Ok(PayoffRisk {
        minimum_pnl_micros: aggregate_lower_bound(
            risks.iter().map(|risk| &risk.minimum_pnl_micros),
        )?,
        maximum_pnl_micros: aggregate_upper_bound(
            risks.iter().map(|risk| &risk.maximum_pnl_micros),
        )?,
    })
}

fn terminal_group_pnl(
    legs: &[&StrategyLeg],
    contracts: &BTreeMap<&str, Option<&ContractMetadata>>,
    spot: i64,
) -> Result<i64, AnalysisError> {
    legs.iter().try_fold(0_i64, |sum, leg| {
        let terminal = if leg.instrument_kind == InstrumentKind::Underlying {
            spot
        } else {
            let contract =
                contracts[leg.leg_id.as_str()].ok_or(AnalysisError::InvalidRequest("contract"))?;
            intrinsic_value(spot, contract.strike_micros, &contract.right)
        };
        let multiplier = contracts
            .get(leg.leg_id.as_str())
            .copied()
            .flatten()
            .map_or(SCALE, |contract| contract.multiplier_micros);
        checked_add(
            sum,
            position_pnl(
                terminal,
                leg.entry_price_micros
                    .ok_or(AnalysisError::InvalidRequest("entry_price"))?,
                leg.quantity_micros,
                multiplier,
            )?,
        )
    })
}

#[derive(Clone, Copy)]
struct ModelInputs {
    spot: f64,
    strike: f64,
    volatility: f64,
    rate: f64,
    dividend: f64,
    time_years: f64,
}

struct ModelPrice {
    value_micros: i64,
    greeks: GreekValuesPpm,
}

fn model_inputs(
    leg: &StrategyLeg,
    contract: &ContractMetadata,
    model: &VersionedPricingModel,
    elapsed_seconds: i64,
    volatility_shift_ppm: i64,
    scenario_spot: Option<i64>,
) -> Result<ModelInputs, UnavailableReason> {
    if model.model != BLACK_SCHOLES_MERTON_MODEL || model.version != BLACK_SCHOLES_MERTON_VERSION {
        return Err(UnavailableReason::UnsupportedPricingModel);
    }
    if model.input_provenance.is_empty() || !model.input_provenance.iter().all(valid_provenance) {
        return Err(UnavailableReason::MissingContractMetadata);
    }
    let spot = scenario_spot
        .or_else(|| {
            model
                .spot_prices_micros
                .get(&leg.underlying_instrument_id)
                .copied()
        })
        .ok_or(UnavailableReason::MissingModelSpot)?;
    let volatility = model
        .implied_volatilities_ppm
        .get(&leg.instrument_id)
        .copied()
        .ok_or(UnavailableReason::MissingImpliedVolatility)?
        .checked_add(volatility_shift_ppm)
        .ok_or(UnavailableReason::MissingImpliedVolatility)?;
    let rate = model
        .risk_free_rate_ppm
        .ok_or(UnavailableReason::MissingRiskFreeRate)?;
    let dividend = model
        .dividend_yields_ppm
        .get(&leg.underlying_instrument_id)
        .copied()
        .ok_or(UnavailableReason::MissingDividendYield)?;
    let time = model
        .time_to_expiry_seconds
        .get(&leg.instrument_id)
        .copied()
        .ok_or(UnavailableReason::MissingTimeToExpiry)?
        .checked_sub(elapsed_seconds)
        .ok_or(UnavailableReason::MissingTimeToExpiry)?;
    if spot <= 0 || volatility <= 0 || time <= 0 || contract.strike_micros <= 0 {
        return Err(UnavailableReason::MissingTimeToExpiry);
    }
    Ok(ModelInputs {
        spot: spot as f64 / SCALE as f64,
        strike: contract.strike_micros as f64 / SCALE as f64,
        volatility: volatility as f64 / SCALE as f64,
        rate: rate as f64 / SCALE as f64,
        dividend: dividend as f64 / SCALE as f64,
        time_years: time as f64 / YEAR_SECONDS,
    })
}

fn black_scholes(inputs: &ModelInputs, right: &OptionRight) -> Result<ModelPrice, AnalysisError> {
    let root_time = inputs.time_years.sqrt();
    let d1 = ((inputs.spot / inputs.strike).ln()
        + (inputs.rate - inputs.dividend + 0.5 * inputs.volatility.powi(2)) * inputs.time_years)
        / (inputs.volatility * root_time);
    let d2 = d1 - inputs.volatility * root_time;
    let discount_rate = (-inputs.rate * inputs.time_years).exp();
    let discount_dividend = (-inputs.dividend * inputs.time_years).exp();
    let pdf = normal_pdf(d1);
    let (value, delta, theta, rho) = match right {
        OptionRight::Call => (
            inputs.spot * discount_dividend * normal_cdf(d1)
                - inputs.strike * discount_rate * normal_cdf(d2),
            discount_dividend * normal_cdf(d1),
            -(inputs.spot * discount_dividend * pdf * inputs.volatility) / (2.0 * root_time)
                - inputs.rate * inputs.strike * discount_rate * normal_cdf(d2)
                + inputs.dividend * inputs.spot * discount_dividend * normal_cdf(d1),
            inputs.strike * inputs.time_years * discount_rate * normal_cdf(d2),
        ),
        OptionRight::Put => (
            inputs.strike * discount_rate * normal_cdf(-d2)
                - inputs.spot * discount_dividend * normal_cdf(-d1),
            discount_dividend * (normal_cdf(d1) - 1.0),
            -(inputs.spot * discount_dividend * pdf * inputs.volatility) / (2.0 * root_time)
                + inputs.rate * inputs.strike * discount_rate * normal_cdf(-d2)
                - inputs.dividend * inputs.spot * discount_dividend * normal_cdf(-d1),
            -inputs.strike * inputs.time_years * discount_rate * normal_cdf(-d2),
        ),
    };
    let gamma = discount_dividend * pdf / (inputs.spot * inputs.volatility * root_time);
    let vega = inputs.spot * discount_dividend * pdf * root_time;
    Ok(ModelPrice {
        value_micros: f64_to_scaled(value.max(0.0))?,
        greeks: GreekValuesPpm {
            delta: Some(f64_to_scaled(delta)?),
            gamma: Some(f64_to_scaled(gamma)?),
            theta: Some(f64_to_scaled(theta / 365.25)?),
            vega: Some(f64_to_scaled(vega / 100.0)?),
            rho: Some(f64_to_scaled(rho / 100.0)?),
        },
    })
}

fn normal_pdf(value: f64) -> f64 {
    (-0.5 * value * value).exp() / (2.0 * PI).sqrt()
}

fn normal_cdf(value: f64) -> f64 {
    let x = value.abs();
    let t = 1.0 / (1.0 + 0.231_641_9 * x);
    let polynomial = t
        * (0.319_381_530
            + t * (-0.356_563_782
                + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    let cdf = 1.0 - normal_pdf(x) * polynomial;
    if value >= 0.0 {
        cdf
    } else {
        1.0 - cdf
    }
}

fn greek_values(greeks: &OptionGreeks) -> Result<GreekValuesPpm, AnalysisError> {
    Ok(GreekValuesPpm {
        delta: greeks.delta.map(f64_to_scaled).transpose()?,
        gamma: greeks.gamma.map(f64_to_scaled).transpose()?,
        theta: greeks.theta.map(f64_to_scaled).transpose()?,
        vega: greeks.vega.map(f64_to_scaled).transpose()?,
        rho: greeks.rho.map(f64_to_scaled).transpose()?,
    })
}

fn parse_timestamp_ms(value: &str) -> Result<i64, AnalysisError> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.timestamp_millis())
        .map_err(|_| AnalysisError::IntegrityFailure("observation_timestamp"))
}

fn f64_to_scaled(value: f64) -> Result<i64, AnalysisError> {
    if !value.is_finite() {
        return Err(AnalysisError::InvalidRequest("non_finite_value"));
    }
    let scaled = (value * SCALE as f64).round();
    if scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
        return Err(AnalysisError::ArithmeticOverflow);
    }
    Ok(scaled as i64)
}

fn position_value(
    price_micros: i64,
    quantity_micros: i64,
    multiplier_micros: i64,
) -> Result<i64, AnalysisError> {
    let value = i128::from(price_micros)
        .checked_mul(i128::from(quantity_micros))
        .and_then(|value| value.checked_mul(i128::from(multiplier_micros)))
        .ok_or(AnalysisError::ArithmeticOverflow)?
        / i128::from(SCALE).pow(2);
    i64::try_from(value).map_err(|_| AnalysisError::ArithmeticOverflow)
}

fn position_pnl(
    value_micros: i64,
    entry_micros: i64,
    quantity_micros: i64,
    multiplier_micros: i64,
) -> Result<i64, AnalysisError> {
    let change = value_micros
        .checked_sub(entry_micros)
        .ok_or(AnalysisError::ArithmeticOverflow)?;
    position_value(change, quantity_micros, multiplier_micros)
}

fn intrinsic_value(spot: i64, strike: i64, right: &OptionRight) -> i64 {
    match right {
        OptionRight::Call => spot.saturating_sub(strike).max(0),
        OptionRight::Put => strike.saturating_sub(spot).max(0),
    }
}

fn shock_price(price: i64, shock_bps: i32) -> Result<i64, AnalysisError> {
    let numerator = i128::from(price)
        .checked_mul(i128::from(10_000 + shock_bps))
        .ok_or(AnalysisError::ArithmeticOverflow)?;
    if numerator < 0 {
        return Err(AnalysisError::InvalidRequest("negative_shocked_price"));
    }
    i64::try_from(numerator / 10_000).map_err(|_| AnalysisError::ArithmeticOverflow)
}

fn aggregate_available<'a>(
    values: impl Iterator<Item = &'a AvailableValue>,
    provenance: &str,
) -> Result<AvailableValue, AnalysisError> {
    let values = values.collect::<Vec<_>>();
    if let Some(reason) = values.iter().find_map(|value| match value {
        AvailableValue::Unavailable { reason } => Some(*reason),
        _ => None,
    }) {
        return Ok(AvailableValue::Unavailable { reason });
    }
    Ok(AvailableValue::Available {
        value_micros: values.iter().try_fold(0_i64, |sum, value| {
            checked_add(sum, value.value().unwrap_or_default())
        })?,
        provenance: provenance.to_string(),
    })
}

fn sum_available<'a>(
    values: impl Iterator<Item = &'a AvailableValue>,
) -> Result<Option<i64>, AnalysisError> {
    let mut sum = 0_i64;
    for value in values {
        let Some(value) = value.value() else {
            return Ok(None);
        };
        sum = checked_add(sum, value)?;
    }
    Ok(Some(sum))
}

fn checked_add(left: i64, right: i64) -> Result<i64, AnalysisError> {
    left.checked_add(right)
        .ok_or(AnalysisError::ArithmeticOverflow)
}

fn unavailable_risk(reason: UnavailableReason) -> PayoffRisk {
    PayoffRisk {
        minimum_pnl_micros: PnlBound::Unavailable(reason),
        maximum_pnl_micros: PnlBound::Unavailable(reason),
    }
}

fn inactive_leg_value(
    analysis: &LegAnalysis,
    unavailable: &mut Vec<UnavailableCalculation>,
    calculation: &str,
) -> AvailableValue {
    if !analysis.lifecycle_complete {
        return unavailable_value(
            unavailable,
            &analysis.leg_id,
            calculation,
            UnavailableReason::MissingLifecycleFacts,
        );
    }
    let value_micros = if analysis.lifecycle.state == LifecycleState::TerminalValued {
        analysis.lifecycle.terminal_value_micros.unwrap_or_default()
    } else {
        0
    };
    AvailableValue::Available {
        value_micros,
        provenance: analysis
            .lifecycle
            .evidence_refs
            .first()
            .map(|reference| reference.content_hash.clone())
            .unwrap_or_default(),
    }
}

fn unavailable_value(
    unavailable: &mut Vec<UnavailableCalculation>,
    scope: &str,
    calculation: &str,
    reason: UnavailableReason,
) -> AvailableValue {
    push_unavailable(unavailable, scope, calculation, reason);
    AvailableValue::Unavailable { reason }
}

fn push_unavailable(
    unavailable: &mut Vec<UnavailableCalculation>,
    scope: &str,
    calculation: &str,
    reason: UnavailableReason,
) {
    unavailable.push(UnavailableCalculation {
        scope: scope.to_string(),
        calculation: calculation.to_string(),
        reason,
    });
}

fn valid_provenance(provenance: &Provenance) -> bool {
    !provenance.source_ref.trim().is_empty()
        && provenance.content_hash.starts_with("sha256:")
        && provenance.content_hash.len() > "sha256:".len()
}

fn sorted_legs(legs: &[StrategyLeg]) -> Vec<&StrategyLeg> {
    let mut legs = legs.iter().collect::<Vec<_>>();
    legs.sort_by(|left, right| left.leg_id.cmp(&right.leg_id));
    legs
}

fn sorted_scenarios(scenarios: &[DeclaredScenario]) -> Vec<&DeclaredScenario> {
    let mut scenarios = scenarios.iter().collect::<Vec<_>>();
    scenarios.sort_by(|left, right| left.scenario_id.cmp(&right.scenario_id));
    scenarios
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest_accounting::{AccountingFill, FillSide, Position, PositionLot};
    use crate::backtest_report::{
        BacktestCostsAndRisk, BacktestInstrumentDetails, BacktestReportAssumptions,
        BacktestReportChart, BacktestReportMetadata, BacktestReportOverview, BacktestReportRun,
        BacktestTrade, BACKTEST_REPORT_SCHEMA,
    };
    use crate::domain::BacktestRunState;
    use crate::historical_data::{
        AssetClass, DatasetNormalizationPolicy, DatasetQualityPolicy, DatasetSnapshotContent,
        DatasetSourceClass, DatasetSourceManifest, DatasetTimeSlice, HistoricalDataKind,
        HistoricalObservation, QualityDisposition, DATASET_SNAPSHOT_SCHEMA,
    };
    use serde_json::json;

    fn provenance(id: &str) -> Provenance {
        Provenance {
            source_ref: format!("tradeassembly://evidence/{id}"),
            content_hash: format!("sha256:{id}"),
        }
    }

    fn report_provenance(report: &BacktestReport) -> Provenance {
        Provenance {
            source_ref: format!("tradeassembly://backtests/{}/report", report.run.run_id),
            content_hash: report.report_hash.clone(),
        }
    }

    fn option(
        symbol: &str,
        strike: f64,
        right: OptionRight,
        greeks: bool,
    ) -> HistoricalObservation {
        HistoricalObservation {
            instrument_id: symbol.to_string(),
            timestamp: "2026-07-23T12:00:00Z".to_string(),
            data: HistoricalObservationData::OptionContract(Box::new(OptionContractObservation {
                contract_symbol: symbol.to_string(),
                underlying_instrument_id: "SPY".to_string(),
                expiration: "2026-08-21".to_string(),
                strike,
                right,
                bid: Some(4.0),
                ask: Some(4.2),
                last: Some(4.1),
                volume: Some(100.0),
                open_interest: Some(1_000.0),
                implied_volatility: Some(0.2),
                greeks: greeks.then_some(OptionGreeks {
                    delta: Some(0.55),
                    gamma: Some(0.02),
                    theta: Some(-0.08),
                    vega: Some(0.15),
                    rho: None,
                }),
            })),
        }
    }

    fn snapshot() -> DatasetSnapshot {
        DatasetSnapshot::build(
            DatasetSnapshotContent {
                schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
                strategy_id: "strategy".to_string(),
                strategy_version_id: Some("version".to_string()),
                source: DatasetSourceManifest {
                    source_class: DatasetSourceClass::Test,
                    plugin_instance_ref: "instance".to_string(),
                    plugin_ref: "plugin".to_string(),
                    operation_id: "option_chain.read_v1".to_string(),
                    plugin_manifest_fingerprint: "sha256:plugin".to_string(),
                    capability_graph_revision_id: "graph".to_string(),
                    capability_graph_fingerprint: "sha256:graph".to_string(),
                    source_request_hash: "sha256:request".to_string(),
                    upstream_refs: BTreeMap::new(),
                },
                asset_classes: vec![AssetClass::Option],
                instruments: vec![
                    "SPY".to_string(),
                    "SPY-C100".to_string(),
                    "SPY-P90".to_string(),
                ],
                data_kind: HistoricalDataKind::OptionChain,
                granularity: "1m".to_string(),
                time_slice: DatasetTimeSlice {
                    start: "2026-07-23T00:00:00Z".to_string(),
                    end: "2026-07-24T00:00:00Z".to_string(),
                },
                calendar: "XNYS".to_string(),
                timezone: "UTC".to_string(),
                normalization_policy: DatasetNormalizationPolicy {
                    timestamp_unit: "rfc3339".to_string(),
                    timezone: "UTC".to_string(),
                    duplicate_policy: "reject".to_string(),
                    price_adjustment: "raw".to_string(),
                },
                quality_policy: DatasetQualityPolicy {
                    missing_intervals: QualityDisposition::Warn,
                    stale_observations: QualityDisposition::Warn,
                    outliers: QualityDisposition::Reject,
                    invalid_markets: QualityDisposition::Reject,
                    calendar_mismatch: QualityDisposition::Reject,
                    corporate_action_gaps: QualityDisposition::Warn,
                    missing_derivative_fields: QualityDisposition::Warn,
                },
                quality_findings: Vec::new(),
                observations: vec![
                    option("SPY-C100", 100.0, OptionRight::Call, true),
                    option("SPY-P90", 90.0, OptionRight::Put, false),
                ],
            },
            1,
        )
        .unwrap()
    }

    fn report(snapshot: &DatasetSnapshot, accounting: &AccountingResult) -> BacktestReport {
        let mut report = BacktestReport {
            schema: BACKTEST_REPORT_SCHEMA.to_string(),
            report_hash: String::new(),
            run: BacktestReportRun {
                run_id: "run".to_string(),
                state: BacktestRunState::Completed,
                strategy_id: "strategy".to_string(),
                strategy_version_id: "version".to_string(),
                strategy_spec_hash: "sha256:spec".to_string(),
                created_at_ms: 1,
                completed_at_ms: 2,
                dataset_id: snapshot.dataset_id.clone(),
                dataset_time_slice: json!({}),
                instruments: snapshot.content.instruments.clone(),
                source_plugin_ref: "plugin".to_string(),
                source_operation_id: "option_chain.read_v1".to_string(),
                source_manifest_fingerprint: "sha256:plugin".to_string(),
            },
            assumptions: BacktestReportAssumptions {
                starting_cash_micros: 1_000_000,
                reporting_currency: "USD".to_string(),
                account_model: json!("cash"),
                execution: json!({}),
                costs: json!({}),
                liquidity: json!({}),
                risk: json!({}),
                end_of_data: json!("mark_to_market"),
                evaluator_version: "1".to_string(),
                compiler_version: "1".to_string(),
            },
            overview: BacktestReportOverview {
                starting_equity_micros: 1_000_000,
                ending_cash_micros: accounting.ending_cash_micros,
                market_value_micros: accounting.market_value_micros,
                ending_equity_micros: accounting.ending_equity_micros,
                gross_pnl_micros: 0,
                net_pnl_micros: 0,
                realized_pnl_micros: accounting.realized_pnl_micros,
                unrealized_pnl_micros: accounting.unrealized_pnl_micros,
                return_bps: Some(0),
                trade_count: 0,
                win_count: 0,
                loss_count: 0,
                wash_count: 0,
                open_count: accounting.positions.len() as u64,
                fees_micros: 0,
                cost_drag_micros: 0,
                reconciliation_status: "reconciled".to_string(),
                reconciliation_residual_micros: 0,
            },
            chart: BacktestReportChart {
                range: json!({}),
                bars: Vec::new(),
                markers: Vec::new(),
                connectors: Vec::new(),
                warnings: Vec::new(),
                evidence_refs: Vec::new(),
            },
            trades: Vec::new(),
            journal: Vec::new(),
            orders: Vec::new(),
            fills: Vec::new(),
            positions: Vec::new(),
            ledger: Vec::new(),
            costs_and_risk: BacktestCostsAndRisk {
                fees_micros: 0,
                cost_drag_micros: 0,
                maximum_loss_micros: 1,
                maximum_position_notional_micros: 1,
                maximum_order_quantity_micros: 1,
                unavailable: Vec::new(),
            },
            diagnostics: Vec::new(),
            metadata: BacktestReportMetadata {
                manifest_hash: "sha256:manifest".to_string(),
                result_hash: "sha256:result".to_string(),
                dataset_hash: snapshot.content_hash.clone(),
                lifecycle_event_hashes: Vec::new(),
                current_attempt_id: "attempt".to_string(),
                replay_ref: "tradeassembly://backtests/run/replay".to_string(),
                available_export_formats: vec!["reportJson".to_string()],
                warnings: Vec::new(),
            },
            provenance: crate::backtest_report::BacktestReportProvenance::from_snapshot(
                snapshot,
                "version",
                "sha256:spec",
                json!({
                    "status": "unavailable",
                    "reason": "verified immutable strategy spec was not available",
                    "indicators": [],
                    "rules": [],
                }),
            ),
        };
        report.report_hash = canonical_hash(&report, "hash").unwrap();
        report
    }

    fn accounting() -> AccountingResult {
        let positions = BTreeMap::from([
            (
                "SPY".to_string(),
                Position {
                    symbol: "SPY".to_string(),
                    quantity: SCALE,
                    average_cost_micros: 95 * SCALE,
                    market_price_micros: 100 * SCALE,
                    market_value_micros: 100 * SCALE,
                    unrealized_pnl_micros: 5 * SCALE,
                    lots: vec![PositionLot {
                        lot_id: "stock".to_string(),
                        symbol: "SPY".to_string(),
                        quantity: SCALE,
                        unit_cost_micros: 95 * SCALE,
                    }],
                },
            ),
            (
                "SPY-C100".to_string(),
                Position {
                    symbol: "SPY-C100".to_string(),
                    quantity: SCALE,
                    average_cost_micros: 4 * SCALE,
                    market_price_micros: 4_100_000,
                    market_value_micros: 4_100_000,
                    unrealized_pnl_micros: 100_000,
                    lots: vec![],
                },
            ),
        ]);
        AccountingResult {
            fills: Vec::<AccountingFill>::new(),
            ledger: Vec::new(),
            positions,
            ending_cash_micros: 0,
            market_value_micros: 104_100_000,
            ending_equity_micros: 104_100_000,
            realized_pnl_micros: 0,
            unrealized_pnl_micros: 5_100_000,
            income_micros: 0,
            costs_micros: 0,
            reconciliation_residual_micros: 0,
        }
    }

    fn instruments() -> Vec<InstrumentConfiguration> {
        vec![
            instrument("SPY", "equity", SCALE, "cash"),
            instrument("SPY-C100", "option_contract", 100 * SCALE, "physical"),
            instrument("SPY-P90", "option_contract", 100 * SCALE, "cash"),
        ]
    }

    fn instrument(
        id: &str,
        family: &str,
        multiplier: i64,
        settlement: &str,
    ) -> InstrumentConfiguration {
        InstrumentConfiguration {
            instrument_id: id.to_string(),
            instrument_family: family.to_string(),
            quantity_unit: "contract".to_string(),
            quantity_scale: 6,
            price_scale: 6,
            contract_multiplier_micros: multiplier,
            settlement: settlement.to_string(),
            metadata: BTreeMap::new(),
        }
    }

    fn request(snapshot: &DatasetSnapshot, report: &BacktestReport) -> DerivativesAnalysisRequest {
        DerivativesAnalysisRequest {
            schema: DERIVATIVES_REQUEST_SCHEMA.to_string(),
            engine_version: "gc13-test".to_string(),
            as_of_ms: DateTime::parse_from_rfc3339("2026-07-23T12:05:00Z")
                .unwrap()
                .timestamp_millis(),
            max_mark_age_ms: 600_000,
            max_greek_age_ms: 600_000,
            source_bindings: SourceBindings {
                run_id: "run".to_string(),
                strategy_id: "strategy".to_string(),
                strategy_version_id: "version".to_string(),
                strategy_spec_hash: "sha256:spec".to_string(),
                manifest_hash: "sha256:manifest".to_string(),
                result_hash: "sha256:result".to_string(),
                report_hash: report.report_hash.clone(),
                dataset_hash: snapshot.content_hash.clone(),
                accounting_hash: accounting_content_hash(&accounting()).unwrap(),
                instrument_configurations_hash: instrument_configurations_hash(&instruments())
                    .unwrap(),
            },
            legs: vec![
                StrategyLeg {
                    leg_id: "call".to_string(),
                    position_group_id: "mixed".to_string(),
                    instrument_id: "SPY-C100".to_string(),
                    underlying_instrument_id: "SPY".to_string(),
                    instrument_kind: InstrumentKind::Option,
                    quantity_micros: SCALE,
                    entry_price_micros: Some(4 * SCALE),
                    report_trade_ids: Vec::new(),
                    lifecycle: LifecycleEvidence {
                        state: LifecycleState::Open,
                        occurred_at_ms: None,
                        settlement_price_micros: None,
                        settlement_cashflow_micros: None,
                        terminal_value_micros: None,
                        evidence_refs: Vec::new(),
                    },
                },
                StrategyLeg {
                    leg_id: "stock".to_string(),
                    position_group_id: "mixed".to_string(),
                    instrument_id: "SPY".to_string(),
                    underlying_instrument_id: "SPY".to_string(),
                    instrument_kind: InstrumentKind::Underlying,
                    quantity_micros: SCALE,
                    entry_price_micros: Some(95 * SCALE),
                    report_trade_ids: Vec::new(),
                    lifecycle: LifecycleEvidence {
                        state: LifecycleState::Open,
                        occurred_at_ms: None,
                        settlement_price_micros: None,
                        settlement_cashflow_micros: None,
                        terminal_value_micros: None,
                        evidence_refs: Vec::new(),
                    },
                },
            ],
            source_marks: BTreeMap::from([(
                "SPY".to_string(),
                SourcedPrice {
                    price_micros: 100 * SCALE,
                    observed_at_ms: DateTime::parse_from_rfc3339("2026-07-23T12:00:00Z")
                        .unwrap()
                        .timestamp_millis(),
                    provenance: Provenance {
                        source_ref: "tradeassembly://backtests/run/accounting".to_string(),
                        content_hash: accounting_content_hash(&accounting()).unwrap(),
                    },
                },
            )]),
            scenarios: vec![DeclaredScenario {
                scenario_id: "terminal-up".to_string(),
                kind: ScenarioKind::Terminal,
                underlying_prices_micros: BTreeMap::from([("SPY".to_string(), 110 * SCALE)]),
                volatility_shifts_ppm: BTreeMap::new(),
                elapsed_seconds: 0,
                declaration: provenance("scenario"),
            }],
            sensitivity_grid: None,
            pricing_model: None,
        }
    }

    fn model() -> VersionedPricingModel {
        VersionedPricingModel {
            model: BLACK_SCHOLES_MERTON_MODEL.to_string(),
            version: BLACK_SCHOLES_MERTON_VERSION.to_string(),
            risk_free_rate_ppm: Some(50_000),
            dividend_yields_ppm: BTreeMap::from([("SPY".to_string(), 10_000)]),
            implied_volatilities_ppm: BTreeMap::from([
                ("SPY-C100".to_string(), 200_000),
                ("SPY-P90".to_string(), 220_000),
            ]),
            spot_prices_micros: BTreeMap::from([("SPY".to_string(), 100 * SCALE)]),
            time_to_expiry_seconds: BTreeMap::from([
                ("SPY-C100".to_string(), 30 * 86_400),
                ("SPY-P90".to_string(), 30 * 86_400),
            ]),
            input_provenance: vec![provenance("model-inputs")],
        }
    }

    #[test]
    fn mixed_underlying_and_option_terminal_payoff_is_leg_and_group_scoped() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let result = analyze(
            &request(&snapshot, &report),
            &snapshot,
            &instruments(),
            &accounting,
            &report,
        )
        .unwrap();
        assert_eq!(result.legs.len(), 2);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(
            result.scenarios[0].aggregate_value.value(),
            Some(1_110_000_000)
        );
        assert_eq!(result.scenarios[0].aggregate_pnl.value(), Some(615_000_000));
        assert!(matches!(
            result.groups[0].payoff_risk.maximum_pnl_micros,
            PnlBound::UnboundedPositive
        ));
        assert_eq!(
            result.accounting_reconciliation.status,
            ReconciliationStatus::Reconciled
        );
    }

    #[test]
    fn source_greeks_preserve_hash_provenance_and_staleness() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut request = request(&snapshot, &report);
        request.max_greek_age_ms = 1;
        let result = analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
        let greeks = result
            .legs
            .iter()
            .find(|leg| leg.leg_id == "call")
            .unwrap()
            .source_greeks
            .as_ref()
            .unwrap();
        assert!(greeks.stale);
        assert_eq!(greeks.source_content_hash, snapshot.content_hash);
        assert_eq!(greeks.values_ppm.delta, Some(550_000));
        assert!(result
            .unavailable
            .iter()
            .any(|item| item.reason == UnavailableReason::StaleGreeks));
    }

    #[test]
    fn underlying_marks_must_match_fresh_hash_bound_accounting_evidence() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut fabricated = request(&snapshot, &report);
        fabricated.source_marks.get_mut("SPY").unwrap().price_micros += SCALE;
        let result = analyze(&fabricated, &snapshot, &instruments(), &accounting, &report).unwrap();
        assert!(matches!(
            result
                .legs
                .iter()
                .find(|leg| leg.leg_id == "stock")
                .unwrap()
                .mark,
            AvailableValue::Unavailable {
                reason: UnavailableReason::SourceFactMismatch
            }
        ));

        let mut stale = request(&snapshot, &report);
        stale.source_marks.get_mut("SPY").unwrap().observed_at_ms = 1;
        let result = analyze(&stale, &snapshot, &instruments(), &accounting, &report).unwrap();
        assert!(result
            .unavailable
            .iter()
            .any(|item| item.reason == UnavailableReason::StaleQuote));
    }

    #[test]
    fn complete_explicit_model_produces_price_volatility_and_time_sensitivity() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut request = request(&snapshot, &report);
        request.pricing_model = Some(model());
        request.scenarios[0].kind = ScenarioKind::HypotheticalStress;
        request.sensitivity_grid = Some(SensitivityGrid {
            price_shocks_bps: vec![-500, 0, 500],
            volatility_shifts_ppm: vec![-20_000, 20_000],
            elapsed_seconds: vec![0, 86_400],
        });
        let first = analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
        let second = analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
        let model_hash = pricing_model_hash(request.pricing_model.as_ref().unwrap()).unwrap();
        assert_eq!(first.sensitivity.len(), 12);
        assert_eq!(
            first.pricing_model_hash.as_deref(),
            Some(model_hash.as_str())
        );
        assert_eq!(first.output_hash, second.output_hash);
        assert!(first.sensitivity.iter().all(|point| matches!(
            &point.aggregate_value,
            AvailableValue::Available { provenance, .. } if provenance == &model_hash
        )));
        let scenario_hash = model_scenario_hash(
            request.pricing_model.as_ref().unwrap(),
            &request.scenarios[0].declaration,
        )
        .unwrap();
        assert!(matches!(
            &first.scenarios[0].legs[0].value,
            AvailableValue::Available { provenance, .. } if provenance == &scenario_hash
        ));
    }

    #[test]
    fn model_pathway_fails_closed_for_every_required_input() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut request = request(&snapshot, &report);
        request.scenarios[0].kind = ScenarioKind::HypotheticalStress;
        request.pricing_model = Some(model());
        type RemoveModelInput = fn(&mut VersionedPricingModel);
        let cases: Vec<(UnavailableReason, RemoveModelInput)> = vec![
            (UnavailableReason::MissingRiskFreeRate, |model| {
                model.risk_free_rate_ppm = None
            }),
            (UnavailableReason::MissingDividendYield, |model| {
                model.dividend_yields_ppm.clear()
            }),
            (UnavailableReason::MissingImpliedVolatility, |model| {
                model.implied_volatilities_ppm.clear()
            }),
            (UnavailableReason::MissingModelSpot, |model| {
                model.spot_prices_micros.clear()
            }),
            (UnavailableReason::MissingTimeToExpiry, |model| {
                model.time_to_expiry_seconds.clear()
            }),
        ];
        for (reason, remove) in cases {
            let mut candidate = request.clone();
            remove(candidate.pricing_model.as_mut().unwrap());
            if reason == UnavailableReason::MissingModelSpot {
                candidate.scenarios[0].underlying_prices_micros.clear();
            }
            let result =
                analyze(&candidate, &snapshot, &instruments(), &accounting, &report).unwrap();
            assert!(
                result.unavailable.iter().any(|item| item.reason == reason),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn absent_contract_quote_multiplier_and_settlement_are_explicit() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut missing_contract_request = request(&snapshot, &report);
        missing_contract_request.legs[0].instrument_id = "UNKNOWN".to_string();
        let result = analyze(
            &missing_contract_request,
            &snapshot,
            &instruments(),
            &accounting,
            &report,
        )
        .unwrap();
        assert!(result
            .unavailable
            .iter()
            .any(|item| item.reason == UnavailableReason::MissingContractMetadata));
        assert!(result
            .unavailable
            .iter()
            .any(|item| item.reason == UnavailableReason::MissingQuote));

        let mut incomplete = instruments();
        incomplete.retain(|item| item.instrument_id != "SPY-C100");
        let mut incomplete_request = request(&snapshot, &report);
        incomplete_request
            .source_bindings
            .instrument_configurations_hash = instrument_configurations_hash(&incomplete).unwrap();
        let result = analyze(
            &incomplete_request,
            &snapshot,
            &incomplete,
            &accounting,
            &report,
        )
        .unwrap();
        assert!(result
            .unavailable
            .iter()
            .any(|item| item.reason == UnavailableReason::MissingMultiplier));
        assert!(result
            .unavailable
            .iter()
            .any(|item| item.reason == UnavailableReason::MissingSettlementFacts));
    }

    #[test]
    fn lifecycle_states_remain_distinct_and_missing_facts_fail_closed() {
        let snapshot = snapshot();
        let accounting = accounting();
        for state in [
            LifecycleState::Exercised,
            LifecycleState::Assigned,
            LifecycleState::Expired,
            LifecycleState::Settled,
            LifecycleState::TerminalValued,
        ] {
            let mut report = report(&snapshot, &accounting);
            let evidence = json!({
                "lifecycleState": lifecycle_state_name(state),
                "occurredAtMs": 1,
                "settlementPriceMicros": 100 * SCALE,
                "settlementCashflowMicros": 0,
                "quantityMicros": SCALE,
                "priceMicros": 0,
            });
            report.trades.push(BacktestTrade {
                trade_id: "lifecycle".to_string(),
                symbol: "SPY-C100".to_string(),
                instrument_type: "option".to_string(),
                instrument_details: BacktestInstrumentDetails {
                    schema: "tradeassembly.instrument.option-contract.v1".to_string(),
                    instrument_id: "SPY-C100".to_string(),
                    asset_class: Some(AssetClass::Option),
                    observed_at: None,
                    fields: Vec::new(),
                },
                side: "long".to_string(),
                quantity_micros: SCALE,
                entry: json!({}),
                exit: (state != LifecycleState::TerminalValued).then_some(evidence.clone()),
                terminal_valuation: (state == LifecycleState::TerminalValued).then_some(evidence),
                realized: state != LifecycleState::TerminalValued,
                pnl_micros: Some(0),
                fee_micros: 0,
                holding_interval_ms: Some(1),
                order_refs: Vec::new(),
                fill_refs: Vec::new(),
                ledger_refs: Vec::new(),
            });
            report.report_hash.clear();
            report.report_hash = canonical_hash(&report, "hash").unwrap();
            let mut request = request(&snapshot, &report);
            request.legs.truncate(1);
            request.legs[0].report_trade_ids = vec!["lifecycle".to_string()];
            request.legs[0].lifecycle.state = state;
            request.legs[0].lifecycle.occurred_at_ms = Some(1);
            request.legs[0].lifecycle.settlement_price_micros = Some(100 * SCALE);
            request.legs[0].lifecycle.settlement_cashflow_micros = Some(0);
            request.legs[0].lifecycle.terminal_value_micros = Some(0);
            request.legs[0].lifecycle.evidence_refs = vec![report_provenance(&report)];
            let result =
                analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
            assert_eq!(result.legs[0].lifecycle.state, state);
            assert!(result.legs[0].lifecycle_complete);
            assert!(!matches!(
                result.legs[0].payoff_risk.minimum_pnl_micros,
                PnlBound::UnboundedNegative | PnlBound::UnboundedPositive
            ));
            assert_eq!(result.scenarios[0].legs[0].value.value(), Some(0));
        }
        let report = report(&snapshot, &accounting);
        let mut missing = request(&snapshot, &report);
        missing.legs[0].lifecycle.state = LifecycleState::Assigned;
        missing.legs[0].lifecycle.occurred_at_ms = Some(1);
        missing.legs[0].lifecycle.settlement_price_micros = Some(100 * SCALE);
        missing.legs[0].lifecycle.evidence_refs = vec![provenance("forged-lifecycle")];
        let result = analyze(&missing, &snapshot, &instruments(), &accounting, &report).unwrap();
        assert!(!result.legs[0].lifecycle_complete);
        assert!(result
            .unavailable
            .iter()
            .any(|item| item.reason == UnavailableReason::MissingLifecycleFacts));
    }

    #[test]
    fn closed_leg_uses_accepted_realized_pnl_and_has_no_future_exposure() {
        let snapshot = snapshot();
        let mut accounting = accounting();
        accounting.positions.remove("SPY-C100");
        accounting.market_value_micros = 100 * SCALE;
        accounting.ending_equity_micros = 106 * SCALE;
        accounting.realized_pnl_micros = 6 * SCALE;
        accounting.unrealized_pnl_micros = 5 * SCALE;
        let mut report = report(&snapshot, &accounting);
        report.trades.push(BacktestTrade {
            trade_id: "call-close".to_string(),
            symbol: "SPY-C100".to_string(),
            instrument_type: "option".to_string(),
            instrument_details: BacktestInstrumentDetails {
                schema: "tradeassembly.instrument.option-contract.v1".to_string(),
                instrument_id: "SPY-C100".to_string(),
                asset_class: Some(AssetClass::Option),
                observed_at: None,
                fields: Vec::new(),
            },
            side: "long".to_string(),
            quantity_micros: SCALE,
            entry: json!({}),
            exit: Some(json!({
                "lifecycleState": "closed",
                "occurredAtMs": DateTime::parse_from_rfc3339("2026-07-23T12:05:00Z")
                    .unwrap()
                    .timestamp_millis(),
            })),
            terminal_valuation: None,
            realized: true,
            pnl_micros: Some(6 * SCALE),
            fee_micros: 0,
            holding_interval_ms: Some(1),
            order_refs: Vec::new(),
            fill_refs: Vec::new(),
            ledger_refs: Vec::new(),
        });
        report.report_hash.clear();
        report.report_hash = canonical_hash(&report, "hash").unwrap();
        let mut request = request(&snapshot, &report);
        request.source_bindings.accounting_hash = accounting_content_hash(&accounting).unwrap();
        request
            .source_marks
            .get_mut("SPY")
            .unwrap()
            .provenance
            .content_hash = request.source_bindings.accounting_hash.clone();
        request.legs[0].report_trade_ids = vec!["call-close".to_string()];
        request.legs[0].lifecycle = LifecycleEvidence {
            state: LifecycleState::Closed,
            occurred_at_ms: Some(request.as_of_ms),
            settlement_price_micros: None,
            settlement_cashflow_micros: None,
            terminal_value_micros: None,
            evidence_refs: vec![report_provenance(&report)],
        };
        let result = analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
        let call = result.legs.iter().find(|leg| leg.leg_id == "call").unwrap();
        assert_eq!(call.realized_pnl.value(), Some(6 * SCALE));
        assert_eq!(call.unrealized_pnl.value(), Some(0));
        assert_eq!(
            call.payoff_risk,
            PayoffRisk {
                minimum_pnl_micros: PnlBound::Finite(6 * SCALE),
                maximum_pnl_micros: PnlBound::Finite(6 * SCALE),
            }
        );
        assert_eq!(result.scenarios[0].legs[0].value.value(), Some(0));
        assert_eq!(result.scenarios[0].legs[0].pnl.value(), Some(6 * SCALE));
        assert_eq!(
            result.accounting_reconciliation.status,
            ReconciliationStatus::Reconciled
        );
    }

    #[test]
    fn scenario_and_grid_budgets_reject_before_calculation() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut request = request(&snapshot, &report);
        request.scenarios = (0..=MAX_SCENARIOS)
            .map(|index| DeclaredScenario {
                scenario_id: format!("s{index}"),
                kind: ScenarioKind::Terminal,
                underlying_prices_micros: BTreeMap::new(),
                volatility_shifts_ppm: BTreeMap::new(),
                elapsed_seconds: 0,
                declaration: provenance(&format!("s{index}")),
            })
            .collect();
        assert_eq!(
            analyze(&request, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::LimitExceeded("scenarios"))
        );
        request.scenarios.clear();
        request.sensitivity_grid = Some(SensitivityGrid {
            price_shocks_bps: vec![0; 17],
            volatility_shifts_ppm: vec![0; 16],
            elapsed_seconds: vec![0],
        });
        assert_eq!(
            analyze(&request, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::LimitExceeded("sensitivity_grid_axis"))
        );
    }

    #[test]
    fn configured_derivatives_bounds_and_duplicates_fail_closed() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut request = request(&snapshot, &report);

        request.max_mark_age_ms = MAX_EVIDENCE_AGE_MS + 1;
        assert_eq!(
            analyze(&request, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::LimitExceeded("evidence_age"))
        );
        request.max_mark_age_ms = 0;
        request.sensitivity_grid = Some(SensitivityGrid {
            price_shocks_bps: vec![0, 0],
            volatility_shifts_ppm: vec![0],
            elapsed_seconds: vec![0],
        });
        assert_eq!(
            analyze(&request, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::InvalidRequest("sensitivity_grid"))
        );
        request.sensitivity_grid = Some(SensitivityGrid {
            price_shocks_bps: vec![MAX_PRICE_SHOCK_BPS + 1],
            volatility_shifts_ppm: vec![0],
            elapsed_seconds: vec![0],
        });
        assert_eq!(
            analyze(&request, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::InvalidRequest("sensitivity_grid"))
        );
        request.sensitivity_grid = Some(SensitivityGrid {
            price_shocks_bps: vec![0],
            volatility_shifts_ppm: vec![0],
            elapsed_seconds: vec![MAX_ELAPSED_SECONDS + 1],
        });
        assert_eq!(
            analyze(&request, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::InvalidRequest("sensitivity_grid"))
        );
        request.sensitivity_grid = Some(SensitivityGrid {
            price_shocks_bps: vec![0],
            volatility_shifts_ppm: vec![0],
            elapsed_seconds: vec![0],
        });
        request.scenarios = vec![DeclaredScenario {
            scenario_id: "bounded".to_string(),
            kind: ScenarioKind::HypotheticalStress,
            underlying_prices_micros: BTreeMap::new(),
            volatility_shifts_ppm: BTreeMap::from([(
                "SPY".to_string(),
                MAX_VOLATILITY_SHIFT_PPM + 1,
            )]),
            elapsed_seconds: 0,
            declaration: provenance("bounded"),
        }];
        assert_eq!(
            analyze(&request, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::InvalidRequest("scenario"))
        );
    }

    #[test]
    fn source_substitution_and_tampering_fail_closed() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut substituted_request = request(&snapshot, &report);
        substituted_request.source_bindings.dataset_hash = "sha256:other".to_string();
        assert_eq!(
            analyze(
                &substituted_request,
                &snapshot,
                &instruments(),
                &accounting,
                &report
            ),
            Err(AnalysisError::IntegrityFailure("source_binding"))
        );
        let mut tampered = snapshot.clone();
        tampered.content.strategy_id = "other".to_string();
        assert_eq!(
            analyze(
                &request(&snapshot, &report),
                &tampered,
                &instruments(),
                &accounting,
                &report
            ),
            Err(AnalysisError::IntegrityFailure("dataset"))
        );
    }

    #[test]
    fn caller_cannot_substitute_instrument_kind_or_option_underlying() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);

        let mut wrong_kind = request(&snapshot, &report);
        wrong_kind.legs[0].instrument_kind = InstrumentKind::Underlying;
        assert_eq!(
            analyze(&wrong_kind, &snapshot, &instruments(), &accounting, &report),
            Err(AnalysisError::IntegrityFailure("instrument_kind"))
        );

        let mut wrong_underlying = request(&snapshot, &report);
        wrong_underlying.legs[0].underlying_instrument_id = "QQQ".to_string();
        wrong_underlying.scenarios[0]
            .underlying_prices_micros
            .insert("QQQ".to_string(), 110 * SCALE);
        assert_eq!(
            analyze(
                &wrong_underlying,
                &snapshot,
                &instruments(),
                &accounting,
                &report
            ),
            Err(AnalysisError::IntegrityFailure("underlying_instrument"))
        );
    }

    #[test]
    fn output_hash_binds_source_content_and_canonical_order() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let request = request(&snapshot, &report);
        let first = analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
        let mut reordered = request.clone();
        reordered.legs.reverse();
        let second = analyze(&reordered, &snapshot, &instruments(), &accounting, &report).unwrap();
        assert_eq!(first.output_hash, second.output_hash);
        let mut changed = request;
        changed
            .source_marks
            .get_mut("SPY")
            .unwrap()
            .provenance
            .content_hash = "sha256:changed".to_string();
        let third = analyze(&changed, &snapshot, &instruments(), &accounting, &report).unwrap();
        assert_ne!(first.output_hash, third.output_hash);
    }

    #[test]
    fn negative_quantity_has_unbounded_call_loss_without_advice() {
        let snapshot = snapshot();
        let accounting = accounting();
        let report = report(&snapshot, &accounting);
        let mut request = request(&snapshot, &report);
        request.legs.truncate(1);
        request.legs[0].quantity_micros = -SCALE;
        let result = analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
        assert!(matches!(
            result.legs[0].payoff_risk.minimum_pnl_micros,
            PnlBound::UnboundedNegative
        ));
        let encoded = serde_json::to_string(&result).unwrap().to_lowercase();
        assert!(!encoded.contains("should buy"));
        assert!(!encoded.contains("should sell"));
    }

    #[test]
    fn multi_symbol_risk_keeps_independent_price_axes() {
        let long = StrategyLeg {
            leg_id: "long".to_string(),
            position_group_id: "multi".to_string(),
            instrument_id: "SPY".to_string(),
            underlying_instrument_id: "SPY".to_string(),
            instrument_kind: InstrumentKind::Underlying,
            quantity_micros: SCALE,
            entry_price_micros: Some(100 * SCALE),
            report_trade_ids: Vec::new(),
            lifecycle: LifecycleEvidence {
                state: LifecycleState::Open,
                occurred_at_ms: None,
                settlement_price_micros: None,
                settlement_cashflow_micros: None,
                terminal_value_micros: None,
                evidence_refs: Vec::new(),
            },
        };
        let mut short = long.clone();
        short.leg_id = "short".to_string();
        short.instrument_id = "QQQ".to_string();
        short.underlying_instrument_id = "QQQ".to_string();
        short.quantity_micros = -SCALE;
        let risk = payoff_risk(&[long, short], &BTreeMap::new()).unwrap();
        assert_eq!(risk.minimum_pnl_micros, PnlBound::UnboundedNegative);
        assert_eq!(risk.maximum_pnl_micros, PnlBound::UnboundedPositive);
    }

    #[test]
    fn accounting_mismatch_is_not_silently_reconciled() {
        let snapshot = snapshot();
        let mut accounting = accounting();
        accounting.unrealized_pnl_micros += 1;
        let report = report(&snapshot, &accounting);
        let mut request = request(&snapshot, &report);
        request.legs[0].quantity_micros = 2 * SCALE;
        request.source_bindings.accounting_hash = accounting_content_hash(&accounting).unwrap();
        let result = analyze(&request, &snapshot, &instruments(), &accounting, &report).unwrap();
        assert_ne!(
            result.accounting_reconciliation.status,
            ReconciliationStatus::Reconciled
        );
    }

    #[test]
    fn test_fixture_uses_accepted_fill_side_type() {
        let fill = AccountingFill {
            fill_id: "fill".to_string(),
            symbol: "SPY".to_string(),
            side: FillSide::Buy,
            quantity: SCALE,
            price_micros: SCALE,
            fee_micros: 0,
        };
        assert_eq!(fill.side, FillSide::Buy);
    }
}
