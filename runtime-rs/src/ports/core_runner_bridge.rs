// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::backtest_contracts::BacktestRunManifest;
use crate::ports::{IdempotencyKey, VersionedPort};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;

pub const CORE_RUNNER_BRIDGE_SCHEMA: &str = "tradeassembly.core-runner-bridge.v1";
pub const CORE_RUNNER_BRIDGE_VERSION: &str = "1";
const MAX_COMMAND_BYTES: usize = 256 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_REFERENCE_BYTES: usize = 2_048;
const MAX_REFERENCES: usize = 128;
const MAX_JSON_DEPTH: usize = 24;
const MAX_JSON_NODES: usize = 16_384;
const MAX_COLLECTION_ITEMS: usize = 4_096;
const MAX_STRING_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerBridgeWorkload {
    HostedDeterministicBacktest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerBridgeReceiptState {
    Accepted,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerBridgeErrorCode {
    MalformedCommand,
    UnsupportedVersion,
    CoreReleaseMismatch,
    WorkloadNotAllowed,
    LiveModeNotAllowed,
    CredentialMaterialNotAllowed,
    PayloadOutOfBounds,
    IdempotencyConflict,
    ReceiptMismatch,
    AdapterFailure,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerBridgeCommand {
    pub schema: String,
    pub bridge_version: String,
    pub core_release: String,
    pub workspace_id: String,
    pub command_id: String,
    pub authority_ref: String,
    pub idempotency_key: IdempotencyKey,
    pub workload: RunnerBridgeWorkload,
    pub submitted_at_ms: i64,
    pub backtest_manifest: BacktestRunManifest,
}

impl fmt::Debug for RunnerBridgeCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerBridgeCommand")
            .field("schema", &self.schema)
            .field("bridge_version", &self.bridge_version)
            .field("core_release", &self.core_release)
            .field("workspace_id", &self.workspace_id)
            .field("command_id", &self.command_id)
            .field("authority_ref", &"***redacted***")
            .field("idempotency_key", &"***redacted***")
            .field("workload", &self.workload)
            .field("submitted_at_ms", &self.submitted_at_ms)
            .field(
                "backtest_manifest_hash",
                &self.backtest_manifest.manifest_hash,
            )
            .finish()
    }
}

impl RunnerBridgeCommand {
    pub fn parse_json(
        bytes: &[u8],
        expected_core_release: &str,
    ) -> Result<Self, RunnerBridgeError> {
        if bytes.len() > MAX_COMMAND_BYTES {
            return Err(error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge command exceeds the byte limit",
            ));
        }
        let command: Self = serde_json::from_slice(bytes).map_err(|_| {
            error(
                RunnerBridgeErrorCode::MalformedCommand,
                "runner bridge command is not valid strict JSON",
            )
        })?;
        command.validate(expected_core_release)?;
        Ok(command)
    }

    pub fn validate(&self, expected_core_release: &str) -> Result<(), RunnerBridgeError> {
        if self.schema != CORE_RUNNER_BRIDGE_SCHEMA
            || self.bridge_version != CORE_RUNNER_BRIDGE_VERSION
        {
            return Err(error(
                RunnerBridgeErrorCode::UnsupportedVersion,
                "runner bridge schema or contract version is unsupported",
            ));
        }
        validate_identifier(&self.core_release, "Core release")?;
        validate_identifier(expected_core_release, "expected Core release")?;
        if self.core_release != expected_core_release {
            return Err(error(
                RunnerBridgeErrorCode::CoreReleaseMismatch,
                "runner bridge Core release does not match the configured release",
            ));
        }
        validate_identifier(&self.workspace_id, "workspace id")?;
        validate_identifier(&self.command_id, "command id")?;
        validate_reference(&self.authority_ref, "authority reference")?;
        validate_identifier(self.idempotency_key.as_str(), "idempotency key")?;
        if self.submitted_at_ms < 0 {
            return Err(error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge submission time is invalid",
            ));
        }
        if self.workload != RunnerBridgeWorkload::HostedDeterministicBacktest {
            return Err(error(
                RunnerBridgeErrorCode::WorkloadNotAllowed,
                "runner bridge workload is not allowed",
            ));
        }
        if self
            .backtest_manifest
            .content
            .authority
            .account_mode
            .eq_ignore_ascii_case("live")
        {
            return Err(error(
                RunnerBridgeErrorCode::LiveModeNotAllowed,
                "live mode is not allowed through the hosted runner bridge",
            ));
        }
        let value = serde_json::to_value(self).map_err(|_| {
            error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge command could not be validated",
            )
        })?;
        let mut nodes = 0;
        inspect_value(&value, 0, &mut nodes)?;
        self.backtest_manifest.verify().map_err(|_| {
            error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge backtest manifest failed integrity validation",
            )
        })?;
        let canonical = serde_json_canonicalizer::to_vec(self).map_err(|_| {
            error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge command could not be canonicalized",
            )
        })?;
        if canonical.len() > MAX_COMMAND_BYTES {
            return Err(error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge command exceeds the canonical byte limit",
            ));
        }
        Ok(())
    }

    pub fn canonical_hash(&self) -> Result<String, RunnerBridgeError> {
        let bytes = serde_json_canonicalizer::to_vec(self).map_err(|_| {
            error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge command could not be canonicalized",
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerBridgeReceipt {
    pub schema: String,
    pub bridge_version: String,
    pub receipt_id: String,
    pub workspace_id: String,
    pub command_id: String,
    pub idempotency_key: IdempotencyKey,
    pub workload: RunnerBridgeWorkload,
    pub core_release: String,
    pub command_sha256: String,
    pub state: RunnerBridgeReceiptState,
    #[serde(default)]
    pub result_ref: Option<String>,
    #[serde(default)]
    pub result_sha256: Option<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub failure_code: Option<String>,
    pub recorded_at_ms: i64,
}

impl fmt::Debug for RunnerBridgeReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerBridgeReceipt")
            .field("schema", &self.schema)
            .field("bridge_version", &self.bridge_version)
            .field("receipt_id", &self.receipt_id)
            .field("workspace_id", &self.workspace_id)
            .field("command_id", &self.command_id)
            .field("idempotency_key", &"***redacted***")
            .field("workload", &self.workload)
            .field("core_release", &self.core_release)
            .field("command_sha256", &self.command_sha256)
            .field("state", &self.state)
            .field("recorded_at_ms", &self.recorded_at_ms)
            .finish()
    }
}

impl RunnerBridgeReceipt {
    pub fn accepted(
        command: &RunnerBridgeCommand,
        expected_core_release: &str,
        receipt_id: impl Into<String>,
        recorded_at_ms: i64,
    ) -> Result<Self, RunnerBridgeError> {
        command.validate(expected_core_release)?;
        let receipt = Self {
            schema: CORE_RUNNER_BRIDGE_SCHEMA.to_string(),
            bridge_version: CORE_RUNNER_BRIDGE_VERSION.to_string(),
            receipt_id: receipt_id.into(),
            workspace_id: command.workspace_id.clone(),
            command_id: command.command_id.clone(),
            idempotency_key: command.idempotency_key.clone(),
            workload: command.workload,
            core_release: command.core_release.clone(),
            command_sha256: command.canonical_hash()?,
            state: RunnerBridgeReceiptState::Accepted,
            result_ref: None,
            result_sha256: None,
            evidence_refs: Vec::new(),
            failure_code: None,
            recorded_at_ms,
        };
        receipt.verify(command, expected_core_release)?;
        Ok(receipt)
    }

    pub fn verify(
        &self,
        command: &RunnerBridgeCommand,
        expected_core_release: &str,
    ) -> Result<(), RunnerBridgeError> {
        command.validate(expected_core_release)?;
        if self.schema != CORE_RUNNER_BRIDGE_SCHEMA
            || self.bridge_version != CORE_RUNNER_BRIDGE_VERSION
        {
            return Err(error(
                RunnerBridgeErrorCode::UnsupportedVersion,
                "runner bridge receipt version is unsupported",
            ));
        }
        validate_identifier(&self.receipt_id, "receipt id")?;
        if self.workspace_id != command.workspace_id
            || self.command_id != command.command_id
            || self.idempotency_key != command.idempotency_key
            || self.workload != command.workload
            || self.core_release != command.core_release
            || self.command_sha256 != command.canonical_hash()?
        {
            return Err(error(
                RunnerBridgeErrorCode::ReceiptMismatch,
                "runner bridge receipt does not match the command",
            ));
        }
        if self.recorded_at_ms < command.submitted_at_ms {
            return Err(error(
                RunnerBridgeErrorCode::ReceiptMismatch,
                "runner bridge receipt predates the command",
            ));
        }
        if self.evidence_refs.len() > MAX_REFERENCES {
            return Err(error(
                RunnerBridgeErrorCode::PayloadOutOfBounds,
                "runner bridge receipt has too many evidence references",
            ));
        }
        for reference in &self.evidence_refs {
            validate_reference(reference, "evidence reference")?;
        }
        match self.state {
            RunnerBridgeReceiptState::Accepted => {
                if self.result_ref.is_some()
                    || self.result_sha256.is_some()
                    || self.failure_code.is_some()
                {
                    return Err(error(
                        RunnerBridgeErrorCode::ReceiptMismatch,
                        "accepted runner bridge receipt has terminal fields",
                    ));
                }
            }
            RunnerBridgeReceiptState::Completed => {
                if self.result_ref.is_none()
                    || self.result_sha256.is_none()
                    || self.failure_code.is_some()
                {
                    return Err(error(
                        RunnerBridgeErrorCode::ReceiptMismatch,
                        "completed runner bridge receipt is missing result integrity fields",
                    ));
                }
            }
            RunnerBridgeReceiptState::Failed => {
                if self.failure_code.is_none()
                    || self.result_ref.is_some()
                    || self.result_sha256.is_some()
                {
                    return Err(error(
                        RunnerBridgeErrorCode::ReceiptMismatch,
                        "failed runner bridge receipt has invalid terminal fields",
                    ));
                }
            }
        }
        if let Some(reference) = &self.result_ref {
            validate_reference(reference, "result reference")?;
        }
        if let Some(hash) = &self.result_sha256 {
            validate_sha256(hash, "result digest")?;
        }
        if let Some(code) = &self.failure_code {
            validate_identifier(code, "failure code")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerBridgeError {
    pub code: RunnerBridgeErrorCode,
    pub message: String,
}

impl fmt::Display for RunnerBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", error_code(self.code), self.message)
    }
}

impl std::error::Error for RunnerBridgeError {}

pub trait CoreRunnerBridgePort: VersionedPort {
    fn dispatch(
        &self,
        command: &RunnerBridgeCommand,
    ) -> Result<RunnerBridgeReceipt, RunnerBridgeError>;

    fn receipt(
        &self,
        workspace_id: &str,
        command_id: &str,
    ) -> Result<Option<RunnerBridgeReceipt>, RunnerBridgeError>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreRunnerBridgeReport {
    pub suite_id: String,
    pub checks: Vec<String>,
}

pub fn verify_core_runner_bridge(
    bridge: &dyn CoreRunnerBridgePort,
    command: &RunnerBridgeCommand,
    expected_core_release: &str,
) -> Result<CoreRunnerBridgeReport, RunnerBridgeError> {
    command.validate(expected_core_release)?;
    let first = bridge.dispatch(command)?;
    first.verify(command, expected_core_release)?;
    let replay = bridge.dispatch(command)?;
    if replay != first {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            "identical runner bridge replay did not return the original receipt",
        ));
    }

    let mut altered = command.clone();
    altered.submitted_at_ms += 1;
    match bridge.dispatch(&altered) {
        Err(RunnerBridgeError {
            code: RunnerBridgeErrorCode::IdempotencyConflict,
            ..
        }) => {}
        _ => {
            return Err(error(
                RunnerBridgeErrorCode::IdempotencyConflict,
                "altered idempotency replay was not rejected",
            ))
        }
    }

    if bridge.receipt(&command.workspace_id, &command.command_id)? != Some(first.clone()) {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            "runner bridge receipt lookup did not return the original receipt",
        ));
    }
    if bridge
        .receipt("different-workspace", &command.command_id)?
        .is_some()
    {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            "runner bridge receipt crossed workspace scope",
        ));
    }

    let mut peer = command.clone();
    peer.workspace_id = format!("{}-peer", command.workspace_id);
    peer.command_id = format!("{}-peer", command.command_id);
    let peer_receipt = bridge.dispatch(&peer)?;
    peer_receipt.verify(&peer, expected_core_release)?;
    if peer_receipt.receipt_id == first.receipt_id {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            "workspace-scoped runner bridge commands shared a receipt",
        ));
    }

    Ok(CoreRunnerBridgeReport {
        suite_id: "tradeassembly.core-runner-bridge.conformance.v1".to_string(),
        checks: vec![
            "strict_command_validation".to_string(),
            "canonical_command_binding".to_string(),
            "identical_replay_returns_original_receipt".to_string(),
            "altered_replay_conflicts".to_string(),
            "workspace_scoped_receipt_lookup".to_string(),
            "workspace_scoped_idempotency".to_string(),
        ],
    })
}

fn inspect_value(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), RunnerBridgeError> {
    if depth > MAX_JSON_DEPTH {
        return Err(error(
            RunnerBridgeErrorCode::PayloadOutOfBounds,
            "runner bridge payload exceeds the nesting limit",
        ));
    }
    *nodes += 1;
    if *nodes > MAX_JSON_NODES {
        return Err(error(
            RunnerBridgeErrorCode::PayloadOutOfBounds,
            "runner bridge payload exceeds the node limit",
        ));
    }
    match value {
        Value::Object(object) => {
            if object.len() > MAX_COLLECTION_ITEMS {
                return Err(error(
                    RunnerBridgeErrorCode::PayloadOutOfBounds,
                    "runner bridge object exceeds the field limit",
                ));
            }
            for (key, child) in object {
                let normalized = normalize_key(key);
                if credential_key(&normalized) {
                    return Err(error(
                        RunnerBridgeErrorCode::CredentialMaterialNotAllowed,
                        "runner bridge payload contains credential-shaped material",
                    ));
                }
                if matches!(normalized.as_str(), "mode" | "accountmode")
                    && child
                        .as_str()
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("live"))
                {
                    return Err(error(
                        RunnerBridgeErrorCode::LiveModeNotAllowed,
                        "live mode is not allowed through the hosted runner bridge",
                    ));
                }
                inspect_value(child, depth + 1, nodes)?;
            }
        }
        Value::Array(items) => {
            if items.len() > MAX_COLLECTION_ITEMS {
                return Err(error(
                    RunnerBridgeErrorCode::PayloadOutOfBounds,
                    "runner bridge array exceeds the item limit",
                ));
            }
            for child in items {
                inspect_value(child, depth + 1, nodes)?;
            }
        }
        Value::String(text) => {
            if text.len() > MAX_STRING_BYTES {
                return Err(error(
                    RunnerBridgeErrorCode::PayloadOutOfBounds,
                    "runner bridge string exceeds the byte limit",
                ));
            }
            if credential_value(text) {
                return Err(error(
                    RunnerBridgeErrorCode::CredentialMaterialNotAllowed,
                    "runner bridge payload contains a credential-shaped reference",
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

fn validate_sha256(value: &str, label: &str) -> Result<(), RunnerBridgeError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
            format!("{label} is invalid"),
        ));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(error(
            RunnerBridgeErrorCode::ReceiptMismatch,
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
        || normalized.starts_with("bearer ")
        || normalized.starts_with("-----begin private key-----")
        || normalized.starts_with("sk-")
}

fn error(code: RunnerBridgeErrorCode, message: impl Into<String>) -> RunnerBridgeError {
    RunnerBridgeError {
        code,
        message: message.into(),
    }
}

fn error_code(code: RunnerBridgeErrorCode) -> &'static str {
    match code {
        RunnerBridgeErrorCode::MalformedCommand => "malformed_command",
        RunnerBridgeErrorCode::UnsupportedVersion => "unsupported_version",
        RunnerBridgeErrorCode::CoreReleaseMismatch => "core_release_mismatch",
        RunnerBridgeErrorCode::WorkloadNotAllowed => "workload_not_allowed",
        RunnerBridgeErrorCode::LiveModeNotAllowed => "live_mode_not_allowed",
        RunnerBridgeErrorCode::CredentialMaterialNotAllowed => "credential_material_not_allowed",
        RunnerBridgeErrorCode::PayloadOutOfBounds => "payload_out_of_bounds",
        RunnerBridgeErrorCode::IdempotencyConflict => "idempotency_conflict",
        RunnerBridgeErrorCode::ReceiptMismatch => "receipt_mismatch",
        RunnerBridgeErrorCode::AdapterFailure => "adapter_failure",
    }
}
