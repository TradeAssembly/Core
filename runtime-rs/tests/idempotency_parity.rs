use serde_json::json;
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn execution_and_worker_paths_are_duplicate_safe_and_ignore_stale_events() {
    let db = format!(
        ".tradeassembly/test-idempotency-parity-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let request = json!({
        "activationId": "activation-idem",
        "submitOrders": true,
        "idempotencyKey": "idem-submit-1",
        "authorityContext": {"actor": "local-user", "surface": "test", "accountMode": "paper"},
        "accountMode": "paper",
        "symbol": "BTC/USD",
        "qty": 0.0002
    });

    let first = service.handle_http("POST", "/product/run-center/run-once", request.clone());
    let second = service.handle_http("POST", "/product/run-center/run-once", request);
    assert_eq!(first.status, 200);
    assert_eq!(second.status, 200);
    assert_eq!(second.body["duplicate"], true);
    assert_eq!(
        first.body["body"]["order_id"],
        second.body["body"]["order_id"]
    );

    let order_id = first.body["body"]["order_id"].as_str().expect("order id");
    let fill = service.handle_http(
        "POST",
        &format!("/orders/{order_id}/status-events"),
        json!({"eventId": "evt-fill-idem", "status": "filled", "eventSequence": 10}),
    );
    assert_eq!(fill.body["accepted"], true);
    let stale = service.handle_http(
        "POST",
        &format!("/orders/{order_id}/status-events"),
        json!({"eventId": "evt-stale-idem", "status": "submitted", "eventSequence": 5}),
    );
    assert_eq!(stale.body["accepted"], false);
    assert_eq!(stale.body["reason"], "stale_event");

    let orders = service.handle_http("GET", "/orders", json!({}));
    assert_eq!(
        orders
            .body
            .as_array()
            .unwrap()
            .iter()
            .filter(|order| order["client_order_id"] == "idem-submit-1")
            .count(),
        1
    );
    let order = orders
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|order| order["id"] == order_id)
        .expect("order");
    assert_eq!(order["status"], "filled");

    let config = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({"strategyId": "strat_local_btc_demo", "mode": "paper", "providerRef": "sim"}),
    );
    let activation = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config.body["body"]["configId"],
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "idem-scheduler-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activation.status, 200, "{:#}", activation.body);

    let lease_1 = service.handle_http(
        "POST",
        "/scheduler/run",
        json!({"workerId": "worker-idem", "idempotencyKey": "lease-idem", "maxCycles": 1}),
    );
    let lease_2 = service.handle_http(
        "POST",
        "/scheduler/run",
        json!({"workerId": "worker-idem", "idempotencyKey": "lease-idem", "maxCycles": 1}),
    );
    assert_eq!(lease_1.body["body"]["lease"]["acquired"], true);
    assert_eq!(lease_2.body["duplicate"], true);
    assert_eq!(
        lease_1.body["body"]["lease"]["leaseId"],
        lease_2.body["body"]["lease"]["leaseId"]
    );
}
