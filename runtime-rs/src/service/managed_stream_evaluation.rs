//! Closed managed source-stream evaluation. Preflight reads current owned
//! bindings only; dispatch retains exact source evidence before normal Core IO.
use super::{
    data_evaluation, data_plans, managed_data_streams, ServiceResponse, TradeAssemblyService,
};
use crate::{
    data_pipeline::canonical_sha256,
    ports::{IdempotencyKey, SideEffectContext},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(crate) const PATH: &str = "/product/strategies/managed-data-runs/step";
const VERSION: &str = "tradeassembly.managed_stream_evaluation_step.v1";
const MAX_BYTES: usize = 131_072;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Command {
    schema_version: String,
    step: data_evaluation::EvaluationStep,
    stream_heads: BTreeMap<String, String>,
    context_selection_receipts: BTreeMap<String, String>,
}
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    canonical_sha256(value).map_err(|_| "managed_stream_step_encoding_invalid".into())
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn command(body: &Value) -> Result<(data_plans::PlanScope, Command), String> {
    if serde_json::to_vec(body)
        .map_err(|_| "managed_stream_step_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("managed_stream_step_too_large".into());
    }
    let scope: data_plans::PlanScope = serde_json::from_value(body["scope"].clone())
        .map_err(|_| "managed_stream_step_scope_invalid")?;
    let c: Command = serde_json::from_value(body["managedStep"].clone())
        .map_err(|_| "managed_stream_step_invalid")?;
    if c.schema_version != VERSION
        || c.stream_heads.is_empty()
        || c.stream_heads.len() > 8
        || c.stream_heads.iter().any(|(node, sha)| {
            node.is_empty()
                || node.len() > 128
                || node.chars().any(char::is_control)
                || !digest(sha)
        })
        || !c.context_selection_receipts.is_empty()
        || c.step.run_id.is_empty()
        || c.step.run_id.len() > 128
        || c.step.run_id.chars().any(char::is_control)
        || c.step.plan_revision_id.len() > 256
        || c.step.window.event_start_ms <= 0
        || c.step.window.event_start_ms >= c.step.window.event_end_ms
        || c.step.window.event_end_ms > c.step.window.as_of_ms
        || c.step.window.as_of_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
        || c.step
            .expected_checkpoint_sha256
            .as_ref()
            .is_some_and(|sha| !digest(sha))
    {
        return Err("managed_stream_step_invalid".into());
    }
    Ok((scope, c))
}
fn context(s: &TradeAssemblyService, body: &Value) -> Result<SideEffectContext, String> {
    let envelope = crate::control_plane::http_envelope("http", "POST", PATH, body)?;
    if envelope.authority.account_mode != "paper" {
        return Err("managed_stream_step_paper_required".into());
    }
    Ok(SideEffectContext::new(
        envelope.authority,
        IdempotencyKey::new(
            body["idempotencyKey"]
                .as_str()
                .ok_or("managed_stream_step_idempotency_required")?,
        )?,
    )
    .with_agent_execution(s.agent_mcp_execution_context()))
}
type Prepared = (
    Command,
    data_plans::StoredDataPlan,
    managed_data_streams::Snapshot,
    SideEffectContext,
);
fn prepared(s: &TradeAssemblyService, body: &Value) -> Result<Prepared, String> {
    let _timing = super::order_plans::financial_timing("managed_eval.prepared");
    // Pure owned-plan/source validation only. The returned values carry no
    // scoped runtime. Preflight and handler start independent rounds; snapshot
    // retention, plugin invocation and atomic commitment use the original ports.
    super::package_validation::with_round(s, |s| prepared_round(s, body))
}
fn prepared_round(s: &TradeAssemblyService, body: &Value) -> Result<Prepared, String> {
    let (scope, c) = command(body)?;
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| "managed_stream_step_not_available")?;
    if let Some(agent) = s.agent_mcp_execution_context() {
        crate::agent_runner::bind_mcp_execution_context(
            s.runtime().as_ref(),
            agent,
            "tradeassembly.strategy.data_run.step",
            body.clone(),
        )?;
        if agent.mode() != "paper" {
            return Err("agent_mcp_execution_context_mismatch".into());
        }
    }
    let ctx = context(s, body)?;
    let plan = super::order_plans::measured("managed_eval.plan", || {
        data_plans::current(s, &c.step.plan_revision_id)
    })?;
    if plan.content.scope != scope
        || plan.content.plan.request.mode != "paper"
        || plan.content.selection_source.as_ref().is_none_or(|source| {
            source.managed_position_id.is_none() || source.source_streams.is_empty()
        })
    {
        return Err("managed_stream_step_plan_binding_invalid".into());
    }
    let program = super::data_strategy::program_for_content(s, &plan.content)?
        .ok_or("managed_stream_step_program_required")?;
    if program.entry
        != (crate::data_strategy::Rule::IsTrue {
            value: crate::data_strategy::Operand::Boolean { value: false },
        })
    {
        return Err("managed_stream_step_entry_admission_forbidden".into());
    }
    program.validate(&plan.content.plan)?;
    let roots = managed_data_streams::plan_roots(&plan);
    if roots.len() != c.stream_heads.len() || roots.keys().ne(c.stream_heads.keys()) {
        return Err("managed_stream_step_exact_heads_required".into());
    }
    for node in plan
        .content
        .plan
        .request
        .nodes
        .iter()
        .filter(|n| roots.contains_key(&n.graph_node_id))
    {
        let interval = node
            .context
            .interval_ms
            .filter(|v| *v > 0)
            .ok_or("managed_stream_step_interval_required")?;
        let duration = (c.step.window.event_end_ms - c.step.window.event_start_ms) as u64;
        let frames = duration / interval;
        if !duration.is_multiple_of(interval)
            || frames == 0
            || frames > 64
            || frames
                .checked_mul(node.context.instruments.len() as u64)
                .is_none_or(|points| points > 256)
        {
            return Err("managed_stream_step_budget_exceeded".into());
        }
    }
    super::order_plans::measured("managed_eval.streams", || {
        managed_data_streams::validate_plan_streams(s, &plan)
    })?;
    let snapshot_timing = super::order_plans::financial_timing("managed_eval.snapshot");
    let snapshot = managed_data_streams::snapshot(
        s,
        &scope,
        &plan.revision_id,
        &roots,
        &c.stream_heads,
        &c.step.window,
    )?;
    managed_data_streams::validate_snapshot_plan(s, &plan, &snapshot)?;
    drop(snapshot_timing);
    data_evaluation::remember_request_plan(s, &c.step, &plan)?;
    Ok((c, plan, snapshot, ctx))
}
pub(super) fn preflight(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    if path != PATH {
        return None;
    }
    let _timing = super::order_plans::financial_timing("managed_eval.preflight");
    prepared(s, body).err().map(error)
}
pub(super) fn response(s: &TradeAssemblyService, body: Value) -> ServiceResponse {
    if s.data_step_request.is_none() {
        return response(&data_evaluation::request_service(s), body);
    }
    let _timing = super::order_plans::financial_timing("managed_eval.response");
    let result = (|| {
        let (c, plan, snapshot, ctx) = prepared(s, &body)?;
        let request_sha256 = hash(&json!({"request":c.step,"authority":ctx.authority}))?;
        let retained = managed_data_streams::retain_snapshot(s, &request_sha256, snapshot, &ctx)?;
        managed_data_streams::validate_snapshot_plan(s, &plan, &retained)?;
        let expected = retained.snapshot_sha256.clone();
        data_evaluation::attach_source_snapshot(s, retained)?;
        let receipt = data_evaluation::step(s, c.step, &ctx)?;
        if receipt.source_snapshot_sha256.as_deref() != Some(expected.as_str()) {
            return Err("managed_stream_step_receipt_snapshot_mismatch".into());
        }
        Ok(receipt)
    })();
    match result {
        Ok(receipt) => {
            ServiceResponse::ok(json!({"receipt":receipt,"executionAuthorityGranted":false}))
        }
        Err(code) => error(code),
    }
}
fn error(code: String) -> ServiceResponse {
    if code == "managed_stream_step_not_available" || code == "data_plan_not_available" {
        ServiceResponse::object_unavailable()
    } else if matches!(
        code.as_str(),
        "managed_stream_step_invalid"
            | "managed_stream_step_too_large"
            | "managed_stream_step_scope_invalid"
            | "managed_stream_step_idempotency_required"
    ) {
        ServiceResponse::bad_request(&code)
    } else {
        ServiceResponse::conflict(&code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Value {
        json!({"scope":{"strategyId":"s","strategyVersionId":"v","strategySpecHash":format!("sha256:{}","a".repeat(64))},
            "managedStep":{"schemaVersion":VERSION,"step":{"runId":"managed-run","planRevisionId":format!("dataplan_{}","b".repeat(64)),
                "window":{"eventStartMs":100,"eventEndMs":200,"asOfMs":201},"expectedCheckpointSha256":null},
                "streamHeads":{"source":"c".repeat(64)},"contextSelectionReceipts":{}},
            "accountMode":"paper","idempotencyKey":"retained-step"})
    }
    #[test]
    fn managed_step_is_separate_closed_wire_and_old_evaluation_step_rejects_it() {
        let q = request();
        command(&q).unwrap();
        assert!(serde_json::from_value::<data_evaluation::EvaluationStep>(
            q["managedStep"].clone()
        )
        .is_err());
        for field in [
            "providerPayload",
            "checkpoint",
            "sourceSnapshot",
            "warmupOrigin",
            "checkpointSeed",
            "accountRef",
            "latestHead",
        ] {
            let mut q = q.clone();
            q["managedStep"][field] = json!({});
            assert!(command(&q).is_err());
        }
        let mut q = q.clone();
        q["managedStep"]["contextSelectionReceipts"] = json!({"context":"receipt"});
        assert!(command(&q).is_err());
    }
    #[test]
    fn managed_step_refuses_nonexact_heads_future_window_and_unknown_version() {
        for (field, value) in [
            (
                "schemaVersion",
                json!("tradeassembly.managed_stream_evaluation_step.v2"),
            ),
            ("streamHeads", json!({"source":"not-a-digest"})),
            ("streamHeads", json!({})),
        ] {
            let mut q = request();
            q["managedStep"][field] = value;
            assert!(command(&q).is_err());
        }
        let mut q = request();
        q["managedStep"]["step"]["window"]["asOfMs"] = json!(199);
        assert!(command(&q).is_err());
    }
    #[test]
    fn new_local_routes_bind_existing_exact_c3_actions_and_owned_strategy_resource() {
        for (path, action) in [
            (PATH, "strategy.data_run.step"),
            (managed_data_streams::CREATE, "strategy.data_capture.create"),
            (managed_data_streams::APPEND, "strategy.data_capture.create"),
        ] {
            let envelope =
                crate::control_plane::http_envelope("http", "POST", path, &request()).unwrap();
            assert_eq!(envelope.command_name, action);
            assert_eq!(envelope.target_object.as_deref(), Some("s"));
            assert_eq!(envelope.authority.account_mode, "paper");
            assert_eq!(envelope.side_effect_class, "research");
        }
    }
    #[test]
    fn foreign_owner_http_refuses_before_new_stream_or_evaluator_records() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory
            .path()
            .join("owned.sqlite")
            .to_string_lossy()
            .into_owned();
        let service =
            TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db));
        let alice = service.for_authenticated_invocation("test", "alice", None, None);
        alice.bind_owned_object("strategy", "s").unwrap();
        let bob = service.for_authenticated_invocation("test", "bob", None, None);
        let names = [
            "control_plane_commands",
            "managed_stream_step_snapshots_v1",
            "data_evaluation_checkpoints_v1",
            "data_evaluation_sources_v1",
            "plugin_operation_receipts",
        ];
        let before = names.map(|ns| service.runtime().storage.list_json(ns).unwrap());
        for path in [
            PATH,
            managed_data_streams::CREATE,
            managed_data_streams::APPEND,
        ] {
            let result = bob.handle_http("POST", path, request());
            assert_eq!(result.status, 404, "{}", result.body);
            assert_eq!(
                names.map(|ns| service.runtime().storage.list_json(ns).unwrap()),
                before
            );
        }
    }
}
