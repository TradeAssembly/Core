use super::{ServiceResponse, TradeAssemblyService};
use crate::agent_runner::VerifiedAgentMcpExecutionContext;
use crate::local_live_authorization::{
    check_local_live_mandate, issue_local_live_mandate, revoke_local_live_mandate,
    LocalLiveMandateBinding, LocalLiveMandateError, LocalLiveMandateRequest, VerifiedLiveActor,
};
use crate::local_owner_identity::LocalOwnerIdentity;
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};

pub(crate) fn issue(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    if service.agent_mcp_execution_context().is_some() {
        return owner_required();
    }
    let Some(actor) = user_actor(service) else {
        return owner_required();
    };
    let Some(config_id) = string(&body, &["configId", "config_id"]) else {
        return ServiceResponse::bad_request("execution_config_required");
    };
    let Some(expires_at_ms) = body
        .get("expiresAtMs")
        .or_else(|| body.get("expires_at_ms"))
        .and_then(Value::as_i64)
    else {
        return ServiceResponse::bad_request("live_mandate_expiry_required");
    };
    let Some(key) = string(&body, &["idempotencyKey", "idempotency_key"]) else {
        return ServiceResponse::bad_request("idempotency_key_required");
    };
    let Some(config) = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .ok()
        .flatten()
    else {
        return ServiceResponse::not_found("execution_config");
    };
    let binding = match binding(service, &config) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let delegate = match body
        .get("delegateDeploymentId")
        .or_else(|| body.get("delegate_deployment_id"))
    {
        Some(id) => match string_value(id) {
            Some(id) => match agent_delegate(service, id, &binding) {
                Ok(actor) => Some(actor),
                Err(response) => return response,
            },
            None => return ServiceResponse::bad_request("delegate_deployment_invalid"),
        },
        None => None,
    };
    let now = match trusted_now(service) {
        Ok(now) => now,
        Err(response) => return response,
    };
    let request = LocalLiveMandateRequest {
        binding,
        issued_at_ms: now,
        expires_at_ms,
        delegate,
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: actor.subject.clone(),
            surface: "live_mandate".into(),
            account_mode: "live".into(),
        },
        IdempotencyKey::new(key).expect("checked nonempty"),
    );
    match issue_local_live_mandate(
        service.runtime().storage.as_ref(),
        &context,
        request,
        &actor,
        now,
    ) {
        Ok(mandate) => ServiceResponse::created(json!({"ok": true, "mandate": mandate})),
        Err(error) => mandate_error(error),
    }
}

pub(crate) fn revoke(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    if service.agent_mcp_execution_context().is_some() {
        return owner_required();
    }
    let Some(actor) = user_actor(service) else {
        return owner_required();
    };
    let Some(id) = string(&body, &["mandateId", "mandate_id"]) else {
        return ServiceResponse::bad_request("live_mandate_id_required");
    };
    let Some(key) = string(&body, &["idempotencyKey", "idempotency_key"]) else {
        return ServiceResponse::bad_request("idempotency_key_required");
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: actor.subject.clone(),
            surface: "live_mandate".into(),
            account_mode: "live".into(),
        },
        IdempotencyKey::new(key).expect("checked nonempty"),
    );
    match revoke_local_live_mandate(
        service.runtime().storage.as_ref(),
        &context,
        id,
        &actor,
        match trusted_now(service) {
            Ok(now) => now,
            Err(response) => return response,
        },
    ) {
        Ok(()) => ServiceResponse::ok(json!({"ok": true, "mandateId": id, "revoked": true})),
        Err(error) => mandate_error(error),
    }
}

pub(crate) fn status(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    if let Some(context) = service.agent_mcp_execution_context() {
        if crate::agent_runner::revalidate_mcp_execution_context(&service.runtime(), context)
            .is_err()
        {
            return ServiceResponse::forbidden("agent_mcp_execution_context_invalid");
        }
    }
    let Some(id) = string(&body, &["mandateId", "mandate_id"]) else {
        return ServiceResponse::bad_request("live_mandate_id_required");
    };
    let Some(value) = service
        .runtime()
        .storage
        .get_json("local_live_mandates", id)
        .ok()
        .flatten()
    else {
        return ServiceResponse::not_found("live_mandate");
    };
    let Ok(mandate) =
        serde_json::from_value::<crate::local_live_authorization::LocalLiveMandate>(value)
    else {
        return ServiceResponse::internal_error("corrupt mandate");
    };
    let Some(config) = service
        .runtime()
        .storage
        .get_json("execution_configs", &mandate.binding.execution_config_id)
        .ok()
        .flatten()
    else {
        return ServiceResponse::forbidden("live_mandate_stale_binding");
    };
    let Ok(current_binding) = binding(service, &config) else {
        return ServiceResponse::forbidden("live_mandate_stale_binding");
    };
    if current_binding != mandate.binding {
        return ServiceResponse::forbidden("live_mandate_stale_binding");
    }
    let actor = if let Some(context) = service.agent_mcp_execution_context() {
        agent_actor(service, context)
    } else {
        user_actor(service)
    };
    let Some(actor) = actor else {
        return owner_required();
    };
    match check_local_live_mandate(
        service.runtime().storage.as_ref(),
        id,
        &mandate.binding,
        &actor,
        match trusted_now(service) {
            Ok(now) => now,
            Err(response) => return response,
        },
    ) {
        Ok(mandate) => ServiceResponse::ok(json!({"ok": true, "mandate": mandate})),
        Err(error) => mandate_error(error),
    }
}

/// Capture only a mandate that was revalidated for the current authenticated
/// actor and this exact saved config. The record is persisted with activation,
/// not accepted from caller-authored authority JSON.
pub(crate) fn activation_authority(
    service: &TradeAssemblyService,
    config: &Value,
    body: &Value,
) -> Result<Value, ServiceResponse> {
    let Some(id) = string(body, &["localLiveMandateId", "local_live_mandate_id"]) else {
        return Err(ServiceResponse::bad_request("local_live_mandate_required"));
    };
    let checked = status(service, json!({"mandateId":id}));
    if checked.status != 200 {
        return Err(checked);
    }
    let mandate = &checked.body["mandate"];
    if mandate["binding"]["executionConfigId"] != config["configId"] {
        return Err(ServiceResponse::forbidden("live_mandate_config_mismatch"));
    }
    let actor = match service.agent_mcp_execution_context() {
        Some(context) => agent_actor(service, context),
        None => user_actor(service),
    }
    .ok_or_else(owner_required)?;
    Ok(json!({
        "schemaVersion":"tradeassembly.local_live_activation_authority.v1",
        "mandateId":id,
        "mandateDigest":mandate["digest"],
        "actor":actor,
    }))
}

fn binding(
    service: &TradeAssemblyService,
    config: &Value,
) -> Result<LocalLiveMandateBinding, ServiceResponse> {
    let config_id = config["configId"]
        .as_str()
        .or_else(|| config["id"].as_str())
        .unwrap_or_default();
    let config =
        super::execution::authorized_activation_config(service, &json!({"configId": config_id}))?;
    let binding = binding_from_config(service, &config)?;
    super::plugin_lifecycle::require_instance(service, &binding.plugin_instance_ref)?;
    Ok(binding)
}

fn binding_from_config(
    service: &TradeAssemblyService,
    config: &Value,
) -> Result<LocalLiveMandateBinding, ServiceResponse> {
    let config_id = config["configId"].as_str().unwrap_or_default();
    let mode = config["mode"]
        .as_str()
        .or_else(|| config["accountMode"].as_str())
        .unwrap_or_default();
    if mode != "live" {
        return Err(ServiceResponse::bad_request("live_config_required"));
    }
    let strategy_version = config["strategyVersionId"]
        .as_str()
        .or_else(|| config["version_id"].as_str())
        .unwrap_or_default()
        .to_string();
    let revision_id = config["capabilityGraphRevisionId"]
        .as_str()
        .unwrap_or_default();
    let evaluation_epoch =
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(trusted_now(service)?)
            .ok_or_else(|| ServiceResponse::internal_error("trusted_clock_invalid"))?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let revision = super::capability_graph::revision_for_context(
        service,
        revision_id,
        config["strategyId"].as_str().unwrap_or_default(),
        strategy_version.as_str(),
        "live",
        &evaluation_epoch,
    )
    .map_err(|error| ServiceResponse::bad_request_with_details("capability_graph_stale", json!({
        "revisionId":revision_id, "blockers":error["blockers"],
        "nextAction":{"tool":"tradeassembly.execution.readiness","arguments":{"config_id":config_id}}
    })))?;
    let selected =
        super::execution::selected_capability_value(&revision["graph"], "broker.order_submit.live")
            .ok_or_else(|| ServiceResponse::bad_request("live_capability_required"))?;
    let instance_ref = selected["pluginInstanceRef"].as_str().unwrap_or_default();
    let instance = service
        .runtime()
        .plugins
        .get_instance(instance_ref)
        .map_err(|_| ServiceResponse::internal_error("live_plugin_storage_unavailable"))?
        .ok_or_else(|| ServiceResponse::not_found("plugin_instance"))?;
    let instance = super::plugin_lifecycle::with_live_status(service, instance);
    if !super::plugin_lifecycle::live_health_evidence_current(&instance) {
        return Err(ServiceResponse::bad_request_with_details(
            "live_plugin_health_stale",
            json!({"nextAction":"verify_live_plugin"}),
        ));
    }
    let owner = LocalOwnerIdentity::for_database(service.db())
        .map_err(|_| ServiceResponse::internal_error("local_owner_unavailable"))?;
    crate::local_live_authorization::binding_from_observed_state(
        config, &revision, selected, &instance, &owner,
    )
    .map_err(|code| {
        ServiceResponse::bad_request_with_details(
            code,
            json!({"nextAction":"complete_live_readiness"}),
        )
    })
}

/// Internal scheduled work exercises the actor captured at activation. It must
/// not synthesize a user identity or trust authority supplied with a new tick.
pub(crate) fn revalidate_run_authority(
    service: &TradeAssemblyService,
    run: &Value,
) -> Result<(), ServiceResponse> {
    let config_id = string(run, &["configId"])
        .ok_or_else(|| ServiceResponse::forbidden("live_mandate_stale_binding"))?;
    let config = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .map_err(|_| ServiceResponse::internal_error("live_mandate_storage_failure"))?
        .ok_or_else(|| ServiceResponse::forbidden("live_mandate_stale_binding"))?;
    let current = binding_from_config(service, &config)?;
    crate::local_live_authorization::check_persisted_run_authority(
        service.runtime().storage.as_ref(),
        run,
        &config,
        &current,
        trusted_now(service)?,
    )
    .map(|_| ())
    .map_err(|error| match error {
        crate::local_live_authorization::RunAuthorityError::Invalid(code) => {
            ServiceResponse::forbidden(code)
        }
        crate::local_live_authorization::RunAuthorityError::Storage => {
            ServiceResponse::internal_error("live_delegate_storage_unavailable")
        }
        crate::local_live_authorization::RunAuthorityError::Mandate(error) => mandate_error(error),
    })
}

fn agent_delegate(
    service: &TradeAssemblyService,
    deployment_id: &str,
    binding: &LocalLiveMandateBinding,
) -> Result<VerifiedLiveActor, ServiceResponse> {
    let deployments = crate::agent_runner::deployments(&service.runtime())
        .map_err(|_| ServiceResponse::bad_request("delegate_deployment_invalid"))?;
    let deployment = deployments
        .into_iter()
        .find(|d| d.deployment_id == deployment_id)
        .ok_or_else(|| ServiceResponse::not_found("delegate_deployment"))?;
    if deployment.mode != "live"
        || deployment.execution_config_version_id != binding.execution_config_id
    {
        return Err(ServiceResponse::bad_request("delegate_binding_mismatch"));
    }
    agent_identity(
        service,
        deployment_id,
        &crate::agent_runner::deployment_binding_digest(&deployment),
    )
}

pub(super) fn verify_external_delegate(
    service: &TradeAssemblyService,
    deployment_id: &str,
    config: &Value,
    activation: &Value,
) -> Result<(), ServiceResponse> {
    let mandate_id = activation["localLiveAuthority"]["mandateId"]
        .as_str()
        .ok_or_else(|| ServiceResponse::forbidden("local_live_mandate_required"))?;
    // Check the authenticated owner and current complete config binding first.
    let checked = status(service, json!({"mandateId":mandate_id}));
    if checked.status != 200 {
        return Err(checked);
    }
    let current_binding = binding(service, config)?;
    let delegate = agent_delegate(service, deployment_id, &current_binding)?;
    let mandate = check_local_live_mandate(
        service.runtime().storage.as_ref(),
        mandate_id,
        &current_binding,
        &delegate,
        trusted_now(service)?,
    )
    .map_err(mandate_error)?;
    if activation["localLiveAuthority"]["mandateDigest"] != mandate.digest {
        return Err(ServiceResponse::forbidden("live_mandate_stale_binding"));
    }
    Ok(())
}

fn agent_actor(
    service: &TradeAssemblyService,
    context: &VerifiedAgentMcpExecutionContext,
) -> Option<VerifiedLiveActor> {
    agent_identity(service, context.deployment_id(), context.binding_digest()).ok()
}
fn agent_identity(
    service: &TradeAssemblyService,
    deployment_id: &str,
    binding_digest: &str,
) -> Result<VerifiedLiveActor, ServiceResponse> {
    let owner = LocalOwnerIdentity::for_database(service.db())
        .map_err(|_| ServiceResponse::internal_error("local_owner_unavailable"))?;
    Ok(crate::local_live_authorization::deployment_actor(
        &owner.stable_identity_id,
        deployment_id,
        binding_digest,
    ))
}
fn user_actor(service: &TradeAssemblyService) -> Option<VerifiedLiveActor> {
    let owner = LocalOwnerIdentity::for_database(service.db()).ok()?;
    let (issuer, subject) = service.responsible_human_identity()?;
    (issuer == owner.issuer && subject == owner.subject).then(|| VerifiedLiveActor {
        actor_kind: "user".into(),
        issuer: issuer.into(),
        subject: subject.into(),
    })
}
fn owner_required() -> ServiceResponse {
    ServiceResponse::forbidden_with_details(
        "live_mandate_owner_required",
        json!({"nextAction":{"surface":"cli","command":["tradeassembly","execution","live-mandate","--help"],"requires":"verified installation owner"}}),
    )
}
fn trusted_now(service: &TradeAssemblyService) -> Result<i64, ServiceResponse> {
    service
        .runtime()
        .clock
        .trusted_now_ms()
        .map_err(|_| ServiceResponse::internal_error("trusted_clock_unavailable"))
}
fn string<'a>(body: &'a Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|name| {
        body.get(*name)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    })
}
fn string_value(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.trim().is_empty())
}
fn mandate_error(error: LocalLiveMandateError) -> ServiceResponse {
    match error {
        LocalLiveMandateError::NotOwner
        | LocalLiveMandateError::AgentNotAllowed
        | LocalLiveMandateError::ActorNotAllowed => {
            ServiceResponse::forbidden("live_mandate_actor_denied")
        }
        LocalLiveMandateError::Conflict => ServiceResponse::conflict("live_mandate_conflict"),
        LocalLiveMandateError::Missing => ServiceResponse::not_found("live_mandate"),
        LocalLiveMandateError::Expired => ServiceResponse::forbidden("live_mandate_expired"),
        LocalLiveMandateError::Revoked => ServiceResponse::forbidden("live_mandate_revoked"),
        LocalLiveMandateError::NotYetValid | LocalLiveMandateError::Invalid(_) => {
            ServiceResponse::bad_request("live_mandate_invalid")
        }
        LocalLiveMandateError::Storage(_) | LocalLiveMandateError::Corrupt => {
            ServiceResponse::internal_error("live_mandate_storage_failure")
        }
    }
}
