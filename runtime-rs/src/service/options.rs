use serde_json::{json, Value};

pub(crate) fn chain(body: Value) -> Value {
    if body["fixture"] != true {
        return json!({"status":"unavailable","code":"option_chain_provider_required","sourceClass":"unavailable","contracts":[],"executionAuthorityGranted":false,"selectionTool":"tradeassembly.strategy.option_selection.create"});
    }

    let underlying = body
        .get("underlying")
        .and_then(Value::as_str)
        .unwrap_or("SPY");
    let right = body.get("right").and_then(Value::as_str).unwrap_or("CALL");
    let provider_ref = "fixture";
    json!({
        "sourceClass":"deterministic_fixture",
        "executionAuthorityGranted":false,
        "underlying": underlying,
        "provider_ref": provider_ref,
        "providerRef": provider_ref,
        "asof": "2026-01-02T00:00:00+00:00",
        "contracts": [
            {"symbol": format!("{underlying}260717C00520000"), "underlying": underlying, "expiry": "2026-07-17", "strike": 520.0, "right": right, "bid": 4.20, "ask": 4.35, "delta": 0.52},
            {"symbol": format!("{underlying}260717C00530000"), "underlying": underlying, "expiry": "2026-07-17", "strike": 530.0, "right": right, "bid": 2.90, "ask": 3.05, "delta": 0.36}
        ],
        "noAdvice": true,
    })
}

pub(crate) fn select(body: Value) -> Value {
    let chain = chain(body.clone());
    if chain["sourceClass"] != "deterministic_fixture" {
        return chain;
    }

    let min_delta = body
        .get("minDelta")
        .or_else(|| body.get("min_delta"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let max_delta = body
        .get("maxDelta")
        .or_else(|| body.get("max_delta"))
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let contracts = chain["contracts"].as_array().cloned().unwrap_or_default();
    let selected = contracts
        .iter()
        .find(|contract| {
            contract["delta"]
                .as_f64()
                .is_some_and(|delta| delta >= min_delta && delta <= max_delta)
        })
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "status": if selected.is_null() {"no_eligible_contract"}else{"selected"},
        "sourceClass":"deterministic_fixture",
        "executionAuthorityGranted":false,
        "selected": selected,
        "rejected": [],
        "inputs": {"minDelta": min_delta, "maxDelta": max_delta},
        "noAdvice": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_options_require_explicit_fixture_and_never_fall_back() {
        assert_eq!(
            chain(json!({"providerRef":"real-provider"}))["status"],
            "unavailable"
        );
        assert_eq!(select(json!({}))["status"], "unavailable");
        let fixture = chain(json!({"fixture":true,"providerRef":"real-provider"}));
        assert_eq!(fixture["sourceClass"], "deterministic_fixture");
        assert_eq!(fixture["providerRef"], "fixture");
        let none = select(json!({"fixture":true,"minDelta":0.8,"maxDelta":0.9}));
        assert_eq!(none["status"], "no_eligible_contract");
        assert!(none["selected"].is_null());
        assert_eq!(none["executionAuthorityGranted"], false);
    }
}
