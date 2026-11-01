//! Selection receipts -> concrete typed data plan. No broker universe mutation.
use super::{data_plans, instrument_screens, strategy, ServiceResponse, TradeAssemblyService};
use crate::{data_pipeline::*, data_strategy::DataStrategy, ports::*, selection_strategy::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tradeassembly_plugin_sdk::instrument_set::SelectionStatus;
use tradeassembly_strategy::spec::{SelectorType, StrategySpec};
pub(super) const PATH: &str = "/product/strategies/selection-plans/prepare";
pub(super) const MANAGED_PATH: &str = "/product/strategies/managed-position-plans/prepare";
/// Separate local wire: the legacy V2 SelectionPlanCommand is unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedPositionPlanCommand {
    pub schema_version: String,
    pub scope: data_plans::PlanScope,
    pub managed_position_id: String,
    pub selection_receipts: BTreeMap<String, String>,
    pub source_captures: BTreeMap<String, String>,
}
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_position_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_position_sha256: Option<String>,
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
    resolve_inner(service, c, prepared_at_ms, None)
}
// Only the traded subject is replaced by the settled owned-position witness.
// Independently selected context still executes its normal freshness checks.
fn selection_members(
    alias: &str,
    trade_alias: &str,
    managed: Option<&(InstrumentContext, String)>,
    used_as_context: bool,
    current_selection: impl FnOnce() -> Result<Vec<InstrumentContext>, String>,
) -> Result<Vec<InstrumentContext>, String> {
    if alias == trade_alias && !used_as_context {
        if let Some((subject, _)) = managed {
            return Ok(vec![subject.clone()]);
        }
    }
    current_selection()
}
fn selection_is_context(template: &SelectionStrategy, alias: &str) -> bool {
    template.nodes.iter().flat_map(|n| std::iter::once(&n.context).chain(n.input_contexts.values()))
        .any(|c| matches!(c, ContextTemplate::Selected { selection_ref, .. } if selection_ref == alias))
}
fn resolve_inner(
    service: &TradeAssemblyService,
    c: &SelectionPlanCommand,
    prepared_at_ms: i64,
    managed_position_id: Option<&str>,
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
    let managed = managed_position_id
        .map(|id| -> Result<_, String> {
            if c.mode != "paper" {
                return Err("managed_plan_paper_required".into());
            }
            let (subject, graph, proof) =
                super::order_plans::management::exit_plan_subject(service, id, &c.scope)?;
            if c.subject_instrument_id != subject.instrument_id
                || c.capability_graph_revision != graph
            {
                return Err("managed_plan_subject_or_graph_mismatch".into());
            }
            Ok((subject, proof))
        })
        .transpose()?;
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
        let used_as_context = selection_is_context(&template, name);
        if name == &template.trade_selection_ref && !used_as_context {
            if let Some((subject, proof)) = &managed {
                evidence.insert(
                    name.clone(),
                    canonical_sha256(&json!({
                        "schemaVersion":"tradeassembly.managed_trade_selection.v1",
                        "subject":subject,"managedPositionSha256":proof,
                    }))
                    .map_err(|_| "selection_plan_encoding_invalid")?,
                );
            }
        }
        let members = selection_members(
            name,
            &template.trade_selection_ref,
            managed.as_ref(),
            used_as_context,
            || {
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
                            SelectionStatus::CompleteEmpty
                                if managed.is_some() && name == &template.trade_selection_ref => {}
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
                Ok(members)
            },
        )?;
        selected.insert(name.clone(), members);
    }
    let resolved_program = if let Some((subject, _)) = &managed {
        // Metadata category remains the published trade market category.
        let binding = template
            .selections
            .get(&template.trade_selection_ref)
            .ok_or("selection_plan_market_missing")?;
        let market = spec
            .signal_market_requirements
            .iter()
            .find(|m| m.requirement_id == binding.market_requirement_ref)
            .ok_or("selection_plan_market_missing")?;
        if serde_json::to_value(&market.asset_class)
            .map_err(|_| "managed_plan_instrument_invalid")?
            != json!(subject.asset_class)
            || serde_json::to_value(&market.instrument_family)
                .map_err(|_| "managed_plan_instrument_invalid")?
                != json!(subject.instrument_family)
        {
            return Err("managed_plan_instrument_mismatch".into());
        }
        template.resolve_managed(&selected, subject)?
    } else {
        template.resolve(&selected, &c.subject_instrument_id)?
    };
    Ok(SelectionPlanSource {
        managed_position_id: managed_position_id.map(str::to_owned),
        managed_position_sha256: managed.map(|(_, proof)| proof),
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
    let fresh = resolve_inner(
        service,
        &source.command,
        source.prepared_at_ms,
        source.managed_position_id.as_deref(),
    )?;
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
    if path == MANAGED_PATH {
        let result = managed_content(service, body).map(|_| ());
        return result.err().map(|code| ServiceResponse::conflict(&code));
    }
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
    if plan.content.selection_source.as_ref().is_none_or(|s| {
        s.command != *command
            || s.managed_position_id.is_some()
            || s.managed_position_sha256.is_some()
    }) {
        return Err("selection_plan_idempotency_conflict".into());
    }
    Ok(plan)
}

fn managed_command(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<(ManagedPositionPlanCommand, SelectionPlanCommand), String> {
    if serde_json::to_vec(body)
        .map_err(|_| "managed_plan_command_invalid")?
        .len()
        > 131072
    {
        return Err("managed_plan_command_too_large".into());
    }
    let c: ManagedPositionPlanCommand = serde_json::from_value(body["managedPositionPlan"].clone())
        .map_err(|_| "managed_plan_command_invalid")?;
    if c.schema_version != "tradeassembly.managed_position_plan.v1"
        || c.managed_position_id.is_empty()
        || c.managed_position_id.len() > 256
        || c.selection_receipts.len() > 16
        || c.source_captures.len() > 32
        || crate::control_plane::http_envelope("http", "POST", MANAGED_PATH, body)?
            .authority
            .account_mode
            != "paper"
    {
        return Err("managed_plan_command_invalid".into());
    }
    let (subject, graph, _) = super::order_plans::management::exit_plan_subject(
        service,
        &c.managed_position_id,
        &c.scope,
    )?;
    let internal = SelectionPlanCommand {
        scope: c.scope.clone(),
        capability_graph_revision: graph,
        mode: "paper".into(),
        subject_instrument_id: subject.instrument_id,
        selection_receipts: c.selection_receipts.clone(),
        source_captures: c.source_captures.clone(),
    };
    Ok((c, internal))
}
fn managed_content(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<data_plans::DataPlanContent, String> {
    let (c, internal) = managed_command(service, body)?;
    build(
        service,
        resolve_inner(
            service,
            &internal,
            service.runtime().clock.trusted_now_ms()?,
            Some(&c.managed_position_id),
        )?,
    )
}
pub(super) fn prepare_managed_response(
    service: &TradeAssemblyService,
    body: Value,
) -> ServiceResponse {
    let result = (|| {
        let (c, internal) = managed_command(service, &body)?;
        let authority =
            crate::control_plane::http_envelope("http", "POST", MANAGED_PATH, &body)?.authority;
        let context = SideEffectContext::new(
            authority,
            IdempotencyKey::new(
                body["idempotencyKey"]
                    .as_str()
                    .ok_or("managed_plan_idempotency_required")?,
            )?,
        )
        .with_agent_execution(service.agent_mcp_execution_context());
        if let Some(plan) = data_plans::previous(service, &c.scope, &context)? {
            if plan.content.selection_source.as_ref().is_none_or(|source| {
                source.command != internal
                    || source.managed_position_id.as_deref() != Some(c.managed_position_id.as_str())
            }) {
                return Err("managed_plan_idempotency_conflict".into());
            }
            return Ok(plan);
        }
        let content = managed_content(service, &body)?;
        data_plans::persist(service.runtime().storage.as_ref(), content, &context)
    })();
    match result {
        Ok(plan) => ServiceResponse::ok(
            json!({"plan":plan,"entryAdmissionAllowed":false,"automaticReversalSupported":false,"executionAuthorityGranted":false,"managedPositionEffects":false}),
        ),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

#[cfg(test)]
mod managed_wire_tests {
    use super::*;
    #[test]
    fn managed_command_is_a_separate_closed_wire_without_caller_subject_or_account() {
        let scope = json!({"strategyId":"s","strategyVersionId":"v","strategySpecHash":"h"});
        let value = json!({"schemaVersion":"tradeassembly.managed_position_plan.v1","scope":scope,"managedPositionId":"position_owned","selectionReceipts":{},"sourceCaptures":{}});
        assert!(serde_json::from_value::<ManagedPositionPlanCommand>(value.clone()).is_ok());
        for field in [
            "subjectInstrumentId",
            "accountRef",
            "activationId",
            "mode",
            "capabilityGraphRevision",
            "entry",
            "quantity",
        ] {
            let mut bad = value.clone();
            bad[field] = json!("forbidden");
            assert!(
                serde_json::from_value::<ManagedPositionPlanCommand>(bad).is_err(),
                "{field}"
            );
        }
        let mut legacy = json!({"scope":scope,"capabilityGraphRevision":"g","mode":"paper","subjectInstrumentId":"subject","selectionReceipts":{},"sourceCaptures":{}});
        assert!(serde_json::from_value::<SelectionPlanCommand>(legacy.clone()).is_ok());
        legacy["managedPositionId"] = json!("position_owned");
        assert!(serde_json::from_value::<SelectionPlanCommand>(legacy).is_err());
    }
    #[test]
    fn managed_trade_binding_bypasses_expired_entry_selection_but_context_still_checks() {
        use std::cell::Cell;
        let owned = (
            InstrumentContext {
                instrument_id: "OWNED".into(),
                asset_class: "equity".into(),
                instrument_family: "equity".into(),
            },
            "settled-owned-position-proof".into(),
        );
        let called = Cell::new(false);
        let traded = selection_members("traded", "traded", Some(&owned), false, || {
            called.set(true);
            Err("instrument_screen_stale".into())
        })
        .unwrap();
        assert_eq!(traded, vec![owned.0.clone()]);
        assert!(
            !called.get(),
            "entry membership expiry must not orphan the owned lot"
        );
        let context = selection_members("context", "traded", Some(&owned), true, || {
            called.set(true);
            Err("instrument_screen_stale".into())
        });
        assert_eq!(context.unwrap_err(), "instrument_screen_stale");
        assert!(called.get());
        called.set(false);
        let entry = selection_members("traded", "traded", None, false, || {
            called.set(true);
            Err("instrument_screen_stale".into())
        });
        assert_eq!(entry.unwrap_err(), "instrument_screen_stale");
        assert!(called.get(), "normal entry still requires fresh membership");
        let same_alias_context = selection_members("traded", "traded", Some(&owned), true, || {
            Err("current_context_screen_expired".into())
        });
        assert_eq!(
            same_alias_context.unwrap_err(),
            "current_context_screen_expired"
        );
    }

    #[test]
    fn traded_selector_used_by_node_or_input_context_requires_current_members() {
        let rule = RuleTemplate::IsTrue {
            value: OperandTemplate::Boolean { value: true },
        };
        let mut template = SelectionStrategy {
            schema_version: "tradeassembly.selection_strategy.v1".into(),
            selections: BTreeMap::new(),
            trade_selection_ref: "traded".into(),
            bindings: vec![],
            nodes: vec![NodeTemplate {
                graph_node_id: "signal".into(),
                parameters: json!({}),
                context: ContextTemplate::Subject {
                    interval_ms: Some(1000),
                },
                input_contexts: BTreeMap::from([(
                    "benchmark".into(),
                    ContextTemplate::Selected {
                        selection_ref: "traded".into(),
                        interval_ms: Some(1000),
                    },
                )]),
            }],
            entry: rule.clone(),
            exit: rule,
        };
        assert!(selection_is_context(&template, "traded"));
        template.nodes[0].input_contexts.clear();
        assert!(!selection_is_context(&template, "traded"));
        template.nodes[0].context = ContextTemplate::Selected {
            selection_ref: "traded".into(),
            interval_ms: Some(1000),
        };
        assert!(selection_is_context(&template, "traded"));
        assert!(!selection_is_context(&template, "other"));
    }

    #[test]
    fn unmanaged_selection_source_serialization_omits_both_new_fields() {
        let legacy = json!({"command":{"scope":{"strategyId":"s","strategyVersionId":"v","strategySpecHash":"h"},"capabilityGraphRevision":"g","mode":"paper","subjectInstrumentId":"subject","selectionReceipts":{},"sourceCaptures":{}},"preparedAtMs":1,"knownAtMs":1,"selectionSha256":{},"resolvedProgram":{"version":1,"nodes":[],"bindings":[],"tradeInstruments":[],"entry":{"kind":"is_true","value":{"kind":"boolean","value":false}},"exit":{"kind":"is_true","value":{"kind":"boolean","value":true}}}});
        let source: SelectionPlanSource = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(source).unwrap(), legacy);
    }
}
