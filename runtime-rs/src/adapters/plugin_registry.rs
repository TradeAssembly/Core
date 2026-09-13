// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    FailureMode, PluginRegistryPort, PortDescriptor, PortKind, SideEffectContext, StoragePort,
    VersionedPort,
};
use serde_json::Value;
use std::sync::Arc;

const MANIFEST_NAMESPACE: &str = "plugin_manifests";
const MANIFEST_REVISION_NAMESPACE: &str = "plugin_manifest_revisions";
const INSTANCE_NAMESPACE: &str = "plugin_instances_v2";
pub(crate) const RESET_NAMESPACES: &[&str] = &[INSTANCE_NAMESPACE];

pub struct StoragePluginRegistry {
    storage: Arc<dyn StoragePort>,
    profile: &'static str,
}

impl StoragePluginRegistry {
    pub fn new(storage: Arc<dyn StoragePort>, profile: &'static str) -> Self {
        Self { storage, profile }
    }

    fn list_active(&self, namespace: &str) -> Result<Vec<Value>, String> {
        let mut records = self
            .storage
            .list_json(namespace)?
            .into_iter()
            .map(|(_, value)| value)
            .filter(|value| value.get("removedAtMs").is_none_or(Value::is_null))
            .collect::<Vec<_>>();
        records.sort_by_key(|value| {
            value
                .get("pluginRef")
                .or_else(|| value.get("instanceRef"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        Ok(records)
    }
}

impl VersionedPort for StoragePluginRegistry {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(
            PortKind::Plugins,
            format!("{}.storage-plugin-registry", self.profile),
        )
        .for_profiles(&[self.profile])
        .with_capabilities(&[
            "plugin.manifest.install",
            "plugin.manifest.list",
            "plugin.instance.configure",
            "plugin.instance.list",
        ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl PluginRegistryPort for StoragePluginRegistry {
    fn list_manifests(&self) -> Result<Vec<Value>, String> {
        self.list_active(MANIFEST_NAMESPACE)
    }

    fn get_manifest(&self, plugin_ref: &str) -> Result<Option<Value>, String> {
        self.storage.get_json(MANIFEST_NAMESPACE, plugin_ref)
    }

    fn install_manifest(
        &self,
        plugin_ref: &str,
        record: Value,
        context: &SideEffectContext,
    ) -> Result<Value, String> {
        let digest = record
            .get("manifestDigest")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "plugin manifest digest is required".to_string())?;
        let revision_key = format!("{plugin_ref}@{digest}");
        if let Some(existing) = self
            .storage
            .get_json(MANIFEST_REVISION_NAMESPACE, &revision_key)?
        {
            if existing == record {
                return Ok(existing);
            }
            return Err(format!("plugin manifest revision {revision_key} conflicts"));
        }
        self.storage.put_json(
            MANIFEST_REVISION_NAMESPACE,
            &revision_key,
            record.clone(),
            context,
        )?;
        if self.get_manifest(plugin_ref)?.is_none() {
            self.storage
                .put_json(MANIFEST_NAMESPACE, plugin_ref, record.clone(), context)?;
        }
        Ok(record)
    }

    fn get_manifest_revision(
        &self,
        plugin_ref: &str,
        manifest_digest: &str,
    ) -> Result<Option<Value>, String> {
        self.storage.get_json(
            MANIFEST_REVISION_NAMESPACE,
            &format!("{plugin_ref}@{manifest_digest}"),
        )
    }

    fn activate_manifest_revision(
        &self,
        plugin_ref: &str,
        manifest_digest: &str,
        context: &SideEffectContext,
    ) -> Result<Value, String> {
        let record = self
            .get_manifest_revision(plugin_ref, manifest_digest)?
            .ok_or_else(|| {
                format!("plugin manifest revision {plugin_ref}@{manifest_digest} is not installed")
            })?;
        self.storage
            .put_json(MANIFEST_NAMESPACE, plugin_ref, record.clone(), context)?;
        Ok(record)
    }

    fn list_instances(&self) -> Result<Vec<Value>, String> {
        self.list_active(INSTANCE_NAMESPACE)
    }

    fn get_instance(&self, instance_ref: &str) -> Result<Option<Value>, String> {
        self.storage.get_json(INSTANCE_NAMESPACE, instance_ref)
    }

    fn put_instance(
        &self,
        instance_ref: &str,
        record: Value,
        context: &SideEffectContext,
    ) -> Result<Value, String> {
        self.storage
            .put_json(INSTANCE_NAMESPACE, instance_ref, record.clone(), context)?;
        Ok(record)
    }
}
