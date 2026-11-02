//! Offline validation of data metadata inside a verified package installation.
use crate::data_pipeline::{canonical_sha256, local_schema_refs, validate_contract};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
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
    let fs_timing = crate::service::order_plans::financial_timing("metadata.document_fs_bounds");
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
    drop(fs_timing);
    let _hash_timing = crate::service::order_plans::financial_timing("metadata.document_hash");
    if format!("{:x}", Sha256::digest(&bytes)) != doc.sha256 {
        return Err("plugin_data_artifact_digest_mismatch".into());
    }
    Ok(bytes)
}

fn document(root: &Path, doc: &DigestedPackagePath, total: &mut usize) -> Result<Value, String> {
    let bytes = document_bytes(root, doc, total)?;
    let _parse_timing =
        crate::service::order_plans::financial_timing("metadata.document_decode_validate");
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "plugin_data_document_invalid")?;
    metadata_budget(&value)?;
    Ok(value)
}

/// Bounded immutable metadata per package-store instance. No ownership,
/// credentials, graph, registry or acquisition state is cached here. Every hit
/// rereads all declared documents with the same path/byte/aggregate bounds.
const MAX_VERIFIED_CACHE_ENTRIES: usize = 8;
pub(crate) struct VerificationCache {
    identities: super::plugin_packages::parsing::PureMemo<String>,
    entries: Mutex<MetadataEntries>,
    max_retained_bytes: usize,
}
impl Default for VerificationCache {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            identities: Default::default(),
            max_retained_bytes: MAX_METADATA_TOTAL_BYTES,
        }
    }
}
#[derive(Default)]
struct MetadataEntries {
    values: VecDeque<(String, Arc<VerifiedMetadata>, usize)>,
    bytes: usize,
}
#[derive(Clone, Default, serde::Serialize)]
pub(crate) struct VerifiedMetadata {
    pub contracts: BTreeMap<String, VersionedOperationData>,
    pub schemas: BTreeMap<String, Value>,
    pub recipes: BTreeMap<String, tradeassembly_plugin_sdk::data_recipe::DataRecipe>,
}

// Count encoded retained metadata without allocating another whole copy. A
// valid package larger than this cache budget still works through the verified
// uncached path. The existing document and package limits remain unchanged.
fn retained_metadata_bytes(metadata: &VerifiedMetadata, limit: usize) -> Option<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let next = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("metadata cache budget"))?;
            self.bytes = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, metadata).ok()?;
    Some(counter.bytes)
}
impl VerificationCache {
    pub fn get(
        &self,
        envelope: &VersionedPackageDescriptor,
        root: &Path,
        manifest: &Value,
    ) -> Result<Arc<VerifiedMetadata>, String> {
        let Some(data) = envelope.data() else {
            return Ok(Arc::new(VerifiedMetadata::default()));
        };
        let _timing = crate::service::order_plans::financial_timing("metadata.total");
        let identity_timing =
            crate::service::order_plans::financial_timing("metadata.identity_hash");
        let identity = canonical_sha256(&serde_json::json!({
            "root": root, "descriptor": data, "manifest": manifest
        }))
        .map_err(|_| "plugin_data_cache_identity_invalid")?;
        drop(identity_timing);
        self.get_identified(envelope, root, manifest, &identity)
    }
    pub(super) fn get_parsed(
        &self,
        root: &Path,
        descriptor: &super::plugin_packages::parsing::Descriptor,
        manifest: &super::plugin_packages::parsing::Manifest,
    ) -> Result<Arc<VerifiedMetadata>, String> {
        let Some(data) = descriptor.envelope().data() else {
            return Ok(Arc::new(VerifiedMetadata::default()));
        };
        let _timing = crate::service::order_plans::financial_timing("metadata.total");
        let identity_timing =
            crate::service::order_plans::financial_timing("metadata.identity_hash");
        // The small witness is derived only from freshly read descriptor/manifest
        // bytes and the current root. Cache the SAME canonical identity used by
        // get(), never a second metadata namespace or a verification result.
        let witness = serde_json::to_vec(&serde_json::json!({"root":root,
            "descriptor":descriptor.encoding_sha256(),"manifest":manifest.encoding_sha256()}))
        .map_err(|_| "plugin_data_cache_identity_invalid")?;
        let identity = self.identities.get(&witness, || {
            canonical_sha256(
                &serde_json::json!({"root":root,"descriptor":data,"manifest":manifest.value()}),
            )
            .map_err(|_| "plugin_data_cache_identity_invalid".into())
        })?;
        drop(identity_timing);
        self.get_identified(descriptor.envelope(), root, manifest.value(), &identity)
    }
    fn get_identified(
        &self,
        envelope: &VersionedPackageDescriptor,
        root: &Path,
        manifest: &Value,
        identity: &str,
    ) -> Result<Arc<VerifiedMetadata>, String> {
        let Some(data) = envelope.data() else {
            return Ok(Arc::new(VerifiedMetadata::default()));
        };
        let cached = self
            .entries
            .lock()
            .map_err(|_| "plugin_data_cache_unavailable")?
            .values
            .iter()
            .find(|(key, _, _)| key.as_str() == identity)
            .map(|(_, value, _)| Arc::clone(value));
        if let Some(cached) = cached {
            // Identity includes every expected digest. Recheck the entire artifact
            // closure, including unrelated operations, rather than trusting mtime.
            let _recheck_timing =
                crate::service::order_plans::financial_timing("metadata.cached_closure_fs_hash");
            let mut total = 0;
            for document in data
                .package
                .response_schemas
                .iter()
                .map(|s| &s.document)
                .chain(data.data_schemas.iter().map(|s| &s.document))
                .chain(data.data_contracts.iter().map(|c| &c.document))
                .chain(data.recipes.iter().map(|r| &r.document))
            {
                document_bytes(root, document, &mut total)?;
            }
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| "plugin_data_cache_unavailable")?;
            if let Some(index) = entries
                .values
                .iter()
                .position(|(key, _, _)| key.as_str() == identity)
            {
                let value = entries.values.remove(index).unwrap();
                entries.values.push_back(value);
            }
            return Ok(cached);
        }
        let verified = Arc::new(crate::service::order_plans::measured(
            "metadata.uncached_decode_validate",
            || load_metadata(envelope, root, manifest),
        )?);
        if let Some(bytes) = retained_metadata_bytes(&verified, self.max_retained_bytes) {
            let mut entries = self
                .entries
                .lock()
                .map_err(|_| "plugin_data_cache_unavailable")?;
            if let Some(index) = entries
                .values
                .iter()
                .position(|(key, _, _)| key.as_str() == identity)
            {
                let previous = entries.values.remove(index).unwrap();
                entries.bytes -= previous.2;
            }
            while entries.values.len() >= MAX_VERIFIED_CACHE_ENTRIES
                || entries.bytes > self.max_retained_bytes - bytes
            {
                let previous = entries.values.pop_front().unwrap();
                entries.bytes -= previous.2;
            }
            entries.bytes += bytes;
            entries
                .values
                .push_back((identity.to_owned(), Arc::clone(&verified), bytes));
        }
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
    let Some(data) = envelope.data() else {
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
    for schema in data.data_schemas {
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
    for entry in data.data_contracts {
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
    let mut recipes = BTreeMap::new();
    for entry in data.recipes {
        let value = document(root, &entry.document, &mut total)?;
        if canonical_sha256(&value).map_err(|_| "plugin_data_recipe_numeric_range")?
            != entry.canonical_sha256
        {
            return Err("plugin_data_recipe_digest_mismatch".into());
        }
        let recipe = tradeassembly_plugin_sdk::data_recipe::DataRecipe::decode(
            &serde_json::to_vec(&value).map_err(|_| "plugin_data_recipe_invalid")?,
        )?;
        if recipe.recipe_id != entry.recipe_id
            || !local_schema_refs(&recipe.parameter_schema)
            || jsonschema::validator_for(&recipe.parameter_schema).is_err()
        {
            return Err("plugin_data_recipe_invalid".into());
        }
        for port in recipe
            .inputs
            .values()
            .chain(recipe.outputs.values().map(|p| &p.data))
        {
            if schemas.get(&port.schema.schema_ref) != Some(&port.schema.sha256) {
                return Err("plugin_data_recipe_schema_unverified".into());
            }
        }
        for node in recipe
            .nodes
            .values()
            .filter(|n| n.operation.plugin_ref == "self")
        {
            let contract = contracts
                .get(&node.operation.operation_id)
                .ok_or("plugin_data_recipe_operation_missing")?;
            if contract.semantics_version != node.operation.semantics_version {
                return Err("plugin_data_recipe_semantics_mismatch".into());
            }
        }
        recipes.insert(entry.recipe_id.clone(), recipe);
    }
    Ok(VerifiedMetadata {
        contracts,
        schemas: documents,
        recipes,
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
    fn encoded_descriptor(v: &VersionedPackageDescriptor) -> Vec<u8> {
        match v {
            VersionedPackageDescriptor::Legacy(v) => serde_json::to_vec(v),
            VersionedPackageDescriptor::Data(v) => serde_json::to_vec(v),
            VersionedPackageDescriptor::Recipes(v) => serde_json::to_vec(v),
        }
        .unwrap()
    }
    #[test]
    fn parsed_identity_matches_canonical_identity_and_keeps_live_closure_checks() {
        let root = tempfile::tempdir().unwrap();
        let (descriptor, manifest) = fixture(root.path());
        let parsing = super::super::plugin_packages::parsing::ParsingCache::default();
        let parsed_descriptor = parsing
            .descriptor(&encoded_descriptor(&descriptor))
            .unwrap();
        let parsed_manifest = parsing
            .manifest(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        let cache = VerificationCache::default();
        let first = cache.get(&descriptor, root.path(), &manifest).unwrap();
        let second = cache
            .get_parsed(root.path(), &parsed_descriptor, &parsed_manifest)
            .unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "same canonical cache namespace"
        );
        let file = root.path().join("unrelated.json");
        let original = fs::read(&file).unwrap();
        fs::write(&file, b"changed after cached parse").unwrap();
        assert!(cache
            .get_parsed(root.path(), &parsed_descriptor, &parsed_manifest)
            .is_err());
        fs::write(file, original).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &cache
                .get_parsed(root.path(), &parsed_descriptor, &parsed_manifest)
                .unwrap()
        ));
        let other = tempfile::tempdir().unwrap();
        fixture(other.path());
        let distinct = cache
            .get_parsed(other.path(), &parsed_descriptor, &parsed_manifest)
            .unwrap();
        assert!(
            !Arc::ptr_eq(&first, &distinct),
            "root binding remains exact"
        );
    }
    #[test]
    fn changed_parsed_manifest_cannot_reuse_previously_verified_metadata() {
        let root = tempfile::tempdir().unwrap();
        let (descriptor, mut manifest) = fixture(root.path());
        let parsing = super::super::plugin_packages::parsing::ParsingCache::default();
        let descriptor = parsing
            .descriptor(&encoded_descriptor(&descriptor))
            .unwrap();
        let original = parsing
            .manifest(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        let cache = VerificationCache::default();
        cache
            .get_parsed(root.path(), &descriptor, &original)
            .unwrap();
        manifest["operations"][0]["id"] = json!("missing_contract_operation");
        let changed = parsing
            .manifest(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        assert!(cache
            .get_parsed(root.path(), &descriptor, &changed)
            .is_err());
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
    fn cache_identity_covers_manifest_and_keeps_distinct_package_roots() {
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
            Arc::ptr_eq(&first, &again),
            "alternating packages must not discard verified metadata"
        );
    }
    #[test]
    fn cache_evicts_least_recent_package_at_the_entry_bound() {
        let roots = (0..=MAX_VERIFIED_CACHE_ENTRIES)
            .map(|_| tempfile::tempdir().unwrap())
            .collect::<Vec<_>>();
        let materials = roots
            .iter()
            .map(|root| fixture(root.path()))
            .collect::<Vec<_>>();
        let cache = VerificationCache::default();
        let get = |index: usize| {
            cache
                .get(
                    &materials[index].0,
                    roots[index].path(),
                    &materials[index].1,
                )
                .unwrap()
        };
        let first = get(0);
        let second = get(1);
        for index in 2..MAX_VERIFIED_CACHE_ENTRIES {
            get(index);
        }
        assert!(Arc::ptr_eq(&first, &get(0)));
        get(MAX_VERIFIED_CACHE_ENTRIES);
        assert!(Arc::ptr_eq(&first, &get(0)));
        assert!(!Arc::ptr_eq(&second, &get(1)));
        let entries = cache.entries.lock().unwrap();
        assert_eq!(entries.values.len(), MAX_VERIFIED_CACHE_ENTRIES);
        assert!(entries.bytes <= MAX_METADATA_TOTAL_BYTES);
    }
    #[test]
    fn retained_metadata_budget_counts_exact_bytes_without_a_second_buffer() {
        let metadata = VerifiedMetadata::default();
        let exact = serde_json::to_vec(&metadata).unwrap().len();
        assert_eq!(retained_metadata_bytes(&metadata, exact), Some(exact));
        assert_eq!(retained_metadata_bytes(&metadata, exact - 1), None);
    }
    #[test]
    fn byte_budget_evicts_before_entry_bound_and_preserves_recent_package() {
        let roots = (0..3)
            .map(|_| tempfile::tempdir().unwrap())
            .collect::<Vec<_>>();
        let materials = roots
            .iter()
            .map(|root| fixture(root.path()))
            .collect::<Vec<_>>();
        let bytes = retained_metadata_bytes(
            &load_metadata(&materials[0].0, roots[0].path(), &materials[0].1).unwrap(),
            MAX_METADATA_TOTAL_BYTES,
        )
        .unwrap();
        let cache = VerificationCache {
            identities: Default::default(),
            entries: Mutex::default(),
            max_retained_bytes: 2 * bytes,
        };
        let get = |index: usize| {
            cache
                .get(
                    &materials[index].0,
                    roots[index].path(),
                    &materials[index].1,
                )
                .unwrap()
        };
        let first = get(0);
        let second = get(1);
        get(2);
        assert!(Arc::ptr_eq(&second, &get(1)));
        assert!(!Arc::ptr_eq(&first, &get(0)));
        let entries = cache.entries.lock().unwrap();
        assert_eq!(entries.values.len(), 2);
        assert_eq!(entries.bytes, 2 * bytes);
    }
    #[test]
    fn valid_metadata_over_retention_budget_works_uncached_and_rejects_tampering() {
        let root = tempfile::tempdir().unwrap();
        let (descriptor, manifest) = fixture(root.path());
        let bytes = retained_metadata_bytes(
            &load_metadata(&descriptor, root.path(), &manifest).unwrap(),
            MAX_METADATA_TOTAL_BYTES,
        )
        .unwrap();
        let cache = VerificationCache {
            identities: Default::default(),
            entries: Mutex::default(),
            max_retained_bytes: bytes - 1,
        };
        let first = cache.get(&descriptor, root.path(), &manifest).unwrap();
        let second = cache.get(&descriptor, root.path(), &manifest).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(cache.entries.lock().unwrap().values.is_empty());
        fs::write(root.path().join("unrelated.json"), b"{}").unwrap();
        assert_eq!(
            cache
                .get(&descriptor, root.path(), &manifest)
                .err()
                .unwrap(),
            "plugin_data_artifact_digest_mismatch"
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
