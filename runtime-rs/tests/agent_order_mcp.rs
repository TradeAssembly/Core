//! Opt-in installed-binary MCP order proof. No real broker is contacted.

use tradeassembly_runtime as runtime_crate;
#[path = "common/controlled_broker_package.rs"]
#[allow(dead_code)]
mod controlled_broker_package;
#[path = "common/controlled_warden.rs"]
#[allow(dead_code)]
mod controlled_warden;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tradeassembly_runtime::agent_runner::{self, AgentDeployment, AgentExecutor};
use tradeassembly_runtime::local_owner_identity::LocalOwnerIdentity;
use tradeassembly_runtime::runtime_config::{RuntimeConfig, RuntimeConfigLayer};
use tradeassembly_runtime::service::TradeAssemblyService;

struct ControlledHttpSink {
    url: String,
    db: PathBuf,
    state: Arc<ControlledSinkState>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct ControlledSinkState {
    db: PathBuf,
    drop_next_response: AtomicBool,
}

impl ControlledHttpSink {
    fn start(root: &std::path::Path) -> Self {
        let db = root.join("controlled-sink.sqlite");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://127.0.0.1:{}/orders",
            listener.local_addr().unwrap().port()
        );
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let state = Arc::new(ControlledSinkState {
            db: db.clone(),
            drop_next_response: AtomicBool::new(false),
        });
        let server_state = Arc::clone(&state);
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let app = Router::new()
                    .route("/orders", post(controlled_http_order))
                    .with_state(server_state);
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
        });
        Self {
            url,
            db,
            state,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn drop_next_response(&self) {
        self.state.drop_next_response.store(true, Ordering::SeqCst);
    }
}

impl Drop for ControlledHttpSink {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn controlled_http_order(
    State(state): State<Arc<ControlledSinkState>>,
    Json(request): Json<Value>,
) -> Result<Response, StatusCode> {
    let body = &request["body"];
    let client_id = body["clientOrderId"]
        .as_str()
        .ok_or(StatusCode::BAD_REQUEST)?;
    let account_ref = request["accountRef"]
        .as_str()
        .ok_or(StatusCode::BAD_REQUEST)?;
    let is_lookup = request["operation"] == "broker.order_lookup.paper"
        || request["operation"] == "broker.order_lookup";
    if is_lookup && !state.db.exists() {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut db = Connection::open(&state.db).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    db.busy_timeout(Duration::from_secs(2))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !is_lookup {
        db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS orders (client_id TEXT PRIMARY KEY, account_ref TEXT NOT NULL, provider_order_id TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL, quantity TEXT NOT NULL, order_type TEXT NOT NULL, time_in_force TEXT NOT NULL, digest TEXT NOT NULL, submissions INTEGER NOT NULL);")
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(body).unwrap()));
        let tx = db
            .transaction()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tx.execute("INSERT INTO orders(client_id,account_ref,provider_order_id,symbol,side,quantity,order_type,time_in_force,digest,submissions) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1) ON CONFLICT(client_id) DO UPDATE SET submissions=submissions+1",
            params![client_id,account_ref,client_id,body["symbol"].as_str(),body["side"].as_str(),body["quantity"].as_str(),body["orderType"].as_str(),body["timeInForce"].as_str(),digest])
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        tx.commit().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    let result: (String, String, String, String, String, String, String, String, i64) = db
        .query_row("SELECT account_ref,provider_order_id,symbol,side,quantity,order_type,time_in_force,digest,submissions FROM orders WHERE client_id=?1",[client_id],|row| {
            Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?))
        })
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if !is_lookup && state.drop_next_response.swap(false, Ordering::SeqCst) {
        let broken = Body::from_stream(futures_util::stream::once(async {
            Err::<Bytes, std::io::Error>(std::io::Error::other(
                "controlled response dropped after sink commit",
            ))
        }));
        return Ok(Response::builder().status(200).body(broken).unwrap());
    }
    Ok(Json(json!({"accountRef":result.0,"clientOrderId":client_id,
        "providerOrderId":result.1,"symbol":result.2,"side":result.3,
        "quantity":result.4,"orderType":result.5,"timeInForce":result.6,
        "status":"accepted","intentDigest":result.7,"submissionCount":result.8}))
    .into_response())
}

struct McpClient {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Value>,
    sequence: u64,
}

impl McpClient {
    fn start(binary: &PathBuf, config: &PathBuf, root: &std::path::Path) -> Self {
        let mut child = Command::new(binary)
            .arg("--config")
            .arg(config)
            .args(["mcp", "serve"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", root)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(value) = serde_json::from_str(&line) {
                    if sender.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            child,
            input: Some(input),
            output,
            sequence: 0,
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.sequence += 1;
        let id = self.sequence;
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let response = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("installed MCP binary responded before deadline");
            if response["id"] == id {
                return response;
            }
            assert_eq!(response["method"], "notifications/tools/list_changed");
        }
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.call("tools/call", json!({"name":name,"arguments":arguments}))["result"].clone()
    }

    fn close(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(
                Instant::now() < deadline,
                "installed MCP binary did not close"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn crash(&mut self) {
        self.child.kill().unwrap();
        assert!(!self.child.wait().unwrap().success());
        self.input.take();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires explicit candidate Core, Warden, controlled broker, and SRT binaries"]
fn installed_stdio_paper_order_recovers_ambiguous_crash_once() {
    let candidate = PathBuf::from(
        std::env::var_os("F2_TEST_RUNTIME_BINARY").expect("candidate Core binary required"),
    )
    .canonicalize()
    .unwrap();
    let controlled = PathBuf::from(
        std::env::var_os("F2_TEST_CONTROLLED_BROKER_BINARY")
            .expect("controlled broker binary required"),
    )
    .canonicalize()
    .unwrap();
    let srt = PathBuf::from(std::env::var_os("F2_TEST_SRT_CLI").expect("SRT binary required"))
        .canonicalize()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut warden = controlled_warden::ControlledWarden::start(&root);
    let sink = ControlledHttpSink::start(&root);
    let db = root.join("runtime.db");
    let layer = RuntimeConfigLayer {
        profile: Some("local".into()),
        database_path: Some(db.display().to_string()),
        artifact_root: Some(root.join("artifacts").display().to_string()),
        oidc_profile: Some("local_owner".into()),
        warden_sidecar_url: Some(format!("http://127.0.0.1:{}", warden.port)),
        warden_token_ref: Some(format!(
            "file://{}",
            root.join("warden/warden.token").display()
        )),
        plugin_sandbox_command: Some(srt.display().to_string()),
        plugin_sandbox_allow_local_egress: Some(true),
        ..Default::default()
    };
    let config_path = root.join("runtime.json");
    std::fs::write(&config_path, serde_json::to_vec(&layer).unwrap()).unwrap();
    let config =
        RuntimeConfig::resolve(Some(layer), Default::default(), Default::default()).unwrap();
    let owner = LocalOwnerIdentity::for_database(&db).unwrap();
    let service = TradeAssemblyService::from_config(config)
        .unwrap()
        .for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
    let _package = controlled_broker_package::install_controlled_package_for_mode_and_sink(
        &service,
        &root,
        &controlled,
        "paper",
        Some(&sink.url),
    );
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({"strategyId":"strat_local_btc_demo","orchestrator":"external_agent",
            "mode":"paper","allowedSymbols":["BTC/USD"],
            "providerRef":"mandate-paper","accountRef":"account://mandate-paper/controlled",
            "dataProviderRef":"mandate-paper","dataAccountRef":"account://mandate-paper/controlled",
            "riskLimits":{"max_notional":10,"max_order_quantity":2}}),
    );
    assert_eq!(saved.status, 200, "{saved:#?}");
    let config_id = saved.body["body"]["configId"].as_str().unwrap();
    let deployment = AgentDeployment {
        executor: AgentExecutor::ExternalClient,
        deployment_id: "stdio-controlled-paper".into(),
        system_project_id: "system-1".into(),
        agent_definition_version_id: "agent-v1".into(),
        execution_config_version_id: config_id.into(),
        studio_tool_allowlist: vec![
            "tradeassembly.order.submit".into(),
            "tradeassembly.order.reconcile".into(),
        ],
        desired_state: "active".into(),
        interval_seconds: 60,
        cron_utc: None,
        mode: "paper".into(),
        prompt: String::new(),
        workspace: String::new(),
        runtime_profile: "local-read-only".into(),
    };
    agent_runner::put_deployment(&service.runtime(), &deployment).unwrap();
    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({"configId":config_id,"idempotencyKey":"stdio-paper-activation",
            "acknowledgementIds":["user_logic","user_risk","no_advice"]}),
    );
    assert_eq!(activated.status, 200, "{activated:#?}");
    let activation_id = activated.body["body"]["activationId"].as_str().unwrap();
    let mut client = McpClient::start(&candidate, &config_path, &root);
    assert!(client.call("initialize", json!({}))["result"].is_object());
    let attached = client.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-paper-attach"}),
    );
    assert_eq!(attached["isError"], false, "{attached:#?}");
    let order = json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-paper",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-paper-order","quantity":"1"},
        "idempotency_key":"stdio-paper-order"});
    let mut over_limit = order.clone();
    over_limit["order"]["clientOrderId"] = json!("stdio-over-limit");
    over_limit["order"]["quantity"] = json!("3");
    over_limit["idempotency_key"] = json!("stdio-over-limit");
    let denied = client.tool("tradeassembly.order.submit", over_limit);
    assert_eq!(
        denied["structuredContent"]["error"]["code"], "agent_order_submission_denied",
        "{denied:#?}"
    );
    assert!(
        !sink.db.exists(),
        "denied order cannot reach the broker sink"
    );
    let first = client.tool("tradeassembly.order.submit", order.clone());
    assert_eq!(
        first["structuredContent"]["status"], "submitted",
        "{first:#?}"
    );
    let repeated = client.tool("tradeassembly.order.submit", order.clone());
    assert_eq!(repeated, first);
    let mut conflicting_order = order.clone();
    conflicting_order["order"]["quantity"] = json!("0.5");
    let conflict = client.tool("tradeassembly.order.submit", conflicting_order);
    assert_eq!(
        conflict["structuredContent"]["error"]["code"], "agent_order_idempotency_conflict",
        "{conflict:#?}"
    );
    client.close();
    let mut restarted = McpClient::start(&candidate, &config_path, &root);
    assert!(restarted.call("initialize", json!({}))["result"].is_object());
    let pending = restarted.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-paper-reattach"}),
    );
    assert_eq!(
        pending["structuredContent"]["error"]["code"], "agent_run_pending_reconcile",
        "{pending:#?}"
    );
    let owner_observation = restarted.tool(
        "tradeassembly.order.observe",
        json!({"original_idempotency_key":"stdio-paper-order",
            "idempotency_key":"stdio-owner-observation"}),
    );
    assert_eq!(
        owner_observation["structuredContent"]["state"], "observed",
        "{owner_observation:#?}"
    );
    assert_eq!(
        owner_observation["structuredContent"]["receipt"]["payload"]["submissionCount"],
        1
    );
    let recovery = restarted.tool(
        "studio.agent_run.recover",
        json!({"deployment_id":deployment.deployment_id,
            "acknowledge_reconciled":true,"idempotency_key":"stdio-owner-recovery",
            "authority_context":{"actor":"ignored","surface":"mcp","accountMode":"paper"}}),
    );
    assert_eq!(recovery["isError"], false, "{recovery:#?}");
    let reattached = restarted.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-paper-reattach"}),
    );
    assert_eq!(reattached["isError"], false, "{reattached:#?}");
    let owner_tool_as_agent = restarted.tool(
        "tradeassembly.order.observe",
        json!({"original_idempotency_key":"stdio-paper-order",
            "idempotency_key":"stdio-agent-owner-observation"}),
    );
    assert_eq!(
        owner_tool_as_agent["structuredContent"]["error"]["code"], "agent_mcp_tool_not_allowed",
        "{owner_tool_as_agent:#?}"
    );
    let after_restart = restarted.tool("tradeassembly.order.submit", order);
    assert_eq!(after_restart, first);
    let ambiguous = json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-paper",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-ambiguous-order","quantity":"1"},
        "idempotency_key":"stdio-ambiguous-order"});
    sink.drop_next_response();
    let lost = restarted.tool("tradeassembly.order.submit", ambiguous.clone());
    assert_eq!(
        lost["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{lost:#?}"
    );
    let retry = restarted.tool("tradeassembly.order.submit", ambiguous);
    assert_eq!(
        retry["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{retry:#?}"
    );
    let observed = restarted.tool(
        "tradeassembly.order.reconcile",
        json!({"original_idempotency_key":"stdio-ambiguous-order",
            "idempotency_key":"stdio-ambiguous-observation"}),
    );
    assert_eq!(
        observed["structuredContent"]["state"], "observed",
        "{observed:#?}"
    );
    assert_eq!(
        observed["structuredContent"]["receipt"]["payload"]["submissionCount"],
        1
    );
    let interrupted = json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-paper",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-crash-order","quantity":"1"},
        "idempotency_key":"stdio-crash-order"});
    sink.drop_next_response();
    let uncertain = restarted.tool("tradeassembly.order.submit", interrupted.clone());
    assert_eq!(
        uncertain["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{uncertain:#?}"
    );
    restarted.crash();
    let mut after_crash = McpClient::start(&candidate, &config_path, &root);
    assert!(after_crash.call("initialize", json!({}))["result"].is_object());
    let stale = after_crash.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-crash-reattach"}),
    );
    assert_eq!(
        stale["structuredContent"]["error"]["code"], "agent_run_pending_reconcile",
        "{stale:#?}"
    );
    let owner_found = after_crash.tool(
        "tradeassembly.order.observe",
        json!({"original_idempotency_key":"stdio-crash-order",
            "idempotency_key":"stdio-crash-observation"}),
    );
    assert_eq!(
        owner_found["structuredContent"]["state"], "observed",
        "{owner_found:#?}"
    );
    assert_eq!(
        owner_found["structuredContent"]["receipt"]["payload"]["submissionCount"],
        1
    );
    warden.assert_healthy();
    let lease = service
        .runtime()
        .leases
        .current(&format!("agent-deployment:{}", deployment.deployment_id))
        .unwrap()
        .unwrap();
    let early_recovery = after_crash.tool(
        "studio.agent_run.recover",
        json!({"deployment_id":deployment.deployment_id,
            "acknowledge_reconciled":true,"idempotency_key":"stdio-crash-owner-early",
            "authority_context":{"actor":"ignored","surface":"mcp","accountMode":"paper"}}),
    );
    assert_eq!(
        early_recovery["structuredContent"]["error"]["code"], "agent_recovery_lease_held",
        "{early_recovery:#?}"
    );
    let recovery_deadline = Instant::now() + Duration::from_secs(130);
    while service.runtime().clock.trusted_now_ms().unwrap() <= lease.expires_at_ms {
        assert!(
            Instant::now() < recovery_deadline,
            "abandoned lease did not expire"
        );
        std::thread::sleep(Duration::from_secs(2));
    }
    let owner_recovery = after_crash.tool(
        "studio.agent_run.recover",
        json!({"deployment_id":deployment.deployment_id,
            "acknowledge_reconciled":true,"idempotency_key":"stdio-crash-owner-recovered",
            "authority_context":{"actor":"ignored","surface":"mcp","accountMode":"paper"}}),
    );
    assert_eq!(owner_recovery["isError"], false, "{owner_recovery:#?}");
    let recovered_attach = after_crash.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-crash-reattach"}),
    );
    assert_eq!(recovered_attach["isError"], false, "{recovered_attach:#?}");
    let replay_after_crash = after_crash.tool("tradeassembly.order.submit", interrupted);
    assert_eq!(
        replay_after_crash["structuredContent"]["status"], "submitted",
        "{replay_after_crash:#?}"
    );
    let evidence =
        rusqlite::Connection::open_with_flags(&sink.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let counts: (i64, i64) = evidence
        .query_row("SELECT COUNT(*), SUM(submissions) FROM orders", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(counts, (3, 3));
    let ambiguous_submissions: i64 = evidence
        .query_row(
            "SELECT submissions FROM orders WHERE client_id='stdio-ambiguous-order'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ambiguous_submissions, 1);
    for key in [
        "stdio-paper-order",
        "stdio-ambiguous-order",
        "stdio-crash-order",
    ] {
        let intent_key = format!("intent_{:x}", Sha256::digest(key.as_bytes()));
        assert!(
            service
                .runtime()
                .storage
                .get_json("broker_order_intents", &intent_key)
                .unwrap()
                .is_some(),
            "missing durable intent for {key}"
        );
        assert!(
            service
                .runtime()
                .storage
                .get_json("plugin_operation_receipts", key)
                .unwrap()
                .is_some(),
            "missing durable receipt for {key}"
        );
    }
    for namespace in ["scheduler_state", "execution_ticks"] {
        assert!(service
            .runtime()
            .storage
            .list_json(namespace)
            .unwrap()
            .is_empty());
    }
    let mut paused = deployment.clone();
    paused.desired_state = "paused".into();
    agent_runner::put_deployment(&service.runtime(), &paused).unwrap();
    let revoked = after_crash.tool(
        "tradeassembly.order.submit",
        json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-paper",
            "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
                "timeInForce":"gtc","clientOrderId":"stdio-revoked-order","quantity":"1"},
            "idempotency_key":"stdio-revoked-order"}),
    );
    assert_eq!(revoked["isError"], true, "{revoked:#?}");
    assert_eq!(
        evidence
            .query_row("SELECT COUNT(*) FROM orders", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    after_crash.close();
    warden.stop();
}
