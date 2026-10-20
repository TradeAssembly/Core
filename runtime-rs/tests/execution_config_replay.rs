// Copyright (c) 2026 OptionLab LLC. All rights reserved.
use serde_json::{json, Value};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn identical_mcp_config_request_replays_before_and_after_saved_context_exists() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("runtime.sqlite");
    let owner = TradeAssemblyService::test_local(database.to_str().unwrap())
        .for_authenticated_invocation("local-owner", "owner", None, None);
    let request = json!({
        "strategy_id":"strat_local_btc_demo", "config_id":"cfg_replay_contract",
        "provider_ref":"sim", "mode":"paper", "symbol":"BTC/USD", "timeframe":"1m",
        "account_ref":"account://sim/paper", "risk_limits":{"max_notional":25,"max_order_quantity":0.0001},
        "idempotency_key":"paper-config-replay-contract"
    });
    let first = owner.call_mcp_tool("tradeassembly.execution.config.save", request.clone());
    assert_eq!(
        first["structuredContent"]["body"]["configId"],
        "cfg_replay_contract"
    );
    let configurations = owner
        .runtime()
        .storage
        .list_json("execution_configs")
        .unwrap();
    assert_eq!(configurations.len(), 1);
    let connection = rusqlite::Connection::open(&database).unwrap();
    let effect_count = || {
        connection.query_row(
        "SELECT count(*) FROM runtime_events WHERE json_valid(payload_json) AND json_extract(payload_json,'$.event_type')='execution.config.saved'",
        [], |row| row.get::<_, u64>(0)).unwrap()
    };
    assert_eq!(effect_count(), 1);
    let replay = owner.call_mcp_tool("tradeassembly.execution.config.save", request.clone());
    assert_eq!(
        replay["structuredContent"]["body"]["configId"], "cfg_replay_contract",
        "{}",
        replay["structuredContent"]["error"]
    );
    assert_eq!(
        owner
            .runtime()
            .storage
            .list_json("execution_configs")
            .unwrap(),
        configurations
    );
    assert_eq!(effect_count(), 1);
    let mut changed = request;
    changed["risk_limits"]["max_notional"] = Value::from(26);
    let rejected = owner.call_mcp_tool("tradeassembly.execution.config.save", changed);
    assert_eq!(rejected["isError"], true);
    assert_eq!(
        rejected["structuredContent"]["error"]["code"],
        "idempotency_conflict"
    );
    assert_eq!(
        owner
            .runtime()
            .storage
            .list_json("execution_configs")
            .unwrap(),
        configurations
    );
    assert_eq!(effect_count(), 1);
}
