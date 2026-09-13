// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectInfo {
    pub key: String,
    pub size_bytes: u64,
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeProfileName {
    Local,
    SelfHosted,
    Production,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeProfile {
    pub name: RuntimeProfileName,
    pub database_url: String,
    pub bus_url: String,
    pub credential_backend: String,
    pub local_artifacts_allowed: bool,
    pub live_trading_allowed: bool,
}

#[derive(Clone, Debug)]
pub struct LocalFilesystemObjectStore {
    root: PathBuf,
}

impl LocalFilesystemObjectStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn name(&self) -> &'static str {
        "filesystem"
    }

    pub fn put_bytes(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> Result<ObjectInfo, String> {
        let path = self.path_for_key(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&path, bytes).map_err(|error| error.to_string())?;
        Ok(ObjectInfo {
            key: key.to_string(),
            size_bytes: bytes.len() as u64,
            content_type: content_type.map(str::to_string),
        })
    }

    pub fn get_bytes(&self, key: &str) -> Result<Vec<u8>, String> {
        fs::read(self.path_for_key(key)?).map_err(|error| error.to_string())
    }

    pub fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, String> {
        validate_key_prefix(prefix)?;
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut items = Vec::new();
        collect_objects(&self.root, &self.root, prefix, &mut items)?;
        items.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(items)
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        let path = self.path_for_key(key)?;
        if path.exists() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn path_for_key(&self, key: &str) -> Result<PathBuf, String> {
        validate_key(key)?;
        Ok(self.root.join(key))
    }
}

pub fn create_object_store(
    provider: Option<&str>,
    root: Option<PathBuf>,
    profile: Option<&RuntimeProfile>,
) -> Result<LocalFilesystemObjectStore, String> {
    let provider = provider.unwrap_or("filesystem");
    if matches!(provider, "s3" | "gcs" | "azure-blob") {
        return Err(format!(
            "hosted object store provider is not available in public core: {provider}"
        ));
    }
    if provider != "filesystem" {
        return Err(format!("unsupported object store provider: {provider}"));
    }
    let external_without_local_artifacts = profile
        .map(|profile| !profile.local_artifacts_allowed)
        .unwrap_or(false);
    if external_without_local_artifacts && root.is_none() {
        return Err("external runtime profiles require an explicit object store root".to_string());
    }
    Ok(LocalFilesystemObjectStore::new(root.unwrap_or_else(|| {
        PathBuf::from(".tradeassembly/objects")
    })))
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("object key is required".to_string());
    }
    if key.contains("://") {
        return Err("object key must be relative, not a URL".to_string());
    }
    let path = Path::new(key);
    if path.is_absolute() {
        return Err("object key must be relative".to_string());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("object key must not escape the object root".to_string());
        }
    }
    Ok(())
}

fn validate_key_prefix(prefix: &str) -> Result<(), String> {
    if prefix.is_empty() {
        return Ok(());
    }
    validate_key(prefix.trim_end_matches('/'))
}

fn collect_objects(
    root: &Path,
    current: &Path,
    prefix: &str,
    out: &mut Vec<ObjectInfo>,
) -> Result<(), String> {
    for entry in fs::read_dir(current).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_objects(root, &path, prefix, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with(prefix) {
            out.push(ObjectInfo {
                key: rel,
                size_bytes: entry.metadata().map_err(|error| error.to_string())?.len(),
                content_type: None,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("tradeassembly-storage-{name}-{stamp}"));
        fs::create_dir_all(&root).expect("temp root");
        root
    }

    #[test]
    fn local_filesystem_object_store_round_trips_relative_keys() {
        let root = temp_root("roundtrip");
        let store = LocalFilesystemObjectStore::new(root.join("objects"));

        let info = store
            .put_bytes(
                "runs/run-1/report.json",
                br#"{"ok":true}"#,
                Some("application/json"),
            )
            .expect("put object");

        assert_eq!(info.key, "runs/run-1/report.json");
        assert_eq!(info.size_bytes, 11);
        assert_eq!(
            store
                .get_bytes("runs/run-1/report.json")
                .expect("get object"),
            br#"{"ok":true}"#
        );
        assert_eq!(
            store
                .list("runs/")
                .expect("list")
                .into_iter()
                .map(|item| item.key)
                .collect::<Vec<_>>(),
            vec!["runs/run-1/report.json"]
        );

        store
            .delete("runs/run-1/report.json")
            .expect("delete object");
        assert!(store.list("runs/").expect("list empty").is_empty());
    }

    #[test]
    fn object_store_rejects_path_escape_and_absolute_keys() {
        let store = LocalFilesystemObjectStore::new(temp_root("reject").join("objects"));

        for key in ["../secret", "/absolute", "https://example.com/object"] {
            assert!(store.put_bytes(key, b"nope", None).is_err(), "{key}");
        }
    }

    #[test]
    fn external_profiles_require_explicit_object_store_root() {
        let profile = RuntimeProfile {
            name: RuntimeProfileName::SelfHosted,
            database_url: "postgresql://db/tradeassembly".to_string(),
            bus_url: "kafka:9092".to_string(),
            credential_backend: "local-file".to_string(),
            local_artifacts_allowed: false,
            live_trading_allowed: false,
        };

        assert!(create_object_store(None, None, Some(&profile)).is_err());

        let store = create_object_store(
            None,
            Some(temp_root("explicit").join("objects")),
            Some(&profile),
        )
        .expect("explicit object store");
        assert_eq!(store.name(), "filesystem");
    }

    #[test]
    fn public_factory_rejects_hosted_object_store_implementations() {
        for provider in ["s3", "gcs", "azure-blob"] {
            assert!(create_object_store(Some(provider), None, None).is_err());
        }
    }
}
