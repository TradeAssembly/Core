// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{strategy_id_from, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::backtest_contracts::canonical_hash;
use crate::domain::{BacktestRunState, RobustnessRunState};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use crate::research_notebook::{
    export_research_notebook, research_notebook_hash, ResearchNotebookAssumption,
    ResearchNotebookBlockKind, ResearchNotebookChartRef, ResearchNotebookContract,
    ResearchNotebookExplainableBlock, ResearchNotebookExportBundle, ResearchNotebookInputRef,
    ResearchNotebookPromptRef, ResearchNotebookRedactionStatus, ResearchNotebookReplayRef,
    ResearchNotebookReportRef, ResearchNotebookSourceProvenance, ResearchNotebookTableRef,
    ResearchNotebookToolCall, ResearchNotebookToolKind, ResearchNotebookWarning,
    ResearchNotebookWarningCode, ResearchNotebookWarningSeverity, RESEARCH_NOTEBOOK_NOTICE,
    RESEARCH_NOTEBOOK_SCHEMA_VERSION,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const NOTEBOOK_NS: &str = "research_notebooks";
const NOTEBOOK_EXPORT_NS: &str = "research_notebook_exports";
const DEFAULT_NOW_UNIX_MS: u64 = 1_785_000_000_000;

#[derive(Clone, Debug)]
struct ArtifactCandidate {
    artifact_id: String,
    artifact_kind: String,
    ref_uri: String,
    content_hash: String,
    title: String,
    source_ref: String,
    warning_count: usize,
    redaction_status: ResearchNotebookRedactionStatus,
    journal_refs: Vec<String>,
    replay_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ArtifactSelector {
    kind: String,
    id: String,
}

pub(crate) fn operation(service: &TradeAssemblyService, operation: &str, request: Value) -> Value {
    match operation {
        "compose" | "create" | "attach" => compose(service, request),
        "list" => list(service, request),
        "inspect" => inspect(service, request),
        "export" => export(service, request),
        "replay" => replay(service, request),
        _ => notebook_error(
            &strategy_id_from(&request),
            "research_notebook_operation_invalid",
        ),
    }
}

pub(crate) fn compose(service: &TradeAssemblyService, request: Value) -> Value {
    let strategy_id = strategy_id_from(&request);
    if service.require_object("strategy", &strategy_id).is_err() {
        return notebook_not_found(&strategy_id);
    }
    let selectors = match selectors(&request) {
        Ok(selectors) => selectors,
        Err(code) => return notebook_error(&strategy_id, &code),
    };
    let resolved = match selectors
        .iter()
        .map(|selector| resolve_selector(service, &strategy_id, selector))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(resolved) => resolved,
        Err(code) => return notebook_error(&strategy_id, &code),
    };
    let mut request = request;
    let object = request
        .as_object_mut()
        .expect("research notebook request must be an object");
    // The Core owns all evidence-bearing fields. Callers select durable records;
    // they cannot inject an artifact URI, hash, journal ref, or replay ref.
    object.insert("artifactRefs".to_string(), Value::Array(resolved));
    object.insert(
        "artifactSelectors".to_string(),
        Value::Array(
            selectors
                .iter()
                .map(|selector| json!({"kind": selector.kind, "id": selector.id}))
                .collect(),
        ),
    );
    object.remove("artifact_selectors");
    object.remove("prompts");
    object.remove("userDecisionRef");
    object.remove("user_decision_ref");
    compose_resolved(service, request)
}

pub(crate) fn list(service: &TradeAssemblyService, request: Value) -> Value {
    let strategy_id = strategy_id_from(&request);
    match service.runtime().storage.list_json(NOTEBOOK_NS) {
        Ok(values) => {
            let mut notebooks = values
                .into_iter()
                .map(|(_, value)| value)
                .filter(|value| value["strategyId"].as_str() == Some(strategy_id.as_str()))
                .filter(|value| {
                    value["notebookId"]
                        .as_str()
                        .is_some_and(|id| service.object_is_visible("research_notebook", id))
                })
                .collect::<Vec<_>>();
            if notebooks
                .iter()
                .any(|notebook| !stored_notebook_valid(notebook))
            {
                return notebook_error(&strategy_id, "research_notebook_integrity_failed");
            }
            notebooks.sort_by(|left, right| {
                left["notebookId"]
                    .as_str()
                    .cmp(&right["notebookId"].as_str())
            });
            json!({"ok": true, "strategyId": strategy_id, "notebooks": notebooks, "noAdvice": LEGAL_BOUNDARY})
        }
        Err(code) => notebook_error(&strategy_id, &code),
    }
}

pub(crate) fn inspect(service: &TradeAssemblyService, request: Value) -> Value {
    let strategy_id = strategy_id_from(&request);
    let Some(notebook_id) = string_field(&request, &["notebookId", "notebook_id"]) else {
        return notebook_error(&strategy_id, "research_notebook_id_required");
    };
    if service
        .require_object("research_notebook", &notebook_id)
        .is_err()
    {
        return notebook_not_found(&strategy_id);
    }
    let Some(notebook) = service
        .runtime()
        .storage
        .get_json(NOTEBOOK_NS, &notebook_id)
        .ok()
        .flatten()
    else {
        return notebook_error(&strategy_id, "research_notebook_not_found");
    };
    if notebook["strategyId"].as_str() != Some(strategy_id.as_str()) {
        return notebook_error(&strategy_id, "research_notebook_strategy_mismatch");
    }
    if !stored_notebook_valid(&notebook) {
        return notebook_error(&strategy_id, "research_notebook_integrity_failed");
    }
    json!({"ok": true, "strategyId": strategy_id, "notebook": notebook, "noAdvice": LEGAL_BOUNDARY})
}

pub(crate) fn export(service: &TradeAssemblyService, request: Value) -> Value {
    let inspected = inspect(service, request.clone());
    if inspected["ok"] != Value::Bool(true) {
        return inspected;
    }
    let strategy_id = strategy_id_from(&request);
    let notebook_id = inspected["notebook"]["notebookId"]
        .as_str()
        .unwrap_or_default();
    match service
        .runtime()
        .storage
        .get_json(NOTEBOOK_EXPORT_NS, notebook_id)
    {
        Ok(Some(bundle)) => {
            let expected = inspected["notebook"]
                .get("notebook")
                .cloned()
                .and_then(|value| serde_json::from_value::<ResearchNotebookContract>(value).ok())
                .and_then(|contract| export_research_notebook(&contract).ok())
                .and_then(|expected| serde_json::to_value(expected).ok());
            if expected.as_ref() != Some(&bundle) {
                return notebook_error(&strategy_id, "research_notebook_export_integrity_failed");
            }
            json!({
                "ok": true,
                "strategyId": strategy_id,
                "notebookId": notebook_id,
                "export": bundle,
                "noAdvice": LEGAL_BOUNDARY,
            })
        }
        Ok(None) => notebook_error(&strategy_id, "research_notebook_export_not_found"),
        Err(code) => notebook_error(&strategy_id, &code),
    }
}

pub(crate) fn replay(service: &TradeAssemblyService, request: Value) -> Value {
    let inspected = inspect(service, request.clone());
    if inspected["ok"] != Value::Bool(true) {
        return inspected;
    }
    let strategy_id = strategy_id_from(&request);
    let notebook = &inspected["notebook"];
    let selectors = notebook
        .get("artifactSelectors")
        .cloned()
        .and_then(|value| selectors(&json!({"artifactSelectors": value})).ok());
    let Some(selectors) = selectors else {
        return notebook_error(&strategy_id, "research_notebook_integrity_failed");
    };
    let resolved = match selectors
        .iter()
        .map(|selector| resolve_selector(service, &strategy_id, selector))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(resolved) => resolved,
        Err(code) => return notebook_error(&strategy_id, &code),
    };
    let current_refs = resolved
        .iter()
        .map(canonical_artifact_ref)
        .collect::<Vec<_>>();
    let stored_refs = notebook["artifactRefs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if current_refs != stored_refs {
        return notebook_error(&strategy_id, "research_notebook_replay_mismatch");
    }
    json!({
        "ok": true,
        "strategyId": strategy_id,
        "notebookId": notebook["notebookId"],
        "status": "verified",
        "notebookHash": notebook["notebookHash"],
        "replayRefs": notebook["replayRefs"],
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn compose_resolved(service: &TradeAssemblyService, request: Value) -> Value {
    let strategy_id = strategy_id_from(&request);
    let created_at_unix_ms = DEFAULT_NOW_UNIX_MS;
    let studio_base_url = string_field(&request, &["studioBaseUrl", "studio_base_url"])
        .unwrap_or_else(|| "http://127.0.0.1:3001".to_string());
    let mut warnings = Vec::new();
    let artifacts = artifact_candidates(&request, &mut warnings);
    let title = string_field(&request, &["title"])
        .unwrap_or_else(|| format!("Research notebook for {strategy_id}"));
    let assumptions = assumptions(&request);
    let notebook_id = notebook_id(&strategy_id, &title, &assumptions, &artifacts);
    let run_id = format!("research_run_{}", &notebook_id[9..]);
    let mut notebook = ResearchNotebookContract {
        schema_version: RESEARCH_NOTEBOOK_SCHEMA_VERSION.to_string(),
        notebook_id: notebook_id.clone(),
        strategy_id: strategy_id.clone(),
        run_id: run_id.clone(),
        title,
        created_at_unix_ms,
        prompts: prompt_refs(&request, &run_id, created_at_unix_ms, &mut warnings),
        tool_calls: tool_calls(&artifacts, created_at_unix_ms),
        inputs: input_refs(&artifacts, created_at_unix_ms),
        data_windows: Vec::new(),
        assumptions,
        explainable_blocks: explainable_blocks(&artifacts, &request),
        report_refs: report_refs(&artifacts),
        chart_refs: chart_refs(&artifacts),
        table_refs: table_refs(&artifacts),
        warnings,
        provenance: provenance(
            "tradeassembly-runtime",
            &format!("journal://research/{run_id}/notebook"),
            created_at_unix_ms,
        ),
        replay_refs: replay_refs(&notebook_id, &run_id),
        redaction_status: if artifacts
            .iter()
            .any(|artifact| artifact.redaction_status == ResearchNotebookRedactionStatus::Redacted)
        {
            ResearchNotebookRedactionStatus::Redacted
        } else {
            ResearchNotebookRedactionStatus::Clean
        },
        metadata: metadata(&artifacts),
        no_advice_notice: RESEARCH_NOTEBOOK_NOTICE.to_string(),
    };
    if notebook.explainable_blocks.is_empty() {
        notebook
            .explainable_blocks
            .push(ResearchNotebookExplainableBlock {
                block_id: "block_limitations".to_string(),
                kind: ResearchNotebookBlockKind::Limitation,
                title: "No referenced artifacts".to_string(),
                summary: "No research artifacts were available to compose into this notebook."
                    .to_string(),
                source_refs: vec![format!("tradeassembly://research/{run_id}")],
                calculation_ref: None,
                model_ref: None,
                user_decision_ref: None,
            });
    }
    let validation = notebook.validate();
    let exported = if validation.ok {
        export_research_notebook(&notebook).ok()
    } else {
        None
    };
    let notebook_hash = research_notebook_hash(&notebook);
    let export_refs = exported
        .as_ref()
        .map(|export| export_refs(&notebook_id, export))
        .unwrap_or_else(|| json!({}));
    let mut result = json!({
        "schemaVersion": "tradeassembly.research_notebook.service.v1",
        "kind": "research_notebook",
        "ok": validation.ok,
        "strategyId": strategy_id,
        "runId": run_id,
        "notebookId": notebook_id,
        "notebookHash": notebook_hash,
        "summary": {
            "artifactRefCount": artifacts.len(),
            "reportRefCount": notebook.report_refs.len(),
            "chartRefCount": notebook.chart_refs.len(),
            "tableRefCount": notebook.table_refs.len(),
            "warningCount": validation.warnings.len(),
            "exported": exported.is_some(),
        },
        "notebook": notebook,
        "artifactSelectors": request.get("artifactSelectors").or_else(|| request.get("artifact_selectors")).cloned().unwrap_or_else(|| json!([])),
        "warnings": validation.warnings,
        "artifactRefs": artifacts.iter().map(artifact_ref_json).collect::<Vec<_>>(),
        "exportRefs": export_refs,
        "replayRefs": {
            "commands": [
                format!("tradeassembly research-notebook compose --notebook-id {} --db {}", result_placeholder(), service.db()),
                format!("tradeassembly research-notebook replay --notebook-id {} --db {}", result_placeholder(), service.db())
            ],
            "refs": replay_refs(result_placeholder(), result_placeholder()).iter().map(|replay| json!({"replayRef": replay.replay_ref, "deterministic": replay.deterministic})).collect::<Vec<_>>()
        },
        "deepLinks": {
            "strategy": {"href": format!("{}/app/strategies/{}", studio_base_url.trim_end_matches('/'), result_placeholder())},
            "research": {"href": format!("{}/app/strategies/{}/research?notebook={}", studio_base_url.trim_end_matches('/'), result_placeholder(), result_placeholder())}
        },
        "sideEffects": {
            "strategyLogicChanged": false,
            "credentialsTouched": false,
            "brokerStateChanged": false,
            "activationChanged": false,
        },
        "agentSummary": {
            "scope": "Local research notebook composition",
            "results": {
                "artifactRefs": artifacts.len(),
                "warnings": result_count_placeholder(),
                "exported": exported.is_some()
            },
            "studioDeepLinks": {
                "research": format!("{}/app/strategies/{}/research?notebook={}", studio_base_url.trim_end_matches('/'), result_placeholder(), result_placeholder())
            }
        },
        "noAdvice": LEGAL_BOUNDARY,
        "noAdviceNotice": RESEARCH_NOTEBOOK_NOTICE,
    });
    let notebook_id = result["notebookId"]
        .as_str()
        .unwrap_or("notebook")
        .to_string();
    let strategy_id = result["strategyId"]
        .as_str()
        .unwrap_or("strategy")
        .to_string();
    let run_id = result["runId"].as_str().unwrap_or("run").to_string();
    result["replayRefs"] = json!({
        "commands": [
            format!("tradeassembly research-notebook compose --notebook-id {notebook_id} --db {}", service.db()),
            format!("tradeassembly research-notebook replay --notebook-id {notebook_id} --db {}", service.db())
        ],
        "refs": replay_refs(&notebook_id, &run_id).iter().map(|replay| json!({"replayRef": replay.replay_ref, "deterministic": replay.deterministic})).collect::<Vec<_>>()
    });
    result["deepLinks"] = json!({
        "strategy": {"href": format!("{}/app/strategies/{strategy_id}", studio_base_url.trim_end_matches('/'))},
        "research": {"href": format!("{}/app/strategies/{strategy_id}/research?notebook={notebook_id}", studio_base_url.trim_end_matches('/'))}
    });
    result["agentSummary"]["results"]["warnings"] = result["summary"]["warningCount"].clone();
    result["agentSummary"]["studioDeepLinks"]["research"] = json!(format!(
        "{}/app/strategies/{strategy_id}/research?notebook={notebook_id}",
        studio_base_url.trim_end_matches('/')
    ));
    result["agentDisplaySummary"] = json!(format!(
        "Captured {} local artifact refs into notebook {notebook_id}; linked {} reports, {} charts, {} tables, redacted sensitive payload surfaces, recorded {} warnings, and provided Studio, export, and replay refs.",
        result["summary"]["artifactRefCount"].as_u64().unwrap_or(0),
        result["summary"]["reportRefCount"].as_u64().unwrap_or(0),
        result["summary"]["chartRefCount"].as_u64().unwrap_or(0),
        result["summary"]["tableRefCount"].as_u64().unwrap_or(0),
        result["summary"]["warningCount"].as_u64().unwrap_or(0),
    ));
    if service
        .bind_inherited_object("research_notebook", &notebook_id, "strategy", &strategy_id)
        .is_err()
    {
        return notebook_not_found(&strategy_id);
    }
    persist_notebook(service, result, exported)
}

pub(crate) fn latest(service: &TradeAssemblyService) -> Option<Value> {
    service
        .runtime()
        .storage
        .list_json(NOTEBOOK_NS)
        .ok()
        .and_then(|values| {
            values.into_iter().map(|(_, value)| value).rfind(|value| {
                value["notebookId"]
                    .as_str()
                    .is_some_and(|id| service.object_is_visible("research_notebook", id))
            })
        })
}

fn persist_notebook(
    service: &TradeAssemblyService,
    mut result: Value,
    exported: Option<ResearchNotebookExportBundle>,
) -> Value {
    let notebook_id = result["notebookId"]
        .as_str()
        .unwrap_or("research_notebook_latest")
        .to_string();
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("research_notebook.compose:{notebook_id}"))
            .expect("valid research notebook idempotency key"),
    );
    result["recordHash"] = json!(stored_record_hash(&result));
    service
        .runtime()
        .storage
        .put_json(NOTEBOOK_NS, &notebook_id, result.clone(), &context)
        .expect("persist research notebook");
    if let Some(export) = exported {
        service
            .runtime()
            .storage
            .put_json(
                NOTEBOOK_EXPORT_NS,
                &notebook_id,
                serde_json::to_value(export).expect("research notebook export json"),
                &context,
            )
            .expect("persist research notebook export");
    }
    let journal_id = service
        .runtime()
        .record_side_effect("research_notebook.composed", result.clone(), &context)
        .expect("record research notebook journal evidence");
    result["journalEvidence"] = json!({
        "eventType": "research_notebook.composed",
        "journalId": journal_id,
        "storageNamespace": NOTEBOOK_NS,
        "exportStorageNamespace": NOTEBOOK_EXPORT_NS,
        "notebookId": notebook_id,
    });
    result["recordHash"] = json!(stored_record_hash(&result));
    service
        .runtime()
        .storage
        .put_json(NOTEBOOK_NS, &notebook_id, result.clone(), &context)
        .expect("persist research notebook with journal evidence");
    result
}

fn selectors(request: &Value) -> Result<Vec<ArtifactSelector>, String> {
    let values = request
        .get("artifactSelectors")
        .or_else(|| request.get("artifact_selectors"))
        .and_then(Value::as_array)
        .ok_or_else(|| "research_notebook_artifact_selectors_required".to_string())?;
    if values.is_empty() {
        return Err("research_notebook_artifact_selectors_required".to_string());
    }
    let mut selectors = values
        .iter()
        .map(|value| {
            let kind = string_field(value, &["kind"])
                .filter(|value| {
                    matches!(
                        value.as_str(),
                        "backtest" | "robustness" | "comparison" | "derivatives"
                    )
                })
                .ok_or_else(|| "research_notebook_selector_kind_invalid".to_string())?;
            let id = string_field(value, &["id"])
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "research_notebook_selector_id_invalid".to_string())?;
            Ok(ArtifactSelector { kind, id })
        })
        .collect::<Result<Vec<_>, String>>()?;
    selectors.sort();
    if selectors.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("research_notebook_duplicate_selector".to_string());
    }
    Ok(selectors)
}

fn resolve_selector(
    service: &TradeAssemblyService,
    strategy_id: &str,
    selector: &ArtifactSelector,
) -> Result<Value, String> {
    match selector.kind.as_str() {
        "backtest" => resolve_backtest(service, strategy_id, &selector.id),
        "robustness" => resolve_robustness(service, strategy_id, &selector.id),
        "comparison" => resolve_comparison(service, strategy_id, &selector.id),
        "derivatives" => resolve_derivatives(service, strategy_id, &selector.id),
        _ => Err("research_notebook_selector_kind_invalid".to_string()),
    }
}

fn resolve_backtest(
    service: &TradeAssemblyService,
    strategy_id: &str,
    run_id: &str,
) -> Result<Value, String> {
    service
        .require_object("backtest_run", run_id)
        .map_err(|_| "research_notebook_artifact_not_found".to_string())?;
    let runtime = service.runtime();
    let run = runtime
        .backtests
        .get_run(run_id)?
        .ok_or_else(|| "research_notebook_artifact_not_found".to_string())?;
    let manifest = runtime
        .backtests
        .get_manifest(run_id)?
        .ok_or_else(|| "research_notebook_artifact_integrity_failed".to_string())?;
    let result = runtime
        .backtests
        .get_result(run_id)?
        .ok_or_else(|| "research_notebook_artifact_incomplete".to_string())?;
    manifest.verify()?;
    result.verify()?;
    if run.state != BacktestRunState::Completed
        || run.manifest_hash != manifest.manifest_hash
        || run.result_hash.as_deref() != Some(result.result_hash.as_str())
        || manifest.content.configuration.strategy.strategy_id != strategy_id
    {
        return Err("research_notebook_artifact_strategy_or_integrity_failed".to_string());
    }
    Ok(json!({
        "artifactId": run_id,
        "artifactKind": "backtest",
        "refUri": format!("tradeassembly://backtests/{run_id}/report"),
        "contentHash": result.result_hash,
        "title": format!("Backtest {run_id}"),
        "sourceRef": format!("tradeassembly://backtests/{run_id}/replay"),
        "journalRefs": [format!("journal://backtest/{run_id}")],
        "replayRefs": [format!("tradeassembly://backtests/{run_id}/replay")],
    }))
}

fn resolve_robustness(
    service: &TradeAssemblyService,
    strategy_id: &str,
    run_id: &str,
) -> Result<Value, String> {
    service
        .require_object("robustness_run", run_id)
        .map_err(|_| "research_notebook_artifact_not_found".to_string())?;
    let runtime = service.runtime();
    let run = runtime
        .robustness
        .get_run(run_id)?
        .ok_or_else(|| "research_notebook_artifact_not_found".to_string())?;
    let manifest = runtime
        .robustness
        .get_manifest(run_id)?
        .ok_or_else(|| "research_notebook_artifact_integrity_failed".to_string())?;
    let result = runtime
        .robustness
        .get_result(run_id)?
        .ok_or_else(|| "research_notebook_artifact_incomplete".to_string())?;
    manifest.verify()?;
    result.verify()?;
    let replay = super::robustness::replay(service, run_id);
    if replay.status != 200 || replay.body["status"].as_str() != Some("verified") {
        return Err("research_notebook_artifact_integrity_failed".to_string());
    }
    let source = &manifest.content.source;
    let source_manifest = runtime
        .backtests
        .get_manifest(&source.source_run_id)?
        .ok_or_else(|| "research_notebook_artifact_integrity_failed".to_string())?;
    if run.state != RobustnessRunState::Completed
        || run.manifest_hash != manifest.manifest_hash
        || run.result_hash.as_deref() != Some(result.result_hash.as_str())
        || source_manifest.content.configuration.strategy.strategy_id != strategy_id
    {
        return Err("research_notebook_artifact_strategy_or_integrity_failed".to_string());
    }
    Ok(json!({
        "artifactId": run_id,
        "artifactKind": "robustness",
        "refUri": result.result_ref,
        "contentHash": result.result_hash,
        "title": format!("Robustness {run_id}"),
        "sourceRef": format!("tradeassembly://robustness-runs/{run_id}/replay"),
        "journalRefs": [format!("journal://robustness/{run_id}")],
        "replayRefs": [format!("tradeassembly://robustness-runs/{run_id}/replay")],
    }))
}

fn resolve_comparison(
    service: &TradeAssemblyService,
    strategy_id: &str,
    comparison_id: &str,
) -> Result<Value, String> {
    let response = super::comparisons::get(service, comparison_id);
    if response.status == 409 {
        return Err("research_notebook_artifact_integrity_failed".to_string());
    }
    if response.status != 200 {
        return Err("research_notebook_artifact_not_found".to_string());
    }
    let manifest = &response.body["manifest"];
    let artifact = &response.body["artifact"];
    let sources = manifest["content"]["sources"]
        .as_array()
        .ok_or_else(|| "research_notebook_artifact_integrity_failed".to_string())?;
    if sources.is_empty()
        || sources
            .iter()
            .any(|source| source["strategyId"].as_str() != Some(strategy_id))
    {
        return Err("research_notebook_artifact_strategy_or_integrity_failed".to_string());
    }
    let content_hash = artifact["artifactHash"]
        .as_str()
        .ok_or_else(|| "research_notebook_artifact_integrity_failed".to_string())?;
    let ref_uri = artifact["artifactRef"]
        .as_str()
        .ok_or_else(|| "research_notebook_artifact_integrity_failed".to_string())?;
    Ok(json!({
        "artifactId": comparison_id,
        "artifactKind": "comparison",
        "refUri": ref_uri,
        "contentHash": content_hash,
        "title": format!("Comparison {comparison_id}"),
        "sourceRef": format!("tradeassembly://research-comparisons/{comparison_id}"),
        "journalRefs": [format!("journal://comparison/{comparison_id}")],
        "replayRefs": [format!("tradeassembly://research-comparisons/{comparison_id}")],
    }))
}

fn resolve_derivatives(
    service: &TradeAssemblyService,
    strategy_id: &str,
    analysis_id: &str,
) -> Result<Value, String> {
    let response = super::derivatives::get(service, analysis_id);
    if response.status == 409 {
        return Err("research_notebook_artifact_integrity_failed".to_string());
    }
    if response.status != 200 {
        return Err("research_notebook_artifact_not_found".to_string());
    }
    super::derivatives::verify_current_source(service, &response.body)
        .map_err(|_| "research_notebook_artifact_integrity_failed".to_string())?;
    let analysis = &response.body["analysis"];
    if analysis["sourceBindings"]["strategyId"].as_str() != Some(strategy_id) {
        return Err("research_notebook_artifact_strategy_or_integrity_failed".to_string());
    }
    let content_hash = analysis["outputHash"]
        .as_str()
        .ok_or_else(|| "research_notebook_artifact_integrity_failed".to_string())?;
    Ok(json!({
        "artifactId": analysis_id,
        "artifactKind": "derivatives",
        "refUri": format!("tradeassembly://derivatives-analyses/{analysis_id}"),
        "contentHash": content_hash,
        "title": format!("Derivatives analysis {analysis_id}"),
        "sourceRef": format!("tradeassembly://derivatives-analyses/{analysis_id}/replay"),
        "journalRefs": [format!("journal://derivatives/{analysis_id}")],
        "replayRefs": [format!("tradeassembly://derivatives-analyses/{analysis_id}/replay")],
    }))
}

fn canonical_artifact_ref(value: &Value) -> Value {
    json!({
        "artifactId": value["artifactId"],
        "artifactKind": value["artifactKind"],
        "refUri": value["refUri"],
        "contentHash": value["contentHash"],
        "title": value["title"],
        "redactionStatus": "clean",
        "journalRefs": value["journalRefs"],
        "replayRefs": value["replayRefs"],
    })
}

fn notebook_error(strategy_id: &str, code: &str) -> Value {
    json!({"ok": false, "strategyId": strategy_id, "code": code, "noAdvice": LEGAL_BOUNDARY})
}

fn notebook_not_found(strategy_id: &str) -> Value {
    notebook_error(strategy_id, "research_notebook_not_found")
}

fn stored_notebook_valid(notebook: &Value) -> bool {
    let Some(contract) = notebook.get("notebook").cloned() else {
        return false;
    };
    let Ok(contract) = serde_json::from_value::<ResearchNotebookContract>(contract) else {
        return false;
    };
    contract.validate().ok
        && notebook["notebookHash"].as_str() == Some(research_notebook_hash(&contract).as_str())
        && notebook["recordHash"].as_str() == Some(stored_record_hash(notebook).as_str())
}

fn stored_record_hash(notebook: &Value) -> String {
    let mut canonical = notebook.clone();
    if let Some(object) = canonical.as_object_mut() {
        object.remove("recordHash");
    }
    canonical_hash(&canonical, "research notebook record")
        .expect("research notebook record must be canonical JSON")
}

fn artifact_candidates(
    request: &Value,
    warnings: &mut Vec<ResearchNotebookWarning>,
) -> Vec<ArtifactCandidate> {
    let mut artifacts = Vec::new();
    for value in artifact_values(request) {
        let artifact_kind = string_field(&value, &["artifactKind", "artifact_kind", "kind"])
            .unwrap_or_else(|| "report".to_string());
        let ref_uri = string_field(
            &value,
            &["refUri", "ref_uri", "uri", "reportRef", "artifactRef"],
        );
        let content_hash = string_field(&value, &["contentHash", "content_hash", "hash"]);
        let source_ref = string_field(&value, &["sourceRef", "source_ref", "replayRef"])
            .or_else(|| ref_uri.clone())
            .unwrap_or_else(|| "tradeassembly://artifact/missing".to_string());
        if value_contains_sensitive_payload(&value) {
            warnings.push(service_warning(
                ResearchNotebookWarningCode::SensitivePayloadRejected,
                "Research notebook skipped an artifact reference with sensitive or raw payload fields.",
            ));
            continue;
        }
        let Some(ref_uri) = ref_uri else {
            warnings.push(service_warning(
                ResearchNotebookWarningCode::BrokenArtifactRef,
                "Research notebook skipped an artifact reference without a stable URI.",
            ));
            continue;
        };
        let Some(content_hash) = content_hash else {
            warnings.push(service_warning(
                ResearchNotebookWarningCode::BrokenArtifactRef,
                "Research notebook skipped an artifact reference without a content hash.",
            ));
            continue;
        };
        if !content_hash.starts_with("sha256:") {
            warnings.push(service_warning(
                ResearchNotebookWarningCode::BrokenArtifactRef,
                "Research notebook skipped an artifact reference with an unsupported content hash.",
            ));
            continue;
        }
        let title = string_field(&value, &["title", "name"])
            .unwrap_or_else(|| artifact_kind.replace('_', " "));
        let artifact_id = string_field(&value, &["artifactId", "artifact_id", "id"])
            .unwrap_or_else(|| stable_id(&artifact_kind, &ref_uri, &content_hash));
        let warning_count = value
            .get("warnings")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let redaction_status = if value
            .get("redactionStatus")
            .or_else(|| value.get("redaction_status"))
            .and_then(Value::as_str)
            .is_some_and(|status| status == "redacted")
        {
            ResearchNotebookRedactionStatus::Redacted
        } else {
            ResearchNotebookRedactionStatus::Clean
        };
        let journal_refs = string_values(&value, "journalRefs", "journal_refs");
        let replay_refs = string_values(&value, "replayRefs", "replay_refs");
        artifacts.push(ArtifactCandidate {
            artifact_id,
            artifact_kind,
            ref_uri,
            content_hash,
            title,
            source_ref,
            warning_count,
            redaction_status,
            journal_refs,
            replay_refs,
        });
    }
    artifacts.sort_by(|left, right| left.artifact_id.cmp(&right.artifact_id));
    artifacts
}

fn artifact_values(request: &Value) -> Vec<Value> {
    [
        "artifactRefs",
        "artifact_refs",
        "outputs",
        "selectedArtifacts",
    ]
    .iter()
    .flat_map(|key| {
        request
            .get(*key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    })
    .collect()
}

fn prompt_refs(
    request: &Value,
    run_id: &str,
    collected_at_unix_ms: u64,
    warnings: &mut Vec<ResearchNotebookWarning>,
) -> Vec<ResearchNotebookPromptRef> {
    request
        .get("prompts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|prompt| {
            if !prompt
                .get("explicitlySaved")
                .or_else(|| prompt.get("explicitly_saved"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                warnings.push(service_warning(
                    ResearchNotebookWarningCode::UnsavedPromptRejected,
                    "Research notebook skipped an unsaved prompt reference.",
                ));
                return None;
            }
            let prompt_ref = string_field(&prompt, &["promptRef", "prompt_ref", "ref"])?;
            let content_hash = string_field(&prompt, &["contentHash", "content_hash"])
                .unwrap_or_else(|| stable_hash(&prompt_ref));
            Some(ResearchNotebookPromptRef {
                prompt_ref,
                role: string_field(&prompt, &["role"]).unwrap_or_else(|| "user".to_string()),
                content_hash,
                explicitly_saved: true,
                redaction_status: ResearchNotebookRedactionStatus::Redacted,
                provenance: provenance(
                    "agent-session",
                    &format!("thread://local/{run_id}"),
                    collected_at_unix_ms,
                ),
            })
        })
        .collect()
}

fn tool_calls(
    artifacts: &[ArtifactCandidate],
    collected_at_unix_ms: u64,
) -> Vec<ResearchNotebookToolCall> {
    artifacts
        .iter()
        .map(|artifact| ResearchNotebookToolCall {
            call_id: format!("tool_{}", artifact.artifact_id),
            tool_name: tool_name(&artifact.artifact_kind).to_string(),
            tool_kind: tool_kind(&artifact.artifact_kind),
            arguments_hash: stable_hash(&artifact.source_ref),
            output_ref: artifact.ref_uri.clone(),
            output_hash: artifact.content_hash.clone(),
            redaction_status: artifact.redaction_status.clone(),
            provenance: provenance(
                "tradeassembly-runtime",
                &artifact.source_ref,
                collected_at_unix_ms,
            ),
        })
        .collect()
}

fn input_refs(
    artifacts: &[ArtifactCandidate],
    collected_at_unix_ms: u64,
) -> Vec<ResearchNotebookInputRef> {
    artifacts
        .iter()
        .map(|artifact| ResearchNotebookInputRef {
            input_id: format!("input_{}", artifact.artifact_id),
            input_kind: artifact.artifact_kind.clone(),
            ref_uri: artifact.ref_uri.clone(),
            content_hash: artifact.content_hash.clone(),
            redaction_status: artifact.redaction_status.clone(),
            provenance: provenance(
                "tradeassembly-runtime",
                &artifact.source_ref,
                collected_at_unix_ms,
            ),
        })
        .collect()
}

fn assumptions(request: &Value) -> Vec<ResearchNotebookAssumption> {
    request
        .get("assumptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            vec![json!({
                "statement": "The notebook includes only selected local TradeAssembly artifact references.",
                "sourceRef": "tradeassembly://research/notebook/request",
                "userSupplied": false
            })]
        })
        .into_iter()
        .enumerate()
        .map(|(index, value)| ResearchNotebookAssumption {
            assumption_id: string_field(&value, &["assumptionId", "assumption_id", "id"])
                .unwrap_or_else(|| format!("assumption_{index:03}")),
            statement: string_field(&value, &["statement"])
                .unwrap_or_else(|| "Selected local artifacts were composed by reference.".to_string()),
            source_ref: string_field(&value, &["sourceRef", "source_ref"])
                .unwrap_or_else(|| "tradeassembly://research/notebook/request".to_string()),
            user_supplied: value
                .get("userSupplied")
                .or_else(|| value.get("user_supplied"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

fn explainable_blocks(
    artifacts: &[ArtifactCandidate],
    request: &Value,
) -> Vec<ResearchNotebookExplainableBlock> {
    let mut blocks = Vec::new();
    blocks.push(ResearchNotebookExplainableBlock {
        block_id: "block_observations".to_string(),
        kind: ResearchNotebookBlockKind::Observation,
        title: "Selected artifacts".to_string(),
        summary: format!(
            "The notebook references {} selected local TradeAssembly artifacts.",
            artifacts.len()
        ),
        source_refs: artifacts
            .iter()
            .map(|artifact| artifact.ref_uri.clone())
            .collect(),
        calculation_ref: None,
        model_ref: None,
        user_decision_ref: None,
    });
    blocks.push(ResearchNotebookExplainableBlock {
        block_id: "block_assumptions".to_string(),
        kind: ResearchNotebookBlockKind::Assumption,
        title: "Composition assumptions".to_string(),
        summary: "The service stores artifact references and hashes, not raw provider or credential-bearing payloads.".to_string(),
        source_refs: vec!["tradeassembly://research/notebook/request".to_string()],
        calculation_ref: None,
        model_ref: None,
        user_decision_ref: None,
    });
    blocks.push(ResearchNotebookExplainableBlock {
        block_id: "block_calculation".to_string(),
        kind: ResearchNotebookBlockKind::DeterministicCalculation,
        title: "Artifact hash ledger".to_string(),
        summary: "Artifact IDs, notebook ID, and export refs are derived deterministically from stable refs and content hashes.".to_string(),
        source_refs: artifacts.iter().map(|artifact| artifact.content_hash.clone()).collect(),
        calculation_ref: Some("tradeassembly://calculation/research-notebook/hash-ledger".to_string()),
        model_ref: None,
        user_decision_ref: None,
    });
    blocks.push(ResearchNotebookExplainableBlock {
        block_id: "block_model".to_string(),
        kind: ResearchNotebookBlockKind::ModelDerivedOutput,
        title: "Agent-facing summary".to_string(),
        summary: "The agent can summarize the referenced artifacts while preserving replay and export refs for inspection.".to_string(),
        source_refs: artifacts.iter().map(|artifact| artifact.ref_uri.clone()).collect(),
        calculation_ref: None,
        model_ref: Some("model://active-thread".to_string()),
        user_decision_ref: None,
    });
    blocks.push(ResearchNotebookExplainableBlock {
        block_id: "block_limitations".to_string(),
        kind: ResearchNotebookBlockKind::Limitation,
        title: "Notebook limits".to_string(),
        summary: "The notebook is descriptive research evidence and does not mutate strategy logic, credentials, broker state, or activation.".to_string(),
        source_refs: vec!["tradeassembly://research/notebook/limits".to_string()],
        calculation_ref: None,
        model_ref: None,
        user_decision_ref: None,
    });
    if let Some(note_ref) = string_field(
        request,
        &["userDecisionRef", "user_decision_ref", "noteRef"],
    ) {
        blocks.push(ResearchNotebookExplainableBlock {
            block_id: "block_user_decision".to_string(),
            kind: ResearchNotebookBlockKind::UserAuthoredDecision,
            title: "Saved user note".to_string(),
            summary:
                "The notebook includes a user-authored decision or note reference for later review."
                    .to_string(),
            source_refs: vec![note_ref.clone()],
            calculation_ref: None,
            model_ref: None,
            user_decision_ref: Some(note_ref),
        });
    } else {
        blocks.push(ResearchNotebookExplainableBlock {
            block_id: "block_user_decision".to_string(),
            kind: ResearchNotebookBlockKind::UserAuthoredDecision,
            title: "No saved user decision".to_string(),
            summary: "No user-authored strategy change or activation decision was stored by this composition run.".to_string(),
            source_refs: vec!["tradeassembly://research/notebook/no-user-decision".to_string()],
            calculation_ref: None,
            model_ref: None,
            user_decision_ref: None,
        });
    }
    blocks
}

fn report_refs(artifacts: &[ArtifactCandidate]) -> Vec<ResearchNotebookReportRef> {
    artifacts
        .iter()
        .filter(|artifact| {
            !artifact.artifact_kind.contains("chart")
                && !artifact.artifact_kind.contains("table")
                && !artifact.artifact_kind.contains("csv")
        })
        .map(|artifact| ResearchNotebookReportRef {
            report_id: format!("report_{}", artifact.artifact_id),
            report_kind: artifact.artifact_kind.clone(),
            report_ref: artifact.ref_uri.clone(),
            content_hash: artifact.content_hash.clone(),
        })
        .collect()
}

fn chart_refs(artifacts: &[ArtifactCandidate]) -> Vec<ResearchNotebookChartRef> {
    artifacts
        .iter()
        .filter(|artifact| {
            artifact.artifact_kind.contains("chart")
                || artifact.ref_uri.ends_with(".svg")
                || artifact.ref_uri.ends_with(".png")
        })
        .map(|artifact| ResearchNotebookChartRef {
            chart_id: format!("chart_{}", artifact.artifact_id),
            title: artifact.title.clone(),
            artifact_ref: artifact.ref_uri.clone(),
            content_hash: artifact.content_hash.clone(),
        })
        .collect()
}

fn table_refs(artifacts: &[ArtifactCandidate]) -> Vec<ResearchNotebookTableRef> {
    artifacts
        .iter()
        .filter(|artifact| {
            artifact.artifact_kind.contains("table")
                || artifact.artifact_kind.contains("csv")
                || artifact.ref_uri.ends_with(".csv")
        })
        .map(|artifact| ResearchNotebookTableRef {
            table_id: format!("table_{}", artifact.artifact_id),
            title: artifact.title.clone(),
            artifact_ref: artifact.ref_uri.clone(),
            content_hash: artifact.content_hash.clone(),
        })
        .collect()
}

fn replay_refs(notebook_id: &str, run_id: &str) -> Vec<ResearchNotebookReplayRef> {
    vec![
        ResearchNotebookReplayRef {
            replay_ref: format!("tradeassembly://research-notebook/{notebook_id}/replay"),
            deterministic: true,
        },
        ResearchNotebookReplayRef {
            replay_ref: format!("journal://research/{run_id}"),
            deterministic: true,
        },
    ]
}

fn metadata(artifacts: &[ArtifactCandidate]) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("artifactRefCount".to_string(), json!(artifacts.len())),
        (
            "warningSummary".to_string(),
            json!({
                "artifactWarnings": artifacts.iter().map(|artifact| artifact.warning_count).sum::<usize>()
            }),
        ),
    ])
}

fn export_refs(notebook_id: &str, export: &ResearchNotebookExportBundle) -> Value {
    json!({
        "json": {"uri": format!("tradeassembly://artifact/research-notebook/{notebook_id}/notebook.json"), "contentHash": export.content_hash},
        "markdown": {"uri": format!("tradeassembly://artifact/research-notebook/{notebook_id}/notebook.md"), "contentHash": export.content_hash},
        "html": {"uri": format!("tradeassembly://artifact/research-notebook/{notebook_id}/notebook.html"), "contentHash": export.content_hash},
    })
}

fn artifact_ref_json(artifact: &ArtifactCandidate) -> Value {
    json!({
        "artifactId": artifact.artifact_id,
        "artifactKind": artifact.artifact_kind,
        "refUri": artifact.ref_uri,
        "contentHash": artifact.content_hash,
        "title": artifact.title,
        "redactionStatus": artifact.redaction_status,
        "journalRefs": artifact.journal_refs,
        "replayRefs": artifact.replay_refs,
    })
}

fn tool_kind(kind: &str) -> ResearchNotebookToolKind {
    match kind {
        value if value.contains("backtest") => ResearchNotebookToolKind::Backtest,
        value if value.contains("monte") => ResearchNotebookToolKind::MonteCarlo,
        value if value.contains("valuation") || value.contains("greeks") => {
            ResearchNotebookToolKind::ScenarioValuation
        }
        value if value.contains("selector") => ResearchNotebookToolKind::SelectorExplain,
        value
            if value.contains("ledger")
                || value.contains("portfolio_risk")
                || value.contains("journal") =>
        {
            ResearchNotebookToolKind::Other
        }
        value if value.contains("fill_quality") || value.contains("fill-quality") => {
            ResearchNotebookToolKind::FillQuality
        }
        value if value.contains("calendar") => ResearchNotebookToolKind::LifecycleCalendar,
        value if value.contains("data") => ResearchNotebookToolKind::DataQuery,
        _ => ResearchNotebookToolKind::Other,
    }
}

fn tool_name(kind: &str) -> &str {
    match kind {
        value if value.contains("backtest") => "tradeassembly.backtest.run",
        value if value.contains("monte") => "tradeassembly.monte_carlo.report",
        value if value.contains("valuation") || value.contains("greeks") => {
            "tradeassembly.scenario.valuation"
        }
        value if value.contains("selector") => "tradeassembly.selector.explain",
        value if value.contains("ledger") => "tradeassembly.ledger.inspect",
        value if value.contains("portfolio_risk") => "tradeassembly.portfolio_risk.report",
        value if value.contains("journal") => "tradeassembly.journal.analytics",
        value if value.contains("fill_quality") || value.contains("fill-quality") => {
            "tradeassembly.fill_quality.report"
        }
        value if value.contains("calendar") => "tradeassembly.lifecycle_calendar.inspect",
        _ => "tradeassembly.artifact.inspect",
    }
}

fn service_warning(code: ResearchNotebookWarningCode, message: &str) -> ResearchNotebookWarning {
    ResearchNotebookWarning {
        code,
        severity: ResearchNotebookWarningSeverity::Error,
        message: message.to_string(),
        replay_ref: "tradeassembly://research-notebook/pending/replay".to_string(),
    }
}

fn provenance(
    source: &str,
    source_ref: &str,
    collected_at_unix_ms: u64,
) -> ResearchNotebookSourceProvenance {
    ResearchNotebookSourceProvenance {
        source: source.to_string(),
        source_ref: source_ref.to_string(),
        collected_at_unix_ms,
        raw_secrets_exposed: false,
    }
}

fn notebook_id(
    strategy_id: &str,
    title: &str,
    assumptions: &[ResearchNotebookAssumption],
    artifacts: &[ArtifactCandidate],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(strategy_id.as_bytes());
    hasher.update(title.as_bytes());
    hasher.update(
        serde_json_canonicalizer::to_vec(&assumptions.to_vec())
            .expect("research notebook assumptions are serializable"),
    );
    for artifact in artifacts {
        hasher.update(artifact.artifact_id.as_bytes());
        hasher.update(artifact.content_hash.as_bytes());
    }
    format!("notebook-{:x}", hasher.finalize())[..25].to_string()
}

fn stable_id(kind: &str, ref_uri: &str, content_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(ref_uri.as_bytes());
    hasher.update(content_hash.as_bytes());
    format!(
        "{}-{:x}",
        kind.replace(['_', '/', ':'], "-"),
        hasher.finalize()
    )[..32]
        .to_string()
}

fn stable_hash(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

fn value_contains_sensitive_payload(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            normalized.contains("secret")
                || normalized.contains("token")
                || normalized.contains("password")
                || normalized.contains("credential")
                || normalized.contains("rawpayload")
                || normalized.contains("raw_payload")
                || value_contains_sensitive_payload(value)
        }),
        Value::Array(values) => values.iter().any(value_contains_sensitive_payload),
        Value::String(value) => {
            let normalized = value.to_ascii_lowercase();
            normalized.contains("api_secret")
                || normalized.contains("broker_account_id")
                || normalized.contains("raw_payload")
                || normalized.contains("hosted-control")
        }
        _ => false,
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn string_values(value: &Value, camel: &str, snake: &str) -> Vec<String> {
    value
        .get(camel)
        .or_else(|| value.get(snake))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn result_placeholder() -> &'static str {
    "__pending__"
}

fn result_count_placeholder() -> Value {
    json!(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn research_notebook_service_rejects_unresolved_selectors_without_mutation() {
        let service = test_service("happy");
        let result = service.research_notebook(sample_request());

        assert_eq!(result["ok"], json!(false));
        assert_eq!(
            result["code"],
            json!("research_notebook_artifact_not_found")
        );
        assert!(service.latest_research_notebook().is_none());
    }

    #[test]
    fn research_notebook_service_is_deterministic() {
        let service = test_service("deterministic");
        let first = service.research_notebook(sample_request());
        let second = service.research_notebook(sample_request());

        assert_eq!(first["notebookId"], second["notebookId"]);
        assert_eq!(first["notebookHash"], second["notebookHash"]);
        assert_eq!(first["exportRefs"], second["exportRefs"]);
    }

    #[test]
    fn research_notebook_service_rejects_legacy_refs_without_secret_echo() {
        let service = test_service("broken");
        let result = service.research_notebook(json!({
            "strategyId": "strat_local_btc_demo",
            "runId": "research-run-broken",
            "artifactRefs": [
                {"artifactKind": "backtest", "refUri": "tradeassembly://report/backtest"},
                {"artifactKind": "fill_quality", "contentHash": "sha256:fill"},
                {"artifactKind": "calendar", "refUri": "tradeassembly://calendar", "contentHash": "md5:bad"},
                {"artifactKind": "broker_payload", "refUri": "tradeassembly://raw", "contentHash": "sha256:raw", "rawPayload": {"api_secret": "DO_NOT_ECHO"}}
            ]
        }));
        let rendered = serde_json::to_string(&result).expect("serialize result");

        assert_eq!(result["ok"], json!(false));
        assert_eq!(
            result["code"],
            json!("research_notebook_artifact_selectors_required")
        );
        assert!(!rendered.contains("DO_NOT_ECHO"));
    }

    #[test]
    fn research_notebook_service_does_not_accept_prompts_as_artifact_evidence() {
        let service = test_service("prompts");
        let mut request = sample_request();
        request["prompts"] = json!([
            {"promptRef": "tradeassembly://prompt/saved", "contentHash": "sha256:saved", "explicitlySaved": true, "role": "user"},
            {"promptRef": "tradeassembly://prompt/unsaved", "contentHash": "sha256:unsaved", "explicitlySaved": false, "role": "assistant"}
        ]);
        let result = service.research_notebook(request);
        let rendered = serde_json::to_string(&result).expect("serialize result");

        assert_eq!(result["ok"], json!(false));
        assert!(rendered.contains("research_notebook_artifact_not_found"));
        assert!(!rendered.contains("tradeassembly://prompt/saved"));
    }

    #[test]
    fn research_notebook_service_output_contains_no_advice_copy() {
        let service = test_service("no-advice");
        let rendered = serde_json::to_string(&service.research_notebook(sample_request()))
            .expect("serialize result")
            .to_ascii_lowercase();

        for forbidden in [
            "should buy",
            "should sell",
            "activate now",
            "increase allocation",
            "decrease allocation",
        ] {
            assert!(!rendered.contains(forbidden), "{forbidden}");
        }
    }

    fn test_service(name: &str) -> TradeAssemblyService {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tradeassembly-research-notebook-{name}-{}-{stamp}.db",
            std::process::id()
        ));
        TradeAssemblyService::test_local(path.to_string_lossy().to_string())
    }

    fn sample_request() -> Value {
        json!({
            "strategyId": "strat_local_btc_demo",
            "runId": "research-run-001",
            "title": "Local strategy research notebook",
            "artifactSelectors": [{"kind":"backtest","id":"backtest-001"}],
            "assumptions": [
                {"assumptionId": "assumption-local", "statement": "The service used selected local artifacts only.", "sourceRef": "tradeassembly://research/request", "userSupplied": false}
            ],
            "userDecisionRef": "tradeassembly://note/review-later"
        })
    }
}
