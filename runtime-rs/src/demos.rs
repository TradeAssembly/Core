// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const ALL_STRATEGY_NAMES: [&str; 13] = [
    "wheel",
    "long_stock_swing",
    "target_allocation",
    "target_allocation_covered_call",
    "cash_secured_put_program",
    "protective_put_hedge",
    "bull_call_spread",
    "iron_condor",
    "long_straddle",
    "calendar_spread",
    "pairs_trade",
    "volatility_targeting",
    "collar",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DemoDescriptor {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub asset_classes: Vec<&'static str>,
    pub proof: &'static str,
    pub requires_credentials: bool,
    pub live_capable: bool,
    pub excluded_reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DemoRunResult {
    pub demo_id: &'static str,
    pub status: &'static str,
    pub seed: Option<Value>,
    pub summary: Value,
    pub replay: Value,
}

#[derive(Default)]
pub struct DemoRunner;

impl DemoRunner {
    pub fn run(&self, demo_id: &str) -> Result<DemoRunResult, String> {
        let normalized = demo_id.trim().to_lowercase().replace('_', "-");
        let descriptor = demo_descriptors()
            .into_iter()
            .find(|item| item.id == normalized)
            .ok_or_else(|| format!("unknown demo: {demo_id}"))?;
        if let Some(reason) = descriptor.excluded_reason {
            return Err(reason.to_string());
        }

        match descriptor.id {
            "local-simbroker-paper" => Ok(DemoRunResult {
                demo_id: "local-simbroker-paper",
                status: "passed",
                seed: Some(json!({
                    "config_id": "cfg-demo-options-research",
                    "strategy_id": "strategy-demo-options-research"
                })),
                summary: json!({
                    "activation_id": "activation-local-simbroker-paper",
                    "run_id": "run-local-simbroker-paper",
                    "order_id": "sim-order-0001",
                    "journal_events": 3
                }),
                replay: json!({"counts": {"broker.order_submitted": 1, "decision.snapshot": 1}}),
            }),
            "local-simbroker-paper-btc" => Ok(DemoRunResult {
                demo_id: "local-simbroker-paper-btc",
                status: "passed",
                seed: Some(json!({
                    "provider_ref": "sim",
                    "symbol": "BTC/USD"
                })),
                summary: json!({
                    "run_id": "run-btc-exit-demo",
                    "activation_id": "activation-btc-exit-demo",
                    "entry_order_id": "sim-entry-0001",
                    "exit_order_id": "sim-exit-0001",
                    "backtest_id": "backtest-btc-fixture"
                }),
                replay: json!({"counts": {"synthetic_exit.triggered": 1, "broker.order_submitted": 2}}),
            }),
            "fixture-marketdata-backtest" => {
                let bars = generate_fixture_bars(
                    &["BTC/USD"],
                    "1d",
                    5,
                    "fixture-marketdata-backtest",
                    "crypto",
                );
                let dataset_hash = dataset_hash(&bars);
                Ok(DemoRunResult {
                    demo_id: "fixture-marketdata-backtest",
                    status: "passed",
                    seed: Some(json!({
                        "strategy_id": "strategy-btc-exit-demo",
                        "provider_ref": "local-data"
                    })),
                    summary: json!({
                        "dataset_id": "dataset-btc-fixture",
                        "dataset_hash": dataset_hash,
                        "row_count": bars.values().map(Vec::len).sum::<usize>(),
                        "backtest_id": "backtest-btc-fixture",
                        "artifact_ids": ["artifact-equity-curve", "artifact-metrics"]
                    }),
                    replay: json!({"counts": {"research.artifact.created": 2}}),
                })
            }
            _ => Err(format!("unknown demo: {demo_id}")),
        }
    }
}

pub fn demo_descriptors() -> Vec<DemoDescriptor> {
    vec![
        DemoDescriptor {
            id: "local-simbroker-paper",
            title: "Local SimBroker paper order",
            summary: "Seeds the options research demo, activates it with local acknowledgements, and submits through SimBroker.",
            asset_classes: vec!["option", "equity"],
            proof: "paper_order",
            requires_credentials: false,
            live_capable: false,
            excluded_reason: None,
        },
        DemoDescriptor {
            id: "local-simbroker-paper-btc",
            title: "Local SimBroker BTC entry/exit",
            summary: "Seeds BTC/USD, runs a local backtest, submits a deterministic SimBroker paper entry, triggers synthetic exit, and replays the journal.",
            asset_classes: vec!["crypto"],
            proof: "local_simbroker_paper",
            requires_credentials: false,
            live_capable: false,
            excluded_reason: None,
        },
        DemoDescriptor {
            id: "fixture-marketdata-backtest",
            title: "Fixture market-data backtest",
            summary: "Creates replayable BTC fixture bars and persists deterministic backtest metrics and artifacts.",
            asset_classes: vec!["crypto"],
            proof: "fixture_marketdata",
            requires_credentials: false,
            live_capable: false,
            excluded_reason: None,
        },
    ]
}

pub fn runnable_demo_descriptors() -> Vec<DemoDescriptor> {
    demo_descriptors()
        .into_iter()
        .filter(|item| item.excluded_reason.is_none())
        .collect()
}

pub fn adapter_router_run() -> Value {
    let result = DemoRunner
        .run("local-simbroker-paper")
        .expect("local demo is bundled");
    json!({
        "status": result.status,
        "demo_id": result.demo_id,
        "order_id": result.summary["order_id"],
        "snapshot": true
    })
}

pub fn paper_e2e_run(use_options: bool, use_live: bool) -> Result<Value, String> {
    if use_options || use_live {
        return Err("outside the FOSS proof catalog".to_string());
    }
    let result = DemoRunner
        .run("local-simbroker-paper-btc")
        .expect("local simulator demo is bundled");
    Ok(json!({
        "status": result.status,
        "demo_id": result.demo_id,
        "order_id": result.summary["entry_order_id"],
        "exit_order_id": result.summary["exit_order_id"],
        "snapshot": true
    }))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FixtureBar {
    pub symbol: String,
    pub ts: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

pub fn generate_fixture_bars(
    symbols: &[&str],
    timeframe: &str,
    lookback: usize,
    seed_key: &str,
    asset_family: &str,
) -> BTreeMap<String, Vec<FixtureBar>> {
    let row_count = lookback.max(2);
    let mut seen = BTreeSet::new();
    let mut out = BTreeMap::new();
    for raw in symbols {
        let symbol = raw.trim().to_uppercase();
        if symbol.is_empty() || !seen.insert(symbol.clone()) {
            continue;
        }
        let mut previous = base_price(asset_family);
        let mut bars = Vec::with_capacity(row_count);
        for idx in 0..row_count {
            let seed = stable_unit(&format!(
                "{seed_key}:{asset_family}:{symbol}:{timeframe}:{lookback}:{idx}"
            ));
            let drift = 0.0003 + seed * 0.004;
            let close = previous * (1.0 + drift);
            let high = previous.max(close) * (1.001 + seed * 0.002);
            let low = previous.min(close) * (0.999 - seed * 0.001);
            bars.push(FixtureBar {
                symbol: symbol.clone(),
                ts: fixture_timestamp(timeframe, row_count, idx),
                open: round6(previous),
                high: round6(high),
                low: round6(low),
                close: round6(close),
                volume: (500_000.0 + seed * 75_000.0).round(),
            });
            previous = close;
        }
        out.insert(symbol, bars);
    }
    out
}

pub fn build_fixture_transport(_base_url: &str) -> bool {
    true
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrategySpec {
    pub name: &'static str,
    pub description: &'static str,
    pub needs_options: bool,
    pub needs_pairs: bool,
    pub needs_vol_inputs: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedOptionSymbol {
    pub symbol: String,
    pub root: String,
    pub expiry: String,
    pub cp: char,
    pub strike: f64,
}

pub fn canonical_strategy_name(name: &str) -> String {
    match name.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
        "long_stock_swing_trade" => "long_stock_swing".to_string(),
        "target_allocation_portfolio" => "target_allocation".to_string(),
        "target_allocation_plus_covered_call_overlay" => {
            "target_allocation_covered_call".to_string()
        }
        "cash_secured_put" => "cash_secured_put_program".to_string(),
        "protective_put" => "protective_put_hedge".to_string(),
        "pairs" => "pairs_trade".to_string(),
        "volatility_targeting_portfolio" => "volatility_targeting".to_string(),
        "long_straddle_event" => "long_straddle".to_string(),
        other => other.to_string(),
    }
}

pub fn resolve_strategies(requested: &[&str]) -> Result<Vec<StrategySpec>, String> {
    let requested = if requested.is_empty() {
        vec!["all"]
    } else {
        requested.to_vec()
    };
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for item in requested {
        let name = canonical_strategy_name(item);
        if name == "all" {
            for strategy in ALL_STRATEGY_NAMES {
                if seen.insert(strategy) {
                    out.push(strategy_spec(strategy).expect("known strategy"));
                }
            }
            continue;
        }
        let spec = strategy_spec(&name).ok_or_else(|| {
            format!(
                "Unknown strategy '{item}'. Known={}",
                ALL_STRATEGY_NAMES.join(",")
            )
        })?;
        if seen.insert(spec.name) {
            out.push(spec);
        }
    }
    Ok(out)
}

pub fn parse_option_symbol(symbol: &str) -> Option<ParsedOptionSymbol> {
    let sym = symbol.trim().to_uppercase();
    if sym.len() < 16 {
        return None;
    }
    let split = sym.len().checked_sub(15)?;
    let (root, tail) = sym.split_at(split);
    if root.is_empty()
        || root.len() > 6
        || !root.chars().all(|ch| ch.is_ascii_uppercase())
        || tail.len() != 15
    {
        return None;
    }
    let date = &tail[0..6];
    let cp = tail.as_bytes()[6] as char;
    let strike = &tail[7..15];
    if !date.chars().all(|ch| ch.is_ascii_digit())
        || !matches!(cp, 'C' | 'P')
        || !strike.chars().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    let month = date[2..4].parse::<u32>().ok()?;
    let day = date[4..6].parse::<u32>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let root = root.to_string();
    let expiry = format!("20{}-{}-{}", &date[0..2], &date[2..4], &date[4..6]);
    let strike = strike.parse::<f64>().ok()? / 1000.0;
    Some(ParsedOptionSymbol {
        symbol: sym,
        root,
        expiry,
        cp,
        strike,
    })
}

pub fn pick_option_contracts(
    symbols: &[&str],
    spot: f64,
) -> Result<BTreeMap<String, String>, String> {
    let mut parsed: Vec<_> = symbols
        .iter()
        .filter_map(|symbol| parse_option_symbol(symbol))
        .filter(|rec| expiry_sort_key(&rec.expiry) >= 20260624)
        .collect();
    if parsed.is_empty() {
        return Err("No valid future option contracts found for selection".to_string());
    }
    parsed.sort_by(|left, right| {
        (
            expiry_sort_key(&left.expiry),
            ((left.strike - spot).abs() * 1000.0) as i64,
            (left.strike * 1000.0) as i64,
        )
            .cmp(&(
                expiry_sort_key(&right.expiry),
                ((right.strike - spot).abs() * 1000.0) as i64,
                (right.strike * 1000.0) as i64,
            ))
    });
    let calls: Vec<_> = parsed
        .iter()
        .filter(|item| item.cp == 'C')
        .cloned()
        .collect();
    let puts: Vec<_> = parsed
        .iter()
        .filter(|item| item.cp == 'P')
        .cloned()
        .collect();
    if calls.is_empty() || puts.is_empty() {
        return Err("Need both call and put contracts to run strategy demos".to_string());
    }
    let call_atm = calls[0].clone();
    let put_atm = puts[0].clone();
    let call_otm = select_by_strike(&calls, call_atm.strike + 5.0);
    let put_otm = select_by_strike(&puts, put_atm.strike - 5.0);
    let call_far = calls
        .iter()
        .filter(|item| {
            item.strike == call_atm.strike
                && expiry_sort_key(&item.expiry) > expiry_sort_key(&call_atm.expiry)
        })
        .min_by_key(|item| expiry_sort_key(&item.expiry))
        .cloned()
        .unwrap_or_else(|| call_otm.clone());

    Ok(BTreeMap::from([
        ("call_atm".to_string(), call_atm.symbol),
        ("put_atm".to_string(), put_atm.symbol),
        ("call_otm".to_string(), call_otm.symbol),
        ("put_otm".to_string(), put_otm.symbol),
        ("call_far".to_string(), call_far.symbol),
    ]))
}

pub fn build_strategy_order_plan(
    strategy: &str,
    symbol: &str,
    option_contracts: &BTreeMap<String, String>,
) -> Result<Value, String> {
    let name = canonical_strategy_name(strategy);
    strategy_spec(&name).ok_or_else(|| format!("Unknown strategy '{strategy}'"))?;
    let call_atm = required_contract(option_contracts, "call_atm")?;
    let put_atm = required_contract(option_contracts, "put_atm")?;
    let call_otm = option_contracts
        .get("call_otm")
        .cloned()
        .unwrap_or_else(|| call_atm.clone());
    let put_otm = option_contracts
        .get("put_otm")
        .cloned()
        .unwrap_or_else(|| put_atm.clone());
    let call_far = option_contracts
        .get("call_far")
        .cloned()
        .unwrap_or_else(|| call_otm.clone());

    let (entry, exit) = match name.as_str() {
        "wheel" | "target_allocation_covered_call" => (
            vec![
                order_payload(symbol, "buy", 100),
                order_payload(&call_otm, "sell", 1),
            ],
            vec![
                order_payload(&call_otm, "buy", 1),
                order_payload(symbol, "sell", 100),
            ],
        ),
        "long_stock_swing" | "target_allocation" => (
            vec![order_payload(symbol, "buy", 1)],
            vec![order_payload(symbol, "sell", 1)],
        ),
        "cash_secured_put_program" => (
            vec![order_payload(&put_otm, "sell", 1)],
            vec![order_payload(&put_otm, "buy", 1)],
        ),
        "protective_put_hedge" => (
            vec![
                order_payload(symbol, "buy", 100),
                order_payload(&put_atm, "buy", 1),
            ],
            vec![
                order_payload(&put_atm, "sell", 1),
                order_payload(symbol, "sell", 100),
            ],
        ),
        "bull_call_spread" => (
            vec![
                order_payload(&call_atm, "buy", 1),
                order_payload(&call_otm, "sell", 1),
            ],
            vec![
                order_payload(&call_otm, "buy", 1),
                order_payload(&call_atm, "sell", 1),
            ],
        ),
        "iron_condor" => (
            vec![
                order_payload(&put_otm, "buy", 1),
                order_payload(&put_atm, "sell", 1),
                order_payload(&call_atm, "sell", 1),
                order_payload(&call_otm, "buy", 1),
            ],
            vec![
                order_payload(&put_otm, "sell", 1),
                order_payload(&put_atm, "buy", 1),
                order_payload(&call_atm, "buy", 1),
                order_payload(&call_otm, "sell", 1),
            ],
        ),
        "long_straddle" => (
            vec![
                order_payload(&call_atm, "buy", 1),
                order_payload(&put_atm, "buy", 1),
            ],
            vec![
                order_payload(&call_atm, "sell", 1),
                order_payload(&put_atm, "sell", 1),
            ],
        ),
        "calendar_spread" => (
            vec![
                order_payload(&call_atm, "sell", 1),
                order_payload(&call_far, "buy", 1),
            ],
            vec![
                order_payload(&call_atm, "buy", 1),
                order_payload(&call_far, "sell", 1),
            ],
        ),
        "pairs_trade" => (
            vec![
                order_payload("SPY", "buy", 1),
                order_payload("IVV", "sell", 1),
            ],
            vec![
                order_payload("SPY", "sell", 1),
                order_payload("IVV", "buy", 1),
            ],
        ),
        "volatility_targeting" => (
            vec![order_payload(symbol, "buy", 1)],
            vec![order_payload(symbol, "sell", 1)],
        ),
        "collar" => (
            vec![
                order_payload(symbol, "buy", 100),
                order_payload(&put_atm, "buy", 1),
                order_payload(&call_otm, "sell", 1),
            ],
            vec![
                order_payload(&call_otm, "buy", 1),
                order_payload(&put_atm, "sell", 1),
                order_payload(symbol, "sell", 100),
            ],
        ),
        _ => return Err(format!("Unhandled strategy '{name}'")),
    };
    Ok(json!({"entry": entry, "exit": exit}))
}

pub fn extract_mid_price(payload: &Value) -> Option<f64> {
    let quote = payload.get("quote")?;
    let bid = quote.get("bp").and_then(parse_json_f64);
    let ask = quote.get("ap").and_then(parse_json_f64);
    match (bid, ask) {
        (Some(left), Some(right)) => Some((left + right) / 2.0),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllocationArgs {
    pub allocation_symbols: String,
    pub allocation_weights: String,
    pub allocation_tolerance_pct: f64,
    pub allocation_hysteresis_pct: f64,
    pub allocation_min_trade_notional: f64,
    pub allocation_allow_fractional: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllocationConfig {
    pub symbols: Vec<String>,
    pub weights: Vec<f64>,
    pub tolerance_pct: f64,
    pub hysteresis_pct: f64,
    pub min_trade_notional: f64,
    pub allow_fractional: bool,
}

pub fn parse_allocation_config(args: AllocationArgs) -> Result<AllocationConfig, String> {
    let symbols: Vec<_> = parse_csv(&args.allocation_symbols)
        .into_iter()
        .map(|symbol| symbol.to_uppercase())
        .collect();
    let weights: Result<Vec<_>, _> = parse_csv(&args.allocation_weights)
        .into_iter()
        .map(|item| item.parse::<f64>())
        .collect();
    let weights = weights.map_err(|err| err.to_string())?;
    if symbols.is_empty() {
        return Err("allocation_symbols cannot be empty".to_string());
    }
    if symbols.len() != weights.len() {
        return Err(
            "allocation_symbols and allocation_weights must have the same length".to_string(),
        );
    }
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return Err("allocation_weights must sum to a positive value".to_string());
    }
    Ok(AllocationConfig {
        symbols,
        weights: weights.into_iter().map(|weight| weight / total).collect(),
        tolerance_pct: args.allocation_tolerance_pct,
        hysteresis_pct: args.allocation_hysteresis_pct,
        min_trade_notional: args.allocation_min_trade_notional,
        allow_fractional: args.allocation_allow_fractional,
    })
}

pub fn market_open_from_clock_payload(body: &Value) -> (bool, String) {
    if let Some(is_open) = body.get("is_open").and_then(Value::as_bool) {
        let ts = body
            .get("timestamp")
            .or_else(|| body.get("next_open"))
            .or_else(|| body.get("next_close"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return (is_open, ts.to_string());
    }
    if let Some(clocks) = body.get("clocks").and_then(Value::as_array) {
        for item in clocks {
            let acronym = item
                .get("market")
                .and_then(|market| market.get("acronym"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_uppercase();
            let phase = item
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if matches!(acronym.as_str(), "NYSE" | "NASDAQ" | "OPRA" | "IEX") && phase == "core" {
                return (
                    true,
                    item.get("timestamp")
                        .or_else(|| item.get("next_market_open"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                );
            }
        }
        let ts = clocks
            .first()
            .and_then(|item| item.get("timestamp"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return (false, ts.to_string());
    }
    (
        false,
        body.get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    )
}

pub fn options_exercise_acceptance_payload(live_requested: bool) -> Value {
    json!({
        "status": "passed",
        "source_e2e": "runtime-rs/tests/demos.rs::options_acceptance_payloads_are_mocked_and_side_effect_free",
        "mode": if live_requested { "real-provider-gated" } else { "mock" },
        "operations": {
            "optionExercise": ["trading_options_exercise", "mutating", "safe", "enabled_mock", "mocked exercise proof"],
            "optionDoNotExercise": ["trading_options_exercise", "mutating", "safe", "enabled_mock", "mocked do-not-exercise proof"]
        },
        "live_requested": live_requested,
        "orders_submitted": 0
    })
}

pub fn options_strategies_acceptance_payload(
    strategies: &[&str],
    live_requested: bool,
) -> Result<Value, String> {
    let selected = resolve_strategies(strategies)?;
    let contracts = pick_option_contracts(
        &[
            "SPY300118C00450000",
            "SPY300118P00450000",
            "SPY300118C00455000",
            "SPY300118P00445000",
            "SPY300219C00450000",
        ],
        450.0,
    )?;
    let mut names = Vec::with_capacity(selected.len());
    for spec in selected {
        build_strategy_order_plan(spec.name, "SPY", &contracts)?;
        names.push(spec.name);
    }
    names.sort();
    Ok(json!({
        "status": "passed",
        "source_e2e": "runtime-rs/tests/demos.rs::options_acceptance_payloads_are_mocked_and_side_effect_free",
        "mode": if live_requested { "real-provider-gated" } else { "mock" },
        "strategy_count": names.len(),
        "strategies": names,
        "live_requested": live_requested,
        "orders_submitted": 0
    }))
}

fn strategy_spec(name: &str) -> Option<StrategySpec> {
    match name {
        "wheel" => Some(StrategySpec {
            name: "wheel",
            description: "Cash-secured put / covered-call wheel demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "long_stock_swing" => Some(StrategySpec {
            name: "long_stock_swing",
            description: "Long stock swing demo using SMA cross checks",
            needs_options: false,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "target_allocation" => Some(StrategySpec {
            name: "target_allocation",
            description: "Target allocation rebalance demo",
            needs_options: false,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "target_allocation_covered_call" => Some(StrategySpec {
            name: "target_allocation_covered_call",
            description: "Target allocation with covered call overlay",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "cash_secured_put_program" => Some(StrategySpec {
            name: "cash_secured_put_program",
            description: "Systematic premium sell demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "protective_put_hedge" => Some(StrategySpec {
            name: "protective_put_hedge",
            description: "Protective put hedge demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "bull_call_spread" => Some(StrategySpec {
            name: "bull_call_spread",
            description: "Bull call spread demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "iron_condor" => Some(StrategySpec {
            name: "iron_condor",
            description: "Iron condor demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "long_straddle" => Some(StrategySpec {
            name: "long_straddle",
            description: "Long straddle demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "calendar_spread" => Some(StrategySpec {
            name: "calendar_spread",
            description: "Calendar spread demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        "pairs_trade" => Some(StrategySpec {
            name: "pairs_trade",
            description: "Beta-neutral pairs trade demo",
            needs_options: false,
            needs_pairs: true,
            needs_vol_inputs: false,
        }),
        "volatility_targeting" => Some(StrategySpec {
            name: "volatility_targeting",
            description: "Volatility-targeted SPY exposure demo",
            needs_options: false,
            needs_pairs: false,
            needs_vol_inputs: true,
        }),
        "collar" => Some(StrategySpec {
            name: "collar",
            description: "Collar strategy demo",
            needs_options: true,
            needs_pairs: false,
            needs_vol_inputs: false,
        }),
        _ => None,
    }
}

fn order_payload(symbol: &str, side: &str, qty: u32) -> Value {
    json!({
        "symbol": symbol,
        "side": side,
        "qty": qty.to_string(),
        "type": "market",
        "time_in_force": "day"
    })
}

fn required_contract(contracts: &BTreeMap<String, String>, key: &str) -> Result<String, String> {
    contracts
        .get(key)
        .cloned()
        .ok_or_else(|| "Missing required option contracts for strategy planning".to_string())
}

fn select_by_strike(rows: &[ParsedOptionSymbol], target: f64) -> ParsedOptionSymbol {
    rows.iter()
        .min_by(|left, right| {
            (
                ((left.strike - target).abs() * 1000.0) as i64,
                expiry_sort_key(&left.expiry),
                (left.strike * 1000.0) as i64,
            )
                .cmp(&(
                    ((right.strike - target).abs() * 1000.0) as i64,
                    expiry_sort_key(&right.expiry),
                    (right.strike * 1000.0) as i64,
                ))
        })
        .expect("non-empty option list")
        .clone()
}

fn parse_json_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<f64>().ok()))
}

fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn expiry_sort_key(expiry: &str) -> u32 {
    expiry
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or_default()
}

fn fixture_timestamp(timeframe: &str, row_count: usize, idx: usize) -> String {
    let day_offset = row_count - idx - 1;
    match timeframe {
        "1m" | "1Min" => format!(
            "2025-12-31T19:{:02}:00Z",
            59usize.saturating_sub(day_offset)
        ),
        "5m" | "5Min" => format!(
            "2025-12-31T{:02}:00:00Z",
            20usize.saturating_sub(day_offset)
        ),
        "1h" | "1Hour" => format!(
            "2025-12-{:02}T20:00:00Z",
            31usize.saturating_sub(day_offset)
        ),
        _ => format!(
            "2025-12-{:02}T20:00:00Z",
            31usize.saturating_sub(day_offset)
        ),
    }
}

fn base_price(asset_family: &str) -> f64 {
    match asset_family.trim().to_lowercase().as_str() {
        "crypto" => 45_000.0,
        "option" => 6.5,
        "future" => 4_800.0,
        "fx" => 1.08,
        "event_contract" => 0.58,
        "fund" => 120.0,
        _ => 100.0,
    }
}

fn stable_unit(seed: &str) -> f64 {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let value = u64::from_be_bytes(bytes);
    value as f64 / u64::MAX as f64
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn dataset_hash(bars: &BTreeMap<String, Vec<FixtureBar>>) -> String {
    let mut hasher = Sha256::new();
    for (symbol, rows) in bars {
        hasher.update(symbol.as_bytes());
        for row in rows {
            hasher.update(row.ts.as_bytes());
            hasher.update(row.close.to_string().as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}
