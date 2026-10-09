//! Bounded GC-11 strategy evaluation and market simulation kernel.
//!
//! The baseline deliberately supports one checked, deterministic V3 path:
//! `signal.rule.evaluate@1` with `config.expression` equal to
//! `bar.open <op> <integer>` or `bar.close <op> <integer>` (`>`, `>=`, `<`, `<=`, `==`) and
//! `config.quantity` as a positive integer. The remaining pipeline operation
//! references must match the static V3 conformance fixture. All other V3
//! semantics fail closed rather than producing inferred trade behavior.

use crate::spec::{OrderType, StrategySpec};
use serde_json::Value;
use std::collections::BTreeMap;

pub mod portfolio_capacity;
pub mod portfolio_program;
pub mod portfolio_vwap;
pub mod session_metrics;
pub mod timeline;
pub mod vwap_rules;

pub type PriceMicros = i64;
pub type MoneyMicros = i64;
pub type Quantity = i64;

const BPS_DENOMINATOR: i64 = 10_000;
const QUANTITY_SCALE: i64 = 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarketObservation {
    pub bar_index: usize,
    pub symbol: String,
    pub open: PriceMicros,
    pub close: PriceMicros,
    pub volume: Quantity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionTiming {
    SameBarOpen,
    SameBarClose,
    NextBarOpen,
    NextBarClose,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartialFillPolicy {
    Reject,
    Allow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelMode {
    Backtest,
    Paper,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationConfig {
    pub timing: ExecutionTiming,
    pub fee_per_order_micros: MoneyMicros,
    pub fee_per_unit_micros: MoneyMicros,
    pub notional_fee_bps: i64,
    pub spread_bps: i64,
    pub slippage_bps: i64,
    pub participation_bps: i64,
    pub partial_fill_policy: PartialFillPolicy,
    pub starting_cash_micros: MoneyMicros,
    pub maximum_order_quantity_micros: Quantity,
    pub maximum_position_notional_micros: MoneyMicros,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decision {
    NoSignal { bar_index: usize },
    OrderIntent(OrderIntent),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderIntent {
    pub decision_bar_index: usize,
    pub symbol: String,
    pub side: OrderSide,
    pub quantity: Quantity,
    /// A notional order resolves its quantity at execution, not at signal time.
    pub notional_micros: Option<MoneyMicros>,
    pub timing: ExecutionTiming,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatedOrder {
    pub intent: OrderIntent,
    pub execution_bar_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fill {
    pub order: SimulatedOrder,
    pub quantity: Quantity,
    pub price_micros: PriceMicros,
    pub fee_micros: MoneyMicros,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reject {
    InsufficientData { decision_bar_index: usize },
    Liquidity { decision_bar_index: usize },
    InsufficientCapital { decision_bar_index: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KernelResult {
    pub decisions: Vec<Decision>,
    pub orders: Vec<SimulatedOrder>,
    pub fills: Vec<Fill>,
    pub rejects: Vec<Reject>,
    pub ending_cash_micros: MoneyMicros,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Comparator {
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    Equal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignalField {
    Open,
    Close,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompiledStrategy {
    entry: SignalExpression,
    exit: Option<SignalExpression>,
    quantity: Quantity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SignalExpression {
    field: SignalField,
    comparator: Comparator,
    threshold_micros: PriceMicros,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelError {
    InvalidStrategySpec,
    UnsupportedSemantics(&'static str),
    InvalidExpression,
    InvalidQuantity,
    InvalidSimulationConfig,
    ArithmeticOverflow,
}

impl CompiledStrategy {
    pub fn compile_json(payload: &Value) -> Result<Self, KernelError> {
        let spec = StrategySpec::model_validate(payload.clone())
            .map_err(|_| KernelError::InvalidStrategySpec)?;
        Self::compile(&spec)
    }

    pub fn compile(spec: &StrategySpec) -> Result<Self, KernelError> {
        if spec
            .stages
            .evaluate
            .substeps
            .iter()
            .any(|step| step.config.contains_key(portfolio_program::CONFIG_KEY))
        {
            return Err(KernelError::UnsupportedSemantics(
                "portfolio strategy requires timestamped portfolio evaluator",
            ));
        }
        let stages = &spec.stages;
        let expected = [
            (&stages.inputs.substeps, "market_data.bars.read@1"),
            (&stages.universe.substeps, "universe.static.select@1"),
            (&stages.evaluate.substeps, "signal.rule.evaluate@1"),
            (&stages.intent.substeps, "intent.trade.build@1"),
            (&stages.sizing.substeps, "sizing.relative.apply@1"),
            (&stages.risk.substeps, "risk.strategy.guard@1"),
            (&stages.exit_policy.substeps, "exit.policy.evaluate@1"),
            (&stages.order_strategy.substeps, "order.intent.build@1"),
            (&stages.price_policy.substeps, "price.snapshot.select@1"),
        ];
        if expected
            .iter()
            .any(|(steps, operation)| steps.len() != 1 || steps[0].operation_ref != *operation)
            || !stages
                .order_strategy
                .policy
                .allowed_order_types
                .contains(&OrderType::Market)
            || stages.order_strategy.policy.conditional_orders_allowed
        {
            return Err(KernelError::UnsupportedSemantics("operation pipeline"));
        }

        let config = &stages.evaluate.substeps[0].config;
        let expression = config
            .get("tradeassembly.expression")
            .and_then(Value::as_str)
            .ok_or(KernelError::UnsupportedSemantics("evaluate expression"))?;
        let quantity = config
            .get("tradeassembly.quantity")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or(KernelError::InvalidQuantity)?;
        let entry = parse_expression(expression)?;
        let exit = stages.exit_policy.substeps[0]
            .config
            .get("tradeassembly.expression")
            .and_then(Value::as_str)
            .map(parse_expression)
            .transpose()?;
        Ok(Self {
            entry,
            exit,
            quantity,
        })
    }

    fn entry_signals(&self, observation: &MarketObservation) -> bool {
        self.entry.signals(observation)
    }

    fn exit_signals(&self, observation: &MarketObservation) -> bool {
        self.exit
            .is_some_and(|expression| expression.signals(observation))
    }
}

pub fn evaluate_observation(
    strategy: &CompiledStrategy,
    observation: &MarketObservation,
    current_position: Quantity,
    timing: ExecutionTiming,
) -> Decision {
    if current_position > 0 && strategy.exit_signals(observation) {
        return Decision::OrderIntent(OrderIntent {
            decision_bar_index: observation.bar_index,
            symbol: observation.symbol.clone(),
            side: OrderSide::Sell,
            quantity: current_position,
            notional_micros: None,
            timing,
        });
    }
    if current_position == 0 && strategy.entry_signals(observation) {
        return Decision::OrderIntent(OrderIntent {
            decision_bar_index: observation.bar_index,
            symbol: observation.symbol.clone(),
            side: OrderSide::Buy,
            quantity: strategy.quantity,
            notional_micros: None,
            timing,
        });
    }
    Decision::NoSignal {
        bar_index: observation.bar_index,
    }
}

impl SignalExpression {
    fn signals(self, observation: &MarketObservation) -> bool {
        let value = match self.field {
            SignalField::Open => observation.open,
            SignalField::Close => observation.close,
        };
        match self.comparator {
            Comparator::Greater => value > self.threshold_micros,
            Comparator::GreaterOrEqual => value >= self.threshold_micros,
            Comparator::Less => value < self.threshold_micros,
            Comparator::LessOrEqual => value <= self.threshold_micros,
            Comparator::Equal => value == self.threshold_micros,
        }
    }
}

pub fn run(
    _mode: KernelMode,
    strategy: &CompiledStrategy,
    observations: &[MarketObservation],
    config: SimulationConfig,
) -> Result<KernelResult, KernelError> {
    validate_config(strategy, config)?;
    let mut result = KernelResult {
        decisions: Vec::new(),
        orders: Vec::new(),
        fills: Vec::new(),
        rejects: Vec::new(),
        ending_cash_micros: config.starting_cash_micros,
    };
    let mut positions = BTreeMap::<String, Quantity>::new();
    for observation in observations {
        let position = positions.get(&observation.symbol).copied().unwrap_or(0);
        let Decision::OrderIntent(intent) =
            evaluate_observation(strategy, observation, position, config.timing)
        else {
            result.decisions.push(Decision::NoSignal {
                bar_index: observation.bar_index,
            });
            continue;
        };
        result.decisions.push(Decision::OrderIntent(intent.clone()));
        let Some(execution_bar_index) = execution_index(observation.bar_index, config.timing)
        else {
            result.rejects.push(Reject::InsufficientData {
                decision_bar_index: observation.bar_index,
            });
            continue;
        };
        let Some(execution_observation) = observations
            .iter()
            .find(|bar| bar.bar_index == execution_bar_index && bar.symbol == intent.symbol)
        else {
            result.rejects.push(Reject::InsufficientData {
                decision_bar_index: observation.bar_index,
            });
            continue;
        };
        let order = SimulatedOrder {
            intent,
            execution_bar_index,
        };
        result.orders.push(order.clone());
        let maximum = execution_observation
            .volume
            .checked_mul(config.participation_bps)
            .ok_or(KernelError::ArithmeticOverflow)?
            / BPS_DENOMINATOR;
        let quantity = if order.intent.quantity <= maximum {
            order.intent.quantity
        } else if config.partial_fill_policy == PartialFillPolicy::Allow && maximum > 0 {
            maximum
        } else {
            result.rejects.push(Reject::Liquidity {
                decision_bar_index: observation.bar_index,
            });
            continue;
        };
        let base = match config.timing {
            ExecutionTiming::SameBarOpen | ExecutionTiming::NextBarOpen => {
                execution_observation.open
            }
            ExecutionTiming::SameBarClose | ExecutionTiming::NextBarClose => {
                execution_observation.close
            }
        };
        let adverse_bps = config
            .spread_bps
            .checked_add(config.slippage_bps)
            .ok_or(KernelError::ArithmeticOverflow)?;
        let price_micros = match order.intent.side {
            OrderSide::Buy => apply_bps_up(base, adverse_bps)?,
            OrderSide::Sell => apply_bps_down(base, adverse_bps)?,
        };
        let gross = scaled_notional(quantity, price_micros)?;
        if quantity > config.maximum_order_quantity_micros
            || gross > config.maximum_position_notional_micros
        {
            result.rejects.push(Reject::InsufficientCapital {
                decision_bar_index: observation.bar_index,
            });
            continue;
        }
        let fee_micros = calculate_fee_micros(quantity, gross, config)?;
        let total = gross
            .checked_add(fee_micros)
            .ok_or(KernelError::ArithmeticOverflow)?;
        if order.intent.side == OrderSide::Buy && total > result.ending_cash_micros {
            result.rejects.push(Reject::InsufficientCapital {
                decision_bar_index: observation.bar_index,
            });
            continue;
        }
        match order.intent.side {
            OrderSide::Buy => {
                result.ending_cash_micros -= total;
                positions.insert(
                    observation.symbol.clone(),
                    position
                        .checked_add(quantity)
                        .ok_or(KernelError::ArithmeticOverflow)?,
                );
            }
            OrderSide::Sell => {
                result.ending_cash_micros = result
                    .ending_cash_micros
                    .checked_add(gross)
                    .and_then(|cash| cash.checked_sub(fee_micros))
                    .ok_or(KernelError::ArithmeticOverflow)?;
                positions.insert(
                    observation.symbol.clone(),
                    position
                        .checked_sub(quantity)
                        .ok_or(KernelError::ArithmeticOverflow)?,
                );
            }
        }
        result.fills.push(Fill {
            order,
            quantity,
            price_micros,
            fee_micros,
        });
    }
    Ok(result)
}

fn parse_expression(expression: &str) -> Result<SignalExpression, KernelError> {
    let words: Vec<_> = expression.split_whitespace().collect();
    if words.len() != 3 {
        return Err(KernelError::InvalidExpression);
    }
    let signal_field = match words[0] {
        "bar.open" => SignalField::Open,
        "bar.close" => SignalField::Close,
        _ => return Err(KernelError::InvalidExpression),
    };
    let comparator = match words[1] {
        ">" => Comparator::Greater,
        ">=" => Comparator::GreaterOrEqual,
        "<" => Comparator::Less,
        "<=" => Comparator::LessOrEqual,
        "==" => Comparator::Equal,
        _ => return Err(KernelError::InvalidExpression),
    };
    let threshold_micros = words[2]
        .parse()
        .map_err(|_| KernelError::InvalidExpression)?;
    Ok(SignalExpression {
        field: signal_field,
        comparator,
        threshold_micros,
    })
}

fn validate_config(
    strategy: &CompiledStrategy,
    config: SimulationConfig,
) -> Result<(), KernelError> {
    if config.fee_per_order_micros < 0
        || config.fee_per_unit_micros < 0
        || config.notional_fee_bps < 0
        || config.spread_bps < 0
        || config.slippage_bps < 0
        || !(0..=BPS_DENOMINATOR).contains(&config.participation_bps)
        || config.starting_cash_micros < 0
        || config.maximum_order_quantity_micros <= 0
        || config.maximum_position_notional_micros <= 0
    {
        return Err(KernelError::InvalidSimulationConfig);
    }
    if config.timing == ExecutionTiming::SameBarOpen
        && (strategy.entry.field != SignalField::Open
            || strategy
                .exit
                .is_some_and(|expression| expression.field != SignalField::Open))
    {
        return Err(KernelError::UnsupportedSemantics(
            "same-bar open requires a bar.open signal",
        ));
    }
    Ok(())
}

fn execution_index(index: usize, timing: ExecutionTiming) -> Option<usize> {
    match timing {
        ExecutionTiming::SameBarOpen | ExecutionTiming::SameBarClose => Some(index),
        ExecutionTiming::NextBarOpen | ExecutionTiming::NextBarClose => index.checked_add(1),
    }
}

fn apply_bps_up(price: PriceMicros, bps: i64) -> Result<PriceMicros, KernelError> {
    let numerator = price
        .checked_mul(
            BPS_DENOMINATOR
                .checked_add(bps)
                .ok_or(KernelError::ArithmeticOverflow)?,
        )
        .ok_or(KernelError::ArithmeticOverflow)?;
    numerator
        .checked_add(BPS_DENOMINATOR - 1)
        .map(|rounded| rounded / BPS_DENOMINATOR)
        .ok_or(KernelError::ArithmeticOverflow)
}

fn apply_bps_down(price: PriceMicros, bps: i64) -> Result<PriceMicros, KernelError> {
    let factor = BPS_DENOMINATOR
        .checked_sub(bps)
        .ok_or(KernelError::InvalidSimulationConfig)?;
    price
        .checked_mul(factor)
        .ok_or(KernelError::ArithmeticOverflow)
        .map(|value| value / BPS_DENOMINATOR)
}

pub fn calculate_fee_micros(
    quantity_micros: Quantity,
    gross_micros: MoneyMicros,
    config: SimulationConfig,
) -> Result<MoneyMicros, KernelError> {
    let units = i128::from(quantity_micros)
        .checked_mul(i128::from(config.fee_per_unit_micros))
        .ok_or(KernelError::ArithmeticOverflow)?
        / i128::from(QUANTITY_SCALE);
    let notional = i128::from(gross_micros)
        .checked_mul(i128::from(config.notional_fee_bps))
        .ok_or(KernelError::ArithmeticOverflow)?
        / i128::from(BPS_DENOMINATOR);
    i64::try_from(
        i128::from(config.fee_per_order_micros)
            .checked_add(units)
            .and_then(|value| value.checked_add(notional))
            .ok_or(KernelError::ArithmeticOverflow)?,
    )
    .map_err(|_| KernelError::ArithmeticOverflow)
}

fn scaled_notional(
    quantity_micros: Quantity,
    price_micros: PriceMicros,
) -> Result<i64, KernelError> {
    let numerator = i128::from(quantity_micros)
        .checked_mul(i128::from(price_micros))
        .ok_or(KernelError::ArithmeticOverflow)?;
    i64::try_from(numerator / i128::from(QUANTITY_SCALE))
        .map_err(|_| KernelError::ArithmeticOverflow)
}
