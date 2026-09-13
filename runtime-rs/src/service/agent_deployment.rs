// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Redacted MCP lifecycle facade for the local durable agent runner.
//!
//! This module deliberately delegates all runner state transitions to
//! `agent_runner`. It does not invoke the runtime adapter or a broker client;
//! recovery only records an acknowledged reconciliation and releases a future
//! scheduler tick.

use super::TradeAssemblyService;
use crate::agent_runner::{
    self, AgentDeployment, ACTIVE_RUNS_NS, RUNS_NS, SCHEMA_VERSION as RUNNER_SCHEMA_VERSION,
};
use crate::ports::{
    AuthorityContext, EventAppend, IdempotencyKey, OutboxRecord, SideEffectContext,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: &str = "tradeassembly.agent_mcp_lifecycle.v1";
const MAX_EVENT_LIMIT: usize = 100;

pub(crate) fn validate_mcp_arguments(name: &str, request: &Value) -> Option<&'static str> {
    if !is_mutation(name) {
        return None;
    }
    if nonempty_alias(request, &["idempotency_key", "idempotencyKey"]).is_none() {
        return Some("agent_deployment_idempotency_required");
    }
    if authority(request).is_none() {
        return Some("agent_deployment_authority_required");
    }
    match name {
        "studio.deployment.create" if !request["deployment"].is_object() => {
            Some("agent_deployment_payload_invalid")
        }
        "studio.deployment.start" | "studio.deployment.pause" | "studio.deployment.stop"
            if nonempty_alias(request, &["deployment_id", "deploymentId"]).is_none() =>
        {
            Some("agent_deployment_id_required")
        }
        "studio.agent_run.recover" => {
            if nonempty_alias(request, &["deployment_id", "deploymentId"]).is_none() {
                Some("agent_deployment_id_required")
            } else if request["acknowledge_reconciled"] != Value::Bool(true)
                && request["acknowledgeReconciled"] != Value::Bool(true)
            {
                Some("agent_recovery_ack_required")
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(crate) fn create(service: &TradeAssemblyService, request: Value) -> Value {
    let deployment = match request
        .get("deployment")
        .cloned()
        .and_then(|value| serde_json::from_value::<AgentDeployment>(value).ok())
    {
        Some(deployment) => deployment,
        None => return error("agent_deployment_payload_invalid"),
    };
    let Some(context) = side_effect_context(&request) else {
        return error("agent_deployment_authority_required");
    };
    if context.authority.account_mode != deployment.mode {
        return error("agent_deployment_authority_mode_mismatch");
    }
    if let Err(code) =
        agent_runner::put_deployment_with_context(&service.runtime(), &deployment, &context)
    {
        return error(&code);
    }
    let summary = safe_deployment(&deployment);
    if record_lifecycle_evidence(
        service,
        &deployment.deployment_id,
        "agent_deployment.created",
        json!({"operation": "create", "deployment": summary}),
        &context,
    )
    .is_err()
    {
        return error("agent_deployment_evidence_failed");
    }
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "deployment": safe_deployment(&deployment),
        "redacted": true,
    })
}

pub(crate) fn start(service: &TradeAssemblyService, request: Value) -> Value {
    set_desired_state(service, request, "active", "start")
}

pub(crate) fn pause(service: &TradeAssemblyService, request: Value) -> Value {
    set_desired_state(service, request, "paused", "pause")
}

pub(crate) fn stop(service: &TradeAssemblyService, request: Value) -> Value {
    set_desired_state(service, request, "stopped", "stop")
}

pub(crate) fn inspect(service: &TradeAssemblyService, request: Value) -> Value {
    let deployments = match agent_runner::deployments(&service.runtime()) {
        Ok(deployments) => deployments,
        Err(_) => return error("agent_deployment_read_failed"),
    };
    let requested_id = nonempty_alias(&request, &["deployment_id", "deploymentId"]);
    let selected = deployments
        .iter()
        .filter(|deployment| {
            requested_id
                .as_deref()
                .is_none_or(|deployment_id| deployment.deployment_id == deployment_id)
        })
        .collect::<Vec<_>>();
    if requested_id.is_some() && selected.is_empty() {
        return error("agent_deployment_not_found");
    }
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "runnerSchemaVersion": RUNNER_SCHEMA_VERSION,
        "host": "local",
        "deployments": selected.into_iter().map(safe_deployment).collect::<Vec<_>>(),
        "redacted": true,
    })
}

pub(crate) fn inspect_run(service: &TradeAssemblyService, request: Value) -> Value {
    let Some(run_id) = nonempty_alias(&request, &["run_id", "runId"]) else {
        return error("agent_run_id_required");
    };
    let record = match service.runtime().storage.get_json(RUNS_NS, &run_id) {
        Ok(Some(record)) => record,
        Ok(None) => return error("agent_run_not_found"),
        Err(_) => return error("agent_run_read_failed"),
    };
    let Some(run) = safe_run(&record) else {
        return error("agent_run_read_failed");
    };
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "run": run,
        "redacted": true,
    })
}

pub(crate) fn events(service: &TradeAssemblyService, request: Value) -> Value {
    let Some(deployment_id) = nonempty_alias(&request, &["deployment_id", "deploymentId"]) else {
        return error("agent_deployment_id_required");
    };
    if deployment(service, &deployment_id).is_err() {
        return error("agent_deployment_not_found");
    }
    let after_sequence = request
        .get("after_sequence")
        .or_else(|| request.get("afterSequence"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if after_sequence < 0 {
        return error("agent_run_events_cursor_invalid");
    }
    let limit = request
        .get("limit")
        .and_then(Value::as_u64)
        .map(|limit| limit as usize)
        .unwrap_or(50)
        .clamp(1, MAX_EVENT_LIMIT);
    let events = match service.runtime().events.replay(
        &format!("agent-deployment:{deployment_id}"),
        after_sequence,
        limit,
    ) {
        Ok(events) => events,
        Err(_) => return error("agent_run_events_read_failed"),
    };
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "deploymentId": deployment_id,
        "events": events.into_iter().map(safe_event).collect::<Vec<_>>(),
        "redacted": true,
    })
}

pub(crate) fn health(service: &TradeAssemblyService, request: Value) -> Value {
    let requested_id = nonempty_alias(&request, &["deployment_id", "deploymentId"]);
    let deployments = match agent_runner::deployments(&service.runtime()) {
        Ok(deployments) => deployments,
        Err(_) => return error("agent_deployment_read_failed"),
    };
    let selected = deployments
        .iter()
        .filter(|deployment| {
            requested_id
                .as_deref()
                .is_none_or(|deployment_id| deployment.deployment_id == deployment_id)
        })
        .collect::<Vec<_>>();
    if requested_id.is_some() && selected.is_empty() {
        return error("agent_deployment_not_found");
    }
    let deployment_ids = selected
        .iter()
        .map(|deployment| deployment.deployment_id.as_str())
        .collect::<Vec<_>>();
    let runs = match service.runtime().storage.list_json(RUNS_NS) {
        Ok(runs) => runs,
        Err(_) => return error("agent_run_read_failed"),
    };
    let runs = runs
        .into_iter()
        .filter_map(|(_, record)| safe_run(&record))
        .filter(|run| {
            run["deploymentId"]
                .as_str()
                .is_some_and(|deployment_id| deployment_ids.contains(&deployment_id))
        })
        .collect::<Vec<_>>();
    let active = match service.runtime().storage.list_json(ACTIVE_RUNS_NS) {
        Ok(active) => active,
        Err(_) => return error("agent_run_read_failed"),
    };
    let pending_reconcile = active
        .into_iter()
        .filter_map(|(deployment_id, record)| {
            let embedded_deployment_id = record
                .get("deploymentId")
                .or_else(|| record.get("deployment_id"))
                .and_then(Value::as_str);
            (embedded_deployment_id.is_none_or(|embedded| embedded == deployment_id.as_str())
                && record["state"] == "pending_reconcile"
                && deployment_ids.contains(&deployment_id.as_str()))
            .then_some(deployment_id)
        })
        .collect::<Vec<_>>();
    let active_count = selected
        .iter()
        .filter(|deployment| deployment.desired_state == "active")
        .count();
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "host": "local",
        "deployments": selected.into_iter().map(safe_deployment).collect::<Vec<_>>(),
        "runCounts": {
            "total": runs.len(),
            "activeDeployments": active_count,
            "pendingReconcileDeployments": pending_reconcile.len(),
        },
        "runs": runs,
        "pendingReconcileDeploymentIds": pending_reconcile,
        "redacted": true,
    })
}

pub(crate) fn recover(service: &TradeAssemblyService, request: Value) -> Value {
    let Some(deployment_id) = nonempty_alias(&request, &["deployment_id", "deploymentId"]) else {
        return error("agent_deployment_id_required");
    };
    if request["acknowledge_reconciled"] != Value::Bool(true)
        && request["acknowledgeReconciled"] != Value::Bool(true)
    {
        return error("agent_recovery_ack_required");
    }
    let Some(context) = side_effect_context(&request) else {
        return error("agent_deployment_authority_required");
    };
    let deployment = match deployment(service, &deployment_id) {
        Ok(deployment) => deployment,
        Err(code) => return error(&code),
    };
    if context.authority.account_mode != deployment.mode {
        return error("agent_deployment_authority_mode_mismatch");
    }
    // `recover_pending_run_with_context` only changes a quarantined run to
    // reconciled and opens a future scheduler tick. It records durable,
    // authority-bound reconciliation evidence before completing its replay-safe
    // receipt; it never starts a runtime adapter or submits an order.
    let receipt = match agent_runner::recover_pending_run_with_context(
        &service.runtime(),
        &deployment_id,
        service.runtime().clock.now_ms(),
        &context,
    ) {
        Ok(receipt) => receipt,
        Err(code)
            if matches!(
                code.as_str(),
                "agent_recovery_lease_held" | "agent_recovery_retryable"
            ) =>
        {
            return retryable_error(&code)
        }
        Err(code) => return error(&code),
    };
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "deploymentId": deployment_id,
        "acknowledgedReconciled": true,
        "orderReplay": false,
        "receipt": safe_receipt(&receipt),
        "redacted": true,
    })
}

fn set_desired_state(
    service: &TradeAssemblyService,
    request: Value,
    desired_state: &str,
    operation: &str,
) -> Value {
    let Some(deployment_id) = nonempty_alias(&request, &["deployment_id", "deploymentId"]) else {
        return error("agent_deployment_id_required");
    };
    let Some(context) = side_effect_context(&request) else {
        return error("agent_deployment_authority_required");
    };
    let current = match deployment(service, &deployment_id) {
        Ok(deployment) => deployment,
        Err(code) => return error(&code),
    };
    if context.authority.account_mode != current.mode {
        return error("agent_deployment_authority_mode_mismatch");
    }
    if let Err(code) = agent_runner::set_desired_state_with_context(
        &service.runtime(),
        &deployment_id,
        desired_state,
        &context,
    ) {
        return error(&code);
    }
    let updated = match deployment(service, &deployment_id) {
        Ok(deployment) => deployment,
        Err(code) => return error(&code),
    };
    if record_lifecycle_evidence(
        service,
        &deployment_id,
        "agent_deployment.desired_state_changed",
        json!({
            "operation": operation,
            "desiredState": desired_state,
            "deployment": safe_deployment(&updated),
        }),
        &context,
    )
    .is_err()
    {
        return error("agent_deployment_evidence_failed");
    }
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "deployment": safe_deployment(&updated),
        "redacted": true,
    })
}

fn deployment(
    service: &TradeAssemblyService,
    deployment_id: &str,
) -> Result<AgentDeployment, String> {
    agent_runner::deployments(&service.runtime())?
        .into_iter()
        .find(|deployment| deployment.deployment_id == deployment_id)
        .ok_or_else(|| "agent_deployment_not_found".to_string())
}

fn side_effect_context(request: &Value) -> Option<SideEffectContext> {
    let authority = authority(request)?;
    let idempotency_key = nonempty_alias(request, &["idempotency_key", "idempotencyKey"])?;
    let idempotency_key = IdempotencyKey::new(idempotency_key).ok()?;
    Some(SideEffectContext::new(authority, idempotency_key))
}

fn authority(request: &Value) -> Option<AuthorityContext> {
    let context = request
        .get("authority_context")
        .or_else(|| request.get("authorityContext"))?;
    let actor = context.get("actor").and_then(Value::as_str)?.trim();
    let surface = context.get("surface").and_then(Value::as_str)?.trim();
    let account_mode = context
        .get("accountMode")
        .or_else(|| context.get("account_mode"))
        .and_then(Value::as_str)?
        .trim();
    if actor.is_empty() || surface.is_empty() || !matches!(account_mode, "paper" | "live") {
        return None;
    }
    Some(AuthorityContext {
        actor: actor.to_string(),
        surface: surface.to_string(),
        account_mode: account_mode.to_string(),
    })
}

fn safe_deployment(deployment: &AgentDeployment) -> Value {
    json!({
        "deploymentId": deployment.deployment_id,
        "systemProjectId": deployment.system_project_id,
        "agentDefinitionVersionId": deployment.agent_definition_version_id,
        "executionConfigVersionId": deployment.execution_config_version_id,
        "studioToolAllowlist": deployment.studio_tool_allowlist,
        "desiredState": deployment.desired_state,
        "intervalSeconds": deployment.interval_seconds,
        "cronUtc": deployment.cron_utc,
        "mode": deployment.mode,
        "bindingDigest": agent_runner::deployment_binding_digest(deployment),
        "credentialPosture": "customer-managed; no credential material is returned",
        "redacted": true,
    })
}

fn safe_run(record: &Value) -> Option<Value> {
    let run = record.get("run").unwrap_or(record);
    let run_id = run.get("runId").or_else(|| run.get("run_id"))?.as_str()?;
    let deployment_id = run
        .get("deploymentId")
        .or_else(|| run.get("deployment_id"))?
        .as_str()?;
    let state = run.get("state")?.as_str()?;
    let mut safe = json!({
        "runId": run_id,
        "deploymentId": deployment_id,
        "state": state,
        "redacted": true,
    });
    copy_string_if_present(&mut safe, run, "triggerId", "triggerId");
    copy_string_if_present(
        &mut safe,
        run,
        "deploymentBindingDigest",
        "deploymentBindingDigest",
    );
    copy_i64_if_present(&mut safe, run, "leaseFence", "leaseFence");
    copy_string_if_present(&mut safe, run, "errorCode", "errorCode");
    copy_string_if_present(&mut safe, record, "errorCode", "errorCode");
    Some(safe)
}

fn safe_event(event: crate::ports::EventRecord) -> Value {
    json!({
        "sequence": event.sequence,
        "eventType": event.event_type,
        "aggregateId": event.aggregate_id,
        "occurredAtMs": event.occurred_at_ms,
        "redacted": true,
    })
}

fn safe_receipt(receipt: &Value) -> Value {
    json!({
        "deploymentId": receipt["deploymentId"],
        "outcome": receipt["outcome"],
        "runId": receipt["runId"],
        "redacted": true,
    })
}

fn copy_string_if_present(target: &mut Value, source: &Value, source_key: &str, target_key: &str) {
    if let Some(value) = source.get(source_key).and_then(Value::as_str) {
        target[target_key] = json!(value);
    }
}

fn copy_i64_if_present(target: &mut Value, source: &Value, source_key: &str, target_key: &str) {
    if let Some(value) = source.get(source_key).and_then(Value::as_i64) {
        target[target_key] = json!(value);
    }
}

fn record_lifecycle_evidence(
    service: &TradeAssemblyService,
    deployment_id: &str,
    event_type: &str,
    summary: Value,
    context: &SideEffectContext,
) -> Result<(), String> {
    let occurred_at_ms = service.runtime().clock.now_ms();
    let payload = json!({
        "schemaVersion": SCHEMA_VERSION,
        "deploymentId": deployment_id,
        "eventType": event_type,
        "authority": {
            "actor": context.authority.actor,
            "surface": context.authority.surface,
            "accountMode": context.authority.account_mode,
        },
        "summary": summary,
        "redacted": true,
    });
    service
        .runtime()
        .record_side_effect(event_type, payload.clone(), context)?;
    let event_key = lifecycle_key(event_type, deployment_id, context.idempotency_key.as_str())?;
    service.runtime().events.append(EventAppend {
        stream: format!("agent-deployment:{deployment_id}"),
        event_type: event_type.to_string(),
        aggregate_id: deployment_id.to_string(),
        payload: payload.clone(),
        idempotency_key: event_key.clone(),
        occurred_at_ms,
        retention_until_ms: None,
    })?;
    service.runtime().outbox.append(OutboxRecord {
        outbox_id: format!(
            "outbox_agent_lifecycle_{}",
            short_hash(&format!(
                "{event_type}:{deployment_id}:{}",
                context.idempotency_key.as_str()
            ))
        ),
        topic: event_type.to_string(),
        payload,
        idempotency_key: event_key,
        created_at_ms: occurred_at_ms,
        fencing_token: 0,
    })
}

fn lifecycle_key(
    event_type: &str,
    deployment_id: &str,
    idempotency_key: &str,
) -> Result<IdempotencyKey, String> {
    IdempotencyKey::new(format!(
        "agent-lifecycle:{}",
        short_hash(&format!("{event_type}:{deployment_id}:{idempotency_key}"))
    ))
}

fn short_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..24].to_string()
}

fn nonempty_alias(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn is_mutation(name: &str) -> bool {
    matches!(
        name,
        "studio.deployment.create"
            | "studio.deployment.start"
            | "studio.deployment.pause"
            | "studio.deployment.stop"
            | "studio.agent_run.recover"
    )
}

/// Authenticated MCP normally defaults to paper. Lifecycle operators may
/// explicitly select paper or live, after their verified identity replaces any
/// caller-supplied actor. Runner-scoped calls get their mode from the durable
/// deployment capability instead.
pub(crate) fn preserve_requested_mcp_mode(name: &str) -> bool {
    is_mutation(name)
}

fn error(code: &str) -> Value {
    json!({"ok": false, "error": {"code": code}})
}

fn retryable_error(code: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "retryable": true}})
}
