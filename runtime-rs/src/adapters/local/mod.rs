// Copyright (c) 2026 OptionLab LLC. All rights reserved.

pub mod credentials;
pub mod journal;
pub mod operations;
pub mod schema;
pub mod sqlite;
pub mod system;

use crate::capability::LocalCapabilityResolver;
use crate::finance_authority::{FinanceAuthorityPort, WardenSidecarAuthority};
use crate::ports::bus::EventBusPort;
use crate::ports::provider::{ProviderCapability, ProviderPort};
use crate::ports::{
    FailureMode, PortDescriptor, PortKind, ServiceRuntime, SideEffectContext, VersionedPort,
};
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::runtime_config::{EnvSecretResolver, RuntimeConfig, SecretResolver};

pub fn runtime(db_path: impl Into<String>) -> ServiceRuntime {
    runtime_from_config(&RuntimeConfig::local(db_path.into()))
        .expect("built-in local runtime configuration must remain valid")
}

#[doc(hidden)]
pub fn test_runtime(db_path: impl Into<String>) -> ServiceRuntime {
    let mut config = RuntimeConfig::local(db_path.into());
    config.oidc_profile = "oidc_test".to_string();
    config.oidc_issuer = "http://127.0.0.1:8080/realms/tradeassembly-dev".to_string();
    config.oidc_audience = "tradeassembly-local".to_string();
    config.oidc_client_id = "tradeassembly-local".to_string();
    runtime_from_config_with_authority(
        &config,
        Some(Arc::new(crate::finance_authority::TestFinanceAuthority)),
    )
    .expect("test local runtime configuration must remain valid")
}

pub fn runtime_from_config(config: &RuntimeConfig) -> Result<ServiceRuntime, String> {
    runtime_from_config_with_authority(config, None)
}

pub fn runtime_from_config_with_authority(
    config: &RuntimeConfig,
    authority: Option<Arc<dyn FinanceAuthorityPort>>,
) -> Result<ServiceRuntime, String> {
    config.validate()?;
    let db_path = config
        .database_path
        .clone()
        .ok_or_else(|| "local database path is required".to_string())?;
    let artifact_root = config
        .artifact_root
        .clone()
        .ok_or_else(|| "local artifact root is required".to_string())?;
    schema::assert_runtime_compatible(Path::new(&db_path))?;
    let credentials: Arc<dyn crate::ports::CredentialPort> =
        Arc::new(credentials::LocalCredentialStore::new(&db_path));
    let operations = Arc::new(operations::LocalSqliteOperations::new(
        &db_path,
        config.telemetry_enabled,
    ));
    let journal: Arc<dyn crate::ports::JournalPort> =
        Arc::new(journal::LocalJournal::new(operations.clone()));
    let finance_authority = match authority {
        Some(authority) => authority,
        None => Arc::new(WardenSidecarAuthority::new(
            &config.warden_sidecar_url,
            EnvSecretResolver.resolve(&config.warden_token_ref)?,
            &config.warden_required_version,
        )?),
    };
    let storage: Arc<dyn crate::ports::StoragePort> =
        Arc::new(sqlite::LocalSqliteStorage::new(&db_path));
    let plugin_packages: Arc<dyn crate::ports::PluginPackagePort> = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            std::path::Path::new(&artifact_root).join("plugins"),
        )?,
    );
    let studio_core_token = config
        .studio_core_token_ref
        .as_deref()
        .map(|reference| EnvSecretResolver.resolve(reference))
        .transpose()?;
    let plugins: Arc<dyn crate::ports::PluginRegistryPort> = Arc::new(
        crate::adapters::plugin_registry::StoragePluginRegistry::new(Arc::clone(&storage), "local"),
    );
    let clock: Arc<dyn crate::ports::ClockPort> = Arc::new(system::SystemClock);
    let plugin_sandbox: Arc<dyn crate::ports::PluginProcessSandboxPort> = Arc::new(
        crate::adapters::plugin_sandbox::SandboxRuntimePluginSandbox::new(
            crate::adapters::plugin_sandbox::resolve_sandbox_command(
                &config.plugin_sandbox_command,
            ),
            std::path::Path::new(&artifact_root).join("plugins/sandbox-settings"),
            config.plugin_sandbox_allow_local_egress,
        )?,
    );
    let dataset_snapshots = Arc::new(
        crate::adapters::dataset_snapshots::StorageDatasetSnapshotRepository::new(Arc::clone(
            &storage,
        )),
    );
    let backtests = Arc::new(
        crate::adapters::backtest_repository::StorageBacktestRunRepository::new(Arc::clone(
            &storage,
        )),
    );
    let capability_resolver: Arc<dyn crate::ports::CapabilityResolverPort> =
        Arc::new(LocalCapabilityResolver);
    let mut plugin_operations =
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            Arc::clone(&storage),
            Arc::clone(&plugins),
            Arc::clone(&credentials),
            Arc::clone(&plugin_packages),
            Arc::clone(&clock),
            plugin_sandbox,
        );
    // Preserve inspection/research when installation-owner state is unavailable.
    // Live readiness/dispatch remain closed without the enforcing boundary;
    // never invent an owner or substitute a caller-provided identity.
    if let Ok(owner) = crate::local_owner_identity::LocalOwnerIdentity::for_database(&db_path) {
        plugin_operations = plugin_operations.with_broker_boundary(Arc::new(
            crate::broker_submission::LocalBrokerSubmissionBoundary::new(
                crate::broker_submission::BrokerSubmissionDependencies {
                    storage: storage.clone(),
                    plugins: plugins.clone(),
                    plugin_packages: plugin_packages.clone(),
                    credentials: credentials.clone(),
                    capability_resolver: capability_resolver.clone(),
                    clock: clock.clone(),
                    leases: operations.clone(),
                    owner,
                },
                finance_authority.clone(),
            ),
        ));
    }
    let plugin_operations = Arc::new(plugin_operations);
    let robustness = Arc::new(
        crate::adapters::robustness_repository::StorageRobustnessRunRepository::new(Arc::clone(
            &storage,
        )),
    );
    let historical_data = Arc::new(
        crate::adapters::historical_data::PluginHistoricalDataAdapter::new(
            plugin_operations.clone(),
        )?,
    );
    Ok(ServiceRuntime {
        storage: Arc::clone(&storage),
        object_authorization: Arc::new(
            crate::adapters::object_authorization::StorageObjectAuthorization::new(Arc::clone(
                &storage,
            )),
        ),
        finance_authority,
        capability_resolver,
        credentials,
        historical_data,
        dataset_snapshots,
        backtests,
        robustness,
        providers: Arc::new(LocalProviderCatalog),
        plugin_operations,
        journal,
        journal_owner: None,
        legal_receipts: Arc::new(
            crate::adapters::legal_receipts::FileLegalReceiptVerifier::new(
                &config.legal_receipt_root,
                &config.legal_trusted_keys_path,
                &config.legal_policy_path,
            ),
        ),
        bus: Arc::new(LocalEventBus::default()),
        queue: operations.clone(),
        events: operations.clone(),
        scheduler: operations.clone(),
        outbox: operations.clone(),
        inbox: operations.clone(),
        leases: operations.clone(),
        identity: if config.oidc_profile == "local_owner" {
            Arc::new(system::LocalOwnerIdentityPort::new(&db_path)?)
        } else if config.oidc_profile == "workos" && config.oidc_client_id.is_empty() {
            Arc::new(system::UnconfiguredIdentity)
        } else {
            Arc::new(system::LocalOidcIdentity::with_trusted_studio_credential(
                &db_path,
                &config.oidc_issuer,
                config.identity_client_binding(),
                studio_core_token,
            )?)
        },
        policy: Arc::new(system::LocalPolicy),
        evidence: operations.clone(),
        plugins,
        plugin_packages,
        telemetry: operations,
        clock,
        exports: Arc::new(system::LocalExportStore::new(artifact_root)),
        scheduler_wake: Arc::new(crate::ports::SchedulerWake::default()),
    })
}

#[derive(Default)]
pub struct LocalEventBus {
    events: Mutex<Vec<(String, Value)>>,
}

impl VersionedPort for LocalEventBus {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::EventStream, "local.event-notifier")
            .for_profiles(&["local"])
            .with_capabilities(&["event.notify"]);
        descriptor.failure_mode = FailureMode::LocalOnly;
        vec![descriptor]
    }
}

impl EventBusPort for LocalEventBus {
    fn publish(
        &self,
        topic: &str,
        payload: Value,
        _context: &SideEffectContext,
    ) -> Result<(), String> {
        self.events
            .lock()
            .map_err(|_| "event bus lock poisoned".to_string())?
            .push((topic.to_string(), payload));
        Ok(())
    }

    fn published(&self) -> Vec<(String, Value)> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }
}

pub struct LocalProviderCatalog;

impl VersionedPort for LocalProviderCatalog {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::Plugins, "local.provider-compatibility")
                .for_profiles(&["local"])
                .with_capabilities(&["plugin.capabilities"]),
        ]
    }
}

impl ProviderPort for LocalProviderCatalog {
    fn capabilities(&self, provider_ref: &str) -> Vec<ProviderCapability> {
        vec![
            ProviderCapability {
                provider_ref: provider_ref.to_string(),
                capability: "marketdata.quote".to_string(),
                available: true,
            },
            ProviderCapability {
                provider_ref: provider_ref.to_string(),
                capability: "broker.order_submit.paper".to_string(),
                available: provider_ref.contains("paper") || provider_ref == "sim",
            },
        ]
    }
}
