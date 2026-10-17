use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::NamedTempFile;
use tradeassembly_runtime::ports::{
    verify_core_paper_runner, CorePaperRunnerPort, IdempotencyKey, PaperRunnerCommand,
    PaperRunnerOperation, PaperRunnerReceiptState, RunnerBridgeErrorCode, CORE_PAPER_RUNNER_SCHEMA,
    CORE_PAPER_RUNNER_VERSION,
};
use tradeassembly_runtime::service::paper_runner::DurableCorePaperRunner;
use tradeassembly_runtime::service::TradeAssemblyService;

const CORE_RELEASE: &str = "test-core-paper-release";

fn save_config_with_mode(service: &TradeAssemblyService, mode: &str) -> Value {
    let response = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": mode,
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
    if mode == "paper" {
        assert_eq!(response.body["body"]["readiness"]["ready"], true);
    }
    response.body["body"].clone()
}

fn save_config(service: &TradeAssemblyService) -> Value {
    save_config_with_mode(service, "paper")
}

fn activate_command(saved: &Value, suffix: &str) -> PaperRunnerCommand {
    let config = &saved["item"];
    let revision = &saved["readiness"]["capabilityGraphRevision"];
    PaperRunnerCommand {
        schema: CORE_PAPER_RUNNER_SCHEMA.to_string(),
        bridge_version: CORE_PAPER_RUNNER_VERSION.to_string(),
        core_release: CORE_RELEASE.to_string(),
        workspace_id: "workspace-hosted-1".to_string(),
        command_id: format!("paper-activate-{suffix}"),
        authority_ref: "tradeassembly://authority/receipt-hosted-1".to_string(),
        idempotency_key: IdempotencyKey::new(format!("paper-activate-idempotency-{suffix}"))
            .expect("idempotency"),
        submitted_at_ms: 100,
        operation: PaperRunnerOperation::Activate {
            activation_id: format!("activation-hosted-{suffix}"),
            config_id: config["configId"].as_str().expect("config id").to_string(),
            strategy_id: config["strategyId"]
                .as_str()
                .expect("strategy id")
                .to_string(),
            strategy_version_id: config["strategyVersionId"]
                .as_str()
                .expect("strategy version")
                .to_string(),
            strategy_spec_hash: config["strategySpecHash"]
                .as_str()
                .expect("strategy hash")
                .to_string(),
            capability_graph_revision_id: revision["revisionId"]
                .as_str()
                .expect("graph revision")
                .to_string(),
            capability_graph_fingerprint: revision["graphFingerprint"]
                .as_str()
                .expect("graph fingerprint")
                .to_string(),
        },
    }
}

fn followup_command(
    activate: &PaperRunnerCommand,
    operation: &str,
    suffix: &str,
) -> PaperRunnerCommand {
    PaperRunnerCommand {
        schema: CORE_PAPER_RUNNER_SCHEMA.to_string(),
        bridge_version: CORE_PAPER_RUNNER_VERSION.to_string(),
        core_release: CORE_RELEASE.to_string(),
        workspace_id: activate.workspace_id.clone(),
        command_id: format!("paper-{operation}-{suffix}"),
        authority_ref: activate.authority_ref.clone(),
        idempotency_key: IdempotencyKey::new(format!("paper-{operation}-idempotency-{suffix}"))
            .expect("idempotency"),
        submitted_at_ms: activate.submitted_at_ms + 1,
        operation: match operation {
            "inspect" => PaperRunnerOperation::Inspect {
                activation_id: activate.operation.activation_id().to_string(),
            },
            "stop" => PaperRunnerOperation::Stop {
                activation_id: activate.operation.activation_id().to_string(),
            },
            _ => panic!("unsupported operation"),
        },
    }
}

#[test]
fn strict_contract_rejects_unknown_release_hash_and_credential_material() {
    let database = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(database.path().to_string_lossy());
    let command = activate_command(&save_config(&service), "strict");
    let bytes = serde_json::to_vec(&command).expect("command json");
    assert_eq!(
        PaperRunnerCommand::parse_json(&bytes, CORE_RELEASE).expect("strict command"),
        command
    );

    let mut unknown = serde_json::to_value(&command).expect("command value");
    unknown["unknownField"] = json!(true);
    assert_eq!(
        PaperRunnerCommand::parse_json(
            &serde_json::to_vec(&unknown).expect("unknown json"),
            CORE_RELEASE
        )
        .expect_err("unknown field")
        .code,
        RunnerBridgeErrorCode::MalformedCommand
    );

    assert_eq!(
        PaperRunnerCommand::parse_json(&bytes, "different-release")
            .expect_err("release mismatch")
            .code,
        RunnerBridgeErrorCode::CoreReleaseMismatch
    );

    let mut credential = command.clone();
    credential.authority_ref = "vault://broker-secret".to_string();
    assert_eq!(
        credential
            .validate(CORE_RELEASE)
            .expect_err("credential reference")
            .code,
        RunnerBridgeErrorCode::CredentialMaterialNotAllowed
    );

    let mut hash = command;
    if let PaperRunnerOperation::Activate {
        strategy_spec_hash, ..
    } = &mut hash.operation
    {
        *strategy_spec_hash = "not-a-hash".to_string();
    }
    assert_eq!(
        hash.validate(CORE_RELEASE).expect_err("invalid hash").code,
        RunnerBridgeErrorCode::PayloadOutOfBounds
    );
}

#[test]
fn durable_runner_activates_inspects_stops_replays_and_recovers() {
    let database = NamedTempFile::new().expect("database");
    let path = database.path().to_string_lossy().to_string();
    let service = TradeAssemblyService::test_local(&path);
    let activate = activate_command(&save_config(&service), "durable");
    let runner = DurableCorePaperRunner::new(service.clone(), CORE_RELEASE).expect("paper runner");

    let report =
        verify_core_paper_runner(&runner, &activate, CORE_RELEASE).expect("port conformance");
    assert_eq!(
        report.suite_id,
        "tradeassembly.core-paper-runner.conformance.v1"
    );
    let activated = runner.dispatch(&activate).expect("activate");
    assert_eq!(activated.state, PaperRunnerReceiptState::Completed);
    assert_eq!(activated.activation_state.as_deref(), Some("active"));
    assert!(activated.result_sha256.is_some());

    let inspect = followup_command(&activate, "inspect", "active");
    let inspected = runner.dispatch(&inspect).expect("inspect active");
    assert_eq!(inspected.activation_state.as_deref(), Some("active"));
    let mut cross_workspace = followup_command(&activate, "inspect", "cross-workspace");
    cross_workspace.workspace_id = "workspace-hosted-peer".to_string();
    cross_workspace.command_id = "paper-inspect-cross-workspace-peer".to_string();
    cross_workspace.idempotency_key =
        IdempotencyKey::new("paper-inspect-cross-workspace-peer").expect("idempotency");
    let denied = runner
        .dispatch(&cross_workspace)
        .expect("workspace denial receipt");
    assert_eq!(denied.state, PaperRunnerReceiptState::Failed);
    assert_eq!(
        denied.failure_code.as_deref(),
        Some("activation_ownership_conflict")
    );

    service
        .runtime()
        .storage
        .clear_namespace("core_paper_runner_receipts_v1")
        .expect("simulate crash before receipt");
    drop(runner);
    drop(service);
    let reopened = TradeAssemblyService::test_local(&path);
    let recovered =
        DurableCorePaperRunner::new(reopened.clone(), CORE_RELEASE).expect("reopened runner");
    let recovered_receipt = recovered
        .receipt(&activate.workspace_id, &activate.command_id)
        .expect("recovered receipt");
    assert_eq!(
        recovered_receipt,
        Some(activated),
        "recovered={recovered_receipt:#?} failure={:?}",
        recovered_receipt
            .as_ref()
            .and_then(|receipt| receipt.failure_code.as_deref())
    );
    assert!(recovered
        .receipt("other-workspace", &activate.command_id)
        .expect("scoped lookup")
        .is_none());

    let stop = followup_command(&activate, "stop", "durable");
    let stopped = recovered.dispatch(&stop).expect("stop");
    assert_eq!(stopped.activation_state.as_deref(), Some("stopped"));
    let stopped_replay = recovered.dispatch(&stop).expect("stop replay");
    assert_eq!(stopped_replay, stopped);
    reopened
        .runtime()
        .storage
        .clear_namespace("core_paper_runner_receipts_v1")
        .expect("simulate receipt loss after stop");
    assert_eq!(
        recovered
            .receipt(&stop.workspace_id, &stop.command_id)
            .expect("reconcile stop receipt"),
        Some(stopped)
    );
    assert_eq!(
        recovered
            .receipt(&inspect.workspace_id, &inspect.command_id)
            .expect("reconcile historical inspect receipt"),
        Some(inspected),
        "inspect recovery must retain the originally bound active snapshot"
    );

    let inspect_stopped = followup_command(&activate, "inspect", "stopped");
    assert_eq!(
        recovered
            .dispatch(&inspect_stopped)
            .expect("inspect stopped")
            .activation_state
            .as_deref(),
        Some("stopped")
    );
}

#[test]
fn immutable_activation_binding_mismatch_fails_closed_and_is_receipted() {
    let database = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(database.path().to_string_lossy());
    let saved = save_config(&service);
    let correct = activate_command(&saved, "mismatch");
    let mut command = correct.clone();
    if let PaperRunnerOperation::Activate {
        capability_graph_fingerprint,
        ..
    } = &mut command.operation
    {
        *capability_graph_fingerprint = "sha256:deadbeef".to_string();
    }
    let runner = DurableCorePaperRunner::new(service, CORE_RELEASE).expect("paper runner");
    let receipt = runner.dispatch(&command).expect("failed receipt");
    assert_eq!(receipt.state, PaperRunnerReceiptState::Failed);
    assert_eq!(
        receipt.failure_code.as_deref(),
        Some("immutable_activation_binding_mismatch")
    );
    assert_eq!(runner.dispatch(&command).expect("replay"), receipt);
    let mut corrected = correct;
    corrected.command_id = "paper-activate-mismatch-corrected".to_string();
    corrected.idempotency_key =
        IdempotencyKey::new("paper-activate-mismatch-corrected").expect("idempotency");
    assert_eq!(
        runner
            .dispatch(&corrected)
            .expect("corrected activation")
            .activation_state
            .as_deref(),
        Some("active"),
        "a rejected immutable binding must not activate the strategy"
    );
}

#[test]
fn persisted_live_config_is_rejected_before_durable_admission() {
    let database = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(database.path().to_string_lossy());
    let live = save_config_with_mode(&service, "live");
    let mut command = activate_command(&save_config(&service), "live-config");
    if let PaperRunnerOperation::Activate { config_id, .. } = &mut command.operation {
        *config_id = live["item"]["configId"]
            .as_str()
            .expect("live config id")
            .to_string();
    }
    let runner = DurableCorePaperRunner::new(service.clone(), CORE_RELEASE).expect("paper runner");

    let error = runner
        .dispatch(&command)
        .expect_err("live config must be rejected");
    assert_eq!(error.code, RunnerBridgeErrorCode::LiveModeNotAllowed);
    for namespace in [
        "core_paper_runner_commands_v1",
        "core_paper_runner_idempotency_v1",
        "core_paper_runner_activation_bindings_v1",
        "core_paper_runner_results_v1",
        "core_paper_runner_receipts_v1",
    ] {
        assert!(
            service
                .runtime()
                .storage
                .list_json(namespace)
                .expect("bridge namespace")
                .is_empty(),
            "{namespace} must remain empty"
        );
    }
}

#[test]
fn concurrent_activation_dispatch_is_duplicate_safe() {
    let database = NamedTempFile::new().expect("database");
    let service = TradeAssemblyService::test_local(database.path().to_string_lossy());
    let command = activate_command(&save_config(&service), "concurrent");
    let runner =
        Arc::new(DurableCorePaperRunner::new(service, CORE_RELEASE).expect("shared paper runner"));
    let barrier = Arc::new(Barrier::new(4));
    let workers = (0..4)
        .map(|_| {
            let runner = Arc::clone(&runner);
            let barrier = Arc::clone(&barrier);
            let command = command.clone();
            thread::spawn(move || {
                barrier.wait();
                runner.dispatch(&command)
            })
        })
        .collect::<Vec<_>>();
    let receipts = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker").expect("dispatch"))
        .collect::<Vec<_>>();
    assert!(
        receipts.iter().all(|receipt| {
            receipt == &receipts[0]
                && receipt.state == PaperRunnerReceiptState::Completed
                && receipt.activation_state.as_deref() == Some("active")
        }),
        "{receipts:#?}"
    );
}

#[test]
fn process_protocol_dispatches_and_looks_up_paper_receipt() {
    let database = NamedTempFile::new().expect("database");
    let path = database.path().to_string_lossy().to_string();
    let service = TradeAssemblyService::test_local(&path);
    let command = activate_command(&save_config(&service), "process");
    drop(service);
    let process_root = tempfile::tempdir().expect("process root");
    fs::create_dir_all(process_root.path().join(".tradeassembly"))
        .expect("TradeAssembly directory");
    fs::write(
        process_root.path().join(".tradeassembly/warden.token"),
        "test-only-runner-token-at-least-32-bytes",
    )
    .expect("Warden token");

    let mut child = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .env("TRADEASSEMBLY_AUTH_PROFILE", "oidc_test")
        .env(
            "TRADEASSEMBLY_AUTH_ISSUER",
            "http://127.0.0.1:8080/realms/tradeassembly-dev",
        )
        .env("TRADEASSEMBLY_AUTH_AUDIENCE", "tradeassembly-local")
        .env("TRADEASSEMBLY_AUTH_CLIENT_ID", "tradeassembly-local")
        .args(["--db", &path, "--core-release", CORE_RELEASE])
        .current_dir(process_root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("runner process");
    {
        let input = child.stdin.as_mut().expect("stdin");
        writeln!(
            input,
            "{}",
            json!({"operation": "dispatch_paper", "command": command})
        )
        .expect("dispatch request");
        writeln!(
            input,
            "{}",
            json!({
                "operation": "paper_receipt",
                "workspace_id": command.workspace_id,
                "command_id": command.command_id,
            })
        )
        .expect("receipt request");
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("runner output");
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = String::from_utf8(output.stdout)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("response json"))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["ok"], true);
    assert_eq!(rows[0]["receipt"]["state"], "failed");
    assert_eq!(
        rows[0]["receipt"]["failureCode"],
        "finance_authority_denied"
    );
    assert_eq!(rows[1]["receipt"], rows[0]["receipt"]);
}
