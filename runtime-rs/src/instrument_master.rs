// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

const SCHEMA_VERSION: &str = "tradeassembly.instrument_context.v1";
const NOTICE: &str = "Instrument context is identity metadata only. Users decide strategy logic, risk scale, credentials, and activation.";

pub fn resolve_instrument(
    symbol: &str,
    asset_class: &str,
    provider_ref: &str,
    provider_symbol: Option<&str>,
    existing_aliases: &[Value],
) -> Value {
    let asset = asset_class.trim().to_lowercase();
    let provider_symbol = normalize_provider_symbol(provider_symbol.unwrap_or(symbol));
    let provenance = json!([{"source": "input", "providerRef": provider_ref}]);
    if !matches!(asset.as_str(), "equity" | "crypto" | "option") {
        let instrument = json!({
            "canonicalId": format!("unsupported:{}:{}", if asset.is_empty() { "unknown" } else { &asset }, provider_symbol),
            "symbol": provider_symbol,
            "assetClass": if asset.is_empty() { "unknown" } else { &asset },
            "instrumentFamily": "unsupported",
            "stale": false,
            "ambiguous": false,
            "unsupported": true,
            "unsupportedReason": "asset_class_not_supported",
            "corporateActionAdjustmentVersion": "unadjusted",
            "provenance": provenance
        });
        return context(
            "resolve",
            json!({"symbol": symbol, "assetClass": asset}),
            provider_ref,
            instrument,
            json!([]),
            json!([{"code": "unsupported_asset_class", "message": "Asset class is not supported by the local instrument master."}]),
        );
    }

    let normalized = normalize_instrument_symbol(symbol, &asset);
    let canonical_id = canonical_instrument_id(&normalized, &asset);
    let existing_canonical_ids = existing_aliases
        .iter()
        .filter_map(|alias| alias.get("canonicalId").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    let ambiguous = !existing_canonical_ids.is_empty()
        && existing_canonical_ids != std::collections::BTreeSet::from([canonical_id.as_str()]);
    let aliases = if ambiguous {
        Value::Array(existing_aliases.to_vec())
    } else {
        json!([{
            "canonicalId": canonical_id,
            "providerRef": provider_ref,
            "providerSymbol": provider_symbol,
            "aliasType": "primary",
            "provenance": provenance
        }])
    };
    let warnings = if ambiguous {
        json!([{"code": "ambiguous_alias", "message": "Provider alias maps to more than one canonical instrument."}])
    } else {
        json!([])
    };
    let mut instrument = canonical_instrument(&normalized, &asset, ambiguous, provenance);
    instrument["warnings"] = warnings.clone();
    context(
        "resolve",
        json!({"symbol": symbol, "assetClass": asset}),
        provider_ref,
        instrument,
        aliases,
        warnings,
    )
}

pub fn instrument_resolve_cli(symbol: &str, asset_class: &str, provider_ref: &str) -> Value {
    resolve_instrument(symbol, asset_class, provider_ref, None, &[])
}

pub fn instrument_resolve_mcp(symbol: &str, asset_class: &str, provider_ref: &str) -> Value {
    instrument_resolve_cli(symbol, asset_class, provider_ref)
}

pub fn instrument_resolve_acp(symbol: &str, asset_class: &str, provider_ref: &str) -> Value {
    instrument_resolve_cli(symbol, asset_class, provider_ref)
}

pub fn instrument_resolve_http(symbol: &str, asset_class: &str, provider_ref: &str) -> Value {
    instrument_resolve_cli(symbol, asset_class, provider_ref)
}

fn context(
    operation: &str,
    input: Value,
    provider_ref: &str,
    instrument: Value,
    aliases: Value,
    warnings: Value,
) -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "operation": operation,
        "input": input,
        "providerRef": provider_ref,
        "canonicalId": instrument["canonicalId"],
        "assetClass": instrument["assetClass"],
        "instrument": instrument,
        "aliases": aliases,
        "provenance": instrument["provenance"],
        "warnings": warnings,
        "ambiguity": {"ambiguous": instrument["ambiguous"]},
        "stale": instrument["stale"],
        "unsupportedReason": instrument["unsupportedReason"],
        "corporateActionAdjustmentVersion": instrument["corporateActionAdjustmentVersion"],
        "confidence": {"level": "deterministic", "meaning": "Identity resolution confidence only; not a trading signal."},
        "notice": NOTICE
    })
}

fn normalize_instrument_symbol(symbol: &str, asset_class: &str) -> Value {
    let text = symbol.trim().to_uppercase().replace(' ', "");
    match asset_class {
        "crypto" => {
            let compact = text.replace('-', "/");
            let (base, quote) = if let Some((base, quote)) = compact.split_once('/') {
                (
                    base.to_string(),
                    if quote.is_empty() {
                        "USD".to_string()
                    } else {
                        quote.to_string()
                    },
                )
            } else if compact.len() >= 6 {
                let split_at = compact.len() - 3;
                (
                    compact[..split_at].to_string(),
                    compact[split_at..].to_string(),
                )
            } else {
                (compact, "USD".to_string())
            };
            json!({"symbol": format!("{base}/{quote}"), "base": base, "quote": quote})
        }
        "option" => parse_occ_option(&text),
        _ => json!({"symbol": text}),
    }
}

fn canonical_instrument_id(normalized: &Value, asset_class: &str) -> String {
    match asset_class {
        "crypto" => format!(
            "ul:crypto:{}-{}",
            normalized["base"].as_str().unwrap_or(""),
            normalized["quote"].as_str().unwrap_or("")
        ),
        "option" => format!(
            "opt:ul:equity:{}:{}:{}:{}:unadjusted",
            normalized["underlying"].as_str().unwrap_or(""),
            normalized["expiry"].as_str().unwrap_or(""),
            normalized["right"].as_str().unwrap_or(""),
            strike_micros(normalized["strike"].as_f64().unwrap_or(0.0))
        ),
        _ => format!("ul:equity:{}", normalized["symbol"].as_str().unwrap_or("")),
    }
}

fn canonical_instrument(
    normalized: &Value,
    asset_class: &str,
    ambiguous: bool,
    provenance: Value,
) -> Value {
    let canonical_id = canonical_instrument_id(normalized, asset_class);
    match asset_class {
        "crypto" => json!({
            "canonicalId": canonical_id,
            "symbol": normalized["symbol"],
            "assetClass": "crypto",
            "instrumentFamily": "crypto_pair",
            "stale": false,
            "ambiguous": ambiguous,
            "unsupported": false,
            "unsupportedReason": null,
            "corporateActionAdjustmentVersion": "unadjusted",
            "provenance": provenance
        }),
        "option" => json!({
            "canonicalId": canonical_id,
            "symbol": normalized["symbol"],
            "assetClass": "option",
            "instrumentFamily": "equity_option",
            "underlyingCanonicalId": format!("ul:equity:{}", normalized["underlying"].as_str().unwrap_or("")),
            "stale": false,
            "ambiguous": ambiguous,
            "unsupported": false,
            "unsupportedReason": null,
            "corporateActionAdjustmentVersion": "unadjusted",
            "provenance": provenance
        }),
        _ => json!({
            "canonicalId": canonical_id,
            "symbol": normalized["symbol"],
            "assetClass": "equity",
            "instrumentFamily": "equity",
            "stale": false,
            "ambiguous": ambiguous,
            "unsupported": false,
            "unsupportedReason": null,
            "corporateActionAdjustmentVersion": "unadjusted",
            "provenance": provenance
        }),
    }
}

fn parse_occ_option(text: &str) -> Value {
    let underlying_len = text.len().saturating_sub(15);
    let underlying = &text[..underlying_len];
    let yymmdd = &text[underlying_len..underlying_len + 6];
    let right = &text[underlying_len + 6..underlying_len + 7];
    let strike_text = &text[underlying_len + 7..];
    let year = 2000 + yymmdd[0..2].parse::<i32>().unwrap_or_default();
    let month = yymmdd[2..4].parse::<i32>().unwrap_or_default();
    let day = yymmdd[4..6].parse::<i32>().unwrap_or_default();
    let strike = strike_text.parse::<f64>().unwrap_or_default() / 1000.0;
    json!({
        "symbol": text,
        "underlying": underlying,
        "expiry": format!("{year:04}-{month:02}-{day:02}"),
        "right": right,
        "strike": strike
    })
}

fn normalize_provider_symbol(symbol: &str) -> String {
    symbol.trim().to_uppercase().replace(' ', "")
}

fn strike_micros(strike: f64) -> i64 {
    (strike * 1_000_000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn instrument_master_normalizes_canonical_identity_and_alias_payload() {
        let result = super::resolve_instrument(" spy ", "equity", "alpaca-paper", Some("SPY"), &[]);

        assert_eq!(
            result["schemaVersion"],
            "tradeassembly.instrument_context.v1"
        );
        assert_eq!(result["instrument"]["canonicalId"], "ul:equity:SPY");
        assert_eq!(result["instrument"]["symbol"], "SPY");
        assert_eq!(result["instrument"]["assetClass"], "equity");
        assert_eq!(result["instrument"]["stale"], false);
        assert_eq!(result["instrument"]["ambiguous"], false);
        assert_eq!(result["instrument"]["unsupported"], false);
        assert_eq!(
            result["instrument"]["corporateActionAdjustmentVersion"],
            "unadjusted"
        );
        assert_eq!(
            result["aliases"],
            json!([{
                "canonicalId": "ul:equity:SPY",
                "providerRef": "alpaca-paper",
                "providerSymbol": "SPY",
                "aliasType": "primary",
                "provenance": [{"source": "input", "providerRef": "alpaca-paper"}]
            }])
        );
        assert_eq!(result["warnings"], json!([]));
        assert!(result["notice"]
            .as_str()
            .unwrap()
            .contains("identity metadata only"));
    }

    #[test]
    fn instrument_master_normalizes_crypto_and_occ_option_ids() {
        let crypto = super::resolve_instrument("btc/usd", "crypto", "alpaca-crypto", None, &[]);
        let option = super::resolve_instrument("SPY240621C00500000", "option", "opra", None, &[]);

        assert_eq!(crypto["instrument"]["canonicalId"], "ul:crypto:BTC-USD");
        assert_eq!(crypto["instrument"]["symbol"], "BTC/USD");
        assert_eq!(
            option["instrument"]["canonicalId"],
            "opt:ul:equity:SPY:2024-06-21:C:500000000:unadjusted"
        );
        assert_eq!(
            option["instrument"]["underlyingCanonicalId"],
            "ul:equity:SPY"
        );
        assert_eq!(option["instrument"]["instrumentFamily"], "equity_option");
    }

    #[test]
    fn instrument_master_warning_cases_do_not_emit_trade_advice() {
        let existing_aliases = json!([
            {"canonicalId": "ul:equity:ABC.A", "providerRef": "demo", "providerSymbol": "ABC", "aliasType": "conflicting", "provenance": [{"source": "fixture", "providerRef": "demo"}]},
            {"canonicalId": "ul:equity:ABC.B", "providerRef": "demo", "providerSymbol": "ABC", "aliasType": "conflicting", "provenance": [{"source": "fixture", "providerRef": "demo"}]}
        ]);
        let ambiguous = super::resolve_instrument(
            "ABC",
            "equity",
            "demo",
            None,
            existing_aliases.as_array().unwrap(),
        );
        let unsupported = super::resolve_instrument("EUR/USD", "fx", "demo", None, &[]);

        assert_eq!(ambiguous["instrument"]["ambiguous"], true);
        assert_eq!(ambiguous["warnings"][0]["code"], "ambiguous_alias");
        assert_eq!(unsupported["instrument"]["unsupported"], true);
        assert_eq!(
            unsupported["instrument"]["unsupportedReason"],
            "asset_class_not_supported"
        );
        assert!(!unsupported["instrument"]["canonicalId"]
            .as_str()
            .unwrap()
            .starts_with("ol:"));
        let rendered =
            serde_json::to_string(&json!({"ambiguous": ambiguous, "unsupported": unsupported}))
                .unwrap()
                .to_lowercase();
        for term in [
            "buy ",
            "sell ",
            "hold ",
            "entry ",
            "exit ",
            "position size",
            "allocation recommendation",
            "api_secret",
            "api_key",
        ] {
            assert!(!rendered.contains(term), "{term}");
        }
    }

    #[test]
    fn instrument_master_cli_agent_and_http_facades_share_payload_shape() {
        let cli = super::instrument_resolve_cli(" spy ", "equity", "alpaca-paper");
        let mcp = super::instrument_resolve_mcp(" spy ", "equity", "alpaca-paper");
        let acp = super::instrument_resolve_acp(" spy ", "equity", "alpaca-paper");
        let http = super::instrument_resolve_http(" spy ", "equity", "alpaca-paper");

        assert_eq!(cli["instrument"]["canonicalId"], "ul:equity:SPY");
        assert_eq!(mcp["instrument"], cli["instrument"]);
        assert_eq!(acp["aliases"], cli["aliases"]);
        assert_eq!(http["warnings"], cli["warnings"]);
        let cli_keys = sorted_keys(&cli);
        assert_eq!(sorted_keys(&mcp), cli_keys);
        assert_eq!(sorted_keys(&acp), cli_keys);
        assert_eq!(sorted_keys(&http), cli_keys);
    }

    fn sorted_keys(value: &serde_json::Value) -> Vec<String> {
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }
}
