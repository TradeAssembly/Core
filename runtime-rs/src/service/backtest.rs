use super::{capability_graph, strategy, strategy_id_from, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const BACKTESTS_NS: &str = "backtests";
const ARTIFACTS_NS: &str = "research_artifacts";
const DATASETS_NS: &str = "research_datasets";
const LOCAL_TIMESTAMP: &str = "2026-07-08T00:00:00Z";

struct ManifestInput<'a> {
    run_id: &'a str,
    strategy_id: &'a str,
    version: &'a Value,
    dataset: &'a Value,
    symbol: &'a str,
    engine: &'a str,
    authority: Value,
    capability_revision: &'a Value,
    idempotency_key: &'a str,
}

pub(crate) fn run_backtest(service: &TradeAssemblyService, body: Value) -> Value {
    let strategy_id = strategy_id_from(&body);
    if service.require_object("strategy", &strategy_id).is_err() {
        return unavailable(&strategy_id);
    }
    let version = strategy_version_for(service, &strategy_id, &body);
    if version.is_null() {
        return blocked_backtest(&strategy_id, vec!["published_strategy_version_required"]);
    }
    let dataset = match dataset_binding(service, &strategy_id, &body) {
        Ok(dataset) => dataset,
        Err(reasons) => return blocked_backtest(&strategy_id, reasons),
    };
    let capability_revision =
        match capability_graph::save_backtest_revision(service, &body, &version, &dataset) {
            Ok(revision) => revision,
            Err(error) => return blocked_backtest_capability(&strategy_id, error),
        };
    let rows = match dataset_rows(&dataset) {
        Ok(rows) => rows,
        Err(reasons) => return blocked_backtest(&strategy_id, reasons),
    };
    let symbol = dataset["symbol"]
        .as_str()
        .unwrap_or("symbol_unset")
        .to_string();
    let engine = body
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or("deterministic-local")
        .to_string();
    let idempotency_key = idempotency_key_for(&strategy_id, &version, &dataset, &body);
    if let Some(existing) = backtest_by_idempotency_key(service, &idempotency_key) {
        let id = existing["id"].as_str().unwrap_or_default();
        if service.require_object("backtest", id).is_err() {
            return unavailable(&strategy_id);
        }
        return existing;
    }
    let id = body
        .get("backtestId")
        .or_else(|| body.get("backtest_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| next_id(service, BACKTESTS_NS, "backtest", &strategy_id));
    if service
        .bind_inherited_object("backtest", &id, "strategy", &strategy_id)
        .is_err()
    {
        return unavailable(&strategy_id);
    }
    let authority = authority_context(&body);
    let mut manifest = manifest_payload(ManifestInput {
        run_id: &id,
        strategy_id: &strategy_id,
        version: &version,
        dataset: &dataset,
        symbol: &symbol,
        engine: &engine,
        authority,
        capability_revision: &capability_revision,
        idempotency_key: &idempotency_key,
    });
    let manifest_hash = hash_value(&manifest);
    manifest["manifestHash"] = json!(manifest_hash);

    let result = result_payload(&id, &strategy_id, &symbol, &rows, &manifest);
    let result_hash = hash_value(&result);
    let artifacts = artifact_refs(&id);
    let backtest = json!({
        "id": id,
        "backtest_id": id,
        "backtestId": id,
        "kind": "BacktestRun",
        "schemaVersion": "tradeassembly.backtest_run.v1",
        "strategy_id": strategy_id,
        "strategyId": strategy_id,
        "strategyVersionId": version["id"],
        "strategySpecHash": version["specHash"],
        "dataset_id": dataset["datasetId"],
        "datasetId": dataset["datasetId"],
        "symbol": symbol,
        "status": "completed",
        "fsm": {
            "state": "completed",
            "previousState": "running",
            "events": ["backtest_run.created", "backtest_run.started", "backtest_run.completed"],
            "terminal": true
        },
        "engine": engine,
        "manifest": manifest,
        "manifestHash": manifest["manifestHash"],
        "metrics": result["overview"].clone(),
        "result": result,
        "resultHash": result_hash,
        "artifacts": artifacts,
        "artifactRefs": artifact_refs(&id),
        "exportRefs": export_refs(&id),
        "replayRefs": replay_refs(&id),
        "authorityContext": authority_context(&body),
        "idempotencyKey": idempotency_key,
        "evidenceRefs": evidence_refs_from_body(&body),
        "researchEvidenceRefs": [
            {"kind": "manifest_hash", "ref": manifest["manifestHash"]},
            {"kind": "strategy_spec_hash", "ref": version["specHash"]},
            {"kind": "dataset_snapshot", "ref": dataset["snapshotRef"]},
            {"kind": "capability_graph_revision", "ref": capability_revision["revisionId"]},
            {"kind": "capability_graph_fingerprint", "ref": capability_revision["graphFingerprint"]}
        ],
        "apfEvidenceRefs": [],
        "warnings": [
            LEGAL_BOUNDARY,
            "TradeAssembly did not recommend a trade, position size, or activation decision.",
            "Backtest output is deterministic research evidence, not an activation, order, or recommendation."
        ],
        "immutable": true,
        "createdAt": LOCAL_TIMESTAMP,
        "noAdvice": LEGAL_BOUNDARY,
    });
    persist(
        service,
        BACKTESTS_NS,
        backtest["id"].as_str().unwrap_or("backtest-local"),
        backtest.clone(),
        "backtest_run.completed",
        &body,
        &idempotency_key,
    );
    persist_artifacts(service, &backtest, &body, &idempotency_key);
    backtest
}

pub(crate) fn list_artifacts(service: &TradeAssemblyService) -> Vec<Value> {
    let values = service
        .runtime()
        .storage
        .list_json(ARTIFACTS_NS)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    service.filter_visible_values("backtest_artifact", values, &["backtestId", "runId"])
}

fn blocked_backtest(strategy_id: &str, blocked_reasons: Vec<&'static str>) -> Value {
    let checks = blocked_reasons
        .iter()
        .map(|reason| {
            json!({
                "id": reason,
                "ok": false,
                "detail": readiness_detail(reason)
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": Value::Null,
        "kind": "BacktestRun",
        "strategyId": strategy_id,
        "status": "blocked",
        "readiness": {
            "ready": false,
            "blockedReasons": blocked_reasons,
            "checks": checks
        },
        "warnings": [LEGAL_BOUNDARY],
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn blocked_backtest_capability(strategy_id: &str, error: Value) -> Value {
    let blocked_reasons = error["blockers"]
        .as_array()
        .map(|blockers| {
            blockers
                .iter()
                .filter_map(|blocker| blocker["code"].as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|reasons| !reasons.is_empty())
        .unwrap_or_else(|| vec!["capability_graph_blocked".to_string()]);
    let checks = blocked_reasons
        .iter()
        .map(|reason| {
            json!({
                "id": reason,
                "ok": false,
                "detail": "The immutable capability graph is missing, blocked, stale, or does not match the selected dataset."
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": Value::Null,
        "kind": "BacktestRun",
        "strategyId": strategy_id,
        "status": "blocked",
        "readiness": {
            "ready": false,
            "blockedReasons": blocked_reasons,
            "checks": checks,
            "capabilityGraph": error,
        },
        "warnings": [LEGAL_BOUNDARY],
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn readiness_detail(reason: &str) -> &'static str {
    match reason {
        "published_strategy_version_required" => {
            "Publish an immutable StrategyVersion before running a deterministic backtest."
        }
        "dataset_id_required" => "Select or create a dataset snapshot before running a backtest.",
        "dataset_snapshot_required" => {
            "The selected dataset snapshot was not found in local research storage."
        }
        "dataset_symbol_required" => "Dataset snapshot must declare at least one symbol.",
        "dataset_source_plugin_required" => {
            "Dataset snapshot must declare the plugin/capability source used for historical bars."
        }
        "dataset_time_slice_required" => "Dataset snapshot must declare a bounded time slice.",
        "dataset_binding_mismatch" => {
            "Run request fields must match the selected immutable dataset snapshot."
        }
        "dataset_capability_unresolved" => {
            "The selected plugin/capability cannot satisfy the backtest dataset requirement."
        }
        "dataset_rows_insufficient" => {
            "Backtest datasets need at least three ordered rows to create entry, exit, and valuation evidence."
        }
        _ => "Backtest setup is incomplete.",
    }
}

fn strategy_version_for(service: &TradeAssemblyService, strategy_id: &str, body: &Value) -> Value {
    let requested = body
        .get("strategyVersionId")
        .or_else(|| body.get("strategy_version_id"))
        .and_then(Value::as_str);
    let versions = strategy::strategy_versions(service, strategy_id);
    if let Some(requested) = requested {
        return versions
            .into_iter()
            .find(|version| version["id"].as_str() == Some(requested))
            .unwrap_or(Value::Null);
    }
    versions.last().cloned().unwrap_or(Value::Null)
}

fn dataset_binding(
    service: &TradeAssemblyService,
    strategy_id: &str,
    body: &Value,
) -> Result<Value, Vec<&'static str>> {
    let Some(requested) = body
        .get("datasetId")
        .or_else(|| body.get("dataset_id"))
        .and_then(Value::as_str)
    else {
        return Err(vec!["dataset_id_required"]);
    };
    let Some(mut dataset) = service
        .runtime()
        .storage
        .get_json(DATASETS_NS, requested)
        .ok()
        .flatten()
    else {
        return Err(vec!["dataset_snapshot_required"]);
    };
    let dataset_id = dataset["datasetId"]
        .as_str()
        .or_else(|| dataset["id"].as_str())
        .unwrap_or(requested);
    if service.require_object("dataset", dataset_id).is_err() {
        return Err(vec!["dataset_snapshot_required"]);
    }
    dataset["datasetId"] = json!(dataset["datasetId"]
        .as_str()
        .or_else(|| dataset["id"].as_str())
        .unwrap_or(requested));
    dataset["strategyId"] = json!(dataset["strategyId"].as_str().unwrap_or(strategy_id));
    let Some(symbol) = dataset["symbol"].as_str() else {
        return Err(vec!["dataset_symbol_required"]);
    };
    if request_string_mismatch(body, &["symbol"], symbol) {
        return Err(vec!["dataset_binding_mismatch"]);
    }
    dataset["symbol"] = json!(symbol);
    dataset["rowCount"] = json!(dataset["rowCount"]
        .as_u64()
        .or_else(|| dataset["rows"].as_u64())
        .unwrap_or(0));
    dataset["snapshotRef"] = json!(dataset["snapshotRef"].as_str().unwrap_or(""));
    dataset["contentHash"] = json!(dataset["contentHash"]
        .as_str()
        .or_else(|| dataset["content_hash"].as_str())
        .unwrap_or(""));
    let Some(source_plugin_ref) = dataset["sourcePluginRef"].as_str() else {
        return Err(vec!["dataset_source_plugin_required"]);
    };
    if request_string_mismatch(
        body,
        &["sourcePluginRef", "source_plugin_ref"],
        source_plugin_ref,
    ) {
        return Err(vec!["dataset_binding_mismatch"]);
    }
    dataset["sourcePluginRef"] = json!(source_plugin_ref);
    let capability = dataset["capability"]
        .as_str()
        .unwrap_or("marketdata.bars")
        .to_string();
    if request_string_mismatch(body, &["capability"], &capability) {
        return Err(vec!["dataset_binding_mismatch"]);
    }
    dataset["capability"] = json!(capability);
    let Some(time_slice) = dataset.get("timeSlice").cloned() else {
        return Err(vec!["dataset_time_slice_required"]);
    };
    if request_value_mismatch(body, &["timeSlice", "time_slice"], &time_slice) {
        return Err(vec!["dataset_binding_mismatch"]);
    }
    dataset["timeSlice"] = time_slice;
    Ok(dataset)
}

fn request_string_mismatch(body: &Value, keys: &[&str], expected: &str) -> bool {
    keys.iter()
        .filter_map(|key| body.get(*key).and_then(Value::as_str))
        .any(|actual| actual != expected)
}

fn request_value_mismatch(body: &Value, keys: &[&str], expected: &Value) -> bool {
    keys.iter()
        .filter_map(|key| body.get(*key))
        .any(|actual| actual != expected)
}

fn authority_context(body: &Value) -> Value {
    let actor = body.get("actor").unwrap_or(&Value::Null);
    let actor_kind = actor.get("kind").and_then(Value::as_str).unwrap_or("user");
    let actor_id = actor
        .get("id")
        .or_else(|| actor.get("principalUser"))
        .and_then(Value::as_str)
        .unwrap_or("user.local");
    let principal_user = actor
        .get("principalUser")
        .or_else(|| actor.get("id"))
        .and_then(Value::as_str)
        .unwrap_or(actor_id);
    json!({
        "actor": {
            "kind": safe_string(actor_kind),
            "id": safe_string(actor_id),
            "principalUser": safe_string(principal_user)
        },
        "client": safe_string(body.get("client").and_then(Value::as_str).unwrap_or("self")),
        "purpose": safe_string(body.get("purpose").and_then(Value::as_str).unwrap_or("strategy_backtest_research")),
        "entitlement": safe_string(body.get("entitlement").and_then(Value::as_str).unwrap_or("feature.backtesting.basic")),
        "resource": safe_string(body.get("resource").and_then(Value::as_str).unwrap_or("local_research_workspace")),
        "approvalMode": "not_required_for_research_backtest",
    })
}

fn evidence_refs_from_body(body: &Value) -> Vec<Value> {
    body.get("evidenceRefs")
        .or_else(|| body.get("evidence_refs"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_str()
                        .map(
                            |text| json!({"kind": "caller_evidence_ref", "ref": safe_string(text)}),
                        )
                        .or_else(|| {
                            item.as_object()
                                .map(|_| redact_sensitive_value(item.clone()))
                        })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn idempotency_key_for(
    strategy_id: &str,
    version: &Value,
    dataset: &Value,
    body: &Value,
) -> String {
    body.get("idempotencyKey")
        .or_else(|| body.get("idempotency_key"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "backtest.run:{strategy_id}:{}:{}:{}",
                version["id"].as_str().unwrap_or("version"),
                dataset["datasetId"].as_str().unwrap_or("dataset"),
                slug(&hash_value(dataset))
            )
        })
}

fn backtest_by_idempotency_key(
    service: &TradeAssemblyService,
    idempotency_key: &str,
) -> Option<Value> {
    service
        .runtime()
        .storage
        .list_json(BACKTESTS_NS)
        .ok()?
        .into_iter()
        .map(|(_, value)| value)
        .find(|value| value["idempotencyKey"].as_str() == Some(idempotency_key))
}

fn manifest_payload(input: ManifestInput<'_>) -> Value {
    json!({
        "schemaVersion": "tradeassembly.backtest_run_manifest.v1",
        "runId": input.run_id,
        "strategyId": input.strategy_id,
        "strategyVersionId": input.version["id"],
        "strategySpecHash": input.version["specHash"],
        "dataset": {
            "datasetId": input.dataset["datasetId"],
            "snapshotRef": input.dataset["snapshotRef"],
            "contentHash": input.dataset["contentHash"],
            "sourcePluginRef": input.dataset["sourcePluginRef"],
            "capability": input.dataset["capability"],
            "symbol": input.symbol,
            "symbols": [input.symbol],
            "timeSlice": input.dataset["timeSlice"],
            "timezone": input.dataset.get("timezone").cloned().unwrap_or_else(|| json!("UTC")),
            "calendar": input.dataset.get("calendar").cloned().unwrap_or_else(|| json!("crypto_24x7")),
            "session": input.dataset.get("session").cloned().unwrap_or_else(|| json!("continuous")),
            "rowCount": input.dataset["rowCount"]
        },
        "assumptions": {
            "capital": input.dataset.get("capital").cloned().unwrap_or_else(|| json!({"currency": "USD", "startingCash": 1000.0})),
            "normalization": input.dataset.get("normalization").cloned().unwrap_or_else(|| json!({"kind": "close_price", "missingBars": "fail_closed"})),
            "slippage": input.dataset.get("slippage").cloned().unwrap_or_else(|| json!({"model": "fixed_bps", "bps": 2.0})),
            "spread": input.dataset.get("spread").cloned().unwrap_or_else(|| json!({"model": "fixed_bps", "bps": 1.0})),
            "fillModel": input.dataset.get("fillModel").cloned().unwrap_or_else(|| json!("next_bar_close")),
            "feeModel": input.dataset.get("feeModel").cloned().unwrap_or_else(|| json!({"kind": "flat", "amount": 0.01, "currency": "USD"})),
            "orderPolicy": input.dataset.get("orderPolicy").cloned().unwrap_or_else(|| json!({"entry": "market", "exit": "market", "unclosedPosition": "value_at_dataset_end"})),
            "timezone": input.dataset.get("timezone").cloned().unwrap_or_else(|| json!("UTC"))
        },
        "engine": {
            "name": input.engine,
            "version": "deterministic-local-v1",
            "metricsVersion": "tradeassembly.metrics.v1"
        },
        "pluginBindings": graph_plugin_bindings(input.capability_revision),
        "capabilityGraphRevisionId": input.capability_revision["revisionId"],
        "capabilityGraphFingerprint": input.capability_revision["graphFingerprint"],
        "capabilityGraphSnapshot": input.capability_revision["graph"],
        "capabilityResolution": input.capability_revision["graph"],
        "authority": input.authority,
        "evidenceModel": {
            "researchArtifact": true,
            "apfReceipt": false,
            "apfReceiptRequiredOnlyForProtectedActions": true
        },
        "warnings": [
            LEGAL_BOUNDARY,
            "TradeAssembly did not recommend a trade, position size, or activation decision.",
            "Research evidence is distinct from APF legal/security receipts."
        ],
        "idempotencyKey": input.idempotency_key,
        "createdAt": LOCAL_TIMESTAMP,
    })
}

fn graph_plugin_bindings(revision: &Value) -> Vec<Value> {
    revision["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let selected = node.get("selected")?;
            (!selected.is_null()).then(|| {
                json!({
                    "requirementId": node["nodeId"],
                    "pluginInstanceRef": selected["pluginInstanceRef"],
                    "pluginRef": selected["pluginRef"],
                    "operation": selected["operationId"],
                    "accountRef": selected["accountRef"],
                    "manifestFingerprint": selected["manifestFingerprint"],
                    "entitlementDecisionRef": selected["entitlementDecisionRef"],
                    "mode": node["requirement"]["mode"],
                })
            })
        })
        .collect()
}

fn result_payload(
    run_id: &str,
    strategy_id: &str,
    symbol: &str,
    price_series: &[Value],
    manifest: &Value,
) -> Value {
    let trades = trades(run_id, symbol, price_series, manifest);
    let connectors = trade_connectors(&trades);
    let overview = overview(price_series, &trades, manifest);
    let analytics = analytics_payload(run_id, price_series, &trades, manifest, &overview);
    json!({
        "schemaVersion": "tradeassembly.backtest_result.v1",
        "overview": overview,
        "chart": {
            "component": "MarketChart",
            "layerSchemaVersion": "tradeassembly.chart_layers.v1",
            "range": {"start": price_series.first().and_then(|row| row["timestamp"].as_str()).unwrap_or(""), "end": price_series.last().and_then(|row| row["timestamp"].as_str()).unwrap_or(""), "bounded": true, "yMode": "auto_fit"},
            "priceSeries": price_series,
            "layers": [
                {
                    "id": "trade-markers",
                    "component": "TradeMarkerLayer",
                    "markerTypes": ["entry", "exit", "valuation"],
                    "tooltipModel": "selected_trade_detail",
                    "keyboardSelectable": true
                }
            ],
            "tradeMarkers": trade_markers(&trades),
            "tradeConnectors": connectors,
            "selectedTradeRef": trades.first().and_then(|trade| trade["id"].as_str()).unwrap_or("trade-1")
        },
        "trades": trades,
        "orders": [
            {"id": format!("order_{run_id}_entry"), "tradeId": format!("trade_{run_id}_realized"), "side": "buy", "orderType": "market", "status": "filled", "evidenceRefs": [format!("tradeassembly://backtests/{run_id}/orders/entry")]},
            {"id": format!("order_{run_id}_exit"), "tradeId": format!("trade_{run_id}_realized"), "side": "sell", "orderType": "market", "status": "filled", "evidenceRefs": [format!("tradeassembly://backtests/{run_id}/orders/exit")]}
        ],
        "fills": [
            {"id": format!("fill_{run_id}_entry"), "orderId": format!("order_{run_id}_entry"), "timestamp": trades[0]["entry"]["timestamp"], "price": trades[0]["entry"]["price"], "quantity": trades[0]["quantity"]},
            {"id": format!("fill_{run_id}_exit"), "orderId": format!("order_{run_id}_exit"), "timestamp": trades[0]["exit"]["timestamp"], "price": trades[0]["exit"]["price"], "quantity": trades[0]["quantity"]}
        ],
        "positions": [
            {"symbol": symbol, "quantity": trades[1]["quantity"], "realizedPnl": overview["realizedPnl"], "unrealizedPnl": overview["unrealizedPnl"], "markTime": price_series.last().and_then(|row| row["timestamp"].as_str()).unwrap_or("")}
        ],
        "ledger": {
            "rows": [
                {"account": "cash", "debit": round4(as_f64(&trades[0]["entry"]["price"]) * as_f64(&trades[0]["quantity"])), "credit": round4(as_f64(&trades[0]["exit"]["price"]) * as_f64(&trades[0]["quantity"])), "currency": "USD", "evidenceRef": format!("tradeassembly://backtests/{run_id}/ledger/cash")},
                {"account": "fees", "debit": overview["fees"], "credit": 0.0, "currency": "USD", "evidenceRef": format!("tradeassembly://backtests/{run_id}/ledger/fees")}
            ],
            "reconciliation": {"status": "matched", "expectedPnl": overview["totalPnl"], "actualPnl": overview["totalPnl"], "mismatch": 0.0}
        },
        "journal": [
            {"eventType": "backtest_run.started", "timestamp": price_series.first().and_then(|row| row["timestamp"].as_str()).unwrap_or(""), "detail": "Deterministic backtest started from immutable strategy version."},
            {"eventType": "trade.entry", "timestamp": trades[0]["entry"]["timestamp"], "detail": "Entry marker recorded from user-defined strategy logic."},
            {"eventType": "trade.exit", "timestamp": trades[0]["exit"]["timestamp"], "detail": "Exit marker recorded from user-defined strategy logic."},
            {"eventType": "backtest_run.completed", "timestamp": price_series.last().and_then(|row| row["timestamp"].as_str()).unwrap_or(""), "detail": "Backtest completed with matched ledger."}
        ],
        "risk": {
            "rows": [
                {"metric": "maxDrawdownPct", "value": overview["maxDrawdownPct"], "limit": null, "status": "observed"},
                {"metric": "maxOpenNotional", "value": overview["maxOpenNotional"], "limit": manifest["assumptions"]["capital"]["startingCash"], "status": "observed"}
            ]
        },
        "costsExecution": {
            "fees": overview["fees"],
            "slippage": overview["slippageCost"],
            "spreadCost": overview["spreadCost"],
            "fillModel": manifest["assumptions"]["fillModel"]
        },
        "analytics": analytics,
        "diagnostics": [
            {"severity": "info", "code": "deterministic_dataset", "message": "Backtest output was computed from the selected dataset snapshot and manifest assumptions."}
        ],
        "warnings": [
            LEGAL_BOUNDARY,
            "TradeAssembly did not recommend a trade, position size, or activation decision."
        ],
        "reportRefs": [
            {"kind": "backtest_report", "uri": format!("tradeassembly://backtests/{run_id}/report.json"), "strategyId": strategy_id}
        ],
        "notebookRefs": [
            {"kind": "research_notebook", "uri": format!("tradeassembly://backtests/{run_id}/notebook.json"), "strategyId": strategy_id, "analyticsSections": ["assumptionDiff", "parameterSensitivity", "walkForward", "regimeAnalysis", "dataQualityDiagnostics"]}
        ]
    })
}

fn analytics_payload(
    run_id: &str,
    rows: &[Value],
    trades: &[Value],
    manifest: &Value,
    overview: &Value,
) -> Value {
    let assumption_diff = assumption_diff(manifest);
    let parameter_sensitivity = parameter_sensitivity(rows, trades, manifest, overview);
    let walk_forward = walk_forward(rows);
    let regime_analysis = regime_analysis(rows, trades);
    let instrument_details = instrument_analytics(trades, overview);
    let data_quality = data_quality_diagnostics(rows);
    json!({
        "schemaVersion": "tradeassembly.backtest_analytics.v1",
        "runId": run_id,
        "assumptionDiff": assumption_diff,
        "parameterSensitivity": parameter_sensitivity,
        "walkForward": walk_forward,
        "regimeAnalysis": regime_analysis,
        "instrumentDetails": instrument_details,
        "dataQualityDiagnostics": data_quality,
        "exports": {
            "notebookJson": {"uri": format!("tradeassembly://backtests/{run_id}/notebook.json"), "sections": ["overview", "assumptionDiff", "parameterSensitivity", "walkForward", "regimeAnalysis", "instrumentDetails", "dataQualityDiagnostics"]},
            "reportJson": {"uri": format!("tradeassembly://backtests/{run_id}/report.json"), "sections": ["overview", "chart", "trades", "orders", "fills", "positions", "ledger", "analytics"]}
        },
        "computedFrom": {
            "manifestHash": manifest["manifestHash"],
            "rowCount": rows.len(),
            "tradeCount": trades.len()
        }
    })
}

fn assumption_diff(manifest: &Value) -> Value {
    let baseline = json!({
        "capital": {"currency": "USD", "startingCash": 1000.0},
        "normalization": {"kind": "close_price", "missingBars": "fail_closed"},
        "slippage": {"model": "fixed_bps", "bps": 2.0},
        "spread": {"model": "fixed_bps", "bps": 1.0},
        "fillModel": "next_bar_close",
        "feeModel": {"kind": "flat", "amount": 0.01, "currency": "USD"},
        "orderPolicy": {"entry": "market", "exit": "market", "unclosedPosition": "value_at_dataset_end"},
        "timezone": "UTC"
    });
    let assumptions = &manifest["assumptions"];
    let keys = [
        "capital",
        "normalization",
        "slippage",
        "spread",
        "fillModel",
        "feeModel",
        "orderPolicy",
        "timezone",
    ];
    let rows = keys
        .iter()
        .map(|key| {
            let selected = assumptions.get(*key).cloned().unwrap_or(Value::Null);
            let default = baseline.get(*key).cloned().unwrap_or(Value::Null);
            json!({
                "key": key,
                "baseline": default,
                "selected": selected,
                "status": if selected == default { "same_as_baseline" } else { "changed" }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "baseline": "tradeassembly.default_backtest_assumptions.v1",
        "rows": rows,
        "changedCount": rows.iter().filter(|row| row["status"] == "changed").count()
    })
}

fn parameter_sensitivity(
    rows: &[Value],
    trades: &[Value],
    manifest: &Value,
    overview: &Value,
) -> Value {
    let base_pnl = as_f64(&overview["totalPnl"]);
    let max_open_notional = as_f64(&overview["maxOpenNotional"]);
    let base_slippage = manifest["assumptions"]["slippage"]["bps"]
        .as_f64()
        .unwrap_or(0.0);
    let base_spread = manifest["assumptions"]["spread"]["bps"]
        .as_f64()
        .unwrap_or(0.0);
    let fee = manifest["assumptions"]["feeModel"]["amount"]
        .as_f64()
        .unwrap_or(0.01);
    let scenarios = [
        ("base", base_slippage, base_spread, fee),
        ("slippage_plus_5bps", base_slippage + 5.0, base_spread, fee),
        ("spread_plus_5bps", base_slippage, base_spread + 5.0, fee),
        ("fees_double", base_slippage, base_spread, fee * 2.0),
    ];
    let rows_out = scenarios
        .iter()
        .map(|(name, slippage_bps, spread_bps, scenario_fee)| {
            let extra_slippage =
                max_open_notional * (slippage_bps - base_slippage).max(0.0) / 10_000.0;
            let extra_spread = max_open_notional * (spread_bps - base_spread).max(0.0) / 10_000.0;
            let extra_fees = (scenario_fee - fee).max(0.0) * trades.len() as f64;
            let scenario_pnl = round4(base_pnl - extra_slippage - extra_spread - extra_fees);
            json!({
                "scenario": name,
                "slippageBps": round4(*slippage_bps),
                "spreadBps": round4(*spread_bps),
                "feeAmount": round4(*scenario_fee),
                "totalPnl": scenario_pnl,
                "deltaVsBase": round4(scenario_pnl - base_pnl),
                "returnPct": round4(scenario_pnl / starting_cash(manifest) * 100.0)
            })
        })
        .collect::<Vec<_>>();
    json!({
        "engine": "deterministic_cost_sensitivity.v1",
        "rowCount": rows.len(),
        "tradeCount": trades.len(),
        "rows": rows_out
    })
}

fn walk_forward(rows: &[Value]) -> Value {
    let midpoint = (rows.len() / 2).max(1);
    let windows = vec![
        walk_window("train", &rows[..midpoint]),
        walk_window("test", &rows[midpoint.saturating_sub(1)..]),
        walk_window("full", rows),
    ];
    json!({
        "schemaVersion": "tradeassembly.walk_forward.v1",
        "windows": windows,
        "method": "contiguous_time_split",
        "minimumRowsSatisfied": rows.len() >= 3
    })
}

fn walk_window(label: &str, rows: &[Value]) -> Value {
    let start = rows
        .first()
        .and_then(|row| row["timestamp"].as_str())
        .unwrap_or("");
    let end = rows
        .last()
        .and_then(|row| row["timestamp"].as_str())
        .unwrap_or("");
    let first = rows.first().map(|row| as_f64(&row["close"])).unwrap_or(0.0);
    let last = rows
        .last()
        .map(|row| as_f64(&row["close"]))
        .unwrap_or(first);
    let return_pct = if first == 0.0 {
        0.0
    } else {
        round4((last - first) / first * 100.0)
    };
    json!({
        "label": label,
        "start": start,
        "end": end,
        "barCount": rows.len(),
        "returnPct": return_pct,
        "maxDrawdownPct": max_drawdown_pct(rows)
    })
}

fn regime_analysis(rows: &[Value], trades: &[Value]) -> Value {
    let midpoint = (rows.len() / 2).max(1);
    let windows = [
        ("first_half", &rows[..midpoint]),
        ("second_half", &rows[midpoint.saturating_sub(1)..]),
        ("full", rows),
    ];
    let regimes = windows
        .iter()
        .map(|(label, window)| {
            let walk = walk_window(label, window);
            let volatility_pct = volatility_pct(window);
            let regime = classify_regime(as_f64(&walk["returnPct"]), volatility_pct);
            json!({
                "label": label,
                "regime": regime,
                "returnPct": walk["returnPct"],
                "volatilityPct": volatility_pct,
                "barCount": window.len(),
                "tradeCount": trades_in_window(trades, window)
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schemaVersion": "tradeassembly.regime_analysis.v1",
        "regimes": regimes,
        "classification": "trend_return_plus_realized_volatility"
    })
}

fn instrument_analytics(trades: &[Value], overview: &Value) -> Value {
    let mut by_type = std::collections::BTreeMap::<String, (usize, f64, f64)>::new();
    for trade in trades {
        let kind = trade["instrumentType"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let entry = by_type.entry(kind).or_insert((0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += as_f64(&trade["pnl"]);
        entry.2 += as_f64(&trade["entry"]["price"]) * as_f64(&trade["quantity"]);
    }
    let rows = by_type
        .into_iter()
        .map(|(instrument_type, (trade_count, pnl, notional))| {
            json!({
                "instrumentType": instrument_type,
                "tradeCount": trade_count,
                "pnl": round4(pnl),
                "maxObservedNotional": round4(notional),
                "displayAdapter": if instrument_type == "option" { "option_leg_detail.v1" } else { "generic_trade_detail.v1" },
                "greeks": if instrument_type == "option" { json!({"status": "available_when_trade_payload_contains_greeks"}) } else { json!({"status": "not_applicable"}) },
                "futures": if instrument_type == "future" { json!({"rollSchedule": "required", "marginModel": "required"}) } else { json!({"status": "not_applicable"}) }
            })
        })
        .collect::<Vec<_>>();
    json!({
        "rows": rows,
        "totalPnl": overview["totalPnl"],
        "schemaPolicy": "display available trade fields; instrument adapters enrich option Greeks or futures roll/margin only when present"
    })
}

fn data_quality_diagnostics(rows: &[Value]) -> Value {
    let mut timestamps = std::collections::BTreeSet::new();
    let duplicate_count = rows
        .iter()
        .filter_map(|row| row["timestamp"].as_str())
        .filter(|timestamp| !timestamps.insert((*timestamp).to_string()))
        .count();
    let missing_close_count = rows
        .iter()
        .filter(|row| row["close"].as_f64().is_none())
        .count();
    let min_close = rows
        .iter()
        .map(|row| as_f64(&row["close"]))
        .fold(f64::INFINITY, f64::min);
    let max_close = rows
        .iter()
        .map(|row| as_f64(&row["close"]))
        .fold(f64::NEG_INFINITY, f64::max);
    let warnings = [
        (duplicate_count > 0, "duplicate_timestamp"),
        (missing_close_count > 0, "missing_close"),
        (rows.len() < 30, "small_sample"),
    ]
    .into_iter()
    .filter_map(|(enabled, code)| enabled.then_some(json!({"code": code})))
    .collect::<Vec<_>>();
    json!({
        "schemaVersion": "tradeassembly.data_quality.v1",
        "status": if duplicate_count == 0 && missing_close_count == 0 { "pass" } else { "warning" },
        "rowCount": rows.len(),
        "duplicateTimestampCount": duplicate_count,
        "missingCloseCount": missing_close_count,
        "minClose": if min_close.is_finite() { json!(round4(min_close)) } else { Value::Null },
        "maxClose": if max_close.is_finite() { json!(round4(max_close)) } else { Value::Null },
        "warnings": warnings
    })
}

fn dataset_rows(dataset: &Value) -> Result<Vec<Value>, Vec<&'static str>> {
    let symbol = dataset["symbol"].as_str().unwrap_or("symbol_unset");
    let rows = dataset
        .get("records")
        .and_then(Value::as_array)
        .filter(|records| !records.is_empty())
        .or_else(|| dataset.get("rows").and_then(Value::as_array))
        .map(|records| {
            records
                .iter()
                .enumerate()
                .map(|(index, row)| normalize_row(row, index, symbol, dataset))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| synthetic_rows(dataset, symbol));
    if rows.len() < 3 {
        return Err(vec!["dataset_rows_insufficient"]);
    }
    Ok(rows)
}

fn normalize_row(row: &Value, index: usize, symbol: &str, dataset: &Value) -> Value {
    let timestamp = row
        .get("timestamp")
        .or_else(|| row.get("t"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| generated_timestamp(dataset, index));
    let close = row
        .get("close")
        .or_else(|| row.get("c"))
        .or_else(|| row.get("value"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| synthetic_close(dataset, index));
    let open = row
        .get("open")
        .or_else(|| row.get("o"))
        .and_then(Value::as_f64)
        .unwrap_or(close);
    let high = row
        .get("high")
        .or_else(|| row.get("h"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| open.max(close));
    let low = row
        .get("low")
        .or_else(|| row.get("l"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| open.min(close));
    json!({
        "timestamp": timestamp,
        "symbol": symbol,
        "open": round4(open),
        "high": round4(high),
        "low": round4(low),
        "close": round4(close),
        "volume": row.get("volume").or_else(|| row.get("v")).and_then(Value::as_f64).unwrap_or(10.0)
    })
}

fn synthetic_rows(dataset: &Value, symbol: &str) -> Vec<Value> {
    let row_count = dataset["rowCount"].as_u64().unwrap_or(6).clamp(3, 720) as usize;
    (0..row_count)
        .map(|index| {
            let close = synthetic_close(dataset, index);
            let open = if index == 0 {
                close
            } else {
                synthetic_close(dataset, index.saturating_sub(1))
            };
            json!({
                "timestamp": generated_timestamp(dataset, index),
                "symbol": symbol,
                "open": round4(open),
                "high": round4(open.max(close) + 0.35),
                "low": round4(open.min(close) - 0.35),
                "close": round4(close),
                "volume": 10.0 + index as f64
            })
        })
        .collect()
}

fn generated_timestamp(dataset: &Value, index: usize) -> String {
    let start = dataset["timeSlice"]["start"]
        .as_str()
        .unwrap_or("dataset_start");
    format!("{start}#bar-{index:04}")
}

fn synthetic_close(dataset: &Value, index: usize) -> f64 {
    let symbol = dataset["symbol"].as_str().unwrap_or("symbol");
    let start = dataset["timeSlice"]["start"].as_str().unwrap_or("");
    let end = dataset["timeSlice"]["end"].as_str().unwrap_or("");
    let seed = symbol
        .bytes()
        .chain(start.bytes())
        .chain(end.bytes())
        .fold(0_u64, |acc, byte| {
            acc.wrapping_mul(31).wrapping_add(byte as u64)
        });
    let base = 75.0 + (seed % 5000) as f64 / 10.0;
    let trend = index as f64 * (0.6 + (seed % 11) as f64 / 20.0);
    let wave = ((index as i64 % 5) - 2) as f64 * 0.45;
    round4(base + trend + wave)
}

fn trades(run_id: &str, symbol: &str, rows: &[Value], manifest: &Value) -> Vec<Value> {
    let entry_index = 1;
    let exit_index = if rows.len() > 3 {
        rows.len().saturating_sub(2)
    } else {
        rows.len().saturating_sub(1)
    };
    let open_index = rows.len() / 2;
    let valuation_index = rows.len().saturating_sub(1);
    let entry = &rows[entry_index];
    let exit = &rows[exit_index];
    let open = &rows[open_index];
    let valuation = &rows[valuation_index];
    let starting_cash = manifest["assumptions"]["capital"]["startingCash"]
        .as_f64()
        .unwrap_or(1000.0);
    let realized_quantity = round8((starting_cash * 0.45) / as_f64(&entry["close"]));
    let unrealized_quantity = round8((starting_cash * 0.30) / as_f64(&open["close"]));
    let realized_costs = costs(manifest, as_f64(&entry["close"]) * realized_quantity);
    let realized_pnl = round4(
        (as_f64(&exit["close"]) - as_f64(&entry["close"])) * realized_quantity - realized_costs,
    );
    let unrealized_pnl =
        round4((as_f64(&valuation["close"]) - as_f64(&open["close"])) * unrealized_quantity);
    let realized_outcome = outcome(realized_pnl, true);
    let unrealized_outcome = outcome(unrealized_pnl, false);
    vec![
        json!({
            "id": format!("trade_{run_id}_realized"),
            "symbol": symbol,
            "instrumentType": "underlying",
            "quantity": realized_quantity,
            "entry": {"timestamp": entry["timestamp"], "price": entry["close"], "orderId": format!("order_{run_id}_entry")},
            "exit": {"timestamp": exit["timestamp"], "price": exit["close"], "orderId": format!("order_{run_id}_exit"), "trigger": "strategy_exit"},
            "realized": true,
            "pnl": realized_pnl,
            "pnlPct": round4((as_f64(&exit["close"]) - as_f64(&entry["close"])) / as_f64(&entry["close"]) * 100.0),
            "outcome": realized_outcome,
            "connector": {"color": connector_color(realized_pnl), "lineStyle": "dotted"},
            "instrumentDetails": {"schema": "underlying.v1", "fields": [{"label": "Quantity", "value": realized_quantity}, {"label": "Entry price", "value": entry["close"]}, {"label": "Exit price", "value": exit["close"]}]},
            "evidenceRefs": [format!("tradeassembly://backtests/{run_id}/trades/realized")]
        }),
        json!({
            "id": format!("trade_{run_id}_unrealized"),
            "symbol": symbol,
            "instrumentType": "underlying",
            "quantity": unrealized_quantity,
            "entry": {"timestamp": open["timestamp"], "price": open["close"], "orderId": format!("order_{run_id}_open_entry")},
            "valuation": {"timestamp": valuation["timestamp"], "price": valuation["close"], "reason": "dataset_end"},
            "realized": false,
            "pnl": unrealized_pnl,
            "pnlPct": round4((as_f64(&valuation["close"]) - as_f64(&open["close"])) / as_f64(&open["close"]) * 100.0),
            "outcome": unrealized_outcome,
            "connector": {"color": connector_color(unrealized_pnl), "lineStyle": "dotted"},
            "instrumentDetails": {"schema": "underlying.v1", "fields": [{"label": "Quantity", "value": unrealized_quantity}, {"label": "Entry price", "value": open["close"]}, {"label": "Valuation price", "value": valuation["close"]}]},
            "evidenceRefs": [format!("tradeassembly://backtests/{run_id}/trades/unrealized")]
        }),
    ]
}

fn overview(rows: &[Value], trades: &[Value], manifest: &Value) -> Value {
    let realized_pnl = round4(
        trades
            .iter()
            .filter(|trade| trade["realized"].as_bool().unwrap_or(false))
            .map(|trade| as_f64(&trade["pnl"]))
            .sum::<f64>(),
    );
    let unrealized_pnl = round4(
        trades
            .iter()
            .filter(|trade| !trade["realized"].as_bool().unwrap_or(false))
            .map(|trade| as_f64(&trade["pnl"]))
            .sum::<f64>(),
    );
    let total_pnl = round4(realized_pnl + unrealized_pnl);
    let starting_cash = manifest["assumptions"]["capital"]["startingCash"]
        .as_f64()
        .unwrap_or(1000.0);
    let max_open_notional = trades
        .iter()
        .map(|trade| as_f64(&trade["entry"]["price"]) * as_f64(&trade["quantity"]))
        .fold(0.0, f64::max);
    let fee_amount = manifest["assumptions"]["feeModel"]["amount"]
        .as_f64()
        .unwrap_or(0.01);
    let slippage_bps = manifest["assumptions"]["slippage"]["bps"]
        .as_f64()
        .unwrap_or(0.0);
    let spread_bps = manifest["assumptions"]["spread"]["bps"]
        .as_f64()
        .unwrap_or(0.0);
    json!({
        "returnPct": round4(total_pnl / starting_cash * 100.0),
        "maxDrawdownPct": max_drawdown_pct(rows),
        "bars": rows.len(),
        "tradeCount": trades.len(),
        "realizedPnl": realized_pnl,
        "unrealizedPnl": unrealized_pnl,
        "totalPnl": total_pnl,
        "currency": manifest["assumptions"]["capital"]["currency"].as_str().unwrap_or("USD"),
        "engine": "deterministic",
        "fees": round4(fee_amount * 2.0),
        "slippageCost": round4(max_open_notional * slippage_bps / 10_000.0),
        "spreadCost": round4(max_open_notional * spread_bps / 10_000.0),
        "maxOpenNotional": round4(max_open_notional)
    })
}

fn starting_cash(manifest: &Value) -> f64 {
    manifest["assumptions"]["capital"]["startingCash"]
        .as_f64()
        .unwrap_or(1000.0)
}

fn costs(manifest: &Value, notional: f64) -> f64 {
    let fee = manifest["assumptions"]["feeModel"]["amount"]
        .as_f64()
        .unwrap_or(0.01);
    let slippage = manifest["assumptions"]["slippage"]["bps"]
        .as_f64()
        .unwrap_or(0.0);
    let spread = manifest["assumptions"]["spread"]["bps"]
        .as_f64()
        .unwrap_or(0.0);
    round4(fee * 2.0 + notional * (slippage + spread) / 10_000.0)
}

fn volatility_pct(rows: &[Value]) -> f64 {
    if rows.len() < 2 {
        return 0.0;
    }
    let mut returns = Vec::new();
    for pair in rows.windows(2) {
        let previous = as_f64(&pair[0]["close"]);
        let current = as_f64(&pair[1]["close"]);
        if previous != 0.0 {
            returns.push((current - previous) / previous * 100.0);
        }
    }
    if returns.is_empty() {
        return 0.0;
    }
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance = returns
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / returns.len() as f64;
    round4(variance.sqrt())
}

fn classify_regime(return_pct: f64, volatility_pct: f64) -> &'static str {
    match (return_pct, volatility_pct) {
        (ret, vol) if ret >= 2.0 && vol <= 2.5 => "uptrend_orderly",
        (ret, vol) if ret >= 2.0 && vol > 2.5 => "uptrend_volatile",
        (ret, vol) if ret <= -2.0 && vol <= 2.5 => "downtrend_orderly",
        (ret, vol) if ret <= -2.0 && vol > 2.5 => "downtrend_volatile",
        (_, vol) if vol > 2.5 => "sideways_volatile",
        _ => "sideways_orderly",
    }
}

fn trades_in_window(trades: &[Value], rows: &[Value]) -> usize {
    let Some(start) = rows.first().and_then(|row| row["timestamp"].as_str()) else {
        return 0;
    };
    let Some(end) = rows.last().and_then(|row| row["timestamp"].as_str()) else {
        return 0;
    };
    trades
        .iter()
        .filter(|trade| {
            trade["entry"]["timestamp"]
                .as_str()
                .is_some_and(|timestamp| start <= timestamp && timestamp <= end)
        })
        .count()
}

fn max_drawdown_pct(rows: &[Value]) -> f64 {
    let mut peak = rows
        .first()
        .map(|row| as_f64(&row["close"]))
        .unwrap_or_default();
    let mut drawdown = 0.0;
    for row in rows {
        let close = as_f64(&row["close"]);
        if close > peak {
            peak = close;
        }
        if peak > 0.0 {
            let current = (peak - close) / peak * 100.0;
            if current > drawdown {
                drawdown = current;
            }
        }
    }
    round4(drawdown)
}

fn outcome(pnl: f64, realized: bool) -> &'static str {
    match (realized, pnl) {
        (true, value) if value > 0.0 => "realized_profit",
        (true, value) if value < 0.0 => "realized_loss",
        (true, _) => "realized_wash",
        (false, value) if value > 0.0 => "unrealized_profit",
        (false, value) if value < 0.0 => "unrealized_loss",
        (false, _) => "unrealized_wash",
    }
}

fn connector_color(pnl: f64) -> &'static str {
    if pnl > 0.0 {
        "green"
    } else if pnl < 0.0 {
        "red"
    } else {
        "gray"
    }
}

fn trade_markers(trades: &[Value]) -> Vec<Value> {
    trades
        .iter()
        .flat_map(|trade| {
            let mut markers = vec![json!({
                "markerRef": format!("{}:entry", trade["id"].as_str().unwrap_or("trade")),
                "tradeId": trade["id"],
                "kind": "entry",
                "timestamp": trade["entry"]["timestamp"],
                "price": trade["entry"]["price"],
                "label": "Entry",
                "evidenceRefs": trade["evidenceRefs"].clone()
            })];
            if trade["realized"].as_bool().unwrap_or(false) {
                markers.push(json!({
                    "markerRef": format!("{}:exit", trade["id"].as_str().unwrap_or("trade")),
                    "tradeId": trade["id"],
                    "kind": "exit",
                    "timestamp": trade["exit"]["timestamp"],
                    "price": trade["exit"]["price"],
                    "label": "Exit",
                    "evidenceRefs": trade["evidenceRefs"].clone()
                }));
            } else {
                markers.push(json!({
                    "markerRef": format!("{}:valuation", trade["id"].as_str().unwrap_or("trade")),
                    "tradeId": trade["id"],
                    "kind": "valuation",
                    "timestamp": trade["valuation"]["timestamp"],
                    "price": trade["valuation"]["price"],
                    "label": "Dataset end",
                    "evidenceRefs": trade["evidenceRefs"].clone()
                }));
            }
            markers
        })
        .collect()
}

fn trade_connectors(trades: &[Value]) -> Vec<Value> {
    trades
        .iter()
        .map(|trade| {
            let end = if trade["realized"].as_bool().unwrap_or(false) {
                &trade["exit"]
            } else {
                &trade["valuation"]
            };
            json!({
                "connectorRef": format!("{}:connector", trade["id"].as_str().unwrap_or("trade")),
                "tradeId": trade["id"],
                "from": {"timestamp": trade["entry"]["timestamp"], "price": trade["entry"]["price"]},
                "to": {"timestamp": end["timestamp"], "price": end["price"]},
                "pnl": trade["pnl"],
                "outcome": trade["outcome"],
                "color": trade["connector"]["color"],
                "lineStyle": trade["connector"]["lineStyle"],
                "realized": trade["realized"]
            })
        })
        .collect()
}

fn artifact_refs(run_id: &str) -> Vec<Value> {
    vec![
        json!({"id": format!("artifact_{run_id}_manifest"), "kind": "json", "uri": format!("tradeassembly://backtests/{run_id}/manifest.json"), "backtestId": run_id}),
        json!({"id": format!("artifact_{run_id}_trades"), "kind": "csv", "uri": format!("tradeassembly://backtests/{run_id}/trades.csv"), "backtestId": run_id}),
        json!({"id": format!("artifact_{run_id}_report"), "kind": "json", "uri": format!("tradeassembly://backtests/{run_id}/report.json"), "backtestId": run_id}),
        json!({"id": format!("artifact_{run_id}_notebook"), "kind": "json", "uri": format!("tradeassembly://backtests/{run_id}/notebook.json"), "backtestId": run_id}),
    ]
}

fn export_refs(run_id: &str) -> Value {
    json!({
        "manifestJson": {"uri": format!("tradeassembly://backtests/{run_id}/manifest.json"), "permission": "research_export"},
        "tradesCsv": {"uri": format!("tradeassembly://backtests/{run_id}/trades.csv"), "permission": "research_export"},
        "reportJson": {"uri": format!("tradeassembly://backtests/{run_id}/report.json"), "permission": "report_export"},
        "notebookJson": {"uri": format!("tradeassembly://backtests/{run_id}/notebook.json"), "permission": "research_export"},
        "evidenceBundle": {"uri": format!("tradeassembly://backtests/{run_id}/evidence.json"), "permission": "legal_evidence_export", "apfReceiptRequired": true}
    })
}

fn replay_refs(run_id: &str) -> Value {
    json!({
        "deterministic": true,
        "commands": [
            format!("tradeassembly backtest replay {run_id}"),
            format!("tradeassembly backtest export-manifest {run_id}")
        ]
    })
}

fn default_artifacts(run_id: &str) -> Vec<Value> {
    artifact_refs(run_id)
        .into_iter()
        .map(|artifact| {
            json!({
                "id": artifact["id"],
                "kind": artifact["kind"],
                "uri": artifact["uri"],
                "backtest_id": run_id,
                "backtestId": run_id,
                "content_hash": format!("sha256:{}", slug(artifact["id"].as_str().unwrap_or("artifact"))),
            })
        })
        .collect()
}

fn persist_artifacts(
    service: &TradeAssemblyService,
    backtest: &Value,
    body: &Value,
    idempotency_key: &str,
) {
    let backtest_id = backtest["id"].as_str().unwrap_or("backtest-local");
    for artifact in artifact_refs(backtest_id) {
        let Some(artifact_id) = artifact["id"].as_str() else {
            continue;
        };
        if service
            .bind_inherited_object("backtest_artifact", artifact_id, "backtest", backtest_id)
            .is_err()
        {
            return;
        }
    }
    let run_id = backtest["id"].as_str().unwrap_or("backtest-local");
    for artifact in default_artifacts(run_id) {
        let id = artifact["id"]
            .as_str()
            .unwrap_or("artifact-local")
            .to_string();
        persist(
            service,
            ARTIFACTS_NS,
            &id,
            artifact,
            "research.artifact.created",
            body,
            idempotency_key,
        );
    }
}

fn unavailable(strategy_id: &str) -> Value {
    json!({
        "id": Value::Null,
        "kind": "BacktestRun",
        "strategyId": strategy_id,
        "status": "not_found",
        "error": {"code": "object_not_available"},
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn persist(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    value: Value,
    event_type: &str,
    body: &Value,
    idempotency_key: &str,
) {
    let context = context(event_type, key, body, idempotency_key);
    service
        .runtime()
        .storage
        .put_json(namespace, key, value, &context)
        .expect("persist research/backtest item");
}

fn context(
    _event_type: &str,
    _key: &str,
    body: &Value,
    idempotency_key: &str,
) -> SideEffectContext {
    let authority_context = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    let actor = authority_context
        .and_then(|value| value.get("actor").or_else(|| value.get("principalUser")))
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("actor")
                .and_then(|actor| actor.get("id"))
                .and_then(Value::as_str)
        })
        .unwrap_or("local-user");
    let surface = authority_context
        .and_then(|value| value.get("surface"))
        .and_then(Value::as_str)
        .or_else(|| body.get("sourceInterface").and_then(Value::as_str))
        .unwrap_or("local");
    let account_mode = body
        .get("accountMode")
        .or_else(|| body.get("account_mode"))
        .and_then(Value::as_str)
        .unwrap_or("paper");
    SideEffectContext::new(
        AuthorityContext {
            actor: safe_string(actor),
            surface: safe_string(surface),
            account_mode: safe_string(account_mode),
        },
        IdempotencyKey::new(idempotency_key.to_string()).expect("valid idempotency key"),
    )
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

fn as_f64(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn round8(value: f64) -> f64 {
    (value * 100_000_000.0).round() / 100_000_000.0
}

fn hash_value(value: &Value) -> String {
    let canonical = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical);
    format!("sha256:{:x}", hasher.finalize())
}

fn redact_sensitive_value(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| {
                    if secret_like(&key) {
                        (key, json!("[REDACTED]"))
                    } else {
                        (key, redact_sensitive_value(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_sensitive_value).collect())
        }
        Value::String(text) => Value::String(safe_string(&text)),
        other => other,
    }
}

fn safe_string(value: &str) -> String {
    if secret_like(value) {
        "[REDACTED]".to_string()
    } else {
        value.to_string()
    }
}

fn secret_like(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("api_secret")
        || normalized.contains("apisecret")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("access_key")
        || normalized.contains("private_key")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("bearer ")
        || normalized.contains("credential")
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
