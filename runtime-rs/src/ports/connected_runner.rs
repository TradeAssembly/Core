// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    PaperRunnerCommand, PaperRunnerReceipt, PaperRunnerReceiptState, VersionedPort,
    CORE_PAPER_RUNNER_SCHEMA,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, fmt};

pub const INSTALLATION_REQUEST_SCHEMA: &str =
    "tradeassembly.studio-personal.runner-installation-request/v1";
pub const RUNNER_LEASE_SCHEMA: &str = "tradeassembly.studio-personal.runner-lease/v1";
pub const HANDOFF_RECEIPT_SCHEMA: &str = "tradeassembly.studio-personal.runner-handoff-receipt/v1";
pub const INSTALLATION_AUTHENTICATION_DOMAIN: &str =
    "tradeassembly.hub.installation.authentication.v1\0";
pub const RUNNER_LEASE_SECONDS: i64 = 30;
pub const MAX_RUNNER_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_REFERENCE_BYTES: usize = 512;
const LEASE_CLOCK_TOLERANCE_MS: i64 = 5_000;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimRunnerCommandRequest {
    pub schema_version: String,
    pub installation_id: String,
    pub request_id: String,
    pub nonce: String,
    pub issued_at_ms: i64,
    pub lease_seconds: i64,
    pub signature: String,
}

impl fmt::Debug for ClaimRunnerCommandRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimRunnerCommandRequest")
            .field("schema_version", &self.schema_version)
            .field("installation_id", &self.installation_id)
            .field("request_id", &self.request_id)
            .field("nonce", &"[REDACTED]")
            .field("issued_at_ms", &self.issued_at_ms)
            .field("lease_seconds", &self.lease_seconds)
            .field("signature", &"[REDACTED]")
            .finish()
    }
}

impl ClaimRunnerCommandRequest {
    pub fn signing_payload(&self) -> Result<String, ConnectedRunnerError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Unsigned<'a> {
            schema_version: &'a str,
            operation: &'static str,
            installation_id: &'a str,
            request_id: &'a str,
            nonce: &'a str,
            issued_at_ms: i64,
            lease_seconds: i64,
        }
        canonical_string(&Unsigned {
            schema_version: &self.schema_version,
            operation: "claim",
            installation_id: &self.installation_id,
            request_id: &self.request_id,
            nonce: &self.nonce,
            issued_at_ms: self.issued_at_ms,
            lease_seconds: self.lease_seconds,
        })
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerLease {
    pub schema_version: String,
    pub installation_id: String,
    pub workspace_id: String,
    pub command_id: String,
    pub command_sha256: String,
    pub lease_generation: u64,
    pub lease_token: String,
    pub leased_until_ms: i64,
    pub command: PaperRunnerCommand,
}

impl fmt::Debug for RunnerLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerLease")
            .field("installation_id", &self.installation_id)
            .field("workspace_id", &self.workspace_id)
            .field("command_id", &self.command_id)
            .field("command_sha256", &self.command_sha256)
            .field("lease_generation", &self.lease_generation)
            .field("lease_token", &"[REDACTED]")
            .field("leased_until_ms", &self.leased_until_ms)
            .finish()
    }
}

impl RunnerLease {
    pub fn validate(
        &self,
        installation_id: &str,
        expected_core_release: &str,
        now_ms: i64,
    ) -> Result<(), ConnectedRunnerError> {
        if self.schema_version != RUNNER_LEASE_SCHEMA
            || self.installation_id != installation_id
            || self.lease_generation == 0
            || self.leased_until_ms <= now_ms
            || self.leased_until_ms
                < now_ms + RUNNER_LEASE_SECONDS * 1_000 - LEASE_CLOCK_TOLERANCE_MS
            || self.leased_until_ms
                > now_ms + RUNNER_LEASE_SECONDS * 1_000 + LEASE_CLOCK_TOLERANCE_MS
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        validate_identifier(&self.workspace_id)?;
        validate_identifier(&self.command_id)?;
        validate_reference(&self.lease_token)?;
        validate_hash(&self.command_sha256)?;
        self.command
            .validate(expected_core_release)
            .map_err(|_| ConnectedRunnerError::invalid_contract())?;
        if self.workspace_id != self.command.workspace_id
            || self.command_id != self.command.command_id
            || self
                .command
                .canonical_hash()
                .map_err(|_| ConnectedRunnerError::invalid_contract())?
                != self.command_sha256
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerOutcome {
    Completed,
    Failed,
}

impl From<PaperRunnerReceiptState> for RunnerOutcome {
    fn from(value: PaperRunnerReceiptState) -> Self {
        match value {
            PaperRunnerReceiptState::Completed => Self::Completed,
            PaperRunnerReceiptState::Failed => Self::Failed,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerCompletion {
    pub outcome: RunnerOutcome,
    pub core_receipt_schema: String,
    pub core_receipt_sha256: String,
    pub core_receipt_ref: String,
    pub completed_at_ms: i64,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteRunnerCommandRequest {
    pub schema_version: String,
    pub installation_id: String,
    pub request_id: String,
    pub nonce: String,
    pub issued_at_ms: i64,
    pub command_id: String,
    pub lease_generation: u64,
    pub lease_token: String,
    pub command_sha256: String,
    pub completion: RunnerCompletion,
    pub signature: String,
}

impl fmt::Debug for CompleteRunnerCommandRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompleteRunnerCommandRequest")
            .field("installation_id", &self.installation_id)
            .field("request_id", &self.request_id)
            .field("nonce", &"[REDACTED]")
            .field("command_id", &self.command_id)
            .field("lease_generation", &self.lease_generation)
            .field("lease_token", &"[REDACTED]")
            .field("command_sha256", &self.command_sha256)
            .field("completion", &self.completion)
            .field("signature", &"[REDACTED]")
            .finish()
    }
}

impl CompleteRunnerCommandRequest {
    pub fn signing_payload(&self) -> Result<String, ConnectedRunnerError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Unsigned<'a> {
            schema_version: &'a str,
            operation: &'static str,
            installation_id: &'a str,
            request_id: &'a str,
            nonce: &'a str,
            issued_at_ms: i64,
            command_id: &'a str,
            lease_generation: u64,
            lease_token: &'a str,
            command_sha256: &'a str,
            completion: &'a RunnerCompletion,
        }
        canonical_string(&Unsigned {
            schema_version: &self.schema_version,
            operation: "complete",
            installation_id: &self.installation_id,
            request_id: &self.request_id,
            nonce: &self.nonce,
            issued_at_ms: self.issued_at_ms,
            command_id: &self.command_id,
            lease_generation: self.lease_generation,
            lease_token: &self.lease_token,
            command_sha256: &self.command_sha256,
            completion: &self.completion,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerHandoffReceipt {
    pub schema_version: String,
    pub receipt_id: String,
    pub installation_id: String,
    pub workspace_id: String,
    pub command_id: String,
    pub command_sha256: String,
    pub lease_generation: u64,
    pub outcome: RunnerOutcome,
    pub core_receipt_schema: String,
    pub core_receipt_sha256: String,
    pub core_receipt_ref: String,
    pub completed_at_ms: i64,
    pub recorded_at_ms: i64,
}

impl RunnerHandoffReceipt {
    pub fn verify(
        &self,
        lease: &RunnerLease,
        completion: &RunnerCompletion,
    ) -> Result<(), ConnectedRunnerError> {
        if self.schema_version != HANDOFF_RECEIPT_SCHEMA
            || self.installation_id != lease.installation_id
            || self.workspace_id != lease.workspace_id
            || self.command_id != lease.command_id
            || self.command_sha256 != lease.command_sha256
            || self.lease_generation != lease.lease_generation
            || self.outcome != completion.outcome
            || self.core_receipt_schema != completion.core_receipt_schema
            || self.core_receipt_sha256 != completion.core_receipt_sha256
            || self.core_receipt_ref != completion.core_receipt_ref
            || self.completed_at_ms != completion.completed_at_ms
            || self.recorded_at_ms < self.completed_at_ms
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        validate_identifier(&self.receipt_id)?;
        Ok(())
    }
}

pub trait ProductRunnerTransportPort: VersionedPort + Send + Sync {
    fn claim(
        &self,
        request: &ClaimRunnerCommandRequest,
    ) -> Result<Option<RunnerLease>, ConnectedRunnerError>;

    fn complete(
        &self,
        request: &CompleteRunnerCommandRequest,
    ) -> Result<RunnerHandoffReceipt, ConnectedRunnerError>;
}

pub trait InstallationSignerPort: VersionedPort + Send + Sync {
    fn installation_id(&self) -> &str;
    fn sign(&self, payload: &str) -> Result<String, ConnectedRunnerError>;
}

pub trait RunnerEntropyPort: VersionedPort + Send + Sync {
    fn opaque_id(&self, prefix: &str) -> Result<String, ConnectedRunnerError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectedRunnerPoll {
    Idle,
    Completed {
        receipt: Box<PaperRunnerReceipt>,
        handoff: Box<RunnerHandoffReceipt>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRunnerErrorCode {
    InvalidContract,
    TransportUnavailable,
    StaleLease,
    SigningFailure,
    CoreFailure,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ConnectedRunnerError {
    pub code: ConnectedRunnerErrorCode,
    message: &'static str,
}

impl ConnectedRunnerError {
    pub fn invalid_contract() -> Self {
        Self {
            code: ConnectedRunnerErrorCode::InvalidContract,
            message: "connected runner contract is invalid",
        }
    }

    pub fn transport_unavailable() -> Self {
        Self {
            code: ConnectedRunnerErrorCode::TransportUnavailable,
            message: "connected runner transport is unavailable",
        }
    }

    pub fn stale_lease() -> Self {
        Self {
            code: ConnectedRunnerErrorCode::StaleLease,
            message: "connected runner lease is stale",
        }
    }

    pub fn signing_failure() -> Self {
        Self {
            code: ConnectedRunnerErrorCode::SigningFailure,
            message: "connected runner signing failed",
        }
    }

    pub fn core_failure() -> Self {
        Self {
            code: ConnectedRunnerErrorCode::CoreFailure,
            message: "connected runner Core dispatch failed",
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self.code,
            ConnectedRunnerErrorCode::TransportUnavailable | ConnectedRunnerErrorCode::StaleLease
        )
    }
}

impl fmt::Debug for ConnectedRunnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectedRunnerError")
            .field("code", &self.code)
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for ConnectedRunnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for ConnectedRunnerError {}

pub fn canonical_receipt_hash(
    receipt: &PaperRunnerReceipt,
) -> Result<String, ConnectedRunnerError> {
    let bytes = serde_json_canonicalizer::to_vec(receipt)
        .map_err(|_| ConnectedRunnerError::invalid_contract())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonical_string<T: Serialize>(value: &T) -> Result<String, ConnectedRunnerError> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| ConnectedRunnerError::invalid_contract())?;
    String::from_utf8(bytes).map_err(|_| ConnectedRunnerError::invalid_contract())
}

fn validate_identifier(value: &str) -> Result<(), ConnectedRunnerError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(ConnectedRunnerError::invalid_contract());
    }
    Ok(())
}

fn validate_reference(value: &str) -> Result<(), ConnectedRunnerError> {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_BYTES
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(ConnectedRunnerError::invalid_contract());
    }
    Ok(())
}

fn validate_hash(value: &str) -> Result<(), ConnectedRunnerError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(ConnectedRunnerError::invalid_contract());
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConnectedRunnerError::invalid_contract());
    }
    Ok(())
}

pub fn core_receipt_schema(receipt: &PaperRunnerReceipt) -> Result<&str, ConnectedRunnerError> {
    if receipt.schema != CORE_PAPER_RUNNER_SCHEMA {
        return Err(ConnectedRunnerError::invalid_contract());
    }
    Ok(&receipt.schema)
}
