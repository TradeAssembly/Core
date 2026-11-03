use clap::Parser;
use serde_json::json;
use std::process::Command as ProcessCommand;
use tradeassembly_runtime::cli::{execute_command, Cli};
use tradeassembly_runtime::service::TradeAssemblyService;

fn execution_cli(service: &TradeAssemblyService, args: &[&str]) -> serde_json::Value {
    let mut argv = vec!["tradeassembly", "execution"];
    argv.extend_from_slice(args);
    let cli = Cli::try_parse_from(argv).expect("execution CLI parses");
    execute_command(service, cli.command)
}

#[test]
fn execution_config_uses_persisted_strategy_version_identity() {
    let db = format!(
        ".tradeassembly/test-execution-config-spec-hash-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let seed = Cli::try_parse_from(["tradeassembly", "seed-btc-demo"]).expect("seed CLI parses");
    let _ = execute_command(&service, seed.command);
    let versions = service
        .runtime()
        .storage
        .list_json("strategy_versions")
        .expect("persisted strategy versions");
    let version = versions
        .iter()
        .map(|(_, value)| value)
        .find(|value| value["strategyId"] == "strat_local_btc_demo")
        .expect("seeded BTC strategy version");

    let response = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "versionId": version["id"],
            "mode": "paper",
            "providerRef": "sim"
        }),
    );

    assert_eq!(response.status, 200, "{:#?}", response.body);
    assert_eq!(
        response.body["body"]["item"]["strategySpecHash"],
        version["specHash"]
    );
}

#[test]
fn execution_orders_risk_positions_scheduler_and_workers_are_persisted() {
    let db = format!(
        ".tradeassembly/test-execution-parity-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);

    let rejected = service.handle_http(
        "POST",
        "/product/run-center/run-once",
        json!({"activationId": "activation-task10", "submitOrders": true}),
    );
    assert_eq!(rejected.status, 403);
    assert_eq!(rejected.body["error"]["code"], "authority_required");

    let rejected_injected_authority = service.handle_http(
        "POST",
        "/product/run-center/run-once",
        json!({
            "activationId": "activation-task10",
            "submitOrders": true,
            "idempotencyKey": "run-task10-no-explicit-authority",
            "accountMode": "paper"
        }),
    );
    assert_eq!(rejected_injected_authority.status, 403);
    assert_eq!(
        rejected_injected_authority.body["error"]["code"],
        "authority_required"
    );

    let dry_run = service.handle_http(
        "POST",
        "/product/run-center/run-once",
        json!({
            "activationId": "activation-task10",
            "submitOrders": false,
            "idempotencyKey": "run-task10-dry",
            "authorityContext": {"actor": "local-user", "surface": "test", "accountMode": "paper"},
            "accountMode": "paper"
        }),
    );
    assert_eq!(dry_run.status, 200);
    assert_eq!(dry_run.body["body"]["submit_orders"], false);
    assert_eq!(dry_run.body["body"]["order"]["status"], "planned");
    assert_eq!(dry_run.body["body"]["risk"]["reserved"], false);
    let dry_order_id = dry_run.body["body"]["order_id"]
        .as_str()
        .expect("dry order id");
    let dry_reconcile = service.handle_http("POST", "/orders/reconcile", json!({}));
    assert!(dry_reconcile.body.as_array().unwrap().iter().any(|order| {
        order["order_id"] == dry_order_id
            && order["status"] == "planned"
            && order["reconciled"] == false
            && order["reconciliationSource"] == "unavailable"
            && order["reconciliationReason"] == "broker_observation_required"
    }));

    let submitted = service.handle_http(
        "POST",
        "/product/run-center/run-once",
        json!({
            "activationId": "activation-task10",
            "submitOrders": true,
            "idempotencyKey": "run-task10-submit",
            "authorityContext": {"actor": "local-user", "surface": "test", "accountMode": "paper"},
            "accountMode": "paper",
            "symbol": "BTC/USD",
            "qty": 0.0002
        }),
    );
    assert_eq!(submitted.status, 200);
    let order_id = submitted.body["body"]["order_id"]
        .as_str()
        .expect("order id");
    assert!(order_id.starts_with("order_"));
    assert_eq!(submitted.body["body"]["submit_orders"], true);
    assert_eq!(submitted.body["body"]["order"]["status"], "submitted");
    assert_eq!(submitted.body["body"]["risk"]["reserved"], true);

    let reopened = TradeAssemblyService::test_local(&db);
    let orders = reopened.handle_http("GET", "/orders", json!({}));
    assert!(orders
        .body
        .as_array()
        .unwrap()
        .iter()
        .any(|order| order["id"] == order_id));
    let reservations = reopened.handle_http("GET", "/risk/reservations", json!({}));
    assert!(reservations
        .body
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["order_id"] == order_id));
    let positions = reopened.handle_http("GET", "/portfolio/positions", json!({}));
    assert!(!positions
        .body
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["entryOrderRefs"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|item| item == order_id))));

    let status_event = reopened.handle_http(
        "POST",
        &format!("/orders/{order_id}/status-events"),
        json!({"eventId": "evt-fill-task10", "status": "filled", "eventSequence": 2}),
    );
    assert_eq!(status_event.body["accepted"], true);

    let reconcile = reopened.handle_http("POST", "/orders/reconcile", json!({}));
    assert!(reconcile.body.as_array().unwrap().iter().any(|order| {
        order["order_id"] == order_id
            && order["status"] == "filled"
            && order["reconciled"] == false
            && order["reconciliationRequired"] == true
            && order["reconciliationReason"] == "broker_observation_required"
    }));
    let repeated_reconcile = reopened.handle_http("POST", "/orders/reconcile", json!({}));
    assert_eq!(repeated_reconcile.body, reconcile.body);

    let scheduler_config = reopened.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({"strategyId": "strat_local_btc_demo", "mode": "paper", "providerRef": "sim"}),
    );
    let scheduler_activation = reopened.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": scheduler_config.body["body"]["configId"],
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "task10-scheduler-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    let scheduler_activation_id = scheduler_activation.body["body"]["activationId"]
        .as_str()
        .expect("scheduler activation");
    let scheduler_start = reopened.handle_http(
        "POST",
        "/product/scheduler/start",
        json!({"activationId": scheduler_activation_id, "intervalSeconds": 30}),
    );
    assert_eq!(scheduler_start.body["body"]["running"], true);
    let worker_run = reopened.handle_http(
        "POST",
        "/scheduler/run",
        json!({"workerId": "worker-task10", "maxCycles": 2, "leaseTtlSeconds": 30}),
    );
    assert_eq!(worker_run.body["body"]["lease"]["acquired"], true);
    assert_eq!(worker_run.body["body"]["iterations"], 2);
    let scheduler_status =
        TradeAssemblyService::test_local(&db).handle_http("GET", "/scheduler/status", json!({}));
    assert_eq!(scheduler_status.body["running"], true);
    assert_eq!(scheduler_status.body["lastWorkerId"], "worker-task10");
    assert_eq!(scheduler_status.body["iterations"], 2);

    let second_worker_run = reopened.handle_http(
        "POST",
        "/scheduler/run",
        json!({
            "workerId": "worker-task10",
            "maxCycles": 1,
            "leaseTtlSeconds": 30,
            "idempotencyKey": "worker-task10-second-run"
        }),
    );
    assert_eq!(second_worker_run.body["body"]["iterations"], 1);
    let cumulative_status =
        TradeAssemblyService::test_local(&db).handle_http("GET", "/scheduler/status", json!({}));
    assert_eq!(cumulative_status.body["iterations"], 3);
}

#[test]
fn execution_config_never_invents_an_external_plugin_account_binding() {
    let db = format!(
        ".tradeassembly/test-alpaca-paper-account-binding-{}.db",
        std::process::id()
    );
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(db);
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "providerRef": "external-plugin-instance",
            "mode": "paper",
            "paperBrokerExecution": true,
            "idempotencyKey": "external-plugin-config-binding"
        }),
    );
    assert_eq!(saved.status, 200);
    assert!(saved.body["body"]["item"]["accountRef"].is_null());
    assert!(saved.body["body"]["item"]["paperBrokerExecution"].is_null());
}

#[test]
fn execution_activation_tick_controls_and_apf_evidence_are_durable() {
    let db = format!(
        ".tradeassembly/test-execution-runtime-{}.db",
        std::process::id()
    );
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);

    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {"max_notional": 25, "max_order_quantity": 0.0003}
        }),
    );
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["body"]["readiness"]["ready"], true);
    let config_id = saved.body["body"]["configId"].as_str().expect("config id");

    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "activation-runtime-test",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activated.status, 200);
    assert_eq!(activated.body["body"]["status"], "active");
    assert_eq!(activated.body["body"]["run"]["kind"], "ExecutionRun");
    assert_eq!(activated.body["body"]["run"]["apf"]["client"], "self");
    assert_eq!(
        activated.body["body"]["run"]["apf"]["purpose"],
        "paper_trading"
    );
    let activation_id = activated.body["body"]["activationId"]
        .as_str()
        .expect("activation id");

    let started = service.handle_http(
        "POST",
        "/product/scheduler/start",
        json!({"activationId": activation_id, "intervalSeconds": 30}),
    );
    assert_eq!(started.body["body"]["running"], true);

    let ticked = service.handle_http(
        "POST",
        "/scheduler/run",
        json!({"activationId": activation_id, "workerId": "worker-runtime", "maxCycles": 2}),
    );
    assert_eq!(ticked.status, 200);
    assert_eq!(ticked.body["body"]["iterations"], 2);
    assert_eq!(ticked.body["body"]["ticks"].as_array().unwrap().len(), 2);
    assert_eq!(
        ticked.body["body"]["ticks"][0]["decision"]["kind"],
        "order_intent"
    );
    assert_eq!(
        ticked.body["body"]["ticks"][1]["decision"]["kind"],
        "no_signal"
    );

    let paused = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "pause_entries"}),
    );
    assert_eq!(paused.status, 200);
    assert_eq!(paused.body["body"]["controls"]["pauseEntries"], "paused");

    let workspace = TradeAssemblyService::test_local(&db).handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(workspace.status, 200);
    assert_eq!(workspace.body["activeRun"]["activationId"], activation_id);
    assert_eq!(workspace.body["activeRun"]["cycle"], 2);
    assert_eq!(
        workspace.body["execution"]["chart"]["schemaVersion"],
        "tradeassembly.execution_chart.v2"
    );
    assert!(workspace.body["execution"]["evidence"]["apf"]["receipts"].is_array());
    assert!(
        workspace.body["execution"]["decisions"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    let analytics = &workspace.body["execution"]["executionAnalytics"];
    assert_eq!(
        analytics["schemaVersion"],
        "tradeassembly.execution_analytics.v2"
    );
    assert_eq!(analytics["activationId"], activation_id);
    assert!(analytics["liveVsBacktestDrift"]["status"].is_string());
    assert!(analytics["fillSlippageCalibration"]["submittedOrders"]
        .as_u64()
        .is_some());
    assert!(analytics["latencyDiagnostics"]["reason"].is_string());
    assert_eq!(
        analytics["capacityLiquidity"]["status"],
        "insufficient_data"
    );
    assert!(analytics["capacityLiquidity"]["requestedQuantity"].is_null());
    assert_eq!(analytics["stressMonitor"]["status"], "insufficient_data");
    assert!(analytics["stressMonitor"]["stressLossEstimate"].is_null());
    assert!(analytics["scenarioAlerts"]["items"].is_array());
    assert_eq!(workspace.body["executionAnalytics"], *analytics);
}

#[test]
fn execution_control_plane_metadata_scheduler_scope_and_replay_are_hardened() {
    let db = format!(
        ".tradeassembly/test-execution-hardening-{}.db",
        std::process::id()
    );
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);

    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {"max_notional": 25, "max_order_quantity": 0.0003}
        }),
    );
    assert_eq!(saved.status, 200);
    let config_id = saved.body["body"]["configId"].as_str().expect("config id");

    let activated_a = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "activation-hardening-a",
            "controlPlaneCommandId": "cmd_spoofed_activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activated_a.status, 200, "{:#}", activated_a.body);
    assert_ne!(
        activated_a.body["body"]["run"]["apf"]["descriptorRef"],
        "finance_action_descriptors/cmd_spoofed_activation"
    );
    let activation_a = activated_a.body["body"]["activationId"]
        .as_str()
        .expect("activation a");

    let stale_lease = service
        .runtime()
        .leases
        .acquire(
            &format!("execution-tick:{activation_a}:1"),
            "dead-worker",
            1,
            1,
        )
        .expect("store stale lease")
        .expect("stale lease claim");

    let activated_b = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "activation-hardening-b",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activated_b.status, 200, "{:#}", activated_b.body);
    let activation_b = activated_b.body["body"]["activationId"]
        .as_str()
        .expect("activation b");
    assert_ne!(activation_a, activation_b);

    let ticked = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_a,
            "workerId": "worker-hardening",
            "maxCycles": 1,
            "leaseTtlSeconds": 30
        }),
    );
    assert_eq!(ticked.status, 200, "{:#}", ticked.body);
    let ticks = ticked.body["body"]["ticks"].as_array().expect("ticks");
    assert_eq!(ticks.len(), 1, "{:#}", ticked.body);
    assert_eq!(ticks[0]["activationId"], activation_a);
    assert_eq!(ticks[0]["status"], "complete");
    assert_eq!(ticks[0]["lease"]["duplicate"], false);
    assert!(ticks[0]["lease"]["fencingToken"].as_i64().unwrap() > stale_lease.fencing_token);

    let workspace = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    let active_runs = workspace.body["activeRuns"]
        .as_array()
        .expect("active runs");
    let run_a = active_runs
        .iter()
        .find(|run| run["activationId"] == activation_a)
        .expect("run a");
    let run_b = active_runs
        .iter()
        .find(|run| run["activationId"] == activation_b)
        .expect("run b");
    assert_eq!(run_a["cycle"], 1);
    assert_eq!(run_b["cycle"], 0);

    let status = service.call_mcp_tool(
        "tradeassembly.execution.status",
        json!({"strategy_id": "strat_local_btc_demo"}),
    );
    let status_runs = status["structuredContent"]["workspace"]["activeRuns"]
        .as_array()
        .expect("status active runs");
    let status_run_a = status_runs
        .iter()
        .find(|run| run["activationId"] == activation_a)
        .expect("status run a");
    assert_eq!(status_run_a["cycle"], 1);

    let replay = service.call_mcp_tool(
        "tradeassembly.execution.replay",
        json!({"strategy_id": "strat_local_btc_demo"}),
    );
    assert_eq!(replay["structuredContent"]["ok"], true);
    assert!(replay["structuredContent"]["workspace"]["activeRuns"]
        .as_array()
        .expect("replay active runs")
        .iter()
        .any(|run| run["activationId"] == activation_a && run["cycle"] == 1));
    assert!(!replay["structuredContent"]["apfAudit"]["receipts"]
        .as_array()
        .expect("apf receipts")
        .is_empty());
    assert!(replay["structuredContent"]["apfAudit"]["descriptors"]
        .as_array()
        .expect("apf descriptors")
        .iter()
        .any(|descriptor| descriptor["source_interface"]["interface_kind"] == "scheduler"));
}

#[test]
fn paper_execution_config_activation_controls_and_status_share_all_transports() {
    let db = format!(
        ".tradeassembly/test-execution-transport-parity-{}.db",
        std::process::id()
    );
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);

    let config = execution_cli(
        &service,
        &[
            "config-save",
            "--strategy-id",
            "strat_local_btc_demo",
            "--mode",
            "paper",
            "--provider-ref",
            "sim",
        ],
    );
    assert_eq!(config["ok"], true, "{config:#}");
    let config_id = config["body"]["configId"].as_str().expect("config id");

    let readiness = execution_cli(
        &service,
        &[
            "readiness",
            "--config-id",
            config_id,
            "--strategy-id",
            "strat_local_btc_demo",
        ],
    );
    assert_eq!(readiness["body"]["ready"], true, "{readiness:#}");

    let activated = execution_cli(
        &service,
        &[
            "activate",
            "--config-id",
            config_id,
            "--strategy-id",
            "strat_local_btc_demo",
            "--idempotency-key",
            "transport-parity-cli-activation",
        ],
    );
    assert_eq!(activated["body"]["status"], "active", "{activated:#}");
    let activation_id = activated["body"]["activationId"]
        .as_str()
        .expect("activation id");

    let http = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({
            "activationId": activation_id,
            "action": "pause_entries",
            "idempotencyKey": "transport-parity-http-pause",
        }),
    );
    assert_eq!(http.status, 200, "{:#}", http.body);
    assert_eq!(http.body["body"]["state"], "paused");

    let graphql = service.execute_graphql(json!({
        "operationName": "ControlStrategyExecution",
        "query": "mutation ControlStrategyExecution { controlStrategyExecution }",
        "variables": {
            "activationId": activation_id,
            "action": "resume_entries",
            "idempotencyKey": "transport-parity-graphql-resume",
        }
    }));
    assert!(graphql.get("errors").is_none(), "{graphql:#}");
    assert_eq!(
        graphql["data"]["controlStrategyExecution"]["body"]["state"], "resumed",
        "{graphql:#}"
    );

    let mcp = service.call_mcp_tool(
        "tradeassembly.execution.control",
        json!({
            "activation_id": activation_id,
            "action": "pause_entries",
            "idempotency_key": "transport-parity-mcp-pause",
        }),
    );
    assert_eq!(mcp["isError"], false, "{mcp:#}");
    assert_eq!(mcp["structuredContent"]["body"]["state"], "paused");

    let cli = execution_cli(
        &service,
        &[
            "control",
            "--activation-id",
            activation_id,
            "--action",
            "resume_entries",
        ],
    );
    assert_eq!(cli["body"]["state"], "resumed", "{cli:#}");

    let status = execution_cli(
        &service,
        &["status", "--strategy-id", "strat_local_btc_demo"],
    );
    assert!(status["activeRuns"]
        .as_array()
        .expect("active runs")
        .iter()
        .any(|run| run["activationId"] == activation_id));

    let durable = service
        .runtime()
        .storage
        .get_json("execution_controls", activation_id)
        .expect("read controls")
        .expect("durable controls");
    assert_eq!(durable["pauseEntries"], "available");
    assert_eq!(durable["resumeEntries"], "resumed");
    assert_eq!(durable["lastControl"]["action"], "resume_entries");
}

#[test]
fn live_activation_fails_closed_across_http_graphql_and_mcp() {
    let db = format!(
        ".tradeassembly/test-live-fail-closed-{}.db",
        std::process::id()
    );
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);

    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "live",
            "providerRef": "alpaca-paper",
            "riskLimits": {"max_notional": 25, "max_order_quantity": 0.0003}
        }),
    );
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["body"]["readiness"]["ready"], false);
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["schemaVersion"],
        "tradeassembly.live_activation_preflight.v1"
    );
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["client"],
        "self"
    );
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["purpose"],
        "live_order_submission"
    );
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["apf"]["requiredPepCoverage"],
        "C5"
    );
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["apf"]["currentPepCoverage"],
        "C5"
    );
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["apf"]["downstreamPepCoverage"],
        "not_registered"
    );
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["legal"]["userOnly"],
        true
    );
    let blocked = saved.body["body"]["readiness"]["blockedReasons"]
        .as_array()
        .expect("blocked reasons");
    assert!(blocked
        .iter()
        .any(|reason| reason.as_str() == Some("legal_acknowledgement")));
    assert!(blocked
        .iter()
        .any(|reason| reason.as_str() == Some("live_credential_posture")));
    assert!(blocked
        .iter()
        .any(|reason| reason.as_str() == Some("downstream_pep_coverage_c5")));
    assert!(!blocked.iter().any(|reason| reason
        .as_str()
        .is_some_and(|value| value.starts_with("cloud_"))));
    assert_eq!(
        saved.body["body"]["readiness"]["livePreflight"]["cloudProfile"]["status"],
        "not_applicable"
    );
    let preflight_blocked = saved.body["body"]["readiness"]["livePreflight"]["blockedReasons"]
        .as_array()
        .expect("live preflight blocked reasons");
    assert_eq!(preflight_blocked, blocked);
    let acknowledgements = saved.body["body"]["readiness"]["acknowledgements"]
        .as_array()
        .expect("acknowledgements");
    assert!(acknowledgements.iter().any(|ack| {
        ack["id"] == "legal_live_trading" && ack["accepted"] == false && ack["userOnly"] == true
    }));
    let config_id = saved.body["body"]["configId"].as_str().expect("config id");

    let http_activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "live-http-denied"
        }),
    );
    assert_eq!(http_activated.status, 403);
    assert_eq!(
        http_activated.body["error"]["code"],
        "finance_authority_denied"
    );
    assert_eq!(
        http_activated.body["error"]["details"]["readiness"]["livePreflight"]["schemaVersion"],
        "tradeassembly.live_activation_preflight.v1"
    );
    assert!(http_activated.body["error"]["details"]["blockedReasons"]
        .as_array()
        .expect("http activation blocked reasons")
        .iter()
        .any(|reason| reason.as_str() == Some("downstream_pep_coverage_c5")));

    let graphql_activated = service.execute_graphql(json!({
        "operationName": "ActivateExecution",
        "query": "mutation ActivateExecution { activateExecution }",
        "variables": {
            "configId": config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "live-graphql-denied"
        }
    }));
    assert_eq!(
        graphql_activated["errors"][0]["message"],
        "finance_authority_denied"
    );
    assert_eq!(
        graphql_activated["errors"][0]["extensions"]["details"]["readiness"]["livePreflight"]
            ["schemaVersion"],
        "tradeassembly.live_activation_preflight.v1"
    );

    let mcp_activated = service.call_mcp_tool(
        "tradeassembly.execution.activate",
        json!({
            "config_id": config_id,
            "strategy_id": "strat_local_btc_demo",
            "idempotency_key": "live-mcp-denied"
        }),
    );
    assert_eq!(mcp_activated["isError"], true);
    assert_eq!(
        mcp_activated["structuredContent"]["error"]["code"],
        "finance_authority_denied"
    );
    assert_eq!(
        mcp_activated["structuredContent"]["details"]["readiness"]["livePreflight"]
            ["schemaVersion"],
        "tradeassembly.live_activation_preflight.v1"
    );
}

#[test]
fn caller_cannot_inject_the_responsible_human_legal_identity() {
    let db = format!(
        ".tradeassembly/test-live-legal-receipt-{}.db",
        std::process::id()
    );
    let base = TradeAssemblyService::test_local(&db);
    let service = base.for_authenticated_invocation(
        "https://hub.tradeassembly.org",
        "subject_123",
        Some("human@example.com".to_string()),
        Some("Responsible Human".to_string()),
    );

    let saved = service.handle_http_from_source(
        "studio",
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "live",
            "providerRef": "alpaca-paper",
            "legalReceiptRef": "receipt_live_123",
            "authorityContext": {"accountMode": "live"},
            "responsibleHuman": {
                "identityIssuer": "https://attacker.example",
                "identitySubject": "attacker_subject"
            },
            "riskLimits": {"max_notional": 25, "max_order_quantity": 0.0003}
        }),
    );
    assert_eq!(saved.status, 200, "{:#?}", saved.body);
    assert_eq!(
        saved.body["body"]["item"]["responsibleHuman"]["identityIssuer"],
        "https://hub.tradeassembly.org"
    );
    assert_eq!(
        saved.body["body"]["item"]["responsibleHuman"]["identitySubject"],
        "subject_123"
    );
}

#[test]
fn execution_activation_survives_real_process_restart() {
    if std::env::var("TRADEASSEMBLY_EXECUTION_RESTART_CHILD").is_ok() {
        return;
    }
    let db = format!(
        ".tradeassembly/test-execution-process-restart-{}.db",
        std::process::id()
    );
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let handoff = TradeAssemblyService::test_local_handoff(&db).expect("fixture handoff");
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({"strategyId": "strat_local_btc_demo", "mode": "paper", "providerRef": "sim"}),
    );
    let config_id = saved.body["body"]["configId"].as_str().expect("config id");
    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({"configId": config_id, "strategyId": "strat_local_btc_demo", "idempotencyKey": "process-restart-activation", "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]}),
    );
    let activation_id = activated.body["body"]["activationId"]
        .as_str()
        .expect("activation id")
        .to_string();
    let ticked = service.handle_http(
        "POST",
        "/scheduler/run",
        json!({"activationId": activation_id, "workerId": "worker-before-restart", "maxCycles": 1}),
    );
    assert_eq!(ticked.status, 200);

    let output = ProcessCommand::new(std::env::current_exe().expect("current exe"))
        .arg("--exact")
        .arg("execution_process_restart_child_probe")
        .arg("--nocapture")
        .env("TRADEASSEMBLY_EXECUTION_RESTART_CHILD", "1")
        .env("TRADEASSEMBLY_EXECUTION_RESTART_DB", &db)
        .env("TRADEASSEMBLY_EXECUTION_RESTART_HANDOFF", &handoff)
        .env("TRADEASSEMBLY_EXECUTION_RESTART_ACTIVATION", &activation_id)
        .output()
        .expect("spawn child test");
    assert!(
        output.status.success(),
        "child failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn execution_process_restart_child_probe() {
    if std::env::var("TRADEASSEMBLY_EXECUTION_RESTART_CHILD").is_err() {
        return;
    }
    let db = std::env::var("TRADEASSEMBLY_EXECUTION_RESTART_DB").expect("db env");
    let handoff = std::env::var("TRADEASSEMBLY_EXECUTION_RESTART_HANDOFF").expect("handoff env");
    let activation_id =
        std::env::var("TRADEASSEMBLY_EXECUTION_RESTART_ACTIVATION").expect("activation env");
    let service =
        TradeAssemblyService::test_local_with_handoff(db, handoff).expect("fixture handoff");
    let workspace = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(workspace.status, 200);
    assert_eq!(workspace.body["activeRun"]["activationId"], activation_id);
    assert_eq!(workspace.body["activeRun"]["cycle"], 1);
    assert!(workspace.body["execution"]["scheduler"]["last_run_id"].is_string());
}
