// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct NormalizedOrderUpdate {
    pub provider_ref: String,
    pub broker_order_id: Option<String>,
    pub client_order_id: Option<String>,
    pub status: String,
    pub stream_event_id: Option<String>,
    pub stream_state_key: String,
    pub status_payload: Value,
}

#[derive(Clone, Debug)]
struct OrderRecord {
    status: String,
    attempts: Vec<Value>,
}

#[derive(Clone, Debug, Default)]
pub struct OrderStatusService {
    orders: HashMap<String, OrderRecord>,
    client_index: HashMap<String, String>,
    reservations: Vec<String>,
    events: Vec<Value>,
}

impl OrderStatusService {
    pub fn seed_accepted_entry(broker_order_id: &str, client_order_id: &str) -> Self {
        let mut service = Self::default();
        service.orders.insert(
            broker_order_id.to_string(),
            OrderRecord {
                status: "accepted".to_string(),
                attempts: Vec::new(),
            },
        );
        service
            .client_index
            .insert(client_order_id.to_string(), broker_order_id.to_string());
        service
            .reservations
            .push(format!("reservation:{client_order_id}"));
        service
    }

    pub fn ingest_broker_order_status_stream(
        &mut self,
        provider_ref: &str,
        payload: &Value,
    ) -> Value {
        let stream = payload["stream"].as_str().unwrap_or("trade_updates");
        let mut ingested = 0_u64;
        let mut unknown_orders = Vec::<String>::new();
        for event in payload["events"].as_array().cloned().unwrap_or_default() {
            let normalized = normalize_broker_order_update(provider_ref, &event, stream);
            let broker_id = normalized.broker_order_id.clone().or_else(|| {
                normalized
                    .client_order_id
                    .as_ref()
                    .and_then(|client_id| self.client_index.get(client_id).cloned())
            });
            let Some(broker_id) = broker_id else {
                if let Some(client_id) = normalized.client_order_id.clone() {
                    unknown_orders.push(client_id.clone());
                    self.events.push(json!({
                        "event_type": "broker.order_status_stream_unknown_order",
                        "payload": {
                            "client_order_id": client_id,
                            "raw_payload": event
                        }
                    }));
                }
                continue;
            };
            if let Some(order) = self.orders.get_mut(&broker_id) {
                order.status = order_status_from_attempt_status(&normalized.status).to_string();
                order.attempts.push(json!({
                    "status": normalized.status,
                    "raw_payload": normalized.status_payload
                }));
                if (order.status == "canceled"
                    || order.status == "filled"
                    || order.status == "rejected")
                    && !self.reservations.is_empty()
                {
                    self.reservations.clear();
                    self.events.push(json!({
                        "event_type": "risk.released",
                        "payload": {"broker_order_id": broker_id}
                    }));
                }
                ingested += 1;
            } else if let Some(client_id) = normalized.client_order_id.clone() {
                unknown_orders.push(client_id.clone());
                self.events.push(json!({
                    "event_type": "broker.order_status_stream_unknown_order",
                    "payload": {
                        "client_order_id": client_id,
                        "raw_payload": event
                    }
                }));
            }
        }
        json!({"ingested": ingested, "unknown_orders": unknown_orders})
    }

    pub fn order_status(&self, broker_order_id: &str) -> Option<&str> {
        self.orders
            .get(broker_order_id)
            .map(|order| order.status.as_str())
    }

    pub fn active_risk_reservations(&self) -> &[String] {
        &self.reservations
    }

    pub fn replay_count(&self, event_type: &str) -> u64 {
        self.events
            .iter()
            .filter(|event| event["event_type"].as_str() == Some(event_type))
            .count() as u64
    }

    pub fn last_attempt(&self, broker_order_id: &str) -> Option<&Value> {
        self.orders
            .get(broker_order_id)
            .and_then(|order| order.attempts.last())
    }

    pub fn last_event(&self, event_type: &str) -> Option<&Value> {
        self.events
            .iter()
            .rev()
            .find(|event| event["event_type"].as_str() == Some(event_type))
    }
}

pub fn normalize_broker_order_update(
    provider_ref: &str,
    update: &Value,
    stream: &str,
) -> NormalizedOrderUpdate {
    let order = &update["order"];
    let event = text(update, &["event"]).unwrap_or_else(|| "calculated".to_string());
    let status = normalize_status(&event, text(order, &["status"]).as_deref());
    let broker_order_id = text(order, &["id", "orderId", "order_id"]);
    let client_order_id = text(order, &["client_order_id", "clientOrderId", "client_id"]);
    let stream_event_id = text(update, &["event_id", "eventId", "id"]);
    let stream_state_key = format!(
        "{}:{}:{}:{}",
        provider_ref,
        stream,
        broker_order_id.as_deref().unwrap_or(""),
        stream_event_id.as_deref().unwrap_or("")
    );
    let mut status_payload = json!({
        "status": status,
        "stream_event": event,
        "stream_state_key": stream_state_key,
    });
    if let Some(filled_qty) = text(order, &["filled_qty", "filledQty"]) {
        status_payload["filled_qty"] = json!(filled_qty);
    }
    if let Some(filled_avg_price) = text(order, &["filled_avg_price", "filledAvgPrice"]) {
        status_payload["filled_avg_price"] = json!(filled_avg_price);
    }
    if let Some(client_order_id) = &client_order_id {
        status_payload["client_order_id"] = json!(client_order_id);
    }
    if let Some(stream_event_id) = &stream_event_id {
        status_payload["stream_event_id"] = json!(stream_event_id);
    }

    NormalizedOrderUpdate {
        provider_ref: provider_ref.to_string(),
        broker_order_id,
        client_order_id,
        status,
        stream_event_id,
        stream_state_key,
        status_payload,
    }
}

fn normalize_status(event: &str, order_status: Option<&str>) -> String {
    match event {
        "new" | "pending_new" | "accepted" | "pending_cancel" | "pending_replace" | "replaced"
        | "suspended" => "accepted".to_string(),
        "partial_fill" => "partially_filled".to_string(),
        "fill" => "filled".to_string(),
        "canceled" | "cancelled" | "expired" | "done_for_day" | "stopped" => "canceled".to_string(),
        "rejected" => "rejected".to_string(),
        "calculated" => order_status
            .map(normalize_order_status_alias)
            .unwrap_or("accepted")
            .to_string(),
        other => normalize_order_status_alias(other).to_string(),
    }
}

fn normalize_order_status_alias(status: &str) -> &str {
    match status {
        "cancelled" | "expired" | "done_for_day" | "stopped" => "canceled",
        "partial_fill" => "partially_filled",
        other => other,
    }
}

fn order_status_from_attempt_status(status: &str) -> &str {
    match status {
        "partially_filled" => "filled",
        other => other,
    }
}

fn text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let value = &value[*key];
        value
            .as_str()
            .map(ToOwned::to_owned)
            .or_else(|| value.as_i64().map(|number| number.to_string()))
            .or_else(|| value.as_u64().map(|number| number.to_string()))
            .or_else(|| value.as_f64().map(|number| number.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn normalizes_alpaca_fill_dict_update() {
        let normalized = super::normalize_broker_order_update(
            "alpaca-paper",
            &json!({
                "event_id": "evt-fill",
                "event": "fill",
                "order": {
                    "id": "ord-1",
                    "client_order_id": "client-1",
                    "filled_qty": "1",
                    "filled_avg_price": "10.5",
                    "qty": "1"
                }
            }),
            "trade_updates",
        );

        assert_eq!(normalized.broker_order_id.as_deref(), Some("ord-1"));
        assert_eq!(normalized.client_order_id.as_deref(), Some("client-1"));
        assert_eq!(normalized.status, "filled");
        assert_eq!(normalized.stream_event_id.as_deref(), Some("evt-fill"));
        assert!(!normalized.stream_state_key.is_empty());
        assert_eq!(
            normalized.status_payload["stream_state_key"],
            normalized.stream_state_key
        );
        assert_eq!(normalized.status_payload["filled_qty"], "1");
        assert_eq!(normalized.status_payload["filled_avg_price"], "10.5");
    }

    #[test]
    fn normalizes_alpaca_aliases_and_full_trade_update_vocabulary() {
        let partial = super::normalize_broker_order_update(
            "alpaca-paper",
            &json!({"event": "partial_fill", "order": {"id": "ord-2", "filled_qty": 0.5, "qty": 1}}),
            "trade_updates",
        );
        let canceled = super::normalize_broker_order_update(
            "alpaca-paper",
            &json!({"event": "cancelled", "order": {"id": "ord-3"}}),
            "trade_updates",
        );

        assert_eq!(partial.status, "partially_filled");
        assert_eq!(partial.status_payload["filled_qty"], "0.5");
        assert_eq!(canceled.status, "canceled");

        for (event, status) in [
            ("new", "accepted"),
            ("pending_new", "accepted"),
            ("accepted", "accepted"),
            ("pending_cancel", "accepted"),
            ("pending_replace", "accepted"),
            ("replaced", "accepted"),
            ("suspended", "accepted"),
            ("partial_fill", "partially_filled"),
            ("fill", "filled"),
            ("canceled", "canceled"),
            ("cancelled", "canceled"),
            ("expired", "canceled"),
            ("done_for_day", "canceled"),
            ("stopped", "canceled"),
            ("rejected", "rejected"),
        ] {
            let normalized = super::normalize_broker_order_update(
                "alpaca-paper",
                &json!({"event": event, "order": {"id": format!("ord-{event}"), "qty": "1", "filled_qty": "0"}}),
                "trade_updates",
            );
            assert_eq!(normalized.status, status);
            assert_eq!(normalized.status_payload["status"], status);
            assert_eq!(normalized.status_payload["stream_event"], event);
        }
    }

    #[test]
    fn normalizes_camel_case_payloads_and_calculated_status() {
        let normalized = super::normalize_broker_order_update(
            "alpaca-paper",
            &json!({
                "eventId": "evt-camel",
                "event": "calculated",
                "order": {
                    "orderId": "ord-camel",
                    "clientOrderId": "client-camel",
                    "status": "filled",
                    "filledQty": "2",
                    "qty": "2",
                    "filledAvgPrice": "11.25"
                }
            }),
            "trade_updates",
        );

        assert_eq!(normalized.broker_order_id.as_deref(), Some("ord-camel"));
        assert_eq!(normalized.client_order_id.as_deref(), Some("client-camel"));
        assert_eq!(normalized.status, "filled");
        assert_eq!(normalized.stream_event_id.as_deref(), Some("evt-camel"));
        assert_eq!(normalized.status_payload["filled_qty"], "2");
        assert_eq!(normalized.status_payload["filled_avg_price"], "11.25");
    }

    #[test]
    fn terminal_stream_update_releases_entry_reservation() {
        let mut service = super::OrderStatusService::seed_accepted_entry(
            "accepted-expiring-entry",
            "client-expiring-entry",
        );

        let result = service.ingest_broker_order_status_stream(
            "alpaca-paper",
            &json!({
                "stream": "trade_updates",
                "events": [{
                    "eventId": "evt-expired-1",
                    "event": "expired",
                    "order": {
                        "orderId": "accepted-expiring-entry",
                        "clientOrderId": "client-expiring-entry",
                        "status": "expired",
                        "filledQty": "0",
                        "qty": "1"
                    }
                }]
            }),
        );

        assert_eq!(result["ingested"], 1);
        assert_eq!(
            service.order_status("accepted-expiring-entry"),
            Some("canceled")
        );
        assert!(service.active_risk_reservations().is_empty());
        assert_eq!(service.replay_count("risk.released"), 1);
    }

    #[test]
    fn order_stream_ingestion_matches_by_client_id_and_journals_unknown_orders() {
        let mut service =
            super::OrderStatusService::seed_accepted_entry("broker-entry", "client-entry");
        let matched = service.ingest_broker_order_status_stream(
            "alpaca-paper",
            &json!({
                "stream": "trade_updates",
                "events": [{
                    "event_id": "client-match-1",
                    "event": "partial_fill",
                    "order": {"client_order_id": "client-entry", "filled_qty": "0.00015", "qty": "0.0003"}
                }]
            }),
        );

        assert_eq!(matched["ingested"], 1);
        assert_eq!(matched["unknown_orders"], json!([]));
        assert_eq!(service.order_status("broker-entry"), Some("filled"));
        let attempt = service.last_attempt("broker-entry").expect("attempt");
        assert_eq!(attempt["status"], "partially_filled");
        assert_eq!(attempt["raw_payload"]["client_order_id"], "client-entry");
        assert_eq!(attempt["raw_payload"]["stream_event_id"], "client-match-1");

        let mut empty = super::OrderStatusService::default();
        let unknown = empty.ingest_broker_order_status_stream(
            "alpaca-paper",
            &json!({"events": [{"event_id": "unknown-1", "event": "fill", "order": {"client_order_id": "missing-client"}}]}),
        );

        assert_eq!(unknown["ingested"], 0);
        assert_eq!(unknown["unknown_orders"], json!(["missing-client"]));
        let event = empty
            .last_event("broker.order_status_stream_unknown_order")
            .unwrap();
        assert_eq!(event["payload"]["client_order_id"], "missing-client");
        assert_eq!(
            event["payload"]["raw_payload"]["order"]["client_order_id"],
            "missing-client"
        );
    }
}
