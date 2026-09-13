// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub stage: String,
    pub strategy_id: String,
    pub inputs: Map<String, Value>,
    pub outputs: Map<String, Value>,
    pub run_id: Option<String>,
}

impl AuditEvent {
    pub fn new(
        stage: impl Into<String>,
        strategy_id: impl Into<String>,
        inputs: Option<Map<String, Value>>,
        outputs: Option<Map<String, Value>>,
        run_id: Option<String>,
    ) -> Self {
        Self {
            stage: stage.into(),
            strategy_id: strategy_id.into(),
            inputs: inputs.unwrap_or_default(),
            outputs: outputs.unwrap_or_default(),
            run_id,
        }
    }

    pub fn payload(&self) -> Value {
        json!({
            "stage": self.stage,
            "strategy_id": self.strategy_id,
            "inputs": self.inputs,
            "outputs": self.outputs,
            "run_id": self.run_id,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryAuditLogger {
    pub events: Vec<AuditEvent>,
}

impl MemoryAuditLogger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn log(
        &mut self,
        stage: impl Into<String>,
        strategy_id: impl Into<String>,
        inputs: Option<Map<String, Value>>,
        outputs: Option<Map<String, Value>>,
    ) -> AuditEvent {
        let event = AuditEvent::new(stage, strategy_id, inputs, outputs, None);
        self.events.push(event.clone());
        event
    }
}

pub trait AuditJournal {
    fn append(&mut self, event_type: &str, payload: Value);
}

#[derive(Debug)]
pub struct JournalAuditLogger<J> {
    journal: J,
    run_id: Option<String>,
}

impl<J: AuditJournal> JournalAuditLogger<J> {
    pub fn new(journal: J) -> Self {
        Self {
            journal,
            run_id: None,
        }
    }

    pub fn begin_run(&mut self, strategy_id: &str) -> String {
        let id = format!("audit_run_{}", RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed));
        self.run_id = Some(id.clone());
        self.journal.append(
            "audit.run.started",
            json!({"run_id": id, "strategy_id": strategy_id}),
        );
        id
    }

    pub fn log(
        &mut self,
        stage: impl Into<String>,
        strategy_id: impl Into<String>,
        inputs: Option<Map<String, Value>>,
        outputs: Option<Map<String, Value>>,
    ) -> AuditEvent {
        let event = AuditEvent::new(stage, strategy_id, inputs, outputs, self.run_id.clone());
        self.journal.append("audit.stage_recorded", event.payload());
        event
    }

    pub fn end_run(&mut self, status: &str) {
        self.journal.append(
            "audit.run.ended",
            json!({"run_id": self.run_id, "status": status}),
        );
    }

    pub fn into_journal(self) -> J {
        self.journal
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderPlan {
    pub strategy_id: String,
    pub symbol: String,
    pub client_order_id: String,
    pub action: String,
    pub provider_ref: Option<String>,
}

pub fn compile_from_payload(strategy_id: &str, payload: &Value) -> OrderPlan {
    compile_order_plan_from_payload(strategy_id, payload)
}

pub fn compile_order_plan_from_payload(strategy_id: &str, payload: &Value) -> OrderPlan {
    let symbol = payload
        .get("legs")
        .and_then(Value::as_array)
        .and_then(|legs| legs.first())
        .and_then(|leg| leg.get("symbol"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let client_order_id = payload
        .get("intent_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let provider_ref = payload
        .get("providerRef")
        .and_then(Value::as_str)
        .map(str::to_string);

    OrderPlan {
        strategy_id: strategy_id.to_string(),
        symbol,
        client_order_id,
        action,
        provider_ref,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeJournal {
        events: Vec<(String, Value)>,
    }

    impl AuditJournal for FakeJournal {
        fn append(&mut self, event_type: &str, payload: Value) {
            self.events.push((event_type.to_string(), payload));
        }
    }

    fn map(entries: [(&str, Value); 1]) -> Map<String, Value> {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }

    #[test]
    fn top_level_audit_imports_preserve_memory_logger_contract() {
        let mut logger = MemoryAuditLogger::new();
        let event = logger.log(
            "rule.evaluated",
            "strategy-1",
            None,
            Some(map([("ok", json!(true))])),
        );

        assert_eq!(event.stage, "rule.evaluated");
        assert_eq!(logger.events, vec![event]);
    }

    #[test]
    fn source_logger_module_reexport_shape_preserves_inputs() {
        let mut logger = MemoryAuditLogger::new();

        logger.log(
            "risk.checked",
            "strategy-1",
            Some(map([("notional", json!(25))])),
            None,
        );

        assert_eq!(logger.events[0].inputs, map([("notional", json!(25))]));
    }

    #[test]
    fn source_journal_logger_records_run_stage_and_end_events() {
        let journal = FakeJournal::default();
        let mut logger = JournalAuditLogger::new(journal);

        let run_id = logger.begin_run("strategy-1");
        let event = logger.log(
            "oms.compiled",
            "strategy-1",
            None,
            Some(map([("order_plan", json!({"symbol": "BTC/USD"}))])),
        );
        logger.end_run("COMPLETED");
        let journal = logger.into_journal();

        assert!(run_id.starts_with("audit_run_"));
        assert_eq!(event.run_id.as_deref(), Some(run_id.as_str()));
        assert_eq!(
            journal
                .events
                .iter()
                .map(|(event_type, _)| event_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "audit.run.started",
                "audit.stage_recorded",
                "audit.run.ended"
            ]
        );
    }

    #[test]
    fn audit_replay_entrypoint_matches_compatibility_path() {
        let payload = json!({
            "intent_id": "intent-1",
            "legs": [{"symbol": "SPY", "qty": 2}],
            "action": "sell",
            "providerRef": "sim",
        });

        let source_plan = compile_from_payload("strategy-1", &payload);
        let compat_plan = compile_order_plan_from_payload("strategy-1", &payload);

        assert_eq!(source_plan, compat_plan);
        assert_eq!(source_plan.symbol, "SPY");
        assert_eq!(source_plan.client_order_id, "intent-1");
    }
}
