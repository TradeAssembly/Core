// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use regex::Regex;
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tradeassembly_runtime::service::TradeAssemblyService;

const BUNDLE_PATH: &str = "plugin-contracts/default-bundle.yaml";
const LOCK_PATH: &str = "plugin-contracts/tradeassembly.plugins.lock.yaml";
const EXTERNAL_DEFAULTS_PATH: &str = "plugin-contracts/default-external-plugins.json";
const MANIFEST_SCHEMA_PATH: &str = "plugin-contracts/schemas/plugin-manifest.schema.json";
const LOCK_SCHEMA_PATH: &str = "plugin-contracts/schemas/plugin-lock.schema.json";
const DOC_PATH: &str = "docs/reference/plugins/plugin-system.md";

type Result<T> = std::result::Result<T, String>;

pub fn run_plugin_contract(args: &[String], root: &Path) -> i32 {
    if let Some(arg) = args.first() {
        eprintln!("unknown plugin-contract argument: {arg}");
        return 1;
    }

    match validate_plugin_contract(root) {
        Ok(report) => {
            println!(
                "Plugin contract passed: bundle_plugins={} locked_plugins={} manifests={}",
                report.bundle_plugins, report.locked_plugins, report.manifests
            );
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

struct PluginContractReport {
    bundle_plugins: usize,
    locked_plugins: usize,
    manifests: usize,
}

fn validate_plugin_contract(root: &Path) -> Result<PluginContractReport> {
    let bundle = read_yaml(root.join(BUNDLE_PATH))?;
    let lock = read_yaml(root.join(LOCK_PATH))?;

    for required in [
        MANIFEST_SCHEMA_PATH,
        LOCK_SCHEMA_PATH,
        EXTERNAL_DEFAULTS_PATH,
        DOC_PATH,
    ] {
        if !root.join(required).is_file() {
            return Err(format!("missing plugin contract file: {required}"));
        }
    }

    assert_yaml_string(&bundle, &["apiVersion"], "tradeassembly.org/v1alpha1")?;
    assert_yaml_string(&bundle, &["kind"], "TradeAssemblyPluginBundle")?;
    assert_yaml_string(&bundle, &["metadata", "name"], "tradeassembly-default")?;
    assert_yaml_string(&lock, &["apiVersion"], "tradeassembly.org/v1alpha1")?;
    assert_yaml_string(&lock, &["kind"], "TradeAssemblyPluginLock")?;

    let bundle_plugins = yaml_sequence(&bundle, &["plugins"])?;
    let locked_plugins = yaml_sequence(&lock, &["plugins"])?;

    if bundle_plugins.is_empty() {
        return Err("default plugin bundle must not be empty".to_string());
    }

    let mut bundle_ids = BTreeSet::new();
    let mut bundle_manifest_paths = BTreeMap::new();
    let mut manifest_count = 0;

    for plugin in bundle_plugins {
        let id = yaml_string(plugin, &["id"])?;
        let locator = yaml_string(plugin, &["locator"])?;
        validate_plugin_id(&id)?;
        validate_locator(&locator)?;
        if !bundle_ids.insert(id.clone()) {
            return Err(format!("duplicate default bundle plugin id: {id}"));
        }

        let manifest_path = yaml_string(plugin, &["manifestPath"])?;
        validate_manifest(root, &manifest_path, &id)?;
        bundle_manifest_paths.insert(id.clone(), manifest_path);
        manifest_count += 1;
    }

    let mut locked_ids = BTreeSet::new();
    for plugin in locked_plugins {
        let id = yaml_string(plugin, &["id"])?;
        validate_plugin_id(&id)?;
        if !bundle_ids.contains(&id) {
            return Err(format!("lock contains plugin outside default bundle: {id}"));
        }
        if !locked_ids.insert(id.clone()) {
            return Err(format!("duplicate plugin lock id: {id}"));
        }

        let source_type = yaml_string(plugin, &["source", "type"])?;
        let manifest_path = bundle_manifest_paths
            .get(&id)
            .ok_or_else(|| format!("lock contains plugin without bundle manifest path: {id}"))?;
        match source_type.as_str() {
            "core" => validate_core_lock(root, plugin, manifest_path)?,
            "git" => validate_git_lock(root, plugin, manifest_path)?,
            _ => {
                return Err(format!(
                    "unsupported plugin lock source type: {source_type}"
                ))
            }
        }
    }

    for id in &bundle_ids {
        if !locked_ids.contains(id) {
            return Err(format!("default bundle plugin missing from lockfile: {id}"));
        }
    }
    validate_runtime_manifest_alignment(root, &bundle_manifest_paths)?;
    validate_default_external_plugins(root)?;

    Ok(PluginContractReport {
        bundle_plugins: bundle_ids.len(),
        locked_plugins: locked_ids.len(),
        manifests: manifest_count,
    })
}

fn validate_default_external_plugins(root: &Path) -> Result<()> {
    let defaults = read_yaml(root.join(EXTERNAL_DEFAULTS_PATH))?;
    assert_yaml_string(
        &defaults,
        &["schemaVersion"],
        "tradeassembly.default-external-plugins.v1",
    )?;
    let plugins = yaml_sequence(&defaults, &["plugins"])?;
    if plugins.is_empty() {
        return Err("default external plugin selection must not be empty".to_string());
    }
    let mut selected = 0;
    let mut plugin_ids = BTreeSet::new();
    for plugin in plugins {
        let plugin_ref = yaml_string(plugin, &["pluginRef"])?;
        validate_plugin_id(&plugin_ref)?;
        if !plugin_ids.insert(plugin_ref.clone()) {
            return Err(format!(
                "duplicate default external plugin selection: {plugin_ref}"
            ));
        }
        validate_non_empty(&yaml_string(plugin, &["version"])?, "plugin version")?;
        let repository = yaml_string(plugin, &["repository"])?;
        if !repository.starts_with("https://") {
            return Err(format!(
                "default external plugin repository must use HTTPS: {plugin_ref}"
            ));
        }
        let source_checkout_dir = yaml_string(plugin, &["sourceCheckoutDir"])?;
        validate_safe_relative_path(&source_checkout_dir, "source checkout directory")?;
        if yaml_bool(plugin, &["selectedByDefault"])? {
            selected += 1;
        }
        if !yaml_bool(plugin, &["optional"])? {
            return Err(format!(
                "default external plugin must remain optional: {plugin_ref}"
            ));
        }
        let targets = yaml_sequence(plugin, &["targets"])?;
        if targets.is_empty() {
            return Err(format!(
                "default external plugin must declare at least one target: {plugin_ref}"
            ));
        }
        let mut target_ids = BTreeSet::new();
        for target in targets {
            let target_id = yaml_string(target, &["target"])?;
            validate_non_empty(&target_id, "plugin target")?;
            if !target_ids.insert(target_id.clone()) {
                return Err(format!(
                    "duplicate default external plugin target: {plugin_ref}/{target_id}"
                ));
            }
            let package_url = yaml_string(target, &["packageUrl"])?;
            if !package_url.starts_with("https://") {
                return Err(format!(
                    "default external plugin package URL must use HTTPS: {plugin_ref}/{target_id}"
                ));
            }
            let package_file = yaml_string(target, &["packageFile"])?;
            validate_safe_relative_path(&package_file, "source package file")?;
            validate_sha256(&yaml_string(target, &["packageSha256"])?)?;
            validate_sha256(&yaml_string(target, &["manifestSha256"])?)?;
        }
    }
    if selected != 1 {
        return Err(format!(
            "exactly one external plugin must be selected by default, found {selected}"
        ));
    }
    Ok(())
}

fn validate_safe_relative_path(value: &str, label: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("{label} must be a safe relative path: {value}"));
    }
    Ok(())
}

fn validate_runtime_manifest_alignment(
    root: &Path,
    bundle_manifest_paths: &BTreeMap<String, String>,
) -> Result<()> {
    let runtime_dir = tempfile::tempdir()
        .map_err(|error| format!("failed to create plugin contract runtime directory: {error}"))?;
    let runtime_db = runtime_dir.path().join("plugin-contract.db");
    let payload = TradeAssemblyService::test_local(runtime_db.to_string_lossy().into_owned())
        .handle_http("POST", "/product/plugins/providers", serde_json::json!({}))
        .body;
    let runtime_plugins = payload
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "runtime plugin workspace did not return plugins array".to_string())?;
    assert_runtime_plugin_set(runtime_plugins, bundle_manifest_paths)?;

    for (plugin_id, manifest_path) in bundle_manifest_paths {
        let manifest = read_yaml(root.join(manifest_path))?;
        let runtime_plugin = runtime_plugins
            .iter()
            .find(|plugin| {
                plugin.get("pluginRef").and_then(serde_json::Value::as_str)
                    == Some(plugin_id.as_str())
            })
            .ok_or_else(|| format!("runtime plugin catalog missing bundled plugin: {plugin_id}"))?;
        let runtime_operations = runtime_plugin
            .get("manifest")
            .and_then(|manifest| manifest.get("operations"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                format!("runtime plugin {plugin_id} did not expose manifest operations")
            })?;

        let manifest_operations = yaml_sequence(&manifest, &["operations"])?;
        if runtime_operations.len() != manifest_operations.len() {
            return Err(format!(
                "runtime plugin {plugin_id} operation count mismatch: expected {}, actual {}",
                manifest_operations.len(),
                runtime_operations.len()
            ));
        }
        for operation in manifest_operations {
            let operation_id = yaml_string(operation, &["id"])?;
            let runtime_operation = runtime_operations
                .iter()
                .find(|item| {
                    item.get("id").and_then(serde_json::Value::as_str)
                        == Some(operation_id.as_str())
                })
                .ok_or_else(|| {
                    format!("runtime plugin {plugin_id} missing manifest operation: {operation_id}")
                })?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "capability",
                &yaml_string(operation, &["capability"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "protocol",
                &yaml_string(operation, &["protocol"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "resourceType",
                &yaml_string(operation, &["resourceType"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "financeResourceType",
                &yaml_string(operation, &["financeAction", "resourceType"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "effect",
                &yaml_string(operation, &["effect"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "risk",
                &yaml_string(operation, &["riskClass"])?,
            )?;
            assert_runtime_bool(
                plugin_id,
                &operation_id,
                runtime_operation,
                "credentialGrantRequired",
                yaml_bool(operation, &["credentialGrantRequired"])?,
            )?;
            assert_runtime_bool(
                plugin_id,
                &operation_id,
                runtime_operation,
                "accountBindingRequired",
                yaml_bool(operation, &["accountBindingRequired"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "apfActionId",
                &yaml_string(operation, &["financeAction", "actionId"])?,
            )?;
            assert_runtime_bool(
                plugin_id,
                &operation_id,
                runtime_operation,
                "mandateRequired",
                yaml_bool(operation, &["mandateRequired"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "purpose",
                &yaml_string(operation, &["purpose"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "sessionAdmission",
                &yaml_string(operation, &["sessionAdmission", "mode"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "approvalMode",
                &yaml_string(operation, &["approval", "mode"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "supervisionMode",
                &yaml_string(operation, &["supervision", "mode"])?,
            )?;
            assert_runtime_sequence(
                plugin_id,
                &operation_id,
                runtime_operation,
                "detectors",
                yaml_sequence(operation, &["detectors"])?,
            )?;
            assert_runtime_sequence(
                plugin_id,
                &operation_id,
                runtime_operation,
                "contextProviders",
                yaml_sequence(operation, &["contextProviders"])?,
            )?;
            assert_runtime_sequence(
                plugin_id,
                &operation_id,
                runtime_operation,
                "policyRefs",
                yaml_sequence(operation, &["policyRefs"])?,
            )?;
            assert_runtime_sequence(
                plugin_id,
                &operation_id,
                runtime_operation,
                "evidence",
                yaml_sequence(operation, &["evidence"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "pepCoverageClass",
                &yaml_string(operation, &["pepCoverage"])?,
            )?;
            assert_runtime_sequence(
                plugin_id,
                &operation_id,
                runtime_operation,
                "checkPacks",
                yaml_sequence(operation, &["checkPacks"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "receiptClass",
                &yaml_string(operation, &["receiptClass"])?,
            )?;
            assert_runtime_string(
                plugin_id,
                &operation_id,
                runtime_operation,
                "redaction",
                &yaml_string(operation, &["redaction"])?,
            )?;
            assert_runtime_bool(
                plugin_id,
                &operation_id,
                runtime_operation,
                "noAdvice",
                yaml_bool(operation, &["noAdvice"])?,
            )?;
            assert_runtime_sequence(
                plugin_id,
                &operation_id,
                runtime_operation,
                "supportedProtocols",
                yaml_sequence(operation, &["supportedProtocols"])?,
            )?;
            assert_canonical_operation(plugin_id, &operation_id, runtime_operation, operation)?;
        }
    }

    Ok(())
}

fn assert_runtime_plugin_set(
    runtime_plugins: &[serde_json::Value],
    bundle_manifest_paths: &BTreeMap<String, String>,
) -> Result<()> {
    let runtime_ids = runtime_plugins
        .iter()
        .map(|plugin| {
            plugin
                .get("pluginRef")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
                .ok_or_else(|| "runtime plugin is missing pluginRef".to_string())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if runtime_ids.len() != runtime_plugins.len() {
        return Err("runtime plugin workspace contains duplicate pluginRef values".to_string());
    }
    let bundled_ids = bundle_manifest_paths
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    if runtime_ids == bundled_ids {
        Ok(())
    } else {
        Err(format!(
            "runtime/default-bundle plugin set mismatch: bundled={bundled_ids:?}, runtime={runtime_ids:?}"
        ))
    }
}

fn assert_canonical_operation(
    plugin_id: &str,
    operation_id: &str,
    runtime_operation: &serde_json::Value,
    manifest_operation: &Value,
) -> Result<()> {
    let expected_operation = serde_json::to_value(manifest_operation).map_err(|error| {
        format!("failed to normalize manifest operation {plugin_id}/{operation_id}: {error}")
    })?;
    if runtime_operation == &expected_operation {
        Ok(())
    } else {
        Err(format!(
            "runtime plugin {plugin_id} operation {operation_id} differs from canonical manifest"
        ))
    }
}

fn assert_runtime_string(
    plugin_id: &str,
    operation_id: &str,
    runtime_operation: &serde_json::Value,
    field: &str,
    expected: &str,
) -> Result<()> {
    let actual = match field {
        "financeResourceType" => runtime_operation.pointer("/financeAction/resourceType"),
        "risk" => runtime_operation.get("riskClass"),
        "apfActionId" => runtime_operation.pointer("/financeAction/actionId"),
        "sessionAdmission" => runtime_operation.pointer("/sessionAdmission/mode"),
        "approvalMode" => runtime_operation.pointer("/approval/mode"),
        "supervisionMode" => runtime_operation.pointer("/supervision/mode"),
        "pepCoverageClass" => runtime_operation.get("pepCoverage"),
        _ => runtime_operation.get(field),
    }
    .and_then(serde_json::Value::as_str)
    .ok_or_else(|| {
        format!("runtime plugin {plugin_id} operation {operation_id} missing {field}")
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "runtime plugin {plugin_id} operation {operation_id} {field} mismatch: expected {expected}, actual {actual}"
        ))
    }
}

fn assert_runtime_bool(
    plugin_id: &str,
    operation_id: &str,
    runtime_operation: &serde_json::Value,
    field: &str,
    expected: bool,
) -> Result<()> {
    let actual = runtime_operation
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            format!("runtime plugin {plugin_id} operation {operation_id} missing {field}")
        })?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "runtime plugin {plugin_id} operation {operation_id} {field} mismatch: expected {expected}, actual {actual}"
        ))
    }
}

fn assert_runtime_sequence(
    plugin_id: &str,
    operation_id: &str,
    runtime_operation: &serde_json::Value,
    field: &str,
    expected: &[Value],
) -> Result<()> {
    let actual = runtime_operation
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("runtime plugin {plugin_id} operation {operation_id} missing {field}"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(ToString::to_string)
                .ok_or_else(|| format!("runtime plugin {plugin_id} operation {operation_id} {field} has non-string item"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let expected = expected
        .iter()
        .map(yaml_scalar_string)
        .collect::<Result<BTreeSet<_>>>()?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "runtime plugin {plugin_id} operation {operation_id} {field} mismatch: expected {expected:?}, actual {actual:?}"
        ))
    }
}

fn validate_manifest(root: &Path, relative: &str, expected_id: &str) -> Result<()> {
    let manifest = read_yaml(root.join(relative))?;
    assert_yaml_string(&manifest, &["apiVersion"], "tradeassembly.org/v1alpha1")?;
    assert_yaml_string(&manifest, &["kind"], "TradeAssemblyPluginManifest")?;
    assert_yaml_string(&manifest, &["metadata", "id"], expected_id)?;
    assert_yaml_string(&manifest, &["manifestVersion"], "0.1")?;
    validate_non_empty(
        &yaml_string(&manifest, &["metadata", "provider", "id"])?,
        "metadata.provider.id",
    )?;
    validate_non_empty(
        &yaml_string(&manifest, &["metadata", "provider", "name"])?,
        "metadata.provider.name",
    )?;

    let capabilities = yaml_sequence(&manifest, &["capabilities"])?;
    let permissions = yaml_sequence(&manifest, &["permissions"])?;
    let operations = yaml_sequence(&manifest, &["operations"])?;
    let host_apis = yaml_sequence(&manifest, &["hostApis"])?;
    let forbidden = yaml_sequence(&manifest, &["forbidden"])?;
    let configuration_fields = yaml_sequence(&manifest, &["configuration", "fields"])?;

    if capabilities.is_empty() || permissions.is_empty() || operations.is_empty() {
        return Err(format!(
            "plugin manifest {expected_id} must declare capabilities, permissions, and operations"
        ));
    }

    let mut capability_ids = BTreeSet::new();
    for capability in capabilities {
        let id = yaml_string(capability, &["id"])?;
        validate_capability_id(&id)?;
        if !capability_ids.insert(id.clone()) {
            return Err(format!(
                "plugin manifest {expected_id} duplicate capability declaration: {id}"
            ));
        }
    }
    for permission in permissions {
        validate_declaration_id(&yaml_string(permission, &["id"])?, "permission")?;
    }
    let mut operation_ids = BTreeSet::new();
    for operation in operations {
        let operation_id = yaml_string(operation, &["id"])?;
        validate_declaration_id(&operation_id, "operation")?;
        if !operation_ids.insert(operation_id.clone()) {
            return Err(format!(
                "plugin manifest {expected_id} duplicate operation declaration: {operation_id}"
            ));
        }
        let capability = yaml_string(operation, &["capability"])?;
        validate_capability_id(&capability)?;
        if !capability_ids.contains(&capability) {
            return Err(format!(
                "plugin manifest {expected_id} operation {operation_id} references undeclared capability: {capability}"
            ));
        }
        validate_one_of(
            &yaml_string(operation, &["protocol"])?,
            &[
                "local-rust",
                "stdio",
                "grpc",
                "rest",
                "mcp",
                "openapi",
                "graphql",
                "native-sdk",
                "constrained-code",
                "webhook",
            ],
            "operation protocol",
        )?;
        validate_one_of(
            &yaml_string(operation, &["effect"])?,
            &[
                "read",
                "research",
                "draft",
                "write",
                "credential",
                "export",
                "admin",
            ],
            "operation effect",
        )?;
        validate_one_of(
            &yaml_string(operation, &["riskClass"])?,
            &["low", "medium", "high", "critical"],
            "operation riskClass",
        )?;
        validate_non_empty(
            &yaml_string(operation, &["resourceType"])?,
            "operation resourceType",
        )?;
        assert_yaml_bool(operation, &["credentialGrantRequired"])?;
        assert_yaml_bool(operation, &["accountBindingRequired"])?;
        let action_id = yaml_string(operation, &["financeAction", "actionId"])?;
        validate_non_empty(&action_id, "financeAction.actionId")?;
        validate_non_empty(
            &yaml_string(operation, &["financeAction", "resourceType"])?,
            "financeAction.resourceType",
        )?;
        assert_yaml_bool(operation, &["mandateRequired"])?;
        validate_non_empty(&yaml_string(operation, &["purpose"])?, "operation purpose")?;
        validate_one_of(
            &yaml_string(operation, &["sessionAdmission", "mode"])?,
            &[
                "none",
                "user_session",
                "agent_session",
                "user_or_agent_session",
                "service_account",
            ],
            "operation sessionAdmission.mode",
        )?;
        validate_one_of(
            &yaml_string(operation, &["approval", "mode"])?,
            &[
                "none",
                "policy_configured",
                "human_confirmation",
                "human_confirmation_or_policy",
                "supervisor",
            ],
            "operation approval.mode",
        )?;
        validate_one_of(
            &yaml_string(operation, &["supervision", "mode"])?,
            &[
                "none",
                "automated_policy",
                "human_in_the_loop",
                "human_on_the_loop",
            ],
            "operation supervision.mode",
        )?;
        validate_non_empty_sequence(operation, &["detectors"], "operation detectors")?;
        validate_non_empty_sequence(
            operation,
            &["contextProviders"],
            "operation contextProviders",
        )?;
        validate_non_empty_sequence(operation, &["policyRefs"], "operation policyRefs")?;
        validate_operation_traits(
            at_path(operation, &["traits"])?,
            &format!("operation {operation_id} traits"),
        )?;
        validate_operation_dependencies(operation, &operation_id, &capability)?;
        validate_non_empty_sequence(operation, &["evidence"], "operation evidence")?;
        validate_one_of(
            &yaml_string(operation, &["pepCoverage"])?,
            &["c0", "c1", "c2", "c3", "c4", "c5"],
            "operation pepCoverage",
        )?;
        validate_non_empty_sequence(operation, &["checkPacks"], "operation checkPacks")?;
        validate_non_empty(
            &yaml_string(operation, &["receiptClass"])?,
            "operation receiptClass",
        )?;
        validate_non_empty(
            &yaml_string(operation, &["redaction"])?,
            "operation redaction",
        )?;
        if !yaml_bool(operation, &["noAdvice"])? {
            return Err(format!(
                "plugin manifest {expected_id} operation {operation_id} must set noAdvice: true"
            ));
        }
        validate_non_empty_sequence(
            operation,
            &["supportedProtocols"],
            "operation supportedProtocols",
        )?;
        assert_yaml_string(operation, &["inputSchema"], "json-schema")?;
        assert_yaml_string(operation, &["outputSchema"], "json-schema")?;
    }
    for host_api in host_apis {
        validate_declaration_id(&yaml_string(host_api, &["id"])?, "host API")?;
    }

    let mut configuration_ids = BTreeSet::new();
    let mut credential_ids = BTreeSet::new();
    for field in configuration_fields {
        let id = yaml_string(field, &["id"])?;
        validate_configuration_field_id(&id)?;
        if !configuration_ids.insert(id.clone()) {
            return Err(format!(
                "plugin manifest {expected_id} duplicate configuration field: {id}"
            ));
        }
        validate_non_empty(&yaml_string(field, &["label"])?, "configuration label")?;
        validate_non_empty(
            &yaml_string(field, &["description"])?,
            "configuration description",
        )?;
        let input_type = yaml_string(field, &["inputType"])?;
        validate_one_of(
            &input_type,
            &["text", "password", "url", "number", "boolean", "select"],
            "configuration inputType",
        )?;
        assert_yaml_bool(field, &["required"])?;
        let storage_class = yaml_string(field, &["storageClass"])?;
        validate_one_of(
            &storage_class,
            &["configuration", "credential"],
            "configuration storageClass",
        )?;
        if let Some(defaults) = field.get("defaultByAccountMode") {
            if storage_class == "credential" {
                return Err(format!(
                    "plugin manifest {expected_id} credential field {id} cannot declare defaultByAccountMode"
                ));
            }
            let defaults = defaults.as_mapping().ok_or_else(|| {
                format!(
                    "plugin manifest {expected_id} defaultByAccountMode must be an object: {id}"
                )
            })?;
            if defaults.is_empty()
                || defaults
                    .keys()
                    .any(|key| !matches!(key.as_str(), Some("paper" | "live")))
            {
                return Err(format!(
                    "plugin manifest {expected_id} defaultByAccountMode must contain only paper/live values: {id}"
                ));
            }
        }
        if storage_class == "credential" {
            if input_type != "password" {
                return Err(format!(
                    "plugin manifest {expected_id} credential field {id} must use password input"
                ));
            }
            credential_ids.insert(id.clone());
        }
        if input_type == "select" {
            validate_non_empty_sequence(field, &["options"], "configuration options")?;
        }
    }

    if let Some(oauth) = manifest["configuration"].get("oauth") {
        let provider = yaml_string(oauth, &["provider"])?;
        if !is_slug(&provider) {
            return Err(format!(
                "plugin manifest {expected_id} OAuth provider must be a slug"
            ));
        }
        let credential_field = yaml_string(oauth, &["credentialField"])?;
        if !credential_ids.contains(&credential_field) {
            return Err(format!("plugin manifest {expected_id} OAuth credentialField is not a credential field: {credential_field}"));
        }
        let environments = yaml_sequence(oauth, &["environments"])?;
        let mut environment_ids = BTreeSet::new();
        for environment in environments {
            let id = yaml_string(environment, &["id"])?;
            if !is_slug(&id) || !environment_ids.insert(id.clone()) {
                return Err(format!("plugin manifest {expected_id} OAuth environment id is invalid or duplicated: {id}"));
            }
            validate_non_empty(
                &yaml_string(environment, &["label"])?,
                "OAuth environment label",
            )?;
            let relay = yaml_string(environment, &["relayBaseUrl"])?;
            validate_oauth_relay_origin(&relay)?;
        }
        let scopes = yaml_sequence(oauth, &["scopes"])?;
        let mut scope_ids = BTreeSet::new();
        for scope in scopes {
            let id = yaml_string(scope, &["id"])?;
            if !is_scope_id(&id) || !scope_ids.insert(id.clone()) {
                return Err(format!(
                    "plugin manifest {expected_id} OAuth scope id is invalid or duplicated: {id}"
                ));
            }
            validate_non_empty(&yaml_string(scope, &["label"])?, "OAuth scope label")?;
        }
    }

    let required_configuration = yaml_sequence(&manifest, &["health", "requiredConfiguration"])?;
    for field in required_configuration {
        let id = yaml_scalar_string(field)?;
        if !configuration_ids.contains(&id) || credential_ids.contains(&id) {
            return Err(format!(
                "plugin manifest {expected_id} health requires invalid configuration field: {id}"
            ));
        }
    }
    let required_credentials = yaml_sequence(&manifest, &["health", "requiredCredentials"])?;
    for field in required_credentials {
        let id = yaml_scalar_string(field)?;
        if !credential_ids.contains(&id) {
            return Err(format!(
                "plugin manifest {expected_id} health requires undeclared credential field: {id}"
            ));
        }
    }
    if let Some(alternatives) = manifest["health"].get("credentialAlternatives") {
        let alternatives = alternatives.as_sequence().ok_or_else(|| {
            format!("plugin manifest {expected_id} health credentialAlternatives must be arrays")
        })?;
        if alternatives.is_empty() {
            return Err(format!(
                "plugin manifest {expected_id} health credentialAlternatives must not be empty"
            ));
        }
        for alternative in alternatives {
            let fields = alternative
                .as_sequence()
                .ok_or_else(|| format!("plugin manifest {expected_id} health credentialAlternatives entries must be arrays"))?;
            if fields.is_empty() {
                return Err(format!("plugin manifest {expected_id} health credential alternatives must not be empty"));
            }
            let mut seen = BTreeSet::new();
            for field in fields {
                let id = yaml_scalar_string(field)?;
                if !credential_ids.contains(&id) || !seen.insert(id.clone()) {
                    return Err(format!("plugin manifest {expected_id} health credential alternative references invalid or duplicated field: {id}"));
                }
            }
        }
    }
    validate_one_of(
        &yaml_string(&manifest, &["trust", "sandbox"])?,
        &["in-process-core", "external-process", "remote-service"],
        "trust.sandbox",
    )?;
    validate_non_empty(&yaml_string(&manifest, &["trust", "level"])?, "trust.level")?;

    let forbidden_values = forbidden
        .iter()
        .map(yaml_scalar_string)
        .collect::<Result<BTreeSet<_>>>()?;
    for required in [
        "trade_advice",
        "raw_secret_output",
        "hosted_oauth_credentials",
        "billing",
        "payouts",
        "enterprise_sso_rbac",
    ] {
        if !forbidden_values.contains(required) {
            return Err(format!(
                "plugin manifest {expected_id} missing forbidden declaration: {required}"
            ));
        }
    }

    Ok(())
}

fn validate_core_lock(root: &Path, plugin: &Value, bundle_manifest_path: &str) -> Result<()> {
    let locator = yaml_string(plugin, &["source", "locator"])?;
    if !locator.starts_with("core://plugins/") {
        return Err(format!(
            "core plugin locator must use core://plugins/: {locator}"
        ));
    }
    let manifest_sha = yaml_string(plugin, &["integrity", "manifestSha256"])?;
    validate_manifest_digest(root, bundle_manifest_path, &manifest_sha)?;
    Ok(())
}

fn validate_git_lock(root: &Path, plugin: &Value, bundle_manifest_path: &str) -> Result<()> {
    let repo = yaml_string(plugin, &["source", "repo"])?;
    let commit = yaml_string(plugin, &["source", "commit"])?;
    let locator = yaml_string(plugin, &["source", "locator"])?;
    let manifest_path = yaml_string(plugin, &["source", "manifestPath"])?;
    let manifest_sha = yaml_string(plugin, &["integrity", "manifestSha256"])?;

    if repo.trim().is_empty() || repo == "TradeAssemblyHQ/TradeAssembly-Product" {
        return Err("plugin Git source repo must be external to Product".to_string());
    }
    validate_commit(&commit)?;
    validate_locator(&locator)?;
    validate_manifest_path(&manifest_path)?;
    validate_manifest_digest(root, bundle_manifest_path, &manifest_sha)?;
    Ok(())
}

fn validate_locator(locator: &str) -> Result<()> {
    if locator.starts_with("core://plugins/") {
        let id = locator.trim_start_matches("core://plugins/");
        return validate_plugin_id(id);
    }

    let parsed = parse_git_locator(locator)?;
    if parsed.repo.trim().is_empty() {
        return Err("git plugin locator repo is empty".to_string());
    }
    if parsed.reference.trim().is_empty() {
        return Err("git plugin locator ref is empty".to_string());
    }
    validate_manifest_path(&parsed.manifest_path)
}

fn parse_git_locator(locator: &str) -> Result<GitLocator> {
    let rest = locator
        .strip_prefix("git+")
        .ok_or_else(|| format!("unsupported plugin locator: {locator}"))?;
    let (before_fragment, manifest_path) = rest
        .split_once('#')
        .ok_or_else(|| format!("git plugin locator missing manifest fragment: {locator}"))?;
    let (repo, query) = before_fragment
        .split_once("?ref=")
        .ok_or_else(|| format!("git plugin locator missing ?ref=: {locator}"))?;
    Ok(GitLocator {
        repo: repo.to_string(),
        reference: query.to_string(),
        manifest_path: manifest_path.to_string(),
    })
}

#[derive(Debug)]
struct GitLocator {
    repo: String,
    reference: String,
    manifest_path: String,
}

fn validate_plugin_id(value: &str) -> Result<()> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"^[a-z0-9]+([._-][a-z0-9]+)+$").unwrap());
    if pattern.is_match(value) {
        Ok(())
    } else {
        Err(format!("invalid plugin id: {value}"))
    }
}

fn validate_declaration_id(value: &str, label: &str) -> Result<()> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"^[a-z][a-z0-9]*(\.[a-z0-9_]+)+$").unwrap());
    if !pattern.is_match(value) {
        return Err(format!("invalid {label} id: {value}"));
    }
    if value.contains("advice")
        || value.contains("billing")
        || value.contains("payout")
        || value.contains("hosted_oauth")
        || value.contains("enterprise_sso")
    {
        return Err(format!("forbidden {label} declaration: {value}"));
    }
    Ok(())
}

fn validate_capability_id(value: &str) -> Result<()> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(r"^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+(@[1-9][0-9]*(\.[0-9]+){0,2})?$").unwrap()
    });
    if !pattern.is_match(value) {
        return Err(format!("invalid capability id: {value}"));
    }
    validate_not_forbidden(value, "capability")
}

fn validate_not_forbidden(value: &str, label: &str) -> Result<()> {
    if value.contains("advice")
        || value.contains("billing")
        || value.contains("payout")
        || value.contains("hosted_oauth")
        || value.contains("enterprise_sso")
    {
        Err(format!("forbidden {label} declaration: {value}"))
    } else {
        Ok(())
    }
}

fn validate_operation_traits(value: &Value, label: &str) -> Result<()> {
    validate_string_set(
        value,
        &["modes"],
        label,
        Some(&[
            "authoring",
            "research",
            "backtest",
            "simulation",
            "paper",
            "live",
        ]),
    )?;
    validate_string_set(
        value,
        &["instrumentFamilies"],
        label,
        Some(&[
            "equity",
            "fund",
            "crypto_spot",
            "option_contract",
            "future_contract",
            "fixed_income",
            "fx_pair",
            "event_contract",
            "custom",
        ]),
    )?;
    for field in [
        "dataShapes",
        "fields",
        "inputSchemaRefs",
        "outputSchemaRefs",
        "timeframes",
    ] {
        validate_string_set(value, &[field], label, None)?;
    }
    for field in ["inputSchemaRefs", "outputSchemaRefs"] {
        for schema_ref in yaml_sequence(value, &[field])? {
            let schema_ref = yaml_scalar_string(schema_ref)?;
            if !valid_schema_ref(&schema_ref) {
                return Err(format!("{label} has invalid {field}: {schema_ref}"));
            }
        }
    }
    for field in ["maxFreshness", "maxLatencyMs"] {
        if let Some(item) = value.get(field) {
            if item.is_null() {
                continue;
            }
            match field {
                "maxFreshness" if item.as_str().is_some_and(|value| !value.is_empty()) => {}
                "maxLatencyMs" if item.as_u64().is_some() => {}
                _ => return Err(format!("{label} has invalid {field}")),
            }
        }
    }
    assert_yaml_bool(value, &["deterministic"])?;
    assert_yaml_bool(value, &["replayable"])?;
    Ok(())
}

fn validate_operation_dependencies(
    operation: &Value,
    operation_id: &str,
    operation_capability: &str,
) -> Result<()> {
    let mut dependency_ids = BTreeSet::new();
    for dependency in yaml_sequence(operation, &["dependencies"])? {
        let dependency_id = yaml_string(dependency, &["dependencyId"])?;
        validate_configuration_field_id(&dependency_id)?;
        if !dependency_ids.insert(dependency_id.clone()) {
            return Err(format!(
                "operation {operation_id} has duplicate dependency id: {dependency_id}"
            ));
        }
        let capability = yaml_string(dependency, &["capability"])?;
        validate_capability_id(&capability)?;
        let dependency_operation = dependency
            .get("operationId")
            .filter(|value| !value.is_null())
            .map(yaml_scalar_string)
            .transpose()?;
        if let Some(dependency_operation) = dependency_operation.as_deref() {
            validate_declaration_id(dependency_operation, "dependency operation")?;
        }
        if dependency_operation.as_deref() == Some(operation_id)
            || (dependency_operation.is_none() && capability == operation_capability)
        {
            return Err(format!(
                "operation {operation_id} dependency {dependency_id} is a self-dependency"
            ));
        }
        assert_yaml_bool(dependency, &["required"])?;
        validate_operation_traits(
            at_path(dependency, &["traits"])?,
            &format!("operation {operation_id} dependency {dependency_id} traits"),
        )?;
    }
    Ok(())
}

fn validate_string_set(
    value: &Value,
    path: &[&str],
    label: &str,
    allowed: Option<&[&str]>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for item in yaml_sequence(value, path)? {
        let item = yaml_scalar_string(item)?;
        validate_non_empty(&item, label)?;
        if !seen.insert(item.clone()) {
            return Err(format!(
                "{label} has duplicate {} value: {item}",
                path.join(".")
            ));
        }
        if allowed.is_some_and(|allowed| !allowed.contains(&item.as_str())) {
            return Err(format!(
                "{label} has unsupported {} value: {item}",
                path.join(".")
            ));
        }
    }
    Ok(())
}

fn valid_schema_ref(value: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| Regex::new(r"^schema://\S+@[1-9][0-9]*(\.[0-9]+){0,2}$").unwrap())
        .is_match(value)
}

fn validate_configuration_field_id(value: &str) -> Result<()> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());
    if pattern.is_match(value) && !value.contains("..") {
        Ok(())
    } else {
        Err(format!("invalid configuration field id: {value}"))
    }
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.split(['-', '_']).all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        })
}

fn is_scope_id(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | ':' | '-')
        })
        && value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
}

fn validate_oauth_relay_origin(value: &str) -> Result<()> {
    let Some(host) = value.strip_prefix("https://") else {
        return Err(format!(
            "OAuth relayBaseUrl must be an HTTPS origin: {value}"
        ));
    };
    if host.is_empty() || host.contains(['@', '/', '?', '#', ' ', '\t', '\n', '\r']) {
        return Err(format!("OAuth relayBaseUrl must be an HTTPS origin without credentials, path, or query: {value}"));
    }
    Ok(())
}

fn validate_one_of(value: &str, allowed: &[&str], label: &str) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!("invalid {label}: {value}"))
    }
}

fn validate_non_empty(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_non_empty_sequence(value: &Value, path: &[&str], label: &str) -> Result<()> {
    let items = yaml_sequence(value, path)?;
    if items.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    for item in items {
        validate_non_empty(&yaml_scalar_string(item)?, label)?;
    }
    Ok(())
}

fn validate_manifest_path(path: &str) -> Result<()> {
    let valid = !path.starts_with('/')
        && !path.contains("..")
        && (path.ends_with(".yaml") || path.ends_with(".yml"));
    if valid {
        Ok(())
    } else {
        Err(format!("invalid plugin manifest path: {path}"))
    }
}

fn validate_commit(value: &str) -> Result<()> {
    let valid = value.len() == 40
        && value.chars().all(|ch| ch.is_ascii_hexdigit())
        && value.chars().all(|ch| !ch.is_ascii_uppercase())
        && !value
            .chars()
            .next()
            .map(|first| value.chars().all(|ch| ch == first))
            .unwrap_or(true);
    if valid {
        Ok(())
    } else {
        Err("plugin Git lock commit must be a nonzero 40-character lowercase SHA".to_string())
    }
}

fn validate_sha256(value: &str) -> Result<()> {
    let valid = value.len() == 64
        && value.chars().all(|ch| ch.is_ascii_hexdigit())
        && value.chars().all(|ch| !ch.is_ascii_uppercase())
        && !value
            .chars()
            .next()
            .map(|first| value.chars().all(|ch| ch == first))
            .unwrap_or(true);
    if valid {
        Ok(())
    } else {
        Err("plugin manifestSha256 must be a 64-character lowercase hex digest".to_string())
    }
}

fn validate_manifest_digest(root: &Path, relative: &str, expected_sha: &str) -> Result<()> {
    validate_sha256(expected_sha)?;
    let path = root.join(relative);
    let raw = fs::read(&path).map_err(|error| {
        format!(
            "failed to read plugin manifest for digest {}: {error}",
            path.display()
        )
    })?;
    let actual = sha256_hex(&raw);
    if actual == expected_sha {
        Ok(())
    } else {
        Err(format!(
            "plugin manifestSha256 mismatch for {relative}: expected {expected_sha}, actual {actual}"
        ))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn read_yaml(path: PathBuf) -> Result<Value> {
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_yaml::from_str(&raw)
        .map_err(|error| format!("invalid yaml in {}: {error}", path.display()))
}

fn yaml_sequence<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Vec<Value>> {
    at_path(value, path)?
        .as_sequence()
        .ok_or_else(|| format!("value is not a sequence: {}", path.join(".")))
}

fn yaml_string(value: &Value, path: &[&str]) -> Result<String> {
    yaml_scalar_string(at_path(value, path)?)
}

fn yaml_bool(value: &Value, path: &[&str]) -> Result<bool> {
    at_path(value, path)?
        .as_bool()
        .ok_or_else(|| format!("value is not a boolean: {}", path.join(".")))
}

fn assert_yaml_bool(value: &Value, path: &[&str]) -> Result<()> {
    yaml_bool(value, path).map(|_| ())
}

fn yaml_scalar_string(value: &Value) -> Result<String> {
    value
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| "value is not a string".to_string())
}

fn assert_yaml_string(value: &Value, path: &[&str], expected: &str) -> Result<()> {
    let actual = yaml_string(value, path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "unexpected value at {}: expected {expected}, got {actual}",
            path.join(".")
        ))
    }
}

fn at_path<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Value> {
    let mut current = value;
    for key in path {
        current = current
            .get(*key)
            .ok_or_else(|| format!("missing required key: {}", path.join(".")))?;
    }
    Ok(current)
}

#[allow(dead_code)]
fn resolve_git_manifest_to_temp(locator: &str, temp_parent: &Path) -> Result<PathBuf> {
    let parsed = parse_git_locator(locator)?;
    validate_manifest_path(&parsed.manifest_path)?;
    fs::create_dir_all(temp_parent)
        .map_err(|error| format!("failed to create temp plugin dir: {error}"))?;
    let checkout = temp_parent.join("plugin");
    run_command(
        "git",
        &[
            "clone",
            "--quiet",
            "--no-checkout",
            parsed.repo.as_str(),
            checkout.to_string_lossy().as_ref(),
        ],
    )?;
    run_command(
        "git",
        &[
            "-C",
            checkout.to_string_lossy().as_ref(),
            "checkout",
            "--quiet",
            parsed.reference.as_str(),
        ],
    )?;
    let manifest = checkout.join(parsed.manifest_path);
    if manifest.is_file() {
        Ok(manifest)
    } else {
        Err(format!(
            "resolved plugin manifest does not exist: {}",
            manifest.display()
        ))
    }
}

#[allow(dead_code)]
fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!(
            "command failed: {program} {}\n{}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn external_git_locator_resolves_manifest_from_repo() {
        let root = temp_path("plugin_locator");
        let source = root.join("source");
        let checkout = root.join("checkout");
        fs::create_dir_all(&source).unwrap();
        run_command("git", &["-C", source.to_string_lossy().as_ref(), "init"]).unwrap();
        run_command(
            "git",
            &[
                "-C",
                source.to_string_lossy().as_ref(),
                "config",
                "user.email",
                "test@example.invalid",
            ],
        )
        .unwrap();
        run_command(
            "git",
            &[
                "-C",
                source.to_string_lossy().as_ref(),
                "config",
                "user.name",
                "Plugin Contract Test",
            ],
        )
        .unwrap();
        fs::write(
            source.join("plugin.yaml"),
            "kind: TradeAssemblyPluginManifest\n",
        )
        .unwrap();
        run_command(
            "git",
            &[
                "-C",
                source.to_string_lossy().as_ref(),
                "add",
                "plugin.yaml",
            ],
        )
        .unwrap();
        run_command(
            "git",
            &[
                "-C",
                source.to_string_lossy().as_ref(),
                "commit",
                "--quiet",
                "-m",
                "add plugin manifest",
            ],
        )
        .unwrap();
        let commit = run_command(
            "git",
            &["-C", source.to_string_lossy().as_ref(), "rev-parse", "HEAD"],
        )
        .unwrap();
        let locator = format!(
            "git+file://{}?ref={}#plugin.yaml",
            source.display(),
            commit.trim()
        );

        let manifest = resolve_git_manifest_to_temp(&locator, &checkout).unwrap();
        assert!(manifest.ends_with("plugin.yaml"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn locator_rejects_missing_ref_and_unsafe_manifest_path() {
        assert!(validate_locator("git+https://example.invalid/plugin.git#plugin.yaml").is_err());
        assert!(
            validate_locator("git+https://example.invalid/plugin.git?ref=main#../plugin.yaml")
                .is_err()
        );
    }

    #[test]
    fn commit_and_digest_require_immutable_hex() {
        assert!(validate_commit("40ecc87311a3534f2bb1b814eebc934caceeadd4").is_ok());
        assert!(validate_commit("0000000000000000000000000000000000000000").is_err());
        assert!(validate_commit("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_err());
        assert!(validate_sha256(
            "40ecc87311a3534f2bb1b814eebc934caceeadd4823dfc2fa3c9489eb3e51143"
        )
        .is_ok());
        assert!(validate_sha256(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        .is_err());
    }

    #[test]
    fn canonical_operation_comparison_rejects_nested_and_added_field_drift() {
        let manifest: Value = serde_yaml::from_str(
            r#"
id: plugin.capability_matrix
financeAction:
  actionId: plugin.capability_matrix
  resourceType: plugin
approval:
  mode: policy_configured
"#,
        )
        .unwrap();
        let exact = serde_json::to_value(&manifest).unwrap();
        assert!(assert_canonical_operation(
            "tradeassembly.test",
            "plugin.capability_matrix",
            &exact,
            &manifest,
        )
        .is_ok());

        let mut nested_drift = exact.clone();
        nested_drift["financeAction"]["resourceType"] = serde_json::json!("account");
        assert!(assert_canonical_operation(
            "tradeassembly.test",
            "plugin.capability_matrix",
            &nested_drift,
            &manifest,
        )
        .is_err());

        let mut added_field = exact;
        added_field["financeResourceType"] = serde_json::json!("plugin");
        assert!(assert_canonical_operation(
            "tradeassembly.test",
            "plugin.capability_matrix",
            &added_field,
            &manifest,
        )
        .is_err());
    }

    #[test]
    fn runtime_plugin_set_rejects_unbundled_defaults() {
        let runtime = vec![
            serde_json::json!({"pluginRef": "tradeassembly.simbroker"}),
            serde_json::json!({"pluginRef": "tradeassembly.local-data"}),
        ];
        let bundle = BTreeMap::from([(
            "tradeassembly.simbroker".to_string(),
            "plugin-contracts/manifests/simbroker.manifest.yaml".to_string(),
        )]);

        let error = assert_runtime_plugin_set(&runtime, &bundle).unwrap_err();
        assert!(error.contains("tradeassembly.local-data"));
    }

    #[test]
    fn operation_dependency_contract_rejects_duplicates_self_refs_and_malformed_capabilities() {
        let duplicate: Value = serde_yaml::from_str(&operation_fixture(
            r#"
  - dependencyId: bars
    capability: market_data.bars.read@1
    required: true
    traits: &dependency_traits
      modes: [backtest]
      instrumentFamilies: [equity]
      dataShapes: [bars]
      fields: [close]
      inputSchemaRefs: []
      outputSchemaRefs: [schema://market-data/bars@1]
      timeframes: [1d]
      deterministic: true
      replayable: true
  - dependencyId: bars
    capability: market_data.quote.read@1
    required: true
    traits: *dependency_traits
"#,
        ))
        .unwrap();
        assert!(validate_operation_dependencies(
            &duplicate,
            "indicator.rsi.calculate",
            "indicator.rsi.calculate@1"
        )
        .unwrap_err()
        .contains("duplicate dependency id"));

        let self_ref: Value = serde_yaml::from_str(&operation_fixture(
            r#"
  - dependencyId: itself
    capability: indicator.rsi.calculate@1
    required: true
    traits:
      modes: []
      instrumentFamilies: []
      dataShapes: []
      fields: []
      inputSchemaRefs: []
      outputSchemaRefs: []
      timeframes: []
      deterministic: false
      replayable: false
"#,
        ))
        .unwrap();
        assert!(validate_operation_dependencies(
            &self_ref,
            "indicator.rsi.calculate",
            "indicator.rsi.calculate@1"
        )
        .unwrap_err()
        .contains("self-dependency"));

        let malformed: Value = serde_yaml::from_str(&operation_fixture(
            r#"
  - dependencyId: bad
    capability: NOT-A-CAPABILITY
    required: true
    traits:
      modes: []
      instrumentFamilies: []
      dataShapes: []
      fields: []
      inputSchemaRefs: []
      outputSchemaRefs: []
      timeframes: []
      deterministic: false
      replayable: false
"#,
        ))
        .unwrap();
        assert!(validate_operation_dependencies(
            &malformed,
            "indicator.rsi.calculate",
            "indicator.rsi.calculate@1"
        )
        .is_err());
    }

    #[test]
    fn operation_traits_are_typed_and_versioned_capabilities_are_supported() {
        assert!(validate_capability_id("indicator.rsi.calculate@1").is_ok());
        assert!(validate_capability_id("indicator.rsi.calculate@1.2.3").is_ok());
        assert!(validate_capability_id("indicator.rsi.calculate@0").is_err());

        let valid: Value = serde_yaml::from_str(
            r#"
modes: [research, backtest]
instrumentFamilies: [equity, option_contract]
dataShapes: [bars]
fields: [close]
inputSchemaRefs: [schema://market-data/bars@1]
outputSchemaRefs: [schema://indicator/value@1]
timeframes: [1d]
maxFreshness: PT5M
maxLatencyMs: 500
deterministic: true
replayable: true
"#,
        )
        .unwrap();
        assert!(validate_operation_traits(&valid, "traits").is_ok());

        let mut unsupported = valid;
        unsupported["modes"] = serde_yaml::to_value(["overnight_magic"]).unwrap();
        assert!(validate_operation_traits(&unsupported, "traits").is_err());
    }

    fn operation_fixture(dependencies: &str) -> String {
        format!(
            r#"id: indicator.rsi.calculate
capability: indicator.rsi.calculate@1
dependencies:{dependencies}"#
        )
    }

    fn temp_path(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("tradeassembly-{name}-{stamp}"))
    }
}
