use clap::CommandFactory;
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tradeassembly_runtime::cli::Cli;
use tradeassembly_runtime::http::{build_router, documented_routes};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn service_exposes_core_product_surfaces() {
    let service = test_service("core-product-surfaces");

    let workspace = service.handle_http("GET", "/workspace", json!({}));
    assert_eq!(workspace.status, 200);
    assert_eq!(workspace.body["workspace"]["id"], "local");
    assert!(!workspace.body["strategies"].as_array().unwrap().is_empty());

    let graphql = service.execute_graphql(json!({
        "operationName": "StrategyLibrary",
        "query": "query StrategyLibrary { strategyLibrary }"
    }));
    assert!(graphql.get("errors").is_none(), "{graphql}");
    assert_eq!(
        graphql["data"]["strategyLibrary"]["strategies"][0]["id"],
        "strat_local_btc_demo"
    );

    let mutation = service.execute_graphql(json!({
        "operationName": "DuplicateStrategy",
        "query": "mutation DuplicateStrategy { duplicateStrategy }",
        "variables": {"strategyId": "strat_local_btc_demo", "name": "BTC duplicate proof"}
    }));
    assert_eq!(mutation["data"]["duplicateStrategy"]["ok"], true);
    assert!(
        mutation["data"]["duplicateStrategy"]["body"]["strategy"]["name"]
            .as_str()
            .unwrap()
            .contains("duplicate")
    );
}

#[test]
fn studio_payload_contracts_match_rust_backend() {
    let service = test_service("studio-payload-contracts");
    let mut spec = valid_v3_spec();

    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "Blank Local Strategy 1", "spec": spec}),
    );
    assert_eq!(created.status, 201);
    let created_id = created.body["body"]["strategy"]["id"]
        .as_str()
        .expect("created strategy id");
    assert!(
        created_id.starts_with("strat_")
            && created_id["strat_".len()..]
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
        "created id must stay Studio-route compatible: {created_id}"
    );

    spec = created.body["body"]["strategy"]["latestSpec"].clone();
    spec["name"] = json!("Saved V3 strategy");
    let saved = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId": created_id, "spec": spec}),
    );
    assert_eq!(saved.body["ok"], true);
    assert_eq!(saved.body["body"]["draftSaved"], true);
    assert!(saved.body["body"]["draft"]["draftHash"]
        .as_str()
        .expect("draft hash")
        .starts_with("sha256:"));
    assert!(saved.body["body"].get("version").is_none());

    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": created_id,
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(published.body["body"]["published"], true);
    assert_eq!(published.body["body"]["activation"]["started"], false);
    assert!(published.body["body"]["version"]["id"]
        .as_str()
        .expect("version id")
        .starts_with("version_"));

    let config = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({"strategyId": created_id}),
    );
    assert!(config.body["body"]["configId"]
        .as_str()
        .expect("config id")
        .starts_with("cfg_"));

    let run_once = service.handle_http(
        "POST",
        "/product/run-center/run-once",
        json!({"strategyId": created_id}),
    );
    assert!(run_once.body["body"]["order_id"]
        .as_str()
        .expect("order id")
        .starts_with("order_"));

    let research = service.handle_http(
        "POST",
        "/product/strategies/research-runs/create",
        json!({"strategyId": created_id, "idempotencyKey": "research-alias-validation"}),
    );
    let canonical = service.handle_http(
        "POST",
        "/product/strategies/backtests/run",
        json!({"strategyId": created_id, "idempotencyKey": "canonical-validation"}),
    );
    assert_eq!(research.status, 400);
    assert_eq!(research.status, canonical.status);
    assert_eq!(research.body, canonical.body);
    let replay = service.handle_http(
        "POST",
        "/product/strategies/backtests/run",
        json!({"strategyId": created_id, "idempotencyKey": "research-alias-validation"}),
    );
    assert_eq!(replay.status, 400);
    assert_eq!(replay.body["error"]["code"], "idempotency_replay_failed");
    assert_eq!(
        service.handle_http("GET", "/backtests", json!({})).body["runs"],
        json!([])
    );

    let reset = service.handle_http("POST", "/demo/reset", json!({}));
    assert_eq!(reset.body["resetApplied"], true);
    let reset_namespaces = reset.body["namespaces"]
        .as_array()
        .expect("reset namespaces");
    for namespace in [
        "dataset_ingestions_v1",
        "dataset_ingestion_idempotency_v1",
        "dataset_snapshots_v1",
        "backtest_manifests_v1",
        "backtest_results_v1",
        "backtest_runs_v2",
        "backtest_attempts_v1",
        "backtest_lifecycle_events_v1",
    ] {
        assert!(
            reset_namespaces
                .iter()
                .any(|value| value.as_str() == Some(namespace)),
            "reset must clear {namespace}"
        );
    }
    let workspace = service.handle_http("GET", "/workspace", json!({}));
    let strategies = workspace.body["strategies"].as_array().expect("strategies");
    assert!(
        !strategies
            .iter()
            .any(|strategy| strategy["id"].as_str() == Some(created_id)),
        "reset should remove locally created strategies"
    );
    assert_eq!(strategies[0]["name"], "BTC fast exit demo");
}

#[test]
fn rust_http_router_documents_legacy_route_groups() {
    let _router = build_router(test_service("route-groups"));
    let routes = documented_routes();
    for expected in [
        "GET /health",
        "GET /ready",
        "GET /workspace",
        "POST /graphql",
        "POST /product/strategies/create",
        "POST /product/strategies/duplicate",
        "POST /product/strategies/backtests/export",
        "POST /product/strategies/backtests/run",
        "POST /product/strategies/research-runs/create",
        "POST /product/strategies/monte-carlo",
        "POST /product/strategies/scenario-valuation",
        "POST /product/run-center/run-once",
        "POST /providers/{ref}/credentials/test",
        "POST /scheduler/run",
        "POST /orders/reconcile",
        "GET /marketdata/bars",
        "POST /marketdata/options/select",
    ] {
        assert!(routes.contains(&expected), "missing route {expected}");
    }
}

#[test]
fn rust_mcp_tools_delegate_to_service_shapes() {
    let service = test_service("mcp-tools");
    let response = service.call_mcp_tool("tradeassembly.strategy.list", json!({}));
    assert_eq!(response["isError"], false);
    assert_eq!(
        response["structuredContent"]["strategies"][0]["id"],
        "strat_local_btc_demo"
    );

    let run = service.call_mcp_tool(
        "tradeassembly.execution.run",
        json!({"activation_id": "activation-btc-exit-demo"}),
    );
    assert_eq!(run["structuredContent"]["submit_orders"], false);

    let scenario = service.call_mcp_tool(
        "tradeassembly.scenario.valuation",
        json!({"strategy_id": "strat_local_btc_demo", "checkpoint_id": "checkpoint_001"}),
    );
    let payload = &scenario["structuredContent"];
    assert_eq!(scenario["isError"], true);
    assert_eq!(payload["error"]["code"], "derivatives_request_invalid");
    assert!(payload.get("analysis").is_none());
    assert!(!payload.to_string().contains("api_secret"));
    assert!(!payload.to_string().to_lowercase().contains("should buy"));

    let monte_carlo = service.call_mcp_tool(
        "tradeassembly.monte_carlo.report",
        json!({"strategy_id": "strat_local_btc_demo"}),
    );
    let mc_payload = &monte_carlo["structuredContent"];
    assert_eq!(monte_carlo["isError"], true);
    assert_eq!(mc_payload["error"]["code"], "robustness_request_invalid");
    assert!(mc_payload.get("result").is_none());
    assert!(!mc_payload.to_string().contains("api_secret"));
    assert!(!mc_payload.to_string().to_lowercase().contains("should buy"));

    let portfolio_risk = service.call_mcp_tool(
        "tradeassembly.portfolio_risk.report",
        json!({"strategy_id": "strat_local_btc_demo", "scope_kind": "strategy"}),
    );
    let risk_payload = &portfolio_risk["structuredContent"];
    assert_eq!(risk_payload["ok"], true);
    assert_eq!(risk_payload["kind"], "portfolio_risk");
    assert_eq!(risk_payload["strategyId"], "strat_local_btc_demo");
    assert_eq!(
        risk_payload["reportEnvelope"]["charts"][0]["id"],
        "portfolio_risk_exposure_table"
    );
    assert!(
        risk_payload["reportEnvelope"]["deepLinks"]["portfolioRisk"]["href"]
            .as_str()
            .unwrap()
            .contains("view=portfolio-risk")
    );
    assert!(!risk_payload.to_string().contains("api_secret"));
    assert!(!risk_payload
        .to_string()
        .to_lowercase()
        .contains("should buy"));

    let fill_quality = service.call_mcp_tool(
        "tradeassembly.fill_quality.report",
        json!({"strategy_id": "strat_local_btc_demo", "analysis_id": "fill_quality_latest"}),
    );
    let fill_payload = &fill_quality["structuredContent"];
    assert_eq!(fill_payload["ok"], true);
    assert_eq!(fill_payload["kind"], "fill_quality");
    assert_eq!(fill_payload["strategyId"], "strat_local_btc_demo");
    assert_eq!(fill_payload["summary"]["fillCount"].as_i64(), Some(2));
    assert!(
        fill_payload["reportEnvelope"]["deepLinks"]["execution"]["href"]
            .as_str()
            .unwrap()
            .contains("fillQuality=fill_quality_latest")
    );
    assert!(fill_payload["agentSummary"]["studioDeepLinks"]["execution"]
        .as_str()
        .unwrap()
        .contains("fillQuality=fill_quality_latest"));
    assert!(!fill_payload.to_string().contains("api_secret"));
    assert!(!fill_payload.to_string().contains("Bearer "));
    assert!(!fill_payload
        .to_string()
        .to_lowercase()
        .contains("should buy"));

    let lifecycle_calendar = service.call_mcp_tool(
        "tradeassembly.lifecycle_calendar.inspect",
        json!({"strategy_id": "strat_local_btc_demo", "timeline_id": "lifecycle_calendar_latest"}),
    );
    let calendar_payload = &lifecycle_calendar["structuredContent"];
    assert_eq!(
        calendar_payload["schemaVersion"],
        "tradeassembly.lifecycle_calendar.service.v1"
    );
    assert_eq!(calendar_payload["strategyId"], "strat_local_btc_demo");
    assert!(
        calendar_payload["summary"]["upcomingCount"]
            .as_i64()
            .unwrap_or(0)
            >= 1
    );
    assert!(calendar_payload["deepLinks"]["calendar"]["href"]
        .as_str()
        .unwrap()
        .contains("lifecycleCalendar="));
    assert!(
        calendar_payload["agentSummary"]["studioDeepLinks"]["calendar"]
            .as_str()
            .unwrap()
            .contains("lifecycleCalendar=")
    );
    assert_eq!(calendar_payload["sideEffects"]["brokerStateChanged"], false);
    assert!(!calendar_payload.to_string().contains("api_secret"));
    assert!(!calendar_payload.to_string().contains("Bearer "));
    assert!(!calendar_payload
        .to_string()
        .to_lowercase()
        .contains("should buy"));
}

fn test_service(name: &str) -> TradeAssemblyService {
    TradeAssemblyService::test_local(temp_db(name).to_string_lossy().to_string())
}

fn valid_v3_spec() -> serde_json::Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/strategy-spec/v3/valid/static-equity.json"
    )))
    .expect("valid checked-in V3 strategy fixture")
}

fn temp_db(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "tradeassembly-backend-parity-{name}-{}-{stamp}.db",
        std::process::id()
    ))
}

#[test]
fn tradeassembly_binary_exposes_runtime_entrypoints() {
    let stdout = Cli::command().render_long_help().to_string();
    for expected in [
        "api",
        "mcp",
        "strategy",
        "providers",
        "backtest",
        "scheduler",
    ] {
        assert!(
            stdout.contains(expected),
            "help missing {expected}: {stdout}"
        );
    }
}
