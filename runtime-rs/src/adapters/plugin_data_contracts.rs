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
    data_contract::OperationDataContract, data_package::VersionedPackageDescriptor,
    DigestedPackagePath,
};

fn document(root: &Path, doc: &DigestedPackagePath) -> Result<Value, String> {
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
    file.take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| "plugin_data_document_unreadable")?;
    if bytes.len() > 1_048_576 {
        return Err("plugin_data_document_too_large".into());
    }
    if format!("{:x}", Sha256::digest(&bytes)) != doc.sha256 {
        return Err("plugin_data_artifact_digest_mismatch".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "plugin_data_document_invalid".into())
}

pub(crate) fn verify(
    envelope: &VersionedPackageDescriptor,
    root: &Path,
    manifest: &Value,
) -> Result<BTreeMap<String, OperationDataContract>, String> {
    let VersionedPackageDescriptor::Data(data) = envelope else {
        return Ok(BTreeMap::new());
    };
    let mut schemas = BTreeMap::new();
    for schema in &data.data_schemas {
        let value = document(root, &schema.document)?;
        if canonical_sha256(&value).map_err(|_| "plugin_data_schema_numeric_range")?
            != schema.canonical_sha256
        {
            return Err("plugin_data_schema_digest_mismatch".into());
        }
        if !local_schema_refs(&value) || jsonschema::validator_for(&value).is_err() {
            return Err("plugin_data_schema_invalid".into());
        }
        schemas.insert(schema.schema_ref.clone(), schema.canonical_sha256.clone());
    }
    let mut contracts = BTreeMap::new();
    for entry in &data.data_contracts {
        let value = document(root, &entry.document)?;
        if canonical_sha256(&value).map_err(|_| "plugin_data_contract_numeric_range")?
            != entry.canonical_sha256
        {
            return Err("plugin_data_contract_digest_mismatch".into());
        }
        let contract: OperationDataContract =
            serde_json::from_value(value).map_err(|_| "plugin_data_contract_invalid")?;
        if contract.operation_id != entry.operation_id
            || !validate_contract(&contract)
            || jsonschema::validator_for(&contract.parameter_schema).is_err()
        {
            return Err("plugin_data_contract_invalid".into());
        }
        let operation = manifest["operations"]
            .as_array()
            .and_then(|ops| ops.iter().find(|op| op["id"] == entry.operation_id))
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
        .map(|schema| document(root, &schema.document))
        .transpose()
}
