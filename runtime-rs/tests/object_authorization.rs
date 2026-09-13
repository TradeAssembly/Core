// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tradeassembly_runtime::finance_authority::TestFinanceAuthority;
use tradeassembly_runtime::ports::{
    AuthorityContext, IdempotencyKey, ObjectOwner, ObjectScope, SideEffectContext,
};
use tradeassembly_runtime::runtime_config::{RuntimeBuilder, RuntimeConfig};
use tradeassembly_runtime::service::TradeAssemblyService;

const STUDIO_TOKEN: &str = "object-authorization-studio-token-32-bytes";
const STUDIO_ISSUER: &str = "https://issuer-studio.example";
const STUDIO_AUDIENCE: &str = "tradeassembly-object-authorization";

fn service_pair() -> (
    TradeAssemblyService,
    TradeAssemblyService,
    TradeAssemblyService,
) {
    let db = tempfile::Builder::new()
        .prefix("tradeassembly-object-authorization-")
        .suffix(".db")
        .tempfile()
        .expect("temporary database")
        .into_temp_path()
        .keep()
        .expect("keep database");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    let alice = base.for_authenticated_invocation(
        "https://issuer-a.example",
        "shared-subject",
        None,
        Some("Alice".to_string()),
    );
    let bob = base.for_authenticated_invocation(
        "https://issuer-b.example",
        "shared-subject",
        None,
        Some("Bob".to_string()),
    );
    (base, alice, bob)
}

fn bind_scope(
    base: &TradeAssemblyService,
    owner: &ObjectOwner,
    object_type: &str,
    object_id: &str,
) {
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("object-scope:{object_type}:{object_id}"))
            .expect("idempotency key"),
    );
    base.runtime()
        .object_authorization
        .bind(
            &ObjectScope {
                object_type: object_type.to_string(),
                object_id: object_id.to_string(),
                owner: owner.clone(),
                parent_type: None,
                parent_id: None,
            },
            &context,
        )
        .expect("bind object scope");
}

fn create_strategy(service: &TradeAssemblyService, id: &str) -> Value {
    let response = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({
            "id": id,
            "name": "Private strategy",
            "symbol": "BTC/USD",
            "providerRef": "sim",
            "creationMode": "import",
        }),
    );
    assert_eq!(response.status, 201, "{:?}", response.body);
    assert_eq!(response.body["ok"], true, "{:?}", response.body);
    response.body
}

#[test]
fn cross_principal_strategy_reads_are_indistinguishable_from_missing() {
    let (_base, alice, bob) = service_pair();
    create_strategy(&alice, "strat_private_alice");

    let owner = alice.handle_http("GET", "/strategies/strat_private_alice", json!({}));
    assert_eq!(owner.status, 200);
    assert_eq!(
        owner.body["strategy"]["id"].as_str(),
        Some("strat_private_alice")
    );

    let foreign = bob.handle_http("GET", "/strategies/strat_private_alice", json!({}));
    let missing = bob.handle_http("GET", "/strategies/strat_missing", json!({}));
    assert_eq!(foreign.status, 404);
    assert_eq!(foreign.body, missing.body);
    let serialized = serde_json::to_string(&foreign.body).expect("serialize response");
    assert!(!serialized.contains("strat_private_alice"));
    assert!(!serialized.contains("Private strategy"));

    let cli_foreign =
        bob.handle_http_from_source("cli", "GET", "/strategies/strat_private_alice", json!({}));
    let cli_missing =
        bob.handle_http_from_source("cli", "GET", "/strategies/strat_missing", json!({}));
    assert_eq!(cli_foreign.body, cli_missing.body);
    assert_eq!(cli_foreign.body, foreign.body);
}

#[test]
fn local_seed_strategy_is_claimed_once_by_the_first_authenticated_owner() {
    let (_base, alice, bob) = service_pair();

    let owner = alice.handle_http("GET", "/strategies/strat_local_btc_demo", json!({}));
    assert_eq!(owner.status, 200);
    assert_eq!(
        owner.body["strategy"]["id"].as_str(),
        Some("strat_local_btc_demo")
    );

    let foreign = bob.handle_http("GET", "/strategies/strat_local_btc_demo", json!({}));
    let missing = bob.handle_http("GET", "/strategies/strat_missing", json!({}));
    assert_eq!(foreign.status, 404);
    assert_eq!(foreign.body, missing.body);
}

#[test]
fn cross_principal_strategy_lists_and_mutations_do_not_enumerate_or_write() {
    let (base, alice, bob) = service_pair();
    create_strategy(&alice, "strat_private_alice");

    let alice_list = alice.handle_http("GET", "/strategies", json!({}));
    assert!(alice_list
        .body
        .as_array()
        .expect("strategy list")
        .iter()
        .any(|strategy| strategy["id"] == "strat_private_alice"));

    let bob_list = bob.handle_http("GET", "/strategies", json!({}));
    assert!(!serde_json::to_string(&bob_list.body)
        .expect("serialize list")
        .contains("strat_private_alice"));

    let commands_before = base.control_plane_commands().len();
    let denied = bob.handle_http(
        "POST",
        "/product/strategies/archive",
        json!({
            "strategyId": "strat_private_alice",
            "owner": "shared-subject",
            "tenantRef": "personal:https://issuer-a.example:shared-subject",
            "actor": {"id": "shared-subject", "kind": "user"},
        }),
    );
    assert_eq!(denied.status, 404);
    assert_eq!(base.control_plane_commands().len(), commands_before);

    let owner = alice.handle_http("GET", "/strategies/strat_private_alice", json!({}));
    assert_eq!(owner.body["strategy"]["status"], "draft");
}

#[test]
fn proposal_ids_cannot_target_or_review_another_strategy_scope() {
    let (base, alice, bob) = service_pair();
    create_strategy(&alice, "strat_private_alice");
    create_strategy(&bob, "strat_private_bob");

    let created = alice.handle_http(
        "POST",
        "/product/strategies/proposals/create",
        json!({
            "strategyId": "strat_private_alice",
            "proposalId": "caller_selected_proposal",
            "specPatch": {"documentation": "Alice proposal"},
            "actor": {"kind": "agent", "id": "alice-agent"},
        }),
    );
    assert_eq!(created.status, 200, "{:?}", created.body);
    let proposal_id = created.body["body"]["proposal"]["id"]
        .as_str()
        .expect("proposal id")
        .to_string();
    assert_ne!(proposal_id, "caller_selected_proposal");

    let commands_before = base.control_plane_commands().len();
    let foreign = bob.handle_http(
        "POST",
        "/product/strategies/proposals/review",
        json!({
            "strategyId": "strat_private_bob",
            "proposalId": proposal_id,
            "decision": "reject",
            "actor": {"kind": "user", "id": "bob"},
        }),
    );
    let missing = bob.handle_http(
        "POST",
        "/product/strategies/proposals/review",
        json!({
            "strategyId": "strat_private_bob",
            "proposalId": "proposal_missing",
            "decision": "reject",
            "actor": {"kind": "user", "id": "bob"},
        }),
    );
    assert_eq!(foreign.status, 404);
    assert_eq!(foreign.body, missing.body);
    assert_eq!(base.control_plane_commands().len(), commands_before);

    let foreign_mcp = bob.call_mcp_tool(
        "tradeassembly.strategy.draft.review_patch",
        json!({
            "strategy_id": "strat_private_bob",
            "proposal_id": proposal_id,
            "decision": "reject",
        }),
    );
    let missing_mcp = bob.call_mcp_tool(
        "tradeassembly.strategy.draft.review_patch",
        json!({
            "strategy_id": "strat_private_bob",
            "proposal_id": "proposal_missing",
            "decision": "reject",
        }),
    );
    assert_eq!(foreign_mcp["error"], missing_mcp["error"]);
    assert_eq!(foreign_mcp["error"]["code"], "object_not_available");
    assert_eq!(base.control_plane_commands().len(), commands_before);
    assert_eq!(
        base.runtime()
            .storage
            .get_json("strategy_draft_proposals", &proposal_id)
            .expect("proposal read")
            .expect("proposal")["status"],
        "pending_review"
    );
}

#[test]
fn mcp_strategy_authorization_matches_missing_without_command_residue() {
    let (base, alice, bob) = service_pair();
    create_strategy(&alice, "strat_private_alice");

    let foreign = bob.call_mcp_tool(
        "tradeassembly.strategy.get",
        json!({"strategy_id": "strat_private_alice"}),
    );
    let missing = bob.call_mcp_tool(
        "tradeassembly.strategy.get",
        json!({"strategy_id": "strat_missing"}),
    );
    assert_eq!(foreign, missing);
    assert_eq!(foreign["error"]["code"], "object_not_available");

    let commands_before = base.control_plane_commands().len();
    let denied = bob.call_mcp_tool(
        "tradeassembly.strategy.save_draft",
        json!({
            "strategy_id": "strat_private_alice",
            "name": "Stolen",
        }),
    );
    assert_eq!(denied["error"]["code"], "object_not_available");
    assert_eq!(base.control_plane_commands().len(), commands_before);

    let sightline_foreign = bob.call_mcp_tool(
        "tradeassembly.sightline.create_proposal",
        json!({
            "strategy_id": "strat_private_alice",
            "patch": {"documentation": "Foreign proposal"},
        }),
    );
    let sightline_missing = bob.call_mcp_tool(
        "tradeassembly.sightline.create_proposal",
        json!({
            "strategy_id": "strat_missing",
            "patch": {"documentation": "Foreign proposal"},
        }),
    );
    assert_eq!(sightline_foreign["error"], sightline_missing["error"]);
    assert_eq!(sightline_foreign["error"]["code"], "object_not_available");
    assert_eq!(base.control_plane_commands().len(), commands_before);
}

#[test]
fn studio_graphql_strategy_authorization_matches_missing() {
    let db = tempfile::Builder::new()
        .prefix("tradeassembly-object-authorization-graphql-")
        .suffix(".db")
        .tempfile()
        .expect("temporary database")
        .into_temp_path()
        .keep()
        .expect("keep database");
    let token_path = db.with_extension("studio-token");
    std::fs::write(&token_path, STUDIO_TOKEN).expect("write Studio token");
    let db = db.to_string_lossy().to_string();
    let mut config = RuntimeConfig::local(db.clone());
    config.oidc_issuer = STUDIO_ISSUER.to_string();
    config.oidc_audience = STUDIO_AUDIENCE.to_string();
    config.oidc_client_id = STUDIO_AUDIENCE.to_string();
    config.studio_core_token_ref = Some(format!("file://{}", token_path.display()));
    let (runtime, _) = RuntimeBuilder::new(config)
        .with_finance_authority(Arc::new(TestFinanceAuthority))
        .build()
        .expect("trusted Studio runtime");
    let base = TradeAssemblyService::from_runtime(db, runtime);
    let alice =
        base.for_authenticated_invocation(STUDIO_ISSUER, "alice", None, Some("Alice".to_string()));
    let bob =
        base.for_authenticated_invocation(STUDIO_ISSUER, "bob", None, Some("Bob".to_string()));
    create_strategy(&alice, "strat_private_alice");
    create_strategy(&bob, "strat_private_bob");

    let foreign = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_strategy_detail_request("bob", "strat_private_alice"),
    );
    let missing = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_strategy_detail_request("bob", "strat_missing"),
    );
    assert_eq!(foreign.status, 200);
    assert_eq!(foreign.body, missing.body);
    assert!(!serde_json::to_string(&foreign.body)
        .expect("serialize GraphQL response")
        .contains("strat_private_alice"));

    let commands_before = base.control_plane_commands().len();
    let denied = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_strategy_request("bob", "PublishStrategy", "strat_private_alice"),
    );
    let missing = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_strategy_request("bob", "PublishStrategy", "strat_missing"),
    );
    assert_eq!(denied.body, missing.body);
    assert_eq!(
        denied.body["error"]["code"], "object_not_available",
        "{:?}",
        denied.body
    );
    assert_eq!(base.control_plane_commands().len(), commands_before);

    let proposal = alice.handle_http(
        "POST",
        "/product/strategies/proposals/create",
        json!({
            "strategyId": "strat_private_alice",
            "specPatch": {"documentation": "Alice GraphQL proposal"},
            "actor": {"kind": "agent", "id": "alice-agent"},
        }),
    );
    let proposal_id = proposal.body["body"]["proposal"]["id"]
        .as_str()
        .expect("proposal id")
        .to_string();
    let commands_before = base.control_plane_commands().len();
    let foreign_proposal = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "ReviewStrategyPatch",
            json!({
                "strategyId": "strat_private_bob",
                "proposalId": proposal_id,
                "decision": "reject",
            }),
        ),
    );
    let missing_proposal = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "ReviewStrategyPatch",
            json!({
                "strategyId": "strat_private_bob",
                "proposalId": "proposal_missing",
                "decision": "reject",
            }),
        ),
    );
    for operation in ["ReviewStrategyPatch", "ApplyStrategyPatch"] {
        let missing_proposal_id = base.handle_studio_graphql(
            Some(STUDIO_TOKEN),
            studio_request_with_variables(
                "bob",
                operation,
                json!({
                    "strategyId": "strat_private_bob",
                    "decision": "reject",
                }),
            ),
        );
        assert_eq!(missing_proposal_id.body, missing_proposal.body);
    }
    assert_eq!(foreign_proposal.body, missing_proposal.body);
    assert_eq!(
        foreign_proposal.body["error"]["code"],
        "object_not_available"
    );
    assert_eq!(base.control_plane_commands().len(), commands_before);

    let alice_owner = ObjectOwner {
        issuer: STUDIO_ISSUER.to_string(),
        subject: "alice".to_string(),
        tenant_ref: format!("personal:{STUDIO_ISSUER}:alice"),
    };
    bind_scope(
        &base,
        &alice_owner,
        "backtest_run",
        "backtest_private_alice",
    );
    let commands_before = base.control_plane_commands().len();
    let foreign_process = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "ProcessBacktest",
            json!({
                "runId": "backtest_private_alice",
                "worker": "studio-local-backtest-worker",
            }),
        ),
    );
    let missing_process = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "ProcessBacktest",
            json!({
                "runId": "backtest_missing",
                "worker": "studio-local-backtest-worker",
            }),
        ),
    );
    assert_eq!(foreign_process.body, missing_process.body);
    assert_eq!(
        foreign_process.body["error"]["code"], "object_not_available",
        "{:?}",
        foreign_process.body
    );
    assert_eq!(base.control_plane_commands().len(), commands_before);

    bind_scope(
        &base,
        &alice_owner,
        "robustness_run",
        "robustness_private_alice",
    );
    let foreign_process = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "ProcessRobustnessRun",
            json!({
                "runId": "robustness_private_alice",
                "worker": "studio-local-robustness-worker",
            }),
        ),
    );
    let missing_process = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "ProcessRobustnessRun",
            json!({
                "runId": "robustness_missing",
                "worker": "studio-local-robustness-worker",
            }),
        ),
    );
    assert_eq!(foreign_process.body, missing_process.body);
    assert_eq!(
        foreign_process.body["error"]["code"], "object_not_available",
        "{:?}",
        foreign_process.body
    );
    assert_eq!(base.control_plane_commands().len(), commands_before);
}

fn studio_strategy_detail_request(subject: &str, strategy_id: &str) -> Value {
    studio_strategy_request(subject, "StrategyDetail", strategy_id)
}

fn studio_strategy_request(subject: &str, operation: &str, strategy_id: &str) -> Value {
    let actor = format!(
        "oidc:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(
            format!("{STUDIO_ISSUER}\0{subject}").as_bytes()
        ))
    );
    json!({
        "operationName": operation,
        "query": format!("operation {operation} {{ result }}"),
        "variables": {
            "strategyId": strategy_id,
            "_studioSession": {
                "actor": actor,
                "audience": [STUDIO_AUDIENCE],
                "displayName": subject,
                "email": format!("{subject}@example.test"),
                "expiresAtMs": 1_900_000_000_000_i64,
                "issuer": STUDIO_ISSUER,
                "subject": subject,
            }
        }
    })
}

#[test]
fn mcp_backtest_denial_leaves_no_command_residue_and_owner_routes_use_bound_types() {
    let (base, alice, bob) = service_pair();
    let owner = ObjectOwner {
        issuer: "https://issuer-a.example".to_string(),
        subject: "shared-subject".to_string(),
        tenant_ref: "personal:https://issuer-a.example:shared-subject".to_string(),
    };
    for (object_type, object_id) in [
        ("dataset_ingestion", "ingestion_alice"),
        ("backtest_run", "backtest_alice"),
    ] {
        bind_scope(&base, &owner, object_type, object_id);
    }

    let owner_dataset = alice.handle_http("GET", "/dataset-ingestions/ingestion_alice", json!({}));
    assert_ne!(
        owner_dataset.body["error"]["code"], "object_not_available",
        "{:?}",
        owner_dataset.body
    );
    let owner_backtest = alice.handle_http("GET", "/backtests/backtest_alice", json!({}));
    assert_ne!(
        owner_backtest.body["error"]["code"], "object_not_available",
        "{:?}",
        owner_backtest.body
    );

    let commands_before = base.control_plane_commands().len();
    let denied = bob.call_mcp_tool(
        "tradeassembly.backtest.cancel",
        json!({"run_id": "backtest_alice", "idempotency_key": "bob-cancel"}),
    );
    let missing = bob.call_mcp_tool(
        "tradeassembly.backtest.cancel",
        json!({"run_id": "backtest_missing", "idempotency_key": "bob-missing"}),
    );
    assert_eq!(denied["error"]["code"], "object_not_available");
    assert_eq!(denied["error"], missing["error"]);
    assert_eq!(base.control_plane_commands().len(), commands_before);
}

#[test]
fn mcp_credential_denials_match_missing_without_command_residue() {
    let (base, alice, bob) = service_pair();

    let owner = alice.call_mcp_tool(
        "tradeassembly.credential.status",
        json!({"provider_ref": "sim"}),
    );
    assert_ne!(owner["error"]["code"], "object_not_available", "{owner:?}");

    for tool in [
        "tradeassembly.credential.status",
        "tradeassembly.credential.test",
    ] {
        let commands_before = base.control_plane_commands().len();
        let foreign = bob.call_mcp_tool(tool, json!({"provider_ref": "sim"}));
        let missing = bob.call_mcp_tool(tool, json!({"provider_ref": "plugin-missing"}));

        assert_eq!(foreign["error"]["code"], "object_not_available");
        assert_eq!(foreign, missing);
        assert_eq!(base.control_plane_commands().len(), commands_before);
    }
}

#[test]
fn comparison_denials_leave_no_graphql_or_mcp_command_residue() {
    let db = tempfile::Builder::new()
        .prefix("tradeassembly-object-authorization-comparison-")
        .suffix(".db")
        .tempfile()
        .expect("temporary database")
        .into_temp_path()
        .keep()
        .expect("keep database");
    let token_path = db.with_extension("studio-token");
    std::fs::write(&token_path, STUDIO_TOKEN).expect("write Studio token");
    let db = db.to_string_lossy().to_string();
    let mut config = RuntimeConfig::local(db.clone());
    config.oidc_issuer = STUDIO_ISSUER.to_string();
    config.oidc_audience = STUDIO_AUDIENCE.to_string();
    config.oidc_client_id = STUDIO_AUDIENCE.to_string();
    config.studio_core_token_ref = Some(format!("file://{}", token_path.display()));
    let (runtime, _) = RuntimeBuilder::new(config)
        .with_finance_authority(Arc::new(TestFinanceAuthority))
        .build()
        .expect("trusted Studio runtime");
    let base = TradeAssemblyService::from_runtime(db, runtime);
    let bob =
        base.for_authenticated_invocation(STUDIO_ISSUER, "bob", None, Some("Bob".to_string()));
    create_strategy(&bob, "strat_private_bob");
    let alice_owner = ObjectOwner {
        issuer: STUDIO_ISSUER.to_string(),
        subject: "alice".to_string(),
        tenant_ref: format!("personal:{STUDIO_ISSUER}:alice"),
    };
    bind_scope(
        &base,
        &alice_owner,
        "backtest_run",
        "backtest_private_alice",
    );
    bind_scope(
        &base,
        &alice_owner,
        "robustness_run",
        "robustness_private_alice",
    );

    let comparison_request = json!({
        "sourceRunIds": ["backtest_private_alice", "robustness_private_alice"],
        "scope": {
            "strategyId": "strat_private_bob",
            "allowCrossVersion": true,
            "compatibleStrategyIds": []
        },
        "idempotencyKey": "comparison-denied"
    });
    let missing_request = json!({
        "sourceRunIds": ["backtest_missing", "robustness_missing"],
        "scope": {
            "strategyId": "strat_private_bob",
            "allowCrossVersion": true,
            "compatibleStrategyIds": []
        },
        "idempotencyKey": "comparison-missing"
    });

    let commands_before = base.control_plane_commands().len();
    let denied_graphql = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "CreateResearchComparison",
            json!({"request": comparison_request.clone()}),
        ),
    );
    let missing_graphql = base.handle_studio_graphql(
        Some(STUDIO_TOKEN),
        studio_request_with_variables(
            "bob",
            "CreateResearchComparison",
            json!({"request": missing_request.clone()}),
        ),
    );
    assert_eq!(denied_graphql.body, missing_graphql.body);
    assert_eq!(
        denied_graphql.body["error"]["code"], "object_not_available",
        "{:?}",
        denied_graphql.body
    );
    assert_eq!(base.control_plane_commands().len(), commands_before);

    let denied_mcp = bob.call_mcp_tool(
        "tradeassembly.comparison.create",
        json!({"request": comparison_request}),
    );
    let missing_mcp = bob.call_mcp_tool(
        "tradeassembly.comparison.create",
        json!({"request": missing_request}),
    );
    assert_eq!(denied_mcp["error"], missing_mcp["error"]);
    assert_eq!(denied_mcp["error"]["code"], "object_not_available");
    assert_eq!(base.control_plane_commands().len(), commands_before);
}

fn studio_request_with_variables(subject: &str, operation: &str, variables: Value) -> Value {
    let actor = format!(
        "oidc:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(
            format!("{STUDIO_ISSUER}\0{subject}").as_bytes()
        ))
    );
    let mut variables = variables;
    variables["_studioSession"] = json!({
        "actor": actor,
        "audience": [STUDIO_AUDIENCE],
        "displayName": subject,
        "email": format!("{subject}@example.test"),
        "expiresAtMs": 1_900_000_000_000_i64,
        "issuer": STUDIO_ISSUER,
        "subject": subject,
    });
    json!({
        "operationName": operation,
        "query": format!("operation {operation} {{ result }}"),
        "variables": variables
    })
}
