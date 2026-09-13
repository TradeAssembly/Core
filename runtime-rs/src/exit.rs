// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct ExitDecision {
    pub triggered: bool,
    pub reason_code: Option<String>,
}

pub fn normalize_exit_policy(policy: &Value) -> Value {
    let mut normalized = policy.as_object().cloned().unwrap_or_default();
    let kind = normalized
        .get("kind")
        .cloned()
        .unwrap_or_else(|| json!("none"));
    let params = normalized
        .get("params")
        .cloned()
        .unwrap_or_else(|| json!({}));

    if !normalized.contains_key("guardrails") {
        normalized.insert(
            "guardrails".to_string(),
            json!([{"kind": kind, "params": params}]),
        );
    }

    Value::Object(normalized)
}

pub fn attachment_from_exit_policy(policy: &Value) -> Value {
    let params = policy
        .get("params")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    match policy.get("kind").and_then(Value::as_str).unwrap_or("") {
        "tp_sl" => json!({
            "type": "TP_SL",
            "tp_pct": pct(params.get("take_profit_pct")),
            "sl_pct": pct(params.get("stop_loss_pct")),
        }),
        "trailing_stop" => json!({
            "type": "TRAILING_STOP",
            "trailing_pct": pct(params.get("trailing_pct")),
        }),
        other => json!({"type": other.to_ascii_uppercase()}),
    }
}

pub fn evaluate_exit_policy(
    policy: &Value,
    rows: &[Value],
    entry_price: f64,
    side: &str,
) -> ExitDecision {
    let params = policy
        .get("params")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let close = rows
        .last()
        .and_then(|row| row.get("close"))
        .and_then(Value::as_f64)
        .unwrap_or(entry_price);

    if policy.get("kind").and_then(Value::as_str) != Some("tp_sl") {
        return ExitDecision {
            triggered: false,
            reason_code: None,
        };
    }

    let take_profit = pct(params.get("take_profit_pct"));
    let stop_loss = pct(params.get("stop_loss_pct"));
    let is_buy = side.eq_ignore_ascii_case("buy");
    let take_profit_hit = if is_buy {
        close >= entry_price * (1.0 + take_profit / 100.0)
    } else {
        close <= entry_price * (1.0 - take_profit / 100.0)
    };
    let stop_loss_hit = if is_buy {
        close <= entry_price * (1.0 - stop_loss / 100.0)
    } else {
        close >= entry_price * (1.0 + stop_loss / 100.0)
    };

    if take_profit_hit {
        ExitDecision {
            triggered: true,
            reason_code: Some("tp_sl.take_profit".to_string()),
        }
    } else if stop_loss_hit {
        ExitDecision {
            triggered: true,
            reason_code: Some("tp_sl.stop_loss".to_string()),
        }
    } else {
        ExitDecision {
            triggered: false,
            reason_code: None,
        }
    }
}

pub fn exit_metadata_from_policy(policy: &Value) -> Value {
    json!({"exit_policy": normalize_exit_policy(policy)})
}

fn pct(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::String(text)) => text
            .trim()
            .trim_end_matches('%')
            .parse::<f64>()
            .unwrap_or_default(),
        Some(Value::Number(number)) => number.as_f64().unwrap_or_default(),
        _ => 0.0,
    }
}

pub trait ExitWatchService {
    fn process_exit_watch(&mut self, watch_id: &str, submit_orders: bool) -> Value;
    fn process_exit_watches(&mut self, submit_orders: bool) -> Value;
    fn cancel_exit_watches_for_entry_order(
        &mut self,
        entry_order_id: &str,
        reason: &str,
    ) -> Vec<Value>;
}

pub struct SyntheticExitOrchestrator<S> {
    service: S,
}

impl<S: ExitWatchService> SyntheticExitOrchestrator<S> {
    pub fn new(service: S) -> Self {
        Self { service }
    }

    pub fn process_watch(&mut self, watch_id: &str, submit_orders: bool) -> Value {
        self.service.process_exit_watch(watch_id, submit_orders)
    }

    pub fn process_active_watches(&mut self) -> Value {
        self.service.process_exit_watches(true)
    }

    pub fn cancel_for_entry_order(&mut self, entry_order_id: &str, reason: &str) -> Vec<Value> {
        self.service
            .cancel_exit_watches_for_entry_order(entry_order_id, reason)
    }

    pub fn into_service(self) -> S {
        self.service
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct StubExitWatchService {
        calls: Vec<Value>,
    }

    impl ExitWatchService for StubExitWatchService {
        fn process_exit_watch(&mut self, watch_id: &str, submit_orders: bool) -> Value {
            self.calls.push(json!(["process", watch_id, submit_orders]));
            json!({"watch_id": watch_id, "triggered": true, "dry_run": !submit_orders})
        }

        fn process_exit_watches(&mut self, submit_orders: bool) -> Value {
            self.calls.push(json!(["process_all", submit_orders]));
            json!({"processed": 1, "triggered": if submit_orders { 1 } else { 0 }})
        }

        fn cancel_exit_watches_for_entry_order(
            &mut self,
            entry_order_id: &str,
            reason: &str,
        ) -> Vec<Value> {
            self.calls.push(json!(["cancel", entry_order_id, reason]));
            Vec::new()
        }
    }

    #[test]
    fn top_level_exit_imports_preserve_policy_helpers() {
        let policy =
            json!({"kind": "tp_sl", "params": {"take_profit_pct": "5%", "stop_loss_pct": 2}});

        let normalized = normalize_exit_policy(&policy);
        let attachment = attachment_from_exit_policy(&policy);
        let decision = evaluate_exit_policy(
            &policy,
            &[json!({"close": 100}), json!({"close": 106})],
            100.0,
            "buy",
        );

        assert_eq!(normalized["guardrails"][0]["kind"], "tp_sl");
        assert_eq!(
            attachment,
            json!({"type": "TP_SL", "tp_pct": 5.0, "sl_pct": 2.0})
        );
        assert!(decision.triggered);
        assert_eq!(decision.reason_code.as_deref(), Some("tp_sl.take_profit"));
    }

    #[test]
    fn source_policy_module_exposes_exit_metadata() {
        let metadata = exit_metadata_from_policy(
            &json!({"kind": "trailing_stop", "params": {"trailing_pct": 3}}),
        );

        assert_eq!(metadata["exit_policy"]["kind"], "trailing_stop");
        assert_eq!(
            metadata["exit_policy"]["guardrails"][0]["kind"],
            "trailing_stop"
        );
    }

    #[test]
    fn source_orchestrator_facade_delegates_to_exit_watch_service() {
        let service = StubExitWatchService::default();
        let mut orchestrator = SyntheticExitOrchestrator::new(service);

        assert_eq!(
            orchestrator.process_watch("watch-1", false)["dry_run"],
            true
        );
        assert_eq!(orchestrator.process_active_watches()["processed"], 1);
        assert_eq!(
            orchestrator.cancel_for_entry_order("order-1", "terminal_entry_state"),
            Vec::<Value>::new()
        );
        let service = orchestrator.into_service();
        assert_eq!(
            service.calls,
            vec![
                json!(["process", "watch-1", false]),
                json!(["process_all", true]),
                json!(["cancel", "order-1", "terminal_entry_state"]),
            ]
        );
    }
}
