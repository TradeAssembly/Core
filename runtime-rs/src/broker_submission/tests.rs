#[path = "../../tests/common/live_plugin_fixture.rs"]
mod live_plugin_fixture;

use super::{
    attempt_fencing_matches, controls_blocked, load_current_state, BrokerExecutionProvenance,
    BrokerSubmissionDependencies,
};
use crate as runtime_crate;
use crate::agent_runner::{self, AgentDeployment, AgentRun, AgentRuntimePort};
use crate::local_live_authorization::deployment_actor;
use crate::local_owner_identity::LocalOwnerIdentity;
use crate::ports::{AuthorityContext, IdempotencyKey, PluginOperationRequest, SideEffectContext};
use crate::service::TradeAssemblyService;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

struct ContextCapturingAdapter {
    runtime: TradeAssemblyService,
    context: Mutex<Option<agent_runner::VerifiedAgentMcpExecutionContext>>,
    request: PluginOperationRequest,
    deps: BrokerSubmissionDependencies,
    ordinary_context: SideEffectContext,
    loader_result: Mutex<Option<Result<BrokerExecutionProvenance, String>>>,
}

impl AgentRuntimePort for ContextCapturingAdapter {
    fn start_or_resume(
        &self,
        run: &AgentRun,
        _trigger: &serde_json::Value,
    ) -> Result<Option<String>, String> {
        Ok(Some(format!("codex-session:{}", run.run_id)))
    }

    fn start_or_resume_with_mcp_execution_context(
        &self,
        run: &AgentRun,
        _trigger: &serde_json::Value,
        capability: &agent_runner::AgentMcpExecutionCapability,
    ) -> Result<Option<String>, String> {
        let verified = capability.verified_context(&self.runtime.runtime())?;
        let typed_context = self
            .ordinary_context
            .clone()
            .with_agent_execution(Some(&verified));
        *self.loader_result.lock().unwrap() = Some(
            load_current_state(&self.deps, &self.request, &typed_context)
                .map(|state| state.provenance),
        );
        *self.context.lock().unwrap() = Some(verified);
        Ok(Some(format!("codex-session:{}", run.run_id)))
    }
}

#[test]
fn order_symbol_requires_exact_nonempty_execution_binding() {
    for config in [json!({}), json!({"symbol":""}), json!({"symbol":"  "})] {
        assert_eq!(
            super::validate_order_symbol(&config, "SPY"),
            Err("execution_symbol_missing".into())
        );
    }
    let config = json!({"symbol":"BTC/USD"});
    assert!(super::validate_order_symbol(&config, "BTC/USD").is_ok());
    for symbol in ["SPY", "BTCUSD", "btc/usd", "BTC/USD "] {
        assert_eq!(
            super::validate_order_symbol(&config, symbol),
            Err("order_symbol_outside_execution_config".into())
        );
    }
}

#[test]
fn controls_fail_closed_for_pause_stop_and_emergency_actions() {
    for controls in [
        json!({"pauseEntries": "paused"}),
        json!({"lastControl": {"action": "stop"}}),
        json!({"lastControl": {"action": "emergency_stop"}}),
        json!({"newEntriesPaused": true}),
        json!({"stop": "stopped"}),
        json!({"emergencyStop": "engaged"}),
    ] {
        assert!(controls_blocked(&controls));
    }
    assert!(!controls_blocked(&json!({"pauseEntries": "available"})));
}

#[test]
fn request_must_carry_the_current_attempt_fencing_token() {
    let mut request: PluginOperationRequest = serde_json::from_value(json!({"correlationId":"c", "pluginInstanceRef":"i", "pluginRef":"p", "manifestFingerprint":"m", "operationId":"o", "capability":"cap", "capabilityGraphRevisionId":"r", "capabilityGraphFingerprint":"f", "strategyId":"s", "strategyVersionId":"v", "strategySpecHash":"h", "activationId":"a", "attemptId":"t", "evaluationTickId":"e", "mode":"live", "purpose":"live_order_submission", "timeoutMs":1, "input":{}})).expect("request fixture");
    let attempt = json!({"fencingToken": 7});
    assert!(!attempt_fencing_matches(&request, &attempt));
    request.fencing_token = Some(7);
    assert!(attempt_fencing_matches(&request, &attempt));
    request.fencing_token = Some(8);
    assert!(!attempt_fencing_matches(&request, &attempt));
}

#[test]
fn loads_agent_state_only_from_named_verified_provenance() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let db = root.join("runtime.db");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    let owner = LocalOwnerIdentity::for_database(&db).unwrap();
    let service = base.for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
    live_plugin_fixture::install_live_metadata_fixture(&service, &root);
    let saved = service.handle_http("POST", "/product/strategy-execution-configs/save", json!({"strategyId":"strat_local_btc_demo","mode":"live","providerRef":"mandate-live","accountRef":"account://mandate-live/controlled"}));
    assert_eq!(saved.status, 200, "{saved:#?}");
    let config_id = saved.body["body"]["item"]["configId"].as_str().unwrap();
    let deployment = AgentDeployment {
        executor: Default::default(),
        deployment_id: "loader-agent".into(),
        system_project_id: "system-1".into(),
        agent_definition_version_id: "agent-v1".into(),
        execution_config_version_id: config_id.into(),
        studio_tool_allowlist: vec!["studio.deployment.inspect".into()],
        desired_state: "active".into(),
        interval_seconds: 60,
        cron_utc: None,
        mode: "live".into(),
        prompt: "inspect supplied redacted trigger".into(),
        workspace: root.display().to_string(),
        runtime_profile: "local-read-only".into(),
    };
    agent_runner::put_deployment(&service.runtime(), &deployment).unwrap();
    let issued = service.handle_http("POST", "/product/live-mandates/issue", json!({"configId":config_id,"expiresAtMs":service.runtime().clock.now_ms()+600_000,"delegateDeploymentId":deployment.deployment_id,"idempotencyKey":"loader-agent-mandate"}));
    assert_eq!(issued.status, 201, "{issued:#?}");
    let mandate = issued.body["mandate"].clone();
    let activation_id = "loader-agent-activation";
    let mut run = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .unwrap()
        .unwrap();
    run["kind"] = json!("ExecutionRun");
    run["state"] = json!("active");
    run["activationId"] = json!(activation_id);
    let binding_digest = agent_runner::deployment_binding_digest(&deployment);
    let agent_actor = deployment_actor(
        &owner.stable_identity_id,
        &deployment.deployment_id,
        &binding_digest,
    );
    run["localLiveAuthority"] = json!({"schemaVersion":"tradeassembly.local_live_activation_authority.v1","mandateId":mandate["mandateId"],"mandateDigest":mandate["digest"],"actor":agent_actor});
    run["correlationId"] = json!("agent-run-correlation");
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("loader-agent-state").unwrap(),
    );
    for (namespace, key) in [
        ("execution_runs", format!("run_{activation_id}")),
        ("execution_activations", activation_id.into()),
    ] {
        service
            .runtime()
            .storage
            .put_json(namespace, &key, run.clone(), &context)
            .unwrap();
    }
    service.runtime().storage.put_json("execution_controls", activation_id, json!({"activationId":activation_id,"pauseEntries":"available","stop":"available","emergencyStop":"available","newEntriesPaused":false}), &context).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            "execution_reconciliation",
            activation_id,
            json!({"activationId":activation_id,"state":"clear","newEntriesPaused":false}),
            &context,
        )
        .unwrap();
    let revision = service
        .runtime()
        .storage
        .get_json(
            "capability_graph_revisions",
            run["capabilityGraphRevisionId"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
    let node = revision["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["selected"].is_object()
                && n["requirement"]["capability"]
                    .as_str()
                    .unwrap()
                    .starts_with("broker.order_submit")
        })
        .unwrap();
    let selected = &node["selected"];
    let request = PluginOperationRequest {
        correlation_id: "agent-request-correlation".into(),
        plugin_instance_ref: selected["pluginInstanceRef"].as_str().unwrap().into(),
        plugin_ref: selected["pluginRef"].as_str().unwrap().into(),
        manifest_fingerprint: selected["manifestFingerprint"].as_str().unwrap().into(),
        operation_id: selected["operationId"].as_str().unwrap().into(),
        capability: node["requirement"]["capability"].as_str().unwrap().into(),
        capability_graph_revision_id: run["capabilityGraphRevisionId"].as_str().unwrap().into(),
        capability_graph_fingerprint: run["capabilityGraphFingerprint"].as_str().unwrap().into(),
        strategy_id: run["strategyId"].as_str().unwrap().into(),
        strategy_version_id: run["strategyVersionId"].as_str().unwrap().into(),
        strategy_spec_hash: run["strategySpecHash"].as_str().unwrap().into(),
        activation_id: activation_id.into(),
        attempt_id: "not-used-for-agent".into(),
        evaluation_tick_id: "agent-tick-not-scheduled".into(),
        mode: "live".into(),
        purpose: "live_order_submission".into(),
        account_ref: selected["accountRef"].as_str().map(str::to_string),
        timeout_ms: 1000,
        fencing_token: None,
        input: json!({}),
        evidence_refs: vec![],
    };
    let deps = BrokerSubmissionDependencies {
        storage: Arc::clone(&service.runtime().storage),
        plugins: Arc::clone(&service.runtime().plugins),
        plugin_packages: Arc::clone(&service.runtime().plugin_packages),
        credentials: Arc::clone(&service.runtime().credentials),
        capability_resolver: Arc::clone(&service.runtime().capability_resolver),
        clock: Arc::clone(&service.runtime().clock),
        leases: Arc::clone(&service.runtime().leases),
        owner: owner.clone(),
    };
    let adapter = ContextCapturingAdapter {
        runtime: service.clone(),
        context: Mutex::new(None),
        request,
        deps,
        ordinary_context: context,
        loader_result: Mutex::new(None),
    };
    let result = agent_runner::supervise_once(
        &service.runtime(),
        &deployment.deployment_id,
        service.runtime().clock.now_ms(),
        &adapter,
    )
    .unwrap();
    assert_eq!(result[0]["outcome"], "completed", "{result:#?}");
    let state = adapter
        .loader_result
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .unwrap();
    assert!(matches!(state, BrokerExecutionProvenance::Agent { .. }));
    let verified = adapter.context.lock().unwrap().clone().unwrap();
    let typed_context = adapter
        .ordinary_context
        .clone()
        .with_agent_execution(Some(&verified));
    assert_eq!(
        load_current_state(&adapter.deps, &adapter.request, &typed_context).unwrap_err(),
        "agent_mcp_execution_context_invalid"
    );
}
