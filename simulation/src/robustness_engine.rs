//! Pure, deterministic robustness projections over verified completed-backtest evidence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const ROBUSTNESS_REQUEST_SCHEMA: &str = "tradeassembly.robustness-request.v1";
pub const ROBUSTNESS_RESULT_SCHEMA: &str = "tradeassembly.robustness-result.v1";
pub const ROBUSTNESS_ENGINE_VERSION: &str = "tradeassembly.robustness-engine.v1";
pub const SYNTHETIC_LOG_RETURN_MODEL_V1: &str = "tradeassembly.synthetic-log-return-model.v1";
pub const PARAMETER_PERTURBATION_MODEL_V1: &str = "tradeassembly.parameter-perturbation-model.v1";

const HARD_MAX_SAMPLES: usize = 100_000;
const HARD_MAX_GRID_POINTS: usize = 10_000;
const HARD_MAX_WINDOWS: usize = 10_000;
const HARD_MAX_SCENARIOS: usize = 10_000;
const HARD_MAX_PATH_POINTS: usize = 2_000_000;
const HARD_MAX_OUTPUT_POINTS: usize = 2_000_000;
const HARD_MAX_MEMORY_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessRequest {
    pub schema: String,
    pub engine_version: String,
    pub seed: u64,
    pub source: VerifiedBacktest,
    #[serde(default)]
    pub supporting_sources: Vec<VerifiedBacktest>,
    pub budgets: StudyBudget,
    pub study: StudyInput,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedBacktest {
    pub verification: VerificationStatus,
    pub bindings: SourceBindings,
    pub units: SourceUnits,
    pub starting_equity_micros: i64,
    pub trades: Vec<ObservedTrade>,
    pub equity: Vec<ObservedEquityPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    IntegrityVerifiedCompleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceBindings {
    pub run_id: String,
    pub manifest_hash: String,
    pub result_hash: String,
    pub report_hash: String,
    pub dataset_hash: String,
    pub strategy_spec_hash: String,
    pub evidence_refs: Vec<String>,
    pub replay_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceUnits {
    pub reporting_currency: String,
    pub equity: Unit,
    pub trade_return: Unit,
    pub cost: Unit,
    pub quantity: Unit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    CurrencyMicros,
    BasisPoints,
    Ratio,
    Quantity,
    Probability,
    Count,
    Steps,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedTrade {
    pub trade_id: String,
    pub sequence: u64,
    pub return_ratio: f64,
    pub pnl_micros: i64,
    pub fee_micros: i64,
    pub slippage_bps: f64,
    pub quantity: f64,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedEquityPoint {
    pub sequence: u64,
    pub equity_micros: i64,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudyBudget {
    pub max_samples: usize,
    pub max_grid_points: usize,
    pub max_windows: usize,
    pub max_scenarios: usize,
    pub max_path_points: usize,
    pub max_output_points: usize,
    pub max_memory_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "study", content = "input", rename_all = "snake_case")]
pub enum StudyInput {
    MonteCarlo(MonteCarloInput),
    ParameterSensitivity(ParameterSensitivityInput),
    FeeSensitivity(CostSensitivityInput),
    SlippageSensitivity(CostSensitivityInput),
    ExecutionSensitivity(CostSensitivityInput),
    WalkForward(WalkForwardInput),
    Regime(RegimeInput),
    Stress(StressInput),
    CapacityLiquidity(CapacityLiquidityInput),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MonteCarloInput {
    pub path_count: usize,
    pub confidence_level: f64,
    pub ruin_equity_ratio: f64,
    pub mode: MonteCarloMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "assumptions", rename_all = "snake_case")]
pub enum MonteCarloMode {
    TradeOrderReshuffle,
    TradeBootstrap,
    ExecutionRandomization {
        minimum_cost_bps: f64,
        maximum_cost_bps: f64,
    },
    ParameterPerturbation {
        model: ParameterPerturbationModel,
    },
    SyntheticPath {
        model: SyntheticPathModel,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterPerturbationModel {
    pub schema: String,
    pub minimum_return_factor: f64,
    pub maximum_return_factor: f64,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyntheticPathModel {
    pub schema: String,
    pub horizon_steps: usize,
    pub mean_log_return_ratio: f64,
    pub volatility_ratio: f64,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterSensitivityInput {
    pub cases: Vec<ParameterCase>,
    pub top_count: usize,
    pub plateau_tolerance_ratio: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterCase {
    pub source_run_id: String,
    pub parameters: BTreeMap<String, f64>,
    pub trade_returns: Vec<f64>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CostSensitivityInput {
    pub baseline_label: String,
    pub cases: Vec<CostCase>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CostCase {
    pub source_run_id: String,
    pub label: String,
    pub fee_bps: f64,
    pub slippage_bps: f64,
    pub execution_model: String,
    pub trade_returns: Vec<f64>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalkForwardInput {
    pub windows: Vec<WalkForwardWindow>,
    pub minimum_oos_return_ratio: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalkForwardWindow {
    pub window_id: String,
    pub in_sample_source_run_id: String,
    pub out_of_sample_source_run_id: String,
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub in_sample_returns: Vec<f64>,
    pub out_of_sample_returns: Vec<f64>,
    pub selected_parameters: BTreeMap<String, f64>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegimeInput {
    pub segmentation_definition: String,
    pub segments: Vec<RegimeSegment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegimeSegment {
    pub source_run_id: String,
    pub regime: String,
    pub trade_returns: Vec<f64>,
    pub costs_micros: i64,
    pub exposure_ratio: Option<f64>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StressInput {
    pub scenarios: Vec<StressScenario>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StressKind {
    HistoricalObserved,
    HypotheticalDeclared,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StressScenario {
    pub name: String,
    pub kind: StressKind,
    pub source_run_id: Option<String>,
    pub shocked_trade_returns: Vec<f64>,
    pub exposure_ratio: Option<f64>,
    pub contribution_micros: BTreeMap<String, i64>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacityLiquidityInput {
    pub observations: Vec<LiquidityObservation>,
    pub scale_points: Vec<f64>,
    pub maximum_participation_ratio: f64,
    pub impact_bps_per_participation_ratio: f64,
    pub reject_above_participation: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiquidityObservation {
    pub source_run_id: String,
    pub trade_id: String,
    pub return_ratio: f64,
    pub quantity: f64,
    pub observed_volume: Option<f64>,
    pub observed_spread_bps: Option<f64>,
    pub open_interest: Option<f64>,
    pub displayed_depth: Option<f64>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudyKind {
    MonteCarlo,
    ParameterSensitivity,
    FeeSensitivity,
    SlippageSensitivity,
    ExecutionSensitivity,
    WalkForward,
    Regime,
    Stress,
    CapacityLiquidity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessResult {
    pub schema: String,
    pub engine_version: String,
    pub seed: u64,
    pub source_bindings: SourceBindings,
    pub supporting_source_bindings: Vec<SourceBindings>,
    pub study: StudyKind,
    pub status: ResultStatus,
    pub output: StudyOutput,
    pub diagnostics: Vec<Diagnostic>,
    pub result_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "study", content = "result", rename_all = "snake_case")]
pub enum StudyOutput {
    MonteCarlo(MonteCarloResult),
    ParameterSensitivity(ParameterSensitivityResult),
    FeeSensitivity(CostSensitivityResult),
    SlippageSensitivity(CostSensitivityResult),
    ExecutionSensitivity(CostSensitivityResult),
    WalkForward(WalkForwardResult),
    Regime(RegimeResult),
    Stress(StressResult),
    CapacityLiquidity(CapacityLiquidityResult),
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub detail: String,
    pub missing_evidence: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactOrigin {
    ObservedBacktest,
    ModelDerived,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metric {
    pub value: f64,
    pub unit: Unit,
    pub origin: FactOrigin,
    pub evidence_refs: Vec<String>,
    pub replay_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfidencePathPoint {
    pub step: usize,
    pub lower: Metric,
    pub median: Metric,
    pub upper: Metric,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfidenceInterval {
    pub confidence_level: f64,
    pub lower: Metric,
    pub upper: Metric,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MonteCarloResult {
    pub path_count: usize,
    pub confidence_path: Vec<ConfidencePathPoint>,
    pub terminal_distribution: Vec<Metric>,
    pub drawdown_distribution: Vec<Metric>,
    pub longest_losing_streak_distribution: Vec<Metric>,
    pub loss_probability: Metric,
    pub ruin_probability: Metric,
    pub terminal_confidence_interval: ConfidenceInterval,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterSensitivityResult {
    pub tested_grid: Vec<ParameterGridPoint>,
    pub top_sets: Vec<ParameterGridPoint>,
    pub plateau_fraction: Metric,
    pub neighbor_stability: Metric,
    pub warnings: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterGridPoint {
    pub parameters: BTreeMap<String, f64>,
    pub return_ratio: Metric,
    pub maximum_drawdown_ratio: Metric,
    pub return_volatility_ratio: Metric,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CostSensitivityResult {
    pub cases: Vec<CostSensitivityPoint>,
    pub break_even_bps: Option<Metric>,
    pub fragility_range_ratio: Metric,
    pub flipped_outcome_count: Metric,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CostSensitivityPoint {
    pub label: String,
    pub fee_bps: Metric,
    pub slippage_bps: Metric,
    pub execution_model: String,
    pub return_ratio: Metric,
    pub maximum_drawdown_ratio: Metric,
    pub return_volatility_ratio: Metric,
    pub cost_drag_ratio: Metric,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalkForwardResult {
    pub windows: Vec<WalkForwardPoint>,
    pub pass_rate: Metric,
    pub mean_degradation_ratio: Metric,
    pub mean_parameter_drift: Metric,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WalkForwardPoint {
    pub window_id: String,
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub in_sample_return_ratio: Metric,
    pub out_of_sample_return_ratio: Metric,
    pub degradation_ratio: Metric,
    pub out_of_sample_drawdown_ratio: Metric,
    pub parameter_drift: Metric,
    pub passed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegimeResult {
    pub segmentation_definition: String,
    pub regimes: Vec<RegimePoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegimePoint {
    pub regime: String,
    pub return_ratio: Metric,
    pub maximum_drawdown_ratio: Metric,
    pub trade_count: Metric,
    pub expectancy_ratio: Metric,
    pub costs_micros: Metric,
    pub exposure_ratio: Option<Metric>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StressResult {
    pub scenarios: Vec<StressPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StressPoint {
    pub name: String,
    pub kind: StressKind,
    pub scenario_return_assumptions: Vec<Metric>,
    pub pnl_micros: Metric,
    pub maximum_drawdown_ratio: Metric,
    pub recovery_steps: Option<Metric>,
    pub exposure_ratio: Option<Metric>,
    pub contribution_micros: BTreeMap<String, Metric>,
    pub failing_trade_count: Metric,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacityLiquidityResult {
    pub scale_points: Vec<CapacityPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacityPoint {
    pub scale: Metric,
    pub maximum_participation_ratio: Metric,
    pub degradation_ratio: Metric,
    pub slippage_bps: Metric,
    pub filled_trade_count: Metric,
    pub rejected_trade_count: Metric,
    pub bottleneck_trade_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineError {
    pub code: String,
    pub detail: String,
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for EngineError {}

pub fn run(request: &RobustnessRequest) -> Result<RobustnessResult, EngineError> {
    validate_request(request)?;
    let kind = study_kind(&request.study);
    let projection = match &request.study {
        StudyInput::MonteCarlo(input) => monte_carlo(request, input),
        StudyInput::ParameterSensitivity(input) => parameter_sensitivity(request, input),
        StudyInput::FeeSensitivity(input) => {
            cost_sensitivity(request, input, StudyKind::FeeSensitivity)
        }
        StudyInput::SlippageSensitivity(input) => {
            cost_sensitivity(request, input, StudyKind::SlippageSensitivity)
        }
        StudyInput::ExecutionSensitivity(input) => {
            cost_sensitivity(request, input, StudyKind::ExecutionSensitivity)
        }
        StudyInput::WalkForward(input) => walk_forward(request, input),
        StudyInput::Regime(input) => regime(request, input),
        StudyInput::Stress(input) => stress(request, input),
        StudyInput::CapacityLiquidity(input) => capacity_liquidity(request, input),
    }?;
    let (status, output, mut diagnostics) = projection;
    diagnostics.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.detail.cmp(&right.detail))
    });
    let mut result = RobustnessResult {
        schema: ROBUSTNESS_RESULT_SCHEMA.to_string(),
        engine_version: request.engine_version.clone(),
        seed: request.seed,
        source_bindings: canonical_bindings(&request.source.bindings),
        supporting_source_bindings: request
            .supporting_sources
            .iter()
            .map(|source| canonical_bindings(&source.bindings))
            .collect(),
        study: kind,
        status,
        output,
        diagnostics,
        result_hash: String::new(),
    };
    result.result_hash = result_hash(&result)?;
    Ok(result)
}

pub fn verify_result_hash(result: &RobustnessResult) -> Result<bool, EngineError> {
    Ok(result.result_hash == result_hash(result)?)
}

fn validate_request(request: &RobustnessRequest) -> Result<(), EngineError> {
    if request.schema != ROBUSTNESS_REQUEST_SCHEMA {
        return invalid(
            "request_schema_unsupported",
            "request schema is not supported",
        );
    }
    if request.engine_version != ROBUSTNESS_ENGINE_VERSION {
        return invalid(
            "engine_version_unsupported",
            "robustness engine version is not supported",
        );
    }
    validate_budget(&request.budgets)?;
    validate_source(&request.source)?;
    validate_supporting_sources(request)?;
    validate_study_source_bindings(request)?;
    validate_study(request)?;
    Ok(())
}

fn validate_supporting_sources(request: &RobustnessRequest) -> Result<(), EngineError> {
    let mut run_ids = BTreeSet::new();
    run_ids.insert(request.source.bindings.run_id.as_str());
    for source in &request.supporting_sources {
        validate_source(source)?;
        if !run_ids.insert(source.bindings.run_id.as_str()) {
            return invalid(
                "duplicate_source_run",
                "robustness sources must bind distinct immutable runs",
            );
        }
        if source.units.reporting_currency != request.source.units.reporting_currency {
            return invalid(
                "source_currency_incompatible",
                "robustness sources must use one reporting currency",
            );
        }
    }
    Ok(())
}

fn validate_study_source_bindings(request: &RobustnessRequest) -> Result<(), EngineError> {
    let available = std::iter::once(request.source.bindings.run_id.as_str())
        .chain(
            request
                .supporting_sources
                .iter()
                .map(|source| source.bindings.run_id.as_str()),
        )
        .collect::<BTreeSet<_>>();
    let required = required_study_sources(&request.study);
    if required
        .into_iter()
        .any(|run_id| !available.contains(run_id))
    {
        return invalid(
            "study_source_not_bound",
            "observed study inputs must reference an exact bound source run",
        );
    }
    Ok(())
}

fn required_study_sources(study: &StudyInput) -> Vec<&str> {
    match study {
        StudyInput::MonteCarlo(_) => Vec::new(),
        StudyInput::ParameterSensitivity(input) => input
            .cases
            .iter()
            .map(|case| case.source_run_id.as_str())
            .collect(),
        StudyInput::FeeSensitivity(input)
        | StudyInput::SlippageSensitivity(input)
        | StudyInput::ExecutionSensitivity(input) => input
            .cases
            .iter()
            .map(|case| case.source_run_id.as_str())
            .collect(),
        StudyInput::WalkForward(input) => input
            .windows
            .iter()
            .flat_map(|window| {
                [
                    window.in_sample_source_run_id.as_str(),
                    window.out_of_sample_source_run_id.as_str(),
                ]
            })
            .collect(),
        StudyInput::Regime(input) => input
            .segments
            .iter()
            .map(|segment| segment.source_run_id.as_str())
            .collect(),
        StudyInput::Stress(input) => input
            .scenarios
            .iter()
            .filter_map(|scenario| scenario.source_run_id.as_deref())
            .collect(),
        StudyInput::CapacityLiquidity(input) => input
            .observations
            .iter()
            .map(|observation| observation.source_run_id.as_str())
            .collect(),
    }
}

fn validate_budget(budget: &StudyBudget) -> Result<(), EngineError> {
    let checks = [
        (
            budget.max_samples,
            HARD_MAX_SAMPLES,
            "sample_budget_invalid",
        ),
        (
            budget.max_grid_points,
            HARD_MAX_GRID_POINTS,
            "grid_budget_invalid",
        ),
        (
            budget.max_windows,
            HARD_MAX_WINDOWS,
            "window_budget_invalid",
        ),
        (
            budget.max_scenarios,
            HARD_MAX_SCENARIOS,
            "scenario_budget_invalid",
        ),
        (
            budget.max_path_points,
            HARD_MAX_PATH_POINTS,
            "path_budget_invalid",
        ),
        (
            budget.max_output_points,
            HARD_MAX_OUTPUT_POINTS,
            "output_budget_invalid",
        ),
        (
            budget.max_memory_bytes,
            HARD_MAX_MEMORY_BYTES,
            "memory_budget_invalid",
        ),
    ];
    for (value, hard_maximum, code) in checks {
        if value == 0 || value > hard_maximum {
            return invalid(
                code,
                "budget must be positive and within the engine hard limit",
            );
        }
    }
    Ok(())
}

fn validate_source(source: &VerifiedBacktest) -> Result<(), EngineError> {
    if source.starting_equity_micros <= 0 {
        return invalid(
            "starting_equity_invalid",
            "starting equity must be greater than zero",
        );
    }
    if source.units.reporting_currency.trim().is_empty()
        || source.units.equity != Unit::CurrencyMicros
        || source.units.trade_return != Unit::Ratio
        || source.units.cost != Unit::CurrencyMicros
        || source.units.quantity != Unit::Quantity
    {
        return invalid(
            "source_units_invalid",
            "source units are incomplete or incompatible",
        );
    }
    validate_bindings(&source.bindings)?;
    let mut trade_ids = BTreeSet::new();
    let mut previous_trade_sequence = None;
    for trade in &source.trades {
        if trade.trade_id.trim().is_empty()
            || !trade_ids.insert(trade.trade_id.as_str())
            || previous_trade_sequence.is_some_and(|value| trade.sequence <= value)
        {
            return invalid(
                "trade_identity_invalid",
                "trade identifiers must be unique and sequences strictly ordered",
            );
        }
        previous_trade_sequence = Some(trade.sequence);
        finite("trade_return_non_finite", trade.return_ratio)?;
        finite("trade_slippage_non_finite", trade.slippage_bps)?;
        finite("trade_quantity_non_finite", trade.quantity)?;
        if trade.return_ratio <= -1.0 || trade.slippage_bps < 0.0 || trade.quantity < 0.0 {
            return invalid(
                "trade_value_invalid",
                "observed trade values are outside valid bounds",
            );
        }
        require_refs(&trade.evidence_refs, "trade_evidence_missing")?;
    }
    let mut previous = None;
    for point in &source.equity {
        if previous.is_some_and(|value| point.sequence <= value) || point.equity_micros < 0 {
            return invalid(
                "equity_series_invalid",
                "equity points must be non-negative and strictly ordered",
            );
        }
        previous = Some(point.sequence);
        require_refs(&point.evidence_refs, "equity_evidence_missing")?;
    }
    Ok(())
}

fn validate_bindings(bindings: &SourceBindings) -> Result<(), EngineError> {
    if bindings.run_id.trim().is_empty() || bindings.replay_ref.trim().is_empty() {
        return invalid(
            "source_binding_missing",
            "source run and replay references are required",
        );
    }
    for hash in [
        &bindings.manifest_hash,
        &bindings.result_hash,
        &bindings.report_hash,
        &bindings.dataset_hash,
        &bindings.strategy_spec_hash,
    ] {
        if !valid_sha256(hash) {
            return invalid(
                "source_hash_invalid",
                "source hashes must be lowercase sha256 values",
            );
        }
    }
    require_refs(&bindings.evidence_refs, "source_evidence_missing")
}

fn validate_study(request: &RobustnessRequest) -> Result<(), EngineError> {
    match &request.study {
        StudyInput::MonteCarlo(input) => {
            if input.path_count == 0 || input.path_count > request.budgets.max_samples {
                return invalid(
                    "sample_budget_exceeded",
                    "Monte Carlo path count exceeds the declared sample budget",
                );
            }
            finite("confidence_non_finite", input.confidence_level)?;
            finite("ruin_threshold_non_finite", input.ruin_equity_ratio)?;
            if !(0.0 < input.confidence_level
                && input.confidence_level < 1.0
                && (0.0..1.0).contains(&input.ruin_equity_ratio))
            {
                return invalid(
                    "monte_carlo_assumption_invalid",
                    "confidence and ruin assumptions are outside valid bounds",
                );
            }
            let horizon = match &input.mode {
                MonteCarloMode::SyntheticPath { model } => model.horizon_steps,
                _ => request.source.trades.len(),
            };
            checked_work(
                input.path_count,
                horizon,
                request.budgets.max_path_points,
                "path_budget_exceeded",
            )?;
            let output_points = checked_product(input.path_count, 3)?
                .checked_add(checked_product(horizon, 3)?)
                .ok_or_else(|| error("output_budget_exceeded", "output point count overflow"))?;
            if output_points > request.budgets.max_output_points {
                return invalid(
                    "output_budget_exceeded",
                    "Monte Carlo output exceeds the declared output budget",
                );
            }
            let memory = checked_product(input.path_count, horizon)?
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or_else(|| error("memory_budget_exceeded", "memory estimate overflow"))?;
            if memory > request.budgets.max_memory_bytes {
                return invalid(
                    "memory_budget_exceeded",
                    "Monte Carlo work exceeds the declared memory budget",
                );
            }
            validate_monte_carlo_mode(&input.mode)?;
        }
        StudyInput::ParameterSensitivity(input) => {
            bounded_count(
                input.cases.len(),
                request.budgets.max_grid_points,
                "grid_budget_exceeded",
            )?;
            if input.top_count == 0 || input.top_count > input.cases.len().max(1) {
                return invalid("top_count_invalid", "top count is outside the tested grid");
            }
            finite(
                "plateau_tolerance_non_finite",
                input.plateau_tolerance_ratio,
            )?;
            if input.plateau_tolerance_ratio < 0.0 {
                return invalid(
                    "plateau_tolerance_invalid",
                    "plateau tolerance must be non-negative",
                );
            }
            for case in &input.cases {
                require_source_run_id(&case.source_run_id)?;
                validate_parameters(&case.parameters)?;
                validate_returns(&case.trade_returns)?;
                require_refs(&case.evidence_refs, "parameter_case_evidence_missing")?;
            }
        }
        StudyInput::FeeSensitivity(input)
        | StudyInput::SlippageSensitivity(input)
        | StudyInput::ExecutionSensitivity(input) => {
            bounded_count(
                input.cases.len(),
                request.budgets.max_grid_points,
                "grid_budget_exceeded",
            )?;
            if input.baseline_label.trim().is_empty() {
                return invalid("baseline_missing", "a baseline case label is required");
            }
            let mut labels = BTreeSet::new();
            for case in &input.cases {
                require_source_run_id(&case.source_run_id)?;
                finite("fee_non_finite", case.fee_bps)?;
                finite("slippage_non_finite", case.slippage_bps)?;
                if case.label.trim().is_empty()
                    || case.execution_model.trim().is_empty()
                    || !labels.insert(case.label.as_str())
                    || case.fee_bps < 0.0
                    || case.slippage_bps < 0.0
                {
                    return invalid(
                        "cost_case_invalid",
                        "cost cases need unique labels and non-negative assumptions",
                    );
                }
                validate_returns(&case.trade_returns)?;
                require_refs(&case.evidence_refs, "cost_case_evidence_missing")?;
            }
            if !input.cases.is_empty() && !labels.contains(input.baseline_label.as_str()) {
                return invalid("baseline_missing", "baseline case was not supplied");
            }
            if let Some(baseline) = input
                .cases
                .iter()
                .find(|case| case.label == input.baseline_label)
            {
                if input
                    .cases
                    .iter()
                    .any(|case| case.trade_returns.len() != baseline.trade_returns.len())
                {
                    return invalid(
                        "cost_case_trade_count_mismatch",
                        "cost sensitivity cases must cover the same observed trade count",
                    );
                }
            }
        }
        StudyInput::WalkForward(input) => {
            bounded_count(
                input.windows.len(),
                request.budgets.max_windows,
                "window_budget_exceeded",
            )?;
            finite("oos_threshold_non_finite", input.minimum_oos_return_ratio)?;
            let mut ids = BTreeSet::new();
            for window in &input.windows {
                if window.window_id.trim().is_empty()
                    || window.in_sample_source_run_id.trim().is_empty()
                    || window.out_of_sample_source_run_id.trim().is_empty()
                    || !ids.insert(window.window_id.as_str())
                    || window.start_sequence >= window.end_sequence
                {
                    return invalid(
                        "walk_forward_window_invalid",
                        "walk-forward windows need unique identifiers and valid bounds",
                    );
                }
                validate_returns(&window.in_sample_returns)?;
                validate_returns(&window.out_of_sample_returns)?;
                validate_parameters(&window.selected_parameters)?;
                require_refs(&window.evidence_refs, "walk_forward_evidence_missing")?;
            }
        }
        StudyInput::Regime(input) => {
            bounded_count(
                input.segments.len(),
                request.budgets.max_scenarios,
                "scenario_budget_exceeded",
            )?;
            if input.segmentation_definition.trim().is_empty() {
                return invalid(
                    "regime_definition_missing",
                    "regime segmentation definition is required",
                );
            }
            let mut regimes = BTreeSet::new();
            for segment in &input.segments {
                if segment.source_run_id.trim().is_empty()
                    || segment.regime.trim().is_empty()
                    || !regimes.insert(segment.regime.as_str())
                {
                    return invalid(
                        "regime_invalid",
                        "regime labels must be non-empty and unique",
                    );
                }
                validate_returns(&segment.trade_returns)?;
                if let Some(exposure) = segment.exposure_ratio {
                    finite("exposure_non_finite", exposure)?;
                    if exposure < 0.0 {
                        return invalid("exposure_invalid", "exposure cannot be negative");
                    }
                }
                require_refs(&segment.evidence_refs, "regime_evidence_missing")?;
            }
        }
        StudyInput::Stress(input) => {
            bounded_count(
                input.scenarios.len(),
                request.budgets.max_scenarios,
                "scenario_budget_exceeded",
            )?;
            let mut names = BTreeSet::new();
            for scenario in &input.scenarios {
                if scenario.name.trim().is_empty() || !names.insert(scenario.name.as_str()) {
                    return invalid(
                        "stress_scenario_invalid",
                        "stress scenario names must be non-empty and unique",
                    );
                }
                match (scenario.kind, scenario.source_run_id.as_deref()) {
                    (StressKind::HistoricalObserved, Some(run_id)) if !run_id.trim().is_empty() => {
                    }
                    (StressKind::HypotheticalDeclared, None) => {}
                    _ => {
                        return invalid(
                            "stress_source_invalid",
                            "historical stress requires a source run and hypothetical stress must not claim one",
                        );
                    }
                }
                validate_returns(&scenario.shocked_trade_returns)?;
                if let Some(exposure) = scenario.exposure_ratio {
                    finite("stress_exposure_non_finite", exposure)?;
                    if exposure < 0.0 {
                        return invalid(
                            "stress_exposure_invalid",
                            "stress exposure cannot be negative",
                        );
                    }
                }
                require_refs(&scenario.evidence_refs, "stress_evidence_missing")?;
            }
        }
        StudyInput::CapacityLiquidity(input) => {
            bounded_count(
                input.scale_points.len(),
                request.budgets.max_grid_points,
                "grid_budget_exceeded",
            )?;
            bounded_count(
                input.observations.len(),
                request.budgets.max_scenarios,
                "scenario_budget_exceeded",
            )?;
            finite(
                "participation_limit_non_finite",
                input.maximum_participation_ratio,
            )?;
            finite(
                "impact_assumption_non_finite",
                input.impact_bps_per_participation_ratio,
            )?;
            if !(0.0 < input.maximum_participation_ratio
                && input.maximum_participation_ratio <= 1.0)
                || input.impact_bps_per_participation_ratio < 0.0
            {
                return invalid(
                    "capacity_assumption_invalid",
                    "capacity assumptions are outside valid bounds",
                );
            }
            for scale in &input.scale_points {
                finite("scale_non_finite", *scale)?;
                if *scale <= 0.0 {
                    return invalid("scale_invalid", "capacity scale must be greater than zero");
                }
            }
            for observation in &input.observations {
                if observation.source_run_id.trim().is_empty()
                    || observation.trade_id.trim().is_empty()
                {
                    return invalid("liquidity_trade_invalid", "liquidity trade id is required");
                }
                finite("liquidity_return_non_finite", observation.return_ratio)?;
                finite("liquidity_quantity_non_finite", observation.quantity)?;
                if observation.return_ratio <= -1.0 || observation.quantity < 0.0 {
                    return invalid(
                        "liquidity_observation_invalid",
                        "liquidity observation is outside valid bounds",
                    );
                }
                for value in [
                    observation.observed_volume,
                    observation.observed_spread_bps,
                    observation.open_interest,
                    observation.displayed_depth,
                ]
                .into_iter()
                .flatten()
                {
                    finite("liquidity_value_non_finite", value)?;
                    if value < 0.0 {
                        return invalid(
                            "liquidity_value_invalid",
                            "liquidity evidence cannot be negative",
                        );
                    }
                }
                require_refs(&observation.evidence_refs, "liquidity_evidence_missing")?;
            }
        }
    }
    if !matches!(request.study, StudyInput::MonteCarlo(_)) {
        validate_projection_budget(request)?;
    }
    Ok(())
}

fn require_source_run_id(run_id: &str) -> Result<(), EngineError> {
    if run_id.trim().is_empty() {
        invalid(
            "study_source_run_missing",
            "observed study inputs require an immutable source run",
        )
    } else {
        Ok(())
    }
}

fn validate_projection_budget(request: &RobustnessRequest) -> Result<(), EngineError> {
    let (work_points, output_points) = match &request.study {
        StudyInput::MonteCarlo(_) => return Ok(()),
        StudyInput::ParameterSensitivity(input) => (
            checked_sum(input.cases.iter().map(|case| case.trade_returns.len()))?,
            checked_product(input.cases.len(), 4)?,
        ),
        StudyInput::FeeSensitivity(input)
        | StudyInput::SlippageSensitivity(input)
        | StudyInput::ExecutionSensitivity(input) => (
            checked_sum(input.cases.iter().map(|case| case.trade_returns.len()))?,
            checked_product(input.cases.len(), 8)?,
        ),
        StudyInput::WalkForward(input) => (
            checked_sum(input.windows.iter().map(|window| {
                window
                    .in_sample_returns
                    .len()
                    .saturating_add(window.out_of_sample_returns.len())
            }))?,
            checked_product(input.windows.len(), 8)?,
        ),
        StudyInput::Regime(input) => (
            checked_sum(
                input
                    .segments
                    .iter()
                    .map(|segment| segment.trade_returns.len()),
            )?,
            checked_product(input.segments.len(), 7)?,
        ),
        StudyInput::Stress(input) => (
            checked_sum(input.scenarios.iter().map(|scenario| {
                scenario
                    .shocked_trade_returns
                    .len()
                    .saturating_add(scenario.contribution_micros.len())
            }))?,
            checked_product(input.scenarios.len(), 7)?,
        ),
        StudyInput::CapacityLiquidity(input) => (
            checked_product(input.observations.len(), input.scale_points.len())?,
            checked_product(
                input.scale_points.len(),
                input.observations.len().saturating_add(7),
            )?,
        ),
    };
    if work_points > request.budgets.max_path_points {
        return invalid(
            "work_budget_exceeded",
            "study work exceeds the declared path-point budget",
        );
    }
    if output_points > request.budgets.max_output_points {
        return invalid(
            "output_budget_exceeded",
            "study output exceeds the declared output budget",
        );
    }
    let estimated_memory = work_points
        .checked_add(output_points)
        .and_then(|points| points.checked_mul(64))
        .ok_or_else(|| error("memory_budget_exceeded", "memory estimate overflow"))?;
    if estimated_memory > request.budgets.max_memory_bytes {
        return invalid(
            "memory_budget_exceeded",
            "study work exceeds the declared memory budget",
        );
    }
    Ok(())
}

fn validate_monte_carlo_mode(mode: &MonteCarloMode) -> Result<(), EngineError> {
    match mode {
        MonteCarloMode::TradeOrderReshuffle | MonteCarloMode::TradeBootstrap => Ok(()),
        MonteCarloMode::ExecutionRandomization {
            minimum_cost_bps,
            maximum_cost_bps,
        } => {
            finite("execution_cost_non_finite", *minimum_cost_bps)?;
            finite("execution_cost_non_finite", *maximum_cost_bps)?;
            if *minimum_cost_bps < 0.0 || minimum_cost_bps > maximum_cost_bps {
                return invalid(
                    "execution_cost_bounds_invalid",
                    "execution cost bounds are invalid",
                );
            }
            Ok(())
        }
        MonteCarloMode::ParameterPerturbation { model } => {
            finite("parameter_factor_non_finite", model.minimum_return_factor)?;
            finite("parameter_factor_non_finite", model.maximum_return_factor)?;
            if model.schema != PARAMETER_PERTURBATION_MODEL_V1
                || model.minimum_return_factor < 0.0
                || model.minimum_return_factor > model.maximum_return_factor
            {
                return invalid(
                    "parameter_model_invalid",
                    "parameter perturbation needs the supported explicit model",
                );
            }
            require_refs(&model.evidence_refs, "parameter_model_evidence_missing")
        }
        MonteCarloMode::SyntheticPath { model } => {
            finite("synthetic_mean_non_finite", model.mean_log_return_ratio)?;
            finite("synthetic_volatility_non_finite", model.volatility_ratio)?;
            if model.schema != SYNTHETIC_LOG_RETURN_MODEL_V1
                || model.horizon_steps == 0
                || model.volatility_ratio < 0.0
            {
                return invalid(
                    "synthetic_model_invalid",
                    "synthetic paths need the supported complete versioned model",
                );
            }
            require_refs(&model.evidence_refs, "synthetic_model_evidence_missing")
        }
    }
}

type Projection = (ResultStatus, StudyOutput, Vec<Diagnostic>);

fn monte_carlo(
    request: &RobustnessRequest,
    input: &MonteCarloInput,
) -> Result<Projection, EngineError> {
    let observed_returns: Vec<f64> = request
        .source
        .trades
        .iter()
        .map(|trade| trade.return_ratio)
        .collect();
    if observed_returns.is_empty() && !matches!(input.mode, MonteCarloMode::SyntheticPath { .. }) {
        return Ok(unavailable(
            "monte_carlo",
            "observed_trade_returns_missing",
            &["completed backtest trade returns"],
        ));
    }
    let horizon = match &input.mode {
        MonteCarloMode::SyntheticPath { model } => model.horizon_steps,
        _ => observed_returns.len(),
    };
    let refs = aggregate_refs(&request.source, mode_refs(&input.mode));
    let mut rng = DeterministicRng::new(request.seed);
    let mut paths = Vec::with_capacity(input.path_count);
    let mut terminal = Vec::with_capacity(input.path_count);
    let mut drawdowns = Vec::with_capacity(input.path_count);
    let mut losing_streaks = Vec::with_capacity(input.path_count);
    let mut loss_count = 0_usize;
    let mut ruin_count = 0_usize;
    let starting = request.source.starting_equity_micros as f64;
    let ruin_level = starting * input.ruin_equity_ratio;
    for _ in 0..input.path_count {
        let returns = monte_carlo_returns(&observed_returns, &input.mode, &mut rng)?;
        let analysis = analyze_returns(starting, &returns);
        if analysis.terminal < starting {
            loss_count += 1;
        }
        if analysis.equity.iter().any(|value| *value <= ruin_level) {
            ruin_count += 1;
        }
        terminal.push(analysis.terminal);
        drawdowns.push(analysis.maximum_drawdown);
        losing_streaks.push(analysis.longest_losing_streak as f64);
        paths.push(analysis.equity);
    }
    let alpha = (1.0 - input.confidence_level) / 2.0;
    let mut confidence_path = Vec::with_capacity(horizon);
    for step in 0..horizon {
        let values: Vec<f64> = paths.iter().map(|path| path[step]).collect();
        confidence_path.push(ConfidencePathPoint {
            step,
            lower: metric(
                quantile(&values, alpha),
                Unit::CurrencyMicros,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            median: metric(
                quantile(&values, 0.5),
                Unit::CurrencyMicros,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            upper: metric(
                quantile(&values, 1.0 - alpha),
                Unit::CurrencyMicros,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
        });
    }
    let result = MonteCarloResult {
        path_count: input.path_count,
        confidence_path,
        terminal_distribution: sorted_metrics(
            &terminal,
            Unit::CurrencyMicros,
            FactOrigin::ModelDerived,
            &refs,
            request,
        ),
        drawdown_distribution: sorted_metrics(
            &drawdowns,
            Unit::Ratio,
            FactOrigin::ModelDerived,
            &refs,
            request,
        ),
        longest_losing_streak_distribution: sorted_metrics(
            &losing_streaks,
            Unit::Count,
            FactOrigin::ModelDerived,
            &refs,
            request,
        ),
        loss_probability: metric(
            loss_count as f64 / input.path_count as f64,
            Unit::Probability,
            FactOrigin::ModelDerived,
            &refs,
            request,
        ),
        ruin_probability: metric(
            ruin_count as f64 / input.path_count as f64,
            Unit::Probability,
            FactOrigin::ModelDerived,
            &refs,
            request,
        ),
        terminal_confidence_interval: ConfidenceInterval {
            confidence_level: input.confidence_level,
            lower: metric(
                quantile(&terminal, alpha),
                Unit::CurrencyMicros,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            upper: metric(
                quantile(&terminal, 1.0 - alpha),
                Unit::CurrencyMicros,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
        },
    };
    Ok((
        ResultStatus::Available,
        StudyOutput::MonteCarlo(result),
        Vec::new(),
    ))
}

fn monte_carlo_returns(
    observed: &[f64],
    mode: &MonteCarloMode,
    rng: &mut DeterministicRng,
) -> Result<Vec<f64>, EngineError> {
    match mode {
        MonteCarloMode::TradeOrderReshuffle => {
            let mut returns = observed.to_vec();
            rng.shuffle(&mut returns);
            Ok(returns)
        }
        MonteCarloMode::TradeBootstrap => Ok((0..observed.len())
            .map(|_| observed[rng.index(observed.len())])
            .collect()),
        MonteCarloMode::ExecutionRandomization {
            minimum_cost_bps,
            maximum_cost_bps,
        } => Ok(observed
            .iter()
            .map(|value| {
                let cost = rng.range(*minimum_cost_bps, *maximum_cost_bps) / 10_000.0;
                (*value - cost).max(-0.999_999_999)
            })
            .collect()),
        MonteCarloMode::ParameterPerturbation { model } => {
            let factor = rng.range(model.minimum_return_factor, model.maximum_return_factor);
            Ok(observed
                .iter()
                .map(|value| (*value * factor).max(-0.999_999_999))
                .collect())
        }
        MonteCarloMode::SyntheticPath { model } => {
            let mut returns = Vec::with_capacity(model.horizon_steps);
            for _ in 0..model.horizon_steps {
                let log_return =
                    model.mean_log_return_ratio + model.volatility_ratio * rng.normal();
                let simple_return = log_return.exp() - 1.0;
                if !simple_return.is_finite() || simple_return <= -1.0 {
                    return invalid(
                        "synthetic_path_non_finite",
                        "synthetic model produced an invalid return",
                    );
                }
                returns.push(simple_return);
            }
            Ok(returns)
        }
    }
}

fn parameter_sensitivity(
    request: &RobustnessRequest,
    input: &ParameterSensitivityInput,
) -> Result<Projection, EngineError> {
    if input.cases.is_empty() {
        return Ok(unavailable(
            "parameter_sensitivity",
            "parameter_grid_missing",
            &["verified parameter grid results"],
        ));
    }
    let mut cases = input.cases.clone();
    cases.sort_by(|left, right| {
        parameter_key(&left.parameters).cmp(&parameter_key(&right.parameters))
    });
    let mut grid = Vec::with_capacity(cases.len());
    for case in &cases {
        if case.trade_returns.is_empty() {
            return Ok(unavailable(
                "parameter_sensitivity",
                "parameter_case_returns_missing",
                &["trade returns for every parameter case"],
            ));
        }
        let refs = aggregate_refs(&request.source, &case.evidence_refs);
        let analysis = analyze_returns(
            request.source.starting_equity_micros as f64,
            &case.trade_returns,
        );
        grid.push(ParameterGridPoint {
            parameters: case.parameters.clone(),
            return_ratio: metric(
                analysis.total_return,
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            maximum_drawdown_ratio: metric(
                analysis.maximum_drawdown,
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            return_volatility_ratio: metric(
                volatility(&case.trade_returns),
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
        });
    }
    let mut ranked = grid.clone();
    ranked.sort_by(|left, right| {
        total_cmp(right.return_ratio.value, left.return_ratio.value)
            .then_with(|| parameter_key(&left.parameters).cmp(&parameter_key(&right.parameters)))
    });
    let best = ranked[0].return_ratio.value;
    let plateau_count = ranked
        .iter()
        .filter(|point| best - point.return_ratio.value <= input.plateau_tolerance_ratio)
        .count();
    let neighbor_count = ranked
        .iter()
        .skip(1)
        .filter(|point| {
            parameter_distance(&ranked[0].parameters, &point.parameters) == 1
                && best - point.return_ratio.value <= input.plateau_tolerance_ratio
        })
        .count();
    let all_neighbors = ranked
        .iter()
        .skip(1)
        .filter(|point| parameter_distance(&ranked[0].parameters, &point.parameters) == 1)
        .count();
    let refs = request.source.bindings.evidence_refs.clone();
    let mut warnings = Vec::new();
    if plateau_count == 1 && ranked.len() > 1 {
        warnings.push(diagnostic(
            "isolated_parameter_peak",
            "best observed parameter result is outside the declared plateau tolerance of neighbors",
            &[] as &[&str],
        ));
    }
    if ranked[0].maximum_drawdown_ratio.value > 0.5 {
        warnings.push(diagnostic(
            "high_observed_drawdown",
            "top observed parameter case has maximum drawdown above 0.5 ratio",
            &[] as &[&str],
        ));
    }
    Ok((
        ResultStatus::Available,
        StudyOutput::ParameterSensitivity(ParameterSensitivityResult {
            tested_grid: grid,
            top_sets: ranked.into_iter().take(input.top_count).collect(),
            plateau_fraction: metric(
                plateau_count as f64 / cases.len() as f64,
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            neighbor_stability: metric(
                if all_neighbors == 0 {
                    0.0
                } else {
                    neighbor_count as f64 / all_neighbors as f64
                },
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            warnings,
        }),
        Vec::new(),
    ))
}

fn cost_sensitivity(
    request: &RobustnessRequest,
    input: &CostSensitivityInput,
    kind: StudyKind,
) -> Result<Projection, EngineError> {
    if input.cases.is_empty() {
        return Ok(unavailable(
            "cost_sensitivity",
            "sensitivity_cases_missing",
            &["verified sensitivity case results"],
        ));
    }
    if input.cases.iter().any(|case| case.trade_returns.is_empty()) {
        return Ok(unavailable(
            "cost_sensitivity",
            "sensitivity_case_returns_missing",
            &["trade returns for every sensitivity case"],
        ));
    }
    let baseline = input
        .cases
        .iter()
        .find(|case| case.label == input.baseline_label)
        .expect("validated baseline");
    let baseline_analysis = analyze_returns(
        request.source.starting_equity_micros as f64,
        &baseline.trade_returns,
    );
    let mut cases = input.cases.clone();
    cases.sort_by(|left, right| {
        total_cmp(left.fee_bps, right.fee_bps)
            .then_with(|| total_cmp(left.slippage_bps, right.slippage_bps))
            .then_with(|| left.execution_model.cmp(&right.execution_model))
            .then_with(|| left.label.cmp(&right.label))
    });
    let mut rows = Vec::with_capacity(cases.len());
    let mut flipped = 0_usize;
    for case in &cases {
        let refs = aggregate_refs(&request.source, &case.evidence_refs);
        let origin = if case.label == input.baseline_label {
            FactOrigin::ObservedBacktest
        } else {
            FactOrigin::ModelDerived
        };
        let analysis = analyze_returns(
            request.source.starting_equity_micros as f64,
            &case.trade_returns,
        );
        flipped += baseline
            .trade_returns
            .iter()
            .zip(case.trade_returns.iter())
            .filter(|(base, tested)| base.signum() != tested.signum())
            .count();
        rows.push(CostSensitivityPoint {
            label: case.label.clone(),
            fee_bps: metric(case.fee_bps, Unit::BasisPoints, origin, &refs, request),
            slippage_bps: metric(case.slippage_bps, Unit::BasisPoints, origin, &refs, request),
            execution_model: case.execution_model.clone(),
            return_ratio: metric(analysis.total_return, Unit::Ratio, origin, &refs, request),
            maximum_drawdown_ratio: metric(
                analysis.maximum_drawdown,
                Unit::Ratio,
                origin,
                &refs,
                request,
            ),
            return_volatility_ratio: metric(
                volatility(&case.trade_returns),
                Unit::Ratio,
                origin,
                &refs,
                request,
            ),
            cost_drag_ratio: metric(
                baseline_analysis.total_return - analysis.total_return,
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
        });
    }
    let returns: Vec<f64> = rows.iter().map(|row| row.return_ratio.value).collect();
    let refs = request.source.bindings.evidence_refs.clone();
    let break_even = match kind {
        StudyKind::FeeSensitivity => bounded_break_even(
            rows.iter()
                .map(|row| (row.fee_bps.value, row.return_ratio.value))
                .collect(),
        ),
        StudyKind::SlippageSensitivity => bounded_break_even(
            rows.iter()
                .map(|row| (row.slippage_bps.value, row.return_ratio.value))
                .collect(),
        ),
        StudyKind::ExecutionSensitivity => None,
        _ => unreachable!("cost sensitivity kind"),
    }
    .map(|value| {
        metric(
            value,
            Unit::BasisPoints,
            FactOrigin::ModelDerived,
            &refs,
            request,
        )
    });
    let break_even_unbounded = matches!(
        kind,
        StudyKind::FeeSensitivity | StudyKind::SlippageSensitivity
    ) && break_even.is_none();
    let result = CostSensitivityResult {
        cases: rows,
        break_even_bps: break_even,
        fragility_range_ratio: metric(
            maximum(&returns) - minimum(&returns),
            Unit::Ratio,
            FactOrigin::ModelDerived,
            &refs,
            request,
        ),
        flipped_outcome_count: metric(
            flipped as f64,
            Unit::Count,
            FactOrigin::ModelDerived,
            &refs,
            request,
        ),
    };
    let output = match kind {
        StudyKind::FeeSensitivity => StudyOutput::FeeSensitivity(result),
        StudyKind::SlippageSensitivity => StudyOutput::SlippageSensitivity(result),
        StudyKind::ExecutionSensitivity => StudyOutput::ExecutionSensitivity(result),
        _ => unreachable!("cost sensitivity kind"),
    };
    let diagnostics = if break_even_unbounded {
        vec![diagnostic(
            "break_even_unbounded",
            "the tested bounded grid does not contain a return break-even point",
            &[] as &[&str],
        )]
    } else {
        Vec::new()
    };
    Ok((ResultStatus::Available, output, diagnostics))
}

fn walk_forward(
    request: &RobustnessRequest,
    input: &WalkForwardInput,
) -> Result<Projection, EngineError> {
    if input.windows.is_empty() {
        return Ok(unavailable(
            "walk_forward",
            "walk_forward_windows_missing",
            &["verified in-sample and out-of-sample windows"],
        ));
    }
    if input.windows.iter().any(|window| {
        window.in_sample_returns.is_empty() || window.out_of_sample_returns.is_empty()
    }) {
        return Ok(unavailable(
            "walk_forward",
            "walk_forward_returns_missing",
            &["in-sample and out-of-sample trade returns for every window"],
        ));
    }
    let mut windows = input.windows.clone();
    windows.sort_by(|left, right| {
        left.start_sequence
            .cmp(&right.start_sequence)
            .then_with(|| left.end_sequence.cmp(&right.end_sequence))
            .then_with(|| left.window_id.cmp(&right.window_id))
    });
    let mut rows = Vec::with_capacity(windows.len());
    let mut passes = 0_usize;
    let mut degradations = Vec::new();
    let mut drifts = Vec::new();
    let mut previous_parameters: Option<&BTreeMap<String, f64>> = None;
    for window in &windows {
        let refs = aggregate_refs(&request.source, &window.evidence_refs);
        let in_sample = analyze_returns(
            request.source.starting_equity_micros as f64,
            &window.in_sample_returns,
        );
        let out_of_sample = analyze_returns(
            request.source.starting_equity_micros as f64,
            &window.out_of_sample_returns,
        );
        let degradation = out_of_sample.total_return - in_sample.total_return;
        let drift = previous_parameters
            .map(|previous| euclidean_parameter_drift(previous, &window.selected_parameters))
            .unwrap_or(0.0);
        let passed = out_of_sample.total_return >= input.minimum_oos_return_ratio;
        if passed {
            passes += 1;
        }
        degradations.push(degradation);
        drifts.push(drift);
        rows.push(WalkForwardPoint {
            window_id: window.window_id.clone(),
            start_sequence: window.start_sequence,
            end_sequence: window.end_sequence,
            in_sample_return_ratio: metric(
                in_sample.total_return,
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            out_of_sample_return_ratio: metric(
                out_of_sample.total_return,
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            degradation_ratio: metric(
                degradation,
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            out_of_sample_drawdown_ratio: metric(
                out_of_sample.maximum_drawdown,
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            parameter_drift: metric(drift, Unit::Ratio, FactOrigin::ModelDerived, &refs, request),
            passed,
        });
        previous_parameters = Some(&window.selected_parameters);
    }
    let refs = request.source.bindings.evidence_refs.clone();
    Ok((
        ResultStatus::Available,
        StudyOutput::WalkForward(WalkForwardResult {
            windows: rows,
            pass_rate: metric(
                passes as f64 / windows.len() as f64,
                Unit::Probability,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            mean_degradation_ratio: metric(
                mean(&degradations),
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            mean_parameter_drift: metric(
                mean(&drifts),
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
        }),
        Vec::new(),
    ))
}

fn regime(request: &RobustnessRequest, input: &RegimeInput) -> Result<Projection, EngineError> {
    if input.segments.is_empty() {
        return Ok(unavailable(
            "regime",
            "regime_segments_missing",
            &["verified regime segmentation and trade assignments"],
        ));
    }
    let mut segments = input.segments.clone();
    segments.sort_by(|left, right| left.regime.cmp(&right.regime));
    let mut rows = Vec::with_capacity(segments.len());
    for segment in &segments {
        if segment.trade_returns.is_empty() {
            return Ok(unavailable(
                "regime",
                "regime_returns_missing",
                &["trade returns for every regime"],
            ));
        }
        let refs = aggregate_refs(&request.source, &segment.evidence_refs);
        let analysis = analyze_returns(
            request.source.starting_equity_micros as f64,
            &segment.trade_returns,
        );
        rows.push(RegimePoint {
            regime: segment.regime.clone(),
            return_ratio: metric(
                analysis.total_return,
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            maximum_drawdown_ratio: metric(
                analysis.maximum_drawdown,
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            trade_count: metric(
                segment.trade_returns.len() as f64,
                Unit::Count,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            expectancy_ratio: metric(
                mean(&segment.trade_returns),
                Unit::Ratio,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            costs_micros: metric(
                segment.costs_micros as f64,
                Unit::CurrencyMicros,
                FactOrigin::ObservedBacktest,
                &refs,
                request,
            ),
            exposure_ratio: segment.exposure_ratio.map(|value| {
                metric(
                    value,
                    Unit::Ratio,
                    FactOrigin::ObservedBacktest,
                    &refs,
                    request,
                )
            }),
        });
    }
    let missing_exposure: Vec<String> = segments
        .iter()
        .filter(|segment| segment.exposure_ratio.is_none())
        .map(|segment| segment.regime.clone())
        .collect();
    let diagnostics = if missing_exposure.is_empty() {
        Vec::new()
    } else {
        vec![diagnostic(
            "regime_exposure_unavailable",
            "exposure is unavailable for one or more regimes",
            &missing_exposure,
        )]
    };
    Ok((
        ResultStatus::Available,
        StudyOutput::Regime(RegimeResult {
            segmentation_definition: input.segmentation_definition.clone(),
            regimes: rows,
        }),
        diagnostics,
    ))
}

fn stress(request: &RobustnessRequest, input: &StressInput) -> Result<Projection, EngineError> {
    if input.scenarios.is_empty() {
        return Ok(unavailable(
            "stress",
            "stress_scenarios_missing",
            &["historical observations or explicit hypothetical scenarios"],
        ));
    }
    let mut scenarios = input.scenarios.clone();
    scenarios.sort_by(|left, right| left.name.cmp(&right.name));
    let mut rows = Vec::with_capacity(scenarios.len());
    for scenario in &scenarios {
        if scenario.shocked_trade_returns.is_empty() {
            return Ok(unavailable(
                "stress",
                "stress_returns_missing",
                &["shocked trade returns for every stress scenario"],
            ));
        }
        let refs = aggregate_refs(&request.source, &scenario.evidence_refs);
        let analysis = analyze_returns(
            request.source.starting_equity_micros as f64,
            &scenario.shocked_trade_returns,
        );
        let origin = match scenario.kind {
            StressKind::HistoricalObserved => FactOrigin::ObservedBacktest,
            StressKind::HypotheticalDeclared => FactOrigin::ModelDerived,
        };
        rows.push(StressPoint {
            name: scenario.name.clone(),
            kind: scenario.kind,
            scenario_return_assumptions: scenario
                .shocked_trade_returns
                .iter()
                .map(|value| metric(*value, Unit::Ratio, origin, &refs, request))
                .collect(),
            pnl_micros: metric(
                analysis.terminal - request.source.starting_equity_micros as f64,
                Unit::CurrencyMicros,
                origin,
                &refs,
                request,
            ),
            maximum_drawdown_ratio: metric(
                analysis.maximum_drawdown,
                Unit::Ratio,
                origin,
                &refs,
                request,
            ),
            recovery_steps: analysis
                .recovery_steps
                .map(|steps| metric(steps as f64, Unit::Steps, origin, &refs, request)),
            exposure_ratio: scenario
                .exposure_ratio
                .map(|value| metric(value, Unit::Ratio, origin, &refs, request)),
            contribution_micros: scenario
                .contribution_micros
                .iter()
                .map(|(trade_id, value)| {
                    (
                        trade_id.clone(),
                        metric(*value as f64, Unit::CurrencyMicros, origin, &refs, request),
                    )
                })
                .collect(),
            failing_trade_count: metric(
                scenario
                    .shocked_trade_returns
                    .iter()
                    .filter(|value| **value < 0.0)
                    .count() as f64,
                Unit::Count,
                origin,
                &refs,
                request,
            ),
        });
    }
    let unrecovered: Vec<String> = rows
        .iter()
        .filter(|scenario| scenario.recovery_steps.is_none())
        .map(|scenario| scenario.name.clone())
        .collect();
    let missing_exposure: Vec<String> = rows
        .iter()
        .filter(|scenario| scenario.exposure_ratio.is_none())
        .map(|scenario| scenario.name.clone())
        .collect();
    let mut diagnostics = Vec::new();
    if !unrecovered.is_empty() {
        diagnostics.push(diagnostic(
            "stress_recovery_unavailable",
            "the supplied scenario series does not recover from maximum drawdown",
            &unrecovered,
        ));
    }
    if !missing_exposure.is_empty() {
        diagnostics.push(diagnostic(
            "stress_exposure_unavailable",
            "exposure is unavailable for one or more stress scenarios",
            &missing_exposure,
        ));
    }
    Ok((
        ResultStatus::Available,
        StudyOutput::Stress(StressResult { scenarios: rows }),
        diagnostics,
    ))
}

fn capacity_liquidity(
    request: &RobustnessRequest,
    input: &CapacityLiquidityInput,
) -> Result<Projection, EngineError> {
    if input.observations.is_empty() {
        return Ok(unavailable(
            "capacity_liquidity",
            "liquidity_observations_missing",
            &["verified trade-level liquidity observations"],
        ));
    }
    if input.scale_points.is_empty() {
        return Ok(unavailable(
            "capacity_liquidity",
            "capacity_scale_grid_missing",
            &["explicit capacity scale grid"],
        ));
    }
    let missing: Vec<String> = input
        .observations
        .iter()
        .filter(|observation| {
            observation.observed_volume.is_none() || observation.observed_spread_bps.is_none()
        })
        .map(|observation| observation.trade_id.clone())
        .collect();
    if !missing.is_empty() {
        return Ok(unavailable_owned(
            "capacity_liquidity",
            "liquidity_fields_missing",
            missing,
        ));
    }
    let mut observations = input.observations.clone();
    observations.sort_by(|left, right| left.trade_id.cmp(&right.trade_id));
    let mut scales = input.scale_points.clone();
    scales.sort_by(|left, right| total_cmp(*left, *right));
    scales.dedup_by(|left, right| left.to_bits() == right.to_bits());
    let baseline = compound_return(
        &observations
            .iter()
            .map(|observation| observation.return_ratio)
            .collect::<Vec<_>>(),
    );
    let mut rows = Vec::with_capacity(scales.len());
    for scale in scales {
        let mut adjusted_returns = Vec::new();
        let mut maximum_participation = 0.0_f64;
        let mut total_slippage = 0.0_f64;
        let mut rejected = 0_usize;
        let mut bottlenecks = Vec::new();
        let mut refs = request.source.bindings.evidence_refs.clone();
        for observation in &observations {
            refs.extend(observation.evidence_refs.clone());
            let volume = observation.observed_volume.expect("validated volume");
            let spread = observation.observed_spread_bps.expect("validated spread");
            if volume <= 0.0 {
                return Ok(unavailable(
                    "capacity_liquidity",
                    "positive_volume_missing",
                    &["positive observed volume for every trade"],
                ));
            }
            let participation = observation.quantity * scale / volume;
            maximum_participation = maximum_participation.max(participation);
            let rejected_trade = input.reject_above_participation
                && participation > input.maximum_participation_ratio;
            if rejected_trade {
                rejected += 1;
                bottlenecks.push(observation.trade_id.clone());
                continue;
            }
            if participation > input.maximum_participation_ratio
                || observation
                    .open_interest
                    .is_some_and(|value| observation.quantity * scale > value)
                || observation
                    .displayed_depth
                    .is_some_and(|value| observation.quantity * scale > value)
            {
                bottlenecks.push(observation.trade_id.clone());
            }
            let slippage = spread / 2.0 + input.impact_bps_per_participation_ratio * participation;
            total_slippage += slippage;
            adjusted_returns
                .push((observation.return_ratio - slippage / 10_000.0).max(-0.999_999_999));
        }
        refs.sort();
        refs.dedup();
        let adjusted = compound_return(&adjusted_returns);
        rows.push(CapacityPoint {
            scale: metric(scale, Unit::Ratio, FactOrigin::ModelDerived, &refs, request),
            maximum_participation_ratio: metric(
                maximum_participation,
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            degradation_ratio: metric(
                baseline - adjusted,
                Unit::Ratio,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            slippage_bps: metric(
                if adjusted_returns.is_empty() {
                    0.0
                } else {
                    total_slippage / adjusted_returns.len() as f64
                },
                Unit::BasisPoints,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            filled_trade_count: metric(
                adjusted_returns.len() as f64,
                Unit::Count,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            rejected_trade_count: metric(
                rejected as f64,
                Unit::Count,
                FactOrigin::ModelDerived,
                &refs,
                request,
            ),
            bottleneck_trade_ids: bottlenecks,
        });
    }
    Ok((
        ResultStatus::Available,
        StudyOutput::CapacityLiquidity(CapacityLiquidityResult { scale_points: rows }),
        Vec::new(),
    ))
}

#[derive(Debug)]
struct ReturnAnalysis {
    equity: Vec<f64>,
    terminal: f64,
    total_return: f64,
    maximum_drawdown: f64,
    longest_losing_streak: usize,
    recovery_steps: Option<usize>,
}

fn analyze_returns(starting_equity: f64, returns: &[f64]) -> ReturnAnalysis {
    let mut equity = Vec::with_capacity(returns.len());
    let mut current = starting_equity;
    let mut peak = starting_equity;
    let mut maximum_drawdown = 0.0_f64;
    let mut current_streak = 0_usize;
    let mut longest_losing_streak = 0_usize;
    let mut trough_index = None;
    let mut peak_before_trough = peak;
    for (index, value) in returns.iter().enumerate() {
        current *= 1.0 + value;
        equity.push(current);
        if current >= peak {
            peak = current;
            current_streak = 0;
        } else if *value < 0.0 {
            current_streak += 1;
            longest_losing_streak = longest_losing_streak.max(current_streak);
        } else {
            current_streak = 0;
        }
        let drawdown = if peak == 0.0 {
            0.0
        } else {
            (peak - current) / peak
        };
        if drawdown > maximum_drawdown {
            maximum_drawdown = drawdown;
            trough_index = Some(index);
            peak_before_trough = peak;
        }
    }
    let recovery_steps = trough_index.and_then(|trough| {
        equity[trough + 1..]
            .iter()
            .position(|value| *value >= peak_before_trough)
            .map(|offset| offset + 1)
    });
    ReturnAnalysis {
        equity,
        terminal: current,
        total_return: current / starting_equity - 1.0,
        maximum_drawdown,
        longest_losing_streak,
        recovery_steps,
    }
}

fn volatility(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let average = mean(values);
    let variance = values
        .iter()
        .map(|value| (value - average).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    variance.sqrt()
}

fn compound_return(values: &[f64]) -> f64 {
    values
        .iter()
        .fold(1.0, |equity, value| equity * (1.0 + value))
        - 1.0
}

fn quantile(values: &[f64], probability: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| total_cmp(*left, *right));
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = probability * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let fraction = position - lower as f64;
        sorted[lower] + (sorted[upper] - sorted[lower]) * fraction
    }
}

fn bounded_break_even(mut points: Vec<(f64, f64)>) -> Option<f64> {
    points
        .sort_by(|left, right| total_cmp(left.0, right.0).then_with(|| total_cmp(left.1, right.1)));
    for point in &points {
        if point.1 == 0.0 {
            return Some(point.0);
        }
    }
    for pair in points.windows(2) {
        let (left_x, left_y) = pair[0];
        let (right_x, right_y) = pair[1];
        if left_y.signum() != right_y.signum() && right_x != left_x && right_y != left_y {
            return Some(left_x + (-left_y) * (right_x - left_x) / (right_y - left_y));
        }
    }
    None
}

fn parameter_distance(left: &BTreeMap<String, f64>, right: &BTreeMap<String, f64>) -> usize {
    if left.keys().ne(right.keys()) {
        return usize::MAX;
    }
    left.iter()
        .zip(right)
        .filter(|((_, left_value), (_, right_value))| left_value.to_bits() != right_value.to_bits())
        .count()
}

fn euclidean_parameter_drift(left: &BTreeMap<String, f64>, right: &BTreeMap<String, f64>) -> f64 {
    let keys: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    keys.iter()
        .map(|key| {
            let difference =
                left.get(*key).copied().unwrap_or(0.0) - right.get(*key).copied().unwrap_or(0.0);
            difference.powi(2)
        })
        .sum::<f64>()
        .sqrt()
}

fn parameter_key(parameters: &BTreeMap<String, f64>) -> String {
    parameters
        .iter()
        .map(|(name, value)| format!("{name}:{:016x}", value.to_bits()))
        .collect::<Vec<_>>()
        .join("|")
}

fn metric(
    value: f64,
    unit: Unit,
    origin: FactOrigin,
    refs: &[String],
    request: &RobustnessRequest,
) -> Metric {
    Metric {
        value: canonical_zero(value),
        unit,
        origin,
        evidence_refs: canonical_refs(refs),
        replay_ref: request.source.bindings.replay_ref.clone(),
    }
}

fn sorted_metrics(
    values: &[f64],
    unit: Unit,
    origin: FactOrigin,
    refs: &[String],
    request: &RobustnessRequest,
) -> Vec<Metric> {
    let mut values = values.to_vec();
    values.sort_by(|left, right| total_cmp(*left, *right));
    values
        .into_iter()
        .map(|value| metric(value, unit, origin, refs, request))
        .collect()
}

fn aggregate_refs(source: &VerifiedBacktest, additional: &[String]) -> Vec<String> {
    let mut refs = source.bindings.evidence_refs.clone();
    refs.extend(additional.iter().cloned());
    canonical_refs(&refs)
}

fn mode_refs(mode: &MonteCarloMode) -> &[String] {
    match mode {
        MonteCarloMode::ParameterPerturbation { model } => &model.evidence_refs,
        MonteCarloMode::SyntheticPath { model } => &model.evidence_refs,
        _ => &[],
    }
}

fn canonical_refs(refs: &[String]) -> Vec<String> {
    refs.iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn canonical_bindings(bindings: &SourceBindings) -> SourceBindings {
    let mut value = bindings.clone();
    value.evidence_refs = canonical_refs(&bindings.evidence_refs);
    value
}

fn result_hash(result: &RobustnessResult) -> Result<String, EngineError> {
    let mut payload = result.clone();
    payload.result_hash.clear();
    let bytes = serde_json_canonicalizer::to_vec(&payload).map_err(|_| {
        error(
            "result_serialization_failed",
            "result could not be canonicalized",
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn unavailable(study: &str, code: &str, missing: &[&str]) -> Projection {
    unavailable_owned(
        study,
        code,
        missing.iter().map(|value| value.to_string()).collect(),
    )
}

fn unavailable_owned(study: &str, code: &str, mut missing: Vec<String>) -> Projection {
    missing.sort();
    missing.dedup();
    (
        ResultStatus::Unavailable,
        StudyOutput::Unavailable,
        vec![diagnostic(
            code,
            &format!("{study} output is unavailable because required evidence is absent"),
            &missing,
        )],
    )
}

fn diagnostic(code: &str, detail: &str, missing: &[impl AsRef<str>]) -> Diagnostic {
    Diagnostic {
        code: code.to_string(),
        detail: detail.to_string(),
        missing_evidence: missing
            .iter()
            .map(|value| value.as_ref().to_string())
            .collect(),
    }
}

fn validate_returns(returns: &[f64]) -> Result<(), EngineError> {
    for value in returns {
        finite("return_non_finite", *value)?;
        if *value <= -1.0 {
            return invalid("return_invalid", "return ratios must be greater than -1");
        }
    }
    Ok(())
}

fn validate_parameters(parameters: &BTreeMap<String, f64>) -> Result<(), EngineError> {
    if parameters.is_empty() || parameters.keys().any(|key| key.trim().is_empty()) {
        return invalid(
            "parameter_set_invalid",
            "parameter sets must contain named values",
        );
    }
    for value in parameters.values() {
        finite("parameter_non_finite", *value)?;
    }
    Ok(())
}

fn require_refs(refs: &[String], code: &str) -> Result<(), EngineError> {
    if refs.iter().all(|value| value.trim().is_empty()) {
        return invalid(code, "at least one evidence reference is required");
    }
    Ok(())
}

fn bounded_count(count: usize, budget: usize, code: &str) -> Result<(), EngineError> {
    if count > budget {
        return invalid(code, "study input count exceeds the declared budget");
    }
    Ok(())
}

fn checked_work(left: usize, right: usize, budget: usize, code: &str) -> Result<(), EngineError> {
    if checked_product(left, right)? > budget {
        return invalid(code, "study work exceeds the declared budget");
    }
    Ok(())
}

fn checked_product(left: usize, right: usize) -> Result<usize, EngineError> {
    left.checked_mul(right)
        .ok_or_else(|| error("budget_overflow", "study budget calculation overflow"))
}

fn checked_sum(values: impl IntoIterator<Item = usize>) -> Result<usize, EngineError> {
    values.into_iter().try_fold(0_usize, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| error("budget_overflow", "study budget calculation overflow"))
    })
}

fn finite(code: &str, value: f64) -> Result<(), EngineError> {
    if !value.is_finite() {
        return invalid(code, "numeric input must be finite");
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn study_kind(study: &StudyInput) -> StudyKind {
    match study {
        StudyInput::MonteCarlo(_) => StudyKind::MonteCarlo,
        StudyInput::ParameterSensitivity(_) => StudyKind::ParameterSensitivity,
        StudyInput::FeeSensitivity(_) => StudyKind::FeeSensitivity,
        StudyInput::SlippageSensitivity(_) => StudyKind::SlippageSensitivity,
        StudyInput::ExecutionSensitivity(_) => StudyKind::ExecutionSensitivity,
        StudyInput::WalkForward(_) => StudyKind::WalkForward,
        StudyInput::Regime(_) => StudyKind::Regime,
        StudyInput::Stress(_) => StudyKind::Stress,
        StudyInput::CapacityLiquidity(_) => StudyKind::CapacityLiquidity,
    }
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn maximum(values: &[f64]) -> f64 {
    values
        .iter()
        .copied()
        .max_by(|left, right| total_cmp(*left, *right))
        .unwrap_or(0.0)
}

fn minimum(values: &[f64]) -> f64 {
    values
        .iter()
        .copied()
        .min_by(|left, right| total_cmp(*left, *right))
        .unwrap_or(0.0)
}

fn total_cmp(left: f64, right: f64) -> Ordering {
    left.total_cmp(&right)
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

fn invalid<T>(code: &str, detail: &str) -> Result<T, EngineError> {
    Err(error(code, detail))
}

fn error(code: &str, detail: &str) -> EngineError {
    EngineError {
        code: code.to_string(),
        detail: detail.to_string(),
    }
}

#[derive(Clone, Debug)]
struct DeterministicRng {
    state: u64,
    spare_normal: Option<f64>,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
            spare_normal: None,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.state = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) / ((1_u64 << 53) as f64)
    }

    fn range(&mut self, minimum: f64, maximum: f64) -> f64 {
        minimum + (maximum - minimum) * self.uniform()
    }

    fn index(&mut self, length: usize) -> usize {
        ((self.next_u64() as u128 * length as u128) >> 64) as usize
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let selected = self.index(index + 1);
            values.swap(index, selected);
        }
    }

    fn normal(&mut self) -> f64 {
        if let Some(value) = self.spare_normal.take() {
            return value;
        }
        let first = self.uniform().max(f64::MIN_POSITIVE);
        let second = self.uniform();
        let radius = (-2.0 * first.ln()).sqrt();
        let angle = std::f64::consts::TAU * second;
        self.spare_normal = Some(radius * angle.sin());
        radius * angle.cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    fn source() -> VerifiedBacktest {
        VerifiedBacktest {
            verification: VerificationStatus::IntegrityVerifiedCompleted,
            bindings: SourceBindings {
                run_id: "run-1".to_string(),
                manifest_hash: hash('a'),
                result_hash: hash('b'),
                report_hash: hash('c'),
                dataset_hash: hash('d'),
                strategy_spec_hash: hash('e'),
                evidence_refs: vec!["evidence://source".to_string()],
                replay_ref: "replay://run-1".to_string(),
            },
            units: SourceUnits {
                reporting_currency: "USD".to_string(),
                equity: Unit::CurrencyMicros,
                trade_return: Unit::Ratio,
                cost: Unit::CurrencyMicros,
                quantity: Unit::Quantity,
            },
            starting_equity_micros: 100_000_000,
            trades: vec![trade("t-1", 0.10), trade("t-2", -0.05), trade("t-3", 0.02)],
            equity: vec![
                equity(1, 110_000_000),
                equity(2, 104_500_000),
                equity(3, 106_590_000),
            ],
        }
    }

    fn trade(id: &str, return_ratio: f64) -> ObservedTrade {
        ObservedTrade {
            trade_id: id.to_string(),
            sequence: id
                .strip_prefix("t-")
                .and_then(|value| value.parse().ok())
                .unwrap_or(1),
            return_ratio,
            pnl_micros: (return_ratio * 100_000_000.0) as i64,
            fee_micros: 10_000,
            slippage_bps: 1.0,
            quantity: 10.0,
            evidence_refs: vec![format!("evidence://{id}")],
        }
    }

    fn equity(sequence: u64, equity_micros: i64) -> ObservedEquityPoint {
        ObservedEquityPoint {
            sequence,
            equity_micros,
            evidence_refs: vec![format!("evidence://equity/{sequence}")],
        }
    }

    fn budgets() -> StudyBudget {
        StudyBudget {
            max_samples: 1_000,
            max_grid_points: 1_000,
            max_windows: 1_000,
            max_scenarios: 1_000,
            max_path_points: 100_000,
            max_output_points: 100_000,
            max_memory_bytes: 10_000_000,
        }
    }

    fn request(study: StudyInput) -> RobustnessRequest {
        let source = source();
        let mut run_ids = required_study_sources(&study)
            .into_iter()
            .collect::<BTreeSet<_>>();
        run_ids.remove(source.bindings.run_id.as_str());
        let supporting_sources = run_ids
            .into_iter()
            .map(|run_id| {
                let mut supporting = source.clone();
                supporting.bindings.run_id = run_id.to_string();
                supporting.bindings.replay_ref = format!("replay://{run_id}");
                supporting
            })
            .collect();
        RobustnessRequest {
            schema: ROBUSTNESS_REQUEST_SCHEMA.to_string(),
            engine_version: ROBUSTNESS_ENGINE_VERSION.to_string(),
            seed: 42,
            source,
            supporting_sources,
            budgets: budgets(),
            study,
        }
    }

    fn monte_carlo(mode: MonteCarloMode) -> StudyInput {
        StudyInput::MonteCarlo(MonteCarloInput {
            path_count: 64,
            confidence_level: 0.90,
            ruin_equity_ratio: 0.50,
            mode,
        })
    }

    #[test]
    fn identical_input_has_identical_hash_and_canonical_bytes() {
        let request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        let first = run(&request).unwrap();
        let second = run(&request).unwrap();
        assert_eq!(first, second);
        assert!(verify_result_hash(&first).unwrap());
        assert_eq!(
            serde_json_canonicalizer::to_vec(&first).unwrap(),
            serde_json_canonicalizer::to_vec(&second).unwrap()
        );
    }

    #[test]
    fn different_seed_changes_stochastic_output_and_hash() {
        let first = run(&request(monte_carlo(MonteCarloMode::TradeBootstrap))).unwrap();
        let mut second_request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        second_request.seed = 43;
        let second = run(&second_request).unwrap();
        assert_ne!(first.result_hash, second.result_hash);
        assert_ne!(first.output, second.output);
        assert_eq!(first.source_bindings, second.source_bindings);
    }

    #[test]
    fn observed_study_input_must_reference_an_exact_bound_source() {
        let mut request = request(StudyInput::ParameterSensitivity(
            ParameterSensitivityInput {
                cases: vec![parameter_case(1.0, vec![0.1])],
                top_count: 1,
                plateau_tolerance_ratio: 0.01,
            },
        ));
        request.supporting_sources.clear();
        let error = run(&request).expect_err("unbound observed source");
        assert_eq!(error.code, "study_source_not_bound");
    }

    #[test]
    fn supporting_sources_must_share_the_reporting_currency() {
        let mut request = request(StudyInput::ParameterSensitivity(
            ParameterSensitivityInput {
                cases: vec![parameter_case(1.0, vec![0.1])],
                top_count: 1,
                plateau_tolerance_ratio: 0.01,
            },
        ));
        request.supporting_sources[0].units.reporting_currency = "EUR".to_string();
        let error = run(&request).expect_err("cross-currency source");
        assert_eq!(error.code, "source_currency_incompatible");
    }

    #[test]
    fn result_hash_detects_tampering() {
        let mut result = run(&request(monte_carlo(MonteCarloMode::TradeBootstrap))).unwrap();
        result.seed += 1;
        assert!(!verify_result_hash(&result).unwrap());
    }

    #[test]
    fn monte_carlo_reshuffle_emits_required_distributions() {
        let result = run(&request(monte_carlo(MonteCarloMode::TradeOrderReshuffle))).unwrap();
        let StudyOutput::MonteCarlo(output) = result.output else {
            panic!("Monte Carlo result");
        };
        assert_eq!(output.path_count, 64);
        assert_eq!(output.confidence_path.len(), 3);
        assert_eq!(output.terminal_distribution.len(), 64);
        assert_eq!(output.drawdown_distribution.len(), 64);
        assert_eq!(output.longest_losing_streak_distribution.len(), 64);
        assert_eq!(output.loss_probability.unit, Unit::Probability);
        assert_eq!(output.ruin_probability.origin, FactOrigin::ModelDerived);
    }

    #[test]
    fn bootstrap_is_bounded_before_execution() {
        let mut request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        request.budgets.max_samples = 10;
        let error = run(&request).unwrap_err();
        assert_eq!(error.code, "sample_budget_exceeded");
    }

    #[test]
    fn path_memory_budget_is_enforced() {
        let mut request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        request.budgets.max_memory_bytes = 16;
        let error = run(&request).unwrap_err();
        assert_eq!(error.code, "memory_budget_exceeded");
    }

    #[test]
    fn missing_observed_trades_is_unavailable_not_success_with_zeroes() {
        let mut request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        request.source.trades.clear();
        let result = run(&request).unwrap();
        assert_eq!(result.status, ResultStatus::Unavailable);
        assert_eq!(result.output, StudyOutput::Unavailable);
        assert_eq!(result.diagnostics[0].code, "observed_trade_returns_missing");
    }

    #[test]
    fn execution_randomization_stays_within_declared_model() {
        let result = run(&request(monte_carlo(
            MonteCarloMode::ExecutionRandomization {
                minimum_cost_bps: 1.0,
                maximum_cost_bps: 5.0,
            },
        )))
        .unwrap();
        assert_eq!(result.status, ResultStatus::Available);
        assert!(matches!(result.output, StudyOutput::MonteCarlo(_)));
    }

    #[test]
    fn parameter_perturbation_requires_versioned_model() {
        let mut model = ParameterPerturbationModel {
            schema: "unsupported".to_string(),
            minimum_return_factor: 0.8,
            maximum_return_factor: 1.2,
            evidence_refs: vec!["model://parameter".to_string()],
        };
        let error = run(&request(monte_carlo(
            MonteCarloMode::ParameterPerturbation {
                model: model.clone(),
            },
        )))
        .unwrap_err();
        assert_eq!(error.code, "parameter_model_invalid");
        model.schema = PARAMETER_PERTURBATION_MODEL_V1.to_string();
        assert!(run(&request(monte_carlo(
            MonteCarloMode::ParameterPerturbation { model },
        )))
        .is_ok());
    }

    #[test]
    fn synthetic_paths_only_succeed_with_complete_versioned_model() {
        let model = SyntheticPathModel {
            schema: SYNTHETIC_LOG_RETURN_MODEL_V1.to_string(),
            horizon_steps: 10,
            mean_log_return_ratio: 0.001,
            volatility_ratio: 0.01,
            evidence_refs: vec!["model://synthetic/v1".to_string()],
        };
        let result = run(&request(monte_carlo(MonteCarloMode::SyntheticPath {
            model: model.clone(),
        })))
        .unwrap();
        let StudyOutput::MonteCarlo(output) = result.output else {
            panic!("Monte Carlo result");
        };
        assert_eq!(output.confidence_path.len(), 10);

        let mut invalid = model;
        invalid.evidence_refs.clear();
        let error = run(&request(monte_carlo(MonteCarloMode::SyntheticPath {
            model: invalid,
        })))
        .unwrap_err();
        assert_eq!(error.code, "synthetic_model_evidence_missing");
    }

    #[test]
    fn non_finite_inputs_fail_closed() {
        let mut request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        request.source.trades[0].return_ratio = f64::NAN;
        let error = run(&request).unwrap_err();
        assert_eq!(error.code, "trade_return_non_finite");
    }

    #[test]
    fn trade_sequence_must_be_explicit_and_strictly_ordered() {
        let mut request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        request.source.trades[1].sequence = request.source.trades[0].sequence;
        let error = run(&request).unwrap_err();
        assert_eq!(error.code, "trade_identity_invalid");
    }

    #[test]
    fn parameter_sensitivity_is_canonically_ordered_and_warns_on_peak() {
        let cases = vec![
            parameter_case(2.0, vec![0.30]),
            parameter_case(1.0, vec![0.01]),
            parameter_case(3.0, vec![0.02]),
        ];
        let result = run(&request(StudyInput::ParameterSensitivity(
            ParameterSensitivityInput {
                cases,
                top_count: 2,
                plateau_tolerance_ratio: 0.01,
            },
        )))
        .unwrap();
        let StudyOutput::ParameterSensitivity(output) = result.output else {
            panic!("parameter sensitivity result");
        };
        assert_eq!(output.tested_grid[0].parameters["lookback"], 1.0);
        assert_eq!(output.top_sets[0].parameters["lookback"], 2.0);
        assert_eq!(output.warnings[0].code, "isolated_parameter_peak");
        assert_eq!(
            output.top_sets[0].return_ratio.origin,
            FactOrigin::ObservedBacktest
        );
    }

    #[test]
    fn empty_parameter_grid_is_explicitly_unavailable() {
        let result = run(&request(StudyInput::ParameterSensitivity(
            ParameterSensitivityInput {
                cases: Vec::new(),
                top_count: 1,
                plateau_tolerance_ratio: 0.01,
            },
        )))
        .unwrap();
        assert_eq!(result.status, ResultStatus::Unavailable);
        assert_eq!(result.diagnostics[0].code, "parameter_grid_missing");
    }

    #[test]
    fn fee_sensitivity_calculates_bounded_break_even_and_drag() {
        let input = cost_input(vec![
            cost_case("base", 0.0, 0.0, "observed", vec![0.10]),
            cost_case("high", 20.0, 0.0, "observed", vec![-0.10]),
        ]);
        let result = run(&request(StudyInput::FeeSensitivity(input))).unwrap();
        let StudyOutput::FeeSensitivity(output) = result.output else {
            panic!("fee sensitivity result");
        };
        assert!((output.break_even_bps.unwrap().value - 10.0).abs() < 1e-12);
        assert!(output.cases[1].cost_drag_ratio.value > 0.0);
    }

    #[test]
    fn slippage_and_execution_sensitivity_have_distinct_outputs() {
        let input = cost_input(vec![
            cost_case("base", 0.0, 0.0, "close", vec![0.05]),
            cost_case("tested", 0.0, 5.0, "next_open", vec![-0.01]),
        ]);
        let slippage = run(&request(StudyInput::SlippageSensitivity(input.clone()))).unwrap();
        let execution = run(&request(StudyInput::ExecutionSensitivity(input))).unwrap();
        assert!(matches!(
            slippage.output,
            StudyOutput::SlippageSensitivity(_)
        ));
        let StudyOutput::ExecutionSensitivity(output) = execution.output else {
            panic!("execution sensitivity result");
        };
        assert!(output.break_even_bps.is_none());
        assert_eq!(output.flipped_outcome_count.value, 1.0);
    }

    #[test]
    fn cost_sensitivity_requires_matching_trade_coverage() {
        let input = cost_input(vec![
            cost_case("base", 0.0, 0.0, "close", vec![0.05, 0.01]),
            cost_case("tested", 0.0, 5.0, "next_open", vec![-0.01]),
        ]);
        let error = run(&request(StudyInput::SlippageSensitivity(input))).unwrap_err();
        assert_eq!(error.code, "cost_case_trade_count_mismatch");
    }

    #[test]
    fn unbounded_break_even_is_diagnostic_not_fabricated() {
        let input = cost_input(vec![
            cost_case("base", 0.0, 0.0, "close", vec![0.05]),
            cost_case("tested", 0.0, 5.0, "close", vec![0.01]),
        ]);
        let result = run(&request(StudyInput::SlippageSensitivity(input))).unwrap();
        let StudyOutput::SlippageSensitivity(output) = result.output else {
            panic!("slippage result");
        };
        assert!(output.break_even_bps.is_none());
        assert_eq!(result.diagnostics[0].code, "break_even_unbounded");
    }

    #[test]
    fn walk_forward_reports_degradation_pass_rate_drift_and_drawdown() {
        let result = run(&request(StudyInput::WalkForward(WalkForwardInput {
            minimum_oos_return_ratio: 0.0,
            windows: vec![
                walk_window("w2", 20, 30, 2.0, vec![0.10], vec![-0.05]),
                walk_window("w1", 1, 10, 1.0, vec![0.10], vec![0.05]),
            ],
        })))
        .unwrap();
        let StudyOutput::WalkForward(output) = result.output else {
            panic!("walk-forward result");
        };
        assert_eq!(output.windows[0].window_id, "w1");
        assert_eq!(output.pass_rate.value, 0.5);
        assert_eq!(output.windows[1].parameter_drift.value, 1.0);
        assert!(output.windows[1].out_of_sample_drawdown_ratio.value > 0.0);
    }

    #[test]
    fn regime_preserves_observed_labels_units_and_optional_exposure() {
        let result = run(&request(StudyInput::Regime(RegimeInput {
            segmentation_definition: "verified volatility quartiles v1".to_string(),
            segments: vec![RegimeSegment {
                source_run_id: "run-regime-high".to_string(),
                regime: "high".to_string(),
                trade_returns: vec![0.10, -0.05],
                costs_micros: 25_000,
                exposure_ratio: Some(0.4),
                evidence_refs: vec!["evidence://regime/high".to_string()],
            }],
        })))
        .unwrap();
        let StudyOutput::Regime(output) = result.output else {
            panic!("regime result");
        };
        assert_eq!(output.regimes[0].trade_count.value, 2.0);
        assert_eq!(output.regimes[0].costs_micros.unit, Unit::CurrencyMicros);
        assert_eq!(
            output.regimes[0].exposure_ratio.as_ref().unwrap().origin,
            FactOrigin::ObservedBacktest
        );
    }

    #[test]
    fn stress_labels_historical_and_hypothetical_origins() {
        let result = run(&request(StudyInput::Stress(StressInput {
            scenarios: vec![
                stress_case("historical", StressKind::HistoricalObserved),
                stress_case("declared", StressKind::HypotheticalDeclared),
            ],
        })))
        .unwrap();
        let StudyOutput::Stress(output) = result.output else {
            panic!("stress result");
        };
        let declared = output
            .scenarios
            .iter()
            .find(|scenario| scenario.name == "declared")
            .unwrap();
        let historical = output
            .scenarios
            .iter()
            .find(|scenario| scenario.name == "historical")
            .unwrap();
        assert_eq!(declared.pnl_micros.origin, FactOrigin::ModelDerived);
        assert_eq!(historical.pnl_micros.origin, FactOrigin::ObservedBacktest);
        assert_eq!(declared.exposure_ratio.as_ref().unwrap().unit, Unit::Ratio);
        assert_eq!(declared.failing_trade_count.value, 1.0);
    }

    #[test]
    fn capacity_requires_real_volume_and_spread_evidence() {
        let mut input = capacity_input();
        input.observations[0].observed_volume = None;
        let result = run(&request(StudyInput::CapacityLiquidity(input))).unwrap();
        assert_eq!(result.status, ResultStatus::Unavailable);
        assert_eq!(result.diagnostics[0].code, "liquidity_fields_missing");
        assert_eq!(result.diagnostics[0].missing_evidence, vec!["t-1"]);
    }

    #[test]
    fn capacity_reports_participation_degradation_rejections_and_bottlenecks() {
        let result = run(&request(StudyInput::CapacityLiquidity(capacity_input()))).unwrap();
        let StudyOutput::CapacityLiquidity(output) = result.output else {
            panic!("capacity result");
        };
        assert_eq!(output.scale_points.len(), 2);
        assert_eq!(output.scale_points[0].scale.value, 1.0);
        assert!(output.scale_points[1].degradation_ratio.value > 0.0);
        assert_eq!(output.scale_points[1].rejected_trade_count.value, 1.0);
        assert_eq!(output.scale_points[1].bottleneck_trade_ids, vec!["t-1"]);
    }

    #[test]
    fn empty_capacity_grid_is_unavailable() {
        let mut input = capacity_input();
        input.scale_points.clear();
        let result = run(&request(StudyInput::CapacityLiquidity(input))).unwrap();
        assert_eq!(result.status, ResultStatus::Unavailable);
        assert_eq!(result.diagnostics[0].code, "capacity_scale_grid_missing");
    }

    #[test]
    fn non_monte_carlo_work_budget_is_enforced() {
        let mut request = request(StudyInput::Regime(RegimeInput {
            segmentation_definition: "verified segmentation".to_string(),
            segments: vec![RegimeSegment {
                source_run_id: "run-regime-one".to_string(),
                regime: "one".to_string(),
                trade_returns: vec![0.01, 0.02],
                costs_micros: 0,
                exposure_ratio: None,
                evidence_refs: vec!["evidence://regime".to_string()],
            }],
        }));
        request.budgets.max_path_points = 1;
        let error = run(&request).unwrap_err();
        assert_eq!(error.code, "work_budget_exceeded");
    }

    #[test]
    fn source_hashes_and_units_fail_closed() {
        let mut invalid_hash_request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        invalid_hash_request.source.bindings.dataset_hash = "sha256:BAD".to_string();
        assert_eq!(
            run(&invalid_hash_request).unwrap_err().code,
            "source_hash_invalid"
        );

        let mut invalid_units_request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        invalid_units_request.source.units.trade_return = Unit::BasisPoints;
        assert_eq!(
            run(&invalid_units_request).unwrap_err().code,
            "source_units_invalid"
        );
    }

    #[test]
    fn evidence_refs_are_canonicalized_in_metrics_and_bindings() {
        let mut request = request(monte_carlo(MonteCarloMode::TradeBootstrap));
        request.source.bindings.evidence_refs = vec![
            " evidence://z ".to_string(),
            "evidence://a".to_string(),
            "evidence://a".to_string(),
        ];
        let result = run(&request).unwrap();
        assert_eq!(
            result.source_bindings.evidence_refs,
            vec!["evidence://a", "evidence://z"]
        );
        let StudyOutput::MonteCarlo(output) = result.output else {
            panic!("Monte Carlo result");
        };
        assert_eq!(
            output.loss_probability.evidence_refs,
            vec!["evidence://a", "evidence://z"]
        );
    }

    fn parameter_case(value: f64, returns: Vec<f64>) -> ParameterCase {
        ParameterCase {
            source_run_id: format!("run-parameter-{value}"),
            parameters: BTreeMap::from([("lookback".to_string(), value)]),
            trade_returns: returns,
            evidence_refs: vec![format!("evidence://parameter/{value}")],
        }
    }

    fn cost_case(
        label: &str,
        fee_bps: f64,
        slippage_bps: f64,
        execution_model: &str,
        trade_returns: Vec<f64>,
    ) -> CostCase {
        CostCase {
            source_run_id: format!("run-cost-{label}"),
            label: label.to_string(),
            fee_bps,
            slippage_bps,
            execution_model: execution_model.to_string(),
            trade_returns,
            evidence_refs: vec![format!("evidence://cost/{label}")],
        }
    }

    fn cost_input(cases: Vec<CostCase>) -> CostSensitivityInput {
        CostSensitivityInput {
            baseline_label: "base".to_string(),
            cases,
        }
    }

    fn walk_window(
        id: &str,
        start: u64,
        end: u64,
        parameter: f64,
        in_sample_returns: Vec<f64>,
        out_of_sample_returns: Vec<f64>,
    ) -> WalkForwardWindow {
        WalkForwardWindow {
            window_id: id.to_string(),
            in_sample_source_run_id: format!("run-{id}-in"),
            out_of_sample_source_run_id: format!("run-{id}-out"),
            start_sequence: start,
            end_sequence: end,
            in_sample_returns,
            out_of_sample_returns,
            selected_parameters: BTreeMap::from([("lookback".to_string(), parameter)]),
            evidence_refs: vec![format!("evidence://window/{id}")],
        }
    }

    fn stress_case(name: &str, kind: StressKind) -> StressScenario {
        StressScenario {
            name: name.to_string(),
            kind,
            source_run_id: (kind == StressKind::HistoricalObserved)
                .then(|| format!("run-stress-{name}")),
            shocked_trade_returns: vec![-0.20, 0.25],
            exposure_ratio: Some(0.75),
            contribution_micros: BTreeMap::from([("t-1".to_string(), -20_000_000)]),
            evidence_refs: vec![format!("evidence://stress/{name}")],
        }
    }

    fn capacity_input() -> CapacityLiquidityInput {
        CapacityLiquidityInput {
            observations: vec![LiquidityObservation {
                source_run_id: "run-capacity".to_string(),
                trade_id: "t-1".to_string(),
                return_ratio: 0.05,
                quantity: 10.0,
                observed_volume: Some(100.0),
                observed_spread_bps: Some(2.0),
                open_interest: Some(25.0),
                displayed_depth: Some(20.0),
                evidence_refs: vec!["evidence://liquidity/t-1".to_string()],
            }],
            scale_points: vec![3.0, 1.0],
            maximum_participation_ratio: 0.20,
            impact_bps_per_participation_ratio: 10.0,
            reject_above_participation: true,
        }
    }
}
