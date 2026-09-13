use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tradeassembly_runtime::strategy_kernel::*;

const UNIT: i64 = 1_000_000;

fn strategy(expression: &str, quantity: i64) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3/valid/static-equity.json");
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    value["stages"]["evaluate"]["substeps"][0]["config"] = json!({
        "tradeassembly.expression": expression,
        "tradeassembly.quantity": quantity,
    });
    value
}

fn strategy_with_exit(entry: &str, exit: &str, quantity: i64) -> Value {
    let mut value = strategy(entry, quantity);
    value["stages"]["exit_policy"]["substeps"][0]["config"]["tradeassembly.expression"] =
        json!(exit);
    value
}

fn config(timing: ExecutionTiming) -> SimulationConfig {
    SimulationConfig {
        timing,
        fee_per_order_micros: 0,
        fee_per_unit_micros: 0,
        notional_fee_bps: 0,
        spread_bps: 0,
        slippage_bps: 0,
        participation_bps: 10_000,
        partial_fill_policy: PartialFillPolicy::Reject,
        starting_cash_micros: 10_000_000_000,
        maximum_order_quantity_micros: 100 * UNIT,
        maximum_position_notional_micros: 10_000_000_000,
    }
}

fn bars() -> Vec<MarketObservation> {
    vec![
        MarketObservation {
            bar_index: 0,
            symbol: "SPY".into(),
            open: 100_000_000,
            close: 101_000_000,
            volume: 10 * UNIT,
        },
        MarketObservation {
            bar_index: 1,
            symbol: "SPY".into(),
            open: 102_000_000,
            close: 103_000_000,
            volume: 10 * UNIT,
        },
    ]
}

#[test]
fn bundled_crypto_demo_compiles_and_generates_closed_trade_evidence() {
    let compiled =
        CompiledStrategy::compile_json(&tradeassembly_runtime::spec::btc_exit_demo_spec_payload())
            .unwrap();
    let observations = vec![
        MarketObservation {
            bar_index: 0,
            symbol: "BTC/USD".into(),
            open: 100_000_000,
            close: 100_000_000,
            volume: 10 * UNIT,
        },
        MarketObservation {
            bar_index: 1,
            symbol: "BTC/USD".into(),
            open: 101_000_000,
            close: 101_000_000,
            volume: 10 * UNIT,
        },
        MarketObservation {
            bar_index: 2,
            symbol: "BTC/USD".into(),
            open: 98_000_000,
            close: 98_000_000,
            volume: 10 * UNIT,
        },
    ];
    let result = run(
        KernelMode::Backtest,
        &compiled,
        &observations,
        config(ExecutionTiming::SameBarClose),
    )
    .unwrap();
    assert_eq!(result.fills.len(), 2);
    assert_eq!(result.fills[0].order.intent.side, OrderSide::Buy);
    assert_eq!(result.fills[1].order.intent.side, OrderSide::Sell);
}

#[test]
fn strategy_expression_controls_decisions_and_no_signal_submits_nothing() {
    let compiled =
        CompiledStrategy::compile_json(&strategy("bar.close > 102000000", 2 * UNIT)).unwrap();
    let result = run(
        KernelMode::Backtest,
        &compiled,
        &bars(),
        config(ExecutionTiming::SameBarClose),
    )
    .unwrap();
    assert_eq!(result.decisions.len(), 2);
    assert_eq!(result.orders.len(), 1);
    assert_eq!(result.fills.len(), 1);
    assert_eq!(result.fills[0].order.intent.decision_bar_index, 1);

    let quiet =
        CompiledStrategy::compile_json(&strategy("bar.close > 200000000", 2 * UNIT)).unwrap();
    let result = run(
        KernelMode::Backtest,
        &quiet,
        &bars(),
        config(ExecutionTiming::SameBarClose),
    )
    .unwrap();
    assert!(result.orders.is_empty());
    assert!(result.fills.is_empty());
}

#[test]
fn next_bar_execution_does_not_use_signal_bar_open() {
    let compiled =
        CompiledStrategy::compile_json(&strategy("bar.close > 100000000", UNIT)).unwrap();
    let result = run(
        KernelMode::Backtest,
        &compiled,
        &bars(),
        config(ExecutionTiming::NextBarOpen),
    )
    .unwrap();
    assert_eq!(result.fills.len(), 1);
    assert_eq!(result.fills[0].order.intent.decision_bar_index, 0);
    assert_eq!(result.fills[0].order.execution_bar_index, 1);
    assert_eq!(result.fills[0].price_micros, 102_000_000);
    assert_eq!(result.rejects.len(), 0);
    assert!(matches!(
        run(
            KernelMode::Backtest,
            &compiled,
            &bars(),
            config(ExecutionTiming::SameBarOpen),
        ),
        Err(KernelError::UnsupportedSemantics(_))
    ));

    let open_signal =
        CompiledStrategy::compile_json(&strategy("bar.open > 99000000", UNIT)).unwrap();
    let same_open = run(
        KernelMode::Backtest,
        &open_signal,
        &bars(),
        config(ExecutionTiming::SameBarOpen),
    )
    .unwrap();
    assert_eq!(same_open.fills[0].price_micros, 100_000_000);
}

#[test]
fn fees_spread_slippage_and_liquidity_are_derived_deterministically() {
    let compiled =
        CompiledStrategy::compile_json(&strategy("bar.close > 100000000", 8 * UNIT)).unwrap();
    let mut simulation = config(ExecutionTiming::SameBarClose);
    simulation.fee_per_order_micros = 1_000;
    simulation.spread_bps = 10;
    simulation.slippage_bps = 10;
    simulation.participation_bps = 5_000;
    simulation.partial_fill_policy = PartialFillPolicy::Allow;
    let result = run(KernelMode::Backtest, &compiled, &bars()[..1], simulation).unwrap();
    assert_eq!(result.fills[0].quantity, 5 * UNIT);
    assert_eq!(result.fills[0].price_micros, 101_202_000);
    assert_eq!(result.fills[0].fee_micros, 1_000);
    assert_eq!(result.ending_cash_micros, 9_493_989_000);
}

#[test]
fn unsupported_semantics_fail_closed_and_paper_matches_backtest() {
    let mut unsupported = strategy("bar.close > 100000000", UNIT);
    unsupported["stages"]["evaluate"]["substeps"][0]["operation_ref"] = json!("signal.magic@1");
    assert!(matches!(
        CompiledStrategy::compile_json(&unsupported),
        Err(KernelError::UnsupportedSemantics(_))
    ));

    let compiled =
        CompiledStrategy::compile_json(&strategy("bar.close > 100000000", UNIT)).unwrap();
    let backtest = run(
        KernelMode::Backtest,
        &compiled,
        &bars(),
        config(ExecutionTiming::NextBarClose),
    )
    .unwrap();
    let paper = run(
        KernelMode::Paper,
        &compiled,
        &bars(),
        config(ExecutionTiming::NextBarClose),
    )
    .unwrap();
    assert_eq!(backtest, paper);
}

#[test]
fn position_state_prevents_duplicate_entries_and_exit_realizes_cash_with_all_costs() {
    let mut observations = bars();
    observations.push(MarketObservation {
        bar_index: 2,
        symbol: "SPY".into(),
        open: 104_000_000,
        close: 99_000_000,
        volume: 10 * UNIT,
    });
    let compiled = CompiledStrategy::compile_json(&strategy_with_exit(
        "bar.close > 100000000",
        "bar.close < 100000000",
        UNIT,
    ))
    .unwrap();
    let mut simulation = config(ExecutionTiming::SameBarClose);
    simulation.fee_per_order_micros = 100;
    simulation.fee_per_unit_micros = 200;
    simulation.notional_fee_bps = 10;
    let result = run(KernelMode::Backtest, &compiled, &observations, simulation).unwrap();
    assert_eq!(result.fills.len(), 2);
    assert_eq!(result.fills[0].order.intent.side, OrderSide::Buy);
    assert_eq!(result.fills[1].order.intent.side, OrderSide::Sell);
    assert_eq!(result.fills[0].fee_micros, 101_300);
    assert_eq!(result.fills[1].fee_micros, 99_300);
    assert_eq!(result.ending_cash_micros, 9_997_799_400);
}
