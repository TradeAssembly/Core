// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct SimBroker {
    fill_steps: usize,
    sequence: usize,
    orders: BTreeMap<String, Value>,
    events: BTreeMap<String, Vec<Value>>,
    idempotency: BTreeMap<String, String>,
}

impl SimBroker {
    pub fn new(fill_steps: usize) -> Self {
        Self {
            fill_steps: fill_steps.max(1),
            sequence: 0,
            orders: BTreeMap::new(),
            events: BTreeMap::new(),
            idempotency: BTreeMap::new(),
        }
    }

    pub fn health(&self) -> Value {
        json!({
            "status": "ok",
            "provider_ref": "sim",
            "capabilities": {
                "provider_ref": "sim",
                "modes": ["paper"],
                "order_stream": true,
                "margin_preview": true,
                "supported_asset_classes": ["equity", "option", "crypto"]
            },
            "orders": self.orders.len()
        })
    }

    pub fn create_order(
        &mut self,
        mut order_plan: Value,
        idempotency_key: Option<&str>,
    ) -> Result<Value, String> {
        if order_plan["mode"].as_str().unwrap_or("paper") != "paper" {
            return Err("SimBroker only accepts paper orders".to_string());
        }
        let client_order_id = idempotency_key
            .map(str::to_string)
            .or_else(|| {
                order_plan
                    .get("client_order_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.next_client_order_id());
        order_plan["client_order_id"] = json!(client_order_id);
        if let Some(order_id) = self
            .idempotency
            .get(order_plan["client_order_id"].as_str().unwrap())
        {
            let state = self.orders.get(order_id).expect("idempotent order exists");
            return Ok(receipt(
                order_id,
                &order_plan,
                state["status"].as_str().unwrap_or("filled"),
            ));
        }
        let order_id = stable_order_id(order_plan["client_order_id"].as_str().unwrap());
        self.idempotency.insert(
            order_plan["client_order_id"].as_str().unwrap().to_string(),
            order_id.clone(),
        );
        self.orders.insert(
            order_id.clone(),
            json!({
                "id": order_id,
                "client_order_id": order_plan["client_order_id"],
                "status": "accepted",
                "symbol": order_plan["symbol"],
                "side": order_plan["side"],
                "qty": order_plan["quantity"].to_string(),
                "filled_qty": "0",
                "order_plan": order_plan
            }),
        );
        self.events.insert(order_id.clone(), Vec::new());
        self.emit(&order_id, "accepted", 0.0);
        for index in 0..self.fill_steps {
            let final_fill = index == self.fill_steps - 1;
            let status = if final_fill {
                "filled"
            } else {
                "partially_filled"
            };
            let quantity = self.orders[&order_id]["order_plan"]["quantity"]
                .as_f64()
                .unwrap_or(0.0);
            let filled_qty = if final_fill {
                quantity
            } else {
                round10(quantity * ((index + 1) as f64 / self.fill_steps as f64))
            };
            self.emit(&order_id, status, filled_qty);
        }
        let final_state = self.orders.get(&order_id).unwrap();
        Ok(receipt(
            &order_id,
            &final_state["order_plan"],
            final_state["status"].as_str().unwrap_or("filled"),
        ))
    }

    pub fn get_order(&self, order_id: &str) -> Value {
        self.orders.get(order_id).map_or_else(
            || json!({"status": 404, "body": {"detail": "order not found"}}),
            |order| json!({"status": 200, "body": order}),
        )
    }

    pub fn get_order_by_client_order_id(&self, client_order_id: &str) -> Value {
        self.idempotency.get(client_order_id).map_or_else(
            || json!({"status": 404, "body": {"detail": "order not found"}}),
            |order_id| self.get_order(order_id),
        )
    }

    pub fn get_order_events(&self, order_id: &str) -> Value {
        if !self.orders.contains_key(order_id) {
            return json!({"status": 404, "body": {"detail": "order not found"}});
        }
        json!({"status": 200, "body": self.events.get(order_id).cloned().unwrap_or_default()})
    }

    fn emit(&mut self, order_id: &str, status: &str, filled_qty: f64) {
        let events = self.events.entry(order_id.to_string()).or_default();
        let order = self.orders.get_mut(order_id).expect("order exists");
        order["status"] = json!(status);
        order["filled_qty"] = json!(decimal_string(filled_qty));
        events.push(json!({
            "id": format!("evt_{}_{}", events.len() + 1, order_id),
            "type": "order_update",
            "order_id": order_id,
            "client_order_id": order["client_order_id"],
            "status": status,
            "filled_qty": decimal_string(filled_qty),
            "qty": order["qty"],
            "ts": "2026-06-24T00:00:00+00:00",
            "broker_metadata": {"provider": "sim"}
        }));
    }

    fn next_client_order_id(&mut self) -> String {
        self.sequence += 1;
        format!("sim-generated-{}", self.sequence)
    }
}

fn receipt(order_id: &str, order_plan: &Value, status: &str) -> Value {
    json!({
        "order_id": order_id,
        "accepted": true,
        "provider_ref": "sim",
        "mode": order_plan["mode"],
        "order_plan": order_plan,
        "client_order_id": order_plan["client_order_id"],
        "status": status
    })
}

fn stable_order_id(client_order_id: &str) -> String {
    let digest = Sha256::digest(client_order_id.as_bytes());
    let hex = format!("{digest:x}");
    format!("ord_{}", &hex[..16])
}

fn round10(value: f64) -> f64 {
    (value * 10_000_000_000.0).round() / 10_000_000_000.0
}

fn decimal_string(value: f64) -> String {
    let text = format!("{value:.10}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    fn order_payload(client_order_id: Option<&str>) -> serde_json::Value {
        let mut payload = json!({
            "symbol": "BTC/USD",
            "side": "buy",
            "quantity": 0.0001,
            "mode": "paper",
            "provider_ref": "sim",
            "order_type": "market",
            "time_in_force": "day"
        });
        if let Some(client_order_id) = client_order_id {
            payload["client_order_id"] = json!(client_order_id);
        }
        payload
    }

    #[test]
    fn simbroker_exposes_health_capabilities_and_order_events() {
        let mut broker = super::SimBroker::new(3);

        let health = broker.health();
        assert_eq!(health["provider_ref"], "sim");
        assert_eq!(health["capabilities"]["order_stream"], true);

        let created = broker
            .create_order(order_payload(None), Some("idem-1"))
            .expect("order created");
        assert_eq!(created["accepted"], true);
        assert_eq!(created["client_order_id"], "idem-1");
        assert_eq!(created["status"], "filled");

        let order = broker.get_order(created["order_id"].as_str().unwrap());
        assert_eq!(order["status"], 200);
        assert_eq!(order["body"]["client_order_id"], "idem-1");

        let events = broker.get_order_events(created["order_id"].as_str().unwrap());
        assert_eq!(events["status"], 200);
        let statuses = events["body"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["status"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec!["accepted", "partially_filled", "partially_filled", "filled"]
        );
    }

    #[test]
    fn simbroker_uses_idempotency_header_for_duplicate_submit() {
        let mut broker = super::SimBroker::new(1);

        let first = broker
            .create_order(order_payload(None), Some("duplicate-key"))
            .unwrap();
        let second = broker
            .create_order(order_payload(None), Some("duplicate-key"))
            .unwrap();

        assert_eq!(second["order_id"], first["order_id"]);
        assert_eq!(broker.health()["orders"], 1);
        let by_client = broker.get_order_by_client_order_id("duplicate-key");
        assert_eq!(by_client["status"], 200);
        assert_eq!(by_client["body"]["id"], first["order_id"]);
    }

    #[test]
    fn simbroker_generates_distinct_client_ids_without_idempotency_header() {
        let mut broker = super::SimBroker::new(1);

        let first = broker.create_order(order_payload(None), None).unwrap();
        let second = broker.create_order(order_payload(None), None).unwrap();

        assert_ne!(second["order_id"], first["order_id"]);
        assert_eq!(broker.health()["orders"], 2);
    }

    #[test]
    fn simbroker_returns_404_for_unknown_orders() {
        let broker = super::SimBroker::new(1);

        assert_eq!(broker.get_order("missing")["status"], 404);
        assert_eq!(broker.get_order_events("missing")["status"], 404);
        assert_eq!(
            broker.get_order_by_client_order_id("missing")["status"],
            404
        );
    }
}
