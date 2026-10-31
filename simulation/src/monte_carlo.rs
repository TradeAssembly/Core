// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::TAU;

use serde_json::{json, Map, Value};

const DEFAULT_START_UNIX_SECONDS: i64 = 1_735_689_600;
const MONTE_CARLO_CONTRACT_VERSION: &str = "tradeassembly.monte_carlo.contract.v1";

#[derive(Clone, Debug, PartialEq)]
pub struct MonteCarloConfig {
    pub seed: i64,
    pub start_unix_seconds: i64,
    pub interval_seconds: i64,
    pub drift: f64,
    pub volatility: f64,
    pub start_prices: BTreeMap<String, f64>,
    pub default_price: f64,
}

impl Default for MonteCarloConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            start_unix_seconds: DEFAULT_START_UNIX_SECONDS,
            interval_seconds: 60,
            drift: 0.0,
            volatility: 0.02,
            start_prices: BTreeMap::new(),
            default_price: 100.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataCapabilities {
    pub instruments_supported: Vec<String>,
    pub supports_quotes: bool,
    pub supports_bars: bool,
    pub supports_chains: bool,
    pub supports_greeks: bool,
    pub historical_granularities: Vec<String>,
    pub max_symbols_per_call: usize,
    pub streaming_support: Vec<String>,
    pub indicators_supported: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    pub symbol: String,
    pub last: f64,
    pub ts: i64,
    pub provider: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub symbol: String,
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonteCarloMarketData {
    config: MonteCarloConfig,
    max_steps: Option<usize>,
    step: usize,
    rngs: BTreeMap<String, NormalRng>,
    prices: BTreeMap<String, f64>,
}

impl MonteCarloMarketData {
    pub fn new(config: MonteCarloConfig, max_steps: Option<usize>) -> Self {
        let prices = config.start_prices.clone();
        Self {
            config,
            max_steps,
            step: 0,
            rngs: BTreeMap::new(),
            prices,
        }
    }

    pub fn capabilities(&self) -> DataCapabilities {
        DataCapabilities {
            instruments_supported: vec!["EQUITY".to_string(), "CRYPTO".to_string()],
            supports_quotes: true,
            supports_bars: true,
            supports_chains: false,
            supports_greeks: false,
            historical_granularities: vec!["1m".to_string()],
            max_symbols_per_call: 100,
            streaming_support: Vec::new(),
            indicators_supported: Vec::new(),
        }
    }

    pub fn get_quotes(&mut self, symbols: &[&str]) -> Result<BTreeMap<String, Quote>, String> {
        if self
            .max_steps
            .is_some_and(|max_steps| self.step >= max_steps)
        {
            return Err("Monte Carlo data exhausted for configured steps".to_string());
        }
        let ts = self.ts_for_step(self.step);
        let mut quotes = BTreeMap::new();
        for symbol in sorted_symbols(symbols) {
            let last = self.next_price(&symbol);
            quotes.insert(
                symbol.clone(),
                Quote {
                    symbol,
                    last,
                    ts,
                    provider: "monte_carlo".to_string(),
                },
            );
        }
        self.step += 1;
        Ok(quotes)
    }

    pub fn get_bars(
        &self,
        symbols: &[&str],
        lookback: usize,
    ) -> Result<BTreeMap<String, Vec<Bar>>, String> {
        if lookback == 0 {
            return Ok(sorted_symbols(symbols)
                .into_iter()
                .map(|symbol| (symbol, Vec::new()))
                .collect());
        }
        let mut bars_by_symbol = BTreeMap::new();
        for symbol in sorted_symbols(symbols) {
            let mut rng = NormalRng::new(stable_seed(self.config.seed, &symbol, "bars"));
            let mut price = self
                .config
                .start_prices
                .get(&symbol)
                .copied()
                .unwrap_or(self.config.default_price);
            let mut bars = Vec::new();
            for index in 0..lookback {
                let open = price;
                let shock = rng.gauss();
                let close = open
                    * ((self.config.drift - 0.5 * self.config.volatility.powi(2))
                        + self.config.volatility * shock)
                        .exp();
                let high = open.max(close) * (1.0 + rng.gauss().abs() * 0.02);
                let low = open.min(close) * (1.0 - rng.gauss().abs() * 0.02);
                bars.push(Bar {
                    symbol: symbol.clone(),
                    ts: self.ts_for_step(index),
                    open,
                    high,
                    low: low.max(0.01),
                    close: close.max(0.01),
                });
                price = close.max(0.01);
            }
            bars_by_symbol.insert(symbol, bars);
        }
        Ok(bars_by_symbol)
    }

    pub fn get_option_chain(&self, _underlying: &str) -> Result<(), String> {
        Err("Monte Carlo data source does not support option chains".to_string())
    }

    pub fn get_option_quotes(&self) -> Result<(), String> {
        Err("Monte Carlo data source does not support option quotes".to_string())
    }

    pub fn get_greeks(&self) -> Result<(), String> {
        Err("Monte Carlo data source does not support greeks".to_string())
    }

    pub fn get_indicators(&self) -> Result<(), String> {
        Err("Monte Carlo data source does not support indicators".to_string())
    }

    fn next_price(&mut self, symbol: &str) -> f64 {
        let price = self
            .prices
            .get(symbol)
            .copied()
            .unwrap_or(self.config.default_price);
        let shock = self.rng_for_symbol(symbol).gauss();
        let next_price = price
            * ((self.config.drift - 0.5 * self.config.volatility.powi(2))
                + self.config.volatility * shock)
                .exp();
        let next_price = next_price.max(0.01);
        self.prices.insert(symbol.to_string(), next_price);
        next_price
    }

    fn rng_for_symbol(&mut self, symbol: &str) -> &mut NormalRng {
        self.rngs
            .entry(symbol.to_string())
            .or_insert_with(|| NormalRng::new(stable_seed(self.config.seed, symbol, "quotes")))
    }

    fn ts_for_step(&self, step: usize) -> i64 {
        self.config.start_unix_seconds + self.config.interval_seconds * step as i64
    }
}

#[derive(Clone, Debug, PartialEq)]
struct NormalRng {
    state: u64,
    spare: Option<f64>,
}

impl NormalRng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed | 1,
            spare: None,
        }
    }

    fn gauss(&mut self) -> f64 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let u1 = self.next_f64().max(f64::MIN_POSITIVE);
        let u2 = self.next_f64();
        let radius = (-2.0 * u1.ln()).sqrt();
        let theta = TAU * u2;
        self.spare = Some(radius * theta.sin());
        radius * theta.cos()
    }

    fn next_f64(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.state >> 11) as f64) / ((1_u64 << 53) as f64)
    }
}

fn sorted_symbols(symbols: &[&str]) -> Vec<String> {
    symbols
        .iter()
        .map(|symbol| symbol.trim())
        .filter(|symbol| !symbol.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn stable_seed(base_seed: i64, symbol: &str, tag: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(format!("{tag}:{symbol}:{base_seed}").as_bytes());
    let digest = hasher.finalize();
    let first_48_bits = digest[..6]
        .iter()
        .fold(0_u64, |acc, byte| (acc << 8) | *byte as u64);
    first_48_bits % (1_u64 << 32)
}

pub fn monte_carlo_contract(input: &Value) -> Value {
    let assumptions = input
        .get("assumptions")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let (sanitized_assumptions, mut warnings) = sanitize_value(&assumptions, "");
    let ledger = assumption_ledger(&sanitized_assumptions);
    validate_assumptions(&ledger, input, &mut warnings);
    warnings.sort_by(|left, right| {
        warning_sort_key(left)
            .cmp(&warning_sort_key(right))
            .then_with(|| left.to_string().cmp(&right.to_string()))
    });
    warnings.dedup();
    let ok = warnings.is_empty();
    let strategy_id = str_field(input, "strategyId")
        .or_else(|| str_field(input, "strategy_id"))
        .unwrap_or("strategy-local");
    let assumption_hash = ledger["assumptionHash"]
        .as_str()
        .unwrap_or("sha256:missing")
        .to_string();
    let seed = ledger["seed"].as_i64().unwrap_or(0);
    let path_count = ledger["pathCount"].as_u64().unwrap_or(0);
    let horizon_days = ledger["horizonDays"].as_u64().unwrap_or(0);
    let request = simulation_request(input, &ledger, strategy_id, &assumption_hash);

    json!({
        "ok": ok,
        "kind": "monte_carlo_contract",
        "schemaVersion": MONTE_CARLO_CONTRACT_VERSION,
        "strategyId": strategy_id,
        "assumptionLedger": ledger,
        "simulationRequest": request,
        "outputs": {
            "assumptionHash": assumption_hash,
            "randomSeed": seed,
            "pathSummary": {
                "pathCount": path_count,
                "horizonDays": horizon_days,
                "cadence": ledger["cadence"].clone(),
                "summaryOnly": true
            },
            "drawdownDistribution": {"kind": "pending_simulation", "paths": []},
            "terminalDistribution": {"kind": "pending_simulation", "paths": []},
            "confidenceBands": {"kind": "pending_simulation", "bands": []},
            "unsupportedInputs": warnings.iter()
                .filter_map(|warning| warning.get("code").and_then(Value::as_str))
                .collect::<Vec<_>>(),
        },
        "warnings": warnings,
        "replayRefs": {
            "assumptionHash": assumption_hash,
            "commands": [
                format!("tradeassembly monte-carlo replay --strategy-id {strategy_id} --assumption-hash {assumption_hash}")
            ],
            "json": {"kind": "json", "path": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/request.json")},
            "ledger": {"kind": "json", "path": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/assumptions.json")}
        },
        "canonicalRequest": canonical_json(&json!({"strategyId": strategy_id, "assumptions": ledger, "request": request})),
        "notice": "Monte Carlo output is research-only sensitivity analysis. TradeAssembly does not tell users what to trade, when to trade, or how much to trade."
    })
}

pub fn monte_carlo_simulation(input: &Value) -> Value {
    let contract = monte_carlo_contract(input);
    let ledger = contract["assumptionLedger"].clone();
    let strategy_id = contract["strategyId"].as_str().unwrap_or("strategy-local");
    let assumption_hash = ledger["assumptionHash"]
        .as_str()
        .unwrap_or("sha256:missing")
        .to_string();
    let mode = str_field(input, "mode").unwrap_or("strategy_returns");
    let mut warnings = contract["warnings"].as_array().cloned().unwrap_or_default();

    if let Some(expected_hash) = str_field(input, "expectedAssumptionHash")
        .or_else(|| str_field(input, "expected_assumption_hash"))
    {
        if expected_hash != assumption_hash {
            warnings.push(warning(
                "assumption_hash_mismatch",
                "Expected assumption hash does not match the canonical ledger hash.",
            ));
        }
    }
    if !matches!(mode, "strategy_returns" | "position_valuation") {
        warnings.push(warning(
            "unsupported_simulation_mode",
            "Monte Carlo simulation mode must be strategy_returns or position_valuation.",
        ));
    }
    if ledger["distribution"] == "historical_bootstrap" && return_series(input).len() < 2 {
        warnings.push(warning(
            "insufficient_data",
            "Historical bootstrap simulation requires at least two deterministic return observations.",
        ));
    }
    if mode == "position_valuation" && valuation_start(input).is_none() {
        warnings.push(warning(
            "missing_valuation_inputs",
            "Position valuation mode requires valuationInputs.startValue.",
        ));
    }

    sort_warnings(&mut warnings);
    let ok = warnings.is_empty();
    let simulation_result = if ok {
        simulate_paths(input, &ledger, mode, &assumption_hash)
    } else {
        json!({
            "mode": mode,
            "skipped": true,
            "reason": "fail_closed",
            "assumptionHash": assumption_hash,
            "pathSummary": {"pathCount": ledger["pathCount"].clone(), "horizonDays": ledger["horizonDays"].clone()}
        })
    };
    let result_hash = format!(
        "sha256:{}",
        sha256_hex(canonical_json(&simulation_result).as_bytes())
    );
    let replay_refs = simulation_replay_refs(strategy_id, &assumption_hash);

    json!({
        "ok": ok,
        "kind": "monte_carlo_simulation",
        "schemaVersion": "tradeassembly.monte_carlo.result.v1",
        "strategyId": strategy_id,
        "assumptionLedger": ledger,
        "simulationRequest": contract["simulationRequest"].clone(),
        "simulationResult": simulation_result,
        "warnings": warnings,
        "journalEvidence": [
            simulation_journal_event(strategy_id, &assumption_hash, &result_hash, ok)
        ],
        "exportRefs": {
            "resultJson": {"kind": "json", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/result.json"), "contentHash": result_hash},
            "summaryCsv": {"kind": "csv", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/summary.csv")},
            "bandsJson": {"kind": "json", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/confidence-bands.json")}
        },
        "replayRefs": replay_refs,
        "contract": contract,
        "notice": "Monte Carlo output is research-only sensitivity analysis. TradeAssembly does not tell users what to trade, when to trade, or how much to trade."
    })
}

fn simulate_paths(input: &Value, ledger: &Value, mode: &str, assumption_hash: &str) -> Value {
    let path_count = ledger["pathCount"].as_u64().unwrap_or(0) as usize;
    let horizon_days = ledger["horizonDays"].as_u64().unwrap_or(0) as usize;
    let seed = ledger["seed"].as_i64().unwrap_or(0);
    let distribution = ledger["distribution"].as_str().unwrap_or("lognormal");
    let historical_returns = return_series(input);
    let annualized_mean = nested_f64(ledger, &["returnInput", "annualizedMean"])
        .or_else(|| nested_f64(ledger, &["returnInput", "mean"]))
        .unwrap_or_else(|| mean(&historical_returns) * 252.0);
    let annualized_volatility = nested_f64(ledger, &["volatilityInput", "annualizedVolatility"])
        .or_else(|| nested_f64(ledger, &["volatilityInput", "volatility"]))
        .unwrap_or_else(|| sample_stddev(&historical_returns) * 252.0_f64.sqrt());
    let daily_mean = annualized_mean / 252.0;
    let daily_volatility = annualized_volatility / 252.0_f64.sqrt();
    let cost_drag =
        nested_f64(ledger, &["costAssumptions", "slippageBps"]).unwrap_or(0.0) / 10_000.0;
    let checkpoint_days = confidence_checkpoint_days(horizon_days);
    let mut checkpoint_values = vec![Vec::new(); checkpoint_days.len()];
    let mut terminal_returns = Vec::with_capacity(path_count);
    let mut terminal_values = Vec::with_capacity(path_count);
    let mut drawdowns = Vec::with_capacity(path_count);
    let start_value = valuation_start(input).unwrap_or(1.0);
    let return_multiplier =
        nested_f64(input, &["valuationInputs", "returnMultiplier"]).unwrap_or(1.0);

    for path_index in 0..path_count {
        let mut rng = NormalRng::new(stable_path_seed(seed, assumption_hash, path_index));
        let mut wealth: f64 = 1.0;
        let mut peak: f64 = 1.0;
        let mut max_drawdown: f64 = 0.0;
        for day in 0..horizon_days {
            let step_return = simulation_step_return(
                distribution,
                daily_mean,
                daily_volatility,
                &historical_returns,
                &mut rng,
            );
            wealth = (wealth * (1.0 + step_return)).max(0.0);
            peak = peak.max(wealth);
            if peak > 0.0 {
                let drawdown = ((peak - wealth) / peak).max(0.0);
                if drawdown > max_drawdown {
                    max_drawdown = drawdown;
                }
            }
            if let Some(checkpoint_index) =
                checkpoint_days.iter().position(|value| *value == day + 1)
            {
                checkpoint_values[checkpoint_index].push(project_value(
                    mode,
                    start_value,
                    return_multiplier,
                    wealth - 1.0,
                ));
            }
        }
        let terminal_return = wealth - 1.0 - cost_drag;
        terminal_returns.push(terminal_return);
        terminal_values.push(project_value(
            mode,
            start_value,
            return_multiplier,
            terminal_return,
        ));
        drawdowns.push(max_drawdown);
    }

    json!({
        "mode": mode,
        "assumptionHash": assumption_hash,
        "pathSummary": {
            "pathCount": path_count,
            "horizonDays": horizon_days,
            "seed": seed,
            "meanTerminalReturn": round6(mean(&terminal_returns)),
            "meanMaxDrawdown": round6(mean(&drawdowns)),
            "summaryOnly": true
        },
        "terminalDistribution": distribution_summary(&terminal_values),
        "terminalReturnDistribution": distribution_summary(&terminal_returns),
        "drawdownDistribution": distribution_summary(&drawdowns),
        "confidenceBands": confidence_bands(&checkpoint_days, checkpoint_values),
        "scenarioSensitivityRefs": scenario_sensitivity_refs(input),
        "assumptionLedgerRef": {"kind": "json", "uri": format!("tradeassembly://monte-carlo/local/{assumption_hash}/assumptions.json")},
        "warnings": []
    })
}

fn simulation_step_return(
    distribution: &str,
    daily_mean: f64,
    daily_volatility: f64,
    historical_returns: &[f64],
    rng: &mut NormalRng,
) -> f64 {
    match distribution {
        "historical_bootstrap" if !historical_returns.is_empty() => {
            let index = ((rng.next_f64() * historical_returns.len() as f64).floor() as usize)
                .min(historical_returns.len() - 1);
            historical_returns[index]
        }
        "student_t" => daily_mean + daily_volatility * rng.gauss() * 1.25,
        _ => {
            (daily_mean - 0.5 * daily_volatility.powi(2) + daily_volatility * rng.gauss()).exp()
                - 1.0
        }
    }
}

fn project_value(
    mode: &str,
    start_value: f64,
    return_multiplier: f64,
    terminal_return: f64,
) -> f64 {
    if mode == "position_valuation" {
        (start_value * (1.0 + terminal_return * return_multiplier)).max(0.0)
    } else {
        terminal_return
    }
}

fn confidence_checkpoint_days(horizon_days: usize) -> Vec<usize> {
    if horizon_days == 0 {
        return Vec::new();
    }
    [1, horizon_days.div_ceil(2), horizon_days]
        .into_iter()
        .filter(|day| *day > 0 && *day <= horizon_days)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn confidence_bands(days: &[usize], values: Vec<Vec<f64>>) -> Value {
    Value::Array(
        days.iter()
            .zip(values)
            .map(|(day, mut day_values)| {
                json!({
                    "day": day,
                    "p05": round6(quantile(&mut day_values, 0.05)),
                    "p50": round6(quantile(&mut day_values, 0.50)),
                    "p95": round6(quantile(&mut day_values, 0.95))
                })
            })
            .collect(),
    )
}

fn distribution_summary(values: &[f64]) -> Value {
    let mut p05 = values.to_vec();
    let mut p50 = values.to_vec();
    let mut p95 = values.to_vec();
    json!({
        "count": values.len(),
        "min": round6(values.iter().copied().fold(f64::INFINITY, f64::min)),
        "max": round6(values.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
        "mean": round6(mean(values)),
        "p05": round6(quantile(&mut p05, 0.05)),
        "p50": round6(quantile(&mut p50, 0.50)),
        "p95": round6(quantile(&mut p95, 0.95))
    })
}

fn quantile(values: &mut [f64], percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[index.min(values.len() - 1)]
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn sample_stddev(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let average = mean(values);
    let variance = values
        .iter()
        .map(|value| (value - average).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    variance.sqrt()
}

fn round6(value: f64) -> f64 {
    if value.is_finite() {
        (value * 1_000_000.0).round() / 1_000_000.0
    } else {
        0.0
    }
}

fn return_series(input: &Value) -> Vec<f64> {
    input
        .get("dataSnapshot")
        .and_then(|snapshot| {
            snapshot
                .get("returns")
                .or_else(|| snapshot.get("strategyReturns"))
        })
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

fn valuation_start(input: &Value) -> Option<f64> {
    nested_f64(input, &["valuationInputs", "startValue"])
}

fn scenario_sensitivity_refs(input: &Value) -> Value {
    let mut refs = Vec::new();
    if let Some(reference) = nested_str(input, &["valuationInputs", "scenarioSensitivityRef"]) {
        refs.push(json!(reference));
    }
    if let Some(reference) = str_field(input, "scenarioValuationRef") {
        refs.push(json!(reference));
    }
    Value::Array(refs)
}

fn simulation_replay_refs(strategy_id: &str, assumption_hash: &str) -> Value {
    json!({
        "assumptionHash": assumption_hash,
        "commands": [
            format!("tradeassembly monte-carlo replay --strategy-id {strategy_id} --assumption-hash {assumption_hash}")
        ],
        "result": {"kind": "json", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/result.json")},
        "ledger": {"kind": "json", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/assumptions.json")}
    })
}

fn simulation_journal_event(
    strategy_id: &str,
    assumption_hash: &str,
    result_hash: &str,
    ok: bool,
) -> Value {
    let payload = json!({
        "eventType": "monte_carlo.simulation.completed",
        "strategyId": strategy_id,
        "assumptionHash": assumption_hash,
        "resultHash": result_hash,
        "ok": ok,
        "storage": "local_journal_only"
    });
    json!({
        "eventType": "monte_carlo.simulation.completed",
        "payload": payload,
        "eventHash": format!("sha256:{}", sha256_hex(canonical_json(&payload).as_bytes())),
        "sideEffects": ["local_journal"]
    })
}

fn stable_path_seed(base_seed: i64, assumption_hash: &str, path_index: usize) -> u64 {
    let mut hasher = Sha256::new();
    hasher
        .update(format!("monte-carlo-path:{base_seed}:{assumption_hash}:{path_index}").as_bytes());
    let digest = hasher.finalize();
    digest[..8]
        .iter()
        .fold(0_u64, |acc, byte| (acc << 8) | *byte as u64)
        | 1
}

fn assumption_ledger(assumptions: &Value) -> Value {
    let return_input = assumptions
        .get("returnInput")
        .or_else(|| assumptions.get("return_input"))
        .cloned()
        .unwrap_or_else(|| json!(null));
    let volatility_input = assumptions
        .get("volatilityInput")
        .or_else(|| assumptions.get("volatility_input"))
        .cloned()
        .unwrap_or_else(|| json!(null));
    let correlation_input = assumptions
        .get("correlationInput")
        .or_else(|| assumptions.get("correlation_input"))
        .cloned()
        .unwrap_or_else(|| json!(null));
    let cadence = assumptions
        .get("cadence")
        .cloned()
        .unwrap_or_else(|| json!(null));
    let cost_assumptions = assumptions
        .get("costAssumptions")
        .or_else(|| assumptions.get("cost_assumptions"))
        .cloned()
        .unwrap_or_else(|| json!(null));
    let data_window = assumptions
        .get("dataWindow")
        .or_else(|| assumptions.get("data_window"))
        .cloned()
        .unwrap_or_else(|| json!(null));
    let source_provenance = assumptions
        .get("sourceProvenance")
        .or_else(|| assumptions.get("source_provenance"))
        .cloned()
        .unwrap_or_else(|| json!(null));
    let core = json!({
        "schemaVersion": MONTE_CARLO_CONTRACT_VERSION,
        "distribution": str_field_value(assumptions, "distribution", "distribution"),
        "returnInput": return_input,
        "volatilityInput": volatility_input,
        "correlationInput": correlation_input,
        "pathCount": int_field_value(assumptions, "pathCount", "path_count"),
        "seed": int_field_value(assumptions, "seed", "seed"),
        "horizonDays": int_field_value(assumptions, "horizonDays", "horizon_days"),
        "cadence": cadence,
        "costAssumptions": cost_assumptions,
        "dataWindow": data_window,
        "sourceProvenance": source_provenance,
        "modelVersion": str_field_value(assumptions, "modelVersion", "model_version"),
    });
    let assumption_hash = format!("sha256:{}", sha256_hex(canonical_json(&core).as_bytes()));
    let mut object = core.as_object().cloned().unwrap_or_default();
    object.insert("assumptionHash".to_string(), json!(assumption_hash));
    Value::Object(object)
}

fn simulation_request(
    input: &Value,
    ledger: &Value,
    strategy_id: &str,
    assumption_hash: &str,
) -> Value {
    json!({
        "strategyId": strategy_id,
        "backtestId": str_field(input, "backtestId").or_else(|| str_field(input, "backtest_id")).unwrap_or(""),
        "checkpointId": str_field(input, "checkpointId").or_else(|| str_field(input, "checkpoint_id")).unwrap_or(""),
        "scenarioValuationRef": str_field(input, "scenarioValuationRef").or_else(|| str_field(input, "scenario_valuation_ref")).unwrap_or(""),
        "assumptionHash": assumption_hash,
        "pathCount": ledger["pathCount"].clone(),
        "seed": ledger["seed"].clone(),
        "horizonDays": ledger["horizonDays"].clone(),
        "cadence": ledger["cadence"].clone(),
        "costAssumptions": ledger["costAssumptions"].clone(),
        "dataWindow": ledger["dataWindow"].clone(),
        "sourceProvenance": ledger["sourceProvenance"].clone(),
        "modelVersion": ledger["modelVersion"].clone(),
    })
}

fn validate_assumptions(ledger: &Value, input: &Value, warnings: &mut Vec<Value>) {
    for field in [
        "distribution",
        "returnInput",
        "volatilityInput",
        "correlationInput",
        "pathCount",
        "seed",
        "horizonDays",
        "cadence",
        "costAssumptions",
        "dataWindow",
        "sourceProvenance",
        "modelVersion",
    ] {
        if missing_ledger_field(ledger, field) {
            warnings.push(warning(
                "missing_assumption",
                &format!("Monte Carlo assumption `{field}` is required."),
            ));
        }
    }
    let distribution = ledger["distribution"].as_str().unwrap_or("");
    if !["lognormal", "historical_bootstrap", "student_t"].contains(&distribution) {
        warnings.push(warning(
            "unsupported_distribution",
            "Distribution must be lognormal, historical_bootstrap, or student_t.",
        ));
    }
    if ledger["pathCount"].as_i64().unwrap_or(0) <= 0 {
        warnings.push(warning(
            "invalid_path_count",
            "Path count must be greater than zero.",
        ));
    }
    if ledger["horizonDays"].as_i64().unwrap_or(0) <= 0 {
        warnings.push(warning(
            "invalid_horizon",
            "Horizon must be greater than zero days.",
        ));
    }
    if ledger["dataWindow"].get("stale").and_then(Value::as_bool) == Some(true) {
        warnings.push(warning(
            "stale_data_window",
            "Data window is marked stale and cannot be used for a validated simulation request.",
        ));
    }
    if str_field(input, "instrumentFamily") == Some("option_chain") {
        warnings.push(warning(
            "unsupported_instrument",
            "Option-chain path simulation requires a valuation adapter and is not part of this request contract.",
        ));
    }
    if str_field(input, "positionState") == Some("ambiguous") {
        warnings.push(warning(
            "ambiguous_position_state",
            "Position state must be resolved before simulation.",
        ));
    }
}

fn missing_ledger_field(ledger: &Value, field: &str) -> bool {
    match ledger.get(field) {
        None | Some(Value::Null) => true,
        Some(Value::String(value)) => value == "missing" || value.trim().is_empty(),
        Some(Value::Array(values)) => field == "sourceProvenance" && values.is_empty(),
        Some(Value::Object(values)) => {
            !matches!(field, "correlationInput" | "costAssumptions") && values.is_empty()
        }
        Some(Value::Number(_)) => false,
        Some(Value::Bool(_)) => false,
    }
}

fn sanitize_value(value: &Value, path: &str) -> (Value, Vec<Value>) {
    match value {
        Value::Object(map) => {
            let mut sanitized = Map::new();
            let mut warnings = Vec::new();
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if secret_key(key) {
                    warnings.push(warning(
                        "secret_material_removed",
                        &format!("Secret-bearing field `{child_path}` was removed from the Monte Carlo contract."),
                    ));
                    continue;
                }
                let (value, mut child_warnings) = sanitize_value(child, &child_path);
                warnings.append(&mut child_warnings);
                sanitized.insert(key.clone(), value);
            }
            (Value::Object(sanitized), warnings)
        }
        Value::Array(values) => {
            let mut sanitized = Vec::new();
            let mut warnings = Vec::new();
            for (index, child) in values.iter().enumerate() {
                let (value, mut child_warnings) =
                    sanitize_value(child, &format!("{path}[{index}]"));
                warnings.append(&mut child_warnings);
                sanitized.push(value);
            }
            (Value::Array(sanitized), warnings)
        }
        _ => (value.clone(), Vec::new()),
    }
}

fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("secret") || key.contains("token") || key.contains("api_key") || key == "apikey"
}

fn str_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn int_field(value: &Value, field: &str) -> Option<i64> {
    value.get(field).and_then(Value::as_i64)
}

fn nested_f64(value: &Value, path: &[&str]) -> Option<f64> {
    let mut cursor = value;
    for segment in path {
        cursor = cursor.get(*segment)?;
    }
    cursor.as_f64()
}

fn nested_str<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for segment in path {
        cursor = cursor.get(*segment)?;
    }
    cursor.as_str()
}

fn int_field_value(value: &Value, field: &str, fallback_field: &str) -> Value {
    int_field(value, field)
        .or_else(|| int_field(value, fallback_field))
        .map_or_else(|| json!(null), |field_value| json!(field_value))
}

fn str_field_value(value: &Value, field: &str, fallback_field: &str) -> Value {
    str_field(value, field)
        .or_else(|| str_field(value, fallback_field))
        .map_or_else(|| json!(null), |field_value| json!(field_value))
}

fn warning(code: &str, message: &str) -> Value {
    json!({"code": code, "message": message})
}

fn sort_warnings(warnings: &mut Vec<Value>) {
    warnings.sort_by(|left, right| {
        warning_sort_key(left)
            .cmp(&warning_sort_key(right))
            .then_with(|| left.to_string().cmp(&right.to_string()))
    });
    warnings.dedup();
}

fn warning_sort_key(value: &Value) -> String {
    value
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut entries = map
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let body = entries
                .into_iter()
                .map(|(key, value)| format!("{}:{value}", serde_json::to_string(&key).unwrap()))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => serde_json::to_string(value).unwrap(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        monte_carlo_contract, monte_carlo_simulation, MonteCarloConfig, MonteCarloMarketData,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn monte_carlo_quotes_are_deterministic_per_seed() {
        let config = MonteCarloConfig {
            seed: 42,
            volatility: 0.01,
            start_prices: BTreeMap::from([("BTC/USD".to_string(), 50_000.0)]),
            ..MonteCarloConfig::default()
        };
        let mut first = MonteCarloMarketData::new(config.clone(), None);
        let mut second = MonteCarloMarketData::new(config, None);

        let first_quotes = first
            .get_quotes(&["BTC/USD", "ETH/USD", "BTC/USD"])
            .expect("first quotes");
        let second_quotes = second
            .get_quotes(&["ETH/USD", "BTC/USD"])
            .expect("second quotes");

        assert_eq!(first_quotes, second_quotes);
        assert_eq!(first_quotes["BTC/USD"].provider, "monte_carlo");
        assert!(first_quotes["BTC/USD"].last > 0.0);
    }

    #[test]
    fn monte_carlo_bars_are_replayable_and_source_shaped() {
        let config = MonteCarloConfig {
            seed: 7,
            drift: 0.001,
            volatility: 0.02,
            start_prices: BTreeMap::from([("SPY".to_string(), 500.0)]),
            ..MonteCarloConfig::default()
        };
        let first = MonteCarloMarketData::new(config.clone(), None);
        let second = MonteCarloMarketData::new(config, None);

        let first_bars = first.get_bars(&["SPY"], 5).expect("first bars");
        let second_bars = second.get_bars(&["SPY"], 5).expect("second bars");
        let capabilities = first.capabilities();

        assert!(capabilities.supports_bars);
        assert!(!capabilities.supports_chains);
        assert_eq!(first_bars, second_bars);
        assert_eq!(first_bars["SPY"][0].open, 500.0);
        assert_eq!(first_bars["SPY"].len(), 5);
        assert!(first_bars["SPY"]
            .iter()
            .all(|bar| bar.low > 0.0 && bar.close > 0.0));
    }

    #[test]
    fn monte_carlo_max_steps_and_unsupported_capabilities_fail_closed() {
        let mut market_data = MonteCarloMarketData::new(
            MonteCarloConfig {
                seed: 99,
                ..MonteCarloConfig::default()
            },
            Some(1),
        );

        market_data.get_quotes(&["BTC/USD"]).expect("first quote");
        assert_eq!(
            market_data.get_quotes(&["BTC/USD"]),
            Err("Monte Carlo data exhausted for configured steps".to_string())
        );
        assert_eq!(
            market_data.get_option_chain("SPY"),
            Err("Monte Carlo data source does not support option chains".to_string())
        );
        assert_eq!(
            market_data.get_indicators(),
            Err("Monte Carlo data source does not support indicators".to_string())
        );
    }

    #[test]
    fn monte_carlo_contract_records_assumptions_hash_outputs_and_replay_refs() {
        let contract = monte_carlo_contract(&json!({
            "strategyId": "strat_mc",
            "backtestId": "bt_1",
            "checkpointId": "checkpoint_1",
            "scenarioValuationRef": "scenario://valuation/1",
            "assumptions": {
                "distribution": "lognormal",
                "returnInput": {"annualizedMean": 0.08, "source": "user"},
                "volatilityInput": {"annualizedVolatility": 0.22, "source": "user"},
                "correlationInput": {"SPY:TLT": -0.25},
                "pathCount": 5000,
                "seed": 12345,
                "horizonDays": 45,
                "cadence": {"rebalance": "none", "evaluation": "daily"},
                "costAssumptions": {"commissionPerContract": 0.65, "slippageBps": 2},
                "dataWindow": {"start": "2026-01-01", "end": "2026-06-01"},
                "sourceProvenance": [{"kind": "backtest", "ref": "bt_1"}],
                "modelVersion": "tradeassembly.monte_carlo.v1"
            }
        }));

        assert_eq!(contract["ok"], true);
        assert_eq!(contract["kind"], "monte_carlo_contract");
        assert_eq!(contract["assumptionLedger"]["distribution"], "lognormal");
        assert_eq!(contract["simulationRequest"]["strategyId"], "strat_mc");
        assert_eq!(contract["simulationRequest"]["backtestId"], "bt_1");
        assert_eq!(
            contract["simulationRequest"]["checkpointId"],
            "checkpoint_1"
        );
        assert_eq!(
            contract["simulationRequest"]["scenarioValuationRef"],
            "scenario://valuation/1"
        );
        assert_eq!(contract["simulationRequest"]["pathCount"], 5000);
        assert_eq!(contract["outputs"]["randomSeed"], 12345);
        assert_eq!(contract["outputs"]["pathSummary"]["pathCount"], 5000);
        assert!(contract["assumptionLedger"]["assumptionHash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(
            contract["replayRefs"]["commands"][0],
            "tradeassembly monte-carlo replay --strategy-id strat_mc --assumption-hash "
                .to_string()
                + contract["assumptionLedger"]["assumptionHash"]
                    .as_str()
                    .unwrap()
        );
        assert!(contract["warnings"].as_array().unwrap().is_empty());
        assert!(contract["notice"]
            .as_str()
            .unwrap()
            .contains("research-only sensitivity analysis"));
    }

    #[test]
    fn monte_carlo_contract_hash_and_serialization_are_deterministic() {
        let first = monte_carlo_contract(&json!({
            "strategyId": "strat_mc",
            "assumptions": {
                "distribution": "lognormal",
                "returnInput": {"source": "user", "annualizedMean": 0.08},
                "volatilityInput": {"source": "user", "annualizedVolatility": 0.22},
                "correlationInput": {},
                "pathCount": 1000,
                "seed": 0,
                "horizonDays": 30,
                "cadence": {"evaluation": "daily", "rebalance": "none"},
                "costAssumptions": {"slippageBps": 0},
                "dataWindow": {"end": "2026-06-01", "start": "2026-01-01"},
                "sourceProvenance": [{"ref": "fixture", "kind": "fixture"}],
                "modelVersion": "tradeassembly.monte_carlo.v1"
            }
        }));
        let second = monte_carlo_contract(&json!({
            "assumptions": {
                "modelVersion": "tradeassembly.monte_carlo.v1",
                "sourceProvenance": [{"kind": "fixture", "ref": "fixture"}],
                "dataWindow": {"start": "2026-01-01", "end": "2026-06-01"},
                "costAssumptions": {"slippageBps": 0},
                "cadence": {"rebalance": "none", "evaluation": "daily"},
                "horizonDays": 30,
                "seed": 0,
                "pathCount": 1000,
                "correlationInput": {},
                "volatilityInput": {"annualizedVolatility": 0.22, "source": "user"},
                "returnInput": {"annualizedMean": 0.08, "source": "user"},
                "distribution": "lognormal"
            },
            "strategyId": "strat_mc"
        }));

        assert_eq!(first["ok"], true);
        assert_eq!(
            first["assumptionLedger"]["assumptionHash"],
            second["assumptionLedger"]["assumptionHash"]
        );
        assert_eq!(first["canonicalRequest"], second["canonicalRequest"]);
    }

    #[test]
    fn monte_carlo_contract_fails_closed_for_missing_stale_or_unsupported_inputs() {
        let contract = monte_carlo_contract(&json!({
            "strategyId": "strat_mc",
            "instrumentFamily": "option_chain",
            "positionState": "ambiguous",
            "assumptions": {
                "distribution": "predictive_magic",
                "pathCount": 0,
                "seed": 7,
                "horizonDays": 30,
                "dataWindow": {"stale": true}
            }
        }));
        let warnings = contract["warnings"].as_array().unwrap();

        assert_eq!(contract["ok"], false);
        assert!(warnings
            .iter()
            .any(|warning| warning["code"] == "missing_assumption"));
        assert!(warnings
            .iter()
            .any(|warning| warning["code"] == "unsupported_distribution"));
        assert!(warnings
            .iter()
            .any(|warning| warning["code"] == "invalid_path_count"));
        assert!(warnings
            .iter()
            .any(|warning| warning["code"] == "stale_data_window"));
        assert!(warnings
            .iter()
            .any(|warning| warning["code"] == "unsupported_instrument"));
        assert!(warnings
            .iter()
            .any(|warning| warning["code"] == "ambiguous_position_state"));
    }

    #[test]
    fn monte_carlo_contract_redacts_secret_material_and_forbids_advice_copy() {
        let contract = monte_carlo_contract(&json!({
            "strategyId": "strat_mc",
            "assumptions": {
                "distribution": "lognormal",
                "returnInput": {"annualizedMean": 0.08},
                "volatilityInput": {"annualizedVolatility": 0.22},
                "correlationInput": {},
                "pathCount": 1000,
                "seed": 7,
                "horizonDays": 30,
                "cadence": {"evaluation": "daily"},
                "costAssumptions": {},
                "dataWindow": {"start": "2026-01-01", "end": "2026-06-01"},
                "sourceProvenance": [{"kind": "fixture", "apiSecret": "SUPER-SECRET-VALUE"}],
                "modelVersion": "tradeassembly.monte_carlo.v1"
            }
        }));
        let rendered = serde_json::to_string(&contract).unwrap();

        assert!(!rendered.contains("SUPER-SECRET-VALUE"));
        assert!(contract["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "secret_material_removed"));
        assert!(!rendered.to_lowercase().contains("should buy"));
        assert!(!rendered.to_lowercase().contains("should sell"));
        assert!(!rendered.to_lowercase().contains("recommended position"));
    }

    #[test]
    fn monte_carlo_simulation_is_deterministic_and_emits_result_refs() {
        let input = json!({
            "strategyId": "strat_mc",
            "mode": "strategy_returns",
            "assumptions": {
                "distribution": "lognormal",
                "returnInput": {"annualizedMean": 0.08, "source": "user"},
                "volatilityInput": {"annualizedVolatility": 0.22, "source": "user"},
                "correlationInput": {},
                "pathCount": 64,
                "seed": 0,
                "horizonDays": 20,
                "cadence": {"evaluation": "daily"},
                "costAssumptions": {"slippageBps": 1},
                "dataWindow": {"start": "2026-01-01", "end": "2026-06-01"},
                "sourceProvenance": [{"kind": "fixture", "ref": "returns"}],
                "modelVersion": "tradeassembly.monte_carlo.v1"
            }
        });

        let first = monte_carlo_simulation(&input);
        let second = monte_carlo_simulation(&input);

        assert_eq!(first["ok"], true);
        assert_eq!(first, second);
        assert_eq!(first["simulationResult"]["pathSummary"]["pathCount"], 64);
        assert_eq!(first["simulationResult"]["pathSummary"]["horizonDays"], 20);
        assert!(first["simulationResult"]["terminalDistribution"]["p95"].is_number());
        assert!(first["simulationResult"]["drawdownDistribution"]["mean"].is_number());
        assert_eq!(
            first["journalEvidence"][0]["eventType"],
            "monte_carlo.simulation.completed"
        );
        assert!(first["exportRefs"]["resultJson"]["uri"]
            .as_str()
            .unwrap()
            .contains("tradeassembly://monte-carlo/strat_mc/"));
        assert!(first["notice"]
            .as_str()
            .unwrap()
            .contains("research-only sensitivity analysis"));
    }

    #[test]
    fn monte_carlo_simulation_supports_position_valuation_when_inputs_exist() {
        let result = monte_carlo_simulation(&json!({
            "strategyId": "strat_mc",
            "mode": "position_valuation",
            "valuationInputs": {"startValue": 2500.0, "returnMultiplier": 0.5, "scenarioSensitivityRef": "scenario://valuation/1"},
            "assumptions": {
                "distribution": "student_t",
                "returnInput": {"annualizedMean": 0.05},
                "volatilityInput": {"annualizedVolatility": 0.18},
                "correlationInput": {},
                "pathCount": 32,
                "seed": 9,
                "horizonDays": 10,
                "cadence": {"evaluation": "daily"},
                "costAssumptions": {},
                "dataWindow": {"start": "2026-01-01", "end": "2026-06-01"},
                "sourceProvenance": [{"kind": "scenario", "ref": "scenario://valuation/1"}],
                "modelVersion": "tradeassembly.monte_carlo.v1"
            }
        }));

        assert_eq!(result["ok"], true);
        assert_eq!(result["simulationResult"]["mode"], "position_valuation");
        assert_eq!(
            result["simulationResult"]["scenarioSensitivityRefs"][0],
            "scenario://valuation/1"
        );
        assert!(result["simulationResult"]["terminalDistribution"]["mean"].is_number());
    }

    #[test]
    fn monte_carlo_simulation_fails_closed_for_data_hash_and_valuation_gaps() {
        let base = json!({
            "strategyId": "strat_mc",
            "mode": "strategy_returns",
            "expectedAssumptionHash": "sha256:not-the-real-hash",
            "assumptions": {
                "distribution": "historical_bootstrap",
                "returnInput": {"annualizedMean": 0.05},
                "volatilityInput": {"annualizedVolatility": 0.18},
                "correlationInput": {},
                "pathCount": 16,
                "seed": 9,
                "horizonDays": 10,
                "cadence": {"evaluation": "daily"},
                "costAssumptions": {},
                "dataWindow": {"start": "2026-01-01", "end": "2026-06-01"},
                "sourceProvenance": [{"kind": "fixture", "ref": "too-short"}],
                "modelVersion": "tradeassembly.monte_carlo.v1"
            },
            "dataSnapshot": {"returns": [0.01]}
        });
        let hash_mismatch = monte_carlo_simulation(&base);
        let unsupported_valuation = monte_carlo_simulation(&json!({
            "strategyId": "strat_mc",
            "mode": "position_valuation",
            "valuationInputs": {},
            "assumptions": base["assumptions"].clone()
        }));

        assert_eq!(hash_mismatch["ok"], false);
        assert!(hash_mismatch["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "assumption_hash_mismatch"));
        assert!(hash_mismatch["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "insufficient_data"));
        assert_eq!(unsupported_valuation["ok"], false);
        assert!(unsupported_valuation["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "missing_valuation_inputs"));
    }

    #[test]
    fn monte_carlo_simulation_redacts_secrets_and_forbids_advice_copy() {
        let result = monte_carlo_simulation(&json!({
            "strategyId": "strat_mc",
            "mode": "strategy_returns",
            "assumptions": {
                "distribution": "lognormal",
                "returnInput": {"annualizedMean": 0.08, "apiToken": "SECRET-TOKEN"},
                "volatilityInput": {"annualizedVolatility": 0.22},
                "correlationInput": {},
                "pathCount": 8,
                "seed": 3,
                "horizonDays": 5,
                "cadence": {"evaluation": "daily"},
                "costAssumptions": {},
                "dataWindow": {"start": "2026-01-01", "end": "2026-06-01"},
                "sourceProvenance": [{"kind": "fixture", "ref": "safe"}],
                "modelVersion": "tradeassembly.monte_carlo.v1"
            }
        }));
        let rendered = serde_json::to_string(&result).unwrap().to_lowercase();

        assert!(!rendered.contains("secret-token"));
        assert!(!rendered.contains("should buy"));
        assert!(!rendered.contains("should sell"));
        assert!(!rendered.contains("recommended position"));
        assert!(result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "secret_material_removed"));
    }
}
