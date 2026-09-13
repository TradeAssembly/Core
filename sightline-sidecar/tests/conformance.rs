use serde_json::{json, Value};
use tradeassembly_sightline_sidecar::{
    conform_agent_connection, conform_approval, conform_command, conform_event, conform_node,
    conform_presence, conform_proposal, conform_room, conform_selection, conform_snapshot,
    conform_surface, event_visible_to_actor, redact_agent_value, resolved_context,
    selection_visible_to_actor, transport_contract, SIGHTLINE_GIT_REV,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/gc04-session.json")).expect("valid fixture")
}

#[test]
fn gc04_fixture_conforms_to_public_adapter_contract() {
    let fixture = fixture();
    assert_eq!(fixture["source_rev"], SIGHTLINE_GIT_REV);

    conform_room(&fixture["room"]).expect("room contract");
    conform_surface(&fixture["surface"]).expect("surface contract");
    conform_snapshot(&fixture["snapshot"]).expect("snapshot contract");
    conform_agent_connection(&fixture["agent_connection"]).expect("agent session contract");
    conform_presence(&fixture["presence"]).expect("presence contract");
    conform_proposal(&fixture["proposal"]).expect("proposal contract");
    conform_approval(&fixture["approval"]).expect("approval contract");
    conform_event(&fixture["selection_event"]).expect("event contract");

    let node = conform_node(&fixture["node"]).expect("semantic node and action contract");
    assert_eq!(node["actions"][1]["authority_level"], "propose");

    let navigation = conform_command(&fixture["navigation"]).expect("navigation command contract");
    assert_eq!(navigation["authority_level"], "navigate");
}

#[test]
fn gc04_visibility_redaction_and_context_fail_closed() {
    let fixture = fixture();
    let selection = conform_selection(&fixture["selection"]).expect("selection contract");
    assert!(!selection_visible_to_actor(&selection, &fixture["agent"]));
    assert!(selection_visible_to_actor(&selection, &fixture["user"]));
    assert!(!event_visible_to_actor(
        &fixture["selection_event"],
        &fixture["agent"]
    ));

    let mut shared_selection = selection;
    shared_selection["shared"] = json!(true);
    assert!(selection_visible_to_actor(
        &shared_selection,
        &fixture["agent"]
    ));

    let resolved = resolved_context(&shared_selection, &fixture["context"])
        .expect("resolved context contract");
    let serialized = serde_json::to_string(&resolved).expect("serialize context");
    assert!(!serialized.contains("fixture_api_value"));
    assert!(!serialized.contains("fixture_oauth_value"));
    assert!(!serialized.contains("fixture_private_value"));
    assert_eq!(resolved["context"]["credential_status"], "connected");
    assert_eq!(resolved["context"]["api_key"], "[REDACTED]");

    let redacted = redact_agent_value(&fixture["node"]["state"]);
    assert_eq!(redacted["credential_status"], "connected");
    assert_eq!(redacted["api_key"], "[REDACTED]");
    let hostile_status = redact_agent_value(&json!({
        "oauth_status": "fixture_oauth_value",
        "api_key_status": "non-pattern-secret",
        "credential_status": "connected"
    }));
    assert_eq!(hostile_status["oauth_status"], "[REDACTED]");
    assert_eq!(hostile_status["api_key_status"], "[REDACTED]");
    assert_eq!(hostile_status["credential_status"], "connected");
    assert!(!selection_visible_to_actor(&json!({}), &fixture["agent"]));
}

#[test]
fn gc04_transport_exposes_public_sightline_mcp_adapter() {
    let transport = transport_contract().expect("Sightline MCP adapter bindings");
    assert_eq!(
        transport["source"]["compatibility_target"]["rev"],
        SIGHTLINE_GIT_REV
    );
    assert_eq!(
        transport["source"]["compatibility_target"]["verification"],
        "verified_local_source"
    );
    assert_eq!(transport["protocol"]["protocol"], "mcp");
    assert_eq!(transport["protocol"]["protocol_version"], "2025-06-18");
    assert_eq!(transport["protocol"]["transport"], "mcp-json-rpc");
    assert_eq!(
        transport["tool_bindings"]
            .as_array()
            .expect("bindings")
            .len(),
        7
    );
}

#[test]
fn every_public_adapter_contract_rejects_missing_required_identity() {
    for result in [
        conform_room(&json!({})),
        conform_surface(&json!({})),
        conform_node(&json!({})),
        conform_snapshot(&json!({})),
        conform_agent_connection(&json!({})),
        conform_presence(&json!({})),
        conform_command(&json!({})),
        conform_proposal(&json!({})),
        conform_approval(&json!({})),
        conform_event(&json!({})),
    ] {
        assert!(result.is_err());
    }
}
