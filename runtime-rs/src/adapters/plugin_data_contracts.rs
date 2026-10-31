//! Offline validation of data metadata inside a verified package installation.
use crate::data_pipeline::{canonical_sha256, local_schema_refs, validate_contract};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path},
    sync::{Arc, Mutex},
};
use tradeassembly_plugin_sdk::{
    data_history::VersionedOperationData, data_package::*, DigestedPackagePath,
};

fn document_bytes(
    root: &Path,
    doc: &DigestedPackagePath,
    total: &mut usize,
) -> Result<Vec<u8>, String> {
    if fs::symlink_metadata(root)
        .map_err(|_| "plugin_data_document_missing")?
        .file_type()
        .is_symlink()
    {
        return Err("plugin_data_symlink_rejected".into());
    }
    let mut path = root.to_path_buf();
    for component in Path::new(&doc.path).components() {
        let Component::Normal(name) = component else {
            return Err("plugin_data_path_invalid".into());
        };
        path.push(name);
        if fs::symlink_metadata(&path)
            .map_err(|_| "plugin_data_document_missing")?
            .file_type()
            .is_symlink()
        {
            return Err("plugin_data_symlink_rejected".into());
        }
    }
    let file = fs::File::open(&path).map_err(|_| "plugin_data_document_missing")?;
    if !file
        .metadata()
        .map_err(|_| "plugin_data_document_missing")?
        .is_file()
    {
        return Err("plugin_data_document_invalid".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_METADATA_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "plugin_data_document_unreadable")?;
    if bytes.len() > MAX_METADATA_DOCUMENT_BYTES {
        return Err("plugin_data_document_too_large".into());
    }
    *total = total
        .checked_add(bytes.len())
        .ok_or("plugin_data_metadata_total_exceeded")?;
    if *total > MAX_METADATA_TOTAL_BYTES {
        return Err("plugin_data_metadata_total_exceeded".into());
    }
    if format!("{:x}", Sha256::digest(&bytes)) != doc.sha256 {
        return Err("plugin_data_artifact_digest_mismatch".into());
    }
    Ok(bytes)
}

fn document(root: &Path, doc: &DigestedPackagePath, total: &mut usize) -> Result<Value, String> {
    let bytes = document_bytes(root, doc, total)?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "plugin_data_document_invalid")?;
    metadata_budget(&value)?;
    Ok(value)
}

/// One bounded immutable metadata entry per package-store instance. No ownership,
/// credentials, graph, registry or acquisition state is cached here. Every hit
/// rereads all declared documents with the same path/byte/aggregate bounds.
#[derive(Default)]
pub(crate) struct VerificationCache {
    entry: Mutex<Option<(String, Arc<VerifiedMetadata>)>>,
}
#[derive(Clone, Default)]
pub(crate) struct VerifiedMetadata {
    pub contracts: BTreeMap<String, VersionedOperationData>,
    pub schemas: BTreeMap<String, Value>,
}
impl VerificationCache {
    pub fn get(
        &self,
        envelope: &VersionedPackageDescriptor,
        root: &Path,
        manifest: &Value,
    ) -> Result<Arc<VerifiedMetadata>, String> {
        let VersionedPackageDescriptor::Data(data) = envelope else {
            return Ok(Arc::new(VerifiedMetadata::default()));
        };
        let identity = canonical_sha256(&serde_json::json!({
            "root": root, "descriptor": data, "manifest": manifest
        }))
        .map_err(|_| "plugin_data_cache_identity_invalid")?;
        let cached = self
            .entry
            .lock()
            .map_err(|_| "plugin_data_cache_unavailable")?
            .as_ref()
            .filter(|(key, _)| key == &identity)
            .map(|(_, value)| Arc::clone(value));
        if let Some(cached) = cached {
            // Identity includes every expected digest. Recheck the entire artifact
            // closure, including unrelated operations, rather than trusting mtime.
            let mut total = 0;
            for document in data
                .package
                .response_schemas
                .iter()
                .map(|s| &s.document)
                .chain(data.data_schemas.iter().map(|s| &s.document))
                .chain(data.data_contracts.iter().map(|c| &c.document))
            {
                document_bytes(root, document, &mut total)?;
            }
            return Ok(cached);
        }
        let verified = Arc::new(load_metadata(envelope, root, manifest)?);
        *self
            .entry
            .lock()
            .map_err(|_| "plugin_data_cache_unavailable")? =
            Some((identity, Arc::clone(&verified)));
        Ok(verified)
    }
}

#[cfg(test)]
pub(crate) fn verify(
    envelope: &VersionedPackageDescriptor,
    root: &Path,
    manifest: &Value,
) -> Result<BTreeMap<String, VersionedOperationData>, String> {
    load_metadata(envelope, root, manifest).map(|metadata| metadata.contracts)
}

fn load_metadata(
    envelope: &VersionedPackageDescriptor,
    root: &Path,
    manifest: &Value,
) -> Result<VerifiedMetadata, String> {
    let VersionedPackageDescriptor::Data(data) = envelope else {
        return Ok(VerifiedMetadata::default());
    };
    if serde_json::to_vec(manifest)
        .map_err(|_| "plugin_data_manifest_invalid")?
        .len()
        > MAX_METADATA_DOCUMENT_BYTES
    {
        return Err("plugin_data_manifest_too_large".into());
    }
    metadata_budget(manifest)?;
    let operations = manifest["operations"]
        .as_array()
        .ok_or("plugin_data_operation_undeclared")?;
    if operations.len() > MAX_MANIFEST_OPERATIONS {
        return Err("plugin_data_manifest_operations_exceeded".into());
    }
    let mut operation_index = BTreeMap::new();
    for op in operations {
        let id = op["id"]
            .as_str()
            .ok_or("plugin_data_operation_undeclared")?;
        if operation_index.insert(id, op).is_some() {
            return Err("plugin_data_operation_duplicate".into());
        }
    }
    let mut total = 0;
    // Base response schemas are metadata too, even though their V1 descriptor
    // binds byte hashes rather than canonical schema identities.
    let mut documents = BTreeMap::new();
    for response in &data.package.response_schemas {
        let value = document(root, &response.document, &mut total)?;
        if !local_schema_refs(&value) {
            return Err("plugin_data_schema_invalid".into());
        }
        schema_budget(&value)?;
        if jsonschema::validator_for(&value).is_err() {
            return Err("plugin_data_schema_invalid".into());
        }
        documents.insert(response.schema_ref.clone(), value);
    }

    let mut schemas = BTreeMap::new();
    for schema in &data.data_schemas {
        let value = document(root, &schema.document, &mut total)?;
        if canonical_sha256(&value).map_err(|_| "plugin_data_schema_numeric_range")?
            != schema.canonical_sha256
        {
            return Err("plugin_data_schema_digest_mismatch".into());
        }
        if !local_schema_refs(&value) {
            return Err("plugin_data_schema_invalid".into());
        }
        schema_budget(&value)?;
        if jsonschema::validator_for(&value).is_err() {
            return Err("plugin_data_schema_invalid".into());
        }
        schemas.insert(schema.schema_ref.clone(), schema.canonical_sha256.clone());
        documents.insert(schema.schema_ref.clone(), value);
    }
    let mut contracts = BTreeMap::new();
    for entry in &data.data_contracts {
        let value = document(root, &entry.document, &mut total)?;
        if canonical_sha256(&value).map_err(|_| "plugin_data_contract_numeric_range")?
            != entry.canonical_sha256
        {
            return Err("plugin_data_contract_digest_mismatch".into());
        }
        let contract: VersionedOperationData =
            serde_json::from_value(value).map_err(|_| "plugin_data_contract_invalid")?;
        schema_budget(&contract.parameter_schema)?;
        if contract.operation_id != entry.operation_id
            || !validate_contract(&contract)
            || jsonschema::validator_for(&contract.parameter_schema).is_err()
        {
            return Err("plugin_data_contract_invalid".into());
        }
        let operation = operation_index
            .get(entry.operation_id.as_str())
            .ok_or("plugin_data_operation_undeclared")?;
        for input in contract.inputs.values() {
            if !operation["traits"]["inputSchemaRefs"]
                .as_array()
                .is_some_and(|refs| refs.iter().any(|r| r == &input.data.schema.schema_ref))
            {
                return Err("plugin_data_input_schema_undeclared".into());
            }
        }
        for output in contract.outputs.values() {
            if !operation["traits"]["outputSchemaRefs"]
                .as_array()
                .is_some_and(|refs| refs.iter().any(|r| r == &output.schema.schema_ref))
            {
                return Err("plugin_data_output_schema_undeclared".into());
            }
        }
        for identity in contract
            .inputs
            .values()
            .map(|p| &p.data.schema)
            .chain(contract.outputs.values().map(|p| &p.schema))
            .chain(contract.state.iter().map(|s| &s.schema))
        {
            if schemas.get(&identity.schema_ref) != Some(&identity.sha256) {
                return Err("plugin_data_schema_unverified".into());
            }
        }
        contracts.insert(entry.operation_id.clone(), contract);
    }
    Ok(VerifiedMetadata {
        contracts,
        schemas: documents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_read_has_per_document_and_aggregate_limits() {
        let dir = tempfile::tempdir().unwrap();
        let value = serde_json::json!({"description":vec!["x".repeat(1000);1000]});
        let bytes = serde_json::to_vec(&value).unwrap();
        let path = dir.path().join("schema.json");
        fs::write(&path, &bytes).unwrap();
        let doc = DigestedPackagePath {
            path: "schema.json".into(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        let mut total = MAX_METADATA_TOTAL_BYTES - bytes.len();
        assert!(document(dir.path(), &doc, &mut total).is_ok());
        assert_eq!(
            document(dir.path(), &doc, &mut total).unwrap_err(),
            "plugin_data_metadata_total_exceeded"
        );
        fs::write(&path, vec![b' '; MAX_METADATA_DOCUMENT_BYTES + 1]).unwrap();
        assert_eq!(
            document(dir.path(), &doc, &mut 0).unwrap_err(),
            "plugin_data_document_too_large"
        );
    }
}

#[cfg(test)]
mod manifest_budget_tests {
    use super::*;
    #[test]
    fn typed_manifest_operation_budget_is_checked_before_document_work() {
        let (descriptor, _, _, _) =
            crate::adapters::plugin_packages::data_package_tests::materials();
        let envelope =
            VersionedPackageDescriptor::decode(&serde_json::to_vec(&descriptor).unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let manifest = |n| serde_json::json!({"operations":(0..n).map(|i|serde_json::json!({"id":format!("op.{i}")})).collect::<Vec<_>>()});
        assert_eq!(
            verify(&envelope, dir.path(), &manifest(513)).unwrap_err(),
            "plugin_data_manifest_operations_exceeded"
        );
        assert_eq!(
            verify(&envelope, dir.path(), &manifest(512)).unwrap_err(),
            "plugin_data_document_missing"
        );
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use serde_json::json;
    fn fixture(root: &Path) -> (VersionedPackageDescriptor, Value) {
        let (mut descriptor, contract, schema, mut manifest) =
            crate::adapters::plugin_packages::data_package_tests::materials();
        fs::write(root.join("contract.json"), &contract).unwrap();
        fs::write(root.join("schema.json"), schema).unwrap();
        let mut unrelated: Value = serde_json::from_slice(&contract).unwrap();
        unrelated["operationId"] = json!("unrelated");
        let bytes = serde_json::to_vec(&unrelated).unwrap();
        fs::write(root.join("unrelated.json"), &bytes).unwrap();
        let mut operation = manifest["operations"][0].clone();
        operation["id"] = json!("unrelated");
        manifest["operations"]
            .as_array_mut()
            .unwrap()
            .push(operation);
        descriptor["dataContracts"].as_array_mut().unwrap().push(json!({
            "operationId":"unrelated", "document":{"path":"unrelated.json","sha256":format!("{:x}",Sha256::digest(&bytes))},
            "canonicalSha256":canonical_sha256(&unrelated).unwrap()
        }));
        (
            VersionedPackageDescriptor::decode(&serde_json::to_vec(&descriptor).unwrap()).unwrap(),
            manifest,
        )
    }
    #[test]
    fn cache_reuses_verified_metadata_but_hashes_unrelated_artifacts_on_every_hit() {
        let dir = tempfile::tempdir().unwrap();
        let (descriptor, manifest) = fixture(dir.path());
        let cache = VerificationCache::default();
        let first = cache.get(&descriptor, dir.path(), &manifest).unwrap();
        let second = cache.get(&descriptor, dir.path(), &manifest).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "reuse parsed verified metadata"
        );
        assert!(second.contracts.contains_key("provide"));
        let path = dir.path().join("unrelated.json");
        let bytes = fs::read(&path).unwrap();
        let mut changed = bytes.clone();
        changed[0] = b' ';
        fs::write(&path, changed).unwrap();
        assert_eq!(
            cache.get(&descriptor, dir.path(), &manifest).err().unwrap(),
            "plugin_data_artifact_digest_mismatch"
        );
        fs::write(&path, &bytes).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &cache.get(&descriptor, dir.path(), &manifest).unwrap()
        ));
        fs::write(&path, vec![b' '; MAX_METADATA_DOCUMENT_BYTES + 1]).unwrap();
        assert_eq!(
            cache.get(&descriptor, dir.path(), &manifest).err().unwrap(),
            "plugin_data_document_too_large"
        );
    }
    #[test]
    fn cache_identity_covers_manifest_root_and_single_entry_eviction() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (descriptor, manifest) = fixture(a.path());
        fixture(b.path());
        let cache = VerificationCache::default();
        let first = cache.get(&descriptor, a.path(), &manifest).unwrap();
        let mut invalid = manifest.clone();
        invalid["operations"][0]["traits"]["outputSchemaRefs"] = json!([]);
        assert_eq!(
            cache.get(&descriptor, a.path(), &invalid).err().unwrap(),
            "plugin_data_output_schema_undeclared"
        );
        let other = cache.get(&descriptor, b.path(), &manifest).unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
        let again = cache.get(&descriptor, a.path(), &manifest).unwrap();
        assert!(
            !Arc::ptr_eq(&first, &again),
            "only one retained cache entry"
        );
    }
    #[cfg(unix)]
    #[test]
    fn cached_metadata_does_not_hide_replaced_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let (descriptor, manifest) = fixture(dir.path());
        let cache = VerificationCache::default();
        cache.get(&descriptor, dir.path(), &manifest).unwrap();
        let file = dir.path().join("unrelated.json");
        let other = dir.path().join("same-bytes.json");
        fs::rename(&file, &other).unwrap();
        std::os::unix::fs::symlink(&other, &file).unwrap();
        assert_eq!(
            cache.get(&descriptor, dir.path(), &manifest).err().unwrap(),
            "plugin_data_symlink_rejected"
        );
    }
}
