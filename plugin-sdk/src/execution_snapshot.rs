//! Typed financial observations, not proof of truth, eligibility or authority.
//! Hosts must witness acquisition and bind provider, account, schema, scope and
//! freshness independently. Missing values never become false or zero.
use crate::execution::{Decimal, PositionAction, QuantityUnit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub const ACCOUNT_SCHEMA: &str = "schema://tradeassembly/execution-account-snapshot@1";
pub const METADATA_SCHEMA: &str = "schema://tradeassembly/execution-instrument-metadata@1";
pub const QUOTE_SCHEMA: &str = "schema://tradeassembly/execution-quote@1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Observed<T> {
    Available { value: T },
    Unavailable { reason: String },
}
impl<T> Observed<T> {
    pub fn required(&self) -> Result<&T, String> {
        match self {
            Self::Available { value } => Ok(value),
            Self::Unavailable { .. } => Err("execution_observation_unavailable".into()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradingMode {
    Paper,
    Live,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBasis {
    ProviderObserved,
    HostAcquired,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationTime {
    pub observed_at_ms: i64,
    pub known_at_ms: i64,
    pub basis: TimeBasis,
}
impl ObservationTime {
    pub fn validate(&self) -> Result<(), String> {
        if self.observed_at_ms <= 0
            || self.known_at_ms < self.observed_at_ms
            || self.known_at_ms > crate::canonical::MAX_EXACT_INTEGER as i64
        {
            return Err("execution_observation_time_invalid".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Currency {
    pub code: String,
    pub decimals: u8,
}
impl Currency {
    pub fn validate(&self) -> Result<(), String> {
        if self.code.is_empty()
            || self.code.len() > 16
            || !self
                .code
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            || self.decimals > 12
        {
            return Err("execution_currency_invalid".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldingSide {
    Long,
    Short,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Position {
    pub instrument_id: String,
    pub side: HoldingSide,
    pub quantity: Decimal,
    pub gross_value: Observed<Decimal>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenOrder {
    pub provider_order_id: String,
    pub client_order_id: Option<String>,
    pub instrument_id: String,
    pub position_action: PositionAction,
    pub remaining_quantity: Decimal,
    pub worst_case_cash: Observed<Decimal>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Coverage {
    pub account_complete: bool,
    pub positions_complete: bool,
    pub open_orders_complete: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountVersion {
    #[serde(rename = "tradeassembly.execution_account_snapshot.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstrumentPermission {
    pub asset_class: String,
    pub instrument_family: String,
    pub actions: Observed<Vec<PositionAction>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountSnapshot {
    pub schema_version: AccountVersion,
    pub account_ref: String,
    pub provider_account_id: String,
    pub mode: TradingMode,
    pub currency: Currency,
    pub time: ObservationTime,
    pub coverage: Coverage,
    pub cash_balance: Observed<Decimal>,
    /// Explicit provider metric, including its broker holds. Not margin buying power.
    pub non_marginable_buying_power: Observed<Decimal>,
    pub borrowing_debt: Observed<Decimal>,
    pub gross_exposure: Observed<Decimal>,
    pub instrument_permissions: Vec<InstrumentPermission>,
    pub positions: Observed<Vec<Position>>,
    pub open_orders: Observed<Vec<OpenOrder>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataVersion {
    #[serde(rename = "tradeassembly.execution_instrument_metadata.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstrumentMetadata {
    pub schema_version: MetadataVersion,
    pub instrument_id: String,
    pub provider_symbol: String,
    pub asset_class: String,
    pub instrument_family: String,
    pub time: ObservationTime,
    pub quantity_unit: Observed<QuantityUnit>,
    pub multiplier: Observed<Decimal>,
    pub quote_currency: Observed<Currency>,
    pub minimum_quantity: Observed<Decimal>,
    pub quantity_increment: Observed<Decimal>,
    pub price_increment: Observed<Decimal>,
    pub tradable: Observed<bool>,
    /// None means explicitly no expiry; Unavailable means expiry is not known.
    pub expires_at_ms: Observed<Option<i64>>,
    pub standard_deliverable: Observed<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuoteVersion {
    #[serde(rename = "tradeassembly.execution_quote.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteOrigin {
    Provider,
    ExplicitModel,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Quote {
    pub schema_version: QuoteVersion,
    pub instrument_id: String,
    pub currency: Currency,
    pub time: ObservationTime,
    pub origin: QuoteOrigin,
    pub source_profile: String,
    pub indicative: Observed<bool>,
    pub bid: Observed<Decimal>,
    pub ask: Observed<Decimal>,
}
fn label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && s.trim() == s && !s.chars().any(char::is_control)
}
impl AccountSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        self.currency.validate()?;
        self.time.validate()?;
        if !label(&self.account_ref) || !label(&self.provider_account_id) {
            return Err("execution_account_identity_invalid".into());
        }
        if self.instrument_permissions.len() > 32
            || self
                .instrument_permissions
                .iter()
                .any(|p| !label(&p.asset_class) || !label(&p.instrument_family))
        {
            return Err("execution_instrument_permissions_invalid".into());
        }
        let permissions = self
            .instrument_permissions
            .iter()
            .map(|p| (&p.asset_class, &p.instrument_family))
            .collect::<std::collections::BTreeSet<_>>();
        if permissions.len() != self.instrument_permissions.len() {
            return Err("execution_instrument_permissions_duplicate".into());
        }
        if let Observed::Available { value } = &self.positions {
            if value.len() > 4096
                || value
                    .iter()
                    .any(|p| !label(&p.instrument_id) || p.quantity.negative())
            {
                return Err("execution_positions_invalid".into());
            }
            let ids = value
                .iter()
                .map(|p| {
                    (
                        &p.instrument_id,
                        match p.side {
                            HoldingSide::Long => 0,
                            HoldingSide::Short => 1,
                        },
                    )
                })
                .collect::<std::collections::BTreeSet<_>>();
            if ids.len() != value.len() {
                return Err("execution_positions_duplicate".into());
            }
        }
        if let Observed::Available { value } = &self.open_orders {
            if value.len() > 4096
                || value.iter().any(|o| {
                    !label(&o.provider_order_id)
                        || !label(&o.instrument_id)
                        || o.client_order_id.as_deref().is_some_and(|s| !label(s))
                        || o.remaining_quantity.negative()
                })
            {
                return Err("execution_open_orders_invalid".into());
            }
            let ids = value
                .iter()
                .map(|o| &o.provider_order_id)
                .collect::<std::collections::BTreeSet<_>>();
            let clients = value
                .iter()
                .filter_map(|o| o.client_order_id.as_ref())
                .collect::<Vec<_>>();
            if ids.len() != value.len()
                || clients
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != clients.len()
            {
                return Err("execution_open_orders_duplicate".into());
            }
        }
        Ok(())
    }
}
impl InstrumentMetadata {
    pub fn validate(&self) -> Result<(), String> {
        self.time.validate()?;
        if [
            &self.instrument_id,
            &self.provider_symbol,
            &self.asset_class,
            &self.instrument_family,
        ]
        .iter()
        .any(|v| !label(v))
        {
            return Err("execution_instrument_identity_invalid".into());
        }
        for value in [
            &self.multiplier,
            &self.minimum_quantity,
            &self.quantity_increment,
            &self.price_increment,
        ] {
            if let Observed::Available { value } = value {
                if !value.positive() {
                    return Err("execution_instrument_increment_invalid".into());
                }
            }
        }
        if let Observed::Available { value } = &self.quote_currency {
            value.validate()?;
        }
        if let Observed::Available { value: Some(t) } = self.expires_at_ms {
            if t <= 0 || t > crate::canonical::MAX_EXACT_INTEGER as i64 {
                return Err("execution_instrument_expiry_invalid".into());
            }
        }
        Ok(())
    }
}
impl Quote {
    pub fn validate(&self) -> Result<(), String> {
        self.currency.validate()?;
        self.time.validate()?;
        if !label(&self.instrument_id) || !label(&self.source_profile) {
            return Err("execution_quote_identity_invalid".into());
        }
        if let Observed::Available { value } = self.bid {
            if value.negative() {
                return Err("execution_quote_price_invalid".into());
            }
        }
        if let Observed::Available { value } = self.ask {
            if !value.positive() {
                return Err("execution_quote_price_invalid".into());
            }
        }
        if let (Observed::Available { value: bid }, Observed::Available { value: ask }) =
            (&self.bid, &self.ask)
        {
            if bid.compare(*ask)?.is_gt() {
                return Err("execution_quote_crossed".into());
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
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn text() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}
pub fn decimal_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":40,"pattern":"^-?[0-9]+(\\.[0-9]{1,12})?$"})
}
fn observed(v: Value) -> Value {
    json!({"oneOf":[object(json!({"status":{"const":"available"},"value":v})),object(json!({"status":{"const":"unavailable"},"reason":text()}))]})
}
fn time() -> Value {
    let stamp = json!({"type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER});
    object(
        json!({"observedAtMs":stamp,"knownAtMs":stamp,"basis":{"enum":["provider_observed","host_acquired"]}}),
    )
}
fn currency() -> Value {
    object(
        json!({"code":{"type":"string","pattern":"^[A-Z0-9]{1,16}$"},"decimals":{"type":"integer","minimum":0,"maximum":12}}),
    )
}
fn actions() -> Value {
    json!({"type":"array","maxItems":4,"uniqueItems":true,"items":{"enum":["buy_to_open","sell_to_close","buy_to_close","sell_to_open"]}})
}
pub fn account_schema() -> Value {
    let position = object(
        json!({"instrumentId":text(),"side":{"enum":["long","short"]},"quantity":decimal_schema(),"grossValue":observed(decimal_schema())}),
    );
    let order = object(
        json!({"providerOrderId":text(),"clientOrderId":{"anyOf":[text(),{"type":"null"}]},"instrumentId":text(),"positionAction":{"enum":["buy_to_open","sell_to_close","buy_to_close","sell_to_open"]},"remainingQuantity":decimal_schema(),"worstCaseCash":observed(decimal_schema())}),
    );
    object(
        json!({"schemaVersion":{"const":"tradeassembly.execution_account_snapshot.v1"},"accountRef":text(),"providerAccountId":text(),"mode":{"enum":["paper","live"]},"currency":currency(),"time":time(),"coverage":object(json!({"accountComplete":{"type":"boolean"},"positionsComplete":{"type":"boolean"},"openOrdersComplete":{"type":"boolean"}})),"cashBalance":observed(decimal_schema()),"nonMarginableBuyingPower":observed(decimal_schema()),"borrowingDebt":observed(decimal_schema()),"grossExposure":observed(decimal_schema()),"instrumentPermissions":{"type":"array","maxItems":32,"items":object(json!({"assetClass":text(),"instrumentFamily":text(),"actions":observed(actions())}))},"positions":observed(json!({"type":"array","maxItems":4096,"items":position})),"openOrders":observed(json!({"type":"array","maxItems":4096,"items":order}))}),
    )
}
pub fn metadata_schema() -> Value {
    object(
        json!({"schemaVersion":{"const":"tradeassembly.execution_instrument_metadata.v1"},"instrumentId":text(),"providerSymbol":text(),"assetClass":text(),"instrumentFamily":text(),"time":time(),"quantityUnit":observed(json!({"enum":["shares","contracts","base_asset"]})),"multiplier":observed(decimal_schema()),"quoteCurrency":observed(currency()),"minimumQuantity":observed(decimal_schema()),"quantityIncrement":observed(decimal_schema()),"priceIncrement":observed(decimal_schema()),"tradable":observed(json!({"type":"boolean"})),"expiresAtMs":observed(json!({"anyOf":[{"type":"null"},{"type":"integer","minimum":1,"maximum":crate::canonical::MAX_EXACT_INTEGER}]})),"standardDeliverable":observed(json!({"type":"boolean"}))}),
    )
}
pub fn quote_schema() -> Value {
    object(
        json!({"schemaVersion":{"const":"tradeassembly.execution_quote.v1"},"instrumentId":text(),"currency":currency(),"time":time(),"origin":{"enum":["provider","explicit_model"]},"sourceProfile":text(),"indicative":observed(json!({"type":"boolean"})),"bid":observed(decimal_schema()),"ask":observed(decimal_schema())}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_is_not_zero_and_expiry_none_is_not_unknown() {
        let missing: Observed<Decimal> = serde_json::from_value(
            json!({"status":"unavailable","reason":"provider_not_reported"}),
        )
        .unwrap();
        assert!(missing.required().is_err());
        assert_ne!(
            missing,
            Observed::Available {
                value: Decimal::ZERO
            }
        );
        assert!(serde_json::from_value::<Observed<Decimal>>(
            json!({"status":"available","value":0})
        )
        .is_err());
        let no_expiry: Observed<Option<i64>> =
            serde_json::from_value(json!({"status":"available","value":null})).unwrap();
        assert_eq!(no_expiry.required().unwrap(), &None);
        assert!(serde_json::from_value::<Quote>(json!({"schemaVersion":"future"})).is_err());
    }
    #[test]
    fn schemas_are_closed_bounded_and_versioned() {
        for (schema, name) in [
            (
                account_schema(),
                "tradeassembly.execution_account_snapshot.v1",
            ),
            (
                metadata_schema(),
                "tradeassembly.execution_instrument_metadata.v1",
            ),
            (quote_schema(), "tradeassembly.execution_quote.v1"),
        ] {
            assert_eq!(schema["additionalProperties"], false);
            assert_eq!(schema["properties"]["schemaVersion"]["const"], name);
            assert!(serde_json::to_vec(&schema).unwrap().len() < 32768);
        }
    }
}
