use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use tradeassembly_runtime::agent_runner::{self, AgentDeployment, RUNS_NS};
use tradeassembly_runtime::auth::{validate_session, SessionValidationRequest};
use tradeassembly_runtime::mcp::{self, MCP_PROTOCOL_VERSION};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn external_agent_attachment_requires_authenticated_persistent_transport() {
    let args =
        json!({"deployment_id":"unknown","activation_id":"unknown","idempotency_key":"attach"});
    assert_eq!(
        mcp::call_tool("tradeassembly.agent.session.attach", args.clone())["isError"],
        true
    );
    let responses = run_mcp_stdio(&[
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tradeassembly.agent.session.attach","arguments":args}}),
    ]);
    assert_eq!(responses[0]["result"]["isError"], true);
    assert_eq!(
        responses[0]["result"]["structuredContent"]["error"]["code"],
        "external_attach_identity_required"
    );
}

#[test]
fn tradeassembly_mcp_stdio_lists_tools_but_denies_protected_calls_without_oidc() {
    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": MCP_PROTOCOL_VERSION}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "tradeassembly.strategy.list", "arguments": {}}
        }),
    ];

    let responses = run_mcp_stdio(&requests);

    assert_eq!(
        responses
            .iter()
            .map(|response| response["id"].as_i64().expect("response id"))
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        responses[0]["result"]["capabilities"]["tools"]["listChanged"],
        true
    );
    let tool_names = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    for expected in mcp::tool_names() {
        assert!(
            tool_names.contains(&expected),
            "missing MCP tool {expected}"
        );
    }
    assert_eq!(responses[2]["result"]["isError"], true);
    assert_eq!(
        responses[2]["result"]["structuredContent"]["error"]["code"],
        "oidc_session_required"
    );
}

#[test]
fn reusable_stdio_harness_uses_service_backed_tool_calls() {
    let input = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tradeassembly.strategy.list", "arguments": {}}
    })
    .to_string();

    let db = temp_db("stdio-harness").to_string_lossy().to_string();
    let served = mcp::serve_stdio_text_for_test(&input, &db, mcp::DEFAULT_STUDIO_BASE_URL);
    assert_eq!(served.exit_code, 0);
    assert_eq!(served.stderr, "");
    let response = serde_json::from_str::<Value>(served.stdout.trim()).expect("json response");
    let strategies = response["result"]["structuredContent"]["strategies"]
        .as_array()
        .expect("service-backed strategies");
    assert!(
        strategies
            .iter()
            .any(|strategy| strategy["id"] == "strat_local_btc_demo"),
        "MCP stdio harness returned stub strategy list: {response:#}"
    );
}

#[test]
fn robustness_artifact_mcp_tools_use_durable_routes_and_fail_closed() {
    let service = test_service("robustness-artifact-parity");

    for (name, arguments) in [
        (
            "tradeassembly.robustness.report",
            json!({"run_id": "missing"}),
        ),
        (
            "tradeassembly.robustness.replay",
            json!({
                "run_id": "missing",
                "idempotency_key": "mcp-replay-missing",
                "actor": "test-user"
            }),
        ),
        (
            "tradeassembly.robustness.export",
            json!({
                "run_id": "missing",
                "format": "json",
                "idempotency_key": "mcp-export-missing",
                "actor": "test-user"
            }),
        ),
    ] {
        let response = mcp::call_service_tool(&service, name, arguments);
        assert_eq!(response["isError"], true, "{response:#}");
        assert_eq!(
            response["structuredContent"]["error"]["code"], "robustness_not_found",
            "{response:#}"
        );
        assert!(!response.to_string().contains("resultHash"));
    }

    for (name, arguments, expected_code) in [
        (
            "tradeassembly.robustness.report",
            json!({"run_id": "run-1", "unexpected": true}),
            "robustness_arguments_invalid",
        ),
        (
            "tradeassembly.robustness.report",
            json!({"run_id": " "}),
            "robustness_run_id_required",
        ),
        (
            "tradeassembly.robustness.replay",
            json!({"run_id": "run-1"}),
            "robustness_idempotency_key_required",
        ),
        (
            "tradeassembly.robustness.export",
            json!({
                "run_id": "run-1",
                "format": "xml",
                "idempotency_key": "mcp-export-invalid"
            }),
            "robustness_export_format_invalid",
        ),
    ] {
        let response = mcp::call_service_tool(&service, name, arguments);
        assert_eq!(response["isError"], true, "{response:#}");
        assert_eq!(
            response["structuredContent"]["error"]["code"], expected_code,
            "{response:#}"
        );
    }

    let definitions = mcp::tool_definitions();
    for (name, required) in [
        ("tradeassembly.robustness.report", json!(["run_id"])),
        (
            "tradeassembly.robustness.replay",
            json!(["run_id", "idempotency_key"]),
        ),
        (
            "tradeassembly.robustness.export",
            json!(["run_id", "format", "idempotency_key"]),
        ),
    ] {
        let definition = definitions
            .as_array()
            .expect("tool definitions")
            .iter()
            .find(|tool| tool["name"] == name)
            .expect("robustness artifact tool");
        assert_eq!(definition["inputSchema"]["required"], required);
        assert_eq!(
            definition["inputSchema"]["additionalProperties"],
            json!(false)
        );
    }
}

#[test]
fn derivatives_mcp_tools_use_the_durable_http_facade() {
    let service = test_service("derivatives-parity");

    let list = mcp::call_service_tool(&service, "tradeassembly.derivatives.list", json!({}));
    assert_eq!(list["isError"], false, "{list:#}");
    assert_eq!(list["structuredContent"]["analyses"], json!([]));

    let invalid = mcp::call_service_tool(
        &service,
        "tradeassembly.derivatives.create",
        json!({"request": {"idempotencyKey": "derivatives-mcp-invalid"}}),
    );
    assert_eq!(invalid["isError"], true, "{invalid:#}");
    assert_eq!(
        invalid["structuredContent"]["error"]["code"],
        "derivatives_request_invalid"
    );
    assert!(!invalid.to_string().contains("outputHash"));

    let missing = mcp::call_service_tool(
        &service,
        "tradeassembly.derivatives.get",
        json!({"analysis_id": "missing"}),
    );
    assert_eq!(missing["isError"], true, "{missing:#}");
    assert_eq!(
        missing["structuredContent"]["error"]["code"],
        "derivatives_analysis_not_found"
    );

    let definitions = mcp::tool_definitions();
    for name in [
        "tradeassembly.derivatives.create",
        "tradeassembly.derivatives.list",
        "tradeassembly.derivatives.get",
        "tradeassembly.derivatives.export",
    ] {
        assert!(definitions
            .as_array()
            .expect("tool definitions")
            .iter()
            .any(|tool| tool["name"] == name));
    }
    let create = definitions
        .as_array()
        .expect("tool definitions")
        .iter()
        .find(|tool| tool["name"] == "tradeassembly.derivatives.create")
        .expect("derivatives create tool");
    assert_eq!(create["inputSchema"]["required"], json!(["request"]));
    assert_eq!(
        create["inputSchema"]["properties"]["request"]["required"],
        json!(["sourceRunId", "idempotencyKey"])
    );
    assert_eq!(
        create["inputSchema"]["properties"]["request"]["additionalProperties"],
        false
    );
}

#[test]
fn derivatives_mcp_stdio_uses_the_same_durable_facade() {
    let responses = run_mcp_stdio(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tradeassembly.derivatives.list", "arguments": {}}
    })]);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(
        responses[0]["result"]["structuredContent"]["error"]["code"],
        "oidc_session_required"
    );
}

#[test]
fn comparison_mcp_tools_use_the_durable_http_facade() {
    let service = test_service("comparison-parity");

    let list = mcp::call_service_tool(&service, "tradeassembly.comparison.list", json!({}));
    assert_eq!(list["isError"], false, "{list:#}");
    assert_eq!(list["structuredContent"]["comparisons"], json!([]));

    let invalid = mcp::call_service_tool(
        &service,
        "tradeassembly.comparison.create",
        json!({"request": {"idempotencyKey": "comparison-mcp-invalid"}}),
    );
    assert_eq!(invalid["isError"], true, "{invalid:#}");
    assert_eq!(
        invalid["structuredContent"]["error"]["code"],
        "comparison_request_invalid"
    );
    assert!(!invalid.to_string().contains("comparisonHash"));

    let missing = mcp::call_service_tool(
        &service,
        "tradeassembly.comparison.get",
        json!({"comparison_id": "missing"}),
    );
    assert_eq!(missing["isError"], true, "{missing:#}");
    assert_eq!(
        missing["structuredContent"]["error"]["code"],
        "comparison_not_found"
    );

    let definitions = mcp::tool_definitions();
    for name in [
        "tradeassembly.comparison.create",
        "tradeassembly.comparison.list",
        "tradeassembly.comparison.get",
        "tradeassembly.comparison.export",
    ] {
        assert!(definitions
            .as_array()
            .expect("tool definitions")
            .iter()
            .any(|tool| tool["name"] == name));
    }
    let create = definitions
        .as_array()
        .expect("tool definitions")
        .iter()
        .find(|tool| tool["name"] == "tradeassembly.comparison.create")
        .expect("comparison create tool");
    assert_eq!(create["inputSchema"]["required"], json!(["request"]));
    assert_eq!(
        create["inputSchema"]["properties"]["request"]["required"],
        json!(["sourceRunIds", "scope", "idempotencyKey"])
    );
    assert_eq!(
        create["inputSchema"]["properties"]["request"]["additionalProperties"],
        false
    );
    assert_eq!(
        create["inputSchema"]["properties"]["request"]["properties"]["sourceRunIds"]["minItems"],
        2
    );
    assert_eq!(
        create["inputSchema"]["properties"]["request"]["properties"]["sourceRunIds"]["maxItems"],
        5
    );
}

#[test]
fn comparison_mcp_stdio_uses_the_same_durable_facade() {
    let responses = run_mcp_stdio(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tradeassembly.comparison.list", "arguments": {}}
    })]);

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(
        responses[0]["result"]["structuredContent"]["error"]["code"],
        "oidc_session_required"
    );
}

#[test]
fn mcp_execution_submission_fails_closed_without_explicit_authority() {
    let service = test_service("execution-submission");
    let response = service.call_mcp_tool(
        "tradeassembly.execution.run",
        json!({
            "activation_id": "activation-btc-exit-demo",
            "submit_orders": true
        }),
    );

    assert_eq!(response["isError"], true);
    assert_eq!(
        response["structuredContent"]["error"]["code"],
        "mcp_order_submission_disabled"
    );
    assert!(!response.to_string().contains("api_secret"));
    assert!(!response.to_string().contains("Bearer "));
}

#[test]
fn sightline_mcp_context_ignores_a_caller_supplied_user_actor() {
    let service = test_service("sightline-unshared-selection");
    service.handle_http(
        "POST",
        "/product/sightline/strategy-editor/publish",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    let unauthenticated = validate_session(
        SessionValidationRequest {
            provider: "unsupported".to_string(),
            ..SessionValidationRequest::default()
        },
        None,
        None,
        None,
    );
    let internal_set = service.set_sightline_selection_for_authenticated_user(
        &unauthenticated,
        json!({"strategyId": "strat_local_btc_demo", "nodeRef": "strategy.risk", "shared": true}),
    );
    assert_eq!(
        internal_set["error"]["code"],
        "sightline_user_authority_required"
    );
    let internal_share = service.share_sightline_selection_for_authenticated_user(
        &unauthenticated,
        json!({"selectionId": "selection_attacker_supplied"}),
    );
    assert_eq!(
        internal_share["error"]["code"],
        "sightline_user_authority_required"
    );
    let set_selection = service.handle_http(
        "POST",
        "/product/sightline/selections/set",
        json!({
            "strategyId": "strat_local_btc_demo",
            "nodeRef": "strategy.risk",
            "shared": false
        }),
    );
    assert_eq!(set_selection.status, 403);
    assert_eq!(
        set_selection.body["error"]["code"],
        "sightline_user_authority_required"
    );

    for name in [
        "tradeassembly.sightline.session_state",
        "tradeassembly.sightline.get_current_selection",
        "tradeassembly.sightline.get_context",
    ] {
        let response =
            service.call_mcp_tool(name, json!({"actor": {"kind": "user", "id": "user.local"}}));
        let body = &response["structuredContent"]["body"];
        assert!(
            body.is_object(),
            "{name} returned an invalid payload: {response:#}"
        );
        assert!(
            body["selection"].is_null(),
            "{name} exposed an unshared selection: {response:#}"
        );
        assert!(
            body["context"].is_null(),
            "{name} exposed unshared context: {response:#}"
        );
    }

    for name in [
        "tradeassembly.sightline.create_proposal",
        "tradeassembly.sightline.request_approval",
    ] {
        let response =
            service.call_mcp_tool(name, json!({"actor": {"kind": "user", "id": "user.local"}}));
        assert_eq!(
            response["structuredContent"]["error"]["code"], "sightline_selection_not_shared",
            "{name} accepted an unshared selection: {response:#}"
        );
    }

    let events = service.call_mcp_tool(
        "tradeassembly.sightline.list_events",
        json!({"actor": {"kind": "user", "id": "user.local"}}),
    );
    assert!(
        events["structuredContent"]["body"]["events"]
            .as_array()
            .expect("events array")
            .iter()
            .all(|event| event["type"] != "selection.changed"),
        "MCP exposed the unshared selection event: {events:#}"
    );

    let api = service.handle_http(
        "GET",
        "/product/sightline/session-state",
        json!({"actor": {"kind": "user", "id": "user.local"}}),
    );
    assert!(
        api.body["body"]["selection"].is_null(),
        "direct Sightline API exposed an unshared selection: {:#}",
        api.body
    );

    let share_selection = service.handle_http(
        "POST",
        "/product/sightline/selections/share",
        json!({
            "strategyId": "strat_local_btc_demo",
            "selectionId": "selection_attacker_supplied",
            "actor": {"kind": "user", "id": "spoofed.user"}
        }),
    );
    assert_eq!(share_selection.status, 403);
    assert_eq!(
        share_selection.body["error"]["code"],
        "sightline_user_authority_required"
    );
}

#[test]
fn direct_sightline_agent_actions_ignore_a_caller_supplied_user_actor() {
    let service = test_service("sightline-direct-agent-attribution");
    service.handle_http(
        "POST",
        "/product/sightline/strategy-editor/publish",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    let session = validate_session(SessionValidationRequest::default(), None, None, None);
    service.set_sightline_selection_for_authenticated_user(
        &session,
        json!({
            "strategyId": "strat_local_btc_demo",
            "nodeRef": "strategy.risk",
            "shared": true
        }),
    );
    let spoofed_actor = json!({"kind": "user", "id": "spoofed.user"});

    for path in [
        "/product/sightline/navigation/request",
        "/product/sightline/nodes/focus",
        "/product/sightline/nodes/highlight",
    ] {
        let response = service.handle_http(
            "POST",
            path,
            json!({"strategyId": "strat_local_btc_demo", "actor": spoofed_actor}),
        );
        assert_eq!(
            response.body["body"]["command"]["issued_by"]["actor_type"],
            "agent"
        );
    }

    let proposal = service.handle_http(
        "POST",
        "/product/sightline/proposals/create",
        json!({
            "strategyId": "strat_local_btc_demo",
            "patch": {"metadata": {"reviewed": true}},
            "actor": spoofed_actor
        }),
    );
    assert_eq!(
        proposal.body["body"]["proposal"]["proposed_by"]["actor_type"],
        "agent"
    );

    let approval = service.handle_http(
        "POST",
        "/product/sightline/approvals/request",
        json!({"strategyId": "strat_local_btc_demo", "actor": spoofed_actor}),
    );
    assert_eq!(
        approval.body["body"]["approval"]["requested_by"]["actor_type"],
        "agent"
    );
}

#[test]
fn mcp_exposes_local_account_install_and_telemetry_tools() {
    let definitions = mcp::tool_definitions();
    let tool_names = definitions
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    for expected in [
        "tradeassembly.account.status",
        "tradeassembly.account.login",
        "tradeassembly.install.claim",
        "tradeassembly.telemetry.emit",
        "tradeassembly.telemetry.opt_out",
        "tradeassembly.telemetry.replay",
    ] {
        assert!(
            tool_names.contains(&expected),
            "missing MCP tool {expected}"
        );
    }

    let service = test_service("account-telemetry");
    let status = service.call_mcp_tool(
        "tradeassembly.account.status",
        json!({"profile": "local", "studio_base_url": "http://127.0.0.1:3001"}),
    );
    assert_eq!(
        status["structuredContent"]["error"]["code"],
        "identity_transport_required"
    );
    assert_eq!(status["isError"], true);
    assert!(!status.to_string().contains("local:user_123"));
    assert!(!status.to_string().contains("api_secret"));

    let emitted = service.call_mcp_tool(
        "tradeassembly.telemetry.emit",
        json!({"profile": "local", "event": "first_backtest"}),
    );
    assert_eq!(emitted["structuredContent"]["status"], "accepted");
    assert_eq!(emitted["structuredContent"]["payload"]["surface"], "local");
}

#[test]
fn mcp_exposes_attribution_journal_review_report_replay_and_export() {
    let definitions = mcp::tool_definitions();
    let tool_names = definitions
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    for expected in [
        "tradeassembly.attribution_journal.review",
        "tradeassembly.attribution_journal.report",
        "tradeassembly.attribution_journal.replay",
        "tradeassembly.attribution_journal.export",
        "tradeassembly.attribution_journal.inspect",
    ] {
        assert!(
            tool_names.contains(&expected),
            "missing MCP attribution tool {expected}"
        );
    }

    let service = test_service("attribution-journal");
    let response = service.call_mcp_tool(
        "tradeassembly.attribution_journal.report",
        json!({
            "strategy_id": "strat_local_btc_demo",
            "review_id": "attribution_journal_latest",
            "studio_base_url": "http://127.0.0.1:3001",
            "metadata": {"api_secret": "SHOULD_NOT_LEAK"}
        }),
    );
    let payload = &response["structuredContent"];
    assert_eq!(
        payload["schemaVersion"],
        "tradeassembly.attribution_journal.service.v1"
    );
    assert_eq!(payload["kind"], "attribution_journal");
    assert!(payload["agentDisplaySummary"]
        .as_str()
        .unwrap()
        .contains("Reviewed"));
    assert!(payload["deepLinks"]["journal"]["href"]
        .as_str()
        .unwrap()
        .contains("attributionJournal=attribution_journal_latest"));
    assert_eq!(payload["sideEffects"]["brokerStateChanged"], false);
    assert!(!response.to_string().contains("SHOULD_NOT_LEAK"));
    assert!(!response.to_string().contains("Bearer "));
    assert!(!response.to_string().to_lowercase().contains("should buy"));
}

#[test]
fn mcp_external_broker_receipts_are_redacted_idempotent_and_reconciliation_safe() {
    let service = test_service("external-broker-receipts");
    let deployment = AgentDeployment {
        executor: Default::default(),
        deployment_id: "deployment-1".to_string(),
        system_project_id: "system-1".to_string(),
        agent_definition_version_id: "agent-v1".to_string(),
        execution_config_version_id: "config-v1".to_string(),
        studio_tool_allowlist: vec![
            "studio.execution.external_receipt.append".to_string(),
            "studio.execution.external_receipt.inspect".to_string(),
            "studio.execution.external_receipt.reconcile".to_string(),
        ],
        desired_state: "active".to_string(),
        interval_seconds: 60,
        cron_utc: None,
        mode: "live".to_string(),
        prompt: "Use only redacted evidence.".to_string(),
        workspace: std::env::temp_dir().display().to_string(),
        runtime_profile: "tradeassembly-local".to_string(),
    };
    agent_runner::put_deployment(&service.runtime(), &deployment).unwrap();
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("external-receipt-run-fixture").unwrap(),
    );
    service
        .runtime()
        .storage
        .put_json(
            RUNS_NS,
            "run-1",
            json!({"run": {"runId": "run-1", "deploymentId": "deployment-1", "state": "completed"}}),
            &context,
        )
        .unwrap();

    let append = |event_type: &str, idempotency_key: &str| {
        service.call_mcp_tool(
            "studio.execution.external_receipt.append",
            json!({
                "deployment_id": "deployment-1",
                "run_id": "run-1",
                "broker": "alpaca",
                "environment": "live",
                "client_order_id": "client-1",
                "broker_order_id": "broker-1",
                "event_type": event_type,
                "occurred_at_ms": 100,
                "receipt": {"status": event_type, "filledQty": "1"},
                "source_ref": "local-alpaca-cli:client-1",
                "idempotency_key": idempotency_key,
                "authority_context": {"actor": "agent-run-1", "surface": "mcp", "accountMode": "live"}
            }),
        )
    };

    let filled = append("order_filled", "external-receipt-1");
    assert_eq!(filled["isError"], false, "{filled:#}");
    assert_eq!(
        filled["structuredContent"]["receipt"]["canonicalEventType"],
        "order_filled"
    );
    assert!(service
        .runtime()
        .journal
        .events()
        .iter()
        .any(|event| event.event_type == "external_broker_receipt.appended"));
    assert_eq!(
        service
            .runtime()
            .outbox
            .claim_pending("external-broker-test", 200, 10)
            .unwrap()
            .len(),
        1
    );

    let duplicate = append("order_filled", "external-receipt-1");
    assert_eq!(duplicate["isError"], false, "{duplicate:#}");
    assert_eq!(duplicate["duplicate"], true, "{duplicate:#}");

    let out_of_order = append("order_accepted", "external-receipt-2");
    assert_eq!(out_of_order["isError"], false, "{out_of_order:#}");
    assert_eq!(out_of_order["structuredContent"]["outOfOrder"], true);
    let inspected = service.call_mcp_tool(
        "studio.execution.external_receipt.inspect",
        json!({"deployment_id": "deployment-1", "client_order_id": "client-1"}),
    );
    assert_eq!(inspected["isError"], false, "{inspected:#}");
    assert_eq!(
        inspected["structuredContent"]["receipt"]["canonicalEventType"],
        "order_filled"
    );
    assert_eq!(
        inspected["structuredContent"]["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let terminal_conflict = append("order_cancelled", "external-receipt-3");
    assert_eq!(terminal_conflict["isError"], false, "{terminal_conflict:#}");
    assert_eq!(
        terminal_conflict["structuredContent"]["reconciliationRequired"],
        true
    );
    assert_eq!(
        terminal_conflict["structuredContent"]["receipt"]["reconciliationStatus"],
        "required"
    );
    let reconciled = service.call_mcp_tool(
        "studio.execution.external_receipt.reconcile",
        json!({
            "deployment_id": "deployment-1",
            "client_order_id": "client-1",
            "resolution": "reviewed",
            "idempotency_key": "external-receipt-reconcile-1",
            "authority_context": {"actor": "operator-1", "surface": "mcp", "accountMode": "live"}
        }),
    );
    assert_eq!(reconciled["isError"], false, "{reconciled:#}");
    assert_eq!(
        reconciled["structuredContent"]["receipt"]["reconciliationStatus"],
        "reconciled"
    );

    let conflicting_key = service.call_mcp_tool(
        "studio.execution.external_receipt.append",
        json!({
            "deployment_id": "deployment-1",
            "run_id": "run-1",
            "broker": "alpaca",
            "environment": "live",
            "client_order_id": "client-2",
            "event_type": "order_accepted",
            "receipt": {"status": "accepted"},
            "idempotency_key": "external-receipt-1",
            "authority_context": {"actor": "agent-run-1", "surface": "mcp", "accountMode": "live"}
        }),
    );
    assert_eq!(conflicting_key["isError"], true, "{conflicting_key:#}");
    assert_eq!(
        conflicting_key["structuredContent"]["error"]["code"],
        "idempotency_conflict"
    );

    let sensitive = service.call_mcp_tool(
        "studio.execution.external_receipt.append",
        json!({
            "deployment_id": "deployment-1",
            "run_id": "run-1",
            "broker": "alpaca",
            "environment": "live",
            "client_order_id": "client-3",
            "event_type": "order_accepted",
            "receipt": {"api_secret": "MUST_NOT_PERSIST"},
            "idempotency_key": "external-receipt-sensitive",
            "authority_context": {"actor": "agent-run-1", "surface": "mcp", "accountMode": "live"}
        }),
    );
    assert_eq!(sensitive["isError"], true, "{sensitive:#}");
    assert_eq!(
        sensitive["structuredContent"]["error"]["code"],
        "external_broker_receipt_sensitive_payload"
    );
    assert!(!service
        .runtime()
        .journal
        .events()
        .iter()
        .any(|event| event.payload.to_string().contains("MUST_NOT_PERSIST")));
}

#[test]
fn mcp_unknown_tool_returns_protocol_safe_error() {
    let service = test_service("unknown-tool");
    let response = service.call_mcp_tool(
        "tradeassembly.missing",
        json!({"api_secret": "SHOULD_NOT_LEAK"}),
    );

    assert_eq!(response["isError"], true);
    assert_eq!(response["structuredContent"]["ok"], false);
    assert!(!response.to_string().contains("SHOULD_NOT_LEAK"));
}

#[test]
fn authenticated_mcp_replaces_spoofed_human_and_records_agent_provenance() {
    let service = test_service("authenticated-authority").for_authenticated_invocation(
        "http://issuer.example",
        "identity_verified_user",
        Some("verified@example.invalid".to_string()),
        Some("Verified User".to_string()),
    );
    let response = service.call_mcp_tool(
        "tradeassembly.strategy.create",
        json!({
            "name": "Authenticated MCP strategy",
            "idempotencyKey": "authenticated-mcp-create",
            "actor": {"kind": "user", "id": "spoofed-user"},
            "actorId": "spoofed-user",
            "principalUser": "spoofed-user",
            "mode": "live",
            "accountMode": "live",
            "authorityContext": {
                "actor": "spoofed-user",
                "principalUser": "spoofed-user",
                "accountMode": "live"
            }
        }),
    );
    assert_eq!(response["isError"], false, "{response:#}");

    let record = service
        .control_plane_commands()
        .into_iter()
        .find(|record| record["envelope"]["idempotency_key"] == "authenticated-mcp-create")
        .expect("authenticated MCP command record");
    assert_eq!(
        record["envelope"]["authority"]["actor"],
        "identity_verified_user"
    );
    assert_eq!(
        record["envelope"]["payload_preview"]["principalUser"], "identity_verified_user",
        "{record:#}"
    );
    assert_eq!(
        record["envelope"]["payload_preview"]["callingAgent"]["id"],
        "tradeassembly.mcp_agent"
    );
    assert_eq!(record["envelope"]["authority"]["account_mode"], "paper");
    assert_eq!(record["envelope"]["payload_preview"]["mode"], "paper");
    assert_eq!(
        record["envelope"]["payload_preview"]["authorityContext"]["accountMode"],
        "paper"
    );
    assert!(!record.to_string().contains("spoofed-user"));
}

#[test]
fn authenticated_mcp_preserves_live_configuration_without_activating() {
    let service = test_service("authenticated-execution-mode").for_authenticated_invocation(
        "http://issuer.example",
        "identity_verified_user",
        Some("verified@example.invalid".to_string()),
        Some("Verified User".to_string()),
    );
    let response = service.call_mcp_tool(
        "tradeassembly.execution.config.save",
        json!({
            "strategy_id": "strat_local_btc_demo",
            "provider_ref": "sim",
            "mode": "live",
            "accountMode": "live",
            "account_mode": "live"
        }),
    );

    assert_eq!(response["isError"], false, "{response:#}");
    assert_eq!(
        response["structuredContent"]["body"]["item"]["mode"], "live",
        "{response:#}"
    );
    let config_id = response["structuredContent"]["body"]["configId"]
        .as_str()
        .unwrap();
    let persisted = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .unwrap()
        .unwrap();
    assert_eq!(persisted["mode"], "live");
    for namespace in [
        "execution_activations",
        "execution_runs",
        "broker_order_intents",
    ] {
        assert!(
            service
                .runtime()
                .storage
                .list_json(namespace)
                .unwrap()
                .is_empty(),
            "configuration save created {namespace}"
        );
    }
}

fn run_mcp_stdio(requests: &[Value]) -> Vec<Value> {
    run_mcp_stdio_with_missing_dependency(requests, false)
}

fn run_mcp_stdio_with_missing_dependency(
    requests: &[Value],
    missing_dependency: bool,
) -> Vec<Value> {
    let session_root = temp_db("anonymous-session");
    let mut child = Command::new(tradeassembly_binary())
        .arg("--db")
        .arg(&session_root)
        .args(["mcp", "serve", "--transport", "stdio"])
        .env("TRADEASSEMBLY_AUTH_PROFILE", "keycloak_local")
        .env("TRADEASSEMBLY_WARDEN_SIDECAR_URL", "http://127.0.0.1:9")
        .env(
            "TRADEASSEMBLY_WARDEN_TOKEN_REF",
            if missing_dependency {
                "file:///nonexistent-tradeassembly-test/warden.token"
            } else {
                "env://TRADEASSEMBLY_TEST_WARDEN_TOKEN"
            },
        )
        .env(
            "TRADEASSEMBLY_TEST_WARDEN_TOKEN",
            "mcp-read-test-token-with-at-least-32-bytes",
        )
        .env(
            "TRADEASSEMBLY_AUTH_CLI_SESSION_PATH",
            session_root.with_extension("bin"),
        )
        .env(
            "TRADEASSEMBLY_AUTH_CLI_SESSION_KEY_PATH",
            session_root.with_extension("key"),
        )
        .env(
            "TRADEASSEMBLY_AUTH_ISSUER",
            "http://127.0.0.1:9/realms/tradeassembly-test",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tradeassembly mcp stdio");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write MCP request");
        }
    }
    let output = child
        .wait_with_output()
        .expect("wait for tradeassembly mcp");
    assert!(
        output.status.success(),
        "mcp serve failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "unexpected stderr"
    );
    String::from_utf8(output.stdout)
        .expect("utf8 stdout")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("json response"))
        .collect()
}

#[test]
fn onboarding_bootstrap_is_available_before_login_without_running_automation() {
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tradeassembly.setup.inspect","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tradeassembly.account.login.status","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tradeassembly.account.login","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tradeassembly.broker.status","arguments":{}}}),
    ];
    let result = run_mcp_stdio(&requests);
    let setup = &result[0]["result"]["structuredContent"];
    assert_eq!(setup["nextAction"]["tool"], "tradeassembly.account.login");
    assert_eq!(setup["automationReady"], false);
    assert!(setup["workspace"].is_null());
    assert_eq!(
        result[1]["result"]["structuredContent"]["status"],
        "required"
    );
    assert_eq!(
        result[2]["result"]["structuredContent"]["status"],
        "pending"
    );
    assert_eq!(
        result[3]["result"]["structuredContent"]["error"]["code"],
        "oidc_session_required"
    );
}

#[test]
fn missing_local_secret_keeps_mcp_inspection_available_but_cannot_execute() {
    let result = run_mcp_stdio_with_missing_dependency(
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tradeassembly.setup.inspect","arguments":{}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tradeassembly.health","arguments":{}}}),
        ],
        true,
    );
    assert_eq!(result[0]["result"]["serverInfo"]["name"], "tradeassembly");
    assert_eq!(
        result[1]["result"]["structuredContent"]["tooling"]["available"],
        false
    );
    assert_eq!(
        result[1]["result"]["structuredContent"]["nextAction"]["action"],
        "repair_installation"
    );
    assert_eq!(
        result[2]["result"]["structuredContent"]["error"]["code"],
        "setup_required"
    );
}

fn tradeassembly_binary() -> PathBuf {
    if let Some(binary) = option_env!("CARGO_BIN_EXE_tradeassembly") {
        return PathBuf::from(binary);
    }
    if let Ok(binary) = std::env::var("CARGO_BIN_EXE_tradeassembly") {
        return PathBuf::from(binary);
    }
    let mut binary = std::env::current_exe()
        .expect("test executable path")
        .parent()
        .expect("test executable directory")
        .parent()
        .expect("target debug directory")
        .join("tradeassembly");
    if cfg!(windows) {
        binary.set_extension("exe");
    }
    if !binary.exists() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string()))
            .arg("build")
            .arg("--manifest-path")
            .arg(manifest)
            .args(["--bin", "tradeassembly"])
            .status()
            .expect("build tradeassembly binary");
        assert!(status.success(), "build tradeassembly binary failed");
    }
    binary
}

fn test_service(name: &str) -> TradeAssemblyService {
    TradeAssemblyService::test_local(temp_db(name).to_string_lossy().to_string())
}

fn temp_db(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "tradeassembly-mcp-parity-{name}-{}-{stamp}.db",
        std::process::id()
    ))
}
