// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerRole {
    Scheduler,
    Runner,
    Oms,
    OrderStatus,
    OrderStream,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventEnvelope {
    pub topic: String,
    pub event_type: String,
    pub producer: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventDelivery {
    pub envelope: EventEnvelope,
    delivery_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AckedDelivery {
    pub envelope: EventEnvelope,
    pub acked: bool,
}

#[derive(Clone, Debug)]
pub struct InMemoryEventBus {
    role: WorkerRole,
    max_deliver: usize,
    queues: BTreeMap<String, VecDeque<EventDelivery>>,
    dead_letters: BTreeMap<String, Vec<EventDelivery>>,
}

impl InMemoryEventBus {
    pub fn new(role: WorkerRole, max_deliver: usize) -> Self {
        Self {
            role,
            max_deliver: max_deliver.max(1),
            queues: BTreeMap::new(),
            dead_letters: BTreeMap::new(),
        }
    }

    pub fn role(&self) -> &WorkerRole {
        &self.role
    }

    pub fn publish(&mut self, envelope: EventEnvelope) {
        self.queues
            .entry(envelope.topic.clone())
            .or_default()
            .push_back(EventDelivery {
                envelope,
                delivery_count: 0,
            });
    }

    pub fn pull(&mut self, topic: &str) -> Option<EventDelivery> {
        let mut delivery = self.queues.get_mut(topic)?.pop_front()?;
        delivery.delivery_count += 1;
        Some(delivery)
    }

    pub fn nack(&mut self, delivery: EventDelivery) {
        let topic = delivery.envelope.topic.clone();
        if delivery.delivery_count >= self.max_deliver {
            self.dead_letters.entry(topic).or_default().push(delivery);
        } else {
            self.queues.entry(topic).or_default().push_back(delivery);
        }
    }

    pub fn list(&self, topic: &str) -> Vec<EventDelivery> {
        self.dead_letters.get(topic).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KafkaConfig {
    pub bootstrap_servers: String,
    pub client_id: String,
}

#[derive(Clone, Debug)]
pub struct KafkaBus {
    config: KafkaConfig,
    bus: InMemoryEventBus,
}

impl KafkaBus {
    pub fn new(config: KafkaConfig) -> Self {
        Self {
            config,
            bus: InMemoryEventBus::new(WorkerRole::Runner, 3),
        }
    }

    pub fn config(&self) -> &KafkaConfig {
        &self.config
    }

    pub fn publish_json(
        &mut self,
        topic: &str,
        _payload_json: &str,
        idempotency_key: Option<&str>,
    ) {
        self.bus.publish(EventEnvelope {
            topic: topic.to_string(),
            event_type: topic.trim_end_matches('s').to_string(),
            producer: self.config.client_id.clone(),
            idempotency_key: idempotency_key.unwrap_or("").to_string(),
        });
    }

    pub fn pull(&mut self, topic: &str) -> Option<EventDelivery> {
        self.bus.pull(topic)
    }

    pub fn ack(&self, delivery: EventDelivery) -> AckedDelivery {
        AckedDelivery {
            envelope: delivery.envelope,
            acked: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KinesisConfig {
    pub stream_name: String,
    pub region_name: String,
}

#[derive(Clone, Debug)]
pub struct KinesisBus {
    config: KinesisConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderClosedError {
    pub provider: String,
    pub reason: String,
}

impl KinesisBus {
    pub fn new(config: KinesisConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &KinesisConfig {
        &self.config
    }

    pub fn publish_json(
        &self,
        _topic: &str,
        _payload_json: &str,
        _idempotency_key: Option<&str>,
    ) -> Result<(), ProviderClosedError> {
        Err(ProviderClosedError {
            provider: "kinesis".to_string(),
            reason: "Kinesis source adapter is importable but disabled until publish/pull/ack/nack/dead-letter semantics are implemented".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nats_compatibility_uses_same_event_port_contract() {
        let mut compat = InMemoryEventBus::new(WorkerRole::Runner, 1);
        let envelope = EventEnvelope {
            topic: "strategy.triggers".to_string(),
            event_type: "strategy.trigger".to_string(),
            producer: "test".to_string(),
            idempotency_key: "nats-compat".to_string(),
        };

        compat.publish(envelope);
        let handle = compat.pull("strategy.triggers").expect("delivery");
        compat.nack(handle);

        let dead = compat.list("strategy.triggers");
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].envelope.event_type, "strategy.trigger");
    }

    #[test]
    fn kafka_source_module_is_durable_bus_compatible() {
        let cfg = KafkaConfig {
            bootstrap_servers: "broker:9092".to_string(),
            client_id: "runner".to_string(),
        };
        let mut bus = KafkaBus::new(cfg);

        bus.publish_json(
            "strategy.triggers",
            r#"{"activation_id":"act-1"}"#,
            Some("trigger-1"),
        );
        let delivery = bus.pull("strategy.triggers").expect("delivery");

        assert_eq!(delivery.envelope.idempotency_key, "trigger-1");
        let acked = bus.ack(delivery);
        assert!(acked.acked);
    }

    #[test]
    fn kinesis_source_module_is_importable_but_fails_closed() {
        let cfg = KinesisConfig {
            stream_name: "tradeassembly-triggers".to_string(),
            region_name: "us-east-1".to_string(),
        };
        let bus = KinesisBus::new(cfg);

        let error = bus
            .publish_json(
                "strategy.triggers",
                r#"{"activation_id":"act-1"}"#,
                Some("trigger-1"),
            )
            .expect_err("kinesis must fail closed");

        assert_eq!(error.provider, "kinesis");
        assert!(error.reason.contains("publish/pull/ack/nack/dead-letter"));
    }
}
