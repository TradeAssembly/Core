//! WorkOS AuthKit public-client authentication.
//!
//! WorkOS access tokens are not OIDC ID tokens: the signed `client_id` claim
//! binds the token to the product application, while `aud` is not required.

use crate::adapters::studio_identity::stable_actor;
use crate::cli_identity::{validated_loopback_redirect, wait_for_callback, CliIdentity};
use crate::runtime_config::RuntimeConfig;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use url::Url;

const MAX_RESPONSE_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, Deserialize)]
struct AccessClaims {
    iss: String,
    sub: String,
    client_id: String,
    exp: usize,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Clone, Debug, Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    n: String,
    e: String,
    alg: Option<String>,
}

#[derive(Clone, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct WorkosSession {
    issuer: String,
    client_id: String,
    tenant_id: String,
    access_token: String,
    refresh_token: String,
    identity: CliIdentity,
}

#[derive(Clone, Debug)]
pub struct WorkosAuthManager {
    config: RuntimeConfig,
}

impl WorkosAuthManager {
    pub fn new(config: RuntimeConfig) -> Self {
        Self { config }
    }

    pub async fn login(&self) -> Result<CliIdentity, String> {
        self.login_with_browser_opener(|url| {
            webbrowser::open(&url).map_err(|_| "workos_browser_open_failed".to_string())
        })
        .await
    }

    pub(crate) async fn login_with_browser_opener<F>(&self, open: F) -> Result<CliIdentity, String>
    where
        F: FnOnce(String) -> Result<(), String>,
    {
        self.validate_configuration()?;
        let redirect = validated_loopback_redirect(&self.config.oidc_redirect_uri)?;
        let listener = TcpListener::bind((
            redirect
                .host_str()
                .ok_or_else(|| "workos_callback_binding_invalid".to_string())?,
            redirect
                .port()
                .ok_or_else(|| "workos_callback_binding_invalid".to_string())?,
        ))
        .await
        .map_err(|_| "workos_callback_listener_unavailable".to_string())?;
        let (verifier, challenge) = pkce_pair();
        let state = random_string(32);
        let authorize = self.authorize_url(&redirect, &state, &challenge)?;
        open(authorize)?;
        let callback = wait_for_callback(
            listener,
            &redirect,
            Duration::from_secs(self.config.oidc_callback_timeout_seconds),
        )
        .await?;
        if callback.state.expose_secret() != state {
            return Err("workos_state_invalid".to_string());
        }
        let token = self
            .exchange_code(callback.code.expose_secret(), &verifier)
            .await?;
        let _lock = self.session_lock()?;
        self.finish(token, None, None).await
    }

    pub async fn current_identity(&self) -> Result<CliIdentity, String> {
        self.validate_configuration()?;
        let _lock = self.session_lock()?;
        let session = self.load_session().map_err(|error| match error.as_str() {
            "bitwarden_session_required" | "bitwarden_session_store_unavailable" => error,
            _ => "oidc_session_invalid".to_string(),
        })?;
        if session.client_id != self.config.oidc_client_id
            || session.issuer != configured_issuer(&self.config)
            || session.identity.issuer != session.issuer
            || session.identity.stable_identity_id
                != stable_actor(&session.issuer, &session.identity.subject)
            || session.identity.audience != vec![self.config.oidc_client_id.clone()]
            || session.identity.subject.is_empty()
            || session.identity.stable_identity_id.is_empty()
        {
            return Err("oidc_session_required".to_string());
        }
        let claims = verify_access_token(&self.config, &session.access_token).await;
        if let Ok(claims) = claims {
            if claims.sub != session.identity.subject {
                let _ = self.delete_session();
                return Err("oidc_session_required".to_string());
            }
            if claims.exp as i64 * 1000 > now_ms() {
                return Ok(identity_from_claims(&claims, &self.config));
            }
        }
        let mut token = self.refresh(&session.refresh_token).await.map_err(|_| {
            let _ = self.delete_session();
            "oidc_session_required".to_string()
        })?;
        if token.refresh_token.is_none() {
            token.refresh_token = Some(session.refresh_token.clone());
        }
        self.finish(
            token,
            Some(&session.identity.subject),
            Some(&session.tenant_id),
        )
        .await
        .inspect_err(|_| {
            let _ = self.delete_session();
        })
    }

    /// Internal relay authentication only; never serialize this token to clients.
    pub(crate) async fn access_token_for_actor(
        &self,
        expected_actor: &str,
    ) -> Result<String, String> {
        let identity = self.current_identity().await?;
        if identity.stable_identity_id != expected_actor {
            return Err("oidc_session_required".to_string());
        }
        let _lock = self.session_lock()?;
        let session = self
            .load_session()
            .map_err(|_| "oidc_session_required".to_string())?;
        if session.identity.stable_identity_id != expected_actor
            || session.client_id != self.config.oidc_client_id
            || session.issuer != configured_issuer(&self.config)
        {
            return Err("oidc_session_required".to_string());
        }
        let claims = verify_access_token(&self.config, &session.access_token)
            .await
            .map_err(|_| "oidc_session_required".to_string())?;
        if claims.sub != identity.subject || claims.exp as i64 * 1000 <= now_ms() {
            return Err("oidc_session_required".to_string());
        }
        Ok(session.access_token)
    }

    pub fn logout(&self) -> Result<(), String> {
        let _lock = self.session_lock()?;
        self.delete_session()
    }

    fn delete_session(&self) -> Result<(), String> {
        if self.config.oidc_session_store == "bitwarden" {
            return self.bitwarden_store().delete();
        }
        let entry = self.keyring_entry()?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("oidc_session_store_unavailable".to_string()),
        }
    }

    // Serialize rotating credentials across independently launched MCP processes.
    // A concurrent caller retries later; it must not exchange the same refresh token.
    fn session_lock(&self) -> Result<std::fs::File, String> {
        let path = std::path::absolute(&self.config.oidc_session_path)
            .map_err(|_| "oidc_session_store_unavailable")?
            .with_extension("workos.lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| "oidc_session_store_unavailable")?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|_| "oidc_session_store_unavailable")?;
        file.try_lock().map_err(|_| "workos_session_busy")?;
        Ok(file)
    }

    fn authorize_url(
        &self,
        redirect: &Url,
        state: &str,
        challenge: &str,
    ) -> Result<String, String> {
        let mut url = provider_endpoint(&self.config, "/user_management/authorize")?;
        url.query_pairs_mut()
            .append_pair("client_id", &self.config.oidc_client_id)
            .append_pair("redirect_uri", redirect.as_str())
            .append_pair("response_type", "code")
            .append_pair("provider", "authkit")
            .append_pair("scope", &scopes(&self.config))
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(url.to_string())
    }

    async fn exchange_code(&self, code: &str, verifier: &str) -> Result<TokenResponse, String> {
        let redirect = &self.config.oidc_redirect_uri;
        let form = [
            ("client_id", self.config.oidc_client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect),
        ];
        self.post_token(&form).await
    }

    async fn refresh(&self, refresh: &str) -> Result<TokenResponse, String> {
        let form = [
            ("client_id", self.config.oidc_client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
        ];
        self.post_token(&form).await
    }

    async fn post_token(&self, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "workos_token_exchange_failed".to_string())?;
        let response = client
            .post(provider_endpoint(
                &self.config,
                "/user_management/authenticate",
            )?)
            .json(
                &form
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeMap<_, _>>(),
            )
            .send()
            .await
            .map_err(|_| "workos_token_exchange_failed".to_string())?;
        if !response.status().is_success() {
            return Err("workos_token_exchange_failed".to_string());
        }
        let bytes = bounded_body(response, "workos_token_exchange_failed").await?;
        serde_json::from_slice(&bytes).map_err(|_| "workos_token_exchange_failed".to_string())
    }

    async fn finish(
        &self,
        token: TokenResponse,
        expected_subject: Option<&str>,
        expected_tenant: Option<&str>,
    ) -> Result<CliIdentity, String> {
        let claims = verify_access_token(&self.config, &token.access_token).await?;
        if expected_subject.is_some_and(|subject| subject != claims.sub) {
            return Err("workos_identity_binding_invalid".to_string());
        }
        let hub =
            crate::hub_identity::project_session(&self.config, &token.access_token, &claims.sub)
                .await?;
        if hub.subject_id != claims.sub {
            return Err("workos_identity_binding_invalid".to_string());
        }
        if expected_tenant.is_some_and(|tenant| tenant != hub.tenant_id) {
            return Err("workos_identity_binding_invalid".to_string());
        }
        let identity = identity_from_claims(&claims, &self.config);
        let refresh_token = token
            .refresh_token
            .ok_or_else(|| "workos_refresh_token_missing".to_string())?;
        let session = WorkosSession {
            issuer: claims.iss,
            client_id: self.config.oidc_client_id.clone(),
            tenant_id: hub.tenant_id,
            access_token: token.access_token,
            refresh_token,
            identity: identity.clone(),
        };
        let raw = serde_json::to_string(&session)
            .map_err(|_| "oidc_session_store_unavailable".to_string())?;
        if self.config.oidc_session_store == "bitwarden" {
            self.bitwarden_store().save(&raw)?;
        } else {
            self.keyring_entry()?
                .set_password(&raw)
                .map_err(|_| "oidc_session_store_unavailable".to_string())?;
        }
        Ok(identity)
    }

    fn session_account(&self) -> String {
        let session_path = std::path::absolute(&self.config.oidc_session_path)
            .unwrap_or_else(|_| std::path::PathBuf::from(&self.config.oidc_session_path));
        let account_material = format!(
            "{}\0{}\0{}",
            self.config.oidc_issuer,
            self.config.oidc_client_id,
            session_path.display()
        );
        URL_SAFE_NO_PAD.encode(Sha256::digest(account_material.as_bytes()))
    }

    fn bitwarden_store(&self) -> crate::bitwarden_session::Store {
        crate::bitwarden_session::Store::new(&self.session_account())
    }

    /// Explicit copy-and-verify migration. The legacy entry is never deleted.
    pub async fn migrate_to_bitwarden(&self) -> Result<(), String> {
        if self.config.oidc_profile != "workos" || self.config.oidc_session_store != "keyring" {
            return Err("workos_migration_source_invalid".into());
        }
        self.current_identity().await?;
        let _lock = self.session_lock()?;
        let raw = self
            .keyring_entry()?
            .get_password()
            .map_err(|_| "oidc_session_required".to_string())?;
        let _: WorkosSession =
            serde_json::from_str(&raw).map_err(|_| "oidc_session_invalid".to_string())?;
        self.bitwarden_store().migrate(&raw)
    }

    fn keyring_entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new("tradeassembly-workos", &self.session_account())
            .map_err(|_| "oidc_session_store_unavailable".to_string())
    }

    fn load_session(&self) -> Result<WorkosSession, String> {
        let raw = if self.config.oidc_session_store == "bitwarden" {
            self.bitwarden_store().load()?
        } else {
            self.keyring_entry()?
                .get_password()
                .map_err(|_| "oidc_session_required".to_string())?
        };
        serde_json::from_str(&raw).map_err(|_| "oidc_session_invalid".to_string())
    }

    fn validate_configuration(&self) -> Result<(), String> {
        if self.config.oidc_client_id.trim().is_empty() || self.config.oidc_issuer.trim().is_empty()
        {
            return Err("workos_configuration_required".to_string());
        }
        self.config
            .validate()
            .map_err(|_| "workos_configuration_invalid".to_string())
    }
}

async fn verify_access_token(config: &RuntimeConfig, token: &str) -> Result<AccessClaims, String> {
    let header = decode_header(token).map_err(|_| "workos_token_header_invalid".to_string())?;
    if header.alg != Algorithm::RS256 {
        return Err("workos_token_header_invalid".to_string());
    }
    let encoded_client: String =
        url::form_urlencoded::byte_serialize(config.oidc_client_id.as_bytes()).collect();
    let jwks_url = provider_endpoint(config, &format!("/sso/jwks/{encoded_client}"))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "workos_jwks_unavailable".to_string())?;
    let response = client
        .get(jwks_url)
        .send()
        .await
        .map_err(|_| "workos_jwks_unavailable".to_string())?;
    if !response.status().is_success() {
        return Err("workos_jwks_unavailable".to_string());
    }
    let bytes = bounded_body(response, "workos_jwks_unavailable").await?;
    let keys: Jwks =
        serde_json::from_slice(&bytes).map_err(|_| "workos_jwks_unavailable".to_string())?;
    verify_access_token_with_jwks(config, token, &keys)
}

fn provider_endpoint(config: &RuntimeConfig, path: &str) -> Result<Url, String> {
    let issuer =
        Url::parse(&config.oidc_issuer).map_err(|_| "workos_configuration_invalid".to_string())?;
    if issuer.scheme() != "https" && issuer.scheme() != "http" {
        return Err("workos_configuration_invalid".to_string());
    }
    let mut endpoint = issuer;
    endpoint.set_path(path);
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

fn verify_access_token_with_jwks(
    config: &RuntimeConfig,
    token: &str,
    keys: &Jwks,
) -> Result<AccessClaims, String> {
    let header = decode_header(token).map_err(|_| "workos_token_header_invalid".to_string())?;
    if header.alg != Algorithm::RS256 || header.kid.as_deref().is_none_or(str::is_empty) {
        return Err("workos_token_header_invalid".to_string());
    }
    let key = keys
        .keys
        .iter()
        .find(|key| {
            key.kty == "RSA"
                && key.kid == header.kid
                && key.alg.as_deref().unwrap_or("RS256") == "RS256"
        })
        .ok_or_else(|| "workos_token_signing_key_unavailable".to_string())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.set_issuer(std::slice::from_ref(&config.oidc_issuer));
    validation.validate_aud = false;
    let claims = decode::<AccessClaims>(
        token,
        &DecodingKey::from_rsa_components(&key.n, &key.e)
            .map_err(|_| "workos_token_signing_key_invalid".to_string())?,
        &validation,
    )
    .map_err(|error| token_validation_error(error.kind()).to_string())?
    .claims;
    if claims.iss != config.oidc_issuer {
        return Err("workos_token_issuer_mismatch".to_string());
    }
    if claims.client_id != config.oidc_client_id {
        return Err("workos_token_client_mismatch".to_string());
    }
    if claims.sub.trim().is_empty() || claims.exp > (i64::MAX / 1000) as usize {
        return Err("workos_token_claims_invalid".to_string());
    }
    Ok(claims)
}

// Only constant categories leave this boundary. Never format JWT errors: serde
// errors may contain attacker-controlled claim contents or other token material.
fn token_validation_error(error: &jsonwebtoken::errors::ErrorKind) -> &'static str {
    use jsonwebtoken::errors::ErrorKind;
    match error {
        ErrorKind::InvalidIssuer => "workos_token_issuer_mismatch",
        ErrorKind::ExpiredSignature => "workos_token_expired",
        ErrorKind::ImmatureSignature => "workos_token_not_yet_valid",
        ErrorKind::InvalidSignature => "workos_token_signature_invalid",
        ErrorKind::Json(_) | ErrorKind::MissingRequiredClaim(_) => "workos_token_claims_invalid",
        _ => "workos_token_invalid",
    }
}

async fn bounded_body(mut response: reqwest::Response, reason: &str) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| reason.to_string())? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(reason.to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn configured_issuer(config: &RuntimeConfig) -> String {
    config.oidc_issuer.clone()
}

fn identity_from_claims(claims: &AccessClaims, config: &RuntimeConfig) -> CliIdentity {
    CliIdentity {
        stable_identity_id: stable_actor(&claims.iss, &claims.sub),
        issuer: claims.iss.clone(),
        subject: claims.sub.clone(),
        audience: vec![config.oidc_client_id.clone()],
        display_name: claims.name.clone(),
        email: claims.email.clone(),
        assurance: Some("workos_authkit_pkce".to_string()),
        expires_at_ms: (claims.exp as i64) * 1000,
    }
}

fn scopes(config: &RuntimeConfig) -> String {
    if config.oidc_scopes.is_empty() {
        "openid profile email offline_access".to_string()
    } else {
        config.oidc_scopes.join(" ")
    }
}
fn random_string(len: usize) -> String {
    let mut bytes = vec![0; len];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn pkce_pair() -> (String, String) {
    let verifier = random_string(32);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts, RsaPrivateKey};
    use serde::Serialize;
    use tempfile::tempdir;

    #[test]
    fn credential_rotation_is_exclusive_and_releases_on_drop() {
        let root = tempdir().unwrap();
        let mut config = RuntimeConfig::local(":memory:");
        config.oidc_session_path = root.path().join("session.bin").display().to_string();
        let manager = WorkosAuthManager::new(config);
        let lock = manager.session_lock().unwrap();
        assert_eq!(manager.session_lock().unwrap_err(), "workos_session_busy");
        drop(lock);
        assert!(manager.session_lock().is_ok());
    }

    #[derive(Serialize)]
    struct TestClaims<'a> {
        iss: &'a str,
        sub: &'a str,
        client_id: &'a str,
        exp: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        nbf: Option<usize>,
    }

    fn setup() -> (RuntimeConfig, RsaPrivateKey, Jwks) {
        let dir = tempdir().unwrap();
        let mut config = RuntimeConfig::local(dir.path().join("runtime.db").display().to_string());
        config.oidc_issuer = "https://identity.example".to_string();
        config.oidc_client_id = "client_test".to_string();
        let key = RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let public = key.to_public_key();
        let jwks = Jwks {
            keys: vec![Jwk {
                kty: "RSA".to_string(),
                kid: Some("test".to_string()),
                n: URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
                e: URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
                alg: Some("RS256".to_string()),
            }],
        };
        (config, key, jwks)
    }

    fn token(key: &RsaPrivateKey, claims: TestClaims<'_>) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test".to_string());
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(
                key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn validator_rejects_wrong_issuer_client_signature_expiry_and_nbf() {
        let (config, key, jwks) = setup();
        let now = (now_ms() / 1000) as usize;
        let valid = || {
            token(
                &key,
                TestClaims {
                    iss: "https://identity.example",
                    sub: "user_1",
                    client_id: "client_test",
                    exp: now + 300,
                    nbf: None,
                },
            )
        };
        assert!(verify_access_token_with_jwks(&config, &valid(), &jwks).is_ok());
        assert!(verify_access_token_with_jwks(
            &config,
            &token(
                &key,
                TestClaims {
                    iss: "https://wrong.example",
                    sub: "user_1",
                    client_id: "client_test",
                    exp: now + 300,
                    nbf: None
                }
            ),
            &jwks
        )
        .is_err());
        assert!(verify_access_token_with_jwks(
            &config,
            &token(
                &key,
                TestClaims {
                    iss: "https://identity.example",
                    sub: "user_1",
                    client_id: "client_other",
                    exp: now + 300,
                    nbf: None
                }
            ),
            &jwks
        )
        .is_err());
        assert!(verify_access_token_with_jwks(&config, &valid().replace('e', "f"), &jwks).is_err());
        assert!(verify_access_token_with_jwks(
            &config,
            &token(
                &key,
                TestClaims {
                    iss: "https://identity.example",
                    sub: "user_1",
                    client_id: "client_test",
                    exp: now - 120,
                    nbf: None
                }
            ),
            &jwks
        )
        .is_err());
        assert!(verify_access_token_with_jwks(
            &config,
            &token(
                &key,
                TestClaims {
                    iss: "https://identity.example",
                    sub: "user_1",
                    client_id: "client_test",
                    exp: now + 300,
                    nbf: Some(now + 300)
                }
            ),
            &jwks
        )
        .is_err());
    }

    #[test]
    fn authorize_url_is_authkit_pkce_and_exact_api_endpoint() {
        let (config, _, _) = setup();
        let manager = WorkosAuthManager::new(config);
        let redirect = Url::parse("http://127.0.0.1:4321/auth/callback").unwrap();
        let url = Url::parse(
            &manager
                .authorize_url(&redirect, "state", "challenge")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            url.as_str().split('?').next().unwrap(),
            "https://identity.example/user_management/authorize"
        );
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("provider").map(String::as_str), Some("authkit"));
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(query.get("state").map(String::as_str), Some("state"));
    }

    #[test]
    fn validation_diagnostics_distinguish_failures_without_claim_contents() {
        let (config, key, jwks) = setup();
        let now = (now_ms() / 1000) as usize;
        for (issuer, client, subject, exp, nbf, expected) in [
            (
                "https://wrong.example",
                "client_test",
                "user_1",
                now + 300,
                None,
                "workos_token_issuer_mismatch",
            ),
            (
                "https://identity.example",
                "other",
                "user_1",
                now + 300,
                None,
                "workos_token_client_mismatch",
            ),
            (
                "https://identity.example",
                "client_test",
                " ",
                now + 300,
                None,
                "workos_token_claims_invalid",
            ),
            (
                "https://identity.example",
                "client_test",
                "user_1",
                now - 120,
                None,
                "workos_token_expired",
            ),
            (
                "https://identity.example",
                "client_test",
                "user_1",
                now + 300,
                Some(now + 300),
                "workos_token_not_yet_valid",
            ),
        ] {
            let signed = token(
                &key,
                TestClaims {
                    iss: issuer,
                    client_id: client,
                    sub: subject,
                    exp,
                    nbf,
                },
            );
            assert_eq!(
                verify_access_token_with_jwks(&config, &signed, &jwks).unwrap_err(),
                expected
            );
        }
        assert_eq!(
            verify_access_token_with_jwks(&config, "not-a-token", &jwks).unwrap_err(),
            "workos_token_header_invalid"
        );
        let signed = token(
            &key,
            TestClaims {
                iss: &config.oidc_issuer,
                client_id: &config.oidc_client_id,
                sub: "user_1",
                exp: now + 300,
                nbf: None,
            },
        );
        assert_eq!(
            verify_access_token_with_jwks(&config, &signed, &Jwks { keys: vec![] }).unwrap_err(),
            "workos_token_signing_key_unavailable"
        );
        let injected =
            jsonwebtoken::errors::ErrorKind::MissingRequiredClaim("secret-sentinel".into());
        assert_eq!(
            token_validation_error(&injected),
            "workos_token_claims_invalid"
        );
        assert_eq!(
            token_validation_error(&jsonwebtoken::errors::ErrorKind::InvalidSignature),
            "workos_token_signature_invalid"
        );
    }

    #[test]
    #[ignore = "requires desktop OS credential store"]
    fn native_keyring_round_trip_uses_os_store() {
        let account = format!("test-{}", random_string(16));
        let entry = keyring::Entry::new("tradeassembly-workos-test", &account).unwrap();
        entry.set_password("synthetic-session-only").unwrap();
        assert_eq!(entry.get_password().unwrap(), "synthetic-session-only");
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));
    }
}
