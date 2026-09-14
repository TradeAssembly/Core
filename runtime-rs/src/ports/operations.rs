// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{IdempotencyKey, VersionedPort};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueRequest {
    pub queue: String,
    pub payload: Value,
    pub idempotency_key: IdempotencyKey,
    pub partition_key: Option<String>,
    pub priority: i64,
    pub available_at_ms: i64,
    pub retention_until_ms: i64,
    pub max_attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueDelivery {
    pub message_id: String,
    pub queue: String,
    pub payload: Value,
    pub idempotency_key: IdempotencyKey,
    pub partition_key: Option<String>,
    pub priority: i64,
    pub attempt: u32,
    pub lease_until_ms: i64,
    pub fencing_token: i64,
}

pub trait DurableQueuePort: VersionedPort {
    fn enqueue(&self, request: QueueRequest) -> Result<String, String>;
    fn claim(
        &self,
        queue: &str,
        owner: &str,
        now_ms: i64,
        visibility_timeout_ms: i64,
        limit: usize,
    ) -> Result<Vec<QueueDelivery>, String>;
    fn acknowledge(&self, message_id: &str, fencing_token: i64) -> Result<(), String>;
    /// Atomically claim only one delivery in the authorized partition. Never
    /// emulate this by claiming globally and filtering after mutation.
    fn claim_partition(
        &self,
        _queue: &str,
        _partition: &str,
        _owner: &str,
        _now_ms: i64,
        _visibility_timeout_ms: i64,
    ) -> Result<Vec<QueueDelivery>, String> {
        Err("queue_partition_claim_unsupported".to_string())
    }
    fn retry(
        &self,
        message_id: &str,
        fencing_token: i64,
        available_at_ms: i64,
        reason: &str,
    ) -> Result<(), String>;
    fn dead_letters(&self, queue: &str, limit: usize) -> Result<Vec<QueueDelivery>, String>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventAppend {
    pub stream: String,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: Value,
    pub idempotency_key: IdempotencyKey,
    pub occurred_at_ms: i64,
    pub retention_until_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub event_id: String,
    pub stream: String,
    pub sequence: i64,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: Value,
    pub idempotency_key: IdempotencyKey,
    pub occurred_at_ms: i64,
}

pub trait EventStreamPort: VersionedPort {
    fn append(&self, event: EventAppend) -> Result<EventRecord, String>;
    fn replay(
        &self,
        stream: &str,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<EventRecord>, String>;
    fn checkpoint(&self, consumer: &str, stream: &str, sequence: i64) -> Result<(), String>;
    fn consumer_checkpoint(&self, consumer: &str, stream: &str) -> Result<i64, String>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledWork {
    pub schedule_key: String,
    pub queue: String,
    pub payload: Value,
    pub due_at_ms: i64,
    pub idempotency_key: IdempotencyKey,
    #[serde(default)]
    pub fencing_token: i64,
}

pub trait SchedulerPort: VersionedPort {
    fn schedule(&self, work: ScheduledWork) -> Result<(), String>;
    fn cancel(&self, schedule_key: &str) -> Result<bool, String>;
    fn claim_key(
        &self,
        schedule_key: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<ScheduledWork>, String>;
    fn claim_due(
        &self,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
        limit: usize,
    ) -> Result<Vec<ScheduledWork>, String>;
    /// Complete a lease using the trusted runtime clock. Agent-controlled time
    /// must never be used for this expiry check.
    fn complete(
        &self,
        schedule_key: &str,
        owner: &str,
        fencing_token: i64,
        now_ms: i64,
    ) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxRecord {
    pub outbox_id: String,
    pub topic: String,
    pub payload: Value,
    pub idempotency_key: IdempotencyKey,
    pub created_at_ms: i64,
    #[serde(default)]
    pub fencing_token: i64,
}

pub trait OutboxRepository: VersionedPort {
    fn append(&self, record: OutboxRecord) -> Result<(), String>;
    fn claim_pending(
        &self,
        owner: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<OutboxRecord>, String>;
    fn mark_published(
        &self,
        outbox_id: &str,
        owner: &str,
        fencing_token: i64,
    ) -> Result<(), String>;
}

pub trait InboxRepository: VersionedPort {
    fn record_once(
        &self,
        source: &str,
        message_id: &str,
        signature_ref: Option<&str>,
        received_at_ms: i64,
    ) -> Result<bool, String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseClaim {
    pub resource: String,
    pub owner: String,
    pub fencing_token: i64,
    pub expires_at_ms: i64,
}

pub trait LeaseRepository: VersionedPort {
    /// Read current ownership without acquiring or extending it. Returned
    /// claims may be expired; admission must compare expiry to its trusted clock.
    /// Unsupported adapters fail closed rather than trusting an old attempt.
    fn current(&self, _resource: &str) -> Result<Option<LeaseClaim>, String> {
        Err("lease inspection is unsupported".to_string())
    }
    fn acquire(
        &self,
        resource: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<LeaseClaim>, String>;
    fn renew(&self, claim: &LeaseClaim, now_ms: i64, lease_ms: i64) -> Result<LeaseClaim, String>;
    fn release(&self, claim: &LeaseClaim) -> Result<(), String>;
}
