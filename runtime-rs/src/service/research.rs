// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{
    backtest, journal_events, portfolio_risk_body_from, strategy, strategy_id_from,
    workspace_evidence, TradeAssemblyService,
};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};

const DATASETS_NS: &str = "research_datasets";
const JOBS_NS: &str = "research_jobs";
const SWEEPS_NS: &str = "research_sweeps";
const UNIVERSES_NS: &str = "research_universes";

pub(crate) fn create_dataset(service: &TradeAssemblyService, body: Value) -> Value {
    let strategy_id = strategy_id_from(&body);
    if service.require_object("strategy", &strategy_id).is_err() {
        return unavailable();
    }
    match requested_snapshot_id(&body) {
        Ok(Some(id)) => {
            return verified_dataset_metadata(service, &strategy_id, id).unwrap_or_else(unavailable)
        }
        Err(()) => return unavailable(),
        Ok(None) => {}
    }
    let source_plugin_ref = body
        .get("sourcePluginRef")
        .or_else(|| body.get("source_plugin_ref"))
        .and_then(Value::as_str)
        .unwrap_or("tradeassembly.local-data");
    let provider = source_plugin_ref.to_ascii_lowercase();
    let local_fixture = provider == "local-data"
        || provider == "tradeassembly.local-data"
        || provider == "demo"
        || provider == "sim";
    if !local_fixture {
        return json!({
            "status": "blocked",
            "error": {
                "code": "provider_ingestion_required",
                "message": "Provider-backed datasets must use immutable historical dataset ingestion; this legacy local fixture path cannot acquire provider data."
            },
            "nextAction": {
                "tool": "tradeassembly.plugin.list",
                "arguments": {},
                "message": "Resolve the requested provider's installed instance and historical operation, then use tradeassembly.dataset_ingestion.create with the owner's explicit data requirements. A plugin reference is not an instance reference."
            },
            "requestedSourcePluginRef": source_plugin_ref,
            "noAdvice": true
        });
    }
    let id = next_id(service, DATASETS_NS, "ds", &strategy_id);
    let mut dataset = unverified_dataset(&json!({"id":id,"datasetId":id,
        "strategyId":strategy_id,"strategy_id":strategy_id,"sourcePluginRef":source_plugin_ref}));
    // These are caller-authored declarations, never acquired or verified data.
    if let Some(symbol) = body.get("symbol").and_then(Value::as_str) {
        dataset["symbol"] = json!(symbol);
    }
    if let Some(records) = body.get("rows").and_then(Value::as_array) {
        dataset["records"] = json!(records);
    }
    if service
        .bind_inherited_object("dataset", &id, "strategy", &strategy_id)
        .is_err()
    {
        return unavailable();
    }
    persist(
        service,
        DATASETS_NS,
        &id,
        dataset.clone(),
        "research.dataset.created",
        &body,
    );
    dataset
}

pub(crate) fn create_universe(service: &TradeAssemblyService, body: Value) -> Value {
    let strategy_id = strategy_id_from(&body);
    if service.require_object("strategy", &strategy_id).is_err() {
        return unavailable();
    }
    let id = next_id(service, UNIVERSES_NS, "universe", &strategy_id);
    let symbols = body
        .get("symbols")
        .cloned()
        .filter(|value| {
            value
                .as_array()
                .is_some_and(|rows| rows.iter().all(Value::is_string))
        })
        .unwrap_or_else(|| json!([]));
    let universe = json!({
        "id": id,
        "strategy_id": strategy_id,
        "strategyId": strategy_id,
        "symbols": symbols,
        "provider_ref": body.get("providerRef").or_else(|| body.get("provider_ref")).and_then(Value::as_str),
        "status":"unverified", "verified":false, "verificationKind":"caller_declared_symbols",
        "researchEvidenceQualified":false, "executionAuthorityGranted":false, "metrics": {},
    });
    if service
        .bind_inherited_object("research_universe", &id, "strategy", &strategy_id)
        .is_err()
    {
        return unavailable();
    }
    persist(
        service,
        UNIVERSES_NS,
        &id,
        universe.clone(),
        "research.universe.created",
        &body,
    );
    universe
}

pub(crate) fn create_job(service: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let mut blocked = backtest::run_backtest(service, body);
    if blocked["status"] == "not_found" {
        return super::ServiceResponse::object_unavailable();
    }
    blocked["kind"] = json!("ResearchJob");
    super::ServiceResponse {
        status: 501,
        body: json!({"ok":false,"error":blocked["error"],"body":blocked}),
    }
}

pub(crate) fn job_status(service: &TradeAssemblyService, body: Value) -> Value {
    let requested = body
        .get("jobId")
        .or_else(|| body.get("job_id"))
        .and_then(Value::as_str)
        .unwrap_or("latest");
    let job = if requested == "latest" {
        list_jobs(service).pop()
    } else {
        if service.require_object("research_job", requested).is_err() {
            return json!({"job": {"status": "not_found"}, "artifacts": []});
        }
        service
            .runtime()
            .storage
            .get_json(JOBS_NS, requested)
            .ok()
            .flatten()
            .map(|record| backtest::historical_summary(&record, "LegacyResearchJob"))
    }
    .unwrap_or_else(|| json!({"status": "not_found"}));
    json!({"job": job, "artifacts": []})
}

pub(crate) fn list_jobs(service: &TradeAssemblyService) -> Vec<Value> {
    visible_stored(service, "research_job", JOBS_NS, &["id"])
        .iter()
        .map(|record| backtest::historical_summary(record, "LegacyResearchJob"))
        .collect()
}

pub(crate) fn list_sweeps(service: &TradeAssemblyService) -> Vec<Value> {
    visible_stored(service, "robustness_run", SWEEPS_NS, &["id", "runId"])
}

pub(crate) fn list_universes(service: &TradeAssemblyService) -> Vec<Value> {
    visible_stored(service, "research_universe", UNIVERSES_NS, &["id"])
        .iter()
        .map(unverified_universe)
        .collect()
}

pub(crate) fn workspace(service: &TradeAssemblyService, strategy_id: &str) -> Value {
    if service.require_object("strategy", strategy_id).is_err() {
        return unavailable();
    }
    let backtests = super::backtest_lifecycle::workspace_runs(service, strategy_id);
    let selected_backtest = backtests.last().cloned().unwrap_or_else(|| json!(null));
    let robustness_runs = service
        .runtime()
        .robustness
        .list_runs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|run| {
            if !service.object_is_visible("robustness_run", &run.run_id) {
                return None;
            }
            let manifest = service
                .runtime()
                .robustness
                .get_manifest(&run.run_id)
                .ok()
                .flatten()?;
            let source_manifest = service
                .runtime()
                .backtests
                .get_manifest(&manifest.content.source.source_run_id)
                .ok()
                .flatten()?;
            if source_manifest.content.configuration.strategy.strategy_id != strategy_id {
                return None;
            }
            Some(json!({
                "runId": run.run_id,
                "state": run.state,
                "resultHash": run.result_hash,
                "result": run.result,
                "failureCode": run.failure_code,
                "createdAtMs": run.created_at_ms,
                "updatedAtMs": run.updated_at_ms,
                "manifest": manifest,
            }))
        })
        .collect::<Vec<_>>();
    let snapshots = verified_dataset_list(service, strategy_id);
    let mut datasets = visible_stored(service, "dataset", DATASETS_NS, &["id", "datasetId"])
        .into_iter()
        .filter(|dataset| dataset["strategyId"].as_str() == Some(strategy_id))
        .map(|dataset| unverified_dataset(&dataset))
        .collect::<Vec<_>>();
    datasets.extend(snapshots.clone());
    let portfolio_risk_report = crate::report_envelope::portfolio_risk_surface(
        service.portfolio_risk_overlay(portfolio_risk_body_from(&json!({
            "strategyId": strategy_id,
            "scopeKind": "strategy"
        }))),
        strategy_id,
        "http://127.0.0.1:3001",
        service.db(),
    );
    json!({
        "workspaceKind": "backtesting",
        "strategy": service.strategy_value(strategy_id),
        "strategyVersions": strategy::strategy_versions(service, strategy_id),
        "metrics": selected_backtest["report"]["overview"].clone(),
        "datasetSetup": {
            "title": "Backtest dataset",
            "selectedDataset": datasets.last().cloned().unwrap_or_else(|| json!(null)),
            "actions": [
                {"action": "create_dataset", "label": "Create Dataset", "enabled": true},
                {"action": "run_backtest", "label": "Run Backtest", "enabled": true}
            ],
            "requirements": ["data_source", "time_slice", "normalization", "slippage", "fill_model", "fees", "order_policy"]
        },
        "selectedBacktest": selected_backtest,
        "robustnessEntryPoints": [
            {"kind": "RobustnessRun", "label": "Monte Carlo", "enabled": !backtests.is_empty()},
            {"kind": "RobustnessRun", "label": "Parameter sensitivity", "enabled": !backtests.is_empty()},
            {"kind": "RobustnessRun", "label": "Cost and execution sensitivity", "enabled": !backtests.is_empty()},
            {"kind": "RobustnessRun", "label": "Walk-forward, regime, stress, and capacity", "enabled": !backtests.is_empty()}
        ],
        "comparisonEntryPoints": [
            {"kind": "Comparison", "label": "Compare runs", "enabled": false, "reason": "Requires dedicated comparison slice."}
        ],
        "datasets": datasets,
        "datasetSnapshots": snapshots,
        "providerStatus": {"deprecated": true, "replacement": "datasetSetup.selectedDataset.sourcePluginRef"},
        "jobs": list_jobs(service),
        "researchJobs": list_jobs(service),
        "artifacts": backtest::list_artifacts(service),
        "researchArtifacts": backtest::list_artifacts(service),
        "robustnessRuns": robustness_runs,
        "portfolioRiskReport": portfolio_risk_report,
        "researchNotebook": super::research_notebook::latest(service)
            .filter(|notebook| {
                notebook["strategyId"].as_str() == Some(strategy_id)
                    && notebook["notebookId"]
                        .as_str()
                        .is_some_and(|id| service.object_is_visible("research_notebook", id))
            })
            .unwrap_or_else(|| json!({})),
        "selectorRuns": super::workspace::local_selector_runs(strategy_id),
        "backtests": backtests,
        "researchSweeps": list_sweeps(service),
        "researchUniverses": list_universes(service),
        "sweeps": list_sweeps(service),
        "universes": list_universes(service),
        "comparisons": [],
        "promotionReadiness": {"ready": false, "movedTo": "execution_workspace"},
        "rollup": {},
        "journal": journal_events(service),
    })
}

pub(crate) fn rollup(service: &TradeAssemblyService) -> Value {
    let recent_backtests = service
        .runtime()
        .backtests
        .list_runs()
        .unwrap_or_default()
        .into_iter()
        .filter(|run| service.object_is_visible("backtest_run", &run.run_id))
        .map(|run| {
            json!({
                "id": run.run_id,
                "status": format!("{:?}", run.state).to_lowercase(),
                "resultHash": run.result_hash,
                "createdAtMs": run.created_at_ms,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "readyForPaper": [],
        "repairQueue": [],
        "freshEvidence": [],
        "recentBacktests": recent_backtests,
        "evidence": workspace_evidence(service),
    })
}

pub(crate) fn promote_to_paper(body: &Value) -> Value {
    json!({
        "strategy_id": strategy_id_from(body),
        "ready": false,
        "status": "blocked",
        "readiness": {"ready": false, "blockedReasons": ["execution_workspace_required"]},
        "noAdvice": super::LEGAL_BOUNDARY,
    })
}

fn visible_stored(
    service: &TradeAssemblyService,
    object_type: &str,
    namespace: &str,
    id_fields: &[&str],
) -> Vec<Value> {
    let values = service
        .runtime()
        .storage
        .list_json(namespace)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    service.filter_visible_values(object_type, values, id_fields)
}

// Legacy descriptions are not immutable snapshots. In particular, historical
// slug hashes, row counts and fabricated economic/time defaults are not evidence.
fn unverified_dataset(record: &Value) -> Value {
    let mut descriptor = json!({"schemaVersion":"tradeassembly.research_dataset_descriptor.v1",
        "status":"unverified","verified":false,"verificationKind":"caller_declared_data",
        "researchEvidenceQualified":false,"executionAuthorityGranted":false,
        "nextAction":{"tool":"tradeassembly.dataset_ingestion.create","path":"/dataset-ingestions"}});
    for field in ["id", "datasetId", "strategyId", "strategy_id", "symbol"] {
        if let Some(value) = record.get(field).and_then(Value::as_str) {
            descriptor[field] = json!(value);
        }
    }
    if let Some(provider) = record
        .get("requestedSourcePluginRef")
        .or_else(|| record.get("sourcePluginRef"))
        .and_then(Value::as_str)
    {
        descriptor["requestedSourcePluginRef"] = json!(provider);
    }
    if let Some(records) = record.get("records").and_then(Value::as_array) {
        descriptor["records"] = json!(records);
    }
    descriptor
}
fn unverified_universe(record: &Value) -> Value {
    let mut descriptor = json!({"status":"unverified","verified":false,
        "verificationKind":"caller_declared_symbols","researchEvidenceQualified":false,
        "executionAuthorityGranted":false,"metrics":{}});
    for field in ["id", "strategyId", "strategy_id", "provider_ref"] {
        if let Some(value) = record.get(field).and_then(Value::as_str) {
            descriptor[field] = json!(value);
        }
    }
    descriptor["symbols"] = record
        .get("symbols")
        .filter(|v| v.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
        .cloned()
        .unwrap_or_else(|| json!([]));
    descriptor
}
fn requested_snapshot_id(body: &Value) -> Result<Option<&str>, ()> {
    let mut id = None;
    for field in ["snapshotId", "snapshot_id", "datasetId", "dataset_id"] {
        if let Some(value) = body.get(field) {
            let value = value
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
                .ok_or(())?;
            if id.is_some_and(|prior| prior != value) {
                return Err(());
            }
            id = Some(value);
        }
    }
    Ok(id)
}
fn snapshot_metadata(snapshot: &crate::historical_data::DatasetSnapshot) -> Value {
    json!({"schemaVersion":"tradeassembly.research_dataset_descriptor.v1",
        "id":snapshot.dataset_id,"datasetId":snapshot.dataset_id,"snapshotId":snapshot.dataset_id,
        "strategyId":snapshot.content.strategy_id,"strategy_id":snapshot.content.strategy_id,
        "status":"verified","verified":true,"verificationKind":"immutable_snapshot_integrity",
        "researchEvidenceQualified":false,"executionAuthorityGranted":false,
        "rows":snapshot.row_count,"rowCount":snapshot.row_count,"snapshotRef":snapshot.snapshot_ref,
        "contentHash":snapshot.content_hash,"content_hash":snapshot.content_hash,"byteLength":snapshot.byte_length,
        "firstTimestamp":snapshot.first_timestamp,"lastTimestamp":snapshot.last_timestamp,"createdAtMs":snapshot.created_at_ms,
        "sourcePluginRef":snapshot.content.source.plugin_ref,"sourceClass":snapshot.content.source.source_class,
        "timeSlice":snapshot.content.time_slice,"normalization":snapshot.content.normalization_policy,
        "calendar":snapshot.content.calendar,"timezone":snapshot.content.timezone,"instruments":snapshot.content.instruments})
}
fn verified_dataset_metadata(
    service: &TradeAssemblyService,
    strategy: &str,
    id: &str,
) -> Option<Value> {
    service.require_object("dataset_snapshot", id).ok()?;
    let snapshot = service.runtime().dataset_snapshots.get(id).ok()??;
    snapshot.verify().ok()?;
    if snapshot.dataset_id != id || snapshot.content.strategy_id != strategy {
        return None;
    }
    Some(snapshot_metadata(&snapshot))
}
fn verified_dataset_list(service: &TradeAssemblyService, strategy: &str) -> Vec<Value> {
    let mut snapshots = service
        .runtime()
        .dataset_snapshots
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            s.content.strategy_id == strategy
                && s.verify().is_ok()
                && service
                    .require_object("dataset_snapshot", &s.dataset_id)
                    .is_ok()
        })
        .collect::<Vec<_>>();
    snapshots
        .sort_by(|a, b| (a.created_at_ms, &a.dataset_id).cmp(&(b.created_at_ms, &b.dataset_id)));
    snapshots.iter().map(snapshot_metadata).collect()
}

/// Read-only projection of finite legacy metadata results. Called only after
/// ordinary invocation/object authorization and construction of the CURRENT
/// C3 envelope, before generic prepare would rewrite its duplicate flag. Never
/// dispatches a mutation or edits the retained command/authorization receipt.
pub(super) fn cached_metadata_projection(
    service: &TradeAssemblyService,
    interface: &str,
    operation: &str,
    request: &Value,
    envelope: &crate::control_plane::ControlPlaneCommandEnvelope,
) -> Result<Option<super::ServiceResponse>, super::ServiceResponse> {
    let (command, universe, field) = match (interface, operation) {
        (
            "http",
            "POST /research/datasets" | "POST /product/strategies/research/datasets/create",
        )
        | ("graphql", "CreateResearchDataset")
        | ("mcp", "tradeassembly.research.dataset.create") => {
            ("research.dataset.create", false, "createResearchDataset")
        }
        ("http", "POST /research/universes")
        | ("graphql", "CreateResearchUniverse")
        | ("mcp", "tradeassembly.research.universe.create") => {
            ("research.universe.create", true, "createResearchUniverse")
        }
        _ => return Ok(None),
    };
    // The similarly named real dataset-ingestion command is deliberately not
    // selected by this finite route/tool table.
    if envelope.command_name != command || envelope.side_effect_class == "read" {
        return Ok(None);
    }
    let request = request.get("request").unwrap_or(request);
    let strategy_id = strategy_id_from(request);
    // Some old aliases do not have a strategy route in the generic object
    // preflight. Authorize the current owner before reading any retained result.
    if service.require_object("strategy", &strategy_id).is_err() {
        return Err(super::ServiceResponse::object_unavailable());
    }
    let raw = service
        .runtime()
        .storage
        .get_json(crate::control_plane::COMMAND_NS, &envelope.command_id)
        .map_err(|_| super::ServiceResponse::internal_error("metadata_replay_unavailable"))?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let record: crate::control_plane::ControlPlaneCommandRecord = serde_json::from_value(raw)
        .map_err(|_| super::ServiceResponse::conflict("metadata_replay_invalid"))?;
    if record.envelope.payload_hash != envelope.payload_hash {
        return Err(super::ServiceResponse::conflict("idempotency_conflict"));
    }
    if record.envelope.schema_version != envelope.schema_version
        || record.envelope.command_id != envelope.command_id
        || record.envelope.command_name != envelope.command_name
        || record.envelope.command_group != envelope.command_group
        || record.envelope.source_interface != envelope.source_interface
        || record.envelope.authority != envelope.authority
        || record.envelope.target_object != envelope.target_object
        || record.envelope.side_effect_class != envelope.side_effect_class
        || record.envelope.idempotency_key != envelope.idempotency_key
        || record.envelope.idempotency_requirement != envelope.idempotency_requirement
        || record.envelope.expected_sequence != envelope.expected_sequence
        || record.envelope.evidence_refs != envelope.evidence_refs
    {
        return Err(super::ServiceResponse::conflict(
            "metadata_replay_scope_changed",
        ));
    }
    let Some(status) = record.response_status else {
        return Ok(None);
    };
    if status >= 400 {
        return Err(super::ServiceResponse::error(
            status,
            "idempotency_replay_failed",
            false,
        ));
    }
    let original = record
        .response_body
        .as_ref()
        .ok_or_else(|| super::ServiceResponse::conflict("metadata_replay_payload_unavailable"))?;
    let data = match interface {
        "graphql" => &original["data"][field],
        "mcp" => original.get("structuredContent").unwrap_or(original),
        _ => original,
    };
    let data = data.get("body").unwrap_or(data);
    let original_strategy = data
        .get("strategyId")
        .or_else(|| data.get("strategy_id"))
        .and_then(Value::as_str);
    if original_strategy.is_some_and(|id| id != strategy_id) {
        return Err(super::ServiceResponse::object_unavailable());
    }
    let mut projected = if universe {
        unverified_universe(data)
    } else {
        unverified_dataset(data)
    };
    // Old defaults are not reclassified as current caller declarations. Only
    // identity metadata remains; raw records and old default symbols stay in the
    // original protected response, not this current projection.
    projected.as_object_mut().unwrap().remove("records");
    if universe {
        projected["symbols"] = request
            .get("symbols")
            .filter(|v| v.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
            .cloned()
            .unwrap_or_else(|| json!([]));
    }
    projected["verificationKind"] = json!("unverified_historical_command_projection");
    projected["strategyId"] = json!(strategy_id);
    let original_hash =
        crate::backtest_contracts::canonical_hash(original, "metadata_replay_encoding_invalid")
            .map_err(|_| super::ServiceResponse::conflict("metadata_replay_invalid"))?;
    let mut body = super::api_result(projected);
    body["metadataProjection"] = json!({"schemaVersion":"tradeassembly.research_metadata_replay_projection.v1",
        "originalCommandId":envelope.command_id,"originalResponseSha256":original_hash,
        "originalResponseReturned":false,"storedCommandUnchanged":true,
        "projectionKind":"sanitized_historical_metadata","executionAuthorityGranted":false});
    Ok(Some(super::ServiceResponse { status, body }))
}

fn unavailable() -> Value {
    json!({
        "status": "not_found",
        "error": {"code": "object_not_available"},
        "noAdvice": true,
    })
}

fn persist(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    value: Value,
    event_type: &str,
    body: &Value,
) {
    let context = SideEffectContext::new(
        authority_context_from_body(body),
        IdempotencyKey::new(format!("{event_type}:{key}")).expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(namespace, key, value, &context)
        .expect("persist research item");
}

fn authority_context_from_body(body: &Value) -> AuthorityContext {
    let context = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    let actor = context
        .and_then(|value| value.get("actor").or_else(|| value.get("principalUser")))
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("actor")
                .and_then(|actor| actor.get("id"))
                .and_then(Value::as_str)
        })
        .unwrap_or("local-user");
    let surface = context
        .and_then(|value| value.get("surface"))
        .and_then(Value::as_str)
        .or_else(|| body.get("sourceInterface").and_then(Value::as_str))
        .unwrap_or("local");
    let account_mode = body
        .get("accountMode")
        .or_else(|| body.get("account_mode"))
        .and_then(Value::as_str)
        .unwrap_or("paper");
    AuthorityContext {
        actor: actor.to_string(),
        surface: surface.to_string(),
        account_mode: account_mode.to_string(),
    }
}

fn next_id(
    service: &TradeAssemblyService,
    namespace: &str,
    prefix: &str,
    strategy_id: &str,
) -> String {
    let count = service
        .runtime()
        .storage
        .list_json(namespace)
        .map(|items| items.len())
        .unwrap_or_default()
        + 1;
    format!("{prefix}_{}_{}", slug(strategy_id), count)
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if slug.is_empty() {
        "local".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
#[path = "research_metadata_tests.rs"]
mod metadata_tests;
