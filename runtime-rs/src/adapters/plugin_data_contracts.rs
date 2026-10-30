//! Offline validation of data metadata inside a verified package installation.
use crate::data_pipeline::{canonical_sha256, local_schema_refs, validate_contract};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path},
};
use tradeassembly_plugin_sdk::{
    data_contract::OperationDataContract, data_package::*, DigestedPackagePath,
};

fn document(root: &Path, doc: &DigestedPackagePath, total: &mut usize) -> Result<Value, String> {
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
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "plugin_data_document_invalid")?;
    metadata_budget(&value)?;
    Ok(value)
}

pub(crate) fn verify(
    envelope: &VersionedPackageDescriptor,
    root: &Path,
    manifest: &Value,
) -> Result<BTreeMap<String, OperationDataContract>, String> {
    let VersionedPackageDescriptor::Data(data) = envelope else {
        return Ok(BTreeMap::new());
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
    }
    let mut contracts = BTreeMap::new();
    for entry in &data.data_contracts {
        let value = document(root, &entry.document, &mut total)?;
        if canonical_sha256(&value).map_err(|_| "plugin_data_contract_numeric_range")?
            != entry.canonical_sha256
        {
            return Err("plugin_data_contract_digest_mismatch".into());
        }
        let contract: OperationDataContract =
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
    Ok(contracts)
}

/// Only called after package verification; each read still verifies byte identity.
pub(crate) fn schema(
    envelope: &VersionedPackageDescriptor,
    root: &Path,
    manifest: &Value,
    schema_ref: &str,
) -> Result<Option<Value>, String> {
    verify(envelope, root, manifest)?;
    let VersionedPackageDescriptor::Data(data) = envelope else {
        return Ok(None);
    };
    data.data_schemas
        .iter()
        .find(|schema| schema.schema_ref == schema_ref)
        .map(|schema| document(root, &schema.document, &mut 0))
        .transpose()
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
