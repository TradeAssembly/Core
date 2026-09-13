// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{SideEffectContext, VersionedPort};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationRequest {
    #[serde(default)]
    pub correlation_id: String,
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub manifest_fingerprint: String,
    pub operation_id: String,
    pub capability: String,
    pub capability_graph_revision_id: String,
    pub capability_graph_fingerprint: String,
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub strategy_spec_hash: String,
    pub activation_id: String,
    pub attempt_id: String,
    pub evaluation_tick_id: String,
    pub mode: String,
    pub purpose: String,
    #[serde(default)]
    pub account_ref: Option<String>,
    pub timeout_ms: u64,
    #[serde(default)]
    pub fencing_token: Option<i64>,
    pub input: Value,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationResponse {
    #[serde(default)]
    pub correlation_id: String,
    pub schema_ref: String,
    pub payload: Value,
    pub observed_at_ms: i64,
    pub source_event_id: String,
    pub content_hash: String,
    pub freshness_state: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    pub deterministic: bool,
    pub replayable: bool,
    #[serde(default)]
    pub provider_outcome_id: Option<String>,
    pub reconciliation_required: bool,
}

pub trait PluginOperationPort: VersionedPort {
    /// Observation-only recovery. The opaque plan can only be constructed after
    /// original evidence and current observer authentication have been checked.
    fn recover_ambiguous_broker_order(
        &self,
        _plan: &crate::broker_submission::BrokerOrderRecoveryPlan,
        _context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        Err("broker_order_recovery_unsupported".into())
    }

    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String>;
}
