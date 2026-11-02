//! Owned execution-calendar windows. Immutable evidence, never entry admission.
//! Entry-selection calendars, trigger anchors and missed-entry policies do not
//! decide whether an already-owned position has an eligible execution interval.
use super::{
    capability_graph, data_collections, data_plans::PlanScope, order_plans::management, strategy,
    ServiceResponse, TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, ports::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tradeassembly_plugin_sdk::session_calendar::{self, SessionCalendar, SessionState};
use tradeassembly_strategy::spec::{SessionExecutionWindow, SessionTrigger, StrategySpec};

pub(crate) const PREPARE: &str = "/product/strategies/management-windows/prepare";
const RECEIPTS: &str = "execution_management_windows_v1";
const KEYS: &str = "execution_management_window_keys_v1";
const VERSION: &str = "tradeassembly.management_window.v1";
const PREFIX: &str = "management_window_";
const MAX_BYTES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub scope: PlanScope,
    pub position_id: String,
    pub trigger_ref: String,
    pub execution_calendar_collection_id: String,
}
/// Stable owned lineage remains readable after stop, final settlement or source
/// revocation. It neither substitutes for current configuration nor authorizes IO.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Lineage {
    pub owner_sha256: String,
    pub position_id: String,
    pub original_entry_plan_id: String,
    pub activation_id: String,
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub exit_policy_sha256: String,
    pub entry_settlement_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Subject {
    pub lineage: Lineage,
    pub config_sha256: String,
    pub source_binding_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Waiting,
    Closed,
    CoverageBlocked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub status: Status,
    pub session_date: Option<String>,
    pub open_at_ms: Option<i64>,
    pub close_at_ms: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub schema_version: String,
    pub window_id: String,
    pub receipt_sha256: String,
    pub command: Command,
    pub(crate) subject: Subject,
    pub trigger_policy_sha256: String,
    pub execution_calendar_policy_sha256: String,
    pub calendar_receipt_sha256: String,
    pub calendar_payload_sha256: String,
    pub calendar_ref: String,
    pub time_zone: String,
    pub session_scope: String,
    pub calendar_revision: String,
    pub range_start: String,
    pub range_end: String,
    pub provider_known_at_ms: i64,
    pub host_known_at_ms: i64,
    pub prepared_at_ms: i64,
    /// Exclusive evidence expiry, independent of execution close.
    pub fresh_until_ms: i64,
    pub max_age_ms: u64,
    pub selection: Selection,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Current {
    pub receipt: Receipt,
    pub status: Status,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "management_window_encoding_invalid".into())
}
fn digest(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text(v: &str) -> bool {
    !v.is_empty() && v.len() <= 256 && v.trim() == v && !v.chars().any(char::is_control)
}
fn time(v: i64) -> bool {
    v > 0 && v <= tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
}
fn identity(r: &Receipt) -> Result<String, String> {
    let mut unsigned = r.clone();
    unsigned.window_id.clear();
    unsigned.receipt_sha256.clear();
    hash(&unsigned)
}
fn validate_command(c: &Command) -> Result<(), String> {
    if !text(&c.position_id)
        || !text(&c.trigger_ref)
        || !text(&c.execution_calendar_collection_id)
        || serde_json::to_vec(c)
            .map_err(|_| "management_window_command_invalid")?
            .len()
            > 16_384
    {
        return Err("management_window_command_invalid".into());
    }
    Ok(())
}
fn declared_execution_binding(
    spec: &StrategySpec,
    graph: &CapabilityGraphResolution,
    trigger_ref: &str,
) -> Result<(SessionTrigger, CapabilitySelection), String> {
    if !spec.stages.market_clock.enabled {
        return Err("management_window_market_clock_required".into());
    }
    let trigger = spec
        .stages
        .market_clock
        .policy
        .session_triggers
        .iter()
        .find(|t| t.trigger_id == trigger_ref)
        .cloned()
        .ok_or("management_window_trigger_missing")?;
    trigger.validate()?;
    trigger
        .execution_window
        .calendar
        .timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| "management_window_timezone_unsupported")?;
    let binding = &trigger.execution_window.calendar;
    let declared = spec
        .capability_requirements
        .required
        .iter()
        .find(|r| r.requirement_id == binding.capability_requirement_ref)
        .ok_or("management_window_calendar_capability_missing")?;
    let selected = graph
        .nodes
        .iter()
        .find(|n| n.node_id == binding.capability_requirement_ref)
        .and_then(|n| n.selected.clone())
        .ok_or("management_window_calendar_unresolved")?;
    if selected.operation.capability != declared.capability {
        return Err("management_window_calendar_capability_mismatch".into());
    }
    if selected.operation.effect != "read"
        || !selected
            .operation
            .traits
            .modes
            .iter()
            .any(|mode| mode == "paper")
        || !selected
            .operation
            .traits
            .output_schema_refs
            .iter()
            .any(|id| id == session_calendar::SESSION_CALENDAR_SCHEMA)
    {
        return Err("management_window_calendar_schema_or_mode_invalid".into());
    }
    Ok((trigger, selected))
}
/// Shared save/readiness and owned-window policy binding. It requires no
/// position, so a typo cannot admit an entry before management becomes usable.
pub(crate) fn policy_binding(
    s: &TradeAssemblyService,
    scope: &PlanScope,
    graph_revision: &str,
    trigger_ref: &str,
) -> Result<SessionTrigger, String> {
    if !text(trigger_ref) {
        return Err("management_window_trigger_missing".into());
    }
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| "management_window_not_available")?;
    let version = strategy::strategy_versions(s, &scope.strategy_id)
        .into_iter()
        .find(|v| v["id"] == scope.strategy_version_id && v["specHash"] == scope.strategy_spec_hash)
        .ok_or("management_window_version_missing")?;
    if version["immutable"] != true
        || version["publishedAt"].as_str().is_none_or(str::is_empty)
        || crate::spec::canonical_hash(&version["spec"])? != scope.strategy_spec_hash
        || !tradeassembly_strategy::spec::validate(&version["spec"]).valid
    {
        return Err("management_window_spec_invalid".into());
    }
    let spec = StrategySpec::model_validate(version["spec"].clone())
        .map_err(|_| "management_window_spec_invalid")?;
    let now = s.runtime().clock.trusted_now_ms()?;
    let epoch = chrono::DateTime::from_timestamp_millis(now)
        .ok_or("management_window_clock_invalid")?
        .to_rfc3339();
    let revision = capability_graph::revision_for_context(
        s,
        graph_revision,
        &scope.strategy_id,
        &scope.strategy_version_id,
        "paper",
        &epoch,
    )
    .map_err(|_| "management_window_graph_stale")?;
    if revision["strategySpecHash"] != scope.strategy_spec_hash {
        return Err("management_window_graph_mismatch".into());
    }
    let graph: CapabilityGraphResolution = serde_json::from_value(revision["graph"].clone())
        .map_err(|_| "management_window_graph_invalid")?;
    let (trigger, selected) = declared_execution_binding(&spec, &graph, trigger_ref)?;
    let pin = super::data_binding::pin(s, &selected)?;
    let package = s
        .runtime()
        .plugin_packages
        .get(&pin.package_sha256)?
        .ok_or("management_window_calendar_package_missing")?;
    super::data_evaluation::allowed_data_operation(&selected, false, &package.manifest)?;
    let actual_schema = s
        .runtime()
        .plugin_packages
        .data_schema(
            &pin.package_sha256,
            session_calendar::SESSION_CALENDAR_SCHEMA,
        )?
        .ok_or("management_window_calendar_schema_missing")?;
    if hash(&actual_schema)? != hash(&session_calendar::schema())? {
        return Err("management_window_calendar_schema_mismatch".into());
    }
    Ok(trigger)
}
fn policy(
    s: &TradeAssemblyService,
    scope: &PlanScope,
    position_id: &str,
    trigger_ref: &str,
    subject: &Subject,
) -> Result<SessionTrigger, String> {
    if subject.lineage.scope != *scope || subject.lineage.position_id != position_id {
        return Err("management_window_scope_mismatch".into());
    }
    policy_binding(
        s,
        scope,
        &subject.lineage.capability_graph_revision,
        trigger_ref,
    )
}
/// Current source target for the manager's normal data-collection child.
/// Inputs remain the manager's explicitly configured bounded source inputs.
#[derive(Clone, Debug)]
pub(crate) struct CalendarAcquisition {
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub calendar: tradeassembly_strategy::spec::SessionCalendarBinding,
}
pub(crate) fn acquisition(
    s: &TradeAssemblyService,
    position_id: &str,
    trigger_ref: &str,
) -> Result<CalendarAcquisition, String> {
    if !text(position_id) || !text(trigger_ref) {
        return Err("management_window_command_invalid".into());
    }
    let subject = management::window_subject(s, position_id)?;
    let trigger = policy(
        s,
        &subject.lineage.scope,
        position_id,
        trigger_ref,
        &subject,
    )?;
    Ok(CalendarAcquisition {
        scope: subject.lineage.scope,
        capability_graph_revision: subject.lineage.capability_graph_revision,
        calendar: trigger.execution_window.calendar,
    })
}
/// Civil coverage uses the published IANA zone; no market anchor is inferred.
/// Selection uses explicit provider UTC intervals and provider session-date labels.
pub fn select(
    document: &SessionCalendar,
    execution: &SessionExecutionWindow,
    at: i64,
) -> Result<Selection, String> {
    document.validate()?;
    if !time(at)
        || document.calendar_ref != execution.calendar.calendar_ref
        || document.time_zone != execution.calendar.timezone
        || document.session_scope != execution.calendar.session_scope
    {
        return Err("management_window_calendar_identity_mismatch".into());
    }
    let zone: chrono_tz::Tz = document
        .time_zone
        .parse()
        .map_err(|_| "management_window_timezone_unsupported")?;
    let current_date = chrono::DateTime::from_timestamp_millis(at)
        .ok_or("management_window_clock_invalid")?
        .with_timezone(&zone)
        .date_naive()
        .to_string();
    if execution.open_offset_ms > 43_200_000 || execution.close_offset_ms > 43_200_000 {
        return Err("management_window_offset_invalid".into());
    }
    let mut intervals = Vec::new();
    for day in &document.days {
        if let SessionState::Open {
            open_at_ms,
            close_at_ms,
            ..
        } = day.session
        {
            let open = open_at_ms
                .checked_add(
                    i64::try_from(execution.open_offset_ms)
                        .map_err(|_| "management_window_offset_invalid")?,
                )
                .filter(|v| time(*v))
                .ok_or("management_window_time_invalid")?;
            let close = close_at_ms
                .checked_sub(
                    i64::try_from(execution.close_offset_ms)
                        .map_err(|_| "management_window_offset_invalid")?,
                )
                .filter(|v| time(*v))
                .ok_or("management_window_time_invalid")?;
            if open >= close {
                return Err("management_window_execution_interval_empty".into());
            }
            intervals.push((day.session_date.clone(), open, close));
        }
    }
    // UTC intervals are provider authority. An overnight interval may be active
    // even when its provider session label is the previous local civil date.
    if let Some((date, open, close)) = intervals
        .iter()
        .find(|(_, open, close)| *open <= at && at < *close)
    {
        return Ok(Selection {
            status: Status::Active,
            session_date: Some(date.clone()),
            open_at_ms: Some(*open),
            close_at_ms: Some(*close),
        });
    }
    let Ok(today) = document.day(&current_date) else {
        return Ok(Selection {
            status: Status::CoverageBlocked,
            session_date: None,
            open_at_ms: None,
            close_at_ms: None,
        });
    };
    let today_ordinal = session_calendar::date_ordinal(&current_date)?;
    if let Some((date, open, close)) = intervals.iter().find(|(date, open, _)| {
        session_calendar::date_ordinal(date).is_ok_and(|date| date >= today_ordinal) && *open > at
    }) {
        return Ok(Selection {
            status: Status::Waiting,
            session_date: Some(date.clone()),
            open_at_ms: Some(*open),
            close_at_ms: Some(*close),
        });
    }
    Ok(Selection {
        status: if matches!(today.session, SessionState::Closed { .. }) {
            Status::Closed
        } else {
            Status::CoverageBlocked
        },
        session_date: Some(today.session_date.clone()),
        open_at_ms: None,
        close_at_ms: None,
    })
}

fn fresh_until(provider: i64, host: i64, max_age: u64, at: i64) -> Result<i64, String> {
    if !time(at) || !time(provider) || !time(host) || provider > host || host > at {
        return Err("management_window_calendar_future".into());
    }
    if max_age == 0 || max_age > 31 * 86_400_000 {
        return Err("management_window_calendar_age_invalid".into());
    }
    let age = i64::try_from(max_age).map_err(|_| "management_window_calendar_age_invalid")?;
    let until = provider
        .checked_add(age)
        .and_then(|a| host.checked_add(age).map(|b| a.min(b)))
        .filter(|v| time(*v))
        .ok_or("management_window_time_invalid")?;
    if at >= until {
        return Err("management_window_calendar_stale".into());
    }
    Ok(until)
}
fn calendar(
    s: &TradeAssemblyService,
    c: &Command,
    subject: &Subject,
    trigger: &SessionTrigger,
    at: i64,
) -> Result<(data_collections::CollectionReceipt, SessionCalendar, i64), String> {
    let r = data_collections::current(s, &c.execution_calendar_collection_id)?;
    let b = &trigger.execution_window.calendar;
    if r.command.scope != c.scope
        || r.command.capability_graph_revision != subject.lineage.capability_graph_revision
        || r.command.mode != "paper"
        || r.command.acquisition_node != b.capability_requirement_ref
        || r.command.response_schema_ref != session_calendar::SESSION_CALENDAR_SCHEMA
        || r.response_schema_sha256 != hash(&session_calendar::schema())?
    {
        return Err("management_window_calendar_binding_invalid".into());
    }
    let document = SessionCalendar::decode(r.response.payload.clone())?;
    let until = fresh_until(document.known_at_ms, r.host_known_at_ms, b.max_age_ms, at)?;
    Ok((r, document, until))
}
fn build(s: &TradeAssemblyService, c: Command, at: i64) -> Result<Receipt, String> {
    validate_command(&c)?;
    let subject = management::window_subject(s, &c.position_id)?;
    let trigger = policy(s, &c.scope, &c.position_id, &c.trigger_ref, &subject)?;
    let (calendar, document, until) = calendar(s, &c, &subject, &trigger, at)?;
    let selection = select(&document, &trigger.execution_window, at)?;
    let mut r = Receipt {
        schema_version: VERSION.into(),
        window_id: String::new(),
        receipt_sha256: String::new(),
        command: c,
        subject,
        trigger_policy_sha256: hash(&trigger)?,
        execution_calendar_policy_sha256: hash(&trigger.execution_window)?,
        calendar_receipt_sha256: calendar.receipt_sha256,
        calendar_payload_sha256: calendar.payload_sha256,
        calendar_ref: document.calendar_ref,
        time_zone: document.time_zone,
        session_scope: document.session_scope,
        calendar_revision: document.revision,
        range_start: document.range_start,
        range_end: document.range_end,
        provider_known_at_ms: document.known_at_ms,
        host_known_at_ms: calendar.host_known_at_ms,
        prepared_at_ms: at,
        fresh_until_ms: until,
        max_age_ms: trigger.execution_window.calendar.max_age_ms,
        selection,
    };
    r.receipt_sha256 = identity(&r)?;
    r.window_id = format!("{PREFIX}{}", r.receipt_sha256);
    Ok(r)
}
/// Exact immutable original metadata, independently of present clock/source or
/// active controls. Ownership and original settled-entry lineage still apply.
pub fn historical(s: &TradeAssemblyService, id: &str) -> Result<Receipt, String> {
    if !id.starts_with(PREFIX) || !digest(&id[PREFIX.len()..]) {
        return Err("management_window_not_available".into());
    }
    let raw = s
        .runtime()
        .storage
        .get_json(RECEIPTS, id)?
        .ok_or("management_window_not_available")?;
    if serde_json::to_vec(&raw)
        .map_err(|_| "management_window_receipt_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("management_window_receipt_invalid".into());
    }
    let r: Receipt =
        serde_json::from_value(raw).map_err(|_| "management_window_receipt_invalid")?;
    validate_command(&r.command)?;
    if r.schema_version != VERSION
        || r.window_id != id
        || r.receipt_sha256 != identity(&r)?
        || id != format!("{PREFIX}{}", r.receipt_sha256)
        || r.subject.lineage != management::window_lineage(s, &r.command.position_id)?
        || r.command.scope != r.subject.lineage.scope
        || !time(r.prepared_at_ms)
    {
        return Err("management_window_receipt_invalid".into());
    }
    Ok(r)
}
/// Rechecks exact current owner/config/graph/source/calendar plus both ages.
/// It never rewrites an immutable receipt when waiting becomes active or closed.
pub fn current(s: &TradeAssemblyService, id: &str) -> Result<Current, String> {
    let _timing = super::order_plans::financial_timing("management.window.current");
    // This is a pure validation round: share immutable package bytes across
    // repeated subject/policy/calendar checks, but retain original live owner,
    // configuration, revocation, entitlement, receipt and trusted-clock ports.
    // The scoped service never escapes to acquisition, dispatch or a write.
    super::package_validation::with_round(s, |s| current_in_round(s, id))
}
fn current_in_round(s: &TradeAssemblyService, id: &str) -> Result<Current, String> {
    let r = {
        let _timing = super::order_plans::financial_timing("management.window.historical");
        historical(s, id)?
    };
    let subject = {
        let _timing = super::order_plans::financial_timing("management.window.subject");
        management::window_subject(s, &r.command.position_id)?
    };
    if subject != r.subject {
        return Err("management_window_subject_changed".into());
    }
    let fresh = {
        let _timing = super::order_plans::financial_timing("management.window.rebuild");
        build(s, r.command.clone(), r.prepared_at_ms)?
    };
    if fresh != r {
        return Err("management_window_source_changed".into());
    }
    let now = s.runtime().clock.trusted_now_ms()?;
    if now < r.prepared_at_ms {
        return Err("management_window_clock_reversed".into());
    }
    let trigger = {
        let _timing = super::order_plans::financial_timing("management.window.policy");
        policy(
            s,
            &r.command.scope,
            &r.command.position_id,
            &r.command.trigger_ref,
            &r.subject,
        )?
    };
    let (_, document, until) = {
        let _timing = super::order_plans::financial_timing("management.window.calendar");
        calendar(s, &r.command, &r.subject, &trigger, now)?
    };
    if until != r.fresh_until_ms {
        return Err("management_window_receipt_invalid".into());
    }
    let coverage = select(&document, &trigger.execution_window, now)?;
    let status = if coverage.status == Status::CoverageBlocked {
        Status::CoverageBlocked
    } else {
        match (r.selection.open_at_ms, r.selection.close_at_ms) {
            (Some(open), Some(close)) if open <= now && now < close => Status::Active,
            (Some(open), Some(_)) if now < open => Status::Waiting,
            (Some(_), Some(_)) => Status::Closed,
            _ => {
                if coverage.status == Status::Closed {
                    Status::Closed
                } else {
                    Status::CoverageBlocked
                }
            }
        }
    };
    Ok(Current { receipt: r, status })
}
fn original(
    s: &TradeAssemblyService,
    c: &Command,
    ctx: &SideEffectContext,
) -> Result<Option<Receipt>, String> {
    let lineage = management::window_lineage(s, &c.position_id)?;
    let key = hash(&(lineage.owner_sha256, ctx.idempotency_key.as_str()))?;
    let Some(record) = s.runtime().storage.get_json(KEYS, &key)? else {
        return Ok(None);
    };
    if record["command"]
        != serde_json::to_value(c).map_err(|_| "management_window_command_invalid")?
    {
        return Err("management_window_idempotency_conflict".into());
    }
    historical(
        s,
        record["windowId"]
            .as_str()
            .ok_or("management_window_key_invalid")?,
    )
    .map(Some)
}
pub fn prepare(
    s: &TradeAssemblyService,
    c: Command,
    ctx: &SideEffectContext,
) -> Result<Receipt, String> {
    if ctx.authority.account_mode != "paper" {
        return Err("management_window_paper_required".into());
    }
    validate_command(&c)?;
    if let Some(prior) = original(s, &c, ctx)? {
        return Ok(prior);
    }
    let r = build(s, c.clone(), s.runtime().clock.trusted_now_ms()?)?;
    let key = hash(&(
        &r.subject.lineage.owner_sha256,
        ctx.idempotency_key.as_str(),
    ))?;
    let bytes = serde_json::to_value(&r).map_err(|_| "management_window_receipt_invalid")?;
    if serde_json::to_vec(&bytes)
        .map_err(|_| "management_window_receipt_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("management_window_receipt_too_large".into());
    }
    let outcome = s.runtime().storage.put_json_batch(
        &[
            StorageWrite::new(RECEIPTS, &r.window_id, bytes, ctx.clone()),
            StorageWrite::new(
                KEYS,
                &key,
                json!({"command":c,"windowId":r.window_id}),
                ctx.clone(),
            ),
        ],
        &[
            StorageExpectation::new(RECEIPTS, &r.window_id, None),
            StorageExpectation::new(KEYS, &key, None),
        ],
    )?;
    if outcome != ComparePutOutcome::Updated {
        return original(s, &c, ctx)?.ok_or("management_window_commit_conflict".into());
    }
    Ok(r)
}
fn context(s: &TradeAssemblyService, body: &Value) -> Result<(Command, SideEffectContext), String> {
    if serde_json::to_vec(body)
        .map_err(|_| "management_window_command_invalid")?
        .len()
        > 32_768
    {
        return Err("management_window_command_too_large".into());
    }
    let c: Command = serde_json::from_value(body["managementWindow"].clone())
        .map_err(|_| "management_window_command_invalid")?;
    let e = crate::control_plane::http_envelope("http", "POST", PREPARE, body)?;
    let ctx = SideEffectContext::new(
        e.authority,
        IdempotencyKey::new(
            body["idempotencyKey"]
                .as_str()
                .ok_or("management_window_idempotency_required")?,
        )?,
    )
    .with_agent_execution(s.agent_mcp_execution_context());
    Ok((c, ctx))
}
pub(crate) fn preflight(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    if path != PREPARE {
        return None;
    }
    let result = (|| {
        let (c, ctx) = context(s, body)?;
        validate_command(&c)?;
        if ctx.authority.account_mode != "paper" {
            return Err("management_window_paper_required".into());
        }
        if original(s, &c, &ctx)?.is_none() {
            build(s, c, s.runtime().clock.trusted_now_ms()?)?;
        }
        Ok::<(), String>(())
    })();
    result.err().map(|e| ServiceResponse::conflict(&e))
}
pub fn prepare_response(s: &TradeAssemblyService, body: Value) -> ServiceResponse {
    match context(s, &body).and_then(|(c, ctx)| prepare(s, c, &ctx)) {
        Ok(r) => ServiceResponse::ok(
            json!({"managementWindow":r,"executionAuthorityGranted":false,"queued":false}),
        ),
        Err(e) => ServiceResponse::conflict(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(value: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis()
    }
    fn execution() -> SessionExecutionWindow {
        serde_json::from_value(json!({"calendar":{"capability_requirement_ref":"execution_only","calendar_ref":"XNYS","timezone":"America/New_York","session_scope":"regular_equity","max_age_ms":86400000},"open_offset_ms":0,"close_offset_ms":0})).unwrap()
    }
    fn calendar() -> SessionCalendar {
        SessionCalendar::decode(json!({"schemaVersion":"tradeassembly.session_calendar.v1","calendarRef":"XNYS","timeZone":"America/New_York","sessionScope":"regular_equity","revision":"provider.explicit.1","rangeStart":"2026-03-06","rangeEnd":"2026-03-09","knownAtMs":1,"days":[
            {"sessionDate":"2026-03-06","session":{"status":"open","openAtMs":ms("2026-03-06T14:30:00Z"),"closeAtMs":ms("2026-03-06T21:00:00Z"),"earlyClose":false}},
            {"sessionDate":"2026-03-07","session":{"status":"closed","reason":"explicit_weekend"}},
            {"sessionDate":"2026-03-08","session":{"status":"closed","reason":"explicit_holiday_or_weekend"}},
            {"sessionDate":"2026-03-09","session":{"status":"open","openAtMs":ms("2026-03-09T13:30:00Z"),"closeAtMs":ms("2026-03-09T20:00:00Z"),"earlyClose":false}}
        ]})).unwrap()
    }
    #[test]
    fn provider_dst_closed_days_and_rollover_do_not_infer_market_anchors() {
        let c = calendar();
        let e = execution();
        let friday = select(&c, &e, ms("2026-03-06T15:00:00Z")).unwrap();
        assert_eq!(friday.status, Status::Active);
        assert_eq!(friday.open_at_ms, Some(ms("2026-03-06T14:30:00Z")));
        let weekend = select(&c, &e, ms("2026-03-08T16:00:00Z")).unwrap();
        assert_eq!(weekend.status, Status::Waiting);
        assert_eq!(weekend.session_date.as_deref(), Some("2026-03-09"));
        assert_eq!(weekend.open_at_ms, Some(ms("2026-03-09T13:30:00Z")));
        let monday = select(&c, &e, ms("2026-03-09T13:30:00Z")).unwrap();
        assert_eq!(monday.status, Status::Active);
        assert_eq!(
            select(&c, &e, ms("2026-03-10T15:00:00Z")).unwrap().status,
            Status::CoverageBlocked
        );
    }
    #[test]
    fn early_close_offsets_and_exclusive_close_are_authoritative() {
        let mut c = calendar();
        c.days[3].session = SessionState::Open {
            open_at_ms: ms("2026-03-09T13:30:00Z"),
            close_at_ms: ms("2026-03-09T17:00:00Z"),
            early_close: true,
        };
        let mut e = execution();
        e.open_offset_ms = 60000;
        e.close_offset_ms = 120000;
        let before = select(&c, &e, ms("2026-03-09T13:30:00Z")).unwrap();
        assert_eq!(before.status, Status::Waiting);
        assert_eq!(before.open_at_ms, Some(ms("2026-03-09T13:31:00Z")));
        let open = select(&c, &e, ms("2026-03-09T13:31:00Z")).unwrap();
        assert_eq!(open.status, Status::Active);
        assert_eq!(open.close_at_ms, Some(ms("2026-03-09T16:58:00Z")));
        assert_eq!(
            select(&c, &e, ms("2026-03-09T16:58:00Z")).unwrap().status,
            Status::CoverageBlocked
        );
        e.open_offset_ms = 43_200_000;
        assert_eq!(
            select(&c, &e, ms("2026-03-09T15:00:00Z")).unwrap_err(),
            "management_window_execution_interval_empty"
        );
    }
    #[test]
    fn local_civil_coverage_uses_iana_zone_without_utc_date_guess() {
        let mut c = calendar();
        c.range_start = "2026-03-08".into();
        c.range_end = "2026-03-08".into();
        c.days = vec![c.days[2].clone()];
        // UTC Monday, but still Sunday in the published New York zone.
        let at = ms("2026-03-09T01:00:00Z");
        assert_eq!(select(&c, &execution(), at).unwrap().status, Status::Closed);
        // The same fresh historical-only all-closed document cannot say Monday is closed.
        assert_eq!(
            select(&c, &execution(), ms("2026-03-09T15:00:00Z"))
                .unwrap()
                .status,
            Status::CoverageBlocked
        );
        c.time_zone = "provider-label-without-IANA-definition".into();
        let mut e = execution();
        e.calendar.timezone = c.time_zone.clone();
        assert_eq!(
            select(&c, &e, at).unwrap_err(),
            "management_window_timezone_unsupported"
        );
    }
    #[test]
    fn explicit_overnight_interval_can_be_active_from_previous_session_date() {
        let mut c = calendar();
        c.range_start = "2026-03-06".into();
        c.range_end = "2026-03-06".into();
        c.days.truncate(1);
        c.days[0].session = SessionState::Open {
            open_at_ms: ms("2026-03-07T00:00:00Z"),
            close_at_ms: ms("2026-03-07T12:00:00Z"),
            early_close: false,
        };
        let current = select(&c, &execution(), ms("2026-03-07T06:00:00Z")).unwrap();
        assert_eq!(current.status, Status::Active);
        assert_eq!(current.session_date.as_deref(), Some("2026-03-06"));
        assert_eq!(
            select(&c, &execution(), ms("2026-03-07T12:00:00Z"))
                .unwrap()
                .status,
            Status::CoverageBlocked
        );
    }
    #[test]
    fn selection_calendar_anchor_and_entry_missed_policy_are_not_planner_inputs() {
        let c = calendar();
        let e = execution();
        // No entry-selection calendar or preopen/missed-entry policy is accepted
        // by this API. Existing-position execution follows only this document.
        assert_eq!(
            select(&c, &e, ms("2026-03-09T19:00:00Z")).unwrap().status,
            Status::Active
        );
        let mut wrong = e;
        wrong.calendar.session_scope = "other_product".into();
        assert_eq!(
            select(&c, &wrong, ms("2026-03-09T19:00:00Z")).unwrap_err(),
            "management_window_calendar_identity_mismatch"
        );
    }
    #[test]
    fn both_calendar_clocks_have_exact_exclusive_freshness_bounds() {
        assert_eq!(fresh_until(50, 60, 100, 149).unwrap(), 150);
        assert_eq!(
            fresh_until(50, 60, 100, 150).unwrap_err(),
            "management_window_calendar_stale"
        );
        assert_eq!(
            fresh_until(60, 50, 100, 100).unwrap_err(),
            "management_window_calendar_future"
        );
        assert_eq!(
            fresh_until(50, 101, 100, 100).unwrap_err(),
            "management_window_calendar_future"
        );
        assert_eq!(
            fresh_until(101, 101, 100, 100).unwrap_err(),
            "management_window_calendar_future"
        );
    }
    #[test]
    fn explicit_provider_holiday_is_closed_without_weekday_inference() {
        let mut c = calendar();
        c.days[3].session = SessionState::Closed {
            reason: "provider_declared_fixture_holiday".into(),
        };
        let result = select(&c, &execution(), ms("2026-03-09T15:00:00Z")).unwrap();
        assert_eq!(result.status, Status::Closed);
        assert!(result.open_at_ms.is_none());
    }
    #[test]
    fn local_route_uses_existing_finite_c3_schedule_policy() {
        let body = json!({"managementWindow":{"scope":{"strategyId":"s","strategyVersionId":"v","strategySpecHash":"a".repeat(64)},"positionId":"position_1","triggerRef":"preopen","executionCalendarCollectionId":"collection_1"},"accountMode":"paper","idempotencyKey":"window-route"});
        let envelope = crate::control_plane::http_envelope("http", "POST", PREPARE, &body).unwrap();
        assert_eq!(envelope.command_name, "strategy.session_schedule.prepare");
        assert_eq!(envelope.side_effect_class, "research");
        assert_eq!(envelope.target_object.as_deref(), Some("s"));
        assert_eq!(envelope.authority.account_mode, "paper");
    }
    #[test]
    fn local_command_rejects_caller_session_date_times_and_financial_fields() {
        let c = json!({"scope":{"strategyId":"s","strategyVersionId":"v","strategySpecHash":"a".repeat(64)},"positionId":"position_1","triggerRef":"preopen","executionCalendarCollectionId":"collection_1"});
        assert!(serde_json::from_value::<Command>(c.clone()).is_ok());
        for field in [
            "sessionDate",
            "openAtMs",
            "closeAtMs",
            "accountRef",
            "quantity",
            "price",
            "activationId",
            "capabilityGraphRevision",
            "calendarCollectionId",
        ] {
            let mut bad = c.clone();
            bad[field] = json!(1);
            assert!(serde_json::from_value::<Command>(bad).is_err(), "{field}");
        }
    }
    fn receipt() -> Receipt {
        let scope = PlanScope {
            strategy_id: "s".into(),
            strategy_version_id: "v".into(),
            strategy_spec_hash: "a".repeat(64),
        };
        Receipt {
            schema_version: VERSION.into(),
            window_id: String::new(),
            receipt_sha256: String::new(),
            command: Command {
                scope: scope.clone(),
                position_id: "position_1".into(),
                trigger_ref: "preopen".into(),
                execution_calendar_collection_id: "collection_1".into(),
            },
            subject: Subject {
                lineage: Lineage {
                    owner_sha256: "b".repeat(64),
                    position_id: "position_1".into(),
                    original_entry_plan_id: "entry_1".into(),
                    activation_id: "activation_1".into(),
                    scope,
                    capability_graph_revision: "graph_1".into(),
                    exit_policy_sha256: "c".repeat(64),
                    entry_settlement_sha256: "d".repeat(64),
                },
                config_sha256: "e".repeat(64),
                source_binding_sha256: "f".repeat(64),
            },
            trigger_policy_sha256: "a".repeat(64),
            execution_calendar_policy_sha256: "b".repeat(64),
            calendar_receipt_sha256: "c".repeat(64),
            calendar_payload_sha256: "d".repeat(64),
            calendar_ref: "XNYS".into(),
            time_zone: "America/New_York".into(),
            session_scope: "regular_equity".into(),
            calendar_revision: "one".into(),
            range_start: "2026-03-09".into(),
            range_end: "2026-03-09".into(),
            provider_known_at_ms: 50,
            host_known_at_ms: 60,
            prepared_at_ms: 100,
            fresh_until_ms: 150,
            max_age_ms: 100,
            selection: Selection {
                status: Status::Active,
                session_date: Some("2026-03-09".into()),
                open_at_ms: Some(1),
                close_at_ms: Some(1000),
            },
        }
    }
    #[test]
    fn same_date_refreshed_evidence_and_distinct_owned_lineage_have_distinct_ids() {
        let r = receipt();
        let original = identity(&r).unwrap();
        for altered in [0, 1, 2, 3] {
            let mut next = r.clone();
            match altered {
                0 => next.calendar_receipt_sha256 = "9".repeat(64),
                1 => next.subject.lineage.position_id = "position_2".into(),
                2 => next.subject.config_sha256 = "8".repeat(64),
                _ => next.calendar_revision = "two".into(),
            }
            assert_ne!(original, identity(&next).unwrap());
        }
        let mut signed = r;
        signed.window_id = format!("{PREFIX}{original}");
        signed.receipt_sha256 = original.clone();
        assert_eq!(identity(&signed).unwrap(), original);
    }
    fn binding_fixture() -> (StrategySpec, CapabilityGraphResolution) {
        let mut spec: Value = serde_json::from_str(include_str!(
            "../../../examples/strategy-spec/v3/valid/static-equity.json"
        ))
        .unwrap();
        let binding = json!({"capability_requirement_ref":"execution_calendar","calendar_ref":"XNYS","timezone":"America/New_York","session_scope":"regular_equity","max_age_ms":86400000});
        spec["stages"]["market_clock"]["policy"]["session_triggers"] = json!([{"version":"tradeassembly.session_trigger.v1","trigger_id":"owned_execution","calendar":binding,"anchor":"open","offset_ms":-300000,"missed_run":{"policy":"skip"},"retry":{"max_attempts":1,"backoff_ms":1000},"execution_window":{"calendar":binding,"open_offset_ms":0,"close_offset_ms":0},"selections":[{"signal_requirement_ref":"signal_primary","selector_ref":"primary_symbol"}]}]);
        let mut requirement = spec["capability_requirements"]["required"][0].clone();
        requirement["requirement_id"] = json!("execution_calendar");
        requirement["capability"] = json!("test.session.calendar@1");
        spec["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap()
            .push(requirement);
        let spec = StrategySpec::model_validate(spec).unwrap();
        let mut graph = crate::data_pipeline::tests::fixture().0;
        graph.nodes.truncate(1);
        graph.nodes[0].node_id = "execution_calendar".into();
        let selected = graph.nodes[0].selected.as_mut().unwrap();
        selected.operation.capability = "test.session.calendar@1".into();
        selected.operation.effect = "read".into();
        selected.operation.traits.modes = vec!["paper".into()];
        selected.operation.traits.output_schema_refs =
            vec![session_calendar::SESSION_CALENDAR_SCHEMA.into()];
        (spec, graph)
    }
    #[test]
    fn preentry_rollover_binding_rejects_unknown_trigger_disabled_clock_and_missing_node() {
        let (spec, graph) = binding_fixture();
        assert!(declared_execution_binding(&spec, &graph, "owned_execution").is_ok());
        assert_eq!(
            declared_execution_binding(&spec, &graph, "typo").unwrap_err(),
            "management_window_trigger_missing"
        );
        let mut disabled = spec.clone();
        disabled.stages.market_clock.enabled = false;
        assert_eq!(
            declared_execution_binding(&disabled, &graph, "owned_execution").unwrap_err(),
            "management_window_market_clock_required"
        );
        let mut missing = graph.clone();
        missing.nodes.clear();
        assert_eq!(
            declared_execution_binding(&spec, &missing, "owned_execution").unwrap_err(),
            "management_window_calendar_unresolved"
        );
    }
    #[test]
    fn preentry_rollover_binding_rejects_wrong_calendar_capability_schema_mode_and_timezone() {
        let (spec, graph) = binding_fixture();
        for variant in 0..4 {
            let mut wrong = graph.clone();
            let op = &mut wrong.nodes[0].selected.as_mut().unwrap().operation;
            match variant {
                0 => op.capability = "wrong.calendar".into(),
                1 => op.traits.output_schema_refs = vec!["wrong.schema".into()],
                2 => op.traits.modes = vec!["live".into()],
                _ => op.effect = "write".into(),
            }
            assert!(declared_execution_binding(&spec, &wrong, "owned_execution").is_err());
        }
        let mut unsupported = spec;
        unsupported.stages.market_clock.policy.session_triggers[0]
            .execution_window
            .calendar
            .timezone = "unsupported-provider-label".into();
        assert_eq!(
            declared_execution_binding(&unsupported, &graph, "owned_execution").unwrap_err(),
            "management_window_timezone_unsupported"
        );
    }
}
