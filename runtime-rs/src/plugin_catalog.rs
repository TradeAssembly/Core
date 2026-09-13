use crate::ports::{
    CredentialResolutionStatus, CredentialStatus, PluginCatalogEntry, PluginOperationContract,
};
use serde_json::Value;

pub fn stored_plugin_entry(
    instance: &Value,
    manifest_record: &Value,
    credential_status: &CredentialStatus,
    health: String,
) -> Option<PluginCatalogEntry> {
    let manifest = manifest_record.get("manifest")?;
    let instance_ref = string_field(instance, &["instanceRef"])?;
    let plugin_ref = string_field(instance, &["pluginRef"])?;
    let provider_ref = string_field(instance, &["providerRef"]).or_else(|| {
        manifest["metadata"]["provider"]["id"]
            .as_str()
            .map(str::to_string)
    })?;
    Some(PluginCatalogEntry {
        instance_ref,
        plugin_ref,
        provider_ref,
        account_ref: string_field(instance, &["accountRef"]),
        name: manifest["metadata"]["name"]
            .as_str()
            .unwrap_or("Installed plugin")
            .to_string(),
        enabled: instance["enabled"].as_bool().unwrap_or(false),
        trust_level: manifest_record["trust"]["level"]
            .as_str()
            .unwrap_or("external-locked")
            .to_string(),
        manifest_fingerprint: manifest_record["manifestDigest"].as_str()?.to_string(),
        capabilities: manifest["capabilities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|capability| capability["id"].as_str().map(str::to_string))
            .collect(),
        operations: manifest["operations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(operation_from_manifest)
            .collect(),
        credential_status: CredentialResolutionStatus {
            configured: credential_status.configured,
            custody: credential_status.custody.clone(),
            status: if credential_status.configured {
                "stored"
            } else {
                "missing"
            }
            .to_string(),
            revision_ref: instance["credentialStatus"]["revisionRef"]
                .as_str()
                .map(str::to_string),
        },
        health,
    })
}

pub fn operation_from_manifest(operation: &Value) -> Option<PluginOperationContract> {
    Some(PluginOperationContract {
        id: string_field(operation, &["id"])?,
        capability: string_field(operation, &["capability"])?,
        protocol: string_field(operation, &["protocol"]).unwrap_or_else(|| "stdio".to_string()),
        resource_type: string_field(operation, &["resourceType"])
            .unwrap_or_else(|| "plugin".to_string()),
        finance_resource_type: string_field(&operation["financeAction"], &["resourceType"])
            .or_else(|| string_field(operation, &["resourceType"]))
            .unwrap_or_else(|| "plugin".to_string()),
        effect: string_field(operation, &["effect"]).unwrap_or_else(|| "read".to_string()),
        risk: string_field(operation, &["riskClass"]).unwrap_or_else(|| "low".to_string()),
        credential_grant_required: operation["credentialGrantRequired"]
            .as_bool()
            .unwrap_or(false),
        account_binding_required: operation["accountBindingRequired"]
            .as_bool()
            .unwrap_or(false),
        apf_action_id: string_field(&operation["financeAction"], &["actionId"])
            .or_else(|| string_field(operation, &["id"]))?,
        mandate_required: operation["mandateRequired"].as_bool().unwrap_or(false),
        purpose: string_field(operation, &["purpose"]).unwrap_or_else(|| "plugin_use".to_string()),
        evidence: string_array_field(operation, &["evidence"]),
        pep_coverage_class: string_field(operation, &["pepCoverage"]).unwrap_or_default(),
        check_packs: string_array_field(operation, &["checkPacks"]),
        receipt_class: string_field(operation, &["receiptClass"]).unwrap_or_default(),
        redaction: string_field(operation, &["redaction"]).unwrap_or_else(|| "hash_or_ref".into()),
        no_advice: operation["noAdvice"].as_bool().unwrap_or(true),
        session_admission: string_field(&operation["sessionAdmission"], &["mode"])
            .unwrap_or_default(),
        approval_mode: string_field(&operation["approval"], &["mode"]).unwrap_or_default(),
        supervision_mode: string_field(&operation["supervision"], &["mode"]).unwrap_or_default(),
        detectors: string_array_field(operation, &["detectors"]),
        context_providers: string_array_field(operation, &["contextProviders"]),
        policy_refs: string_array_field(operation, &["policyRefs"]),
        traits: serde_json::from_value(operation["traits"].clone()).unwrap_or_default(),
        dependencies: serde_json::from_value(operation["dependencies"].clone()).unwrap_or_default(),
        supported_protocols: string_array_field(operation, &["supportedProtocols"]),
    })
}

fn value_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key))
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    value_field(value, keys)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn string_array_field(value: &Value, keys: &[&str]) -> Vec<String> {
    value_field(value, keys)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
