// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub last: Option<f64>,
    pub close: Option<f64>,
    pub price: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionContract {
    pub underlying: String,
    pub expiry: String,
    pub strike: Value,
    pub right: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Leg {
    pub symbol: Option<String>,
    pub contract: Option<OptionContract>,
    pub qty: Option<f64>,
    pub target_notional: Option<f64>,
    pub limit_price: Option<f64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub quotes: BTreeMap<String, Quote>,
    pub option_quotes: BTreeMap<String, Quote>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PricePolicyResult {
    pub legs: Vec<Leg>,
    pub order_type: String,
}

pub fn apply_auto_limit_policy(
    legs: &[Leg],
    snapshot: &MarketSnapshot,
    source: &str,
    round_to: Option<f64>,
) -> PricePolicyResult {
    let mut out = legs.to_vec();
    let source = source.trim().to_ascii_lowercase();
    for leg in &mut out {
        let Some(symbol) = leg
            .symbol
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(quote) = snapshot.quotes.get(symbol) else {
            continue;
        };
        if let Some(price) = quote_price(quote, source.as_str()) {
            leg.limit_price = Some(round_step(price, round_to));
        }
    }
    PricePolicyResult {
        legs: out,
        order_type: "LMT".to_string(),
    }
}

pub fn apply_option_mid_policy(
    legs: &[Leg],
    snapshot: &MarketSnapshot,
    round_to: Option<f64>,
) -> PricePolicyResult {
    let mut out = legs.to_vec();
    for leg in &mut out {
        let Some(contract) = &leg.contract else {
            continue;
        };
        let Some(quote) = snapshot.option_quotes.get(&contract_key(contract)) else {
            continue;
        };
        if let (Some(bid), Some(ask)) = (quote.bid, quote.ask) {
            leg.limit_price = Some(round_step((bid + ask) / 2.0, round_to));
        }
    }
    PricePolicyResult {
        legs: out,
        order_type: "LMT".to_string(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllocationOutcome {
    pub legs: Vec<Leg>,
    pub sizing: BTreeMap<String, Value>,
    pub metadata: BTreeMap<String, Value>,
}

pub fn apply_builtin_allocation_policy(
    legs: &[Leg],
    plugin_ref: &str,
    plugin_config: &BTreeMap<String, Value>,
    sizing: Option<&BTreeMap<String, Value>>,
) -> Result<AllocationOutcome, String> {
    let normalized_ref = plugin_ref.trim().to_ascii_lowercase();
    if !matches!(
        normalized_ref.as_str(),
        "plugin-allocation" | "builtin:plugin-allocation" | "builtin:plugin-allocation@1"
    ) {
        return Err(format!(
            "allocation manifest '{plugin_ref}' not found or disabled"
        ));
    }

    let objective = plugin_config
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("risk_parity")
        .trim()
        .to_ascii_lowercase();
    let mut metadata = BTreeMap::new();
    metadata.insert(
        "allocation_plugin_ref".to_string(),
        Value::String(plugin_ref.to_string()),
    );
    metadata.insert(
        "allocation_policy_params".to_string(),
        Value::Object(plugin_config.clone().into_iter().collect()),
    );

    if objective == "momentum" {
        let window = plugin_config
            .get("rebalance_window")
            .and_then(number_value)
            .unwrap_or(10.0)
            .max(1.0);
        let base_qty = (window as i64 / 4).max(1) as f64;
        let mut out = Vec::new();
        for (index, leg) in legs.iter().enumerate() {
            let mut next = leg.clone();
            next.qty = Some((base_qty - index as f64).max(1.0));
            out.push(next);
        }
        metadata.insert(
            "allocation_policy_output_mode".to_string(),
            Value::String("concrete_sizing".to_string()),
        );
        return Ok(AllocationOutcome {
            legs: out,
            sizing: sizing.cloned().unwrap_or_default(),
            metadata,
        });
    }

    let total_exposure = sizing
        .and_then(|map| map.get("total_exposure_value"))
        .and_then(number_value);
    let Some(total_exposure) = total_exposure else {
        return Err(
            "allocation plugin requires total exposure before relative allocations can be applied"
                .to_string(),
        );
    };
    let symbols = legs.iter().filter_map(leg_symbol).collect::<Vec<_>>();
    if symbols.is_empty() {
        metadata.insert(
            "allocation_policy_output_mode".to_string(),
            Value::String("relative_allocations".to_string()),
        );
        return Ok(AllocationOutcome {
            legs: legs.to_vec(),
            sizing: sizing.cloned().unwrap_or_default(),
            metadata,
        });
    }
    let weight = 1.0 / symbols.len() as f64;
    let mut out = legs.to_vec();
    for leg in &mut out {
        if leg_symbol(leg).is_some() {
            leg.target_notional = Some(total_exposure * weight);
        }
    }
    metadata.insert(
        "allocation_policy_output_mode".to_string(),
        Value::String("relative_allocations".to_string()),
    );
    let mut next_sizing = sizing.cloned().unwrap_or_default();
    next_sizing.insert("override".to_string(), Value::Bool(false));
    Ok(AllocationOutcome {
        legs: out,
        sizing: next_sizing,
        metadata,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RiskLimits {
    pub max_notional: f64,
    pub max_order_quantity: f64,
    pub max_daily_loss: f64,
    pub max_concurrent_positions: usize,
    pub max_buying_power_pct: Option<f64>,
    pub max_capital_at_risk_pct: Option<f64>,
    pub max_margin_used_pct: Option<f64>,
    pub max_delta_abs: Option<f64>,
    pub attribution_scope: AttributionScope,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RiskAccountState {
    pub account_equity: f64,
    pub buying_power: f64,
    pub daily_realized_pnl: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllocationTarget {
    pub symbol: String,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AllocationPolicy {
    None,
    TargetAllocations(Vec<AllocationTarget>),
    Plugin {
        plugin_ref: String,
        plugin_config: BTreeMap<String, Value>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuantityResolution {
    pub quantity: f64,
    pub evidence: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderPlan {
    pub symbol: String,
    pub side: String,
    pub quantity: f64,
    pub limit_price: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RiskDenied {
    pub reason: String,
    pub notional: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrokerCapabilities {
    pub provider_ref: String,
    pub modes: Vec<String>,
    pub constraints: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RiskReservation {
    pub id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub notional: f64,
    pub delta_exposure: f64,
    pub status: String,
    pub provider_ref: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PositionRecord {
    pub id: String,
    pub strategy_id: String,
    pub symbol: String,
    pub quantity: f64,
    pub average_price: f64,
    pub notional: f64,
    pub status: String,
    pub provider_ref: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AttributionScope {
    #[default]
    Strict,
    InclusiveByUnderlying,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterExposureScopeInput<'a> {
    pub reservations: &'a [RiskReservation],
    pub positions: &'a [PositionRecord],
    pub strategy_id: &'a str,
    pub symbol: &'a str,
    pub attribution_scope: AttributionScope,
    pub account_id: Option<&'a str>,
    pub provider_ref: Option<&'a str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExposureScopeResult {
    pub reservations: Vec<RiskReservation>,
    pub positions: Vec<PositionRecord>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstraintValidation {
    pub allowed: bool,
    pub reason: Option<String>,
    pub checks: Vec<BTreeMap<String, Value>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reservation {
    pub key: String,
    pub created_at: f64,
    pub ttl_seconds: i64,
    pub order_id: Option<String>,
    pub reservation_id: Option<String>,
    pub status: String,
    pub run_id: Option<String>,
    pub strategy_id: Option<String>,
}

impl Reservation {
    pub fn expired_at(&self, now: f64) -> bool {
        self.ttl_seconds > 0 && (now - self.created_at) > self.ttl_seconds as f64
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExposureLedger {
    ttl_seconds: i64,
    reservations: BTreeMap<String, Reservation>,
    clock_override: Option<f64>,
}

impl ExposureLedger {
    pub fn new(ttl_seconds: i64) -> Self {
        Self {
            ttl_seconds,
            reservations: BTreeMap::new(),
            clock_override: None,
        }
    }

    pub fn new_with_clock(ttl_seconds: i64, now: f64) -> Self {
        Self {
            ttl_seconds,
            reservations: BTreeMap::new(),
            clock_override: Some(now),
        }
    }

    pub fn set_clock(&mut self, now: f64) {
        self.clock_override = Some(now);
    }

    pub fn reserve(
        &mut self,
        key: &str,
        strategy_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Reservation {
        self.gc();
        if let Some(existing) = self.reservations.get(key) {
            if !existing.expired_at(self.now()) {
                return existing.clone();
            }
        }
        let reservation = Reservation {
            key: key.to_string(),
            created_at: self.now(),
            ttl_seconds: self.ttl_seconds,
            order_id: None,
            reservation_id: None,
            status: "ACTIVE".to_string(),
            run_id: run_id.map(str::to_string),
            strategy_id: strategy_id.map(str::to_string),
        };
        self.reservations
            .insert(key.to_string(), reservation.clone());
        reservation
    }

    pub fn link_order(&mut self, key: &str, order_id: &str) {
        if let Some(reservation) = self.reservations.get_mut(key) {
            reservation.order_id = Some(order_id.to_string());
        }
    }

    pub fn get(&mut self, key: &str) -> Option<&Reservation> {
        self.gc();
        self.reservations.get(key)
    }

    fn gc(&mut self) {
        let now = self.now();
        self.reservations
            .retain(|_, reservation| !reservation.expired_at(now));
    }

    fn now(&self) -> f64 {
        self.clock_override.unwrap_or_else(current_unix_seconds)
    }
}

pub fn resolve_order_quantity(
    symbol: &str,
    allocation: &AllocationPolicy,
    risk_limits: &RiskLimits,
    account: &RiskAccountState,
    rows: &[BTreeMap<String, Value>],
    default_quantity: f64,
) -> QuantityResolution {
    let default = default_quantity.min(risk_limits.max_order_quantity);
    match allocation {
        AllocationPolicy::TargetAllocations(targets) => {
            resolve_target_allocation_quantity(TargetAllocationInput {
                symbol,
                targets,
                risk_limits,
                account,
                rows,
                default_quantity: default,
                exposure_cap_override: None,
                mode: "target_allocations",
            })
        }
        AllocationPolicy::Plugin {
            plugin_ref,
            plugin_config,
        } => resolve_plugin_quantity(
            symbol,
            plugin_ref,
            plugin_config,
            risk_limits,
            account,
            rows,
            default,
        ),
        AllocationPolicy::None => QuantityResolution {
            quantity: default,
            evidence: evidence_map([
                ("mode", json!("none")),
                ("applied", json!(false)),
                ("quantity", json!(default)),
            ]),
        },
    }
}

pub fn estimate_order_notional(
    order_plan: &OrderPlan,
    rows: &[BTreeMap<String, Value>],
) -> Result<f64, RiskDenied> {
    let Some(price) = risk_latest_price(rows) else {
        return Err(RiskDenied {
            reason: "risk_denied: cannot estimate order notional without a local price".to_string(),
            notional: None,
        });
    };
    Ok(order_plan.quantity.abs() * price * order_multiplier(&order_plan.symbol))
}

pub fn check_order_risk(
    order_plan: &OrderPlan,
    limits: &RiskLimits,
    rows: &[BTreeMap<String, Value>],
    active_reservations: &[RiskReservation],
    account: Option<&RiskAccountState>,
    active_positions: &[PositionRecord],
) -> Result<f64, RiskDenied> {
    let quantity = order_plan.quantity.abs();
    if quantity > limits.max_order_quantity {
        return Err(RiskDenied {
            reason: format!(
                "risk_denied: quantity {} exceeds max_order_quantity {}",
                format_number(quantity),
                format_number(limits.max_order_quantity)
            ),
            notional: None,
        });
    }

    let notional = estimate_order_notional(order_plan, rows)?;
    if notional > limits.max_notional {
        return Err(RiskDenied {
            reason: format!(
                "risk_denied: notional {:.2} exceeds max_notional {:.2}",
                notional, limits.max_notional
            ),
            notional: Some(notional),
        });
    }

    if let Some(account) = account {
        let daily_loss = (-account.daily_realized_pnl).max(0.0);
        if daily_loss >= limits.max_daily_loss {
            return Err(RiskDenied {
                reason: format!(
                    "risk_denied: daily_loss {:.2} reached max_daily_loss {:.2}",
                    daily_loss, limits.max_daily_loss
                ),
                notional: Some(notional),
            });
        }
        if notional > account.buying_power {
            return Err(RiskDenied {
                reason: format!(
                    "risk_denied: notional {:.2} exceeds buying_power {:.2}",
                    notional, account.buying_power
                ),
                notional: Some(notional),
            });
        }
    }

    let reserved_notional = active_reservations
        .iter()
        .map(|item| item.notional)
        .sum::<f64>();
    let aggregate = reserved_notional + notional;

    if let Some(account) = account {
        if let Some(max_buying_power_pct) = limits.max_buying_power_pct {
            let starting_buying_power = account.buying_power + reserved_notional;
            let buying_power_limit = (max_buying_power_pct / 100.0) * starting_buying_power;
            if aggregate > buying_power_limit {
                return Err(RiskDenied {
                    reason: format!(
                        "risk_denied: aggregate exposure {:.2} exceeds {}% of buying_power {:.2}",
                        aggregate,
                        format_number(max_buying_power_pct),
                        starting_buying_power
                    ),
                    notional: Some(notional),
                });
            }
        }
        if let Some(max_capital_at_risk_pct) = limits.max_capital_at_risk_pct {
            let equity_limit = (max_capital_at_risk_pct / 100.0) * account.account_equity;
            if aggregate > equity_limit {
                return Err(RiskDenied {
                    reason: format!(
                        "risk_denied: aggregate exposure {:.2} exceeds {}% of account_equity {:.2}",
                        aggregate,
                        format_number(max_capital_at_risk_pct),
                        account.account_equity
                    ),
                    notional: Some(notional),
                });
            }
        }
        if let Some(max_margin_used_pct) = limits.max_margin_used_pct {
            let margin_limit = (max_margin_used_pct / 100.0) * account.account_equity;
            let position_notional = active_positions
                .iter()
                .filter(|item| item.status.eq_ignore_ascii_case("active"))
                .map(|item| item.notional)
                .sum::<f64>();
            let margin_used = aggregate + position_notional;
            if margin_used > margin_limit {
                return Err(RiskDenied {
                    reason: format!(
                        "risk_denied: margin_used {:.2} exceeds {}% of account_equity {:.2}",
                        margin_used,
                        format_number(max_margin_used_pct),
                        account.account_equity
                    ),
                    notional: Some(notional),
                });
            }
        }
    }

    if let Some(max_delta_abs) = limits.max_delta_abs {
        let order_delta = estimate_order_delta_exposure(order_plan, rows);
        let active_delta = active_reservations
            .iter()
            .map(estimate_reservation_delta_exposure)
            .sum::<f64>();
        let position_delta = active_positions
            .iter()
            .filter(|item| item.status.eq_ignore_ascii_case("active"))
            .map(estimate_position_delta_exposure)
            .sum::<f64>();
        let aggregate_delta = active_delta + position_delta + order_delta;
        if aggregate_delta.abs() > max_delta_abs {
            return Err(RiskDenied {
                reason: format!(
                    "risk_denied: abs_delta {:.4} exceeds max_delta_abs {}",
                    aggregate_delta.abs(),
                    format_number(max_delta_abs)
                ),
                notional: Some(notional),
            });
        }
    }

    if aggregate > limits.max_notional {
        return Err(RiskDenied {
            reason: format!(
                "risk_denied: aggregate exposure {:.2} exceeds max_notional {:.2}",
                aggregate, limits.max_notional
            ),
            notional: Some(notional),
        });
    }

    let active_position_count = active_positions
        .iter()
        .filter(|item| item.status.eq_ignore_ascii_case("active"))
        .map(|item| item.id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if active_reservations.len() + active_position_count >= limits.max_concurrent_positions {
        return Err(RiskDenied {
            reason: format!(
                "risk_denied: max_concurrent_positions {} reached",
                limits.max_concurrent_positions
            ),
            notional: Some(notional),
        });
    }

    Ok(notional)
}

pub fn estimate_order_delta_exposure(
    order_plan: &OrderPlan,
    rows: &[BTreeMap<String, Value>],
) -> f64 {
    let parsed = parse_option_symbol(&order_plan.symbol);
    let unit_delta = delta_from_rows(rows).unwrap_or_else(|| match parsed.as_ref() {
        Some(parsed) if parsed.right == "CALL" => 0.5,
        Some(_) => -0.5,
        None => 1.0,
    });
    let signed_quantity = if order_plan.side == "buy" {
        order_plan.quantity
    } else {
        -order_plan.quantity
    };
    let multiplier = if parsed.is_some() { 100.0 } else { 1.0 };
    round8(signed_quantity * unit_delta * multiplier)
}

pub fn estimate_position_delta_exposure(position: &PositionRecord) -> f64 {
    let parsed = parse_option_symbol(&position.symbol);
    let (unit_delta, multiplier) = match parsed {
        Some(parsed) if parsed.right == "CALL" => (0.5, 100.0),
        Some(_) => (-0.5, 100.0),
        None => (1.0, 1.0),
    };
    round8(position.quantity * unit_delta * multiplier)
}

pub fn estimate_reservation_delta_exposure(reservation: &RiskReservation) -> f64 {
    reservation.delta_exposure
}

pub fn validate_broker_order_constraints(
    order_plan: &OrderPlan,
    broker_capabilities: &BrokerCapabilities,
    market_snapshot: Option<&BTreeMap<String, Value>>,
) -> ConstraintValidation {
    if broker_capabilities.constraints.is_empty() {
        return ConstraintValidation {
            allowed: true,
            reason: None,
            checks: Vec::new(),
        };
    }

    let instrument_kind = instrument_kind_for_order(order_plan);
    let mut effective =
        constraint_for_instrument(&broker_capabilities.constraints, instrument_kind)
            .cloned()
            .unwrap_or_default();
    if let Some(symbol_constraint) =
        symbol_constraint(&broker_capabilities.constraints, &order_plan.symbol)
    {
        for (key, value) in symbol_constraint {
            effective.insert(key.clone(), value.clone());
        }
    }
    if effective.is_empty() {
        return ConstraintValidation {
            allowed: true,
            reason: None,
            checks: Vec::new(),
        };
    }

    let quantity = order_plan.quantity.abs();
    let mut checks = Vec::new();
    if let Some(min_qty) = effective.get("min_qty").and_then(number_value) {
        checks.push(evidence_map([
            ("constraint", json!("min_qty")),
            ("actual", json!(quantity)),
            ("required", json!(min_qty)),
            ("instrument", json!(instrument_kind)),
            ("symbol", json!(order_plan.symbol)),
        ]));
        if quantity < min_qty {
            return ConstraintValidation {
                allowed: false,
                reason: Some(format!(
                    "validation_denied: qty {} below min_qty {} for {} {}",
                    format_number(quantity),
                    format_number(min_qty),
                    instrument_kind,
                    order_plan.symbol
                )),
                checks,
            };
        }
    }

    if let Some(min_notional) = effective.get("min_notional").and_then(number_value) {
        let price = order_constraint_price(order_plan, market_snapshot);
        let notional = price.map(|price| quantity * price * order_multiplier(&order_plan.symbol));
        checks.push(evidence_map([
            ("constraint", json!("min_notional")),
            (
                "actual",
                notional
                    .map(round8)
                    .map_or(Value::Null, |value| json!(value)),
            ),
            ("required", json!(min_notional)),
            ("instrument", json!(instrument_kind)),
            ("symbol", json!(order_plan.symbol)),
            ("price", price.map_or(Value::Null, |value| json!(value))),
        ]));
        if let Some(notional) = notional {
            if notional < min_notional {
                return ConstraintValidation {
                    allowed: false,
                    reason: Some(format!(
                        "validation_denied: notional {:.2} below min_notional {} for {} {}",
                        notional,
                        format_number(min_notional),
                        instrument_kind,
                        order_plan.symbol
                    )),
                    checks,
                };
            }
        }
    }

    ConstraintValidation {
        allowed: true,
        reason: None,
        checks,
    }
}

pub fn estimate_option_strategy_margin(order_plans: &[OrderPlan]) -> BTreeMap<String, Value> {
    let option_legs = order_plans
        .iter()
        .filter_map(option_leg)
        .collect::<Vec<_>>();
    if option_legs.is_empty() {
        return evidence_map([
            ("margin_model", json!("not_applicable")),
            ("buying_power_effect", json!(0.0)),
        ]);
    }
    if option_legs.len() != order_plans.len() {
        return evidence_map([
            ("margin_model", json!("mixed_or_unsupported")),
            ("buying_power_effect", Value::Null),
        ]);
    }
    if option_legs.len() != 2 {
        let gross = order_plans
            .iter()
            .filter(|plan| plan.side == "buy")
            .map(|plan| signed_premium(plan).max(0.0) * 100.0 * plan.quantity)
            .sum::<f64>();
        return evidence_map([
            ("margin_model", json!("unsupported_option_strategy")),
            ("buying_power_effect", json!(round2(gross))),
        ]);
    }

    let first = &option_legs[0];
    let second = &option_legs[1];
    let same_series = first.underlying == second.underlying
        && first.expiry == second.expiry
        && first.right == second.right;
    let sides_match = (first.side == "buy" && second.side == "sell")
        || (first.side == "sell" && second.side == "buy");
    let quantities_match = first.quantity == second.quantity;
    if !same_series || !sides_match || !quantities_match {
        return evidence_map([
            ("margin_model", json!("unsupported_option_strategy")),
            ("buying_power_effect", Value::Null),
        ]);
    }

    let spread_width = (first.strike - second.strike).abs();
    let quantity = first.quantity;
    let net_credit = round8(order_plans.iter().map(signed_premium).sum::<f64>());
    let buying_power_effect = if net_credit >= 0.0 {
        ((spread_width - net_credit) * 100.0 * quantity).max(0.0)
    } else {
        net_credit.abs() * 100.0 * quantity
    };
    evidence_map([
        ("margin_model", json!("defined_risk_option_spread")),
        ("underlying", json!(first.underlying)),
        ("expiry", json!(first.expiry)),
        ("right", json!(first.right)),
        ("spread_width", json!(round4(spread_width))),
        ("net_credit", json!(round4(net_credit))),
        ("buying_power_effect", json!(round2(buying_power_effect))),
        ("quantity", json!(quantity)),
    ])
}

pub fn normalize_broker_margin_payload(
    payload: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let buying_power_effect = first_number(
        payload,
        &[
            "buying_power_effect",
            "buyingPowerEffect",
            "initial_margin",
            "initialMargin",
            "margin",
        ],
    )
    .ok_or_else(|| {
        "broker margin payload requires buying_power_effect or initial_margin".to_string()
    })?;
    let initial_margin = first_number(payload, &["initial_margin", "initialMargin", "margin"])
        .unwrap_or(buying_power_effect);
    let maintenance_margin = first_number(payload, &["maintenance_margin", "maintenanceMargin"]);
    let currency = payload
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();
    let raw_keys = payload.keys().map(|key| json!(key)).collect::<Vec<_>>();

    Ok(evidence_map([
        ("margin_model", json!("broker_margin_preview")),
        ("buying_power_effect", json!(round2(buying_power_effect))),
        ("initial_margin", json!(round2(initial_margin))),
        (
            "maintenance_margin",
            maintenance_margin.map_or(Value::Null, |value| json!(round2(value))),
        ),
        ("currency", json!(currency)),
        ("raw_keys", Value::Array(raw_keys)),
    ]))
}

pub fn filter_exposure_scope(input: FilterExposureScopeInput<'_>) -> ExposureScopeResult {
    let reservations = input
        .reservations
        .iter()
        .filter(|item| {
            item.status == "active"
                && account_matches(
                    item.account_id.as_deref(),
                    item.provider_ref.as_deref(),
                    input.account_id,
                    input.provider_ref,
                )
                && attribution_matches(
                    &item.strategy_id,
                    &item.symbol,
                    input.strategy_id,
                    input.symbol,
                    input.attribution_scope,
                )
        })
        .cloned()
        .collect();
    let positions = input
        .positions
        .iter()
        .filter(|item| {
            item.status == "active"
                && account_matches(
                    item.account_id.as_deref(),
                    item.provider_ref.as_deref(),
                    input.account_id,
                    input.provider_ref,
                )
                && attribution_matches(
                    &item.strategy_id,
                    &item.symbol,
                    input.strategy_id,
                    input.symbol,
                    input.attribution_scope,
                )
        })
        .cloned()
        .collect();

    ExposureScopeResult {
        reservations,
        positions,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionFillInput<'a> {
    pub strategy_id: &'a str,
    pub symbol: &'a str,
    pub side: &'a str,
    pub quantity: f64,
    pub notional: f64,
    pub provider_ref: Option<&'a str>,
    pub account_id: Option<&'a str>,
    pub existing: Option<&'a PositionRecord>,
}

pub fn normalize_broker_position_symbol(symbol: &str) -> String {
    let cleaned = symbol.trim().to_ascii_uppercase();
    if matches!(cleaned.as_str(), "BTCUSD" | "BTC/USD") {
        return "BTC/USD".to_string();
    }
    if matches!(cleaned.as_str(), "ETHUSD" | "ETH/USD") {
        return "ETH/USD".to_string();
    }
    if cleaned.is_empty() {
        String::new()
    } else {
        cleaned.replace('/', ".")
    }
}

pub fn infer_position_asset_class(symbol: &str) -> &'static str {
    let text = symbol.to_ascii_uppercase();
    if text.contains('/') || text.ends_with("USD") {
        return "crypto";
    }
    if text.len() >= 15 {
        let tail = &text[text.len().saturating_sub(9)..];
        if tail.contains('C') || tail.contains('P') {
            return "option";
        }
    }
    "equity"
}

pub fn position_id(strategy_id: &str, symbol: &str) -> String {
    let payload_repr = format!("[('strategy_id', '{strategy_id}'), ('symbol', '{symbol}')]");
    let digest = Sha256::digest(payload_repr.as_bytes());
    let hex = format!("{digest:x}");
    format!("pos_{}", &hex[..12])
}

pub fn build_position_fill(input: PositionFillInput<'_>) -> PositionRecord {
    let position_id = position_id(input.strategy_id, input.symbol);
    let signed_quantity = if input.side == "buy" {
        input.quantity
    } else {
        -input.quantity
    };
    let current_quantity = input.existing.map(|item| item.quantity).unwrap_or(0.0);
    let current_notional = input.existing.map(|item| item.notional).unwrap_or(0.0);
    let next_quantity = round10(current_quantity + signed_quantity);
    let status = if next_quantity.abs() < 0.0000000001 {
        "closed"
    } else {
        "active"
    };
    let mut fill_notional = input.notional.abs();
    let next_notional = if input.side == "buy" {
        current_notional + fill_notional
    } else {
        if fill_notional <= 0.0 && current_quantity.abs() > 0.0 {
            fill_notional =
                current_notional * (input.quantity.abs() / current_quantity.abs()).min(1.0);
        }
        (current_notional - current_notional.min(fill_notional)).max(0.0)
    };
    let average_price = if status != "closed" && next_quantity != 0.0 {
        (next_notional / next_quantity).abs()
    } else {
        0.0
    };
    PositionRecord {
        id: position_id,
        strategy_id: input.strategy_id.to_string(),
        symbol: input.symbol.to_string(),
        quantity: if status == "closed" {
            0.0
        } else {
            next_quantity
        },
        average_price: if status == "closed" {
            0.0
        } else {
            average_price
        },
        notional: if status == "closed" {
            0.0
        } else {
            round10(next_notional)
        },
        status: status.to_string(),
        provider_ref: input.provider_ref.map(str::to_string),
        account_id: input.account_id.map(str::to_string),
    }
}

pub fn risk_status(
    account: &RiskAccountState,
    reservations: &[RiskReservation],
    positions: &[PositionRecord],
) -> BTreeMap<String, Value> {
    let active_reservations = reservations
        .iter()
        .filter(|item| item.status.eq_ignore_ascii_case("active"))
        .cloned()
        .collect::<Vec<_>>();
    let active_positions = positions
        .iter()
        .filter(|item| item.status.eq_ignore_ascii_case("active"))
        .cloned()
        .collect::<Vec<_>>();

    let mut positions_by_strategy: BTreeMap<String, Value> = BTreeMap::new();
    for position in &active_positions {
        let entry = positions_by_strategy
            .entry(position.strategy_id.clone())
            .or_insert_with(
                || json!({"activeQuantity": 0.0, "activeNotional": 0.0, "activePositions": 0}),
            );
        let object = entry.as_object_mut().expect("strategy bucket is object");
        let active_quantity = object
            .get("activeQuantity")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            + position.quantity;
        let active_notional = object
            .get("activeNotional")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            + position.notional;
        let active_count = object
            .get("activePositions")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        object.insert(
            "activeQuantity".to_string(),
            json!(round10(active_quantity)),
        );
        object.insert("activeNotional".to_string(), json!(round6(active_notional)));
        object.insert("activePositions".to_string(), json!(active_count));
    }

    evidence_map([
        (
            "account",
            json!({
                "account_equity": account.account_equity,
                "buying_power": account.buying_power,
                "daily_realized_pnl": account.daily_realized_pnl,
            }),
        ),
        ("activeReservations", json!(active_reservations)),
        (
            "activeNotional",
            json!(round6(
                active_reservations
                    .iter()
                    .map(|item| item.notional)
                    .sum::<f64>()
            )),
        ),
        ("activeCount", json!(active_reservations.len())),
        ("activePositions", json!(active_positions)),
        ("positionsByStrategy", json!(positions_by_strategy)),
    ])
}

pub fn stress_test_portfolio(
    account: &RiskAccountState,
    positions: &[PositionRecord],
    shocks: &BTreeMap<String, f64>,
) -> BTreeMap<String, Value> {
    let default_shock = *shocks.get("default").unwrap_or(&-0.05);
    let active_positions = positions
        .iter()
        .filter(|item| item.status.eq_ignore_ascii_case("active"))
        .collect::<Vec<_>>();
    let mut by_asset_class: BTreeMap<String, Value> = BTreeMap::new();
    let mut position_results = Vec::new();
    let mut total_loss = 0.0;
    let mut total_notional = 0.0;

    for position in active_positions {
        let asset_class = infer_position_asset_class(&position.symbol).to_string();
        let shock = *shocks.get(&asset_class).unwrap_or(&default_shock);
        let notional = position.notional;
        total_notional = round6(total_notional + notional);
        let stressed_pnl = if position.quantity >= 0.0 {
            notional * shock
        } else {
            -notional * shock
        };
        let stressed_loss = round6((-stressed_pnl).max(0.0));
        total_loss = round6(total_loss + stressed_loss);
        let bucket = by_asset_class.entry(asset_class.clone()).or_insert_with(
            || json!({"notional": 0.0, "stressed_loss": 0.0, "positions": 0, "shock": shock}),
        );
        let object = bucket.as_object_mut().expect("asset bucket is object");
        object.insert(
            "notional".to_string(),
            json!(round6(
                object
                    .get("notional")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    + notional
            )),
        );
        object.insert(
            "stressed_loss".to_string(),
            json!(round6(
                object
                    .get("stressed_loss")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    + stressed_loss
            )),
        );
        object.insert(
            "positions".to_string(),
            json!(object.get("positions").and_then(Value::as_u64).unwrap_or(0) + 1),
        );
        position_results.push(json!({
            "id": position.id,
            "symbol": position.symbol,
            "asset_class": asset_class,
            "quantity": position.quantity,
            "notional": notional,
            "shock": shock,
            "stressed_pnl": round6(stressed_pnl),
            "stressed_loss": stressed_loss,
        }));
    }

    let equity_after = round6(account.account_equity - total_loss);
    let buying_power_after = round6(account.buying_power - total_loss);
    evidence_map([
        (
            "shocks",
            json!(shocks
                .iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect::<BTreeMap<_, _>>()),
        ),
        ("position_count", json!(position_results.len())),
        ("total_notional", json!(round6(total_notional))),
        ("total_stressed_loss", json!(total_loss)),
        ("equity_after_stress", json!(equity_after)),
        ("buying_power_after_stress", json!(buying_power_after)),
        (
            "breaches",
            json!({
                "negative_equity": equity_after < 0.0,
                "negative_buying_power": buying_power_after < 0.0,
            }),
        ),
        ("by_asset_class", json!(by_asset_class)),
        ("positions", Value::Array(position_results)),
    ])
}

fn resolve_plugin_quantity(
    symbol: &str,
    plugin_ref: &str,
    plugin_config: &BTreeMap<String, Value>,
    risk_limits: &RiskLimits,
    account: &RiskAccountState,
    rows: &[BTreeMap<String, Value>],
    default_quantity: f64,
) -> QuantityResolution {
    let normalized_ref = plugin_ref.trim().to_ascii_lowercase();
    if !matches!(
        normalized_ref.as_str(),
        "plugin-allocation" | "builtin:plugin-allocation" | "builtin:plugin-allocation@1"
    ) {
        return QuantityResolution {
            quantity: default_quantity,
            evidence: evidence_map([
                ("mode", json!("plugin")),
                ("plugin_ref", json!(plugin_ref)),
                ("applied", json!(false)),
                ("quantity", json!(default_quantity)),
                ("reason", json!("unsupported_plugin")),
            ]),
        };
    }
    let objective = plugin_config
        .get("objective")
        .and_then(Value::as_str)
        .unwrap_or("risk_parity")
        .trim()
        .to_ascii_lowercase();
    if objective == "momentum" {
        let window = plugin_config
            .get("rebalance_window")
            .and_then(number_value)
            .unwrap_or(10.0)
            .max(1.0);
        let concrete_quantity = ((window as i64 / 4).max(1) as f64).max(1.0);
        let quantity = concrete_quantity.min(risk_limits.max_order_quantity);
        return QuantityResolution {
            quantity,
            evidence: evidence_map([
                ("mode", json!("plugin")),
                ("plugin_ref", json!(plugin_ref)),
                ("plugin_output_mode", json!("concrete_sizing")),
                ("objective", json!(objective)),
                ("applied", json!(true)),
                ("quantity", json!(quantity)),
                (
                    "bounded_by_max_order_quantity",
                    json!(quantity < concrete_quantity),
                ),
            ]),
        };
    }

    let symbols = plugin_config
        .get("symbols")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![symbol.to_string()]);
    let weight = 1.0 / symbols.len() as f64;
    let targets = symbols
        .into_iter()
        .map(|symbol| AllocationTarget { symbol, weight })
        .collect::<Vec<_>>();
    let (total_exposure, total_evidence) = resolve_plugin_total_exposure(plugin_config, account);
    let mut resolution = resolve_target_allocation_quantity(TargetAllocationInput {
        symbol,
        targets: &targets,
        risk_limits,
        account,
        rows,
        default_quantity,
        exposure_cap_override: total_exposure,
        mode: "plugin",
    });
    resolution
        .evidence
        .insert("plugin_ref".to_string(), json!(plugin_ref));
    resolution.evidence.insert(
        "plugin_output_mode".to_string(),
        json!("relative_allocations"),
    );
    resolution
        .evidence
        .insert("objective".to_string(), json!(objective));
    resolution
        .evidence
        .insert("total_exposure_resolution".to_string(), total_evidence);
    resolution
}

struct TargetAllocationInput<'a> {
    symbol: &'a str,
    targets: &'a [AllocationTarget],
    risk_limits: &'a RiskLimits,
    account: &'a RiskAccountState,
    rows: &'a [BTreeMap<String, Value>],
    default_quantity: f64,
    exposure_cap_override: Option<f64>,
    mode: &'a str,
}

fn resolve_target_allocation_quantity(input: TargetAllocationInput<'_>) -> QuantityResolution {
    let TargetAllocationInput {
        symbol,
        targets,
        risk_limits,
        account,
        rows,
        default_quantity,
        exposure_cap_override,
        mode,
    } = input;
    let weight = target_weight(symbol, targets);
    let price = latest_price(rows);
    let (Some(weight), Some(price)) = (weight, price) else {
        return QuantityResolution {
            quantity: default_quantity,
            evidence: evidence_map([
                ("mode", json!(mode)),
                ("applied", json!(false)),
                ("quantity", json!(default_quantity)),
                ("reason", json!("missing_weight_or_price")),
            ]),
        };
    };
    if price <= 0.0 {
        return QuantityResolution {
            quantity: default_quantity,
            evidence: evidence_map([
                ("mode", json!(mode)),
                ("applied", json!(false)),
                ("quantity", json!(default_quantity)),
                ("reason", json!("missing_weight_or_price")),
            ]),
        };
    }
    let (exposure_cap, exposure_source) = if let Some(override_value) = exposure_cap_override {
        (
            override_value
                .min(risk_limits.max_notional)
                .min(account.buying_power),
            "plugin_total_exposure",
        )
    } else {
        let mut exposure_cap = risk_limits.max_notional.min(account.buying_power);
        if let Some(max_buying_power_pct) = risk_limits.max_buying_power_pct {
            exposure_cap = exposure_cap.min(account.buying_power * (max_buying_power_pct / 100.0));
        }
        if let Some(max_capital_at_risk_pct) = risk_limits.max_capital_at_risk_pct {
            exposure_cap =
                exposure_cap.min(account.account_equity * (max_capital_at_risk_pct / 100.0));
        }
        (exposure_cap, "risk_cap")
    };
    let target_notional = (exposure_cap * weight).max(0.0);
    let quantity = (target_notional / price).min(risk_limits.max_order_quantity);
    if quantity <= 0.0 {
        return QuantityResolution {
            quantity: default_quantity,
            evidence: evidence_map([
                ("mode", json!(mode)),
                ("applied", json!(false)),
                ("quantity", json!(default_quantity)),
                ("reason", json!("zero_target_quantity")),
            ]),
        };
    }
    QuantityResolution {
        quantity,
        evidence: evidence_map([
            ("mode", json!(mode)),
            ("applied", json!(true)),
            ("symbol", json!(symbol)),
            ("target_weight", json!(weight)),
            ("target_notional", json!(round8(target_notional))),
            ("total_exposure", json!(round8(exposure_cap))),
            ("total_exposure_source", json!(exposure_source)),
            ("price", json!(price)),
            ("quantity", json!(quantity)),
            (
                "bounded_by_max_order_quantity",
                json!(quantity >= risk_limits.max_order_quantity),
            ),
        ]),
    }
}

fn resolve_plugin_total_exposure(
    params: &BTreeMap<String, Value>,
    account: &RiskAccountState,
) -> (Option<f64>, Value) {
    let mut raw_value = params.get("total_exposure_value").and_then(number_value);
    let mut exposure_type = params
        .get("total_exposure_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if raw_value.is_none() {
        raw_value = params.get("position_size_value").and_then(number_value);
        if raw_value.is_some() && exposure_type.is_empty() {
            exposure_type = params
                .get("position_sizing_mode")
                .and_then(Value::as_str)
                .unwrap_or("dollar_amount")
                .trim()
                .to_ascii_lowercase();
        }
    }
    let Some(raw_value) = raw_value.filter(|value| *value > 0.0) else {
        return (
            None,
            json!({"applied": false, "reason": "missing_total_exposure"}),
        );
    };
    if exposure_type == "percent_of_account" {
        let basis = params
            .get("basis")
            .and_then(Value::as_str)
            .unwrap_or("buying_power")
            .trim()
            .to_ascii_lowercase();
        let base = if basis == "equity" {
            account.account_equity
        } else {
            account.buying_power
        };
        let total = base * (raw_value / 100.0);
        return (
            Some(total),
            json!({"applied": true, "type": exposure_type, "basis": basis, "value": raw_value, "base": base, "total_exposure": total}),
        );
    }
    (
        Some(raw_value),
        json!({"applied": true, "type": if exposure_type.is_empty() { "dollar_amount" } else { exposure_type.as_str() }, "value": raw_value, "total_exposure": raw_value}),
    )
}

fn quote_price(quote: &Quote, source: &str) -> Option<f64> {
    match source {
        "mid" => match (quote.bid, quote.ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
            _ => quote.last,
        },
        "bid" => quote.bid.or(quote.last),
        "ask" => quote.ask.or(quote.last),
        _ => quote.last,
    }
}

fn round_step(value: f64, step: Option<f64>) -> f64 {
    let Some(step) = step else {
        return value;
    };
    if step <= 0.0 {
        return value;
    }
    round8((value / step).round() * step)
}

fn round8(value: f64) -> f64 {
    (value * 100_000_000.0).round() / 100_000_000.0
}

fn round10(value: f64) -> f64 {
    (value * 10_000_000_000.0).round() / 10_000_000_000.0
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn contract_key(contract: &OptionContract) -> String {
    format!(
        "{}-{}-{}-{}",
        contract.underlying,
        contract.expiry,
        value_key(&contract.strike),
        contract.right
    )
}

fn value_key(value: &Value) -> String {
    match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn number_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        _ => None,
    }
}

fn first_number(payload: &BTreeMap<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| payload.get(*key).and_then(number_value))
}

fn leg_symbol(leg: &Leg) -> Option<String> {
    leg.symbol
        .as_ref()
        .map(|symbol| symbol.trim())
        .filter(|symbol| !symbol.is_empty())
        .map(str::to_string)
        .or_else(|| {
            leg.contract
                .as_ref()
                .map(|contract| contract.underlying.trim())
                .filter(|symbol| !symbol.is_empty())
                .map(str::to_string)
        })
}

fn target_weight(symbol: &str, targets: &[AllocationTarget]) -> Option<f64> {
    let normalized = normalize_symbol(symbol);
    targets
        .iter()
        .find(|target| normalize_symbol(&target.symbol) == normalized)
        .map(|target| target.weight)
}

fn latest_price(rows: &[BTreeMap<String, Value>]) -> Option<f64> {
    let latest = rows.last()?;
    for key in ["last", "close", "price"] {
        if let Some(value) = latest.get(key).and_then(number_value) {
            return Some(value);
        }
    }
    None
}

fn risk_latest_price(rows: &[BTreeMap<String, Value>]) -> Option<f64> {
    let latest = rows.last()?;
    for key in ["close", "last", "price"] {
        if let Some(value) = latest.get(key).and_then(number_value) {
            return Some(value);
        }
    }
    None
}

fn delta_from_rows(rows: &[BTreeMap<String, Value>]) -> Option<f64> {
    let latest = rows.last()?;
    for key in ["delta", "option_delta"] {
        if let Some(value) = latest.get(key).and_then(number_value) {
            return Some(value);
        }
    }
    latest
        .get("greeks")
        .and_then(Value::as_object)
        .and_then(|greeks| greeks.get("delta"))
        .and_then(number_value)
}

fn order_multiplier(symbol: &str) -> f64 {
    let text = symbol.to_ascii_uppercase();
    if text.contains('/') || text.ends_with("USD") {
        return 1.0;
    }
    if text.len() > 15 && (text.contains('C') || text.contains('P')) {
        return 100.0;
    }
    1.0
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedOptionLeg {
    underlying: String,
    expiry: String,
    right: String,
    strike: f64,
    side: String,
    quantity: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct ParsedOptionSymbol {
    underlying: String,
    expiry: String,
    right: String,
    strike: f64,
}

fn option_leg(order_plan: &OrderPlan) -> Option<ParsedOptionLeg> {
    let parsed = parse_option_symbol(&order_plan.symbol)?;
    Some(ParsedOptionLeg {
        underlying: parsed.underlying,
        expiry: parsed.expiry,
        right: parsed.right,
        strike: parsed.strike,
        side: order_plan.side.clone(),
        quantity: order_plan.quantity,
    })
}

fn parse_option_symbol(symbol: &str) -> Option<ParsedOptionSymbol> {
    let text = symbol
        .trim()
        .to_ascii_uppercase()
        .replace("O:", "")
        .replace(' ', "");
    if text.len() < 16 {
        return None;
    }
    let strike_start = text.len().checked_sub(8)?;
    let strike_text = &text[strike_start..];
    if !strike_text.chars().all(|item| item.is_ascii_digit()) {
        return None;
    }
    let right_index = strike_start.checked_sub(1)?;
    let right_char = text.as_bytes().get(right_index).copied()? as char;
    let right = match right_char {
        'C' => "CALL",
        'P' => "PUT",
        _ => return None,
    };
    let before_right = &text[..right_index];
    let date_len = if before_right.len() >= 8
        && before_right[before_right.len() - 8..]
            .chars()
            .all(|item| item.is_ascii_digit())
    {
        8
    } else if before_right.len() >= 6
        && before_right[before_right.len() - 6..]
            .chars()
            .all(|item| item.is_ascii_digit())
    {
        6
    } else {
        return None;
    };
    let date_start = before_right.len() - date_len;
    let underlying = before_right[..date_start].to_string();
    if underlying.is_empty() {
        return None;
    }
    let raw_date = &before_right[date_start..];
    let expiry = if date_len == 6 {
        format!(
            "20{}-{}-{}",
            &raw_date[0..2],
            &raw_date[2..4],
            &raw_date[4..6]
        )
    } else {
        format!(
            "{}-{}-{}",
            &raw_date[0..4],
            &raw_date[4..6],
            &raw_date[6..8]
        )
    };
    Some(ParsedOptionSymbol {
        underlying,
        expiry,
        right: right.to_string(),
        strike: strike_text.parse::<f64>().ok()? / 1000.0,
    })
}

fn signed_premium(order_plan: &OrderPlan) -> f64 {
    let price = order_plan.limit_price.unwrap_or(0.0);
    if order_plan.side == "sell" {
        price
    } else {
        -price
    }
}

fn account_matches(
    item_account_id: Option<&str>,
    item_provider_ref: Option<&str>,
    account_id: Option<&str>,
    provider_ref: Option<&str>,
) -> bool {
    if let Some(account_id) = account_id.filter(|value| *value != "local") {
        return item_account_id == Some(account_id)
            || (item_account_id.is_none() && item_provider_ref == provider_ref);
    }
    if let Some(provider_ref) = provider_ref {
        return item_provider_ref.is_none() || item_provider_ref == Some(provider_ref);
    }
    true
}

fn attribution_matches(
    item_strategy_id: &str,
    item_symbol: &str,
    strategy_id: &str,
    symbol: &str,
    attribution_scope: AttributionScope,
) -> bool {
    if item_strategy_id == strategy_id {
        return true;
    }
    if attribution_scope == AttributionScope::Strict {
        return false;
    }
    underlying_key(item_symbol) == underlying_key(symbol)
}

fn underlying_key(symbol: &str) -> String {
    let text = symbol.to_ascii_uppercase().replace(' ', "");
    if matches!(text.as_str(), "BTCUSD" | "BTC/USD") {
        return "BTC".to_string();
    }
    if matches!(text.as_str(), "ETHUSD" | "ETH/USD") {
        return "ETH".to_string();
    }
    if let Some(parsed) = parse_option_symbol(&text) {
        return parsed.underlying;
    }
    if let Some((underlying, _)) = text.split_once('/') {
        return underlying.to_string();
    }
    if text.ends_with("USD") && text.len() > 3 {
        return text[..text.len() - 3].to_string();
    }
    text
}

fn instrument_kind_for_order(order_plan: &OrderPlan) -> &'static str {
    let symbol = order_plan.symbol.to_ascii_uppercase();
    if parse_option_symbol(&symbol).is_some() {
        return "OPTION";
    }
    if symbol.contains('/') || matches!(symbol.as_str(), "BTCUSD" | "ETHUSD" | "SOLUSD") {
        return "CRYPTO";
    }
    "EQUITY"
}

fn constraint_for_instrument<'a>(
    constraints: &'a BTreeMap<String, Value>,
    instrument_kind: &str,
) -> Option<&'a Map<String, Value>> {
    [
        instrument_kind.to_string(),
        instrument_kind.to_ascii_lowercase(),
        title_case_ascii(instrument_kind),
    ]
    .iter()
    .find_map(|key| constraints.get(key).and_then(Value::as_object))
}

fn symbol_constraint<'a>(
    constraints: &'a BTreeMap<String, Value>,
    symbol: &str,
) -> Option<&'a Map<String, Value>> {
    let by_symbol = constraints.get("by_symbol").and_then(Value::as_object)?;
    let normalized = symbol.to_ascii_uppercase().replace('/', "");
    by_symbol.iter().find_map(|(key, value)| {
        if key.to_ascii_uppercase().replace('/', "") == normalized {
            value.as_object()
        } else {
            None
        }
    })
}

fn order_constraint_price(
    order_plan: &OrderPlan,
    market_snapshot: Option<&BTreeMap<String, Value>>,
) -> Option<f64> {
    if order_plan.limit_price.is_some() {
        return order_plan.limit_price;
    }
    let snapshot = market_snapshot?;
    if let Some(quotes) = snapshot.get("quotes").and_then(Value::as_object) {
        let compact_symbol = order_plan.symbol.replace('/', "");
        if let Some(quote) = quotes
            .get(&order_plan.symbol)
            .or_else(|| quotes.get(&compact_symbol))
            .and_then(Value::as_object)
        {
            return quote_price_from_json_map(quote);
        }
    }
    quote_price_from_btree(snapshot)
}

fn quote_price_from_json_map(quote: &Map<String, Value>) -> Option<f64> {
    let bid = quote.get("bid").and_then(number_value);
    let ask = quote.get("ask").and_then(number_value);
    let last = quote
        .get("last")
        .or_else(|| quote.get("close"))
        .or_else(|| quote.get("price"))
        .and_then(number_value);
    if last.is_some() {
        return last;
    }
    match (bid, ask) {
        (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
        (Some(bid), None) => Some(bid),
        (None, Some(ask)) => Some(ask),
        _ => None,
    }
}

fn quote_price_from_btree(quote: &BTreeMap<String, Value>) -> Option<f64> {
    let bid = quote.get("bid").and_then(number_value);
    let ask = quote.get("ask").and_then(number_value);
    let last = quote
        .get("last")
        .or_else(|| quote.get("close"))
        .or_else(|| quote.get("price"))
        .and_then(number_value);
    if last.is_some() {
        return last;
    }
    match (bid, ask) {
        (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
        (Some(bid), None) => Some(bid),
        (None, Some(ask)) => Some(ask),
        _ => None,
    }
}

fn title_case_ascii(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!(
        "{}{}",
        first.to_ascii_uppercase(),
        chars.as_str().to_ascii_lowercase()
    )
}

fn format_number(value: f64) -> String {
    let text = format!("{value:.8}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn normalize_symbol(symbol: &str) -> String {
    symbol.to_ascii_uppercase().replace(['/', ' '], "")
}

fn current_unix_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn evidence_map(items: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_policy_applies_auto_limit_and_option_midpoint() {
        let mut snapshot = MarketSnapshot::default();
        snapshot.quotes.insert(
            "SPY".to_string(),
            Quote {
                bid: Some(519.91),
                ask: Some(520.09),
                last: Some(520.02),
                ..Quote::default()
            },
        );
        snapshot.option_quotes.insert(
            "SPY-2026-06-19-520-C".to_string(),
            Quote {
                bid: Some(4.21),
                ask: Some(4.39),
                ..Quote::default()
            },
        );

        let auto = apply_auto_limit_policy(
            &[Leg {
                symbol: Some("SPY".to_string()),
                ..Leg::default()
            }],
            &snapshot,
            "mid",
            Some(0.05),
        );
        let option = apply_option_mid_policy(
            &[Leg {
                contract: Some(OptionContract {
                    underlying: "SPY".to_string(),
                    expiry: "2026-06-19".to_string(),
                    strike: json!(520),
                    right: "C".to_string(),
                }),
                ..Leg::default()
            }],
            &snapshot,
            Some(0.01),
        );

        assert_eq!("LMT", auto.order_type);
        assert_eq!(Some(520.0), auto.legs[0].limit_price);
        assert_eq!("LMT", option.order_type);
        assert_eq!(Some(4.3), option.legs[0].limit_price);
    }

    #[test]
    fn allocation_policy_supports_concrete_and_relative_outputs() {
        let legs = vec![
            Leg {
                symbol: Some("BTC/USD".to_string()),
                ..Leg::default()
            },
            Leg {
                symbol: Some("ETH/USD".to_string()),
                ..Leg::default()
            },
        ];
        let mut momentum = BTreeMap::new();
        momentum.insert("objective".to_string(), json!("momentum"));
        momentum.insert("rebalance_window".to_string(), json!(8));

        let concrete =
            apply_builtin_allocation_policy(&legs, "builtin:plugin-allocation@1", &momentum, None)
                .unwrap();

        assert_eq!(Some(2.0), concrete.legs[0].qty);
        assert_eq!(Some(1.0), concrete.legs[1].qty);
        assert_eq!(
            Some(&json!("concrete_sizing")),
            concrete.metadata.get("allocation_policy_output_mode")
        );

        let mut relative = BTreeMap::new();
        relative.insert("objective".to_string(), json!("risk_parity"));
        let sizing = BTreeMap::from([("total_exposure_value".to_string(), json!(1000))]);

        let relative_out = apply_builtin_allocation_policy(
            &legs,
            "builtin:plugin-allocation@1",
            &relative,
            Some(&sizing),
        )
        .unwrap();

        assert_eq!(Some(500.0), relative_out.legs[0].target_notional);
        assert_eq!(Some(500.0), relative_out.legs[1].target_notional);
        assert_eq!(Some(&json!(false)), relative_out.sizing.get("override"));
    }

    #[test]
    fn resolve_order_quantity_supports_percent_of_account_plugin_sizing() {
        let mut plugin_config = BTreeMap::new();
        plugin_config.insert("symbols".to_string(), json!(["BTC/USD"]));
        plugin_config.insert(
            "position_sizing_mode".to_string(),
            json!("percent_of_account"),
        );
        plugin_config.insert("position_size_value".to_string(), json!(2));
        plugin_config.insert("basis".to_string(), json!("equity"));
        let rows = vec![BTreeMap::from([("close".to_string(), json!(50000))])];

        let result = resolve_order_quantity(
            "BTC/USD",
            &AllocationPolicy::Plugin {
                plugin_ref: "builtin:plugin-allocation@1".to_string(),
                plugin_config,
            },
            &RiskLimits {
                max_notional: 5000.0,
                max_order_quantity: 10.0,
                max_daily_loss: 2500.0,
                max_concurrent_positions: 5,
                max_buying_power_pct: None,
                max_capital_at_risk_pct: None,
                max_margin_used_pct: None,
                max_delta_abs: None,
                attribution_scope: AttributionScope::Strict,
            },
            &RiskAccountState {
                account_equity: 100000.0,
                buying_power: 50000.0,
                daily_realized_pnl: 0.0,
            },
            &rows,
            0.001,
        );

        assert_eq!(0.04, result.quantity);
        assert_eq!(
            Some(&json!("relative_allocations")),
            result.evidence.get("plugin_output_mode")
        );
        assert_eq!(
            Some(&json!("plugin_total_exposure")),
            result.evidence.get("total_exposure_source")
        );
        assert_eq!(
            Some(
                &json!({"applied": true, "type": "percent_of_account", "basis": "equity", "value": 2.0, "base": 100000.0, "total_exposure": 2000.0})
            ),
            result.evidence.get("total_exposure_resolution")
        );
    }

    #[test]
    fn exposure_ledger_dedupes_links_and_expires_reservations() {
        let mut ledger = ExposureLedger::new_with_clock(1, 1000.0);

        let first = ledger.reserve("intent-1", Some("strategy-1"), Some("run-1"));
        let duplicate = ledger.reserve("intent-1", Some("strategy-1"), Some("run-2"));
        ledger.link_order("intent-1", "order-1");

        assert_eq!(first, duplicate);
        assert_eq!(
            Some("order-1"),
            ledger
                .get("intent-1")
                .and_then(|item| item.order_id.as_deref())
        );

        ledger.set_clock(1010.0);

        assert_eq!(None, ledger.get("intent-1"));
    }

    #[test]
    fn order_notional_uses_latest_local_price_and_option_multiplier() {
        let crypto_plan = OrderPlan {
            symbol: "BTC/USD".to_string(),
            side: "buy".to_string(),
            quantity: 0.02,
            ..OrderPlan::default()
        };
        let option_plan = OrderPlan {
            symbol: "SPY240621C00450000".to_string(),
            side: "buy".to_string(),
            quantity: 1.0,
            ..OrderPlan::default()
        };
        let rows = vec![BTreeMap::from([("last".to_string(), json!(50000))])];

        assert_eq!(
            1000.0,
            estimate_order_notional(&crypto_plan, &rows).unwrap()
        );
        assert_eq!(
            125.0,
            estimate_order_notional(
                &option_plan,
                &[BTreeMap::from([("close".to_string(), json!(1.25))])]
            )
            .unwrap()
        );
        assert!(estimate_order_notional(&crypto_plan, &[]).is_err());
    }

    #[test]
    fn broker_constraints_deny_min_quantity_and_min_notional() {
        let constraints = json!({
            "CRYPTO": {"min_qty": 0.001, "min_notional": 50},
            "by_symbol": {"BTC/USD": {"min_notional": 75}}
        });
        let caps = BrokerCapabilities {
            provider_ref: "sim".to_string(),
            modes: vec!["paper".to_string()],
            constraints: constraints
                .as_object()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
        };

        let qty_plan = OrderPlan {
            symbol: "BTC/USD".to_string(),
            side: "buy".to_string(),
            quantity: 0.0003,
            ..OrderPlan::default()
        };
        let qty_result = validate_broker_order_constraints(&qty_plan, &caps, None);
        assert!(!qty_result.allowed);
        assert!(qty_result.reason.unwrap().contains("min_qty"));

        let notional_plan = OrderPlan {
            symbol: "BTC/USD".to_string(),
            side: "buy".to_string(),
            quantity: 0.002,
            limit_price: Some(30000.0),
        };
        let notional_result = validate_broker_order_constraints(&notional_plan, &caps, None);
        assert!(!notional_result.allowed);
        assert!(notional_result.reason.unwrap().contains("min_notional 75"));
    }

    #[test]
    fn option_strategy_margin_estimates_defined_risk_vertical() {
        let short_call = OrderPlan {
            symbol: "SPY20260116C00430000".to_string(),
            side: "sell".to_string(),
            quantity: 1.0,
            limit_price: Some(1.30),
        };
        let long_call = OrderPlan {
            symbol: "SPY20260116C00435000".to_string(),
            side: "buy".to_string(),
            quantity: 1.0,
            limit_price: Some(0.45),
        };

        let margin = estimate_option_strategy_margin(&[short_call, long_call]);

        assert_eq!(
            Some(&json!("defined_risk_option_spread")),
            margin.get("margin_model")
        );
        assert_eq!(Some(&json!(5.0)), margin.get("spread_width"));
        assert_eq!(Some(&json!(0.85)), margin.get("net_credit"));
        assert_eq!(Some(&json!(415.0)), margin.get("buying_power_effect"));
    }

    #[test]
    fn broker_margin_payload_normalizes_margin_aliases_and_raw_keys() {
        let payload = BTreeMap::from([
            ("buyingPowerEffect".to_string(), json!("123.45")),
            ("initialMargin".to_string(), json!("123.45")),
            ("maintenanceMargin".to_string(), json!("87.65")),
            ("currency".to_string(), json!("USD")),
        ]);

        let normalized = normalize_broker_margin_payload(&payload).unwrap();

        assert_eq!(
            Some(&json!("broker_margin_preview")),
            normalized.get("margin_model")
        );
        assert_eq!(Some(&json!(123.45)), normalized.get("buying_power_effect"));
        assert_eq!(Some(&json!(123.45)), normalized.get("initial_margin"));
        assert_eq!(Some(&json!(87.65)), normalized.get("maintenance_margin"));
        assert_eq!(
            Some(&json!([
                "buyingPowerEffect",
                "currency",
                "initialMargin",
                "maintenanceMargin"
            ])),
            normalized.get("raw_keys")
        );
        assert!(normalize_broker_margin_payload(&BTreeMap::new()).is_err());
    }

    #[test]
    fn exposure_scope_strict_ignores_other_strategy_and_inclusive_counts_underlying() {
        let position = PositionRecord {
            id: "position-other-btc".to_string(),
            strategy_id: "other-strategy".to_string(),
            symbol: "BTCUSD".to_string(),
            status: "active".to_string(),
            provider_ref: Some("alpaca-paper".to_string()),
            account_id: Some("paper-account".to_string()),
            ..PositionRecord::default()
        };
        let reservation = RiskReservation {
            id: "risk-same-strategy".to_string(),
            strategy_id: "strategy-1".to_string(),
            symbol: "ETH/USD".to_string(),
            status: "active".to_string(),
            provider_ref: Some("alpaca-paper".to_string()),
            account_id: Some("paper-account".to_string()),
            ..RiskReservation::default()
        };

        let strict = filter_exposure_scope(FilterExposureScopeInput {
            reservations: std::slice::from_ref(&reservation),
            positions: std::slice::from_ref(&position),
            strategy_id: "strategy-1",
            symbol: "BTC/USD",
            attribution_scope: AttributionScope::Strict,
            account_id: Some("paper-account"),
            provider_ref: Some("alpaca-paper"),
        });
        let inclusive = filter_exposure_scope(FilterExposureScopeInput {
            reservations: &[reservation],
            positions: &[position],
            strategy_id: "strategy-1",
            symbol: "BTC/USD",
            attribution_scope: AttributionScope::InclusiveByUnderlying,
            account_id: Some("paper-account"),
            provider_ref: Some("alpaca-paper"),
        });

        assert_eq!(1, strict.reservations.len());
        assert_eq!(0, strict.positions.len());
        assert_eq!(1, inclusive.reservations.len());
        assert_eq!(1, inclusive.positions.len());
    }

    #[test]
    fn check_order_risk_enforces_account_and_aggregate_limits() {
        let plan = OrderPlan {
            symbol: "BTC/USD".to_string(),
            side: "buy".to_string(),
            quantity: 0.02,
            ..OrderPlan::default()
        };
        let rows = vec![BTreeMap::from([("close".to_string(), json!(50000))])];
        let account = RiskAccountState {
            account_equity: 1000.0,
            buying_power: 20000.0,
            daily_realized_pnl: 0.0,
        };
        let reservation = RiskReservation {
            id: "risk-1".to_string(),
            strategy_id: "strategy-1".to_string(),
            symbol: "BTC/USD".to_string(),
            notional: 100.0,
            status: "active".to_string(),
            ..RiskReservation::default()
        };

        let allowed = check_order_risk(
            &plan,
            &RiskLimits {
                max_notional: 1200.0,
                max_order_quantity: 1.0,
                max_daily_loss: 250.0,
                max_concurrent_positions: 5,
                max_buying_power_pct: None,
                max_capital_at_risk_pct: None,
                max_margin_used_pct: None,
                max_delta_abs: None,
                attribution_scope: AttributionScope::Strict,
            },
            &rows,
            std::slice::from_ref(&reservation),
            Some(&RiskAccountState {
                buying_power: 1500.0,
                ..account.clone()
            }),
            &[],
        )
        .unwrap();

        assert_eq!(1000.0, allowed);

        let denied = check_order_risk(
            &plan,
            &RiskLimits {
                max_notional: 1200.0,
                max_order_quantity: 1.0,
                max_daily_loss: 250.0,
                max_concurrent_positions: 5,
                max_buying_power_pct: Some(5.0),
                max_capital_at_risk_pct: None,
                max_margin_used_pct: None,
                max_delta_abs: None,
                attribution_scope: AttributionScope::Strict,
            },
            &rows,
            &[reservation],
            Some(&account),
            &[],
        )
        .unwrap_err();

        assert!(denied.reason.contains("aggregate exposure"));
        assert!(denied.reason.contains("5% of buying_power"));
    }

    #[test]
    fn check_order_risk_enforces_margin_delta_and_concurrent_positions() {
        let option_plan = OrderPlan {
            symbol: "SPY240621C00450000".to_string(),
            side: "buy".to_string(),
            quantity: 1.0,
            limit_price: Some(1.0),
        };
        let rows = vec![BTreeMap::from([
            ("close".to_string(), json!(1.0)),
            ("delta".to_string(), json!(0.60)),
        ])];
        let account = RiskAccountState {
            account_equity: 1000.0,
            buying_power: 1000.0,
            daily_realized_pnl: 0.0,
        };
        let active_position = PositionRecord {
            id: "position-1".to_string(),
            strategy_id: "strategy-1".to_string(),
            symbol: "SPY240621C00450000".to_string(),
            quantity: 1.0,
            average_price: 2.0,
            notional: 400.0,
            status: "active".to_string(),
            ..PositionRecord::default()
        };
        let active_reservation = RiskReservation {
            id: "risk-delta".to_string(),
            strategy_id: "strategy-1".to_string(),
            symbol: "SPY240621C00450000".to_string(),
            notional: 20.0,
            delta_exposure: 25.0,
            status: "active".to_string(),
            ..RiskReservation::default()
        };

        let margin_denied = check_order_risk(
            &OrderPlan {
                symbol: "BTC/USD".to_string(),
                side: "buy".to_string(),
                quantity: 0.003,
                ..OrderPlan::default()
            },
            &RiskLimits {
                max_notional: 1000.0,
                max_order_quantity: 1.0,
                max_daily_loss: 250.0,
                max_concurrent_positions: 5,
                max_buying_power_pct: None,
                max_capital_at_risk_pct: None,
                max_margin_used_pct: Some(50.0),
                max_delta_abs: None,
                attribution_scope: AttributionScope::Strict,
            },
            &[BTreeMap::from([("close".to_string(), json!(50000))])],
            &[],
            Some(&account),
            std::slice::from_ref(&active_position),
        )
        .unwrap_err();

        assert!(margin_denied.reason.contains("margin_used"));

        let delta_denied = check_order_risk(
            &option_plan,
            &RiskLimits {
                max_notional: 1000.0,
                max_order_quantity: 2.0,
                max_daily_loss: 250.0,
                max_concurrent_positions: 5,
                max_buying_power_pct: None,
                max_capital_at_risk_pct: None,
                max_margin_used_pct: None,
                max_delta_abs: Some(60.0),
                attribution_scope: AttributionScope::Strict,
            },
            &rows,
            &[active_reservation],
            Some(&account),
            std::slice::from_ref(&active_position),
        )
        .unwrap_err();

        assert!(delta_denied.reason.contains("abs_delta"));

        let concurrent_denied = check_order_risk(
            &option_plan,
            &RiskLimits {
                max_notional: 1000.0,
                max_order_quantity: 2.0,
                max_daily_loss: 250.0,
                max_concurrent_positions: 1,
                max_buying_power_pct: None,
                max_capital_at_risk_pct: None,
                max_margin_used_pct: None,
                max_delta_abs: None,
                attribution_scope: AttributionScope::Strict,
            },
            &rows,
            &[],
            Some(&account),
            &[active_position],
        )
        .unwrap_err();

        assert!(concurrent_denied
            .reason
            .contains("max_concurrent_positions"));
        assert_eq!(60.0, estimate_order_delta_exposure(&option_plan, &rows));
    }

    #[test]
    fn portfolio_helpers_normalize_symbols_and_build_position_fills() {
        assert_eq!("BTC/USD", normalize_broker_position_symbol("btcusd"));
        assert_eq!("ETH/USD", normalize_broker_position_symbol("ETH/USD"));
        assert_eq!("BRK.B", normalize_broker_position_symbol("brk/b"));
        assert_eq!("crypto", infer_position_asset_class("BTC/USD"));
        assert_eq!("option", infer_position_asset_class("SPY240621C00450000"));
        assert_eq!("equity", infer_position_asset_class("SPY"));

        let opened = build_position_fill(PositionFillInput {
            strategy_id: "strategy-1",
            symbol: "BTC/USD",
            side: "buy",
            quantity: 0.02,
            notional: 1000.0,
            provider_ref: Some("sim"),
            account_id: Some("local"),
            existing: None,
        });
        let reduced = build_position_fill(PositionFillInput {
            strategy_id: "strategy-1",
            symbol: "BTC/USD",
            side: "sell",
            quantity: 0.01,
            notional: 500.0,
            provider_ref: Some("sim"),
            account_id: Some("local"),
            existing: Some(&opened),
        });
        let closed = build_position_fill(PositionFillInput {
            strategy_id: "strategy-1",
            symbol: "BTC/USD",
            side: "sell",
            quantity: 0.01,
            notional: 500.0,
            provider_ref: Some("sim"),
            account_id: Some("local"),
            existing: Some(&reduced),
        });

        assert!(opened.id.starts_with("pos_"));
        assert_eq!(0.02, opened.quantity);
        assert_eq!(50000.0, opened.average_price);
        assert_eq!(1000.0, opened.notional);
        assert_eq!("active", opened.status);
        assert_eq!(0.01, reduced.quantity);
        assert_eq!(500.0, reduced.notional);
        assert_eq!(0.0, closed.quantity);
        assert_eq!(0.0, closed.notional);
        assert_eq!("closed", closed.status);
    }

    #[test]
    fn portfolio_status_and_stress_aggregate_active_exposure() {
        let account = RiskAccountState {
            account_equity: 1000.0,
            buying_power: 800.0,
            daily_realized_pnl: 0.0,
        };
        let positions = vec![
            PositionRecord {
                id: "pos-btc".to_string(),
                strategy_id: "strategy-1".to_string(),
                symbol: "BTC/USD".to_string(),
                quantity: 0.01,
                average_price: 40000.0,
                notional: 400.0,
                status: "active".to_string(),
                ..PositionRecord::default()
            },
            PositionRecord {
                id: "pos-closed".to_string(),
                strategy_id: "strategy-1".to_string(),
                symbol: "SPY".to_string(),
                quantity: 0.0,
                notional: 0.0,
                status: "closed".to_string(),
                ..PositionRecord::default()
            },
        ];
        let reservations = vec![RiskReservation {
            id: "risk-1".to_string(),
            strategy_id: "strategy-1".to_string(),
            symbol: "BTC/USD".to_string(),
            notional: 100.0,
            status: "active".to_string(),
            ..RiskReservation::default()
        }];
        let status = risk_status(&account, &reservations, &positions);

        assert_eq!(Some(&json!(100.0)), status.get("activeNotional"));
        assert_eq!(Some(&json!(1)), status.get("activeCount"));
        assert_eq!(
            Some(&json!({"activeNotional": 400.0, "activePositions": 1, "activeQuantity": 0.01})),
            status
                .get("positionsByStrategy")
                .and_then(Value::as_object)
                .and_then(|value| value.get("strategy-1"))
        );

        let stress = stress_test_portfolio(
            &account,
            &positions,
            &BTreeMap::from([
                ("crypto".to_string(), -0.10),
                ("default".to_string(), -0.05),
            ]),
        );

        assert_eq!(Some(&json!(1)), stress.get("position_count"));
        assert_eq!(Some(&json!(400.0)), stress.get("total_notional"));
        assert_eq!(Some(&json!(40.0)), stress.get("total_stressed_loss"));
        assert_eq!(Some(&json!(960.0)), stress.get("equity_after_stress"));
        assert_eq!(Some(&json!(760.0)), stress.get("buying_power_after_stress"));
    }
}
