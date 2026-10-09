//! Controlled port wiring only: no process, broker network or owner credentials.
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tradeassembly_runtime::{
    adapters::plugin_operations::LocalPluginOperations, ports::*, service::TradeAssemblyService,
};

struct Package(InstalledPluginPackage);
#[test]
fn readiness_is_unavailable_for_unqualified_or_missing_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let service =
        TradeAssemblyService::test_local(temp.path().join("boundary.db").to_string_lossy());
    assert!(service
        .runtime()
        .finance_authority
        .verify_broker_boundary()
        .is_err());
    assert!(
        LocalPluginOperations::new(service.runtime().storage.clone())
            .verify_broker_boundary()
            .is_err()
    );
    let guard = Guard {
        calls: Arc::new(AtomicUsize::new(0)),
        deny: false,
    };
    assert!(
        guard.verify_available().is_err(),
        "a custom allow adapter must explicitly implement availability"
    );
}

impl VersionedPort for Package {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginPackagePort for Package {
    fn install(&self, _: &PluginPackageInstallRequest) -> Result<InstalledPluginPackage, String> {
        Err("unused".into())
    }
    fn get(&self, _: &str) -> Result<Option<InstalledPluginPackage>, String> {
        Ok(Some(self.0.clone()))
    }
    fn remove(&self, _: &str) -> Result<bool, String> {
        Err("unused".into())
    }
}
struct Sink(Arc<AtomicUsize>);
impl VersionedPort for Sink {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginProcessSandboxPort for Sink {
    fn spawn(&self, _: &PluginSandboxRequest) -> Result<SandboxedPluginProcess, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("controlled_spawn_failure".into())
    }
}
struct Guard {
    calls: Arc<AtomicUsize>,
    deny: bool,
}
impl BrokerSubmissionPort for Guard {
    fn admit(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
        prepared: &tradeassembly_runtime::ports::broker_submission::BrokerPreparedBinding,
    ) -> Result<BrokerSubmissionPermit, String> {
        assert_eq!(prepared.plugin_instance_ref, request.plugin_instance_ref);
        assert_eq!(prepared.plugin_ref, request.plugin_ref);
        assert_eq!(prepared.manifest_fingerprint, request.manifest_fingerprint);
        assert_eq!(prepared.package_sha256, "package-digest");
        assert_eq!(
            prepared.configuration_digest,
            tradeassembly_runtime::spec::canonical_hash(&json!({"mode":"live"})).unwrap()
        );
        assert_eq!(prepared.credential_ref, "unused-controlled");
        assert_eq!(prepared.credential_generation, 7);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.deny {
            return Err("controlled_admission_denial".into());
        }
        Ok(BrokerSubmissionPermit {
            envelope: tradeassembly_runtime::control_plane::ControlPlaneCommandEnvelope {
                schema_version: "controlled".into(),
                command_id: "controlled".into(),
                correlation_id: request.correlation_id.clone(),
                command_name: "order.submit.live".into(),
                command_group: "broker".into(),
                source_interface: "controlled-test".into(),
                target_object: request.account_ref.clone(),
                side_effect_class: "live_order".into(),
                authority: context.authority.clone(),
                idempotency_key: context.idempotency_key.clone(),
                idempotency_requirement: "required".into(),
                expected_sequence: None,
                evidence_refs: vec![],
                payload_hash: "controlled".into(),
                payload_preview: json!({}),
            },
        })
    }
    fn complete(
        &self,
        _: &BrokerSubmissionPermit,
        _: &PluginOperationResponse,
    ) -> Result<(), String> {
        panic!("failed sink must not complete authorization")
    }
}

#[test]
fn admission_denies_before_sink_and_uncertain_sink_is_never_redispatched() {
    for deny in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let service =
            TradeAssemblyService::test_local(directory.path().join("state.db").to_string_lossy());
        let runtime = service.runtime();
        let context = SideEffectContext::new(
            AuthorityContext {
                actor: "controlled-owner".into(),
                surface: "test".into(),
                account_mode: "live".into(),
            },
            IdempotencyKey::new("controlled-order").unwrap(),
        );
        let manifest = json!({
            "capabilities":[{"id":"broker.order_submit.live"}],
            "operations":[{"id":"broker.live_order_submit","capability":"broker.order_submit.live",
                "credentialGrantRequired":false,"traits":{"modes":["live"]}}],
            "runtime":{"timeoutSeconds":1}
        });
        runtime
            .plugins
            .install_manifest(
                "controlled.plugin",
                json!({
                    "manifest":manifest,"manifestDigest":"manifest-digest",
                    "package":{"packageSha256":"package-digest"}
                }),
                &context,
            )
            .unwrap();
        runtime.plugins.put_instance("controlled-instance", json!({
            "instanceRef":"controlled-instance","pluginRef":"controlled.plugin","enabled":true,
            "configuration":{"mode":"live"},"credentialRef":"unused-controlled",
            "activePackageSha256":"package-digest","credentialRevision":7
        }), &context).unwrap();
        let request = PluginOperationRequest {
            correlation_id: "controlled-correlation".into(),
            plugin_instance_ref: "controlled-instance".into(),
            plugin_ref: "controlled.plugin".into(),
            manifest_fingerprint: "manifest-digest".into(),
            operation_id: "broker.live_order_submit".into(),
            capability: "broker.order_submit.live".into(),
            capability_graph_revision_id: "revision".into(),
            capability_graph_fingerprint: "fingerprint".into(),
            strategy_id: "strategy".into(),
            strategy_version_id: "version".into(),
            strategy_spec_hash: "hash".into(),
            activation_id: "activation".into(),
            attempt_id: "attempt".into(),
            evaluation_tick_id: "tick".into(),
            mode: "live".into(),
            purpose: "live_order_submission".into(),
            account_ref: Some("account://controlled".into()),
            timeout_ms: 1000,
            fencing_token: Some(1),
            input: json!({"clientOrderId":"controlled-order"}),
            evidence_refs: vec![],
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let launches = Arc::new(AtomicUsize::new(0));
        let adapter = LocalPluginOperations::with_external_host(
            runtime.storage.clone(),
            runtime.plugins.clone(),
            runtime.credentials.clone(),
            Arc::new(Package(InstalledPluginPackage {
                plugin_ref: "controlled.plugin".into(),
                version: "1".into(),
                package_sha256: "package-digest".into(),
                manifest_sha256: "manifest-digest".into(),
                manifest,
                descriptor: json!({}),
                install_root: directory.path().into(),
                executable_path: directory.path().join("never-executed"),
                host_contract: "1".into(),
                sdk_version: "0.1.0".into(),
            })),
            runtime.clock.clone(),
            Arc::new(Sink(launches.clone())),
        )
        .with_broker_boundary(Arc::new(Guard {
            calls: calls.clone(),
            deny,
        }));
        let expected = if deny {
            "plugin_order_submission_denied"
        } else {
            "plugin_order_reconciliation_required"
        };
        assert_eq!(adapter.invoke(&request, &context).unwrap_err(), expected);
        assert_eq!(adapter.invoke(&request, &context).unwrap_err(), expected);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(launches.load(Ordering::SeqCst), usize::from(!deny));
        let binding = runtime
            .storage
            .get_json("plugin_operation_prepared_bindings", "controlled-order")
            .unwrap()
            .unwrap();
        assert_eq!(binding["pluginInstanceRef"], "controlled-instance");
        assert_eq!(binding["packageSha256"], "package-digest");
        assert_eq!(binding["credentialGeneration"], 7);
        assert_eq!(
            binding["configurationDigest"],
            tradeassembly_runtime::spec::canonical_hash(&json!({"mode":"live"})).unwrap()
        );
        assert!(runtime
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap()
            .is_empty());
    }
}
