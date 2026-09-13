//! Exact identity checks shared by submission and read-only recovery evidence.
//! Identity equality does not imply a fill or authorize another submission.

use crate::broker_order_intent::CanonicalBrokerOrder;
use crate::ports::PluginOperationResponse;
use serde_json::{json, Value};

pub(crate) fn validate_order_identity(
    intent: &Value,
    response: &PluginOperationResponse,
) -> Result<(), String> {
    let invalid = || "broker_order_observation_mismatch".to_string();
    if response.reconciliation_required {
        return Err("plugin_order_reconciliation_required".into());
    }
    let payload = &response.payload;
    let account = intent["accountRef"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(invalid)?;
    let provider = response
        .provider_outcome_id
        .as_deref()
        .filter(|v| !v.is_empty() && v.len() <= 256)
        .ok_or_else(invalid)?;
    if payload["accountRef"] != account || payload["providerOrderId"] != provider {
        return Err(invalid());
    }
    // Select identity fields only; broker status and fill data remain observations.
    // The existing canonical parser performs checked decimal arithmetic, no f64.
    let observed = CanonicalBrokerOrder::from_input(&json!({
        "symbol":payload["symbol"], "side":payload["side"],
        "orderType":payload["orderType"], "timeInForce":payload["timeInForce"],
        "clientOrderId":payload["clientOrderId"], "quantity":payload["quantity"]
    }))
    .map_err(|_| invalid())?;
    let original = CanonicalBrokerOrder::from_input(&intent["order"]).map_err(|_| invalid())?;
    if observed != original {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Value, PluginOperationResponse) {
        let intent = json!({"accountRef":"account://controlled", "order":{
            "symbol":"FIXTURE", "side":"buy", "orderType":"market",
            "timeInForce":"day", "clientOrderId":"original", "quantityMicros":1000001
        }});
        let response: PluginOperationResponse = serde_json::from_value(json!({
            "correlationId":"lookup", "schemaRef":"test", "observedAtMs":1,
            "sourceEventId":"test", "contentHash":"test", "freshnessState":"fresh",
            "deterministic":false, "replayable":true, "providerOutcomeId":"provider",
            "reconciliationRequired":false, "payload":{
                "accountRef":"account://controlled", "providerOrderId":"provider",
                "symbol":"FIXTURE", "side":"buy", "orderType":"market",
                "timeInForce":"day", "clientOrderId":"original", "quantity":"1.000001",
                "status":"accepted"
            }
        }))
        .unwrap();
        (intent, response)
    }

    #[test]
    fn exact_identity_is_required_without_fabricating_a_fill() {
        let (intent, response) = fixture();
        validate_order_identity(&intent, &response).unwrap();
        assert_eq!(response.payload["status"], "accepted");
        for (field, value) in [
            ("accountRef", json!("account://wrong")),
            ("providerOrderId", json!("wrong")),
            ("symbol", json!("OTHER")),
            ("side", json!("sell")),
            ("orderType", json!("limit")),
            ("timeInForce", json!("gtc")),
            ("clientOrderId", json!("wrong")),
            ("quantity", json!("1.000002")),
            ("quantity", json!("1.0000001")),
            ("quantity", Value::Null),
        ] {
            let mut changed = response.clone();
            changed.payload[field] = value;
            assert!(
                validate_order_identity(&intent, &changed).is_err(),
                "{field}"
            );
        }
        let mut changed = response.clone();
        changed.provider_outcome_id = None;
        assert!(validate_order_identity(&intent, &changed).is_err());
        changed = response;
        changed.reconciliation_required = true;
        assert!(validate_order_identity(&intent, &changed).is_err());
    }
}
