use async_graphql::{Object, Request, Schema};
use serde_json::{json, Value};

#[derive(Default)]
pub struct Query;

#[derive(Default)]
pub struct Mutation;

#[Object]
impl Query {
    async fn health(&self) -> Value {
        json!({"status": "ok"})
    }

    async fn local_workspace(&self) -> Value {
        schema_payload("localWorkspace")
    }

    async fn strategy_library(&self) -> Value {
        schema_payload("strategyLibrary")
    }

    async fn strategy_home(&self) -> Value {
        schema_payload("strategyHome")
    }

    async fn strategy_detail(&self) -> Value {
        schema_payload("strategyDetail")
    }

    async fn strategy_instrument_context(&self) -> Value {
        schema_payload("strategyInstrumentContext")
    }

    async fn strategy_version_history(&self) -> Value {
        schema_payload("strategyVersionHistory")
    }

    async fn strategy_builder(&self) -> Value {
        schema_payload("strategyBuilder")
    }

    async fn strategy_contract_metadata(&self) -> Value {
        schema_payload("strategyContractMetadata")
    }

    async fn strategy_research_workspace(&self) -> Value {
        schema_payload("strategyResearchWorkspace")
    }

    async fn strategy_execution_workspace(&self) -> Value {
        schema_payload("strategyExecutionWorkspace")
    }

    async fn run_center(&self) -> Value {
        schema_payload("runCenter")
    }

    async fn robustness_runs(&self) -> Value {
        schema_payload("robustnessRuns")
    }

    async fn robustness_run(&self, run_id: String) -> Value {
        schema_payload_with_request("robustnessRun", json!({"runId": run_id}))
    }

    async fn robustness_report(&self, run_id: String) -> Value {
        schema_payload_with_request("robustnessReport", json!({"runId": run_id}))
    }

    async fn research_rollup(&self) -> Value {
        schema_payload("researchRollup")
    }

    async fn alert_center(&self) -> Value {
        schema_payload("alertCenter")
    }

    async fn discover_workspace(&self) -> Value {
        schema_payload("discoverWorkspace")
    }

    async fn strategy_share_workspace(&self) -> Value {
        schema_payload("strategyShareWorkspace")
    }

    async fn plugins_providers(&self) -> Value {
        schema_payload("pluginsProviders")
    }

    async fn admin_plugin_manifest_registry(&self) -> Value {
        schema_payload("adminPluginManifestRegistry")
    }

    async fn connect_workspace(&self) -> Value {
        schema_payload("connectWorkspace")
    }
}

#[Object]
impl Mutation {
    async fn noop(&self) -> bool {
        true
    }

    async fn store_credentials(&self) -> Value {
        schema_payload("storeCredentials")
    }

    async fn test_credentials(&self) -> Value {
        schema_payload("testCredentials")
    }

    async fn install_plugin_manifest(&self) -> Value {
        schema_payload("installPluginManifest")
    }

    async fn install_plugin_package(&self) -> Value {
        schema_payload("installPluginPackage")
    }

    async fn create_plugin_instance(&self) -> Value {
        schema_payload("createPluginInstance")
    }

    async fn configure_plugin_instance(&self) -> Value {
        schema_payload("configurePluginInstance")
    }

    async fn enable_plugin_instance(&self) -> Value {
        schema_payload("enablePluginInstance")
    }

    async fn disable_plugin_instance(&self) -> Value {
        schema_payload("disablePluginInstance")
    }

    async fn upgrade_plugin_instance(&self) -> Value {
        schema_payload("upgradePluginInstance")
    }

    async fn rollback_plugin_instance(&self) -> Value {
        schema_payload("rollbackPluginInstance")
    }

    async fn remove_plugin_instance(&self) -> Value {
        schema_payload("removePluginInstance")
    }

    async fn store_plugin_credentials(&self) -> Value {
        schema_payload("storePluginCredentials")
    }

    async fn revoke_plugin_credentials(&self) -> Value {
        schema_payload("revokePluginCredentials")
    }

    async fn refresh_plugin_health(&self) -> Value {
        schema_payload("refreshPluginHealth")
    }

    async fn create_strategy(&self) -> Value {
        schema_payload("createStrategy")
    }

    async fn create_strategy_ai_draft(&self) -> Value {
        schema_payload("createStrategyAiDraft")
    }

    async fn save_builder_draft(&self) -> Value {
        schema_payload("saveBuilderDraft")
    }

    async fn select_strategy_semantic_node(&self) -> Value {
        schema_payload("selectStrategySemanticNode")
    }

    async fn propose_strategy_patch(&self) -> Value {
        schema_payload("proposeStrategyPatch")
    }

    async fn review_strategy_patch(&self) -> Value {
        schema_payload("reviewStrategyPatch")
    }

    async fn apply_strategy_patch(&self) -> Value {
        schema_payload("applyStrategyPatch")
    }

    async fn validate_builder_draft(&self) -> Value {
        schema_payload("validateBuilderDraft")
    }

    async fn validate_strategy_expression(&self) -> Value {
        schema_payload("validateStrategyExpression")
    }

    async fn publish_strategy(&self) -> Value {
        schema_payload("publishStrategy")
    }

    async fn duplicate_strategy(&self) -> Value {
        schema_payload("duplicateStrategy")
    }

    async fn archive_strategy(&self) -> Value {
        schema_payload("archiveStrategy")
    }

    async fn restore_strategy(&self) -> Value {
        schema_payload("restoreStrategy")
    }

    async fn compile_strategy_preview(&self) -> Value {
        schema_payload("compileStrategyPreview")
    }

    async fn run_strategy_research(&self) -> Value {
        schema_payload("runStrategyResearch")
    }

    async fn run_scenario_valuation(&self) -> Value {
        schema_payload("runScenarioValuation")
    }

    async fn run_portfolio_risk(&self) -> Value {
        schema_payload("runPortfolioRisk")
    }

    async fn portfolio_risk_report(&self) -> Value {
        schema_payload("portfolioRiskReport")
    }

    async fn fill_quality_inspect(&self) -> Value {
        schema_payload("fillQualityInspect")
    }

    async fn fill_quality_report(&self) -> Value {
        schema_payload("fillQualityReport")
    }

    async fn fill_quality_replay(&self) -> Value {
        schema_payload("fillQualityReplay")
    }

    async fn fill_quality_export(&self) -> Value {
        schema_payload("fillQualityExport")
    }

    async fn attribution_journal_review(&self) -> Value {
        schema_payload("attributionJournalReview")
    }

    async fn attribution_journal_report(&self) -> Value {
        schema_payload("attributionJournalReport")
    }

    async fn attribution_journal_inspect(&self) -> Value {
        schema_payload("attributionJournalInspect")
    }

    async fn attribution_journal_replay(&self) -> Value {
        schema_payload("attributionJournalReplay")
    }

    async fn attribution_journal_export(&self) -> Value {
        schema_payload("attributionJournalExport")
    }

    async fn create_research_dataset(&self) -> Value {
        schema_payload("createResearchDataset")
    }

    async fn queue_strategy_research_job(&self) -> Value {
        schema_payload("queueStrategyResearchJob")
    }

    async fn strategy_research_job_status(&self) -> Value {
        schema_payload("strategyResearchJobStatus")
    }

    async fn promote_strategy_research_to_paper(&self) -> Value {
        schema_payload("promoteStrategyResearchToPaper")
    }

    async fn save_execution_config(&self) -> Value {
        schema_payload("saveExecutionConfig")
    }

    async fn activation_readiness(&self) -> Value {
        schema_payload("activationReadiness")
    }

    async fn activate_execution(&self) -> Value {
        schema_payload("activateExecution")
    }

    async fn deactivate_strategy_execution(&self) -> Value {
        schema_payload("deactivateStrategyExecution")
    }

    async fn update_provider_instance(&self) -> Value {
        schema_payload("updateProviderInstance")
    }

    async fn enable_provider_instance(&self) -> Value {
        schema_payload("enableProviderInstance")
    }

    async fn disable_provider_instance(&self) -> Value {
        schema_payload("disableProviderInstance")
    }

    async fn acknowledge_alert(&self) -> Value {
        schema_payload("acknowledgeAlert")
    }

    async fn create_strategy_share_snapshot(&self) -> Value {
        schema_payload("createStrategyShareSnapshot")
    }

    async fn revoke_strategy_share_snapshot(&self) -> Value {
        schema_payload("revokeStrategyShareSnapshot")
    }

    async fn run_backtest(&self) -> Value {
        schema_payload("runBacktest")
    }

    async fn create_robustness_run(&self, request: Value) -> Value {
        schema_payload_with_request("createRobustnessRun", request)
    }

    async fn process_robustness_run(&self, run_id: String, worker: Option<String>) -> Value {
        schema_payload_with_request(
            "processRobustnessRun",
            json!({"runId": run_id, "worker": worker}),
        )
    }

    async fn cancel_robustness_run(
        &self,
        run_id: String,
        idempotency_key: Option<String>,
    ) -> Value {
        schema_payload_with_request(
            "cancelRobustnessRun",
            json!({"runId": run_id, "idempotencyKey": idempotency_key}),
        )
    }

    async fn retry_robustness_run(&self, run_id: String, idempotency_key: Option<String>) -> Value {
        schema_payload_with_request(
            "retryRobustnessRun",
            json!({"runId": run_id, "idempotencyKey": idempotency_key}),
        )
    }

    async fn replay_robustness_run(
        &self,
        run_id: String,
        idempotency_key: Option<String>,
    ) -> Value {
        schema_payload_with_request(
            "replayRobustnessRun",
            json!({"runId": run_id, "idempotencyKey": idempotency_key}),
        )
    }

    async fn export_robustness_run(
        &self,
        run_id: String,
        format: Option<String>,
        idempotency_key: Option<String>,
    ) -> Value {
        schema_payload_with_request(
            "exportRobustnessRun",
            json!({
                "runId": run_id,
                "format": format.unwrap_or_else(|| "json".to_string()),
                "idempotencyKey": idempotency_key,
            }),
        )
    }

    async fn backtest_report_export(&self) -> Value {
        schema_payload("backtestReportExport")
    }

    async fn run_research_sweep(&self) -> Value {
        schema_payload("runResearchSweep")
    }

    async fn create_research_universe(&self) -> Value {
        schema_payload("createResearchUniverse")
    }

    async fn run_strategy_once(&self) -> Value {
        schema_payload("runStrategyOnce")
    }

    async fn scheduler_start(&self) -> Value {
        schema_payload("schedulerStart")
    }

    async fn scheduler_run(&self) -> Value {
        schema_payload("schedulerRun")
    }

    async fn scheduler_stop(&self) -> Value {
        schema_payload("schedulerStop")
    }

    async fn reconcile_orders(&self) -> Value {
        schema_payload("reconcileOrders")
    }
}

fn schema_payload(field: &str) -> Value {
    json!({"field": field, "runtime": "rust"})
}

fn schema_payload_with_request(field: &str, request: Value) -> Value {
    json!({"field": field, "runtime": "rust", "request": request})
}

pub async fn execute(request: Value) -> Value {
    let query = request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut graphql_request = Request::new(query);
    if let Some(operation_name) = request.get("operationName").and_then(Value::as_str) {
        if !operation_name.is_empty() {
            graphql_request = graphql_request.operation_name(operation_name);
        }
    }
    if let Some(variables) = request.get("variables") {
        graphql_request =
            graphql_request.variables(async_graphql::Variables::from_json(variables.clone()));
    }

    let schema = Schema::build(Query, Mutation, async_graphql::EmptySubscription).finish();
    serde_json::to_value(schema.execute(graphql_request).await)
        .unwrap_or_else(|error| json!({"errors": [{"message": error.to_string()}]}))
}

pub fn is_standard_introspection(request: &Value) -> bool {
    if request.get("operationName").and_then(Value::as_str) == Some("__schema") {
        return false;
    }
    let query = request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    query.contains("__schema") || query.contains("__type")
}
