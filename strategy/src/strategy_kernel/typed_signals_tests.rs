use super::*;
fn config() -> SimulationConfig {
    SimulationConfig {
        timing: ExecutionTiming::NextBarOpen,
        fee_per_order_micros: 10000,
        fee_per_unit_micros: 0,
        notional_fee_bps: 0,
        spread_bps: 10,
        slippage_bps: 20,
        participation_bps: 10000,
        partial_fill_policy: PartialFillPolicy::Reject,
        starting_cash_micros: 1000000000,
        maximum_order_quantity_micros: 10000000,
        maximum_position_notional_micros: 1000000000,
    }
}
fn bars() -> Vec<MarketObservation> {
    [100, 110, 90, 95]
        .into_iter()
        .enumerate()
        .map(|(bar_index, price)| MarketObservation {
            bar_index,
            symbol: "SYNTHETIC".into(),
            open: price * 1000000,
            close: price * 1000000,
            volume: 100000000,
        })
        .collect()
}
#[test]
fn typed_signals_reuse_real_next_bar_cost_cash_and_partial_fill_rules() {
    let signals = [
        BooleanSignal {
            entry: Some(true),
            exit: Some(false),
        },
        BooleanSignal {
            entry: Some(false),
            exit: Some(false),
        },
        BooleanSignal {
            entry: Some(false),
            exit: Some(true),
        },
        BooleanSignal {
            entry: Some(false),
            exit: Some(false),
        },
    ];
    let result = run_with_signals(&bars(), &signals, 1000000, config()).unwrap();
    assert_eq!(result.fills.len(), 2);
    assert_eq!(result.fills[0].order.execution_bar_index, 1);
    assert_eq!(result.fills[0].price_micros, 110330000);
    assert_eq!(result.fills[1].order.execution_bar_index, 3);
    assert_eq!(result.fills[1].price_micros, 94715000);
    assert_eq!(result.ending_cash_micros, 984365000);
    let mut illiquid = bars();
    illiquid[1].volume = 0;
    let refused = run_with_signals(&illiquid, &signals, 1000000, config()).unwrap();
    assert!(refused.fills.is_empty());
    assert!(matches!(refused.rejects[0], Reject::Liquidity { .. }));
}
#[test]
fn typed_unavailable_is_preserved_and_invalid_grid_cannot_panic_or_trade() {
    let signals = [BooleanSignal {
        entry: None,
        exit: None,
    }; 4];
    let result = run_with_signals(&bars(), &signals, 1000000, config()).unwrap();
    assert!(result.fills.is_empty());
    assert_eq!(result.decisions.len(), 4);
    let partial = [BooleanSignal {
        entry: Some(true),
        exit: None,
    }; 4];
    assert!(run_with_signals(&bars(), &partial, 1000000, config())
        .unwrap()
        .fills
        .is_empty());
    let mut invalid = bars();
    invalid[2].bar_index = 999;
    assert!(run_with_signals(&invalid, &signals, 1000000, config()).is_err());
    assert!(run_with_signals(&bars(), &signals[..3], 1000000, config()).is_err());
    let mut same_bar = config();
    same_bar.timing = ExecutionTiming::SameBarOpen;
    assert!(run_with_signals(&bars(), &signals, 1000000, same_bar).is_err());
}
