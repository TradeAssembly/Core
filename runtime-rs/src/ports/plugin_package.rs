// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::VersionedPort;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPackageInstallRequest {
    pub source_type: String,
    pub locator: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    #[serde(default)]
    pub offline: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledPluginPackage {
    pub plugin_ref: String,
    pub version: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub manifest: Value,
    pub descriptor: Value,
    pub install_root: PathBuf,
    pub executable_path: PathBuf,
    pub host_contract: String,
    pub sdk_version: String,
}

/// A single current, verified package read with requested operation metadata.
/// It carries immutable data only, never invocation or account authority.
#[derive(Clone)]
pub struct OperationDataSnapshot {
    pub package: InstalledPluginPackage,
    pub contracts: std::collections::BTreeMap<
        String,
        tradeassembly_plugin_sdk::data_history::VersionedOperationData,
    >,
}

pub trait PluginPackagePort: VersionedPort {
    fn install(
        &self,
        request: &PluginPackageInstallRequest,
    ) -> Result<InstalledPluginPackage, String>;

    fn get(&self, package_sha256: &str) -> Result<Option<InstalledPluginPackage>, String>;

    /// Revalidates installed package bytes before returning pinned metadata.
    fn data_contract(
        &self,
        _package_sha256: &str,
        _operation_id: &str,
    ) -> Result<Option<tradeassembly_plugin_sdk::data_contract::OperationDataContract>, String>
    {
        Err("plugin_data_contract_loading_unsupported".into())
    }

    /// Full versioned operation metadata. Existing adapters retain their legacy
    /// implementation; no caller may discard V3 history to use the old path.
    fn operation_data(
        &self,
        package_sha256: &str,
        operation_id: &str,
    ) -> Result<Option<tradeassembly_plugin_sdk::data_history::VersionedOperationData>, String>
    {
        self.data_contract(package_sha256, operation_id)
            .map(|record| record.map(Into::into))
    }

    /// Implementations may share byte verification within this one lookup. The
    /// default preserves existing adapters; no snapshot survives a caller's
    /// current-validation boundary or replaces invocation checks.
    fn operation_data_snapshot(
        &self,
        package_sha256: &str,
        operations: &[String],
    ) -> Result<Option<OperationDataSnapshot>, String> {
        if operations.len() > tradeassembly_plugin_sdk::data_package::MAX_DATA_CONTRACTS {
            return Err("plugin_data_snapshot_bounds_invalid".into());
        }
        let Some(package) = self.get(package_sha256)? else {
            return Ok(None);
        };
        let mut contracts = std::collections::BTreeMap::new();
        for operation in operations {
            if let Some(contract) = self.operation_data(package_sha256, operation)? {
                contracts.insert(operation.clone(), contract);
            }
        }
        Ok(Some(OperationDataSnapshot { package, contracts }))
    }

    fn data_recipe(
        &self,
        _package_sha256: &str,
        _recipe_id: &str,
    ) -> Result<Option<tradeassembly_plugin_sdk::data_recipe::DataRecipe>, String> {
        Err("plugin_data_recipe_loading_unsupported".into())
    }

    fn data_schema(
        &self,
        _package_sha256: &str,
        _schema_ref: &str,
    ) -> Result<Option<Value>, String> {
        Err("plugin_data_schema_loading_unsupported".into())
    }

    fn remove(&self, package_sha256: &str) -> Result<bool, String>;
}
