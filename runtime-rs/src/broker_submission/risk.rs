use serde_json::{Map, Value};

const MICROS_PER_UNIT: i128 = 1_000_000;

/// Enforce the explicit, bounded pre-trade controls for one order.
///
/// This is a deterministic estimate. It does not make a fill guarantee.
pub(super) fn enforce_order_limits(
    quantity_micros: i64,
    price_micros: i64,
    limits: &Value,
) -> Result<Value, String> {
    if quantity_micros <= 0 {
        return Err("invalid_order_quantity".to_owned());
    }
    if price_micros <= 0 {
        return Err("invalid_order_price".to_owned());
    }

    let object = limits
        .as_object()
        .ok_or_else(|| "missing_risk_limits".to_owned())?;
    reject_unsupported_controls(object)?;

    let maximum_order_quantity_micros = parse_limit(object, "max_order_quantity")?;
    let maximum_notional_micros = parse_limit(object, "max_notional")?;

    if quantity_micros > maximum_order_quantity_micros {
        return Err("order_quantity_limit_exceeded".to_owned());
    }

    let product = i128::from(quantity_micros)
        .checked_mul(i128::from(price_micros))
        .ok_or_else(|| "invalid_order_notional".to_owned())?;
    let maximum_product = i128::from(maximum_notional_micros)
        .checked_mul(MICROS_PER_UNIT)
        .ok_or_else(|| "invalid_max_notional".to_owned())?;
    if product > maximum_product {
        return Err("order_notional_limit_exceeded".to_owned());
    }

    let estimated_notional_micros = product
        .checked_add(MICROS_PER_UNIT - 1)
        .and_then(|value| value.checked_div(MICROS_PER_UNIT))
        .ok_or_else(|| "invalid_order_notional".to_owned())?;
    let estimated_notional_micros = i64::try_from(estimated_notional_micros)
        .map_err(|_| "invalid_order_notional".to_owned())?;

    Ok(serde_json::json!({
        "priceMicros": price_micros,
        "quantityMicros": quantity_micros,
        "estimatedNotionalMicros": estimated_notional_micros,
        "maximumNotionalMicros": maximum_notional_micros,
        "maximumOrderQuantityMicros": maximum_order_quantity_micros,
    }))
}

fn reject_unsupported_controls(object: &Map<String, Value>) -> Result<(), String> {
    for key in object.keys() {
        if key != "max_notional" && key != "max_order_quantity" {
            return Err("unsupported_risk_control".to_owned());
        }
    }
    Ok(())
}

fn parse_limit(object: &Map<String, Value>, key: &str) -> Result<i64, String> {
    let value = object.get(key).ok_or_else(|| format!("missing_{key}"))?;
    let parsed = crate::broker_order_intent::parse_quantity(value)?;
    if parsed <= 0 {
        return Err(format!("invalid_{key}"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn limits() -> Value {
        json!({"max_notional": "10", "max_order_quantity": "2"})
    }

    #[test]
    fn accepts_boundary_equality_and_rounds_notional_up() {
        let result = enforce_order_limits(2_000_000, 5_000_000, &limits()).unwrap();
        assert_eq!(result["quantityMicros"], 2_000_000);
        assert_eq!(result["priceMicros"], 5_000_000);
        assert_eq!(result["estimatedNotionalMicros"], 10_000_000);
        let fraction = enforce_order_limits(1_000_001, 1, &limits()).unwrap();
        assert_eq!(fraction["estimatedNotionalMicros"], 2);
    }

    #[test]
    fn rejects_quantity_above_limit() {
        assert_eq!(
            enforce_order_limits(2_000_001, 1_000_000, &limits()),
            Err("order_quantity_limit_exceeded".to_owned())
        );
    }

    #[test]
    fn rejects_fractional_unit_notional_overcap_without_rounding_bypass() {
        let limits = json!({"max_notional": "0.000001", "max_order_quantity": "2"});
        assert_eq!(
            enforce_order_limits(1_000_001, 1, &limits),
            Err("order_notional_limit_exceeded".to_owned())
        );
    }

    #[test]
    fn rejects_invalid_and_overflow_values() {
        for (quantity, price) in [(0, 1), (1, 0), (i64::MAX, 1)] {
            assert!(enforce_order_limits(quantity, price, &limits()).is_err());
        }
        for value in [
            json!(0),
            json!("0"),
            json!("9223372036854.775808"),
            json!("1e2"),
        ] {
            let invalid = json!({"max_notional": value, "max_order_quantity": "2"});
            assert!(enforce_order_limits(1, 1, &invalid).is_err());
        }
    }

    #[test]
    fn requires_explicit_limits() {
        for value in [Value::Null, json!({}), json!({"max_notional": "1"})] {
            assert!(enforce_order_limits(1, 1, &value).is_err());
        }
    }

    #[test]
    fn rejects_every_unsupported_configured_control() {
        for key in ["daily_loss", "positions", "max_position", "stop_loss"] {
            let mut value = limits();
            value
                .as_object_mut()
                .expect("limits object")
                .insert((*key).to_owned(), json!("1"));
            assert_eq!(
                enforce_order_limits(1, 1, &value),
                Err("unsupported_risk_control".to_owned())
            );
        }
    }
}
