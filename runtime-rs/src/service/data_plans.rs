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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_source: Option<super::data_recipes::RecipePlanSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_authentication: BTreeMap<String, super::data_captures::SourceAuthentication>,
    pub scope: PlanScope,
    pub plan: CompiledDataPlan,
    pub package_sha256: BTreeMap<String, String>,
    /// Hashes of non-secret execution context. Older prepared records require
    /// explicit preparation again before execution; legacy checkpoints are untouched.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binding_sha256: BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredDataPlan {
    pub revision_id: String,
    pub revision_sha256: String,
    pub content: DataPlanContent,
}

pub(crate) fn prepare(
    service: &TradeAssemblyService,
    scope: PlanScope,
    mut request: PipelineRequest,
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
    super::data_captures::resolve_parameters(service, &scope, &mut request)?;
    let runtime = service.runtime();
    let mut contracts = Vec::new();
    let mut packages = BTreeMap::new();
    let mut bindings = BTreeMap::new();
    let mut snapshots =
        std::collections::BTreeMap::<String, crate::ports::OperationDataSnapshot>::new();
    for node in &request.nodes {
        let selected = graph
            .nodes
            .iter()
            .find(|n| n.node_id == node.graph_node_id)
            .and_then(|n| n.selected.as_ref())
            .ok_or("data_plan_node_unresolved")?;
        let pinned = super::data_binding::pin_with_package_loader(service, selected, |digest| {
            if !snapshots.contains_key(digest) {
                let operations = request
                    .nodes
                    .iter()
                    .filter_map(|node| {
                        graph
                            .nodes
                            .iter()
                            .find(|entry| entry.node_id == node.graph_node_id)
                            .and_then(|entry| entry.selected.as_ref())
                    })
                    .filter(|entry| entry.plugin_ref == selected.plugin_ref)
                    .map(|entry| entry.operation_id.clone())
                    .collect::<Vec<_>>();
                let snapshot = runtime
                    .plugin_packages
                    .operation_data_snapshot(digest, &operations)?
                    .ok_or("data_plan_package_missing")?;
                snapshots.insert(digest.into(), snapshot);
            }
            Ok(snapshots[digest].package.clone())
        })?;
        let digest = &pinned.package_sha256;
        let contract = snapshots[digest]
            .contracts
            .get(&selected.operation_id)
            .cloned()
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
        bindings.insert(node.graph_node_id.clone(), pinned.context_sha256);
    }
    let plan = compile_data_plan(&graph, request, contracts).map_err(|errors| {
        format!(
            "data_plan_compile_failed:{}",
            serde_json::to_string(&errors).unwrap_or_default()
        )
    })?;
    if let Some(program) = super::data_strategy::program(service, &scope)? {
        program.validate(&plan)?;
    }
    let source_authentication =
        super::data_captures::validate_plan_sources(service, &scope, &graph, &plan)?;
    Ok(DataPlanContent {
        recipe_source: None,
        source_authentication,
        scope,
        plan,
        package_sha256: packages,
        binding_sha256: bindings,
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

pub(super) fn read_owned(
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
    Ok(stored)
}

pub fn current(
    service: &TradeAssemblyService,
    revision_id: &str,
) -> Result<StoredDataPlan, String> {
    let stored = read_owned(service, revision_id)?;
    let fresh = match &stored.content.recipe_source {
        Some(source) => super::data_recipes::prepare(
            service,
            stored.content.scope.clone(),
            source.program.clone(),
        )?,
        None => prepare(
            service,
            stored.content.scope.clone(),
            stored.content.plan.request.clone(),
        )?,
    };
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

pub(crate) fn persist(
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
pub(crate) mod tests;
