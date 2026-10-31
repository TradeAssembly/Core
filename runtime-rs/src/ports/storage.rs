// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{SideEffectContext, VersionedPort};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct StorageWrite {
    pub namespace: String,
    pub key: String,
    pub value: Value,
    pub context: SideEffectContext,
}

impl StorageWrite {
    pub fn new(
        namespace: impl Into<String>,
        key: impl Into<String>,
        value: Value,
        context: SideEffectContext,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            key: key.into(),
            value,
            context,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StorageExpectation {
    pub namespace: String,
    pub key: String,
    pub value: Option<Value>,
}

impl StorageExpectation {
    pub fn new(namespace: impl Into<String>, key: impl Into<String>, value: Option<Value>) -> Self {
        Self {
            namespace: namespace.into(),
            key: key.into(),
            value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImmutablePutOutcome {
    Created,
    AlreadyPresent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparePutOutcome {
    Updated,
    Conflict,
}

pub trait StoragePort: VersionedPort {
    fn adapter_name(&self) -> &'static str;
    fn put_json(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<(), String>;
    fn put_json_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<ImmutablePutOutcome, String>;
    fn compare_and_put_json(
        &self,
        namespace: &str,
        key: &str,
        expected: Value,
        replacement: Value,
        context: &SideEffectContext,
    ) -> Result<ComparePutOutcome, String>;
    fn put_json_batch(
        &self,
        _writes: &[StorageWrite],
        _expectations: &[StorageExpectation],
    ) -> Result<ComparePutOutcome, String> {
        Err("atomic storage batch is unsupported".to_string())
    }
    fn get_json(&self, namespace: &str, key: &str) -> Result<Option<Value>, String>;
    fn list_json(&self, namespace: &str) -> Result<Vec<(String, Value)>, String>;
    /// Reads one lexicographically ordered page without materializing an
    /// unbounded namespace. Adapters that cannot provide this guarantee must
    /// fail closed rather than silently falling back to `list_json`.
    fn list_json_page(
        &self,
        _namespace: &str,
        _after_key: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<(String, Value)>, String> {
        Err("paged storage listing is unsupported".to_string())
    }
    /// Trusted local pre-dispatch durability barrier only. No public export route.
    fn execution_checkpoint_snapshot(&self, _destination: &std::path::Path) -> Result<(), String> {
        Err("execution_checkpoint_snapshot_unsupported".into())
    }
    /// Bounded, consistent read for the public portability allowlist.
    fn public_workspace_snapshot(
        &self,
        _namespaces: &[&str],
        _max_rows: usize,
        _max_bytes: usize,
    ) -> Result<Vec<(String, String, Value)>, String> {
        Err("workspace_snapshot_unsupported".to_string())
    }
    /// Commit to a completely empty durable workspace, or verify an exact retry.
    /// Implementations must check emptiness and write the receipt in one transaction.
    fn import_public_workspace(
        &self,
        _writes: &[StorageWrite],
        _digest: &str,
        _bootstrap: &[StorageExpectation],
    ) -> Result<ImmutablePutOutcome, String> {
        Err("workspace_import_unsupported".to_string())
    }
    fn clear_namespace(&self, namespace: &str) -> Result<(), String>;
}
