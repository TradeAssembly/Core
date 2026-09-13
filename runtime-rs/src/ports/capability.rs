// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::VersionedPort;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequirement {
    pub requirement_id: String,
    pub capability: String,
    pub mode: String,
    #[serde(default)]
    pub declaration: String,
    #[serde(default)]
    pub required_for: Vec<String>,
    #[serde(default)]
    pub stage_refs: Vec<String>,
    #[serde(default)]
    pub substep_refs: Vec<String>,
    #[serde(default)]
    pub constraints: CapabilityRequirementConstraints,
    #[serde(default)]
    pub dependency_refs: Vec<String>,
    #[serde(default)]
    pub fallback_policy: String,
    #[serde(default)]
    pub policy_tags: Vec<String>,
    pub asset_class: Option<String>,
    pub instrument_family: Option<String>,
    pub strategy_id: Option<String>,
    pub strategy_version_id: Option<String>,
    pub plugin_instance_ref: Option<String>,
    pub plugin_ref: Option<String>,
    pub operation_id: Option<String>,
    pub account_ref: Option<String>,
    pub purpose: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequirementConstraints {
    #[serde(default)]
    pub instrument_families: Vec<String>,
    #[serde(default)]
    pub data_shapes: Vec<String>,
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub input_paths: Vec<String>,
    #[serde(default)]
    pub schema_refs: Vec<String>,
    pub timeframe: Option<String>,
    pub max_freshness: Option<String>,
    pub max_latency_ms: Option<u64>,
    #[serde(default)]
    pub deterministic: bool,
    #[serde(default)]
    pub replayable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialResolutionStatus {
    pub configured: bool,
    pub custody: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginOperationContract {
    pub id: String,
    pub capability: String,
    pub protocol: String,
    pub resource_type: String,
    pub finance_resource_type: String,
    pub effect: String,
    pub risk: String,
    pub credential_grant_required: bool,
    pub account_binding_required: bool,
    pub apf_action_id: String,
    pub mandate_required: bool,
    pub purpose: String,
    pub evidence: Vec<String>,
    pub pep_coverage_class: String,
    pub check_packs: Vec<String>,
    pub receipt_class: String,
    pub redaction: String,
    pub no_advice: bool,
    #[serde(default)]
    pub session_admission: String,
    #[serde(default)]
    pub approval_mode: String,
    #[serde(default)]
    pub supervision_mode: String,
    #[serde(default)]
    pub detectors: Vec<String>,
    #[serde(default)]
    pub context_providers: Vec<String>,
    #[serde(default)]
    pub policy_refs: Vec<String>,
    #[serde(default)]
    pub traits: PluginOperationTraits,
    #[serde(default)]
    pub dependencies: Vec<PluginOperationDependency>,
    pub supported_protocols: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginOperationTraits {
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub instrument_families: Vec<String>,
    #[serde(default)]
    pub data_shapes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<String>,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_paths: Vec<String>,
    #[serde(default)]
    pub input_schema_refs: Vec<String>,
    #[serde(default)]
    pub output_schema_refs: Vec<String>,
    #[serde(default)]
    pub timeframes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_freshness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_latency_ms: Option<u64>,
    #[serde(default)]
    pub deterministic: bool,
    #[serde(default)]
    pub replayable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginOperationDependency {
    pub dependency_id: String,
    pub capability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default = "default_dependency_required")]
    pub required: bool,
    #[serde(default)]
    pub traits: PluginOperationTraits,
}

fn default_dependency_required() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApfOperationMetadata {
    pub action_id: String,
    pub resource_type: String,
    pub operation_resource_type: String,
    pub mandate_required: bool,
    pub purpose: String,
    pub evidence: Vec<String>,
    pub pep_coverage_class: String,
    pub check_packs: Vec<String>,
    pub receipt_class: String,
    pub credential_grant_required: bool,
    pub account_binding_required: bool,
    pub session_admission: String,
    pub approval_mode: String,
    pub supervision_mode: String,
    pub detectors: Vec<String>,
    pub context_providers: Vec<String>,
    pub policy_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalogEntry {
    #[serde(default)]
    pub instance_ref: String,
    pub plugin_ref: String,
    pub provider_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub trust_level: String,
    #[serde(default)]
    pub manifest_fingerprint: String,
    pub capabilities: Vec<String>,
    pub operations: Vec<PluginOperationContract>,
    pub credential_status: CredentialResolutionStatus,
    pub health: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementGrant {
    pub grant_id: String,
    pub profile: String,
    pub source: String,
    pub capability: String,
    pub effect: String,
    pub state: String,
    #[serde(default = "default_entitlement_scope")]
    pub scope: String,
    #[serde(default = "default_entitlement_source_type")]
    pub source_type: String,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub precedence: i64,
    #[serde(default)]
    pub limits: BTreeMap<String, Value>,
    #[serde(default)]
    pub issued_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<String>,
    pub reason: Option<String>,
}

fn default_entitlement_scope() -> String {
    "global".to_string()
}

fn default_entitlement_source_type() -> String {
    "local_default".to_string()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResolverCatalog {
    pub plugins: Vec<PluginCatalogEntry>,
    pub entitlements: Vec<EntitlementGrant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityBlocker {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementDecision {
    pub profile: String,
    pub source: String,
    pub capability: String,
    pub decision: String,
    pub precedence: i64,
    #[serde(default)]
    pub limits: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCandidate {
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub provider_ref: String,
    pub name: String,
    pub manifest_fingerprint: String,
    pub enabled: bool,
    #[serde(default)]
    pub health: String,
    pub capability: String,
    pub operation: PluginOperationContract,
    pub apf: ApfOperationMetadata,
    pub credential_status: CredentialResolutionStatus,
    pub entitlement_decision: EntitlementDecision,
    pub blockers: Vec<CapabilityBlocker>,
    pub warnings: Vec<String>,
    pub resolution_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityResolution {
    pub schema_version: String,
    pub ok: bool,
    pub state: String,
    pub requirement: CapabilityRequirement,
    pub profile: EntitlementProfile,
    pub candidates: Vec<CapabilityCandidate>,
    pub blockers: Vec<CapabilityBlocker>,
    pub entitlement_decisions: Vec<EntitlementDecision>,
    pub no_silent_fallback: bool,
    pub no_advice: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementProfile {
    pub id: String,
    pub source: String,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_instance_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityGraphRequest {
    pub requirements: Vec<CapabilityRequirement>,
    #[serde(default)]
    pub bindings: BTreeMap<String, CapabilityBinding>,
    #[serde(default)]
    pub configured_fallbacks: BTreeMap<String, Vec<CapabilityBinding>>,
    #[serde(default)]
    pub evaluation_epoch: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySelection {
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub provider_ref: String,
    pub operation_id: String,
    pub account_ref: Option<String>,
    pub manifest_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_revision_ref: Option<String>,
    pub entitlement_decision_ref: Option<String>,
    pub selection_reason: String,
    pub operation: PluginOperationContract,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityGraphNode {
    pub node_id: String,
    pub origin: String,
    pub parent_node_id: Option<String>,
    pub dependency_id: Option<String>,
    pub required: bool,
    pub requirement: CapabilityRequirement,
    pub candidates: Vec<CapabilityCandidate>,
    pub selected: Option<CapabilitySelection>,
    pub blockers: Vec<CapabilityBlocker>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityGraphEdge {
    pub from_node_id: String,
    pub to_node_id: String,
    pub dependency_id: String,
    pub origin: String,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReadiness {
    pub level: String,
    pub state: String,
    pub blocking_node_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityGraphResolution {
    pub schema_version: String,
    pub ok: bool,
    pub state: String,
    pub evaluation_epoch: String,
    pub nodes: Vec<CapabilityGraphNode>,
    pub edges: Vec<CapabilityGraphEdge>,
    pub blockers: Vec<CapabilityBlocker>,
    pub readiness: Vec<CapabilityReadiness>,
    pub graph_fingerprint: String,
    pub no_silent_fallback: bool,
    pub no_advice: bool,
}

pub trait CapabilityResolverPort: VersionedPort {
    fn resolve(
        &self,
        requirement: CapabilityRequirement,
        catalog: CapabilityResolverCatalog,
    ) -> CapabilityResolution;

    fn resolve_graph(
        &self,
        request: CapabilityGraphRequest,
        catalog: CapabilityResolverCatalog,
    ) -> CapabilityGraphResolution;
}
