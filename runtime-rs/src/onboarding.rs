// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Public, read-only onboarding facts. Authentication controls private inspection.
use crate::service::TradeAssemblyService;
use serde_json::{json, Value};

pub fn inspect(
    service: Option<&TradeAssemblyService>,
    identity: Option<Value>,
    studio_base_url: &str,
) -> Value {
    inspect_for_surface(service, identity, studio_base_url, false)
}

pub fn inspect_for_surface(
    service: Option<&TradeAssemblyService>,
    identity: Option<Value>,
    studio_base_url: &str,
    agent_only: bool,
) -> Value {
    let authenticated = identity
        .as_ref()
        .is_some_and(|value| value["authenticated"] == true);
    let evidence = service.map(TradeAssemblyService::onboarding_snapshot);
    inspect_surface_facts(
        evidence,
        identity,
        authenticated,
        studio_base_url,
        agent_only,
    )
}

#[cfg(test)]
fn inspect_facts(
    evidence: Option<Value>,
    identity: Option<Value>,
    authenticated: bool,
    studio_base_url: &str,
) -> Value {
    inspect_surface_facts(evidence, identity, authenticated, studio_base_url, false)
}

fn inspect_surface_facts(
    evidence: Option<Value>,
    identity: Option<Value>,
    authenticated: bool,
    studio_base_url: &str,
    agent_only: bool,
) -> Value {
    // Do not expose supplied service facts without a verified identity.
    let evidence = evidence.filter(|_| authenticated);
    let connections = evidence.as_ref().and_then(|v| v["connections"].as_array());
    let connection_checks: Vec<Value> = connections.into_iter().flatten().map(|item| {
        let configured = item["configured"] == true;
        let stale = item["verificationStale"] == true;
        let verified = configured && !stale && item["accountVerified"] == true
            && item["verificationReceipt"].is_object();
        json!({
            "instanceRef": item["instanceRef"],
            "credentials": {"status": if configured { "passed" } else { "not_run" }},
            "accountRead": {
                "status": if stale { "stale" } else if verified { "passed" }
                    else if item["health"]["state"] == "degraded" { "failed" } else { "not_run" },
                "evidence": item["verificationReceipt"],
                "diagnostics": item["health"]["diagnostics"],
            },
            "restartObservation": item.get("restartObservation").cloned().unwrap_or_else(|| json!({"status": "not_run"})),
            "studioObservation": item.get("studioObservation").cloned().unwrap_or_else(|| json!({"status": "not_run"})),
        })
    }).collect();
    let verified = !connection_checks.is_empty()
        && connection_checks
            .iter()
            .all(|item| item["accountRead"]["status"] == "passed");
    let connection_action = connections
        .filter(|items| items.len() == 1)
        .and_then(|items| items[0].get("nextAction"))
        .filter(|action| action.is_object())
        .cloned();
    let restart_verified = verified
        && connection_checks
            .iter()
            .all(|item| item["restartObservation"]["status"] == "passed");
    let studio_verified = verified
        && connection_checks
            .iter()
            .all(|item| item["studioObservation"]["status"] == "passed");
    let complete = authenticated && verified && restart_verified && (agent_only || studio_verified);
    json!({
        "schemaVersion": "tradeassembly.setup.v1",
        "tooling": {"available": true},
        "identity": identity.unwrap_or_else(|| json!({"authenticated": false})),
        "workspace": evidence,
        "studioBaseUrl": studio_base_url,
        "setupOnly": true,
        "acceptance": {
            "schemaVersion": "tradeassembly.setup-acceptance.v1",
            "complete": complete,
            "requestedSurface": if agent_only { "agent" } else { "studio" },
            "tooling": {"status": "passed"},
            "identity": {"status": if authenticated { "passed" } else { "not_run" }},
            "connections": connection_checks,
            "studioParity": {"status": if agent_only { "not_requested" } else if studio_verified { "passed" } else { "not_run" }, "scope": "authenticated_server_observation", "reason": if agent_only { json!("Agent-only setup does not request Studio verification.") } else if studio_verified { Value::Null } else { json!("Use Check Studio setup on the matching connection. A configured URL alone is not evidence.") }},
            "restartVerification": {"status": if restart_verified { "passed" } else { "not_run" }, "reason": if restart_verified { Value::Null } else { json!("Stored receipts do not prove a fresh account read after restarting this installation.") }},
        },
        "nextAction": if complete {
            json!({"action": "none", "message": "Setup observations are complete. No strategy was created or activated."})
        } else if verified && agent_only {
            json!({"action": "verify_restart", "message": "Restart only this installation, perform a fresh broker.verify, and repeat setup.inspect with surface=agent. Never restart another installation."})
        } else if verified {
            json!({"action": "verify_studio_and_restart", "message": "Use Check Studio setup on the matching connection. Restart only this installation, perform a fresh broker.verify, and repeat Check Studio setup so both surfaces observe the new receipt. Never restart another installation."})
        } else if authenticated {
            connection_action.unwrap_or_else(|| json!({"tool": "tradeassembly.broker.status", "arguments": {}, "message": "Inspect connection diagnostics and select the intended connection. Configured credentials alone do not prove account connectivity. Strategy setup and activation are separate."}))
        } else {
            json!({"tool": "tradeassembly.account.login", "arguments": {}, "message": "Sign in in your browser."})
        },
        "automationReady": false,
        "message": "Setup inspection does not create, start, pause, or change any strategy."
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_surface_excludes_only_studio_not_broker_or_restart() {
        let facts = json!({"connections":[{
            "configured":true, "accountVerified":true,
            "verificationReceipt":{"id":"receipt"},
            "restartObservation":{"status":"passed"}
        }]});
        let project = |facts, authenticated| {
            inspect_surface_facts(
                Some(facts),
                Some(json!({"authenticated":authenticated})),
                authenticated,
                "http://127.0.0.1:3001",
                true,
            )
        };
        let result = project(facts.clone(), true);
        assert_eq!(result["acceptance"]["complete"], true);
        assert_eq!(
            result["acceptance"]["studioParity"]["status"],
            "not_requested"
        );
        assert_eq!(result["automationReady"], false);
        assert_eq!(
            project(facts.clone(), false)["acceptance"]["complete"],
            false
        );
        let mut stale = facts.clone();
        stale["connections"][0]["verificationStale"] = json!(true);
        assert_eq!(project(stale, true)["acceptance"]["complete"], false);
        let mut not_restarted = facts;
        not_restarted["connections"][0]["restartObservation"]["status"] = json!("not_run");
        let result = project(not_restarted, true);
        assert_eq!(result["acceptance"]["complete"], false);
        assert_eq!(result["nextAction"]["action"], "verify_restart");
    }

    #[test]
    fn signed_out_inspection_exposes_no_private_workspace() {
        let result = inspect(None, None, "http://127.0.0.1:3001");
        assert_eq!(result["nextAction"]["tool"], "tradeassembly.account.login");
        assert!(result["workspace"].is_null());
        assert_eq!(result["automationReady"], false);
    }

    #[test]
    fn configured_credentials_and_failed_health_are_not_acceptance() {
        let result = inspect_facts(
            Some(json!({"connections": [{
                "instanceRef":"test", "configured":true, "accountVerified":false,
                "health":{"state":"degraded","diagnostics":["plugin_response_invalid"]}
            }]})),
            Some(json!({"authenticated":true})),
            true,
            "http://127.0.0.1:3002",
        );
        assert_eq!(result["acceptance"]["complete"], false);
        assert_eq!(
            result["acceptance"]["connections"][0]["credentials"]["status"],
            "passed"
        );
        assert_eq!(
            result["acceptance"]["connections"][0]["accountRead"]["status"],
            "failed"
        );
    }

    #[test]
    fn receipt_does_not_prove_studio_or_restart_and_stale_receipts_fail() {
        for stale in [false, true] {
            let result = inspect_facts(
                Some(json!({"connections":[{
                    "instanceRef":"test", "configured":true, "accountVerified":true,
                    "verificationStale":stale, "verificationReceipt":{"id":"receipt"}
                }]})),
                Some(json!({"authenticated":true})),
                true,
                "http://127.0.0.1:3002",
            );
            assert_eq!(result["acceptance"]["complete"], false);
            assert_eq!(result["acceptance"]["studioParity"]["status"], "not_run");
            assert_eq!(
                result["acceptance"]["restartVerification"]["status"],
                "not_run"
            );
            assert_eq!(
                result["acceptance"]["connections"][0]["accountRead"]["status"],
                if stale { "stale" } else { "passed" }
            );
        }
    }

    #[test]
    fn completion_requires_every_current_service_observation() {
        let facts = json!({"connections":[{
            "instanceRef":"test", "configured":true, "accountVerified":true,
            "verificationStale":false, "verificationReceipt":{"id":"receipt"},
            "studioObservation":{"status":"passed"}, "restartObservation":{"status":"passed"}
        }]});
        let project = |facts, authenticated| {
            inspect_facts(
                Some(facts),
                Some(json!({"authenticated":authenticated})),
                authenticated,
                "http://127.0.0.1:3002",
            )
        };
        assert_eq!(project(facts.clone(), true)["acceptance"]["complete"], true);
        assert_eq!(
            project(facts.clone(), false)["acceptance"]["complete"],
            false
        );
        for observation in ["studioObservation", "restartObservation"] {
            for status in ["not_run", "failed", "stale", "unavailable"] {
                let mut incomplete = facts.clone();
                incomplete["connections"][0][observation]["status"] = json!(status);
                assert_eq!(project(incomplete, true)["acceptance"]["complete"], false);
            }
        }
        let mut stale = facts;
        stale["connections"][0]["verificationStale"] = json!(true);
        assert_eq!(project(stale, true)["acceptance"]["complete"], false);
    }

    #[test]
    fn unauthenticated_identity_object_cannot_expose_connection_facts() {
        let result = inspect_facts(
            Some(json!({"connections":[{"instanceRef":"private"}]})),
            Some(json!({"authenticated":false})),
            false,
            "http://127.0.0.1:3002",
        );
        assert!(result["workspace"].is_null());
        assert_eq!(result["acceptance"]["connections"], json!([]));
        assert_eq!(result["nextAction"]["tool"], "tradeassembly.account.login");
    }
}
