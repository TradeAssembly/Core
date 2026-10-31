//! Current immutable operation identity shared by plans and host captures.
use super::TradeAssemblyService;
use crate::{data_pipeline::canonical_sha256, ports::CapabilitySelection};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedOperation {
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub operation_id: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub context_sha256: String,
    pub account_ref: Option<String>,
}
pub(crate) fn pin(
    service: &TradeAssemblyService,
    selected: &CapabilitySelection,
) -> Result<PinnedOperation, String> {
    pin_with_package_loader(service, selected, |digest| {
        service
            .runtime()
            .plugin_packages
            .get(digest)?
            .ok_or("data_plan_package_missing".into())
    })
}

/// Only immutable package data may be reused inside one current-validation call.
/// Instance, ownership, configuration and credential bindings are always read now.
pub(crate) fn pin_with_package_loader(
    service: &TradeAssemblyService,
    selected: &CapabilitySelection,
    load: impl FnOnce(&str) -> Result<crate::ports::InstalledPluginPackage, String>,
) -> Result<PinnedOperation, String> {
    service
        .require_object("plugin_instance", &selected.plugin_instance_ref)
        .map_err(|_| "data_plan_not_available")?;
    let runtime = service.runtime();
    let instance = runtime
        .plugins
        .get_instance(&selected.plugin_instance_ref)?
        .ok_or("data_plan_instance_missing")?;
    if instance["enabled"] != true
        || instance["pluginRef"] != selected.plugin_ref
        || instance["instanceRef"] != selected.plugin_instance_ref
        || instance.get("removedAtMs").is_some_and(|v| !v.is_null())
    {
        return Err("data_plan_instance_stale".into());
    }
    let digest = instance["activePackageSha256"]
        .as_str()
        .ok_or("data_plan_package_missing")?;
    let manifest = runtime
        .plugins
        .get_manifest(&selected.plugin_ref)?
        .ok_or("data_plan_manifest_missing")?;
    if manifest["manifestDigest"] != selected.manifest_fingerprint
        || manifest["package"]["packageSha256"] != digest
    {
        return Err("data_plan_package_binding_mismatch".into());
    }
    let package = load(digest)?;
    if package.package_sha256 != digest
        || package.plugin_ref != selected.plugin_ref
        || package.manifest_sha256 != selected.manifest_fingerprint
    {
        return Err("data_plan_package_binding_mismatch".into());
    }
    let context_sha256 = canonical_sha256(
        &json!({"configuration":instance.get("configuration").cloned().unwrap_or_else(||json!({})),
        "accountRef":instance["accountRef"],"credentialRef":instance["credentialRef"],
        "credentialRevision":crate::live_execution_checks::credential_revision(&instance)}),
    )
    .map_err(|_| "data_plan_binding_invalid")?;
    Ok(PinnedOperation {
        plugin_instance_ref: selected.plugin_instance_ref.clone(),
        plugin_ref: selected.plugin_ref.clone(),
        operation_id: selected.operation_id.clone(),
        package_sha256: digest.into(),
        manifest_sha256: selected.manifest_fingerprint.clone(),
        context_sha256,
        account_ref: selected.account_ref.clone(),
    })
}
