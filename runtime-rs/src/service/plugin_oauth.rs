//! Request-scoped OAuth handoff for plugin connections.
//!
//! This module deliberately contains no provider client credentials.  The
//! local runtime authenticates the current actor, sends only its short-lived
//! WorkOS access token to an operator-allowlisted relay, and stores the
//! resulting provider token through the normal credential port.

use super::{plugin_lifecycle, ServiceResponse, TradeAssemblyService};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
use std::time::Duration;
use url::Url;

const MAX_BODY_BYTES: usize = 256 * 1024;

/// Configuration supplied by the service owner.  Empty allowlists fail
/// closed.  The manifest environment is resolved before calling this module.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RelayConfig {
    pub relay_base_url: String,
    pub relay_origins: BTreeSet<String>,
    pub authorization_origins: BTreeSet<String>,
    pub timeout_ms: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Handoff {
    pub(crate) instance_ref: String,
    pub(crate) environment: String,
    pub(crate) mode: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) verifier: String,
    pub(crate) challenge: String,
}

pub(crate) fn start(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    run_blocking(|| start_blocking(service, body))
        .unwrap_or_else(|_| ServiceResponse::internal_error("plugin_oauth_worker_failed"))
}

fn start_blocking(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let _lock = match oauth_lock(service) {
        Ok(lock) => lock,
        Err(code) => return ServiceResponse::conflict(code),
    };
    let (instance_ref, environment, scopes) = match validate_request(&body) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    let record = match plugin_lifecycle::require_instance(service, &instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    let Some(mode) = record["accountMode"]
        .as_str()
        .filter(|mode| matches!(*mode, "paper" | "live"))
    else {
        return ServiceResponse::conflict("plugin_oauth_account_mode_invalid");
    };
    let Some((issuer, subject)) = service.responsible_human_identity() else {
        return ServiceResponse::unauthorized("plugin_oauth_actor_required");
    };
    let actor = invocation_actor(issuer, subject);
    let manifest =
        plugin_lifecycle::get_manifest(service, record["pluginRef"].as_str().unwrap_or_default());
    let descriptor = &manifest.body["manifest"]["configuration"]["oauth"];
    if !scopes_allowed(descriptor, &scopes) {
        return ServiceResponse::bad_request("plugin_oauth_scope_invalid");
    }
    let credential_field = match bounded(descriptor, "credentialField", 128) {
        Ok(field) => field,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    let relay = match manifest_relay(service, &record, &environment) {
        Ok(relay) => relay,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    if let Err(code) = relay_endpoint(&relay, "/v1/connections/start") {
        return ServiceResponse::bad_request(code);
    }
    let Some(config) = service.oauth_config.clone() else {
        return ServiceResponse::bad_request("plugin_oauth_not_configured");
    };
    let hosted_actor = service.hosted_oauth_actor.as_deref().unwrap_or(&actor);
    let token = match actor_token(&config, hosted_actor) {
        Ok(token) => token,
        Err(code) => return ServiceResponse::unauthorized(code),
    };
    // One pending connection per owned instance. A stale browser tab cannot
    // replace credentials after a newer connection attempt has started.
    let older = match service.runtime().storage.list_json("plugin_oauth_attempts") {
        Ok(attempts) => attempts,
        Err(_) => return ServiceResponse::internal_error("oauth attempt lookup failed"),
    };
    let request_key = if body.get("idempotency_key").is_some() {
        match bounded(&body, "idempotency_key", 256) {
            Ok(key) => Some(key),
            Err(code) => return ServiceResponse::bad_request(code),
        }
    } else {
        None
    };
    let request_digest = digest(&json!({"environment":environment,"scopes":scopes,
        "hostedActor":hosted_actor,"mode":mode,"configuration":record["configuration"],
        "manifestDigest":manifest.body["manifestDigest"],"relayBaseUrl":relay.relay_base_url}));
    if let Some(key) = request_key.as_ref() {
        if let Some((_, attempt)) = older.iter().find(|(_, attempt)| {
            attempt["actor"] == actor
                && attempt["instanceRef"] == instance_ref
                && attempt["requestKey"] == *key
        }) {
            return match replay_start(attempt, &request_digest, service.runtime().clock.now_ms()) {
                Ok(receipt) => ServiceResponse::ok(receipt),
                Err(code) => ServiceResponse::conflict(code),
            };
        }
    }
    for (key, mut attempt) in older
        .into_iter()
        .filter(|(_, value)| value["instanceRef"] == instance_ref && value["actor"] == actor)
    {
        attempt["invalidated"] = json!(true);
        if persist_attempt(service, &key, attempt.clone()).is_err() {
            return ServiceResponse::internal_error("oauth invalidation failed");
        }
        if let Some(reference) = attempt["handoffRef"].as_str() {
            let _ = service.runtime().credentials.revoke(reference);
        }
    }
    let handoff = new_handoff(instance_ref.clone(), environment, mode.to_string(), scopes);
    let handoff_ref = handoff_ref(&handoff);
    let fields =
        std::collections::BTreeMap::from([("verifier".to_string(), handoff.verifier.clone())]);
    if service
        .runtime()
        .credentials
        .store(&handoff_ref, &fields)
        .is_err()
    {
        return ServiceResponse::internal_error("oauth handoff storage failed");
    }
    let attempt_key = attempt_key(&actor, &instance_ref, &handoff_ref);
    let manifest_digest =
        plugin_lifecycle::get_manifest(service, record["pluginRef"].as_str().unwrap_or_default())
            .body["manifestDigest"]
            .clone();
    let attempt = json!({"instanceRef":instance_ref,"environment":handoff.environment,"mode":handoff.mode,"scopes":handoff.scopes,"handoffRef":handoff_ref,"actor":actor,"hostedActor":hosted_actor,"manifestDigest":manifest_digest,"configurationDigest":digest(&record["configuration"]),"credentialField":credential_field,"relayBaseUrl":relay.relay_base_url,"createdAtMs":service.runtime().clock.now_ms(),"requestKey":request_key,"requestDigest":request_digest});
    if persist_attempt(service, &attempt_key, attempt.clone()).is_err() {
        let _ = service.runtime().credentials.revoke(&handoff_ref);
        return ServiceResponse::internal_error("oauth handoff state failed");
    }
    match start_with_token(&relay, &token, handoff) {
        Ok(value) => {
            if let Some(connection_id) = value.get("connectionId").and_then(Value::as_str) {
                let mut attempt = attempt;
                attempt["connectionId"] = json!(connection_id);
                attempt["expiresAtMs"] = value["expiresAtMs"].clone();
                attempt["startReceipt"] = value.clone();
                if persist_attempt(service, &attempt_key, attempt).is_err() {
                    let _ = service.runtime().credentials.revoke(&handoff_ref);
                    return ServiceResponse::internal_error("oauth handoff state failed");
                }
            }
            ServiceResponse::ok(value)
        }
        Err(code) => {
            let _ = service.runtime().credentials.revoke(&handoff_ref);
            ServiceResponse::bad_gateway(code)
        }
    }
}

fn replay_start(
    attempt: &Value,
    expected_digest: &str,
    now_ms: i64,
) -> Result<Value, &'static str> {
    if attempt["requestDigest"] != expected_digest {
        return Err("plugin_oauth_idempotency_conflict");
    }
    if attempt["invalidated"] == true {
        return Err("plugin_oauth_attempt_ended");
    }
    if !attempt["startReceipt"].is_object() {
        // A process/network failure may have happened after the remote write.
        // Never manufacture a second authorization for this request key.
        return Err("plugin_oauth_start_outcome_unknown");
    }
    if attempt["expiresAtMs"].as_i64().unwrap_or(0) <= now_ms {
        return Err("plugin_oauth_attempt_ended");
    }
    Ok(attempt["startReceipt"].clone())
}

pub(crate) fn status(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    run_blocking(|| status_blocking(service, body))
        .unwrap_or_else(|_| ServiceResponse::internal_error("plugin_oauth_worker_failed"))
}

fn status_blocking(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let _lock = match oauth_lock(service) {
        Ok(lock) => lock,
        Err(code) => return ServiceResponse::conflict(code),
    };
    let instance_ref = match bounded(&body, "instanceRef", 128) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    let connection_id = match bounded(&body, "connectionId", 256) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    let record = match plugin_lifecycle::require_instance(service, &instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    let Some((issuer, subject)) = service.responsible_human_identity() else {
        return ServiceResponse::unauthorized("plugin_oauth_actor_required");
    };
    let actor = invocation_actor(issuer, subject);
    let attempts = service
        .runtime()
        .storage
        .list_json("plugin_oauth_attempts")
        .unwrap_or_default();
    let Some((attempt_key, mut attempt)) = attempts.into_iter().find(|(_, value)| {
        value["actor"] == actor
            && value["instanceRef"] == instance_ref
            && value["connectionId"] == connection_id
            && value["invalidated"] != true
    }) else {
        return ServiceResponse::bad_request("plugin_oauth_attempt_not_found");
    };
    if attempt["configurationDigest"] != digest(&record["configuration"]) {
        return ServiceResponse::conflict("plugin_oauth_configuration_changed");
    }
    let hosted_actor = service.hosted_oauth_actor.as_deref().unwrap_or(&actor);
    if attempt["hostedActor"].as_str().unwrap_or(&actor) != hosted_actor {
        return ServiceResponse::conflict("plugin_oauth_identity_changed");
    }
    let current_manifest =
        plugin_lifecycle::get_manifest(service, record["pluginRef"].as_str().unwrap_or_default())
            .body["manifestDigest"]
            .clone();
    if attempt["manifestDigest"] != current_manifest {
        return ServiceResponse::conflict("plugin_oauth_manifest_changed");
    }
    if attempt["mode"] != record["accountMode"] || record["removedAtMs"].is_number() {
        return ServiceResponse::conflict("plugin_oauth_configuration_changed");
    }
    if attempt["receipt"].is_object()
        && attempt["storedCredentialRevision"] != record["credentialRevision"]
    {
        return ServiceResponse::conflict("plugin_oauth_credentials_changed");
    }
    if attempt["acknowledged"] == true {
        return ServiceResponse::ok(attempt["receipt"].clone());
    }
    let handoff_ref = attempt["handoffRef"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if attempt["expiresAtMs"].as_i64().unwrap_or(0) <= service.runtime().clock.now_ms() {
        let _ = service.runtime().credentials.revoke(&handoff_ref);
        return ServiceResponse::conflict("plugin_oauth_handoff_expired");
    }
    let Some(fields) = service
        .runtime()
        .credentials
        .resolve_fields(&handoff_ref)
        .ok()
        .flatten()
    else {
        return ServiceResponse::conflict("plugin_oauth_handoff_expired");
    };
    let verifier = fields.get("verifier").cloned().unwrap_or_default();
    let handoff = Handoff {
        instance_ref: instance_ref.clone(),
        environment: attempt["environment"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        mode: attempt["mode"].as_str().unwrap_or_default().to_string(),
        scopes: attempt["scopes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
        verifier,
    };
    let Some(config) = service.oauth_config.clone() else {
        return ServiceResponse::bad_request("plugin_oauth_not_configured");
    };
    let token = match actor_token(&config, hosted_actor) {
        Ok(token) => token,
        Err(code) => return ServiceResponse::unauthorized(code),
    };
    let relay = match manifest_relay(service, &record, &handoff.environment) {
        Ok(relay) => relay,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    if attempt["relayBaseUrl"] != relay.relay_base_url {
        return ServiceResponse::conflict("plugin_oauth_manifest_changed");
    }
    // A lost acknowledgement response must not cause another credential write.
    if attempt["receipt"].is_object() {
        return finish_ack(
            service,
            &attempt_key,
            attempt,
            &relay,
            &token,
            &connection_id,
            &handoff,
        );
    }
    let result = match status_with_token(&relay, &token, &connection_id, &handoff) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_gateway(code),
    };
    if result["status"] == "ready" {
        let access_token = result["accessToken"].as_str().unwrap_or_default();
        let stored = plugin_lifecycle::store_credentials(
            service,
            &instance_ref,
            json!({"credentials":{attempt["credentialField"].as_str().unwrap_or_default():access_token}}),
        );
        if stored.status != 200 {
            return ServiceResponse::conflict("plugin_oauth_credential_store_failed");
        }
        attempt["storedCredentialRevision"] =
            plugin_lifecycle::require_instance(service, &instance_ref)
                .map(|record| record["credentialRevision"].clone())
                .unwrap_or(Value::Null);
        attempt["receipt"] = json!({"status":"ready","instanceRef":instance_ref,"credentialStatus":stored.body["credentialStatus"],"accountId":result["accountId"],"mode":handoff.mode,"scopes":handoff.scopes});
        if persist_attempt(service, &attempt_key, attempt.clone()).is_err() {
            return ServiceResponse::internal_error("oauth receipt storage failed");
        }
        return finish_ack(
            service,
            &attempt_key,
            attempt,
            &relay,
            &token,
            &connection_id,
            &handoff,
        );
    }
    ServiceResponse::ok(result)
}

fn finish_ack(
    service: &TradeAssemblyService,
    key: &str,
    mut attempt: Value,
    relay: &RelayConfig,
    token: &str,
    connection_id: &str,
    handoff: &Handoff,
) -> ServiceResponse {
    if ack_with_token(relay, token, connection_id, handoff).is_err() {
        let mut receipt = attempt["receipt"].clone();
        receipt["ackPending"] = json!(true);
        return ServiceResponse::ok(receipt);
    }
    attempt["acknowledged"] = json!(true);
    if persist_attempt(service, key, attempt.clone()).is_err() {
        return ServiceResponse::internal_error("oauth receipt storage failed");
    }
    let _ = service.runtime().credentials.revoke(&handoff_ref(handoff));
    ServiceResponse::ok(attempt["receipt"].clone())
}

pub(crate) fn disconnect(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let _lock = match oauth_lock(service) {
        Ok(lock) => lock,
        Err(code) => return ServiceResponse::conflict(code),
    };
    let instance_ref = match bounded(&body, "instanceRef", 128) {
        Ok(value) => value,
        Err(code) => return ServiceResponse::bad_request(code),
    };
    if let Err(response) = plugin_lifecycle::require_instance(service, &instance_ref) {
        return response;
    }
    if service.responsible_human_identity().is_none() {
        return ServiceResponse::unauthorized("plugin_oauth_actor_required");
    }
    match service.runtime().storage.list_json("plugin_oauth_attempts") {
        Ok(attempts) => {
            for (key, mut attempt) in attempts
                .into_iter()
                .filter(|(_, value)| value["instanceRef"] == instance_ref)
            {
                attempt["invalidated"] = json!(true);
                if persist_attempt(service, &key, attempt.clone()).is_err() {
                    return ServiceResponse::internal_error("oauth invalidation failed");
                }
                if let Some(reference) = attempt["handoffRef"].as_str() {
                    let _ = service.runtime().credentials.revoke(reference);
                }
            }
        }
        Err(_) => return ServiceResponse::internal_error("oauth invalidation failed"),
    }
    let response = plugin_lifecycle::revoke_credentials(service, &instance_ref);
    if response.status != 200 {
        return response;
    }
    ServiceResponse::ok(
        json!({"ok":true,"instanceRef":instance_ref,"remoteRevocation":"not_requested"}),
    )
}

/// Cancel only this handoff. Existing provider credentials are preserved.
pub(crate) fn cancel_attempt(
    service: &TradeAssemblyService,
    instance: &str,
    connection: &str,
) -> Result<(), String> {
    let _lock = oauth_lock(service).map_err(str::to_owned)?;
    plugin_lifecycle::require_instance(service, instance)
        .map_err(|_| "plugin_oauth_instance_unavailable")?;
    let (issuer, subject) = service
        .responsible_human_identity()
        .ok_or("plugin_oauth_actor_required")?;
    let actor = invocation_actor(issuer, subject);
    for (key, mut attempt) in service
        .runtime()
        .storage
        .list_json("plugin_oauth_attempts")?
    {
        if attempt["actor"] == actor
            && attempt["instanceRef"] == instance
            && attempt["connectionId"] == connection
        {
            attempt["invalidated"] = json!(true);
            persist_attempt(service, &key, attempt.clone())?;
            if let Some(reference) = attempt["handoffRef"].as_str() {
                service.runtime().credentials.revoke(reference)?;
            }
        }
    }
    Ok(())
}

/// Start a relay handoff after the service has performed actor and instance
/// authorization. This is kept separate so the service layer never has to
/// pass an identity token through a JSON body or journal record.
pub(crate) fn start_with_token(
    config: &RelayConfig,
    token: &str,
    handoff: Handoff,
) -> Result<Value, &'static str> {
    let relay = relay_endpoint(config, "/v1/connections/start")?;
    let authorization = post_json(
        config,
        &relay,
        token,
        json!({
            "instanceRef": handoff.instance_ref,
            "environment": handoff.environment,
            "mode": handoff.mode,
            "scopes": handoff.scopes,
            "handoffChallenge": handoff.challenge,
        }),
    )?;
    let url = authorization
        .get("authorizationUrl")
        .and_then(Value::as_str)
        .ok_or("plugin_oauth_relay_response_invalid")?;
    if authorization
        .get("connectionId")
        .and_then(Value::as_str)
        .is_none()
        || authorization
            .get("expiresAtMs")
            .and_then(Value::as_i64)
            .is_none()
    {
        return Err("plugin_oauth_relay_response_invalid");
    }
    validate_authorization_url(config, url)?;
    Ok(json!({
        "connectionId": authorization.get("connectionId"),
        "authorizationUrl": url,
        "expiresAtMs": authorization.get("expiresAtMs"),
        "environment": handoff.environment,
        "mode": handoff.mode,
        "scopes": handoff.scopes,
        "handoffRef": handoff_ref(&handoff),
    }))
}

pub(crate) fn status_with_token(
    config: &RelayConfig,
    token: &str,
    connection_id: &str,
    handoff: &Handoff,
) -> Result<Value, &'static str> {
    let relay = relay_endpoint(config, "/v1/connections/status")?;
    let result = post_json(
        config,
        &relay,
        token,
        json!({
            "connectionId": connection_id,
            "instanceRef": handoff.instance_ref,
            "verifier": handoff.verifier,
        }),
    )?;
    validate_status(&result, &handoff.mode, &handoff.scopes)
}

fn ack_with_token(
    config: &RelayConfig,
    token: &str,
    connection_id: &str,
    handoff: &Handoff,
) -> Result<(), &'static str> {
    let relay = relay_endpoint(config, "/v1/connections/ack")?;
    let response = post_json(
        config,
        &relay,
        token,
        json!({"connectionId":connection_id,"instanceRef":handoff.instance_ref,"verifier":handoff.verifier}),
    )?;
    if response["acknowledged"] != true {
        return Err("plugin_oauth_ack_invalid");
    }
    Ok(())
}

pub(crate) fn new_handoff(
    instance_ref: String,
    environment: String,
    mode: String,
    scopes: Vec<String>,
) -> Handoff {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Handoff {
        instance_ref,
        environment,
        mode,
        scopes,
        verifier,
        challenge,
    }
}

pub(crate) fn validate_request(
    body: &Value,
) -> Result<(String, String, Vec<String>), &'static str> {
    let instance_ref = bounded(body, "instanceRef", 128)?;
    let environment = bounded(body, "environment", 32)?;
    let mut scopes = body
        .get("scopes")
        .and_then(Value::as_array)
        .ok_or("plugin_oauth_scopes_required")?
        .iter()
        .map(|scope| {
            scope
                .as_str()
                .filter(|scope| !scope.is_empty() && scope.len() <= 128)
                .map(str::to_string)
                .ok_or("plugin_oauth_scope_invalid")
        })
        .collect::<Result<Vec<_>, _>>()?;
    scopes.sort();
    if scopes.len() > 64 || scopes.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("plugin_oauth_scopes_invalid");
    }
    Ok((instance_ref, environment, scopes))
}

fn bounded(body: &Value, key: &str, max: usize) -> Result<String, &'static str> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= max)
        .map(str::to_string)
        .ok_or("plugin_oauth_request_invalid")
}

fn relay_endpoint(config: &RelayConfig, path: &str) -> Result<Url, &'static str> {
    let base = Url::parse(&config.relay_base_url).map_err(|_| "plugin_oauth_relay_invalid")?;
    if base.scheme() != "https"
        || !origin_allowed(&base, &config.relay_origins)
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || base.path() != "/"
    {
        return Err("plugin_oauth_relay_not_allowed");
    }
    base.join(path).map_err(|_| "plugin_oauth_relay_invalid")
}

fn validate_authorization_url(config: &RelayConfig, value: &str) -> Result<(), &'static str> {
    let url = Url::parse(value).map_err(|_| "plugin_oauth_authorization_url_invalid")?;
    if url.scheme() != "https"
        || !origin_allowed(&url, &config.authorization_origins)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url
            .query_pairs()
            .any(|(key, _)| matches!(key.as_ref(), "code" | "access_token" | "token"))
    {
        return Err("plugin_oauth_authorization_url_not_allowed");
    }
    Ok(())
}

fn origin_allowed(url: &Url, allowlist: &BTreeSet<String>) -> bool {
    allowlist.contains(&url.origin().ascii_serialization())
}

fn post_json(
    config: &RelayConfig,
    url: &Url,
    token: &str,
    body: Value,
) -> Result<Value, &'static str> {
    if token.is_empty() || token.len() > 16_384 {
        return Err("plugin_oauth_actor_token_invalid");
    }
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(config.timeout_ms.clamp(100, 15_000)))
        .build()
        .map_err(|_| "plugin_oauth_transport_unavailable")?;
    let response = client
        .post(url.clone())
        .bearer_auth(token)
        .json(&body)
        .send()
        .map_err(|_| "plugin_oauth_relay_unavailable")?;
    if !response.status().is_success() {
        return Err("plugin_oauth_relay_rejected");
    }
    let mut bytes = Vec::new();
    response
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "plugin_oauth_relay_response_invalid")?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err("plugin_oauth_relay_response_too_large");
    }
    serde_json::from_slice(&bytes).map_err(|_| "plugin_oauth_relay_response_invalid")
}

fn validate_status(
    value: &Value,
    expected_mode: &str,
    expected_scopes: &[String],
) -> Result<Value, &'static str> {
    match value.get("status").and_then(Value::as_str) {
        Some("pending") => Ok(json!({"status": "pending"})),
        Some("failed") => Ok(json!({"status": "failed"})),
        Some("ready") => {
            let mode = value
                .get("mode")
                .and_then(Value::as_str)
                .ok_or("plugin_oauth_relay_response_invalid")?;
            let scopes = value
                .get("scopes")
                .and_then(Value::as_array)
                .ok_or("plugin_oauth_relay_response_invalid")?;
            let scopes = scopes
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            if mode != expected_mode
                || scopes != expected_scopes
                || value
                    .get("accessToken")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                || value
                    .get("accountId")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            {
                return Err("plugin_oauth_binding_mismatch");
            }
            Ok(
                json!({"status":"ready","accessToken":value["accessToken"],"accountId":value["accountId"],"mode":mode,"scopes":scopes}),
            )
        }
        _ => Err("plugin_oauth_relay_response_invalid"),
    }
}

fn handoff_ref(handoff: &Handoff) -> String {
    let digest = Sha256::digest(handoff.verifier.as_bytes());
    format!("oauth-handoff-{}", URL_SAFE_NO_PAD.encode(digest))
}

fn relay_config() -> RelayConfig {
    let relay_origins = origins("TRADEASSEMBLY_OAUTH_RELAY_ORIGINS");
    let authorization_origins = origins("TRADEASSEMBLY_OAUTH_AUTHORIZATION_ORIGINS");
    RelayConfig {
        relay_base_url: String::new(),
        relay_origins,
        authorization_origins,
        timeout_ms: 10_000,
    }
}

fn manifest_relay(
    service: &TradeAssemblyService,
    record: &Value,
    environment: &str,
) -> Result<RelayConfig, &'static str> {
    let plugin_ref = record["pluginRef"]
        .as_str()
        .ok_or("plugin_oauth_manifest_invalid")?;
    let response = plugin_lifecycle::get_manifest(service, plugin_ref);
    if response.status != 200 {
        return Err("plugin_oauth_manifest_invalid");
    }
    let manifest = &response.body["manifest"];
    let environments = manifest["configuration"]["oauth"]["environments"]
        .as_array()
        .ok_or("plugin_oauth_environment_invalid")?;
    let entry = environments
        .iter()
        .find(|entry| entry["id"].as_str() == Some(environment))
        .ok_or("plugin_oauth_environment_invalid")?;
    let relay_base_url = entry["relayBaseUrl"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("plugin_oauth_relay_invalid")?
        .to_string();
    let config = match service.connection_profile.as_ref() {
        Some(profile) if profile.environment == environment => RelayConfig {
            timeout_ms: 10_000,
            relay_origins: profile.relay_origins.iter().cloned().collect(),
            authorization_origins: profile.authorization_origins.iter().cloned().collect(),
            ..RelayConfig::default()
        },
        Some(_) => return Err("plugin_oauth_environment_invalid"),
        None => relay_config(),
    };
    Ok(RelayConfig {
        relay_base_url,
        ..config
    })
}

fn origins(name: &str) -> BTreeSet<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn actor_token(
    config: &crate::runtime_config::RuntimeConfig,
    actor: &str,
) -> Result<String, &'static str> {
    let runtime =
        tokio::runtime::Runtime::new().map_err(|_| "plugin_oauth_identity_unavailable")?;
    runtime
        .block_on(
            crate::workos_identity::WorkosAuthManager::new(config.clone())
                .access_token_for_actor(actor),
        )
        .map_err(|_| "plugin_oauth_identity_required")
}

/// Blocking OAuth work must not attempt to create a Tokio runtime on an MCP
/// runtime thread. The scoped thread keeps the borrowed service valid while
/// moving both identity and reqwest blocking calls off the async executor.
fn run_blocking<T, F>(operation: F) -> Result<T, ()>
where
    T: Send,
    F: FnOnce() -> T + Send,
{
    std::thread::scope(|scope| scope.spawn(operation).join().map_err(|_| ()))
}

fn attempt_key(actor: &str, instance_ref: &str, handoff_ref: &str) -> String {
    format!("{actor}:{instance_ref}:{handoff_ref}")
}

// All authenticated service paths bind the canonical stable actor into
// SessionPrincipal.subject. Keep that value unchanged for WorkOS lookup and
// durable ownership keys.
fn invocation_actor(_issuer: &str, subject: &str) -> String {
    subject.to_string()
}

fn digest(value: &Value) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.to_string().as_bytes()))
}

fn persist_attempt(service: &TradeAssemblyService, key: &str, value: Value) -> Result<(), String> {
    let actor = service
        .responsible_human_identity()
        .map(|(issuer, subject)| invocation_actor(issuer, subject))
        .unwrap_or_else(|| "unknown".to_string());
    let context = crate::ports::SideEffectContext::new(
        crate::ports::AuthorityContext {
            actor,
            surface: "plugin_oauth".to_string(),
            account_mode: value["mode"].as_str().unwrap_or("paper").to_string(),
        },
        crate::ports::IdempotencyKey::new(format!("plugin-oauth:{key}"))?,
    );
    service
        .runtime()
        .storage
        .put_json("plugin_oauth_attempts", key, value, &context)
}

fn oauth_lock(service: &TradeAssemblyService) -> Result<std::fs::File, &'static str> {
    let path = std::path::PathBuf::from(format!("{}.oauth.lock", service.db()));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|_| "plugin_oauth_lock_busy")?;
    file.try_lock().map_err(|_| "plugin_oauth_lock_busy")?;
    Ok(file)
}

fn scopes_allowed(descriptor: &Value, requested: &[String]) -> bool {
    let Some(declared) = descriptor["scopes"].as_array() else {
        return false;
    };
    requested
        .iter()
        .all(|scope| declared.iter().any(|entry| entry["id"] == *scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_start_replay_rejects_changed_ended_and_ambiguous_requests() {
        let mut attempt = json!({"requestDigest":"bound-request","expiresAtMs":200,
            "startReceipt":{"connectionId":"connection","authorizationUrl":"https://broker.example/authorize"}});
        assert_eq!(
            replay_start(&attempt, "bound-request", 100).unwrap(),
            attempt["startReceipt"]
        );
        assert_eq!(
            replay_start(&attempt, "changed-account-or-scopes", 100),
            Err("plugin_oauth_idempotency_conflict")
        );
        assert_eq!(
            replay_start(&attempt, "bound-request", 200),
            Err("plugin_oauth_attempt_ended")
        );
        attempt["startReceipt"] = Value::Null;
        assert_eq!(
            replay_start(&attempt, "bound-request", 100),
            Err("plugin_oauth_start_outcome_unknown")
        );
        attempt["invalidated"] = json!(true);
        assert_eq!(
            replay_start(&attempt, "bound-request", 100),
            Err("plugin_oauth_attempt_ended")
        );
    }

    #[test]
    fn validates_scopes_mode_and_rejects_duplicates() {
        let valid =
            json!({"instanceRef":"alpaca","environment":"production","scopes":["data","trading"]});
        assert!(validate_request(&valid).is_ok());
        let duplicate =
            json!({"instanceRef":"alpaca","environment":"production","scopes":["data","data"]});
        assert_eq!(
            validate_request(&duplicate),
            Err("plugin_oauth_scopes_invalid")
        );
    }

    #[test]
    fn relay_and_authorization_urls_fail_closed() {
        let config = RelayConfig::default();
        assert_eq!(
            relay_endpoint(&config, "/v1/connections/start"),
            Err("plugin_oauth_relay_invalid")
        );
        assert_eq!(
            validate_authorization_url(&config, "http://evil.test/callback"),
            Err("plugin_oauth_authorization_url_not_allowed")
        );
    }

    #[test]
    fn lock_releases_without_deleting_the_shared_inode() {
        let dir = tempfile::tempdir().unwrap();
        let service =
            TradeAssemblyService::test_local(dir.path().join("test.db").to_string_lossy());
        let lock = oauth_lock(&service).unwrap();
        assert!(oauth_lock(&service).is_err());
        drop(lock);
        assert!(oauth_lock(&service).is_ok());
        assert!(dir.path().join("test.db.oauth.lock").exists());
    }

    #[test]
    fn read_only_scopes_are_allowed_but_undeclared_scopes_are_not() {
        let descriptor = json!({"scopes":[{"id":"read"}]});
        assert!(validate_request(
            &json!({"instanceRef":"x","environment":"production","scopes":[]})
        )
        .is_ok());
        assert!(scopes_allowed(&descriptor, &[]));
        assert!(scopes_allowed(&descriptor, &["read".into()]));
        assert!(!scopes_allowed(&descriptor, &["admin".into()]));
    }

    #[test]
    fn ready_is_strictly_bound_and_redacted_to_the_expected_shape() {
        let mut ready = json!({"status":"ready","mode":"live","scopes":[],"accessToken":"fixture","accountId":"account","unexpected":"not returned"});
        let valid = validate_status(&ready, "live", &[]).unwrap();
        assert!(valid.get("unexpected").is_none());
        assert!(validate_status(&ready, "paper", &[]).is_err());
        ready["accountId"] = json!("");
        assert!(validate_status(&ready, "live", &[]).is_err());
        ready["accountId"] = json!("account");
        ready["accessToken"] = json!("");
        assert!(validate_status(&ready, "live", &[]).is_err());
    }

    #[test]
    fn verifier_is_not_the_challenge() {
        let handoff = new_handoff(
            "instance".into(),
            "production".into(),
            "paper".into(),
            vec!["data".into()],
        );
        assert_ne!(handoff.verifier, handoff.challenge);
        assert!(handoff_ref(&handoff).starts_with("oauth-handoff-"));
    }

    #[test]
    fn start_from_an_active_tokio_runtime_returns_an_error_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("oauth-runtime.db");
        let db = db.to_string_lossy().to_string();
        let mut service = TradeAssemblyService::test_local(&db).for_authenticated_invocation(
            "oidc",
            "oauth-runtime-test",
            None,
            None,
        );
        service.oauth_config = Some(crate::runtime_config::RuntimeConfig::local(&db));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let response = runtime.block_on(async {
            start(
                &service,
                json!({
                    "instanceRef": "alpaca",
                    "environment": "production",
                    "scopes": []
                }),
            )
        });
        assert!(response.status >= 400);
        assert_ne!(response.body["error"]["code"], json!("unknown_mcp_tool"));
    }

    #[test]
    fn actor_token_from_an_active_tokio_runtime_is_isolated_on_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::runtime_config::RuntimeConfig::local(
            dir.path().join("oauth-runtime.db").to_string_lossy(),
        );
        config.oidc_session_path = dir
            .path()
            .join("missing-session.bin")
            .to_string_lossy()
            .to_string();
        config.oidc_session_key_path = dir
            .path()
            .join("missing-session.key")
            .to_string_lossy()
            .to_string();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result =
            runtime.block_on(async { run_blocking(|| actor_token(&config, "oauth-runtime-test")) });
        assert_eq!(result, Ok(Err("plugin_oauth_identity_required")));
    }

    #[test]
    fn invocation_actor_uses_the_canonical_stable_identity() {
        let raw = "subject-1";
        let stable = crate::adapters::studio_identity::stable_actor("issuer", raw);
        assert_eq!(invocation_actor("issuer", raw), raw);
        assert_eq!(invocation_actor("issuer", &stable), stable);
    }
}
