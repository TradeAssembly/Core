//! Explicit strategy-bound stateless screening over admitted named documents.
//! Candidate sets grant no order authority and never remove managed positions.
use super::{
    capability_graph, data_binding, data_collections, data_evaluation, data_plans::PlanScope,
    strategy, TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, ports::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_plugin_sdk::{
    data_contract::{DataContractVersion, DataShape, SchemaIdentity},
    instrument_screen as wire,
    instrument_set::*,
};
use tradeassembly_strategy::spec::{InstrumentScreen, ScreenQuery, SelectorType, StrategySpec};
const RECEIPTS: &str = "instrument_screen_receipts_v1";
const KEYS: &str = "instrument_screen_keys_v1";
const CREATE_PATH: &str = "/product/strategies/instrument-screens/create";
const GET_PATH: &str = "/product/strategies/instrument-screens/get";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenCommand {
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub mode: String,
    pub signal_requirement_ref: String,
    pub selector_ref: String,
    pub collections: BTreeMap<String, String>,
    pub knowledge_cutoff_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenOperationEvidence {
    pub operation: data_binding::PinnedOperation,
    pub contract_sha256: String,
    pub policy_sha256: String,
    pub request_sha256: String,
    pub response: PluginOperationResponse,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenReceipt {
    pub schema_version: String,
    pub selection_id: String,
    pub receipt_sha256: String,
    pub command: ScreenCommand,
    pub operation: ScreenOperationEvidence,
    pub instrument_set: InstrumentSet,
}
struct Prepared {
    policy: InstrumentScreen,
    graph: CapabilityGraphResolution,
    selected: CapabilitySelection,
    pin: data_binding::PinnedOperation,
    contract_sha256: String,
    request: wire::ScreenRequest,
    asset_class: String,
    family: String,
    allowed_contexts: BTreeSet<String>,
    input_expires_at_ms: i64,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "instrument_screen_encoding_invalid".into())
}
fn identity(r: &ScreenReceipt) -> Result<String, String> {
    let mut v = r.clone();
    v.selection_id.clear();
    v.receipt_sha256.clear();
    hash(&v)
}
/// UTC calendar-date arithmetic is explicit; this is not exchange-session DTE.
pub fn materialize_query(query: &ScreenQuery, cutoff_ms: i64) -> Result<Value, String> {
    use chrono::Datelike;
    let date = chrono::DateTime::from_timestamp_millis(cutoff_ms)
        .filter(|_| cutoff_ms > 0)
        .ok_or("instrument_screen_date_invalid")?
        .date_naive();
    let mut value =
        serde_json::to_value(&query.literal).map_err(|_| "instrument_screen_query_invalid")?;
    for (key, binding) in &query.date_bindings {
        if value.get(key).is_some() || binding.offset_calendar_days.unsigned_abs() > 3660 {
            return Err("instrument_screen_query_invalid".into());
        }
        let days = chrono::Days::new(binding.offset_calendar_days.unsigned_abs() as u64);
        let shifted = if binding.offset_calendar_days >= 0 {
            date.checked_add_days(days)
        } else {
            date.checked_sub_days(days)
        }
        .filter(|d| (1..=9999).contains(&d.year()))
        .ok_or("instrument_screen_date_invalid")?;
        value[key] = json!(shifted.format("%Y-%m-%d").to_string());
    }
    Ok(value)
}
fn policy(
    service: &TradeAssemblyService,
    c: &ScreenCommand,
) -> Result<(StrategySpec, InstrumentScreen, String, String), String> {
    service
        .require_object("strategy", &c.scope.strategy_id)
        .map_err(|_| "instrument_screen_not_available")?;
    let version = strategy::strategy_versions(service, &c.scope.strategy_id)
        .into_iter()
        .find(|v| {
            v["id"] == c.scope.strategy_version_id && v["specHash"] == c.scope.strategy_spec_hash
        })
        .ok_or("instrument_screen_version_missing")?;
    if crate::spec::canonical_hash(&version["spec"])? != c.scope.strategy_spec_hash {
        return Err("instrument_screen_spec_hash_invalid".into());
    }
    // Admission validates canonical JSON; parse that identical representation so
    // schema-valid integral forms (e.g. top_n:1.0) retain the same semantics.
    let spec = StrategySpec::model_validate(version["spec"].clone())
        .map_err(|_| "instrument_screen_spec_invalid")?;
    if !tradeassembly_strategy::spec::validate(&version["spec"]).valid {
        return Err("instrument_screen_spec_invalid".into());
    }
    let markets = spec
        .signal_market_requirements
        .iter()
        .filter(|m| m.requirement_id == c.signal_requirement_ref)
        .collect::<Vec<_>>();
    if markets.len() != 1 {
        return Err("instrument_screen_market_missing".into());
    }
    let selectors = markets[0]
        .selectors
        .iter()
        .filter(|s| s.selector_id == c.selector_ref)
        .collect::<Vec<_>>();
    if selectors.len() != 1 || selectors[0].selector_type != SelectorType::Screened {
        return Err("instrument_screen_selector_missing".into());
    }
    let p = selectors[0]
        .screen
        .clone()
        .ok_or("instrument_screen_selector_missing")?;
    p.validate()?;
    let asset = serde_json::to_value(&markets[0].asset_class)
        .map_err(|_| "instrument_screen_market_invalid")?
        .as_str()
        .ok_or("instrument_screen_market_invalid")?
        .to_string();
    let family = serde_json::to_value(&markets[0].instrument_family)
        .map_err(|_| "instrument_screen_market_invalid")?
        .as_str()
        .ok_or("instrument_screen_market_invalid")?
        .to_string();
    Ok((spec, p, asset, family))
}
fn node<'a>(
    graph: &'a CapabilityGraphResolution,
    id: &str,
) -> Result<&'a CapabilitySelection, String> {
    graph
        .nodes
        .iter()
        .find(|n| n.node_id == id)
        .and_then(|n| n.selected.as_ref())
        .ok_or("instrument_screen_node_unresolved".into())
}
fn fixed_schema(
    service: &TradeAssemblyService,
    pin: &data_binding::PinnedOperation,
    identity: SchemaIdentity,
) -> Result<(), String> {
    let schema = service
        .runtime()
        .plugin_packages
        .data_schema(&pin.package_sha256, &identity.schema_ref)?
        .ok_or("instrument_screen_protocol_schema_missing")?;
    if hash(&schema)? != identity.sha256 {
        return Err("instrument_screen_protocol_schema_mismatch".into());
    }
    Ok(())
}
fn prepare(service: &TradeAssemblyService, c: &ScreenCommand) -> Result<Prepared, String> {
    let now = service.runtime().clock.now_ms();
    if !matches!(c.mode.as_str(), "paper" | "backtest" | "live")
        || c.knowledge_cutoff_ms <= 0
        || c.knowledge_cutoff_ms > now
        || c.collections.len() > 8
    {
        return Err("instrument_screen_command_invalid".into());
    }
    let (spec, p, asset_class, family) = policy(service, c)?;
    if c.collections.keys().ne(p.inputs.keys()) {
        return Err("instrument_screen_input_bindings_invalid".into());
    }
    let epoch = chrono::DateTime::from_timestamp_millis(now)
        .ok_or("instrument_screen_clock_invalid")?
        .to_rfc3339();
    let revision = capability_graph::revision_for_context(
        service,
        &c.capability_graph_revision,
        &c.scope.strategy_id,
        &c.scope.strategy_version_id,
        &c.mode,
        &epoch,
    )
    .map_err(|_| "instrument_screen_graph_stale")?;
    if revision["strategySpecHash"] != c.scope.strategy_spec_hash {
        return Err("instrument_screen_strategy_mismatch".into());
    }
    let graph: CapabilityGraphResolution = serde_json::from_value(revision["graph"].clone())
        .map_err(|_| "instrument_screen_graph_invalid")?;
    let selected = node(&graph, &p.capability_requirement_ref)?.clone();
    let required = spec
        .capability_requirements
        .required
        .iter()
        .find(|r| r.requirement_id == p.capability_requirement_ref)
        .ok_or("instrument_screen_capability_missing")?;
    if selected.operation.capability != required.capability {
        return Err("instrument_screen_capability_mismatch".into());
    }
    let pin = data_binding::pin(service, &selected)?;
    let package = service
        .runtime()
        .plugin_packages
        .get(&pin.package_sha256)?
        .ok_or("instrument_screen_package_missing")?;
    data_evaluation::allowed_data_operation(&selected, true, &package.manifest)?;
    let contract = service
        .runtime()
        .plugin_packages
        .operation_data(&pin.package_sha256, &pin.operation_id)?
        .ok_or("instrument_screen_contract_missing")?;
    if contract.v3().is_some()
        || contract.schema_version != DataContractVersion::V2
        || contract.state.is_some()
        || contract.semantics_version != p.semantics_version
        || contract.warmup.minimum_observations != 1
        || contract.warmup.observations_parameter.is_some()
        || contract.warmup.additional_observations != 0
        || !selected.operation.traits.deterministic
        || !selected.operation.traits.replayable
        || !selected.operation.traits.modes.contains(&c.mode)
        || !selected
            .operation
            .traits
            .instrument_families
            .contains(&family)
        || !contract.applicability.asset_classes.contains(&asset_class)
        || !contract.applicability.instrument_families.contains(&family)
        || !contract.applicability.modes.contains(&c.mode)
        || contract.inputs.keys().ne(p.inputs.keys())
        || contract.outputs.len() != 1
        || !selected
            .operation
            .traits
            .input_schema_refs
            .iter()
            .any(|s| s == wire::SCREEN_REQUEST_SCHEMA)
        || !selected
            .operation
            .traits
            .output_schema_refs
            .iter()
            .any(|s| s == wire::SCREEN_RESULT_SCHEMA)
    {
        return Err("instrument_screen_contract_unsupported".into());
    }
    let output = contract
        .outputs
        .get("instruments")
        .ok_or("instrument_screen_contract_unsupported")?;
    if output.schema != wire::result_identity()
        || output.shape != DataShape::Table
        || output.time.interval_ms.is_some()
    {
        return Err("instrument_screen_output_contract_invalid".into());
    }
    for id in [
        wire::request_identity(),
        wire::result_identity(),
        wire::input_identity(),
    ] {
        fixed_schema(service, &pin, id)?;
    }
    let mut parameters = p.parameters.clone();
    tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut parameters)
        .map_err(|_| "instrument_screen_parameters_invalid")?;
    if !jsonschema::validator_for(&contract.parameter_schema)
        .map_err(|_| "instrument_screen_parameter_schema_invalid")?
        .is_valid(&parameters)
    {
        return Err("instrument_screen_parameters_invalid".into());
    }
    let mut inputs = BTreeMap::new();
    let mut queries = BTreeMap::new();
    let mut expires = i64::MAX;
    for (port, input_policy) in &p.inputs {
        let input_contract = &contract.inputs[port];
        if !input_contract.required
            || input_contract.data.schema != wire::input_identity()
            || input_contract.data.shape != DataShape::Table
            || input_contract.data.time.interval_ms.is_some()
            || input_contract.dependency_id != *port
        {
            return Err("instrument_screen_input_contract_invalid".into());
        }
        let source = data_collections::current(service, &c.collections[port])?;
        let expected_query = materialize_query(&input_policy.query, c.knowledge_cutoff_ms)?;
        let source_requirement = spec
            .capability_requirements
            .required
            .iter()
            .find(|r| r.requirement_id == input_policy.capability_requirement_ref)
            .ok_or("instrument_screen_input_capability_missing")?;
        if source.command.scope != c.scope
            || source.command.capability_graph_revision != c.capability_graph_revision
            || source.command.mode != c.mode
            || source.command.acquisition_node != input_policy.capability_requirement_ref
            || source.command.response_schema_ref != input_policy.response_schema_ref
            || hash(&source.command.input)? != hash(&expected_query)?
            || node(&graph, &input_policy.capability_requirement_ref)?
                .operation
                .capability
                != source_requirement.capability
            || source.host_known_at_ms > c.knowledge_cutoff_ms
        {
            return Err("instrument_screen_input_binding_invalid".into());
        }
        let expiry = source
            .host_known_at_ms
            .checked_add(
                i64::try_from(input_policy.max_age_ms)
                    .map_err(|_| "instrument_screen_expiry_invalid")?,
            )
            .ok_or("instrument_screen_expiry_invalid")?;
        if now >= expiry {
            return Err("instrument_screen_input_stale".into());
        }
        expires = expires.min(expiry);
        let scope_value = source
            .response
            .payload
            .pointer(&input_policy.coverage.scope_pointer)
            .ok_or("instrument_screen_input_incomplete")?;
        if source
            .response
            .payload
            .pointer(&input_policy.coverage.complete_pointer)
            != Some(&Value::Bool(true))
            || hash(scope_value)? != hash(&input_policy.coverage.scope_value)?
        {
            return Err("instrument_screen_input_incomplete".into());
        }
        queries.insert(port.clone(), expected_query);
        inputs.insert(
            port.clone(),
            wire::ScreenInput {
                schema: SchemaIdentity {
                    schema_ref: source.command.response_schema_ref,
                    sha256: source.response_schema_sha256,
                },
                collection_id: source.collection_id,
                receipt_sha256: source.receipt_sha256,
                observed_at_ms: source.response.observed_at_ms,
                known_at_ms: source.host_known_at_ms,
                payload: source.response.payload,
            },
        );
    }
    let request = wire::ScreenRequest {
        schema_version: wire::ScreenRequestVersion::V1,
        semantics_version: p.semantics_version.clone(),
        policy_sha256: hash(&p)?,
        universe_definition_sha256: hash(
            &json!({"inputs":p.inputs,"queries":queries,"candidateInput":p.candidate_input,"recordsPointer":p.candidate_records_pointer,"idPointer":p.candidate_id_pointer}),
        )?,
        knowledge_cutoff_ms: c.knowledge_cutoff_ms,
        candidate_input: p.candidate_input.clone(),
        candidate_records_pointer: p.candidate_records_pointer.clone(),
        candidate_id_pointer: p.candidate_id_pointer.clone(),
        max_candidates: p.max_candidates,
        top_n: p.top_n,
        parameters,
        inputs,
    };
    request.validate()?;
    let allowed_contexts = spec
        .signal_market_requirements
        .iter()
        .flat_map(|m| &m.selectors)
        .filter(|s| s.selector_type == SelectorType::NormalizedInstrument)
        .filter_map(|s| s.normalized_id.clone())
        .collect();
    Ok(Prepared {
        policy: p,
        graph,
        selected,
        pin,
        contract_sha256: hash(&contract)?,
        request,
        asset_class,
        family,
        allowed_contexts,
        input_expires_at_ms: expires,
    })
}
pub(crate) fn command_binding(
    service: &TradeAssemblyService,
    c: &ScreenCommand,
) -> Result<String, String> {
    let p = prepare(service, c)?;
    hash(&(
        &p.policy,
        &p.pin,
        &p.contract_sha256,
        &p.request,
        &p.allowed_contexts,
        p.input_expires_at_ms,
    ))
}
fn validate_mode(c: &ScreenCommand, ctx: &SideEffectContext) -> Result<(), String> {
    if ctx.authority.account_mode != if c.mode == "live" { "live" } else { "paper" } {
        return Err("instrument_screen_mode_mismatch".into());
    }
    Ok(())
}
fn build_set(
    prepared: &Prepared,
    evidence: &ScreenOperationEvidence,
) -> Result<InstrumentSet, String> {
    // Anchor availability/TTL to the actual host-observed pure result. A retry
    // after failed persistence may reuse that result; it cannot renew its age.
    let known = evidence.response.observed_at_ms;
    let result = wire::decode_result(evidence.response.payload.clone())?;
    result.validate_against(&prepared.request)?;
    if result
        .members
        .iter()
        .any(|m| m.asset_class != prepared.asset_class || m.instrument_family != prepared.family)
    {
        return Err("instrument_screen_member_family_invalid".into());
    }
    if result.members.iter().any(|m| {
        m.context_instruments
            .iter()
            .any(|id| id != &m.instrument_id && !prepared.allowed_contexts.contains(id))
    }) {
        return Err("instrument_screen_context_undeclared".into());
    }
    let mut reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for rejected in &result.rejections {
        for reason in &rejected.reason_codes {
            reasons
                .entry(reason.clone())
                .or_default()
                .push(rejected.instrument_id.clone());
        }
    }
    let mut summaries = vec![];
    for (reason, mut ids) in reasons {
        ids.sort();
        summaries.push(RejectionSummary {
            reason_code: reason,
            count: ids.len() as u64,
            evidence_sha256: hash(&ids)?,
        });
    }
    let candidates = prepared.request.candidate_ids()?;
    let expiry = known
        .checked_add(
            i64::try_from(prepared.policy.ttl_ms)
                .map_err(|_| "instrument_screen_expiry_invalid")?,
        )
        .ok_or("instrument_screen_expiry_invalid")?
        .min(prepared.input_expires_at_ms);
    let set = InstrumentSet {
        schema_version: InstrumentSetVersion::V1,
        source: SelectionSource::Screener {
            receipt_sha256: hash(evidence)?,
            semantics_version: prepared.policy.semantics_version.clone(),
        },
        status: result.status,
        universe_definition_sha256: prepared.request.universe_definition_sha256.clone(),
        candidate_membership_sha256: hash(&candidates)?,
        ranking_policy_sha256: hash(
            &json!({"policy":prepared.request.policy_sha256,"package":prepared.pin.package_sha256,"semantics":prepared.policy.semantics_version}),
        )?,
        candidate_count: candidates.len() as u64,
        evaluated_at_ms: known,
        knowledge_cutoff_ms: prepared.request.knowledge_cutoff_ms,
        known_at_ms: known,
        expires_at_ms: expiry,
        data_receipt_sha256: prepared
            .request
            .inputs
            .values()
            .map(|i| i.receipt_sha256.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        members: result.members,
        rejection_summary: summaries,
        issues: result.issues,
    };
    set.validate()?;
    Ok(set)
}
pub fn current(service: &TradeAssemblyService, id: &str) -> Result<ScreenReceipt, String> {
    let value = service
        .runtime()
        .storage
        .get_json(RECEIPTS, id)?
        .ok_or("instrument_screen_missing")?;
    if serde_json::to_vec(&value)
        .map_err(|_| "instrument_screen_receipt_invalid")?
        .len()
        > 786432
    {
        return Err("instrument_screen_receipt_invalid".into());
    }
    let r: ScreenReceipt =
        serde_json::from_value(value).map_err(|_| "instrument_screen_receipt_invalid")?;
    if r.schema_version != "tradeassembly.instrument_screen_receipt.v1"
        || r.selection_id != id
        || identity(&r)? != r.receipt_sha256
        || id != format!("selection_{}", r.receipt_sha256)
    {
        return Err("instrument_screen_receipt_invalid".into());
    }
    let prepared = prepare(service, &r.command)?;
    if prepared.pin != r.operation.operation
        || prepared.contract_sha256 != r.operation.contract_sha256
        || prepared.request.policy_sha256 != r.operation.policy_sha256
        || hash(&prepared.request)? != r.operation.request_sha256
        || build_set(&prepared, &r.operation)? != r.instrument_set
    {
        return Err("instrument_screen_binding_stale".into());
    }
    if service.runtime().clock.now_ms() < r.instrument_set.known_at_ms {
        return Err("instrument_screen_not_yet_available".into());
    }
    if service.runtime().clock.now_ms() >= r.instrument_set.expires_at_ms {
        return Err("instrument_screen_stale".into());
    }
    Ok(r)
}
pub fn acquire(
    service: &TradeAssemblyService,
    c: ScreenCommand,
    ctx: &SideEffectContext,
) -> Result<ScreenReceipt, String> {
    validate_mode(&c, ctx)?;
    let prepared = prepare(service, &c)?;
    let key = hash(
        &json!({"strategy":c.scope.strategy_id,"authority":ctx.authority,"key":ctx.idempotency_key.as_str()}),
    )?;
    let command_sha = hash(&c)?;
    if let Some(previous) = service.runtime().storage.get_json(KEYS, &key)? {
        if previous["commandSha256"] != command_sha {
            return Err("instrument_screen_idempotency_conflict".into());
        }
        return current(
            service,
            previous["selectionId"]
                .as_str()
                .ok_or("instrument_screen_receipt_invalid")?,
        );
    }
    let correlation = hash(
        &json!({"command":command_sha,"key":key,"request":prepared.request,"pin":prepared.pin}),
    )?;
    let selected = &prepared.selected;
    let request = PluginOperationRequest {
        correlation_id: correlation.clone(),
        plugin_instance_ref: selected.plugin_instance_ref.clone(),
        plugin_ref: selected.plugin_ref.clone(),
        manifest_fingerprint: selected.manifest_fingerprint.clone(),
        operation_id: selected.operation_id.clone(),
        capability: selected.operation.capability.clone(),
        capability_graph_revision_id: c.capability_graph_revision.clone(),
        capability_graph_fingerprint: prepared.graph.graph_fingerprint.clone(),
        strategy_id: c.scope.strategy_id.clone(),
        strategy_version_id: c.scope.strategy_version_id.clone(),
        strategy_spec_hash: c.scope.strategy_spec_hash.clone(),
        activation_id: String::new(),
        attempt_id: correlation.clone(),
        evaluation_tick_id: correlation.clone(),
        mode: c.mode.clone(),
        purpose: selected.operation.purpose.clone(),
        account_ref: selected.account_ref.clone(),
        timeout_ms: 15000,
        fencing_token: None,
        input: serde_json::to_value(&prepared.request)
            .map_err(|_| "instrument_screen_request_invalid")?,
        evidence_refs: prepared
            .request
            .inputs
            .values()
            .map(|i| i.receipt_sha256.clone())
            .collect(),
    };
    let mut call = ctx.clone();
    call.idempotency_key = IdempotencyKey::new(format!("instrument-screen:{correlation}"))?;
    let response = service
        .runtime()
        .plugin_operations
        .invoke(&request, &call)?;
    let known = service.runtime().clock.now_ms();
    wire::payload_budget(
        &serde_json::to_value(&response).map_err(|_| "instrument_screen_response_invalid")?,
    )?;
    super::data_captures::capturable_payload(&response.payload)?;
    if response.correlation_id != correlation
        || response.schema_ref != wire::SCREEN_RESULT_SCHEMA
        || !response.deterministic
        || !response.replayable
        || response.reconciliation_required
        || response.freshness_state != "fresh"
        || response.source_event_id.is_empty()
        || response.content_hash.is_empty()
        || response.observed_at_ms < prepared.request.knowledge_cutoff_ms
        || response.observed_at_ms > known
    {
        return Err("instrument_screen_response_invalid".into());
    }
    let evidence = ScreenOperationEvidence {
        operation: prepared.pin.clone(),
        contract_sha256: prepared.contract_sha256.clone(),
        policy_sha256: prepared.request.policy_sha256.clone(),
        request_sha256: hash(&prepared.request)?,
        response,
    };
    let instrument_set = build_set(&prepared, &evidence)?;
    let fresh = prepare(service, &c)?;
    if fresh.pin != prepared.pin
        || fresh.contract_sha256 != prepared.contract_sha256
        || fresh.request != prepared.request
    {
        return Err("instrument_screen_binding_stale".into());
    }
    if service.runtime().clock.now_ms() >= instrument_set.expires_at_ms {
        return Err("instrument_screen_stale".into());
    }
    let mut r = ScreenReceipt {
        schema_version: "tradeassembly.instrument_screen_receipt.v1".into(),
        selection_id: String::new(),
        receipt_sha256: String::new(),
        command: c,
        operation: evidence,
        instrument_set,
    };
    r.receipt_sha256 = identity(&r)?;
    r.selection_id = format!("selection_{}", r.receipt_sha256);
    let value = serde_json::to_value(&r).map_err(|_| "instrument_screen_encoding_invalid")?;
    if serde_json::to_vec(&value)
        .map_err(|_| "instrument_screen_encoding_invalid")?
        .len()
        > 786432
    {
        return Err("instrument_screen_receipt_too_large".into());
    }
    if service
        .runtime()
        .storage
        .put_json_batch(
            &[
                StorageWrite::new(RECEIPTS, &r.selection_id, value, ctx.clone()),
                StorageWrite::new(
                    KEYS,
                    &key,
                    json!({"selectionId":r.selection_id,"commandSha256":command_sha}),
                    ctx.clone(),
                ),
            ],
            &[
                StorageExpectation::new(RECEIPTS, &r.selection_id, None),
                StorageExpectation::new(KEYS, &key, None),
            ],
        )
        .map_err(|_| "instrument_screen_commit_failed")?
        != ComparePutOutcome::Updated
    {
        return Err("instrument_screen_commit_conflict".into());
    }
    Ok(r)
}

fn context(service: &TradeAssemblyService, body: &Value) -> Result<SideEffectContext, String> {
    let envelope = crate::control_plane::http_envelope("http", "POST", CREATE_PATH, body)?;
    Ok(SideEffectContext::new(
        envelope.authority,
        IdempotencyKey::new(
            body["idempotencyKey"]
                .as_str()
                .ok_or("instrument_screen_idempotency_required")?,
        )
        .map_err(|_| "instrument_screen_idempotency_required")?,
    )
    .with_agent_execution(service.agent_mcp_execution_context()))
}
fn command(body: &Value) -> Result<ScreenCommand, String> {
    serde_json::from_value(body["screen"].clone())
        .map_err(|_| "instrument_screen_command_invalid".into())
}
fn error(code: &str) -> super::ServiceResponse {
    if matches!(
        code,
        "instrument_screen_command_invalid"
            | "instrument_screen_idempotency_required"
            | "instrument_screen_id_required"
            | "instrument_screen_not_available"
    ) {
        super::ServiceResponse::bad_request(code)
    } else {
        super::ServiceResponse::conflict(code)
    }
}
fn public_view(r: &ScreenReceipt) -> Value {
    json!({"selection":{"schemaVersion":"tradeassembly.instrument_screen_summary.v1","selectionId":r.selection_id,"receiptSha256":r.receipt_sha256,"scope":r.command.scope,"capabilityGraphRevision":r.command.capability_graph_revision,"mode":r.command.mode,"signalRequirementRef":r.command.signal_requirement_ref,"selectorRef":r.command.selector_ref,"pluginRef":r.operation.operation.plugin_ref,"operationId":r.operation.operation.operation_id,"packageSha256":r.operation.operation.package_sha256,"manifestSha256":r.operation.operation.manifest_sha256,"contractSha256":r.operation.contract_sha256,"policySha256":r.operation.policy_sha256},"instrumentSet":r.instrument_set,"executionAuthorityGranted":false})
}
pub fn create_response(service: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let result = (|| acquire(service, command(&body)?, &context(service, &body)?))();
    match result {
        Ok(r) => super::ServiceResponse::ok(public_view(&r)),
        Err(code) => error(&code),
    }
}
pub fn read_response(service: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let result = body["selectionId"]
        .as_str()
        .ok_or("instrument_screen_id_required".to_string())
        .and_then(|id| current(service, id).map_err(|_| "instrument_screen_not_available".into()));
    match result {
        Ok(r) => super::ServiceResponse::ok(public_view(&r)),
        Err(code) => error(&code),
    }
}
/// Recheck current facts before the outer command cache returns a prior set.
pub(crate) fn preflight(
    service: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<super::ServiceResponse> {
    if !matches!(path, CREATE_PATH | GET_PATH) {
        return None;
    }
    let result: Result<(), String> = (|| {
        if path == CREATE_PATH {
            let c = command(body)?;
            let ctx = context(service, body)?;
            validate_mode(&c, &ctx)?;
            prepare(service, &c)?;
            let key = hash(
                &json!({"strategy":c.scope.strategy_id,"authority":ctx.authority,"key":ctx.idempotency_key.as_str()}),
            )?;
            if let Some(previous) = service.runtime().storage.get_json(KEYS, &key)? {
                if previous["commandSha256"] != hash(&c)? {
                    return Err("instrument_screen_idempotency_conflict".into());
                }
                current(
                    service,
                    previous["selectionId"]
                        .as_str()
                        .ok_or("instrument_screen_receipt_invalid")?,
                )?;
            }
        } else {
            current(
                service,
                body["selectionId"]
                    .as_str()
                    .ok_or("instrument_screen_id_required")?,
            )
            .map_err(|_| "instrument_screen_not_available")?;
        }
        Ok(())
    })();
    result.err().map(|code| error(&code))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utc_date_bindings_are_explicit_calendar_days_and_do_not_override_literals() {
        use tradeassembly_strategy::spec::{QueryDateBasis, QueryDateBinding};
        let mut query = ScreenQuery {
            literal: BTreeMap::from([("symbol".into(), json!("SPY"))]),
            date_bindings: BTreeMap::from([
                (
                    "asOfDate".into(),
                    QueryDateBinding {
                        basis: QueryDateBasis::SelectionUtcDate,
                        offset_calendar_days: 0,
                    },
                ),
                (
                    "lower".into(),
                    QueryDateBinding {
                        basis: QueryDateBasis::SelectionUtcDate,
                        offset_calendar_days: 45,
                    },
                ),
                (
                    "upper".into(),
                    QueryDateBinding {
                        basis: QueryDateBasis::SelectionUtcDate,
                        offset_calendar_days: 60,
                    },
                ),
            ]),
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-12-31T23:59:59Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            materialize_query(&query, now).unwrap(),
            json!({"symbol":"SPY","asOfDate":"2026-12-31","lower":"2027-02-14","upper":"2027-03-01"})
        );
        query.literal.insert("lower".into(), json!("not_implicit"));
        assert!(materialize_query(&query, now).is_err());
    }
}
