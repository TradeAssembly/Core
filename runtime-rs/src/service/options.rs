use serde_json::{json, Value};

pub(crate) fn chain(body: Value) -> Value {
    let underlying = body
        .get("underlying")
        .and_then(Value::as_str)
        .unwrap_or("SPY");
    let right = body.get("right").and_then(Value::as_str).unwrap_or("CALL");
    let provider_ref = body
        .get("providerRef")
        .or_else(|| body.get("provider_ref"))
        .and_then(Value::as_str)
        .unwrap_or("local-data");
    json!({
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
            let delta = contract["delta"].as_f64().unwrap_or(0.0);
            delta >= min_delta && delta <= max_delta
        })
        .cloned()
        .unwrap_or_else(|| contracts.first().cloned().unwrap_or_else(|| json!({})));
    json!({
        "selected": selected,
        "rejected": [],
        "inputs": {"minDelta": min_delta, "maxDelta": max_delta},
        "noAdvice": true,
    })
}
