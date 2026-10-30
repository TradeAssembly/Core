//! Explicit package V2 envelope; the embedded legacy descriptor retains wire V1.
use crate::{DigestedPackagePath, PluginPackageDescriptor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DataPackageVersion {
    #[serde(rename = "2")]
    V2,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataContractDocument {
    pub operation_id: String,
    pub document: DigestedPackagePath,
    pub canonical_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataSchemaDocument {
    pub schema_ref: String,
    pub document: DigestedPackagePath,
    pub canonical_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataPackageDescriptor {
    pub package_contract_version: DataPackageVersion,
    pub package: PluginPackageDescriptor,
    pub data_contracts: Vec<DataContractDocument>,
    pub data_schemas: Vec<DataSchemaDocument>,
}
#[derive(Clone, Debug)]
pub enum VersionedPackageDescriptor {
    Legacy(PluginPackageDescriptor),
    Data(DataPackageDescriptor),
}
impl VersionedPackageDescriptor {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "plugin_package_descriptor_invalid")?;
        let result = match value["packageContractVersion"].as_str() {
            Some("1") => Self::Legacy(
                serde_json::from_value(value).map_err(|_| "plugin_package_descriptor_invalid")?,
            ),
            Some("2") => Self::Data(
                serde_json::from_value(value).map_err(|_| "plugin_package_descriptor_invalid")?,
            ),
            _ => return Err("plugin_package_version_unsupported".into()),
        };
        result
            .base()
            .validate()
            .map_err(|_| "plugin_package_descriptor_invalid")?;
        if let Self::Data(data) = &result {
            if data.data_contracts.is_empty()
                || data.data_contracts.len() > 64
                || data.data_schemas.len() > 256
            {
                return Err("plugin_data_package_bounds_invalid".into());
            }
            let mut operations = BTreeSet::new();
            let mut schemas = BTreeSet::new();
            for c in &data.data_contracts {
                if c.operation_id.is_empty()
                    || !operations.insert(&c.operation_id)
                    || !digest(&c.canonical_sha256)
                {
                    return Err("plugin_data_contract_identity_invalid".into());
                }
                c.document
                    .validate()
                    .map_err(|_| "plugin_data_contract_path_invalid")?;
            }
            for s in &data.data_schemas {
                if s.schema_ref.is_empty()
                    || !schemas.insert(&s.schema_ref)
                    || !digest(&s.canonical_sha256)
                {
                    return Err("plugin_data_schema_identity_invalid".into());
                }
                s.document
                    .validate()
                    .map_err(|_| "plugin_data_schema_path_invalid")?;
            }
        }
        Ok(result)
    }
    pub fn base(&self) -> &PluginPackageDescriptor {
        match self {
            Self::Legacy(d) => d,
            Self::Data(d) => &d.package,
        }
    }
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
