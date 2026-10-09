use clap::Parser;
use serde_json::{json, Value};
use tempfile::TempDir;
use tradeassembly_runtime::cli::{execute_command, Cli};
use tradeassembly_runtime::service::TradeAssemblyService;

const SECRET: &str = "DATASET_INTERFACE_SECRET_MUST_NOT_LEAK";

#[test]
fn ingestion_discovery_exposes_required_policies_and_resolvable_enum_schema() {
    let tools = tradeassembly_runtime::mcp::tool_definitions();
    let schema = &tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "tradeassembly.dataset_ingestion.create")
        .unwrap()["inputSchema"];
    let validator = jsonschema::validator_for(schema).expect("nested policy references resolve");
    let original = request("schema-only");
    let mut input = json!({});
    for (source, target) in [
        ("strategyId", "strategy_id"),
        ("pluginInstanceRef", "plugin_instance_ref"),
        ("operationId", "operation_id"),
        ("assetClass", "asset_class"),
        ("instruments", "instruments"),
        ("dataKind", "data_kind"),
        ("granularity", "granularity"),
        ("timeSlice", "time_slice"),
        ("calendar", "calendar"),
        ("normalizationPolicy", "normalization_policy"),
        ("qualityPolicy", "quality_policy"),
        ("idempotencyKey", "idempotency_key"),
    ] {
        input[target] = original[source].clone();
    }
    assert!(validator.is_valid(&input), "{input:#}");
    input["quality_policy"]["outliers"] = json!("invented");
    assert!(!validator.is_valid(&input));
    input["quality_policy"]["outliers"] = json!("reject");
    input["normalization_policy"]
        .as_object_mut()
        .unwrap()
        .remove("timestampUnit");
    assert!(!validator.is_valid(&input));
}

#[test]
fn historical_dataset_content_is_equivalent_with_bounded_mcp_pages() {
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
    assert_eq!(
        http.body["snapshot"]["content"]["observations"],
        graphql_payload["snapshot"]["content"]["observations"]
    );
    assert!(mcp_payload["snapshot"]["content"]
        .get("observations")
        .is_none());
    assert_eq!(mcp_payload["observationPage"]["total"], 3);
    let ingestion_id = &mcp_payload["ingestion"]["ingestionId"];
    let page = |offset: Value, limit: Value| {
        service.call_mcp_tool(
        "tradeassembly.dataset_ingestion.get",
        json!({"ingestion_id":ingestion_id,"observation_offset":offset,"observation_limit":limit}),
    )["structuredContent"].clone()
    };
    let first = page(json!(0), json!(2));
    let second = page(json!(2), json!(2));
    assert_eq!(first["observationPage"]["nextOffset"], 2);
    assert!(second["observationPage"]["nextOffset"].is_null());
    assert_eq!(
        first["snapshot"]["contentHash"],
        mcp_payload["snapshot"]["contentHash"]
    );
    let mut rows = first["snapshot"]["content"]["observations"]
        .as_array()
        .unwrap()
        .clone();
    rows.extend(
        second["snapshot"]["content"]["observations"]
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    assert_eq!(
        json!(rows),
        http.body["snapshot"]["content"]["observations"]
    );
    assert_eq!(page(json!(100), json!(2))["observationPage"]["returned"], 0);
    for (offset, limit) in [
        (json!(-1), json!(1)),
        (json!(0), json!(1001)),
        (json!(0), json!("all")),
    ] {
        assert_eq!(
            page(offset, limit)["error"]["code"],
            "dataset_observation_page_invalid"
        );
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
fn authenticated_dataset_ingestion_list_survives_reopen_and_filters_foreign_owner() {
    let temp = TempDir::new().expect("temp dir");
    let database = temp.path().join("tradeassembly.db");
    let base = TradeAssemblyService::test_local(database.display().to_string());
    let owner = base.for_authenticated_invocation(
        "https://issuer-a.example",
        "alice",
        None,
        Some("Alice".to_string()),
    );
    let created = owner.handle_http("POST", "/dataset-ingestions", request("owner-list"));
    assert_eq!(created.status, 201, "{:#}", created.body);
    let ingestion_id = created.body["ingestion"]["ingestionId"]
        .as_str()
        .expect("ingestion id")
        .to_string();
    let snapshot_id = created.body["snapshot"]["datasetId"]
        .as_str()
        .expect("snapshot id")
        .to_string();

    let reopened = TradeAssemblyService::test_local(database.display().to_string());
    let reopened_owner = reopened.for_authenticated_invocation(
        "https://issuer-a.example",
        "alice",
        None,
        Some("Alice".to_string()),
    );
    let listed = reopened_owner.handle_http("GET", "/dataset-ingestions", json!({}));
    let owner_record = listed.body["ingestions"]
        .as_array()
        .expect("ingestion list")
        .iter()
        .find(|record| record["ingestion"]["ingestionId"] == ingestion_id)
        .expect("owner ingestion after reopen");
    assert_eq!(owner_record["snapshot"]["datasetId"], snapshot_id);
    assert_eq!(owner_record["snapshot"]["rowCount"], 3);
    assert_eq!(owner_record["source"]["pluginInstanceRef"], "local-data");

    let foreign = reopened.for_authenticated_invocation(
        "https://issuer-b.example",
        "bob",
        None,
        Some("Bob".to_string()),
    );
    let foreign_list = foreign.handle_http("GET", "/dataset-ingestions", json!({}));
    assert!(!serde_json::to_string(&foreign_list.body)
        .expect("serialize foreign list")
        .contains(&ingestion_id));
    let foreign_page = foreign.call_mcp_tool(
        "tradeassembly.dataset_ingestion.get",
        json!({
            "ingestion_id":ingestion_id, "observation_limit":1000
        }),
    );
    assert!(!foreign_page.to_string().contains(&snapshot_id));
    assert!(!foreign_page.to_string().contains("\"observations\""));
    let owner_list =
        reopened_owner.call_mcp_tool("tradeassembly.dataset_ingestion.list", json!({}));
    assert!(owner_list.to_string().contains(&snapshot_id));
    assert!(!owner_list.to_string().contains("\"observations\""));
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
