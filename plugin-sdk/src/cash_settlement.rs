//! Provider-observed settlement, never inference from a later wall-clock balance.
//! A provider MUST support a coherent monotonic account revision which includes
//! the named posting and all cash/position/open-order fields at that revision.
//! Providers without that evidence cannot advertise this operation.
use crate::{
    broker_v2::{self, OrderObservation, OrderStatus},
    execution::Decimal,
    execution_snapshot::{self as account, AccountSnapshot, TradingMode},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub const OPERATION: &str = "broker.cash_settlement.paper_v1";
pub const CAPABILITY: &str = "broker.cash_settlement.paper@1";
pub const REQUEST_SCHEMA: &str = "schema://tradeassembly/cash-settlement-request@1";
pub const RESPONSE_SCHEMA: &str = "schema://tradeassembly/cash-settlement-observation@1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub schema_version: String,
    pub plan_id: String,
    pub canonical_account_sha256: String,
    pub client_order_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountCheckpoint {
    pub ledger_epoch: String,
    pub sequence: u64,
    pub account: AccountSnapshot,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub schema_version: String,
    pub canonical_account_sha256: String,
    pub order: OrderObservation,
    pub posting_id: String,
    pub posting_sequence: u64,
    pub gross_cash: Decimal,
    pub fee_cash: Decimal,
    pub fees_final: bool,
    pub checkpoint: AccountCheckpoint,
}
fn label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_graphic())
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Observation {
    pub fn validate(&self, now: i64, max_age_ms: i64) -> Result<(), String> {
        self.order.validate(now)?;
        let a = &self.checkpoint.account;
        a.validate()?;
        let debit = *self.order.cash_spent.required()?;
        if self.schema_version != "tradeassembly.cash_settlement_observation.v1"
            || !digest(&self.canonical_account_sha256)
            || self.canonical_account_sha256 != self.order.canonical_account_sha256
            || !label(&self.posting_id)
            || !label(&self.checkpoint.ledger_epoch)
            || self.posting_sequence == 0
            || self.posting_sequence > self.checkpoint.sequence
            || self.checkpoint.sequence > crate::canonical::MAX_EXACT_INTEGER
            || !self.fees_final
            || self.gross_cash.negative()
            || self.fee_cash.negative()
            || self.gross_cash.checked_add(self.fee_cash)? != debit
            || !matches!(
                self.order.status,
                OrderStatus::Filled
                    | OrderStatus::Rejected
                    | OrderStatus::Cancelled
                    | OrderStatus::Expired
            )
            || self.order.provider_order_id.is_none()
            || a.mode != TradingMode::Paper
            || !a.coverage.account_complete
            || !a.coverage.positions_complete
            || !a.coverage.open_orders_complete
            || a.time.observed_at_ms > now
            || a.time.known_at_ms > now
            || now - a.time.observed_at_ms >= max_age_ms
            || max_age_ms <= 0
            || self.order.known_at_ms > a.time.known_at_ms
        {
            return Err("cash_settlement_evidence_invalid".into());
        }
        if *self.order.filled_quantity.required()? == Decimal::ZERO
            && self.gross_cash != Decimal::ZERO
        {
            return Err("cash_settlement_unfilled_gross_invalid".into());
        }
        for d in [
            *a.cash_balance.required()?,
            *a.non_marginable_buying_power.required()?,
            *a.gross_exposure.required()?,
        ] {
            if d.negative() {
                return Err("cash_settlement_account_invalid".into());
            }
        }
        if *a.borrowing_debt.required()? != Decimal::ZERO {
            return Err("cash_settlement_borrowing_unsupported".into());
        }
        a.positions.required()?;
        a.open_orders.required()?;
        for amount in [self.gross_cash, self.fee_cash, debit] {
            amount
                .multiply("1".parse()?)?
                .round(a.currency.decimals, crate::execution::Rounding::Exact)?;
        }
        Ok(())
    }
}
fn object(v: Value) -> Value {
    let required = v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    json!({"type":"object","properties":v,"required":required,"additionalProperties":false})
}
fn text() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}
pub fn request_schema() -> Value {
    object(
        json!({"schemaVersion":{"const":"tradeassembly.cash_settlement_request.v1"},"planId":text(),"canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"clientOrderId":text()}),
    )
}
pub fn response_schema() -> Value {
    object(
        json!({"schemaVersion":{"const":"tradeassembly.cash_settlement_observation.v1"},"canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"order":broker_v2::observation_schema(),"postingId":text(),"postingSequence":{"type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER},"grossCash":account::decimal_schema(),"feeCash":account::decimal_schema(),"feesFinal":{"type":"boolean"},"checkpoint":object(json!({"ledgerEpoch":text(),"sequence":{"type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER},"account":account::account_schema()}))}),
    )
}
