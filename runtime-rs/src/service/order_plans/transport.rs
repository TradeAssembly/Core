//! Closed host construction of V2 requests from owned plans and active runs.
use super::*;
use crate::broker_submission::v2;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmissionCommand {
    pub plan_id: String,
    pub activation_id: String,
    pub attempt_id: String,
    pub broker_requirement_ref: String,
}
impl SubmissionCommand {
    pub fn validate(&self) -> Result<(), String> {
        if [
            &self.plan_id,
            &self.activation_id,
            &self.attempt_id,
            &self.broker_requirement_ref,
        ]
        .iter()
        .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err("broker_v2_command_invalid".into());
        }
        Ok(())
    }
}
pub(crate) fn request(
    s: &TradeAssemblyService,
    c: &SubmissionCommand,
) -> Result<
    (
        submission::OrderPlanSource,
        PluginOperationRequest,
        SideEffectContext,
    ),
    String,
> {
    c.validate()?;
    let source = submission::source(s, &c.plan_id)?;
    let p = source.historical()?;
    let key = IdempotencyKey::new(format!("broker-v2:{}", p.plan_id))?;
    let ctx = SideEffectContext::new(
        AuthorityContext {
            actor: s
                .invocation_owner()
                .ok_or("broker_v2_owner_required")?
                .subject,
            surface: "broker_boundary_v2".into(),
            account_mode: "paper".into(),
        },
        key,
    )
    .with_agent_execution(s.agent_mcp_execution_context());
    if let Some(state) = source.state()? {
        let saved: PluginOperationRequest =
            serde_json::from_value(state.intent["originalRequest"].clone())
                .map_err(|_| "broker_v2_original_request_invalid")?;
        if saved.activation_id != c.activation_id
            || saved.attempt_id != c.attempt_id
            || state.intent["brokerRequirementRef"] != c.broker_requirement_ref
        {
            return Err("broker_v2_original_command_conflict".into());
        }
        return Ok((source, saved, ctx));
    }
    let revision = s
        .runtime()
        .storage
        .get_json(
            "capability_graph_revisions",
            &p.command.capability_graph_revision,
        )?
        .ok_or("broker_v2_graph_missing")?;
    let node = revision["graph"]["nodes"]
        .as_array()
        .and_then(|nodes| {
            nodes.iter().find(|n| {
                n["nodeId"] == c.broker_requirement_ref
                    && n["selected"]["operation"]["capability"] == v2::SUBMIT_CAPABILITY
            })
        })
        .ok_or("broker_v2_node_not_selected")?;
    let selected = &node["selected"];
    if selected["operationId"] != v2::SUBMIT_OPERATION {
        return Err("broker_v2_operation_unsupported".into());
    }
    let attempt = s
        .runtime()
        .storage
        .get_json("execution_attempts", &c.attempt_id)?
        .ok_or("execution_attempts_record_missing")?;
    let input = tradeassembly_plugin_sdk::broker_v2::SubmitRequest {
        schema_version: "tradeassembly.broker_submit.v2".into(),
        plan_id: p.plan_id.clone(),
        canonical_account_sha256: p
            .canonical_account
            .ok_or("broker_v2_canonical_plan_required")?
            .identity_sha256,
        order: p.order,
    };
    input.validate()?;
    let r = PluginOperationRequest {
        correlation_id: format!("broker-v2:{}", v2::key_hash(&p.plan_id)),
        plugin_instance_ref: selected["pluginInstanceRef"]
            .as_str()
            .ok_or("broker_v2_binding_invalid")?
            .into(),
        plugin_ref: selected["pluginRef"]
            .as_str()
            .ok_or("broker_v2_binding_invalid")?
            .into(),
        manifest_fingerprint: selected["manifestFingerprint"]
            .as_str()
            .ok_or("broker_v2_binding_invalid")?
            .into(),
        operation_id: v2::SUBMIT_OPERATION.into(),
        capability: v2::SUBMIT_CAPABILITY.into(),
        capability_graph_revision_id: p.command.capability_graph_revision,
        capability_graph_fingerprint: revision["graphFingerprint"]
            .as_str()
            .ok_or("broker_v2_binding_invalid")?
            .into(),
        strategy_id: p.command.scope.strategy_id,
        strategy_version_id: p.command.scope.strategy_version_id,
        strategy_spec_hash: p.command.scope.strategy_spec_hash,
        activation_id: c.activation_id.clone(),
        attempt_id: c.attempt_id.clone(),
        evaluation_tick_id: format!("order-plan:{}", p.plan_id),
        mode: "paper".into(),
        purpose: "paper_trading".into(),
        account_ref: selected["accountRef"].as_str().map(str::to_string),
        timeout_ms: 15000,
        fencing_token: attempt["fencingToken"].as_i64(),
        input: serde_json::to_value(input).map_err(|_| "broker_v2_input_invalid")?,
        evidence_refs: vec![
            format!("order-plan:{}", p.plan_id),
            format!("broker-node:{}", c.broker_requirement_ref),
        ],
    };
    Ok((source, r, ctx))
}
pub(crate) fn submit(s: &TradeAssemblyService, c: SubmissionCommand) -> Result<Value, String> {
    let (source, request, ctx) = request(s, &c)?;
    let response = s
        .runtime()
        .plugin_operations
        .invoke_order_v2(&request, &ctx, &source)?;
    Ok(
        json!({"planId":c.plan_id,"receipt":response,"submission":source.state()?.map(submission::summary),"executionAuthorityGranted":false,"brokerDispatchAllowed":false}),
    )
}
pub(crate) fn status(s: &TradeAssemblyService, id: &str) -> Result<Value, String> {
    let source = submission::source(s, id)?;
    let p = source.historical()?;
    Ok(
        json!({"planId":p.plan_id,"entryId":p.entry_id,"canonicalAccountSha256":p.canonical_account.as_ref().map(|a|a.identity_sha256.as_str()),"cashReserved":held(s,&p)?,"release":released(s,&p)?,"submission":source.state()?.map(submission::summary),"executionAuthorityGranted":false,"brokerDispatchAllowed":false}),
    )
}

pub(crate) fn reconcile(
    s: &TradeAssemblyService,
    id: &str,
    outer_key: &str,
) -> Result<Value, String> {
    let source = submission::source(s, id)?;
    let ctx = SideEffectContext::new(
        AuthorityContext {
            actor: s
                .invocation_owner()
                .ok_or("broker_v2_owner_required")?
                .subject,
            surface: "broker_recovery_v2".into(),
            account_mode: "paper".into(),
        },
        IdempotencyKey::new(format!(
            "broker-reconcile-v2:{}",
            v2::key_hash(&format!("{id}:{outer_key}"))
        ))?,
    )
    .with_agent_execution(s.agent_mcp_execution_context());
    let result = s
        .runtime()
        .plugin_operations
        .reconcile_order_v2(&source, &ctx)?;
    Ok(
        json!({"planId":id,"recovery":result,"submission":source.state()?.map(submission::summary),"executionAuthorityGranted":false,"brokerDispatchAllowed":false}),
    )
}
