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

    fn remove(&self, package_sha256: &str) -> Result<bool, String>;
}
