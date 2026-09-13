use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn side_effects_require_authority_and_idempotency_context() {
    let service = TradeAssemblyService::test_local(test_db("service-ports-authority"));

    let no_authority = service.record_side_effect(
        "strategy.create",
        json!({"strategy_id": "strat_test"}),
        None,
        Some(IdempotencyKey::new("idem-1").expect("idempotency key")),
    );
    assert_eq!(
        no_authority.expect_err("missing authority must fail"),
        "authority context is required for side effects"
    );

    let no_idempotency = service.record_side_effect(
        "strategy.create",
        json!({"strategy_id": "strat_test"}),
        Some(AuthorityContext::local_cli()),
        None,
    );
    assert_eq!(
        no_idempotency.expect_err("missing idempotency must fail"),
        "idempotency key is required for side effects"
    );
}

#[test]
fn side_effects_write_storage_journal_and_bus_evidence() {
    let service = TradeAssemblyService::test_local(test_db("service-ports-evidence"));
    let journal_id = service
        .record_side_effect(
            "strategy.create",
            json!({"strategy_id": "strat_test", "name": "Test strategy"}),
            Some(AuthorityContext::local_cli()),
            Some(IdempotencyKey::new("idem-strategy-create").expect("idempotency key")),
        )
        .expect("record side effect");

    assert_eq!(journal_id, "journal-local-0001");

    let runtime = service.runtime();
    let stored = runtime
        .storage
        .get_json("side_effects", "strategy.create")
        .expect("read side-effect storage")
        .expect("stored side-effect payload");
    assert_eq!(stored["strategy_id"], "strat_test");

    let events = runtime.journal.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "strategy.create");
    assert_eq!(events[0].authority.actor, "local-user");
    assert_eq!(events[0].idempotency_key.as_str(), "idem-strategy-create");

    let published = runtime.bus.published();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].0, "journal.side_effect_recorded");
    assert_eq!(published[0].1["journal_id"], "journal-local-0001");
}

#[test]
fn local_runtime_exposes_redacted_credential_status_and_provider_capabilities() {
    let service = TradeAssemblyService::test_local(test_db("service-ports-runtime"));
    let runtime = service.runtime();

    let credentials = runtime.credentials.status("alpaca-paper");
    assert_eq!(credentials.provider_ref, "alpaca-paper");
    assert_eq!(credentials.custody, "local/customer-managed");
    assert_eq!(credentials.redacted_display, None);

    let capabilities = runtime.providers.capabilities("alpaca-paper");
    assert!(capabilities.iter().any(|capability| capability.capability
        == "broker.order_submit.paper"
        && capability.available));
}

#[test]
fn duplicate_execution_stale_scheduler_and_out_of_order_order_updates_are_safe() {
    let service = TradeAssemblyService::test_local(test_db("serverless-duplicates"));
    let run_body = json!({
        "activationId": "activation-serverless",
        "submitOrders": true,
        "idempotencyKey": "serverless-run-duplicate",
        "authorityContext": {"actor": "local-user", "surface": "serverless-test", "accountMode": "paper"},
        "accountMode": "paper"
    });

    let first = service.handle_http("POST", "/product/run-center/run-once", run_body.clone());
    let duplicate = service.handle_http("POST", "/product/run-center/run-once", run_body);
    assert_eq!(first.status, 200);
    assert_eq!(duplicate.body["duplicate"], true);
    assert_eq!(
        duplicate.body["body"]["order_id"],
        first.body["body"]["order_id"]
    );
    assert_eq!(
        service
            .handle_http("GET", "/orders", json!({}))
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let scheduler_body = json!({
        "workerId": "worker-serverless",
        "idempotencyKey": "scheduler-trigger-window-1",
        "maxCycles": 1,
        "leaseTtlSeconds": 30
    });
    let scheduler_first = service.handle_http("POST", "/scheduler/run", scheduler_body.clone());
    let scheduler_duplicate = service.handle_http("POST", "/scheduler/run", scheduler_body);
    assert_eq!(scheduler_first.body["body"]["lease"]["duplicate"], false);
    assert_eq!(scheduler_duplicate.body["duplicate"], true);
    assert_eq!(
        scheduler_duplicate.body["body"]["lease"]["leaseId"],
        scheduler_first.body["body"]["lease"]["leaseId"]
    );

    let order_id = first.body["body"]["order_id"].as_str().unwrap();
    let accepted = service.handle_http(
        "POST",
        &format!("/orders/{order_id}/status-events"),
        json!({"eventSequence": 5, "status": "accepted"}),
    );
    let stale = service.handle_http(
        "POST",
        &format!("/orders/{order_id}/status-events"),
        json!({"eventSequence": 3, "status": "filled"}),
    );
    assert_eq!(accepted.body["accepted"], true);
    assert_eq!(stale.body["accepted"], false);
    assert_eq!(stale.body["reason"], "stale_event");
}

fn test_db(name: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "tradeassembly-service-ports-{name}-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("create isolated test directory");
    directory.join("runtime.db").to_string_lossy().to_string()
}
