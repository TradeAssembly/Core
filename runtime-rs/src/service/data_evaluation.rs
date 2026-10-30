//! Shared event-frame evaluation. Provider acquisition may be batched; transforms
//! receive only one event-time frame and their own previously committed state.
use super::{capability_graph, data_plans, TradeAssemblyService};
use crate::data_pipeline::{canonical_sha256, DataContext, PipelineNode};
use crate::ports::{
    CapabilityGraphResolution, CapabilitySelection, ComparePutOutcome, IdempotencyKey, LeaseClaim,
    PluginOperationRequest, SideEffectContext, StorageExpectation, StorageWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_plugin_sdk::{data_contract::*, data_evaluation::*};

const RUNS: &str = "data_evaluation_checkpoints_v1";
const RECEIPTS: &str = "data_evaluation_receipts_v1";
const SOURCES: &str = "data_evaluation_sources_v1";
const FENCES: &str = "data_evaluation_fences_v1";
const MAX_POINTS: usize = 256;
const MAX_FRAMES: usize = 64;
const MAX_NODES: usize = 32;
const MAX_STEP_BYTES: usize = 8 * 1024 * 1024;
const MAX_STATE_BYTES: usize = 256 * 1024;
const LEASE_MS: i64 = 60_000;
const MAX_STEP_DURATION: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationStep {
    pub run_id: String,
    pub plan_revision_id: String,
    pub window: EvaluationWindow,
    pub expected_checkpoint_sha256: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationCheckpoint {
    pub run_id: String,
    pub plan_revision_id: String,
    pub generation: u64,
    pub cursors: BTreeMap<String, i64>,
    pub states: BTreeMap<String, BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub state_availability: BTreeMap<String, StateAvailability>,
    pub valid_counts: BTreeMap<String, u64>,
    pub checkpoint_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateAvailability {
    pub known_at_ms: i64,
    pub arrived_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationStatus {
    Ready,
    WarmingUp,
    NoNewData,
    WaitingForInput,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationReceipt {
    pub receipt_sha256: String,
    pub request_sha256: String,
    pub status: EvaluationStatus,
    pub checkpoint: EvaluationCheckpoint,
    pub outputs: BTreeMap<String, Vec<DataEvaluationResponse>>,
    pub frames_processed: u64,
}

fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    canonical_sha256(value).map_err(|_| "data_evaluation_numeric_or_encoding_invalid".into())
}
fn bytes<T: Serialize>(value: &T) -> Result<usize, String> {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .map_err(|_| "data_evaluation_encoding_invalid".into())
}
fn key(parts: &[&str]) -> Result<String, String> {
    hash(&parts)
}
fn checkpoint_hash(checkpoint: &EvaluationCheckpoint) -> Result<String, String> {
    let mut value = checkpoint.clone();
    value.checkpoint_sha256.clear();
    hash(&value)
}
fn validate_state_availability(checkpoint: &EvaluationCheckpoint) -> Result<(), String> {
    for node in checkpoint.states.keys() {
        let availability = checkpoint
            .state_availability
            .get(node)
            .ok_or("data_evaluation_state_availability_missing_requires_replay")?;
        if availability.arrived_at_ms < availability.known_at_ms {
            return Err("data_evaluation_state_availability_invalid".into());
        }
    }
    if checkpoint
        .state_availability
        .keys()
        .any(|node| !checkpoint.states.contains_key(node))
    {
        return Err("data_evaluation_state_availability_invalid".into());
    }
    Ok(())
}

fn receipt_hash(receipt: &EvaluationReceipt) -> Result<String, String> {
    let mut value = receipt.clone();
    value.receipt_sha256.clear();
    hash(&value)
}
fn schema_valid(
    service: &TradeAssemblyService,
    package: &str,
    identity: &SchemaIdentity,
    value: &Value,
) -> Result<(), String> {
    let schema = service
        .runtime()
        .plugin_packages
        .data_schema(package, &identity.schema_ref)?
        .ok_or("data_evaluation_schema_missing")?;
    if hash(&schema)? != identity.sha256 {
        return Err("data_evaluation_schema_changed".into());
    }
    let validator =
        jsonschema::validator_for(&schema).map_err(|_| "data_evaluation_schema_invalid")?;
    if !validator.is_valid(value) {
        return Err("data_evaluation_value_schema_invalid".into());
    }
    Ok(())
}
fn schema_values(batch: &DataBatch) -> Result<Value, String> {
    let values = batch
        .points
        .iter()
        .filter_map(|p| match &p.result {
            PointValue::Ready(v) => Some(v.clone()),
            PointValue::WarmingUp => None,
        })
        .collect::<Vec<_>>();
    match batch.shape {
        DataShape::Scalar => {
            if values.len() == 1 {
                Ok(values[0].clone())
            } else {
                Err("data_evaluation_scalar_cardinality_invalid".into())
            }
        }
        _ => Ok(Value::Array(values)),
    }
}
fn allowed_data_operation(
    selection: &CapabilitySelection,
    transform: bool,
    manifest: &Value,
) -> Result<(), String> {
    let operation = &selection.operation;
    if operation.effect != "read"
        || operation.id.starts_with("broker.")
        || operation.capability.starts_with("broker.")
        || operation.apf_action_id.contains("order")
        || operation.finance_resource_type.contains("order")
    {
        return Err("data_evaluation_effect_not_allowed".into());
    }
    if transform
        && (operation.credential_grant_required
            || operation.account_binding_required
            || manifest["permissions"].as_array().is_some_and(|p| {
                p.iter()
                    .any(|p| p == "network.outbound" || p["id"] == "network.outbound")
            }))
    {
        return Err("data_evaluation_transform_privilege_rejected".into());
    }
    Ok(())
}
fn live_lease(service: &TradeAssemblyService, claim: &LeaseClaim) -> Result<(), String> {
    let now = service.runtime().clock.now_ms();
    let current = service
        .runtime()
        .leases
        .current(&claim.resource)?
        .ok_or("data_evaluation_lease_missing")?;
    if now <= 0
        || current.owner != claim.owner
        || current.fencing_token != claim.fencing_token
        || current.expires_at_ms <= now
    {
        return Err("data_evaluation_lease_stale".into());
    }
    Ok(())
}
fn invoke(
    service: &TradeAssemblyService,
    plan: &data_plans::StoredDataPlan,
    node: &PipelineNode,
    selection: &CapabilitySelection,
    payload: &DataEvaluationRequest,
    claim: &mut LeaseClaim,
    context: &SideEffectContext,
) -> Result<DataEvaluationResponse, String> {
    data_plans::current(service, &plan.revision_id)?;
    live_lease(service, claim)?;
    *claim = service
        .runtime()
        .leases
        .renew(claim, service.runtime().clock.now_ms(), LEASE_MS)?;
    if bytes(payload)? > 512 * 1024 {
        return Err("data_evaluation_request_too_large".into());
    }
    let invocation = hash(
        &json!({"plan":plan.revision_sha256,"run":claim.resource,"node":node.graph_node_id,"payload":payload,"authority":context.authority}),
    )?;
    let request = PluginOperationRequest {
        correlation_id: invocation.clone(),
        plugin_instance_ref: selection.plugin_instance_ref.clone(),
        plugin_ref: selection.plugin_ref.clone(),
        manifest_fingerprint: selection.manifest_fingerprint.clone(),
        operation_id: selection.operation_id.clone(),
        capability: selection.operation.capability.clone(),
        capability_graph_revision_id: plan.content.plan.request.capability_graph_revision.clone(),
        capability_graph_fingerprint: plan.content.plan.capability_graph_fingerprint.clone(),
        strategy_id: plan.content.scope.strategy_id.clone(),
        strategy_version_id: plan.content.scope.strategy_version_id.clone(),
        strategy_spec_hash: plan.content.scope.strategy_spec_hash.clone(),
        activation_id: claim.resource.clone(),
        attempt_id: invocation.clone(),
        evaluation_tick_id: hash(&payload.window)?,
        mode: plan.content.plan.request.mode.clone(),
        purpose: selection.operation.purpose.clone(),
        account_ref: selection.account_ref.clone(),
        timeout_ms: 15_000,
        fencing_token: None,
        input: serde_json::to_value(payload).map_err(|_| "data_evaluation_request_invalid")?,
        evidence_refs: vec![format!("data-plan://{}", plan.revision_sha256)],
    };
    let mut call_context = context.clone();
    call_context.idempotency_key = IdempotencyKey::new(format!("data-evaluation:{invocation}"))?;
    let response = service
        .runtime()
        .plugin_operations
        .invoke(&request, &call_context)?;
    if response.reconciliation_required
        || response.schema_ref != EVALUATION_RESPONSE_SCHEMA
        || !response.replayable
        || (plan
            .content
            .plan
            .contracts
            .iter()
            .any(|c| c.graph_node_id == node.graph_node_id && !c.contract.inputs.is_empty())
            && !response.deterministic)
    {
        return Err("data_evaluation_response_contract_invalid".into());
    }
    let mut payload: DataEvaluationResponse =
        serde_json::from_value(response.payload).map_err(|_| "data_evaluation_response_invalid")?;
    for batch in payload.outputs.values_mut() {
        for point in &mut batch.points {
            if let PointValue::Ready(value) = &mut point.result {
                tradeassembly_plugin_sdk::canonical::normalize_numbers(value)
                    .map_err(|_| "data_evaluation_numeric_range_invalid")?;
            }
        }
    }
    for value in payload.state.values_mut() {
        tradeassembly_plugin_sdk::canonical::normalize_numbers(value)
            .map_err(|_| "data_evaluation_numeric_range_invalid")?;
    }
    Ok(payload)
}
fn validate_batch(
    service: &TradeAssemblyService,
    package: &str,
    port: &DataPort,
    context: &DataContext,
    batch: &DataBatch,
    window: &EvaluationWindow,
) -> Result<(), String> {
    if batch.schema != port.schema || batch.shape != port.shape || batch.points.len() > MAX_POINTS {
        return Err("data_evaluation_batch_contract_invalid".into());
    }
    let instruments = context
        .instruments
        .iter()
        .map(|i| i.instrument_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut previous = None;
    let mut identities = BTreeSet::new();
    for point in &batch.points {
        let p = &point.provenance;
        if !instruments.contains(point.instrument_id.as_str())
            || point.source_event_id.is_empty()
            || p.source_revision.is_empty()
            || p.event_start_ms >= p.event_end_ms
            || p.event_start_ms < window.event_start_ms
            || p.event_end_ms > window.event_end_ms
            || p.known_at_ms < p.event_end_ms
            || p.known_at_ms > window.as_of_ms
            || p.arrived_at_ms < p.known_at_ms
            || port.time.interval_ms.is_some_and(|n| {
                p.event_end_ms
                    .checked_sub(p.event_start_ms)
                    .and_then(|v| u64::try_from(v).ok())
                    != Some(n)
            })
            || previous.is_some_and(|prior| prior > (p.event_end_ms, point.instrument_id.as_str()))
            || !identities.insert((p.event_end_ms, point.instrument_id.as_str()))
            || point_sha256(point).map_err(|_| "data_evaluation_point_hash_invalid")?
                != p.content_sha256
        {
            return Err("data_evaluation_event_contract_invalid".into());
        }
        previous = Some((p.event_end_ms, point.instrument_id.as_str()));
    }
    if batch
        .points
        .iter()
        .any(|p| matches!(p.result, PointValue::Ready(_)))
    {
        schema_valid(service, package, &port.schema, &schema_values(batch)?)?;
    }
    Ok(())
}

pub fn step(
    service: &TradeAssemblyService,
    request: EvaluationStep,
    context: &SideEffectContext,
) -> Result<EvaluationReceipt, String> {
    if request.run_id.is_empty()
        || request.run_id.len() > 128
        || request.window.event_start_ms >= request.window.event_end_ms
        || request.window.event_end_ms > request.window.as_of_ms
        || request.window.as_of_ms > service.runtime().clock.now_ms()
    {
        return Err("data_evaluation_request_invalid".into());
    }
    let started = std::time::Instant::now();
    let plan = data_plans::current(service, &request.plan_revision_id)?;
    if plan.content.plan.execution_order.len() > MAX_NODES {
        return Err("data_evaluation_plan_too_large".into());
    }
    let run_key = key(&[&request.run_id, &request.plan_revision_id])?;
    let request_hash = hash(&json!({"request":request,"authority":context.authority}))?;
    let receipt_key = key(&[&run_key, context.idempotency_key.as_str()])?;
    let runtime = service.runtime();
    if let Some(value) = runtime.storage.get_json(RECEIPTS, &receipt_key)? {
        let receipt: EvaluationReceipt =
            serde_json::from_value(value).map_err(|_| "data_evaluation_receipt_invalid")?;
        if receipt.request_sha256 != request_hash {
            return Err("data_evaluation_idempotency_conflict".into());
        }
        if receipt_hash(&receipt)? != receipt.receipt_sha256
            || checkpoint_hash(&receipt.checkpoint)? != receipt.checkpoint.checkpoint_sha256
            || receipt.checkpoint.run_id != request.run_id
            || receipt.checkpoint.plan_revision_id != request.plan_revision_id
        {
            return Err("data_evaluation_receipt_invalid".into());
        }
        validate_state_availability(&receipt.checkpoint)?;
        return Ok(receipt);
    }
    let prior = runtime.storage.get_json(RUNS, &run_key)?;
    let mut checkpoint: EvaluationCheckpoint = match &prior {
        Some(v) => {
            serde_json::from_value(v.clone()).map_err(|_| "data_evaluation_checkpoint_invalid")?
        }
        None => EvaluationCheckpoint {
            run_id: request.run_id.clone(),
            plan_revision_id: request.plan_revision_id.clone(),
            generation: 0,
            cursors: BTreeMap::new(),
            states: BTreeMap::new(),
            state_availability: BTreeMap::new(),
            valid_counts: BTreeMap::new(),
            checkpoint_sha256: String::new(),
        },
    };
    if prior.is_some()
        && (checkpoint_hash(&checkpoint)? != checkpoint.checkpoint_sha256
            || checkpoint.run_id != request.run_id
            || checkpoint.plan_revision_id != request.plan_revision_id)
    {
        return Err("data_evaluation_checkpoint_invalid".into());
    }
    validate_state_availability(&checkpoint)?;
    let prior_hash = prior.as_ref().map(|_| checkpoint.checkpoint_sha256.clone());
    if prior_hash != request.expected_checkpoint_sha256 {
        return Err("data_evaluation_checkpoint_conflict".into());
    }
    let resource = format!("data-evaluation:{run_key}");
    let owner = format!("{}:{}", context.authority.actor, receipt_key);
    let mut claim = runtime
        .leases
        .acquire(&resource, &owner, runtime.clock.now_ms(), LEASE_MS)?
        .ok_or("data_evaluation_busy")?;
    let result = (|| {
        let old_fence = runtime.storage.get_json(FENCES, &run_key)?;
        let fence = json!({"owner":claim.owner,"fencingToken":claim.fencing_token});
        if runtime.storage.put_json_batch(
            &[StorageWrite::new(
                FENCES,
                &run_key,
                fence.clone(),
                context.clone(),
            )],
            &[StorageExpectation::new(FENCES, &run_key, old_fence)],
        )? != ComparePutOutcome::Updated
        {
            return Err("data_evaluation_fence_conflict".into());
        }
        StepEnvironment {
            service,
            plan: &plan,
            request: &request,
            context,
            run_key,
            prior,
            fence,
            receipt_key,
            request_hash,
            started,
        }
        .evaluate(&mut checkpoint, &mut claim)
    })();
    let _ = runtime.leases.release(&claim);
    result
}

struct TransformFrame<'a> {
    graph: &'a CapabilityGraphResolution,
    node: &'a PipelineNode,
    contract: &'a OperationDataContract,
    package: &'a str,
    time: i64,
    values: &'a BTreeMap<String, DataEvaluationResponse>,
}

struct StepEnvironment<'a> {
    service: &'a TradeAssemblyService,
    plan: &'a data_plans::StoredDataPlan,
    request: &'a EvaluationStep,
    context: &'a SideEffectContext,
    run_key: String,
    prior: Option<Value>,
    fence: Value,
    receipt_key: String,
    request_hash: String,
    started: std::time::Instant,
}
impl StepEnvironment<'_> {
    fn check_budget(&self) -> Result<(), String> {
        if self.started.elapsed() >= MAX_STEP_DURATION {
            return Err("data_evaluation_deadline_exceeded".into());
        }
        Ok(())
    }
    fn graph(&self) -> Result<CapabilityGraphResolution, String> {
        let revision = capability_graph::get_revision(
            self.service,
            &self.plan.content.plan.request.capability_graph_revision,
        );
        serde_json::from_value(revision["revision"]["graph"].clone())
            .map_err(|_| "data_evaluation_graph_invalid".into())
    }
    fn evaluate(
        &self,
        checkpoint: &mut EvaluationCheckpoint,
        claim: &mut LeaseClaim,
    ) -> Result<EvaluationReceipt, String> {
        let graph = self.graph()?;
        let plan = &self.plan.content.plan;
        let mut roots: BTreeMap<(String, String), DataBatch> = BTreeMap::new();
        let mut step_bytes = 0;
        let mut pending_source = false;
        for node in &plan.request.nodes {
            self.check_budget()?;
            let contract = &plan
                .contracts
                .iter()
                .find(|c| c.graph_node_id == node.graph_node_id)
                .ok_or("data_evaluation_contract_missing")?
                .contract;
            let selected = graph
                .nodes
                .iter()
                .find(|n| n.node_id == node.graph_node_id)
                .and_then(|n| n.selected.as_ref())
                .ok_or("data_evaluation_node_unresolved")?;
            let package = self
                .service
                .runtime()
                .plugin_packages
                .get(&self.plan.content.package_sha256[&node.graph_node_id])?
                .ok_or("data_evaluation_package_missing")?;
            allowed_data_operation(selected, !contract.inputs.is_empty(), &package.manifest)?;
            if !selected
                .operation
                .traits
                .output_schema_refs
                .iter()
                .any(|r| r == EVALUATION_RESPONSE_SCHEMA)
            {
                return Err("data_evaluation_protocol_not_declared".into());
            }
            if !contract.inputs.is_empty() {
                continue;
            }
            if contract.state.is_some() {
                return Err("data_evaluation_provider_state_unsupported".into());
            }
            let payload = DataEvaluationRequest {
                schema_version: EvaluationVersion::V1,
                plan_sha256: plan.plan_sha256.clone(),
                node_id: node.graph_node_id.clone(),
                semantics_version: contract.semantics_version.clone(),
                instruments: node
                    .context
                    .instruments
                    .iter()
                    .map(|i| i.instrument_id.clone())
                    .collect(),
                window: self.request.window.clone(),
                parameters: node.parameters.clone(),
                inputs: BTreeMap::new(),
                prior_state: BTreeMap::new(),
                max_points: MAX_POINTS as u64,
            };
            let response = invoke(
                self.service,
                self.plan,
                node,
                selected,
                &payload,
                claim,
                self.context,
            )?;
            self.check_budget()?;
            if !response.state.is_empty() || response.outputs.len() != contract.outputs.len() {
                return Err("data_evaluation_provider_response_invalid".into());
            }
            step_bytes += bytes(&response)?;
            for (port, batch) in response.outputs {
                let declaration = contract
                    .outputs
                    .get(&port)
                    .ok_or("data_evaluation_output_undeclared")?;
                validate_batch(
                    self.service,
                    &package.package_sha256,
                    declaration,
                    &node.context,
                    &batch,
                    &self.request.window,
                )?;
                let mut new = batch.clone();
                new.points.clear();
                let mut positions = checkpoint.cursors.clone();
                for point in batch.points {
                    if !point.provenance.completed {
                        pending_source = true;
                        continue;
                    }
                    let cursor = key(&[&node.graph_node_id, &port, &point.instrument_id])?;
                    let source_key = key(&[
                        &self.run_key,
                        &node.graph_node_id,
                        &port,
                        &point.instrument_id,
                        &point.provenance.event_start_ms.to_string(),
                    ])?;
                    if let Some(old) = self
                        .service
                        .runtime()
                        .storage
                        .get_json(SOURCES, &source_key)?
                    {
                        if old["contentSha256"] != point.provenance.content_sha256 {
                            return Err("data_evaluation_revision_requires_replay".into());
                        }
                        continue;
                    }
                    if checkpoint
                        .cursors
                        .get(&cursor)
                        .is_some_and(|end| *end >= point.provenance.event_end_ms)
                    {
                        return Err("data_evaluation_out_of_order".into());
                    }
                    if positions
                        .get(&cursor)
                        .copied()
                        .unwrap_or(self.request.window.event_start_ms)
                        != point.provenance.event_start_ms
                    {
                        return Err("data_evaluation_gap_requires_policy".into());
                    }
                    positions.insert(cursor, point.provenance.event_end_ms);
                    new.points.push(point);
                }
                roots.insert((node.graph_node_id.clone(), port), new);
            }
        }
        if roots.is_empty() || step_bytes > MAX_STEP_BYTES {
            return Err("data_evaluation_source_or_budget_invalid".into());
        }
        // A frame barrier prevents one source advancing beyond another. A missing
        // member remains waiting; the host does not fill, forward-fill or drop it.
        let mut frames: Option<BTreeSet<i64>> = None;
        let mut any_points = pending_source;
        for ((node_id, _), batch) in &roots {
            any_points |= !batch.points.is_empty();
            let node = plan
                .request
                .nodes
                .iter()
                .find(|n| &n.graph_node_id == node_id)
                .unwrap();
            let mut groups: BTreeMap<i64, BTreeSet<&str>> = BTreeMap::new();
            for point in &batch.points {
                groups
                    .entry(point.provenance.event_end_ms)
                    .or_default()
                    .insert(&point.instrument_id);
            }
            let ready = groups
                .into_iter()
                .filter(|(_, ids)| {
                    node.context
                        .instruments
                        .iter()
                        .all(|i| ids.contains(i.instrument_id.as_str()))
                })
                .map(|(t, _)| t)
                .collect::<BTreeSet<_>>();
            frames = Some(match frames {
                None => ready,
                Some(before) => before.intersection(&ready).copied().collect(),
            });
        }
        let frames = frames.unwrap_or_default();
        // Require a common complete prefix. Never skip an unmatched earlier frame.
        let first_missing = roots
            .values()
            .flat_map(|b| b.points.iter().map(|p| p.provenance.event_end_ms))
            .filter(|t| !frames.contains(t))
            .min();
        let frames = frames
            .into_iter()
            .filter(|t| first_missing.is_none_or(|missing| *t < missing))
            .take(MAX_FRAMES)
            .collect::<Vec<_>>();
        let mut outputs: BTreeMap<String, Vec<DataEvaluationResponse>> = BTreeMap::new();
        let mut source_writes = BTreeMap::new();
        let mut any_ready = false;
        for frame in &frames {
            self.check_budget()?;
            let mut values: BTreeMap<String, DataEvaluationResponse> = BTreeMap::new();
            for node_id in &plan.execution_order {
                let node = plan
                    .request
                    .nodes
                    .iter()
                    .find(|n| &n.graph_node_id == node_id)
                    .unwrap();
                let contract = &plan
                    .contracts
                    .iter()
                    .find(|c| &c.graph_node_id == node_id)
                    .unwrap()
                    .contract;
                let package = &self.plan.content.package_sha256[node_id];
                let response = if contract.inputs.is_empty() {
                    let mut response = DataEvaluationResponse {
                        schema_version: EvaluationVersion::V1,
                        outputs: BTreeMap::new(),
                        state: BTreeMap::new(),
                    };
                    for (port, declaration) in &contract.outputs {
                        let points = roots[&(node_id.clone(), port.clone())]
                            .points
                            .iter()
                            .filter(|p| p.provenance.event_end_ms == *frame)
                            .cloned()
                            .collect::<Vec<_>>();
                        for point in &points {
                            let cursor = key(&[node_id, port, &point.instrument_id])?;
                            checkpoint
                                .cursors
                                .insert(cursor, point.provenance.event_end_ms);
                            let index = key(&[
                                &self.run_key,
                                node_id,
                                port,
                                &point.instrument_id,
                                &point.provenance.event_start_ms.to_string(),
                            ])?;
                            source_writes.insert(index,json!({"contentSha256":point.provenance.content_sha256,"sourceEventId":point.source_event_id,"sourceRevision":point.provenance.source_revision}));
                        }
                        response.outputs.insert(
                            port.clone(),
                            DataBatch {
                                schema: declaration.schema.clone(),
                                shape: declaration.shape.clone(),
                                points,
                            },
                        );
                    }
                    response
                } else {
                    self.transform(
                        TransformFrame {
                            graph: &graph,
                            node,
                            contract,
                            package,
                            time: *frame,
                            values: &values,
                        },
                        checkpoint,
                        claim,
                    )?
                };
                step_bytes += bytes(&response)?;
                if step_bytes > MAX_STEP_BYTES {
                    return Err("data_evaluation_step_too_large".into());
                }
                any_ready |= response
                    .outputs
                    .values()
                    .flat_map(|b| &b.points)
                    .any(|p| matches!(p.result, PointValue::Ready(_)));
                values.insert(node_id.clone(), response.clone());
                outputs.entry(node_id.clone()).or_default().push(response);
            }
        }
        // Readiness is determined by terminal operations, not ready provider rows.
        let consumers = plan
            .request
            .bindings
            .iter()
            .map(|b| b.producer_node.as_str())
            .collect::<BTreeSet<_>>();
        if !frames.is_empty() {
            any_ready = outputs
                .iter()
                .filter(|(id, _)| !consumers.contains(id.as_str()))
                .flat_map(|(_, r)| r)
                .flat_map(|r| r.outputs.values())
                .flat_map(|b| &b.points)
                .any(|p| matches!(p.result, PointValue::Ready(_)));
        }
        let status = if frames.is_empty() {
            if any_points {
                EvaluationStatus::WaitingForInput
            } else {
                EvaluationStatus::NoNewData
            }
        } else if any_ready {
            EvaluationStatus::Ready
        } else {
            EvaluationStatus::WarmingUp
        };
        if !frames.is_empty() {
            checkpoint.generation = checkpoint
                .generation
                .checked_add(1)
                .ok_or("data_evaluation_generation_overflow")?;
        }
        checkpoint.checkpoint_sha256 = checkpoint_hash(checkpoint)?;
        let mut receipt = EvaluationReceipt {
            receipt_sha256: String::new(),
            request_sha256: self.request_hash.clone(),
            status,
            checkpoint: checkpoint.clone(),
            outputs,
            frames_processed: frames.len() as u64,
        };
        receipt.receipt_sha256 = receipt_hash(&receipt)?;
        self.check_budget()?;
        data_plans::current(self.service, &self.plan.revision_id)?;
        live_lease(self.service, claim)?;
        let mut writes = vec![
            StorageWrite::new(
                RUNS,
                &self.run_key,
                serde_json::to_value(checkpoint)
                    .map_err(|_| "data_evaluation_checkpoint_invalid")?,
                self.context.clone(),
            ),
            StorageWrite::new(
                RECEIPTS,
                &self.receipt_key,
                serde_json::to_value(&receipt).map_err(|_| "data_evaluation_receipt_invalid")?,
                self.context.clone(),
            ),
        ];
        let mut expectations = vec![
            StorageExpectation::new(RUNS, &self.run_key, self.prior.clone()),
            StorageExpectation::new(RECEIPTS, &self.receipt_key, None),
            StorageExpectation::new(FENCES, &self.run_key, Some(self.fence.clone())),
        ];
        for (id, value) in source_writes {
            expectations.push(StorageExpectation::new(SOURCES, &id, None));
            writes.push(StorageWrite::new(SOURCES, id, value, self.context.clone()));
        }
        if self
            .service
            .runtime()
            .storage
            .put_json_batch(&writes, &expectations)?
            != ComparePutOutcome::Updated
        {
            return Err("data_evaluation_commit_conflict".into());
        }
        Ok(receipt)
    }

    fn transform(
        &self,
        frame: TransformFrame<'_>,
        checkpoint: &mut EvaluationCheckpoint,
        claim: &mut LeaseClaim,
    ) -> Result<DataEvaluationResponse, String> {
        self.check_budget()?;
        let TransformFrame {
            graph,
            node,
            contract,
            package,
            time: frame,
            values,
        } = frame;
        let plan = &self.plan.content.plan;
        let mut inputs = BTreeMap::new();
        let mut ready = true;
        let mut lineage = BTreeSet::new();
        let mut known = i64::MIN;
        let mut arrived = i64::MIN;
        for (port, input) in &contract.inputs {
            let Some(binding) = plan
                .request
                .bindings
                .iter()
                .find(|b| b.consumer_node == node.graph_node_id && b.input_port == *port)
            else {
                if input.required {
                    return Err("data_evaluation_required_input_missing".into());
                }
                continue;
            };
            let batch = values
                .get(&binding.producer_node)
                .and_then(|r| r.outputs.get(&binding.output_port))
                .ok_or("data_evaluation_input_missing")?;
            for point in &batch.points {
                lineage.insert(point.provenance.content_sha256.clone());
                known = known.max(point.provenance.known_at_ms);
                arrived = arrived.max(point.provenance.arrived_at_ms);
            }
            let input_ready = batch
                .points
                .iter()
                .all(|p| matches!(p.result, PointValue::Ready(_)))
                && !batch.points.is_empty();
            if input.required && !input_ready {
                ready = false;
            }
            inputs.insert(port.clone(), batch.clone());
        }
        let prior_state = checkpoint
            .states
            .get(&node.graph_node_id)
            .cloned()
            .unwrap_or_default();
        // Host readiness counters are state too, even for a stateless plugin.
        if checkpoint.states.contains_key(&node.graph_node_id) {
            let availability = checkpoint
                .state_availability
                .get(&node.graph_node_id)
                .ok_or("data_evaluation_state_availability_missing_requires_replay")?;
            if availability.known_at_ms > self.request.window.as_of_ms {
                return Err("data_evaluation_state_not_available_at_cutoff".into());
            }
            known = known.max(availability.known_at_ms);
            arrived = arrived.max(availability.arrived_at_ms);
        }
        lineage.insert(hash(&prior_state)?);
        let interval = node
            .context
            .interval_ms
            .ok_or("data_evaluation_event_time_operation_unsupported")?;
        let start = frame
            .checked_sub(i64::try_from(interval).map_err(|_| "data_evaluation_interval_invalid")?)
            .ok_or("data_evaluation_interval_invalid")?;
        let window = EvaluationWindow {
            event_start_ms: start,
            event_end_ms: frame,
            as_of_ms: known,
        };
        let mut response = if ready {
            for (port, input) in &contract.inputs {
                if input.required {
                    for point in &inputs[port].points {
                        let key = key(&[&node.graph_node_id, port, &point.instrument_id])?;
                        let count = checkpoint.valid_counts.entry(key).or_default();
                        *count = count
                            .checked_add(1)
                            .ok_or("data_evaluation_history_overflow")?;
                    }
                }
            }
            let selected = graph
                .nodes
                .iter()
                .find(|n| n.node_id == node.graph_node_id)
                .and_then(|n| n.selected.as_ref())
                .ok_or("data_evaluation_node_unresolved")?;
            invoke(
                self.service,
                self.plan,
                node,
                selected,
                &DataEvaluationRequest {
                    schema_version: EvaluationVersion::V1,
                    plan_sha256: plan.plan_sha256.clone(),
                    node_id: node.graph_node_id.clone(),
                    semantics_version: contract.semantics_version.clone(),
                    instruments: node
                        .context
                        .instruments
                        .iter()
                        .map(|i| i.instrument_id.clone())
                        .collect(),
                    window: window.clone(),
                    parameters: node.parameters.clone(),
                    inputs,
                    prior_state,
                    max_points: MAX_POINTS as u64,
                },
                claim,
                self.context,
            )?
        } else {
            DataEvaluationResponse {
                schema_version: EvaluationVersion::V1,
                state: prior_state,
                outputs: contract
                    .outputs
                    .iter()
                    .map(|(name, port)| {
                        (
                            name.clone(),
                            DataBatch {
                                schema: port.schema.clone(),
                                shape: port.shape.clone(),
                                points: node
                                    .context
                                    .instruments
                                    .iter()
                                    .map(|i| DataPoint {
                                        instrument_id: i.instrument_id.clone(),
                                        source_event_id: String::new(),
                                        provenance: BatchProvenance {
                                            content_sha256: String::new(),
                                            upstream_content_sha256: vec![],
                                            event_start_ms: start,
                                            event_end_ms: frame,
                                            known_at_ms: known,
                                            arrived_at_ms: arrived,
                                            completed: true,
                                            source_revision: contract.semantics_version.clone(),
                                        },
                                        result: PointValue::WarmingUp,
                                    })
                                    .collect(),
                            },
                        )
                    })
                    .collect(),
            }
        };
        if response.outputs.len() != contract.outputs.len() {
            return Err("data_evaluation_output_cardinality_invalid".into());
        }
        let minimum = match &contract.warmup.observations_parameter {
            Some(p) => node.parameters[p]
                .as_u64()
                .ok_or("data_evaluation_warmup_invalid")?
                .checked_add(contract.warmup.additional_observations)
                .ok_or("data_evaluation_history_overflow")?,
            None => contract.warmup.additional_observations,
        }
        .max(contract.warmup.minimum_observations);
        let warmed = contract
            .inputs
            .iter()
            .filter(|(_, input)| input.required)
            .all(|(port, _)| {
                node.input_contexts
                    .get(port)
                    .unwrap_or(&node.context)
                    .instruments
                    .iter()
                    .all(|i| {
                        key(&[&node.graph_node_id, port, &i.instrument_id])
                            .ok()
                            .and_then(|k| checkpoint.valid_counts.get(&k))
                            .is_some_and(|n| *n >= minimum)
                    })
            });
        for (port, batch) in &mut response.outputs {
            if batch.points.len() != node.context.instruments.len() {
                return Err("data_evaluation_output_cardinality_invalid".into());
            }
            for point in &mut batch.points {
                if point.provenance.event_start_ms != start
                    || point.provenance.event_end_ms != frame
                    || (!ready || !warmed) && matches!(point.result, PointValue::Ready(_))
                {
                    return Err("data_evaluation_premature_output".into());
                }
                point.source_event_id = format!(
                    "derived:{}",
                    key(&[
                        &plan.plan_sha256,
                        &node.graph_node_id,
                        port,
                        &point.instrument_id,
                        &frame.to_string()
                    ])?
                );
                point.provenance = BatchProvenance {
                    content_sha256: String::new(),
                    upstream_content_sha256: lineage.iter().cloned().collect(),
                    event_start_ms: start,
                    event_end_ms: frame,
                    known_at_ms: known,
                    arrived_at_ms: arrived,
                    completed: true,
                    source_revision: contract.semantics_version.clone(),
                };
                point.provenance.content_sha256 =
                    point_sha256(point).map_err(|_| "data_evaluation_output_hash_invalid")?;
            }
            validate_batch(
                self.service,
                package,
                contract
                    .outputs
                    .get(port)
                    .ok_or("data_evaluation_output_undeclared")?,
                &node.context,
                batch,
                &window,
            )?;
        }
        if bytes(&response.state)? > MAX_STATE_BYTES {
            return Err("data_evaluation_state_too_large".into());
        }
        match &contract.state {
            None => {
                if !response.state.is_empty() {
                    return Err("data_evaluation_state_undeclared".into());
                }
            }
            Some(state) => {
                let expected = match state.partition {
                    StatePartition::Instrument => node
                        .context
                        .instruments
                        .iter()
                        .map(|i| i.instrument_id.clone())
                        .collect::<BTreeSet<_>>(),
                    StatePartition::ResearchRun
                        if plan.request.mode == "backtest" || plan.request.mode == "research" =>
                    {
                        BTreeSet::from([RESEARCH_RUN_STATE_SLOT.to_owned()])
                    }
                    StatePartition::Activation => {
                        return Err("data_evaluation_activation_state_not_integrated".into())
                    }
                    _ => return Err("data_evaluation_state_partition_invalid".into()),
                };
                if ready && response.state.keys().cloned().collect::<BTreeSet<_>>() != expected {
                    return Err("data_evaluation_state_partition_invalid".into());
                }
                for value in response.state.values() {
                    schema_valid(self.service, package, &state.schema, value)?;
                }
            }
        }
        // Include the availability of the decision to process/wait as well as
        // plugin state. Unchanged/empty state must not lose host warmup causality.
        checkpoint.state_availability.insert(
            node.graph_node_id.clone(),
            StateAvailability {
                known_at_ms: known,
                arrived_at_ms: arrived,
            },
        );
        checkpoint
            .states
            .insert(node.graph_node_id.clone(), response.state.clone());
        Ok(response)
    }
}

#[cfg(test)]
#[path = "data_evaluation_tests.rs"]
mod tests;
