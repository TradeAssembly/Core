// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialStatus {
    pub provider_ref: String,
    pub configured: bool,
    pub custody: String,
    pub redacted_display: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerCredentials {
    pub provider_ref: String,
    pub api_key: String,
    pub api_secret: String,
    pub base_url: String,
}

pub trait CredentialPort: VersionedPort {
    fn status(&self, provider_ref: &str) -> CredentialStatus;
    fn store(
        &self,
        credential_ref: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<CredentialStatus, String>;
    fn revoke(&self, credential_ref: &str) -> Result<bool, String>;
    fn resolve_fields(
        &self,
        credential_ref: &str,
    ) -> Result<Option<BTreeMap<String, String>>, String>;
    fn resolve_broker_credentials(
        &self,
        provider_ref: &str,
    ) -> Result<Option<BrokerCredentials>, String>;
}

use crate::ports::VersionedPort;
