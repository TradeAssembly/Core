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

#[test]
fn typed_research_150_observation_warmup_has_no_early_intent_or_next_bar_fill() {
    let observations = (0..152)
        .map(|bar_index| MarketObservation {
            bar_index,
            symbol: "SYNTHETIC".into(),
            open: 100_000_000,
            close: 100_000_000,
            volume: 100_000_000,
        })
        .collect::<Vec<_>>();
    let signals = vec![
        BooleanSignal {
            entry: Some(true),
            exit: Some(false)
        };
        152
    ];
    let original_signals = signals.clone();
    let old = run_with_signals(&observations, &signals, 1_000_000, config()).unwrap();
    assert_eq!(old.fills[0].order.execution_bar_index, 1);
    let warm =
        run_with_signals_after_warmup(&observations, &signals, 1_000_000, config(), 150).unwrap();
    assert_eq!(
        signals, original_signals,
        "publisher evidence must remain unchanged"
    );
    assert_eq!(
        warm.decisions.len(),
        152,
        "all observations remain represented"
    );
    assert!(warm.decisions[..150]
        .iter()
        .all(|decision| matches!(decision, Decision::NoSignal { .. })));
    assert_eq!(warm.orders.len(), 1);
    assert_eq!(warm.orders[0].intent.decision_bar_index, 150);
    assert_eq!(warm.orders[0].execution_bar_index, 151);
    assert_eq!(warm.fills.len(), 1);
    assert_eq!(warm.fills[0].order.execution_bar_index, 151);
    assert_eq!(warm.fills[0].quantity, 1_000_000);
    assert_eq!(warm.fills[0].fee_micros, old.fills[0].fee_micros);
    assert_eq!(warm.fills[0].price_micros, old.fills[0].price_micros);
    let zero =
        run_with_signals_after_warmup(&observations, &signals, 1_000_000, config(), 0).unwrap();
    assert_eq!(zero.decisions, old.decisions);
    assert_eq!(zero.fills, old.fills);
    assert_eq!(zero.ending_cash_micros, old.ending_cash_micros);
    assert!(
        run_with_signals_after_warmup(&observations, &signals, 1_000_000, config(), 152).is_err()
    );
    assert!(
        run_with_signals_after_warmup(&observations, &signals, 1_000_000, config(), 256).is_err()
    );
}
#[test]
fn typed_research_warmup_does_not_convert_unavailable_signal_to_false_or_an_order() {
    let mut signals = [BooleanSignal {
        entry: Some(true),
        exit: Some(false),
    }; 4];
    signals[2] = BooleanSignal {
        entry: None,
        exit: Some(false),
    };
    signals[3] = BooleanSignal {
        entry: Some(false),
        exit: Some(false),
    };
    let result = run_with_signals_after_warmup(&bars(), &signals, 1_000_000, config(), 2).unwrap();
    assert_eq!(signals[2].entry, None);
    assert!(result.orders.is_empty());
    assert!(result.fills.is_empty());
    assert_eq!(result.decisions.len(), 4);
}
