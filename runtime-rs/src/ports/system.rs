// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext, VersionedPort};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityClaims {
    pub issuer: String,
    pub subject: String,
    pub audience: Vec<String>,
    pub assurance: Option<String>,
    pub expires_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedStudioSession {
    pub issuer: String,
    pub subject: String,
    pub audience: Vec<String>,
    pub actor: String,
    pub display_name: String,
    pub email: Option<String>,
    pub expires_at_ms: i64,
}

pub trait IdentityPort: VersionedPort {
    fn store_verified_session(
        &self,
        _session_ref: &str,
        _claims: &IdentityClaims,
    ) -> Result<(), String> {
        Err("verified session storage is not supported by this identity adapter".to_string())
    }

    fn authenticate_studio(
        &self,
        _bearer_credential: &str,
        _session: &TrustedStudioSession,
        _now_ms: i64,
    ) -> Result<IdentityClaims, String> {
        Err("trusted Studio authentication is not supported by this identity adapter".to_string())
    }
    fn resolve(&self, session_ref: &str, now_ms: i64) -> Result<IdentityClaims, String>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRequest {
    pub action: String,
    pub resource: String,
    pub purpose: String,
    pub authority: AuthorityContext,
    pub context: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDecision {
    pub decision: String,
    pub policy_version: String,
    pub explanation: Vec<String>,
    pub constraints: Value,
}

pub trait PolicyPort: VersionedPort {
    fn evaluate(&self, request: PolicyRequest) -> Result<PolicyDecision, String>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRecord {
    pub evidence_id: String,
    pub evidence_type: String,
    pub aggregate_id: String,
    pub payload: Value,
    pub idempotency_key: IdempotencyKey,
    pub recorded_at_ms: i64,
}

pub trait EvidencePort: VersionedPort {
    fn append(&self, record: EvidenceRecord) -> Result<(), String>;
    fn list(&self, aggregate_id: &str) -> Result<Vec<EvidenceRecord>, String>;
}

pub trait PluginRegistryPort: VersionedPort {
    fn list_manifests(&self) -> Result<Vec<Value>, String>;
    fn get_manifest(&self, plugin_ref: &str) -> Result<Option<Value>, String>;
    fn install_manifest(
        &self,
        plugin_ref: &str,
        record: Value,
        context: &SideEffectContext,
    ) -> Result<Value, String>;
    fn get_manifest_revision(
        &self,
        plugin_ref: &str,
        manifest_digest: &str,
    ) -> Result<Option<Value>, String>;
    fn activate_manifest_revision(
        &self,
        plugin_ref: &str,
        manifest_digest: &str,
        context: &SideEffectContext,
    ) -> Result<Value, String>;
    fn list_instances(&self) -> Result<Vec<Value>, String>;
    fn get_instance(&self, instance_ref: &str) -> Result<Option<Value>, String>;
    fn put_instance(
        &self,
        instance_ref: &str,
        record: Value,
        context: &SideEffectContext,
    ) -> Result<Value, String>;
}

pub trait TelemetryPort: VersionedPort {
    fn emit(
        &self,
        event_type: &str,
        fields: Value,
        idempotency_key: &IdempotencyKey,
    ) -> Result<bool, String>;
}

pub trait ClockPort: VersionedPort {
    fn now_ms(&self) -> i64;

    fn trusted_now_ms(&self) -> Result<i64, String> {
        Ok(self.now_ms())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportArtifact {
    pub artifact_ref: String,
    pub content_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

pub trait ExportPort: VersionedPort {
    fn write(&self, key: &str, content_type: &str, bytes: &[u8]) -> Result<ExportArtifact, String>;
    fn read(&self, artifact_ref: &str) -> Result<Vec<u8>, String>;
}
