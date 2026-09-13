// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};

static MESSAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn default_id() -> String {
    format!("msg_{}", MESSAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

fn default_ts() -> String {
    "1970-01-01T00:00:00Z".to_string()
}

fn strategy_trigger_type() -> String {
    "strategy.trigger".to_string()
}

fn order_update_type() -> String {
    "order_update".to_string()
}

fn default_submit_true() -> bool {
    true
}

fn default_mode() -> String {
    "paper".to_string()
}

fn default_source() -> String {
    "scheduler".to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageEnvelope {
    pub producer: String,
    pub message_type: String,
    pub activation_id: String,
    pub idempotency_key: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyTriggerMessage {
    #[serde(default = "default_id")]
    pub id: String,
    #[serde(default = "default_ts")]
    pub ts: String,
    #[serde(default = "strategy_trigger_type")]
    pub r#type: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    pub activation_id: String,
    pub strategy_id: String,
    #[serde(default)]
    pub config_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub timeframe: Option<String>,
    #[serde(default)]
    pub provider_ref: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub cycle: Option<i64>,
    #[serde(default = "default_submit_true")]
    pub submit_exit: bool,
    #[serde(default = "default_submit_true")]
    pub submit_orders: bool,
    #[serde(default)]
    pub dispatch_order_intents: bool,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub market_clock: Map<String, Value>,
    #[serde(default)]
    pub payload: Map<String, Value>,
}

impl StrategyTriggerMessage {
    pub fn to_envelope(&self, producer: &str, idempotency_key: &str) -> MessageEnvelope {
        MessageEnvelope {
            producer: producer.to_string(),
            message_type: self.r#type.clone(),
            activation_id: self.activation_id.clone(),
            idempotency_key: idempotency_key.to_string(),
            payload: serde_json::to_value(self).expect("strategy trigger serializes"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeIntentMessage {
    pub strategy_id: String,
    pub idempotency_key: String,
    pub legs: Vec<Map<String, Value>>,
    #[serde(default)]
    pub target_price_policy: Map<String, Value>,
    #[serde(default)]
    pub time_budget_ms: Option<i64>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderIntentMessage {
    pub intent_id: String,
    pub legs: Vec<Map<String, Value>>,
    pub order_type: String,
    #[serde(rename = "tif", alias = "time_in_force")]
    pub time_in_force: String,
    #[serde(default)]
    pub attachments: Option<Map<String, Value>>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl OrderIntentMessage {
    pub fn model_dump_by_alias(&self) -> Value {
        serde_json::to_value(self).expect("order intent serializes")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderUpdateMessage {
    #[serde(default = "default_id")]
    pub id: String,
    #[serde(default = "default_ts")]
    pub ts: String,
    #[serde(default = "order_update_type")]
    pub r#type: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    pub order_id: String,
    pub status: String,
    #[serde(default)]
    pub fills: Vec<Map<String, Value>>,
    #[serde(default)]
    pub broker_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskVerdictMessage {
    pub decision_id: String,
    pub passed: bool,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub exposure_before: Map<String, Value>,
    #[serde(default)]
    pub exposure_after: Map<String, Value>,
    #[serde(default)]
    pub reservation_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerEvent {
    pub id: String,
    pub ts: String,
    pub r#type: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    pub strategy_id: String,
    pub source: String,
    #[serde(default)]
    pub payload: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeIntent {
    pub strategy_id: String,
    pub idempotency_key: String,
    pub legs: Vec<Map<String, Value>>,
    pub target_price_policy: Map<String, Value>,
    #[serde(default)]
    pub time_budget_ms: Option<i64>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderPlan {
    pub intent_id: String,
    pub legs: Vec<Map<String, Value>>,
    pub order_type: String,
    pub tif: String,
    #[serde(default)]
    pub attachments: Option<Map<String, Value>>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderUpdate {
    pub id: String,
    pub ts: String,
    pub r#type: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    pub order_id: String,
    pub status: String,
    #[serde(default)]
    pub fills: Vec<Map<String, Value>>,
    #[serde(default)]
    pub broker_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskVerdict {
    pub decision_id: String,
    pub passed: bool,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub exposure_before: Map<String, Value>,
    #[serde(default)]
    pub exposure_after: Map<String, Value>,
    #[serde(default)]
    pub reservation_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn strategy_trigger_message_serializes_into_durable_envelope() {
        let trigger: StrategyTriggerMessage = serde_json::from_value(json!({
            "activation_id": "act-1",
            "strategy_id": "strat-1",
            "config_id": "cfg-1",
            "version_id": "ver-1",
            "symbol": "BTC/USD",
            "timeframe": "1m",
            "provider_ref": "alpaca-paper",
            "cycle": 7,
            "submit_orders": false,
            "dispatch_order_intents": true,
            "market_clock": {"allowed": true},
        }))
        .expect("trigger parses");

        let envelope = trigger.to_envelope("scheduler", "trigger-key");

        assert_eq!(envelope.message_type, "strategy.trigger");
        assert_eq!(envelope.activation_id, "act-1");
        assert_eq!(envelope.idempotency_key, "trigger-key");
        assert_eq!(envelope.payload["strategy_id"], "strat-1");
        assert_eq!(envelope.payload["cycle"], 7);
        assert_eq!(envelope.payload["submit_orders"], false);
        assert_eq!(envelope.payload["dispatch_order_intents"], true);
        assert_eq!(envelope.payload["market_clock"], json!({"allowed": true}));
    }

    #[test]
    fn message_contracts_forbid_unexpected_fields() {
        let result = serde_json::from_value::<StrategyTriggerMessage>(json!({
            "activation_id": "act-1",
            "strategy_id": "strat-1",
            "unexpected": true,
        }));

        assert!(result.is_err());
    }

    #[test]
    fn trade_intent_and_order_intent_keep_source_shape_aliases() {
        let trade: TradeIntentMessage = serde_json::from_value(json!({
            "strategy_id": "strat-1",
            "idempotency_key": "intent-key",
            "legs": [{"symbol": "SPY", "side": "buy"}],
            "target_price_policy": {"kind": "midpoint"},
        }))
        .expect("trade parses");
        let order: OrderIntentMessage = serde_json::from_value(json!({
            "intent_id": "intent-1",
            "legs": trade.legs,
            "order_type": "limit",
            "tif": "day",
            "attachments": {"take_profit": {"limit_price": 101}},
        }))
        .expect("order parses");

        assert_eq!(order.time_in_force, "day");
        assert_eq!(order.model_dump_by_alias()["tif"], "day");
        assert_eq!(
            order.attachments,
            Some(object(json!({"take_profit": {"limit_price": 101}})))
        );
    }

    #[test]
    fn order_update_and_risk_verdict_messages_are_audit_shaped() {
        let update: OrderUpdateMessage = serde_json::from_value(json!({
            "order_id": "ord-1",
            "status": "FILLED",
            "fills": [{"qty": 1, "price": 10.5}],
            "broker_metadata": {"adapter": "sim"},
        }))
        .expect("update parses");
        let verdict: RiskVerdictMessage = serde_json::from_value(json!({
            "decision_id": "risk-1",
            "passed": false,
            "reasons": ["max_notional_exceeded"],
            "exposure_before": {"notional": 0},
            "exposure_after": {"notional": 1000},
        }))
        .expect("verdict parses");

        assert_eq!(update.r#type, "order_update");
        assert_eq!(update.fills[0]["price"], 10.5);
        assert!(!verdict.passed);
        assert_eq!(verdict.reasons, vec!["max_notional_exceeded"]);
    }

    #[test]
    fn source_models_messages_import_path_preserves_source_contracts() {
        let trigger: TriggerEvent = serde_json::from_value(json!({
            "id": "evt-1",
            "ts": "2026-01-01T00:00:00Z",
            "type": "trigger",
            "strategy_id": "strat-1",
            "source": "scheduler",
        }))
        .expect("trigger event parses");
        let trade: TradeIntent = serde_json::from_value(json!({
            "strategy_id": "strat-1",
            "idempotency_key": "idem-1",
            "legs": [{"symbol": "BTC/USD", "side": "buy", "qty": 1}],
            "target_price_policy": {"type": "market"},
        }))
        .expect("trade parses");
        let order: OrderPlan = serde_json::from_value(json!({
            "intent_id": "intent-1",
            "legs": trade.legs,
            "order_type": "market",
            "tif": "gtc",
        }))
        .expect("order parses");
        let update: OrderUpdate = serde_json::from_value(json!({
            "id": "evt-2",
            "ts": "2026-01-01T00:00:00Z",
            "type": "order_update",
            "order_id": "ord-1",
            "status": "filled",
        }))
        .expect("update parses");
        let verdict: RiskVerdict = serde_json::from_value(json!({
            "decision_id": "decision-1",
            "passed": true,
        }))
        .expect("verdict parses");

        assert_eq!(trigger.payload, Map::new());
        assert_eq!(trade.metadata, Map::new());
        assert_eq!(order.tif, "gtc");
        assert_eq!(update.fills, Vec::<Map<String, Value>>::new());
        assert_eq!(verdict.exposure_before, Map::new());
    }
}
