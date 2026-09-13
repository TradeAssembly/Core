// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::bus::ConfigError;
pub use crate::bus::RuntimeProfileName;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

static EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerRole {
    Scheduler,
    Runner,
    Oms,
    OrderStream,
    OrderStatus,
    OutboxPublisher,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventProvider {
    Memory,
    Sqlite,
    KafkaMemory,
    Kafka,
    Nats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventAdapterOptions {
    pub provider: Option<EventProvider>,
    pub profile: RuntimeProfileName,
    pub bus_url: Option<String>,
    pub role: WorkerRole,
    pub max_deliver: Option<usize>,
}

impl Default for EventAdapterOptions {
    fn default() -> Self {
        Self {
            provider: None,
            profile: RuntimeProfileName::Local,
            bus_url: None,
            role: WorkerRole::Runner,
            max_deliver: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventEnvelope {
    pub topic: String,
    pub event_type: String,
    pub producer: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub event_id: String,
    pub schema_version: String,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub attempt: usize,
}

impl EventEnvelope {
    pub fn new(
        topic: impl Into<String>,
        event_type: impl Into<String>,
        producer: impl Into<String>,
        idempotency_key: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            topic: topic.into(),
            event_type: event_type.into(),
            producer: producer.into(),
            idempotency_key: idempotency_key.into(),
            payload,
            event_id: next_id("event"),
            schema_version: "1.0".to_string(),
            correlation_id: next_id("correlation"),
            causation_id: None,
            attempt: 1,
        }
    }

    pub fn with_event_id(mut self, event_id: impl Into<String>) -> Self {
        self.event_id = event_id.into();
        self
    }

    pub fn next_attempt(&self, producer: Option<&str>) -> Self {
        let mut next = self.clone();
        next.event_id = next_id("event");
        next.causation_id = Some(self.event_id.clone());
        next.attempt += 1;
        if let Some(producer) = producer {
            next.producer = producer.to_string();
        }
        next
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventDelivery {
    pub envelope: EventEnvelope,
    pub delivery_count: usize,
    pub acked: bool,
    pub dead_lettered: bool,
}

#[derive(Clone, Debug)]
pub struct InMemoryEventBus {
    provider: EventProvider,
    pub role: WorkerRole,
    max_deliver: usize,
    queues: BTreeMap<String, VecDeque<EventDelivery>>,
    inflight: BTreeMap<String, EventDelivery>,
    dead_letters: Vec<EventDelivery>,
    outbox: BTreeMap<String, EventEnvelope>,
    outbox_idempotency: BTreeMap<String, String>,
    published: BTreeSet<String>,
    failed: BTreeMap<String, String>,
}

impl InMemoryEventBus {
    pub fn new(role: WorkerRole, max_deliver: usize) -> Self {
        Self::with_provider(EventProvider::Memory, role, max_deliver)
    }

    fn with_provider(provider: EventProvider, role: WorkerRole, max_deliver: usize) -> Self {
        Self {
            provider,
            role,
            max_deliver: max_deliver.max(1),
            queues: BTreeMap::new(),
            inflight: BTreeMap::new(),
            dead_letters: Vec::new(),
            outbox: BTreeMap::new(),
            outbox_idempotency: BTreeMap::new(),
            published: BTreeSet::new(),
            failed: BTreeMap::new(),
        }
    }

    pub fn provider(&self) -> EventProvider {
        self.provider
    }

    pub fn publish(&mut self, envelope: EventEnvelope) {
        self.queues
            .entry(envelope.topic.clone())
            .or_default()
            .push_back(EventDelivery {
                envelope,
                delivery_count: 1,
                acked: false,
                dead_lettered: false,
            });
    }

    pub fn pull(&mut self, topic: &str) -> Option<EventDelivery> {
        let delivery = self.queues.get_mut(topic)?.pop_front()?;
        self.inflight
            .insert(delivery.envelope.event_id.clone(), delivery.clone());
        Some(delivery)
    }

    pub fn ack(&mut self, mut delivery: EventDelivery) -> Result<EventDelivery, ConfigError> {
        self.inflight.remove(&delivery.envelope.event_id);
        delivery.acked = true;
        Ok(delivery)
    }

    pub fn nack(&mut self, mut delivery: EventDelivery) -> Result<EventDelivery, ConfigError> {
        self.inflight.remove(&delivery.envelope.event_id);
        if delivery.delivery_count >= self.max_deliver {
            delivery.dead_lettered = true;
            delivery.acked = true;
            self.dead_letters.push(delivery.clone());
            return Ok(delivery);
        }
        delivery.delivery_count += 1;
        self.queues
            .entry(delivery.envelope.topic.clone())
            .or_default()
            .push_front(delivery.clone());
        Ok(delivery)
    }

    pub fn redeliver_inflight(&mut self) -> usize {
        let inflight = self.inflight.values().cloned().collect::<Vec<_>>();
        let count = inflight.len();
        for delivery in inflight {
            let _ = self.nack(delivery);
        }
        count
    }

    pub fn list(&self, topic: Option<&str>) -> Vec<EventDelivery> {
        self.dead_letters
            .iter()
            .filter(|delivery| topic.is_none_or(|topic| delivery.envelope.topic == topic))
            .cloned()
            .collect()
    }

    pub fn dead_letters(&self, topic: &str) -> Vec<EventDelivery> {
        self.list(Some(topic))
    }

    pub fn add(&mut self, envelope: EventEnvelope) {
        if let Some(existing) = self.outbox_idempotency.get(&envelope.idempotency_key) {
            if existing != &envelope.event_id {
                return;
            }
        }
        self.outbox_idempotency
            .insert(envelope.idempotency_key.clone(), envelope.event_id.clone());
        self.outbox
            .entry(envelope.event_id.clone())
            .or_insert(envelope);
    }

    pub fn pending(&self, limit: usize) -> Vec<EventEnvelope> {
        self.outbox
            .values()
            .filter(|event| !self.published.contains(&event.event_id))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn mark_published(&mut self, event_id: &str) {
        self.published.insert(event_id.to_string());
    }

    pub fn mark_failed(&mut self, event_id: &str, error: impl Into<String>) {
        self.failed.insert(event_id.to_string(), error.into());
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryOutboxStore {
    pending: BTreeMap<String, EventEnvelope>,
    idempotency: BTreeMap<String, String>,
    published: BTreeSet<String>,
    failed: BTreeMap<String, String>,
}

impl InMemoryOutboxStore {
    pub fn add(&mut self, envelope: EventEnvelope) {
        if let Some(existing) = self.idempotency.get(&envelope.idempotency_key) {
            if existing != &envelope.event_id {
                return;
            }
        }
        self.idempotency
            .insert(envelope.idempotency_key.clone(), envelope.event_id.clone());
        self.pending
            .entry(envelope.event_id.clone())
            .or_insert(envelope);
    }

    pub fn pending(&self, limit: usize) -> Vec<EventEnvelope> {
        self.pending
            .values()
            .filter(|event| !self.published.contains(&event.event_id))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn mark_published(&mut self, event_id: &str) {
        self.published.insert(event_id.to_string());
    }

    pub fn mark_failed(&mut self, event_id: &str, error: impl Into<String>) {
        self.failed.insert(event_id.to_string(), error.into());
    }

    pub fn failed(&self) -> BTreeMap<String, String> {
        self.failed.clone()
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryDeadLetterStore {
    deliveries: Vec<EventDelivery>,
}

impl InMemoryDeadLetterStore {
    pub fn add(&mut self, mut delivery: EventDelivery, _reason: &str) {
        delivery.dead_lettered = true;
        self.deliveries.push(delivery);
    }

    pub fn list(&self, topic: Option<&str>) -> Vec<EventDelivery> {
        self.deliveries
            .iter()
            .filter(|delivery| topic.is_none_or(|topic| delivery.envelope.topic == topic))
            .cloned()
            .collect()
    }
}

pub fn create_event_adapter(options: EventAdapterOptions) -> Result<InMemoryEventBus, ConfigError> {
    let selected = options.provider.unwrap_or_else(|| {
        if options.profile != RuntimeProfileName::Local {
            EventProvider::Nats
        } else {
            EventProvider::Memory
        }
    });
    if options.profile != RuntimeProfileName::Local
        && matches!(
            selected,
            EventProvider::Memory | EventProvider::Sqlite | EventProvider::KafkaMemory
        )
    {
        return Err(configuration_error(
            "external profiles require NATS JetStream or an explicitly configured compatible durable-event adapter",
            json!({"profile": profile_name(options.profile), "provider": provider_name(selected)}),
        ));
    }
    Ok(InMemoryEventBus::with_provider(
        selected,
        options.role,
        options.max_deliver.unwrap_or(5),
    ))
}

fn configuration_error(message: impl Into<String>, details: Value) -> ConfigError {
    ConfigError {
        code: "configuration_error".to_string(),
        message: message.into(),
        details,
    }
}

fn provider_name(provider: EventProvider) -> &'static str {
    match provider {
        EventProvider::Memory => "memory",
        EventProvider::Sqlite => "sqlite",
        EventProvider::KafkaMemory => "kafka-memory",
        EventProvider::Kafka => "kafka",
        EventProvider::Nats => "nats",
    }
}

fn profile_name(profile: RuntimeProfileName) -> &'static str {
    match profile {
        RuntimeProfileName::Local => "local",
        RuntimeProfileName::DistributedDev => "distributed-dev",
        RuntimeProfileName::Production => "production",
    }
}

fn next_id(prefix: &str) -> String {
    let id = EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{id}")
}
