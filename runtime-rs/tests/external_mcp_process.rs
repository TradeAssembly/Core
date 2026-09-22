//! Opt-in real-Warden/real-stdio attachment proof; no broker order is submitted.
use tradeassembly_runtime as runtime_crate;
#[allow(dead_code)]
#[path = "common/controlled_warden.rs"]
mod controlled_warden;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use tradeassembly_runtime::local_owner_identity::LocalOwnerIdentity;
use tradeassembly_runtime::runtime_config::{RuntimeConfig, RuntimeConfigLayer};
use tradeassembly_runtime::service::TradeAssemblyService;

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    next: u64,
}
impl Client {
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let message = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("MCP response before deadline");
            if message["id"] == id {
                return message;
            }
            assert_eq!(message["method"], "notifications/tools/list_changed");
        }
    }
    fn tool(&mut self, name: &str, args: Value) -> Value {
        self.call("tools/call", json!({"name":name,"arguments":args}))["result"].clone()
    }
    fn eof(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "MCP failed to close after EOF");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires explicit F2_TEST_WARDEN_BINARY and F2_TEST_RUNTIME_BINARY"]
fn real_stdio_attaches_to_real_warden_activation_and_quarantines_on_eof() {
    let binary = std::env::var_os("F2_TEST_RUNTIME_BINARY").expect("explicit runtime binary");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let _warden = controlled_warden::ControlledWarden::start(&root);
    let db = root.join("runtime.db");
    let layer = RuntimeConfigLayer {
        profile: Some("local".into()),
        database_path: Some(db.display().to_string()),
        artifact_root: Some(root.join("artifacts").display().to_string()),
        oidc_profile: Some("local_owner".into()),
        warden_sidecar_url: Some(format!("http://127.0.0.1:{}", _warden.port)),
        warden_token_ref: Some(format!(
            "file://{}",
            root.join("warden/warden.token").display()
        )),
        ..Default::default()
    };
    let path = root.join("runtime.json");
    std::fs::write(&path, serde_json::to_vec(&layer).unwrap()).unwrap();
    let config =
        RuntimeConfig::resolve(Some(layer), Default::default(), Default::default()).unwrap();
    let owner = LocalOwnerIdentity::for_database(&db).unwrap();
    let service = TradeAssemblyService::from_config(config)
        .unwrap()
        .for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
    let saved = service.handle_http("POST","/product/strategy-execution-configs/save",json!({
        "strategyId":"strat_local_btc_demo","orchestrator":"external_agent","mode":"paper","providerRef":"sim","accountRef":"controlled-paper",
        "riskLimits":{"max_notional":25,"max_order_quantity":0.0003}
    }));
    assert_eq!(saved.status, 200, "config error: {}", saved.body["error"]);
    let config_id = saved.body["body"]["configId"]
        .as_str()
        .expect("saved config id");
    let activated = service.handle_http("POST","/product/strategy-execution-activations/activate",json!({
        "configId":config_id,"idempotencyKey":"real-activation","acknowledgementIds":["user_logic","user_risk","no_advice"]
    }));
    assert_eq!(
        activated.body["body"]["status"], "active",
        "activation error/blockers: {} {}",
        activated.body["error"]["code"], activated.body["error"]["details"]["blockedReasons"]
    );
    let activation_id = activated.body["body"]["activationId"].as_str().unwrap();
    let deployment = service.call_mcp_tool("studio.deployment.create",json!({
        "deployment":{"executor":"external_client","deploymentId":"process-external","systemProjectId":"system-local","agentDefinitionVersionId":"agent-v1","executionConfigVersionId":config_id,"studioToolAllowlist":["tradeassembly.health"],"desiredState":"active","mode":"paper"},
        "idempotency_key":"create-external","authority_context":{"actor":"ignored","surface":"mcp","accountMode":"paper"}
    }));
    assert_eq!(
        deployment["isError"], false,
        "deployment error: {}",
        deployment["structuredContent"]["error"]
    );
    let start_client = || {
        let mut child = Command::new(&binary)
            .args(["--config", path.to_str().unwrap(), "mcp", "serve"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &root)
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str(&line) {
                    if send.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        Client {
            child,
            input: Some(input),
            output,
            next: 0,
        }
    };
    let mut client = start_client();
    assert!(client.call("initialize", json!({}))["result"].is_object());
    let args = json!({"deployment_id":"process-external","activation_id":activation_id,"idempotency_key":"attach-process"});
    let attached = client.tool("tradeassembly.agent.session.attach", args.clone());
    assert_eq!(
        attached["isError"], false,
        "attach error: {}",
        attached["structuredContent"]["error"]
    );
    assert_eq!(
        client.tool("tradeassembly.agent.session.attach", args.clone()),
        attached
    );
    let tools = client.call("tools/list", json!({}));
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"tradeassembly.health"));
    assert_eq!(
        client.tool("tradeassembly.health", json!({}))["isError"],
        false
    );
    assert_eq!(
        client.tool(
            "studio.deployment.stop",
            json!({"deployment_id":"process-external"})
        )["isError"],
        true
    );
    client.eof();
    let active = service
        .runtime()
        .storage
        .get_json("agent_active_runs", "process-external")
        .unwrap()
        .unwrap();
    assert_eq!(active["state"], "pending_reconcile");
    let mut restarted = start_client();
    assert!(restarted.call("initialize", json!({}))["result"].is_object());
    let rejected = restarted.tool("tradeassembly.agent.session.attach", args.clone());
    assert_eq!(
        rejected["structuredContent"]["error"]["code"],
        "agent_run_pending_reconcile"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("agent_runs")
            .unwrap()
            .len(),
        1
    );
    let recovery = service.call_mcp_tool("studio.agent_run.recover",json!({
        "deployment_id":"process-external","acknowledge_reconciled":true,"idempotency_key":"owner-recovery",
        "authority_context":{"actor":"ignored","surface":"mcp","accountMode":"paper"}
    }));
    assert_eq!(
        recovery["isError"], false,
        "recovery error: {}",
        recovery["structuredContent"]["error"]
    );
    let reattached = restarted.tool("tradeassembly.agent.session.attach", args);
    assert_eq!(
        reattached["isError"], false,
        "reattach error: {}",
        reattached["structuredContent"]["error"]
    );
    assert_ne!(
        reattached["structuredContent"]["runId"],
        attached["structuredContent"]["runId"]
    );
    assert_eq!(
        restarted.tool("tradeassembly.agent.session.detach", json!({}))["isError"],
        false
    );
    restarted.eof();
    for ns in ["execution_ticks", "scheduler_state", "broker_order_intents"] {
        assert!(
            service.runtime().storage.list_json(ns).unwrap().is_empty(),
            "unexpected side effect in {ns}"
        );
    }
    assert!(
        !service
            .runtime()
            .storage
            .list_json(tradeassembly_runtime::finance_authority::RECEIPT_NS)
            .unwrap()
            .is_empty(),
        "real Warden receipts required"
    );
}
