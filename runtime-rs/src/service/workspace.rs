// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{
    capability_graph, execution_body, journal_events, performance_summary, providers,
    TradeAssemblyService, LEGAL_BOUNDARY,
};
use serde_json::{json, Value};

pub(crate) fn workspace(service: &TradeAssemblyService) -> Value {
    let backtests = service
        .runtime()
        .backtests
        .list_runs()
        .unwrap_or_default()
        .into_iter()
        .map(|run| {
            json!({
                "id": run.run_id,
                "status": format!("{:?}", run.state).to_lowercase(),
                "resultHash": run.result_hash,
                "createdAtMs": run.created_at_ms,
            })
        })
        .collect::<Vec<_>>();
    let credential_status = providers::plugin_catalog(service)
        .into_iter()
        .map(|plugin| {
            (
                plugin.instance_ref,
                serde_json::to_value(plugin.credential_status).unwrap_or_else(|_| json!({})),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "workspace": {"id": "local", "name": "Local Workspace", "mode": "local"},
        "id": "local",
        "name": "Local Workspace",
        "mode": "local",
        "strategies": service.strategies(),
        "providers": service.providers(),
        "journalEvents": journal_events(),
        "journal_events": journal_events(),
        "researchMetrics": {"datasetRows": 6, "returnPct": 0.24, "maxDrawdownPct": 0.12, "replayable": true},
        "marketBars": local_market_bars(),
        "credentialStatus": credential_status,
        "entitlements": {},
        "backtests": backtests,
        "researchJobs": super::research::list_jobs(service),
        "researchSweeps": super::research::list_sweeps(service),
        "researchUniverses": super::research::list_universes(service),
        "researchArtifacts": super::backtest::list_artifacts(service),
        "selectorRuns": local_selector_runs("strat_local_btc_demo"),
        "shareSnapshots": [],
        "runs": [],
        "orders": super::orders::list_orders(service),
        "orderAttempts": super::orders::list_attempts(service),
        "positions": super::positions::list(service),
        "performanceSummary": performance_summary(),
        "execution": execution_body(),
        "risk": super::risk::status(service),
        "scheduler": super::scheduler::status(service),
        "legalBoundary": LEGAL_BOUNDARY,
    })
}

pub(crate) fn local_market_bars() -> Value {
    json!([
        {"ts": "2026-01-02T00:00:00Z", "open": 42965.0, "high": 43042.0, "low": 42927.0, "close": 43000.0, "volume": 10, "symbol": "BTC/USD"},
        {"ts": "2026-01-02T00:01:00Z", "open": 43000.0, "high": 43167.0, "low": 42962.0, "close": 43125.0, "volume": 11, "symbol": "BTC/USD"},
        {"ts": "2026-01-02T00:02:00Z", "open": 43125.0, "high": 43167.0, "low": 43042.0, "close": 43080.0, "volume": 12, "symbol": "BTC/USD"},
        {"ts": "2026-01-02T00:03:00Z", "open": 43080.0, "high": 43202.0, "low": 43042.0, "close": 43160.0, "volume": 13, "symbol": "BTC/USD"},
        {"ts": "2026-01-02T00:04:00Z", "open": 43160.0, "high": 43262.0, "low": 43122.0, "close": 43220.0, "volume": 14, "symbol": "BTC/USD"},
        {"ts": "2026-01-02T00:05:00Z", "open": 43220.0, "high": 43262.0, "low": 43152.0, "close": 43190.0, "volume": 15, "symbol": "BTC/USD"},
    ])
}

pub(crate) fn local_selector_runs(strategy_id: &str) -> Value {
    json!([{
        "run_id": "selector_run_demo",
        "strategy_id": strategy_id,
        "accepted_candidates": [{
            "candidate_id": "candidate_spy_520p",
            "status": "accepted",
            "contract": {"symbol": "SPY 2026-07-17 520P", "dte": 34, "delta": -0.34, "bid": 4.1, "ask": 4.2, "mid": 4.15},
            "reason_codes": [],
        }],
        "rejected_candidates": [{
            "candidate_id": "candidate_spy_500p",
            "status": "rejected",
            "contract": {"symbol": "SPY 2026-07-17 500P", "dte": 34, "delta": -0.12, "bid": 1.14, "ask": 1.22, "mid": 1.18},
            "reason_codes": ["outside_delta"],
        }],
        "reasonCodeSummary": {"outside_delta": 1},
        "warnings": ["Rejected candidates fail closed; TradeAssembly did not substitute another contract."],
        "replay_refs": {"snapshotRef": "snapshot://local/selector-demo"},
        "export_refs": {"csv": {"path": "tradeassembly://selector-runs/selector_run_demo/candidates.csv"}},
    }])
}

pub(crate) fn builder_state(service: &TradeAssemblyService, strategy_id: &str) -> Value {
    let latest_version = super::strategy::latest_version(service, strategy_id);
    let draft = super::strategy::draft_value(service, strategy_id);
    let validation = super::strategy::validate_builder_draft(
        service,
        json!({"strategyId": strategy_id, "spec": draft["spec"].clone()}),
    );
    let capability_bindings = draft
        .get("capabilityBindings")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let capability_resolutions = capability_resolutions(service, strategy_id, &draft["spec"]);
    let capability_graph_resolution =
        capability_graph_resolution(service, strategy_id, &draft["spec"], &capability_bindings);
    json!({
        "strategy": service.strategy_value(strategy_id),
        "latestVersion": latest_version,
        "currentVersion": latest_version,
        "versionHistory": super::strategy::strategy_versions(service, strategy_id),
        "draft": draft,
        "capabilityBindings": capability_bindings,
        "editableSpec": draft["spec"].clone(),
        "semanticSections": super::strategy::semantic_sections(strategy_id, &draft),
        "visualEditor": {},
        "validation": validation["body"]["validation"].clone(),
        "publishReadiness": {"ready": validation["body"]["validation"]["ok"].as_bool().unwrap_or(false), "blockedReasons": validation["body"]["validation"]["readiness"]["blockedReasons"].clone()},
        "capabilityResolutions": capability_resolutions,
        "capabilityGraphResolution": capability_graph_resolution,
        "executionConfig": null,
        "activationReadiness": {"ready": false, "blockedReasons": ["execution_config_required", "human_acknowledgement_required"]},
        "providers": service.providers(),
        "acknowledgements": [],
        "aiSessions": super::strategy::strategy_proposals(service, strategy_id),
        "proposals": super::strategy::strategy_proposals(service, strategy_id),
    })
}

fn capability_graph_resolution(
    service: &TradeAssemblyService,
    strategy_id: &str,
    spec: &Value,
    bindings: &Value,
) -> Value {
    let requirements = [
        ("required", &spec["capability_requirements"]["required"]),
        ("optional", &spec["capability_requirements"]["optional"]),
    ]
    .into_iter()
    .flat_map(|(declaration, requirements)| {
        requirements
            .as_array()
            .into_iter()
            .flatten()
            .map(move |requirement| {
                let mut requirement = requirement.clone();
                requirement["declaration"] = json!(declaration);
                requirement
            })
    })
    .collect::<Vec<_>>();

    capability_graph::resolve(
        service,
        json!({
            "strategyId": strategy_id,
            "mode": "paper",
            "requirements": requirements,
            "bindings": bindings,
        }),
    )
}

fn capability_resolutions(
    service: &TradeAssemblyService,
    strategy_id: &str,
    spec: &Value,
) -> Vec<Value> {
    let asset_class = spec["signal_market_requirements"]
        .as_array()
        .and_then(|requirements| requirements.first())
        .and_then(|requirement| requirement["asset_class"].as_str());
    let signal_instrument_family = spec["signal_market_requirements"]
        .as_array()
        .and_then(|requirements| requirements.first())
        .and_then(|requirement| requirement["instrument_family"].as_str());

    [
        ("required", &spec["capability_requirements"]["required"]),
        ("optional", &spec["capability_requirements"]["optional"]),
    ]
    .into_iter()
    .flat_map(|(declaration, requirements)| {
        requirements
            .as_array()
            .into_iter()
            .flatten()
            .map(move |requirement| (declaration, requirement))
    })
    .map(|(declaration, requirement)| {
        let mut request = requirement.clone();
        if let Some(map) = request.as_object_mut() {
            map.insert("strategyId".to_string(), json!(strategy_id));
            map.insert("mode".to_string(), json!("paper"));
            if let Some(asset_class) = asset_class {
                map.insert("assetClass".to_string(), json!(asset_class));
            }
            let instrument_family = requirement["constraints"]["instrument_families"]
                .as_array()
                .and_then(|families| families.first())
                .and_then(Value::as_str)
                .or(signal_instrument_family);
            if let Some(instrument_family) = instrument_family {
                map.insert("instrumentFamily".to_string(), json!(instrument_family));
            }
        }
        let mut resolution =
            providers::resolve_capability(service, providers::requirement_from_body(&request));
        if let Some(map) = resolution.as_object_mut() {
            map.insert("declaration".to_string(), json!(declaration));
        }
        resolution
    })
    .collect()
}
