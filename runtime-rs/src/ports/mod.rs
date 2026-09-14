// Copyright (c) 2026 OptionLab LLC. All rights reserved.

pub mod authority;
pub mod backtest;
pub mod broker_submission;
pub mod bus;
pub mod capability;
pub mod conformance;
pub mod connected_runner;
pub mod contract;
pub mod core_runner_bridge;
pub mod credentials;
pub mod historical_data;
pub mod idempotency;
pub mod journal;
pub mod legal;
pub mod object_authorization;
pub mod operations;
pub mod paper_runner_bridge;
pub mod plugin_operation;
pub mod plugin_package;
pub mod plugin_sandbox;
pub mod provider;
pub mod reach;
pub mod robustness;
pub mod storage;
pub mod system;

use serde_json::Value;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub use crate::finance_authority::FinanceAuthorityPort;
pub use authority::AuthorityContext;
pub use backtest::{BacktestImmutableWrite, BacktestRunRepository};
pub use broker_submission::{BrokerSubmissionPermit, BrokerSubmissionPort};
pub use bus::EventBusPort;
pub use capability::{
    ApfOperationMetadata, CapabilityBinding, CapabilityBlocker, CapabilityCandidate,
    CapabilityGraphEdge, CapabilityGraphNode, CapabilityGraphRequest, CapabilityGraphResolution,
    CapabilityReadiness, CapabilityRequirement, CapabilityRequirementConstraints,
    CapabilityResolution, CapabilityResolverCatalog, CapabilityResolverPort, CapabilitySelection,
    CredentialResolutionStatus, EntitlementDecision, EntitlementGrant, EntitlementProfile,
    PluginCatalogEntry, PluginOperationContract, PluginOperationDependency, PluginOperationTraits,
};
pub use conformance::{verify_operational_ports, OperationalPortSet, PortConformanceReport};
pub use connected_runner::{
    canonical_receipt_hash, core_receipt_schema, ClaimRunnerCommandRequest,
    CompleteRunnerCommandRequest, ConnectedRunnerError, ConnectedRunnerErrorCode,
    ConnectedRunnerPoll, InstallationSignerPort, ProductRunnerTransportPort, RunnerCompletion,
    RunnerEntropyPort, RunnerHandoffReceipt, RunnerLease, RunnerOutcome, HANDOFF_RECEIPT_SCHEMA,
    INSTALLATION_AUTHENTICATION_DOMAIN, INSTALLATION_REQUEST_SCHEMA, MAX_RUNNER_RESPONSE_BYTES,
    RUNNER_LEASE_SCHEMA, RUNNER_LEASE_SECONDS,
};
pub use contract::{FailureMode, PortDescriptor, PortKind, VersionedPort, PORT_CONTRACT_VERSION};
pub use core_runner_bridge::{
    verify_core_runner_bridge, CoreRunnerBridgePort, CoreRunnerBridgeReport, RunnerBridgeCommand,
    RunnerBridgeError, RunnerBridgeErrorCode, RunnerBridgeReceipt, RunnerBridgeReceiptState,
    RunnerBridgeWorkload, CORE_RUNNER_BRIDGE_SCHEMA, CORE_RUNNER_BRIDGE_VERSION,
};
pub use credentials::{BrokerCredentials, CredentialPort, CredentialStatus};
pub use historical_data::{
    DatasetSnapshotRepository, HistoricalDataPage, HistoricalDataPort, HistoricalDataRequest,
    SnapshotWriteOutcome,
};
pub use idempotency::IdempotencyKey;
pub use journal::{JournalEvent, JournalPort};
pub use legal::{
    LegalReceiptExpectation, LegalReceiptFailure, LegalReceiptPort, VerifiedLegalReceipt,
    LEGAL_CANONICALIZATION_PROFILE, LEGAL_RECEIPT_EXPORT_SCHEMA, LEGAL_RECEIPT_SCHEMA,
    LEGAL_SIGNING_ALGORITHM,
};
pub use object_authorization::{ObjectAuthorizationPort, ObjectOwner, ObjectScope};
pub use operations::{
    DurableQueuePort, EventAppend, EventRecord, EventStreamPort, InboxRepository, LeaseClaim,
    LeaseRepository, OutboxRecord, OutboxRepository, QueueDelivery, QueueRequest, ScheduledWork,
    SchedulerPort,
};
pub use paper_runner_bridge::{
    verify_core_paper_runner, CorePaperRunnerPort, CorePaperRunnerReport, PaperRunnerCommand,
    PaperRunnerOperation, PaperRunnerOperationKind, PaperRunnerReceipt, PaperRunnerReceiptState,
    CORE_PAPER_RUNNER_SCHEMA, CORE_PAPER_RUNNER_VERSION,
};
pub use plugin_operation::{PluginOperationPort, PluginOperationRequest, PluginOperationResponse};
pub use plugin_package::{InstalledPluginPackage, PluginPackageInstallRequest, PluginPackagePort};
pub use plugin_sandbox::{PluginProcessSandboxPort, PluginSandboxRequest, SandboxedPluginProcess};
pub use provider::{ProviderCapability, ProviderPort};
pub use reach::{
    ReachCommand, ReachFinishRequest, ReachNodeRequest, ReachNodeScope, ReachOperation, ReachPoll,
    ReachReceipt, ReachRecord, ReachState, ReachTransitionRequest, ReachTransportPort,
    REACH_COMMAND_SCHEMA, REACH_NODE_REQUEST_SCHEMA, REACH_RECEIPT_SCHEMA,
};
pub use robustness::{RobustnessImmutableWrite, RobustnessRunRepository};
pub use storage::{
    ComparePutOutcome, ImmutablePutOutcome, StorageExpectation, StoragePort, StorageWrite,
};
pub use system::{
    ClockPort, EvidencePort, EvidenceRecord, ExportArtifact, ExportPort, IdentityClaims,
    IdentityPort, PluginRegistryPort, PolicyDecision, PolicyPort, PolicyRequest, TelemetryPort,
    TrustedStudioSession,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideEffectContext {
    pub authority: AuthorityContext,
    pub idempotency_key: IdempotencyKey,
    // Never deserialize runner identity from operation arguments. Only the
    // authenticated service path may carry the runner-issued capability here.
    agent_execution: Option<crate::agent_runner::VerifiedAgentMcpExecutionContext>,
}

impl SideEffectContext {
    pub fn new(authority: AuthorityContext, idempotency_key: IdempotencyKey) -> Self {
        Self {
            authority,
            idempotency_key,
            agent_execution: None,
        }
    }

    pub(crate) fn with_agent_execution(
        mut self,
        context: Option<&crate::agent_runner::VerifiedAgentMcpExecutionContext>,
    ) -> Self {
        self.agent_execution = context.cloned();
        self
    }

    /// This is identity provenance, not a dispatch permit. Consumers must
    /// revalidate its current durable run and deployment before a side effect.
    pub fn agent_execution(
        &self,
    ) -> Option<&crate::agent_runner::VerifiedAgentMcpExecutionContext> {
        self.agent_execution.as_ref()
    }
}

#[derive(Clone)]
pub struct ServiceRuntime {
    pub storage: Arc<dyn StoragePort>,
    pub object_authorization: Arc<dyn ObjectAuthorizationPort>,
    pub finance_authority: Arc<dyn FinanceAuthorityPort>,
    pub capability_resolver: Arc<dyn CapabilityResolverPort>,
    pub credentials: Arc<dyn CredentialPort>,
    pub historical_data: Arc<dyn HistoricalDataPort>,
    pub dataset_snapshots: Arc<dyn DatasetSnapshotRepository>,
    pub backtests: Arc<dyn BacktestRunRepository>,
    pub robustness: Arc<dyn RobustnessRunRepository>,
    pub providers: Arc<dyn ProviderPort>,
    pub plugin_operations: Arc<dyn PluginOperationPort>,
    pub journal: Arc<dyn JournalPort>,
    pub journal_owner: Option<ObjectOwner>,
    pub legal_receipts: Arc<dyn LegalReceiptPort>,
    pub bus: Arc<dyn EventBusPort>,
    pub queue: Arc<dyn DurableQueuePort>,
    pub events: Arc<dyn EventStreamPort>,
    pub scheduler: Arc<dyn SchedulerPort>,
    pub outbox: Arc<dyn OutboxRepository>,
    pub inbox: Arc<dyn InboxRepository>,
    pub leases: Arc<dyn LeaseRepository>,
    pub identity: Arc<dyn IdentityPort>,
    pub policy: Arc<dyn PolicyPort>,
    pub evidence: Arc<dyn EvidencePort>,
    pub plugins: Arc<dyn PluginRegistryPort>,
    pub plugin_packages: Arc<dyn PluginPackagePort>,
    pub telemetry: Arc<dyn TelemetryPort>,
    pub clock: Arc<dyn ClockPort>,
    pub exports: Arc<dyn ExportPort>,
    pub scheduler_wake: Arc<SchedulerWake>,
}

#[derive(Default)]
pub struct SchedulerWake {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl SchedulerWake {
    pub fn signal(&self) {
        if let Ok(mut generation) = self.generation.lock() {
            *generation = generation.wrapping_add(1);
            self.changed.notify_all();
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
            .lock()
            .map(|generation| *generation)
            .unwrap_or_default()
    }

    pub fn wait(&self, observed: u64, timeout: Duration) -> (u64, bool) {
        let Ok(generation) = self.generation.lock() else {
            return (observed, true);
        };
        if *generation != observed {
            return (*generation, true);
        }
        match self.changed.wait_timeout(generation, timeout) {
            Ok((generation, result)) => (*generation, !result.timed_out()),
            Err(_) => (observed, true),
        }
    }
}

impl ServiceRuntime {
    pub fn record_side_effect(
        &self,
        event_type: &str,
        payload: Value,
        context: &SideEffectContext,
    ) -> Result<String, String> {
        let journal_id = self.journal.record(JournalEvent {
            event_type: event_type.to_string(),
            authority: context.authority.clone(),
            idempotency_key: context.idempotency_key.clone(),
            payload,
            owner: self.journal_owner.clone(),
        })?;
        self.bus.publish(
            "journal.side_effect_recorded",
            serde_json::json!({
                "journal_id": journal_id,
                "event_type": event_type,
                "idempotency_key": context.idempotency_key.as_str(),
                "authority": context.authority.actor,
            }),
            context,
        )?;
        Ok(journal_id)
    }
}
