// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskReservation {
    pub id: String,
    pub run_id: String,
    pub activation_id: String,
    pub strategy_id: String,
    pub order_id: String,
    pub symbol: String,
    pub notional: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutboxEvent {
    pub id: String,
    pub topic: String,
    pub event_type: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub attempts: usize,
    pub status: String,
    pub published_at: Option<String>,
    pub last_error: Option<String>,
}

impl OutboxEvent {
    pub fn new(
        id: impl Into<String>,
        topic: impl Into<String>,
        aggregate_type: impl Into<String>,
        aggregate_id: impl Into<String>,
        event_type: impl Into<String>,
        payload: Value,
        idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            topic: topic.into(),
            event_type: event_type.into(),
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.into(),
            idempotency_key: idempotency_key.into(),
            payload,
            attempts: 0,
            status: "pending".to_string(),
            published_at: None,
            last_error: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageEnvelope {
    pub correlation_id: String,
    pub idempotency_key: String,
    pub attempt: usize,
    pub producer: String,
    pub message_type: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Delivery {
    pub subject: String,
    pub envelope: MessageEnvelope,
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryOutbox {
    events: Vec<OutboxEvent>,
}

impl InMemoryOutbox {
    pub fn reserve_risk(&mut self, reservation: RiskReservation) -> OutboxEvent {
        let event = OutboxEvent::new(
            format!("risk:{}:reserved:outbox", reservation.id),
            "risk.events",
            "risk_reservation",
            reservation.id.clone(),
            "risk.reserved",
            json!({
                "id": reservation.id,
                "run_id": reservation.run_id,
                "activation_id": reservation.activation_id,
                "strategy_id": reservation.strategy_id,
                "order_id": reservation.order_id,
                "symbol": reservation.symbol,
                "notional": reservation.notional,
            }),
            format!("risk:{}:reserved", reservation.id),
        );
        self.append(event.clone());
        event
    }

    pub fn append(&mut self, event: OutboxEvent) -> OutboxEvent {
        if !self
            .events
            .iter()
            .any(|existing| existing.idempotency_key == event.idempotency_key)
        {
            self.events.push(event.clone());
        }
        event
    }

    pub fn list_all(&self) -> Vec<OutboxEvent> {
        self.events.clone()
    }

    pub fn skip_locked_claim_semantics(&self) -> &'static str {
        "for update skip locked"
    }

    pub fn claim_batch(&mut self, topic: Option<&str>, limit: usize) -> Vec<OutboxEvent> {
        let mut claimed = Vec::new();
        for event in &mut self.events {
            if claimed.len() >= limit {
                break;
            }
            if !matches!(event.status.as_str(), "pending" | "failed") {
                continue;
            }
            if topic.is_some_and(|requested| requested != event.topic) {
                continue;
            }
            event.status = "claimed".to_string();
            event.attempts += 1;
            claimed.push(event.clone());
        }
        claimed
    }

    pub fn mark_failed(&mut self, event_id: &str, error: impl Into<String>) -> Option<OutboxEvent> {
        let event = self.events.iter_mut().find(|event| event.id == event_id)?;
        event.status = "failed".to_string();
        event.last_error = Some(error.into());
        Some(event.clone())
    }

    pub fn mark_published(&mut self, event_id: &str) -> Option<OutboxEvent> {
        let event = self.events.iter_mut().find(|event| event.id == event_id)?;
        event.status = "published".to_string();
        event.published_at = Some("published".to_string());
        event.last_error = None;
        Some(event.clone())
    }

    pub fn list(&self, status: &str) -> Vec<OutboxEvent> {
        self.events
            .iter()
            .filter(|event| event.status == status)
            .cloned()
            .collect()
    }
}

pub trait PublishBus {
    fn publish(&mut self, subject: &str, envelope: MessageEnvelope) -> Result<(), String>;
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryBus {
    queues: BTreeMap<String, VecDeque<Delivery>>,
}

impl PublishBus for InMemoryBus {
    fn publish(&mut self, subject: &str, envelope: MessageEnvelope) -> Result<(), String> {
        self.queues
            .entry(subject.to_string())
            .or_default()
            .push_back(Delivery {
                subject: subject.to_string(),
                envelope,
            });
        Ok(())
    }
}

impl InMemoryBus {
    pub fn pull(&mut self, subject: &str) -> Option<Delivery> {
        self.queues.get_mut(subject)?.pop_front()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailingBus {
    error: String,
}

impl FailingBus {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

impl PublishBus for FailingBus {
    fn publish(&mut self, _subject: &str, _envelope: MessageEnvelope) -> Result<(), String> {
        Err(self.error.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxPublishResult {
    pub claimed: usize,
    pub published: usize,
    pub failed: usize,
    pub event_ids: Vec<String>,
}

pub struct OutboxPublisher<'a, B: PublishBus> {
    outbox: &'a mut InMemoryOutbox,
    bus: &'a mut B,
    producer: String,
}

impl<'a, B: PublishBus> OutboxPublisher<'a, B> {
    pub fn new(outbox: &'a mut InMemoryOutbox, bus: &'a mut B) -> Self {
        Self {
            outbox,
            bus,
            producer: "outbox".to_string(),
        }
    }

    pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
        self.producer = producer.into();
        self
    }

    pub fn publish_pending(&mut self, topic: Option<&str>, limit: usize) -> OutboxPublishResult {
        let claimed = self.outbox.claim_batch(topic, limit);
        let mut published = 0;
        let mut failed = 0;
        let mut event_ids = Vec::new();

        for event in claimed {
            event_ids.push(event.id.clone());
            match self.bus.publish(&event.topic, self.envelope(&event)) {
                Ok(()) => {
                    self.outbox.mark_published(&event.id);
                    published += 1;
                }
                Err(error) => {
                    self.outbox.mark_failed(&event.id, error);
                    failed += 1;
                }
            }
        }

        OutboxPublishResult {
            claimed: event_ids.len(),
            published,
            failed,
            event_ids,
        }
    }

    fn envelope(&self, event: &OutboxEvent) -> MessageEnvelope {
        MessageEnvelope {
            correlation_id: event.id.clone(),
            idempotency_key: event.idempotency_key.clone(),
            attempt: event.attempts.max(1),
            producer: self.producer.clone(),
            message_type: event.event_type.clone(),
            payload: json!({
                "outbox_event_id": event.id,
                "aggregate_type": event.aggregate_type,
                "aggregate_id": event.aggregate_id,
                "payload": event.payload,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reservation() -> RiskReservation {
        RiskReservation {
            id: "risk-1".to_string(),
            run_id: "run-1".to_string(),
            activation_id: "act-1".to_string(),
            strategy_id: "strat-1".to_string(),
            order_id: "order-1".to_string(),
            symbol: "BTC/USD".to_string(),
            notional: 25,
        }
    }

    #[test]
    fn failed_claims_remain_retryable() {
        let mut outbox = InMemoryOutbox::default();
        outbox.reserve_risk(reservation());

        let first = outbox.claim_batch(Some("risk.events"), 10);
        outbox.mark_failed(&first[0].id, "bus down");
        let second = outbox.claim_batch(Some("risk.events"), 10);

        assert_eq!(first[0].attempts, 1);
        assert_eq!(second[0].attempts, 2);
    }
}
