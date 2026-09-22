// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{capability_graph, ServiceResponse, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::backtest_contracts::{
    canonical_hash, AccountModel, BacktestAttempt, BacktestAttemptState, BacktestConfiguration,
    BacktestDiagnostic, BacktestLifecycleEvent, BacktestOrderType, BacktestResult,
    BacktestRiskConfiguration, BacktestRunManifest, BacktestRunManifestContent, BacktestRunRecord,
    CapabilityGraphBinding, CapitalConfiguration, CostConfiguration, DatasetBinding,
    EndOfDataPolicy, EvaluationTiming, ExecutionConfiguration, FillPolicy, FillTiming,
    InstrumentConfiguration, LiquidityConfiguration, MissingDataPolicy, PriceSource,
    StrategyVersionBinding, BACKTEST_ATTEMPT_SCHEMA, BACKTEST_CONFIGURATION_SCHEMA,
    BACKTEST_MANIFEST_SCHEMA, BACKTEST_RUN_SCHEMA,
};
use crate::domain::{BacktestRunCommand, BacktestRunFsm, BacktestRunState, LifecycleFsm};
use crate::ports::{
    AuthorityContext, IdempotencyKey, QueueDelivery, QueueRequest, SideEffectContext,
};
use crate::strategy_kernel::portfolio_program::ResearchProgram;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const QUEUE: &str = "backtest_runs_v1";
const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const LEASE_MS: i64 = 30_000;

pub type BacktestExecutor<'a> =
    dyn Fn(&BacktestRunManifest, &BacktestAttempt) -> Result<Option<BacktestResult>, String> + 'a;

struct WorkerLease<'a> {
    message_id: &'a str,
    fencing_token: i64,
    lease_until_ms: i64,
}

pub fn process(service: &TradeAssemblyService, worker: &str) -> ServiceResponse {
    if service.invocation_owner().is_some() {
        return ServiceResponse::object_unavailable();
    }
    process_with(service, worker, &|manifest, _| {
        execute_manifest(service, manifest)
    })
}

/// Owner-facing processing never clears authority or claims another run.
pub fn process_request(
    service: &TradeAssemblyService,
    worker: &str,
    run_id: Option<&str>,
) -> ServiceResponse {
    match run_id {
        Some(run_id) => process_run(service, worker, run_id),
        None => process(service, worker),
    }
}

pub fn process_run(service: &TradeAssemblyService, worker: &str, run_id: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("backtest_run_id_required");
    }
    if let Err(response) = service.require_object("backtest_run", run_id) {
        return response;
    }
    let runtime = service.runtime();
    let deliveries =
        match runtime
            .queue
            .claim_partition(QUEUE, run_id, worker, runtime.clock.now_ms(), LEASE_MS)
        {
            Ok(deliveries) => deliveries,
            Err(code) if code == "queue_partition_claim_unsupported" => {
                return ServiceResponse::conflict("queue_partition_claim_unsupported");
            }
            Err(_) => return ServiceResponse::conflict("backtest_scoped_claim_failed"),
        };
    let Some(delivery) = deliveries.into_iter().next() else {
        return get(service, run_id);
    };
    if delivery.partition_key.as_deref() != Some(run_id)
        || delivery.payload["runId"].as_str() != Some(run_id)
    {
        return ServiceResponse::conflict("backtest_queue_binding_invalid");
    }
    process_delivery(service, worker, delivery, &|manifest, _| {
        execute_manifest(service, manifest)
    })
}

fn execute_manifest(
    service: &TradeAssemblyService,
    manifest: &BacktestRunManifest,
) -> Result<Option<BacktestResult>, String> {
    let version = exact_strategy_version(service, manifest)
        .ok_or_else(|| "backtest_strategy_version_required".to_string())?;
    let snapshot = service
        .runtime()
        .dataset_snapshots
        .get(&manifest.content.configuration.dataset.dataset_id)?
        .ok_or_else(|| "backtest_dataset_unavailable".to_string())?;
    crate::backtest_engine::execute(
        manifest,
        &version,
        &snapshot,
        service.runtime().clock.now_ms(),
    )
    .map(Some)
}

pub fn replay(service: &TradeAssemblyService, run_id: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("backtest_not_found");
    }
    if let Err(response) = service.require_object("backtest_run", run_id) {
        return response;
    }
    let runtime = service.runtime();
    let (Some(run), Some(manifest), Some(expected)) = (
        runtime.backtests.get_run(run_id).ok().flatten(),
        runtime.backtests.get_manifest(run_id).ok().flatten(),
        runtime.backtests.get_result(run_id).ok().flatten(),
    ) else {
        return ServiceResponse::bad_request("backtest_not_found");
    };
    if run.state != BacktestRunState::Completed {
        return ServiceResponse::conflict("backtest_not_replayable");
    }
    let Some(version) = exact_strategy_version(service, &manifest) else {
        return ServiceResponse::bad_request("backtest_strategy_version_required");
    };
    let Some(snapshot) = runtime
        .dataset_snapshots
        .get(&manifest.content.configuration.dataset.dataset_id)
        .ok()
        .flatten()
    else {
        return ServiceResponse::bad_request("backtest_dataset_unavailable");
    };
    match crate::backtest_engine::replay(&manifest, &version, &snapshot, &expected) {
        Ok(result) => ServiceResponse::ok(json!({
            "runId": run_id,
            "status": "verified",
            "resultHash": result.result_hash,
            "navigation": navigation(service, run_id, &manifest.content.configuration.strategy.strategy_id),
            "noAdvice": LEGAL_BOUNDARY,
        })),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn create(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let mut configuration: BacktestConfiguration = match body.get("configuration") {
        Some(configuration) => match serde_json::from_value(configuration.clone()) {
            Ok(configuration) => configuration,
            Err(_) => return ServiceResponse::bad_request("backtest_config_invalid"),
        },
        None => match compatibility_configuration(service, &body) {
            Ok(configuration) => configuration,
            Err(code) => return ServiceResponse::bad_request(&code),
        },
    };
    if let Err(code) = resolve_capability_graph(service, &body, &mut configuration) {
        return ServiceResponse::bad_request(&code);
    }
    if let Err(response) = service.require_object("strategy", &configuration.strategy.strategy_id) {
        return response;
    }
    if let Err(response) =
        service.require_object("dataset_snapshot", &configuration.dataset.dataset_id)
    {
        return response;
    }
    let validation = configuration.validate();
    if !validation.ready {
        return ServiceResponse::bad_request_with_details(
            "backtest_config_invalid",
            json!({"diagnostics": validation.diagnostics}),
        );
    }
    if !strategy_version_matches(service, &configuration) {
        return ServiceResponse::bad_request("backtest_strategy_version_required");
    }
    let Some(strategy_version) = exact_strategy_version_for_configuration(service, &configuration)
    else {
        return ServiceResponse::bad_request("backtest_strategy_version_required");
    };
    if strategy_version
        .get("spec")
        .ok_or(())
        .and_then(|spec| ResearchProgram::compile_json(spec).map_err(|_| ()))
        .is_err()
    {
        return ServiceResponse::bad_request("backtest_strategy_semantics_unsupported");
    }
    let snapshot = match service
        .runtime()
        .dataset_snapshots
        .get(&configuration.dataset.dataset_id)
    {
        Ok(Some(snapshot)) if snapshot.verify().is_ok() => snapshot,
        _ => return ServiceResponse::bad_request("backtest_dataset_integrity_failed"),
    };
    if snapshot.snapshot_ref != configuration.dataset.snapshot_ref
        || snapshot.content_hash != configuration.dataset.content_hash
    {
        return ServiceResponse::bad_request("backtest_dataset_integrity_failed");
    }
    let idempotency_key = match body
        .get("idempotencyKey")
        .or_else(|| body.get("idempotency_key"))
        .and_then(Value::as_str)
    {
        Some(value) => match IdempotencyKey::new(value) {
            Ok(key) => key,
            Err(_) => return ServiceResponse::bad_request("backtest_request_conflict"),
        },
        None => return ServiceResponse::bad_request("backtest_request_conflict"),
    };
    let request_hash = match request_hash(&body, &configuration) {
        Ok(hash) => hash,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    let runtime = service.runtime();
    if let Ok(Some(existing)) = runtime
        .backtests
        .find_run_by_idempotency_key(idempotency_key.as_str())
    {
        if existing.request_hash == request_hash {
            return ServiceResponse::created(run_value(&runtime, existing, true, false));
        }
        return ServiceResponse::conflict("backtest_request_conflict");
    }
    let now = runtime.clock.now_ms();
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{}:{}", idempotency_key.as_str(), request_hash).as_bytes())
    );
    let run_id = body
        .get("runId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("backtest_{}", &digest[..16]));
    if service
        .bind_inherited_object(
            "backtest_run",
            &run_id,
            "strategy",
            &configuration.strategy.strategy_id,
        )
        .is_err()
    {
        return ServiceResponse::object_unavailable();
    }
    let attempt_id = format!("{run_id}:attempt:1");
    let context = context(&body, idempotency_key.clone());
    let manifest = match BacktestRunManifest::build(
        BacktestRunManifestContent {
            schema: BACKTEST_MANIFEST_SCHEMA.to_string(),
            run_id: run_id.clone(),
            request_hash: request_hash.clone(),
            idempotency_key: idempotency_key.as_str().to_string(),
            configuration,
            validation,
            authority: context.authority.clone(),
            client: safe_token_field(&body, "client", "self"),
            purpose: safe_token_field(&body, "purpose", "strategy_backtest_research"),
            evidence_refs: if body.get("evidenceRefs").is_some() {
                evidence_hashes(&body, "evidenceRefs")
            } else {
                evidence_hashes(&body, "evidence_refs")
            },
            first_attempt_id: attempt_id.clone(),
        },
        now,
    ) {
        Ok(manifest) => manifest,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    admit_manifest(service, manifest, context)
}

pub fn create_from_manifest(
    service: &TradeAssemblyService,
    manifest: BacktestRunManifest,
) -> ServiceResponse {
    if manifest.verify().is_err() || !manifest.content.validation.ready {
        return ServiceResponse::bad_request("backtest_config_invalid");
    }
    if manifest
        .content
        .authority
        .account_mode
        .eq_ignore_ascii_case("live")
    {
        return ServiceResponse::forbidden("backtest_live_mode_not_allowed");
    }
    if let Err(response) = service.require_object(
        "strategy",
        &manifest.content.configuration.strategy.strategy_id,
    ) {
        return response;
    }
    if let Err(response) = service.require_object(
        "dataset_snapshot",
        &manifest.content.configuration.dataset.dataset_id,
    ) {
        return response;
    }
    if service
        .bind_inherited_object(
            "backtest_run",
            &manifest.content.run_id,
            "strategy",
            &manifest.content.configuration.strategy.strategy_id,
        )
        .is_err()
    {
        return ServiceResponse::object_unavailable();
    }
    if !strategy_version_matches(service, &manifest.content.configuration) {
        return ServiceResponse::bad_request("backtest_strategy_version_required");
    }
    let Some(strategy_version) =
        exact_strategy_version_for_configuration(service, &manifest.content.configuration)
    else {
        return ServiceResponse::bad_request("backtest_strategy_version_required");
    };
    if strategy_version
        .get("spec")
        .ok_or(())
        .and_then(|spec| ResearchProgram::compile_json(spec).map_err(|_| ()))
        .is_err()
    {
        return ServiceResponse::bad_request("backtest_strategy_semantics_unsupported");
    }
    let snapshot = match service
        .runtime()
        .dataset_snapshots
        .get(&manifest.content.configuration.dataset.dataset_id)
    {
        Ok(Some(snapshot)) if snapshot.verify().is_ok() => snapshot,
        _ => return ServiceResponse::bad_request("backtest_dataset_integrity_failed"),
    };
    if snapshot.snapshot_ref != manifest.content.configuration.dataset.snapshot_ref
        || snapshot.content_hash != manifest.content.configuration.dataset.content_hash
    {
        return ServiceResponse::bad_request("backtest_dataset_integrity_failed");
    }
    let idempotency_key = match IdempotencyKey::new(&manifest.content.idempotency_key) {
        Ok(key) => key,
        Err(_) => return ServiceResponse::bad_request("backtest_request_conflict"),
    };
    let context = SideEffectContext::new(manifest.content.authority.clone(), idempotency_key);
    admit_manifest(service, manifest, context)
}

fn admit_manifest(
    service: &TradeAssemblyService,
    manifest: BacktestRunManifest,
    context: SideEffectContext,
) -> ServiceResponse {
    let runtime = service.runtime();
    let run_id = manifest.content.run_id.clone();
    let request_hash = manifest.content.request_hash.clone();
    let idempotency_key = manifest.content.idempotency_key.clone();
    if let Ok(Some(existing)) = runtime
        .backtests
        .find_run_by_idempotency_key(&idempotency_key)
    {
        if existing.request_hash == request_hash && existing.manifest_hash == manifest.manifest_hash
        {
            return ServiceResponse::created(run_value(&runtime, existing, true, false));
        }
        return ServiceResponse::conflict("backtest_request_conflict");
    }
    let now = runtime.clock.now_ms();
    let attempt_id = manifest.content.first_attempt_id.clone();
    if attempt_id != format!("{run_id}:attempt:1") {
        return ServiceResponse::bad_request("backtest_config_invalid");
    }
    if let Err(code) = runtime.backtests.put_manifest(&manifest, &context) {
        return ServiceResponse::conflict(&code);
    }
    let draft = BacktestRunRecord {
        schema: BACKTEST_RUN_SCHEMA.to_string(),
        run_id: run_id.clone(),
        manifest_hash: manifest.manifest_hash.clone(),
        request_hash,
        idempotency_key,
        state: BacktestRunState::Draft,
        sequence: 0,
        current_attempt_id: attempt_id.clone(),
        fencing_token: 0,
        result_hash: None,
        result: None,
        failure_code: None,
        events: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    if let Err(code) = runtime.backtests.create_run(&draft, &context) {
        return ServiceResponse::conflict(&code);
    }
    let attempt = attempt(&run_id, &attempt_id, 1, now);
    if let Err(code) = runtime.backtests.put_attempt(&attempt, &context) {
        return ServiceResponse::bad_request(&code);
    }
    let ready = match transition(
        service,
        &draft,
        BacktestRunCommand::ValidateReady,
        &context,
        "configuration_validated",
    ) {
        Ok(run) => run,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    let queued = match transition(
        service,
        &ready,
        BacktestRunCommand::Execute,
        &context,
        "execution_queued",
    ) {
        Ok(run) => run,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    let queue_message_id = match enqueue(service, &queued, &attempt, &context) {
        Ok(id) => id,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    let mut queued_attempt = attempt.clone();
    queued_attempt.queue_message_id = Some(queue_message_id);
    queued_attempt.updated_at_ms = now;
    if runtime
        .backtests
        .compare_and_put_attempt(&attempt, &queued_attempt, &context)
        .ok()
        != Some(true)
    {
        return ServiceResponse::conflict("backtest_worker_lease_lost");
    }
    ServiceResponse {
        status: 202,
        body: run_value(&runtime, queued, false, false),
    }
}

pub fn get(service: &TradeAssemblyService, run_id: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("backtest_not_found");
    }
    if let Err(response) = service.require_object("backtest_run", run_id) {
        return response;
    }
    match service.runtime().backtests.get_run(run_id) {
        Ok(Some(run)) => {
            let mut value = run_value(&service.runtime(), run.clone(), false, true);
            if run.state == BacktestRunState::Completed {
                match report_projection(service, run_id) {
                    Ok(report) => {
                        let strategy_id = report.run.strategy_id.clone();
                        value["report"] =
                            serde_json::to_value(report).unwrap_or_else(|_| Value::Null);
                        value["navigation"] = navigation(service, run_id, &strategy_id);
                    }
                    Err(code) => return ServiceResponse::conflict(&code),
                }
            }
            ServiceResponse::ok(value)
        }
        Ok(None) => ServiceResponse::bad_request("backtest_not_found"),
        Err(_) => ServiceResponse::internal_error("backtest read failed"),
    }
}

pub fn list(service: &TradeAssemblyService) -> ServiceResponse {
    match service.runtime().backtests.list_runs() {
        Ok(runs) => ServiceResponse::ok(
            json!({"runs": runs.into_iter().filter(|run| service.object_is_visible("backtest_run", &run.run_id)).map(|run| run_value(&service.runtime(), run, false, false)).collect::<Vec<_>>(), "noAdvice": LEGAL_BOUNDARY}),
        ),
        Err(_) => ServiceResponse::internal_error("backtest list failed"),
    }
}

pub fn workspace_runs(service: &TradeAssemblyService, strategy_id: &str) -> Vec<Value> {
    if service.require_object("strategy", strategy_id).is_err() {
        return Vec::new();
    }
    let runtime = service.runtime();
    let mut runs = runtime
        .backtests
        .list_runs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|run| {
            if !service.object_is_visible("backtest_run", &run.run_id) {
                return None;
            }
            let manifest = runtime.backtests.get_manifest(&run.run_id).ok().flatten()?;
            if manifest.content.configuration.strategy.strategy_id != strategy_id {
                return None;
            }
            let mut value = run_value(&runtime, run.clone(), false, true);
            value["id"] = value["runId"].clone();
            value["strategyId"] = json!(manifest.content.configuration.strategy.strategy_id);
            value["strategyVersionId"] =
                json!(manifest.content.configuration.strategy.strategy_version_id);
            if run.state == BacktestRunState::Completed {
                match report_projection(service, value["runId"].as_str().unwrap_or_default()) {
                    Ok(report) => {
                        value["report"] = serde_json::to_value(report).ok()?;
                        value["navigation"] = navigation(
                            service,
                            value["runId"].as_str()?,
                            value["strategyId"].as_str().unwrap_or_default(),
                        );
                    }
                    Err(code) => {
                        value["result"] = Value::Null;
                        value["reportIntegrityError"] = json!(code);
                    }
                }
            }
            Some(value)
        })
        .collect::<Vec<_>>();
    runs.sort_by_key(|run| run["createdAtMs"].as_i64().unwrap_or_default());
    runs
}

pub fn export(service: &TradeAssemblyService, run_id: &str, export_kind: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("backtest_not_found");
    }
    if let Err(response) = service.require_object("backtest_run", run_id) {
        return response;
    }
    let report = match report_projection(service, run_id) {
        Ok(report) => report,
        Err(code) if code == "backtest_not_found" => return ServiceResponse::bad_request(&code),
        Err(code) if code == "backtest_result_required" => return ServiceResponse::conflict(&code),
        Err(code) => return ServiceResponse::conflict(&code),
    };
    let manifest = match service.runtime().backtests.get_manifest(run_id) {
        Ok(Some(manifest)) => manifest,
        _ => return ServiceResponse::conflict("backtest_report_integrity_failed"),
    };
    match crate::backtest_report::export(&report, &manifest, export_kind) {
        Ok(artifact) => ServiceResponse::ok(json!({
            "backtestId": run_id,
            "exportKind": export_kind,
            "contentHash": artifact.content_hash,
            "payload": artifact,
            "navigation": navigation(service, run_id, &manifest.content.configuration.strategy.strategy_id),
            "noAdvice": LEGAL_BOUNDARY,
        })),
        Err(code) => ServiceResponse::bad_request(&code),
    }
}

pub fn report(service: &TradeAssemblyService, run_id: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("backtest_not_found");
    }
    if let Err(response) = service.require_object("backtest_run", run_id) {
        return response;
    }
    match report_projection(service, run_id) {
        Ok(report) => ServiceResponse::ok(
            json!({"report": report, "navigation": navigation(service, run_id, &report.run.strategy_id), "noAdvice": LEGAL_BOUNDARY}),
        ),
        Err(code) if code == "backtest_not_found" => ServiceResponse::bad_request(&code),
        Err(code) if code == "backtest_result_required" => ServiceResponse::conflict(&code),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

fn navigation(service: &TradeAssemblyService, run_id: &str, strategy_id: &str) -> Value {
    let Some(origin) = service.studio_base_url() else {
        return json!({"available": false, "reason": "studio_origin_unavailable"});
    };
    if strategy_id.trim().is_empty() || run_id.trim().is_empty() {
        return json!({"available": false, "reason": "backtest_identity_unavailable"});
    }
    let mut url = match url::Url::parse(origin) {
        Ok(value) => value,
        Err(_) => return json!({"available": false, "reason": "studio_origin_unavailable"}),
    };
    url.path_segments_mut()
        .expect("validated Studio origin cannot be a base")
        .extend(["app", "strategies", strategy_id, "research"]);
    url.query_pairs_mut()
        .append_pair("view", "backtest")
        .append_pair("runId", run_id);
    json!({
        "available": true,
        "strategyId": strategy_id,
        "runId": run_id,
        "href": url.to_string(),
    })
}

fn report_projection(
    service: &TradeAssemblyService,
    run_id: &str,
) -> Result<crate::backtest_report::BacktestReport, String> {
    let runtime = service.runtime();
    let run = runtime
        .backtests
        .get_run(run_id)?
        .ok_or_else(|| "backtest_not_found".to_string())?;
    let manifest = runtime
        .backtests
        .get_manifest(run_id)?
        .ok_or_else(|| "backtest_report_integrity_failed".to_string())?;
    let result = runtime
        .backtests
        .get_result(run_id)?
        .ok_or_else(|| "backtest_result_required".to_string())?;
    let snapshot = runtime
        .dataset_snapshots
        .get(&manifest.content.configuration.dataset.dataset_id)?
        .ok_or_else(|| "backtest_report_integrity_failed".to_string())?;
    let attempts = runtime.backtests.list_attempts(run_id)?;
    let events = runtime.backtests.list_events(run_id)?;
    let strategy = &manifest.content.configuration.strategy;
    let strategy_spec = runtime
        .storage
        .list_json("strategy_versions")
        .ok()
        .into_iter()
        .flatten()
        .map(|(_, value)| value)
        .find(|value| version_matches_binding(value, strategy))
        .and_then(|value| value.get("spec").cloned());
    crate::backtest_report::project_with_strategy_spec(
        &run,
        &manifest,
        &result,
        &snapshot,
        &attempts,
        &events,
        strategy_spec.as_ref(),
    )
}

pub fn cancel(service: &TradeAssemblyService, run_id: &str, body: Value) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("backtest_not_found");
    }
    if let Err(response) = service.require_object("backtest_run", run_id) {
        return response;
    }
    let runtime = service.runtime();
    let key = command_key(&body, "cancel", run_id);
    let context = context(&body, key);
    let Some(run) = runtime.backtests.get_run(run_id).ok().flatten() else {
        return ServiceResponse::bad_request("backtest_not_found");
    };
    if !matches!(
        run.state,
        BacktestRunState::Draft
            | BacktestRunState::Ready
            | BacktestRunState::Queued
            | BacktestRunState::Running
    ) {
        return ServiceResponse::conflict("backtest_not_cancelable");
    }
    let Some(attempt) = runtime
        .backtests
        .get_attempt(&run.current_attempt_id)
        .ok()
        .flatten()
    else {
        return ServiceResponse::conflict("backtest_not_cancelable");
    };
    let mut canceled_attempt = attempt.clone();
    canceled_attempt.state = BacktestAttemptState::Canceled;
    canceled_attempt.stage = "canceled".to_string();
    canceled_attempt.updated_at_ms = runtime.clock.now_ms();
    if runtime
        .backtests
        .compare_and_put_attempt(&attempt, &canceled_attempt, &context)
        .ok()
        != Some(true)
    {
        return ServiceResponse::conflict("backtest_not_cancelable");
    }
    match transition(
        service,
        &run,
        BacktestRunCommand::Cancel,
        &context,
        "cancellation_requested",
    ) {
        Ok(run) => ServiceResponse {
            status: 202,
            body: run_value(&runtime, run, false, false),
        },
        Err(_) => ServiceResponse::conflict("backtest_not_cancelable"),
    }
}

pub fn retry(service: &TradeAssemblyService, run_id: &str, body: Value) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("backtest_not_found");
    }
    if let Err(response) = service.require_object("backtest_run", run_id) {
        return response;
    }
    let runtime = service.runtime();
    let key = command_key(&body, "retry", run_id);
    let context = context(&body, key);
    let Some(run) = runtime.backtests.get_run(run_id).ok().flatten() else {
        return ServiceResponse::bad_request("backtest_not_found");
    };
    if run.state != BacktestRunState::Failed {
        return ServiceResponse::conflict("backtest_not_retryable");
    }
    let ordinal = runtime
        .backtests
        .list_attempts(run_id)
        .map(|attempts| attempts.len() as u32 + 1)
        .unwrap_or(2);
    let attempt_id = format!("{run_id}:attempt:{ordinal}");
    let now = runtime.clock.now_ms();
    let new_attempt = attempt(run_id, &attempt_id, ordinal, now);
    if runtime
        .backtests
        .put_attempt(&new_attempt, &context)
        .is_err()
    {
        return ServiceResponse::conflict("backtest_not_retryable");
    }
    let mut replacement = run.clone();
    replacement.current_attempt_id = attempt_id.clone();
    replacement.failure_code = None;
    match transition_replacement(
        service,
        &run,
        replacement,
        BacktestRunCommand::Retry,
        &context,
        "retry_queued",
    ) {
        Ok(queued) => match enqueue(service, &queued, &new_attempt, &context) {
            Ok(queue_message_id) => {
                let mut queued_attempt = new_attempt.clone();
                queued_attempt.queue_message_id = Some(queue_message_id);
                queued_attempt.updated_at_ms = now;
                if runtime
                    .backtests
                    .compare_and_put_attempt(&new_attempt, &queued_attempt, &context)
                    .ok()
                    != Some(true)
                {
                    return ServiceResponse::conflict("backtest_worker_lease_lost");
                }
                ServiceResponse {
                    status: 202,
                    body: run_value(&runtime, queued, false, false),
                }
            }
            Err(code) => ServiceResponse::bad_request(&code),
        },
        Err(_) => ServiceResponse::conflict("backtest_not_retryable"),
    }
}

pub fn process_with(
    service: &TradeAssemblyService,
    worker: &str,
    executor: &BacktestExecutor<'_>,
) -> ServiceResponse {
    let runtime = service.runtime();
    let now = runtime.clock.now_ms();
    let deliveries = match runtime.queue.claim(QUEUE, worker, now, LEASE_MS, 1) {
        Ok(deliveries) => deliveries,
        Err(_) => return ServiceResponse::internal_error("queue claim failed"),
    };
    let Some(delivery) = deliveries.into_iter().next() else {
        return ServiceResponse {
            status: 202,
            body: json!({"status":"idle", "noAdvice": LEGAL_BOUNDARY}),
        };
    };
    process_delivery(service, worker, delivery, executor)
}

fn process_delivery(
    service: &TradeAssemblyService,
    worker: &str,
    delivery: QueueDelivery,
    executor: &BacktestExecutor<'_>,
) -> ServiceResponse {
    let runtime = service.runtime();
    let now = runtime.clock.now_ms();
    let Some((run_id, manifest_hash, attempt_id)) = payload(&delivery.payload) else {
        let _ = runtime
            .queue
            .acknowledge(&delivery.message_id, delivery.fencing_token);
        return ServiceResponse::bad_request("backtest_config_invalid");
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: worker.to_string(),
            surface: "backtest-worker".to_string(),
            account_mode: "paper".to_string(),
        },
        IdempotencyKey::new(format!(
            "worker:{}:{}",
            delivery.message_id, delivery.fencing_token
        ))
        .expect("non-empty"),
    );
    let (Some(run), Some(manifest), Some(mut attempt)) = (
        runtime.backtests.get_run(&run_id).ok().flatten(),
        runtime.backtests.get_manifest(&run_id).ok().flatten(),
        runtime.backtests.get_attempt(&attempt_id).ok().flatten(),
    ) else {
        let _ = runtime
            .queue
            .acknowledge(&delivery.message_id, delivery.fencing_token);
        return ServiceResponse::bad_request("backtest_not_found");
    };
    if run.manifest_hash != manifest_hash
        || manifest.manifest_hash != manifest_hash
        || run.current_attempt_id != attempt_id
        || run.state == BacktestRunState::Canceled
    {
        let _ = runtime
            .queue
            .acknowledge(&delivery.message_id, delivery.fencing_token);
        return ServiceResponse::conflict("backtest_stale_completion");
    }
    let started = match (&run.state, &attempt.state) {
        (BacktestRunState::Queued, BacktestAttemptState::Queued) => {
            let mut replacement = run.clone();
            replacement.fencing_token = delivery.fencing_token;
            match transition_replacement(
                service,
                &run,
                replacement,
                BacktestRunCommand::Start,
                &context,
                "worker_claimed",
            ) {
                Ok(run) => run,
                Err(_) => return ServiceResponse::conflict("backtest_worker_lease_lost"),
            }
        }
        (BacktestRunState::Running, BacktestAttemptState::Running)
            if attempt.fencing_token < delivery.fencing_token =>
        {
            match refresh_running_fence(service, &run, delivery.fencing_token, &context) {
                Ok(run) => run,
                Err(_) => return ServiceResponse::conflict("backtest_worker_lease_lost"),
            }
        }
        (BacktestRunState::Running, BacktestAttemptState::Completed)
            if attempt.fencing_token < delivery.fencing_token =>
        {
            match refresh_running_fence(service, &run, delivery.fencing_token, &context) {
                Ok(run) => run,
                Err(_) => return ServiceResponse::conflict("backtest_worker_lease_lost"),
            }
        }
        _ => {
            let _ = runtime
                .queue
                .acknowledge(&delivery.message_id, delivery.fencing_token);
            return ServiceResponse::conflict("backtest_worker_lease_lost");
        }
    };
    let expected_attempt = attempt.clone();
    attempt.state = BacktestAttemptState::Running;
    attempt.stage = "executing".to_string();
    attempt.progress_bps = 100;
    attempt.lease_owner = Some(worker.to_string());
    attempt.fencing_token = delivery.fencing_token;
    attempt.updated_at_ms = now;
    if runtime
        .backtests
        .compare_and_put_attempt(&expected_attempt, &attempt, &context)
        .ok()
        != Some(true)
    {
        return ServiceResponse::conflict("backtest_worker_lease_lost");
    }
    match executor(&manifest, &attempt) {
        Ok(Some(result)) => complete(
            service,
            &started,
            &attempt,
            result,
            &context,
            WorkerLease {
                message_id: &delivery.message_id,
                fencing_token: delivery.fencing_token,
                lease_until_ms: delivery.lease_until_ms,
            },
        ),
        Ok(None) => {
            let _ = runtime.queue.retry(
                &delivery.message_id,
                delivery.fencing_token,
                now + LEASE_MS,
                "backtest_executor_result_required",
            );
            ServiceResponse {
                status: 202,
                body: run_value(&runtime, started, false, true),
            }
        }
        Err(code) => fail(
            service,
            &started,
            &attempt,
            &context,
            WorkerLease {
                message_id: &delivery.message_id,
                fencing_token: delivery.fencing_token,
                lease_until_ms: delivery.lease_until_ms,
            },
            &code,
        ),
    }
}

fn complete(
    service: &TradeAssemblyService,
    run: &BacktestRunRecord,
    attempt: &BacktestAttempt,
    result: BacktestResult,
    context: &SideEffectContext,
    lease: WorkerLease<'_>,
) -> ServiceResponse {
    let runtime = service.runtime();
    if result.content.run_id != run.run_id
        || result.content.manifest_hash != run.manifest_hash
        || result.verify().is_err()
    {
        return ServiceResponse::bad_request("backtest_result_write_conflict");
    }
    if runtime.clock.now_ms() >= lease.lease_until_ms {
        return stale_worker(service, &lease);
    }
    let mut final_attempt = attempt.clone();
    final_attempt.state = BacktestAttemptState::Completed;
    final_attempt.stage = "completed".to_string();
    final_attempt.progress_bps = 10_000;
    final_attempt.updated_at_ms = runtime.clock.now_ms();
    if runtime
        .backtests
        .compare_and_put_attempt(attempt, &final_attempt, context)
        .ok()
        != Some(true)
    {
        return stale_worker(service, &lease);
    }
    if runtime.backtests.put_result(&result, context).is_err() {
        let mut failed_attempt = final_attempt.clone();
        failed_attempt.state = BacktestAttemptState::Failed;
        failed_attempt.stage = "failed".to_string();
        failed_attempt.failure_code = Some("backtest_result_write_conflict".to_string());
        failed_attempt.updated_at_ms = runtime.clock.now_ms();
        let _ = runtime
            .backtests
            .compare_and_put_attempt(&final_attempt, &failed_attempt, context);
        let mut failed = run.clone();
        failed.failure_code = Some("backtest_result_write_conflict".to_string());
        let _ = transition_replacement(
            service,
            run,
            failed,
            BacktestRunCommand::Fail,
            context,
            "backtest_result_write_conflict",
        );
        let _ = runtime
            .queue
            .acknowledge(lease.message_id, lease.fencing_token);
        return ServiceResponse::conflict("backtest_result_write_conflict");
    }
    let mut completed = run.clone();
    completed.result_hash = Some(result.result_hash.clone());
    completed.result = Some(result.clone());
    completed.failure_code = None;
    let completed = match transition_replacement(
        service,
        run,
        completed,
        BacktestRunCommand::Complete,
        context,
        "executor_result_persisted",
    ) {
        Ok(run) => run,
        Err(_) => return ServiceResponse::conflict("backtest_stale_completion"),
    };
    let _ = runtime
        .queue
        .acknowledge(lease.message_id, lease.fencing_token);
    let mut value = run_value(&runtime, completed.clone(), false, true);
    match report_projection(service, &completed.run_id) {
        Ok(report) => {
            value["report"] = serde_json::to_value(report).unwrap_or_else(|_| Value::Null);
            ServiceResponse::ok(value)
        }
        Err(code) => ServiceResponse::conflict(&code),
    }
}

fn fail(
    service: &TradeAssemblyService,
    run: &BacktestRunRecord,
    attempt: &BacktestAttempt,
    context: &SideEffectContext,
    lease: WorkerLease<'_>,
    code: &str,
) -> ServiceResponse {
    let runtime = service.runtime();
    if runtime.clock.now_ms() >= lease.lease_until_ms {
        return stale_worker(service, &lease);
    }
    let mut failed_attempt = attempt.clone();
    failed_attempt.state = BacktestAttemptState::Failed;
    failed_attempt.stage = "failed".to_string();
    failed_attempt.failure_code = Some(code.to_string());
    failed_attempt.updated_at_ms = runtime.clock.now_ms();
    if runtime
        .backtests
        .compare_and_put_attempt(attempt, &failed_attempt, context)
        .ok()
        != Some(true)
    {
        return stale_worker(service, &lease);
    }
    let mut failed = run.clone();
    failed.failure_code = Some(code.to_string());
    let failed = match transition_replacement(
        service,
        run,
        failed,
        BacktestRunCommand::Fail,
        context,
        code,
    ) {
        Ok(run) => run,
        Err(_) => return ServiceResponse::conflict("backtest_stale_completion"),
    };
    let _ = runtime
        .queue
        .acknowledge(lease.message_id, lease.fencing_token);
    ServiceResponse::ok(run_value(&runtime, failed, false, true))
}

fn stale_worker(service: &TradeAssemblyService, lease: &WorkerLease<'_>) -> ServiceResponse {
    let runtime = service.runtime();
    let _ = runtime.queue.retry(
        lease.message_id,
        lease.fencing_token,
        runtime.clock.now_ms() + LEASE_MS,
        "backtest_worker_lease_lost",
    );
    ServiceResponse::conflict("backtest_worker_lease_lost")
}

fn transition(
    service: &TradeAssemblyService,
    run: &BacktestRunRecord,
    command: BacktestRunCommand,
    context: &SideEffectContext,
    detail: &str,
) -> Result<BacktestRunRecord, String> {
    transition_replacement(service, run, run.clone(), command, context, detail)
}

fn refresh_running_fence(
    service: &TradeAssemblyService,
    expected: &BacktestRunRecord,
    fencing_token: i64,
    context: &SideEffectContext,
) -> Result<BacktestRunRecord, String> {
    if expected.state != BacktestRunState::Running || fencing_token <= expected.fencing_token {
        return Err("backtest_worker_lease_lost".to_string());
    }
    let mut replacement = expected.clone();
    replacement.fencing_token = fencing_token;
    replacement.sequence = expected.sequence + 1;
    replacement.updated_at_ms = service.runtime().clock.now_ms();
    let event = lifecycle_event(expected, &replacement, "worker_reclaimed")?;
    replacement.events.push(event.clone());
    if !service
        .runtime()
        .backtests
        .compare_and_put_run(expected, &replacement, context)?
    {
        return Err("backtest_worker_lease_lost".to_string());
    }
    let _ = service.runtime().backtests.append_event(&event, context);
    Ok(replacement)
}

fn transition_replacement(
    service: &TradeAssemblyService,
    expected: &BacktestRunRecord,
    mut replacement: BacktestRunRecord,
    command: BacktestRunCommand,
    context: &SideEffectContext,
    detail: &str,
) -> Result<BacktestRunRecord, String> {
    let outcome = BacktestRunFsm::transition(expected.state, command)
        .map_err(|_| "backtest_run_transition_invalid".to_string())?;
    replacement.state = outcome.next_state;
    replacement.sequence = expected.sequence + 1;
    replacement.updated_at_ms = service.runtime().clock.now_ms();
    let event = lifecycle_event(expected, &replacement, detail)?;
    replacement.events.push(event.clone());
    if !service
        .runtime()
        .backtests
        .compare_and_put_run(expected, &replacement, context)?
    {
        return Err("backtest_stale_completion".to_string());
    }
    let _ = service.runtime().backtests.append_event(&event, context);
    Ok(replacement)
}
fn lifecycle_event(
    from: &BacktestRunRecord,
    to: &BacktestRunRecord,
    detail: &str,
) -> Result<BacktestLifecycleEvent, String> {
    let previous = from.events.last().map(|event| event.event_hash.clone());
    let mut event = BacktestLifecycleEvent {
        event_id: format!("{}:{}", to.run_id, to.sequence),
        run_id: to.run_id.clone(),
        sequence: to.sequence,
        event_type: format!("backtest_run.{:?}", to.state).to_lowercase(),
        from_state: from.state,
        to_state: to.state,
        attempt_id: to.current_attempt_id.clone(),
        occurred_at_ms: to.updated_at_ms,
        previous_event_hash: previous,
        event_hash: String::new(),
        detail: detail.to_string(),
    };
    event.event_hash = canonical_hash(
        &json!({"runId":event.run_id,"sequence":event.sequence,"from":event.from_state,"to":event.to_state,"attemptId":event.attempt_id,"previous":event.previous_event_hash,"detail":event.detail}),
        "backtest_event_hash_failed",
    )?;
    Ok(event)
}
fn enqueue(
    service: &TradeAssemblyService,
    run: &BacktestRunRecord,
    attempt: &BacktestAttempt,
    context: &SideEffectContext,
) -> Result<String, String> {
    let now = service.runtime().clock.now_ms();
    service.runtime().queue.enqueue(QueueRequest { queue: QUEUE.to_string(), payload: json!({"runId":run.run_id,"manifestHash":run.manifest_hash,"attemptId":attempt.attempt_id}), idempotency_key: context.idempotency_key.clone(), partition_key: Some(run.run_id.clone()), priority: 0, available_at_ms: now, retention_until_ms: now + RETENTION_MS, max_attempts: 10 })
}
fn attempt(run_id: &str, attempt_id: &str, ordinal: u32, now: i64) -> BacktestAttempt {
    BacktestAttempt {
        schema: BACKTEST_ATTEMPT_SCHEMA.to_string(),
        attempt_id: attempt_id.to_string(),
        run_id: run_id.to_string(),
        ordinal,
        state: BacktestAttemptState::Queued,
        stage: "queued".to_string(),
        progress_bps: 0,
        queue_message_id: None,
        lease_owner: None,
        fencing_token: 0,
        created_at_ms: now,
        updated_at_ms: now,
        failure_code: None,
        diagnostics: Vec::<BacktestDiagnostic>::new(),
    }
}
fn payload(value: &Value) -> Option<(String, String, String)> {
    Some((
        value.get("runId")?.as_str()?.to_string(),
        value.get("manifestHash")?.as_str()?.to_string(),
        value.get("attemptId")?.as_str()?.to_string(),
    ))
}
fn context(body: &Value, idempotency_key: IdempotencyKey) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext {
            actor: string_field(body, "actor", "local-user"),
            surface: string_field(body, "surface", "service"),
            account_mode: string_field(body, "accountMode", "paper"),
        },
        idempotency_key,
    )
}
fn command_key(body: &Value, command: &str, run_id: &str) -> IdempotencyKey {
    IdempotencyKey::new(
        body.get("idempotencyKey")
            .or_else(|| body.get("idempotency_key"))
            .and_then(Value::as_str)
            .unwrap_or(&format!("{command}:{run_id}")),
    )
    .expect("non-empty")
}
fn string_field(body: &Value, key: &str, fallback: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}
fn safe_token_field(body: &Value, key: &str, fallback: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "_-.".contains(character))
        })
        .unwrap_or(fallback)
        .to_string()
}
fn evidence_hashes(body: &Value, key: &str) -> Vec<String> {
    body.get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| canonical_hash(value, "backtest_evidence_hash_failed").ok())
                .collect()
        })
        .unwrap_or_default()
}

fn compatibility_configuration(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<BacktestConfiguration, String> {
    let strategy_id = body
        .get("strategyId")
        .or_else(|| body.get("strategy_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "backtest_strategy_version_required".to_string())?;
    super::strategy::materialize_default_version(service, strategy_id);
    let requested_version = body
        .get("strategyVersionId")
        .or_else(|| body.get("strategy_version_id"))
        .and_then(Value::as_str);
    let version = service
        .runtime()
        .storage
        .list_json("strategy_versions")?
        .into_iter()
        .map(|(_, value)| value)
        .filter(|value| {
            value["strategyId"]
                .as_str()
                .or_else(|| value["strategy_id"].as_str())
                == Some(strategy_id)
                && requested_version
                    .map(|id| value["id"].as_str() == Some(id))
                    .unwrap_or(true)
        })
        .max_by_key(|value| value["version"].as_u64().unwrap_or(0))
        .ok_or_else(|| "backtest_strategy_version_required".to_string())?;
    let dataset_id = body
        .get("datasetId")
        .or_else(|| body.get("dataset_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "backtest_dataset_unavailable".to_string())?;
    let snapshot = service
        .runtime()
        .dataset_snapshots
        .get(dataset_id)?
        .ok_or_else(|| "backtest_dataset_unavailable".to_string())?;
    snapshot.verify()?;
    let source = &snapshot.content.source;
    let source_class = serde_json::to_value(&source.source_class)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| "backtest_dataset_integrity_failed".to_string())?;
    let starting_cash = body
        .get("startingCashMicros")
        .and_then(Value::as_i64)
        .unwrap_or(100_000_000_000);
    let asset_family = snapshot
        .content
        .asset_classes
        .first()
        .and_then(|asset| serde_json::to_value(asset).ok())
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| "backtest_dataset_integrity_failed".to_string())?;
    let fill_timing = match body.get("fillTiming").and_then(Value::as_str) {
        Some("same_bar") => FillTiming::SameBar,
        _ => FillTiming::NextBar,
    };
    let fill_price_source = match body.get("fillPriceSource").and_then(Value::as_str) {
        Some("close") => PriceSource::Close,
        _ => PriceSource::Open,
    };
    let end_of_data = match body.get("endOfData").and_then(Value::as_str) {
        Some("force_close") => EndOfDataPolicy::ForceClose,
        _ => EndOfDataPolicy::MarkToMarket,
    };
    Ok(BacktestConfiguration {
        schema: BACKTEST_CONFIGURATION_SCHEMA.to_string(),
        strategy: StrategyVersionBinding {
            strategy_id: strategy_id.to_string(),
            strategy_version_id: version["id"]
                .as_str()
                .ok_or_else(|| "backtest_strategy_version_required".to_string())?
                .to_string(),
            strategy_spec_hash: version["specHash"]
                .as_str()
                .ok_or_else(|| "backtest_strategy_version_required".to_string())?
                .to_string(),
        },
        dataset: DatasetBinding {
            dataset_id: snapshot.dataset_id.clone(),
            snapshot_ref: snapshot.snapshot_ref.clone(),
            content_hash: snapshot.content_hash.clone(),
            schema: snapshot.content.schema.clone(),
            source_plugin_ref: source.plugin_ref.clone(),
            source_plugin_instance_ref: source.plugin_instance_ref.clone(),
            source_operation_id: source.operation_id.clone(),
            source_manifest_fingerprint: source.plugin_manifest_fingerprint.clone(),
            source_class,
            capability_graph_revision_id: source.capability_graph_revision_id.clone(),
            capability_graph_fingerprint: source.capability_graph_fingerprint.clone(),
            instruments: snapshot.content.instruments.clone(),
            selected_time_slice: snapshot.content.time_slice.clone(),
            granularity: snapshot.content.granularity.clone(),
            calendar: snapshot.content.calendar.clone(),
            timezone: snapshot.content.timezone.clone(),
            corporate_action_policy: snapshot
                .content
                .normalization_policy
                .price_adjustment
                .clone(),
            benchmark: body
                .get("benchmark")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        capability_graph: CapabilityGraphBinding {
            revision_id: String::new(),
            graph_fingerprint: String::new(),
        },
        capital: CapitalConfiguration {
            starting_cash_micros: starting_cash,
            reporting_currency: "USD".to_string(),
            account_model: AccountModel::Cash,
            maximum_leverage_micros: 1_000_000,
            margin_policy: "none".to_string(),
        },
        execution: ExecutionConfiguration {
            evaluation_timing: EvaluationTiming::BarClose,
            fill_timing,
            fill_price_source,
            order_type: BacktestOrderType::Market,
            time_in_force: "day".to_string(),
            fill_policy: match body.get("fillPolicy").and_then(Value::as_str) {
                Some("participation_capped_partial") => FillPolicy::ParticipationCappedPartial,
                _ => FillPolicy::Full,
            },
            reject_on_insufficient_capital: true,
            reject_on_insufficient_liquidity: true,
            stale_data_policy: MissingDataPolicy::Reject,
            missing_data_policy: MissingDataPolicy::Reject,
        },
        costs: CostConfiguration {
            fixed_fee_micros: integer(body, "fixedFeeMicros", 0),
            per_unit_fee_micros: integer(body, "perUnitFeeMicros", 0),
            notional_fee_bps: unsigned(body, "notionalFeeBps", 0),
            spread_bps: unsigned(body, "spreadBps", 0),
            slippage_bps: unsigned(body, "slippageBps", 0),
        },
        liquidity: LiquidityConfiguration {
            maximum_participation_bps: unsigned(body, "maximumParticipationBps", 10_000),
            minimum_volume_micros: integer(body, "minimumVolumeMicros", 0),
        },
        instruments: snapshot
            .content
            .instruments
            .iter()
            .map(|instrument_id| InstrumentConfiguration {
                instrument_id: instrument_id.clone(),
                instrument_family: asset_family.clone(),
                quantity_unit: "units".to_string(),
                quantity_scale: 6,
                price_scale: 6,
                contract_multiplier_micros: 1_000_000,
                settlement: "cash".to_string(),
                metadata: Default::default(),
            })
            .collect(),
        risk: BacktestRiskConfiguration {
            maximum_position_notional_micros: integer(
                body,
                "maximumPositionNotionalMicros",
                starting_cash,
            ),
            maximum_order_quantity_micros: integer(
                body,
                "maximumOrderQuantityMicros",
                1_000_000_000,
            ),
            maximum_loss_micros: starting_cash,
            maximum_open_positions: 1,
        },
        end_of_data,
        evaluator_version: "tradeassembly.strategy-kernel.v1".to_string(),
        compiler_version: "tradeassembly.strategy-compiler.v1".to_string(),
        plugin_fingerprints: std::iter::once((
            source.plugin_ref.clone(),
            source.plugin_manifest_fingerprint.clone(),
        ))
        .collect(),
        variable_overrides: Default::default(),
    })
}

fn resolve_capability_graph(
    service: &TradeAssemblyService,
    body: &Value,
    configuration: &mut BacktestConfiguration,
) -> Result<(), String> {
    let version = service
        .runtime()
        .storage
        .list_json("strategy_versions")?
        .into_iter()
        .map(|(_, value)| value)
        .find(|value| {
            value["id"].as_str() == Some(configuration.strategy.strategy_version_id.as_str())
                && value["strategyId"].as_str() == Some(configuration.strategy.strategy_id.as_str())
        })
        .ok_or_else(|| "backtest_strategy_version_required".to_string())?;
    let dataset = json!({
        "datasetId": configuration.dataset.dataset_id,
        "sourcePluginRef": configuration.dataset.source_plugin_instance_ref,
        "symbol": configuration.dataset.instruments.first().cloned().unwrap_or_default(),
        "createdAt": configuration.dataset.selected_time_slice.start,
    });
    let mut request = json!({
        "strategyId": configuration.strategy.strategy_id,
        "strategyVersionId": configuration.strategy.strategy_version_id,
        "evaluationEpoch": body.get("evaluationEpoch")
            .or_else(|| body.get("evaluation_epoch"))
            .cloned()
            .unwrap_or_else(|| json!(configuration.dataset.selected_time_slice.start)),
        "authorityContext": body.get("authorityContext")
            .or_else(|| body.get("authority_context"))
            .cloned()
            .unwrap_or(Value::Null),
    });
    if !configuration.capability_graph.revision_id.trim().is_empty() {
        request["capabilityGraphRevisionId"] = json!(configuration.capability_graph.revision_id);
    } else if let Some(revision_id) = body
        .get("capabilityGraphRevisionId")
        .or_else(|| body.get("capability_graph_revision_id"))
        .and_then(Value::as_str)
    {
        request["capabilityGraphRevisionId"] = json!(revision_id);
    }
    let revision = capability_graph::save_backtest_revision(service, &request, &version, &dataset)
        .map_err(|_| "backtest_capability_revision_stale".to_string())?;
    let revision_id = revision["revisionId"]
        .as_str()
        .ok_or_else(|| "backtest_capability_revision_stale".to_string())?;
    let fingerprint = revision["graphFingerprint"]
        .as_str()
        .ok_or_else(|| "backtest_capability_revision_stale".to_string())?;
    if (!configuration.capability_graph.revision_id.trim().is_empty()
        && configuration.capability_graph.revision_id != revision_id)
        || (!configuration
            .capability_graph
            .graph_fingerprint
            .trim()
            .is_empty()
            && configuration.capability_graph.graph_fingerprint != fingerprint)
    {
        return Err("backtest_capability_revision_stale".to_string());
    }
    configuration.capability_graph = CapabilityGraphBinding {
        revision_id: revision_id.to_string(),
        graph_fingerprint: fingerprint.to_string(),
    };
    Ok(())
}

fn integer(body: &Value, key: &str, fallback: i64) -> i64 {
    body.get(key).and_then(Value::as_i64).unwrap_or(fallback)
}

fn unsigned(body: &Value, key: &str, fallback: u32) -> u32 {
    body.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(fallback)
}
fn request_hash(body: &Value, configuration: &BacktestConfiguration) -> Result<String, String> {
    canonical_hash(
        &json!({"configuration":configuration,"client":body.get("client"),"purpose":body.get("purpose"),"authority":{"actor":body.get("actor"),"surface":body.get("surface"),"accountMode":body.get("accountMode")},"evidenceRefs":body.get("evidenceRefs")}),
        "backtest_request_hash_failed",
    )
}
fn strategy_version_matches(
    service: &TradeAssemblyService,
    configuration: &BacktestConfiguration,
) -> bool {
    service
        .runtime()
        .storage
        .list_json("strategy_versions")
        .ok()
        .into_iter()
        .flatten()
        .any(|(_, version)| version_matches_binding(&version, &configuration.strategy))
}

fn version_matches_binding(version: &Value, binding: &StrategyVersionBinding) -> bool {
    version["id"].as_str() == Some(binding.strategy_version_id.as_str())
        && version["strategyId"]
            .as_str()
            .or_else(|| version["strategy_id"].as_str())
            == Some(binding.strategy_id.as_str())
        && version["specHash"].as_str() == Some(binding.strategy_spec_hash.as_str())
        && version.get("spec").is_some_and(|spec| {
            canonical_hash(spec, "backtest_strategy_hash_failed")
                .ok()
                .as_deref()
                == Some(binding.strategy_spec_hash.as_str())
        })
}

fn exact_strategy_version(
    service: &TradeAssemblyService,
    manifest: &BacktestRunManifest,
) -> Option<Value> {
    let binding = &manifest.content.configuration.strategy;
    service
        .runtime()
        .storage
        .list_json("strategy_versions")
        .ok()?
        .into_iter()
        .map(|(_, value)| value)
        .find(|version| version_matches_binding(version, binding))
}

fn exact_strategy_version_for_configuration(
    service: &TradeAssemblyService,
    configuration: &BacktestConfiguration,
) -> Option<Value> {
    let binding = &configuration.strategy;
    service
        .runtime()
        .storage
        .list_json("strategy_versions")
        .ok()?
        .into_iter()
        .map(|(_, value)| value)
        .find(|version| version_matches_binding(version, binding))
}
fn run_value(
    runtime: &crate::ports::ServiceRuntime,
    run: BacktestRunRecord,
    duplicate: bool,
    include_details: bool,
) -> Value {
    let attempt = runtime
        .backtests
        .get_attempt(&run.current_attempt_id)
        .ok()
        .flatten();
    let mut value = json!({
        "runId": run.run_id,
        "manifestHash": run.manifest_hash,
        "status": format!("{:?}", run.state).to_lowercase(),
        "sequence": run.sequence,
        "attempt": attempt,
        "resultHash": run.result_hash,
        "failureCode": run.failure_code,
        "duplicate": duplicate,
        "createdAtMs": run.created_at_ms,
        "updatedAtMs": run.updated_at_ms,
        "noAdvice": LEGAL_BOUNDARY,
    });
    if include_details {
        value["manifest"] = runtime
            .backtests
            .get_manifest(&run.run_id)
            .ok()
            .flatten()
            .and_then(|manifest| serde_json::to_value(manifest).ok())
            .unwrap_or(Value::Null);
        value["result"] = run
            .result
            .as_ref()
            .and_then(|result| serde_json::to_value(result).ok())
            .unwrap_or(Value::Null);
        value["events"] = serde_json::to_value(&run.events).unwrap_or_else(|_| json!([]));
        value["attempts"] = runtime
            .backtests
            .list_attempts(&run.run_id)
            .ok()
            .and_then(|attempts| serde_json::to_value(attempts).ok())
            .unwrap_or_else(|| json!([]));
    }
    value
}
