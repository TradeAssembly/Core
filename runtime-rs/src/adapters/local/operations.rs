// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    DurableQueuePort, EventAppend, EventRecord, EventStreamPort, EvidencePort, EvidenceRecord,
    FailureMode, InboxRepository, LeaseClaim, LeaseRepository, OutboxRecord, OutboxRepository,
    PortDescriptor, PortKind, QueueDelivery, QueueRequest, ScheduledWork, SchedulerPort,
    TelemetryPort, VersionedPort,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS runtime_queue (
  message_id TEXT PRIMARY KEY,
  queue_name TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  partition_key TEXT,
  priority INTEGER NOT NULL,
  available_at_ms INTEGER NOT NULL,
  retention_until_ms INTEGER NOT NULL,
  max_attempts INTEGER NOT NULL,
  attempt INTEGER NOT NULL DEFAULT 0,
  state TEXT NOT NULL DEFAULT 'pending',
  lease_owner TEXT,
  lease_until_ms INTEGER,
  fencing_token INTEGER NOT NULL DEFAULT 0,
  failure_reason TEXT,
  UNIQUE(queue_name, idempotency_key)
);
CREATE INDEX IF NOT EXISTS runtime_queue_claim_idx
  ON runtime_queue(queue_name, state, available_at_ms, priority DESC);

CREATE TABLE IF NOT EXISTS runtime_events (
  event_id TEXT PRIMARY KEY,
  stream_name TEXT NOT NULL,
  sequence_num INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  occurred_at_ms INTEGER NOT NULL,
  retention_until_ms INTEGER,
  UNIQUE(stream_name, sequence_num),
  UNIQUE(stream_name, idempotency_key)
);
CREATE TABLE IF NOT EXISTS runtime_event_checkpoints (
  consumer_name TEXT NOT NULL,
  stream_name TEXT NOT NULL,
  sequence_num INTEGER NOT NULL,
  PRIMARY KEY(consumer_name, stream_name)
);

CREATE TABLE IF NOT EXISTS runtime_schedules (
  schedule_key TEXT PRIMARY KEY,
  queue_name TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  due_at_ms INTEGER NOT NULL,
  idempotency_key TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL DEFAULT 'pending',
  lease_owner TEXT,
  lease_until_ms INTEGER,
  fencing_token INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS runtime_schedule_due_idx
  ON runtime_schedules(state, due_at_ms);

CREATE TABLE IF NOT EXISTS runtime_outbox (
  outbox_id TEXT PRIMARY KEY,
  topic TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  idempotency_key TEXT NOT NULL UNIQUE,
  created_at_ms INTEGER NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',
  lease_owner TEXT,
  lease_until_ms INTEGER,
  fencing_token INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS runtime_inbox (
  source TEXT NOT NULL,
  message_id TEXT NOT NULL,
  signature_ref TEXT,
  received_at_ms INTEGER NOT NULL,
  PRIMARY KEY(source, message_id)
);
CREATE TABLE IF NOT EXISTS runtime_leases (
  resource TEXT PRIMARY KEY,
  owner TEXT NOT NULL,
  fencing_token INTEGER NOT NULL,
  expires_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_evidence (
  evidence_id TEXT PRIMARY KEY,
  evidence_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  idempotency_key TEXT NOT NULL UNIQUE,
  recorded_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS runtime_evidence_aggregate_idx
  ON runtime_evidence(aggregate_id, recorded_at_ms, evidence_id);
CREATE TABLE IF NOT EXISTS runtime_telemetry (
  event_id TEXT PRIMARY KEY,
  event_type TEXT NOT NULL,
  fields_json TEXT NOT NULL,
  idempotency_key TEXT NOT NULL UNIQUE,
  created_at_ms INTEGER NOT NULL
);
"#;

#[derive(Clone, Debug)]
pub struct LocalSqliteOperations {
    db_path: String,
    telemetry_enabled: bool,
}

impl LocalSqliteOperations {
    pub fn new(db_path: impl Into<String>, telemetry_enabled: bool) -> Self {
        Self {
            db_path: db_path.into(),
            telemetry_enabled,
        }
    }

    pub fn verify_wal(&self) -> Result<bool, String> {
        let connection = self.connection()?;
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        Ok(mode.eq_ignore_ascii_case("wal") || self.db_path == ":memory:")
    }

    pub(crate) fn prepare_upgrade(&self) -> Result<(), String> {
        self.connection().map(drop)
    }

    fn connection(&self) -> Result<Connection, String> {
        if self.db_path != ":memory:" {
            if let Some(parent) = Path::new(&self.db_path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("create local database directory: {error}"))?;
            }
        }
        let connection = Connection::open(&self.db_path).map_err(sqlite_error)?;
        let was_empty = !super::schema::has_user_tables(&connection)?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(sqlite_error)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
            )
            .map_err(sqlite_error)?;
        connection.execute_batch(SCHEMA).map_err(sqlite_error)?;
        ensure_sqlite_column(
            &connection,
            "runtime_outbox",
            "fencing_token",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        super::schema::mark_current_if_new(&connection, was_empty)?;
        Ok(connection)
    }

    fn descriptor_for(&self, kind: PortKind, capabilities: &[&str]) -> PortDescriptor {
        let mut descriptor = PortDescriptor::new(kind, "local.sqlite-wal")
            .for_profiles(&["local"])
            .with_capabilities(capabilities);
        descriptor.failure_mode = FailureMode::LocalOnly;
        descriptor
    }
}

impl VersionedPort for LocalSqliteOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            self.descriptor_for(
                PortKind::DurableQueue,
                &["queue.lease", "queue.retry", "queue.dead_letter"],
            ),
            self.descriptor_for(
                PortKind::EventStream,
                &["event.append", "event.replay", "event.checkpoint"],
            ),
            self.descriptor_for(
                PortKind::Scheduler,
                &["schedule.create", "schedule.cancel", "schedule.claim"],
            ),
            self.descriptor_for(
                PortKind::Outbox,
                &["outbox.append", "outbox.claim", "outbox.publish"],
            ),
            self.descriptor_for(
                PortKind::Inbox,
                &["inbox.deduplicate", "inbox.signature_ref"],
            ),
            self.descriptor_for(
                PortKind::Lease,
                &[
                    "lease.acquire",
                    "lease.renew",
                    "lease.fence",
                    "lease.inspect",
                ],
            ),
            self.descriptor_for(PortKind::Evidence, &["evidence.append", "evidence.replay"]),
            self.descriptor_for(
                PortKind::Telemetry,
                &["telemetry.consent_gate", "telemetry.local_buffer"],
            ),
        ]
    }
}

impl DurableQueuePort for LocalSqliteOperations {
    fn enqueue(&self, request: QueueRequest) -> Result<String, String> {
        if request.queue.trim().is_empty()
            || request.retention_until_ms <= request.available_at_ms
            || request.max_attempts == 0
        {
            return Err("invalid durable queue request".to_string());
        }
        let message_id = stable_id(
            "message",
            &format!("{}:{}", request.queue, request.idempotency_key.as_str()),
        );
        let payload = serde_json::to_string(&request.payload).map_err(|error| error.to_string())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        transaction
            .execute(
                r#"INSERT INTO runtime_queue
                   (message_id, queue_name, payload_json, idempotency_key, partition_key,
                    priority, available_at_ms, retention_until_ms, max_attempts)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                   ON CONFLICT(queue_name, idempotency_key) DO NOTHING"#,
                params![
                    &message_id,
                    &request.queue,
                    &payload,
                    request.idempotency_key.as_str(),
                    &request.partition_key,
                    request.priority,
                    request.available_at_ms,
                    request.retention_until_ms,
                    request.max_attempts,
                ],
            )
            .map_err(sqlite_error)?;
        let stored = transaction
            .query_row(
                r#"SELECT message_id, payload_json, partition_key, priority,
                          available_at_ms, retention_until_ms, max_attempts
                   FROM runtime_queue
                   WHERE queue_name=?1 AND idempotency_key=?2"#,
                params![&request.queue, request.idempotency_key.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, u32>(6)?,
                    ))
                },
            )
            .map_err(sqlite_error)?;
        let stored_payload =
            serde_json::from_str::<Value>(&stored.1).map_err(|error| error.to_string())?;
        if stored_payload != request.payload
            || stored.2 != request.partition_key
            || stored.3 != request.priority
            || stored.4 != request.available_at_ms
            || stored.5 != request.retention_until_ms
            || stored.6 != request.max_attempts
        {
            return Err("queue idempotency key was reused with a different request".to_string());
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(stored.0)
    }

    fn claim(
        &self,
        queue: &str,
        owner: &str,
        now_ms: i64,
        visibility_timeout_ms: i64,
        limit: usize,
    ) -> Result<Vec<QueueDelivery>, String> {
        if queue.trim().is_empty()
            || owner.trim().is_empty()
            || visibility_timeout_ms <= 0
            || limit == 0
        {
            return Err("invalid durable queue claim".to_string());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        transaction
            .execute(
                r#"UPDATE runtime_queue SET state='dead', failure_reason='retention_expired'
                   WHERE queue_name=?1 AND state != 'acked' AND retention_until_ms <= ?2"#,
                params![queue, now_ms],
            )
            .map_err(sqlite_error)?;
        let ids = {
            let mut statement = transaction
                .prepare(
                    r#"SELECT message_id FROM runtime_queue
                       WHERE queue_name=?1
                         AND retention_until_ms > ?2
                         AND attempt < max_attempts
                         AND ((state='pending' AND available_at_ms <= ?2)
                              OR (state='leased' AND lease_until_ms <= ?2))
                       ORDER BY priority DESC, available_at_ms, message_id
                       LIMIT ?3"#,
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map(params![queue, now_ms, limit as i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(sqlite_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sqlite_error)?;
            rows
        };
        let mut deliveries = Vec::with_capacity(ids.len());
        for message_id in ids {
            transaction
                .execute(
                    r#"UPDATE runtime_queue SET
                       state='leased', lease_owner=?2, lease_until_ms=?3,
                       attempt=attempt+1, fencing_token=fencing_token+1
                       WHERE message_id=?1"#,
                    params![message_id, owner, now_ms + visibility_timeout_ms],
                )
                .map_err(sqlite_error)?;
            deliveries.push(read_queue_delivery(&transaction, &message_id)?);
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(deliveries)
    }

    fn claim_partition(
        &self,
        queue: &str,
        partition: &str,
        owner: &str,
        now_ms: i64,
        visibility_timeout_ms: i64,
    ) -> Result<Vec<QueueDelivery>, String> {
        if queue.trim().is_empty()
            || partition.trim().is_empty()
            || owner.trim().is_empty()
            || visibility_timeout_ms <= 0
        {
            return Err("invalid durable queue claim".to_string());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let id = transaction
            .query_row(
                r#"SELECT message_id FROM runtime_queue
               WHERE queue_name=?1 AND partition_key=?2 AND retention_until_ms > ?3
                 AND attempt < max_attempts
                 AND ((state='pending' AND available_at_ms <= ?3)
                      OR (state='leased' AND lease_until_ms <= ?3))
               ORDER BY priority DESC, available_at_ms, message_id LIMIT 1"#,
                params![queue, partition, now_ms],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?;
        let mut deliveries = Vec::new();
        if let Some(id) = id {
            transaction.execute(
                "UPDATE runtime_queue SET state='leased', lease_owner=?2, lease_until_ms=?3, attempt=attempt+1, fencing_token=fencing_token+1 WHERE message_id=?1",
                params![id, owner, now_ms + visibility_timeout_ms],
            ).map_err(sqlite_error)?;
            deliveries.push(read_queue_delivery(&transaction, &id)?);
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(deliveries)
    }

    fn acknowledge(&self, message_id: &str, fencing_token: i64) -> Result<(), String> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE runtime_queue SET state='acked', lease_owner=NULL, lease_until_ms=NULL WHERE message_id=?1 AND state='leased' AND fencing_token=?2",
                params![message_id, fencing_token],
            )
            .map_err(sqlite_error)?;
        require_fenced_change(changed, "queue acknowledgement")
    }

    fn retry(
        &self,
        message_id: &str,
        fencing_token: i64,
        available_at_ms: i64,
        reason: &str,
    ) -> Result<(), String> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                r#"UPDATE runtime_queue SET
                   state=CASE WHEN attempt >= max_attempts THEN 'dead' ELSE 'pending' END,
                   available_at_ms=?3, failure_reason=?4, lease_owner=NULL, lease_until_ms=NULL
                   WHERE message_id=?1 AND state='leased' AND fencing_token=?2"#,
                params![message_id, fencing_token, available_at_ms, reason],
            )
            .map_err(sqlite_error)?;
        require_fenced_change(changed, "queue retry")
    }

    fn dead_letters(&self, queue: &str, limit: usize) -> Result<Vec<QueueDelivery>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                r#"SELECT message_id, queue_name, payload_json, idempotency_key, partition_key,
                          priority, attempt, COALESCE(lease_until_ms, 0), fencing_token
                   FROM runtime_queue WHERE queue_name=?1 AND state='dead'
                   ORDER BY available_at_ms, message_id LIMIT ?2"#,
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(params![queue, limit as i64], queue_delivery_from_row)
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        Ok(rows)
    }
}

impl EventStreamPort for LocalSqliteOperations {
    fn append(&self, event: EventAppend) -> Result<EventRecord, String> {
        if event.stream.trim().is_empty() || event.event_type.trim().is_empty() {
            return Err("event stream and event type are required".to_string());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        if let Some(existing) =
            find_event_by_idempotency(&transaction, &event.stream, event.idempotency_key.as_str())?
        {
            let stored_retention = transaction
                .query_row(
                    "SELECT retention_until_ms FROM runtime_events WHERE event_id=?1",
                    params![&existing.event_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(sqlite_error)?;
            if existing.event_type != event.event_type
                || existing.aggregate_id != event.aggregate_id
                || existing.payload != event.payload
                || existing.occurred_at_ms != event.occurred_at_ms
                || stored_retention != event.retention_until_ms
            {
                return Err("event idempotency key was reused with a different request".to_string());
            }
            transaction.commit().map_err(sqlite_error)?;
            return Ok(existing);
        }
        let sequence = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence_num), 0) + 1 FROM runtime_events WHERE stream_name=?1",
                params![event.stream],
                |row| row.get::<_, i64>(0),
            )
            .map_err(sqlite_error)?;
        let event_id = stable_id(
            "event",
            &format!("{}:{}", event.stream, event.idempotency_key.as_str()),
        );
        transaction
            .execute(
                r#"INSERT INTO runtime_events
                   (event_id, stream_name, sequence_num, event_type, aggregate_id, payload_json,
                    idempotency_key, occurred_at_ms, retention_until_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
                params![
                    event_id,
                    event.stream,
                    sequence,
                    event.event_type,
                    event.aggregate_id,
                    serde_json::to_string(&event.payload).map_err(|error| error.to_string())?,
                    event.idempotency_key.as_str(),
                    event.occurred_at_ms,
                    event.retention_until_ms,
                ],
            )
            .map_err(sqlite_error)?;
        let record = read_event(&transaction, &event_id)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok(record)
    }

    fn replay(
        &self,
        stream: &str,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<EventRecord>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                r#"SELECT event_id, stream_name, sequence_num, event_type, aggregate_id,
                          payload_json, idempotency_key, occurred_at_ms
                   FROM runtime_events
                   WHERE stream_name=?1 AND sequence_num > ?2
                   ORDER BY sequence_num LIMIT ?3"#,
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(
                params![stream, after_sequence, limit as i64],
                event_from_row,
            )
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        Ok(rows)
    }

    fn checkpoint(&self, consumer: &str, stream: &str, sequence: i64) -> Result<(), String> {
        let connection = self.connection()?;
        connection
            .execute(
                r#"INSERT INTO runtime_event_checkpoints(consumer_name, stream_name, sequence_num)
                   VALUES (?1, ?2, ?3)
                   ON CONFLICT(consumer_name, stream_name) DO UPDATE SET
                     sequence_num=MAX(sequence_num, excluded.sequence_num)"#,
                params![consumer, stream, sequence],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    fn consumer_checkpoint(&self, consumer: &str, stream: &str) -> Result<i64, String> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT sequence_num FROM runtime_event_checkpoints WHERE consumer_name=?1 AND stream_name=?2",
                params![consumer, stream],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.unwrap_or(0))
            .map_err(sqlite_error)
    }
}

impl SchedulerPort for LocalSqliteOperations {
    fn schedule(&self, work: ScheduledWork) -> Result<(), String> {
        let connection = self.connection()?;
        connection
            .execute(
                r#"INSERT INTO runtime_schedules
                   (schedule_key, queue_name, payload_json, due_at_ms, idempotency_key)
                   VALUES (?1, ?2, ?3, ?4, ?5)
                   ON CONFLICT(schedule_key) DO UPDATE SET
                     queue_name=excluded.queue_name, payload_json=excluded.payload_json,
                     due_at_ms=excluded.due_at_ms, idempotency_key=excluded.idempotency_key,
                     state='pending', lease_owner=NULL, lease_until_ms=NULL
                   WHERE runtime_schedules.state='canceled'"#,
                params![
                    work.schedule_key,
                    work.queue,
                    serde_json::to_string(&work.payload).map_err(|error| error.to_string())?,
                    work.due_at_ms,
                    work.idempotency_key.as_str(),
                ],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    fn claim_key(
        &self,
        schedule_key: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<ScheduledWork>, String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let changed = transaction
            .execute(
                r#"UPDATE runtime_schedules
                   SET state='leased', lease_owner=?2, lease_until_ms=?3,
                       fencing_token=fencing_token+1
                   WHERE schedule_key=?1 AND due_at_ms <= ?4
                     AND (state='pending' OR (state='leased' AND lease_until_ms <= ?4))"#,
                params![schedule_key, owner, now_ms + lease_ms, now_ms],
            )
            .map_err(sqlite_error)?;
        let work = if changed == 1 {
            Some(read_schedule(&transaction, schedule_key)?)
        } else {
            None
        };
        transaction.commit().map_err(sqlite_error)?;
        Ok(work)
    }

    fn cancel(&self, schedule_key: &str) -> Result<bool, String> {
        let connection = self.connection()?;
        connection
            .execute(
                "UPDATE runtime_schedules SET state='canceled' WHERE schedule_key=?1 AND state != 'complete'",
                params![schedule_key],
            )
            .map(|changed| changed == 1)
            .map_err(sqlite_error)
    }

    fn claim_due(
        &self,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
        limit: usize,
    ) -> Result<Vec<ScheduledWork>, String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let keys = {
            let mut statement = transaction
                .prepare(
                    r#"SELECT schedule_key FROM runtime_schedules
                       WHERE due_at_ms <= ?1
                         AND (state='pending' OR (state='leased' AND lease_until_ms <= ?1))
                       ORDER BY due_at_ms, schedule_key LIMIT ?2"#,
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map(params![now_ms, limit as i64], |row| row.get::<_, String>(0))
                .map_err(sqlite_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sqlite_error)?;
            rows
        };
        let mut work = Vec::with_capacity(keys.len());
        for key in keys {
            transaction
                .execute(
                    "UPDATE runtime_schedules SET state='leased', lease_owner=?2, lease_until_ms=?3, fencing_token=fencing_token+1 WHERE schedule_key=?1",
                    params![key, owner, now_ms + lease_ms],
                )
                .map_err(sqlite_error)?;
            work.push(read_schedule(&transaction, &key)?);
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(work)
    }

    fn complete(
        &self,
        schedule_key: &str,
        owner: &str,
        fencing_token: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE runtime_schedules SET state='complete', lease_until_ms=NULL WHERE schedule_key=?1 AND state='leased' AND lease_owner=?2 AND fencing_token=?3 AND lease_until_ms > ?4",
                params![schedule_key, owner, fencing_token, now_ms],
            )
            .map_err(sqlite_error)?;
        require_fenced_change(changed, "schedule completion")
    }
}

impl OutboxRepository for LocalSqliteOperations {
    fn append(&self, record: OutboxRecord) -> Result<(), String> {
        let connection = self.connection()?;
        connection
            .execute(
                r#"INSERT INTO runtime_outbox(outbox_id, topic, payload_json, idempotency_key, created_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5)
                   ON CONFLICT(idempotency_key) DO NOTHING"#,
                params![
                    record.outbox_id,
                    record.topic,
                    serde_json::to_string(&record.payload).map_err(|error| error.to_string())?,
                    record.idempotency_key.as_str(),
                    record.created_at_ms,
                ],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    fn claim_pending(
        &self,
        owner: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<OutboxRecord>, String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT outbox_id FROM runtime_outbox WHERE state='pending' OR (state='leased' AND lease_until_ms <= ?1) ORDER BY created_at_ms, outbox_id LIMIT ?2",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map(params![now_ms, limit as i64], |row| row.get::<_, String>(0))
                .map_err(sqlite_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sqlite_error)?;
            rows
        };
        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            transaction
                .execute(
                    "UPDATE runtime_outbox SET state='leased', lease_owner=?2, lease_until_ms=?3, fencing_token=fencing_token+1 WHERE outbox_id=?1",
                    params![id, owner, now_ms + 30_000],
                )
                .map_err(sqlite_error)?;
            records.push(read_outbox(&transaction, &id)?);
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(records)
    }

    fn mark_published(
        &self,
        outbox_id: &str,
        owner: &str,
        fencing_token: i64,
    ) -> Result<(), String> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE runtime_outbox SET state='published', lease_until_ms=NULL WHERE outbox_id=?1 AND state='leased' AND lease_owner=?2 AND fencing_token=?3",
                params![outbox_id, owner, fencing_token],
            )
            .map_err(sqlite_error)?;
        require_fenced_change(changed, "outbox publication")
    }
}

impl InboxRepository for LocalSqliteOperations {
    fn record_once(
        &self,
        source: &str,
        message_id: &str,
        signature_ref: Option<&str>,
        received_at_ms: i64,
    ) -> Result<bool, String> {
        let connection = self.connection()?;
        connection
            .execute(
                "INSERT INTO runtime_inbox(source, message_id, signature_ref, received_at_ms) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(source, message_id) DO NOTHING",
                params![source, message_id, signature_ref, received_at_ms],
            )
            .map(|changed| changed == 1)
            .map_err(sqlite_error)
    }
}

impl LeaseRepository for LocalSqliteOperations {
    fn current(&self, resource: &str) -> Result<Option<LeaseClaim>, String> {
        let connection = self.connection()?;
        Ok(read_lease_optional(&connection, resource)?.filter(|claim| claim.expires_at_ms > 0))
    }

    fn acquire(
        &self,
        resource: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<LeaseClaim>, String> {
        let expires_at_ms = lease_expiry(now_ms, lease_ms)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let existing = read_lease_optional(&transaction, resource)?;
        if let Some(existing) = &existing {
            if existing.expires_at_ms > now_ms && existing.owner != owner {
                transaction.commit().map_err(sqlite_error)?;
                return Ok(None);
            }
        }
        let token = existing
            .map_or(Some(1), |claim| claim.fencing_token.checked_add(1))
            .ok_or_else(|| "lease fencing token exhausted".to_string())?;
        transaction
            .execute(
                r#"INSERT INTO runtime_leases(resource, owner, fencing_token, expires_at_ms)
                   VALUES (?1, ?2, ?3, ?4)
                   ON CONFLICT(resource) DO UPDATE SET owner=excluded.owner,
                     fencing_token=excluded.fencing_token, expires_at_ms=excluded.expires_at_ms"#,
                params![resource, owner, token, expires_at_ms],
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok(Some(LeaseClaim {
            resource: resource.to_string(),
            owner: owner.to_string(),
            fencing_token: token,
            expires_at_ms,
        }))
    }

    fn renew(&self, claim: &LeaseClaim, now_ms: i64, lease_ms: i64) -> Result<LeaseClaim, String> {
        let connection = self.connection()?;
        let expires = lease_expiry(now_ms, lease_ms)?;
        let changed = connection
            .execute(
                "UPDATE runtime_leases SET expires_at_ms=?4 WHERE resource=?1 AND owner=?2 AND fencing_token=?3 AND expires_at_ms > ?5",
                params![claim.resource, claim.owner, claim.fencing_token, expires, now_ms],
            )
            .map_err(sqlite_error)?;
        require_fenced_change(changed, "lease renewal")?;
        Ok(LeaseClaim {
            expires_at_ms: expires,
            ..claim.clone()
        })
    }

    fn release(&self, claim: &LeaseClaim) -> Result<(), String> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE runtime_leases SET expires_at_ms=0 WHERE resource=?1 AND owner=?2 AND fencing_token=?3 AND expires_at_ms>0",
                params![claim.resource, claim.owner, claim.fencing_token],
            )
            .map_err(sqlite_error)?;
        require_fenced_change(changed, "lease release")
    }
}

impl EvidencePort for LocalSqliteOperations {
    fn append(&self, record: EvidenceRecord) -> Result<(), String> {
        let connection = self.connection()?;
        connection
            .execute(
                r#"INSERT INTO runtime_evidence
                   (evidence_id, evidence_type, aggregate_id, payload_json, idempotency_key, recorded_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(idempotency_key) DO NOTHING"#,
                params![
                    record.evidence_id,
                    record.evidence_type,
                    record.aggregate_id,
                    serde_json::to_string(&record.payload).map_err(|error| error.to_string())?,
                    record.idempotency_key.as_str(),
                    record.recorded_at_ms,
                ],
            )
            .map_err(sqlite_error)?;
        Ok(())
    }

    fn list(&self, aggregate_id: &str) -> Result<Vec<EvidenceRecord>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT evidence_id, evidence_type, aggregate_id, payload_json, idempotency_key, recorded_at_ms FROM runtime_evidence WHERE aggregate_id=?1 ORDER BY recorded_at_ms, evidence_id",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(params![aggregate_id], evidence_from_row)
            .map_err(sqlite_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_error)?;
        Ok(rows)
    }
}

impl TelemetryPort for LocalSqliteOperations {
    fn emit(
        &self,
        event_type: &str,
        fields: Value,
        idempotency_key: &crate::ports::IdempotencyKey,
    ) -> Result<bool, String> {
        if !self.telemetry_enabled {
            return Ok(false);
        }
        let event_id = stable_id("telemetry", idempotency_key.as_str());
        let connection = self.connection()?;
        connection
            .execute(
                r#"INSERT INTO runtime_telemetry(event_id, event_type, fields_json, idempotency_key, created_at_ms)
                   VALUES (?1, ?2, ?3, ?4, unixepoch('subsec') * 1000)
                   ON CONFLICT(idempotency_key) DO NOTHING"#,
                params![
                    event_id,
                    event_type,
                    serde_json::to_string(&fields).map_err(|error| error.to_string())?,
                    idempotency_key.as_str(),
                ],
            )
            .map(|changed| changed == 1)
            .map_err(sqlite_error)
    }
}

fn read_queue_delivery(connection: &Connection, message_id: &str) -> Result<QueueDelivery, String> {
    connection
        .query_row(
            r#"SELECT message_id, queue_name, payload_json, idempotency_key, partition_key,
                      priority, attempt, COALESCE(lease_until_ms, 0), fencing_token
               FROM runtime_queue WHERE message_id=?1"#,
            params![message_id],
            queue_delivery_from_row,
        )
        .map_err(sqlite_error)
}

fn queue_delivery_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueDelivery> {
    Ok(QueueDelivery {
        message_id: row.get(0)?,
        queue: row.get(1)?,
        payload: parse_json_column(row.get::<_, String>(2)?)?,
        idempotency_key: parse_idempotency(row.get(3)?)?,
        partition_key: row.get(4)?,
        priority: row.get(5)?,
        attempt: row.get::<_, i64>(6)? as u32,
        lease_until_ms: row.get(7)?,
        fencing_token: row.get(8)?,
    })
}

fn find_event_by_idempotency(
    connection: &Connection,
    stream: &str,
    idempotency_key: &str,
) -> Result<Option<EventRecord>, String> {
    connection
        .query_row(
            r#"SELECT event_id, stream_name, sequence_num, event_type, aggregate_id,
                      payload_json, idempotency_key, occurred_at_ms
               FROM runtime_events WHERE stream_name=?1 AND idempotency_key=?2"#,
            params![stream, idempotency_key],
            event_from_row,
        )
        .optional()
        .map_err(sqlite_error)
}

fn read_event(connection: &Connection, event_id: &str) -> Result<EventRecord, String> {
    connection
        .query_row(
            r#"SELECT event_id, stream_name, sequence_num, event_type, aggregate_id,
                      payload_json, idempotency_key, occurred_at_ms
               FROM runtime_events WHERE event_id=?1"#,
            params![event_id],
            event_from_row,
        )
        .map_err(sqlite_error)
}

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    Ok(EventRecord {
        event_id: row.get(0)?,
        stream: row.get(1)?,
        sequence: row.get(2)?,
        event_type: row.get(3)?,
        aggregate_id: row.get(4)?,
        payload: parse_json_column(row.get::<_, String>(5)?)?,
        idempotency_key: parse_idempotency(row.get(6)?)?,
        occurred_at_ms: row.get(7)?,
    })
}

fn read_schedule(connection: &Connection, key: &str) -> Result<ScheduledWork, String> {
    connection
        .query_row(
            "SELECT schedule_key, queue_name, payload_json, due_at_ms, idempotency_key, fencing_token FROM runtime_schedules WHERE schedule_key=?1",
            params![key],
            |row| {
                Ok(ScheduledWork {
                    schedule_key: row.get(0)?,
                    queue: row.get(1)?,
                    payload: parse_json_column(row.get::<_, String>(2)?)?,
                    due_at_ms: row.get(3)?,
                    idempotency_key: parse_idempotency(row.get(4)?)?,
                    fencing_token: row.get(5)?,
                })
            },
        )
        .map_err(sqlite_error)
}

fn read_outbox(connection: &Connection, id: &str) -> Result<OutboxRecord, String> {
    connection
        .query_row(
            "SELECT outbox_id, topic, payload_json, idempotency_key, created_at_ms, fencing_token FROM runtime_outbox WHERE outbox_id=?1",
            params![id],
            |row| {
                Ok(OutboxRecord {
                    outbox_id: row.get(0)?,
                    topic: row.get(1)?,
                    payload: parse_json_column(row.get::<_, String>(2)?)?,
                    idempotency_key: parse_idempotency(row.get(3)?)?,
                    created_at_ms: row.get(4)?,
                    fencing_token: row.get(5)?,
                })
            },
        )
        .map_err(sqlite_error)
}

fn lease_expiry(now_ms: i64, lease_ms: i64) -> Result<i64, String> {
    if now_ms < 0 || lease_ms <= 0 {
        return Err("lease duration or clock is invalid".to_string());
    }
    now_ms
        .checked_add(lease_ms)
        .ok_or_else(|| "lease expiry overflow".to_string())
}

fn read_lease_optional(
    connection: &Connection,
    resource: &str,
) -> Result<Option<LeaseClaim>, String> {
    connection
        .query_row(
            "SELECT resource, owner, fencing_token, expires_at_ms FROM runtime_leases WHERE resource=?1",
            params![resource],
            |row| {
                Ok(LeaseClaim {
                    resource: row.get(0)?,
                    owner: row.get(1)?,
                    fencing_token: row.get(2)?,
                    expires_at_ms: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(sqlite_error)
}

fn evidence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRecord> {
    Ok(EvidenceRecord {
        evidence_id: row.get(0)?,
        evidence_type: row.get(1)?,
        aggregate_id: row.get(2)?,
        payload: parse_json_column(row.get::<_, String>(3)?)?,
        idempotency_key: parse_idempotency(row.get(4)?)?,
        recorded_at_ms: row.get(5)?,
    })
}

fn parse_json_column(body: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&body).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            body.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn parse_idempotency(value: String) -> rusqlite::Result<crate::ports::IdempotencyKey> {
    crate::ports::IdempotencyKey::new(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, error.into())
    })
}

fn ensure_sqlite_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sqlite_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)?;
    drop(statement);
    if !columns.iter().any(|existing| existing == column) {
        connection
            .execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
            ))
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn stable_id(prefix: &str, value: &str) -> String {
    let digest = Sha256::digest(format!("{prefix}:{value}").as_bytes());
    format!("{prefix}-{:x}", digest)[..prefix.len() + 1 + 24].to_string()
}

fn require_fenced_change(changed: usize, operation: &str) -> Result<(), String> {
    if changed == 1 {
        Ok(())
    } else {
        Err(format!("stale or missing ownership for {operation}"))
    }
}

fn sqlite_error(error: rusqlite::Error) -> String {
    format!(
        "sqlite operational store failed: {}",
        error
            .sqlite_error_code()
            .map_or_else(|| "unknown".to_string(), |code| format!("{code:?}"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::IdempotencyKey;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(label: &str) -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("tradeassembly-operations-{label}-{stamp}.db"))
            .to_string_lossy()
            .to_string()
    }

    fn key(value: &str) -> IdempotencyKey {
        IdempotencyKey::new(value).expect("idempotency key")
    }

    #[test]
    fn lease_inspection_is_read_only_and_release_preserves_fencing_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("leases.db");
        let first = LocalSqliteOperations::new(path.to_string_lossy(), false);
        assert_eq!(first.current("resource").unwrap(), None);
        let claim = first
            .acquire("resource", "worker", 100, 50)
            .unwrap()
            .unwrap();
        assert_eq!(first.current("resource").unwrap(), Some(claim.clone()));
        assert_eq!(first.current("resource").unwrap(), Some(claim.clone()));
        first.release(&claim).unwrap();
        assert_eq!(first.current("resource").unwrap(), None);
        drop(first);
        let reopened = LocalSqliteOperations::new(path.to_string_lossy(), false);
        let replacement = reopened
            .acquire("resource", "worker", 110, 50)
            .unwrap()
            .unwrap();
        assert!(replacement.fencing_token > claim.fencing_token);
        assert!(reopened.renew(&claim, 111, 50).is_err());
        assert!(reopened.release(&claim).is_err());
        assert_eq!(
            reopened.current("resource").unwrap(),
            Some(replacement.clone())
        );
        let takeover = reopened
            .acquire("resource", "different-worker", 161, 50)
            .unwrap()
            .unwrap();
        assert!(takeover.fencing_token > replacement.fencing_token);
        assert_eq!(reopened.current("resource").unwrap(), Some(takeover));
        for (now, duration) in [(100, 0), (100, -1), (-1, 50), (i64::MAX, 1)] {
            assert!(reopened
                .acquire("invalid", "worker", now, duration)
                .is_err());
        }
        assert_eq!(reopened.current("invalid").unwrap(), None);
    }

    #[test]
    fn queue_recovers_after_reopen_and_rejects_stale_fencing_tokens() {
        let path = test_path("queue");
        let message_id = {
            let first = LocalSqliteOperations::new(&path, false);
            first
                .enqueue(QueueRequest {
                    queue: "strategy-cycle".to_string(),
                    payload: serde_json::json!({"activationId": "activation-1"}),
                    idempotency_key: key("cycle-1"),
                    partition_key: Some("activation-1".to_string()),
                    priority: 10,
                    available_at_ms: 100,
                    retention_until_ms: 10_000,
                    max_attempts: 3,
                })
                .expect("enqueue")
        };

        let reopened = LocalSqliteOperations::new(&path, false);
        let first_claim = reopened
            .claim("strategy-cycle", "worker-a", 100, 50, 1)
            .expect("first claim")
            .pop()
            .expect("delivery");
        assert_eq!(first_claim.message_id, message_id);
        assert!(reopened.acknowledge(&message_id, 0).is_err());

        let second_claim = reopened
            .claim("strategy-cycle", "worker-b", 151, 50, 1)
            .expect("expired claim")
            .pop()
            .expect("redelivery");
        assert!(second_claim.fencing_token > first_claim.fencing_token);
        assert!(reopened
            .acknowledge(&message_id, first_claim.fencing_token)
            .is_err());
        reopened
            .acknowledge(&message_id, second_claim.fencing_token)
            .expect("current owner acknowledges");
        assert!(reopened
            .claim("strategy-cycle", "worker-c", 300, 50, 1)
            .expect("empty queue")
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scheduler_completion_rejects_expiry_across_independent_connections() {
        let path = test_path("scheduler-expiry");
        let schedule_key = "scheduler-expiry-key";
        let first = LocalSqliteOperations::new(&path, false);
        first
            .schedule(ScheduledWork {
                schedule_key: schedule_key.to_string(),
                queue: "scheduler".to_string(),
                payload: serde_json::json!({"fixtureId": "scheduler-expiry"}),
                due_at_ms: 100,
                idempotency_key: key("scheduler-expiry-key"),
                fencing_token: 0,
            })
            .expect("schedule");
        let first_claim = first
            .claim_key(schedule_key, "worker-a", 100, 10)
            .expect("first claim")
            .expect("first lease");

        let second = LocalSqliteOperations::new(&path, false);
        assert!(second
            .complete(schedule_key, "worker-a", first_claim.fencing_token, 110)
            .is_err());
        let replacement = second
            .claim_key(schedule_key, "worker-b", 110, 10)
            .expect("replacement claim")
            .expect("replacement lease");
        assert!(second
            .complete(schedule_key, "worker-a", first_claim.fencing_token, 111)
            .is_err());
        second
            .complete(schedule_key, "worker-b", replacement.fencing_token, 111)
            .expect("unexpired completion");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn queue_deduplicates_and_dead_letters_after_bounded_attempts() {
        let path = test_path("deadletter");
        let adapter = LocalSqliteOperations::new(&path, false);
        let request = QueueRequest {
            queue: "orders".to_string(),
            payload: serde_json::json!({"orderId": "order-1"}),
            idempotency_key: key("order-1-submit"),
            partition_key: Some("account-1".to_string()),
            priority: 0,
            available_at_ms: 0,
            retention_until_ms: 10_000,
            max_attempts: 2,
        };
        assert_eq!(
            adapter.enqueue(request.clone()).expect("first enqueue"),
            adapter.enqueue(request).expect("deduplicated enqueue")
        );
        let first = adapter
            .claim("orders", "worker", 0, 10, 1)
            .expect("claim")
            .remove(0);
        adapter
            .retry(&first.message_id, first.fencing_token, 11, "ambiguous")
            .expect("retry");
        let second = adapter
            .claim("orders", "worker", 11, 10, 1)
            .expect("claim")
            .remove(0);
        adapter
            .retry(
                &second.message_id,
                second.fencing_token,
                22,
                "still ambiguous",
            )
            .expect("dead letter");
        assert_eq!(
            adapter
                .dead_letters("orders", 10)
                .expect("dead letters")
                .len(),
            1
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn event_replay_checkpoint_inbox_and_lease_survive_restart() {
        let path = test_path("events");
        let first = LocalSqliteOperations::new(&path, false);
        let event = EventAppend {
            stream: "strategy-events".to_string(),
            event_type: "strategy.activated".to_string(),
            aggregate_id: "activation-1".to_string(),
            payload: serde_json::json!({"mode": "paper"}),
            idempotency_key: key("activation-1-start"),
            occurred_at_ms: 100,
            retention_until_ms: Some(10_000),
        };
        let appended = EventStreamPort::append(&first, event.clone()).expect("append");
        assert_eq!(
            appended,
            EventStreamPort::append(&first, event).expect("idempotent append")
        );
        first
            .checkpoint("monitor", "strategy-events", appended.sequence)
            .expect("checkpoint");
        assert!(first
            .record_once("broker", "update-1", Some("signature://update-1"), 101)
            .expect("first inbox"));
        assert!(!first
            .record_once("broker", "update-1", Some("signature://update-1"), 102)
            .expect("duplicate inbox"));
        let claim = first
            .acquire("activation-1", "worker-a", 100, 50)
            .expect("lease")
            .expect("claim");
        drop(first);

        let reopened = LocalSqliteOperations::new(&path, false);
        assert_eq!(
            reopened.replay("strategy-events", 0, 10).expect("replay"),
            vec![appended]
        );
        assert_eq!(
            reopened
                .consumer_checkpoint("monitor", "strategy-events")
                .expect("checkpoint"),
            1
        );
        assert!(reopened
            .acquire("activation-1", "worker-b", 149, 50)
            .expect("contended lease")
            .is_none());
        let replacement = reopened
            .acquire("activation-1", "worker-b", 151, 50)
            .expect("expired lease")
            .expect("replacement");
        assert!(replacement.fencing_token > claim.fencing_token);
        assert!(reopened.release(&claim).is_err());
        reopened.release(&replacement).expect("release");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn telemetry_is_opt_in_and_evidence_is_always_durable() {
        let path = test_path("evidence");
        let disabled = LocalSqliteOperations::new(&path, false);
        assert!(!disabled
            .emit(
                "runtime.started",
                serde_json::json!({}),
                &key("telemetry-1")
            )
            .expect("disabled telemetry"));
        EvidencePort::append(
            &disabled,
            EvidenceRecord {
                evidence_id: "evidence-1".to_string(),
                evidence_type: "legal.acknowledged".to_string(),
                aggregate_id: "user-1".to_string(),
                payload: serde_json::json!({"documentHash": "sha256:abc"}),
                idempotency_key: key("legal-1"),
                recorded_at_ms: 100,
            },
        )
        .expect("evidence");
        let enabled = LocalSqliteOperations::new(&path, true);
        assert!(enabled
            .emit(
                "runtime.started",
                serde_json::json!({}),
                &key("telemetry-1")
            )
            .expect("enabled telemetry"));
        assert!(!enabled
            .emit(
                "runtime.started",
                serde_json::json!({}),
                &key("telemetry-1")
            )
            .expect("deduplicated telemetry"));
        assert_eq!(enabled.list("user-1").expect("evidence list").len(), 1);
        let _ = std::fs::remove_file(path);
    }
}
