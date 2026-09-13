// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::backtest_report;
use crate::domain::{LifecycleFsm, RobustnessRunCommand, RobustnessRunFsm, RobustnessRunState};
use crate::ports::{
    AuthorityContext, ClockPort, IdempotencyKey, QueueRequest, RobustnessRunRepository,
    ServiceRuntime, SideEffectContext,
};
use crate::robustness_contracts::{
    RobustnessAttempt, RobustnessAttemptState, RobustnessLifecycleEvent, RobustnessResult,
    RobustnessRunManifest, RobustnessRunManifestContent, RobustnessRunRecord,
    ROBUSTNESS_ATTEMPT_SCHEMA, ROBUSTNESS_RUN_SCHEMA,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const LEASE_MS: i64 = 30_000;
pub const QUEUE: &str = "robustness_runs_v1";
const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

pub type RobustnessExecutor<'a> =
    dyn Fn(&RobustnessRunManifest, &RobustnessAttempt) -> Result<RobustnessResult, String> + 'a;

#[derive(Clone, Debug, PartialEq)]
pub struct CreateRobustnessRun {
    pub idempotency_key: String,
    pub request_hash: String,
    pub manifest: RobustnessRunManifestContent,
    pub context: SideEffectContext,
}

pub fn create(
    runtime: &ServiceRuntime,
    mut request: CreateRobustnessRun,
) -> Result<RobustnessRunRecord, String> {
    if request.idempotency_key != request.manifest.idempotency_key
        || request.request_hash != request.manifest.request_hash
    {
        return Err("robustness_request_conflict".to_string());
    }
    if let Some(existing) = runtime
        .robustness
        .find_run_by_idempotency_key(&request.idempotency_key)?
    {
        return if existing.request_hash == request.request_hash {
            ensure_queued(runtime, &existing)?;
            Ok(existing)
        } else {
            Err("robustness_request_conflict".to_string())
        };
    }
    verify_source(runtime, &request.manifest)?;
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{}:{}", request.idempotency_key, request.request_hash).as_bytes())
    );
    if request.manifest.run_id.trim().is_empty() {
        request.manifest.run_id = format!("robustness_{}", &digest[..16]);
    }
    if request.manifest.first_attempt_id.trim().is_empty() {
        request.manifest.first_attempt_id = format!("{}:attempt:1", request.manifest.run_id);
    }
    let now = runtime.clock.now_ms();
    let manifest = RobustnessRunManifest::build(request.manifest, now)?;
    runtime
        .robustness
        .put_manifest(&manifest, &request.context)?;
    let draft = RobustnessRunRecord {
        schema: ROBUSTNESS_RUN_SCHEMA.to_string(),
        run_id: manifest.content.run_id.clone(),
        manifest_hash: manifest.manifest_hash.clone(),
        request_hash: manifest.content.request_hash.clone(),
        idempotency_key: manifest.content.idempotency_key.clone(),
        state: RobustnessRunState::Draft,
        sequence: 0,
        current_attempt_id: manifest.content.first_attempt_id.clone(),
        fencing_token: 0,
        result_hash: None,
        result: None,
        failure_code: None,
        events: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    runtime.robustness.create_run(&draft, &request.context)?;
    runtime.robustness.put_attempt(
        &new_attempt(&draft.run_id, &draft.current_attempt_id, 1, now),
        &request.context,
    )?;
    let ready = transition(
        runtime.robustness.as_ref(),
        runtime.clock.as_ref(),
        &draft,
        RobustnessRunCommand::ValidateReady,
        &request.context,
        "source_bindings_verified",
    )?;
    let queued = transition(
        runtime.robustness.as_ref(),
        runtime.clock.as_ref(),
        &ready,
        RobustnessRunCommand::Execute,
        &request.context,
        "execution_queued",
    )?;
    enqueue(runtime, &queued)?;
    Ok(queued)
}

pub fn process_with(
    runtime: &ServiceRuntime,
    worker: &str,
    executor: &RobustnessExecutor<'_>,
) -> Result<Option<RobustnessRunRecord>, String> {
    let now = runtime.clock.now_ms();
    let recovery_context = SideEffectContext::new(
        AuthorityContext {
            actor: worker.to_string(),
            surface: "robustness-worker-recovery".to_string(),
            account_mode: "research".to_string(),
        },
        IdempotencyKey::new(format!("robustness-recovery:{worker}:{now}"))?,
    );
    recover(runtime, &recovery_context)?;
    let Some(delivery) = runtime
        .queue
        .claim(QUEUE, worker, now, LEASE_MS, 1)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let Some((run_id, manifest_hash, attempt_id)) = queue_payload(&delivery.payload) else {
        runtime
            .queue
            .acknowledge(&delivery.message_id, delivery.fencing_token)?;
        return Err("robustness_queue_payload_invalid".to_string());
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: worker.to_string(),
            surface: "robustness-worker".to_string(),
            account_mode: "research".to_string(),
        },
        IdempotencyKey::new(format!(
            "robustness-worker:{}:{}",
            delivery.message_id, delivery.fencing_token
        ))?,
    );
    let run = runtime
        .robustness
        .get_run(&run_id)?
        .ok_or_else(|| "robustness_not_found".to_string())?;
    let manifest = runtime
        .robustness
        .get_manifest(&run_id)?
        .ok_or_else(|| "robustness_manifest_integrity_failed".to_string())?;
    let _attempt = runtime
        .robustness
        .get_attempt(&attempt_id)?
        .ok_or_else(|| "robustness_attempt_not_found".to_string())?;
    if run.manifest_hash != manifest_hash
        || manifest.manifest_hash != manifest_hash
        || run.current_attempt_id != attempt_id
    {
        runtime
            .queue
            .acknowledge(&delivery.message_id, delivery.fencing_token)?;
        return Err("robustness_stale_completion".to_string());
    }
    if matches!(
        run.state,
        RobustnessRunState::Completed | RobustnessRunState::Failed | RobustnessRunState::Canceled
    ) {
        runtime
            .queue
            .acknowledge(&delivery.message_id, delivery.fencing_token)?;
        return Ok(Some(run));
    }
    let (running, attempt) = match claim(runtime, &run_id, worker, &context) {
        Ok(claimed) => claimed,
        Err(code) => {
            runtime.queue.retry(
                &delivery.message_id,
                delivery.fencing_token,
                now + LEASE_MS,
                &code,
            )?;
            return Err(code);
        }
    };
    let outcome = executor(&manifest, &attempt);
    if runtime.clock.now_ms() >= delivery.lease_until_ms {
        runtime.queue.retry(
            &delivery.message_id,
            delivery.fencing_token,
            runtime.clock.now_ms() + LEASE_MS,
            "robustness_worker_lease_lost",
        )?;
        return Err("robustness_worker_lease_lost".to_string());
    }
    let terminal = match outcome {
        Ok(result) => complete(
            runtime,
            &run_id,
            &attempt_id,
            worker,
            attempt.fencing_token,
            result,
            &context,
        )?,
        Err(code) => fail(
            runtime,
            &running,
            &attempt,
            worker,
            attempt.fencing_token,
            &code,
            &context,
        )?,
    };
    runtime
        .queue
        .acknowledge(&delivery.message_id, delivery.fencing_token)?;
    Ok(Some(terminal))
}

pub fn claim(
    runtime: &ServiceRuntime,
    run_id: &str,
    worker: &str,
    context: &SideEffectContext,
) -> Result<(RobustnessRunRecord, RobustnessAttempt), String> {
    let run = runtime
        .robustness
        .get_run(run_id)?
        .ok_or_else(|| "robustness_not_found".to_string())?;
    let attempt = runtime
        .robustness
        .get_attempt(&run.current_attempt_id)?
        .ok_or_else(|| "robustness_attempt_not_found".to_string())?;
    let now = runtime.clock.now_ms();
    if run.state != RobustnessRunState::Queued || attempt.state != RobustnessAttemptState::Queued {
        return Err("robustness_worker_lease_lost".to_string());
    }
    let fence = run.fencing_token + 1;
    let mut claimed_attempt = attempt.clone();
    claimed_attempt.state = RobustnessAttemptState::Running;
    claimed_attempt.lease_owner = Some(worker.to_string());
    claimed_attempt.lease_until_ms = Some(now + LEASE_MS);
    claimed_attempt.heartbeat_at_ms = Some(now);
    claimed_attempt.fencing_token = fence;
    claimed_attempt.updated_at_ms = now;
    if !runtime
        .robustness
        .compare_and_put_attempt(&attempt, &claimed_attempt, context)?
    {
        return Err("robustness_worker_lease_lost".to_string());
    }
    let mut replacement = run.clone();
    replacement.fencing_token = fence;
    let running = transition_replacement(
        runtime.robustness.as_ref(),
        runtime.clock.as_ref(),
        &run,
        replacement,
        RobustnessRunCommand::Start,
        context,
        "worker_claimed",
    )?;
    Ok((running, claimed_attempt))
}

pub fn heartbeat(
    runtime: &ServiceRuntime,
    run_id: &str,
    attempt_id: &str,
    worker: &str,
    fencing_token: i64,
    context: &SideEffectContext,
) -> Result<RobustnessAttempt, String> {
    let attempt = runtime
        .robustness
        .get_attempt(attempt_id)?
        .ok_or_else(|| "robustness_attempt_not_found".to_string())?;
    let now = runtime.clock.now_ms();
    if attempt.run_id != run_id
        || attempt.state != RobustnessAttemptState::Running
        || attempt.lease_owner.as_deref() != Some(worker)
        || attempt.fencing_token != fencing_token
        || attempt.lease_until_ms.is_none_or(|until| until <= now)
    {
        return Err("robustness_worker_lease_lost".to_string());
    }
    let mut next = attempt.clone();
    next.heartbeat_at_ms = Some(now);
    next.lease_until_ms = Some(now + LEASE_MS);
    next.updated_at_ms = now;
    if !runtime
        .robustness
        .compare_and_put_attempt(&attempt, &next, context)?
    {
        return Err("robustness_worker_lease_lost".to_string());
    }
    Ok(next)
}

pub fn complete(
    runtime: &ServiceRuntime,
    run_id: &str,
    attempt_id: &str,
    worker: &str,
    fencing_token: i64,
    result: RobustnessResult,
    context: &SideEffectContext,
) -> Result<RobustnessRunRecord, String> {
    let run = runtime
        .robustness
        .get_run(run_id)?
        .ok_or_else(|| "robustness_not_found".to_string())?;
    let attempt = checked_lease(runtime, &run, attempt_id, worker, fencing_token)?;
    let manifest = runtime
        .robustness
        .get_manifest(run_id)?
        .ok_or_else(|| "robustness_manifest_integrity_failed".to_string())?;
    if result.content.run_id != run_id
        || result.content.manifest_hash != manifest.manifest_hash
        || result.content.source != manifest.content.source
    {
        return Err("robustness_result_write_conflict".to_string());
    }
    result.verify()?;
    runtime.robustness.put_result(&result, context)?;
    let mut done_attempt = attempt.clone();
    done_attempt.state = RobustnessAttemptState::Completed;
    done_attempt.lease_until_ms = None;
    done_attempt.updated_at_ms = runtime.clock.now_ms();
    if !runtime
        .robustness
        .compare_and_put_attempt(&attempt, &done_attempt, context)?
    {
        return Err("robustness_worker_lease_lost".to_string());
    }
    let mut replacement = run.clone();
    replacement.result_hash = Some(result.result_hash.clone());
    replacement.result = Some(result);
    transition_replacement(
        runtime.robustness.as_ref(),
        runtime.clock.as_ref(),
        &run,
        replacement,
        RobustnessRunCommand::Complete,
        context,
        "result_persisted",
    )
}

pub fn cancel(
    runtime: &ServiceRuntime,
    run_id: &str,
    context: &SideEffectContext,
) -> Result<RobustnessRunRecord, String> {
    let run = runtime
        .robustness
        .get_run(run_id)?
        .ok_or_else(|| "robustness_not_found".to_string())?;
    if !matches!(
        run.state,
        RobustnessRunState::Draft
            | RobustnessRunState::Ready
            | RobustnessRunState::Queued
            | RobustnessRunState::Running
    ) {
        return Err("robustness_not_cancelable".to_string());
    }
    let attempt = runtime
        .robustness
        .get_attempt(&run.current_attempt_id)?
        .ok_or_else(|| "robustness_attempt_not_found".to_string())?;
    let mut canceled = attempt.clone();
    canceled.state = RobustnessAttemptState::Canceled;
    canceled.lease_until_ms = None;
    canceled.updated_at_ms = runtime.clock.now_ms();
    if !runtime
        .robustness
        .compare_and_put_attempt(&attempt, &canceled, context)?
    {
        return Err("robustness_not_cancelable".to_string());
    }
    transition(
        runtime.robustness.as_ref(),
        runtime.clock.as_ref(),
        &run,
        RobustnessRunCommand::Cancel,
        context,
        "cancellation_requested",
    )
}

pub fn retry(
    runtime: &ServiceRuntime,
    run_id: &str,
    context: &SideEffectContext,
) -> Result<RobustnessRunRecord, String> {
    let run = runtime
        .robustness
        .get_run(run_id)?
        .ok_or_else(|| "robustness_not_found".to_string())?;
    let manifest = runtime
        .robustness
        .get_manifest(run_id)?
        .ok_or_else(|| "robustness_manifest_integrity_failed".to_string())?;
    let attempts = runtime.robustness.list_attempts(run_id)?;
    if run.state != RobustnessRunState::Failed
        || attempts.len() as u32 >= manifest.content.budget.maximum_attempts
    {
        return Err("robustness_not_retryable".to_string());
    }
    let ordinal = attempts.len() as u32 + 1;
    let attempt_id = format!("{run_id}:attempt:{ordinal}");
    runtime.robustness.put_attempt(
        &new_attempt(run_id, &attempt_id, ordinal, runtime.clock.now_ms()),
        context,
    )?;
    let mut replacement = run.clone();
    replacement.current_attempt_id = attempt_id;
    let queued = transition_replacement(
        runtime.robustness.as_ref(),
        runtime.clock.as_ref(),
        &run,
        replacement,
        RobustnessRunCommand::Retry,
        context,
        "retry_queued",
    )?;
    enqueue(runtime, &queued)?;
    Ok(queued)
}

pub fn recover(
    runtime: &ServiceRuntime,
    context: &SideEffectContext,
) -> Result<Vec<RobustnessRunRecord>, String> {
    let mut recovered = Vec::new();
    for run in runtime.robustness.list_runs()? {
        if run.state == RobustnessRunState::Queued {
            ensure_queued(runtime, &run)?;
            recovered.push(run);
            continue;
        }
        if run.state != RobustnessRunState::Running {
            continue;
        }
        let attempt = runtime
            .robustness
            .get_attempt(&run.current_attempt_id)?
            .ok_or_else(|| "robustness_attempt_not_found".to_string())?;
        if attempt
            .lease_until_ms
            .is_some_and(|until| until <= runtime.clock.now_ms())
        {
            let mut failed = attempt.clone();
            failed.state = RobustnessAttemptState::Failed;
            failed.lease_until_ms = None;
            failed.failure_code = Some("robustness_worker_lease_expired".to_string());
            failed.updated_at_ms = runtime.clock.now_ms();
            if runtime
                .robustness
                .compare_and_put_attempt(&attempt, &failed, context)?
            {
                let mut replacement = run.clone();
                replacement.failure_code = failed.failure_code.clone();
                let failed_run = transition_replacement(
                    runtime.robustness.as_ref(),
                    runtime.clock.as_ref(),
                    &run,
                    replacement,
                    RobustnessRunCommand::Fail,
                    context,
                    "worker_lease_expired",
                )?;
                let manifest = runtime
                    .robustness
                    .get_manifest(&run.run_id)?
                    .ok_or_else(|| "robustness_manifest_integrity_failed".to_string())?;
                if (runtime.robustness.list_attempts(&run.run_id)?.len() as u32)
                    < manifest.content.budget.maximum_attempts
                {
                    recovered.push(retry(runtime, &run.run_id, context)?);
                } else {
                    recovered.push(failed_run);
                }
            }
        }
    }
    Ok(recovered)
}

fn fail(
    runtime: &ServiceRuntime,
    run: &RobustnessRunRecord,
    attempt: &RobustnessAttempt,
    worker: &str,
    token: i64,
    code: &str,
    context: &SideEffectContext,
) -> Result<RobustnessRunRecord, String> {
    checked_lease(runtime, run, &attempt.attempt_id, worker, token)?;
    let mut failed_attempt = attempt.clone();
    failed_attempt.state = RobustnessAttemptState::Failed;
    failed_attempt.lease_until_ms = None;
    failed_attempt.failure_code = Some(code.to_string());
    failed_attempt.updated_at_ms = runtime.clock.now_ms();
    if !runtime
        .robustness
        .compare_and_put_attempt(attempt, &failed_attempt, context)?
    {
        return Err("robustness_worker_lease_lost".to_string());
    }
    let mut replacement = run.clone();
    replacement.failure_code = Some(code.to_string());
    transition_replacement(
        runtime.robustness.as_ref(),
        runtime.clock.as_ref(),
        run,
        replacement,
        RobustnessRunCommand::Fail,
        context,
        code,
    )
}

fn ensure_queued(runtime: &ServiceRuntime, run: &RobustnessRunRecord) -> Result<(), String> {
    if run.state == RobustnessRunState::Queued {
        enqueue(runtime, run)?;
    }
    Ok(())
}

fn enqueue(runtime: &ServiceRuntime, run: &RobustnessRunRecord) -> Result<String, String> {
    let attempt = runtime
        .robustness
        .get_attempt(&run.current_attempt_id)?
        .ok_or_else(|| "robustness_attempt_not_found".to_string())?;
    let manifest = runtime
        .robustness
        .get_manifest(&run.run_id)?
        .ok_or_else(|| "robustness_manifest_integrity_failed".to_string())?;
    let queued_at_ms = run.updated_at_ms;
    runtime.queue.enqueue(QueueRequest {
        queue: QUEUE.to_string(),
        payload: serde_json::json!({
            "runId": run.run_id,
            "manifestHash": run.manifest_hash,
            "attemptId": attempt.attempt_id,
        }),
        idempotency_key: IdempotencyKey::new(format!("robustness-queue:{}", attempt.attempt_id))?,
        partition_key: Some(run.run_id.clone()),
        priority: 0,
        available_at_ms: queued_at_ms,
        retention_until_ms: queued_at_ms + RETENTION_MS,
        max_attempts: manifest.content.budget.maximum_attempts,
    })
}

fn queue_payload(value: &serde_json::Value) -> Option<(String, String, String)> {
    Some((
        value.get("runId")?.as_str()?.to_string(),
        value.get("manifestHash")?.as_str()?.to_string(),
        value.get("attemptId")?.as_str()?.to_string(),
    ))
}

fn verify_source(
    runtime: &ServiceRuntime,
    content: &RobustnessRunManifestContent,
) -> Result<(), String> {
    let source_run = runtime
        .backtests
        .get_run(&content.source.source_run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let source_manifest = runtime
        .backtests
        .get_manifest(&content.source.source_run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let source_result = runtime
        .backtests
        .get_result(&content.source.source_run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let dataset = runtime
        .dataset_snapshots
        .get(&content.source.source_dataset_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let strategy = &source_manifest.content.configuration.strategy;
    let strategy_version = runtime
        .storage
        .list_json("strategy_versions")?
        .into_iter()
        .map(|(_, value)| value)
        .find(|value| {
            value.get("id").and_then(Value::as_str) == Some(strategy.strategy_version_id.as_str())
                && value
                    .get("strategyId")
                    .or_else(|| value.get("strategy_id"))
                    .and_then(Value::as_str)
                    == Some(strategy.strategy_id.as_str())
                && value.get("spec").and_then(|spec| {
                    crate::backtest_contracts::canonical_hash(
                        spec,
                        "robustness_strategy_hash_failed",
                    )
                    .ok()
                }) == Some(strategy.strategy_spec_hash.clone())
        })
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let report = backtest_report::project_with_strategy_spec(
        &source_run,
        &source_manifest,
        &source_result,
        &dataset,
        &runtime.backtests.list_attempts(&source_run.run_id)?,
        &runtime.backtests.list_events(&source_run.run_id)?,
        strategy_version.get("spec"),
    )?;
    if source_run.state != crate::domain::BacktestRunState::Completed
        || source_manifest.manifest_hash != content.source.source_manifest_hash
        || source_result.result_hash != content.source.source_result_hash
        || dataset.content_hash != content.source.source_dataset_hash
        || report.report_hash != content.source.source_report_hash
    {
        return Err("robustness_source_integrity_failed".to_string());
    }
    Ok(())
}
fn checked_lease(
    runtime: &ServiceRuntime,
    run: &RobustnessRunRecord,
    attempt_id: &str,
    worker: &str,
    token: i64,
) -> Result<RobustnessAttempt, String> {
    let attempt = runtime
        .robustness
        .get_attempt(attempt_id)?
        .ok_or_else(|| "robustness_attempt_not_found".to_string())?;
    if run.state != RobustnessRunState::Running
        || run.current_attempt_id != attempt_id
        || run.fencing_token != token
        || attempt.state != RobustnessAttemptState::Running
        || attempt.lease_owner.as_deref() != Some(worker)
        || attempt.fencing_token != token
        || attempt
            .lease_until_ms
            .is_none_or(|until| until <= runtime.clock.now_ms())
    {
        return Err("robustness_worker_lease_lost".to_string());
    }
    Ok(attempt)
}
fn new_attempt(run_id: &str, attempt_id: &str, ordinal: u32, now: i64) -> RobustnessAttempt {
    RobustnessAttempt {
        schema: ROBUSTNESS_ATTEMPT_SCHEMA.to_string(),
        attempt_id: attempt_id.to_string(),
        run_id: run_id.to_string(),
        ordinal,
        state: RobustnessAttemptState::Queued,
        lease_owner: None,
        lease_until_ms: None,
        fencing_token: 0,
        heartbeat_at_ms: None,
        created_at_ms: now,
        updated_at_ms: now,
        failure_code: None,
    }
}
fn transition(
    repo: &dyn RobustnessRunRepository,
    clock: &dyn ClockPort,
    expected: &RobustnessRunRecord,
    command: RobustnessRunCommand,
    context: &SideEffectContext,
    detail: &str,
) -> Result<RobustnessRunRecord, String> {
    transition_replacement(
        repo,
        clock,
        expected,
        expected.clone(),
        command,
        context,
        detail,
    )
}
fn transition_replacement(
    repo: &dyn RobustnessRunRepository,
    clock: &dyn ClockPort,
    expected: &RobustnessRunRecord,
    mut replacement: RobustnessRunRecord,
    command: RobustnessRunCommand,
    context: &SideEffectContext,
    detail: &str,
) -> Result<RobustnessRunRecord, String> {
    let outcome = RobustnessRunFsm::transition(expected.state, command)
        .map_err(|_| "robustness_run_transition_invalid".to_string())?;
    replacement.state = outcome.next_state;
    replacement.sequence = expected.sequence + 1;
    replacement.updated_at_ms = clock.now_ms();
    let event = event(expected, &replacement, detail)?;
    replacement.events.push(event.clone());
    if !repo.compare_and_put_run(expected, &replacement, context)? {
        return Err("robustness_stale_completion".to_string());
    }
    repo.append_event(&event, context)?;
    Ok(replacement)
}
fn event(
    from: &RobustnessRunRecord,
    to: &RobustnessRunRecord,
    detail: &str,
) -> Result<RobustnessLifecycleEvent, String> {
    let previous = from.events.last().map(|event| event.event_hash.clone());
    let mut event = RobustnessLifecycleEvent {
        event_id: format!("{}:{}", to.run_id, to.sequence),
        run_id: to.run_id.clone(),
        sequence: to.sequence,
        event_type: format!("robustness_run.{:?}", to.state).to_lowercase(),
        from_state: from.state,
        to_state: to.state,
        attempt_id: to.current_attempt_id.clone(),
        occurred_at_ms: to.updated_at_ms,
        previous_event_hash: previous,
        event_hash: String::new(),
        detail: detail.to_string(),
    };
    event.event_hash = crate::backtest_contracts::canonical_hash(
        &serde_json::json!({"runId": event.run_id, "sequence": event.sequence, "from": event.from_state, "to": event.to_state, "attemptId": event.attempt_id, "previous": event.previous_event_hash, "detail": event.detail}),
        "robustness_event_hash_failed",
    )?;
    Ok(event)
}
