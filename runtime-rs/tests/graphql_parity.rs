use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use tradeassembly_runtime::finance_authority::TestFinanceAuthority;
use tradeassembly_runtime::{
    http::build_router,
    runtime_config::{RuntimeBuilder, RuntimeConfig},
    service::{TradeAssemblyService, GRAPHQL_OPERATIONS},
};

static NEXT_DB_ID: AtomicU64 = AtomicU64::new(1);
const STUDIO_CORE_TOKEN: &str = "graphql-parity-studio-core-token-with-32-bytes";
const STUDIO_ISSUER: &str = "https://id.graphql-parity.test";
const STUDIO_SUBJECT: &str = "graphql-parity-user";
const STUDIO_AUDIENCE: &str = "tradeassembly-graphql-parity";

#[tokio::test]
async fn rust_graphql_endpoint_supports_standard_introspection() {
    let payload = post_graphql_unauthenticated(json!({
        "query": "query IntrospectionQuery { __schema { queryType { name } mutationType { name } } }"
    }))
    .await;

    assert!(
        payload.get("errors").is_none(),
        "standard introspection must execute through Rust GraphQL schema: {payload:#}"
    );
    assert_eq!(payload["data"]["__schema"]["queryType"]["name"], "Query");
    assert_eq!(
        payload["data"]["__schema"]["mutationType"]["name"],
        "Mutation"
    );
}

#[tokio::test]
async fn rust_graphql_endpoint_rejects_missing_or_invalid_studio_transport_authority() {
    let body = trusted_studio_request(json!({
        "operationName": "StrategyLibrary",
        "query": "query StrategyLibrary { strategyLibrary }",
        "variables": {}
    }));
    let missing = send_graphql(test_service("graphql-auth-missing"), body.clone(), None).await;
    assert_eq!(missing.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing.1["error"]["code"],
        "studio_transport_authentication_required"
    );

    let invalid = send_graphql(
        test_service("graphql-auth-invalid"),
        body,
        Some("wrong-studio-core-token-with-at-least-32-bytes"),
    )
    .await;
    assert_eq!(invalid.0, StatusCode::UNAUTHORIZED);
    assert_eq!(invalid.1["error"]["code"], "studio_identity_invalid");
}

#[tokio::test]
async fn rust_graphql_endpoint_rejects_forged_oidc_actor_assertions() {
    let mut body = trusted_studio_request(json!({
        "operationName": "StrategyLibrary",
        "query": "query StrategyLibrary { strategyLibrary }",
        "variables": {}
    }));
    body["variables"]["_studioSession"]["actor"] = json!("oidc:forged");
    let response = send_graphql(
        test_service("graphql-auth-forged-actor"),
        body,
        Some(STUDIO_CORE_TOKEN),
    )
    .await;
    assert_eq!(response.0, StatusCode::UNAUTHORIZED);
    assert_eq!(response.1["error"]["code"], "studio_identity_invalid");

    let mut unknown_field = trusted_studio_request(json!({
        "operationName": "StrategyLibrary",
        "query": "query StrategyLibrary { strategyLibrary }",
        "variables": {}
    }));
    unknown_field["variables"]["_studioSession"]["injectedAuthority"] = json!("forged");
    let response = send_graphql(
        test_service("graphql-auth-unknown-assertion-field"),
        unknown_field,
        Some(STUDIO_CORE_TOKEN),
    )
    .await;
    assert_eq!(response.0, StatusCode::UNAUTHORIZED);
    assert_eq!(response.1["error"]["code"], "studio_oidc_session_required");
}

#[tokio::test]
async fn trusted_studio_transport_preserves_safe_invalid_strategy_imports() {
    let service = test_service("graphql-trusted-strategy-import");
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3/valid/static-equity.json");
    let mut spec: Value =
        serde_json::from_slice(&std::fs::read(fixture_path).expect("StrategySpec fixture"))
            .expect("StrategySpec JSON");
    spec["name"] = json!("Imported invalid StrategySpec");
    spec["trade_market_requirements"] = json!([]);

    let imported = post_graphql_on(
        service,
        json!({
            "operationName": "CreateStrategy",
            "query": "mutation CreateStrategy { createStrategy }",
            "variables": {
                "creationMode": "import",
                "name": "Imported invalid StrategySpec",
                "spec": spec,
            }
        }),
    )
    .await;

    assert!(
        imported.get("errors").is_none(),
        "trusted Studio import failed: {imported:#}"
    );
    assert_eq!(
        imported["data"]["createStrategy"]["body"]["draft"]["status"],
        "invalid"
    );
}

#[tokio::test]
async fn trusted_studio_transport_binds_sightline_actions_to_the_oidc_actor() {
    let service = test_service("graphql-trusted-sightline");
    let published = post_graphql_on(
        service.clone(),
        json!({
            "operationName": "PublishSightlineStudioSurface",
            "query": "mutation PublishSightlineStudioSurface { publishSightlineStudioSurface }",
            "variables": {
                "route": "/app/strategies/strat_local_btc_demo/builder"
            }
        }),
    )
    .await;
    assert_eq!(
        published["data"]["publishSightlineStudioSurface"]["ok"], true,
        "{published:#}"
    );

    let selected = post_graphql_on(
        service.clone(),
        json!({
            "operationName": "SetSightlineSelection",
            "query": "mutation SetSightlineSelection { setSightlineSelection }",
            "variables": {
                "strategyId": "strat_local_btc_demo",
                "nodeRef": "strategy.risk",
                "actor": {"kind": "agent", "id": "forged-agent"},
                "principalUser": "forged-user"
            }
        }),
    )
    .await;
    let selected_actor =
        &selected["data"]["setSightlineSelection"]["body"]["selection"]["selectedBy"];
    assert_eq!(selected_actor["actorType"], "user", "{selected:#}");
    assert_eq!(
        selected_actor["actorId"],
        stable_studio_actor(),
        "{selected:#}"
    );
    let selection_id = selected["data"]["setSightlineSelection"]["body"]["selection"]
        ["selectionId"]
        .as_str()
        .expect("selection id")
        .to_string();

    let shared = post_graphql_on(
        service.clone(),
        json!({
            "operationName": "ShareSightlineSelection",
            "query": "mutation ShareSightlineSelection { shareSightlineSelection }",
            "variables": {
                "selectionId": selection_id,
                "actor": {"kind": "agent", "id": "forged-agent"}
            }
        }),
    )
    .await;
    assert_eq!(
        shared["data"]["shareSightlineSelection"]["body"]["selection"]["shared"], true,
        "{shared:#}"
    );

    let persisted = service
        .runtime()
        .storage
        .list_json("sightline_events")
        .expect("Sightline events");
    let serialized = serde_json::to_string(&persisted).expect("serialize Sightline evidence");
    assert!(serialized.contains(&stable_studio_actor()));
    assert!(!serialized.contains(STUDIO_CORE_TOKEN));
    assert!(!serialized.contains(STUDIO_SUBJECT));
    let command_evidence =
        serde_json::to_string(&service.control_plane_commands()).expect("command evidence");
    assert!(!command_evidence.contains(STUDIO_CORE_TOKEN));
    assert!(!command_evidence.contains(STUDIO_SUBJECT));

    let untrusted = service.execute_graphql(json!({
        "operationName": "SetSightlineSelection",
        "query": "mutation SetSightlineSelection { setSightlineSelection }",
        "variables": {
            "strategyId": "strat_local_btc_demo",
            "nodeRef": "strategy.risk",
            "actor": {"kind": "user", "id": stable_studio_actor()}
        }
    }));
    assert_eq!(
        untrusted["errors"][0]["message"], "sightline_user_authority_required",
        "{untrusted:#}"
    );
}

#[tokio::test]
async fn rust_graphql_endpoint_covers_registered_studio_operations() {
    for operation_name in GRAPHQL_OPERATIONS {
        let payload = post_graphql(json!({
            "operationName": operation_name,
            "query": format!("query {operation_name} {{ __typename }}"),
            "variables": {
                "strategyId": "strat_local_btc_demo",
                "providerRef": "sim",
                "ref": "sim",
                "snapshotId": "latest",
                "alertId": "local_test_alert",
                "activationId": "activation-btc-exit-demo",
                "engine": "deterministic-fixture",
                "brief": "Draft local BTC paper strategy",
                "market": "crypto",
                "horizon": "intraday",
                "mode": "blank",
                "name": "GraphQL parity strategy",
                "versionId": "ver_local",
                "expression": "true",
                "spec": {"schemaVersion": "tradeassembly.strategy.v1", "name": "GraphQL parity"}
            }
        }))
        .await;

        assert!(
            payload.get("errors").is_none(),
            "{operation_name} returned GraphQL errors: {payload:#}"
        );
        assert!(
            payload.get("data").and_then(Value::as_object).is_some(),
            "{operation_name} did not return a data object: {payload:#}"
        );
    }
}

#[tokio::test]
async fn rust_graphql_schema_advertises_studio_operation_fields() {
    let payload = post_graphql(json!({
        "query": "query IntrospectionQuery { __schema { queryType { fields { name } } mutationType { fields { name } } } }"
    }))
    .await;

    assert!(
        payload.get("errors").is_none(),
        "introspection failed: {payload:#}"
    );
    let query_fields = field_names(&payload["data"]["__schema"]["queryType"]["fields"]);
    let mutation_fields = field_names(&payload["data"]["__schema"]["mutationType"]["fields"]);

    for expected in [
        "localWorkspace",
        "strategyLibrary",
        "strategyBuilder",
        "strategyResearchWorkspace",
        "strategyExecutionWorkspace",
        "runCenter",
        "alertCenter",
        "connectWorkspace",
        "pluginsProviders",
    ] {
        assert!(
            query_fields.contains(&expected.to_string()),
            "missing query field {expected}: {query_fields:?}"
        );
    }

    for expected in [
        "createStrategy",
        "createStrategyAiDraft",
        "saveBuilderDraft",
        "selectStrategySemanticNode",
        "proposeStrategyPatch",
        "reviewStrategyPatch",
        "applyStrategyPatch",
        "validateBuilderDraft",
        "publishStrategy",
        "duplicateStrategy",
        "runBacktest",
        "backtestReportExport",
        "runScenarioValuation",
        "runPortfolioRisk",
        "portfolioRiskReport",
        "fillQualityInspect",
        "fillQualityReport",
        "fillQualityReplay",
        "fillQualityExport",
        "attributionJournalReview",
        "attributionJournalReport",
        "attributionJournalInspect",
        "attributionJournalReplay",
        "attributionJournalExport",
        "runResearchSweep",
        "schedulerRun",
        "reconcileOrders",
        "installPluginManifest",
        "installPluginPackage",
        "createPluginInstance",
        "configurePluginInstance",
        "enablePluginInstance",
        "disablePluginInstance",
        "upgradePluginInstance",
        "rollbackPluginInstance",
        "removePluginInstance",
        "storePluginCredentials",
        "revokePluginCredentials",
        "refreshPluginHealth",
    ] {
        assert!(
            mutation_fields.contains(&expected.to_string()),
            "missing mutation field {expected}: {mutation_fields:?}"
        );
    }
}

#[test]
fn plugin_lifecycle_graphql_mutations_share_durable_handlers_without_echoing_secrets() {
    let service = test_service("plugin-lifecycle-graphql");
    install_credential_fixture(&service);
    let created = service.execute_graphql(json!({
        "operationName": "CreatePluginInstance",
        "query": "mutation CreatePluginInstance { createPluginInstance(instanceRef: \"graphql-fixture\", pluginRef: \"example.graphql-plugin\") }",
        "variables": {
            "instanceRef": "graphql-fixture",
            "pluginRef": "example.graphql-plugin",
            "configuration": {
                "base_url": "https://plugin.example.invalid",
                "account_mode": "paper"
            }
        }
    }));
    assert!(created.get("errors").is_none(), "{created:#}");
    assert_eq!(
        created["data"]["createPluginInstance"]["instance"]["instanceRef"],
        "graphql-fixture"
    );

    let secret = "graphql-secret-must-not-echo";
    let stored = service.execute_graphql(json!({
        "operationName": "StorePluginCredentials",
        "query": "mutation StorePluginCredentials { storePluginCredentials(instanceRef: \"graphql-fixture\", credentials: {}) }",
        "variables": {
            "instanceRef": "graphql-fixture",
            "credentials": {
                "api_key": "graphql-key-must-not-echo",
                "api_secret": secret
            }
        }
    }));
    assert!(stored.get("errors").is_none(), "{stored:#}");
    assert_eq!(
        stored["data"]["storePluginCredentials"]["credentialStatus"]["configured"],
        true
    );
    assert!(!stored.to_string().contains(secret));
    assert!(!stored.to_string().contains("graphql-key-must-not-echo"));

    let fetched = service.execute_graphql(json!({
        "operationName": "PluginInstance",
        "query": "query PluginInstance { pluginInstance(instanceRef: \"graphql-fixture\") }",
        "variables": {"instanceRef": "graphql-fixture"}
    }));
    assert!(fetched.get("errors").is_none(), "{fetched:#}");
    assert_eq!(
        fetched["data"]["pluginInstance"]["credentialStatus"]["configured"],
        true
    );
    assert!(!fetched.to_string().contains(secret));
}

fn install_credential_fixture(service: &TradeAssemblyService) {
    let records = service.handle_http("GET", "/plugins/manifests", json!({}));
    assert_eq!(records.status, 200, "{:#}", records.body);
    let mut manifest = records.body["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
        .expect("simbroker manifest")["manifest"]
        .clone();
    manifest["metadata"]["id"] = json!("example.graphql-plugin");
    manifest["metadata"]["name"] = json!("GraphQL Credential Fixture");
    manifest["metadata"]["provider"] = json!({"id": "example", "name": "Example"});
    manifest["configuration"] = json!({
        "fields": [
            {"id": "api_key", "label": "API key", "description": "Fixture key.", "inputType": "password", "required": true, "storageClass": "credential"},
            {"id": "api_secret", "label": "API secret", "description": "Fixture secret.", "inputType": "password", "required": true, "storageClass": "credential"},
            {"id": "base_url", "label": "API URL", "description": "Fixture endpoint.", "inputType": "url", "required": true, "storageClass": "configuration"},
            {"id": "account_mode", "label": "Account mode", "description": "Fixture mode.", "inputType": "select", "required": true, "storageClass": "configuration", "options": ["paper"]}
        ]
    });
    manifest["health"] = json!({
        "requiredConfiguration": ["base_url", "account_mode"],
        "requiredCredentials": ["api_key", "api_secret"],
        "connectionCheckOperation": null
    });
    let bytes = serde_json_canonicalizer::to_vec(&manifest).expect("canonical manifest");
    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let installed = service.handle_http(
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": manifest,
            "source": {
                "type": "git",
                "commit": commit,
                "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#example.graphql-plugin")
            },
            "integrity": {"manifestSha256": digest},
            "trust": {"level": "local-test"}
        }),
    );
    assert_eq!(installed.status, 200, "{:#}", installed.body);
}

async fn post_graphql(body: Value) -> Value {
    post_graphql_on(test_service("graphql-router"), body).await
}

async fn post_graphql_on(service: TradeAssemblyService, body: Value) -> Value {
    let (status, body) = send_graphql(
        service,
        trusted_studio_request(body),
        Some(STUDIO_CORE_TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    body
}

async fn post_graphql_unauthenticated(body: Value) -> Value {
    let (status, body) = send_graphql(test_service("graphql-router-public"), body, None).await;
    assert_eq!(status, StatusCode::OK);
    body
}

async fn send_graphql(
    service: TradeAssemblyService,
    body: Value,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/graphql")
        .header("content-type", "application/json");
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    let response = build_router(service)
        .oneshot(
            request
                .body(Body::from(body.to_string()))
                .expect("graphql request"),
        )
        .await
        .expect("graphql response");

    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    (status, serde_json::from_slice(&body).expect("json body"))
}

fn test_service(name: &str) -> TradeAssemblyService {
    let db = temp_db(name);
    let token_path = db.with_extension("studio-core.token");
    std::fs::write(&token_path, STUDIO_CORE_TOKEN).expect("write Studio Core test token");
    let db = db.to_string_lossy().to_string();
    let mut config = RuntimeConfig::local(db.clone());
    config.oidc_issuer = STUDIO_ISSUER.to_string();
    config.oidc_audience = STUDIO_AUDIENCE.to_string();
    config.oidc_client_id = STUDIO_AUDIENCE.to_string();
    config.studio_core_token_ref = Some(format!("file://{}", token_path.display()));
    let (runtime, _) = RuntimeBuilder::new(config)
        .with_finance_authority(Arc::new(TestFinanceAuthority))
        .build()
        .expect("trusted Studio test runtime");
    TradeAssemblyService::from_runtime(db, runtime)
}

fn trusted_studio_request(mut body: Value) -> Value {
    let actor = stable_studio_actor();
    let variables = body
        .as_object_mut()
        .expect("GraphQL request object")
        .entry("variables")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("GraphQL variables object");
    variables.insert(
        "_studioSession".to_string(),
        json!({
            "actor": actor,
            "audience": [STUDIO_AUDIENCE],
            "displayName": "GraphQL Parity User",
            "email": "graphql-parity@example.test",
            "expiresAtMs": 1_900_000_000_000_i64,
            "issuer": STUDIO_ISSUER,
            "subject": STUDIO_SUBJECT
        }),
    );
    body
}

fn stable_studio_actor() -> String {
    format!(
        "oidc:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(
            format!("{STUDIO_ISSUER}\0{STUDIO_SUBJECT}").as_bytes()
        ))
    )
}

fn temp_db(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "tradeassembly-graphql-parity-{name}-{}-{stamp}-{}.db",
        std::process::id(),
        NEXT_DB_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

fn field_names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("fields array")
        .iter()
        .filter_map(|field| field["name"].as_str().map(ToOwned::to_owned))
        .collect()
}
