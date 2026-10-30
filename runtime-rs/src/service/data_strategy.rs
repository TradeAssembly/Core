//! Product entry points for immutable strategy-bound data plans and signal runs.
use super::{data_evaluation, data_plans, strategy, ServiceResponse, TradeAssemblyService};
use crate::{
    data_pipeline::PipelineRequest,
    data_strategy::{DataStrategy, CONFIG_KEY},
    ports::{IdempotencyKey, SideEffectContext},
};
use serde_json::{json, Value};

pub fn program(
    service: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
) -> Result<Option<DataStrategy>, String> {
    service
        .require_object("strategy", &scope.strategy_id)
        .map_err(|_| "data_strategy_not_available")?;
    let version = strategy::strategy_versions(service, &scope.strategy_id)
        .into_iter()
        .find(|v| v["id"] == scope.strategy_version_id && v["specHash"] == scope.strategy_spec_hash)
        .ok_or("data_strategy_version_missing")?;
    let mut programs = version
        .pointer("/spec/stages/evaluate/substeps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.get("config").and_then(|c| c.get(CONFIG_KEY)));
    let Some(value) = programs.next() else {
        return Ok(None);
    };
    if programs.next().is_some() {
        return Err("data_strategy_program_ambiguous".into());
    }
    let stage = &version["spec"]["stages"]["evaluate"];
    let steps = stage["substeps"]
        .as_array()
        .ok_or("data_strategy_program_invalid")?;
    if stage["enabled"] != true
        || steps.len() != 1
        || steps[0]["optional"] == true
        || steps[0]["config"].as_object().is_none_or(|c| c.len() != 1)
    {
        return Err("data_strategy_evaluate_stage_unsupported".into());
    }
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|_| "data_strategy_program_invalid".into())
}
fn context(service: &TradeAssemblyService, body: &Value) -> Result<SideEffectContext, String> {
    let key = body["idempotencyKey"]
        .as_str()
        .ok_or("data_strategy_idempotency_required")?;
    let authority = crate::control_plane::http_envelope(
        "http",
        "POST",
        "/product/strategies/data-runs/step",
        body,
    )?
    .authority;
    Ok(SideEffectContext::new(
        authority,
        IdempotencyKey::new(key).map_err(|_| "data_strategy_idempotency_required")?,
    )
    .with_agent_execution(service.agent_mcp_execution_context()))
}
fn preparation_request(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<(data_plans::PlanScope, PipelineRequest, DataStrategy), String> {
    let scope: data_plans::PlanScope =
        serde_json::from_value(body["scope"].clone()).map_err(|_| "data_strategy_scope_invalid")?;
    let program = program(service, &scope)?.ok_or("data_strategy_program_missing")?;
    let graph = body["capabilityGraphRevisionId"]
        .as_str()
        .ok_or("data_strategy_graph_required")?;
    let mode = body["mode"].as_str().ok_or("data_strategy_mode_required")?;
    // Read-only evaluation supports these contexts; it grants no activation.
    if !matches!(mode, "paper" | "live" | "backtest") {
        return Err("data_strategy_mode_invalid".into());
    }
    let request = PipelineRequest {
        source_captures: body
            .get("sourceCaptures")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "data_strategy_capture_bindings_invalid")?
            .unwrap_or_default(),
        capability_graph_revision: graph.into(),
        mode: mode.into(),
        nodes: program.nodes.clone(),
        bindings: program.bindings.clone(),
        trade_instruments: program.trade_instruments.clone(),
    };
    Ok((scope, request, program))
}
pub fn prepare(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let result: Result<data_plans::StoredDataPlan, String> = (|| {
        let (scope, request, program) = preparation_request(service, &body)?;
        let plan = data_plans::save(service, scope, request, &context(service, &body)?)?;
        program.validate(&plan.content.plan)?;
        Ok(plan)
    })();
    match result {
        Ok(plan) => ServiceResponse::ok(json!({"plan":plan,"executionAuthorityGranted":false})),
        Err(code) => error_response(&code),
    }
}
pub fn evaluate(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let result: Result<data_evaluation::EvaluationReceipt, String> = (|| {
        let step: data_evaluation::EvaluationStep = serde_json::from_value(body["step"].clone())
            .map_err(|_| "data_strategy_step_invalid")?;
        let plan = data_plans::current(service, &step.plan_revision_id)?;
        let program =
            program(service, &plan.content.scope)?.ok_or("data_strategy_program_missing")?;
        program.validate(&plan.content.plan)?;
        let ctx = context(service, &body)?;
        data_evaluation::step(service, step, &ctx)
    })();
    match result {
        Ok(receipt) => {
            ServiceResponse::ok(json!({"receipt":receipt,"executionAuthorityGranted":false}))
        }
        Err(code) => error_response(&code),
    }
}

/// Revalidate ownership and immutable bindings before a transport-level cached
/// command can return a previously completed response.
pub(crate) fn preflight(
    service: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    let result: Result<(), String> = match path {
        "/product/strategies/data-plans/prepare" => (|| {
            let (scope, request, _) = preparation_request(service, body)?;
            data_plans::prepare(service, scope, request)?;
            Ok(())
        })(),
        "/product/strategies/data-runs/step" => (|| {
            let step: data_evaluation::EvaluationStep =
                serde_json::from_value(body["step"].clone())
                    .map_err(|_| "data_strategy_step_invalid")?;
            let plan = data_plans::current(service, &step.plan_revision_id)?;
            program(service, &plan.content.scope)?
                .ok_or("data_strategy_program_missing")?
                .validate(&plan.content.plan)
        })(),
        _ => return None,
    };
    result.err().map(|code| error_response(&code))
}

fn error_response(code: &str) -> ServiceResponse {
    if matches!(
        code,
        "data_strategy_capture_bindings_invalid"
            | "data_strategy_scope_invalid"
            | "data_strategy_step_invalid"
            | "data_strategy_idempotency_required"
            | "data_strategy_graph_required"
            | "data_strategy_mode_required"
            | "data_strategy_mode_invalid"
    ) {
        ServiceResponse::bad_request(code)
    } else {
        ServiceResponse::conflict(code)
    }
}
