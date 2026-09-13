// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{ServiceResponse, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use crate::research_comparison::{
    durable::{ComparisonService, CreateComparisonRequest},
    ComparisonScope,
};
use serde_json::{json, Value};

pub fn create(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    match create_inner(service, &body) {
        Ok(value) => ServiceResponse::created(value),
        Err(code) if code == "object_not_available" => ServiceResponse::object_unavailable(),
        Err(code) if code.contains("conflict") => ServiceResponse::conflict(&code),
        Err(code) => ServiceResponse::bad_request(&code),
    }
}

fn create_inner(service: &TradeAssemblyService, body: &Value) -> Result<Value, String> {
    let request = request(body)?;
    let strategy_id = request.scope.strategy_id.clone();
    service
        .require_object("strategy", &strategy_id)
        .map_err(|_| "object_not_available".to_string())?;
    for source_run_id in &request.source_run_ids {
        if !service.object_is_visible("backtest_run", source_run_id)
            && !service.object_is_visible("robustness_run", source_run_id)
        {
            return Err("object_not_available".to_string());
        }
    }
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: safe_token(body, "actor", "local-user"),
            surface: safe_token(body, "surface", "service"),
            account_mode: "research".to_string(),
        },
        IdempotencyKey::new(request.idempotency_key.clone())
            .map_err(|_| "comparison_request_invalid".to_string())?,
    );
    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    let record = comparisons.create_with_prewrite(
        request,
        runtime.clock.now_ms(),
        &context,
        |comparison_id| {
            service
                .bind_inherited_object(
                    "research_comparison",
                    comparison_id,
                    "strategy",
                    &strategy_id,
                )
                .map_err(|_| "object_not_available".to_string())
        },
    )?;
    full_record(&comparisons, &record.comparison_id)
}

pub fn get(service: &TradeAssemblyService, comparison_id: &str) -> ServiceResponse {
    if let Err(response) = service.require_object("research_comparison", comparison_id) {
        return response;
    }
    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    match full_record(&comparisons, comparison_id) {
        Ok(value) => ServiceResponse::ok(value),
        Err(code) if code == "comparison_not_found" => ServiceResponse::bad_request(&code),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn list(service: &TradeAssemblyService) -> ServiceResponse {
    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    match comparisons.list() {
        Ok(records) => ServiceResponse::ok(json!({
            "comparisons": records
                .into_iter()
                .filter(|record| service.object_is_visible("research_comparison", &record.comparison_id))
                .collect::<Vec<_>>(),
            "noAdvice": LEGAL_BOUNDARY,
        })),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn export(
    service: &TradeAssemblyService,
    comparison_id: &str,
    format: &str,
) -> ServiceResponse {
    if let Err(response) = service.require_object("research_comparison", comparison_id) {
        return response;
    }
    let runtime = service.runtime();
    let comparisons = ComparisonService::new(
        runtime.storage.as_ref(),
        runtime.backtests.as_ref(),
        runtime.dataset_snapshots.as_ref(),
    );
    match comparisons.get_export(comparison_id, format) {
        Ok(Some(value)) => ServiceResponse::ok(json!({
            "comparisonId": comparison_id,
            "export": value,
            "noAdvice": LEGAL_BOUNDARY,
        })),
        Ok(None) => ServiceResponse::bad_request("comparison_not_found"),
        Err(code) if code == "comparison_export_kind_invalid" => {
            ServiceResponse::bad_request(&code)
        }
        Err(code) => ServiceResponse::conflict(&code),
    }
}

fn full_record(comparisons: &ComparisonService<'_>, comparison_id: &str) -> Result<Value, String> {
    let record = comparisons
        .get(comparison_id)?
        .ok_or_else(|| "comparison_not_found".to_string())?;
    let manifest = comparisons
        .get_manifest(comparison_id)?
        .ok_or_else(|| "comparison_manifest_integrity_failed".to_string())?;
    let artifact = comparisons
        .get_artifact(comparison_id)?
        .ok_or_else(|| "comparison_artifact_integrity_failed".to_string())?;
    Ok(json!({
        "record": record,
        "manifest": manifest,
        "artifact": artifact,
        "noAdvice": LEGAL_BOUNDARY,
    }))
}

fn request(body: &Value) -> Result<CreateComparisonRequest, String> {
    let source_run_ids = body
        .get("sourceRunIds")
        .or_else(|| body.get("source_run_ids"))
        .cloned()
        .ok_or_else(|| "comparison_request_invalid".to_string())
        .and_then(|value| {
            serde_json::from_value(value).map_err(|_| "comparison_request_invalid".to_string())
        })?;
    let scope: ComparisonScope = body
        .get("scope")
        .cloned()
        .ok_or_else(|| "comparison_request_invalid".to_string())
        .and_then(|value| {
            serde_json::from_value(value).map_err(|_| "comparison_request_invalid".to_string())
        })?;
    let policy = body
        .get("policy")
        .cloned()
        .map(|value| {
            serde_json::from_value(value).map_err(|_| "comparison_request_invalid".to_string())
        })
        .transpose()?
        .unwrap_or_default();
    let idempotency_key = body
        .get("idempotencyKey")
        .or_else(|| body.get("idempotency_key"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "comparison_request_invalid".to_string())?
        .to_string();
    Ok(CreateComparisonRequest {
        source_run_ids,
        scope,
        policy,
        idempotency_key,
    })
}

fn safe_token(body: &Value, key: &str, fallback: &str) -> String {
    body.get(key)
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
