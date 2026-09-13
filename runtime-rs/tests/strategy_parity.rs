use serde_json::{json, Value};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn authenticated_mcp_cannot_publish_by_claiming_user_actor() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("authority.db");
    let service = TradeAssemblyService::test_local(database.to_string_lossy())
        .for_authenticated_invocation("local-owner", "user.local", None, None);
    // The authenticated fixture claims its built-in strategy through the
    // normal local-owner initialization path, without database mutation.
    let id = "strat_local_btc_demo";
    let before = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": id}),
    );
    let hash = before.body["draft"]["draftHash"].as_str().unwrap();
    let namespaces = [
        "strategies",
        "strategy_drafts",
        "strategy_versions",
        "strategy_draft_proposals",
        "strategy_semantic_selections",
    ];
    let state_before =
        namespaces.map(|namespace| service.runtime().storage.list_json(namespace).unwrap());
    let journal_before = service.runtime().journal.try_events().unwrap();
    for kind in ["agent", "user"] {
        let result = service.call_mcp_tool(
            "tradeassembly.strategy.version.publish",
            json!({
                "strategy_id": id,
                "commandId": format!("mcp-publish-denied-{kind}"),
                "expected_draft_hash": hash,
                "actor": {"kind": kind, "id": "caller-controlled"}
            }),
        );
        assert_eq!(
            result["structuredContent"]["error"]["code"], "agent_cannot_publish_strategy_version",
            "MCP publication must reject before processing draft state"
        );
        let action = &result["structuredContent"]["details"]["nextAction"];
        assert_eq!(action["requiresOwnerAcknowledgement"], true);
        assert_eq!(action["reuseCurrentInstallation"], true);
        assert_eq!(action["commandArguments"][2], id);
        assert_eq!(action["commandArguments"][4], hash);
    }
    // Exercise a real evaluation-epoch boundary rather than relying on two reads
    // finishing inside the same second. This metadata is not persisted strategy state.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let after = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": id}),
    );
    let state_after =
        namespaces.map(|namespace| service.runtime().storage.list_json(namespace).unwrap());
    assert_eq!(
        state_before, state_after,
        "MCP denial changed persisted strategy state"
    );
    assert_eq!(
        journal_before,
        service.runtime().journal.try_events().unwrap()
    );
    let mut before_view = before.body.clone();
    let mut after_view = after.body.clone();
    for view in [&mut before_view, &mut after_view] {
        let graph = view["capabilityGraphResolution"].as_object_mut().unwrap();
        // graphFingerprint includes evaluationEpoch. Compare all actual graph
        // inputs/results and all builder fields, excluding only those derived fields.
        assert!(graph.remove("evaluationEpoch").unwrap().is_string());
        assert!(graph.remove("graphFingerprint").unwrap().is_string());
    }
    assert!(
        before_view == after_view,
        "MCP denial changed builder state beyond read-time graph metadata"
    );
    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({"strategyId": id, "expectedDraftHash": hash}),
    );
    assert_eq!(published.body["body"]["published"], true);
    assert_eq!(published.body["body"]["activation"]["started"], false);
}

#[test]
fn strategy_workspace_versions_and_builder_state_round_trip_through_storage() {
    let db = format!(
        ".tradeassembly/test-strategy-parity-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let mut source_spec = v3_fixture();
    source_spec["name"] = json!("Stored Strategy");

    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "Stored Strategy", "spec": source_spec, "symbol": "BTC/USD"}),
    );
    assert_eq!(created.status, 201);
    assert_eq!(created.body["ok"], true);
    let strategy_id = created.body["body"]["strategy"]["id"]
        .as_str()
        .expect("created strategy id")
        .to_string();
    assert_ne!(strategy_id, "strat_local_created");
    let spec_v1 = created.body["body"]["strategy"]["latestSpec"].clone();
    assert_eq!(spec_v1["strategy_id"], strategy_id);

    let list = service.handle_http("GET", "/strategies", json!({}));
    assert_strategy_named(&list.body, &strategy_id, "Stored Strategy");

    let detail = service.handle_http("GET", &format!("/strategies/{strategy_id}"), json!({}));
    assert_eq!(detail.body["strategy"]["id"], strategy_id);
    assert_eq!(detail.body["strategy"]["name"], "Stored Strategy");
    assert_eq!(detail.body["strategy"]["version"], 0);
    assert!(detail.body["strategy"]["currentVersionId"].is_null());

    let mut spec_v2 = spec_v1.clone();
    spec_v2["name"] = json!("Stored Strategy Updated");
    spec_v2["signal_market_requirements"][0]["selectors"][0]["normalized_id"] = json!("QQQ");
    let saved = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({
            "strategyId": strategy_id,
            "name": "Stored Strategy Updated",
            "spec": spec_v2,
            "capabilityBindings": {
                "req_bars": {
                    "pluginInstanceRef": "local-data",
                    "pluginRef": "tradeassembly.local-data",
                    "operationId": "marketdata.bars.read_v1"
                }
            }
        }),
    );
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["ok"], true);
    assert_eq!(saved.body["body"]["draftSaved"], true);
    assert_eq!(saved.body["body"]["validation"]["ok"], true);

    let updated = service.handle_http("GET", &format!("/strategies/{strategy_id}"), json!({}));
    assert_eq!(updated.body["strategy"]["name"], "Stored Strategy Updated");
    assert_eq!(updated.body["strategy"]["version"], 0);
    assert_eq!(updated.body["strategy"]["draftRevision"], 2);
    assert_eq!(
        updated.body["strategy"]["latestSpec"]["signal_market_requirements"][0]["selectors"][0]
            ["normalized_id"],
        "SPY"
    );

    let draft_builder = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": strategy_id}),
    );
    assert_eq!(
        draft_builder.body["editableSpec"]["signal_market_requirements"][0]["selectors"][0]
            ["normalized_id"],
        "QQQ"
    );
    assert_eq!(
        draft_builder.body["capabilityBindings"]["req_bars"]["pluginInstanceRef"],
        "local-data"
    );
    let bars_node = draft_builder.body["capabilityGraphResolution"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["nodeId"] == "req_bars")
        .expect("bars capability node");
    assert_eq!(bars_node["selected"]["pluginInstanceRef"], "local-data");

    let invalid_save = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({
            "strategyId": strategy_id,
            "spec": {
                "schemaVersion": "tradeassembly.strategy.v1",
                "name": "Invalid runtime-bound strategy",
                "provider_ref": "sim",
                "credential_handle": "secret-handle"
            }
        }),
    );
    assert_eq!(invalid_save.body["ok"], false);
    assert_eq!(invalid_save.body["error"]["code"], "strategy_draft_invalid");

    let draft_history = service.handle_http(
        "POST",
        "/product/strategies/version-history",
        json!({"strategyId": strategy_id}),
    );
    assert_eq!(
        draft_history.body["versions"]
            .as_array()
            .expect("versions")
            .len(),
        0,
        "saving a draft must not create immutable StrategyVersion rows"
    );

    let missing_hash = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "commandId": "cmd_publish_without_draft_hash",
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(missing_hash.body["ok"], false);
    assert_eq!(
        missing_hash.body["error"]["code"],
        "strategy_draft_hash_required"
    );

    let missing_graphql_hash = service.execute_graphql(json!({
        "operationName": "PublishStrategy",
        "query": "mutation PublishStrategy { publishStrategy }",
        "variables": {
            "strategyId": strategy_id,
            "commandId": "cmd_graphql_publish_without_draft_hash",
            "actor": {"kind": "user", "id": "user.local"}
        }
    }));
    assert_eq!(
        missing_graphql_hash["errors"][0]["message"],
        "strategy_draft_hash_required"
    );

    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"},
            "authorityContext": {
                "actor": "user.publisher",
                "surface": "studio",
                "accountMode": "paper"
            }
        }),
    );
    assert_eq!(published.status, 200);
    assert_eq!(published.body["body"]["published"], true);
    assert_eq!(published.body["body"]["activation"]["started"], false);
    assert_eq!(published.body["body"]["version"]["version"], 1);
    let publication_events = service
        .runtime()
        .journal
        .events()
        .into_iter()
        .filter(|event| event.event_type == "strategy.version_published")
        .collect::<Vec<_>>();
    assert_eq!(publication_events.len(), 2);
    assert!(publication_events.iter().all(|event| {
        event.authority.actor == "user.publisher"
            && event.authority.surface == "studio"
            && event.authority.account_mode == "paper"
    }));

    let replayed = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"},
            "authorityContext": {
                "actor": "user.publisher",
                "surface": "studio",
                "accountMode": "paper"
            }
        }),
    );
    assert_eq!(replayed.status, 200);
    assert_eq!(replayed.body["body"]["published"], true);
    assert_eq!(replayed.body["duplicate"], true, "{:#?}", replayed.body);
    assert_eq!(
        replayed.body["body"]["version"]["id"],
        published.body["body"]["version"]["id"]
    );

    let retried_with_new_envelope = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "commandId": "cmd_publish_retry_with_new_envelope",
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"},
            "authorityContext": {
                "actor": "user.publisher",
                "surface": "studio",
                "accountMode": "paper"
            }
        }),
    );
    assert_eq!(retried_with_new_envelope.status, 200);
    assert_eq!(retried_with_new_envelope.body["body"]["replayed"], true);
    assert_eq!(
        retried_with_new_envelope.body["body"]["version"]["id"],
        published.body["body"]["version"]["id"]
    );

    let history = service.handle_http(
        "POST",
        "/product/strategies/version-history",
        json!({"strategyId": strategy_id}),
    );
    assert_eq!(
        history.body["versions"].as_array().expect("versions").len(),
        1
    );
    assert_eq!(history.body["versions"][0]["version"], 1);
    assert_eq!(history.body["versions"][0]["immutable"], true);

    let mut changed_spec = spec_v2.clone();
    changed_spec["name"] = json!("Stored Strategy Republished");
    changed_spec["signal_market_requirements"][0]["selectors"][0]["normalized_id"] = json!("IWM");
    let changed_draft = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({
            "strategyId": strategy_id,
            "name": "Stored Strategy Republished",
            "spec": changed_spec
        }),
    );
    assert_eq!(changed_draft.body["ok"], true);

    let republished = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": changed_draft.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(republished.body["body"]["published"], true);
    assert_eq!(republished.body["body"]["version"]["version"], 2);
    assert_ne!(
        republished.body["body"]["version"]["id"],
        published.body["body"]["version"]["id"]
    );

    let stale_publish = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "commandId": "cmd_publish_stale_draft",
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(stale_publish.body["ok"], false);
    assert_eq!(
        stale_publish.body["error"]["code"],
        "strategy_draft_changed"
    );

    let stale_mcp_publish = service.call_mcp_tool(
        "tradeassembly.strategy.version.publish",
        json!({
            "strategy_id": strategy_id,
            "expected_draft_hash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(stale_mcp_publish["structuredContent"]["ok"], false);
    assert_eq!(
        stale_mcp_publish["structuredContent"]["error"]["code"],
        "agent_cannot_publish_strategy_version"
    );
    let current_draft_hash = changed_draft.body["body"]["draft"]["draftHash"]
        .as_str()
        .expect("current draft hash");

    for malformed in [
        json!({
            "strategy_id": strategy_id,
            "actor": {"kind": "user", "id": "user.local"}
        }),
        json!({
            "strategy_id": strategy_id,
            "expected_draft_hash": current_draft_hash,
        }),
        json!({
            "strategy_id": strategy_id,
            "expected_draft_hash": current_draft_hash,
            "actor": {"kind": "service", "id": "service.local"}
        }),
    ] {
        let rejected = service.call_mcp_tool("tradeassembly.strategy.version.publish", malformed);
        assert_eq!(rejected["structuredContent"]["ok"], false);
        assert_eq!(
            rejected["structuredContent"]["error"]["code"],
            "invalid_tool_arguments"
        );
    }

    let agent_publish = service.call_mcp_tool(
        "tradeassembly.strategy.version.publish",
        json!({
            "strategy_id": strategy_id,
            "expected_draft_hash": current_draft_hash,
            "actor": {"kind": "agent", "id": "agent.local"}
        }),
    );
    assert_eq!(
        agent_publish["structuredContent"]["ok"], false,
        "{agent_publish:#}"
    );
    assert_eq!(
        agent_publish["structuredContent"]["error"]["code"],
        "agent_cannot_publish_strategy_version"
    );

    let builder = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": strategy_id}),
    );
    assert_eq!(builder.body["strategy"]["version"], 2);
    assert_eq!(
        builder.body["editableSpec"]["signal_market_requirements"][0]["selectors"][0]
            ["normalized_id"],
        "IWM"
    );
    assert_eq!(builder.body["currentVersion"]["version"], 2);
    assert_eq!(builder.body["draft"]["revision"], 3);

    let duplicate = service.handle_http(
        "POST",
        "/product/strategies/duplicate",
        json!({"strategyId": strategy_id, "name": "Stored Strategy Copy"}),
    );
    assert_eq!(duplicate.body["ok"], true);
    let duplicate_id = duplicate.body["body"]["strategy"]["id"]
        .as_str()
        .expect("duplicate id")
        .to_string();
    assert_ne!(duplicate_id, strategy_id);

    let workspace = service.handle_http("GET", "/workspace", json!({}));
    assert_strategy_named(
        &workspace.body["strategies"],
        &strategy_id,
        "Stored Strategy Republished",
    );
    assert_strategy_named(
        &workspace.body["strategies"],
        &duplicate_id,
        "Stored Strategy Copy",
    );

    let validation = service.handle_http("POST", "/strategy/contracts/validate", spec_v2);
    assert_eq!(validation.status, 200, "{:#?}", validation.body);
    assert_eq!(validation.body["body"]["ok"], true);

    let metadata = service.handle_http("GET", "/strategy/contracts/schema", json!({}));
    assert_eq!(metadata.status, 200);
    assert_eq!(metadata.body["title"], "StrategySpec");
    assert_eq!(metadata.body["type"], "object");
    assert!(metadata.body["required"].is_array());

    let agent_publish = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({"strategyId": strategy_id, "actor": {"kind": "agent", "id": "research_agent_v1"}}),
    );
    assert_eq!(agent_publish.body["ok"], false);
    assert_eq!(
        agent_publish.body["error"]["code"],
        "agent_cannot_publish_strategy_version"
    );
}

#[test]
fn strategy_draft_proposals_require_user_review_and_reject_stale_patches() {
    let db = format!(
        ".tradeassembly/test-strategy-proposals-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);

    let selection = service.handle_http(
        "POST",
        "/product/strategies/semantic-selection",
        json!({"strategyId": "strat_local_btc_demo", "nodeRef": "risk.sizing"}),
    );
    assert_eq!(selection.status, 200);
    assert_eq!(
        selection.body["body"]["selection"]["nodeRef"],
        "risk.sizing"
    );
    assert_eq!(
        selection.body["body"]["selection"]["context"]["payload"]["allocation"]["mode"],
        "none"
    );
    assert!(
        selection.body["body"]["selection"]["context"]["payload"]["stages"]["risk"].is_object()
    );
    let draft_hash = selection.body["body"]["selection"]["draftHash"]
        .as_str()
        .expect("draft hash")
        .to_string();

    let proposal = service.handle_http(
        "POST",
        "/product/strategies/proposals/create",
        json!({
            "strategyId": "strat_local_btc_demo",
            "expectedDraftHash": draft_hash,
            "selectionRef": selection.body["body"]["selection"]["id"].clone(),
            "specPatch": [{"op": "add", "path": "/stages/risk/documentation", "value": "Reviewed risk policy note"}],
            "summary": "Add risk policy documentation",
            "actor": {"kind": "agent", "id": "research_agent_v1"},
            "purpose": "strategy_authoring",
            "evidenceRefs": ["hash:user_instruction"]
        }),
    );
    assert_eq!(proposal.status, 200);
    assert_eq!(
        proposal.body["body"]["proposal"]["status"],
        "pending_review"
    );
    assert_eq!(proposal.body["body"]["proposal"]["riskLevel"], "high");
    assert_eq!(
        proposal.body["body"]["proposal"]["requiresExplicitReview"],
        true
    );
    assert_eq!(
        proposal.body["body"]["proposal"]["evidenceRefs"][0],
        "hash:user_instruction"
    );
    let proposal_id = proposal.body["body"]["proposal"]["id"]
        .as_str()
        .expect("proposal id")
        .to_string();

    let agent_apply = service.handle_http(
        "POST",
        "/product/strategies/proposals/review",
        json!({
            "strategyId": "strat_local_btc_demo",
            "proposalId": proposal_id,
            "decision": "accept",
            "actor": {"kind": "agent", "id": "research_agent_v1"}
        }),
    );
    assert_eq!(agent_apply.body["ok"], false);
    assert_eq!(
        agent_apply.body["error"]["code"],
        "agent_cannot_apply_strategy_patch"
    );

    let missing_review = service.handle_http(
        "POST",
        "/product/strategies/proposals/review",
        json!({
            "strategyId": "strat_local_btc_demo",
            "proposalId": proposal_id,
            "decision": "accept",
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(missing_review.body["ok"], false);
    assert_eq!(
        missing_review.body["error"]["code"],
        "high_impact_strategy_patch_requires_explicit_review"
    );

    let accepted = service.handle_http(
        "POST",
        "/product/strategies/proposals/review",
        json!({
            "strategyId": "strat_local_btc_demo",
            "proposalId": proposal_id,
            "decision": "accept",
            "actor": {"kind": "user", "id": "user.local"},
            "approvalMode": "explicit_user_review",
            "highImpactAcknowledged": true,
            "evidenceRefs": ["hash:user_review"]
        }),
    );
    assert_eq!(accepted.body["ok"], true);
    assert_eq!(accepted.body["body"]["applied"], true);
    assert_eq!(accepted.body["body"]["proposal"]["status"], "accepted");
    assert_eq!(
        accepted.body["body"]["draft"]["spec"]["stages"]["risk"]["documentation"],
        "Reviewed risk policy note"
    );

    let detail = service.handle_http("GET", "/strategies/strat_local_btc_demo", json!({}));
    assert_ne!(
        detail.body["strategy"]["latestSpec"]["stages"]["risk"]["documentation"],
        "Reviewed risk policy note",
        "accepted proposal must update draft without rewriting published/latest spec"
    );

    let secret_patch = service.handle_http(
        "POST",
        "/product/strategies/proposals/create",
        json!({
            "strategyId": "strat_local_btc_demo",
            "expectedDraftHash": accepted.body["body"]["draft"]["draftHash"].clone(),
            "specPatch": {"provider_ref": "sim", "credential_handle": "secret-handle"},
            "actor": {"kind": "agent", "id": "research_agent_v1"}
        }),
    );
    assert_eq!(secret_patch.body["ok"], false);
    assert_eq!(
        secret_patch.body["error"]["code"],
        "strategy_patch_contains_runtime_binding"
    );

    let stale = service.handle_http(
        "POST",
        "/product/strategies/proposals/create",
        json!({
            "strategyId": "strat_local_btc_demo",
            "proposalId": "proposal_stale",
            "expectedDraftHash": "hash-stale",
            "specPatch": {"price_policy": {"kind": "limit"}},
            "actor": {"kind": "agent", "id": "research_agent_v1"}
        }),
    );
    assert_eq!(stale.body["body"]["proposal"]["status"], "conflict");
}

fn assert_strategy_named(strategies: &Value, strategy_id: &str, name: &str) {
    let items = strategies.as_array().expect("strategy array");
    assert!(
        items
            .iter()
            .any(|strategy| strategy["id"] == strategy_id && strategy["name"] == name),
        "missing strategy {strategy_id} named {name}: {items:#?}"
    );
}

fn v3_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .expect("checked-in V3 fixture")
}
