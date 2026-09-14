// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{ConnectedRunnerError, VersionedPort};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub const REACH_COMMAND_SCHEMA: &str = "tradeassembly.relay.reach-command/v1";
pub const REACH_RECEIPT_SCHEMA: &str = "tradeassembly.relay.reach-receipt/v1";
pub const REACH_NODE_REQUEST_SCHEMA: &str = "tradeassembly.relay.reach-node-request/v1";
const MAX_COMMAND_BYTES: usize = 32 * 1024;
const MAX_TEXT: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReachOperation {
    Activate,
    Inspect,
    Stop,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachCommand {
    pub schema: String,
    pub tenant_id: String,
    pub workspace_id: String,
    pub node_id: String,
    pub actor_ref: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub activation_id: String,
    pub deployment_id: String,
    pub deployment_version: String,
    pub authority_ref: String,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
    pub operation: ReachOperation,
}

impl ReachCommand {
    pub fn validate(&self) -> Result<(), ConnectedRunnerError> {
        if self.schema != REACH_COMMAND_SCHEMA
            || self.issued_at_ms < 0
            || self.expires_at_ms <= self.issued_at_ms
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        for value in [
            &self.tenant_id,
            &self.workspace_id,
            &self.node_id,
            &self.actor_ref,
            &self.request_id,
            &self.idempotency_key,
            &self.activation_id,
            &self.deployment_id,
            &self.deployment_version,
            &self.authority_ref,
        ] {
            validate_text(value)?;
        }
        let bytes = serde_json_canonicalizer::to_vec(self)
            .map_err(|_| ConnectedRunnerError::invalid_contract())?;
        if bytes.len() > MAX_COMMAND_BYTES {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, ConnectedRunnerError> {
        self.validate()?;
        let bytes = serde_json_canonicalizer::to_vec(self)
            .map_err(|_| ConnectedRunnerError::invalid_contract())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReachState {
    Queued,
    Delivered,
    Accepted,
    Completed,
    Rejected,
    Expired,
}

impl ReachState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Rejected | Self::Expired)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachReceipt {
    pub schema: String,
    pub command_digest: String,
    pub state: ReachState,
    pub evidence_ref: String,
    pub recorded_at_ms: i64,
    pub liquidation_performed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachRecord {
    pub command: ReachCommand,
    pub command_digest: String,
    pub state: ReachState,
    pub created_at_ms: i64,
    pub delivered_at_ms: Option<i64>,
    pub accepted_at_ms: Option<i64>,
    pub receipt: Option<ReachReceipt>,
    pub revision: u64,
}

impl ReachRecord {
    pub fn validate_for(
        &self,
        node: &ReachNodeScope,
        now_ms: i64,
    ) -> Result<(), ConnectedRunnerError> {
        self.command.validate()?;
        if self.command_digest != self.command.digest()?
            || self.command.tenant_id != node.tenant_id
            || self.command.workspace_id != node.workspace_id
            || self.command.node_id != node.node_id
            || self.command.actor_ref != node.actor_ref
            || self.created_at_ms != self.command.issued_at_ms
            || self.command.expires_at_ms <= now_ms
            || self.revision == 0
            || self.state.terminal() != self.receipt.is_some()
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        if let Some(receipt) = &self.receipt {
            if receipt.schema != REACH_RECEIPT_SCHEMA
                || receipt.command_digest != self.command_digest
                || receipt.state != self.state
                || receipt.liquidation_performed
                || receipt.recorded_at_ms < self.created_at_ms
            {
                return Err(ConnectedRunnerError::invalid_contract());
            }
            validate_text(&receipt.evidence_ref)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachNodeScope {
    pub tenant_id: String,
    pub workspace_id: String,
    pub node_id: String,
    pub actor_ref: String,
}

impl ReachNodeScope {
    pub fn validate(&self) -> Result<(), ConnectedRunnerError> {
        for value in [
            &self.tenant_id,
            &self.workspace_id,
            &self.node_id,
            &self.actor_ref,
        ] {
            validate_text(value)?;
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachNodeRequest {
    pub node: ReachNodeScope,
    pub request_id: String,
    pub nonce: String,
    pub issued_at_ms: i64,
    pub signature: String,
}

impl fmt::Debug for ReachNodeRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReachNodeRequest")
            .field("node", &self.node)
            .field("request_id", &self.request_id)
            .field("nonce", &"[REDACTED]")
            .field("issued_at_ms", &self.issued_at_ms)
            .field("signature", &"[REDACTED]")
            .finish()
    }
}

impl ReachNodeRequest {
    pub fn signing_payload(
        &self,
        operation: &str,
        command_digest: Option<&str>,
    ) -> Result<String, ConnectedRunnerError> {
        if !matches!(operation, "claim" | "accept" | "complete" | "reject") {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        self.node.validate()?;
        validate_text(&self.request_id)?;
        validate_text(&self.nonce)?;
        if self.issued_at_ms < 0 || self.signature.len() > 1024 {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        let value = serde_json::json!({
            "schema": REACH_NODE_REQUEST_SCHEMA,
            "operation": operation,
            "tenantId": self.node.tenant_id,
            "workspaceId": self.node.workspace_id,
            "nodeId": self.node.node_id,
            "actorRef": self.node.actor_ref,
            "requestId": self.request_id,
            "nonce": self.nonce,
            "issuedAtMs": self.issued_at_ms,
            "commandDigest": command_digest,
        });
        let bytes = serde_json_canonicalizer::to_vec(&value)
            .map_err(|_| ConnectedRunnerError::invalid_contract())?;
        String::from_utf8(bytes).map_err(|_| ConnectedRunnerError::invalid_contract())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachTransitionRequest {
    pub request: ReachNodeRequest,
    pub command_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReachFinishRequest {
    pub request: ReachNodeRequest,
    pub command_digest: String,
    pub state: ReachState,
    pub evidence_ref: String,
}

pub trait ReachTransportPort: VersionedPort + Send + Sync {
    fn claim(
        &self,
        request: &ReachNodeRequest,
    ) -> Result<Option<ReachRecord>, ConnectedRunnerError>;
    fn accept(&self, request: &ReachTransitionRequest)
        -> Result<ReachRecord, ConnectedRunnerError>;
    fn finish(&self, request: &ReachFinishRequest) -> Result<ReachRecord, ConnectedRunnerError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReachPoll {
    Idle,
    Completed {
        request_id: String,
        state: ReachState,
        evidence_ref: String,
    },
}

fn validate_text(value: &str) -> Result<(), ConnectedRunnerError> {
    if value.trim().is_empty() || value.len() > MAX_TEXT || value.chars().any(char::is_control) {
        return Err(ConnectedRunnerError::invalid_contract());
    }
    Ok(())
}
