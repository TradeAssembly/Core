use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum SpecVersion {
    #[serde(rename = "3.0")]
    V3,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortfolioScope {
    Strategy,
    Account,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StageName {
    Inputs,
    Universe,
    Evaluate,
    Intent,
    Sizing,
    Risk,
    ExitPolicy,
    OrderStrategy,
    PricePolicy,
    MarketClock,
}

impl StageName {
    pub const TRANSFORMATION_ORDER: [Self; 9] = [
        Self::Inputs,
        Self::Universe,
        Self::Evaluate,
        Self::Intent,
        Self::Sizing,
        Self::Risk,
        Self::ExitPolicy,
        Self::OrderStrategy,
        Self::PricePolicy,
    ];

    pub const VALIDATION_ORDER: [Self; 10] = [
        Self::MarketClock,
        Self::Inputs,
        Self::Universe,
        Self::Evaluate,
        Self::Intent,
        Self::Sizing,
        Self::Risk,
        Self::ExitPolicy,
        Self::OrderStrategy,
        Self::PricePolicy,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inputs => "inputs",
            Self::Universe => "universe",
            Self::Evaluate => "evaluate",
            Self::Intent => "intent",
            Self::Sizing => "sizing",
            Self::Risk => "risk",
            Self::ExitPolicy => "exit_policy",
            Self::OrderStrategy => "order_strategy",
            Self::PricePolicy => "price_policy",
            Self::MarketClock => "market_clock",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Equity,
    Fund,
    Crypto,
    Option,
    Future,
    FixedIncome,
    Fx,
    EventContract,
    Custom,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentFamily {
    Equity,
    Fund,
    CryptoSpot,
    OptionContract,
    FutureContract,
    FixedIncome,
    FxPair,
    EventContract,
    Custom,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignalMarketRole {
    PrimarySignal,
    Context,
    Benchmark,
    HedgeInput,
    ScreenerSeed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AvailabilityRequirements {
    pub historical: bool,
    pub realtime: bool,
    pub delayed_ok: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QualityRequirements {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_freshness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordering: Option<String>,
    pub replayable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_history: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstrumentSelector {
    pub selector_id: String,
    #[serde(rename = "type")]
    pub selector_type: SelectorType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectorType {
    NormalizedInstrument,
    SymbolicAlias,
    Dynamic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignalMarketRequirement {
    pub requirement_id: String,
    pub asset_class: AssetClass,
    pub instrument_family: InstrumentFamily,
    pub role: SignalMarketRole,
    pub data_shapes: Vec<String>,
    pub fields: Vec<String>,
    pub timeframes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookback: Option<String>,
    pub availability: AvailabilityRequirements,
    #[serde(default)]
    pub calendar_refs: Vec<String>,
    pub selectors: Vec<InstrumentSelector>,
    pub quality: QualityRequirements,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TradeMarketRequirement {
    pub requirement_id: String,
    pub asset_classes: Vec<AssetClass>,
    pub instrument_families: Vec<InstrumentFamily>,
    #[serde(default)]
    pub signal_requirement_refs: Vec<String>,
    pub supports_single_leg: bool,
    pub supports_multi_leg: bool,
    pub supports_mixed_instruments: bool,
    #[serde(default)]
    pub allowed_currencies: Vec<String>,
    #[serde(default)]
    pub settlement_types: Vec<String>,
    #[serde(default)]
    pub quantity_units: Vec<String>,
    #[serde(default)]
    pub lifecycle_constraints: Vec<String>,
    pub required_capability_refs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessLevel {
    Authoring,
    Research,
    Backtest,
    Simulation,
    Paper,
    Live,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    None,
    ExplicitOnly,
    Configured,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityConstraints {
    #[serde(default)]
    pub instrument_families: Vec<InstrumentFamily>,
    #[serde(default)]
    pub data_shapes: Vec<String>,
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub input_paths: Vec<String>,
    #[serde(default)]
    pub schema_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeframe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_freshness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_latency_ms: Option<u64>,
    #[serde(default)]
    pub deterministic: bool,
    #[serde(default)]
    pub replayable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequirement {
    pub requirement_id: String,
    pub capability: String,
    pub purpose: String,
    pub required_for: Vec<ReadinessLevel>,
    pub stage_refs: Vec<StageName>,
    #[serde(default)]
    pub substep_refs: Vec<String>,
    pub constraints: CapabilityConstraints,
    #[serde(default)]
    pub dependency_refs: Vec<String>,
    pub fallback_policy: FallbackPolicy,
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequirements {
    pub required: Vec<CapabilityRequirement>,
    #[serde(default)]
    pub optional: Vec<CapabilityRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VariableType {
    Integer,
    Decimal,
    Boolean,
    String,
    Duration,
    Enum,
    Json,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverrideBounds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    #[serde(rename = "type")]
    pub variable_type: VariableType,
    pub default: Value,
    pub stage_ref: StageName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub substep_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_schema_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub runtime_override_allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_bounds: Option<OverrideBounds>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiMetadata>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiMetadata {
    pub control: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AllocationMode {
    None,
    RelativeWeights,
    Formula,
    Operation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AllocationTarget {
    pub selector_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_weight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<Expression>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub mode: AllocationMode,
    #[serde(default)]
    pub targets: Vec<AllocationTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<Expression>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_ref: Option<String>,
    #[serde(default)]
    pub capability_requirement_refs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ExpressionLanguage {
    #[serde(rename = "cel@1")]
    CelV1,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub language: ExpressionLanguage,
    pub source: String,
    pub result_type: String,
    pub max_cost: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    FailClosed,
    SkipOptional,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputContract {
    pub schema_version: String,
    pub journalable: bool,
    pub replayable: bool,
    pub content_addressable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StateContract {
    pub schema_ref: String,
    pub version: String,
    pub checkpoint_namespace: String,
    pub initialization: String,
    pub reset: String,
    pub migration_compatibility: String,
    pub event_time: String,
    pub ordering: String,
    pub replay: String,
    pub sharing_scope: StateSharingScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing_authorization_capability_ref: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateSharingScope {
    Instrument,
    Account,
    Activation,
    ResearchRun,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubstepKind {
    Fetch,
    Transform,
    Screen,
    Calculate,
    Signal,
    Combine,
    Intent,
    Size,
    Guard,
    Exit,
    Order,
    Price,
    Clock,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Substep {
    pub id: String,
    pub kind: SubstepKind,
    pub operation_ref: String,
    pub capability_requirement_refs: Vec<String>,
    #[serde(default)]
    pub reads: Vec<String>,
    #[serde(default)]
    pub writes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema_ref: Option<String>,
    pub output_contract: OutputContract,
    pub optional: bool,
    pub failure_policy: FailurePolicy,
    #[serde(default)]
    pub selector_refs: Vec<String>,
    #[serde(default)]
    pub config: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StateContract>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub enabled: bool,
    #[serde(default)]
    pub substeps: Vec<Substep>,
    #[serde(default)]
    pub reads: Vec<String>,
    #[serde(default)]
    pub writes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema_ref: Option<String>,
    pub output_contract: OutputContract,
    pub failure_policy: FailurePolicy,
    #[serde(default)]
    pub config: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitMonitoringMode {
    BrokerNative,
    Synthetic,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitScope {
    FullPosition,
    IndividualLegs,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitTriggerKind {
    Indicator,
    Expression,
    Time,
    Price,
    ProfitLoss,
    Lifecycle,
    Operation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExitTrigger {
    pub id: String,
    pub kind: ExitTriggerKind,
    pub scope: ExitScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_requirement_ref: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitComposition {
    Any,
    All,
    Priority,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndOfDataBehavior {
    FailClosed,
    CloseSyntheticPositions,
    PreserveOpenPositions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExitPolicy {
    pub monitoring: Vec<ExitMonitoringMode>,
    pub triggers: Vec<ExitTrigger>,
    pub composition: ExitComposition,
    #[serde(default)]
    pub precedence: Vec<String>,
    pub end_of_data: EndOfDataBehavior,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
    Trailing,
    Conditional,
    MultiLeg,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MultiLegMode {
    NotApplicable,
    AtomicRequired,
    DeterministicLegSequence,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderPolicy {
    pub allowed_order_types: Vec<OrderType>,
    pub multi_leg_mode: MultiLegMode,
    #[serde(default)]
    pub time_in_force: Vec<String>,
    pub conditional_orders_allowed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PriceRounding {
    NearestTick,
    FavorableTick,
    UnfavorableTick,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PricePolicy {
    pub snapshot_inputs: Vec<String>,
    pub source_precedence: Vec<String>,
    pub max_staleness: String,
    pub rounding: PriceRounding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<Expression>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClockWindow {
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HolidayBehavior {
    FollowCalendar,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EarlyCloseBehavior {
    FollowCalendar,
    Exclude,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClockPolicy {
    pub timezone: String,
    pub calendar_refs: Vec<String>,
    pub windows: Vec<ClockWindow>,
    pub days: Vec<String>,
    pub holiday_behavior: HolidayBehavior,
    pub early_close_behavior: EarlyCloseBehavior,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_capability_requirement_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eligibility_expression: Option<Expression>,
}

macro_rules! policy_stage {
    ($name:ident, $policy:ty) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub enabled: bool,
            #[serde(default)]
            pub substeps: Vec<Substep>,
            #[serde(default)]
            pub reads: Vec<String>,
            #[serde(default)]
            pub writes: Vec<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub input_schema_ref: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub output_schema_ref: Option<String>,
            pub output_contract: OutputContract,
            pub failure_policy: FailurePolicy,
            #[serde(default)]
            pub config: BTreeMap<String, Value>,
            pub policy: $policy,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub documentation: Option<String>,
        }
    };
}

policy_stage!(ExitPolicyStage, ExitPolicy);
policy_stage!(OrderPolicyStage, OrderPolicy);
policy_stage!(PricePolicyStage, PricePolicy);
policy_stage!(ClockPolicyStage, ClockPolicy);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stages {
    pub inputs: Stage,
    pub universe: Stage,
    pub evaluate: Stage,
    pub intent: Stage,
    pub sizing: Stage,
    pub risk: Stage,
    pub exit_policy: ExitPolicyStage,
    pub order_strategy: OrderPolicyStage,
    pub price_policy: PricePolicyStage,
    pub market_clock: ClockPolicyStage,
}

impl Stages {
    pub fn view(&self, name: &StageName) -> StageView<'_> {
        match name {
            StageName::Inputs => StageView::from_stage(&self.inputs),
            StageName::Universe => StageView::from_stage(&self.universe),
            StageName::Evaluate => StageView::from_stage(&self.evaluate),
            StageName::Intent => StageView::from_stage(&self.intent),
            StageName::Sizing => StageView::from_stage(&self.sizing),
            StageName::Risk => StageView::from_stage(&self.risk),
            StageName::ExitPolicy => StageView::from_policy_stage(
                self.exit_policy.enabled,
                &self.exit_policy.substeps,
                &self.exit_policy.reads,
                &self.exit_policy.writes,
                self.exit_policy.input_schema_ref.as_deref(),
                self.exit_policy.output_schema_ref.as_deref(),
                &self.exit_policy.config,
            ),
            StageName::OrderStrategy => StageView::from_policy_stage(
                self.order_strategy.enabled,
                &self.order_strategy.substeps,
                &self.order_strategy.reads,
                &self.order_strategy.writes,
                self.order_strategy.input_schema_ref.as_deref(),
                self.order_strategy.output_schema_ref.as_deref(),
                &self.order_strategy.config,
            ),
            StageName::PricePolicy => StageView::from_policy_stage(
                self.price_policy.enabled,
                &self.price_policy.substeps,
                &self.price_policy.reads,
                &self.price_policy.writes,
                self.price_policy.input_schema_ref.as_deref(),
                self.price_policy.output_schema_ref.as_deref(),
                &self.price_policy.config,
            ),
            StageName::MarketClock => StageView::from_policy_stage(
                self.market_clock.enabled,
                &self.market_clock.substeps,
                &self.market_clock.reads,
                &self.market_clock.writes,
                self.market_clock.input_schema_ref.as_deref(),
                self.market_clock.output_schema_ref.as_deref(),
                &self.market_clock.config,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StageView<'a> {
    pub enabled: bool,
    pub substeps: &'a [Substep],
    pub reads: &'a [String],
    pub writes: &'a [String],
    pub input_schema_ref: Option<&'a str>,
    pub output_schema_ref: Option<&'a str>,
    pub config: &'a BTreeMap<String, Value>,
}

impl<'a> StageView<'a> {
    fn from_stage(stage: &'a Stage) -> Self {
        Self {
            enabled: stage.enabled,
            substeps: &stage.substeps,
            reads: &stage.reads,
            writes: &stage.writes,
            input_schema_ref: stage.input_schema_ref.as_deref(),
            output_schema_ref: stage.output_schema_ref.as_deref(),
            config: &stage.config,
        }
    }

    fn from_policy_stage(
        enabled: bool,
        substeps: &'a [Substep],
        reads: &'a [String],
        writes: &'a [String],
        input_schema_ref: Option<&'a str>,
        output_schema_ref: Option<&'a str>,
        config: &'a BTreeMap<String, Value>,
    ) -> Self {
        Self {
            enabled,
            substeps,
            reads,
            writes,
            input_schema_ref,
            output_schema_ref,
            config,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StrategySpec {
    pub spec_version: SpecVersion,
    pub strategy_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub portfolio_scope: PortfolioScope,
    pub capability_requirements: CapabilityRequirements,
    pub signal_market_requirements: Vec<SignalMarketRequirement>,
    pub trade_market_requirements: Vec<TradeMarketRequirement>,
    pub allocation: Allocation,
    pub variables: BTreeMap<String, Variable>,
    pub stages: Stages,
}

impl StrategySpec {
    pub fn model_validate(payload: Value) -> Result<Self, String> {
        serde_json::from_value(payload).map_err(|error| error.to_string())
    }
}
