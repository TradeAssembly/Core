use std::fmt;

use serde_json::{json, Value};

pub const SIGHTLINE_GIT_URL: &str = "https://github.com/TradeAssemblyHQ/sightline.git";
pub const SIGHTLINE_GIT_REV: &str = "6620d6add943975b1f0a8ec43975420621dcda83";
pub const TRADEASSEMBLY_REDACTION_POLICY: &str = "tradeassembly.redaction.agent_safe.v1";

const SIGHTLINE_MCP_TOOLS: [&str; 10] = [
    "sightline.get_session_capabilities",
    "sightline.list_rooms",
    "sightline.list_surfaces",
    "sightline.get_current_selection",
    "sightline.get_shared_selection",
    "sightline.get_context",
    "sightline.create_proposal",
    "sightline.request_approval",
    "sightline.navigate_surface",
    "sightline.subscribe_events",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    contract: &'static str,
    stage: &'static str,
}

impl ContractError {
    fn new(contract: &'static str, stage: &'static str) -> Self {
        Self { contract, stage }
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Sightline {} contract {} failed",
            self.contract, self.stage
        )
    }
}

impl std::error::Error for ContractError {}

pub fn contract_source() -> Value {
    json!({
        "contract": "tradeassembly.sightline.adapter.v1",
        "source": "checked_in_public_adapter_contract",
        "compatibility_target": {
            "git": SIGHTLINE_GIT_URL,
            "rev": SIGHTLINE_GIT_REV,
            "verification": "verified_local_source",
        },
    })
}

pub fn conform_actor(value: &Value) -> Result<Value, ContractError> {
    let value = conform_object(
        "ActorRef",
        value,
        &[&["actor_type", "actorType"], &["actor_id", "actorId"]],
    )?;
    let actor_type = alias(&value, &["actor_type", "actorType"])
        .and_then(Value::as_str)
        .ok_or_else(|| ContractError::new("ActorRef", "actor_type"))?;
    if !["agent", "service", "system", "user"].contains(&actor_type) {
        return Err(ContractError::new("ActorRef", "actor_type"));
    }
    Ok(value)
}

pub fn conform_room(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "CollaborationRoom",
        value,
        &[
            &["room_id", "roomId"],
            &["app_id", "appId"],
            &["owner_user_ref", "ownerUserRef"],
            &["status"],
        ],
    )
}

pub fn conform_surface(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "SurfaceInstance",
        value,
        &[
            &["surface_id", "surfaceId"],
            &["room_id", "roomId"],
            &["app_id", "appId"],
            &["route"],
            &["status"],
        ],
    )
}

pub fn conform_node(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "SemanticNode",
        value,
        &[
            &["node_id", "nodeId"],
            &["surface_id", "surfaceId"],
            &["type"],
            &["role"],
            &["label"],
            &["summary"],
        ],
    )
}

pub fn conform_object_ref(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "SemanticObjectRef",
        value,
        &[
            &["app_id", "appId"],
            &["object_type", "objectType"],
            &["object_id", "objectId"],
        ],
    )
}

pub fn conform_selection(value: &Value) -> Result<Value, ContractError> {
    let value = conform_object(
        "SelectionContext",
        value,
        &[
            &["selection_id", "selectionId"],
            &["room_id", "roomId"],
            &["surface_id", "surfaceId"],
            &["selected_by", "selectedBy"],
            &["node_ref", "nodeRef"],
            &["shared"],
        ],
    )?;
    if alias(&value, &["shared"])
        .and_then(Value::as_bool)
        .is_none()
    {
        return Err(ContractError::new("SelectionContext", "shared"));
    }
    Ok(value)
}

pub fn conform_snapshot(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "ContextSnapshot",
        value,
        &[
            &["snapshot_id", "snapshotId"],
            &["room_id", "roomId"],
            &["surface_id", "surfaceId"],
            &["root_node_id", "rootNodeId"],
            &["version"],
        ],
    )
}

pub fn conform_agent_connection(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "AgentConnection",
        value,
        &[
            &["agent_connection_id", "agentConnectionId"],
            &["room_id", "roomId"],
            &["agent_ref", "agentRef"],
            &["status"],
        ],
    )
}

pub fn conform_presence(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "Presence",
        value,
        &[
            &["presence_id", "presenceId"],
            &["room_id", "roomId"],
            &["actor"],
            &["status"],
        ],
    )
}

pub fn conform_command(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "SemanticCommand",
        value,
        &[
            &["command_id", "commandId"],
            &["room_id", "roomId"],
            &["issued_by", "issuedBy"],
            &["target"],
            &["command_type", "commandType"],
            &["authority_level", "authorityLevel"],
            &["idempotency_key", "idempotencyKey"],
            &["status"],
        ],
    )
}

pub fn conform_proposal(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "Proposal",
        value,
        &[
            &["proposal_id", "proposalId"],
            &["room_id", "roomId"],
            &["proposed_by", "proposedBy"],
            &["target_object", "targetObject"],
            &["patch_format", "patchFormat"],
            &["patches"],
            &["status"],
        ],
    )
}

pub fn conform_approval(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "ApprovalRequest",
        value,
        &[
            &["approval_id", "approvalId"],
            &["room_id", "roomId"],
            &["requested_by", "requestedBy"],
            &["action"],
            &["target"],
            &["authority_level", "authorityLevel"],
            &["status"],
        ],
    )
}

pub fn conform_event(value: &Value) -> Result<Value, ContractError> {
    conform_object(
        "SightlineEvent",
        value,
        &[
            &["event_id", "eventId"],
            &["room_id", "roomId"],
            &["type"],
            &["payload"],
            &["sequence"],
        ],
    )
}

pub fn resolved_context(selection: &Value, context: &Value) -> Result<Value, ContractError> {
    let selection = conform_selection(selection)?;
    let shared = selection["shared"].as_bool().unwrap_or(false);
    Ok(json!({
        "resolution_kind": "selection",
        "resolution_source": if shared { "shared" } else { "explicit" },
        "room_id": alias(&selection, &["room_id", "roomId"]),
        "surface_id": alias(&selection, &["surface_id", "surfaceId"]),
        "selection": selection,
        "context": redact_agent_value(context),
    }))
}

pub fn selection_visible_to_actor(selection: &Value, actor: &Value) -> bool {
    let Ok(selection) = conform_selection(selection) else {
        return false;
    };
    let Ok(actor) = conform_actor(actor) else {
        return false;
    };
    alias(&actor, &["actor_type", "actorType"]).and_then(Value::as_str) != Some("agent")
        || selection["shared"].as_bool() == Some(true)
}

pub fn event_visible_to_actor(event: &Value, actor: &Value) -> bool {
    let Ok(event) = conform_event(event) else {
        return false;
    };
    let Ok(actor) = conform_actor(actor) else {
        return false;
    };
    let event_type = event["type"].as_str().unwrap_or_default();
    if alias(&actor, &["actor_type", "actorType"]).and_then(Value::as_str) != Some("agent")
        || !event_type.starts_with("selection.")
    {
        return true;
    }
    match event_type {
        "selection.changed" => {
            event["payload"].get("shared").and_then(Value::as_bool) == Some(true)
        }
        "selection.shared" => true,
        _ => false,
    }
}

pub fn redact_agent_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(redact_agent_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    if sensitive_key(key) && !safe_status_value(key, value) {
                        (key.clone(), json!("[REDACTED]"))
                    } else {
                        (key.clone(), redact_agent_value(value))
                    }
                })
                .collect(),
        ),
        Value::String(text) if secret_shaped(text) => json!("[REDACTED]"),
        _ => value.clone(),
    }
}

pub fn transport_contract() -> Result<Value, ContractError> {
    let mappings = [
        (
            "tradeassembly.sightline.session_state",
            &[
                "sightline.get_session_capabilities",
                "sightline.list_rooms",
                "sightline.list_surfaces",
            ][..],
        ),
        (
            "tradeassembly.sightline.get_current_selection",
            &[
                "sightline.get_current_selection",
                "sightline.get_shared_selection",
            ][..],
        ),
        (
            "tradeassembly.sightline.get_context",
            &["sightline.get_context"][..],
        ),
        (
            "tradeassembly.sightline.create_proposal",
            &["sightline.create_proposal"][..],
        ),
        (
            "tradeassembly.sightline.request_approval",
            &["sightline.request_approval"][..],
        ),
        (
            "tradeassembly.sightline.navigate_surface",
            &["sightline.navigate_surface"][..],
        ),
        (
            "tradeassembly.sightline.list_events",
            &["sightline.subscribe_events"][..],
        ),
    ];
    if mappings
        .iter()
        .flat_map(|(_, tools)| *tools)
        .any(|tool| !SIGHTLINE_MCP_TOOLS.contains(tool))
    {
        return Err(ContractError::new("Sightline MCP tools", "bind"));
    }
    Ok(json!({
        "source": contract_source(),
        "protocol": {
            "interface_kind": "agent",
            "protocol": "mcp",
            "protocol_version": "2025-06-18",
            "transport": "mcp-json-rpc",
        },
        "tool_bindings": mappings.iter().map(|(tradeassembly_tool, sightline_tools)| json!({
            "tradeassembly_tool": tradeassembly_tool,
            "sightline_tools": sightline_tools,
        })).collect::<Vec<_>>(),
    }))
}

fn conform_object(
    contract: &'static str,
    value: &Value,
    required_aliases: &[&[&str]],
) -> Result<Value, ContractError> {
    if !value.is_object() {
        return Err(ContractError::new(contract, "object"));
    }
    for aliases in required_aliases {
        let Some(field) = alias(value, aliases) else {
            return Err(ContractError::new(contract, "required_field"));
        };
        if field.is_null() || field.as_str().is_some_and(str::is_empty) {
            return Err(ContractError::new(contract, "required_field"));
        }
    }
    Ok(value.clone())
}

fn alias<'a>(value: &'a Value, aliases: &[&str]) -> Option<&'a Value> {
    aliases.iter().find_map(|alias| value.get(*alias))
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("apikey")
        || normalized.contains("apisecret")
        || normalized.contains("accesstoken")
        || normalized.contains("refreshtoken")
        || normalized.contains("accounttoken")
        || normalized.contains("bearertoken")
        || normalized.contains("credentialhandle")
        || normalized.contains("credentialvalue")
        || normalized.contains("credentialstatus")
        || normalized.contains("oauth")
        || normalized.contains("password")
        || normalized.contains("privatekey")
        || normalized.contains("secret")
}

fn safe_status_value(key: &str, value: &Value) -> bool {
    let normalized_key = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let status_key = [
        "available",
        "configured",
        "connected",
        "health",
        "present",
        "required",
        "status",
    ]
    .iter()
    .any(|suffix| normalized_key.ends_with(suffix));
    let Some(status) = value.as_str() else {
        return false;
    };
    status_key
        && [
            "absent",
            "available",
            "configured",
            "connected",
            "disabled",
            "disconnected",
            "enabled",
            "error",
            "healthy",
            "invalid",
            "missing",
            "not_configured",
            "not_ready",
            "not_required",
            "ok",
            "present",
            "ready",
            "required",
            "unavailable",
            "unhealthy",
            "unknown",
            "valid",
        ]
        .contains(&status)
}

fn secret_shaped(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("sk_live_")
        || trimmed.starts_with("sk_test_")
        || trimmed.starts_with("ghp_")
        || trimmed.starts_with("github_pat_")
        || trimmed.starts_with("xoxb-")
        || trimmed.starts_with("xoxp-")
        || trimmed.starts_with("Bearer ")
        || trimmed.contains(&["BEGIN ", "PRIVATE KEY"].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_visibility_inputs_fail_closed() {
        assert!(!selection_visible_to_actor(&json!({}), &json!({})));
        assert!(!event_visible_to_actor(&json!({}), &json!({})));
    }
}
