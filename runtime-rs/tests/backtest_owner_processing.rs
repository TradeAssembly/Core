use serde_json::{json, Value};
use tradeassembly_runtime::ports::{IdempotencyKey, QueueRequest};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn partition_claim_preserves_other_work_and_fences_expired_leases() {
    let directory = tempfile::tempdir().unwrap();
    let service =
        TradeAssemblyService::test_local(directory.path().join("queue.db").to_string_lossy());
    let runtime = service.runtime();
    for partition in ["other", "mine"] {
        runtime
            .queue
            .enqueue(QueueRequest {
                queue: "scoped-test".into(),
                payload: json!({"runId":partition}),
                idempotency_key: IdempotencyKey::new(partition).unwrap(),
                partition_key: Some(partition.into()),
                priority: 0,
                available_at_ms: 0,
                retention_until_ms: 1000,
                max_attempts: 3,
            })
            .unwrap();
    }
    let first = runtime
        .queue
        .claim_partition("scoped-test", "mine", "a", 0, 50)
        .unwrap()
        .remove(0);
    assert_eq!(first.partition_key.as_deref(), Some("mine"));
    assert!(runtime
        .queue
        .claim_partition("scoped-test", "mine", "b", 49, 50)
        .unwrap()
        .is_empty());
    let second = runtime
        .queue
        .claim_partition("scoped-test", "mine", "b", 50, 50)
        .unwrap()
        .remove(0);
    assert!(second.fencing_token > first.fencing_token);
    assert!(runtime
        .queue
        .acknowledge(&first.message_id, first.fencing_token)
        .is_err());
    runtime
        .queue
        .acknowledge(&second.message_id, second.fencing_token)
        .unwrap();
    assert!(runtime
        .queue
        .claim_partition("scoped-test", "mine", "c", 101, 50)
        .unwrap()
        .is_empty());
    let other = runtime
        .queue
        .claim_partition("scoped-test", "other", "c", 101, 50)
        .unwrap()
        .remove(0);
    assert_eq!(other.attempt, 1);
    assert_eq!(other.partition_key.as_deref(), Some("other"));
    assert!(runtime
        .queue
        .claim_partition("scoped-test", "other", "d", 1000, 50)
        .unwrap()
        .is_empty());
}

fn call(service: &TradeAssemblyService, name: &str, arguments: Value) -> Value {
    let response = service.call_mcp_tool(name, arguments);
    response["structuredContent"].clone()
}

#[test]
fn authenticated_processing_is_exact_run_scoped_and_duplicate_safe() {
    let directory = tempfile::tempdir().unwrap();
    let base =
        TradeAssemblyService::test_local(directory.path().join("runtime.db").to_string_lossy());
    let owner = base.for_authenticated_invocation("test", "owner", None, None);
    let other = base.for_authenticated_invocation("test", "other", None, None);
    let ingestion = call(
        &owner,
        "tradeassembly.dataset_ingestion.create",
        json!({
            "strategyId":"strat_local_btc_demo", "pluginInstanceRef":"local-data",
            "operationId":"marketdata.bars.read_v1", "assetClass":"crypto_spot",
            "instruments":["BTC/USD"], "dataKind":"bars", "granularity":"1m",
            "timeSlice":{"start":"2026-01-02T00:00:00Z","end":"2026-01-02T00:02:00Z"},
            "calendar":"crypto_24x7", "timezone":"UTC",
            "normalizationPolicy":{"timestampUnit":"rfc3339","timezone":"UTC","duplicatePolicy":"reject","priceAdjustment":"raw"},
            "qualityPolicy":{"missingIntervals":"reject","staleObservations":"reject","outliers":"warn","invalidMarkets":"reject","calendarMismatch":"reject","corporateActionGaps":"warn","missingDerivativeFields":"reject"},
            "maxRows":3, "idempotencyKey":"owner-dataset",
            "authorityContext":{"actor":"owner","surface":"test"}
        }),
    );
    assert_eq!(ingestion["status"], "completed", "{ingestion:#}");
    let create = |key: &str| {
        let run = call(
            &owner,
            "tradeassembly.backtest.run",
            json!({
                "strategy_id":"strat_local_btc_demo",
                "dataset_id":ingestion["snapshot"]["datasetId"], "idempotency_key":key
            }),
        );
        assert_eq!(run["status"], "queued", "{run:#}");
        run["runId"].as_str().unwrap().to_owned()
    };
    let first = create("first-run");
    let second = create("second-run");
    let export_request = json!({"backtest_id":second,"export_kind":"reportJson"});
    let early_export = call(
        &owner,
        "tradeassembly.backtest.export",
        export_request.clone(),
    );
    assert_eq!(
        early_export["error"]["code"], "backtest_result_required",
        "{early_export:#}"
    );
    let process = |service: &TradeAssemblyService, run: &str| {
        call(
            service,
            "tradeassembly.backtest.process",
            json!({"run_id":run,"worker":"test-worker","idempotency_key":format!("process-{run}")}),
        )
    };
    let denied = call(
        &other,
        "tradeassembly.backtest.process",
        json!({"run_id":second,"worker":"test-worker","idempotency_key":"foreign-process"}),
    );
    assert_eq!(
        denied["error"]["code"], "object_not_available",
        "{denied:#}"
    );
    let unscoped = call(
        &owner,
        "tradeassembly.backtest.process",
        json!({"worker":"test-worker","idempotency_key":"unscoped"}),
    );
    assert!(unscoped.get("error").is_some(), "{unscoped:#}");
    let completed = process(&owner, &second);
    assert_eq!(completed["runId"], second, "{completed:#}");
    assert_eq!(completed["status"], "completed", "{completed:#}");
    let untouched = call(
        &owner,
        "tradeassembly.backtest.get",
        json!({"run_id":first}),
    );
    assert_eq!(untouched["status"], "queued", "{untouched:#}");
    let repeated = process(&owner, &second);
    assert_eq!(repeated["resultHash"], completed["resultHash"]);
    assert_eq!(repeated["sequence"], completed["sequence"]);
    let fresh = call(
        &owner,
        "tradeassembly.backtest.process",
        json!({"run_id":second,"idempotency_key":"fresh-completed-process"}),
    );
    assert_eq!(fresh["sequence"], completed["sequence"]);
    let exported = call(&owner, "tradeassembly.backtest.export", export_request);
    assert!(exported["contentHash"].is_string(), "{exported:#}");
    let denied_graphql = other.execute_graphql(json!({
        "operationName":"ProcessBacktest", "variables":{"runId":first},
        "query":"mutation ProcessBacktest($runId: String!) { processBacktest(runId: $runId) }"
    }));
    assert!(
        denied_graphql.to_string().contains("object_not_available"),
        "{denied_graphql:#}"
    );
    let graph = owner.execute_graphql(json!({
        "operationName":"ProcessBacktest", "variables":{"runId":first},
        "query":"mutation ProcessBacktest($runId: String!) { processBacktest(runId: $runId) }"
    }));
    assert_eq!(
        graph["data"]["processBacktest"]["status"], "completed",
        "{graph:#}"
    );
}
