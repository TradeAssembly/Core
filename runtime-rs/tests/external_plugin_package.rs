// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use clap::Parser;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use tradeassembly_runtime::cli::{execute_command, Cli};
use tradeassembly_runtime::service::{ServiceResponse, TradeAssemblyService};

#[test]
#[ignore = "requires TRADEASSEMBLY_ALPACA_PLUGIN_PACKAGE and TRADEASSEMBLY_ALPACA_PLUGIN_LOCK"]
fn standalone_alpaca_package_installs_resolves_and_invokes_without_exposing_secrets() {
    let package_path = required_path("TRADEASSEMBLY_ALPACA_PLUGIN_PACKAGE");
    let lock_path = required_path("TRADEASSEMBLY_ALPACA_PLUGIN_LOCK");
    let lock: Value = serde_json::from_slice(&fs::read(lock_path).expect("read plugin lock"))
        .expect("plugin lock");
    let package_sha256 = lock["packageSha256"].as_str().expect("package digest");
    let manifest_sha256 = lock["manifestSha256"].as_str().expect("manifest digest");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind Alpaca mock");
    let address = listener.local_addr().expect("mock address");
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().expect("accept Alpaca request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).expect("read Alpaca request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            requests_tx
                .send(String::from_utf8_lossy(&bytes).to_string())
                .expect("capture Alpaca request");
            let request = String::from_utf8_lossy(&bytes);
            let body = if request.contains("POST /v2/orders ") {
                r#"{"id":"order-package","client_order_id":"stable-package-order","symbol":"SPY","side":"buy","status":"filled","qty":"1","filled_qty":"1"}"#
            } else if request.contains("/v2/stocks/bars") {
                r#"{"bars":{"SPY":[{"t":"2026-01-02T00:00:00Z","o":500.0,"h":502.0,"l":499.0,"c":501.0,"v":10},{"t":"2026-01-02T00:01:00Z","o":501.0,"h":503.0,"l":500.0,"c":502.0,"v":12}]},"next_page_token":null}"#
            } else {
                r#"{"id":"acct-package","status":"ACTIVE","currency":"USD","buying_power":"25000"}"#
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nx-request-id: mock-request\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write Alpaca response");
        }
    });

    let directory = tempfile::tempdir().expect("temporary runtime directory");
    let service = TradeAssemblyService::test_local_with_plugin_local_egress(
        directory
            .path()
            .join("runtime.db")
            .to_string_lossy()
            .to_string(),
    );
    let package_locator = package_path.to_string_lossy().into_owned();
    let cli = Cli::try_parse_from([
        "tradeassembly",
        "plugins",
        "install-default",
        "--package-source",
        package_locator.as_str(),
        "--offline",
    ])
    .expect("default package CLI parses");
    let installed = execute_command(&service, cli.command);
    assert_eq!(installed["package"]["pluginRef"], "tradeassembly.alpaca");
    assert_eq!(installed["package"]["packageSha256"], package_sha256);
    assert_eq!(installed["package"]["manifestSha256"], manifest_sha256);

    let endpoint = format!("http://{address}");
    let created = request(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "alpaca-package-paper",
            "pluginRef": "tradeassembly.alpaca",
            "providerRef": "alpaca",
            "accountRef": "account://alpaca/paper",
            "enabled": true,
            "configuration": {
                "tradingApiUrl": endpoint,
                "dataApiUrl": endpoint,
            },
        }),
    );
    assert_eq!(created["instance"]["health"]["state"], "needs_credentials");

    let api_key = "package-test-key";
    let api_secret = "test-package-secret";
    let credentials = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-package-paper/credentials",
        json!({"credentials": {"api_key": api_key, "api_secret": api_secret}}),
    );
    assert_eq!(credentials["credentialStatus"]["configured"], true);
    assert_no_secrets(&credentials, &[api_key, api_secret]);

    let invocation = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-package-paper/operations/account.health:invoke",
        json!({
            "mode": "paper",
            "purpose": "account_health",
            "accountRef": "account://alpaca/paper",
            "idempotencyKey": "external-package-health-1",
            "input": {},
        }),
    );
    assert_eq!(
        invocation["response"]["payload"]["account"]["id"],
        "acct-package"
    );
    assert_eq!(
        invocation["binding"]["pluginInstanceRef"],
        "alpaca-package-paper"
    );
    assert_eq!(invocation["binding"]["operationId"], "account.health");
    assert_eq!(invocation["binding"]["capability"], "account.health");
    assert_no_secrets(&invocation, &[api_key, api_secret]);

    let health = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-package-paper/health:refresh",
        json!({}),
    );
    assert_eq!(health["instance"]["health"]["state"], "ready");
    assert_eq!(health["instance"]["health"]["connectivityChecked"], true);
    assert_eq!(
        health["instance"]["health"]["account"]["id"],
        "acct-package"
    );
    assert_eq!(
        health["instance"]["accountRef"],
        "account://alpaca-package-paper/acct-package"
    );
    assert_eq!(
        health["instance"]["health"]["account"]["mode"],
        "provider_reported"
    );
    assert!(health["instance"]["accountMode"].is_null());
    assert_no_secrets(&health, &[api_key, api_secret]);

    let mut strategy_spec: Value = serde_json::from_slice(include_bytes!(
        "../../examples/strategy-spec/v3/valid/crypto-spot-24x7.json"
    ))
    .expect("crypto strategy fixture");
    strategy_spec["strategy_id"] = json!("external.alpaca.crypto");
    strategy_spec["name"] = json!("External Alpaca crypto contract");
    let created_strategy = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "External Alpaca crypto contract", "spec": strategy_spec}),
    );
    assert_eq!(created_strategy.status, 201, "{:#}", created_strategy.body);
    let strategy_id = created_strategy.body["body"]["strategy"]["id"]
        .as_str()
        .expect("strategy id");
    let published_strategy = request(
        &service,
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": created_strategy.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    let version = &published_strategy["body"]["version"];
    let graph = request(
        &service,
        "POST",
        "/capability-graph-revisions",
        json!({
            "configId": "cfg_external_alpaca_crypto",
            "strategyId": strategy_id,
            "strategyVersionId": version["id"],
            "strategySpecHash": version["specHash"],
            "mode": "research",
            "evaluationEpoch": "2026-08-16T00:00:00Z",
            "idempotencyKey": "external-alpaca-crypto-graph-v1",
            "bindings": {
                "req_bars": {
                    "pluginInstanceRef": "alpaca-package-paper",
                    "pluginRef": "tradeassembly.alpaca",
                    "operationId": "marketdata.bars.read_v1"
                },
                "req_logic": {
                    "pluginInstanceRef": "core-runtime",
                    "pluginRef": "tradeassembly.core-runtime",
                    "operationId": "expression.cel.evaluate_v1"
                },
                "req_calendar": {
                    "pluginInstanceRef": "core-runtime",
                    "pluginRef": "tradeassembly.core-runtime",
                    "operationId": "calendar.session.resolve_v1"
                }
            }
        }),
    );
    let bars_node = graph["revision"]["graph"]["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("capability graph nodes: {graph:#}"))
        .iter()
        .find(|node| node["nodeId"] == "req_bars")
        .expect("bars requirement node");
    assert_eq!(bars_node["blockers"], json!([]), "{graph:#}");
    assert_eq!(
        bars_node["selected"]["pluginInstanceRef"],
        "alpaca-package-paper"
    );

    let ingestion = service.handle_http(
        "POST",
        "/dataset-ingestions",
        json!({
            "strategyId": "strat_local_btc_demo",
            "pluginInstanceRef": "alpaca-package-paper",
            "operationId": "marketdata.bars.read_v1",
            "assetClass": "equity",
            "instruments": ["SPY"],
            "dataKind": "bars",
            "granularity": "1m",
            "timeSlice": {
                "start": "2026-01-02T00:00:00Z",
                "end": "2026-01-02T00:01:00Z"
            },
            "calendar": "XNYS",
            "timezone": "America/New_York",
            "normalizationPolicy": {
                "timestampUnit": "rfc3339",
                "timezone": "UTC",
                "duplicatePolicy": "reject",
                "priceAdjustment": "split_dividend_adjusted"
            },
            "qualityPolicy": {
                "missingIntervals": "reject",
                "staleObservations": "reject",
                "outliers": "warn",
                "invalidMarkets": "reject",
                "calendarMismatch": "reject",
                "corporateActionGaps": "warn",
                "missingDerivativeFields": "reject"
            },
            "maxRows": 100,
            "idempotencyKey": "external-package-dataset-1",
            "authorityContext": {"actor": "user.local", "surface": "integration-test"}
        }),
    );
    assert_eq!(ingestion.status, 201, "{:#}", ingestion.body);
    assert_eq!(
        ingestion.body["status"], "completed",
        "{:#}",
        ingestion.body
    );
    assert_eq!(ingestion.body["snapshot"]["rowCount"], 2);
    assert_eq!(
        ingestion.body["snapshot"]["content"]["source"]["pluginInstanceRef"],
        "alpaca-package-paper"
    );
    assert_eq!(
        ingestion.body["snapshot"]["content"]["source"]["pluginRef"],
        "tradeassembly.alpaca"
    );
    assert_eq!(
        ingestion.body["snapshot"]["content"]["observations"][0]["data"]["kind"],
        "bar"
    );
    assert_no_secrets(&ingestion.body, &[api_key, api_secret]);

    let paper_order = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-package-paper/operations/broker.paper_order_submit:invoke",
        json!({
            "mode": "paper",
            "purpose": "paper_trading",
            "accountRef": "account://alpaca/paper",
            "strategyId": "strat_local_btc_demo",
            "strategyVersionId": "version_local_btc_demo_v1",
            "strategySpecHash": "spec-hash-package",
            "activationId": "activation-package",
            "attemptId": "attempt-package-1",
            "evaluationTickId": "tick-package-1",
            "fencingToken": 1,
            "idempotencyKey": "external-package-order-1",
            "input": {
                "symbol": "SPY",
                "side": "buy",
                "quantityMicros": 1_000_000,
                "clientOrderId": "stable-package-order"
            },
            "evidenceRefs": ["evidence://package/order-1"]
        }),
    );
    assert_eq!(
        paper_order["response"]["payload"]["providerOrderId"],
        "order-package"
    );
    assert_eq!(
        paper_order["response"]["payload"]["clientOrderId"],
        "stable-package-order"
    );
    assert_eq!(paper_order["response"]["payload"]["status"], "filled");
    assert_no_secrets(&paper_order, &[api_key, api_secret]);

    let duplicate_order = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-package-paper/operations/broker.paper_order_submit:invoke",
        json!({
            "mode": "paper",
            "purpose": "paper_trading",
            "accountRef": "account://alpaca/paper",
            "strategyId": "strat_local_btc_demo",
            "strategyVersionId": "version_local_btc_demo_v1",
            "strategySpecHash": "spec-hash-package",
            "activationId": "activation-package",
            "attemptId": "attempt-package-1",
            "evaluationTickId": "tick-package-1",
            "fencingToken": 1,
            "idempotencyKey": "external-package-order-1",
            "input": {
                "symbol": "SPY",
                "side": "buy",
                "quantityMicros": 1_000_000,
                "clientOrderId": "stable-package-order"
            },
            "evidenceRefs": ["evidence://package/order-1"]
        }),
    );
    assert_eq!(
        duplicate_order["response"]["contentHash"],
        paper_order["response"]["contentHash"]
    );

    server.join().expect("Alpaca mock server");
    let captured = [
        requests_rx.recv().unwrap(),
        requests_rx.recv().unwrap(),
        requests_rx.recv().unwrap(),
        requests_rx.recv().unwrap(),
    ];
    assert!(captured
        .iter()
        .any(|request| request.contains("GET /v2/stocks/bars?")));
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.contains("POST /v2/orders "))
            .count(),
        1
    );
    for request in captured {
        let normalized = request.to_ascii_lowercase();
        assert!(normalized.contains("apca-api-key-id: package-test-key"));
        assert!(normalized.contains("apca-api-secret-key: test-package-secret"));
    }

    let degraded = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-package-paper/health:refresh",
        json!({}),
    );
    assert_eq!(
        degraded["instance"]["health"]["state"], "degraded",
        "{degraded:#}"
    );
    let blocked = service.handle_http(
        "POST",
        "/plugins/instances/alpaca-package-paper/operations/marketdata.bars.read_v1:invoke",
        json!({
            "mode": "paper",
            "purpose": "historical_research",
            "idempotencyKey": "degraded-package-must-not-invoke",
            "input": {},
        }),
    );
    assert_eq!(blocked.status, 400);
    assert_eq!(blocked.body["error"]["code"], "plugin_invocation_blocked");
    assert_eq!(
        blocked.body["error"]["details"]["blockers"][0]["code"],
        "plugin_unhealthy"
    );

    let database = fs::read(directory.path().join("runtime.db")).expect("runtime database");
    let database = String::from_utf8_lossy(&database);
    assert!(!database.contains(api_key));
    assert!(!database.contains(api_secret));
}

fn required_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .unwrap_or_else(|| panic!("{name} must point to a file"))
}

fn request(service: &TradeAssemblyService, method: &str, path: &str, body: Value) -> Value {
    let response = service.handle_http(method, path, body);
    assert_success(method, path, response)
}

fn assert_success(method: &str, path: &str, response: ServiceResponse) -> Value {
    assert_eq!(response.status, 200, "{method} {path}: {:#}", response.body);
    response.body
}

fn assert_no_secrets(value: &Value, secrets: &[&str]) {
    let rendered = value.to_string();
    for secret in secrets {
        assert!(
            !rendered.contains(secret),
            "secret appeared in public response"
        );
    }
}
