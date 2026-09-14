// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Consumer of the versioned Hub identity contract; no Hub source dependency.
use crate::runtime_config::RuntimeConfig;
use serde::Deserialize;
use std::time::Duration;
use url::Url;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HubIdentity {
    pub subject_id: String,
    pub tenant_id: String,
}

#[derive(Deserialize)]
struct HubErrorBody {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

pub fn validate_base_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|_| "hub_configuration_invalid")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("hub_configuration_invalid".to_string());
    }
    Ok(())
}

pub async fn project_session(
    config: &RuntimeConfig,
    access_token: &str,
    expected_subject: &str,
) -> Result<HubIdentity, String> {
    validate_base_url(&config.hub_base_url)?;
    let endpoint = Url::parse(&config.hub_base_url)
        .and_then(|url| url.join("/v1/identity/session"))
        .map_err(|_| "hub_configuration_invalid")?;
    request_projection(endpoint, access_token, expected_subject).await
}

async fn request_projection(
    endpoint: Url,
    access_token: &str,
    expected_subject: &str,
) -> Result<HubIdentity, String> {
    if access_token.is_empty() || expected_subject.trim().is_empty() {
        return Err("hub_identity_required".to_string());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| "hub_client_unavailable")?;
    let mut response = client
        .post(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| "hub_unavailable")?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "hub_unavailable")? {
            if body.len() + chunk.len() > 8192 {
                return Err("hub_unavailable".to_string());
            }
            body.extend_from_slice(&chunk);
        }
        let error = serde_json::from_slice::<HubErrorBody>(&body).ok();
        return Err(match (
            status,
            error.as_ref().and_then(|body| body.error.as_deref()),
            error.as_ref().and_then(|body| body.message.as_deref()),
        ) {
            (403, _, Some("Verified caller lacks required authority")) => {
                "hub_identity_permission_denied"
            }
            (403, _, Some("Verified caller is not a registered product client")) => {
                "hub_identity_client_unregistered"
            }
            (401, Some("unauthorized"), _) => "hub_identity_unauthorized",
            (403, Some("forbidden"), _) => "hub_identity_forbidden",
            (401, _, _) => "hub_identity_unauthorized",
            (403, _, _) => "hub_identity_forbidden",
            _ => "hub_unavailable",
        }
        .to_string());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "hub_response_invalid")? {
        if body.len() + chunk.len() > 8192 {
            return Err("hub_response_invalid".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    let identity: HubIdentity =
        serde_json::from_slice(&body).map_err(|_| "hub_response_invalid")?;
    if identity.subject_id != expected_subject
        || identity.tenant_id.trim().is_empty()
        || identity.tenant_id.len() > 512
    {
        return Err("hub_identity_binding_invalid".to_string());
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn hub_configuration_is_https_origin_only() {
        assert!(validate_base_url("https://hub.tradeassembly.ai").is_ok());
        for value in [
            "http://localhost:8080",
            "https://user:secret@hub.example",
            "https://hub.example/path",
            "https://hub.example?token=x",
        ] {
            assert!(validate_base_url(value).is_err());
        }
    }

    async fn respond(status: &str, body: &str) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let count = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..count])
                .starts_with("POST /v1/identity/session "));
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        Url::parse(&format!("http://{addr}/v1/identity/session")).unwrap()
    }

    #[tokio::test]
    async fn accepts_only_matching_bounded_identity_projection() {
        let endpoint = respond(
            "200 OK",
            r#"{"subject_id":"user_test","tenant_id":"user:user_test"}"#,
        )
        .await;
        assert_eq!(
            request_projection(endpoint, "synthetic-test-token", "user_test")
                .await
                .unwrap()
                .tenant_id,
            "user:user_test"
        );
        let endpoint = respond("200 OK", r#"{"subject_id":"other","tenant_id":"tenant"}"#).await;
        assert_eq!(
            request_projection(endpoint, "synthetic-test-token", "user_test")
                .await
                .unwrap_err(),
            "hub_identity_binding_invalid"
        );
    }

    #[tokio::test]
    async fn denied_and_malformed_responses_are_redacted() {
        for (status, body, reason) in [
            (
                "403 Forbidden",
                "sensitive provider error",
                "hub_identity_forbidden",
            ),
            (
                "401 Unauthorized",
                r#"{"error":"unauthorized"}"#,
                "hub_identity_unauthorized",
            ),
            (
                "403 Forbidden",
                r#"{"error":"forbidden"}"#,
                "hub_identity_forbidden",
            ),
            (
                "403 Forbidden",
                r#"{"error":"forbidden","message":"Verified caller is not a registered product client"}"#,
                "hub_identity_client_unregistered",
            ),
            ("200 OK", "sensitive provider error", "hub_response_invalid"),
            ("302 Found", "redirect", "hub_unavailable"),
        ] {
            let endpoint = respond(status, body).await;
            assert_eq!(
                request_projection(endpoint, "synthetic-test-token", "user_test")
                    .await
                    .unwrap_err(),
                reason
            );
        }
    }
}
