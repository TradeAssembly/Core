use clap::Parser;
use serde_json::{json, Value};
use tempfile::TempDir;
use tradeassembly_runtime::cli::{execute_command, Cli};
use tradeassembly_runtime::service::TradeAssemblyService;

const SECRET: &str = "DATASET_INTERFACE_SECRET_MUST_NOT_LEAK";

#[test]
fn historical_dataset_payloads_are_equivalent_across_http_graphql_and_mcp() {
    let temp = TempDir::new().expect("temp dir");
    let service = TradeAssemblyService::test_local(
        temp.path().join("tradeassembly.db").display().to_string(),
    );

    let http = service.handle_http("POST", "/dataset-ingestions", request("surface-http"));
    assert_eq!(http.status, 201, "{:#}", http.body);

    let graphql = service.execute_graphql(json!({
        "operationName": "CreateDatasetIngestion",
        "query": "mutation CreateDatasetIngestion($request: JSON!) { createDatasetIngestion(request: $request) }",
        "variables": {"request": request("surface-graphql")}
    }));
    let graphql_payload = &graphql["data"]["createDatasetIngestion"];
    assert!(graphql.get("errors").is_none(), "{graphql:#}");

    let mcp = service.call_mcp_tool(
        "tradeassembly.dataset_ingestion.create",
        request("surface-mcp"),
    );
    assert_eq!(mcp["isError"], false, "{mcp:#}");
    let mcp_payload = &mcp["structuredContent"];

    for payload in [&http.body, graphql_payload, mcp_payload] {
        assert_eq!(payload["status"], "completed", "{payload:#}");
        assert_eq!(payload["snapshot"]["rowCount"], 3);
        assert_eq!(payload["source"]["pluginInstanceRef"], "local-data");
        assert_eq!(payload["noAdvice"], true);
    }
    for payload in [graphql_payload, mcp_payload] {
        assert_eq!(
            http.body["snapshot"]["content"]["observations"],
            payload["snapshot"]["content"]["observations"]
        );
        assert_eq!(
            http.body["snapshot"]["content"]["source"]["sourceRequestHash"],
            payload["snapshot"]["content"]["source"]["sourceRequestHash"]
        );
        assert_eq!(
            http.body["snapshot"]["content"]["source"]["pluginManifestFingerprint"],
            payload["snapshot"]["content"]["source"]["pluginManifestFingerprint"]
        );
        assert_eq!(
            http.body["snapshot"]["content"]["timeSlice"],
            payload["snapshot"]["content"]["timeSlice"]
        );
        assert_eq!(
            http.body["snapshot"]["content"]["qualityPolicy"],
            payload["snapshot"]["content"]["qualityPolicy"]
        );
        assert!(payload["snapshot"]["datasetId"]
            .as_str()
            .is_some_and(|value| value.starts_with("dataset_")));
    }
    let replay = service.handle_http("POST", "/dataset-ingestions", request("surface-http"));
    assert_eq!(replay.status, 201, "{:#}", replay.body);
    assert_eq!(
        replay.body["ingestion"]["ingestionId"],
        http.body["ingestion"]["ingestionId"]
    );

    let records = service.control_plane_commands();
    for (source, key) in [
        ("http", "surface-http"),
        ("graphql", "surface-graphql"),
        ("mcp", "surface-mcp"),
    ] {
        let record = records
            .iter()
            .find(|record| {
                record["envelope"]["source_interface"] == source
                    && record["envelope"]["command_name"] == "research.dataset.create"
            })
            .unwrap_or_else(|| panic!("missing {source} control-plane record: {records:#?}"));
        assert_eq!(record["envelope"]["idempotency_key"], key);
        assert_eq!(record["envelope"]["authority"]["actor"], "user.local");
        assert!(!record.to_string().contains(SECRET));
    }

    let ingestion_id = http.body["ingestion"]["ingestionId"]
        .as_str()
        .expect("ingestion id");
    let verify = service.handle_http(
        "POST",
        &format!("/dataset-ingestions/{ingestion_id}/verify"),
        json!({}),
    );
    assert_eq!(verify.status, 200, "{:#}", verify.body);
    assert_eq!(verify.body["verified"], true);
    assert_eq!(
        verify.body["snapshotId"],
        http.body["snapshot"]["datasetId"]
    );

    let listed = service.handle_http("GET", "/dataset-ingestions", json!({}));
    assert_eq!(listed.body["ingestions"].as_array().unwrap().len(), 3);
    let serialized = serde_json::to_string(&listed.body).expect("serialize list");
    assert!(!serialized.contains(SECRET));
}

#[test]
fn historical_dataset_errors_are_stable_across_surfaces() {
    let temp = TempDir::new().expect("temp dir");
    let service = TradeAssemblyService::test_local(
        temp.path().join("tradeassembly.db").display().to_string(),
    );
    let missing = "ingestion_missing";

    let http = service.handle_http("GET", &format!("/dataset-ingestions/{missing}"), json!({}));
    assert_eq!(http.status, 404);
    assert_eq!(http.body["error"]["code"], "dataset_ingestion_not_found");

    let graphql = service.execute_graphql(json!({
        "operationName": "DatasetIngestionGet",
        "query": "query DatasetIngestionGet($ingestionId: String!) { datasetIngestionGet(ingestionId: $ingestionId) }",
        "variables": {"ingestionId": missing}
    }));
    assert_eq!(
        graphql["errors"][0]["message"],
        "dataset_ingestion_not_found"
    );

    let mcp = service.call_mcp_tool(
        "tradeassembly.dataset_ingestion.get",
        json!({"ingestion_id": missing}),
    );
    assert_eq!(mcp["isError"], true);
    assert_eq!(
        mcp["structuredContent"]["error"]["code"],
        "dataset_ingestion_not_found"
    );
}

#[test]
fn dataset_ingestion_cli_executes_the_same_service_contract() {
    let temp = TempDir::new().expect("temp dir");
    let service = TradeAssemblyService::test_local(
        temp.path().join("tradeassembly.db").display().to_string(),
    );
    let raw = serde_json::to_string(&request("surface-cli")).expect("serialize request");
    let cli = Cli::try_parse_from([
        "tradeassembly",
        "dataset-ingestion",
        "create",
        "--request-json",
        &raw,
    ])
    .expect("parse CLI");
    let payload = execute_command(&service, cli.command);
    assert_eq!(payload["status"], "completed", "{payload:#}");
    assert_eq!(payload["snapshot"]["rowCount"], 3);
    assert!(!serde_json::to_string(&payload).unwrap().contains(SECRET));
    let records = service.control_plane_commands();
    assert!(records.iter().any(|record| {
        record["envelope"]["source_interface"] == "cli"
            && record["envelope"]["command_name"] == "research.dataset.create"
            && record["envelope"]["idempotency_key"] == "surface-cli"
    }));
}

fn request(idempotency_key: &str) -> Value {
    json!({
        "strategyId": "strat_local_btc_demo",
        "pluginInstanceRef": "local-data",
        "operationId": "marketdata.bars.read_v1",
        "assetClass": "crypto_spot",
        "instruments": ["BTC/USD"],
        "dataKind": "bars",
        "granularity": "1m",
        "timeSlice": {
            "start": "2026-01-02T00:00:00Z",
            "end": "2026-01-02T00:02:00Z"
        },
        "calendar": "crypto_24x7",
        "timezone": "UTC",
        "normalizationPolicy": {
            "timestampUnit": "rfc3339",
            "timezone": "UTC",
            "duplicatePolicy": "reject",
            "priceAdjustment": "raw"
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
        "idempotencyKey": idempotency_key,
        "authorityContext": {"actor": "user.local", "surface": "surface-test"},
        "apiSecret": SECRET
    })
}
