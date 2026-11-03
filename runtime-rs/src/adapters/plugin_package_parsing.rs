//! Pure decoded documents only. Every caller still rereads current bytes,
//! validates installation bindings, hashes executable/metadata and checks live
//! authority. No filesystem result or authorization decision is cached here.
use super::{canonical_sha256, sha256, InstalledPluginPackage, VersionedPackageDescriptor};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const MAX_ENTRIES: usize = 16;
const MAX_ENCODED_BYTES: usize = 8 * 1_048_576;

pub(crate) struct PureMemo<T> {
    entries: Mutex<VecDeque<(String, usize, Arc<T>)>>,
}
impl<T> Default for PureMemo<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
        }
    }
}
impl<T: Serialize> PureMemo<T> {
    pub(crate) fn get(
        &self,
        bytes: &[u8],
        decode: impl FnOnce() -> Result<T, String>,
    ) -> Result<Arc<T>, String> {
        let digest = sha256(bytes);
        if let Ok(mut entries) = self.entries.lock() {
            if let Some(index) = entries.iter().position(|(key, _, _)| key == &digest) {
                let entry = entries.remove(index).unwrap();
                let result = Arc::clone(&entry.2);
                entries.push_back(entry);
                return Ok(result);
            }
        }
        let value = Arc::new(decode()?);
        // Bound the expanded retained representation too: a short YAML alias
        // document must not retain an arbitrarily large expanded Value.
        if bytes.len() <= MAX_ENCODED_BYTES {
            if let Some(charge) = retained_size(&(&digest, value.as_ref())) {
                if let Ok(mut entries) = self.entries.lock() {
                    if let Some(index) = entries.iter().position(|(key, _, _)| key == &digest) {
                        let entry = entries.remove(index).unwrap();
                        let result = Arc::clone(&entry.2);
                        entries.push_back(entry);
                        return Ok(result);
                    }
                    let mut retained: usize = entries.iter().map(|(_, size, _)| size).sum();
                    while entries.len() >= MAX_ENTRIES || retained > MAX_ENCODED_BYTES - charge {
                        retained -= entries.pop_front().unwrap().1;
                    }
                    entries.push_back((digest, charge, Arc::clone(&value)));
                }
            }
        }
        Ok(value)
    }
}

fn retained_size(value: &impl Serialize) -> Option<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= MAX_ENCODED_BYTES)
                .ok_or_else(|| std::io::Error::other("pure parse retention bound"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.0)
}

pub(crate) struct Descriptor {
    envelope: VersionedPackageDescriptor,
    value: Value,
    encoding_sha256: String,
}
impl Serialize for Descriptor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The decoded envelope and generic descriptor tree are both retained.
        (&self.value, &self.value, &self.encoding_sha256).serialize(serializer)
    }
}
impl Descriptor {
    pub(crate) fn envelope(&self) -> &VersionedPackageDescriptor {
        &self.envelope
    }
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }
    pub(crate) fn encoding_sha256(&self) -> &str {
        &self.encoding_sha256
    }
}
#[derive(Serialize)]
pub(crate) struct Manifest {
    value: Value,
    canonical_sha256: String,
    encoding_sha256: String,
}
impl Manifest {
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }
    pub(crate) fn canonical_sha256(&self) -> &str {
        &self.canonical_sha256
    }
    pub(crate) fn encoding_sha256(&self) -> &str {
        &self.encoding_sha256
    }
}
#[derive(Default)]
pub(crate) struct ParsingCache {
    record: PureMemo<InstalledPluginPackage>,
    descriptor: PureMemo<Descriptor>,
    manifest: PureMemo<Manifest>,
}
impl ParsingCache {
    pub(crate) fn record(&self, bytes: &[u8]) -> Result<Arc<InstalledPluginPackage>, String> {
        self.record.get(bytes, || {
            serde_json::from_slice(bytes).map_err(|_| "plugin_package_record_invalid".into())
        })
    }
    pub(crate) fn descriptor(&self, bytes: &[u8]) -> Result<Arc<Descriptor>, String> {
        self.descriptor.get(bytes, || {
            Ok(Descriptor {
                envelope: VersionedPackageDescriptor::decode(bytes)?,
                value: serde_json::from_slice(bytes)
                    .map_err(|_| "plugin_package_descriptor_invalid")?,
                encoding_sha256: sha256(bytes),
            })
        })
    }
    pub(crate) fn manifest(&self, bytes: &[u8]) -> Result<Arc<Manifest>, String> {
        self.manifest.get(bytes, || {
            let value =
                serde_yaml::from_slice(bytes).map_err(|_| "plugin_package_manifest_invalid")?;
            Ok(Manifest {
                canonical_sha256: canonical_sha256(&value)?,
                value,
                encoding_sha256: sha256(bytes),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_bytes_reuse_only_pure_parse_and_changed_bytes_invalidate() {
        let cache = ParsingCache::default();
        let first = cache.manifest(b"answer: 1\n").unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &cache.manifest(b"answer: 1\n").unwrap()
        ));
        let changed = cache.manifest(b"answer: 2\n").unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_ne!(first.canonical_sha256(), changed.canonical_sha256());
        let same_value = cache.manifest(b"{\"answer\":1}").unwrap();
        assert!(!Arc::ptr_eq(&first, &same_value));
        assert_eq!(first.canonical_sha256(), same_value.canonical_sha256());
    }
    #[test]
    fn short_encoding_cannot_retain_oversized_expanded_representation() {
        let cache = PureMemo::<String>::default();
        let first = cache
            .get(b"small encoding", || Ok("x".repeat(MAX_ENCODED_BYTES)))
            .unwrap();
        let second = cache
            .get(b"small encoding", || Ok("x".repeat(MAX_ENCODED_BYTES)))
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(cache.entries.lock().unwrap().is_empty());
    }
    #[test]
    fn failure_is_not_retained_and_capacity_does_not_reject_valid_documents() {
        let cache = PureMemo::<usize>::default();
        assert!(cache.get(b"bad", || Err("invalid".into())).is_err());
        assert_eq!(*cache.get(b"bad", || Ok(7)).unwrap(), 7);
        for n in 0..MAX_ENTRIES + 1 {
            cache.get(&n.to_le_bytes(), || Ok(n)).unwrap();
        }
        assert_eq!(cache.entries.lock().unwrap().len(), MAX_ENTRIES);
        let oversized = vec![b'x'; MAX_ENCODED_BYTES + 1];
        let a = cache.get(&oversized, || Ok(1)).unwrap();
        let b = cache.get(&oversized, || Ok(1)).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
    }
}
