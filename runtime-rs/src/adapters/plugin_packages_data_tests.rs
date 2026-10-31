use super::*;
use crate::data_pipeline::{canonical_sha256 as canonical, tests::fixture};
use serde_json::json;

pub(crate) fn materials() -> (Value, Vec<u8>, Vec<u8>, Value) {
    let (_, _, contracts) = fixture();
    let mut contract = contracts
        .into_iter()
        .find(|c| c.contract.operation_id == "provide")
        .unwrap()
        .contract;
    let schema = json!({"type":"array","items":{"type":"number"}});
    let schema_bytes = serde_json::to_vec_pretty(&schema).unwrap();
    contract.outputs.get_mut("value").unwrap().schema.sha256 = canonical(&schema).unwrap();
    let contract_bytes = serde_json::to_vec_pretty(&contract).unwrap();
    let manifest = json!({"metadata":{"id":"example.data","version":"1"},"operations":[{"id":"provide","traits":{"inputSchemaRefs":[],"outputSchemaRefs":["schema://test/series@1"]}}]});
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let descriptor = json!({"packageContractVersion":"2","package":{
        "packageContractVersion":"1","plugin":{"id":"example.data","version":"1"},
        "manifest":{"path":"manifest.json","sha256":sha256(&manifest_bytes)},
        "targets":[{"target":tradeassembly_plugin_sdk::host_target(),"binary":{"path":"bin/plugin","sha256":sha256(b"not-executed")}}],
        "compatibility":{"hostContract":{"minimum":"1","maximum":"1"},"sdkVersion":tradeassembly_plugin_sdk::SDK_VERSION}
    },"dataContracts":[{"operationId":"provide","document":{"path":"contract.json","sha256":sha256(&contract_bytes)},"canonicalSha256":canonical(&contract).unwrap()}],
    "dataSchemas":[{"schemaRef":"schema://test/series@1","document":{"path":"schema.json","sha256":sha256(&schema_bytes)},"canonicalSha256":canonical(&schema).unwrap()}]});
    (descriptor, contract_bytes, schema_bytes, manifest)
}
pub(crate) fn archive_files(files: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(gzip);
    for (path, bytes) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        archive.append_data(&mut h, path, bytes.as_slice()).unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap()
}
fn install(
    directory: &Path,
    descriptor: &Value,
    contract: Vec<u8>,
    schema: Vec<u8>,
    manifest: &Value,
) -> Result<(LocalPluginPackageStore, InstalledPluginPackage), String> {
    let archive = archive_files(vec![
        (
            "tradeassembly-plugin.json",
            serde_json::to_vec(descriptor).unwrap(),
        ),
        ("manifest.json", serde_json::to_vec(manifest).unwrap()),
        ("contract.json", contract),
        ("schema.json", schema),
        ("bin/plugin", b"not-executed".to_vec()),
    ]);
    let path = directory.join("fixture.tar.gz");
    fs::write(&path, &archive).unwrap();
    let store = LocalPluginPackageStore::new(directory.join("store"))?;
    let installed = store.install(&PluginPackageInstallRequest {
        source_type: "file".into(),
        locator: path.to_string_lossy().into(),
        package_sha256: sha256(&archive),
        manifest_sha256: canonical(manifest).unwrap(),
        offline: true,
    })?;
    Ok((store, installed))
}
#[test]
fn data_package_loads_verified_contract_and_detects_post_install_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let (descriptor, contract, schema, manifest) = materials();
    assert_ne!(
        descriptor["dataSchemas"][0]["document"]["sha256"],
        descriptor["dataSchemas"][0]["canonicalSha256"]
    );
    let (store, installed) = install(dir.path(), &descriptor, contract, schema, &manifest).unwrap();
    let loaded = store
        .data_contract(&installed.package_sha256, "provide")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.operation_id, "provide");
    fs::write(installed.install_root.join("schema.json"), b"{}").unwrap();
    assert!(store
        .data_contract(&installed.package_sha256, "provide")
        .is_err());
}
#[test]
fn data_package_rejects_unverified_schema_and_unsupported_versions_without_installing() {
    for case in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let (mut descriptor, contract, schema, manifest) = materials();
        match case {
            0 => descriptor["dataSchemas"][0]["canonicalSha256"] = json!("f".repeat(64)),
            1 => descriptor["packageContractVersion"] = json!("3"),
            _ => descriptor["dataContracts"][0]["document"]["path"] = json!("../contract.json"),
        };
        assert!(install(dir.path(), &descriptor, contract, schema, &manifest).is_err());
        assert_eq!(
            fs::read_dir(dir.path().join("store/packages"))
                .unwrap()
                .count(),
            0
        );
    }
}
#[test]
fn data_package_rejects_nonlocal_schema_references_even_with_matching_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut descriptor, contract, _, manifest) = materials();
    let schema = json!({"$ref":"https://invalid.example/schema.json"});
    let bytes = serde_json::to_vec(&schema).unwrap();
    descriptor["dataSchemas"][0]["document"]["sha256"] = json!(sha256(&bytes));
    descriptor["dataSchemas"][0]["canonicalSha256"] = json!(canonical(&schema).unwrap());
    assert!(install(dir.path(), &descriptor, contract, bytes, &manifest)
        .err()
        .unwrap()
        .contains("schema_invalid"));
}
#[cfg(unix)]
#[test]
fn data_package_rejects_symlinked_documents() {
    let dir = tempfile::tempdir().unwrap();
    let (descriptor, contract, schema, manifest) = materials();
    let (store, installed) = install(dir.path(), &descriptor, contract, schema, &manifest).unwrap();
    let source = installed.install_root.join("schema.json");
    let target = dir.path().join("moved.json");
    fs::rename(&source, &target).unwrap();
    std::os::unix::fs::symlink(target, source).unwrap();
    assert!(store
        .data_contract(&installed.package_sha256, "provide")
        .unwrap_err()
        .contains("symlink"));
}
#[test]
fn data_package_preserves_strict_legacy_descriptor_and_wire_version() {
    let (descriptor, _, _, _) = materials();
    let base = descriptor["package"].clone();
    assert!(VersionedPackageDescriptor::decode(&serde_json::to_vec(&base).unwrap()).is_ok());
    assert!(serde_json::from_value::<PluginPackageDescriptor>(descriptor).is_err());
    let mut extra = base;
    extra["dataContracts"] = json!([]);
    assert!(VersionedPackageDescriptor::decode(&serde_json::to_vec(&extra).unwrap()).is_err());
    assert_eq!(tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION, "1");
}

#[cfg(unix)]
#[test]
fn data_package_rejects_symlinked_installation_root() {
    let dir = tempfile::tempdir().unwrap();
    let (descriptor, contract, schema, manifest) = materials();
    let (store, installed) = install(dir.path(), &descriptor, contract, schema, &manifest).unwrap();
    let moved = dir.path().join("relocated-package");
    fs::rename(&installed.install_root, &moved).unwrap();
    std::os::unix::fs::symlink(moved, &installed.install_root).unwrap();
    assert!(store
        .data_contract(&installed.package_sha256, "provide")
        .unwrap_err()
        .contains("symlink"));
}

#[test]
#[ignore = "requires DATA_PACKAGE_TEST_ARCHIVE; validates bytes without executing the package"]
fn data_package_external_broad_archive_verifies_without_execution() {
    let path = std::path::PathBuf::from(
        std::env::var_os("DATA_PACKAGE_TEST_ARCHIVE").expect("explicit local package required"),
    );
    let bytes = fs::read(&path).unwrap();
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
    let manifest: Value = tar
        .entries()
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.path().unwrap().as_ref() == std::path::Path::new("manifest.json"))
        .map(|mut e| {
            let mut b = vec![];
            std::io::Read::read_to_end(&mut e, &mut b).unwrap();
            serde_json::from_slice(&b).unwrap()
        })
        .expect("manifest required");
    let dir = tempfile::tempdir().unwrap();
    let store = LocalPluginPackageStore::new(dir.path().join("store")).unwrap();
    let installed = store
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: sha256(&bytes),
            manifest_sha256: canonical(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    assert!(manifest["operations"].as_array().unwrap().len() > 64);
    assert!(store.get(&installed.package_sha256).unwrap().is_some());
    assert!(store
        .data_contract(&installed.package_sha256, "indicator.ema")
        .unwrap()
        .is_some());
}

#[test]
fn data_package_v3_metadata_is_loaded_without_legacy_downgrade() {
    use tradeassembly_plugin_sdk::data_history::*;
    let dir = tempfile::tempdir().unwrap();
    let (mut descriptor, contract, schema, manifest) = materials();
    let mut operation: tradeassembly_plugin_sdk::data_contract::OperationDataContract =
        serde_json::from_slice(&contract).unwrap();
    operation.schema_version = tradeassembly_plugin_sdk::data_contract::DataContractVersion::V2;
    let record = OperationDataV3 {
        schema_version: HistoryVersion::V3,
        operation,
        history: HistoryContract {
            first_ready: std::collections::BTreeMap::from([(
                "value".into(),
                std::collections::BTreeMap::new(),
            )]),
            partitioning: Partitioning::InstrumentIndependent,
            required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
            reconstruction: Reconstruction::FiniteWindow,
        },
    };
    let bytes = serde_json::to_vec(&record).unwrap();
    descriptor["dataContracts"][0]["document"]["sha256"] = json!(sha256(&bytes));
    descriptor["dataContracts"][0]["canonicalSha256"] = json!(canonical(&record).unwrap());
    let (store, installed) = install(dir.path(), &descriptor, bytes, schema, &manifest).unwrap();
    assert!(store
        .operation_data(&installed.package_sha256, "provide")
        .unwrap()
        .unwrap()
        .v3()
        .is_some());
    assert_eq!(
        store
            .data_contract(&installed.package_sha256, "provide")
            .unwrap_err(),
        "plugin_data_contract_version_requires_v3"
    );
}

fn oversized_invalid_response_schema() -> Vec<u8> {
    serde_json::to_vec(&json!({"type":7,"description":"x".repeat(tradeassembly_plugin_sdk::data_package::MAX_METADATA_DOCUMENT_BYTES)})).unwrap()
}

#[test]
fn data_package_install_bounds_response_before_schema_compilation() {
    let dir = tempfile::tempdir().unwrap();
    let (mut descriptor, contract, _, manifest) = materials();
    let oversized = oversized_invalid_response_schema();
    descriptor["package"]["responseSchemas"] = json!([{"schemaRef":"schema://test/response@1","document":{"path":"schema.json","sha256":sha256(&oversized)}}]);
    assert_eq!(
        install(dir.path(), &descriptor, contract, oversized, &manifest)
            .err()
            .unwrap(),
        "plugin_data_document_too_large"
    );
    assert_eq!(
        fs::read_dir(dir.path().join("store/packages"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn data_package_reload_bounds_response_before_schema_compilation() {
    let dir = tempfile::tempdir().unwrap();
    let (mut descriptor, contract, schema, manifest) = materials();
    descriptor["package"]["responseSchemas"] = json!([{"schemaRef":"schema://test/response@1","document":{"path":"schema.json","sha256":sha256(&schema)}}]);
    let (store, mut installed) =
        install(dir.path(), &descriptor, contract, schema, &manifest).unwrap();
    let oversized = oversized_invalid_response_schema();
    descriptor["package"]["responseSchemas"][0]["document"]["sha256"] = json!(sha256(&oversized));
    fs::write(installed.install_root.join("schema.json"), oversized).unwrap();
    fs::write(
        installed.install_root.join("tradeassembly-plugin.json"),
        serde_json::to_vec(&descriptor).unwrap(),
    )
    .unwrap();
    installed.descriptor = descriptor;
    fs::write(
        installed.install_root.join(INSTALLED_RECORD),
        serde_json::to_vec(&installed).unwrap(),
    )
    .unwrap();
    assert_eq!(
        store.get(&installed.package_sha256).unwrap_err(),
        "plugin_data_document_too_large"
    );
}

/// Explicit local diagnostic, never enabled by the normal test suite. No plugin
/// executable is run; the caller supplies an already retained package archive.
#[test]
#[ignore = "requires an explicit local diagnostic package"]
fn data_package_bounded_lookup_cost_probe() {
    use std::time::Instant;
    let path = PathBuf::from(std::env::var_os("DATA_PACKAGE_PROBE").expect("DATA_PACKAGE_PROBE"));
    let bytes = fs::read(&path).unwrap();
    let mut archive = Archive::new(GzDecoder::new(Cursor::new(&bytes)));
    let manifest_bytes = archive
        .entries()
        .unwrap()
        .map(Result::unwrap)
        .find_map(|mut entry| {
            (entry.path().unwrap().as_ref() == Path::new("manifest.json")).then(|| {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                bytes
            })
        })
        .expect("manifest.json");
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = LocalPluginPackageStore::new(dir.path()).unwrap();
    let started = Instant::now();
    let installed = store
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: sha256(&bytes),
            manifest_sha256: canonical(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    let install_us = started.elapsed().as_micros();
    let envelope =
        VersionedPackageDescriptor::decode(&serde_json::to_vec(&installed.descriptor).unwrap())
            .unwrap();
    let VersionedPackageDescriptor::Data(data) = envelope else {
        panic!("data package")
    };
    let operation = &data.data_contracts[0].operation_id;
    let schema = &data.data_schemas[0].schema_ref;
    let mut timings = std::collections::BTreeMap::new();
    for action in ["get", "operation_data", "data_schema"] {
        let started = Instant::now();
        for _ in 0..10 {
            match action {
                "get" => {
                    store.get(&installed.package_sha256).unwrap().unwrap();
                }
                "operation_data" => {
                    store
                        .operation_data(&installed.package_sha256, operation)
                        .unwrap()
                        .unwrap();
                }
                _ => {
                    store
                        .data_schema(&installed.package_sha256, schema)
                        .unwrap()
                        .unwrap();
                }
            }
        }
        timings.insert(action, started.elapsed().as_micros());
    }
    let raw_record = fs::read(installed.install_root.join(INSTALLED_RECORD)).unwrap();
    let raw_manifest = fs::read(installed.install_root.join(&data.package.manifest.path)).unwrap();
    let target = target_record(&data.package).unwrap();
    let raw_executable = fs::read(installed.install_root.join(&target.binary.path)).unwrap();
    let mut phases = std::collections::BTreeMap::new();
    for action in [
        "parse_record",
        "parse_manifest_yaml",
        "manifest_canonical",
        "hash_executable",
    ] {
        let started = Instant::now();
        for _ in 0..10 {
            match action {
                "parse_record" => {
                    std::hint::black_box(
                        serde_json::from_slice::<InstalledPluginPackage>(&raw_record).unwrap(),
                    );
                }
                "parse_manifest_yaml" => {
                    std::hint::black_box(serde_yaml::from_slice::<Value>(&raw_manifest).unwrap());
                }
                "manifest_canonical" => {
                    std::hint::black_box(canonical(&manifest).unwrap());
                }
                _ => {
                    std::hint::black_box(sha256(&raw_executable));
                }
            }
        }
        phases.insert(action, started.elapsed().as_micros());
    }
    eprintln!(
        "PACKAGE_PHASE_PROBE {}",
        json!({"iterations":10,"phaseMicros":phases})
    );
    eprintln!(
        "PACKAGE_COST_PROBE {}",
        json!({"archiveSha256":installed.package_sha256,"contractCount":data.data_contracts.len(),"iterations":10,"installMicros":install_us,"lookupMicros":timings,"executesPlugin":false})
    );
}

#[test]
fn data_package_snapshot_is_current_bounded_and_returns_independent_values() {
    let dir = tempfile::tempdir().unwrap();
    let (descriptor, contract, schema, manifest) = materials();
    let (store, installed) = install(dir.path(), &descriptor, contract, schema, &manifest).unwrap();
    let operations = vec!["provide".to_owned()];
    let first = store
        .operation_data_snapshot(&installed.package_sha256, &operations)
        .unwrap()
        .unwrap();
    assert_eq!(first.contracts["provide"].operation_id, "provide");
    let mut changed = first.package.clone();
    changed.manifest["metadata"]["version"] = json!("changed-by-caller");
    assert_eq!(
        store
            .get(&installed.package_sha256)
            .unwrap()
            .unwrap()
            .manifest["metadata"]["version"],
        "1"
    );
    let too_many =
        vec!["provide".to_owned(); tradeassembly_plugin_sdk::data_package::MAX_DATA_CONTRACTS + 1];
    assert_eq!(
        store
            .operation_data_snapshot(&installed.package_sha256, &too_many)
            .err()
            .unwrap(),
        "plugin_data_snapshot_bounds_invalid"
    );
    fs::write(&installed.executable_path, b"tampered-executable").unwrap();
    assert_eq!(
        store
            .operation_data_snapshot(&installed.package_sha256, &operations)
            .err()
            .unwrap(),
        "plugin_package_integrity_invalid"
    );
}

#[test]
fn recipe_package_v3_loads_reopens_and_detects_unrelated_recipe_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let (mut descriptor, contract, schema, manifest) = materials();
    let contract_value: Value = serde_json::from_slice(&contract).unwrap();
    let recipe = json!({"schemaVersion":"tradeassembly.data_recipe.v1","recipeId":"source-alias","semanticsVersion":"1","parameterSchema":{"type":"object","additionalProperties":false},"inputs":{},"nodes":{"source":{"operation":{"pluginRef":"self","operationId":"provide","semanticsVersion":contract_value["semanticsVersion"]},"parameters":{}}},"bindings":[],"outputs":{"value":{"producer":{"kind":"output","node":"source","port":"value"},"data":contract_value["outputs"]["value"]}}});
    let recipe_bytes = serde_json::to_vec(&recipe).unwrap();
    descriptor["packageContractVersion"] = json!("3");
    descriptor["recipes"] = json!([{"recipeId":"source-alias","document":{"path":"recipe.json","sha256":sha256(&recipe_bytes)},"canonicalSha256":canonical(&recipe).unwrap()}]);
    let archive = archive_files(vec![
        (
            "tradeassembly-plugin.json",
            serde_json::to_vec(&descriptor).unwrap(),
        ),
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("contract.json", contract),
        ("schema.json", schema),
        ("recipe.json", recipe_bytes),
        ("bin/plugin", b"not-executed".to_vec()),
    ]);
    let archive_path = dir.path().join("recipes.tar.gz");
    fs::write(&archive_path, &archive).unwrap();
    let root = dir.path().join("store");
    let store = LocalPluginPackageStore::new(&root).unwrap();
    let installed = store
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: archive_path.to_string_lossy().into(),
            package_sha256: sha256(&archive),
            manifest_sha256: canonical(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    let loaded = store
        .data_recipe(&installed.package_sha256, "source-alias")
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(&loaded).unwrap(), recipe);
    assert!(store
        .data_recipe(&installed.package_sha256, "missing")
        .unwrap()
        .is_none());
    drop(store);
    let reopened = LocalPluginPackageStore::new(&root).unwrap();
    assert_eq!(
        reopened
            .data_recipe(&installed.package_sha256, "source-alias")
            .unwrap()
            .unwrap(),
        loaded
    );
    reopened
        .operation_data(&installed.package_sha256, "provide")
        .unwrap()
        .unwrap();
    fs::write(installed.install_root.join("recipe.json"), b"{}").unwrap();
    assert_eq!(
        reopened
            .operation_data(&installed.package_sha256, "provide")
            .unwrap_err(),
        "plugin_data_artifact_digest_mismatch"
    );
    assert_eq!(
        reopened
            .data_recipe(&installed.package_sha256, "source-alias")
            .unwrap_err(),
        "plugin_data_artifact_digest_mismatch"
    );
}
