use super::{api_result, path_id, ServiceResponse, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::ports::{
    AuthorityContext, IdempotencyKey, PluginOperationRequest, PluginOperationResponse,
    SideEffectContext,
};
use serde_json::{json, Value};

const ORDERS_NS: &str = "execution_orders";
const ATTEMPTS_NS: &str = "execution_order_attempts";
const RUNS_NS: &str = "execution_runs";
const IDEMPOTENCY_NS: &str = "execution_idempotency";

pub(crate) fn run_once_response(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    if plugin_execution_requested(&body) && !submission_requested(&body) {
        return ServiceResponse::bad_request("plugin_order_submission_not_requested");
    }
    if legacy_broker_execution_requested(&body) && !plugin_execution_requested(&body) {
        return ServiceResponse::bad_request("plugin_order_binding_required");
    }
    if submission_requested(&body) && !has_authority(&body) {
        return ServiceResponse::forbidden("authority_required");
    }
    ServiceResponse::ok(api_result(run_once(service, body)))
}

pub(crate) fn run_once(service: &TradeAssemblyService, body: Value) -> Value {
    if plugin_execution_requested(&body) && !submission_requested(&body) {
        return json!({
            "status": "failed",
            "error": "plugin_order_submission_not_requested",
            "noAdvice": LEGAL_BOUNDARY,
        });
    }
    if legacy_broker_execution_requested(&body) && !plugin_execution_requested(&body) {
        return json!({
            "status": "failed",
            "error": "plugin_order_binding_required",
            "noAdvice": LEGAL_BOUNDARY,
        });
    }
    let idempotency_key = idempotency_key(&body);
    if let Some(key) = &idempotency_key {
        if let Ok(Some(value)) = service.runtime().storage.get_json(IDEMPOTENCY_NS, key) {
            let mut duplicate = value;
            duplicate["duplicate"] = json!(true);
            return duplicate;
        }
    }

    let submit_orders = submission_requested(&body);
    let order_id = idempotency_key
        .as_deref()
        .map(|key| format!("order_{}", slug(key)))
        .unwrap_or_else(|| format!("order_{}", next_count(service, ORDERS_NS)));
    let run_id = format!("run_{}", slug(&order_id));
    let symbol = body
        .get("symbol")
        .and_then(Value::as_str)
        .unwrap_or("BTC/USD");
    let qty = body.get("qty").and_then(Value::as_f64).unwrap_or(0.0002);
    let side = body.get("side").and_then(Value::as_str).unwrap_or("buy");
    let plugin_execution = plugin_execution_requested(&body);
    let status = if plugin_execution {
        "submission_pending"
    } else if submit_orders {
        "submitted"
    } else {
        "planned"
    };
    let client_order_id = body["pluginOperationRequest"]["input"]["clientOrderId"]
        .as_str()
        .or(idempotency_key.as_deref())
        .unwrap_or(&order_id)
        .to_string();
    let order = json!({
        "id": order_id,
        "order_id": order_id,
        "correlationId": correlation_id(&body),
        "client_order_id": client_order_id,
        "status": status,
        "symbol": symbol,
        "side": side,
        "qty": qty,
        "account_ref": body["pluginOperationRequest"].get("accountRef").cloned().unwrap_or_else(|| json!(account_mode(&body))),
        "activation_id": body.get("activationId").or_else(|| body.get("activation_id")).cloned().unwrap_or_else(|| json!("activation")),
        "strategy_id": body.get("strategyId").or_else(|| body.get("strategy_id")).cloned().unwrap_or(Value::Null),
        "source_observation_id": body.get("sourceObservationId").or_else(|| body.get("source_observation_id")).cloned().unwrap_or(Value::Null),
        "evidenceRefs": body.get("evidenceRefs").cloned().unwrap_or_else(|| json!([])),
        "eventSequence": 1,
        "submit": submit_orders,
        "accountMode": account_mode(&body),
        "brokerExecution": if plugin_execution {
            body["pluginOperationRequest"]["pluginRef"].as_str().unwrap_or("plugin")
        } else {
            "local-simulated"
        },
        "noAdvice": LEGAL_BOUNDARY,
    });
    persist_with_body(
        service,
        ORDERS_NS,
        &order_id,
        order.clone(),
        "broker.order_planned",
        &body,
    );
    persist_with_body(
        service,
        ATTEMPTS_NS,
        &format!("attempt_{order_id}"),
        json!({
            "id": format!("attempt_{order_id}"),
            "order_id": order_id,
            "correlationId": correlation_id(&body),
            "activation_id": order["activation_id"],
            "strategy_id": body.get("strategyId").or_else(|| body.get("strategy_id")).cloned().unwrap_or(Value::Null),
            "status": status,
        }),
        "broker.order_attempted",
        &body,
    );
    let mut order = order;
    if submit_orders {
        if let Err(error) = super::positions::validate_order_transition(service, &order) {
            order["status"] = json!("rejected");
            order["error"] = json!(&error);
            persist_with_body(
                service,
                ORDERS_NS,
                &order_id,
                order.clone(),
                "paper.position.transition_rejected",
                &body,
            );
            return failed_order_result(&body, &run_id, &order_id, order, &error);
        }
    }
    if submit_orders && super::risk::reserve_for_order(service, &order).is_err() {
        order["status"] = json!("risk_reservation_failed");
        persist_with_body(
            service,
            ORDERS_NS,
            &order_id,
            order.clone(),
            "risk.reservation_failed",
            &body,
        );
        return json!({
            "id": run_id,
            "run_id": run_id,
            "order_id": order_id,
            "activation_id": body.get("activationId").or_else(|| body.get("activation_id")).cloned().unwrap_or_else(|| json!("activation")),
            "status": "failed",
            "duplicate": false,
            "order": order,
            "error": "risk_reservation_failed",
        });
    }
    if plugin_execution {
        let context = SideEffectContext::new(
            authority_from_body(&body),
            IdempotencyKey::new(
                idempotency_key
                    .clone()
                    .unwrap_or_else(|| format!("plugin:{order_id}")),
            )
            .expect("valid plugin idempotency key"),
        )
        .with_agent_execution(service.agent_mcp_execution_context());
        let result = serde_json::from_value::<PluginOperationRequest>(
            body["pluginOperationRequest"].clone(),
        )
        .map_err(|_| "plugin_operation_request_invalid".to_string())
        .and_then(|request| {
            service
                .runtime()
                .plugin_operations
                .invoke(&request, &context)
        });
        match result {
            Ok(receipt) => {
                if receipt.reconciliation_required {
                    order["status"] = json!("reconciliation_required");
                    order["pluginError"] = json!("plugin_order_reconciliation_required");
                    order["secretsRedacted"] = json!(true);
                    order["pluginReceipt"] =
                        serde_json::to_value(receipt).unwrap_or_else(|_| json!({"redacted": true}));
                    persist_with_body(
                        service,
                        ORDERS_NS,
                        &order_id,
                        order.clone(),
                        "plugin.order.reconciliation_required",
                        &body,
                    );
                    return failed_order_result(
                        &body,
                        &run_id,
                        &order_id,
                        order,
                        "plugin_order_reconciliation_required",
                    );
                }
                let require_exact_simulation_price =
                    body["pluginOperationRequest"]["pluginRef"] == "tradeassembly.simbroker";
                let expected_simulation_price_micros =
                    body["pluginOperationRequest"]["input"]["fillPriceMicros"].as_i64();
                let canonical = match canonical_order_receipt(
                    &receipt,
                    &client_order_id,
                    symbol,
                    side,
                    qty,
                    require_exact_simulation_price,
                    expected_simulation_price_micros,
                ) {
                    Ok(canonical) => canonical,
                    Err(error) => {
                        order["status"] = json!("submission_failed");
                        order["pluginError"] = json!(error);
                        order["secretsRedacted"] = json!(true);
                        persist_with_body(
                            service,
                            ORDERS_NS,
                            &order_id,
                            order.clone(),
                            "plugin.order.receipt_invalid",
                            &body,
                        );
                        return failed_order_result(&body, &run_id, &order_id, order, error);
                    }
                };
                order["status"] = json!(canonical.status);
                order["provider_order_id"] = json!(canonical.provider_order_id);
                order["client_order_id"] = json!(canonical.client_order_id);
                order["filled_qty"] = json!(canonical.filled_quantity);
                order["fill_price"] = canonical.fill_price.map_or(Value::Null, Value::from);
                order["pluginReceipt"] =
                    serde_json::to_value(receipt).unwrap_or_else(|_| json!({"redacted": true}));
                persist_with_body(
                    service,
                    ORDERS_NS,
                    &order_id,
                    order.clone(),
                    "plugin.order.submitted",
                    &body,
                );
            }
            Err(error) if error == "plugin_order_reconciliation_required" => {
                order["status"] = json!("reconciliation_required");
                order["pluginError"] = json!("plugin_order_reconciliation_required");
                order["secretsRedacted"] = json!(true);
                persist_with_body(
                    service,
                    ORDERS_NS,
                    &order_id,
                    order.clone(),
                    "plugin.order.reconciliation_required",
                    &body,
                );
                return failed_order_result(
                    &body,
                    &run_id,
                    &order_id,
                    order,
                    "plugin_order_reconciliation_required",
                );
            }
            Err(_) => {
                order["status"] = json!("submission_failed");
                order["pluginError"] = json!("plugin_operation_failed");
                order["secretsRedacted"] = json!(true);
                persist_with_body(
                    service,
                    ORDERS_NS,
                    &order_id,
                    order.clone(),
                    "plugin.order.submission_failed",
                    &body,
                );
                return failed_order_result(
                    &body,
                    &run_id,
                    &order_id,
                    order,
                    "plugin_order_submission_failed",
                );
            }
        }
    }
    if submit_orders && order["filled_qty"].as_f64().is_some_and(|qty| qty > 0.0) {
        if let Err(error) = super::positions::apply_filled_order(service, &order) {
            order["status"] = json!("reconciliation_required");
            order["positionAccountingError"] = json!(&error);
            persist_with_body(
                service,
                ORDERS_NS,
                &order_id,
                order.clone(),
                "paper.position.accounting_failed",
                &body,
            );
            return failed_order_result(&body, &run_id, &order_id, order, &error);
        }
    }
    let result = json!({
        "id": run_id,
        "run_id": run_id,
        "correlationId": correlation_id(&body),
        "order_id": order_id,
        "entry_order_id": order_id,
        "activation_id": body.get("activationId").or_else(|| body.get("activation_id")).cloned().unwrap_or_else(|| json!("activation-btc-exit-demo")),
        "submit_exit": body.get("submitExit").or_else(|| body.get("submit_exit")).and_then(Value::as_bool).unwrap_or(false),
        "submit_orders": submit_orders,
        "status": "completed",
        "duplicate": false,
        "order": order,
        "risk": {"reserved": submit_orders, "policy": "local"},
        "journal": [{"event_type": if submit_orders { "broker.order_submitted" } else { "broker.order_planned" }, "idempotency_key": idempotency_key}],
    });
    persist_with_body(
        service,
        RUNS_NS,
        &run_id,
        result.clone(),
        "execution.run_completed",
        &body,
    );
    if let Some(key) = idempotency_key {
        persist_with_body(
            service,
            IDEMPOTENCY_NS,
            &key,
            result.clone(),
            "execution.idempotency_recorded",
            &body,
        );
    }
    result
}

fn correlation_id(body: &Value) -> Value {
    body.get("controlPlaneCorrelationId")
        .or_else(|| body.get("controlPlaneCommandId"))
        .cloned()
        .unwrap_or(Value::Null)
}

#[derive(Debug)]
struct CanonicalOrderReceipt {
    status: String,
    provider_order_id: String,
    client_order_id: String,
    filled_quantity: f64,
    fill_price: Option<f64>,
}

fn canonical_order_receipt(
    receipt: &PluginOperationResponse,
    expected_client_order_id: &str,
    expected_symbol: &str,
    expected_side: &str,
    expected_quantity: f64,
    require_exact_simulation_price: bool,
    expected_simulation_price_micros: Option<i64>,
) -> Result<CanonicalOrderReceipt, &'static str> {
    let payload = receipt
        .payload
        .as_object()
        .ok_or("plugin_order_receipt_invalid")?;
    let string = |field: &str| {
        payload
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
    };
    let provider_order_id = string("providerOrderId").ok_or("plugin_order_receipt_invalid")?;
    let client_order_id = string("clientOrderId").ok_or("plugin_order_receipt_invalid")?;
    let symbol = string("symbol").ok_or("plugin_order_receipt_invalid")?;
    let side = string("side").ok_or("plugin_order_receipt_invalid")?;
    let status = string("status").ok_or("plugin_order_receipt_invalid")?;
    if client_order_id != expected_client_order_id
        || symbol != expected_symbol
        || side != expected_side
        || !canonical_order_status(status)
    {
        return Err("plugin_order_receipt_binding_invalid");
    }
    if let Some(quantity) = order_quantity(payload, "quantity", "quantityMicros") {
        if !approximately_equal(quantity, expected_quantity) {
            return Err("plugin_order_receipt_binding_invalid");
        }
    }
    let filled_quantity =
        order_quantity(payload, "filledQuantity", "filledQuantityMicros").unwrap_or(0.0);
    let fill_price = if require_exact_simulation_price {
        let expected = expected_simulation_price_micros
            .filter(|expected| *expected > 0)
            .ok_or("plugin_order_receipt_invalid")?;
        if payload.get("fillPriceMicros").and_then(Value::as_i64) != Some(expected)
            || payload.contains_key("fillPrice")
        {
            return Err("plugin_order_receipt_invalid");
        }
        Some(expected as f64 / 1_000_000.0)
    } else {
        order_quantity(payload, "fillPrice", "fillPriceMicros")
    };
    if !filled_quantity.is_finite()
        || filled_quantity < 0.0
        || filled_quantity > expected_quantity
        || matches!(status, "filled" | "partially_filled") && filled_quantity == 0.0
        || filled_quantity > 0.0 && fill_price.is_none_or(|price| price <= 0.0)
    {
        return Err("plugin_order_receipt_invalid");
    }
    Ok(CanonicalOrderReceipt {
        status: status.to_string(),
        provider_order_id: provider_order_id.to_string(),
        client_order_id: client_order_id.to_string(),
        filled_quantity,
        fill_price,
    })
}

fn canonical_order_status(status: &str) -> bool {
    matches!(
        status,
        "new"
            | "accepted"
            | "pending"
            | "pending_new"
            | "submitted"
            | "partially_filled"
            | "filled"
            | "done_for_day"
            | "canceled"
            | "expired"
            | "replaced"
            | "pending_cancel"
            | "pending_replace"
            | "rejected"
            | "suspended"
            | "calculated"
            | "stopped"
    )
}

fn order_quantity(
    payload: &serde_json::Map<String, Value>,
    decimal_field: &str,
    micros_field: &str,
) -> Option<f64> {
    payload
        .get(decimal_field)
        .and_then(decimal_value)
        .or_else(|| {
            payload
                .get(micros_field)
                .and_then(decimal_value)
                .map(|micros| micros / 1_000_000.0)
        })
}

fn decimal_value(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<f64>().ok()))
        .filter(|value| value.is_finite())
}

fn approximately_equal(left: f64, right: f64) -> bool {
    (left - right).abs() <= 1e-9_f64.max(right.abs() * 1e-9)
}

fn failed_order_result(
    body: &Value,
    run_id: &str,
    order_id: &str,
    order: Value,
    error: &str,
) -> Value {
    json!({
        "id": run_id,
        "run_id": run_id,
        "order_id": order_id,
        "activation_id": body.get("activationId").or_else(|| body.get("activation_id")).cloned().unwrap_or_else(|| json!("activation")),
        "status": "failed",
        "duplicate": false,
        "order": order,
        "error": error,
    })
}

pub(crate) fn list_orders(service: &TradeAssemblyService) -> Vec<Value> {
    list_or_default(service, ORDERS_NS, sample_order())
}

pub(crate) fn stored_orders(service: &TradeAssemblyService) -> Vec<Value> {
    stored_values(service, ORDERS_NS)
}

pub(crate) fn list_attempts(service: &TradeAssemblyService) -> Vec<Value> {
    list_or_default(
        service,
        ATTEMPTS_NS,
        json!({"id": "attempt_local", "order_id": "order_local_0001", "status": "submitted"}),
    )
}

pub(crate) fn stored_attempts(service: &TradeAssemblyService) -> Vec<Value> {
    stored_values(service, ATTEMPTS_NS)
}

pub(crate) fn status_event(service: &TradeAssemblyService, path: &str, body: Value) -> Value {
    let order_id = path_id(path);
    let Some(mut order) = service
        .runtime()
        .storage
        .get_json(ORDERS_NS, order_id)
        .ok()
        .flatten()
    else {
        return json!({"order_id": order_id, "accepted": false, "reason": "unknown_order"});
    };
    let incoming_sequence = body
        .get("eventSequence")
        .or_else(|| body.get("event_sequence"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let current_sequence = order["eventSequence"].as_u64().unwrap_or(0);
    if incoming_sequence < current_sequence {
        return json!({"order_id": order_id, "accepted": false, "reason": "stale_event"});
    }
    order["status"] = body
        .get("status")
        .cloned()
        .unwrap_or_else(|| json!("submitted"));
    order["eventSequence"] = json!(incoming_sequence);
    persist(
        service,
        ORDERS_NS,
        order_id,
        order.clone(),
        "broker.order_status_updated",
    );
    json!({"order_id": order_id, "accepted": true, "payload": body, "order": order})
}

pub(crate) fn reconcile(service: &TradeAssemblyService) -> Vec<Value> {
    list_orders(service)
        .into_iter()
        .map(|mut order| {
            // This route has no trusted current broker lookup. A receipt or a
            // locally persisted order is only an observation, never evidence
            // that permits changing status or fill quantities.
            order["reconciled"] = json!(false);
            order["reconciliationSource"] = json!("unavailable");
            order["reconciliationRequired"] = json!(true);
            order["reconciliationReason"] = json!("broker_observation_required");
            order
        })
        .collect()
}

pub(crate) fn status_stream(service: &TradeAssemblyService) -> Value {
    json!({"events": list_orders(service), "watched": true})
}

fn list_or_default(service: &TradeAssemblyService, namespace: &str, default: Value) -> Vec<Value> {
    let values = stored_values(service, namespace);
    if values.is_empty() {
        vec![default]
    } else {
        values
    }
}

fn stored_values(service: &TradeAssemblyService, namespace: &str) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

fn submission_requested(body: &Value) -> bool {
    body.get("submitOrders")
        .or_else(|| body.get("submit_orders"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn legacy_broker_execution_requested(body: &Value) -> bool {
    submission_requested(body)
        && body
            .get("paperBrokerExecution")
            .or_else(|| body.get("paper_broker_execution"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn plugin_execution_requested(body: &Value) -> bool {
    body.get("pluginOperationRequest")
        .is_some_and(Value::is_object)
}

fn has_authority(body: &Value) -> bool {
    idempotency_key(body).is_some()
        && body
            .get("controlPlaneExplicitAuthority")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        && body
            .get("authorityContext")
            .or_else(|| body.get("authority_context"))
            .is_some_and(Value::is_object)
        && matches!(account_mode(body).as_str(), "paper" | "live")
}

fn idempotency_key(body: &Value) -> Option<String> {
    body.get("idempotencyKey")
        .or_else(|| body.get("idempotency_key"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn account_mode(body: &Value) -> String {
    body.get("accountMode")
        .or_else(|| body.get("account_mode"))
        .and_then(Value::as_str)
        .unwrap_or("paper")
        .to_string()
}

fn persist(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    value: Value,
    event_type: &str,
) {
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("{event_type}:{key}")).expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(namespace, key, value.clone(), &context)
        .expect("persist execution item");
    let _ = service
        .runtime()
        .record_side_effect(event_type, value, &context);
}

fn persist_with_body(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    value: Value,
    event_type: &str,
    body: &Value,
) {
    let context = SideEffectContext::new(
        authority_from_body(body),
        IdempotencyKey::new(format!(
            "{}:{event_type}:{key}",
            idempotency_key(body).unwrap_or_else(|| "auto".to_string())
        ))
        .expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(namespace, key, value.clone(), &context)
        .expect("persist execution item");
    let _ = service
        .runtime()
        .record_side_effect(event_type, value, &context);
}

fn authority_from_body(body: &Value) -> AuthorityContext {
    let context = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    AuthorityContext {
        actor: context
            .and_then(|value| value.get("actor").or_else(|| value.get("principalUser")))
            .and_then(Value::as_str)
            .unwrap_or("local-user")
            .to_string(),
        surface: context
            .and_then(|value| value.get("surface"))
            .and_then(Value::as_str)
            .unwrap_or("runtime")
            .to_string(),
        account_mode: account_mode(body),
    }
}

fn next_count(service: &TradeAssemblyService, namespace: &str) -> usize {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .map(|items| items.len())
        .unwrap_or_default()
        + 1
}

fn sample_order() -> Value {
    json!({"id": "order_local_0001", "order_id": "order_local_0001", "status": "filled", "symbol": "BTC/USD", "side": "buy", "qty": 0.0002})
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(payload: Value) -> PluginOperationResponse {
        PluginOperationResponse {
            correlation_id: "correlation".to_string(),
            schema_ref: "schema://broker/order-receipt@1".to_string(),
            payload,
            observed_at_ms: 1,
            source_event_id: "receipt".to_string(),
            content_hash: "sha256:receipt".to_string(),
            freshness_state: "fresh".to_string(),
            evidence_refs: vec!["evidence://receipt".to_string()],
            deterministic: true,
            replayable: true,
            provider_outcome_id: Some("provider-order".to_string()),
            reconciliation_required: false,
        }
    }

    #[test]
    fn filled_receipt_requires_a_finite_positive_price() {
        let base = json!({
            "providerOrderId": "provider-order",
            "clientOrderId": "client-order",
            "symbol": "BTC/USD",
            "side": "buy",
            "quantity": 1.0,
            "filledQuantity": 1.0,
            "status": "filled",
        });
        assert_eq!(
            canonical_order_receipt(
                &receipt(base.clone()),
                "client-order",
                "BTC/USD",
                "buy",
                1.0,
                true,
                Some(101_250_000),
            )
            .expect_err("missing price must fail"),
            "plugin_order_receipt_invalid"
        );
        for price in [0.0, -1.0] {
            let mut invalid = base.clone();
            invalid["fillPrice"] = json!(price);
            assert_eq!(
                canonical_order_receipt(
                    &receipt(invalid),
                    "client-order",
                    "BTC/USD",
                    "buy",
                    1.0,
                    true,
                    Some(101_250_000),
                )
                .expect_err("non-positive price must fail"),
                "plugin_order_receipt_invalid"
            );
        }
        for price in ["NaN", "inf", "-inf"] {
            let mut invalid = base.clone();
            invalid["fillPrice"] = json!(price);
            assert_eq!(
                canonical_order_receipt(
                    &receipt(invalid),
                    "client-order",
                    "BTC/USD",
                    "buy",
                    1.0,
                    true,
                    Some(101_250_000),
                )
                .expect_err("non-finite price must fail"),
                "plugin_order_receipt_invalid"
            );
        }
        let mut valid = base;
        valid["fillPriceMicros"] = json!(101_250_000);
        let canonical = canonical_order_receipt(
            &receipt(valid.clone()),
            "client-order",
            "BTC/USD",
            "buy",
            1.0,
            true,
            Some(101_250_000),
        )
        .expect("matching positive price");
        assert_eq!(canonical.fill_price, Some(101.25));
        let mut conflicting = valid.clone();
        conflicting["fillPrice"] = json!(999.0);
        assert_eq!(
            canonical_order_receipt(
                &receipt(conflicting),
                "client-order",
                "BTC/USD",
                "buy",
                1.0,
                true,
                Some(101_250_000),
            )
            .expect_err("competing decimal price must fail"),
            "plugin_order_receipt_invalid"
        );
        for (price, source_class) in [(101_250_001, None), (102_000_000, Some("external_broker"))] {
            valid["fillPriceMicros"] = json!(price);
            if let Some(source_class) = source_class {
                valid["sourceClass"] = json!(source_class);
            } else {
                valid
                    .as_object_mut()
                    .expect("receipt object")
                    .remove("sourceClass");
            }
            assert_eq!(
                canonical_order_receipt(
                    &receipt(valid.clone()),
                    "client-order",
                    "BTC/USD",
                    "buy",
                    1.0,
                    true,
                    Some(101_250_000),
                )
                .expect_err("mismatched simulated price must fail"),
                "plugin_order_receipt_invalid"
            );
        }
    }

    #[test]
    fn reconcile_only_reports_missing_broker_observation() {
        let dir = tempfile::tempdir().expect("isolated order fixture");
        let db = dir.path().join("runtime.db");
        let service = TradeAssemblyService::test_local(db.to_string_lossy());
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("orders-reconcile-truth-fixture").expect("valid idempotency key"),
        );
        for (id, status, filled_qty) in [
            ("planned-order", "planned", Value::Null),
            ("plugin-order", "new", json!(0.25)),
            ("filled-order", "filled", json!(1.0)),
        ] {
            service
                .runtime()
                .storage
                .put_json(
                    ORDERS_NS,
                    id,
                    json!({
                        "order_id": id,
                        "status": status,
                        "filled_qty": filled_qty,
                        "provider_order_id": "provider-identity",
                    }),
                    &context,
                )
                .expect("persist order fixture");
        }

        let before = service.runtime().storage.list_json(ORDERS_NS).unwrap();
        let first = reconcile(&service);
        for (id, status, filled_qty) in [
            ("planned-order", "planned", Value::Null),
            ("plugin-order", "new", json!(0.25)),
            ("filled-order", "filled", json!(1.0)),
        ] {
            let order = first
                .iter()
                .find(|order| order["order_id"] == id)
                .expect("reconciled observation");
            assert_eq!(order["status"], status);
            assert_eq!(order["filled_qty"], filled_qty);
            assert_eq!(order["provider_order_id"], "provider-identity");
            assert_eq!(order["reconciled"], false);
            assert_eq!(order["reconciliationSource"], "unavailable");
            assert_eq!(order["reconciliationRequired"], true);
            assert_eq!(order["reconciliationReason"], "broker_observation_required");
        }
        assert!(service
            .runtime()
            .storage
            .get_json("side_effects", "broker.order_reconciled")
            .expect("side effect lookup")
            .is_none());
        assert_eq!(first, reconcile(&service));
        assert_eq!(
            before,
            service.runtime().storage.list_json(ORDERS_NS).unwrap(),
            "observation must not mutate stored broker status or fills"
        );
    }
}
