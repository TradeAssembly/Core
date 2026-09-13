// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::credentials::{BrokerCredentials, CredentialPort, CredentialStatus};
use crate::ports::{FailureMode, PortDescriptor, PortKind, VersionedPort};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct LocalCredentialStore {
    credentials_dir: PathBuf,
}

#[derive(Deserialize, Serialize)]
struct StoredCredential {
    credential_ref: String,
    fields: BTreeMap<String, String>,
}

impl LocalCredentialStore {
    pub fn new(db_path: &str) -> Self {
        let db = PathBuf::from(db_path);
        let credentials_dir = db
            .parent()
            .map(|parent| parent.join("credentials"))
            .unwrap_or_else(|| PathBuf::from(".tradeassembly/credentials"));
        Self { credentials_dir }
    }

    fn credential_path(&self, credential_ref: &str) -> Result<PathBuf, String> {
        validate_credential_ref(credential_ref)?;
        Ok(self.credentials_dir.join(format!("{credential_ref}.json")))
    }

    fn read(&self, credential_ref: &str) -> Result<Option<StoredCredential>, String> {
        let path = self.credential_path(credential_ref)?;
        if !path.exists() {
            return Ok(None);
        }
        let body = std::fs::read_to_string(&path)
            .map_err(|_| format!("read local credential {credential_ref} failed"))?;
        let value: serde_json::Value = serde_json::from_str(&body)
            .map_err(|_| format!("parse local credential {credential_ref} failed"))?;
        let stored = if value.get("fields").is_some() {
            serde_json::from_value(value)
                .map_err(|_| format!("parse local credential {credential_ref} failed"))?
        } else {
            let provider_ref = value
                .get("provider_ref")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(credential_ref)
                .to_string();
            let fields = ["api_key", "api_secret", "base_url"]
                .into_iter()
                .filter_map(|key| {
                    value
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .map(|field| (key.to_string(), field.to_string()))
                })
                .collect();
            StoredCredential {
                credential_ref: provider_ref,
                fields,
            }
        };
        if stored.credential_ref != credential_ref {
            return Err(format!(
                "credential reference mismatch for {credential_ref}"
            ));
        }
        Ok(Some(stored))
    }
}

impl VersionedPort for LocalCredentialStore {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Credentials, "local.credential-file")
            .for_profiles(&["local"])
            .with_capabilities(&[
                "credential.status",
                "credential.store",
                "credential.revoke",
                "credential.resolve",
            ]);
        descriptor.failure_mode = FailureMode::LocalOnly;
        descriptor.secret_fields = vec!["credential_ref".to_string()];
        vec![descriptor]
    }
}

impl CredentialPort for LocalCredentialStore {
    fn status(&self, provider_ref: &str) -> CredentialStatus {
        let redacted_display = self
            .read(provider_ref)
            .ok()
            .flatten()
            .and_then(|credential| {
                credential
                    .fields
                    .get("api_key")
                    .or_else(|| credential.fields.values().next())
                    .and_then(|value| redact(value))
            });
        CredentialStatus {
            provider_ref: provider_ref.to_string(),
            configured: redacted_display.is_some(),
            custody: "local/customer-managed".to_string(),
            redacted_display,
        }
    }

    fn store(
        &self,
        credential_ref: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<CredentialStatus, String> {
        if fields.is_empty() || fields.values().any(|value| value.trim().is_empty()) {
            return Err(format!("credential {credential_ref} is incomplete"));
        }
        let path = self.credential_path(credential_ref)?;
        std::fs::create_dir_all(&self.credentials_dir)
            .map_err(|_| "create local credential directory failed".to_string())?;
        set_owner_only_directory(&self.credentials_dir)?;
        let temporary = path.with_extension("json.tmp");
        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|_| format!("write local credential {credential_ref} failed"))?;
        let payload = serde_json::to_vec(&StoredCredential {
            credential_ref: credential_ref.to_string(),
            fields: fields.clone(),
        })
        .map_err(|_| format!("serialize local credential {credential_ref} failed"))?;
        file.write_all(&payload)
            .map_err(|_| format!("write local credential {credential_ref} failed"))?;
        file.sync_all()
            .map_err(|_| format!("sync local credential {credential_ref} failed"))?;
        std::fs::rename(&temporary, &path)
            .map_err(|_| format!("commit local credential {credential_ref} failed"))?;
        set_owner_only_file(&path)?;
        Ok(self.status(credential_ref))
    }

    fn revoke(&self, credential_ref: &str) -> Result<bool, String> {
        let path = self.credential_path(credential_ref)?;
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(path)
            .map_err(|_| format!("revoke local credential {credential_ref} failed"))?;
        Ok(true)
    }

    fn resolve_broker_credentials(
        &self,
        provider_ref: &str,
    ) -> Result<Option<BrokerCredentials>, String> {
        let Some(fields) = self.resolve_fields(provider_ref)? else {
            return Ok(None);
        };
        let api_key = fields.get("api_key").cloned().unwrap_or_default();
        let api_secret = fields.get("api_secret").cloned().unwrap_or_default();
        let base_url = fields.get("base_url").cloned().unwrap_or_default();
        if api_key.trim().is_empty() || api_secret.trim().is_empty() || base_url.trim().is_empty() {
            return Err(format!("credential {provider_ref} is incomplete"));
        }
        Ok(Some(BrokerCredentials {
            provider_ref: provider_ref.to_string(),
            api_key,
            api_secret,
            base_url,
        }))
    }

    fn resolve_fields(
        &self,
        credential_ref: &str,
    ) -> Result<Option<BTreeMap<String, String>>, String> {
        Ok(self
            .read(credential_ref)?
            .map(|credential| credential.fields))
    }
}

fn validate_credential_ref(value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err("credential reference is invalid".to_string())
    }
}

fn set_owner_only_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "secure local credential directory failed".to_string())?;
    Ok(())
}

fn set_owner_only_file(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| "secure local credential file failed".to_string())?;
    Ok(())
}

fn redact(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return None;
    }
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 8 {
        return Some("***redacted***".to_string());
    }
    let prefix = chars.iter().take(4).collect::<String>();
    let suffix = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    Some(format!("{prefix}...{suffix}"))
}
