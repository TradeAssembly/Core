// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::historical_data::DatasetSnapshot;
use crate::ports::{
    DatasetSnapshotRepository, FailureMode, ImmutablePutOutcome, PortDescriptor, PortKind,
    SideEffectContext, SnapshotWriteOutcome, StoragePort, VersionedPort,
};
use std::sync::Arc;

const SNAPSHOTS_NS: &str = "dataset_snapshots_v1";

pub struct StorageDatasetSnapshotRepository {
    storage: Arc<dyn StoragePort>,
}

impl StorageDatasetSnapshotRepository {
    pub fn new(storage: Arc<dyn StoragePort>) -> Self {
        Self { storage }
    }
}

impl VersionedPort for StorageDatasetSnapshotRepository {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(
            PortKind::Storage,
            format!("{}.dataset-snapshots", self.storage.adapter_name()),
        )
        .for_profiles(&["local", "self_hosted"])
        .with_capabilities(&[
            "dataset.snapshot.immutable",
            "dataset.snapshot.integrity_verify",
        ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl DatasetSnapshotRepository for StorageDatasetSnapshotRepository {
    fn put(
        &self,
        snapshot: &DatasetSnapshot,
        context: &SideEffectContext,
    ) -> Result<SnapshotWriteOutcome, String> {
        snapshot.verify()?;
        if let Some(existing) = self.get(&snapshot.dataset_id)? {
            return if existing.content_hash == snapshot.content_hash {
                Ok(SnapshotWriteOutcome::AlreadyPresent)
            } else {
                Err("dataset_snapshot_write_conflict".to_string())
            };
        }
        let value = serde_json::to_value(snapshot)
            .map_err(|_| "dataset_snapshot_serialization_failed".to_string())?;
        match self
            .storage
            .put_json_if_absent(SNAPSHOTS_NS, &snapshot.dataset_id, value, context)
        {
            Ok(ImmutablePutOutcome::Created) => Ok(SnapshotWriteOutcome::Created),
            Ok(ImmutablePutOutcome::AlreadyPresent) => Ok(SnapshotWriteOutcome::AlreadyPresent),
            Err(error) if error == "immutable_storage_conflict" => {
                match self.get(&snapshot.dataset_id)? {
                    Some(existing) if existing.content_hash == snapshot.content_hash => {
                        Ok(SnapshotWriteOutcome::AlreadyPresent)
                    }
                    Some(_) => Err("dataset_snapshot_write_conflict".to_string()),
                    None => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    fn get(&self, dataset_id: &str) -> Result<Option<DatasetSnapshot>, String> {
        self.storage
            .get_json(SNAPSHOTS_NS, dataset_id)?
            .map(|value| {
                let snapshot: DatasetSnapshot = serde_json::from_value(value)
                    .map_err(|_| "dataset_snapshot_integrity_failed".to_string())?;
                snapshot.verify()?;
                Ok(snapshot)
            })
            .transpose()
    }

    fn list(&self) -> Result<Vec<DatasetSnapshot>, String> {
        self.storage
            .list_json(SNAPSHOTS_NS)?
            .into_iter()
            .map(|(_, value)| {
                let snapshot: DatasetSnapshot = serde_json::from_value(value)
                    .map_err(|_| "dataset_snapshot_integrity_failed".to_string())?;
                snapshot.verify()?;
                Ok(snapshot)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::local::sqlite::LocalSqliteStorage;
    use crate::historical_data::{
        AssetClass, BarObservation, DatasetNormalizationPolicy, DatasetQualityPolicy,
        DatasetSnapshotContent, DatasetSourceClass, DatasetSourceManifest, DatasetTimeSlice,
        HistoricalDataKind, HistoricalObservation, HistoricalObservationData, QualityDisposition,
        DATASET_SNAPSHOT_SCHEMA,
    };
    use crate::ports::{AuthorityContext, IdempotencyKey};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn path() -> String {
        tempfile::Builder::new()
            .prefix("tradeassembly-dataset-snapshot-")
            .suffix(".db")
            .tempfile()
            .expect("temporary database")
            .into_temp_path()
            .keep()
            .expect("persist temporary database path")
            .to_string_lossy()
            .to_string()
    }

    fn context() -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("dataset-snapshot-test").expect("key"),
        )
    }

    fn snapshot() -> DatasetSnapshot {
        DatasetSnapshot::build(
            DatasetSnapshotContent {
                schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
                strategy_id: "strategy_1".to_string(),
                strategy_version_id: None,
                source: DatasetSourceManifest {
                    source_class: DatasetSourceClass::Test,
                    plugin_instance_ref: "local-data".to_string(),
                    plugin_ref: "tradeassembly.local-data".to_string(),
                    operation_id: "marketdata.bars.read_v1".to_string(),
                    plugin_manifest_fingerprint: "sha256:manifest".to_string(),
                    capability_graph_revision_id: "graph_1".to_string(),
                    capability_graph_fingerprint: "sha256:graph".to_string(),
                    source_request_hash: "sha256:request".to_string(),
                    upstream_refs: BTreeMap::new(),
                },
                asset_classes: vec![AssetClass::Equity],
                instruments: vec!["SPY".to_string()],
                data_kind: HistoricalDataKind::Bars,
                granularity: "1d".to_string(),
                time_slice: DatasetTimeSlice {
                    start: "2026-01-01T00:00:00Z".to_string(),
                    end: "2026-01-03T00:00:00Z".to_string(),
                },
                calendar: "XNYS".to_string(),
                timezone: "America/New_York".to_string(),
                normalization_policy: DatasetNormalizationPolicy {
                    timestamp_unit: "rfc3339".to_string(),
                    timezone: "UTC".to_string(),
                    duplicate_policy: "reject".to_string(),
                    price_adjustment: "split_dividend_adjusted".to_string(),
                },
                quality_policy: DatasetQualityPolicy {
                    missing_intervals: QualityDisposition::Warn,
                    stale_observations: QualityDisposition::Reject,
                    outliers: QualityDisposition::Warn,
                    invalid_markets: QualityDisposition::Reject,
                    calendar_mismatch: QualityDisposition::Reject,
                    corporate_action_gaps: QualityDisposition::Warn,
                    missing_derivative_fields: QualityDisposition::Reject,
                },
                quality_findings: vec![],
                observations: vec![HistoricalObservation {
                    instrument_id: "SPY".to_string(),
                    timestamp: "2026-01-02T21:00:00Z".to_string(),
                    data: HistoricalObservationData::Bar(BarObservation {
                        open: 598.0,
                        high: 601.0,
                        low: 597.0,
                        close: 600.0,
                        volume: 1_000_000.0,
                        vwap: None,
                        session: None,
                    }),
                }],
            },
            100,
        )
        .expect("snapshot")
    }

    #[test]
    fn immutable_snapshot_survives_restart_and_deduplicates() {
        let db = path();
        let stored = snapshot();
        let first = StorageDatasetSnapshotRepository::new(Arc::new(LocalSqliteStorage::new(&db)));
        assert_eq!(
            first.put(&stored, &context()).expect("create"),
            SnapshotWriteOutcome::Created
        );
        assert_eq!(
            first.put(&stored, &context()).expect("dedupe"),
            SnapshotWriteOutcome::AlreadyPresent
        );
        let rebuilt = DatasetSnapshot::build(stored.content.clone(), 101).expect("rebuilt");
        assert_eq!(rebuilt.dataset_id, stored.dataset_id);
        assert_eq!(
            first.put(&rebuilt, &context()).expect("content retry"),
            SnapshotWriteOutcome::AlreadyPresent
        );
        drop(first);

        let restarted =
            StorageDatasetSnapshotRepository::new(Arc::new(LocalSqliteStorage::new(&db)));
        assert_eq!(
            restarted.get(&stored.dataset_id).expect("read"),
            Some(stored)
        );
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn immutable_snapshot_rejects_conflicting_persisted_value_and_tamper() {
        let db = path();
        let stored = snapshot();
        let storage: Arc<dyn StoragePort> = Arc::new(LocalSqliteStorage::new(&db));
        let repository = StorageDatasetSnapshotRepository::new(Arc::clone(&storage));
        repository.put(&stored, &context()).expect("create");

        let mut conflicting = serde_json::to_value(&stored).expect("value");
        conflicting["createdAtMs"] = json!(101);
        assert_eq!(
            storage
                .put_json_if_absent(SNAPSHOTS_NS, &stored.dataset_id, conflicting, &context(),)
                .expect_err("immutable conflict"),
            "immutable_storage_conflict"
        );

        let mut tampered = serde_json::to_value(&stored).expect("value");
        tampered["content"]["observations"][0]["data"]["data"]["close"] = json!(1.0);
        storage
            .put_json(SNAPSHOTS_NS, &stored.dataset_id, tampered, &context())
            .expect("simulate disk owner tamper");
        assert_eq!(
            repository
                .get(&stored.dataset_id)
                .expect_err("tamper rejected"),
            "dataset_snapshot_integrity_failed"
        );
        let _ = std::fs::remove_file(db);
    }
}
