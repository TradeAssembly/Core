use reqwest::Method;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tradeassembly_runtime::{
    http::{build_router, documented_routes, route_is_documented},
    service::TradeAssemblyService,
};

static NEXT_DB_ID: AtomicU64 = AtomicU64::new(1);

#[tokio::test]
async fn loopback_and_claimed_local_owner_do_not_authorize_studio_graphql() {
    let base_url = spawn_api().await;
    let client = reqwest::Client::new();
    let request = json!({
        "query": "query { strategies { id } }",
        "actor": "local-owner",
        "principalUser": "local-owner",
        "variables": {
            "authority": {"actor": "local-owner"},
            "_studioSession": {
                "subject": "local-owner",
                "issuer": "local",
                "displayName": "Local owner"
            }
        }
    });
    for bearer in [None, Some("untrusted-local-transport")] {
        let mut pending = client.post(format!("{base_url}/graphql")).json(&request);
        if let Some(bearer) = bearer {
            pending = pending.bearer_auth(bearer);
        }
        let response = pending.send().await.expect("loopback response");
        assert_eq!(response.status().as_u16(), 401);
        let body: Value = response.json().await.expect("structured response");
        assert!(body.get("data").is_none());
        assert!(!body.to_string().contains("untrusted-local-transport"));
    }
}

#[tokio::test]
async fn documented_http_routes_are_served_by_rust_axum_api() {
    let base_url = spawn_api().await;
    let client = reqwest::Client::new();

    for route in documented_routes() {
        let (method, path) = concrete_route(route);
        let response = client
            .request(method, format!("{base_url}{path}"))
            .json(&default_body())
            .send()
            .await
            .unwrap_or_else(|error| panic!("{route} request failed: {error}"));
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .unwrap_or_else(|error| panic!("{route} returned non-JSON body: {error}"));
        let route_not_found = status.as_u16() == 404 && body["error"]["code"] == "route_not_found";
        assert!(
            !route_not_found,
            "{route} fell through route_not_found: {body:#}"
        );
        let expected_domain_not_found = status.as_u16() == 404;
        let expected_authority_gate = status.as_u16() == 403
            && matches!(
                route,
                "POST /product/sightline/selections/set"
                    | "POST /product/sightline/selections/share"
            );
        if route == "POST /graphql" {
            assert_eq!(
                status.as_u16(),
                401,
                "{route} must fail closed at the Studio transport boundary: {body:#}"
            );
            assert_eq!(
                body["error"]["code"], "studio_transport_authentication_required",
                "{route} must fail closed with the explicit Studio authentication error"
            );
            continue;
        }
        if route == "POST /product/strategies/share-snapshots/create" {
            assert_eq!(status.as_u16(), 501);
            assert_eq!(body["error"]["code"], "durable_share_snapshots_unavailable");
            assert_eq!(body["error"]["retryable"], false);
            assert!(body.get("snapshot").is_none());
            continue;
        }
        assert!(
            status.is_success()
                || status.as_u16() == 400
                || expected_domain_not_found
                || expected_authority_gate,
            "{route} returned unexpected status {status}: {body:#}"
        );
        if expected_authority_gate {
            assert_eq!(
                body["error"]["code"], "sightline_user_authority_required",
                "{route} must fail closed with the explicit trusted-principal error"
            );
        }
    }
}

#[tokio::test]
async fn http_errors_use_structured_redacted_envelopes() {
    let base_url = spawn_api().await;
    let response = reqwest::get(format!("{base_url}/missing-route"))
        .await
        .expect("missing route response");
    assert_eq!(response.status().as_u16(), 404);
    let body: Value = response.json().await.expect("json body");
    assert_eq!(body["detail"], "route_not_found");
    assert_eq!(body["error"]["code"], "route_not_found");
    assert!(!body.to_string().contains("api_secret"));
}

#[tokio::test]
async fn http_validation_errors_use_required_status_and_redacted_body() {
    let base_url = spawn_api().await;
    let response = reqwest::Client::new()
        .post(format!("{base_url}/strategy/contracts/validate"))
        .json(&json!({"api_secret": "should-not-leak"}))
        .send()
        .await
        .expect("validation response");
    assert_eq!(response.status().as_u16(), 400);

    let body: Value = response.json().await.expect("json body");
    assert_eq!(body["detail"], "validation_failed");
    assert_eq!(body["error"]["code"], "validation_failed");
    assert_eq!(body["error"]["retryable"], false);
    assert!(!body.to_string().contains("should-not-leak"));
}

#[test]
fn service_error_helpers_cover_http_status_contract() {
    assert_error(
        tradeassembly_runtime::service::ServiceResponse::forbidden("authority_required"),
        403,
        "authority_required",
    );
    assert_error(
        tradeassembly_runtime::service::ServiceResponse::conflict("idempotency_conflict"),
        409,
        "idempotency_conflict",
    );
    assert_error(
        tradeassembly_runtime::service::ServiceResponse::bad_gateway("provider_unavailable"),
        502,
        "provider_unavailable",
    );

    let internal =
        tradeassembly_runtime::service::ServiceResponse::internal_error("database password leaked");
    assert_error(internal.clone(), 500, "internal_error");
    assert!(!internal
        .body
        .to_string()
        .contains("database password leaked"));
}

#[test]
fn http_route_registry_matches_dynamic_paths_used_by_router() {
    assert!(route_is_documented(
        "POST",
        "/strategies/strat_local_btc_demo/backtests"
    ));
    assert!(route_is_documented(
        "POST",
        "/plugins/instances/alpaca-paper/operations/status:invoke"
    ));
    assert!(route_is_documented("POST", "/risk/portfolio-overlay"));
    assert!(route_is_documented(
        "POST",
        "/product/strategies/portfolio-risk/report"
    ));
    assert!(route_is_documented(
        "POST",
        "/product/strategies/fill-quality/report"
    ));
    assert!(route_is_documented(
        "POST",
        "/product/strategies/attribution-journal/report"
    ));
    assert!(route_is_documented(
        "POST",
        "/product/strategies/attribution-journal/replay"
    ));
    assert!(route_is_documented(
        "POST",
        "/product/strategies/attribution-journal/export"
    ));
    assert!(route_is_documented("POST", "/journal/attribution-review"));
    assert!(route_is_documented("POST", "/execution/fill-quality"));
    assert!(route_is_documented(
        "DELETE",
        "/providers/alpaca-paper/credentials"
    ));
    assert!(!route_is_documented("POST", "/product/not-a-real-route"));
}

async fn spawn_api() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind api listener");
    let address = listener.local_addr().expect("listener address");
    let router = build_router(test_service("http-server"));
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("axum server should run");
    });
    format!("http://{address}")
}

fn test_service(name: &str) -> TradeAssemblyService {
    TradeAssemblyService::test_local(temp_db(name).to_string_lossy().to_string())
}

fn temp_db(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "tradeassembly-http-parity-{name}-{}-{stamp}-{}.db",
        std::process::id(),
        NEXT_DB_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn assert_error(
    response: tradeassembly_runtime::service::ServiceResponse,
    status: u16,
    code: &str,
) {
    assert_eq!(response.status, status);
    assert_eq!(response.body["detail"], code);
    assert_eq!(response.body["error"]["code"], code);
}

fn concrete_route(route: &str) -> (Method, String) {
    let (method, path) = route.split_once(' ').expect("METHOD /path route");
    let method = Method::from_bytes(method.as_bytes()).expect("http method");
    let path = path
        .replace("{ref}", "alpaca-paper")
        .replace("{strategy_id}", "strat_local_btc_demo")
        .replace("{backtest_id}", "backtest_local")
        .replace("{order_id}", "order_local")
        .replace("{operation_id}", "status")
        .replace("{operation}", "status");
    (method, path)
}

fn default_body() -> Value {
    json!({
        "strategyId": "strat_local_btc_demo",
        "providerRef": "alpaca-paper",
        "provider_ref": "alpaca-paper",
        "ref": "alpaca-paper",
        "snapshotId": "latest",
        "alertId": "local_test_alert",
        "activationId": "activation-btc-exit-demo",
        "engine": "deterministic-fixture",
        "mode": "blank",
        "name": "HTTP parity strategy",
        "versionId": "ver_local",
        "expression": "true",
        "stub": true,
        "spec": {"schemaVersion": "tradeassembly.strategy.v1", "name": "HTTP parity"}
    })
}
