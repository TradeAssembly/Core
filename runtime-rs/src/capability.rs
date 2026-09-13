// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    ApfOperationMetadata, CapabilityBinding, CapabilityBlocker, CapabilityCandidate,
    CapabilityGraphEdge, CapabilityGraphNode, CapabilityGraphRequest, CapabilityGraphResolution,
    CapabilityReadiness, CapabilityRequirement, CapabilityRequirementConstraints,
    CapabilityResolution, CapabilityResolverCatalog, CapabilityResolverPort, CapabilitySelection,
    EntitlementDecision, EntitlementGrant, EntitlementProfile, PluginOperationContract,
    PluginOperationDependency, PluginOperationTraits,
};
use crate::ports::{PortDescriptor, PortKind, VersionedPort};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CAPABILITY_RESOLUTION_SCHEMA_VERSION: &str = "tradeassembly.capability_resolution.v1";
pub const CAPABILITY_GRAPH_SCHEMA_VERSION: &str = "tradeassembly.capability_graph.v1";
pub const LOCAL_FULL_PROFILE: &str = "local_full";

/// Local metadata discovery does not require brokerage onboarding. This is not
/// an exception for general reads, a disabled plugin, or degraded host health.
pub(crate) fn setup_discovery_allowed(operation: &PluginOperationContract, health: &str) -> bool {
    matches!(health, "needs_configuration" | "needs_credentials")
        && operation.id == "plugin.capability_matrix"
        && operation.capability == "plugin.capability_matrix"
        && operation.effect == "read"
        && operation.apf_action_id == "plugin.describe"
        && !operation.credential_grant_required
        && !operation.account_binding_required
}

#[derive(Clone, Debug, Default)]
pub struct LocalCapabilityResolver;

impl VersionedPort for LocalCapabilityResolver {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::Plugins, "core.capability-resolver")
                .for_profiles(&["local", "self_hosted", "serverless"])
                .with_capabilities(&["capability.resolve", "entitlement.resolve"]),
        ]
    }
}

impl CapabilityResolverPort for LocalCapabilityResolver {
    fn resolve(
        &self,
        requirement: CapabilityRequirement,
        catalog: CapabilityResolverCatalog,
    ) -> CapabilityResolution {
        resolve(requirement, catalog)
    }

    fn resolve_graph(
        &self,
        request: CapabilityGraphRequest,
        catalog: CapabilityResolverCatalog,
    ) -> CapabilityGraphResolution {
        resolve_graph(request, catalog)
    }
}

pub fn resolve(
    requirement: CapabilityRequirement,
    catalog: CapabilityResolverCatalog,
) -> CapabilityResolution {
    resolve_requirement(requirement, catalog, false, None)
}

fn resolve_graph_requirement(
    requirement: CapabilityRequirement,
    catalog: CapabilityResolverCatalog,
    evaluation_epoch: &str,
) -> CapabilityResolution {
    resolve_requirement(requirement, catalog, true, Some(evaluation_epoch))
}

fn resolve_requirement(
    requirement: CapabilityRequirement,
    catalog: CapabilityResolverCatalog,
    graph_contract: bool,
    evaluation_epoch: Option<&str>,
) -> CapabilityResolution {
    let mut candidates = Vec::new();
    let entitlement_decision =
        entitlement_decision(&requirement, &catalog.entitlements, evaluation_epoch);
    let mut top_blockers = Vec::new();
    if entitlement_decision.decision == "deny" {
        top_blockers.push(blocker(
            entitlement_blocker_code(&entitlement_decision, graph_contract),
            entitlement_blocker_message(&entitlement_decision),
            Some(requirement.capability.clone()),
        ));
    }

    for plugin in &catalog.plugins {
        if let Some(preferred_instance) = requirement.plugin_instance_ref.as_deref() {
            if plugin.instance_ref != preferred_instance
                && plugin.provider_ref != preferred_instance
            {
                continue;
            }
        }
        if let Some(preferred_plugin) = requirement.plugin_ref.as_deref() {
            if plugin.plugin_ref != preferred_plugin && plugin.provider_ref != preferred_plugin {
                continue;
            }
        }
        for operation in plugin.operations.iter().filter(|operation| {
            operation_matches_requirement(operation, &requirement, graph_contract)
        }) {
            let mut blockers = Vec::new();
            if !plugin.enabled {
                blockers.push(blocker(
                    "disabled_plugin",
                    "Plugin is disabled.",
                    Some(plugin.plugin_ref.clone()),
                ));
            }
            if graph_contract
                && !matches!(plugin.health.as_str(), "ok" | "ready" | "healthy")
                && !setup_discovery_allowed(operation, &plugin.health)
            {
                blockers.push(blocker(
                    "plugin_unhealthy",
                    "Plugin health does not permit capability use.",
                    Some(plugin.plugin_ref.clone()),
                ));
            }
            if !mode_matches(&requirement.mode, operation) {
                blockers.push(blocker(
                    "mode_incompatible",
                    "Plugin operation does not support the requested mode.",
                    Some(plugin.plugin_ref.clone()),
                ));
            }
            if !asset_class_matches(requirement.asset_class.as_deref(), operation) {
                blockers.push(blocker(
                    "instrument_unsupported",
                    "Plugin operation does not support the requested instrument class.",
                    Some(plugin.plugin_ref.clone()),
                ));
            }
            if graph_contract {
                blockers.extend(constraint_blockers(&requirement, operation));
            }
            if operation.credential_grant_required && !plugin.credential_status.configured {
                blockers.push(blocker(
                    "credential_missing",
                    "Plugin operation requires a configured credential handle.",
                    Some(plugin.plugin_ref.clone()),
                ));
            }
            if let Some(account_blocker) =
                account_binding_blocker(&requirement, plugin, operation, graph_contract)
            {
                blockers.push(account_blocker);
            }
            if entitlement_decision.decision == "deny" {
                blockers.push(blocker(
                    entitlement_blocker_code(&entitlement_decision, graph_contract),
                    entitlement_blocker_message(&entitlement_decision),
                    Some(plugin.plugin_ref.clone()),
                ));
            }
            if graph_contract {
                if let Some(limit_blocker) =
                    entitlement_limit_blocker(&requirement, &entitlement_decision)
                {
                    blockers.push(limit_blocker);
                }
                if !authority_metadata_complete(operation) {
                    blockers.push(blocker(
                        "authority_metadata_missing",
                        "Plugin operation is missing required authority metadata.",
                        Some(plugin.plugin_ref.clone()),
                    ));
                }
            }
            candidates.push(CapabilityCandidate {
                plugin_instance_ref: plugin.instance_ref.clone(),
                plugin_ref: plugin.plugin_ref.clone(),
                provider_ref: plugin.provider_ref.clone(),
                name: plugin.name.clone(),
                manifest_fingerprint: plugin.manifest_fingerprint.clone(),
                enabled: plugin.enabled,
                health: plugin.health.clone(),
                capability: requirement.capability.clone(),
                operation: operation.clone(),
                apf: apf_metadata(operation),
                credential_status: plugin.credential_status.clone(),
                entitlement_decision: entitlement_decision.clone(),
                blockers,
                warnings: Vec::new(),
                resolution_fingerprint: fingerprint(&[
                    &requirement.capability,
                    &requirement.mode,
                    &plugin.plugin_ref,
                    &operation.id,
                    &operation.apf_action_id,
                ]),
            });
        }
    }

    if candidates.is_empty() {
        let missing_code = if requirement.operation_id.is_some()
            && catalog_has_capability_with_different_operation(&requirement, &catalog)
        {
            if graph_contract {
                "missing_operation"
            } else {
                "operation_unsupported"
            }
        } else if catalog_has_capability_mode_mismatch(&requirement, &catalog) {
            "mode_incompatible"
        } else {
            "missing_plugin"
        };
        top_blockers.push(blocker(
            missing_code,
            match missing_code {
                "operation_unsupported" | "missing_operation" => {
                    "Plugins exist for this capability, but none declare the requested operation."
                }
                "mode_incompatible" => {
                    "Plugins exist for this capability, but none support the requested mode."
                }
                _ => "No installed plugin declares this capability.",
            },
            Some(requirement.capability.clone()),
        ));
    }

    let has_usable_candidate = candidates
        .iter()
        .any(|candidate| candidate.blockers.is_empty());
    if !has_usable_candidate {
        for blocker in candidates
            .iter()
            .flat_map(|candidate| candidate.blockers.iter().cloned())
        {
            if !top_blockers
                .iter()
                .any(|existing| existing.code == blocker.code)
            {
                top_blockers.push(blocker);
            }
        }
    }

    CapabilityResolution {
        schema_version: CAPABILITY_RESOLUTION_SCHEMA_VERSION.to_string(),
        ok: has_usable_candidate,
        state: if has_usable_candidate {
            "candidates".to_string()
        } else {
            "blocked".to_string()
        },
        requirement,
        profile: EntitlementProfile {
            id: LOCAL_FULL_PROFILE.to_string(),
            source: "local".to_string(),
            default: true,
        },
        candidates,
        blockers: top_blockers,
        entitlement_decisions: vec![entitlement_decision],
        no_silent_fallback: true,
        no_advice: true,
    }
}

pub fn resolve_graph(
    request: CapabilityGraphRequest,
    mut catalog: CapabilityResolverCatalog,
) -> CapabilityGraphResolution {
    catalog.plugins.sort_by(|left, right| {
        (
            &left.instance_ref,
            &left.plugin_ref,
            &left.provider_ref,
            &left.name,
        )
            .cmp(&(
                &right.instance_ref,
                &right.plugin_ref,
                &right.provider_ref,
                &right.name,
            ))
    });
    catalog
        .entitlements
        .sort_by(|left, right| left.grant_id.cmp(&right.grant_id));

    let mut requirements = BTreeMap::new();
    let mut global_blockers = Vec::new();
    for requirement in &request.requirements {
        if requirement.requirement_id.trim().is_empty()
            || requirements
                .insert(requirement.requirement_id.clone(), requirement.clone())
                .is_some()
        {
            push_unique_blocker(
                &mut global_blockers,
                blocker(
                    "duplicate_requirement",
                    "Capability requirement ids must be non-empty and unique.",
                    Some(requirement.requirement_id.clone()),
                ),
            );
        }
    }

    let mut requirement_blockers = validate_requirement_dependencies(&requirements);
    let required = propagate_required_requirements(&mut requirements);
    let mut builder = GraphBuilder {
        request: &request,
        catalog: &catalog,
        nodes: BTreeMap::new(),
        edges: Vec::new(),
        requirement_blockers: std::mem::take(&mut requirement_blockers),
    };

    for requirement in requirements.values() {
        builder.resolve_node(
            requirement.clone(),
            "strategy_requirement",
            None,
            None,
            required.contains(&requirement.requirement_id),
            &[],
        );
    }
    for requirement in requirements.values() {
        for dependency_ref in &requirement.dependency_refs {
            if requirements.contains_key(dependency_ref) {
                builder.edges.push(CapabilityGraphEdge {
                    from_node_id: requirement.requirement_id.clone(),
                    to_node_id: dependency_ref.clone(),
                    dependency_id: dependency_ref.clone(),
                    origin: "strategy_dependency".to_string(),
                    required: required.contains(&requirement.requirement_id),
                });
            }
        }
    }

    let mut nodes = builder.nodes.into_values().collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    let mut edges = builder.edges;
    edges.sort_by(|left, right| {
        (&left.from_node_id, &left.to_node_id, &left.dependency_id).cmp(&(
            &right.from_node_id,
            &right.to_node_id,
            &right.dependency_id,
        ))
    });
    edges.dedup_by(|left, right| {
        left.from_node_id == right.from_node_id
            && left.to_node_id == right.to_node_id
            && left.dependency_id == right.dependency_id
            && left.origin == right.origin
    });

    for node in &nodes {
        if (node.required || node.selected.is_some())
            && (node.selected.is_none() || !node.blockers.is_empty())
        {
            for item in &node.blockers {
                push_unique_blocker(&mut global_blockers, item.clone());
            }
        }
    }
    sort_blockers(&mut global_blockers);
    let readiness = readiness(&nodes, &global_blockers);
    let ok = global_blockers.is_empty()
        && nodes
            .iter()
            .filter(|node| node.required)
            .all(|node| node.selected.is_some() && node.blockers.is_empty());
    let evaluation_epoch = if request.evaluation_epoch.trim().is_empty() {
        "unspecified".to_string()
    } else {
        request.evaluation_epoch.trim().to_string()
    };
    let graph_fingerprint = graph_fingerprint(&evaluation_epoch, &nodes, &edges, &readiness);

    CapabilityGraphResolution {
        schema_version: CAPABILITY_GRAPH_SCHEMA_VERSION.to_string(),
        ok,
        state: if ok { "selected" } else { "blocked" }.to_string(),
        evaluation_epoch,
        nodes,
        edges,
        blockers: global_blockers,
        readiness,
        graph_fingerprint,
        no_silent_fallback: true,
        no_advice: true,
    }
}

struct GraphBuilder<'a> {
    request: &'a CapabilityGraphRequest,
    catalog: &'a CapabilityResolverCatalog,
    nodes: BTreeMap<String, CapabilityGraphNode>,
    edges: Vec<CapabilityGraphEdge>,
    requirement_blockers: BTreeMap<String, Vec<CapabilityBlocker>>,
}

impl GraphBuilder<'_> {
    #[allow(clippy::too_many_arguments)]
    fn resolve_node(
        &mut self,
        requirement: CapabilityRequirement,
        origin: &str,
        parent_node_id: Option<String>,
        dependency_id: Option<String>,
        required: bool,
        operation_stack: &[String],
    ) {
        let node_id = requirement.requirement_id.clone();
        if self.nodes.contains_key(&node_id) {
            return;
        }
        let (effective_requirement, mut candidates, selected, mut blockers) =
            select_node(&node_id, &requirement, self.request, self.catalog);
        if let Some(existing) = self.requirement_blockers.remove(&node_id) {
            blockers.extend(existing);
        }
        let selected_key = selected.as_ref().map(|selection| {
            format!(
                "{}|{}",
                selection.operation.capability, selection.operation_id
            )
        });
        let dependency_cycle = selected_key
            .as_ref()
            .is_some_and(|key| operation_stack.contains(key));
        let selected = if dependency_cycle {
            blockers.push(blocker(
                "dependency_cycle",
                "Plugin operation dependencies contain a cycle.",
                Some(node_id.clone()),
            ));
            None
        } else {
            selected
        };
        sort_candidates(&mut candidates);
        sort_blockers(&mut blockers);
        self.nodes.insert(
            node_id.clone(),
            CapabilityGraphNode {
                node_id: node_id.clone(),
                origin: origin.to_string(),
                parent_node_id,
                dependency_id,
                required,
                requirement: effective_requirement.clone(),
                candidates,
                selected: selected.clone(),
                blockers,
            },
        );

        let Some(selection) = selected else {
            return;
        };
        let mut next_stack = operation_stack.to_vec();
        next_stack.push(selected_key.expect("selected operation key"));
        let mut dependencies = selection.operation.dependencies.clone();
        dependencies.sort_by(|left, right| left.dependency_id.cmp(&right.dependency_id));
        for dependency in dependencies {
            let child_id = format!("{node_id}::{}", dependency.dependency_id);
            let child_required = dependency.required;
            self.edges.push(CapabilityGraphEdge {
                from_node_id: node_id.clone(),
                to_node_id: child_id.clone(),
                dependency_id: dependency.dependency_id.clone(),
                origin: "plugin_dependency".to_string(),
                required: child_required,
            });
            self.resolve_node(
                dependency_requirement(
                    &child_id,
                    &effective_requirement,
                    &selection.operation,
                    &dependency,
                ),
                "plugin_dependency",
                Some(node_id.clone()),
                Some(dependency.dependency_id),
                child_required,
                &next_stack,
            );
        }
    }
}

fn select_node(
    node_id: &str,
    requirement: &CapabilityRequirement,
    request: &CapabilityGraphRequest,
    catalog: &CapabilityResolverCatalog,
) -> (
    CapabilityRequirement,
    Vec<CapabilityCandidate>,
    Option<CapabilitySelection>,
    Vec<CapabilityBlocker>,
) {
    let base_resolution = resolve_graph_requirement(
        requirement.clone(),
        catalog.clone(),
        &request.evaluation_epoch,
    );
    let mut candidates = base_resolution.candidates.clone();
    sort_candidates(&mut candidates);
    let direct_binding = request
        .bindings
        .get(node_id)
        .cloned()
        .or_else(|| embedded_binding(requirement));
    let policy = normalized_fallback_policy(&requirement.fallback_policy);

    if let Some(binding) = direct_binding.as_ref() {
        let bound_requirement = apply_binding(requirement, binding);
        let resolution = resolve_graph_requirement(
            bound_requirement.clone(),
            catalog.clone(),
            &request.evaluation_epoch,
        );
        if let Some(selected) = unique_usable_candidate(&resolution.candidates) {
            let selected = selection(selected, &bound_requirement, "explicit_binding");
            return (bound_requirement, candidates, Some(selected), Vec::new());
        }
        if policy != "configured" {
            let mut blockers = resolution.blockers;
            push_unique_blocker(
                &mut blockers,
                blocker(
                    if usable_candidates(&resolution.candidates).len() > 1 {
                        "ambiguous_binding"
                    } else {
                        "explicit_binding_invalid"
                    },
                    "Explicit capability binding did not resolve to one usable operation.",
                    Some(node_id.to_string()),
                ),
            );
            return (bound_requirement, candidates, None, blockers);
        }
    }

    match policy {
        "explicit_only" => (
            requirement.clone(),
            candidates,
            None,
            vec![blocker(
                "fallback_not_permitted",
                "Capability requirement requires an explicit binding.",
                Some(node_id.to_string()),
            )],
        ),
        "configured" => {
            let fallbacks = request
                .configured_fallbacks
                .get(node_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for (index, binding) in fallbacks.iter().enumerate() {
                let bound_requirement = apply_binding(requirement, binding);
                let resolution = resolve_graph_requirement(
                    bound_requirement.clone(),
                    catalog.clone(),
                    &request.evaluation_epoch,
                );
                if let Some(selected) = unique_usable_candidate(&resolution.candidates) {
                    return (
                        bound_requirement,
                        candidates,
                        Some(selection(
                            selected,
                            &apply_binding(requirement, binding),
                            &format!("configured_fallback:{index}"),
                        )),
                        Vec::new(),
                    );
                }
            }
            (
                requirement.clone(),
                candidates,
                None,
                vec![blocker(
                    "fallback_not_permitted",
                    "No configured fallback resolved to one usable operation.",
                    Some(node_id.to_string()),
                )],
            )
        }
        "none" => {
            let usable = usable_candidates(&candidates);
            if usable.len() == 1 {
                (
                    requirement.clone(),
                    candidates.clone(),
                    Some(selection(
                        usable[0],
                        requirement,
                        "automatic_single_candidate",
                    )),
                    Vec::new(),
                )
            } else if usable.len() > 1 {
                (
                    requirement.clone(),
                    candidates,
                    None,
                    vec![blocker(
                        "ambiguous_binding",
                        "Multiple usable plugin operations require an explicit binding.",
                        Some(node_id.to_string()),
                    )],
                )
            } else {
                (
                    requirement.clone(),
                    candidates,
                    None,
                    base_resolution.blockers,
                )
            }
        }
        _ => (
            requirement.clone(),
            candidates,
            None,
            vec![blocker(
                "fallback_not_permitted",
                "Capability requirement declares an unsupported fallback policy.",
                Some(node_id.to_string()),
            )],
        ),
    }
}

fn embedded_binding(requirement: &CapabilityRequirement) -> Option<CapabilityBinding> {
    if requirement.plugin_instance_ref.is_none()
        && requirement.plugin_ref.is_none()
        && requirement.account_ref.is_none()
    {
        return None;
    }
    Some(CapabilityBinding {
        plugin_instance_ref: requirement.plugin_instance_ref.clone(),
        plugin_ref: requirement.plugin_ref.clone(),
        operation_id: requirement.operation_id.clone(),
        account_ref: requirement.account_ref.clone(),
    })
}

fn apply_binding(
    requirement: &CapabilityRequirement,
    binding: &CapabilityBinding,
) -> CapabilityRequirement {
    let mut result = requirement.clone();
    if binding.plugin_instance_ref.is_some() {
        result.plugin_instance_ref = binding.plugin_instance_ref.clone();
    }
    if binding.plugin_ref.is_some() {
        result.plugin_ref = binding.plugin_ref.clone();
    }
    if binding.operation_id.is_some() {
        result.operation_id = binding.operation_id.clone();
    }
    if binding.account_ref.is_some() {
        result.account_ref = binding.account_ref.clone();
    }
    result
}

fn unique_usable_candidate(candidates: &[CapabilityCandidate]) -> Option<&CapabilityCandidate> {
    let usable = usable_candidates(candidates);
    (usable.len() == 1).then(|| usable[0])
}

fn usable_candidates(candidates: &[CapabilityCandidate]) -> Vec<&CapabilityCandidate> {
    candidates
        .iter()
        .filter(|candidate| candidate.blockers.is_empty())
        .collect()
}

fn selection(
    candidate: &CapabilityCandidate,
    requirement: &CapabilityRequirement,
    reason: &str,
) -> CapabilitySelection {
    CapabilitySelection {
        plugin_instance_ref: candidate.plugin_instance_ref.clone(),
        plugin_ref: candidate.plugin_ref.clone(),
        provider_ref: candidate.provider_ref.clone(),
        operation_id: candidate.operation.id.clone(),
        account_ref: requirement.account_ref.clone(),
        manifest_fingerprint: candidate.manifest_fingerprint.clone(),
        credential_revision_ref: candidate.credential_status.revision_ref.clone(),
        entitlement_decision_ref: candidate.entitlement_decision.grant_id.clone(),
        selection_reason: reason.to_string(),
        operation: candidate.operation.clone(),
    }
}

fn normalized_fallback_policy(value: &str) -> &str {
    match value.trim() {
        "" => "none",
        other => other,
    }
}

fn dependency_requirement(
    child_id: &str,
    parent: &CapabilityRequirement,
    operation: &PluginOperationContract,
    dependency: &PluginOperationDependency,
) -> CapabilityRequirement {
    CapabilityRequirement {
        requirement_id: child_id.to_string(),
        capability: dependency.capability.clone(),
        mode: parent.mode.clone(),
        declaration: if dependency.required {
            "required"
        } else {
            "optional"
        }
        .to_string(),
        required_for: parent.required_for.clone(),
        stage_refs: parent.stage_refs.clone(),
        substep_refs: parent.substep_refs.clone(),
        constraints: constraints_from_traits(&dependency.traits),
        dependency_refs: Vec::new(),
        fallback_policy: "none".to_string(),
        policy_tags: parent.policy_tags.clone(),
        asset_class: parent.asset_class.clone(),
        instrument_family: parent.instrument_family.clone(),
        strategy_id: parent.strategy_id.clone(),
        strategy_version_id: parent.strategy_version_id.clone(),
        plugin_instance_ref: None,
        plugin_ref: None,
        operation_id: dependency.operation_id.clone(),
        account_ref: None,
        purpose: Some(operation.purpose.clone()),
    }
}

fn constraints_from_traits(traits: &PluginOperationTraits) -> CapabilityRequirementConstraints {
    CapabilityRequirementConstraints {
        instrument_families: traits.instrument_families.clone(),
        data_shapes: traits.data_shapes.clone(),
        operations: traits.operations.clone(),
        fields: traits.fields.clone(),
        input_paths: traits.input_paths.clone(),
        schema_refs: traits
            .input_schema_refs
            .iter()
            .chain(traits.output_schema_refs.iter())
            .cloned()
            .collect(),
        timeframe: (traits.timeframes.len() == 1).then(|| traits.timeframes[0].clone()),
        max_freshness: traits.max_freshness.clone(),
        max_latency_ms: traits.max_latency_ms,
        deterministic: traits.deterministic,
        replayable: traits.replayable,
    }
}

pub fn local_full_entitlements() -> Vec<EntitlementGrant> {
    [
        "marketdata.quote",
        "marketdata.bars",
        "market_data.bars.read@1",
        "calendar.session.resolve@1",
        "expression.cel.evaluate@1",
        "indicator.bars.normalize@1",
        "indicator.stateful.calculate@1",
        "order.intent.build@1",
        "order.option.submit@1",
        "order.option_combo.submit@1",
        "marketdata.crypto",
        "marketdata.options_chain",
        "broker.order_submit.paper",
        // Product eligibility is not authority to submit. Live still requires
        // the local mandate, risk checks and broker-side policy enforcement.
        "broker.order_submit.live",
        "broker.paper_order.submit",
        "feature.backtesting.basic",
        "feature.paper_trading",
        "feature.plugin.install",
        "plugin.capability_matrix",
        "account.health",
        "indicator.calculate",
    ]
    .into_iter()
    .map(|capability| EntitlementGrant {
        grant_id: format!("grant:local_full:{capability}"),
        profile: LOCAL_FULL_PROFILE.to_string(),
        source: "local_default".to_string(),
        capability: capability.to_string(),
        effect: "allow".to_string(),
        state: "active".to_string(),
        scope: "global".to_string(),
        source_type: "local_default".to_string(),
        source_ref: Some("tradeassembly.public_core".to_string()),
        precedence: 0,
        limits: [
            (
                "modes".to_string(),
                json!([
                    "authoring",
                    "research",
                    "backtest",
                    "simulation",
                    "paper",
                    "live"
                ]),
            ),
            ("notional".to_string(), json!("unbounded_local_default")),
        ]
        .into_iter()
        .collect(),
        issued_at: Some("2026-07-08T00:00:00Z".to_string()),
        expires_at: None,
        revoked_at: None,
        reason: Some("local public core default grant".to_string()),
    })
    .collect()
}

pub fn blocker(code: &str, message: &str, subject: Option<String>) -> CapabilityBlocker {
    CapabilityBlocker {
        code: code.to_string(),
        message: message.to_string(),
        subject,
    }
}

fn entitlement_decision(
    requirement: &CapabilityRequirement,
    grants: &[EntitlementGrant],
    evaluation_epoch: Option<&str>,
) -> EntitlementDecision {
    let mut matching = grants
        .iter()
        .filter(|grant| grant.capability == requirement.capability)
        .filter(|grant| grant.revoked_at.is_none())
        .filter(|grant| !grant_is_expired(grant, evaluation_epoch))
        .collect::<Vec<_>>();
    matching.sort_by_key(|grant| std::cmp::Reverse(grant.precedence));

    if let Some(deny) = matching
        .iter()
        .copied()
        .find(|grant| grant.effect == "deny" && matches!(grant.state.as_str(), "active" | "denied"))
    {
        return EntitlementDecision {
            profile: deny.profile.clone(),
            source: deny.source.clone(),
            capability: requirement.capability.clone(),
            decision: "deny".to_string(),
            precedence: deny.precedence,
            limits: deny.limits.clone(),
            grant_id: Some(deny.grant_id.clone()),
            expires_at: deny.expires_at.clone(),
            reason: deny.reason.clone(),
        };
    }
    if let Some(allow) = matching
        .iter()
        .copied()
        .find(|grant| grant.effect == "allow" && grant.state == "active")
    {
        return EntitlementDecision {
            profile: allow.profile.clone(),
            source: allow.source.clone(),
            capability: requirement.capability.clone(),
            decision: "allow".to_string(),
            precedence: allow.precedence,
            limits: allow.limits.clone(),
            grant_id: Some(allow.grant_id.clone()),
            expires_at: allow.expires_at.clone(),
            reason: allow.reason.clone(),
        };
    }
    EntitlementDecision {
        profile: LOCAL_FULL_PROFILE.to_string(),
        source: "local_default_missing_grant".to_string(),
        capability: requirement.capability.clone(),
        decision: "deny".to_string(),
        precedence: 0,
        limits: Default::default(),
        grant_id: None,
        expires_at: None,
        reason: Some("no active entitlement grant matched this capability".to_string()),
    }
}

fn grant_is_expired(grant: &EntitlementGrant, evaluation_epoch: Option<&str>) -> bool {
    if matches!(grant.state.as_str(), "expired" | "revoked") {
        return true;
    }
    let Some(expires_at) = grant.expires_at.as_deref() else {
        return false;
    };
    let Some(expires_at) = normalize_rfc3339_second(expires_at) else {
        return evaluation_epoch.is_some();
    };
    let evaluated_at = match evaluation_epoch {
        Some(epoch) => normalize_rfc3339_second(epoch),
        None => now_rfc3339_second(),
    };
    evaluated_at.is_none_or(|evaluated_at| expires_at <= evaluated_at)
}

fn entitlement_blocker_code(decision: &EntitlementDecision, graph_contract: bool) -> &'static str {
    if graph_contract || decision.grant_id.is_some() {
        "entitlement_denied"
    } else {
        "no_entitlement_grant"
    }
}

fn entitlement_blocker_message(decision: &EntitlementDecision) -> &'static str {
    if decision.grant_id.is_some() {
        "Capability is denied by an explicit entitlement grant."
    } else {
        "No active entitlement grant matched this capability."
    }
}

fn apf_metadata(operation: &PluginOperationContract) -> ApfOperationMetadata {
    ApfOperationMetadata {
        action_id: operation.apf_action_id.clone(),
        resource_type: operation.finance_resource_type.clone(),
        operation_resource_type: operation.resource_type.clone(),
        mandate_required: operation.mandate_required,
        purpose: operation.purpose.clone(),
        evidence: operation.evidence.clone(),
        pep_coverage_class: operation.pep_coverage_class.clone(),
        check_packs: operation.check_packs.clone(),
        receipt_class: operation.receipt_class.clone(),
        credential_grant_required: operation.credential_grant_required,
        account_binding_required: operation.account_binding_required,
        session_admission: operation.session_admission.clone(),
        approval_mode: operation.approval_mode.clone(),
        supervision_mode: operation.supervision_mode.clone(),
        detectors: operation.detectors.clone(),
        context_providers: operation.context_providers.clone(),
        policy_refs: operation.policy_refs.clone(),
    }
}

fn mode_matches(mode: &str, operation: &PluginOperationContract) -> bool {
    let normalized = mode.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    if !operation.traits.modes.is_empty() {
        return operation
            .traits
            .modes
            .iter()
            .any(|candidate| candidate == &normalized);
    }
    let declares_mode = operation.supported_protocols.iter().any(|protocol| {
        protocol.starts_with("mode:") || matches!(protocol.as_str(), "broker.paper" | "broker.live")
    });
    if !declares_mode {
        return true;
    }
    operation.supported_protocols.iter().any(|protocol| {
        protocol == &format!("mode:{normalized}")
            || (normalized == "paper" && protocol == "broker.paper")
            || (normalized == "live" && protocol == "broker.live")
    })
}

fn account_binding_blocker(
    requirement: &CapabilityRequirement,
    plugin: &crate::ports::PluginCatalogEntry,
    operation: &PluginOperationContract,
    graph_contract: bool,
) -> Option<CapabilityBlocker> {
    if !operation.account_binding_required {
        return None;
    }
    let account_ref = requirement
        .account_ref
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if account_ref.is_empty() {
        return Some(blocker(
            if graph_contract {
                "account_binding_required"
            } else {
                "account_binding_missing"
            },
            "Plugin operation requires an explicit account binding.",
            Some(plugin.plugin_ref.clone()),
        ));
    }
    let mode = requirement.mode.trim().to_ascii_lowercase();
    let mode = if mode.is_empty() {
        "paper"
    } else {
        mode.as_str()
    };
    let allowed_refs = [
        format!("account://{}/{mode}", plugin.provider_ref),
        format!("account://{}/{mode}", plugin.plugin_ref),
    ];
    let registered = plugin.account_ref.as_deref() == Some(account_ref);
    if !plugin.enabled
        || !plugin.credential_status.configured
        || !(registered || allowed_refs.iter().any(|allowed| allowed == account_ref))
    {
        return Some(blocker(
            if graph_contract {
                "account_binding_mismatch"
            } else {
                "account_binding_invalid"
            },
            "Account binding is not registered for this plugin, mode, and credential handle.",
            Some(plugin.plugin_ref.clone()),
        ));
    }
    None
}

fn operation_matches_requirement(
    operation: &PluginOperationContract,
    requirement: &CapabilityRequirement,
    graph_contract: bool,
) -> bool {
    if !capability_matches(&requirement.capability, &operation.capability) {
        return false;
    }
    let Some(requested) = requirement.operation_id.as_deref() else {
        return true;
    };
    let requested = requested.trim();
    requested.is_empty()
        || operation.id == requested
        || !graph_contract
            && (operation.apf_action_id == requested || operation.effect == requested)
}

fn asset_class_matches(asset_class: Option<&str>, operation: &PluginOperationContract) -> bool {
    let Some(asset_class) = asset_class else {
        return true;
    };
    let normalized = asset_class.trim().to_ascii_lowercase();
    if !operation.traits.instrument_families.is_empty() {
        return operation
            .traits
            .instrument_families
            .iter()
            .any(|family| instrument_family_matches(&normalized, family));
    }
    operation.supported_protocols.iter().any(|protocol| {
        protocol == &format!("asset:{normalized}")
            || (normalized == "crypto" && protocol == "asset:crypto")
            || (normalized == "equity" && protocol == "asset:equity")
            || (normalized == "option" && protocol == "asset:option")
    }) || !operation
        .supported_protocols
        .iter()
        .any(|protocol| protocol.starts_with("asset:"))
}

fn catalog_has_capability_with_different_operation(
    requirement: &CapabilityRequirement,
    catalog: &CapabilityResolverCatalog,
) -> bool {
    catalog.plugins.iter().any(|plugin| {
        if let Some(preferred_instance) = requirement.plugin_instance_ref.as_deref() {
            if plugin.instance_ref != preferred_instance
                && plugin.provider_ref != preferred_instance
            {
                return false;
            }
        }
        if let Some(preferred_plugin) = requirement.plugin_ref.as_deref() {
            if plugin.plugin_ref != preferred_plugin && plugin.provider_ref != preferred_plugin {
                return false;
            }
        }
        plugin
            .operations
            .iter()
            .any(|operation| capability_matches(&requirement.capability, &operation.capability))
    })
}

fn catalog_has_capability_mode_mismatch(
    requirement: &CapabilityRequirement,
    catalog: &CapabilityResolverCatalog,
) -> bool {
    catalog.plugins.iter().any(|plugin| {
        if let Some(preferred_instance) = requirement.plugin_instance_ref.as_deref() {
            if plugin.instance_ref != preferred_instance
                && plugin.provider_ref != preferred_instance
            {
                return false;
            }
        }
        if let Some(preferred_plugin) = requirement.plugin_ref.as_deref() {
            if plugin.plugin_ref != preferred_plugin && plugin.provider_ref != preferred_plugin {
                return false;
            }
        }
        plugin
            .operations
            .iter()
            .any(|operation| capability_matches(&requirement.capability, &operation.capability))
    })
}

fn capability_matches(required: &str, offered: &str) -> bool {
    required.trim() == offered.trim()
}

fn instrument_family_matches(required: &str, offered: &str) -> bool {
    required == offered
        || (required == "crypto" && offered == "crypto_spot")
        || (required == "option" && offered == "option_contract")
        || (required == "stock" && offered == "equity")
}

fn constraint_blockers(
    requirement: &CapabilityRequirement,
    operation: &PluginOperationContract,
) -> Vec<CapabilityBlocker> {
    let constraints = &requirement.constraints;
    let mut blockers = Vec::new();
    let subject = Some(operation.id.clone());

    let mut required_families = constraints.instrument_families.clone();
    if let Some(family) = requirement.instrument_family.as_ref() {
        required_families.push(family.clone());
    }
    if !required_families.iter().all(|required| {
        operation
            .traits
            .instrument_families
            .iter()
            .any(|offered| instrument_family_matches(required, offered))
    }) {
        blockers.push(blocker(
            "instrument_unsupported",
            "Plugin operation does not satisfy required instrument families.",
            subject.clone(),
        ));
    }
    if !is_subset(&constraints.data_shapes, &operation.traits.data_shapes)
        || !is_subset(&constraints.fields, &operation.traits.fields)
        || !is_subset(&constraints.input_paths, &operation.traits.input_paths)
        || !constraints.operations.is_empty()
            && !constraints.operations.iter().all(|required| {
                required == &operation.id || operation.traits.operations.contains(required)
            })
    {
        blockers.push(blocker(
            "constraint_mismatch",
            "Plugin operation does not satisfy required data, field, operation, or input-path constraints.",
            subject.clone(),
        ));
    }
    let offered_schemas = operation
        .traits
        .input_schema_refs
        .iter()
        .chain(operation.traits.output_schema_refs.iter())
        .cloned()
        .collect::<Vec<_>>();
    if !is_subset(&constraints.schema_refs, &offered_schemas) {
        blockers.push(blocker(
            "schema_incompatible",
            "Plugin operation does not satisfy required schema references.",
            subject.clone(),
        ));
    }
    if constraints.timeframe.as_ref().is_some_and(|required| {
        !operation
            .traits
            .timeframes
            .iter()
            .any(|offered| offered == "*" || offered == required)
    }) {
        blockers.push(blocker(
            "constraint_mismatch",
            "Plugin operation does not support the required timeframe.",
            subject.clone(),
        ));
    }
    let freshness_unmet = constraints.max_freshness.as_ref().is_some_and(|required| {
        let Some(required) = duration_seconds(required) else {
            return true;
        };
        operation
            .traits
            .max_freshness
            .as_ref()
            .and_then(|offered| duration_seconds(offered))
            .is_none_or(|offered| offered > required)
    });
    let latency_unmet = constraints.max_latency_ms.is_some_and(|required| {
        operation
            .traits
            .max_latency_ms
            .is_none_or(|offered| offered > required)
    });
    if freshness_unmet
        || latency_unmet
        || constraints.deterministic && !operation.traits.deterministic
        || constraints.replayable && !operation.traits.replayable
    {
        blockers.push(blocker(
            "quality_requirement_unmet",
            "Plugin operation does not satisfy required freshness, latency, determinism, or replay quality.",
            subject,
        ));
    }
    blockers
}

fn is_subset(required: &[String], offered: &[String]) -> bool {
    required
        .iter()
        .all(|required| offered.iter().any(|offered| offered == required))
}

fn duration_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Some(days) = value
        .strip_prefix('P')
        .and_then(|days| days.strip_suffix('D'))
    {
        return days.parse::<u64>().ok()?.checked_mul(86_400);
    }
    let time = value.strip_prefix("PT")?;
    let (number, multiplier) = if let Some(hours) = time.strip_suffix('H') {
        (hours, 3_600)
    } else if let Some(minutes) = time.strip_suffix('M') {
        (minutes, 60)
    } else {
        (time.strip_suffix('S')?, 1)
    };
    number.parse::<u64>().ok()?.checked_mul(multiplier)
}

fn entitlement_limit_blocker(
    requirement: &CapabilityRequirement,
    decision: &EntitlementDecision,
) -> Option<CapabilityBlocker> {
    if decision.decision != "allow" {
        return None;
    }
    let allowed_modes = decision
        .limits
        .get("modes")
        .or_else(|| decision.limits.get("allowedModes"))
        .and_then(Value::as_array)
        .map(|modes| modes.iter().filter_map(Value::as_str).collect::<Vec<_>>());
    let single_mode = decision.limits.get("mode").and_then(Value::as_str);
    let mode = requirement.mode.trim();
    let denied = single_mode.is_some_and(|allowed| allowed != mode)
        || allowed_modes.is_some_and(|allowed| !allowed.contains(&mode));
    denied.then(|| {
        blocker(
            "entitlement_limit_exceeded",
            "Entitlement limits do not permit the requested mode.",
            Some(requirement.capability.clone()),
        )
    })
}

fn authority_metadata_complete(operation: &PluginOperationContract) -> bool {
    !operation.apf_action_id.trim().is_empty()
        && !operation.finance_resource_type.trim().is_empty()
        && !operation.resource_type.trim().is_empty()
        && !operation.purpose.trim().is_empty()
        && !operation.receipt_class.trim().is_empty()
        && !operation.redaction.trim().is_empty()
        && operation.no_advice
        && operation
            .detectors
            .iter()
            .any(|detector| detector == "no_advice")
}

fn validate_requirement_dependencies(
    requirements: &BTreeMap<String, CapabilityRequirement>,
) -> BTreeMap<String, Vec<CapabilityBlocker>> {
    let mut result = BTreeMap::<String, Vec<CapabilityBlocker>>::new();
    for requirement in requirements.values() {
        for dependency in &requirement.dependency_refs {
            if !requirements.contains_key(dependency) {
                result
                    .entry(requirement.requirement_id.clone())
                    .or_default()
                    .push(blocker(
                        "dependency_missing",
                        "Strategy capability dependency does not name an existing requirement.",
                        Some(dependency.clone()),
                    ));
            }
        }
    }

    fn visit(
        node_id: &str,
        requirements: &BTreeMap<String, CapabilityRequirement>,
        visiting: &mut Vec<String>,
        visited: &mut BTreeSet<String>,
        result: &mut BTreeMap<String, Vec<CapabilityBlocker>>,
    ) {
        if visited.contains(node_id) {
            return;
        }
        if let Some(index) = visiting.iter().position(|item| item == node_id) {
            for member in &visiting[index..] {
                result.entry(member.clone()).or_default().push(blocker(
                    "dependency_cycle",
                    "Strategy capability dependencies contain a cycle.",
                    Some(member.clone()),
                ));
            }
            return;
        }
        let Some(requirement) = requirements.get(node_id) else {
            return;
        };
        visiting.push(node_id.to_string());
        for dependency in &requirement.dependency_refs {
            if requirements.contains_key(dependency) {
                visit(dependency, requirements, visiting, visited, result);
            }
        }
        visiting.pop();
        visited.insert(node_id.to_string());
    }

    let mut visited = BTreeSet::new();
    for node_id in requirements.keys() {
        visit(
            node_id,
            requirements,
            &mut Vec::new(),
            &mut visited,
            &mut result,
        );
    }
    for blockers in result.values_mut() {
        sort_blockers(blockers);
    }
    result
}

fn propagate_required_requirements(
    requirements: &mut BTreeMap<String, CapabilityRequirement>,
) -> BTreeSet<String> {
    let mut required = requirements
        .values()
        .filter(|requirement| requirement.declaration != "optional")
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<BTreeSet<_>>();
    loop {
        let mut changed = false;
        let parents = required.iter().cloned().collect::<Vec<_>>();
        for parent_id in parents {
            let Some(parent) = requirements.get(&parent_id).cloned() else {
                continue;
            };
            for dependency_id in &parent.dependency_refs {
                if required.insert(dependency_id.clone()) {
                    changed = true;
                }
                if let Some(dependency) = requirements.get_mut(dependency_id) {
                    for level in &parent.required_for {
                        if !dependency.required_for.contains(level) {
                            dependency.required_for.push(level.clone());
                        }
                    }
                    dependency.required_for.sort();
                    dependency.required_for.dedup();
                }
            }
        }
        if !changed {
            return required;
        }
    }
}

fn readiness(
    nodes: &[CapabilityGraphNode],
    global_blockers: &[CapabilityBlocker],
) -> Vec<CapabilityReadiness> {
    const LEVELS: [&str; 6] = [
        "authoring",
        "research",
        "backtest",
        "simulation",
        "paper",
        "live",
    ];
    LEVELS
        .into_iter()
        .map(|level| {
            let mut blocking_node_ids = nodes
                .iter()
                .filter(|node| node.required || node.selected.is_some())
                .filter(|node| {
                    node.requirement.required_for.is_empty()
                        || node
                            .requirement
                            .required_for
                            .iter()
                            .any(|required_for| required_for == level)
                })
                .filter(|node| node.selected.is_none() || !node.blockers.is_empty())
                .map(|node| node.node_id.clone())
                .collect::<Vec<_>>();
            let graph_structure_blocked = global_blockers
                .iter()
                .any(|item| item.code == "duplicate_requirement");
            if graph_structure_blocked && blocking_node_ids.is_empty() {
                blocking_node_ids.push("graph".to_string());
            }
            blocking_node_ids.sort();
            blocking_node_ids.dedup();
            CapabilityReadiness {
                level: level.to_string(),
                state: if blocking_node_ids.is_empty() {
                    "ready"
                } else {
                    "blocked"
                }
                .to_string(),
                blocking_node_ids,
            }
        })
        .collect()
}

fn graph_fingerprint(
    evaluation_epoch: &str,
    nodes: &[CapabilityGraphNode],
    edges: &[CapabilityGraphEdge],
    readiness: &[CapabilityReadiness],
) -> String {
    let node_projection = nodes
        .iter()
        .map(|node| {
            json!({
                "nodeId": node.node_id,
                "origin": node.origin,
                "required": node.required,
                "requirement": node.requirement,
                "selected": node.selected.as_ref().map(|selected| json!({
                    "pluginInstanceRef": selected.plugin_instance_ref,
                    "pluginRef": selected.plugin_ref,
                    "operationId": selected.operation_id,
                    "accountRef": selected.account_ref,
                    "manifestFingerprint": selected.manifest_fingerprint,
                    "entitlementDecisionRef": selected.entitlement_decision_ref,
                    "selectionReason": selected.selection_reason,
                })),
                "blockers": node.blockers,
            })
        })
        .collect::<Vec<_>>();
    let projection = json!({
        "schemaVersion": CAPABILITY_GRAPH_SCHEMA_VERSION,
        "evaluationEpoch": evaluation_epoch,
        "nodes": node_projection,
        "edges": edges,
        "readiness": readiness,
    });
    let bytes = serde_json_canonicalizer::to_vec(&projection)
        .expect("capability graph projection is canonical JSON");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn sort_candidates(candidates: &mut [CapabilityCandidate]) {
    candidates.sort_by(|left, right| {
        (
            &left.plugin_instance_ref,
            &left.plugin_ref,
            &left.operation.id,
        )
            .cmp(&(
                &right.plugin_instance_ref,
                &right.plugin_ref,
                &right.operation.id,
            ))
    });
}

fn sort_blockers(blockers: &mut Vec<CapabilityBlocker>) {
    blockers.sort_by(|left, right| {
        (&left.code, &left.subject, &left.message).cmp(&(
            &right.code,
            &right.subject,
            &right.message,
        ))
    });
    blockers.dedup();
}

fn push_unique_blocker(blockers: &mut Vec<CapabilityBlocker>, item: CapabilityBlocker) {
    if !blockers.contains(&item) {
        blockers.push(item);
    }
}

fn fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}

fn normalize_rfc3339_second(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 20 || value.as_bytes().get(4).is_none_or(|byte| *byte != b'-') {
        return None;
    }
    if !value.ends_with('Z') {
        return None;
    }
    Some(value[..20].to_string())
}

pub(crate) fn now_rfc3339_second() -> Option<String> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{CredentialResolutionStatus, PluginCatalogEntry};

    #[test]
    fn local_live_eligibility_needs_no_hosted_grant_and_preserves_explicit_deny() {
        let requirement = CapabilityRequirement {
            capability: "broker.order_submit.live".into(),
            mode: "live".into(),
            ..Default::default()
        };
        let mut grants = local_full_entitlements();
        let allowed = entitlement_decision(&requirement, &grants, None);
        assert_eq!(allowed.decision, "allow");
        assert_eq!(allowed.source, "local_default");
        let mut deny = grants
            .iter()
            .find(|g| g.capability == requirement.capability)
            .unwrap()
            .clone();
        deny.grant_id = "explicit-local-deny".into();
        deny.effect = "deny".into();
        grants.push(deny);
        let denied = entitlement_decision(&requirement, &grants, None);
        assert_eq!(denied.decision, "deny");
        assert_eq!(denied.grant_id.as_deref(), Some("explicit-local-deny"));
    }

    #[test]
    fn resolver_denies_declared_capability_without_explicit_grant() {
        let resolution = resolve(
            CapabilityRequirement {
                requirement_id: "req-no-grant".to_string(),
                capability: "test.capability".to_string(),
                mode: "paper".to_string(),
                declaration: "required".to_string(),
                asset_class: Some("crypto".to_string()),
                instrument_family: None,
                strategy_id: None,
                plugin_ref: Some("tradeassembly.test".to_string()),
                operation_id: None,
                account_ref: None,
                purpose: None,
                ..Default::default()
            },
            CapabilityResolverCatalog {
                plugins: vec![test_plugin(false, true)],
                entitlements: Vec::new(),
            },
        );

        assert!(!resolution.ok, "{resolution:#?}");
        assert_eq!(resolution.entitlement_decisions[0].decision, "deny");
        assert!(resolution.entitlement_decisions[0].grant_id.is_none());
        assert!(resolution
            .blockers
            .iter()
            .any(|blocker| blocker.code == "no_entitlement_grant"));
    }

    #[test]
    fn resolver_blocks_required_account_binding_until_account_ref_is_present() {
        let catalog = CapabilityResolverCatalog {
            plugins: vec![test_plugin(true, true)],
            entitlements: vec![EntitlementGrant {
                grant_id: "grant:test".to_string(),
                profile: LOCAL_FULL_PROFILE.to_string(),
                source: "test".to_string(),
                capability: "test.capability".to_string(),
                effect: "allow".to_string(),
                state: "active".to_string(),
                scope: "global".to_string(),
                source_type: "test".to_string(),
                source_ref: None,
                precedence: 10,
                limits: Default::default(),
                issued_at: None,
                expires_at: None,
                revoked_at: None,
                reason: None,
            }],
        };

        let missing = resolve(
            CapabilityRequirement {
                requirement_id: "req-account-missing".to_string(),
                capability: "test.capability".to_string(),
                mode: "paper".to_string(),
                declaration: "required".to_string(),
                asset_class: Some("crypto".to_string()),
                instrument_family: None,
                strategy_id: None,
                plugin_ref: Some("tradeassembly.test".to_string()),
                operation_id: None,
                account_ref: None,
                purpose: None,
                ..Default::default()
            },
            catalog.clone(),
        );
        assert!(!missing.ok, "{missing:#?}");
        assert!(missing
            .blockers
            .iter()
            .any(|blocker| blocker.code == "account_binding_missing"));

        let bound = resolve(
            CapabilityRequirement {
                requirement_id: "req-account-bound".to_string(),
                capability: "test.capability".to_string(),
                mode: "paper".to_string(),
                declaration: "required".to_string(),
                asset_class: Some("crypto".to_string()),
                instrument_family: None,
                strategy_id: None,
                plugin_ref: Some("tradeassembly.test".to_string()),
                operation_id: None,
                account_ref: Some("account://test-provider/paper".to_string()),
                purpose: None,
                ..Default::default()
            },
            catalog.clone(),
        );
        assert!(bound.ok, "{bound:#?}");

        let mut registered_catalog = catalog;
        registered_catalog.plugins[0].account_ref =
            Some("account://test-provider/provider-account-42".to_string());
        let registered = resolve(
            CapabilityRequirement {
                requirement_id: "req-account-registered".to_string(),
                capability: "test.capability".to_string(),
                mode: "paper".to_string(),
                declaration: "required".to_string(),
                asset_class: Some("crypto".to_string()),
                instrument_family: None,
                strategy_id: None,
                plugin_ref: Some("tradeassembly.test".to_string()),
                operation_id: None,
                account_ref: Some("account://test-provider/provider-account-42".to_string()),
                purpose: None,
                ..Default::default()
            },
            registered_catalog,
        );
        assert!(registered.ok, "{registered:#?}");
    }

    #[test]
    fn resolver_ignores_expired_or_revoked_entitlement_grants() {
        let requirement = CapabilityRequirement {
            requirement_id: "req-expired-grants".to_string(),
            capability: "test.capability".to_string(),
            mode: "paper".to_string(),
            declaration: "required".to_string(),
            asset_class: Some("crypto".to_string()),
            instrument_family: None,
            strategy_id: None,
            plugin_ref: Some("tradeassembly.test".to_string()),
            operation_id: Some("test.operation".to_string()),
            account_ref: Some("account://test-provider/paper".to_string()),
            purpose: None,
            ..Default::default()
        };
        let resolution = resolve(
            requirement,
            CapabilityResolverCatalog {
                plugins: vec![test_plugin(true, true)],
                entitlements: vec![
                    entitlement(
                        "expired-deny",
                        "deny",
                        100,
                        Some("2020-01-01T00:00:00Z"),
                        None,
                    ),
                    entitlement(
                        "revoked-deny",
                        "deny",
                        200,
                        None,
                        Some("2026-07-08T00:00:00Z"),
                    ),
                    entitlement("active-allow", "allow", 1, None, None),
                ],
            },
        );

        assert!(resolution.ok, "{resolution:#?}");
        assert_eq!(resolution.entitlement_decisions[0].decision, "allow");
        assert_eq!(
            resolution.entitlement_decisions[0].grant_id.as_deref(),
            Some("active-allow")
        );
    }

    #[test]
    fn graph_resolves_transitive_dependency_from_a_different_plugin() {
        let mut indicator = test_operation("indicator.rsi", "indicator.rsi.calculate@1");
        indicator.dependencies = vec![PluginOperationDependency {
            dependency_id: "bars".to_string(),
            capability: "market_data.bars.read@1".to_string(),
            operation_id: Some("bars.read".to_string()),
            required: true,
            traits: PluginOperationTraits {
                data_shapes: vec!["bars".to_string()],
                fields: vec!["close".to_string()],
                deterministic: true,
                replayable: true,
                ..Default::default()
            },
        }];
        let mut bars = test_operation("bars.read", "market_data.bars.read@1");
        bars.traits.data_shapes = vec!["bars".to_string()];
        bars.traits.fields = vec!["close".to_string()];
        bars.traits.deterministic = true;
        bars.traits.replayable = true;
        let graph = resolve_graph(
            graph_request(vec![requirement("indicator", "indicator.rsi.calculate@1")]),
            graph_catalog(
                vec![
                    graph_plugin("indicator-instance", "example.indicator", indicator),
                    graph_plugin("data-instance", "example.data", bars),
                ],
                &["indicator.rsi.calculate@1", "market_data.bars.read@1"],
            ),
        );

        assert!(graph.ok, "{graph:#?}");
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.origin == "plugin_dependency")
                .and_then(|node| node.selected.as_ref())
                .map(|selected| selected.plugin_ref.as_str()),
            Some("example.data")
        );
    }

    #[test]
    fn graph_fails_closed_for_missing_and_cyclic_dependencies() {
        let mut missing = requirement("missing", "test.capability");
        missing.dependency_refs = vec!["not-present".to_string()];
        let mut left = requirement("left", "left.capability");
        left.dependency_refs = vec!["right".to_string()];
        let mut right = requirement("right", "right.capability");
        right.dependency_refs = vec!["left".to_string()];
        let graph = resolve_graph(
            graph_request(vec![missing, left, right]),
            graph_catalog(
                vec![
                    graph_plugin(
                        "test",
                        "example.test",
                        test_operation("test", "test.capability"),
                    ),
                    graph_plugin(
                        "left",
                        "example.left",
                        test_operation("left", "left.capability"),
                    ),
                    graph_plugin(
                        "right",
                        "example.right",
                        test_operation("right", "right.capability"),
                    ),
                ],
                &["test.capability", "left.capability", "right.capability"],
            ),
        );

        assert!(!graph.ok);
        assert!(graph
            .blockers
            .iter()
            .any(|item| item.code == "dependency_missing"));
        assert!(graph
            .blockers
            .iter()
            .any(|item| item.code == "dependency_cycle"));
    }

    #[test]
    fn graph_detects_cycles_across_plugin_operation_dependencies() {
        let mut alpha = test_operation("alpha", "alpha.capability@1");
        alpha.dependencies = vec![PluginOperationDependency {
            dependency_id: "beta".to_string(),
            capability: "beta.capability@1".to_string(),
            operation_id: Some("beta".to_string()),
            required: true,
            traits: Default::default(),
        }];
        let mut beta = test_operation("beta", "beta.capability@1");
        beta.dependencies = vec![PluginOperationDependency {
            dependency_id: "alpha".to_string(),
            capability: "alpha.capability@1".to_string(),
            operation_id: Some("alpha".to_string()),
            required: true,
            traits: Default::default(),
        }];
        let graph = resolve_graph(
            graph_request(vec![requirement("alpha-root", "alpha.capability@1")]),
            graph_catalog(
                vec![
                    graph_plugin("alpha", "example.alpha", alpha),
                    graph_plugin("beta", "example.beta", beta),
                ],
                &["alpha.capability@1", "beta.capability@1"],
            ),
        );

        assert!(!graph.ok);
        assert!(graph
            .blockers
            .iter()
            .any(|item| item.code == "dependency_cycle"));
    }

    #[test]
    fn graph_requires_explicit_choice_and_honors_configured_order() {
        let plugins = vec![
            graph_plugin(
                "instance-a",
                "example.a",
                test_operation("read", "shared.capability"),
            ),
            graph_plugin(
                "instance-b",
                "example.b",
                test_operation("read", "shared.capability"),
            ),
        ];
        let catalog = graph_catalog(plugins, &["shared.capability"]);
        let ambiguous = resolve_graph(
            graph_request(vec![requirement("shared", "shared.capability")]),
            catalog.clone(),
        );
        assert!(!ambiguous.ok);
        assert_eq!(ambiguous.nodes[0].blockers[0].code, "ambiguous_binding");

        let mut explicit_only = requirement("shared", "shared.capability");
        explicit_only.fallback_policy = "explicit_only".to_string();
        let explicit_only = resolve_graph(graph_request(vec![explicit_only]), catalog.clone());
        assert_eq!(
            explicit_only.nodes[0].blockers[0].code,
            "fallback_not_permitted"
        );

        let mut configured = requirement("shared", "shared.capability");
        configured.fallback_policy = "configured".to_string();
        let mut request = graph_request(vec![configured]);
        request.configured_fallbacks.insert(
            "shared".to_string(),
            vec![
                CapabilityBinding {
                    plugin_instance_ref: Some("does-not-exist".to_string()),
                    ..Default::default()
                },
                CapabilityBinding {
                    plugin_instance_ref: Some("instance-b".to_string()),
                    operation_id: Some("read".to_string()),
                    ..Default::default()
                },
            ],
        );
        let configured = resolve_graph(request, catalog);
        assert!(configured.ok, "{configured:#?}");
        let selected = configured.nodes[0].selected.as_ref().unwrap();
        assert_eq!(selected.plugin_instance_ref, "instance-b");
        assert_eq!(selected.selection_reason, "configured_fallback:1");
    }

    #[test]
    fn graph_requires_exact_operation_ids_and_uses_graph_blocker_names() {
        let operation = test_operation("bars.read", "market_data.bars.read@1");
        let catalog = graph_catalog(
            vec![graph_plugin("data", "example.data", operation)],
            &["market_data.bars.read@1"],
        );
        for alias in ["read", "test.read"] {
            let mut requested = requirement("bars", "market_data.bars.read@1");
            requested.operation_id = Some(alias.to_string());
            let graph = resolve_graph(graph_request(vec![requested]), catalog.clone());
            assert!(!graph.ok, "alias {alias} resolved: {graph:#?}");
            assert!(graph
                .blockers
                .iter()
                .any(|item| item.code == "missing_operation"));
            assert!(!graph
                .blockers
                .iter()
                .any(|item| item.code == "operation_unsupported"));
        }

        let mut exact = requirement("bars", "market_data.bars.read@1");
        exact.operation_id = Some("bars.read".to_string());
        assert!(resolve_graph(graph_request(vec![exact]), catalog).ok);
    }

    #[test]
    fn graph_entitlement_expiry_uses_only_the_supplied_epoch() {
        let capability = "market_data.bars.read@1";
        let mut grant = entitlement_with_capability(capability, BTreeMap::new());
        grant.expires_at = Some("2030-01-01T00:00:00Z".to_string());
        let catalog = CapabilityResolverCatalog {
            plugins: vec![graph_plugin(
                "data",
                "example.data",
                test_operation("bars.read", capability),
            )],
            entitlements: vec![grant],
        };
        let mut before_request = graph_request(vec![requirement("bars", capability)]);
        before_request.evaluation_epoch = "2029-12-31T23:59:59Z".to_string();
        let first = resolve_graph(before_request.clone(), catalog.clone());
        let replay = resolve_graph(before_request, catalog.clone());
        assert!(first.ok, "{first:#?}");
        assert_eq!(first, replay);

        let mut after_request = graph_request(vec![requirement("bars", capability)]);
        after_request.evaluation_epoch = "2030-01-01T00:00:00Z".to_string();
        let expired = resolve_graph(after_request, catalog.clone());
        assert!(!expired.ok, "{expired:#?}");
        assert!(expired
            .blockers
            .iter()
            .any(|item| item.code == "entitlement_denied"));

        let mut opaque_request = graph_request(vec![requirement("bars", capability)]);
        opaque_request.evaluation_epoch = "catalog-v1".to_string();
        assert!(!resolve_graph(opaque_request, catalog).ok);
    }

    #[test]
    fn graph_uses_entitlement_denied_when_no_grant_exists() {
        let capability = "market_data.bars.read@1";
        let graph = resolve_graph(
            graph_request(vec![requirement("bars", capability)]),
            CapabilityResolverCatalog {
                plugins: vec![graph_plugin(
                    "data",
                    "example.data",
                    test_operation("bars.read", capability),
                )],
                entitlements: Vec::new(),
            },
        );
        assert!(!graph.ok, "{graph:#?}");
        assert!(graph
            .blockers
            .iter()
            .any(|item| item.code == "entitlement_denied"));
        assert!(!graph
            .blockers
            .iter()
            .any(|item| item.code == "no_entitlement_grant"));
    }

    #[test]
    fn graph_evaluates_every_typed_constraint_family() {
        let mut operation = test_operation("bars.read", "market_data.bars.read@1");
        operation.traits = PluginOperationTraits {
            modes: vec!["backtest".to_string()],
            instrument_families: vec!["equity".to_string()],
            data_shapes: vec!["bars".to_string()],
            operations: vec!["read".to_string()],
            fields: vec!["open".to_string(), "close".to_string()],
            input_paths: vec!["market_data.source_bars".to_string()],
            input_schema_refs: vec!["schema://bars/request@1".to_string()],
            output_schema_refs: vec!["schema://bars/result@1".to_string()],
            timeframes: vec!["1d".to_string()],
            max_freshness: Some("PT5M".to_string()),
            max_latency_ms: Some(250),
            deterministic: true,
            replayable: true,
        };
        let mut valid = requirement("bars", "market_data.bars.read@1");
        valid.mode = "backtest".to_string();
        valid.instrument_family = Some("equity".to_string());
        valid.constraints = CapabilityRequirementConstraints {
            instrument_families: vec!["equity".to_string()],
            data_shapes: vec!["bars".to_string()],
            operations: vec!["read".to_string()],
            fields: vec!["close".to_string()],
            input_paths: vec!["market_data.source_bars".to_string()],
            schema_refs: vec![
                "schema://bars/request@1".to_string(),
                "schema://bars/result@1".to_string(),
            ],
            timeframe: Some("1d".to_string()),
            max_freshness: Some("PT10M".to_string()),
            max_latency_ms: Some(500),
            deterministic: true,
            replayable: true,
        };
        let catalog = graph_catalog(
            vec![graph_plugin("data", "example.data", operation)],
            &["market_data.bars.read@1"],
        );
        let valid_graph = resolve_graph(graph_request(vec![valid.clone()]), catalog.clone());
        assert!(valid_graph.ok, "{valid_graph:#?}");

        type RequirementMutation = fn(&mut CapabilityRequirement);
        let cases: [(&str, RequirementMutation); 9] = [
            ("instrument", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.instrument_families = vec!["future".to_string()]
            }),
            ("shape", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.data_shapes = vec!["ticks".to_string()]
            }),
            ("field", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.fields = vec!["bid".to_string()]
            }),
            ("operation", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.operations = vec!["other.read".to_string()]
            }),
            ("schema", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.schema_refs = vec!["schema://other@1".to_string()]
            }),
            ("timeframe", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.timeframe = Some("1m".to_string())
            }),
            ("freshness", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.max_freshness = Some("PT1M".to_string())
            }),
            ("latency", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.max_latency_ms = Some(100)
            }),
            ("input_path", |requirement: &mut CapabilityRequirement| {
                requirement.constraints.input_paths = vec!["unsupported.path".to_string()]
            }),
        ];
        for (label, mutate) in cases {
            let mut invalid = valid.clone();
            mutate(&mut invalid);
            let graph = resolve_graph(graph_request(vec![invalid]), catalog.clone());
            assert!(!graph.ok, "{label}: {graph:#?}");
        }

        let mut weak_operation = test_operation("bars.read", "market_data.bars.read@1");
        weak_operation.traits.modes = vec!["backtest".to_string()];
        weak_operation.traits.instrument_families = vec!["equity".to_string()];
        weak_operation.traits.data_shapes = vec!["bars".to_string()];
        weak_operation.traits.fields = vec!["open".to_string(), "close".to_string()];
        weak_operation.traits.input_schema_refs = vec!["schema://bars/request@1".to_string()];
        weak_operation.traits.output_schema_refs = vec!["schema://bars/result@1".to_string()];
        weak_operation.traits.timeframes = vec!["1d".to_string()];
        weak_operation.traits.max_freshness = Some("PT5M".to_string());
        weak_operation.traits.max_latency_ms = Some(250);
        weak_operation.traits.deterministic = false;
        weak_operation.traits.replayable = false;
        let quality_graph = resolve_graph(
            graph_request(vec![valid]),
            graph_catalog(
                vec![graph_plugin("weak", "example.weak", weak_operation)],
                &["market_data.bars.read@1"],
            ),
        );
        assert!(quality_graph.nodes[0].candidates[0]
            .blockers
            .iter()
            .any(|item| item.code == "quality_requirement_unmet"));
    }

    #[test]
    fn optional_missing_nodes_do_not_block_unrelated_readiness() {
        let mut required = requirement("required", "present.capability");
        required.required_for = vec!["backtest".to_string()];
        let mut optional = requirement("optional", "missing.capability");
        optional.declaration = "optional".to_string();
        optional.required_for = vec!["backtest".to_string()];
        let graph = resolve_graph(
            graph_request(vec![optional, required]),
            graph_catalog(
                vec![graph_plugin(
                    "present",
                    "example.present",
                    test_operation("present", "present.capability"),
                )],
                &["present.capability"],
            ),
        );

        assert!(graph.ok, "{graph:#?}");
        assert!(graph
            .readiness
            .iter()
            .all(|readiness| readiness.state == "ready"));
        assert!(graph
            .nodes
            .iter()
            .find(|node| node.node_id == "optional")
            .is_some_and(|node| node.selected.is_none()));
    }

    #[test]
    fn setup_discovery_requires_safe_declaration_and_preserves_other_health_gates() {
        let mut operation = test_operation("plugin.capability_matrix", "plugin.capability_matrix");
        operation.apf_action_id = "plugin.describe".into();
        for health in ["needs_configuration", "needs_credentials"] {
            assert!(super::setup_discovery_allowed(&operation, health));
            let mut plugin = graph_plugin("discovery", "example.plugin", operation.clone());
            plugin.health = health.into();
            plugin.credential_status.configured = false;
            let graph = resolve_graph(
                graph_request(vec![requirement("inspect", "plugin.capability_matrix")]),
                graph_catalog(vec![plugin.clone()], &["plugin.capability_matrix"]),
            );
            assert!(!graph.nodes[0].candidates[0]
                .blockers
                .iter()
                .any(|b| b.code == "plugin_unhealthy"));
            plugin.enabled = false;
            let graph = resolve_graph(
                graph_request(vec![requirement("inspect", "plugin.capability_matrix")]),
                graph_catalog(vec![plugin], &["plugin.capability_matrix"]),
            );
            assert!(graph.nodes[0].candidates[0]
                .blockers
                .iter()
                .any(|b| b.code == "disabled_plugin"));
        }
        for health in ["disabled", "degraded", "host_unavailable", "unknown"] {
            assert!(!super::setup_discovery_allowed(&operation, health));
        }
        for change in 0..6 {
            let mut unsafe_operation = operation.clone();
            match change {
                0 => unsafe_operation.id = "account.health".into(),
                1 => unsafe_operation.capability = "broker.submit".into(),
                2 => unsafe_operation.effect = "write".into(),
                3 => unsafe_operation.apf_action_id = "order.submit".into(),
                4 => unsafe_operation.credential_grant_required = true,
                _ => unsafe_operation.account_binding_required = true,
            }
            assert!(!super::setup_discovery_allowed(
                &unsafe_operation,
                "needs_configuration"
            ));
        }
    }

    #[test]
    fn graph_blocks_unhealthy_credential_account_authority_and_entitlement_limits() {
        let mut operation = test_operation("submit", "broker.submit@1");
        operation.credential_grant_required = true;
        operation.account_binding_required = true;
        let mut plugin = graph_plugin("broker", "example.broker", operation.clone());
        plugin.health = "degraded".to_string();
        plugin.credential_status.configured = false;
        let mut request_requirement = requirement("submit", "broker.submit@1");
        request_requirement.mode = "live".to_string();
        let catalog = CapabilityResolverCatalog {
            plugins: vec![plugin],
            entitlements: vec![entitlement_with_capability(
                "broker.submit@1",
                BTreeMap::from([("mode".to_string(), json!("paper"))]),
            )],
        };
        let graph = resolve_graph(graph_request(vec![request_requirement]), catalog);
        let codes = graph.nodes[0]
            .candidates
            .iter()
            .flat_map(|candidate| candidate.blockers.iter())
            .map(|item| item.code.as_str())
            .collect::<BTreeSet<_>>();
        for code in [
            "plugin_unhealthy",
            "credential_missing",
            "account_binding_required",
            "entitlement_limit_exceeded",
        ] {
            assert!(codes.contains(code), "missing {code}: {graph:#?}");
        }
        assert_eq!(graph.nodes[0].candidates[0].health, "degraded");
        let mut serialized = serde_json::to_value(&graph).unwrap();
        assert_eq!(
            serialized["nodes"][0]["candidates"][0]["health"],
            json!("degraded")
        );
        serialized["nodes"][0]["candidates"][0]
            .as_object_mut()
            .unwrap()
            .remove("health");
        let historical: CapabilityGraphResolution = serde_json::from_value(serialized).unwrap();
        assert_eq!(historical.nodes[0].candidates[0].health, "");

        let mut no_authority = test_operation("read", "authority.missing@1");
        no_authority.apf_action_id.clear();
        let authority_graph = resolve_graph(
            graph_request(vec![requirement("authority", "authority.missing@1")]),
            graph_catalog(
                vec![graph_plugin("authority", "example.authority", no_authority)],
                &["authority.missing@1"],
            ),
        );
        assert!(authority_graph.nodes[0].candidates[0]
            .blockers
            .iter()
            .any(|item| item.code == "authority_metadata_missing"));
    }

    #[test]
    fn graph_fingerprint_is_order_stable_and_output_contains_no_secret_values() {
        let requirements = vec![
            requirement("alpha", "alpha.capability"),
            requirement("beta", "beta.capability"),
        ];
        let plugins = vec![
            graph_plugin(
                "alpha",
                "example.alpha",
                test_operation("alpha", "alpha.capability"),
            ),
            graph_plugin(
                "beta",
                "example.beta",
                test_operation("beta", "beta.capability"),
            ),
        ];
        let first = resolve_graph(
            graph_request(requirements.clone()),
            graph_catalog(plugins.clone(), &["alpha.capability", "beta.capability"]),
        );
        let second = resolve_graph(
            graph_request(requirements.into_iter().rev().collect()),
            graph_catalog(
                plugins.into_iter().rev().collect(),
                &["beta.capability", "alpha.capability"],
            ),
        );

        assert_eq!(first.graph_fingerprint, second.graph_fingerprint);
        let rendered = serde_json::to_string(&first).unwrap();
        assert!(!rendered.contains("api_secret"));
        assert!(!rendered.contains("credential_value"));
        assert!(first
            .nodes
            .iter()
            .all(|node| node.selected.as_ref().is_some_and(|selected| {
                !selected.manifest_fingerprint.is_empty()
                    && selected.entitlement_decision_ref.is_some()
            })));
    }

    fn entitlement(
        grant_id: &str,
        effect: &str,
        precedence: i64,
        expires_at: Option<&str>,
        revoked_at: Option<&str>,
    ) -> EntitlementGrant {
        EntitlementGrant {
            grant_id: grant_id.to_string(),
            profile: LOCAL_FULL_PROFILE.to_string(),
            source: "test".to_string(),
            capability: "test.capability".to_string(),
            effect: effect.to_string(),
            state: "active".to_string(),
            scope: "global".to_string(),
            source_type: "test".to_string(),
            source_ref: None,
            precedence,
            limits: Default::default(),
            issued_at: None,
            expires_at: expires_at.map(str::to_string),
            revoked_at: revoked_at.map(str::to_string),
            reason: None,
        }
    }

    fn entitlement_with_capability(
        capability: &str,
        limits: BTreeMap<String, Value>,
    ) -> EntitlementGrant {
        EntitlementGrant {
            grant_id: format!("grant:{capability}"),
            profile: LOCAL_FULL_PROFILE.to_string(),
            source: "test".to_string(),
            capability: capability.to_string(),
            effect: "allow".to_string(),
            state: "active".to_string(),
            scope: "global".to_string(),
            source_type: "test".to_string(),
            source_ref: None,
            precedence: 10,
            limits,
            issued_at: None,
            expires_at: None,
            revoked_at: None,
            reason: None,
        }
    }

    fn requirement(id: &str, capability: &str) -> CapabilityRequirement {
        CapabilityRequirement {
            requirement_id: id.to_string(),
            capability: capability.to_string(),
            mode: "paper".to_string(),
            declaration: "required".to_string(),
            fallback_policy: "none".to_string(),
            ..Default::default()
        }
    }

    fn graph_request(requirements: Vec<CapabilityRequirement>) -> CapabilityGraphRequest {
        CapabilityGraphRequest {
            requirements,
            bindings: BTreeMap::new(),
            configured_fallbacks: BTreeMap::new(),
            evaluation_epoch: "catalog-v1".to_string(),
        }
    }

    fn graph_catalog(
        plugins: Vec<PluginCatalogEntry>,
        capabilities: &[&str],
    ) -> CapabilityResolverCatalog {
        CapabilityResolverCatalog {
            plugins,
            entitlements: capabilities
                .iter()
                .map(|capability| entitlement_with_capability(capability, BTreeMap::new()))
                .collect(),
        }
    }

    fn graph_plugin(
        instance_ref: &str,
        plugin_ref: &str,
        operation: PluginOperationContract,
    ) -> PluginCatalogEntry {
        PluginCatalogEntry {
            instance_ref: instance_ref.to_string(),
            plugin_ref: plugin_ref.to_string(),
            provider_ref: instance_ref.to_string(),
            account_ref: None,
            name: plugin_ref.to_string(),
            enabled: true,
            trust_level: "test".to_string(),
            manifest_fingerprint: format!("sha256:{plugin_ref}"),
            capabilities: vec![operation.capability.clone()],
            operations: vec![operation],
            credential_status: CredentialResolutionStatus {
                configured: true,
                custody: "test-opaque".to_string(),
                status: "stored".to_string(),
                revision_ref: None,
            },
            health: "ok".to_string(),
        }
    }

    fn test_operation(id: &str, capability: &str) -> PluginOperationContract {
        PluginOperationContract {
            id: id.to_string(),
            capability: capability.to_string(),
            protocol: "local-rust".to_string(),
            resource_type: "test_resource".to_string(),
            finance_resource_type: "test_resource".to_string(),
            effect: "read".to_string(),
            risk: "low".to_string(),
            credential_grant_required: false,
            account_binding_required: false,
            apf_action_id: "test.read".to_string(),
            mandate_required: false,
            purpose: "test_evaluation".to_string(),
            evidence: vec!["request_hash".to_string()],
            pep_coverage_class: "c1".to_string(),
            check_packs: vec!["test_checks".to_string()],
            receipt_class: "test_receipt".to_string(),
            redaction: "hash_or_ref".to_string(),
            no_advice: true,
            session_admission: "user_or_agent_session".to_string(),
            approval_mode: "policy_configured".to_string(),
            supervision_mode: "automated_policy".to_string(),
            detectors: vec!["no_advice".to_string(), "capability_scope".to_string()],
            context_providers: vec!["tradeassembly.strategy_context".to_string()],
            policy_refs: vec!["tradeassembly.local_full".to_string()],
            traits: PluginOperationTraits {
                modes: vec!["paper".to_string(), "backtest".to_string()],
                ..Default::default()
            },
            dependencies: Vec::new(),
            supported_protocols: vec!["local-rust".to_string(), "mode:paper".to_string()],
        }
    }

    fn test_plugin(configured: bool, account_binding_required: bool) -> PluginCatalogEntry {
        PluginCatalogEntry {
            instance_ref: "test-provider".to_string(),
            plugin_ref: "tradeassembly.test".to_string(),
            provider_ref: "test-provider".to_string(),
            account_ref: None,
            name: "Test Plugin".to_string(),
            enabled: true,
            trust_level: "test".to_string(),
            manifest_fingerprint: "sha256:test-plugin".to_string(),
            capabilities: vec!["test.capability".to_string()],
            operations: vec![PluginOperationContract {
                id: "test.operation".to_string(),
                capability: "test.capability".to_string(),
                protocol: "local-rust".to_string(),
                resource_type: "test_resource".to_string(),
                finance_resource_type: "test_resource".to_string(),
                effect: "write".to_string(),
                risk: "medium".to_string(),
                credential_grant_required: true,
                account_binding_required,
                apf_action_id: "test.action".to_string(),
                mandate_required: true,
                purpose: "test".to_string(),
                evidence: vec!["request_hash".to_string()],
                pep_coverage_class: "c3".to_string(),
                check_packs: vec!["test_checks".to_string()],
                receipt_class: "test_receipt".to_string(),
                redaction: "hash_or_ref".to_string(),
                no_advice: true,
                session_admission: "user_or_agent_session".to_string(),
                approval_mode: "policy_configured".to_string(),
                supervision_mode: "automated_policy".to_string(),
                detectors: vec![
                    "no_advice".to_string(),
                    "capability_scope".to_string(),
                    "credential_scope".to_string(),
                    "risk_limit".to_string(),
                ],
                context_providers: vec![
                    "sightline.session_context".to_string(),
                    "tradeassembly.strategy_context".to_string(),
                ],
                policy_refs: vec![
                    "apf.finance.default".to_string(),
                    "tradeassembly.local_full".to_string(),
                ],
                traits: Default::default(),
                dependencies: Vec::new(),
                supported_protocols: vec![
                    "mode:paper".to_string(),
                    "asset:crypto".to_string(),
                    "local-rust".to_string(),
                ],
            }],
            credential_status: CredentialResolutionStatus {
                configured,
                custody: "test".to_string(),
                status: if configured { "stored" } else { "missing" }.to_string(),
                revision_ref: None,
            },
            health: "ok".to_string(),
        }
    }
}
