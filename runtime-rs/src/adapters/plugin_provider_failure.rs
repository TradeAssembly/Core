// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Optional provider diagnostics. These records never authorize execution.
use crate::ports::{SideEffectContext, StoragePort};
use serde::{Deserialize, Serialize};
use tradeassembly_plugin_sdk::PluginResponse;

const SEPARATOR: &str = "|provider|";
const NAMESPACE: &str = "plugin_broker_provider_failures";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Cause {
    InsufficientBuyingPower,
    CryptoNotEnabled,
    AccountNotAuthorized,
    AccountBlocked,
    RegionRestricted,
    FractionalNotAllowed,
    WashTrade,
    OrderLimit,
    InvalidOrder,
    Unknown,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Detail {
    http_status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_code: Option<u32>,
    cause: Cause,
}

fn valid_primary(primary: &str) -> bool {
    matches!(
        primary,
        "provider_auth_failed"
            | "provider_request_failed"
            | "provider_order_absent"
            | "provider_timeout"
            | "provider_network_error"
    )
}

fn canonical_number(value: &str, max: u32) -> Option<u32> {
    let number: u32 = value.parse().ok()?;
    (number <= max && number.to_string() == value).then_some(number)
}

fn from_wire(response: &PluginResponse) -> Option<Detail> {
    let mut status = None;
    let mut code = None;
    let mut cause = None;
    for diagnostic in &response.redacted_diagnostics {
        // Never read messages: even apparently harmless provider text is private.
        if let Some(value) = diagnostic.code.strip_prefix("provider_http_") {
            if status.is_some() {
                return None;
            }
            let number = canonical_number(value, 599)? as u16;
            if number < 100 {
                return None;
            }
            status = Some(number);
        } else if let Some(value) = diagnostic.code.strip_prefix("provider_code_") {
            if code.is_some() {
                return None;
            }
            code = Some(canonical_number(value, 999_999_999)?);
        } else if let Some(value) = diagnostic.code.strip_prefix("provider_cause_") {
            if cause.is_some() {
                return None;
            }
            cause = Some(serde_json::from_value(serde_json::Value::String(value.into())).ok()?);
        }
    }
    Some(Detail {
        http_status: status?,
        provider_code: code,
        cause: cause?,
    })
}

pub(crate) fn wire_error(response: &PluginResponse) -> String {
    let primary = &response.provider_outcome.code;
    let base = format!("plugin_operation_failed:{primary}");
    if !valid_primary(primary) {
        return base;
    }
    match from_wire(response).and_then(|detail| serde_json::to_string(&detail).ok()) {
        Some(detail) => format!("{base}{SEPARATOR}{detail}"),
        None => base,
    }
}

fn parse(error: &str) -> Option<(&str, Detail)> {
    let (base, raw) = error.split_once(SEPARATOR)?;
    if raw.len() > 200 || !valid_primary(base.strip_prefix("plugin_operation_failed:")?) {
        return None;
    }
    let detail: Detail = serde_json::from_str(raw).ok()?;
    if !(100..=599).contains(&detail.http_status)
        || detail.provider_code.is_some_and(|n| n > 999_999_999)
    {
        return None;
    }
    Some((base, detail))
}

pub(crate) fn base_error(error: &str) -> &str {
    parse(error).map(|(base, _)| base).unwrap_or(error)
}

pub(crate) fn record(
    storage: &dyn StoragePort,
    key: &str,
    request_hash: &str,
    error: &str,
    context: &SideEffectContext,
) {
    let Some((base, detail)) = parse(error) else {
        return;
    };
    let value = serde_json::json!({"schemaVersion":"tradeassembly.plugin_provider_failure.v1", "requestHash":request_hash,
        "code":base.strip_prefix("plugin_operation_failed:").unwrap(), "detail":detail});
    let _ = storage.put_json_if_absent(NAMESPACE, key, value, context);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tradeassembly_plugin_sdk::{
        ProviderOutcome, Reconciliation, RedactedDiagnostic, ResponseStatus,
    };
    fn response(codes: &[&str]) -> PluginResponse {
        PluginResponse {
            schema_version: "1".into(),
            request_id: "test".into(),
            status: ResponseStatus::Failed,
            response_schema: "test".into(),
            payload: serde_json::json!({}),
            provider_outcome: ProviderOutcome {
                code: "provider_auth_failed".into(),
                provider_request_id: None,
                provider_reference: None,
            },
            reconciliation: Reconciliation::NotRequired,
            evidence_references: vec![],
            redacted_diagnostics: codes
                .iter()
                .map(|code| RedactedDiagnostic {
                    code: (*code).into(),
                    message: "private-secret-canary".into(),
                })
                .collect(),
        }
    }
    #[test]
    fn safe_provider_fields_survive_without_messages_or_primary_change() {
        let wire = response(&[
            "provider_http_403",
            "provider_code_40310000",
            "provider_cause_crypto_not_enabled",
        ]);
        let error = wire_error(&wire);
        assert!(!error.contains("canary"));
        assert_eq!(
            base_error(&error),
            "plugin_operation_failed:provider_auth_failed"
        );
        let (_, detail) = parse(&error).unwrap();
        assert_eq!(detail.http_status, 403);
        assert_eq!(detail.provider_code, Some(40310000));
        assert_eq!(detail.cause, Cause::CryptoNotEnabled);
    }
    #[test]
    fn malformed_duplicate_unknown_and_unbounded_fields_are_not_retained() {
        for codes in [
            vec!["provider_http_403", "provider_cause_secret-canary"],
            vec![
                "provider_http_403",
                "provider_http_401",
                "provider_cause_unknown",
            ],
            vec![
                "provider_http_403",
                "provider_code_1000000000",
                "provider_cause_unknown",
            ],
            vec!["provider_http_0403", "provider_cause_unknown"],
            vec![
                "provider_http_403",
                "provider_code_0001",
                "provider_cause_unknown",
            ],
        ] {
            assert_eq!(
                wire_error(&response(&codes)),
                "plugin_operation_failed:provider_auth_failed"
            );
        }
        for suffix in [
            r#"{"httpStatus":403,"cause":"unknown","secret":"canary"}"#,
            r#"{"httpStatus":999,"cause":"unknown"}"#,
            r#"{"httpStatus":403,"cause":"unknown","providerCode":1000000000}"#,
        ] {
            let input = format!("plugin_operation_failed:provider_auth_failed{SEPARATOR}{suffix}");
            assert!(parse(&input).is_none());
            assert_eq!(base_error(&input), input);
        }
    }
}
