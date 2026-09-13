// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Durable, credential-free local evidence archive outbox.
//!
//! This module owns only the local queue and the provider-neutral transport
//! contract. It does not contain an HTTP client, access token, cloud endpoint,
//! or broker credential. A product/cloud adapter can implement
//! [`ArchiveTransportPort`] without making Core depend on that adapter.

use crate::ports::{
    AuthorityContext, ComparePutOutcome, FailureMode, IdempotencyKey, ImmutablePutOutcome,
    PortDescriptor, PortKind, ServiceRuntime, SideEffectContext, StorageExpectation, StorageWrite,
    VersionedPort,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;

/// Matches the product-owned Cloud gateway request schema without importing a
/// Cloud crate into Core.
pub const ARCHIVE_INGEST_SCHEMA: &str = "tradeassembly.studio.tool-gateway/v1";
/// Payload schema nested inside the gateway request. It is the local,
/// redacted event/receipt envelope, not a separate HTTP contract.
pub const ARCHIVE_EVIDENCE_SCHEMA: &str = "tradeassembly.archive-evidence.v1";
pub const CLOUD_ARCHIVE_RECORD_SCHEMA: &str = "tradeassembly.studio.cloud-archive-record/v1";
pub const ARCHIVE_INTEGRITY_ALGORITHM: &str = "sha256-jcs";
pub const MAX_SYNC_BATCH: usize = 50;
/// The upper bound on records examined by one sync call. It is deliberately
/// independent from the number of archive requests dispatched so terminal
/// history cannot turn a bounded retry into an unbounded scan.
pub const MAX_SYNC_SCAN: usize = MAX_SYNC_BATCH;
pub const MAX_QUERY_BATCH: usize = 100;

const RECORDS_NS: &str = "archive_evidence_records_v1";
const STATES_NS: &str = "archive_evidence_states_v1";
const IDEMPOTENCY_NS: &str = "archive_evidence_idempotency_v1";
const SYNC_STATUS_NS: &str = "archive_evidence_sync_status_v1";
const SYNC_STATUS_KEY: &str = "latest";
const SYNC_CURSOR_NS: &str = "archive_evidence_sync_cursors_v1";
const LEASE_MS: i64 = 30_000;
// Keep Core's accepted envelope strictly inside the product-owned Cloud
// gateway boundary. Otherwise an accepted local receipt could be retained
// forever yet be structurally impossible for a conforming gateway to ingest.
const MAX_EVIDENCE_BYTES: usize = 64 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveEvidenceKind {
    Event,
    Receipt,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveEvidenceInput {
    pub tenant_id: String,
    pub workspace_id: String,
    pub tool_name: String,
    pub request_id: String,
    pub evidence_kind: ArchiveEvidenceKind,
    pub source: String,
    pub aggregate_id: String,
    pub occurred_at_ms: i64,
    pub payload: Value,
    pub context: SideEffectContext,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveEvidenceEnvelope {
    /// Exact gateway request fields. A product-owned adapter can serialize this
    /// request without translating Core types.
    pub schema: String,
    pub tenant_id: String,
    pub workspace_id: String,
    pub tool_name: String,
    pub request_id: String,
    pub idempotency_key: IdempotencyKey,
    pub payload: Value,
    pub payload_sha256: String,
    pub occurred_at_ms: i64,
    /// Exact canonical gateway idempotency digest. The Cloud receipt returns
    /// this same value, allowing local parity verification.
    pub request_digest_sha256: String,
}

impl ArchiveEvidenceEnvelope {
    pub fn verify_integrity(&self) -> Result<(), ArchiveOutboxError> {
        if self.schema != ARCHIVE_INGEST_SCHEMA
            || self.occurred_at_ms <= 0
            || !self.payload.is_object()
            || contains_sensitive_key(&self.payload)
            || !valid_sha256(&self.payload_sha256)
            || !valid_sha256(&self.request_digest_sha256)
        {
            return Err(ArchiveOutboxError::InvalidEnvelope);
        }
        for identifier in [
            &self.tenant_id,
            &self.workspace_id,
            &self.tool_name,
            &self.request_id,
            self.idempotency_key.as_str(),
        ] {
            validate_identifier(identifier)?;
        }
        let canonical_payload = canonical_bytes(&self.payload)?;
        if canonical_payload.len() > MAX_EVIDENCE_BYTES {
            return Err(ArchiveOutboxError::PayloadTooLarge);
        }
        if self.payload_sha256 != hex_digest(&canonical_payload)
            || self.request_digest_sha256 != request_digest(self)?
        {
            return Err(ArchiveOutboxError::IntegrityMismatch);
        }
        Ok(())
    }

    pub fn ingest_request(&self) -> ArchiveIngestRequest {
        ArchiveIngestRequest {
            schema: self.schema.clone(),
            tenant_id: self.tenant_id.clone(),
            workspace_id: self.workspace_id.clone(),
            tool_name: self.tool_name.clone(),
            request_id: self.request_id.clone(),
            idempotency_key: self.idempotency_key.clone(),
            payload: self.payload.clone(),
            payload_sha256: self.payload_sha256.clone(),
            occurred_at_ms: self.occurred_at_ms,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveIngestRequest {
    pub schema: String,
    pub tenant_id: String,
    pub workspace_id: String,
    pub tool_name: String,
    pub request_id: String,
    pub idempotency_key: IdempotencyKey,
    pub payload: Value,
    pub payload_sha256: String,
    pub occurred_at_ms: i64,
}

impl ArchiveIngestRequest {
    pub fn request_digest_sha256(&self) -> Result<String, ArchiveOutboxError> {
        request_digest_from_parts(
            &self.schema,
            &self.tenant_id,
            &self.workspace_id,
            &self.tool_name,
            &self.request_id,
            &self.payload_sha256,
            self.occurred_at_ms,
        )
    }

    pub fn verify(&self) -> Result<(), ArchiveOutboxError> {
        let envelope = ArchiveEvidenceEnvelope {
            schema: self.schema.clone(),
            tenant_id: self.tenant_id.clone(),
            workspace_id: self.workspace_id.clone(),
            tool_name: self.tool_name.clone(),
            request_id: self.request_id.clone(),
            idempotency_key: self.idempotency_key.clone(),
            payload: self.payload.clone(),
            payload_sha256: self.payload_sha256.clone(),
            occurred_at_ms: self.occurred_at_ms,
            request_digest_sha256: self.request_digest_sha256()?,
        };
        if self.schema != ARCHIVE_INGEST_SCHEMA {
            return Err(ArchiveOutboxError::InvalidEnvelope);
        }
        envelope.verify_integrity()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveIngestReceipt {
    pub schema: String,
    pub request_id: String,
    pub idempotency_key: IdempotencyKey,
    pub request_digest_sha256: String,
    pub entitlement_snapshot_ref: String,
    pub archive_ref: String,
    pub record_digest_sha256: String,
    /// Product gateways that expose an ordered archive cursor can populate
    /// this. Current source-only adapters may return `None`.
    pub archive_cursor: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveTransportFailure {
    pub code: String,
    pub retryable: bool,
}

impl ArchiveTransportFailure {
    pub fn new(code: impl AsRef<str>, retryable: bool) -> Self {
        Self {
            code: safe_error_code(code.as_ref()),
            retryable,
        }
    }
}

/// Provider-neutral evidence ingest boundary.
///
/// Implementations must treat `request_id` and `idempotency_key` as durable
/// idempotency keys and return the same receipt for an exact replay.
pub trait ArchiveTransportPort: VersionedPort {
    fn ingest(
        &self,
        request: &ArchiveIngestRequest,
    ) -> Result<ArchiveIngestReceipt, ArchiveTransportFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveOutboxError {
    RequestIdRequired,
    SourceRequired,
    AggregateIdRequired,
    InvalidIdentifier,
    InvalidOccurredAt,
    InvalidPayload,
    SensitivePayload,
    PayloadTooLarge,
    InvalidEnvelope,
    IntegrityMismatch,
    IdempotencyConflict,
    RequestIdConflict,
    StorageUnavailable,
    StorageInconsistent,
    InvalidSyncOwner,
    InvalidLimit,
}

impl ArchiveOutboxError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RequestIdRequired => "archive_request_id_required",
            Self::SourceRequired => "archive_evidence_source_required",
            Self::AggregateIdRequired => "archive_evidence_aggregate_id_required",
            Self::InvalidIdentifier => "archive_evidence_identifier_invalid",
            Self::InvalidOccurredAt => "archive_evidence_occurred_at_invalid",
            Self::InvalidPayload => "archive_evidence_payload_invalid",
            Self::SensitivePayload => "archive_evidence_sensitive_payload",
            Self::PayloadTooLarge => "archive_evidence_payload_too_large",
            Self::InvalidEnvelope => "archive_evidence_envelope_invalid",
            Self::IntegrityMismatch => "archive_evidence_integrity_mismatch",
            Self::IdempotencyConflict => "archive_evidence_idempotency_conflict",
            Self::RequestIdConflict => "archive_request_id_conflict",
            Self::StorageUnavailable => "archive_evidence_storage_unavailable",
            Self::StorageInconsistent => "archive_evidence_storage_inconsistent",
            Self::InvalidSyncOwner => "archive_evidence_sync_owner_invalid",
            Self::InvalidLimit => "archive_evidence_limit_invalid",
        }
    }
}

impl fmt::Display for ArchiveOutboxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ArchiveOutboxError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveEvidenceSyncStatus {
    pub state: String,
    pub attempts: u32,
    pub next_attempt_at_ms: Option<i64>,
    pub last_attempt_at_ms: Option<i64>,
    pub last_error_code: Option<String>,
    pub published_at_ms: Option<i64>,
    pub entitlement_snapshot_ref: Option<String>,
    pub archive_ref: Option<String>,
    pub record_digest_sha256: Option<String>,
    pub acknowledged_cursor: Option<i64>,
    pub lease_owner: Option<String>,
    pub lease_until_ms: Option<i64>,
    pub fencing_token: i64,
}

impl ArchiveEvidenceSyncStatus {
    fn pending(queued_at_ms: i64) -> Self {
        Self {
            state: "pending".to_string(),
            attempts: 0,
            next_attempt_at_ms: Some(queued_at_ms),
            last_attempt_at_ms: None,
            last_error_code: None,
            published_at_ms: None,
            entitlement_snapshot_ref: None,
            archive_ref: None,
            record_digest_sha256: None,
            acknowledged_cursor: None,
            lease_owner: None,
            lease_until_ms: None,
            fencing_token: 0,
        }
    }

    fn is_published(&self) -> bool {
        self.state == "published"
    }

    fn reconciliation_required(&self) -> bool {
        self.state == "reconciliation_required"
    }

    fn is_leased_until(&self, now_ms: i64) -> bool {
        self.state == "leased" && self.lease_until_ms.is_some_and(|until| until > now_ms)
    }

    fn due(&self, now_ms: i64) -> bool {
        !self.is_published()
            && !self.reconciliation_required()
            && !self.is_leased_until(now_ms)
            && self.next_attempt_at_ms.is_none_or(|at| at <= now_ms)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveEnqueueReceipt {
    pub evidence: ArchiveEvidenceEnvelope,
    pub sync: ArchiveEvidenceSyncStatus,
    pub duplicate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveEvidenceRecord {
    pub evidence: ArchiveEvidenceEnvelope,
    pub sync: ArchiveEvidenceSyncStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEvidenceQuery {
    pub tenant_id: Option<String>,
    pub workspace_id: Option<String>,
    pub after_request_id: Option<String>,
    pub limit: usize,
}

impl Default for ArchiveEvidenceQuery {
    fn default() -> Self {
        Self {
            tenant_id: None,
            workspace_id: None,
            after_request_id: None,
            limit: MAX_QUERY_BATCH,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveEvidencePage {
    pub records: Vec<ArchiveEvidenceRecord>,
    pub next_after_request_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveOutboxStatus {
    pub schema: String,
    pub total: usize,
    pub pending: usize,
    pub leased: usize,
    pub published: usize,
    pub unavailable: usize,
    pub reconciliation_required: usize,
    pub oldest_pending_at_ms: Option<i64>,
    pub last_sync_at_ms: Option<i64>,
    pub acknowledged_cursor: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveSyncItem {
    pub request_id: String,
    pub outcome: String,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveSyncReport {
    pub schema: String,
    /// Number of local records examined while selecting this bounded batch.
    pub scanned: usize,
    pub attempted: usize,
    pub published: usize,
    pub unavailable: usize,
    pub reconciliation_required: usize,
    pub skipped: usize,
    pub acknowledged_cursor: Option<i64>,
    /// Per-sync-owner continuation. `None` means the next call wraps to the
    /// first record in the namespace.
    pub next_scan_after_request_id: Option<String>,
    pub items: Vec<ArchiveSyncItem>,
}

#[derive(Clone)]
pub struct ArchiveEvidenceOutbox {
    runtime: Arc<ServiceRuntime>,
}

impl ArchiveEvidenceOutbox {
    pub fn new(runtime: Arc<ServiceRuntime>) -> Self {
        Self { runtime }
    }

    /// Persist redacted evidence locally. This function never calls an archive
    /// transport, so a cloud outage cannot stop local evidence persistence.
    pub fn enqueue(
        &self,
        input: ArchiveEvidenceInput,
    ) -> Result<ArchiveEnqueueReceipt, ArchiveOutboxError> {
        let envelope = envelope_from_input(&input)?;
        let mapping = idempotency_mapping(&envelope);
        let status = ArchiveEvidenceSyncStatus::pending(envelope.occurred_at_ms);
        let writes = vec![
            StorageWrite::new(
                RECORDS_NS,
                &envelope.request_id,
                serde_json::to_value(&envelope).map_err(|_| ArchiveOutboxError::InvalidEnvelope)?,
                input.context.clone(),
            ),
            StorageWrite::new(
                IDEMPOTENCY_NS,
                envelope.idempotency_key.as_str(),
                mapping,
                input.context.clone(),
            ),
            StorageWrite::new(
                STATES_NS,
                &envelope.request_id,
                serde_json::to_value(&status)
                    .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
                input.context.clone(),
            ),
        ];
        let expectations = vec![
            StorageExpectation::new(RECORDS_NS, &envelope.request_id, None),
            StorageExpectation::new(IDEMPOTENCY_NS, envelope.idempotency_key.as_str(), None),
            StorageExpectation::new(STATES_NS, &envelope.request_id, None),
        ];
        match self.runtime.storage.put_json_batch(&writes, &expectations) {
            Ok(ComparePutOutcome::Updated) => Ok(ArchiveEnqueueReceipt {
                evidence: envelope,
                sync: status,
                duplicate: false,
            }),
            Ok(ComparePutOutcome::Conflict) => self.replay_enqueue(&envelope, &input.context),
            Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
        }
    }

    /// Retry a previously reconciliation-required evidence item. The original immutable
    /// envelope and idempotency key are retained.
    pub fn requeue(
        &self,
        request_id: &str,
        now_ms: i64,
        context: &SideEffectContext,
    ) -> Result<ArchiveEvidenceSyncStatus, ArchiveOutboxError> {
        validate_identifier(request_id)?;
        if now_ms <= 0 {
            return Err(ArchiveOutboxError::InvalidOccurredAt);
        }
        let envelope = self.load_envelope(request_id)?;
        let current = self.load_or_repair_state(&envelope)?;
        if current.is_published() || current.is_leased_until(now_ms) {
            return Ok(current);
        }
        let mut replacement = current.clone();
        replacement.state = "pending".to_string();
        replacement.next_attempt_at_ms = Some(now_ms);
        replacement.last_error_code = None;
        replacement.lease_owner = None;
        replacement.lease_until_ms = None;
        match self.runtime.storage.compare_and_put_json(
            STATES_NS,
            request_id,
            serde_json::to_value(&current).map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            serde_json::to_value(&replacement)
                .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            context,
        ) {
            Ok(ComparePutOutcome::Updated) => Ok(replacement),
            Ok(ComparePutOutcome::Conflict) => self.load_or_repair_state(&envelope),
            Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
        }
    }

    /// Sync at most `limit` due evidence records. `owner` must remain stable
    /// for one logical worker so its durable bounded-scan continuation remains
    /// fair across calls. Transport errors are recorded as durable retry/block
    /// state and returned in the report, not as raw provider errors.
    pub fn sync(
        &self,
        transport: &dyn ArchiveTransportPort,
        owner: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<ArchiveSyncReport, ArchiveOutboxError> {
        if !is_safe_identifier(owner) {
            return Err(ArchiveOutboxError::InvalidSyncOwner);
        }
        if now_ms <= 0 {
            return Err(ArchiveOutboxError::InvalidOccurredAt);
        }
        if limit == 0 {
            return Err(ArchiveOutboxError::InvalidLimit);
        }
        let limit = limit.min(MAX_SYNC_BATCH);
        let scan_after_request_id = self.read_scan_cursor(owner)?;
        // Fetch one extra record to distinguish an exact final page from a
        // continuation. The local storage adapter performs this as a bounded
        // `ORDER BY ... LIMIT`, not an unbounded namespace materialization.
        let mut envelopes = self.load_envelope_page(
            scan_after_request_id.as_deref(),
            MAX_SYNC_SCAN.saturating_add(1),
        )?;
        let has_more = envelopes.len() > MAX_SYNC_SCAN;
        if has_more {
            envelopes.truncate(MAX_SYNC_SCAN);
        }
        let candidate_count = envelopes.len();
        let mut report = ArchiveSyncReport {
            schema: ARCHIVE_INGEST_SCHEMA.to_string(),
            scanned: 0,
            attempted: 0,
            published: 0,
            unavailable: 0,
            reconciliation_required: 0,
            skipped: 0,
            acknowledged_cursor: self.read_acknowledged_cursor()?,
            next_scan_after_request_id: None,
            items: Vec::new(),
        };
        let mut last_scanned_request_id = None;

        for envelope in envelopes {
            if report.attempted >= limit {
                break;
            }
            report.scanned += 1;
            last_scanned_request_id = Some(envelope.request_id.clone());
            let Some(claimed) = self.claim_due(&envelope, owner, now_ms)? else {
                report.skipped += 1;
                continue;
            };
            report.attempted += 1;
            let request = envelope.ingest_request();
            let outcome = match transport.ingest(&request) {
                Ok(receipt) if receipt_matches(&receipt, &envelope) => {
                    if self.mark_published(&envelope, &claimed, now_ms, &receipt)? {
                        report.published += 1;
                        report.acknowledged_cursor =
                            max_cursor(report.acknowledged_cursor, receipt.archive_cursor);
                        ArchiveSyncItem {
                            request_id: envelope.request_id.clone(),
                            outcome: "published".to_string(),
                            error_code: None,
                        }
                    } else {
                        report.skipped += 1;
                        ArchiveSyncItem {
                            request_id: envelope.request_id.clone(),
                            outcome: "lease_lost".to_string(),
                            error_code: None,
                        }
                    }
                }
                Ok(_) => {
                    self.mark_failure(
                        &envelope,
                        &claimed,
                        now_ms,
                        ArchiveTransportFailure::new("archive_ack_integrity_mismatch", false),
                    )?;
                    report.reconciliation_required += 1;
                    ArchiveSyncItem {
                        request_id: envelope.request_id.clone(),
                        outcome: "reconciliation_required".to_string(),
                        error_code: Some("archive_ack_integrity_mismatch".to_string()),
                    }
                }
                Err(failure) => {
                    let error_code = failure.code.clone();
                    if failure.retryable {
                        self.mark_failure(&envelope, &claimed, now_ms, failure)?;
                        report.unavailable += 1;
                        ArchiveSyncItem {
                            request_id: envelope.request_id.clone(),
                            outcome: "unavailable".to_string(),
                            error_code: Some(error_code),
                        }
                    } else {
                        self.mark_failure(&envelope, &claimed, now_ms, failure)?;
                        report.reconciliation_required += 1;
                        ArchiveSyncItem {
                            request_id: envelope.request_id.clone(),
                            outcome: "reconciliation_required".to_string(),
                            error_code: Some(error_code),
                        }
                    }
                }
            };
            report.items.push(outcome);
        }
        report.next_scan_after_request_id = if last_scanned_request_id.is_some()
            && (report.scanned < candidate_count || has_more)
        {
            last_scanned_request_id
        } else {
            None
        };
        self.store_scan_cursor(owner, now_ms, report.next_scan_after_request_id.as_deref())?;
        self.store_sync_status(owner, now_ms, &report)?;
        Ok(report)
    }

    pub fn status(&self) -> Result<ArchiveOutboxStatus, ArchiveOutboxError> {
        let sync_status = self
            .runtime
            .storage
            .get_json(SYNC_STATUS_NS, SYNC_STATUS_KEY)
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?;
        let mut status = ArchiveOutboxStatus {
            schema: ARCHIVE_INGEST_SCHEMA.to_string(),
            total: 0,
            pending: 0,
            leased: 0,
            published: 0,
            unavailable: 0,
            reconciliation_required: 0,
            oldest_pending_at_ms: None,
            last_sync_at_ms: sync_status
                .as_ref()
                .and_then(|value| value["lastSyncAtMs"].as_i64()),
            acknowledged_cursor: sync_status
                .as_ref()
                .and_then(|value| value["acknowledgedCursor"].as_i64()),
        };
        for envelope in self.load_envelopes()? {
            let sync = self.read_state_or_default(&envelope)?;
            status.total += 1;
            match sync.state.as_str() {
                "published" => status.published += 1,
                "reconciliation_required" => status.reconciliation_required += 1,
                "leased" => status.leased += 1,
                "unavailable" => {
                    status.unavailable += 1;
                    status.oldest_pending_at_ms = Some(
                        status
                            .oldest_pending_at_ms
                            .map_or(envelope.occurred_at_ms, |current| {
                                current.min(envelope.occurred_at_ms)
                            }),
                    );
                }
                _ => {
                    status.pending += 1;
                    status.oldest_pending_at_ms = Some(
                        status
                            .oldest_pending_at_ms
                            .map_or(envelope.occurred_at_ms, |current| {
                                current.min(envelope.occurred_at_ms)
                            }),
                    );
                }
            }
        }
        Ok(status)
    }

    pub fn query(
        &self,
        query: ArchiveEvidenceQuery,
    ) -> Result<ArchiveEvidencePage, ArchiveOutboxError> {
        if query.limit == 0 {
            return Err(ArchiveOutboxError::InvalidLimit);
        }
        if let Some(tenant_id) = &query.tenant_id {
            validate_identifier(tenant_id)?;
        }
        if let Some(workspace_id) = &query.workspace_id {
            validate_identifier(workspace_id)?;
        }
        if let Some(after) = &query.after_request_id {
            validate_identifier(after)?;
        }
        let limit = query.limit.min(MAX_QUERY_BATCH);
        let mut matching = self
            .load_envelopes()?
            .into_iter()
            .filter(|envelope| {
                query
                    .tenant_id
                    .as_ref()
                    .is_none_or(|tenant_id| &envelope.tenant_id == tenant_id)
            })
            .filter(|envelope| {
                query
                    .workspace_id
                    .as_ref()
                    .is_none_or(|workspace_id| &envelope.workspace_id == workspace_id)
            })
            .filter(|envelope| {
                query
                    .after_request_id
                    .as_ref()
                    .is_none_or(|after| envelope.request_id.as_str() > after.as_str())
            })
            .collect::<Vec<_>>();
        matching.sort_by(|left, right| left.request_id.cmp(&right.request_id));
        let has_more = matching.len() > limit;
        matching.truncate(limit);
        let records = matching
            .into_iter()
            .map(|evidence| {
                let sync = self.read_state_or_default(&evidence)?;
                Ok(ArchiveEvidenceRecord { evidence, sync })
            })
            .collect::<Result<Vec<_>, ArchiveOutboxError>>()?;
        let next_after_request_id = has_more
            .then(|| {
                records
                    .last()
                    .map(|record| record.evidence.request_id.clone())
            })
            .flatten();
        Ok(ArchiveEvidencePage {
            records,
            next_after_request_id,
        })
    }

    fn replay_enqueue(
        &self,
        envelope: &ArchiveEvidenceEnvelope,
        context: &SideEffectContext,
    ) -> Result<ArchiveEnqueueReceipt, ArchiveOutboxError> {
        let Some(mapping) = self
            .runtime
            .storage
            .get_json(IDEMPOTENCY_NS, envelope.idempotency_key.as_str())
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?
        else {
            return Err(ArchiveOutboxError::StorageInconsistent);
        };
        if mapping != idempotency_mapping(envelope) {
            return Err(ArchiveOutboxError::IdempotencyConflict);
        }
        let stored = self.load_envelope(&envelope.request_id)?;
        if stored != *envelope {
            return Err(ArchiveOutboxError::RequestIdConflict);
        }
        let sync = match self
            .runtime
            .storage
            .get_json(STATES_NS, &envelope.request_id)
        {
            Ok(Some(value)) => decode_status(value)?,
            Ok(None) => {
                let pending = ArchiveEvidenceSyncStatus::pending(envelope.occurred_at_ms);
                match self.runtime.storage.put_json_if_absent(
                    STATES_NS,
                    &envelope.request_id,
                    serde_json::to_value(&pending)
                        .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
                    context,
                ) {
                    Ok(_) => pending,
                    Err(_) => return Err(ArchiveOutboxError::StorageUnavailable),
                }
            }
            Err(_) => return Err(ArchiveOutboxError::StorageUnavailable),
        };
        Ok(ArchiveEnqueueReceipt {
            evidence: stored,
            sync,
            duplicate: true,
        })
    }

    fn load_envelope(
        &self,
        request_id: &str,
    ) -> Result<ArchiveEvidenceEnvelope, ArchiveOutboxError> {
        let Some(value) = self
            .runtime
            .storage
            .get_json(RECORDS_NS, request_id)
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?
        else {
            return Err(ArchiveOutboxError::StorageInconsistent);
        };
        decode_envelope(value)
    }

    fn load_envelopes(&self) -> Result<Vec<ArchiveEvidenceEnvelope>, ArchiveOutboxError> {
        let mut records = self
            .runtime
            .storage
            .list_json(RECORDS_NS)
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?
            .into_iter()
            .map(|(_, value)| decode_envelope(value))
            .collect::<Result<Vec<_>, _>>()?;
        records.sort_by(|left, right| {
            (left.occurred_at_ms, left.request_id.as_str())
                .cmp(&(right.occurred_at_ms, right.request_id.as_str()))
        });
        Ok(records)
    }

    fn load_envelope_page(
        &self,
        after_request_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ArchiveEvidenceEnvelope>, ArchiveOutboxError> {
        let records = self
            .runtime
            .storage
            .list_json_page(RECORDS_NS, after_request_id, limit)
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?;
        records
            .into_iter()
            .map(|(key, value)| {
                let envelope = decode_envelope(value)?;
                if envelope.request_id != key {
                    return Err(ArchiveOutboxError::StorageInconsistent);
                }
                Ok(envelope)
            })
            .collect()
    }

    fn read_state_or_default(
        &self,
        envelope: &ArchiveEvidenceEnvelope,
    ) -> Result<ArchiveEvidenceSyncStatus, ArchiveOutboxError> {
        match self
            .runtime
            .storage
            .get_json(STATES_NS, &envelope.request_id)
        {
            Ok(Some(value)) => decode_status(value),
            Ok(None) => Ok(ArchiveEvidenceSyncStatus::pending(envelope.occurred_at_ms)),
            Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
        }
    }

    fn load_or_repair_state(
        &self,
        envelope: &ArchiveEvidenceEnvelope,
    ) -> Result<ArchiveEvidenceSyncStatus, ArchiveOutboxError> {
        match self
            .runtime
            .storage
            .get_json(STATES_NS, &envelope.request_id)
        {
            Ok(Some(value)) => decode_status(value),
            Ok(None) => {
                let pending = ArchiveEvidenceSyncStatus::pending(envelope.occurred_at_ms);
                let context = state_context(envelope, "repair")?;
                match self.runtime.storage.put_json_if_absent(
                    STATES_NS,
                    &envelope.request_id,
                    serde_json::to_value(&pending)
                        .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
                    &context,
                ) {
                    Ok(_) => Ok(pending),
                    Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
                }
            }
            Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
        }
    }

    fn claim_due(
        &self,
        envelope: &ArchiveEvidenceEnvelope,
        owner: &str,
        now_ms: i64,
    ) -> Result<Option<ArchiveEvidenceSyncStatus>, ArchiveOutboxError> {
        let current = self.load_or_repair_state(envelope)?;
        if !current.due(now_ms) {
            return Ok(None);
        }
        let mut claimed = current.clone();
        claimed.state = "leased".to_string();
        claimed.attempts = claimed.attempts.saturating_add(1);
        claimed.last_attempt_at_ms = Some(now_ms);
        claimed.next_attempt_at_ms = None;
        claimed.lease_owner = Some(owner.to_string());
        claimed.lease_until_ms = Some(now_ms.saturating_add(LEASE_MS));
        claimed.fencing_token = claimed.fencing_token.saturating_add(1);
        let context = state_context(envelope, &format!("claim-{}", claimed.fencing_token))?;
        match self.runtime.storage.compare_and_put_json(
            STATES_NS,
            &envelope.request_id,
            serde_json::to_value(&current).map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            serde_json::to_value(&claimed).map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            &context,
        ) {
            Ok(ComparePutOutcome::Updated) => Ok(Some(claimed)),
            Ok(ComparePutOutcome::Conflict) => Ok(None),
            Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
        }
    }

    fn mark_published(
        &self,
        envelope: &ArchiveEvidenceEnvelope,
        claimed: &ArchiveEvidenceSyncStatus,
        now_ms: i64,
        receipt: &ArchiveIngestReceipt,
    ) -> Result<bool, ArchiveOutboxError> {
        let mut published = claimed.clone();
        published.state = "published".to_string();
        published.next_attempt_at_ms = None;
        published.last_error_code = None;
        published.published_at_ms = Some(now_ms);
        published.entitlement_snapshot_ref = Some(receipt.entitlement_snapshot_ref.clone());
        published.archive_ref = Some(receipt.archive_ref.clone());
        published.record_digest_sha256 = Some(receipt.record_digest_sha256.clone());
        published.acknowledged_cursor = receipt.archive_cursor;
        published.lease_owner = None;
        published.lease_until_ms = None;
        let context = state_context(envelope, &format!("published-{}", claimed.fencing_token))?;
        match self.runtime.storage.compare_and_put_json(
            STATES_NS,
            &envelope.request_id,
            serde_json::to_value(claimed).map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            serde_json::to_value(&published)
                .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            &context,
        ) {
            Ok(ComparePutOutcome::Updated) => Ok(true),
            Ok(ComparePutOutcome::Conflict) => Ok(false),
            Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
        }
    }

    fn mark_failure(
        &self,
        envelope: &ArchiveEvidenceEnvelope,
        claimed: &ArchiveEvidenceSyncStatus,
        now_ms: i64,
        failure: ArchiveTransportFailure,
    ) -> Result<bool, ArchiveOutboxError> {
        let mut replacement = claimed.clone();
        replacement.state = if failure.retryable {
            "unavailable".to_string()
        } else {
            "reconciliation_required".to_string()
        };
        replacement.next_attempt_at_ms = failure
            .retryable
            .then(|| now_ms.saturating_add(retry_delay_ms(claimed.attempts)));
        replacement.last_error_code = Some(failure.code);
        replacement.lease_owner = None;
        replacement.lease_until_ms = None;
        let context = state_context(envelope, &format!("failed-{}", claimed.fencing_token))?;
        match self.runtime.storage.compare_and_put_json(
            STATES_NS,
            &envelope.request_id,
            serde_json::to_value(claimed).map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            serde_json::to_value(&replacement)
                .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
            &context,
        ) {
            Ok(ComparePutOutcome::Updated) => Ok(true),
            Ok(ComparePutOutcome::Conflict) => Ok(false),
            Err(_) => Err(ArchiveOutboxError::StorageUnavailable),
        }
    }

    fn read_scan_cursor(&self, owner: &str) -> Result<Option<String>, ArchiveOutboxError> {
        let value = self
            .runtime
            .storage
            .get_json(SYNC_CURSOR_NS, owner)
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?;
        let Some(value) = value else {
            return Ok(None);
        };
        match value.get("afterRequestId") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(after_request_id)) if is_safe_identifier(after_request_id) => {
                Ok(Some(after_request_id.clone()))
            }
            _ => Err(ArchiveOutboxError::StorageInconsistent),
        }
    }

    fn store_scan_cursor(
        &self,
        owner: &str,
        now_ms: i64,
        after_request_id: Option<&str>,
    ) -> Result<(), ArchiveOutboxError> {
        let context = sync_context(owner, now_ms, "cursor")?;
        self.runtime
            .storage
            .put_json(
                SYNC_CURSOR_NS,
                owner,
                json!({
                    "schema": ARCHIVE_INGEST_SCHEMA,
                    "owner": owner,
                    "afterRequestId": after_request_id,
                    "updatedAtMs": now_ms,
                }),
                &context,
            )
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)
    }

    fn store_sync_status(
        &self,
        owner: &str,
        now_ms: i64,
        report: &ArchiveSyncReport,
    ) -> Result<(), ArchiveOutboxError> {
        let context = sync_context(owner, now_ms, "report")?;
        // Several independently scheduled workers may publish a status
        // receipt. Merge the high-water cursor with compare-and-put so a
        // slower writer cannot regress it after a later archive acknowledgement.
        let mut current = self
            .runtime
            .storage
            .get_json(SYNC_STATUS_NS, SYNC_STATUS_KEY)
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?;
        for _ in 0..4 {
            let acknowledged_cursor = max_cursor(
                current
                    .as_ref()
                    .and_then(|value| value["acknowledgedCursor"].as_i64()),
                report.acknowledged_cursor,
            );
            let replacement = json!({
                "schema": ARCHIVE_INGEST_SCHEMA,
                "lastSyncAtMs": now_ms,
                "owner": owner,
                "scanned": report.scanned,
                "attempted": report.attempted,
                "published": report.published,
                "unavailable": report.unavailable,
                "reconciliationRequired": report.reconciliation_required,
                "skipped": report.skipped,
                "acknowledgedCursor": acknowledged_cursor,
                "nextScanAfterRequestId": report.next_scan_after_request_id,
            });
            let outcome = match &current {
                Some(expected) => self.runtime.storage.compare_and_put_json(
                    SYNC_STATUS_NS,
                    SYNC_STATUS_KEY,
                    expected.clone(),
                    replacement,
                    &context,
                ),
                None => self
                    .runtime
                    .storage
                    .put_json_if_absent(SYNC_STATUS_NS, SYNC_STATUS_KEY, replacement, &context)
                    .map(|outcome| match outcome {
                        ImmutablePutOutcome::Created => ComparePutOutcome::Updated,
                        ImmutablePutOutcome::AlreadyPresent => ComparePutOutcome::Conflict,
                    }),
            }
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)?;
            if outcome == ComparePutOutcome::Updated {
                return Ok(());
            }
            current = self
                .runtime
                .storage
                .get_json(SYNC_STATUS_NS, SYNC_STATUS_KEY)
                .map_err(|_| ArchiveOutboxError::StorageUnavailable)?;
        }
        Err(ArchiveOutboxError::StorageUnavailable)
    }

    fn read_acknowledged_cursor(&self) -> Result<Option<i64>, ArchiveOutboxError> {
        self.runtime
            .storage
            .get_json(SYNC_STATUS_NS, SYNC_STATUS_KEY)
            .map_err(|_| ArchiveOutboxError::StorageUnavailable)
            .map(|value| value.and_then(|value| value["acknowledgedCursor"].as_i64()))
    }
}

fn envelope_from_input(
    input: &ArchiveEvidenceInput,
) -> Result<ArchiveEvidenceEnvelope, ArchiveOutboxError> {
    if input.request_id.trim().is_empty() {
        return Err(ArchiveOutboxError::RequestIdRequired);
    }
    if input.source.trim().is_empty() {
        return Err(ArchiveOutboxError::SourceRequired);
    }
    if input.aggregate_id.trim().is_empty() {
        return Err(ArchiveOutboxError::AggregateIdRequired);
    }
    for identifier in [
        &input.tenant_id,
        &input.workspace_id,
        &input.tool_name,
        &input.request_id,
        &input.source,
        &input.aggregate_id,
        input.context.idempotency_key.as_str(),
    ] {
        validate_identifier(identifier)?;
    }
    if input.occurred_at_ms <= 0 {
        return Err(ArchiveOutboxError::InvalidOccurredAt);
    }
    if !input.payload.is_object() {
        return Err(ArchiveOutboxError::InvalidPayload);
    }
    if contains_sensitive_key(&input.payload) {
        return Err(ArchiveOutboxError::SensitivePayload);
    }
    let payload = json!({
        "schema": ARCHIVE_EVIDENCE_SCHEMA,
        "kind": input.evidence_kind,
        "source": input.source.clone(),
        "aggregateId": input.aggregate_id.clone(),
        "authorityContext": input.context.authority.clone(),
        "redacted": true,
        "evidence": input.payload.clone(),
    });
    let canonical_payload = canonical_bytes(&payload)?;
    if canonical_payload.len() > MAX_EVIDENCE_BYTES {
        return Err(ArchiveOutboxError::PayloadTooLarge);
    }
    let payload_sha256 = hex_digest(&canonical_payload);
    let mut envelope = ArchiveEvidenceEnvelope {
        schema: ARCHIVE_INGEST_SCHEMA.to_string(),
        tenant_id: input.tenant_id.clone(),
        workspace_id: input.workspace_id.clone(),
        tool_name: input.tool_name.clone(),
        request_id: input.request_id.clone(),
        idempotency_key: input.context.idempotency_key.clone(),
        payload,
        payload_sha256,
        occurred_at_ms: input.occurred_at_ms,
        request_digest_sha256: String::new(),
    };
    envelope.request_digest_sha256 = request_digest(&envelope)?;
    envelope.verify_integrity()?;
    Ok(envelope)
}

fn canonical_bytes(value: &Value) -> Result<Vec<u8>, ArchiveOutboxError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| ArchiveOutboxError::InvalidEnvelope)
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn request_digest(envelope: &ArchiveEvidenceEnvelope) -> Result<String, ArchiveOutboxError> {
    request_digest_from_parts(
        &envelope.schema,
        &envelope.tenant_id,
        &envelope.workspace_id,
        &envelope.tool_name,
        &envelope.request_id,
        &envelope.payload_sha256,
        envelope.occurred_at_ms,
    )
}

fn request_digest_from_parts(
    schema: &str,
    tenant_id: &str,
    workspace_id: &str,
    tool_name: &str,
    request_id: &str,
    payload_sha256: &str,
    occurred_at_ms: i64,
) -> Result<String, ArchiveOutboxError> {
    let tuple = json!([
        schema,
        tenant_id,
        workspace_id,
        tool_name,
        request_id,
        payload_sha256,
        occurred_at_ms,
    ]);
    Ok(hex_digest(&canonical_bytes(&tuple)?))
}

fn idempotency_mapping(envelope: &ArchiveEvidenceEnvelope) -> Value {
    json!({
        "schema": ARCHIVE_INGEST_SCHEMA,
        "requestId": envelope.request_id,
        "requestDigestSha256": envelope.request_digest_sha256,
    })
}

fn decode_envelope(value: Value) -> Result<ArchiveEvidenceEnvelope, ArchiveOutboxError> {
    let envelope = serde_json::from_value::<ArchiveEvidenceEnvelope>(value)
        .map_err(|_| ArchiveOutboxError::StorageInconsistent)?;
    envelope.verify_integrity()?;
    Ok(envelope)
}

fn decode_status(value: Value) -> Result<ArchiveEvidenceSyncStatus, ArchiveOutboxError> {
    let status = serde_json::from_value::<ArchiveEvidenceSyncStatus>(value)
        .map_err(|_| ArchiveOutboxError::StorageInconsistent)?;
    if !matches!(
        status.state.as_str(),
        "pending" | "leased" | "published" | "unavailable" | "reconciliation_required"
    ) || status.attempts == 0 && status.state == "leased"
    {
        return Err(ArchiveOutboxError::StorageInconsistent);
    }
    Ok(status)
}

fn state_context(
    envelope: &ArchiveEvidenceEnvelope,
    action: &str,
) -> Result<SideEffectContext, ArchiveOutboxError> {
    Ok(SideEffectContext::new(
        AuthorityContext {
            actor: "archive-outbox".to_string(),
            surface: "archive-sync".to_string(),
            account_mode: "local".to_string(),
        },
        IdempotencyKey::new(format!("archive-evidence:{}:{action}", envelope.request_id))
            .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
    ))
}

fn sync_context(
    owner: &str,
    now_ms: i64,
    action: &str,
) -> Result<SideEffectContext, ArchiveOutboxError> {
    Ok(SideEffectContext::new(
        AuthorityContext {
            actor: format!("archive-sync:{owner}"),
            surface: "archive-sync".to_string(),
            account_mode: "local".to_string(),
        },
        IdempotencyKey::new(format!("archive-sync-{action}:{owner}:{now_ms}"))
            .map_err(|_| ArchiveOutboxError::StorageInconsistent)?,
    ))
}

fn receipt_matches(receipt: &ArchiveIngestReceipt, envelope: &ArchiveEvidenceEnvelope) -> bool {
    receipt.schema == ARCHIVE_INGEST_SCHEMA
        && receipt.request_id == envelope.request_id
        && receipt.idempotency_key == envelope.idempotency_key
        && receipt.request_digest_sha256 == envelope.request_digest_sha256
        && is_safe_identifier(&receipt.entitlement_snapshot_ref)
        && is_safe_identifier(&receipt.archive_ref)
        && valid_sha256(&receipt.record_digest_sha256)
        && matches!(
            cloud_archive_record_digest(envelope, &receipt.entitlement_snapshot_ref),
            Ok(expected) if expected == receipt.record_digest_sha256
        )
        && receipt.archive_cursor.is_none_or(|cursor| cursor >= 0)
}

/// Matches the product-owned Cloud `RedactedArchiveEnvelope` digest. Core has
/// all of these values once a gateway receipt is returned. Its own envelope
/// rejects sensitive keys before persistence, so Cloud's redaction projection
/// is exactly the persisted payload rather than an unverified assertion.
fn cloud_archive_record_digest(
    envelope: &ArchiveEvidenceEnvelope,
    entitlement_snapshot_ref: &str,
) -> Result<String, ArchiveOutboxError> {
    let record = json!({
        "schema": CLOUD_ARCHIVE_RECORD_SCHEMA,
        "tenantId": envelope.tenant_id,
        "workspaceId": envelope.workspace_id,
        "toolName": envelope.tool_name,
        "requestId": envelope.request_id,
        "requestDigestSha256": envelope.request_digest_sha256,
        "entitlementSnapshotRef": entitlement_snapshot_ref,
        "occurredAtMs": envelope.occurred_at_ms,
        "redactedPayload": envelope.payload,
    });
    Ok(hex_digest(&canonical_bytes(&record)?))
}

fn max_cursor(current: Option<i64>, candidate: Option<i64>) -> Option<i64> {
    match (current, candidate) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn retry_delay_ms(attempts: u32) -> i64 {
    let shift = attempts.saturating_sub(1).min(6);
    1_000_i64.saturating_mul(1_i64 << shift).min(60_000)
}

fn validate_identifier(value: &str) -> Result<(), ArchiveOutboxError> {
    if is_safe_identifier(value) {
        Ok(())
    } else {
        Err(ArchiveOutboxError::InvalidIdentifier)
    }
}

fn is_safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn safe_error_code(value: &str) -> String {
    let valid = !value.is_empty()
        && value.len() <= 96
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        });
    if valid {
        value.to_string()
    } else {
        "archive_transport_failed".to_string()
    }
}

fn contains_sensitive_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let normalized = key
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>();
            [
                "token",
                "secret",
                "password",
                "authorization",
                "credential",
                "privatekey",
                "apikey",
                "authorizationcode",
            ]
            .iter()
            .any(|needle| normalized.contains(needle))
                || contains_sensitive_key(value)
        }),
        Value::Array(values) => values.iter().any(contains_sensitive_key),
        _ => false,
    }
}

/// A concise descriptor helper for test and adapter implementations. The
/// transport is intentionally LocalOnly until a product-owned adapter provides
/// authenticated cloud configuration outside Core.
pub fn archive_transport_descriptor(adapter_id: impl Into<String>) -> PortDescriptor {
    let mut descriptor = PortDescriptor::new(PortKind::Outbox, adapter_id)
        .for_profiles(&["local", "self-hosted"])
        .with_capabilities(&[
            "archive.ingest.redacted-evidence",
            "archive.idempotent-replay",
            "archive.integrity.sha256-jcs",
        ]);
    descriptor.failure_mode = FailureMode::LocalOnly;
    descriptor.configuration_schema = json!({
        "type": "object",
        "additionalProperties": false,
    });
    descriptor.redaction_rules = vec![
        "credential-free".to_string(),
        "redacted-evidence-only".to_string(),
        "no-raw-transport-errors".to_string(),
    ];
    descriptor
}
