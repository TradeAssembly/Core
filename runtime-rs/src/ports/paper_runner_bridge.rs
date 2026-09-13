// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{IdempotencyKey, RunnerBridgeError, RunnerBridgeErrorCode, VersionedPort};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;

pub const CORE_PAPER_RUNNER_SCHEMA: &str = "tradeassembly.core-paper-runner.v1";
pub const CORE_PAPER_RUNNER_VERSION: &str = "1";
const MAX_COMMAND_BYTES: usize = 64 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_REFERENCE_BYTES: usize = 2_048;
const MAX_JSON_DEPTH: usize = 16;
const MAX_JSON_NODES: usize = 2_048;
const MAX_STRING_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaperRunnerOperation {
    Activate {
        activation_id: String,
        config_id: String,
        strategy_id: String,
        strategy_version_id: String,
        strategy_spec_hash: String,
        capability_graph_revision_id: String,
        capability_graph_fingerprint: String,
    },
    Inspect {
        activation_id: String,
    },
    Stop {
        activation_id: String,
    },
}

impl PaperRunnerOperation {
    pub fn activation_id(&self) -> &str {
        match self {
            Self::Activate { activation_id, .. }
            | Self::Inspect { activation_id }
            | Self::Stop { activation_id } => activation_id,
        }
    }

    pub fn kind(&self) -> PaperRunnerOperationKind {
        match self {
            Self::Activate { .. } => PaperRunnerOperationKind::Activate,
            Self::Inspect { .. } => PaperRunnerOperationKind::Inspect,
            Self::Stop { .. } => PaperRunnerOperationKind::Stop,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperRunnerOperationKind {
    Activate,
    Inspect,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperRunnerReceiptState {
    Completed,
    Failed,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaperRunnerCommand {
    pub schema: String,
    pub bridge_version: String,
    pub core_release: String,
    pub workspace_id: String,
    pub command_id: String,
    pub authority_ref: String,
    pub idempotency_key: IdempotencyKey,
    pub submitted_at_ms: i64,
    pub operation: PaperRunnerOperation,
}

impl fmt::Debug for PaperRunnerCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaperRunnerCommand")
            .field("schema", &self.schema)
            .field("bridge_version", &self.bridge_version)
            .field("core_release", &self.core_release)
            .field("workspace_id", &self.workspace_id)
            .field("command_id", &self.command_id)
            .field("authority_ref", &"***redacted***")
            .field("idempotency_key", &"***redacted***")
            .field("submitted_at_ms", &self.submitted_at_ms)
            .field("operation", &self.operation)
            .finish()
    }
}

impl PaperRunnerCommand {
    pub fn parse_json(
        bytes: &[u8],
        expected_core_release: &str,
    ) -> Result<Self, RunnerBridgeError> {
        if bytes.len() > MAX_COMMAND_BYTES {
            return Err(error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "paper runner command exceeds the byte limit",
            ));
        }
        let command: Self = serde_json::from_slice(bytes).map_err(|_| {
            error(
                RunnerBridgeErrorCode::MalformedCommand,
                "paper runner command is not valid strict JSON",
            )
        })?;
        command.validate(expected_core_release)?;
        Ok(command)
    }

    pub fn validate(&self, expected_core_release: &str) -> Result<(), RunnerBridgeError> {
        if self.schema != CORE_PAPER_RUNNER_SCHEMA
            || self.bridge_version != CORE_PAPER_RUNNER_VERSION
        {
            return Err(error(
                RunnerBridgeErrorCode::UnsupportedVersion,
                "paper runner schema or contract version is unsupported",
            ));
        }
        validate_identifier(&self.core_release, "Core release")?;
        validate_identifier(expected_core_release, "expected Core release")?;
        if self.core_release != expected_core_release {
            return Err(error(
                RunnerBridgeErrorCode::CoreReleaseMismatch,
                "paper runner Core release does not match the configured release",
            ));
        }
        validate_identifier(&self.workspace_id, "workspace id")?;
        validate_identifier(&self.command_id, "command id")?;
        validate_reference(&self.authority_ref, "authority reference")?;
        validate_identifier(self.idempotency_key.as_str(), "idempotency key")?;
        if self.submitted_at_ms < 0 {
            return Err(error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "paper runner submission time is invalid",
            ));
        }
        validate_identifier(self.operation.activation_id(), "activation id")?;
        if let PaperRunnerOperation::Activate {
            config_id,
            strategy_id,
            strategy_version_id,
            strategy_spec_hash,
            capability_graph_revision_id,
            capability_graph_fingerprint,
            ..
        } = &self.operation
        {
            validate_identifier(config_id, "configuration id")?;
            validate_identifier(strategy_id, "strategy id")?;
            validate_identifier(strategy_version_id, "strategy version id")?;
            validate_hash(strategy_spec_hash, "strategy specification hash")?;
            validate_identifier(capability_graph_revision_id, "capability graph revision id")?;
            validate_hash(capability_graph_fingerprint, "capability graph fingerprint")?;
        }
        let value = serde_json::to_value(self).map_err(|_| {
            error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "paper runner command could not be validated",
            )
        })?;
        let mut nodes = 0;
        inspect_value(&value, 0, &mut nodes)?;
        let canonical = serde_json_canonicalizer::to_vec(self).map_err(|_| {
            error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "paper runner command could not be canonicalized",
            )
        })?;
        if canonical.len() > MAX_COMMAND_BYTES {
            return Err(error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "paper runner command exceeds the canonical byte limit",
            ));
        }
        Ok(())
    }

    pub fn canonical_hash(&self) -> Result<String, RunnerBridgeError> {
        let bytes = serde_json_canonicalizer::to_vec(self).map_err(|_| {
            error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "paper runner command could not be canonicalized",
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaperRunnerReceipt {
    pub schema: String,
    pub bridge_version: String,
    pub receipt_id: String,
    pub workspace_id: String,
    pub command_id: String,
    pub idempotency_key: IdempotencyKey,
    pub operation: PaperRunnerOperationKind,
    pub activation_id: String,
    pub core_release: String,
    pub command_sha256: String,
    pub state: PaperRunnerReceiptState,
    #[serde(default)]
    pub activation_state: Option<String>,
    #[serde(default)]
    pub result_sha256: Option<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub failure_code: Option<String>,
    pub recorded_at_ms: i64,
}

impl fmt::Debug for PaperRunnerReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaperRunnerReceipt")
            .field("receipt_id", &self.receipt_id)
            .field("workspace_id", &self.workspace_id)
            .field("command_id", &self.command_id)
            .field("idempotency_key", &"***redacted***")
            .field("operation", &self.operation)
            .field("activation_id", &self.activation_id)
            .field("state", &self.state)
            .field("activation_state", &self.activation_state)
            .finish()
    }
}

impl PaperRunnerReceipt {
    pub fn verify(
        &self,
        command: &PaperRunnerCommand,
        expected_core_release: &str,
    ) -> Result<(), RunnerBridgeError> {
        command.validate(expected_core_release)?;
        if self.schema != CORE_PAPER_RUNNER_SCHEMA
            || self.bridge_version != CORE_PAPER_RUNNER_VERSION
        {
            return Err(error(
                RunnerBridgeErrorCode::UnsupportedVersion,
                "paper runner receipt version is unsupported",
            ));
        }
        validate_identifier(&self.receipt_id, "receipt id")?;
        if self.workspace_id != command.workspace_id
            || self.command_id != command.command_id
            || self.idempotency_key != command.idempotency_key
            || self.operation != command.operation.kind()
            || self.activation_id != command.operation.activation_id()
            || self.core_release != command.core_release
            || self.command_sha256 != command.canonical_hash()?
        {
            return Err(error(
                RunnerBridgeErrorCode::ReceiptMismatch,
                "paper runner receipt does not match its command",
            ));
        }
        if self.recorded_at_ms < command.submitted_at_ms || self.evidence_refs.len() > 64 {
            return Err(error(
                RunnerBridgeErrorCode::ReceiptMismatch,
                "paper runner receipt bounds are invalid",
            ));
        }
        for reference in &self.evidence_refs {
            validate_reference(reference, "evidence reference")?;
        }
        match self.state {
            PaperRunnerReceiptState::Completed => {
                if self.activation_state.is_none()
                    || self.result_sha256.is_none()
                    || self.failure_code.is_some()
                {
                    return Err(error(
                        RunnerBridgeErrorCode::ReceiptMismatch,
                        "completed paper runner receipt is missing result integrity",
                    ));
                }
            }
            PaperRunnerReceiptState::Failed => {
                if self.failure_code.is_none()
                    || self.activation_state.is_some()
                    || self.result_sha256.is_some()
                {
                    return Err(error(
                        RunnerBridgeErrorCode::ReceiptMismatch,
                        "failed paper runner receipt has invalid terminal fields",
                    ));
                }
            }
        }
        if let Some(state) = &self.activation_state {
            validate_identifier(state, "activation state")?;
        }
        if let Some(hash) = &self.result_sha256 {
            validate_hash(hash, "result digest")?;
        }
        if let Some(code) = &self.failure_code {
            validate_identifier(code, "failure code")?;
        }
        Ok(())
    }
}

pub trait CorePaperRunnerPort: VersionedPort {
    fn dispatch(
        &self,
        command: &PaperRunnerCommand,
    ) -> Result<PaperRunnerReceipt, RunnerBridgeError>;

    fn receipt(
        &self,
        workspace_id: &str,
        command_id: &str,
    ) -> Result<Option<PaperRunnerReceipt>, RunnerBridgeError>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorePaperRunnerReport {
    pub suite_id: String,
    pub checks: Vec<String>,
}

pub fn verify_core_paper_runner(
    bridge: &dyn CorePaperRunnerPort,
    command: &PaperRunnerCommand,
    expected_core_release: &str,
) -> Result<CorePaperRunnerReport, RunnerBridgeError> {
    command.validate(expected_core_release)?;
    let first = bridge.dispatch(command)?;
    first.verify(command, expected_core_release)?;
    if bridge.dispatch(command)? != first {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            "identical paper runner replay did not return the original receipt",
        ));
    }
    let mut altered = command.clone();
    altered.submitted_at_ms += 1;
    if !matches!(
        bridge.dispatch(&altered),
        Err(RunnerBridgeError {
            code: RunnerBridgeErrorCode::IdempotencyConflict,
            ..
        })
    ) {
        return Err(error(
            RunnerBridgeErrorCode::IdempotencyConflict,
            "altered paper runner replay was not rejected",
        ));
    }
    if bridge.receipt(&command.workspace_id, &command.command_id)? != Some(first.clone()) {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            "paper runner receipt lookup did not return the original receipt",
        ));
    }
    if bridge
        .receipt("different-workspace", &command.command_id)?
        .is_some()
    {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            "paper runner receipt crossed workspace scope",
        ));
    }
    Ok(CorePaperRunnerReport {
        suite_id: "tradeassembly.core-paper-runner.conformance.v1".to_string(),
        checks: vec![
            "strict_command_validation".to_string(),
            "immutable_activation_binding".to_string(),
            "identical_replay_returns_original_receipt".to_string(),
            "altered_replay_conflicts".to_string(),
            "workspace_scoped_receipt_lookup".to_string(),
        ],
    })
}

fn inspect_value(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), RunnerBridgeError> {
    if depth > MAX_JSON_DEPTH {
        return Err(error(
            RunnerBridgeErrorCode::PayloadOutOfBounds,
            "paper runner payload exceeds the nesting limit",
        ));
    }
    *nodes += 1;
    if *nodes > MAX_JSON_NODES {
        return Err(error(
            RunnerBridgeErrorCode::PayloadOutOfBounds,
            "paper runner payload exceeds the node limit",
        ));
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = normalize_key(key);
                if credential_key(&normalized) {
                    return Err(error(
                        RunnerBridgeErrorCode::CredentialMaterialNotAllowed,
                        "paper runner payload contains credential-shaped material",
                    ));
                }
                if matches!(normalized.as_str(), "mode" | "accountmode")
                    && child
                        .as_str()
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("live"))
                {
                    return Err(error(
                        RunnerBridgeErrorCode::LiveModeNotAllowed,
                        "live mode is not allowed through the paper runner",
                    ));
                }
                inspect_value(child, depth + 1, nodes)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                inspect_value(child, depth + 1, nodes)?;
            }
        }
        Value::String(text) => {
            if text.len() > MAX_STRING_BYTES {
                return Err(error(
                    RunnerBridgeErrorCode::PayloadOutOfBounds,
                    "paper runner string exceeds the byte limit",
                ));
            }
            if credential_value(text) {
                return Err(error(
                    RunnerBridgeErrorCode::CredentialMaterialNotAllowed,
                    "paper runner payload contains a credential-shaped reference",
                ));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), RunnerBridgeError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.' | ':' | '/' | '@')
        })
    {
        return Err(error(
            RunnerBridgeErrorCode::PayloadOutOfBounds,
            format!("{label} is invalid"),
        ));
    }
    Ok(())
}

fn validate_reference(value: &str, label: &str) -> Result<(), RunnerBridgeError> {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_BYTES
        || !value.chars().all(|character| character.is_ascii_graphic())
        || credential_value(value)
    {
        return Err(error(
            RunnerBridgeErrorCode::CredentialMaterialNotAllowed,
            format!("{label} is invalid or credential-shaped"),
        ));
    }
    Ok(())
}

fn validate_hash(value: &str, label: &str) -> Result<(), RunnerBridgeError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(error(
            RunnerBridgeErrorCode::PayloadOutOfBounds,
            format!("{label} is invalid"),
        ));
    };
    if hex.len() < 8 || hex.len() > 128 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(error(
            RunnerBridgeErrorCode::PayloadOutOfBounds,
            format!("{label} is invalid"),
        ));
    }
    Ok(())
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn credential_key(key: &str) -> bool {
    matches!(
        key,
        "credential"
            | "credentials"
            | "credentialref"
            | "credentialhandle"
            | "password"
            | "secret"
            | "clientsecret"
            | "sharedsecret"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "apitoken"
            | "apikey"
            | "privatekey"
            | "brokeraccount"
            | "brokeraccountref"
            | "accountnumber"
            | "hostedcredentialref"
    ) || key.ends_with("credentialref")
        || key.ends_with("credentialhandle")
        || key.ends_with("password")
        || key.ends_with("secret")
        || key.ends_with("accesstoken")
        || key.ends_with("refreshtoken")
        || key.ends_with("apikey")
        || key.ends_with("privatekey")
}

fn credential_value(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.starts_with("credential://")
        || normalized.starts_with("secret://")
        || normalized.starts_with("vault://")
        || normalized.starts_with("kms://")
        || normalized.contains("-----begin private key-----")
}

fn error(code: RunnerBridgeErrorCode, message: impl Into<String>) -> RunnerBridgeError {
    RunnerBridgeError {
        code,
        message: message.into(),
    }
}
