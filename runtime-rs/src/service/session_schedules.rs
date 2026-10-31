//! Immutable host calendar plans. Queue/runner dispatch is a separate operation.
use super::{
    capability_graph, data_collections, data_plans::PlanScope, strategy, TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, ports::*, session_schedule::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tradeassembly_plugin_sdk::session_calendar::{self, SessionCalendar};
use tradeassembly_strategy::spec::{SessionCalendarBinding, SessionTrigger, StrategySpec};
const RECEIPTS: &str = "session_occurrence_plans_v1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleCommand {
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub mode: String,
    pub trigger_ref: String,
    pub session_date: String,
    pub calendar_collection_id: String,
    pub execution_calendar_collection_id: String,
    pub knowledge_cutoff_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleReceipt {
    pub schema_version: String,
    pub receipt_sha256: String,
    pub command: ScheduleCommand,
    pub occurrence: SessionOccurrence,
    pub prepared_at_ms: i64,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "session_schedule_encoding_invalid".into())
}
fn identity(r: &ScheduleReceipt) -> Result<String, String> {
    let mut r = r.clone();
    r.receipt_sha256.clear();
    hash(&r)
}
fn policy(s: &TradeAssemblyService, c: &ScheduleCommand) -> Result<SessionTrigger, String> {
    s.require_object("strategy", &c.scope.strategy_id)
        .map_err(|_| "session_schedule_not_available")?;
    if serde_json::to_vec(c)
        .map_err(|_| "session_schedule_command_invalid")?
        .len()
        > 16384
        || !matches!(c.mode.as_str(), "paper" | "backtest" | "live")
        || c.knowledge_cutoff_ms <= 0
        || c.knowledge_cutoff_ms > s.runtime().clock.now_ms()
    {
        return Err("session_schedule_command_invalid".into());
    }
    let v = strategy::strategy_versions(s, &c.scope.strategy_id)
        .into_iter()
        .find(|v| {
            v["id"] == c.scope.strategy_version_id && v["specHash"] == c.scope.strategy_spec_hash
        })
        .ok_or("session_schedule_version_missing")?;
    if crate::spec::canonical_hash(&v["spec"])? != c.scope.strategy_spec_hash
        || !tradeassembly_strategy::spec::validate(&v["spec"]).valid
    {
        return Err("session_schedule_spec_invalid".into());
    }
    let spec = StrategySpec::model_validate(v["spec"].clone())
        .map_err(|_| "session_schedule_spec_invalid")?;
    let t = spec
        .stages
        .market_clock
        .policy
        .session_triggers
        .iter()
        .find(|t| t.trigger_id == c.trigger_ref)
        .cloned()
        .ok_or("session_schedule_trigger_missing")?;
    let epoch = chrono::DateTime::from_timestamp_millis(s.runtime().clock.now_ms())
        .ok_or("session_schedule_clock_invalid")?
        .to_rfc3339();
    let revision = capability_graph::revision_for_context(
        s,
        &c.capability_graph_revision,
        &c.scope.strategy_id,
        &c.scope.strategy_version_id,
        &c.mode,
        &epoch,
    )
    .map_err(|_| "session_schedule_graph_stale")?;
    if revision["strategySpecHash"] != c.scope.strategy_spec_hash {
        return Err("session_schedule_strategy_mismatch".into());
    }
    let graph: CapabilityGraphResolution = serde_json::from_value(revision["graph"].clone())
        .map_err(|_| "session_schedule_graph_invalid")?;
    for binding in [&t.calendar, &t.execution_window.calendar] {
        let declared = spec
            .capability_requirements
            .required
            .iter()
            .find(|r| r.requirement_id == binding.capability_requirement_ref)
            .ok_or("session_schedule_capability_missing")?;
        let selected = graph
            .nodes
            .iter()
            .find(|n| n.node_id == binding.capability_requirement_ref)
            .and_then(|n| n.selected.as_ref())
            .ok_or("session_schedule_node_unresolved")?;
        if selected.operation.capability != declared.capability {
            return Err("session_schedule_capability_mismatch".into());
        }
    }
    Ok(t)
}
fn calendar(
    s: &TradeAssemblyService,
    c: &ScheduleCommand,
    id: &str,
    b: &SessionCalendarBinding,
    at: i64,
) -> Result<(SessionCalendar, String), String> {
    let r = data_collections::current(s, id)?;
    if r.command.scope != c.scope
        || r.command.capability_graph_revision != c.capability_graph_revision
        || r.command.mode != c.mode
        || r.command.acquisition_node != b.capability_requirement_ref
        || r.command.response_schema_ref != session_calendar::SESSION_CALENDAR_SCHEMA
        || r.host_known_at_ms > c.knowledge_cutoff_ms
    {
        return Err("session_schedule_calendar_binding_invalid".into());
    }
    if r.response_schema_sha256 != hash(&session_calendar::schema())? {
        return Err("session_schedule_calendar_schema_mismatch".into());
    }
    let document: SessionCalendar = SessionCalendar::decode(r.response.payload.clone())
        .map_err(|_| "session_schedule_calendar_invalid")?;
    document.validate()?;
    if document.known_at_ms > r.host_known_at_ms
        || document.known_at_ms > c.knowledge_cutoff_ms
        || at < r.host_known_at_ms
    {
        return Err("session_schedule_calendar_future".into());
    }
    let age = i64::try_from(b.max_age_ms).map_err(|_| "session_schedule_calendar_age_invalid")?;
    // Provider knowledge and host admission ages remain separate, both explicit.
    if at - document.known_at_ms >= age || at - r.host_known_at_ms >= age {
        return Err("session_schedule_calendar_stale".into());
    }
    Ok((document, r.receipt_sha256))
}
fn build(
    s: &TradeAssemblyService,
    c: &ScheduleCommand,
    at: i64,
) -> Result<SessionOccurrence, String> {
    let t = policy(s, c)?;
    let (a, ha) = calendar(s, c, &c.calendar_collection_id, &t.calendar, at)?;
    let (b, hb) = calendar(
        s,
        c,
        &c.execution_calendar_collection_id,
        &t.execution_window.calendar,
        at,
    )?;
    plan_occurrence(
        &c.scope.strategy_id,
        &c.scope.strategy_version_id,
        &c.scope.strategy_spec_hash,
        &t,
        &c.session_date,
        &a,
        &ha,
        &b,
        &hb,
        at,
    )
}
pub fn current(s: &TradeAssemblyService, id: &str) -> Result<ScheduleReceipt, String> {
    let v = s
        .runtime()
        .storage
        .get_json(RECEIPTS, id)?
        .ok_or("session_schedule_missing")?;
    if serde_json::to_vec(&v)
        .map_err(|_| "session_schedule_receipt_invalid")?
        .len()
        > 65536
    {
        return Err("session_schedule_receipt_invalid".into());
    }
    let r: ScheduleReceipt =
        serde_json::from_value(v).map_err(|_| "session_schedule_receipt_invalid")?;
    if r.schema_version != "tradeassembly.session_schedule_receipt.v1"
        || identity(&r)? != r.receipt_sha256
        || r.occurrence.occurrence_id != id
        || build(s, &r.command, r.prepared_at_ms)? != r.occurrence
    {
        return Err("session_schedule_receipt_invalid".into());
    }
    let t = policy(s, &r.command)?;
    calendar(
        s,
        &r.command,
        &r.command.calendar_collection_id,
        &t.calendar,
        s.runtime().clock.now_ms(),
    )?;
    calendar(
        s,
        &r.command,
        &r.command.execution_calendar_collection_id,
        &t.execution_window.calendar,
        s.runtime().clock.now_ms(),
    )?;
    Ok(r)
}
pub fn prepare(
    s: &TradeAssemblyService,
    c: ScheduleCommand,
    ctx: &SideEffectContext,
) -> Result<ScheduleReceipt, String> {
    let live = ctx.authority.account_mode == "live";
    if live != (c.mode == "live") {
        return Err("session_schedule_mode_mismatch".into());
    }
    let now = s.runtime().clock.now_ms();
    let o = build(s, &c, now)?;
    if let Some(v) = s.runtime().storage.get_json(RECEIPTS, &o.occurrence_id)? {
        let prior: ScheduleReceipt =
            serde_json::from_value(v).map_err(|_| "session_schedule_receipt_invalid")?;
        if prior.command != c {
            return Err("session_schedule_occurrence_conflict".into());
        }
        return current(s, &o.occurrence_id);
    }
    let mut r = ScheduleReceipt {
        schema_version: "tradeassembly.session_schedule_receipt.v1".into(),
        receipt_sha256: String::new(),
        command: c,
        occurrence: o,
        prepared_at_ms: now,
    };
    r.receipt_sha256 = identity(&r)?;
    let id = &r.occurrence.occurrence_id;
    let value = serde_json::to_value(&r).map_err(|_| "session_schedule_encoding_invalid")?;
    match s
        .runtime()
        .storage
        .put_json_batch(
            &[StorageWrite::new(RECEIPTS, id, value, ctx.clone())],
            &[StorageExpectation::new(RECEIPTS, id, None)],
        )
        .map_err(|_| "session_schedule_commit_failed")?
    {
        ComparePutOutcome::Updated => Ok(r),
        ComparePutOutcome::Conflict => Err("session_schedule_commit_conflict".into()),
    }
}
fn view(_s: &TradeAssemblyService, r: &ScheduleReceipt) -> Value {
    json!({"schedule":{"schemaVersion":"tradeassembly.session_schedule_summary.v1","receiptSha256":r.receipt_sha256,"scope":r.command.scope,"capabilityGraphRevision":r.command.capability_graph_revision,"mode":r.command.mode,"preparedAtMs":r.prepared_at_ms,"occurrence":r.occurrence},"queued":false,"executionAuthorityGranted":false})
}
pub fn prepare_response(s: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let result = (|| {
        let c: ScheduleCommand = serde_json::from_value(body["schedule"].clone())
            .map_err(|_| "session_schedule_command_invalid".to_string())?;
        let e = crate::control_plane::http_envelope(
            "http",
            "POST",
            "/product/strategies/session-schedules/prepare",
            &body,
        );
        let e = e?;
        let ctx = SideEffectContext::new(
            e.authority,
            IdempotencyKey::new(
                body["idempotencyKey"]
                    .as_str()
                    .ok_or("session_schedule_idempotency_required")?,
            )?,
        )
        .with_agent_execution(s.agent_mcp_execution_context());
        prepare(s, c, &ctx)
    })();
    match result {
        Ok(r) => super::ServiceResponse::ok(view(s, &r)),
        Err(e) => super::ServiceResponse::conflict(&e),
    }
}
pub fn get_response(s: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    match body["occurrenceId"]
        .as_str()
        .ok_or("session_schedule_id_required".to_string())
        .and_then(|id| current(s, id))
    {
        Ok(r) => super::ServiceResponse::ok(view(s, &r)),
        Err(_) => super::ServiceResponse::bad_request("session_schedule_not_available"),
    }
}

pub(crate) fn preflight(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<super::ServiceResponse> {
    let result: Result<(), String> = (|| {
        if path == "/product/strategies/session-schedules/prepare" {
            let c: ScheduleCommand = serde_json::from_value(body["schedule"].clone())
                .map_err(|_| "session_schedule_command_invalid")?;
            let e = crate::control_plane::http_envelope("http", "POST", path, body)?;
            if (e.authority.account_mode == "live") != (c.mode == "live") {
                return Err("session_schedule_mode_mismatch".into());
            }
            let o = build(s, &c, s.runtime().clock.now_ms())?;
            if s.runtime()
                .storage
                .get_json(RECEIPTS, &o.occurrence_id)?
                .is_some()
            {
                let r = current(s, &o.occurrence_id)?;
                if r.command != c {
                    return Err("session_schedule_occurrence_conflict".into());
                }
            }
        } else if path == "/product/strategies/session-schedules/get" {
            current(
                s,
                body["occurrenceId"]
                    .as_str()
                    .ok_or("session_schedule_id_required")?,
            )
            .map_err(|_| "session_schedule_not_available")?;
        }
        Ok(())
    })();
    result.err().map(|e| super::ServiceResponse::conflict(&e))
}
