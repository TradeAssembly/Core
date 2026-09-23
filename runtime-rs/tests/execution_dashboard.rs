use clap::Parser;
use serde_json::{json, Value};
use tradeassembly_runtime::cli::{execute_command, Cli};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

fn test_db(name: &str) -> String {
    format!(
        ".tradeassembly/test-execution-dashboard-{name}-{}.db",
        std::process::id()
    )
}

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("test idempotency key"),
    )
}

fn activate(service: &TradeAssemblyService, suffix: &str) -> Value {
    activate_with_orchestrator(service, suffix, "deterministic")
}

fn activate_with_orchestrator(
    service: &TradeAssemblyService,
    suffix: &str,
    orchestrator: &str,
) -> Value {
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "orchestrator": orchestrator,
            "mode": "paper",
            "providerRef": "sim",
            "accountRef": "controlled-paper-account",
            "riskLimits": {
                "max_notional": 25,
                "max_order_quantity": 0.0003,
            },
            "capabilityBindings": {
                "execution.market.bars": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "marketdata.bars.read_v1",
                },
                "execution.broker.submit": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "broker.paper_order_submit",
                },
            },
        }),
    );
    assert_eq!(saved.status, 200, "{:#}", saved.body);
    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": saved.body["body"]["configId"],
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": format!("dashboard-{suffix}"),
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activated.status, 200, "{:#}", activated.body);
    activated.body["body"].clone()
}

#[test]
fn authenticated_external_session_attaches_to_real_activation_and_rejects_other_owner() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("attach.db");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    assert!(base
        .attach_external_agent_session("missing", "missing", "anonymous")
        .is_err());
    let owner = base.for_authenticated_invocation("test", "owner", None, None);
    let activated = activate_with_orchestrator(&owner, "external-owner", "external_agent");
    assert_eq!(activated["status"], "active");
    let activation_id = activated["activationId"].as_str().unwrap();
    let created = owner.call_mcp_tool("studio.deployment.create",json!({
        "deployment":{
            "executor":"external_client","deploymentId":"owner-external",
            "systemProjectId":"system-owner","agentDefinitionVersionId":"agent-v1",
            "executionConfigVersionId":activated["run"]["configId"],
            "studioToolAllowlist":["tradeassembly.health"],"desiredState":"active","mode":"paper"
        },
        "idempotency_key":"owner-deployment","authority_context":{"actor":"ignored","surface":"mcp","accountMode":"paper"}
    }));
    assert_eq!(created["isError"], false);
    let other = base.for_authenticated_invocation("test", "other", None, None);
    assert!(other
        .attach_external_agent_session("owner-external", activation_id, "other-attach")
        .is_err());
    assert_eq!(namespace_count(&owner, "agent_runs"), 0);
    let mut session = owner
        .attach_external_agent_session("owner-external", activation_id, "owner-attach")
        .unwrap_or_else(|error| {
            let config = owner
                .runtime()
                .storage
                .get_json(
                    "execution_configs",
                    activated["run"]["configId"].as_str().unwrap(),
                )
                .unwrap()
                .unwrap();
            let record = owner
                .runtime()
                .storage
                .get_json("execution_activations", activation_id)
                .unwrap()
                .unwrap();
            let diff: Vec<_> = [
                "strategyVersionId",
                "strategySpecHash",
                "capabilityGraphRevisionId",
                "accountRef",
                "riskLimits",
            ]
            .into_iter()
            .filter(|field| record[*field].is_null() || record[*field] != config[*field])
            .map(|field| (field, config[field].clone(), record[field].clone()))
            .collect();
            panic!(
                "attach failed: {}; binding differences: {diff:?}",
                error.body
            )
        });
    let scoped =
        owner.with_verified_agent_mcp_execution_context(session.verified_context().unwrap());
    assert_eq!(
        scoped.call_mcp_tool("tradeassembly.health", json!({}))["isError"],
        false
    );
    assert!(scoped
        .attach_external_agent_session("owner-external", activation_id, "nested")
        .is_err());
    assert!(owner
        .attach_external_agent_session("owner-external", activation_id, "duplicate-session")
        .is_err());
    session.close().unwrap();
    assert_eq!(
        scoped.call_mcp_tool("tradeassembly.health", json!({}))["isError"],
        true
    );
    assert_eq!(namespace_count(&owner, "scheduler_state"), 0);
    assert_eq!(namespace_count(&owner, "execution_ticks"), 0);

    tradeassembly_runtime::agent_runner::recover_pending_run(
        &owner.runtime(),
        "owner-external",
        owner.runtime().clock.trusted_now_ms().unwrap(),
    )
    .unwrap();
    let connection =
        tradeassembly_runtime::cli::external_mcp::ExternalMcpConnection::new().unwrap();
    let args = json!({"deployment_id":"owner-external","activation_id":activation_id,"idempotency_key":"connection-attach"});
    let attached = connection.attach(&owner, &args).unwrap();
    assert_eq!(connection.attach(&owner, &args).unwrap(), attached);
    assert!(connection.attach(&other, &args).is_err());
    let connection_context = connection.execution_context().unwrap().unwrap();
    let original_lease = connection_context
        .current_lease(
            owner.runtime().storage.as_ref(),
            owner.runtime().clock.as_ref(),
            owner.runtime().leases.as_ref(),
        )
        .unwrap();
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(
            tradeassembly_runtime::agent_runner::DEFAULT_HEARTBEAT_MS + 5_000,
        );
    loop {
        let lease = owner
            .runtime()
            .leases
            .current(&original_lease.resource)
            .unwrap()
            .unwrap();
        if lease.expires_at_ms > original_lease.expires_at_ms {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "idle connection did not renew its production lease"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        owner
            .with_verified_agent_mcp_execution_context(connection_context.clone())
            .call_mcp_tool("tradeassembly.health", json!({}))["isError"],
        false
    );
    drop(connection); // Same guard is dropped on stdio EOF.
    assert!(connection_context
        .current_lease(
            owner.runtime().storage.as_ref(),
            owner.runtime().clock.as_ref(),
            owner.runtime().leases.as_ref()
        )
        .is_err());
}

#[test]
fn external_instrument_universe_is_saved_without_inventing_a_primary_symbol() {
    let directory = tempfile::tempdir().unwrap();
    let service =
        TradeAssemblyService::test_local(directory.path().join("universe.db").to_string_lossy());
    let body = json!({"strategyId":"strat_local_btc_demo", "orchestrator":"external_agent", "mode":"paper", "allowed_symbols":["SPY","QQQ"]});
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        body.clone(),
    );
    assert_eq!(saved.status, 200);
    let config = &saved.body["body"]["item"];
    assert_eq!(config["allowedSymbols"], json!(["SPY", "QQQ"]));
    assert_eq!(config["symbol"], "");
    let mut different = body.clone();
    different["allowed_symbols"] = json!(["SPY", "IWM"]);
    let other = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        different,
    );
    assert_ne!(config["configId"], other.body["body"]["item"]["configId"]);
    let before = namespace_count(&service, "execution_configs");
    for invalid in [
        json!({"orchestrator":"deterministic","allowed_symbols":["SPY","QQQ"]}),
        json!({"orchestrator":"external_agent","allowed_symbols":[]}),
        json!({"orchestrator":"external_agent","allowed_symbols":["SPY"],"allowedSymbols":["QQQ"]}),
        json!({"orchestrator":"external_agent","allowed_symbols":["SPY"],"symbol":"IWM"}),
    ] {
        let result =
            service.handle_http("POST", "/product/strategy-execution-configs/save", invalid);
        assert!(result.body.to_string().contains("execution_"));
        assert_eq!(namespace_count(&service, "execution_configs"), before);
    }
}

#[test]
fn external_agent_activation_is_durable_without_scheduler_state_or_ticks() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("external-agent.db");
    let service = TradeAssemblyService::test_local(db.to_string_lossy());
    let activated = activate_with_orchestrator(&service, "external", "external_agent");
    assert_eq!(activated["status"], "active", "{activated:#}");
    assert_eq!(activated["run"]["orchestrator"], "external_agent");
    assert_eq!(activated["scheduler"]["running"], false);
    assert_eq!(
        activated["run"]["health"]["status"],
        "awaiting_external_agent"
    );
    assert_eq!(namespace_count(&service, "scheduler_state"), 0);
    assert_eq!(namespace_count(&service, "execution_ticks"), 0);
    let activation_id = activated["activationId"].as_str().unwrap();
    let tick = service.handle_http_from_source(
        "scheduler",
        "POST",
        &format!("/product/strategy-execution-activations/{activation_id}/ticks/evaluate"),
        json!({"tickId":"must-not-run", "sequence":1,"idempotencyKey":"external-no-tick"}),
    );
    assert!(
        tick.body
            .to_string()
            .contains("external_agent_evaluation_required"),
        "{}",
        tick.body
    );
    let denied = service.handle_http(
        "POST",
        "/scheduler/start",
        json!({"activationId":activation_id}),
    );
    assert!(
        denied
            .body
            .to_string()
            .contains("external_agent_scheduler_not_applicable"),
        "{}",
        denied.body
    );
    assert_eq!(namespace_count(&service, "scheduler_state"), 0);
    drop(service);
    let reopened = TradeAssemblyService::test_local(db.to_string_lossy());
    let duplicate = reopened.handle_http("POST", "/product/strategy-execution-activations/activate", json!({
        "configId":activated["run"]["configId"], "activationId":activation_id,
        "idempotencyKey":"external-retry", "acknowledgementIds":["user_logic","user_risk","no_advice"]
    }));
    assert_eq!(
        duplicate.body["body"]["duplicate"], true,
        "{}",
        duplicate.body
    );
    assert_eq!(namespace_count(&reopened, "execution_runs"), 1);
    assert_eq!(namespace_count(&reopened, "scheduler_state"), 0);
    assert_eq!(namespace_count(&reopened, "execution_ticks"), 0);
    for action in ["stop", "resume_entries"] {
        let controlled = reopened.handle_http("POST", "/product/strategy-execution-activations/control", json!({"activationId":activation_id,"action":action,"idempotencyKey":format!("external-{action}")}));
        assert_eq!(
            controlled.body["body"]["outcome"]["status"], "completed",
            "{}",
            controlled.body
        );
    }
    let count: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM runtime_schedules", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        count, 0,
        "activation, manual start and resume must never enqueue ticks"
    );
}

#[test]
fn external_agent_uses_published_structure_without_claiming_deterministic_compilation() {
    let directory = tempfile::tempdir().unwrap();
    let base =
        TradeAssemblyService::test_local(directory.path().join("owner.db").to_string_lossy());
    let owner = base.for_authenticated_invocation("test", "owner", None, None);
    let spec: Value = serde_json::from_str(include_str!(
        "../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .unwrap();
    assert!(tradeassembly_runtime::spec::validate_strategy_spec_report(&spec).valid);
    assert!(tradeassembly_runtime::strategy_kernel::CompiledStrategy::compile_json(&spec).is_err());
    let created = owner.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name":"External owner evaluator", "spec":spec}),
    );
    let strategy_id = created.body["body"]["strategy"]["id"].clone();
    let published = owner.handle_http("POST", "/product/strategies/publish", json!({"strategyId":strategy_id,"expectedDraftHash":created.body["body"]["draft"]["draftHash"]}));
    assert_eq!(
        published.body["body"]["published"], true,
        "{}",
        published.body
    );
    let save = |orchestrator: &str, risk: Value| {
        owner.call_mcp_tool("tradeassembly.execution.config.save", json!({"strategy_id":strategy_id,"version_id":published.body["body"]["version"]["id"],"orchestrator":orchestrator,"provider_ref":"sim","symbol":"SPY","mode":"paper","risk_limits":risk}))["structuredContent"]["body"]["configId"].clone()
    };
    let risk = json!({"max_notional":100,"max_order_quantity":1});
    let external = save("external_agent", risk.clone());
    let deterministic = save("deterministic", risk);
    assert_ne!(external, deterministic);
    let readiness = |config: Value| {
        owner.call_mcp_tool(
            "tradeassembly.execution.readiness",
            json!({"config_id":config}),
        )["structuredContent"]["body"]
            .clone()
    };
    let ready = readiness(external.clone());
    assert_eq!(
        ready["ready"], true,
        "blockers: {}",
        ready["blockedReasons"]
    );
    assert_eq!(readiness(deterministic)["ready"], false);
    let activated = owner.call_mcp_tool("tradeassembly.execution.activate", json!({"config_id":external,"idempotency_key":"owner-external-activation","acknowledgement_ids":["user_logic","user_risk","no_advice"]}));
    assert_eq!(
        activated["structuredContent"]["body"]["status"], "active",
        "{activated:#}"
    );
    let invalid_risk = save(
        "external_agent",
        json!({"max_notional":0,"max_order_quantity":1}),
    );
    assert_eq!(
        readiness(invalid_risk)["ready"],
        false,
        "External evaluation must not bypass deterministic risk limits"
    );
}

fn put(service: &TradeAssemblyService, namespace: &str, key: &str, value: Value) {
    service
        .runtime()
        .storage
        .put_json(
            namespace,
            key,
            value,
            &context(&format!("dashboard:{namespace}:{key}")),
        )
        .expect("seed dashboard evidence");
}

fn namespace_count(service: &TradeAssemblyService, namespace: &str) -> usize {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .expect("list namespace")
        .len()
}

#[test]
fn dashboard_uses_compact_audit_reference_while_replay_keeps_complete_evidence() {
    let db = test_db("compact-audit-reference");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let _ = activate(&service, "compact-audit-reference");
    put(
        &service,
        "finance_receipts",
        "large-audit-receipt",
        json!({"receiptId": "large-audit-receipt", "payload": "x".repeat(1_000_000)}),
    );

    let dashboard = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(dashboard.status, 200, "{:#}", dashboard.body);
    assert_eq!(dashboard.body["apfAudit"]["scope"], "dashboard");
    assert_eq!(dashboard.body["apfAudit"]["complete"], false);
    assert_eq!(dashboard.body["apfAudit"]["receipts"], json!([]));
    assert_eq!(
        dashboard.body["execution"]["evidence"]["apf"]["complete"],
        false
    );
    assert!(
        serde_json::to_vec(&dashboard.body)
            .expect("serialize dashboard")
            .len()
            < 500_000,
        "routine dashboard must not embed complete authority evidence"
    );

    let replay = service.call_mcp_tool(
        "tradeassembly.execution.replay",
        json!({"strategy_id": "strat_local_btc_demo"}),
    );
    assert!(replay["structuredContent"]["apfAudit"]["receipts"]
        .as_array()
        .is_some_and(|receipts| !receipts.is_empty()));
    assert!(
        serde_json::to_vec(&replay["structuredContent"]["apfAudit"])
            .expect("serialize replay audit")
            .len()
            > 1_000_000,
        "explicit replay keeps complete authority evidence"
    );

    let _ = std::fs::remove_file(db);
}

#[test]
fn generated_execution_draft_is_not_reported_as_durable_configuration() {
    let db = test_db("generated-draft-not-durable");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);

    let response = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(response.status, 200, "{:#}", response.body);
    assert_eq!(response.body["configs"], json!([]));
    assert_eq!(
        response.body["draftConfig"]["kind"],
        "StrategyExecutionConfig"
    );
    assert!(response.body["draftConfig"]["configId"].is_string());
    assert!(response.body["activeRun"]["configId"].is_null());
    assert_eq!(namespace_count(&service, "execution_configs"), 0);

    let unsaved_activation = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {
                "max_notional": 25,
                "max_order_quantity": 0.0003,
            },
            "idempotencyKey": "unsaved-generated-draft",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(
        unsaved_activation.status, 400,
        "{:#}",
        unsaved_activation.body
    );
    assert_eq!(
        unsaved_activation.body["error"]["details"]["error"]["code"],
        "execution_config_required"
    );
    assert_eq!(namespace_count(&service, "execution_activations"), 0);

    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {
                "max_notional": 25,
                "max_order_quantity": 0.0003,
            },
        }),
    );
    assert_eq!(saved.status, 200, "{:#}", saved.body);
    let saved_config_id = saved.body["body"]["configId"].clone();

    let persisted = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(persisted.status, 200, "{:#}", persisted.body);
    assert_eq!(persisted.body["configs"].as_array().map(Vec::len), Some(1));
    assert_eq!(persisted.body["activeRun"]["configId"], saved_config_id);
}

#[test]
fn execution_workspace_orders_saved_configs_by_durable_recency() {
    let db = test_db("config-recency");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let save = |config_id: &str, symbol: &str| {
        let response = service.handle_http(
            "POST",
            "/product/strategy-execution-configs/save",
            json!({
                "configId": config_id,
                "strategyId": "strat_local_btc_demo",
                "mode": "paper",
                "providerRef": "sim",
                "dataProviderRef": "local-data",
                "symbol": symbol,
                "riskLimits": {
                    "max_notional": 25,
                    "max_order_quantity": 0.0003,
                },
            }),
        );
        assert_eq!(response.status, 200, "{:#}", response.body);
        response.body["body"]["item"].clone()
    };
    let mut newer = save("cfg-newer", "ETH/USD");
    let mut older = save("cfg-older", "BTC/USD");
    newer["savedAtMs"] = json!(20);
    older["savedAtMs"] = json!(10);
    put(&service, "execution_configs", "cfg-newer", newer);
    put(&service, "execution_configs", "cfg-older", older);

    let workspace = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(workspace.status, 200, "{:#}", workspace.body);
    assert_eq!(workspace.body["configs"][1]["configId"], "cfg-newer");
    assert_eq!(workspace.body["configs"][1]["symbol"], "ETH/USD");
}

#[test]
fn activation_replay_is_config_bound_and_initial_ledger_has_no_sample_evidence() {
    let db = test_db("activation-binding");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let first = activate(&service, "binding-first");
    let activation_id = first["activationId"].as_str().expect("activation id");
    let ledger = &first["run"]["ledger"];
    assert_eq!(ledger["orders"], json!([]));
    assert_eq!(ledger["positions"], json!([]));
    assert_eq!(ledger["totals"]["totalPnl"], 0.0);
    assert_eq!(ledger["reconciliation"]["state"], "clear");

    let duplicate = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "activationId": activation_id,
            "configId": first["run"]["configId"],
            "strategyId": first["run"]["strategyId"],
            "idempotencyKey": "binding-duplicate",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(duplicate.status, 200, "{:#}", duplicate.body);
    assert_eq!(duplicate.body["body"]["duplicate"], true);
    assert_eq!(
        duplicate.body["body"]["run"]["strategyId"],
        first["run"]["strategyId"]
    );

    let second_config = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "configId": "cfg_binding_conflict",
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {
                "max_notional": 50,
                "max_order_quantity": 0.0003,
            },
            "capabilityBindings": {
                "execution.market.bars": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "marketdata.bars.read_v1",
                },
                "execution.broker.submit": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "broker.paper_order_submit",
                },
            },
        }),
    );
    assert_eq!(second_config.status, 200, "{:#}", second_config.body);
    let conflict = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "activationId": activation_id,
            "configId": second_config.body["body"]["configId"],
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "binding-forgery",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(conflict.status, 400, "{:#}", conflict.body);
    let detail = &conflict.body["error"]["details"];
    assert_eq!(detail["status"], "blocked");
    assert_eq!(detail["failClosed"], true);
    assert_eq!(detail["error"]["code"], "activation_id_conflict");
    assert!(detail.get("run").is_none());
}

#[test]
fn dashboard_query_is_pure_activation_scoped_and_evidence_derived() {
    let db = test_db("scope-and-truth");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let left = activate(&service, "left");
    let right = activate(&service, "right");
    let left_id = left["activationId"].as_str().expect("left activation");
    let right_id = right["activationId"].as_str().expect("right activation");

    for (activation_id, symbol, price, order_id, reconciliation) in [
        (left_id, "BTC/USD", 43_210.0, "order-left", "clear"),
        (right_id, "ETH/USD", 2_345.0, "order-right", "required"),
    ] {
        put(
            &service,
            "execution_orders",
            order_id,
            json!({
                "id": order_id,
                "order_id": order_id,
                "activation_id": activation_id,
                "symbol": symbol,
                "side": "buy",
                "qty": 1.0,
                "status": "filled",
                "fill_price": price,
                "filled_at_ms": 1_784_000_000_000_i64,
            }),
        );
        put(
            &service,
            "execution_order_attempts",
            &format!("attempt-{order_id}"),
            json!({
                "id": format!("attempt-{order_id}"),
                "order_id": order_id,
                "activation_id": activation_id,
                "status": "filled",
            }),
        );
        put(
            &service,
            "positions",
            order_id,
            json!({
                "id": format!("position-{order_id}"),
                "order_id": order_id,
                "activation_id": activation_id,
                "strategy_id": "strat_local_btc_demo",
                "symbol": symbol,
                "status": "open",
                "qty": 1.0,
                "average_price": price,
                "mark_price": price + 5.0,
            }),
        );
        put(
            &service,
            "risk_reservations",
            order_id,
            json!({
                "id": format!("risk-{order_id}"),
                "order_id": order_id,
                "activation_id": activation_id,
                "status": "reserved",
                "symbol": symbol,
                "notional": price,
            }),
        );
        put(
            &service,
            "execution_ticks",
            &format!("{activation_id}:1"),
            json!({
                "schemaVersion": "tradeassembly.evaluation_tick.v1",
                "tickId": format!("tick-{order_id}"),
                "activationId": activation_id,
                "sequence": 1,
                "decision": {
                    "kind": "order_intent",
                    "facts": {
                        "symbol": symbol,
                        "openMicros": (price * 1_000_000.0) as i64,
                        "closeMicros": (price * 1_000_000.0) as i64,
                        "observedAtMs": 1_784_000_000_000_i64,
                        "pluginRef": "tradeassembly.simbroker",
                        "operationId": "marketdata.bars.read_v1",
                    },
                },
                "orderIntent": {
                    "clientOrderId": order_id,
                    "symbol": symbol,
                    "side": "buy",
                    "quantity": 1.0,
                },
                "orderResult": {
                    "order_id": order_id,
                    "status": "completed",
                },
                "status": "completed",
            }),
        );
        put(
            &service,
            "execution_reconciliation",
            activation_id,
            json!({
                "schemaVersion": "tradeassembly.execution_reconciliation.v1",
                "activationId": activation_id,
                "state": reconciliation,
                "newEntriesPaused": reconciliation != "clear",
                "reason": if reconciliation == "clear" {
                    Value::Null
                } else {
                    json!("broker_outcome_uncertain")
                },
                "evidenceRefs": [format!("journal://reconciliation/{activation_id}")],
                "pendingActions": [],
                "ambiguousActions": [],
            }),
        );
    }

    put(
        &service,
        "execution_order_attempts",
        "attempt-cross-activation-collision",
        json!({
            "id": "attempt-cross-activation-collision",
            "order_id": "order-right",
            "activation_id": left_id,
            "status": "filled",
        }),
    );
    put(
        &service,
        "positions",
        "position-cross-activation-collision",
        json!({
            "id": "position-cross-activation-collision",
            "order_id": "order-right",
            "activation_id": left_id,
            "strategy_id": "strat_local_btc_demo",
            "symbol": "LEAK/USD",
            "status": "open",
            "qty": 10.0,
            "average_price": 1.0,
            "mark_price": 999.0,
        }),
    );

    let configs_before = namespace_count(&service, "strategy_execution_configs");
    let runs_before = namespace_count(&service, "execution_runs");
    let response = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({
            "strategyId": "strat_local_btc_demo",
            "activationId": right_id,
            "eventLimit": 25,
        }),
    );
    assert_eq!(response.status, 200, "{:#}", response.body);
    assert_eq!(response.body["selectedActivationId"], right_id);
    assert_eq!(response.body["snapshot"]["mutationFree"], true);

    let orders = response.body["execution"]["orders"]["items"]
        .as_array()
        .expect("scoped orders");
    assert_eq!(orders.len(), 1, "{orders:#?}");
    assert_eq!(orders[0]["order_id"], "order-right");
    let positions = response.body["execution"]["positions"]["items"]
        .as_array()
        .expect("scoped positions");
    assert_eq!(positions.len(), 1, "{positions:#?}");
    assert_eq!(positions[0]["symbol"], "ETH/USD");
    assert_eq!(
        response.body["execution"]["reconciliation"]["state"],
        "required"
    );
    assert_eq!(
        response.body["execution"]["chart"]["priceSeries"][0]["symbol"],
        "ETH/USD"
    );
    assert_eq!(
        response.body["execution"]["chart"]["priceSeries"][0]["close"],
        2345.0
    );
    assert_eq!(
        response.body["execution"]["ledger"]["totals"]["unrealizedPnl"],
        5.0
    );
    assert!(
        !response.body.to_string().contains("order-left"),
        "left activation leaked into right dashboard: {:#}",
        response.body
    );
    assert!(
        !response
            .body
            .to_string()
            .contains("cross-activation-collision"),
        "foreign objects with colliding order ids leaked into right dashboard: {:#}",
        response.body
    );
    assert_eq!(
        namespace_count(&service, "strategy_execution_configs"),
        configs_before
    );
    assert_eq!(namespace_count(&service, "execution_runs"), runs_before);

    let repeated = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({
            "strategyId": "strat_local_btc_demo",
            "activationId": right_id,
            "eventLimit": 25,
        }),
    );
    assert_eq!(repeated.body["snapshot"], response.body["snapshot"]);
    assert_eq!(
        repeated.body["execution"]["chart"],
        response.body["execution"]["chart"]
    );
}

#[test]
fn dashboard_selection_and_cursor_are_consistent_across_interfaces() {
    let db = test_db("interface-parity");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "interfaces");
    let activation_id = activation["activationId"]
        .as_str()
        .expect("activation id")
        .to_string();
    let controlled = service.handle_http_from_source(
        "cli",
        "POST",
        "/product/strategy-execution-activations/control",
        json!({
            "activationId": activation_id,
            "action": "pause_entries",
            "idempotencyKey": "dashboard-pause-interfaces",
            "authorityContext": {
                "actor": "local-user",
                "surface": "cli",
                "accountMode": "paper",
            },
        }),
    );
    assert_eq!(controlled.status, 200, "{:#}", controlled.body);

    let request = json!({
        "strategyId": "strat_local_btc_demo",
        "activationId": activation_id,
        "afterSequence": 1,
        "eventLimit": 1,
    });
    let http = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        request.clone(),
    );
    assert_eq!(http.status, 200, "{:#}", http.body);
    assert_eq!(http.body["events"].as_array().map(Vec::len), Some(1));

    let graphql = service.execute_graphql(json!({
        "operationName": "StrategyExecutionWorkspace",
        "query": "query StrategyExecutionWorkspace { strategyExecutionWorkspace }",
        "variables": request,
    }));
    assert!(graphql.get("errors").is_none(), "{graphql:#}");
    let graphql_workspace = &graphql["data"]["strategyExecutionWorkspace"];

    let mcp = service.call_mcp_tool(
        "tradeassembly.execution.status",
        json!({
            "strategy_id": "strat_local_btc_demo",
            "activation_id": activation_id,
            "after_sequence": 1,
            "event_limit": 1,
        }),
    );
    let mcp_workspace = &mcp["structuredContent"]["workspace"];

    let cli = Cli::try_parse_from([
        "tradeassembly",
        "execution",
        "status",
        "--strategy-id",
        "strat_local_btc_demo",
        "--activation-id",
        activation_id.as_str(),
        "--after-sequence",
        "1",
        "--event-limit",
        "1",
    ])
    .expect("execution status CLI parses");
    let cli_workspace = execute_command(&service, cli.command);

    for workspace in [graphql_workspace, mcp_workspace, &cli_workspace] {
        assert_eq!(workspace["selectedActivationId"], activation_id);
        assert_eq!(
            workspace["snapshot"]["snapshotId"],
            http.body["snapshot"]["snapshotId"]
        );
        assert_eq!(workspace["snapshot"]["mutationFree"], true);
    }
    assert_eq!(graphql_workspace["events"], http.body["events"]);
    assert_eq!(mcp_workspace["events"], http.body["events"]);
}

#[test]
fn explicit_unknown_activation_does_not_fall_back_to_another_run() {
    let db = test_db("unknown-activation");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let actual = activate(&service, "known");

    let response = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({
            "strategyId": "strat_local_btc_demo",
            "activationId": "activation_missing",
        }),
    );

    assert_eq!(response.status, 200, "{:#}", response.body);
    assert_eq!(response.body["selection"]["status"], "activation_not_found");
    assert_eq!(response.body["selectedActivationId"], Value::Null);
    assert_ne!(
        response.body["selectedActivationId"], actual["activationId"],
        "an explicit unknown activation must not select another run"
    );
    assert_eq!(response.body["events"], json!([]));
    assert_eq!(response.body["execution"]["orders"]["items"], json!([]));
    assert_eq!(response.body["execution"]["positions"]["items"], json!([]));
}

#[test]
fn audited_cancel_and_liquidate_are_idempotent_durable_and_activation_scoped() {
    let db = test_db("audited-controls");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let left = activate(&service, "controls-left");
    let right = activate(&service, "controls-right");
    let left_id = left["activationId"].as_str().expect("left activation");
    let right_id = right["activationId"].as_str().expect("right activation");

    for (activation_id, order_id) in [(left_id, "order-left-open"), (right_id, "order-right-open")]
    {
        put(
            &service,
            "execution_orders",
            order_id,
            json!({
                "id": order_id,
                "order_id": order_id,
                "activation_id": activation_id,
                "strategy_id": "strat_local_btc_demo",
                "symbol": "BTC/USD",
                "side": "buy",
                "qty": 1.0,
                "status": "submitted",
                "brokerExecution": "local-simulated",
            }),
        );
        put(
            &service,
            "risk_reservations",
            order_id,
            json!({
                "id": format!("risk-{order_id}"),
                "order_id": order_id,
                "activation_id": activation_id,
                "status": "reserved",
                "notional": 10.0,
            }),
        );
    }
    put(
        &service,
        "execution_orders",
        "order-right-position",
        json!({
            "id": "order-right-position",
            "order_id": "order-right-position",
            "activation_id": right_id,
            "strategy_id": "strat_local_btc_demo",
            "symbol": "BTC/USD",
            "side": "buy",
            "qty": 2.0,
            "status": "filled",
            "brokerExecution": "local-simulated",
            "fill_price": 100.0,
        }),
    );
    put(
        &service,
        "positions",
        "order-right-position",
        json!({
            "id": "position-right",
            "order_id": "order-right-position",
            "activation_id": right_id,
            "strategy_id": "strat_local_btc_demo",
            "symbol": "BTC/USD",
            "side": "long",
            "qty": 2.0,
            "status": "open",
            "average_price": 100.0,
            "mark_price": 105.0,
        }),
    );

    let cancel_request = json!({
        "activationId": right_id,
        "action": "cancel_orders",
        "idempotencyKey": "controls-cancel-right",
        "authorityContext": {
            "actor": "local-user",
            "surface": "studio",
            "accountMode": "paper",
        },
    });
    let canceled = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        cancel_request.clone(),
    );
    assert_eq!(canceled.status, 200, "{:#}", canceled.body);
    let cancel_outcome = &canceled.body["body"]["outcome"];
    assert_eq!(cancel_outcome["status"], "completed");
    assert_eq!(cancel_outcome["actor"]["actor"], "local-user");
    assert_eq!(
        cancel_outcome["affected"]["orderRefs"],
        json!(["order-right-open"])
    );
    assert!(cancel_outcome["commandId"].as_str().is_some());
    assert_eq!(cancel_outcome["idempotencyKey"], "controls-cancel-right");

    let duplicate = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        cancel_request,
    );
    assert_eq!(duplicate.status, 200, "{:#}", duplicate.body);
    assert_eq!(duplicate.body["duplicate"], true);
    assert_eq!(duplicate.body["body"]["outcome"], *cancel_outcome);

    let liquidated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({
            "activationId": right_id,
            "action": "liquidate",
            "idempotencyKey": "controls-liquidate-right",
            "authorityContext": {
                "actor": "local-user",
                "surface": "studio",
                "accountMode": "paper",
            },
        }),
    );
    assert_eq!(liquidated.status, 200, "{:#}", liquidated.body);
    let liquidation_outcome = &liquidated.body["body"]["outcome"];
    assert_eq!(liquidation_outcome["status"], "completed");
    assert_eq!(
        liquidation_outcome["affected"]["positionRefs"],
        json!(["position-right"])
    );

    let orders = service
        .runtime()
        .storage
        .list_json("execution_orders")
        .expect("orders");
    let order = |id: &str| {
        orders
            .iter()
            .find(|(_, value)| value["order_id"] == id)
            .map(|(_, value)| value)
            .expect("order")
    };
    assert_eq!(order("order-left-open")["status"], "submitted");
    assert_eq!(order("order-right-open")["status"], "canceled");
    assert_eq!(
        order("order-right-open")["controlOutcomeRef"],
        cancel_outcome["commandId"]
    );
    let liquidation_order = orders
        .iter()
        .map(|(_, value)| value)
        .find(|value| value["liquidatesPositionRef"] == "position-right")
        .expect("liquidation order");
    assert_eq!(liquidation_order["status"], "filled");
    assert_eq!(liquidation_order["side"], "sell");

    let positions = service
        .runtime()
        .storage
        .list_json("positions")
        .expect("positions");
    let right_position = positions
        .iter()
        .find(|(_, value)| value["id"] == "position-right")
        .map(|(_, value)| value)
        .expect("right position");
    assert_eq!(right_position["status"], "closed");
    assert_eq!(
        right_position["closeReason"],
        "user_requested_paper_liquidation"
    );

    let restarted = TradeAssemblyService::test_local(&db);
    assert_eq!(namespace_count(&restarted, "execution_control_outcomes"), 2);
    let workspace = restarted.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({
            "strategyId": "strat_local_btc_demo",
            "activationId": right_id,
        }),
    );
    assert_eq!(workspace.status, 200, "{:#}", workspace.body);
    assert_eq!(
        workspace.body["execution"]["orders"]["items"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
    assert_eq!(
        workspace.body["execution"]["controlOutcomes"]["items"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}

#[test]
fn external_plugin_cancel_and_liquidate_fail_closed_with_affected_refs() {
    let db = test_db("external-control-fail-closed");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "external-controls");
    let activation_id = activation["activationId"].as_str().expect("activation id");
    put(
        &service,
        "execution_orders",
        "order-external",
        json!({
            "id": "order-external",
            "order_id": "order-external",
            "activation_id": activation_id,
            "strategy_id": "strat_local_btc_demo",
            "symbol": "BTC/USD",
            "side": "buy",
            "qty": 1.0,
            "status": "submitted",
            "brokerExecution": "alpaca-paper",
        }),
    );
    put(
        &service,
        "positions",
        "order-external",
        json!({
            "id": "position-external",
            "order_id": "order-external",
            "activation_id": activation_id,
            "strategy_id": "strat_local_btc_demo",
            "symbol": "BTC/USD",
            "side": "long",
            "qty": 1.0,
            "status": "open",
        }),
    );

    for (action, blocked_ref) in [
        ("cancel_orders", "order-external"),
        ("liquidate", "position-external"),
    ] {
        let response = service.handle_http(
            "POST",
            "/product/strategy-execution-activations/control",
            json!({
                "activationId": activation_id,
                "action": action,
                "idempotencyKey": format!("external-{action}"),
            }),
        );
        assert_eq!(response.status, 200, "{:#}", response.body);
        let outcome = &response.body["body"]["outcome"];
        assert_eq!(outcome["status"], "rejected");
        assert_eq!(
            outcome["failureReason"],
            "external_plugin_control_operation_unavailable"
        );
        assert_eq!(outcome["affected"]["blockedRefs"], json!([blocked_ref]));
    }

    let order = service
        .runtime()
        .storage
        .get_json("execution_orders", "order-external")
        .expect("read order")
        .expect("order");
    assert_eq!(order["status"], "submitted");
    let position = service
        .runtime()
        .storage
        .get_json("positions", "order-external")
        .expect("read position")
        .expect("position");
    assert_eq!(position["status"], "open");
}

#[test]
fn stop_and_emergency_stop_do_not_silently_cancel_or_liquidate() {
    let db = test_db("stop-semantics");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let normal = activate(&service, "normal-stop");
    let emergency = activate(&service, "emergency-stop");
    let normal_id = normal["activationId"].as_str().expect("normal activation");
    let emergency_id = emergency["activationId"]
        .as_str()
        .expect("emergency activation");

    for (activation_id, suffix) in [(normal_id, "normal"), (emergency_id, "emergency")] {
        put(
            &service,
            "execution_orders",
            &format!("order-{suffix}"),
            json!({
                "id": format!("order-{suffix}"),
                "order_id": format!("order-{suffix}"),
                "activation_id": activation_id,
                "strategy_id": "strat_local_btc_demo",
                "symbol": "BTC/USD",
                "side": "buy",
                "qty": 1.0,
                "status": "submitted",
                "brokerExecution": "local-simulated",
            }),
        );
        put(
            &service,
            "positions",
            &format!("position-{suffix}"),
            json!({
                "id": format!("position-{suffix}"),
                "order_id": format!("order-{suffix}"),
                "activation_id": activation_id,
                "strategy_id": "strat_local_btc_demo",
                "symbol": "BTC/USD",
                "qty": 1.0,
                "status": "open",
            }),
        );
    }

    for (activation_id, action, expected_state) in [
        (normal_id, "stop", "stopped"),
        (emergency_id, "emergency_stop", "emergency_stopped"),
    ] {
        let response = service.handle_http(
            "POST",
            "/product/strategy-execution-activations/control",
            json!({
                "activationId": activation_id,
                "action": action,
                "idempotencyKey": format!("{action}-{activation_id}"),
            }),
        );
        assert_eq!(response.status, 200, "{:#}", response.body);
        assert_eq!(response.body["body"]["outcome"]["status"], "completed");
        assert_eq!(
            response.body["body"]["outcome"]["resultingState"]["runState"],
            expected_state
        );
        assert_eq!(
            response.body["body"]["outcome"]["reconciliationConsequence"],
            "evaluation_stopped_orders_and_positions_unchanged"
        );
    }

    for (_, value) in service
        .runtime()
        .storage
        .list_json("execution_orders")
        .expect("orders")
    {
        assert_eq!(value["status"], "submitted");
    }
    for (_, value) in service
        .runtime()
        .storage
        .list_json("positions")
        .expect("positions")
    {
        assert_eq!(value["status"], "open");
    }
}
