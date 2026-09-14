// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    ClaimRunnerCommandRequest, CompleteRunnerCommandRequest, ConnectedRunnerError,
    InstallationSignerPort, PortDescriptor, PortKind, ProductRunnerTransportPort,
    ReachFinishRequest, ReachNodeRequest, ReachRecord, ReachTransitionRequest, ReachTransportPort,
    RunnerEntropyPort, RunnerHandoffReceipt, RunnerLease, VersionedPort,
    INSTALLATION_AUTHENTICATION_DOMAIN, MAX_RUNNER_RESPONSE_BYTES,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::{Signer as _, SigningKey};
use rand::{rngs::OsRng, RngCore};
use reqwest::{
    blocking::{Client, Response},
    redirect::Policy,
    StatusCode,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt, fs, io::Read, path::Path, time::Duration};
use url::Url;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct HttpProductRunnerTransport {
    base_url: Url,
    client: Client,
}

impl HttpProductRunnerTransport {
    pub fn new(base_url: &str) -> Result<Self, ConnectedRunnerError> {
        let mut base_url =
            Url::parse(base_url).map_err(|_| ConnectedRunnerError::invalid_contract())?;
        if base_url.username() != ""
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "" | "/")
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        let secure = base_url.scheme() == "https";
        let loopback_http = base_url.scheme() == "http" && is_loopback(&base_url);
        if !secure && !loopback_http {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        base_url.set_path("/");
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| ConnectedRunnerError::transport_unavailable())?;
        Ok(Self { base_url, client })
    }

    fn post<Request: Serialize, ResponseBody: DeserializeOwned>(
        &self,
        path_segments: &[&str],
        request: &Request,
    ) -> Result<ResponseBody, ConnectedRunnerError> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| ConnectedRunnerError::invalid_contract())?;
            segments.clear();
            for segment in path_segments {
                segments.push(segment);
            }
        }
        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .json(request)
            .send()
            .map_err(|_| ConnectedRunnerError::transport_unavailable())?;
        parse_response(response)
    }
}

impl fmt::Debug for HttpProductRunnerTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpProductRunnerTransport")
            .field("origin", &self.base_url.origin().ascii_serialization())
            .finish()
    }
}

impl VersionedPort for HttpProductRunnerTransport {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::CoreRunnerBridge, "product-runner-http")
                .for_profiles(&["local", "self_hosted"])
                .with_capabilities(&["connected_paper_runner_transport"]),
        ]
    }
}

impl ProductRunnerTransportPort for HttpProductRunnerTransport {
    fn claim(
        &self,
        request: &ClaimRunnerCommandRequest,
    ) -> Result<Option<RunnerLease>, ConnectedRunnerError> {
        self.post(
            &[
                "v1",
                "installations",
                &request.installation_id,
                "runner-commands",
                "claim",
            ],
            request,
        )
    }

    fn complete(
        &self,
        request: &CompleteRunnerCommandRequest,
    ) -> Result<RunnerHandoffReceipt, ConnectedRunnerError> {
        self.post(
            &[
                "v1",
                "installations",
                &request.installation_id,
                "runner-commands",
                &request.command_id,
                "complete",
            ],
            request,
        )
    }
}

pub struct HttpReachTransport {
    base_url: Url,
    client: Client,
}

impl HttpReachTransport {
    pub fn new(base_url: &str) -> Result<Self, ConnectedRunnerError> {
        let mut base_url =
            Url::parse(base_url).map_err(|_| ConnectedRunnerError::invalid_contract())?;
        if base_url.username() != ""
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "" | "/")
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        let secure = base_url.scheme() == "https";
        let loopback_http = base_url.scheme() == "http" && is_loopback(&base_url);
        if !secure && !loopback_http {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        base_url.set_path("/");
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| ConnectedRunnerError::transport_unavailable())?;
        Ok(Self { base_url, client })
    }

    fn post<Request: Serialize, ResponseBody: DeserializeOwned>(
        &self,
        path_segments: &[&str],
        request: &Request,
    ) -> Result<ResponseBody, ConnectedRunnerError> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| ConnectedRunnerError::invalid_contract())?;
            segments.clear();
            for segment in path_segments {
                segments.push(segment);
            }
        }
        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .json(request)
            .send()
            .map_err(|_| ConnectedRunnerError::transport_unavailable())?;
        parse_response(response)
    }
}

impl fmt::Debug for HttpReachTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpReachTransport")
            .field("origin", &self.base_url.origin().ascii_serialization())
            .finish()
    }
}

impl VersionedPort for HttpReachTransport {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::CoreRunnerBridge, "relay-reach-http")
                .for_profiles(&["local", "self_hosted"])
                .with_capabilities(&["outbound_reach_transport"]),
        ]
    }
}

impl ReachTransportPort for HttpReachTransport {
    fn claim(
        &self,
        request: &ReachNodeRequest,
    ) -> Result<Option<ReachRecord>, ConnectedRunnerError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ClaimResponse {
            command: Option<ReachRecord>,
        }
        Ok(self
            .post::<_, ClaimResponse>(&["v1", "reach", "node", "claim"], request)?
            .command)
    }

    fn accept(
        &self,
        request: &ReachTransitionRequest,
    ) -> Result<ReachRecord, ConnectedRunnerError> {
        self.post(&["v1", "reach", "node", "accept"], request)
    }

    fn finish(&self, request: &ReachFinishRequest) -> Result<ReachRecord, ConnectedRunnerError> {
        self.post(&["v1", "reach", "node", "finish"], request)
    }
}

pub struct Ed25519InstallationSigner {
    installation_id: String,
    signing_key: SigningKey,
}

impl Ed25519InstallationSigner {
    pub fn from_seed(
        installation_id: impl Into<String>,
        seed: [u8; 32],
    ) -> Result<Self, ConnectedRunnerError> {
        let installation_id = installation_id.into();
        if installation_id.is_empty()
            || installation_id.len() > 256
            || !installation_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        Ok(Self {
            installation_id,
            signing_key: SigningKey::from_bytes(&seed),
        })
    }
}

impl fmt::Debug for Ed25519InstallationSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ed25519InstallationSigner")
            .field("installation_id", &self.installation_id)
            .field("signing_key", &"[REDACTED]")
            .finish()
    }
}

impl VersionedPort for Ed25519InstallationSigner {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::Identity, "local-ed25519-installation-signer")
                .for_profiles(&["local", "self_hosted"])
                .with_capabilities(&["installation_request_signing"]),
        ]
    }
}

impl InstallationSignerPort for Ed25519InstallationSigner {
    fn installation_id(&self) -> &str {
        &self.installation_id
    }

    fn sign(&self, payload: &str) -> Result<String, ConnectedRunnerError> {
        let mut message =
            Vec::with_capacity(INSTALLATION_AUTHENTICATION_DOMAIN.len() + payload.len());
        message.extend_from_slice(INSTALLATION_AUTHENTICATION_DOMAIN.as_bytes());
        message.extend_from_slice(payload.as_bytes());
        Ok(URL_SAFE_NO_PAD.encode(self.signing_key.sign(&message).to_bytes()))
    }
}

#[derive(Debug, Default)]
pub struct OsRunnerEntropy;

impl VersionedPort for OsRunnerEntropy {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::CoreRunnerBridge, "os-runner-entropy")
                .for_profiles(&["local", "self_hosted"])
                .with_capabilities(&["connected_runner_request_ids"]),
        ]
    }
}

impl RunnerEntropyPort for OsRunnerEntropy {
    fn opaque_id(&self, prefix: &str) -> Result<String, ConnectedRunnerError> {
        if prefix.is_empty()
            || prefix.len() > 64
            || !prefix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        let mut bytes = [0_u8; 24];
        OsRng.fill_bytes(&mut bytes);
        Ok(format!("{prefix}-{}", URL_SAFE_NO_PAD.encode(bytes)))
    }
}

pub fn read_ed25519_seed_file(path: &Path) -> Result<[u8; 32], ConnectedRunnerError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ConnectedRunnerError::signing_failure())?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(ConnectedRunnerError::signing_failure());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ConnectedRunnerError::signing_failure());
        }
    }
    if metadata.len() == 0 || metadata.len() > 128 {
        return Err(ConnectedRunnerError::signing_failure());
    }
    let encoded = fs::read_to_string(path).map_err(|_| ConnectedRunnerError::signing_failure())?;
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded.trim())
        .map_err(|_| ConnectedRunnerError::signing_failure())?;
    decoded
        .try_into()
        .map_err(|_| ConnectedRunnerError::signing_failure())
}

fn parse_response<T: DeserializeOwned>(mut response: Response) -> Result<T, ConnectedRunnerError> {
    if response.status() == StatusCode::CONFLICT {
        return Err(ConnectedRunnerError::stale_lease());
    }
    if response.status() != StatusCode::OK {
        return Err(ConnectedRunnerError::transport_unavailable());
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take((MAX_RUNNER_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ConnectedRunnerError::transport_unavailable())?;
    if bytes.len() > MAX_RUNNER_RESPONSE_BYTES {
        return Err(ConnectedRunnerError::invalid_contract());
    }
    serde_json::from_slice(&bytes).map_err(|_| ConnectedRunnerError::invalid_contract())
}

fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}
