//! Explicit, versioned research compiler; the stateless execution path cannot consume it.
use super::portfolio_vwap::Policy;
use super::{KernelError, StrategySpec};
use crate::spec::{
    EarlyCloseBehavior, ExitComposition, ExitMonitoringMode, ExitScope, ExitTriggerKind,
    HolidayBehavior, OrderType, StageName,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONFIG_KEY: &str = "tradeassembly.portfolio_vwap";

pub enum ResearchProgram {
    Price(super::CompiledStrategy),
    Portfolio(PortfolioProgram, std::collections::BTreeSet<String>),
}

impl ResearchProgram {
    pub fn compile_json(payload: &Value) -> Result<Self, KernelError> {
        let spec = StrategySpec::model_validate(payload.clone())
            .map_err(|_| KernelError::InvalidStrategySpec)?;
        if spec
            .stages
            .evaluate
            .substeps
            .iter()
            .any(|step| step.config.contains_key(CONFIG_KEY))
        {
            Ok(Self::Portfolio(
                PortfolioProgram::compile(&spec)?,
                static_universe(&spec)?,
            ))
        } else {
            super::CompiledStrategy::compile(&spec).map(Self::Price)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PriceBasis {
    ProviderBarVwap,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FillContract {
    ExactNextMinuteOpenExpireRemainder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortfolioProgram {
    #[schemars(range(min = 1, max = 1))]
    pub version: u32,
    pub price_basis: PriceBasis,
    pub fills: FillContract,
    pub policy: Policy,
}

/// Capability discovery, not a strategy recommendation or an activation grant.
pub fn discovery() -> Value {
    serde_json::json!({
        "id":"portfolio-vwap-v1",
        "configPointer":"/stages/evaluate/substeps/0/config/tradeassembly.portfolio_vwap",
        "configSchema":schemars::schema_for!(PortfolioProgram),
        "backtestSupported":true,
        "statelessExecutionSupported":false,
        "constraints": {
            "universe":"One signal-market requirement with explicit normalized-instrument selectors, equity/fund assets, and timeframes [1m].",
            "pipeline":"Exactly one enabled, nonoptional substep per stage; no additional stage/substep configs, expressions, or state. The evaluate substep contains only the portfolio configuration.",
            "operations":{
                "inputs":"market_data.bars.read@1", "universe":"universe.static.select@1",
                "evaluate":"signal.rule.evaluate@1", "intent":"intent.trade.build@1",
                "sizing":"sizing.relative.apply@1", "risk":"risk.strategy.guard@1",
                "exit_policy":"exit.policy.evaluate@1", "order_strategy":"order.intent.build@1",
                "price_policy":"price.snapshot.select@1", "market_clock":"calendar.session.resolve@1"
            },
            "exitPolicy": {
                "monitoring":["synthetic"], "composition":"any", "precedence":[],
                "triggers":[
                    {"id":"vwap_proximity","kind":"indicator","scope":"full_position"},
                    {"id":"maximum_holding","kind":"time","scope":"full_position"},
                    {"id":"maximum_loss","kind":"profit_loss","scope":"full_position"}
                ]
            },
            "clockPolicy":{
                "timezone":"America/New_York", "calendar_refs":["XNYS"],
                "windows":[{"start":"09:30","end":"16:00"}],
                "days":["monday","tuesday","wednesday","thursday","friday"],
                "holiday_behavior":"follow_calendar", "early_close_behavior":"follow_calendar"
            },
            "data":"Immutable one-minute bars with provider VWAP and calendar session evidence; extended-hours bars are excluded. Missing evidence is rejected.",
            "backtest":"Cash account, no leverage, bar-close evaluation, next-bar open fills. Maximum open positions must equal the owner policy. No variable overrides. End-of-data handling must match the strategy. Portfolio-wide loss caps below starting cash remain unsupported.",
            "units":"Money/prices/quantities use millionths; durations use milliseconds; ratios use basis points. Volume window is preceding completed bars, excluding the current bar.",
            "semantics":"Stable alphabetical symbol tie-break; one exact-next-minute fill opportunity; partial remainder expires. Holding-time and loss exits use strictly exceeded thresholds; other thresholds are inclusive. Entry cooldown begins at the fill."
        },
        "notice":"All strategy and risk values must come from the owner. Compilation support is not dataset readiness, broker authorization, publication, promotion, or activation."
    })
}

pub fn compilation_report(payload: &Value, schema_valid: bool) -> Value {
    let backtest = ResearchProgram::compile_json(payload);
    let deterministic = super::CompiledStrategy::compile_json(payload);
    let describe = |result: Result<(), KernelError>| match result {
        Ok(()) if schema_valid => serde_json::json!({"supported":true}),
        Ok(()) => serde_json::json!({"supported":false,"reason":"strategy_schema_invalid"}),
        Err(error) => serde_json::json!({"supported":false,"reason":format!("{error:?}")}),
    };
    serde_json::json!({
        "backtest":describe(backtest.map(|_| ())),
        "statelessExecution":describe(deterministic.map(|_| ())),
        "authorizationGranted":false
    })
}

impl PortfolioProgram {
    pub fn compile_json(payload: &Value) -> Result<Self, KernelError> {
        let spec = StrategySpec::model_validate(payload.clone())
            .map_err(|_| KernelError::InvalidStrategySpec)?;
        Self::compile(&spec)
    }

    pub fn compile(spec: &StrategySpec) -> Result<Self, KernelError> {
        static_universe(spec)?;
        let unsupported = || KernelError::UnsupportedSemantics("portfolio operation pipeline");
        for (name, operation) in [
            (StageName::Inputs, "market_data.bars.read@1"),
            (StageName::Universe, "universe.static.select@1"),
            (StageName::Evaluate, "signal.rule.evaluate@1"),
            (StageName::Intent, "intent.trade.build@1"),
            (StageName::Sizing, "sizing.relative.apply@1"),
            (StageName::Risk, "risk.strategy.guard@1"),
            (StageName::ExitPolicy, "exit.policy.evaluate@1"),
            (StageName::OrderStrategy, "order.intent.build@1"),
            (StageName::PricePolicy, "price.snapshot.select@1"),
            (StageName::MarketClock, "calendar.session.resolve@1"),
        ] {
            let stage = spec.stages.view(&name);
            if !stage.enabled || !stage.config.is_empty() || stage.substeps.len() != 1 {
                return Err(unsupported());
            }
            let step = &stage.substeps[0];
            if step.operation_ref != operation
                || step.optional
                || step.expression.is_some()
                || step.state.is_some()
            {
                return Err(unsupported());
            }
            // No silently ignored expressions, sizing overrides, or extra risk rules.
            if name == StageName::Evaluate {
                if step.config.len() != 1 || !step.config.contains_key(CONFIG_KEY) {
                    return Err(unsupported());
                }
            } else if !step.config.is_empty() {
                return Err(unsupported());
            }
        }
        let stages = &spec.stages;
        if stages.order_strategy.policy.allowed_order_types != [OrderType::Market]
            || stages.order_strategy.policy.conditional_orders_allowed
            || stages.price_policy.policy.expression.is_some()
        {
            return Err(unsupported());
        }
        let exits = &stages.exit_policy.policy;
        let expected_exits = [
            ("vwap_proximity", ExitTriggerKind::Indicator),
            ("maximum_holding", ExitTriggerKind::Time),
            ("maximum_loss", ExitTriggerKind::ProfitLoss),
        ];
        if exits.composition != ExitComposition::Any
            || !exits.precedence.is_empty()
            || exits.monitoring != [ExitMonitoringMode::Synthetic]
            || exits.triggers.len() != expected_exits.len()
            || exits
                .triggers
                .iter()
                .zip(expected_exits)
                .any(|(trigger, (id, kind))| {
                    trigger.id != id
                        || trigger.kind != kind
                        || trigger.scope != ExitScope::FullPosition
                        || trigger.expression.is_some()
                        || trigger.capability_requirement_ref.is_some()
                })
        {
            return Err(KernelError::UnsupportedSemantics("portfolio exit policy"));
        }
        let clock = &stages.market_clock.policy;
        if clock.timezone != "America/New_York"
            || clock.calendar_refs != ["XNYS"]
            || clock.holiday_behavior != HolidayBehavior::FollowCalendar
            || clock.early_close_behavior != EarlyCloseBehavior::FollowCalendar
            || clock.eligibility_expression.is_some()
            || clock.windows.len() != 1
            || clock.windows[0].start != "09:30"
            || clock.windows[0].end != "16:00"
            || clock
                .days
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
                != ["monday", "tuesday", "wednesday", "thursday", "friday"]
                    .into_iter()
                    .collect()
        {
            return Err(KernelError::UnsupportedSemantics(
                "portfolio regular-session calendar",
            ));
        }
        let program: Self =
            serde_json::from_value(stages.evaluate.substeps[0].config[CONFIG_KEY].clone())
                .map_err(|_| KernelError::UnsupportedSemantics("portfolio policy contract"))?;
        program.validate()?;
        Ok(program)
    }

    pub fn validate(&self) -> Result<(), KernelError> {
        if self.version != 1
            || self.policy.entry_notional_micros <= 0
            || self.policy.maximum_positions == 0
            || self.policy.maximum_exposure_micros <= 0
            || self.policy.entry_notional_micros > self.policy.maximum_exposure_micros
            || !(1..=10_000).contains(&self.policy.volume_window)
            || self.policy.rules.validate().is_err()
        {
            return Err(KernelError::UnsupportedSemantics("portfolio policy values"));
        }
        Ok(())
    }
}

fn static_universe(spec: &StrategySpec) -> Result<std::collections::BTreeSet<String>, KernelError> {
    let error = || KernelError::UnsupportedSemantics("portfolio static one-minute universe");
    if spec.signal_market_requirements.len() != 1 {
        return Err(error());
    }
    let requirement = &spec.signal_market_requirements[0];
    if requirement.timeframes != ["1m"]
        || !matches!(
            requirement.asset_class,
            crate::spec::AssetClass::Equity | crate::spec::AssetClass::Fund
        )
    {
        return Err(error());
    }
    let mut instruments = std::collections::BTreeSet::new();
    for selector in &requirement.selectors {
        let symbol = selector
            .normalized_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(error)?;
        if selector.selector_type != crate::spec::SelectorType::NormalizedInstrument
            || selector.alias.is_some()
            || selector.expression.is_some()
            || !instruments.insert(symbol.to_string())
        {
            return Err(error());
        }
    }
    if instruments.is_empty() {
        return Err(error());
    }
    Ok(instruments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn spec() -> Value {
        let mut spec: Value = serde_json::from_str(include_str!(
            "../../../examples/strategy-spec/v3/valid/static-equity.json"
        ))
        .unwrap();
        spec["stages"]["evaluate"]["substeps"][0]["config"] = json!({CONFIG_KEY: {
            "version":1, "price_basis":"provider_bar_vwap", "fills":"exact_next_minute_open_expire_remainder",
            "policy": {"volume_window":20,"entry_notional_micros":100_000_000,"maximum_positions":4,"maximum_exposure_micros":500_000_000,
                "rules":{"entry_discount_bps":50,"minimum_volume_ratio_bps":12000,"exit_proximity_bps":10,
                    "maximum_holding_ms":900000,"maximum_loss_bps":50,"entry_cooldown_ms":600000}}
        }});
        spec["stages"]["exit_policy"]["policy"]["monitoring"] = json!(["synthetic"]);
        spec["stages"]["exit_policy"]["policy"]["triggers"] = json!([
            {"id":"vwap_proximity","kind":"indicator","scope":"full_position"},
            {"id":"maximum_holding","kind":"time","scope":"full_position"},
            {"id":"maximum_loss","kind":"profit_loss","scope":"full_position"}
        ]);
        spec["stages"]["order_strategy"]["policy"]["allowed_order_types"] = json!(["market"]);
        spec["signal_market_requirements"][0]["timeframes"] = json!(["1m"]);
        spec
    }

    #[test]
    fn explicit_policy_roundtrips_and_stateless_compiler_refuses_it() {
        let payload = spec();
        let validation = crate::spec::validate_strategy_spec_report(&payload);
        assert!(validation.valid, "{:?}", validation.diagnostics);
        let compiled = PortfolioProgram::compile_json(&payload).unwrap();
        assert_eq!(compiled.policy.maximum_positions, 4);
        assert_eq!(compiled.policy.volume_window, 20);
        assert_eq!(
            serde_json::to_value(compiled).unwrap(),
            payload["stages"]["evaluate"]["substeps"][0]["config"][CONFIG_KEY]
        );
        assert!(super::super::CompiledStrategy::compile_json(&payload).is_err());
    }

    #[test]
    fn missing_unknown_and_unsupported_contract_fields_fail_closed() {
        for (field, value) in [
            ("version", json!(2)),
            ("price_basis", json!("close")),
            ("fills", json!("next_available")),
            ("unexpected", json!(true)),
            ("policy", Value::Null),
        ] {
            let mut payload = spec();
            payload["stages"]["evaluate"]["substeps"][0]["config"][CONFIG_KEY][field] = value;
            assert!(PortfolioProgram::compile_json(&payload).is_err(), "{field}");
        }
        let mut payload = spec();
        payload["stages"]["evaluate"]["substeps"][0]["config"][CONFIG_KEY]["policy"]
            .as_object_mut()
            .unwrap()
            .remove("rules");
        assert!(PortfolioProgram::compile_json(&payload).is_err());
    }

    #[test]
    fn mixed_rules_disabled_risk_and_hidden_exit_triggers_are_not_ignored() {
        let mut mixed = spec();
        mixed["stages"]["evaluate"]["substeps"][0]["config"]["tradeassembly.expression"] =
            json!("bar.close > 1");
        mixed["stages"]["evaluate"]["substeps"][0]["config"]["tradeassembly.quantity"] = json!(1);
        assert!(PortfolioProgram::compile_json(&mixed).is_err());
        assert!(super::super::CompiledStrategy::compile_json(&mixed).is_err());
        let mut disabled = spec();
        disabled["stages"]["risk"]["enabled"] = json!(false);
        assert!(PortfolioProgram::compile_json(&disabled).is_err());
        let mut extra_exit = spec();
        extra_exit["stages"]["exit_policy"]["policy"]["triggers"] =
            json!([{"id":"end","kind":"lifecycle","scope":"full_position"}]);
        assert!(PortfolioProgram::compile_json(&extra_exit).is_err());
    }
}
