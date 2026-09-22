//! Source-free installation acceptance. No broker orders or existing rigs.
use serde_json::{json, Value};
use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn executable() -> std::path::PathBuf {
    std::env::var_os("F2_TEST_TRADEASSEMBLY_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_tradeassembly").into())
}

fn binary() -> Command {
    let mut command = Command::new(executable());
    command.env_clear();
    command.current_dir(std::env::temp_dir().canonicalize().unwrap());
    command
}

#[test]
fn setup_reports_missing_packaged_binary_before_runtime_loading() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().canonicalize().unwrap().join("installation");
    let output = binary()
        .args(["setup", "--state-dir"])
        .arg(&state)
        .arg("--warden-binary")
        .arg(state.join("missing-warden"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["ok"], false);
    assert!(response["error"]["code"].is_string());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("file-backed secret"));
}

#[test]
fn policy_supervision_parser_and_missing_state_fail_before_runtime_loading() {
    use clap::Parser;
    use tradeassembly_runtime::cli::{Cli, Command as CliCommand, PolicyCommand};
    for action in ["install", "verify", "status", "uninstall"] {
        let cli = Cli::try_parse_from([
            "tradeassembly",
            "policy",
            "launchd",
            action,
            "--state-dir",
            "/isolated/fixture",
        ])
        .expect("policy lifecycle is discoverable");
        assert!(matches!(
            cli.command,
            CliCommand::Policy {
                command: PolicyCommand::Launchd { .. }
            }
        ));
    }
    let root = tempfile::tempdir().unwrap();
    let output = binary()
        .args(["policy", "launchd", "install", "--state-dir"])
        .arg(root.path().join("missing"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("file-backed secret"));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["error"]["code"]
        .as_str()
        .unwrap()
        .starts_with("local_install_"));
}

struct OwnedProcess(Child);

#[cfg(unix)]
struct OwnedAgentProcess(Child);

#[cfg(unix)]
impl Drop for OwnedAgentProcess {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            // This test creates its own process group; a timeout must stop the
            // model/MCP descendants before deleting the isolated test state.
            let _ = Command::new("/bin/kill")
                .args(["-TERM", "--", &format!("-{}", self.0.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// Actual provider smoke, deliberately separate from the credential-free gates.
#[test]
#[ignore = "requires explicit F2_TEST_CODEX_BINARY, F2_TEST_CODEX_AUTH_FILE and real Warden; invokes a model"]
#[cfg(unix)]
fn real_codex_nontrading_wake_uses_installation_mcp() {
    exercise_real_codex(false);
}

#[test]
#[ignore = "actual model interruption, real lease expiry and recovery; requires explicit Codex/auth/Warden"]
#[cfg(unix)]
fn real_codex_interruption_quarantines_before_explicit_recovery() {
    exercise_real_codex(true);
}

#[cfg(unix)]
fn exercise_real_codex(interrupt: bool) {
    use std::os::unix::process::CommandExt;
    let codex = std::env::var_os("F2_TEST_CODEX_BINARY").expect("explicit Codex binary");
    let auth = std::env::var_os("F2_TEST_CODEX_AUTH_FILE").expect("explicit authorized auth file");
    let warden = std::env::var_os("F2_TEST_WARDEN_BINARY").expect("explicit Warden binary");
    for path in [&codex, &auth, &warden] {
        assert!(Path::new(path).is_absolute());
    }
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let state = base.join("installation");
    let home = base.join("codex");
    let workspace = base.join("workspace");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&workspace).unwrap();
    std::os::unix::fs::symlink(auth, home.join("auth.json")).unwrap();
    std::fs::write(home.join("config.toml"),
        "sandbox_mode = \"read-only\"\napproval_policy = \"never\"\ncli_auth_credentials_store = \"file\"\n").unwrap();
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    prepare(&state, Path::new(&warden), port);
    drop(reservation);
    let mut policy = OwnedProcess(
        binary()
            .args(["policy", "serve", "--state-dir"])
            .arg(&state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(policy.0.try_wait().unwrap().is_none());
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .is_ok_and(|r| r.status().is_success())
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(100));
    }
    let deployment = tradeassembly_runtime::agent_runner::AgentDeployment {
        executor: Default::default(),
        deployment_id: "nontrading-inspection".into(), system_project_id: "smoke".into(),
        agent_definition_version_id: "smoke-agent-v1".into(), execution_config_version_id: "smoke-config-v1".into(),
        studio_tool_allowlist: vec!["studio.deployment.inspect".into()], desired_state: "active".into(),
        interval_seconds: 60, cron_utc: None, mode: "paper".into(),
        prompt: "This is a nontrading connection test. Call studio.deployment.inspect on tradeAssemblyMcpServer for deployment_id nontrading-inspection exactly once on this wake. Preserve the entire unmodified tool response in the tool output (if using code mode, emit the whole result as JSON). In your final response report only the returned deployment ID. Do not use shell, read files, call other services, submit orders, or change any state.".into(),
        workspace: workspace.to_str().unwrap().into(), runtime_profile: String::new(),
    };
    let definition = base.join("deployment.json");
    std::fs::write(&definition, serde_json::to_vec(&deployment).unwrap()).unwrap();
    let created = binary()
        .arg("--config")
        .arg(state.join("runtime.json"))
        .args(["agent", "create", "--deployment-file"])
        .arg(definition)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "fixture deployment creation failed"
    );
    let invoke = || {
        let mut runner = OwnedAgentProcess(
            binary()
                .env("HOME", std::env::var_os("HOME").unwrap())
                .env("CODEX_HOME", &home)
                .env("TRADEASSEMBLY_CODEX_BIN", &codex)
                .arg("--config")
                .arg(state.join("runtime.json"))
                .args(["agent", "run", "--runner-id", "nontrading-smoke", "--once"])
                .process_group(0)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut stdout = runner.0.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut output = String::new();
            std::io::Read::read_to_string(&mut stdout, &mut output).unwrap();
            output
        });
        let deadline = Instant::now() + Duration::from_secs(180);
        let status = loop {
            if let Some(status) = runner.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "bounded real agent smoke timed out"
            );
            std::thread::sleep(Duration::from_millis(250));
        };
        let output = reader.join().unwrap();
        let response: Value = serde_json::from_str(&output).unwrap();
        assert!(status.success(), "runner failed: {}", response["error"]);
        response
    };
    if interrupt {
        use tradeassembly_runtime::agent_runner::{ACTIVE_RUNS_NS, RUNS_NS};
        use tradeassembly_runtime::runtime_config::{RuntimeConfig, RuntimeConfigLayer};
        let config = RuntimeConfig::resolve(
            Some(
                RuntimeConfigLayer::from_json_file(state.join("runtime.json").to_str().unwrap())
                    .unwrap(),
            ),
            RuntimeConfigLayer::default(),
            RuntimeConfigLayer::default(),
        )
        .unwrap();
        let service =
            tradeassembly_runtime::service::TradeAssemblyService::from_config(config).unwrap();
        let mut interrupted = OwnedAgentProcess(
            binary()
                .env("HOME", std::env::var_os("HOME").unwrap())
                .env("CODEX_HOME", &home)
                .env("TRADEASSEMBLY_CODEX_BIN", &codex)
                .arg("--config")
                .arg(state.join("runtime.json"))
                .args(["agent", "run", "--runner-id", "interrupted-smoke", "--once"])
                .process_group(0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(90);
        let active = loop {
            assert!(
                interrupted.0.try_wait().unwrap().is_none(),
                "runner exited before interruption"
            );
            let mut seen = Vec::new();
            records(&home.join("sessions"), &mut seen);
            if seen
                .iter()
                .any(|item| item["payload"]["type"] == "task_started")
            {
                let active = service
                    .runtime()
                    .storage
                    .get_json(ACTIVE_RUNS_NS, "nontrading-inspection")
                    .unwrap()
                    .unwrap();
                assert_eq!(active["state"], "running");
                break active;
            }
            assert!(Instant::now() < deadline, "actual model turn did not start");
            std::thread::sleep(Duration::from_millis(25));
        };
        // End only this test's process group before model completion.
        drop(interrupted);
        let lease_expiry = active["leaseExpiresAtMs"].as_i64().unwrap();
        while chrono::Utc::now().timestamp_millis() <= lease_expiry {
            std::thread::sleep(Duration::from_millis(100));
        }
        for _ in 0..2 {
            let quarantined = invoke();
            assert_eq!(
                quarantined["data"]["receipts"][0]["outcome"],
                "pending_reconcile"
            );
        }
        let old_run_id = active["runId"].as_str().unwrap();
        let old = service
            .runtime()
            .storage
            .get_json(RUNS_NS, old_run_id)
            .unwrap()
            .unwrap();
        assert_eq!(old["run"]["state"], "pending_reconcile");
        let recovery = || {
            let mut command = binary();
            command
                .arg("--config")
                .arg(state.join("runtime.json"))
                .args(["agent", "recover", "nontrading-inspection"]);
            command
        };
        assert!(
            !recovery().output().unwrap().status.success(),
            "recovery requires explicit acknowledgment"
        );
        assert!(recovery()
            .arg("--acknowledge-reconciled")
            .output()
            .unwrap()
            .status
            .success());
        let recovered = service
            .runtime()
            .storage
            .get_json(RUNS_NS, old_run_id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered["run"]["state"], "reconciled");
        // Recovery releases a future tick, never the uncertain original turn.
        std::thread::sleep(Duration::from_secs(60));
    }
    let response = invoke();
    if response["data"]["receipts"][0]["outcome"] != "completed" {
        std::fs::remove_file(home.join("auth.json")).unwrap();
        let retained = root.keep();
        panic!(
            "runner did not complete; private diagnostic rig retained at {}",
            retained.display()
        );
    }
    // The first process has exited. Wait for an actual due tick, then start a
    // distinct runner process against the same durable installation and session.
    std::thread::sleep(Duration::from_secs(60));
    let resumed = invoke();
    assert_eq!(resumed["data"]["receipts"][0]["outcome"], "completed");
    assert_ne!(
        resumed["data"]["receipts"][0]["runId"],
        response["data"]["receipts"][0]["runId"]
    );
    let paused = binary()
        .arg("--config")
        .arg(state.join("runtime.json"))
        .args(["agent", "pause", "nontrading-inspection"])
        .output()
        .unwrap();
    assert!(paused.status.success());
    let inactive = invoke();
    assert_eq!(inactive["data"]["receipts"], json!([]));
    // Completion alone is not a successful tool call. Inspect only this temporary
    // test's session records; never the owner's conversation history.
    fn records(path: &Path, items: &mut Vec<Value>) {
        if !path.is_dir() {
            return;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                records(&path, items);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                for line in std::fs::read_to_string(path).unwrap().lines() {
                    if let Ok(value) = serde_json::from_str(line) {
                        items.push(value);
                    }
                }
            }
        }
    }
    let mut items = Vec::new();
    records(&home.join("sessions"), &mut items);
    let sessions: std::collections::BTreeSet<_> = items
        .iter()
        .filter(|item| item["type"] == "session_meta")
        .filter_map(|item| item["payload"]["id"].as_str())
        .collect();
    assert_eq!(
        sessions.len(),
        if interrupt { 2 } else { 1 },
        "second wake must resume the same actual Codex session"
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["payload"]["type"] == "task_complete")
            .count(),
        2,
        "paused runner must not start a third model turn"
    );
    let calls: Vec<&Value> = items
        .iter()
        .filter(|v| {
            let payload = &v["payload"];
            (payload["type"] == "function_call"
                && payload["name"].as_str().is_some_and(|name| {
                    name.contains("tradeassembly_runner_") && name.contains("deployment")
                }))
                || (payload["type"] == "custom_tool_call"
                    && payload["name"] == "exec"
                    && payload["input"].as_str().is_some_and(|input| {
                        // The isolated home has only our bound server. Code mode
                        // may discover its exact name through ALL_TOOLS instead
                        // of embedding that name as a literal in the program.
                        input.contains("tools")
                            && input.contains("deployment")
                            && input.contains("inspect")
                    }))
        })
        .collect();
    if calls.is_empty() {
        std::fs::remove_file(home.join("auth.json")).unwrap();
        let retained = root.keep();
        panic!(
            "no bound MCP call; private test records at {}",
            retained.display()
        );
    }
    let digest = tradeassembly_runtime::agent_runner::deployment_binding_digest(&deployment);
    let valid_response = |text: &str| {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return false;
        };
        let body = value.get("structuredContent").unwrap_or(&value);
        body["ok"] == true
            && body["schemaVersion"] == "tradeassembly.agent_mcp_lifecycle.v1"
            && body["deployments"].as_array().is_some_and(|rows| {
                rows.iter().any(|row| {
                    row["deploymentId"] == "nontrading-inspection" && row["bindingDigest"] == digest
                })
            })
    };
    let successful_receipts = calls
        .iter()
        .filter(|call| {
            items.iter().any(|item| {
                let payload = &item["payload"];
                if !matches!(
                    payload["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output")
                ) || payload["call_id"] != call["payload"]["call_id"]
                {
                    return false;
                }
                let output = &payload["output"];
                output.as_str().is_some_and(valid_response)
                    || output.as_array().is_some_and(|parts| {
                        parts
                            .iter()
                            .any(|part| part["text"].as_str().is_some_and(valid_response))
                    })
            })
        })
        .count();
    if successful_receipts < 2 {
        std::fs::remove_file(home.join("auth.json")).unwrap();
        let retained = root.keep();
        panic!("both wakes require verified MCP responses; found {successful_receipts}; private diagnostics {}", retained.display());
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn prepare(state: &Path, warden: &Path, port: u16) -> Value {
    let output = binary()
        .args(["setup", "--state-dir"])
        .arg(state)
        .arg("--warden-binary")
        .arg(warden)
        .arg("--warden-port")
        .arg(port.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[ignore = "requires explicitly selected real pinned Warden binary via F2_TEST_WARDEN_BINARY"]
fn real_binary_setup_and_policy_service_work_without_source_or_path() {
    let warden =
        std::env::var_os("F2_TEST_WARDEN_BINARY").expect("select a real pinned Warden binary");
    let warden = Path::new(&warden).canonicalize().unwrap();
    let root = tempfile::tempdir().unwrap();
    let state = root.path().canonicalize().unwrap().join("installation");
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    let first = prepare(&state, &warden, port);
    assert_eq!(first["ok"], true);
    assert_eq!(
        first["nextCommands"][0]["command"],
        executable().canonicalize().unwrap().display().to_string()
    );
    let config: Value =
        serde_json::from_slice(&std::fs::read(state.join("runtime.json")).unwrap()).unwrap();
    assert_eq!(config["oidcProfile"], "local_owner");
    let token_path = config["wardenTokenRef"]
        .as_str()
        .unwrap()
        .strip_prefix("file://")
        .unwrap();
    let token_before = std::fs::read(token_path).unwrap();
    prepare(&state, &warden, port);
    assert_eq!(std::fs::read(token_path).unwrap(), token_before);
    assert!(!first
        .to_string()
        .contains(std::str::from_utf8(&token_before).unwrap()));
    drop(reservation);
    let mut policy = OwnedProcess(
        binary()
            .args(["policy", "serve", "--state-dir"])
            .arg(&state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            policy.0.try_wait().unwrap().is_none(),
            "policy exited before health"
        );
        if let Ok(response) = client.get(format!("http://127.0.0.1:{port}/health")).send() {
            let health: Value = response.json().unwrap();
            assert_eq!(health["service"], "warden-sidecar");
            assert_eq!(health["status"], "ok");
            break;
        }
        assert!(Instant::now() < deadline, "policy health timeout");
        std::thread::sleep(Duration::from_millis(50));
    }
    // Exercise the distinct broker authorization transport with real Warden.
    // The release policy still denies Live submission: no broker is invoked.
    {
        use tradeassembly_runtime::finance_authority::{
            FinanceAuthorityPort, WardenSidecarAuthority, DECISION_NS, RECEIPT_NS,
            REQUIRED_WARDEN_SERVICE_VERSION,
        };
        use tradeassembly_runtime::ports::StoragePort;
        let authority = WardenSidecarAuthority::new(
            format!("http://127.0.0.1:{port}"),
            std::str::from_utf8(&token_before).unwrap().trim(),
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .unwrap();
        let storage = tradeassembly_runtime::adapters::local::sqlite::LocalSqliteStorage::new(
            root.path()
                .join("broker-policy-proof.sqlite")
                .display()
                .to_string(),
        );
        let mut envelope = tradeassembly_runtime::control_plane::http_envelope(
            "plugin_operation_boundary",
            "POST",
            "/orders",
            &serde_json::json!({"idempotencyKey": "real-warden-live-denial"}),
        )
        .unwrap();
        envelope.command_name = "order.submit.live".into();
        envelope.side_effect_class = "live_order".into();
        envelope.authority.account_mode = "live".into();
        envelope.target_object = Some("account://controlled-test".into());
        assert_eq!(
            authority
                .prepare_broker_submission(&storage, &envelope)
                .unwrap_err(),
            "denied:broker_submission_policy"
        );
        let decision = storage
            .get_json(DECISION_NS, &envelope.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            decision["request"]["pep_id"],
            "pep-tradeassembly-broker-submission"
        );
        assert_eq!(decision["request"]["pep_coverage"], "c5");
        assert_eq!(decision["decision"]["decision"], "deny");
        let receipt = storage
            .get_json(RECEIPT_NS, &envelope.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            receipt["receipt"]["pep_id"],
            "pep-tradeassembly-broker-submission"
        );
        assert_eq!(receipt["signature"]["algorithm"], "ed25519");
        assert!(authority
            .complete_broker_submission(&storage, &envelope, 202)
            .is_err());
    }
    let mut duplicate = OwnedProcess(
        binary()
            .args(["policy", "serve", "--state-dir"])
            .arg(&state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = duplicate.0.try_wait().unwrap() {
            assert!(!status.success(), "occupied policy port must fail");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "duplicate policy process did not fail"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        policy.0.try_wait().unwrap().is_none(),
        "existing policy was interrupted"
    );
    let mut mcp = binary()
        .arg("--config")
        .arg(state.join("runtime.json"))
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = mcp.stdin.take().unwrap();
    for message in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tradeassembly.strategy.list","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tradeassembly.setup.inspect","arguments":{"surface":"agent"}}}),
    ] {
        writeln!(input, "{message}").unwrap();
    }
    drop(input);
    let output = mcp.wait_with_output().unwrap();
    assert!(output.status.success());
    let messages: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        messages[1]["result"]["structuredContent"]["ok"], true,
        "{}",
        messages[1]
    );
    assert!(messages[1]["result"]["structuredContent"]["strategies"].is_array());
    assert_eq!(
        messages[2]["result"]["structuredContent"]["identity"]["authenticated"],
        true
    );
    assert_eq!(
        messages[2]["result"]["structuredContent"]["acceptance"]["complete"],
        false
    );
    assert_eq!(
        messages[2]["result"]["structuredContent"]["automationReady"],
        false
    );
    if std::env::var_os("F2_TEST_TRADEASSEMBLY_BINARY").is_some() {
        assert_eq!(first["sandboxConfigured"], true);
        let installed = binary()
            .arg("--config")
            .arg(state.join("runtime.json"))
            .args(["plugins", "install-default", "--offline"])
            .output()
            .unwrap();
        assert!(
            installed.status.success(),
            "bundled Alpaca install failed: {}",
            String::from_utf8_lossy(&installed.stdout)
        );
        let installed: Value = serde_json::from_slice(&installed.stdout).unwrap();
        assert_eq!(installed["ok"], true);
        assert!(installed.to_string().contains("tradeassembly.alpaca"));
        assert!(installed.to_string().contains("0.1.9"));
        // A new user must be able to inspect the installed connector before
        // providing brokerage credentials. Exercise the real Core host boundary.
        for args in [
            vec![
                "plugins",
                "create",
                "--plugin-ref",
                "tradeassembly.alpaca",
                "--instance-ref",
                "discovery-fixture",
            ],
            vec!["plugins", "enable", "discovery-fixture"],
        ] {
            let output = binary()
                .arg("--config")
                .arg(state.join("runtime.json"))
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "plugin setup failed: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        let request = root.path().join("capability-request.json");
        std::fs::write(
            &request,
            serde_json::to_vec(&json!({
                "mode":"paper", "purpose":"plugin_administration",
                "idempotencyKey":"fixture-capability-discovery", "payload":{}
            }))
            .unwrap(),
        )
        .unwrap();
        let output = binary()
            .arg("--config")
            .arg(state.join("runtime.json"))
            .args([
                "plugins",
                "invoke",
                "discovery-fixture",
                "plugin.capability_matrix",
                "--request-file",
            ])
            .arg(request)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "host discovery failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(
            response["data"]["response"]["payload"]["pluginId"],
            "tradeassembly.alpaca"
        );
        let capabilities = &response["data"]["response"]["payload"];
        assert_eq!(capabilities["liveOrderSubmit"], true);
        for operation in ["broker.paper_order_submit", "broker.live_order_submit"] {
            assert!(capabilities["operations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == operation));
        }
        assert_eq!(
            response["data"]["response"]["schemaRef"],
            "schema://tradeassembly.alpaca/capability-response@1"
        );
        assert!(response["data"]["response"]["sourceEventId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
        assert!(response["data"]["response"]["contentHash"]
            .as_str()
            .is_some_and(|hash| !hash.is_empty()));
    }
    let profile = tradeassembly_runtime::agent_launchd::LaunchdProfile {
        executable: executable(),
        // No deployed strategies exist; this test must never call a model.
        codex_executable: executable(),
        runtime_config_path: state.join("runtime.json"),
        database_path: state.join("runtime.db"),
        log_directory: state.clone(),
        runner_id: "empty-config-proof".into(),
    };
    let arguments = profile.program_arguments().unwrap();
    let runner = binary()
        .args(&arguments[1..])
        .arg("--once")
        .output()
        .unwrap();
    assert!(
        runner.status.success(),
        "empty runner failed: {}",
        String::from_utf8_lossy(&runner.stdout)
    );
    let runner: Value = serde_json::from_slice(&runner.stdout).unwrap();
    assert_eq!(runner["ok"], true);
    assert_eq!(runner["data"]["receipts"], json!([]));
    drop(policy);
    prepare(&state, &warden, port);
    assert_eq!(std::fs::read(token_path).unwrap(), token_before);
}
