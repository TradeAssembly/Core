// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use tradeassembly_runtime::bus::{
    create_bus, BusOptions, BusProvider, BusRole, MessageEnvelope, RuntimeProfileName,
};

fn envelope(idempotency_key: &str) -> MessageEnvelope {
    MessageEnvelope::new(
        "test",
        "strategy.trigger",
        idempotency_key,
        serde_json::json!({"activation_id": "act-1"}),
    )
    .with_activation_id("act-1")
}

#[test]
fn create_bus_defaults_to_memory_for_local_profile() {
    let bus = create_bus(BusOptions::default()).expect("memory bus");

    assert_eq!(bus.provider(), BusProvider::Memory);
}

#[test]
fn distributed_profile_defaults_to_nats_jetstream_when_provider_is_omitted() {
    let mut bus = create_bus(BusOptions {
        profile: RuntimeProfileName::DistributedDev,
        bus_url: Some("nats://bus:4222".to_string()),
        consumer: Some("runner".to_string()),
        ..BusOptions::default()
    })
    .expect("NATS JetStream bus");

    assert_eq!(bus.provider(), BusProvider::NatsJetstream);
    bus.publish("strategy.triggers", envelope("profile-nats-trigger"))
        .expect("publish");
    let delivery = bus.pull("strategy.triggers").expect("delivery");
    assert_eq!(delivery.envelope.idempotency_key, "profile-nats-trigger");
}

#[test]
fn nats_jetstream_dead_letters_acknowledge_the_original_delivery() {
    let mut bus = create_bus(BusOptions {
        provider: Some(BusProvider::NatsJetstream),
        profile: RuntimeProfileName::Production,
        bus_url: Some("nats://bus:4222".to_string()),
        max_deliver: Some(1),
        ..BusOptions::default()
    })
    .expect("NATS JetStream bus");

    bus.publish("strategy.triggers", envelope("nats-poison"))
        .expect("publish");
    let delivery = bus.pull("strategy.triggers").expect("delivery");
    bus.nack(delivery).expect("dead letter");

    let dead = bus.dead_letters("strategy.triggers");
    assert_eq!(dead.len(), 1);
    assert!(dead[0].acked);
    assert!(dead[0].dead_lettered);
}

#[test]
fn external_profiles_reject_local_only_bus_providers() {
    let error = create_bus(BusOptions {
        provider: Some(BusProvider::Memory),
        profile: RuntimeProfileName::Production,
        bus_url: Some("nats://bus:4222".to_string()),
        ..BusOptions::default()
    })
    .expect_err("local provider rejected");

    assert_eq!(error.code, "configuration_error");
    assert_eq!(error.details["profile"], "production");
    assert_eq!(error.details["provider"], "memory");
}

#[test]
fn memory_bus_redelivers_and_dead_letters() {
    let mut bus = create_bus(BusOptions {
        provider: Some(BusProvider::Memory),
        max_deliver: Some(2),
        ..BusOptions::default()
    })
    .expect("memory bus");

    bus.publish("strategy.triggers", envelope("memory-poison"))
        .expect("publish");
    let first = bus.pull("strategy.triggers").expect("first");
    bus.nack(first).expect("nack");
    let second = bus.pull("strategy.triggers").expect("second");
    assert_eq!(second.delivery_count, 2);
    bus.nack(second.clone()).expect("dead letter");

    let dead = bus.dead_letters("strategy.triggers");
    assert_eq!(dead.len(), 1);
    assert!(dead[0].dead_lettered);
    assert_eq!(dead[0].envelope.idempotency_key, "memory-poison");
}

#[test]
fn sqlite_bus_instances_share_path_state() {
    let path = format!("test-bus-{}", std::process::id());
    let mut first = create_bus(BusOptions {
        provider: Some(BusProvider::Sqlite),
        bus_url: Some(path.clone()),
        ..BusOptions::default()
    })
    .expect("sqlite bus");
    first
        .publish("strategy.triggers", envelope("sqlite-trigger"))
        .expect("publish");

    let mut second = create_bus(BusOptions {
        provider: Some(BusProvider::Sqlite),
        bus_url: Some(path),
        ..BusOptions::default()
    })
    .expect("sqlite bus");
    let delivery = second.pull("strategy.triggers").expect("delivery");
    assert_eq!(delivery.delivery_count, 1);
    second.ack(delivery).expect("ack");
    assert!(second.pull("strategy.triggers").is_none());
}

#[test]
fn kafka_provider_publishes_pulls_acks_and_dead_letters() {
    let mut bus = create_bus(BusOptions {
        provider: Some(BusProvider::Kafka),
        bus_url: Some("kafka:9092".to_string()),
        consumer: Some("runner".to_string()),
        role: BusRole::Runner,
        max_deliver: Some(1),
        ..BusOptions::default()
    })
    .expect("kafka bus");

    assert_eq!(bus.provider(), BusProvider::Kafka);
    bus.publish("strategy.triggers", envelope("kafka-trigger"))
        .expect("publish");
    let delivery = bus.pull("strategy.triggers").expect("delivery");
    bus.nack(delivery.clone()).expect("dead letter");

    let dead = bus.dead_letters("strategy.triggers");
    assert_eq!(dead.len(), 1);
    assert!(dead[0].dead_lettered);
    assert_eq!(dead[0].envelope.idempotency_key, "kafka-trigger");
}

#[test]
fn kinesis_is_rejected_as_publish_only_for_durable_bus() {
    let error = create_bus(BusOptions {
        provider: Some(BusProvider::Kinesis),
        ..BusOptions::default()
    })
    .expect_err("kinesis rejected");

    assert_eq!(error.code, "configuration_error");
    assert_eq!(error.details["provider"], "kinesis");
    assert!(error.details["reason"]
        .as_str()
        .expect("reason")
        .contains("pull/ack/nack/dead-letter"));
}
