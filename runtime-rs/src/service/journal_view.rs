use super::{ServiceResponse, TradeAssemblyService};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Subject alone is insufficient: also verify issuer and tenant via object scope.
pub(super) fn events(service: &TradeAssemblyService) -> Result<Vec<Value>, ServiceResponse> {
    let owner = service
        .invocation_owner()
        .ok_or_else(|| ServiceResponse::error(401, "journal_owner_required", false))?;
    let runtime = service.runtime();
    let records = runtime
        .journal
        .try_events()
        .map_err(|_| ServiceResponse::error(503, "journal_read_unavailable", true))?;
    let mut rows = Vec::new();
    for event in records {
        if let Some(event_owner) = &event.owner {
            if event_owner == &owner {
                rows.push(row(event));
            }
            continue;
        }
        // Legacy writers use local-user for the acting machinery. Ownership is
        // the referenced object's durable scope, never that display actor.
        let mut references = Vec::new();
        for field in ["strategy_id", "strategyId"] {
            if let Some(id) = event.payload.get(field).and_then(Value::as_str) {
                references.push(id);
            }
        }
        if event.event_type == "strategy.created" {
            if let Some(id) = event.payload.get("id").and_then(Value::as_str) {
                references.push(id);
            }
        }
        // Unbound legacy records must not become globally visible.
        if references.is_empty() {
            continue;
        }
        let mut visible = true;
        for id in references {
            let scope = runtime
                .object_authorization
                .scope("strategy", id)
                .map_err(|_| ServiceResponse::error(503, "journal_scope_unavailable", true))?;
            if !scope.is_some_and(|scope| scope.owner == owner) {
                visible = false;
                break;
            }
        }
        if visible {
            rows.push(row(event));
        }
    }
    Ok(rows)
}

fn row(event: crate::ports::journal::JournalEvent) -> Value {
    crate::mcp::redact_sensitive_payload(
        json!({"event_type": event.event_type, "eventType": event.event_type,
        "owner": event.owner, "authority": event.authority,
        "idempotency_key": event.idempotency_key.as_str(), "payload": event.payload}),
    )
}

pub(super) fn export(service: &TradeAssemblyService) -> ServiceResponse {
    let events = match events(service) {
        Ok(events) => events,
        Err(error) => return error,
    };
    let canonical = match serde_json_canonicalizer::to_vec(&events) {
        Ok(bytes) => bytes,
        Err(_) => return ServiceResponse::error(503, "journal_export_unavailable", true),
    };
    ServiceResponse::ok(json!({
        "schemaVersion": 1, "scope": "authenticated_owner",
        "legacyProjection": "owned_strategy_events_only",
        "eventsSha256": format!("{:x}", Sha256::digest(canonical)),
        "events": events,
    }))
}
