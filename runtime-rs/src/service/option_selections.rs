//! Strategy-bound option selection. Provider reads only; never submits orders.
use super::{
    capability_graph, data_binding, data_captures, data_evaluation, data_plans, data_strategy,
    strategy, ServiceResponse, TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, option_selection::*, ports::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use tradeassembly_strategy::spec::*;
const RECEIPTS: &str = "option_selection_receipts_v1";
const KEYS: &str = "option_selection_keys_v1";
const PATH: &str = "/product/strategies/option-selections/create";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionCommand {
    pub scope: data_plans::PlanScope,
    pub capability_graph_revision: String,
    pub acquisition_node: String,
    pub mode: String,
    pub stage_substep_id: String,
    pub signal_step: data_evaluation::EvaluationStep,
    pub signal_idempotency_key: String,
    pub signal_event_end_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionReceipt {
    pub schema_version: String,
    pub receipt_id: String,
    pub command: SelectionCommand,
    pub provider: data_binding::PinnedOperation,
    pub signal_receipt_sha256: String,
    pub signal_known_at_ms: i64,
    pub acquired_at_ms: i64,
    pub responses: Vec<PluginOperationResponse>,
    pub payload_sha256: Vec<String>,
    pub result: OptionSelectionResult,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "option_selection_encoding_invalid".into())
}
fn identity(r: &SelectionReceipt) -> Result<String, String> {
    let mut r = r.clone();
    r.receipt_id.clear();
    hash(&r)
}
fn context(service: &TradeAssemblyService, body: &Value) -> Result<SideEffectContext, String> {
    let authority = crate::control_plane::http_envelope("http", "POST", PATH, body)?.authority;
    Ok(SideEffectContext::new(
        authority,
        IdempotencyKey::new(
            body["idempotencyKey"]
                .as_str()
                .ok_or("option_selection_command_invalid")?,
        )?,
    )
    .with_agent_execution(service.agent_mcp_execution_context()))
}
fn policy(
    service: &TradeAssemblyService,
    c: &SelectionCommand,
) -> Result<(OptionSelection, String), String> {
    service
        .require_object("strategy", &c.scope.strategy_id)
        .map_err(|_| "option_selection_not_available")?;
    let version = strategy::strategy_versions(service, &c.scope.strategy_id)
        .into_iter()
        .find(|v| {
            v["id"] == c.scope.strategy_version_id && v["specHash"] == c.scope.strategy_spec_hash
        })
        .ok_or("option_selection_version_missing")?;
    let spec: StrategySpec = serde_json::from_value(version["spec"].clone())
        .map_err(|_| "option_selection_spec_invalid")?;
    let mut choices = vec![];
    for stage in StageName::VALIDATION_ORDER {
        let view = spec.stages.view(&stage);
        for step in view.substeps {
            if step.id == c.stage_substep_id {
                if let Some(p) = &step.option_selection {
                    if stage != StageName::Intent
                        || step.operation_ref != "option.contract.select@1"
                        || !view.enabled
                        || step.optional
                        || step.kind != SubstepKind::Screen
                        || step.failure_policy != FailurePolicy::FailClosed
                    {
                        return Err("option_selection_stage_invalid".into());
                    }
                    choices.push(p.clone());
                }
            }
        }
    }
    if choices.len() != 1 {
        return Err("option_selection_stage_ambiguous".into());
    }
    let p = choices.remove(0);
    p.validate()?;
    // Explicit model estimation is a separately bound operation. This stage
    // currently accepts provider-supplied analytics only; never falls back.
    if !matches!(p.analytics, OptionAnalytics::ProviderSupplied) {
        return Err("option_selection_explicit_model_stage_unavailable".into());
    }
    let underlying = spec
        .signal_market_requirements
        .iter()
        .find(|r| r.requirement_id == p.signal_requirement_ref)
        .and_then(|r| {
            r.selectors
                .iter()
                .find(|s| s.selector_id == p.underlying_selector_ref)
        })
        .filter(|s| s.selector_type == SelectorType::NormalizedInstrument)
        .and_then(|s| s.normalized_id.clone())
        .ok_or("option_selection_underlying_selector_unavailable")?;
    if underlying.is_empty() || underlying.len() > 128 {
        return Err("option_selection_underlying_invalid".into());
    }
    Ok((p, underlying))
}
fn graph(
    service: &TradeAssemblyService,
    c: &SelectionCommand,
) -> Result<CapabilityGraphResolution, String> {
    if c.mode == "backtest" {
        return Err("option_selection_historical_chain_replay_unavailable".into());
    }
    if c.mode != "paper" && c.mode != "live" {
        return Err("option_selection_mode_invalid".into());
    }
    let now = chrono::DateTime::from_timestamp_millis(service.runtime().clock.now_ms())
        .ok_or("option_selection_clock_invalid")?
        .to_rfc3339();
    let r = capability_graph::revision_for_context(
        service,
        &c.capability_graph_revision,
        &c.scope.strategy_id,
        &c.scope.strategy_version_id,
        &c.mode,
        &now,
    )
    .map_err(|_| "option_selection_graph_stale")?;
    if r["strategySpecHash"] != c.scope.strategy_spec_hash {
        return Err("option_selection_strategy_mismatch".into());
    }
    serde_json::from_value(r["graph"].clone()).map_err(|_| "option_selection_graph_invalid".into())
}
fn selected<'a>(
    g: &'a CapabilityGraphResolution,
    c: &SelectionCommand,
    p: &OptionSelection,
) -> Result<&'a CapabilitySelection, String> {
    let s = g
        .nodes
        .iter()
        .find(|n| n.node_id == c.acquisition_node)
        .and_then(|n| n.selected.as_ref())
        .ok_or("option_chain_unresolved")?;
    if !g.nodes.iter().any(|n| {
        n.node_id == c.acquisition_node
            && n.requirement.requirement_id == p.chain_capability_requirement_ref
    }) {
        return Err("option_chain_requirement_mismatch".into());
    }
    if !s
        .operation
        .traits
        .data_shapes
        .iter()
        .any(|s| s == "option_chain")
        || !s.operation.traits.modes.contains(&c.mode)
    {
        return Err("option_chain_context_unsupported".into());
    }
    Ok(s)
}
fn references_underlying(rule: &crate::data_strategy::Rule, underlying: &str) -> bool {
    use crate::data_strategy::{Operand, Rule};
    let matches =
        |o: &Operand| matches!(o, Operand::Output { instrument, .. } if instrument == underlying);
    match rule {
        Rule::Compare { left, right, .. } => matches(left) || matches(right),
        Rule::IsTrue { value } => matches(value),
        Rule::All { rules } | Rule::Any { rules } => {
            rules.iter().any(|r| references_underlying(r, underlying))
        }
        Rule::Not { rule } => references_underlying(rule, underlying),
    }
}
fn signal(
    service: &TradeAssemblyService,
    c: &SelectionCommand,
    ctx: &SideEffectContext,
    p: &OptionSelection,
    underlying: &str,
) -> Result<(String, i64), String> {
    let (plan, receipt) =
        data_evaluation::admitted_signal(service, &c.signal_step, &c.signal_idempotency_key, ctx)?;
    if plan.content.scope != c.scope
        || plan.content.plan.request.mode != c.mode
        || plan.content.plan.request.capability_graph_revision != c.capability_graph_revision
    {
        return Err("option_signal_scope_mismatch".into());
    }
    let program =
        data_strategy::program(service, &c.scope)?.ok_or("option_signal_program_missing")?;
    program.validate(&plan.content.plan)?;
    if !references_underlying(&program.entry, underlying)
        || !program.nodes.iter().any(|n| {
            n.context
                .instruments
                .iter()
                .any(|i| i.instrument_id == underlying)
        })
        || plan.content.source_authentication.values().any(|s| {
            !matches!(
                s,
                data_captures::SourceAuthentication::HostAcquisition { .. }
            )
        })
        || plan.content.source_authentication.is_empty()
    {
        return Err("option_signal_verified_context_required".into());
    }
    let frame = receipt
        .signals
        .iter()
        .find(|s| s.event_end_ms == c.signal_event_end_ms)
        .ok_or("option_signal_frame_missing")?;
    let now = service.runtime().clock.now_ms();
    if frame.entry != (crate::data_strategy::SignalValue::Ready { value: true })
        || frame.known_at_ms > now
        || frame.arrived_at_ms > now
        || now.saturating_sub(frame.event_end_ms) > p.max_age_ms as i64
    {
        return Err("option_signal_not_current_entry".into());
    }
    Ok((
        receipt.receipt_sha256,
        frame.known_at_ms.max(frame.arrived_at_ms),
    ))
}
fn validate_current(
    service: &TradeAssemblyService,
    c: &SelectionCommand,
    ctx: &SideEffectContext,
) -> Result<
    (
        OptionSelection,
        String,
        CapabilityGraphResolution,
        data_binding::PinnedOperation,
        String,
        i64,
    ),
    String,
> {
    let (p, u) = policy(service, c)?;
    let g = graph(service, c)?;
    let s = selected(&g, c, &p)?;
    let pin = data_binding::pin(service, s)?;
    let (sig, time) = signal(service, c, ctx, &p, &u)?;
    Ok((p, u, g, pin, sig, time))
}
fn acquire_page(
    service: &TradeAssemblyService,
    c: &SelectionCommand,
    g: &CapabilityGraphResolution,
    s: &CapabilitySelection,
    input: Value,
    ctx: &SideEffectContext,
    page: u32,
) -> Result<PluginOperationResponse, String> {
    let pin = data_binding::pin(service, s)?;
    let package = service
        .runtime()
        .plugin_packages
        .get(&pin.package_sha256)?
        .ok_or("option_chain_package_missing")?;
    data_evaluation::allowed_data_operation(s, false, &package.manifest)?;
    let correlation = hash(&(
        c,
        &input,
        &ctx.authority,
        ctx.idempotency_key.as_str(),
        page,
    ))?;
    let request = PluginOperationRequest {
        correlation_id: correlation.clone(),
        plugin_instance_ref: s.plugin_instance_ref.clone(),
        plugin_ref: s.plugin_ref.clone(),
        manifest_fingerprint: s.manifest_fingerprint.clone(),
        operation_id: s.operation_id.clone(),
        capability: s.operation.capability.clone(),
        capability_graph_revision_id: c.capability_graph_revision.clone(),
        capability_graph_fingerprint: g.graph_fingerprint.clone(),
        strategy_id: c.scope.strategy_id.clone(),
        strategy_version_id: c.scope.strategy_version_id.clone(),
        strategy_spec_hash: c.scope.strategy_spec_hash.clone(),
        activation_id: String::new(),
        attempt_id: correlation.clone(),
        evaluation_tick_id: correlation.clone(),
        mode: c.mode.clone(),
        purpose: s.operation.purpose.clone(),
        account_ref: s.account_ref.clone(),
        timeout_ms: 15000,
        fencing_token: None,
        input,
        evidence_refs: vec![],
    };
    let mut call = ctx.clone();
    call.idempotency_key = IdempotencyKey::new(format!("option-chain:{correlation}"))?;
    let response = service
        .runtime()
        .plugin_operations
        .invoke(&request, &call)?;
    if serde_json::to_vec(&response)
        .map_err(|_| "option_chain_response_invalid")?
        .len()
        > 4 * 1024 * 1024
    {
        return Err("option_chain_response_too_large".into());
    }
    tradeassembly_plugin_sdk::data_package::metadata_budget(&response.payload)?;
    data_captures::capturable_payload(&response.payload)?;
    if response.correlation_id != correlation
        || response.reconciliation_required
        || response.freshness_state != "fresh"
        || response.observed_at_ms <= 0
        || response.observed_at_ms > service.runtime().clock.now_ms()
        || response.content_hash.is_empty()
        || response.source_event_id.is_empty()
        || !s
            .operation
            .traits
            .output_schema_refs
            .contains(&response.schema_ref)
        || response.payload["sourceClass"] != "production"
    {
        return Err("option_chain_acquisition_invalid".into());
    }
    let schema = service
        .runtime()
        .plugin_packages
        .data_schema(&pin.package_sha256, &response.schema_ref)?
        .ok_or("option_chain_schema_missing")?;
    if !jsonschema::validator_for(&schema)
        .map_err(|_| "option_chain_schema_invalid")?
        .is_valid(&response.payload)
    {
        return Err("option_chain_schema_invalid".into());
    }
    Ok(response)
}
fn normalize(row: &Value) -> Result<OptionCandidate, String> {
    fn text(v: &Value) -> Result<String, String> {
        v.as_str()
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .ok_or("option_chain_contract_invalid".into())
    }
    let data = row
        .pointer("/data/data")
        .ok_or("option_chain_contract_invalid")?;
    if row.pointer("/data/kind") != Some(&json!("option_contract"))
        || row["instrumentId"] != data["contractSymbol"]
    {
        return Err("option_chain_contract_invalid".into());
    }
    let observed_at_ms = chrono::DateTime::parse_from_rfc3339(&text(&row["timestamp"])?)
        .map_err(|_| "option_chain_timestamp_invalid")?
        .timestamp_millis();
    Ok(OptionCandidate {
        contract_id: text(&data["contractSymbol"])?,
        underlying: text(&data["underlyingInstrumentId"])?,
        expiry: text(&data["expiration"])?,
        strike: data["strike"]
            .as_f64()
            .ok_or("option_chain_strike_invalid")?,
        right: serde_json::from_value(data["right"].clone())
            .map_err(|_| "option_chain_right_invalid")?,
        observed_at_ms,
        bid: data["bid"].as_f64(),
        ask: data["ask"].as_f64(),
        delta: data
            .pointer("/greeks/delta")
            .and_then(Value::as_f64)
            .map(|value| DeltaValue {
                value,
                origin: AnalyticsOrigin::ProviderSupplied,
            }),
        volume: data["volume"].as_f64(),
        open_interest: data["openInterest"].as_f64(),
    })
}
fn key(c: &SelectionCommand, ctx: &SideEffectContext) -> Result<String, String> {
    hash(&(
        "option_selection_command.v2",
        &c.scope.strategy_id,
        &ctx.authority.actor,
        &ctx.authority.account_mode,
        ctx.idempotency_key.as_str(),
    ))
}
fn current_receipt(
    service: &TradeAssemblyService,
    c: &SelectionCommand,
    ctx: &SideEffectContext,
    key: &str,
) -> Result<Option<SelectionReceipt>, String> {
    let mut value = service.runtime().storage.get_json(KEYS, key)?;
    if value.is_none() {
        // Earlier host indexes were transport scoped. Preserve them without
        // silently reacquiring or choosing between divergent legacy decisions.
        let mut entries = vec![];
        let surfaces = [ctx.authority.surface.as_str(), "http", "mcp"]
            .into_iter()
            .collect::<BTreeSet<_>>();
        for surface in surfaces {
            let mut authority = ctx.authority.clone();
            authority.surface = surface.into();
            let legacy = hash(&(
                &c.scope.strategy_id,
                &authority,
                ctx.idempotency_key.as_str(),
            ))?;
            if let Some(entry) = service.runtime().storage.get_json(KEYS, &legacy)? {
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
        }
        if entries.len() > 1 {
            return Err("option_selection_legacy_idempotency_ambiguous".into());
        }
        value = entries.pop();
    }
    let Some(v) = value else { return Ok(None) };
    if v["commandSha256"] != hash(c)? {
        return Err("option_selection_idempotency_conflict".into());
    }
    let raw = service
        .runtime()
        .storage
        .get_json(
            RECEIPTS,
            v["receiptId"]
                .as_str()
                .ok_or("option_selection_receipt_invalid")?,
        )?
        .ok_or("option_selection_receipt_invalid")?;
    let r: SelectionReceipt =
        serde_json::from_value(raw).map_err(|_| "option_selection_receipt_invalid")?;
    let (policy, _, _, pin, sig, _) = validate_current(service, c, ctx)?;
    if identity(&r)? != r.receipt_id
        || v["receiptId"] != r.receipt_id
        || hash(&r.command)? != hash(c)?
        || r.provider != pin
        || r.signal_receipt_sha256 != sig
        || r.schema_version != "tradeassembly.option_selection_receipt.v1"
    {
        return Err("option_selection_receipt_invalid".into());
    }
    let now = service.runtime().clock.now_ms();
    let fresh = |time: i64| {
        time > 0
            && now
                .checked_sub(time)
                .is_some_and(|age| age >= 0 && age <= policy.max_age_ms as i64)
    };
    if !fresh(r.acquired_at_ms)
        || r.responses
            .iter()
            .any(|response| !fresh(response.observed_at_ms))
        || matches!(&r.result.outcome,OptionOutcome::Selected{contract} if !fresh(contract.observed_at_ms))
    {
        return Err("option_selection_receipt_stale".into());
    }
    Ok(Some(r))
}
pub(crate) fn execute(
    service: &TradeAssemblyService,
    c: SelectionCommand,
    ctx: &SideEffectContext,
) -> Result<SelectionReceipt, String> {
    let (p, underlying, g, pin, sig, signal_time) = validate_current(service, &c, ctx)?;
    let key = key(&c, ctx)?;
    if let Some(r) = current_receipt(service, &c, ctx, &key)? {
        return Ok(r);
    }
    let s = selected(&g, &c, &p)?;
    let mut candidates = vec![];
    let mut responses = vec![];
    let mut tokens = BTreeSet::new();
    let mut token = None;
    let started = std::time::Instant::now();
    let mut complete = false;
    let mut total_bytes = 0usize;
    for page in 0..p.max_pages {
        if started.elapsed().as_secs() >= 30 {
            return Err("option_chain_deadline_exceeded".into());
        }
        let input = json!({"instruments":[underlying],"maxRows":p.max_contracts,"pageToken":token});
        let response = acquire_page(service, &c, &g, s, input, ctx, page)?;
        total_bytes = total_bytes
            .checked_add(
                serde_json::to_vec(&response)
                    .map_err(|_| "option_chain_response_invalid")?
                    .len(),
            )
            .ok_or("option_chain_response_too_large")?;
        if total_bytes > 8 * 1024 * 1024 {
            return Err("option_chain_response_too_large".into());
        }
        if started.elapsed().as_secs() >= 30 {
            return Err("option_chain_deadline_exceeded".into());
        }

        if response.observed_at_ms < signal_time {
            return Err("option_chain_precedes_signal_availability".into());
        }
        let rows = response.payload["observations"]
            .as_array()
            .ok_or("option_chain_observations_invalid")?;
        if rows.len().saturating_add(candidates.len()) > p.max_contracts as usize {
            return Err("option_chain_candidate_budget_exceeded".into());
        }
        for row in rows {
            candidates.push(normalize(row)?);
        }
        token = match response.payload.get("nextPageToken") {
            Some(Value::Null) => None,
            Some(Value::String(t))
                if !t.is_empty() && t.len() <= 4096 && tokens.insert(t.clone()) =>
            {
                Some(t.clone())
            }
            _ => return Err("option_chain_pagination_invalid".into()),
        };
        responses.push(response);
        if token.is_none() {
            complete = true;
            break;
        }
    }
    if !complete {
        return Err("option_chain_incomplete".into());
    }
    let acquired_at_ms = service.runtime().clock.now_ms();
    let result = crate::option_selection::select(&p, &underlying, acquired_at_ms, &candidates)?;
    let (_, _, _, current_pin, current_sig, _) = validate_current(service, &c, ctx)?;
    if current_pin != pin || current_sig != sig {
        return Err("option_selection_binding_changed".into());
    }
    let payload_sha256 = responses
        .iter()
        .map(|r| hash(&r.payload))
        .collect::<Result<Vec<_>, _>>()?;
    let mut r = SelectionReceipt {
        schema_version: "tradeassembly.option_selection_receipt.v1".into(),
        receipt_id: String::new(),
        command: c.clone(),
        provider: pin,
        signal_receipt_sha256: sig,
        signal_known_at_ms: signal_time,
        acquired_at_ms,
        responses,
        payload_sha256,
        result,
    };
    r.receipt_id = identity(&r)?;
    if serde_json::to_vec(&r)
        .map_err(|_| "option_selection_receipt_invalid")?
        .len()
        > 8 * 1024 * 1024
    {
        return Err("option_selection_receipt_too_large".into());
    }
    if service.runtime().storage.put_json_batch(
        &[
            StorageWrite::new(RECEIPTS, &r.receipt_id, json!(r), ctx.clone()),
            StorageWrite::new(
                KEYS,
                &key,
                json!({"commandSha256":hash(&c)?,"receiptId":r.receipt_id}),
                ctx.clone(),
            ),
        ],
        &[
            StorageExpectation::new(RECEIPTS, &r.receipt_id, None),
            StorageExpectation::new(KEYS, &key, None),
        ],
    )? != ComparePutOutcome::Updated
    {
        return Err("option_selection_commit_conflict".into());
    }
    Ok(r)
}
fn error(code: &str) -> ServiceResponse {
    if code == "option_selection_command_invalid" {
        ServiceResponse::bad_request(code)
    } else {
        ServiceResponse::conflict(code)
    }
}
fn public_receipt(r: &SelectionReceipt) -> Value {
    // Pagination cursors remain in private acquisition evidence, never public
    // command responses. Keep a stable projection across durable/redacted retries.
    let evidence=r.responses.iter().zip(&r.payload_sha256).map(|(response,payload_sha256)|json!({"schemaRef":response.schema_ref,"observedAtMs":response.observed_at_ms,"sourceEventId":response.source_event_id,"contentHash":response.content_hash,"payloadSha256":payload_sha256})).collect::<Vec<_>>();
    json!({"schemaVersion":"tradeassembly.option_selection_view.v1","receiptId":r.receipt_id,"command":r.command,"provider":r.provider,"signalReceiptSha256":r.signal_receipt_sha256,"signalKnownAtMs":r.signal_known_at_ms,"acquiredAtMs":r.acquired_at_ms,"acquisitionEvidence":evidence,"result":r.result})
}
pub fn create(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let result = (|| {
        let c: SelectionCommand = serde_json::from_value(body["selection"].clone())
            .map_err(|_| "option_selection_command_invalid")?;
        execute(service, c, &context(service, &body)?)
    })();
    match result {
        Ok(receipt) => ServiceResponse::ok(
            json!({"selection":public_receipt(&receipt),"executionAuthorityGranted":false}),
        ),
        Err(code) => error(&code),
    }
}
pub(crate) fn preflight(
    service: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    if path != PATH {
        return None;
    }
    let result: Result<(), String> = (|| {
        let c: SelectionCommand = serde_json::from_value(body["selection"].clone())
            .map_err(|_| "option_selection_command_invalid")?;
        let ctx = context(service, body)?;
        validate_current(service, &c, &ctx)?;
        current_receipt(service, &c, &ctx, &key(&c, &ctx)?)?;
        Ok(())
    })();
    result.err().map(|e| error(&e))
}
