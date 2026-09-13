// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::adapters::studio_identity::TrustedStudioVerifier;
use crate::auth::{authorize_local_owner, AuthzActor, AuthzRequest, AuthzTarget};
use crate::capability::LocalCapabilityResolver;
use crate::finance_authority::{FinanceAuthorityPort, WardenSidecarAuthority};
use crate::ports::bus::EventBusPort;
use crate::ports::credentials::{BrokerCredentials, CredentialPort, CredentialStatus};
use crate::ports::journal::{JournalEvent, JournalPort};
use crate::ports::provider::{ProviderCapability, ProviderPort};
use crate::ports::storage::StoragePort;
use crate::ports::{
    ClockPort, ComparePutOutcome, DurableQueuePort, EventAppend, EventRecord, EventStreamPort,
    EvidencePort, EvidenceRecord, ExportArtifact, ExportPort, FailureMode, IdempotencyKey,
    IdentityClaims, IdentityPort, ImmutablePutOutcome, InboxRepository, LeaseClaim,
    LeaseRepository, OutboxRecord, OutboxRepository, PolicyDecision, PolicyPort, PolicyRequest,
    PortDescriptor, PortKind, QueueDelivery, QueueRequest, ScheduledWork, SchedulerPort,
    ServiceRuntime, SideEffectContext, StorageExpectation, StorageWrite, TelemetryPort,
    TrustedStudioSession, VersionedPort,
};
use crate::runtime_config::{BootstrapSecrets, EnvSecretResolver, RuntimeConfig, SecretResolver};
use async_nats::header::{HeaderMap, NATS_MESSAGE_ID};
use async_nats::jetstream::consumer::{pull, AckPolicy, DeliverPolicy};
use async_nats::jetstream::context::traits::Publisher as JetStreamPublisher;
use async_nats::jetstream::context::PublishAckFuture;
use async_nats::jetstream::message::PublishMessage;
use async_nats::jetstream::publish::PublishAck;
use async_nats::jetstream::stream;
use async_nats::jetstream::{self, AckKind};
use postgres::{Client, NoTls, Row};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const POSTGRES_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tradeassembly_kv (
    namespace TEXT NOT NULL,
    item_key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    authority_actor TEXT NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    PRIMARY KEY (namespace, item_key)
);
CREATE TABLE IF NOT EXISTS runtime_journal (
    journal_id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    authority_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    recorded_at_ms BIGINT NOT NULL
);
ALTER TABLE runtime_journal ADD COLUMN IF NOT EXISTS owner_json TEXT;
CREATE TABLE IF NOT EXISTS runtime_queue_messages (
    message_id TEXT PRIMARY KEY,
    queue_name TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    partition_key TEXT,
    priority BIGINT NOT NULL,
    available_at_ms BIGINT NOT NULL,
    retention_until_ms BIGINT NOT NULL,
    max_attempts INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    attempt INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_until_ms BIGINT,
    fencing_token BIGINT NOT NULL DEFAULT 0,
    delivery_reply_subject TEXT,
    last_stream_sequence BIGINT,
    last_consumer_sequence BIGINT,
    dead_letter_reason TEXT,
    published_at_ms BIGINT,
    created_at_ms BIGINT NOT NULL,
    UNIQUE(queue_name, idempotency_key)
);
ALTER TABLE runtime_queue_messages
  ADD COLUMN IF NOT EXISTS published_at_ms BIGINT;
CREATE INDEX IF NOT EXISTS runtime_queue_claim_idx
  ON runtime_queue_messages(queue_name, state, available_at_ms, priority DESC, message_id);
CREATE TABLE IF NOT EXISTS runtime_event_checkpoints (
    consumer_name TEXT NOT NULL,
    stream_name TEXT NOT NULL,
    sequence_num BIGINT NOT NULL,
    PRIMARY KEY(consumer_name, stream_name)
);
CREATE TABLE IF NOT EXISTS runtime_events (
    event_id TEXT PRIMARY KEY,
    stream_name TEXT NOT NULL,
    sequence_num BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    occurred_at_ms BIGINT NOT NULL,
    retention_until_ms BIGINT,
    published_at_ms BIGINT,
    UNIQUE(stream_name, sequence_num),
    UNIQUE(stream_name, idempotency_key)
);
CREATE INDEX IF NOT EXISTS runtime_events_pending_idx
  ON runtime_events(published_at_ms, stream_name, sequence_num);
CREATE TABLE IF NOT EXISTS runtime_schedules (
    schedule_key TEXT PRIMARY KEY,
    queue_name TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    due_at_ms BIGINT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL DEFAULT 'pending',
    lease_owner TEXT,
    lease_until_ms BIGINT,
    fencing_token BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS runtime_schedule_due_idx
  ON runtime_schedules(state, due_at_ms);
CREATE TABLE IF NOT EXISTS runtime_outbox (
    outbox_id TEXT PRIMARY KEY,
    topic TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    created_at_ms BIGINT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    lease_owner TEXT,
    lease_until_ms BIGINT,
    fencing_token BIGINT NOT NULL DEFAULT 0
);
ALTER TABLE runtime_outbox
  ADD COLUMN IF NOT EXISTS fencing_token BIGINT NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS runtime_inbox (
    source TEXT NOT NULL,
    message_id TEXT NOT NULL,
    signature_ref TEXT,
    received_at_ms BIGINT NOT NULL,
    PRIMARY KEY(source, message_id)
);
CREATE TABLE IF NOT EXISTS runtime_leases (
    resource TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    fencing_token BIGINT NOT NULL,
    expires_at_ms BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_evidence (
    evidence_id TEXT PRIMARY KEY,
    evidence_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    recorded_at_ms BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS runtime_evidence_aggregate_idx
  ON runtime_evidence(aggregate_id, recorded_at_ms, evidence_id);
CREATE TABLE IF NOT EXISTS runtime_telemetry (
    event_id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    fields_json TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    created_at_ms BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS oidc_sessions (
    session_ref TEXT PRIMARY KEY,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    audience_json TEXT NOT NULL,
    assurance TEXT,
    expires_at_ms BIGINT NOT NULL
);
"#;

const QUEUE_STREAM: &str = "TRADEASSEMBLY_QUEUE";
const EVENT_STREAM: &str = "TRADEASSEMBLY_EVENT";
const BUS_STREAM: &str = "TRADEASSEMBLY_NOTIFY";
const QUEUE_TRANSPORT_ACK_WAIT: Duration = Duration::from_secs(5);

pub fn runtime_from_config(
    config: &RuntimeConfig,
    secrets: &BootstrapSecrets,
) -> Result<ServiceRuntime, String> {
    runtime_from_config_with_authority(config, secrets, None)
}

pub fn runtime_from_config_with_authority(
    config: &RuntimeConfig,
    secrets: &BootstrapSecrets,
    authority: Option<Arc<dyn FinanceAuthorityPort>>,
) -> Result<ServiceRuntime, String> {
    config.validate()?;
    let state = Arc::new(SelfHostedPostgresState::new(
        &secrets.postgres_url,
        config.telemetry_enabled,
        &config.oidc_issuer,
        config.identity_client_binding(),
        secrets.studio_core_token.clone(),
    )?);
    let jetstream = Arc::new(SelfHostedJetStream::new(&secrets.nats_url, state.clone())?);
    let artifact_root = file_artifact_root(
        config
            .object_store_endpoint
            .as_deref()
            .ok_or_else(|| "object store endpoint is required".to_string())?,
    )?;
    let credentials: Arc<dyn CredentialPort> = Arc::new(SelfHostedCredentialStore::default());
    let finance_authority = match authority {
        Some(authority) => authority,
        None => Arc::new(WardenSidecarAuthority::new(
            &config.warden_sidecar_url,
            &secrets.warden_token,
            &config.warden_required_version,
        )?),
    };
    let plugin_storage: Arc<dyn StoragePort> = state.clone();
    let plugin_packages: Arc<dyn crate::ports::PluginPackagePort> = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            artifact_root.join("plugins"),
        )?,
    );
    let plugins: Arc<dyn crate::ports::PluginRegistryPort> = Arc::new(
        crate::adapters::plugin_registry::StoragePluginRegistry::new(
            Arc::clone(&plugin_storage),
            "self_hosted",
        ),
    );
    let clock: Arc<dyn crate::ports::ClockPort> = Arc::new(SelfHostedClock);
    let plugin_sandbox: Arc<dyn crate::ports::PluginProcessSandboxPort> = Arc::new(
        crate::adapters::plugin_sandbox::SandboxRuntimePluginSandbox::new(
            crate::adapters::plugin_sandbox::resolve_sandbox_command(
                &config.plugin_sandbox_command,
            ),
            artifact_root.join("plugins/sandbox-settings"),
            config.plugin_sandbox_allow_local_egress,
        )?,
    );
    let dataset_snapshots = Arc::new(
        crate::adapters::dataset_snapshots::StorageDatasetSnapshotRepository::new(state.clone()),
    );
    let backtests = Arc::new(
        crate::adapters::backtest_repository::StorageBacktestRunRepository::new(state.clone()),
    );
    let plugin_operations = Arc::new(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            state.clone(),
            Arc::clone(&plugins),
            Arc::clone(&credentials),
            Arc::clone(&plugin_packages),
            Arc::clone(&clock),
            plugin_sandbox,
        ),
    );
    let robustness = Arc::new(
        crate::adapters::robustness_repository::StorageRobustnessRunRepository::new(state.clone()),
    );
    let historical_data = Arc::new(
        crate::adapters::historical_data::PluginHistoricalDataAdapter::new(
            plugin_operations.clone(),
        )?,
    );
    Ok(ServiceRuntime {
        storage: state.clone(),
        object_authorization: Arc::new(
            crate::adapters::object_authorization::StorageObjectAuthorization::new(state.clone()),
        ),
        finance_authority,
        capability_resolver: Arc::new(LocalCapabilityResolver),
        credentials,
        historical_data,
        dataset_snapshots,
        backtests,
        robustness,
        providers: Arc::new(SelfHostedProviderCatalog),
        plugin_operations,
        journal: state.clone(),
        journal_owner: None,
        legal_receipts: Arc::new(
            crate::adapters::legal_receipts::FileLegalReceiptVerifier::new(
                &config.legal_receipt_root,
                &config.legal_trusted_keys_path,
                &config.legal_policy_path,
            ),
        ),
        bus: jetstream.clone(),
        queue: jetstream.clone(),
        events: jetstream,
        scheduler: state.clone(),
        outbox: state.clone(),
        inbox: state.clone(),
        leases: state.clone(),
        identity: state.clone(),
        policy: Arc::new(SelfHostedPolicy),
        evidence: state.clone(),
        plugins,
        plugin_packages,
        telemetry: state,
        clock,
        exports: Arc::new(SelfHostedFileExportStore::new(artifact_root)?),
        scheduler_wake: Arc::new(crate::ports::SchedulerWake::default()),
    })
}

fn file_artifact_root(endpoint: &str) -> Result<PathBuf, String> {
    if let Some(path) = endpoint.strip_prefix("file://") {
        Ok(PathBuf::from(path))
    } else {
        Err("public-core self_hosted currently requires file:// artifact storage".to_string())
    }
}

struct SelfHostedPostgresState {
    client: Mutex<Client>,
    telemetry_enabled: bool,
    oidc_issuer: String,
    oidc_audience: String,
    trusted_studio: TrustedStudioVerifier,
}

impl SelfHostedPostgresState {
    fn new(
        database_url: &str,
        telemetry_enabled: bool,
        oidc_issuer: &str,
        oidc_audience: &str,
        studio_core_token: Option<String>,
    ) -> Result<Self, String> {
        let client = run_postgres_blocking(|| {
            let mut client = Client::connect(database_url, NoTls).map_err(redacted_pg_error)?;
            client
                .batch_execute(POSTGRES_SCHEMA)
                .map_err(redacted_pg_error)?;
            Ok::<Client, String>(client)
        })?;
        Ok(Self {
            client: Mutex::new(client),
            telemetry_enabled,
            oidc_issuer: oidc_issuer.to_string(),
            oidc_audience: oidc_audience.to_string(),
            trusted_studio: TrustedStudioVerifier::new(
                oidc_issuer,
                oidc_audience,
                studio_core_token,
            )?,
        })
    }

    fn with_client<T: Send>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        run_postgres_blocking(|| {
            let mut client = self
                .client
                .lock()
                .map_err(|_| "postgres state lock poisoned".to_string())?;
            f(&mut client)
        })
    }

    fn descriptor(&self, kind: PortKind, capabilities: &[&str]) -> PortDescriptor {
        PortDescriptor::new(kind, "self_hosted.postgres")
            .for_profiles(&["self_hosted"])
            .with_capabilities(capabilities)
    }

    fn upsert_queue_metadata(&self, request: &QueueRequest) -> Result<(String, bool), String> {
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
        self.with_client(|client| {
            let mut transaction = client.transaction().map_err(redacted_pg_error)?;
            transaction
                .execute(
                    r#"INSERT INTO runtime_queue_messages
                       (message_id, queue_name, payload_json, idempotency_key, partition_key, priority,
                        available_at_ms, retention_until_ms, max_attempts, created_at_ms)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                       ON CONFLICT (queue_name, idempotency_key) DO NOTHING"#,
                    &[
                        &message_id,
                        &request.queue,
                        &payload,
                        &request.idempotency_key.as_str(),
                        &request.partition_key,
                        &request.priority,
                        &request.available_at_ms,
                        &request.retention_until_ms,
                        &(request.max_attempts as i32),
                        &epoch_ms(),
                    ],
                )
                .map_err(redacted_pg_error)?;
            let row = transaction
                .query_one(
                    r#"SELECT message_id, payload_json, partition_key, priority, available_at_ms,
                              retention_until_ms, max_attempts, published_at_ms
                       FROM runtime_queue_messages
                       WHERE queue_name=$1 AND idempotency_key=$2
                       FOR UPDATE"#,
                    &[&request.queue, &request.idempotency_key.as_str()],
                )
                .map_err(redacted_pg_error)?;
            let stored_payload = serde_json::from_str::<Value>(&row.get::<_, String>(1))
                .map_err(|error| error.to_string())?;
            let matches_request = stored_payload == request.payload
                && row.get::<_, Option<String>>(2) == request.partition_key
                && row.get::<_, i64>(3) == request.priority
                && row.get::<_, i64>(4) == request.available_at_ms
                && row.get::<_, i64>(5) == request.retention_until_ms
                && row.get::<_, i32>(6) == request.max_attempts as i32;
            if !matches_request {
                return Err("queue idempotency key was reused with a different request".to_string());
            }
            let stored_message_id = row.get::<_, String>(0);
            let should_publish = row.get::<_, Option<i64>>(7).is_none();
            transaction.commit().map_err(redacted_pg_error)?;
            Ok((stored_message_id, should_publish))
        })
    }

    fn mark_queue_published(&self, message_id: &str) -> Result<(), String> {
        self.with_client(|client| {
            let updated = client
                .execute(
                    r#"UPDATE runtime_queue_messages
                       SET published_at_ms=COALESCE(published_at_ms, $2)
                       WHERE message_id=$1"#,
                    &[&message_id, &epoch_ms()],
                )
                .map_err(redacted_pg_error)?;
            if updated != 1 {
                return Err("queue publication metadata is unavailable".to_string());
            }
            Ok(())
        })
    }

    fn queue_subject(&self, queue: &str) -> String {
        format!("queue.{}", nats_component(queue))
    }

    fn event_subject(&self, stream: &str) -> String {
        format!("event.{}", nats_component(stream))
    }

    fn bus_subject(&self, topic: &str) -> String {
        format!("notify.{}", nats_component(topic))
    }

    fn stage_expired_queue_replies(&self, queue: &str, now_ms: i64) -> Result<(), String> {
        self.with_client(|client| {
            client
                .execute(
                    r#"UPDATE runtime_queue_messages
                       SET state=CASE
                             WHEN attempt >= max_attempts OR retention_until_ms <= $2
                               THEN 'terminating'
                             ELSE 'retrying'
                           END,
                           available_at_ms=GREATEST(available_at_ms, $2),
                           dead_letter_reason=CASE
                             WHEN retention_until_ms <= $2
                               THEN COALESCE(dead_letter_reason, 'retention_expired')
                             WHEN attempt >= max_attempts
                               THEN COALESCE(dead_letter_reason, 'max_attempts_exhausted')
                             ELSE dead_letter_reason
                           END
                       WHERE queue_name=$1 AND state='leased' AND lease_until_ms <= $2
                         AND delivery_reply_subject IS NOT NULL"#,
                    &[&queue, &now_ms],
                )
                .map_err(redacted_pg_error)?;
            Ok(())
        })
    }

    fn pending_queue_replies(
        &self,
        queue: &str,
        now_ms: i64,
    ) -> Result<Vec<PendingQueueReply>, String> {
        self.with_client(|client| {
            client
                .query(
                    r#"SELECT message_id, fencing_token, delivery_reply_subject, state,
                              available_at_ms
                       FROM runtime_queue_messages
                       WHERE queue_name=$1
                         AND state IN ('acknowledging', 'retrying', 'terminating')
                         AND delivery_reply_subject IS NOT NULL
                       ORDER BY message_id"#,
                    &[&queue],
                )
                .map_err(redacted_pg_error)?
                .into_iter()
                .map(|row| pending_queue_reply_from_row(&row, now_ms))
                .collect()
        })
    }

    fn claim_queue_delivery(
        &self,
        owner: &str,
        now_ms: i64,
        visibility_timeout_ms: i64,
        delivery: JetStreamDelivery,
    ) -> Result<QueueClaimOutcome, String> {
        self.with_client(|client| {
            let mut transaction = client.transaction().map_err(redacted_pg_error)?;
            let row = transaction
                .query_opt(
                    r#"SELECT queue_name, payload_json, idempotency_key, partition_key, priority,
                              available_at_ms, retention_until_ms, max_attempts, state,
                              attempt, fencing_token, lease_until_ms
                       FROM runtime_queue_messages WHERE message_id=$1
                       FOR UPDATE"#,
                    &[&delivery.message_id],
                )
                .map_err(redacted_pg_error)?
                .ok_or_else(|| "queue metadata is unavailable".to_string())?;
            let available_at_ms: i64 = row.get(5);
            let retention_until_ms: i64 = row.get(6);
            let max_attempts: i32 = row.get(7);
            let state: String = row.get(8);
            let attempt: i32 = row.get(9);
            let fencing_token: i64 = row.get(10);
            if state == "acked" || state == "dead" {
                transaction.commit().map_err(redacted_pg_error)?;
                return Ok(QueueClaimOutcome::Terminate);
            }
            if state == "leased" {
                let lease_until_ms = row.get::<_, Option<i64>>(11).unwrap_or(now_ms + 1);
                transaction.commit().map_err(redacted_pg_error)?;
                return Ok(QueueClaimOutcome::Defer(Duration::from_millis(
                    lease_until_ms.saturating_sub(now_ms).max(1) as u64,
                )));
            }
            if state != "pending" {
                transaction.commit().map_err(redacted_pg_error)?;
                return Ok(QueueClaimOutcome::Terminate);
            }
            if retention_until_ms <= now_ms {
                transaction
                    .execute(
                        r#"UPDATE runtime_queue_messages
                           SET state='dead', dead_letter_reason='retention_expired'
                           WHERE message_id=$1"#,
                        &[&delivery.message_id],
                    )
                    .map_err(redacted_pg_error)?;
                transaction.commit().map_err(redacted_pg_error)?;
                return Ok(QueueClaimOutcome::Terminate);
            }
            if available_at_ms > now_ms {
                transaction.commit().map_err(redacted_pg_error)?;
                return Ok(QueueClaimOutcome::Defer(Duration::from_millis(
                    available_at_ms.saturating_sub(now_ms) as u64,
                )));
            }
            let next_attempt = attempt + 1;
            if next_attempt > max_attempts {
                transaction
                    .execute(
                        r#"UPDATE runtime_queue_messages
                           SET state='dead', dead_letter_reason='max_attempts_exhausted'
                           WHERE message_id=$1"#,
                        &[&delivery.message_id],
                    )
                    .map_err(redacted_pg_error)?;
                transaction.commit().map_err(redacted_pg_error)?;
                return Ok(QueueClaimOutcome::Terminate);
            }
            let next_fencing_token = fencing_token + 1;
            let updated = transaction
                .execute(
                    r#"UPDATE runtime_queue_messages
                       SET state='leased', attempt=$2, lease_owner=$3, lease_until_ms=$4,
                           fencing_token=$5, delivery_reply_subject=$6,
                           last_stream_sequence=$7, last_consumer_sequence=$8
                       WHERE message_id=$1 AND state='pending'"#,
                    &[
                        &delivery.message_id,
                        &next_attempt,
                        &owner,
                        &(now_ms + visibility_timeout_ms),
                        &next_fencing_token,
                        &delivery.reply_subject,
                        &delivery.stream_sequence,
                        &delivery.consumer_sequence,
                    ],
                )
                .map_err(redacted_pg_error)?;
            if updated != 1 {
                transaction.commit().map_err(redacted_pg_error)?;
                return Ok(QueueClaimOutcome::Defer(Duration::from_millis(1)));
            }
            let claimed = QueueDelivery {
                message_id: delivery.message_id,
                queue: row.get(0),
                payload: serde_json::from_str::<Value>(&row.get::<_, String>(1))
                    .map_err(|error| error.to_string())?,
                idempotency_key: IdempotencyKey::new(row.get::<_, String>(2))?,
                partition_key: row.get(3),
                priority: row.get(4),
                attempt: next_attempt as u32,
                lease_until_ms: now_ms + visibility_timeout_ms,
                fencing_token: next_fencing_token,
            };
            transaction.commit().map_err(redacted_pg_error)?;
            Ok(QueueClaimOutcome::Claimed(claimed))
        })
    }

    fn begin_queue_acknowledgement(
        &self,
        message_id: &str,
        fencing_token: i64,
    ) -> Result<PendingQueueReply, String> {
        self.with_client(|client| {
            let row = client
                .query_opt(
                    r#"UPDATE runtime_queue_messages
                       SET state='acknowledging'
                       WHERE message_id=$1 AND state='leased' AND fencing_token=$2
                       RETURNING message_id, fencing_token, delivery_reply_subject, state,
                                 available_at_ms"#,
                    &[&message_id, &fencing_token],
                )
                .map_err(redacted_pg_error)?
                .ok_or_else(|| "queue fencing token is stale".to_string())?;
            pending_queue_reply_from_row(&row, epoch_ms())
        })
    }

    fn begin_queue_retry(
        &self,
        message_id: &str,
        fencing_token: i64,
        available_at_ms: i64,
        reason: &str,
    ) -> Result<PendingQueueReply, String> {
        self.with_client(|client| {
            let row = client
                .query_opt(
                    r#"UPDATE runtime_queue_messages
                       SET state=CASE
                             WHEN attempt >= max_attempts THEN 'terminating'
                             ELSE 'retrying'
                           END,
                           available_at_ms=$3,
                           dead_letter_reason=CASE
                             WHEN attempt >= max_attempts THEN $4
                             ELSE dead_letter_reason
                           END
                       WHERE message_id=$1 AND state='leased' AND fencing_token=$2
                       RETURNING message_id, fencing_token, delivery_reply_subject, state,
                                 available_at_ms"#,
                    &[&message_id, &fencing_token, &available_at_ms, &reason],
                )
                .map_err(redacted_pg_error)?
                .ok_or_else(|| "queue fencing token is stale".to_string())?;
            pending_queue_reply_from_row(&row, epoch_ms())
        })
    }

    fn finalize_queue_reply(&self, reply: &PendingQueueReply) -> Result<(), String> {
        let (transition_state, final_state) = match reply.kind {
            QueueReplyKind::Ack => ("acknowledging", "acked"),
            QueueReplyKind::Retry => ("retrying", "pending"),
            QueueReplyKind::Terminate => ("terminating", "dead"),
        };
        self.with_client(|client| {
            let changed = client
                .execute(
                    r#"UPDATE runtime_queue_messages
                       SET state=$3, lease_owner=NULL, lease_until_ms=NULL
                       WHERE message_id=$1 AND fencing_token=$2 AND state=$4"#,
                    &[
                        &reply.message_id,
                        &reply.fencing_token,
                        &final_state,
                        &transition_state,
                    ],
                )
                .map_err(redacted_pg_error)?;
            if changed == 1 {
                return Ok(());
            }
            let current = client
                .query_opt(
                    "SELECT state FROM runtime_queue_messages WHERE message_id=$1 AND fencing_token=$2",
                    &[&reply.message_id, &reply.fencing_token],
                )
                .map_err(redacted_pg_error)?
                .map(|row| row.get::<_, String>(0));
            if current.as_deref() == Some(final_state) {
                Ok(())
            } else {
                Err("queue reply fencing token is stale".to_string())
            }
        })
    }

    fn queue_dead_letters(&self, queue: &str, limit: usize) -> Result<Vec<QueueDelivery>, String> {
        self.with_client(|client| {
            let rows = client
                .query(
                    r#"SELECT message_id, queue_name, payload_json, idempotency_key, partition_key,
                              priority, attempt, lease_until_ms, fencing_token
                       FROM runtime_queue_messages
                       WHERE queue_name=$1 AND state='dead'
                       ORDER BY available_at_ms, message_id
                       LIMIT $2"#,
                    &[&queue, &(limit as i64)],
                )
                .map_err(redacted_pg_error)?;
            rows.into_iter().map(queue_delivery_from_row).collect()
        })
    }

    fn set_checkpoint(&self, consumer: &str, stream: &str, sequence: i64) -> Result<(), String> {
        self.with_client(|client| {
            client
                .execute(
                    r#"INSERT INTO runtime_event_checkpoints(consumer_name, stream_name, sequence_num)
                       VALUES ($1,$2,$3)
                       ON CONFLICT (consumer_name, stream_name) DO UPDATE
                       SET sequence_num=GREATEST(
                         runtime_event_checkpoints.sequence_num,
                         EXCLUDED.sequence_num
                       )"#,
                    &[&consumer, &stream, &sequence],
                )
                .map_err(redacted_pg_error)?;
            Ok(())
        })
    }

    fn checkpoint(&self, consumer: &str, stream: &str) -> Result<i64, String> {
        self.with_client(|client| {
            Ok(client
                .query_opt(
                    "SELECT sequence_num FROM runtime_event_checkpoints WHERE consumer_name=$1 AND stream_name=$2",
                    &[&consumer, &stream],
                )
                .map_err(redacted_pg_error)?
                .map(|row| row.get(0))
                .unwrap_or(0))
        })
    }

    fn upsert_event(
        &self,
        event: &EventAppend,
    ) -> Result<(EventRecord, Option<i64>, bool), String> {
        if event.stream.trim().is_empty() || event.event_type.trim().is_empty() {
            return Err("event stream and event type are required".to_string());
        }
        self.with_client(|client| {
            let mut transaction = client.transaction().map_err(redacted_pg_error)?;
            transaction
                .query_one("SELECT pg_advisory_xact_lock(hashtext($1))", &[&event.stream])
                .map_err(redacted_pg_error)?;
            let existing = transaction
                .query_opt(
                    r#"SELECT event_id, stream_name, sequence_num, event_type, aggregate_id,
                              payload_json, idempotency_key, occurred_at_ms, retention_until_ms,
                              published_at_ms
                       FROM runtime_events
                       WHERE stream_name=$1 AND idempotency_key=$2
                       FOR UPDATE"#,
                    &[&event.stream, &event.idempotency_key.as_str()],
                )
                .map_err(redacted_pg_error)?;
            let row = if let Some(row) = existing {
                row
            } else {
                let sequence: i64 = transaction
                    .query_one(
                        "SELECT COALESCE(MAX(sequence_num), 0) + 1 FROM runtime_events WHERE stream_name=$1",
                        &[&event.stream],
                    )
                    .map_err(redacted_pg_error)?
                    .get(0);
                let event_id = stable_id(
                    "event",
                    &format!("{}:{}", event.stream, event.idempotency_key.as_str()),
                );
                transaction
                    .execute(
                        r#"INSERT INTO runtime_events
                           (event_id, stream_name, sequence_num, event_type, aggregate_id,
                            payload_json, idempotency_key, occurred_at_ms, retention_until_ms)
                           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
                        &[
                            &event_id,
                            &event.stream,
                            &sequence,
                            &event.event_type,
                            &event.aggregate_id,
                            &serde_json::to_string(&event.payload)
                                .map_err(|error| error.to_string())?,
                            &event.idempotency_key.as_str(),
                            &event.occurred_at_ms,
                            &event.retention_until_ms,
                        ],
                    )
                    .map_err(redacted_pg_error)?;
                transaction
                    .query_one(
                        r#"SELECT event_id, stream_name, sequence_num, event_type, aggregate_id,
                                  payload_json, idempotency_key, occurred_at_ms,
                                  retention_until_ms, published_at_ms
                           FROM runtime_events WHERE event_id=$1"#,
                        &[&event_id],
                    )
                    .map_err(redacted_pg_error)?
            };
            let stored = postgres_event_from_row(&row)?;
            if stored.0.event_type != event.event_type
                || stored.0.aggregate_id != event.aggregate_id
                || stored.0.payload != event.payload
                || stored.0.occurred_at_ms != event.occurred_at_ms
                || stored.1 != event.retention_until_ms
            {
                return Err("event idempotency key was reused with a different request".to_string());
            }
            transaction.commit().map_err(redacted_pg_error)?;
            let should_publish = stored.2.is_none();
            Ok((stored.0, stored.1, should_publish))
        })
    }

    fn mark_event_published(&self, event_id: &str) -> Result<(), String> {
        self.with_client(|client| {
            let count = client
                .execute(
                    "UPDATE runtime_events SET published_at_ms=COALESCE(published_at_ms, $2) WHERE event_id=$1",
                    &[&event_id, &epoch_ms()],
                )
                .map_err(redacted_pg_error)?;
            if count == 1 {
                Ok(())
            } else {
                Err("event publication metadata is unavailable".to_string())
            }
        })
    }

    fn pending_events(&self, limit: usize) -> Result<Vec<(EventRecord, Option<i64>)>, String> {
        self.with_client(|client| {
            client
                .query(
                    r#"SELECT event_id, stream_name, sequence_num, event_type, aggregate_id,
                              payload_json, idempotency_key, occurred_at_ms,
                              retention_until_ms, published_at_ms
                       FROM runtime_events WHERE published_at_ms IS NULL
                       ORDER BY stream_name, sequence_num LIMIT $1"#,
                    &[&(limit as i64)],
                )
                .map_err(redacted_pg_error)?
                .into_iter()
                .map(|row| {
                    let (record, retention, _) = postgres_event_from_row(&row)?;
                    Ok((record, retention))
                })
                .collect()
        })
    }

    fn replay_events(
        &self,
        stream: &str,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<EventRecord>, String> {
        if stream.trim().is_empty() || after_sequence < 0 || limit == 0 {
            return Err("invalid event replay request".to_string());
        }
        self.with_client(|client| {
            client
                .query(
                    r#"SELECT event_id, stream_name, sequence_num, event_type, aggregate_id,
                              payload_json, idempotency_key, occurred_at_ms,
                              retention_until_ms, published_at_ms
                       FROM runtime_events
                       WHERE stream_name=$1 AND sequence_num > $2
                       ORDER BY sequence_num LIMIT $3"#,
                    &[&stream, &after_sequence, &(limit as i64)],
                )
                .map_err(redacted_pg_error)?
                .into_iter()
                .map(|row| postgres_event_from_row(&row).map(|stored| stored.0))
                .collect()
        })
    }
}

fn run_postgres_blocking<T: Send>(operation: impl FnOnce() -> T + Send) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(operation)
        }
        Ok(_) => std::thread::scope(|scope| {
            scope
                .spawn(operation)
                .join()
                .expect("scoped Postgres operation panicked")
        }),
        Err(_) => operation(),
    }
}

impl VersionedPort for SelfHostedPostgresState {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            self.descriptor(
                PortKind::Storage,
                &["storage.json", "storage.transactional", "storage.postgres"],
            ),
            self.descriptor(
                PortKind::Scheduler,
                &["schedule.create", "schedule.cancel", "schedule.claim"],
            ),
            self.descriptor(
                PortKind::Outbox,
                &["outbox.append", "outbox.claim", "outbox.publish"],
            ),
            self.descriptor(
                PortKind::Inbox,
                &["inbox.deduplicate", "inbox.signature_ref"],
            ),
            self.descriptor(
                PortKind::Lease,
                &["lease.acquire", "lease.renew", "lease.fence"],
            ),
            self.descriptor(
                PortKind::Identity,
                &["oidc.discovery", "oidc.session.resolve"],
            ),
            self.descriptor(PortKind::Evidence, &["evidence.append", "evidence.replay"]),
            self.descriptor(
                PortKind::Telemetry,
                &["telemetry.consent_gate", "telemetry.postgres"],
            ),
        ]
    }
}

impl StoragePort for SelfHostedPostgresState {
    fn adapter_name(&self) -> &'static str {
        "postgres"
    }

    fn put_json(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        validate_key(namespace, key)?;
        self.with_client(|client| {
            client
                .execute(
                    r#"INSERT INTO tradeassembly_kv(namespace, item_key, value_json, idempotency_key, authority_actor, updated_at_ms)
                       VALUES ($1,$2,$3,$4,$5,$6)
                       ON CONFLICT (namespace, item_key) DO UPDATE SET
                         value_json=EXCLUDED.value_json,
                         idempotency_key=EXCLUDED.idempotency_key,
                         authority_actor=EXCLUDED.authority_actor,
                         updated_at_ms=EXCLUDED.updated_at_ms"#,
                    &[
                        &namespace,
                        &key,
                        &serde_json::to_string(&value).map_err(|error| error.to_string())?,
                        &context.idempotency_key.as_str(),
                        &context.authority.actor,
                        &epoch_ms(),
                    ],
                )
                .map_err(redacted_pg_error)?;
            Ok(())
        })
    }

    fn put_json_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<ImmutablePutOutcome, String> {
        validate_key(namespace, key)?;
        let value_json = serde_json::to_string(&value).map_err(|error| error.to_string())?;
        self.with_client(|client| {
            let inserted = client
                .query_opt(
                    r#"INSERT INTO tradeassembly_kv(namespace, item_key, value_json, idempotency_key, authority_actor, updated_at_ms)
                       VALUES ($1,$2,$3,$4,$5,$6)
                       ON CONFLICT (namespace, item_key) DO NOTHING
                       RETURNING item_key"#,
                    &[
                        &namespace,
                        &key,
                        &value_json,
                        &context.idempotency_key.as_str(),
                        &context.authority.actor,
                        &epoch_ms(),
                    ],
                )
                .map_err(redacted_pg_error)?;
            if inserted.is_some() {
                return Ok(ImmutablePutOutcome::Created);
            }
            let existing = client
                .query_one(
                    "SELECT value_json FROM tradeassembly_kv WHERE namespace=$1 AND item_key=$2",
                    &[&namespace, &key],
                )
                .map_err(redacted_pg_error)?
                .get::<_, String>(0);
            let existing: Value = serde_json::from_str(&existing)
                .map_err(|_| "immutable_storage_existing_value_invalid".to_string())?;
            if existing != value {
                return Err("immutable_storage_conflict".to_string());
            }
            Ok(ImmutablePutOutcome::AlreadyPresent)
        })
    }

    fn compare_and_put_json(
        &self,
        namespace: &str,
        key: &str,
        expected: Value,
        replacement: Value,
        context: &SideEffectContext,
    ) -> Result<ComparePutOutcome, String> {
        validate_key(namespace, key)?;
        let expected_json = serde_json::to_string(&expected).map_err(|error| error.to_string())?;
        let replacement_json =
            serde_json::to_string(&replacement).map_err(|error| error.to_string())?;
        self.with_client(|client| {
            let updated = client
                .execute(
                    r#"UPDATE tradeassembly_kv
                       SET value_json=$3, idempotency_key=$4, authority_actor=$5, updated_at_ms=$6
                       WHERE namespace=$1 AND item_key=$2 AND value_json=$7"#,
                    &[
                        &namespace,
                        &key,
                        &replacement_json,
                        &context.idempotency_key.as_str(),
                        &context.authority.actor,
                        &epoch_ms(),
                        &expected_json,
                    ],
                )
                .map_err(redacted_pg_error)?;
            Ok(if updated == 1 {
                ComparePutOutcome::Updated
            } else {
                ComparePutOutcome::Conflict
            })
        })
    }

    fn put_json_batch(
        &self,
        writes: &[StorageWrite],
        expectations: &[StorageExpectation],
    ) -> Result<ComparePutOutcome, String> {
        for write in writes {
            validate_key(&write.namespace, &write.key)?;
        }
        for expectation in expectations {
            validate_key(&expectation.namespace, &expectation.key)?;
        }
        let serialized = writes
            .iter()
            .map(|write| {
                serde_json::to_string(&write.value)
                    .map(|value| (write, value))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let lock_ids = postgres_batch_lock_ids(expectations);
        self.with_client(|client| {
            let mut transaction = client.transaction().map_err(redacted_pg_error)?;
            for lock_id in lock_ids {
                transaction
                    .query_one("SELECT pg_advisory_xact_lock($1)", &[&lock_id])
                    .map_err(redacted_pg_error)?;
            }
            for expectation in expectations {
                let current = transaction
                    .query_opt(
                        "SELECT value_json FROM tradeassembly_kv WHERE namespace=$1 AND item_key=$2 FOR UPDATE",
                        &[&expectation.namespace, &expectation.key],
                    )
                    .map_err(redacted_pg_error)?
                    .map(|row| serde_json::from_str::<Value>(&row.get::<_, String>(0)))
                    .transpose()
                    .map_err(|_| "storage expectation value invalid".to_string())?;
                if current != expectation.value {
                    return Ok(ComparePutOutcome::Conflict);
                }
            }
            for (write, value_json) in serialized {
                transaction
                    .execute(
                        r#"INSERT INTO tradeassembly_kv(namespace, item_key, value_json, idempotency_key, authority_actor, updated_at_ms)
                           VALUES ($1,$2,$3,$4,$5,$6)
                           ON CONFLICT (namespace, item_key) DO UPDATE SET
                             value_json=EXCLUDED.value_json,
                             idempotency_key=EXCLUDED.idempotency_key,
                             authority_actor=EXCLUDED.authority_actor,
                             updated_at_ms=EXCLUDED.updated_at_ms"#,
                        &[
                            &write.namespace,
                            &write.key,
                            &value_json,
                            &write.context.idempotency_key.as_str(),
                            &write.context.authority.actor,
                            &epoch_ms(),
                        ],
                    )
                    .map_err(redacted_pg_error)?;
            }
            transaction.commit().map_err(redacted_pg_error)?;
            Ok(ComparePutOutcome::Updated)
        })
    }

    fn get_json(&self, namespace: &str, key: &str) -> Result<Option<Value>, String> {
        validate_key(namespace, key)?;
        self.with_client(|client| {
            client
                .query_opt(
                    "SELECT value_json FROM tradeassembly_kv WHERE namespace=$1 AND item_key=$2",
                    &[&namespace, &key],
                )
                .map_err(redacted_pg_error)?
                .map(|row| {
                    serde_json::from_str::<Value>(&row.get::<_, String>(0))
                        .map_err(|error| error.to_string())
                })
                .transpose()
        })
    }

    fn list_json(&self, namespace: &str) -> Result<Vec<(String, Value)>, String> {
        if namespace.trim().is_empty() {
            return Err("storage namespace is required".to_string());
        }
        self.with_client(|client| {
            client
                .query(
                    "SELECT item_key, value_json FROM tradeassembly_kv WHERE namespace=$1 ORDER BY item_key",
                    &[&namespace],
                )
                .map_err(redacted_pg_error)?
                .into_iter()
                .map(|row| {
                    Ok((
                        row.get::<_, String>(0),
                        serde_json::from_str::<Value>(&row.get::<_, String>(1))
                            .map_err(|error| error.to_string())?,
                    ))
                })
                .collect()
        })
    }

    fn list_json_page(
        &self,
        namespace: &str,
        after_key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Value)>, String> {
        if namespace.trim().is_empty() {
            return Err("storage namespace is required".to_string());
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit =
            i64::try_from(limit).map_err(|_| "storage page limit is invalid".to_string())?;
        let after_key = after_key.map(str::to_string);
        self.with_client(|client| {
            client
                .query(
                    "SELECT item_key, value_json FROM tradeassembly_kv \
                     WHERE namespace=$1 AND ($2::text IS NULL OR item_key > $2) \
                     ORDER BY item_key LIMIT $3",
                    &[&namespace, &after_key, &limit],
                )
                .map_err(redacted_pg_error)?
                .into_iter()
                .map(|row| {
                    Ok((
                        row.get::<_, String>(0),
                        serde_json::from_str::<Value>(&row.get::<_, String>(1))
                            .map_err(|error| error.to_string())?,
                    ))
                })
                .collect()
        })
    }

    fn clear_namespace(&self, namespace: &str) -> Result<(), String> {
        if namespace.trim().is_empty() {
            return Err("storage namespace is required".to_string());
        }
        self.with_client(|client| {
            client
                .execute(
                    "DELETE FROM tradeassembly_kv WHERE namespace=$1",
                    &[&namespace],
                )
                .map_err(redacted_pg_error)?;
            Ok(())
        })
    }
}

impl JournalPort for SelfHostedPostgresState {
    fn record(&self, event: JournalEvent) -> Result<String, String> {
        let mut canonical = serde_json::json!({
            "event_type": event.event_type.clone(),
            "authority": event.authority.clone(),
            "idempotency_key": event.idempotency_key.as_str(),
            "payload": event.payload.clone(),
            "owner": event.owner.clone(),
        });
        if event.owner.is_none() {
            canonical
                .as_object_mut()
                .expect("journal canonical envelope is an object")
                .remove("owner");
        }
        let journal_id = format!("journal-{}", canonical_hash(&canonical));
        let legacy_id = stable_id(
            "journal",
            &format!("{}:{}", event.event_type, event.idempotency_key.as_str()),
        );
        let owner_json = event
            .owner
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?;
        self.with_client(|client| {
            if event.owner.is_none() {
                if let Some(row) = client
                    .query_opt(
                        "SELECT event_type, authority_json, idempotency_key, payload_json, owner_json FROM runtime_journal WHERE journal_id=$1",
                        &[&legacy_id],
                    )
                    .map_err(redacted_pg_error)?
                {
                    let same = row.get::<_, String>(0) == event.event_type
                        && row.get::<_, String>(1)
                            == serde_json::to_string(&event.authority).map_err(|error| error.to_string())?
                        && row.get::<_, String>(2) == event.idempotency_key.as_str()
                        && row.get::<_, String>(3)
                            == serde_json::to_string(&event.payload).map_err(|error| error.to_string())?
                        && row.get::<_, Option<String>>(4).is_none();
                    if same {
                        return Ok(legacy_id.clone());
                    }
                }
            }
            client
                .execute(
                    r#"INSERT INTO runtime_journal(journal_id, event_type, authority_json, idempotency_key, payload_json, recorded_at_ms, owner_json)
                       VALUES ($1,$2,$3,$4,$5,$6,$7)
                       ON CONFLICT (journal_id) DO NOTHING"#,
                    &[
                        &journal_id,
                        &event.event_type,
                        &serde_json::to_string(&event.authority).map_err(|error| error.to_string())?,
                        &event.idempotency_key.as_str(),
                        &serde_json::to_string(&event.payload).map_err(|error| error.to_string())?,
                        &epoch_ms(),
                        &owner_json,
                    ],
                )
                .map_err(redacted_pg_error)?;
            Ok(journal_id)
        })
    }

    fn try_events(&self) -> Result<Vec<JournalEvent>, String> {
        self.with_client(|client| {
            client
                .query(
                    "SELECT event_type, authority_json, idempotency_key, payload_json, owner_json FROM runtime_journal ORDER BY recorded_at_ms, journal_id",
                    &[],
                )
                .map_err(redacted_pg_error)?
                .into_iter()
                .map(|row| {
                    Ok(JournalEvent {
                        event_type: row.get(0),
                        authority: serde_json::from_str(&row.get::<_, String>(1))
                            .map_err(|error| error.to_string())?,
                        idempotency_key: IdempotencyKey::new(row.get::<_, String>(2))?,
                        payload: serde_json::from_str(&row.get::<_, String>(3))
                            .map_err(|error| error.to_string())?,
                        owner: row
                            .get::<_, Option<String>>(4)
                            .map(|owner| serde_json::from_str(&owner))
                            .transpose()
                            .map_err(|error| error.to_string())?,
                    })
                })
                .collect()
        })
    }

    fn events(&self) -> Vec<JournalEvent> {
        self.try_events()
            .unwrap_or_else(|_| panic!("journal read failed"))
    }
}

impl SchedulerPort for SelfHostedPostgresState {
    fn schedule(&self, work: ScheduledWork) -> Result<(), String> {
        self.with_client(|client| {
            client.execute(
                r#"INSERT INTO runtime_schedules(schedule_key, queue_name, payload_json, due_at_ms, idempotency_key)
                   VALUES ($1,$2,$3,$4,$5)
                   ON CONFLICT (schedule_key) DO NOTHING"#,
                &[
                    &work.schedule_key,
                    &work.queue,
                    &serde_json::to_string(&work.payload).map_err(|error| error.to_string())?,
                    &work.due_at_ms,
                    &work.idempotency_key.as_str(),
                ],
            ).map_err(redacted_pg_error)?;
            Ok(())
        })
    }

    fn claim_key(
        &self,
        schedule_key: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<ScheduledWork>, String> {
        self.with_client(|client| {
            client
                .query_opt(
                    r#"UPDATE runtime_schedules
                       SET state='leased', lease_owner=$2, lease_until_ms=$3,
                           fencing_token=fencing_token+1
                       WHERE schedule_key=$1 AND due_at_ms <= $4
                         AND (state='pending' OR (state='leased' AND lease_until_ms <= $4))
                       RETURNING schedule_key, queue_name, payload_json, due_at_ms,
                                 idempotency_key, fencing_token"#,
                    &[&schedule_key, &owner, &(now_ms + lease_ms), &now_ms],
                )
                .map_err(redacted_pg_error)?
                .map(|row| {
                    Ok(ScheduledWork {
                        schedule_key: row.get(0),
                        queue: row.get(1),
                        payload: serde_json::from_str(&row.get::<_, String>(2))
                            .map_err(|error| error.to_string())?,
                        due_at_ms: row.get(3),
                        idempotency_key: IdempotencyKey::new(row.get::<_, String>(4))?,
                        fencing_token: row.get(5),
                    })
                })
                .transpose()
        })
    }

    fn cancel(&self, schedule_key: &str) -> Result<bool, String> {
        self.with_client(|client| {
            client
                .execute(
                    "DELETE FROM runtime_schedules WHERE schedule_key=$1",
                    &[&schedule_key],
                )
                .map(|count| count > 0)
                .map_err(redacted_pg_error)
        })
    }

    fn claim_due(
        &self,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
        limit: usize,
    ) -> Result<Vec<ScheduledWork>, String> {
        self.with_client(|client| {
            let rows = client
                .query(
                    r#"WITH claimable AS (
                         SELECT schedule_key
                         FROM runtime_schedules
                         WHERE due_at_ms <= $1
                           AND (state='pending' OR (state='leased' AND lease_until_ms <= $1))
                         ORDER BY due_at_ms, schedule_key
                         FOR UPDATE SKIP LOCKED
                         LIMIT $2
                       )
                       UPDATE runtime_schedules AS schedule
                       SET state='leased', lease_owner=$3, lease_until_ms=$4,
                           fencing_token=schedule.fencing_token+1
                       FROM claimable
                       WHERE schedule.schedule_key=claimable.schedule_key
                       RETURNING schedule.schedule_key, schedule.queue_name, schedule.payload_json,
                                 schedule.due_at_ms, schedule.idempotency_key,
                                 schedule.fencing_token"#,
                    &[&now_ms, &(limit as i64), &owner, &(now_ms + lease_ms)],
                )
                .map_err(redacted_pg_error)?;
            rows.into_iter()
                .map(|row| {
                    Ok(ScheduledWork {
                        schedule_key: row.get(0),
                        queue: row.get(1),
                        payload: serde_json::from_str(&row.get::<_, String>(2))
                            .map_err(|error| error.to_string())?,
                        due_at_ms: row.get(3),
                        idempotency_key: IdempotencyKey::new(row.get::<_, String>(4))?,
                        fencing_token: row.get(5),
                    })
                })
                .collect()
        })
    }

    fn complete(
        &self,
        schedule_key: &str,
        owner: &str,
        fencing_token: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        self.with_client(|client| {
            let count = client
                .execute(
                    "DELETE FROM runtime_schedules WHERE schedule_key=$1 AND state='leased' AND lease_owner=$2 AND fencing_token=$3 AND lease_until_ms > $4",
                    &[&schedule_key, &owner, &fencing_token, &now_ms],
                )
                .map_err(redacted_pg_error)?;
            if count == 1 {
                Ok(())
            } else {
                Err("scheduled work ownership is stale".to_string())
            }
        })
    }
}

impl OutboxRepository for SelfHostedPostgresState {
    fn append(&self, record: OutboxRecord) -> Result<(), String> {
        self.with_client(|client| {
            client.execute(
                r#"INSERT INTO runtime_outbox(outbox_id, topic, payload_json, idempotency_key, created_at_ms)
                   VALUES ($1,$2,$3,$4,$5)
                   ON CONFLICT (outbox_id) DO NOTHING"#,
                &[
                    &record.outbox_id,
                    &record.topic,
                    &serde_json::to_string(&record.payload).map_err(|error| error.to_string())?,
                    &record.idempotency_key.as_str(),
                    &record.created_at_ms,
                ],
            ).map_err(redacted_pg_error)?;
            Ok(())
        })
    }

    fn claim_pending(
        &self,
        owner: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<OutboxRecord>, String> {
        self.with_client(|client| {
            let rows = client
                .query(
                    r#"WITH claimable AS (
                         SELECT outbox_id
                         FROM runtime_outbox
                         WHERE state='pending' OR (state='leased' AND lease_until_ms <= $1)
                         ORDER BY created_at_ms, outbox_id
                         FOR UPDATE SKIP LOCKED
                         LIMIT $2
                       )
                       UPDATE runtime_outbox AS outbox
                       SET state='leased', lease_owner=$3, lease_until_ms=$4,
                           fencing_token=outbox.fencing_token+1
                       FROM claimable
                       WHERE outbox.outbox_id=claimable.outbox_id
                       RETURNING outbox.outbox_id, outbox.topic, outbox.payload_json,
                                 outbox.idempotency_key, outbox.created_at_ms,
                                 outbox.fencing_token"#,
                    &[&now_ms, &(limit as i64), &owner, &(now_ms + 30_000)],
                )
                .map_err(redacted_pg_error)?;
            rows.into_iter()
                .map(|row| {
                    Ok(OutboxRecord {
                        outbox_id: row.get(0),
                        topic: row.get(1),
                        payload: serde_json::from_str(&row.get::<_, String>(2))
                            .map_err(|error| error.to_string())?,
                        idempotency_key: IdempotencyKey::new(row.get::<_, String>(3))?,
                        created_at_ms: row.get(4),
                        fencing_token: row.get(5),
                    })
                })
                .collect()
        })
    }

    fn mark_published(
        &self,
        outbox_id: &str,
        owner: &str,
        fencing_token: i64,
    ) -> Result<(), String> {
        self.with_client(|client| {
            let count = client
                .execute(
                    "DELETE FROM runtime_outbox WHERE outbox_id=$1 AND lease_owner=$2 AND fencing_token=$3",
                    &[&outbox_id, &owner, &fencing_token],
                )
                .map_err(redacted_pg_error)?;
            if count == 1 {
                Ok(())
            } else {
                Err("outbox ownership is stale".to_string())
            }
        })
    }
}

impl InboxRepository for SelfHostedPostgresState {
    fn record_once(
        &self,
        source: &str,
        message_id: &str,
        signature_ref: Option<&str>,
        received_at_ms: i64,
    ) -> Result<bool, String> {
        self.with_client(|client| {
            client
                .execute(
                    r#"INSERT INTO runtime_inbox(source, message_id, signature_ref, received_at_ms)
                       VALUES ($1,$2,$3,$4)
                       ON CONFLICT (source, message_id) DO NOTHING"#,
                    &[&source, &message_id, &signature_ref, &received_at_ms],
                )
                .map(|count| count == 1)
                .map_err(redacted_pg_error)
        })
    }
}

impl LeaseRepository for SelfHostedPostgresState {
    fn acquire(
        &self,
        resource: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<LeaseClaim>, String> {
        self.with_client(|client| {
            client
                .query_opt(
                    r#"INSERT INTO runtime_leases(resource, owner, fencing_token, expires_at_ms)
                       VALUES ($1,$2,1,$4)
                       ON CONFLICT(resource) DO UPDATE SET
                         owner=EXCLUDED.owner,
                         fencing_token=runtime_leases.fencing_token+1,
                         expires_at_ms=EXCLUDED.expires_at_ms
                       WHERE runtime_leases.expires_at_ms <= $3
                       RETURNING fencing_token, expires_at_ms"#,
                    &[&resource, &owner, &now_ms, &(now_ms + lease_ms)],
                )
                .map_err(redacted_pg_error)
                .map(|row| {
                    row.map(|row| LeaseClaim {
                        resource: resource.to_string(),
                        owner: owner.to_string(),
                        fencing_token: row.get(0),
                        expires_at_ms: row.get(1),
                    })
                })
        })
    }

    fn renew(&self, claim: &LeaseClaim, now_ms: i64, lease_ms: i64) -> Result<LeaseClaim, String> {
        self.with_client(|client| {
            let count = client.execute(
                "UPDATE runtime_leases SET expires_at_ms=$4 WHERE resource=$1 AND owner=$2 AND fencing_token=$3",
                &[&claim.resource, &claim.owner, &claim.fencing_token, &(now_ms + lease_ms)],
            ).map_err(redacted_pg_error)?;
            if count == 1 {
                Ok(LeaseClaim {
                    expires_at_ms: now_ms + lease_ms,
                    ..claim.clone()
                })
            } else {
                Err("lease renewal is stale".to_string())
            }
        })
    }

    fn release(&self, claim: &LeaseClaim) -> Result<(), String> {
        self.with_client(|client| {
            let count = client.execute(
                "DELETE FROM runtime_leases WHERE resource=$1 AND owner=$2 AND fencing_token=$3",
                &[&claim.resource, &claim.owner, &claim.fencing_token],
            ).map_err(redacted_pg_error)?;
            if count == 1 {
                Ok(())
            } else {
                Err("lease release is stale".to_string())
            }
        })
    }
}

impl EvidencePort for SelfHostedPostgresState {
    fn append(&self, record: EvidenceRecord) -> Result<(), String> {
        self.with_client(|client| {
            client.execute(
                r#"INSERT INTO runtime_evidence(evidence_id, evidence_type, aggregate_id, payload_json, idempotency_key, recorded_at_ms)
                   VALUES ($1,$2,$3,$4,$5,$6)
                   ON CONFLICT (evidence_id) DO NOTHING"#,
                &[
                    &record.evidence_id,
                    &record.evidence_type,
                    &record.aggregate_id,
                    &serde_json::to_string(&record.payload).map_err(|error| error.to_string())?,
                    &record.idempotency_key.as_str(),
                    &record.recorded_at_ms,
                ],
            ).map_err(redacted_pg_error)?;
            Ok(())
        })
    }

    fn list(&self, aggregate_id: &str) -> Result<Vec<EvidenceRecord>, String> {
        self.with_client(|client| {
            client
                .query(
                    r#"SELECT evidence_id, evidence_type, aggregate_id, payload_json, idempotency_key, recorded_at_ms
                       FROM runtime_evidence WHERE aggregate_id=$1 ORDER BY recorded_at_ms, evidence_id"#,
                    &[&aggregate_id],
                )
                .map_err(redacted_pg_error)?
                .into_iter()
                .map(|row| {
                    Ok(EvidenceRecord {
                        evidence_id: row.get(0),
                        evidence_type: row.get(1),
                        aggregate_id: row.get(2),
                        payload: serde_json::from_str(&row.get::<_, String>(3))
                            .map_err(|error| error.to_string())?,
                        idempotency_key: IdempotencyKey::new(row.get::<_, String>(4))?,
                        recorded_at_ms: row.get(5),
                    })
                })
                .collect()
        })
    }
}

impl TelemetryPort for SelfHostedPostgresState {
    fn emit(
        &self,
        event_type: &str,
        fields: Value,
        idempotency_key: &IdempotencyKey,
    ) -> Result<bool, String> {
        if !self.telemetry_enabled {
            return Ok(false);
        }
        self.with_client(|client| {
            client.execute(
                r#"INSERT INTO runtime_telemetry(event_id, event_type, fields_json, idempotency_key, created_at_ms)
                   VALUES ($1,$2,$3,$4,$5)
                   ON CONFLICT (idempotency_key) DO NOTHING"#,
                &[
                    &stable_id("telemetry", idempotency_key.as_str()),
                    &event_type,
                    &serde_json::to_string(&fields).map_err(|error| error.to_string())?,
                    &idempotency_key.as_str(),
                    &epoch_ms(),
                ],
            ).map(|count| count == 1).map_err(redacted_pg_error)
        })
    }
}

impl IdentityPort for SelfHostedPostgresState {
    fn store_verified_session(
        &self,
        session_ref: &str,
        claims: &IdentityClaims,
    ) -> Result<(), String> {
        if claims.issuer != self.oidc_issuer || !claims.audience.contains(&self.oidc_audience) {
            return Err(
                "verified OIDC claims do not match configured issuer and audience".to_string(),
            );
        }
        let audience_json =
            serde_json::to_string(&claims.audience).map_err(|error| error.to_string())?;
        self.with_client(|client| {
            client
                .execute(
                    r#"INSERT INTO oidc_sessions(session_ref, issuer, subject, audience_json, assurance, expires_at_ms)
                       VALUES ($1, $2, $3, $4, $5, $6)
                       ON CONFLICT(session_ref) DO UPDATE SET issuer=EXCLUDED.issuer,
                         subject=EXCLUDED.subject, audience_json=EXCLUDED.audience_json,
                         assurance=EXCLUDED.assurance, expires_at_ms=EXCLUDED.expires_at_ms"#,
                    &[
                        &session_ref,
                        &claims.issuer,
                        &claims.subject,
                        &audience_json,
                        &claims.assurance,
                        &claims.expires_at_ms,
                    ],
                )
                .map_err(redacted_pg_error)
        })?;
        Ok(())
    }

    fn authenticate_studio(
        &self,
        bearer_credential: &str,
        session: &TrustedStudioSession,
        now_ms: i64,
    ) -> Result<IdentityClaims, String> {
        self.trusted_studio
            .authenticate(bearer_credential, session, now_ms)
    }

    fn resolve(&self, session_ref: &str, now_ms: i64) -> Result<IdentityClaims, String> {
        let claims = self.with_client(|client| {
            client.query_opt(
                "SELECT issuer, subject, audience_json, assurance, expires_at_ms FROM oidc_sessions WHERE session_ref=$1",
                &[&session_ref],
            ).map_err(redacted_pg_error)
        })?.ok_or_else(|| "OIDC session is unavailable".to_string())?;
        let claims = IdentityClaims {
            issuer: claims.get(0),
            subject: claims.get(1),
            audience: serde_json::from_str(&claims.get::<_, String>(2))
                .map_err(|error| error.to_string())?,
            assurance: claims.get(3),
            expires_at_ms: claims.get(4),
        };
        if claims.issuer != self.oidc_issuer
            || !claims.audience.contains(&self.oidc_audience)
            || claims.expires_at_ms <= now_ms
        {
            return Err(
                "OIDC session is expired or does not match configured authority".to_string(),
            );
        }
        Ok(claims)
    }
}

#[derive(Clone)]
struct JetStreamDelivery {
    message_id: String,
    reply_subject: String,
    stream_sequence: i64,
    consumer_sequence: i64,
}

enum QueueClaimOutcome {
    Claimed(QueueDelivery),
    Defer(Duration),
    Terminate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueReplyKind {
    Ack,
    Retry,
    Terminate,
}

struct PendingQueueReply {
    message_id: String,
    fencing_token: i64,
    reply_subject: String,
    kind: QueueReplyKind,
    delay_ms: i64,
}

enum JetStreamCommand {
    PublishQueue {
        subject: String,
        message_id: String,
        payload: Vec<u8>,
        response: mpsc::Sender<Result<(), String>>,
    },
    FetchQueue {
        queue: String,
        subject: String,
        limit: usize,
        response: mpsc::Sender<Result<Vec<JetStreamDelivery>, String>>,
    },
    AckReply {
        reply_subject: String,
        kind: AckKind,
        response: mpsc::Sender<Result<(), String>>,
    },
    PublishEvent {
        subject: String,
        message_id: String,
        payload: Vec<u8>,
        response: mpsc::Sender<Result<i64, String>>,
    },
    PublishBus {
        subject: String,
        message_id: String,
        payload: Vec<u8>,
        response: mpsc::Sender<Result<(), String>>,
    },
}

struct SelfHostedJetStream {
    commands: mpsc::Sender<JetStreamCommand>,
    state: Arc<SelfHostedPostgresState>,
    published: Mutex<Vec<(String, Value)>>,
}

impl SelfHostedJetStream {
    fn new(nats_url: &str, state: Arc<SelfHostedPostgresState>) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<JetStreamCommand>();
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        let nats_url = nats_url.to_string();
        thread::Builder::new()
            .name("tradeassembly-self-hosted-jetstream".to_string())
            .spawn(move || jetstream_loop(&nats_url, rx, startup_tx))
            .map_err(|_| "JetStream worker thread failed to start".to_string())?;
        startup_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "JetStream startup timed out".to_string())??;
        let adapter = Self {
            commands: tx,
            state,
            published: Mutex::new(Vec::new()),
        };
        adapter.recover_pending_events()?;
        Ok(adapter)
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<T, String>>) -> JetStreamCommand,
    ) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        self.commands
            .send(build(tx))
            .map_err(|_| "JetStream command channel is unavailable".to_string())?;
        rx.recv()
            .map_err(|_| "JetStream command response channel is unavailable".to_string())?
    }

    fn publish_event_record(
        &self,
        record: &EventRecord,
        retention_until_ms: Option<i64>,
    ) -> Result<(), String> {
        let payload = serde_json::to_vec(&json!({
            "eventType": record.event_type,
            "aggregateId": record.aggregate_id,
            "payload": record.payload,
            "idempotencyKey": record.idempotency_key.as_str(),
            "occurredAtMs": record.occurred_at_ms,
            "retentionUntilMs": retention_until_ms,
        }))
        .map_err(|error| error.to_string())?;
        self.request(|response| JetStreamCommand::PublishEvent {
            subject: self.state.event_subject(&record.stream),
            message_id: record.event_id.clone(),
            payload,
            response,
        })?;
        self.state.mark_event_published(&record.event_id)
    }

    fn recover_pending_events(&self) -> Result<(), String> {
        loop {
            let pending = self.state.pending_events(100)?;
            if pending.is_empty() {
                return Ok(());
            }
            for (record, retention_until_ms) in pending {
                self.publish_event_record(&record, retention_until_ms)?;
            }
        }
    }

    fn send_queue_reply(&self, reply: &PendingQueueReply) -> Result<(), String> {
        let kind = match reply.kind {
            QueueReplyKind::Ack => AckKind::Ack,
            QueueReplyKind::Retry if reply.delay_ms > 0 => {
                AckKind::Nak(Some(Duration::from_millis(reply.delay_ms as u64)))
            }
            QueueReplyKind::Retry => AckKind::Nak(None),
            QueueReplyKind::Terminate => AckKind::Term,
        };
        self.request(|response| JetStreamCommand::AckReply {
            reply_subject: reply.reply_subject.clone(),
            kind,
            response,
        })?;
        self.state.finalize_queue_reply(reply)
    }

    fn recover_queue_replies(&self, queue: &str, now_ms: i64) -> Result<(), String> {
        self.state.stage_expired_queue_replies(queue, now_ms)?;
        for reply in self.state.pending_queue_replies(queue, now_ms)? {
            self.send_queue_reply(&reply)?;
        }
        Ok(())
    }
}

impl VersionedPort for SelfHostedJetStream {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::DurableQueue, "self_hosted.nats-jetstream")
                .for_profiles(&["self_hosted"])
                .with_capabilities(&[
                    "queue.lease",
                    "queue.retry",
                    "queue.dead_letter",
                    "queue.at_least_once",
                ]),
            PortDescriptor::new(PortKind::EventStream, "self_hosted.nats-jetstream")
                .for_profiles(&["self_hosted"])
                .with_capabilities(&[
                    "event.append",
                    "event.replay",
                    "event.checkpoint",
                    "event.at_least_once",
                ]),
            PortDescriptor::new(PortKind::EventStream, "self_hosted.event-notifier")
                .for_profiles(&["self_hosted"])
                .with_capabilities(&["event.notify"]),
        ]
    }
}

impl DurableQueuePort for SelfHostedJetStream {
    fn enqueue(&self, request: QueueRequest) -> Result<String, String> {
        let (message_id, should_publish) = self.state.upsert_queue_metadata(&request)?;
        if !should_publish {
            return Ok(message_id);
        }
        let payload = serde_json::to_vec(&json!({
            "messageId": message_id,
            "queue": request.queue,
            "payload": request.payload,
            "idempotencyKey": request.idempotency_key.as_str(),
            "partitionKey": request.partition_key,
            "priority": request.priority,
            "availableAtMs": request.available_at_ms,
            "retentionUntilMs": request.retention_until_ms,
            "maxAttempts": request.max_attempts,
        }))
        .map_err(|error| error.to_string())?;
        self.request(|response| JetStreamCommand::PublishQueue {
            subject: self.state.queue_subject(&request.queue),
            message_id: message_id.clone(),
            payload,
            response,
        })?;
        self.state.mark_queue_published(&message_id)?;
        Ok(message_id)
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
            || limit == 0
            || visibility_timeout_ms <= 0
        {
            return Err("invalid durable queue claim".to_string());
        }
        self.recover_queue_replies(queue, now_ms)?;
        let deliveries = self.request(|response| JetStreamCommand::FetchQueue {
            queue: queue.to_string(),
            subject: self.state.queue_subject(queue),
            limit,
            response,
        })?;
        let mut claimed = Vec::new();
        for delivery in deliveries {
            match self.state.claim_queue_delivery(
                owner,
                now_ms,
                visibility_timeout_ms,
                delivery.clone(),
            )? {
                QueueClaimOutcome::Claimed(queue_delivery) => claimed.push(queue_delivery),
                QueueClaimOutcome::Defer(delay) => {
                    self.request(|response| JetStreamCommand::AckReply {
                        reply_subject: delivery.reply_subject.clone(),
                        kind: AckKind::Nak(Some(delay)),
                        response,
                    })?;
                }
                QueueClaimOutcome::Terminate => {
                    self.request(|response| JetStreamCommand::AckReply {
                        reply_subject: delivery.reply_subject.clone(),
                        kind: AckKind::Term,
                        response,
                    })?;
                }
            }
        }
        Ok(claimed)
    }

    fn acknowledge(&self, message_id: &str, fencing_token: i64) -> Result<(), String> {
        let reply = self
            .state
            .begin_queue_acknowledgement(message_id, fencing_token)?;
        self.send_queue_reply(&reply)
    }

    fn retry(
        &self,
        message_id: &str,
        fencing_token: i64,
        available_at_ms: i64,
        reason: &str,
    ) -> Result<(), String> {
        let reply =
            self.state
                .begin_queue_retry(message_id, fencing_token, available_at_ms, reason)?;
        self.send_queue_reply(&reply)
    }

    fn dead_letters(&self, queue: &str, limit: usize) -> Result<Vec<QueueDelivery>, String> {
        self.state.queue_dead_letters(queue, limit)
    }
}

impl EventStreamPort for SelfHostedJetStream {
    fn append(&self, event: EventAppend) -> Result<EventRecord, String> {
        let (record, retention_until_ms, should_publish) = self.state.upsert_event(&event)?;
        if should_publish {
            self.publish_event_record(&record, retention_until_ms)?;
        }
        Ok(record)
    }

    fn replay(
        &self,
        stream: &str,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<EventRecord>, String> {
        self.state.replay_events(stream, after_sequence, limit)
    }

    fn checkpoint(&self, consumer: &str, stream: &str, sequence: i64) -> Result<(), String> {
        self.state.set_checkpoint(consumer, stream, sequence)
    }

    fn consumer_checkpoint(&self, consumer: &str, stream: &str) -> Result<i64, String> {
        self.state.checkpoint(consumer, stream)
    }
}

impl EventBusPort for SelfHostedJetStream {
    fn publish(
        &self,
        topic: &str,
        payload: Value,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        if let Ok(mut published) = self.published.lock() {
            published.push((topic.to_string(), payload.clone()));
        }
        self.request(|response| JetStreamCommand::PublishBus {
            subject: self.state.bus_subject(topic),
            message_id: stable_id("notify", context.idempotency_key.as_str()),
            payload: serde_json::to_vec(&payload)
                .map_err(|error| error.to_string())
                .unwrap_or_default(),
            response,
        })
    }

    fn published(&self) -> Vec<(String, Value)> {
        self.published
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default()
    }
}

fn jetstream_loop(
    nats_url: &str,
    receiver: mpsc::Receiver<JetStreamCommand>,
    startup: mpsc::SyncSender<Result<(), String>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = startup.send(Err("JetStream runtime initialization failed".to_string()));
            return;
        }
    };
    runtime.block_on(async move {
        let client = match async_nats::connect(nats_url).await {
            Ok(client) => client,
            Err(_) => {
                let _ = startup.send(Err("JetStream connection failed".to_string()));
                return;
            }
        };
        let context = jetstream::new(client.clone());
        let mut streams = HashSet::new();
        for (name, subjects, allow_direct) in [
            (QUEUE_STREAM, vec!["queue.>".to_string()], false),
            (EVENT_STREAM, vec!["event.>".to_string()], true),
            (BUS_STREAM, vec!["notify.>".to_string()], false),
        ] {
            if ensure_stream(&context, &mut streams, name, subjects, allow_direct)
                .await
                .is_err()
            {
                let _ = startup.send(Err("JetStream stream initialization failed".to_string()));
                return;
            }
        }
        if startup.send(Ok(())).is_err() {
            return;
        }
        while let Ok(command) = receiver.recv() {
            match command {
                JetStreamCommand::PublishQueue {
                    subject,
                    message_id,
                    payload,
                    response,
                } => {
                    let result = if let Err(error) = ensure_stream(
                        &context,
                        &mut streams,
                        QUEUE_STREAM,
                        vec!["queue.>".to_string()],
                        false,
                    )
                    .await
                    {
                        Err(error)
                    } else {
                        publish_with_id(&context, &subject, &message_id, payload)
                            .await
                            .map_err(js_error)
                    };
                    let _ = response.send(result.map(|_| ()));
                }
                JetStreamCommand::FetchQueue {
                    queue,
                    subject,
                    limit,
                    response,
                } => {
                    let consumer_name = format!("queue_{}", nats_component(&queue));
                    let result = if let Err(error) = ensure_stream(
                        &context,
                        &mut streams,
                        QUEUE_STREAM,
                        vec!["queue.>".to_string()],
                        false,
                    )
                    .await
                    {
                        Err(error)
                    } else {
                        fetch_queue_deliveries(&context, &consumer_name, &subject, limit).await
                    };
                    let _ = response.send(result);
                }
                JetStreamCommand::AckReply {
                    reply_subject,
                    kind,
                    response,
                } => {
                    let result = client
                        .publish(reply_subject, kind.into())
                        .await
                        .map_err(js_error)
                        .map(|_| ());
                    let _ = response.send(result);
                }
                JetStreamCommand::PublishEvent {
                    subject,
                    message_id,
                    payload,
                    response,
                } => {
                    let result = if let Err(error) = ensure_stream(
                        &context,
                        &mut streams,
                        EVENT_STREAM,
                        vec!["event.>".to_string()],
                        true,
                    )
                    .await
                    {
                        Err(error)
                    } else {
                        publish_with_id(&context, &subject, &message_id, payload)
                            .await
                            .map(|ack| ack.sequence as i64)
                            .map_err(js_error)
                    };
                    let _ = response.send(result);
                }
                JetStreamCommand::PublishBus {
                    subject,
                    message_id,
                    payload,
                    response,
                } => {
                    let result = if let Err(error) = ensure_stream(
                        &context,
                        &mut streams,
                        BUS_STREAM,
                        vec!["notify.>".to_string()],
                        false,
                    )
                    .await
                    {
                        Err(error)
                    } else {
                        publish_with_id(&context, &subject, &message_id, payload)
                            .await
                            .map(|_| ())
                            .map_err(js_error)
                    };
                    let _ = response.send(result);
                }
            }
        }
    });
}

async fn ensure_stream(
    context: &jetstream::Context,
    streams: &mut HashSet<String>,
    name: &str,
    subjects: Vec<String>,
    allow_direct: bool,
) -> Result<(), String> {
    if streams.contains(name) {
        return Ok(());
    }
    context
        .get_or_create_stream(stream::Config {
            name: name.to_string(),
            subjects,
            allow_direct,
            ..Default::default()
        })
        .await
        .map_err(js_error)?;
    streams.insert(name.to_string());
    Ok(())
}

async fn publish_with_id(
    context: &jetstream::Context,
    subject: &str,
    message_id: &str,
    payload: Vec<u8>,
) -> Result<PublishAck, async_nats::Error> {
    let mut headers = HeaderMap::new();
    headers.insert(NATS_MESSAGE_ID, message_id);
    let future: PublishAckFuture = context
        .publish_message(
            PublishMessage::build()
                .payload(payload.into())
                .headers(headers)
                .message_id(message_id)
                .outbound_message(subject.to_string()),
        )
        .await?;
    future.await.map_err(Into::into)
}

async fn fetch_queue_deliveries(
    context: &jetstream::Context,
    consumer_name: &str,
    subject: &str,
    limit: usize,
) -> Result<Vec<JetStreamDelivery>, String> {
    let stream = context.get_stream(QUEUE_STREAM).await.map_err(js_error)?;
    let consumer = stream
        .get_or_create_consumer(
            consumer_name,
            pull::Config {
                durable_name: Some(consumer_name.to_string()),
                filter_subject: subject.to_string(),
                ack_policy: AckPolicy::Explicit,
                deliver_policy: DeliverPolicy::All,
                ack_wait: QUEUE_TRANSPORT_ACK_WAIT,
                max_deliver: 10_000,
                ..Default::default()
            },
        )
        .await
        .map_err(js_error)?;
    let mut messages = consumer
        .fetch()
        .max_messages(limit)
        .messages()
        .await
        .map_err(js_error)?;
    let mut deliveries = Vec::new();
    use futures_util::StreamExt;
    while let Some(message) = messages.next().await {
        let message = message.map_err(js_error)?;
        let info = message.info().map_err(js_error)?;
        let payload: Value =
            serde_json::from_slice(&message.payload).map_err(|error| error.to_string())?;
        deliveries.push(JetStreamDelivery {
            message_id: payload["messageId"]
                .as_str()
                .unwrap_or("missing")
                .to_string(),
            reply_subject: message
                .reply
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            stream_sequence: info.stream_sequence as i64,
            consumer_sequence: info.consumer_sequence as i64,
        });
    }
    Ok(deliveries)
}

#[derive(Default)]
struct SelfHostedPolicy;

impl VersionedPort for SelfHostedPolicy {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![PortDescriptor::new(PortKind::Policy, "self_hosted.policy")
            .for_profiles(&["self_hosted"])
            .with_capabilities(&["policy.evaluate", "policy.explain"])]
    }
}

impl PolicyPort for SelfHostedPolicy {
    fn evaluate(&self, request: PolicyRequest) -> Result<PolicyDecision, String> {
        let actor = AuthzActor {
            principal_id: request.authority.actor,
            ..AuthzActor::default()
        };
        let decision = authorize_local_owner(AuthzRequest {
            capability: request.action,
            actor,
            target: Some(AuthzTarget {
                resource_type: Some(request.resource),
                ..AuthzTarget::default()
            }),
            attributes: request
                .context
                .as_object()
                .cloned()
                .unwrap_or_else(Map::new),
        });
        Ok(PolicyDecision {
            decision: if decision.allowed { "allow" } else { "deny" }.to_string(),
            policy_version: "tradeassembly.self-hosted-owner.v1".to_string(),
            explanation: vec![decision.reason],
            constraints: json!({"purpose": request.purpose}),
        })
    }
}

struct SelfHostedCredentialStore {
    resolver: Arc<dyn SecretResolver>,
}

impl Default for SelfHostedCredentialStore {
    fn default() -> Self {
        Self {
            resolver: Arc::new(EnvSecretResolver),
        }
    }
}

fn provider_credential_reference(provider_ref: &str) -> Option<String> {
    let env_key = format!(
        "TRADEASSEMBLY_{}_CREDENTIAL_REF",
        provider_ref
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    );
    std::env::var(env_key).ok()
}

impl VersionedPort for SelfHostedCredentialStore {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::Credentials, "self_hosted.secret-resolver")
                .for_profiles(&["self_hosted"])
                .with_capabilities(&["credential.status", "credential.secret_ref"]),
        ]
    }
}

impl CredentialPort for SelfHostedCredentialStore {
    fn status(&self, provider_ref: &str) -> CredentialStatus {
        let credentials = self.resolve_broker_credentials(provider_ref).ok().flatten();
        CredentialStatus {
            provider_ref: provider_ref.to_string(),
            configured: credentials.is_some(),
            custody: "self-hosted/operator-managed".to_string(),
            redacted_display: credentials.as_ref().map(|_| "configured".to_string()),
        }
    }

    fn store(
        &self,
        _credential_ref: &str,
        _fields: &std::collections::BTreeMap<String, String>,
    ) -> Result<CredentialStatus, String> {
        Err(
            "self-hosted credentials are operator-managed through configured secret references"
                .to_string(),
        )
    }

    fn revoke(&self, _credential_ref: &str) -> Result<bool, String> {
        Err(
            "self-hosted credentials are operator-managed through configured secret references"
                .to_string(),
        )
    }

    fn resolve_fields(
        &self,
        credential_ref: &str,
    ) -> Result<Option<std::collections::BTreeMap<String, String>>, String> {
        let Some(reference) = provider_credential_reference(credential_ref) else {
            return Ok(None);
        };
        let encoded = self.resolver.resolve(&reference)?;
        let value: Value = serde_json::from_str(&encoded)
            .map_err(|_| "self-hosted credential payload is invalid".to_string())?;
        let stored_ref = value
            .get("credential_ref")
            .or_else(|| value.get("provider_ref"))
            .and_then(Value::as_str)
            .unwrap_or(credential_ref);
        if stored_ref != credential_ref {
            return Err("self-hosted credential reference mismatch".to_string());
        }
        let fields = if let Some(fields) = value.get("fields").and_then(Value::as_object) {
            fields
                .iter()
                .map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.clone(), value.to_string()))
                        .ok_or_else(|| "self-hosted credential field is invalid".to_string())
                })
                .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?
        } else {
            ["api_key", "api_secret", "base_url"]
                .into_iter()
                .filter_map(|key| {
                    value
                        .get(key)
                        .and_then(Value::as_str)
                        .map(|value| (key.to_string(), value.to_string()))
                })
                .collect()
        };
        if fields.is_empty() || fields.values().any(|value| value.trim().is_empty()) {
            return Err("self-hosted credential payload is incomplete".to_string());
        }
        Ok(Some(fields))
    }

    fn resolve_broker_credentials(
        &self,
        provider_ref: &str,
    ) -> Result<Option<BrokerCredentials>, String> {
        let Some(fields) = self.resolve_fields(provider_ref)? else {
            return Ok(None);
        };
        let api_key = fields.get("api_key").cloned().unwrap_or_default();
        let api_secret = fields.get("api_secret").cloned().unwrap_or_default();
        let base_url = fields.get("base_url").cloned().unwrap_or_default();
        if api_key.trim().is_empty() || api_secret.trim().is_empty() || base_url.trim().is_empty() {
            return Err("self-hosted broker credential payload is incomplete".to_string());
        }
        Ok(Some(BrokerCredentials {
            provider_ref: provider_ref.to_string(),
            api_key,
            api_secret,
            base_url,
        }))
    }
}

struct SelfHostedProviderCatalog;

impl VersionedPort for SelfHostedProviderCatalog {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::Plugins, "self_hosted.provider-compatibility")
                .for_profiles(&["self_hosted"])
                .with_capabilities(&["plugin.capabilities"]),
        ]
    }
}

impl ProviderPort for SelfHostedProviderCatalog {
    fn capabilities(&self, provider_ref: &str) -> Vec<ProviderCapability> {
        vec![
            ProviderCapability {
                provider_ref: provider_ref.to_string(),
                capability: "marketdata.quote".to_string(),
                available: true,
            },
            ProviderCapability {
                provider_ref: provider_ref.to_string(),
                capability: "broker.order_submit.paper".to_string(),
                available: provider_ref.contains("paper") || provider_ref == "sim",
            },
        ]
    }
}

struct SelfHostedClock;

impl VersionedPort for SelfHostedClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![PortDescriptor::new(PortKind::Clock, "system.utc-clock")
            .for_profiles(&["self_hosted"])
            .with_capabilities(&["clock.utc_ms"])]
    }
}

impl ClockPort for SelfHostedClock {
    fn now_ms(&self) -> i64 {
        epoch_ms()
    }

    fn trusted_now_ms(&self) -> Result<i64, String> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .map_err(|_| "system clock is unavailable".to_string())
    }
}

struct SelfHostedFileExportStore {
    root: PathBuf,
}

impl SelfHostedFileExportStore {
    fn new(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|_| "create artifact root failed".to_string())?;
        let root =
            fs::canonicalize(root).map_err(|_| "canonicalize artifact root failed".to_string())?;
        Ok(Self { root })
    }

    fn read_path(&self, artifact_ref: &str) -> Result<PathBuf, String> {
        let relative = Path::new(artifact_ref);
        let mut components = relative.components();
        if relative.is_absolute()
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
        {
            return Err("artifact ref is unsupported".to_string());
        }
        let canonical = fs::canonicalize(self.root.join(relative))
            .map_err(|_| "read export artifact failed".to_string())?;
        if !canonical.starts_with(&self.root) {
            return Err("artifact ref escapes configured root".to_string());
        }
        Ok(canonical)
    }
}

impl VersionedPort for SelfHostedFileExportStore {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Exports, "self_hosted.file-artifacts")
            .for_profiles(&["self_hosted"])
            .with_capabilities(&[
                "artifact.write",
                "artifact.read",
                "artifact.safe_equivalent",
            ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl ExportPort for SelfHostedFileExportStore {
    fn write(&self, key: &str, content_type: &str, bytes: &[u8]) -> Result<ExportArtifact, String> {
        let artifact_ref = stable_id("artifact", key);
        let path = self.root.join(&artifact_ref);
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err("write export artifact failed".to_string());
        }
        fs::write(&path, bytes).map_err(|_| "write export artifact failed".to_string())?;
        Ok(ExportArtifact {
            artifact_ref,
            content_type: content_type.to_string(),
            size_bytes: bytes.len() as u64,
            sha256: format!("sha256:{:x}", Sha256::digest(bytes)),
        })
    }

    fn read(&self, artifact_ref: &str) -> Result<Vec<u8>, String> {
        fs::read(self.read_path(artifact_ref)?)
            .map_err(|_| "read export artifact failed".to_string())
    }
}

fn queue_delivery_from_row(row: Row) -> Result<QueueDelivery, String> {
    Ok(QueueDelivery {
        message_id: row.get(0),
        queue: row.get(1),
        payload: serde_json::from_str(&row.get::<_, String>(2))
            .map_err(|error| error.to_string())?,
        idempotency_key: IdempotencyKey::new(row.get::<_, String>(3))?,
        partition_key: row.get(4),
        priority: row.get(5),
        attempt: row.get::<_, i32>(6) as u32,
        lease_until_ms: row.get::<_, Option<i64>>(7).unwrap_or_default(),
        fencing_token: row.get(8),
    })
}

fn postgres_event_from_row(row: &Row) -> Result<(EventRecord, Option<i64>, Option<i64>), String> {
    Ok((
        EventRecord {
            event_id: row.get(0),
            stream: row.get(1),
            sequence: row.get(2),
            event_type: row.get(3),
            aggregate_id: row.get(4),
            payload: serde_json::from_str(&row.get::<_, String>(5))
                .map_err(|error| error.to_string())?,
            idempotency_key: IdempotencyKey::new(row.get::<_, String>(6))?,
            occurred_at_ms: row.get(7),
        },
        row.get(8),
        row.get(9),
    ))
}

fn pending_queue_reply_from_row(row: &Row, now_ms: i64) -> Result<PendingQueueReply, String> {
    let state: String = row.get(3);
    let kind = match state.as_str() {
        "acknowledging" => QueueReplyKind::Ack,
        "retrying" => QueueReplyKind::Retry,
        "terminating" => QueueReplyKind::Terminate,
        _ => return Err("queue reply state is invalid".to_string()),
    };
    Ok(PendingQueueReply {
        message_id: row.get(0),
        fencing_token: row.get(1),
        reply_subject: row
            .get::<_, Option<String>>(2)
            .ok_or_else(|| "queue reply metadata is unavailable".to_string())?,
        kind,
        delay_ms: row.get::<_, i64>(4).saturating_sub(now_ms).max(0),
    })
}

fn validate_key(namespace: &str, key: &str) -> Result<(), String> {
    if namespace.trim().is_empty() || key.trim().is_empty() {
        return Err("storage namespace and key are required".to_string());
    }
    Ok(())
}

fn postgres_batch_lock_ids(expectations: &[StorageExpectation]) -> Vec<i64> {
    let mut lock_ids = expectations
        .iter()
        .map(|expectation| {
            let mut hasher = Sha256::new();
            hasher.update((expectation.namespace.len() as u64).to_be_bytes());
            hasher.update(expectation.namespace.as_bytes());
            hasher.update((expectation.key.len() as u64).to_be_bytes());
            hasher.update(expectation.key.as_bytes());
            let digest = hasher.finalize();
            let mut bytes = [0_u8; 8];
            bytes.copy_from_slice(&digest[..8]);
            i64::from_be_bytes(bytes)
        })
        .collect::<Vec<_>>();
    lock_ids.sort_unstable();
    lock_ids.dedup();
    lock_ids
}

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn nats_component(value: &str) -> String {
    format!("v1_{:x}", Sha256::digest(value.as_bytes()))
}

fn stable_id(prefix: &str, seed: &str) -> String {
    format!("{prefix}-{:x}", Sha256::digest(seed.as_bytes()))
}

fn canonical_hash(value: &Value) -> String {
    let mut digest = Sha256::new();
    digest.update(serde_json_canonicalizer::to_vec(value).expect("JSON values are serializable"));
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn js_error(error: impl ToString) -> String {
    format!("jetstream operation failed: {}", error.to_string())
}

fn redacted_pg_error(error: postgres::Error) -> String {
    format!(
        "postgres operation failed: {}",
        error.code().map(|code| code.code()).unwrap_or("unknown")
    )
}

#[cfg(test)]
mod tests {
    use super::{
        nats_component, postgres_batch_lock_ids, SelfHostedFileExportStore, SelfHostedPostgresState,
    };
    use crate::ports::{
        AuthorityContext, ComparePutOutcome, ExportPort, IdempotencyKey, SideEffectContext,
        StorageExpectation, StoragePort, StorageWrite,
    };
    use serde_json::json;
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn nats_components_do_not_collapse_distinct_names() {
        assert_ne!(nats_component("a-b"), nats_component("a_b"));
        assert_eq!(nats_component("a-b"), nats_component("a-b"));
    }

    #[test]
    fn postgres_batch_locks_are_unique_and_deterministically_ordered() {
        let expectations = [
            StorageExpectation::new("z", "two", None),
            StorageExpectation::new("a", "one", None),
            StorageExpectation::new("z", "two", None),
        ];
        let lock_ids = postgres_batch_lock_ids(&expectations);
        let reversed = postgres_batch_lock_ids(&[
            expectations[2].clone(),
            expectations[1].clone(),
            expectations[0].clone(),
        ]);
        assert_eq!(lock_ids, reversed);
        assert_eq!(lock_ids.len(), 2);
        assert!(lock_ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    #[ignore = "requires TRADEASSEMBLY_TEST_POSTGRES_URL"]
    fn postgres_absent_expectation_has_one_winner_across_connections() {
        let database_url =
            std::env::var("TRADEASSEMBLY_TEST_POSTGRES_URL").expect("test Postgres URL");
        let left = Arc::new(
            SelfHostedPostgresState::new(
                &database_url,
                false,
                "https://identity.example.test",
                "tradeassembly-studio",
                None,
            )
            .expect("left Postgres state"),
        );
        let right = Arc::new(
            SelfHostedPostgresState::new(
                &database_url,
                false,
                "https://identity.example.test",
                "tradeassembly-studio",
                None,
            )
            .expect("right Postgres state"),
        );
        let fixture = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let namespace = format!("gc33_t04a_{fixture}");
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("gc33-t04a-postgres-race").expect("idempotency key"),
        );
        let write = StorageWrite::new(
            &namespace,
            "activation",
            json!({"state": "active"}),
            context,
        );
        let expectation = StorageExpectation::new(&namespace, "activation", None);
        let barrier = Arc::new(Barrier::new(2));
        let workers = [left, right]
            .into_iter()
            .map(|state| {
                let barrier = Arc::clone(&barrier);
                let write = write.clone();
                let expectation = expectation.clone();
                thread::spawn(move || {
                    barrier.wait();
                    state.put_json_batch(&[write], &[expectation])
                })
            })
            .collect::<Vec<_>>();
        let outcomes = workers
            .into_iter()
            .map(|worker| worker.join().expect("Postgres worker").expect("batch"))
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == ComparePutOutcome::Updated)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == ComparePutOutcome::Conflict)
                .count(),
            1
        );
    }

    #[test]
    fn export_reads_are_confined_to_the_configured_root() {
        let fixture = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let parent = std::env::temp_dir().join(format!("tradeassembly-export-{fixture}"));
        let root = parent.join("root");
        fs::create_dir_all(&root).expect("artifact root");
        let outside = parent.join("outside-secret");
        fs::write(&outside, b"secret").expect("outside fixture");
        let store = SelfHostedFileExportStore::new(root.clone()).expect("store");
        let artifact = store
            .write("report.json", "application/json", b"safe")
            .expect("write artifact");
        assert!(!artifact.artifact_ref.contains('/'));
        assert_eq!(
            store.read(&artifact.artifact_ref).expect("read artifact"),
            b"safe"
        );
        assert!(store.read("../outside-secret").is_err());
        assert!(store.read(outside.to_string_lossy().as_ref()).is_err());
        assert!(store
            .read(&format!("file://{}", outside.display()))
            .is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink fixture");
            assert!(store.read("escape").is_err());
        }
        let _ = fs::remove_dir_all(parent);
    }
}
