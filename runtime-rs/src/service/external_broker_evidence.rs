// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Credential-free evidence intake for orders placed through a customer-owned broker interface.

use super::TradeAssemblyService;
use crate::agent_runner::{deployments, RUNS_NS};
use crate::ports::{
    AuthorityContext, EventAppend, IdempotencyKey, OutboxRecord, SideEffectContext,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const RECEIPTS_NS: &str = "external_broker_receipts";
const EVENTS_NS: &str = "external_broker_receipt_events";
const IDEMPOTENCY_NS: &str = "external_broker_receipt_idempotency";
const RECONCILIATIONS_NS: &str = "external_broker_reconciliations";
const SCHEMA_VERSION: &str = "tradeassembly.external_broker_receipt.v1";

pub(crate) fn append(service: &TradeAssemblyService, request: Value) -> Value {
    let input = match validated_append_input(service, request) {
        Ok(input) => input,
        Err(code) => return error(&code),
    };
    let payload_hash = digest(&input);
    if let Some(result) = prior_result(
        service,
        input["idempotency_key"].as_str().unwrap_or_default(),
        &payload_hash,
    ) {
        return result;
    }

    let receipt_key = receipt_key(
        input["deployment_id"].as_str().unwrap_or_default(),
        input["client_order_id"].as_str().unwrap_or_default(),
    );
    let prior = service
        .runtime()
        .storage
        .get_json(RECEIPTS_NS, &receipt_key)
        .ok()
        .flatten();
    let incoming_rank = state_rank(input["event_type"].as_str().unwrap_or_default());
    let prior_rank = prior
        .as_ref()
        .and_then(|receipt| receipt["canonicalEventType"].as_str())
        .map(state_rank)
        .unwrap_or_default();
    let terminal_conflict = prior.as_ref().is_some_and(|receipt| {
        let current = receipt["canonicalEventType"].as_str().unwrap_or_default();
        is_terminal(current)
            && is_terminal(input["event_type"].as_str().unwrap_or_default())
            && current != input["event_type"].as_str().unwrap_or_default()
    });
    let stale = !terminal_conflict && prior.is_some() && incoming_rank < prior_rank;
    let reconciliation_status = if terminal_conflict {
        "required"
    } else {
        prior
            .as_ref()
            .and_then(|receipt| receipt["reconciliationStatus"].as_str())
            .unwrap_or("clear")
    };
    let event_id = format!(
        "external_receipt_event_{}",
        short_hash(&format!("{}:{}", input["idempotency_key"], payload_hash))
    );
    let event = json!({
        "schemaVersion": SCHEMA_VERSION,
        "eventId": event_id,
        "deploymentId": input["deployment_id"],
        "runId": input["run_id"],
        "broker": input["broker"],
        "environment": input["environment"],
        "clientOrderId": input["client_order_id"],
        "brokerOrderId": input["broker_order_id"],
        "eventType": input["event_type"],
        "occurredAtMs": input["occurred_at_ms"],
        "receipt": input["receipt"],
        "sourceRef": input["source_ref"],
        "payloadDigest": payload_hash,
        "outOfOrder": stale,
        "terminalConflict": terminal_conflict,
        "redacted": true,
    });
    let context = context(&input);
    if service
        .runtime()
        .storage
        .put_json(EVENTS_NS, &event_id, event.clone(), &context)
        .is_err()
    {
        return error("external_broker_receipt_persist_failed");
    }

    let mut canonical = if stale || terminal_conflict {
        prior.expect("prior receipt exists for a stale or terminal-conflict event")
    } else {
        json!({
            "schemaVersion": SCHEMA_VERSION,
            "receiptId": receipt_key,
            "deploymentId": input["deployment_id"],
            "runId": input["run_id"],
            "broker": input["broker"],
            "environment": input["environment"],
            "clientOrderId": input["client_order_id"],
            "brokerOrderId": input["broker_order_id"],
            "canonicalEventType": input["event_type"],
            "occurredAtMs": input["occurred_at_ms"],
            "receipt": input["receipt"],
            "sourceRef": input["source_ref"],
            "reconciliationStatus": reconciliation_status,
            "redacted": true,
        })
    };
    if terminal_conflict {
        canonical["reconciliationStatus"] = json!("required");
    }
    if !stale
        && service
            .runtime()
            .storage
            .put_json(RECEIPTS_NS, &receipt_key, canonical.clone(), &context)
            .is_err()
    {
        return error("external_broker_receipt_persist_failed");
    }
    if record_evidence(service, &input, &event, &context).is_err() {
        return error("external_broker_receipt_evidence_failed");
    }
    let result = json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "receipt": canonical,
        "event": event,
        "duplicate": false,
        "outOfOrder": stale,
        "reconciliationRequired": terminal_conflict,
    });
    if service
        .runtime()
        .storage
        .put_json(
            IDEMPOTENCY_NS,
            input["idempotency_key"].as_str().unwrap_or_default(),
            json!({"payloadDigest": payload_hash, "result": result}),
            &context,
        )
        .is_err()
    {
        return error("external_broker_receipt_idempotency_persist_failed");
    }
    result
}

pub(crate) fn inspect(service: &TradeAssemblyService, request: Value) -> Value {
    let Some(deployment_id) = nonempty(&request, "deployment_id") else {
        return error("external_broker_receipt_deployment_required");
    };
    let Some(client_order_id) = nonempty(&request, "client_order_id") else {
        return error("external_broker_receipt_client_order_required");
    };
    let key = receipt_key(&deployment_id, &client_order_id);
    let receipt = service
        .runtime()
        .storage
        .get_json(RECEIPTS_NS, &key)
        .ok()
        .flatten();
    let mut events = service
        .runtime()
        .storage
        .list_json(EVENTS_NS)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .filter(|event| {
            event["deploymentId"] == deployment_id && event["clientOrderId"] == client_order_id
        })
        .collect::<Vec<_>>();
    events.sort_by_key(|event| event["occurredAtMs"].as_i64().unwrap_or_default());
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "receipt": receipt,
        "events": events,
        "redacted": true,
    })
}

pub(crate) fn reconcile(service: &TradeAssemblyService, request: Value) -> Value {
    let Some(deployment_id) = nonempty(&request, "deployment_id") else {
        return error("external_broker_receipt_deployment_required");
    };
    let Some(client_order_id) = nonempty(&request, "client_order_id") else {
        return error("external_broker_receipt_client_order_required");
    };
    let Some(idempotency_key) = nonempty(&request, "idempotency_key") else {
        return error("external_broker_receipt_idempotency_required");
    };
    let Some(authority) = authority(&request) else {
        return error("external_broker_receipt_authority_required");
    };
    let resolution = nonempty(&request, "resolution").unwrap_or_else(|| "reviewed".to_string());
    if !matches!(resolution.as_str(), "reviewed" | "unresolved") {
        return error("external_broker_receipt_resolution_invalid");
    }
    let key = receipt_key(&deployment_id, &client_order_id);
    let Some(mut receipt) = service
        .runtime()
        .storage
        .get_json(RECEIPTS_NS, &key)
        .ok()
        .flatten()
    else {
        return error("external_broker_receipt_not_found");
    };
    let input = json!({
        "deployment_id": deployment_id,
        "client_order_id": client_order_id,
        "resolution": resolution,
        "idempotency_key": idempotency_key,
    });
    let payload_hash = digest(&input);
    if let Some(result) = prior_result(service, &idempotency_key, &payload_hash) {
        return result;
    }
    receipt["reconciliationStatus"] = json!(if resolution == "reviewed" {
        "reconciled"
    } else {
        "required"
    });
    let context = SideEffectContext::new(
        authority,
        IdempotencyKey::new(idempotency_key.clone()).expect("validated key"),
    );
    if service
        .runtime()
        .storage
        .put_json(RECEIPTS_NS, &key, receipt.clone(), &context)
        .is_err()
    {
        return error("external_broker_receipt_persist_failed");
    }
    let case_id = format!(
        "external_reconciliation_{}",
        short_hash(&format!("{key}:{idempotency_key}"))
    );
    let case = json!({
        "schemaVersion": SCHEMA_VERSION,
        "caseId": case_id,
        "receiptId": key,
        "deploymentId": deployment_id,
        "clientOrderId": client_order_id,
        "resolution": resolution,
        "status": receipt["reconciliationStatus"],
        "redacted": true,
    });
    if service
        .runtime()
        .storage
        .put_json(RECONCILIATIONS_NS, &case_id, case.clone(), &context)
        .is_err()
        || service
            .runtime()
            .record_side_effect("external_broker_receipt.reconciled", case.clone(), &context)
            .is_err()
    {
        return error("external_broker_receipt_evidence_failed");
    }
    let result = json!({"ok": true, "schemaVersion": SCHEMA_VERSION, "receipt": receipt, "case": case, "duplicate": false});
    if service
        .runtime()
        .storage
        .put_json(
            IDEMPOTENCY_NS,
            &idempotency_key,
            json!({"payloadDigest": payload_hash, "result": result}),
            &context,
        )
        .is_err()
    {
        return error("external_broker_receipt_idempotency_persist_failed");
    }
    result
}

fn validated_append_input(service: &TradeAssemblyService, request: Value) -> Result<Value, String> {
    let deployment_id =
        nonempty(&request, "deployment_id").ok_or("external_broker_receipt_deployment_required")?;
    let run_id = nonempty(&request, "run_id").ok_or("external_broker_receipt_run_required")?;
    let broker = nonempty(&request, "broker").ok_or("external_broker_receipt_broker_required")?;
    let environment =
        nonempty(&request, "environment").ok_or("external_broker_receipt_environment_required")?;
    let client_order_id = nonempty(&request, "client_order_id")
        .ok_or("external_broker_receipt_client_order_required")?;
    let event_type =
        nonempty(&request, "event_type").ok_or("external_broker_receipt_event_type_required")?;
    let idempotency_key = nonempty(&request, "idempotency_key")
        .ok_or("external_broker_receipt_idempotency_required")?;
    let authority = authority(&request).ok_or("external_broker_receipt_authority_required")?;
    if !matches!(environment.as_str(), "paper" | "live") {
        return Err("external_broker_receipt_environment_invalid".to_string());
    }
    if !matches!(
        event_type.as_str(),
        "order_submitted"
            | "order_accepted"
            | "order_partially_filled"
            | "order_filled"
            | "order_cancelled"
            | "order_rejected"
    ) {
        return Err("external_broker_receipt_event_type_invalid".to_string());
    }
    if !request["receipt"].is_object() {
        return Err("external_broker_receipt_payload_invalid".to_string());
    }
    if contains_sensitive_key(&request["receipt"]) {
        return Err("external_broker_receipt_sensitive_payload".to_string());
    }
    let deployment = deployments(&service.runtime())
        .map_err(|_| "external_broker_receipt_deployment_not_found".to_string())?
        .into_iter()
        .find(|deployment| deployment.deployment_id == deployment_id)
        .ok_or("external_broker_receipt_deployment_not_found")?;
    let run = service
        .runtime()
        .storage
        .get_json(RUNS_NS, &run_id)
        .map_err(|_| "external_broker_receipt_run_not_found".to_string())?
        .ok_or("external_broker_receipt_run_not_found")?;
    if run.pointer("/run/deploymentId").and_then(Value::as_str)
        != Some(deployment.deployment_id.as_str())
    {
        return Err("external_broker_receipt_run_mismatch".to_string());
    }
    Ok(json!({
        "deployment_id": deployment_id,
        "run_id": run_id,
        "broker": broker,
        "environment": environment,
        "client_order_id": client_order_id,
        "broker_order_id": nonempty(&request, "broker_order_id"),
        "event_type": event_type,
        "occurred_at_ms": request["occurred_at_ms"].as_i64().unwrap_or_else(|| service.runtime().clock.now_ms()),
        "receipt": request["receipt"].clone(),
        "source_ref": nonempty(&request, "source_ref"),
        "idempotency_key": idempotency_key,
        "authority_context": {"actor": authority.actor, "surface": authority.surface, "accountMode": authority.account_mode},
    }))
}

fn record_evidence(
    service: &TradeAssemblyService,
    input: &Value,
    event: &Value,
    context: &SideEffectContext,
) -> Result<(), String> {
    let now_ms = service.runtime().clock.now_ms();
    service.runtime().record_side_effect(
        "external_broker_receipt.appended",
        event.clone(),
        context,
    )?;
    service.runtime().events.append(EventAppend {
        stream: format!(
            "agent-deployment:{}",
            input["deployment_id"].as_str().unwrap_or_default()
        ),
        event_type: "external_broker_receipt.appended".to_string(),
        aggregate_id: input["deployment_id"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        payload: event.clone(),
        idempotency_key: context.idempotency_key.clone(),
        occurred_at_ms: now_ms,
        retention_until_ms: None,
    })?;
    service.runtime().outbox.append(OutboxRecord {
        outbox_id: format!(
            "outbox_external_broker_{}",
            short_hash(context.idempotency_key.as_str())
        ),
        topic: "external_broker_receipt.appended".to_string(),
        payload: event.clone(),
        idempotency_key: context.idempotency_key.clone(),
        created_at_ms: now_ms,
        fencing_token: 0,
    })
}

fn prior_result(service: &TradeAssemblyService, key: &str, payload_hash: &str) -> Option<Value> {
    let stored = service
        .runtime()
        .storage
        .get_json(IDEMPOTENCY_NS, key)
        .ok()
        .flatten()?;
    if stored["payloadDigest"] != payload_hash {
        return Some(error("external_broker_receipt_idempotency_conflict"));
    }
    let mut result = stored["result"].clone();
    result["duplicate"] = json!(true);
    Some(result)
}

fn context(input: &Value) -> SideEffectContext {
    let authority = authority(input).expect("validated authority");
    SideEffectContext::new(
        authority,
        IdempotencyKey::new(
            input["idempotency_key"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
        .expect("validated key"),
    )
}

fn authority(value: &Value) -> Option<AuthorityContext> {
    let context = value
        .get("authority_context")
        .or_else(|| value.get("authorityContext"))?;
    let actor = context.get("actor").and_then(Value::as_str)?.trim();
    if actor.is_empty() {
        return None;
    }
    Some(AuthorityContext {
        actor: actor.to_string(),
        surface: context
            .get("surface")
            .and_then(Value::as_str)
            .unwrap_or("mcp")
            .to_string(),
        account_mode: context
            .get("accountMode")
            .or_else(|| context.get("account_mode"))
            .and_then(Value::as_str)
            .unwrap_or("paper")
            .to_string(),
    })
}

fn nonempty(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn contains_sensitive_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let key = key.replace('-', "_").to_ascii_lowercase();
            matches!(
                key.as_str(),
                "access_token"
                    | "api_key"
                    | "api_secret"
                    | "authorization"
                    | "bearer"
                    | "client_secret"
                    | "password"
                    | "private_key"
                    | "refresh_token"
                    | "secret"
                    | "token"
            ) || key.ends_with("_token")
                || key.ends_with("_secret")
                || contains_sensitive_key(value)
        }),
        Value::Array(items) => items.iter().any(contains_sensitive_key),
        _ => false,
    }
}

fn state_rank(event_type: &str) -> u8 {
    match event_type {
        "order_submitted" => 10,
        "order_accepted" => 20,
        "order_partially_filled" => 30,
        "order_filled" | "order_cancelled" | "order_rejected" => 40,
        _ => 0,
    }
}

fn is_terminal(event_type: &str) -> bool {
    matches!(
        event_type,
        "order_filled" | "order_cancelled" | "order_rejected"
    )
}

fn receipt_key(deployment_id: &str, client_order_id: &str) -> String {
    format!(
        "external_receipt_{}",
        short_hash(&format!("{deployment_id}:{client_order_id}"))
    )
}

fn digest(value: &Value) -> String {
    format!("sha256:{:x}", Sha256::digest(value.to_string().as_bytes()))
}

fn short_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_string()
}

fn error(code: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "retryable": false}, "redacted": true})
}
