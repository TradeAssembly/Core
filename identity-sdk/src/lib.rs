// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Provider-neutral OpenID Connect boundary for TradeAssembly Hub and Studio clients.

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreJwsSigningAlgorithm, CoreProviderMetadata,
};
use openidconnect::reqwest;
use openidconnect::{
    AccessTokenHash, AsyncHttpClient, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    HttpRequest, HttpResponse, IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
use url::Url;

pub const HUB_PRODUCTION_ISSUER: &str = "https://id.tradeassembly.org";
pub const HUB_STAGING_ISSUER: &str = "https://id.staging.tradeassembly.org";
pub const DEFAULT_AUTHORIZATION_TTL_SECONDS: u64 = 600;
pub const DEFAULT_MAX_PENDING_AUTHORIZATIONS: usize = 1_024;
pub const DEFAULT_MAX_HTTP_RESPONSE_BYTES: usize = 1_048_576;
pub const MAX_TOKEN_CLOCK_SKEW_SECONDS: i64 = 60;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OidcProfile {
    TradeAssemblyHubProduction,
    TradeAssemblyHubStaging,
    KeycloakLocalDev,
    OidcServerMockTest,
    ConfiguredOidc,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClaimMapping {
    pub email: String,
    pub email_verified: String,
    pub display_name: String,
    pub authentication_context: String,
    pub authentication_methods: String,
    pub authentication_time: String,
}

impl Default for ClaimMapping {
    fn default() -> Self {
        Self {
            email: "email".to_string(),
            email_verified: "email_verified".to_string(),
            display_name: "name".to_string(),
            authentication_context: "acr".to_string(),
            authentication_methods: "amr".to_string(),
            authentication_time: "auth_time".to_string(),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    fn expose(&self) -> &str {
        self.expose_secret()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone)]
pub struct OidcClientConfig {
    pub profile: OidcProfile,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<SecretString>,
    pub audience: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub claim_mapping: ClaimMapping,
    pub allowed_signing_algorithms: Vec<String>,
    pub authorization_ttl: Duration,
    pub max_pending_authorizations: usize,
    pub max_http_response_bytes: usize,
    pub allow_insecure_loopback_issuer: bool,
}

impl fmt::Debug for OidcClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.redacted().fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RedactedOidcConfig {
    pub profile: OidcProfile,
    pub issuer: String,
    pub client_id: String,
    pub client_secret_configured: bool,
    pub audience: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub allowed_signing_algorithms: Vec<String>,
    pub authorization_ttl_seconds: u64,
    pub max_pending_authorizations: usize,
    pub max_http_response_bytes: usize,
}

impl OidcClientConfig {
    pub fn hub_production(client_id: impl Into<String>, redirect_uri: impl Into<String>) -> Self {
        Self::named(
            OidcProfile::TradeAssemblyHubProduction,
            HUB_PRODUCTION_ISSUER,
            client_id,
            redirect_uri,
        )
    }

    pub fn hub_staging(client_id: impl Into<String>, redirect_uri: impl Into<String>) -> Self {
        Self::named(
            OidcProfile::TradeAssemblyHubStaging,
            HUB_STAGING_ISSUER,
            client_id,
            redirect_uri,
        )
    }

    pub fn keycloak_local(
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let mut config = Self::named(
            OidcProfile::KeycloakLocalDev,
            issuer,
            client_id,
            redirect_uri,
        );
        config.allow_insecure_loopback_issuer = true;
        config
    }

    pub fn oidc_server_mock(
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let mut config = Self::named(
            OidcProfile::OidcServerMockTest,
            issuer,
            client_id,
            redirect_uri,
        );
        config.allow_insecure_loopback_issuer = true;
        config
    }

    pub fn configured(
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        audience: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let client_id = client_id.into();
        Self {
            profile: OidcProfile::ConfiguredOidc,
            issuer: issuer.into(),
            audience: audience.into(),
            redirect_uri: redirect_uri.into(),
            scopes: default_scopes(),
            claim_mapping: ClaimMapping::default(),
            allowed_signing_algorithms: vec!["RS256".to_string()],
            authorization_ttl: Duration::from_secs(DEFAULT_AUTHORIZATION_TTL_SECONDS),
            max_pending_authorizations: DEFAULT_MAX_PENDING_AUTHORIZATIONS,
            max_http_response_bytes: DEFAULT_MAX_HTTP_RESPONSE_BYTES,
            allow_insecure_loopback_issuer: false,
            client_id,
            client_secret: None,
        }
    }

    fn named(
        profile: OidcProfile,
        issuer: impl Into<String>,
        client_id: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        let client_id = client_id.into();
        Self {
            profile,
            issuer: issuer.into(),
            audience: client_id.clone(),
            redirect_uri: redirect_uri.into(),
            scopes: default_scopes(),
            claim_mapping: ClaimMapping::default(),
            allowed_signing_algorithms: vec!["RS256".to_string()],
            authorization_ttl: Duration::from_secs(DEFAULT_AUTHORIZATION_TTL_SECONDS),
            max_pending_authorizations: DEFAULT_MAX_PENDING_AUTHORIZATIONS,
            max_http_response_bytes: DEFAULT_MAX_HTTP_RESPONSE_BYTES,
            allow_insecure_loopback_issuer: false,
            client_id,
            client_secret: None,
        }
    }

    pub fn validate(&self) -> Result<(), OidcError> {
        if self.client_id.trim().is_empty() {
            return Err(OidcError::Configuration("client_id is required"));
        }
        if self.audience.trim().is_empty() {
            return Err(OidcError::Configuration("audience is required"));
        }
        if self.authorization_ttl.is_zero() {
            return Err(OidcError::Configuration(
                "authorization_ttl must be positive",
            ));
        }
        if self.max_pending_authorizations == 0 {
            return Err(OidcError::Configuration(
                "max_pending_authorizations must be positive",
            ));
        }
        if self.max_http_response_bytes == 0 {
            return Err(OidcError::Configuration(
                "max_http_response_bytes must be positive",
            ));
        }
        if !self.scopes.iter().any(|scope| scope == "openid") {
            return Err(OidcError::Configuration("openid scope is required"));
        }
        if self.allowed_signing_algorithms.is_empty()
            || self
                .allowed_signing_algorithms
                .iter()
                .any(|algorithm| algorithm != "RS256")
        {
            return Err(OidcError::Configuration(
                "only RS256 is supported in this release",
            ));
        }

        let issuer = parse_url("issuer", &self.issuer)?;
        if issuer.query().is_some() || issuer.fragment().is_some() {
            return Err(OidcError::Configuration(
                "issuer cannot contain query or fragment",
            ));
        }
        validate_secure_url(
            "issuer",
            &issuer,
            self.allow_insecure_loopback_issuer
                && matches!(
                    self.profile,
                    OidcProfile::KeycloakLocalDev | OidcProfile::OidcServerMockTest
                ),
        )?;

        let redirect = parse_url("redirect_uri", &self.redirect_uri)?;
        validate_secure_url("redirect_uri", &redirect, true)?;

        if matches!(
            self.profile,
            OidcProfile::TradeAssemblyHubProduction | OidcProfile::TradeAssemblyHubStaging
        ) && self.issuer
            != match self.profile {
                OidcProfile::TradeAssemblyHubProduction => HUB_PRODUCTION_ISSUER,
                OidcProfile::TradeAssemblyHubStaging => HUB_STAGING_ISSUER,
                _ => unreachable!(),
            }
        {
            return Err(OidcError::Configuration("Hub profile issuer is fixed"));
        }

        for claim in [
            &self.claim_mapping.email,
            &self.claim_mapping.email_verified,
            &self.claim_mapping.display_name,
            &self.claim_mapping.authentication_context,
            &self.claim_mapping.authentication_methods,
            &self.claim_mapping.authentication_time,
        ] {
            if claim.trim().is_empty() {
                return Err(OidcError::Configuration("claim mappings cannot be empty"));
            }
        }
        Ok(())
    }

    pub fn redacted(&self) -> RedactedOidcConfig {
        RedactedOidcConfig {
            profile: self.profile,
            issuer: self.issuer.clone(),
            client_id: self.client_id.clone(),
            client_secret_configured: self.client_secret.is_some(),
            audience: self.audience.clone(),
            redirect_uri: self.redirect_uri.clone(),
            scopes: self.scopes.clone(),
            allowed_signing_algorithms: self.allowed_signing_algorithms.clone(),
            authorization_ttl_seconds: self.authorization_ttl.as_secs(),
            max_pending_authorizations: self.max_pending_authorizations,
            max_http_response_bytes: self.max_http_response_bytes,
        }
    }
}

fn default_scopes() -> Vec<String> {
    vec![
        "openid".to_string(),
        "profile".to_string(),
        "email".to_string(),
    ]
}

fn parse_url(field: &'static str, value: &str) -> Result<Url, OidcError> {
    Url::parse(value).map_err(|_| {
        OidcError::Configuration(match field {
            "issuer" => "issuer must be an absolute URL",
            _ => "redirect_uri must be an absolute URL",
        })
    })
}

fn validate_secure_url(
    field: &'static str,
    value: &Url,
    allow_loopback_http: bool,
) -> Result<(), OidcError> {
    if value.scheme() == "https" {
        return Ok(());
    }
    let loopback = matches!(value.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if value.scheme() == "http" && allow_loopback_http && loopback {
        return Ok(());
    }
    Err(OidcError::Configuration(match field {
        "issuer" => "issuer must use HTTPS except for named loopback development profiles",
        _ => "redirect_uri must use HTTPS or loopback HTTP",
    }))
}

#[derive(Clone)]
pub struct PendingAuthorization {
    nonce: SecretString,
    pkce_verifier: SecretString,
    redirect_uri: String,
    expires_at: DateTime<Utc>,
}

impl PendingAuthorization {
    pub fn new(
        nonce: impl Into<String>,
        pkce_verifier: impl Into<String>,
        redirect_uri: impl Into<String>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            nonce: SecretString::new(nonce),
            pkce_verifier: SecretString::new(pkce_verifier),
            redirect_uri: redirect_uri.into(),
            expires_at,
        }
    }

    pub fn nonce_secret(&self) -> &str {
        self.nonce.expose()
    }

    pub fn pkce_verifier_secret(&self) -> &str {
        self.pkce_verifier.expose()
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }
}

impl fmt::Debug for PendingAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingAuthorization")
            .field("nonce", &"[REDACTED]")
            .field("pkce_verifier", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[async_trait]
pub trait PendingAuthorizationStore: Send + Sync {
    async fn put(
        &self,
        state_digest: [u8; 32],
        pending: PendingAuthorization,
    ) -> Result<(), OidcError>;

    async fn consume(
        &self,
        state_digest: [u8; 32],
    ) -> Result<Option<PendingAuthorization>, OidcError>;

    async fn purge_expired(&self, now: DateTime<Utc>) -> Result<(), OidcError>;
}

pub struct MemoryPendingAuthorizationStore {
    records: Mutex<HashMap<[u8; 32], PendingAuthorization>>,
    capacity: usize,
}

impl MemoryPendingAuthorizationStore {
    pub fn new(capacity: usize) -> Result<Self, OidcError> {
        if capacity == 0 {
            return Err(OidcError::Configuration(
                "pending authorization capacity must be positive",
            ));
        }
        Ok(Self {
            records: Mutex::new(HashMap::new()),
            capacity,
        })
    }
}

#[async_trait]
impl PendingAuthorizationStore for MemoryPendingAuthorizationStore {
    async fn put(
        &self,
        state_digest: [u8; 32],
        pending: PendingAuthorization,
    ) -> Result<(), OidcError> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| OidcError::StateStoreUnavailable)?;
        if records.contains_key(&state_digest) {
            return Err(OidcError::DuplicateAuthorizationState);
        }
        if records.len() >= self.capacity {
            return Err(OidcError::StateStoreCapacityExceeded);
        }
        records.insert(state_digest, pending);
        Ok(())
    }

    async fn consume(
        &self,
        state_digest: [u8; 32],
    ) -> Result<Option<PendingAuthorization>, OidcError> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| OidcError::StateStoreUnavailable)?;
        Ok(records.remove(&state_digest))
    }

    async fn purge_expired(&self, now: DateTime<Utc>) -> Result<(), OidcError> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| OidcError::StateStoreUnavailable)?;
        records.retain(|_, record| record.expires_at > now);
        Ok(())
    }
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Clone, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuthorizationRequest {
    pub authorization_url: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct AuthorizationCallback {
    pub code: SecretString,
    pub state: SecretString,
}

impl AuthorizationCallback {
    pub fn new(code: impl Into<String>, state: impl Into<String>) -> Self {
        Self {
            code: SecretString::new(code),
            state: SecretString::new(state),
        }
    }
}

impl fmt::Debug for AuthorizationCallback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationCallback")
            .field("code", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedIdentity {
    pub stable_identity_id: String,
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub display_name: Option<String>,
    pub authenticated_at: Option<DateTime<Utc>>,
    pub authentication_context: Option<String>,
    pub authentication_methods: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct AuthorizationGrant {
    pub identity: NormalizedIdentity,
    pub refresh_token: Option<SecretString>,
}

pub trait IdentityProjector: Send + Sync {
    fn project(
        &self,
        issuer: &str,
        subject: &str,
        verified_claims: &Value,
        mapping: &ClaimMapping,
    ) -> Result<NormalizedIdentity, OidcError>;
}

#[derive(Clone, Debug, Default)]
pub struct StableIdentityProjector;

impl IdentityProjector for StableIdentityProjector {
    fn project(
        &self,
        issuer: &str,
        subject: &str,
        verified_claims: &Value,
        mapping: &ClaimMapping,
    ) -> Result<NormalizedIdentity, OidcError> {
        if issuer.trim().is_empty() || subject.trim().is_empty() {
            return Err(OidcError::InvalidIdentityClaims);
        }
        Ok(NormalizedIdentity {
            stable_identity_id: stable_identity_id(issuer, subject),
            issuer: issuer.to_string(),
            subject: subject.to_string(),
            email: optional_string(verified_claims, &mapping.email)?,
            email_verified: optional_bool(verified_claims, &mapping.email_verified)?,
            display_name: optional_string(verified_claims, &mapping.display_name)?,
            authenticated_at: optional_timestamp(verified_claims, &mapping.authentication_time)?,
            authentication_context: optional_string(
                verified_claims,
                &mapping.authentication_context,
            )?,
            authentication_methods: optional_string_list(
                verified_claims,
                &mapping.authentication_methods,
            )?,
            expires_at: required_timestamp(verified_claims, "exp")?,
        })
    }
}

pub fn stable_identity_id(issuer: &str, subject: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"tradeassembly.identity.v1\0");
    digest.update(issuer.as_bytes());
    digest.update(b"\0");
    digest.update(subject.as_bytes());
    format!("identity_{:x}", digest.finalize())
}

fn optional_string(claims: &Value, name: &str) -> Result<Option<String>, OidcError> {
    match claims.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(OidcError::InvalidIdentityClaims),
    }
}

fn optional_bool(claims: &Value, name: &str) -> Result<Option<bool>, OidcError> {
    match claims.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(OidcError::InvalidIdentityClaims),
    }
}

fn optional_string_list(claims: &Value, name: &str) -> Result<Vec<String>, OidcError> {
    match claims.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or(OidcError::InvalidIdentityClaims)
            })
            .collect(),
        Some(_) => Err(OidcError::InvalidIdentityClaims),
    }
}

fn optional_timestamp(claims: &Value, name: &str) -> Result<Option<DateTime<Utc>>, OidcError> {
    let Some(value) = claims.get(name) else {
        return Ok(None);
    };
    let seconds = value.as_i64().ok_or(OidcError::InvalidIdentityClaims)?;
    DateTime::from_timestamp(seconds, 0)
        .map(Some)
        .ok_or(OidcError::InvalidIdentityClaims)
}

fn required_timestamp(claims: &Value, name: &str) -> Result<DateTime<Utc>, OidcError> {
    optional_timestamp(claims, name)?.ok_or(OidcError::InvalidIdentityClaims)
}

#[async_trait]
pub trait OidcProviderPort: Send + Sync {
    async fn begin_authorization(&self) -> Result<AuthorizationRequest, OidcError>;

    async fn complete_authorization(
        &self,
        callback: AuthorizationCallback,
    ) -> Result<NormalizedIdentity, OidcError>;
}

pub struct OidcService<S, C, P = StableIdentityProjector>
where
    S: PendingAuthorizationStore,
    C: Clock,
    P: IdentityProjector,
{
    config: OidcClientConfig,
    store: Arc<S>,
    clock: Arc<C>,
    projector: Arc<P>,
    http_client: BoundedHttpClient,
    metadata: RwLock<Option<CoreProviderMetadata>>,
}

impl<S, C> OidcService<S, C, StableIdentityProjector>
where
    S: PendingAuthorizationStore,
    C: Clock,
{
    pub fn new(config: OidcClientConfig, store: Arc<S>, clock: Arc<C>) -> Result<Self, OidcError> {
        Self::with_projector(config, store, clock, Arc::new(StableIdentityProjector))
    }
}

impl<S, C, P> OidcService<S, C, P>
where
    S: PendingAuthorizationStore,
    C: Clock,
    P: IdentityProjector,
{
    pub fn with_projector(
        config: OidcClientConfig,
        store: Arc<S>,
        clock: Arc<C>,
        projector: Arc<P>,
    ) -> Result<Self, OidcError> {
        config.validate()?;
        let http_client = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| OidcError::HttpClientInitialization)?;
        let max_response_bytes = config.max_http_response_bytes;
        Ok(Self {
            config,
            store,
            clock,
            projector,
            http_client: BoundedHttpClient {
                client: http_client,
                max_response_bytes,
            },
            metadata: RwLock::new(None),
        })
    }

    pub fn redacted_config(&self) -> RedactedOidcConfig {
        self.config.redacted()
    }

    async fn provider_metadata(&self, refresh: bool) -> Result<CoreProviderMetadata, OidcError> {
        if !refresh {
            if let Some(metadata) = self.metadata.read().await.clone() {
                return Ok(metadata);
            }
        }
        let expected_issuer = IssuerUrl::new(self.config.issuer.clone())
            .map_err(|_| OidcError::Configuration("issuer must be an absolute URL"))?;
        let discovered = CoreProviderMetadata::discover_async(expected_issuer, &self.http_client)
            .await
            .map_err(|_| OidcError::DiscoveryFailed)?;
        if discovered.issuer().as_str() != self.config.issuer {
            return Err(OidcError::IssuerMismatch);
        }
        *self.metadata.write().await = Some(discovered.clone());
        Ok(discovered)
    }

    fn client_secret(&self) -> Option<ClientSecret> {
        self.config
            .client_secret
            .as_ref()
            .map(|value| ClientSecret::new(value.expose().to_string()))
    }

    fn validate_token_shape_and_algorithm(&self, encoded: &str) -> Result<(), OidcError> {
        let mut segments = encoded.split('.');
        let header = segments.next().ok_or(OidcError::InvalidIdToken)?;
        let payload = segments.next().ok_or(OidcError::InvalidIdToken)?;
        let signature = segments.next().ok_or(OidcError::InvalidIdToken)?;
        if segments.next().is_some()
            || header.is_empty()
            || payload.is_empty()
            || signature.is_empty()
        {
            return Err(OidcError::InvalidIdToken);
        }

        let header: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(header)
                .map_err(|_| OidcError::InvalidIdToken)?,
        )
        .map_err(|_| OidcError::InvalidIdToken)?;
        let algorithm = header
            .get("alg")
            .and_then(Value::as_str)
            .ok_or(OidcError::InvalidIdToken)?;
        if !self
            .config
            .allowed_signing_algorithms
            .iter()
            .any(|allowed| allowed == algorithm)
        {
            return Err(OidcError::UnsupportedSigningAlgorithm);
        }

        Ok(())
    }

    fn verified_payload(&self, encoded: &str) -> Result<Value, OidcError> {
        let payload = encoded.split('.').nth(1).ok_or(OidcError::InvalidIdToken)?;

        serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(payload)
                .map_err(|_| OidcError::InvalidIdToken)?,
        )
        .map_err(|_| OidcError::InvalidIdToken)
    }
}

#[derive(Clone)]
struct BoundedHttpClient {
    client: reqwest::Client,
    max_response_bytes: usize,
}

#[derive(Debug, Error)]
enum BoundedHttpError {
    #[error("OIDC HTTP transport failed")]
    Transport(#[from] reqwest::Error),
    #[error("OIDC HTTP request or response was invalid")]
    Http(#[from] http::Error),
    #[error("OIDC HTTP response exceeded the configured byte limit")]
    ResponseTooLarge,
}

impl<'client> AsyncHttpClient<'client> for BoundedHttpClient {
    type Error = BoundedHttpError;
    type Future =
        Pin<Box<dyn Future<Output = Result<HttpResponse, Self::Error>> + Send + Sync + 'client>>;

    fn call(&'client self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            let request: reqwest::Request = request.try_into()?;
            let mut response = self.client.execute(request).await?;
            if response
                .content_length()
                .is_some_and(|length| length > self.max_response_bytes as u64)
            {
                return Err(BoundedHttpError::ResponseTooLarge);
            }
            let mut builder = http::Response::builder()
                .status(response.status())
                .version(response.version());
            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if body.len().saturating_add(chunk.len()) > self.max_response_bytes {
                    return Err(BoundedHttpError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(builder.body(body)?)
        })
    }
}

#[async_trait]
impl<S, C, P> OidcProviderPort for OidcService<S, C, P>
where
    S: PendingAuthorizationStore,
    C: Clock,
    P: IdentityProjector,
{
    async fn begin_authorization(&self) -> Result<AuthorizationRequest, OidcError> {
        let metadata = self.provider_metadata(false).await?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.config.client_id.clone()),
            self.client_secret(),
        )
        .set_redirect_uri(
            RedirectUrl::new(self.config.redirect_uri.clone())
                .map_err(|_| OidcError::Configuration("redirect_uri must be an absolute URL"))?,
        );

        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut request = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(challenge);
        for scope in &self.config.scopes {
            request = request.add_scope(Scope::new(scope.clone()));
        }
        let (authorization_url, state, nonce) = request.url();
        let expires_at = self.clock.now()
            + chrono::Duration::from_std(self.config.authorization_ttl)
                .map_err(|_| OidcError::Configuration("authorization_ttl is too large"))?;
        self.store.purge_expired(self.clock.now()).await?;
        self.store
            .put(
                state_digest(state.secret()),
                PendingAuthorization {
                    nonce: SecretString::new(nonce.secret().to_string()),
                    pkce_verifier: SecretString::new(verifier.secret().to_string()),
                    redirect_uri: self.config.redirect_uri.clone(),
                    expires_at,
                },
            )
            .await?;

        Ok(AuthorizationRequest {
            authorization_url: authorization_url.to_string(),
            expires_at,
        })
    }

    async fn complete_authorization(
        &self,
        callback: AuthorizationCallback,
    ) -> Result<NormalizedIdentity, OidcError> {
        let pending = self
            .store
            .consume(state_digest(callback.state.expose()))
            .await?
            .ok_or(OidcError::AuthorizationStateInvalid)?;
        if pending.expires_at <= self.clock.now() {
            return Err(OidcError::AuthorizationStateExpired);
        }
        if pending.redirect_uri != self.config.redirect_uri {
            return Err(OidcError::RedirectMismatch);
        }

        let metadata = self.provider_metadata(false).await?;
        let mut client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.config.client_id.clone()),
            self.client_secret(),
        )
        .set_redirect_uri(
            RedirectUrl::new(self.config.redirect_uri.clone())
                .map_err(|_| OidcError::Configuration("redirect_uri must be an absolute URL"))?,
        );
        let token_response = client
            .exchange_code(AuthorizationCode::new(callback.code.expose().to_string()))
            .map_err(|_| OidcError::TokenEndpointUnavailable)?
            .set_pkce_verifier(PkceCodeVerifier::new(
                pending.pkce_verifier.expose().to_string(),
            ))
            .request_async(&self.http_client)
            .await
            .map_err(|_| OidcError::TokenExchangeFailed)?;
        let id_token = token_response.id_token().ok_or(OidcError::MissingIdToken)?;
        let encoded = id_token.to_string();
        self.validate_token_shape_and_algorithm(&encoded)?;
        let nonce = Nonce::new(pending.nonce.expose().to_string());

        let refreshed = self.provider_metadata(true).await?;
        client = CoreClient::from_provider_metadata(
            refreshed,
            ClientId::new(self.config.client_id.clone()),
            self.client_secret(),
        )
        .set_redirect_uri(
            RedirectUrl::new(self.config.redirect_uri.clone())
                .map_err(|_| OidcError::Configuration("redirect_uri must be an absolute URL"))?,
        );

        let validation_time = self.clock.now();
        let issue_time_limit =
            validation_time + chrono::Duration::seconds(MAX_TOKEN_CLOCK_SKEW_SECONDS);
        let initial_token_is_valid = {
            let initial_verifier = client
                .id_token_verifier()
                .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
                .set_time_fn(move || validation_time)
                .set_issue_time_verifier_fn(move |issued_at| {
                    if issued_at <= issue_time_limit {
                        Ok(())
                    } else {
                        Err("ID token issue time is in the future".to_string())
                    }
                });
            id_token.claims(&initial_verifier, &nonce).is_ok()
        };
        if !initial_token_is_valid {
            let refreshed = self.provider_metadata(true).await?;
            client = CoreClient::from_provider_metadata(
                refreshed,
                ClientId::new(self.config.client_id.clone()),
                self.client_secret(),
            )
            .set_redirect_uri(
                RedirectUrl::new(self.config.redirect_uri.clone()).map_err(|_| {
                    OidcError::Configuration("redirect_uri must be an absolute URL")
                })?,
            );
        }
        let validation_time = self.clock.now();
        let issue_time_limit =
            validation_time + chrono::Duration::seconds(MAX_TOKEN_CLOCK_SKEW_SECONDS);
        let verifier = client
            .id_token_verifier()
            .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
            .set_time_fn(move || validation_time)
            .set_issue_time_verifier_fn(move |issued_at| {
                if issued_at <= issue_time_limit {
                    Ok(())
                } else {
                    Err("ID token issue time is in the future".to_string())
                }
            });
        let verified = id_token
            .claims(&verifier, &nonce)
            .map_err(|_| OidcError::IdTokenVerificationFailed)?;

        if !verified
            .audiences()
            .iter()
            .any(|audience| audience.as_str() == self.config.audience)
        {
            return Err(OidcError::AudienceMismatch);
        }
        if let Some(expected_hash) = verified.access_token_hash() {
            let actual_hash = AccessTokenHash::from_token(
                token_response.access_token(),
                id_token
                    .signing_alg()
                    .map_err(|_| OidcError::InvalidIdToken)?,
                id_token
                    .signing_key(&verifier)
                    .map_err(|_| OidcError::IdTokenVerificationFailed)?,
            )
            .map_err(|_| OidcError::InvalidIdToken)?;
            if actual_hash != *expected_hash {
                return Err(OidcError::AccessTokenSubstitution);
            }
        }

        let raw_claims = self.verified_payload(&encoded)?;

        self.projector.project(
            self.config.issuer.as_str(),
            verified.subject().as_str(),
            &raw_claims,
            &self.config.claim_mapping,
        )
    }
}

impl<S, C, P> OidcService<S, C, P>
where
    S: PendingAuthorizationStore,
    C: Clock,
    P: IdentityProjector,
{
    /// Completes an authorization while retaining the refresh credential for a
    /// caller-owned encrypted session store.
    pub async fn complete_authorization_grant(
        &self,
        callback: AuthorizationCallback,
    ) -> Result<AuthorizationGrant, OidcError> {
        let pending = self
            .store
            .consume(state_digest(callback.state.expose()))
            .await?
            .ok_or(OidcError::AuthorizationStateInvalid)?;
        if pending.expires_at <= self.clock.now() {
            return Err(OidcError::AuthorizationStateExpired);
        }
        if pending.redirect_uri != self.config.redirect_uri {
            return Err(OidcError::RedirectMismatch);
        }

        let metadata = self.provider_metadata(false).await?;
        let mut client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.config.client_id.clone()),
            self.client_secret(),
        )
        .set_redirect_uri(
            RedirectUrl::new(self.config.redirect_uri.clone())
                .map_err(|_| OidcError::Configuration("redirect_uri must be an absolute URL"))?,
        );
        let token_response = client
            .exchange_code(AuthorizationCode::new(callback.code.expose().to_string()))
            .map_err(|_| OidcError::TokenEndpointUnavailable)?
            .set_pkce_verifier(PkceCodeVerifier::new(
                pending.pkce_verifier.expose().to_string(),
            ))
            .request_async(&self.http_client)
            .await
            .map_err(|_| OidcError::TokenExchangeFailed)?;
        let id_token = token_response.id_token().ok_or(OidcError::MissingIdToken)?;
        let encoded = id_token.to_string();
        self.validate_token_shape_and_algorithm(&encoded)?;
        let nonce = Nonce::new(pending.nonce.expose().to_string());

        let refreshed = self.provider_metadata(true).await?;
        client = CoreClient::from_provider_metadata(
            refreshed,
            ClientId::new(self.config.client_id.clone()),
            self.client_secret(),
        )
        .set_redirect_uri(
            RedirectUrl::new(self.config.redirect_uri.clone())
                .map_err(|_| OidcError::Configuration("redirect_uri must be an absolute URL"))?,
        );

        let validation_time = self.clock.now();
        let issue_time_limit =
            validation_time + chrono::Duration::seconds(MAX_TOKEN_CLOCK_SKEW_SECONDS);
        let initial_token_is_valid = {
            let initial_verifier = client
                .id_token_verifier()
                .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
                .set_time_fn(move || validation_time)
                .set_issue_time_verifier_fn(move |issued_at| {
                    if issued_at <= issue_time_limit {
                        Ok(())
                    } else {
                        Err("ID token issue time is in the future".to_string())
                    }
                });
            id_token.claims(&initial_verifier, &nonce).is_ok()
        };
        if !initial_token_is_valid {
            let refreshed = self.provider_metadata(true).await?;
            client = CoreClient::from_provider_metadata(
                refreshed,
                ClientId::new(self.config.client_id.clone()),
                self.client_secret(),
            )
            .set_redirect_uri(
                RedirectUrl::new(self.config.redirect_uri.clone()).map_err(|_| {
                    OidcError::Configuration("redirect_uri must be an absolute URL")
                })?,
            );
        }
        let validation_time = self.clock.now();
        let issue_time_limit =
            validation_time + chrono::Duration::seconds(MAX_TOKEN_CLOCK_SKEW_SECONDS);
        let verifier = client
            .id_token_verifier()
            .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
            .set_time_fn(move || validation_time)
            .set_issue_time_verifier_fn(move |issued_at| {
                if issued_at <= issue_time_limit {
                    Ok(())
                } else {
                    Err("ID token issue time is in the future".to_string())
                }
            });
        let verified = id_token
            .claims(&verifier, &nonce)
            .map_err(|_| OidcError::IdTokenVerificationFailed)?;

        if !verified
            .audiences()
            .iter()
            .any(|audience| audience.as_str() == self.config.audience)
        {
            return Err(OidcError::AudienceMismatch);
        }
        if let Some(expected_hash) = verified.access_token_hash() {
            let actual_hash = AccessTokenHash::from_token(
                token_response.access_token(),
                id_token
                    .signing_alg()
                    .map_err(|_| OidcError::InvalidIdToken)?,
                id_token
                    .signing_key(&verifier)
                    .map_err(|_| OidcError::IdTokenVerificationFailed)?,
            )
            .map_err(|_| OidcError::InvalidIdToken)?;
            if actual_hash != *expected_hash {
                return Err(OidcError::AccessTokenSubstitution);
            }
        }

        let raw_claims = self.verified_payload(&encoded)?;
        let identity = self.projector.project(
            self.config.issuer.as_str(),
            verified.subject().as_str(),
            &raw_claims,
            &self.config.claim_mapping,
        )?;
        let refresh_token = token_response
            .refresh_token()
            .map(|token| SecretString::new(token.secret().to_string()));
        Ok(AuthorizationGrant {
            identity,
            refresh_token,
        })
    }

    pub async fn refresh_authorization_grant(
        &self,
        refresh_token: &SecretString,
        expected_issuer: &str,
        expected_subject: &str,
    ) -> Result<AuthorizationGrant, OidcError> {
        let metadata = self.provider_metadata(true).await?;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(self.config.client_id.clone()),
            self.client_secret(),
        )
        .set_redirect_uri(
            RedirectUrl::new(self.config.redirect_uri.clone())
                .map_err(|_| OidcError::Configuration("redirect_uri must be an absolute URL"))?,
        );
        let original_refresh = RefreshToken::new(refresh_token.expose_secret().to_string());
        let token_response = client
            .exchange_refresh_token(&original_refresh)
            .map_err(|_| OidcError::TokenEndpointUnavailable)?
            .request_async(&self.http_client)
            .await
            .map_err(|_| OidcError::TokenExchangeFailed)?;
        let id_token = token_response.id_token().ok_or(OidcError::MissingIdToken)?;
        let encoded = id_token.to_string();
        self.validate_token_shape_and_algorithm(&encoded)?;

        let validation_time = self.clock.now();
        let issue_time_limit =
            validation_time + chrono::Duration::seconds(MAX_TOKEN_CLOCK_SKEW_SECONDS);
        let verifier = client
            .id_token_verifier()
            .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
            .set_time_fn(move || validation_time)
            .set_issue_time_verifier_fn(move |issued_at| {
                if issued_at <= issue_time_limit {
                    Ok(())
                } else {
                    Err("ID token issue time is in the future".to_string())
                }
            });
        let verified = id_token
            .claims(&verifier, |nonce: Option<&Nonce>| {
                if nonce.is_none() {
                    Ok(())
                } else {
                    Err("refreshed ID token must not introduce a nonce".to_string())
                }
            })
            .map_err(|_| OidcError::IdTokenVerificationFailed)?;
        if !verified
            .audiences()
            .iter()
            .any(|audience| audience.as_str() == self.config.audience)
        {
            return Err(OidcError::AudienceMismatch);
        }
        if let Some(expected_hash) = verified.access_token_hash() {
            let actual_hash = AccessTokenHash::from_token(
                token_response.access_token(),
                id_token
                    .signing_alg()
                    .map_err(|_| OidcError::InvalidIdToken)?,
                id_token
                    .signing_key(&verifier)
                    .map_err(|_| OidcError::IdTokenVerificationFailed)?,
            )
            .map_err(|_| OidcError::InvalidIdToken)?;
            if actual_hash != *expected_hash {
                return Err(OidcError::AccessTokenSubstitution);
            }
        }
        let raw_claims = self.verified_payload(&encoded)?;
        let identity = self.projector.project(
            self.config.issuer.as_str(),
            verified.subject().as_str(),
            &raw_claims,
            &self.config.claim_mapping,
        )?;
        if identity.issuer != expected_issuer || identity.subject != expected_subject {
            return Err(OidcError::IdentityChanged);
        }
        let refresh_token = token_response
            .refresh_token()
            .map(|token| SecretString::new(token.secret().to_string()))
            .or_else(|| Some(refresh_token.clone()));
        Ok(AuthorizationGrant {
            identity,
            refresh_token,
        })
    }
}

fn state_digest(state: &str) -> [u8; 32] {
    Sha256::digest(state.as_bytes()).into()
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum OidcError {
    #[error("invalid OIDC configuration: {0}")]
    Configuration(&'static str),
    #[error("OIDC HTTP client initialization failed")]
    HttpClientInitialization,
    #[error("OIDC discovery failed")]
    DiscoveryFailed,
    #[error("OIDC issuer mismatch")]
    IssuerMismatch,
    #[error("pending authorization store unavailable")]
    StateStoreUnavailable,
    #[error("pending authorization store capacity exceeded")]
    StateStoreCapacityExceeded,
    #[error("duplicate authorization state")]
    DuplicateAuthorizationState,
    #[error("authorization state is invalid or already consumed")]
    AuthorizationStateInvalid,
    #[error("authorization state expired")]
    AuthorizationStateExpired,
    #[error("authorization redirect binding mismatch")]
    RedirectMismatch,
    #[error("OIDC token endpoint unavailable")]
    TokenEndpointUnavailable,
    #[error("OIDC token exchange failed")]
    TokenExchangeFailed,
    #[error("OIDC token response omitted the ID token")]
    MissingIdToken,
    #[error("OIDC ID token is invalid")]
    InvalidIdToken,
    #[error("OIDC ID token signing algorithm is not allowed")]
    UnsupportedSigningAlgorithm,
    #[error("OIDC ID token verification failed")]
    IdTokenVerificationFailed,
    #[error("OIDC ID token audience mismatch")]
    AudienceMismatch,
    #[error("OIDC access token substitution detected")]
    AccessTokenSubstitution,
    #[error("OIDC identity claims are invalid")]
    InvalidIdentityClaims,
    #[error("OIDC refresh changed the authenticated identity")]
    IdentityChanged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hub_is_the_default_named_profile() {
        let config = OidcClientConfig::hub_production(
            "tradeassembly-local",
            "http://127.0.0.1:3000/auth/callback",
        );
        config.validate().unwrap();
        assert_eq!(OidcProfile::TradeAssemblyHubProduction, config.profile);
        assert_eq!(HUB_PRODUCTION_ISSUER, config.issuer);
        assert_eq!("tradeassembly-local", config.audience);
    }

    #[test]
    fn configured_http_issuer_is_rejected() {
        let config = OidcClientConfig::configured(
            "http://issuer.example.com",
            "client",
            "client",
            "http://127.0.0.1:3000/auth/callback",
        );
        assert!(matches!(
            config.validate(),
            Err(OidcError::Configuration(_))
        ));
    }

    #[test]
    fn only_named_local_profiles_allow_loopback_http_issuer() {
        let config = OidcClientConfig::keycloak_local(
            "http://127.0.0.1:8080/realms/tradeassembly-dev",
            "tradeassembly-local",
            "http://127.0.0.1:3000/auth/callback",
        );
        config.validate().unwrap();

        let mut configured = OidcClientConfig::configured(
            "http://127.0.0.1:8080/realms/tradeassembly-dev",
            "tradeassembly-local",
            "tradeassembly-local",
            "http://127.0.0.1:3000/auth/callback",
        );
        configured.allow_insecure_loopback_issuer = true;
        assert!(configured.validate().is_err());
    }

    #[test]
    fn identity_projection_keys_on_issuer_and_subject_not_email() {
        let projector = StableIdentityProjector;
        let mapping = ClaimMapping::default();
        let expires_at = Utc::now().timestamp() + 300;
        let first = projector
            .project(
                "https://issuer-a.example",
                "subject-1",
                &json!({
                    "email": "first@example.com",
                    "email_verified": true,
                    "exp": expires_at
                }),
                &mapping,
            )
            .unwrap();
        let email_change = projector
            .project(
                "https://issuer-a.example",
                "subject-1",
                &json!({
                    "email": "second@example.com",
                    "email_verified": true,
                    "exp": expires_at
                }),
                &mapping,
            )
            .unwrap();
        let other_issuer = projector
            .project(
                "https://issuer-b.example",
                "subject-1",
                &json!({
                    "email": "first@example.com",
                    "email_verified": true,
                    "exp": expires_at
                }),
                &mapping,
            )
            .unwrap();
        let other_subject = projector
            .project(
                "https://issuer-a.example",
                "subject-2",
                &json!({
                    "email": "first@example.com",
                    "email_verified": true,
                    "exp": expires_at
                }),
                &mapping,
            )
            .unwrap();

        assert_eq!(first.stable_identity_id, email_change.stable_identity_id);
        assert_ne!(first.stable_identity_id, other_issuer.stable_identity_id);
        assert_ne!(first.stable_identity_id, other_subject.stable_identity_id);
    }

    #[test]
    fn diagnostics_redact_client_secret_and_callback_material() {
        let mut config = OidcClientConfig::hub_production(
            "tradeassembly-local",
            "http://127.0.0.1:3000/auth/callback",
        );
        config.client_secret = Some(SecretString::new("never-print-this"));
        let config_debug = format!("{config:?}");
        let callback_debug = format!(
            "{:?}",
            AuthorizationCallback::new("code-never-print", "state-never-print")
        );

        assert!(!config_debug.contains("never-print-this"));
        assert!(!callback_debug.contains("code-never-print"));
        assert!(!callback_debug.contains("state-never-print"));
        assert!(config_debug.contains("client_secret_configured: true"));
    }

    #[tokio::test]
    async fn pending_state_is_consumed_once() {
        let store = MemoryPendingAuthorizationStore::new(1).unwrap();
        let digest = state_digest("state");
        let pending = PendingAuthorization {
            nonce: SecretString::new("nonce"),
            pkce_verifier: SecretString::new("verifier"),
            redirect_uri: "http://127.0.0.1/callback".to_string(),
            expires_at: Utc::now(),
        };
        store.put(digest, pending).await.unwrap();
        assert!(store.consume(digest).await.unwrap().is_some());
        assert!(store.consume(digest).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn duplicate_state_does_not_replace_the_original_record() {
        let store = MemoryPendingAuthorizationStore::new(1).unwrap();
        let digest = state_digest("state");
        let original = PendingAuthorization {
            nonce: SecretString::new("original-nonce"),
            pkce_verifier: SecretString::new("original-verifier"),
            redirect_uri: "http://127.0.0.1/callback".to_string(),
            expires_at: Utc::now() + chrono::Duration::minutes(1),
        };
        let replacement = PendingAuthorization {
            nonce: SecretString::new("replacement-nonce"),
            pkce_verifier: SecretString::new("replacement-verifier"),
            redirect_uri: "http://127.0.0.1/other".to_string(),
            expires_at: Utc::now() + chrono::Duration::minutes(2),
        };

        store.put(digest, original).await.unwrap();
        assert_eq!(
            Err(OidcError::DuplicateAuthorizationState),
            store.put(digest, replacement).await
        );
        let consumed = store.consume(digest).await.unwrap().unwrap();
        assert_eq!("original-nonce", consumed.nonce.expose());
        assert_eq!("http://127.0.0.1/callback", consumed.redirect_uri);
    }

    #[tokio::test]
    async fn expired_records_can_be_purged_to_release_bounded_capacity() {
        let store = MemoryPendingAuthorizationStore::new(1).unwrap();
        let now = Utc::now();
        store
            .put(
                state_digest("expired"),
                PendingAuthorization {
                    nonce: SecretString::new("nonce"),
                    pkce_verifier: SecretString::new("verifier"),
                    redirect_uri: "http://127.0.0.1/callback".to_string(),
                    expires_at: now - chrono::Duration::seconds(1),
                },
            )
            .await
            .unwrap();
        store.purge_expired(now).await.unwrap();
        store
            .put(
                state_digest("new"),
                PendingAuthorization {
                    nonce: SecretString::new("new-nonce"),
                    pkce_verifier: SecretString::new("new-verifier"),
                    redirect_uri: "http://127.0.0.1/callback".to_string(),
                    expires_at: now + chrono::Duration::minutes(1),
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn expired_or_redirect_mismatched_state_fails_before_provider_io() {
        let now = Utc::now();
        let config = OidcClientConfig::oidc_server_mock(
            "http://127.0.0.1:9",
            "client",
            "http://127.0.0.1/callback",
        );

        for (state, expires_at, redirect_uri, expected) in [
            (
                "expired",
                now - chrono::Duration::seconds(1),
                "http://127.0.0.1/callback",
                OidcError::AuthorizationStateExpired,
            ),
            (
                "redirect-mismatch",
                now + chrono::Duration::minutes(1),
                "http://127.0.0.1/other",
                OidcError::RedirectMismatch,
            ),
        ] {
            let store = Arc::new(MemoryPendingAuthorizationStore::new(1).unwrap());
            store
                .put(
                    state_digest(state),
                    PendingAuthorization {
                        nonce: SecretString::new("nonce"),
                        pkce_verifier: SecretString::new("verifier"),
                        redirect_uri: redirect_uri.to_string(),
                        expires_at,
                    },
                )
                .await
                .unwrap();
            let service = OidcService::new(config.clone(), store, Arc::new(SystemClock)).unwrap();
            let result = service
                .complete_authorization(AuthorizationCallback::new("code", state))
                .await;
            assert_eq!(Err(expected), result);
        }
    }
}
