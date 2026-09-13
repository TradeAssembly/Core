// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Deterministic, lexical policy checks for external plugin responses.
//!
//! This module deliberately makes no semantic or model-based classification
//! claims. Callers should invoke [`validate_plugin_response_policy`] after
//! decoding a plugin response and before projecting it into host records.

use serde_json::Value;
use std::collections::BTreeMap;
use tradeassembly_plugin_sdk::{PluginRequest, PluginResponse, DEFAULT_MAX_ENVELOPE_BYTES};

const MAX_JSON_DEPTH: usize = 32;
const MAX_STRING_BYTES: usize = 16 * 1024;
const MAX_ARRAY_ITEMS: usize = 1_024;
const MAX_OBJECT_FIELDS: usize = 256;
const MAX_JSON_NODES: usize = 8_192;
const MAX_DIAGNOSTIC_MESSAGE_BYTES: usize = 512;
const MAX_IDENTIFIER_BYTES: usize = 128;

/// Validates a decoded external plugin response using stable, host-safe errors.
pub fn validate_plugin_response_policy(
    request: &PluginRequest,
    response: &PluginResponse,
    operation: &Value,
    credential_values: &BTreeMap<String, String>,
) -> Result<(), String> {
    validate_wire_binding(request, response)?;
    validate_response_schema(response, operation)?;
    validate_response_envelope(response, credential_values)?;
    validate_evidence_references(request, response)?;
    validate_provider_outcome(response)?;
    validate_redacted_diagnostics(response)?;
    validate_payload(&response.payload)?;

    if operation.get("effect").and_then(Value::as_str) == Some("export") {
        validate_export_payload(&response.payload)?;
    }

    Ok(())
}

fn validate_wire_binding(request: &PluginRequest, response: &PluginResponse) -> Result<(), String> {
    if request.request_id.is_empty()
        || request.context.idempotency_key.is_empty()
        || request.request_id != request.context.idempotency_key
        || response.schema_version != request.schema_version
        || response.request_id != request.request_id
    {
        return Err("plugin_response_binding_mismatch".to_string());
    }
    Ok(())
}

fn validate_response_schema(response: &PluginResponse, operation: &Value) -> Result<(), String> {
    let declared = operation["traits"]["outputSchemaRefs"]
        .as_array()
        .is_some_and(|schemas| {
            schemas
                .iter()
                .any(|schema| schema.as_str() == Some(response.response_schema.as_str()))
        });
    if !declared {
        return Err("plugin_response_schema_undeclared".to_string());
    }
    Ok(())
}

fn validate_response_envelope(
    response: &PluginResponse,
    credential_values: &BTreeMap<String, String>,
) -> Result<(), String> {
    let serialized =
        serde_json::to_vec(response).map_err(|_| "plugin_response_invalid".to_string())?;
    if serialized.len() >= DEFAULT_MAX_ENVELOPE_BYTES {
        return Err("plugin_response_envelope_too_large".to_string());
    }
    if credential_values
        .values()
        .filter(|secret| !secret.is_empty())
        .any(|secret| {
            let needle = serialized_string_contents(secret);
            serialized
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        })
    {
        return Err("plugin_response_secret_leak".to_string());
    }
    Ok(())
}

fn serialized_string_contents(value: &str) -> String {
    let encoded = serde_json::to_string(value).expect("strings serialize");
    encoded[1..encoded.len() - 1].to_string()
}

fn validate_evidence_references(
    request: &PluginRequest,
    response: &PluginResponse,
) -> Result<(), String> {
    if response
        .evidence_references
        .iter()
        .any(|reference| !request.context.evidence_references.contains(reference))
    {
        return Err("plugin_response_evidence_unbound".to_string());
    }
    Ok(())
}

fn validate_provider_outcome(response: &PluginResponse) -> Result<(), String> {
    let outcome = &response.provider_outcome;
    if !is_safe_identifier(&outcome.code)
        || outcome
            .provider_request_id
            .as_deref()
            .is_some_and(|value| !is_safe_identifier(value))
        || outcome
            .provider_reference
            .as_deref()
            .is_some_and(|value| !is_safe_identifier(value))
    {
        return Err("plugin_response_provider_outcome_invalid".to_string());
    }
    Ok(())
}

fn validate_redacted_diagnostics(response: &PluginResponse) -> Result<(), String> {
    for diagnostic in &response.redacted_diagnostics {
        if !is_safe_identifier(&diagnostic.code)
            || diagnostic.message.is_empty()
            || diagnostic.message.len() > MAX_DIAGNOSTIC_MESSAGE_BYTES
            || contains_control_character(&diagnostic.message)
        {
            return Err("plugin_response_diagnostic_invalid".to_string());
        }
        if contains_advice_phrase(&diagnostic.message) {
            return Err("plugin_response_advice_forbidden".to_string());
        }
    }
    Ok(())
}

fn validate_payload(payload: &Value) -> Result<(), String> {
    let mut budget = JsonBudget::default();
    validate_json_value(payload, 1, &mut budget)
}

fn validate_json_value(value: &Value, depth: usize, budget: &mut JsonBudget) -> Result<(), String> {
    if depth > MAX_JSON_DEPTH {
        return Err("plugin_response_json_depth_exceeded".to_string());
    }
    budget.nodes += 1;
    if budget.nodes > MAX_JSON_NODES {
        return Err("plugin_response_json_node_limit_exceeded".to_string());
    }
    match value {
        Value::String(text) => {
            if text.len() > MAX_STRING_BYTES {
                return Err("plugin_response_string_too_large".to_string());
            }
            if contains_advice_phrase(text) {
                return Err("plugin_response_advice_forbidden".to_string());
            }
        }
        Value::Array(values) => {
            if values.len() > MAX_ARRAY_ITEMS {
                return Err("plugin_response_array_limit_exceeded".to_string());
            }
            for child in values {
                validate_json_value(child, depth + 1, budget)?;
            }
        }
        Value::Object(fields) => {
            if fields.len() > MAX_OBJECT_FIELDS {
                return Err("plugin_response_object_limit_exceeded".to_string());
            }
            for (key, child) in fields {
                if is_secret_shaped_key(key) {
                    return Err("plugin_response_secret_field_forbidden".to_string());
                }
                validate_json_value(child, depth + 1, budget)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn validate_export_payload(payload: &Value) -> Result<(), String> {
    match payload {
        Value::String(value) => validate_export_string(value),
        Value::Array(values) => {
            for value in values {
                validate_export_payload(value)?;
            }
            Ok(())
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                if is_unsafe_export_key(key) {
                    return Err("plugin_response_export_field_forbidden".to_string());
                }
                validate_export_payload(value)?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

fn validate_export_string(value: &str) -> Result<(), String> {
    if contains_control_character(value) {
        return Err("plugin_response_export_control_character".to_string());
    }
    let trimmed = value.trim_start();
    if matches!(trimmed.chars().next(), Some('=' | '+' | '-' | '@')) {
        return Err("plugin_response_export_formula_forbidden".to_string());
    }
    if is_unsafe_path(value) {
        return Err("plugin_response_export_path_forbidden".to_string());
    }
    Ok(())
}

#[derive(Default)]
struct JsonBudget {
    nodes: usize,
}

fn is_secret_shaped_key(key: &str) -> bool {
    matches!(
        normalized_key(key).as_str(),
        "apikey"
            | "apisecret"
            | "accesstoken"
            | "authorization"
            | "bearertoken"
            | "clientsecret"
            | "credential"
            | "credentials"
            | "password"
            | "privatekey"
            | "refreshtoken"
            | "secret"
            | "token"
    )
}

fn is_unsafe_export_key(key: &str) -> bool {
    matches!(
        normalized_key(key).as_str(),
        "destination"
            | "directory"
            | "filepath"
            | "filename"
            | "html"
            | "markup"
            | "outputfile"
            | "outputpath"
            | "path"
            | "template"
    )
}

fn normalized_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn is_safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn contains_control_character(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn is_unsafe_path(value: &str) -> bool {
    let value = value.trim();
    value.starts_with('/')
        || value.starts_with('\\')
        || value.starts_with("..")
        || value.contains("../")
        || value.contains("..\\")
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
}

/// Returns true only for explicit lexical advice constructions.
fn contains_advice_phrase(value: &str) -> bool {
    let tokens: Vec<_> = value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if !is_trade_action(token) {
            continue;
        }
        let start = index.saturating_sub(3);
        let prior = &tokens[start..index];
        if prior
            .iter()
            .any(|word| matches!(word.as_str(), "should" | "must" | "need"))
            || prior.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "recommend" | "recommended" | "suggest" | "advised"
                )
            }) && !prior
                .iter()
                .any(|word| matches!(word.as_str(), "not" | "never"))
            || tokens
                .get(index + 1)
                .is_some_and(|word| matches!(word.as_str(), "now" | "immediately"))
            || index >= 2 && tokens[index - 2] == "time" && tokens[index - 1] == "to"
        {
            return true;
        }
    }
    false
}

fn is_trade_action(word: &str) -> bool {
    word.starts_with("purchas")
        || matches!(
            word,
            "buy" | "buying" | "long" | "sell" | "selling" | "short" | "trade" | "trading"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tradeassembly_plugin_sdk::PluginResponse;

    fn request() -> PluginRequest {
        serde_json::from_value(json!({
            "schemaVersion": "1", "hostContractVersion": "1", "sdkVersion": "0.1.0",
            "requestId": "request-1",
            "metadata": {"operationId": "report.export", "pluginId": "test", "pluginInstanceId": "test-1", "packageDigest": "package", "manifestDigest": "manifest", "capabilityBindingId": "binding", "capabilityFingerprint": "fingerprint"},
            "context": {"authorityId": "user", "purpose": "report", "mode": "paper", "fencingToken": "none", "timeoutMs": 1000, "idempotencyKey": "request-1", "evidenceReferences": ["evidence-1"]},
            "payload": {"kind": "operation", "payload": {}}
        })).expect("request fixture")
    }

    fn response(payload: Value) -> PluginResponse {
        serde_json::from_value(json!({
            "schemaVersion": "1", "requestId": "request-1", "status": "succeeded",
            "responseSchema": "schema://test/report@1", "payload": payload,
            "providerOutcome": {"code": "ok"}, "reconciliation": "not_required",
            "evidenceReferences": ["evidence-1"]
        }))
        .expect("response fixture")
    }

    fn operation(effect: &str) -> Value {
        json!({"effect": effect, "traits": {"outputSchemaRefs": ["schema://test/report@1"]}})
    }

    fn validate(payload: Value) -> Result<(), String> {
        validate_plugin_response_policy(
            &request(),
            &response(payload),
            &operation("read"),
            &BTreeMap::new(),
        )
    }

    #[test]
    fn accepts_factual_status_and_negated_recommendation_language() {
        assert_eq!(
            validate(json!({"status": "provider did not recommend buying; quote unavailable"})),
            Ok(())
        );
    }

    #[test]
    fn rejects_binding_schema_and_unbound_evidence() {
        let mut invalid = response(json!({"status": "ok"}));
        invalid.request_id = "other".to_string();
        assert_eq!(
            validate_plugin_response_policy(
                &request(),
                &invalid,
                &operation("read"),
                &BTreeMap::new()
            ),
            Err("plugin_response_binding_mismatch".to_string())
        );

        let mut invalid = response(json!({"status": "ok"}));
        invalid.response_schema = "schema://test/other@1".to_string();
        assert_eq!(
            validate_plugin_response_policy(
                &request(),
                &invalid,
                &operation("read"),
                &BTreeMap::new()
            ),
            Err("plugin_response_schema_undeclared".to_string())
        );

        let mut invalid = response(json!({"status": "ok"}));
        invalid.evidence_references = vec!["outside-request".to_string()];
        assert_eq!(
            validate_plugin_response_policy(
                &request(),
                &invalid,
                &operation("read"),
                &BTreeMap::new()
            ),
            Err("plugin_response_evidence_unbound".to_string())
        );
    }

    #[test]
    fn rejects_secret_echoes_and_nested_secret_keys() {
        let credentials =
            BTreeMap::from([("api_secret".to_string(), "only-once-secret".to_string())]);
        assert_eq!(
            validate_plugin_response_policy(
                &request(),
                &response(json!({"note": "only-once-secret"})),
                &operation("read"),
                &credentials
            ),
            Err("plugin_response_secret_leak".to_string())
        );
        assert_eq!(
            validate(json!({"outer": [{"refresh-token": "redacted"}]})),
            Err("plugin_response_secret_field_forbidden".to_string())
        );
    }

    #[test]
    fn rejects_explicit_advice_in_payload_and_diagnostics() {
        assert_eq!(
            validate(json!({"message": "you should buy now"})),
            Err("plugin_response_advice_forbidden".to_string())
        );
        for suffix in ["e", "ing"] {
            assert_eq!(
                validate(json!({"message": format!("you should purchas{suffix} SPY")})),
                Err("plugin_response_advice_forbidden".to_string())
            );
        }
        let mut invalid = response(json!({"status": "ok"}));
        invalid.redacted_diagnostics = serde_json::from_value(
            json!([{"code": "provider_notice", "message": "recommend selling now"}]),
        )
        .expect("diagnostic fixture");
        assert_eq!(
            validate_plugin_response_policy(
                &request(),
                &invalid,
                &operation("read"),
                &BTreeMap::new()
            ),
            Err("plugin_response_advice_forbidden".to_string())
        );
    }

    #[test]
    fn rejects_oversized_and_invalid_json_shapes() {
        assert_eq!(
            validate(json!({"value": "x".repeat(MAX_STRING_BYTES + 1)})),
            Err("plugin_response_string_too_large".to_string())
        );
        assert_eq!(
            validate(Value::Array(
                (0..=MAX_ARRAY_ITEMS).map(|_| json!(0)).collect()
            )),
            Err("plugin_response_array_limit_exceeded".to_string())
        );
        assert_eq!(
            validate(Value::Object(
                (0..=MAX_OBJECT_FIELDS)
                    .map(|index| (index.to_string(), json!(0)))
                    .collect()
            )),
            Err("plugin_response_object_limit_exceeded".to_string())
        );
    }

    #[test]
    fn rejects_invalid_provider_and_diagnostic_fields() {
        let mut invalid = response(json!({"status": "ok"}));
        invalid.provider_outcome.code = "bad code".to_string();
        assert_eq!(
            validate_plugin_response_policy(
                &request(),
                &invalid,
                &operation("read"),
                &BTreeMap::new()
            ),
            Err("plugin_response_provider_outcome_invalid".to_string())
        );

        let mut invalid = response(json!({"status": "ok"}));
        invalid.redacted_diagnostics =
            serde_json::from_value(json!([{"code": "notice", "message": "line one\nline two"}]))
                .expect("diagnostic fixture");
        assert_eq!(
            validate_plugin_response_policy(
                &request(),
                &invalid,
                &operation("read"),
                &BTreeMap::new()
            ),
            Err("plugin_response_diagnostic_invalid".to_string())
        );
    }

    #[test]
    fn export_rejects_formula_controls_paths_and_plugin_selected_artifacts() {
        for payload in [
            json!({"value": "=SUM(A1:A2)"}),
            json!({"value": "bad\u{0007}"}),
            json!({"value": "../report.csv"}),
            json!({"filename": "report.csv"}),
            json!({"html": "<b>report</b>"}),
        ] {
            assert!(validate_plugin_response_policy(
                &request(),
                &response(payload),
                &operation("export"),
                &BTreeMap::new()
            )
            .is_err());
        }
    }
}
