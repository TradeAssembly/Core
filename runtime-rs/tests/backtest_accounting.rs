use std::collections::BTreeMap;
use tradeassembly_runtime::backtest_accounting::*;

const UNIT: i64 = 1_000_000;

fn fill(id: &str, side: FillSide, quantity: i64, price: i64, fee: i64) -> AccountingFill {
    AccountingFill {
        fill_id: id.to_string(),
        symbol: "SPY".to_string(),
        side,
        quantity: quantity * UNIT,
        price_micros: price * UNIT,
        fee_micros: fee * UNIT,
    }
}

fn input(
    fills: Vec<AccountingFill>,
    mark: Option<i64>,
    end_of_data: EndOfDataPolicy,
) -> AccountingInput {
    AccountingInput {
        starting_cash_micros: 10_000 * UNIT,
        fills,
        cash_entries: vec![],
        marks_micros: mark
            .map(|value| BTreeMap::from([("SPY".to_string(), value * UNIT)]))
            .unwrap_or_default(),
        end_of_data,
        reconciliation_tolerance_micros: 0,
    }
}

#[test]
fn closed_profitable_loss_and_flat_wash_reconcile() {
    for (name, sell_price, expected) in [("profit", 120, 20), ("loss", 80, -20), ("wash", 100, 0)] {
        let result = account(&input(
            vec![
                fill(&format!("{name}-buy"), FillSide::Buy, 1, 100, 0),
                fill(&format!("{name}-sell"), FillSide::Sell, 1, sell_price, 0),
            ],
            None,
            EndOfDataPolicy::MarkToMarket,
        ))
        .unwrap();
        assert_eq!(result.realized_pnl_micros, expected * UNIT);
        assert_eq!(result.ending_equity_micros, (10_000 + expected) * UNIT);
        assert!(result.positions.is_empty());
        assert_eq!(result.reconciliation_residual_micros, 0);
    }
}

#[test]
fn open_position_is_marked_to_market() {
    let result = account(&input(
        vec![fill("buy", FillSide::Buy, 2, 100, 0)],
        Some(125),
        EndOfDataPolicy::MarkToMarket,
    ))
    .unwrap();
    let position = result.positions.get("SPY").unwrap();
    assert_eq!(position.quantity, 2 * UNIT);
    assert_eq!(position.market_value_micros, 250 * UNIT);
    assert_eq!(result.unrealized_pnl_micros, 50 * UNIT);
    assert_eq!(result.ending_equity_micros, 10_050 * UNIT);
}

#[test]
fn force_close_realizes_open_position_at_mark() {
    let result = account(&input(
        vec![fill("buy", FillSide::Buy, 2, 100, 0)],
        Some(125),
        EndOfDataPolicy::ForceClose,
    ))
    .unwrap();
    assert_eq!(result.fills.last().unwrap().fill_id, "force_close:SPY");
    assert!(result.positions.is_empty());
    assert_eq!(result.realized_pnl_micros, 50 * UNIT);
    assert_eq!(result.unrealized_pnl_micros, 0);
    assert_eq!(result.ending_equity_micros, 10_050 * UNIT);
}

#[test]
fn fill_fees_are_costs_and_already_reflected_in_fill_economics() {
    let result = account(&input(
        vec![
            fill("buy", FillSide::Buy, 1, 101, 2),
            fill("sell", FillSide::Sell, 1, 110, 3),
        ],
        None,
        EndOfDataPolicy::MarkToMarket,
    ))
    .unwrap();
    assert_eq!(result.realized_pnl_micros, 9 * UNIT);
    assert_eq!(result.costs_micros, 5 * UNIT);
    assert_eq!(result.ending_equity_micros, 10_004 * UNIT);
}

#[test]
fn multiple_fills_keep_fifo_lots_and_weighted_position_value() {
    let result = account(&input(
        vec![
            fill("one", FillSide::Buy, 2, 100, 0),
            fill("two", FillSide::Buy, 1, 130, 0),
            fill("three", FillSide::Sell, 2, 120, 0),
        ],
        Some(140),
        EndOfDataPolicy::MarkToMarket,
    ))
    .unwrap();
    let position = result.positions.get("SPY").unwrap();
    assert_eq!(result.realized_pnl_micros, 40 * UNIT);
    assert_eq!(position.average_cost_micros, 130 * UNIT);
    assert_eq!(
        position.lots,
        vec![PositionLot {
            lot_id: "two".to_string(),
            symbol: "SPY".to_string(),
            quantity: UNIT,
            unit_cost_micros: 130 * UNIT
        }]
    );
    assert_eq!(result.unrealized_pnl_micros, 10 * UNIT);
}

#[test]
fn missing_mark_and_overflow_fail_closed() {
    assert_eq!(
        account(&input(
            vec![fill("buy", FillSide::Buy, 1, 100, 0)],
            None,
            EndOfDataPolicy::MarkToMarket
        )),
        Err(AccountingError::MissingMark {
            symbol: "SPY".to_string()
        })
    );
    let mut overflowing = input(
        vec![AccountingFill {
            fill_id: "overflow".to_string(),
            symbol: "SPY".to_string(),
            side: FillSide::Buy,
            quantity: i64::MAX,
            price_micros: 2,
            fee_micros: 0,
        }],
        Some(1),
        EndOfDataPolicy::MarkToMarket,
    );
    overflowing.starting_cash_micros = i64::MAX;
    assert_eq!(
        account(&overflowing),
        Err(AccountingError::ArithmeticOverflow)
    );
}

#[test]
fn exact_equation_includes_income_and_external_costs() {
    let mut request = input(
        vec![fill("buy", FillSide::Buy, 1, 100, 2)],
        Some(110),
        EndOfDataPolicy::MarkToMarket,
    );
    request.cash_entries = vec![
        CashEntry {
            entry_id: "dividend".to_string(),
            kind: CashEntryKind::Income,
            amount_micros: 4 * UNIT,
        },
        CashEntry {
            entry_id: "data".to_string(),
            kind: CashEntryKind::Cost,
            amount_micros: UNIT,
        },
    ];
    let result = account(&request).unwrap();
    assert_eq!(result.ending_equity_micros, 10_011 * UNIT);
    assert_eq!(
        result.ending_equity_micros,
        request.starting_cash_micros
            + result.realized_pnl_micros
            + result.unrealized_pnl_micros
            + result.income_micros
            - result.costs_micros
    );
    assert_eq!(result.reconciliation_residual_micros, 0);
}
