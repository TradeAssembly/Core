//! Bounded broker onboarding orchestration.
//!
//! This module deliberately does not accept credentials or submit orders.  It
//! exposes the existing plugin UI for connection and verifies the existing
//! `account.health` operation for an instance selected by the caller.

use super::{plugin_lifecycle, ServiceResponse, TradeAssemblyService};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use url::Url;

const RECEIPTS_NS: &str = "broker_onboarding_receipts_v1";
const RESTART_NS: &str = "broker_onboarding_restart_observations_v1";
const STUDIO_OBSERVATIONS_NS: &str = "broker_onboarding_studio_observations_v1";

#[derive(Clone, Debug)]
struct ProcessObservation {
    boot_id: String,
    started_at_ms: i64,
}

static PROCESS_OBSERVATION: OnceLock<ProcessObservation> = OnceLock::new();

pub(super) fn initialize_process_observation() {
    let _ = process_observation();
}

fn process_observation() -> &'static ProcessObservation {
    PROCESS_OBSERVATION.get_or_init(|| {
        let started_at_ms = now_ms();
        let mut entropy = [0_u8; 16];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut entropy);
        let boot_id = format!(
            "sha256:{:x}",
            Sha256::digest(
                format!("{}:{}:{entropy:?}", std::process::id(), started_at_ms).as_bytes()
            )
        );
        ProcessObservation {
            boot_id,
            started_at_ms,
        }
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

/// Return a deep link to the current plugin UI. No credentials are accepted.
pub(crate) fn connect(service: &TradeAssemblyService, body: Value) -> Value {
    let mode = match required_mode(&body) {
        Ok(mode) => mode,
        Err(error) => return error,
    };
    let base = body["studio_base_url"]
        .as_str()
        .or_else(|| body["studioBaseUrl"].as_str())
        .unwrap_or("http://127.0.0.1:3001");
    let mut connect_url = match Url::parse(base) {
        Ok(url)
            if matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) =>
        {
            url
        }
        _ => {
            return error(
                "studio_url_not_local",
                "Broker setup requires a local browser connection screen.",
            )
        }
    };
    connect_url.set_query(None);
    connect_url.set_fragment(None);
    let requested_ref = required_string(&body, &["instanceRef", "instance_ref"]);
    let plugin_ref = body
        .get("pluginRef")
        .or_else(|| body.get("plugin_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            plugin_lifecycle::list_manifests(service)["manifests"]
                .as_array()
                .and_then(|manifests| {
                    manifests.iter().find(|manifest| {
                        manifest["pluginRef"]
                            .as_str()
                            .is_some_and(|value| value.to_ascii_lowercase().contains("alpaca"))
                    })
                })
                .and_then(|manifest| manifest["pluginRef"].as_str())
                .map(str::to_string)
        });
    let mut instance_ref = if requested_ref.is_empty() {
        format!("alpaca-{mode}")
    } else {
        requested_ref.clone()
    };
    if requested_ref.is_empty()
        && service
            .runtime()
            .plugins
            .get_instance(&instance_ref)
            .ok()
            .flatten()
            .is_some()
        && plugin_lifecycle::require_instance(service, &instance_ref).is_err()
    {
        let Some(authority) = request_authority(&body) else {
            return error(
                "authority_context_required",
                "Choose an owned broker connection.",
            );
        };
        let digest = format!("{:x}", Sha256::digest(authority.actor.as_bytes()));
        instance_ref = format!("alpaca-{mode}-{}", &digest[..16]);
    }
    match plugin_lifecycle::require_instance(service, &instance_ref) {
        Ok(_) => {}
        Err(_) => {
            let Some(plugin_ref) = plugin_ref else {
                return error(
                    "plugin_manifest_not_installed",
                    "no installed Alpaca plugin manifest is available",
                );
            };
            let declaration = plugin_lifecycle::get_manifest(service, &plugin_ref).body;
            let fields = declaration["manifest"]["configuration"]["fields"].as_array();
            let mut configuration = json!({});
            for field in fields
                .into_iter()
                .flatten()
                .filter(|field| field["storageClass"] == "configuration")
            {
                let Some(id) = field["id"].as_str() else {
                    continue;
                };
                if let Some(value) = field.get("default") {
                    configuration[id] = value.clone();
                }
                if let Some(value) = field
                    .get("defaultByAccountMode")
                    .and_then(|defaults| defaults.get(mode.as_str()))
                {
                    configuration[id] = value.clone();
                }
                if matches!(id, "mode" | "account_mode") {
                    configuration[id] = json!(mode);
                }
            }
            let created = plugin_lifecycle::create_instance(
                service,
                json!({
                    "instanceRef": instance_ref,
                    "pluginRef": plugin_ref,
                    "providerRef": instance_ref,
                    "accountMode": mode,
                    "configuration": configuration,
                    "enabled": true,
                }),
            );
            if created.status != 200 {
                return response_value(created);
            }
        }
    };
    let instance = match require_alpaca(service, &instance_ref) {
        Ok(instance) => instance,
        Err(error) => return error,
    };
    if let Some(existing_mode) = configured_mode(&instance) {
        if existing_mode != mode {
            return error(
                "account_mode_mismatch",
                "instance mode does not match requested mode",
            );
        }
    }
    connect_url.set_path("/app/plugins-providers");
    connect_url
        .query_pairs_mut()
        .append_pair("instance", &instance_ref);
    json!({
        "ok": true,
        "instanceRef": instance_ref,
        "providerRef": instance["providerRef"],
        "mode": mode,
        "connectionMethod": "plugin_ui",
        "credentialsAccepted": false,
        "connectUrl": connect_url.to_string(),
        "message": "Use the plugin connection UI to authorize access or enter API credentials, then verify the connection. This interface never receives credentials.",
    })
}

/// Return installation/configuration/health/verification separately. An
/// omitted instanceRef lists all visible plugin instances.
pub(crate) fn status(service: &TradeAssemblyService, body: Value) -> Value {
    let requested = body
        .get("instanceRef")
        .or_else(|| body.get("instance_ref"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let records = if let Some(instance_ref) = requested {
        match plugin_lifecycle::require_instance(service, &instance_ref) {
            Ok(record) => vec![record],
            Err(response) => return response_value(response),
        }
    } else {
        plugin_lifecycle::list_instances(service)["instances"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let instances = records
        .into_iter()
        .filter(is_alpaca)
        .map(|record| status_record(service, record))
        .collect::<Vec<_>>();
    json!({"ok": true, "instances": instances})
}

/// Refresh and verify an account health check, then persist a narrow receipt.
pub(crate) fn verify(service: &TradeAssemblyService, body: Value) -> Value {
    let instance_ref = required_string(&body, &["instanceRef", "instance_ref"]);
    let mode = match required_mode(&body) {
        Ok(mode) => mode,
        Err(error) => return error,
    };
    let before = match require_alpaca(service, &instance_ref) {
        Ok(instance) => instance,
        Err(error) => return error,
    };
    if configured_mode(&before) != Some(mode.as_str()) {
        return error(
            "account_mode_mismatch",
            "Select a connection configured for the requested environment.",
        );
    }
    let refreshed = plugin_lifecycle::refresh_health(service, &instance_ref);
    if refreshed.status != 200 {
        return response_value(refreshed);
    }
    let instance = refreshed.body.get("instance").cloned().unwrap_or(before);
    let health = &instance["health"];
    let checked = health["connectivityChecked"].as_bool().unwrap_or(false);
    let account = health.get("account").cloned().unwrap_or(Value::Null);
    let account_id = account["id"].as_str().filter(|id| !id.is_empty());
    let account_mode = account["mode"].as_str();
    let credential_configured = instance["credentialStatus"]["configured"]
        .as_bool()
        .unwrap_or(false);
    if health["state"] != "ready" || !credential_configured || !checked || account_id.is_none() {
        return json!({
            "ok": false, "instanceRef": instance_ref, "mode": mode,
            "verified": false, "connectivityChecked": checked,
            "diagnostics": health["diagnostics"],
            "error": {"code": "account_health_incomplete", "message": "account.health did not return a usable account id"}
        });
    }
    if account_mode != Some(mode.as_str()) {
        return json!({
            "ok": false, "instanceRef": instance_ref, "mode": mode,
            "verified": false, "connectivityChecked": checked,
            "error": {"code": "account_mode_mismatch", "message": "broker account mode does not match requested mode", "details": {"accountMode": account_mode}}
        });
    }
    let receipt = receipt(
        service,
        &instance,
        &instance_ref,
        mode.clone(),
        account_id.unwrap(),
    );
    let key = format!("{instance_ref}:{}", receipt["instanceRevision"]);
    let Some(authority) = request_authority(&body) else {
        return error(
            "authority_context_required",
            "Account verification requires owner authority.",
        );
    };
    let Ok(idempotency) = IdempotencyKey::new(format!(
        "broker.verify:{}",
        required_string(&body, &["idempotency_key"])
    )) else {
        return error(
            "idempotency_required",
            "Verification requires an idempotency key.",
        );
    };
    let context = SideEffectContext::new(authority, idempotency);
    if let Err(message) =
        service
            .runtime()
            .storage
            .put_json(RECEIPTS_NS, &key, receipt.clone(), &context)
    {
        return json!({"ok": false, "instanceRef": instance_ref, "error": {"code": "verification_receipt_persist_failed", "message": message}});
    }
    let restart_observation_persisted =
        persist_restart_observation(service, &receipt, &context).is_ok();
    json!({
        "ok": true, "instanceRef": instance_ref, "providerRef": instance["providerRef"],
        "mode": mode, "verified": true, "connectivityChecked": true,
        "account": {"id": account_id.unwrap(), "mode": account_mode},
        "receipt": receipt,
        "restartObservationPersisted": restart_observation_persisted,
        "restartObservation": restart_observation_status(service, &instance),
    })
}

/// Record a trusted Studio-side setup observation. This is callable only from
/// the authenticated Studio GraphQL path; it never refreshes broker health.
pub(crate) fn observe_studio_setup(
    service: &TradeAssemblyService,
    body: Value,
    principal: &crate::auth::SessionPrincipal,
) -> Value {
    let instance_ref = required_string(&body, &["instanceRef", "instance_ref"]);
    let mode = match required_mode(&body) {
        Ok(mode) => mode,
        Err(error) => return error,
    };
    let idempotency_key = match body
        .get("idempotencyKey")
        .or_else(|| body.get("idempotency_key"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| IdempotencyKey::new(value).ok())
    {
        Some(value) => value,
        None => {
            return error(
                "idempotency_required",
                "Studio observation requires an idempotency key.",
            )
        }
    };
    let Some(origin) = service.studio_base_url() else {
        return error(
            "studio_origin_unavailable",
            "Trusted Studio origin is not configured.",
        );
    };
    let instance = match require_alpaca(service, &instance_ref) {
        Ok(instance) => instance,
        Err(error) => return error,
    };
    let Some(receipt) = latest_receipt(service, &instance) else {
        return error(
            "broker_verification_required",
            "A current successful broker verification is required.",
        );
    };
    if !receipt_matches(&instance, &receipt)
        || receipt["mode"].as_str() != Some(mode.as_str())
        || instance["health"]["account"]["mode"].as_str() != Some(mode.as_str())
    {
        return error(
            "broker_verification_stale",
            "The broker verification receipt is stale or does not match the requested mode.",
        );
    }
    let issuer = principal.issuer.as_str();
    let subject = principal.subject.as_str();
    if issuer.is_empty() || subject.is_empty() {
        return error(
            "studio_identity_required",
            "A verified Studio human identity is required.",
        );
    }
    let workspace_id = workspace_id(service.db());
    let receipt_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json_canonicalizer::to_vec(&receipt).unwrap_or_default())
    );
    let observation = json!({
        "schemaVersion": "tradeassembly.studio_setup_observation.v1",
        "workspaceId": workspace_id,
        "instanceRef": instance_ref,
        "mode": mode,
        "accountId": receipt["accountId"],
        "owner": {"issuer": issuer, "subject": subject},
        "studioOrigin": origin,
        "verificationReceiptHash": receipt_hash,
        "instanceRevision": receipt["instanceRevision"],
        "activePackageSha256": receipt["activePackageSha256"],
        "credentialRevision": receipt["credentialRevision"],
        "configurationSha256": receipt["configurationSha256"],
        "observedAtMs": service.runtime().clock.now_ms(),
    });
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: subject.to_string(),
            surface: "studio".to_string(),
            account_mode: mode.clone(),
        },
        idempotency_key,
    );
    let key = format!("{}:{}", instance_ref, context.idempotency_key.as_str());
    if let Err(message) = service.runtime().storage.put_json(
        STUDIO_OBSERVATIONS_NS,
        &key,
        observation.clone(),
        &context,
    ) {
        return error("studio_observation_persist_failed", &message);
    }
    json!({"ok": true, "observation": observation})
}

fn workspace_id(db: &str) -> String {
    let path = std::path::Path::new(db);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(path)
    };
    format!(
        "workspace:sha256:{:x}",
        Sha256::digest(absolute.to_string_lossy().as_bytes())
    )
}

fn status_record(service: &TradeAssemblyService, instance: Value) -> Value {
    let instance_ref = instance["instanceRef"].as_str().unwrap_or_default();
    let credential = plugin_lifecycle::credential_status_for(service, instance_ref);
    let credential_status = credential
        .body
        .get("credentialStatus")
        .cloned()
        .unwrap_or(Value::Null);
    let receipt = latest_receipt(service, &instance);
    let stale = receipt
        .as_ref()
        .is_some_and(|receipt| !receipt_matches(&instance, receipt));
    let health = &instance["health"];
    let restart = restart_observation_status(service, &instance);
    let studio_observation = studio_observation_status(service, &instance);
    json!({
        "instanceRef": instance["instanceRef"], "providerRef": instance["providerRef"],
        "pluginRef": instance["pluginRef"], "installed": true,
        "mode": configured_mode(&instance),
        "enabled": instance["enabled"],
        "configured": credential_status["configured"],
        "credentialStatus": credential_status,
        "health": health,
        "accountVerified": receipt.is_some() && !stale && credential_status["configured"] == true && health["state"] == "ready",
        "verificationStale": stale,
        "verificationReceipt": receipt,
        "restartObservation": restart,
        "studioObservation": studio_observation,
        "nextAction": connection_next_action(&instance, credential_status["configured"] == true, service.runtime().clock.now_ms()),
        "declaredCapabilities": declared_capabilities(service, &instance["pluginRef"]),
        "accountTestedCapabilities": if receipt.is_some() && !stale { json!(["account.health"]) } else { json!([]) },
    })
}

fn studio_observation_status(service: &TradeAssemblyService, instance: &Value) -> Value {
    let Some(instance_ref) = instance["instanceRef"].as_str() else {
        return json!({"status": "not_run", "reason": "instance_unavailable"});
    };
    let Some(receipt) = latest_receipt(service, instance) else {
        return json!({"status": "not_run", "reason": "broker_verification_required"});
    };
    let Some((issuer, subject)) = service.responsible_human_identity() else {
        return json!({"status": "not_run", "reason": "studio_identity_required"});
    };
    let current_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json_canonicalizer::to_vec(&receipt).unwrap_or_default())
    );
    let observation = service
        .runtime()
        .storage
        .list_json(STUDIO_OBSERVATIONS_NS)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .filter(|value| {
            value["instanceRef"].as_str() == Some(instance_ref)
                && value["owner"]["issuer"].as_str() == Some(issuer)
                && value["owner"]["subject"].as_str() == Some(subject)
        })
        .max_by_key(|value| value["observedAtMs"].as_i64().unwrap_or(0));
    let Some(observation) = observation else {
        return json!({"status": "not_run", "reason": "studio_observation_required"});
    };
    let valid = receipt_matches(instance, &receipt)
        && receipt["accountId"] == instance["health"]["account"]["id"]
        && receipt["mode"] == instance["health"]["account"]["mode"]
        && observation["workspaceId"] == workspace_id(service.db())
        && observation["mode"] == receipt["mode"]
        && observation["accountId"] == receipt["accountId"]
        && observation["owner"]["issuer"].as_str() == Some(issuer)
        && observation["owner"]["subject"].as_str() == Some(subject)
        && observation["studioOrigin"].as_str() == service.studio_base_url()
        && observation["verificationReceiptHash"] == current_hash
        && observation["instanceRevision"] == instance["updatedAtMs"]
        && observation["activePackageSha256"] == instance["activePackageSha256"]
        && observation["credentialRevision"] == instance["credentialRevision"]
        && observation["configurationSha256"] == receipt["configurationSha256"];
    json!({
        "status": if valid { "passed" } else { "stale" },
        "observation": observation,
        "reason": if valid { Value::Null } else { json!("Stored Studio observation no longer matches current owner, origin, broker receipt, or instance revision.") },
    })
}

fn persist_restart_observation(
    service: &TradeAssemblyService,
    receipt: &Value,
    request_context: &SideEffectContext,
) -> Result<(), String> {
    let observation = process_observation();
    let record = json!({
        "schemaVersion": "tradeassembly.broker_restart_observation.v1",
        "instanceRef": receipt["instanceRef"],
        "processBootId": observation.boot_id,
        "processStartedAtMs": observation.started_at_ms,
        "checkedAt": receipt["checkedAt"],
        "verificationReceipt": receipt,
        "instanceRevision": receipt["instanceRevision"],
        "activePackageSha256": receipt["activePackageSha256"],
        "credentialRevision": receipt["credentialRevision"],
        "configurationSha256": receipt["configurationSha256"],
        "mode": receipt["mode"],
        "accountId": receipt["accountId"],
        "verificationReceiptHash": format!(
            "sha256:{:x}",
            Sha256::digest(serde_json_canonicalizer::to_vec(receipt).unwrap_or_default())
        ),
        "authorityActor": request_context.authority.actor.clone(),
    });
    let key = format!(
        "{}:{}",
        receipt["instanceRef"].as_str().unwrap_or_default(),
        observation.boot_id
    );
    let context = SideEffectContext::new(
        request_context.authority.clone(),
        IdempotencyKey::new(format!("broker.restart-observation:{key}"))?,
    );
    service
        .runtime()
        .storage
        .put_json(RESTART_NS, &key, record, &context)
}

fn restart_observation_status(service: &TradeAssemblyService, instance: &Value) -> Value {
    let Some(principal) = service.invocation_principal.as_ref() else {
        return json!({"status": "not_run", "reason": "authenticated_owner_required"});
    };
    let Some(instance_ref) = instance["instanceRef"].as_str() else {
        return json!({"status": "not_run", "reason": "instance_unavailable"});
    };
    let current = process_observation();
    let latest = latest_receipt(service, instance);
    let records = service
        .runtime()
        .storage
        .list_json(RESTART_NS)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, value)| {
            (value["instanceRef"].as_str() == Some(instance_ref)).then_some(value)
        })
        .collect::<Vec<_>>();
    let current_binding = records.iter().find(|record| {
        record["processBootId"].as_str() == Some(current.boot_id.as_str())
            && record_matches_instance(instance, record)
            && record["authorityActor"].as_str() == Some(principal.subject.as_str())
            && latest.as_ref() == record.get("verificationReceipt")
            && record["checkedAt"]
                .as_i64()
                .is_some_and(|checked| checked >= current.started_at_ms)
    });
    let prior_binding = records.iter().find(|record| {
        record["processBootId"].as_str() != Some(current.boot_id.as_str())
            && record_matches_instance(instance, record)
            && record["authorityActor"].as_str() == Some(principal.subject.as_str())
            && record["checkedAt"]
                .as_i64()
                .is_some_and(|checked| checked < current.started_at_ms)
    });
    let current_checked_at = current_binding
        .and_then(|record| record["checkedAt"].as_i64())
        .unwrap_or_default();
    let ordered = prior_binding
        .is_some_and(|record| prior_observation_is_ordered(record, current, current_checked_at));
    json!({
        "status": if current_binding.is_some() && ordered { "passed" } else { "not_run" },
        "currentProcessObserved": current_binding.is_some(),
        "priorProcessObserved": ordered,
        "processBootId": current.boot_id,
        "processStartedAtMs": current.started_at_ms,
        "reason": if current_binding.is_some() && ordered { Value::Null } else { json!("A fresh successful verification in a distinct process is required after the prior observation.") },
    })
}

fn record_matches_instance(instance: &Value, record: &Value) -> bool {
    let configuration_hash = Sha256::digest(
        serde_json_canonicalizer::to_vec(&instance["configuration"]).unwrap_or_default(),
    );
    let receipt = &record["verificationReceipt"];
    let receipt_consistent = receipt.is_object()
        && [
            "instanceRef",
            "instanceRevision",
            "activePackageSha256",
            "credentialRevision",
            "configurationSha256",
            "mode",
            "accountId",
            "checkedAt",
        ]
        .iter()
        .all(|key| receipt[*key] == record[*key])
        && serde_json_canonicalizer::to_vec(receipt)
            .ok()
            .is_some_and(|encoded| {
                record["verificationReceiptHash"] == format!("sha256:{:x}", Sha256::digest(encoded))
            });
    receipt_consistent
        && record["accountId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        && record["instanceRevision"] == instance["updatedAtMs"]
        && record["activePackageSha256"] == instance["activePackageSha256"]
        && record["credentialRevision"].as_u64().unwrap_or(0)
            == instance["credentialRevision"].as_u64().unwrap_or(0)
        && record["configurationSha256"] == format!("sha256:{configuration_hash:x}")
        && record["accountId"] == instance["health"]["account"]["id"]
        && record["mode"]
            == configured_mode(instance)
                .map(|mode| Value::String(mode.to_string()))
                .unwrap_or(Value::Null)
        && record["mode"] == instance["health"]["account"]["mode"]
        && record["authorityActor"]
            .as_str()
            .is_some_and(|actor| !actor.trim().is_empty())
}

fn prior_observation_is_ordered(
    prior: &Value,
    current: &ProcessObservation,
    current_checked_at: i64,
) -> bool {
    prior["processBootId"]
        .as_str()
        .is_some_and(|boot| !boot.is_empty())
        && prior["processBootId"].as_str() != Some(current.boot_id.as_str())
        && prior["processStartedAtMs"]
            .as_i64()
            .is_some_and(|started| started < current.started_at_ms)
        && prior["checkedAt"]
            .as_i64()
            .is_some_and(|checked| checked < current.started_at_ms && checked < current_checked_at)
        && current_checked_at >= current.started_at_ms
}

fn connection_next_action(instance: &Value, configured: bool, now_ms: i64) -> Value {
    let Some(mode) = configured_mode(instance).filter(|mode| matches!(*mode, "paper" | "live"))
    else {
        return json!({"action":"select_account_mode", "message":"Choose paper or live before connecting this account."});
    };
    let Some(instance_ref) = instance["instanceRef"]
        .as_str()
        .filter(|value| !value.is_empty())
    else {
        return json!({"action":"inspect_connection", "message":"The connection has no usable instance reference."});
    };
    json!({
        "tool": if configured { "tradeassembly.broker.verify" } else { "tradeassembly.broker.connect" },
        "arguments": {
            "instanceRef": instance_ref, "mode": mode,
            "idempotency_key": format!("broker-setup-{instance_ref}-{now_ms}"),
        },
        "message": if configured { "Read account health for this configured environment. This does not place an order." }
            else { "Open this connection's plugin-owned credential screen." },
    })
}

fn configured_mode(instance: &Value) -> Option<&str> {
    instance["accountMode"]
        .as_str()
        .or_else(|| instance["configuration"]["mode"].as_str())
        .or_else(|| instance["configuration"]["account_mode"].as_str())
}

fn require_alpaca(service: &TradeAssemblyService, instance_ref: &str) -> Result<Value, Value> {
    if instance_ref.is_empty() {
        return Err(error("instance_ref_required", "instanceRef is required"));
    }
    let record =
        plugin_lifecycle::require_instance(service, instance_ref).map_err(response_value)?;
    if !is_alpaca(&record) {
        return Err(error(
            "unsupported_provider",
            "broker onboarding currently supports Alpaca only",
        ));
    }
    Ok(record)
}

fn is_alpaca(record: &Value) -> bool {
    ["pluginRef", "providerRef"]
        .iter()
        .filter_map(|key| record[*key].as_str())
        .any(|value| value.to_ascii_lowercase().contains("alpaca"))
}

fn declared_capabilities(service: &TradeAssemblyService, plugin_ref: &Value) -> Value {
    let Some(plugin_ref) = plugin_ref.as_str() else {
        return json!([]);
    };
    let response = plugin_lifecycle::get_manifest(service, plugin_ref);
    response.body["manifest"]["capabilities"]
        .clone()
        .as_array()
        .map(|caps| {
            Value::Array(
                caps.iter()
                    .filter_map(|cap| cap["id"].as_str().map(|id| json!(id)))
                    .collect(),
            )
        })
        .unwrap_or_else(|| json!([]))
}

fn receipt(
    service: &TradeAssemblyService,
    instance: &Value,
    instance_ref: &str,
    mode: String,
    account_id: &str,
) -> Value {
    let credential_revision = instance["credentialRevision"].as_u64().unwrap_or(0);
    let configuration_hash = Sha256::digest(
        serde_json_canonicalizer::to_vec(&instance["configuration"]).unwrap_or_default(),
    );
    json!({
        "schemaVersion": "tradeassembly.broker_onboarding_receipt.v1",
        "instanceRef": instance_ref, "instanceRevision": instance["updatedAtMs"],
        "activePackageSha256": instance["activePackageSha256"],
        "credentialRevision": credential_revision,
        "configurationSha256": format!("sha256:{configuration_hash:x}"),
        "mode": mode, "accountId": account_id,
        "checkedAt": service.runtime().clock.now_ms(),
    })
}

fn latest_receipt(service: &TradeAssemblyService, instance: &Value) -> Option<Value> {
    let instance_ref = instance["instanceRef"].as_str()?;
    service
        .runtime()
        .storage
        .list_json(RECEIPTS_NS)
        .ok()?
        .into_iter()
        .filter_map(|(_, value)| {
            (value["instanceRef"].as_str() == Some(instance_ref)).then_some(value)
        })
        .max_by_key(|value| value["checkedAt"].as_i64().unwrap_or(0))
}

fn receipt_matches(instance: &Value, receipt: &Value) -> bool {
    let configuration_hash = Sha256::digest(
        serde_json_canonicalizer::to_vec(&instance["configuration"]).unwrap_or_default(),
    );
    receipt["instanceRevision"] == instance["updatedAtMs"]
        && receipt["activePackageSha256"] == instance["activePackageSha256"]
        && receipt["credentialRevision"].as_u64().unwrap_or(0)
            == instance["credentialRevision"].as_u64().unwrap_or(0)
        && receipt["configurationSha256"] == format!("sha256:{configuration_hash:x}")
        && receipt["accountId"] == instance["health"]["account"]["id"]
}

fn required_string(body: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| body.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

fn required_mode(body: &Value) -> Result<String, Value> {
    match body.get("mode").and_then(Value::as_str) {
        Some("paper") => Ok("paper".to_string()),
        Some("live") => Ok("live".to_string()),
        _ => Err(error(
            "mode_required",
            "mode must be explicitly paper or live",
        )),
    }
}

/// Validate command-envelope fields before any onboarding write is attempted.
/// Credentials are intentionally rejected at this boundary.
pub(crate) fn validate_mcp_arguments(name: &str, arguments: &Value) -> Option<&'static str> {
    let Some(object) = arguments.as_object() else {
        return Some("broker_arguments_invalid");
    };
    if matches!(
        name,
        "tradeassembly.broker.connect" | "tradeassembly.broker.verify"
    ) {
        if object
            .get("idempotency_key")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            return Some("idempotency_key_required");
        }
        if object.get("instanceRef").and_then(Value::as_str).is_none()
            && object.get("instance_ref").and_then(Value::as_str).is_none()
            && name == "tradeassembly.broker.verify"
        {
            return Some("instance_ref_required");
        }
        if object.get("mode").and_then(Value::as_str) != Some("paper")
            && object.get("mode").and_then(Value::as_str) != Some("live")
        {
            return Some("mode_required");
        }
        if object.contains_key("authority_context") && request_authority(arguments).is_none() {
            return Some("authority_context_invalid");
        }
        if request_authority(arguments).is_some_and(|authority| {
            Some(authority.account_mode.as_str()) != arguments["mode"].as_str()
        }) {
            return Some("authority_mode_mismatch");
        }
    }
    if matches!(
        name,
        "tradeassembly.broker.connect"
            | "tradeassembly.broker.status"
            | "tradeassembly.broker.verify"
    ) && contains_credential_key(arguments)
    {
        return Some("credentials_must_be_entered_in_plugin_ui");
    }
    None
}

/// Validate the caller-supplied compatibility authority before authenticated
/// binding replaces its actor. The actor is never trusted; only a supplied
/// mode conflict is meaningful at this stage.
pub(crate) fn validate_requested_mode_authority(arguments: &Value) -> Option<&'static str> {
    let object = arguments.as_object()?;
    let mode = object.get("mode").and_then(Value::as_str)?;
    let authority = object
        .get("authority_context")
        .or_else(|| object.get("authorityContext"))?;
    let supplied_mode = authority
        .get("account_mode")
        .or_else(|| authority.get("accountMode"))
        .and_then(Value::as_str);
    if supplied_mode.is_some_and(|supplied_mode| supplied_mode != mode) {
        Some("authority_mode_mismatch")
    } else {
        None
    }
}

fn contains_credential_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            [
                "credential",
                "credentials",
                "secret",
                "password",
                "api_key",
                "api_secret",
                "token",
                "authorization",
            ]
            .iter()
            .any(|term| normalized.contains(term))
                || contains_credential_key(value)
        }),
        Value::Array(items) => items.iter().any(contains_credential_key),
        _ => false,
    }
}

fn response_value(response: ServiceResponse) -> Value {
    response.body
}

fn error(code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message}})
}

fn request_authority(body: &Value) -> Option<AuthorityContext> {
    let value = &body["authority_context"];
    let actor = value["actor"].as_str()?.trim();
    let surface = value["surface"].as_str()?.trim();
    let mode = value["account_mode"]
        .as_str()
        .or_else(|| value["accountMode"].as_str())?;
    if actor.is_empty() || surface.is_empty() || !matches!(mode, "paper" | "live") {
        return None;
    }
    Some(AuthorityContext {
        actor: actor.into(),
        surface: surface.into(),
        account_mode: mode.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finance_authority::TestFinanceAuthority;
    use crate::runtime_config::RuntimeBuilder;
    use crate::runtime_config::RuntimeConfig;
    use base64::Engine as _;
    use std::sync::Arc;

    #[test]
    fn connection_next_actions_are_valid_and_never_invent_mode() {
        for mode in ["paper", "live"] {
            for configured in [false, true] {
                let action = connection_next_action(
                    &json!({"instanceRef":"owned-test","configuration":{"mode":mode}}),
                    configured,
                    1234,
                );
                assert_eq!(action["arguments"]["mode"], mode);
                assert_eq!(
                    validate_mcp_arguments(action["tool"].as_str().unwrap(), &action["arguments"]),
                    None
                );
                assert_eq!(
                    action["tool"],
                    if configured {
                        "tradeassembly.broker.verify"
                    } else {
                        "tradeassembly.broker.connect"
                    }
                );
            }
        }
        let missing = connection_next_action(&json!({"instanceRef":"test"}), true, 1234);
        assert_eq!(missing["action"], "select_account_mode");
        assert!(missing["tool"].is_null());
    }

    fn fixture_service(db: &str) -> TradeAssemblyService {
        let service = TradeAssemblyService::test_local(db.to_string());
        let mut manifest = plugin_lifecycle::get_manifest(&service, "tradeassembly.simbroker").body;
        manifest["pluginRef"] = json!("test.alpaca");
        manifest["manifest"]["metadata"]["id"] = json!("test.alpaca");
        manifest["manifest"]["configuration"] = json!({"fields":[
            {"id":"mode","inputType":"select","options":["paper","live"],"storageClass":"configuration","required":true},
            {"id":"tradingApiUrl","inputType":"url","storageClass":"configuration","required":true,"defaultByAccountMode":{"paper":"https://paper.example.test","live":"https://live.example.test"}},
            {"id":"api_key","inputType":"password","storageClass":"credential","required":true}
        ]});
        manifest["manifest"]["health"]["requiredConfiguration"] = json!(["tradingApiUrl"]);
        manifest["manifest"]["health"]["requiredCredentials"] = json!(["api_key"]);
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("fixture-manifest").unwrap(),
        );
        service
            .runtime()
            .plugins
            .install_manifest("test.alpaca", manifest, &context)
            .unwrap();
        service
    }

    fn studio_fixture_service(db: &str) -> TradeAssemblyService {
        let token_path = std::path::Path::new(db).with_extension("studio-core.token");
        std::fs::write(
            &token_path,
            "broker-onboarding-studio-core-token-with-32-bytes",
        )
        .unwrap();
        let mut config = RuntimeConfig::local(db.to_string());
        config.oidc_issuer = "https://id.broker-onboarding.test".to_string();
        config.oidc_audience = "tradeassembly-broker-onboarding".to_string();
        config.oidc_client_id = config.oidc_audience.clone();
        config.studio_core_token_ref = Some(format!("file://{}", token_path.display()));
        config.studio_base_url = Some("http://127.0.0.1:3002/".to_string());
        let (runtime, manifest) = RuntimeBuilder::new(config.clone())
            .with_finance_authority(Arc::new(TestFinanceAuthority))
            .build()
            .unwrap();
        fixture_service_from_runtime(TradeAssemblyService {
            db: db.to_string(),
            runtime: Arc::new(runtime),
            runtime_manifest: Some(manifest),
            oauth_config: Some(config),
            invocation_principal: None,
            invocation_actor_kind: "user",
            agent_mcp_execution_context: None,
        })
    }

    fn fixture_service_from_runtime(service: TradeAssemblyService) -> TradeAssemblyService {
        let mut manifest = plugin_lifecycle::get_manifest(&service, "tradeassembly.simbroker").body;
        manifest["pluginRef"] = json!("test.alpaca");
        manifest["manifest"]["metadata"]["id"] = json!("test.alpaca");
        manifest["manifest"]["configuration"] = json!({"fields":[
            {"id":"mode","inputType":"select","options":["paper","live"],"storageClass":"configuration","required":true},
            {"id":"tradingApiUrl","inputType":"url","storageClass":"configuration","required":true,"defaultByAccountMode":{"paper":"https://paper.example.test","live":"https://live.example.test"}},
            {"id":"api_key","inputType":"password","storageClass":"credential","required":true}
        ]});
        manifest["manifest"]["health"]["requiredConfiguration"] = json!(["tradingApiUrl"]);
        manifest["manifest"]["health"]["requiredCredentials"] = json!(["api_key"]);
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("studio-fixture-manifest").unwrap(),
        );
        service
            .runtime()
            .plugins
            .install_manifest("test.alpaca", manifest, &context)
            .unwrap();
        service
    }

    fn prepared_studio_instance(db: &str) -> (TradeAssemblyService, Value) {
        let base = studio_fixture_service(db);
        let service = base.for_authenticated_invocation(
            "https://id.broker-onboarding.test",
            studio_subject(),
            None,
            Some("Broker Owner".to_string()),
        );
        assert_eq!(
            connect(
                &service,
                json!({
                    "instanceRef":"alpaca-paper", "pluginRef":"test.alpaca", "mode":"paper"
                })
            )["ok"],
            true
        );
        service.claim_local_seed_plugin_instances();
        let mut instance = plugin_lifecycle::require_instance(&service, "alpaca-paper").unwrap();
        instance["health"] = json!({
            "state":"ready", "connectivityChecked":true,
            "account":{"id":"acct-studio", "mode":"paper"}
        });
        let context = SideEffectContext::new(
            AuthorityContext {
                actor: "broker-owner".to_string(),
                surface: "test".to_string(),
                account_mode: "paper".to_string(),
            },
            IdempotencyKey::new("studio-fixture-instance").unwrap(),
        );
        service
            .runtime()
            .plugins
            .put_instance("alpaca-paper", instance.clone(), &context)
            .unwrap();
        service
            .bind_owned_object("plugin_instance", "alpaca-paper")
            .unwrap();
        service
            .bind_inherited_object(
                "credential_reference",
                "alpaca-paper",
                "plugin_instance",
                "alpaca-paper",
            )
            .unwrap();
        let verified = receipt(
            &service,
            &instance,
            "alpaca-paper",
            "paper".to_string(),
            "acct-studio",
        );
        service
            .runtime()
            .storage
            .put_json(RECEIPTS_NS, "studio-fixture-receipt", verified, &context)
            .unwrap();
        (service, instance)
    }

    fn studio_request(idempotency_key: &str, mode: &str) -> Value {
        json!({
            "operationName":"ObserveStudioSetup",
            "query":"mutation ObserveStudioSetup($instanceRef: String!, $mode: String!, $idempotencyKey: String!) { observeStudioSetup(instanceRef: $instanceRef, mode: $mode, idempotencyKey: $idempotencyKey) }",
            "variables": {
                "instanceRef":"alpaca-paper", "mode":mode, "idempotencyKey":idempotency_key,
                "_studioSession": {
                    "issuer":"https://id.broker-onboarding.test",
                    "subject":"broker-owner",
                    "audience":["tradeassembly-broker-onboarding"],
                    "actor":studio_actor(),
                    "displayName":"Broker Owner",
                    "email":"owner@example.test",
                    "expiresAtMs":1900000000000_i64
                }
            }
        })
    }

    fn studio_actor() -> String {
        format!(
            "oidc:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(
                b"https://id.broker-onboarding.test\0broker-owner"
            ))
        )
    }

    fn studio_subject() -> String {
        studio_actor()
    }

    #[test]
    fn observe_studio_setup_authenticated_transport_persists_verified_owner_observation() {
        let dir = tempfile::tempdir().unwrap();
        let (service, _) = prepared_studio_instance(dir.path().join("studio.db").to_str().unwrap());
        let response = service.handle_studio_graphql(
            Some("broker-onboarding-studio-core-token-with-32-bytes"),
            studio_request("studio-observe-1", "paper"),
        );
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(
            response.body["data"]["observeStudioSetup"]["ok"], true,
            "{}",
            response.body
        );
        let observation = &response.body["data"]["observeStudioSetup"]["observation"];
        assert_eq!(
            observation["owner"]["issuer"],
            "https://id.broker-onboarding.test"
        );
        assert_eq!(observation["owner"]["subject"], studio_subject());
        assert_ne!(observation["owner"]["subject"], "forged");
        assert_eq!(
            service
                .runtime()
                .storage
                .list_json(STUDIO_OBSERVATIONS_NS)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn observe_studio_setup_rejects_normal_or_unauthenticated_transport_without_persisting() {
        let dir = tempfile::tempdir().unwrap();
        let (service, _) = prepared_studio_instance(dir.path().join("studio.db").to_str().unwrap());
        let request = studio_request("studio-normal", "paper");
        let normal = service.execute_graphql(request.clone());
        assert_eq!(
            normal["errors"][0]["message"],
            "studio_authentication_required"
        );
        let unauthenticated = service.handle_studio_graphql(None, request);
        assert_eq!(unauthenticated.status, 401);
        assert_eq!(
            unauthenticated.body["error"]["code"],
            "studio_transport_authentication_required"
        );
        assert!(service
            .runtime()
            .storage
            .list_json(STUDIO_OBSERVATIONS_NS)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn observe_studio_setup_rejects_wrong_mode_stale_configuration_and_health_account() {
        let dir = tempfile::tempdir().unwrap();
        let (service, mut instance) =
            prepared_studio_instance(dir.path().join("studio.db").to_str().unwrap());
        let principal = service.invocation_principal.clone().unwrap();
        assert_eq!(
            observe_studio_setup(
                &service,
                json!({"instanceRef":"alpaca-paper", "mode":"live", "idempotencyKey":"wrong-mode"}),
                &principal
            )["error"]["code"],
            "broker_verification_stale"
        );
        instance["configuration"]["tradingApiUrl"] = json!("https://changed.example.test");
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("studio-stale-config").unwrap(),
        );
        service
            .runtime()
            .plugins
            .put_instance("alpaca-paper", instance.clone(), &context)
            .unwrap();
        assert_eq!(
            observe_studio_setup(
                &service,
                json!({"instanceRef":"alpaca-paper", "mode":"paper", "idempotencyKey":"stale-config"}),
                &principal
            )["error"]["code"],
            "broker_verification_stale"
        );
        instance["configuration"]["tradingApiUrl"] = json!("https://paper.example.test");
        instance["health"]["account"]["id"] = json!("wrong-account");
        service
            .runtime()
            .plugins
            .put_instance("alpaca-paper", instance, &context)
            .unwrap();
        assert_eq!(
            observe_studio_setup(
                &service,
                json!({"instanceRef":"alpaca-paper", "mode":"paper", "idempotencyKey":"health-account"}),
                &principal
            )["error"]["code"],
            "broker_verification_stale"
        );
    }

    #[test]
    fn studio_observation_status_filters_foreign_owner_without_receipt_leak() {
        let dir = tempfile::tempdir().unwrap();
        let (service, instance) =
            prepared_studio_instance(dir.path().join("studio.db").to_str().unwrap());
        let principal = crate::auth::SessionPrincipal {
            provider: "oidc".to_string(),
            issuer: "https://id.broker-onboarding.test".to_string(),
            subject: studio_subject(),
            email: None,
            email_verified: None,
            name: None,
            picture: None,
        };
        let observed = observe_studio_setup(
            &service,
            json!({"instanceRef":"alpaca-paper", "mode":"paper", "idempotencyKey":"direct-observe"}),
            &principal,
        );
        assert_eq!(observed["ok"], true);
        let foreign = service.for_authenticated_invocation(
            "https://id.broker-onboarding.test",
            "foreign-owner",
            None,
            None,
        );
        let status = studio_observation_status(&foreign, &instance);
        assert_eq!(status["status"], "not_run");
        assert!(status.get("observation").is_none());
    }

    #[test]
    fn clean_connection_creation_is_durable_and_does_not_prove_broker_or_start_automation() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("onboarding.db");
        let service = fixture_service(db.to_str().unwrap()).for_authenticated_invocation(
            "test-issuer",
            "verified-owner",
            None,
            None,
        );
        for mode in ["paper", "live"] {
            let response = service.call_mcp_tool(
                "tradeassembly.broker.connect",
                json!({
                    "mode":mode,
                    "idempotency_key":format!("connect-{mode}")
                }),
            );
            assert_ne!(response["isError"], true, "{response}");
            let connected = &response["structuredContent"];
            assert_eq!(connected["ok"], true, "{connected}");
            let instance =
                plugin_lifecycle::require_instance(&service, &format!("alpaca-{mode}")).unwrap();
            assert_eq!(
                instance["configuration"]["tradingApiUrl"],
                format!("https://{mode}.example.test")
            );
            assert!(connected["connectUrl"]
                .as_str()
                .unwrap()
                .contains("/app/plugins-providers?instance=alpaca-"));
            let checked = verify(
                &service,
                json!({"instanceRef":format!("alpaca-{mode}"),"mode":mode}),
            );
            assert_ne!(checked["verified"], true, "{checked}");
        }
        assert!(crate::agent_runner::deployments(&service.runtime())
            .unwrap()
            .is_empty());
        drop(service);
        let restarted = TradeAssemblyService::test_local(db.to_string_lossy().to_string());
        let records = status(&restarted, json!({}));
        assert_eq!(records["instances"].as_array().unwrap().len(), 2);
        for instance in records["instances"].as_array().unwrap() {
            assert_eq!(instance["accountVerified"], false);
            assert_eq!(instance["configured"], false);
        }
        let rejected = connect(
            &restarted,
            json!({"instanceRef":"alpaca-paper","mode":"live"}),
        );
        assert_eq!(rejected["error"]["code"], "account_mode_mismatch");
    }

    #[test]
    fn durable_receipt_is_not_proof_after_configuration_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("receipt.db");
        let service = fixture_service(db.to_str().unwrap());
        assert_eq!(connect(&service, json!({"mode":"paper"}))["ok"], true);
        let instance = plugin_lifecycle::require_instance(&service, "alpaca-paper").unwrap();
        let record = receipt(
            &service,
            &instance,
            "alpaca-paper",
            "paper".into(),
            "mock-account",
        );
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("fixture-receipt").unwrap(),
        );
        service
            .runtime()
            .storage
            .put_json(RECEIPTS_NS, "mock-receipt", record.clone(), &context)
            .unwrap();
        drop(service);
        let restarted = TradeAssemblyService::test_local(db.to_string_lossy().to_string());
        assert_eq!(latest_receipt(&restarted, &instance), Some(record));
        assert_eq!(
            plugin_lifecycle::configure_instance(
                &restarted,
                "alpaca-paper",
                json!({"configuration":{"mode":"live"}})
            )
            .status,
            200
        );
        let result = status(&restarted, json!({"instanceRef":"alpaca-paper"}));
        assert_eq!(result["instances"][0]["verificationStale"], true);
        assert_eq!(result["instances"][0]["accountVerified"], false);
    }

    #[test]
    fn command_validation_requires_explicit_mode_and_envelope() {
        assert_eq!(
            validate_mcp_arguments("tradeassembly.broker.verify", &json!({})),
            Some("idempotency_key_required")
        );
        assert_eq!(
            validate_mcp_arguments(
                "tradeassembly.broker.verify",
                &json!({"idempotency_key":"k", "authority_context":{"actor":"test","surface":"mcp","account_mode":"paper"}, "instanceRef":"alpaca-paper"})
            ),
            Some("mode_required")
        );
    }

    #[test]
    fn requested_mode_conflict_is_rejected_before_actor_binding() {
        assert_eq!(
            validate_requested_mode_authority(&json!({
                "mode": "live",
                "authority_context": {"actor": "forged", "account_mode": "paper"}
            })),
            Some("authority_mode_mismatch")
        );
        assert_eq!(
            validate_requested_mode_authority(&json!({
                "mode": "live",
                "authorityContext": {"actor": "forged", "accountMode": "paper"}
            })),
            Some("authority_mode_mismatch")
        );
    }

    #[test]
    fn command_validation_rejects_credential_material_recursively() {
        assert_eq!(
            validate_mcp_arguments(
                "tradeassembly.broker.connect",
                &json!({"mode":"paper", "idempotency_key":"k", "authority_context":{"actor":"test","surface":"mcp","account_mode":"paper"}, "configuration":{"api_key":"redacted"}})
            ),
            Some("credentials_must_be_entered_in_plugin_ui")
        );
    }

    #[test]
    fn mcp_broker_calls_require_authenticated_session_and_ignore_forged_actor() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("authority.db");
        let base = fixture_service(db.to_str().unwrap());
        let rejected = base.call_mcp_tool(
            "tradeassembly.broker.connect",
            json!({"mode":"paper", "idempotency_key":"unauthenticated"}),
        );
        assert_eq!(
            rejected["structuredContent"]["error"]["code"],
            "authority_context_required"
        );

        let authenticated =
            base.for_authenticated_invocation("test-issuer", "verified-owner", None, None);
        let response = authenticated.call_mcp_tool(
            "tradeassembly.broker.connect",
            json!({
                "mode": "paper",
                "idempotency_key": "forged-actor",
                "authority_context": {"actor": "forged", "surface": "mcp", "account_mode": "paper"}
            }),
        );
        assert_eq!(response["structuredContent"]["ok"], true, "{response}");
        let commands = authenticated
            .runtime()
            .storage
            .list_json("control_plane_commands")
            .unwrap();
        let command = commands
            .into_iter()
            .map(|(_, value)| value)
            .find(|value| value["envelope"]["idempotency_key"] == "forged-actor")
            .expect("broker command record");
        assert_eq!(command["envelope"]["authority"]["actor"], "verified-owner");
        assert_ne!(command["envelope"]["authority"]["actor"], "forged");

        for (arguments, code) in [
            (json!({"idempotency_key":"missing-mode"}), "mode_required"),
            (json!({"mode":"paper"}), "idempotency_key_required"),
        ] {
            let rejected = authenticated.call_mcp_tool("tradeassembly.broker.connect", arguments);
            assert_eq!(rejected["structuredContent"]["error"]["code"], code);
        }
    }

    #[test]
    fn receipt_matching_marks_revision_and_package_changes_stale() {
        let instance = json!({"updatedAtMs": 2, "activePackageSha256":"pkg", "credentialRevision":1, "configuration": {"mode":"paper"}});
        let mut receipt = receipt_stub(&instance);
        assert!(receipt_matches(&instance, &receipt));
        receipt["activePackageSha256"] = json!("new-pkg");
        assert!(!receipt_matches(&instance, &receipt));
    }

    #[test]
    fn restart_observation_requires_distinct_prior_process_and_ordered_success() {
        let current = ProcessObservation {
            boot_id: "boot-new".to_string(),
            started_at_ms: 200,
        };
        let prior = json!({
            "processBootId": "boot-old",
            "processStartedAtMs": 100,
            "checkedAt": 150,
        });
        assert!(prior_observation_is_ordered(&prior, &current, 250));
        assert!(!prior_observation_is_ordered(&prior, &current, 150));
        assert!(!prior_observation_is_ordered(
            &json!({"processBootId":"boot-new", "processStartedAtMs":100, "checkedAt":150}),
            &current,
            250
        ));
        assert!(!prior_observation_is_ordered(
            &json!({"processBootId":"boot-old", "processStartedAtMs":250, "checkedAt":260}),
            &current,
            300
        ));
        assert!(!prior_observation_is_ordered(&prior, &current, 199));
        assert!(!prior_observation_is_ordered(
            &json!({"processBootId":"boot-old", "processStartedAtMs":100, "checkedAt":210}),
            &current,
            250
        ));
    }

    #[test]
    fn service_reconstruction_preserves_the_process_observation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("process.db");
        let first = TradeAssemblyService::test_local(path.to_string_lossy());
        let observation = process_observation().clone();
        assert!(observation.started_at_ms <= first.runtime().clock.now_ms());
        let second = TradeAssemblyService::test_local(path.to_string_lossy());
        assert_eq!(process_observation().boot_id, observation.boot_id);
        assert_eq!(
            process_observation().started_at_ms,
            observation.started_at_ms
        );
        assert!(observation.started_at_ms <= second.runtime().clock.now_ms());
    }

    #[test]
    fn restart_record_requires_bound_receipt_and_current_account_configuration() {
        let instance = json!({"instanceRef":"alpaca-paper", "updatedAtMs":2,
            "activePackageSha256":"package", "credentialRevision":1,
            "configuration":{"mode":"paper"},
            "health":{"account":{"id":"account", "mode":"paper"}}});
        let mut receipt = receipt_stub(&instance);
        receipt["instanceRef"] = json!("alpaca-paper");
        receipt["accountId"] = json!("account");
        receipt["mode"] = json!("paper");
        receipt["checkedAt"] = json!(250);
        let mut record = receipt.clone();
        record["authorityActor"] = json!("owner");
        record["verificationReceiptHash"] = json!(format!(
            "sha256:{:x}",
            Sha256::digest(serde_json_canonicalizer::to_vec(&receipt).unwrap())
        ));
        record["verificationReceipt"] = receipt;
        assert!(record_matches_instance(&instance, &record));
        for field in [
            "accountId",
            "mode",
            "credentialRevision",
            "activePackageSha256",
            "configurationSha256",
            "verificationReceiptHash",
            "authorityActor",
        ] {
            let mut invalid = record.clone();
            invalid[field] = Value::Null;
            assert!(!record_matches_instance(&instance, &invalid), "{field}");
        }
        let mut changed = instance.clone();
        changed["credentialRevision"] = json!(2);
        assert!(!record_matches_instance(&changed, &record));
        for (field, value) in [
            ("updatedAtMs", json!(3)),
            ("activePackageSha256", json!("different-package")),
            ("configuration", json!({"mode":"live"})),
            ("health", json!({"account":{"id":"other", "mode":"paper"}})),
        ] {
            let mut changed = instance.clone();
            changed[field] = value;
            assert!(!record_matches_instance(&changed, &record), "{field}");
        }
    }

    fn receipt_stub(instance: &Value) -> Value {
        let hash =
            Sha256::digest(serde_json_canonicalizer::to_vec(&instance["configuration"]).unwrap());
        json!({
            "instanceRevision": instance["updatedAtMs"],
            "activePackageSha256": instance["activePackageSha256"],
            "credentialRevision": instance["credentialRevision"],
            "configurationSha256": format!("sha256:{hash:x}"),
        })
    }
}
