//! Selection receipts -> concrete typed data plan. No broker universe mutation.
use super::{data_plans, instrument_screens, strategy, ServiceResponse, TradeAssemblyService};
use crate::{data_pipeline::*, data_strategy::DataStrategy, ports::*, selection_strategy::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tradeassembly_plugin_sdk::instrument_set::SelectionStatus;
use tradeassembly_strategy::spec::{SelectorType, StrategySpec};
pub(super) const PATH: &str = "/product/strategies/selection-plans/prepare";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionPlanCommand {
    pub scope: data_plans::PlanScope,
    pub capability_graph_revision: String,
    pub mode: String,
    pub subject_instrument_id: String,
    pub selection_receipts: BTreeMap<String, String>,
    pub source_captures: BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionPlanSource {
    pub command: SelectionPlanCommand,
    pub prepared_at_ms: i64,
    pub known_at_ms: i64,
    pub selection_sha256: BTreeMap<String, String>,
    pub resolved_program: DataStrategy,
}
pub(super) fn template(
    service: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
) -> Result<Option<(StrategySpec, SelectionStrategy)>, String> {
    service
        .require_object("strategy", &scope.strategy_id)
        .map_err(|_| "selection_plan_not_available")?;
    let version = strategy::strategy_versions(service, &scope.strategy_id)
        .into_iter()
        .find(|v| v["id"] == scope.strategy_version_id && v["specHash"] == scope.strategy_spec_hash)
        .ok_or("selection_plan_version_missing")?;
    let stage = &version["spec"]["stages"]["evaluate"];
    let steps = stage["substeps"]
        .as_array()
        .ok_or("selection_plan_stage_invalid")?;
    let mut values = steps.iter().filter_map(|s| s["config"].get(CONFIG_KEY));
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if crate::spec::canonical_hash(&version["spec"])? != scope.strategy_spec_hash {
        return Err("selection_plan_spec_hash_invalid".into());
    }

    let spec = StrategySpec::model_validate(version["spec"].clone())
        .map_err(|_| "selection_plan_spec_invalid")?;

    if values.next().is_some()
        || stage["enabled"] != true
        || steps.len() != 1
        || steps[0]["optional"] == true
        || steps[0]["config"].as_object().is_none_or(|v| v.len() != 1)
    {
        return Err("selection_plan_stage_invalid".into());
    }
    if !tradeassembly_strategy::spec::validate(&version["spec"]).valid {
        return Err("selection_plan_spec_invalid".into());
    }
    let template: SelectionStrategy =
        serde_json::from_value(value.clone()).map_err(|_| "selection_plan_template_invalid")?;
    if template.schema_version != "tradeassembly.selection_strategy.v1" {
        return Err("selection_plan_template_version_unsupported".into());
    }
    Ok(Some((spec, template)))
}
fn resolve(
    service: &TradeAssemblyService,
    c: &SelectionPlanCommand,
    prepared_at_ms: i64,
) -> Result<SelectionPlanSource, String> {
    if !matches!(c.mode.as_str(), "paper" | "live" | "backtest")
        || prepared_at_ms <= 0
        || prepared_at_ms > service.runtime().clock.now_ms()
        || c.selection_receipts.len() > 16
        || c.source_captures.len() > 32
    {
        return Err("selection_plan_command_invalid".into());
    }
    let (spec, template) = template(service, &c.scope)?.ok_or("selection_plan_template_missing")?;
    if c.selection_receipts
        .keys()
        .any(|k| !template.selections.contains_key(k))
    {
        return Err("selection_plan_binding_unknown".into());
    }
    let mut selected = BTreeMap::new();
    let mut evidence = BTreeMap::new();
    let mut known = prepared_at_ms;
    for (name, binding) in &template.selections {
        let markets = spec
            .signal_market_requirements
            .iter()
            .filter(|m| m.requirement_id == binding.market_requirement_ref)
            .collect::<Vec<_>>();
        if markets.len() != 1 {
            return Err("selection_plan_market_missing".into());
        }
        let market = markets[0];
        let selectors = market
            .selectors
            .iter()
            .filter(|s| s.selector_id == binding.selector_ref)
            .collect::<Vec<_>>();
        if selectors.len() != 1 {
            return Err("selection_plan_selector_missing".into());
        }
        let selector = selectors[0];
        let members = match selector.selector_type {
            SelectorType::Screened => {
                let id = c
                    .selection_receipts
                    .get(name)
                    .ok_or("selection_plan_screen_required")?;
                let r = instrument_screens::current(service, id)?;
                if r.command.scope != c.scope
                    || r.command.mode != c.mode
                    || r.command.capability_graph_revision != c.capability_graph_revision
                    || r.command.signal_requirement_ref != binding.market_requirement_ref
                    || r.command.selector_ref != binding.selector_ref
                {
                    return Err("selection_plan_screen_binding_mismatch".into());
                }
                match r.instrument_set.status {
                    SelectionStatus::CompleteNonempty => {}
                    SelectionStatus::CompleteEmpty => {
                        return Err("selection_plan_no_eligible_instrument".into())
                    }
                    _ => return Err("selection_plan_incomplete".into()),
                }
                known = known.max(r.instrument_set.known_at_ms);
                evidence.insert(name.clone(), r.receipt_sha256);
                r.instrument_set
                    .members
                    .into_iter()
                    .map(|i| InstrumentContext {
                        instrument_id: i.instrument_id,
                        asset_class: i.asset_class,
                        instrument_family: i.instrument_family,
                    })
                    .collect()
            }
            SelectorType::NormalizedInstrument => {
                if c.selection_receipts.contains_key(name) {
                    return Err("selection_plan_static_receipt_rejected".into());
                }
                let id = selector
                    .normalized_id
                    .clone()
                    .filter(|i| !i.is_empty())
                    .ok_or("selection_plan_static_invalid")?;
                let asset_class = serde_json::to_value(&market.asset_class)
                    .map_err(|_| "selection_plan_static_invalid")?
                    .as_str()
                    .ok_or("selection_plan_static_invalid")?
                    .to_owned();
                let instrument_family = serde_json::to_value(&market.instrument_family)
                    .map_err(|_| "selection_plan_static_invalid")?
                    .as_str()
                    .ok_or("selection_plan_static_invalid")?
                    .to_owned();
                evidence.insert(name.clone(),canonical_sha256(&json!({"scope":c.scope,"selector":selector,"assetClass":asset_class,"instrumentFamily":instrument_family})).map_err(|_|"selection_plan_encoding_invalid")?);
                vec![InstrumentContext {
                    instrument_id: id,
                    asset_class,
                    instrument_family,
                }]
            }
            _ => return Err("selection_plan_selector_producer_unavailable".into()),
        };
        selected.insert(name.clone(), members);
    }
    let resolved_program = template.resolve(&selected, &c.subject_instrument_id)?;
    Ok(SelectionPlanSource {
        command: c.clone(),
        prepared_at_ms,
        known_at_ms: known,
        selection_sha256: evidence,
        resolved_program,
    })
}
/// Read-only immutable domain binding; host preparation/knowledge time is an
/// effect result and is deliberately not regenerated into a signed command hash.
pub(super) fn command_binding(
    service: &TradeAssemblyService,
    c: &SelectionPlanCommand,
) -> Result<String, String> {
    let content = build(
        service,
        resolve(service, c, service.runtime().clock.now_ms())?,
    )?;
    let source = content
        .selection_source
        .as_ref()
        .ok_or("selection_plan_source_invalid")?;
    canonical_sha256(&json!({"command":c,"selections":source.selection_sha256,"resolvedProgram":source.resolved_program,"compiledPlan":content.plan,"packages":content.package_sha256,"bindings":content.binding_sha256,"sourceAuthentication":content.source_authentication})).map_err(|_|"selection_plan_encoding_invalid".into())
}
pub(super) fn rebuild(
    service: &TradeAssemblyService,
    source: &SelectionPlanSource,
) -> Result<data_plans::DataPlanContent, String> {
    let fresh = resolve(service, &source.command, source.prepared_at_ms)?;
    if fresh != *source {
        return Err("selection_plan_source_stale".into());
    }
    build(service, fresh)
}
fn build(
    service: &TradeAssemblyService,
    source: SelectionPlanSource,
) -> Result<data_plans::DataPlanContent, String> {
    let program = &source.resolved_program;
    let c = &source.command;
    let request = PipelineRequest {
        selected_outputs: vec![],
        source_captures: c.source_captures.clone(),
        capability_graph_revision: c.capability_graph_revision.clone(),
        mode: c.mode.clone(),
        trade_instruments: program.trade_instruments.clone(),
        nodes: program.nodes.clone(),
        bindings: program.bindings.clone(),
    };
    data_plans::prepare_selected(service, c.scope.clone(), request, source)
}
fn command(body: &Value) -> Result<SelectionPlanCommand, String> {
    if serde_json::to_vec(body)
        .map_err(|_| "selection_plan_command_invalid")?
        .len()
        > 131072
    {
        return Err("selection_plan_command_too_large".into());
    }
    serde_json::from_value(body["selectionPlan"].clone())
        .map_err(|_| "selection_plan_command_invalid".into())
}
pub(super) fn preflight(
    service: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    if path != PATH {
        return None;
    }
    let result = (|| {
        let c = command(body)?;
        let authority = crate::control_plane::http_envelope("http", "POST", PATH, body)?.authority;
        if authority.account_mode != if c.mode == "live" { "live" } else { "paper" } {
            return Err("selection_plan_mode_mismatch".into());
        }
        build(
            service,
            resolve(service, &c, service.runtime().clock.now_ms())?,
        )?;
        Ok::<_, String>(())
    })();
    result.err().map(|code| ServiceResponse::conflict(&code))
}
pub(super) fn prepare_response(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let result = (|| {
        let c = command(&body)?;
        let authority = crate::control_plane::http_envelope("http", "POST", PATH, &body)?.authority;
        if authority.account_mode != if c.mode == "live" { "live" } else { "paper" } {
            return Err("selection_plan_mode_mismatch".into());
        }
        let key = body["idempotencyKey"]
            .as_str()
            .ok_or("selection_plan_idempotency_required")?;
        let context = SideEffectContext::new(authority, IdempotencyKey::new(key)?)
            .with_agent_execution(service.agent_mcp_execution_context());
        if let Some(previous) = data_plans::previous(service, &c.scope, &context)? {
            return original(previous, &c);
        }
        let content = build(
            service,
            resolve(service, &c, service.runtime().clock.now_ms())?,
        )?;
        match data_plans::persist(service.runtime().storage.as_ref(), content, &context) {
            Err(code) if code == "data_plan_idempotency_conflict" => original(
                data_plans::previous(service, &c.scope, &context)?
                    .ok_or("selection_plan_original_missing")?,
                &c,
            ),
            result => result,
        }
    })();
    match result {
        Ok(plan) => ServiceResponse::ok(
            json!({"plan":plan,"executionAuthorityGranted":false,"managedPositionEffects":false}),
        ),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

fn original(
    plan: data_plans::StoredDataPlan,
    command: &SelectionPlanCommand,
) -> Result<data_plans::StoredDataPlan, String> {
    if plan
        .content
        .selection_source
        .as_ref()
        .is_none_or(|s| s.command != *command)
    {
        return Err("selection_plan_idempotency_conflict".into());
    }
    Ok(plan)
}
