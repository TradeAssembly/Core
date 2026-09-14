// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{api_result, providers, ServiceResponse, TradeAssemblyService};
use crate::live_execution_checks;
use crate::ports::PluginPackageInstallRequest;
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const REQUIRED_FORBIDDEN: &[&str] = &[
    "trade_advice",
    "raw_secret_output",
    "hosted_oauth_credentials",
    "billing",
    "payouts",
    "enterprise_sso_rbac",
];

pub(crate) fn list_manifests(service: &TradeAssemblyService) -> Value {
    json!({"manifests": manifests(service)})
}

pub(crate) fn get_manifest(service: &TradeAssemblyService, plugin_ref: &str) -> ServiceResponse {
    match manifest(service, plugin_ref) {
        Some(record) => ServiceResponse::ok(record),
        None => ServiceResponse::bad_request_with_details(
            "plugin_manifest_not_installed",
            json!({"pluginRef": plugin_ref}),
        ),
    }
}

pub(crate) fn install_manifest(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let Some(manifest) = body.get("manifest").cloned() else {
        return invalid("plugin_manifest_required", "manifest is required");
    };
    let source = body.get("source").cloned().unwrap_or(Value::Null);
    let expected_digest = body
        .get("integrity")
        .and_then(|value| value.get("manifestSha256"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let plugin_ref = match validate_manifest(service, &manifest, &source) {
        Ok(plugin_ref) => plugin_ref,
        Err(message) => return invalid("plugin_manifest_invalid", &message),
    };
    let digest = match manifest_digest(&manifest) {
        Ok(digest) => digest,
        Err(message) => return invalid("plugin_manifest_invalid", &message),
    };
    if expected_digest != digest {
        return invalid(
            "plugin_manifest_digest_mismatch",
            "manifest digest does not match the declared integrity value",
        );
    }
    if let Ok(Some(existing)) = service.runtime().plugins.get_manifest(&plugin_ref) {
        if existing["manifestDigest"] == digest {
            return ServiceResponse::ok(json!({
                "ok": true,
                "manifest": existing,
                "idempotent": true,
            }));
        }
        return invalid(
            "plugin_manifest_install_conflict",
            "a different manifest revision is already active; use package upgrade",
        );
    }
    let now = service.runtime().clock.now_ms();
    let record = json!({
        "pluginRef": plugin_ref,
        "manifest": manifest,
        "manifestDigest": digest,
        "source": source,
        "trust": body.get("trust").cloned().unwrap_or_else(|| json!({"level": "local-unverified"})),
        "installedAtMs": now,
    });
    let context = context("plugin.manifest.installed", &plugin_ref, now);
    match service
        .runtime()
        .plugins
        .install_manifest(&plugin_ref, record, &context)
    {
        Ok(record) => ServiceResponse::ok(json!({"ok": true, "manifest": record})),
        Err(message) => invalid("plugin_manifest_install_conflict", &message),
    }
}

pub(crate) fn install_package(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    let source = body.get("source").cloned().unwrap_or(Value::Null);
    let package = source
        .get("package")
        .cloned()
        .unwrap_or_else(|| source.clone());
    let integrity = body.get("integrity").cloned().unwrap_or(Value::Null);
    let request = PluginPackageInstallRequest {
        source_type: string(&package, "type"),
        locator: string(&package, "locator"),
        package_sha256: string(&integrity, "packageSha256"),
        manifest_sha256: string(&integrity, "manifestSha256"),
        offline: body
            .get("offline")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    let installed = match service.runtime().plugin_packages.install(&request) {
        Ok(installed) => installed,
        Err(message) => return invalid("plugin_package_install_failed", &message),
    };
    let plugin_ref = match validate_manifest(service, &installed.manifest, &source) {
        Ok(plugin_ref) => plugin_ref,
        Err(message) => return invalid("plugin_manifest_invalid", &message),
    };
    let digest = match manifest_digest(&installed.manifest) {
        Ok(digest) => digest,
        Err(message) => return invalid("plugin_manifest_invalid", &message),
    };
    let canonical_digest = digest.strip_prefix("sha256:").unwrap_or(&digest);
    if plugin_ref != installed.plugin_ref
        || canonical_digest != request.manifest_sha256
        || canonical_digest != installed.manifest_sha256
    {
        return invalid(
            "plugin_package_manifest_digest_mismatch",
            "package manifest identity or digest does not match the lock",
        );
    }
    if let Ok(Some(existing)) = service
        .runtime()
        .plugins
        .get_manifest_revision(&plugin_ref, &installed.manifest_sha256)
    {
        if existing["package"]["packageSha256"] == installed.package_sha256 {
            return ServiceResponse::ok(json!({
                "ok": true,
                "package": package_public_record(&installed),
                "manifest": existing,
                "idempotent": true,
            }));
        }
        return invalid(
            "plugin_package_install_conflict",
            "manifest revision is already bound to a different package",
        );
    }
    let now = service.runtime().clock.now_ms();
    let record = package_manifest_record(&installed, source, &body, now);
    let context = context("plugin.package.installed", &plugin_ref, now);
    match service
        .runtime()
        .plugins
        .install_manifest(&plugin_ref, record.clone(), &context)
    {
        Ok(_) => ServiceResponse::ok(json!({
            "ok": true,
            "package": package_public_record(&installed),
            "manifest": record,
        })),
        Err(message) => invalid("plugin_package_install_conflict", &message),
    }
}

pub(crate) fn list_instances(service: &TradeAssemblyService) -> Value {
    json!({"instances": instances(service)})
}

pub(crate) fn workspace(service: &TradeAssemblyService) -> Value {
    let manifests = manifests(service);
    let instances = instances(service);
    let plugins = instances
        .iter()
        .filter_map(|instance| {
            let plugin_ref = string(instance, "pluginRef");
            let manifest = manifests
                .iter()
                .find(|record| string(record, "pluginRef") == plugin_ref)?;
            let declaration = &manifest["manifest"];
            let capabilities = declaration["capabilities"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|capability| capability["id"].as_str().map(str::to_string))
                .collect::<Vec<_>>();
            Some(json!({
                "ref": plugin_ref,
                "instanceRef": instance["instanceRef"],
                "pluginRef": plugin_ref,
                "providerRef": instance["providerRef"],
                "accountRef": instance["accountRef"],
                "accountMode": instance["accountMode"],
                "name": declaration["metadata"]["name"],
                "version": declaration["metadata"]["version"],
                "enabled": instance["enabled"],
                "activePackageSha256": instance["activePackageSha256"],
                "previousPackageSha256": instance["previousPackageSha256"],
                "configuration": instance["configuration"],
                "configurationContract": declaration["configuration"],
                "credentialStatus": instance["credentialStatus"],
                "health": instance["health"],
                "capabilities": capabilities,
                "capabilityResolution": {
                    "profile": {"id": "local_full", "source": "local", "default": true},
                    "noSilentFallback": true,
                    "capabilityCount": capabilities.len(),
                },
                "manifest": {
                    "id": plugin_ref,
                    "name": declaration["metadata"]["name"],
                    "version": declaration["metadata"]["version"],
                    "provider": declaration["metadata"]["provider"],
                    "fingerprint": manifest["manifestDigest"],
                    "capabilities": capabilities,
                    "operations": declaration["operations"],
                    "permissions": declaration["permissions"],
                    "hostApis": declaration["hostApis"],
                    "trust": declaration["trust"],
                    "configuration": declaration["configuration"],
                    "health": declaration["health"],
                },
                "source": manifest["source"],
                "trust": manifest["trust"],
                "fingerprint": manifest["manifestDigest"],
            }))
        })
        .collect::<Vec<_>>();
    let credential_status = plugins
        .iter()
        .filter_map(|plugin| {
            Some((
                plugin["instanceRef"].as_str()?.to_string(),
                plugin["credentialStatus"].clone(),
            ))
        })
        .collect::<Map<_, _>>();
    let provider_health = plugins
        .iter()
        .filter_map(|plugin| {
            Some((
                plugin["instanceRef"].as_str()?.to_string(),
                plugin["health"].clone(),
            ))
        })
        .collect::<Map<_, _>>();
    json!({
        "workspacePrimitive": "plugins",
        "plugins": plugins,
        "providers": plugins,
        "credentialStatus": credential_status,
        "providerHealth": provider_health,
        "registry": {"manifests": manifests, "instances": instances},
        "manifestRegistry": {"manifests": manifests},
        "entitlements": providers::list_entitlements(service),
        "localCredentialBackends": [{"kind": "local-file", "scope": "instance", "summary": "customer-managed"}],
    })
}

pub(crate) fn compatibility_instance_ref(
    service: &TradeAssemblyService,
    requested_ref: &str,
) -> String {
    instances(service)
        .into_iter()
        .find(|instance| {
            string(instance, "instanceRef") == requested_ref
                || string(instance, "pluginRef") == requested_ref
                || string(instance, "providerRef") == requested_ref
        })
        .map(|instance| string(&instance, "instanceRef"))
        .unwrap_or_else(|| requested_ref.to_string())
}

pub(crate) fn active_compatibility_instance_ref(
    service: &TradeAssemblyService,
    requested_ref: &str,
) -> Result<String, ServiceResponse> {
    let instance_ref = compatibility_instance_ref(service, requested_ref);
    let record = require_instance(service, &instance_ref)?;
    if removed(&record) {
        return Err(invalid(
            "plugin_instance_removed",
            "plugin instance was removed",
        ));
    }
    Ok(instance_ref)
}

pub(crate) fn compatibility_instance_response(response: ServiceResponse) -> ServiceResponse {
    if response.status != 200 {
        return response;
    }
    let instance = &response.body["instance"];
    ServiceResponse::ok(api_result(json!({
        "plugin": instance["pluginRef"],
        "pluginRef": instance["pluginRef"],
        "provider": instance["providerRef"],
        "providerRef": instance["providerRef"],
        "updated": true,
        "enabled": instance["enabled"],
    })))
}

pub(crate) fn get_instance(service: &TradeAssemblyService, instance_ref: &str) -> ServiceResponse {
    match require_instance(service, instance_ref) {
        Ok(record) if record.get("removedAtMs").is_none_or(Value::is_null) => {
            ServiceResponse::ok(record)
        }
        Ok(_) => invalid("plugin_instance_not_found", "plugin instance was not found"),
        Err(response) => response,
    }
}

pub(crate) fn create_instance(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    if body
        .get("accountMode")
        .is_some_and(|mode| !matches!(mode.as_str(), Some("paper" | "live")))
    {
        return invalid(
            "plugin_account_mode_invalid",
            "Account mode must be paper or live.",
        );
    }
    let instance_ref = string(&body, "instanceRef");
    let plugin_ref = string(&body, "pluginRef");
    if !valid_ref(&instance_ref) || !valid_ref(&plugin_ref) {
        return invalid(
            "plugin_instance_ref_invalid",
            "instanceRef and pluginRef must be stable local identifiers",
        );
    }
    if manifest(service, &plugin_ref).is_none() {
        return invalid(
            "plugin_manifest_not_installed",
            "instance pluginRef does not identify an installed manifest",
        );
    }
    if instance(service, &instance_ref).is_some() {
        return invalid(
            "plugin_instance_exists",
            "instanceRef already identifies a plugin instance",
        );
    }
    let configuration = body
        .get("configuration")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Err(message) = validate_configuration(service, &plugin_ref, &configuration, true) {
        return invalid("plugin_configuration_invalid", &message);
    }
    let now = service.runtime().clock.now_ms();
    let record = json!({
        "instanceRef": instance_ref,
        "pluginRef": plugin_ref,
        "providerRef": body.get("providerRef").cloned().unwrap_or(Value::Null),
        "accountRef": body.get("accountRef").cloned().unwrap_or(Value::Null),
        "accountMode": body.get("accountMode").cloned().unwrap_or(Value::Null),
        "enabled": body.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        "configuration": configuration,
        "credentialRef": instance_ref,
        "activePackageSha256": manifest(service, &plugin_ref)
            .and_then(|record| record.get("package").cloned())
            .and_then(|package| package.get("packageSha256").cloned())
            .unwrap_or(Value::Null),
        "previousPackageSha256": Value::Null,
        "health": {"state": "needs_configuration", "checkedAtMs": Value::Null, "diagnostics": []},
        "installedAtMs": now,
        "updatedAtMs": now,
        "removedAtMs": Value::Null,
    });
    if let Err(message) = service.bind_owned_object("plugin_instance", &instance_ref) {
        return invalid("plugin_instance_authorization_failed", &message);
    }
    if let Err(message) = service.bind_inherited_object(
        "credential_reference",
        &instance_ref,
        "plugin_instance",
        &instance_ref,
    ) {
        return invalid("plugin_instance_authorization_failed", &message);
    }
    save_instance(service, &instance_ref, record)
}

pub(crate) fn upgrade_instance(
    service: &TradeAssemblyService,
    instance_ref: &str,
    body: Value,
) -> ServiceResponse {
    let original = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if removed(&original) {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    if let Some(response) = reject_if_in_use(service, instance_ref) {
        return response;
    }
    let installed_response = install_package(service, body);
    if installed_response.status != 200 {
        return installed_response;
    }
    let installed = &installed_response.body["package"];
    let plugin_ref = string(&original, "pluginRef");
    if installed["pluginRef"] != plugin_ref {
        return invalid(
            "plugin_package_identity_mismatch",
            "upgrade package does not match the instance plugin",
        );
    }
    let next_digest = string(installed, "packageSha256");
    let next_manifest_digest = string(installed, "manifestSha256");
    let current_digest = string(&original, "activePackageSha256");
    if next_digest == current_digest {
        return ServiceResponse::ok(json!({
            "ok": true,
            "instance": original,
            "idempotent": true,
        }));
    }
    let mut updated = original.clone();
    updated["previousPackageSha256"] = if current_digest.is_empty() {
        Value::Null
    } else {
        json!(current_digest)
    };
    updated["activePackageSha256"] = json!(next_digest);
    updated["updatedAtMs"] = json!(service.runtime().clock.now_ms());
    let saved = save_instance(service, instance_ref, updated.clone());
    if saved.status != 200 {
        return saved;
    }
    let activation_context = context(
        "plugin.package.activated",
        instance_ref,
        service.runtime().clock.now_ms(),
    );
    match service.runtime().plugins.activate_manifest_revision(
        &plugin_ref,
        &next_manifest_digest,
        &activation_context,
    ) {
        Ok(manifest) => ServiceResponse::ok(json!({
            "ok": true,
            "instance": updated,
            "manifest": manifest,
            "idempotent": false,
        })),
        Err(message) => {
            let _ = save_instance(service, instance_ref, original);
            invalid("plugin_package_activation_failed", &message)
        }
    }
}

pub(crate) fn rollback_instance(
    service: &TradeAssemblyService,
    instance_ref: &str,
) -> ServiceResponse {
    let original = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if removed(&original) {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    if let Some(response) = reject_if_in_use(service, instance_ref) {
        return response;
    }
    let previous_digest = string(&original, "previousPackageSha256");
    if previous_digest.is_empty() {
        return invalid(
            "plugin_package_rollback_unavailable",
            "no previous package is available",
        );
    }
    let previous = match service.runtime().plugin_packages.get(&previous_digest) {
        Ok(Some(package)) => package,
        Ok(None) => {
            return invalid(
                "plugin_package_rollback_unavailable",
                "previous package is not installed",
            )
        }
        Err(message) => return invalid("plugin_package_rollback_failed", &message),
    };
    if previous.plugin_ref != string(&original, "pluginRef") {
        return invalid(
            "plugin_package_identity_mismatch",
            "previous package does not match the instance plugin",
        );
    }
    let current_digest = string(&original, "activePackageSha256");
    let mut updated = original.clone();
    updated["activePackageSha256"] = json!(previous.package_sha256);
    updated["previousPackageSha256"] = if current_digest.is_empty() {
        Value::Null
    } else {
        json!(current_digest)
    };
    updated["updatedAtMs"] = json!(service.runtime().clock.now_ms());
    let saved = save_instance(service, instance_ref, updated.clone());
    if saved.status != 200 {
        return saved;
    }
    let activation_context = context(
        "plugin.package.rolled_back",
        instance_ref,
        service.runtime().clock.now_ms(),
    );
    match service.runtime().plugins.activate_manifest_revision(
        &previous.plugin_ref,
        &previous.manifest_sha256,
        &activation_context,
    ) {
        Ok(manifest) => ServiceResponse::ok(json!({
            "ok": true,
            "instance": updated,
            "manifest": manifest,
        })),
        Err(message) => {
            let _ = save_instance(service, instance_ref, original);
            invalid("plugin_package_rollback_failed", &message)
        }
    }
}

pub(crate) fn configure_instance(
    service: &TradeAssemblyService,
    instance_ref: &str,
    body: Value,
) -> ServiceResponse {
    let mut record = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if record
        .get("removedAtMs")
        .is_some_and(|value| !value.is_null())
    {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    let plugin_ref = string(&record, "pluginRef");
    let patch = body
        .get("configuration")
        .cloned()
        .unwrap_or_else(|| body.clone());
    let Some(patch) = patch.as_object() else {
        return invalid(
            "plugin_configuration_invalid",
            "configuration must be an object",
        );
    };
    let mut merged = record
        .get("configuration")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    merged.extend(patch.clone());
    let configuration = Value::Object(merged);
    if let Err(message) = validate_configuration(service, &plugin_ref, &configuration, false) {
        return invalid("plugin_configuration_invalid", &message);
    }
    record["configuration"] = configuration;
    if let Some(account_ref) = body.get("accountRef") {
        if !account_ref.is_null()
            && account_ref
                .as_str()
                .is_none_or(|value| !valid_account_ref(value))
        {
            return invalid(
                "plugin_account_ref_invalid",
                "accountRef must be an account:// URI without whitespace",
            );
        }
        record["accountRef"] = account_ref.clone();
    }
    record["updatedAtMs"] = json!(service.runtime().clock.now_ms());
    save_instance(service, instance_ref, record)
}

fn valid_account_ref(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("account://")
        && value.len() > "account://".len()
        && !value.chars().any(char::is_whitespace)
}

pub(crate) fn set_enabled(
    service: &TradeAssemblyService,
    instance_ref: &str,
    enabled: bool,
) -> ServiceResponse {
    let mut record = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if record
        .get("removedAtMs")
        .is_some_and(|value| !value.is_null())
    {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    if !enabled {
        if let Some(response) = reject_if_in_use(service, instance_ref) {
            return response;
        }
    }
    record["enabled"] = json!(enabled);
    record["updatedAtMs"] = json!(service.runtime().clock.now_ms());
    save_instance(service, instance_ref, record)
}

pub(crate) fn remove_instance(
    service: &TradeAssemblyService,
    instance_ref: &str,
) -> ServiceResponse {
    let mut record = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if removed(&record) {
        return ServiceResponse::ok(json!({
            "ok": true,
            "instance": with_live_status(service, record),
            "idempotent": true,
        }));
    }
    if let Some(response) = reject_if_in_use(service, instance_ref) {
        return response;
    }
    if record["enabled"].as_bool().unwrap_or(false) {
        return invalid(
            "plugin_instance_must_be_disabled",
            "disable this plugin instance before removal",
        );
    }
    if service
        .runtime()
        .credentials
        .status(instance_ref)
        .configured
    {
        return invalid(
            "plugin_credentials_must_be_revoked",
            "revoke this instance credential before removal",
        );
    }
    let now = service.runtime().clock.now_ms();
    record["enabled"] = json!(false);
    record["removedAtMs"] = json!(now);
    record["updatedAtMs"] = json!(now);
    save_instance(service, instance_ref, record)
}

pub(crate) fn store_credentials(
    service: &TradeAssemblyService,
    instance_ref: &str,
    body: Value,
) -> ServiceResponse {
    let mut record = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if removed(&record) {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    if let Err(response) = service.require_object("credential_reference", instance_ref) {
        return response;
    }
    let plugin_ref = string(&record, "pluginRef");
    let values = body
        .get("credentials")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            body.as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "instanceRef" | "pluginRef" | "providerRef" | "paper" | "mode"
                    )
                })
                .collect()
        });
    let fields = match credential_fields(service, &plugin_ref, &values) {
        Ok(fields) => fields,
        Err(message) => return invalid("plugin_credentials_invalid", &message),
    };
    if let Some(manifest) = manifest(service, &plugin_ref) {
        // CredentialPort::store replaces the complete map. Validate exactly
        // what will be persisted; merging with the old map can hide a mixed
        // or partial replacement.
        if let Err(message) = validate_credential_requirements(&manifest, &fields) {
            return invalid("plugin_credentials_invalid", &message);
        }
    }
    match service.runtime().credentials.store(instance_ref, &fields) {
        Ok(status) => {
            let revision = next_credential_revision(&record);
            record["credentialRevision"] = json!(revision);
            record["credentialStatus"] = credential_status(&status, revision);
            record["updatedAtMs"] = json!(service.runtime().clock.now_ms());
            let saved = save_instance(service, instance_ref, record);
            if saved.status != 200 {
                return saved;
            }
            ServiceResponse::ok(json!({
                "ok": true,
                "instanceRef": instance_ref,
                "credentialStatus": credential_status(&status, revision),
            }))
        }
        Err(message) => invalid("plugin_credentials_store_failed", &message),
    }
}

pub(crate) struct CompatibilityFields {
    pub configuration: Map<String, Value>,
    pub credentials: Map<String, Value>,
}

pub(crate) fn partition_compatibility_fields(
    service: &TradeAssemblyService,
    instance_ref: &str,
    body: &Map<String, Value>,
) -> Result<CompatibilityFields, String> {
    let record = instance(service, instance_ref)
        .ok_or_else(|| "plugin instance was not found".to_string())?;
    if removed(&record) {
        return Err("plugin instance was removed".to_string());
    }
    let plugin_ref = string(&record, "pluginRef");
    let manifest = manifest(service, &plugin_ref)
        .ok_or_else(|| "plugin manifest is not installed".to_string())?;
    let declarations = manifest["manifest"]["configuration"]["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|field| {
            Some((
                field["id"].as_str()?.to_string(),
                field["storageClass"].as_str()?.to_string(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let mut configuration = body
        .get("configuration")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut credentials = body
        .get("credentials")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (key, value) in body {
        match declarations.get(key).map(String::as_str) {
            Some("configuration") => {
                configuration.insert(key.clone(), value.clone());
            }
            Some("credential") => {
                credentials.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    Ok(CompatibilityFields {
        configuration,
        credentials,
    })
}

pub(crate) fn credential_status_for(
    service: &TradeAssemblyService,
    instance_ref: &str,
) -> ServiceResponse {
    let record = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if removed(&record) {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    let status = service.runtime().credentials.status(instance_ref);
    ServiceResponse::ok(json!({
        "instanceRef": instance_ref,
        "credentialStatus": credential_status(&status, credential_revision(&record)),
    }))
}

pub(crate) fn revoke_credentials(
    service: &TradeAssemblyService,
    instance_ref: &str,
) -> ServiceResponse {
    let mut record = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if removed(&record) {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    if let Err(response) = service.require_object("credential_reference", instance_ref) {
        return response;
    }
    match service.runtime().credentials.revoke(instance_ref) {
        Ok(removed) => {
            let revision = next_credential_revision(&record);
            record["credentialRevision"] = json!(revision);
            record["credentialStatus"] = credential_status(
                &service.runtime().credentials.status(instance_ref),
                revision,
            );
            record["updatedAtMs"] = json!(service.runtime().clock.now_ms());
            let saved = save_instance(service, instance_ref, record);
            if saved.status != 200 {
                return saved;
            }
            ServiceResponse::ok(
                json!({"ok": true, "instanceRef": instance_ref, "revoked": removed}),
            )
        }
        Err(message) => invalid("plugin_credentials_revoke_failed", &message),
    }
}

pub(crate) fn refresh_health(
    service: &TradeAssemblyService,
    instance_ref: &str,
) -> ServiceResponse {
    let mut record = match require_instance(service, instance_ref) {
        Ok(record) => record,
        Err(response) => return response,
    };
    if removed(&record) {
        return invalid("plugin_instance_removed", "plugin instance was removed");
    }
    let checked_at = service.runtime().clock.now_ms();
    let health_revision = record["health"]["revision"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(1);
    let mut state = health_state(service, &record);
    let mut connectivity_checked = false;
    let mut account = Value::Null;
    let mut diagnostics = match state.as_str() {
        "host_unavailable" => {
            vec!["The installed plugin executable is unavailable or invalid.".to_string()]
        }
        "needs_configuration" => {
            vec!["Required non-secret configuration is missing.".to_string()]
        }
        "needs_credentials" => {
            vec!["Required write-only credentials are missing.".to_string()]
        }
        "disabled" => vec!["Plugin instance is disabled.".to_string()],
        _ => Vec::new(),
    };
    let plugin_ref = string(&record, "pluginRef");
    let health_operation_declared = manifest(service, &plugin_ref).is_some_and(|manifest| {
        manifest["manifest"]["operations"]
            .as_array()
            .is_some_and(|operations| {
                operations
                    .iter()
                    .any(|operation| operation["id"] == "account.health")
            })
    });
    if state == "ready" && health_operation_declared {
        let mode = record["configuration"]["mode"]
            .as_str()
            .or_else(|| record["configuration"]["account_mode"].as_str())
            .or_else(|| record["accountMode"].as_str())
            .unwrap_or("paper");
        let response = providers::invoke_health_operation(
            service,
            &format!("/plugins/instances/{instance_ref}/operations/account.health:invoke"),
            json!({
                "mode": mode,
                "purpose": "account_health",
                "accountRef": record.get("accountRef").cloned().unwrap_or(Value::Null),
                "idempotencyKey": format!("plugin.health:{instance_ref}:{health_revision}"),
                "input": {},
            }),
        );
        connectivity_checked = true;
        if response.status == 200 {
            let payload = response.body["response"]["payload"].clone();
            account = payload
                .get("account")
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or(payload);
            if let Some(account_id) = account["id"].as_str().filter(|value| !value.is_empty()) {
                record["accountRef"] = json!(format!("account://{instance_ref}/{account_id}"));
            }
            if let Some(account_mode) = account["mode"]
                .as_str()
                .filter(|value| matches!(*value, "paper" | "live"))
            {
                record["accountMode"] = json!(account_mode);
            }
        } else {
            state = "degraded".to_string();
            diagnostics.push(
                response.body["error"]["details"]["reason"]
                    .as_str()
                    .or_else(|| response.body["error"]["code"].as_str())
                    .unwrap_or("plugin_health_check_failed")
                    .to_string(),
            );
        }
    }
    let binding = health_evidence_binding(&record);
    record["health"] = json!({
        "state": state,
        "checkedAtMs": checked_at,
        "revision": health_revision,
        "diagnostics": diagnostics,
        "connectivityChecked": connectivity_checked,
        "account": account,
        "binding": binding,
    });
    // Observations have their own revision. Changing the lifecycle revision here
    // would invalidate otherwise-current receipts on every health read.
    save_instance(service, instance_ref, record)
}

fn manifests(service: &TradeAssemblyService) -> Vec<Value> {
    let mut records = builtin_manifests(service);
    if let Ok(stored) = service.runtime().plugins.list_manifests() {
        for record in stored {
            let plugin_ref = string(&record, "pluginRef");
            records.retain(|item| string(item, "pluginRef") != plugin_ref);
            records.push(record);
        }
    }
    records.sort_by_key(|record| string(record, "pluginRef"));
    records
}

fn package_manifest_record(
    installed: &crate::ports::InstalledPluginPackage,
    source: Value,
    body: &Value,
    installed_at_ms: i64,
) -> Value {
    json!({
        "pluginRef": installed.plugin_ref,
        "manifest": installed.manifest,
        "manifestDigest": installed.manifest_sha256,
        "source": source,
        "package": package_public_record(installed),
        "trust": body.get("trust").cloned().unwrap_or_else(|| json!({"level": "external-locked"})),
        "installedAtMs": installed_at_ms,
    })
}

fn package_public_record(installed: &crate::ports::InstalledPluginPackage) -> Value {
    json!({
        "pluginRef": installed.plugin_ref,
        "version": installed.version,
        "packageSha256": installed.package_sha256,
        "manifestSha256": installed.manifest_sha256,
        "hostContract": installed.host_contract,
        "sdkVersion": installed.sdk_version,
        "target": tradeassembly_plugin_sdk::host_target(),
    })
}

fn manifest(service: &TradeAssemblyService, plugin_ref: &str) -> Option<Value> {
    service
        .runtime()
        .plugins
        .get_manifest(plugin_ref)
        .ok()
        .flatten()
        .or_else(|| {
            builtin_manifests(service)
                .into_iter()
                .find(|record| string(record, "pluginRef") == plugin_ref)
        })
}

fn instances(service: &TradeAssemblyService) -> Vec<Value> {
    let mut records = Vec::new();
    for default in builtin_instances(service) {
        let instance_ref = string(&default, "instanceRef");
        match service.runtime().plugins.get_instance(&instance_ref) {
            Ok(Some(stored)) if stored.get("removedAtMs").is_none_or(Value::is_null) => {
                records.push(with_live_status(service, stored));
            }
            Ok(Some(_)) => {}
            _ => records.push(with_live_status(service, default)),
        }
    }
    if let Ok(stored) = service.runtime().plugins.list_instances() {
        for record in stored {
            let instance_ref = string(&record, "instanceRef");
            if records
                .iter()
                .all(|item| string(item, "instanceRef") != instance_ref)
            {
                records.push(with_live_status(service, record));
            }
        }
    }
    records.sort_by_key(|record| string(record, "instanceRef"));
    service.filter_visible_values("plugin_instance", records, &["instanceRef"])
}

pub(crate) fn require_instance(
    service: &TradeAssemblyService,
    instance_ref: &str,
) -> Result<Value, ServiceResponse> {
    let record = instance(service, instance_ref).ok_or_else(|| {
        ServiceResponse::bad_request_with_details(
            "plugin_instance_not_found",
            json!({"instanceRef": instance_ref}),
        )
    })?;
    service.require_object("plugin_instance", instance_ref)?;
    Ok(record)
}

fn instance(service: &TradeAssemblyService, instance_ref: &str) -> Option<Value> {
    service
        .runtime()
        .plugins
        .get_instance(instance_ref)
        .ok()
        .flatten()
        .or_else(|| {
            builtin_instances(service)
                .into_iter()
                .find(|record| string(record, "instanceRef") == instance_ref)
        })
        .map(|record| with_live_status(service, record))
}

fn builtin_manifests(service: &TradeAssemblyService) -> Vec<Value> {
    providers::builtin_plugin_catalog(service)
        .into_iter()
        .map(|plugin| built_in_manifest_record(&plugin))
        .collect()
}

pub(crate) fn built_in_manifest_digest(plugin: &crate::ports::PluginCatalogEntry) -> String {
    built_in_manifest_record(plugin)["manifestDigest"]
        .as_str()
        .expect("built-in manifest digest")
        .to_string()
}

fn built_in_manifest_record(plugin: &crate::ports::PluginCatalogEntry) -> Value {
    let configuration = configuration_contract();
    let manifest = json!({
        "apiVersion": "tradeassembly.org/v1alpha1",
        "kind": "TradeAssemblyPluginManifest",
        "manifestVersion": "0.1",
        "metadata": {
            "id": plugin.plugin_ref,
            "name": plugin.name,
            "version": "0.1.0",
            "provider": {"id": plugin.provider_ref, "name": plugin.provider_ref},
        },
        "runtime": {
            "protocol": if plugin.trust_level == "core" { "rust-native" } else { "stdio" },
            "entrypoint": if plugin.trust_level == "core" { "tradeassembly-core" } else { "bin/tradeassembly-plugin" },
            "timeoutSeconds": 15,
        },
        "capabilities": plugin.capabilities.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
        "permissions": [{"id": "local.state.read"}],
        "hostApis": [{"id": "tradeassembly.journal.append"}],
        "operations": plugin.operations.iter().map(manifest_operation_value).collect::<Vec<_>>(),
        "configuration": configuration,
        "health": health_contract(),
        "trust": {"sandbox": if plugin.trust_level == "core" { "in-process-core" } else { "external-process" }, "level": plugin.trust_level},
        "forbidden": REQUIRED_FORBIDDEN,
    });
    let digest = manifest_digest(&manifest).expect("built-in manifest is canonical JSON");
    json!({
        "pluginRef": plugin.plugin_ref,
        "manifest": manifest,
        "manifestDigest": digest,
        "source": {"type": "core", "locator": format!("core://plugins/{}", plugin.plugin_ref)},
        "trust": {"level": plugin.trust_level},
        "installedAtMs": 0,
    })
}

fn manifest_operation_value(operation: &crate::ports::PluginOperationContract) -> Value {
    json!({
        "id": operation.id,
        "capability": operation.capability,
        "protocol": operation.protocol,
        "effect": operation.effect,
        "riskClass": operation.risk,
        "resourceType": operation.resource_type,
        "credentialGrantRequired": operation.credential_grant_required,
        "accountBindingRequired": operation.account_binding_required,
        "financeAction": {
            "actionId": operation.apf_action_id,
            "resourceType": operation.finance_resource_type,
        },
        "mandateRequired": operation.mandate_required,
        "purpose": operation.purpose,
        "sessionAdmission": {"mode": operation.session_admission},
        "approval": {"mode": operation.approval_mode},
        "supervision": {"mode": operation.supervision_mode},
        "detectors": operation.detectors,
        "contextProviders": operation.context_providers,
        "policyRefs": operation.policy_refs,
        "traits": operation.traits,
        "dependencies": operation.dependencies,
        "evidence": operation.evidence,
        "pepCoverage": operation.pep_coverage_class,
        "checkPacks": operation.check_packs,
        "receiptClass": operation.receipt_class,
        "redaction": operation.redaction,
        "noAdvice": operation.no_advice,
        "supportedProtocols": operation.supported_protocols,
        "inputSchema": "json-schema",
        "outputSchema": "json-schema",
    })
}

fn builtin_instances(service: &TradeAssemblyService) -> Vec<Value> {
    providers::plugin_catalog(service)
        .into_iter()
        .map(|plugin| {
            let instance_ref = plugin.provider_ref.clone();
            json!({
                "instanceRef": instance_ref,
                "pluginRef": plugin.plugin_ref,
                "providerRef": plugin.provider_ref,
                "accountRef": Value::Null,
                "enabled": plugin.enabled,
                "configuration": default_configuration(),
                "credentialRef": instance_ref,
                "credentialStatus": plugin.credential_status,
                "health": {"state": plugin.health, "checkedAtMs": Value::Null, "diagnostics": []},
                "installedAtMs": 0,
                "updatedAtMs": 0,
                "removedAtMs": Value::Null,
            })
        })
        .collect()
}

fn configuration_contract() -> Value {
    json!({"fields": []})
}

fn health_contract() -> Value {
    json!({"requiredConfiguration": [], "requiredCredentials": [], "connectionCheckOperation": Value::Null})
}

fn default_configuration() -> Value {
    let fields = configuration_contract()["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Value::Object(
        fields
            .into_iter()
            .filter(|field| field["storageClass"] == "configuration")
            .filter_map(|field| {
                Some((
                    field["id"].as_str()?.to_string(),
                    field.get("default")?.clone(),
                ))
            })
            .collect(),
    )
}

fn validate_manifest(
    service: &TradeAssemblyService,
    manifest: &Value,
    source: &Value,
) -> Result<String, String> {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../plugin-contracts/schemas/plugin-manifest.schema.json"
    ))
    .map_err(|error| format!("plugin manifest schema is invalid: {error}"))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| format!("plugin manifest schema could not compile: {error}"))?;
    if let Err(error) = validator.validate(manifest) {
        return Err(format!(
            "manifest does not match the canonical schema: {error}"
        ));
    }
    let object = manifest
        .as_object()
        .ok_or_else(|| "manifest must be an object".to_string())?;
    let allowed = [
        "apiVersion",
        "kind",
        "manifestVersion",
        "metadata",
        "runtime",
        "capabilities",
        "permissions",
        "hostApis",
        "operations",
        "configuration",
        "health",
        "trust",
        "forbidden",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if let Some(field) = object.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(format!("unknown critical manifest field: {field}"));
    }
    if manifest["apiVersion"] != "tradeassembly.org/v1alpha1"
        || manifest["kind"] != "TradeAssemblyPluginManifest"
        || manifest["manifestVersion"] != "0.1"
    {
        return Err("manifest contract version is unsupported".to_string());
    }
    let plugin_ref = manifest["metadata"]["id"]
        .as_str()
        .ok_or_else(|| "metadata.id is required".to_string())?;
    if !valid_ref(plugin_ref) {
        return Err("metadata.id is invalid".to_string());
    }
    for field in ["name", "version"] {
        if manifest["metadata"][field]
            .as_str()
            .is_none_or(str::is_empty)
        {
            return Err(format!("metadata.{field} is required"));
        }
    }
    if manifest["metadata"]["provider"]["id"]
        .as_str()
        .is_none_or(str::is_empty)
    {
        return Err("metadata.provider.id is required".to_string());
    }
    let protocol = manifest["runtime"]["protocol"].as_str().unwrap_or_default();
    if !matches!(protocol, "stdio" | "grpc" | "rest" | "rust-native") {
        return Err("runtime.protocol is invalid".to_string());
    }
    let entrypoint = manifest["runtime"]["entrypoint"]
        .as_str()
        .unwrap_or_default();
    if entrypoint.is_empty() || entrypoint.starts_with('/') || entrypoint.contains("..") {
        return Err("runtime.entrypoint is unsafe".to_string());
    }
    let forbidden = manifest["forbidden"]
        .as_array()
        .ok_or_else(|| "forbidden declarations are required".to_string())?;
    for required in REQUIRED_FORBIDDEN {
        if !forbidden.iter().any(|item| item == required) {
            return Err(format!("missing forbidden declaration: {required}"));
        }
    }
    let capabilities = declaration_ids(&manifest["capabilities"], "capability")?;
    let operations = manifest["operations"]
        .as_array()
        .ok_or_else(|| "operations are required".to_string())?;
    if operations.is_empty() {
        return Err("operations are required".to_string());
    }
    let mut operation_ids = BTreeSet::new();
    for operation in operations {
        let operation_id = operation["id"]
            .as_str()
            .filter(|id| valid_ref(id))
            .ok_or_else(|| "operation id is invalid".to_string())?;
        if !operation_ids.insert(operation_id.to_string()) {
            return Err(format!("duplicate operation id: {operation_id}"));
        }
        let capability = operation["capability"]
            .as_str()
            .ok_or_else(|| "operation capability is required".to_string())?;
        if !capabilities.contains(capability) {
            return Err(format!(
                "operation references undeclared capability: {capability}"
            ));
        }
        if operation["noAdvice"] != true {
            return Err("every operation must set noAdvice: true".to_string());
        }
        validate_operation_dependencies(operation_id, capability, operation)?;
    }
    validate_configuration_contract(&manifest["configuration"])?;
    validate_source(service, source, plugin_ref, manifest)?;
    Ok(plugin_ref.to_string())
}

fn declaration_ids(value: &Value, label: &str) -> Result<BTreeSet<String>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{label} declarations are required"))?;
    if items.is_empty() {
        return Err(format!("{label} declarations are required"));
    }
    let mut ids = BTreeSet::new();
    for item in items {
        let id = item["id"]
            .as_str()
            .filter(|id| label != "capability" || valid_capability_ref(id))
            .filter(|id| label == "capability" || valid_ref(id))
            .ok_or_else(|| format!("{label} id is invalid"))?;
        if !ids.insert(id.to_string()) {
            return Err(format!("duplicate {label} id: {id}"));
        }
    }
    Ok(ids)
}

fn validate_operation_dependencies(
    operation_id: &str,
    operation_capability: &str,
    operation: &Value,
) -> Result<(), String> {
    let dependencies = operation["dependencies"]
        .as_array()
        .ok_or_else(|| format!("operation {operation_id} dependencies are required"))?;
    let mut dependency_ids = BTreeSet::new();
    for dependency in dependencies {
        let dependency_id = dependency["dependencyId"]
            .as_str()
            .filter(|id| valid_ref(id))
            .ok_or_else(|| format!("operation {operation_id} dependency id is invalid"))?;
        if !dependency_ids.insert(dependency_id) {
            return Err(format!(
                "operation {operation_id} has duplicate dependency id: {dependency_id}"
            ));
        }
        let capability = dependency["capability"]
            .as_str()
            .filter(|capability| valid_capability_ref(capability))
            .ok_or_else(|| {
                format!("operation {operation_id} dependency {dependency_id} capability is invalid")
            })?;
        let dependency_operation = dependency["operationId"].as_str();
        if dependency_operation.is_some_and(|id| !valid_ref(id)) {
            return Err(format!(
                "operation {operation_id} dependency {dependency_id} operationId is invalid"
            ));
        }
        if dependency_operation == Some(operation_id)
            || (dependency_operation.is_none() && capability == operation_capability)
        {
            return Err(format!(
                "operation {operation_id} dependency {dependency_id} is a self-dependency"
            ));
        }
    }
    Ok(())
}

fn validate_configuration_contract(value: &Value) -> Result<(), String> {
    let fields = value["fields"]
        .as_array()
        .ok_or_else(|| "configuration.fields is required".to_string())?;
    let mut ids = BTreeSet::new();
    for field in fields {
        let id = field["id"]
            .as_str()
            .filter(|id| valid_ref(id))
            .ok_or_else(|| "configuration field id is invalid".to_string())?;
        if !ids.insert(id) {
            return Err(format!("duplicate configuration field: {id}"));
        }
        let storage = field["storageClass"].as_str().unwrap_or_default();
        if !matches!(storage, "configuration" | "credential") {
            return Err(format!("configuration field {id} has invalid storageClass"));
        }
        let input = field["inputType"].as_str().unwrap_or_default();
        if !matches!(
            input,
            "text" | "password" | "url" | "number" | "boolean" | "select"
        ) {
            return Err(format!("configuration field {id} has invalid inputType"));
        }
        if storage == "credential" && input != "password" {
            return Err(format!("credential field {id} must use password input"));
        }
    }
    Ok(())
}

fn validate_source(
    service: &TradeAssemblyService,
    source: &Value,
    plugin_ref: &str,
    candidate_manifest: &Value,
) -> Result<(), String> {
    let source_type = source["type"]
        .as_str()
        .ok_or_else(|| "source.type is required".to_string())?;
    let locator = source["locator"]
        .as_str()
        .ok_or_else(|| "source.locator is required".to_string())?;
    match source_type {
        "core" if locator == format!("core://plugins/{plugin_ref}") => {
            let registered = builtin_manifests(service).into_iter().any(|record| {
                string(&record, "pluginRef") == plugin_ref
                    && record["manifest"] == *candidate_manifest
            });
            if registered {
                Ok(())
            } else {
                Err(
                    "core provenance is reserved for exact registered built-in manifests"
                        .to_string(),
                )
            }
        }
        "git" => {
            let commit = source["commit"].as_str().unwrap_or_default();
            let immutable = commit.len() == 40
                && commit
                    .chars()
                    .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
                && locator.starts_with("git+https://")
                && locator.contains(&format!("?ref={commit}#"))
                && !locator.contains("..")
                && !locator.ends_with('#');
            if immutable {
                Ok(())
            } else {
                Err("git source must use an immutable lowercase commit locator".to_string())
            }
        }
        "package" => {
            let package = source.get("package").unwrap_or(source);
            let package_type = package["type"].as_str().unwrap_or_default();
            let package_locator = package["locator"].as_str().unwrap_or_default();
            let locator_matches_type = match package_type {
                "file" => !package_locator.contains("://"),
                "https" => package_locator.starts_with("https://"),
                _ => false,
            };
            if locator_matches_type
                && !package_locator.trim().is_empty()
                && locator == package_locator
            {
                Ok(())
            } else {
                Err("package source must declare its locked file or HTTPS locator".to_string())
            }
        }
        _ => Err("source provenance is invalid or mutable".to_string()),
    }
}

fn validate_configuration(
    service: &TradeAssemblyService,
    plugin_ref: &str,
    configuration: &Value,
    allow_missing_required: bool,
) -> Result<(), String> {
    let manifest = manifest(service, plugin_ref)
        .ok_or_else(|| "plugin manifest is not installed".to_string())?;
    let fields = manifest["manifest"]["configuration"]["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let object = configuration
        .as_object()
        .ok_or_else(|| "configuration must be an object".to_string())?;
    let declarations = fields
        .iter()
        .filter_map(|field| Some((field["id"].as_str()?.to_string(), field)))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in object {
        let field = declarations
            .get(key)
            .ok_or_else(|| format!("unknown configuration field: {key}"))?;
        if field["storageClass"] == "credential" {
            return Err(format!(
                "credential field {key} cannot be stored as configuration"
            ));
        }
        validate_field_value(field, value)?;
    }
    if !allow_missing_required {
        for (id, field) in declarations {
            if field["storageClass"] == "configuration"
                && field["required"] == true
                && !object.contains_key(&id)
                && field.get("default").is_none()
            {
                return Err(format!("required configuration field is missing: {id}"));
            }
        }
    }
    Ok(())
}

fn credential_fields(
    service: &TradeAssemblyService,
    plugin_ref: &str,
    values: &Map<String, Value>,
) -> Result<BTreeMap<String, String>, String> {
    let manifest = manifest(service, plugin_ref)
        .ok_or_else(|| "plugin manifest is not installed".to_string())?;
    let fields = manifest["manifest"]["configuration"]["fields"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let declarations = fields
        .iter()
        .filter(|field| field["storageClass"] == "credential")
        .filter_map(|field| Some((field["id"].as_str()?.to_string(), field)))
        .collect::<BTreeMap<_, _>>();
    let has_alternatives = manifest["manifest"]["health"]["credentialAlternatives"]
        .as_array()
        .is_some_and(|alternatives| !alternatives.is_empty());
    if declarations.is_empty() {
        return Err("plugin does not declare credential fields".to_string());
    }
    for key in values.keys() {
        if !declarations.contains_key(key) {
            return Err(format!("unknown credential field: {key}"));
        }
    }
    let mut result = BTreeMap::new();
    for (id, field) in declarations {
        let value = values.get(&id).and_then(Value::as_str).unwrap_or_default();
        if !has_alternatives && field["required"] == true && value.trim().is_empty() {
            return Err(format!("required credential field is missing: {id}"));
        }
        if !value.is_empty() {
            result.insert(id, value.to_string());
        }
    }
    Ok(result)
}

fn validate_credential_requirements(
    manifest_record: &Value,
    fields: &BTreeMap<String, String>,
) -> Result<(), String> {
    let manifest = &manifest_record["manifest"];
    if let Some(alternatives) = manifest["health"]["credentialAlternatives"]
        .as_array()
        .filter(|items| !items.is_empty())
    {
        let active = alternatives
            .iter()
            .filter_map(Value::as_array)
            .filter(|alternative| {
                alternative.iter().any(|field| {
                    field.as_str().is_some_and(|id| {
                        fields.get(id).is_some_and(|value| !value.trim().is_empty())
                    })
                })
            })
            .collect::<Vec<_>>();
        if active.len() != 1 {
            return Err("credentials must complete exactly one credential alternative".to_string());
        }
        if active[0].iter().any(|field| {
            field
                .as_str()
                .is_none_or(|id| fields.get(id).is_none_or(|value| value.trim().is_empty()))
        }) {
            return Err("credential alternative is partially configured".to_string());
        }
        return Ok(());
    }
    let required = manifest["health"]["requiredCredentials"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    if required
        .clone()
        .any(|id| fields.get(id).is_none_or(|value| value.trim().is_empty()))
    {
        return Err("required credential field is missing".to_string());
    }
    Ok(())
}

fn validate_field_value(field: &Value, value: &Value) -> Result<(), String> {
    let id = field["id"].as_str().unwrap_or("field");
    let valid = match field["inputType"].as_str().unwrap_or_default() {
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "select" => value.as_str().is_some_and(|candidate| {
            field["options"]
                .as_array()
                .is_some_and(|options| options.iter().any(|option| option == candidate))
        }),
        "text" | "password" | "url" => value.is_string(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("configuration field {id} has an invalid value"))
    }
}

pub(crate) fn health_state(service: &TradeAssemblyService, record: &Value) -> String {
    if !record["enabled"].as_bool().unwrap_or(false) {
        return "disabled".to_string();
    }
    let plugin_ref = string(record, "pluginRef");
    let Some(manifest) = manifest(service, &plugin_ref) else {
        return "degraded".to_string();
    };
    let config = record["configuration"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let missing_config = manifest["manifest"]["health"]["requiredConfiguration"]
        .as_array()
        .is_some_and(|required| {
            required.iter().filter_map(Value::as_str).any(|key| {
                config
                    .get(key)
                    .is_none_or(|value| value.is_null() || value == "")
            })
        });
    if missing_config {
        return "needs_configuration".to_string();
    }
    let requires_credentials = manifest["manifest"]["health"]["requiredCredentials"]
        .as_array()
        .is_some_and(|required| !required.is_empty());
    let credential_alternatives = manifest["manifest"]["health"]["credentialAlternatives"]
        .as_array()
        .is_some_and(|alternatives| !alternatives.is_empty());
    if requires_credentials || credential_alternatives {
        let resolved = service
            .runtime()
            .credentials
            .resolve_fields(&string(record, "credentialRef"))
            .ok()
            .flatten()
            .unwrap_or_default();
        if validate_credential_requirements(&manifest, &resolved).is_err() {
            return "needs_credentials".to_string();
        }
    }
    if registered_in_process_host(service, &manifest)
        || registered_external_host(service, record, &manifest)
    {
        "ready".to_string()
    } else {
        "host_unavailable".to_string()
    }
}

pub(crate) fn effective_health_state(service: &TradeAssemblyService, record: &Value) -> String {
    let computed = health_state(service, record);
    if computed == "ready" && record["health"]["state"] == "degraded" {
        "degraded".to_string()
    } else {
        computed
    }
}

fn registered_in_process_host(service: &TradeAssemblyService, manifest: &Value) -> bool {
    if manifest["source"]["type"] != "core"
        || manifest["manifest"]["runtime"]["protocol"] != "rust-native"
    {
        return false;
    }
    let plugin_ref = string(manifest, "pluginRef");
    builtin_manifests(service).into_iter().any(|registered| {
        string(&registered, "pluginRef") == plugin_ref
            && registered["manifestDigest"] == manifest["manifestDigest"]
    })
}

fn registered_external_host(
    service: &TradeAssemblyService,
    record: &Value,
    manifest: &Value,
) -> bool {
    let package_sha256 = string(record, "activePackageSha256");
    if package_sha256.is_empty() || manifest["package"]["packageSha256"] != package_sha256 {
        return false;
    }
    service
        .runtime()
        .plugin_packages
        .get(&package_sha256)
        .ok()
        .flatten()
        .is_some_and(|package| {
            package.plugin_ref == string(record, "pluginRef")
                && package.manifest_sha256 == manifest["manifestDigest"]
        })
}

fn removed(record: &Value) -> bool {
    record
        .get("removedAtMs")
        .is_some_and(|value| !value.is_null())
}

pub(super) fn with_live_status(service: &TradeAssemblyService, mut record: Value) -> Value {
    let credential_ref = string(&record, "credentialRef");
    if !credential_ref.is_empty() {
        record = live_execution_checks::apply_current_credential_status(
            record,
            &service.runtime().credentials.status(&credential_ref),
        );
    }
    let state = effective_health_state(service, &record);
    record["health"]["state"] = json!(state);
    record["health"]["evidenceCurrent"] = json!(health_evidence_current(&record));
    record
}

// Only non-secret configuration and credential generation metadata are bound.
// This establishes observation identity, not freshness or trading readiness.
fn health_evidence_binding(record: &Value) -> Value {
    live_execution_checks::live_health_evidence_binding(record)
}

pub(crate) fn live_health_evidence_current(record: &Value) -> bool {
    live_execution_checks::live_health_evidence_current(record)
}

fn health_evidence_current(record: &Value) -> bool {
    live_execution_checks::live_health_evidence_current(record)
}

fn credential_status(status: &crate::ports::CredentialStatus, revision: u64) -> Value {
    json!({
        "configured": status.configured,
        "custody": status.custody,
        "redactedDisplay": status.redacted_display,
        "revisionRef": format!("credential-generation:{revision}"),
    })
}

fn credential_revision(record: &Value) -> u64 {
    record["credentialRevision"].as_u64().unwrap_or(0)
}

fn next_credential_revision(record: &Value) -> u64 {
    credential_revision(record).saturating_add(1)
}

fn save_instance(
    service: &TradeAssemblyService,
    instance_ref: &str,
    record: Value,
) -> ServiceResponse {
    let context = context(
        "plugin.instance.updated",
        instance_ref,
        service.runtime().clock.now_ms(),
    );
    match service
        .runtime()
        .plugins
        .put_instance(instance_ref, record, &context)
    {
        Ok(record) => {
            ServiceResponse::ok(json!({"ok": true, "instance": with_live_status(service, record)}))
        }
        Err(message) => invalid("plugin_instance_persist_failed", &message),
    }
}

fn reject_if_in_use(service: &TradeAssemblyService, instance_ref: &str) -> Option<ServiceResponse> {
    let dependencies = super::execution::plugin_dependency_runs(service, instance_ref);
    if dependencies.is_empty() {
        return None;
    }
    Some(ServiceResponse::conflict_with_details(
        "plugin_instance_in_use",
        json!({
            "instanceRef": instance_ref,
            "dependencies": dependencies,
            "requiredAction": "stop_execution",
        }),
    ))
}

fn manifest_digest(manifest: &Value) -> Result<String, String> {
    let bytes = serde_json_canonicalizer::to_vec(manifest)
        .map_err(|_| "manifest cannot be canonicalized".to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':'))
        && !value.contains("..")
}

fn valid_capability_ref(value: &str) -> bool {
    let mut parts = value.split('@');
    let Some(name) = parts.next() else {
        return false;
    };
    if !valid_ref(name) || !name.contains('.') {
        return false;
    }
    let Some(version) = parts.next() else {
        return true;
    };
    parts.next().is_none()
        && !version.is_empty()
        && version.split('.').count() <= 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn context(event_type: &str, key: &str, now: i64) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("{event_type}:{key}:{now}"))
            .expect("valid plugin lifecycle idempotency key"),
    )
}

fn invalid(code: &str, message: &str) -> ServiceResponse {
    ServiceResponse::bad_request_with_details(code, json!({"message": message}))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::service::TradeAssemblyService;

    #[test]
    fn health_observation_is_bound_to_configuration_and_credential_generation() {
        let mut record = json!({
            "instanceRef":"test", "pluginRef":"test.plugin", "enabled":true,
            "configuration":{"tradingApiUrl":"https://example.invalid"},
            "credentialRevision":1, "credentialStatus":{"configured":true},
            "accountMode":"live", "accountRef":"account://test/one",
            "health":{"state":"ready", "connectivityChecked":true}
        });
        assert!(!health_evidence_current(&record));
        record["health"]["binding"] = health_evidence_binding(&record);
        assert!(health_evidence_current(&record));
        for (field, value) in [
            ("credentialRevision", json!(2)),
            (
                "configuration",
                json!({"tradingApiUrl":"https://changed.invalid"}),
            ),
            ("accountMode", json!("paper")),
            ("accountRef", json!("account://test/two")),
            ("activePackageSha256", json!("changed")),
            ("enabled", json!(false)),
            ("credentialStatus", json!({"configured":false})),
        ] {
            let mut changed = record.clone();
            changed[field] = value;
            assert!(!health_evidence_current(&changed), "{field}");
        }
    }

    fn dependency(dependency_id: &str, capability: &str) -> Value {
        json!({
            "dependencyId": dependency_id,
            "capability": capability,
            "required": true,
            "traits": {
                "modes": ["backtest"],
                "instrumentFamilies": ["equity"],
                "dataShapes": ["bars"],
                "fields": ["close"],
                "inputSchemaRefs": [],
                "outputSchemaRefs": ["schema://market-data/bars@1"],
                "timeframes": ["1d"],
                "deterministic": true,
                "replayable": true
            }
        })
    }

    #[test]
    fn runtime_manifest_dependency_validation_rejects_unsafe_graph_edges() {
        let duplicate = json!({
            "dependencies": [
                dependency("bars", "market_data.bars.read@1"),
                dependency("bars", "market_data.quote.read@1")
            ]
        });
        assert!(validate_operation_dependencies(
            "indicator.rsi.calculate",
            "indicator.rsi.calculate@1",
            &duplicate,
        )
        .unwrap_err()
        .contains("duplicate dependency id"));

        let self_dependency = json!({
            "dependencies": [dependency("self", "indicator.rsi.calculate@1")]
        });
        assert!(validate_operation_dependencies(
            "indicator.rsi.calculate",
            "indicator.rsi.calculate@1",
            &self_dependency,
        )
        .unwrap_err()
        .contains("self-dependency"));

        let malformed = json!({"dependencies": [dependency("bad", "NOT-A-CAPABILITY")]});
        assert!(validate_operation_dependencies(
            "indicator.rsi.calculate",
            "indicator.rsi.calculate@1",
            &malformed,
        )
        .is_err());
    }

    fn oauth_fixture(alternatives: bool) -> (TradeAssemblyService, String) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let database = std::env::temp_dir().join(format!(
            "tradeassembly-oauth-lifecycle-{}-{nonce}.db",
            std::process::id(),
        ));
        let service = TradeAssemblyService::test_local(database.to_string_lossy().to_string());
        let plugin_ref = "fixture.oauth".to_string();
        let instance_ref = format!("fixture-instance-{nonce}");
        let mut manifest = builtin_manifests(&service)
            .into_iter()
            .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
            .expect("simbroker fixture")["manifest"]
            .clone();
        manifest["metadata"]["id"] = json!(plugin_ref);
        manifest["configuration"]["fields"] = json!([
            {"id": "api_key", "label": "API key", "description": "fixture", "inputType": "password", "required": true, "storageClass": "credential"},
            {"id": "api_secret", "label": "API secret", "description": "fixture", "inputType": "password", "required": true, "storageClass": "credential"},
            {"id": "access_token", "label": "Access token", "description": "fixture", "inputType": "password", "required": false, "storageClass": "credential"}
        ]);
        if alternatives {
            manifest["health"]["requiredCredentials"] = json!([]);
            manifest["health"]["credentialAlternatives"] =
                json!([["api_key", "api_secret"], ["access_token"]]);
        } else {
            manifest["health"]["requiredCredentials"] = json!(["api_key", "api_secret"]);
            manifest["health"]
                .as_object_mut()
                .expect("health object")
                .remove("credentialAlternatives");
        }
        let digest = manifest_digest(&manifest).expect("fixture digest");
        let installed = install_manifest(
            &service,
            json!({
                "manifest": manifest,
                "source": {"type": "package", "locator": "fixture-manifest.json", "package": {"type": "file", "locator": "fixture-manifest.json"}},
                "integrity": {"manifestSha256": digest}
            }),
        );
        assert_eq!(installed.status, 200, "{installed:?}");
        let created = create_instance(
            &service,
            json!({
            "instanceRef": instance_ref,
                "pluginRef": plugin_ref,
                "enabled": true,
                "configuration": {}
            }),
        );
        assert_eq!(created.status, 200, "{created:?}");
        (service, instance_ref)
    }

    fn resolved(service: &TradeAssemblyService, instance_ref: &str) -> BTreeMap<String, String> {
        service
            .runtime()
            .credentials
            .resolve_fields(instance_ref)
            .expect("resolve credentials")
            .expect("stored credentials")
    }

    #[test]
    fn credential_alternatives_replace_atomically_and_reject_mixed_or_partial_values() {
        let (service, instance_ref) = oauth_fixture(true);
        let pair = store_credentials(
            &service,
            &instance_ref,
            json!({"credentials": {"api_key": "key-1", "api_secret": "secret-1"}}),
        );
        assert_eq!(pair.status, 200);
        assert_eq!(resolved(&service, &instance_ref).len(), 2);

        let mixed = store_credentials(
            &service,
            &instance_ref,
            json!({"credentials": {"api_key": "key-2", "access_token": "token-2"}}),
        );
        assert_eq!(mixed.status, 400);
        assert_eq!(
            resolved(&service, &instance_ref).get("api_key"),
            Some(&"key-1".to_string())
        );

        let partial = store_credentials(
            &service,
            &instance_ref,
            json!({"credentials": {"access_token": ""}}),
        );
        assert_eq!(partial.status, 400);
        assert_eq!(
            resolved(&service, &instance_ref).get("api_secret"),
            Some(&"secret-1".to_string())
        );

        let token = store_credentials(
            &service,
            &instance_ref,
            json!({"credentials": {"access_token": "token-2"}}),
        );
        assert_eq!(token.status, 200);
        assert_eq!(
            resolved(&service, &instance_ref).keys().collect::<Vec<_>>(),
            vec![&"access_token".to_string()]
        );

        let pair_again = store_credentials(
            &service,
            &instance_ref,
            json!({"credentials": {"api_key": "key-3", "api_secret": "secret-3"}}),
        );
        assert_eq!(pair_again.status, 200);
        assert_eq!(resolved(&service, &instance_ref).len(), 2);
        assert_ne!(
            health_state(&service, &get_instance(&service, &instance_ref).body),
            "needs_credentials"
        );
    }

    #[test]
    fn legacy_required_credentials_remain_backward_compatible() {
        let (service, instance_ref) = oauth_fixture(false);
        let missing = store_credentials(
            &service,
            &instance_ref,
            json!({"credentials": {"api_key": "key-only"}}),
        );
        assert_eq!(missing.status, 400);
        let complete = store_credentials(
            &service,
            &instance_ref,
            json!({"credentials": {"api_key": "key", "api_secret": "secret"}}),
        );
        assert_eq!(complete.status, 200);
    }
}
