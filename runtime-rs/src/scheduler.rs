// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetClass {
    Equity,
    Option,
    Crypto,
    Future,
    Fx,
    Fund,
    Index,
}

impl AssetClass {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Equity => "equity",
            Self::Option => "option",
            Self::Crypto => "crypto",
            Self::Future => "future",
            Self::Fx => "fx",
            Self::Fund => "fund",
            Self::Index => "index",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageSpec {
    pub ref_id: String,
    #[serde(default)]
    pub config: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategySpec {
    pub symbol: String,
    pub asset_class: AssetClass,
    pub entry_rule: String,
    pub exit_rule: String,
    #[serde(default = "default_timeframe")]
    pub timeframe: String,
    #[serde(default)]
    pub stages: Map<String, Value>,
}

impl StrategySpec {
    pub fn new(
        symbol: impl Into<String>,
        asset_class: AssetClass,
        entry_rule: impl Into<String>,
        exit_rule: impl Into<String>,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            asset_class,
            entry_rule: entry_rule.into(),
            exit_rule: exit_rule.into(),
            timeframe: default_timeframe(),
            stages: Map::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketClockDecision {
    pub allowed: bool,
    pub asset_class: String,
    pub reason_code: String,
    pub evaluated_at_utc: String,
    pub session: String,
    pub details: Map<String, Value>,
    pub blocked_layers: Vec<String>,
    pub reason_codes: Vec<String>,
    pub layers: Map<String, Value>,
}

impl MarketClockDecision {
    pub fn as_dict(&self) -> Value {
        json!({
            "allowed": self.allowed,
            "assetClass": self.asset_class,
            "reasonCode": self.reason_code,
            "evaluatedAtUtc": self.evaluated_at_utc,
            "session": self.session,
            "details": self.details,
            "blockedLayers": self.blocked_layers,
            "reasonCodes": self.reason_codes,
            "layers": self.layers,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub strategy_id: String,
    #[serde(default)]
    pub symbols: Vec<String>,
    pub timeframe: String,
    pub interval_seconds: Option<f64>,
    pub live: bool,
    pub asset: String,
    pub dry_run: bool,
    pub provider_ref: Option<String>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl SchedulerConfig {
    pub fn from_args(strategy_id: impl Into<String>, symbols: Option<Vec<String>>) -> Self {
        Self {
            strategy_id: strategy_id.into(),
            symbols: symbols.unwrap_or_default(),
            timeframe: "5m".to_string(),
            interval_seconds: None,
            live: false,
            asset: "equity".to_string(),
            dry_run: false,
            provider_ref: None,
            metadata: Map::new(),
        }
    }

    pub fn dry_run(mut self, value: bool) -> Self {
        self.dry_run = value;
        self
    }

    pub fn timeframe(mut self, value: impl Into<String>) -> Self {
        self.timeframe = value.into();
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerTriggerProducer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerWorker;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerTriggerPublication;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerWorkerResult;

pub type StrategyScheduler = SchedulerTriggerProducer;
pub type MAScheduler = SchedulerTriggerProducer;

pub fn scheduler_from_args(
    strategy_id: impl Into<String>,
    symbols: Option<Vec<String>>,
) -> SchedulerConfig {
    SchedulerConfig::from_args(strategy_id, symbols)
}

pub fn decision_to_dict(decision: &MarketClockDecision) -> Value {
    decision.as_dict()
}

pub fn evaluate_market_clock(spec: &StrategySpec, now_utc: Option<&str>) -> MarketClockDecision {
    evaluate_scheduler_market_clock(spec, now_utc)
}

pub fn evaluate_scheduler_market_clock(
    spec: &StrategySpec,
    now_utc: Option<&str>,
) -> MarketClockDecision {
    let now = now_utc.unwrap_or("1970-01-01T00:00:00+00:00").to_string();
    let asset_class = spec.asset_class.as_str().to_string();
    match spec.asset_class {
        AssetClass::Crypto => MarketClockDecision {
            allowed: true,
            asset_class,
            reason_code: "crypto.always_open".to_string(),
            evaluated_at_utc: now,
            session: "open".to_string(),
            details: map([("source", json!("builtin"))]),
            blocked_layers: Vec::new(),
            reason_codes: vec!["crypto.always_open".to_string()],
            layers: Map::new(),
        },
        _ => MarketClockDecision {
            allowed: false,
            asset_class,
            reason_code: "regular_session.closed".to_string(),
            evaluated_at_utc: now,
            session: "closed".to_string(),
            details: map([("source", json!("builtin"))]),
            blocked_layers: vec!["asset_class".to_string()],
            reason_codes: vec!["asset_class.regular_session.closed".to_string()],
            layers: Map::new(),
        },
    }
}

pub fn scheduler_trigger_key(
    activation_id: &str,
    cycle: u64,
    symbol: &str,
    timeframe: &str,
    provider_ref: &str,
    mode: &str,
) -> String {
    let payload = format!(
        "{{\"activation_id\":\"{}\",\"cycle\":{},\"mode\":\"{}\",\"provider_ref\":\"{}\",\"symbol\":\"{}\",\"timeframe\":\"{}\"}}",
        escape_json(activation_id),
        cycle,
        escape_json(mode),
        escape_json(provider_ref),
        escape_json(symbol),
        escape_json(timeframe)
    );
    let digest = Sha256::digest(payload.as_bytes());
    hex_lower(&digest)[..32].to_string()
}

fn default_timeframe() -> String {
    "1d".to_string()
}

fn escape_json(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect()
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_trigger_key_matches_source_hash_shape() {
        let key = scheduler_trigger_key("activation-1", 1, "BTC/USD", "1m", "sim", "paper");

        assert_eq!(key, "5c7026eabe39d13dd706d0069c9b5ef4"); // gitleaks:allow -- deterministic hash of public inputs above, not a credential
    }

    #[test]
    fn source_market_clock_wrapper_uses_local_crypto_guard() {
        let spec = StrategySpec::new("BTC/USD", AssetClass::Crypto, "close > 1", "close < 1");

        let decision = evaluate_market_clock(&spec, Some("2026-01-01T00:00:00+00:00"));

        assert!(decision.allowed);
        assert_eq!(decision.reason_code, "crypto.always_open");
        assert_eq!(decision_to_dict(&decision)["allowed"], true);
    }

    #[test]
    fn source_strategy_config_wrapper_preserves_defaults() {
        let cfg =
            scheduler_from_args("strategy-1", Some(vec!["BTC/USD".to_string()])).dry_run(true);

        assert_eq!(cfg.strategy_id, "strategy-1");
        assert_eq!(cfg.symbols, ["BTC/USD"]);
        assert_eq!(cfg.timeframe, "5m");
        assert!(cfg.dry_run);
        let _strategy_scheduler: StrategyScheduler = SchedulerTriggerProducer;
        let _worker = SchedulerWorker;
    }
}
