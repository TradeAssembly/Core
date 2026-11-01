//! Immutable captured segments behind a stable, owned computation source.
//! A stream is evidence, never provider, strategy-entry, or brokerage authority.
use super::{data_captures, data_plans, TradeAssemblyService};
use crate::{data_pipeline::canonical_sha256, ports::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const STREAMS: &str = "managed_capture_streams_v1";
const HEADS: &str = "managed_capture_stream_heads_v1";
const INDEX: &str = "managed_capture_stream_index_v1";
const KEYS: &str = "managed_capture_stream_commands_v1";
const MAX_SEGMENTS: u64 = tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER;
const MAX_RECORD_BYTES: usize = 131_072;
pub(crate) const CREATE: &str = "/product/strategies/managed-data-streams/create";
pub(crate) const APPEND: &str = "/product/strategies/managed-data-streams/append";

use super::order_plans::management::automation::reversal::{self, Input, Origin};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Source {
    pub acquisition_node: String,
    pub interval_ms: u64,
    pub origin: Origin,
    pub parameter_extracts: BTreeMap<String, String>,
    pub input_template: BTreeMap<String, Input>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateCommand {
    pub schema_version: String,
    pub position_id: String,
    pub replay_node: String,
    pub acquisition_node: String,
    pub interval_ms: u64,
    pub origin: Origin,
    pub parameter_extracts: BTreeMap<String, String>,
    pub input_template: BTreeMap<String, Input>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AppendCommand {
    pub schema_version: String,
    pub stream_id: String,
    pub expected_head_sha256: String,
    pub capture_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Binding {
    pub schema_version: String,
    pub owner_sha256: String,
    pub scope: data_plans::PlanScope,
    pub position_id: String,
    pub position_source_sha256: String,
    pub owned_trade_instrument: String,
    pub graph_revision: String,
    pub source: Source,
    pub replay_node: String,
    pub seed_capture_id: String,
    pub seed_receipt_sha256: String,
    pub acquisition: super::data_binding::PinnedOperation,
    pub replay: super::data_binding::PinnedOperation,
    pub replay_contract_sha256: String,
    pub context: crate::data_pipeline::DataContext,
    pub original_origin_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Head {
    pub schema_version: String,
    pub stream_id: String,
    pub binding_sha256: String,
    pub sequence: u64,
    pub previous_head_sha256: Option<String>,
    pub capture_id: String,
    pub capture_receipt_sha256: String,
    pub event_start_ms: i64,
    pub event_end_ms: i64,
    pub admitted_at_ms: i64,
    pub head_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stream {
    binding: Binding,
    head: Head,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "managed_stream_encoding_invalid".into())
}
fn value<T: Serialize>(v: &T) -> Result<Value, String> {
    let v = serde_json::to_value(v).map_err(|_| "managed_stream_encoding_invalid")?;
    if serde_json::to_vec(&v)
        .map_err(|_| "managed_stream_encoding_invalid")?
        .len()
        > MAX_RECORD_BYTES
    {
        return Err("managed_stream_record_too_large".into());
    }
    Ok(v)
}
fn seal(head: &mut Head) -> Result<(), String> {
    head.head_sha256.clear();
    head.head_sha256 = hash(head)?;
    Ok(())
}
fn stream_id(binding: &Binding) -> Result<String, String> {
    Ok(format!("managedstream_{}", hash(binding)?))
}
fn owner(s: &TradeAssemblyService) -> Result<String, String> {
    hash(
        &s.invocation_owner()
            .ok_or("managed_stream_owner_required")?,
    )
}
fn index(id: &str, sequence: u64) -> String {
    format!("{id}:{sequence}")
}
fn read_head(s: &TradeAssemblyService, binding: &Binding, sequence: u64) -> Result<Head, String> {
    let id = stream_id(binding)?;
    let digest = s
        .runtime()
        .storage
        .get_json(INDEX, &index(&id, sequence))?
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or("managed_stream_segment_missing")?;
    let raw = s
        .runtime()
        .storage
        .get_json(HEADS, &digest)?
        .ok_or("managed_stream_segment_missing")?;
    let head: Head = serde_json::from_value(raw).map_err(|_| "managed_stream_head_invalid")?;
    let mut verified = head.clone();
    seal(&mut verified)?;
    if verified != head
        || head.schema_version != "tradeassembly.managed_data_stream_head.v1"
        || head.stream_id != id
        || head.sequence != sequence
        || head.binding_sha256 != hash(binding)?
        || head.head_sha256 != digest
        || head.event_start_ms >= head.event_end_ms
        || head.admitted_at_ms <= 0
        || head.admitted_at_ms > s.runtime().clock.trusted_now_ms()?
    {
        return Err("managed_stream_head_invalid".into());
    }
    Ok(head)
}
fn view(binding: &Binding, head: &Head) -> Result<Value, String> {
    Ok(
        json!({"streamId":head.stream_id,"bindingSha256":head.binding_sha256,
        "seedCaptureId":binding.seed_capture_id,"headSha256":head.head_sha256,
        "eventStartMs":binding.original_origin_ms,"originalOriginMs":binding.original_origin_ms,
        "eventEndMs":head.event_end_ms,"intervalMs":binding.source.interval_ms,
        "context":binding.context,"sequence":head.sequence,"executionAuthorityGranted":false}),
    )
}
impl Source {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.acquisition_node.is_empty()
            || self.acquisition_node.len() > 128
            || self.interval_ms == 0
            || self.interval_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
            || self.parameter_extracts.is_empty()
            || self.parameter_extracts.len() > 32
            || self.parameter_extracts.iter().any(|(k, p)| {
                k.is_empty()
                    || k.len() > 128
                    || !k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    || !p.starts_with('/')
                    || p.len() > 1024
            })
        {
            return Err("managed_stream_source_invalid".into());
        }
        if let Origin::Parameter(origin) = &self.origin {
            if origin.payload_pointer.len() > 1024
                || !origin.payload_pointer.starts_with('/')
                || self.parameter_extracts.get(&origin.replay_parameter)
                    != Some(&origin.payload_pointer)
            {
                return Err("managed_stream_origin_binding_invalid".into());
            }
        }
        reversal::validate_input_template(&self.input_template)?;
        Ok(())
    }
}
/// Pre-entry metadata readiness only. Actual captured origin/exit readiness is
/// independently required before financial admission; no provider is called here.
pub(in crate::service) fn validate_automatic_binding(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    graph: &str,
    replay_node: &str,
    raw: &Value,
) -> Result<(), String> {
    let source: Source =
        serde_json::from_value(raw.clone()).map_err(|_| "managed_stream_source_invalid")?;
    source.validate()?;
    let (spec, template) =
        super::selection_plans::template(s, scope)?.ok_or("managed_stream_template_required")?;
    for (alias, reference) in &template.selections {
        let used=template.nodes.iter().flat_map(|n|std::iter::once(&n.context).chain(n.input_contexts.values()))
            .any(|c|matches!(c,crate::selection_strategy::ContextTemplate::Selected{selection_ref,..} if selection_ref==alias));
        if used {
            let market = spec
                .signal_market_requirements
                .iter()
                .find(|m| m.requirement_id == reference.market_requirement_ref)
                .ok_or("managed_stream_context_binding_missing")?;
            let selector = market
                .selectors
                .iter()
                .find(|x| x.selector_id == reference.selector_ref)
                .ok_or("managed_stream_context_binding_missing")?;
            if matches!(
                selector.selector_type,
                tradeassembly_strategy::spec::SelectorType::Screened
            ) {
                return Err("managed_stream_screened_context_unqualified".into());
            }
        }
    }
    let node = template
        .nodes
        .iter()
        .find(|n| n.graph_node_id == replay_node)
        .ok_or("managed_stream_source_node_missing")?;
    if matches!(&node.context,crate::selection_strategy::ContextTemplate::Literal{context} if context.instruments.len()!=1)
    {
        return Err("managed_stream_single_instrument_context_required".into());
    }
    if template
        .bindings
        .iter()
        .any(|b| b.consumer_node == replay_node)
    {
        return Err("managed_stream_source_must_be_root".into());
    }
    let interval = match &node.context {
        crate::selection_strategy::ContextTemplate::Subject { interval_ms }
        | crate::selection_strategy::ContextTemplate::Selected { interval_ms, .. } => *interval_ms,
        crate::selection_strategy::ContextTemplate::Literal { context } => context.interval_ms,
    };
    if interval != Some(source.interval_ms) {
        return Err("managed_stream_interval_binding_invalid".into());
    }
    let epoch = chrono::DateTime::from_timestamp_millis(s.runtime().clock.trusted_now_ms()?)
        .ok_or("managed_stream_clock_invalid")?
        .to_rfc3339();
    let revision = super::capability_graph::revision_for_context(
        s,
        graph,
        &scope.strategy_id,
        &scope.strategy_version_id,
        "paper",
        &epoch,
    )
    .map_err(|_| "managed_stream_graph_not_current")?;
    if revision["strategySpecHash"] != scope.strategy_spec_hash {
        return Err("managed_stream_scope_mismatch".into());
    }
    let resolved: CapabilityGraphResolution = serde_json::from_value(revision["graph"].clone())
        .map_err(|_| "managed_stream_graph_invalid")?;
    let selection = |id: &str| {
        resolved
            .nodes
            .iter()
            .find(|n| n.node_id == id)
            .and_then(|n| n.selected.as_ref())
            .ok_or("managed_stream_source_unresolved")
    };
    let replay = selection(replay_node)?;
    let acquisition = selection(&source.acquisition_node)?;
    let pin = super::data_binding::pin(s, replay)?;
    super::data_binding::pin(s, acquisition)?;
    let contract = s
        .runtime()
        .plugin_packages
        .operation_data(&pin.package_sha256, &pin.operation_id)?
        .ok_or("managed_stream_source_contract_missing")?;
    if !contract.inputs.is_empty()
        || contract.state.is_some()
        || replay.operation.credential_grant_required
        || replay.operation.account_binding_required
        || !replay.operation.traits.deterministic
        || !replay.operation.traits.replayable
        || contract.outputs.is_empty()
        || contract
            .outputs
            .values()
            .any(|p| p.time.interval_ms != Some(source.interval_ms) || !p.time.completed_only)
    {
        return Err("managed_stream_pure_source_required".into());
    }
    Ok(())
}
fn verify_segment(
    binding: &Binding,
    receipt: &data_captures::CaptureReceipt,
) -> Result<(), String> {
    let p = &receipt.policy;
    if p.scope != binding.scope
        || p.mode != "paper"
        || p.capability_graph_revision != binding.graph_revision
        || p.acquisition_node != binding.source.acquisition_node
        || p.replay_node != binding.replay_node
        || p.parameter_extracts != binding.source.parameter_extracts
        || p.context != binding.context
        || receipt.acquisition != binding.acquisition
        || receipt.replay != binding.replay
        || receipt.replay_contract_sha256 != binding.replay_contract_sha256
        || p.context.interval_ms != Some(binding.source.interval_ms)
        || p.event_start_ms < binding.original_origin_ms
        || (p.event_start_ms - binding.original_origin_ms) % binding.source.interval_ms as i64 != 0
        || p.event_start_ms >= p.event_end_ms
        || (p.event_end_ms - p.event_start_ms) % binding.source.interval_ms as i64 != 0
    {
        return Err("managed_stream_segment_binding_changed".into());
    }
    if let Origin::Parameter(origin) = &binding.source.origin {
        let actual = receipt
            .replay_parameters
            .get(&origin.replay_parameter)
            .and_then(Value::as_i64)
            .ok_or("managed_stream_origin_unproven")?;
        // Explicit legacy replay origins may be segment-local or cumulative.
        if actual != p.event_start_ms && actual != binding.original_origin_ms {
            return Err("managed_stream_origin_changed".into());
        }
    }
    validate_capture_input(
        &binding.source,
        receipt,
        binding.original_origin_ms,
        &binding.owned_trade_instrument,
    )?;
    Ok(())
}
/// Verify the complete actual query, including feed/adjustment/literal fields.
/// Only declared time/symbol substitutions vary; an opaque hash is insufficient.
pub(crate) fn validate_capture_input(
    source: &Source,
    capture: &data_captures::CaptureReceipt,
    original_origin_ms: i64,
    owned_trade_instrument: &str,
) -> Result<(), String> {
    source.validate()?;
    if capture.schema_version != data_captures::CaptureReceiptVersion::V2 {
        return Err("managed_stream_input_witness_required".into());
    }
    let witness = data_captures::verified_input_witness(capture)?;
    if hash(&witness.input)? != capture.input_sha256 {
        return Err("managed_stream_input_witness_invalid".into());
    }
    let as_of = reversal::witness_as_of(
        &source.input_template,
        &witness.input,
        capture.policy.event_end_ms,
        capture.acquisition_response.observed_at_ms,
    )?;
    let window = tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
        event_start_ms: capture.policy.event_start_ms,
        event_end_ms: capture.policy.event_end_ms,
        as_of_ms: as_of.unwrap_or(capture.acquisition_response.observed_at_ms),
    };
    let expected = reversal::materialize_input(
        &source.input_template,
        source.interval_ms,
        owned_trade_instrument,
        &window,
        original_origin_ms,
    )?;
    if hash(&expected)? != capture.input_sha256 {
        return Err("managed_stream_query_configuration_changed".into());
    }
    Ok(())
}
fn head_for(
    binding: &Binding,
    receipt: &data_captures::CaptureReceipt,
    prior: Option<&Head>,
    now: i64,
) -> Result<Head, String> {
    verify_segment(binding, receipt)?;
    if let Some(prior) = prior {
        if receipt.policy.event_start_ms != prior.event_end_ms {
            return Err("managed_stream_noncontiguous_segment".into());
        }
    } else if receipt.capture_id != binding.seed_capture_id
        || receipt.receipt_sha256 != binding.seed_receipt_sha256
        || receipt.policy.event_start_ms != binding.original_origin_ms
    {
        return Err("managed_stream_seed_changed".into());
    }
    let sequence = prior.map_or(Ok(0), |p| {
        p.sequence
            .checked_add(1)
            .filter(|v| *v <= MAX_SEGMENTS)
            .ok_or("managed_stream_sequence_exhausted")
    })?;
    if now < receipt.replay_observed_at_ms || now < receipt.acquisition_response.observed_at_ms {
        return Err("managed_stream_capture_not_yet_available".into());
    }
    let mut head = Head {
        schema_version: "tradeassembly.managed_data_stream_head.v1".into(),
        stream_id: stream_id(binding)?,
        binding_sha256: hash(binding)?,
        sequence,
        previous_head_sha256: prior.map(|p| p.head_sha256.clone()),
        capture_id: receipt.capture_id.clone(),
        capture_receipt_sha256: receipt.receipt_sha256.clone(),
        event_start_ms: receipt.policy.event_start_ms,
        event_end_ms: receipt.policy.event_end_ms,
        admitted_at_ms: now,
        head_sha256: String::new(),
    };
    seal(&mut head)?;
    Ok(head)
}
fn stored(
    s: &TradeAssemblyService,
    id: &str,
    scope: &data_plans::PlanScope,
) -> Result<(Value, Stream), String> {
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| "managed_stream_not_available")?;
    let raw = s
        .runtime()
        .storage
        .get_json(STREAMS, id)?
        .ok_or("managed_stream_not_available")?;
    let stream: Stream =
        serde_json::from_value(raw.clone()).map_err(|_| "managed_stream_record_invalid")?;
    if stream.binding.scope != *scope
        || stream.binding.owner_sha256 != owner(s)?
        || stream.binding.schema_version != "tradeassembly.managed_data_stream_binding.v1"
        || stream.head.stream_id != id
        || stream_id(&stream.binding)? != id
        || stream.head.binding_sha256 != hash(&stream.binding)?
    {
        return Err("managed_stream_not_available".into());
    }
    let mut check = stream.head.clone();
    seal(&mut check)?;
    if check != stream.head {
        return Err("managed_stream_head_invalid".into());
    }
    let (_, graph, proof) =
        super::order_plans::management::exit_plan_subject(s, &stream.binding.position_id, scope)?;
    if graph != stream.binding.graph_revision || proof != stream.binding.position_source_sha256 {
        return Err("managed_stream_position_binding_changed".into());
    }
    let seed = data_captures::current(s, &stream.binding.seed_capture_id)?;
    verify_segment(&stream.binding, &seed)?;
    if seed.receipt_sha256 != stream.binding.seed_receipt_sha256 {
        return Err("managed_stream_seed_changed".into());
    }
    Ok((raw, stream))
}
pub(crate) fn binding(
    s: &TradeAssemblyService,
    id: &str,
    scope: &data_plans::PlanScope,
) -> Result<Binding, String> {
    Ok(stored(s, id, scope)?.1.binding)
}
/// Bind a new explicit managed V2 plan without putting a mutable stream head
/// in its identity. Immutable legacy acquisition bindings are unchanged.
pub(crate) fn bind_plan_sources(
    s: &TradeAssemblyService,
    content: &mut data_plans::DataPlanContent,
    streams: &BTreeMap<String, String>,
) -> Result<(), String> {
    let source = content
        .selection_source
        .as_ref()
        .ok_or("managed_stream_managed_plan_required")?;
    let position = source
        .managed_position_id
        .as_ref()
        .ok_or("managed_stream_managed_plan_required")?;
    let roots = content
        .plan
        .contracts
        .iter()
        .filter(|c| c.contract.inputs.is_empty())
        .map(|c| c.graph_node_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if streams.is_empty()
        || streams.len() > 8
        || streams
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            != roots
    {
        return Err("managed_stream_exact_roots_required".into());
    }
    for (node, id) in streams {
        let b = binding(s, id, &content.scope)?;
        let declared = content
            .plan
            .request
            .nodes
            .iter()
            .find(|n| &n.graph_node_id == node)
            .ok_or("managed_stream_source_node_missing")?;
        if b.position_id != *position
            || b.replay_node != *node
            || b.graph_revision != content.plan.request.capability_graph_revision
            || b.context != declared.context
            || content.plan.request.mode != "paper"
            || content.plan.request.source_captures.get(node) != Some(&b.seed_capture_id)
            || content.package_sha256.get(node) != Some(&b.replay.package_sha256)
            || content.binding_sha256.get(node) != Some(&b.replay.context_sha256)
            || !matches!(content.source_authentication.get(node),Some(data_captures::SourceAuthentication::HostAcquisition{capture_id,receipt_sha256,..}) if capture_id==&b.seed_capture_id && receipt_sha256==&b.seed_receipt_sha256)
        {
            return Err("managed_stream_plan_binding_invalid".into());
        }
        content.source_authentication.insert(
            node.clone(),
            data_captures::SourceAuthentication::HostCaptureStream {
                stream_id: id.clone(),
                binding_sha256: hash(&b)?,
            },
        );
    }
    Ok(())
}
pub(crate) fn plan_roots(plan: &data_plans::StoredDataPlan) -> BTreeMap<String, (String, String)> {
    plan.content
        .source_authentication
        .iter()
        .filter_map(|(node, auth)| match auth {
            data_captures::SourceAuthentication::HostCaptureStream {
                stream_id,
                binding_sha256,
            } => Some((node.clone(), (stream_id.clone(), binding_sha256.clone()))),
            _ => None,
        })
        .collect()
}
pub(crate) fn validate_plan_streams(
    s: &TradeAssemblyService,
    plan: &data_plans::StoredDataPlan,
) -> Result<(), String> {
    let roots = plan_roots(plan);
    if roots.is_empty() {
        return Ok(());
    }
    if roots.len() > 8 {
        return Err("managed_stream_source_budget_exceeded".into());
    }
    let position = plan
        .content
        .selection_source
        .as_ref()
        .and_then(|source| source.managed_position_id.as_ref())
        .ok_or("managed_stream_managed_plan_required")?;
    for (node, (id, expected)) in roots {
        let b = binding(s, &id, &plan.content.scope)?;
        let context = &plan
            .content
            .plan
            .request
            .nodes
            .iter()
            .find(|n| n.graph_node_id == node)
            .ok_or("managed_stream_source_node_missing")?
            .context;
        if hash(&b)? != expected
            || b.position_id != *position
            || b.replay_node != node
            || b.graph_revision != plan.content.plan.request.capability_graph_revision
            || b.context != *context
            || plan.content.plan.request.source_captures.get(&node) != Some(&b.seed_capture_id)
            || plan.content.package_sha256.get(&node) != Some(&b.replay.package_sha256)
            || plan.content.binding_sha256.get(&node) != Some(&b.replay.context_sha256)
        {
            return Err("managed_stream_plan_binding_invalid".into());
        }
    }
    Ok(())
}
pub(crate) fn validate_snapshot_plan(
    s: &TradeAssemblyService,
    plan: &data_plans::StoredDataPlan,
    snapshot: &Snapshot,
) -> Result<(), String> {
    let mut check = snapshot.clone();
    seal_snapshot(&mut check)?;
    let roots = plan_roots(plan);
    if check != *snapshot
        || snapshot.schema_version != "tradeassembly.managed_stream_input_snapshot.v1"
        || snapshot.owner_sha256 != owner(s)?
        || snapshot.plan_revision_id != plan.revision_id
        || roots.keys().ne(snapshot.sources.keys())
        || snapshot.window.as_of_ms > s.runtime().clock.trusted_now_ms()?
    {
        return Err("managed_stream_snapshot_plan_mismatch".into());
    }
    validate_plan_streams(s, plan)?;
    for (node, (id, expected)) in roots {
        let source = &snapshot.sources[&node];
        if source.stream_id != id || source.binding_sha256 != expected || source.segments.is_empty()
        {
            return Err("managed_stream_snapshot_plan_mismatch".into());
        }
        let b = binding(s, &id, &plan.content.scope)?;
        for head in &source.segments {
            if read_head(s, &b, head.sequence)? != *head
                || head.admitted_at_ms > snapshot.window.as_of_ms
            {
                return Err("managed_stream_snapshot_segment_changed".into());
            }
        }
    }
    Ok(())
}
fn creation_binding(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    c: &CreateCommand,
) -> Result<(Binding, data_captures::CaptureReceipt), String> {
    if c.schema_version != "tradeassembly.managed_data_stream_create.v1"
        || c.position_id.is_empty()
        || c.position_id.len() > 256
        || c.replay_node.is_empty()
        || c.replay_node.len() > 128
    {
        return Err("managed_stream_command_invalid".into());
    }
    let source = Source {
        acquisition_node: c.acquisition_node.clone(),
        interval_ms: c.interval_ms,
        origin: c.origin.clone(),
        parameter_extracts: c.parameter_extracts.clone(),
        input_template: c.input_template.clone(),
    };
    source.validate()?;
    let (plan, _, proof) =
        super::order_plans::management::entry_signal_source(s, &c.position_id, scope)?;
    let seed_id = match plan.content.source_authentication.get(&c.replay_node) {
        Some(data_captures::SourceAuthentication::HostAcquisition { capture_id, .. }) => capture_id,
        _ => return Err("managed_stream_entry_capture_required".into()),
    };
    let seed = data_captures::current(s, seed_id)?;
    let origin = seed.policy.event_start_ms;
    if let Origin::Parameter(parameter) = &source.origin {
        if seed
            .replay_parameters
            .get(&parameter.replay_parameter)
            .and_then(Value::as_i64)
            != Some(origin)
        {
            return Err("managed_stream_original_origin_unproven".into());
        }
    }
    if seed.policy.context.instruments.len() != 1 {
        return Err("managed_stream_original_origin_unproven".into());
    }
    let binding = Binding {
        schema_version: "tradeassembly.managed_data_stream_binding.v1".into(),
        owner_sha256: owner(s)?,
        scope: scope.clone(),
        position_id: c.position_id.clone(),
        position_source_sha256: proof,
        owned_trade_instrument: super::order_plans::management::exit_plan_subject(
            s,
            &c.position_id,
            scope,
        )?
        .0
        .instrument_id,
        graph_revision: plan.content.plan.request.capability_graph_revision.clone(),
        source,
        replay_node: c.replay_node.clone(),
        seed_capture_id: seed.capture_id.clone(),
        seed_receipt_sha256: seed.receipt_sha256.clone(),
        acquisition: seed.acquisition.clone(),
        replay: seed.replay.clone(),
        replay_contract_sha256: seed.replay_contract_sha256.clone(),
        context: seed.policy.context.clone(),
        original_origin_ms: origin,
    };
    validate_automatic_binding(
        s,
        scope,
        &binding.graph_revision,
        &binding.replay_node,
        &value(&binding.source)?,
    )?;
    Ok((binding, seed))
}
pub(crate) fn create(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    c: CreateCommand,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    if ctx.authority.account_mode != "paper" {
        return Err("managed_stream_paper_required".into());
    }
    let (binding, seed) = creation_binding(s, scope, &c)?;
    let head = head_for(&binding, &seed, None, s.runtime().clock.trusted_now_ms()?)?;
    let key = hash(
        &json!({"owner":binding.owner_sha256,"authority":ctx.authority,"key":ctx.idempotency_key.as_str(),"operation":"create"}),
    )?;
    let command = hash(&binding)?;
    if let Some(previous) = s.runtime().storage.get_json(KEYS, &key)? {
        if previous["commandSha256"] != command {
            return Err("managed_stream_idempotency_conflict".into());
        }
        stored(s, &head.stream_id, scope)?;
        return Ok(previous["response"].clone());
    }
    let response = view(&binding, &head)?;
    let stream = Stream {
        binding,
        head: head.clone(),
    };
    let writes = [
        StorageWrite::new(STREAMS, &head.stream_id, value(&stream)?, ctx.clone()),
        StorageWrite::new(HEADS, &head.head_sha256, value(&head)?, ctx.clone()),
        StorageWrite::new(
            INDEX,
            index(&head.stream_id, head.sequence),
            json!(head.head_sha256),
            ctx.clone(),
        ),
        StorageWrite::new(
            KEYS,
            &key,
            json!({"commandSha256":command,"response":response}),
            ctx.clone(),
        ),
    ];
    let expects = [
        StorageExpectation::new(STREAMS, &head.stream_id, None),
        StorageExpectation::new(HEADS, &head.head_sha256, None),
        StorageExpectation::new(INDEX, index(&head.stream_id, 0), None),
        StorageExpectation::new(KEYS, &key, None),
    ];
    if s.runtime().storage.put_json_batch(&writes, &expects)? != ComparePutOutcome::Updated {
        if let Some(previous) = s.runtime().storage.get_json(KEYS, &key)? {
            if previous["commandSha256"] == command {
                return Ok(previous["response"].clone());
            }
            return Err("managed_stream_idempotency_conflict".into());
        }
        return Err("managed_stream_create_conflict".into());
    }
    Ok(response)
}
fn request_scope(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Result<(data_plans::PlanScope, SideEffectContext), String> {
    if serde_json::to_vec(body)
        .map_err(|_| "managed_stream_command_invalid")?
        .len()
        > 131_072
    {
        return Err("managed_stream_command_too_large".into());
    }
    let scope: data_plans::PlanScope = serde_json::from_value(body["scope"].clone())
        .map_err(|_| "managed_stream_scope_required")?;
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| "managed_stream_not_available")?;
    let envelope = crate::control_plane::http_envelope("http", "POST", path, body)?;
    if envelope.authority.account_mode != "paper" {
        return Err("managed_stream_paper_required".into());
    }
    if let Some(context) = s.agent_mcp_execution_context() {
        if context.mode() != "paper" {
            return Err("agent_mcp_execution_context_mismatch".into());
        }
        let (_, actor) = s
            .responsible_human_identity()
            .ok_or("managed_stream_owner_required")?;
        crate::agent_runner::bind_mcp_execution_context(
            s.runtime().as_ref(),
            context,
            "tradeassembly.strategy.data_capture.create",
            json!({"mode":"paper","authority_context":{"actor":actor,"accountMode":"paper"}}),
        )?;
    }
    let key = body["idempotencyKey"]
        .as_str()
        .ok_or("managed_stream_idempotency_required")?;
    let context = SideEffectContext::new(envelope.authority, IdempotencyKey::new(key)?)
        .with_agent_execution(s.agent_mcp_execution_context());
    Ok((scope, context))
}
fn check_append(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    c: &AppendCommand,
    ctx: &SideEffectContext,
) -> Result<(), String> {
    if c.schema_version != "tradeassembly.managed_data_stream_append.v1" {
        return Err("managed_stream_command_invalid".into());
    }
    let (_, stream) = stored(s, &c.stream_id, scope)?;
    let key = hash(
        &json!({"owner":stream.binding.owner_sha256,"authority":ctx.authority,"key":ctx.idempotency_key.as_str(),"operation":"append"}),
    )?;
    if let Some(previous) = s.runtime().storage.get_json(KEYS, &key)? {
        if previous["commandSha256"] != hash(c)? {
            return Err("managed_stream_idempotency_conflict".into());
        }
        return Ok(());
    }
    if stream.head.head_sha256 != c.expected_head_sha256 {
        return Err("managed_stream_head_conflict".into());
    }
    head_for(
        &stream.binding,
        &data_captures::current(s, &c.capture_id)?,
        Some(&stream.head),
        s.runtime().clock.trusted_now_ms()?,
    )?;
    Ok(())
}
pub(crate) fn preflight(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<super::ServiceResponse> {
    if path != CREATE && path != APPEND {
        return None;
    }
    let result = (|| {
        let (scope, ctx) = request_scope(s, path, body)?;
        if path == CREATE {
            let c: CreateCommand = serde_json::from_value(body["stream"].clone())
                .map_err(|_| "managed_stream_command_invalid")?;
            creation_binding(s, &scope, &c)?;
        } else {
            let c: AppendCommand = serde_json::from_value(body["append"].clone())
                .map_err(|_| "managed_stream_command_invalid")?;
            check_append(s, &scope, &c, &ctx)?;
        }
        Ok::<_, String>(())
    })();
    result.err().map(|code| {
        if code == "managed_stream_not_available" {
            super::ServiceResponse::object_unavailable()
        } else {
            super::ServiceResponse::conflict(&code)
        }
    })
}
pub(crate) fn response(
    s: &TradeAssemblyService,
    path: &str,
    body: Value,
) -> super::ServiceResponse {
    let result = (|| {
        let (scope, ctx) = request_scope(s, path, &body)?;
        if path == CREATE {
            create(
                s,
                &scope,
                serde_json::from_value(body["stream"].clone())
                    .map_err(|_| "managed_stream_command_invalid")?,
                &ctx,
            )
        } else if path == APPEND {
            append(
                s,
                &scope,
                serde_json::from_value(body["append"].clone())
                    .map_err(|_| "managed_stream_command_invalid")?,
                &ctx,
            )
        } else {
            Err("managed_stream_route_invalid".into())
        }
    })();
    match result {
        Ok(stream) => {
            super::ServiceResponse::ok(json!({"stream":stream,"executionAuthorityGranted":false}))
        }
        Err(code) if code == "managed_stream_not_available" => {
            super::ServiceResponse::object_unavailable()
        }
        Err(code) => super::ServiceResponse::conflict(&code),
    }
}
pub(crate) fn append(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    c: AppendCommand,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    if c.schema_version != "tradeassembly.managed_data_stream_append.v1"
        || ctx.authority.account_mode != "paper"
    {
        return Err("managed_stream_command_invalid".into());
    }
    let (raw, stream) = stored(s, &c.stream_id, scope)?;
    let command = hash(&c)?;
    let key = hash(
        &json!({"owner":stream.binding.owner_sha256,"authority":ctx.authority,"key":ctx.idempotency_key.as_str(),"operation":"append"}),
    )?;
    if let Some(previous) = s.runtime().storage.get_json(KEYS, &key)? {
        if previous["commandSha256"] != command {
            return Err("managed_stream_idempotency_conflict".into());
        }
        return Ok(previous["response"].clone());
    }
    if stream.head.head_sha256 != c.expected_head_sha256 {
        return Err("managed_stream_head_conflict".into());
    }
    let capture = data_captures::current(s, &c.capture_id)?;
    let head = head_for(
        &stream.binding,
        &capture,
        Some(&stream.head),
        s.runtime().clock.trusted_now_ms()?,
    )?;
    let response = view(&stream.binding, &head)?;
    let next = Stream {
        binding: stream.binding,
        head: head.clone(),
    };
    let writes = [
        StorageWrite::new(STREAMS, &c.stream_id, value(&next)?, ctx.clone()),
        StorageWrite::new(HEADS, &head.head_sha256, value(&head)?, ctx.clone()),
        StorageWrite::new(
            INDEX,
            index(&c.stream_id, head.sequence),
            json!(head.head_sha256),
            ctx.clone(),
        ),
        StorageWrite::new(
            KEYS,
            &key,
            json!({"commandSha256":command,"response":response}),
            ctx.clone(),
        ),
    ];
    let expects = [
        StorageExpectation::new(STREAMS, &c.stream_id, Some(raw)),
        StorageExpectation::new(HEADS, &head.head_sha256, None),
        StorageExpectation::new(INDEX, index(&c.stream_id, head.sequence), None),
        StorageExpectation::new(KEYS, &key, None),
    ];
    if s.runtime().storage.put_json_batch(&writes, &expects)? != ComparePutOutcome::Updated {
        if let Some(previous) = s.runtime().storage.get_json(KEYS, &key)? {
            if previous["commandSha256"] == command {
                return Ok(previous["response"].clone());
            }
            return Err("managed_stream_idempotency_conflict".into());
        }
        return Err("managed_stream_append_conflict".into());
    }
    Ok(response)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NodeSnapshot {
    pub stream_id: String,
    pub binding_sha256: String,
    pub admitted_head_sha256: String,
    pub segments: Vec<Head>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub schema_version: String,
    pub owner_sha256: String,
    pub plan_revision_id: String,
    pub window: tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
    pub sources: BTreeMap<String, NodeSnapshot>,
    pub snapshot_sha256: String,
}
#[derive(Clone)]
pub(crate) struct ReplayPart {
    pub head: Head,
    pub capture: data_captures::CaptureReceipt,
    pub window: tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
}
/// Materialize no parameters: every plugin receives the exact parameters of
/// its own verified capture, with a bounded intersecting requested interval.
pub(crate) fn replay_parts(
    s: &TradeAssemblyService,
    snapshot: &Snapshot,
    node: &str,
) -> Result<Vec<ReplayPart>, String> {
    let mut check = snapshot.clone();
    seal_snapshot(&mut check)?;
    if check != *snapshot || snapshot.owner_sha256 != owner(s)? {
        return Err("managed_stream_snapshot_invalid".into());
    }
    let source = snapshot
        .sources
        .get(node)
        .ok_or("managed_stream_snapshot_source_missing")?;
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for head in &source.segments {
        let capture = data_captures::current(s, &head.capture_id)?;
        if capture.receipt_sha256 != head.capture_receipt_sha256
            || head.stream_id != source.stream_id
            || head.binding_sha256 != source.binding_sha256
            || capture.policy.event_start_ms != head.event_start_ms
            || capture.policy.event_end_ms != head.event_end_ms
        {
            return Err("managed_stream_segment_changed".into());
        }
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&capture)
                    .map_err(|_| "managed_stream_capture_invalid")?
                    .len(),
            )
            .ok_or("managed_stream_source_budget_exceeded")?;
        if bytes > 8 * 1024 * 1024 {
            return Err("managed_stream_source_budget_exceeded".into());
        }
        let window = tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
            event_start_ms: head.event_start_ms.max(snapshot.window.event_start_ms),
            event_end_ms: head.event_end_ms.min(snapshot.window.event_end_ms),
            as_of_ms: snapshot.window.as_of_ms,
        };
        if window.event_start_ms >= window.event_end_ms || head.admitted_at_ms > window.as_of_ms {
            return Err("managed_stream_snapshot_window_invalid".into());
        }
        result.push(ReplayPart {
            head: head.clone(),
            capture,
            window,
        });
    }
    Ok(result)
}
pub(crate) fn envelope_part(
    part: &ReplayPart,
    response: &mut tradeassembly_plugin_sdk::data_evaluation::DataEvaluationResponse,
) -> Result<(), String> {
    use tradeassembly_plugin_sdk::data_evaluation::point_sha256;
    if !response.state.is_empty()
        || response
            .outputs
            .keys()
            .ne(part.capture.replay_response.outputs.keys())
    {
        return Err("managed_stream_replay_mismatch".into());
    }
    for (port, batch) in &mut response.outputs {
        let expected = &part.capture.replay_response.outputs[port];
        if batch.schema != expected.schema || batch.shape != expected.shape {
            return Err("managed_stream_replay_mismatch".into());
        }
        let points = expected
            .points
            .iter()
            .filter(|p| {
                p.provenance.event_start_ms >= part.window.event_start_ms
                    && p.provenance.event_end_ms <= part.window.event_end_ms
            })
            .collect::<Vec<_>>();
        if batch.points.len() != points.len()
            || batch.points.iter().zip(points).any(|(a, b)| a != b)
        {
            return Err("managed_stream_replay_mismatch".into());
        }
        for point in &mut batch.points {
            let raw = point.provenance.content_sha256.clone();
            point.provenance.known_at_ms = point
                .provenance
                .known_at_ms
                .max(part.capture.acquisition_response.observed_at_ms)
                .max(part.capture.replay_observed_at_ms)
                .max(part.head.admitted_at_ms);
            point.provenance.arrived_at_ms = point
                .provenance
                .arrived_at_ms
                .max(point.provenance.known_at_ms);
            if point.provenance.arrived_at_ms > part.window.as_of_ms {
                return Err("managed_stream_source_not_yet_available".into());
            }
            point.provenance.upstream_content_sha256.extend([
                raw,
                part.capture.receipt_sha256.clone(),
                part.head.head_sha256.clone(),
                part.head.binding_sha256.clone(),
            ]);
            point.provenance.upstream_content_sha256.sort();
            point.provenance.upstream_content_sha256.dedup();
            point.provenance.content_sha256 =
                point_sha256(point).map_err(|_| "managed_stream_point_invalid")?;
        }
    }
    Ok(())
}
fn seal_snapshot(s: &mut Snapshot) -> Result<(), String> {
    s.snapshot_sha256.clear();
    s.snapshot_sha256 = hash(s)?;
    Ok(())
}
/// Seal the exact selected input head(s) under the original evaluation key
/// BEFORE source invocation. This is a retained command input, not a checkpoint
/// or an output commit; failures cannot replace it with a newer observation.
pub(crate) fn retain_snapshot(
    s: &TradeAssemblyService,
    request_sha256: &str,
    snapshot: Snapshot,
    ctx: &SideEffectContext,
) -> Result<Snapshot, String> {
    const SNAPSHOTS: &str = "managed_stream_step_snapshots_v1";
    let key = hash(
        &json!({"owner":snapshot.owner_sha256,"plan":snapshot.plan_revision_id,"authority":ctx.authority,"key":ctx.idempotency_key.as_str()}),
    )?;
    let encoded = json!({"schemaVersion":"tradeassembly.managed_stream_original_step.v1","requestSha256":request_sha256,"snapshot":snapshot});
    if serde_json::to_vec(&encoded)
        .map_err(|_| "managed_stream_snapshot_invalid")?
        .len()
        > 1_048_576
    {
        return Err("managed_stream_snapshot_too_large".into());
    }
    if let Some(raw) = s.runtime().storage.get_json(SNAPSHOTS, &key)? {
        if raw != encoded {
            return Err("managed_stream_original_snapshot_changed".into());
        }
        return Ok(snapshot);
    }
    let outcome = s.runtime().storage.put_json_batch(
        &[StorageWrite::new(
            SNAPSHOTS,
            &key,
            encoded.clone(),
            ctx.clone(),
        )],
        &[StorageExpectation::new(SNAPSHOTS, &key, None)],
    )?;
    if outcome != ComparePutOutcome::Updated
        && s.runtime().storage.get_json(SNAPSHOTS, &key)? != Some(encoded)
    {
        return Err("managed_stream_original_snapshot_changed".into());
    }
    Ok(snapshot)
}
/// Read-only selection of immutable input evidence. An expected old head remains
/// valid after a later append; it can never select the later segment implicitly.
pub(crate) fn snapshot(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    plan_revision_id: &str,
    roots: &BTreeMap<String, (String, String)>,
    heads: &BTreeMap<String, String>,
    window: &tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
) -> Result<Snapshot, String> {
    if roots.is_empty()
        || roots.len() > 8
        || roots.keys().ne(heads.keys())
        || window.event_start_ms >= window.event_end_ms
        || window.event_end_ms > window.as_of_ms
        || window.as_of_ms > s.runtime().clock.trusted_now_ms()?
    {
        return Err("managed_stream_snapshot_invalid".into());
    }
    let mut sources = BTreeMap::new();
    let mut source_bytes = 0usize;
    for (node, (id, binding_sha)) in roots {
        let (_, stream) = stored(s, id, scope)?;
        if hash(&stream.binding)? != *binding_sha {
            return Err("managed_stream_binding_changed".into());
        }
        let raw = s
            .runtime()
            .storage
            .get_json(HEADS, &heads[node])?
            .ok_or("managed_stream_head_missing")?;
        let expected: Head =
            serde_json::from_value(raw).map_err(|_| "managed_stream_head_invalid")?;
        if expected.sequence > stream.head.sequence
            || read_head(s, &stream.binding, expected.sequence)? != expected
            || expected.head_sha256 != heads[node]
            || expected.admitted_at_ms > window.as_of_ms
            || window.event_start_ms < stream.binding.original_origin_ms
            || window.event_end_ms > expected.event_end_ms
            || (window.event_start_ms - stream.binding.original_origin_ms)
                % stream.binding.source.interval_ms as i64
                != 0
            || (window.event_end_ms - window.event_start_ms)
                % stream.binding.source.interval_ms as i64
                != 0
            || (window.event_end_ms - window.event_start_ms)
                / stream.binding.source.interval_ms as i64
                > 256
        {
            return Err("managed_stream_window_outside_admission".into());
        }
        let (mut lo, mut hi) = (0, expected.sequence);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if read_head(s, &stream.binding, mid)?.event_end_ms <= window.event_start_ms {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut cursor = window.event_start_ms;
        let mut segments = Vec::new();
        let mut previous = None;
        for sequence in lo..=expected.sequence {
            let head = read_head(s, &stream.binding, sequence)?;
            if head.event_start_ms > cursor
                || head.event_end_ms <= cursor
                || head.admitted_at_ms > window.as_of_ms
                || previous
                    .as_ref()
                    .is_some_and(|sha| head.previous_head_sha256.as_ref() != Some(sha))
            {
                return Err("managed_stream_segment_chain_invalid".into());
            }
            if sequence > 0 && previous.is_none() {
                let before = read_head(s, &stream.binding, sequence - 1)?;
                if head.previous_head_sha256.as_ref() != Some(&before.head_sha256)
                    || before.event_end_ms != head.event_start_ms
                {
                    return Err("managed_stream_segment_chain_invalid".into());
                }
            }
            let capture = data_captures::current(s, &head.capture_id)?;
            verify_segment(&stream.binding, &capture)?;
            if capture.receipt_sha256 != head.capture_receipt_sha256
                || capture.policy.event_start_ms != head.event_start_ms
                || capture.policy.event_end_ms != head.event_end_ms
                || capture.acquisition_response.observed_at_ms > window.as_of_ms
                || capture.replay_observed_at_ms > window.as_of_ms
            {
                return Err("managed_stream_segment_changed".into());
            }
            source_bytes = source_bytes
                .checked_add(
                    serde_json::to_vec(&capture)
                        .map_err(|_| "managed_stream_capture_invalid")?
                        .len(),
                )
                .ok_or("managed_stream_source_budget_exceeded")?;
            if source_bytes > 8 * 1024 * 1024 || segments.len() >= 256 {
                return Err("managed_stream_source_budget_exceeded".into());
            }
            cursor = head.event_end_ms.min(window.event_end_ms);
            previous = Some(head.head_sha256.clone());
            segments.push(head);
            if cursor == window.event_end_ms {
                break;
            }
        }
        if cursor != window.event_end_ms {
            return Err("managed_stream_coverage_gap".into());
        }
        sources.insert(
            node.clone(),
            NodeSnapshot {
                stream_id: id.clone(),
                binding_sha256: binding_sha.clone(),
                admitted_head_sha256: heads[node].clone(),
                segments,
            },
        );
    }
    let mut result = Snapshot {
        schema_version: "tradeassembly.managed_stream_input_snapshot.v1".into(),
        owner_sha256: owner(s)?,
        plan_revision_id: plan_revision_id.into(),
        window: window.clone(),
        sources,
        snapshot_sha256: String::new(),
    };
    seal_snapshot(&mut result)?;
    if serde_json::to_vec(&result)
        .map_err(|_| "managed_stream_snapshot_invalid")?
        .len()
        > 1_048_576
    {
        return Err("managed_stream_snapshot_too_large".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tradeassembly_plugin_sdk::data_evaluation::{DataEvaluationResponse, EvaluationWindow};
    fn source() -> Source {
        serde_json::from_value(json!({"acquisitionNode":"acquire","intervalMs":1000,
            "origin":{"kind":"admitted_capture_start"},"parameterExtracts":{"bars":"/bars"},
            "inputTemplate":{"symbol":{"kind":"trade_instrument_id"},"start":{"kind":"event_start_ms"},
                "end":{"kind":"event_end_ms"},"feed":{"kind":"literal","value":"feed-a"}}}))
        .unwrap()
    }
    fn capture(start: i64, end: i64) -> data_captures::CaptureReceipt {
        let (start, end) = (start + 1000, end + 1000);
        let source = source();
        let input = reversal::materialize_input(
            &source.input_template,
            1000,
            "BTC/USD",
            &EvaluationWindow {
                event_start_ms: start,
                event_end_ms: end,
                as_of_ms: end + 100,
            },
            1000,
        )
        .unwrap();
        let pin = json!({"pluginInstanceRef":"instance","pluginRef":"provider","operationId":"operation","packageSha256":"a".repeat(64),"manifestSha256":"b".repeat(64),"contextSha256":"c".repeat(64),"accountRef":null});
        serde_json::from_value(json!({"schemaVersion":"tradeassembly.data_capture_receipt.v2","captureId":format!("capture-{start}"),"receiptSha256":format!("receipt-{start}"),
            "policy":{"scope":{"strategyId":"strategy","strategyVersionId":"version","strategySpecHash":"sha256:scope"},"capabilityGraphRevision":"graph","mode":"paper","acquisitionNode":"acquire","replayNode":"replay","parameterExtracts":source.parameter_extracts,
            "context":{"instruments":[{"instrumentId":"BTC/USD","assetClass":"crypto","instrumentFamily":"crypto_spot"}],"intervalMs":1000},
            "eventStartMs":start,"eventEndMs":end,"gapPolicy":"strict_fixed_interval"},
            "inputSha256":hash(&input).unwrap(),"inputWitness":{"schemaVersion":"tradeassembly.data_capture_input_witness.v1","input":input},
            "acquisition":pin,"replay":pin,"replayContractSha256":"d".repeat(64),
            "acquisitionResponse":{"schemaRef":"schema://capture","payload":{},"observedAtMs":end+100,"sourceEventId":"source","contentHash":"hash","freshnessState":"fresh","deterministic":false,"replayable":false,"reconciliationRequired":false},
            "payloadSha256":"payload","replayParameters":{"bars":[]},
            "replayResponse":{"schemaVersion":"tradeassembly.data_evaluation.v1","outputs":{},"state":{}},
            "replayAsOfMs":end+100,"replayObservedAtMs":end+100,"replayPayloadSha256":"replay","replayContentHash":"replay","replaySourceEventId":"replay"})).unwrap()
    }
    fn binding(c: &data_captures::CaptureReceipt) -> Binding {
        Binding {
            schema_version: "tradeassembly.managed_data_stream_binding.v1".into(),
            owner_sha256: "owner".into(),
            scope: c.policy.scope.clone(),
            position_id: "position".into(),
            position_source_sha256: "proof".into(),
            owned_trade_instrument: "BTC/USD".into(),
            graph_revision: "graph".into(),
            source: source(),
            replay_node: "replay".into(),
            seed_capture_id: c.capture_id.clone(),
            seed_receipt_sha256: c.receipt_sha256.clone(),
            acquisition: c.acquisition.clone(),
            replay: c.replay.clone(),
            replay_contract_sha256: c.replay_contract_sha256.clone(),
            context: c.policy.context.clone(),
            original_origin_ms: 1000,
        }
    }
    #[test]
    fn query_continuity_rejects_feed_symbol_and_unwitnessed_inputs() {
        let source = source();
        let original = capture(0, 1000);
        validate_capture_input(&source, &original, 1000, "BTC/USD").unwrap();
        for (field, value) in [("feed", json!("feed-b")), ("symbol", json!("ETH/USD"))] {
            let mut changed = original.clone();
            let input = &mut changed.input_witness.as_mut().unwrap().input;
            input[field] = value;
            changed.input_sha256 = hash(input).unwrap();
            assert_eq!(
                validate_capture_input(&source, &changed, 1000, "BTC/USD").unwrap_err(),
                "managed_stream_query_configuration_changed"
            );
        }
        let mut legacy = original;
        legacy.schema_version = data_captures::CaptureReceiptVersion::V1;
        legacy.input_witness = None;
        assert_eq!(
            validate_capture_input(&source, &legacy, 1000, "BTC/USD").unwrap_err(),
            "managed_stream_input_witness_required"
        );
    }
    #[test]
    fn nested_formatted_witness_binds_exact_query_and_all_as_of_leaves() {
        let mut source = source();
        source.input_template=serde_json::from_value(json!({
            "symbol":{"kind":"trade_instrument_id"},"feed":{"kind":"literal","value":"feed-a"},
            "query":{"kind":"object","fields":{
                "start":{"kind":"window_timestamp","basis":"event_start","format":"rfc3339_utc_millis"},
                "end":{"kind":"window_timestamp","basis":"event_end","format":"rfc3339_utc_millis"},
                "asOf":{"kind":"window_timestamp","basis":"as_of","format":"rfc3339_utc_millis"}}},
            "cutoff":{"kind":"as_of_ms"}})).unwrap();
        let mut original = capture(0, 1000);
        let input = reversal::materialize_input(
            &source.input_template,
            1000,
            "BTC/USD",
            &EvaluationWindow {
                event_start_ms: 1000,
                event_end_ms: 2000,
                as_of_ms: 2100,
            },
            1000,
        )
        .unwrap();
        original.input_sha256 = hash(&input).unwrap();
        original.input_witness.as_mut().unwrap().input = input;
        validate_capture_input(&source, &original, 1000, "BTC/USD").unwrap();
        for (field, value, error) in [
            (
                "feed",
                json!("feed-b"),
                "managed_stream_query_configuration_changed",
            ),
            (
                "symbol",
                json!("OTHER"),
                "managed_stream_query_configuration_changed",
            ),
            ("cutoff", json!(2101), "managed_stream_query_time_invalid"),
        ] {
            let mut changed = original.clone();
            changed.input_witness.as_mut().unwrap().input[field] = value;
            changed.input_sha256 = hash(&changed.input_witness.as_ref().unwrap().input).unwrap();
            assert_eq!(
                validate_capture_input(&source, &changed, 1000, "BTC/USD").unwrap_err(),
                error
            );
        }
        for (field, value, error) in [
            (
                "start",
                json!("1970-01-01T00:00:01.001Z"),
                "managed_stream_query_configuration_changed",
            ),
            (
                "asOf",
                json!("1970-01-01T00:00:02.100+00:00"),
                "managed_stream_query_time_invalid",
            ),
            (
                "asOf",
                json!("1970-01-01T00:00:02.101Z"),
                "managed_stream_query_time_invalid",
            ),
            (
                "apiSecret",
                json!("never-retained"),
                "data_capture_input_credential_material_forbidden",
            ),
        ] {
            let mut changed = original.clone();
            changed.input_witness.as_mut().unwrap().input["query"][field] = value;
            changed.input_sha256 = hash(&changed.input_witness.as_ref().unwrap().input).unwrap();
            assert_eq!(
                validate_capture_input(&source, &changed, 1000, "BTC/USD").unwrap_err(),
                error
            );
        }
    }
    #[test]
    fn heads_bind_contiguous_segments_and_actual_availability() {
        let first = capture(0, 1000);
        let binding = binding(&first);
        let h0 = head_for(&binding, &first, None, 2100).unwrap();
        let next = capture(1000, 2000);
        let h1 = head_for(&binding, &next, Some(&h0), 3100).unwrap();
        assert_eq!(h1.previous_head_sha256, Some(h0.head_sha256.clone()));
        assert_eq!(h1.sequence, 1);
        assert_ne!(h1.head_sha256, h0.head_sha256);
        assert_eq!(
            head_for(&binding, &capture(2000, 3000), Some(&h1), 4099).unwrap_err(),
            "managed_stream_capture_not_yet_available"
        );
        assert_eq!(
            head_for(&binding, &capture(3000, 4000), Some(&h1), 5100).unwrap_err(),
            "managed_stream_noncontiguous_segment"
        );
        let mut changed = next;
        changed.replay.context_sha256 = "changed".into();
        assert_eq!(
            head_for(&binding, &changed, Some(&h0), 3100).unwrap_err(),
            "managed_stream_segment_binding_changed"
        );
    }
    #[test]
    fn replay_envelope_preserves_values_and_adds_host_causality() {
        let mut capture = capture(0, 1000);
        let mut response:DataEvaluationResponse=serde_json::from_value(json!({"schemaVersion":"tradeassembly.data_evaluation.v1","state":{},"outputs":{"bars":{"schema":{"schemaRef":"schema://bars","sha256":"e".repeat(64)},"shape":"series","points":[{"instrumentId":"BTC/USD","sourceEventId":"bar-1","provenance":{"contentSha256":"raw","upstreamContentSha256":[],"eventStartMs":1000,"eventEndMs":2000,"knownAtMs":2000,"arrivedAtMs":2000,"completed":true,"sourceRevision":"v1"},"result":{"state":"ready","value":{"close":100}}}]}}})).unwrap();
        capture.replay_response = response.clone();
        let head = head_for(&binding(&capture), &capture, None, 2200).unwrap();
        let part = ReplayPart {
            capture,
            head,
            window: EvaluationWindow {
                event_start_ms: 1000,
                event_end_ms: 2000,
                as_of_ms: 2200,
            },
        };
        envelope_part(&part, &mut response).unwrap();
        let p = &response.outputs["bars"].points[0];
        assert_eq!(p.provenance.known_at_ms, 2200);
        assert_eq!(p.provenance.arrived_at_ms, 2200);
        assert_eq!(
            p.result,
            part.capture.replay_response.outputs["bars"].points[0].result
        );
        assert!(p
            .provenance
            .upstream_content_sha256
            .contains(&part.head.head_sha256));
        let mut changed = part.capture.replay_response.clone();
        changed.outputs.get_mut("bars").unwrap().points.clear();
        assert_eq!(
            envelope_part(&part, &mut changed).unwrap_err(),
            "managed_stream_replay_mismatch"
        );
        let mut early = part.clone();
        early.window.as_of_ms = 2199;
        assert_eq!(
            envelope_part(&early, &mut part.capture.replay_response.clone()).unwrap_err(),
            "managed_stream_source_not_yet_available"
        );
    }
    #[test]
    fn original_step_snapshot_is_sealed_across_service_recreation() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("stream-retry.db");
        let path = db.to_str().unwrap();
        let ctx = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("same-original-step").unwrap(),
        );
        let first = TradeAssemblyService::test_local(path).for_authenticated_invocation(
            "test",
            "stream-owner",
            None,
            None,
        );
        let mut snapshot = Snapshot {
            schema_version: "tradeassembly.managed_stream_input_snapshot.v1".into(),
            owner_sha256: owner(&first).unwrap(),
            plan_revision_id: "owned-plan".into(),
            window: EvaluationWindow {
                event_start_ms: 0,
                event_end_ms: 1000,
                as_of_ms: 1100,
            },
            sources: BTreeMap::new(),
            snapshot_sha256: String::new(),
        };
        seal_snapshot(&mut snapshot).unwrap();
        assert_eq!(
            retain_snapshot(&first, "request-a", snapshot.clone(), &ctx).unwrap(),
            snapshot
        );
        drop(first);
        let next = TradeAssemblyService::test_local(path).for_authenticated_invocation(
            "test",
            "stream-owner",
            None,
            None,
        );
        assert_eq!(
            retain_snapshot(&next, "request-a", snapshot.clone(), &ctx).unwrap(),
            snapshot
        );
        assert_eq!(
            retain_snapshot(&next, "request-b", snapshot.clone(), &ctx).unwrap_err(),
            "managed_stream_original_snapshot_changed"
        );
        let mut changed = snapshot;
        changed.window.as_of_ms = 1200;
        seal_snapshot(&mut changed).unwrap();
        assert_eq!(
            retain_snapshot(&next, "request-a", changed, &ctx).unwrap_err(),
            "managed_stream_original_snapshot_changed"
        );
        assert_eq!(
            next.runtime()
                .storage
                .list_json("managed_stream_step_snapshots_v1")
                .unwrap()
                .len(),
            1
        );
    }
}
