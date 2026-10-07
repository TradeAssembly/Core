//! Timestamp-batched portfolio simulation for explicit owner-supplied VWAP rules.
//! Orders get one exact next-minute execution opportunity; unfilled remainder expires.
use super::portfolio_capacity::PortfolioCapacity;
use super::session_metrics::{CompletedBar, SessionAccumulator};
use super::timeline::{BarTime, Timeline};
use super::vwap_rules::{PositionFacts, Signal, VwapRules};
use super::*;

#[derive(Clone, Debug)]
pub struct Bar {
    pub time: BarTime,
    pub market: MarketObservation,
    pub provider_vwap_micros: i64,
    /// Liquidity eligibility may suppress fills without changing observed signal volume.
    pub available_volume_micros: i64,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub rules: VwapRules,
    pub volume_window: usize,
    pub entry_notional_micros: i64,
    pub maximum_positions: usize,
    pub maximum_exposure_micros: i64,
}

#[derive(Clone, Copy)]
struct Position {
    quantity: i64,
    facts: PositionFacts,
}

/// Source indices remain stable even when provider rows are grouped by symbol.
pub fn simulate(
    bars: &[Bar],
    policy: &Policy,
    config: SimulationConfig,
) -> Result<KernelResult, KernelError> {
    let invalid = || KernelError::InvalidSimulationConfig;
    policy.rules.validate().map_err(|_| invalid())?;
    let template = SessionAccumulator::new(policy.volume_window).map_err(|_| invalid())?;
    // Reuse the legacy cost/risk validator without enabling a same-bar signal path.
    let validation_strategy = CompiledStrategy {
        entry: SignalExpression {
            field: SignalField::Close,
            comparator: Comparator::Equal,
            threshold_micros: 0,
        },
        exit: None,
        quantity: 1,
    };
    validate_config(&validation_strategy, config)?;
    if config.timing != ExecutionTiming::NextBarOpen
        || policy.entry_notional_micros <= 0
        || config
            .spread_bps
            .checked_add(config.slippage_bps)
            .is_none_or(|bps| bps >= BPS_DENOMINATOR)
    {
        return Err(invalid());
    }
    let mut capacity =
        PortfolioCapacity::new(policy.maximum_positions, policy.maximum_exposure_micros)
            .map_err(|_| invalid())?;
    let mut source_indices = std::collections::BTreeSet::new();
    for bar in bars {
        if !source_indices.insert(bar.market.bar_index)
            || bar.market.symbol != bar.time.symbol
            || bar.market.open <= 0
            || bar.market.close <= 0
            || bar.market.volume < 0
            || bar.provider_vwap_micros <= 0
            || bar.available_volume_micros < 0
            || bar.available_volume_micros > bar.market.volume
        {
            return Err(KernelError::UnsupportedSemantics("invalid portfolio bar"));
        }
    }
    let timeline = Timeline::new(bars.iter().map(|b| b.time.clone()).collect(), 60_000)
        .map_err(|_| KernelError::UnsupportedSemantics("invalid portfolio timeline"))?;
    let mut result = KernelResult {
        decisions: vec![],
        orders: vec![],
        fills: vec![],
        rejects: vec![],
        ending_cash_micros: config.starting_cash_micros,
    };
    let mut positions = BTreeMap::<String, Position>::new();
    let mut metrics = BTreeMap::<String, SessionAccumulator>::new();
    let mut last_entries = BTreeMap::<String, i64>::new();
    let mut pending = BTreeMap::<usize, SimulatedOrder>::new();
    for batch in timeline.batches() {
        // Opening marks precede fills. Never use this minute's close for an opening fill.
        for &index in &batch {
            let bar = &bars[index];
            if let Some(position) = positions.get(&bar.time.symbol) {
                capacity
                    .mark_position(
                        &bar.time.symbol,
                        scaled_notional(position.quantity, bar.market.open)?,
                    )
                    .map_err(|_| invalid())?;
            }
        }
        // Closing orders release exposure/cash before entries at the same event time.
        for side in [OrderSide::Sell, OrderSide::Buy] {
            for &index in &batch {
                if !pending.get(&index).is_some_and(|o| o.intent.side == side) {
                    continue;
                }
                let order = pending.remove(&index).expect("pending order checked");
                let bar = &bars[index];
                let fill = execution_fill(&order, bar, config, result.ending_cash_micros)?;
                match fill {
                    Err(reject) => {
                        if side == OrderSide::Buy {
                            capacity
                                .release_pending(&bar.time.symbol)
                                .map_err(|_| invalid())?;
                        }
                        result.rejects.push(reject);
                    }
                    Ok(fill) => {
                        let gross = scaled_notional(fill.quantity, fill.price_micros)?;
                        if side == OrderSide::Buy {
                            // Marks may have risen since reservation. Fail closed before consuming cash.
                            let mut candidate = capacity.clone();
                            candidate
                                .fill(&bar.time.symbol, gross, true)
                                .map_err(|_| invalid())?;
                            if candidate.committed_exposure_micros()
                                > i128::from(policy.maximum_exposure_micros)
                            {
                                capacity
                                    .release_pending(&bar.time.symbol)
                                    .map_err(|_| invalid())?;
                                result.rejects.push(Reject::InsufficientCapital {
                                    decision_bar_index: order.intent.decision_bar_index,
                                });
                                continue;
                            }
                            capacity = candidate;
                            result.ending_cash_micros = result
                                .ending_cash_micros
                                .checked_sub(gross)
                                .and_then(|v| v.checked_sub(fill.fee_micros))
                                .ok_or(KernelError::ArithmeticOverflow)?;
                            positions.insert(
                                bar.time.symbol.clone(),
                                Position {
                                    quantity: fill.quantity,
                                    facts: PositionFacts {
                                        average_entry_price_micros: fill.price_micros,
                                        first_fill_time_ms: bar.time.start_ms,
                                    },
                                },
                            );
                            last_entries.insert(bar.time.symbol.clone(), bar.time.start_ms);
                        } else {
                            let position =
                                positions.get_mut(&bar.time.symbol).ok_or_else(invalid)?;
                            position.quantity = position
                                .quantity
                                .checked_sub(fill.quantity)
                                .filter(|v| *v >= 0)
                                .ok_or_else(invalid)?;
                            capacity
                                .mark_position(
                                    &bar.time.symbol,
                                    scaled_notional(position.quantity, bar.market.open)?,
                                )
                                .map_err(|_| invalid())?;
                            if position.quantity == 0 {
                                positions.remove(&bar.time.symbol);
                            }
                            result.ending_cash_micros = result
                                .ending_cash_micros
                                .checked_add(gross)
                                .and_then(|v| v.checked_sub(fill.fee_micros))
                                .ok_or(KernelError::ArithmeticOverflow)?;
                        }
                        result.fills.push(fill);
                    }
                }
            }
        }
        // All closing marks precede new reservations: symbol order cannot hide exposure.
        for &index in &batch {
            let bar = &bars[index];
            if let Some(position) = positions.get(&bar.time.symbol) {
                capacity
                    .mark_position(
                        &bar.time.symbol,
                        scaled_notional(position.quantity, bar.market.close)?,
                    )
                    .map_err(|_| invalid())?;
            }
        }
        for index in batch {
            let bar = &bars[index];
            let source_index = bar.market.bar_index;
            let symbol = &bar.time.symbol;
            let close_time = bar
                .time
                .start_ms
                .checked_add(60_000)
                .ok_or(KernelError::ArithmeticOverflow)?;
            let observed = metrics
                .entry(symbol.clone())
                .or_insert_with(|| template.clone())
                .observe(CompletedBar {
                    close_time_ms: close_time,
                    session_start_ms: bar.time.session_start_ms,
                    vwap_basis_micros: bar.provider_vwap_micros,
                    volume_micros: bar.market.volume,
                })
                .map_err(|_| KernelError::UnsupportedSemantics("invalid session metrics"))?;
            let signal = policy
                .rules
                .evaluate(
                    close_time,
                    bar.market.close,
                    bar.market.volume,
                    observed,
                    positions.get(symbol).map(|p| p.facts),
                    last_entries.get(symbol).copied(),
                )
                .map_err(|_| KernelError::UnsupportedSemantics("invalid portfolio signal"))?;
            let (side, quantity, notional_micros) = match signal {
                Signal::Hold => {
                    result.decisions.push(Decision::NoSignal {
                        bar_index: source_index,
                    });
                    continue;
                }
                Signal::Enter => (OrderSide::Buy, 0, Some(policy.entry_notional_micros)),
                Signal::Exit { .. } => (
                    OrderSide::Sell,
                    positions.get(symbol).ok_or_else(invalid)?.quantity,
                    None,
                ),
            };
            let intent = OrderIntent {
                decision_bar_index: source_index,
                symbol: symbol.clone(),
                side,
                quantity,
                notional_micros,
                timing: config.timing,
            };
            result.decisions.push(Decision::OrderIntent(intent.clone()));
            let Some(next) = timeline.next_bar(index).map_err(|_| invalid())? else {
                result.rejects.push(Reject::InsufficientData {
                    decision_bar_index: source_index,
                });
                continue;
            };
            if side == OrderSide::Buy
                && capacity
                    .reserve(symbol, policy.entry_notional_micros)
                    .is_err()
            {
                result.rejects.push(Reject::InsufficientCapital {
                    decision_bar_index: source_index,
                });
                continue;
            }
            let order = SimulatedOrder {
                intent,
                execution_bar_index: bars[next].market.bar_index,
            };
            result.orders.push(order.clone());
            if pending.insert(next, order).is_some() {
                return Err(invalid());
            }
        }
    }
    if !pending.is_empty() {
        return Err(invalid());
    }
    Ok(result)
}

fn execution_fill(
    order: &SimulatedOrder,
    bar: &Bar,
    config: SimulationConfig,
    cash: i64,
) -> Result<Result<Fill, Reject>, KernelError> {
    let index = order.intent.decision_bar_index;
    let bps = config
        .spread_bps
        .checked_add(config.slippage_bps)
        .ok_or(KernelError::ArithmeticOverflow)?;
    let price = match order.intent.side {
        OrderSide::Buy => apply_bps_up(bar.market.open, bps)?,
        OrderSide::Sell => apply_bps_down(bar.market.open, bps)?,
    };
    if price <= 0 {
        return Err(KernelError::InvalidSimulationConfig);
    }
    let requested = match order.intent.notional_micros {
        Some(notional) => {
            i64::try_from(i128::from(notional) * i128::from(QUANTITY_SCALE) / i128::from(price))
                .map_err(|_| KernelError::ArithmeticOverflow)?
        }
        None => order.intent.quantity,
    };
    let available = i64::try_from(
        i128::from(bar.available_volume_micros) * i128::from(config.participation_bps)
            / i128::from(BPS_DENOMINATOR),
    )
    .map_err(|_| KernelError::ArithmeticOverflow)?;
    let quantity = if requested <= available {
        requested
    } else if config.partial_fill_policy == PartialFillPolicy::Allow {
        available
    } else {
        return Ok(Err(Reject::Liquidity {
            decision_bar_index: index,
        }));
    };
    if quantity <= 0 {
        return Ok(Err(Reject::Liquidity {
            decision_bar_index: index,
        }));
    }
    let gross = scaled_notional(quantity, price)?;
    let fee = calculate_fee_micros(quantity, gross, config)?;
    if quantity > config.maximum_order_quantity_micros
        || gross <= 0
        || (order.intent.side == OrderSide::Buy
            && (gross > config.maximum_position_notional_micros
                || gross
                    .checked_add(fee)
                    .ok_or(KernelError::ArithmeticOverflow)?
                    > cash))
    {
        return Ok(Err(Reject::InsufficientCapital {
            decision_bar_index: index,
        }));
    }
    Ok(Ok(Fill {
        order: order.clone(),
        quantity,
        price_micros: price,
        fee_micros: fee,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    const U: i64 = 1_000_000;

    fn policy() -> Policy {
        Policy {
            rules: VwapRules {
                entry_discount_bps: 50,
                minimum_volume_ratio_bps: 12_000,
                exit_proximity_bps: 10,
                maximum_holding_ms: 900_000,
                maximum_loss_bps: 50,
                entry_cooldown_ms: 600_000,
            },
            volume_window: 1,
            entry_notional_micros: 100 * U,
            maximum_positions: 1,
            maximum_exposure_micros: 500 * U,
        }
    }
    fn config() -> SimulationConfig {
        SimulationConfig {
            timing: ExecutionTiming::NextBarOpen,
            fee_per_order_micros: 0,
            fee_per_unit_micros: 0,
            notional_fee_bps: 0,
            spread_bps: 0,
            slippage_bps: 0,
            participation_bps: 10_000,
            partial_fill_policy: PartialFillPolicy::Reject,
            starting_cash_micros: 1_000 * U,
            maximum_order_quantity_micros: 100 * U,
            maximum_position_notional_micros: 500 * U,
        }
    }
    fn bars() -> Vec<Bar> {
        let mut bars = vec![];
        // Intentionally provider-grouped and reverse alphabetical input.
        for symbol in ["B", "A"] {
            for (minute, (open, close, volume)) in [
                (100, 100, 100),
                (100, 99, 200),
                (98, 100, 100),
                (101, 100, 100),
            ]
            .into_iter()
            .enumerate()
            {
                bars.push(Bar {
                    time: BarTime {
                        symbol: symbol.into(),
                        start_ms: minute as i64 * 60_000,
                        session_start_ms: 0,
                        session_end_ms: 240_000,
                    },
                    market: MarketObservation {
                        bar_index: bars.len(),
                        symbol: symbol.into(),
                        open: open * U,
                        close: close * U,
                        volume: volume * U,
                    },
                    provider_vwap_micros: 100 * U,
                    available_volume_micros: volume * U,
                });
            }
        }
        bars
    }

    #[test]
    fn grouped_symbols_share_capacity_and_fill_only_at_next_open() {
        let bars = bars();
        let result = simulate(&bars, &policy(), config()).unwrap();
        assert_eq!(result.fills.len(), 2);
        let buy = &result.fills[0];
        assert_eq!(buy.order.intent.symbol, "A");
        assert_eq!(buy.order.intent.decision_bar_index, 5);
        assert_eq!(buy.order.execution_bar_index, 6);
        assert_eq!(buy.order.intent.notional_micros, Some(100 * U));
        assert_eq!(buy.price_micros, 98 * U);
        assert_eq!(buy.quantity, 100 * U / 98);
        assert_eq!(result.fills[1].order.intent.side, OrderSide::Sell);
        assert_eq!(result.fills[1].order.execution_bar_index, 7);
        assert_eq!(result.fills[1].price_micros, 101 * U);
        assert!(result.ending_cash_micros > config().starting_cash_micros);
        assert_eq!(simulate(&bars, &policy(), config()).unwrap(), result);
    }

    #[test]
    fn execution_minute_close_cannot_change_earlier_entry_or_open_fill() {
        let baseline = simulate(&bars(), &policy(), config()).unwrap();
        let mut changed = bars();
        changed[6].market.close = 1_000 * U;
        let changed = simulate(&changed, &policy(), config()).unwrap();
        assert_eq!(baseline.fills[0], changed.fills[0]);
        assert_eq!(&baseline.decisions[..4], &changed.decisions[..4]);
    }

    #[test]
    fn missing_next_minute_does_not_fill_later_or_reserve_a_slot() {
        let mut bars = bars();
        bars.retain(|b| b.time.start_ms != 120_000);
        for (index, bar) in bars.iter_mut().enumerate() {
            bar.market.bar_index = index;
        }
        let result = simulate(&bars, &policy(), config()).unwrap();
        assert!(result.fills.is_empty());
        assert!(result.orders.is_empty());
        assert_eq!(
            result
                .rejects
                .iter()
                .filter(|r| matches!(r, Reject::InsufficientData { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn partial_fill_expires_remainder_and_exit_sells_only_filled_quantity() {
        let mut bars = bars();
        bars[6].market.volume = U / 2;
        bars[6].available_volume_micros = U / 2;
        let mut config = config();
        config.partial_fill_policy = PartialFillPolicy::Allow;
        let result = simulate(&bars, &policy(), config).unwrap();
        assert_eq!(result.fills.len(), 2);
        assert_eq!(result.fills[0].quantity, U / 2);
        assert_eq!(result.fills[1].quantity, U / 2);
    }

    #[test]
    fn fees_and_shared_cash_can_reject_an_otherwise_valid_entry() {
        let mut config = config();
        config.starting_cash_micros = 100 * U;
        config.fee_per_order_micros = U;
        let result = simulate(&bars(), &policy(), config).unwrap();
        assert!(result.fills.is_empty());
        assert_eq!(result.ending_cash_micros, 100 * U);
        assert!(result.rejects.iter().any(|r| matches!(
            r,
            Reject::InsufficientCapital {
                decision_bar_index: 5
            }
        )));
    }

    #[test]
    fn same_bar_execution_and_duplicate_observations_fail_closed() {
        let mut config = config();
        config.timing = ExecutionTiming::SameBarClose;
        assert!(simulate(&bars(), &policy(), config).is_err());
        let mut bars = bars();
        bars[1].time.start_ms = 0;
        assert!(simulate(&bars, &policy(), super::tests::config()).is_err());
    }
}
