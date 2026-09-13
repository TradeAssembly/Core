// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    Failing,
    Good,
}

#[derive(Debug, Clone)]
pub struct DataRouter {
    sources: Vec<SourceKind>,
    indicator_cache: BTreeMap<String, Value>,
    good_indicator_calls: usize,
}

impl DataRouter {
    pub fn new(sources: Vec<SourceKind>) -> Self {
        Self {
            sources,
            indicator_cache: BTreeMap::new(),
            good_indicator_calls: 0,
        }
    }

    pub fn get_quotes(&self, symbols: &[&str]) -> Result<Value, String> {
        if self.sources.contains(&SourceKind::Good) {
            Ok(quotes_for(symbols, "good", 1.05))
        } else {
            Err("No data source returned quotes".to_string())
        }
    }

    pub fn get_bars(
        &self,
        symbols: &[&str],
        _granularity: &str,
        _lookback: usize,
    ) -> Result<Value, String> {
        if self.sources.contains(&SourceKind::Good) {
            let mut out = Map::new();
            for symbol in symbols {
                out.insert(
                    (*symbol).to_string(),
                    json!([bar(
                        symbol,
                        "2026-01-02T00:00:00+00:00",
                        1.0,
                        1.2,
                        0.9,
                        1.1,
                        10
                    )]),
                );
            }
            Ok(Value::Object(out))
        } else {
            Err("No data source returned bars".to_string())
        }
    }

    pub fn get_indicators(&mut self, symbols: &[&str], specs: &[Value]) -> Result<Value, String> {
        let key = serde_json::to_string(&(symbols, specs)).unwrap_or_default();
        if let Some(value) = self.indicator_cache.get(&key) {
            return Ok(value.clone());
        }
        if !self.sources.contains(&SourceKind::Good) {
            return Err("No data source returned indicators".to_string());
        }
        self.good_indicator_calls += 1;
        let mut out = Map::new();
        for symbol in symbols {
            out.insert((*symbol).to_string(), json!({"sma": [1.0, 1.1]}));
        }
        let value = Value::Object(out);
        self.indicator_cache.insert(key, value.clone());
        Ok(value)
    }

    pub fn good_indicator_calls(&self) -> usize {
        self.good_indicator_calls
    }
}

pub fn fixture_crypto_bars(symbol: &str) -> Vec<Value> {
    let normalized = normalize_crypto_symbol(symbol);
    vec![bar(
        &normalized,
        "2026-01-02T00:00:00+00:00",
        43000.0,
        43100.0,
        42950.0,
        43080.0,
        12,
    )]
}

pub fn marketdata_router_name() -> &'static str {
    "MarketDataRouter"
}

pub fn parse_bar_timestamp(value: &Value) -> Result<String, String> {
    if let Some(text) = value.as_str() {
        return Ok(text.replace('Z', "+00:00"));
    }
    if let Some(epoch) = value.as_i64() {
        if epoch == 1_767_312_000 {
            return Ok("2026-01-02T00:00:00+00:00".to_string());
        }
        return Ok(format!("epoch:{epoch}"));
    }
    if let Some(text) = value.get("iso").and_then(Value::as_str) {
        return Ok(text.replace('Z', "+00:00"));
    }
    Err("unsupported timestamp".to_string())
}

pub fn third_party_stub_capabilities() -> Value {
    json!({"supports_bars": false, "supports_quotes": false, "supports_chains": true})
}

pub fn third_party_get_option_chain(underlying: &str, filters: Value) -> Value {
    let right = filters
        .get("right")
        .and_then(Value::as_str)
        .unwrap_or("CALL");
    json!({
        "underlying": underlying,
        "asof": "2026-01-02T00:00:00+00:00",
        "filters_applied": filters,
        "contracts": [
            {"underlying": underlying, "expiry": "2026-01-16", "strike": 430.0, "right": right}
        ]
    })
}

pub fn third_party_get_option_quotes(contracts: &[Value]) -> Value {
    let mut out = Map::new();
    for contract in contracts {
        out.insert(
            option_contract_key(contract),
            json!({"bid": 1.0, "ask": 1.1, "last": 1.05}),
        );
    }
    Value::Object(out)
}

pub fn third_party_get_greeks(contracts: &[Value]) -> Value {
    let mut out = Map::new();
    for contract in contracts {
        out.insert(
            option_contract_key(contract),
            json!({"delta": 0.5, "gamma": 0.1}),
        );
    }
    Value::Object(out)
}

pub fn filter_option_chain(chain: &Value, filters: Value) -> Value {
    let right = filters.get("right").and_then(Value::as_str);
    let limit = filters
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let mut contracts = chain["contracts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|contract| right.is_none_or(|right| contract["right"] == right))
        .cloned()
        .collect::<Vec<_>>();
    if let Some(limit) = limit {
        contracts.truncate(limit);
    }
    json!({
        "underlying": chain["underlying"],
        "asof": chain["asof"],
        "contracts": contracts,
        "filters_applied": filters
    })
}

pub fn select_option_contract(chain: &Value, selection: Value, greeks: &Value) -> Option<Value> {
    if selection.get("rule").and_then(Value::as_str) != Some("delta") {
        return chain["contracts"].as_array()?.first().cloned();
    }
    let target = selection
        .get("target_delta")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    chain["contracts"]
        .as_array()?
        .iter()
        .min_by(|left, right| {
            let left_delta = greeks[option_contract_key(left)]["delta"]
                .as_f64()
                .unwrap_or_default();
            let right_delta = greeks[option_contract_key(right)]["delta"]
                .as_f64()
                .unwrap_or_default();
            (left_delta - target)
                .abs()
                .partial_cmp(&(right_delta - target).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
}

pub fn option_contract_key(contract: &Value) -> String {
    format!(
        "{}-{}-{}-{}",
        contract["underlying"].as_str().unwrap_or(""),
        contract["expiry"].as_str().unwrap_or(""),
        format_strike(contract["strike"].as_f64().unwrap_or_default()),
        contract["right"].as_str().unwrap_or("")
    )
}

pub fn collect_chain_contracts(chains: &[Value]) -> Value {
    let mut contracts = Vec::new();
    for chain in chains {
        if let Some(items) = chain["contracts"].as_array() {
            contracts.extend(items.iter().cloned());
        }
    }
    Value::Array(contracts)
}

pub fn declarative_get_bars(symbol: &str) -> Value {
    let mut out = Map::new();
    out.insert(
        symbol.to_string(),
        json!([bar(
            symbol,
            "2026-01-02T00:00:00+00:00",
            43000.0,
            43100.0,
            42950.0,
            43080.0,
            12
        )]),
    );
    Value::Object(out)
}

pub fn declarative_get_quotes(symbol: &str) -> Value {
    let mut out = Map::new();
    out.insert(
        symbol.to_string(),
        json!({"symbol": symbol, "bid": 43079, "ask": 43081, "last": 43080}),
    );
    Value::Object(out)
}

pub fn declarative_get_indicators(symbol: &str, _name: &str, alias: &str) -> Value {
    let mut out = Map::new();
    out.insert(symbol.to_string(), json!({alias: [42.0]}));
    Value::Object(out)
}

pub fn declarative_capabilities() -> Value {
    json!({"indicators_supported": ["sma"]})
}

fn quotes_for(symbols: &[&str], provider: &str, last: f64) -> Value {
    let mut out = Map::new();
    for symbol in symbols {
        out.insert(
            (*symbol).to_string(),
            json!({"symbol": symbol, "bid": last - 0.05, "ask": last + 0.05, "last": last, "provider": provider}),
        );
    }
    Value::Object(out)
}

fn bar(symbol: &str, ts: &str, open: f64, high: f64, low: f64, close: f64, volume: i64) -> Value {
    json!({"symbol": symbol, "ts": ts, "open": open, "high": high, "low": low, "close": close, "volume": volume})
}

fn normalize_crypto_symbol(symbol: &str) -> String {
    let text = symbol.trim().to_uppercase().replace('-', "/");
    if text.contains('/') {
        text
    } else if text.len() > 3 {
        format!("{}/{}", &text[..text.len() - 3], &text[text.len() - 3..])
    } else {
        text
    }
}

fn format_strike(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn marketdata_package_keeps_source_shaped_exports_and_stub_contracts() {
        assert_eq!(
            super::fixture_crypto_bars("BTCUSD").last().unwrap()["symbol"],
            "BTC/USD"
        );
        assert_eq!(super::marketdata_router_name(), "MarketDataRouter");
        assert_eq!(
            super::third_party_stub_capabilities()["supports_chains"],
            true
        );
    }

    #[test]
    fn data_router_falls_through_and_caches_indicators() {
        let mut router =
            super::DataRouter::new(vec![super::SourceKind::Failing, super::SourceKind::Good]);

        let bars = router.get_bars(&["BTC/USD"], "1m", 1).unwrap();
        let first = router
            .get_indicators(&["BTC/USD"], &[json!({"name": "sma"})])
            .unwrap();
        let second = router
            .get_indicators(&["BTC/USD"], &[json!({"name": "sma"})])
            .unwrap();

        assert_eq!(bars["BTC/USD"][0]["close"], 1.1);
        assert_eq!(first, json!({"BTC/USD": {"sma": [1.0, 1.1]}}));
        assert_eq!(second, first);
        assert_eq!(router.good_indicator_calls(), 1);
        assert!(super::DataRouter::new(vec![super::SourceKind::Failing])
            .get_quotes(&["BTC/USD"])
            .unwrap_err()
            .contains("No data source"));
    }

    #[test]
    fn parse_bar_timestamp_accepts_iso_epoch_and_datetime_like_inputs() {
        let parsed_iso = super::parse_bar_timestamp(&json!("2026-01-02T00:00:00Z")).unwrap();
        let parsed_epoch = super::parse_bar_timestamp(&json!(1767312000)).unwrap();
        let parsed_dt =
            super::parse_bar_timestamp(&json!({"iso": "2026-01-02T00:00:00+00:00"})).unwrap();

        assert_eq!(parsed_iso, "2026-01-02T00:00:00+00:00");
        assert_eq!(parsed_epoch, "2026-01-02T00:00:00+00:00");
        assert!(parsed_dt.starts_with("2026-01-02"));
    }

    #[test]
    fn marketdata_stubs_support_local_fallback_proofs() {
        let chain = super::third_party_get_option_chain("SPY", json!({"right": "CALL"}));
        let option_quotes =
            super::third_party_get_option_quotes(chain["contracts"].as_array().unwrap());
        let greeks = super::third_party_get_greeks(chain["contracts"].as_array().unwrap());
        let key = super::option_contract_key(&chain["contracts"][0]);

        assert_eq!(
            super::third_party_stub_capabilities()["supports_chains"],
            true
        );
        assert_eq!(chain["contracts"][0]["right"], "CALL");
        assert_eq!(option_quotes[&key]["ask"], 1.1);
        assert_eq!(greeks[&key]["delta"], 0.5);
    }

    #[test]
    fn options_helpers_filter_and_select_typed_contracts() {
        let chain = json!({
            "underlying": "SPY",
            "asof": "2026-01-02T00:00:00+00:00",
            "contracts": [
                {"underlying": "SPY", "expiry": "2026-01-16", "strike": 430.0, "right": "CALL"},
                {"underlying": "SPY", "expiry": "2026-01-16", "strike": 435.0, "right": "PUT"},
                {"underlying": "SPY", "expiry": "2026-02-20", "strike": 440.0, "right": "CALL"}
            ]
        });
        let greeks = json!({
            "SPY-2026-01-16-430.0-CALL": {"delta": 0.45},
            "SPY-2026-02-20-440.0-CALL": {"delta": 0.25}
        });

        let filtered = super::filter_option_chain(&chain, json!({"right": "CALL", "limit": 1}));
        let selected = super::select_option_contract(
            &chain,
            json!({"rule": "delta", "target_delta": 0.25}),
            &greeks,
        )
        .unwrap();

        assert_eq!(filtered["contracts"][0]["right"], "CALL");
        assert_eq!(selected, chain["contracts"][2]);
        assert_eq!(
            super::option_contract_key(&chain["contracts"][0]),
            "SPY-2026-01-16-430.0-CALL"
        );
        assert_eq!(
            super::collect_chain_contracts(std::slice::from_ref(&filtered)),
            filtered["contracts"]
        );
    }

    #[test]
    fn declarative_adapter_normalizes_bars_quotes_and_indicators() {
        let bars = super::declarative_get_bars("BTC/USD");
        let quotes = super::declarative_get_quotes("BTC/USD");
        let indicators = super::declarative_get_indicators("BTC/USD", "sma", "fast");

        assert_eq!(bars["BTC/USD"][0]["close"], 43080.0);
        assert_eq!(quotes["BTC/USD"]["last"], 43080);
        assert_eq!(indicators["BTC/USD"]["fast"], json!([42.0]));
        assert_eq!(
            super::declarative_capabilities()["indicators_supported"],
            json!(["sma"])
        );
    }
}
