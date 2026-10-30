//! Host-only plan preparation/persistence. No public transport route or evaluator.
use super::{capability_graph, TradeAssemblyService};
use crate::data_pipeline::{
    canonical_sha256, compile_data_plan, CompiledDataPlan, PipelineRequest, VerifiedDataContract,
};
use crate::ports::{
    CapabilityGraphResolution, ComparePutOutcome, SideEffectContext, StorageExpectation,
    StoragePort, StorageWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

const PLANS: &str = "compiled_data_plan_revisions_v1";
const KEYS: &str = "compiled_data_plan_idempotency_v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanScope {
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub strategy_spec_hash: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataPlanContent {
    pub scope: PlanScope,
    pub plan: CompiledDataPlan,
    pub package_sha256: BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredDataPlan {
    pub revision_id: String,
    pub revision_sha256: String,
    pub content: DataPlanContent,
}

fn prepare(
    service: &TradeAssemblyService,
    scope: PlanScope,
    request: PipelineRequest,
) -> Result<DataPlanContent, String> {
    service
        .require_object("strategy", &scope.strategy_id)
        .map_err(|_| "data_plan_not_available")?;
    let now_ms = service.runtime().clock.now_ms();
    if now_ms <= 0 {
        return Err("data_plan_clock_unavailable".into());
    }
    let epoch = chrono::DateTime::from_timestamp_millis(now_ms)
        .ok_or("data_plan_clock_unavailable")?
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let revision = capability_graph::revision_for_context(
        service,
        &request.capability_graph_revision,
        &scope.strategy_id,
        &scope.strategy_version_id,
        &request.mode,
        &epoch,
    )
    .map_err(|_| "data_plan_graph_stale")?;
    if revision["strategySpecHash"] != scope.strategy_spec_hash {
        return Err("data_plan_strategy_mismatch".into());
    }
    let graph: CapabilityGraphResolution =
        serde_json::from_value(revision["graph"].clone()).map_err(|_| "data_plan_graph_invalid")?;
    let runtime = service.runtime();
    let mut contracts = Vec::new();
    let mut packages = BTreeMap::new();
    for node in &request.nodes {
        let selected = graph
            .nodes
            .iter()
            .find(|n| n.node_id == node.graph_node_id)
            .and_then(|n| n.selected.as_ref())
            .ok_or("data_plan_node_unresolved")?;
        service
            .require_object("plugin_instance", &selected.plugin_instance_ref)
            .map_err(|_| "data_plan_not_available")?;
        let instance = runtime
            .plugins
            .get_instance(&selected.plugin_instance_ref)?
            .ok_or("data_plan_instance_missing")?;
        if instance["enabled"] != true
            || instance["pluginRef"] != selected.plugin_ref
            || instance["instanceRef"] != selected.plugin_instance_ref
            || instance.get("removedAtMs").is_some_and(|v| !v.is_null())
        {
            return Err("data_plan_instance_stale".into());
        }
        let digest = instance["activePackageSha256"]
            .as_str()
            .ok_or("data_plan_package_missing")?;
        let manifest_record = runtime
            .plugins
            .get_manifest(&selected.plugin_ref)?
            .ok_or("data_plan_manifest_missing")?;
        if manifest_record["manifestDigest"] != selected.manifest_fingerprint
            || manifest_record["package"]["packageSha256"] != digest
        {
            return Err("data_plan_package_binding_mismatch".into());
        }
        let package = runtime
            .plugin_packages
            .get(digest)?
            .ok_or("data_plan_package_missing")?;
        if package.plugin_ref != selected.plugin_ref
            || package.manifest_sha256 != selected.manifest_fingerprint
        {
            return Err("data_plan_package_binding_mismatch".into());
        }
        let contract = runtime
            .plugin_packages
            .data_contract(digest, &selected.operation_id)?
            .ok_or("data_plan_contract_missing")?;
        let contract_sha256 =
            canonical_sha256(&contract).map_err(|_| "data_plan_contract_invalid")?;
        contracts.push(VerifiedDataContract {
            graph_node_id: node.graph_node_id.clone(),
            manifest_fingerprint: selected.manifest_fingerprint.clone(),
            contract_sha256,
            contract,
        });
        packages.insert(node.graph_node_id.clone(), digest.to_owned());
    }
    let plan = compile_data_plan(&graph, request, contracts).map_err(|errors| {
        format!(
            "data_plan_compile_failed:{}",
            serde_json::to_string(&errors).unwrap_or_default()
        )
    })?;
    Ok(DataPlanContent {
        scope,
        plan,
        package_sha256: packages,
    })
}

pub fn save(
    service: &TradeAssemblyService,
    scope: PlanScope,
    request: PipelineRequest,
    context: &SideEffectContext,
) -> Result<StoredDataPlan, String> {
    let content = prepare(service, scope, request)?;
    persist(service.runtime().storage.as_ref(), content, context)
}

pub fn current(
    service: &TradeAssemblyService,
    revision_id: &str,
) -> Result<StoredDataPlan, String> {
    let value = service
        .runtime()
        .storage
        .get_json(PLANS, revision_id)?
        .ok_or("data_plan_not_available")?;
    let stored: StoredDataPlan =
        serde_json::from_value(value).map_err(|_| "data_plan_record_invalid")?;
    service
        .require_object("strategy", &stored.content.scope.strategy_id)
        .map_err(|_| "data_plan_not_available")?;
    if record(stored.content.clone())? != stored || stored.revision_id != revision_id {
        return Err("data_plan_record_invalid".into());
    }
    let fresh = prepare(
        service,
        stored.content.scope.clone(),
        stored.content.plan.request.clone(),
    )?;
    if fresh != stored.content {
        return Err("data_plan_stale".into());
    }
    Ok(stored)
}

fn record(content: DataPlanContent) -> Result<StoredDataPlan, String> {
    let mut unhashed = content.plan.clone();
    unhashed.plan_sha256.clear();
    if canonical_sha256(&unhashed).map_err(|_| "data_plan_hash_invalid")?
        != content.plan.plan_sha256
    {
        return Err("data_plan_hash_invalid".into());
    }
    let digest = canonical_sha256(&content).map_err(|_| "data_plan_hash_invalid")?;
    Ok(StoredDataPlan {
        revision_id: format!("dataplan_{digest}"),
        revision_sha256: digest,
        content,
    })
}

fn persist(
    storage: &dyn StoragePort,
    content: DataPlanContent,
    context: &SideEffectContext,
) -> Result<StoredDataPlan, String> {
    let stored = record(content)?;
    let value = serde_json::to_value(&stored).map_err(|_| "data_plan_record_invalid")?;
    let key=canonical_sha256(&json!({"strategyId":stored.content.scope.strategy_id,"authority":context.authority,"key":context.idempotency_key.as_str()})).map_err(|_|"data_plan_key_invalid")?;
    let idempotency =
        json!({"revisionId":stored.revision_id,"revisionSha256":stored.revision_sha256});
    for _ in 0..2 {
        let old_key = storage.get_json(KEYS, &key)?;
        let old_plan = storage.get_json(PLANS, &stored.revision_id)?;
        if old_key.as_ref().is_some_and(|v| v != &idempotency) {
            return Err("data_plan_idempotency_conflict".into());
        }
        if old_plan.as_ref().is_some_and(|v| v != &value) {
            return Err("data_plan_immutable_conflict".into());
        }
        if old_key.is_some() {
            return if old_plan.is_some() {
                Ok(stored)
            } else {
                Err("data_plan_partial_commit".into())
            };
        }
        let mut writes = vec![StorageWrite::new(
            KEYS,
            &key,
            idempotency.clone(),
            context.clone(),
        )];
        if old_plan.is_none() {
            writes.push(StorageWrite::new(
                PLANS,
                &stored.revision_id,
                value.clone(),
                context.clone(),
            ));
        }
        let expectations = vec![
            StorageExpectation::new(KEYS, &key, None),
            StorageExpectation::new(PLANS, &stored.revision_id, old_plan),
        ];
        if storage.put_json_batch(&writes, &expectations)? == ComparePutOutcome::Updated {
            return Ok(stored);
        }
    }
    Err("data_plan_commit_conflict".into())
}

#[cfg(test)]
#[path = "data_plans_tests.rs"]
mod tests;
