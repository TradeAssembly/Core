// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use tradeassembly_runtime::ports::{
    AuthorityContext, IdempotencyKey, ObjectOwner, ObjectScope, SideEffectContext,
};
use tradeassembly_runtime::service::TradeAssemblyService;

fn service_pair() -> (
    TradeAssemblyService,
    TradeAssemblyService,
    TradeAssemblyService,
) {
    let db = tempfile::Builder::new()
        .prefix("tradeassembly-object-authorization-execution-")
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

fn bind_default_strategy(service: &TradeAssemblyService) {
    let owner = ObjectOwner {
        issuer: "https://issuer-a.example".to_string(),
        subject: "alice".to_string(),
        tenant_ref: "personal:https://issuer-a.example:alice".to_string(),
    };
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("object-scope:execution-test").expect("idempotency key"),
    );
    service
        .runtime()
        .object_authorization
        .bind(
            &ObjectScope {
                object_type: "strategy".to_string(),
                object_id: "strat_local_btc_demo".to_string(),
                owner,
                parent_type: None,
                parent_id: None,
            },
            &context,
        )
        .expect("bind default strategy");
}

fn save_config(service: &TradeAssemblyService, strategy_id: &str) -> String {
    let response = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": strategy_id,
            "providerRef": "sim",
            "mode": "paper",
            "riskLimits": {"max_notional": 25.0, "max_order_quantity": 0.0003},
        }),
    );
    assert_eq!(response.status, 200, "{:?}", response.body);
    response.body["body"]["configId"]
        .as_str()
        .expect("config id")
        .to_string()
}

fn unavailable(value: &Value) {
    let value = value.get("body").unwrap_or(value);
    assert_eq!(value["error"]["code"], "object_not_available", "{value:?}");
}

#[test]
fn execution_configuration_activation_and_workspace_are_owner_scoped() {
    let (base, alice, bob) = service_pair();
    bind_default_strategy(&alice);
    let config_id = save_config(&alice, "strat_local_btc_demo");

    let commands_before = base.control_plane_commands().len();
    let foreign_config = bob.handle_http(
        "POST",
        "/product/strategy-execution-configs/activation-readiness",
        json!({"configId": config_id}),
    );
    unavailable(&foreign_config.body);
    assert_eq!(base.control_plane_commands().len(), commands_before);

    let foreign_activation = bob.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({"configId": config_id, "idempotencyKey": "bob-activation"}),
    );
    unavailable(&foreign_activation.body);
    assert_eq!(base.control_plane_commands().len(), commands_before);
    assert!(base
        .runtime()
        .storage
        .list_json("execution_runs")
        .expect("execution runs")
        .is_empty());

    let activation = alice.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config_id,
            "idempotencyKey": "alice-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activation.status, 200, "{:?}", activation.body);
    let activation_id = activation.body["body"]["activationId"]
        .as_str()
        .expect("activation id")
        .to_string();

    let owner_workspace = alice.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo", "activationId": activation_id}),
    );
    assert_eq!(
        owner_workspace.body["selectedActivationId"], activation_id,
        "{:?}",
        owner_workspace.body
    );
    let owner_control = alice.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "pause_entries"}),
    );
    assert_eq!(owner_control.status, 200, "{:?}", owner_control.body);
    assert_eq!(
        owner_control.body["body"]["state"], "paused",
        "{:?}",
        owner_control.body
    );

    let foreign_workspace = bob.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo", "activationId": activation_id}),
    );
    unavailable(&foreign_workspace.body);
    let rendered = serde_json::to_string(&foreign_workspace.body).expect("serialize response");
    assert!(!rendered.contains("strat_local_btc_demo"));
    assert!(!rendered.contains("alice-activation"));
}

#[test]
fn execution_evidence_journal_and_export_refs_do_not_cross_principals() {
    let (_base, alice, bob) = service_pair();
    bind_default_strategy(&alice);
    let config_id = save_config(&alice, "strat_local_btc_demo");
    let activation = alice.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config_id,
            "idempotencyKey": "alice-evidence-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activation.status, 200, "{:?}", activation.body);
    let activation_id = activation.body["body"]["activationId"]
        .as_str()
        .expect("activation id")
        .to_string();

    let owner = alice.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo", "activationId": activation_id}),
    );
    assert!(owner.body["execution"]["evidence"]["exports"].is_array());
    assert!(owner.body["journal"].is_object());

    let foreign_center = bob.handle_http("POST", "/product/run-center", json!({}));
    let rendered_center = serde_json::to_string(&foreign_center.body).expect("serialize center");
    assert!(!rendered_center.contains(&activation_id));
    assert!(!rendered_center.contains("tradeassembly://execution/"));

    let foreign_control = bob.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "pause_entries"}),
    );
    unavailable(&foreign_control.body);

    let owner_deactivate = alice.handle_http(
        "POST",
        "/product/strategy-execution-activations/deactivate",
        json!({"activationId": activation_id}),
    );
    assert_eq!(owner_deactivate.status, 200, "{:?}", owner_deactivate.body);
    assert_eq!(owner_deactivate.body["body"]["status"], "deactivated");
}
