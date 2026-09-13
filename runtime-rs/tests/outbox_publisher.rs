// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use tradeassembly_runtime::outbox::{
    FailingBus, InMemoryBus, InMemoryOutbox, OutboxEvent, OutboxPublisher, RiskReservation,
};

fn make_reservation() -> RiskReservation {
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
fn outbox_publisher_claims_publishes_and_marks_events() {
    let mut outbox = InMemoryOutbox::default();
    outbox.reserve_risk(make_reservation());
    let mut bus = InMemoryBus::default();

    let result = {
        let mut publisher =
            OutboxPublisher::new(&mut outbox, &mut bus).with_producer("risk-outbox");
        publisher.publish_pending(Some("risk.events"), 10)
    };
    let delivery = bus.pull("risk.events").expect("delivery");

    assert_eq!(result.claimed, 1);
    assert_eq!(result.published, 1);
    assert_eq!(result.failed, 0);
    assert_eq!(delivery.envelope.producer, "risk-outbox");
    assert_eq!(delivery.envelope.message_type, "risk.reserved");
    assert_eq!(delivery.envelope.idempotency_key, "risk:risk-1:reserved");
    assert_eq!(
        delivery.envelope.payload["aggregate_type"],
        "risk_reservation"
    );
    assert_eq!(delivery.envelope.payload["payload"]["id"], "risk-1");
    assert_eq!(outbox.list("published")[0].id, result.event_ids[0]);
}

#[test]
fn outbox_publisher_marks_failed_publish_for_retry() {
    let mut outbox = InMemoryOutbox::default();
    outbox.reserve_risk(make_reservation());
    let mut bus = FailingBus::new("bus down");

    let result = {
        let mut publisher = OutboxPublisher::new(&mut outbox, &mut bus);
        publisher.publish_pending(Some("risk.events"), 10)
    };
    let failed = outbox.list("failed");

    assert_eq!(result.claimed, 1);
    assert_eq!(result.published, 0);
    assert_eq!(result.failed, 1);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].last_error.as_deref(), Some("bus down"));

    let retryable = outbox.claim_batch(Some("risk.events"), 10);
    assert_eq!(retryable.len(), 1);
    assert_eq!(retryable[0].attempts, 2);
}

#[test]
fn outbox_publisher_accepts_explicit_outbox_protocol_event() {
    let mut outbox = InMemoryOutbox::default();
    let event = OutboxEvent::new(
        "outbox-1",
        "risk.events",
        "risk_reservation",
        "risk-1",
        "risk.reserved",
        serde_json::json!({"id": "risk-1"}),
        "risk:risk-1:reserved",
    );
    outbox.append(event.clone());
    outbox.append(event.with_id("outbox-duplicate"));
    let mut bus = InMemoryBus::default();

    let result = {
        let mut publisher =
            OutboxPublisher::new(&mut outbox, &mut bus).with_producer("postgres-outbox");
        publisher.publish_pending(Some("risk.events"), 10)
    };
    let delivery = bus.pull("risk.events").expect("delivery");

    assert_eq!(result.claimed, 1);
    assert_eq!(result.published, 1);
    assert_eq!(result.failed, 0);
    assert_eq!(delivery.envelope.producer, "postgres-outbox");
    assert_eq!(delivery.envelope.idempotency_key, "risk:risk-1:reserved");
    let published = outbox.list("published");
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].id, "outbox-1");
    assert!(published[0].published_at.is_some());
}
