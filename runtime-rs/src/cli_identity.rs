// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::runtime_config::{EnvSecretResolver, RuntimeConfig, SecretResolver};
use crate::{ports::IdentityClaims, service::TradeAssemblyService};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tradeassembly_identity_sdk::{
    AuthorizationCallback, AuthorizationGrant, MemoryPendingAuthorizationStore, OidcClientConfig,
    OidcProviderPort, OidcService, SecretString, SystemClock,
};
use url::Url;

const SESSION_VERSION: u8 = 1;
const SESSION_AAD: &[u8] = b"tradeassembly.auth-oidc-session.v1";
const MAX_CALLBACK_BYTES: usize = 8_192;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliIdentity {
    pub stable_identity_id: String,
    pub issuer: String,
    pub subject: String,
    pub audience: Vec<String>,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub assurance: Option<String>,
    pub expires_at_ms: i64,
}

impl CliIdentity {
    pub fn redacted_status(&self) -> Value {
        json!({
            "authenticated": true,
            "identityKind": if self.issuer == "local-owner" { "local_owner" } else { "provider" },
            "hubAuthentication": "not_checked",
            "stableIdentityId": self.stable_identity_id,
            "issuer": self.issuer,
            "displayName": self.display_name,
            "expiresAtMs": self.expires_at_ms,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredSession {
    version: u8,
    identity: CliIdentity,
    refresh_token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CliIdentityManager {
    config: RuntimeConfig,
}

impl CliIdentityManager {
    pub fn new(config: RuntimeConfig) -> Self {
        Self { config }
    }

    pub fn uses_local_owner(&self) -> bool {
        self.config.oidc_profile == "local_owner"
    }

    pub(crate) fn configuration(&self) -> &RuntimeConfig {
        &self.config
    }

    pub async fn login(&self) -> Result<CliIdentity, String> {
        if self.config.oidc_profile == "local_owner" {
            return self.local_owner_identity();
        }
        if self.config.oidc_profile == "workos" {
            return crate::workos_identity::WorkosAuthManager::new(self.config.clone())
                .login()
                .await;
        }
        self.login_with_browser_opener(|authorization_url| {
            webbrowser::open(&authorization_url).map_err(|_| "oidc_browser_open_failed".to_string())
        })
        .await
    }

    pub(crate) async fn login_with_browser_opener<F>(
        &self,
        open_browser: F,
    ) -> Result<CliIdentity, String>
    where
        F: FnOnce(String) -> Result<(), String>,
    {
        if self.config.oidc_profile == "workos" {
            return crate::workos_identity::WorkosAuthManager::new(self.config.clone())
                .login_with_browser_opener(open_browser)
                .await;
        }
        let redirect = validated_loopback_redirect(&self.config.oidc_redirect_uri)?;
        let listener = TcpListener::bind((
            redirect
                .host_str()
                .map(|host| host.trim_matches(['[', ']']))
                .ok_or_else(|| "oidc_callback_binding_invalid".to_string())?,
            redirect
                .port()
                .ok_or_else(|| "oidc_callback_binding_invalid".to_string())?,
        ))
        .await
        .map_err(|_| "oidc_callback_listener_unavailable".to_string())?;
        let service = self.oidc_service()?;
        let authorization = service
            .begin_authorization()
            .await
            .map_err(|_| "oidc_authorization_start_failed".to_string())?;
        open_browser(authorization.authorization_url)?;
        let callback = wait_for_callback(
            listener,
            &redirect,
            Duration::from_secs(self.config.oidc_callback_timeout_seconds),
        )
        .await?;
        let grant = service
            .complete_authorization_grant(callback)
            .await
            .map_err(|_| "oidc_authorization_failed".to_string())?;
        let identity = cli_identity_from_grant(&grant, &self.config)?;
        self.session_store().write(&StoredSession {
            version: SESSION_VERSION,
            identity: identity.clone(),
            refresh_token: grant
                .refresh_token
                .as_ref()
                .map(SecretString::expose_secret)
                .map(str::to_string),
        })?;
        Ok(identity)
    }

    pub async fn current_identity(&self) -> Result<CliIdentity, String> {
        if self.config.oidc_profile == "local_owner" {
            return self.local_owner_identity();
        }
        if self.config.oidc_profile == "workos" {
            return crate::workos_identity::WorkosAuthManager::new(self.config.clone())
                .current_identity()
                .await;
        }
        let store = self.session_store();
        let session = store.read()?;
        validate_stored_binding(&session.identity, &self.config)?;
        if session.identity.expires_at_ms > now_ms() {
            return Ok(session.identity);
        }
        let Some(refresh_token) = session.refresh_token else {
            let _ = store.delete();
            return Err("oidc_session_required".to_string());
        };
        let service = self.oidc_service()?;
        let grant = match service
            .refresh_authorization_grant(
                &SecretString::new(refresh_token),
                &session.identity.issuer,
                &session.identity.subject,
            )
            .await
        {
            Ok(grant) => grant,
            Err(_) => {
                let _ = store.delete();
                return Err("oidc_session_required".to_string());
            }
        };
        let identity = cli_identity_from_grant(&grant, &self.config)?;
        store.write(&StoredSession {
            version: SESSION_VERSION,
            identity: identity.clone(),
            refresh_token: grant
                .refresh_token
                .as_ref()
                .map(SecretString::expose_secret)
                .map(str::to_string),
        })?;
        Ok(identity)
    }

    pub async fn status(&self) -> Value {
        match self.current_identity().await {
            Ok(identity) => identity.redacted_status(),
            Err(_) => json!({"authenticated": false, "reason": "oidc_session_required"}),
        }
    }

    pub fn logout(&self) -> Result<(), String> {
        if self.config.oidc_profile == "local_owner" {
            // OS process access is local authority. Never delete the persistent
            // owner on logout and thereby orphan its records.
            return Ok(());
        }
        if self.config.oidc_profile == "workos" {
            return crate::workos_identity::WorkosAuthManager::new(self.config.clone()).logout();
        }
        self.session_store().delete()
    }

    fn oidc_service(
        &self,
    ) -> Result<OidcService<MemoryPendingAuthorizationStore, SystemClock>, String> {
        let config = oidc_client_config(&self.config)?;
        OidcService::new(
            config,
            Arc::new(
                MemoryPendingAuthorizationStore::new(1)
                    .map_err(|_| "oidc_state_store_failed".to_string())?,
            ),
            Arc::new(SystemClock),
        )
        .map_err(|_| "oidc_configuration_invalid".to_string())
    }

    fn session_store(&self) -> EncryptedSessionStore {
        EncryptedSessionStore {
            session_path: PathBuf::from(&self.config.oidc_session_path),
            key_path: PathBuf::from(&self.config.oidc_session_key_path),
        }
    }

    fn local_owner_identity(&self) -> Result<CliIdentity, String> {
        self.config.validate()?;
        let owner = crate::local_owner_identity::LocalOwnerIdentity::for_database(
            self.config
                .database_path
                .as_deref()
                .ok_or("local_owner_database_required")?,
        )?;
        Ok(CliIdentity {
            stable_identity_id: owner.stable_identity_id,
            issuer: owner.issuer,
            subject: owner.subject,
            audience: owner.audience,
            display_name: Some("Local owner".to_string()),
            email: None,
            assurance: Some("local-process".to_string()),
            expires_at_ms: i64::MAX,
        })
    }
}

pub async fn authenticated_service(
    service: &TradeAssemblyService,
    config: &RuntimeConfig,
) -> Result<TradeAssemblyService, String> {
    let identity = CliIdentityManager::new(config.clone())
        .current_identity()
        .await?;
    bind_authenticated_service(service, identity)
}

#[derive(Debug)]
pub struct VerifiedCliSession {
    identity: CliIdentity,
}

pub async fn verify_cli_session(config: &RuntimeConfig) -> Result<VerifiedCliSession, String> {
    let identity = CliIdentityManager::new(config.clone())
        .current_identity()
        .await?;
    Ok(VerifiedCliSession { identity })
}

pub fn authenticated_service_from_config(
    config: RuntimeConfig,
    verified_session: VerifiedCliSession,
) -> Result<TradeAssemblyService, String> {
    let service = TradeAssemblyService::from_config_without_local_scheduler_resume(config)?;
    let service = bind_authenticated_service(&service, verified_session.identity)?;
    service.resume_local_scheduler_workers();
    Ok(service)
}

fn bind_authenticated_service(
    service: &TradeAssemblyService,
    identity: CliIdentity,
) -> Result<TradeAssemblyService, String> {
    service.register_verified_invocation_identity(
        &identity.stable_identity_id,
        &IdentityClaims {
            issuer: identity.issuer.clone(),
            subject: identity.subject.clone(),
            audience: identity.audience.clone(),
            assurance: identity.assurance.clone(),
            expires_at_ms: identity.expires_at_ms,
        },
        now_ms(),
    )?;
    Ok(service.for_authenticated_invocation(
        identity.issuer,
        identity.stable_identity_id,
        identity.email,
        identity.display_name,
    ))
}

fn oidc_client_config(config: &RuntimeConfig) -> Result<OidcClientConfig, String> {
    let mut oidc = match config.oidc_profile.as_str() {
        "tradeassembly_hub_production" => {
            OidcClientConfig::hub_production(&config.oidc_client_id, &config.oidc_redirect_uri)
        }
        "tradeassembly_hub_staging" => {
            OidcClientConfig::hub_staging(&config.oidc_client_id, &config.oidc_redirect_uri)
        }
        "keycloak_local" => OidcClientConfig::keycloak_local(
            &config.oidc_issuer,
            &config.oidc_client_id,
            &config.oidc_redirect_uri,
        ),
        "oidc_test" => OidcClientConfig::oidc_server_mock(
            &config.oidc_issuer,
            &config.oidc_client_id,
            &config.oidc_redirect_uri,
        ),
        "configured_oidc" => OidcClientConfig::configured(
            &config.oidc_issuer,
            &config.oidc_client_id,
            &config.oidc_audience,
            &config.oidc_redirect_uri,
        ),
        "workos" => return Err("workos_profile_requires_workos_adapter".to_string()),
        _ => return Err("oidc_profile_unsupported".to_string()),
    };
    oidc.scopes = config.oidc_scopes.clone();
    oidc.audience = config.oidc_audience.clone();
    oidc.issuer = config.oidc_issuer.clone();
    oidc.client_secret = config
        .oidc_client_secret_ref
        .as_deref()
        .map(|reference| EnvSecretResolver.resolve(reference))
        .transpose()?
        .map(SecretString::new);
    oidc.validate()
        .map_err(|_| "oidc_configuration_invalid".to_string())?;
    Ok(oidc)
}

fn cli_identity_from_grant(
    grant: &AuthorizationGrant,
    config: &RuntimeConfig,
) -> Result<CliIdentity, String> {
    if grant.identity.issuer != config.oidc_issuer {
        return Err("oidc_identity_binding_invalid".to_string());
    }
    Ok(CliIdentity {
        stable_identity_id: grant.identity.stable_identity_id.clone(),
        issuer: grant.identity.issuer.clone(),
        subject: grant.identity.subject.clone(),
        audience: vec![config.oidc_audience.clone()],
        display_name: grant.identity.display_name.clone(),
        email: grant.identity.email.clone(),
        assurance: grant.identity.authentication_context.clone(),
        expires_at_ms: grant.identity.expires_at.timestamp_millis(),
    })
}

fn validate_stored_binding(identity: &CliIdentity, config: &RuntimeConfig) -> Result<(), String> {
    if identity.issuer != config.oidc_issuer
        || !identity.audience.contains(&config.oidc_audience)
        || identity.subject.trim().is_empty()
        || identity.stable_identity_id.trim().is_empty()
    {
        return Err("oidc_session_required".to_string());
    }
    Ok(())
}

pub(crate) async fn wait_for_callback(
    listener: TcpListener,
    redirect: &Url,
    timeout: Duration,
) -> Result<AuthorizationCallback, String> {
    let accepted = tokio::time::timeout(timeout, listener.accept())
        .await
        .map_err(|_| "oidc_callback_timeout".to_string())?
        .map_err(|_| "oidc_callback_failed".to_string())?;
    let (mut stream, peer) = accepted;
    if !peer.ip().is_loopback() {
        return Err("oidc_callback_peer_invalid".to_string());
    }
    let bytes = tokio::time::timeout(timeout, async {
        let mut request = Vec::with_capacity(1_024);
        loop {
            let mut chunk = [0_u8; 1_024];
            let count = stream
                .read(&mut chunk)
                .await
                .map_err(|_| "oidc_callback_failed".to_string())?;
            if count == 0 {
                break;
            }
            if request.len() + count > MAX_CALLBACK_BYTES {
                return Err("oidc_callback_invalid".to_string());
            }
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        if request.is_empty() || !request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Err("oidc_callback_invalid".to_string());
        }
        Ok(request)
    })
    .await
    .map_err(|_| "oidc_callback_timeout".to_string())??;
    let request = std::str::from_utf8(&bytes).map_err(|_| "oidc_callback_invalid".to_string())?;
    let callback = parse_callback_request(request, redirect);
    let (status, body) = if callback.is_ok() {
        (
            "200 OK",
            "TradeAssembly received the sign-in response and is verifying it. Return to the connection page or your agent to check completion. You may close this window.",
        )
    } else {
        (
            "400 Bad Request",
            "TradeAssembly could not complete this sign-in attempt. Return to the connection page or your agent to retry.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|_| "oidc_callback_failed".to_string())?;
    callback
}

fn parse_callback_request(request: &str, redirect: &Url) -> Result<AuthorizationCallback, String> {
    let mut lines = request.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "oidc_callback_invalid".to_string())?;
    let mut request_parts = request_line.split_whitespace();
    if request_parts.next() != Some("GET") {
        return Err("oidc_callback_invalid".to_string());
    }
    let target = request_parts
        .next()
        .ok_or_else(|| "oidc_callback_invalid".to_string())?;
    if request_parts.next() != Some("HTTP/1.1") || request_parts.next().is_some() {
        return Err("oidc_callback_invalid".to_string());
    }
    let expected_host = redirect
        .host_str()
        .and_then(|host| {
            let host = host.trim_matches(['[', ']']);
            redirect.port().map(|port| {
                if host.contains(':') {
                    format!("[{host}]:{port}")
                } else {
                    format!("{host}:{port}")
                }
            })
        })
        .ok_or_else(|| "oidc_callback_binding_invalid".to_string())?;
    let mut hosts = lines.filter_map(|line| {
        line.split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map(|(_, value)| value.trim().to_string())
    });
    let host = hosts.next();
    if host.as_deref() != Some(expected_host.as_str()) || hosts.next().is_some() {
        return Err("oidc_callback_host_invalid".to_string());
    }
    let target = Url::parse(&format!("http://{expected_host}{target}"))
        .map_err(|_| "oidc_callback_invalid".to_string())?;
    if target.path() != redirect.path() || target.fragment().is_some() {
        return Err("oidc_callback_path_invalid".to_string());
    }
    let mut code = None;
    let mut state = None;
    let mut provider_error = false;
    for (name, value) in target.query_pairs() {
        match name.as_ref() {
            "code" if code.is_none() => code = Some(value.into_owned()),
            "code" => return Err("oidc_callback_invalid".to_string()),
            "state" if state.is_none() => state = Some(value.into_owned()),
            "state" => return Err("oidc_callback_invalid".to_string()),
            "error" => provider_error = true,
            _ => {}
        }
    }
    if provider_error {
        return Err("oidc_provider_rejected".to_string());
    }
    let code = code.filter(|value| !value.is_empty());
    let state = state.filter(|value| !value.is_empty());
    match (code, state) {
        (Some(code), Some(state)) => Ok(AuthorizationCallback::new(code, state)),
        _ => Err("oidc_callback_invalid".to_string()),
    }
}

#[derive(Clone, Debug)]
struct EncryptedSessionStore {
    session_path: PathBuf,
    key_path: PathBuf,
}

impl EncryptedSessionStore {
    fn write(&self, session: &StoredSession) -> Result<(), String> {
        let key = self.load_or_create_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| "oidc_session_store_unavailable".to_string())?;
        let mut nonce_bytes = [0_u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let plaintext = serde_json::to_vec(session)
            .map_err(|_| "oidc_session_store_unavailable".to_string())?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: &plaintext,
                    aad: SESSION_AAD,
                },
            )
            .map_err(|_| "oidc_session_store_unavailable".to_string())?;
        let mut envelope = vec![SESSION_VERSION];
        envelope.extend_from_slice(&nonce_bytes);
        envelope.extend_from_slice(&ciphertext);
        atomic_private_write(&self.session_path, &envelope)
    }

    fn read(&self) -> Result<StoredSession, String> {
        ensure_private_file(&self.session_path)?;
        let key = self.load_existing_key()?;
        let envelope =
            fs::read(&self.session_path).map_err(|_| "oidc_session_required".to_string())?;
        if envelope.len() < 30 || envelope[0] != SESSION_VERSION {
            return Err("oidc_session_invalid".to_string());
        }
        let cipher =
            Aes256Gcm::new_from_slice(&key).map_err(|_| "oidc_session_invalid".to_string())?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&envelope[1..13]),
                Payload {
                    msg: &envelope[13..],
                    aad: SESSION_AAD,
                },
            )
            .map_err(|_| "oidc_session_invalid".to_string())?;
        let session: StoredSession =
            serde_json::from_slice(&plaintext).map_err(|_| "oidc_session_invalid".to_string())?;
        if session.version != SESSION_VERSION {
            return Err("oidc_session_invalid".to_string());
        }
        Ok(session)
    }

    fn delete(&self) -> Result<(), String> {
        remove_private_material(&self.session_path)?;
        remove_private_material(&self.key_path)
    }

    fn load_or_create_key(&self) -> Result<[u8; 32], String> {
        match self.load_existing_key() {
            Ok(key) => return Ok(key),
            Err(error) if error != "oidc_session_required" => return Err(error),
            Err(_) => {}
        }
        let mut key = [0_u8; 32];
        OsRng.fill_bytes(&mut key);
        create_private_file(&self.key_path, URL_SAFE_NO_PAD.encode(key))?;
        Ok(key)
    }

    fn load_existing_key(&self) -> Result<[u8; 32], String> {
        ensure_private_file(&self.key_path)?;
        let encoded =
            fs::read_to_string(&self.key_path).map_err(|_| "oidc_session_required".to_string())?;
        let decoded = URL_SAFE_NO_PAD
            .decode(encoded.trim())
            .map_err(|_| "oidc_session_invalid".to_string())?;
        decoded
            .try_into()
            .map_err(|_| "oidc_session_invalid".to_string())
    }
}

fn remove_private_material(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("oidc_session_store_unavailable".to_string()),
    }
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    ensure_parent(path)?;
    let mut suffix = [0_u8; 8];
    OsRng.fill_bytes(&mut suffix);
    let temporary = path.with_extension(format!("tmp-{}", URL_SAFE_NO_PAD.encode(suffix)));
    create_private_file(&temporary, bytes)?;
    fs::rename(&temporary, path).map_err(|_| {
        let _ = fs::remove_file(&temporary);
        "oidc_session_store_unavailable".to_string()
    })?;
    ensure_private_file(path)
}

fn create_private_file(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), String> {
    ensure_parent(path)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "oidc_session_store_unavailable".to_string())?;
    file.write_all(contents.as_ref())
        .and_then(|_| file.sync_all())
        .map_err(|_| "oidc_session_store_unavailable".to_string())?;
    ensure_private_file(path)
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|_| "oidc_session_store_unavailable".to_string())?;
    }
    Ok(())
}

fn ensure_private_file(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "oidc_session_required".to_string()
        } else {
            "oidc_session_store_unavailable".to_string()
        }
    })?;
    if !metadata.is_file() {
        return Err("oidc_session_store_unavailable".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("oidc_session_permissions_invalid".to_string());
        }
    }
    Ok(())
}

pub(crate) fn validated_loopback_redirect(value: &str) -> Result<Url, String> {
    let redirect = Url::parse(value).map_err(|_| "oidc_callback_binding_invalid".to_string())?;
    if redirect.scheme() != "http"
        || !redirect
            .host_str()
            .map(|host| host.trim_matches(['[', ']']))
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"))
        || redirect.port().is_none()
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return Err("oidc_callback_binding_invalid".to_string());
    }
    Ok(redirect)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Form, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use chrono::Utc;
    use openidconnect::core::{CoreJsonWebKeySet, CoreRsaPrivateSigningKey};
    use openidconnect::{JsonWebKeyId, PrivateSigningKey};
    use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey, LineEnding};
    use rsa::{Pkcs1v15Sign, RsaPrivateKey};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    fn local_config(root: &Path) -> RuntimeConfig {
        let mut config = RuntimeConfig::local(root.join("runtime.db").display().to_string());
        config.oidc_profile = "keycloak_local".to_string();
        config.oidc_issuer = "http://127.0.0.1:8080/realms/tradeassembly-dev".to_string();
        config.oidc_client_id = "tradeassembly-local".to_string();
        config.oidc_audience = "tradeassembly-local".to_string();
        config.oidc_session_path = root.join("session.bin").display().to_string();
        config.oidc_session_key_path = root.join("session.key").display().to_string();
        config
    }

    #[tokio::test]
    async fn authenticated_constructor_validates_session_before_opening_core() {
        let root = tempdir().unwrap();
        let mut config = local_config(root.path());
        config.database_path = Some(root.path().display().to_string());

        let error = verify_cli_session(&config)
            .await
            .expect_err("missing session must fail before opening the invalid database path");

        assert_eq!(error, "oidc_session_required");
    }

    fn identity(expires_at_ms: i64) -> CliIdentity {
        CliIdentity {
            stable_identity_id: "identity_test".to_string(),
            issuer: "http://127.0.0.1:8080/realms/tradeassembly-dev".to_string(),
            subject: "subject-1".to_string(),
            audience: vec!["tradeassembly-local".to_string()],
            display_name: Some("Local User".to_string()),
            email: Some("local@example.invalid".to_string()),
            assurance: None,
            expires_at_ms,
        }
    }

    #[derive(Clone)]
    struct MockIssuerState(Arc<Mutex<MockIssuerInner>>);

    struct MockIssuerInner {
        issuer: String,
        key: MockKey,
        expected_nonce: Option<String>,
        expected_challenge: Option<String>,
    }

    #[derive(Clone)]
    struct MockKey {
        pem: String,
        kid: String,
    }

    #[derive(Deserialize)]
    struct MockTokenForm {
        grant_type: String,
        code: String,
        code_verifier: String,
        redirect_uri: String,
        client_id: String,
    }

    async fn start_mock_issuer() -> (String, MockIssuerState) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let state = MockIssuerState(Arc::new(Mutex::new(MockIssuerInner {
            issuer: issuer.clone(),
            key: mock_key().clone(),
            expected_nonce: None,
            expected_challenge: None,
        })));
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(mock_discovery))
            .route("/authorize", get(|| async { StatusCode::NO_CONTENT }))
            .route("/token", post(mock_token))
            .route("/jwks", get(mock_jwks))
            .with_state(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (issuer, state)
    }

    async fn mock_discovery(State(state): State<MockIssuerState>) -> Json<Value> {
        let issuer = state.0.lock().unwrap().issuer.clone();
        Json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "jwks_uri": format!("{issuer}/jwks"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"]
        }))
    }

    async fn mock_jwks(State(state): State<MockIssuerState>) -> Json<CoreJsonWebKeySet> {
        let key = state.0.lock().unwrap().key.clone();
        Json(CoreJsonWebKeySet::new(vec![
            mock_signing_key(&key).as_verification_key()
        ]))
    }

    async fn mock_token(
        State(state): State<MockIssuerState>,
        Form(form): Form<MockTokenForm>,
    ) -> Response {
        let (issuer, key, nonce, challenge) = {
            let inner = state.0.lock().unwrap();
            (
                inner.issuer.clone(),
                inner.key.clone(),
                inner.expected_nonce.clone(),
                inner.expected_challenge.clone(),
            )
        };
        let actual_challenge =
            URL_SAFE_NO_PAD.encode(Sha256::digest(form.code_verifier.as_bytes()));
        if form.grant_type != "authorization_code"
            || form.code != "runtime-valid-code"
            || form.client_id != "tradeassembly-runtime-test"
            || !form.redirect_uri.starts_with("http://127.0.0.1:")
            || challenge.as_deref() != Some(actual_challenge.as_str())
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
        let now = Utc::now().timestamp();
        let access_token = "runtime-access-token";
        let access_hash = Sha256::digest(access_token.as_bytes());
        let claims = json!({
            "iss": issuer,
            "aud": ["tradeassembly-runtime-test"],
            "sub": "runtime-user-123",
            "exp": now + 300,
            "iat": now,
            "nonce": nonce.unwrap(),
            "email": "runtime-user@example.invalid",
            "email_verified": true,
            "at_hash": URL_SAFE_NO_PAD.encode(&access_hash[..access_hash.len() / 2])
        });
        Json(json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": 300,
            "id_token": mock_id_token(&key, &claims),
            "refresh_token": "runtime-refresh-token"
        }))
        .into_response()
    }

    fn mock_id_token(key: &MockKey, claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"alg": "RS256", "kid": key.kid, "typ": "JWT"})).unwrap(),
        );
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
        let signing_input = format!("{header}.{claims}");
        let private_key = RsaPrivateKey::from_pkcs1_pem(&key.pem).unwrap();
        let digest = Sha256::digest(signing_input.as_bytes());
        let signature = private_key
            .sign_with_rng(&mut OsRng, Pkcs1v15Sign::new::<Sha256>(), digest.as_slice())
            .unwrap();
        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    fn mock_signing_key(key: &MockKey) -> CoreRsaPrivateSigningKey {
        CoreRsaPrivateSigningKey::from_pem(&key.pem, Some(JsonWebKeyId::new(key.kid.clone())))
            .unwrap()
    }

    fn mock_key() -> &'static MockKey {
        static KEY: OnceLock<MockKey> = OnceLock::new();
        KEY.get_or_init(|| {
            let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
            MockKey {
                pem: key.to_pkcs1_pem(LineEnding::LF).unwrap().to_string(),
                kid: "runtime-key-1".to_string(),
            }
        })
    }

    #[tokio::test]
    async fn real_oidc_loopback_flow_persists_one_encrypted_session() {
        let root = tempdir().unwrap();
        let (issuer, issuer_state) = start_mock_issuer().await;
        let callback_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let callback_port = callback_probe.local_addr().unwrap().port();
        drop(callback_probe);
        let mut config = local_config(root.path());
        config.oidc_profile = "oidc_test".to_string();
        config.oidc_issuer = issuer;
        config.oidc_client_id = "tradeassembly-runtime-test".to_string();
        config.oidc_audience = "tradeassembly-runtime-test".to_string();
        config.oidc_redirect_uri = format!("http://127.0.0.1:{callback_port}/auth/cli/callback");
        let manager = CliIdentityManager::new(config.clone());
        let identity = manager
            .login_with_browser_opener(move |authorization_url| {
                let authorization = Url::parse(&authorization_url).unwrap();
                let query = authorization
                    .query_pairs()
                    .collect::<std::collections::HashMap<_, _>>();
                let state = query.get("state").unwrap().to_string();
                let nonce = query.get("nonce").unwrap().to_string();
                let challenge = query.get("code_challenge").unwrap().to_string();
                {
                    let mut inner = issuer_state.0.lock().unwrap();
                    inner.expected_nonce = Some(nonce);
                    inner.expected_challenge = Some(challenge);
                }
                tokio::spawn(async move {
                    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", callback_port))
                        .await
                        .unwrap();
                    stream
                        .write_all(
                            format!(
                                "GET /auth/cli/callback?code=runtime-valid-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{callback_port}\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                });
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(identity.subject, "runtime-user-123");
        assert_eq!(
            manager.current_identity().await.unwrap().stable_identity_id,
            identity.stable_identity_id
        );
        let ciphertext = fs::read(&config.oidc_session_path).unwrap();
        let serialized = String::from_utf8_lossy(&ciphertext);
        assert!(!serialized.contains("runtime-user-123"));
        assert!(!serialized.contains("runtime-refresh-token"));
    }

    #[tokio::test]
    async fn bootstrap_returns_before_callback_deduplicates_and_reuses_session() {
        use crate::identity_bootstrap::IdentityBootstrap;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempdir().unwrap();
        let (issuer, issuer_state) = start_mock_issuer().await;
        let callback_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let callback_port = callback_probe.local_addr().unwrap().port();
        drop(callback_probe);
        let mut config = local_config(root.path());
        config.oidc_profile = "oidc_test".to_string();
        config.oidc_issuer = issuer;
        config.oidc_client_id = "tradeassembly-runtime-test".to_string();
        config.oidc_audience = "tradeassembly-runtime-test".to_string();
        config.oidc_redirect_uri = format!("http://127.0.0.1:{callback_port}/auth/cli/callback");
        let bootstrap = IdentityBootstrap::new(config.clone());
        let opens = Arc::new(AtomicUsize::new(0));
        let opens_for_worker = opens.clone();
        let issuer_for_worker = issuer_state.clone();
        let first = bootstrap
            .start_with_browser_opener(move |authorization_url| {
                opens_for_worker.fetch_add(1, Ordering::SeqCst);
                let authorization = Url::parse(&authorization_url).unwrap();
                let query = authorization
                    .query_pairs()
                    .collect::<std::collections::HashMap<_, _>>();
                let state = query.get("state").unwrap().to_string();
                let nonce = query.get("nonce").unwrap().to_string();
                let challenge = query.get("code_challenge").unwrap().to_string();
                {
                    let mut inner = issuer_for_worker.0.lock().unwrap();
                    inner.expected_nonce = Some(nonce);
                    inner.expected_challenge = Some(challenge);
                }
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", callback_port))
                        .await
                        .unwrap();
                    stream
                        .write_all(
                            format!(
                                "GET /auth/cli/callback?code=runtime-valid-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{callback_port}\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                });
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(first["status"], "pending");
        let duplicate = bootstrap
            .start_with_browser_opener(|_| panic!("duplicate start opened a second browser"))
            .await
            .unwrap();
        assert_eq!(duplicate["status"], "pending");
        assert_eq!(duplicate["attemptId"], first["attemptId"]);
        for _ in 0..20 {
            if bootstrap.status().await.unwrap()["authenticated"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(bootstrap.status().await.unwrap()["authenticated"], true);
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        let restarted = IdentityBootstrap::new(config);
        assert_eq!(restarted.status().await.unwrap()["authenticated"], true);
    }

    #[tokio::test]
    async fn bootstrap_wrong_state_fails_without_creating_session() {
        use crate::identity_bootstrap::IdentityBootstrap;
        let root = tempdir().unwrap();
        let (issuer, issuer_state) = start_mock_issuer().await;
        let callback_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let callback_port = callback_probe.local_addr().unwrap().port();
        drop(callback_probe);
        let mut config = local_config(root.path());
        config.oidc_profile = "oidc_test".to_string();
        config.oidc_issuer = issuer;
        config.oidc_client_id = "tradeassembly-runtime-test".to_string();
        config.oidc_audience = "tradeassembly-runtime-test".to_string();
        config.oidc_redirect_uri = format!("http://127.0.0.1:{callback_port}/auth/cli/callback");
        let bootstrap = IdentityBootstrap::new(config.clone());
        bootstrap
            .start_with_browser_opener(move |authorization_url| {
                let authorization = Url::parse(&authorization_url).unwrap();
                let query = authorization
                    .query_pairs()
                    .collect::<std::collections::HashMap<_, _>>();
                {
                    let mut inner = issuer_state.0.lock().unwrap();
                    inner.expected_nonce = Some(query.get("nonce").unwrap().to_string());
                    inner.expected_challenge = Some(query.get("code_challenge").unwrap().to_string());
                }
                tokio::spawn(async move {
                    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", callback_port))
                        .await
                        .unwrap();
                    stream
                        .write_all(
                            format!(
                                "GET /auth/cli/callback?code=runtime-valid-code&state=wrong-state HTTP/1.1\r\nHost: 127.0.0.1:{callback_port}\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                });
                Ok(())
            })
            .await
            .unwrap();
        for _ in 0..20 {
            if bootstrap.status().await.unwrap()["status"] == "failed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let status = bootstrap.status().await.unwrap();
        assert_eq!(status["status"], "failed");
        assert_eq!(status["authenticated"], false);
        assert!(!Path::new(&config.oidc_session_path).exists());
    }

    #[test]
    fn encrypted_session_reopens_and_contains_no_plaintext_secret() {
        let root = tempdir().unwrap();
        let config = local_config(root.path());
        let store = CliIdentityManager::new(config.clone()).session_store();
        store
            .write(&StoredSession {
                version: SESSION_VERSION,
                identity: identity(now_ms() + 60_000),
                refresh_token: Some("refresh-secret-value".to_string()),
            })
            .unwrap();
        let bytes = fs::read(&config.oidc_session_path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("refresh-secret-value"));
        assert!(!text.contains("subject-1"));
        assert_eq!(
            "identity_test",
            store.read().unwrap().identity.stable_identity_id
        );
    }

    #[test]
    fn tamper_wrong_key_truncation_and_permissions_fail_closed() {
        let root = tempdir().unwrap();
        let config = local_config(root.path());
        let store = CliIdentityManager::new(config.clone()).session_store();
        store
            .write(&StoredSession {
                version: SESSION_VERSION,
                identity: identity(now_ms() + 60_000),
                refresh_token: None,
            })
            .unwrap();
        let mut bytes = fs::read(&config.oidc_session_path).unwrap();
        bytes[20] ^= 0x7f;
        fs::write(&config.oidc_session_path, &bytes).unwrap();
        assert!(store.read().is_err());
        fs::write(&config.oidc_session_path, [SESSION_VERSION]).unwrap();
        assert!(store.read().is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config.oidc_session_path, fs::Permissions::from_mode(0o644))
                .unwrap();
            assert!(matches!(
                store.read(),
                Err(error) if error == "oidc_session_permissions_invalid"
            ));
        }
    }

    #[test]
    fn status_and_logout_are_redacted_and_idempotent() {
        let root = tempdir().unwrap();
        let config = local_config(root.path());
        let manager = CliIdentityManager::new(config.clone());
        manager
            .session_store()
            .write(&StoredSession {
                version: SESSION_VERSION,
                identity: identity(now_ms() + 60_000),
                refresh_token: Some("refresh-secret-value".to_string()),
            })
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let status = runtime.block_on(manager.status());
        let encoded = serde_json::to_string(&status).unwrap();
        assert_eq!(Some(true), status["authenticated"].as_bool());
        assert!(!encoded.contains("refresh-secret-value"));
        assert!(!encoded.contains("subject-1"));
        let replayed_session = fs::read(&config.oidc_session_path).unwrap();
        manager.logout().unwrap();
        assert!(!Path::new(&config.oidc_session_key_path).exists());
        atomic_private_write(Path::new(&config.oidc_session_path), &replayed_session).unwrap();
        assert!(manager.session_store().read().is_err());
        manager.logout().unwrap();
        assert_eq!(
            Some(false),
            runtime.block_on(manager.status())["authenticated"].as_bool()
        );
    }

    #[test]
    fn callback_parser_enforces_method_host_path_and_provider_success() {
        let redirect = Url::parse("http://127.0.0.1:8976/callback").unwrap();
        let valid =
            "GET /callback?code=code-value&state=state-value HTTP/1.1\r\nHost: 127.0.0.1:8976\r\n\r\n";
        assert!(parse_callback_request(valid, &redirect).is_ok());
        for invalid in [
            "POST /callback?code=a&state=b HTTP/1.1\r\nHost: 127.0.0.1:8976\r\n\r\n",
            "GET /wrong?code=a&state=b HTTP/1.1\r\nHost: 127.0.0.1:8976\r\n\r\n",
            "GET /callback?code=a&state=b HTTP/1.1\r\nHost: attacker.invalid\r\n\r\n",
            "GET /callback?code=a&state=b HTTP/1.1\r\nHost: 127.0.0.1:8976\r\nHost: 127.0.0.1:8976\r\n\r\n",
            "GET /callback?error=access_denied&state=b HTTP/1.1\r\nHost: 127.0.0.1:8976\r\n\r\n",
            "GET /callback?code=a HTTP/1.1\r\nHost: 127.0.0.1:8976\r\n\r\n",
            "GET /callback?code=a&code=b&state=c HTTP/1.1\r\nHost: 127.0.0.1:8976\r\n\r\n",
            "GET /callback?code=a&state=b&state=c HTTP/1.1\r\nHost: 127.0.0.1:8976\r\n\r\n",
        ] {
            assert!(parse_callback_request(invalid, &redirect).is_err());
        }
        let ipv6 = Url::parse("http://[::1]:8976/callback").unwrap();
        let valid_ipv6 = "GET /callback?code=a&state=b HTTP/1.1\r\nHost: [::1]:8976\r\n\r\n";
        assert!(parse_callback_request(valid_ipv6, &ipv6).is_ok());
    }

    #[tokio::test]
    async fn callback_listener_accepts_a_split_bounded_http_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let redirect = Url::parse(&format!("http://{address}/callback")).unwrap();
        tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            stream
                .write_all(b"GET /callback?code=a&state=b HTTP/1.1\r\n")
                .await
                .unwrap();
            tokio::task::yield_now().await;
            stream
                .write_all(format!("Host: {address}\r\n\r\n").as_bytes())
                .await
                .unwrap();
        });
        let callback = wait_for_callback(listener, &redirect, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!("[REDACTED]", format!("{:?}", callback.code));
        assert_eq!("[REDACTED]", format!("{:?}", callback.state));
    }

    #[test]
    fn stored_identity_is_bound_to_config_and_expiry() {
        let root = tempdir().unwrap();
        let config = local_config(root.path());
        assert!(validate_stored_binding(&identity(now_ms() + 1_000), &config).is_ok());
        let mut wrong = identity(now_ms() + 1_000);
        wrong.issuer = "https://issuer.invalid".to_string();
        assert!(validate_stored_binding(&wrong, &config).is_err());
    }

    #[test]
    fn secret_debug_is_redacted() {
        assert_eq!(
            "[REDACTED]",
            format!("{:?}", SecretString::new("secret-value"))
        );
    }
}
