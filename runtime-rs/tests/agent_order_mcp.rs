//! Opt-in installed-binary MCP order proof. No real broker is contacted.

use tradeassembly_runtime as runtime_crate;
#[path = "common/controlled_broker_package.rs"]
#[allow(dead_code)]
mod controlled_broker_package;
#[path = "common/controlled_legal_receipt.rs"]
mod controlled_legal_receipt;
#[path = "common/controlled_warden.rs"]
#[allow(dead_code)]
mod controlled_warden;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, post},
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
    atomic::{AtomicBool, AtomicU64, Ordering},
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
    reject_next_submit: AtomicBool,
    fail_next_lookup: AtomicBool,
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
            reject_next_submit: AtomicBool::new(false),
            fail_next_lookup: AtomicBool::new(false),
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

    fn reject_next_submit(&self) {
        self.state.reject_next_submit.store(true, Ordering::SeqCst);
    }

    fn fail_next_lookup(&self) {
        self.state.fail_next_lookup.store(true, Ordering::SeqCst);
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

// Test-only loopback policy adapter. Core still sends its embedded shipping
// bundle; only this disposable Warden instance receives the two Live allows.
struct ControlledLivePolicyProxy {
    url: String,
    state: Arc<ControlledPolicyState>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct ControlledPolicyState {
    warden_url: String,
    installs: AtomicU64,
    client: reqwest::Client,
}

impl ControlledLivePolicyProxy {
    fn start(warden_port: u16) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = Arc::new(ControlledPolicyState {
            warden_url: format!("http://127.0.0.1:{warden_port}"),
            installs: AtomicU64::new(0),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(4))
                .build()
                .unwrap(),
        });
        let server_state = Arc::clone(&state);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(
                    listener,
                    Router::new()
                        .fallback(any(controlled_warden_proxy_request))
                        .with_state(server_state),
                )
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
            });
        });
        Self {
            url,
            state,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn assert_used(&self) {
        assert!(
            self.state.installs.load(Ordering::SeqCst) > 0,
            "private Live policy was never installed in real Warden"
        );
    }
}

impl Drop for ControlledLivePolicyProxy {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn controlled_warden_proxy_request(
    State(state): State<Arc<ControlledPolicyState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, StatusCode> {
    let path = uri.path();
    if path != "/health"
        && path != "/v1/policies"
        && path != "/v1/actions/authorize"
        && !path.starts_with("/v1/receipts/")
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let payload = if method == Method::POST && path == "/v1/policies" {
        let mut policy: Value =
            serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
        if policy["schema_version"] != "apf.policy_bundle.v1"
            || policy["bundle_id"] != "tradeassembly-studio-core"
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        let rules = policy["rules"]
            .as_array_mut()
            .ok_or(StatusCode::BAD_REQUEST)?;
        for action in ["execution.activate.live", "order.submit.live"] {
            let rule = rules
                .iter_mut()
                .find(|rule| rule["action"] == action)
                .ok_or(StatusCode::BAD_REQUEST)?;
            if rule["required_decision"] != "deny" {
                return Err(StatusCode::BAD_REQUEST);
            }
            rule["required_decision"] = json!("allow");
        }
        policy["version"] = json!("2099-01-01.fixture-live");
        state.installs.fetch_add(1, Ordering::SeqCst);
        serde_json::to_vec(&policy).map_err(|_| StatusCode::BAD_REQUEST)?
    } else {
        body.to_vec()
    };
    let mut request = state.client.request(
        reqwest::Method::from_bytes(method.as_str().as_bytes())
            .map_err(|_| StatusCode::BAD_REQUEST)?,
        format!("{}{}", state.warden_url, uri),
    );
    if let Some(authorization) = headers.get(axum::http::header::AUTHORIZATION) {
        request = request.header(
            reqwest::header::AUTHORIZATION,
            authorization
                .to_str()
                .map_err(|_| StatusCode::BAD_REQUEST)?,
        );
    }
    if method == Method::POST {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload);
    }
    let response = request.send().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let status =
        StatusCode::from_u16(response.status().as_u16()).map_err(|_| StatusCode::BAD_GATEWAY)?;
    let bytes = response
        .bytes()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    Ok(Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .unwrap())
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
    if is_lookup && state.fail_next_lookup.swap(false, Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if !is_lookup && state.reject_next_submit.swap(false, Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
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

    fn tool_pair_in_flight(&mut self, name: &str, arguments: Value) -> [Value; 2] {
        let first_id = self.sequence + 1;
        let second_id = self.sequence + 2;
        self.sequence = second_id;
        let input = self.input.as_mut().unwrap();
        for id in [first_id, second_id] {
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
                    "params":{"name":name,"arguments":arguments}})
            )
            .unwrap();
        }
        input.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut results = [None, None];
        while results.iter().any(Option::is_none) {
            let response = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("both in-flight MCP calls answered before deadline");
            if response["id"] == first_id {
                results[0] = Some(response["result"].clone());
            } else if response["id"] == second_id {
                results[1] = Some(response["result"].clone());
            } else {
                assert_eq!(response["method"], "notifications/tools/list_changed");
            }
        }
        [results[0].take().unwrap(), results[1].take().unwrap()]
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
    assert_eq!(
        activated.status, 200,
        "code={} blockers={:?}",
        activated.body["error"]["code"], activated.body["error"]["details"]["blockedReasons"]
    );
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
    let absent_order = json!({"activation_id":activation_id,
        "plugin_instance_ref":"mandate-paper",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-absent-order","quantity":"1"},
        "idempotency_key":"stdio-absent-order"});
    sink.reject_next_submit();
    let absent_submit = after_crash.tool("tradeassembly.order.submit", absent_order.clone());
    assert_eq!(
        absent_submit["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{absent_submit:#?}"
    );
    let absent_lookup = after_crash.tool(
        "tradeassembly.order.reconcile",
        json!({"original_idempotency_key":"stdio-absent-order",
            "idempotency_key":"stdio-absent-observation"}),
    );
    assert_eq!(
        absent_lookup["structuredContent"]["error"]["code"], "plugin_order_reconciliation_required",
        "{absent_lookup:#?}"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(
                "plugin_broker_recovery_outcomes",
                "stdio-absent-observation"
            )
            .unwrap()
            .unwrap()["state"],
        "absent"
    );
    let absent_replay = after_crash.tool("tradeassembly.order.submit", absent_order);
    assert_eq!(
        absent_replay["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{absent_replay:#?}"
    );
    let blocked_after_absence = json!({"activation_id":activation_id,
        "plugin_instance_ref":"mandate-paper",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-after-absent-order","quantity":"1"},
        "idempotency_key":"stdio-after-absent-order"});
    // Absence fences replay of the original key, not independent future
    // orders. Deny this distinct order explicitly at the controlled sink.
    sink.reject_next_submit();
    let blocked = after_crash.tool("tradeassembly.order.submit", blocked_after_absence);
    assert_eq!(
        blocked["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{blocked:#?}"
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
    let absent_submissions: i64 = evidence
        .query_row(
            "SELECT COUNT(*) FROM orders WHERE client_id='stdio-absent-order'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(absent_submissions, 0);
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

#[test]
#[ignore = "requires explicit candidate Core, Warden, controlled broker, and SRT binaries"]
fn installed_stdio_unknown_lookup_fails_closed_without_replay() {
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
        deployment_id: "stdio-controlled-unknown".into(),
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
        json!({"configId":config_id,"idempotencyKey":"stdio-unknown-activation",
            "acknowledgementIds":["user_logic","user_risk","no_advice"]}),
    );
    assert_eq!(activated.status, 200, "{activated:#?}");
    let activation_id = activated.body["body"]["activationId"].as_str().unwrap();
    let mut client = McpClient::start(&candidate, &config_path, &root);
    assert!(client.call("initialize", json!({}))["result"].is_object());
    let attached = client.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-unknown-attach"}),
    );
    assert_eq!(attached["isError"], false, "{attached:#?}");
    let order = json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-paper",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-unknown-order","quantity":"1"},
        "idempotency_key":"stdio-unknown-order"});
    sink.drop_next_response();
    let uncertain = client.tool("tradeassembly.order.submit", order.clone());
    assert_eq!(
        uncertain["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{uncertain:#?}"
    );
    sink.fail_next_lookup();
    let unknown = client.tool(
        "tradeassembly.order.reconcile",
        json!({"original_idempotency_key":"stdio-unknown-order",
            "idempotency_key":"stdio-unknown-observation"}),
    );
    assert_eq!(
        unknown["structuredContent"]["error"]["code"], "plugin_order_reconciliation_required",
        "{unknown:#?}"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(
                "plugin_broker_recovery_outcomes",
                "stdio-unknown-observation"
            )
            .unwrap()
            .unwrap()["state"],
        "unresolved"
    );
    let replay = client.tool("tradeassembly.order.submit", order);
    assert_eq!(
        replay["structuredContent"]["error"]["code"], "agent_order_reconciliation_required",
        "{replay:#?}"
    );
    let evidence =
        Connection::open_with_flags(&sink.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let counts: (i64, i64) = evidence
        .query_row("SELECT COUNT(*), SUM(submissions) FROM orders", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(counts, (1, 1));
    assert!(service
        .runtime()
        .storage
        .get_json("plugin_operation_receipts", "stdio-unknown-order")
        .unwrap()
        .is_none());
    client.close();
    warden.stop();
}

#[test]
#[ignore = "requires explicit candidate Core, Warden, controlled broker, and SRT binaries"]
fn installed_stdio_live_order_uses_private_real_warden_c5_once() {
    controlled_live_order_case(true);
}

#[test]
#[ignore = "requires explicit candidate Core, Warden, controlled broker, and SRT binaries"]
fn installed_stdio_shipping_live_policy_denies_order() {
    controlled_live_order_case(false);
}

fn controlled_live_order_case(override_live_policy: bool) {
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
    let proxy = override_live_policy.then(|| ControlledLivePolicyProxy::start(warden.port));
    let sink = ControlledHttpSink::start(&root);
    let db = root.join("runtime.db");
    let layer = RuntimeConfigLayer {
        profile: Some("local".into()),
        database_path: Some(db.display().to_string()),
        artifact_root: Some(root.join("artifacts").display().to_string()),
        oidc_profile: Some("local_owner".into()),
        warden_sidecar_url: Some(proxy.as_ref().map_or_else(
            || format!("http://127.0.0.1:{}", warden.port),
            |proxy| proxy.url.clone(),
        )),
        warden_token_ref: Some(format!(
            "file://{}",
            root.join("warden/warden.token").display()
        )),
        legal_receipt_root: Some(root.join("legal/receipts").display().to_string()),
        legal_trusted_keys_path: Some(root.join("legal/trusted-keys.json").display().to_string()),
        legal_policy_path: Some(root.join("legal/policy.json").display().to_string()),
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
        "live",
        Some(&sink.url),
    );
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({"strategyId":"strat_local_btc_demo","orchestrator":"external_agent",
            "mode":"live","allowedSymbols":["BTC/USD"],
            "providerRef":"mandate-live","accountRef":"account://mandate-live/controlled",
            "dataProviderRef":"mandate-live","dataAccountRef":"account://mandate-live/controlled",
            "legalReceiptRef":controlled_legal_receipt::RECEIPT_REF,
            "riskLimits":{"max_notional":10,"max_order_quantity":2}}),
    );
    assert_eq!(saved.status, 200, "{saved:#?}");
    controlled_legal_receipt::write(
        &root,
        &saved.body["body"]["item"],
        &owner,
        service.runtime().clock.trusted_now_ms().unwrap(),
    );
    let configured = &saved.body["body"]["item"];
    let verifier = tradeassembly_runtime::adapters::legal_receipts::FileLegalReceiptVerifier::new(
        root.join("legal/receipts"),
        root.join("legal/trusted-keys.json"),
        root.join("legal/policy.json"),
    );
    let expectation = tradeassembly_runtime::ports::LegalReceiptExpectation {
        receipt_ref: controlled_legal_receipt::RECEIPT_REF.into(),
        identity_issuer: owner.issuer.clone(),
        identity_subject: owner.subject.clone(),
        resource_ref: format!(
            "tradeassembly://strategies/{}",
            configured["strategyId"].as_str().unwrap()
        ),
        resource_version_refs: vec![
            format!(
                "tradeassembly://strategy-versions/{}",
                configured["strategyVersionId"].as_str().unwrap()
            ),
            configured["strategySpecHash"]
                .as_str()
                .unwrap()
                .to_ascii_lowercase(),
        ],
        environment: configured["legalEnvironment"].as_str().unwrap().into(),
    };
    let verified = tradeassembly_runtime::ports::LegalReceiptPort::verify(
        &verifier,
        &expectation,
        service.runtime().clock.trusted_now_ms().unwrap(),
    );
    assert!(
        verified.is_ok(),
        "controlled legal fixture verification: {verified:?}"
    );
    let config_id = saved.body["body"]["configId"].as_str().unwrap();
    let deployment = AgentDeployment {
        executor: AgentExecutor::ExternalClient,
        deployment_id: "stdio-controlled-live".into(),
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
        mode: "live".into(),
        prompt: String::new(),
        workspace: String::new(),
        runtime_profile: "local-read-only".into(),
    };
    agent_runner::put_deployment(&service.runtime(), &deployment).unwrap();
    let without_mandate = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({"configId":config_id,"idempotencyKey":"stdio-live-no-mandate",
            "acknowledgementIds":["user_logic","user_risk","no_advice"]}),
    );
    assert_ne!(
        without_mandate.status, 200,
        "live activation requires a mandate"
    );
    assert!(
        !sink.db.exists(),
        "unmandated activation cannot reach the sink"
    );
    let issued = service.handle_http(
        "POST",
        "/product/live-mandates/issue",
        json!({"configId":config_id,
            "expiresAtMs":service.runtime().clock.trusted_now_ms().unwrap()+600_000,
            "delegateDeploymentId":deployment.deployment_id,
            "idempotencyKey":"stdio-live-mandate"}),
    );
    assert_eq!(issued.status, 201, "{issued:#?}");
    let mandate_id = issued.body["mandate"]["mandateId"].as_str().unwrap();
    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({"configId":config_id,"idempotencyKey":"stdio-live-activation",
            "localLiveMandateId":mandate_id,
            "acknowledgementIds":["user_logic","user_risk","no_advice"]}),
    );
    assert_eq!(activated.status, 200, "valid mandate can activate locally");
    let activation_id = activated.body["body"]["activationId"].as_str().unwrap();
    let other_owner =
        service.for_authenticated_invocation(&owner.issuer, "controlled-other-owner", None, None);
    let wrong_identity = other_owner.attach_external_agent_session(
        &deployment.deployment_id,
        activation_id,
        "stdio-live-wrong-identity",
    );
    assert!(
        wrong_identity.is_err(),
        "other owner cannot attach to the deployment"
    );
    let mut wrong_mode = deployment.clone();
    wrong_mode.deployment_id = "stdio-live-wrong-mode".into();
    wrong_mode.mode = "paper".into();
    agent_runner::put_deployment(&service.runtime(), &wrong_mode).unwrap();
    let mode_mismatch = service.attach_external_agent_session(
        &wrong_mode.deployment_id,
        activation_id,
        "stdio-live-mode-mismatch",
    );
    assert_eq!(
        mode_mismatch.err().unwrap().body["error"]["code"],
        "external_agent_binding_invalid"
    );
    assert!(
        !sink.db.exists(),
        "failed attach cannot reach the broker sink"
    );
    if !override_live_policy {
        let mut client = McpClient::start(&candidate, &config_path, &root);
        assert!(client.call("initialize", json!({}))["result"].is_object());
        let attached = client.tool(
            "tradeassembly.agent.session.attach",
            json!({"deployment_id":deployment.deployment_id,
                "activation_id":activation_id,"idempotency_key":"shipping-live-attach"}),
        );
        assert_eq!(attached["isError"], false, "valid Live session can attach");
        let denied = client.tool(
            "tradeassembly.order.submit",
            json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-live",
                "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
                    "timeInForce":"gtc","clientOrderId":"shipping-live-denied","quantity":"1"},
                "idempotency_key":"shipping-live-denied"}),
        );
        assert_eq!(
            denied["structuredContent"]["error"]["code"], "agent_order_submission_denied",
            "shipping Live order denied"
        );
        assert!(
            !sink.db.exists(),
            "shipping denial cannot reach broker sink"
        );
        let decisions = service
            .runtime()
            .storage
            .list_json("warden_decisions")
            .unwrap();
        assert!(decisions.iter().any(|(_, decision)| {
            decision["request"]["action"] == "order.submit.live"
                && decision["decision"]["decision"] == "deny"
        }));
        client.close();
        warden.assert_healthy();
        warden.stop();
        return;
    }
    assert_eq!(
        activated.status, 200,
        "code={} blockers={:?}",
        activated.body["error"]["code"], activated.body["error"]["details"]["blockedReasons"]
    );
    let activation_id = activated.body["body"]["activationId"].as_str().unwrap();
    let mut client = McpClient::start(&candidate, &config_path, &root);
    assert!(client.call("initialize", json!({}))["result"].is_object());
    let attached = client.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-live-attach"}),
    );
    assert_eq!(attached["isError"], false, "{attached:#?}");
    let order = json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-live",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-live-order","quantity":"1"},
        "idempotency_key":"stdio-live-order"});
    let mut other_client = McpClient::start(&candidate, &config_path, &root);
    assert!(other_client.call("initialize", json!({}))["result"].is_object());
    let competing_attach = other_client.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-live-competing-attach"}),
    );
    assert_eq!(
        competing_attach["structuredContent"]["error"]["code"],
        "agent_run_pending_reconcile"
    );
    let unattached_order = other_client.tool("tradeassembly.order.submit", order.clone());
    assert_eq!(
        unattached_order["structuredContent"]["error"]["code"],
        "agent_order_attachment_required"
    );
    other_client.close();
    assert!(
        !sink.db.exists(),
        "competing client cannot reach the broker sink"
    );
    let mut over_limit = order.clone();
    over_limit["order"]["clientOrderId"] = json!("stdio-live-over-limit");
    over_limit["order"]["quantity"] = json!("3");
    over_limit["idempotency_key"] = json!("stdio-live-over-limit");
    let denied = client.tool("tradeassembly.order.submit", over_limit);
    assert_eq!(
        denied["structuredContent"]["error"]["code"],
        "agent_order_submission_denied"
    );
    assert!(
        !sink.db.exists(),
        "risk-denied order cannot reach the broker sink"
    );
    let [first, concurrent_duplicate] =
        client.tool_pair_in_flight("tradeassembly.order.submit", order.clone());
    assert_eq!(
        first["structuredContent"]["status"], "submitted",
        "{first:#?}"
    );
    assert_eq!(concurrent_duplicate, first);
    assert_eq!(client.tool("tradeassembly.order.submit", order), first);
    let evidence =
        Connection::open_with_flags(&sink.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let counts: (i64, i64) = evidence
        .query_row("SELECT COUNT(*), SUM(submissions) FROM orders", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(counts, (1, 1));
    let decisions = service
        .runtime()
        .storage
        .list_json("warden_decisions")
        .unwrap();
    let live = decisions
        .iter()
        .find(|(_, record)| record["request"]["action"] == "order.submit.live")
        .unwrap();
    assert_eq!(live.1["decision"]["decision"], "allow");
    assert_eq!(live.1["request"]["pep_coverage"], "c5");
    let signed = service
        .runtime()
        .storage
        .get_json("finance_receipts", &live.0)
        .unwrap()
        .unwrap();
    assert_eq!(signed["schema_version"], "warden.signed_receipt.v2");
    assert_eq!(signed["signature"]["algorithm"], "ed25519");
    let detached = client.tool("tradeassembly.agent.session.detach", json!({}));
    assert_eq!(detached["structuredContent"]["status"], "detached");
    assert_eq!(
        detached["structuredContent"]["reconciliationRequired"],
        true
    );
    let unattached_after_detach = client.tool(
        "tradeassembly.order.submit",
        json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-live",
            "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
                "timeInForce":"gtc","clientOrderId":"stdio-live-detached","quantity":"1"},
            "idempotency_key":"stdio-live-detached"}),
    );
    assert_eq!(
        unattached_after_detach["structuredContent"]["error"]["code"],
        "agent_order_attachment_required"
    );
    let recovered = client.tool(
        "studio.agent_run.recover",
        json!({"deployment_id":deployment.deployment_id,
            "acknowledge_reconciled":true,"idempotency_key":"stdio-live-owner-recovery",
            "authority_context":{"actor":"ignored","surface":"mcp","accountMode":"live"}}),
    );
    assert_eq!(
        recovered["isError"], false,
        "owner recovery code={}",
        recovered["structuredContent"]["error"]["code"]
    );
    let reattached = client.tool(
        "tradeassembly.agent.session.attach",
        json!({"deployment_id":deployment.deployment_id,"activation_id":activation_id,
            "idempotency_key":"stdio-live-reattach"}),
    );
    assert_eq!(
        reattached["isError"], false,
        "clean detach can re-attach after reconciliation"
    );
    let lease_resource = format!("agent-deployment:{}", deployment.deployment_id);
    let original_lease = service
        .runtime()
        .leases
        .current(&lease_resource)
        .unwrap()
        .unwrap();
    service.runtime().leases.release(&original_lease).unwrap();
    let unleased = client.tool(
        "tradeassembly.order.submit",
        json!({"activation_id":activation_id,"plugin_instance_ref":"mandate-live",
            "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
                "timeInForce":"gtc","clientOrderId":"stdio-live-unleased","quantity":"1"},
            "idempotency_key":"stdio-live-unleased"}),
    );
    assert_eq!(unleased["isError"], true, "released lease cannot submit");
    let replacement_lease = service
        .runtime()
        .leases
        .acquire(
            &lease_resource,
            "controlled-replacement",
            service.runtime().clock.trusted_now_ms().unwrap(),
            90_000,
        )
        .unwrap()
        .unwrap();
    let mut stale_order = json!({"activation_id":activation_id,
        "plugin_instance_ref":"mandate-live",
        "order":{"symbol":"BTC/USD","side":"buy","orderType":"market",
            "timeInForce":"gtc","clientOrderId":"stdio-live-stale","quantity":"1"},
        "idempotency_key":"stdio-live-stale"});
    let stale = client.tool("tradeassembly.order.submit", stale_order.clone());
    assert_eq!(
        stale["isError"], true,
        "replaced lease must reject old session"
    );
    stale_order["idempotency_key"] = json!("stdio-live-stale-again");
    let stale_again = client.tool("tradeassembly.order.submit", stale_order);
    assert_eq!(stale_again["isError"], true, "old session cannot revive");
    assert_eq!(
        service
            .runtime()
            .leases
            .current(&lease_resource)
            .unwrap()
            .unwrap(),
        replacement_lease
    );
    assert_eq!(
        evidence
            .query_row("SELECT COUNT(*), SUM(submissions) FROM orders", [], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap(),
        (1, 1),
        "stale session cannot add a sink effect"
    );
    proxy.as_ref().unwrap().assert_used();
    warden.assert_healthy();
    for namespace in ["scheduler_state", "execution_ticks"] {
        assert!(service
            .runtime()
            .storage
            .list_json(namespace)
            .unwrap()
            .is_empty());
    }
    client.close();
    warden.stop();
}
