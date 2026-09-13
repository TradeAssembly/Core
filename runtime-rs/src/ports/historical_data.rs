// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::historical_data::{
    AssetClass, DatasetNormalizationPolicy, DatasetQualityPolicy, DatasetSnapshot,
    DatasetSourceClass, DatasetTimeSlice, HistoricalDataKind, HistoricalObservation,
};
use crate::ports::{SideEffectContext, VersionedPort};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoricalDataRequest {
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub operation_id: String,
    pub plugin_manifest_fingerprint: String,
    pub capability_graph_revision_id: String,
    pub capability_graph_fingerprint: String,
    pub asset_class: AssetClass,
    pub instruments: Vec<String>,
    pub data_kind: HistoricalDataKind,
    pub granularity: String,
    pub time_slice: DatasetTimeSlice,
    pub calendar: String,
    pub timezone: String,
    pub normalization_policy: DatasetNormalizationPolicy,
    pub quality_policy: DatasetQualityPolicy,
    pub credential_handle: Option<String>,
    pub page_token: Option<String>,
    pub max_rows: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoricalDataPage {
    pub source_class: DatasetSourceClass,
    pub observations: Vec<HistoricalObservation>,
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub upstream_refs: BTreeMap<String, String>,
}

pub trait HistoricalDataPort: VersionedPort {
    fn acquire_page(
        &self,
        request: &HistoricalDataRequest,
        context: &SideEffectContext,
    ) -> Result<HistoricalDataPage, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotWriteOutcome {
    Created,
    AlreadyPresent,
}

pub trait DatasetSnapshotRepository: VersionedPort {
    fn put(
        &self,
        snapshot: &DatasetSnapshot,
        context: &SideEffectContext,
    ) -> Result<SnapshotWriteOutcome, String>;
    fn get(&self, dataset_id: &str) -> Result<Option<DatasetSnapshot>, String>;
    fn list(&self) -> Result<Vec<DatasetSnapshot>, String>;
}
