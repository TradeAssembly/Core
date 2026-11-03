// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! The owning control-plane projection, shared with installer provenance checks.
use serde_json::{Map, Value};

const SENSITIVE_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "api_secret",
    "authorization",
    "bearer",
    "client_secret",
    "code_verifier",
    "password",
    "private_key",
    "refresh_token",
    "secret",
    "token",
];

pub(crate) fn redact_sensitive_payload(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) {
                        (key, Value::String("[REDACTED]".to_string()))
                    } else {
                        (key, redact_sensitive_payload(value))
                    }
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_sensitive_payload).collect())
        }
        Value::String(text) if is_sensitive_text(&text) => Value::String("[REDACTED]".to_string()),
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.replace('-', "_").to_ascii_lowercase();
    let compact = normalized.replace('_', "");
    SENSITIVE_KEYS.contains(&normalized.as_str())
        || normalized.ends_with("_token")
        || normalized.ends_with("_secret")
        || compact.contains("token")
        || compact.contains("secret")
        || compact == "apikey"
        || compact == "privatekey"
        || compact == "authorizationurl"
        || compact == "handoffverifier"
        || compact == "verifier"
}

pub(crate) fn is_sensitive_text(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("api_secret")
        || normalized.contains("apisecret")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("access_key")
        || normalized.contains("private_key")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("bearer ")
}
