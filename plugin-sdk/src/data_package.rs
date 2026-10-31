//! Explicit package V2 envelope; the embedded legacy descriptor retains wire V1.
use crate::{DigestedPackagePath, PluginPackageDescriptor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Bounded V2 metadata capacity; this does not expand wire frames or runtime DAGs.
pub const MAX_DATA_CONTRACTS: usize = 256;
pub const MAX_DATA_SCHEMAS: usize = 256;
pub const MAX_DATA_RECIPES: usize = 256;
pub const MAX_DESCRIPTOR_BYTES: usize = 1_048_576;
pub const MAX_METADATA_DOCUMENT_BYTES: usize = 1_048_576;
pub const MAX_METADATA_TOTAL_BYTES: usize = 16 * 1_048_576;
pub const MAX_MANIFEST_OPERATIONS: usize = 512;

/// Bound structural work before canonicalization/schema compilation. Local JSON
/// pointers are expanded against a shared node budget; cycles, anchors and
/// dynamic/recursive references are not supported by data package metadata.
pub fn metadata_budget(value: &serde_json::Value) -> Result<(), &'static str> {
    value_budget(value, false)
}
pub fn schema_budget(value: &serde_json::Value) -> Result<(), &'static str> {
    value_budget(value, true)
}
fn value_budget(value: &serde_json::Value, resolve_refs: bool) -> Result<(), &'static str> {
    use serde_json::Value;
    let mut stack = vec![(value, 0usize, Vec::<&str>::new())];
    let mut nodes = 0usize;
    let mut patterns = 0usize;
    while let Some((item, depth, refs)) = stack.pop() {
        nodes += 1;
        if nodes > 32_768 || depth > 32 {
            return Err("plugin_data_metadata_bounds_invalid");
        }
        match item {
            Value::String(s) if s.len() > 16_384 => {
                return Err("plugin_data_metadata_bounds_invalid")
            }
            Value::Object(map) => {
                if map.len() > 2048 {
                    return Err("plugin_data_metadata_bounds_invalid");
                }
                for (key, child) in map {
                    if key.len() > 256 {
                        return Err("plugin_data_metadata_bounds_invalid");
                    }
                    if matches!(key.as_str(), "$dynamicRef" | "$recursiveRef") {
                        return Err("plugin_data_schema_reference_unsupported");
                    }
                    if key == "$ref" && resolve_refs {
                        let reference = child
                            .as_str()
                            .ok_or("plugin_data_schema_reference_unsupported")?;
                        if !reference.starts_with("#/")
                            || refs.contains(&reference)
                            || refs.len() >= 8
                        {
                            return Err("plugin_data_schema_reference_unsupported");
                        }
                        let target = value
                            .pointer(&reference[1..])
                            .ok_or("plugin_data_schema_reference_unsupported")?;
                        let mut next = refs.clone();
                        next.push(reference);
                        stack.push((target, depth + 1, next));
                    }
                    if key == "patternProperties" && resolve_refs {
                        if let Some(properties) = child.as_object() {
                            for pattern in properties.keys() {
                                patterns += 1;
                                if patterns > 64 || pattern.len() > 512 {
                                    return Err("plugin_data_metadata_bounds_invalid");
                                }
                            }
                        }
                    }
                    if key == "pattern" && resolve_refs && child.is_string() {
                        patterns += 1;
                        if patterns > 64 || child.as_str().is_none_or(|s| s.len() > 512) {
                            return Err("plugin_data_metadata_bounds_invalid");
                        }
                    }
                    stack.push((child, depth + 1, refs.clone()));
                }
            }
            Value::Array(items) => {
                if items.len() > 2048 {
                    return Err("plugin_data_metadata_bounds_invalid");
                }
                for child in items {
                    stack.push((child, depth + 1, refs.clone()));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecipePackageVersion {
    #[serde(rename = "3")]
    V3,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataRecipeDocument {
    pub recipe_id: String,
    pub document: DigestedPackagePath,
    pub canonical_sha256: String,
}
/// V1 and V2 stay closed; recipes require their own explicit package envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipePackageDescriptor {
    pub package_contract_version: RecipePackageVersion,
    pub package: PluginPackageDescriptor,
    pub data_contracts: Vec<DataContractDocument>,
    pub data_schemas: Vec<DataSchemaDocument>,
    pub recipes: Vec<DataRecipeDocument>,
}
#[derive(Clone, Copy, Serialize)]
pub struct DataPackageView<'a> {
    pub package: &'a PluginPackageDescriptor,
    pub data_contracts: &'a [DataContractDocument],
    pub data_schemas: &'a [DataSchemaDocument],
    pub recipes: &'a [DataRecipeDocument],
}
#[derive(Clone, Debug)]
pub enum VersionedPackageDescriptor {
    Legacy(PluginPackageDescriptor),
    Data(DataPackageDescriptor),
    Recipes(RecipePackageDescriptor),
}
impl VersionedPackageDescriptor {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_DESCRIPTOR_BYTES {
            return Err("plugin_package_descriptor_too_large".into());
        }
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "plugin_package_descriptor_invalid")?;
        let result = match value["packageContractVersion"].as_str() {
            Some("1") => Self::Legacy(
                serde_json::from_value(value).map_err(|_| "plugin_package_descriptor_invalid")?,
            ),
            Some("2") => {
                metadata_budget(&value)?;
                Self::Data(
                    serde_json::from_value(value)
                        .map_err(|_| "plugin_package_descriptor_invalid")?,
                )
            }
            Some("3") => {
                metadata_budget(&value)?;
                Self::Recipes(
                    serde_json::from_value(value)
                        .map_err(|_| "plugin_package_descriptor_invalid")?,
                )
            }
            _ => return Err("plugin_package_version_unsupported".into()),
        };
        result
            .base()
            .validate()
            .map_err(|_| "plugin_package_descriptor_invalid")?;
        if let Some(data) = result.data() {
            if (data.data_contracts.is_empty() && data.recipes.is_empty())
                || data.data_contracts.len() > MAX_DATA_CONTRACTS
                || data.data_schemas.len() > MAX_DATA_SCHEMAS
                || data.package.response_schemas.len() > MAX_DATA_SCHEMAS
                || data.recipes.len() > MAX_DATA_RECIPES
            {
                return Err("plugin_data_package_bounds_invalid".into());
            }
            let mut operations = BTreeSet::new();
            let mut schemas = BTreeSet::new();
            for c in data.data_contracts {
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
            for s in data.data_schemas {
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
            let mut recipe_ids = BTreeSet::new();
            for recipe in data.recipes {
                if recipe.recipe_id.is_empty()
                    || recipe.recipe_id.len() > 128
                    || !recipe_ids.insert(&recipe.recipe_id)
                    || !digest(&recipe.canonical_sha256)
                {
                    return Err("plugin_data_recipe_identity_invalid".into());
                }
                recipe
                    .document
                    .validate()
                    .map_err(|_| "plugin_data_recipe_path_invalid")?;
            }
        }
        Ok(result)
    }
    pub fn data(&self) -> Option<DataPackageView<'_>> {
        match self {
            Self::Legacy(_) => None,
            Self::Data(d) => Some(DataPackageView {
                package: &d.package,
                data_contracts: &d.data_contracts,
                data_schemas: &d.data_schemas,
                recipes: &[],
            }),
            Self::Recipes(d) => Some(DataPackageView {
                package: &d.package,
                data_contracts: &d.data_contracts,
                data_schemas: &d.data_schemas,
                recipes: &d.recipes,
            }),
        }
    }

    pub fn base(&self) -> &PluginPackageDescriptor {
        match self {
            Self::Legacy(d) => d,
            Self::Data(d) => &d.package,
            Self::Recipes(d) => &d.package,
        }
    }
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn descriptor(count: usize) -> Value {
        let digest = "a".repeat(64);
        json!({"packageContractVersion":"2","package":{"packageContractVersion":"1","plugin":{"id":"example","version":"1"},"manifest":{"path":"manifest.json","sha256":digest},"targets":[{"target":"test","binary":{"path":"bin/plugin","sha256":digest}}],"compatibility":{"hostContract":{"minimum":"1","maximum":"1"},"sdkVersion":"test"}},"dataContracts":(0..count).map(|n|json!({"operationId":format!("op.{n}"),"document":{"path":format!("contracts/{n}.json"),"sha256":digest},"canonicalSha256":digest})).collect::<Vec<_>>(),"dataSchemas":[]})
    }
    #[test]
    fn data_package_capacity_has_explicit_accept_and_reject_boundaries_and_old_versions_stay_closed(
    ) {
        assert_eq!(
            VersionedPackageDescriptor::decode(&serde_json::to_vec(&descriptor(0)).unwrap())
                .unwrap_err(),
            "plugin_data_package_bounds_invalid"
        );
        for count in [64, 65, 101, 256] {
            assert!(
                VersionedPackageDescriptor::decode(
                    &serde_json::to_vec(&descriptor(count)).unwrap()
                )
                .is_ok(),
                "{count}"
            );
        }
        assert_eq!(
            VersionedPackageDescriptor::decode(&serde_json::to_vec(&descriptor(257)).unwrap())
                .unwrap_err(),
            "plugin_data_package_bounds_invalid"
        );
        let mut d = descriptor(1);
        let mut old = d["package"].clone();
        assert!(VersionedPackageDescriptor::decode(&serde_json::to_vec(&old).unwrap()).is_ok());
        old["dataContracts"] = json!([]);
        assert!(VersionedPackageDescriptor::decode(&serde_json::to_vec(&old).unwrap()).is_err());
        let mut schemas = descriptor(1);
        schemas["dataSchemas"]=json!((0..256).map(|n|json!({"schemaRef":format!("schema://test/{n}"),"document":{"path":format!("schemas/{n}.json"),"sha256":"a".repeat(64)},"canonicalSha256":"a".repeat(64)})).collect::<Vec<_>>());
        assert!(VersionedPackageDescriptor::decode(&serde_json::to_vec(&schemas).unwrap()).is_ok());
        let extra = schemas["dataSchemas"][0].clone();
        schemas["dataSchemas"].as_array_mut().unwrap().push(extra);
        assert_eq!(
            VersionedPackageDescriptor::decode(&serde_json::to_vec(&schemas).unwrap()).unwrap_err(),
            "plugin_data_package_bounds_invalid"
        );
        d["packageContractVersion"] = json!("4");
        assert_eq!(
            VersionedPackageDescriptor::decode(&serde_json::to_vec(&d).unwrap()).unwrap_err(),
            "plugin_package_version_unsupported"
        );
        assert_eq!(
            VersionedPackageDescriptor::decode(&vec![b' '; MAX_DESCRIPTOR_BYTES + 1]).unwrap_err(),
            "plugin_package_descriptor_too_large"
        );
    }
    #[test]
    fn data_package_structural_and_reference_work_are_bounded() {
        let mut nested = json!(null);
        for _ in 0..33 {
            nested = json!({"x":nested});
        }
        assert!(metadata_budget(&nested).is_err());
        assert!(metadata_budget(&json!({"x":vec![0;2049]})).is_err());
        assert!(schema_budget(&json!({"pattern":"x".repeat(513)})).is_err());
        assert!(
            schema_budget(&json!({"$defs":{"a":{"$ref":"#/$defs/a"}},"$ref":"#/$defs/a"})).is_err()
        );
        assert!(schema_budget(&json!({"$ref":"https://example.invalid/schema"})).is_err());
        assert!(schema_budget(
            &json!({"$defs":{"n":{"type":"number"}},"items":{"$ref":"#/$defs/n"}})
        )
        .is_ok());
        assert!(schema_budget(
            &json!({"type":"object","properties":{"pattern":{"type":"string"}}})
        )
        .is_ok());
        // Generic contract wrappers do not resolve a parameter schema's pointers
        // against the contract root; the host validates that schema separately.
        assert!(metadata_budget(
            &json!({"parameterSchema":{"$ref":"#/$defs/n","$defs":{"n":{"type":"number"}}}})
        )
        .is_ok());
    }
}

#[cfg(test)]
mod recipe_package_tests {
    use super::*;
    use serde_json::{json, Value};
    fn package(count: usize) -> Value {
        let hash = "a".repeat(64);
        json!({"packageContractVersion":"3","package":{"packageContractVersion":"1","plugin":{"id":"recipes","version":"1"},"manifest":{"path":"manifest.json","sha256":hash},"targets":[{"target":"test","binary":{"path":"bin/plugin","sha256":hash}}],"compatibility":{"hostContract":{"minimum":"1","maximum":"1"},"sdkVersion":"test"}},"dataContracts":[],"dataSchemas":[],"recipes":(0..count).map(|i|json!({"recipeId":format!("recipe.{i}"),"document":{"path":format!("recipes/{i}.json"),"sha256":hash},"canonicalSha256":hash})).collect::<Vec<_>>()})
    }
    fn decode(value: &Value) -> Result<VersionedPackageDescriptor, String> {
        VersionedPackageDescriptor::decode(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn recipes_require_explicit_version_and_bounded_distinct_artifact_identities() {
        assert!(decode(&package(1)).is_ok());
        assert!(decode(&package(MAX_DATA_RECIPES)).is_ok());
        assert_eq!(
            decode(&package(0)).unwrap_err(),
            "plugin_data_package_bounds_invalid"
        );
        assert_eq!(
            decode(&package(MAX_DATA_RECIPES + 1)).unwrap_err(),
            "plugin_data_package_bounds_invalid"
        );
        let mut value = package(2);
        value["recipes"][1] = value["recipes"][0].clone();
        assert_eq!(
            decode(&value).unwrap_err(),
            "plugin_data_recipe_identity_invalid"
        );
        let mut value = package(1);
        value["recipes"][0]["document"]["path"] = json!("../recipe.json");
        assert_eq!(
            decode(&value).unwrap_err(),
            "plugin_data_recipe_path_invalid"
        );
        let mut value = package(1);
        value["packageContractVersion"] = json!("2");
        assert_eq!(
            decode(&value).unwrap_err(),
            "plugin_package_descriptor_invalid"
        );
    }
}
