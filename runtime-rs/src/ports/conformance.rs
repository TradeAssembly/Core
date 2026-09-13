// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    DurableQueuePort, EventAppend, EventStreamPort, EvidencePort, EvidenceRecord, IdempotencyKey,
    InboxRepository, LeaseRepository, OutboxRecord, OutboxRepository, QueueRequest, ScheduledWork,
    SchedulerPort, TelemetryPort,
};
use serde::Serialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortConformanceReport {
    pub suite_id: String,
    pub fixture_id: String,
    pub checks: Vec<String>,
}

pub struct OperationalPortSet<'a> {
    pub queue: &'a dyn DurableQueuePort,
    pub events: &'a dyn EventStreamPort,
    pub scheduler: &'a dyn SchedulerPort,
    pub outbox: &'a dyn OutboxRepository,
    pub inbox: &'a dyn InboxRepository,
    pub leases: &'a dyn LeaseRepository,
    pub evidence: &'a dyn EvidencePort,
    pub telemetry: &'a dyn TelemetryPort,
}

pub fn verify_operational_ports(
    ports: OperationalPortSet<'_>,
) -> Result<PortConformanceReport, String> {
    let fixture = fixture_id();
    let now = 1_800_000_000_000_i64;
    let queue_name = format!("conformance-{fixture}");
    let queue_key = key(&format!("{fixture}-queue"))?;
    let message_id = ports.queue.enqueue(QueueRequest {
        queue: queue_name.clone(),
        payload: json!({"fixtureId": fixture}),
        idempotency_key: queue_key.clone(),
        partition_key: Some(fixture.clone()),
        priority: 1,
        available_at_ms: now,
        retention_until_ms: now + 60_000,
        max_attempts: 3,
    })?;
    if message_id
        != ports.queue.enqueue(QueueRequest {
            queue: queue_name.clone(),
            payload: json!({"fixtureId": fixture}),
            idempotency_key: queue_key.clone(),
            partition_key: Some(fixture.clone()),
            priority: 1,
            available_at_ms: now,
            retention_until_ms: now + 60_000,
            max_attempts: 3,
        })?
    {
        return Err("queue idempotency conformance failed".to_string());
    }
    if ports
        .queue
        .enqueue(QueueRequest {
            queue: queue_name.clone(),
            payload: json!({"fixtureId": fixture, "changed": true}),
            idempotency_key: queue_key,
            partition_key: Some(fixture.clone()),
            priority: 1,
            available_at_ms: now,
            retention_until_ms: now + 60_000,
            max_attempts: 3,
        })
        .is_ok()
    {
        return Err("changed queue idempotency request was accepted".to_string());
    }
    let delayed_queue = format!("conformance-delayed-{fixture}");
    ports.queue.enqueue(QueueRequest {
        queue: delayed_queue.clone(),
        payload: json!({"fixtureId": fixture, "delayed": true}),
        idempotency_key: key(&format!("{fixture}-delayed-queue"))?,
        partition_key: Some(fixture.clone()),
        priority: 1,
        available_at_ms: now + 100,
        retention_until_ms: now + 60_000,
        max_attempts: 3,
    })?;
    if !ports
        .queue
        .claim(&delayed_queue, "conformance-delayed", now, 10, 1)?
        .is_empty()
    {
        return Err("queue not-before conformance failed".to_string());
    }
    let first = one(
        ports
            .queue
            .claim(&queue_name, "conformance-a", now, 10, 1)?,
        "queue first delivery",
    )?;
    let second = one(
        ports
            .queue
            .claim(&queue_name, "conformance-b", now + 11, 10, 1)?,
        "queue redelivery",
    )?;
    if second.fencing_token <= first.fencing_token {
        return Err("queue fencing conformance failed".to_string());
    }
    if ports
        .queue
        .acknowledge(&message_id, first.fencing_token)
        .is_ok()
    {
        return Err("stale queue acknowledgement was accepted".to_string());
    }
    if ports
        .queue
        .retry(&message_id, first.fencing_token, now + 20, "stale-delivery")
        .is_ok()
    {
        return Err("stale queue retry was accepted".to_string());
    }
    ports.queue.acknowledge(&message_id, second.fencing_token)?;

    let stream = format!("conformance-{fixture}");
    let event_key = key(&format!("{fixture}-event"))?;
    let event_request = EventAppend {
        stream: stream.clone(),
        event_type: "conformance.recorded".to_string(),
        aggregate_id: fixture.clone(),
        payload: json!({"fixtureId": fixture}),
        idempotency_key: event_key.clone(),
        occurred_at_ms: now,
        retention_until_ms: Some(now + 60_000),
    };
    if ports.events.consumer_checkpoint(&fixture, &stream)? != 0 {
        return Err("initial event checkpoint conformance failed".to_string());
    }
    let appended = ports.events.append(event_request.clone())?;
    if ports.events.append(event_request)? != appended {
        return Err("event idempotency conformance failed".to_string());
    }
    if ports
        .events
        .append(EventAppend {
            stream: stream.clone(),
            event_type: "conformance.changed".to_string(),
            aggregate_id: fixture.clone(),
            payload: json!({"fixtureId": fixture, "changed": true}),
            idempotency_key: event_key.clone(),
            occurred_at_ms: now,
            retention_until_ms: Some(now + 60_000),
        })
        .is_ok()
    {
        return Err("changed event idempotency request was accepted".to_string());
    }
    let peer_stream = format!("conformance-peer-{fixture}");
    let peer = ports.events.append(EventAppend {
        stream: peer_stream.clone(),
        event_type: "conformance.recorded".to_string(),
        aggregate_id: fixture.clone(),
        payload: json!({"fixtureId": fixture, "peer": true}),
        idempotency_key: event_key,
        occurred_at_ms: now,
        retention_until_ms: Some(now + 60_000),
    })?;
    if peer.event_id == appended.event_id || ports.events.replay(&peer_stream, 0, 10)? != vec![peer]
    {
        return Err("cross-stream event identity conformance failed".to_string());
    }
    ports
        .events
        .checkpoint(&fixture, &stream, appended.sequence)?;
    let replayed = ports.events.replay(&stream, 0, 10)?;
    let checkpoint = ports.events.consumer_checkpoint(&fixture, &stream)?;
    if replayed != vec![appended.clone()] || checkpoint != appended.sequence {
        return Err(format!(
            "event replay or checkpoint conformance failed: replayed={replayed:?} checkpoint={checkpoint} expected={appended:?}"
        ));
    }
    let next = ports.events.append(EventAppend {
        stream: stream.clone(),
        event_type: "conformance.followup".to_string(),
        aggregate_id: fixture.clone(),
        payload: json!({"fixtureId": fixture, "followup": true}),
        idempotency_key: key(&format!("{fixture}-event-followup"))?,
        occurred_at_ms: now + 1,
        retention_until_ms: Some(now + 60_000),
    })?;
    let next_sequence = next.sequence;
    if ports.events.replay(&stream, appended.sequence, 10)? != vec![next] {
        return Err("event replay resume conformance failed".to_string());
    }
    ports.events.checkpoint(&fixture, &stream, next_sequence)?;
    ports
        .events
        .checkpoint(&fixture, &stream, appended.sequence)?;
    if ports.events.consumer_checkpoint(&fixture, &stream)? != next_sequence {
        return Err("event checkpoint regression conformance failed".to_string());
    }

    let schedule_key = format!("{fixture}-schedule");
    let scheduled_work = ScheduledWork {
        schedule_key: schedule_key.clone(),
        queue: queue_name,
        payload: json!({"fixtureId": fixture}),
        due_at_ms: now,
        idempotency_key: key(&format!("{fixture}-schedule"))?,
        fencing_token: 0,
    };
    ports.scheduler.schedule(scheduled_work.clone())?;
    let first_schedule = ports
        .scheduler
        .claim_key(&schedule_key, "conformance", now, 100)?
        .ok_or_else(|| "scheduled work exact claim failed".to_string())?;
    ports.scheduler.schedule(ScheduledWork {
        payload: json!({"fixtureId": fixture, "unexpectedMutation": true}),
        ..scheduled_work
    })?;
    if ports
        .scheduler
        .claim_key(&schedule_key, "conformance-reset", now + 1, 100)?
        .is_some()
    {
        return Err("schedule identity mutation reset an active claim".to_string());
    }
    if ports
        .scheduler
        .complete(
            &schedule_key,
            "conformance",
            first_schedule.fencing_token,
            now + 100,
        )
        .is_ok()
    {
        return Err("expired schedule completion conformance failed".to_string());
    }
    let second_schedule = ports
        .scheduler
        .claim_key(&schedule_key, "conformance", now + 100, 100)?
        .ok_or_else(|| "scheduled work replacement lease failed".to_string())?;
    if second_schedule.fencing_token <= first_schedule.fencing_token {
        return Err("schedule fencing conformance failed".to_string());
    }
    if ports
        .scheduler
        .complete(
            &schedule_key,
            "conformance",
            first_schedule.fencing_token,
            now + 101,
        )
        .is_ok()
    {
        return Err("stale schedule fencing conformance failed".to_string());
    }
    ports.scheduler.complete(
        &schedule_key,
        "conformance",
        second_schedule.fencing_token,
        now + 101,
    )?;

    let canceled_schedule_key = format!("{fixture}-schedule-canceled");
    let canceled_work = ScheduledWork {
        schedule_key: canceled_schedule_key.clone(),
        queue: format!("{fixture}-canceled"),
        payload: json!({"fixtureId": fixture, "canceled": true}),
        due_at_ms: now,
        idempotency_key: key(&format!("{fixture}-schedule-canceled"))?,
        fencing_token: 0,
    };
    ports.scheduler.schedule(canceled_work.clone())?;
    if !ports.scheduler.cancel(&canceled_schedule_key)? {
        return Err("schedule cancellation conformance failed".to_string());
    }
    ports.scheduler.schedule(canceled_work)?;
    let restarted_schedule = ports
        .scheduler
        .claim_key(&canceled_schedule_key, "conformance", now, 100)?
        .ok_or_else(|| "canceled schedule restart conformance failed".to_string())?;
    ports.scheduler.complete(
        &canceled_schedule_key,
        "conformance",
        restarted_schedule.fencing_token,
        now,
    )?;

    let outbox_id = format!("{fixture}-outbox");
    ports.outbox.append(OutboxRecord {
        outbox_id: outbox_id.clone(),
        topic: "conformance.recorded".to_string(),
        payload: json!({"fixtureId": fixture}),
        idempotency_key: key(&format!("{fixture}-outbox"))?,
        created_at_ms: now,
        fencing_token: 0,
    })?;
    let first_outbox = one(
        ports.outbox.claim_pending("conformance", now, 1)?,
        "outbox record",
    )?;
    let second_outbox = one(
        ports.outbox.claim_pending("conformance", now + 30_001, 1)?,
        "outbox replacement lease",
    )?;
    if second_outbox.fencing_token <= first_outbox.fencing_token
        || ports
            .outbox
            .mark_published(&outbox_id, "conformance", first_outbox.fencing_token)
            .is_ok()
    {
        return Err("outbox fencing conformance failed".to_string());
    }
    ports
        .outbox
        .mark_published(&outbox_id, "conformance", second_outbox.fencing_token)?;

    if !ports
        .inbox
        .record_once("conformance", &fixture, Some("signature://fixture"), now)?
        || ports.inbox.record_once(
            "conformance",
            &fixture,
            Some("signature://fixture"),
            now + 1,
        )?
    {
        return Err("inbox deduplication conformance failed".to_string());
    }

    let resource = format!("conformance-{fixture}");
    let lease = ports
        .leases
        .acquire(&resource, "conformance-a", now, 10)?
        .ok_or_else(|| "initial lease conformance failed".to_string())?;
    if ports
        .leases
        .acquire(&resource, "conformance-b", now + 9, 10)?
        .is_some()
    {
        return Err("contended lease conformance failed".to_string());
    }
    let replacement = ports
        .leases
        .acquire(&resource, "conformance-b", now + 11, 10)?
        .ok_or_else(|| "expired lease conformance failed".to_string())?;
    if replacement.fencing_token <= lease.fencing_token || ports.leases.release(&lease).is_ok() {
        return Err("lease fencing conformance failed".to_string());
    }
    ports.leases.release(&replacement)?;

    ports.evidence.append(EvidenceRecord {
        evidence_id: format!("{fixture}-evidence"),
        evidence_type: "conformance.recorded".to_string(),
        aggregate_id: fixture.clone(),
        payload: json!({"fixtureId": fixture}),
        idempotency_key: key(&format!("{fixture}-evidence"))?,
        recorded_at_ms: now,
    })?;
    if ports.evidence.list(&fixture)?.len() != 1 {
        return Err("evidence durability conformance failed".to_string());
    }
    let _ = ports.telemetry.emit(
        "conformance.recorded",
        json!({"fixtureId": fixture}),
        &key(&format!("{fixture}-telemetry"))?,
    )?;

    Ok(PortConformanceReport {
        suite_id: "tradeassembly.port.conformance.v1".to_string(),
        fixture_id: fixture,
        checks: vec![
            "queue.idempotency-redelivery-fencing".to_string(),
            "events.append-replay-checkpoint".to_string(),
            "scheduler.claim-complete".to_string(),
            "outbox.claim-publish".to_string(),
            "inbox.deduplicate".to_string(),
            "lease.expiry-fencing".to_string(),
            "evidence.append-replay".to_string(),
            "telemetry.consent-aware".to_string(),
        ],
    })
}

fn fixture_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("port-{nanos:x}")
}

fn key(value: &str) -> Result<IdempotencyKey, String> {
    IdempotencyKey::new(value)
}

fn one<T>(mut values: Vec<T>, name: &str) -> Result<T, String> {
    if values.len() != 1 {
        return Err(format!("{name} conformance expected one record"));
    }
    Ok(values.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::local::operations::LocalSqliteOperations;
    use std::sync::Arc;

    #[test]
    fn local_sqlite_passes_shared_operational_port_conformance() {
        let path = std::env::temp_dir().join(format!("tradeassembly-{}.db", fixture_id()));
        let adapter = Arc::new(LocalSqliteOperations::new(
            path.to_string_lossy().to_string(),
            false,
        ));
        let report = verify_operational_ports(OperationalPortSet {
            queue: adapter.as_ref(),
            events: adapter.as_ref(),
            scheduler: adapter.as_ref(),
            outbox: adapter.as_ref(),
            inbox: adapter.as_ref(),
            leases: adapter.as_ref(),
            evidence: adapter.as_ref(),
            telemetry: adapter.as_ref(),
        })
        .expect("conformance");
        assert_eq!(report.checks.len(), 8);
        let _ = std::fs::remove_file(path);
    }
}
