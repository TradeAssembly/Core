//! Deterministic cash-account accounting derived solely from fills and marks.

use crate::strategy_kernel;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

pub type MoneyMicros = i64;
pub type PriceMicros = i64;
pub type Quantity = i64;
const QUANTITY_SCALE: i64 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillSide {
    Buy,
    Sell,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountingFill {
    pub fill_id: String,
    pub symbol: String,
    pub side: FillSide,
    pub quantity: Quantity,
    pub price_micros: PriceMicros,
    pub fee_micros: MoneyMicros,
}

impl AccountingFill {
    pub fn from_kernel_fill(fill: &strategy_kernel::Fill) -> Self {
        Self {
            fill_id: format!(
                "kernel:{}:{}",
                fill.order.intent.decision_bar_index, fill.order.execution_bar_index
            ),
            symbol: fill.order.intent.symbol.clone(),
            side: match fill.order.intent.side {
                strategy_kernel::OrderSide::Buy => FillSide::Buy,
                strategy_kernel::OrderSide::Sell => FillSide::Sell,
            },
            quantity: fill.quantity,
            price_micros: fill.price_micros,
            fee_micros: fill.fee_micros,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CashEntryKind {
    Income,
    Cost,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CashEntry {
    pub entry_id: String,
    pub kind: CashEntryKind,
    pub amount_micros: MoneyMicros,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndOfDataPolicy {
    MarkToMarket,
    ForceClose,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountingInput {
    pub starting_cash_micros: MoneyMicros,
    pub fills: Vec<AccountingFill>,
    #[serde(default)]
    pub cash_entries: Vec<CashEntry>,
    pub marks_micros: BTreeMap<String, PriceMicros>,
    pub end_of_data: EndOfDataPolicy,
    pub reconciliation_tolerance_micros: MoneyMicros,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerEntryKind {
    FillNotional,
    FillFee,
    Income,
    Cost,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerEntry {
    pub sequence: u64,
    pub entry_id: String,
    pub kind: LedgerEntryKind,
    pub cash_delta_micros: MoneyMicros,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PositionLot {
    pub lot_id: String,
    pub symbol: String,
    pub quantity: Quantity,
    pub unit_cost_micros: PriceMicros,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Position {
    pub symbol: String,
    pub quantity: Quantity,
    pub average_cost_micros: PriceMicros,
    pub market_price_micros: PriceMicros,
    pub market_value_micros: MoneyMicros,
    pub unrealized_pnl_micros: MoneyMicros,
    pub lots: Vec<PositionLot>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountingResult {
    pub fills: Vec<AccountingFill>,
    pub ledger: Vec<LedgerEntry>,
    pub positions: BTreeMap<String, Position>,
    pub ending_cash_micros: MoneyMicros,
    pub market_value_micros: MoneyMicros,
    pub ending_equity_micros: MoneyMicros,
    pub realized_pnl_micros: MoneyMicros,
    pub unrealized_pnl_micros: MoneyMicros,
    pub income_micros: MoneyMicros,
    pub costs_micros: MoneyMicros,
    pub reconciliation_residual_micros: MoneyMicros,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccountingError {
    ArithmeticOverflow,
    InvalidInput(&'static str),
    MissingMark { symbol: String },
    UnsupportedMargin,
    UnsupportedShort { symbol: String },
    ReconciliationFailed { residual_micros: MoneyMicros },
}

pub fn account(input: &AccountingInput) -> Result<AccountingResult, AccountingError> {
    if input.starting_cash_micros < 0 {
        return Err(AccountingError::UnsupportedMargin);
    }
    if input.reconciliation_tolerance_micros < 0 {
        return Err(AccountingError::InvalidInput(
            "reconciliation tolerance must not be negative",
        ));
    }

    let mut state = State {
        cash_micros: input.starting_cash_micros,
        lots: BTreeMap::new(),
        fills: Vec::new(),
        ledger: Vec::new(),
        realized_pnl_micros: 0,
        income_micros: 0,
        costs_micros: 0,
    };
    for fill in &input.fills {
        state.apply_fill(fill.clone())?;
    }
    for entry in &input.cash_entries {
        state.apply_cash_entry(entry)?;
    }

    if input.end_of_data == EndOfDataPolicy::ForceClose {
        let open_symbols: Vec<String> = state.lots.keys().cloned().collect();
        for symbol in open_symbols {
            let quantity = state.position_quantity(&symbol)?;
            if quantity == 0 {
                continue;
            }
            let price_micros = required_mark(&input.marks_micros, &symbol)?;
            state.apply_fill(AccountingFill {
                fill_id: format!("force_close:{symbol}"),
                symbol,
                side: FillSide::Sell,
                quantity,
                price_micros,
                fee_micros: 0,
            })?;
        }
    }

    let mut positions = BTreeMap::new();
    let mut market_value_micros = 0;
    let mut unrealized_pnl_micros = 0;
    for (symbol, lots) in &state.lots {
        if lots.is_empty() {
            continue;
        }
        let mark = required_mark(&input.marks_micros, symbol)?;
        let quantity = lots
            .iter()
            .try_fold(0_i64, |total, lot| checked_add(total, lot.quantity))?;
        let cost_basis = lots.iter().try_fold(0_i64, |total, lot| {
            checked_add(total, notional(lot.quantity, lot.unit_cost_micros)?)
        })?;
        let market_value = notional(quantity, mark)?;
        let unrealized = checked_sub(market_value, cost_basis)?;
        market_value_micros = checked_add(market_value_micros, market_value)?;
        unrealized_pnl_micros = checked_add(unrealized_pnl_micros, unrealized)?;
        positions.insert(
            symbol.clone(),
            Position {
                symbol: symbol.clone(),
                quantity,
                average_cost_micros: cost_basis
                    .checked_mul(QUANTITY_SCALE)
                    .ok_or(AccountingError::ArithmeticOverflow)?
                    .checked_div(quantity)
                    .ok_or(AccountingError::ArithmeticOverflow)?,
                market_price_micros: mark,
                market_value_micros: market_value,
                unrealized_pnl_micros: unrealized,
                lots: lots.iter().cloned().collect(),
            },
        );
    }

    let ending_equity_micros = checked_add(state.cash_micros, market_value_micros)?;
    let expected_equity_micros = checked_sub(
        checked_add(
            checked_add(input.starting_cash_micros, state.realized_pnl_micros)?,
            unrealized_pnl_micros,
        )?,
        state.costs_micros,
    )?;
    let expected_equity_micros = checked_add(expected_equity_micros, state.income_micros)?;
    let residual = checked_sub(ending_equity_micros, expected_equity_micros)?;
    if residual.unsigned_abs() > input.reconciliation_tolerance_micros as u64 {
        return Err(AccountingError::ReconciliationFailed {
            residual_micros: residual,
        });
    }

    Ok(AccountingResult {
        fills: state.fills,
        ledger: state.ledger,
        positions,
        ending_cash_micros: state.cash_micros,
        market_value_micros,
        ending_equity_micros,
        realized_pnl_micros: state.realized_pnl_micros,
        unrealized_pnl_micros,
        income_micros: state.income_micros,
        costs_micros: state.costs_micros,
        reconciliation_residual_micros: residual,
    })
}

struct State {
    cash_micros: MoneyMicros,
    lots: BTreeMap<String, VecDeque<PositionLot>>,
    fills: Vec<AccountingFill>,
    ledger: Vec<LedgerEntry>,
    realized_pnl_micros: MoneyMicros,
    income_micros: MoneyMicros,
    costs_micros: MoneyMicros,
}

impl State {
    fn apply_fill(&mut self, fill: AccountingFill) -> Result<(), AccountingError> {
        validate_fill(&fill)?;
        let gross = notional(fill.quantity, fill.price_micros)?;
        let cash_delta = match fill.side {
            FillSide::Buy => checked_neg(gross)?,
            FillSide::Sell => gross,
        };
        self.apply_ledger(
            format!("{}:notional", fill.fill_id),
            LedgerEntryKind::FillNotional,
            cash_delta,
        )?;
        self.apply_ledger(
            format!("{}:fee", fill.fill_id),
            LedgerEntryKind::FillFee,
            checked_neg(fill.fee_micros)?,
        )?;
        self.costs_micros = checked_add(self.costs_micros, fill.fee_micros)?;
        match fill.side {
            FillSide::Buy => {
                self.lots
                    .entry(fill.symbol.clone())
                    .or_default()
                    .push_back(PositionLot {
                        lot_id: fill.fill_id.clone(),
                        symbol: fill.symbol.clone(),
                        quantity: fill.quantity,
                        unit_cost_micros: fill.price_micros,
                    })
            }
            FillSide::Sell => self.close_lots(&fill)?,
        }
        self.fills.push(fill);
        Ok(())
    }

    fn close_lots(&mut self, fill: &AccountingFill) -> Result<(), AccountingError> {
        let lots =
            self.lots
                .get_mut(&fill.symbol)
                .ok_or_else(|| AccountingError::UnsupportedShort {
                    symbol: fill.symbol.clone(),
                })?;
        let mut remaining = fill.quantity;
        while remaining > 0 {
            let mut lot = lots
                .pop_front()
                .ok_or_else(|| AccountingError::UnsupportedShort {
                    symbol: fill.symbol.clone(),
                })?;
            let closed = remaining.min(lot.quantity);
            let sale = notional(closed, fill.price_micros)?;
            let basis = notional(closed, lot.unit_cost_micros)?;
            self.realized_pnl_micros =
                checked_add(self.realized_pnl_micros, checked_sub(sale, basis)?)?;
            remaining = checked_sub(remaining, closed)?;
            lot.quantity = checked_sub(lot.quantity, closed)?;
            if lot.quantity > 0 {
                lots.push_front(lot);
            }
        }
        Ok(())
    }

    fn apply_cash_entry(&mut self, entry: &CashEntry) -> Result<(), AccountingError> {
        if entry.entry_id.is_empty() || entry.amount_micros < 0 {
            return Err(AccountingError::InvalidInput("cash entry is invalid"));
        }
        let (kind, delta) = match entry.kind {
            CashEntryKind::Income => {
                self.income_micros = checked_add(self.income_micros, entry.amount_micros)?;
                (LedgerEntryKind::Income, entry.amount_micros)
            }
            CashEntryKind::Cost => {
                self.costs_micros = checked_add(self.costs_micros, entry.amount_micros)?;
                (LedgerEntryKind::Cost, checked_neg(entry.amount_micros)?)
            }
        };
        self.apply_ledger(entry.entry_id.clone(), kind, delta)
    }

    fn apply_ledger(
        &mut self,
        entry_id: String,
        kind: LedgerEntryKind,
        cash_delta_micros: MoneyMicros,
    ) -> Result<(), AccountingError> {
        self.cash_micros = checked_add(self.cash_micros, cash_delta_micros)?;
        if self.cash_micros < 0 {
            return Err(AccountingError::UnsupportedMargin);
        }
        self.ledger.push(LedgerEntry {
            sequence: u64::try_from(self.ledger.len())
                .map_err(|_| AccountingError::ArithmeticOverflow)?,
            entry_id,
            kind,
            cash_delta_micros,
        });
        Ok(())
    }

    fn position_quantity(&self, symbol: &str) -> Result<Quantity, AccountingError> {
        self.lots
            .get(symbol)
            .map(|lots| {
                lots.iter()
                    .try_fold(0_i64, |total, lot| checked_add(total, lot.quantity))
            })
            .transpose()?
            .ok_or_else(|| AccountingError::UnsupportedShort {
                symbol: symbol.to_string(),
            })
    }
}

fn validate_fill(fill: &AccountingFill) -> Result<(), AccountingError> {
    if fill.fill_id.is_empty()
        || fill.symbol.is_empty()
        || fill.quantity <= 0
        || fill.price_micros < 0
        || fill.fee_micros < 0
    {
        return Err(AccountingError::InvalidInput("fill is invalid"));
    }
    Ok(())
}

fn required_mark(
    marks: &BTreeMap<String, PriceMicros>,
    symbol: &str,
) -> Result<PriceMicros, AccountingError> {
    let mark = *marks
        .get(symbol)
        .ok_or_else(|| AccountingError::MissingMark {
            symbol: symbol.to_string(),
        })?;
    if mark < 0 {
        return Err(AccountingError::InvalidInput("mark must not be negative"));
    }
    Ok(mark)
}

fn notional(quantity: Quantity, price_micros: PriceMicros) -> Result<MoneyMicros, AccountingError> {
    let numerator = i128::from(quantity)
        .checked_mul(i128::from(price_micros))
        .ok_or(AccountingError::ArithmeticOverflow)?;
    i64::try_from(numerator / i128::from(QUANTITY_SCALE))
        .map_err(|_| AccountingError::ArithmeticOverflow)
}

fn checked_add(left: i64, right: i64) -> Result<i64, AccountingError> {
    left.checked_add(right)
        .ok_or(AccountingError::ArithmeticOverflow)
}

fn checked_sub(left: i64, right: i64) -> Result<i64, AccountingError> {
    left.checked_sub(right)
        .ok_or(AccountingError::ArithmeticOverflow)
}

fn checked_neg(value: i64) -> Result<i64, AccountingError> {
    value
        .checked_neg()
        .ok_or(AccountingError::ArithmeticOverflow)
}
