//! Shared numeric normalization and canonical identity for data contracts.
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn canonical_sha256<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(value)?;
    normalize_numbers(&mut value)?;
    serde_json_canonicalizer::to_vec(&value).map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

/// Integer identity is exact only inside the interoperable binary64 integer
/// range. Do not let canonicalization silently round a larger integer. Decimal
/// parameters use finite binary64 semantics; integral forms normalize to integers.
pub const MAX_EXACT_INTEGER: u64 = 9_007_199_254_740_991;

pub fn normalize_numbers(value: &mut Value) -> Result<(), serde_json::Error> {
    let invalid =
        || <serde_json::Error as serde::ser::Error>::custom("number_outside_supported_exact_range");
    match value {
        Value::Number(number) => {
            if let Some(n) = number.as_i64() {
                if n.unsigned_abs() > MAX_EXACT_INTEGER {
                    return Err(invalid());
                }
            } else if let Some(n) = number.as_u64() {
                if n > MAX_EXACT_INTEGER {
                    return Err(invalid());
                }
            } else {
                let n = number.as_f64().ok_or_else(invalid)?;
                if !n.is_finite() || n.abs() > MAX_EXACT_INTEGER as f64 {
                    return Err(invalid());
                }
                if n.fract() == 0.0 {
                    *number = serde_json::Number::from(n as i64);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_numbers(value)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                normalize_numbers(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_identity_normalizes_integral_forms_and_rejects_lossy_integers() {
        assert_eq!(
            canonical_sha256(&json!({"window": 5})).unwrap(),
            canonical_sha256(&json!({"window": 5.0})).unwrap()
        );
        assert_eq!(
            canonical_sha256(&json!(-0.0)).unwrap(),
            canonical_sha256(&json!(0)).unwrap()
        );
        assert_ne!(
            canonical_sha256(&json!(5.25)).unwrap(),
            canonical_sha256(&json!(5.5)).unwrap()
        );
        assert!(canonical_sha256(&json!({"nested":[9_007_199_254_740_992_u64]})).is_err());
        assert!(canonical_sha256(&json!(-9_007_199_254_740_992_i64)).is_err());
        assert!(canonical_sha256(&json!(MAX_EXACT_INTEGER)).is_ok());
    }
}
