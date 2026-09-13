// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use tradeassembly_runtime::platform_events::{
    create_event_adapter, EventAdapterOptions, EventProvider, InMemoryDeadLetterStore,
    InMemoryEventBus, InMemoryOutboxStore, RuntimeProfileName, WorkerRole,
};

fn envelope(idempotency_key: &str) -> tradeassembly_runtime::platform_events::EventEnvelope {
    tradeassembly_runtime::platform_events::EventEnvelope::new(
        "strategy.triggers",
        "strategy.trigger",
        "test",
        idempotency_key,
        serde_json::json!({"activation_id": "act-1"}),
    )
}

#[test]
fn event_envelope_ack_nack_and_dead_letter_flow() {
    let mut bus = InMemoryEventBus::new(WorkerRole::Runner, 1);
    bus.publish(envelope("trigger-1"));
    let handle = bus.pull("strategy.triggers").expect("delivery");

    bus.nack(handle).expect("dead letter");

    let dead = bus.list(Some("strategy.triggers"));
    assert_eq!(dead.len(), 1);
    assert!(dead[0].dead_lettered);
    assert_eq!(dead[0].envelope.idempotency_key, "trigger-1");
}

#[test]
fn duplicate_and_stale_event_delivery_is_bounded_by_ack_nack_contracts() {
    let mut bus = InMemoryEventBus::new(WorkerRole::Runner, 2);
    bus.publish(envelope("duplicate-trigger"));
    let first = bus.pull("strategy.triggers").expect("first delivery");

    let redelivered = bus.redeliver_inflight();
    assert_eq!(redelivered, 1);
    let second = bus.pull("strategy.triggers").expect("redelivery");
    assert_eq!(second.envelope.idempotency_key, "duplicate-trigger");
    assert_eq!(second.delivery_count, 2);

    let terminal = bus.nack(second).expect("dead letter after bounded retry");
    assert!(terminal.dead_lettered);
    assert_eq!(bus.dead_letters("strategy.triggers").len(), 1);
    assert!(bus.ack(first).is_ok());
}

#[test]
fn unsupported_external_event_adapters_fail_closed() {
    let error = create_event_adapter(EventAdapterOptions {
        provider: Some(EventProvider::KafkaMemory),
        profile: RuntimeProfileName::Production,
        bus_url: Some("kafka.internal:9092".to_string()),
        ..EventAdapterOptions::default()
    })
    .expect_err("memory-backed kafka rejected in production");

    assert_eq!(error.code, "configuration_error");
    assert_eq!(error.details["provider"], "kafka-memory");
}

#[test]
fn in_memory_event_adapter_uses_same_event_ports() {
    let mut adapter = InMemoryEventBus::new(WorkerRole::Runner, 2);
    let event = tradeassembly_runtime::platform_events::EventEnvelope::new(
        "order.intents",
        "order.intent",
        "runner",
        "intent-1",
        serde_json::json!({"run_id": "run-1"}),
    );

    adapter.publish(event);
    let handle = adapter.pull("order.intents").expect("delivery");
    assert_eq!(handle.envelope.event_type, "order.intent");
    let acked = adapter.ack(handle).expect("ack");
    assert!(acked.acked);
}

#[test]
fn outbox_and_dead_letter_stores_are_platform_ports() {
    let mut outbox = InMemoryOutboxStore::default();
    let mut dead_letters = InMemoryDeadLetterStore::default();
    let event = tradeassembly_runtime::platform_events::EventEnvelope::new(
        "orders",
        "order.intent",
        "test",
        "order-1",
        serde_json::json!({}),
    );

    outbox.add(event.clone());
    outbox.add(event.clone());
    outbox.add(event.clone().with_event_id("duplicate-event-id"));
    assert_eq!(outbox.pending(100), vec![event.clone()]);
    outbox.mark_published(&event.event_id);
    assert!(outbox.pending(100).is_empty());

    let mut bus = InMemoryEventBus::new(WorkerRole::Runner, 1);
    bus.add(event.clone());
    bus.add(event.clone().with_event_id("duplicate-event-id"));
    assert_eq!(bus.pending(100), vec![event.clone()]);
    bus.publish(event);
    let handle = bus.pull("orders").expect("delivery");
    let dead = bus.nack(handle).expect("dead letter");
    dead_letters.add(dead.clone(), "max-deliver");
    assert!(dead_letters.list(Some("orders"))[0].dead_lettered);
}

#[test]
fn kafka_memory_provider_publishes_pulls_and_dead_letters() {
    let mut adapter = create_event_adapter(EventAdapterOptions {
        provider: Some(EventProvider::KafkaMemory),
        max_deliver: Some(1),
        ..EventAdapterOptions::default()
    })
    .expect("adapter");

    assert_eq!(adapter.provider(), EventProvider::KafkaMemory);
    adapter.publish(envelope("kafka-test"));
    let handle = adapter.pull("strategy.triggers").expect("delivery");
    adapter.nack(handle).expect("dead letter");
    assert_eq!(
        adapter.list(Some("strategy.triggers"))[0]
            .envelope
            .idempotency_key,
        "kafka-test"
    );
}

#[test]
fn external_profile_defaults_to_nats_and_rejects_memory() {
    let nats = create_event_adapter(EventAdapterOptions {
        profile: RuntimeProfileName::DistributedDev,
        bus_url: Some("nats://bus.internal:4222".to_string()),
        ..EventAdapterOptions::default()
    })
    .expect("NATS adapter");
    assert_eq!(nats.provider(), EventProvider::Nats);

    let error = create_event_adapter(EventAdapterOptions {
        provider: Some(EventProvider::Memory),
        profile: RuntimeProfileName::DistributedDev,
        bus_url: Some("nats://bus.internal:4222".to_string()),
        ..EventAdapterOptions::default()
    })
    .expect_err("memory rejected");
    assert_eq!(error.details["provider"], "memory");
}
