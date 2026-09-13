use serde_json::{json, Value};
use std::collections::BTreeMap;
use tradeassembly_runtime::order_updates::{normalize_broker_order_update, OrderStatusService};
use tradeassembly_runtime::risk::{
    check_order_risk, AttributionScope, OrderPlan, RiskAccountState, RiskLimits,
};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn buying_power_rejection_fails_before_provider_submit_and_creates_no_reservation() {
    let fixture = fixture("buying-power-rejection");
    let request = &fixture["request"];
    let plan = OrderPlan {
        symbol: request["symbol"].as_str().unwrap().to_string(),
        side: request["side"].as_str().unwrap().to_string(),
        quantity: request["quantity"].as_f64().unwrap(),
        limit_price: request["limit_price"].as_f64(),
    };
    let account = RiskAccountState {
        account_equity: request["account"]["account_equity"].as_f64().unwrap(),
        buying_power: request["account"]["buying_power"].as_f64().unwrap(),
        daily_realized_pnl: request["account"]["daily_realized_pnl"].as_f64().unwrap(),
    };
    let rows = vec![BTreeMap::from([(
        "close".to_string(),
        json!(request["limit_price"].as_f64().unwrap()),
    )])];

    let denied = check_order_risk(
        &plan,
        &RiskLimits {
            max_notional: 100_000.0,
            max_order_quantity: 1.0,
            max_daily_loss: 1_000.0,
            max_concurrent_positions: 5,
            max_buying_power_pct: None,
            max_capital_at_risk_pct: None,
            max_margin_used_pct: None,
            max_delta_abs: None,
            attribution_scope: AttributionScope::Strict,
        },
        &rows,
        &[],
        Some(&account),
        &[],
    )
    .expect_err("fixture should fail local risk before provider submit");

    assert!(denied.reason.contains(
        fixture["expected"]["denied_reason_contains"]
            .as_str()
            .unwrap()
    ));
    assert_eq!(fixture["expected"]["reservation_created"], false);
}

#[test]
fn retry_after_unknown_submit_state_uses_idempotency_and_keeps_reservation() {
    let fixture = fixture("network-timeout-submit-unknown");
    let db = test_db("provider-outage-timeout");
    let service = TradeAssemblyService::test_local(&db);
    let request = fixture["request"].clone();

    let first = service.handle_http("POST", "/product/run-center/run-once", request.clone());
    assert_eq!(first.status, 200);
    assert_eq!(first.body["ok"], true);
    assert_eq!(first.body["body"]["risk"]["reserved"], true);

    let second = service.handle_http("POST", "/product/run-center/run-once", request);
    assert_eq!(second.status, 200);
    assert_eq!(second.body["duplicate"], true);
    assert_eq!(
        second.body["body"]["order_id"],
        first.body["body"]["order_id"]
    );

    let orders = service.handle_http("GET", "/orders", json!({}));
    assert_eq!(orders.body.as_array().unwrap().len(), 1);
    let reservations = service.handle_http("GET", "/risk/reservations", json!({}));
    assert_eq!(reservations.body.as_array().unwrap().len(), 1);
    assert_eq!(
        reservations.body[0]["status"],
        fixture["expected"]["reservation_status"]
    );
}

#[test]
fn accepted_stream_disconnect_preserves_reserved_exposure_until_terminal_event() {
    let fixture = fixture("accepted-then-stream-disconnect");
    let mut service = seeded_order_status_service(&fixture);

    let result = service.ingest_broker_order_status_stream(
        fixture["provider_ref"].as_str().unwrap(),
        &fixture["stream"],
    );

    assert_eq!(result["ingested"], 1);
    assert_eq!(
        service.order_status(fixture["seed"]["broker_order_id"].as_str().unwrap()),
        fixture["expected"]["order_status"].as_str()
    );
    assert_eq!(service.active_risk_reservations().len(), 1);
}

#[test]
fn duplicate_terminal_order_updates_release_risk_once_and_replay_deterministically() {
    let fixture = fixture("duplicate-order-update-event");
    let mut service = seeded_order_status_service(&fixture);

    let first = service.ingest_broker_order_status_stream(
        fixture["provider_ref"].as_str().unwrap(),
        &json!({
            "stream": fixture["stream"]["stream"],
            "events": [fixture["stream"]["events"][0].clone()]
        }),
    );
    let second = service.ingest_broker_order_status_stream(
        fixture["provider_ref"].as_str().unwrap(),
        &json!({
            "stream": fixture["stream"]["stream"],
            "events": [fixture["stream"]["events"][1].clone()]
        }),
    );

    assert_eq!(first["ingested"], 1);
    assert_eq!(second["ingested"], 1);
    assert!(service.active_risk_reservations().is_empty());
    assert_eq!(
        service.replay_count("risk.released"),
        fixture["expected"]["risk_release_count"].as_u64().unwrap()
    );
}

#[test]
fn terminal_events_release_reservations_and_normalize_provider_statuses() {
    let fixture = fixture("terminal-events");
    for event in fixture["terminal_events"].as_array().unwrap() {
        let event = event.as_str().unwrap();
        let broker_order_id = format!("terminal-entry-{event}");
        let client_order_id = format!("client-terminal-entry-{event}");
        let mut service =
            OrderStatusService::seed_accepted_entry(&broker_order_id, &client_order_id);
        let update = json!({
            "stream": "trade_updates",
            "events": [{
                "eventId": format!("evt-{event}-1"),
                "event": event,
                "order": {
                    "orderId": broker_order_id,
                    "clientOrderId": client_order_id,
                    "status": event,
                    "filledQty": "0",
                    "qty": "1"
                }
            }]
        });

        let normalized =
            normalize_broker_order_update("alpaca-paper", &update["events"][0], "trade_updates");
        assert_eq!(
            normalized.status,
            fixture["expected"]["normalized_status"][event]
                .as_str()
                .unwrap()
        );
        let result = service.ingest_broker_order_status_stream("alpaca-paper", &update);
        assert_eq!(result["ingested"], 1);
        assert!(service.active_risk_reservations().is_empty());
        assert_eq!(service.replay_count("risk.released"), 1);
    }
}

#[test]
fn credential_missing_or_expired_errors_are_redacted() {
    let fixture = fixture("credential-missing-expired");
    let service = TradeAssemblyService::test_local(test_db("provider-outage-credentials"));

    let response = service.handle_http(
        "POST",
        &format!(
            "/providers/{}/credentials/test",
            fixture["provider_ref"].as_str().unwrap()
        ),
        fixture["request"].clone(),
    );

    assert_eq!(response.status, 200);
    assert_eq!(response.body["body"]["ok"], fixture["expected"]["ok"]);
    assert_eq!(
        response.body["body"]["configured"],
        fixture["expected"]["configured"]
    );
    assert_eq!(
        response.body["body"]["message"],
        fixture["expected"]["message"]
    );
    let body = response.body.to_string();
    for secret_key in ["api_key", "api_secret"] {
        let secret = fixture["request"][secret_key].as_str().unwrap();
        assert!(!body.contains(secret), "{secret_key} leaked into response");
    }
}

fn seeded_order_status_service(fixture: &Value) -> OrderStatusService {
    OrderStatusService::seed_accepted_entry(
        fixture["seed"]["broker_order_id"].as_str().unwrap(),
        fixture["seed"]["client_order_id"].as_str().unwrap(),
    )
}

fn fixture(name: &str) -> Value {
    let body = match name {
        "buying-power-rejection" => {
            include_str!("../fixtures/provider/alpaca/buying-power-rejection.json")
        }
        "network-timeout-submit-unknown" => {
            include_str!("../fixtures/provider/alpaca/network-timeout-submit-unknown.json")
        }
        "accepted-then-stream-disconnect" => {
            include_str!("../fixtures/provider/alpaca/accepted-then-stream-disconnect.json")
        }
        "duplicate-order-update-event" => {
            include_str!("../fixtures/provider/alpaca/duplicate-order-update-event.json")
        }
        "terminal-events" => include_str!("../fixtures/provider/alpaca/terminal-events.json"),
        "credential-missing-expired" => {
            include_str!("../fixtures/provider/alpaca/credential-missing-expired.json")
        }
        other => panic!("unknown fixture {other}"),
    };
    serde_json::from_str(body).expect("fixture parses")
}

fn test_db(name: &str) -> String {
    format!(".tradeassembly/test-{name}-{}.db", std::process::id())
}
