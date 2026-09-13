// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub trait JournalBackend {
    fn name(&self) -> &'static str;
    fn connect_args(&self, url: &str) -> Map<String, Value>;
    fn configure_engine(&self, engine: &mut dyn SqlEngine);
    fn init_db(&self, engine: &mut dyn SqlEngine, metadata: &mut dyn Metadata);
    fn encode_json(&self, payload: Value) -> Value;
    fn decode_json(&self, payload: Value, fallback: Option<Value>) -> Value;
}

pub trait SqlEngine {
    fn backend_name(&self) -> &str;
    fn exec_driver_sql(&mut self, sql: &str);
}

pub trait Metadata {
    fn create_all(&mut self, engine_backend_name: &str);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SQLiteBackend;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostgresBackend;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    SQLite(SQLiteBackend),
    Postgres(PostgresBackend),
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Self::SQLite(backend) => backend.name(),
            Self::Postgres(backend) => backend.name(),
        }
    }
}

impl JournalBackend for SQLiteBackend {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn connect_args(&self, _url: &str) -> Map<String, Value> {
        Map::from_iter([("check_same_thread".to_string(), json!(false))])
    }

    fn configure_engine(&self, engine: &mut dyn SqlEngine) {
        engine.exec_driver_sql("PRAGMA journal_mode=WAL;");
    }

    fn init_db(&self, engine: &mut dyn SqlEngine, metadata: &mut dyn Metadata) {
        metadata.create_all(engine.backend_name());
        engine.exec_driver_sql(
            "ALTER TABLE strategy_execution_configs ADD COLUMN execution_mode TEXT;",
        );
        engine.exec_driver_sql(
            "CREATE INDEX IF NOT EXISTS idx_reservations_active_intent ON reservations(intent_id);",
        );
    }

    fn encode_json(&self, payload: Value) -> Value {
        Value::String(payload.to_string())
    }

    fn decode_json(&self, payload: Value, fallback: Option<Value>) -> Value {
        match payload {
            Value::String(text) => {
                serde_json::from_str(&text).unwrap_or_else(|_| fallback.unwrap_or(Value::Null))
            }
            other => other,
        }
    }
}

impl JournalBackend for PostgresBackend {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn connect_args(&self, _url: &str) -> Map<String, Value> {
        Map::new()
    }

    fn configure_engine(&self, _engine: &mut dyn SqlEngine) {}

    fn init_db(&self, engine: &mut dyn SqlEngine, metadata: &mut dyn Metadata) {
        metadata.create_all(engine.backend_name());
        engine.exec_driver_sql(
            "ALTER TABLE strategy_execution_configs ADD COLUMN IF NOT EXISTS execution_mode TEXT;",
        );
        engine.exec_driver_sql(
            "CREATE INDEX IF NOT EXISTS idx_reservations_active_intent ON reservations(intent_id);",
        );
    }

    fn encode_json(&self, payload: Value) -> Value {
        payload
    }

    fn decode_json(&self, payload: Value, _fallback: Option<Value>) -> Value {
        payload
    }
}

pub fn backend_for_url(url: &str) -> Backend {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Backend::Postgres(PostgresBackend)
    } else {
        Backend::SQLite(SQLiteBackend)
    }
}

pub fn backend_for_engine(engine: &dyn SqlEngine) -> Backend {
    match engine.backend_name() {
        "postgres" | "postgresql" => Backend::Postgres(PostgresBackend),
        _ => Backend::SQLite(SQLiteBackend),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalEvent {
    pub event_type: String,
    pub payload: Value,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct JsonlJournal {
    events: Vec<JournalEvent>,
}

impl JsonlJournal {
    pub fn append(&mut self, event_type: &str, payload: Value) {
        self.events.push(JournalEvent {
            event_type: event_type.to_string(),
            payload,
        });
    }

    pub fn read(&self) -> Vec<JournalEvent> {
        self.events.clone()
    }
}

pub type SQLiteJournal = JsonlJournal;

pub fn replay(events: Vec<JournalEvent>) -> Map<String, Value> {
    let mut counts = BTreeMap::<String, u64>::new();
    for event in events {
        *counts.entry(event.event_type).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(key, count)| (key, json!(count)))
        .collect()
}

pub fn journal_event_hash(event_type: &str, payload: &Value) -> String {
    let canonical = format!("{event_type}:{}", canonical_json(payload));
    let digest = Sha256::digest(canonical.as_bytes());
    hex_lower(&digest)
}

#[derive(Clone, Debug, PartialEq)]
pub struct RiskReservation {
    pub id: String,
    pub run_id: String,
    pub strategy_id: String,
    pub intent_id: String,
    pub amount: f64,
    pub active: bool,
    pub order_id: Option<String>,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct NoopJournalWriter;

impl NoopJournalWriter {
    pub fn add_reservation(
        &mut self,
        _run_id: &str,
        _strategy_id: &str,
        _intent_id: &str,
        _amount: f64,
    ) -> Option<String> {
        None
    }

    pub fn link_reservation_order(&mut self, _reservation_id: &str, _order_id: &str) {}

    pub fn record_order_entry(&mut self, _order_id: &str, _strategy_id: &str) {}

    pub fn release_reservations_for_run(&mut self, _run_id: &str) {}
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct NoopReservationReader;

impl NoopReservationReader {
    pub fn count_active_reservations(&self, _strategy_id: &str) -> usize {
        0
    }

    pub fn sum_active_reservations(&self, _strategy_id: &str) -> f64 {
        0.0
    }
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct JournalDbAdapter {
    reservations: BTreeMap<String, RiskReservation>,
    reservation_keys: BTreeMap<(String, String, String), String>,
    events: Vec<JournalEvent>,
}

impl JournalDbAdapter {
    pub fn add_reservation(
        &mut self,
        run_id: &str,
        strategy_id: &str,
        intent_id: &str,
        amount: f64,
    ) -> Option<String> {
        let key = (
            run_id.to_string(),
            strategy_id.to_string(),
            intent_id.to_string(),
        );
        if let Some(existing) = self.reservation_keys.get(&key) {
            return Some(existing.clone());
        }
        let id = format!("reservation-{}", self.reservations.len() + 1);
        self.reservation_keys.insert(key, id.clone());
        self.reservations.insert(
            id.clone(),
            RiskReservation {
                id: id.clone(),
                run_id: run_id.to_string(),
                strategy_id: strategy_id.to_string(),
                intent_id: intent_id.to_string(),
                amount,
                active: true,
                order_id: None,
            },
        );
        self.events.push(JournalEvent {
            event_type: "journal.reservation_added".to_string(),
            payload: json!({"reservation_id": id, "run_id": run_id, "strategy_id": strategy_id, "intent_id": intent_id, "amount": amount}),
        });
        Some(id)
    }

    pub fn link_reservation_order(&mut self, reservation_id: &str, order_id: &str) {
        if let Some(reservation) = self.reservations.get_mut(reservation_id) {
            reservation.order_id = Some(order_id.to_string());
        }
        self.events.push(JournalEvent {
            event_type: "journal.reservation_order_linked".to_string(),
            payload: json!({"reservation_id": reservation_id, "order_id": order_id}),
        });
    }

    pub fn record_order_entry(&mut self, order_id: &str, strategy_id: &str) {
        self.events.push(JournalEvent {
            event_type: "journal.order_entry_recorded".to_string(),
            payload: json!({"order_id": order_id, "strategy_id": strategy_id}),
        });
    }

    pub fn release_reservations_for_run(&mut self, run_id: &str) {
        for reservation in self.reservations.values_mut() {
            if reservation.run_id == run_id {
                reservation.active = false;
            }
        }
    }

    pub fn count_active_reservations(&self, strategy_id: &str) -> usize {
        self.reservations
            .values()
            .filter(|reservation| reservation.strategy_id == strategy_id && reservation.active)
            .count()
    }

    pub fn sum_active_reservations(&self, strategy_id: &str) -> f64 {
        self.reservations
            .values()
            .filter(|reservation| reservation.strategy_id == strategy_id && reservation.active)
            .map(|reservation| reservation.amount)
            .sum()
    }

    pub fn reservation(&self, reservation_id: &str) -> Option<&RiskReservation> {
        self.reservations.get(reservation_id)
    }

    pub fn event_types(&self) -> Vec<&str> {
        self.events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect()
    }
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let parts = keys
                .into_iter()
                .map(|key| format!("{:?}:{}", key, canonical_json(&map[key])))
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
        other => other.to_string(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FakeEngine {
        backend_name: String,
        commands: Vec<String>,
    }

    impl FakeEngine {
        fn new(backend_name: &str) -> Self {
            Self {
                backend_name: backend_name.to_string(),
                commands: Vec::new(),
            }
        }
    }

    impl SqlEngine for FakeEngine {
        fn backend_name(&self) -> &str {
            &self.backend_name
        }

        fn exec_driver_sql(&mut self, sql: &str) {
            self.commands.push(sql.to_string());
        }
    }

    #[derive(Default, Debug)]
    struct FakeMetadata {
        created_with: Option<String>,
    }

    impl Metadata for FakeMetadata {
        fn create_all(&mut self, engine_backend_name: &str) {
            self.created_with = Some(engine_backend_name.to_string());
        }
    }

    #[test]
    fn source_backend_protocol_is_importable() {
        let backend: Box<dyn JournalBackend> = Box::new(SQLiteBackend);

        assert_eq!(backend.name(), "sqlite");
        assert_eq!(
            backend.connect_args("sqlite:///tmp/tradeassembly.db"),
            Map::from_iter([("check_same_thread".to_string(), json!(false))])
        );
    }

    #[test]
    fn backend_for_url_selects_sqlite_and_postgres() {
        assert!(matches!(
            backend_for_url("sqlite:///tmp/tradeassembly.db"),
            Backend::SQLite(_)
        ));
        assert!(matches!(
            backend_for_url("/tmp/tradeassembly.db"),
            Backend::SQLite(_)
        ));
        assert!(matches!(
            backend_for_url("postgresql://localhost/tradeassembly"),
            Backend::Postgres(_)
        ));
    }

    #[test]
    fn backend_for_engine_uses_sqlalchemy_like_backend_name() {
        assert!(matches!(
            backend_for_engine(&FakeEngine::new("sqlite")),
            Backend::SQLite(_)
        ));
        assert!(matches!(
            backend_for_engine(&FakeEngine::new("postgresql")),
            Backend::Postgres(_)
        ));
    }

    #[test]
    fn sqlite_backend_json_and_best_effort_init() {
        let mut engine = FakeEngine::new("sqlite");
        let mut metadata = FakeMetadata::default();
        let backend = SQLiteBackend;

        backend.configure_engine(&mut engine);
        backend.init_db(&mut engine, &mut metadata);

        assert_eq!(metadata.created_with.as_deref(), Some("sqlite"));
        assert!(engine
            .commands
            .contains(&"PRAGMA journal_mode=WAL;".to_string()));
        assert!(engine
            .commands
            .iter()
            .any(|command| command.contains("ADD COLUMN execution_mode")));
        assert!(engine
            .commands
            .iter()
            .any(|command| command.contains("idx_reservations_active_intent")));
        let encoded = backend.encode_json(json!({"a": 1}));
        assert_eq!(encoded, json!("{\"a\":1}"));
        assert_eq!(backend.decode_json(encoded, None), json!({"a": 1}));
        assert_eq!(
            backend.decode_json(json!("{bad"), Some(json!({"fallback": true}))),
            json!({"fallback": true})
        );
    }

    #[test]
    fn postgres_backend_json_is_native_and_best_effort_init() {
        let mut engine = FakeEngine::new("postgresql");
        let mut metadata = FakeMetadata::default();
        let backend = PostgresBackend;
        let payload = json!({"a": 1});

        backend.configure_engine(&mut engine);
        backend.init_db(&mut engine, &mut metadata);

        assert_eq!(metadata.created_with.as_deref(), Some("postgresql"));
        assert!(engine
            .commands
            .iter()
            .any(|command| command.contains("ADD COLUMN IF NOT EXISTS execution_mode")));
        assert!(engine
            .commands
            .iter()
            .any(|command| command.contains("idx_reservations_active_intent")));
        assert_eq!(backend.encode_json(payload.clone()), payload);
        assert_eq!(backend.decode_json(payload.clone(), None), payload);
    }

    #[test]
    fn top_level_journal_imports_preserve_jsonl_replay_and_hash_contract() {
        let mut journal = JsonlJournal::default();

        journal.append("demo.event", json!({"value": 1}));

        assert_eq!(
            replay(journal.read()),
            Map::from_iter([("demo.event".to_string(), json!(1))])
        );
        assert!(!journal_event_hash("demo.event", &json!({"value": 1})).is_empty());
    }

    #[test]
    fn noop_source_ports_are_safe_defaults() {
        let mut writer = NoopJournalWriter;
        let reader = NoopReservationReader;

        assert!(writer
            .add_reservation("run-1", "strategy-1", "intent-1", 10.0)
            .is_none());
        writer.link_reservation_order("missing", "order-1");
        writer.record_order_entry("order-1", "strategy-1");
        writer.release_reservations_for_run("run-1");
        assert_eq!(reader.count_active_reservations("strategy-1"), 0);
        assert_eq!(reader.sum_active_reservations("strategy-1"), 0.0);
    }

    #[test]
    fn journal_db_adapter_persists_reservation_and_order_evidence() {
        let mut adapter = JournalDbAdapter::default();

        let reservation_id = adapter
            .add_reservation("run-1", "strategy-1", "intent-1", 25.5)
            .expect("reservation id");
        let duplicate_id = adapter
            .add_reservation("run-1", "strategy-1", "intent-1", 25.5)
            .expect("duplicate reservation id");

        assert_eq!(reservation_id, duplicate_id);
        assert_eq!(adapter.count_active_reservations("strategy-1"), 1);
        assert_eq!(adapter.sum_active_reservations("strategy-1"), 25.5);

        adapter.link_reservation_order(&reservation_id, "order-1");
        assert_eq!(
            adapter
                .reservation(&reservation_id)
                .and_then(|item| item.order_id.as_deref()),
            Some("order-1")
        );

        adapter.record_order_entry("order-1", "strategy-1");
        let event_types = adapter.event_types();
        assert!(event_types.contains(&"journal.reservation_added"));
        assert!(event_types.contains(&"journal.reservation_order_linked"));
        assert!(event_types.contains(&"journal.order_entry_recorded"));

        adapter.release_reservations_for_run("run-1");
        assert_eq!(adapter.count_active_reservations("strategy-1"), 0);
        assert_eq!(adapter.sum_active_reservations("strategy-1"), 0.0);
    }
}
