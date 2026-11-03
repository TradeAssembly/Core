//! Provider-confirmed sale proceeds and a separate final fee debit.
//!
//! This read contract never derives proceeds from a limit price or a balance
//! delta. A provider must supply a coherent account revision that includes the
//! named posting. The host still owns account acquisition, plan/entry ownership,
//! checkpoint epoch/replay checks, and atomic cash/position reconciliation.
use crate::{
    broker_v2::{self, OrderObservation, OrderStatus},
    execution::{BrokerOrderV2, Decimal, PositionAction, PriceInstruction, QuantityUnit, Rounding},
    execution_snapshot::{self as account, TradingMode},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const OPERATION: &str = "broker.close_settlement.paper_v1";
pub const CAPABILITY: &str = "broker.close_settlement.paper@1";
pub const REQUEST_SCHEMA: &str = "schema://tradeassembly/close-settlement-request@1";
pub const RESPONSE_SCHEMA: &str = "schema://tradeassembly/close-settlement-observation@1";
pub const OBSERVATION_SCHEMA: &str = RESPONSE_SCHEMA;

/// The same coherent, monotonic account checkpoint used by cash settlement.
pub use crate::cash_settlement::AccountCheckpoint;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub schema_version: String,
    pub plan_id: String,
    pub original_entry_plan_id: String,
    pub canonical_account_sha256: String,
    pub client_order_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub schema_version: String,
    pub canonical_account_sha256: String,
    pub order: OrderObservation,
    pub posting_id: String,
    pub posting_sequence: u64,
    /// Actual cumulative proceeds before fees, never expected limit notional.
    pub gross_proceeds: Decimal,
    /// Actual cumulative cash debit for fees, separate from gross proceeds.
    pub fee_cash: Decimal,
    pub fees_final: bool,
    pub checkpoint: AccountCheckpoint,
}

fn label(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_graphic())
}

fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn plan_id(s: &str) -> bool {
    s.strip_prefix("orderplan_").is_some_and(digest)
}

impl Request {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != "tradeassembly.close_settlement_request.v1"
            || !plan_id(&self.plan_id)
            || !plan_id(&self.original_entry_plan_id)
            || !digest(&self.canonical_account_sha256)
            || !label(&self.client_order_id, 128)
        {
            return Err("close_settlement_request_invalid".into());
        }
        Ok(())
    }
}

impl Observation {
    pub fn validate(&self, now: i64, max_age_ms: i64) -> Result<(), String> {
        // Bound the clock before any age subtraction and enforce the same exact
        // integer wire domain for order and account observation timestamps.
        if now <= 0 || now > crate::canonical::MAX_EXACT_INTEGER as i64 || max_age_ms <= 0 {
            return Err("close_settlement_time_invalid".into());
        }
        self.order.validate(now)?;
        let a = &self.checkpoint.account;
        a.validate()?;
        let filled = *self.order.filled_quantity.required()?;
        let fee_debit = *self.order.cash_spent.required()?;
        if self.schema_version != "tradeassembly.close_settlement_observation.v1"
            || !digest(&self.canonical_account_sha256)
            || self.canonical_account_sha256 != self.order.canonical_account_sha256
            || self.order.order.position_action != PositionAction::SellToClose
            || !matches!(self.order.order.pricing, PriceInstruction::Limit { .. })
            || !matches!(
                self.order.status,
                OrderStatus::Filled
                    | OrderStatus::Cancelled
                    | OrderStatus::Expired
                    | OrderStatus::Rejected
            )
            || (self.order.status != OrderStatus::Rejected && !self.order.provider_order_id.as_deref().is_some_and(|s| label(s,256)))
            || !label(&self.posting_id, 256)
            || !label(&self.checkpoint.ledger_epoch, 256)
            || self.posting_sequence == 0
            || self.posting_sequence > self.checkpoint.sequence
            || self.checkpoint.sequence > crate::canonical::MAX_EXACT_INTEGER
            || !self.fees_final
            || self.gross_proceeds.negative()
            || self.fee_cash.negative()
            // Sale cashSpent is FEES DEBIT ONLY. Neither a negative credit nor
            // a positive net-credit substitute is settlement fee evidence.
            || fee_debit != self.fee_cash
            || (filled.positive() && !self.gross_proceeds.positive())
            || (filled == Decimal::ZERO && self.gross_proceeds != Decimal::ZERO)
            || (self.order.order.instrument.quantity_unit == QuantityUnit::Contracts
                && !filled.is_integer())
            || a.mode != TradingMode::Paper
            || self.order.order.instrument.quote_currency != a.currency.code
            || !a.coverage.account_complete
            || !a.coverage.positions_complete
            || !a.coverage.open_orders_complete
            || a.time.observed_at_ms > now
            || a.time.known_at_ms > now
            || now - a.time.observed_at_ms >= max_age_ms
            || now - self.order.observed_at_ms >= max_age_ms
            || self.order.observed_at_ms > a.time.observed_at_ms
            || self.order.known_at_ms > a.time.known_at_ms
        {
            return Err("close_settlement_evidence_invalid".into());
        }
        for amount in [
            *a.cash_balance.required()?,
            *a.non_marginable_buying_power.required()?,
            *a.gross_exposure.required()?,
        ] {
            if amount.negative() {
                return Err("close_settlement_account_invalid".into());
            }
        }
        if *a.borrowing_debt.required()? != Decimal::ZERO {
            return Err("close_settlement_borrowing_unsupported".into());
        }
        a.positions.required()?;
        a.open_orders.required()?;
        for amount in [self.gross_proceeds, self.fee_cash, fee_debit] {
            amount
                .multiply("1".parse()?)?
                .round(a.currency.decimals, Rounding::Exact)?;
        }
        Ok(())
    }

    /// Bind observed account/order evidence to the host's persisted close order.
    /// The host must independently bind both request plan IDs to its ledger.
    pub fn validate_for(
        &self,
        request: &Request,
        expected_order: &BrokerOrderV2,
        now: i64,
        max_age_ms: i64,
    ) -> Result<(), String> {
        request.validate()?;
        self.validate(now, max_age_ms)?;
        if self.canonical_account_sha256 != request.canonical_account_sha256
            || self.order.order.client_order_id != request.client_order_id
            || self.order.order != *expected_order
        {
            return Err("close_settlement_binding_invalid".into());
        }
        Ok(())
    }
}

pub fn decode_request_json(bytes: &[u8]) -> Result<Request, String> {
    let request: Request =
        serde_json::from_slice(bytes).map_err(|_| "close_settlement_request_invalid")?;
    request.validate()?;
    Ok(request)
}

fn object(properties: Value) -> Value {
    let required = properties
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

fn text(max: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max,"pattern":"^[!-~]+$"})
}

pub fn request_schema() -> Value {
    let plan = json!({"type":"string","pattern":"^orderplan_[0-9a-f]{64}$"});
    object(json!({
        "schemaVersion":{"const":"tradeassembly.close_settlement_request.v1"},
        "planId":plan,
        "originalEntryPlanId":plan,
        "canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},
        "clientOrderId":text(128)
    }))
}

pub fn observation_schema() -> Value {
    let mut order = broker_v2::observation_schema();
    order["properties"]["order"]["properties"]["positionAction"] = json!({"const":"sell_to_close"});
    order["properties"]["status"] = json!({"enum":["filled","cancelled","expired","rejected"]});
    order["properties"]["providerOrderId"] = text(256);
    let available = object(json!({
        "status":{"const":"available"},
        "value":account::decimal_schema()
    }));
    order["properties"]["filledQuantity"] = available.clone();
    order["properties"]["cashSpent"] = available;
    let sequence = json!({
        "type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER
    });
    let mut account = account::account_schema();
    account["properties"]["mode"] = json!({"const":"paper"});
    for field in ["accountComplete", "positionsComplete", "openOrdersComplete"] {
        account["properties"]["coverage"]["properties"][field] = json!({"const":true});
    }
    object(json!({
        "schemaVersion":{"const":"tradeassembly.close_settlement_observation.v1"},
        "canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},
        "order":order,
        "postingId":text(256),
        "postingSequence":sequence,
        "grossProceeds":account::decimal_schema(),
        "feeCash":account::decimal_schema(),
        "feesFinal":{"const":true},
        "checkpoint":object(json!({"ledgerEpoch":text(256),"sequence":sequence,"account":account}))
    }))
}

pub fn response_schema() -> Value {
    observation_schema()
}

pub fn schema() -> Value {
    observation_schema()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_snapshot::Observed;

    fn request() -> Request {
        Request {
            schema_version: "tradeassembly.close_settlement_request.v1".into(),
            plan_id: format!("orderplan_{}", "a".repeat(64)),
            original_entry_plan_id: format!("orderplan_{}", "b".repeat(64)),
            canonical_account_sha256: "c".repeat(64),
            client_order_id: "fixture.close".into(),
        }
    }

    fn observation() -> Observation {
        serde_json::from_value(json!({
            "schemaVersion":"tradeassembly.close_settlement_observation.v1",
            "canonicalAccountSha256":"c".repeat(64),
            "order":{
                "schemaVersion":"tradeassembly.broker_order_observation.v2",
                "canonicalAccountSha256":"c".repeat(64),
                "order":{
                    "schemaVersion":"tradeassembly.broker_order.v2",
                    "instrument":{"instrumentId":"fixture-option","providerSymbol":"FIX",
                        "assetClass":"option","instrumentFamily":"equity_option",
                        "quantityUnit":"contracts","multiplier":"100","quoteCurrency":"USD"},
                    "positionAction":"sell_to_close","quantity":"2",
                    "pricing":{"kind":"limit","limitPrice":"1.25"},
                    "timeInForce":"day","clientOrderId":"fixture.close"
                },
                "providerOrderId":"fixture.provider","status":"filled",
                "filledQuantity":{"status":"available","value":"2"},
                "cashSpent":{"status":"available","value":"1.25"},
                "observedAtMs":900,"knownAtMs":910
            },
            "postingId":"fixture.posting","postingSequence":4,
            "grossProceeds":"275","feeCash":"1.25","feesFinal":true,
            "checkpoint":{"ledgerEpoch":"fixture.epoch","sequence":5,"account":{
                "schemaVersion":"tradeassembly.execution_account_snapshot.v1",
                "accountRef":"fixture.account","providerAccountId":"fixture.provider.account",
                "mode":"paper","currency":{"code":"USD","decimals":2},
                "time":{"observedAtMs":920,"knownAtMs":930,"basis":"provider_observed"},
                "coverage":{"accountComplete":true,"positionsComplete":true,"openOrdersComplete":true},
                "cashBalance":{"status":"available","value":"1000"},
                "nonMarginableBuyingPower":{"status":"available","value":"1000"},
                "borrowingDebt":{"status":"available","value":"0"},
                "grossExposure":{"status":"available","value":"0"},
                "instrumentPermissions":[],
                "positions":{"status":"available","value":[]},
                "openOrders":{"status":"available","value":[]}
            }}
        })).unwrap()
    }

    fn invalid(change: impl FnOnce(&mut Observation)) {
        let mut evidence = observation();
        change(&mut evidence);
        assert!(evidence.validate(1_000, 200).is_err());
    }

    #[test]
    fn actual_proceeds_and_separate_final_fees_bind_to_the_close_order() {
        let evidence = observation();
        // The observed 275 proceeds deliberately differ from 250 limit notional.
        evidence
            .validate_for(&request(), &evidence.order.order, 1_000, 200)
            .unwrap();
        let mut other = evidence.order.order.clone();
        other.client_order_id = "other.close".into();
        assert!(evidence
            .validate_for(&request(), &other, 1_000, 200)
            .is_err());
        let mut other = request();
        other.canonical_account_sha256 = "d".repeat(64);
        assert!(evidence
            .validate_for(&other, &evidence.order.order, 1_000, 200)
            .is_err());
    }

    #[test]
    fn fee_debit_cannot_be_replaced_by_gross_or_net_credit() {
        for value in ["275", "273.75", "-273.75", "0"] {
            invalid(|o| {
                o.order.cash_spent = Observed::Available {
                    value: value.parse().unwrap(),
                }
            });
        }
        invalid(|o| o.fee_cash = "-1.25".parse().unwrap());
        invalid(|o| o.fee_cash = "1.251".parse().unwrap());
        invalid(|o| o.fees_final = false);
        invalid(|o| {
            o.order.cash_spent = Observed::Unavailable {
                reason: "pending".into(),
            }
        });
    }

    #[test]
    fn terminal_partial_or_zero_fill_has_actual_proceeds_only() {
        for status in [OrderStatus::Cancelled, OrderStatus::Expired] {
            let mut evidence = observation();
            evidence.order.status = status;
            evidence.order.filled_quantity = Observed::Available {
                value: "1".parse().unwrap(),
            };
            evidence.gross_proceeds = "137.5".parse().unwrap();
            evidence.validate(1_000, 200).unwrap();
        }
        let mut evidence = observation();
        evidence.order.status = OrderStatus::Rejected;
        evidence.order.filled_quantity = Observed::Available {
            value: Decimal::ZERO,
        };
        evidence.gross_proceeds = Decimal::ZERO;
        evidence.validate(1_000, 200).unwrap();
        evidence.gross_proceeds = "1".parse().unwrap();
        assert!(evidence.validate(1_000, 200).is_err());
        invalid(|o| o.gross_proceeds = Decimal::ZERO);
        invalid(|o| o.gross_proceeds = "-1".parse().unwrap());
        invalid(|o| o.gross_proceeds = "275.001".parse().unwrap());
        invalid(|o| {
            o.order.filled_quantity = Observed::Available {
                value: "3".parse().unwrap(),
            }
        });
        invalid(|o| {
            o.order.status = OrderStatus::Cancelled;
            o.order.filled_quantity = Observed::Available {
                value: "0.5".parse().unwrap(),
            };
        });
    }

    #[test]
    fn uncertain_non_close_and_incoherent_account_evidence_fail_closed() {
        for status in [
            OrderStatus::Open,
            OrderStatus::PartiallyFilled,
            OrderStatus::Unknown,
        ] {
            invalid(|o| o.order.status = status);
        }
        for action in [
            PositionAction::BuyToOpen,
            PositionAction::BuyToClose,
            PositionAction::SellToOpen,
        ] {
            invalid(|o| o.order.order.position_action = action);
        }
        invalid(|o| o.canonical_account_sha256 = "d".repeat(64));
        invalid(|o| o.order.order.pricing = PriceInstruction::Market {});
        invalid(|o| o.checkpoint.account.mode = TradingMode::Live);
        invalid(|o| o.checkpoint.account.currency.code = "EUR".into());
        invalid(|o| o.checkpoint.account.coverage.positions_complete = false);
        invalid(|o| o.checkpoint.account.coverage.account_complete = false);
        invalid(|o| o.checkpoint.account.coverage.open_orders_complete = false);
        invalid(|o| {
            o.checkpoint.account.cash_balance = Observed::Unavailable {
                reason: "missing".into(),
            }
        });
        invalid(|o| {
            o.checkpoint.account.positions = Observed::Unavailable {
                reason: "missing".into(),
            }
        });
        invalid(|o| {
            o.checkpoint.account.open_orders = Observed::Unavailable {
                reason: "missing".into(),
            }
        });
        invalid(|o| {
            o.checkpoint.account.borrowing_debt = Observed::Available {
                value: "1".parse().unwrap(),
            }
        });
    }

    #[test]
    fn checkpoint_sequences_clocks_and_identifiers_are_bounded() {
        invalid(|o| o.posting_sequence = 0);
        invalid(|o| o.posting_sequence = 6);
        invalid(|o| o.checkpoint.sequence = crate::canonical::MAX_EXACT_INTEGER + 1);
        invalid(|o| o.posting_id = "x".repeat(257));
        invalid(|o| o.checkpoint.ledger_epoch = "bad epoch".into());
        invalid(|o| o.order.provider_order_id = None);
        invalid(|o| o.checkpoint.account.time.observed_at_ms = 800);
        invalid(|o| o.order.observed_at_ms = 800);
        invalid(|o| o.checkpoint.account.time.known_at_ms = 1_001);
        invalid(|o| o.order.known_at_ms = 931);
        assert!(observation().validate(1_000, 0).is_err());
        assert!(observation().validate(i64::MAX, 200).is_err());
        let mut evidence = observation();
        evidence.posting_sequence = crate::canonical::MAX_EXACT_INTEGER;
        evidence.checkpoint.sequence = crate::canonical::MAX_EXACT_INTEGER;
        evidence.validate(1_000, 200).unwrap();
    }

    #[test]
    fn request_and_observation_are_closed_and_versioned() {
        let mut value = serde_json::to_value(request()).unwrap();
        value["unknown"] = json!(true);
        assert!(serde_json::from_value::<Request>(value).is_err());
        let mut invalid_request = request();
        invalid_request.original_entry_plan_id = "orderplan_short".into();
        assert!(invalid_request.validate().is_err());
        invalid_request = request();
        invalid_request.client_order_id = "x".repeat(129);
        assert!(invalid_request.validate().is_err());
        let mut value = serde_json::to_value(observation()).unwrap();
        value["checkpoint"]["unknown"] = json!(true);
        assert!(serde_json::from_value::<Observation>(value).is_err());
        let mut value = serde_json::to_value(observation()).unwrap();
        value["grossProceeds"] = json!(275);
        assert!(serde_json::from_value::<Observation>(value).is_err());
        invalid(|o| o.schema_version = "tradeassembly.cash_settlement_observation.v1".into());
        assert_eq!(schema(), observation_schema());
        assert_eq!(response_schema(), observation_schema());
        assert_eq!(request_schema()["additionalProperties"], json!(false));
        assert_eq!(schema()["properties"]["feesFinal"], json!({"const":true}));
    }
}
