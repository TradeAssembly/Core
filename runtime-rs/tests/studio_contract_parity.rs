// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn studio_runtime_payloads_preserve_legacy_contract_shapes() {
    let service = test_service("runtime-payloads");

    let connect = service.execute_graphql(json!({
        "operationName": "ConnectWorkspace",
        "query": "query ConnectWorkspace { connectWorkspace }",
        "variables": studio_variables(),
    }));
    let credential_status = &connect["data"]["connectWorkspace"]["credentialStatus"];
    assert!(
        credential_status["sim"].is_object(),
        "Studio connect workspace expects provider-keyed credential status: {connect:#}"
    );
    assert!(
        credential_status["sim"]["configured"].is_boolean(),
        "Studio connect workspace status must expose configured boolean: {connect:#}"
    );
    assert!(
        credential_status
            .as_object()
            .expect("credential status object")
            .values()
            .all(Value::is_object),
        "Studio credentialStatus table cannot render scalar or null entries: {connect:#}"
    );
    assert_eq!(
        connect["data"]["connectWorkspace"]["workspacePrimitive"], "plugins",
        "Studio connect workspace must present Plugins as the user-facing primitive: {connect:#}"
    );
    let plugin_workspace = service.execute_graphql(json!({
        "operationName": "PluginsProviders",
        "query": "query PluginsProviders { pluginsProviders }",
        "variables": studio_variables(),
    }));
    let payload = &plugin_workspace["data"]["pluginsProviders"];
    assert_eq!(
        payload["workspacePrimitive"], "plugins",
        "{plugin_workspace:#}"
    );
    assert!(
        payload["plugins"]
            .as_array()
            .expect("plugins")
            .iter()
            .any(|plugin| {
                plugin["ref"] == "tradeassembly.simbroker"
                    && plugin["pluginRef"] == "tradeassembly.simbroker"
                    && plugin["providerRef"] == "sim"
                    && plugin["capabilityResolution"]["profile"]["id"] == "local_full"
                    && plugin["capabilityResolution"]["noSilentFallback"] == true
            }),
        "plugin workspace must expose resolver state for installed plugins: {plugin_workspace:#}"
    );
    assert_no_secret_leak(&plugin_workspace, "PluginsProviders");

    let dataset = service.execute_graphql(json!({
        "operationName": "CreateResearchDataset",
        "query": "mutation CreateResearchDataset { createResearchDataset }",
        "variables": studio_variables(),
    }));
    let dataset_id = dataset["data"]["createResearchDataset"]["body"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing dataset id: {dataset:#}"));
    assert!(
        dataset_id.starts_with("ds_"),
        "Studio e2e status contract expects ds_ dataset ids, got {dataset:#}"
    );

    let scenario = service.execute_graphql(json!({
        "operationName": "RunScenarioValuation",
        "query": "mutation RunScenarioValuation { runScenarioValuation }",
        "variables": {
            "strategyId": "strat_local_btc_demo",
            "checkpointId": "checkpoint_001"
        },
    }));
    assert_eq!(
        scenario["errors"][0]["message"], "derivatives_request_invalid",
        "{scenario:#}"
    );
    assert!(scenario.get("data").is_none());
    assert_no_secret_leak(&scenario, "RunScenarioValuation");

    let monte_carlo = service.execute_graphql(json!({
        "operationName": "RunMonteCarlo",
        "query": "mutation RunMonteCarlo { runMonteCarlo }",
        "variables": {
            "strategyId": "strat_local_btc_demo"
        },
    }));
    assert_eq!(
        monte_carlo["errors"][0]["message"], "robustness_request_invalid",
        "{monte_carlo:#}"
    );
    assert!(monte_carlo.get("data").is_none());
    assert_no_secret_leak(&monte_carlo, "RunMonteCarlo");

    let portfolio_risk = service.execute_graphql(json!({
        "operationName": "RunPortfolioRisk",
        "query": "mutation RunPortfolioRisk { runPortfolioRisk }",
        "variables": {
            "strategyId": "strat_local_btc_demo",
            "scopeKind": "strategy"
        },
    }));
    let payload = &portfolio_risk["data"]["runPortfolioRisk"];
    assert_eq!(payload["ok"], true, "{portfolio_risk:#}");
    assert_eq!(payload["kind"], "portfolio_risk");
    assert_eq!(
        payload["reportEnvelope"]["charts"][0]["id"],
        "portfolio_risk_exposure_table"
    );
    assert!(
        payload["reportEnvelope"]["renderTargets"]["tradeassembly"]["href"]
            .as_str()
            .unwrap()
            .contains("view=portfolio-risk")
    );
    assert_no_secret_leak(&portfolio_risk, "RunPortfolioRisk");

    let fill_quality = service.execute_graphql(json!({
        "operationName": "FillQualityReport",
        "query": "mutation FillQualityReport { fillQualityReport }",
        "variables": {
            "strategyId": "strat_local_btc_demo",
            "analysisId": "fill_quality_latest"
        },
    }));
    let payload = &fill_quality["data"]["fillQualityReport"];
    assert_eq!(payload["ok"], true, "{fill_quality:#}");
    assert_eq!(payload["kind"], "fill_quality");
    assert_eq!(payload["summary"]["fillCount"].as_i64(), Some(2));
    assert!(payload["reportEnvelope"]["deepLinks"]["execution"]["href"]
        .as_str()
        .unwrap()
        .contains("fillQuality=fill_quality_latest"));
    assert_no_secret_leak(&fill_quality, "FillQualityReport");

    let lifecycle_calendar = service.execute_graphql(json!({
        "operationName": "LifecycleCalendarInspect",
        "query": "mutation LifecycleCalendarInspect { lifecycleCalendarInspect }",
        "variables": {
            "strategyId": "strat_local_btc_demo",
            "timelineId": "lifecycle_calendar_latest"
        },
    }));
    let calendar_payload = &lifecycle_calendar["data"]["lifecycleCalendarInspect"];
    assert_eq!(
        calendar_payload["schemaVersion"], "tradeassembly.lifecycle_calendar.service.v1",
        "{lifecycle_calendar:#}"
    );
    assert_eq!(calendar_payload["strategyId"], "strat_local_btc_demo");
    assert!(
        calendar_payload["summary"]["upcomingCount"]
            .as_i64()
            .unwrap_or(0)
            >= 1
    );
    assert!(calendar_payload["deepLinks"]["calendar"]["href"]
        .as_str()
        .unwrap()
        .contains("lifecycleCalendar="));
    assert_no_secret_leak(&lifecycle_calendar, "LifecycleCalendarInspect");

    let workspace = service.execute_graphql(json!({
        "operationName": "StrategyResearchWorkspace",
        "query": "query StrategyResearchWorkspace { strategyResearchWorkspace }",
        "variables": {
            "strategyId": "strat_local_btc_demo"
        },
    }));
    let report = &workspace["data"]["strategyResearchWorkspace"]["portfolioRiskReport"];
    assert_eq!(report["kind"], "portfolio_risk", "{workspace:#}");
    assert!(
        report["reportEnvelope"]["deepLinks"]["portfolioRisk"]["href"]
            .as_str()
            .expect("portfolio risk deep link")
            .contains("view=portfolio-risk")
    );
    assert_no_secret_leak(&workspace, "StrategyResearchWorkspace");

    let execution_workspace = service.execute_graphql(json!({
        "operationName": "StrategyExecutionWorkspace",
        "query": "query StrategyExecutionWorkspace { strategyExecutionWorkspace }",
        "variables": {
            "strategyId": "strat_local_btc_demo"
        },
    }));
    let report = &execution_workspace["data"]["strategyExecutionWorkspace"]["fillQualityReport"];
    assert_eq!(report["kind"], "fill_quality", "{execution_workspace:#}");
    assert!(report["perFill"].as_array().expect("per-fill rows").len() >= 2);
    let calendar = &execution_workspace["data"]["strategyExecutionWorkspace"]["lifecycleCalendar"];
    assert_eq!(
        calendar["schemaVersion"], "tradeassembly.lifecycle_calendar.service.v1",
        "{execution_workspace:#}"
    );
    assert!(calendar["deepLinks"]["calendar"]["href"]
        .as_str()
        .unwrap()
        .contains("lifecycleCalendar="));
    assert_no_secret_leak(&execution_workspace, "StrategyExecutionWorkspace");

    let attribution_journal = service.execute_graphql(json!({
        "operationName": "AttributionJournalReport",
        "query": "mutation AttributionJournalReport { attributionJournalReport }",
        "variables": {
            "strategyId": "strat_local_btc_demo",
            "reviewId": "attribution_journal_latest",
            "metadata": {"api_secret": "SHOULD_NOT_LEAK"}
        },
    }));
    let journal_payload = &attribution_journal["data"]["attributionJournalReport"];
    assert_eq!(
        journal_payload["schemaVersion"], "tradeassembly.attribution_journal.service.v1",
        "{attribution_journal:#}"
    );
    assert_eq!(journal_payload["kind"], "attribution_journal");
    assert!(
        journal_payload["summary"]["closedCount"]
            .as_i64()
            .unwrap_or(0)
            >= 1
    );
    assert!(journal_payload["deepLinks"]["journal"]["href"]
        .as_str()
        .unwrap()
        .contains("attributionJournal=attribution_journal_latest"));
    assert_eq!(journal_payload["sideEffects"]["brokerStateChanged"], false);
    assert_no_secret_leak(&attribution_journal, "AttributionJournalReport");

    let execution_workspace = service.execute_graphql(json!({
        "operationName": "StrategyExecutionWorkspace",
        "query": "query StrategyExecutionWorkspace { strategyExecutionWorkspace }",
        "variables": {
            "strategyId": "strat_local_btc_demo"
        },
    }));
    let journal_report =
        &execution_workspace["data"]["strategyExecutionWorkspace"]["attributionJournal"];
    assert_eq!(
        journal_report["kind"], "attribution_journal",
        "{execution_workspace:#}"
    );
    assert!(journal_report["deepLinks"]["journal"]["href"]
        .as_str()
        .unwrap()
        .contains("attributionJournal="));
    assert_no_secret_leak(
        &execution_workspace,
        "StrategyExecutionWorkspaceAttributionJournal",
    );
}

fn studio_variables() -> Value {
    json!({
        "strategyId": "strat_local_btc_demo",
        "providerRef": "sim",
        "pluginRef": "tradeassembly.simbroker",
        "accountRef": "account://sim/paper",
        "ref": "sim",
        "snapshotId": "latest",
        "route": "/app/strategies/strat_local_btc_demo/builder",
        "alertId": "local_test_alert",
        "activationId": "activation-btc-exit-demo",
        "configId": "cfg_local_btc_exit",
        "engine": "deterministic-fixture",
        "brief": "Draft local BTC paper strategy",
        "market": "crypto",
        "horizon": "intraday",
        "mode": "blank",
        "name": "Studio contract parity strategy",
        "versionId": "ver_local",
        "expression": "true",
        "apiKey": "KEY_SHOULD_NOT_LEAK",
        "apiSecret": "SECRET_SHOULD_NOT_LEAK",
        "paper": true,
        "parameters": {"risk.max_notional": [10, 25]},
        "symbols": ["BTC/USD", "ETH/USD"],
        "datasetId": "demo",
        "jobId": "latest",
        "stub": true,
        "spec": valid_v3_spec()
    })
}

fn valid_v3_spec() -> Value {
    let mut spec: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/strategy-spec/v3/valid/crypto-spot-24x7.json"
    )))
    .expect("valid checked-in V3 strategy fixture");
    spec["strategy_id"] = json!("strat_local_btc_demo");
    spec["name"] = json!("BTC fast exit demo");
    spec
}

fn assert_no_secret_leak(payload: &Value, operation_name: &str) {
    let text = payload.to_string();
    for secret in ["KEY_SHOULD_NOT_LEAK", "SECRET_SHOULD_NOT_LEAK"] {
        assert!(
            !text.contains(secret),
            "{operation_name} leaked a raw credential into Studio GraphQL payload"
        );
    }
}

fn test_service(name: &str) -> TradeAssemblyService {
    TradeAssemblyService::test_local(temp_db(name).to_string_lossy().to_string())
}

fn temp_db(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "tradeassembly-studio-contract-{name}-{}-{stamp}.db",
        std::process::id()
    ))
}
