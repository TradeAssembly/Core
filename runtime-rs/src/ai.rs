// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AiGatewayConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: f64,
}

pub fn load_ai_gateway_config() -> AiGatewayConfig {
    let env = std::env::vars().collect::<BTreeMap<_, _>>();
    load_ai_gateway_config_from(&env)
}

pub fn load_ai_gateway_config_from(env: &BTreeMap<String, String>) -> AiGatewayConfig {
    let provider = env_text(env, "TRADEASSEMBLY_AI_PROVIDER", "stub").to_ascii_lowercase();
    let mut base_url = env_text(env, "TRADEASSEMBLY_AI_BASE_URL", "http://127.0.0.1:4000/v1");
    let api_key = env_text(env, "TRADEASSEMBLY_AI_API_KEY", "")
        .or_else_nonempty(|| env_text(env, "OPENAI_API_KEY", ""));
    let model = env_text(env, "TRADEASSEMBLY_AI_MODEL", "tradeassembly-local-draft");
    if provider == "stub" {
        return AiGatewayConfig {
            provider,
            base_url: String::new(),
            api_key: String::new(),
            model,
            timeout_seconds: 30.0,
        };
    }
    if !base_url.is_empty() && !base_url.ends_with("/v1") {
        base_url.push_str("/v1");
    }
    if provider == "openai" && base_url.is_empty() {
        base_url = "https://api.openai.com/v1".to_string();
    }
    AiGatewayConfig {
        provider,
        base_url,
        api_key: if api_key.is_empty() {
            "local-dev".to_string()
        } else {
            api_key
        },
        model,
        timeout_seconds: 30.0,
    }
}

pub fn strategy_draft_payload(
    config: &AiGatewayConfig,
    brief: &str,
    market: &str,
    horizon: &str,
) -> Result<Value, String> {
    if config.provider == "stub" {
        let mut content = object_from_value(deterministic_strategy_draft(brief, market, horizon))?;
        content.insert("source".to_string(), Value::String("stub".to_string()));
        content.insert(
            "model".to_string(),
            Value::String("deterministic-stub".to_string()),
        );
        return Ok(Value::Object(content));
    }

    match chat_completion_payload(config, brief, market, horizon) {
        Ok(mut content) => {
            content.insert("source".to_string(), Value::String(config.provider.clone()));
            content.insert("model".to_string(), Value::String(config.model.clone()));
            Ok(Value::Object(content))
        }
        Err(error) => {
            let mut content =
                object_from_value(deterministic_strategy_draft(brief, market, horizon))?;
            content.insert(
                "source".to_string(),
                Value::String("stub-fallback".to_string()),
            );
            content.insert(
                "model".to_string(),
                Value::String("deterministic-stub".to_string()),
            );
            content.insert("error_message".to_string(), Value::String(error));
            Ok(Value::Object(content))
        }
    }
}

pub fn deterministic_strategy_draft(brief: &str, market: &str, horizon: &str) -> Value {
    let normalized_market = market.trim().to_ascii_lowercase();
    let is_equity = matches!(
        normalized_market.as_str(),
        "stock" | "stocks" | "equity" | "equities"
    );
    let symbol = if is_equity { "SPY" } else { "BTC/USD" };
    let asset_class = if is_equity { "equity" } else { "crypto" };
    let provider_ref = if is_equity { "sim" } else { "local-data" };
    let timeframe = if is_equity || matches!(horizon, "swing" | "daily") {
        "1d"
    } else {
        "1m"
    };
    let name_suffix = {
        let trimmed = brief.trim();
        if trimmed.is_empty() {
            "User-defined strategy".to_string()
        } else {
            trimmed.chars().take(48).collect::<String>()
        }
    };
    json!({
        "name": format!("Local draft: {name_suffix}"),
        "symbol": symbol,
        "asset_class": asset_class,
        "provider_ref": provider_ref,
        "timeframe": timeframe,
        "entry_rule": if asset_class == "crypto" {
            "user_defined_btc_momentum_entry"
        } else {
            "current(metric(\"price\")) > previous(metric(\"price\"))"
        },
        "exit_rule": if asset_class == "crypto" {
            "user_defined_exit_after_entry_receipt"
        } else {
            "current(metric(\"price\")) < previous(metric(\"price\"))"
        },
        "risk": risk_payload(asset_class),
        "notes": ["Drafted as local starter logic. User owns all market selection and risk scale."],
    })
}

pub fn clean_json_content(value: &str) -> Result<Value, String> {
    let mut stripped = value.trim();
    if stripped.starts_with("```") {
        stripped = stripped.trim_matches('`').trim();
        if let Some(rest) = stripped.strip_prefix("json") {
            stripped = rest.trim();
        }
    }
    let parsed: Value = serde_json::from_str(stripped).map_err(|error| error.to_string())?;
    if parsed.is_object() {
        Ok(parsed)
    } else {
        Err("ai response content was not a JSON object".to_string())
    }
}

fn chat_completion_payload(
    config: &AiGatewayConfig,
    brief: &str,
    market: &str,
    horizon: &str,
) -> Result<Map<String, Value>, String> {
    if config.base_url.trim().is_empty() {
        return Err("ai base_url is required".to_string());
    }
    let client = Client::builder()
        .timeout(Duration::from_secs_f64(config.timeout_seconds.max(0.001)))
        .build()
        .map_err(|error| error.to_string())?;
    let payload = json!({
        "model": config.model,
        "messages": draft_prompt(brief, market, horizon),
        "temperature": 0.2,
        "response_format": {"type": "json_object"},
    });
    let mut request = client
        .post(format!(
            "{}/chat/completions",
            config.base_url.trim_end_matches('/')
        ))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .json(&payload);
    if !config.api_key.trim().is_empty() {
        request = request.header(AUTHORIZATION, format!("Bearer {}", config.api_key));
    }
    let response = request.send().map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("ai gateway returned status {status}"));
    }
    let response_payload: Value = response.json().map_err(|error| error.to_string())?;
    extract_json_content(&response_payload)
}

fn extract_json_content(payload: &Value) -> Result<Map<String, Value>, String> {
    let choices = payload
        .get("choices")
        .and_then(Value::as_array)
        .filter(|choices| !choices.is_empty())
        .ok_or_else(|| "ai response did not contain choices".to_string())?;
    let message = choices[0]
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| "ai response choice missing message".to_string())?;
    let content = message
        .get("content")
        .ok_or_else(|| "ai response content missing".to_string())?;
    let content = if let Some(text) = content.as_str() {
        text.to_string()
    } else if let Some(items) = content.as_array() {
        items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        return Err("ai response content missing".to_string());
    };
    object_from_value(clean_json_content(&content)?)
}

fn draft_prompt(brief: &str, market: &str, horizon: &str) -> Vec<Value> {
    vec![
        json!({
            "role": "system",
            "content": "Draft TradeAssembly strategy metadata for local user-owned automation. Never tell users what to trade, when to trade, or how much to trade. Return JSON only.",
        }),
        json!({
            "role": "user",
            "content": serde_json::to_string(&json!({
                "brief": brief,
                "market": market,
                "horizon": horizon,
            }))
            .expect("serialize prompt payload"),
        }),
    ]
}

fn object_from_value(value: Value) -> Result<Map<String, Value>, String> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "expected JSON object".to_string())
}

fn risk_payload(asset_class: &str) -> Value {
    if asset_class == "crypto" {
        json!({
            "max_notional": 25,
            "max_order_quantity": 0.0003,
            "max_daily_loss": 25,
            "max_concurrent_positions": 1,
        })
    } else {
        json!({
            "max_notional": 1000,
            "max_order_quantity": 1,
            "max_daily_loss": 250,
            "max_concurrent_positions": 1,
        })
    }
}

fn env_text(env: &BTreeMap<String, String>, key: &str, default: &str) -> String {
    env.get(key)
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

trait NonEmptyFallback {
    fn or_else_nonempty<F: FnOnce() -> String>(self, fallback: F) -> String;
}

impl NonEmptyFallback for String {
    fn or_else_nonempty<F: FnOnce() -> String>(self, fallback: F) -> String {
        if self.is_empty() {
            fallback()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clean_json_content, deterministic_strategy_draft, load_ai_gateway_config_from,
        strategy_draft_payload, AiGatewayConfig,
    };
    use std::collections::BTreeMap;

    fn env(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn ai_gateway_config_uses_tradeassembly_env_and_normalizes_v1_base_url() {
        let config = load_ai_gateway_config_from(&env(&[
            ("TRADEASSEMBLY_AI_PROVIDER", "openai-compatible"),
            ("TRADEASSEMBLY_AI_BASE_URL", "http://localhost:11434"),
            ("TRADEASSEMBLY_AI_API_KEY", "local-key"),
            ("TRADEASSEMBLY_AI_MODEL", "local-model"),
        ]));

        assert_eq!(
            config,
            AiGatewayConfig {
                provider: "openai-compatible".to_string(),
                base_url: "http://localhost:11434/v1".to_string(),
                api_key: "local-key".to_string(),
                model: "local-model".to_string(),
                timeout_seconds: 30.0,
            }
        );
    }

    #[test]
    fn deterministic_strategy_draft_preserves_non_advisory_stub_shape() {
        let draft = deterministic_strategy_draft("Draft a BTC paper test", "crypto", "intraday");

        assert_eq!(draft["symbol"], "BTC/USD");
        assert_eq!(draft["provider_ref"], "local-data");
        assert_eq!(draft["entry_rule"], "user_defined_btc_momentum_entry");
        assert_eq!(draft["risk"]["max_notional"], 25);
        assert!(draft["notes"][0]
            .as_str()
            .expect("note")
            .contains("User owns all market selection"));
    }

    #[test]
    fn strategy_draft_payload_adds_source_and_model_for_runtime_facade() {
        let config = AiGatewayConfig {
            provider: "stub".to_string(),
            base_url: "".to_string(),
            api_key: "".to_string(),
            model: "tradeassembly-local-draft".to_string(),
            timeout_seconds: 30.0,
        };

        let payload = strategy_draft_payload(
            &config,
            "Draft a local BTC paper test",
            "crypto",
            "intraday",
        )
        .expect("payload");

        assert_eq!(payload["source"], "stub");
        assert_eq!(payload["model"], "deterministic-stub");
        assert_eq!(payload["entry_rule"], "user_defined_btc_momentum_entry");
    }

    #[test]
    fn clean_json_content_accepts_plain_and_fenced_json_objects() {
        assert_eq!(
            clean_json_content(r#"{"symbol":"SPY","asset_class":"equity"}"#).expect("plain")
                ["symbol"],
            "SPY"
        );
        assert_eq!(
            clean_json_content("```json\n{\"symbol\":\"BTC/USD\"}\n```").expect("fenced")["symbol"],
            "BTC/USD"
        );
    }
}
