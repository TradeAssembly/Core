use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn providers_credentials_marketdata_options_and_packs_are_service_backed_and_redacted() {
    let db = temp_db("provider-marketdata-parity");
    let service = TradeAssemblyService::test_local(db.to_string_lossy().to_string());
    install_external_alpaca_fixture(&service);
    let provider_ref = "alpaca-paper";

    let missing_credentials = service.handle_http(
        "POST",
        &format!("/providers/{provider_ref}/credentials/test"),
        json!({}),
    );
    assert_eq!(missing_credentials.status, 200);
    assert_eq!(missing_credentials.body["body"]["ok"], false);
    assert_eq!(missing_credentials.body["body"]["configured"], false);
    assert!(!missing_credentials.body.to_string().contains("stub"));

    let mcp_missing = service.call_mcp_tool(
        "tradeassembly.credential.test",
        json!({"provider_ref": provider_ref}),
    );
    assert_eq!(mcp_missing["isError"], false);
    assert_eq!(mcp_missing["structuredContent"]["body"]["ok"], false);
    assert!(!mcp_missing.to_string().contains("stub"));

    let stored = service.handle_http(
        "POST",
        &format!("/providers/{provider_ref}/credentials"),
        json!({
            "api_key": "KEY_SHOULD_NOT_LEAK",
            "api_secret": "SECRET_SHOULD_NOT_LEAK",
            "paper": true
        }),
    );
    assert_eq!(stored.status, 200);
    assert_eq!(stored.body["body"]["stored"], true);
    assert!(!stored.body.to_string().contains("SECRET_SHOULD_NOT_LEAK"));

    let reopened = TradeAssemblyService::test_local(db.to_string_lossy().to_string());
    let status = reopened.handle_http(
        "GET",
        &format!("/providers/{provider_ref}/credentials"),
        json!({}),
    );
    assert_eq!(status.body["configured"], true);
    assert_eq!(status.body["custody"], "local/customer-managed");
    assert_eq!(status.body["redactedDisplay"], "KEY_...LEAK");
    assert!(!status.body.to_string().contains("SECRET_SHOULD_NOT_LEAK"));

    let tested = reopened.handle_http(
        "POST",
        &format!("/providers/{provider_ref}/credentials/test"),
        json!({}),
    );
    assert_eq!(tested.body["body"]["ok"], true);
    assert_eq!(tested.body["body"]["configured"], true);
    assert!(!tested.body.to_string().contains("SECRET_SHOULD_NOT_LEAK"));

    let providers = reopened.handle_http("GET", "/providers", json!({}));
    assert_provider_has_capability(
        &providers.body["providers"],
        provider_ref,
        "marketdata.crypto",
    );
    assert_eq!(
        providers.body["credentialStatus"][provider_ref]["configured"],
        true
    );

    let capabilities = reopened.call_mcp_tool(
        "tradeassembly.plugin.capability_matrix",
        json!({"provider_ref": provider_ref}),
    );
    assert_eq!(capabilities["isError"], false);
    assert!(capabilities["structuredContent"]["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .any(|capability| capability == "marketdata.crypto"));
    let canonical_capabilities = reopened.call_mcp_tool(
        "tradeassembly.plugin.capability_matrix",
        json!({"pluginRef": "tradeassembly.alpaca-paper"}),
    );
    assert_eq!(
        canonical_capabilities["structuredContent"]["pluginRef"],
        "tradeassembly.alpaca-paper"
    );
    assert_eq!(
        canonical_capabilities["structuredContent"]["providerRef"],
        "alpaca-paper"
    );

    let pack_compatibility = reopened.call_mcp_tool(
        "tradeassembly.provider.pack_compatibility",
        json!({"ref": provider_ref, "manifest": instrument_pack_manifest()}),
    );
    assert_eq!(pack_compatibility["isError"], false);
    assert_eq!(pack_compatibility["structuredContent"]["ok"], true);
    assert_eq!(
        pack_compatibility["structuredContent"]["providerRef"],
        provider_ref
    );

    let quote = reopened.handle_http(
        "GET",
        "/marketdata/quote",
        json!({"symbol": "BTC/USD", "providerRef": provider_ref}),
    );
    assert_eq!(quote.body["symbol"], "BTC/USD");
    assert_eq!(quote.body["provider_ref"], provider_ref);
    assert!(quote.body["bid"].as_f64().expect("bid") > 0.0);

    let chain = reopened.handle_http(
        "GET",
        "/marketdata/options/chain",
        json!({"underlying": "SPY", "right": "CALL", "providerRef": provider_ref}),
    );
    assert_eq!(chain.body["underlying"], "SPY");
    assert_eq!(chain.body["provider_ref"], provider_ref);
    assert!(
        chain.body["contracts"][0]["bid"]
            .as_f64()
            .expect("option bid")
            > 0.0
    );

    let selected = reopened.handle_http(
        "POST",
        "/marketdata/options/select",
        json!({"underlying": "SPY", "right": "CALL", "minDelta": 0.40, "maxDelta": 0.60}),
    );
    assert_eq!(selected.body["selected"]["right"], "CALL");
    assert!(selected.body["rejected"]
        .as_array()
        .expect("rejected")
        .is_empty());
    assert_eq!(selected.body["noAdvice"], true);

    let manifest = instrument_pack_manifest();
    let validation = reopened.handle_http(
        "POST",
        "/marketdata/instrument-packs/validate",
        json!({"manifest": manifest}),
    );
    assert_eq!(validation.body["validation"]["valid"], true);

    let installed = reopened.handle_http(
        "POST",
        "/marketdata/instrument-packs/install",
        json!({"manifest": instrument_pack_manifest()}),
    );
    assert_eq!(installed.body["installed"], true);
    assert_eq!(installed.body["ref"], "custom.options-task9@2026.06.0");

    let packs = TradeAssemblyService::test_local(db.to_string_lossy().to_string()).handle_http(
        "GET",
        "/marketdata/instrument-packs",
        json!({}),
    );
    assert!(packs.body["packs"]
        .as_array()
        .expect("packs")
        .iter()
        .any(|pack| pack["ref"] == "custom.options-task9@2026.06.0"));

    let all_output = json!([stored.body, status.body, tested.body, providers.body]).to_string();
    assert!(!all_output.contains("KEY_SHOULD_NOT_LEAK"));
    assert!(!all_output.contains("SECRET_SHOULD_NOT_LEAK"));
    assert!(!all_output.contains("Bearer "));
}

#[test]
fn plugin_instance_actions_accept_canonical_plugin_refs() {
    let service = test_service("plugin-instance-canonical");

    let disabled = service.handle_http(
        "POST",
        "/product/providers/disable-instance",
        json!({"pluginRef": "tradeassembly.simbroker", "disabled": true}),
    );
    assert_eq!(disabled.status, 200);
    assert_eq!(
        disabled.body["body"]["pluginRef"],
        "tradeassembly.simbroker"
    );
    assert_eq!(disabled.body["body"]["providerRef"], "sim");
    assert_eq!(disabled.body["body"]["enabled"], false);

    let disabled_workspace = service.handle_http("POST", "/product/plugins/providers", json!({}));
    assert_eq!(
        plugin_enabled(
            &disabled_workspace.body["plugins"],
            "tradeassembly.simbroker"
        ),
        Some(false)
    );

    let enabled = service.handle_http(
        "POST",
        "/product/providers/enable-instance",
        json!({"ref": "tradeassembly.simbroker", "enabled": true}),
    );
    assert_eq!(enabled.status, 200);
    assert_eq!(enabled.body["body"]["pluginRef"], "tradeassembly.simbroker");
    assert_eq!(enabled.body["body"]["providerRef"], "sim");
    assert_eq!(enabled.body["body"]["enabled"], true);
    let stored_enabled = service
        .runtime()
        .storage
        .get_json("plugin_instances_v2", "sim")
        .expect("plugin instance storage read")
        .expect("canonical plugin instance state");
    assert_eq!(stored_enabled["enabled"], true);

    let enabled_workspace = service.handle_http("POST", "/product/plugins/providers", json!({}));
    assert_eq!(
        plugin_enabled(
            &enabled_workspace.body["plugins"],
            "tradeassembly.simbroker"
        ),
        Some(true),
        "{:#}",
        enabled_workspace.body["plugins"]
    );
}

#[test]
fn entitlement_grants_require_local_owner_admin_authority() {
    let service = test_service("entitlement-grants");

    let missing_resolve_capability = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({"mode": "paper"}),
    );
    assert_eq!(missing_resolve_capability.status, 200);
    assert_eq!(missing_resolve_capability.body["ok"], false);
    assert_blocker(&missing_resolve_capability.body, "capability_required");

    let missing_capability = service.handle_http(
        "POST",
        "/entitlements/grant",
        json!({
            "source": "local_owner_admin",
            "authorityContext": {"actor": "local-owner", "surface": "local-admin"}
        }),
    );
    assert_eq!(missing_capability.status, 200);
    assert_eq!(missing_capability.body["ok"], false);
    assert_eq!(
        missing_capability.body["error"]["code"],
        "capability_required"
    );

    let public_allow_grant = service.handle_http(
        "POST",
        "/entitlements/grant",
        json!({
            "capability": "marketdata.quote",
            "source": "local_owner_admin",
            "authorityContext": {"actor": "local-owner", "surface": "local-admin"}
        }),
    );
    assert_eq!(public_allow_grant.status, 200);
    assert_eq!(public_allow_grant.body["ok"], false);
    assert_eq!(
        public_allow_grant.body["error"]["code"],
        "entitlement_grant_unavailable"
    );
}

fn assert_provider_has_capability(providers: &Value, provider_ref: &str, capability: &str) {
    let provider = providers
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["providerRef"] == provider_ref || provider["ref"] == provider_ref)
        .unwrap_or_else(|| panic!("missing provider {provider_ref}: {providers:#}"));
    assert!(
        provider["capabilities"]
            .as_array()
            .expect("capabilities")
            .iter()
            .any(|item| item == capability),
        "provider {provider_ref} missing capability {capability}: {provider:#}"
    );
}

fn plugin_enabled(plugins: &Value, plugin_ref: &str) -> Option<bool> {
    plugins
        .as_array()?
        .iter()
        .find(|plugin| plugin["pluginRef"] == plugin_ref)
        .and_then(|plugin| plugin["enabled"].as_bool())
}

#[test]
fn capability_resolver_records_local_full_entitlement_and_blocks_denied_capability() {
    let service = test_service("capability-resolver-entitlements");

    let missing_credentials = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "requirementId": "req-paper-order-after-credentials",
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "assetClass": "crypto",
            "strategyId": "strat_local_btc_demo",
            "pluginRef": "tradeassembly.alpaca-paper"
        }),
    );
    assert_eq!(missing_credentials.status, 200);
    let body = &missing_credentials.body;
    assert_eq!(
        body["schemaVersion"],
        "tradeassembly.capability_resolution.v1"
    );
    assert_eq!(body["profile"]["id"], "local_full");
    assert_eq!(body["entitlementDecisions"][0]["profile"], "local_full");
    assert_eq!(body["entitlementDecisions"][0]["decision"], "allow");
    assert_eq!(body["ok"], false);
    assert_blocker(body, "credential_missing");
    assert_blocker(body, "account_binding_missing");
    assert_eq!(body["noSilentFallback"], true);

    let stored = service.handle_http(
        "POST",
        "/providers/alpaca-paper/credentials",
        json!({
            "api_key": "KEY_SHOULD_NOT_LEAK",
            "api_secret": "SECRET_SHOULD_NOT_LEAK",
            "paper": true
        }),
    );
    assert_eq!(stored.status, 200);

    let resolved = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "requirementId": "req-paper-order-after-deny",
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "assetClass": "crypto",
            "strategyId": "strat_local_btc_demo",
            "pluginRef": "tradeassembly.alpaca-paper"
        }),
    );
    assert_eq!(resolved.status, 200);
    assert_eq!(resolved.body["ok"], false, "{:#}", resolved.body);
    assert_blocker(&resolved.body, "account_binding_missing");

    let invalid_account = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "requirementId": "req-paper-order-invalid-account-binding",
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "assetClass": "crypto",
            "strategyId": "strat_local_btc_demo",
            "pluginRef": "tradeassembly.alpaca-paper",
            "accountRef": "account://attacker/paper"
        }),
    );
    assert_eq!(invalid_account.status, 200);
    assert_eq!(invalid_account.body["ok"], false);
    assert_blocker(&invalid_account.body, "account_binding_invalid");

    let resolved = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "requirementId": "req-paper-order-after-account-binding",
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "assetClass": "crypto",
            "strategyId": "strat_local_btc_demo",
            "pluginRef": "tradeassembly.alpaca-paper",
            "accountRef": "account://alpaca-paper/paper"
        }),
    );
    assert_eq!(resolved.status, 200);
    assert_eq!(resolved.body["ok"], true, "{:#}", resolved.body);
    assert_eq!(resolved.body["state"], "candidates");
    assert_candidate(
        &resolved.body,
        "tradeassembly.alpaca-paper",
        "broker.paper_order_submit",
        "order.submit.paper",
    );
    assert_eq!(
        resolved.body["candidates"][0]["credentialStatus"]["configured"],
        true
    );
    assert_eq!(
        resolved.body["candidates"][0]["operation"]["resourceType"],
        "brokerage_account"
    );
    assert_eq!(
        resolved.body["candidates"][0]["operation"]["financeResourceType"],
        "brokerage_account"
    );
    assert_eq!(
        resolved.body["candidates"][0]["apf"]["resourceType"],
        "brokerage_account"
    );
    assert_eq!(
        resolved.body["candidates"][0]["apf"]["operationResourceType"],
        "brokerage_account"
    );
    assert_eq!(
        resolved.body["candidates"][0]["apf"]["pepCoverageClass"],
        "c3"
    );
    assert_eq!(
        resolved.body["candidates"][0]["apf"]["credentialGrantRequired"],
        true
    );

    let deny = service.handle_http(
        "POST",
        "/entitlements/deny",
        json!({
            "capability": "broker.order_submit.paper",
            "reason": "test explicit deny"
        }),
    );
    assert_eq!(deny.status, 200);

    let denied = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "requirementId": "req-paper-order",
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "assetClass": "crypto",
            "strategyId": "strat_local_btc_demo",
            "pluginRef": "tradeassembly.alpaca-paper",
            "accountRef": "account://alpaca-paper/paper"
        }),
    );
    assert_eq!(denied.status, 200);
    assert_eq!(denied.body["ok"], false);
    assert_blocker(&denied.body, "entitlement_denied");

    let serialized = json!([
        missing_credentials.body,
        stored.body,
        resolved.body,
        denied.body
    ])
    .to_string();
    assert!(!serialized.contains("KEY_SHOULD_NOT_LEAK"));
    assert!(!serialized.contains("SECRET_SHOULD_NOT_LEAK"));
    assert!(!serialized.contains("Bearer "));
}

#[test]
fn activation_readiness_and_activation_use_capability_resolution() {
    let service = test_service("activation-readiness-resolver");

    let readiness = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/activation-readiness",
        json!({
            "strategyId": "strat_local_btc_demo",
            "providerRef": "alpaca-paper",
            "mode": "paper",
            "assetClass": "crypto"
        }),
    );
    assert_eq!(readiness.status, 200);
    assert_eq!(
        readiness.body["body"]["ready"], false,
        "{:#}",
        readiness.body
    );
    assert_blocker(
        &readiness.body["body"]["capabilityResolution"],
        "credential_missing",
    );
    assert_blocker(
        &readiness.body["body"]["capabilityResolution"],
        "account_binding_required",
    );

    let blocked_config = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "providerRef": "alpaca-paper",
            "mode": "paper",
            "assetClass": "crypto"
        }),
    );
    assert_eq!(blocked_config.status, 200, "{:#}", blocked_config.body);
    let blocked_activation = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": blocked_config.body["body"]["configId"],
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "blocked-alpaca-activation"
        }),
    );
    assert_eq!(blocked_activation.status, 400);
    assert_eq!(
        blocked_activation.body["detail"],
        "activation_capability_blocked"
    );

    let stored = service.handle_http(
        "POST",
        "/providers/alpaca-paper/credentials",
        json!({
            "api_key": "KEY_SHOULD_NOT_LEAK",
            "api_secret": "SECRET_SHOULD_NOT_LEAK",
            "paper": true
        }),
    );
    assert_eq!(stored.status, 200);

    let ready_config = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "providerRef": "sim",
            "mode": "paper",
            "assetClass": "crypto",
            "capabilityBindings": {
                "req_bars": {
                    "pluginInstanceRef": "local-data",
                    "pluginRef": "tradeassembly.local-data",
                    "operationId": "marketdata.bars.read_v1"
                }
            }
        }),
    );
    assert_eq!(ready_config.status, 200, "{:#}", ready_config.body);
    let ready_config_id = ready_config.body["body"]["configId"].clone();

    let ready = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/activation-readiness",
        json!({
            "configId": ready_config_id.clone(),
            "strategyId": "strat_local_btc_demo"
        }),
    );
    assert_eq!(ready.status, 200);
    assert_eq!(ready.body["body"]["ready"], true, "{:#}", ready.body);
    let resolved_nodes = ready.body["body"]["capabilityResolution"]["nodes"]
        .as_array()
        .expect("capability graph nodes");
    let selected_operation = |requirement_id: &str| {
        resolved_nodes
            .iter()
            .find(|node| node["nodeId"] == requirement_id)
            .and_then(|node| node["selected"]["operationId"].as_str())
    };
    assert_eq!(
        selected_operation("execution.broker.submit"),
        Some("broker.paper_order_submit")
    );
    assert_eq!(
        selected_operation("req_bars"),
        Some("marketdata.bars.read_v1")
    );
    assert_eq!(
        resolved_nodes
            .iter()
            .find(|node| node["nodeId"] == "req_bars")
            .and_then(|node| node["selected"]["pluginRef"].as_str()),
        Some("tradeassembly.local-data")
    );

    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": ready_config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "resolved-sim-activation"
        }),
    );
    assert_eq!(activated.status, 200);
    assert_eq!(activated.body["body"]["status"], "active");
    let serialized = json!([
        readiness.body,
        blocked_activation.body,
        ready.body,
        activated.body
    ])
    .to_string();
    assert!(!serialized.contains("KEY_SHOULD_NOT_LEAK"));
    assert!(!serialized.contains("SECRET_SHOULD_NOT_LEAK"));
}

#[test]
fn plugin_capability_matrix_fails_closed_when_matrix_entitlement_is_denied() {
    let service = test_service("capability-matrix-denied");

    let deny = service.handle_http(
        "POST",
        "/entitlements/deny",
        json!({
            "capability": "plugin.capability_matrix",
            "reason": "matrix disabled for test"
        }),
    );
    assert_eq!(deny.status, 200);

    let matrix = service.call_mcp_tool(
        "tradeassembly.plugin.capability_matrix",
        json!({"ref": "tradeassembly.simbroker"}),
    );
    assert_eq!(matrix["isError"], false);
    let content = &matrix["structuredContent"];
    assert_eq!(content["ok"], false, "{matrix:#}");
    assert_eq!(content["compatible"], false);
    assert_eq!(content["blocked"], true);
    assert_blocker(content, "entitlement_denied");
    assert_eq!(
        content["capabilityResolution"]["requirement"]["capability"],
        "plugin.capability_matrix"
    );
}

#[test]
fn capability_resolver_fails_closed_for_unknown_or_mode_incompatible_requirements() {
    let service = test_service("resolver-failures");

    let missing = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({"capability": "marketdata.depth", "mode": "paper"}),
    );
    assert_eq!(missing.status, 200);
    assert_eq!(missing.body["ok"], false);
    assert_blocker(&missing.body, "missing_plugin");

    let live = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({"capability": "broker.order_submit.paper", "mode": "live"}),
    );
    assert_eq!(live.status, 200);
    assert_eq!(live.body["ok"], false);
    assert_blocker(&live.body, "mode_incompatible");

    let undeclared_read_mode = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({"capability": "marketdata.quote", "mode": "backtest", "pluginRef": "tradeassembly.simbroker"}),
    );
    assert_eq!(undeclared_read_mode.status, 200);
    assert_eq!(undeclared_read_mode.body["ok"], false);
    assert_blocker(&undeclared_read_mode.body, "mode_incompatible");

    let unsupported_instrument = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "instrumentType": "future",
            "pluginRef": "tradeassembly.simbroker"
        }),
    );
    assert_eq!(unsupported_instrument.status, 200);
    assert_eq!(unsupported_instrument.body["ok"], false);
    assert_blocker(&unsupported_instrument.body, "instrument_unsupported");

    let unknown_preferred_plugin = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "assetClass": "crypto",
            "pluginRef": "tradeassembly.missing"
        }),
    );
    assert_eq!(unknown_preferred_plugin.status, 200);
    assert_eq!(unknown_preferred_plugin.body["ok"], false);
    assert_blocker(&unknown_preferred_plugin.body, "missing_plugin");

    let unsupported_operation = service.handle_http(
        "POST",
        "/plugins/capabilities/resolve",
        json!({
            "capability": "broker.order_submit.paper",
            "operation": "marketdata.quote.read",
            "mode": "paper",
            "assetClass": "crypto",
            "pluginRef": "tradeassembly.simbroker"
        }),
    );
    assert_eq!(unsupported_operation.status, 200);
    assert_eq!(unsupported_operation.body["ok"], false);
    assert_blocker(&unsupported_operation.body, "operation_unsupported");

    let unknown_matrix = service.call_mcp_tool(
        "tradeassembly.plugin.capability_matrix",
        json!({"ref": "tradeassembly.missing"}),
    );
    assert_eq!(unknown_matrix["isError"], false);
    assert_eq!(unknown_matrix["structuredContent"]["ok"], false);
    assert_eq!(
        unknown_matrix["structuredContent"]["blockers"][0]["code"],
        "missing_plugin"
    );
    assert!(unknown_matrix["structuredContent"]["capabilities"]
        .as_array()
        .expect("capabilities")
        .is_empty());

    let missing_matrix_ref =
        service.call_mcp_tool("tradeassembly.plugin.capability_matrix", json!({}));
    assert_eq!(missing_matrix_ref["isError"], false);
    assert_eq!(missing_matrix_ref["structuredContent"]["ok"], false);
    assert_eq!(
        missing_matrix_ref["structuredContent"]["blockers"][0]["code"],
        "ref_required"
    );

    let missing_legacy_matrix_ref =
        service.call_mcp_tool("tradeassembly.provider.capability_matrix", json!({}));
    assert_eq!(missing_legacy_matrix_ref["isError"], false);
    assert_eq!(missing_legacy_matrix_ref["structuredContent"]["ok"], false);
    assert_eq!(
        missing_legacy_matrix_ref["structuredContent"]["blockers"][0]["code"],
        "ref_required"
    );

    let missing_pack_ref = service.call_mcp_tool(
        "tradeassembly.provider.pack_compatibility",
        json!({"manifest": instrument_pack_manifest()}),
    );
    assert_eq!(missing_pack_ref["isError"], false);
    assert_eq!(missing_pack_ref["structuredContent"]["ok"], false);
    assert_eq!(
        missing_pack_ref["structuredContent"]["blockers"][0]["code"],
        "ref_required"
    );

    let legacy_matrix_ref = service.call_mcp_tool(
        "tradeassembly.provider.capability_matrix",
        json!({"ref": "alpaca-paper"}),
    );
    assert_eq!(legacy_matrix_ref["isError"], false);
    assert_eq!(legacy_matrix_ref["structuredContent"]["ok"], true);
    assert_eq!(
        legacy_matrix_ref["structuredContent"]["pluginRef"],
        "tradeassembly.alpaca-paper"
    );
}

#[test]
fn capability_resolver_honors_graphql_mcp_instrument_aliases_and_invoke_fails_closed() {
    let service = test_service("resolver-aliases");

    let graphql = service.execute_graphql(json!({
        "operationName": "ResolveCapability",
        "query": "query ResolveCapability { pluginCapabilityResolution }",
        "variables": {
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "instrumentType": "future",
            "pluginRef": "tradeassembly.simbroker"
        }
    }));
    let graphql_resolution = &graphql["data"]["pluginCapabilityResolution"];
    assert_eq!(graphql_resolution["ok"], false, "{graphql:#}");
    assert_blocker(graphql_resolution, "instrument_unsupported");

    let mcp = service.call_mcp_tool(
        "tradeassembly.plugin.capability_resolve",
        json!({
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "instrument_type": "future",
            "plugin_ref": "tradeassembly.simbroker"
        }),
    );
    let mcp_resolution = &mcp["structuredContent"];
    assert_eq!(mcp_resolution["ok"], false, "{mcp:#}");
    assert_blocker(mcp_resolution, "instrument_unsupported");

    let invoke = service.handle_http(
        "POST",
        "/plugins/instances/sim/operations/broker.paper_order_submit:invoke",
        json!({
            "capability": "broker.order_submit.paper",
            "mode": "paper",
            "assetClass": "crypto"
        }),
    );
    assert_eq!(invoke.status, 400);
    assert_eq!(invoke.body["detail"], "plugin_idempotency_key_required");
}

fn test_service(name: &str) -> TradeAssemblyService {
    let service = TradeAssemblyService::test_local(temp_db(name).to_string_lossy().to_string());
    install_external_alpaca_fixture(&service);
    service
}

fn temp_db(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "tradeassembly-provider-marketdata-{name}-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("create isolated test directory");
    directory.join("runtime.db")
}

fn assert_blocker(body: &Value, code: &str) {
    assert!(
        body["blockers"]
            .as_array()
            .expect("blockers")
            .iter()
            .any(|blocker| blocker["code"] == code),
        "missing blocker {code}: {body:#}"
    );
}

fn install_external_alpaca_fixture(service: &TradeAssemblyService) {
    let listed = service.handle_http("GET", "/plugins/manifests", json!({}));
    assert_eq!(listed.status, 200, "{:#}", listed.body);
    let mut manifest = listed.body["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
        .expect("simbroker manifest")["manifest"]
        .clone();
    manifest["metadata"]["id"] = json!("tradeassembly.alpaca-paper");
    manifest["metadata"]["name"] = json!("External Alpaca Test Fixture");
    manifest["metadata"]["provider"] =
        json!({"id": "alpaca-paper", "name": "External Alpaca Test Fixture"});
    manifest["capabilities"]
        .as_array_mut()
        .expect("capabilities")
        .extend([
            json!({
                "id": "marketdata.crypto",
                "description": "External crypto market data fixture."
            }),
            json!({
                "id": "marketdata.options_chain",
                "description": "External option-chain fixture."
            }),
        ]);
    manifest["configuration"] = json!({
        "fields": [
            {
                "id": "api_key",
                "label": "API key",
                "description": "Fixture key.",
                "inputType": "password",
                "required": true,
                "storageClass": "credential"
            },
            {
                "id": "api_secret",
                "label": "API secret",
                "description": "Fixture secret.",
                "inputType": "password",
                "required": true,
                "storageClass": "credential"
            },
            {
                "id": "base_url",
                "label": "API URL",
                "description": "Fixture endpoint.",
                "inputType": "url",
                "required": true,
                "storageClass": "configuration",
                "default": "https://paper-api.alpaca.markets"
            },
            {
                "id": "account_mode",
                "label": "Account mode",
                "description": "Fixture mode.",
                "inputType": "select",
                "required": true,
                "storageClass": "configuration",
                "default": "paper",
                "options": ["paper"]
            }
        ]
    });
    manifest["health"] = json!({
        "requiredConfiguration": ["base_url", "account_mode"],
        "requiredCredentials": ["api_key", "api_secret"],
        "connectionCheckOperation": null
    });
    if let Some(operation) = manifest["operations"]
        .as_array_mut()
        .and_then(|operations| {
            operations
                .iter_mut()
                .find(|operation| operation["id"] == "broker.paper_order_submit")
        })
    {
        operation["credentialGrantRequired"] = json!(true);
        operation["accountBindingRequired"] = json!(true);
    }
    manifest["operations"]
        .as_array_mut()
        .expect("operations")
        .retain(|operation| operation["id"] != "broker.paper_order_preview");
    let digest = format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json_canonicalizer::to_vec(&manifest).expect("canonical fixture manifest")
        )
    );
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let installed = service.handle_http(
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": manifest,
            "source": {
                "type": "git",
                "commit": commit,
                "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#tradeassembly.alpaca-paper")
            },
            "integrity": {"manifestSha256": digest},
            "trust": {"level": "local-test"}
        }),
    );
    assert_eq!(installed.status, 200, "{:#}", installed.body);
    let created = service.handle_http(
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "alpaca-paper",
            "pluginRef": "tradeassembly.alpaca-paper",
            "providerRef": "alpaca-paper",
            "enabled": true,
            "configuration": {
                "base_url": "https://paper-api.alpaca.markets",
                "account_mode": "paper"
            }
        }),
    );
    assert_eq!(created.status, 200, "{:#}", created.body);
}

fn assert_candidate(body: &Value, plugin_ref: &str, operation_id: &str, action_id: &str) {
    let candidates = body["candidates"].as_array().expect("candidates");
    assert!(
        candidates.iter().any(|candidate| {
            candidate["pluginRef"] == plugin_ref
                && candidate["operation"]["id"] == operation_id
                && candidate["apf"]["actionId"] == action_id
        }),
        "missing candidate {plugin_ref}/{operation_id}/{action_id}: {body:#}"
    );
}

fn instrument_pack_manifest() -> Value {
    json!({
        "schemaVersion": "tradeassembly.instrument_pack_manifest.v1",
        "packId": "custom.options-task9",
        "packVersion": "2026.06.0",
        "instrumentFamilies": [
            {"assetClass": "option", "instrumentFamily": "equity_option", "executable": true},
            {"assetClass": "equity", "instrumentFamily": "listed_equity", "executable": true}
        ],
        "selectorSchemaRefs": ["selector.options.v1"],
        "lifecycleAdapterRefs": ["lifecycle.options.v1"],
        "valuationAdapterRefs": ["valuation.black_scholes.v1"],
        "marginAdapterRefs": ["margin.local_preview.v1"],
        "providerRequirements": [{"providerRef": "alpaca-paper", "capabilities": ["marketdata.options_chain"], "assetClasses": ["equity", "option"]}],
        "sourceProvenance": {"source": "local-fixture", "observedAt": "2026-06-24T00:00:00Z", "files": [{"path": "task9.json", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]},
        "fingerprint": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "compatibilityMetadata": {"serverlessSafe": true}
    })
}
