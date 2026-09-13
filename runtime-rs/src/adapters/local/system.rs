// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::adapters::studio_identity::TrustedStudioVerifier;
use crate::auth::{authorize_local_owner, AuthzActor, AuthzRequest, AuthzTarget};
use crate::ports::{
    ClockPort, ExportArtifact, ExportPort, FailureMode, IdentityClaims, IdentityPort,
    PolicyDecision, PolicyPort, PolicyRequest, PortDescriptor, PortKind, TrustedStudioSession,
    VersionedPort,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Map;
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Allows setup inspection without inventing an identity authority.
pub struct UnconfiguredIdentity;

/// Local process authority only. Does not implement HTTP/Studio authentication.
pub struct LocalOwnerIdentityPort {
    owner: crate::local_owner_identity::LocalOwnerIdentity,
}

impl LocalOwnerIdentityPort {
    pub fn new(database_path: &str) -> Result<Self, String> {
        Ok(Self {
            owner: crate::local_owner_identity::LocalOwnerIdentity::for_database(database_path)?,
        })
    }

    fn claims(&self) -> IdentityClaims {
        IdentityClaims {
            issuer: self.owner.issuer.clone(),
            subject: self.owner.subject.clone(),
            audience: self.owner.audience.clone(),
            assurance: Some("local-process".to_string()),
            expires_at_ms: i64::MAX,
        }
    }
}

impl VersionedPort for LocalOwnerIdentityPort {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::Identity, "local.installation-owner")
                .for_profiles(&["local"]),
        ]
    }
}

impl IdentityPort for LocalOwnerIdentityPort {
    fn resolve(&self, session_ref: &str, _now_ms: i64) -> Result<IdentityClaims, String> {
        if session_ref != self.owner.stable_identity_id {
            return Err("local_owner_identity_mismatch".to_string());
        }
        Ok(self.claims())
    }

    fn store_verified_session(
        &self,
        session_ref: &str,
        claims: &IdentityClaims,
    ) -> Result<(), String> {
        if session_ref != self.owner.stable_identity_id || *claims != self.claims() {
            return Err("local_owner_identity_mismatch".to_string());
        }
        Ok(())
    }
}

impl VersionedPort for UnconfiguredIdentity {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::Identity, "local.identity-unconfigured")
                .for_profiles(&["local"]),
        ]
    }
}

impl IdentityPort for UnconfiguredIdentity {
    fn resolve(&self, _session_ref: &str, _now_ms: i64) -> Result<IdentityClaims, String> {
        Err("workos_configuration_required".to_string())
    }
}

#[cfg(test)]
mod unconfigured_identity_tests {
    use super::*;

    #[test]
    fn no_identity_or_transport_can_be_accepted_before_configuration() {
        let adapter = UnconfiguredIdentity;
        assert!(adapter.resolve("existing-session", 0).is_err());
        assert!(adapter
            .store_verified_session(
                "session",
                &IdentityClaims {
                    issuer: "https://untrusted.example".to_string(),
                    subject: "claimed-user".to_string(),
                    audience: vec![],
                    assurance: None,
                    expires_at_ms: i64::MAX,
                }
            )
            .is_err());
    }
}

#[derive(Clone, Debug)]
pub struct LocalOidcIdentity {
    db_path: String,
    issuer: String,
    audience: String,
    trusted_studio: TrustedStudioVerifier,
}

impl LocalOidcIdentity {
    pub fn new(
        db_path: impl Into<String>,
        issuer: impl Into<String>,
        audience: impl Into<String>,
    ) -> Result<Self, String> {
        Self::with_trusted_studio_credential(db_path, issuer, audience, None)
    }

    pub fn with_trusted_studio_credential(
        db_path: impl Into<String>,
        issuer: impl Into<String>,
        audience: impl Into<String>,
        credential: Option<String>,
    ) -> Result<Self, String> {
        let issuer = issuer.into();
        let audience = audience.into();
        let adapter = Self {
            db_path: db_path.into(),
            trusted_studio: TrustedStudioVerifier::new(
                issuer.clone(),
                audience.clone(),
                credential,
            )?,
            issuer,
            audience,
        };
        if !adapter.issuer.starts_with("http://") && !adapter.issuer.starts_with("https://") {
            return Err("OIDC issuer must be an absolute HTTP(S) URL".to_string());
        }
        if adapter.audience.trim().is_empty() {
            return Err("OIDC audience is required".to_string());
        }
        adapter.connection()?;
        Ok(adapter)
    }

    pub fn store_verified_session(
        &self,
        session_ref: &str,
        claims: &IdentityClaims,
    ) -> Result<(), String> {
        if claims.issuer != self.issuer || !claims.audience.contains(&self.audience) {
            return Err(
                "verified OIDC claims do not match configured issuer and audience".to_string(),
            );
        }
        let connection = self.connection()?;
        connection
            .execute(
                r#"INSERT INTO oidc_sessions(session_ref, issuer, subject, audience_json, assurance, expires_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(session_ref) DO UPDATE SET issuer=excluded.issuer,
                     subject=excluded.subject, audience_json=excluded.audience_json,
                     assurance=excluded.assurance, expires_at_ms=excluded.expires_at_ms"#,
                params![
                    session_ref,
                    claims.issuer,
                    claims.subject,
                    serde_json::to_string(&claims.audience).map_err(|error| error.to_string())?,
                    claims.assurance,
                    claims.expires_at_ms,
                ],
            )
            .map_err(|_| "store verified OIDC session failed".to_string())?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection, String> {
        if self.db_path != ":memory:" {
            if let Some(parent) = Path::new(&self.db_path)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)
                    .map_err(|_| "create local OIDC session directory failed".to_string())?;
            }
        }
        let connection = Connection::open(&self.db_path)
            .map_err(|_| "open local OIDC session store failed".to_string())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "configure local OIDC session store failed".to_string())?;
        connection
            .execute_batch(
                r#"PRAGMA journal_mode=WAL;
                   PRAGMA synchronous=FULL;
                   CREATE TABLE IF NOT EXISTS oidc_sessions(
                     session_ref TEXT PRIMARY KEY,
                     issuer TEXT NOT NULL,
                     subject TEXT NOT NULL,
                     audience_json TEXT NOT NULL,
                     assurance TEXT,
                     expires_at_ms INTEGER NOT NULL
                   );"#,
            )
            .map_err(|_| "initialize local OIDC session store failed".to_string())?;
        Ok(connection)
    }
}

impl VersionedPort for LocalOidcIdentity {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Identity, "local.oidc-session")
            .for_profiles(&["local"])
            .with_capabilities(&["oidc.discovery", "oidc.session.resolve"]);
        descriptor.secret_fields = vec!["clientSecretRef".to_string(), "sessionRef".to_string()];
        descriptor.configuration_schema = serde_json::json!({
            "type": "object",
            "required": ["issuer", "audience", "clientId"],
            "properties": {
                "issuer": {"type": "string", "format": "uri"},
                "audience": {"type": "string"},
                "clientId": {"type": "string"},
                "clientSecretRef": {"type": "string"}
            },
            "additionalProperties": false
        });
        vec![descriptor]
    }
}

impl IdentityPort for LocalOidcIdentity {
    fn store_verified_session(
        &self,
        session_ref: &str,
        claims: &IdentityClaims,
    ) -> Result<(), String> {
        LocalOidcIdentity::store_verified_session(self, session_ref, claims)
    }

    fn authenticate_studio(
        &self,
        bearer_credential: &str,
        session: &TrustedStudioSession,
        now_ms: i64,
    ) -> Result<IdentityClaims, String> {
        self.trusted_studio
            .authenticate(bearer_credential, session, now_ms)
    }

    fn resolve(&self, session_ref: &str, now_ms: i64) -> Result<IdentityClaims, String> {
        let connection = self.connection()?;
        let claims = connection
            .query_row(
                "SELECT issuer, subject, audience_json, assurance, expires_at_ms FROM oidc_sessions WHERE session_ref=?1",
                params![session_ref],
                |row| {
                    let audiences: String = row.get(2)?;
                    Ok(IdentityClaims {
                        issuer: row.get(0)?,
                        subject: row.get(1)?,
                        audience: serde_json::from_str(&audiences).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                audiences.len(),
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                        assurance: row.get(3)?,
                        expires_at_ms: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|_| "resolve OIDC session failed".to_string())?
            .ok_or_else(|| "OIDC session is unavailable".to_string())?;
        if claims.issuer != self.issuer
            || !claims.audience.contains(&self.audience)
            || claims.expires_at_ms <= now_ms
        {
            return Err(
                "OIDC session is expired or does not match configured authority".to_string(),
            );
        }
        Ok(claims)
    }
}

#[derive(Default)]
pub struct LocalPolicy;

impl VersionedPort for LocalPolicy {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![PortDescriptor::new(PortKind::Policy, "local.policy")
            .for_profiles(&["local"])
            .with_capabilities(&["policy.evaluate", "policy.explain"])]
    }
}

impl PolicyPort for LocalPolicy {
    fn evaluate(&self, request: PolicyRequest) -> Result<PolicyDecision, String> {
        let actor = AuthzActor {
            principal_id: request.authority.actor,
            ..AuthzActor::default()
        };
        let decision = authorize_local_owner(AuthzRequest {
            capability: request.action,
            actor,
            target: Some(AuthzTarget {
                resource_type: Some(request.resource),
                ..AuthzTarget::default()
            }),
            attributes: request
                .context
                .as_object()
                .cloned()
                .unwrap_or_else(Map::new),
        });
        Ok(PolicyDecision {
            decision: if decision.allowed { "allow" } else { "deny" }.to_string(),
            policy_version: "tradeassembly.local-owner.v1".to_string(),
            explanation: vec![decision.reason],
            constraints: serde_json::json!({"purpose": request.purpose}),
        })
    }
}

#[derive(Default)]
pub struct SystemClock;

impl VersionedPort for SystemClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![PortDescriptor::new(PortKind::Clock, "system.utc-clock")
            .for_profiles(&["local", "self_hosted", "serverless"])
            .with_capabilities(&["clock.utc_ms"])]
    }
}

impl ClockPort for SystemClock {
    fn now_ms(&self) -> i64 {
        self.trusted_now_ms().unwrap_or_default()
    }

    fn trusted_now_ms(&self) -> Result<i64, String> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .map_err(|_| "system clock is unavailable".to_string())
    }
}

pub struct LocalExportStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl LocalExportStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock: Mutex::new(()),
        }
    }

    fn path(&self, key: &str) -> Result<PathBuf, String> {
        let path = Path::new(key);
        if key.trim().is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("export key must be a non-empty relative path".to_string());
        }
        Ok(self.root.join(path))
    }
}

impl VersionedPort for LocalExportStore {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor =
            PortDescriptor::new(PortKind::Exports, "local.content-addressed-export")
                .for_profiles(&["local"])
                .with_capabilities(&["export.write", "export.read", "export.sha256"]);
        descriptor.failure_mode = FailureMode::LocalOnly;
        vec![descriptor]
    }
}

impl ExportPort for LocalExportStore {
    fn write(&self, key: &str, content_type: &str, bytes: &[u8]) -> Result<ExportArtifact, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "export store lock poisoned".to_string())?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        let requested = self.path(key)?;
        let extension = requested
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        let relative = format!("sha256/{}/{digest}.{extension}", &digest[..2]);
        let path = self.path(&relative)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if !path.exists() {
            std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        }
        Ok(ExportArtifact {
            artifact_ref: relative,
            content_type: content_type.to_string(),
            size_bytes: bytes.len() as u64,
            sha256: digest,
        })
    }

    fn read(&self, artifact_ref: &str) -> Result<Vec<u8>, String> {
        std::fs::read(self.path(artifact_ref)?).map_err(|error| error.to_string())
    }
}
