//! Exact, closed V2 limit-order transport. No implicit V1 conversion.
use crate::{
    execution::{BrokerOrderV2, Decimal, PriceInstruction},
    execution_snapshot::Observed,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub const SUBMIT_SCHEMA: &str = "schema://tradeassembly/broker-submit@2";
pub const OBSERVATION_SCHEMA: &str = "schema://tradeassembly/broker-order-observation@2";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitRequest {
    pub schema_version: String,
    pub plan_id: String,
    pub canonical_account_sha256: String,
    pub order: BrokerOrderV2,
}
impl SubmitRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.order.validate()?;
        if self.schema_version != "tradeassembly.broker_submit.v2"
            || !self.plan_id.strip_prefix("orderplan_").is_some_and(digest)
            || !digest(&self.canonical_account_sha256)
            || !matches!(self.order.pricing, PriceInstruction::Limit { .. })
        {
            return Err("broker_v2_request_invalid".into());
        }
        Ok(())
    }
}
pub fn decode_submit_json(bytes: &[u8]) -> Result<SubmitRequest, String> {
    let r: SubmitRequest =
        serde_json::from_slice(bytes).map_err(|_| "broker_v2_request_invalid")?;
    r.validate()?;
    Ok(r)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Rejected,
    Cancelled,
    Expired,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrderObservation {
    pub schema_version: String,
    pub canonical_account_sha256: String,
    pub order: BrokerOrderV2,
    pub provider_order_id: Option<String>,
    pub status: OrderStatus,
    pub filled_quantity: Observed<Decimal>,
    /// Provider-confirmed debit including fees; unavailable is never zero.
    pub cash_spent: Observed<Decimal>,
    pub observed_at_ms: i64,
    pub known_at_ms: i64,
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl OrderObservation {
    pub fn validate(&self, now: i64) -> Result<(), String> {
        self.order.validate()?;
        if self.schema_version != "tradeassembly.broker_order_observation.v2"
            || !digest(&self.canonical_account_sha256)
            || self.observed_at_ms <= 0
            || self.known_at_ms < self.observed_at_ms
            || self.known_at_ms > now
            || self
                .provider_order_id
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err("broker_v2_observation_invalid".into());
        }
        if self.status != OrderStatus::Unknown {
            let filled = *self.filled_quantity.required()?;
            if filled.negative()
                || filled.compare(self.order.quantity)?.is_gt()
                || (self.status == OrderStatus::Filled && filled != self.order.quantity)
                || (matches!(self.status, OrderStatus::Open | OrderStatus::Rejected)
                    && filled != Decimal::ZERO)
                || (self.status == OrderStatus::PartiallyFilled
                    && (!filled.positive() || filled == self.order.quantity))
                || (self.status != OrderStatus::Rejected && self.provider_order_id.is_none())
            {
                return Err("broker_v2_fill_identity_invalid".into());
            }
        }
        if let Observed::Available { value } = self.cash_spent {
            if value.negative() {
                return Err("broker_v2_cash_invalid".into());
            }
        }
        Ok(())
    }
}
fn object(properties: Value) -> Value {
    let required = properties
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
}
fn decimal() -> Value {
    crate::execution_snapshot::decimal_schema()
}
fn label(max: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max})
}
pub fn order_schema() -> Value {
    object(
        json!({"schemaVersion":{"const":"tradeassembly.broker_order.v2"},"instrument":object(json!({"instrumentId":label(256),"providerSymbol":label(128),"assetClass":label(64),"instrumentFamily":label(64),"quantityUnit":{"enum":["shares","contracts","base_asset"]},"multiplier":decimal(),"quoteCurrency":label(16)})),"positionAction":{"enum":["buy_to_open","sell_to_close","buy_to_close","sell_to_open"]},"quantity":decimal(),"pricing":object(json!({"kind":{"const":"limit"},"limitPrice":decimal()})),"timeInForce":{"enum":["day","gtc"]},"clientOrderId":label(128)}),
    )
}
fn observed_decimal() -> Value {
    json!({"oneOf":[object(json!({"status":{"const":"available"},"value":decimal()})),object(json!({"status":{"const":"unavailable"},"reason":label(256)}))]})
}
pub fn submit_schema() -> Value {
    object(
        json!({"schemaVersion":{"const":"tradeassembly.broker_submit.v2"},"planId":{"type":"string","pattern":"^orderplan_[0-9a-f]{64}$"},"canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"order":order_schema()}),
    )
}
pub fn observation_schema() -> Value {
    object(
        json!({"schemaVersion":{"const":"tradeassembly.broker_order_observation.v2"},"canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"order":order_schema(),"providerOrderId":{"type":["string","null"],"minLength":1,"maxLength":256},"status":{"enum":["open","partially_filled","filled","rejected","cancelled","expired","unknown"]},"filledQuantity":observed_decimal(),"cashSpent":observed_decimal(),"observedAtMs":{"type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER},"knownAtMs":{"type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER}}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Value {
        json!({"schemaVersion":"tradeassembly.broker_submit.v2","planId":format!("orderplan_{}","a".repeat(64)),"canonicalAccountSha256":"b".repeat(64),"order":{"schemaVersion":"tradeassembly.broker_order.v2","instrument":{"instrumentId":"fixture","providerSymbol":"FIX","assetClass":"equity","instrumentFamily":"equity","quantityUnit":"shares","multiplier":"1","quoteCurrency":"USD"},"positionAction":"buy_to_open","quantity":"0.123456789012","pricing":{"kind":"limit","limitPrice":"1.01"},"timeInForce":"day","clientOrderId":"fixture.client"}})
    }
    #[test]
    fn exact_v2_cannot_be_silently_downgraded_or_numerically_rounded() {
        let mut value = request();
        let r: SubmitRequest = serde_json::from_value(value.clone()).unwrap();
        r.validate().unwrap();
        assert_eq!(r.order.quantity.to_string(), "0.123456789012");
        value["order"]["quantity"] = json!(0.123456789012);
        assert!(serde_json::from_value::<SubmitRequest>(value).is_err());
        let mut value = request();
        value["order"]["pricing"] = json!({"kind":"market"});
        assert!(serde_json::from_value::<SubmitRequest>(value)
            .unwrap()
            .validate()
            .is_err());
    }
    #[test]
    fn unknown_observation_is_distinct_from_definitive_zero_fill() {
        let r: SubmitRequest = serde_json::from_value(request()).unwrap();
        let mut o = OrderObservation {
            schema_version: "tradeassembly.broker_order_observation.v2".into(),
            canonical_account_sha256: r.canonical_account_sha256,
            order: r.order,
            provider_order_id: None,
            status: OrderStatus::Unknown,
            filled_quantity: Observed::Unavailable {
                reason: "timeout".into(),
            },
            cash_spent: Observed::Unavailable {
                reason: "unobserved".into(),
            },
            observed_at_ms: 1,
            known_at_ms: 1,
        };
        o.validate(1).unwrap();
        o.status = OrderStatus::Rejected;
        assert!(o.validate(1).is_err());
        o.filled_quantity = Observed::Available {
            value: Decimal::ZERO,
        };
        o.cash_spent = Observed::Available {
            value: Decimal::ZERO,
        };
        o.validate(1).unwrap();
    }
}
