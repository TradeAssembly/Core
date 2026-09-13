// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

pub const SCHEMA_VERSION: &str = "tradeassembly.instrument_pack_manifest.v1";
pub const VALIDATION_SCHEMA_VERSION: &str = "tradeassembly.instrument_pack_validation.v1";
pub const COMPATIBILITY_SCHEMA_VERSION: &str = "tradeassembly.instrument_pack_compatibility.v1";
pub const NOTICE: &str =
    "Instrument packs provide identity and compatibility metadata only. Users decide strategy logic, risk scale, credentials, and activation.";
pub const MANIFEST_KEYS: &[&str] = &[
    "schemaVersion",
    "packId",
    "packVersion",
    "instrumentFamilies",
    "selectorSchemaRefs",
    "lifecycleAdapterRefs",
    "valuationAdapterRefs",
    "marginAdapterRefs",
    "providerRequirements",
    "sourceProvenance",
    "fingerprint",
    "compatibilityMetadata",
    "notice",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpJsonResponse {
    pub status_code: u16,
    pub body: Value,
}

pub fn validate_instrument_pack_manifest(payload: &Value) -> Value {
    let mut errors = Vec::<Value>::new();
    for field in [
        "packVersion",
        "lifecycleAdapterRefs",
        "valuationAdapterRefs",
        "marginAdapterRefs",
        "providerRequirements",
        "sourceProvenance",
        "compatibilityMetadata",
    ] {
        if payload.get(field).is_none() {
            errors.push(error("missing_field", &format!("$.{field}")));
        }
    }
    if payload
        .get("packId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        errors.push(error("invalid_field", "$.packId"));
    }
    match payload.get("instrumentFamilies").and_then(Value::as_array) {
        Some(items) if !items.is_empty() => validate_asset_scope(items, &mut errors),
        _ => errors.push(error("invalid_field", "$.instrumentFamilies")),
    }
    if normalized_fingerprint(payload).is_none() {
        errors.push(error("invalid_fingerprint", "$.fingerprint"));
    }
    scan_forbidden(payload, "$", &mut errors);

    if errors.is_empty() {
        let manifest =
            normalize_instrument_pack_manifest(payload).expect("validated manifest normalizes");
        validation_result(true, json!([]), manifest)
    } else {
        errors.sort_by_key(|item| {
            (
                item["code"].as_str().unwrap_or("").to_string(),
                item["path"].as_str().unwrap_or("").to_string(),
            )
        });
        validation_result(false, Value::Array(errors), Value::Null)
    }
}

pub fn normalize_instrument_pack_manifest(payload: &Value) -> Result<Value, String> {
    let validation = validate_instrument_pack_manifest_without_normalizing(payload);
    if let Some(error_path) = validation {
        return Err(error_path);
    }

    let mut families = payload["instrumentFamilies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    families.sort_by_key(|item| {
        item["assetClass"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase()
    });

    let refs = |key: &str| -> Value {
        let values = payload
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| item.as_str().map(ToOwned::to_owned))
            .collect::<BTreeSet<_>>();
        Value::Array(values.iter().map(|item| json!(item)).collect())
    };

    let mut provider_requirements = payload["providerRequirements"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    provider_requirements.sort_by_key(|item| {
        item["providerRef"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase()
    });

    Ok(json!({
        "schemaVersion": payload.get("schemaVersion").cloned().unwrap_or_else(|| json!(SCHEMA_VERSION)),
        "packId": payload["packId"].as_str().unwrap_or("").trim(),
        "packVersion": payload["packVersion"].as_str().unwrap_or(""),
        "instrumentFamilies": families,
        "selectorSchemaRefs": refs("selectorSchemaRefs"),
        "lifecycleAdapterRefs": refs("lifecycleAdapterRefs"),
        "valuationAdapterRefs": refs("valuationAdapterRefs"),
        "marginAdapterRefs": refs("marginAdapterRefs"),
        "providerRequirements": provider_requirements,
        "sourceProvenance": payload["sourceProvenance"].clone(),
        "fingerprint": normalized_fingerprint(payload).unwrap_or_default(),
        "compatibilityMetadata": canonical_json(&payload["compatibilityMetadata"]),
        "notice": NOTICE
    }))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        Value::Object(object) => {
            let mut sorted = Map::new();
            for key in object.keys().collect::<BTreeSet<_>>() {
                sorted.insert(key.clone(), canonical_json(&object[key]));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

pub fn instrument_pack_manifest_json(payload: &Value) -> Result<String, String> {
    let manifest = normalize_instrument_pack_manifest(payload)?;
    serde_json::to_string(&manifest).map_err(|error| error.to_string())
}

pub fn validation_schema_keys(_result: &Value) -> Vec<&'static str> {
    vec![
        "schemaVersion",
        "manifestSchemaVersion",
        "valid",
        "errors",
        "warnings",
        "manifest",
        "notice",
    ]
}

pub fn manifest_keys() -> &'static [&'static str] {
    MANIFEST_KEYS
}

pub fn instrument_pack_validate_http(manifest: &Value) -> HttpJsonResponse {
    HttpJsonResponse {
        status_code: 200,
        body: json!({"validation": validate_instrument_pack_manifest(manifest)}),
    }
}

pub fn instrument_pack_install_http(manifest: &Value) -> HttpJsonResponse {
    let validation = validate_instrument_pack_manifest(manifest);
    if !validation["valid"].as_bool().unwrap_or(false) {
        return HttpJsonResponse {
            status_code: 400,
            body: json!({"validation": validation}),
        };
    }
    HttpJsonResponse {
        status_code: 200,
        body: json!({
            "ref": pack_ref(manifest),
            "validation": validation
        }),
    }
}

pub fn instrument_pack_status_http(installed_manifests: &[Value]) -> HttpJsonResponse {
    let packs = installed_manifests
        .iter()
        .map(|manifest| {
            json!({
                "ref": pack_ref(manifest),
                "packId": manifest["packId"],
                "packVersion": manifest["packVersion"],
                "valid": validate_instrument_pack_manifest(manifest)["valid"]
            })
        })
        .collect::<Vec<_>>();
    HttpJsonResponse {
        status_code: 200,
        body: json!({"packs": packs}),
    }
}

pub fn provider_pack_compatibility_http(
    manifest: &Value,
    provider_refs: &[&str],
    strategy_id: &str,
) -> HttpJsonResponse {
    let providers = provider_refs
        .iter()
        .map(|provider_ref| json!({"providerRef": provider_ref, "capabilities": [], "enabled": true}))
        .collect::<Vec<_>>();

    HttpJsonResponse {
        status_code: 200,
        body: provider_pack_compatibility_report(manifest, &providers, &[], Some(strategy_id), 0),
    }
}

pub fn provider_pack_compatibility_report(
    manifest: &Value,
    providers: &[Value],
    conformance_events: &[Value],
    strategy_id: Option<&str>,
    max_conformance_age_seconds: u64,
) -> Value {
    let requirements = manifest["providerRequirements"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let provider_rows = requirements
        .iter()
        .map(|requirement| {
            let provider_ref = requirement["providerRef"].as_str().unwrap_or("");
            let provider = providers
                .iter()
                .find(|provider| provider["providerRef"].as_str() == Some(provider_ref))
                .cloned()
                .unwrap_or_else(|| json!({"providerRef": provider_ref, "capabilities": []}));
            provider_compatibility_row(
                requirement,
                &provider,
                conformance_events,
                strategy_id,
                max_conformance_age_seconds,
            )
        })
        .collect::<Vec<_>>();

    json!({
        "schemaVersion": COMPATIBILITY_SCHEMA_VERSION,
        "packRef": pack_ref(manifest),
        "strategyId": strategy_id,
        "providers": provider_rows,
        "notice": "Compatibility metadata only. Users provide credentials, strategy logic, risk scale, and activation authority."
    })
}

fn validation_result(valid: bool, errors: Value, manifest: Value) -> Value {
    json!({
        "schemaVersion": VALIDATION_SCHEMA_VERSION,
        "manifestSchemaVersion": SCHEMA_VERSION,
        "valid": valid,
        "errors": errors,
        "warnings": [],
        "manifest": manifest,
        "notice": NOTICE
    })
}

fn validate_instrument_pack_manifest_without_normalizing(payload: &Value) -> Option<String> {
    let mut errors = Vec::new();
    scan_forbidden(payload, "$", &mut errors);
    errors
        .first()
        .map(|item| item["path"].as_str().unwrap_or("$").to_string())
}

fn validate_asset_scope(families: &[Value], errors: &mut Vec<Value>) {
    let mut has_equity = false;
    let mut has_option = false;
    for family in families {
        let asset_class = family["assetClass"].as_str().unwrap_or("");
        let executable = family["executable"].as_bool().unwrap_or(false);
        if asset_class == "equity" && executable {
            has_equity = true;
        }
        if asset_class == "option" && executable {
            has_option = true;
        }
        if asset_class == "crypto" && executable {
            errors.push(error(
                "placeholder_only_asset_class",
                "$.instrumentFamilies",
            ));
        }
        if !["crypto", "equity", "option"].contains(&asset_class) {
            errors.push(error("deferred_asset_class", "$.instrumentFamilies"));
        }
    }
    if !has_equity || !has_option {
        errors.push(error("missing_executable_family", "$.instrumentFamilies"));
    }
}

fn scan_forbidden(value: &Value, path: &str, errors: &mut Vec<Value>) {
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            let child_path = format!("{path}.{key}");
            match key.as_str() {
                "hostedOAuth" | "billingPlan" | "payoutRail" => {
                    errors.push(error("public_core_boundary", blocked_path(path)))
                }
                "api_secret" | "apiKey" | "clientSecret" => {
                    errors.push(error("secret_material", blocked_path(path)))
                }
                "privatePackage" => errors.push(error("private_package", blocked_path(path))),
                "brokerCredentialCustody" => {
                    errors.push(error("credential_custody", blocked_path(path)))
                }
                "recommendTrade" | "defaultRiskScale" => {
                    errors.push(error("trade_boundary", blocked_path(path)))
                }
                "predictivePerformance" => {
                    errors.push(error("predictive_claim", blocked_path(path)))
                }
                _ => scan_forbidden(child, &child_path, errors),
            }
        }
    } else if let Some(array) = value.as_array() {
        for child in array {
            scan_forbidden(child, path, errors);
        }
    }
}

fn blocked_path(path: &str) -> &str {
    if path == "$" {
        "$.<blocked>"
    } else if path.contains("sourceProvenance") {
        "$.sourceProvenance.<blocked>"
    } else if path.contains("providerRequirements") {
        "$.providerRequirements.<blocked>"
    } else if path.contains("compatibilityMetadata") {
        "$.compatibilityMetadata.<blocked>"
    } else {
        "$.<blocked>"
    }
}

fn normalized_fingerprint(payload: &Value) -> Option<String> {
    let raw = payload
        .get("fingerprint")
        .or_else(|| payload.get("checksum"))?
        .as_str()?;
    let lower = raw.to_ascii_lowercase();
    let hex = lower.strip_prefix("sha256:").unwrap_or(&lower);
    if hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(format!("sha256:{hex}"))
    } else {
        None
    }
}

fn pack_ref(manifest: &Value) -> String {
    format!(
        "{}@{}",
        manifest["packId"].as_str().unwrap_or("unknown"),
        manifest["packVersion"].as_str().unwrap_or("unknown")
    )
}

fn provider_compatibility_row(
    requirement: &Value,
    provider: &Value,
    conformance_events: &[Value],
    strategy_id: Option<&str>,
    max_conformance_age_seconds: u64,
) -> Value {
    let provider_ref = requirement["providerRef"].as_str().unwrap_or("");
    let required_operations = requirement["capabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| item.as_str().map(required_operation))
        .collect::<BTreeSet<_>>();
    let provider_capabilities = value_string_set(&provider["capabilities"]);
    let explicit_unsupported = value_string_set(&provider["unsupportedOperations"]);
    let mut missing_capabilities = required_operations
        .iter()
        .filter(|operation| !provider_capabilities.contains(*operation))
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut unsupported_operations = required_operations
        .intersection(&explicit_unsupported)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut warnings = Vec::<String>::new();

    if !unsupported_operations.is_empty() {
        warnings.push("provider_explicitly_unsupported".to_string());
    }

    let asset_classes = value_string_set(&requirement["assetClasses"]);
    let mut missing_asset_classes = BTreeSet::<String>::new();
    if asset_classes.contains("option")
        && !provider_capabilities.contains("marketdata.option_chain")
    {
        missing_asset_classes.insert("option".to_string());
    }

    let scope = provider_strategy_scope(provider, strategy_id);
    if !scope.0 {
        warnings.push("strategy_scope_missing".to_string());
    }
    let credential = provider_credential_handle(provider);
    if credential["status"] == "missing" {
        warnings.push("credential_handle_missing".to_string());
    }

    let conformance = provider_conformance(
        provider_ref,
        conformance_events,
        max_conformance_age_seconds,
    );
    match conformance["status"].as_str().unwrap_or("") {
        "failed" => {
            warnings.push("conformance_failed".to_string());
            for operation in conformance["failedOperations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                if let Some(operation) = operation.as_str() {
                    unsupported_operations.insert(required_operation(operation));
                }
            }
        }
        "stale" => warnings.push("conformance_stale".to_string()),
        "missing" => warnings.push("conformance_missing".to_string()),
        _ => {}
    }
    for operation in &unsupported_operations {
        missing_capabilities.remove(operation);
    }

    let supported_operations = required_operations
        .difference(&missing_capabilities)
        .filter(|operation| !unsupported_operations.contains(*operation))
        .cloned()
        .collect::<Vec<_>>();
    let status = if !missing_capabilities.is_empty()
        || !missing_asset_classes.is_empty()
        || !unsupported_operations.is_empty()
        || !scope.0
        || credential["status"] == "missing"
        || conformance["status"] == "failed"
    {
        "unsupported"
    } else if warnings
        .iter()
        .any(|warning| warning == "conformance_stale" || warning == "conformance_missing")
    {
        "warning"
    } else {
        "supported"
    };

    json!({
        "providerRef": provider_ref,
        "status": status,
        "supportedOperations": supported_operations,
        "missingCapabilities": missing_capabilities.into_iter().collect::<Vec<_>>(),
        "missingAssetClasses": missing_asset_classes.into_iter().collect::<Vec<_>>(),
        "unsupportedOperations": unsupported_operations.into_iter().collect::<Vec<_>>(),
        "strategyScope": {
            "authorized": scope.0,
            "policy": scope.1,
        },
        "credentialHandle": credential,
        "conformance": conformance,
        "warnings": warnings,
        "provenance": {
            "rawSecretsExposed": false,
            "providerFingerprint": provider["fingerprint"].as_str().unwrap_or("")
        }
    })
}

fn provider_strategy_scope(provider: &Value, strategy_id: Option<&str>) -> (bool, String) {
    let policy = provider["strategyScopePolicy"]
        .as_str()
        .unwrap_or("unscoped")
        .to_string();
    if policy != "scoped" {
        return (true, policy);
    }
    let allowed = strategy_id
        .map(|strategy_id| value_string_set(&provider["strategyScope"]).contains(strategy_id))
        .unwrap_or(false);
    (allowed, policy)
}

fn provider_credential_handle(provider: &Value) -> Value {
    if !provider["localCredentials"].as_bool().unwrap_or(false) {
        return json!({"status": "not_required", "storesHandlesOnly": true, "rawSecretsExposed": false});
    }
    if provider["credentialRef"].as_str().is_some() {
        json!({"status": "configured", "storesHandlesOnly": true, "rawSecretsExposed": false})
    } else {
        json!({"status": "missing", "storesHandlesOnly": true, "rawSecretsExposed": false})
    }
}

fn provider_conformance(
    provider_ref: &str,
    conformance_events: &[Value],
    max_conformance_age_seconds: u64,
) -> Value {
    let event = conformance_events.iter().find(|event| {
        event["payload"]["probes"]
            .as_array()
            .map(|probes| {
                probes
                    .iter()
                    .any(|probe| probe["provider_ref"].as_str() == Some(provider_ref))
            })
            .unwrap_or(false)
    });
    let Some(event) = event else {
        return json!({"status": "missing", "failedOperations": []});
    };
    let age = event["ageSeconds"].as_u64().unwrap_or(0);
    if max_conformance_age_seconds > 0 && age > max_conformance_age_seconds {
        return json!({"status": "stale", "ageSeconds": age, "failedOperations": []});
    }
    let failed_operations = event["payload"]["probes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|probe| probe["operations"].as_array().cloned().unwrap_or_default())
        .filter(|operation| operation["status"].as_str() == Some("failed"))
        .filter_map(|operation| operation["operation"].as_str().map(required_operation))
        .collect::<Vec<_>>();
    if !failed_operations.is_empty() || !event["payload"]["ok"].as_bool().unwrap_or(false) {
        json!({"status": "failed", "failedOperations": failed_operations})
    } else {
        json!({"status": "current", "failedOperations": []})
    }
}

fn required_operation(capability: &str) -> String {
    match capability {
        "options.chain" | "option_chain" => "marketdata.option_chain".to_string(),
        "bars" => "marketdata.bars".to_string(),
        other => other.to_string(),
    }
}

fn value_string_set(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
        .collect()
}

fn error(code: &str, path: &str) -> Value {
    json!({"code": code, "path": path})
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    fn valid_manifest() -> Value {
        json!({
            "schemaVersion": super::SCHEMA_VERSION,
            "packId": "core.equity-options",
            "packVersion": "2026.06.0",
            "instrumentFamilies": [
                {"assetClass": "option", "instrumentFamily": "equity_option", "executable": true},
                {"assetClass": "equity", "instrumentFamily": "listed_equity", "executable": true},
                {"assetClass": "crypto", "instrumentFamily": "spot_crypto", "executable": false, "metadata": {"status": "placeholder"}}
            ],
            "selectorSchemaRefs": ["selector.option.v1", "selector.equity.v1"],
            "lifecycleAdapterRefs": ["lifecycle.equity_option.v1"],
            "valuationAdapterRefs": ["valuation.black_scholes.v1", "valuation.equity_mark.v1"],
            "marginAdapterRefs": ["margin.reg_t.v1"],
            "providerRequirements": [
                {
                    "providerRef": "alpaca-paper",
                    "capabilities": ["options.chain", "marketdata.bars"],
                    "assetClasses": ["equity", "option"]
                },
                {
                    "providerRef": "local-data",
                    "capabilities": ["instrument.resolve"],
                    "assetClasses": ["equity"]
                }
            ],
            "sourceProvenance": {
                "source": "local-fixture",
                "observedAt": "2026-06-23T00:00:00Z",
                "files": [{"path": "packs/equity-options.json", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]
            },
            "fingerprint": "SHA256:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
            "compatibilityMetadata": {
                "requires": {"providerCapabilityMap": "tradeassembly.provider_capability_map.v1"},
                "v0": true
            }
        })
    }

    #[test]
    fn valid_equity_options_manifest_normalizes_to_public_payload() {
        let result = super::validate_instrument_pack_manifest(&valid_manifest());

        assert_eq!(result["schemaVersion"], super::VALIDATION_SCHEMA_VERSION);
        assert_eq!(result["manifestSchemaVersion"], super::SCHEMA_VERSION);
        assert_eq!(result["valid"], true);
        assert_eq!(result["errors"], json!([]));
        assert_eq!(result["warnings"], json!([]));
        assert_eq!(result["notice"], super::NOTICE);

        let manifest = &result["manifest"];
        assert_eq!(
            manifest["fingerprint"],
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(
            manifest["selectorSchemaRefs"],
            json!(["selector.equity.v1", "selector.option.v1"])
        );
        assert_eq!(
            manifest["instrumentFamilies"]
                .as_array()
                .expect("families")
                .iter()
                .map(|item| item["assetClass"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["crypto", "equity", "option"]
        );
        assert_eq!(manifest["instrumentFamilies"][0]["executable"], false);
        assert!(manifest["notice"]
            .as_str()
            .expect("notice")
            .contains("Users decide strategy logic"));

        let rendered = serde_json::to_string(&result).unwrap().to_lowercase();
        assert!(!rendered.contains("api_secret"));
        assert!(!rendered.contains("recommend trade"));
    }

    #[test]
    fn missing_required_fields_and_bad_shapes_return_stable_errors() {
        let result = super::validate_instrument_pack_manifest(&json!({
            "schemaVersion": super::SCHEMA_VERSION,
            "packId": "",
            "instrumentFamilies": [],
            "selectorSchemaRefs": ["selector.option.v1"],
            "fingerprint": "not-a-fingerprint"
        }));

        assert_eq!(result["valid"], false);
        assert_eq!(result["manifest"], Value::Null);
        let mut errors = result["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["code"].as_str().unwrap().to_string(),
                    item["path"].as_str().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>();
        errors.sort();
        assert_eq!(
            errors,
            vec![
                (
                    "invalid_field".to_string(),
                    "$.instrumentFamilies".to_string()
                ),
                ("invalid_field".to_string(), "$.packId".to_string()),
                (
                    "invalid_fingerprint".to_string(),
                    "$.fingerprint".to_string()
                ),
                (
                    "missing_field".to_string(),
                    "$.compatibilityMetadata".to_string()
                ),
                (
                    "missing_field".to_string(),
                    "$.lifecycleAdapterRefs".to_string()
                ),
                (
                    "missing_field".to_string(),
                    "$.marginAdapterRefs".to_string()
                ),
                ("missing_field".to_string(), "$.packVersion".to_string()),
                (
                    "missing_field".to_string(),
                    "$.providerRequirements".to_string()
                ),
                (
                    "missing_field".to_string(),
                    "$.sourceProvenance".to_string()
                ),
                (
                    "missing_field".to_string(),
                    "$.valuationAdapterRefs".to_string()
                ),
            ]
        );
    }

    #[test]
    fn forbidden_declarations_fail_closed_without_echoing_values() {
        for (patch, expected_code) in [
            (
                json!({"providerRequirements": [{"hostedOAuth": {"clientSecret": "SUPER-SECRET-VALUE"}}]}),
                "public_core_boundary",
            ),
            (
                json!({"providerRequirements": [{"api_secret": "SUPER-SECRET-VALUE"}]}),
                "secret_material",
            ),
            (
                json!({"providerRequirements": [{"billingPlan": "pro"}]}),
                "public_core_boundary",
            ),
            (
                json!({"providerRequirements": [{"payoutRail": "platform"}]}),
                "public_core_boundary",
            ),
            (
                json!({"providerRequirements": [{"privatePackage": "git@github.com:owner/private.git"}]}),
                "private_package",
            ),
            (
                json!({"providerRequirements": [{"brokerCredentialCustody": true}]}),
                "credential_custody",
            ),
            (
                json!({"compatibilityMetadata": {"recommendTrade": true}}),
                "trade_boundary",
            ),
            (
                json!({"compatibilityMetadata": {"defaultRiskScale": 0.02}}),
                "trade_boundary",
            ),
            (
                json!({"compatibilityMetadata": {"predictivePerformance": "high confidence"}}),
                "predictive_claim",
            ),
        ] {
            let mut payload = valid_manifest();
            merge_object(&mut payload, patch);

            let result = super::validate_instrument_pack_manifest(&payload);

            assert_eq!(result["valid"], false);
            assert!(result["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["code"] == expected_code));
            let rendered = serde_json::to_string(&result).unwrap();
            assert!(!rendered.contains("SUPER-SECRET-VALUE"));
            assert!(!rendered.contains("high confidence"));
            assert!(!rendered.contains("recommendTrade"));
        }
    }

    #[test]
    fn v0_asset_scope_rejects_executable_crypto_and_deferred_assets() {
        let mut payload = valid_manifest();
        payload["instrumentFamilies"] = json!([
            {"assetClass": "crypto", "instrumentFamily": "spot_crypto", "executable": true},
            {"assetClass": "future", "instrumentFamily": "equity_index_future", "executable": true}
        ]);

        let result = super::validate_instrument_pack_manifest(&payload);
        let codes = result["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["code"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert!(!result["valid"].as_bool().unwrap());
        assert!(codes.contains(&"placeholder_only_asset_class"));
        assert!(codes.contains(&"deferred_asset_class"));
        assert!(codes.contains(&"missing_executable_family"));
    }

    #[test]
    fn checksum_alias_and_deterministic_normalization_match() {
        let left = valid_manifest();
        let mut right = valid_manifest();
        right.as_object_mut().unwrap().remove("fingerprint");
        right["checksum"] =
            json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        right["selectorSchemaRefs"] = json!([
            "selector.option.v1",
            "selector.equity.v1",
            "selector.option.v1"
        ]);
        right["providerRequirements"] = json!([
            {"providerRef": "local-data", "capabilities": ["instrument.resolve"], "assetClasses": ["equity"]},
            {"providerRef": "alpaca-paper", "capabilities": ["options.chain", "marketdata.bars"], "assetClasses": ["equity", "option"]}
        ]);
        right["compatibilityMetadata"] = json!({"v0": true, "requires": {"providerCapabilityMap": "tradeassembly.provider_capability_map.v1"}});

        assert_eq!(
            super::instrument_pack_manifest_json(&left).unwrap(),
            super::instrument_pack_manifest_json(&right).unwrap()
        );
        assert_eq!(
            super::validate_instrument_pack_manifest(&right)["manifest"]["fingerprint"],
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
    }

    #[test]
    fn normalize_errors_point_at_blocked_secret_surfaces() {
        let mut payload = valid_manifest();
        payload["sourceProvenance"] = json!({"apiKey": "abc123"});

        let error = super::normalize_instrument_pack_manifest(&payload).unwrap_err();

        assert!(error.contains("$.sourceProvenance.<blocked>"));
        assert!(!error.contains("abc123"));
    }

    #[test]
    fn validation_schema_keys_remain_stable() {
        let result = super::validate_instrument_pack_manifest(&valid_manifest());

        assert_eq!(
            super::validation_schema_keys(&result),
            vec![
                "schemaVersion",
                "manifestSchemaVersion",
                "valid",
                "errors",
                "warnings",
                "manifest",
                "notice",
            ]
        );
        assert_eq!(super::manifest_keys(), super::MANIFEST_KEYS);
    }

    #[test]
    fn instrument_pack_http_status_and_compatibility_are_public_safe() {
        let manifest = local_only_manifest();

        let validation = super::instrument_pack_validate_http(&manifest);
        assert_eq!(validation.status_code, 200);
        assert_eq!(validation.body["validation"]["valid"], true);

        let installed = super::instrument_pack_install_http(&manifest);
        assert_eq!(installed.status_code, 200);
        assert_eq!(installed.body["ref"], "core.equity-options@2026.06.0");

        let status = super::instrument_pack_status_http(std::slice::from_ref(&manifest));
        assert_eq!(status.status_code, 200);
        assert_eq!(
            status.body["packs"][0]["ref"],
            "core.equity-options@2026.06.0"
        );

        let compatibility =
            super::provider_pack_compatibility_http(&manifest, &["local-data"], "strat_alpha");
        assert_eq!(compatibility.status_code, 200);
        assert_eq!(
            compatibility.body["packRef"],
            "core.equity-options@2026.06.0"
        );
        assert_eq!(
            compatibility.body["providers"][0]["providerRef"],
            "local-data"
        );
        let rendered = serde_json::to_string(&compatibility.body).unwrap();
        assert!(!rendered
            .to_lowercase()
            .contains("never tells users what to trade"));
        assert!(!rendered.contains("SUPER-SECRET-VALUE"));
    }

    #[test]
    fn provider_pack_compatibility_reports_supported_provider_without_secret_provenance() {
        let payload = local_only_manifest();
        let manifest = super::validate_instrument_pack_manifest(&payload)["manifest"].clone();
        let providers = vec![
            provider(
                "local-data",
                &[
                    "marketdata.bars",
                    "marketdata.option_chain",
                    "marketdata.quotes",
                ],
                false,
                None,
                &[],
                "unscoped",
                &[],
            ),
            provider(
                "alpaca-paper",
                &["marketdata.bars", "broker.paper"],
                true,
                Some("cred_alpaca"),
                &[],
                "unscoped",
                &[],
            ),
        ];
        let events = vec![conformance_event("local-data", "passed", None, 0)];

        let report = super::provider_pack_compatibility_report(
            &manifest,
            &providers,
            &events,
            Some("strat_alpha"),
            60 * 60 * 24,
        );

        let local = &report["providers"][0];
        assert_eq!(report["schemaVersion"], super::COMPATIBILITY_SCHEMA_VERSION);
        assert_eq!(report["packRef"], "core.equity-options@2026.06.0");
        assert_eq!(local["providerRef"], "local-data");
        assert_eq!(local["status"], "supported");
        assert_eq!(
            local["supportedOperations"],
            json!(["marketdata.bars", "marketdata.option_chain"])
        );
        assert_eq!(local["missingCapabilities"], json!([]));
        assert_eq!(local["unsupportedOperations"], json!([]));
        assert_eq!(local["strategyScope"]["authorized"], true);
        assert_eq!(local["credentialHandle"]["status"], "not_required");
        assert_eq!(local["conformance"]["status"], "current");
        assert_eq!(local["provenance"]["rawSecretsExposed"], false);
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("SUPER-SECRET-VALUE"));
    }

    #[test]
    fn provider_pack_compatibility_fails_closed_for_missing_capability_scope_and_credential() {
        let mut payload = valid_manifest();
        payload["providerRequirements"][0]["providerRef"] = json!("alpaca-paper");
        let manifest = super::validate_instrument_pack_manifest(&payload)["manifest"].clone();
        let providers = vec![provider(
            "alpaca-paper",
            &["marketdata.bars", "broker.paper"],
            true,
            None,
            &[],
            "scoped",
            &["strat_other"],
        )];
        let events = vec![conformance_event("alpaca-paper", "passed", None, 0)];

        let report = super::provider_pack_compatibility_report(
            &manifest,
            &providers,
            &events,
            Some("strat_alpha"),
            60 * 60 * 24,
        );

        let alpaca = &report["providers"][0];
        assert_eq!(alpaca["status"], "unsupported");
        assert!(alpaca["missingCapabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("marketdata.option_chain")));
        assert!(alpaca["missingAssetClasses"]
            .as_array()
            .unwrap()
            .contains(&json!("option")));
        assert_eq!(alpaca["strategyScope"]["authorized"], false);
        assert_eq!(alpaca["credentialHandle"]["status"], "missing");
        assert!(alpaca["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("strategy_scope_missing")));
        assert!(alpaca["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("credential_handle_missing")));
    }

    #[test]
    fn conformance_status_only_narrows_support_never_widens() {
        let manifest =
            super::validate_instrument_pack_manifest(&local_only_manifest())["manifest"].clone();
        let providers = vec![provider(
            "local-data",
            &["marketdata.bars", "marketdata.option_chain"],
            false,
            None,
            &[],
            "unscoped",
            &[],
        )];

        let failed = super::provider_pack_compatibility_report(
            &manifest,
            &providers,
            &[conformance_event(
                "local-data",
                "passed",
                Some("option_chain"),
                0,
            )],
            None,
            60 * 60 * 24,
        );
        let stale = super::provider_pack_compatibility_report(
            &manifest,
            &providers,
            &[conformance_event(
                "local-data",
                "passed",
                None,
                60 * 60 * 24 * 4,
            )],
            None,
            60,
        );
        let missing =
            super::provider_pack_compatibility_report(&manifest, &providers, &[], None, 60);

        assert_eq!(failed["providers"][0]["status"], "unsupported");
        assert!(failed["providers"][0]["unsupportedOperations"]
            .as_array()
            .unwrap()
            .contains(&json!("marketdata.option_chain")));
        assert_eq!(failed["providers"][0]["conformance"]["status"], "failed");
        assert_eq!(stale["providers"][0]["status"], "warning");
        assert_eq!(stale["providers"][0]["conformance"]["status"], "stale");
        assert!(stale["providers"][0]["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("conformance_stale")));
        assert_eq!(missing["providers"][0]["status"], "warning");
        assert_eq!(missing["providers"][0]["conformance"]["status"], "missing");
        assert!(missing["providers"][0]["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("conformance_missing")));
    }

    #[test]
    fn explicit_unsupported_operation_narrows_advertised_support() {
        let manifest =
            super::validate_instrument_pack_manifest(&local_only_manifest())["manifest"].clone();
        let providers = vec![provider(
            "local-data",
            &["marketdata.bars", "marketdata.option_chain"],
            false,
            None,
            &["marketdata.option_chain"],
            "unscoped",
            &[],
        )];
        let events = vec![conformance_event("local-data", "passed", None, 0)];

        let report = super::provider_pack_compatibility_report(
            &manifest,
            &providers,
            &events,
            None,
            60 * 60 * 24,
        );

        let local = &report["providers"][0];
        assert_eq!(local["status"], "unsupported");
        assert_eq!(local["supportedOperations"], json!(["marketdata.bars"]));
        assert_eq!(
            local["unsupportedOperations"],
            json!(["marketdata.option_chain"])
        );
        assert!(local["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("provider_explicitly_unsupported")));
    }

    fn merge_object(target: &mut Value, patch: Value) {
        let target = target.as_object_mut().expect("target object");
        for (key, value) in patch.as_object().expect("patch object") {
            target.insert(key.clone(), value.clone());
        }
    }

    fn local_only_manifest() -> Value {
        let mut payload = valid_manifest();
        payload["providerRequirements"] = json!([
            {"providerRef": "local-data", "capabilities": ["options.chain", "marketdata.bars"], "assetClasses": ["equity", "option"]}
        ]);
        payload
    }

    fn provider(
        provider_ref: &str,
        capabilities: &[&str],
        local_credentials: bool,
        credential_ref: Option<&str>,
        unsupported_operations: &[&str],
        strategy_scope_policy: &str,
        strategy_scope: &[&str],
    ) -> Value {
        json!({
            "providerRef": provider_ref,
            "capabilities": capabilities,
            "localCredentials": local_credentials,
            "credentialRef": credential_ref,
            "unsupportedOperations": unsupported_operations,
            "strategyScopePolicy": strategy_scope_policy,
            "strategyScope": strategy_scope,
            "fingerprint": format!("sha256:{provider_ref}"),
            "enabled": true
        })
    }

    fn conformance_event(
        provider_ref: &str,
        status: &str,
        failed_operation: Option<&str>,
        age_seconds: u64,
    ) -> Value {
        let mut operations = vec![
            json!({"operation": "bars", "status": "passed", "errors": [], "provider_ref": provider_ref}),
            json!({"operation": "option_chain", "status": "passed", "errors": [], "provider_ref": provider_ref}),
        ];
        if let Some(failed_operation) = failed_operation {
            for operation in &mut operations {
                if operation["operation"] == failed_operation {
                    operation["status"] = json!("failed");
                    operation["errors"] = json!(["runtime probe failed"]);
                }
            }
        }
        json!({
            "eventType": "marketdata.conformance.checked",
            "ageSeconds": age_seconds,
            "payload": {
                "ok": status == "passed" && failed_operation.is_none(),
                "probes": [{"provider_ref": provider_ref, "status": status, "operations": operations}],
                "failures": []
            }
        })
    }
}
