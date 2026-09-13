// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use tradeassembly_runtime::service::TradeAssemblyService;

fn service_pair() -> (
    TradeAssemblyService,
    TradeAssemblyService,
    TradeAssemblyService,
) {
    let db = tempfile::Builder::new()
        .prefix("tradeassembly-object-authorization-research-")
        .suffix(".db")
        .tempfile()
        .expect("temporary database")
        .into_temp_path()
        .keep()
        .expect("keep database");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    let alice = base.for_authenticated_invocation(
        "https://issuer-a.example",
        "alice",
        None,
        Some("Alice".to_string()),
    );
    let bob = base.for_authenticated_invocation(
        "https://issuer-b.example",
        "bob",
        None,
        Some("Bob".to_string()),
    );
    (base, alice, bob)
}

fn create_strategy(service: &TradeAssemblyService, id: &str) {
    let response = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({
            "id": id,
            "name": "Private research strategy",
            "symbol": "BTC/USD",
            "providerRef": "sim",
            "creationMode": "import",
        }),
    );
    assert_eq!(response.status, 201, "{:?}", response.body);
    assert_eq!(response.body["ok"], true, "{:?}", response.body);
    let draft_hash = response.body["body"]["draft"]["draftHash"].clone();
    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({"strategyId": id, "expectedDraftHash": draft_hash}),
    );
    assert_eq!(published.status, 200, "{:?}", published.body);
}

fn body(response: Value) -> Value {
    response["body"].clone()
}

#[test]
fn legacy_dataset_path_rejects_provider_data_without_persisting_a_fixture() {
    let (base, alice, _) = service_pair();
    create_strategy(&alice, "provider-data-test");
    let before = base
        .runtime()
        .storage
        .list_json("research_datasets")
        .unwrap();
    let response = alice.handle_http(
        "POST",
        "/research/datasets",
        json!({
            "strategyId": "provider-data-test", "sourcePluginRef": "tradeassembly.alpaca",
            "symbol": "OWNER-SUPPLIED", "rows": [{"close": 1}]
        }),
    );
    let result = body(response.body);
    assert_eq!(result["error"]["code"], "provider_ingestion_required");
    assert!(result.get("datasetId").is_none());
    assert_eq!(result["nextAction"]["tool"], "tradeassembly.plugin.list");
    assert_eq!(result["nextAction"]["arguments"], json!({}));
    assert_eq!(
        base.runtime()
            .storage
            .list_json("research_datasets")
            .unwrap(),
        before
    );

    let response = alice.handle_http(
        "POST",
        "/research/datasets",
        json!({
            "strategyId": "provider-data-test", "sourcePluginRef": "tradeassembly.local-data",
            "symbol": "OWNER-SUPPLIED", "rows": [{"close": 1}]
        }),
    );
    let result = body(response.body);
    assert!(result["datasetId"].is_string());
    assert_eq!(result["records"], json!([{"close": 1}]));
    assert_eq!(
        base.runtime()
            .storage
            .list_json("research_datasets")
            .unwrap()
            .len(),
        before.len() + 1
    );
}

#[test]
fn cross_principal_research_roots_are_private_and_opaque() {
    let (_base, alice, bob) = service_pair();
    let strategy_id = "strat_private_research";
    create_strategy(&alice, strategy_id);

    let dataset = alice.handle_http(
        "POST",
        "/research/datasets",
        json!({"strategyId": strategy_id, "symbol": "BTC/USD"}),
    );
    assert_eq!(dataset.status, 200, "{:?}", dataset.body);
    let dataset = body(dataset.body);
    let dataset_id = dataset["datasetId"]
        .as_str()
        .expect("dataset id")
        .to_string();

    let universe = alice.handle_http(
        "POST",
        "/research/universes",
        json!({"strategyId": strategy_id, "symbols": ["BTC/USD"]}),
    );
    assert_eq!(universe.status, 200, "{:?}", universe.body);
    let universe_id = body(universe.body)["id"]
        .as_str()
        .expect("universe id")
        .to_string();

    // Research-job authorization is exercised through the explicit research
    // job fixture route; the legacy front door is now a durable backtest alias.
    let run = alice.handle_http(
        "POST",
        "/research/jobs",
        json!({"strategyId": strategy_id, "datasetId": dataset_id, "symbol": "BTC/USD"}),
    );
    assert_eq!(run.status, 200, "{:?}", run.body);
    let run = body(run.body);
    let job_id = run["id"].as_str().expect("job id").to_string();

    let owner_jobs = alice.handle_http("GET", "/research/jobs", json!({}));
    assert!(owner_jobs
        .body
        .as_array()
        .expect("owner jobs")
        .iter()
        .any(|job| job["id"].as_str() == Some(job_id.as_str())));
    let foreign_jobs = bob.handle_http("GET", "/research/jobs", json!({}));
    assert!(!serde_json::to_string(&foreign_jobs.body)
        .expect("serialize foreign jobs")
        .contains(&job_id));

    let owner_universes = alice.handle_http("GET", "/research/universes", json!({}));
    assert!(owner_universes
        .body
        .as_array()
        .expect("owner universes")
        .iter()
        .any(|universe| universe["id"].as_str() == Some(universe_id.as_str())));
    let foreign_universes = bob.handle_http("GET", "/research/universes", json!({}));
    assert!(!serde_json::to_string(&foreign_universes.body)
        .expect("serialize foreign universes")
        .contains(&universe_id));

    let foreign_status = bob.handle_http(
        "POST",
        "/product/strategies/research/jobs/status",
        json!({"jobId": job_id}),
    );
    let missing_status = bob.handle_http(
        "POST",
        "/product/strategies/research/jobs/status",
        json!({"jobId": "job_missing"}),
    );
    assert_eq!(foreign_status.status, missing_status.status);
    assert_eq!(foreign_status.body, missing_status.body);
    assert!(!serde_json::to_string(&foreign_status.body)
        .expect("serialize foreign status")
        .contains(&job_id));

    let foreign_dataset = bob.handle_http(
        "POST",
        "/research/datasets",
        json!({"strategyId": strategy_id, "symbol": "BTC/USD"}),
    );
    assert_eq!(foreign_dataset.status, 200);
    let serialized = serde_json::to_string(&foreign_dataset.body).expect("serialize dataset");
    assert!(serialized.contains("object_not_available"));
    assert!(!serialized.contains(&dataset_id));
}
