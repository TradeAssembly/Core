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
    ProcessFailed,
    ProcessTerminated,
    ProcessTransportFailed,
    SandboxFailed,
    InvalidResponse,
    ResponseBindingMismatch,
    ResponseSchemaInvalid,
    ResponsePolicyRejected,
    ProviderOrderAbsent,
    ProviderTimeout,
    ProviderRejected,
    ProviderNetworkError,
    ProviderAuthFailed,
    ProviderRequestFailed,
    ProviderSchemaInvalid,
    ProviderOrderTerminal,
    PluginRequestInvalid,
    PluginConfigurationInvalid,
    PluginCredentialsInvalid,
    PluginClientUnavailable,
    PluginOperationUnsupported,
    AccountBindingMissing,
    OrderIdentityMismatch,
    CorrelationMismatch,
    CompletionRejected,
    ReceiptPersistenceFailed,
    Unknown,
}

/// Closed subcategories preserve actionable distinctions without copying errors.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Detail {
    ResponseBindingMismatch,
    ResponseEvidenceUnbound,
    ResponseSchemaMissing,
    ResponseSchemaInvalid,
    ResponseSchemaUndeclared,
    PackageDescriptorInvalid,
    ResponseEnvelopeTooLarge,
    ResponseSecretLeak,
    ResponseProviderOutcomeInvalid,
    ResponseDiagnosticInvalid,
    ResponseAdviceForbidden,
    ResponseJsonDepthExceeded,
    ResponseJsonNodeLimitExceeded,
    ResponseStringTooLarge,
    ResponseArrayLimitExceeded,
    ResponseObjectLimitExceeded,
    ResponseSecretFieldForbidden,
    ResponseExportFieldForbidden,
    ResponseExportControlCharacter,
    ResponseExportFormulaForbidden,
    ResponseExportPathForbidden,
    InvalidEndpoint,
    PaperModeRequired,
    LiveModeRequired,
    BrokerLookupModeInvalid,
    BrokerLookupEndpointInvalid,
    CredentialGrantMissing,
    CredentialGrantInvalid,
}

impl Detail {
    fn code(self) -> Code {
        match self {
            Self::ResponseBindingMismatch | Self::ResponseEvidenceUnbound => {
                Code::ResponseBindingMismatch
            }
            Self::ResponseSchemaMissing
            | Self::ResponseSchemaInvalid
            | Self::ResponseSchemaUndeclared
            | Self::PackageDescriptorInvalid => Code::ResponseSchemaInvalid,
            Self::InvalidEndpoint
            | Self::PaperModeRequired
            | Self::LiveModeRequired
            | Self::BrokerLookupModeInvalid
            | Self::BrokerLookupEndpointInvalid => Code::PluginConfigurationInvalid,
            Self::CredentialGrantMissing | Self::CredentialGrantInvalid => {
                Code::PluginCredentialsInvalid
            }
            _ => Code::ResponsePolicyRejected,
        }
    }
}

fn classify_detail(phase: Phase, error: &str) -> Option<Detail> {
    if !matches!(phase, Phase::Execution) {
        return None;
    }
    Some(match error {
        "plugin_response_binding_mismatch" => Detail::ResponseBindingMismatch,
        "plugin_response_evidence_unbound" => Detail::ResponseEvidenceUnbound,
        "plugin_response_schema_missing" => Detail::ResponseSchemaMissing,
        "plugin_response_schema_invalid" => Detail::ResponseSchemaInvalid,
        "plugin_response_schema_undeclared" => Detail::ResponseSchemaUndeclared,
        "plugin_package_descriptor_invalid" => Detail::PackageDescriptorInvalid,
        "plugin_response_envelope_too_large" => Detail::ResponseEnvelopeTooLarge,
        "plugin_response_secret_leak" => Detail::ResponseSecretLeak,
        "plugin_response_provider_outcome_invalid" => Detail::ResponseProviderOutcomeInvalid,
        "plugin_response_diagnostic_invalid" => Detail::ResponseDiagnosticInvalid,
        "plugin_response_advice_forbidden" => Detail::ResponseAdviceForbidden,
        "plugin_response_json_depth_exceeded" => Detail::ResponseJsonDepthExceeded,
        "plugin_response_json_node_limit_exceeded" => Detail::ResponseJsonNodeLimitExceeded,
        "plugin_response_string_too_large" => Detail::ResponseStringTooLarge,
        "plugin_response_array_limit_exceeded" => Detail::ResponseArrayLimitExceeded,
        "plugin_response_object_limit_exceeded" => Detail::ResponseObjectLimitExceeded,
        "plugin_response_secret_field_forbidden" => Detail::ResponseSecretFieldForbidden,
        "plugin_response_export_field_forbidden" => Detail::ResponseExportFieldForbidden,
        "plugin_response_export_control_character" => Detail::ResponseExportControlCharacter,
        "plugin_response_export_formula_forbidden" => Detail::ResponseExportFormulaForbidden,
        "plugin_response_export_path_forbidden" => Detail::ResponseExportPathForbidden,
        "plugin_operation_failed:invalid_endpoint" => Detail::InvalidEndpoint,
        "plugin_operation_failed:paper_mode_required" => Detail::PaperModeRequired,
        "plugin_operation_failed:live_mode_required" => Detail::LiveModeRequired,
        "plugin_operation_failed:broker_lookup_mode_invalid" => Detail::BrokerLookupModeInvalid,
        "plugin_operation_failed:broker_lookup_endpoint_invalid" => {
            Detail::BrokerLookupEndpointInvalid
        }
        "plugin_operation_failed:credential_grant_missing" => Detail::CredentialGrantMissing,
        "plugin_operation_failed:credential_grant_invalid" => Detail::CredentialGrantInvalid,
        _ => return None,
    })
}

fn classify(phase: Phase, error: &str) -> Code {
    if let Some(detail) = classify_detail(phase, error) {
        return detail.code();
    }
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
            "plugin_process_failed:terminated" => Code::ProcessTerminated,
            "plugin_request_write_failed"
            | "plugin_process_stdin_unavailable"
            | "plugin_process_stdout_unavailable"
            | "plugin_process_wait_failed" => Code::ProcessTransportFailed,
            "plugin_sandbox_unavailable"
            | "plugin_sandbox_settings_unavailable"
            | "plugin_sandbox_target_invalid"
            | "plugin_sandbox_policy_invalid"
            | "plugin_sandbox_windows_port_range_invalid" => Code::SandboxFailed,
            "plugin_response_invalid" => Code::InvalidResponse,
            "plugin_operation_failed:provider_order_absent" => Code::ProviderOrderAbsent,
            "plugin_operation_failed:provider_timeout" => Code::ProviderTimeout,
            "plugin_operation_failed:provider_rejected" => Code::ProviderRejected,
            "plugin_operation_failed:provider_network_error" => Code::ProviderNetworkError,
            "plugin_operation_failed:provider_auth_failed" => Code::ProviderAuthFailed,
            "plugin_operation_failed:provider_request_failed" => Code::ProviderRequestFailed,
            "plugin_operation_failed:provider_schema_invalid" => Code::ProviderSchemaInvalid,
            "plugin_operation_failed:provider_order_terminal" => Code::ProviderOrderTerminal,
            "plugin_operation_failed:invalid_request" => Code::PluginRequestInvalid,
            "plugin_operation_failed:client_unavailable" => Code::PluginClientUnavailable,
            "plugin_operation_failed:unsupported_operation" => Code::PluginOperationUnsupported,
            "plugin_operation_failed:account_binding_missing" => Code::AccountBindingMissing,
            // Only a canonical, bounded OS exit code is recognized. Evidence
            // retains its typed numeric value, never the raw error string.
            _ if process_exit_code(error).is_some() => Code::ProcessFailed,
            _ => Code::Unknown,
        },
    }
}

fn process_exit_code(error: &str) -> Option<u8> {
    let value = error.strip_prefix("plugin_process_failed:exit_code=")?;
    let code = value.parse::<u8>().ok()?;
    (code != 0 && value == code.to_string()).then_some(code)
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Failure {
    schema_version: String,
    request_hash: String,
    phase: Phase,
    code: Code,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<Detail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_code: Option<u8>,
}

pub(crate) fn record(
    storage: &dyn StoragePort,
    key: &str,
    request_hash: &str,
    phase: Phase,
    error: &str,
    context: &SideEffectContext,
) {
    let original_error = error;
    let error = super::plugin_provider_failure::base_error(error);
    let record = Failure {
        schema_version: SCHEMA.into(),
        request_hash: request_hash.into(),
        phase,
        code: classify(phase, error),
        detail: classify_detail(phase, error),
        exit_code: matches!(phase, Phase::Execution)
            .then(|| process_exit_code(error))
            .flatten(),
    };
    // If diagnostic storage also fails, the caller still returns the original
    // reconciliation-required result. Never allow another dispatch here.
    if let Ok(value) = serde_json::to_value(record) {
        if matches!(
            storage.put_json_if_absent(NAMESPACE, key, value, context),
            Ok(crate::ports::ImmutablePutOutcome::Created)
        ) && matches!(phase, Phase::Execution)
        {
            super::plugin_provider_failure::record(
                storage,
                key,
                request_hash,
                original_error,
                context,
            );
        }
    }
}

pub(crate) fn read(storage: &dyn StoragePort, key: &str) -> Option<Value> {
    let value = storage.get_json(NAMESPACE, key).ok()??;
    let failure: Failure = serde_json::from_value(value).ok()?;
    let request = storage.get_json("plugin_operation_requests", key).ok()??;
    if failure.schema_version != SCHEMA
        || request["requestHash"].as_str() != Some(failure.request_hash.as_str())
        || failure.detail.is_some_and(|detail| {
            !matches!(failure.phase, Phase::Execution) || detail.code() != failure.code
        })
        || failure.exit_code.is_some_and(|code| {
            code == 0
                || failure.code != Code::ProcessFailed
                || !matches!(failure.phase, Phase::Execution)
        })
    {
        return None;
    }
    serde_json::to_value(failure).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_detail_is_separate_bound_and_first_write_only() {
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
            IdempotencyKey::new("provider-diagnostic").unwrap(),
        );
        storage
            .put_json(
                "plugin_operation_requests",
                "key",
                json!({"requestHash":"bound"}),
                &context,
            )
            .unwrap();
        let first = "plugin_operation_failed:provider_auth_failed|provider|{\"httpStatus\":403,\"providerCode\":40310000,\"cause\":\"crypto_not_enabled\"}";
        record(
            storage.as_ref(),
            "key",
            "bound",
            Phase::Execution,
            first,
            &context,
        );
        record(
            storage.as_ref(),
            "key",
            "bound",
            Phase::Execution,
            &first.replace("40310000", "40310001"),
            &context,
        );
        assert_eq!(
            read(storage.as_ref(), "key"),
            Some(
                json!({"schemaVersion":SCHEMA,"requestHash":"bound","phase":"execution","code":"provider_auth_failed"})
            )
        );
        let detail = storage
            .get_json("plugin_broker_provider_failures", "key")
            .unwrap()
            .unwrap();
        assert_eq!(detail["requestHash"], "bound");
        assert_eq!(detail["detail"]["providerCode"], 40310000);
        assert!(storage
            .get_json("plugin_operation_receipts", "key")
            .unwrap()
            .is_none());
        record(
            storage.as_ref(),
            "old",
            "bound",
            Phase::Execution,
            "plugin_operation_failed:provider_auth_failed",
            &context,
        );
        record(
            storage.as_ref(),
            "old",
            "bound",
            Phase::Execution,
            first,
            &context,
        );
        assert!(storage
            .get_json("plugin_broker_provider_failures", "old")
            .unwrap()
            .is_none());
    }

    #[test]
    fn execution_failures_use_closed_serialized_categories() {
        for (error, code) in [
            (
                "plugin_operation_failed:provider_auth_failed",
                "provider_auth_failed",
            ),
            (
                "plugin_operation_failed:provider_request_failed",
                "provider_request_failed",
            ),
            (
                "plugin_operation_failed:provider_schema_invalid",
                "provider_schema_invalid",
            ),
            (
                "plugin_operation_failed:provider_order_terminal",
                "provider_order_terminal",
            ),
            (
                "plugin_operation_failed:invalid_request",
                "plugin_request_invalid",
            ),
            (
                "plugin_operation_failed:invalid_endpoint",
                "plugin_configuration_invalid",
            ),
            (
                "plugin_operation_failed:paper_mode_required",
                "plugin_configuration_invalid",
            ),
            (
                "plugin_operation_failed:live_mode_required",
                "plugin_configuration_invalid",
            ),
            (
                "plugin_operation_failed:broker_lookup_mode_invalid",
                "plugin_configuration_invalid",
            ),
            (
                "plugin_operation_failed:broker_lookup_endpoint_invalid",
                "plugin_configuration_invalid",
            ),
            (
                "plugin_operation_failed:credential_grant_missing",
                "plugin_credentials_invalid",
            ),
            (
                "plugin_operation_failed:credential_grant_invalid",
                "plugin_credentials_invalid",
            ),
            (
                "plugin_operation_failed:client_unavailable",
                "plugin_client_unavailable",
            ),
            (
                "plugin_operation_failed:unsupported_operation",
                "plugin_operation_unsupported",
            ),
            (
                "plugin_operation_failed:account_binding_missing",
                "account_binding_missing",
            ),
            ("plugin_process_failed:exit_code=2", "process_failed"),
            ("plugin_process_failed:exit_code=3", "process_failed"),
            ("plugin_process_failed:exit_code=255", "process_failed"),
            ("plugin_process_failed:terminated", "process_terminated"),
            ("plugin_request_write_failed", "process_transport_failed"),
            ("plugin_process_wait_failed", "process_transport_failed"),
            ("plugin_sandbox_unavailable", "sandbox_failed"),
            ("plugin_sandbox_target_invalid", "sandbox_failed"),
            (
                "plugin_response_binding_mismatch",
                "response_binding_mismatch",
            ),
            (
                "plugin_response_evidence_unbound",
                "response_binding_mismatch",
            ),
            ("plugin_response_schema_missing", "response_schema_invalid"),
            ("plugin_response_schema_invalid", "response_schema_invalid"),
            (
                "plugin_response_schema_undeclared",
                "response_schema_invalid",
            ),
            ("plugin_response_secret_leak", "response_policy_rejected"),
            (
                "plugin_response_advice_forbidden",
                "response_policy_rejected",
            ),
            (
                "plugin_response_diagnostic_invalid",
                "response_policy_rejected",
            ),
            (
                "plugin_response_json_depth_exceeded",
                "response_policy_rejected",
            ),
        ] {
            assert_eq!(
                serde_json::to_value(classify(Phase::Execution, error)).unwrap(),
                code
            );
            // A recognized prefix never permits forwarding arbitrary suffixes.
            assert_eq!(
                classify(Phase::Execution, &format!("{error}:credential-canary")),
                Code::Unknown
            );
            assert_eq!(
                classify_detail(Phase::Execution, &format!("{error}:credential-canary")),
                None
            );
            if let Some(detail) = classify_detail(Phase::Execution, error) {
                assert_eq!(detail.code(), classify(Phase::Execution, error));
            }
        }
        for code in [
            "0",
            "-1",
            "256",
            "2147483647",
            "02",
            "+2",
            "2\n",
            "credential-canary",
        ] {
            assert_eq!(
                classify(
                    Phase::Execution,
                    &format!("plugin_process_failed:exit_code={code}")
                ),
                Code::Unknown
            );
        }
    }

    #[test]
    fn persisted_failure_is_closed_bound_and_never_replaces_the_first_failure() {
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
            IdempotencyKey::new("diagnostic-record").unwrap(),
        );
        storage
            .put_json(
                "plugin_operation_requests",
                "key",
                json!({"requestHash":"bound"}),
                &context,
            )
            .unwrap();
        record(
            storage.as_ref(),
            "key",
            "bound",
            Phase::Execution,
            "plugin_operation_failed:provider_auth_failed",
            &context,
        );
        record(
            storage.as_ref(),
            "key",
            "bound",
            Phase::Execution,
            "plugin_operation_failed:credential-canary",
            &context,
        );
        assert_eq!(
            read(storage.as_ref(), "key"),
            Some(
                json!({"schemaVersion":SCHEMA,"requestHash":"bound","phase":"execution","code":"provider_auth_failed"})
            )
        );
        assert!(storage
            .get_json("plugin_operation_receipts", "key")
            .unwrap()
            .is_none());
        storage
            .put_json(
                "plugin_operation_requests",
                "policy",
                json!({"requestHash":"bound"}),
                &context,
            )
            .unwrap();
        record(
            storage.as_ref(),
            "policy",
            "bound",
            Phase::Execution,
            "plugin_response_provider_outcome_invalid",
            &context,
        );
        assert_eq!(
            read(storage.as_ref(), "policy"),
            Some(
                json!({"schemaVersion":SCHEMA,"requestHash":"bound","phase":"execution","code":"response_policy_rejected","detail":"response_provider_outcome_invalid"})
            )
        );
        storage
            .put_json(
                "plugin_operation_requests",
                "exit",
                json!({"requestHash":"bound"}),
                &context,
            )
            .unwrap();
        record(
            storage.as_ref(),
            "exit",
            "bound",
            Phase::Execution,
            "plugin_process_failed:exit_code=3",
            &context,
        );
        assert_eq!(
            read(storage.as_ref(), "exit"),
            Some(
                json!({"schemaVersion":SCHEMA,"requestHash":"bound","phase":"execution","code":"process_failed","exitCode":3})
            )
        );
    }

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
            json!({"detail":"credential-canary"}),
            json!({"detail":"response_secret_leak"}),
            json!({"phase":"completion","code":"response_policy_rejected","detail":"response_secret_leak"}),
            json!({"code":"process_failed","exitCode":0}),
            json!({"code":"process_failed","exitCode":256}),
            json!({"code":"process_failed","exitCode":-1}),
            json!({"code":"process_failed","exitCode":"3"}),
            json!({"exitCode":3}),
            json!({"phase":"completion","code":"process_failed","exitCode":3}),
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
