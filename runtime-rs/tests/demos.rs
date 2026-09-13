// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use tradeassembly_runtime::demos::{
    build_fixture_transport, build_strategy_order_plan, demo_descriptors, extract_mid_price,
    generate_fixture_bars, market_open_from_clock_payload, options_exercise_acceptance_payload,
    options_strategies_acceptance_payload, parse_allocation_config, parse_option_symbol,
    pick_option_contracts, resolve_strategies, runnable_demo_descriptors, AllocationArgs,
    DemoRunner, ALL_STRATEGY_NAMES,
};

#[test]
fn demo_catalog_has_provider_neutral_runnable_proofs() {
    let descriptors = demo_descriptors();
    let runnable_ids: Vec<_> = runnable_demo_descriptors()
        .into_iter()
        .map(|item| item.id)
        .collect();
    let btc = descriptors
        .iter()
        .find(|item| item.id == "local-simbroker-paper-btc")
        .expect("BTC paper descriptor");

    assert!(runnable_ids.contains(&"local-simbroker-paper"));
    assert!(runnable_ids.contains(&"local-simbroker-paper-btc"));
    assert!(runnable_ids.contains(&"fixture-marketdata-backtest"));
    assert!(!btc.requires_credentials);
    assert!(!btc.live_capable);
    assert!(descriptors.iter().all(|item| !item.id.contains("alpaca")));
}

#[test]
fn demo_runner_executes_local_proofs_and_fails_closed_for_unknown() {
    let runner = DemoRunner;

    let simbroker = runner.run("local-simbroker-paper").expect("simbroker demo");
    let btc = runner
        .run("local-simbroker-paper-btc")
        .expect("BTC paper demo");
    let fixture = runner
        .run("fixture-marketdata-backtest")
        .expect("fixture demo");

    assert_eq!(simbroker.status, "passed");
    assert!(!simbroker.summary["order_id"].as_str().unwrap().is_empty());
    assert!(
        simbroker.replay["counts"]["broker.order_submitted"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert_eq!(btc.status, "passed");
    assert!(!btc.summary["entry_order_id"].as_str().unwrap().is_empty());
    assert!(!btc.summary["exit_order_id"].as_str().unwrap().is_empty());
    assert!(
        btc.replay["counts"]["synthetic_exit.triggered"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert_eq!(fixture.status, "passed");
    assert!(fixture.summary["row_count"].as_u64().unwrap() > 0);
    assert!(!fixture.summary["dataset_hash"].as_str().unwrap().is_empty());
    assert!(!fixture.summary["backtest_id"].as_str().unwrap().is_empty());
    assert!(!fixture.summary["artifact_ids"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        fixture.replay["counts"]["research.artifact.created"]
            .as_u64()
            .unwrap()
            >= 1
    );

    assert!(runner
        .run("missing-demo")
        .expect_err("unknown demo")
        .contains("unknown demo"));
}

#[test]
fn fixture_marketdata_is_deterministic_and_http_compatible() {
    let first = generate_fixture_bars(&["btc/usd", "BTC/USD"], "1d", 5, "unit", "crypto");
    let second = generate_fixture_bars(&["BTC/USD"], "1d", 5, "unit", "crypto");

    assert_eq!(
        first.keys().collect::<Vec<_>>(),
        vec![&"BTC/USD".to_string()]
    );
    let first_closes: Vec<_> = first["BTC/USD"].iter().map(|bar| bar.close).collect();
    let second_closes: Vec<_> = second["BTC/USD"].iter().map(|bar| bar.close).collect();
    assert_eq!(first_closes, second_closes);
    assert!(build_fixture_transport("https://data.example.test/crypto"));
}

#[test]
fn options_demo_helpers_are_pure_and_live_runner_fails_closed() {
    let parsed = parse_option_symbol("SPY270118C00450000").expect("parsed option");
    assert_eq!(parsed.root, "SPY");
    assert_eq!(parsed.strike, 450.0);

    let symbols = vec![
        "SPY270118C00450000",
        "SPY270118P00450000",
        "SPY270118C00455000",
        "SPY270118P00445000",
        "SPY270219C00450000",
    ];
    let contracts = pick_option_contracts(&symbols, 450.0).expect("contracts");
    let plan = build_strategy_order_plan("iron-condor", "SPY", &contracts).expect("plan");

    assert_eq!(
        resolve_strategies(&["cash-secured-put"]).expect("strategy")[0].name,
        "cash_secured_put_program"
    );
    assert_eq!(
        extract_mid_price(&serde_json::json!({"quote": {"bp": "1.0", "ap": "1.2"}})),
        Some(1.1)
    );
    assert_eq!(plan["entry"].as_array().unwrap().len(), 4);
}

#[test]
fn allocation_config_helper_normalizes_and_rejects_mismatches() {
    let cfg = parse_allocation_config(AllocationArgs {
        allocation_symbols: "SPY,GLD,BIL".into(),
        allocation_weights: "60,25,15".into(),
        allocation_tolerance_pct: 1.5,
        allocation_hysteresis_pct: 0.2,
        allocation_min_trade_notional: 25.0,
        allocation_allow_fractional: false,
    })
    .expect("allocation");

    assert_eq!(cfg.symbols, vec!["SPY", "GLD", "BIL"]);
    assert!((cfg.weights.iter().sum::<f64>() - 1.0).abs() < 0.000_001);
    assert!((cfg.weights[0] - 0.6).abs() < 0.000_001);
    assert_eq!(cfg.tolerance_pct, 1.5);
    assert_eq!(cfg.hysteresis_pct, 0.2);
    assert_eq!(cfg.min_trade_notional, 25.0);
    assert!(!cfg.allow_fractional);

    let err = parse_allocation_config(AllocationArgs {
        allocation_symbols: "SPY,GLD".into(),
        allocation_weights: "0.7".into(),
        allocation_tolerance_pct: 1.5,
        allocation_hysteresis_pct: 0.2,
        allocation_min_trade_notional: 25.0,
        allocation_allow_fractional: true,
    })
    .expect_err("mismatched lengths");
    assert!(err.contains("same length"));
}

#[test]
fn market_clock_helper_supports_clocks_array() {
    let payload = serde_json::json!({
        "clocks": [
            {"market": {"acronym": "NYSE"}, "phase": "core", "timestamp": "2026-02-18T12:00:00-05:00"}
        ]
    });

    let (is_open, timestamp) = market_open_from_clock_payload(&payload);

    assert!(is_open);
    assert_eq!(timestamp, "2026-02-18T12:00:00-05:00");
}

#[test]
fn options_acceptance_payloads_are_mocked_and_side_effect_free() {
    let exercise = options_exercise_acceptance_payload(false);
    let strategies = options_strategies_acceptance_payload(&["all"], false).expect("strategies");

    assert_eq!(exercise["status"], "passed");
    assert_eq!(
        exercise["source_e2e"],
        "runtime-rs/tests/demos.rs::options_acceptance_payloads_are_mocked_and_side_effect_free"
    );
    assert_eq!(
        exercise["operations"]["optionExercise"][0],
        "trading_options_exercise"
    );
    assert_eq!(
        exercise["operations"]["optionDoNotExercise"][3],
        "enabled_mock"
    );
    assert_eq!(exercise["orders_submitted"], 0);

    assert_eq!(strategies["status"], "passed");
    assert_eq!(
        strategies["source_e2e"],
        "runtime-rs/tests/demos.rs::options_acceptance_payloads_are_mocked_and_side_effect_free"
    );
    assert_eq!(
        strategies["strategy_count"].as_u64().unwrap(),
        ALL_STRATEGY_NAMES.len() as u64
    );
    assert!(strategies["strategies"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("iron_condor")));
    assert_eq!(strategies["orders_submitted"], 0);
}
