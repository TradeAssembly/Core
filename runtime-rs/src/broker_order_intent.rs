// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MICROS_PER_UNIT: i128 = 1_000_000;
const MAX_SYMBOL_LEN: usize = 32;
const MAX_CLIENT_ORDER_ID_LEN: usize = 128;

/// The small, explicit order shape supported by the initial broker matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CanonicalBrokerOrder {
    pub(crate) symbol: String,
    pub(crate) side: String,
    pub(crate) order_type: String,
    pub(crate) time_in_force: String,
    pub(crate) client_order_id: String,
    pub(crate) quantity_micros: i64,
}

impl CanonicalBrokerOrder {
    /// Parse the initial market-order input without floating-point conversion.
    pub(crate) fn from_input(input: &Value) -> Result<Self, String> {
        let object = input
            .as_object()
            .ok_or_else(|| "invalid_order_input".to_owned())?;

        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "symbol"
                    | "side"
                    | "orderType"
                    | "timeInForce"
                    | "clientOrderId"
                    | "quantity"
                    | "quantityMicros"
            ) {
                return Err("unsupported_order_field".to_owned());
            }
        }

        let symbol = bounded_string(object.get("symbol"), MAX_SYMBOL_LEN, "symbol")?;
        let side = bounded_string(object.get("side"), 4, "side")?;
        if side != "buy" && side != "sell" {
            return Err("unsupported_order_side".to_owned());
        }

        let order_type = bounded_string(object.get("orderType"), 6, "orderType")?;
        if order_type != "market" {
            return Err("unsupported_order_type".to_owned());
        }

        let time_in_force = bounded_string(object.get("timeInForce"), 3, "timeInForce")?;
        if time_in_force != "day" && time_in_force != "gtc" {
            return Err("unsupported_time_in_force".to_owned());
        }

        let client_order_id = bounded_string(
            object.get("clientOrderId"),
            MAX_CLIENT_ORDER_ID_LEN,
            "clientOrderId",
        )?;

        // Presence of quantity is authoritative: a malformed quantity must not
        // fall through to quantityMicros and thereby change the request.
        let quantity_micros = if let Some(quantity) = object.get("quantity") {
            parse_quantity(quantity)?
        } else {
            parse_quantity_micros(object.get("quantityMicros"))?
        };

        Ok(Self {
            symbol,
            side,
            order_type,
            time_in_force,
            client_order_id,
            quantity_micros,
        })
    }

    /// Decimal quantity used by later broker dispatch comparisons.
    pub(crate) fn canonical_quantity(&self) -> String {
        format_micros(self.quantity_micros)
    }
}

fn bounded_string(value: Option<&Value>, maximum: usize, field: &str) -> Result<String, String> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing_or_invalid_{field}"))?;
    if value.is_empty() || value.len() > maximum {
        return Err(format!("invalid_{field}"));
    }
    Ok(value.to_owned())
}

pub(crate) fn parse_quantity(value: &Value) -> Result<i64, String> {
    let text = match value {
        Value::String(value) => value.as_str().to_owned(),
        Value::Number(value) => value.to_string(),
        _ => return Err("invalid_quantity".to_owned()),
    };
    parse_decimal_micros(&text)
}

fn parse_quantity_micros(value: Option<&Value>) -> Result<i64, String> {
    let value = value.ok_or_else(|| "missing_quantity".to_owned())?;
    let quantity = value
        .as_i64()
        .ok_or_else(|| "invalid_quantity_micros".to_owned())?;
    if quantity <= 0 {
        return Err("invalid_quantity_micros".to_owned());
    }
    Ok(quantity)
}

fn parse_decimal_micros(text: &str) -> Result<i64, String> {
    if text.is_empty() || text.contains('e') || text.contains('E') {
        return Err("invalid_quantity".to_owned());
    }
    if text.starts_with('+') || text.starts_with('-') {
        return Err("invalid_quantity".to_owned());
    }

    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (text, ""),
    };
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 6
    {
        return Err("invalid_quantity".to_owned());
    }

    let whole = whole
        .parse::<i128>()
        .map_err(|_| "invalid_quantity".to_owned())?;
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i128>()
            .map_err(|_| "invalid_quantity".to_owned())?
    };
    let scale = 10_i128.pow((6 - fraction.len()) as u32);
    let micros = whole
        .checked_mul(MICROS_PER_UNIT)
        .and_then(|value| value.checked_add(fraction_value.checked_mul(scale)?))
        .ok_or_else(|| "invalid_quantity".to_owned())?;
    if micros <= 0 || micros > i64::MAX as i128 {
        return Err("invalid_quantity".to_owned());
    }
    Ok(micros as i64)
}

fn format_micros(quantity_micros: i64) -> String {
    let whole = quantity_micros / 1_000_000;
    let fraction = quantity_micros % 1_000_000;
    if fraction == 0 {
        return whole.to_string();
    }
    format!("{whole}.{fraction:06}")
        .trim_end_matches('0')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid() -> Value {
        json!({
            "symbol": "SPY",
            "side": "buy",
            "orderType": "market",
            "timeInForce": "day",
            "clientOrderId": "client-1",
            "quantity": "1.25"
        })
    }

    #[test]
    fn quantity_precedes_quantity_micros() {
        let mut input = valid();
        input["quantityMicros"] = json!(2_000_000);
        assert_eq!(
            CanonicalBrokerOrder::from_input(&input)
                .unwrap()
                .quantity_micros,
            1_250_000
        );
    }

    #[test]
    fn malformed_quantity_does_not_fall_back() {
        let mut input = valid();
        input["quantity"] = json!("bad");
        input["quantityMicros"] = json!(1_000_000);
        assert_eq!(
            CanonicalBrokerOrder::from_input(&input),
            Err("invalid_quantity".to_owned())
        );
    }

    #[test]
    fn rejects_non_positive_and_overflow_quantities() {
        for quantity in [json!(0), json!(-1), json!("9223372036854.775808")] {
            let mut input = valid();
            input["quantity"] = quantity;
            assert!(CanonicalBrokerOrder::from_input(&input).is_err());
        }
    }

    #[test]
    fn rejects_excessive_precision_and_exponents() {
        for quantity in [json!("1.0000001"), json!("1e2")] {
            let mut input = valid();
            input["quantity"] = quantity;
            assert_eq!(
                CanonicalBrokerOrder::from_input(&input),
                Err("invalid_quantity".to_owned())
            );
        }
    }

    #[test]
    fn numeric_and_string_quantities_are_exactly_equal() {
        let mut numeric = valid();
        numeric["quantity"] = json!(1.25);
        let string = CanonicalBrokerOrder::from_input(&valid()).unwrap();
        assert_eq!(CanonicalBrokerOrder::from_input(&numeric).unwrap(), string);
        assert_eq!(string.canonical_quantity(), "1.25");
    }

    #[test]
    fn quantity_micros_is_supported_when_quantity_absent() {
        let mut input = valid();
        input.as_object_mut().unwrap().remove("quantity");
        input["quantityMicros"] = json!(1_250_000);
        let order = CanonicalBrokerOrder::from_input(&input).unwrap();
        assert_eq!(order.canonical_quantity(), "1.25");
    }

    #[test]
    fn rejects_missing_fields_and_unsupported_values() {
        let mut missing = valid();
        missing.as_object_mut().unwrap().remove("clientOrderId");
        assert_eq!(
            CanonicalBrokerOrder::from_input(&missing),
            Err("missing_or_invalid_clientOrderId".to_owned())
        );
        for (field, value) in [("orderType", json!("limit")), ("timeInForce", json!("ioc"))] {
            let mut input = valid();
            input[field] = value;
            assert!(CanonicalBrokerOrder::from_input(&input).is_err());
        }

        let mut priced = valid();
        priced["limit_price"] = json!(100);
        assert_eq!(
            CanonicalBrokerOrder::from_input(&priced),
            Err("unsupported_order_field".to_owned())
        );
    }
}
