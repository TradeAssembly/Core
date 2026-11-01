//! Stateless paper lookup with the exact original host order and account binding.
//!
//! Host-owned instrument labels are supplied context, not provider-derived
//! facts. A provider must independently compare the observed account and every
//! observable order field before echoing the original host identity. This read
//! request grants no submit authority and supplies no settlement evidence.
use crate::{broker_v2, canonical::canonical_sha256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const LOOKUP_SCHEMA: &str = "schema://tradeassembly/broker-lookup@3";
pub const REQUEST_SCHEMA: &str = LOOKUP_SCHEMA;
pub const RESPONSE_SCHEMA: &str = broker_v2::OBSERVATION_SCHEMA;
pub const VERSION: &str = "tradeassembly.broker_lookup.v3";
pub const OPERATION_ID: &str = "broker.order_lookup.paper_v3";
pub const CAPABILITY: &str = "broker.order_lookup.paper@3";
pub const DEPENDENCY_ID: &str = "broker.lookup";
pub const MAX_REQUEST_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperEnvironment {
    Paper,
}
impl<'de> Deserialize<'de> for PaperEnvironment {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mode = String::deserialize(deserializer)?;
        if mode == "paper" {
            Ok(Self::Paper)
        } else {
            Err(serde::de::Error::custom("broker_lookup_v3_paper_required"))
        }
    }
}

/// Exact original account identity, without credentials or connector aliases.
/// These strings follow the host's canonical-account-v1 construction and are
/// never trimmed, case-folded, inferred from an endpoint, or remapped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpectedPaperAccount {
    pub broker_namespace: String,
    pub environment: PaperEnvironment,
    pub provider_account_id: String,
}
impl ExpectedPaperAccount {
    pub fn validate(&self) -> Result<(), String> {
        if !account_label(&self.broker_namespace) || !account_label(&self.provider_account_id) {
            return Err("broker_lookup_v3_account_invalid".into());
        }
        Ok(())
    }
    /// Same identity document as Core broker_submission/account_identity.rs.
    /// Recomputing this digest does not authenticate the observed provider account.
    pub fn canonical_account_sha256(&self) -> Result<String, String> {
        self.validate()?;
        canonical_sha256(&json!({
            "version":"tradeassembly.canonical_account.v1",
            "brokerNamespace":self.broker_namespace,
            "environment":self.environment,
            "providerAccountId":self.provider_account_id,
        }))
        .map_err(|_| "broker_lookup_v3_account_invalid".into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LookupRequestV3 {
    pub schema_version: String,
    /// Exact original immutable V2 submission, not a new or amended order.
    pub original_submit: broker_v2::SubmitRequest,
    pub expected_account: ExpectedPaperAccount,
}
impl LookupRequestV3 {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION {
            return Err("broker_lookup_v3_request_invalid".into());
        }
        self.original_submit.validate()?;
        if self.original_submit.canonical_account_sha256
            != self.expected_account.canonical_account_sha256()?
        {
            return Err("broker_lookup_v3_account_mismatch".into());
        }
        Ok(())
    }
    /// Host-side structural and exact-echo validation. This cannot prove that a
    /// provider actually compared its observations, or establish a cash posting.
    pub fn validate_observation(
        &self,
        observation: &broker_v2::OrderObservation,
        now: i64,
    ) -> Result<(), String> {
        self.validate()?;
        if now <= 0 || now > crate::canonical::MAX_EXACT_INTEGER as i64 {
            return Err("broker_lookup_v3_time_invalid".into());
        }
        observation.validate(now)?;
        if observation.order != self.original_submit.order
            || observation.canonical_account_sha256 != self.original_submit.canonical_account_sha256
        {
            return Err("broker_lookup_v3_observation_binding_mismatch".into());
        }
        Ok(())
    }
}

pub fn decode_lookup_json(bytes: &[u8]) -> Result<LookupRequestV3, String> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("broker_lookup_v3_request_too_large".into());
    }
    let request: LookupRequestV3 =
        serde_json::from_slice(bytes).map_err(|_| "broker_lookup_v3_request_invalid")?;
    request.validate()?;
    Ok(request)
}

fn account_label(value: &str) -> bool {
    // Match Core's identity text checks exactly, preserving identity bytes.
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
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
fn account_label_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256,"pattern":"^[^\\u0000-\\u001F\\u007F-\\u009F]+$"})
}
pub fn request_schema() -> Value {
    object(json!({
        "schemaVersion":{"const":VERSION},
        "originalSubmit":broker_v2::submit_schema(),
        "expectedAccount":object(json!({
            "brokerNamespace":account_label_schema(),
            "environment":{"const":"paper"},
            "providerAccountId":account_label_schema(),
        })),
    }))
}
pub fn schema() -> Value {
    request_schema()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{broker_v2::OrderStatus, execution_snapshot::Observed};

    fn request() -> LookupRequestV3 {
        serde_json::from_value(json!({
            "schemaVersion":VERSION,
            "originalSubmit":{
                "schemaVersion":"tradeassembly.broker_submit.v2",
                "planId":format!("orderplan_{}","a".repeat(64)),
                "canonicalAccountSha256":"794362c12a3edb69c64643c583b7fe562f0db4fbf1a3028809fd56f8fdd5d7ad",
                "order":{
                    "schemaVersion":"tradeassembly.broker_order.v2",
                    "instrument":{"instrumentId":"host.opaque.one","providerSymbol":"FIX",
                        "assetClass":"equity","instrumentFamily":"equity","quantityUnit":"shares",
                        "multiplier":"1","quoteCurrency":"USD"},
                    "positionAction":"buy_to_open","quantity":"0.123456789012",
                    "pricing":{"kind":"limit","limitPrice":"12.000000000001"},
                    "timeInForce":"day","clientOrderId":"fixed.original.client",
                },
            },
            "expectedAccount":{"brokerNamespace":"fixture.broker","environment":"paper",
                "providerAccountId":"fixture-account"},
        })).unwrap()
    }
    fn decode(value: &Value) -> Result<LookupRequestV3, String> {
        decode_lookup_json(&serde_json::to_vec(value).unwrap())
    }
    fn unknown(request: &LookupRequestV3) -> broker_v2::OrderObservation {
        broker_v2::OrderObservation {
            schema_version: "tradeassembly.broker_order_observation.v2".into(),
            canonical_account_sha256: request.original_submit.canonical_account_sha256.clone(),
            order: request.original_submit.order.clone(),
            provider_order_id: None,
            status: OrderStatus::Unknown,
            filled_quantity: Observed::Unavailable {
                reason: "not_observed".into(),
            },
            cash_spent: Observed::Unavailable {
                reason: "not_observed".into(),
            },
            observed_at_ms: 1000,
            known_at_ms: 1001,
        }
    }
    #[test]
    fn lookup_v3_recomputes_exact_host_identity_and_preserves_decimal_order() {
        let request = request();
        request.validate().unwrap();
        assert_eq!(
            request.expected_account.canonical_account_sha256().unwrap(),
            "794362c12a3edb69c64643c583b7fe562f0db4fbf1a3028809fd56f8fdd5d7ad"
        );
        let bytes = serde_json::to_vec(&request).unwrap();
        assert_eq!(decode_lookup_json(&bytes).unwrap(), request);
        assert_eq!(
            request.original_submit.order.quantity.to_string(),
            "0.123456789012"
        );
        assert_eq!(
            serde_json::to_value(&request).unwrap()["originalSubmit"]["order"]["pricing"]
                ["limitPrice"],
            "12.000000000001"
        );
        for field in ["brokerNamespace", "providerAccountId"] {
            let mut drifted = serde_json::to_value(&request).unwrap();
            drifted["expectedAccount"][field] = json!("different");
            assert_eq!(
                decode(&drifted).unwrap_err(),
                "broker_lookup_v3_account_mismatch"
            );
        }
        let mut exact = request.expected_account.clone();
        exact.provider_account_id.push(' ');
        assert_ne!(
            exact.canonical_account_sha256().unwrap(),
            request.original_submit.canonical_account_sha256
        );
    }
    #[test]
    fn lookup_v3_is_closed_at_every_level_and_paper_only() {
        let original = serde_json::to_value(request()).unwrap();
        for pointer in [
            "",
            "/originalSubmit",
            "/originalSubmit/order",
            "/originalSubmit/order/instrument",
            "/expectedAccount",
        ] {
            let mut invalid = original.clone();
            invalid
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), Value::Null);
            assert!(decode(&invalid).is_err());
        }
        for pointer in [
            "/schemaVersion",
            "/originalSubmit",
            "/expectedAccount",
            "/expectedAccount/brokerNamespace",
            "/expectedAccount/providerAccountId",
            "/expectedAccount/environment",
        ] {
            let mut invalid = original.clone();
            *invalid.pointer_mut(pointer).unwrap() = Value::Null;
            assert!(decode(&invalid).is_err());
        }
        for mode in [
            json!("live"),
            json!("backtest"),
            json!("research"),
            json!("Paper"),
            json!({"paper":null}),
            json!(true),
            json!(1),
        ] {
            let mut invalid = original.clone();
            invalid["expectedAccount"]["environment"] = mode;
            assert!(decode(&invalid).is_err());
        }
        let mut v2 = original;
        v2["schemaVersion"] = json!("tradeassembly.broker_lookup.v2");
        assert!(decode(&v2).is_err());
        assert!(
            decode(&json!({"schemaVersion":"tradeassembly.broker_lookup.v2",
            "canonicalAccountSha256":"b".repeat(64),"clientOrderId":"fixed.original.client"}))
            .is_err()
        );
    }
    #[test]
    fn lookup_v3_reuses_strict_v2_limit_and_decimal_validation_without_changing_v2_bytes() {
        let request = request();
        let old_bytes = serde_json::to_vec(&request.original_submit).unwrap();
        let old = broker_v2::decode_submit_json(&old_bytes).unwrap();
        assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
        let original = serde_json::to_value(&request).unwrap();
        for (pointer, value) in [
            (
                "/originalSubmit/schemaVersion",
                json!("tradeassembly.broker_submit.v3"),
            ),
            ("/originalSubmit/planId", json!("a".repeat(64))),
            ("/originalSubmit/order/quantity", json!(0.123456789012)),
            ("/originalSubmit/order/quantity", json!("0.1234567890123")),
            ("/originalSubmit/order/quantity", json!("0")),
            ("/originalSubmit/order/pricing", json!({"kind":"market"})),
            (
                "/originalSubmit/order/clientOrderId",
                json!("x".repeat(129)),
            ),
        ] {
            let mut invalid = original.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(decode(&invalid).is_err());
        }
        assert!(broker_v2::decode_submit_json(&serde_json::to_vec(&request).unwrap()).is_err());
    }
    #[test]
    fn lookup_v3_bounds_identity_text_and_decoder_before_use() {
        let original = serde_json::to_value(request()).unwrap();
        for field in ["brokerNamespace", "providerAccountId"] {
            for value in [
                "".to_owned(),
                "x".repeat(257),
                "unsafe\nvalue".into(),
                "unsafe\u{0085}value".into(),
            ] {
                let mut invalid = original.clone();
                invalid["expectedAccount"][field] = json!(value);
                assert!(decode(&invalid).is_err());
            }
        }
        let mut boundary = request();
        boundary.expected_account.provider_account_id = "x".repeat(256);
        boundary.original_submit.canonical_account_sha256 = boundary
            .expected_account
            .canonical_account_sha256()
            .unwrap();
        boundary.validate().unwrap();
        let mut bytes = serde_json::to_vec(&request()).unwrap();
        bytes.resize(MAX_REQUEST_BYTES + 1, b' ');
        assert_eq!(
            decode_lookup_json(&bytes).unwrap_err(),
            "broker_lookup_v3_request_too_large"
        );
        let duplicate = format!(
            "{{\"schemaVersion\":\"{VERSION}\",{}",
            String::from_utf8(serde_json::to_vec(&request()).unwrap())
                .unwrap()
                .trim_start_matches('{')
        );
        assert!(decode_lookup_json(duplicate.as_bytes()).is_err());
    }
    #[test]
    fn lookup_v3_response_preserves_unknowns_and_refuses_same_symbol_identity_substitution() {
        let request = request();
        let mut observation = unknown(&request);
        request.validate_observation(&observation, 1001).unwrap();
        assert!(observation.filled_quantity.required().is_err());
        assert!(observation.cash_spent.required().is_err());
        observation.order.instrument.instrument_id = "host.opaque.other".into();
        assert_eq!(
            request
                .validate_observation(&observation, 1001)
                .unwrap_err(),
            "broker_lookup_v3_observation_binding_mismatch"
        );
        observation = unknown(&request);
        observation.canonical_account_sha256 = "c".repeat(64);
        assert_eq!(
            request
                .validate_observation(&observation, 1001)
                .unwrap_err(),
            "broker_lookup_v3_observation_binding_mismatch"
        );
        observation = unknown(&request);
        observation.status = OrderStatus::Filled;
        assert!(request.validate_observation(&observation, 1001).is_err());
        assert!(request
            .validate_observation(&unknown(&request), 1000)
            .is_err());
        assert!(request
            .validate_observation(
                &unknown(&request),
                crate::canonical::MAX_EXACT_INTEGER as i64 + 1
            )
            .is_err());
    }
    #[test]
    fn lookup_v3_schema_is_closed_and_keeps_nested_submit_contract_identical() {
        let document = request_schema();
        assert_eq!(schema(), document);
        assert_eq!(document["additionalProperties"], false);
        assert_eq!(document["properties"]["schemaVersion"]["const"], VERSION);
        assert_eq!(
            document["properties"]["originalSubmit"],
            broker_v2::submit_schema()
        );
        assert_eq!(
            document["properties"]["expectedAccount"]["additionalProperties"],
            false
        );
        assert_eq!(
            document["properties"]["expectedAccount"]["properties"]["environment"]["const"],
            "paper"
        );
        assert_eq!(document["required"].as_array().unwrap().len(), 3);
        assert_eq!(
            document["properties"]["expectedAccount"]["required"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
}
