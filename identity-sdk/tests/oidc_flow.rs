// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use openidconnect::core::{CoreJsonWebKeySet, CoreRsaPrivateSigningKey};
use openidconnect::{JsonWebKeyId, PrivateSigningKey};
use rand::rngs::OsRng;
use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey, LineEnding};
use rsa::{Pkcs1v15Sign, RsaPrivateKey};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, OnceLock};
use tradeassembly_identity_sdk::{
    AuthorizationCallback, MemoryPendingAuthorizationStore, OidcClientConfig, OidcError,
    OidcProviderPort, OidcService, SecretString, SystemClock, DEFAULT_MAX_HTTP_RESPONSE_BYTES,
};
use url::Url;

const CLIENT_ID: &str = "tradeassembly-test";
const REDIRECT_URI: &str = "http://127.0.0.1:3000/auth/callback";
const ACCESS_TOKEN: &str = "test-access-token";

#[derive(Clone, Copy, Debug, Default)]
enum TokenFault {
    #[default]
    None,
    WrongNonce,
    WrongAudience,
    WrongIssuer,
    Expired,
    FutureIssued,
    MissingSubject,
    InvalidEmailClaim,
    InvalidSignature,
    UnsupportedAlgorithm,
    AccessTokenSubstitution,
    OversizedDiscovery,
    RefreshSubjectChanged,
}

#[derive(Clone)]
struct KeyMaterial {
    pem: String,
    kid: String,
}

struct IssuerState {
    issuer: String,
    published_key: KeyMaterial,
    signing_key: KeyMaterial,
    expected_nonce: Option<String>,
    expected_challenge: Option<String>,
    fault: TokenFault,
    token_calls: usize,
}

#[derive(Clone)]
struct SharedIssuer(Arc<Mutex<IssuerState>>);

struct TestIssuer {
    state: SharedIssuer,
    service: Arc<OidcService<MemoryPendingAuthorizationStore, SystemClock>>,
}

impl TestIssuer {
    async fn start(fault: TokenFault) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test issuer");
        let address = listener.local_addr().expect("test issuer address");
        let issuer = format!("http://{address}");
        let (first, _) = test_keys();
        let state = SharedIssuer(Arc::new(Mutex::new(IssuerState {
            issuer: issuer.clone(),
            published_key: first.clone(),
            signing_key: first.clone(),
            expected_nonce: None,
            expected_challenge: None,
            fault,
            token_calls: 0,
        })));
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/authorize", get(|| async { StatusCode::NO_CONTENT }))
            .route("/token", post(token))
            .route("/jwks", get(jwks))
            .with_state(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test issuer");
        });

        let config = OidcClientConfig::oidc_server_mock(&issuer, CLIENT_ID, REDIRECT_URI);
        let store = Arc::new(MemoryPendingAuthorizationStore::new(16).unwrap());
        let service = Arc::new(OidcService::new(config, store, Arc::new(SystemClock)).unwrap());
        Self { state, service }
    }

    async fn begin(&self) -> (String, String) {
        let request = self.service.begin_authorization().await.unwrap();
        let url = Url::parse(&request.authorization_url).unwrap();
        let query = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            Some("S256"),
            query.get("code_challenge_method").map(|v| v.as_ref())
        );
        let state = query.get("state").expect("state").to_string();
        let nonce = query.get("nonce").expect("nonce").to_string();
        let challenge = query
            .get("code_challenge")
            .expect("code challenge")
            .to_string();
        let mut issuer = self.state.0.lock().unwrap();
        issuer.expected_nonce = Some(nonce);
        issuer.expected_challenge = Some(challenge);
        (state, "valid-code".to_string())
    }

    fn rotate_signing_key(&self) {
        let (_, second) = test_keys();
        let mut issuer = self.state.0.lock().unwrap();
        issuer.published_key = second.clone();
        issuer.signing_key = second.clone();
    }

    fn revoke_cached_signing_key(&self) {
        let (_, second) = test_keys();
        let mut issuer = self.state.0.lock().unwrap();
        issuer.published_key = second.clone();
    }

    fn token_calls(&self) -> usize {
        self.state.0.lock().unwrap().token_calls
    }
}

async fn discovery(State(state): State<SharedIssuer>) -> Json<Value> {
    let (issuer, fault) = {
        let state = state.0.lock().unwrap();
        (state.issuer.clone(), state.fault)
    };
    let mut metadata = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"]
    });
    if matches!(fault, TokenFault::OversizedDiscovery) {
        metadata["padding"] = json!("x".repeat(DEFAULT_MAX_HTTP_RESPONSE_BYTES));
    }
    Json(metadata)
}

async fn jwks(State(state): State<SharedIssuer>) -> Json<CoreJsonWebKeySet> {
    let key = state.0.lock().unwrap().published_key.clone();
    let signing_key = signing_key(&key);
    Json(CoreJsonWebKeySet::new(vec![
        signing_key.as_verification_key()
    ]))
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    code: Option<String>,
    code_verifier: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    refresh_token: Option<String>,
}

async fn token(State(state): State<SharedIssuer>, Form(form): Form<TokenForm>) -> Response {
    let (issuer, key, expected_nonce, expected_challenge, fault) = {
        let mut state = state.0.lock().unwrap();
        state.token_calls += 1;
        (
            state.issuer.clone(),
            state.signing_key.clone(),
            state.expected_nonce.clone(),
            state.expected_challenge.clone(),
            state.fault,
        )
    };
    let is_refresh = form.grant_type == "refresh_token";
    if is_refresh {
        if form.refresh_token.as_deref() != Some("refresh-1")
            || form.client_id.as_deref() != Some(CLIENT_ID)
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    } else {
        let Some(code_verifier) = form.code_verifier.as_deref() else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        if form.grant_type != "authorization_code"
            || form.code.as_deref() != Some("valid-code")
            || form.redirect_uri.as_deref() != Some(REDIRECT_URI)
            || form.client_id.as_deref() != Some(CLIENT_ID)
            || expected_challenge.as_deref() != Some(challenge.as_str())
        {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    }

    let nonce = (!is_refresh).then(|| match fault {
        TokenFault::WrongNonce => "wrong-nonce".to_string(),
        _ => expected_nonce.expect("authorization nonce armed"),
    });
    let audience = match fault {
        TokenFault::WrongAudience => "other-client",
        _ => CLIENT_ID,
    };
    let token_issuer = match fault {
        TokenFault::WrongIssuer => format!("{issuer}/other"),
        _ => issuer,
    };
    let now = Utc::now().timestamp();
    let (issued_at, expires_at) = match fault {
        TokenFault::Expired => (now - 600, now - 300),
        TokenFault::FutureIssued => (now + 600, now + 900),
        _ => (now, now + 300),
    };
    let hash_input = match fault {
        TokenFault::AccessTokenSubstitution => "different-access-token",
        _ => ACCESS_TOKEN,
    };
    let access_hash = Sha256::digest(hash_input.as_bytes());
    let mut claims = json!({
        "iss": token_issuer,
        "aud": [audience],
        "exp": expires_at,
        "iat": issued_at,
        "auth_time": now,
        "nonce": nonce,
        "acr": "urn:tradeassembly:test",
        "amr": ["pwd"],
        "email": "trader@example.com",
        "email_verified": true,
        "at_hash": URL_SAFE_NO_PAD.encode(&access_hash[..access_hash.len() / 2])
    });
    if !matches!(fault, TokenFault::MissingSubject) {
        claims["sub"] = json!(
            if is_refresh && matches!(fault, TokenFault::RefreshSubjectChanged) {
                "user-456"
            } else {
                "user-123"
            }
        );
    }
    if let Some(nonce) = nonce {
        claims["nonce"] = json!(nonce);
    } else {
        claims.as_object_mut().unwrap().remove("nonce");
    }
    if matches!(fault, TokenFault::InvalidEmailClaim) {
        claims["email"] = json!(42);
    }
    let signing_material = if matches!(fault, TokenFault::InvalidSignature) {
        test_keys().1.clone()
    } else {
        key
    };
    let algorithm = if matches!(fault, TokenFault::UnsupportedAlgorithm) {
        "HS256"
    } else {
        "RS256"
    };
    let id_token = encode_id_token(&signing_material, algorithm, &claims);

    Json(json!({
        "access_token": ACCESS_TOKEN,
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token,
        "refresh_token": if is_refresh { "refresh-2" } else { "refresh-1" }
    }))
    .into_response()
}

fn encode_id_token(key: &KeyMaterial, algorithm: &str, claims: &Value) -> String {
    let header = json!({"alg": algorithm, "kid": key.kid, "typ": "JWT"});
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
    let signing_input = format!("{header}.{claims}");
    let private_key = RsaPrivateKey::from_pkcs1_pem(&key.pem).unwrap();
    let digest = Sha256::digest(signing_input.as_bytes());
    let signature = private_key
        .sign_with_rng(&mut OsRng, Pkcs1v15Sign::new::<Sha256>(), digest.as_slice())
        .unwrap();
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

fn signing_key(key: &KeyMaterial) -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(&key.pem, Some(JsonWebKeyId::new(key.kid.clone()))).unwrap()
}

fn test_keys() -> &'static (KeyMaterial, KeyMaterial) {
    static KEYS: OnceLock<(KeyMaterial, KeyMaterial)> = OnceLock::new();
    KEYS.get_or_init(|| (generate_key("key-1"), generate_key("key-2")))
}

fn generate_key(kid: &str) -> KeyMaterial {
    let key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
    KeyMaterial {
        pem: key.to_pkcs1_pem(LineEnding::LF).unwrap().to_string(),
        kid: kid.to_string(),
    }
}

#[tokio::test]
async fn authorization_code_flow_validates_pkce_and_returns_normalized_identity() {
    let issuer = TestIssuer::start(TokenFault::None).await;
    let (state, code) = issuer.begin().await;
    let identity = issuer
        .service
        .complete_authorization(AuthorizationCallback::new(code, state))
        .await
        .unwrap();

    assert_eq!("user-123", identity.subject);
    assert_eq!(Some("trader@example.com"), identity.email.as_deref());
    assert_eq!(Some(true), identity.email_verified);
    assert_eq!(
        Some("urn:tradeassembly:test"),
        identity.authentication_context.as_deref()
    );
    assert_eq!(vec!["pwd"], identity.authentication_methods);
    assert_eq!(1, issuer.token_calls());
}

#[tokio::test]
async fn refresh_grant_rotates_credential_and_preserves_identity() {
    let issuer = TestIssuer::start(TokenFault::None).await;
    let (state, code) = issuer.begin().await;
    let grant = issuer
        .service
        .complete_authorization_grant(AuthorizationCallback::new(code, state))
        .await
        .unwrap();
    let refresh = grant.refresh_token.as_ref().expect("refresh token");
    assert_eq!("refresh-1", refresh.expose_secret());

    let refreshed = issuer
        .service
        .refresh_authorization_grant(refresh, &grant.identity.issuer, &grant.identity.subject)
        .await
        .unwrap();
    assert_eq!(
        grant.identity.stable_identity_id,
        refreshed.identity.stable_identity_id
    );
    assert_eq!(
        "refresh-2",
        refreshed
            .refresh_token
            .as_ref()
            .expect("rotated refresh token")
            .expose_secret()
    );
    assert_eq!(2, issuer.token_calls());
}

#[tokio::test]
async fn refresh_fails_closed_for_invalid_credential_or_changed_subject() {
    let issuer = TestIssuer::start(TokenFault::None).await;
    let expected_issuer = issuer.state.0.lock().unwrap().issuer.clone();
    assert!(matches!(
        issuer
            .service
            .refresh_authorization_grant(
                &SecretString::new("invalid-refresh"),
                &expected_issuer,
                "user-123",
            )
            .await,
        Err(OidcError::TokenExchangeFailed)
    ));

    let issuer = TestIssuer::start(TokenFault::RefreshSubjectChanged).await;
    let (state, code) = issuer.begin().await;
    let grant = issuer
        .service
        .complete_authorization_grant(AuthorizationCallback::new(code, state))
        .await
        .unwrap();
    assert!(matches!(
        issuer
            .service
            .refresh_authorization_grant(
                grant.refresh_token.as_ref().unwrap(),
                &grant.identity.issuer,
                &grant.identity.subject,
            )
            .await,
        Err(OidcError::IdentityChanged)
    ));
}

#[tokio::test]
async fn callback_state_is_single_use_under_replay_and_concurrency() {
    let issuer = TestIssuer::start(TokenFault::None).await;
    let (state, code) = issuer.begin().await;
    let first = issuer
        .service
        .complete_authorization(AuthorizationCallback::new(code.clone(), state.clone()));
    let second = issuer
        .service
        .complete_authorization(AuthorizationCallback::new(code.clone(), state.clone()));
    let (first, second) = tokio::join!(first, second);
    assert_eq!(1, usize::from(first.is_ok()) + usize::from(second.is_ok()));
    assert_eq!(1, issuer.token_calls());

    let replay = issuer
        .service
        .complete_authorization(AuthorizationCallback::new(code, state))
        .await;
    assert_eq!(Err(OidcError::AuthorizationStateInvalid), replay);
    assert_eq!(1, issuer.token_calls());
}

#[tokio::test]
async fn tampered_callback_state_is_rejected_before_token_exchange() {
    let issuer = TestIssuer::start(TokenFault::None).await;
    issuer.begin().await;
    let result = issuer
        .service
        .complete_authorization(AuthorizationCallback::new("valid-code", "tampered-state"))
        .await;
    assert_eq!(Err(OidcError::AuthorizationStateInvalid), result);
    assert_eq!(0, issuer.token_calls());
}

#[tokio::test]
async fn signed_token_validation_fails_closed_for_security_faults() {
    let cases = [
        TokenFault::WrongNonce,
        TokenFault::WrongAudience,
        TokenFault::WrongIssuer,
        TokenFault::Expired,
        TokenFault::FutureIssued,
        TokenFault::MissingSubject,
        TokenFault::InvalidEmailClaim,
        TokenFault::InvalidSignature,
        TokenFault::UnsupportedAlgorithm,
        TokenFault::AccessTokenSubstitution,
    ];
    for fault in cases {
        let issuer = TestIssuer::start(fault).await;
        let (state, code) = issuer.begin().await;
        let result = issuer
            .service
            .complete_authorization(AuthorizationCallback::new(code, state))
            .await;
        assert!(
            result.is_err(),
            "{fault:?} unexpectedly returned an identity"
        );
    }
}

#[tokio::test]
async fn signing_key_rotation_refreshes_discovery_and_jwks() {
    let issuer = TestIssuer::start(TokenFault::None).await;
    let (state, code) = issuer.begin().await;
    issuer.rotate_signing_key();

    let identity = issuer
        .service
        .complete_authorization(AuthorizationCallback::new(code, state))
        .await
        .unwrap();
    assert_eq!("user-123", identity.subject);
}

#[tokio::test]
async fn signing_key_removed_from_current_jwks_is_rejected() {
    let issuer = TestIssuer::start(TokenFault::None).await;
    let (state, code) = issuer.begin().await;
    issuer.revoke_cached_signing_key();

    let result = issuer
        .service
        .complete_authorization(AuthorizationCallback::new(code, state))
        .await;
    assert_eq!(Err(OidcError::IdTokenVerificationFailed), result);
}

#[tokio::test]
async fn oversized_discovery_response_is_rejected() {
    let issuer = TestIssuer::start(TokenFault::OversizedDiscovery).await;
    assert_eq!(
        Err(OidcError::DiscoveryFailed),
        issuer.service.begin_authorization().await
    );
}

#[tokio::test]
async fn external_oidc_server_mock_uses_the_same_provider_port_when_configured() {
    let Ok(issuer) = std::env::var("TRADEASSEMBLY_OIDC_MOCK_ISSUER") else {
        return;
    };
    let client_id = std::env::var("TRADEASSEMBLY_OIDC_MOCK_CLIENT_ID")
        .unwrap_or_else(|_| "tradeassembly-test".to_string());
    let redirect_uri = std::env::var("TRADEASSEMBLY_OIDC_MOCK_REDIRECT_URI")
        .unwrap_or_else(|_| REDIRECT_URI.to_string());
    let config = OidcClientConfig::oidc_server_mock(issuer, client_id, redirect_uri);
    let service = OidcService::new(
        config,
        Arc::new(MemoryPendingAuthorizationStore::new(16).unwrap()),
        Arc::new(SystemClock),
    )
    .unwrap();

    let request = service.begin_authorization().await.unwrap();
    let url = Url::parse(&request.authorization_url).unwrap();
    assert!(url
        .query_pairs()
        .any(|(name, value)| { name == "code_challenge_method" && value == "S256" }));
    let client = openidconnect::reqwest::ClientBuilder::new()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = client.get(url).send().await.unwrap();
    assert!(response.status().is_redirection());
    let location = response
        .headers()
        .get(openidconnect::reqwest::header::LOCATION)
        .expect("mock authorization redirect")
        .to_str()
        .unwrap();
    let callback = Url::parse(location).unwrap();
    let query = callback
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    let code = query.get("code").expect("authorization code").to_string();
    let state = query.get("state").expect("callback state").to_string();
    service
        .complete_authorization(AuthorizationCallback::new(code, state))
        .await
        .unwrap();
}
