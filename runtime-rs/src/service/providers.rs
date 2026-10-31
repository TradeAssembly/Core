use super::{plugin_lifecycle, ServiceResponse, TradeAssemblyService};
use crate::capability::{
    local_full_entitlements, CAPABILITY_RESOLUTION_SCHEMA_VERSION, LOCAL_FULL_PROFILE,
};
use crate::instrument_packs;
use crate::plugin_catalog::stored_plugin_entry as portable_stored_plugin_entry;
use crate::ports::{
    AuthorityContext, CapabilityRequirement, CapabilityRequirementConstraints,
    CapabilityResolverCatalog, CredentialResolutionStatus, EntitlementGrant, IdempotencyKey,
    PluginCatalogEntry, PluginOperationContract, PluginOperationRequest, PluginOperationTraits,
    SideEffectContext,
};
use serde_json::{json, Value};

const ENTITLEMENT_NS: &str = "entitlement_grants";

pub(crate) fn list(service: &TradeAssemblyService) -> Vec<Value> {
    plugin_catalog(service)
        .into_iter()
        .map(|plugin| plugin_value(&plugin))
        .collect()
}

pub(crate) fn connect_workspace(service: &TradeAssemblyService) -> Value {
    let plugins = plugin_catalog(service);
    let credential_status = plugins
        .iter()
        .map(|plugin| {
            (
                plugin.instance_ref.clone(),
                serde_json::to_value(&plugin.credential_status).unwrap_or_else(|_| json!({})),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "workspacePrimitive": "plugins",
        "plugins": plugins.iter().map(plugin_value).collect::<Vec<_>>(),
        "providers": plugins.iter().map(plugin_value).collect::<Vec<_>>(),
        "credentialStatus": credential_status,
        "providerHealth": plugins.iter().map(|plugin| (plugin.provider_ref.clone(), json!({"status": plugin.health, "mode": "paper"}))).collect::<serde_json::Map<_, _>>(),
        "localCredentialBackends": [{"kind": "local-file", "scope": "workspace", "summary": "customer-managed"}],
        "capabilityResolution": resolve_capability(service, requirement_from_body(&json!({"capability": "broker.order_submit.paper", "mode": "paper", "assetClass": "crypto", "pluginRef": "tradeassembly.simbroker"}))),
    })
}

pub(crate) fn capability_matrix(
    service: &TradeAssemblyService,
    requested_ref: &str,
    manifest: Option<Value>,
) -> Value {
    let Some(plugin) = plugin_catalog(service)
        .into_iter()
        .find(|plugin| plugin.plugin_ref == requested_ref || plugin.provider_ref == requested_ref)
    else {
        return json!({
            "ok": false,
            "ref": requested_ref,
            "pluginRef": requested_ref,
            "providerRef": requested_ref,
            "capabilities": [],
            "compatible": false,
            "blocked": true,
            "blockers": [{"code": "missing_plugin", "message": "No installed plugin matches this ref."}],
            "capabilityResolution": resolve_capability(service, requirement_from_body(&json!({
                "capability": "plugin.capability_matrix",
                "mode": "paper",
                "pluginRef": requested_ref,
            }))),
        });
    };
    let plugin_ref = plugin.plugin_ref.clone();
    let provider_ref = plugin.provider_ref.clone();
    let capabilities = plugin
        .capabilities
        .iter()
        .map(|capability| json!(capability))
        .collect::<Vec<_>>();
    let resolution = resolve_capability(
        service,
        requirement_from_body(&json!({
            "capability": "plugin.capability_matrix",
            "operation": "plugin.capability_matrix",
            "mode": "paper",
            "pluginRef": plugin_ref.clone(),
        })),
    );
    let resolution_ok = resolution
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let compatibility = manifest.map(|manifest| {
        instrument_packs::provider_pack_compatibility_report(
            &manifest,
            &[plugin_value(&plugin)],
            &[],
            None,
            86_400,
        )
    });
    let compatibility_ok = compatibility
        .as_ref()
        .and_then(|report| report.get("ok"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut blockers = resolution
        .get("blockers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !compatibility_ok {
        blockers.push(json!({
            "code": "instrument_pack_incompatible",
            "message": "Instrument pack compatibility checks did not pass for this plugin."
        }));
    }
    json!({
        "ok": resolution_ok && compatibility_ok,
        "ref": plugin_ref,
        "pluginRef": plugin_ref,
        "providerRef": provider_ref,
        "capabilities": capabilities,
        "compatible": resolution_ok && compatibility_ok,
        "blocked": !(resolution_ok && compatibility_ok),
        "blockers": blockers,
        "compatibility": compatibility,
        "capabilityResolution": resolution,
    })
}

pub(crate) fn capability_matrix_ref_required() -> Value {
    json!({
        "ok": false,
        "workspacePrimitive": "plugins",
        "capabilities": [],
        "operations": [],
        "compatible": false,
        "blockers": [{
            "code": "ref_required",
            "message": "Capability matrix requires an explicit pluginRef, providerRef, or ref."
        }],
        "noSilentFallback": true,
        "noAdvice": true,
    })
}

pub(crate) fn capabilities(provider_ref: &str, service: &TradeAssemblyService) -> Vec<Value> {
    let mut capabilities = service
        .runtime()
        .providers
        .capabilities(provider_ref)
        .into_iter()
        .filter(|capability| capability.available)
        .map(|capability| json!(capability.capability))
        .collect::<Vec<_>>();
    match provider_ref {
        "core-runtime" => capabilities.extend(
            [
                json!("calendar.session.resolve@1"),
                json!("market_data.bars.read@1"),
                json!("expression.cel.evaluate@1"),
                json!("indicator.bars.normalize@1"),
                json!("indicator.stateful.calculate@1"),
                json!("order.intent.build@1"),
                json!("order.option.submit@1"),
                json!("order.option_combo.submit@1"),
                json!("plugin.capability_matrix"),
            ]
            .into_iter()
            .chain(
                crate::capability::PORTFOLIO_RESEARCH_OPERATIONS
                    .iter()
                    .map(|(capability, _)| json!(capability)),
            ),
        ),
        "sim" => capabilities.extend([
            json!("broker.paper"),
            json!("broker.order_submit"),
            json!("marketdata.bars"),
            json!("marketdata.quote"),
            json!("market_data.bars.read@1"),
            json!("plugin.capability_matrix"),
        ]),
        "local-data" => capabilities.extend([
            json!("marketdata.stub"),
            json!("marketdata.bars"),
            json!("marketdata.quote"),
            json!("marketdata.options_chain"),
            json!("market_data.bars.read@1"),
            json!("plugin.capability_matrix"),
            json!("research.local"),
        ]),
        _ => capabilities.extend([
            json!("broker.paper"),
            json!("marketdata.crypto"),
            json!("marketdata.quote"),
            json!("marketdata.bars"),
            json!("market_data.bars.read@1"),
            json!("marketdata.options_chain"),
            json!("plugin.capability_matrix"),
        ]),
    }
    capabilities.sort_by_key(|value| value.as_str().unwrap_or_default().to_string());
    capabilities.dedup();
    capabilities
}

pub(crate) fn resolve_capability(
    service: &TradeAssemblyService,
    requirement: CapabilityRequirement,
) -> Value {
    if requirement.capability.trim().is_empty() {
        return json!({
            "schemaVersion": CAPABILITY_RESOLUTION_SCHEMA_VERSION,
            "ok": false,
            "state": "blocked",
            "requirement": requirement,
            "profile": {"id": LOCAL_FULL_PROFILE, "source": "local", "default": true},
            "candidates": [],
            "blockers": [{"code": "capability_required", "message": "Capability resolution requires an explicit capability."}],
            "entitlementDecisions": [],
            "noSilentFallback": true,
            "noAdvice": true,
        });
    }
    let catalog = CapabilityResolverCatalog {
        plugins: plugin_catalog(service),
        entitlements: entitlements(service),
    };
    serde_json::to_value(
        service
            .runtime()
            .capability_resolver
            .resolve(requirement, catalog),
    )
    .unwrap_or_else(|_| json!({"ok": false, "error": "capability_resolution_serialize_failed"}))
}

pub(crate) fn requirement_from_body(body: &Value) -> CapabilityRequirement {
    let capability = body
        .get("capability")
        .or_else(|| body.get("capabilityRef"))
        .or_else(|| body.get("capability_ref"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    CapabilityRequirement {
        requirement_id: body
            .get("requirementId")
            .or_else(|| body.get("requirement_id"))
            .and_then(Value::as_str)
            .unwrap_or("req-local")
            .to_string(),
        capability,
        mode: body
            .get("mode")
            .or_else(|| body.get("accountMode"))
            .or_else(|| body.get("account_mode"))
            .and_then(Value::as_str)
            .unwrap_or("paper")
            .to_string(),
        declaration: string_field(body, &["declaration"]).unwrap_or_else(|| "required".to_string()),
        required_for: string_array_field(body, &["requiredFor", "required_for"]),
        stage_refs: string_array_field(body, &["stageRefs", "stage_refs"]),
        substep_refs: string_array_field(body, &["substepRefs", "substep_refs"]),
        constraints: requirement_constraints(body),
        dependency_refs: string_array_field(body, &["dependencyRefs", "dependency_refs"]),
        fallback_policy: string_field(body, &["fallbackPolicy", "fallback_policy"])
            .unwrap_or_else(|| "none".to_string()),
        policy_tags: string_array_field(body, &["policyTags", "policy_tags"]),
        asset_class: body
            .get("assetClass")
            .or_else(|| body.get("asset_class"))
            .or_else(|| body.get("instrumentType"))
            .or_else(|| body.get("instrument_type"))
            .and_then(Value::as_str)
            .map(str::to_string),
        instrument_family: body
            .get("instrumentFamily")
            .or_else(|| body.get("instrument_family"))
            .and_then(Value::as_str)
            .map(str::to_string),
        strategy_id: body
            .get("strategyId")
            .or_else(|| body.get("strategy_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        strategy_version_id: string_field(body, &["strategyVersionId", "strategy_version_id"]),
        plugin_instance_ref: string_field(
            body,
            &[
                "pluginInstanceRef",
                "plugin_instance_ref",
                "instanceRef",
                "instance_ref",
            ],
        ),
        plugin_ref: body
            .get("pluginRef")
            .or_else(|| body.get("plugin_ref"))
            .or_else(|| body.get("providerRef"))
            .or_else(|| body.get("provider_ref"))
            .and_then(Value::as_str)
            .map(str::to_string),
        operation_id: body
            .get("operation")
            .or_else(|| body.get("operationId"))
            .or_else(|| body.get("operation_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        account_ref: body
            .get("accountRef")
            .or_else(|| body.get("account_ref"))
            .and_then(Value::as_str)
            .map(str::to_string),
        purpose: body
            .get("purpose")
            .or_else(|| body.get("purposeCode"))
            .or_else(|| body.get("purpose_code"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn requirement_constraints(body: &Value) -> CapabilityRequirementConstraints {
    let constraints = body.get("constraints").unwrap_or(&Value::Null);
    CapabilityRequirementConstraints {
        instrument_families: string_array_field(
            constraints,
            &["instrumentFamilies", "instrument_families"],
        ),
        data_shapes: string_array_field(constraints, &["dataShapes", "data_shapes"]),
        operations: string_array_field(constraints, &["operations"]),
        fields: string_array_field(constraints, &["fields"]),
        input_paths: string_array_field(constraints, &["inputPaths", "input_paths"]),
        schema_refs: string_array_field(constraints, &["schemaRefs", "schema_refs"]),
        timeframe: string_field(constraints, &["timeframe"]),
        max_freshness: string_field(constraints, &["maxFreshness", "max_freshness"]),
        max_latency_ms: value_field(constraints, &["maxLatencyMs", "max_latency_ms"])
            .and_then(Value::as_u64),
        deterministic: value_field(constraints, &["deterministic"])
            .and_then(Value::as_bool)
            .unwrap_or(false),
        replayable: value_field(constraints, &["replayable"])
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn value_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key))
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    value_field(value, keys)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn string_array_field(value: &Value, keys: &[&str]) -> Vec<String> {
    value_field(value, keys)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

pub(crate) fn deny_entitlement(service: &TradeAssemblyService, body: Value) -> Value {
    let Some(capability) = explicit_capability_from(&body) else {
        return entitlement_error(
            "capability_required",
            "Entitlement changes require an explicit capability.",
        );
    };
    persist_entitlement(
        service,
        EntitlementGrant {
            grant_id: entitlement_id("deny", &capability),
            profile: LOCAL_FULL_PROFILE.to_string(),
            source: "local_override".to_string(),
            capability,
            effect: "deny".to_string(),
            state: "active".to_string(),
            scope: body
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("global")
                .to_string(),
            source_type: "local_override".to_string(),
            source_ref: Some("tradeassembly.local_admin".to_string()),
            precedence: body
                .get("precedence")
                .and_then(Value::as_i64)
                .unwrap_or(100),
            limits: body
                .get("limits")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            issued_at: Some("2026-07-08T00:00:00Z".to_string()),
            expires_at: body
                .get("expiresAt")
                .or_else(|| body.get("expires_at"))
                .and_then(Value::as_str)
                .map(str::to_string),
            revoked_at: None,
            reason: body
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some("explicit local deny".to_string())),
        },
    )
}

pub(crate) fn grant_entitlement(_service: &TradeAssemblyService, body: Value) -> Value {
    let Some(capability) = explicit_capability_from(&body) else {
        return entitlement_error(
            "capability_required",
            "Entitlement changes require an explicit capability.",
        );
    };
    entitlement_error(
        "entitlement_grant_unavailable",
        &format!(
            "Public runtime does not accept allow grants for {capability}; configure a host-owned entitlement store."
        ),
    )
}

fn persist_entitlement(service: &TradeAssemblyService, grant: EntitlementGrant) -> Value {
    let context = context("entitlement.updated", &grant.grant_id);
    service
        .runtime()
        .storage
        .put_json(
            ENTITLEMENT_NS,
            &grant.grant_id,
            serde_json::to_value(&grant).expect("serialize entitlement grant"),
            &context,
        )
        .expect("persist entitlement grant");
    json!({"ok": true, "grant": grant})
}

pub(crate) fn entitlements(service: &TradeAssemblyService) -> Vec<EntitlementGrant> {
    let mut grants = local_full_entitlements();
    if let Ok(stored) = service.runtime().storage.list_json(ENTITLEMENT_NS) {
        grants.extend(
            stored
                .into_iter()
                .filter_map(|(_, value)| serde_json::from_value::<EntitlementGrant>(value).ok()),
        );
    }
    grants
}

pub(crate) fn list_entitlements(service: &TradeAssemblyService) -> Value {
    json!({
        "profile": {"id": LOCAL_FULL_PROFILE, "source": "local", "default": true},
        "grants": entitlements(service),
        "allowGrantsHostOwned": true,
    })
}

pub(crate) fn revoke_entitlement(service: &TradeAssemblyService, grant_id: &str) -> Value {
    let Ok(Some(value)) = service.runtime().storage.get_json(ENTITLEMENT_NS, grant_id) else {
        return entitlement_error(
            "entitlement_override_not_found",
            "Only durable local entitlement overrides can be revoked.",
        );
    };
    let Ok(mut grant) = serde_json::from_value::<EntitlementGrant>(value) else {
        return entitlement_error(
            "entitlement_override_invalid",
            "The stored entitlement override is invalid.",
        );
    };
    grant.state = "revoked".to_string();
    grant.revoked_at = Some(format!("epoch-ms:{}", service.runtime().clock.now_ms()));
    let context = context("entitlement.revoked", grant_id);
    match service.runtime().storage.put_json(
        ENTITLEMENT_NS,
        grant_id,
        serde_json::to_value(&grant).expect("serialize revoked entitlement"),
        &context,
    ) {
        Ok(()) => json!({"ok": true, "grant": grant}),
        Err(_) => entitlement_error(
            "entitlement_revoke_failed",
            "The entitlement override could not be revoked.",
        ),
    }
}

pub(crate) fn invoke_operation(
    service: &TradeAssemblyService,
    path: &str,
    body: Value,
) -> ServiceResponse {
    invoke_operation_with_health_policy(service, path, body, false)
}

pub(crate) fn invoke_health_operation(
    service: &TradeAssemblyService,
    path: &str,
    body: Value,
) -> ServiceResponse {
    invoke_operation_with_health_policy(service, path, body, true)
}

fn invoke_operation_with_health_policy(
    service: &TradeAssemblyService,
    path: &str,
    body: Value,
    allow_degraded_health_probe: bool,
) -> ServiceResponse {
    let Some((instance_ref, operation_id)) = plugin_operation_from_path(path) else {
        return ServiceResponse::bad_request_with_details(
            "plugin_operation_path_invalid",
            json!({"path": path}),
        );
    };
    let Some(plugin) = plugin_catalog(service)
        .into_iter()
        .find(|plugin| plugin.instance_ref == instance_ref)
    else {
        return ServiceResponse::bad_request_with_details(
            "plugin_instance_not_found",
            json!({"instanceRef": instance_ref, "path": path}),
        );
    };
    let Some(operation) = plugin
        .operations
        .iter()
        .find(|operation| operation.id == operation_id)
        .cloned()
    else {
        return ServiceResponse::bad_request_with_details(
            "plugin_operation_not_declared",
            json!({"pluginRef": plugin.plugin_ref, "operationId": operation_id}),
        );
    };
    let health_probe = allow_degraded_health_probe
        && operation.id == "account.health"
        && plugin.health == "degraded";
    if !health_probe
        && !matches!(plugin.health.as_str(), "ok" | "ready" | "healthy")
        && !crate::capability::setup_discovery_allowed(&operation, &plugin.health)
    {
        return ServiceResponse::bad_request_with_details(
            "plugin_invocation_blocked",
            json!({
                "pluginRef": plugin.plugin_ref,
                "providerRef": plugin.provider_ref,
                "operationId": operation.id,
                "blockers": [{
                    "code": "plugin_unhealthy",
                    "message": "Plugin health does not permit invocation.",
                    "subject": plugin.instance_ref,
                }],
            }),
        );
    }

    let mut requirement_body = body.clone();
    if let Some(map) = requirement_body.as_object_mut() {
        map.insert("capability".to_string(), json!(operation.capability));
        map.insert("pluginRef".to_string(), json!(plugin.plugin_ref));
        map.insert("pluginInstanceRef".to_string(), json!(plugin.instance_ref));
        map.insert("operation".to_string(), json!(operation.id));
    } else {
        requirement_body = json!({
            "capability": operation.capability,
            "pluginRef": plugin.plugin_ref,
            "pluginInstanceRef": plugin.instance_ref,
            "operation": operation.id,
        });
    }
    let resolution = resolve_capability(service, requirement_from_body(&requirement_body));
    let candidate_allowed = resolution["candidates"]
        .as_array()
        .map(|candidates| {
            candidates.iter().any(|candidate| {
                candidate["pluginInstanceRef"] == plugin.instance_ref
                    && candidate["pluginRef"] == plugin.plugin_ref
                    && candidate["operation"]["id"] == operation.id
                    && candidate["blockers"]
                        .as_array()
                        .map(Vec::is_empty)
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    if !candidate_allowed {
        return ServiceResponse::bad_request_with_details(
            "plugin_invocation_blocked",
            json!({
                "pluginRef": plugin.plugin_ref,
                "providerRef": plugin.provider_ref,
                "operationId": operation.id,
                "resolver": resolution,
            }),
        );
    }

    let Some(idempotency_key) = string_field(&body, &["idempotencyKey", "idempotency_key"])
        .and_then(|value| IdempotencyKey::new(value).ok())
    else {
        return ServiceResponse::bad_request_with_details(
            "plugin_idempotency_key_required",
            json!({"instanceRef": plugin.instance_ref, "operationId": operation.id}),
        );
    };
    let direct_binding_id = format!("direct:{}:{}", plugin.instance_ref, operation.id);
    let correlation_id = string_field(&body, &["correlationId", "correlation_id"])
        .unwrap_or_else(|| idempotency_key.as_str().to_string());
    let request = PluginOperationRequest {
        correlation_id,
        plugin_instance_ref: plugin.instance_ref.clone(),
        plugin_ref: plugin.plugin_ref.clone(),
        manifest_fingerprint: plugin.manifest_fingerprint.clone(),
        operation_id: operation.id.clone(),
        capability: operation.capability.clone(),
        capability_graph_revision_id: string_field(
            &body,
            &["capabilityGraphRevisionId", "capability_graph_revision_id"],
        )
        .unwrap_or(direct_binding_id),
        capability_graph_fingerprint: string_field(
            &body,
            &["capabilityGraphFingerprint", "capability_graph_fingerprint"],
        )
        .unwrap_or_else(|| plugin.manifest_fingerprint.clone()),
        strategy_id: string_field(&body, &["strategyId", "strategy_id"]).unwrap_or_default(),
        strategy_version_id: string_field(&body, &["strategyVersionId", "strategy_version_id"])
            .unwrap_or_default(),
        strategy_spec_hash: string_field(&body, &["strategySpecHash", "strategy_spec_hash"])
            .unwrap_or_default(),
        activation_id: string_field(&body, &["activationId", "activation_id"]).unwrap_or_default(),
        attempt_id: string_field(&body, &["attemptId", "attempt_id"]).unwrap_or_default(),
        evaluation_tick_id: string_field(&body, &["evaluationTickId", "evaluation_tick_id"])
            .unwrap_or_default(),
        mode: string_field(&body, &["mode"]).unwrap_or_default(),
        purpose: string_field(&body, &["purpose"]).unwrap_or_else(|| operation.purpose.clone()),
        account_ref: string_field(&body, &["accountRef", "account_ref"]),
        timeout_ms: value_field(&body, &["timeoutMs", "timeout_ms"])
            .and_then(Value::as_u64)
            .unwrap_or(15_000),
        fencing_token: value_field(&body, &["fencingToken", "fencing_token"])
            .and_then(Value::as_i64),
        input: body.get("input").cloned().unwrap_or_else(|| json!({})),
        evidence_refs: string_array_field(&body, &["evidenceRefs", "evidence_refs"]),
    };
    let authority = body
        .get("authority")
        .cloned()
        .and_then(|value| serde_json::from_value::<AuthorityContext>(value).ok())
        .unwrap_or_else(AuthorityContext::local_cli);
    let context = SideEffectContext::new(authority, idempotency_key)
        .with_agent_execution(service.agent_mcp_execution_context());
    match service
        .runtime()
        .plugin_operations
        .invoke(&request, &context)
    {
        Ok(response) => ServiceResponse::ok(json!({
            "ok": true,
            "instanceRef": plugin.instance_ref,
            "pluginRef": plugin.plugin_ref,
            "operationId": operation.id,
            "binding": {
                "pluginInstanceRef": plugin.instance_ref,
                "pluginRef": plugin.plugin_ref,
                "operationId": operation.id,
                "capability": operation.capability,
                "manifestFingerprint": plugin.manifest_fingerprint,
            },
            "resolver": resolution,
            "response": response,
        })),
        Err(message) => ServiceResponse::bad_request_with_details(
            "plugin_invocation_failed",
            json!({
                "instanceRef": plugin.instance_ref,
                "pluginRef": plugin.plugin_ref,
                "operationId": operation.id,
                "reason": message,
            }),
        ),
    }
}

fn plugin_operation_from_path(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("/plugins/instances/")?;
    let (plugin_ref, operation_rest) = rest.split_once("/operations/")?;
    let operation_id = operation_rest.strip_suffix(":invoke")?;
    if plugin_ref.trim().is_empty() || operation_id.trim().is_empty() {
        return None;
    }
    Some((plugin_ref.to_string(), operation_id.to_string()))
}

pub(crate) fn plugin_catalog(service: &TradeAssemblyService) -> Vec<PluginCatalogEntry> {
    let mut catalog = builtin_plugin_catalog(service);
    if let Ok(instances) = service.runtime().plugins.list_instances() {
        for instance in instances {
            if instance
                .get("removedAtMs")
                .is_some_and(|value| !value.is_null())
            {
                continue;
            }
            let plugin_ref = string_field(&instance, &["pluginRef"]).unwrap_or_default();
            let Ok(Some(manifest)) = service.runtime().plugins.get_manifest(&plugin_ref) else {
                continue;
            };
            let instance_ref = string_field(&instance, &["instanceRef"]).unwrap_or_default();
            let credential_status = service.runtime().credentials.status(
                string_field(&instance, &["credentialRef"])
                    .as_deref()
                    .unwrap_or(&instance_ref),
            );
            let health = plugin_lifecycle::effective_health_state(service, &instance);
            let Some(entry) =
                portable_stored_plugin_entry(&instance, &manifest, &credential_status, health)
            else {
                continue;
            };
            catalog.retain(|candidate| candidate.instance_ref != entry.instance_ref);
            catalog.push(entry);
        }
    }
    catalog.sort_by(|left, right| left.instance_ref.cmp(&right.instance_ref));
    catalog
}

pub(crate) fn builtin_plugin_catalog(service: &TradeAssemblyService) -> Vec<PluginCatalogEntry> {
    vec![
        core_runtime_plugin(service),
        sim_plugin(service),
        local_data_plugin(service),
    ]
}

fn core_runtime_plugin(service: &TradeAssemblyService) -> PluginCatalogEntry {
    let provider_ref = "core-runtime";
    with_manifest_fingerprint(
        service,
        PluginCatalogEntry {
            instance_ref: provider_ref.to_string(),
            plugin_ref: "tradeassembly.core-runtime".to_string(),
            provider_ref: provider_ref.to_string(),
            account_ref: None,
            name: "TradeAssembly Core Runtime".to_string(),
            enabled: plugin_enabled(service, "tradeassembly.core-runtime", provider_ref),
            trust_level: "core".to_string(),
            manifest_fingerprint: String::new(),
            capabilities: capabilities(provider_ref, service)
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect(),
            operations: vec![
                capability_matrix_operation("local-rust", &["mcp", "graphql", "local-rust"]),
                immutable_dataset_read_operation(),
                core_strategy_operation(
                    "expression.cel.evaluate_v1",
                    "expression.cel.evaluate@1",
                    "evaluate",
                    &["strategy_envelope", "positions"],
                    &[],
                    &["positions.current"],
                    &["schema://strategy/positions@1"],
                    &["schema://strategy/envelope@1"],
                    &["equity", "crypto_spot", "option_contract"],
                    &[
                        "authoring",
                        "research",
                        "backtest",
                        "simulation",
                        "paper",
                        "live",
                    ],
                ),
                core_strategy_operation(
                    "calendar.session.resolve_v1",
                    "calendar.session.resolve@1",
                    "resolve",
                    &["event_time", "calendar"],
                    &["event_time"],
                    &["run.event_time"],
                    &["schema://runtime/event-time@1"],
                    &["schema://runtime/calendar-session@1"],
                    &[
                        "equity",
                        "crypto_spot",
                        "option_contract",
                        "future_contract",
                    ],
                    &["research", "backtest", "simulation", "paper", "live"],
                ),
                core_strategy_operation(
                    "order.intent.build_v1",
                    "order.intent.build@1",
                    "build",
                    &["order_intent"],
                    &[],
                    &[],
                    &["schema://strategy/envelope@1"],
                    &["schema://strategy/order-intent@1"],
                    &[
                        "equity",
                        "crypto_spot",
                        "option_contract",
                        "future_contract",
                    ],
                    &["research", "backtest", "paper", "live"],
                ),
                core_strategy_operation(
                    "order.option.submit_v1",
                    "order.option.submit@1",
                    "build",
                    &["order_intent"],
                    &[],
                    &[],
                    &["schema://strategy/envelope@1"],
                    &["schema://strategy/order-intent@1"],
                    &["option_contract"],
                    &["paper", "live"],
                ),
                core_strategy_operation(
                    "order.option_combo.submit_v1",
                    "order.option_combo.submit@1",
                    "build",
                    &["order_intent"],
                    &[],
                    &[],
                    &["schema://strategy/envelope@1"],
                    &["schema://strategy/order-intent@1"],
                    &["option_contract"],
                    &["paper", "live"],
                ),
                core_strategy_operation(
                    "indicator.bars.normalize_v1",
                    "indicator.bars.normalize@1",
                    "transform",
                    &["bars"],
                    &["open", "high", "low", "close", "volume"],
                    &[],
                    &["schema://market-data/bars@1"],
                    &["schema://market-data/bars@1"],
                    &["equity", "crypto_spot"],
                    &["research", "backtest", "simulation", "paper", "live"],
                ),
                core_strategy_operation(
                    "indicator.stateful.calculate_v1",
                    "indicator.stateful.calculate@1",
                    "calculate",
                    &["bars", "indicator_state"],
                    &["close"],
                    &[],
                    &["schema://market-data/bars@1"],
                    &["schema://indicator/state@1"],
                    &["equity", "crypto_spot"],
                    &["research", "backtest", "simulation", "paper", "live"],
                ),
            ]
            .into_iter()
            .chain(crate::capability::PORTFOLIO_RESEARCH_OPERATIONS.iter().map(
                |(capability, operation)| {
                    core_strategy_operation(
                        operation,
                        capability,
                        "evaluate",
                        &["strategy_envelope"],
                        &[],
                        &[],
                        &["schema://strategy/envelope@1"],
                        &["schema://strategy/envelope@1"],
                        &["equity"],
                        &["research", "backtest"],
                    )
                },
            ))
            .collect(),
            credential_status: CredentialResolutionStatus {
                configured: true,
                custody: "none".to_string(),
                status: "not_required".to_string(),
                revision_ref: None,
            },
            health: "ok".to_string(),
        },
    )
}

fn sim_plugin(service: &TradeAssemblyService) -> PluginCatalogEntry {
    let provider_ref = "sim";
    with_manifest_fingerprint(
        service,
        PluginCatalogEntry {
            instance_ref: provider_ref.to_string(),
            plugin_ref: "tradeassembly.simbroker".to_string(),
            provider_ref: provider_ref.to_string(),
            account_ref: Some("account://sim/paper".to_string()),
            name: "SimBroker".to_string(),
            enabled: plugin_enabled(service, "tradeassembly.simbroker", provider_ref),
            trust_level: "core".to_string(),
            manifest_fingerprint: String::new(),
            capabilities: capabilities(provider_ref, service)
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect(),
            operations: vec![
                capability_matrix_operation("local-rust", &["mcp", "graphql", "local-rust"]),
                marketdata_quote_operation(false),
                marketdata_bars_operation(false),
                canonical_marketdata_bars_operation(false),
                paper_preview_operation(),
                paper_submit_operation(false),
            ],
            credential_status: CredentialResolutionStatus {
                configured: true,
                custody: "local-fixture".to_string(),
                status: "stored".to_string(),
                revision_ref: None,
            },
            health: "ok".to_string(),
        },
    )
}

fn local_data_plugin(service: &TradeAssemblyService) -> PluginCatalogEntry {
    let provider_ref = "local-data";
    with_manifest_fingerprint(
        service,
        PluginCatalogEntry {
            instance_ref: provider_ref.to_string(),
            plugin_ref: "tradeassembly.local-data".to_string(),
            provider_ref: provider_ref.to_string(),
            account_ref: None,
            name: "Local Data".to_string(),
            enabled: plugin_enabled(service, "tradeassembly.local-data", provider_ref),
            trust_level: "core".to_string(),
            manifest_fingerprint: String::new(),
            capabilities: capabilities(provider_ref, service)
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect(),
            operations: vec![
                capability_matrix_operation("local-rust", &["mcp", "graphql", "local-rust"]),
                marketdata_quote_operation(false),
                marketdata_bars_operation(false),
                canonical_marketdata_bars_operation(false),
                options_chain_operation(false),
            ],
            credential_status: CredentialResolutionStatus {
                configured: true,
                custody: "local-fixture".to_string(),
                status: "stored".to_string(),
                revision_ref: None,
            },
            health: "ok".to_string(),
        },
    )
}

fn plugin_enabled(service: &TradeAssemblyService, plugin_ref: &str, provider_ref: &str) -> bool {
    if let Ok(Some(instance)) = service.runtime().plugins.get_instance(provider_ref) {
        if instance
            .get("removedAtMs")
            .is_some_and(|value| !value.is_null())
        {
            return false;
        }
        return instance
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }
    let plugin_key = format!("plugin:{plugin_ref}:enabled");
    let provider_key = format!("plugin:{provider_ref}:enabled");
    read_plugin_enabled(service, &plugin_key)
        .or_else(|| read_plugin_enabled(service, &provider_key))
        .unwrap_or(true)
}

fn read_plugin_enabled(service: &TradeAssemblyService, key: &str) -> Option<bool> {
    service
        .runtime()
        .storage
        .get_json("plugin_instances", key)
        .ok()
        .flatten()
        .and_then(|value| value.get("enabled").and_then(Value::as_bool))
}

fn capability_matrix_operation(
    protocol: &str,
    supported_protocols: &[&str],
) -> PluginOperationContract {
    operation(
        "plugin.capability_matrix",
        "plugin.capability_matrix",
        protocol,
        "plugin.capability_matrix",
        "read",
        "low",
        false,
        false,
        "plugin",
        "plugin_administration",
        &["request_hash", "plugin_manifest_hash"],
        "c1",
        &["plugin_manifest_integrity"],
        "admin_evidence",
        supported_protocols,
    )
}

fn marketdata_quote_operation(credential_required: bool) -> PluginOperationContract {
    let evidence = if credential_required {
        &[
            "request_hash",
            "credential_grant_ref",
            "plugin_manifest_hash",
        ][..]
    } else {
        &["request_hash", "plugin_manifest_hash"][..]
    };
    let check_packs = if credential_required {
        &["market_data_freshness", "credential_scope"][..]
    } else {
        &["deterministic_fixture"][..]
    };
    let protocol = if credential_required {
        "openapi"
    } else {
        "local-rust"
    };
    let supported_protocols = if credential_required {
        &[
            "mcp",
            "openapi",
            "graphql",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
        ][..]
    } else {
        &[
            "mcp",
            "graphql",
            "local-rust",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
        ][..]
    };
    operation(
        "marketdata.quote.read",
        "marketdata.quote",
        protocol,
        "market_data.read",
        "read",
        "low",
        credential_required,
        credential_required,
        "market_data",
        "market_data_research",
        evidence,
        "c2",
        check_packs,
        "read_evidence",
        supported_protocols,
    )
}

fn marketdata_bars_operation(credential_required: bool) -> PluginOperationContract {
    let evidence = if credential_required {
        &[
            "request_hash",
            "credential_grant_ref",
            "plugin_manifest_hash",
        ][..]
    } else {
        &["request_hash", "plugin_manifest_hash"][..]
    };
    let check_packs = if credential_required {
        &["market_data_freshness", "credential_scope"][..]
    } else {
        &["deterministic_fixture"][..]
    };
    let protocol = if credential_required {
        "openapi"
    } else {
        "local-rust"
    };
    let supported_protocols = if credential_required {
        &[
            "mcp",
            "openapi",
            "graphql",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
        ][..]
    } else {
        &[
            "mcp",
            "graphql",
            "local-rust",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
        ][..]
    };
    operation(
        "marketdata.bars.read",
        "marketdata.bars",
        protocol,
        "market_data.read",
        "read",
        "low",
        credential_required,
        credential_required,
        "market_data",
        "backtest_dataset_setup",
        evidence,
        "c2",
        check_packs,
        "read_evidence",
        supported_protocols,
    )
}

fn canonical_marketdata_bars_operation(credential_required: bool) -> PluginOperationContract {
    let mut operation = marketdata_bars_operation(credential_required);
    operation.id = "marketdata.bars.read_v1".to_string();
    operation.capability = "market_data.bars.read@1".to_string();
    operation.traits.modes = if credential_required {
        strings(vec!["research", "paper"])
    } else {
        strings(vec![
            "authoring",
            "research",
            "backtest",
            "simulation",
            "paper",
            "live",
        ])
    };
    operation.traits.operations = strings(vec!["read"]);
    operation.traits.instrument_families =
        strings(vec!["crypto_spot", "equity", "option_contract"]);
    operation.traits.input_paths = strings(vec!["market_data.source_bars"]);
    operation.traits.max_freshness = Some("PT5M".to_string());
    operation.supported_protocols = if credential_required {
        strings(vec![
            "mcp",
            "openapi",
            "graphql",
            "mode:research",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
            "asset:option_contract",
        ])
    } else {
        strings(vec![
            "mcp",
            "graphql",
            "local-rust",
            "mode:authoring",
            "mode:research",
            "mode:backtest",
            "mode:simulation",
            "mode:paper",
            "mode:live",
            "asset:crypto",
            "asset:equity",
            "asset:option_contract",
        ])
    };
    operation
}

#[allow(clippy::too_many_arguments)]
fn core_strategy_operation(
    id: &str,
    capability: &str,
    semantic_operation: &str,
    data_shapes: &[&str],
    fields: &[&str],
    input_paths: &[&str],
    input_schema_refs: &[&str],
    output_schema_refs: &[&str],
    instrument_families: &[&str],
    modes: &[&str],
) -> PluginOperationContract {
    let effect = if semantic_operation == "resolve" {
        "read"
    } else if semantic_operation == "build" {
        "draft"
    } else {
        "research"
    };
    let mut operation = operation(
        id,
        capability,
        "local-rust",
        "strategy.runtime.evaluate",
        effect,
        "low",
        false,
        false,
        "strategy_runtime",
        "strategy_execution",
        &[
            "request_hash",
            "strategy_version_hash",
            "plugin_manifest_hash",
        ],
        "c2",
        &["deterministic_runtime", "capability_scope"],
        "runtime_evidence",
        &["mcp", "graphql", "local-rust"],
    );
    operation.traits = PluginOperationTraits {
        modes: modes.iter().map(|value| (*value).to_string()).collect(),
        instrument_families: instrument_families
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        data_shapes: data_shapes
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        operations: vec![semantic_operation.to_string()],
        fields: fields.iter().map(|value| (*value).to_string()).collect(),
        input_paths: input_paths
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        input_schema_refs: input_schema_refs
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        output_schema_refs: output_schema_refs
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        timeframes: Vec::new(),
        max_freshness: None,
        max_latency_ms: None,
        deterministic: true,
        replayable: true,
    };
    operation.supported_protocols.extend(
        modes.iter().map(|mode| format!("mode:{mode}")).chain(
            instrument_families
                .iter()
                .map(|family| format!("asset:{family}")),
        ),
    );
    operation
}

fn immutable_dataset_read_operation() -> PluginOperationContract {
    let mut operation = core_strategy_operation(
        "dataset.snapshot.read_v1",
        "market_data.bars.read@1",
        "read",
        &["bars"],
        &["open", "high", "low", "close", "volume"],
        &["market_data.source_bars"],
        &["schema://market-data/bars@1"],
        &["schema://market-data/bars@1"],
        &["equity", "crypto_spot", "option_contract"],
        &["backtest", "simulation"],
    );
    operation.traits.timeframes = strings(vec!["*"]);
    operation.traits.max_freshness = Some("PT0S".to_string());
    operation
}

fn options_chain_operation(credential_required: bool) -> PluginOperationContract {
    let mut operation = operation(
        "marketdata.options_chain.read",
        "marketdata.options_chain",
        if credential_required {
            "openapi"
        } else {
            "local-rust"
        },
        "market_data.read",
        "read",
        "medium",
        credential_required,
        credential_required,
        "market_data",
        "strategy_research",
        if credential_required {
            &[
                "request_hash",
                "credential_grant_ref",
                "plugin_manifest_hash",
            ]
        } else {
            &["request_hash", "plugin_manifest_hash"]
        },
        "c3",
        if credential_required {
            &["market_data_freshness", "credential_scope"]
        } else {
            &["deterministic_fixture"]
        },
        "read_evidence",
        if credential_required {
            &[
                "mcp",
                "openapi",
                "graphql",
                "mode:research",
                "mode:paper",
                "asset:option_contract",
                "asset:equity",
            ]
        } else {
            &[
                "mcp",
                "graphql",
                "local-rust",
                "mode:research",
                "mode:backtest",
                "asset:option_contract",
                "asset:equity",
            ]
        },
    );
    operation.traits.modes = if credential_required {
        strings(vec!["research", "paper"])
    } else {
        strings(vec!["research", "backtest"])
    };
    operation.traits.instrument_families = strings(vec!["option_contract", "equity"]);
    operation.traits.data_shapes = strings(vec!["option_chain"]);
    operation.traits.operations = strings(vec!["read"]);
    operation.traits.fields = strings(vec![
        "symbol",
        "expiration",
        "strike",
        "right",
        "bid",
        "ask",
    ]);
    operation.traits.input_schema_refs =
        strings(vec!["schema://market-data/option-chain-request@1"]);
    operation.traits.output_schema_refs = strings(vec!["schema://market-data/option-chain@1"]);
    operation.traits.timeframes = strings(vec!["*"]);
    operation.traits.deterministic = !credential_required;
    operation.traits.replayable = !credential_required;
    operation
}

fn paper_preview_operation() -> PluginOperationContract {
    operation(
        "broker.paper_order_preview",
        "broker.order_submit.paper",
        "local-rust",
        "order.preview.paper",
        "draft",
        "medium",
        false,
        false,
        "brokerage_account",
        "paper_trading",
        &["request_hash", "strategy_version_hash", "risk_profile_hash"],
        "c3",
        &["risk_limits"],
        "order_preview",
        &[
            "mcp",
            "graphql",
            "local-rust",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
        ],
    )
}

fn paper_submit_operation(credential_required: bool) -> PluginOperationContract {
    let evidence = if credential_required {
        &[
            "request_hash",
            "strategy_version_hash",
            "risk_profile_hash",
            "credential_grant_ref",
        ][..]
    } else {
        &["request_hash", "strategy_version_hash", "risk_profile_hash"][..]
    };
    let check_packs = if credential_required {
        &["risk_limits", "credential_scope", "pep_receipt_sink"][..]
    } else {
        &["risk_limits", "local_simulation", "pep_receipt_sink"][..]
    };
    let protocol = if credential_required {
        "openapi"
    } else {
        "local-rust"
    };
    let supported_protocols = if credential_required {
        &[
            "mcp",
            "openapi",
            "graphql",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
        ][..]
    } else {
        &[
            "mcp",
            "graphql",
            "local-rust",
            "mode:paper",
            "asset:crypto",
            "asset:equity",
        ][..]
    };
    operation(
        "broker.paper_order_submit",
        "broker.order_submit.paper",
        protocol,
        "order.submit.paper",
        "write",
        "high",
        credential_required,
        credential_required,
        "brokerage_account",
        "paper_trading",
        evidence,
        "c3",
        check_packs,
        "finance_receipt",
        supported_protocols,
    )
}

#[allow(clippy::too_many_arguments)]
fn operation(
    id: &str,
    capability: &str,
    protocol: &str,
    action_id: &str,
    effect: &str,
    risk: &str,
    credential_grant_required: bool,
    account_binding_required: bool,
    resource_type: &str,
    purpose: &str,
    evidence: &[&str],
    pep_coverage_class: &str,
    check_packs: &[&str],
    receipt_class: &str,
    supported_protocols: &[&str],
) -> PluginOperationContract {
    PluginOperationContract {
        id: id.to_string(),
        capability: capability.to_string(),
        protocol: protocol.to_string(),
        resource_type: resource_type.to_string(),
        finance_resource_type: resource_type.to_string(),
        effect: effect.to_string(),
        risk: risk.to_string(),
        credential_grant_required,
        account_binding_required,
        apf_action_id: action_id.to_string(),
        mandate_required: !matches!(effect, "read" | "draft"),
        purpose: purpose.to_string(),
        evidence: evidence.iter().map(|item| item.to_string()).collect(),
        pep_coverage_class: pep_coverage_class.to_string(),
        check_packs: check_packs.iter().map(|item| item.to_string()).collect(),
        receipt_class: receipt_class.to_string(),
        redaction: "hash_or_ref".to_string(),
        no_advice: true,
        session_admission: "user_or_agent_session".to_string(),
        approval_mode: if matches!(effect, "write" | "credential" | "admin") {
            "human_confirmation_or_policy"
        } else {
            "policy_configured"
        }
        .to_string(),
        supervision_mode: if matches!(effect, "write" | "credential" | "admin") {
            "human_on_the_loop"
        } else {
            "automated_policy"
        }
        .to_string(),
        detectors: detectors_for_operation(id, credential_grant_required),
        context_providers: context_providers_for_operation(id),
        policy_refs: ["apf.finance.default", "tradeassembly.local_full"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        traits: operation_traits(id, supported_protocols, credential_grant_required),
        dependencies: Vec::new(),
        supported_protocols: supported_protocols
            .iter()
            .map(|item| item.to_string())
            .collect(),
    }
}

fn operation_traits(
    id: &str,
    supported_protocols: &[&str],
    credential_required: bool,
) -> PluginOperationTraits {
    let modes = supported_protocols
        .iter()
        .filter_map(|item| item.strip_prefix("mode:"))
        .map(str::to_string)
        .collect();
    let instrument_families = supported_protocols
        .iter()
        .filter_map(|item| item.strip_prefix("asset:"))
        .map(|family| match family {
            "crypto" => "crypto_spot",
            "option" => "option_contract",
            "future" => "future_contract",
            other => other,
        })
        .map(str::to_string)
        .collect();
    let (
        data_shapes,
        operations,
        fields,
        input_paths,
        input_schema_refs,
        output_schema_refs,
        timeframes,
        deterministic,
        replayable,
    ) = match id {
        "plugin.capability_matrix" => (
            vec!["capability_matrix"],
            vec![],
            vec![],
            vec![],
            vec!["schema://plugin/capability-request@1"],
            vec!["schema://plugin/capability-matrix@1"],
            vec![],
            true,
            true,
        ),
        "marketdata.quote.read" => (
            vec!["quote"],
            vec![],
            vec!["symbol", "price", "timestamp"],
            vec![],
            vec!["schema://market-data/quote-request@1"],
            vec!["schema://market-data/quote@1"],
            vec![],
            !credential_required,
            !credential_required,
        ),
        "marketdata.bars.read" => (
            vec!["bars"],
            vec![],
            vec!["open", "high", "low", "close", "volume", "timestamp"],
            vec![],
            vec!["schema://market-data/bars-request@1"],
            vec!["schema://market-data/bars@1"],
            vec!["*"],
            !credential_required,
            !credential_required,
        ),
        "marketdata.options_chain.read" => (
            vec!["option_chain"],
            vec![],
            vec!["symbol", "expiration", "strike", "right", "bid", "ask"],
            vec![],
            vec!["schema://market-data/option-chain-request@1"],
            vec!["schema://market-data/option-chain@1"],
            vec![],
            false,
            false,
        ),
        "broker.paper_order_preview" | "broker.paper_order_submit" => (
            vec!["order_intent"],
            vec![],
            vec!["symbol", "side", "quantity", "order_type"],
            vec![],
            vec!["schema://strategy/order-intent@1"],
            vec!["schema://broker/order-result@1"],
            vec![],
            id == "broker.paper_order_preview",
            id == "broker.paper_order_preview",
        ),
        _ => (
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            false,
            false,
        ),
    };
    PluginOperationTraits {
        modes,
        instrument_families,
        data_shapes: strings(data_shapes),
        operations: strings(operations),
        fields: strings(fields),
        input_paths: strings(input_paths),
        input_schema_refs: strings(input_schema_refs),
        output_schema_refs: strings(output_schema_refs),
        timeframes: strings(timeframes),
        max_freshness: None,
        max_latency_ms: None,
        deterministic,
        replayable,
    }
}

fn strings(values: Vec<&str>) -> Vec<String> {
    values.into_iter().map(str::to_string).collect()
}

fn detectors_for_operation(id: &str, credential_grant_required: bool) -> Vec<String> {
    let detectors = match id {
        "plugin.capability_matrix" => {
            vec!["no_advice", "capability_scope", "plugin_manifest_integrity"]
        }
        "marketdata.quote.read" if credential_grant_required => {
            vec!["no_advice", "capability_scope", "credential_scope"]
        }
        "marketdata.quote.read" => vec!["no_advice", "capability_scope", "data_provenance"],
        "marketdata.bars.read" | "marketdata.options_chain.read" if credential_grant_required => {
            vec![
                "no_advice",
                "capability_scope",
                "credential_scope",
                "data_provenance",
            ]
        }
        "marketdata.bars.read" | "marketdata.options_chain.read" => {
            vec!["no_advice", "capability_scope", "data_provenance"]
        }
        "broker.paper_order_preview" => vec!["no_advice", "capability_scope", "risk_limit"],
        "broker.paper_order_submit" if credential_grant_required => vec![
            "no_advice",
            "capability_scope",
            "credential_scope",
            "risk_limit",
            "order_authority",
        ],
        "broker.paper_order_submit" => vec![
            "no_advice",
            "capability_scope",
            "risk_limit",
            "order_authority",
        ],
        _ => vec!["no_advice", "capability_scope"],
    };
    detectors.into_iter().map(str::to_string).collect()
}

fn context_providers_for_operation(id: &str) -> Vec<String> {
    let providers = if id == "plugin.capability_matrix" {
        vec!["sightline.session_context", "tradeassembly.plugin_context"]
    } else {
        vec![
            "sightline.session_context",
            "tradeassembly.strategy_context",
            "tradeassembly.plugin_context",
        ]
    };
    providers.into_iter().map(str::to_string).collect()
}

pub(crate) fn plugin_value(plugin: &PluginCatalogEntry) -> Value {
    json!({
        "ref": plugin.plugin_ref,
        "instanceRef": plugin.instance_ref,
        "pluginRef": plugin.plugin_ref,
        "providerRef": plugin.provider_ref,
        "accountRef": plugin.account_ref,
        "name": plugin.name,
        "mode": "paper",
        "enabled": plugin.enabled,
        "capabilities": plugin.capabilities,
        "fingerprint": plugin.manifest_fingerprint,
        "manifest": {
            "id": plugin.plugin_ref,
            "name": plugin.name,
            "fingerprint": plugin.manifest_fingerprint,
            "capabilities": plugin.capabilities,
            "operations": plugin.operations.iter().map(operation_value).collect::<Vec<_>>(),
            "trustLevel": plugin.trust_level,
        },
        "credentialStatus": plugin.credential_status,
        "health": {"status": plugin.health},
        "capabilityResolution": {
            "profile": {"id": LOCAL_FULL_PROFILE, "source": "local", "default": true},
            "noSilentFallback": true,
            "capabilityCount": plugin.capabilities.len(),
        },
    })
}

pub(crate) fn operation_value(operation: &PluginOperationContract) -> Value {
    serde_json::to_value(operation).unwrap_or_else(|_| json!({}))
}

fn with_manifest_fingerprint(
    service: &TradeAssemblyService,
    mut plugin: PluginCatalogEntry,
) -> PluginCatalogEntry {
    plugin.manifest_fingerprint = service
        .runtime()
        .plugins
        .get_manifest(&plugin.plugin_ref)
        .ok()
        .flatten()
        .and_then(|record| record["manifestDigest"].as_str().map(str::to_string))
        .unwrap_or_else(|| plugin_lifecycle::built_in_manifest_digest(&plugin));
    plugin
}

fn explicit_capability_from(body: &Value) -> Option<String> {
    body.get("capability")
        .or_else(|| body.get("capabilityRef"))
        .or_else(|| body.get("capability_ref"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|capability| !capability.is_empty())
        .map(str::to_string)
}

fn entitlement_error(code: &str, message: &str) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

fn entitlement_id(effect: &str, capability: &str) -> String {
    format!(
        "grant:local_override:{effect}:{}",
        capability.replace('.', "_")
    )
}

fn context(event_type: &str, key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("{event_type}:{key}")).expect("valid idempotency key"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_parser_preserves_the_complete_strategy_and_binding_contract() {
        let requirement = requirement_from_body(&json!({
            "requirement_id": "req-rsi",
            "capability": "indicator.rsi.calculate@1",
            "purpose": "entry_evaluation",
            "required_for": ["authoring", "research", "backtest", "paper"],
            "stage_refs": ["evaluate"],
            "substep_refs": ["rsi-entry"],
            "constraints": {
                "instrument_families": ["equity", "option_contract"],
                "data_shapes": ["bars"],
                "operations": ["calculate"],
                "fields": ["close"],
                "input_paths": ["market_data.bars"],
                "schema_refs": ["schema://indicator/rsi@1"],
                "timeframe": "1m",
                "max_freshness": "PT2M",
                "max_latency_ms": 250,
                "deterministic": true,
                "replayable": true
            },
            "dependency_refs": ["req-bars"],
            "fallback_policy": "explicit_only",
            "policy_tags": ["research-approved"],
            "declaration": "required",
            "mode": "backtest",
            "asset_class": "equity",
            "instrument_family": "equity",
            "strategy_id": "strategy-1",
            "strategy_version_id": "version-4",
            "plugin_instance_ref": "indicator-instance",
            "plugin_ref": "example.indicators",
            "operation_id": "indicator.rsi.calculate",
            "account_ref": "account://example/backtest"
        }));

        assert_eq!(requirement.requirement_id, "req-rsi");
        assert_eq!(requirement.capability, "indicator.rsi.calculate@1");
        assert_eq!(
            requirement.required_for,
            ["authoring", "research", "backtest", "paper"]
        );
        assert_eq!(requirement.stage_refs, ["evaluate"]);
        assert_eq!(requirement.substep_refs, ["rsi-entry"]);
        assert_eq!(
            requirement.constraints.instrument_families,
            ["equity", "option_contract"]
        );
        assert_eq!(requirement.constraints.data_shapes, ["bars"]);
        assert_eq!(requirement.constraints.operations, ["calculate"]);
        assert_eq!(requirement.constraints.fields, ["close"]);
        assert_eq!(requirement.constraints.input_paths, ["market_data.bars"]);
        assert_eq!(
            requirement.constraints.schema_refs,
            ["schema://indicator/rsi@1"]
        );
        assert_eq!(requirement.constraints.timeframe.as_deref(), Some("1m"));
        assert_eq!(
            requirement.constraints.max_freshness.as_deref(),
            Some("PT2M")
        );
        assert_eq!(requirement.constraints.max_latency_ms, Some(250));
        assert!(requirement.constraints.deterministic);
        assert!(requirement.constraints.replayable);
        assert_eq!(requirement.dependency_refs, ["req-bars"]);
        assert_eq!(requirement.fallback_policy, "explicit_only");
        assert_eq!(requirement.policy_tags, ["research-approved"]);
        assert_eq!(requirement.declaration, "required");
        assert_eq!(requirement.mode, "backtest");
        assert_eq!(requirement.asset_class.as_deref(), Some("equity"));
        assert_eq!(requirement.instrument_family.as_deref(), Some("equity"));
        assert_eq!(requirement.strategy_id.as_deref(), Some("strategy-1"));
        assert_eq!(
            requirement.strategy_version_id.as_deref(),
            Some("version-4")
        );
        assert_eq!(
            requirement.plugin_instance_ref.as_deref(),
            Some("indicator-instance")
        );
        assert_eq!(
            requirement.plugin_ref.as_deref(),
            Some("example.indicators")
        );
        assert_eq!(
            requirement.operation_id.as_deref(),
            Some("indicator.rsi.calculate")
        );
        assert_eq!(
            requirement.account_ref.as_deref(),
            Some("account://example/backtest")
        );
        assert_eq!(requirement.purpose.as_deref(), Some("entry_evaluation"));
    }
}
