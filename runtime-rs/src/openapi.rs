// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApiManifestError {
    message: String,
}

impl OpenApiManifestError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OpenApiManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for OpenApiManifestError {}

pub fn compile_manifest_from_sources(
    spec_uri: impl AsRef<Path>,
    mapping_uri: impl AsRef<Path>,
) -> Result<Value, OpenApiManifestError> {
    let spec_text = fs::read_to_string(spec_uri.as_ref()).map_err(|error| {
        OpenApiManifestError::new(format!("failed to read OpenAPI spec: {error}"))
    })?;
    let mapping_text = fs::read_to_string(mapping_uri.as_ref())
        .map_err(|error| OpenApiManifestError::new(format!("failed to read mapping: {error}")))?;
    let spec = serde_json::from_str::<Value>(&spec_text)
        .map_err(|error| OpenApiManifestError::new(format!("invalid OpenAPI JSON: {error}")))?;
    let mapping = serde_json::from_str::<Value>(&mapping_text)
        .map_err(|error| OpenApiManifestError::new(format!("invalid mapping JSON: {error}")))?;

    let manifest_identity = mapping
        .get("manifest")
        .and_then(Value::as_object)
        .ok_or_else(|| OpenApiManifestError::new("manifest.name/version are required"))?;
    let name = required_string(manifest_identity, "name")?;
    let version = required_string(manifest_identity, "version")?;
    let kind = manifest_identity
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("marketdata");
    let base_url = manifest_identity
        .get("base_url")
        .and_then(Value::as_str)
        .or_else(|| {
            spec.get("servers")
                .and_then(Value::as_array)
                .and_then(|servers| servers.first())
                .and_then(|server| server.get("url"))
                .and_then(Value::as_str)
        })
        .unwrap_or("");
    let operations = openapi_operations(&spec);
    let operation_ids = operations.keys().cloned().collect::<Vec<_>>();
    let profile_capabilities = mapping
        .get("profile_capabilities")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let mut capabilities = Map::new();
    capabilities.insert(
        "http".to_string(),
        json!({
            "base_url": base_url,
            "operations": operations,
        }),
    );
    if let Some(indicators) = profile_capabilities.get("indicators") {
        capabilities.insert("indicators".to_string(), indicators.clone());
    }
    capabilities.insert(
        "openapi_inventory".to_string(),
        json!({
            "operation_ids": operation_ids,
            "all_operation_ids": operation_ids,
            "spec_sha256": sha256_hex(&spec_text),
            "mapping_sha256": sha256_hex(&mapping_text),
        }),
    );
    let manifest_body = json!({
        "name": name,
        "version": version,
        "kind": kind,
        "capabilities": capabilities,
    });
    let manifest_hash = sha256_hex(&canonical_json(&manifest_body));
    let mut manifest = manifest_body;
    manifest
        .as_object_mut()
        .expect("manifest object")
        .insert("hash".to_string(), json!(manifest_hash));
    Ok(manifest)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, OpenApiManifestError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| OpenApiManifestError::new("manifest.name/version are required"))
}

fn openapi_operations(spec: &Value) -> Map<String, Value> {
    let mut operations = Map::new();
    let Some(paths) = spec.get("paths").and_then(Value::as_object) else {
        return operations;
    };
    for path_item in paths.values() {
        let Some(methods) = path_item.as_object() else {
            continue;
        };
        for operation in methods.values() {
            let Some(operation_id) = operation.get("operationId").and_then(Value::as_str) else {
                continue;
            };
            let query = operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|parameter| parameter.get("in").and_then(Value::as_str) == Some("query"))
                .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                .map(|name| (name.to_string(), json!(format!("{{{name}}}"))))
                .collect::<Map<_, _>>();
            operations.insert(operation_id.to_string(), json!({ "query": query }));
        }
    }
    operations
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    hex_lower(&digest)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let parts = keys
                .into_iter()
                .map(|key| format!("{:?}:{}", key, canonical_json(&map[key])))
                .collect::<Vec<_>>();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn compile_manifest_from_local_openapi_sources_records_hashes() {
        let root = temp_dir("openapi-manifest-ok");
        let spec_path = root.join("openapi.json");
        let mapping_path = root.join("mapping.json");
        fs::write(
            &spec_path,
            json!({
                "openapi": "3.1.0",
                "servers": [{"url": "https://api.example.test"}],
                "paths": {
                    "/bars": {
                        "get": {
                            "operationId": "StockBars",
                            "parameters": [{"name": "symbol", "in": "query", "required": true, "schema": {"type": "string"}}],
                        }
                    }
                },
            })
            .to_string(),
        )
        .expect("write spec");
        fs::write(
            &mapping_path,
            json!({
                "manifest": {"name": "ExampleData", "version": "1.0.0", "kind": "marketdata", "base_url": "https://override.example.test"},
                "profile_capabilities": {"indicators": {"supported": ["sma"]}},
            })
            .to_string(),
        )
        .expect("write mapping");

        let manifest = compile_manifest_from_sources(&spec_path, &mapping_path).expect("manifest");
        let inventory = &manifest["capabilities"]["openapi_inventory"];

        assert_eq!(manifest["name"], "ExampleData");
        assert_eq!(
            manifest["capabilities"]["http"]["base_url"],
            "https://override.example.test"
        );
        assert_eq!(
            manifest["capabilities"]["http"]["operations"]["StockBars"]["query"],
            json!({"symbol": "{symbol}"})
        );
        assert_eq!(
            manifest["capabilities"]["indicators"]["supported"],
            json!(["sma"])
        );
        assert_eq!(inventory["operation_ids"], json!(["StockBars"]));
        assert_eq!(inventory["all_operation_ids"], json!(["StockBars"]));
        assert_eq!(inventory["spec_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(inventory["mapping_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(manifest["hash"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn compile_manifest_from_sources_requires_mapping_identity() {
        let root = temp_dir("openapi-manifest-invalid");
        let spec_path = root.join("openapi.json");
        let mapping_path = root.join("mapping.json");
        fs::write(
            &spec_path,
            json!({"openapi": "3.1.0", "servers": [{"url": "https://api.example.test"}], "paths": {"/health": {"get": {"operationId": "health"}}}}).to_string(),
        )
        .expect("write spec");
        fs::write(
            &mapping_path,
            json!({"manifest": {"name": "MissingVersion"}}).to_string(),
        )
        .expect("write mapping");

        let error =
            compile_manifest_from_sources(&spec_path, &mapping_path).expect_err("identity error");

        assert!(error.to_string().contains("manifest.name/version"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("tradeassembly-{name}-{stamp}"));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
