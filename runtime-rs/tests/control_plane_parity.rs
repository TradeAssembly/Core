use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tempfile::TempDir;
use tradeassembly_runtime::adapters::local::sqlite::LocalSqliteStorage;
use tradeassembly_runtime::control_plane::{
    complete_command, http_envelope, prepare_command, ControlPlaneCommandEnvelope,
};
use tradeassembly_runtime::finance_authority::{FinanceAuthorityPort, TestFinanceAuthority};
use tradeassembly_runtime::ports::StoragePort;
use tradeassembly_runtime::service::TradeAssemblyService;

fn install_credential_fixture(service: &TradeAssemblyService) {
    let manifests = service.handle_http("GET", "/plugins/manifests", json!({}));
    let mut manifest = manifests.body["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.local-data")
        .expect("local-data manifest")["manifest"]
        .clone();
    manifest["metadata"]["id"] = json!("example.audit-credentials");
    manifest["metadata"]["name"] = json!("Audit Credential Fixture");
    manifest["metadata"]["provider"] = json!({"id": "example", "name": "Example"});
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
            }
        ]
    });
    manifest["health"] = json!({
        "requiredConfiguration": [],
        "requiredCredentials": ["api_key", "api_secret"],
        "connectionCheckOperation": null
    });
    let canonical = serde_json_canonicalizer::to_vec(&manifest).expect("canonical manifest");
    let digest = format!("sha256:{:x}", Sha256::digest(canonical));
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let installed = service.handle_http(
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": manifest,
            "source": {
                "type": "git",
                "commit": commit,
                "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#example.audit-credentials")
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
            "pluginRef": "example.audit-credentials",
            "instanceRef": "audit-credentials",
            "providerRef": "audit-credentials",
            "enabled": true
        }),
    );
    assert_eq!(created.status, 200, "{:#}", created.body);
}

#[test]
fn control_plane_records_shared_command_envelopes_across_surfaces() {
    let (_database, db) = test_db("control-plane-parity");
    let service = TradeAssemblyService::test_local(&db);

    let http = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "HTTP strategy", "idempotencyKey": "cp-http-create", "correlationId": "caller-forged"}),
    );
    assert_eq!(http.status, 201);

    let cli = service.handle_http_from_source(
        "cli",
        "POST",
        "/product/strategies/create",
        json!({"name": "CLI strategy", "idempotencyKey": "cp-cli-create", "controlPlaneCorrelationId": "caller-forged"}),
    );
    assert_eq!(cli.status, 201);

    let graphql = service.execute_graphql(json!({
        "operationName": "CreateStrategy",
        "query": "mutation CreateStrategy { createStrategy }",
        "variables": {"name": "GraphQL strategy", "idempotencyKey": "cp-graphql-create", "correlationId": "caller-forged"}
    }));
    assert!(graphql.get("errors").is_none(), "{graphql:#}");

    let mcp = service.call_mcp_tool(
        "tradeassembly.strategy.create",
        json!({"name": "MCP strategy", "idempotencyKey": "cp-mcp-create", "controlPlaneCorrelationId": "caller-forged"}),
    );
    assert_eq!(mcp["isError"], false);

    let records = service.control_plane_commands();
    assert_command(&records, "http", "strategy.create", "draft_mutation");
    assert_command(&records, "cli", "strategy.create", "draft_mutation");
    assert_command(&records, "graphql", "strategy.create", "draft_mutation");
    assert_command(&records, "mcp", "strategy.create", "mutation");
    assert!(
        records
            .iter()
            .all(|record| record["envelope"]["authority"]["actor"] == "local-user"),
        "local/personal mode must still carry authority context: {records:#?}"
    );
    let strategy_create_records = records
        .iter()
        .filter(|record| record["envelope"]["command_name"] == "strategy.create")
        .collect::<Vec<_>>();
    assert_eq!(strategy_create_records.len(), 4);
    let offenders = strategy_create_records
        .iter()
        .filter(|record| {
            record["envelope"]["correlation_id"] != record["envelope"]["command_id"]
                || record["envelope"]["correlation_id"] == "caller-forged"
        })
        .map(|record| {
            json!({
                "source": record["envelope"]["source_interface"],
                "commandId": record["envelope"]["command_id"],
                "correlationId": record["envelope"]["correlation_id"],
            })
        })
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "every surface must derive its trusted root correlation: {offenders:#?}"
    );
}

#[test]
fn control_plane_rejects_changed_payload_for_same_idempotency_key_before_mutation() {
    let (_database, db) = test_db("control-plane-conflict");
    let service = TradeAssemblyService::test_local(&db);
    let first = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "First envelope", "idempotencyKey": "cp-conflict"}),
    );
    assert_eq!(first.status, 201);

    let duplicate = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "First envelope", "idempotencyKey": "cp-conflict"}),
    );
    assert_eq!(duplicate.status, 201);

    let conflict = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "Changed envelope", "idempotencyKey": "cp-conflict"}),
    );
    assert_eq!(conflict.status, 409);
    assert_eq!(conflict.body["error"]["code"], "idempotency_conflict");

    let records = service.control_plane_commands();
    let matching = records
        .iter()
        .find(|record| {
            record["envelope"]["source_interface"] == "http"
                && record["envelope"]["command_name"] == "strategy.create"
                && record["envelope"]["idempotency_key"] == "cp-conflict"
        })
        .expect("control-plane record");
    assert_eq!(matching["duplicate"], true);

    let strategies = service.handle_http("GET", "/strategies", json!({}));
    assert!(!strategies
        .body
        .as_array()
        .expect("strategies")
        .iter()
        .any(|strategy| strategy["name"] == "Changed envelope"));
}

#[test]
fn incomplete_retry_reauthorizes_but_completed_duplicate_replays() {
    let (_database, db) = test_db("control-plane-authority-retry");
    let storage = LocalSqliteStorage::new(&db);
    let authority = CountingAuthority::default();
    let envelope = http_envelope(
        "http",
        "POST",
        "/product/strategies/create",
        &json!({"name": "Retry", "idempotencyKey": "authority-retry"}),
    )
    .expect("envelope");

    let first = prepare_command(&storage, &authority, envelope.clone()).expect("first prepare");
    assert!(!first.duplicate);
    let retry = prepare_command(&storage, &authority, envelope.clone()).expect("retry prepare");
    assert!(!retry.duplicate, "incomplete retry must re-enter authority");
    assert_eq!(authority.prepares.load(Ordering::SeqCst), 2);

    complete_command(&storage, &authority, &envelope, 201).expect("complete command");
    let duplicate = prepare_command(&storage, &authority, envelope).expect("completed duplicate");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.response_status, Some(201));
    assert_eq!(authority.prepares.load(Ordering::SeqCst), 2);
    assert_eq!(authority.completes.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct CountingAuthority {
    prepares: AtomicUsize,
    completes: AtomicUsize,
}

impl FinanceAuthorityPort for CountingAuthority {
    fn prepare_command(
        &self,
        _storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        assert_eq!(envelope.idempotency_key.as_str(), "authority-retry");
        self.prepares.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn complete_command(
        &self,
        _storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
        status: u16,
    ) -> Result<(), String> {
        assert_eq!(envelope.idempotency_key.as_str(), "authority-retry");
        assert_eq!(status, 201);
        self.completes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn audit(&self, _storage: &dyn StoragePort) -> Value {
        json!({})
    }
}

#[test]
fn demo_reset_clears_control_plane_replay_state_for_seeded_strategy() {
    let (_database, db) = test_db("demo-reset-control-plane");
    let service = TradeAssemblyService::test_local(&db);
    let spec = valid_crypto_v3_spec("BTC Full Builder Parity");
    let request = json!({
        "operationName": "SaveBuilderDraft",
        "query": "mutation SaveBuilderDraft { saveBuilderDraft }",
        "variables": {
            "strategyId": "strat_local_btc_demo",
            "name": "BTC Full Builder Parity",
            "spec": spec
        }
    });

    let first = service.execute_graphql(request.clone());
    assert!(first.get("errors").is_none(), "{first:#}");
    assert_eq!(seeded_strategy_name(&service), "BTC Full Builder Parity");
    assert_eq!(
        builder_state_spec_name(&service),
        "BTC Full Builder Parity",
        "saved draft must update the latest version spec used by Studio reload"
    );

    let reset = service.handle_http("POST", "/demo/reset-btc", json!({}));
    assert_eq!(reset.status, 200, "{:#}", reset.body);
    assert_eq!(reset.body["resetApplied"], true);
    assert_eq!(seeded_strategy_name(&service), "BTC fast exit demo");
    let commands_after_reset = service.control_plane_commands();
    assert!(
        commands_after_reset
            .iter()
            .all(|command| command["envelope"]["side_effect_class"] == "read"),
        "reset must clear mutating replay state so repeated demo mutations can reapply: {commands_after_reset:#?}"
    );

    let second = service.execute_graphql(request);
    assert!(second.get("errors").is_none(), "{second:#}");
    assert_eq!(seeded_strategy_name(&service), "BTC Full Builder Parity");
    assert_eq!(
        builder_state_spec_name(&service),
        "BTC Full Builder Parity",
        "save after reset must not replay stale control-plane state"
    );
}

#[test]
fn post_backed_view_loaders_reexecute_after_strategy_mutation() {
    let (_database, db) = test_db("post-view-loader-reexecute");
    let service = TradeAssemblyService::test_local(&db);
    let first_read = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(first_read.status, 200);
    assert_eq!(
        first_read.body["editableSpec"]["name"],
        "BTC fast exit demo"
    );

    let save = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({
            "strategyId": "strat_local_btc_demo",
            "name": "BTC Full Builder Parity",
            "spec": valid_crypto_v3_spec("BTC Full Builder Parity")
        }),
    );
    assert_eq!(save.status, 200, "{:#}", save.body);

    let second_read = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(second_read.status, 200);
    assert_eq!(
        second_read.body["editableSpec"]["name"], "BTC Full Builder Parity",
        "POST-backed view loaders must not replay stale read bodies after state changes"
    );

    let records = service.control_plane_commands();
    let loader = records
        .iter()
        .find(|record| {
            record["envelope"]["command_name"] == "product.strategies.builder_state.post"
        })
        .expect("builder-state command record");
    assert_eq!(loader["envelope"]["side_effect_class"], "read");
    assert_eq!(loader["duplicate"], false);

    let plugin_workspace = service.handle_http("POST", "/product/plugins/providers", json!({}));
    assert_eq!(plugin_workspace.status, 200, "{:#}", plugin_workspace.body);

    let plugin_update = service.handle_http(
        "POST",
        "/product/providers/disable-instance",
        json!({"pluginRef": "tradeassembly.simbroker", "disabled": true}),
    );
    assert_eq!(plugin_update.status, 200, "{:#}", plugin_update.body);

    let plugin_workspace_after_update =
        service.handle_http("POST", "/product/plugins/providers", json!({}));
    assert_eq!(plugin_workspace_after_update.status, 200);
    let plugin_loader = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["command_name"] == "product.plugins.providers.post")
        .expect("plugin provider workspace command record");
    assert_eq!(plugin_loader["envelope"]["side_effect_class"], "read");
    assert_eq!(
        plugin_loader["duplicate"], false,
        "plugin provider workspace POST loader must re-execute after instance state changes"
    );
}

#[test]
fn graphql_strategy_builder_returns_the_canonical_refresh_state() {
    let (_database, db) = test_db("graphql-builder-state-parity");
    let service = TradeAssemblyService::test_local(&db);

    let query = || {
        service.execute_graphql(json!({
            "operationName": "StrategyBuilder",
            "query": "query StrategyBuilder { strategyBuilder }",
            "variables": {"strategyId": "strat_local_btc_demo"}
        }))
    };
    let initial = query();
    let initial_state = &initial["data"]["strategyBuilder"];
    assert!(initial_state["draft"]["draftHash"].as_str().is_some());
    assert_eq!(initial_state["editableSpec"]["name"], "BTC fast exit demo");
    assert!(initial_state["validation"]["ok"].is_boolean());
    assert!(initial_state["capabilityResolutions"].is_array());
    assert_eq!(
        initial_state["capabilityGraphResolution"]["schemaVersion"],
        "tradeassembly.capability_graph.v1"
    );
    assert!(initial_state["capabilityGraphResolution"]["nodes"].is_array());

    let save = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({
            "strategyId": "strat_local_btc_demo",
            "name": "GraphQL refresh parity",
            "spec": valid_crypto_v3_spec("GraphQL refresh parity")
        }),
    );
    assert_eq!(save.status, 200, "{:#}", save.body);

    let refreshed = query();
    let refreshed_state = &refreshed["data"]["strategyBuilder"];
    assert_eq!(
        refreshed_state["editableSpec"]["name"],
        "GraphQL refresh parity"
    );
    assert_eq!(
        refreshed_state["draft"]["draftHash"], save.body["body"]["draft"]["draftHash"],
        "GraphQL refresh must return the persisted draft identity"
    );
    assert_eq!(
        refreshed_state["capabilityGraphResolution"]["schemaVersion"],
        "tradeassembly.capability_graph.v1"
    );
}

fn seeded_strategy_name(service: &TradeAssemblyService) -> String {
    service
        .handle_http("GET", "/strategies", json!({}))
        .body
        .as_array()
        .expect("strategies")
        .iter()
        .find(|strategy| strategy["id"] == "strat_local_btc_demo")
        .and_then(|strategy| strategy["name"].as_str())
        .expect("seeded strategy name")
        .to_string()
}

fn builder_state_spec_name(service: &TradeAssemblyService) -> String {
    service
        .handle_http(
            "POST",
            "/product/strategies/builder-state",
            json!({"strategyId": "strat_local_btc_demo"}),
        )
        .body["editableSpec"]["name"]
        .as_str()
        .expect("builder editable spec name")
        .to_string()
}

fn valid_crypto_v3_spec(name: &str) -> Value {
    let mut spec: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/strategy-spec/v3/valid/crypto-spot-24x7.json"
    )))
    .expect("valid checked-in crypto V3 strategy fixture");
    spec["strategy_id"] = json!("strat_local_btc_demo");
    spec["name"] = json!(name);
    spec
}

fn test_db(label: &str) -> (TempDir, String) {
    let directory = tempfile::Builder::new()
        .prefix(&format!("tradeassembly-control-plane-{label}-"))
        .tempdir()
        .expect("temporary control-plane database directory");
    let path = directory.path().join("runtime.db");
    (directory, path.to_string_lossy().into_owned())
}

#[test]
fn control_plane_preview_redacts_credentials_and_exposes_local_admin_audit_route() {
    let (_database, db) = test_db("control-plane-redaction");
    let service = TradeAssemblyService::test_local(&db);
    install_credential_fixture(&service);
    let stored = service.handle_http(
        "POST",
        "/plugins/instances/audit-credentials/credentials",
        json!({
            "credentials": {
                "api_key": "KEY_VISIBLE_SUFFIX",
                "api_secret": "SHOULD_NOT_LEAK"
            },
            "idempotencyKey": "cp-credential"
        }),
    );
    assert_eq!(stored.status, 200);

    let audit = service.handle_http("GET", "/admin/control-plane", json!({}));
    assert_eq!(audit.status, 200);
    assert!(!audit.body.to_string().contains("SHOULD_NOT_LEAK"));
    assert_command(
        audit.body["commands"].as_array().expect("commands"),
        "http",
        "credential.store",
        "credential",
    );
}

#[test]
fn finance_authority_records_valid_paper_order_contracts_and_is_idempotent() {
    let (_database, db) = test_db("finance-authority-paper");
    let service = TradeAssemblyService::test_local(&db);
    let request = json!({
        "activationId": "activation-apf-paper",
        "strategyId": "strat_local_btc_demo",
        "strategyVersionId": "ver_local",
        "symbol": "BTC/USD",
        "submitOrders": true,
        "accountMode": "paper",
        "idempotencyKey": "finance-paper-submit",
        "sightlineRefs": ["room-local"],
        "authorityContext": {"actor": "local-user", "surface": "http", "accountMode": "paper"}
    });

    let first = service.handle_http("POST", "/product/run-center/run-once", request.clone());
    assert_eq!(first.status, 200, "{:#}", first.body);
    let duplicate = service.handle_http("POST", "/product/run-center/run-once", request);
    assert_eq!(duplicate.status, 200, "{:#}", duplicate.body);
    assert_eq!(duplicate.body["duplicate"], true);

    let orders = service.handle_http("GET", "/orders", json!({}));
    let order_count = orders
        .body
        .as_array()
        .expect("orders")
        .iter()
        .filter(|order| order["client_order_id"] == "finance-paper-submit")
        .count();
    assert_eq!(
        order_count, 1,
        "duplicate command must not dispatch a second order: {orders:#?}"
    );

    let audit = finance_audit(&service);
    assert_eq!(
        count_actions(&audit, "order.submit.paper"),
        1,
        "duplicate idempotency key must update, not duplicate, finance receipts: {audit:#}"
    );
    let descriptor_json = find_descriptor(&audit, "order.submit.paper");
    assert_eq!(descriptor_json["resource"]["kind"], "brokerage_account");
    assert_eq!(descriptor_json["source_interface"]["pep_coverage"], "c3");
    assert_eq!(descriptor_json["source_interface"]["can_block"], true);
    assert_eq!(
        descriptor_json["sightline_refs"][0]["kind"],
        "sightline_context_digest"
    );
    assert_ne!(descriptor_json["sightline_refs"][0]["id"], "room-local");
    assert_eq!(
        audit["receipts"][0]["schema_version"],
        "warden.signed_receipt.v2"
    );
    assert_eq!(audit["receipts"][0]["signature"]["algorithm"], "ed25519");
}

#[test]
fn finance_authority_blocks_live_order_submission_before_dispatch() {
    let (_database, db) = test_db("finance-authority-live-deny");
    let service = TradeAssemblyService::test_local(&db);
    let denied = service.handle_http(
        "POST",
        "/product/run-center/run-once",
        json!({
            "activationId": "activation-apf-live",
            "strategyId": "strat_local_btc_demo",
            "strategyVersionId": "ver_live",
            "symbol": "BTC/USD",
            "submitOrders": true,
            "accountMode": "live",
            "idempotencyKey": "finance-live-submit",
            "authorityContext": {"actor": "local-user", "surface": "http", "accountMode": "live"}
        }),
    );
    assert_eq!(denied.status, 403);
    assert_eq!(denied.body["error"]["code"], "finance_authority_denied");

    let audit = finance_audit(&service);
    let descriptor = find_descriptor(&audit, "order.submit.live");
    assert_eq!(descriptor["purpose"], "live_order_submission");
    assert_eq!(descriptor["source_interface"]["pep_coverage"], "c3");
    assert_eq!(audit["receipts"][0]["receipt"]["decision"], "deny");

    let commands = service.control_plane_commands();
    let command = commands
        .iter()
        .find(|record| record["envelope"]["idempotency_key"] == "finance-live-submit")
        .expect("live command record");
    assert_eq!(command["response_status"], 403);
}

#[test]
fn finance_authority_audit_never_exposes_raw_credential_material() {
    let (_database, db) = test_db("finance-authority-credential");
    let service = TradeAssemblyService::test_local(&db);
    install_credential_fixture(&service);
    let stored = service.handle_http(
        "POST",
        "/plugins/instances/audit-credentials/credentials",
        json!({
            "credentials": {
                "api_key": "KEY_VISIBLE_SUFFIX",
                "api_secret": "SHOULD_NOT_LEAK"
            },
            "idempotencyKey": "finance-credential-store"
        }),
    );
    assert_eq!(stored.status, 200);

    let audit = finance_audit(&service);
    assert!(!audit.to_string().contains("SHOULD_NOT_LEAK"));
    let descriptor = find_descriptor(&audit, "credential.configure");
    assert_eq!(descriptor["side_effect_class"], "credential");
    assert_eq!(
        audit["receipts"][0]["receipt"]["action"],
        "credential.configure"
    );
}

#[test]
fn finance_authority_completes_mcp_guard_denials_with_receipts() {
    let (_database, db) = test_db("finance-authority-mcp-deny");
    let service = TradeAssemblyService::test_local(&db);
    let response = service.call_mcp_tool(
        "tradeassembly.execution.run",
        json!({
            "activation_id": "activation-apf-mcp",
            "strategy_id": "strat_local_btc_demo",
            "strategy_version_id": "ver_local",
            "submit_orders": true,
            "account_mode": "paper",
            "idempotency_key": "finance-mcp-denied"
        }),
    );
    assert_eq!(response["isError"], true);
    assert!(response["content"][0]["text"]
        .as_str()
        .expect("mcp error text")
        .contains("mcp_order_submission_disabled"));

    let command = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["idempotency_key"] == "finance-mcp-denied")
        .expect("mcp command record");
    assert_eq!(command["response_status"], 403);
    let audit = finance_audit(&service);
    assert_eq!(audit["receipts"][0]["receipt"]["decision"], "allow");
    assert_eq!(audit["outcomes"][0]["outcome"], "failed");
    assert_eq!(
        find_descriptor(&audit, "order.submit.paper")["source_interface"]["interface_kind"],
        "mcp"
    );
}

#[test]
fn finance_authority_rejects_unknown_graphql_mutations_before_dispatch() {
    let (_database, db) = test_db("finance-authority-graphql-deny");
    let service = TradeAssemblyService::test_local(&db);
    let response = service.execute_graphql(json!({
        "operationName": "UnknownMutation",
        "query": "mutation UnknownMutation { unknownMutation }",
        "variables": {"idempotencyKey": "finance-graphql-unknown"}
    }));
    assert!(response.get("errors").is_some(), "{response:#}");

    let command = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["idempotency_key"] == "finance-graphql-unknown")
        .expect("graphql command record");
    assert_eq!(command["response_status"], 403);
    let audit = finance_audit(&service);
    assert!(audit["receipts"].as_array().expect("receipts").is_empty());
}

#[test]
fn finance_authority_blocks_graphql_live_order_submission_before_dispatch() {
    let (_database, db) = test_db("finance-authority-graphql-live-deny");
    let service = TradeAssemblyService::test_local(&db);
    let response = service.execute_graphql(json!({
        "operationName": "RunStrategyOnce",
        "query": "mutation RunStrategyOnce { runStrategyOnce }",
        "variables": {
            "activationId": "activation-apf-graphql-live",
            "strategyId": "strat_local_btc_demo",
            "strategyVersionId": "ver_live",
            "symbol": "BTC/USD",
            "submitOrders": true,
            "accountMode": "live",
            "idempotencyKey": "finance-graphql-live-submit",
            "authorityContext": {"actor": "local-user", "surface": "graphql", "accountMode": "live"}
        }
    }));
    assert_eq!(
        response["errors"][0]["message"], "finance_authority_denied",
        "{response:#}"
    );

    let command = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["idempotency_key"] == "finance-graphql-live-submit")
        .expect("graphql live command record");
    assert_eq!(command["response_status"], 403);

    let audit = finance_audit(&service);
    let descriptor = find_descriptor(&audit, "order.submit.live");
    assert_eq!(descriptor["source_interface"]["interface_kind"], "graphql");
    assert_eq!(audit["receipts"][0]["receipt"]["decision"], "deny");

    let orders = service.handle_http("GET", "/orders", json!({}));
    assert!(!orders
        .body
        .as_array()
        .expect("orders")
        .iter()
        .any(|order| order["client_order_id"] == "finance-graphql-live-submit"));
}

#[test]
fn control_plane_hashes_raw_payload_for_credential_idempotency_without_leaking_it() {
    let (_database, db) = test_db("finance-authority-credential-conflict");
    let service = TradeAssemblyService::test_local(&db);
    install_credential_fixture(&service);
    let first = service.handle_http(
        "POST",
        "/plugins/instances/audit-credentials/credentials",
        json!({
            "credentials": {
                "api_key": "KEY_VISIBLE_SUFFIX",
                "api_secret": "FIRST_SECRET_DO_NOT_LEAK"
            },
            "idempotencyKey": "finance-credential-conflict"
        }),
    );
    assert_eq!(first.status, 200);
    let conflict = service.handle_http(
        "POST",
        "/plugins/instances/audit-credentials/credentials",
        json!({
            "credentials": {
                "api_key": "KEY_VISIBLE_SUFFIX",
                "api_secret": "SECOND_SECRET_DO_NOT_LEAK"
            },
            "idempotencyKey": "finance-credential-conflict"
        }),
    );
    assert_eq!(conflict.status, 409);
    let audit = service.handle_http("GET", "/admin/control-plane", json!({}));
    let serialized = audit.body.to_string();
    assert!(!serialized.contains("FIRST_SECRET_DO_NOT_LEAK"));
    assert!(!serialized.contains("SECOND_SECRET_DO_NOT_LEAK"));
}

#[test]
fn finance_authority_rejects_unverifiable_expected_sequence_with_receipt() {
    let (_database, db) = test_db("finance-authority-stale-sequence");
    let service = TradeAssemblyService::test_local(&db);
    let response = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({
            "strategyId": "strat_local_btc_demo",
            "expectedSequence": 42,
            "currentSequence": 42,
            "idempotencyKey": "finance-stale-sequence"
        }),
    );
    assert_eq!(response.status, 409);
    assert_eq!(response.body["error"]["code"], "stale_sequence");

    let command = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["idempotency_key"] == "finance-stale-sequence")
        .expect("stale command record");
    assert_eq!(command["response_status"], 409);
    let audit = finance_audit(&service);
    assert_eq!(audit["receipts"][0]["receipt"]["decision"], "allow");
    assert_eq!(audit["outcomes"][0]["status"], 409);
}

#[test]
fn graphql_and_mcp_fail_closed_when_finance_receipt_completion_fails() {
    let (_database, db) = test_db("finance-authority-complete-failure");
    let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.clone());
    runtime.finance_authority = Arc::new(FailingCompleteAuthority);
    let service = TradeAssemblyService::from_runtime(db, runtime);

    let graphql = service.execute_graphql(json!({
        "operationName": "CreateStrategy",
        "query": "mutation CreateStrategy { createStrategy }",
        "variables": {
            "name": "GraphQL completion failure",
            "idempotencyKey": "finance-complete-fail-graphql"
        }
    }));
    assert_eq!(
        graphql["errors"][0]["message"],
        "control_plane_persistence_failed"
    );
    let graphql_record = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["idempotency_key"] == "finance-complete-fail-graphql")
        .expect("graphql completion failure record");
    assert!(graphql_record["response_status"].is_null());
    assert!(graphql_record["response_body"].is_null());

    let mcp = service.call_mcp_tool(
        "tradeassembly.strategy.create",
        json!({
            "name": "MCP completion failure",
            "idempotencyKey": "finance-complete-fail-mcp"
        }),
    );
    assert_eq!(mcp["isError"], true);
    assert_eq!(
        mcp["structuredContent"]["error"]["code"],
        "control_plane_persistence_failed"
    );
    let mcp_record = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["idempotency_key"] == "finance-complete-fail-mcp")
        .expect("mcp completion failure record");
    assert!(mcp_record["response_status"].is_null());
    assert!(mcp_record["response_body"].is_null());
}

struct FailingCompleteAuthority;

impl FinanceAuthorityPort for FailingCompleteAuthority {
    fn prepare_command(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        TestFinanceAuthority.prepare_command(storage, envelope)
    }

    fn complete_command(
        &self,
        _storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
        _status: u16,
    ) -> Result<(), String> {
        if envelope.side_effect_class == "read" {
            Ok(())
        } else {
            Err("injected_completion_failure".to_string())
        }
    }

    fn audit(&self, storage: &dyn StoragePort) -> Value {
        TestFinanceAuthority.audit(storage)
    }
}

fn assert_command(records: &[Value], source: &str, command_name: &str, side_effect_class: &str) {
    assert!(
        records.iter().any(|record| {
            record["envelope"]["source_interface"] == source
                && record["envelope"]["command_name"] == command_name
                && record["envelope"]["side_effect_class"] == side_effect_class
                && record["envelope"]["schema_version"] == "tradeassembly.control_plane.command.v1"
                && record["response_status"].as_u64().is_some()
        }),
        "missing {source}/{command_name}/{side_effect_class}: {records:#?}"
    );
}

fn finance_audit(service: &TradeAssemblyService) -> Value {
    let audit = service.handle_http("GET", "/admin/finance-authority", json!({}));
    assert_eq!(audit.status, 200);
    audit.body
}

fn find_descriptor(audit: &Value, action_id: &str) -> Value {
    audit["descriptors"]
        .as_array()
        .expect("descriptors")
        .iter()
        .find(|descriptor| descriptor["action_id"] == action_id)
        .cloned()
        .unwrap_or_else(|| panic!("missing descriptor {action_id}: {audit:#}"))
}

fn count_actions(audit: &Value, action_id: &str) -> usize {
    audit["descriptors"]
        .as_array()
        .expect("descriptors")
        .iter()
        .filter(|descriptor| descriptor["action_id"] == action_id)
        .count()
}
