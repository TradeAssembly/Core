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
    let symbol = body
        .get("symbol")
        .and_then(Value::as_str)
        .unwrap_or("BTC/USD");
    let id = next_id(service, DATASETS_NS, "ds", &strategy_id);
    let records = body.get("rows").and_then(Value::as_array).cloned();
    let row_count = body
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| rows.len())
        .or_else(|| {
            body.get("rows")
                .and_then(Value::as_u64)
                .map(|rows| rows as usize)
        })
        .unwrap_or(6);
    let dataset = json!({
        "id": id,
        "datasetId": id,
        "strategy_id": strategy_id,
        "strategyId": strategy_id,
        "symbol": symbol,
        "records": records.clone().unwrap_or_default(),
        "rows": row_count,
        "rowCount": row_count,
        "snapshotRef": format!("snapshot://local/{id}"),
        "content_hash": format!("sha256:{}", slug(&id)),
        "contentHash": format!("sha256:{}", slug(&id)),
        "sourcePluginRef": body.get("sourcePluginRef").or_else(|| body.get("source_plugin_ref")).and_then(Value::as_str).unwrap_or("tradeassembly.local-data"),
        "capability": body.get("capability").and_then(Value::as_str).unwrap_or("marketdata.bars"),
        "timeSlice": body.get("timeSlice").or_else(|| body.get("time_slice")).cloned().unwrap_or_else(|| json!({"start": "2026-01-02T00:00:00Z", "end": "2026-01-02T00:05:00Z"})),
        "normalization": body.get("normalization").cloned().unwrap_or_else(|| json!({"kind": "close_price", "missingBars": "fail_closed"})),
        "timezone": body.get("timezone").and_then(Value::as_str).unwrap_or("UTC"),
        "calendar": body.get("calendar").and_then(Value::as_str).unwrap_or("crypto_24x7"),
        "session": body.get("session").and_then(Value::as_str).unwrap_or("continuous"),
        "capital": body.get("capital").cloned().unwrap_or_else(|| json!({"currency": "USD", "startingCash": body.get("startingCash").or_else(|| body.get("starting_cash")).and_then(Value::as_f64).unwrap_or(1000.0)})),
        "slippage": body.get("slippage").cloned().unwrap_or_else(|| json!({"model": "fixed_bps", "bps": body.get("slippageBps").or_else(|| body.get("slippage_bps")).and_then(Value::as_f64).unwrap_or(2.0)})),
        "spread": body.get("spread").cloned().unwrap_or_else(|| json!({"model": "fixed_bps", "bps": body.get("spreadBps").or_else(|| body.get("spread_bps")).and_then(Value::as_f64).unwrap_or(1.0)})),
        "fillModel": body.get("fillModel").or_else(|| body.get("fill_model")).cloned().unwrap_or_else(|| json!("next_bar_close")),
        "feeModel": body.get("feeModel").or_else(|| body.get("fee_model")).cloned().unwrap_or_else(|| json!({"kind": "flat", "amount": body.get("feeAmount").or_else(|| body.get("fee_amount")).and_then(Value::as_f64).unwrap_or(0.01), "currency": "USD"})),
        "orderPolicy": body.get("orderPolicy").or_else(|| body.get("order_policy")).cloned().unwrap_or_else(|| json!({"entry": "market", "exit": "market", "unclosedPosition": "value_at_dataset_end"})),
    });
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
        .unwrap_or_else(|| json!(["BTC/USD", "ETH/USD"]));
    let universe = json!({
        "id": id,
        "strategy_id": strategy_id,
        "strategyId": strategy_id,
        "symbols": symbols,
        "provider_ref": body.get("providerRef").or_else(|| body.get("provider_ref")).cloned().unwrap_or_else(|| json!("local-data")),
        "metrics": {"rows": 3},
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

pub(crate) fn create_job(service: &TradeAssemblyService, body: Value) -> Value {
    let strategy_id = strategy_id_from(&body);
    if service.require_object("strategy", &strategy_id).is_err() {
        return unavailable();
    }
    let id = next_id(service, JOBS_NS, "research_job", &strategy_id);
    let backtest = backtest::run_backtest(service, body.clone());
    if backtest["status"].as_str() == Some("blocked") {
        return json!({
            "id": id,
            "status": "blocked",
            "strategy_id": strategy_id,
            "strategyId": strategy_id,
            "blockedBacktest": backtest,
            "artifacts": [],
            "noAdvice": super::LEGAL_BOUNDARY,
        });
    }
    let backtest_id = backtest["id"].as_str().unwrap_or("backtest-local");
    let job = json!({
        "id": id,
        "status": "completed",
        "strategy_id": strategy_id,
        "strategyId": strategy_id,
        "dataset_id": body.get("datasetId").or_else(|| body.get("dataset_id")).cloned().unwrap_or_else(|| json!("dataset-local-fixture")),
        "datasetId": body.get("datasetId").or_else(|| body.get("dataset_id")).cloned().unwrap_or_else(|| json!("dataset-local-fixture")),
        "engine": body.get("engine").cloned().unwrap_or_else(|| json!("auto")),
        "backtest_id": backtest_id,
        "backtestId": backtest_id,
        "artifacts": backtest["artifacts"].clone(),
    });
    if service
        .bind_inherited_object("research_job", &id, "strategy", &strategy_id)
        .is_err()
    {
        return unavailable();
    }
    persist(
        service,
        JOBS_NS,
        &id,
        job.clone(),
        "research.job.completed",
        &body,
    );
    job
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
    }
    .unwrap_or_else(|| json!({"status": "not_found"}));
    json!({"job": job, "artifacts": job["artifacts"].clone()})
}

pub(crate) fn list_jobs(service: &TradeAssemblyService) -> Vec<Value> {
    visible_stored(service, "research_job", JOBS_NS, &["id"])
}

pub(crate) fn list_sweeps(service: &TradeAssemblyService) -> Vec<Value> {
    visible_stored(service, "robustness_run", SWEEPS_NS, &["id", "runId"])
}

pub(crate) fn list_universes(service: &TradeAssemblyService) -> Vec<Value> {
    visible_or_default(
        service,
        "research_universe",
        UNIVERSES_NS,
        json!({"id": "universe_local", "symbols": ["BTC/USD", "ETH/USD"]}),
    )
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
    let datasets = visible_stored(service, "dataset", DATASETS_NS, &["id", "datasetId"])
        .into_iter()
        .filter(|dataset| dataset["strategyId"].as_str() == Some(strategy_id))
        .collect::<Vec<_>>();
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
        "datasetSnapshots": [],
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
        "journal": journal_events(),
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
        "evidence": workspace_evidence(),
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

fn visible_or_default(
    service: &TradeAssemblyService,
    object_type: &str,
    namespace: &str,
    default: Value,
) -> Vec<Value> {
    let stored = visible_stored(service, object_type, namespace, &["id"]);
    if stored.is_empty() && service.invocation_owner().is_none() {
        vec![default]
    } else {
        stored
    }
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
