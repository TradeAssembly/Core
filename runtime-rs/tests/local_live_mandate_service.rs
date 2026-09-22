use clap::Parser;
use serde_json::{json, Value};
use tradeassembly_runtime::agent_runner::{
    self, AgentDeployment, AgentMcpExecutionCapability, AgentRun, AgentRuntimePort,
};
use tradeassembly_runtime::local_owner_identity::LocalOwnerIdentity;
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

#[path = "common/live_plugin_fixture.rs"]
mod live_plugin_fixture;
use tradeassembly_runtime as runtime_crate;

#[test]
fn mandate_routes_fail_closed_without_verified_owner_or_config() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("runtime.db");
    let service = TradeAssemblyService::test_local(db.to_string_lossy());
    let anonymous = service.handle_http(
        "POST",
        "/product/live-mandates/issue",
        json!({"configId":"missing","expiresAtMs":9_999_999_999_i64,"idempotencyKey":"r1"}),
    );
    assert_eq!(anonymous.status, 403);
    assert_eq!(
        anonymous.body["error"]["code"],
        "live_mandate_owner_required"
    );
}

#[test]
fn mandate_mcp_tools_are_discoverable_and_status_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let service = TradeAssemblyService::test_local(dir.path().join("runtime.db").to_string_lossy());
    let response = service.call_mcp_tool(
        "tradeassembly.execution.live_mandate.status",
        json!({"mandateId":"missing"}),
    );
    assert_eq!(
        response["structuredContent"]["error"]["code"],
        "route_not_found"
    );
}

#[test]
fn owner_issue_flow_uses_production_config_route() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let db = root.join("runtime.db");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    let owner = LocalOwnerIdentity::for_database(&db).unwrap();
    let service = base.for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
    let seeded = service.handle_http("POST", "/demo/reset", json!({}));
    assert!(seeded.status < 500, "{seeded:#?}");
    live_plugin_fixture::install_live_metadata_fixture(&service, &root);
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
        "strategyId":"strat_local_btc_demo", "mode":"live", "providerRef":"mandate-live",
        "accountRef":"account://mandate-live/controlled"
        }),
    );
    assert_eq!(saved.status, 200, "{saved:#?}");
    let config_id = saved.body["body"]["item"]["configId"]
        .as_str()
        .unwrap_or_default();
    let request = json!({
        "configId": config_id, "expiresAtMs": service.runtime().clock.now_ms() + 600_000, "idempotencyKey":"mandate-positive-1"
    });
    let issued = service.handle_http("POST", "/product/live-mandates/issue", request.clone());
    assert_eq!(saved.body["body"]["item"]["mode"], "live");
    let revision = service
        .runtime()
        .storage
        .get_json(
            "capability_graph_revisions",
            saved.body["body"]["item"]["capabilityGraphRevisionId"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
        .unwrap();
    let blockers: Vec<_> = revision["graph"]["nodes"].as_array().unwrap().iter()
        .filter(|node| node["blockers"].as_array().is_some_and(|items| !items.is_empty()))
        .map(|node| json!({"requirement":node["requirement"]["capability"],"blockers":node["blockers"]})).collect();
    assert_eq!(
        issued.status, 201,
        "{issued:#?}; graph blockers={blockers:?}"
    );
    let mandate = &issued.body["mandate"];
    let id = mandate["mandateId"].as_str().expect("mandate ID");
    let status = service.handle_http(
        "POST",
        "/product/live-mandates/status",
        json!({"mandateId":id}),
    );
    assert_eq!(status.status, 200, "{status:#?}");
    assert_eq!(status.body["mandate"], *mandate);
    let bound_readiness = service.call_mcp_tool(
        "tradeassembly.execution.readiness",
        json!({
            "config_id":config_id, "local_live_mandate_id":id,
            "localLiveAuthority":{"actor":{"subject":"spoofed"},"mandateDigest":"spoofed"}
        }),
    );
    let captured = &bound_readiness["structuredContent"]["body"]["localLiveAuthority"];
    assert_eq!(captured["mandateId"], id);
    assert_eq!(captured["mandateDigest"], mandate["digest"]);
    assert_eq!(captured["actor"]["subject"], owner.subject);
    assert_eq!(captured["actor"]["issuer"], owner.issuer);
    assert_eq!(captured["actor"]["actorKind"], "user");
    let missing_readiness = service.call_mcp_tool(
        "tradeassembly.execution.readiness",
        json!({
            "config_id":config_id, "local_live_mandate_id":"nonexistent",
            "localLiveAuthority":captured
        }),
    );
    let missing = &missing_readiness["structuredContent"]["body"];
    assert_eq!(missing["ready"], false);
    assert_eq!(missing["localLiveMandateError"]["code"], "route_not_found");
    assert!(missing["localLiveAuthority"].is_null());
    let retry = service.handle_http("POST", "/product/live-mandates/issue", request.clone());
    assert_eq!(retry.body["mandate"], *mandate, "{retry:#?}");
    let mut changed = request.clone();
    changed["expiresAtMs"] = json!(request["expiresAtMs"].as_i64().unwrap() + 1);
    let conflict = service.handle_http("POST", "/product/live-mandates/issue", changed);
    assert_eq!(conflict.status, 409, "{conflict:#?}");
    let mcp = service.call_mcp_tool(
        "tradeassembly.execution.live_mandate.status",
        json!({"mandateId":id}),
    );
    assert_eq!(mcp["structuredContent"]["mandate"], *mandate, "{mcp:#?}");
    let cli = tradeassembly_runtime::cli::Cli::try_parse_from([
        "tradeassembly",
        "execution",
        "live-mandate",
        "status",
        "--mandate-id",
        id,
    ])
    .unwrap();
    let cli_status = tradeassembly_runtime::cli::execute_command(&service, cli.command);
    assert_eq!(cli_status["mandate"], *mandate, "{cli_status:#?}");
    let original = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .unwrap()
        .unwrap();
    let mut edited = original.clone();
    edited["riskConfig"] = json!({"changed":true});
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: "fixture".into(),
            surface: "test".into(),
            account_mode: "live".into(),
        },
        IdempotencyKey::new("fixture-config-mutation").unwrap(),
    );
    service
        .runtime()
        .storage
        .put_json("execution_configs", config_id, edited, &context)
        .unwrap();
    let stale = service.handle_http(
        "POST",
        "/product/live-mandates/status",
        json!({"mandateId":id}),
    );
    assert_eq!(
        stale.body["error"]["code"], "live_mandate_stale_binding",
        "{stale:#?}"
    );
    service
        .runtime()
        .storage
        .put_json("execution_configs", config_id, original, &context)
        .unwrap();
    let stranger = base.for_authenticated_invocation(&owner.issuer, "another-owner", None, None);
    let mut stranger_request = request.clone();
    stranger_request["idempotencyKey"] = json!("stranger-issue");
    let denied = stranger.handle_http("POST", "/product/live-mandates/issue", stranger_request);
    assert_eq!(denied.status, 403, "{denied:#?}");
    let mut spoof = request;
    spoof["idempotencyKey"] = json!("spoof-issue");
    spoof["authorityContext"] =
        json!({"issuer":owner.issuer,"subject":owner.subject,"actor":"user"});
    let denied = base.handle_http("POST", "/product/live-mandates/issue", spoof);
    assert_eq!(denied.status, 403, "{denied:#?}");
    let deployment = AgentDeployment {
        executor: Default::default(),
        deployment_id: "mandate-runner".into(),
        system_project_id: "controlled-system".into(),
        agent_definition_version_id: "controlled-agent-v1".into(),
        execution_config_version_id: config_id.into(),
        studio_tool_allowlist: ["status", "issue", "revoke"]
            .map(|action| format!("tradeassembly.execution.live_mandate.{action}"))
            .to_vec(),
        desired_state: "active".into(),
        interval_seconds: 60,
        cron_utc: None,
        mode: "live".into(),
        prompt: "Inspect the supplied approval record.".into(),
        workspace: root.to_string_lossy().into_owned(),
        runtime_profile: "local".into(),
    };
    agent_runner::put_deployment(&service.runtime(), &deployment).unwrap();
    let delegated = service.handle_http(
        "POST",
        "/product/live-mandates/issue",
        json!({
            "configId": config_id, "expiresAtMs":service.runtime().clock.now_ms()+600_000,
            "delegateDeploymentId":deployment.deployment_id,"idempotencyKey":"delegated-issue"
        }),
    );
    assert_eq!(delegated.status, 201, "{delegated:#?}");
    let adapter = ApprovalAdapter {
        service: service.clone(),
        mandate_id: delegated.body["mandate"]["mandateId"]
            .as_str()
            .unwrap()
            .into(),
        config_id: config_id.into(),
        allowed: true,
        bound: std::sync::Mutex::new(None),
    };
    let receipts = agent_runner::supervise_once(
        &service.runtime(),
        "controlled-runner",
        service.runtime().clock.now_ms(),
        &adapter,
    )
    .unwrap();
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    let completed = adapter
        .bound
        .lock()
        .unwrap()
        .take()
        .expect("adapter executed");
    let ended = completed.call_mcp_tool(
        "tradeassembly.execution.live_mandate.status",
        json!({"mandateId":adapter.mandate_id}),
    );
    assert_eq!(ended["isError"], true, "{ended}");
    assert!(ended
        .to_string()
        .contains("agent_mcp_execution_context_invalid"));
    let mut other_deployment = deployment.clone();
    other_deployment.deployment_id = "other-mandate-runner".into();
    agent_runner::put_deployment(&service.runtime(), &other_deployment).unwrap();
    let other_adapter = ApprovalAdapter {
        service: service.clone(),
        mandate_id: adapter.mandate_id.clone(),
        config_id: config_id.into(),
        allowed: false,
        bound: std::sync::Mutex::new(None),
    };
    agent_runner::supervise_once(
        &service.runtime(),
        "controlled-other-runner",
        service.runtime().clock.now_ms(),
        &other_adapter,
    )
    .unwrap();
    assert!(
        other_adapter.bound.lock().unwrap().is_some(),
        "other adapter ran"
    );
    let current_instance = service
        .runtime()
        .plugins
        .get_instance("mandate-live")
        .unwrap()
        .unwrap();
    let mut changed_credentials = current_instance.clone();
    changed_credentials["credentialRevision"] = json!(2);
    service
        .runtime()
        .plugins
        .put_instance("mandate-live", changed_credentials, &context)
        .unwrap();
    let stale_credentials = service.handle_http(
        "POST",
        "/product/live-mandates/status",
        json!({"mandateId":id}),
    );
    assert_eq!(
        stale_credentials.body["error"]["code"], "live_mandate_stale_binding",
        "{stale_credentials:#?}"
    );
    service
        .runtime()
        .plugins
        .put_instance("mandate-live", current_instance, &context)
        .unwrap();
    // Controlled persisted activation fixture only: no activation command,
    // scheduler start, model call or broker operation is performed here.
    let config = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .unwrap()
        .unwrap();
    let mut fixture_run = config.clone();
    fixture_run["kind"] = json!("ExecutionRun");
    fixture_run["state"] = json!("active");
    fixture_run["activationId"] = json!("mandate-evaluation-fixture");
    fixture_run["localLiveAuthority"] = captured.clone();
    service
        .runtime()
        .storage
        .put_json(
            "execution_runs",
            "run_mandate-evaluation-fixture",
            fixture_run.clone(),
            &context,
        )
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            "execution_activations",
            "mandate-evaluation-fixture",
            fixture_run,
            &context,
        )
        .unwrap();
    let scope = service
        .runtime()
        .object_authorization
        .scope("execution_config", config_id)
        .unwrap()
        .unwrap();
    service
        .runtime()
        .object_authorization
        .bind(
            &tradeassembly_runtime::ports::ObjectScope {
                object_type: "execution_activation".into(),
                object_id: "mandate-evaluation-fixture".into(),
                owner: scope.owner,
                parent_type: Some("execution_config".into()),
                parent_id: Some(config_id.into()),
            },
            &context,
        )
        .unwrap();
    let before_revoke = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/mandate-evaluation-fixture/ticks/evaluate",
        json!({"idempotencyKey":"probe-valid-authority"}),
    );
    assert_eq!(
        before_revoke.body["error"]["details"]["reason"], "tick_id_required",
        "{before_revoke:#?}"
    );
    let set_run_authority = |authority: Value| {
        for (namespace, key) in [
            ("execution_runs", "run_mandate-evaluation-fixture"),
            ("execution_activations", "mandate-evaluation-fixture"),
        ] {
            let mut record = service
                .runtime()
                .storage
                .get_json(namespace, key)
                .unwrap()
                .unwrap();
            record["localLiveAuthority"] = authority.clone();
            service
                .runtime()
                .storage
                .put_json(namespace, key, record, &context)
                .unwrap();
        }
    };
    set_run_authority(json!({
        "schemaVersion":"tradeassembly.local_live_activation_authority.v1",
        "mandateId":delegated.body["mandate"]["mandateId"],
        "mandateDigest":delegated.body["mandate"]["digest"],
        "actor":delegated.body["mandate"]["delegate"],
    }));
    let delegated_tick = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/mandate-evaluation-fixture/ticks/evaluate",
        json!({"idempotencyKey":"probe-persisted-delegate"}),
    );
    assert_eq!(
        delegated_tick.body["error"]["details"]["reason"], "tick_id_required",
        "{delegated_tick:#?}"
    );
    let mut updated_deployment = deployment.clone();
    updated_deployment.agent_definition_version_id = "controlled-agent-v2".into();
    assert_eq!(
        agent_runner::put_deployment(&service.runtime(), &updated_deployment).unwrap_err(),
        "agent_deployment_binding_immutable"
    );
    // Simulate corrupt/replaced durable state after proving the normal API
    // itself refuses to mutate a bound deployment.
    let original_deployment = service
        .runtime()
        .storage
        .get_json(agent_runner::DEPLOYMENTS_NS, &deployment.deployment_id)
        .unwrap()
        .unwrap();
    let mut replaced = original_deployment.clone();
    replaced["deployment"]["agentDefinitionVersionId"] = json!("controlled-agent-v2");
    service
        .runtime()
        .storage
        .put_json(
            agent_runner::DEPLOYMENTS_NS,
            &deployment.deployment_id,
            replaced,
            &context,
        )
        .unwrap();
    let changed_delegate = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/mandate-evaluation-fixture/ticks/evaluate",
        json!({"idempotencyKey":"probe-changed-delegate"}),
    );
    assert_eq!(
        changed_delegate.body["error"]["code"], "live_mandate_delegate_stale",
        "{changed_delegate:#?}"
    );
    service
        .runtime()
        .storage
        .put_json(
            agent_runner::DEPLOYMENTS_NS,
            &deployment.deployment_id,
            original_deployment,
            &context,
        )
        .unwrap();
    set_run_authority(captured.clone());
    let revoked = service.handle_http(
        "POST",
        "/product/live-mandates/revoke",
        json!({"mandateId":id,"idempotencyKey":"revoke-1"}),
    );
    assert_eq!(revoked.status, 200, "{revoked:#?}");
    let after_revoke = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/mandate-evaluation-fixture/ticks/evaluate",
        json!({"idempotencyKey":"probe-revoked-authority"}),
    );
    assert_eq!(
        after_revoke.body["error"]["code"], "live_mandate_revoked",
        "{after_revoke:#?}"
    );
    let status = service.handle_http(
        "POST",
        "/product/live-mandates/status",
        json!({"mandateId":id}),
    );
    assert_eq!(
        status.body["error"]["code"], "live_mandate_revoked",
        "{status:#?}"
    );
}

struct ApprovalAdapter {
    service: TradeAssemblyService,
    mandate_id: String,
    config_id: String,
    allowed: bool,
    bound: std::sync::Mutex<Option<TradeAssemblyService>>,
}

impl AgentRuntimePort for ApprovalAdapter {
    fn start_or_resume(&self, _: &AgentRun, _: &Value) -> Result<Option<String>, String> {
        panic!("supervisor must supply its verified execution capability")
    }

    fn start_or_resume_with_mcp_execution_context(
        &self,
        _: &AgentRun,
        _: &Value,
        capability: &AgentMcpExecutionCapability,
    ) -> Result<Option<String>, String> {
        let verified = capability.verified_context(&self.service.runtime())?;
        let bound = self
            .service
            .with_verified_agent_mcp_execution_context(verified);
        let status = bound.call_mcp_tool(
            "tradeassembly.execution.live_mandate.status",
            json!({"mandateId":self.mandate_id}),
        );
        if !self.allowed {
            assert_eq!(
                status["structuredContent"]["error"]["code"], "live_mandate_actor_denied",
                "{status}"
            );
            *self.bound.lock().unwrap() = Some(bound);
            return Ok(Some("controlled-other-session".into()));
        }
        assert_eq!(status["isError"], false, "{status}");
        assert_eq!(
            status["structuredContent"]["mandate"]["mandateId"],
            self.mandate_id
        );
        for (action, body) in [
            (
                "issue",
                json!({"configId":self.config_id,"expiresAtMs":self.service.runtime().clock.now_ms()+600_000,"idempotencyKey":"agent-issue"}),
            ),
            (
                "revoke",
                json!({"mandateId":self.mandate_id,"idempotencyKey":"agent-revoke"}),
            ),
        ] {
            let denied = bound.call_mcp_tool(
                &format!("tradeassembly.execution.live_mandate.{action}"),
                body,
            );
            assert_eq!(
                denied["structuredContent"]["error"]["code"], "live_mandate_owner_required",
                "{denied}"
            );
        }
        *self.bound.lock().unwrap() = Some(bound);
        Ok(Some("controlled-session".into()))
    }
}

// A real installed package/manifest/instance with controlled health metadata.
// Its dummy binary is deliberately never executed: this proves the approval
// service path, not broker connectivity, policy-sidecar behavior or trading.
