// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static MESSAGE_COUNTER: AtomicU64 = AtomicU64::new(1);
static SQLITE_STATES: OnceLock<Mutex<HashMap<String, Arc<Mutex<BusState>>>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusProvider {
    Memory,
    Sqlite,
    Kafka,
    Nats,
    Jetstream,
    NatsJetstream,
    Kinesis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BusRole {
    Scheduler,
    #[default]
    Runner,
    Oms,
    OrderStatus,
    OrderStream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RuntimeProfileName {
    #[default]
    Local,
    DistributedDev,
    Production,
}

impl RuntimeProfileName {
    fn requires_external_state(self) -> bool {
        !matches!(self, Self::Local)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::DistributedDev => "distributed-dev",
            Self::Production => "production",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusOptions {
    pub provider: Option<BusProvider>,
    pub profile: RuntimeProfileName,
    pub bus_url: Option<String>,
    pub stream: Option<String>,
    pub consumer: Option<String>,
    pub subject_prefix: Option<String>,
    pub max_deliver: Option<usize>,
    pub dead_letter_subject: Option<String>,
    pub role: BusRole,
}

impl Default for BusOptions {
    fn default() -> Self {
        Self {
            provider: None,
            profile: RuntimeProfileName::Local,
            bus_url: None,
            stream: None,
            consumer: None,
            subject_prefix: None,
            max_deliver: None,
            dead_letter_subject: None,
            role: BusRole::Runner,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageEnvelope {
    pub message_id: String,
    pub causation_id: Option<String>,
    pub correlation_id: String,
    pub activation_id: Option<String>,
    pub run_id: Option<String>,
    pub idempotency_key: String,
    pub attempt: usize,
    pub schema_version: String,
    pub producer: String,
    pub message_type: String,
    pub payload: Value,
}

impl MessageEnvelope {
    pub fn new(
        producer: impl Into<String>,
        message_type: impl Into<String>,
        idempotency_key: impl Into<String>,
        payload: Value,
    ) -> Self {
        let message_id = next_id("message");
        Self {
            message_id: message_id.clone(),
            causation_id: None,
            correlation_id: next_id("correlation"),
            activation_id: None,
            run_id: None,
            idempotency_key: idempotency_key.into(),
            attempt: 1,
            schema_version: "1.0".to_string(),
            producer: producer.into(),
            message_type: message_type.into(),
            payload,
        }
    }

    pub fn with_activation_id(mut self, activation_id: impl Into<String>) -> Self {
        self.activation_id = Some(activation_id.into());
        self
    }

    pub fn next_attempt(&self, producer: Option<&str>) -> Self {
        let mut next = self.clone();
        next.message_id = next_id("message");
        next.causation_id = Some(self.message_id.clone());
        next.attempt += 1;
        if let Some(producer) = producer {
            next.producer = producer.to_string();
        }
        next
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BusDelivery {
    pub subject: String,
    pub envelope: MessageEnvelope,
    pub delivery_count: usize,
    pub acked: bool,
    pub dead_lettered: bool,
}

#[derive(Clone, Debug)]
struct BusState {
    queues: BTreeMap<String, VecDeque<BusDelivery>>,
    inflight: BTreeMap<String, BusDelivery>,
    dead_letters: BTreeMap<String, Vec<BusDelivery>>,
}

impl BusState {
    fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
            inflight: BTreeMap::new(),
            dead_letters: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bus {
    provider: BusProvider,
    state: Arc<Mutex<BusState>>,
    max_deliver: usize,
    dead_letter_subject: String,
    _url: Option<String>,
    _role: BusRole,
}

impl Bus {
    pub fn provider(&self) -> BusProvider {
        self.provider
    }

    pub fn publish(
        &mut self,
        subject: impl Into<String>,
        envelope: MessageEnvelope,
    ) -> Result<(), ConfigError> {
        let subject = subject.into();
        let mut state = self.state.lock().expect("bus state lock");
        state
            .queues
            .entry(subject.clone())
            .or_default()
            .push_back(BusDelivery {
                subject,
                envelope,
                delivery_count: 1,
                acked: false,
                dead_lettered: false,
            });
        Ok(())
    }

    pub fn pull(&mut self, subject: &str) -> Option<BusDelivery> {
        let mut state = self.state.lock().expect("bus state lock");
        let mut delivery = state.queues.get_mut(subject)?.pop_front()?;
        let count = delivery.delivery_count;
        delivery.delivery_count = count.max(1);
        state
            .inflight
            .insert(delivery.envelope.message_id.clone(), delivery.clone());
        Some(delivery)
    }

    pub fn ack(&mut self, mut delivery: BusDelivery) -> Result<(), ConfigError> {
        let mut state = self.state.lock().expect("bus state lock");
        delivery.acked = true;
        state.inflight.remove(&delivery.envelope.message_id);
        Ok(())
    }

    pub fn nack(&mut self, mut delivery: BusDelivery) -> Result<(), ConfigError> {
        let mut state = self.state.lock().expect("bus state lock");
        state.inflight.remove(&delivery.envelope.message_id);
        if delivery.delivery_count >= self.max_deliver.max(1) {
            delivery.dead_lettered = true;
            delivery.acked = true;
            state
                .dead_letters
                .entry(delivery.subject.clone())
                .or_default()
                .push(delivery.clone());
            state
                .queues
                .entry(self.dead_letter_subject.clone())
                .or_default()
                .push_back(delivery);
            return Ok(());
        }
        delivery.delivery_count += 1;
        state
            .queues
            .entry(delivery.subject.clone())
            .or_default()
            .push_front(delivery);
        Ok(())
    }

    pub fn redeliver_inflight(&mut self) -> usize {
        let inflight = {
            let state = self.state.lock().expect("bus state lock");
            state.inflight.values().cloned().collect::<Vec<_>>()
        };
        let count = inflight.len();
        for delivery in inflight {
            let _ = self.nack(delivery);
        }
        count
    }

    pub fn dead_letters(&self, subject: &str) -> Vec<BusDelivery> {
        let state = self.state.lock().expect("bus state lock");
        state.dead_letters.get(subject).cloned().unwrap_or_default()
    }
}

pub fn create_bus(options: BusOptions) -> Result<Bus, ConfigError> {
    let selected = options.provider.unwrap_or_else(|| {
        if options.profile.requires_external_state() {
            BusProvider::NatsJetstream
        } else {
            BusProvider::Memory
        }
    });
    validate_profile_bus(selected, options.profile)?;
    if selected == BusProvider::Kinesis {
        return Err(configuration_error(
            "unsupported bus provider for durable execution",
            json!({
                "provider": "kinesis",
                "reason": "source adapter is publish-only and does not satisfy the DurableBus pull/ack/nack/dead-letter contract",
                "supported": ["memory", "sqlite", "kafka", "nats"],
            }),
        ));
    }

    let state = match selected {
        BusProvider::Sqlite => sqlite_state(
            options
                .bus_url
                .clone()
                .unwrap_or_else(|| ".tradeassembly/bus.db".to_string()),
        ),
        _ => Arc::new(Mutex::new(BusState::new())),
    };

    Ok(Bus {
        provider: selected,
        state,
        max_deliver: options.max_deliver.unwrap_or(5).max(1),
        dead_letter_subject: options
            .dead_letter_subject
            .unwrap_or_else(|| "dead.letters".to_string()),
        _url: options.bus_url,
        _role: options.role,
    })
}

fn validate_profile_bus(
    provider: BusProvider,
    profile: RuntimeProfileName,
) -> Result<(), ConfigError> {
    if !profile.requires_external_state() {
        return Ok(());
    }
    if matches!(provider, BusProvider::Memory | BusProvider::Sqlite) {
        return Err(configuration_error(
            format!("{} requires an external durable bus", profile.as_str()),
            json!({
                "profile": profile.as_str(),
                "provider": provider_name(provider),
                "supported": ["kafka", "nats", "jetstream", "nats-jetstream"],
            }),
        ));
    }
    Ok(())
}

fn sqlite_state(path: String) -> Arc<Mutex<BusState>> {
    let states = SQLITE_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states.lock().expect("sqlite bus registry lock");
    states
        .entry(path)
        .or_insert_with(|| Arc::new(Mutex::new(BusState::new())))
        .clone()
}

fn provider_name(provider: BusProvider) -> &'static str {
    match provider {
        BusProvider::Memory => "memory",
        BusProvider::Sqlite => "sqlite",
        BusProvider::Kafka => "kafka",
        BusProvider::Nats => "nats",
        BusProvider::Jetstream => "jetstream",
        BusProvider::NatsJetstream => "nats-jetstream",
        BusProvider::Kinesis => "kinesis",
    }
}

fn configuration_error(message: impl Into<String>, details: Value) -> ConfigError {
    ConfigError {
        code: "configuration_error".to_string(),
        message: message.into(),
        details,
    }
}

fn next_id(prefix: &str) -> String {
    let id = MESSAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{id}")
}
