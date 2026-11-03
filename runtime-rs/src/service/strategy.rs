// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{
    api_result, evidence, strategy_id_from, workspace_evidence, TradeAssemblyService,
    LEGAL_BOUNDARY,
};
use crate::ports::{AuthorityContext, CapabilityBinding, IdempotencyKey, SideEffectContext};
use crate::spec;
use serde_json::{json, Value};

const STRATEGIES_NS: &str = "strategies";
const VERSIONS_NS: &str = "strategy_versions";
const DRAFTS_NS: &str = "strategy_drafts";
const PROPOSALS_NS: &str = "strategy_draft_proposals";
const SELECTIONS_NS: &str = "strategy_semantic_selections";
const LOCAL_TIMESTAMP: &str = "2026-07-08T00:00:00Z";

pub(crate) fn strategies(service: &TradeAssemblyService) -> Vec<Value> {
    let mut stored = service
        .runtime()
        .storage
        .list_json(STRATEGIES_NS)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let stored_ids = stored
        .iter()
        .filter_map(|strategy| strategy["id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    for strategy in default_strategies() {
        if !stored_ids
            .iter()
            .any(|stored_id| stored_id == strategy["id"].as_str().unwrap_or_default())
        {
            stored.push(strategy);
        }
    }
    stored.sort_by(|left, right| {
        let left_id = left["id"].as_str().unwrap_or_default();
        let right_id = right["id"].as_str().unwrap_or_default();
        match (
            left_id == "strat_local_btc_demo",
            right_id == "strat_local_btc_demo",
        ) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left_id.cmp(right_id),
        }
    });
    service.filter_visible_values("strategy", stored, &["id", "strategyId", "strategy_id"])
}

pub(crate) fn create_strategy(service: &TradeAssemblyService, body: Value) -> Value {
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Local strategy");
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| next_strategy_id(service, name));
    let blank = body.get("spec").is_none() && template_id(&body) == "blank";
    let spec = body.get("spec").cloned().unwrap_or_else(|| {
        if blank {
            json!({"spec_version": "3.0", "strategy_id": id, "name": name})
        } else {
            spec::strategy_template_spec_payload(template_id(&body))
        }
    });
    let strategy = strategy_record(
        &id,
        name,
        body.get("symbol")
            .and_then(Value::as_str)
            .unwrap_or(if blank { "" } else { "BTC/USD" }),
        body.get("providerRef")
            .or_else(|| body.get("provider_ref"))
            .and_then(Value::as_str)
            .unwrap_or(if blank { "" } else { "sim" }),
        "draft",
        0,
        spec,
    );
    if service.bind_owned_object("strategy", &id).is_err() {
        return json!({
            "ok": false,
            "body": {},
            "error": {"code": "object_authorization_unavailable"}
        });
    }
    let mut draft = draft_record(&strategy, strategy["latestSpec"].clone(), 1, None);
    let validation = validate_draft_report(&draft);
    let import_mode = body
        .get("creationMode")
        .or_else(|| body.get("creation_mode"))
        .or_else(|| body.get("mode"))
        .and_then(Value::as_str)
        == Some("import");
    let persistence_violations = draft_persistence_violations(&draft);
    if !persistence_violations.is_empty()
        || (!import_mode && !blank && !validation["ok"].as_bool().unwrap_or(false))
    {
        return json!({
            "ok": false,
            "body": {
                "strategy": strategy,
                "draft": draft,
                "validation": validation,
                "persistenceViolations": persistence_violations,
            },
            "error": {"code": "strategy_draft_invalid"}
        });
    }
    draft["status"] = json!(draft_status(&validation));
    persist_strategy(service, strategy.clone(), "strategy.created");
    persist_draft(service, draft.clone(), "strategy.draft.created");
    api_result(json!({
        "strategy": strategy,
        "draft": draft,
        "validation": validation,
        "journal": evidence("strategy.created"),
    }))
}

pub(crate) fn save_builder_draft(service: &TradeAssemblyService, body: Value) -> Value {
    let id = strategy_id_from(&body);
    if service.require_object("strategy", &id).is_err() {
        return unavailable_result();
    }
    materialize_default_version(service, &id);
    let mut strategy = strategy_value(service, &id);
    let current_draft = draft_value(service, &id);
    let next_revision = current_draft["revision"].as_u64().unwrap_or(0) + 1;
    let spec = body
        .get("spec")
        .cloned()
        .unwrap_or_else(|| current_draft["spec"].clone());
    let base_version = strategy["currentVersionId"].as_str().map(str::to_string);
    let mut pending_draft =
        draft_record(&strategy, spec.clone(), next_revision, base_version.clone());
    let capability_bindings = body
        .get("capabilityBindings")
        .or_else(|| body.get("capability_bindings"))
        .cloned()
        .unwrap_or_else(|| {
            current_draft
                .get("capabilityBindings")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}))
        });
    let capability_bindings = match serde_json::from_value::<
        std::collections::BTreeMap<String, CapabilityBinding>,
    >(capability_bindings)
    {
        Ok(bindings) => bindings,
        Err(_) => {
            return json!({
                "ok": false,
                "error": {
                    "code": "strategy_capability_bindings_invalid",
                    "message": "Capability bindings must map requirement ids to plugin operation references."
                }
            })
        }
    };
    pending_draft["capabilityBindings"] = json!(capability_bindings);
    let validation = validate_draft_report(&pending_draft);
    let persistence_violations = draft_persistence_violations(&pending_draft);
    if !persistence_violations.is_empty() {
        return json!({
            "ok": false,
            "body": {
                "strategy": strategy,
                "draft": current_draft,
                "draftSaved": false,
                "validation": validation,
                "persistenceViolations": persistence_violations,
            },
            "error": {"code": "strategy_draft_invalid"}
        });
    }
    pending_draft["status"] = json!(draft_status(&validation));
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| strategy["name"].as_str().unwrap_or("Local strategy"));
    let name = name.to_string();
    strategy["name"] = json!(name.clone());
    strategy["status"] = json!("draft");
    strategy["draft_revision"] = json!(next_revision);
    strategy["draftRevision"] = json!(next_revision);
    persist_strategy(service, strategy.clone(), "strategy.draft_saved");
    let mut draft = pending_draft;
    draft["name"] = json!(name);
    persist_draft(service, draft.clone(), "strategy.draft_saved");
    api_result(json!({
        "strategy": strategy,
        "draft": draft,
        "draftSaved": true,
        "validation": validation,
        "journal": evidence("strategy.draft_saved"),
    }))
}

pub(crate) fn validate_builder_draft(service: &TradeAssemblyService, body: Value) -> Value {
    let id = strategy_id_from(&body);
    if service.require_object("strategy", &id).is_err() {
        return unavailable_result();
    }
    let strategy = strategy_value(service, &id);
    let spec = body
        .get("spec")
        .cloned()
        .unwrap_or_else(|| draft_value(service, &id)["spec"].clone());
    let draft = draft_record(
        &strategy,
        spec,
        draft_value(service, &id)["revision"].as_u64().unwrap_or(1),
        strategy["currentVersionId"].as_str().map(str::to_string),
    );
    api_result(json!({
        "draft": draft,
        "validation": validate_draft_report(&draft),
        "compilation": crate::strategy_kernel::portfolio_program::compilation_report(&draft["spec"], spec::validate_strategy_spec_report(&draft["spec"]).valid),
    }))
}

pub(crate) fn publish_strategy(service: &TradeAssemblyService, body: Value) -> Value {
    let id = strategy_id_from(&body);
    if service.require_object("strategy", &id).is_err() {
        return unavailable_result();
    }
    if actor_context(&body)["kind"].as_str() == Some("agent") {
        return json!({
            "ok": false,
            "body": {"strategyId": id, "published": false},
            "error": {"code": "agent_cannot_publish_strategy_version"}
        });
    }
    let mut strategy = strategy_value(service, &id);
    let draft = draft_value(service, &id);
    let expected_draft_hash = body
        .get("expectedDraftHash")
        .or_else(|| body.get("expected_draft_hash"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let Some(expected_draft_hash) = expected_draft_hash else {
        return json!({
            "ok": false,
            "body": {"strategyId": id, "published": false},
            "error": {"code": "strategy_draft_hash_required"}
        });
    };
    if Some(expected_draft_hash) != draft["draftHash"].as_str() {
        return json!({
            "ok": false,
            "body": {"strategyId": id, "published": false, "draft": draft},
            "error": {"code": "strategy_draft_changed"}
        });
    }
    let validation = validate_draft_report(&draft);
    if !validation["ok"].as_bool().unwrap_or(false) {
        return json!({
            "ok": false,
            "body": {
                "strategy": strategy,
                "draft": draft,
                "validation": validation,
                "published": false,
            },
            "error": {"code": "strategy_draft_invalid"}
        });
    }
    let versions = strategy_versions(service, &id);
    let draft_hash = draft["draftHash"].as_str().unwrap_or_default();
    let current_version_id = strategy["currentVersionId"]
        .as_str()
        .or_else(|| strategy["current_version_id"].as_str());
    if let Some(version) = versions.iter().find(|version| {
        current_version_id == version["id"].as_str()
            && version["specHash"].as_str() == Some(draft_hash)
    }) {
        return api_result(json!({
            "strategy": strategy,
            "draft": draft,
            "version": version,
            "published": true,
            "replayed": true,
            "activation": {"started": false, "reason": "publishing does not activate execution"},
        }));
    }
    let next_version = versions
        .iter()
        .filter_map(|version| version["version"].as_u64())
        .max()
        .unwrap_or(0)
        + 1;
    let authority = authority_context_from_body(&body);
    let version = version_record(&id, next_version, draft["spec"].clone());
    persist_version_with_authority(
        service,
        &id,
        version.clone(),
        "strategy.version_published",
        authority.clone(),
    );
    strategy["status"] = json!("published");
    strategy["version"] = json!(next_version);
    strategy["current_version_id"] = version["id"].clone();
    strategy["currentVersionId"] = version["id"].clone();
    strategy["spec"] = draft["spec"].clone();
    strategy["latestSpec"] = draft["spec"].clone();
    persist_strategy_with_authority(
        service,
        strategy.clone(),
        "strategy.version_published",
        authority,
    );
    api_result(json!({
        "strategy": strategy,
        "draft": draft,
        "version": version,
        "published": true,
        "activation": {"started": false, "reason": "publishing does not activate execution"},
        "journal": evidence("strategy.version_published"),
    }))
}

pub(crate) fn select_strategy_semantic_node(service: &TradeAssemblyService, body: Value) -> Value {
    let id = strategy_id_from(&body);
    let node_ref = body
        .get("nodeRef")
        .or_else(|| body.get("node_ref"))
        .and_then(Value::as_str)
        .unwrap_or("strategy.root");
    let draft = draft_value(service, &id);
    let selection_id = format!("sel_{}_{}", slug(&id), slug(node_ref));
    let selection = json!({
        "id": selection_id,
        "strategyId": id,
        "draftId": draft["id"],
        "draftHash": draft["draftHash"],
        "nodeRef": node_ref,
        "objectRef": format!("tradeassembly://strategy/{id}/draft/{}", draft["revision"].as_u64().unwrap_or(1)),
        "redaction": "safe_inline",
        "context": semantic_context_for(&draft["spec"], node_ref),
        "evidenceRefs": [format!("hash:{}", short_hash(draft["draftHash"].as_str().unwrap_or_default()))],
        "createdAt": LOCAL_TIMESTAMP,
    });
    persist_json(
        service,
        SELECTIONS_NS,
        &selection_id,
        selection.clone(),
        "strategy.semantic_selected",
    );
    api_result(json!({"selection": selection, "journal": evidence("strategy.semantic_selected")}))
}

pub(crate) fn propose_strategy_patch(service: &TradeAssemblyService, body: Value) -> Value {
    let id = strategy_id_from(&body);
    let draft = draft_value(service, &id);
    let expected_hash = body
        .get("expectedDraftHash")
        .or_else(|| body.get("expected_draft_hash"))
        .and_then(Value::as_str);
    let status = if expected_hash.is_some() && expected_hash != draft["draftHash"].as_str() {
        "conflict"
    } else {
        "pending_review"
    };
    let patch = body
        .get("specPatch")
        .or_else(|| body.get("spec_patch"))
        .or_else(|| body.get("patch"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let patch_violations = patch_runtime_boundary_violations(&patch);
    if !patch_violations.is_empty() {
        return json!({
            "ok": false,
            "body": {
                "strategyId": id,
                "status": "rejected",
                "violations": patch_violations,
            },
            "error": {"code": "strategy_patch_contains_runtime_binding"}
        });
    }
    let affected_paths = affected_paths(&patch);
    let risk_level = risk_level_for_paths(&affected_paths);
    let mut proposal_identity = body.clone();
    if let Some(fields) = proposal_identity.as_object_mut() {
        fields.remove("proposalId");
        fields.remove("proposal_id");
    }
    let proposal_id = format!(
        "proposal_{}_{}",
        slug(&id),
        short_hash(&payload_hash(&proposal_identity))
    );
    let proposal = json!({
        "id": proposal_id,
        "strategyId": id,
        "draftId": draft["id"],
        "baseDraftHash": draft["draftHash"],
        "status": status,
        "summary": body.get("summary").and_then(Value::as_str).unwrap_or("Agent proposed strategy edit"),
        "actor": actor_context(&body),
        "client": body.get("client").cloned().unwrap_or_else(|| json!("self")),
        "purpose": body.get("purpose").and_then(Value::as_str).unwrap_or("strategy_authoring"),
        "evidenceRefs": evidence_refs_from(&body, &draft),
        "selectionRef": body.get("selectionRef").or_else(|| body.get("selection_ref")).cloned().unwrap_or(json!(null)),
        "patch": redact_sensitive_json(&patch),
        "affectedPaths": affected_paths,
        "riskLevel": risk_level,
        "requiresExplicitReview": risk_level == "high",
        "policyPaths": policy_paths_for(&affected_paths),
        "createdAt": LOCAL_TIMESTAMP,
    });
    let proposal_id = proposal["id"]
        .as_str()
        .unwrap_or("proposal_local")
        .to_string();
    if service
        .bind_inherited_object("strategy_proposal", &proposal_id, "strategy", &id)
        .is_err()
    {
        return unavailable_result();
    }
    persist_json(
        service,
        PROPOSALS_NS,
        &proposal_id,
        proposal.clone(),
        "strategy.patch.proposed",
    );
    api_result(json!({"proposal": proposal, "journal": evidence("strategy.patch.proposed")}))
}

pub(crate) fn review_strategy_patch(service: &TradeAssemblyService, body: Value) -> Value {
    let id = strategy_id_from(&body);
    let proposal_id = body
        .get("proposalId")
        .or_else(|| body.get("proposal_id"))
        .and_then(Value::as_str)
        .unwrap_or("proposal_local");
    let decision = body
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("reject");
    if service
        .require_object("strategy_proposal", proposal_id)
        .is_err()
    {
        return unavailable_result();
    }
    let Some(mut proposal) = service
        .runtime()
        .storage
        .get_json(PROPOSALS_NS, proposal_id)
        .ok()
        .flatten()
    else {
        return unavailable_result();
    };
    if proposal["strategyId"].as_str() != Some(id.as_str()) {
        return unavailable_result();
    }
    if matches!(decision, "accept" | "accepted" | "apply")
        && actor_context(&body)["kind"].as_str() == Some("agent")
    {
        return json!({
            "ok": false,
            "body": {"proposalId": proposal_id, "decision": decision},
            "error": {"code": "agent_cannot_apply_strategy_patch"}
        });
    }
    let current_draft = draft_value(service, &id);
    if matches!(decision, "accept" | "accepted" | "apply")
        && proposal["baseDraftHash"] != current_draft["draftHash"]
    {
        proposal["status"] = json!("conflict");
        persist_json(
            service,
            PROPOSALS_NS,
            proposal_id,
            proposal.clone(),
            "strategy.patch.conflict",
        );
        return api_result(json!({
            "proposal": proposal,
            "draft": current_draft,
            "applied": false,
            "conflict": true,
            "journal": evidence("strategy.patch.conflict"),
        }));
    }
    if matches!(decision, "accept" | "accepted" | "apply") {
        if proposal["riskLevel"].as_str() == Some("high") && !has_explicit_high_impact_review(&body)
        {
            return json!({
                "ok": false,
                "body": {
                    "proposal": proposal,
                    "draft": current_draft,
                    "applied": false,
                    "required": ["highImpactAcknowledged", "approvalMode", "evidenceRefs"],
                },
                "error": {"code": "high_impact_strategy_patch_requires_explicit_review"}
            });
        }
        let merged = merge_patch(current_draft["spec"].clone(), proposal["patch"].clone());
        let merged_draft = draft_record(
            &strategy_value(service, &id),
            merged.clone(),
            current_draft["revision"].as_u64().unwrap_or(0) + 1,
            current_draft["baseVersionId"].as_str().map(str::to_string),
        );
        let validation = validate_draft_report(&merged_draft);
        if !validation["ok"].as_bool().unwrap_or(false) {
            return json!({
                "ok": false,
                "body": {
                    "proposal": proposal,
                    "draft": current_draft,
                    "validation": validation,
                    "applied": false,
                },
                "error": {"code": "strategy_patch_invalid_after_merge"}
            });
        }
        let saved = save_builder_draft(
            service,
            json!({
                "strategyId": id,
                "name": merged["name"].as_str().unwrap_or_else(|| current_draft["name"].as_str().unwrap_or("Local strategy")),
                "spec": merged,
            }),
        );
        proposal["status"] = json!("accepted");
        proposal["reviewedAt"] = json!(LOCAL_TIMESTAMP);
        proposal["reviewer"] = actor_context(&body);
        proposal["approval"] = json!({
            "mode": body.get("approvalMode").or_else(|| body.get("approval_mode")).and_then(Value::as_str).unwrap_or("explicit_user_review"),
            "highImpactAcknowledged": body.get("highImpactAcknowledged").or_else(|| body.get("high_impact_acknowledged")).and_then(Value::as_bool).unwrap_or(false),
            "evidenceRefs": body.get("evidenceRefs").or_else(|| body.get("evidence_refs")).cloned().unwrap_or_else(|| json!([])),
        });
        persist_json(
            service,
            PROPOSALS_NS,
            proposal_id,
            proposal.clone(),
            "strategy.patch.accepted",
        );
        return api_result(json!({
            "proposal": proposal,
            "draft": saved["body"]["draft"].clone(),
            "applied": true,
            "journal": evidence("strategy.patch.accepted"),
        }));
    }
    proposal["status"] = json!(match decision {
        "edit" | "edited" => "edited",
        "refresh" | "refreshed" => "refresh_requested",
        _ => "rejected",
    });
    proposal["reviewedAt"] = json!(LOCAL_TIMESTAMP);
    proposal["reviewer"] = actor_context(&body);
    persist_json(
        service,
        PROPOSALS_NS,
        proposal_id,
        proposal.clone(),
        "strategy.patch_reviewed",
    );
    api_result(json!({
        "proposal": proposal,
        "draft": current_draft,
        "applied": false,
        "journal": evidence("strategy.patch_reviewed"),
    }))
}

pub(crate) fn strategy_proposals(service: &TradeAssemblyService, id: &str) -> Vec<Value> {
    let mut proposals = service
        .runtime()
        .storage
        .list_json(PROPOSALS_NS)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, proposal)| proposal["strategyId"].as_str() == Some(id))
        .map(|(_, proposal)| proposal)
        .collect::<Vec<_>>();
    proposals.sort_by(|left, right| {
        left["id"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["id"].as_str().unwrap_or_default())
    });
    proposals
}

pub(crate) fn semantic_sections(strategy_id: &str, draft: &Value) -> Vec<Value> {
    let spec = &draft["spec"];
    vec![
        semantic_section(strategy_id, draft, "strategy.summary", "Summary", spec),
        semantic_section(
            strategy_id,
            draft,
            "market.requirements",
            "Market requirements",
            &semantic_payload(spec, "market.requirements"),
        ),
        semantic_section(
            strategy_id,
            draft,
            "entry.logic",
            "Entry logic",
            &semantic_payload(spec, "entry.logic"),
        ),
        semantic_section(
            strategy_id,
            draft,
            "risk.sizing",
            "Risk and sizing",
            &semantic_payload(spec, "risk.sizing"),
        ),
        semantic_section(
            strategy_id,
            draft,
            "orders.price_policy",
            "Orders and price policy",
            &semantic_payload(spec, "orders.price_policy"),
        ),
        semantic_section(
            strategy_id,
            draft,
            "exit.policy",
            "Exit policy",
            &semantic_payload(spec, "exit.policy"),
        ),
        semantic_section(
            strategy_id,
            draft,
            "capability.requirements",
            "Capability requirements",
            &semantic_payload(spec, "capability.requirements"),
        ),
    ]
}

pub(crate) fn draft_value(service: &TradeAssemblyService, id: &str) -> Value {
    if let Ok(Some(draft)) = service.runtime().storage.get_json(DRAFTS_NS, id) {
        return draft;
    }
    let strategy = strategy_value(service, id);
    draft_record(
        &strategy,
        strategy["latestSpec"].clone(),
        strategy["draftRevision"].as_u64().unwrap_or(1),
        strategy["currentVersionId"].as_str().map(str::to_string),
    )
}

pub(crate) fn duplicate_strategy(service: &TradeAssemblyService, body: Value) -> Value {
    let source_id = strategy_id_from(&body);
    if service.require_object("strategy", &source_id).is_err() {
        return unavailable_result();
    }
    let source = strategy_value(service, &source_id);
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("BTC duplicate proof");
    let id = next_strategy_id(service, name);
    let strategy = strategy_record(
        &id,
        name,
        source["symbol"].as_str().unwrap_or("BTC/USD"),
        source["providerRef"].as_str().unwrap_or("sim"),
        "draft",
        0,
        source["latestSpec"].clone(),
    );
    if service.bind_owned_object("strategy", &id).is_err() {
        return unavailable_result();
    }
    persist_strategy(service, strategy.clone(), "strategy.duplicated");
    let draft = draft_record(&strategy, strategy["latestSpec"].clone(), 1, None);
    persist_draft(service, draft.clone(), "strategy.draft.created");
    api_result(
        json!({"strategy": strategy, "draft": draft, "journal": evidence("strategy.duplicated")}),
    )
}

pub(crate) fn set_strategy_status(service: &TradeAssemblyService, id: &str, status: &str) -> Value {
    if service.require_object("strategy", id).is_err() {
        return unavailable_result();
    }
    let mut strategy = strategy_value(service, id);
    strategy["status"] = json!(status);
    persist_strategy(service, strategy.clone(), "strategy.status_changed");
    strategy
}

pub(crate) fn strategy_value(service: &TradeAssemblyService, id: &str) -> Value {
    if service.require_object("strategy", id).is_err() {
        return Value::Null;
    }
    if let Ok(Some(strategy)) = service.runtime().storage.get_json(STRATEGIES_NS, id) {
        return strategy;
    }
    default_strategy(id)
}

pub(crate) fn strategy_detail(service: &TradeAssemblyService, id: &str) -> Value {
    json!({"strategy": strategy_value(service, id), "evidence": workspace_evidence(service)})
}

pub(crate) fn strategy_versions(service: &TradeAssemblyService, id: &str) -> Vec<Value> {
    if service.require_object("strategy", id).is_err() {
        return Vec::new();
    }
    let mut versions = service
        .runtime()
        .storage
        .list_json(VERSIONS_NS)
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| key.starts_with(&format!("{id}:")))
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let stored_strategy = service
        .runtime()
        .storage
        .get_json(STRATEGIES_NS, id)
        .ok()
        .flatten()
        .is_some();
    if versions.is_empty() && !stored_strategy {
        let strategy = strategy_value(service, id);
        if strategy["version"].as_u64().unwrap_or(0) > 0 {
            versions.push(version_record(id, 1, strategy["latestSpec"].clone()));
        }
    }
    versions.sort_by_key(|version| version["version"].as_u64().unwrap_or(0));
    versions
}

pub(crate) fn latest_version(service: &TradeAssemblyService, id: &str) -> Value {
    strategy_versions(service, id).pop().unwrap_or(Value::Null)
}

pub(crate) fn reset_local_demo(
    service: &TradeAssemblyService,
) -> Result<Vec<&'static str>, String> {
    let mut namespaces = vec![
        STRATEGIES_NS,
        VERSIONS_NS,
        DRAFTS_NS,
        PROPOSALS_NS,
        SELECTIONS_NS,
        "control_plane_commands",
        "control_plane_sequences",
        super::capability_graph::REVISIONS_NS,
        super::capability_graph::IDEMPOTENCY_NS,
        "finance_action_descriptors",
        "warden_decisions",
        "finance_receipts",
        "warden_downstream_outcomes",
        "execution_idempotency",
        "execution_orders",
        "execution_order_attempts",
        "risk_reservations",
        "positions",
        "worker_leases",
        "backtests",
        "research_artifacts",
        "research_datasets",
        "research_jobs",
        "research_sweeps",
        "research_universes",
        "portfolio_risk_snapshots",
        "fill_quality_reports",
        "attribution_journal_reviews",
        "attribution_journal_exports",
        "lifecycle_calendar_timelines",
        "research_notebooks",
        "research_notebook_exports",
        "dataset_ingestions_v1",
        "dataset_ingestion_idempotency_v1",
        "dataset_snapshots_v1",
        "backtest_manifests_v1",
        "backtest_results_v1",
        "backtest_runs_v2",
        "backtest_attempts_v1",
        "backtest_lifecycle_events_v1",
    ];
    namespaces.extend_from_slice(super::execution::RESET_NAMESPACES);
    namespaces.extend_from_slice(crate::adapters::plugin_operations::RESET_NAMESPACES);
    namespaces.extend_from_slice(crate::adapters::plugin_registry::RESET_NAMESPACES);
    namespaces.sort_unstable();
    namespaces.dedup();
    for namespace in &namespaces {
        service.runtime().storage.clear_namespace(namespace)?;
    }
    Ok(namespaces)
}

fn persist_strategy(service: &TradeAssemblyService, strategy: Value, event_type: &str) {
    persist_strategy_with_authority(service, strategy, event_type, AuthorityContext::local_cli());
}

fn persist_strategy_with_authority(
    service: &TradeAssemblyService,
    strategy: Value,
    event_type: &str,
    authority: AuthorityContext,
) {
    let id = strategy["id"].as_str().unwrap_or("strat_local_unknown");
    let context = SideEffectContext::new(
        authority,
        IdempotencyKey::new(format!(
            "{event_type}:{id}:{}",
            strategy["draftRevision"]
                .as_u64()
                .or_else(|| strategy["version"].as_u64())
                .unwrap_or(0)
        ))
        .expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(STRATEGIES_NS, id, strategy.clone(), &context)
        .expect("persist strategy");
    service
        .runtime()
        .record_side_effect(event_type, strategy, &context)
        .expect("journal strategy change");
}

fn persist_draft(service: &TradeAssemblyService, draft: Value, event_type: &str) {
    let strategy_id = draft["strategyId"]
        .as_str()
        .unwrap_or("strat_local_unknown")
        .to_string();
    persist_json(service, DRAFTS_NS, &strategy_id, draft, event_type);
}

fn persist_version(
    service: &TradeAssemblyService,
    strategy_id: &str,
    version: Value,
    event_type: &str,
) {
    persist_version_with_authority(
        service,
        strategy_id,
        version,
        event_type,
        AuthorityContext::local_cli(),
    );
}

fn persist_version_with_authority(
    service: &TradeAssemblyService,
    strategy_id: &str,
    version: Value,
    event_type: &str,
    authority: AuthorityContext,
) {
    let key = format!(
        "{strategy_id}:{}",
        version["id"].as_str().unwrap_or("version_local")
    );
    persist_json_with_authority(service, VERSIONS_NS, &key, version, event_type, authority);
}

fn persist_json(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    payload: Value,
    event_type: &str,
) {
    persist_json_with_authority(
        service,
        namespace,
        key,
        payload,
        event_type,
        AuthorityContext::local_cli(),
    );
}

fn persist_json_with_authority(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    payload: Value,
    event_type: &str,
    authority: AuthorityContext,
) {
    let context = SideEffectContext::new(
        authority,
        IdempotencyKey::new(format!("{event_type}:{namespace}:{key}"))
            .expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(namespace, key, payload.clone(), &context)
        .expect("persist strategy object");
    service
        .runtime()
        .record_side_effect(event_type, payload, &context)
        .expect("journal strategy object change");
}

fn authority_context_from_body(body: &Value) -> AuthorityContext {
    let context = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    AuthorityContext {
        actor: context
            .and_then(|value| value.get("actor").or_else(|| value.get("principalUser")))
            .and_then(Value::as_str)
            .unwrap_or("local-user")
            .to_string(),
        surface: context
            .and_then(|value| value.get("surface"))
            .and_then(Value::as_str)
            .unwrap_or("local")
            .to_string(),
        account_mode: body
            .get("accountMode")
            .or_else(|| body.get("account_mode"))
            .or_else(|| context.and_then(|value| value.get("accountMode")))
            .and_then(Value::as_str)
            .unwrap_or("paper")
            .to_string(),
    }
}

pub(crate) fn materialize_default_version(service: &TradeAssemblyService, id: &str) {
    let stored_version = service
        .runtime()
        .storage
        .list_json(VERSIONS_NS)
        .unwrap_or_default()
        .into_iter()
        .any(|(key, _)| key.starts_with(&format!("{id}:")));
    if stored_version {
        return;
    }
    let strategy = service
        .runtime()
        .storage
        .get_json(STRATEGIES_NS, id)
        .ok()
        .flatten()
        .unwrap_or_else(|| default_strategy(id));
    let version = strategy["version"].as_u64().unwrap_or(0);
    if version > 0 {
        persist_version(
            service,
            id,
            version_record(id, version, strategy["latestSpec"].clone()),
            "strategy.version_materialized",
        );
    }
}

pub(crate) fn seed_default_version(service: &TradeAssemblyService, id: &str) {
    let stored_version = service
        .runtime()
        .storage
        .list_json(VERSIONS_NS)
        .unwrap_or_default()
        .into_iter()
        .any(|(key, _)| key.starts_with(&format!("{id}:")));
    if stored_version {
        return;
    }
    let default = default_strategy(id);
    if default["version"].as_u64().unwrap_or(0) > 0 {
        persist_version(
            service,
            id,
            version_record(
                id,
                default["version"].as_u64().unwrap_or(1),
                default["latestSpec"].clone(),
            ),
            "strategy.version_materialized",
        );
    }
}

fn next_strategy_id(service: &TradeAssemblyService, name: &str) -> String {
    let count = service
        .runtime()
        .storage
        .list_json(STRATEGIES_NS)
        .map(|items| items.len())
        .unwrap_or_default()
        + 1;
    format!("strat_{}{}", slug(name), count)
}

fn unavailable_result() -> Value {
    json!({
        "ok": false,
        "body": {},
        "error": {"code": "object_not_available"}
    })
}

fn strategy_record(
    id: &str,
    name: &str,
    symbol: &str,
    provider_ref: &str,
    status: &str,
    version: u64,
    mut spec: Value,
) -> Value {
    if let Some(object) = spec.as_object_mut() {
        object.insert("strategy_id".to_string(), json!(id));
        object.insert("name".to_string(), json!(name));
    }
    let asset_class = if symbol.is_empty() {
        ""
    } else if symbol.contains('/') {
        "crypto"
    } else {
        "equity"
    };
    let current_version_id = (version > 0).then(|| version_id(id, version));
    json!({
        "id": id,
        "name": name,
        "symbol": symbol,
        "asset_class": asset_class,
        "assetClass": asset_class,
        "status": status,
        "version": version,
        "draft_revision": 1,
        "draftRevision": 1,
        "current_version_id": current_version_id,
        "currentVersionId": current_version_id,
        "provider_ref": provider_ref,
        "providerRef": provider_ref,
        "spec": spec,
        "latestSpec": spec,
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn version_record(strategy_id: &str, version: u64, spec: Value) -> Value {
    let identity = spec_identity(&spec);
    json!({
        "id": version_id(strategy_id, version),
        "strategy_id": strategy_id,
        "strategyId": strategy_id,
        "version": version,
        "spec": spec,
        "specHash": identity.canonical_hash,
        "identityAlgorithm": "RFC8785+SHA-256",
        "sourceRepresentation": "normalized_parsed_json_value",
        "normalizedSourceSpecBytes": identity.normalized_source_bytes,
        "normalizedSourceSpecHash": identity.normalized_source_hash,
        "canonicalSpecBytes": identity.canonical_bytes,
        "canonicalSpecHash": identity.canonical_hash,
        "immutable": true,
        "publishedAt": LOCAL_TIMESTAMP,
    })
}

fn draft_record(
    strategy: &Value,
    spec: Value,
    revision: u64,
    base_version_id: Option<String>,
) -> Value {
    let strategy_id = strategy["id"].as_str().unwrap_or("strat_local_unknown");
    let name = spec["name"]
        .as_str()
        .or_else(|| strategy["name"].as_str())
        .unwrap_or("Local strategy");
    let identity = spec_identity(&spec);
    json!({
        "id": format!("draft_{}_r{revision}", slug(strategy_id)),
        "strategyId": strategy_id,
        "strategy_id": strategy_id,
        "name": name,
        "status": "draft",
        "revision": revision,
        "baseVersionId": base_version_id,
        "draftHash": identity.canonical_hash,
        "specHash": identity.canonical_hash,
        "identityAlgorithm": "RFC8785+SHA-256",
        "sourceRepresentation": "normalized_parsed_json_value",
        "normalizedSourceSpecHash": identity.normalized_source_hash,
        "canonicalSpecHash": identity.canonical_hash,
        "spec": spec,
        "capabilityBindings": {},
        "updatedAt": LOCAL_TIMESTAMP,
    })
}

fn validate_draft_report(draft: &Value) -> Value {
    let spec_payload = draft["spec"].clone();
    let report = spec::validate_strategy_spec_report(&spec_payload);
    let spec_ok = report.valid;
    let spec_errors = report
        .diagnostics
        .iter()
        .map(spec::Diagnostic::display_line)
        .collect::<Vec<_>>();
    let mut checks = Vec::new();
    push_check(&mut checks, "spec", spec_ok, spec_errors.join("; "));
    let enclosing_strategy_id = draft["strategyId"]
        .as_str()
        .or_else(|| draft["strategy_id"].as_str())
        .unwrap_or_default();
    let spec_strategy_id = spec_payload["strategy_id"].as_str().unwrap_or_default();
    push_check(
        &mut checks,
        "strategy_family_identity",
        !enclosing_strategy_id.is_empty() && spec_strategy_id == enclosing_strategy_id,
        format!(
            "StrategySpec strategy_id must match enclosing strategy family: expected {enclosing_strategy_id}, got {spec_strategy_id}."
        ),
    );
    let signal_markets_declared = spec_payload["signal_market_requirements"]
        .as_array()
        .is_some_and(|requirements| !requirements.is_empty());
    let trade_markets_declared = spec_payload["trade_market_requirements"]
        .as_array()
        .is_some_and(|requirements| !requirements.is_empty());
    push_check(
        &mut checks,
        "signal_market_requirements",
        signal_markets_declared,
        "At least one signal market requirement is required.",
    );
    push_check(
        &mut checks,
        "trade_market_requirements",
        trade_markets_declared,
        "At least one trade market requirement is required.",
    );
    let required_capabilities = spec_payload["capability_requirements"]["required"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    push_check(
        &mut checks,
        "capability_requirements",
        required_capabilities > 0,
        "At least one capability requirement is required.",
    );
    let runtime_violations = spec::strategy_spec_runtime_boundary_violations(&spec_payload);
    push_check(
        &mut checks,
        "portable_strategy_spec",
        runtime_violations.is_empty(),
        runtime_violations.join("; "),
    );
    let ok = checks
        .iter()
        .all(|check: &Value| check["ok"].as_bool().unwrap_or(false));
    json!({
        "ok": ok,
        "report": report,
        "checks": checks,
        "errors": checks.iter().filter(|check| !check["ok"].as_bool().unwrap_or(false)).map(|check| check["detail"].clone()).collect::<Vec<_>>(),
        "readiness": {
            "specValid": spec_ok,
            "marketRequirementsDeclared": signal_markets_declared && trade_markets_declared,
            "capabilityDeclared": required_capabilities > 0,
            "workspaceCompatible": false,
            "researchReady": ok,
            "paperReady": false,
            "liveReady": false,
            "spec": spec_ok,
            "capabilities": required_capabilities > 0,
            "execution": false,
            "live": false,
            "blockedReasons": if ok { json!(["execution_config_required", "human_acknowledgement_required"]) } else { json!(["draft_validation_failed"]) },
        },
    })
}

fn draft_status(validation: &Value) -> &'static str {
    if validation["ok"].as_bool().unwrap_or(false) {
        "draft"
    } else {
        "invalid"
    }
}

fn draft_persistence_violations(draft: &Value) -> Vec<String> {
    let spec_payload = &draft["spec"];
    let enclosing_strategy_id = draft["strategyId"]
        .as_str()
        .or_else(|| draft["strategy_id"].as_str())
        .unwrap_or_default();
    let spec_strategy_id = spec_payload["strategy_id"].as_str().unwrap_or_default();
    let mut violations = spec::strategy_spec_runtime_boundary_violations(spec_payload);
    if enclosing_strategy_id.is_empty() || spec_strategy_id != enclosing_strategy_id {
        violations.push(format!(
            "StrategySpec strategy_id must match enclosing strategy family: expected {enclosing_strategy_id}, got {spec_strategy_id}."
        ));
    }
    violations.sort();
    violations.dedup();
    violations
}

struct SpecIdentity {
    normalized_source_bytes: String,
    normalized_source_hash: String,
    canonical_bytes: String,
    canonical_hash: String,
}

fn spec_identity(value: &Value) -> SpecIdentity {
    let original = serde_json::to_vec(value).expect("StrategySpec Value must serialize");
    let canonical = spec::canonical_bytes(value).expect("StrategySpec Value must canonicalize");
    SpecIdentity {
        normalized_source_bytes: String::from_utf8(original.clone()).expect("JSON is UTF-8"),
        normalized_source_hash: spec::raw_hash(&original),
        canonical_bytes: String::from_utf8(canonical.clone()).expect("canonical JSON is UTF-8"),
        canonical_hash: spec::raw_hash(&canonical),
    }
}

fn push_check(checks: &mut Vec<Value>, label: &str, ok: bool, detail: impl Into<String>) {
    checks.push(json!({
        "label": label,
        "ok": ok,
        "detail": if ok { "Pass".to_string() } else { detail.into() },
    }));
}

fn semantic_section(
    strategy_id: &str,
    draft: &Value,
    node_ref: &str,
    label: &str,
    payload: &Value,
) -> Value {
    json!({
        "nodeRef": node_ref,
        "label": label,
        "strategyId": strategy_id,
        "draftId": draft["id"],
        "draftHash": draft["draftHash"],
        "objectRef": format!("tradeassembly://strategy/{strategy_id}/node/{node_ref}"),
        "summary": semantic_summary(payload),
        "safeForAgent": patch_runtime_boundary_violations(payload).is_empty(),
    })
}

fn semantic_context_for(spec: &Value, node_ref: &str) -> Value {
    let payload = semantic_payload(spec, node_ref);
    json!({
        "nodeRef": node_ref,
        "payload": redact_sensitive_json(&payload),
        "redaction": "safe_inline_redacted",
        "copyContext": format!("Selected StrategySpec node {node_ref}; propose changes through strategy draft proposal review."),
    })
}

pub(crate) fn semantic_payload(spec: &Value, node_ref: &str) -> Value {
    match node_ref {
        "market.requirements" => json!({
            "signal_market_requirements": spec["signal_market_requirements"].clone(),
            "trade_market_requirements": spec["trade_market_requirements"].clone(),
        }),
        "entry.logic" => spec["stages"]["evaluate"].clone(),
        "risk.sizing" => json!({
            "allocation": spec["allocation"].clone(),
            "stages": {
                "sizing": spec["stages"]["sizing"].clone(),
                "risk": spec["stages"]["risk"].clone(),
            },
        }),
        "orders.price_policy" => json!({
            "stages": {
                "order_strategy": spec["stages"]["order_strategy"].clone(),
                "price_policy": spec["stages"]["price_policy"].clone(),
            },
        }),
        "exit.policy" => spec["stages"]["exit_policy"].clone(),
        "capability.requirements" => spec["capability_requirements"].clone(),
        _ => spec.clone(),
    }
}

fn semantic_summary(payload: &Value) -> String {
    if payload.is_null() {
        return "No data recorded".to_string();
    }
    if let Some(object) = payload.as_object() {
        let keys = object.keys().take(4).cloned().collect::<Vec<_>>();
        return keys.join(", ");
    }
    payload.to_string().chars().take(90).collect()
}

fn actor_context(body: &Value) -> Value {
    if let Some(actor) = body.get("actor") {
        return actor.clone();
    }
    let kind = body
        .get("actorKind")
        .or_else(|| body.get("actor_kind"))
        .and_then(Value::as_str)
        .unwrap_or("user");
    json!({
        "kind": kind,
        "id": body.get("actorId").or_else(|| body.get("actor_id")).and_then(Value::as_str).unwrap_or(if kind == "agent" { "agent.local" } else { "user.local" }),
        "principalUser": body.get("principalUser").or_else(|| body.get("principal_user")).and_then(Value::as_str).unwrap_or("user.local"),
    })
}

fn evidence_refs_from(body: &Value, draft: &Value) -> Vec<Value> {
    if let Some(items) = body
        .get("evidenceRefs")
        .or_else(|| body.get("evidence_refs"))
        .and_then(Value::as_array)
    {
        return items.clone();
    }
    vec![json!({
        "kind": "draft_hash",
        "ref": draft["draftHash"],
        "encrypted": false,
    })]
}

fn has_explicit_high_impact_review(body: &Value) -> bool {
    let acknowledged = body
        .get("highImpactAcknowledged")
        .or_else(|| body.get("high_impact_acknowledged"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let approval_mode = body
        .get("approvalMode")
        .or_else(|| body.get("approval_mode"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let has_evidence = body
        .get("evidenceRefs")
        .or_else(|| body.get("evidence_refs"))
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    acknowledged
        && matches!(
            approval_mode,
            "explicit_user_review" | "approved_with_human_confirmation" | "human"
        )
        && has_evidence
}

fn patch_runtime_boundary_violations(value: &Value) -> Vec<String> {
    let mut violations = spec::strategy_spec_runtime_boundary_violations(value);
    collect_patch_path_violations(value, "$", &mut violations);
    violations.sort();
    violations.dedup();
    violations
}

fn collect_patch_path_violations(value: &Value, path: &str, violations: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if is_sensitive_or_runtime_key(key) {
                    violations.push(format!(
                        "{child_path} belongs to runtime configuration or secret material, not StrategySpec"
                    ));
                }
                if key == "path" {
                    if let Some(pointer) = child.as_str() {
                        for segment in pointer
                            .trim_start_matches('/')
                            .split('/')
                            .filter(|segment| !segment.is_empty())
                        {
                            if is_sensitive_or_runtime_key(segment) {
                                violations.push(format!(
                                    "{child_path} targets runtime configuration or secret material"
                                ));
                            }
                        }
                    }
                }
                collect_patch_path_violations(child, &child_path, violations);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_patch_path_violations(item, &format!("{path}[{index}]"), violations);
            }
        }
        _ => {}
    }
}

fn redact_sensitive_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, child) in map {
                if is_sensitive_or_runtime_key(key) {
                    redacted.insert(key.clone(), json!("[redacted]"));
                } else {
                    redacted.insert(key.clone(), redact_sensitive_json(child));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_sensitive_json).collect()),
        _ => value.clone(),
    }
}

fn is_sensitive_or_runtime_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "credential"
            | "credentials"
            | "credentialref"
            | "credentialhandle"
            | "secret"
            | "apisecret"
            | "apikey"
            | "token"
            | "password"
            | "providerref"
            | "pluginref"
            | "accountref"
            | "brokeraccount"
            | "brokeraccountid"
            | "accountbinding"
            | "accountbindingid"
            | "plugininstance"
            | "plugininstanceid"
            | "activation"
            | "activationid"
            | "acknowledgement"
            | "acknowledgementid"
            | "acknowledgementids"
            | "liveauthority"
    )
}

fn affected_paths(patch: &Value) -> Vec<String> {
    if let Some(items) = patch.as_array() {
        let paths = items
            .iter()
            .filter_map(|item| item.get("path").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            return paths;
        }
    }
    let mut paths = Vec::new();
    collect_paths(patch, "$", &mut paths);
    paths
}

fn collect_paths(value: &Value, path: &str, paths: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let next = format!("{path}.{key}");
                paths.push(next.clone());
                collect_paths(child, &next, paths);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_paths(child, &format!("{path}[{index}]"), paths);
            }
        }
        _ => {}
    }
}

fn risk_level_for_paths(paths: &[String]) -> &'static str {
    if paths.iter().any(|path| {
        [
            "sizing",
            "risk",
            "guardrail",
            "order",
            "price_policy",
            "exit",
            "live",
            "instrument_family",
            "market_clock",
            "fallback",
        ]
        .iter()
        .any(|needle| path.contains(needle))
    }) {
        "high"
    } else {
        "low"
    }
}

fn policy_paths_for(paths: &[String]) -> Vec<Value> {
    paths
        .iter()
        .filter(|path| risk_level_for_paths(&[(*path).clone()]) == "high")
        .map(|path| {
            json!({
                "path": path,
                "policy": "strategy_editor.high_impact_review",
                "approvalMode": "explicit_user_review",
            })
        })
        .collect()
}

fn merge_patch(mut target: Value, patch: Value) -> Value {
    if let Some(ops) = patch.as_array() {
        for op in ops {
            if op["op"].as_str() == Some("replace") || op["op"].as_str() == Some("add") {
                if let Some(path) = op["path"].as_str() {
                    set_json_path(&mut target, path, op["value"].clone());
                }
            }
        }
        return target;
    }
    merge_object(&mut target, patch);
    target
}

fn merge_object(target: &mut Value, patch: Value) {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                merge_object(target_map.entry(key).or_insert(Value::Null), value);
            }
        }
        (target_value, patch_value) => *target_value = patch_value,
    }
}

fn set_json_path(target: &mut Value, path: &str, value: Value) {
    let parts = path
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        *target = value;
        return;
    }
    let mut cursor = target;
    for part in &parts[..parts.len() - 1] {
        if !cursor.is_object() {
            *cursor = json!({});
        }
        cursor = cursor
            .as_object_mut()
            .expect("object")
            .entry((*part).to_string())
            .or_insert_with(|| json!({}));
    }
    if !cursor.is_object() {
        *cursor = json!({});
    }
    cursor
        .as_object_mut()
        .expect("object")
        .insert(parts[parts.len() - 1].to_string(), value);
}

fn payload_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|_| value.to_string().into_bytes());
    spec::raw_hash(&bytes)
}

fn short_hash(hash: &str) -> String {
    hash.chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .take(12)
        .collect::<String>()
}

fn version_id(strategy_id: &str, version: u64) -> String {
    format!("version_{}_v{version}", slug(strategy_id))
}

fn default_strategies() -> Vec<Value> {
    vec![
        default_strategy("strat_local_btc_demo"),
        strategy_record(
            "strat_local_crypto_replay",
            "Crypto Replay Draft",
            "ETH/USD",
            "local-data",
            "draft",
            1,
            spec::strategy_template_spec_payload("blank"),
        ),
    ]
}

fn default_strategy(id: &str) -> Value {
    if id == "strat_local_crypto_replay" {
        return strategy_record(
            "strat_local_crypto_replay",
            "Crypto Replay Draft",
            "ETH/USD",
            "local-data",
            "draft",
            1,
            spec::strategy_template_spec_payload("blank"),
        );
    }
    strategy_record(
        id,
        if id == "strat_local_btc_demo" {
            "BTC fast exit demo"
        } else {
            "Local Strategy"
        },
        "BTC/USD",
        "sim",
        "published",
        1,
        spec::btc_exit_demo_spec_payload(),
    )
}

fn template_id(body: &Value) -> &str {
    body.get("templateId")
        .or_else(|| body.get("template_id"))
        .and_then(Value::as_str)
        .unwrap_or("blank")
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if slug.is_empty() {
        "strategy".to_string()
    } else {
        slug
    }
}
