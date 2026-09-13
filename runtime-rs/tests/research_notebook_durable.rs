// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use tradeassembly_runtime::backtest_contracts::canonical_hash;
use tradeassembly_runtime::historical_data::{
    AssetClass, BarObservation, DatasetNormalizationPolicy, DatasetQualityPolicy, DatasetSnapshot,
    DatasetSnapshotContent, DatasetSourceClass, DatasetSourceManifest, DatasetTimeSlice,
    HistoricalDataKind, HistoricalObservation, HistoricalObservationData, QualityDisposition,
    DATASET_SNAPSHOT_SCHEMA,
};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::robustness_engine::{MonteCarloInput, MonteCarloMode, StudyInput};
use tradeassembly_runtime::service::TradeAssemblyService;

const UNIT: i64 = 1_000_000;

struct Fixture {
    _directory: TempDir,
    service: TradeAssemblyService,
    backtest_request: Value,
}

#[test]
fn notebook_composes_and_replays_server_resolved_durable_artifacts() {
    let fixture = fixture("notebook-durable");
    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.backtest_request);
    assert_eq!(created.status, 202, "{created:#?}");
    let backtest_id = text(&created.body, "runId");
    let completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"notebook-durable-worker"}),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");

    let robustness = fixture.service.handle_http(
        "POST",
        "/robustness-runs",
        json!({
            "sourceRunId": backtest_id,
            "studyKind": "monte_carlo",
            "assumptions": StudyInput::MonteCarlo(MonteCarloInput {
                path_count: 32,
                confidence_level: 0.9,
                ruin_equity_ratio: 0.5,
                mode: MonteCarloMode::TradeBootstrap,
            }),
            "budget": {
                "maximumSamples": 32,
                "maximumGridPoints": 10,
                "maximumWindows": 10,
                "maximumScenarios": 10,
                "maximumOutputBytes": 1_000_000,
                "maximumAttempts": 2
            },
            "deterministicSeed": 23,
            "idempotencyKey": "notebook-robustness",
            "client": "self",
            "purpose": "strategy_robustness_research"
        }),
    );
    assert_eq!(robustness.status, 202, "{robustness:#?}");
    let robustness_id = text(&robustness.body, "runId");
    let processed = fixture.service.handle_http(
        "POST",
        "/robustness-runs:process",
        json!({"worker":"notebook-robustness-worker"}),
    );
    assert_eq!(processed.status, 200, "{processed:#?}");

    let derivatives = fixture.service.handle_http(
        "POST",
        "/derivatives-analyses",
        json!({
            "sourceRunId": backtest_id,
            "idempotencyKey": "notebook-derivatives",
            "asOfMs": 1767240000000_i64,
            "scenarios": [{
                "scenarioId": "flat",
                "kind": "hypothetical_stress",
                "underlyingPricesMicros": {"SPY": 97 * UNIT},
                "volatilityShiftsPpm": {},
                "elapsedSeconds": 0
            }]
        }),
    );
    assert_eq!(derivatives.status, 201, "{derivatives:#?}");
    let derivatives_id = text(&derivatives.body, "analysisId");

    let list_query = json!({
        "operationName": "ResearchNotebookList",
        "query": "query ResearchNotebookList { researchNotebookList }",
        "variables": {"strategyId": "strategy-1"}
    });
    let initially_listed = fixture.service.execute_graphql(list_query.clone());
    assert_eq!(
        initially_listed["data"]["researchNotebookList"]["notebooks"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    let comparison = fixture.service.handle_http(
        "POST",
        "/research-comparisons",
        json!({
            "sourceRunIds": [backtest_id, robustness_id],
            "scope": {
                "strategyId": "strategy-1",
                "allowCrossVersion": false,
                "compatibleStrategyIds": []
            },
            "policy": {
                "currency": "warn",
                "instrumentScope": "warn",
                "datasetRange": "warn",
                "strategyVersion": "warn",
                "accounting": "warn",
                "fill": "warn",
                "cost": "warn",
                "studySemantics": "warn"
            },
            "idempotencyKey": "notebook-comparison"
        }),
    );
    assert_eq!(comparison.status, 201, "{comparison:#?}");
    let comparison_id = text(&comparison.body["record"], "comparisonId");

    let compose_variables = json!({
        "strategyId": "strategy-1",
        "title": "Durable notebook",
        "artifactSelectors": [
                {"kind":"derivatives", "id": derivatives_id, "contentHash":"attacker-value"},
                {"kind":"comparison", "id": comparison_id, "refUri":"tradeassembly://attacker"},
                {"kind":"robustness", "id": robustness_id},
                {"kind":"backtest", "id": backtest_id}
        ]
    });
    let cross_strategy = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId": "strategy-other",
            "artifactSelectors": [{"kind":"backtest", "id": backtest_id}]
        }),
    );
    assert_eq!(
        cross_strategy.body["code"],
        "research_notebook_artifact_strategy_or_integrity_failed"
    );
    let graphql_compose = fixture.service.execute_graphql(json!({
        "operationName": "ResearchNotebookCompose",
        "query": "mutation ResearchNotebookCompose { researchNotebookCompose }",
        "variables": compose_variables
    }));
    let compose = &graphql_compose["data"]["researchNotebookCompose"];
    assert_eq!(compose["ok"], true, "{graphql_compose:#?}");
    assert_eq!(compose["artifactRefs"].as_array().map(Vec::len), Some(4));
    assert!(!compose.to_string().contains("attacker-value"));
    assert!(!compose.to_string().contains("tradeassembly://attacker"));

    let graphql_listed = fixture.service.execute_graphql(list_query);
    assert_eq!(
        graphql_listed["data"]["researchNotebookList"]["notebooks"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "repeated GraphQL reads must reflect current durable state: {graphql_listed:#?}"
    );

    let notebook_id = text(compose, "notebookId");
    let http_compose = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId": "strategy-1",
            "title": "Durable notebook",
            "artifactSelectors": [
                {"kind":"backtest", "id": backtest_id},
                {"kind":"robustness", "id": robustness_id},
                {"kind":"comparison", "id": comparison_id},
                {"kind":"derivatives", "id": derivatives_id}
            ]
        }),
    );
    assert_eq!(http_compose.body["notebookId"], notebook_id);
    let listed = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/list",
        json!({"strategyId":"strategy-1"}),
    );
    assert_eq!(listed.body["notebooks"].as_array().map(Vec::len), Some(1));
    let inspected = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/inspect",
        json!({"strategyId":"strategy-1", "notebookId": notebook_id}),
    );
    assert_eq!(
        inspected.body["notebook"]["notebookHash"],
        compose["notebookHash"]
    );
    let mcp_inspected = fixture.service.call_mcp_tool(
        "tradeassembly.research_notebook.inspect",
        json!({"strategy_id":"strategy-1", "notebook_id": notebook_id}),
    );
    assert_eq!(
        mcp_inspected["structuredContent"]["notebook"]["notebookHash"], compose["notebookHash"],
        "{mcp_inspected:#?}"
    );
    let replay = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/replay",
        json!({"strategyId":"strategy-1", "notebookId": notebook_id}),
    );
    assert_eq!(replay.body["status"], "verified", "{replay:#?}");
    let exported = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/export",
        json!({"strategyId":"strategy-1", "notebookId": notebook_id}),
    );
    assert_eq!(exported.body["ok"], true, "{exported:#?}");
    assert_eq!(
        exported.body["export"]["contentHash"],
        compose["exportRefs"]["json"]["contentHash"]
    );
    let stored_export = fixture
        .service
        .runtime()
        .storage
        .get_json("research_notebook_exports", &notebook_id)
        .unwrap()
        .unwrap();
    let mut tampered_export = stored_export.clone();
    tampered_export["attackerPayload"] = json!("substituted export");
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "research_notebook_exports",
            &notebook_id,
            tampered_export,
            &context("notebook-export-tamper"),
        )
        .unwrap();
    let failed_export = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/export",
        json!({"strategyId":"strategy-1", "notebookId": notebook_id}),
    );
    assert_eq!(
        failed_export.body["code"],
        "research_notebook_export_integrity_failed"
    );
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "research_notebook_exports",
            &notebook_id,
            stored_export,
            &context("notebook-export-restore"),
        )
        .unwrap();

    let stored_notebook = fixture
        .service
        .runtime()
        .storage
        .get_json("research_notebooks", &notebook_id)
        .unwrap()
        .unwrap();
    let mut tampered_notebook = stored_notebook.clone();
    tampered_notebook["artifactRefs"][0]["refUri"] = json!("tradeassembly://attacker");
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "research_notebooks",
            &notebook_id,
            tampered_notebook,
            &context("notebook-record-tamper"),
        )
        .unwrap();
    let failed_inspect = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/inspect",
        json!({"strategyId":"strategy-1", "notebookId": notebook_id}),
    );
    assert_eq!(
        failed_inspect.body["code"],
        "research_notebook_integrity_failed"
    );
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "research_notebooks",
            &notebook_id,
            stored_notebook,
            &context("notebook-record-restore"),
        )
        .unwrap();

    let stored_derivatives = fixture
        .service
        .runtime()
        .storage
        .get_json("derivatives_analyses_v1", &derivatives_id)
        .unwrap()
        .unwrap();
    let mut tampered = stored_derivatives.clone();
    tampered["analysis"]["outputHash"] = json!("sha256:tampered");
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "derivatives_analyses_v1",
            &derivatives_id,
            tampered,
            &context("notebook-tamper"),
        )
        .unwrap();
    let failed_replay = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/replay",
        json!({"strategyId":"strategy-1", "notebookId": notebook_id}),
    );
    assert_eq!(
        failed_replay.body["code"], "research_notebook_artifact_integrity_failed",
        "{failed_replay:#?}"
    );
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "derivatives_analyses_v1",
            &derivatives_id,
            stored_derivatives,
            &context("notebook-derivatives-restore"),
        )
        .unwrap();

    let robustness_only = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId":"strategy-1",
            "artifactSelectors":[{"kind":"robustness","id":robustness_id}]
        }),
    );
    assert_eq!(robustness_only.body["ok"], true, "{robustness_only:#?}");
    let derivatives_only = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId":"strategy-1",
            "artifactSelectors":[{"kind":"derivatives","id":derivatives_id}]
        }),
    );
    assert_eq!(derivatives_only.body["ok"], true, "{derivatives_only:#?}");

    let stored_result = fixture
        .service
        .runtime()
        .storage
        .get_json("backtest_results_v1", &backtest_id)
        .unwrap()
        .unwrap();
    let mut tampered_result = stored_result.clone();
    tampered_result["content"]["accounting"]["cashMicros"] = json!(123);
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "backtest_results_v1",
            &backtest_id,
            tampered_result,
            &context("notebook-source-tamper"),
        )
        .unwrap();
    for derived_notebook in [&robustness_only, &derivatives_only] {
        let failed = fixture.service.handle_http(
            "POST",
            "/product/strategies/research-notebook/replay",
            json!({
                "strategyId":"strategy-1",
                "notebookId":derived_notebook.body["notebookId"]
            }),
        );
        assert_eq!(
            failed.body["code"], "research_notebook_artifact_integrity_failed",
            "{failed:#?}"
        );
    }
}

#[test]
fn notebook_rejects_duplicate_selectors_before_persistence() {
    let service = TradeAssemblyService::test_local(
        tempfile::tempdir()
            .unwrap()
            .path()
            .join("notebook.db")
            .display()
            .to_string(),
    );
    let duplicate = service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId":"strategy-1",
            "artifactSelectors":[
                {"kind":"backtest","id":"missing"},
                {"kind":"backtest","id":"missing"}
            ]
        }),
    );
    assert_eq!(
        duplicate.body["code"],
        "research_notebook_duplicate_selector"
    );
    let listed = service.handle_http(
        "POST",
        "/product/strategies/research-notebook/list",
        json!({"strategyId":"strategy-1"}),
    );
    assert_eq!(listed.body["notebooks"].as_array().map(Vec::len), Some(0));

    let missing = service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId":"strategy-1",
            "artifactSelectors":[{"kind":"backtest","id":"missing"}]
        }),
    );
    assert_eq!(missing.body["code"], "research_notebook_artifact_not_found");
    let unsupported = service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId":"strategy-1",
            "artifactSelectors":[{"kind":"external","id":"artifact-1"}]
        }),
    );
    assert_eq!(
        unsupported.body["code"],
        "research_notebook_selector_kind_invalid"
    );
    let still_empty = service.handle_http(
        "POST",
        "/product/strategies/research-notebook/list",
        json!({"strategyId":"strategy-1"}),
    );
    assert_eq!(
        still_empty.body["notebooks"].as_array().map(Vec::len),
        Some(0)
    );
}

#[test]
fn notebook_rejects_incomplete_backtest_before_persistence() {
    let fixture = fixture("notebook-incomplete");
    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.backtest_request);
    assert_eq!(created.status, 202, "{created:#?}");
    let backtest_id = text(&created.body, "runId");
    let compose = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/compose",
        json!({
            "strategyId":"strategy-1",
            "artifactSelectors":[{"kind":"backtest","id":backtest_id}]
        }),
    );
    assert_eq!(
        compose.body["code"],
        "research_notebook_artifact_incomplete"
    );
    let listed = fixture.service.handle_http(
        "POST",
        "/product/strategies/research-notebook/list",
        json!({"strategyId":"strategy-1"}),
    );
    assert_eq!(listed.body["notebooks"].as_array().map(Vec::len), Some(0));
}

fn fixture(name: &str) -> Fixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let db = directory
        .path()
        .join(format!("{name}.db"))
        .display()
        .to_string();
    let service = TradeAssemblyService::test_local(&db);
    let mut spec: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/strategy-spec/v3/valid/static-equity.json"),
        )
        .expect("strategy fixture"),
    )
    .expect("strategy JSON");
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.expression":"bar.close > 101000000","tradeassembly.quantity":UNIT});
    spec["stages"]["exit_policy"]["substeps"][0]["config"]["tradeassembly.expression"] =
        json!("bar.close < 100000000");
    let spec_hash = canonical_hash(&spec, "strategy hash").expect("strategy hash");
    service.runtime().storage.put_json(
        "strategy_versions", "strategy-1:1",
        json!({"id":"version-1","strategyId":"strategy-1","version":1,"specHash":spec_hash,"spec":spec,"published":true,"immutable":true}),
        &context("notebook-strategy"),
    ).expect("strategy version");
    let snapshot = DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: Some("version-1".to_string()),
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: "local-data".to_string(),
                plugin_ref: "tradeassembly.local-data".to_string(),
                operation_id: "marketdata.bars.read_v1".to_string(),
                plugin_manifest_fingerprint: "sha256:plugin".to_string(),
                capability_graph_revision_id: "graph-1".to_string(),
                capability_graph_fingerprint: "sha256:graph".to_string(),
                source_request_hash: "sha256:request".to_string(),
                upstream_refs: BTreeMap::new(),
            },
            asset_classes: vec![AssetClass::Equity],
            instruments: vec!["SPY".to_string()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1h".to_string(),
            time_slice: DatasetTimeSlice {
                start: "2026-01-01T00:00:00Z".to_string(),
                end: "2026-01-01T04:00:00Z".to_string(),
            },
            calendar: "XNYS".to_string(),
            timezone: "UTC".to_string(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".to_string(),
                timezone: "UTC".to_string(),
                duplicate_policy: "reject".to_string(),
                price_adjustment: "none".to_string(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: QualityDisposition::Reject,
                stale_observations: QualityDisposition::Reject,
                outliers: QualityDisposition::Allow,
                invalid_markets: QualityDisposition::Reject,
                calendar_mismatch: QualityDisposition::Reject,
                corporate_action_gaps: QualityDisposition::Reject,
                missing_derivative_fields: QualityDisposition::Allow,
            },
            quality_findings: vec![],
            observations: vec![
                bar("2026-01-01T00:00:00Z", 100.0, 102.0),
                bar("2026-01-01T01:00:00Z", 103.0, 104.0),
                bar("2026-01-01T02:00:00Z", 99.0, 98.0),
                bar("2026-01-01T03:00:00Z", 97.0, 97.0),
            ],
        },
        1,
    )
    .expect("snapshot");
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context("notebook-dataset"))
        .expect("dataset snapshot");
    Fixture {
        _directory: directory,
        service,
        backtest_request: json!({"strategyId":"strategy-1","strategyVersionId":"version-1","datasetId":snapshot.dataset_id,"startingCashMicros":1_000 * UNIT,"fillTiming":"next_bar","fillPriceSource":"open","endOfData":"mark_to_market","fixedFeeMicros":0,"perUnitFeeMicros":0,"notionalFeeBps":0,"spreadBps":0,"slippageBps":0,"maximumParticipationBps":10_000,"minimumVolumeMicros":UNIT,"maximumPositionNotionalMicros":1_000 * UNIT,"maximumOrderQuantityMicros":10 * UNIT,"client":"self","purpose":"strategy_backtest_research","idempotencyKey":format!("{name}-create")}),
    }
}

fn bar(timestamp: &str, open: f64, close: f64) -> HistoricalObservation {
    HistoricalObservation {
        instrument_id: "SPY".to_string(),
        timestamp: timestamp.to_string(),
        data: HistoricalObservationData::Bar(BarObservation {
            open,
            high: open.max(close),
            low: open.min(close),
            close,
            volume: 100.0,
        }),
    }
}

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).unwrap(),
    )
}

fn text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing {key}: {value:#}"))
        .to_string()
}
