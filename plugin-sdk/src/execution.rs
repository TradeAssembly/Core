//! Exact versioned execution values. Legacy broker micro-unit wire is unchanged.
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};
const MAX_SCALE: u8 = 12;
const MAX_COEFFICIENT: i128 = 1_000_000_000_000_000_000_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal {
    coefficient: i128,
    scale: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    Floor,
    Ceil,
    Exact,
}
#[derive(Clone, Copy, Debug)]
pub struct Product {
    coefficient: i128,
    scale: u8,
}
fn pow(scale: u8) -> Result<i128, String> {
    10_i128
        .checked_pow(u32::from(scale))
        .ok_or("execution_decimal_overflow".into())
}
fn normalize(mut coefficient: i128, mut scale: u8) -> (i128, u8) {
    while scale > 0 && coefficient % 10 == 0 {
        coefficient /= 10;
        scale -= 1;
    }
    (coefficient, scale)
}
impl FromStr for Decimal {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > 40 || text.trim() != text || text.starts_with('+') {
            return Err("execution_decimal_invalid".into());
        }
        let (negative, unsigned) = text.strip_prefix('-').map_or((false, text), |s| (true, s));
        let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || fraction.len() > usize::from(MAX_SCALE)
            || unsigned.ends_with('.')
        {
            return Err("execution_decimal_invalid".into());
        }
        let digits = format!("{whole}{fraction}");
        let mut coefficient = digits
            .parse::<i128>()
            .map_err(|_| "execution_decimal_overflow")?;
        if coefficient > MAX_COEFFICIENT {
            return Err("execution_decimal_bounds_invalid".into());
        }
        if negative {
            coefficient = -coefficient;
        }
        let (coefficient, scale) = normalize(coefficient, fraction.len() as u8);
        Ok(Self { coefficient, scale })
    }
}
impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.coefficient < 0 { "-" } else { "" };
        let digits = self.coefficient.unsigned_abs().to_string();
        if self.scale == 0 {
            return write!(f, "{sign}{digits}");
        }
        let width = usize::from(self.scale);
        if digits.len() <= width {
            write!(f, "{sign}0.{}{}", "0".repeat(width - digits.len()), digits)
        } else {
            let split = digits.len() - width;
            write!(f, "{sign}{}.{}", &digits[..split], &digits[split..])
        }
    }
}
impl Serialize for Decimal {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(D::Error::custom)
    }
}
impl Decimal {
    pub const ZERO: Self = Self {
        coefficient: 0,
        scale: 0,
    };
    pub fn positive(self) -> bool {
        self.coefficient > 0
    }
    pub fn negative(self) -> bool {
        self.coefficient < 0
    }
    pub fn is_integer(self) -> bool {
        self.scale == 0
    }
    fn aligned(self, other: Self) -> Result<(i128, i128, u8), String> {
        let scale = self.scale.max(other.scale);
        Ok((
            self.coefficient
                .checked_mul(pow(scale - self.scale)?)
                .ok_or("execution_decimal_overflow")?,
            other
                .coefficient
                .checked_mul(pow(scale - other.scale)?)
                .ok_or("execution_decimal_overflow")?,
            scale,
        ))
    }
    pub fn compare(self, other: Self) -> Result<std::cmp::Ordering, String> {
        let (a, b, _) = self.aligned(other)?;
        Ok(a.cmp(&b))
    }
    fn from_parts(coefficient: i128, scale: u8) -> Result<Self, String> {
        let (coefficient, scale) = normalize(coefficient, scale);
        if scale > MAX_SCALE || coefficient.unsigned_abs() > MAX_COEFFICIENT as u128 {
            return Err("execution_decimal_bounds_invalid".into());
        }
        Ok(Self { coefficient, scale })
    }
    pub fn checked_add(self, other: Self) -> Result<Self, String> {
        let (a, b, scale) = self.aligned(other)?;
        Self::from_parts(a.checked_add(b).ok_or("execution_decimal_overflow")?, scale)
    }
    pub fn checked_subtract(self, other: Self) -> Result<Self, String> {
        let (a, b, scale) = self.aligned(other)?;
        Self::from_parts(a.checked_sub(b).ok_or("execution_decimal_overflow")?, scale)
    }
    pub fn is_multiple(self, step: Self) -> Result<bool, String> {
        if !step.positive() {
            return Err("execution_increment_invalid".into());
        }
        let (a, b, _) = self.aligned(step)?;
        Ok(a % b == 0)
    }
    pub fn multiply(self, other: Self) -> Result<Product, String> {
        Product {
            coefficient: self.coefficient,
            scale: self.scale,
        }
        .multiply(other)
    }
}
impl Product {
    pub fn multiply(self, other: Decimal) -> Result<Self, String> {
        let scale = self
            .scale
            .checked_add(other.scale)
            .filter(|s| *s <= 36)
            .ok_or("execution_decimal_overflow")?;
        let coefficient = self
            .coefficient
            .checked_mul(other.coefficient)
            .ok_or("execution_decimal_overflow")?;
        let (coefficient, scale) = normalize(coefficient, scale);
        Ok(Self { coefficient, scale })
    }
    pub fn round(self, scale: u8, mode: Rounding) -> Result<Decimal, String> {
        if scale > MAX_SCALE {
            return Err("execution_currency_precision_invalid".into());
        }
        if self.scale <= scale {
            return Decimal::from_parts(self.coefficient, self.scale);
        }
        let divisor = pow(self.scale - scale)?;
        let quotient = self.coefficient / divisor;
        let remainder = self.coefficient % divisor;
        let delta = match mode {
            Rounding::Exact if remainder != 0 => {
                return Err("execution_decimal_precision_loss".into())
            }
            Rounding::Ceil if remainder > 0 => 1,
            Rounding::Floor if remainder < 0 => -1,
            _ => 0,
        };
        Decimal::from_parts(
            quotient
                .checked_add(delta)
                .ok_or("execution_decimal_overflow")?,
            scale,
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderVersion {
    #[serde(rename = "tradeassembly.broker_order.v2")]
    V2,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantityUnit {
    Shares,
    Contracts,
    BaseAsset,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionAction {
    BuyToOpen,
    SellToClose,
    BuyToClose,
    SellToOpen,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PriceInstruction {
    Limit { limit_price: Decimal },
    Market {},
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstrumentIdentity {
    pub instrument_id: String,
    pub provider_symbol: String,
    pub asset_class: String,
    pub instrument_family: String,
    pub quantity_unit: QuantityUnit,
    pub multiplier: Decimal,
    pub quote_currency: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrokerOrderV2 {
    pub schema_version: OrderVersion,
    pub instrument: InstrumentIdentity,
    pub position_action: PositionAction,
    pub quantity: Decimal,
    pub pricing: PriceInstruction,
    pub time_in_force: String,
    pub client_order_id: String,
}
fn label(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.trim() == s && !s.chars().any(char::is_control)
}
impl BrokerOrderV2 {
    pub fn validate(&self) -> Result<(), String> {
        let i = &self.instrument;
        if !label(&i.instrument_id, 256)
            || !label(&i.provider_symbol, 128)
            || !label(&i.asset_class, 64)
            || !label(&i.instrument_family, 64)
            || !label(&i.quote_currency, 16)
            || !label(&self.client_order_id, 128)
            || !self.quantity.positive()
            || !i.multiplier.positive()
            || !matches!(self.time_in_force.as_str(), "day" | "gtc")
        {
            return Err("broker_order_v2_invalid".into());
        }
        if i.quantity_unit == QuantityUnit::Contracts && !self.quantity.is_integer() {
            return Err("broker_order_v2_fractional_contract_unsupported".into());
        }
        if let PriceInstruction::Limit { limit_price } = self.pricing {
            if !limit_price.positive() {
                return Err("broker_order_v2_price_invalid".into());
            }
        }
        Ok(())
    }
    /// Conservative cash estimate; only a named current metadata/quote/account
    /// admission can authorize its use. No fee or fill assumption is introduced.
    pub fn limit_notional(&self, currency_scale: u8) -> Result<Decimal, String> {
        self.validate()?;
        let PriceInstruction::Limit { limit_price } = self.pricing else {
            return Err("broker_order_v2_market_notional_unbounded".into());
        };
        self.quantity
            .multiply(limit_price)?
            .multiply(self.instrument.multiplier)?
            .round(currency_scale, Rounding::Ceil)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }
    #[test]
    fn exact_normalization_precision_and_signed_rounding() {
        assert_eq!(d("001.2300").to_string(), "1.23");
        assert_eq!(d("-0.000").to_string(), "0");
        assert_ne!(d("9007199254740992"), d("9007199254740993"));
        assert_eq!(
            d("0.00000001")
                .multiply(d("65000.01"))
                .unwrap()
                .round(8, Rounding::Ceil)
                .unwrap(),
            d("0.00065001")
        );
        assert_eq!(
            d("-1.001")
                .multiply(d("1"))
                .unwrap()
                .round(2, Rounding::Floor)
                .unwrap(),
            d("-1.01")
        );
        assert!(d("1.001")
            .multiply(d("1"))
            .unwrap()
            .round(2, Rounding::Exact)
            .is_err());
        for bad in ["1e-8", "NaN", "+1", "1.", ".1", " 1", "1.0000000000001"] {
            assert!(bad.parse::<Decimal>().is_err(), "{bad}");
        }
        assert!(serde_json::from_str::<Decimal>("0.1").is_err());
        assert!(d("1.25").is_multiple(d("0.05")).unwrap());
        assert!(!d("1.26").is_multiple(d("0.05")).unwrap());
    }
    #[test]
    fn explicit_contract_multiplier_and_strict_wire() {
        let order:BrokerOrderV2=serde_json::from_value(serde_json::json!({"schemaVersion":"tradeassembly.broker_order.v2","instrument":{"instrumentId":"fixture-option","providerSymbol":"fixture-occ","assetClass":"option","instrumentFamily":"equity_option","quantityUnit":"contracts","multiplier":"100","quoteCurrency":"USD"},"positionAction":"buy_to_open","quantity":"2","pricing":{"kind":"limit","limitPrice":"1.25"},"timeInForce":"day","clientOrderId":"fixture-order"})).unwrap();
        assert_eq!(order.limit_notional(2).unwrap(), d("250"));
        let mut micro = order.clone();
        micro.instrument.multiplier = d("0.1");
        micro.quantity = d("1");
        micro.pricing = PriceInstruction::Limit {
            limit_price: d("65000"),
        };
        assert_eq!(micro.limit_notional(2).unwrap(), d("6500"));

        let mut fractional = order.clone();
        fractional.quantity = d("0.5");
        assert!(fractional.validate().is_err());
        let mut market = order.clone();
        market.pricing = PriceInstruction::Market {};
        assert_eq!(
            market.limit_notional(2).unwrap_err(),
            "broker_order_v2_market_notional_unbounded"
        );
        let mut v = serde_json::to_value(order).unwrap();
        v["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<BrokerOrderV2>(v.clone()).is_err());
        v.as_object_mut().unwrap().remove("unknown");
        v["schemaVersion"] = serde_json::json!("tradeassembly.broker_order.v99");
        assert!(serde_json::from_value::<BrokerOrderV2>(v).is_err());
    }
}
