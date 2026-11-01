//! Host-witnessed acquisition and bounded replay receipts. No caller-supplied
//! response is admitted, and a receipt grants no execution authority.
use super::{
    capability_graph,
    data_binding::{self, PinnedOperation},
    data_evaluation,
    data_plans::PlanScope,
    ServiceResponse, TradeAssemblyService,
};
use crate::{
    data_pipeline::{canonical_sha256, DataContext, PipelineNode},
    ports::*,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_plugin_sdk::{data_evaluation::*, data_history::VersionedOperationData};
const RECEIPTS: &str = "data_acquisition_receipts_v1";
const KEYS: &str = "data_acquisition_keys_v1";
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_POINTS: u64 = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapPolicy {
    StrictFixedInterval,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturePolicy {
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub mode: String,
    pub acquisition_node: String,
    pub replay_node: String,
    /// Parameter name -> RFC 6901 pointer in the ACTUAL acquisition payload.
    pub parameter_extracts: BTreeMap<String, String>,
    pub context: DataContext,
    pub event_start_ms: i64,
    pub event_end_ms: i64,
    pub gap_policy: GapPolicy,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_version: Option<CaptureReceiptVersion>,
    pub policy: CapturePolicy,
    pub input: Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureReceiptVersion {
    #[serde(rename = "tradeassembly.data_capture_receipt.v1")]
    V1,
    #[serde(rename = "tradeassembly.data_capture_receipt.v2")]
    V2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputWitnessVersion {
    #[serde(rename = "tradeassembly.data_capture_input_witness.v1")]
    V1,
}
/// Protected actual nonsecret input, created by the host before acquisition.
/// It is never accepted as a caller-provided receipt or provider response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputWitness {
    pub schema_version: InputWitnessVersion,
    pub input: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptureReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_witness: Option<InputWitness>,
    pub schema_version: CaptureReceiptVersion,
    pub capture_id: String,
    pub receipt_sha256: String,
    pub policy: CapturePolicy,
    pub input_sha256: String,
    pub acquisition: PinnedOperation,
    pub replay: PinnedOperation,
    pub replay_contract_sha256: String,
    pub acquisition_response: PluginOperationResponse,
    pub payload_sha256: String,
    pub replay_parameters: Value,
    pub replay_response: DataEvaluationResponse,
    pub replay_as_of_ms: i64,
    pub replay_observed_at_ms: i64,
    pub replay_payload_sha256: String,
    pub replay_content_hash: String,
    pub replay_source_event_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceAuthentication {
    UnverifiedInput,
    HostCaptureStream {
        stream_id: String,
        binding_sha256: String,
    },
    HostAcquisition {
        capture_id: String,
        receipt_sha256: String,
        payload_sha256: String,
    },
}
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    canonical_sha256(value).map_err(|_| "data_capture_encoding_invalid".into())
}
fn bounded<T: Serialize>(value: &T, limit: usize) -> Result<(), String> {
    if serde_json::to_vec(value)
        .map_err(|_| "data_capture_encoding_invalid")?
        .len()
        > limit
    {
        return Err("data_capture_too_large".into());
    }
    Ok(())
}
pub(crate) fn capturable_payload(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let normalized = key
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if matches!(
                    normalized.as_str(),
                    "headers"
                        | "requestheaders"
                        | "responseheaders"
                        | "ephemeralcredentialgrant"
                        | "credentials"
                        | "authorization"
                        | "cookie"
                        | "setcookie"
                ) {
                    return Err("data_capture_transport_material_forbidden".into());
                }
                capturable_payload(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                capturable_payload(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn identity(receipt: &CaptureReceipt) -> Result<String, String> {
    let mut r = receipt.clone();
    r.capture_id.clear();
    r.receipt_sha256.clear();
    hash(&r)
}
fn check_v2_input(input: &Value) -> Result<(), String> {
    if !input.is_object() {
        return Err("data_capture_input_object_required".into());
    }
    bounded(input, 65_536)?;
    tradeassembly_plugin_sdk::data_package::metadata_budget(input)?;
    capturable_payload(input)?;
    nonsecret_witness_input(input)
}
// V2 retains the actual query, unlike V1's input hash. Refuse credential fields
// recursively before acquisition or durable command admission. Keep the legacy
// payload validator unchanged for existing V1 requests and receipts.
fn nonsecret_witness_input(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                let normalized: String = key
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect();
                if matches!(
                    normalized.as_str(),
                    "apikey"
                        | "apisecret"
                        | "password"
                        | "passwd"
                        | "passphrase"
                        | "accesstoken"
                        | "refreshtoken"
                        | "idtoken"
                        | "authtoken"
                        | "bearertoken"
                        | "sessiontoken"
                        | "privatekey"
                        | "secretkey"
                        | "clientsecret"
                        | "credential"
                        | "secret"
                        | "secrets"
                ) {
                    return Err("data_capture_input_credential_material_forbidden".into());
                }
                nonsecret_witness_input(child)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                nonsecret_witness_input(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn command_identity(
    policy: &CapturePolicy,
    input_sha256: &str,
    requested: Option<CaptureReceiptVersion>,
) -> Result<String, String> {
    if requested == Some(CaptureReceiptVersion::V2) {
        hash(&(policy, input_sha256, CaptureReceiptVersion::V2))
    } else {
        hash(&(policy, input_sha256))
    }
}
fn validate_receipt_input(receipt: &CaptureReceipt) -> Result<(), String> {
    validate_witness(
        receipt.schema_version,
        &receipt.input_sha256,
        receipt.input_witness.as_ref(),
    )
}
fn validate_witness(
    version: CaptureReceiptVersion,
    input_sha256: &str,
    witness: Option<&InputWitness>,
) -> Result<(), String> {
    match (version, witness) {
        (CaptureReceiptVersion::V1, None) => Ok(()),
        (CaptureReceiptVersion::V2, Some(witness)) => {
            check_v2_input(&witness.input)?;
            if hash(&witness.input)? != input_sha256 {
                return Err("data_capture_input_witness_hash_mismatch".into());
            }
            Ok(())
        }
        _ => Err("data_capture_input_witness_version_mismatch".into()),
    }
}
pub(crate) fn verified_input_witness(receipt: &CaptureReceipt) -> Result<&InputWitness, String> {
    validate_receipt_input(receipt)?;
    if receipt.schema_version != CaptureReceiptVersion::V2 {
        return Err("data_capture_v2_input_witness_required".into());
    }
    receipt
        .input_witness
        .as_ref()
        .ok_or_else(|| "data_capture_v2_input_witness_required".into())
}
fn check_policy(policy: &CapturePolicy) -> Result<u64, String> {
    bounded(policy, 65536)?;
    let interval = policy
        .context
        .interval_ms
        .filter(|n| *n > 0 && *n <= i64::MAX as u64)
        .ok_or("data_capture_interval_required")?;
    let duration = policy
        .event_end_ms
        .checked_sub(policy.event_start_ms)
        .filter(|d| *d > 0)
        .ok_or("data_capture_window_invalid")? as u64;
    let instruments = policy
        .context
        .instruments
        .iter()
        .map(|i| i.instrument_id.as_str())
        .collect::<BTreeSet<_>>();
    if instruments.is_empty()
        || instruments.len() != policy.context.instruments.len()
        || instruments.iter().any(|i| i.is_empty())
        || !duration.is_multiple_of(interval)
        || !matches!(policy.mode.as_str(), "paper" | "backtest" | "live")
        || policy.parameter_extracts.is_empty()
        || policy.parameter_extracts.len() > 32
    {
        return Err("data_capture_policy_invalid".into());
    }
    for (name, pointer) in &policy.parameter_extracts {
        if name.is_empty()
            || name.len() > 128
            || !pointer.starts_with('/')
            || pointer.len() > 1024
            || pointer.split('/').skip(1).any(|s| {
                let mut chars = s.chars();
                while let Some(c) = chars.next() {
                    if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                        return true;
                    }
                }
                false
            })
        {
            return Err("data_capture_parameter_pointer_invalid".into());
        }
    }
    let points = (duration / interval)
        .checked_mul(instruments.len() as u64)
        .filter(|n| *n <= MAX_POINTS)
        .ok_or("data_capture_coverage_budget_exceeded")?;
    Ok(points)
}
fn selection<'a>(
    graph: &'a CapabilityGraphResolution,
    node: &str,
) -> Result<&'a CapabilitySelection, String> {
    graph
        .nodes
        .iter()
        .find(|n| n.node_id == node)
        .and_then(|n| n.selected.as_ref())
        .ok_or_else(|| "data_capture_node_unresolved".into())
}
fn resolve(
    service: &TradeAssemblyService,
    policy: &CapturePolicy,
) -> Result<CapabilityGraphResolution, String> {
    check_policy(policy)?;
    service
        .require_object("strategy", &policy.scope.strategy_id)
        .map_err(|_| "data_capture_not_available")?;
    let now = chrono::DateTime::from_timestamp_millis(service.runtime().clock.now_ms())
        .ok_or("data_capture_clock_invalid")?
        .to_rfc3339();
    let revision = capability_graph::revision_for_context(
        service,
        &policy.capability_graph_revision,
        &policy.scope.strategy_id,
        &policy.scope.strategy_version_id,
        &policy.mode,
        &now,
    )
    .map_err(|_| "data_capture_graph_stale")?;
    if revision["strategySpecHash"] != policy.scope.strategy_spec_hash {
        return Err("data_capture_strategy_mismatch".into());
    }
    serde_json::from_value(revision["graph"].clone())
        .map_err(|_| "data_capture_graph_invalid".into())
}
fn extract(policy: &CapturePolicy, payload: &Value) -> Result<Value, String> {
    let mut parameters = serde_json::Map::new();
    for (name, pointer) in &policy.parameter_extracts {
        parameters.insert(
            name.clone(),
            payload
                .pointer(pointer)
                .ok_or("data_capture_parameter_missing")?
                .clone(),
        );
    }
    let mut result = Value::Object(parameters);
    tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut result)
        .map_err(|_| "data_capture_numeric_range_invalid")?;
    Ok(result)
}
fn contract(
    service: &TradeAssemblyService,
    pin: &PinnedOperation,
) -> Result<VersionedOperationData, String> {
    let c = service
        .runtime()
        .plugin_packages
        .operation_data(&pin.package_sha256, &pin.operation_id)?
        .ok_or("data_capture_replay_contract_missing")?;
    if !c.inputs.is_empty() || c.state.is_some() {
        return Err("data_capture_stateless_source_required".into());
    }
    Ok(c)
}
fn invoke(
    service: &TradeAssemblyService,
    policy: &CapturePolicy,
    graph: &CapabilityGraphResolution,
    node: &str,
    input: Value,
    ctx: &SideEffectContext,
    phase: &str,
) -> Result<PluginOperationResponse, String> {
    let selected = selection(graph, node)?;
    let pin = data_binding::pin(service, selected)?;
    let package = service
        .runtime()
        .plugin_packages
        .get(&pin.package_sha256)?
        .ok_or("data_capture_package_missing")?;
    data_evaluation::allowed_data_operation(selected, false, &package.manifest)?;
    if !selected.operation.traits.modes.contains(&policy.mode)
        || policy.context.instruments.iter().any(|i| {
            !selected
                .operation
                .traits
                .instrument_families
                .contains(&i.instrument_family)
        })
    {
        return Err("data_capture_context_unsupported".into());
    }
    let correlation = hash(
        &json!({"phase":phase,"policy":policy,"input":input,"authority":ctx.authority,"key":ctx.idempotency_key.as_str()}),
    )?;
    let request = PluginOperationRequest {
        correlation_id: correlation.clone(),
        plugin_instance_ref: selected.plugin_instance_ref.clone(),
        plugin_ref: selected.plugin_ref.clone(),
        manifest_fingerprint: selected.manifest_fingerprint.clone(),
        operation_id: selected.operation_id.clone(),
        capability: selected.operation.capability.clone(),
        capability_graph_revision_id: policy.capability_graph_revision.clone(),
        capability_graph_fingerprint: graph.graph_fingerprint.clone(),
        strategy_id: policy.scope.strategy_id.clone(),
        strategy_version_id: policy.scope.strategy_version_id.clone(),
        strategy_spec_hash: policy.scope.strategy_spec_hash.clone(),
        activation_id: String::new(),
        attempt_id: correlation.clone(),
        evaluation_tick_id: correlation.clone(),
        mode: policy.mode.clone(),
        purpose: selected.operation.purpose.clone(),
        account_ref: selected.account_ref.clone(),
        timeout_ms: 15000,
        fencing_token: None,
        input,
        evidence_refs: vec![],
    };
    let mut call = ctx.clone();
    call.idempotency_key = IdempotencyKey::new(format!("data-capture:{correlation}"))?;
    let response = service
        .runtime()
        .plugin_operations
        .invoke(&request, &call)?;
    bounded(&response, MAX_BYTES)?;
    tradeassembly_plugin_sdk::data_package::metadata_budget(&response.payload)?;
    capturable_payload(&response.payload)?;
    if response.correlation_id != correlation
        || response.reconciliation_required
        || response.freshness_state != "fresh"
        || response.source_event_id.is_empty()
        || response.content_hash.is_empty()
        || response.observed_at_ms <= 0
        || response.observed_at_ms > service.runtime().clock.now_ms()
        || !selected
            .operation
            .traits
            .output_schema_refs
            .contains(&response.schema_ref)
    {
        return Err("data_capture_response_invalid".into());
    }
    let schema = service
        .runtime()
        .plugin_packages
        .data_schema(&pin.package_sha256, &response.schema_ref)?
        .ok_or("data_capture_response_schema_missing")?;
    if !jsonschema::validator_for(&schema)
        .map_err(|_| "data_capture_response_schema_invalid")?
        .is_valid(&response.payload)
    {
        return Err("data_capture_response_schema_invalid".into());
    }
    Ok(response)
}
fn validate_coverage(service: &TradeAssemblyService, r: &CaptureReceipt) -> Result<(), String> {
    let expected = check_policy(&r.policy)? as usize;
    let c = contract(service, &r.replay)?;
    check_replay_partition(&c, &r.policy.context)?;
    if hash(&c)? != r.replay_contract_sha256
        || r.replay_response.outputs.keys().ne(c.outputs.keys())
        || !r.replay_response.state.is_empty()
    {
        return Err("data_capture_replay_contract_invalid".into());
    }
    let window = EvaluationWindow {
        event_start_ms: r.policy.event_start_ms,
        event_end_ms: r.policy.event_end_ms,
        as_of_ms: r.replay_as_of_ms,
    };
    let interval = r.policy.context.interval_ms.unwrap() as i64;
    for (port, batch) in &r.replay_response.outputs {
        data_evaluation::validate_batch(
            service,
            &r.replay.package_sha256,
            &c.outputs[port],
            &r.policy.context,
            batch,
            &window,
        )?;
        if batch.points.len() != expected {
            return Err("data_capture_coverage_gap".into());
        }
        let mut positions: BTreeMap<&str, i64> = BTreeMap::new();
        for point in &batch.points {
            let prior = positions
                .get(point.instrument_id.as_str())
                .copied()
                .unwrap_or(r.policy.event_start_ms);
            if !point.provenance.completed
                || !matches!(point.result, PointValue::Ready(_))
                || point.provenance.event_start_ms != prior
                || point.provenance.event_end_ms.checked_sub(prior) != Some(interval)
            {
                return Err("data_capture_coverage_gap".into());
            }
            positions.insert(&point.instrument_id, point.provenance.event_end_ms);
        }
        if r.policy
            .context
            .instruments
            .iter()
            .any(|i| positions.get(i.instrument_id.as_str()) != Some(&r.policy.event_end_ms))
        {
            return Err("data_capture_coverage_gap".into());
        }
    }
    Ok(())
}
pub(crate) fn current(service: &TradeAssemblyService, id: &str) -> Result<CaptureReceipt, String> {
    if id.len() != 72
        || !id.starts_with("capture_")
        || !id[8..]
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err("data_capture_not_available".into());
    }
    let value = service
        .runtime()
        .storage
        .get_json(RECEIPTS, id)?
        .ok_or("data_capture_not_available")?;
    bounded(&value, MAX_BYTES)?;
    let r: CaptureReceipt =
        serde_json::from_value(value).map_err(|_| "data_capture_receipt_invalid")?;
    service
        .require_object("strategy", &r.policy.scope.strategy_id)
        .map_err(|_| "data_capture_not_available")?;
    validate_receipt_input(&r)?;
    let digest = identity(&r)?;
    if r.capture_id != id
        || r.receipt_sha256 != digest
        || id != format!("capture_{digest}")
        || hash(&r.acquisition_response.payload)? != r.payload_sha256
        || hash(&r.replay_response)? != r.replay_payload_sha256
        || r.replay_observed_at_ms < r.replay_as_of_ms
        || extract(&r.policy, &r.acquisition_response.payload)? != r.replay_parameters
    {
        return Err("data_capture_receipt_invalid".into());
    }
    let graph = resolve(service, &r.policy)?;
    if data_binding::pin(service, selection(&graph, &r.policy.acquisition_node)?)? != r.acquisition
        || data_binding::pin(service, selection(&graph, &r.policy.replay_node)?)? != r.replay
    {
        return Err("data_capture_binding_stale".into());
    }
    validate_coverage(service, &r)?;
    Ok(r)
}
fn check_replay_partition(
    contract: &VersionedOperationData,
    context: &DataContext,
) -> Result<(), String> {
    if contract.v3().is_some_and(|v| {
        v.history.partitioning
            == tradeassembly_plugin_sdk::data_history::Partitioning::InstrumentIndependent
    }) && context.instruments.len() > 1
    {
        return Err("data_capture_multi_instrument_independent_replay_unsupported".into());
    }
    Ok(())
}
pub(crate) fn acquire(
    service: &TradeAssemblyService,
    mut command: CaptureCommand,
    ctx: &SideEffectContext,
) -> Result<CaptureReceipt, String> {
    command
        .policy
        .context
        .instruments
        .sort_by(|a, b| a.instrument_id.cmp(&b.instrument_id));
    if !command.input.is_object() {
        return Err("data_capture_input_object_required".into());
    }
    bounded(&command.input, 65536)?;
    tradeassembly_plugin_sdk::data_package::metadata_budget(&command.input)?;
    if command.receipt_version == Some(CaptureReceiptVersion::V2) {
        check_v2_input(&command.input)?;
    }
    let input_witness =
        (command.receipt_version == Some(CaptureReceiptVersion::V2)).then(|| InputWitness {
            schema_version: InputWitnessVersion::V1,
            input: command.input.clone(),
        });
    let receipt_version = command.receipt_version.unwrap_or(CaptureReceiptVersion::V1);
    let graph = resolve(service, &command.policy)?;
    let acquisition = data_binding::pin(
        service,
        selection(&graph, &command.policy.acquisition_node)?,
    )?;
    let replay_selection = selection(&graph, &command.policy.replay_node)?;
    let replay = data_binding::pin(service, replay_selection)?;
    let c = contract(service, &replay)?;
    check_replay_partition(&c, &command.policy.context)?;
    if replay_selection.operation.credential_grant_required
        || replay_selection.operation.account_binding_required
        || !replay_selection.operation.traits.deterministic
        || !replay_selection.operation.traits.replayable
    {
        return Err("data_capture_pure_replay_required".into());
    }
    let input_sha256 = hash(&command.input)?;
    let command_hash = command_identity(&command.policy, &input_sha256, command.receipt_version)?;
    let key = hash(
        &json!({"strategy":command.policy.scope.strategy_id,"authority":ctx.authority,"key":ctx.idempotency_key.as_str()}),
    )?;
    if let Some(value) = service.runtime().storage.get_json(KEYS, &key)? {
        if value["commandSha256"] != command_hash {
            return Err("data_capture_idempotency_conflict".into());
        }
        return current(
            service,
            value["captureId"]
                .as_str()
                .ok_or("data_capture_receipt_invalid")?,
        );
    }
    let response = invoke(
        service,
        &command.policy,
        &graph,
        &command.policy.acquisition_node,
        command.input,
        ctx,
        "acquire",
    )?;
    let parameters = extract(&command.policy, &response.payload)?;
    if !jsonschema::validator_for(&c.parameter_schema)
        .map_err(|_| "data_capture_parameter_schema_invalid")?
        .is_valid(&parameters)
    {
        return Err("data_capture_parameters_invalid".into());
    }
    crate::data_pipeline::compile_data_plan(
        &graph,
        crate::data_pipeline::PipelineRequest {
            selected_outputs: vec![],
            source_captures: BTreeMap::new(),
            capability_graph_revision: command.policy.capability_graph_revision.clone(),
            mode: command.policy.mode.clone(),
            trade_instruments: vec![],
            nodes: vec![PipelineNode {
                graph_node_id: command.policy.replay_node.clone(),
                parameters: parameters.clone(),
                context: command.policy.context.clone(),
                input_contexts: BTreeMap::new(),
            }],
            bindings: vec![],
        },
        vec![crate::data_pipeline::VerifiedDataContract {
            graph_node_id: command.policy.replay_node.clone(),
            manifest_fingerprint: replay.manifest_sha256.clone(),
            contract_sha256: hash(&c)?,
            contract: c.clone(),
        }],
    )
    .map_err(|errors| {
        format!(
            "data_capture_replay_plan_invalid:{}",
            serde_json::to_string(&errors).unwrap_or_default()
        )
    })?;
    let as_of = service.runtime().clock.now_ms();
    let payload = DataEvaluationRequest {
        schema_version: EvaluationVersion::V1,
        plan_sha256: command_hash.clone(),
        node_id: command.policy.replay_node.clone(),
        semantics_version: c.semantics_version.clone(),
        instruments: command
            .policy
            .context
            .instruments
            .iter()
            .map(|i| i.instrument_id.clone())
            .collect(),
        window: EvaluationWindow {
            event_start_ms: command.policy.event_start_ms,
            event_end_ms: command.policy.event_end_ms,
            as_of_ms: as_of,
        },
        parameters: parameters.clone(),
        inputs: BTreeMap::new(),
        prior_state: BTreeMap::new(),
        max_points: MAX_POINTS,
    };
    let replay_response = invoke(
        service,
        &command.policy,
        &graph,
        &command.policy.replay_node,
        serde_json::to_value(payload).map_err(|_| "data_capture_encoding_invalid")?,
        ctx,
        "replay",
    )?;
    if replay_response.schema_ref != EVALUATION_RESPONSE_SCHEMA
        || !replay_response.deterministic
        || !replay_response.replayable
    {
        return Err("data_capture_pure_replay_required".into());
    }
    let replay_payload_sha256 = hash(&replay_response.payload)?;
    let replay_observed_at_ms = replay_response.observed_at_ms;
    let replay_content_hash = replay_response.content_hash.clone();
    let replay_source_event_id = replay_response.source_event_id.clone();
    let mut receipt = CaptureReceipt {
        input_witness,
        schema_version: receipt_version,
        capture_id: String::new(),
        receipt_sha256: String::new(),
        policy: command.policy,
        input_sha256,
        acquisition,
        replay,
        replay_contract_sha256: hash(&c)?,
        payload_sha256: hash(&response.payload)?,
        acquisition_response: response,
        replay_parameters: parameters,
        replay_response: serde_json::from_value(replay_response.payload)
            .map_err(|_| "data_capture_replay_response_invalid")?,
        replay_as_of_ms: as_of,
        replay_observed_at_ms,
        replay_payload_sha256,
        replay_content_hash,
        replay_source_event_id,
    };
    for batch in receipt.replay_response.outputs.values_mut() {
        for point in &mut batch.points {
            if let PointValue::Ready(value) = &mut point.result {
                tradeassembly_plugin_sdk::canonical::normalize_numbers(value)
                    .map_err(|_| "data_capture_numeric_range_invalid")?;
            }
        }
    }
    validate_coverage(service, &receipt)?;
    let fresh = resolve(service, &receipt.policy)?;
    if data_binding::pin(
        service,
        selection(&fresh, &receipt.policy.acquisition_node)?,
    )? != receipt.acquisition
        || data_binding::pin(service, selection(&fresh, &receipt.policy.replay_node)?)?
            != receipt.replay
    {
        return Err("data_capture_binding_stale".into());
    }
    validate_receipt_input(&receipt)?;
    receipt.receipt_sha256 = identity(&receipt)?;
    receipt.capture_id = format!("capture_{}", receipt.receipt_sha256);
    bounded(&receipt, MAX_BYTES)?;
    let value = serde_json::to_value(&receipt).map_err(|_| "data_capture_encoding_invalid")?;
    let storage = service.runtime().storage.clone();
    let writes = vec![
        StorageWrite::new(RECEIPTS, &receipt.capture_id, value.clone(), ctx.clone()),
        StorageWrite::new(
            KEYS,
            &key,
            json!({"captureId":receipt.capture_id,"commandSha256":command_hash}),
            ctx.clone(),
        ),
    ];
    if storage.put_json_batch(
        &writes,
        &[
            StorageExpectation::new(RECEIPTS, &receipt.capture_id, None),
            StorageExpectation::new(KEYS, &key, None),
        ],
    )? != ComparePutOutcome::Updated
    {
        return Err("data_capture_commit_conflict".into());
    }
    Ok(receipt)
}
fn context(service: &TradeAssemblyService, body: &Value) -> Result<SideEffectContext, String> {
    let envelope = crate::control_plane::http_envelope(
        "http",
        "POST",
        "/product/strategies/data-captures/create",
        body,
    )?;
    Ok(SideEffectContext::new(
        envelope.authority,
        IdempotencyKey::new(
            body["idempotencyKey"]
                .as_str()
                .ok_or("data_capture_idempotency_required")?,
        )?,
    )
    .with_agent_execution(service.agent_mcp_execution_context()))
}
fn error_response(code: &str) -> ServiceResponse {
    if matches!(
        code,
        "data_capture_command_invalid"
            | "data_capture_id_required"
            | "data_capture_idempotency_required"
    ) {
        ServiceResponse::bad_request(code)
    } else {
        ServiceResponse::conflict(code)
    }
}
pub fn create(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let result = (|| {
        let command: CaptureCommand = serde_json::from_value(body["capture"].clone())
            .map_err(|_| "data_capture_command_invalid")?;
        acquire(service, command, &context(service, &body)?)
    })();
    match result {
        Ok(receipt) => {
            ServiceResponse::ok(json!({"capture":receipt,"executionAuthorityGranted":false}))
        }
        Err(code) => error_response(&code),
    }
}
pub fn read(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    match body["captureId"]
        .as_str()
        .ok_or("data_capture_id_required")
        .and_then(|id| current(service, id).map_err(|_| "data_capture_not_available"))
    {
        Ok(receipt) => {
            ServiceResponse::ok(json!({"capture":receipt,"executionAuthorityGranted":false}))
        }
        Err(code) => ServiceResponse::bad_request(code),
    }
}
pub(crate) fn preflight(
    service: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    if path != "/product/strategies/data-captures/create"
        && path != "/product/strategies/data-captures/get"
    {
        return None;
    }
    let result: Result<(), String> = (|| {
        if path.ends_with("create") {
            let c: CaptureCommand = serde_json::from_value(body["capture"].clone())
                .map_err(|_| "data_capture_command_invalid")?;
            if c.receipt_version == Some(CaptureReceiptVersion::V2) {
                check_v2_input(&c.input)?;
            }
            let graph = resolve(service, &c.policy)?;
            data_binding::pin(service, selection(&graph, &c.policy.acquisition_node)?)?;
            data_binding::pin(service, selection(&graph, &c.policy.replay_node)?)?;
            let mut policy = c.policy;
            policy
                .context
                .instruments
                .sort_by(|a, b| a.instrument_id.cmp(&b.instrument_id));
            bounded(&c.input, 65536)?;
            let ctx = context(service, body)?;
            let key = hash(
                &json!({"strategy":policy.scope.strategy_id,"authority":ctx.authority,"key":ctx.idempotency_key.as_str()}),
            )?;
            if let Some(value) = service.runtime().storage.get_json(KEYS, &key)? {
                if value["commandSha256"]
                    != command_identity(&policy, &hash(&c.input)?, c.receipt_version)?
                {
                    return Err("data_capture_idempotency_conflict".into());
                }
                current(
                    service,
                    value["captureId"]
                        .as_str()
                        .ok_or("data_capture_receipt_invalid")?,
                )?;
            }
        } else {
            current(
                service,
                body["captureId"]
                    .as_str()
                    .ok_or("data_capture_id_required")?,
            )?;
        }
        Ok(())
    })();
    result.err().map(|code| error_response(&code))
}

pub(crate) fn validate_plan_sources(
    service: &TradeAssemblyService,
    scope: &PlanScope,
    graph: &CapabilityGraphResolution,
    plan: &crate::data_pipeline::CompiledDataPlan,
) -> Result<BTreeMap<String, SourceAuthentication>, String> {
    let mut result = BTreeMap::new();
    for id in plan.request.source_captures.keys() {
        if !plan
            .contracts
            .iter()
            .any(|c| &c.graph_node_id == id && c.contract.inputs.is_empty())
        {
            return Err("data_capture_binding_not_source".into());
        }
    }
    for contract in &plan.contracts {
        if !contract.contract.inputs.is_empty() {
            continue;
        }
        let id = &contract.graph_node_id;
        let auth = match plan.request.source_captures.get(id) {
            None => SourceAuthentication::UnverifiedInput,
            Some(capture_id) => {
                let r = current(service, capture_id)?;
                let node = plan
                    .request
                    .nodes
                    .iter()
                    .find(|n| &n.graph_node_id == id)
                    .ok_or("data_capture_binding_not_source")?;
                if r.policy.scope.strategy_id != scope.strategy_id
                    || r.policy.context != node.context
                    || r.replay_parameters != node.parameters
                    || r.replay != data_binding::pin(service, selection(graph, id)?)?
                    || r.replay_contract_sha256 != contract.contract_sha256
                {
                    return Err("data_capture_plan_binding_mismatch".into());
                }
                SourceAuthentication::HostAcquisition {
                    capture_id: r.capture_id,
                    receipt_sha256: r.receipt_sha256,
                    payload_sha256: r.payload_sha256,
                }
            }
        };
        result.insert(id.clone(), auth);
    }
    Ok(result)
}
pub(crate) fn validate_window(
    service: &TradeAssemblyService,
    plan: &super::data_plans::StoredDataPlan,
    window: &EvaluationWindow,
) -> Result<(), String> {
    if plan
        .content
        .source_authentication
        .values()
        .any(|source| matches!(source, SourceAuthentication::HostCaptureStream { .. }))
    {
        super::managed_data_streams::validate_plan_streams(service, plan)?;
        super::data_evaluation::source_snapshot(service, plan, window)?
            .ok_or("managed_stream_private_snapshot_required")?;
    }
    for auth in plan.content.source_authentication.values() {
        if let SourceAuthentication::HostAcquisition { capture_id, .. } = auth {
            let r = current(service, capture_id)?;
            if window.event_start_ms < r.policy.event_start_ms
                || window.event_end_ms > r.policy.event_end_ms
                || window.as_of_ms
                    < r.acquisition_response
                        .observed_at_ms
                        .max(r.replay_observed_at_ms)
            {
                return Err("data_capture_window_outside_admission".into());
            }
        }
    }
    Ok(())
}
pub(crate) fn envelope_source(
    service: &TradeAssemblyService,
    plan: &super::data_plans::StoredDataPlan,
    node: &PipelineNode,
    response: &mut DataEvaluationResponse,
) -> Result<(), String> {
    if matches!(
        plan.content.source_authentication.get(&node.graph_node_id),
        Some(SourceAuthentication::HostCaptureStream { .. })
    ) {
        return Err("managed_stream_segment_envelope_required".into());
    }
    let Some(SourceAuthentication::HostAcquisition { capture_id, .. }) =
        plan.content.source_authentication.get(&node.graph_node_id)
    else {
        return Ok(());
    };
    let receipt = current(service, capture_id)?;
    for (port, batch) in &mut response.outputs {
        let original = receipt
            .replay_response
            .outputs
            .get(port)
            .ok_or("data_capture_replay_mismatch")?;
        for point in &mut batch.points {
            if !original.points.iter().any(|p| p == point) {
                return Err("data_capture_replay_mismatch".into());
            }
            // Preserve the provider point in the immutable receipt. This host
            // envelope records when its observed capture actually became usable.
            let raw_hash = point.provenance.content_sha256.clone();
            point.provenance.known_at_ms = point
                .provenance
                .known_at_ms
                .max(receipt.acquisition_response.observed_at_ms)
                .max(receipt.replay_observed_at_ms);
            point.provenance.arrived_at_ms = point
                .provenance
                .arrived_at_ms
                .max(receipt.replay_observed_at_ms)
                .max(point.provenance.known_at_ms);
            point
                .provenance
                .upstream_content_sha256
                .extend([raw_hash, receipt.receipt_sha256.clone()]);
            point.provenance.upstream_content_sha256.sort();
            point.provenance.upstream_content_sha256.dedup();
            point.provenance.content_sha256 =
                point_sha256(point).map_err(|_| "data_capture_point_invalid")?;
        }
    }
    Ok(())
}

pub(crate) fn resolve_parameters(
    service: &TradeAssemblyService,
    scope: &PlanScope,
    request: &mut crate::data_pipeline::PipelineRequest,
) -> Result<(), String> {
    for (id, capture_id) in &request.source_captures {
        let receipt = current(service, capture_id)?;
        let node = request
            .nodes
            .iter_mut()
            .find(|n| &n.graph_node_id == id)
            .ok_or("data_capture_binding_not_source")?;
        node.context
            .instruments
            .sort_by(|a, b| a.instrument_id.cmp(&b.instrument_id));
        if receipt.policy.scope.strategy_id != scope.strategy_id
            || receipt.policy.context != node.context
            || (!node.parameters.as_object().is_some_and(|p| p.is_empty())
                && node.parameters != receipt.replay_parameters)
        {
            return Err("data_capture_plan_binding_mismatch".into());
        }
        node.parameters = receipt.replay_parameters;
    }
    Ok(())
}

#[cfg(test)]
mod partition_tests {
    use super::*;
    #[test]
    fn independent_multi_instrument_replay_rejects_before_acquisition() {
        use tradeassembly_plugin_sdk::data_history::*;
        let (_, mut request, contracts) = crate::data_pipeline::tests::fixture();
        let mut operation = contracts
            .into_iter()
            .find(|c| c.contract.inputs.is_empty())
            .unwrap()
            .contract;
        operation.schema_version = tradeassembly_plugin_sdk::data_contract::DataContractVersion::V2;
        let operation = (*operation).clone();
        let mut contract = VersionedOperationData::V3(OperationDataV3 {
            schema_version: HistoryVersion::V3,
            history: HistoryContract {
                first_ready: operation
                    .outputs
                    .keys()
                    .map(|k| (k.clone(), BTreeMap::new()))
                    .collect(),
                partitioning: Partitioning::InstrumentIndependent,
                required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
                reconstruction: Reconstruction::FiniteWindow,
            },
            operation,
        });
        let context = &mut request.nodes[0].context;
        assert!(check_replay_partition(&contract, context).is_ok());
        let mut b = context.instruments[0].clone();
        b.instrument_id = "B".into();
        context.instruments.push(b);
        assert_eq!(
            check_replay_partition(&contract, context).unwrap_err(),
            "data_capture_multi_instrument_independent_replay_unsupported"
        );
        if let VersionedOperationData::V3(c) = &mut contract {
            c.history.partitioning = Partitioning::AllDeclaredContext;
        }
        assert!(check_replay_partition(&contract, context).is_ok());
    }
}

#[cfg(test)]
mod input_witness_tests {
    use super::*;
    fn command() -> CaptureCommand {
        CaptureCommand {
            receipt_version: None,
            policy: CapturePolicy {
                scope: PlanScope {
                    strategy_id: "s".into(),
                    strategy_version_id: "v".into(),
                    strategy_spec_hash: format!("sha256:{}", "a".repeat(64)),
                },
                capability_graph_revision: "graph".into(),
                mode: "paper".into(),
                acquisition_node: "acquire".into(),
                replay_node: "replay".into(),
                parameter_extracts: BTreeMap::from([("bars".into(), "/bars".into())]),
                context: DataContext {
                    instruments: vec![crate::data_pipeline::InstrumentContext {
                        instrument_id: "OWNED".into(),
                        asset_class: "equity".into(),
                        instrument_family: "equity".into(),
                    }],
                    interval_ms: Some(10),
                },
                event_start_ms: 10,
                event_end_ms: 30,
                gap_policy: GapPolicy::StrictFixedInterval,
            },
            input: json!({"symbol":"OWNED","feed":"declared","start":10,"end":30}),
        }
    }
    #[test]
    fn capture_none_and_explicit_v1_preserve_original_command_hash_and_none_wire() {
        #[derive(Serialize)]
        struct LegacyCommand<'a> {
            policy: &'a CapturePolicy,
            input: &'a Value,
        }
        let c = command();
        assert_eq!(
            serde_json::to_vec(&c).unwrap(),
            serde_json::to_vec(&LegacyCommand {
                policy: &c.policy,
                input: &c.input
            })
            .unwrap()
        );
        let old = json!({"policy":c.policy,"input":c.input});
        assert_eq!(serde_json::to_value(&c).unwrap(), old);
        let input_sha = hash(&c.input).unwrap();
        let original = hash(&(c.policy.clone(), &input_sha)).unwrap();
        assert_eq!(
            command_identity(&c.policy, &input_sha, None).unwrap(),
            original
        );
        assert_eq!(
            command_identity(&c.policy, &input_sha, Some(CaptureReceiptVersion::V1)).unwrap(),
            original
        );
        assert_ne!(
            command_identity(&c.policy, &input_sha, Some(CaptureReceiptVersion::V2)).unwrap(),
            original
        );
        let decoded: CaptureCommand = serde_json::from_value(old).unwrap();
        assert!(decoded.receipt_version.is_none());
    }
    #[test]
    fn v2_witness_is_closed_exact_nonsecret_and_v1_cannot_claim_it() {
        let input = command().input;
        let witness = InputWitness {
            schema_version: InputWitnessVersion::V1,
            input: input.clone(),
        };
        let sha = hash(&input).unwrap();
        validate_witness(CaptureReceiptVersion::V2, &sha, Some(&witness)).unwrap();
        assert!(validate_witness(CaptureReceiptVersion::V1, &sha, Some(&witness)).is_err());
        assert!(validate_witness(CaptureReceiptVersion::V2, &sha, None).is_err());
        let mut wrong = witness.clone();
        wrong.input["feed"] = json!("changed");
        assert_eq!(
            validate_witness(CaptureReceiptVersion::V2, &sha, Some(&wrong)).unwrap_err(),
            "data_capture_input_witness_hash_mismatch"
        );
        let invalid = json!({"schemaVersion":"tradeassembly.data_capture_input_witness.v1","input":{},"authority":"forbidden"});
        assert!(serde_json::from_value::<InputWitness>(invalid).is_err());
        let secret = InputWitness {
            schema_version: InputWitnessVersion::V1,
            input: json!({"credentials":{"token":"not-persisted"}}),
        };
        assert!(validate_witness(
            CaptureReceiptVersion::V2,
            &hash(&secret.input).unwrap(),
            Some(&secret)
        )
        .is_err());
        assert!(check_v2_input(&json!({"tooLarge":"x".repeat(65_536)})).is_err());
    }
    #[test]
    fn v2_input_witness_refuses_normalized_nested_credential_fields_only() {
        for name in [
            "api_key",
            "API-KEY",
            "apiSecret",
            "Password",
            "accessToken",
            "refresh_token",
            "private.key",
            "clientSecret",
            "pass_phrase",
        ] {
            let input = json!({"query":[{"nested":{(name):"not-a-secret-test-value"}}]});
            // These fields remain subject to their original V1 semantics.
            capturable_payload(&input).unwrap();
            assert_eq!(
                check_v2_input(&input).unwrap_err(),
                "data_capture_input_credential_material_forbidden",
                "{name}"
            );
            let witness = InputWitness {
                schema_version: InputWitnessVersion::V1,
                input,
            };
            assert_eq!(
                validate_witness(
                    CaptureReceiptVersion::V2,
                    &hash(&witness.input).unwrap(),
                    Some(&witness)
                )
                .unwrap_err(),
                "data_capture_input_credential_material_forbidden"
            );
        }
        check_v2_input(&json!({"symbol":"OWNED","feed":"declared",
            "adjustment":"raw","pageToken":"noncredential-pagination-cursor"}))
        .unwrap();
    }
    #[test]
    fn requested_v2_nested_credentials_refuse_before_command_and_provider_records() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory
            .path()
            .join("nested-input-guard.sqlite")
            .to_string_lossy()
            .into_owned();
        let service = TradeAssemblyService::test_local(&db);
        let names = [
            "control_plane_commands",
            RECEIPTS,
            KEYS,
            "plugin_operation_receipts",
        ];
        let before = names.map(|ns| service.runtime().storage.list_json(ns).unwrap());
        for name in [
            "api_key",
            "apiSecret",
            "password",
            "accessToken",
            "privateKey",
        ] {
            let mut c = command();
            c.receipt_version = Some(CaptureReceiptVersion::V2);
            c.input = json!({"query":[{"nested":{(name):"not-a-secret-test-value"}}]});
            let result = service.handle_http("POST", "/product/strategies/data-captures/create",
                json!({"capture":c,"accountMode":"paper","idempotencyKey":format!("nested-{name}")}));
            assert_eq!(result.status, 409, "{}", result.body);
            assert_eq!(
                result.body["error"]["code"],
                "data_capture_input_credential_material_forbidden"
            );
            assert_eq!(
                names.map(|ns| service.runtime().storage.list_json(ns).unwrap()),
                before
            );
        }
    }
    #[test]
    fn requested_v2_secret_input_refuses_before_outer_command_or_capture_persistence() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory
            .path()
            .join("input-guard.sqlite")
            .to_string_lossy()
            .into_owned();
        let service = TradeAssemblyService::test_local(&db);
        let mut c = command();
        c.receipt_version = Some(CaptureReceiptVersion::V2);
        c.input = json!({"headers":{"authorization":"not-a-secret-test-value"}});
        let names = [
            "control_plane_commands",
            RECEIPTS,
            KEYS,
            "plugin_operation_receipts",
        ];
        let before = names.map(|ns| service.runtime().storage.list_json(ns).unwrap());
        let result = service.handle_http(
            "POST",
            "/product/strategies/data-captures/create",
            json!({
                "capture":c,"accountMode":"paper","idempotencyKey":"v2-refuse-before-storage",
            }),
        );
        assert_eq!(result.status, 409, "{}", result.body);
        assert_eq!(
            result.body["error"]["code"],
            "data_capture_transport_material_forbidden"
        );
        assert_eq!(
            names.map(|ns| service.runtime().storage.list_json(ns).unwrap()),
            before
        );
    }
}

#[cfg(test)]
mod legacy_receipt_wire_tests {
    use super::*;
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Legacy<'a> {
        schema_version: CaptureReceiptVersion,
        capture_id: &'a str,
        receipt_sha256: &'a str,
        policy: &'a CapturePolicy,
        input_sha256: &'a str,
        acquisition: &'a PinnedOperation,
        replay: &'a PinnedOperation,
        replay_contract_sha256: &'a str,
        acquisition_response: &'a PluginOperationResponse,
        payload_sha256: &'a str,
        replay_parameters: &'a Value,
        replay_response: &'a DataEvaluationResponse,
        replay_as_of_ms: i64,
        replay_observed_at_ms: i64,
        replay_payload_sha256: &'a str,
        replay_content_hash: &'a str,
        replay_source_event_id: &'a str,
    }
    #[test]
    fn absent_v1_witness_preserves_original_receipt_field_bytes_and_identity_hash() {
        // Serialization fixture only. Empty output data is never admitted or
        // represented as evaluated/available provider evidence by this test.
        let pin=serde_json::from_value(json!({"pluginInstanceRef":"fixture","pluginRef":"fixture","operationId":"fixture",
            "packageSha256":"a".repeat(64),"manifestSha256":"b".repeat(64),"contextSha256":"c".repeat(64),"accountRef":null})).unwrap();
        let response = PluginOperationResponse {
            correlation_id: "serializer-only".into(),
            schema_ref: "schema://example/serializer@1".into(),
            content_hash: "d".repeat(64),
            payload: json!({}),
            observed_at_ms: 30,
            source_event_id: "serializer-only".into(),
            freshness_state: "serializer-only".into(),
            evidence_refs: vec![],
            deterministic: true,
            replayable: true,
            provider_outcome_id: None,
            reconciliation_required: false,
        };
        let policy:CapturePolicy=serde_json::from_value(json!({"scope":{"strategyId":"s","strategyVersionId":"v","strategySpecHash":"h"},
            "capabilityGraphRevision":"g","mode":"paper","acquisitionNode":"a","replayNode":"r","parameterExtracts":{"v":"/v"},
            "context":{"instruments":[{"instrumentId":"X","assetClass":"equity","instrumentFamily":"equity"}],"intervalMs":10},
            "eventStartMs":10,"eventEndMs":30,"gapPolicy":"strict_fixed_interval"})).unwrap();
        let r=CaptureReceipt {
            input_witness:None,schema_version:CaptureReceiptVersion::V1,capture_id:String::new(),receipt_sha256:String::new(),
            policy,input_sha256:hash(&json!({})).unwrap(),acquisition:pin,replay:serde_json::from_value(json!({
                "pluginInstanceRef":"fixture","pluginRef":"fixture","operationId":"replay","packageSha256":"a".repeat(64),
                "manifestSha256":"b".repeat(64),"contextSha256":"c".repeat(64),"accountRef":null})).unwrap(),
            replay_contract_sha256:"e".repeat(64),acquisition_response:response,payload_sha256:"f".repeat(64),replay_parameters:json!({}),
            replay_response:DataEvaluationResponse {schema_version:EvaluationVersion::V1,outputs:BTreeMap::new(),state:BTreeMap::new()},
            replay_as_of_ms:31,replay_observed_at_ms:32,replay_payload_sha256:"a".repeat(64),replay_content_hash:"b".repeat(64),replay_source_event_id:"serializer-only".into(),
        };
        let old = Legacy {
            schema_version: r.schema_version,
            capture_id: &r.capture_id,
            receipt_sha256: &r.receipt_sha256,
            policy: &r.policy,
            input_sha256: &r.input_sha256,
            acquisition: &r.acquisition,
            replay: &r.replay,
            replay_contract_sha256: &r.replay_contract_sha256,
            acquisition_response: &r.acquisition_response,
            payload_sha256: &r.payload_sha256,
            replay_parameters: &r.replay_parameters,
            replay_response: &r.replay_response,
            replay_as_of_ms: r.replay_as_of_ms,
            replay_observed_at_ms: r.replay_observed_at_ms,
            replay_payload_sha256: &r.replay_payload_sha256,
            replay_content_hash: &r.replay_content_hash,
            replay_source_event_id: &r.replay_source_event_id,
        };
        assert_eq!(
            serde_json::to_vec(&r).unwrap(),
            serde_json::to_vec(&old).unwrap()
        );
        assert_eq!(identity(&r).unwrap(), hash(&old).unwrap());
    }
}
