// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{robustness_lifecycle, ServiceResponse, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::backtest_contracts::canonical_hash;
use crate::domain::{BacktestRunState, RobustnessRunState};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use crate::robustness_contracts::{
    RobustnessBudget, RobustnessRunManifestContent, RobustnessSourceBinding, RobustnessStudyKind,
    ROBUSTNESS_MANIFEST_SCHEMA,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn create(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let source_run_id = match required_string(&body, "sourceRunId", "source_run_id") {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    if let Err(response) = service.require_object("backtest_run", source_run_id) {
        return response;
    }
    let study_kind = match body
        .get("studyKind")
        .or_else(|| body.get("study_kind"))
        .cloned()
        .ok_or("robustness_study_kind_required")
        .and_then(|value| {
            serde_json::from_value::<RobustnessStudyKind>(value)
                .map_err(|_| "robustness_study_kind_invalid")
        }) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    let assumptions = match body.get("assumptions") {
        Some(value) if value.is_object() => value.clone(),
        _ => return ServiceResponse::bad_request("robustness_assumptions_required"),
    };
    let budget = match body
        .get("budget")
        .cloned()
        .ok_or("robustness_budget_required")
        .and_then(|value| {
            serde_json::from_value::<RobustnessBudget>(value)
                .map_err(|_| "robustness_budget_invalid")
        }) {
        Ok(value) if value.validate() => value,
        Ok(_) => return ServiceResponse::bad_request("robustness_budget_invalid"),
        Err(code) => return ServiceResponse::bad_request(code),
    };
    let deterministic_seed = match body
        .get("deterministicSeed")
        .or_else(|| body.get("deterministic_seed"))
        .and_then(Value::as_u64)
    {
        Some(value) => value,
        None => return ServiceResponse::bad_request("robustness_seed_required"),
    };
    let idempotency_key = match required_string(&body, "idempotencyKey", "idempotency_key")
        .and_then(|value| IdempotencyKey::new(value).map_err(|_| "robustness_request_conflict"))
    {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    let source = match source_binding(service, source_run_id) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    let supporting_sources = match supporting_source_bindings(service, &body, source_run_id) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    let engine_version = safe_token(
        &body,
        "engineVersion",
        "engine_version",
        crate::robustness_engine::ROBUSTNESS_ENGINE_VERSION,
    );
    let client = safe_token(&body, "client", "client", "self");
    let purpose = safe_token(&body, "purpose", "purpose", "strategy_robustness_research");
    let authority = AuthorityContext {
        actor: safe_token(&body, "actor", "actor", "local-user"),
        surface: safe_token(&body, "surface", "surface", "service"),
        account_mode: "research".to_string(),
    };
    let request_hash = match canonical_hash(
        &json!({
            "studyKind": study_kind,
            "engineVersion": engine_version,
            "deterministicSeed": deterministic_seed,
            "source": source,
            "supportingSources": supporting_sources,
            "assumptions": assumptions,
            "budget": budget,
            "authority": authority,
            "client": client,
            "purpose": purpose,
        }),
        "robustness_request_hash_failed",
    ) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(&code),
    };
    let context = SideEffectContext::new(authority.clone(), idempotency_key.clone());
    let run_id = format!(
        "robustness_{}",
        &format!(
            "{:x}",
            Sha256::digest(format!("{}:{request_hash}", idempotency_key.as_str()).as_bytes())
        )[..16]
    );
    if service
        .bind_inherited_object("robustness_run", &run_id, "backtest_run", source_run_id)
        .is_err()
    {
        return ServiceResponse::object_unavailable();
    }
    match robustness_lifecycle::create(
        service.runtime().as_ref(),
        robustness_lifecycle::CreateRobustnessRun {
            idempotency_key: idempotency_key.as_str().to_string(),
            request_hash: request_hash.clone(),
            manifest: RobustnessRunManifestContent {
                schema: ROBUSTNESS_MANIFEST_SCHEMA.to_string(),
                run_id,
                request_hash,
                idempotency_key: idempotency_key.as_str().to_string(),
                study_kind,
                engine_version,
                deterministic_seed,
                source,
                supporting_sources,
                assumptions,
                budget,
                authority,
                client,
                purpose,
                first_attempt_id: String::new(),
            },
            context,
        },
    ) {
        Ok(run) => ServiceResponse {
            status: 202,
            body: run_value(service, run, true),
        },
        Err(code) if code == "robustness_request_conflict" => ServiceResponse::conflict(&code),
        Err(code) => ServiceResponse::bad_request(&code),
    }
}

fn supporting_source_bindings(
    service: &TradeAssemblyService,
    body: &Value,
    primary_run_id: &str,
) -> Result<Vec<RobustnessSourceBinding>, String> {
    let Some(values) = body
        .get("sourceRunIds")
        .or_else(|| body.get("source_run_ids"))
    else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .ok_or_else(|| "robustness_source_runs_invalid".to_string())?;
    let mut run_ids = std::collections::BTreeSet::new();
    run_ids.insert(primary_run_id.to_string());
    let mut sources = Vec::new();
    for value in values {
        let run_id = value
            .as_str()
            .filter(|run_id| !run_id.trim().is_empty())
            .ok_or_else(|| "robustness_source_runs_invalid".to_string())?;
        if run_ids.insert(run_id.to_string()) {
            sources.push(source_binding(service, run_id)?);
        }
    }
    sources.sort_by(|left, right| left.source_run_id.cmp(&right.source_run_id));
    Ok(sources)
}

pub fn get(service: &TradeAssemblyService, run_id: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("robustness_request_invalid");
    }
    if let Err(response) = service.require_object("robustness_run", run_id) {
        return response;
    }
    match service.runtime().robustness.get_run(run_id) {
        Ok(Some(run)) => ServiceResponse::ok(run_value(service, run, true)),
        Ok(None) => ServiceResponse::bad_request("robustness_not_found"),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn get_from_body(service: &TradeAssemblyService, body: &Value) -> ServiceResponse {
    match required_string(body, "runId", "run_id") {
        Ok(run_id) => get(service, run_id),
        Err(_) => ServiceResponse::bad_request("robustness_not_found"),
    }
}

pub fn list(service: &TradeAssemblyService) -> ServiceResponse {
    match service.runtime().robustness.list_runs() {
        Ok(runs) => ServiceResponse::ok(json!({
            "runs": runs
                .into_iter()
                .filter(|run| service.object_is_visible("robustness_run", &run.run_id))
                .map(|run| run_value(service, run, false))
                .collect::<Vec<_>>(),
            "noAdvice": LEGAL_BOUNDARY,
        })),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn report(service: &TradeAssemblyService, run_id: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("robustness_request_invalid");
    }
    if let Err(response) = service.require_object("robustness_run", run_id) {
        return response;
    }
    match completed_evidence(service, run_id) {
        Ok((manifest, result)) => {
            let evidence = match crate::robustness_projection::public_evidence(&manifest, &result) {
                Ok(value) => value,
                Err(code) => return ServiceResponse::conflict(&code),
            };
            let navigation = navigation(service, &manifest, run_id);
            ServiceResponse::ok(json!({
                "runId": run_id,
                "manifest": manifest,
                "result": result,
                "evidence": evidence,
                "navigation": navigation,
                "noAdvice": LEGAL_BOUNDARY,
            }))
        }
        Err(code) if code == "robustness_not_found" => ServiceResponse::bad_request(&code),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn replay(service: &TradeAssemblyService, run_id: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("robustness_request_invalid");
    }
    if let Err(response) = service.require_object("robustness_run", run_id) {
        return response;
    }
    let runtime = service.runtime();
    let (manifest, expected) = match completed_evidence(service, run_id) {
        Ok(value) => value,
        Err(code) if code == "robustness_not_found" => return ServiceResponse::bad_request(&code),
        Err(code) => return ServiceResponse::conflict(&code),
    };
    let sources = match std::iter::once(&manifest.content.source)
        .chain(manifest.content.supporting_sources.iter())
        .map(|binding| source_artifacts(runtime.as_ref(), binding))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(value) => value,
        Err(code) => return ServiceResponse::conflict(&code),
    };
    match crate::robustness_projection::execute_with_sources(
        &manifest,
        &sources,
        expected.created_at_ms,
    ) {
        Ok(actual) if actual == expected => {
            let evidence = match crate::robustness_projection::public_evidence(&manifest, &expected)
            {
                Ok(value) => value,
                Err(code) => return ServiceResponse::conflict(&code),
            };
            ServiceResponse::ok(json!({
                "runId": run_id,
                "status": "verified",
                "manifestHash": manifest.manifest_hash,
                "resultHash": expected.result_hash,
                "sourceHashes": std::iter::once(&manifest.content.source)
                    .chain(manifest.content.supporting_sources.iter())
                    .map(|source| json!({
                        "runId": source.source_run_id,
                        "manifestHash": source.source_manifest_hash,
                        "resultHash": source.source_result_hash,
                        "reportHash": source.source_report_hash,
                        "datasetHash": source.source_dataset_hash,
                    }))
                    .collect::<Vec<_>>(),
                "evidence": evidence,
                "navigation": navigation(service, &manifest, run_id),
                "noAdvice": LEGAL_BOUNDARY,
            }))
        }
        Ok(_) => ServiceResponse::conflict("robustness_replay_mismatch"),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn export(service: &TradeAssemblyService, run_id: &str, format: &str) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("robustness_request_invalid");
    }
    if let Err(response) = service.require_object("robustness_run", run_id) {
        return response;
    }
    let (manifest, result) = match completed_evidence(service, run_id) {
        Ok(value) => value,
        Err(code) if code == "robustness_not_found" => return ServiceResponse::bad_request(&code),
        Err(code) => return ServiceResponse::conflict(&code),
    };
    let (media_type, extension, bytes) = match format {
        "json" => {
            let evidence = match crate::robustness_projection::public_evidence(&manifest, &result) {
                Ok(value) => value,
                Err(code) => return ServiceResponse::conflict(&code),
            };
            match serde_json_canonicalizer::to_vec(&json!({
                "manifest": manifest,
                "result": result,
                "evidence": evidence,
                "noAdvice": LEGAL_BOUNDARY,
            })) {
                Ok(value) => ("application/json", "json", value),
                Err(_) => {
                    return ServiceResponse::conflict("robustness_export_serialization_failed")
                }
            }
        }
        "csv" => match robustness_csv(&manifest, &result) {
            Ok(value) => ("text/csv", "csv", value.into_bytes()),
            Err(code) => return ServiceResponse::conflict(&code),
        },
        _ => return ServiceResponse::bad_request("robustness_export_format_invalid"),
    };
    match service.runtime().exports.write(
        &format!("robustness/{run_id}/report.{extension}"),
        media_type,
        &bytes,
    ) {
        Ok(artifact) => ServiceResponse::ok(json!({
            "runId": run_id,
            "format": format,
            "artifact": artifact,
            "resultHash": result.result_hash,
            "navigation": navigation(service, &manifest, run_id),
            "noAdvice": LEGAL_BOUNDARY,
        })),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

fn navigation(
    service: &TradeAssemblyService,
    manifest: &crate::robustness_contracts::RobustnessRunManifest,
    run_id: &str,
) -> Value {
    let Some(origin) = service.studio_base_url() else {
        return json!({"available": false, "reason": "studio_origin_unavailable"});
    };
    let Some(strategy_id) = service
        .runtime()
        .backtests
        .get_manifest(&manifest.content.source.source_run_id)
        .ok()
        .flatten()
        .map(|value| value.content.configuration.strategy.strategy_id)
    else {
        return json!({"available": false, "reason": "strategy_unavailable"});
    };
    let mut url = match url::Url::parse(origin) {
        Ok(value) => value,
        Err(_) => return json!({"available": false, "reason": "studio_origin_unavailable"}),
    };
    url.path_segments_mut()
        .expect("validated Studio origin is not cannot-be-a-base")
        .extend(["app", "strategies", &strategy_id, "research"]);
    url.query_pairs_mut().append_pair("studyRunId", run_id);
    json!({
        "available": true,
        "strategyId": strategy_id,
        "studyRunId": run_id,
        "href": url.to_string(),
    })
}

pub fn cancel(service: &TradeAssemblyService, run_id: &str, body: Value) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("robustness_request_invalid");
    }
    if let Err(response) = service.require_object("robustness_run", run_id) {
        return response;
    }
    let context = command_context(&body, "cancel", run_id);
    match robustness_lifecycle::cancel(service.runtime().as_ref(), run_id, &context) {
        Ok(run) => ServiceResponse {
            status: 202,
            body: run_value(service, run, true),
        },
        Err(code) if code == "robustness_not_found" => ServiceResponse::bad_request(&code),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn retry(service: &TradeAssemblyService, run_id: &str, body: Value) -> ServiceResponse {
    if run_id.trim().is_empty() {
        return ServiceResponse::bad_request("robustness_request_invalid");
    }
    if let Err(response) = service.require_object("robustness_run", run_id) {
        return response;
    }
    let context = command_context(&body, "retry", run_id);
    match robustness_lifecycle::retry(service.runtime().as_ref(), run_id, &context) {
        Ok(run) => ServiceResponse {
            status: 202,
            body: run_value(service, run, true),
        },
        Err(code) if code == "robustness_not_found" => ServiceResponse::bad_request(&code),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn process(service: &TradeAssemblyService, worker: &str) -> ServiceResponse {
    if service.invocation_owner().is_some() {
        return ServiceResponse::object_unavailable();
    }
    let runtime = service.runtime();
    match robustness_lifecycle::process_with(runtime.as_ref(), worker, &|manifest, _| {
        let sources = std::iter::once(&manifest.content.source)
            .chain(manifest.content.supporting_sources.iter())
            .map(|binding| source_artifacts(runtime.as_ref(), binding))
            .collect::<Result<Vec<_>, _>>()?;
        crate::robustness_projection::execute_with_sources(
            manifest,
            &sources,
            runtime.clock.now_ms(),
        )
    }) {
        Ok(Some(run)) => ServiceResponse::ok(run_value(service, run, true)),
        Ok(None) => ServiceResponse {
            status: 202,
            body: json!({"state":"idle","noAdvice":LEGAL_BOUNDARY}),
        },
        Err(code)
            if code == "robustness_not_found" || code == "robustness_source_integrity_failed" =>
        {
            ServiceResponse::bad_request(&code)
        }
        Err(code) => ServiceResponse::conflict(&code),
    }
}

fn completed_evidence(
    service: &TradeAssemblyService,
    run_id: &str,
) -> Result<
    (
        crate::robustness_contracts::RobustnessRunManifest,
        crate::robustness_contracts::RobustnessResult,
    ),
    String,
> {
    let runtime = service.runtime();
    let run = runtime
        .robustness
        .get_run(run_id)?
        .ok_or_else(|| "robustness_not_found".to_string())?;
    let manifest = runtime
        .robustness
        .get_manifest(run_id)?
        .ok_or_else(|| "robustness_manifest_integrity_failed".to_string())?;
    let result = runtime
        .robustness
        .get_result(run_id)?
        .ok_or_else(|| "robustness_result_unavailable".to_string())?;
    manifest.verify()?;
    result.verify()?;
    if run.state != RobustnessRunState::Completed
        || run.manifest_hash != manifest.manifest_hash
        || run.result_hash.as_deref() != Some(result.result_hash.as_str())
        || run.result.as_ref() != Some(&result)
        || result.content.run_id != run.run_id
        || result.content.manifest_hash != manifest.manifest_hash
    {
        return Err("robustness_evidence_integrity_failed".to_string());
    }
    Ok((manifest, result))
}

fn robustness_csv(
    manifest: &crate::robustness_contracts::RobustnessRunManifest,
    result: &crate::robustness_contracts::RobustnessResult,
) -> Result<String, String> {
    let mut rows = vec![("path".to_string(), "value".to_string())];
    let evidence = serde_json::to_value(crate::robustness_projection::public_evidence(
        manifest, result,
    )?)
    .map_err(|_| "robustness_export_serialization_failed".to_string())?;
    flatten_json("$.evidence", &evidence, &mut rows);
    flatten_json("$.output", &result.content.output, &mut rows);
    Ok(rows
        .into_iter()
        .map(|(path, value)| format!("{},{}\n", csv_cell(&path), csv_cell(&value)))
        .collect())
}

fn flatten_json(path: &str, value: &Value, rows: &mut Vec<(String, String)>) {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                flatten_json(&format!("{path}.{key}"), value, rows);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                flatten_json(&format!("{path}[{index}]"), value, rows);
            }
        }
        _ => rows.push((path.to_string(), value.to_string())),
    }
}

fn csv_cell(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@']) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn source_artifacts(
    runtime: &super::ServiceRuntime,
    binding: &RobustnessSourceBinding,
) -> Result<crate::robustness_projection::RobustnessSourceArtifacts, String> {
    let run_id = &binding.source_run_id;
    let run = runtime
        .backtests
        .get_run(run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let manifest = runtime
        .backtests
        .get_manifest(run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let result = runtime
        .backtests
        .get_result(run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let dataset = runtime
        .dataset_snapshots
        .get(&binding.source_dataset_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let strategy = &manifest.content.configuration.strategy;
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
        })
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let report = crate::backtest_report::project_with_strategy_spec(
        &run,
        &manifest,
        &result,
        &dataset,
        &runtime.backtests.list_attempts(run_id)?,
        &runtime.backtests.list_events(run_id)?,
        strategy_version.get("spec"),
    )?;
    Ok(crate::robustness_projection::RobustnessSourceArtifacts {
        binding: binding.clone(),
        manifest,
        result,
        report,
        dataset,
        strategy_version,
    })
}

fn source_binding(
    service: &TradeAssemblyService,
    run_id: &str,
) -> Result<RobustnessSourceBinding, String> {
    let runtime = service.runtime();
    let run = runtime
        .backtests
        .get_run(run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let manifest = runtime
        .backtests
        .get_manifest(run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let result = runtime
        .backtests
        .get_result(run_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let dataset = runtime
        .dataset_snapshots
        .get(&manifest.content.configuration.dataset.dataset_id)?
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let strategy = &manifest.content.configuration.strategy;
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
                && value
                    .get("spec")
                    .and_then(|spec| canonical_hash(spec, "robustness_strategy_hash_failed").ok())
                    == Some(strategy.strategy_spec_hash.clone())
        })
        .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
    let report = crate::backtest_report::project_with_strategy_spec(
        &run,
        &manifest,
        &result,
        &dataset,
        &runtime.backtests.list_attempts(run_id)?,
        &runtime.backtests.list_events(run_id)?,
        strategy_version.get("spec"),
    )?;
    if run.state != BacktestRunState::Completed {
        return Err("robustness_source_integrity_failed".to_string());
    }
    Ok(RobustnessSourceBinding {
        source_run_id: run_id.to_string(),
        source_manifest_hash: manifest.manifest_hash,
        source_result_hash: result.result_hash,
        source_report_hash: report.report_hash,
        source_dataset_id: dataset.dataset_id,
        source_dataset_hash: dataset.content_hash,
    })
}

fn run_value(
    service: &TradeAssemblyService,
    run: crate::robustness_contracts::RobustnessRunRecord,
    detailed: bool,
) -> Value {
    let runtime = service.runtime();
    let manifest = runtime.robustness.get_manifest(&run.run_id).ok().flatten();
    let source_run_ids = manifest
        .as_ref()
        .map(|manifest| {
            std::iter::once(&manifest.content.source)
                .chain(manifest.content.supporting_sources.iter())
                .map(|source| source.source_run_id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let attempts = if detailed {
        runtime
            .robustness
            .list_attempts(&run.run_id)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let evidence = manifest
        .as_ref()
        .zip(run.result.as_ref())
        .map(|(manifest, result)| {
            crate::robustness_projection::public_evidence(manifest, result)
                .map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
                .unwrap_or_else(|code| {
                    json!({
                        "available": false,
                        "reason": code,
                    })
                })
        })
        .unwrap_or(Value::Null);
    json!({
        "runId": run.run_id,
        "manifestHash": run.manifest_hash,
        "requestHash": run.request_hash,
        "state": run.state,
        "sequence": run.sequence,
        "currentAttemptId": run.current_attempt_id,
        "resultHash": run.result_hash,
        "result": run.result,
        "evidence": evidence,
        "navigation": manifest
            .as_ref()
            .map(|manifest| navigation(service, manifest, &run.run_id))
            .unwrap_or_else(|| json!({"available": false, "reason": "manifest_unavailable"})),
        "failureCode": run.failure_code,
        "createdAtMs": run.created_at_ms,
        "updatedAtMs": run.updated_at_ms,
        "sourceRunIds": source_run_ids,
        "manifest": if detailed { serde_json::to_value(manifest).unwrap_or(Value::Null) } else { Value::Null },
        "attempts": attempts,
        "events": if detailed { run.events } else { Vec::new() },
        "terminal": matches!(run.state, RobustnessRunState::Completed | RobustnessRunState::Failed | RobustnessRunState::Canceled),
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn command_context(body: &Value, command: &str, run_id: &str) -> SideEffectContext {
    let key = body
        .get("idempotencyKey")
        .or_else(|| body.get("idempotency_key"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("robustness-{command}:{run_id}"));
    SideEffectContext::new(
        AuthorityContext {
            actor: safe_token(body, "actor", "actor", "local-user"),
            surface: safe_token(body, "surface", "surface", "service"),
            account_mode: "research".to_string(),
        },
        IdempotencyKey::new(key).expect("generated command key is non-empty"),
    )
}

fn required_string<'a>(body: &'a Value, camel: &str, snake: &str) -> Result<&'a str, &'static str> {
    body.get(camel)
        .or_else(|| body.get(snake))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("robustness_request_invalid")
}

fn safe_token(body: &Value, camel: &str, snake: &str, fallback: &str) -> String {
    body.get(camel)
        .or_else(|| body.get(snake))
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "_-.@:".contains(character)
                })
        })
        .unwrap_or(fallback)
        .to_string()
}
