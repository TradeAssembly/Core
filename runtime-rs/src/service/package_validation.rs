//! Immutable package-byte reuse inside one pure financial validation round.
//! The original runtime's operation port is deliberately never wrapped. Every
//! independent approval, C5 admission/revalidation and dispatch starts fresh.
use super::TradeAssemblyService;
use crate::ports::{
    InstalledPluginPackage, OperationDataSnapshot, PluginPackageInstallRequest, PluginPackagePort,
    PortDescriptor, VerifiedPackageMetadataSnapshot, VersionedPort,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex},
};
use tradeassembly_plugin_sdk::{data_history::VersionedOperationData, data_recipe::DataRecipe};

const MAX_CACHED_PACKAGES: usize = 32;
const MAX_CACHED_METADATA_BYTES: usize = 64 * 1_048_576;

/// This callback is used only by pure entry/management validators. It clones
/// the existing service (including invocation identity) without bootstrap,
/// reauthentication, storage mutation, or replacement of any authority port.
/// Nested validators receive this same scoped service, without opening another
/// round. No cached service is returned or stored by callers.
pub(super) fn with_round<T>(
    service: &TradeAssemblyService,
    validate: impl FnOnce(&TradeAssemblyService) -> Result<T, String>,
) -> Result<T, String> {
    let mut runtime = (*service.runtime()).clone();
    runtime.plugin_packages = Arc::new(ValidationPackages::new(runtime.plugin_packages.clone()));
    let mut scoped = service.clone();
    scoped.runtime = Arc::new(runtime);
    validate(&scoped)
}

#[derive(Default)]
struct Cache {
    packages: BTreeMap<String, Arc<VerifiedPackageMetadataSnapshot>>,
    bytes: usize,
}
struct ValidationPackages {
    inner: Arc<dyn PluginPackagePort>,
    cache: Mutex<Cache>,
    max_packages: usize,
    max_bytes: usize,
}
impl ValidationPackages {
    fn new(inner: Arc<dyn PluginPackagePort>) -> Self {
        Self {
            inner,
            cache: Mutex::new(Cache::default()),
            max_packages: MAX_CACHED_PACKAGES,
            max_bytes: MAX_CACHED_METADATA_BYTES,
        }
    }
    fn snapshot(
        &self,
        digest: &str,
    ) -> Result<Option<Arc<VerifiedPackageMetadataSnapshot>>, String> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| "package_validation_unavailable")?;
        if let Some(snapshot) = cache.packages.get(digest) {
            return Ok(Some(Arc::clone(snapshot)));
        }
        // Cache eligibility never changes a valid package's availability.
        // When full, use the original uncached port, including byte checks.
        if cache.packages.len() >= self.max_packages || cache.bytes >= self.max_bytes {
            return Ok(None);
        }
        let Some(snapshot) = self.inner.verified_metadata_snapshot(digest)? else {
            return Ok(None);
        };
        if snapshot.package.package_sha256 != digest {
            return Err("package_validation_binding_mismatch".into());
        }
        // Count serialization without allocating another unbounded byte vector.
        // An oversized snapshot remains usable in this call, but is not retained.
        let mut size = BoundedSize {
            bytes: 0,
            limit: self.max_bytes - cache.bytes,
        };
        let eligible = serde_json::to_writer(&mut size, &snapshot).is_ok();
        let snapshot = Arc::new(snapshot);
        if eligible {
            cache.bytes += size.bytes;
            cache.packages.insert(digest.into(), Arc::clone(&snapshot));
        }
        Ok(Some(snapshot))
    }
}
struct BoundedSize {
    bytes: usize,
    limit: usize,
}
impl Write for BoundedSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|total| *total <= self.limit)
            .ok_or_else(|| std::io::Error::other("metadata cache ineligible"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl VersionedPort for ValidationPackages {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.inner.descriptors()
    }
}
impl PluginPackagePort for ValidationPackages {
    fn install(&self, _: &PluginPackageInstallRequest) -> Result<InstalledPluginPackage, String> {
        Err("package_validation_read_only".into())
    }
    fn remove(&self, _: &str) -> Result<bool, String> {
        Err("package_validation_read_only".into())
    }
    fn get(&self, digest: &str) -> Result<Option<InstalledPluginPackage>, String> {
        match self.snapshot(digest)? {
            Some(snapshot) => Ok(Some(snapshot.package.clone())),
            None => self.inner.get(digest),
        }
    }
    fn verified_metadata_snapshot(
        &self,
        digest: &str,
    ) -> Result<Option<VerifiedPackageMetadataSnapshot>, String> {
        self.snapshot(digest)
            .map(|snapshot| snapshot.map(|snapshot| (*snapshot).clone()))
    }
    fn operation_data(
        &self,
        digest: &str,
        operation: &str,
    ) -> Result<Option<VersionedOperationData>, String> {
        match self.snapshot(digest)? {
            Some(snapshot) => Ok(snapshot.contracts.get(operation).cloned()),
            None => self.inner.operation_data(digest, operation),
        }
    }
    fn data_contract(
        &self,
        digest: &str,
        operation: &str,
    ) -> Result<Option<tradeassembly_plugin_sdk::data_contract::OperationDataContract>, String>
    {
        match self.snapshot(digest)? {
            Some(snapshot) => snapshot
                .contracts
                .get(operation)
                .cloned()
                .map(VersionedOperationData::into_legacy)
                .transpose(),
            None => self.inner.data_contract(digest, operation),
        }
    }
    fn operation_data_snapshot(
        &self,
        digest: &str,
        operations: &[String],
    ) -> Result<Option<OperationDataSnapshot>, String> {
        if operations.len() > tradeassembly_plugin_sdk::data_package::MAX_DATA_CONTRACTS {
            return Err("plugin_data_snapshot_bounds_invalid".into());
        }
        match self.snapshot(digest)? {
            Some(snapshot) => Ok(Some(OperationDataSnapshot {
                package: snapshot.package.clone(),
                contracts: operations
                    .iter()
                    .filter_map(|id| {
                        snapshot
                            .contracts
                            .get(id)
                            .map(|contract| (id.clone(), contract.clone()))
                    })
                    .collect(),
            })),
            None => self.inner.operation_data_snapshot(digest, operations),
        }
    }
    fn data_schema(&self, digest: &str, schema: &str) -> Result<Option<Value>, String> {
        match self.snapshot(digest)? {
            Some(snapshot) => Ok(snapshot.schemas.get(schema).cloned()),
            None => self.inner.data_schema(digest, schema),
        }
    }
    fn data_recipe(&self, digest: &str, recipe: &str) -> Result<Option<DataRecipe>, String> {
        match self.snapshot(digest)? {
            Some(snapshot) => Ok(snapshot.recipes.get(recipe).cloned()),
            None => self.inner.data_recipe(digest, recipe),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    // Authored package-port observations only; no operation/process dispatch.
    struct CountingPackages {
        snapshots: AtomicUsize,
        reads: AtomicUsize,
        schemas: AtomicUsize,
        tampered: AtomicBool,
        optimized: bool,
    }
    impl CountingPackages {
        fn new(optimized: bool) -> Self {
            Self {
                snapshots: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
                schemas: AtomicUsize::new(0),
                tampered: AtomicBool::new(false),
                optimized,
            }
        }
        fn check(&self) -> Result<(), String> {
            if self.tampered.load(Ordering::SeqCst) {
                Err("fixture_artifact_tampered".into())
            } else {
                Ok(())
            }
        }
        fn package(digest: &str) -> InstalledPluginPackage {
            InstalledPluginPackage {
                plugin_ref: "fixture.package".into(),
                version: "1".into(),
                package_sha256: digest.into(),
                manifest_sha256: "b".repeat(64),
                manifest: json!({"version":1}),
                descriptor: json!({}),
                install_root: "/fixture/never-executed".into(),
                executable_path: "/fixture/never-executed/plugin".into(),
                host_contract: "1".into(),
                sdk_version: "1".into(),
            }
        }
    }
    impl VersionedPort for CountingPackages {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            vec![]
        }
    }
    impl PluginPackagePort for CountingPackages {
        fn install(
            &self,
            _: &PluginPackageInstallRequest,
        ) -> Result<InstalledPluginPackage, String> {
            panic!("pure validation installed a package")
        }
        fn remove(&self, _: &str) -> Result<bool, String> {
            panic!("pure validation removed a package")
        }
        fn get(&self, digest: &str) -> Result<Option<InstalledPluginPackage>, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.check()?;
            Ok(Some(Self::package(digest)))
        }
        fn verified_metadata_snapshot(
            &self,
            digest: &str,
        ) -> Result<Option<VerifiedPackageMetadataSnapshot>, String> {
            self.snapshots.fetch_add(1, Ordering::SeqCst);
            if !self.optimized {
                return Ok(None);
            }
            self.check()?;
            Ok(Some(VerifiedPackageMetadataSnapshot {
                package: Self::package(digest),
                contracts: BTreeMap::new(),
                schemas: BTreeMap::from([(
                    "schema://fixture/value@1".into(),
                    json!({"type":"number"}),
                )]),
                recipes: BTreeMap::new(),
            }))
        }
        fn data_schema(&self, _: &str, _: &str) -> Result<Option<Value>, String> {
            self.schemas.fetch_add(1, Ordering::SeqCst);
            self.check()?;
            Ok(Some(json!({"type":"number"})))
        }
    }
    #[test]
    fn round_shares_package_and_schema_but_next_round_rechecks_tampering() {
        let inner = Arc::new(CountingPackages::new(true));
        let digest = "a".repeat(64);
        let round = ValidationPackages::new(inner.clone());
        for _ in 0..15 {
            assert!(round.get(&digest).unwrap().is_some());
            assert_eq!(
                round
                    .data_schema(&digest, "schema://fixture/value@1")
                    .unwrap(),
                Some(json!({"type":"number"}))
            );
        }
        assert_eq!(inner.snapshots.load(Ordering::SeqCst), 1);
        assert_eq!(inner.reads.load(Ordering::SeqCst), 0);
        assert_eq!(inner.schemas.load(Ordering::SeqCst), 0);
        drop(round);
        inner.tampered.store(true, Ordering::SeqCst);
        assert_eq!(
            ValidationPackages::new(inner.clone())
                .get(&digest)
                .unwrap_err(),
            "fixture_artifact_tampered"
        );
        assert_eq!(inner.snapshots.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn cache_capacity_and_oversize_preserve_uncached_availability_and_tamper_checks() {
        let inner = Arc::new(CountingPackages::new(true));
        let round = ValidationPackages::new(inner.clone());
        for i in 0..MAX_CACHED_PACKAGES {
            assert!(round.get(&format!("{i:064x}")).unwrap().is_some());
        }
        let extra = format!("{:064x}", MAX_CACHED_PACKAGES);
        assert!(round.get(&extra).unwrap().is_some());
        assert_eq!(inner.reads.load(Ordering::SeqCst), 1);
        inner.tampered.store(true, Ordering::SeqCst);
        assert_eq!(round.get(&extra).unwrap_err(), "fixture_artifact_tampered");
        assert_eq!(
            round.cache.lock().unwrap().packages.len(),
            MAX_CACHED_PACKAGES
        );
        inner.tampered.store(false, Ordering::SeqCst);
        let mut small = ValidationPackages::new(inner.clone());
        small.max_bytes = 1;
        assert!(small.get(&extra).unwrap().is_some());
        assert_eq!(small.cache.lock().unwrap().bytes, 0);
        assert!(small.cache.lock().unwrap().packages.is_empty());
        inner.tampered.store(true, Ordering::SeqCst);
        assert_eq!(small.get(&extra).unwrap_err(), "fixture_artifact_tampered");
    }
    #[test]
    fn unsupported_snapshot_adapter_falls_back_without_caching_or_mutation() {
        let inner = Arc::new(CountingPackages::new(false));
        let round = ValidationPackages::new(inner.clone());
        let d = "a".repeat(64);
        for _ in 0..2 {
            round.get(&d).unwrap();
            round.data_schema(&d, "schema://fixture/value@1").unwrap();
        }
        assert_eq!(inner.reads.load(Ordering::SeqCst), 2);
        assert_eq!(inner.schemas.load(Ordering::SeqCst), 2);
        assert_eq!(
            round.remove(&d).unwrap_err(),
            "package_validation_read_only"
        );
        let request = PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: "unused".into(),
            package_sha256: d,
            manifest_sha256: "b".repeat(64),
            offline: true,
        };
        assert_eq!(
            round.install(&request).unwrap_err(),
            "package_validation_read_only"
        );
    }
    #[test]
    fn scoped_service_keeps_authority_ports_and_original_dispatch_port() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("round.db").to_string_lossy().into_owned();
        let mut runtime = crate::adapters::local::test_runtime(&db);
        let packages = Arc::new(CountingPackages::new(true));
        runtime.plugin_packages = packages.clone();
        let s = TradeAssemblyService::from_runtime(&db, runtime)
            .for_authenticated_invocation("fixture", "owner", None, None);
        s.bind_owned_object("plugin_instance", "fixture.owner-bound")
            .unwrap();
        let original = s.runtime();
        let d = "a".repeat(64);
        with_round(&s, |round| {
            assert_eq!(round.invocation_owner(), s.invocation_owner());
            assert!(round
                .require_object("plugin_instance", "fixture.owner-bound")
                .is_ok());
            assert_eq!(round.db(), s.db());
            let r = round.runtime();
            assert!(Arc::ptr_eq(
                &r.plugin_operations,
                &original.plugin_operations
            ));
            assert!(Arc::ptr_eq(&r.credentials, &original.credentials));
            assert!(Arc::ptr_eq(&r.plugins, &original.plugins));
            assert!(Arc::ptr_eq(&r.storage, &original.storage));
            assert!(Arc::ptr_eq(
                &r.object_authorization,
                &original.object_authorization
            ));
            assert!(Arc::ptr_eq(
                &r.capability_resolver,
                &original.capability_resolver
            ));
            assert!(Arc::ptr_eq(&r.clock, &original.clock));
            r.plugin_packages.get(&d)?;
            let nested = |same_round: &TradeAssemblyService| {
                same_round
                    .runtime()
                    .plugin_packages
                    .data_schema(&d, "schema://fixture/value@1")?;
                Ok(())
            };
            nested(round)
        })
        .unwrap();
        assert_eq!(packages.snapshots.load(Ordering::SeqCst), 1);
        with_round(&s, |round| {
            round.runtime().plugin_packages.get(&d)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(packages.snapshots.load(Ordering::SeqCst), 2);
        assert!(Arc::ptr_eq(
            &s.runtime().plugin_packages,
            &original.plugin_packages
        ));
        let other = s.for_authenticated_invocation("fixture", "different-owner", None, None);
        with_round(&other, |round| {
            assert!(round
                .require_object("plugin_instance", "fixture.owner-bound")
                .is_err());
            Ok(())
        })
        .unwrap();
        assert_eq!(packages.snapshots.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn cached_package_never_caches_current_configuration_credentials_or_revocation() {
        use super::super::{capability_graph, data_binding, data_plans};
        use crate::ports::CapabilityGraphResolution;
        let (_dir, s, scope, request) = data_plans::tests::fixture_service(None);
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("package-validation-current").unwrap(),
        );
        let saved = data_plans::save(&s, scope, request.clone(), &context).unwrap();
        let revision = capability_graph::get_revision(&s, &request.capability_graph_revision);
        let graph: CapabilityGraphResolution =
            serde_json::from_value(revision["revision"]["graph"].clone()).unwrap();
        let selected = graph
            .nodes
            .iter()
            .filter_map(|n| n.selected.as_ref())
            .find(|n| n.operation_id == "provide")
            .unwrap();
        let runtime = s.runtime();
        let original = runtime
            .plugins
            .get_instance(&selected.plugin_instance_ref)
            .unwrap()
            .unwrap();
        with_round(&s, |round| {
            assert_eq!(data_plans::current(round, &saved.revision_id)?, saved);
            let pin = data_binding::pin(round, selected)?;
            let mut changed = original.clone();
            changed["configuration"] = json!({"revision":"changed"});
            runtime.plugins.put_instance(
                &selected.plugin_instance_ref,
                changed.clone(),
                &context,
            )?;
            assert_ne!(
                data_binding::pin(round, selected)?.context_sha256,
                pin.context_sha256
            );
            assert_eq!(
                data_plans::current(round, &saved.revision_id).unwrap_err(),
                "data_plan_stale"
            );
            changed = original.clone();
            changed["credentialRevision"] = json!(2);
            runtime.plugins.put_instance(
                &selected.plugin_instance_ref,
                changed.clone(),
                &context,
            )?;
            assert_ne!(
                data_binding::pin(round, selected)?.context_sha256,
                pin.context_sha256
            );
            changed["enabled"] = json!(false);
            runtime
                .plugins
                .put_instance(&selected.plugin_instance_ref, changed, &context)?;
            assert_eq!(
                data_binding::pin(round, selected).unwrap_err(),
                "data_plan_instance_stale"
            );
            assert_eq!(
                data_plans::current(round, &saved.revision_id).unwrap_err(),
                "data_plan_graph_stale"
            );
            Ok(())
        })
        .unwrap();
    }

    struct MovingClock(std::sync::atomic::AtomicI64);
    impl VersionedPort for MovingClock {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            vec![]
        }
    }
    impl crate::ports::ClockPort for MovingClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    #[derive(Default)]
    struct NoDispatch(AtomicUsize);
    impl VersionedPort for NoDispatch {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            vec![]
        }
    }
    impl crate::ports::PluginOperationPort for NoDispatch {
        fn invoke(
            &self,
            _: &crate::ports::PluginOperationRequest,
            _: &SideEffectContext,
        ) -> Result<crate::ports::PluginOperationResponse, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("unexpected_dispatch".into())
        }
    }
    #[test]
    fn current_graph_observes_entitlement_expiry_during_a_cached_round_without_dispatch() {
        use super::super::data_plans;
        let operations = Arc::new(NoDispatch::default());
        let (_dir, mut s, scope, mut request) =
            data_plans::tests::fixture_service(Some(operations.clone()));
        let now = s.runtime().clock.now_ms();
        let clock = Arc::new(MovingClock(std::sync::atomic::AtomicI64::new(now)));
        let mut runtime = (*s.runtime()).clone();
        runtime.clock = clock.clone();
        s.runtime = Arc::new(runtime);
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("cached-round-expiry").unwrap(),
        );
        let mut grant = s
            .runtime()
            .storage
            .get_json("entitlement_grants", "test.provide")
            .unwrap()
            .unwrap();
        grant["expiresAt"] = json!(chrono::DateTime::from_timestamp_millis(now + 5000)
            .unwrap()
            .to_rfc3339());
        s.runtime()
            .storage
            .put_json("entitlement_grants", "test.provide", grant, &context)
            .unwrap();
        // Changing an entitlement's terms changes its decision reference even
        // before expiry. Pin a NEW normal graph after that authorized change;
        // the original fixture graph must not be silently rewritten or reused.
        let graph = super::super::capability_graph::save_revision(
            &s,
            json!({"configId":"cfg_cached_entitlement_fixture","evaluationEpoch":chrono::DateTime::from_timestamp_millis(now).unwrap().to_rfc3339(),"bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":request.mode,"additionalRequirements":[{"requirementId":"derived","capability":"test.derive","mode":request.mode,"declaration":"required","fallbackPolicy":"none"}]}),
        );
        assert_eq!(graph["revision"]["graph"]["ok"], true, "{graph}");
        request.capability_graph_revision =
            graph["revision"]["revisionId"].as_str().unwrap().into();
        let saved = data_plans::save(&s, scope, request, &context).unwrap();
        with_round(&s, |round| {
            assert_eq!(data_plans::current(round, &saved.revision_id)?, saved);
            clock.0.store(now + 6000, Ordering::SeqCst);
            assert_eq!(
                data_plans::current(round, &saved.revision_id).unwrap_err(),
                "data_plan_graph_stale"
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(operations.0.load(Ordering::SeqCst), 0);
    }
}
