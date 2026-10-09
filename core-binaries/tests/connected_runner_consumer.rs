// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tempfile::{NamedTempFile, TempDir};
use tradeassembly_runtime::adapters::connected_runner::{
    read_ed25519_seed_file, Ed25519InstallationSigner, HttpProductRunnerTransport,
};
use tradeassembly_runtime::adapters::local::sqlite::LocalSqliteStorage;
use tradeassembly_runtime::cli_identity::authenticated_service;
use tradeassembly_runtime::finance_authority::TestFinanceAuthority;
use tradeassembly_runtime::ports::{
    canonical_receipt_hash, AuthorityContext, ClaimRunnerCommandRequest, ClockPort,
    CompleteRunnerCommandRequest, ConnectedRunnerError, ConnectedRunnerErrorCode,
    ConnectedRunnerPoll, CorePaperRunnerPort, IdempotencyKey, InstallationSignerPort,
    PaperRunnerCommand, PaperRunnerOperation, PaperRunnerReceipt, PaperRunnerReceiptState,
    PortDescriptor, ProductRunnerTransportPort, RunnerEntropyPort, RunnerHandoffReceipt,
    RunnerLease, RunnerOutcome, SideEffectContext, StoragePort, VersionedPort,
    CORE_PAPER_RUNNER_SCHEMA, CORE_PAPER_RUNNER_VERSION, HANDOFF_RECEIPT_SCHEMA,
    INSTALLATION_AUTHENTICATION_DOMAIN, INSTALLATION_REQUEST_SCHEMA, RUNNER_LEASE_SCHEMA,
};
use tradeassembly_runtime::runtime_config::{RuntimeBuilder, RuntimeConfig};
use tradeassembly_runtime::service::connected_runner::ConnectedPaperRunnerConsumer;
use tradeassembly_runtime::service::paper_runner::DurableCorePaperRunner;
use tradeassembly_runtime::service::TradeAssemblyService;

const CORE_RELEASE: &str = "connected-runner-test-release";
const INSTALLATION_ID: &str = "installation-1";

#[derive(Default)]
struct TestClock(AtomicI64);

impl TestClock {
    fn at(now_ms: i64) -> Self {
        Self(AtomicI64::new(now_ms))
    }

    fn advance(&self, delta_ms: i64) {
        self.0.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl VersionedPort for TestClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}

impl ClockPort for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct TestEntropy(AtomicU64);

impl VersionedPort for TestEntropy {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}

impl RunnerEntropyPort for TestEntropy {
    fn opaque_id(&self, prefix: &str) -> Result<String, ConnectedRunnerError> {
        Ok(format!(
            "{prefix}-{}",
            self.0.fetch_add(1, Ordering::SeqCst)
        ))
    }
}

#[derive(Default)]
struct FakeTransport {
    leases: Mutex<VecDeque<Option<RunnerLease>>>,
    completions: Mutex<Vec<CompleteRunnerCommandRequest>>,
    fail_completions: AtomicU64,
}

impl FakeTransport {
    fn with_leases(leases: impl IntoIterator<Item = Option<RunnerLease>>) -> Self {
        Self {
            leases: Mutex::new(leases.into_iter().collect()),
            ..Self::default()
        }
    }

    fn fail_next_completion(&self) {
        self.fail_completions.store(1, Ordering::SeqCst);
    }
}

impl VersionedPort for FakeTransport {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}

impl ProductRunnerTransportPort for FakeTransport {
    fn claim(
        &self,
        _request: &ClaimRunnerCommandRequest,
    ) -> Result<Option<RunnerLease>, ConnectedRunnerError> {
        Ok(self
            .leases
            .lock()
            .expect("leases")
            .pop_front()
            .unwrap_or(None))
    }

    fn complete(
        &self,
        request: &CompleteRunnerCommandRequest,
    ) -> Result<RunnerHandoffReceipt, ConnectedRunnerError> {
        self.completions
            .lock()
            .expect("completions")
            .push(request.clone());
        if self
            .fail_completions
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            return Err(ConnectedRunnerError::transport_unavailable());
        }
        Ok(RunnerHandoffReceipt {
            schema_version: HANDOFF_RECEIPT_SCHEMA.to_string(),
            receipt_id: format!("handoff-{}", request.lease_generation),
            installation_id: request.installation_id.clone(),
            workspace_id: "workspace-connected".to_string(),
            command_id: request.command_id.clone(),
            command_sha256: request.command_sha256.clone(),
            lease_generation: request.lease_generation,
            outcome: request.completion.outcome,
            core_receipt_schema: request.completion.core_receipt_schema.clone(),
            core_receipt_sha256: request.completion.core_receipt_sha256.clone(),
            core_receipt_ref: request.completion.core_receipt_ref.clone(),
            completed_at_ms: request.completion.completed_at_ms,
            recorded_at_ms: request.completion.completed_at_ms,
        })
    }
}

struct CountingRunner {
    calls: AtomicU64,
}

impl VersionedPort for CountingRunner {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}

struct FailedRunner;

impl VersionedPort for FailedRunner {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}

impl CorePaperRunnerPort for FailedRunner {
    fn dispatch(
        &self,
        command: &PaperRunnerCommand,
    ) -> Result<PaperRunnerReceipt, tradeassembly_runtime::ports::RunnerBridgeError> {
        Ok(PaperRunnerReceipt {
            schema: CORE_PAPER_RUNNER_SCHEMA.to_string(),
            bridge_version: CORE_PAPER_RUNNER_VERSION.to_string(),
            receipt_id: "failed-connected-receipt".to_string(),
            workspace_id: command.workspace_id.clone(),
            command_id: command.command_id.clone(),
            idempotency_key: command.idempotency_key.clone(),
            operation: command.operation.kind(),
            activation_id: command.operation.activation_id().to_string(),
            core_release: command.core_release.clone(),
            command_sha256: command.canonical_hash()?,
            state: PaperRunnerReceiptState::Failed,
            activation_state: None,
            result_sha256: None,
            evidence_refs: Vec::new(),
            failure_code: Some("authority_denied".to_string()),
            recorded_at_ms: command.submitted_at_ms,
        })
    }

    fn receipt(
        &self,
        _workspace_id: &str,
        _command_id: &str,
    ) -> Result<Option<PaperRunnerReceipt>, tradeassembly_runtime::ports::RunnerBridgeError> {
        Ok(None)
    }
}

impl CorePaperRunnerPort for CountingRunner {
    fn dispatch(
        &self,
        _command: &PaperRunnerCommand,
    ) -> Result<PaperRunnerReceipt, tradeassembly_runtime::ports::RunnerBridgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("invalid lease must not dispatch")
    }

    fn receipt(
        &self,
        _workspace_id: &str,
        _command_id: &str,
    ) -> Result<Option<PaperRunnerReceipt>, tradeassembly_runtime::ports::RunnerBridgeError> {
        Ok(None)
    }
}

fn save_config(service: &TradeAssemblyService) -> Value {
    let response = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {
                "max_notional": 25.0,
                "max_order_quantity": 0.00017,
            },
            "capabilityBindings": {
                "execution.market.bars": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "marketdata.bars.read_v1",
                },
                "execution.broker.submit": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "broker.paper_order_submit",
                },
            },
        }),
    );
    assert_eq!(response.status, 200, "{:#}", response.body);
    response.body["body"].clone()
}

fn command(saved: &Value) -> PaperRunnerCommand {
    let config = &saved["item"];
    let revision = &saved["readiness"]["capabilityGraphRevision"];
    PaperRunnerCommand {
        schema: CORE_PAPER_RUNNER_SCHEMA.to_string(),
        bridge_version: CORE_PAPER_RUNNER_VERSION.to_string(),
        core_release: CORE_RELEASE.to_string(),
        workspace_id: "workspace-connected".to_string(),
        command_id: "connected-command-1".to_string(),
        authority_ref: "tradeassembly://authority/paper-connected".to_string(),
        idempotency_key: IdempotencyKey::new("connected-idempotency-1").expect("idempotency"),
        submitted_at_ms: 1_000_000,
        operation: PaperRunnerOperation::Activate {
            activation_id: "connected-activation-1".to_string(),
            config_id: config["configId"].as_str().expect("config").to_string(),
            strategy_id: config["strategyId"].as_str().expect("strategy").to_string(),
            strategy_version_id: config["strategyVersionId"]
                .as_str()
                .expect("version")
                .to_string(),
            strategy_spec_hash: config["strategySpecHash"]
                .as_str()
                .expect("strategy hash")
                .to_string(),
            capability_graph_revision_id: revision["revisionId"]
                .as_str()
                .expect("revision")
                .to_string(),
            capability_graph_fingerprint: revision["graphFingerprint"]
                .as_str()
                .expect("graph fingerprint")
                .to_string(),
        },
    }
}

fn lease(command: PaperRunnerCommand, generation: u64, now_ms: i64) -> RunnerLease {
    RunnerLease {
        schema_version: RUNNER_LEASE_SCHEMA.to_string(),
        installation_id: INSTALLATION_ID.to_string(),
        workspace_id: command.workspace_id.clone(),
        command_id: command.command_id.clone(),
        command_sha256: command.canonical_hash().expect("command hash"),
        lease_generation: generation,
        lease_token: format!("lease-token-{generation}"),
        leased_until_ms: now_ms + 30_000,
        command,
    }
}

fn consumer(
    transport: Arc<FakeTransport>,
    runner: Arc<dyn CorePaperRunnerPort>,
    clock: Arc<TestClock>,
) -> ConnectedPaperRunnerConsumer {
    ConnectedPaperRunnerConsumer::new(
        CORE_RELEASE,
        transport,
        Arc::new(Ed25519InstallationSigner::from_seed(INSTALLATION_ID, [7; 32]).expect("signer")),
        runner,
        clock,
        Arc::new(TestEntropy::default()),
    )
    .expect("consumer")
}

fn response_server(status: &str, body: Vec<u8>) -> (String, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let status = status.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = vec![0_u8; 8 * 1024];
        let read = stream.read(&mut request).expect("request");
        request.truncate(read);
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .expect("headers");
        stream.write_all(&body).expect("body");
        request
    });
    (format!("http://{address}"), handle)
}

fn repeated_idle_server() -> (String, thread::JoinHandle<std::time::Duration>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let address = listener.local_addr().expect("address");
    let handle = thread::spawn(move || {
        let mut accepted_at = Vec::new();
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        for _ in 0..2 {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "timed out waiting for runner claim"
                        );
                        thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            };
            // Accepted sockets may inherit the listener's nonblocking mode.
            // This fixture uses a bounded blocking read after accepting.
            stream
                .set_nonblocking(false)
                .expect("blocking request stream");
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .expect("read timeout");
            let mut request = vec![0_u8; 8 * 1024];
            let _read = stream.read(&mut request).expect("request");
            accepted_at.push(Instant::now());
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnull"
            )
            .expect("response");
        }
        accepted_at[1].duration_since(accepted_at[0])
    });
    (format!("http://{address}"), handle)
}

#[test]
fn canonical_claim_and_completion_are_hub_compatible_and_key_safe() {
    let signer = Ed25519InstallationSigner::from_seed(INSTALLATION_ID, [7; 32]).expect("signer");
    let request = ClaimRunnerCommandRequest {
        schema_version: INSTALLATION_REQUEST_SCHEMA.to_string(),
        installation_id: INSTALLATION_ID.to_string(),
        request_id: "request-1".to_string(),
        nonce: "nonce-1".to_string(),
        issued_at_ms: 1_000_000,
        lease_seconds: 30,
        signature: String::new(),
    };
    let payload = request.signing_payload().expect("payload");
    assert_eq!(
        payload,
        r#"{"installationId":"installation-1","issuedAtMs":1000000,"leaseSeconds":30,"nonce":"nonce-1","operation":"claim","requestId":"request-1","schemaVersion":"tradeassembly.studio-personal.runner-installation-request/v1"}"#
    );
    let signature = URL_SAFE_NO_PAD
        .decode(signer.sign(&payload).expect("signature"))
        .expect("signature bytes");
    let signature = Signature::from_slice(&signature).expect("Ed25519 signature");
    VerifyingKey::from_bytes(
        &<[u8; 32]>::try_from(
            ed25519_dalek::SigningKey::from_bytes(&[7; 32])
                .verifying_key()
                .as_bytes()
                .as_slice(),
        )
        .expect("public key"),
    )
    .expect("verifying key")
    .verify(
        format!("{INSTALLATION_AUTHENTICATION_DOMAIN}{payload}").as_bytes(),
        &signature,
    )
    .expect("Hub-domain signature");
    assert!(!format!("{signer:?}").contains(&URL_SAFE_NO_PAD.encode([7; 32])));
}

#[test]
fn idle_and_invalid_leases_never_dispatch() {
    let clock = Arc::new(TestClock::at(1_000_000));
    let counting = Arc::new(CountingRunner {
        calls: AtomicU64::new(0),
    });
    let idle = Arc::new(FakeTransport::with_leases([None]));
    assert_eq!(
        consumer(idle, counting.clone(), clock.clone())
            .poll_once()
            .expect("idle"),
        ConnectedRunnerPoll::Idle
    );

    let database = NamedTempFile::new().expect("database");
    let service = configured_test_service(database.path().to_string_lossy());
    let valid = lease(command(&save_config(&service)), 1, 1_000_000);
    let invalid = [
        {
            let mut lease = valid.clone();
            lease.schema_version = "unknown".to_string();
            lease
        },
        {
            let mut lease = valid.clone();
            lease.installation_id = "other-installation".to_string();
            lease
        },
        {
            let mut lease = valid.clone();
            lease.workspace_id = "other-workspace".to_string();
            lease
        },
        {
            let mut lease = valid.clone();
            lease.command_id = "other-command".to_string();
            lease
        },
        {
            let mut lease = valid.clone();
            lease.command_sha256 = format!("sha256:{:064x}", 1);
            lease
        },
        {
            let mut lease = valid.clone();
            lease.leased_until_ms = 1_000_000;
            lease
        },
        {
            let mut lease = valid.clone();
            lease.leased_until_ms = 1_024_999;
            lease
        },
        {
            let mut lease = valid;
            lease.leased_until_ms = 1_036_000;
            lease
        },
    ];
    for invalid_lease in invalid {
        let transport = Arc::new(FakeTransport::with_leases([Some(invalid_lease)]));
        assert_eq!(
            consumer(transport, counting.clone(), clock.clone())
                .poll_once()
                .expect_err("invalid lease")
                .code,
            ConnectedRunnerErrorCode::InvalidContract
        );
    }
    assert_eq!(counting.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn failed_core_receipt_is_completed_as_failed_with_integrity() {
    let database = NamedTempFile::new().expect("database");
    let service = configured_test_service(database.path().to_string_lossy());
    let command = command(&save_config(&service));
    let transport = Arc::new(FakeTransport::with_leases([Some(lease(
        command, 1, 1_000_000,
    ))]));
    let result = consumer(
        transport.clone(),
        Arc::new(FailedRunner),
        Arc::new(TestClock::at(1_000_000)),
    )
    .poll_once()
    .expect("failed receipt handoff");
    let ConnectedRunnerPoll::Completed { receipt, handoff } = result else {
        panic!("expected completed handoff");
    };
    assert_eq!(receipt.state, PaperRunnerReceiptState::Failed);
    assert_eq!(handoff.outcome, RunnerOutcome::Failed);
    let completions = transport.completions.lock().expect("completions");
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].completion.outcome, RunnerOutcome::Failed);
    assert_eq!(
        completions[0].completion.core_receipt_sha256,
        canonical_receipt_hash(&receipt).expect("receipt hash")
    );
}

#[test]
fn durable_dispatch_completion_outage_and_redelivery_are_duplicate_safe() {
    let database = NamedTempFile::new().expect("database");
    let path = database.path().to_string_lossy().to_string();
    let service = configured_test_service(&path);
    let command = command(&save_config(&service));
    let clock = Arc::new(TestClock::at(1_000_000));
    let first = lease(command.clone(), 1, 1_000_000);
    let second = lease(command.clone(), 2, 1_031_000);
    let transport = Arc::new(FakeTransport::with_leases([Some(first), Some(second)]));
    transport.fail_next_completion();
    let runner =
        Arc::new(DurableCorePaperRunner::new(service, CORE_RELEASE).expect("paper runner"));
    let first_consumer = consumer(transport.clone(), runner, clock.clone());
    assert_eq!(
        first_consumer
            .poll_once()
            .expect_err("completion outage")
            .code,
        ConnectedRunnerErrorCode::TransportUnavailable
    );

    drop(first_consumer);
    clock.advance(31_000);
    let reopened = configured_test_service(&path);
    let second_consumer = consumer(
        transport.clone(),
        Arc::new(
            DurableCorePaperRunner::new(reopened.clone(), CORE_RELEASE).expect("reopened runner"),
        ),
        clock,
    );
    let ConnectedRunnerPoll::Completed { receipt, handoff } =
        second_consumer.poll_once().expect("redelivered completion")
    else {
        panic!("expected completion");
    };
    assert_eq!(receipt.state, PaperRunnerReceiptState::Completed);
    assert_eq!(handoff.lease_generation, 2);
    let completions = transport.completions.lock().expect("completions");
    assert_eq!(completions.len(), 2);
    assert_eq!(
        completions[0].completion.core_receipt_sha256,
        completions[1].completion.core_receipt_sha256
    );
    assert_eq!(
        completions[1].completion.core_receipt_sha256,
        canonical_receipt_hash(&receipt).expect("receipt hash")
    );
    assert_eq!(
        reopened
            .runtime()
            .storage
            .list_json("execution_activations")
            .expect("activations")
            .len(),
        1
    );
    assert_eq!(completions[1].completion.outcome, RunnerOutcome::Completed);
}

#[test]
fn http_transport_is_loopback_or_https_only_redirect_free_and_bounded() {
    assert!(HttpProductRunnerTransport::new("http://example.com").is_err());
    assert!(HttpProductRunnerTransport::new("https://user:password@example.com").is_err());
    assert!(HttpProductRunnerTransport::new("https://example.com/path").is_err());

    let (url, server) = response_server("200 OK", b"null".to_vec());
    let transport = HttpProductRunnerTransport::new(&url).expect("loopback transport");
    let claim = ClaimRunnerCommandRequest {
        schema_version: INSTALLATION_REQUEST_SCHEMA.to_string(),
        installation_id: INSTALLATION_ID.to_string(),
        request_id: "request-http".to_string(),
        nonce: "nonce-http".to_string(),
        issued_at_ms: 1_000_000,
        lease_seconds: 30,
        signature: "signature".to_string(),
    };
    assert!(transport.claim(&claim).expect("idle claim").is_none());
    let request = String::from_utf8(server.join().expect("server")).expect("request text");
    assert!(
        request.starts_with("POST /v1/installations/installation-1/runner-commands/claim HTTP/1.1")
    );

    let (url, server) = response_server("302 Found", b"{}".to_vec());
    let transport = HttpProductRunnerTransport::new(&url).expect("transport");
    assert_eq!(
        transport.claim(&claim).expect_err("redirect").code,
        ConnectedRunnerErrorCode::TransportUnavailable
    );
    server.join().expect("redirect server");

    let (url, server) = response_server("200 OK", vec![b'x'; 64 * 1024 + 1]);
    let transport = HttpProductRunnerTransport::new(&url).expect("transport");
    assert_eq!(
        transport.claim(&claim).expect_err("oversized").code,
        ConnectedRunnerErrorCode::InvalidContract
    );
    server.join().expect("oversized server");

    let completion = CompleteRunnerCommandRequest {
        schema_version: INSTALLATION_REQUEST_SCHEMA.to_string(),
        installation_id: INSTALLATION_ID.to_string(),
        request_id: "request-complete".to_string(),
        nonce: "nonce-complete".to_string(),
        issued_at_ms: 1_000_000,
        command_id: "desk/command-complete".to_string(),
        lease_generation: 2,
        lease_token: "lease-token-2".to_string(),
        command_sha256: format!("sha256:{:064x}", 2),
        completion: tradeassembly_runtime::ports::RunnerCompletion {
            outcome: RunnerOutcome::Completed,
            core_receipt_schema: CORE_PAPER_RUNNER_SCHEMA.to_string(),
            core_receipt_sha256: format!("sha256:{:064x}", 3),
            core_receipt_ref: "tradeassembly://runner-receipts/workspace/command".to_string(),
            completed_at_ms: 1_000_000,
        },
        signature: "signature".to_string(),
    };
    let (url, server) = response_server("409 Conflict", b"{}".to_vec());
    let transport = HttpProductRunnerTransport::new(&url).expect("transport");
    assert_eq!(
        transport
            .complete(&completion)
            .expect_err("stale completion")
            .code,
        ConnectedRunnerErrorCode::StaleLease
    );
    let request = String::from_utf8(server.join().expect("stale server")).expect("request text");
    assert!(request.starts_with(
        "POST /v1/installations/installation-1/runner-commands/desk%2Fcommand-complete/complete HTTP/1.1"
    ));
}

#[test]
fn installation_seed_file_is_regular_private_and_exactly_bounded() {
    let directory = TempDir::new().expect("directory");
    let path = directory.path().join("installation.key");
    fs::write(&path, format!("{}\n", URL_SAFE_NO_PAD.encode([7; 32]))).expect("seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }
    assert_eq!(
        read_ed25519_seed_file(&path).expect("private seed"),
        [7; 32]
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::{symlink, PermissionsExt};
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("public mode");
        assert!(read_ed25519_seed_file(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private mode");
        let link = directory.path().join("installation-link.key");
        symlink(&path, &link).expect("symlink");
        assert!(read_ed25519_seed_file(&link).is_err());
    }
    assert!(read_ed25519_seed_file(Path::new("/path/that/does/not/exist")).is_err());
}

#[test]
fn connected_runner_binary_claims_once_without_exposing_key_material() {
    let directory = TempDir::new().expect("directory");
    let key_path = directory.path().join("installation.key");
    let encoded_seed = URL_SAFE_NO_PAD.encode([7; 32]);
    fs::write(&key_path, format!("{encoded_seed}\n")).expect("seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }
    let tradeassembly_dir = directory.path().join(".tradeassembly");
    fs::create_dir(&tradeassembly_dir).expect("TradeAssembly directory");
    fs::write(
        tradeassembly_dir.join("warden.token"),
        "test-warden-token-32-bytes-minimum\n",
    )
    .expect("Warden token");
    let database = directory.path().join("runner.sqlite3");
    drop(configured_test_service(database.to_string_lossy()));
    let (url, server) = response_server("200 OK", b"null".to_vec());
    let mut command = Command::new(env!("CARGO_BIN_EXE_tradeassembly-connected-runner"));
    configure_verified_oidc_session(&mut command, directory.path());
    let output = command
        .current_dir(directory.path())
        .args([
            "--db",
            database.to_str().expect("database path"),
            "--core-release",
            CORE_RELEASE,
            "--product-url",
            &url,
            "--installation-id",
            INSTALLATION_ID,
            "--installation-key-file",
            key_path.to_str().expect("key path"),
            "--once",
        ])
        .output()
        .expect("connected runner process");
    assert!(output.status.success(), "{output:?}");
    let request = String::from_utf8(server.join().expect("server")).expect("request text");
    assert!(
        request.starts_with("POST /v1/installations/installation-1/runner-commands/claim HTTP/1.1")
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    let stderr = String::from_utf8(output.stderr).expect("stderr");
    assert_eq!(stdout.trim(), r#"{"status":"idle"}"#);
    assert!(stderr.is_empty(), "{stderr}");
    assert!(!stdout.contains(&encoded_seed));
    assert!(!stderr.contains(&encoded_seed));
}

#[test]
fn connected_runner_fails_closed_before_polling_without_oidc_session() {
    let directory = TempDir::new().expect("directory");
    let key_path = directory.path().join("installation.key");
    let encoded_seed = URL_SAFE_NO_PAD.encode([7; 32]);
    fs::write(&key_path, format!("{encoded_seed}\n")).expect("seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }
    let tradeassembly_dir = directory.path().join(".tradeassembly");
    fs::create_dir(&tradeassembly_dir).expect("TradeAssembly directory");
    fs::write(
        tradeassembly_dir.join("warden.token"),
        "test-warden-token-32-bytes-minimum\n",
    )
    .expect("Warden token");
    let database = directory.path().join("runner.sqlite3");
    let service = configured_test_service(database.to_string_lossy());
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("persisted-runner-before-oidc").expect("idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(
            "execution_runs",
            "run_activation_before_oidc",
            json!({
                "activationId": "activation_before_oidc",
                "state": "active",
                "cycle": 0,
            }),
            &context,
        )
        .expect("persist active execution");
    service
        .runtime()
        .storage
        .put_json(
            "scheduler_state",
            "current",
            json!({
                "running": true,
                "state": "running",
                "activationId": "activation_before_oidc",
                "intervalSeconds": 1,
                "workerId": "worker-before-oidc",
            }),
            &context,
        )
        .expect("persist scheduler state");
    drop(service);
    let output = Command::new(env!("CARGO_BIN_EXE_tradeassembly-connected-runner"))
        .current_dir(directory.path())
        .args([
            "--db",
            database.to_str().expect("database path"),
            "--core-release",
            CORE_RELEASE,
            "--product-url",
            "http://127.0.0.1:9",
            "--installation-id",
            INSTALLATION_ID,
            "--installation-key-file",
            key_path.to_str().expect("key path"),
            "--once",
        ])
        .output()
        .expect("connected runner process");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr");
    assert!(stderr.contains("CoreFailure"), "{stderr}");
    assert!(!stderr.contains(&encoded_seed));
    let storage = LocalSqliteStorage::new(database.to_string_lossy());
    assert_eq!(
        storage
            .get_json("execution_runs", "run_activation_before_oidc")
            .expect("read execution")
            .expect("persisted execution")["cycle"],
        0
    );
    assert_eq!(
        storage
            .get_json("scheduler_state", "current")
            .expect("read scheduler")
            .expect("persisted scheduler")["running"],
        true
    );
}

#[test]
fn authenticated_service_reopens_owned_execution_config() {
    let directory = TempDir::new().expect("directory");
    let database = directory.path().join("runner.sqlite3");
    let mut config = RuntimeConfig::local(database.to_string_lossy());
    config.oidc_profile = "oidc_test".to_string();
    config.oidc_issuer = "http://127.0.0.1:8080/realms/tradeassembly-dev".to_string();
    config.oidc_audience = "tradeassembly-local".to_string();
    config.oidc_client_id = "tradeassembly-local".to_string();
    let (key_path, session_path) = write_verified_oidc_session(directory.path());
    config.oidc_session_key_path = key_path.to_string_lossy().into_owned();
    config.oidc_session_path = session_path.to_string_lossy().into_owned();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");

    let first = configured_test_service(database.to_string_lossy());
    let first = runtime
        .block_on(authenticated_service(&first, &config))
        .expect("first authenticated service");
    let saved = first.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "providerRef": "sim",
            "mode": "paper",
        }),
    );
    assert_eq!(saved.status, 200, "{:?}", saved.body);
    let config_id = saved.body["body"]["configId"]
        .as_str()
        .expect("config ID")
        .to_string();
    drop(first);

    let reopened = configured_test_service(database.to_string_lossy());
    let reopened = runtime
        .block_on(authenticated_service(&reopened, &config))
        .expect("reopened authenticated service");
    assert!(
        reopened
            .runtime()
            .storage
            .get_json("execution_configs", &config_id)
            .expect("reopened config storage")
            .is_some(),
        "saved config was not durable"
    );
    let config_scope = reopened
        .runtime()
        .object_authorization
        .scope("execution_config", &config_id)
        .expect("config scope")
        .expect("persisted config scope");
    assert_eq!(config_scope.owner.subject, "identity_connected_runner");
    let strategy = reopened.handle_http("GET", "/strategies/strat_local_btc_demo", json!({}));
    assert_eq!(strategy.status, 200, "{:?}", strategy.body);
    let readiness = reopened.handle_http(
        "POST",
        "/product/strategy-execution-configs/activation-readiness",
        json!({"configId": config_id}),
    );
    assert_eq!(readiness.status, 200, "{:?}", readiness.body);
    let activated = reopened.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "activationId": "activation_connected_runner_reopen",
            "configId": config_id,
            "idempotencyKey": "connected-runner-reopen",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activated.status, 200, "{:?}", activated.body);
    assert_eq!(
        activated.body["body"]["run"]["state"], "active",
        "{:?}",
        activated.body
    );
}

#[test]
fn connected_runner_continuous_mode_waits_between_claims_and_is_supervisor_stoppable() {
    let directory = TempDir::new().expect("directory");
    let key_path = directory.path().join("installation.key");
    fs::write(&key_path, format!("{}\n", URL_SAFE_NO_PAD.encode([7; 32]))).expect("seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).expect("private mode");
    }
    let tradeassembly_dir = directory.path().join(".tradeassembly");
    fs::create_dir(&tradeassembly_dir).expect("TradeAssembly directory");
    fs::write(
        tradeassembly_dir.join("warden.token"),
        "test-warden-token-32-bytes-minimum\n",
    )
    .expect("Warden token");
    let database = directory.path().join("runner.sqlite3");
    drop(configured_test_service(database.to_string_lossy()));
    let (url, server) = repeated_idle_server();
    let mut command = Command::new(env!("CARGO_BIN_EXE_tradeassembly-connected-runner"));
    configure_verified_oidc_session(&mut command, directory.path());
    let mut child = command
        .current_dir(directory.path())
        .args([
            "--db",
            database.to_str().expect("database path"),
            "--core-release",
            CORE_RELEASE,
            "--product-url",
            &url,
            "--installation-id",
            INSTALLATION_ID,
            "--installation-key-file",
            key_path.to_str().expect("key path"),
            "--poll-interval-ms",
            "100",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("connected runner process");
    let interval = server.join().expect("server");
    assert!(
        interval >= std::time::Duration::from_millis(80),
        "{interval:?}"
    );
    child.kill().expect("stop supervised runner");
    assert!(!child.wait().expect("runner status").success());
}

fn configure_verified_oidc_session(command: &mut Command, root: &Path) {
    let (key_path, session_path) = write_verified_oidc_session(root);
    command
        .env("TRADEASSEMBLY_AUTH_PROFILE", "oidc_test")
        .env(
            "TRADEASSEMBLY_AUTH_ISSUER",
            "http://127.0.0.1:8080/realms/tradeassembly-dev",
        )
        .env("TRADEASSEMBLY_AUTH_AUDIENCE", "tradeassembly-local")
        .env("TRADEASSEMBLY_AUTH_CLIENT_ID", "tradeassembly-local")
        .env("TRADEASSEMBLY_AUTH_CLI_SESSION_KEY_PATH", key_path)
        .env("TRADEASSEMBLY_AUTH_CLI_SESSION_PATH", session_path);
}

fn configured_test_service(db: impl ToString) -> TradeAssemblyService {
    let db = db.to_string();
    let mut config = RuntimeConfig::local(db.clone());
    config.oidc_profile = "oidc_test".to_string();
    config.oidc_issuer = "http://127.0.0.1:8080/realms/tradeassembly-dev".to_string();
    config.oidc_audience = "tradeassembly-local".to_string();
    config.oidc_client_id = "tradeassembly-local".to_string();
    let (runtime, _) = RuntimeBuilder::new(config)
        .with_finance_authority(Arc::new(TestFinanceAuthority))
        .build()
        .expect("configured test runtime");
    TradeAssemblyService::from_runtime(db, runtime)
}

fn write_verified_oidc_session(root: &Path) -> (PathBuf, PathBuf) {
    const AAD: &[u8] = b"tradeassembly.auth-oidc-session.v1";
    let key_path = root.join("oidc-session.key");
    let session_path = root.join("oidc-session.bin");
    let key = [0x42_u8; 32];
    let nonce = [0x24_u8; 12];
    let expires_at_ms = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("millisecond clock")
        + 300_000;
    let plaintext = serde_json::to_vec(&json!({
        "version": 1,
        "identity": {
            "stableIdentityId": "identity_connected_runner",
            "issuer": "http://127.0.0.1:8080/realms/tradeassembly-dev",
            "subject": "connected-runner-user",
            "audience": ["tradeassembly-local"],
            "displayName": "Connected Runner User",
            "email": "connected-runner@example.invalid",
            "assurance": null,
            "expiresAtMs": expires_at_ms
        },
        "refreshToken": null
    }))
    .expect("serialize OIDC session");
    let ciphertext = Aes256Gcm::new_from_slice(&key)
        .expect("session cipher")
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: AAD,
            },
        )
        .expect("encrypt OIDC session");
    let mut envelope = vec![1_u8];
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    fs::write(&key_path, URL_SAFE_NO_PAD.encode(key)).expect("write OIDC key");
    fs::write(&session_path, envelope).expect("write OIDC session");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))
            .expect("protect OIDC key");
        fs::set_permissions(&session_path, fs::Permissions::from_mode(0o600))
            .expect("protect OIDC session");
    }
    (key_path, session_path)
}
