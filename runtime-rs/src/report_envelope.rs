// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpJsonResponse {
    pub status_code: u16,
    pub body: Value,
}

pub fn report_envelope(
    kind: &str,
    strategy_id: &str,
    backtest_id: &str,
    studio_base_url: &str,
) -> Value {
    let strategy_href = format!("{studio_base_url}/app/strategies/{strategy_id}");
    let backtest_href = format!("{strategy_href}/research?backtestId={backtest_id}");
    json!({
        "schemaVersion": "tradeassembly.report.v1",
        "bundleSchema": {
            "families": ["backtest", "monte_carlo", "paper", "live", "journal", "replay"]
        },
        "kind": kind,
        "strategyId": strategy_id,
        "summary": {
            "status": "completed",
            "primaryMetric": {"name": "return_pct", "value": 0.0}
        },
        "renderContract": {
            "preferred": "tradeassembly_ui_deep_links",
            "userVisible": ["summary", "deepLinks", "views", "warnings"],
            "agentVisible": ["artifactBundle", "artifactRefs", "exportRefs", "replayRefs", "raw"],
            "fallbackOrder": [
                {"capability": "side_by_side_tradeassembly_ui"},
                {"capability": "single_page_html_artifact"},
                {"capability": "static_svg_preview"},
                {"capability": "cli_replay_commands"}
            ]
        },
        "deepLinks": {
            "strategy": {"href": strategy_href},
            "research": {"href": format!("{studio_base_url}/app/strategies/{strategy_id}/research")},
            "backtest": {"href": backtest_href}
        },
        "renderTargets": {
            "tradeassembly": {"href": backtest_href},
            "htmlArtifact": {"inlineRef": "previews.html"},
            "staticSvg": {"inlineRef": "previews.svg"}
        },
        "previews": {
            "html": {"content": "<!doctype html><html><body>TradeAssembly report</body></html>"},
            "svg": {"content": "<svg viewBox=\"0 0 10 10\"></svg>"}
        },
        "charts": [
            {"id": "underlying_with_markers", "formats": ["interactive", "html", "svg"], "data": {"priceSeries": [{"t": "2026-01-01", "close": 1.0}]}},
            {"id": "equity_curve", "formats": ["interactive", "html", "svg"], "data": {"points": [{"t": "2026-01-01", "equity": 1000.0}]}},
            {"id": "drawdown", "formats": ["interactive", "html", "svg"], "data": {"points": []}},
            {"id": "trade_journal", "formats": ["interactive", "html", "svg"], "data": {"rows": []}},
            {"id": "monte_carlo", "formats": ["interactive", "html", "svg"], "data": {"paths": []}},
            {"id": "scenario_matrix", "formats": ["interactive", "html", "svg"], "data": {"rows": [{"scenario": "Baseline"}]}}
        ],
        "journal": {"rows": []},
        "exportRefs": {
            "tradeJournalCsv": {"uri": format!("tradeassembly://artifact/{backtest_id}/trade-journal.csv")}
        },
        "artifactRefs": [
            {"id": "artifact-report", "uri": format!("tradeassembly://artifact/{backtest_id}/report"), "backtestId": backtest_id}
        ],
        "artifactBundle": {
            "refs": [{"id": "artifact-report"}]
        },
        "replayRefs": {
            "commands": [
                format!("tradeassembly report-envelope --strategy-id {strategy_id} --backtest-id {backtest_id}"),
                format!("tradeassembly research-artifacts --strategy-id {strategy_id} --backtest-id {backtest_id}")
            ]
        },
        "raw": {
            "backtest": {"id": backtest_id}
        },
        "warnings": [
            "Descriptive research output only. TradeAssembly did not recommend a trade, position size, or activation decision."
        ]
    })
}

pub fn report_envelope_http(kind: &str, strategy_id: &str, backtest_id: &str) -> HttpJsonResponse {
    HttpJsonResponse {
        status_code: 200,
        body: report_envelope(kind, strategy_id, backtest_id, "http://127.0.0.1:3001"),
    }
}

pub fn report_envelope_cli(strategy_id: &str, backtest_id: &str) -> Value {
    report_envelope(
        "backtest",
        strategy_id,
        backtest_id,
        "http://127.0.0.1:3001",
    )
}

pub fn report_envelope_mcp(strategy_id: &str, backtest_id: &str) -> Value {
    report_envelope(
        "backtest",
        strategy_id,
        backtest_id,
        "http://127.0.0.1:3001",
    )
}

pub fn selector_report_envelope(
    selector_run: &Value,
    strategy_id: &str,
    studio_base_url: &str,
    db_path: &str,
) -> Value {
    let payload = canonical_selector_run(selector_run);
    let run_id = str_at(&payload, &["run_id"]);
    let selector_id = str_at(&payload, &["selector", "selector_id"]);
    let links = selector_deep_links(studio_base_url, strategy_id, &run_id);
    let charts = selector_charts(&links, &payload);
    let summary = selector_summary(strategy_id, &payload);
    let replay_refs = selector_replay_refs(db_path, strategy_id, &payload);
    let artifact_refs = selector_artifact_refs(&payload);
    json!({
        "schemaVersion": "tradeassembly.report.v1",
        "bundleSchema": {
            "version": "tradeassembly.report.v1",
            "kind": "selector",
            "families": ["selector", "market_snapshot", "candidate_filter", "journal", "replay", "review"],
            "requiredSections": [
                "summary",
                "deepLinks",
                "views",
                "renderTargets",
                "charts",
                "journal",
                "artifactRefs",
                "exportRefs",
                "assumptions",
                "skipReasons",
                "warnings",
                "replayRefs"
            ]
        },
        "kind": "selector",
        "strategyId": strategy_id,
        "selectorId": selector_id,
        "selectorRunId": run_id,
        "summary": summary,
        "deepLinks": links,
        "views": selector_views(&links),
        "renderContract": render_contract(),
        "renderTargets": {
            "tradeassembly": {
                "kind": "interactive_ui",
                "label": "Open selector run in TradeAssembly",
                "href": links["selector"]["href"],
                "uri": links["selector"]["uri"],
                "sideBySide": true
            },
            "htmlArtifact": {
                "kind": "single_page_html",
                "inlineRef": "previews.html",
                "downloadName": "tradeassembly-selector-report.html"
            },
            "staticSvg": {
                "kind": "static_svg",
                "inlineRef": "previews.svg",
                "downloadName": "tradeassembly-selector-report.svg"
            },
            "cli": {"kind": "commands", "commands": replay_refs["commands"]}
        },
        "previews": selector_previews(&summary, &charts, &links),
        "charts": charts,
        "journal": {
            "rows": [],
            "totals": {
                "accepted": array_len(&payload, "accepted_candidates"),
                "rejected": array_len(&payload, "rejected_candidates")
            }
        },
        "artifactRefs": artifact_refs,
        "artifactBundle": {
            "id": format!("bundle_selector_{run_id}"),
            "kind": "selector",
            "strategyId": strategy_id,
            "selectorRunId": run_id,
            "refs": artifact_refs,
            "agentUse": "Inspect accepted/rejected candidate evidence, reason codes, replay refs, and export refs when the user asks for detail."
        },
        "exportRefs": selector_export_refs(db_path, strategy_id, &payload),
        "assumptions": payload.get("provider_data_assumptions").cloned().unwrap_or_else(|| json!([])),
        "skipReasons": selector_skip_reasons(&payload),
        "warnings": [no_advice_selector_notice()],
        "replayRefs": replay_refs,
        "raw": {"selectorRun": payload}
    })
}

pub fn selector_report_envelope_http(selector_run: &Value, strategy_id: &str) -> HttpJsonResponse {
    HttpJsonResponse {
        status_code: 200,
        body: selector_report_envelope(
            selector_run,
            strategy_id,
            "http://127.0.0.1:3001",
            ".tradeassembly/tradeassembly.db",
        ),
    }
}

pub fn selector_report_envelope_cli(selector_run: &Value, strategy_id: &str) -> Value {
    selector_report_envelope(
        selector_run,
        strategy_id,
        "http://127.0.0.1:3001",
        ".tradeassembly/tradeassembly.db",
    )
}

pub fn selector_report_envelope_mcp(selector_run: &Value, strategy_id: &str) -> Value {
    selector_report_envelope_cli(selector_run, strategy_id)
}

pub fn selector_report_envelope_acp(selector_run: &Value, strategy_id: &str) -> Value {
    selector_report_envelope_cli(selector_run, strategy_id)
}

pub fn monte_carlo_surface(input: &Value, _studio_base_url: &str, _db_path: &str) -> Value {
    json!({
        "schemaVersion": "tradeassembly.monte_carlo.surface.v1",
        "ok": false,
        "kind": "monte_carlo",
        "strategyId": str_at(input, &["strategyId"]),
        "simulationResult": {},
        "warnings": ["A durable robustness run bound to an exact completed backtest is required."],
        "error": {
            "code": "monte_carlo_requires_durable_robustness_run",
            "message": "Create or read a durable robustness run; request-shaped simulation output is not available."
        },
        "notice": monte_carlo_no_advice_notice()
    })
}

pub fn monte_carlo_report_envelope(
    monte_carlo_surface: &Value,
    strategy_id: &str,
    studio_base_url: &str,
    db_path: &str,
) -> Value {
    let assumption_hash = str_at(monte_carlo_surface, &["assumptionHash"]);
    let links = monte_carlo_deep_links(studio_base_url, strategy_id, &assumption_hash);
    let simulation_result = monte_carlo_surface
        .get("simulationResult")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let charts = json!([
        {
            "id": "monte_carlo_terminal_distribution",
            "title": "Terminal distribution",
            "href": links["monteCarlo"]["href"],
            "type": "distribution",
            "formats": ["interactive", "html", "svg", "csv", "json"],
            "data": simulation_result.get("terminalDistribution").cloned().unwrap_or_else(|| json!({}))
        },
        {
            "id": "monte_carlo_drawdown_bands",
            "title": "Drawdown distribution",
            "href": links["monteCarlo"]["href"],
            "type": "drawdown_distribution",
            "formats": ["interactive", "html", "svg", "csv", "json"],
            "data": simulation_result.get("drawdownDistribution").cloned().unwrap_or_else(|| json!({}))
        },
        {
            "id": "monte_carlo_confidence_bands",
            "title": "Confidence bands",
            "href": links["monteCarlo"]["href"],
            "type": "confidence_bands",
            "formats": ["interactive", "html", "svg", "json"],
            "data": simulation_result.get("confidenceBands").cloned().unwrap_or_else(|| json!([]))
        },
        {
            "id": "monte_carlo_assumption_ledger",
            "title": "Assumption ledger",
            "href": links["monteCarlo"]["href"],
            "type": "assumption_ledger",
            "formats": ["interactive", "html", "json"],
            "data": monte_carlo_surface.get("assumptionLedger").cloned().unwrap_or_else(|| json!({}))
        }
    ]);
    let replay_refs = monte_carlo_replay_refs(db_path, strategy_id, &assumption_hash);
    let artifact_refs = json!([
        {"id": "monte-carlo-result", "kind": "monte_carlo_result", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/result.json")},
        {"id": "monte-carlo-assumptions", "kind": "assumption_ledger", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/assumptions.json")},
        {"id": "monte-carlo-confidence-bands", "kind": "confidence_bands", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/confidence-bands.json")}
    ]);
    json!({
        "schemaVersion": "tradeassembly.report.v1",
        "bundleSchema": {
            "version": "tradeassembly.report.v1",
            "kind": "monte_carlo",
            "families": ["monte_carlo", "assumptions", "distribution", "drawdown", "journal", "replay", "review"],
            "requiredSections": ["summary", "deepLinks", "views", "charts", "assumptions", "warnings", "modelProvenance", "exportRefs", "replayRefs"]
        },
        "kind": "monte_carlo",
        "strategyId": strategy_id,
        "assumptionHash": assumption_hash,
        "summary": {
            "title": format!("Monte Carlo research for {strategy_id}"),
            "status": if monte_carlo_surface["ok"].as_bool().unwrap_or(false) { "available" } else { "blocked" },
            "primaryMetric": {"name": "path_count", "value": monte_carlo_surface["assumptionLedger"]["pathCount"].clone()},
            "interpretation": "Monte Carlo output is descriptive, replayable research evidence for user review in TradeAssembly Studio."
        },
        "deepLinks": links,
        "views": monte_carlo_views(&links),
        "renderContract": render_contract(),
        "renderTargets": {
            "tradeassembly": {
                "kind": "interactive_ui",
                "label": "Open Monte Carlo research in TradeAssembly",
                "href": links["monteCarlo"]["href"],
                "uri": links["monteCarlo"]["uri"],
                "sideBySide": true
            },
            "htmlArtifact": {"kind": "single_page_html", "inlineRef": "previews.html", "downloadName": "tradeassembly-monte-carlo.html"},
            "staticSvg": {"kind": "static_svg", "inlineRef": "previews.svg", "downloadName": "tradeassembly-monte-carlo.svg"},
            "cli": {"kind": "commands", "commands": replay_refs["commands"]}
        },
        "previews": monte_carlo_previews(monte_carlo_surface, &links),
        "charts": charts,
        "assumptions": monte_carlo_surface.get("assumptionLedger").cloned().unwrap_or_else(|| json!({})),
        "warnings": monte_carlo_surface.get("warnings").cloned().unwrap_or_else(|| json!([])),
        "modelProvenance": monte_carlo_surface.get("modelProvenance").cloned().unwrap_or_else(|| json!({})),
        "artifactRefs": artifact_refs,
        "artifactBundle": {
            "id": format!("bundle_monte_carlo_{}", assumption_hash.replace("sha256:", "")),
            "kind": "monte_carlo",
            "strategyId": strategy_id,
            "assumptionHash": assumption_hash,
            "refs": artifact_refs,
            "agentUse": "Inspect distribution summaries, drawdown bands, confidence bands, assumption ledger, warnings, provenance, replay refs, and export refs when the user asks for detail."
        },
        "exportRefs": monte_carlo_surface.get("exportRefs").cloned().unwrap_or_else(|| json!({})),
        "replayRefs": replay_refs,
        "journalEvidence": monte_carlo_surface.get("journalEvidence").cloned().unwrap_or_else(|| json!([])),
        "raw": {"monteCarlo": monte_carlo_surface}
    })
}

pub fn portfolio_risk_surface(
    snapshot: Value,
    strategy_id: &str,
    studio_base_url: &str,
    db_path: &str,
) -> Value {
    let scope_id = str_at(&snapshot, &["scope", "id"]);
    let strategy_id = if strategy_id.is_empty() {
        scope_id.as_str()
    } else {
        strategy_id
    };
    let snapshot_id = str_at(&snapshot, &["snapshotId"]);
    let links = portfolio_risk_deep_links(studio_base_url, strategy_id, &snapshot_id);
    let mut surface = json!({
        "schemaVersion": "tradeassembly.portfolio_risk.surface.v1",
        "ok": snapshot["ok"].as_bool().unwrap_or(false),
        "kind": "portfolio_risk",
        "strategyId": strategy_id,
        "snapshotId": snapshot_id,
        "snapshotHash": snapshot["snapshotHash"].clone(),
        "scope": snapshot["scope"].clone(),
        "exposureTables": snapshot["exposureTables"].clone(),
        "concentrationWarnings": snapshot["concentrationWarnings"].clone(),
        "missingDataWarnings": snapshot["missingDataWarnings"].clone(),
        "thresholdComparisons": threshold_comparisons(&snapshot),
        "scenarioRefs": snapshot["scenarioRefs"].clone(),
        "staleDataPolicy": snapshot["staleDataPolicy"].clone(),
        "modelProvenance": snapshot["modelProvenance"].clone(),
        "journalEvidence": snapshot["journalEvidence"].clone(),
        "exportRefs": portfolio_risk_export_refs(strategy_id, &snapshot_id),
        "replayRefs": portfolio_risk_replay_refs(db_path, strategy_id, &snapshot_id),
        "agentSummary": {
            "scope": snapshot["scope"].clone(),
            "inputs": {
                "exposureTableCount": snapshot["exposureTables"].as_array().map(Vec::len).unwrap_or(0),
                "warningCount": snapshot["missingDataWarnings"].as_array().map(Vec::len).unwrap_or(0),
                "concentrationWarningCount": snapshot["concentrationWarnings"].as_array().map(Vec::len).unwrap_or(0)
            },
            "thresholds": threshold_comparisons(&snapshot),
            "warnings": snapshot["missingDataWarnings"].clone(),
            "studioDeepLink": links["portfolioRisk"]["href"]
        },
        "notice": portfolio_risk_no_advice_notice()
    });
    surface["reportEnvelope"] =
        portfolio_risk_report_envelope(&surface, strategy_id, studio_base_url, db_path);
    surface
}

pub fn portfolio_risk_report_envelope(
    surface: &Value,
    strategy_id: &str,
    studio_base_url: &str,
    db_path: &str,
) -> Value {
    let snapshot_id = str_at(surface, &["snapshotId"]);
    let links = portfolio_risk_deep_links(studio_base_url, strategy_id, &snapshot_id);
    let charts = json!([
        {
            "id": "portfolio_risk_exposure_table",
            "title": "Exposure table",
            "href": links["portfolioRisk"]["href"],
            "type": "exposure_table",
            "formats": ["interactive", "html", "svg", "csv", "json"],
            "data": surface.get("exposureTables").cloned().unwrap_or_else(|| json!([]))
        },
        {
            "id": "portfolio_risk_concentration_overlay",
            "title": "Concentration overlay",
            "href": links["portfolioRisk"]["href"],
            "type": "concentration_overlay",
            "formats": ["interactive", "html", "svg", "csv", "json"],
            "data": {
                "warnings": surface.get("concentrationWarnings").cloned().unwrap_or_else(|| json!([])),
                "thresholdComparisons": surface.get("thresholdComparisons").cloned().unwrap_or_else(|| json!([]))
            }
        }
    ]);
    let replay_refs = portfolio_risk_replay_refs(db_path, strategy_id, &snapshot_id);
    let artifact_refs = json!([
        {"id": "portfolio-risk-snapshot", "kind": "portfolio_risk_snapshot", "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/snapshot.json")},
        {"id": "portfolio-risk-exposures", "kind": "portfolio_risk_exposure_table", "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/exposures.csv")},
        {"id": "portfolio-risk-warnings", "kind": "portfolio_risk_warnings", "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/warnings.json")}
    ]);
    json!({
        "schemaVersion": "tradeassembly.report.v1",
        "bundleSchema": {
            "version": "tradeassembly.report.v1",
            "kind": "portfolio_risk",
            "families": ["portfolio_risk", "exposure", "concentration", "thresholds", "journal", "replay", "review"],
            "requiredSections": ["summary", "deepLinks", "views", "charts", "thresholdComparisons", "warnings", "modelProvenance", "exportRefs", "replayRefs"]
        },
        "kind": "portfolio_risk",
        "strategyId": strategy_id,
        "snapshotId": snapshot_id,
        "summary": {
            "title": format!("Portfolio risk overlay for {strategy_id}"),
            "status": if surface["ok"].as_bool().unwrap_or(false) { "available" } else { "blocked" },
            "primaryMetric": {"name": "exposure_tables", "value": surface["exposureTables"].as_array().map(Vec::len).unwrap_or(0)},
            "interpretation": "Portfolio risk output is descriptive, replayable evidence for user review in TradeAssembly Studio."
        },
        "deepLinks": links,
        "views": portfolio_risk_views(&links),
        "renderContract": render_contract(),
        "renderTargets": {
            "tradeassembly": {
                "kind": "interactive_ui",
                "label": "Open portfolio risk overlay in TradeAssembly",
                "href": links["portfolioRisk"]["href"],
                "uri": links["portfolioRisk"]["uri"],
                "sideBySide": true
            },
            "htmlArtifact": {"kind": "single_page_html", "inlineRef": "previews.html", "downloadName": "tradeassembly-portfolio-risk.html"},
            "staticSvg": {"kind": "static_svg", "inlineRef": "previews.svg", "downloadName": "tradeassembly-portfolio-risk.svg"},
            "cli": {"kind": "commands", "commands": replay_refs["commands"]}
        },
        "previews": portfolio_risk_previews(surface, &links),
        "charts": charts,
        "exposureTables": surface.get("exposureTables").cloned().unwrap_or_else(|| json!([])),
        "thresholdComparisons": surface.get("thresholdComparisons").cloned().unwrap_or_else(|| json!([])),
        "warnings": surface.get("missingDataWarnings").cloned().unwrap_or_else(|| json!([])),
        "concentrationWarnings": surface.get("concentrationWarnings").cloned().unwrap_or_else(|| json!([])),
        "modelProvenance": surface.get("modelProvenance").cloned().unwrap_or_else(|| json!({})),
        "artifactRefs": artifact_refs,
        "artifactBundle": {
            "id": format!("bundle_portfolio_risk_{snapshot_id}"),
            "kind": "portfolio_risk",
            "strategyId": strategy_id,
            "snapshotId": snapshot_id,
            "refs": artifact_refs,
            "agentUse": "Inspect exposure tables, concentration warnings, threshold comparisons, stale-data warnings, provenance, replay refs, and export refs when the user asks for detail."
        },
        "exportRefs": surface.get("exportRefs").cloned().unwrap_or_else(|| portfolio_risk_export_refs(strategy_id, &snapshot_id)),
        "replayRefs": replay_refs,
        "journalEvidence": surface.get("journalEvidence").cloned().unwrap_or_else(|| json!({})),
        "raw": {"portfolioRisk": surface}
    })
}

pub fn scenario_valuation_surface(
    strategy_id: &str,
    checkpoint_id: &str,
    _studio_base_url: &str,
    _db_path: &str,
) -> Value {
    json!({
        "schemaVersion": "tradeassembly.scenario_valuation.v1",
        "ok": false,
        "kind": "scenario_valuation",
        "strategyId": strategy_id,
        "checkpointId": checkpoint_id,
        "scenarioMatrix": {"rows": []},
        "greeks": {"rows": []},
        "perLegDetails": [],
        "assumptions": [],
        "warnings": ["A durable derivatives analysis bound to an exact completed source run is required."],
        "modelProvenance": {},
        "error": {
            "code": "scenario_valuation_requires_durable_derivatives_analysis",
            "message": "Create or read a durable derivatives analysis; fixture-backed scenario values are not available."
        }
    })
}

pub fn scenario_report_envelope(
    scenario_surface: &Value,
    strategy_id: &str,
    studio_base_url: &str,
    db_path: &str,
) -> Value {
    let checkpoint_id = str_at(scenario_surface, &["checkpointId"]);
    let links = scenario_deep_links(studio_base_url, strategy_id, &checkpoint_id);
    let charts = json!([
        {
            "id": "scenario_matrix",
            "title": "Scenario matrix",
            "href": links["scenario"]["href"],
            "type": "scenario_matrix",
            "formats": ["interactive", "html", "svg", "csv", "json"],
            "data": scenario_surface.get("scenarioMatrix").cloned().unwrap_or_else(|| json!({"rows": []}))
        },
        {
            "id": "greeks_table",
            "title": "Greeks table",
            "href": links["scenario"]["href"],
            "type": "greeks_table",
            "formats": ["interactive", "html", "svg", "csv", "json"],
            "data": scenario_surface.get("greeks").cloned().unwrap_or_else(|| json!({"rows": []}))
        }
    ]);
    let replay_refs = scenario_replay_refs(db_path, strategy_id, &checkpoint_id);
    let artifact_refs = json!([
        {"id": "scenario-matrix", "kind": "scenario_matrix", "uri": format!("tradeassembly://scenario-valuations/{checkpoint_id}/matrix.json")},
        {"id": "greeks-table", "kind": "greeks_table", "uri": format!("tradeassembly://scenario-valuations/{checkpoint_id}/greeks.json")}
    ]);
    json!({
        "schemaVersion": "tradeassembly.report.v1",
        "bundleSchema": {
            "version": "tradeassembly.report.v1",
            "kind": "scenario_valuation",
            "families": ["scenario_valuation", "greeks", "per_leg_detail", "journal", "replay", "review"],
            "requiredSections": ["summary", "deepLinks", "views", "charts", "assumptions", "warnings", "modelProvenance", "exportRefs", "replayRefs"]
        },
        "kind": "scenario_valuation",
        "strategyId": strategy_id,
        "checkpointId": checkpoint_id,
        "summary": {
            "title": format!("Scenario valuation for {strategy_id}"),
            "status": "available",
            "primaryMetric": {"name": "scenario_rows", "value": scenario_surface["scenarioMatrix"]["rows"].as_array().map(Vec::len).unwrap_or(0)},
            "interpretation": "Scenario valuation output is descriptive and replayable for user review in TradeAssembly Studio."
        },
        "deepLinks": links,
        "views": scenario_views(&links),
        "renderContract": render_contract(),
        "renderTargets": {
            "tradeassembly": {
                "kind": "interactive_ui",
                "label": "Open scenario valuation in TradeAssembly",
                "href": links["scenario"]["href"],
                "uri": links["scenario"]["uri"],
                "sideBySide": true
            },
            "htmlArtifact": {"kind": "single_page_html", "inlineRef": "previews.html", "downloadName": "tradeassembly-scenario-valuation.html"},
            "staticSvg": {"kind": "static_svg", "inlineRef": "previews.svg", "downloadName": "tradeassembly-scenario-valuation.svg"},
            "cli": {"kind": "commands", "commands": replay_refs["commands"]}
        },
        "previews": scenario_previews(scenario_surface, &links),
        "charts": charts,
        "perLegDetails": scenario_surface.get("perLegDetails").cloned().unwrap_or_else(|| json!([])),
        "assumptions": scenario_surface.get("assumptions").cloned().unwrap_or_else(|| json!([])),
        "warnings": [scenario_no_advice_notice()],
        "modelProvenance": scenario_surface.get("modelProvenance").cloned().unwrap_or_else(|| json!({})),
        "artifactRefs": artifact_refs,
        "artifactBundle": {
            "id": format!("bundle_scenario_{checkpoint_id}"),
            "kind": "scenario_valuation",
            "strategyId": strategy_id,
            "checkpointId": checkpoint_id,
            "refs": artifact_refs,
            "agentUse": "Inspect scenario matrix rows, per-leg Greeks, assumptions, warnings, provenance, replay refs, and export refs when the user asks for detail."
        },
        "exportRefs": {
            "json": {"kind": "json", "uri": format!("tradeassembly://scenario-valuations/{checkpoint_id}/report.json")},
            "csv": {"kind": "csv", "uri": format!("tradeassembly://scenario-valuations/{checkpoint_id}/scenario-matrix.csv")},
            "greeksCsv": {"kind": "csv", "uri": format!("tradeassembly://scenario-valuations/{checkpoint_id}/greeks.csv")},
            "htmlPreview": {"kind": "html", "inlineRef": "previews.html"},
            "svgPreview": {"kind": "svg", "inlineRef": "previews.svg"}
        },
        "replayRefs": replay_refs,
        "raw": {"scenarioValuation": scenario_surface}
    })
}

fn scenario_no_advice_notice() -> &'static str {
    "Scenario valuation output is descriptive research evidence only. TradeAssembly did not recommend what to trade, when to trade, or how much to trade."
}

fn monte_carlo_no_advice_notice() -> &'static str {
    "Monte Carlo output is descriptive research evidence only. TradeAssembly did not recommend what to trade, when to trade, or how much to trade."
}

fn portfolio_risk_no_advice_notice() -> &'static str {
    "Portfolio risk output is descriptive evidence only. TradeAssembly did not recommend what to trade, when to trade, or how much to trade."
}

fn no_advice_selector_notice() -> &'static str {
    "Selectors are deterministic filters over user-defined criteria. TradeAssembly did not recommend what to trade, when to trade, or how much to trade."
}

fn render_contract() -> Value {
    json!({
        "preferred": "tradeassembly_ui_deep_links",
        "userVisible": ["summary", "deepLinks", "views", "warnings"],
        "agentVisible": ["artifactBundle", "artifactRefs", "exportRefs", "replayRefs", "raw"],
        "fallbackOrder": [
            {"capability": "side_by_side_tradeassembly_ui"},
            {"capability": "single_page_html_artifact"},
            {"capability": "static_svg_preview"},
            {"capability": "cli_replay_commands"}
        ]
    })
}

fn scenario_href(base_url: &str, strategy_id: &str, checkpoint_id: &str) -> String {
    format!(
        "{}/app/strategies/{strategy_id}/research?view=scenario-valuation&checkpointId={checkpoint_id}",
        base_url.trim_end_matches('/')
    )
}

fn monte_carlo_href(base_url: &str, strategy_id: &str, assumption_hash: &str) -> String {
    format!(
        "{}/app/strategies/{strategy_id}/research?view=monte-carlo&assumptionHash={}",
        base_url.trim_end_matches('/'),
        assumption_hash.replace(':', "%3A")
    )
}

fn portfolio_risk_href(base_url: &str, strategy_id: &str, snapshot_id: &str) -> String {
    format!(
        "{}/app/strategies/{strategy_id}/research?view=portfolio-risk&snapshotId={snapshot_id}",
        base_url.trim_end_matches('/')
    )
}

fn scenario_deep_links(base_url: &str, strategy_id: &str, checkpoint_id: &str) -> Value {
    let base_url = base_url.trim_end_matches('/');
    let strategy_path = format!("/app/strategies/{strategy_id}");
    let research_path = format!("{strategy_path}/research");
    json!({
        "strategy": {
            "label": "Strategy",
            "href": format!("{base_url}{strategy_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}")
        },
        "research": {
            "label": "Research",
            "href": format!("{base_url}{research_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/research")
        },
        "scenario": {
            "label": "Scenario valuation",
            "href": scenario_href(base_url, strategy_id, checkpoint_id),
            "uri": format!("tradeassembly://strategy/{strategy_id}/scenario-valuation/{checkpoint_id}")
        },
        "replay": {
            "label": "Replay",
            "href": format!("{base_url}{strategy_path}/replay?checkpointId={checkpoint_id}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/replay/{checkpoint_id}")
        }
    })
}

fn monte_carlo_deep_links(base_url: &str, strategy_id: &str, assumption_hash: &str) -> Value {
    let base_url = base_url.trim_end_matches('/');
    let strategy_path = format!("/app/strategies/{strategy_id}");
    let research_path = format!("{strategy_path}/research");
    json!({
        "strategy": {
            "href": format!("{base_url}{strategy_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}")
        },
        "research": {
            "href": format!("{base_url}{research_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/research")
        },
        "monteCarlo": {
            "href": monte_carlo_href(base_url, strategy_id, assumption_hash),
            "uri": format!("tradeassembly://strategy/{strategy_id}/monte-carlo/{assumption_hash}")
        },
        "replay": {
            "href": format!("{base_url}{research_path}?view=monte-carlo&replay=1"),
            "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/replay")
        }
    })
}

fn portfolio_risk_deep_links(base_url: &str, strategy_id: &str, snapshot_id: &str) -> Value {
    let base_url = base_url.trim_end_matches('/');
    let strategy_path = format!("/app/strategies/{strategy_id}");
    let research_path = format!("{strategy_path}/research");
    json!({
        "strategy": {
            "href": format!("{base_url}{strategy_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}")
        },
        "research": {
            "href": format!("{base_url}{research_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/research")
        },
        "portfolioRisk": {
            "href": portfolio_risk_href(base_url, strategy_id, snapshot_id),
            "uri": format!("tradeassembly://strategy/{strategy_id}/portfolio-risk/{snapshot_id}")
        },
        "replay": {
            "href": format!("{base_url}{research_path}?view=portfolio-risk&replay=1&snapshotId={snapshot_id}"),
            "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/replay")
        }
    })
}

fn scenario_views(links: &Value) -> Value {
    Value::Array(
        ["scenario", "research", "strategy", "replay"]
            .into_iter()
            .map(|key| {
                let link = &links[key];
                json!({
                    "id": key,
                    "label": link["label"],
                    "href": link["href"],
                    "uri": link["uri"]
                })
            })
            .collect(),
    )
}

fn monte_carlo_views(links: &Value) -> Value {
    Value::Array(
        ["monteCarlo", "research", "strategy", "replay"]
            .into_iter()
            .map(|key| {
                json!({
                    "id": key,
                    "href": links[key]["href"],
                    "uri": links[key]["uri"]
                })
            })
            .collect(),
    )
}

fn portfolio_risk_views(links: &Value) -> Value {
    Value::Array(
        ["portfolioRisk", "research", "strategy", "replay"]
            .into_iter()
            .map(|key| {
                json!({
                    "id": key,
                    "href": links[key]["href"],
                    "uri": links[key]["uri"]
                })
            })
            .collect(),
    )
}

fn scenario_replay_refs(db_path: &str, strategy_id: &str, checkpoint_id: &str) -> Value {
    json!({
        "deterministic": true,
        "checkpointId": checkpoint_id,
        "commands": [
            format!("tradeassembly scenario valuation --db {db_path} --strategy-id {strategy_id} --checkpoint-id {checkpoint_id}"),
            format!("tradeassembly report-envelope --db {db_path} --strategy-id {strategy_id} --kind scenario_valuation --checkpoint-id {checkpoint_id}")
        ],
        "checks": ["strategy_id", "checkpoint_id", "scenario_matrix", "greeks", "model_provenance"]
    })
}

fn monte_carlo_replay_refs(db_path: &str, strategy_id: &str, assumption_hash: &str) -> Value {
    json!({
        "assumptionHash": assumption_hash,
        "commands": [
            format!("tradeassembly monte-carlo replay --db {db_path} --strategy-id {strategy_id} --assumption-hash {assumption_hash}"),
            format!("tradeassembly monte-carlo report --db {db_path} --strategy-id {strategy_id} --assumption-hash {assumption_hash}")
        ],
        "result": {"kind": "json", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/result.json")},
        "ledger": {"kind": "json", "uri": format!("tradeassembly://monte-carlo/{strategy_id}/{assumption_hash}/assumptions.json")},
        "checks": ["strategy_id", "assumption_hash", "path_summary", "confidence_bands", "warning_codes"]
    })
}

fn portfolio_risk_replay_refs(db_path: &str, strategy_id: &str, snapshot_id: &str) -> Value {
    json!({
        "snapshotId": snapshot_id,
        "commands": [
            format!("tradeassembly risk overlay --db {db_path} --strategy-id {strategy_id} --scope-kind strategy"),
            format!("tradeassembly report-envelope --db {db_path} --strategy-id {strategy_id} --kind portfolio_risk")
        ],
        "snapshot": {"kind": "json", "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/snapshot.json")},
        "checks": ["strategy_id", "snapshot_id", "exposure_tables", "concentration_warnings", "warning_codes"]
    })
}

fn scenario_previews(surface: &Value, links: &Value) -> Value {
    let title = escape_html("TradeAssembly scenario valuation");
    let scenario_href = escape_html(links["scenario"]["href"].as_str().unwrap_or(""));
    let row_count = surface["scenarioMatrix"]["rows"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0)
        .to_string();
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body><main><h1>{title}</h1><p>Scenario rows: {row_count}</p><p><a href=\"{scenario_href}\">Open scenario valuation in TradeAssembly</a></p><p>{}</p></main></body></html>",
        escape_html(scenario_no_advice_notice())
    );
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"960\" height=\"540\" viewBox=\"0 0 960 540\"><rect width=\"960\" height=\"540\" fill=\"#f8fafc\"/><text x=\"40\" y=\"70\" font-family=\"Inter, Arial\" font-size=\"30\" fill=\"#0f172a\">{title}</text><text x=\"40\" y=\"120\" font-family=\"Inter, Arial\" font-size=\"18\" fill=\"#334155\">Scenario rows: {row_count}</text><rect x=\"40\" y=\"165\" width=\"880\" height=\"220\" rx=\"8\" fill=\"#ffffff\" stroke=\"#cbd5e1\"/><text x=\"70\" y=\"225\" font-family=\"Inter, Arial\" font-size=\"22\" fill=\"#0f172a\">Scenario matrix and Greeks table available in TradeAssembly.</text><text x=\"40\" y=\"430\" font-family=\"Inter, Arial\" font-size=\"16\" fill=\"#475569\">Open the deep link for interactive review, replay, and exports.</text></svg>"
    );
    json!({
        "html": {"mediaType": "text/html", "content": html},
        "svg": {"mediaType": "image/svg+xml", "content": svg}
    })
}

fn monte_carlo_previews(surface: &Value, links: &Value) -> Value {
    let title = escape_html("TradeAssembly Monte Carlo research");
    let monte_carlo_href = escape_html(links["monteCarlo"]["href"].as_str().unwrap_or(""));
    let path_count = surface["assumptionLedger"]["pathCount"]
        .as_u64()
        .unwrap_or(0);
    let horizon_days = surface["assumptionLedger"]["horizonDays"]
        .as_u64()
        .unwrap_or(0);
    let warning_count = surface["warnings"].as_array().map(Vec::len).unwrap_or(0);
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body><main><h1>{title}</h1><p>Paths: {path_count}</p><p>Horizon days: {horizon_days}</p><p>Warnings: {warning_count}</p><p><a href=\"{monte_carlo_href}\">Open Monte Carlo research in TradeAssembly</a></p><p>{}</p></main></body></html>",
        escape_html(monte_carlo_no_advice_notice())
    );
    json!({
        "html": {"content": html},
        "svg": {"content": format!("<svg viewBox=\"0 0 320 120\" xmlns=\"http://www.w3.org/2000/svg\"><text x=\"16\" y=\"32\">Monte Carlo paths: {path_count}</text><text x=\"16\" y=\"64\">Horizon days: {horizon_days}</text><text x=\"16\" y=\"96\">Warnings: {warning_count}</text></svg>")}
    })
}

fn portfolio_risk_previews(surface: &Value, links: &Value) -> Value {
    let title = escape_html("TradeAssembly portfolio risk overlay");
    let href = escape_html(links["portfolioRisk"]["href"].as_str().unwrap_or(""));
    let exposure_count = surface["exposureTables"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let concentration_count = surface["concentrationWarnings"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let warning_count = surface["missingDataWarnings"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body><main><h1>{title}</h1><p>Exposure tables: {exposure_count}</p><p>Concentration warnings: {concentration_count}</p><p>Missing data warnings: {warning_count}</p><p><a href=\"{href}\">Open portfolio risk overlay in TradeAssembly</a></p><p>{}</p></main></body></html>",
        escape_html(portfolio_risk_no_advice_notice())
    );
    json!({
        "html": {"content": html},
        "svg": {"content": format!("<svg viewBox=\"0 0 360 140\" xmlns=\"http://www.w3.org/2000/svg\"><text x=\"16\" y=\"32\">Portfolio risk overlay</text><text x=\"16\" y=\"64\">Exposure tables: {exposure_count}</text><text x=\"16\" y=\"96\">Concentration warnings: {concentration_count}</text><text x=\"16\" y=\"124\">Warnings: {warning_count}</text></svg>")}
    })
}

fn threshold_comparisons(snapshot: &Value) -> Value {
    let rows = snapshot
        .get("concentrationWarnings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|warning| {
            json!({
                "thresholdId": warning.get("thresholdId").cloned().unwrap_or(Value::Null),
                "group": warning.get("group").cloned().unwrap_or(Value::Null),
                "metric": warning.get("metric").cloned().unwrap_or(Value::Null),
                "observed": warning.get("observed").cloned().unwrap_or(Value::Null),
                "limit": warning.get("limit").cloned().unwrap_or(Value::Null),
                "severity": warning.get("severity").cloned().unwrap_or(Value::Null)
            })
        })
        .collect::<Vec<_>>();
    Value::Array(rows)
}

fn portfolio_risk_export_refs(strategy_id: &str, snapshot_id: &str) -> Value {
    json!({
        "json": {"kind": "json", "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/snapshot.json")},
        "csv": {"kind": "csv", "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/exposures.csv")},
        "warningsJson": {"kind": "json", "uri": format!("tradeassembly://portfolio-risk/{strategy_id}/{snapshot_id}/warnings.json")},
        "htmlPreview": {"kind": "html", "inlineRef": "previews.html"},
        "svgPreview": {"kind": "svg", "inlineRef": "previews.svg"}
    })
}

fn selector_deep_links(base_url: &str, strategy_id: &str, run_id: &str) -> Value {
    let base_url = base_url.trim_end_matches('/');
    let strategy_path = format!("/app/strategies/{strategy_id}");
    let research_path = format!("{strategy_path}/research");
    json!({
        "strategy": {
            "label": "Strategy",
            "href": format!("{base_url}{strategy_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}")
        },
        "research": {
            "label": "Research",
            "href": format!("{base_url}{research_path}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/research")
        },
        "selector": {
            "label": "Selector run",
            "href": format!("{base_url}{research_path}?selectorRunId={run_id}"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/selector-run/{run_id}")
        },
        "review": {
            "label": "Review",
            "href": format!("{base_url}{strategy_path}/share"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/review")
        },
        "replay": {
            "label": "Replay",
            "href": format!("{base_url}{strategy_path}/replay"),
            "uri": format!("tradeassembly://strategy/{strategy_id}/replay")
        }
    })
}

fn selector_views(links: &Value) -> Value {
    let mut views = Vec::new();
    for key in ["strategy", "research", "selector", "review", "replay"] {
        let link = &links[key];
        views.push(json!({
            "id": key,
            "label": link["label"],
            "href": link["href"],
            "uri": link["uri"]
        }));
    }
    Value::Array(views)
}

fn selector_summary(strategy_id: &str, payload: &Value) -> Value {
    let accepted = array_len(payload, "accepted_candidates");
    let rejected = array_len(payload, "rejected_candidates");
    let first_contract = payload
        .get("accepted_candidates")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .or_else(|| {
            payload
                .get("rejected_candidates")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
        })
        .and_then(|item| item.get("contract"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    json!({
        "title": format!("Selector report for {strategy_id}"),
        "status": "available",
        "selectorId": str_at(payload, &["selector", "selector_id"]),
        "selectorRunId": str_at(payload, &["run_id"]),
        "symbol": str_at(payload, &["selector", "underlying_symbol"]),
        "dataSource": first_contract.get("source_provider").cloned().unwrap_or(Value::Null),
        "primaryMetric": {"name": "accepted_candidates", "value": accepted},
        "counts": {
            "acceptedCandidates": accepted,
            "rejectedCandidates": rejected,
            "reasonCodes": payload.get("reasonCodeSummary").cloned().unwrap_or_else(|| json!({}))
        },
        "interpretation": "Selector output is descriptive and replayable for user review in TradeAssembly Studio."
    })
}

fn selector_charts(links: &Value, payload: &Value) -> Value {
    let rows: Vec<Value> = selector_candidates(payload)
        .into_iter()
        .map(|item| {
            json!({
                "candidateId": item.get("candidate_id").cloned().unwrap_or(Value::Null),
                "status": item.get("status").cloned().unwrap_or(Value::Null),
                "contractId": item.get("contract").and_then(|contract| contract.get("canonical_contract_id")).cloned().unwrap_or(Value::Null),
                "reasonCodes": item.get("reason_codes").cloned().unwrap_or_else(|| json!([]))
            })
        })
        .collect();
    let candidate_ids: Vec<Value> = rows
        .iter()
        .filter_map(|row| row.get("candidateId").cloned())
        .filter(|value| !value.is_null())
        .collect();
    let reason_rows: Vec<Value> = sorted_reason_entries(payload)
        .into_iter()
        .map(|(key, value)| json!({"reasonCode": key, "count": value}))
        .collect();
    json!([
        {
            "id": "selector_candidates",
            "title": "Selector candidates",
            "href": links["selector"]["href"],
            "type": "selector_candidate_table",
            "refs": candidate_ids,
            "formats": ["interactive", "html", "svg"],
            "data": {"rows": rows}
        },
        {
            "id": "selector_reason_codes",
            "title": "Selector reason-code summary",
            "href": links["selector"]["href"],
            "type": "reason_code_summary",
            "refs": [],
            "formats": ["interactive", "html", "svg"],
            "data": {"rows": reason_rows}
        }
    ])
}

fn selector_artifact_refs(payload: &Value) -> Value {
    let refs: Vec<Value> = selector_candidates(payload)
        .into_iter()
        .filter_map(|item| {
            let candidate_id = item.get("candidate_id").and_then(Value::as_str)?;
            let status = item.get("status").and_then(Value::as_str).unwrap_or("unknown");
            let run_id = str_at(payload, &["run_id"]);
            Some(json!({
                "id": candidate_id,
                "kind": format!("selector_candidate_{status}"),
                "uri": item
                    .get("replay_ref")
                    .cloned()
                    .unwrap_or_else(|| json!(format!("tradeassembly://selector-runs/{run_id}/candidates/{candidate_id}"))),
                "selectorRunId": run_id,
                "reasonCodes": item.get("reason_codes").cloned().unwrap_or_else(|| json!([]))
            }))
        })
        .collect();
    Value::Array(refs)
}

fn selector_replay_refs(db_path: &str, strategy_id: &str, payload: &Value) -> Value {
    let run_id = str_at(payload, &["run_id"]);
    let mut commands = payload
        .get("replay_refs")
        .and_then(|refs| refs.get("commands"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    commands.insert(
        0,
        json!(format!(
            "tradeassembly selector-report-envelope --db {db_path} --strategy-id {strategy_id} --selector-run-json selector-run-{run_id}.json"
        )),
    );
    let mut refs = payload
        .get("replay_refs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    refs.insert("commands".to_string(), Value::Array(commands));
    refs.insert("deterministic".to_string(), Value::Bool(true));
    refs.insert(
        "checks".to_string(),
        json!([
            "selector_spec_hash",
            "snapshot_ref",
            "candidate_reason_codes",
            "canonical_payload"
        ]),
    );
    Value::Object(refs)
}

fn selector_export_refs(db_path: &str, strategy_id: &str, payload: &Value) -> Value {
    let run_id = str_at(payload, &["run_id"]);
    let mut refs = payload
        .get("export_refs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    refs.insert(
        "reportJson".to_string(),
        json!({
            "kind": "json",
            "command": format!("tradeassembly selector-report-envelope --db {db_path} --strategy-id {strategy_id} --selector-run-json selector-run-{run_id}.json")
        }),
    );
    refs.entry("candidateCsv".to_string())
        .or_insert_with(|| json!({"kind": "csv", "path": format!("tradeassembly://selector-runs/{run_id}/candidates.csv")}));
    refs.insert(
        "htmlPreview".to_string(),
        json!({"kind": "html", "inlineRef": "previews.html"}),
    );
    refs.insert(
        "svgPreview".to_string(),
        json!({"kind": "svg", "inlineRef": "previews.svg"}),
    );
    Value::Object(refs)
}

fn selector_previews(summary: &Value, charts: &Value, links: &Value) -> Value {
    let title = escape_html(
        summary
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("TradeAssembly selector report"),
    );
    let accepted = escape_html(&summary["counts"]["acceptedCandidates"].to_string());
    let rejected = escape_html(&summary["counts"]["rejectedCandidates"].to_string());
    let chart_items = charts
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            format!(
                "<li>{}</li>",
                escape_html(item.get("title").and_then(Value::as_str).unwrap_or(""))
            )
        })
        .collect::<String>();
    let selector_href = escape_html(links["selector"]["href"].as_str().unwrap_or(""));
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body><main><h1>{title}</h1><p>Accepted candidates: {accepted}</p><p>Rejected candidates: {rejected}</p><p><a href=\"{selector_href}\">Open selector run in TradeAssembly</a></p><h2>Views</h2><ul>{chart_items}</ul><p>{}</p></main></body></html>",
        escape_html(no_advice_selector_notice())
    );
    let svg_title = if title.chars().count() > 80 {
        title.chars().take(80).collect::<String>()
    } else {
        title.clone()
    };
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"960\" height=\"540\" viewBox=\"0 0 960 540\"><rect width=\"960\" height=\"540\" fill=\"#f8fafc\"/><text x=\"40\" y=\"70\" font-family=\"Inter, Arial\" font-size=\"30\" fill=\"#0f172a\">{svg_title}</text><text x=\"40\" y=\"120\" font-family=\"Inter, Arial\" font-size=\"18\" fill=\"#334155\">Accepted: {accepted} | Rejected: {rejected}</text><rect x=\"40\" y=\"165\" width=\"880\" height=\"220\" rx=\"8\" fill=\"#ffffff\" stroke=\"#cbd5e1\"/><text x=\"70\" y=\"225\" font-family=\"Inter, Arial\" font-size=\"22\" fill=\"#0f172a\">Selector reason-code and candidate table available in TradeAssembly.</text><text x=\"40\" y=\"430\" font-family=\"Inter, Arial\" font-size=\"16\" fill=\"#475569\">Open the TradeAssembly deep link for interactive review, replay, and exports.</text></svg>"
    );
    json!({
        "html": {"mediaType": "text/html", "content": html},
        "svg": {"mediaType": "image/svg+xml", "content": svg}
    })
}

fn selector_skip_reasons(payload: &Value) -> Value {
    Value::Array(
        sorted_reason_entries(payload)
            .into_iter()
            .filter(|(key, count)| key != "accepted" && *count > 0)
            .map(|(key, _)| Value::String(key))
            .collect(),
    )
}

fn sorted_reason_entries(payload: &Value) -> Vec<(String, i64)> {
    let mut rows: Vec<(String, i64)> = payload
        .get("reasonCodeSummary")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.iter())
        .filter_map(|(key, value)| value.as_i64().map(|count| (key.clone(), count)))
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

fn selector_candidates(payload: &Value) -> Vec<&Value> {
    let mut items = Vec::new();
    if let Some(accepted) = payload.get("accepted_candidates").and_then(Value::as_array) {
        items.extend(accepted);
    }
    if let Some(rejected) = payload.get("rejected_candidates").and_then(Value::as_array) {
        items.extend(rejected);
    }
    items
}

fn canonical_selector_run(selector_run: &Value) -> Value {
    let mut payload = serde_json::Map::new();
    for key in [
        "schema_version",
        "run_id",
        "selector",
        "snapshot_ref",
        "accepted_candidates",
        "rejected_candidates",
        "provider_data_assumptions",
        "warnings",
        "replay_refs",
        "export_refs",
        "reasonCodeSummary",
        "notice",
    ] {
        if let Some(value) = selector_run.get(key) {
            payload.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(payload)
}

fn array_len(payload: &Value, key: &str) -> usize {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

fn str_at(value: &Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn report_envelope_links_studio_and_keeps_agent_artifacts() {
        let envelope = super::report_envelope(
            "backtest",
            "strat_local_btc_demo",
            "bt-001",
            "http://127.0.0.1:3001",
        );

        assert_eq!(envelope["schemaVersion"], "tradeassembly.report.v1");
        let families = envelope["bundleSchema"]["families"].as_array().unwrap();
        for family in [
            "backtest",
            "monte_carlo",
            "paper",
            "live",
            "journal",
            "replay",
        ] {
            assert!(families.iter().any(|item| item == family));
        }
        assert_eq!(envelope["kind"], "backtest");
        assert_eq!(envelope["strategyId"], "strat_local_btc_demo");
        assert_eq!(envelope["summary"]["status"], "completed");
        assert_eq!(envelope["summary"]["primaryMetric"]["name"], "return_pct");
        assert_eq!(
            envelope["renderContract"]["preferred"],
            "tradeassembly_ui_deep_links"
        );
        assert_eq!(
            envelope["renderContract"]["userVisible"],
            serde_json::json!(["summary", "deepLinks", "views", "warnings"])
        );
        assert_eq!(
            envelope["renderContract"]["fallbackOrder"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["capability"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "side_by_side_tradeassembly_ui",
                "single_page_html_artifact",
                "static_svg_preview",
                "cli_replay_commands",
            ]
        );
        assert_eq!(
            envelope["renderTargets"]["tradeassembly"]["href"],
            envelope["deepLinks"]["backtest"]["href"]
        );
        assert_eq!(
            envelope["renderTargets"]["htmlArtifact"]["inlineRef"],
            "previews.html"
        );
        assert_eq!(
            envelope["renderTargets"]["staticSvg"]["inlineRef"],
            "previews.svg"
        );
        assert!(envelope["previews"]["html"]["content"]
            .as_str()
            .unwrap()
            .starts_with("<!doctype html>"));
        assert!(envelope["previews"]["svg"]["content"]
            .as_str()
            .unwrap()
            .starts_with("<svg"));
        assert!(envelope["deepLinks"]["strategy"]["href"]
            .as_str()
            .unwrap()
            .ends_with("/app/strategies/strat_local_btc_demo"));
        assert!(envelope["deepLinks"]["backtest"]["href"]
            .as_str()
            .unwrap()
            .contains("backtestId=bt-001"));
        let chart_ids = envelope["charts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|chart| chart["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        for chart in [
            "underlying_with_markers",
            "equity_curve",
            "drawdown",
            "trade_journal",
            "monte_carlo",
            "scenario_matrix",
        ] {
            assert!(chart_ids.contains(&chart));
        }
        assert!(!envelope["charts"][0]["data"]["priceSeries"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            envelope["charts"][0]["formats"],
            serde_json::json!(["interactive", "html", "svg"])
        );
        assert!(!envelope["charts"][1]["data"]["points"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            envelope["charts"][5]["data"]["rows"][0]["scenario"],
            "Baseline"
        );
        assert_eq!(envelope["journal"]["rows"], serde_json::json!([]));
        assert!(envelope["exportRefs"].get("tradeJournalCsv").is_some());
        assert!(envelope["artifactRefs"][0]["uri"]
            .as_str()
            .unwrap()
            .starts_with("tradeassembly://artifact/"));
        assert_eq!(
            envelope["artifactBundle"]["refs"][0]["id"],
            envelope["artifactRefs"][0]["id"]
        );
        assert!(envelope["replayRefs"]["commands"][0]
            .as_str()
            .unwrap()
            .starts_with("tradeassembly report-envelope"));
        assert!(envelope["replayRefs"]["commands"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .starts_with("tradeassembly research-artifacts"));
        assert_eq!(envelope["raw"]["backtest"]["id"], "bt-001");
        assert_eq!(
            envelope["warnings"],
            serde_json::json!(["Descriptive research output only. TradeAssembly did not recommend a trade, position size, or activation decision."])
        );
    }

    #[test]
    fn report_envelope_http_cli_and_mcp_facades_share_payload_shape() {
        let http = super::report_envelope_http("backtest", "strat_local_btc_demo", "bt-001");
        let cli = super::report_envelope_cli("strat_local_btc_demo", "bt-001");
        let mcp = super::report_envelope_mcp("strat_local_btc_demo", "bt-001");

        assert_eq!(http.status_code, 200);
        assert!(http.body["deepLinks"]["research"]["href"]
            .as_str()
            .unwrap()
            .ends_with("/app/strategies/strat_local_btc_demo/research"));
        assert_eq!(cli["artifactRefs"][0]["backtestId"], "bt-001");
        assert_eq!(mcp["raw"]["backtest"]["id"], "bt-001");
        assert_eq!(
            mcp["renderContract"]["agentVisible"],
            serde_json::json!([
                "artifactBundle",
                "artifactRefs",
                "exportRefs",
                "replayRefs",
                "raw"
            ])
        );
    }

    #[test]
    fn selector_report_envelope_links_user_to_tradeassembly_and_keeps_agent_artifacts() {
        let selector_run = sample_selector_run();
        let envelope = super::selector_report_envelope(
            &selector_run,
            "strat_selector",
            "http://127.0.0.1:3001",
            ".tradeassembly/tradeassembly.db",
        );

        assert_eq!(envelope["schemaVersion"], "tradeassembly.report.v1");
        assert_eq!(
            envelope["bundleSchema"]["families"],
            serde_json::json!([
                "selector",
                "market_snapshot",
                "candidate_filter",
                "journal",
                "replay",
                "review"
            ])
        );
        assert_eq!(envelope["kind"], "selector");
        assert_eq!(envelope["strategyId"], "strat_selector");
        assert_eq!(envelope["selectorId"], "sel_spy_put_30d");
        assert_eq!(envelope["selectorRunId"], "selector_run_1");
        assert_eq!(envelope["summary"]["counts"]["acceptedCandidates"], 1);
        assert_eq!(envelope["summary"]["counts"]["rejectedCandidates"], 1);
        assert_eq!(envelope["summary"]["dataSource"], "alpaca-paper");
        assert_eq!(
            envelope["renderContract"]["preferred"],
            "tradeassembly_ui_deep_links"
        );
        assert_eq!(
            envelope["renderContract"]["userVisible"],
            serde_json::json!(["summary", "deepLinks", "views", "warnings"])
        );
        assert_eq!(
            envelope["renderContract"]["agentVisible"],
            serde_json::json!([
                "artifactBundle",
                "artifactRefs",
                "exportRefs",
                "replayRefs",
                "raw"
            ])
        );
        assert_eq!(
            envelope["renderTargets"]["tradeassembly"]["href"],
            envelope["deepLinks"]["selector"]["href"]
        );
        assert_eq!(
            envelope["renderTargets"]["tradeassembly"]["sideBySide"],
            true
        );
        assert_eq!(
            envelope["renderTargets"]["htmlArtifact"]["inlineRef"],
            "previews.html"
        );
        assert_eq!(
            envelope["renderTargets"]["staticSvg"]["inlineRef"],
            "previews.svg"
        );
        assert!(envelope["deepLinks"]["selector"]["href"]
            .as_str()
            .unwrap()
            .ends_with("/app/strategies/strat_selector/research?selectorRunId=selector_run_1"));
        assert!(envelope["previews"]["html"]["content"]
            .as_str()
            .unwrap()
            .starts_with("<!doctype html>"));
        assert!(envelope["previews"]["svg"]["content"]
            .as_str()
            .unwrap()
            .starts_with("<svg"));
        let chart_ids = envelope["charts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|chart| chart["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            chart_ids,
            vec!["selector_candidates", "selector_reason_codes"]
        );
        assert_eq!(
            envelope["charts"][0]["data"]["rows"][0]["status"],
            "accepted"
        );
        assert_eq!(
            envelope["skipReasons"],
            serde_json::json!(["outside_delta"])
        );
        assert_eq!(
            envelope["artifactRefs"][0]["kind"],
            "selector_candidate_accepted"
        );
        assert_eq!(
            envelope["artifactRefs"][1]["kind"],
            "selector_candidate_rejected"
        );
        assert!(envelope["artifactBundle"]["agentUse"]
            .as_str()
            .unwrap()
            .starts_with("Inspect accepted/rejected candidate evidence"));
        for key in [
            "json",
            "csv",
            "reportJson",
            "candidateCsv",
            "htmlPreview",
            "svgPreview",
        ] {
            assert!(envelope["exportRefs"].get(key).is_some());
        }
        assert!(envelope["replayRefs"]["commands"][0]
            .as_str()
            .unwrap()
            .starts_with("tradeassembly selector-report-envelope --db "));
        assert_eq!(envelope["replayRefs"]["deterministic"], true);
        assert_eq!(envelope["raw"]["selectorRun"]["run_id"], "selector_run_1");
        assert_eq!(
            envelope["warnings"],
            serde_json::json!([
                "Selectors are deterministic filters over user-defined criteria. TradeAssembly did not recommend what to trade, when to trade, or how much to trade."
            ])
        );
        let serialized = serde_json::to_string(&envelope).unwrap();
        assert!(!serialized.contains("SUPER-SECRET-VALUE"));
        assert!(serialized.contains("what to trade, when to trade, or how much to trade"));
        assert!(!serialized.to_lowercase().contains("should buy"));
        assert!(!serialized.to_lowercase().contains("should sell"));
    }

    #[test]
    fn selector_report_envelope_http_cli_mcp_and_acp_facades_share_payload_shape() {
        let selector_run = sample_selector_run();
        let http = super::selector_report_envelope_http(&selector_run, "strat_selector");
        let cli = super::selector_report_envelope_cli(&selector_run, "strat_selector");
        let mcp = super::selector_report_envelope_mcp(&selector_run, "strat_selector");
        let acp = super::selector_report_envelope_acp(&selector_run, "strat_selector");

        assert_eq!(http.status_code, 200);
        assert_eq!(
            http.body["deepLinks"]["selector"]["uri"],
            "tradeassembly://strategy/strat_selector/selector-run/selector_run_1"
        );
        assert_eq!(cli["exportRefs"]["reportJson"]["kind"], "json");
        assert_eq!(cli["kind"], "selector");
        assert_eq!(mcp["raw"]["selectorRun"], acp["raw"]["selectorRun"]);
        assert_eq!(mcp["raw"]["selectorRun"]["run_id"], selector_run["run_id"]);
        assert!(mcp["raw"]["selectorRun"]
            .get("redactedSecretExample")
            .is_none());
    }

    #[test]
    fn scenario_report_envelope_includes_matrix_greeks_assumptions_exports_and_replay() {
        let surface = serde_json::json!({
            "schemaVersion": "tradeassembly.scenario_valuation.v1",
            "ok": true,
            "kind": "scenario_valuation",
            "strategyId": "strat_local_btc_demo",
            "checkpointId": "checkpoint_001",
            "scenarioMatrix": {"rows": [{"scenario": "underlying_down_2pct", "underlyingMovePct": -2.0, "valuation": 121.5}]},
            "greeks": {"rows": [{"legId": "leg_1", "delta": -0.34, "gamma": 0.02, "theta": -0.03, "vega": 0.11}]},
            "assumptions": ["Black-Scholes local deterministic placeholder valuation."],
            "warnings": ["Valuation output is descriptive research evidence only."],
            "modelProvenance": {"adapter": "local-scenario-valuation", "version": "v0"}
        });

        let envelope = super::scenario_report_envelope(
            &surface,
            "strat_local_btc_demo",
            "http://127.0.0.1:3001",
            ".tradeassembly/tradeassembly.db",
        );

        assert_eq!(envelope["kind"], "scenario_valuation");
        assert_eq!(envelope["strategyId"], "strat_local_btc_demo");
        assert_eq!(envelope["checkpointId"], "checkpoint_001");
        assert_eq!(
            envelope["deepLinks"]["scenario"]["uri"],
            "tradeassembly://strategy/strat_local_btc_demo/scenario-valuation/checkpoint_001"
        );
        assert_eq!(envelope["views"][0]["id"], "scenario");
        assert_eq!(envelope["charts"][0]["id"], "scenario_matrix");
        assert_eq!(envelope["charts"][1]["id"], "greeks_table");
        assert_eq!(
            envelope["charts"][0]["data"]["rows"][0]["scenario"],
            "underlying_down_2pct"
        );
        assert_eq!(
            envelope["assumptions"][0],
            "Black-Scholes local deterministic placeholder valuation."
        );
        assert_eq!(
            envelope["modelProvenance"]["adapter"],
            "local-scenario-valuation"
        );
        assert!(envelope["exportRefs"]["csv"]["uri"]
            .as_str()
            .unwrap()
            .contains("checkpoint_001"));
        assert!(envelope["replayRefs"]["commands"][0]
            .as_str()
            .unwrap()
            .starts_with("tradeassembly scenario valuation --db "));
        assert!(envelope["warnings"][0]
            .as_str()
            .unwrap()
            .contains("did not recommend"));
        assert!(!envelope.to_string().to_lowercase().contains("should buy"));
    }

    #[test]
    fn legacy_scenario_surface_never_emits_fixture_prices_or_greeks() {
        let surface = super::scenario_valuation_surface(
            "strategy-1",
            "checkpoint-1",
            "http://127.0.0.1:3001",
            ".tradeassembly/tradeassembly.db",
        );

        assert_eq!(surface["ok"], false);
        assert_eq!(
            surface["error"]["code"],
            "scenario_valuation_requires_durable_derivatives_analysis"
        );
        assert_eq!(
            surface["scenarioMatrix"]["rows"].as_array().map(Vec::len),
            Some(0)
        );
        assert_eq!(surface["greeks"]["rows"].as_array().map(Vec::len), Some(0));
        assert!(!surface.to_string().contains("121.5"));
        assert!(!surface.to_string().contains("-0.34"));
    }

    #[test]
    fn legacy_monte_carlo_surface_never_emits_request_shaped_results() {
        let surface = super::monte_carlo_surface(
            &serde_json::json!({"strategyId": "strat_local_btc_demo"}),
            "http://127.0.0.1:3001",
            ".tradeassembly/tradeassembly.db",
        );
        assert_eq!(surface["ok"], false);
        assert_eq!(surface["kind"], "monte_carlo");
        assert_eq!(
            surface["error"]["code"],
            "monte_carlo_requires_durable_robustness_run"
        );
        assert_eq!(surface["simulationResult"], serde_json::json!({}));
        assert!(surface.get("reportEnvelope").is_none());
        assert!(!surface.to_string().contains("api_secret"));
        assert!(!surface.to_string().to_lowercase().contains("should buy"));
    }

    fn sample_selector_run() -> serde_json::Value {
        serde_json::json!({
            "schema_version": "tradeassembly.selector.v1",
            "run_id": "selector_run_1",
            "selector": {
                "schema_version": "tradeassembly.selector.v1",
                "selector_id": "sel_spy_put_30d",
                "underlying_symbol": "SPY",
                "right": "put",
                "min_dte": 30,
                "max_dte": 45,
                "target_delta": -0.35,
                "min_delta": -0.4,
                "max_delta": -0.25,
                "max_bid_ask_width": 0.2,
                "min_open_interest": 100,
                "min_volume": 10,
                "max_snapshot_age_seconds": 300,
                "required_provider_capabilities": ["options_chain"]
            },
            "snapshot_ref": "snapshot://local/snap_001",
            "accepted_candidates": [
                {
                    "candidate_id": "candidate_accepted",
                    "status": "accepted",
                    "contract": selector_contract("occ:SPY260717P00520000", 520.0, -0.34),
                    "reason_codes": ["accepted"],
                    "explanations": ["Candidate satisfied the user-defined selector filters."],
                    "data_assumptions": [],
                    "warnings": [],
                    "provenance": {"source": "local_selector", "rawSecretsExposed": false},
                    "replay_ref": "tradeassembly://selector-runs/selector_run_1/candidates/candidate_accepted"
                }
            ],
            "rejected_candidates": [
                {
                    "candidate_id": "candidate_rejected",
                    "status": "rejected",
                    "contract": selector_contract("occ:SPY260717P00500000", 500.0, -0.12),
                    "reason_codes": ["outside_delta"],
                    "explanations": ["Delta -0.12 was above max -0.25."],
                    "data_assumptions": [],
                    "warnings": ["Rejected fail-closed; TradeAssembly did not substitute another contract."],
                    "provenance": {"source": "local_selector", "rawSecretsExposed": false},
                    "replay_ref": "tradeassembly://selector-runs/selector_run_1/candidates/candidate_rejected"
                }
            ],
            "provider_data_assumptions": ["Selection used local snapshot fields and provider capability metadata only."],
            "warnings": ["Selectors are deterministic filters over user-defined criteria. TradeAssembly did not recommend what to trade, when to trade, or how much to trade."],
            "replay_refs": {
                "commands": ["tradeassembly selector replay --run-id selector_run_1 --snapshot snapshot://local/snap_001"],
                "snapshotRef": "snapshot://local/snap_001",
                "deterministic": true
            },
            "export_refs": {
                "json": {"kind": "json", "path": "tradeassembly://selector-runs/selector_run_1/report.json"},
                "csv": {"kind": "csv", "path": "tradeassembly://selector-runs/selector_run_1/candidates.csv"}
            },
            "reasonCodeSummary": {"accepted": 1, "outside_delta": 1},
            "notice": "Selectors are deterministic filters over user-defined criteria. TradeAssembly did not recommend what to trade, when to trade, or how much to trade.",
            "redactedSecretExample": "SUPER-SECRET-VALUE"
        })
    }

    fn selector_contract(contract_id: &str, strike: f64, delta: f64) -> serde_json::Value {
        serde_json::json!({
            "canonical_contract_id": contract_id,
            "provider_aliases": {"alpaca-paper": contract_id},
            "underlying_symbol": "SPY",
            "symbol": contract_id,
            "right": "put",
            "strike": strike,
            "expiration": "2026-07-17",
            "dte": 34,
            "bid": 4.1,
            "ask": 4.2,
            "mid": 4.15,
            "last": 4.15,
            "open_interest": 950,
            "volume": 110,
            "iv": 0.22,
            "delta": delta,
            "gamma": 0.02,
            "theta": -0.03,
            "vega": 0.11,
            "rho": -0.02,
            "spread_width": 0.1,
            "as_of": "2026-06-23T13:59:00Z",
            "source_provider": "alpaca-paper",
            "supported_capabilities": ["options_chain"],
            "support_status": "supported",
            "provenance": {"snapshot": "fixture", "rawSecretsExposed": false}
        })
    }
}
