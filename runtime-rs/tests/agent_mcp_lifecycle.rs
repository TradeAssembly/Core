// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use tradeassembly_runtime::agent_runner::{ACTIVE_RUNS_NS, DEPLOYMENTS_NS, RUNS_NS};
use tradeassembly_runtime::mcp;
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

fn authority(mode: &str) -> Value {
    json!({"actor": "operator-1", "surface": "mcp", "accountMode": mode})
}

fn deployment(workspace: &str, desired_state: &str, prompt: &str) -> Value {
    json!({
        "deploymentId": "deploy-mcp",
        "systemProjectId": "system-local",
        "agentDefinitionVersionId": "agent-v1",
        "executionConfigVersionId": "config-v1",
        "studioToolAllowlist": [
            "studio.deployment.inspect",
            "studio.agent_run.health",
            "studio.execution.external_receipt.append"
        ],
        "desiredState": desired_state,
        "intervalSeconds": 60,
        "cronUtc": "0 * * * *",
        "mode": "live",
        "prompt": prompt,
        "workspace": workspace,
        "runtimeProfile": "tradeassembly-local"
    })
}

fn create_request(workspace: &str, idempotency_key: &str, prompt: &str) -> Value {
    json!({
        "deployment": deployment(workspace, "paused", prompt),
        "idempotency_key": idempotency_key,
        "authority_context": authority("live")
    })
}

fn state_request(idempotency_key: &str) -> Value {
    json!({
        "deployment_id": "deploy-mcp",
        "idempotency_key": idempotency_key,
        "authority_context": authority("live")
    })
}

fn code(response: &Value) -> &str {
    response["structuredContent"]["error"]["code"]
        .as_str()
        .unwrap_or_default()
}

#[test]
fn external_client_mcp_deployment_does_not_require_a_supervised_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("external.db");
    let service = TradeAssemblyService::test_local(db.to_str().unwrap())
        .for_authenticated_invocation("test", "external-owner", None, None);
    let mut request = create_request("", "external-create", "");
    request["deployment"]["executor"] = json!("external_client");
    request["deployment"]["mode"] = json!("paper");
    request["authority_context"] = authority("paper");
    for field in ["workspace", "prompt", "intervalSeconds", "cronUtc"] {
        request["deployment"].as_object_mut().unwrap().remove(field);
    }
    let created = service.call_mcp_tool("studio.deployment.create", request.clone());
    assert_eq!(created["isError"], false, "{}", code(&created));
    assert_eq!(
        created["structuredContent"]["deployment"]["executor"],
        "external_client"
    );
    let duplicate = service.call_mcp_tool("studio.deployment.create", request);
    assert_eq!(duplicate["isError"], false);
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json(DEPLOYMENTS_NS)
            .unwrap()
            .len(),
        1
    );
    assert!(service
        .runtime()
        .storage
        .list_json(tradeassembly_runtime::agent_runner::SCHEDULES_NS)
        .unwrap()
        .is_empty());
    let definitions = mcp::tool_definitions();
    let create = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "studio.deployment.create")
        .unwrap();
    assert_eq!(
        create["inputSchema"]["properties"]["deployment"]["properties"]["executor"]["enum"],
        json!(["supervised", "external_client"])
    );
}

#[test]
fn agent_deployment_mcp_lifecycle_is_redacted_authorized_and_idempotent() {
    let directory = tempfile::tempdir().expect("temporary workspace");
    let db = directory
        .path()
        .join("tradeassembly.db")
        .display()
        .to_string();
    let workspace = directory.path().display().to_string();
    let service = TradeAssemblyService::test_local(&db);

    let mut missing_authority = create_request(
        &workspace,
        "agent-mcp-create-no-authority",
        "SENTINEL_PROMPT_MUST_NOT_LEAK",
    );
    missing_authority
        .as_object_mut()
        .expect("request object")
        .remove("authority_context");
    let rejected = service.call_mcp_tool("studio.deployment.create", missing_authority);
    assert_eq!(rejected["isError"], true, "{rejected:#}");
    assert_eq!(code(&rejected), "agent_deployment_authority_required");
    assert!(service
        .runtime()
        .storage
        .list_json(DEPLOYMENTS_NS)
        .expect("deployment list")
        .is_empty());

    let request = create_request(
        &workspace,
        "agent-mcp-create-1",
        "SENTINEL_PROMPT_MUST_NOT_LEAK",
    );
    let created = service.call_mcp_tool("studio.deployment.create", request.clone());
    assert_eq!(created["isError"], false, "{created:#}");
    let created_body = &created["structuredContent"];
    assert_eq!(created_body["deployment"]["deploymentId"], "deploy-mcp");
    assert_eq!(created_body["deployment"]["desiredState"], "paused");
    assert_eq!(created_body["deployment"]["mode"], "live");
    assert_eq!(created_body["deployment"]["cronUtc"], "0 * * * *");
    assert_eq!(created_body["redacted"], true);
    let rendered = created.to_string();
    assert!(!rendered.contains("SENTINEL_PROMPT_MUST_NOT_LEAK"));
    assert!(!rendered.contains(&workspace));
    assert!(!rendered.contains("runtimeProfile"));

    let commands = service.control_plane_commands();
    let command = commands
        .iter()
        .find(|command| command["envelope"]["command_name"] == "agent.deployment.create")
        .expect("persisted create command");
    assert_eq!(command["envelope"]["authority"]["actor"], "operator-1");
    assert_eq!(command["envelope"]["source_interface"], "mcp");
    assert_eq!(command["envelope"]["idempotency_key"], "agent-mcp-create-1");
    assert_eq!(command["response_status"], 200);

    let duplicate = service.call_mcp_tool("studio.deployment.create", request.clone());
    assert_eq!(duplicate["isError"], false, "{duplicate:#}");
    assert_eq!(duplicate["duplicate"], true, "{duplicate:#}");

    let mut conflict_request = request;
    conflict_request["deployment"]["prompt"] = json!("changed nonsecret prompt");
    let conflict = service.call_mcp_tool("studio.deployment.create", conflict_request);
    assert_eq!(conflict["isError"], true, "{conflict:#}");
    assert_eq!(code(&conflict), "idempotency_conflict");
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json(DEPLOYMENTS_NS)
            .expect("deployment list")
            .len(),
        1
    );

    for (tool, key, expected_state) in [
        ("studio.deployment.start", "agent-mcp-start-1", "active"),
        ("studio.deployment.pause", "agent-mcp-pause-1", "paused"),
        ("studio.deployment.stop", "agent-mcp-stop-1", "stopped"),
    ] {
        let response = service.call_mcp_tool(tool, state_request(key));
        assert_eq!(response["isError"], false, "{tool}: {response:#}");
        assert_eq!(
            response["structuredContent"]["deployment"]["desiredState"], expected_state,
            "{tool}: {response:#}"
        );
    }
    assert!(service
        .runtime()
        .storage
        .list_json(RUNS_NS)
        .expect("run list")
        .is_empty());

    let inspected = service.call_mcp_tool(
        "studio.deployment.inspect",
        json!({"deployment_id": "deploy-mcp"}),
    );
    assert_eq!(inspected["isError"], false, "{inspected:#}");
    assert_eq!(
        inspected["structuredContent"]["deployments"][0]["desiredState"],
        "stopped"
    );
    assert!(!inspected
        .to_string()
        .contains("SENTINEL_PROMPT_MUST_NOT_LEAK"));
    assert!(!inspected.to_string().contains(&workspace));
}

#[test]
fn authenticated_mcp_lifecycle_preserves_explicit_live_mode_and_verified_identity() {
    let directory = tempfile::tempdir().expect("temporary workspace");
    let db = directory
        .path()
        .join("tradeassembly.db")
        .display()
        .to_string();
    let workspace = directory.path().display().to_string();
    let base = TradeAssemblyService::test_local(&db);
    let service = base.for_authenticated_invocation(
        "test-issuer",
        "verified-user-42",
        Some("verified@example.test".to_string()),
        Some("Verified User".to_string()),
    );
    let request = json!({
        "deployment": deployment(&workspace, "paused", "authenticated lifecycle prompt"),
        "idempotency_key": "authenticated-live-create",
        "authority_context": {
            "actor": "forged-user",
            "surface": "mcp",
            "accountMode": "live"
        }
    });

    let created = service.call_mcp_tool("studio.deployment.create", request);
    assert_eq!(created["isError"], false, "{created:#}");
    assert_eq!(created["structuredContent"]["deployment"]["mode"], "live");
    let command = service
        .control_plane_commands()
        .into_iter()
        .find(|command| command["envelope"]["command_name"] == "agent.deployment.create")
        .expect("durable control command");
    assert_eq!(
        command["envelope"]["authority"]["actor"],
        "verified-user-42"
    );
    assert_eq!(command["envelope"]["authority"]["account_mode"], "live");
}

#[test]
fn agent_run_mcp_status_events_and_acknowledged_recovery_never_replay_a_run() {
    let directory = tempfile::tempdir().expect("temporary workspace");
    let db = directory
        .path()
        .join("tradeassembly.db")
        .display()
        .to_string();
    let workspace = directory.path().display().to_string();
    let service = TradeAssemblyService::test_local(&db);
    let created = service.call_mcp_tool(
        "studio.deployment.create",
        create_request(
            &workspace,
            "agent-mcp-create-pending",
            "PENDING_PROMPT_MUST_NOT_LEAK",
        ),
    );
    assert_eq!(created["isError"], false, "{created:#}");

    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("agent-mcp-pending-fixture").expect("fixture idempotency"),
    );
    service
        .runtime()
        .storage
        .put_json(
            RUNS_NS,
            "run-pending",
            json!({
                "schemaVersion": "tradeassembly.agent_runner.v1",
                "kind": "AgentRun",
                "run": {
                    "runId": "run-pending",
                    "deploymentId": "deploy-mcp",
                    "triggerId": "trigger-pending",
                    "state": "pending_reconcile",
                    "leaseFence": 7,
                    "deploymentBindingDigest": "binding-pending",
                    "codexSessionRef": "OPAQUE_SESSION_MUST_NOT_LEAK"
                },
                "errorCode": "agent_runtime_failed"
            }),
            &context,
        )
        .expect("persist pending run");
    service
        .runtime()
        .storage
        .put_json(
            ACTIVE_RUNS_NS,
            "deploy-mcp",
            json!({"runId": "run-pending", "state": "pending_reconcile"}),
            &context,
        )
        .expect("persist pending run pointer");

    let inspected =
        service.call_mcp_tool("studio.agent_run.inspect", json!({"run_id": "run-pending"}));
    assert_eq!(inspected["isError"], false, "{inspected:#}");
    assert_eq!(
        inspected["structuredContent"]["run"]["state"],
        "pending_reconcile"
    );
    assert!(!inspected
        .to_string()
        .contains("OPAQUE_SESSION_MUST_NOT_LEAK"));

    let events = service.call_mcp_tool(
        "studio.agent_run.events",
        json!({"deployment_id": "deploy-mcp", "limit": 100}),
    );
    assert_eq!(events["isError"], false, "{events:#}");
    assert!(events["structuredContent"]["events"]
        .as_array()
        .expect("event array")
        .iter()
        .all(|event| event.get("payload").is_none()));
    assert!(!events.to_string().contains("PENDING_PROMPT_MUST_NOT_LEAK"));

    let health = service.call_mcp_tool("studio.agent_run.health", json!({}));
    let repeated_health = service.call_mcp_tool("studio.agent_run.health", json!({}));
    assert_eq!(health["isError"], false, "{health:#}");
    assert_eq!(repeated_health["isError"], false, "{repeated_health:#}");
    assert_ne!(repeated_health["duplicate"], true, "{repeated_health:#}");
    assert_eq!(
        health["structuredContent"]["pendingReconcileDeploymentIds"],
        json!(["deploy-mcp"])
    );
    assert!(!health.to_string().contains("OPAQUE_SESSION_MUST_NOT_LEAK"));

    let no_ack = service.call_mcp_tool(
        "studio.agent_run.recover",
        json!({
            "deployment_id": "deploy-mcp",
            "idempotency_key": "agent-mcp-recover-no-ack",
            "authority_context": authority("live")
        }),
    );
    assert_eq!(no_ack["isError"], true, "{no_ack:#}");
    assert_eq!(code(&no_ack), "agent_recovery_ack_required");
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(RUNS_NS, "run-pending")
            .expect("pending run")
            .expect("pending run value")["run"]["state"],
        "pending_reconcile"
    );

    let recovery = service.call_mcp_tool(
        "studio.agent_run.recover",
        json!({
            "deployment_id": "deploy-mcp",
            "acknowledge_reconciled": true,
            "idempotency_key": "agent-mcp-recover-1",
            "authority_context": authority("live")
        }),
    );
    assert_eq!(recovery["isError"], false, "{recovery:#}");
    assert_eq!(
        recovery["structuredContent"]["receipt"]["outcome"],
        "reconciled"
    );
    assert_eq!(recovery["structuredContent"]["orderReplay"], false);
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(RUNS_NS, "run-pending")
            .expect("reconciled run")
            .expect("reconciled run value")["run"]["state"],
        "reconciled"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json(ACTIVE_RUNS_NS, "deploy-mcp")
            .expect("active state")
            .expect("active state value")["state"],
        "reconciled"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json(RUNS_NS)
            .expect("run list")
            .len(),
        1
    );
    let event_count = service
        .runtime()
        .events
        .replay("agent-deployment:deploy-mcp", 0, 100)
        .expect("lifecycle events")
        .len();

    let duplicate = service.call_mcp_tool(
        "studio.agent_run.recover",
        json!({
            "deployment_id": "deploy-mcp",
            "acknowledge_reconciled": true,
            "idempotency_key": "agent-mcp-recover-1",
            "authority_context": authority("live")
        }),
    );
    assert_eq!(duplicate["isError"], false, "{duplicate:#}");
    assert_eq!(duplicate["duplicate"], true, "{duplicate:#}");
    assert_eq!(
        service
            .runtime()
            .events
            .replay("agent-deployment:deploy-mcp", 0, 100)
            .expect("lifecycle events")
            .len(),
        event_count
    );
}

#[test]
fn agent_lifecycle_mcp_schemas_expose_explicit_authority_and_recovery_acknowledgement() {
    let definitions = mcp::tool_definitions();
    let definitions = definitions.as_array().expect("tool definitions");
    for (name, required, read_only) in [
        (
            "studio.deployment.create",
            json!(["deployment", "idempotency_key", "authority_context"]),
            false,
        ),
        (
            "studio.deployment.start",
            json!(["deployment_id", "idempotency_key", "authority_context"]),
            false,
        ),
        (
            "studio.deployment.pause",
            json!(["deployment_id", "idempotency_key", "authority_context"]),
            false,
        ),
        (
            "studio.deployment.stop",
            json!(["deployment_id", "idempotency_key", "authority_context"]),
            false,
        ),
        ("studio.deployment.inspect", json!(null), true),
        ("studio.agent_run.inspect", json!(["run_id"]), true),
        ("studio.agent_run.events", json!(["deployment_id"]), true),
        ("studio.agent_run.health", json!(null), true),
        (
            "studio.agent_run.recover",
            json!([
                "deployment_id",
                "acknowledge_reconciled",
                "idempotency_key",
                "authority_context"
            ]),
            false,
        ),
    ] {
        let definition = definitions
            .iter()
            .find(|definition| definition["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(
            definition["annotations"]["readOnlyHint"], read_only,
            "{name}"
        );
        assert_eq!(
            definition["inputSchema"]["additionalProperties"], false,
            "{name}"
        );
        if !required.is_null() {
            assert_eq!(definition["inputSchema"]["required"], required, "{name}");
        }
    }
}

#[test]
fn runner_scoped_mcp_tool_discovery_exposes_only_the_deployment_allowlist() {
    let definitions = mcp::tool_definitions_for_agent_execution_context(&[
        "studio.deployment.inspect".to_string(),
        "studio.execution.external_receipt.append".to_string(),
    ]);
    let names = definitions
        .as_array()
        .expect("tool definitions")
        .iter()
        .filter_map(|definition| definition["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "studio.deployment.inspect",
            "studio.execution.external_receipt.append"
        ]
    );
    assert!(!names.contains(&"studio.agent_run.health"));
}
