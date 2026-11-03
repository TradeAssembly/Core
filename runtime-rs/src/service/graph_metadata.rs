//! Bounded pure decoding reuse inside the existing immutable-package round.
//! No graph resolution, grant, instance, credential, ownership, health, clock,
//! receipt or financial result is cached. Fresh input witnesses must still match.
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub(super) struct GraphMetadata {
    pub catalog: Memo<crate::plugin_catalog::ManifestCatalog>,
    pub eligible: Memo<super::default_free_plugins::EligiblePackage>,
}
impl Default for GraphMetadata {
    fn default() -> Self {
        Self {
            catalog: Memo::new(16, 32 * 1_048_576),
            eligible: Memo::new(8, 32 * 1_048_576),
        }
    }
}
struct Entry<T> {
    witness: Value,
    value: Arc<T>,
    bytes: usize,
}
struct State<T> {
    entries: BTreeMap<String, Entry<T>>,
    bytes: usize,
}
pub(super) struct Memo<T> {
    state: Mutex<State<T>>,
    max_entries: usize,
    max_bytes: usize,
}
impl<T: Serialize> Memo<T> {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            state: Mutex::new(State {
                entries: BTreeMap::new(),
                bytes: 0,
            }),
            max_entries,
            max_bytes,
        }
    }
    pub fn get(
        &self,
        key: &str,
        witness: &Value,
        compute: impl FnOnce() -> Option<T>,
    ) -> Option<Arc<T>> {
        if let Ok(state) = self.state.lock() {
            if let Some(entry) = state
                .entries
                .get(key)
                .filter(|entry| &entry.witness == witness)
            {
                return Some(entry.value.clone());
            }
        }
        // Poisoning, capacity and oversize never turn valid metadata unavailable.
        // A failure is never memoized, so a later independent read can recover.
        let value = Arc::new(compute()?);
        if let Ok(mut state) = self.state.lock() {
            let old = state.entries.get(key).map_or(0, |e| e.bytes);
            if state.entries.contains_key(key) || state.entries.len() < self.max_entries {
                if let Some(bytes) = super::package_validation::bounded_size(
                    &(key, witness, value.as_ref()),
                    self.max_bytes - state.bytes + old,
                ) {
                    state.bytes = state.bytes - old + bytes;
                    state.entries.insert(
                        key.into(),
                        Entry {
                            witness: witness.clone(),
                            value: value.clone(),
                            bytes,
                        },
                    );
                }
            }
        }
        Some(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exact_witness_changes_and_new_round_recompute_but_never_cache_failure() {
        let cache = Memo::<Value>::new(2, 10000);
        let count = std::cell::Cell::new(0);
        let decode = || {
            count.set(count.get() + 1);
            Some(json!({"pure":count.get()}))
        };
        let first = cache
            .get("manifest", &json!({"version":1}), decode)
            .unwrap();
        assert_eq!(
            *first,
            *cache
                .get("manifest", &json!({"version":1}), || panic!(
                    "same bytes decoded twice"
                ))
                .unwrap()
        );
        assert_ne!(
            *first,
            *cache
                .get("manifest", &json!({"version":2}), decode)
                .unwrap()
        );
        assert!(cache.get("missing", &json!(null), || None).is_none());
        assert_eq!(
            *cache
                .get("missing", &json!(null), || Some(json!("recovered")))
                .unwrap(),
            json!("recovered")
        );
        let next_round = Memo::<Value>::new(2, 10000);
        next_round
            .get("manifest", &json!({"version":1}), decode)
            .unwrap();
        assert_eq!(count.get(), 3);
    }
    #[test]
    fn capacity_and_size_fall_back_without_retaining_or_truncating() {
        let cache = Memo::<Value>::new(1, 100);
        let v = "x".repeat(200);
        assert_eq!(
            *cache.get("large", &json!(1), || Some(json!(v))).unwrap(),
            json!(v)
        );
        assert!(cache.state.lock().unwrap().entries.is_empty());
        cache.get("one", &json!(1), || Some(json!(1))).unwrap();
        assert_eq!(
            *cache.get("two", &json!(2), || Some(json!(2))).unwrap(),
            json!(2)
        );
        assert_eq!(cache.state.lock().unwrap().entries.len(), 1);
        assert!(cache.state.lock().unwrap().bytes <= 100);
    }
}
