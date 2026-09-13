use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use tradeassembly_runtime::backtest_contracts::canonical_hash;
use tradeassembly_runtime::cli::{execute_backtests_command, BacktestsCommand};
use tradeassembly_runtime::historical_data::{
    AssetClass, BarObservation, DatasetNormalizationPolicy, DatasetQualityPolicy, DatasetSnapshot,
    DatasetSnapshotContent, DatasetSourceClass, DatasetSourceManifest, DatasetTimeSlice,
    HistoricalDataKind, HistoricalObservation, HistoricalObservationData, QualityDisposition,
    DATASET_SNAPSHOT_SCHEMA,
};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::robustness_engine::{
    CostCase, CostSensitivityInput, MonteCarloInput, MonteCarloMode, StudyInput,
};
use tradeassembly_runtime::service::TradeAssemblyService;

const UNIT: i64 = 1_000_000;

struct Fixture {
    _directory: TempDir,
    db: String,
    service: TradeAssemblyService,
    request: Value,
}

#[test]
fn durable_robustness_run_executes_real_backtest_evidence_and_survives_restart() {
    let fixture = fixture("robustness-durable");
    let created_backtest =
        fixture
            .service
            .handle_http("POST", "/backtests", fixture.request.clone());
    assert_eq!(created_backtest.status, 202, "{created_backtest:#?}");
    let backtest_id = text(&created_backtest.body, "runId");
    let completed_backtest = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"robustness-source-worker"}),
    );
    assert_eq!(completed_backtest.status, 200, "{completed_backtest:#?}");
    assert_eq!(completed_backtest.body["status"], "completed");

    let assumptions = serde_json::to_value(StudyInput::MonteCarlo(MonteCarloInput {
        path_count: 128,
        confidence_level: 0.9,
        ruin_equity_ratio: 0.5,
        mode: MonteCarloMode::TradeBootstrap,
    }))
    .expect("typed assumptions");
    let created = fixture.service.handle_http(
        "POST",
        "/robustness-runs",
        json!({
            "sourceRunId": backtest_id,
            "studyKind": "monte_carlo",
            "assumptions": assumptions,
            "budget": {
                "maximumSamples": 128,
                "maximumGridPoints": 10,
                "maximumWindows": 10,
                "maximumScenarios": 10,
                "maximumOutputBytes": 1000000,
                "maximumAttempts": 2
            },
            "deterministicSeed": 23,
            "idempotencyKey": "robustness-durable-create",
            "client": "self",
            "purpose": "strategy_robustness_research"
        }),
    );
    assert_eq!(created.status, 202, "{created:#?}");
    assert_eq!(created.body["state"], "queued");
    let robustness_id = text(&created.body, "runId");
    assert_eq!(
        created.body["manifest"]["content"]["source"]["sourceRunId"],
        backtest_id
    );

    let completed = fixture.service.handle_http(
        "POST",
        "/robustness-runs:process",
        json!({"worker":"robustness-durable-worker"}),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");
    assert_eq!(completed.body["state"], "completed");
    assert!(completed.body["resultHash"].as_str().is_some());
    assert_eq!(
        completed.body["result"]["content"]["source"]["sourceRunId"],
        backtest_id
    );

    let reopened = TradeAssemblyService::test_local(&fixture.db);
    let fetched = reopened.handle_http(
        "GET",
        &format!("/robustness-runs/{robustness_id}"),
        json!({}),
    );
    assert_eq!(fetched.status, 200, "{fetched:#?}");
    assert_eq!(fetched.body["resultHash"], completed.body["resultHash"]);
    assert_eq!(
        fetched.body["manifest"]["content"]["source"]["sourceResultHash"],
        completed_backtest.body["resultHash"]
    );
    let report = reopened.handle_http(
        "GET",
        &format!("/robustness-runs/{robustness_id}/report"),
        json!({}),
    );
    assert_eq!(report.status, 200, "{report:#?}");
    assert_eq!(
        report.body["result"]["resultHash"],
        completed.body["resultHash"]
    );
    let replay = reopened.handle_http(
        "POST",
        &format!("/robustness-runs/{robustness_id}/replay"),
        json!({"idempotencyKey":"robustness-durable-replay"}),
    );
    assert_eq!(replay.status, 200, "{replay:#?}");
    assert_eq!(replay.body["status"], "verified");
    assert_eq!(replay.body["resultHash"], completed.body["resultHash"]);
    for format in ["json", "csv"] {
        let exported = reopened.handle_http(
            "POST",
            &format!("/robustness-runs/{robustness_id}/export"),
            json!({
                "format":format,
                "idempotencyKey":format!("robustness-durable-export-{format}")
            }),
        );
        assert_eq!(exported.status, 200, "{exported:#?}");
        assert_eq!(exported.body["format"], format);
        assert_eq!(exported.body["resultHash"], completed.body["resultHash"]);
        assert!(exported.body["artifact"]["sha256"]
            .as_str()
            .is_some_and(
                |value| value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
            ));
        assert!(exported.body["artifact"]["artifactRef"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256/")));
    }
    let graphql_report = reopened.execute_graphql(json!({
        "operationName":"RobustnessReport",
        "query":"query RobustnessReport($runId: String!) { robustnessReport(runId: $runId) }",
        "variables":{"runId":robustness_id}
    }));
    assert!(
        graphql_report.get("errors").is_none(),
        "{graphql_report:#?}"
    );
    assert_eq!(
        graphql_report["data"]["robustnessReport"]["result"]["resultHash"],
        completed.body["resultHash"]
    );
    let graphql_replay = reopened.execute_graphql(json!({
        "operationName":"ReplayRobustnessRun",
        "query":"mutation ReplayRobustnessRun($runId: String!, $idempotencyKey: String) { replayRobustnessRun(runId: $runId, idempotencyKey: $idempotencyKey) }",
        "variables":{"runId":robustness_id,"idempotencyKey":"robustness-graphql-replay"}
    }));
    assert!(
        graphql_replay.get("errors").is_none(),
        "{graphql_replay:#?}"
    );
    assert_eq!(
        graphql_replay["data"]["replayRobustnessRun"]["resultHash"],
        completed.body["resultHash"]
    );
    let graphql_export = reopened.execute_graphql(json!({
        "operationName":"ExportRobustnessRun",
        "query":"mutation ExportRobustnessRun($runId: String!, $format: String, $idempotencyKey: String) { exportRobustnessRun(runId: $runId, format: $format, idempotencyKey: $idempotencyKey) }",
        "variables":{"runId":robustness_id,"format":"json","idempotencyKey":"robustness-graphql-export"}
    }));
    assert!(
        graphql_export.get("errors").is_none(),
        "{graphql_export:#?}"
    );
    assert_eq!(
        graphql_export["data"]["exportRobustnessRun"]["resultHash"],
        completed.body["resultHash"]
    );
    let replayed_process = reopened.handle_http(
        "POST",
        "/robustness-runs:process",
        json!({"worker":"robustness-restart-worker"}),
    );
    assert_eq!(replayed_process.status, 202, "{replayed_process:#?}");
    assert_eq!(replayed_process.body["state"], "idle");
}

#[test]
fn robustness_cost_study_replaces_client_claims_with_verified_source_evidence() {
    let fixture = fixture("robustness-source-provenance");
    let mut baseline_request = fixture.request.clone();
    baseline_request["idempotencyKey"] = json!("robustness-source-baseline");
    baseline_request["slippageBps"] = json!(0);
    let baseline_created = fixture
        .service
        .handle_http("POST", "/backtests", baseline_request);
    assert_eq!(baseline_created.status, 202, "{baseline_created:#?}");
    let baseline_run_id = text(&baseline_created.body, "runId");
    let baseline_completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"robustness-source-worker-1"}),
    );
    assert_eq!(baseline_completed.status, 200, "{baseline_completed:#?}");

    let mut stressed_request = fixture.request.clone();
    stressed_request["idempotencyKey"] = json!("robustness-source-stressed");
    stressed_request["slippageBps"] = json!(40);
    let stressed_created = fixture
        .service
        .handle_http("POST", "/backtests", stressed_request);
    assert_eq!(stressed_created.status, 202, "{stressed_created:#?}");
    let stressed_run_id = text(&stressed_created.body, "runId");
    let stressed_completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"robustness-source-worker-2"}),
    );
    assert_eq!(stressed_completed.status, 200, "{stressed_completed:#?}");

    let assumptions = serde_json::to_value(StudyInput::SlippageSensitivity(CostSensitivityInput {
        baseline_label: "baseline".to_string(),
        cases: vec![
            CostCase {
                source_run_id: baseline_run_id.clone(),
                label: "baseline".to_string(),
                fee_bps: 9_999.0,
                slippage_bps: 9_999.0,
                execution_model: "client-fabricated-model".to_string(),
                trade_returns: vec![0.99],
                evidence_refs: vec!["client-fabricated-evidence".to_string()],
            },
            CostCase {
                source_run_id: stressed_run_id.clone(),
                label: "stressed".to_string(),
                fee_bps: 8_888.0,
                slippage_bps: 8_888.0,
                execution_model: "client-fabricated-model".to_string(),
                trade_returns: vec![0.88],
                evidence_refs: vec!["client-fabricated-evidence".to_string()],
            },
        ],
    }))
    .expect("typed assumptions");
    let created = fixture.service.handle_http(
        "POST",
        "/robustness-runs",
        json!({
            "sourceRunId": baseline_run_id,
            "sourceRunIds": [stressed_run_id],
            "studyKind": "slippage_sensitivity",
            "assumptions": assumptions,
            "budget": {
                "maximumSamples": 100,
                "maximumGridPoints": 10,
                "maximumWindows": 10,
                "maximumScenarios": 10,
                "maximumOutputBytes": 1000000,
                "maximumAttempts": 2
            },
            "deterministicSeed": 31,
            "idempotencyKey": "robustness-source-provenance",
            "client": "self",
            "purpose": "strategy_robustness_research"
        }),
    );
    assert_eq!(created.status, 202, "{created:#?}");

    let completed = fixture.service.handle_http(
        "POST",
        "/robustness-runs:process",
        json!({"worker":"robustness-source-provenance-worker"}),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");
    assert_eq!(completed.body["state"], "completed");
    let engine_result = &completed.body["result"]["content"]["output"];
    assert_eq!(engine_result["status"], "available");
    assert_eq!(
        engine_result["supportingSourceBindings"][0]["runId"],
        stressed_run_id
    );
    let cases = engine_result["output"]["result"]["cases"]
        .as_array()
        .expect("cost sensitivity cases");
    assert_eq!(cases[0]["slippageBps"]["value"], 0.0);
    assert_eq!(cases[1]["slippageBps"]["value"], 40.0);
    assert!(cases
        .iter()
        .all(|case| { case["executionModel"] == "tradeassembly.strategy-kernel.v1" }));
    let result_text = serde_json::to_string(engine_result).expect("result JSON");
    assert!(!result_text.contains("client-fabricated"));
}

#[test]
fn durable_backtest_runs_complete_replay_and_survive_restart() {
    let fixture = fixture("durable");
    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.request.clone());
    assert_eq!(created.status, 202);
    assert_eq!(created.body["status"], "queued");
    let run_id = text(&created.body, "runId");

    let completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({
            "worker": "research-parity-worker",
            "idempotencyKey": "research-parity-process"
        }),
    );
    assert_eq!(completed.status, 200);
    assert_eq!(completed.body["status"], "completed");
    assert_eq!(
        completed.body["result"]["content"]["fills"]
            .as_array()
            .expect("fills")
            .len(),
        2
    );
    assert_eq!(
        completed.body["result"]["content"]["accounting"]["reconciliationResidualMicros"],
        0
    );
    assert_eq!(
        completed.body["events"]
            .as_array()
            .expect("lifecycle events")
            .last()
            .expect("terminal event")["eventType"],
        "backtest_run.completed"
    );

    let listed = fixture.service.handle_http("GET", "/backtests", json!({}));
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["runs"][0]["runId"], run_id);
    assert!(listed.body["runs"][0].get("result").is_none());

    let reopened = TradeAssemblyService::test_local(&fixture.db);
    let fetched = reopened.handle_http("GET", &format!("/backtests/{run_id}"), json!({}));
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body["resultHash"], completed.body["resultHash"]);
    assert_eq!(
        fetched.body["manifest"]["content"]["configuration"]["strategy"]["strategyId"],
        "strategy-1"
    );
    assert_eq!(
        fetched.body["attempts"][0]["state"],
        Value::String("completed".to_string())
    );

    let report = reopened.handle_http("GET", &format!("/backtests/{run_id}/report"), json!({}));
    assert_eq!(report.status, 200, "report projection failed: {report:#?}");
    assert_eq!(report.body["report"]["run"]["runId"], run_id);
    assert_eq!(
        report.body["report"]["overview"]["reconciliationResidualMicros"],
        0
    );
    assert_eq!(
        report.body["report"]["metadata"]["availableExportFormats"],
        json!([
            "manifestJson",
            "reportJson",
            "tradesCsv",
            "ordersFillsCsv",
            "positionsLedgerCsv",
            "equityCsv",
        ])
    );
    let connector = &report.body["report"]["chart"]["connectors"][0];
    assert!(connector["from"]["timestamp"].is_string());
    assert!(connector["to"]["timestamp"].is_string());
    assert!(connector["from"]["price"].is_number());
    assert!(connector["to"]["price"].is_number());
    assert_eq!(connector["tone"], "loss");
    assert_eq!(
        fetched.body["report"]["reportHash"],
        report.body["report"]["reportHash"]
    );
    assert_eq!(
        report.body["report"]["journal"]
            .as_array()
            .expect("report journal")
            .last()
            .expect("completed lifecycle event")["eventType"],
        "backtest_run.completed"
    );
    assert!(report.body["report"]["journal"][0]["evidenceHash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("sha256:")));
    let trade = &report.body["report"]["trades"][0];
    assert_eq!(trade["instrumentType"], "equity");
    assert_eq!(
        trade["instrumentDetails"]["schema"],
        "tradeassembly.instrument.bar.v1"
    );
    assert_eq!(trade["instrumentDetails"]["instrumentId"], "SPY");
    assert_eq!(trade["instrumentDetails"]["assetClass"], "equity");
    assert_eq!(
        trade["instrumentDetails"]["fields"],
        json!([
            {"key":"instrument_id","label":"Instrument","value":"SPY"},
            {"key":"asset_class","label":"Asset class","value":"equity"},
        ])
    );
    let entry_fill_id = trade["entry"]["fillId"].as_str().expect("entry fill ID");
    let exit_fill_id = trade["exit"]["fillId"].as_str().expect("exit fill ID");
    assert_eq!(trade["entry"]["lifecycleState"], "open");
    assert_eq!(trade["exit"]["lifecycleState"], "closed");
    assert!(trade["exit"]["occurredAtMs"].is_i64());
    assert_eq!(trade["orderRefs"], json!([]));
    assert_eq!(
        trade["ledgerRefs"],
        json!([
            format!("{entry_fill_id}:notional"),
            format!("{entry_fill_id}:fee"),
            format!("{exit_fill_id}:notional"),
            format!("{exit_fill_id}:fee"),
        ])
    );

    for export_kind in [
        "manifestJson",
        "reportJson",
        "tradesCsv",
        "ordersFillsCsv",
        "positionsLedgerCsv",
        "equityCsv",
    ] {
        let exported = reopened.handle_http(
            "POST",
            "/product/strategies/backtests/export",
            json!({"backtestId":run_id,"exportKind":export_kind}),
        );
        assert_eq!(
            exported.status, 200,
            "{export_kind} export failed: {exported:#?}"
        );
        assert_eq!(exported.body["backtestId"], run_id);
        assert_eq!(exported.body["exportKind"], export_kind);
        assert!(
            exported.body["contentHash"].as_str().is_some_and(|hash| {
                hash.strip_prefix("sha256:")
                    .is_some_and(|digest| digest.len() == 64)
            }),
            "{export_kind} must carry a canonical content hash: {exported:#?}"
        );
        assert!(
            exported.body.get("payload").is_some(),
            "{export_kind} must contain its durable payload"
        );
        assert!(
            exported.body["payload"]["content"].is_string(),
            "{export_kind} must carry actual content: {exported:#?}"
        );
        assert!(
            exported.body["payload"]["byteLength"].as_u64().is_some(),
            "{export_kind} must carry byte length metadata: {exported:#?}"
        );
        if export_kind == "tradesCsv" {
            assert_eq!(exported.body["payload"]["mediaType"], "text/csv");
            assert!(
                exported.body["payload"]["content"]
                    .as_str()
                    .is_some_and(|content| content.starts_with(
                        "trade_id,symbol,realized,quantity_micros,entry_fill_id,exit_fill_id"
                    )),
                "CSV export must use the documented fill schema: {exported:#?}"
            );
        }
    }

    let replay = reopened.handle_http("POST", &format!("/backtests/{run_id}/replay"), json!({}));
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body["status"], "verified");
    assert_eq!(replay.body["resultHash"], completed.body["resultHash"]);
}

#[test]
fn open_position_report_uses_terminal_dataset_valuation_coordinates() {
    let fixture =
        fixture_with_expressions("open-position", "bar.close > 101000000", "bar.close < 0");
    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.request.clone());
    let run_id = text(&created.body, "runId");
    let completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"open-position-worker","idempotencyKey":"open-position-process"}),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");
    let report =
        fixture
            .service
            .handle_http("GET", &format!("/backtests/{run_id}/report"), json!({}));
    assert_eq!(report.status, 200, "{report:#?}");
    assert_eq!(report.body["report"]["overview"]["openCount"], 1);
    let trade = &report.body["report"]["trades"][0];
    assert_eq!(trade["realized"], false);
    assert!(trade["exit"].is_null());
    assert_eq!(
        trade["terminalValuation"]["timestamp"],
        report.body["report"]["chart"]["range"]["end"]
    );
    let connector = &report.body["report"]["chart"]["connectors"][0];
    assert_eq!(connector["tone"], "unrealized");
    assert_eq!(
        connector["to"]["timestamp"],
        report.body["report"]["chart"]["range"]["end"]
    );
    assert!(connector["to"]["price"].is_number());
    let entry_fill_id = trade["entry"]["fillId"].as_str().expect("entry fill ID");
    assert_eq!(
        trade["ledgerRefs"],
        json!([
            format!("{entry_fill_id}:notional"),
            format!("{entry_fill_id}:fee"),
        ])
    );
}

#[test]
fn no_trade_report_is_empty_without_invented_metrics() {
    let fixture = fixture_with_expressions("no-trade", "bar.close > 999999999", "bar.close < 0");
    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.request.clone());
    let run_id = text(&created.body, "runId");
    let completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"no-trade-worker","idempotencyKey":"no-trade-process"}),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");
    let report =
        fixture
            .service
            .handle_http("GET", &format!("/backtests/{run_id}/report"), json!({}));
    assert_eq!(report.status, 200, "{report:#?}");
    assert_eq!(report.body["report"]["overview"]["tradeCount"], 0);
    assert!(report.body["report"]["overview"].is_object());
    assert!(!report.body["report"]["chart"]["bars"]
        .as_array()
        .expect("chart bars")
        .is_empty());
    assert!(!report.body["report"]["journal"]
        .as_array()
        .expect("lifecycle journal")
        .is_empty());
    assert_eq!(report.body["report"]["trades"], json!([]));
    assert_eq!(report.body["report"]["orders"], json!([]));
    assert_eq!(report.body["report"]["fills"], json!([]));
    assert_eq!(report.body["report"]["positions"], json!([]));
    assert_eq!(report.body["report"]["ledger"], json!([]));
    assert_eq!(report.body["report"]["chart"]["markers"], json!([]));
    assert_eq!(report.body["report"]["chart"]["connectors"], json!([]));
    assert!(report.body["report"]["diagnostics"]
        .as_array()
        .expect("report diagnostics")
        .iter()
        .any(|diagnostic| diagnostic["code"] == "backtest_lifecycle_verified"));
    assert_eq!(
        report.body["report"]["costsAndRisk"]["unavailable"][0]["metric"],
        "max_drawdown_bps"
    );
}

#[test]
fn report_rejects_cross_run_result_substitution_and_exports_are_deterministic() {
    let fixture = fixture("substitution");
    let first = fixture
        .service
        .handle_http("POST", "/backtests", fixture.request.clone());
    let first_id = text(&first.body, "runId");
    let first_complete = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"substitution-worker","idempotencyKey":"substitution-first-process"}),
    );
    assert_eq!(first_complete.status, 200, "{first_complete:#?}");
    let first_export = fixture.service.handle_http(
        "POST",
        &format!("/backtests/{first_id}/export"),
        json!({"exportKind":"reportJson"}),
    );
    let second_export = fixture.service.handle_http(
        "POST",
        &format!("/backtests/{first_id}/export"),
        json!({"exportKind":"reportJson"}),
    );
    assert_eq!(first_export.status, 200, "{first_export:#?}");
    assert_eq!(
        first_export.body["contentHash"],
        second_export.body["contentHash"]
    );
    assert_eq!(
        first_export.body["payload"]["content"],
        second_export.body["payload"]["content"]
    );

    let mut second_request = fixture.request.clone();
    second_request["idempotencyKey"] = json!("substitution-second-create");
    let second = fixture
        .service
        .handle_http("POST", "/backtests", second_request);
    let second_id = text(&second.body, "runId");
    let second_complete = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"substitution-worker","idempotencyKey":"substitution-second-process"}),
    );
    assert_eq!(second_complete.status, 200, "{second_complete:#?}");
    let result = fixture
        .service
        .runtime()
        .backtests
        .get_result(&second_id)
        .expect("result read")
        .expect("result");
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "backtest_results_v1",
            &first_id,
            serde_json::to_value(result).expect("result JSON"),
            &context("substitution-tamper"),
        )
        .expect("substitution write");
    let substituted =
        fixture
            .service
            .handle_http("GET", &format!("/backtests/{first_id}/report"), json!({}));
    assert_eq!(substituted.status, 409, "{substituted:#?}");
    assert_eq!(
        substituted.body["error"]["code"],
        "backtest_result_integrity_failed"
    );
}

#[test]
fn workspace_withholds_raw_result_when_report_integrity_fails() {
    let fixture = fixture("workspace-integrity");
    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.request.clone());
    let run_id = text(&created.body, "runId");
    let completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"workspace-integrity-worker","idempotencyKey":"workspace-integrity-process"}),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");

    let dataset_id = fixture.request["datasetId"]
        .as_str()
        .expect("fixture dataset ID");
    let mut snapshot = fixture
        .service
        .runtime()
        .dataset_snapshots
        .get(dataset_id)
        .expect("snapshot read")
        .expect("snapshot");
    let HistoricalObservationData::Bar(bar) = &mut snapshot.content.observations[0].data else {
        panic!("bar required")
    };
    bar.close = 999.0;
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "dataset_snapshots_v1",
            dataset_id,
            serde_json::to_value(snapshot).expect("snapshot JSON"),
            &context("workspace-tamper-snapshot"),
        )
        .expect("tampered storage write");

    let report =
        fixture
            .service
            .handle_http("GET", &format!("/backtests/{run_id}/report"), json!({}));
    assert_eq!(report.status, 409, "{report:#?}");
    assert_eq!(
        report.body["error"]["code"],
        "dataset_snapshot_integrity_failed"
    );
    let workspace = fixture.service.execute_graphql(json!({
        "operationName": "StrategyResearchWorkspace",
        "query": "query StrategyResearchWorkspace($strategyId: String!) { strategyResearchWorkspace(strategyId: $strategyId) }",
        "variables": {"strategyId": "strategy-1"}
    }));
    let workspace_run = workspace["data"]["strategyResearchWorkspace"]["backtests"]
        .as_array()
        .expect("workspace backtests")
        .iter()
        .find(|run| run["runId"] == run_id)
        .unwrap_or_else(|| {
            panic!("tampered run remains visible with an explicit integrity error: {workspace:#}")
        });
    assert_eq!(
        workspace_run["reportIntegrityError"],
        "dataset_snapshot_integrity_failed"
    );
    assert!(workspace_run["report"].is_null());
    assert!(workspace_run["result"].is_null());
}

#[test]
fn backtest_lifecycle_is_consistent_across_graphql_mcp_and_cli() {
    let fixture = fixture("surface-parity");
    let created = fixture.service.handle_http(
        "POST",
        "/graphql",
        json!({
            "operationName": "RunBacktest",
            "query": "mutation RunBacktest($request: JSON) { runBacktest(request: $request) }",
            "variables": {"request": fixture.request}
        }),
    );
    assert_eq!(created.status, 200);
    assert!(
        created.body.get("errors").is_none(),
        "GraphQL create failed: {created:#?}"
    );
    let run_id = text(&created.body["data"]["runBacktest"], "runId");

    let processed = fixture.service.call_mcp_tool(
        "tradeassembly.backtest.process",
        json!({
            "worker": "surface-parity-worker",
            "idempotency_key": "surface-parity-process"
        }),
    );
    assert_eq!(processed["isError"], false);
    assert_eq!(
        processed["structuredContent"]["status"],
        Value::String("completed".to_string())
    );

    let fetched = fixture
        .service
        .call_mcp_tool("tradeassembly.backtest.get", json!({"run_id": run_id}));
    assert_eq!(fetched["isError"], false);
    assert_eq!(
        fetched["structuredContent"]["resultHash"],
        processed["structuredContent"]["resultHash"]
    );

    let listed = execute_backtests_command(&fixture.service, BacktestsCommand::List);
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["runs"][0]["runId"], run_id);

    let replay = fixture.service.handle_http(
        "POST",
        "/graphql",
        json!({
            "operationName": "ReplayBacktest",
            "query": "mutation ReplayBacktest($runId: String!) { replayBacktest(runId: $runId) }",
            "variables": {"runId": run_id}
        }),
    );
    assert_eq!(replay.status, 200);
    assert!(
        replay.body.get("errors").is_none(),
        "GraphQL replay failed: {replay:#?}"
    );
    assert_eq!(
        replay.body["data"]["replayBacktest"]["status"],
        Value::String("verified".to_string())
    );

    let graphql_report = fixture.service.handle_http(
        "POST",
        "/graphql",
        json!({
            "operationName": "BacktestReport",
            "query": "query BacktestReport($runId: String!) { backtestReport(runId: $runId) }",
            "variables": {"runId": run_id}
        }),
    );
    assert_eq!(graphql_report.status, 200, "{graphql_report:#?}");
    let mcp_report = fixture.service.call_mcp_tool(
        "tradeassembly.backtest.report",
        json!({"backtest_id": run_id}),
    );
    assert_eq!(mcp_report["isError"], false, "{mcp_report:#?}");
    let cli_report = execute_backtests_command(
        &fixture.service,
        BacktestsCommand::Report {
            run_id: run_id.clone(),
        },
    );
    assert_eq!(cli_report.status, 200, "{cli_report:#?}");
    let graphql_hash = &graphql_report.body["data"]["backtestReport"]["report"]["reportHash"];
    assert_eq!(
        graphql_hash,
        &mcp_report["structuredContent"]["report"]["reportHash"]
    );
    assert_eq!(graphql_hash, &cli_report.body["report"]["reportHash"]);

    let graphql_export = fixture.service.handle_http(
        "POST",
        "/graphql",
        json!({
            "operationName": "BacktestReportExport",
            "query": "mutation BacktestReportExport($backtestId: String!, $exportKind: String) { backtestReportExport(backtestId: $backtestId, exportKind: $exportKind) }",
            "variables": {"backtestId": run_id, "exportKind": "tradesCsv"}
        }),
    );
    assert_eq!(graphql_export.status, 200, "{graphql_export:#?}");
    let mcp_export = fixture.service.call_mcp_tool(
        "tradeassembly.backtest.export",
        json!({"backtest_id": run_id, "export_kind": "tradesCsv"}),
    );
    assert_eq!(mcp_export["isError"], false, "{mcp_export:#?}");
    let cli_export = execute_backtests_command(
        &fixture.service,
        BacktestsCommand::Export {
            run_id,
            export_kind: "tradesCsv".to_string(),
        },
    );
    assert_eq!(cli_export.status, 200, "{cli_export:#?}");
    let graphql_export_hash = &graphql_export.body["data"]["backtestReportExport"]["contentHash"];
    assert_eq!(
        graphql_export_hash,
        &mcp_export["structuredContent"]["contentHash"]
    );
    assert_eq!(graphql_export_hash, &cli_export.body["contentHash"]);
}

#[test]
fn missing_or_tampered_immutable_inputs_fail_closed() {
    let fixture = fixture("integrity");
    let mut missing_dataset = fixture.request.clone();
    missing_dataset["datasetId"] = json!("dataset_missing");
    missing_dataset["idempotencyKey"] = json!("missing-dataset");
    let missing = fixture
        .service
        .handle_http("POST", "/backtests", missing_dataset);
    assert_eq!(missing.status, 400);
    assert_eq!(
        missing.body["error"]["code"],
        "backtest_dataset_unavailable"
    );

    let mut missing_version = fixture.request.clone();
    missing_version["strategyVersionId"] = json!("version-missing");
    missing_version["idempotencyKey"] = json!("missing-version");
    let missing = fixture
        .service
        .handle_http("POST", "/backtests", missing_version);
    assert_eq!(missing.status, 400);
    assert_eq!(
        missing.body["error"]["code"],
        "backtest_strategy_version_required"
    );

    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.request.clone());
    let run_id = text(&created.body, "runId");
    let mut snapshot = fixture
        .service
        .runtime()
        .dataset_snapshots
        .get(
            fixture.request["datasetId"]
                .as_str()
                .expect("fixture dataset ID"),
        )
        .expect("snapshot read")
        .expect("snapshot");
    let HistoricalObservationData::Bar(bar) = &mut snapshot.content.observations[0].data else {
        panic!("bar required")
    };
    bar.close = 999.0;
    fixture
        .service
        .runtime()
        .storage
        .put_json(
            "dataset_snapshots_v1",
            fixture.request["datasetId"]
                .as_str()
                .expect("fixture dataset ID"),
            serde_json::to_value(snapshot).expect("snapshot JSON"),
            &context("tamper-snapshot"),
        )
        .expect("tampered storage write");

    let failed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"integrity-worker","idempotencyKey":"integrity-process"}),
    );
    assert_eq!(failed.status, 200);
    assert_eq!(failed.body["status"], "failed");
    assert_eq!(
        failed.body["failureCode"],
        "dataset_snapshot_integrity_failed"
    );

    let retry = fixture.service.handle_http(
        "POST",
        &format!("/backtests/{run_id}/retry"),
        json!({"idempotency_key":"integrity-retry"}),
    );
    assert_eq!(retry.status, 202);
    assert_eq!(retry.body["status"], "queued");
}

#[test]
fn backtest_evidence_and_control_plane_records_do_not_leak_caller_secrets() {
    let fixture = fixture("redaction");
    let secret = "SHOULD_NOT_LEAK_TOKEN";
    let mut request = fixture.request.clone();
    request["idempotencyKey"] = json!("secret-redaction");
    request["actor"] = json!({"kind":"user","id":"local-user","apiSecret":secret});
    request["evidenceRefs"] = json!([
        format!("sightline://context/{secret}"),
        {"kind":"strategy_note","ref":"safe-note","apiSecret":secret}
    ]);
    let created = fixture.service.handle_http("POST", "/backtests", request);
    let run_id = text(&created.body, "runId");
    let processed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({"worker":"redaction-worker","idempotencyKey":"secret-process"}),
    );
    let fetched = fixture
        .service
        .handle_http("GET", &format!("/backtests/{run_id}"), json!({}));
    let replay =
        fixture
            .service
            .handle_http("POST", &format!("/backtests/{run_id}/replay"), json!({}));
    let admin = fixture
        .service
        .handle_http("GET", "/admin/control-plane", json!({}));

    for payload in [
        created.body,
        processed.body,
        fetched.body,
        replay.body,
        admin.body,
    ] {
        let serialized = payload.to_string();
        assert!(!serialized.contains(secret), "secret leaked: {payload:#}");
        assert!(!serialized.contains("Bearer "));
    }
    let exported = fixture.service.handle_http(
        "POST",
        &format!("/backtests/{run_id}/export"),
        json!({"exportKind":"reportJson"}),
    );
    assert_eq!(exported.status, 200, "{exported:#?}");
    let serialized = exported.body.to_string();
    assert!(
        !serialized.contains(secret),
        "secret leaked in export: {serialized}"
    );
    for forbidden in ["should buy", "should sell", "recommend"] {
        assert!(
            !serialized.to_ascii_lowercase().contains(forbidden),
            "export contains advice language: {forbidden}"
        );
    }
}

#[test]
fn derivatives_analysis_uses_completed_backtest_evidence_and_survives_restart() {
    let fixture = fixture("derivatives-durable");
    let created = fixture
        .service
        .handle_http("POST", "/backtests", fixture.request.clone());
    assert_eq!(created.status, 202, "{created:#?}");
    let source_run_id = text(&created.body, "runId");
    let completed = fixture.service.handle_http(
        "POST",
        "/backtests:process",
        json!({
            "worker":"derivatives-source-worker",
            "idempotencyKey":"derivatives-source-process"
        }),
    );
    assert_eq!(completed.status, 200, "{completed:#?}");
    assert_eq!(completed.body["status"], "completed");

    let journal_before_invalid = fixture.service.runtime().journal.events().len();
    let rejected = fixture.service.handle_http(
        "POST",
        "/derivatives-analyses",
        json!({
            "sourceRunId": source_run_id,
            "idempotencyKey": "derivatives-rejected-create",
            "sensitivityGrid": {
                "priceShocksBps": [0, 0],
                "volatilityShiftsPpm": [0],
                "elapsedSeconds": [0]
            },
            "actor":"local-user",
            "surface":"http"
        }),
    );
    assert_eq!(rejected.status, 400, "{rejected:#?}");
    assert_eq!(
        fixture.service.runtime().journal.events().len(),
        journal_before_invalid
    );
    let empty = fixture
        .service
        .handle_http("GET", "/derivatives-analyses", json!({}));
    assert_eq!(empty.body["analyses"].as_array().map(Vec::len), Some(0));

    for (idempotency_key, oversized_input) in [
        (
            "derivatives-too-many-scenarios",
            json!({
                "scenarios": (0..17)
                    .map(|index| json!({
                        "scenarioId": format!("scenario-{index}"),
                        "kind": "hypothetical_stress"
                    }))
                    .collect::<Vec<_>>()
            }),
        ),
        (
            "derivatives-too-wide-grid",
            json!({
                "sensitivityGrid": {
                    "priceShocksBps": (0..17).collect::<Vec<_>>(),
                    "volatilityShiftsPpm": [0],
                    "elapsedSeconds": [0]
                }
            }),
        ),
    ] {
        let journal_before_limit = fixture.service.runtime().journal.events().len();
        let mut request = json!({
            "sourceRunId": source_run_id,
            "idempotencyKey": idempotency_key,
            "actor": "local-user",
            "surface": "http"
        });
        request.as_object_mut().expect("request object").extend(
            oversized_input
                .as_object()
                .expect("oversized request object")
                .clone(),
        );
        let rejected_limit = fixture
            .service
            .handle_http("POST", "/derivatives-analyses", request);
        assert_eq!(rejected_limit.status, 400, "{rejected_limit:#?}");
        assert_eq!(
            fixture.service.runtime().journal.events().len(),
            journal_before_limit
        );
    }

    let request = json!({
        "sourceRunId": source_run_id,
        "idempotencyKey": "derivatives-durable-create",
        "asOfMs": 1767240000000_i64,
        "maxMarkAgeMs": 120000_i64,
        "maxGreekAgeMs": 240000_i64,
        "sensitivityGrid": {
            "priceShocksBps": [-750, 250],
            "volatilityShiftsPpm": [-125000, 50000],
            "elapsedSeconds": [0, 3600]
        },
        "scenarios": [{
            "scenarioId": "declared-flat",
            "kind": "hypothetical_stress",
            "underlyingPricesMicros": {"SPY": 97 * UNIT},
            "volatilityShiftsPpm": {"SPY": 50000},
            "elapsedSeconds": 3600,
            "inputProvenance": "caller-value-must-not-survive"
        }],
        "actor":"local-user",
        "surface":"http"
    });
    let analysis = fixture
        .service
        .handle_http("POST", "/derivatives-analyses", request.clone());
    assert_eq!(analysis.status, 201, "{analysis:#?}");
    let analysis_id = text(&analysis.body, "analysisId");
    assert_eq!(analysis.body["sourceRunId"], source_run_id);
    assert_eq!(analysis.body["request"]["asOfMs"], 1767240000000_i64);
    assert_eq!(analysis.body["request"]["maxMarkAgeMs"], 120000_i64);
    assert_eq!(analysis.body["request"]["maxGreekAgeMs"], 240000_i64);
    assert_eq!(
        analysis.body["request"]["sensitivityGrid"],
        request["sensitivityGrid"]
    );
    assert_eq!(
        analysis.body["request"]["scenarios"][0]["elapsedSeconds"],
        3600
    );
    assert_eq!(
        analysis.body["analysis"]["sourceBindings"]["resultHash"],
        completed.body["resultHash"]
    );
    assert_eq!(
        analysis.body["analysis"]["legs"][0]["lifecycle"]["state"],
        "closed"
    );
    assert_eq!(
        analysis.body["analysis"]["legs"][0]["lifecycleComplete"],
        true
    );
    assert_eq!(
        analysis.body["analysis"]["scenarios"][0]["declaration"]["sourceRef"],
        format!(
            "tradeassembly://backtests/{}/derivatives/scenarios/declared-flat",
            source_run_id
        )
    );
    assert!(!analysis
        .body
        .to_string()
        .contains("caller-value-must-not-survive"));
    assert!(analysis.body["analysis"]["outputHash"]
        .as_str()
        .is_some_and(|value| value.starts_with("sha256:")));

    let replay = fixture
        .service
        .handle_http("POST", "/derivatives-analyses", request.clone());
    assert_eq!(replay.status, 201, "{replay:#?}");
    assert_eq!(replay.body["analysisId"], analysis_id);
    assert_eq!(
        replay.body["analysis"]["outputHash"],
        analysis.body["analysis"]["outputHash"]
    );

    let mut conflicting = request.clone();
    conflicting["asOfMs"] = json!(1767236400000_i64);
    let conflict = fixture
        .service
        .handle_http("POST", "/derivatives-analyses", conflicting);
    assert_eq!(conflict.status, 409, "{conflict:#?}");
    assert_eq!(conflict.body["error"]["code"], "idempotency_conflict");

    let reopened = TradeAssemblyService::test_local(&fixture.db);
    let fetched = reopened.handle_http(
        "GET",
        &format!("/derivatives-analyses/{analysis_id}"),
        json!({}),
    );
    assert_eq!(fetched.status, 200, "{fetched:#?}");
    assert_eq!(
        fetched.body["analysis"]["outputHash"],
        analysis.body["analysis"]["outputHash"]
    );
    let listed = reopened.handle_http("GET", "/derivatives-analyses", json!({}));
    assert_eq!(listed.status, 200, "{listed:#?}");
    assert_eq!(listed.body["analyses"].as_array().map(Vec::len), Some(1));
    let graphql_get = reopened.execute_graphql(json!({
        "operationName":"DerivativesAnalysis",
        "query":"query DerivativesAnalysis($analysisId: String!) { derivativesAnalysis(analysisId: $analysisId) }",
        "variables":{"analysisId":analysis_id}
    }));
    assert!(graphql_get.get("errors").is_none(), "{graphql_get:#?}");
    assert_eq!(
        graphql_get["data"]["derivativesAnalysis"]["analysis"]["outputHash"],
        analysis.body["analysis"]["outputHash"]
    );
    let exported = reopened.handle_http(
        "GET",
        &format!("/derivatives-analyses/{analysis_id}/exports/json"),
        json!({}),
    );
    assert_eq!(exported.status, 200, "{exported:#?}");
    assert_eq!(
        exported.body["contentHash"],
        analysis.body["exports"]["json"]["sha256"]
    );
    assert!(exported.body["content"]
        .as_str()
        .is_some_and(|value| value.contains(&analysis_id) || value.contains("outputHash")));
    let graphql_export = reopened.execute_graphql(json!({
        "operationName":"DerivativesAnalysisExport",
        "query":"query DerivativesAnalysisExport($analysisId: String!, $format: String) { derivativesAnalysisExport(analysisId: $analysisId, format: $format) }",
        "variables":{"analysisId":analysis_id,"format":"json"}
    }));
    assert!(
        graphql_export.get("errors").is_none(),
        "{graphql_export:#?}"
    );
    assert_eq!(
        graphql_export["data"]["derivativesAnalysisExport"]["contentHash"],
        exported.body["contentHash"]
    );

    let mut alternate_request = request.clone();
    alternate_request["idempotencyKey"] = json!("derivatives-alternate-config");
    alternate_request["maxMarkAgeMs"] = json!(120001_i64);
    let alternate = reopened.handle_http("POST", "/derivatives-analyses", alternate_request);
    assert_eq!(alternate.status, 201, "{alternate:#?}");
    assert_ne!(alternate.body["analysisId"], analysis_id);
    assert_ne!(alternate.body["requestHash"], analysis.body["requestHash"]);

    let mut tampered = fetched.body;
    tampered["request"]["maxMarkAgeMs"] = json!(120001_i64);
    let tamper_context = SideEffectContext::new(
        AuthorityContext {
            actor: "integrity-test".to_string(),
            surface: "test".to_string(),
            account_mode: "research".to_string(),
        },
        IdempotencyKey::new("derivatives-tamper-fixture").expect("idempotency key"),
    );
    reopened
        .runtime()
        .storage
        .put_json(
            "derivatives_analyses_v1",
            &analysis_id,
            tampered,
            &tamper_context,
        )
        .expect("tamper fixture");
    let rejected_tamper = reopened.handle_http(
        "GET",
        &format!("/derivatives-analyses/{analysis_id}"),
        json!({}),
    );
    assert_eq!(rejected_tamper.status, 409, "{rejected_tamper:#?}");
    assert_eq!(
        rejected_tamper.body["error"]["code"],
        "derivatives_evidence_integrity_failed"
    );
}

fn fixture(name: &str) -> Fixture {
    fixture_with_expressions(name, "bar.close > 101000000", "bar.close < 100000000")
}

fn fixture_with_expressions(name: &str, entry_expression: &str, exit_expression: &str) -> Fixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let db = directory
        .path()
        .join(format!("{name}.db"))
        .display()
        .to_string();
    let service = TradeAssemblyService::test_local(&db);
    let spec = strategy_spec(entry_expression, exit_expression);
    let spec_hash = canonical_hash(&spec, "strategy hash").expect("strategy hash");
    service
        .runtime()
        .storage
        .put_json(
            "strategy_versions",
            "strategy-1:1",
            json!({
                "id":"version-1",
                "strategyId":"strategy-1",
                "version":1,
                "specHash":spec_hash,
                "spec":spec,
                "published":true,
                "immutable":true
            }),
            &context(&format!("{name}-strategy")),
        )
        .expect("strategy version");
    let snapshot = snapshot();
    service
        .runtime()
        .dataset_snapshots
        .put(&snapshot, &context(&format!("{name}-dataset")))
        .expect("dataset snapshot");
    let request = json!({
        "strategyId":"strategy-1",
        "strategyVersionId":"version-1",
        "datasetId":snapshot.dataset_id,
        "startingCashMicros":1_000 * UNIT,
        "fillTiming":"next_bar",
        "fillPriceSource":"open",
        "endOfData":"mark_to_market",
        "fixedFeeMicros":0,
        "perUnitFeeMicros":0,
        "notionalFeeBps":0,
        "spreadBps":0,
        "slippageBps":0,
        "maximumParticipationBps":10_000,
        "minimumVolumeMicros":UNIT,
        "maximumPositionNotionalMicros":1_000 * UNIT,
        "maximumOrderQuantityMicros":10 * UNIT,
        "client":"self",
        "purpose":"strategy_backtest_research",
        "idempotencyKey":format!("{name}-create")
    });
    Fixture {
        _directory: directory,
        db,
        service,
        request,
    }
}

fn strategy_spec(entry_expression: &str, exit_expression: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3/valid/static-equity.json");
    let mut spec: Value =
        serde_json::from_slice(&fs::read(path).expect("strategy fixture")).expect("strategy JSON");
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.expression":entry_expression,"tradeassembly.quantity":UNIT});
    spec["stages"]["exit_policy"]["substeps"][0]["config"]["tradeassembly.expression"] =
        json!(exit_expression);
    spec
}

fn snapshot() -> DatasetSnapshot {
    DatasetSnapshot::build(
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
    .expect("snapshot")
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
        IdempotencyKey::new(key).expect("idempotency key"),
    )
}

fn text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing {key}: {value:#}"))
        .to_string()
}
