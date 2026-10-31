use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json_canonicalizer::to_vec(value)
        .map_err(|error| format!("RFC 8785 canonicalization failed: {error}"))
}

pub fn canonical_hash<T: Serialize>(value: &T) -> Result<String, String> {
    Ok(raw_hash(&canonical_bytes(value)?))
}

pub fn raw_hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_rfc_8785_number_and_key_order_vector() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"numbers":[333333333.33333329,1e30,4.50,2e-3,1e-27],"literals":[null,true,false]}"#,
        )
        .unwrap();
        let canonical =
            String::from_utf8(canonical_bytes(&value).expect("canonical bytes")).expect("utf8");
        assert_eq!(
            canonical,
            r#"{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27]}"#
        );
    }

    #[test]
    fn hash_is_independent_of_object_insertion_order() {
        let left: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        let right: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        assert_eq!(
            canonical_hash(&left).unwrap(),
            canonical_hash(&right).unwrap()
        );
    }
}
