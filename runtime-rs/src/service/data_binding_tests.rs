// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::*;
use crate::ports::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct CountPackages {
    inner: Arc<dyn PluginPackagePort>,
    snapshots: AtomicUsize,
    direct_gets: AtomicUsize,
}
impl VersionedPort for CountPackages {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.inner.descriptors()
    }
}
impl PluginPackagePort for CountPackages {
    fn install(
        &self,
        request: &PluginPackageInstallRequest,
    ) -> Result<InstalledPluginPackage, String> {
        self.inner.install(request)
    }
    fn get(&self, digest: &str) -> Result<Option<InstalledPluginPackage>, String> {
        self.direct_gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(digest)
    }
    fn remove(&self, digest: &str) -> Result<bool, String> {
        self.inner.remove(digest)
    }
    fn operation_data_snapshot(
        &self,
        digest: &str,
        operations: &[String],
    ) -> Result<Option<OperationDataSnapshot>, String> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        self.inner.operation_data_snapshot(digest, operations)
    }
}
fn selected(
    service: &TradeAssemblyService,
    request: &crate::data_pipeline::PipelineRequest,
) -> CapabilitySelection {
    let revision =
        super::super::capability_graph::get_revision(service, &request.capability_graph_revision);
    let graph: crate::ports::CapabilityGraphResolution =
        serde_json::from_value(revision["revision"]["graph"].clone()).unwrap();
    graph
        .nodes
        .into_iter()
        .filter_map(|n| n.selected)
        .find(|s| s.operation_id == "derive")
        .unwrap()
}
fn fixture() -> (
    tempfile::TempDir,
    TradeAssemblyService,
    CapabilitySelection,
    Arc<CountPackages>,
) {
    let (dir, base, _, request) = super::super::data_plans::tests::fixture_service_with_history(
        None,
        tradeassembly_plugin_sdk::data_contract::StatePartition::Instrument,
        "paper",
        true,
    );
    let selection = selected(&base, &request);
    let count = Arc::new(CountPackages {
        inner: base.runtime().plugin_packages.clone(),
        snapshots: AtomicUsize::new(0),
        direct_gets: AtomicUsize::new(0),
    });
    let mut runtime = (*base.runtime()).clone();
    runtime.plugin_packages = count.clone();
    let service = TradeAssemblyService::from_runtime(base.db(), runtime);
    (dir, service, selection, count)
}
fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).unwrap(),
    )
}
#[test]
fn operation_metadata_round_batches_immutable_reads_but_keeps_instance_pins_current() {
    let (_dir, s, selection, count) = fixture();
    let operations = vec![selection.operation_id.clone()];
    let mut round = OperationMetadataRound::default();
    let (first, contract) = round.pin_operation(&s, &selection, &operations).unwrap();
    assert!(
        contract.unwrap().v3().is_some(),
        "history semantics must not downgrade"
    );
    let mut instance = s
        .runtime()
        .plugins
        .get_instance(&selection.plugin_instance_ref)
        .unwrap()
        .unwrap();
    instance["configuration"] = json!({"changed":true});
    instance["credentialRevision"] = json!(2);
    s.runtime()
        .plugins
        .put_instance(
            &selection.plugin_instance_ref,
            instance.clone(),
            &context("change-binding"),
        )
        .unwrap();
    let (changed, _) = round.pin_operation(&s, &selection, &operations).unwrap();
    assert_ne!(first.context_sha256, changed.context_sha256);
    assert_eq!(count.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(count.direct_gets.load(Ordering::SeqCst), 0);
    instance["enabled"] = json!(false);
    s.runtime()
        .plugins
        .put_instance(
            &selection.plugin_instance_ref,
            instance.clone(),
            &context("disable-binding"),
        )
        .unwrap();
    assert_eq!(
        round
            .pin_operation(&s, &selection, &operations)
            .unwrap_err(),
        "data_plan_instance_stale"
    );
    instance["enabled"] = json!(true);
    s.runtime()
        .plugins
        .put_instance(
            &selection.plugin_instance_ref,
            instance,
            &context("enable-binding"),
        )
        .unwrap();
    let mut fresh = OperationMetadataRound::default();
    fresh.pin_operation(&s, &selection, &operations).unwrap();
    assert_eq!(
        count.snapshots.load(Ordering::SeqCst),
        2,
        "next validation must read package bytes again"
    );
}
#[test]
fn next_operation_metadata_round_refuses_tampered_artifact_after_a_warm_read() {
    let (_dir, s, selection, count) = fixture();
    let operations = vec![selection.operation_id.clone()];
    let mut round = OperationMetadataRound::default();
    let (pin, _) = round.pin_operation(&s, &selection, &operations).unwrap();
    let package = count.inner.get(&pin.package_sha256).unwrap().unwrap();
    std::fs::write(package.executable_path, b"changed artifact").unwrap();
    let mut fresh = OperationMetadataRound::default();
    assert_eq!(
        fresh
            .pin_operation(&s, &selection, &operations)
            .unwrap_err(),
        "plugin_package_integrity_invalid"
    );
    assert_eq!(count.snapshots.load(Ordering::SeqCst), 2);
}
struct DefaultSnapshotPackages(Arc<dyn PluginPackagePort>);
impl VersionedPort for DefaultSnapshotPackages {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.0.descriptors()
    }
}
impl PluginPackagePort for DefaultSnapshotPackages {
    fn install(&self, r: &PluginPackageInstallRequest) -> Result<InstalledPluginPackage, String> {
        self.0.install(r)
    }
    fn get(&self, d: &str) -> Result<Option<InstalledPluginPackage>, String> {
        self.0.get(d)
    }
    fn remove(&self, d: &str) -> Result<bool, String> {
        self.0.remove(d)
    }
    fn operation_data(
        &self,
        d: &str,
        o: &str,
    ) -> Result<Option<tradeassembly_plugin_sdk::data_history::VersionedOperationData>, String>
    {
        self.0.operation_data(d, o)
    }
}
#[test]
fn default_snapshot_adapter_preserves_versioned_metadata_and_missing_operation_semantics() {
    let (_dir, s, selection, count) = fixture();
    let mut runtime = (*s.runtime()).clone();
    runtime.plugin_packages = Arc::new(DefaultSnapshotPackages(count.inner.clone()));
    let service = TradeAssemblyService::from_runtime(s.db(), runtime);
    let ops = vec![selection.operation_id.clone()];
    let mut round = OperationMetadataRound::default();
    let (_, contract) = round.pin_operation(&service, &selection, &ops).unwrap();
    assert!(contract.unwrap().v3().is_some());
    let mut missing = selection.clone();
    missing.operation_id = "missing".into();
    let (_, contract) = round.pin_operation(&service, &missing, &ops).unwrap();
    assert!(contract.is_none());
}
