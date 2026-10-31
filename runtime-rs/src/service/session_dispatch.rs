//! Durable calendar-occurrence computation. No broker or managed-position writes.
use super::{
    data_collections, data_evaluation, data_plans, instrument_screens, selection_plans,
    session_schedules, TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, ports::*, session_schedule::SessionOccurrenceStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_strategy::spec::SelectorType;
const JOBS: &str = "session_dispatch_jobs_v1";
const QUEUE: &str = "session-dispatch.v1";
const MAX_BYTES: usize = 1_048_576;
const LEASE_MS: i64 = 120_000;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SubjectPolicy {
    AllSelected { max_members: usize },
    Explicit { instruments: Vec<String> },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatchRequest {
    pub occurrence_id: String,
    pub scope: data_plans::PlanScope,
    pub capability_graph_revision: String,
    pub mode: String,
    pub subjects: SubjectPolicy,
    pub event_start_ms: i64,
    pub event_end_ms: i64,
    /// Must match actual node contexts; no guessed interval or resampling.
    pub interval_ms: u64,
    pub max_frames_per_step: u64,
    pub source_captures_by_subject: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_activation_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Request {
    Enqueue { job: Box<DispatchRequest> },
    Advance { job_id: String },
    Inspect { job_id: String },
    StartWorker { job_id: String },
    StopWorker { job_id: String },
    ResumeWorkers {},
    Pause { job_id: String, paused: bool },
}
/// Finite remote computation commands. Background authority is never delegated.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RemoteRequest {
    Enqueue { job: Box<DispatchRequest> },
    Advance { job_id: String },
    Inspect { job_id: String },
    Pause { job_id: String, paused: bool },
}
impl RemoteRequest {
    pub fn local(&self) -> Result<Value, String> {
        value(self)
    }
}
pub(crate) fn remote_binding(
    s: &TradeAssemblyService,
    r: &RemoteRequest,
) -> Result<(data_plans::PlanScope, String, String), String> {
    let who = owner(s)?;
    let request = match r {
        RemoteRequest::Enqueue { job } => job.as_ref().clone(),
        RemoteRequest::Advance { job_id }
        | RemoteRequest::Inspect { job_id }
        | RemoteRequest::Pause { job_id, .. } => {
            let (job, _) = read(s, job_id)?;
            if job.core_revision != revision()? {
                return Err("session_dispatch_source_changed".into());
            }
            // Unpause must not implicitly start a locally authorized worker.
            if matches!(r, RemoteRequest::Pause { paused: false, .. })
                && worker::enabled(s, job_id)?
            {
                return Err("session_dispatch_remote_background_authority_unsupported".into());
            }
            job.request
        }
    };
    if request.execution_activation_id.is_some() {
        return Err("session_dispatch_remote_financial_authority_unsupported".into());
    }
    let schedule = validate_request(s, &request)?;
    let domain = hash(&(
        "tradeassembly.session_dispatch.remote.v1",
        &who,
        revision()?,
        &request,
        &schedule.receipt_sha256,
    ))?;
    Ok((request.scope, request.capability_graph_revision, domain))
}
pub(crate) fn validate_remote_snapshot(
    s: &TradeAssemblyService,
    r: &RemoteRequest,
    snapshot: &Value,
) -> Result<(), String> {
    let old: Job =
        serde_json::from_value(snapshot.clone()).map_err(|_| "session_dispatch_record_invalid")?;
    let (current, _) = read(s, &old.job_id)?;
    if old.record_sha256 != record_hash(&old)?
        || old.request != current.request
        || old.owner_sha256 != current.owner_sha256
        || old.core_revision != current.core_revision
        || old.schedule_sha256 != current.schedule_sha256
    {
        return Err("session_dispatch_record_invalid".into());
    }
    let id = match r {
        RemoteRequest::Enqueue { job } => job_id(job, &owner(s)?)?,
        RemoteRequest::Advance { job_id }
        | RemoteRequest::Inspect { job_id }
        | RemoteRequest::Pause { job_id, .. } => job_id.clone(),
    };
    if old.job_id != id {
        return Err("session_dispatch_record_invalid".into());
    }
    remote_binding(s, r)?;
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Complete,
    CompleteEmpty,
    Partial,
    Failed,
    Uncertain,
    SkippedMissed,
    Closed,
}
impl Status {
    fn terminal(&self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Child {
    key: String,
    path: String,
    body: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Job {
    pub schema_version: String,
    pub job_id: String,
    pub request: DispatchRequest,
    pub core_revision: String,
    pub owner_sha256: String,
    pub schedule_sha256: String,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub status: Status,
    pub paused: bool,
    pub generation: u64,
    pub attempts: u32,
    pub next_attempt_at_ms: i64,
    pub error: Option<String>,
    pub record_sha256: String,
    pub fencing_token: i64,
    queue_message_id: Option<String>,
    pending: Option<Child>,
    results: BTreeMap<String, Value>,
    subjects: Option<Vec<String>>,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "session_dispatch_encoding_invalid".into())
}
fn value<T: Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|_| "session_dispatch_encoding_invalid".into())
}
fn context(key: &str) -> Result<SideEffectContext, String> {
    Ok(SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key)?,
    ))
}
fn owner(s: &TradeAssemblyService) -> Result<String, String> {
    let (issuer, session) = s
        .responsible_human_identity()
        .ok_or("session_dispatch_human_required")?;
    let now = s.runtime().clock.now_ms();
    let claims = s
        .runtime()
        .identity
        .resolve(session, now)
        .map_err(|_| "session_dispatch_identity_unavailable")?;
    if claims.issuer != issuer || claims.expires_at_ms <= now {
        return Err("session_dispatch_identity_unavailable".into());
    }
    hash(
        &s.invocation_owner()
            .ok_or("session_dispatch_human_required")?,
    )
}
fn revision() -> Result<String, String> {
    Ok(crate::build_identity::current()
        .ok_or("session_dispatch_build_identity_required")?
        .revision()
        .into())
}
fn read(s: &TradeAssemblyService, id: &str) -> Result<(Job, Value), String> {
    if !id.starts_with("sessionjob_") || id.len() != 75 {
        return Err("session_dispatch_not_available".into());
    }
    let v = s
        .runtime()
        .storage
        .get_json(JOBS, id)?
        .ok_or("session_dispatch_not_available")?;
    if serde_json::to_vec(&v)
        .map_err(|_| "session_dispatch_record_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("session_dispatch_record_invalid".into());
    }
    let j: Job =
        serde_json::from_value(v.clone()).map_err(|_| "session_dispatch_record_invalid")?;
    s.require_object("strategy", &j.request.scope.strategy_id)
        .map_err(|_| "session_dispatch_not_available")?;
    let actual = s
        .invocation_owner()
        .ok_or("session_dispatch_human_required")?;
    if j.schema_version != "tradeassembly.session_dispatch.v1"
        || j.record_sha256 != record_hash(&j)?
        || j.job_id != id
        || hash(&actual)? != j.owner_sha256
        || job_id(&j.request, &j.owner_sha256)? != id
    {
        return Err("session_dispatch_not_available".into());
    }
    Ok((j, v))
}
fn record_hash(j: &Job) -> Result<String, String> {
    let mut unsigned = j.clone();
    unsigned.record_sha256.clear();
    hash(&unsigned)
}
fn job_id(r: &DispatchRequest, owner: &str) -> Result<String, String> {
    Ok(format!(
        "sessionjob_{}",
        hash(&(owner, &r.scope, &r.occurrence_id, &r.mode))?
    ))
}
fn validate_request(
    s: &TradeAssemblyService,
    r: &DispatchRequest,
) -> Result<session_schedules::ScheduleReceipt, String> {
    s.require_object("strategy", &r.scope.strategy_id)
        .map_err(|_| "session_dispatch_not_available")?;
    if serde_json::to_vec(r)
        .map_err(|_| "session_dispatch_request_invalid")?
        .len()
        > 262_144
        || r.source_captures_by_subject.iter().any(|(subject, refs)| {
            subject.is_empty()
                || subject.len() > 256
                || refs.iter().any(|(node, id)| {
                    node.is_empty() || node.len() > 128 || id.is_empty() || id.len() > 256
                })
        })
    {
        return Err("session_dispatch_request_bounds_invalid".into());
    }
    if r.mode == "backtest" {
        return Err("session_dispatch_historical_universe_unsupported".into());
    }
    if r.mode != "paper"
        || r.interval_ms == 0
        || r.interval_ms > 86_400_000
        || r.event_start_ms <= 0
        || r.event_end_ms <= r.event_start_ms
        || !(1..=64).contains(&r.max_frames_per_step)
        || r.source_captures_by_subject.len() > 32
    {
        return Err("session_dispatch_request_invalid".into());
    }
    let interval = i64::try_from(r.interval_ms).map_err(|_| "session_dispatch_request_invalid")?;
    let span = r
        .event_end_ms
        .checked_sub(r.event_start_ms)
        .ok_or("session_dispatch_request_invalid")?;
    if span % interval != 0 || span / interval > 4096 {
        return Err("session_dispatch_frame_bounds_invalid".into());
    }
    let max_subjects = match &r.subjects {
        SubjectPolicy::AllSelected { max_members } if (1..=32).contains(max_members) => {
            *max_members
        }
        SubjectPolicy::Explicit { instruments }
            if !instruments.is_empty()
                && instruments.len() <= 32
                && instruments.iter().all(|i| !i.is_empty() && i.len() <= 256)
                && instruments.iter().collect::<BTreeSet<_>>().len() == instruments.len() =>
        {
            instruments.len()
        }
        _ => return Err("session_dispatch_subjects_invalid".into()),
    };
    let chunks = (span as u64 / r.interval_ms).div_ceil(r.max_frames_per_step);
    if max_subjects as u64 * (chunks + 1) + 144 > 1024
        || r.source_captures_by_subject.values().any(|v| v.len() > 32)
    {
        return Err("session_dispatch_work_bounds_invalid".into());
    }
    let receipt = session_schedules::current(s, &r.occurrence_id)?;
    if receipt.command.scope != r.scope
        || receipt.command.capability_graph_revision != r.capability_graph_revision
        || receipt.command.mode != r.mode
    {
        return Err("session_dispatch_schedule_binding_mismatch".into());
    }
    let (spec, template) =
        selection_plans::template(s, &r.scope)?.ok_or("session_dispatch_template_missing")?;
    let trigger = spec
        .stages
        .market_clock
        .policy
        .session_triggers
        .iter()
        .find(|t| t.trigger_id == receipt.command.trigger_ref)
        .ok_or("session_dispatch_trigger_missing")?;
    for b in template.selections.values() {
        if !trigger.selections.iter().any(|t| {
            t.signal_requirement_ref == b.market_requirement_ref && t.selector_ref == b.selector_ref
        }) {
            return Err("session_dispatch_selection_not_scheduled".into());
        }
    }
    Ok(receipt)
}
fn persist(s: &TradeAssemblyService, old: Value, j: &mut Job) -> Result<(), String> {
    j.generation = j
        .generation
        .checked_add(1)
        .ok_or("session_dispatch_generation_exhausted")?;
    j.record_sha256 = record_hash(j)?;
    let v = value(j)?;
    if serde_json::to_vec(&v)
        .map_err(|_| "session_dispatch_encoding_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("session_dispatch_record_too_large".into());
    }
    let key = format!("session-job:{}:{}", j.job_id, j.generation);
    if s.runtime()
        .storage
        .compare_and_put_json(JOBS, &j.job_id, old, v, &context(&key)?)?
        != ComparePutOutcome::Updated
    {
        return Err("session_dispatch_revision_conflict".into());
    }
    Ok(())
}
fn ensure_queue(s: &TradeAssemblyService, j: &mut Job) -> Result<(), String> {
    if j.queue_message_id.is_some() {
        return Ok(());
    }
    let old = value(j)?;
    let schedule = session_schedules::current(s, &j.request.occurrence_id)?;
    let due = schedule.occurrence.due_at_ms.unwrap_or(j.created_at_ms);
    let id = s.runtime().queue.enqueue(QueueRequest {
        queue: QUEUE.into(),
        payload: json!({"jobId":j.job_id}),
        idempotency_key: IdempotencyKey::new(format!("queue:{}", j.job_id))?,
        partition_key: Some(j.job_id.clone()),
        priority: 0,
        available_at_ms: due,
        retention_until_ms: due
            .checked_add(7 * 86_400_000)
            .ok_or("session_dispatch_time_invalid")?,
        max_attempts: 32768,
    })?;
    j.queue_message_id = Some(id);
    persist(s, old, j)
}

pub fn enqueue(s: &TradeAssemblyService, r: DispatchRequest) -> Result<Job, String> {
    let who = owner(s)?;
    let current = revision()?;
    let schedule = validate_request(s, &r)?;
    let id = job_id(&r, &who)?;
    if s.runtime().storage.get_json(JOBS, &id)?.is_some() {
        let (mut j, _) = read(s, &id)?;
        if j.request != r
            || j.core_revision != current
            || j.schedule_sha256 != schedule.receipt_sha256
        {
            return Err("session_dispatch_occurrence_conflict".into());
        }
        if !j.status.terminal() {
            ensure_queue(s, &mut j)?;
        }
        return Ok(j);
    }
    let now = s.runtime().clock.now_ms();
    let status = match schedule.occurrence.status {
        SessionOccurrenceStatus::Closed => Status::Closed,
        SessionOccurrenceStatus::SkippedMissed => Status::SkippedMissed,
        SessionOccurrenceStatus::Scheduled => Status::Pending,
    };
    let mut j = Job {
        schema_version: "tradeassembly.session_dispatch.v1".into(),
        job_id: id.clone(),
        request: r,
        core_revision: current,
        owner_sha256: who,
        schedule_sha256: schedule.receipt_sha256,
        created_at_ms: now,
        started_at_ms: None,
        status,
        paused: false,
        generation: 0,
        attempts: 0,
        next_attempt_at_ms: now,
        error: None,
        record_sha256: String::new(),
        fencing_token: 0,
        queue_message_id: None,
        pending: None,
        results: BTreeMap::new(),
        subjects: None,
    };
    j.record_sha256 = record_hash(&j)?;
    if s.runtime().storage.put_json_if_absent(
        JOBS,
        &id,
        value(&j)?,
        &context(&format!("new:{id}"))?,
    )? != ImmutablePutOutcome::Created
    {
        return Err("session_dispatch_revision_conflict".into());
    }
    if !j.status.terminal() {
        ensure_queue(s, &mut j)?;
    }
    Ok(j)
}
// Every child body is retained before its effect; retries never generate a new cutoff.
fn child(j: &Job, key: String, path: &str, mut body: Value) -> Child {
    body["idempotencyKey"] = json!(format!("session-child:{}:{key}", j.job_id));
    body["accountMode"] = json!("paper");
    Child {
        key,
        path: path.into(),
        body,
    }
}
fn next(s: &TradeAssemblyService, j: &mut Job) -> Result<Option<Child>, String> {
    let r = &j.request;
    let (spec, template) =
        selection_plans::template(s, &r.scope)?.ok_or("session_dispatch_template_missing")?;
    let mut selections = BTreeMap::new();
    for (alias, b) in &template.selections {
        let market = spec
            .signal_market_requirements
            .iter()
            .find(|m| m.requirement_id == b.market_requirement_ref)
            .ok_or("session_dispatch_market_missing")?;
        let selector = market
            .selectors
            .iter()
            .find(|v| v.selector_id == b.selector_ref)
            .ok_or("session_dispatch_selector_missing")?;
        match selector.selector_type {
            SelectorType::Screened => {
                let policy = selector
                    .screen
                    .as_ref()
                    .ok_or("session_dispatch_screen_missing")?;
                let mut collections = BTreeMap::new();
                for (port, input) in &policy.inputs {
                    let key = format!("collection:{alias}:{port}");
                    if let Some(v) = j.results.get(&key) {
                        collections.insert(
                            port.clone(),
                            v["collectionId"]
                                .as_str()
                                .ok_or("session_dispatch_result_invalid")?
                                .to_string(),
                        );
                    } else {
                        let command = data_collections::CollectionCommand {
                            scope: r.scope.clone(),
                            capability_graph_revision: r.capability_graph_revision.clone(),
                            mode: r.mode.clone(),
                            acquisition_node: input.capability_requirement_ref.clone(),
                            response_schema_ref: input.response_schema_ref.clone(),
                            input: instrument_screens::materialize_query(
                                &input.query,
                                s.runtime().clock.now_ms(),
                            )?,
                        };
                        return Ok(Some(child(
                            j,
                            key,
                            "/product/strategies/data-collections/create",
                            json!({"collection":command}),
                        )));
                    }
                }
                let key = format!("screen:{alias}");
                if let Some(v) = j.results.get(&key) {
                    selections.insert(
                        alias.clone(),
                        v["selectionId"]
                            .as_str()
                            .ok_or("session_dispatch_result_invalid")?
                            .to_string(),
                    );
                } else {
                    let c = instrument_screens::ScreenCommand {
                        scope: r.scope.clone(),
                        capability_graph_revision: r.capability_graph_revision.clone(),
                        mode: r.mode.clone(),
                        signal_requirement_ref: b.market_requirement_ref.clone(),
                        selector_ref: b.selector_ref.clone(),
                        collections,
                        knowledge_cutoff_ms: s.runtime().clock.now_ms(),
                    };
                    return Ok(Some(child(
                        j,
                        key,
                        "/product/strategies/instrument-screens/create",
                        json!({"screen":c}),
                    )));
                }
            }
            SelectorType::NormalizedInstrument => {}
            _ => return Err("session_dispatch_selector_producer_unavailable".into()),
        }
    }
    if j.subjects.is_none() {
        let alias = &template.trade_selection_ref;
        let b = template
            .selections
            .get(alias)
            .ok_or("session_dispatch_trade_selection_missing")?;
        let market = spec
            .signal_market_requirements
            .iter()
            .find(|m| m.requirement_id == b.market_requirement_ref)
            .ok_or("session_dispatch_market_missing")?;
        let selector = market
            .selectors
            .iter()
            .find(|v| v.selector_id == b.selector_ref)
            .ok_or("session_dispatch_selector_missing")?;
        let members = if selector.selector_type == SelectorType::Screened {
            let receipt = instrument_screens::current(
                s,
                selections
                    .get(alias)
                    .ok_or("session_dispatch_screen_missing")?,
            )?;
            use tradeassembly_plugin_sdk::instrument_set::SelectionStatus;
            match receipt.instrument_set.status {
                SelectionStatus::CompleteNonempty => receipt
                    .instrument_set
                    .members
                    .into_iter()
                    .map(|m| m.instrument_id)
                    .collect::<Vec<_>>(),
                SelectionStatus::CompleteEmpty => {
                    j.status = Status::CompleteEmpty;
                    return Ok(None);
                }
                SelectionStatus::Partial => {
                    j.status = Status::Partial;
                    return Ok(None);
                }
                _ => {
                    j.status = Status::Failed;
                    j.error = Some("session_dispatch_selection_failed".into());
                    return Ok(None);
                }
            }
        } else {
            vec![selector
                .normalized_id
                .clone()
                .ok_or("session_dispatch_static_invalid")?]
        };
        let chosen = match &r.subjects {
            SubjectPolicy::AllSelected { max_members } => {
                if members.len() > *max_members {
                    return Err("session_dispatch_subject_limit_exceeded".into());
                }
                members
            }
            SubjectPolicy::Explicit { instruments } => {
                if instruments.iter().any(|i| !members.contains(i)) {
                    return Err("session_dispatch_subject_not_selected".into());
                }
                instruments.clone()
            }
        };
        if r.source_captures_by_subject
            .keys()
            .any(|i| !chosen.contains(i))
        {
            return Err("session_dispatch_capture_subject_not_selected".into());
        }
        j.subjects = Some(chosen);
    }
    for subject in j.subjects.as_ref().unwrap() {
        let key = format!("plan:{subject}");
        let Some(plan) = j.results.get(&key) else {
            let c = selection_plans::SelectionPlanCommand {
                scope: r.scope.clone(),
                capability_graph_revision: r.capability_graph_revision.clone(),
                mode: r.mode.clone(),
                subject_instrument_id: subject.clone(),
                selection_receipts: selections.clone(),
                source_captures: r
                    .source_captures_by_subject
                    .get(subject)
                    .cloned()
                    .unwrap_or_default(),
            };
            return Ok(Some(child(
                j,
                key,
                "/product/strategies/selection-plans/prepare",
                json!({"selectionPlan":c}),
            )));
        };
        let plan_id = plan["revisionId"]
            .as_str()
            .ok_or("session_dispatch_result_invalid")?;
        let stored = data_plans::current(s, plan_id)?;
        if stored
            .content
            .plan
            .request
            .nodes
            .iter()
            .any(|n| n.context.interval_ms != Some(r.interval_ms))
        {
            return Err("session_dispatch_mixed_interval_unsupported".into());
        }
        let mut start = r.event_start_ms;
        let mut checkpoint = None;
        let mut n: usize = 0;
        while start < r.event_end_ms {
            let key = format!("evaluate:{subject}:{n}");
            let delta = r
                .interval_ms
                .checked_mul(r.max_frames_per_step)
                .and_then(|v| i64::try_from(v).ok())
                .ok_or("session_dispatch_time_invalid")?;
            let end = start
                .checked_add(delta)
                .ok_or("session_dispatch_time_invalid")?
                .min(r.event_end_ms);
            if let Some(result) = j.results.get(&key) {
                checkpoint = Some(
                    result["checkpointSha256"]
                        .as_str()
                        .ok_or("session_dispatch_result_invalid")?
                        .to_string(),
                );
            } else {
                let step = data_evaluation::EvaluationStep {
                    run_id: format!("session-run:{}", hash(&(&j.job_id, subject))?),
                    plan_revision_id: plan_id.into(),
                    window: tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
                        event_start_ms: start,
                        event_end_ms: end,
                        as_of_ms: s.runtime().clock.now_ms(),
                    },
                    expected_checkpoint_sha256: checkpoint,
                };
                return Ok(Some(child(
                    j,
                    key,
                    "/product/strategies/data-runs/step",
                    json!({"step":step}),
                )));
            }
            start = end;
            n += 1;
        }
        if let Some(activation) = &r.execution_activation_id {
            let (_, config) = super::order_plans::decision::run(s, activation)?;
            let automatic = super::execution::typed_cycle::automatic(&config)?;
            if j.subjects
                .as_ref()
                .is_none_or(|subjects| subjects.len() > automatic.max_subjects_per_occurrence)
            {
                return Err("typed_cycle_subject_limit".into());
            }
            let signal = j
                .results
                .get(&format!(
                    "evaluate:{subject}:{}",
                    n.checked_sub(1).ok_or("typed_cycle_signal_missing")?
                ))
                .and_then(|v| v["receiptSha256"].as_str())
                .ok_or("typed_cycle_signal_missing")?;
            for phase in 0..8 {
                let key = format!("execute:{subject}:{phase}");
                if let Some(result) = j.results.get(&key) {
                    match result["status"].as_str() {
                        Some("complete" | "skipped") => break,
                        Some("running") => continue,
                        Some("uncertain") => {
                            j.status = Status::Uncertain;
                            j.error = Some("typed_cycle_reconciliation_required".into());
                            return Ok(None);
                        }
                        Some("blocked") => {
                            j.status = Status::Failed;
                            j.error = Some(
                                result["error"]
                                    .as_str()
                                    .unwrap_or("typed_cycle_blocked")
                                    .into(),
                            );
                            return Ok(None);
                        }
                        _ => return Err("typed_cycle_result_invalid".into()),
                    }
                }
                return Ok(Some(child(
                    j,
                    key,
                    super::execution::typed_cycle::INTERNAL,
                    json!({"cycle":{"scope":r.scope,"activationId":activation,"occurrenceId":r.occurrence_id,"planRevisionId":plan_id,"signalReceiptSha256":signal}}),
                )));
            }
        }
    }
    j.status = Status::Complete;
    Ok(None)
}
fn projection(c: &Child, body: &Value) -> Result<Value, String> {
    if c.path == super::execution::typed_cycle::INTERNAL {
        if body["executionAuthorityGranted"] != false
            || !matches!(
                body["status"].as_str(),
                Some("running" | "complete" | "skipped" | "blocked" | "uncertain")
            )
        {
            return Err("typed_cycle_result_invalid".into());
        }
        return Ok(body.clone());
    }
    if c.key.starts_with("collection:") {
        return Ok(
            json!({"collectionId":body["collection"]["collectionId"].as_str().ok_or("session_dispatch_collection_result_invalid")?}),
        );
    }
    if c.key.starts_with("screen:") {
        return Ok(
            json!({"selectionId":body["selection"]["selectionId"].as_str().ok_or("session_dispatch_screen_result_invalid")?}),
        );
    }
    if c.key.starts_with("plan:") {
        return Ok(
            json!({"revisionId":body["plan"]["revisionId"].as_str().ok_or("session_dispatch_plan_result_invalid")?}),
        );
    }
    Ok(
        json!({"receiptSha256":body["receipt"]["receiptSha256"].as_str().ok_or("session_dispatch_evaluation_result_invalid")?,"checkpointSha256":body["receipt"]["checkpoint"]["checkpointSha256"].as_str().ok_or("session_dispatch_evaluation_result_invalid")?,"status":body["receipt"]["status"],"framesProcessed":body["receipt"]["framesProcessed"]}),
    )
}
pub fn advance(s: &TradeAssemblyService, id: &str) -> Result<Job, String> {
    let who = owner(s)?;
    let (mut j, _) = read(s, id)?;
    if j.owner_sha256 != who || j.core_revision != revision()? {
        return Err("session_dispatch_source_changed".into());
    }
    if j.status.terminal() || j.paused {
        return Ok(j);
    }
    let schedule = validate_request(s, &j.request)?;
    if schedule.receipt_sha256 != j.schedule_sha256 {
        return Err("session_dispatch_schedule_changed".into());
    }
    let now = s.runtime().clock.now_ms();
    let due = schedule
        .occurrence
        .due_at_ms
        .ok_or("session_dispatch_due_missing")?;
    if now < due || now < j.next_attempt_at_ms {
        return Ok(j);
    }
    let (spec, _) = selection_plans::template(s, &j.request.scope)?
        .ok_or("session_dispatch_template_missing")?;
    let retry = spec
        .stages
        .market_clock
        .policy
        .session_triggers
        .iter()
        .find(|t| t.trigger_id == schedule.command.trigger_ref)
        .ok_or("session_dispatch_trigger_missing")?
        .retry
        .clone();
    ensure_queue(s, &mut j)?;
    let lease = s.runtime().leases.acquire(
        &format!("session-dispatch:{id}"),
        &format!("worker:{}", j.owner_sha256),
        now,
        LEASE_MS,
    )?;
    let Some(lease) = lease else { return Ok(j) };
    let result = (|| {
        // Re-read after the fence; stale callers cannot overwrite another phase.
        let (loaded, mut old) = read(s, id)?;
        j = loaded;
        j.fencing_token = lease.fencing_token;
        persist(s, old, &mut j)?;
        old = value(&j)?;
        if j.status.terminal() || j.paused {
            return Ok(j.clone());
        }
        let deliveries =
            s.runtime()
                .queue
                .claim_partition(QUEUE, id, &lease.owner, now, LEASE_MS)?;
        let Some(delivery) = deliveries.first() else {
            return Ok(j.clone());
        };
        if deliveries.len() != 1 {
            return Err("session_dispatch_queue_contract_invalid".into());
        }
        if j.started_at_ms.is_none() {
            if now
                > schedule
                    .occurrence
                    .latest_start_at_ms
                    .ok_or("session_dispatch_latest_missing")?
            {
                j.status = Status::SkippedMissed;
                persist(s, old.clone(), &mut j)?;
                s.runtime()
                    .queue
                    .acknowledge(&delivery.message_id, delivery.fencing_token)?;
                return Ok(j.clone());
            }
            j.started_at_ms = Some(now);
            j.status = Status::Running;
        }
        if j.pending.is_none() {
            match next(s, &mut j) {
                Ok(next) => j.pending = next,
                Err(error) => {
                    j.error = Some(error);
                    j.status = Status::Failed;
                }
            }

            persist(s, old.clone(), &mut j)?;
            old = value(&j)?;
            if j.status.terminal() {
                s.runtime()
                    .queue
                    .acknowledge(&delivery.message_id, delivery.fencing_token)?;
                return Ok(j.clone());
            }
        }
        let c = j.pending.clone().ok_or("session_dispatch_child_missing")?;
        if j.attempts >= retry.max_attempts {
            j.status = Status::Uncertain;
            j.error = Some("session_dispatch_attempts_exhausted_reconciliation_required".into());
            persist(s, old, &mut j)?;
            s.runtime()
                .queue
                .acknowledge(&delivery.message_id, delivery.fencing_token)?;
            return Ok(j.clone());
        }
        j.attempts += 1;
        persist(s, old, &mut j)?;
        old = value(&j)?;
        let response = if c.path == super::execution::typed_cycle::INTERNAL {
            super::execution::typed_cycle::response(s, &c.body)
        } else {
            s.handle_http("POST", &c.path, c.body.clone())
        };
        // Even if authority expires during the child, retain original domain
        // evidence and pending request. Never commit through a stale lease.
        owner(s)?;
        let current = s.runtime().leases.current(&lease.resource)?;
        if current.as_ref() != Some(&lease) || lease.expires_at_ms <= s.runtime().clock.now_ms() {
            return Err("session_dispatch_lease_stale".into());
        }
        if response.status == 200
            && c.path == super::execution::typed_cycle::INTERNAL
            && response.body["nextAttemptAtMs"]
                .as_i64()
                .is_some_and(|time| time > s.runtime().clock.now_ms())
        {
            j.next_attempt_at_ms = response.body["nextAttemptAtMs"].as_i64().unwrap();
            j.attempts = 0;
            j.error = None;
            persist(s, old, &mut j)?;
            s.runtime().queue.retry(
                &delivery.message_id,
                delivery.fencing_token,
                j.next_attempt_at_ms,
                "waiting for declared execution session",
            )?;
            return Ok(j.clone());
        }
        if response.status == 200 {
            j.results
                .insert(c.key.clone(), projection(&c, &response.body)?);
            j.pending = None;
            j.attempts = 0;
            j.error = None;
            j.next_attempt_at_ms = s.runtime().clock.now_ms();
        } else {
            j.error = Some(
                super::service_error_code(&response.body)
                    .unwrap_or_else(|| "session_dispatch_child_failed".into()),
            );
            if j.error.as_deref() == Some("instrument_screen_input_incomplete") {
                j.status = Status::Partial;
            } else if j.attempts >= retry.max_attempts {
                j.status = Status::Failed;
            }
            j.next_attempt_at_ms = s
                .runtime()
                .clock
                .now_ms()
                .checked_add(retry.backoff_ms as i64)
                .ok_or("session_dispatch_time_invalid")?;
        }
        persist(s, old, &mut j)?;
        if j.status.terminal() {
            s.runtime()
                .queue
                .acknowledge(&delivery.message_id, delivery.fencing_token)?;
        } else {
            s.runtime().queue.retry(
                &delivery.message_id,
                delivery.fencing_token,
                j.next_attempt_at_ms,
                "next retained computation phase",
            )?;
        }
        Ok(j.clone())
    })();
    let _ = s.runtime().leases.release(&lease);
    result
}
pub fn run(s: &TradeAssemblyService, input: Value) -> Value {
    if input.get("kind") == Some(&json!("resume_workers")) {
        let parsed = serde_json::from_value::<Request>(input.clone());
        return match parsed {
            Ok(Request::ResumeWorkers {}) => match worker::resume_owned(s) {
                Ok(n) => {
                    json!({"ok":true,"body":{"workersResumed":n,"executionAuthorityGranted":false}})
                }
                Err(e) => json!({"ok":false,"error":{"code":e}}),
            },
            _ => json!({"ok":false,"error":{"code":"session_dispatch_command_invalid"}}),
        };
    }
    let result = (|| {
        let r: Request =
            serde_json::from_value(input).map_err(|_| "session_dispatch_command_invalid")?;
        match r {
            Request::Enqueue { job } => enqueue(s, *job),
            Request::Advance { job_id } => advance(s, &job_id),
            Request::Inspect { job_id } => Ok(read(s, &job_id)?.0),
            Request::StartWorker { job_id } => {
                worker::start(s, &job_id)?;
                Ok(read(s, &job_id)?.0)
            }
            Request::StopWorker { job_id } => {
                worker::stop(s, &job_id)?;
                Ok(read(s, &job_id)?.0)
            }
            Request::ResumeWorkers {} => {
                Err("session_dispatch_resume_requires_dedicated_result".into())
            }
            Request::Pause { job_id, paused } => {
                owner(s)?;
                let (mut j, old) = read(s, &job_id)?;
                j.paused = paused;
                persist(s, old, &mut j)?;
                worker::wake(s, &job_id);
                if !paused && worker::enabled(s, &job_id)? && !j.status.terminal() {
                    worker::start(s, &job_id)?;
                }
                Ok(j)
            }
        }
    })();
    match result {
        Ok(job) => {
            json!({"ok":true,"body":{"worker":worker::status(s,&job.job_id),"job":job,"currentness":"not_checked","executionAuthorityGranted":false,"brokerOrders":false}})
        }
        Err(e) => json!({"ok":false,"error":{"code":e}}),
    }
}

#[cfg(test)]
mod tests;

mod worker;
pub(crate) fn resume_local_workers(s: &TradeAssemblyService) {
    worker::resume_local(s);
}

#[cfg(test)]
pub(crate) fn wake_for_test(s: &TradeAssemblyService, id: &str) {
    worker::wake(s, id);
}

#[cfg(test)]
pub(crate) fn halt_worker_for_test(s: &TradeAssemblyService, id: &str) {
    worker::halt_for_test(s, id);
}
