// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{
    api_result, execution, providers, research, strategy, strategy_id_from, TradeAssemblyService,
    LEGAL_BOUNDARY,
};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tradeassembly_sightline_sidecar as sightline_contracts;

const ROOMS_NS: &str = "sightline_rooms";
const SURFACES_NS: &str = "sightline_surfaces";
const NODES_NS: &str = "sightline_nodes";
const SELECTIONS_NS: &str = "sightline_selections";
const SELECTION_POINTERS_NS: &str = "sightline_selection_pointers";
const SNAPSHOTS_NS: &str = "sightline_context_snapshots";
const AGENTS_NS: &str = "sightline_agent_connections";
const PRESENCE_NS: &str = "sightline_presence";
const COMMANDS_NS: &str = "sightline_commands";
const EVENTS_NS: &str = "sightline_events";
const APPROVALS_NS: &str = "sightline_approvals";
const PROPOSALS_NS: &str = "sightline_proposals";
const APP_ID: &str = "tradeassembly_studio";
const ROOM_ID: &str = "room_tradeassembly_local";
const LOCAL_TIMESTAMP: &str = "2026-07-08T00:00:00Z";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StudioSurfaceKind {
    Dashboard,
    StrategyLibrary,
    StrategyOverview,
    StrategyEditor,
    Backtest,
    Robustness,
    Execution,
    PluginLibrary,
    ResearchRollup,
    RunCenter,
    Journal,
    Review,
}

impl StudioSurfaceKind {
    fn id(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::StrategyLibrary => "strategy_library",
            Self::StrategyOverview => "strategy_overview",
            Self::StrategyEditor => "strategy_editor",
            Self::Backtest => "backtest",
            Self::Robustness => "robustness",
            Self::Execution => "execution",
            Self::PluginLibrary => "plugin_library",
            Self::ResearchRollup => "research_rollup",
            Self::RunCenter => "run_center",
            Self::Journal => "journal",
            Self::Review => "review",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::StrategyLibrary => "Strategies",
            Self::StrategyOverview => "Strategy",
            Self::StrategyEditor => "Strategy Editor",
            Self::Backtest => "Backtest Workspace",
            Self::Robustness => "Robustness Workspace",
            Self::Execution => "Execution Workspace",
            Self::PluginLibrary => "Plugins",
            Self::ResearchRollup => "Research",
            Self::RunCenter => "Run Center",
            Self::Journal => "Journal",
            Self::Review => "Replay Evidence",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StudioRoute {
    route: String,
    kind: StudioSurfaceKind,
    strategy_id: Option<String>,
    resource_id: Option<String>,
    query: std::collections::BTreeMap<String, String>,
    high_authority: bool,
}

pub(crate) fn session_state(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let actor = agent_actor_from_body(&body);
    let selection = current_selection(service, &body);
    let events = list_events_payload(service, &body);
    let event_cursor = events
        .iter()
        .filter_map(|event| event["sequence"].as_u64())
        .max()
        .unwrap_or(0);
    api_result(json!({
        "schemaVersion": "tradeassembly.sightline.session_state.v1",
        "room": room(service),
        "surfaces": list_values(service, SURFACES_NS),
        "nodes": list_values(service, NODES_NS),
        "contextSnapshots": list_values(service, SNAPSHOTS_NS),
        "commands": list_values(service, COMMANDS_NS),
        "selection": selection_for_actor(&selection, &actor),
        "agentPresence": list_values(service, PRESENCE_NS),
        "agents": list_values(service, AGENTS_NS),
        "proposals": list_values(service, PROPOSALS_NS),
        "approvals": list_values(service, APPROVALS_NS),
        "events": events,
        "eventCursor": event_cursor,
        "redaction": redaction_summary(),
        "authority": authority_summary(),
        "contractSource": sightline_contracts::contract_source(),
        "transport": require_contract(sightline_contracts::transport_contract()),
        "noAdvice": LEGAL_BOUNDARY,
    }))
}

pub(crate) fn register_surface(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let strategy_id = optional_string(&body, &["strategyId", "strategy_id"])
        .unwrap_or_else(|| "strat_local_btc_demo".to_string());
    let surface_id = optional_string(&body, &["surfaceId", "surface_id"])
        .unwrap_or_else(|| strategy_editor_surface_id(&strategy_id));
    let route = optional_string(&body, &["route"])
        .unwrap_or_else(|| format!("/app/strategies/{strategy_id}/builder"));
    let surface = require_contract(sightline_contracts::conform_surface(&json!({
        "surface_id": surface_id,
        "surfaceId": surface_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "app_id": APP_ID,
        "appId": APP_ID,
        "user_ref": user_actor(&body),
        "userRef": user_actor(&body),
        "route": route,
        "title": optional_string(&body, &["title"]).unwrap_or_else(|| "Strategy Editor".to_string()),
        "active_object": strategy_object_ref(&strategy_id, None),
        "activeObject": strategy_object_ref(&strategy_id, None),
        "root_node_id": format!("strategy.{strategy_id}.root"),
        "rootNodeId": format!("strategy.{strategy_id}.root"),
        "visible_snapshot_id": format!("snap_{}", slug(&surface_id)),
        "visibleSnapshotId": format!("snap_{}", slug(&surface_id)),
        "status": "active",
        "follow_mode": "independent",
        "followMode": "independent",
        "last_seen_at": LOCAL_TIMESTAMP,
        "lastSeenAt": LOCAL_TIMESTAMP,
    })));
    persist(
        service,
        SURFACES_NS,
        &surface_id,
        surface.clone(),
        "sightline.surface.registered",
    );
    append_event(
        service,
        "surface.connected",
        json!({"surface_id": surface_id, "route": route}),
        Some(surface_id.clone()),
        Some(user_actor(&body)),
    );
    api_result(
        json!({"surface": surface, "room": room(service), "events": list_events_payload(service, &body)}),
    )
}

pub(crate) fn publish_strategy_editor_surface(
    service: &TradeAssemblyService,
    body: Value,
) -> Value {
    let strategy_id = strategy_id_from(&body);
    publish_studio_surface(
        service,
        json!({"route": format!("/app/strategies/{strategy_id}/builder")}),
    )
}

pub(crate) fn publish_studio_surface(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let Some(route) = optional_string(&body, &["route"]) else {
        return sightline_route_error("sightline_route_required");
    };
    let descriptor = match parse_studio_route(&route) {
        Ok(descriptor) => descriptor,
        Err(code) => return sightline_route_error(code),
    };
    let surface_id = studio_surface_id(&descriptor);
    let active_object = studio_object_ref(&descriptor);
    let root_node_id = format!("{}.root", surface_id);
    let snapshot_id = format!("snap_{surface_id}");
    let surface = require_contract(sightline_contracts::conform_surface(&json!({
        "surface_id": surface_id,
        "surfaceId": surface_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "app_id": APP_ID,
        "appId": APP_ID,
        "user_ref": user_actor(&json!({})),
        "userRef": user_actor(&json!({})),
        "route": descriptor.route,
        "title": descriptor.kind.title(),
        "surface_kind": descriptor.kind.id(),
        "surfaceKind": descriptor.kind.id(),
        "strategy_id": descriptor.strategy_id,
        "strategyId": descriptor.strategy_id,
        "resource_id": descriptor.resource_id,
        "resourceId": descriptor.resource_id,
        "active_object": active_object,
        "activeObject": active_object,
        "root_node_id": root_node_id,
        "rootNodeId": root_node_id,
        "visible_snapshot_id": snapshot_id,
        "visibleSnapshotId": snapshot_id,
        "status": "active",
        "follow_mode": "independent",
        "followMode": "independent",
        "last_seen_at": LOCAL_TIMESTAMP,
        "lastSeenAt": LOCAL_TIMESTAMP,
    })));
    let nodes = studio_surface_nodes(service, &descriptor, &surface_id);
    let snapshot = studio_context_snapshot(&surface, &descriptor, &nodes);
    let content_hash = hash_value(&json!({
        "surface": surface,
        "nodes": nodes,
        "snapshot": snapshot,
    }));
    let unchanged = service
        .runtime()
        .storage
        .get_json(SNAPSHOTS_NS, &snapshot_id)
        .ok()
        .flatten()
        .and_then(|stored| stored["contentHash"].as_str().map(str::to_string))
        .as_deref()
        == Some(content_hash.as_str());
    let mut snapshot = snapshot;
    snapshot["contentHash"] = json!(content_hash);
    if !unchanged {
        persist_versioned(
            service,
            SURFACES_NS,
            &surface_id,
            surface.clone(),
            "sightline.surface.published",
            &content_hash,
        );
        for node in &nodes {
            let node_id = node["node_id"].as_str().unwrap_or("node");
            persist_versioned(
                service,
                NODES_NS,
                &node_key(&surface_id, node_id),
                node.clone(),
                "sightline.node.published",
                &content_hash,
            );
        }
        persist_versioned(
            service,
            SNAPSHOTS_NS,
            &snapshot_id,
            snapshot.clone(),
            "sightline.snapshot.published",
            &content_hash,
        );
        append_event(
            service,
            "surface.snapshot.updated",
            json!({
                "snapshot_id": snapshot_id,
                "content_hash": content_hash,
                "node_count": nodes.len(),
                "route": descriptor.route,
            }),
            Some(surface_id.clone()),
            Some(user_actor(&json!({}))),
        );
    }
    api_result(json!({
        "room": room(service),
        "surface": surface,
        "nodes": nodes,
        "snapshot": snapshot,
        "changed": !unchanged,
        "redaction": redaction_summary(),
        "noAdvice": LEGAL_BOUNDARY,
    }))
}

pub(crate) fn set_selection(
    service: &TradeAssemblyService,
    body: Value,
    selected_by: Value,
) -> Value {
    ensure_room(service);
    let strategy_id = strategy_id_from(&body);
    let surface_id = optional_string(&body, &["surfaceId", "surface_id"])
        .unwrap_or_else(|| strategy_editor_surface_id(&strategy_id));
    let requested_node_ref = optional_string(&body, &["nodeId", "node_id", "nodeRef", "node_ref"])
        .unwrap_or_else(|| format!("strategy.{strategy_id}.root"));
    let resolved_node_ref = selection_node_ref_alias(&requested_node_ref);
    let Some(node) = node_for(service, &surface_id, &requested_node_ref)
        .or_else(|| node_for_spec_path(service, &surface_id, resolved_node_ref))
    else {
        return json!({
            "ok": false,
            "body": {"selection": Value::Null, "context": Value::Null},
            "error": {"code": "sightline_node_not_registered"},
        });
    };
    let node_id = node["node_id"]
        .as_str()
        .unwrap_or(&requested_node_ref)
        .to_string();
    let selection_id = optional_string(&body, &["selectionId", "selection_id"])
        .unwrap_or_else(|| format!("selection_{}_{}", slug(&surface_id), slug(&node_id)));
    let context_level = body
        .get("contextLevel")
        .or_else(|| body.get("context_level"))
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .min(2);
    let shared = body.get("shared").and_then(Value::as_bool).unwrap_or(false);
    let selection = require_contract(sightline_contracts::conform_selection(&json!({
        "selection_id": selection_id,
        "selectionId": selection_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "surface_id": surface_id,
        "surfaceId": surface_id,
        "selected_by": selected_by,
        "selectedBy": selected_by,
        "node_ref": {"surface_id": surface_id, "node_id": node_id},
        "nodeRef": {"surfaceId": surface_id, "nodeId": node_id},
        "object_refs": node["object_refs"].clone(),
        "objectRefs": node["object_refs"].clone(),
        "summary": node["summary"].as_str().unwrap_or("Selected StrategySpec context"),
        "context_level": context_level,
        "contextLevel": context_level,
        "redaction_policy_ref": "tradeassembly.redaction.agent_safe.v1",
        "redactionPolicyRef": "tradeassembly.redaction.agent_safe.v1",
        "shared": shared,
        "pinned": body.get("pinned").and_then(Value::as_bool).unwrap_or(false),
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "evidenceRefs": evidence_refs_for(&[ROOM_ID, &surface_id, &node_id, &selection_id]),
    })));
    persist(
        service,
        SELECTIONS_NS,
        &selection_id,
        selection.clone(),
        "sightline.selection.changed",
    );
    persist_pointer(
        service,
        &selection_pointer_key("current", Some(&surface_id)),
        json!({"selection_id": selection_id, "surface_id": surface_id}),
    );
    persist_pointer(
        service,
        "current",
        json!({"selection_id": selection_id, "surface_id": surface_id}),
    );
    if shared {
        persist_pointer(
            service,
            &selection_pointer_key("shared", Some(&surface_id)),
            json!({"selection_id": selection_id, "surface_id": surface_id}),
        );
        persist_pointer(
            service,
            "shared",
            json!({"selection_id": selection_id, "surface_id": surface_id}),
        );
    }
    append_event(
        service,
        "selection.changed",
        json!({"selection_id": selection_id, "shared": shared, "node_id": node_id}),
        Some(surface_id),
        Some(selected_by),
    );
    api_result(
        json!({"selection": selection, "context": context_for_selection(service, &selection), "redaction": redaction_summary()}),
    )
}

pub(crate) fn share_selection(
    service: &TradeAssemblyService,
    body: Value,
    sharing_user: Value,
) -> Value {
    let selection_id = optional_string(&body, &["selectionId", "selection_id"])
        .or_else(|| {
            current_selection(service, &body)["selection_id"]
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "selection_none".to_string());
    let Some(mut selection) = service
        .runtime()
        .storage
        .get_json(SELECTIONS_NS, &selection_id)
        .ok()
        .flatten()
    else {
        return json!({
            "ok": false,
            "body": {"selectionId": selection_id},
            "error": {"code": "sightline_selection_not_found"},
        });
    };
    selection["shared"] = json!(true);
    selection["sharedAt"] = json!(LOCAL_TIMESTAMP);
    persist(
        service,
        SELECTIONS_NS,
        &selection_id,
        selection.clone(),
        "sightline.selection.shared",
    );
    let surface_id = selection["surface_id"].as_str().unwrap_or_default();
    persist_pointer(
        service,
        &selection_pointer_key("shared", Some(surface_id)),
        json!({"selection_id": selection_id, "surface_id": surface_id}),
    );
    persist_pointer(
        service,
        "shared",
        json!({"selection_id": selection_id, "surface_id": surface_id}),
    );
    append_event(
        service,
        "selection.shared",
        json!({"selection_id": selection_id}),
        selection["surface_id"].as_str().map(str::to_string),
        Some(sharing_user),
    );
    api_result(
        json!({"selection": selection, "context": context_for_selection(service, &selection), "redaction": redaction_summary()}),
    )
}

pub(crate) fn get_context(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let selection = current_selection(service, &body);
    if selection.is_null() {
        return json!({
            "ok": false,
            "body": {"context": Value::Null},
            "error": {"code": "sightline_selection_not_found"},
        });
    }
    let actor = agent_actor_from_body(&body);
    if selection_for_actor(&selection, &actor).is_null() {
        return json!({
            "ok": false,
            "body": {"context": Value::Null},
            "error": {"code": "sightline_selection_not_shared"},
        });
    }
    let context = context_for_selection(service, &selection);
    append_event(
        service,
        "context.read",
        json!({
            "selection_id": selection["selection_id"],
            "context_level": context["context_level"],
            "redaction_policy_ref": context["redaction_policy_ref"],
        }),
        selection["surface_id"].as_str().map(str::to_string),
        Some(actor),
    );
    api_result(json!({
        "context": context,
        "selection": selection,
        "redaction": redaction_summary(),
        "noAdvice": LEGAL_BOUNDARY,
    }))
}

pub(crate) fn connect_agent(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let actor = agent_actor_from_body(&body);
    let agent_id = actor["actor_id"]
        .as_str()
        .unwrap_or("agent_local")
        .to_string();
    let connection_id = optional_string(&body, &["agentConnectionId", "agent_connection_id"])
        .unwrap_or_else(|| format!("agent_connection_{}", slug(&agent_id)));
    let scopes = body.get("scopes").cloned().unwrap_or_else(|| {
        json!([
            "observe_surface",
            "read_selection",
            "read_context_level_2",
            "create_proposal"
        ])
    });
    let connection = require_contract(sightline_contracts::conform_agent_connection(&json!({
        "agent_connection_id": connection_id,
        "agentConnectionId": connection_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "agent_ref": actor,
        "agentRef": actor,
        "client_name": actor["client_name"].as_str().unwrap_or("TradeAssembly local agent"),
        "clientName": actor["client_name"].as_str().unwrap_or("TradeAssembly local agent"),
        "scopes": scopes,
        "presence_color": "#0b61ff",
        "presenceColor": "#0b61ff",
        "status": "connected",
        "current_selection_id": selection_for_actor(&current_selection(service, &body), &actor)["selection_id"].clone(),
        "currentSelectionId": selection_for_actor(&current_selection(service, &body), &actor)["selection_id"].clone(),
        "last_seen_at": LOCAL_TIMESTAMP,
        "lastSeenAt": LOCAL_TIMESTAMP,
    })));
    persist(
        service,
        AGENTS_NS,
        &connection_id,
        connection.clone(),
        "sightline.agent.connected",
    );
    let presence = presence_record(&body, "inspecting");
    let presence_id = presence["presence_id"]
        .as_str()
        .unwrap_or("presence")
        .to_string();
    persist(
        service,
        PRESENCE_NS,
        &presence_id,
        presence.clone(),
        "sightline.presence.changed",
    );
    append_event(
        service,
        "agent.connected",
        json!({"agent_connection_id": connection_id}),
        None,
        Some(actor),
    );
    api_result(json!({"agent": connection, "presence": presence, "room": room(service)}))
}

pub(crate) fn update_presence(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let status = optional_string(&body, &["status"]).unwrap_or_else(|| "viewing".to_string());
    let presence = presence_record(&body, &status);
    let presence_id = presence["presence_id"]
        .as_str()
        .unwrap_or("presence")
        .to_string();
    persist(
        service,
        PRESENCE_NS,
        &presence_id,
        presence.clone(),
        "sightline.presence.changed",
    );
    append_event(
        service,
        "presence.changed",
        json!({"presence_id": presence_id, "status": status}),
        presence["surface_id"].as_str().map(str::to_string),
        Some(agent_actor_from_body(&body)),
    );
    api_result(json!({"presence": presence}))
}

pub(crate) fn request_navigation(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let surface_id = optional_string(&body, &["surfaceId", "surface_id"])
        .unwrap_or_else(|| strategy_editor_surface_id(&strategy_id_from(&body)));
    let Some(surface) = service
        .runtime()
        .storage
        .get_json(SURFACES_NS, &surface_id)
        .ok()
        .flatten()
    else {
        return sightline_navigation_error("sightline_surface_not_found");
    };
    if surface["status"].as_str() != Some("active") || surface["room_id"].as_str() != Some(ROOM_ID)
    {
        return sightline_navigation_error("sightline_surface_inactive");
    }
    let Some(route) = optional_string(&body, &["route"])
        .or_else(|| surface["route"].as_str().map(str::to_string))
    else {
        return sightline_navigation_error("sightline_route_required");
    };
    let destination = match parse_studio_route(&route) {
        Ok(destination) => destination,
        Err(code) => return sightline_navigation_error(code),
    };
    if destination.high_authority {
        return sightline_navigation_error("sightline_navigation_user_required");
    }
    if !navigation_in_surface_scope(&surface, &destination) {
        return sightline_navigation_error("sightline_navigation_scope_mismatch");
    }
    let mut normalized_body = body.clone();
    if let Value::Object(values) = &mut normalized_body {
        values.insert("surfaceId".to_string(), json!(surface_id));
        values.insert("route".to_string(), json!(destination.route));
        if let Some(strategy_id) = destination.strategy_id.as_deref() {
            values.insert("strategyId".to_string(), json!(strategy_id));
        }
    }
    let mut command = semantic_command(service, &normalized_body, "navigate_surface", "navigate");
    command["target"]["object_ref"] = surface["active_object"].clone();
    command["target"]["objectRef"] = surface["active_object"].clone();
    let command_id = command["command_id"]
        .as_str()
        .unwrap_or("command")
        .to_string();
    if let Ok(Some(existing)) = service.runtime().storage.get_json(COMMANDS_NS, &command_id) {
        return api_result(json!({
            "command": existing,
            "replayed": true,
            "blockedHighAuthority": false,
            "audit": {"requestingPrincipal": agent_actor_from_body(&body), "roomId": ROOM_ID},
        }));
    }
    persist(
        service,
        COMMANDS_NS,
        &command_id,
        command.clone(),
        "sightline.command.requested",
    );
    append_event(
        service,
        "agent.command.requested",
        json!({"command_id": command_id, "command_type": "navigate_surface"}),
        command["target"]["surface_id"].as_str().map(str::to_string),
        Some(agent_actor_from_body(&body)),
    );
    api_result(json!({
        "command": command,
        "replayed": false,
        "blockedHighAuthority": false,
        "audit": {"requestingPrincipal": agent_actor_from_body(&body), "roomId": ROOM_ID},
    }))
}

pub(crate) fn focus_node(service: &TradeAssemblyService, body: Value) -> Value {
    audited_node_command(service, body, "focus_node", "focus")
}

pub(crate) fn highlight_node(service: &TradeAssemblyService, body: Value) -> Value {
    audited_node_command(service, body, "highlight_node", "highlight")
}

pub(crate) fn create_proposal(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let actor = agent_actor_from_body(&body);
    let selection = current_selection(service, &body);
    let Some(selection) = shared_selection_for_actor(&selection, &actor) else {
        return selection_not_shared_error();
    };
    let strategy_id = strategy_id_from(&body);
    let sightline_refs = sightline_refs_from_body(&body, &selection);
    let mut strategy_body = body.clone();
    if let Value::Object(map) = &mut strategy_body {
        map.insert("strategyId".to_string(), json!(strategy_id));
        map.entry("selectionRef".to_string())
            .or_insert_with(|| selection["selection_id"].clone());
        map.insert("actor".to_string(), actor.clone());
        map.entry("purpose".to_string())
            .or_insert_with(|| json!("strategy_authoring"));
        map.insert("sightlineRefs".to_string(), json!(sightline_refs.clone()));
        map.entry("evidenceRefs".to_string()).or_insert_with(|| {
            json!(evidence_refs_for(
                &sightline_refs
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            ))
        });
    }
    let result = strategy::propose_strategy_patch(service, strategy_body.clone());
    let proposal = result["body"]["proposal"].clone();
    if !proposal.is_null() {
        let proposal_id = proposal["id"]
            .as_str()
            .unwrap_or("proposal_local")
            .to_string();
        let sightline_proposal =
            sightline_proposal(&proposal, &strategy_body, &selection, &sightline_refs);
        persist(
            service,
            PROPOSALS_NS,
            &proposal_id,
            sightline_proposal.clone(),
            "sightline.proposal.created",
        );
        append_event(
            service,
            "proposal.created",
            json!({"proposal_id": proposal_id, "strategy_proposal": proposal["id"]}),
            selection["surface_id"].as_str().map(str::to_string),
            Some(actor),
        );
        return api_result(json!({
            "proposal": sightline_proposal,
            "strategyProposal": redact_sensitive_json(&proposal),
            "selection": selection,
            "redaction": redaction_summary(),
            "sightlineRefs": sightline_refs,
        }));
    }
    result
}

pub(crate) fn request_approval(service: &TradeAssemblyService, body: Value) -> Value {
    ensure_room(service);
    let actor = agent_actor_from_body(&body);
    let selection = current_selection(service, &body);
    let Some(selection) = shared_selection_for_actor(&selection, &actor) else {
        return selection_not_shared_error();
    };
    let approval_id = optional_string(&body, &["approvalId", "approval_id"])
        .unwrap_or_else(|| format!("approval_{}", short_hash(&hash_value(&body))));
    let target = body
        .get("target")
        .and_then(|target| {
            sightline_contracts::conform_object_ref(&redact_sensitive_json(target)).ok()
        })
        .or_else(|| {
            selection["object_refs"]
                .as_array()
                .and_then(|refs| refs.first().cloned())
        })
        .unwrap_or_else(|| strategy_object_ref(&strategy_id_from(&body), None));
    let approval = require_contract(sightline_contracts::conform_approval(&json!({
        "approval_id": approval_id,
        "approvalId": approval_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "requested_by": actor,
        "requestedBy": actor,
        "action": optional_string(&body, &["action"]).unwrap_or_else(|| "review_proposal".to_string()),
        "target": target,
        "authority_level": optional_string(&body, &["authorityLevel", "authority_level"]).unwrap_or_else(|| "mutate".to_string()),
        "authorityLevel": optional_string(&body, &["authorityLevel", "authority_level"]).unwrap_or_else(|| "mutate".to_string()),
        "summary": optional_string(&body, &["summary"]).unwrap_or_else(|| "Approval requested for user review.".to_string()),
        "consequences": body.get("consequences").cloned().unwrap_or_else(|| json!(["TradeAssembly will re-check authority before any durable mutation."])),
        "status": "pending",
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "sightlineRefs": sightline_refs_from_body(&body, &selection),
        "humanOnly": high_authority_approval(&body),
        "agentCannotSelfApprove": true,
    })));
    persist(
        service,
        APPROVALS_NS,
        &approval_id,
        approval.clone(),
        "sightline.approval.requested",
    );
    append_event(
        service,
        "approval.requested",
        json!({"approval_id": approval_id, "human_only": high_authority_approval(&body)}),
        selection["surface_id"].as_str().map(str::to_string),
        Some(actor),
    );
    api_result(
        json!({"approval": approval, "selection": selection, "redaction": redaction_summary()}),
    )
}

pub(crate) fn list_events(service: &TradeAssemblyService, body: Value) -> Value {
    api_result(json!({"events": list_events_payload(service, &body), "truncated": false}))
}

pub(crate) fn current_selection_context(service: &TradeAssemblyService, body: Value) -> Value {
    let selection = current_selection(service, &body);
    if selection.is_null() {
        return json!({
            "ok": false,
            "body": {"selection": Value::Null},
            "error": {"code": "sightline_selection_not_found"},
        });
    }
    let actor = agent_actor_from_body(&body);
    if selection_for_actor(&selection, &actor).is_null() {
        return json!({
            "ok": false,
            "body": {"selection": Value::Null, "context": Value::Null},
            "error": {"code": "sightline_selection_not_shared"},
        });
    }
    api_result(
        json!({"selection": selection, "context": context_for_selection(service, &selection)}),
    )
}

pub(crate) fn reset(service: &TradeAssemblyService) -> Result<Vec<&'static str>, String> {
    clear_reset_namespaces_with(|namespace| service.runtime().storage.clear_namespace(namespace))
}

fn clear_reset_namespaces_with(
    mut clear: impl FnMut(&str) -> Result<(), String>,
) -> Result<Vec<&'static str>, String> {
    let namespaces = [
        ROOMS_NS,
        SURFACES_NS,
        NODES_NS,
        SELECTIONS_NS,
        SELECTION_POINTERS_NS,
        SNAPSHOTS_NS,
        AGENTS_NS,
        PRESENCE_NS,
        COMMANDS_NS,
        EVENTS_NS,
        APPROVALS_NS,
        PROPOSALS_NS,
    ];
    for namespace in namespaces {
        clear(namespace)?;
    }
    Ok(namespaces.to_vec())
}

fn ensure_room(service: &TradeAssemblyService) -> Value {
    if let Ok(Some(room)) = service.runtime().storage.get_json(ROOMS_NS, ROOM_ID) {
        return room;
    }
    let room = require_contract(sightline_contracts::conform_room(&json!({
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "app_id": APP_ID,
        "appId": APP_ID,
        "workspace_id": "workspace_local",
        "workspaceId": "workspace_local",
        "owner_user_ref": user_actor(&json!({})),
        "ownerUserRef": user_actor(&json!({})),
        "status": "active",
        "policy_ref": "tradeassembly.local_sightline_policy.v1",
        "policyRef": "tradeassembly.local_sightline_policy.v1",
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "updated_at": LOCAL_TIMESTAMP,
        "updatedAt": LOCAL_TIMESTAMP,
    })));
    persist(
        service,
        ROOMS_NS,
        ROOM_ID,
        room.clone(),
        "sightline.room.created",
    );
    append_event(
        service,
        "room.created",
        json!({"room_id": ROOM_ID}),
        None,
        Some(user_actor(&json!({}))),
    );
    room
}

fn room(service: &TradeAssemblyService) -> Value {
    ensure_room(service)
}

fn parse_studio_route(route: &str) -> Result<StudioRoute, &'static str> {
    if route.is_empty()
        || route.len() > 512
        || !route.is_ascii()
        || route.contains("://")
        || route.contains('\\')
        || route.contains("..")
        || route.contains('%')
        || route.contains('#')
    {
        return Err("sightline_route_invalid");
    }
    let (path, query) = route.split_once('?').unwrap_or((route, ""));
    if !path.starts_with("/app") || path[4..].starts_with("//") || path.contains("//") {
        return Err("sightline_route_invalid");
    }
    let query = parse_studio_query(query)?;
    let segments = path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let (kind, strategy_id, resource_id) = match segments.as_slice() {
        ["app"] | ["app", "dashboard"] => (StudioSurfaceKind::Dashboard, None, None),
        ["app", "strategies"] => (StudioSurfaceKind::StrategyLibrary, None, None),
        ["app", "strategies", strategy_id] if valid_route_id(strategy_id) => (
            StudioSurfaceKind::StrategyOverview,
            Some((*strategy_id).to_string()),
            None,
        ),
        ["app", "strategies", strategy_id, "builder"] if valid_route_id(strategy_id) => (
            StudioSurfaceKind::StrategyEditor,
            Some((*strategy_id).to_string()),
            None,
        ),
        ["app", "strategies", strategy_id, "research"] if valid_route_id(strategy_id) => (
            if query.get("view").map(String::as_str) == Some("monte-carlo") {
                StudioSurfaceKind::Robustness
            } else {
                StudioSurfaceKind::Backtest
            },
            Some((*strategy_id).to_string()),
            None,
        ),
        ["app", "strategies", strategy_id, "run" | "execution"] if valid_route_id(strategy_id) => (
            StudioSurfaceKind::Execution,
            Some((*strategy_id).to_string()),
            None,
        ),
        ["app", "plugins-providers"] | ["app", "admin", "plugin-manifests"] => {
            (StudioSurfaceKind::PluginLibrary, None, None)
        }
        ["app", "research"] => (StudioSurfaceKind::ResearchRollup, None, None),
        ["app", "run"] => (StudioSurfaceKind::RunCenter, None, None),
        ["app", "alerts"] => (StudioSurfaceKind::Journal, None, None),
        ["app", "reviews", snapshot_id] if valid_route_id(snapshot_id) => (
            StudioSurfaceKind::Review,
            query.get("strategyId").cloned(),
            Some((*snapshot_id).to_string()),
        ),
        _ => return Err("sightline_route_unsupported"),
    };
    let high_authority = query.get("mode").map(String::as_str) == Some("live");
    Ok(StudioRoute {
        route: route.to_string(),
        kind,
        strategy_id,
        resource_id,
        query,
        high_authority,
    })
}

fn parse_studio_query(
    query: &str,
) -> Result<std::collections::BTreeMap<String, String>, &'static str> {
    let mut parsed = std::collections::BTreeMap::new();
    if query.is_empty() {
        return Ok(parsed);
    }
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            return Err("sightline_route_invalid");
        };
        let allowed = match key {
            "view" => matches!(value, "backtest" | "monte-carlo"),
            "mode" => matches!(value, "paper" | "live"),
            "activation" | "activationId" | "strategyId" => valid_route_id(value),
            _ => false,
        };
        if !allowed || parsed.insert(key.to_string(), value.to_string()).is_some() {
            return Err("sightline_route_invalid");
        }
    }
    Ok(parsed)
}

fn valid_route_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn studio_surface_id(descriptor: &StudioRoute) -> String {
    if let Some(resource_id) = descriptor.resource_id.as_deref() {
        return format!("surface_{}_{}", descriptor.kind.id(), slug(resource_id));
    }
    descriptor
        .strategy_id
        .as_ref()
        .map(|strategy_id| format!("surface_{}_{}", descriptor.kind.id(), slug(strategy_id)))
        .unwrap_or_else(|| format!("surface_{}", descriptor.kind.id()))
}

fn studio_object_ref(descriptor: &StudioRoute) -> Value {
    if let Some(resource_id) = descriptor.resource_id.as_deref() {
        return json!({
            "app_id": APP_ID,
            "appId": APP_ID,
            "object_type": descriptor.kind.id(),
            "objectType": descriptor.kind.id(),
            "object_id": resource_id,
            "objectId": resource_id,
            "path": format!("tradeassembly://review/{resource_id}"),
        });
    }
    if let Some(strategy_id) = descriptor.strategy_id.as_deref() {
        let mut object = strategy_object_ref(strategy_id, None);
        object["object_type"] = json!(descriptor.kind.id());
        object["objectType"] = json!(descriptor.kind.id());
        object["path"] = json!(format!(
            "tradeassembly://strategy/{strategy_id}/{}",
            descriptor.kind.id()
        ));
        object
    } else {
        json!({
            "app_id": APP_ID,
            "appId": APP_ID,
            "object_type": descriptor.kind.id(),
            "objectType": descriptor.kind.id(),
            "object_id": descriptor.kind.id(),
            "objectId": descriptor.kind.id(),
            "path": format!("tradeassembly://workspace/{}", descriptor.kind.id()),
        })
    }
}

fn studio_surface_nodes(
    service: &TradeAssemblyService,
    descriptor: &StudioRoute,
    surface_id: &str,
) -> Vec<Value> {
    if descriptor.kind == StudioSurfaceKind::StrategyEditor {
        let strategy_id = descriptor.strategy_id.as_deref().unwrap_or_default();
        let draft = strategy::draft_value(service, strategy_id);
        return strategy_editor_nodes(strategy_id, surface_id, &draft);
    }
    let state = studio_surface_state(service, descriptor);
    let object_ref = studio_object_ref(descriptor);
    let mut nodes = vec![studio_semantic_node(
        surface_id,
        format!("{surface_id}.root"),
        format!("studio.{}", descriptor.kind.id()),
        descriptor.kind.title(),
        studio_surface_summary(descriptor.kind),
        object_ref.clone(),
        state.clone(),
    )];
    let scope_id = descriptor.strategy_id.as_deref();
    match descriptor.kind {
        StudioSurfaceKind::Backtest => {
            nodes.extend([
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "backtest",
                    "dataset",
                    "studio.backtest.dataset",
                    "Historical dataset",
                    "Selected immutable dataset and source binding.",
                    object_ref.clone(),
                    state["selectedDataset"].clone(),
                ),
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "backtest",
                    "run",
                    "studio.backtest.run",
                    "Backtest run",
                    "Selected run configuration and lifecycle state.",
                    object_ref.clone(),
                    select_fields(
                        &state["selectedBacktest"],
                        &[
                            "id",
                            "runId",
                            "status",
                            "state",
                            "datasetId",
                            "strategyVersionId",
                            "createdAt",
                            "completedAt",
                        ],
                    ),
                ),
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "backtest",
                    "report",
                    "studio.backtest.report",
                    "Backtest report",
                    "Immutable accounting, trade, and report evidence.",
                    object_ref.clone(),
                    select_fields(
                        &state["selectedBacktest"]["report"],
                        &["schemaVersion", "header", "overview", "status", "integrity"],
                    ),
                ),
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "backtest",
                    "journal",
                    "studio.backtest.journal",
                    "Backtest journal",
                    "Lifecycle and trade journal evidence for the selected run.",
                    object_ref,
                    json!({"journalCount": state["journalCount"]}),
                ),
            ]);
        }
        StudioSurfaceKind::Execution => {
            nodes.extend([
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "execution",
                    "activation",
                    "studio.execution.activation",
                    "Paper activation",
                    "Selected paper activation and durable run state.",
                    object_ref.clone(),
                    json!({
                        "selectedActivationId": state["selectedActivationId"],
                        "selection": state["selection"],
                        "activeRun": state["activeRun"],
                        "scheduler": state["scheduler"],
                    }),
                ),
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "execution",
                    "health",
                    "studio.execution.health",
                    "Execution health",
                    "Runtime health, scheduler, and reconciliation state.",
                    object_ref.clone(),
                    json!({
                        "health": state["activeRun"]["health"],
                        "scheduler": state["scheduler"],
                        "reconciliation": state["reconciliation"],
                    }),
                ),
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "execution",
                    "positions",
                    "studio.execution.positions",
                    "Positions and risk",
                    "Current positions, performance, and risk evidence.",
                    object_ref.clone(),
                    json!({
                        "positionCount": value_count(&state["positions"]),
                        "performanceSummary": state["performanceSummary"],
                        "risk": select_fields(&state["risk"], &[
                            "available", "activeCount", "activeNotional", "reason"
                        ]),
                    }),
                ),
                studio_detail_node(
                    surface_id,
                    scope_id,
                    "execution",
                    "journal",
                    "studio.execution.journal",
                    "Execution journal",
                    "Decision and control journal evidence for the selected activation.",
                    object_ref,
                    json!({"journalCount": value_count(&state["journal"])}),
                ),
            ]);
        }
        StudioSurfaceKind::PluginLibrary => {
            nodes.push(studio_detail_node(
                surface_id,
                None,
                "plugins",
                "inventory",
                "studio.plugin.inventory",
                "Installed plugins",
                "Installed plugin identities, capabilities, health, and credential presence.",
                object_ref.clone(),
                json!({"pluginCount": state["pluginCount"]}),
            ));
            if let Some(plugins) = state["plugins"].as_array() {
                for plugin in plugins {
                    let instance_ref = plugin["instanceRef"]
                        .as_str()
                        .or_else(|| plugin["ref"].as_str())
                        .or_else(|| plugin["pluginRef"].as_str())
                        .unwrap_or("unknown");
                    nodes.push(studio_detail_node(
                        surface_id,
                        None,
                        "plugin",
                        &slug(instance_ref),
                        "studio.plugin.instance",
                        "Plugin instance",
                        "Installed plugin manifest, capability, health, and credential status.",
                        object_ref.clone(),
                        plugin.clone(),
                    ));
                }
            }
        }
        _ => {}
    }
    nodes
}

fn studio_surface_state(service: &TradeAssemblyService, descriptor: &StudioRoute) -> Value {
    let state = match descriptor.kind {
        StudioSurfaceKind::Dashboard | StudioSurfaceKind::StrategyLibrary => {
            let strategies = strategy::strategies(service)
                .iter()
                .map(safe_strategy_value)
                .collect::<Vec<_>>();
            json!({
                "strategies": strategies,
                "strategyCount": strategies.len(),
            })
        }
        StudioSurfaceKind::StrategyOverview => {
            let strategy_id = descriptor.strategy_id.as_deref().unwrap_or_default();
            json!({"strategy": safe_strategy_value(&strategy::strategy_value(service, strategy_id))})
        }
        StudioSurfaceKind::StrategyEditor => Value::Null,
        StudioSurfaceKind::Backtest => {
            let strategy_id = descriptor.strategy_id.as_deref().unwrap_or_default();
            let workspace = research::workspace(service, strategy_id);
            json!({
                "strategy": safe_strategy_value(&workspace["strategy"]),
                "selectedBacktest": select_fields(&workspace["selectedBacktest"], &[
                    "id", "runId", "status", "state", "datasetId", "strategyVersionId",
                    "createdAt", "completedAt", "report"
                ]),
                "selectedDataset": select_fields(&workspace["datasetSetup"]["selectedDataset"], &[
                    "id", "datasetId", "symbol", "status", "sourcePluginRef", "startAt", "endAt",
                    "rowCount", "snapshotHash"
                ]),
                "datasetCount": value_count(&workspace["datasets"]),
                "backtestCount": value_count(&workspace["backtests"]),
                "artifactCount": value_count(&workspace["artifacts"]),
                "journalCount": value_count(&workspace["journal"]),
            })
        }
        StudioSurfaceKind::Robustness => {
            let strategy_id = descriptor.strategy_id.as_deref().unwrap_or_default();
            let workspace = research::workspace(service, strategy_id);
            json!({
                "strategy": safe_strategy_value(&workspace["strategy"]),
                "monteCarloReport": workspace["monteCarloReport"],
                "sweeps": workspace["sweeps"],
                "comparisonCount": value_count(&workspace["comparisons"]),
            })
        }
        StudioSurfaceKind::Execution => {
            let strategy_id = descriptor.strategy_id.as_deref().unwrap_or_default();
            let workspace = execution::workspace(
                service,
                strategy_id,
                &json!({
                    "activationId": descriptor.query.get("activation").or_else(|| descriptor.query.get("activationId")),
                }),
            );
            json!({
                "strategy": safe_strategy_value(&workspace["strategy"]),
                "selectedActivationId": workspace["selectedActivationId"],
                "selection": workspace["selection"],
                "snapshot": workspace["snapshot"],
                "activeRun": select_fields(&workspace["activeRun"], &[
                    "activationId", "state", "mode", "cycle", "checkpointId", "startedAt",
                    "stoppedAt", "readiness", "health"
                ]),
                "performanceSummary": workspace["performanceSummary"],
                "positions": workspace["positions"],
                "risk": workspace["risk"],
                "journal": workspace["journal"],
                "scheduler": workspace["scheduler"],
                "reconciliation": workspace["execution"]["reconciliation"],
            })
        }
        StudioSurfaceKind::PluginLibrary => {
            let plugins = providers::list(service)
                .iter()
                .map(|plugin| {
                    select_fields(
                        plugin,
                        &[
                            "ref",
                            "instanceRef",
                            "pluginRef",
                            "name",
                            "version",
                            "enabled",
                            "capabilities",
                            "fingerprint",
                            "credentialStatus",
                            "health",
                            "capabilityResolution",
                        ],
                    )
                })
                .collect::<Vec<_>>();
            json!({"plugins": plugins, "pluginCount": plugins.len()})
        }
        StudioSurfaceKind::ResearchRollup => {
            let rollup = research::rollup(service);
            json!({
                "recentBacktests": rollup["recentBacktests"],
                "readyForPaperCount": value_count(&rollup["readyForPaper"]),
                "repairQueueCount": value_count(&rollup["repairQueue"]),
            })
        }
        StudioSurfaceKind::RunCenter => {
            let center = execution::run_center(service);
            json!({
                "activeRuns": center["activeRuns"],
                "scheduler": center["scheduler"],
                "openRisk": center["openRisk"],
                "positionCount": value_count(&center["positions"]),
            })
        }
        StudioSurfaceKind::Journal => json!({
            "scope": "workspace",
            "summary": "Local journal and alert workspace.",
        }),
        StudioSurfaceKind::Review => {
            let snapshot_id = descriptor.resource_id.as_deref().unwrap_or_default();
            let payload = service
                .dispatch_http(
                    "POST",
                    "/product/share-snapshots/export",
                    json!({"snapshotId": snapshot_id}),
                )
                .body;
            json!({
                "snapshot": select_fields(&payload["snapshot"], &["id", "status"]),
                "strategy": safe_strategy_value(&payload["factsheet"]["strategy"]),
                "evidence": payload["evidence"],
                "journalCount": value_count(&payload["journal"]),
            })
        }
    };
    redact_sensitive_json(&state)
}

fn safe_strategy_value(strategy: &Value) -> Value {
    select_fields(
        strategy,
        &[
            "id",
            "strategyId",
            "name",
            "status",
            "symbol",
            "assetClass",
            "version",
            "latestVersion",
            "updatedAt",
        ],
    )
}

fn select_fields(value: &Value, fields: &[&str]) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    let mut selected = serde_json::Map::new();
    for field in fields {
        if let Some(field_value) = value.get(*field) {
            selected.insert((*field).to_string(), field_value.clone());
        }
    }
    Value::Object(selected)
}

fn value_count(value: &Value) -> usize {
    value.as_array().map(Vec::len).unwrap_or(0)
}

fn studio_semantic_node(
    surface_id: &str,
    node_id: String,
    node_type: String,
    label: &str,
    summary: &str,
    object_ref: Value,
    state: Value,
) -> Value {
    require_contract(sightline_contracts::conform_node(&json!({
        "node_id": node_id,
        "nodeId": node_id,
        "surface_id": surface_id,
        "surfaceId": surface_id,
        "type": node_type,
        "role": "semantic-context",
        "label": label,
        "summary": summary,
        "object_refs": [object_ref],
        "objectRefs": [object_ref],
        "data_refs": [],
        "dataRefs": [],
        "state": redact_sensitive_json(&state),
        "actions": [],
        "context_provider_id": "tradeassembly.studio_surface_context",
        "contextProviderId": "tradeassembly.studio_surface_context",
        "visibility": "visible",
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "updated_at": LOCAL_TIMESTAMP,
        "updatedAt": LOCAL_TIMESTAMP,
    })))
}

#[allow(clippy::too_many_arguments)]
fn studio_detail_node(
    surface_id: &str,
    scope_id: Option<&str>,
    domain: &str,
    node_ref: &str,
    node_type: &str,
    label: &str,
    summary: &str,
    object_ref: Value,
    state: Value,
) -> Value {
    let node_id = scope_id
        .map(|scope| format!("{}.{}.{}", slug(scope), domain, node_ref))
        .unwrap_or_else(|| format!("{domain}.{node_ref}"));
    studio_semantic_node(
        surface_id,
        node_id,
        node_type.to_string(),
        label,
        summary,
        object_ref,
        state,
    )
}

fn studio_surface_summary(kind: StudioSurfaceKind) -> &'static str {
    match kind {
        StudioSurfaceKind::Dashboard => "Current local Studio workspace state.",
        StudioSurfaceKind::StrategyLibrary => "Available strategies and lifecycle state.",
        StudioSurfaceKind::StrategyOverview => "Selected strategy identity and lifecycle state.",
        StudioSurfaceKind::StrategyEditor => "Current StrategySpec draft and semantic sections.",
        StudioSurfaceKind::Backtest => {
            "Selected dataset, backtest run, report, and evidence state."
        }
        StudioSurfaceKind::Robustness => "Monte Carlo, sweep, and comparison evidence state.",
        StudioSurfaceKind::Execution => {
            "Selected activation, health, positions, risk, and journal state."
        }
        StudioSurfaceKind::PluginLibrary => {
            "Installed plugins, capabilities, health, and credential status."
        }
        StudioSurfaceKind::ResearchRollup => "Workspace research and recent backtest state.",
        StudioSurfaceKind::RunCenter => "Continuous execution and scheduler state.",
        StudioSurfaceKind::Journal => "Local journal and alert state.",
        StudioSurfaceKind::Review => "Immutable replay evidence, journal, and strategy facts.",
    }
}

fn studio_context_snapshot(surface: &Value, descriptor: &StudioRoute, nodes: &[Value]) -> Value {
    require_contract(sightline_contracts::conform_snapshot(&json!({
        "snapshot_id": surface["visible_snapshot_id"],
        "snapshotId": surface["visible_snapshot_id"],
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "surface_id": surface["surface_id"],
        "surfaceId": surface["surface_id"],
        "root_node_id": surface["root_node_id"],
        "rootNodeId": surface["root_node_id"],
        "active_object": surface["active_object"],
        "activeObject": surface["active_object"],
        "visible_nodes": nodes,
        "visibleNodes": nodes,
        "visible_context": {
            "summary": studio_surface_summary(descriptor.kind),
            "surfaceKind": descriptor.kind.id(),
            "redactionPolicyRef": "tradeassembly.redaction.agent_safe.v1",
            "secretPolicy": "secrets_are_refs_only"
        },
        "visibleContext": {
            "summary": studio_surface_summary(descriptor.kind),
            "surfaceKind": descriptor.kind.id(),
            "redactionPolicyRef": "tradeassembly.redaction.agent_safe.v1",
            "secretPolicy": "secrets_are_refs_only"
        },
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "version": 1,
    })))
}

fn navigation_in_surface_scope(surface: &Value, destination: &StudioRoute) -> bool {
    let source_kind = surface["surface_kind"].as_str().unwrap_or_default();
    match source_kind {
        "dashboard" | "strategy_library" | "research_rollup" | "run_center" => true,
        "plugin_library" => destination.kind == StudioSurfaceKind::PluginLibrary,
        "journal" => destination.kind == StudioSurfaceKind::Journal,
        "strategy_overview" | "strategy_editor" | "backtest" | "robustness" | "execution" => {
            surface["strategy_id"].as_str() == destination.strategy_id.as_deref()
        }
        "review" => {
            destination.kind == StudioSurfaceKind::Review
                && surface["resource_id"].as_str() == destination.resource_id.as_deref()
        }
        _ => false,
    }
}

fn sightline_route_error(code: &'static str) -> Value {
    json!({
        "ok": false,
        "body": {"surface": Value::Null},
        "error": {"code": code},
    })
}

fn sightline_navigation_error(code: &'static str) -> Value {
    json!({
        "ok": false,
        "body": {"command": Value::Null},
        "error": {"code": code},
    })
}

fn strategy_editor_nodes(strategy_id: &str, surface_id: &str, draft: &Value) -> Vec<Value> {
    let spec = &draft["spec"];
    let draft_hash = draft["draftHash"].as_str();
    let root = semantic_node(SemanticNodeInput {
        strategy_id,
        surface_id,
        node_id: format!("strategy.{strategy_id}.root"),
        parent_node_id: None,
        node_type: "strategy.stage",
        label: "Strategy contract",
        summary: "Current StrategySpec root, validation state, and editable draft context.",
        spec_path: "strategy.summary",
        draft_hash,
        state: json!({
            "strategyId": strategy_id,
            "draftHash": draft["draftHash"],
            "redaction": "agent_safe",
            "validationState": draft.get("validation").cloned().unwrap_or_else(|| json!("not_run")),
        }),
    });
    let mut nodes = vec![root];
    for (node_ref, label, summary, kind) in [
        (
            "market.requirements",
            "Market requirements",
            "Signal and trade market requirements.",
            "strategy.universe",
        ),
        (
            "entry.logic",
            "Entry logic",
            "Entry evaluation and condition logic.",
            "strategy.rule_group",
        ),
        (
            "risk.sizing",
            "Risk and sizing",
            "User-defined risk limits and sizing policy.",
            "strategy.risk_limit",
        ),
        (
            "orders.price_policy",
            "Orders and price policy",
            "Order policy and execution intent defaults.",
            "strategy.order_policy",
        ),
        (
            "exit.policy",
            "Exit policy",
            "Exit evaluation and position-closing policy.",
            "strategy.rule_group",
        ),
        (
            "capability.requirements",
            "Capability requirements",
            "Plugin capability requirements and readiness.",
            "capability.requirement",
        ),
    ] {
        nodes.push(semantic_node(SemanticNodeInput {
            strategy_id,
            surface_id,
            node_id: format!("{}.{}", slug(strategy_id), node_ref.replace('.', "_")),
            parent_node_id: Some(format!("strategy.{strategy_id}.root")),
            node_type: kind,
            label,
            summary,
            spec_path: node_ref,
            draft_hash,
            state: strategy::semantic_payload(spec, node_ref),
        }));
    }
    nodes
}

struct SemanticNodeInput<'a> {
    strategy_id: &'a str,
    surface_id: &'a str,
    node_id: String,
    parent_node_id: Option<String>,
    node_type: &'a str,
    label: &'a str,
    summary: &'a str,
    spec_path: &'a str,
    draft_hash: Option<&'a str>,
    state: Value,
}

fn semantic_node(input: SemanticNodeInput<'_>) -> Value {
    let mut node = json!({
        "node_id": input.node_id,
        "nodeId": input.node_id,
        "surface_id": input.surface_id,
        "surfaceId": input.surface_id,
        "type": input.node_type,
        "role": "semantic-context",
        "label": input.label,
        "summary": input.summary,
        "object_refs": [strategy_object_ref(input.strategy_id, input.draft_hash)],
        "objectRefs": [strategy_object_ref(input.strategy_id, input.draft_hash)],
        "data_refs": [{
            "ref_id": format!("data_{}", slug(input.spec_path)),
            "kind": "strategy_spec_path",
            "label": input.spec_path,
            "object_ref": strategy_object_ref(input.strategy_id, input.draft_hash),
            "path": input.spec_path,
        }],
        "dataRefs": [{
            "refId": format!("data_{}", slug(input.spec_path)),
            "kind": "strategy_spec_path",
            "label": input.spec_path,
            "objectRef": strategy_object_ref(input.strategy_id, input.draft_hash),
            "path": input.spec_path,
        }],
        "state": redact_sensitive_json(&input.state),
        "actions": [
            {"action_id": "share_context", "label": "Share context", "authority_level": "observe"},
            {"action_id": "create_proposal", "label": "Create proposal", "authority_level": "propose"}
        ],
        "context_provider_id": "tradeassembly.strategy_spec_context",
        "contextProviderId": "tradeassembly.strategy_spec_context",
        "visibility": "visible",
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "updated_at": LOCAL_TIMESTAMP,
        "updatedAt": LOCAL_TIMESTAMP,
    });
    if let Some(parent_node_id) = input.parent_node_id {
        node["parent_node_id"] = json!(parent_node_id);
        node["parentNodeId"] = json!(parent_node_id);
    }
    require_contract(sightline_contracts::conform_node(&node))
}

fn current_selection(service: &TradeAssemblyService, body: &Value) -> Value {
    if let Some(selection_id) = optional_string(body, &["selectionId", "selection_id"]) {
        if let Ok(Some(selection)) = service
            .runtime()
            .storage
            .get_json(SELECTIONS_NS, &selection_id)
        {
            return selection;
        }
    }
    let surface_id = optional_string(body, &["surfaceId", "surface_id"]);
    let pointer_key = selection_pointer_key("current", surface_id.as_deref());
    service
        .runtime()
        .storage
        .get_json(SELECTION_POINTERS_NS, &pointer_key)
        .ok()
        .flatten()
        .and_then(|pointer| {
            pointer["selection_id"].as_str().and_then(|selection_id| {
                service
                    .runtime()
                    .storage
                    .get_json(SELECTIONS_NS, selection_id)
                    .ok()
                    .flatten()
            })
        })
        .unwrap_or(Value::Null)
}

fn selection_pointer_key(kind: &str, surface_id: Option<&str>) -> String {
    surface_id
        .filter(|surface_id| !surface_id.is_empty())
        .map(|surface_id| format!("{kind}:{surface_id}"))
        .unwrap_or_else(|| kind.to_string())
}

fn selection_for_actor(selection: &Value, actor: &Value) -> Value {
    if !selection.is_null() && sightline_contracts::selection_visible_to_actor(selection, actor) {
        selection.clone()
    } else {
        Value::Null
    }
}

fn agent_actor_from_body(body: &Value) -> Value {
    let mut actor = actor_from_body(body, "agent");
    actor["actor_type"] = json!("agent");
    actor["actorType"] = json!("agent");
    actor
}

fn shared_selection_for_actor(selection: &Value, actor: &Value) -> Option<Value> {
    let visible = selection_for_actor(selection, actor);
    (!visible.is_null()).then_some(visible)
}

fn selection_not_shared_error() -> Value {
    json!({
        "ok": false,
        "body": {"selection": Value::Null, "context": Value::Null},
        "error": {"code": "sightline_selection_not_shared"},
    })
}

fn context_for_selection(service: &TradeAssemblyService, selection: &Value) -> Value {
    let node_ref = &selection["node_ref"];
    let surface_id = node_ref["surface_id"].as_str().unwrap_or("");
    let node_id = node_ref["node_id"].as_str().unwrap_or("");
    let node = node_for(service, surface_id, node_id).unwrap_or_else(|| json!({}));
    let payload = json!({
        "node": redact_sensitive_json(&node),
        "state": redact_sensitive_json(&node["state"]),
        "actions": node["actions"].clone(),
        "evidenceRefs": selection["evidenceRefs"].clone(),
        "noAdvice": LEGAL_BOUNDARY,
        "redaction_policy_ref": sightline_contracts::TRADEASSEMBLY_REDACTION_POLICY,
        "payload_handling": "sanitized_inline_plus_refs",
    });
    let mut context = require_contract(sightline_contracts::resolved_context(selection, &payload));
    let extensions = json!({
        "schemaVersion": "tradeassembly.sightline.context.v1",
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "selection_id": selection["selection_id"],
        "selectionId": selection["selection_id"],
        "surface_id": surface_id,
        "surfaceId": surface_id,
        "node_ref": node_ref,
        "nodeRef": selection["nodeRef"],
        "context_level": selection["context_level"].as_u64().unwrap_or(2).min(2),
        "contextLevel": selection["context_level"].as_u64().unwrap_or(2).min(2),
        "summary": selection["summary"],
        "object_refs": selection["object_refs"],
        "objectRefs": selection["object_refs"],
        "payload": payload,
        "redaction_policy_ref": "tradeassembly.redaction.agent_safe.v1",
        "redactionPolicyRef": "tradeassembly.redaction.agent_safe.v1",
        "payloadHandling": "sanitized_inline_plus_refs",
    });
    if let (Value::Object(context), Value::Object(extensions)) = (&mut context, extensions) {
        context.extend(extensions);
    }
    context
}

fn node_for(service: &TradeAssemblyService, surface_id: &str, node_id: &str) -> Option<Value> {
    service
        .runtime()
        .storage
        .get_json(NODES_NS, &node_key(surface_id, node_id))
        .ok()
        .flatten()
}

fn node_for_spec_path(
    service: &TradeAssemblyService,
    surface_id: &str,
    spec_path: &str,
) -> Option<Value> {
    list_values(service, NODES_NS).into_iter().find(|node| {
        node["surface_id"].as_str() == Some(surface_id)
            && node["data_refs"].as_array().is_some_and(|refs| {
                refs.iter()
                    .any(|data_ref| data_ref["path"].as_str() == Some(spec_path))
            })
    })
}

fn selection_node_ref_alias(node_ref: &str) -> &str {
    match node_ref {
        "strategy.root" => "strategy.summary",
        "strategy.inputs" => "market.requirements",
        "strategy.rules" => "entry.logic",
        "strategy.risk" => "risk.sizing",
        "strategy.orders" => "orders.price_policy",
        "strategy.capabilities" => "capability.requirements",
        _ => node_ref,
    }
}

fn audited_node_command(
    service: &TradeAssemblyService,
    body: Value,
    command_type: &str,
    action: &str,
) -> Value {
    let command = semantic_command(service, &body, command_type, action);
    let command_id = command["command_id"]
        .as_str()
        .unwrap_or("command")
        .to_string();
    persist(
        service,
        COMMANDS_NS,
        &command_id,
        command.clone(),
        "sightline.command.requested",
    );
    append_event(
        service,
        "agent.command.requested",
        json!({"command_id": command_id, "command_type": command_type}),
        command["target"]["surface_id"].as_str().map(str::to_string),
        Some(agent_actor_from_body(&body)),
    );
    api_result(json!({"command": command}))
}

fn semantic_command(
    service: &TradeAssemblyService,
    body: &Value,
    command_type: &str,
    action: &str,
) -> Value {
    ensure_room(service);
    let strategy_id = strategy_id_from(body);
    let surface_id = optional_string(body, &["surfaceId", "surface_id"])
        .unwrap_or_else(|| strategy_editor_surface_id(&strategy_id));
    let command_id = optional_string(body, &["commandId", "command_id"]).unwrap_or_else(|| {
        format!(
            "command_{}_{}",
            slug(command_type),
            short_hash(&hash_value(body))
        )
    });
    let authority_level = if high_authority_route(body) {
        "high_authority"
    } else {
        "navigate"
    };
    let actor = agent_actor_from_body(body);
    require_contract(sightline_contracts::conform_command(&json!({
        "command_id": command_id,
        "commandId": command_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "issued_by": actor,
        "issuedBy": actor,
        "target": {
            "target_kind": "surface",
            "targetKind": "surface",
            "surface_id": surface_id,
            "surfaceId": surface_id,
            "object_ref": strategy_object_ref(&strategy_id, None),
            "objectRef": strategy_object_ref(&strategy_id, None),
        },
        "command_type": command_type,
        "commandType": command_type,
        "payload": {
            "action": action,
            "route": optional_string(body, &["route"]).unwrap_or_else(|| format!("/app/strategies/{strategy_id}/builder")),
            "nodeId": optional_string(body, &["nodeId", "node_id", "nodeRef", "node_ref"]),
            "reason": "Agent requested navigation to a registered Studio surface.",
            "humanOnlyGate": high_authority_route(body),
        },
        "authority_level": authority_level,
        "authorityLevel": authority_level,
        "idempotency_key": optional_string(body, &["idempotencyKey", "idempotency_key"]).unwrap_or_else(|| format!("sightline:{command_id}")),
        "idempotencyKey": optional_string(body, &["idempotencyKey", "idempotency_key"]).unwrap_or_else(|| format!("sightline:{command_id}")),
        "status": if high_authority_route(body) { "rejected" } else { "requested" },
        "error": if high_authority_route(body) { json!({
            "code": "user_initiated_context_required",
            "message": "A verified user context is required.",
            "retryable": false,
            "user_action": "Continue in TradeAssembly Studio."
        }) } else { Value::Null },
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "updated_at": LOCAL_TIMESTAMP,
        "updatedAt": LOCAL_TIMESTAMP,
    })))
}

fn sightline_proposal(
    proposal: &Value,
    body: &Value,
    selection: &Value,
    sightline_refs: &[String],
) -> Value {
    let proposal_id = proposal["id"].as_str().unwrap_or("proposal_local");
    let target_object = selection["object_refs"]
        .as_array()
        .and_then(|refs| refs.first())
        .cloned()
        .unwrap_or_else(|| strategy_object_ref(&strategy_id_from(body), None));
    let affected_refs = selection["object_refs"]
        .as_array()
        .filter(|refs| !refs.is_empty())
        .cloned()
        .unwrap_or_else(|| vec![target_object.clone()]);
    let actor = agent_actor_from_body(body);
    let proposal = json!({
        "proposal_id": proposal_id,
        "proposalId": proposal_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "proposed_by": actor,
        "proposedBy": actor,
        "target_object": target_object,
        "targetObject": target_object,
        "base_version": proposal["baseDraftHash"],
        "baseVersion": proposal["baseDraftHash"],
        "patch_format": "json-merge-patch",
        "patchFormat": "json-merge-patch",
        "patches": [redact_sensitive_json(&proposal["patch"])],
        "affected_refs": affected_refs,
        "affectedRefs": affected_refs,
        "explanation": proposal["summary"].as_str().unwrap_or("Agent proposed StrategySpec patch."),
        "risk_level": proposal["riskLevel"].as_str().unwrap_or("low"),
        "riskLevel": proposal["riskLevel"].as_str().unwrap_or("low"),
        "validation_result": proposal.get("validation").cloned().unwrap_or_else(|| json!({"status": "pending"})),
        "validationResult": proposal.get("validation").cloned().unwrap_or_else(|| json!({"status": "pending"})),
        "status": "pending",
        "strategyProposal": redact_sensitive_json(proposal),
        "selection": redact_sensitive_json(selection),
        "sightlineRefs": sightline_refs,
        "redaction": redaction_summary(),
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "updated_at": LOCAL_TIMESTAMP,
        "updatedAt": LOCAL_TIMESTAMP,
    });
    require_contract(sightline_contracts::conform_proposal(&proposal))
}

fn presence_record(body: &Value, status: &str) -> Value {
    let actor = agent_actor_from_body(body);
    let actor_id = actor["actor_id"].as_str().unwrap_or("agent_local");
    let strategy_id = strategy_id_from(body);
    let surface_id = optional_string(body, &["surfaceId", "surface_id"])
        .unwrap_or_else(|| strategy_editor_surface_id(&strategy_id));
    let node_id = optional_string(body, &["nodeId", "node_id", "nodeRef", "node_ref"]);
    require_contract(sightline_contracts::conform_presence(&json!({
        "presence_id": format!("presence_{}", slug(actor_id)),
        "presenceId": format!("presence_{}", slug(actor_id)),
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "actor": actor,
        "surface_id": surface_id,
        "surfaceId": surface_id,
        "focus_node": node_id.as_ref().map(|node_id| json!({"surface_id": surface_id, "node_id": node_id})).unwrap_or(Value::Null),
        "focusNode": node_id.as_ref().map(|node_id| json!({"surfaceId": surface_id, "nodeId": node_id})).unwrap_or(Value::Null),
        "selection_id": optional_string(body, &["selectionId", "selection_id"]),
        "selectionId": optional_string(body, &["selectionId", "selection_id"]),
        "status": status,
        "color": "#0b61ff",
        "label": actor["client_name"].as_str().unwrap_or("Agent"),
        "updated_at": LOCAL_TIMESTAMP,
        "updatedAt": LOCAL_TIMESTAMP,
    })))
}

fn list_events_payload(service: &TradeAssemblyService, body: &Value) -> Vec<Value> {
    let limit = body
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .min(200) as usize;
    let mut events = list_values(service, EVENTS_NS);
    events.sort_by(|left, right| {
        left["sequence"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&right["sequence"].as_u64().unwrap_or(0))
    });
    let actor = agent_actor_from_body(body);
    events
        .into_iter()
        .rev()
        .filter(|event| event_visible_to_actor(service, event, &actor))
        .take(limit)
        .collect()
}

fn event_visible_to_actor(_service: &TradeAssemblyService, event: &Value, actor: &Value) -> bool {
    sightline_contracts::event_visible_to_actor(event, actor)
}

fn append_event(
    service: &TradeAssemblyService,
    event_type: &str,
    payload: Value,
    surface_id: Option<String>,
    actor: Option<Value>,
) -> Value {
    let sequence = service
        .runtime()
        .storage
        .list_json(EVENTS_NS)
        .map(|items| items.len() as u64 + 1)
        .unwrap_or(1);
    let event_id = format!("evt_sightline_{sequence:06}");
    let mut event = json!({
        "event_id": event_id,
        "eventId": event_id,
        "room_id": ROOM_ID,
        "roomId": ROOM_ID,
        "type": event_type,
        "payload": redact_sensitive_json(&payload),
        "trace_id": format!("trace_sightline_{sequence:06}"),
        "traceId": format!("trace_sightline_{sequence:06}"),
        "created_at": LOCAL_TIMESTAMP,
        "createdAt": LOCAL_TIMESTAMP,
        "sequence": sequence,
    });
    if let Some(surface_id) = surface_id {
        event["surface_id"] = json!(surface_id);
        event["surfaceId"] = json!(surface_id);
    }
    if let Some(actor) = actor {
        event["actor"] = actor;
    }
    event = require_contract(sightline_contracts::conform_event(&event));
    persist(
        service,
        EVENTS_NS,
        &event_id,
        event.clone(),
        "sightline.event.recorded",
    );
    event
}

fn persist(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    payload: Value,
    event_type: &str,
) {
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("{event_type}:{namespace}:{key}"))
            .expect("valid sightline idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(namespace, key, payload.clone(), &context)
        .expect("persist sightline object");
    service
        .runtime()
        .record_side_effect(event_type, payload, &context)
        .expect("journal sightline object change");
}

fn persist_versioned(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    payload: Value,
    event_type: &str,
    content_hash: &str,
) {
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!(
            "{event_type}:{namespace}:{key}:{}",
            short_hash(content_hash)
        ))
        .expect("valid versioned sightline idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(namespace, key, payload.clone(), &context)
        .expect("persist versioned sightline object");
    service
        .runtime()
        .record_side_effect(event_type, payload, &context)
        .expect("journal versioned sightline object change");
}

fn persist_pointer(service: &TradeAssemblyService, key: &str, payload: Value) {
    let content_hash = hash_value(&payload);
    persist_versioned(
        service,
        SELECTION_POINTERS_NS,
        key,
        payload,
        "sightline.selection.pointer.changed",
        &content_hash,
    );
}

fn list_values(service: &TradeAssemblyService, namespace: &str) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

fn strategy_object_ref(strategy_id: &str, version: Option<&str>) -> Value {
    let mut object = json!({
        "app_id": APP_ID,
        "appId": APP_ID,
        "object_type": "strategy_draft",
        "objectType": "strategy_draft",
        "object_id": strategy_id,
        "objectId": strategy_id,
        "path": format!("tradeassembly://strategy/{strategy_id}/draft"),
    });
    if let Some(version) = version {
        object["version"] = json!(version);
        object["hash"] = json!(version);
    }
    object
}

fn actor_from_body(body: &Value, default_kind: &str) -> Value {
    if let Some(actor) = body.get("actor").filter(|actor| actor.is_object()) {
        return normalize_actor(actor, default_kind);
    }
    if default_kind == "agent" {
        json!({
            "actor_type": "agent",
            "actorType": "agent",
            "actor_id": optional_string(body, &["agentId", "agent_id"]).unwrap_or_else(|| "tradeassembly.local_agent".to_string()),
            "actorId": optional_string(body, &["agentId", "agent_id"]).unwrap_or_else(|| "tradeassembly.local_agent".to_string()),
            "client_name": optional_string(body, &["clientName", "client_name"]).unwrap_or_else(|| "TradeAssembly local agent".to_string()),
            "clientName": optional_string(body, &["clientName", "client_name"]).unwrap_or_else(|| "TradeAssembly local agent".to_string()),
            "display_name": optional_string(body, &["displayName", "display_name"]).unwrap_or_else(|| "TradeAssembly local agent".to_string()),
            "displayName": optional_string(body, &["displayName", "display_name"]).unwrap_or_else(|| "TradeAssembly local agent".to_string()),
        })
    } else {
        user_actor(body)
    }
}

fn normalize_actor(actor: &Value, default_kind: &str) -> Value {
    let kind = actor
        .get("actor_type")
        .or_else(|| actor.get("actorType"))
        .or_else(|| actor.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or(default_kind);
    let id = actor
        .get("actor_id")
        .or_else(|| actor.get("actorId"))
        .or_else(|| actor.get("id"))
        .and_then(Value::as_str)
        .unwrap_or(if kind == "agent" {
            "agent.local"
        } else {
            "user.local"
        });
    let display = actor
        .get("display_name")
        .or_else(|| actor.get("displayName"))
        .or_else(|| actor.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(if kind == "agent" {
            "Local Agent"
        } else {
            "Local User"
        });
    let mut normalized = json!({
        "actor_type": kind,
        "actorType": kind,
        "actor_id": id,
        "actorId": id,
        "display_name": display,
        "displayName": display,
    });
    if kind == "agent" {
        normalized["client_name"] = actor
            .get("client_name")
            .or_else(|| actor.get("clientName"))
            .and_then(Value::as_str)
            .unwrap_or(display)
            .into();
        normalized["clientName"] = normalized["client_name"].clone();
    }
    normalized
}

fn user_actor(_body: &Value) -> Value {
    json!({
        "actor_type": "user",
        "actorType": "user",
        "actor_id": "user.local",
        "actorId": "user.local",
        "display_name": "Local User",
        "displayName": "Local User",
    })
}

pub(crate) fn user_actor_from_principal(principal: &crate::auth::SessionPrincipal) -> Value {
    let display_name = principal.name.as_deref().unwrap_or("Local User");
    json!({
        "actor_type": "user",
        "actorType": "user",
        "actor_id": principal.subject,
        "actorId": principal.subject,
        "display_name": display_name,
        "displayName": display_name,
    })
}

fn redaction_summary() -> Value {
    json!({
        "policyRef": "tradeassembly.redaction.agent_safe.v1",
        "payloadHandling": "sanitized_inline_plus_refs",
        "redactedFields": [
            "api_key",
            "api_secret",
            "oauth",
            "access_token",
            "refresh_token",
            "credential_handle",
            "account_token",
            "private_key",
            "secret"
        ],
        "agentVisible": true,
        "secretsShared": false,
    })
}

fn authority_summary() -> Value {
    json!({
        "userPrincipal": "user.local",
        "agentPrincipalDistinct": true,
        "durableMutationsOwnedBy": "tradeassembly.control_plane",
        "highAuthorityRequiresTradeAssemblyAPF": true,
        "humanOnlyLegalAcknowledgements": true,
    })
}

fn high_authority_route(body: &Value) -> bool {
    optional_string(body, &["route"])
        .map(|route| {
            route.contains("live")
                || route.contains("approval")
                || route.contains("acknowledgement")
                || route.contains("order")
        })
        .unwrap_or(false)
}

fn high_authority_approval(body: &Value) -> bool {
    optional_string(body, &["authorityLevel", "authority_level"])
        .map(|level| matches!(level.as_str(), "runtime" | "high_authority"))
        .unwrap_or(false)
}

fn sightline_refs_from_body(body: &Value, selection: &Value) -> Vec<String> {
    let mut refs = vec![ROOM_ID.to_string()];
    for key in [
        "surfaceId",
        "surface_id",
        "nodeId",
        "node_id",
        "nodeRef",
        "node_ref",
        "proposalId",
        "proposal_id",
        "approvalId",
        "approval_id",
    ] {
        if let Some(value) = body.get(key).and_then(Value::as_str) {
            refs.push(value.to_string());
        }
    }
    for pointer in [
        "/surface_id",
        "/selection_id",
        "/node_ref/node_id",
        "/node_ref/surface_id",
    ] {
        if let Some(value) = selection.pointer(pointer).and_then(Value::as_str) {
            refs.push(value.to_string());
        }
    }
    refs.sort();
    refs.dedup();
    refs
}

fn evidence_refs_for(parts: &[&str]) -> Vec<Value> {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .map(|part| json!({"kind": "sightline_context", "ref": format!("hash:{}", short_hash(&hash_text(part)))}))
        .collect()
}

fn node_key(surface_id: &str, node_id: &str) -> String {
    format!("{}:{}", surface_id, node_id)
}

fn strategy_editor_surface_id(strategy_id: &str) -> String {
    format!("surface_strategy_editor_{}", slug(strategy_id))
}

fn optional_string(body: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| body.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn redact_sensitive_json(value: &Value) -> Value {
    sightline_contracts::redact_agent_value(value)
}

fn require_contract(result: Result<Value, sightline_contracts::ContractError>) -> Value {
    result.unwrap_or_else(|error| panic!("{error}"))
}

fn hash_value(value: &Value) -> String {
    hash_text(&serde_json::to_string(value).unwrap_or_default())
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn short_hash(hash: &str) -> String {
    hash.trim_start_matches("sha256:")
        .chars()
        .take(12)
        .collect()
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service(name: &str) -> TradeAssemblyService {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tradeassembly_sightline_{name}_{}_{}.json",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_file(&path);
        TradeAssemblyService::test_local(path.to_str().unwrap())
    }

    fn test_user_actor() -> Value {
        user_actor(&json!({}))
    }

    #[test]
    fn sightline_context_redacts_secrets_and_credentials() {
        let secret = json!({
            "api_key": "live-secret",
            "credential_handle": "cred_123",
            "nested": {"oauth_token": "token"}
        });
        let redacted = redact_sensitive_json(&secret);
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(!serialized.contains("live-secret"));
        assert!(!serialized.contains("cred_123"));
        assert!(!serialized.contains("\"token\""));
        assert!(serialized.contains("[REDACTED]"));
    }

    #[test]
    fn studio_routes_publish_distinct_server_derived_surfaces() {
        let service = test_service("studio_surfaces");
        let routes = [
            (
                "/app/strategies/strat_local_btc_demo/builder",
                "strategy_editor",
                "strategy.stage",
            ),
            (
                "/app/strategies/strat_local_btc_demo/research?view=backtest",
                "backtest",
                "studio.backtest",
            ),
            (
                "/app/strategies/strat_local_btc_demo/execution?mode=paper",
                "execution",
                "studio.execution",
            ),
            (
                "/app/plugins-providers",
                "plugin_library",
                "studio.plugin_library",
            ),
            (
                "/app/reviews/latest?strategyId=strat_local_btc_demo",
                "review",
                "studio.review",
            ),
        ];
        let mut surface_ids = std::collections::BTreeSet::new();
        for (route, expected_kind, expected_node_type) in routes {
            let result = publish_studio_surface(&service, json!({"route": route}));
            assert_eq!(result["ok"], true, "{result}");
            assert_eq!(result["body"]["surface"]["surfaceKind"], expected_kind);
            assert_eq!(result["body"]["nodes"][0]["type"], expected_node_type);
            surface_ids.insert(
                result["body"]["surface"]["surfaceId"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        assert_eq!(surface_ids.len(), 5);
    }

    #[test]
    fn supported_workspaces_publish_granular_backend_owned_nodes() {
        let service = test_service("studio_granular_nodes");
        for (route, expected_types) in [
            (
                "/app/strategies/strat_local_btc_demo/research?view=backtest",
                vec![
                    "studio.backtest.dataset",
                    "studio.backtest.run",
                    "studio.backtest.report",
                    "studio.backtest.journal",
                ],
            ),
            (
                "/app/strategies/strat_local_btc_demo/execution?mode=paper",
                vec![
                    "studio.execution.activation",
                    "studio.execution.health",
                    "studio.execution.positions",
                    "studio.execution.journal",
                ],
            ),
            (
                "/app/plugins-providers",
                vec!["studio.plugin.inventory", "studio.plugin.instance"],
            ),
        ] {
            let published = publish_studio_surface(&service, json!({"route": route}));
            assert_eq!(published["ok"], true, "{published}");
            let node_types = published["body"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|node| node["type"].as_str())
                .collect::<Vec<_>>();
            for expected_type in expected_types {
                assert!(
                    node_types.contains(&expected_type),
                    "missing {expected_type} in {node_types:?}"
                );
            }
            let serialized = serde_json::to_string(&published["body"]["nodes"]).unwrap();
            for forbidden in ["api_key", "api_secret", "access_token", "refresh_token"] {
                assert!(!serialized.contains(forbidden), "{serialized}");
            }
        }
    }

    #[test]
    fn workspace_selection_requires_registered_node_and_remains_surface_scoped() {
        let service = test_service("workspace_selection_scope");
        let backtest = publish_studio_surface(
            &service,
            json!({"route": "/app/strategies/strat_local_btc_demo/research?view=backtest"}),
        );
        let backtest_surface = backtest["body"]["surface"]["surfaceId"].as_str().unwrap();
        let report_node = backtest["body"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["type"] == "studio.backtest.report")
            .unwrap()["nodeId"]
            .as_str()
            .unwrap();
        let selected = set_selection(
            &service,
            json!({
                "surfaceId": backtest_surface,
                "nodeId": report_node,
                "shared": true,
            }),
            test_user_actor(),
        );
        assert_eq!(selected["ok"], true, "{selected}");
        assert_eq!(
            selected["body"]["selection"]["nodeRef"]["surfaceId"],
            backtest_surface
        );

        let execution = publish_studio_surface(
            &service,
            json!({"route": "/app/strategies/strat_local_btc_demo/execution?mode=paper"}),
        );
        let execution_surface = execution["body"]["surface"]["surfaceId"].as_str().unwrap();
        assert!(session_state(
            &service,
            json!({"surfaceId": execution_surface, "actor": test_user_actor()})
        )["body"]["selection"]
            .is_null());

        let selection_count = list_values(&service, SELECTIONS_NS).len();
        let rejected = set_selection(
            &service,
            json!({
                "surfaceId": execution_surface,
                "nodeId": report_node,
                "shared": true,
            }),
            test_user_actor(),
        );
        assert_eq!(rejected["error"]["code"], "sightline_node_not_registered");
        assert_eq!(list_values(&service, SELECTIONS_NS).len(), selection_count);
    }

    #[test]
    fn workspace_scoped_plugin_node_can_be_selected_without_strategy_id() {
        let service = test_service("plugin_workspace_selection");
        let published =
            publish_studio_surface(&service, json!({"route": "/app/plugins-providers"}));
        let surface_id = published["body"]["surface"]["surfaceId"].as_str().unwrap();
        let plugin_node = published["body"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["type"] == "studio.plugin.instance")
            .unwrap()["nodeId"]
            .as_str()
            .unwrap();
        let selected = set_selection(
            &service,
            json!({
                "surfaceId": surface_id,
                "nodeId": plugin_node,
                "shared": true,
            }),
            test_user_actor(),
        );
        assert_eq!(selected["ok"], true, "{selected}");
        assert_eq!(
            selected["body"]["selection"]["nodeRef"]["nodeId"],
            plugin_node
        );
    }

    #[test]
    fn existing_strategy_scoped_plugin_route_publishes_automatically() {
        let service = test_service("strategy_scoped_plugin_route");
        let result = publish_studio_surface(
            &service,
            json!({"route": "/app/plugins-providers?strategyId=strat_local_btc_demo"}),
        );
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["body"]["surface"]["surfaceKind"], "plugin_library");
        assert_eq!(result["body"]["nodes"][0]["type"], "studio.plugin_library");
    }

    #[test]
    fn studio_publication_ignores_caller_context_and_secret_fields() {
        let service = test_service("server_derived");
        let result = publish_studio_surface(
            &service,
            json!({
                "route": "/app/plugins-providers",
                "context": {"apiSecret": "injected-secret"},
                "nodes": [{"state": {"access_token": "injected-token"}}],
                "actions": [{"action": "activate_live"}],
                "actor": {"kind": "user", "id": "spoofed.user"},
                "authorityContext": {"role": "admin"},
            }),
        );
        assert_eq!(result["ok"], true);
        let serialized = serde_json::to_string(&result["body"]["snapshot"]).unwrap();
        for forbidden in [
            "injected-secret",
            "injected-token",
            "activate_live",
            "spoofed.user",
            "authorityContext",
        ] {
            assert!(!serialized.contains(forbidden), "{serialized}");
        }
        assert!(serialized.contains("plugin_library"));
    }

    #[test]
    fn repeated_studio_publication_is_event_idempotent() {
        let service = test_service("publish_idempotence");
        let route = "/app/strategies/strat_local_btc_demo/research?view=backtest";
        let first = publish_studio_surface(&service, json!({"route": route}));
        let event_count = list_values(&service, EVENTS_NS).len();
        let second = publish_studio_surface(&service, json!({"route": route}));
        assert_eq!(
            first["body"]["surface"]["surfaceId"],
            second["body"]["surface"]["surfaceId"]
        );
        assert_eq!(
            first["body"]["snapshot"]["snapshotId"],
            second["body"]["snapshot"]["snapshotId"]
        );
        assert_eq!(first["body"]["changed"], true);
        assert_eq!(second["body"]["changed"], false);
        assert_eq!(list_values(&service, EVENTS_NS).len(), event_count);
    }

    #[test]
    fn navigation_requires_a_safe_registered_surface_and_scope() {
        let service = test_service("navigation_scope");
        let published = publish_studio_surface(
            &service,
            json!({"route": "/app/strategies/strat_local_btc_demo/builder"}),
        );
        let surface_id = published["body"]["surface"]["surfaceId"]
            .as_str()
            .unwrap()
            .to_string();

        let safe = request_navigation(
            &service,
            json!({
                "surfaceId": surface_id,
                "route": "/app/strategies/strat_local_btc_demo/research?view=backtest",
                "idempotencyKey": "navigate:test:backtest",
                "reason": "api_secret=SHOULD_NOT_LEAK",
            }),
        );
        assert_eq!(safe["ok"], true, "{safe}");
        assert_eq!(safe["body"]["command"]["status"], "requested");
        assert_eq!(safe["body"]["command"]["issuedBy"]["actorType"], "agent");
        assert_eq!(
            safe["body"]["command"]["payload"]["reason"],
            "Agent requested navigation to a registered Studio surface."
        );
        assert_eq!(list_values(&service, COMMANDS_NS).len(), 1);
        let event_count = list_values(&service, EVENTS_NS).len();
        let state = session_state(&service, json!({}));
        assert!(
            !serde_json::to_string(&state)
                .unwrap()
                .contains("SHOULD_NOT_LEAK"),
            "{state}"
        );

        let replay = request_navigation(
            &service,
            json!({
                "surfaceId": surface_id,
                "route": "/app/strategies/strat_local_btc_demo/research?view=backtest",
                "idempotencyKey": "navigate:test:backtest",
                "reason": "api_secret=SHOULD_NOT_LEAK",
            }),
        );
        assert_eq!(replay["ok"], true);
        assert_eq!(replay["body"]["replayed"], true);
        assert_eq!(list_values(&service, EVENTS_NS).len(), event_count);

        for (body, expected_code) in [
            (
                json!({
                    "surfaceId": "surface_missing",
                    "route": "/app/strategies/strat_local_btc_demo/research"
                }),
                "sightline_surface_not_found",
            ),
            (
                json!({"surfaceId": surface_id, "route": "https://example.com/app"}),
                "sightline_route_invalid",
            ),
            (
                json!({"surfaceId": surface_id, "route": "/app/../secrets"}),
                "sightline_route_invalid",
            ),
            (
                json!({"surfaceId": surface_id, "route": "/app/connect"}),
                "sightline_route_unsupported",
            ),
            (
                json!({
                    "surfaceId": surface_id,
                    "route": "/app/strategies/strat_local_btc_demo/execution?mode=live"
                }),
                "sightline_navigation_user_required",
            ),
            (
                json!({
                    "surfaceId": surface_id,
                    "route": "/app/strategies/strat_other/research"
                }),
                "sightline_navigation_scope_mismatch",
            ),
        ] {
            let denied = request_navigation(&service, body);
            assert_eq!(denied["ok"], false, "{denied}");
            assert_eq!(denied["error"]["code"], expected_code, "{denied}");
        }
        assert_eq!(list_values(&service, COMMANDS_NS).len(), 1);
        assert_eq!(list_values(&service, EVENTS_NS).len(), event_count);
    }

    #[test]
    fn session_state_exposes_automatic_context_and_command_cursor() {
        let service = test_service("automatic_session_state");
        let published =
            publish_studio_surface(&service, json!({"route": "/app/plugins-providers"}));
        let surface_id = published["body"]["surface"]["surfaceId"].as_str().unwrap();
        let navigation = request_navigation(
            &service,
            json!({
                "surfaceId": surface_id,
                "route": "/app/admin/plugin-manifests"
            }),
        );
        assert_eq!(navigation["ok"], true);
        let state = session_state(&service, json!({}));
        assert!(!state["body"]["nodes"].as_array().unwrap().is_empty());
        assert!(!state["body"]["contextSnapshots"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(state["body"]["commands"].as_array().unwrap().len(), 1);
        assert!(state["body"]["eventCursor"].as_u64().unwrap() > 0);
    }

    #[test]
    fn graphql_publishes_server_derived_studio_surface() {
        let service = test_service("graphql_studio_surface");
        let response = service.execute_graphql(json!({
            "operationName": "PublishSightlineStudioSurface",
            "query": "mutation PublishSightlineStudioSurface { publishSightlineStudioSurface }",
            "variables": {
                "route": "/app/strategies/strat_local_btc_demo/research?view=backtest",
                "context": {"apiSecret": "must-not-cross-boundary"}
            }
        }));
        let result = &response["data"]["publishSightlineStudioSurface"];
        assert_eq!(result["ok"], true, "{response}");
        assert_eq!(result["body"]["surface"]["surfaceKind"], "backtest");
        assert!(!serde_json::to_string(result)
            .unwrap()
            .contains("must-not-cross-boundary"));
    }

    #[test]
    fn session_publishes_strategy_editor_surface_and_selection() {
        let service = test_service("session");
        let published = publish_strategy_editor_surface(
            &service,
            json!({"strategyId": "strat_local_btc_demo"}),
        );
        assert_eq!(published["ok"], true);
        assert!(published["body"]["nodes"].as_array().unwrap().len() >= 5);
        let selected = set_selection(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "nodeRef": "strategy.risk",
                "shared": true
            }),
            test_user_actor(),
        );
        assert_eq!(selected["ok"], true);
        assert_eq!(selected["body"]["selection"]["shared"], true);
        assert_eq!(
            selected["body"]["context"]["payload"]["state"]["allocation"]["mode"],
            "none"
        );
        assert!(selected["body"]["context"]["payload"]["state"]["stages"]["sizing"].is_object());
        assert!(selected["body"]["context"]["payload"]["state"]["stages"]["risk"].is_object());
        assert_eq!(
            selected["body"]["context"]["redaction_policy_ref"],
            "tradeassembly.redaction.agent_safe.v1"
        );
        let state = session_state(
            &service,
            json!({"strategyId": "strat_local_btc_demo", "actor": {"kind": "agent", "id": "agent.test"}}),
        );
        assert_eq!(
            state["body"]["contractSource"]["compatibility_target"]["rev"],
            sightline_contracts::SIGHTLINE_GIT_REV
        );
        assert_eq!(
            state["body"]["contractSource"]["compatibility_target"]["verification"],
            "verified_local_source"
        );
        assert_eq!(state["body"]["transport"]["protocol"]["protocol"], "mcp");
    }

    #[test]
    fn selection_rejects_unregistered_surface_and_node_without_side_effects() {
        let service = test_service("selection_fail_closed");
        let published = publish_strategy_editor_surface(
            &service,
            json!({"strategyId": "strat_local_btc_demo"}),
        );
        let surface_id = published["body"]["surface"]["surfaceId"]
            .as_str()
            .expect("published surface id");
        let selection_count = list_values(&service, SELECTIONS_NS).len();
        let pointer_count = list_values(&service, SELECTION_POINTERS_NS).len();
        let event_count = list_values(&service, EVENTS_NS).len();

        for body in [
            json!({
                "strategyId": "strat_local_btc_demo",
                "surfaceId": surface_id,
                "nodeId": "unregistered.node",
                "shared": true,
            }),
            json!({
                "strategyId": "strat_local_btc_demo",
                "surfaceId": "surface_unknown",
                "nodeRef": "risk.sizing",
                "shared": true,
            }),
        ] {
            let rejected = set_selection(&service, body, test_user_actor());
            assert_eq!(rejected["ok"], false);
            assert_eq!(rejected["error"]["code"], "sightline_node_not_registered");
            assert!(rejected["body"]["selection"].is_null());
            assert!(rejected["body"]["context"].is_null());
        }

        assert_eq!(list_values(&service, SELECTIONS_NS).len(), selection_count);
        assert_eq!(
            list_values(&service, SELECTION_POINTERS_NS).len(),
            pointer_count
        );
        assert_eq!(list_values(&service, EVENTS_NS).len(), event_count);
    }

    #[test]
    fn current_selection_uses_explicit_pointer_not_fixed_timestamp_sorting() {
        let service = test_service("selection_pointer");
        let published = publish_strategy_editor_surface(
            &service,
            json!({"strategyId": "strat_local_btc_demo"}),
        );
        let surface_id = published["body"]["surface"]["surfaceId"]
            .as_str()
            .expect("published surface id");

        let first = set_selection(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "surfaceId": surface_id,
                "nodeRef": "entry.logic",
                "shared": true,
            }),
            test_user_actor(),
        );
        assert_eq!(first["ok"], true);
        let second = set_selection(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "surfaceId": surface_id,
                "nodeRef": "risk.sizing",
                "shared": true,
            }),
            test_user_actor(),
        );
        assert_eq!(second["ok"], true);

        let current = current_selection(&service, &json!({"surfaceId": surface_id}));
        assert_eq!(
            current["nodeRef"]["nodeId"],
            second["body"]["selection"]["nodeRef"]["nodeId"]
        );
        assert_ne!(
            current["nodeRef"]["nodeId"],
            first["body"]["selection"]["nodeRef"]["nodeId"]
        );
    }

    #[test]
    fn demo_reset_clears_current_and_shared_selection_pointers() {
        let service = test_service("selection_pointer_reset");
        let published = publish_strategy_editor_surface(
            &service,
            json!({"strategyId": "strat_local_btc_demo"}),
        );
        let surface_id = published["body"]["surface"]["surfaceId"]
            .as_str()
            .expect("published surface id");
        let selected = set_selection(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "surfaceId": surface_id,
                "nodeRef": "risk.sizing",
                "shared": true,
            }),
            test_user_actor(),
        );
        assert_eq!(selected["ok"], true, "{selected:#}");
        assert_eq!(list_values(&service, SELECTIONS_NS).len(), 1);
        assert_eq!(list_values(&service, SELECTION_POINTERS_NS).len(), 4);

        let reset = service.handle_http("POST", "/demo/reset-btc", json!({}));
        assert_eq!(reset.status, 200, "{:#}", reset.body);
        assert_eq!(reset.body["resetApplied"], true);
        assert!(
            reset.body["namespaces"]
                .as_array()
                .expect("reset namespaces")
                .iter()
                .any(|namespace| namespace.as_str() == Some(SELECTION_POINTERS_NS)),
            "{:#}",
            reset.body
        );
        assert!(list_values(&service, SELECTIONS_NS).is_empty());
        assert!(list_values(&service, SELECTION_POINTERS_NS).is_empty());
        assert!(current_selection(&service, &json!({"surfaceId": surface_id})).is_null());
    }

    #[test]
    fn reset_fails_closed_when_a_sightline_namespace_cannot_be_cleared() {
        let mut attempted = Vec::new();
        let result = clear_reset_namespaces_with(|namespace| {
            attempted.push(namespace.to_string());
            if namespace == SELECTION_POINTERS_NS {
                Err("injected reset failure".to_string())
            } else {
                Ok(())
            }
        });

        assert_eq!(result, Err("injected reset failure".to_string()));
        assert_eq!(
            attempted.last().map(String::as_str),
            Some(SELECTION_POINTERS_NS)
        );
        assert!(!attempted.iter().any(|namespace| namespace == SNAPSHOTS_NS));
    }

    #[test]
    fn agent_context_requires_an_explicitly_shared_selection() {
        let service = test_service("unshared_context");
        publish_strategy_editor_surface(&service, json!({"strategyId": "strat_local_btc_demo"}));
        set_selection(
            &service,
            json!({"strategyId": "strat_local_btc_demo", "nodeRef": "strategy.risk", "shared": false}),
            test_user_actor(),
        );

        let denied = get_context(
            &service,
            json!({"actor": {"kind": "agent", "id": "agent.test"}}),
        );
        assert_eq!(denied["ok"], false);
        assert_eq!(denied["error"]["code"], "sightline_selection_not_shared");
        assert!(denied["body"]["context"].is_null());

        let current_selection = current_selection_context(
            &service,
            json!({"actor": {"kind": "agent", "id": "agent.test"}}),
        );
        assert_eq!(current_selection["ok"], false);
        assert_eq!(
            current_selection["error"]["code"],
            "sightline_selection_not_shared"
        );
        assert!(current_selection["body"]["selection"].is_null());
        assert!(current_selection["body"]["context"].is_null());

        let caller_claiming_user = get_context(
            &service,
            json!({"actor": {"kind": "user", "id": "user.local"}}),
        );
        assert_eq!(caller_claiming_user["ok"], false);
        assert_eq!(
            caller_claiming_user["error"]["code"],
            "sightline_selection_not_shared"
        );
    }

    #[test]
    fn proposal_routes_through_strategy_proposal_with_sightline_refs() {
        let service = test_service("proposal");
        publish_strategy_editor_surface(&service, json!({"strategyId": "strat_local_btc_demo"}));
        set_selection(
            &service,
            json!({"strategyId": "strat_local_btc_demo", "nodeRef": "strategy.risk", "shared": true}),
            test_user_actor(),
        );
        let proposal = create_proposal(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "summary": "Adjust local metadata for review",
                "patch": {"metadata": {
                    "last_agent_reviewed_node": "strategy.risk",
                    "secret_note": "fixture_proposal_value"
                }},
                "actor": {"kind": "agent", "id": "agent.test"}
            }),
        );
        assert_eq!(proposal["ok"], true);
        assert_eq!(proposal["body"]["proposal"]["status"], "pending");
        assert!(proposal["body"]["sightlineRefs"].as_array().unwrap().len() >= 2);
        assert!(!serde_json::to_string(&proposal)
            .unwrap()
            .contains("fixture_proposal_value"));
    }

    #[test]
    fn proposal_and_approval_force_agent_attribution() {
        let service = test_service("agent_attribution");
        publish_strategy_editor_surface(&service, json!({"strategyId": "strat_local_btc_demo"}));
        set_selection(
            &service,
            json!({"strategyId": "strat_local_btc_demo", "nodeRef": "strategy.risk", "shared": true}),
            test_user_actor(),
        );
        let spoofed_actor = json!({"kind": "user", "id": "spoofed.user"});

        let proposal = create_proposal(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "patch": {"metadata": {"reviewed": true}},
                "actor": spoofed_actor
            }),
        );
        assert_eq!(
            proposal["body"]["proposal"]["proposed_by"]["actor_type"],
            "agent"
        );

        let approval = request_approval(
            &service,
            json!({"strategyId": "strat_local_btc_demo", "actor": spoofed_actor}),
        );
        assert_eq!(
            approval["body"]["approval"]["requested_by"]["actor_type"],
            "agent"
        );
    }

    #[test]
    fn proposal_and_approval_require_a_shared_selection() {
        let service = test_service("missing_selection_mutations");
        publish_strategy_editor_surface(&service, json!({"strategyId": "strat_local_btc_demo"}));

        let proposal = create_proposal(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "patch": {"metadata": {"reviewed": true}},
                "actor": {"kind": "agent", "id": "agent.test"}
            }),
        );
        assert_eq!(proposal["ok"], false);
        assert_eq!(proposal["error"]["code"], "sightline_selection_not_shared");

        let approval = request_approval(
            &service,
            json!({
                "strategyId": "strat_local_btc_demo",
                "actor": {"kind": "agent", "id": "agent.test"}
            }),
        );
        assert_eq!(approval["ok"], false);
        assert_eq!(approval["error"]["code"], "sightline_selection_not_shared");
    }

    #[test]
    fn graphql_sightline_proposal_returns_strategy_review_payload() {
        let service = test_service("graphql");
        publish_strategy_editor_surface(&service, json!({"strategyId": "strat_local_btc_demo"}));
        set_selection(
            &service,
            json!({"strategyId": "strat_local_btc_demo", "nodeRef": "strategy.risk", "shared": true}),
            test_user_actor(),
        );
        let response = service.execute_graphql(json!({
            "operationName": "SightlineCreateProposal",
            "query": "mutation SightlineCreateProposal { sightlineCreateProposal }",
            "variables": {
                "strategyId": "strat_local_btc_demo",
                "summary": "Adjust local metadata for review",
                "patch": {"metadata": {"last_agent_reviewed_node": "strategy.risk"}},
                "actor": {"kind": "agent", "id": "agent.test"}
            }
        }));
        assert_eq!(response["data"]["sightlineCreateProposal"]["ok"], true);
        assert_eq!(
            response["data"]["sightlineCreateProposal"]["body"]["strategyProposal"]["status"],
            "pending_review"
        );
        assert!(
            response["data"]["sightlineCreateProposal"]["body"]["sightlineRefs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some(ROOM_ID))
        );
    }
}
