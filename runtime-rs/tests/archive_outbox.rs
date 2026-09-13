use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tradeassembly_runtime::archive_outbox::{
    archive_transport_descriptor, ArchiveEvidenceInput, ArchiveEvidenceKind, ArchiveEvidenceOutbox,
    ArchiveEvidenceQuery, ArchiveIngestReceipt, ArchiveIngestRequest, ArchiveOutboxError,
    ArchiveTransportFailure, ArchiveTransportPort, ARCHIVE_EVIDENCE_SCHEMA, ARCHIVE_INGEST_SCHEMA,
    CLOUD_ARCHIVE_RECORD_SCHEMA, MAX_SYNC_BATCH, MAX_SYNC_SCAN,
};
use tradeassembly_runtime::ports::{
    AuthorityContext, IdempotencyKey, SideEffectContext, VersionedPort,
};
use tradeassembly_runtime::service::TradeAssemblyService;

#[derive(Clone, Copy)]
enum Plan {
    Unavailable,
    Accept(Option<i64>),
    MismatchedRecordDigest,
}

struct RecordingTransport {
    plans: Mutex<VecDeque<Plan>>,
    requests: Mutex<Vec<ArchiveIngestRequest>>,
}

impl RecordingTransport {
    fn new(plans: impl IntoIterator<Item = Plan>) -> Self {
        Self {
            plans: Mutex::new(plans.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ArchiveIngestRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl VersionedPort for RecordingTransport {
    fn descriptors(&self) -> Vec<tradeassembly_runtime::ports::PortDescriptor> {
        vec![archive_transport_descriptor("test.recording-archive")]
    }
}

impl ArchiveTransportPort for RecordingTransport {
    fn ingest(
        &self,
        request: &ArchiveIngestRequest,
    ) -> Result<ArchiveIngestReceipt, ArchiveTransportFailure> {
        request
            .verify()
            .expect("outbox requests are valid gateway requests");
        self.requests
            .lock()
            .expect("requests lock")
            .push(request.clone());
        match self
            .plans
            .lock()
            .expect("plans lock")
            .pop_front()
            .unwrap_or(Plan::Accept(None))
        {
            Plan::Unavailable => Err(ArchiveTransportFailure::new("gateway_unavailable", true)),
            Plan::Accept(cursor) => Ok(receipt_for(request, cursor)),
            Plan::MismatchedRecordDigest => {
                let mut receipt = receipt_for(request, None);
                receipt.record_digest_sha256 = "f".repeat(64);
                Ok(receipt)
            }
        }
    }
}

struct BlockingTransport {
    state: (Mutex<(bool, bool)>, Condvar),
}

impl BlockingTransport {
    fn new() -> Self {
        Self {
            state: (Mutex::new((false, false)), Condvar::new()),
        }
    }

    fn wait_until_claimed(&self) {
        let (lock, wake) = &self.state;
        let mut state = lock.lock().expect("blocking transport lock");
        while !state.0 {
            let (next, result) = wake
                .wait_timeout(state, Duration::from_secs(2))
                .expect("blocking transport wait");
            assert!(!result.timed_out(), "sync did not claim its archive record");
            state = next;
        }
    }

    fn release(&self) {
        let (lock, wake) = &self.state;
        let mut state = lock.lock().expect("blocking transport lock");
        state.1 = true;
        wake.notify_all();
    }
}

impl VersionedPort for BlockingTransport {
    fn descriptors(&self) -> Vec<tradeassembly_runtime::ports::PortDescriptor> {
        vec![archive_transport_descriptor("test.blocking-archive")]
    }
}

impl ArchiveTransportPort for BlockingTransport {
    fn ingest(
        &self,
        request: &ArchiveIngestRequest,
    ) -> Result<ArchiveIngestReceipt, ArchiveTransportFailure> {
        request
            .verify()
            .expect("outbox requests are valid gateway requests");
        let (lock, wake) = &self.state;
        let mut state = lock.lock().expect("blocking transport lock");
        state.0 = true;
        wake.notify_all();
        while !state.1 {
            state = wake.wait(state).expect("blocking transport wait");
        }
        Ok(receipt_for(request, None))
    }
}

fn receipt_for(request: &ArchiveIngestRequest, cursor: Option<i64>) -> ArchiveIngestReceipt {
    ArchiveIngestReceipt {
        schema: ARCHIVE_INGEST_SCHEMA.to_string(),
        request_id: request.request_id.clone(),
        idempotency_key: request.idempotency_key.clone(),
        request_digest_sha256: request.request_digest_sha256().expect("request digest"),
        entitlement_snapshot_ref: "entitlement-local-test".to_string(),
        archive_ref: format!("archive-{}", request.request_id),
        record_digest_sha256: cloud_archive_record_digest(request, "entitlement-local-test"),
        archive_cursor: cursor,
    }
}

fn cloud_archive_record_digest(
    request: &ArchiveIngestRequest,
    entitlement_snapshot_ref: &str,
) -> String {
    let record = json!({
        "schema": CLOUD_ARCHIVE_RECORD_SCHEMA,
        "tenantId": request.tenant_id,
        "workspaceId": request.workspace_id,
        "toolName": request.tool_name,
        "requestId": request.request_id,
        "requestDigestSha256": request.request_digest_sha256().expect("request digest"),
        "entitlementSnapshotRef": entitlement_snapshot_ref,
        "occurredAtMs": request.occurred_at_ms,
        "redactedPayload": request.payload,
    });
    format!(
        "{:x}",
        Sha256::digest(
            serde_json_canonicalizer::to_vec(&record).expect("canonical Cloud archive record")
        )
    )
}

fn local_evidence_payload(input: &ArchiveEvidenceInput) -> serde_json::Value {
    json!({
        "schema": ARCHIVE_EVIDENCE_SCHEMA,
        "kind": input.evidence_kind,
        "source": input.source,
        "aggregateId": input.aggregate_id,
        "authorityContext": input.context.authority,
        "redacted": true,
        "evidence": input.payload,
    })
}

fn input(request_id: &str, idempotency_key: &str, occurred_at_ms: i64) -> ArchiveEvidenceInput {
    ArchiveEvidenceInput {
        tenant_id: "tenant-local".to_string(),
        workspace_id: "workspace-local".to_string(),
        tool_name: "studio.execution.external_receipt.append".to_string(),
        request_id: request_id.to_string(),
        evidence_kind: ArchiveEvidenceKind::Receipt,
        source: "studio".to_string(),
        aggregate_id: "deployment-local".to_string(),
        occurred_at_ms,
        payload: json!({
            "broker": "alpaca",
            "eventType": "order_accepted",
            "clientOrderId": "client-order-local",
            "redacted": true,
        }),
        context: SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new(idempotency_key).expect("idempotency key"),
        ),
    }
}

fn operator_context(idempotency_key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext {
            actor: "operator-local-test".to_string(),
            surface: "mcp".to_string(),
            account_mode: "paper".to_string(),
        },
        IdempotencyKey::new(idempotency_key).expect("operator idempotency key"),
    )
}

fn test_db(name: &str) -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().expect("tempdir");
    let db = directory
        .path()
        .join(format!("{name}.db"))
        .to_string_lossy()
        .to_string();
    (directory, db)
}

#[test]
fn receipt_is_local_before_transport_and_restart_retries_the_exact_gateway_request() {
    let (_directory, db) = test_db("archive-restart");
    let service = TradeAssemblyService::test_local(&db);
    let outbox = ArchiveEvidenceOutbox::new(service.runtime());
    let queued = outbox
        .enqueue(input("request-receipt-1", "archive-idempotency-1", 1_000))
        .expect("queue local receipt");
    assert!(!queued.duplicate);
    assert_eq!(queued.sync.state, "pending");
    assert_eq!(queued.evidence.schema, ARCHIVE_INGEST_SCHEMA);
    assert_eq!(
        queued.evidence.payload["schema"],
        "tradeassembly.archive-evidence.v1"
    );
    let gateway_request = queued.evidence.ingest_request();
    let cloud_digest_input = (
        gateway_request.schema.as_str(),
        gateway_request.tenant_id.as_str(),
        gateway_request.workspace_id.as_str(),
        gateway_request.tool_name.as_str(),
        gateway_request.request_id.as_str(),
        gateway_request.payload_sha256.as_str(),
        gateway_request.occurred_at_ms,
    );
    let cloud_digest = format!(
        "{:x}",
        Sha256::digest(
            serde_json_canonicalizer::to_vec(&cloud_digest_input)
                .expect("canonical Cloud gateway tuple")
        )
    );
    assert_eq!(
        queued.evidence.request_digest_sha256, cloud_digest,
        "local request digest must match the product gateway tuple"
    );

    let duplicate = outbox
        .enqueue(input("request-receipt-1", "archive-idempotency-1", 1_000))
        .expect("exact idempotent enqueue");
    assert!(duplicate.duplicate);

    let transport = RecordingTransport::new([Plan::Unavailable, Plan::Accept(Some(7))]);
    assert!(
        transport.requests().is_empty(),
        "enqueue must not call transport"
    );
    let unavailable = outbox
        .sync(&transport, "local-sync", 1_000, 1)
        .expect("outage is a durable sync result");
    assert_eq!(unavailable.attempted, 1);
    assert_eq!(unavailable.unavailable, 1);
    assert_eq!(unavailable.published, 0);

    let page = outbox
        .query(ArchiveEvidenceQuery {
            tenant_id: Some("tenant-local".to_string()),
            workspace_id: Some("workspace-local".to_string()),
            after_request_id: None,
            limit: 10,
        })
        .expect("local evidence query after outage");
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].sync.state, "unavailable");
    assert_eq!(
        page.records[0].evidence.payload["evidence"]["clientOrderId"],
        "client-order-local"
    );

    drop(outbox);
    drop(service);

    let restarted_service = TradeAssemblyService::test_local(&db);
    let restarted_outbox = ArchiveEvidenceOutbox::new(restarted_service.runtime());
    let published = restarted_outbox
        .sync(&transport, "local-sync", 2_001, 1)
        .expect("restart retry");
    assert_eq!(published.published, 1);
    assert_eq!(published.acknowledged_cursor, Some(7));
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1], "retry must replay exact request");
    assert_eq!(
        requests[0].idempotency_key.as_str(),
        "archive-idempotency-1"
    );
    assert_eq!(
        requests[0].request_digest_sha256().expect("request digest"),
        queued.evidence.request_digest_sha256
    );

    let status = restarted_outbox.status().expect("outbox status");
    assert_eq!(status.published, 1);
    assert_eq!(status.unavailable, 0);
    assert_eq!(status.reconciliation_required, 0);
    assert_eq!(status.acknowledged_cursor, Some(7));
}

#[test]
fn mismatched_cloud_record_digest_is_reconciliation_required_and_does_not_mark_receipt_published() {
    let (_directory, db) = test_db("archive-mismatch");
    let service = TradeAssemblyService::test_local(&db);
    let outbox = ArchiveEvidenceOutbox::new(service.runtime());
    outbox
        .enqueue(input("request-receipt-2", "archive-idempotency-2", 1_000))
        .expect("queue local receipt");
    let transport = RecordingTransport::new([Plan::MismatchedRecordDigest]);

    let report = outbox
        .sync(&transport, "local-sync", 1_000, 1)
        .expect("mismatched ack is contained");
    assert_eq!(report.reconciliation_required, 1);
    assert_eq!(report.published, 0);
    assert_eq!(report.items[0].outcome, "reconciliation_required");
    assert_eq!(
        report.items[0].error_code.as_deref(),
        Some("archive_ack_integrity_mismatch")
    );

    let status = outbox.status().expect("outbox status");
    assert_eq!(status.reconciliation_required, 1);
    assert_eq!(status.published, 0);
    assert_eq!(status.unavailable, 0);
    let skipped = outbox
        .sync(&transport, "local-sync", 10_000, 1)
        .expect("reconciliation-required record stays paused");
    assert_eq!(skipped.attempted, 0);
    assert_eq!(skipped.skipped, 1);

    let requeued = outbox
        .requeue(
            "request-receipt-2",
            11_000,
            &operator_context("archive-requeue-receipt-2"),
        )
        .expect("operator-authorized requeue");
    assert_eq!(requeued.state, "pending");
}

#[test]
fn sensitive_payload_is_rejected_before_local_archive_persistence() {
    let (_directory, db) = test_db("archive-sensitive");
    let service = TradeAssemblyService::test_local(&db);
    let outbox = ArchiveEvidenceOutbox::new(service.runtime());
    let mut sensitive = input("request-sensitive", "archive-idempotency-sensitive", 1_000);
    sensitive.payload = json!({"access_token": "never-store-this"});
    let error = outbox
        .enqueue(sensitive)
        .expect_err("sensitive input fails closed");
    assert_eq!(error, ArchiveOutboxError::SensitivePayload);
    assert_eq!(outbox.status().expect("empty outbox").total, 0);
}

#[test]
fn gateway_boundary_limits_reject_undeliverable_local_evidence() {
    let (_directory, db) = test_db("archive-gateway-boundary");
    let service = TradeAssemblyService::test_local(&db);
    let outbox = ArchiveEvidenceOutbox::new(service.runtime());

    let mut oversized_identifier = input("request-boundary", "archive-boundary", 1_000);
    oversized_identifier.tenant_id = "t".repeat(161);
    assert_eq!(
        outbox
            .enqueue(oversized_identifier)
            .expect_err("Cloud-rejected identifier must not enter local retry state"),
        ArchiveOutboxError::InvalidIdentifier
    );

    let mut exact_gateway_payload = input(
        "request-payload-boundary-exact",
        "archive-payload-boundary-exact",
        1_000,
    );
    exact_gateway_payload.tenant_id = "t".repeat(160);
    exact_gateway_payload.payload = json!({"receipt": ""});
    let fixed_bytes =
        serde_json_canonicalizer::to_vec(&local_evidence_payload(&exact_gateway_payload))
            .expect("canonical local evidence payload")
            .len();
    exact_gateway_payload.payload = json!({"receipt": "x".repeat(64 * 1024 - fixed_bytes)});
    assert_eq!(
        serde_json_canonicalizer::to_vec(&local_evidence_payload(&exact_gateway_payload))
            .expect("canonical exact gateway payload")
            .len(),
        64 * 1024,
        "the test fixture must sit exactly on the Cloud payload boundary"
    );
    outbox
        .enqueue(exact_gateway_payload.clone())
        .expect("Cloud-sized identifier and payload enter local retry state");

    let mut oversized_payload = exact_gateway_payload;
    oversized_payload.request_id = "request-payload-boundary-over".to_string();
    oversized_payload.context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("archive-payload-boundary-over").expect("idempotency key"),
    );
    oversized_payload.payload = json!({"receipt": "x".repeat(64 * 1024 - fixed_bytes + 1)});
    assert_eq!(
        outbox
            .enqueue(oversized_payload)
            .expect_err("Cloud-rejected payload must not enter local retry state"),
        ArchiveOutboxError::PayloadTooLarge
    );
    assert_eq!(outbox.status().expect("one valid envelope").total, 1);
}

#[test]
fn bounded_sync_scan_continues_after_terminal_history_without_redecoding_it() {
    let (_directory, db) = test_db("archive-bounded-scan");
    let service = TradeAssemblyService::test_local(&db);
    let outbox = ArchiveEvidenceOutbox::new(service.runtime());

    for index in 0..=MAX_SYNC_SCAN {
        outbox
            .enqueue(input(
                &format!("request-history-{index:03}"),
                &format!("archive-history-{index:03}"),
                1_000,
            ))
            .expect("queue historical evidence");
    }
    let history_transport = RecordingTransport::new(std::iter::repeat_n(
        Plan::Accept(None),
        MAX_SYNC_SCAN.saturating_add(1),
    ));
    let first_history_page = outbox
        .sync(
            &history_transport,
            "history-publisher",
            1_000,
            MAX_SYNC_BATCH,
        )
        .expect("publish first historical page");
    assert_eq!(first_history_page.published, MAX_SYNC_SCAN);
    assert_eq!(
        first_history_page.next_scan_after_request_id.as_deref(),
        Some("request-history-049")
    );
    let second_history_page = outbox
        .sync(
            &history_transport,
            "history-publisher",
            1_001,
            MAX_SYNC_BATCH,
        )
        .expect("publish final historical page");
    assert_eq!(second_history_page.published, 1);
    assert!(second_history_page.next_scan_after_request_id.is_none());

    outbox
        .enqueue(input(
            "request-history-z-due",
            "archive-history-z-due",
            2_000,
        ))
        .expect("queue evidence after terminal history");
    let due_transport = RecordingTransport::new([Plan::Accept(None)]);
    let first_bounded_scan = outbox
        .sync(&due_transport, "bounded-scanner", 2_000, 1)
        .expect("bounded terminal-history scan");
    assert_eq!(first_bounded_scan.scanned, MAX_SYNC_SCAN);
    assert_eq!(first_bounded_scan.attempted, 0);
    assert_eq!(first_bounded_scan.skipped, MAX_SYNC_SCAN);
    assert!(
        first_bounded_scan.next_scan_after_request_id.is_some(),
        "the next invocation must continue after the bounded terminal page"
    );
    assert!(due_transport.requests().is_empty());

    let second_bounded_scan = outbox
        .sync(&due_transport, "bounded-scanner", 2_001, 1)
        .expect("continued bounded scan reaches due record");
    assert_eq!(second_bounded_scan.scanned, 2);
    assert_eq!(second_bounded_scan.skipped, 1);
    assert_eq!(second_bounded_scan.published, 1);
    assert!(second_bounded_scan.next_scan_after_request_id.is_none());
    assert_eq!(due_transport.requests().len(), 1);
}

#[test]
fn acknowledged_cursor_never_regresses_when_later_sync_receipt_is_lower() {
    let (_directory, db) = test_db("archive-cursor-high-water");
    let service = TradeAssemblyService::test_local(&db);
    let outbox = ArchiveEvidenceOutbox::new(service.runtime());
    outbox
        .enqueue(input("request-cursor-1", "archive-cursor-1", 1_000))
        .expect("queue first receipt");
    let transport = RecordingTransport::new([Plan::Accept(Some(9)), Plan::Accept(Some(3))]);
    outbox
        .sync(&transport, "sync-owner-a", 1_000, 1)
        .expect("publish high cursor");

    outbox
        .enqueue(input("request-cursor-2", "archive-cursor-2", 2_000))
        .expect("queue second receipt");
    outbox
        .sync(&transport, "sync-owner-b", 2_000, 1)
        .expect("publish lower cursor without regression");
    assert_eq!(
        outbox.status().expect("outbox status").acknowledged_cursor,
        Some(9)
    );
}

#[test]
fn requeue_does_not_revoke_an_active_sync_lease() {
    let (_directory, db) = test_db("archive-requeue-active-lease");
    let service = TradeAssemblyService::test_local(&db);
    let outbox = ArchiveEvidenceOutbox::new(service.runtime());
    outbox
        .enqueue(input("request-active-lease", "archive-active-lease", 1_000))
        .expect("queue receipt");
    let transport = Arc::new(BlockingTransport::new());
    let worker_outbox = outbox.clone();
    let worker_transport = Arc::clone(&transport);
    let worker = std::thread::spawn(move || {
        worker_outbox.sync(worker_transport.as_ref(), "lease-owner", 1_000, 1)
    });
    transport.wait_until_claimed();

    let status = outbox
        .requeue(
            "request-active-lease",
            1_001,
            &operator_context("archive-requeue-active-lease"),
        )
        .expect("requeue inspects active lease");
    assert_eq!(status.state, "leased");
    assert!(status.lease_until_ms.is_some());

    transport.release();
    assert_eq!(
        worker
            .join()
            .expect("sync worker panicked")
            .expect("sync completes")
            .published,
        1
    );
}
