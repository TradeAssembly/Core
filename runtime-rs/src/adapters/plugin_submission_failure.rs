// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Diagnostic evidence never authorizes a retry or replaces a broker receipt.
use crate::ports::{SideEffectContext, StoragePort};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const NAMESPACE: &str = "plugin_broker_submission_failures";
const SCHEMA: &str = "tradeassembly.plugin_submission_failure.v1";

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Execution,
    Completion,
    Persistence,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Code {
    PayloadSchemaMismatch,
    SchemaIdentityMismatch,
    ProcessTimeout,
    InvalidResponse,
    ProviderOrderAbsent,
    ProviderTimeout,
    ProviderRejected,
    ProviderNetworkError,
    OrderIdentityMismatch,
    CorrelationMismatch,
    CompletionRejected,
    ReceiptPersistenceFailed,
    Unknown,
}

fn classify(phase: Phase, error: &str) -> Code {
    match phase {
        Phase::Completion => match error {
            "broker_order_observation_mismatch" => Code::OrderIdentityMismatch,
            "broker_submission_response_correlation_mismatch" => Code::CorrelationMismatch,
            _ => Code::CompletionRejected,
        },
        Phase::Persistence => Code::ReceiptPersistenceFailed,
        Phase::Execution => match error {
            "plugin_response_payload_schema_mismatch" => Code::PayloadSchemaMismatch,
            "plugin_response_schema_digest_mismatch" => Code::SchemaIdentityMismatch,
            "plugin_process_timeout" => Code::ProcessTimeout,
            "plugin_response_invalid" => Code::InvalidResponse,
            "plugin_operation_failed:provider_order_absent" => Code::ProviderOrderAbsent,
            "plugin_operation_failed:provider_timeout" => Code::ProviderTimeout,
            "plugin_operation_failed:provider_rejected" => Code::ProviderRejected,
            "plugin_operation_failed:provider_network_error" => Code::ProviderNetworkError,
            _ => Code::Unknown,
        },
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Failure {
    schema_version: String,
    request_hash: String,
    phase: Phase,
    code: Code,
}

pub(crate) fn record(
    storage: &dyn StoragePort,
    key: &str,
    request_hash: &str,
    phase: Phase,
    error: &str,
    context: &SideEffectContext,
) {
    let record = Failure {
        schema_version: SCHEMA.into(),
        request_hash: request_hash.into(),
        phase,
        code: classify(phase, error),
    };
    // If diagnostic storage also fails, the caller still returns the original
    // reconciliation-required result. Never allow another dispatch here.
    if let Ok(value) = serde_json::to_value(record) {
        let _ = storage.put_json_if_absent(NAMESPACE, key, value, context);
    }
}

pub(crate) fn read(storage: &dyn StoragePort, key: &str) -> Option<Value> {
    let value = storage.get_json(NAMESPACE, key).ok()??;
    let failure: Failure = serde_json::from_value(value).ok()?;
    let request = storage.get_json("plugin_operation_requests", key).ok()??;
    if failure.schema_version != SCHEMA
        || request["requestHash"].as_str() != Some(failure.request_hash.as_str())
    {
        return None;
    }
    serde_json::to_value(failure).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_error_and_unknown_provider_codes_never_enter_evidence() {
        for error in [
            "credential-canary",
            "plugin_operation_failed:credential-canary",
            "plugin_operation_failed:provider_order_absent credential-canary",
        ] {
            assert_eq!(classify(Phase::Execution, error), Code::Unknown);
            let value = serde_json::to_string(&classify(Phase::Persistence, error)).unwrap();
            assert_eq!(value, "\"receipt_persistence_failed\"");
        }
        assert_eq!(
            classify(
                Phase::Execution,
                "plugin_operation_failed:provider_order_absent"
            ),
            Code::ProviderOrderAbsent
        );
    }

    #[test]
    fn reader_rejects_unknown_fields_codes_and_wrong_request_binding() {
        use crate::ports::{AuthorityContext, IdempotencyKey};
        use crate::service::TradeAssemblyService;
        use serde_json::json;
        let temp = tempfile::tempdir().unwrap();
        let service = TradeAssemblyService::test_local(
            temp.path()
                .join("state.sqlite")
                .to_string_lossy()
                .into_owned(),
        );
        let storage = service.runtime().storage.clone();
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("diagnostic-test").unwrap(),
        );
        let value = json!({"schemaVersion": SCHEMA, "requestHash": "bound",
            "phase": "execution", "code": "process_timeout"});
        storage
            .put_json(
                "plugin_operation_requests",
                "key",
                json!({"requestHash":"bound"}),
                &context,
            )
            .unwrap();
        for patch in [
            json!({"code":"credential-canary"}),
            json!({"extra":"credential-canary"}),
            json!({"requestHash":"other"}),
        ] {
            let mut changed = value.clone();
            changed
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            storage
                .put_json(NAMESPACE, "key", changed, &context)
                .unwrap();
            assert!(read(storage.as_ref(), "key").is_none());
        }
        storage
            .put_json(NAMESPACE, "key", value.clone(), &context)
            .unwrap();
        assert_eq!(read(storage.as_ref(), "key"), Some(value));
    }
}
