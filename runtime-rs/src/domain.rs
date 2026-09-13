// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainLayer {
    TradeAssemblyCore,
    ApfCore,
    ApfFinanceProfile,
    ApfFederation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mutability {
    Immutable,
    Mutable,
    AppendOnly,
    DerivedProjection,
    EphemeralObservation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordContractStatus {
    Canonical,
    CompatibilityOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordContract {
    pub name: &'static str,
    pub layer: DomainLayer,
    pub mutability: Mutability,
    pub status: RecordContractStatus,
    pub lifecycle_reason: &'static str,
}

pub fn canonical_record_contracts() -> Vec<RecordContract> {
    vec![
        record("Strategy", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "names the user-owned strategy family across drafts, versions, research, and execution"),
        record("StrategyDraft", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "captures mutable authoring state before an immutable version is published"),
        record("StrategyVersion", DomainLayer::TradeAssemblyCore, Mutability::Immutable, "snapshots one StrategySpec so backtests and executions never point at mutable logic"),
        record("StrategySpec", DomainLayer::TradeAssemblyCore, Mutability::Immutable, "stores broker-neutral strategy logic without credentials, account bindings, or activation authority"),
        record("StrategyExecutionConfig", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "owns revision history for one StrategyVersion without owning executable state"),
        record("StrategyExecutionConfigRevision", DomainLayer::TradeAssemblyCore, Mutability::Immutable, "snapshots concrete runtime bindings and user-owned risk configuration for activation or research"),
        compatibility_record("ExecutionRun", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "preserves legacy import and API shapes while callers migrate to Activation or ExecutionAttempt"),
        record("Activation", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "stores the durable continuous-execution mandate that survives UI and worker restarts"),
        record("ExecutionAttempt", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "records one supervised worker tenure or restart beneath an Activation"),
        record("ExecutionSession", DomainLayer::TradeAssemblyCore, Mutability::EphemeralObservation, "tracks observable Studio, CLI, MCP, or Sightline session state without owning runtime truth"),
        record("BacktestRun", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "records one reproducible research run for one immutable StrategyVersion and manifest"),
        record("RobustnessRun", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "records Monte Carlo, sweep, sensitivity, or robustness analysis tied to immutable inputs"),
        record("Comparison", DomainLayer::TradeAssemblyCore, Mutability::Immutable, "stores a reviewed comparison of strategy versions or run artifacts"),
        record("DatasetIngestion", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "records one idempotent historical-data acquisition attempt and its monotonic lifecycle"),
        record("DataSnapshot", DomainLayer::TradeAssemblyCore, Mutability::Immutable, "pins data source, slice, fingerprint, and replay inputs used by research or runtime"),
        record("PluginManifest", DomainLayer::TradeAssemblyCore, Mutability::Immutable, "declares plugin capabilities, operations, permissions, schemas, and forbidden behavior"),
        record("PluginInstance", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "stores local installation, config, connection, and health state for one manifest"),
        record("CapabilityResolution", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "maps abstract requirements to candidate or selected plugin capabilities without mutating StrategySpec"),
        record("EntitlementGrant", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "represents feature availability and limits; it does not authorize an action by itself"),
        record("Approval", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "captures explicit user or delegated approval before a protected side effect is consumed once"),
        record("OrderIntent", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "captures broker-neutral order intent before approval, authority, and plugin submission"),
        record("DecisionRecord", DomainLayer::TradeAssemblyCore, Mutability::AppendOnly, "records strategy evaluation output and the evidence that produced it"),
        record("EventLedger", DomainLayer::TradeAssemblyCore, Mutability::AppendOnly, "stores replayable domain, control, authority, and side-effect events"),
        record("ReportArtifact", DomainLayer::TradeAssemblyCore, Mutability::Immutable, "stores exportable backtest, execution, comparison, or evidence report artifacts"),
        record("SchedulerLease", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "prevents duplicate active execution under local and serverless worker profiles"),
        record("EvaluationTick", DomainLayer::TradeAssemblyCore, Mutability::AppendOnly, "records one durable idempotent evaluation beneath an ExecutionAttempt"),
        record("RuntimeCheckpoint", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "stores monotonic feed, plugin, broker, and replay cursors for restart safety"),
        record("RuntimeWatch", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "persists stream subscription intent independent of Studio being open"),
        record("OutboxRecord", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "dedupes pending event publication and external side effects"),
        record("InboxRecord", DomainLayer::TradeAssemblyCore, Mutability::AppendOnly, "dedupes external plugin, broker, scheduler, and gateway messages"),
        record("ReconciliationState", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "records local-vs-external state comparison after callbacks, polls, or restart"),
        record("RiskReservation", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "reserves and releases risk exposure for pending order intent"),
        record("BrokerAccountBinding", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "binds a plugin instance to a broker or data account identity and mode"),
        record("AccountAuthorityDecision", DomainLayer::TradeAssemblyCore, Mutability::AppendOnly, "proves a mode, account, and action was allowed before the side effect"),
        record("DelegatedAuthorityToken", DomainLayer::TradeAssemblyCore, Mutability::Mutable, "keeps scoped, expiring delegated authority compatible with future policy fabric"),
        record("User", DomainLayer::ApfCore, Mutability::Mutable, "identifies the human principal without assuming a hosted identity system"),
        record("Identity", DomainLayer::ApfCore, Mutability::Mutable, "normalizes local, development, OIDC, SAML-brokered, or future identity refs"),
        record("Organization", DomainLayer::ApfCore, Mutability::Mutable, "creates the tenant, personal, or local install boundary"),
        record("Membership", DomainLayer::ApfCore, Mutability::Mutable, "connects users to organization scope, roles, and external groups"),
        record("Actor", DomainLayer::ApfCore, Mutability::Mutable, "represents user, agent, plugin, worker, adapter, service account, or support actor"),
        record("RoleBinding", DomainLayer::ApfCore, Mutability::Mutable, "records role assignments scoped to resources without replacing policy decisions"),
        record("ResourceRelationship", DomainLayer::ApfCore, Mutability::Mutable, "records ReBAC edges such as owner, delegate, support, runtime, or evidence relation"),
        record("OrgEntitlementSnapshot", DomainLayer::ApfCore, Mutability::DerivedProjection, "captures feature availability and limits separate from authorization"),
        record("PolicyDecision", DomainLayer::ApfCore, Mutability::AppendOnly, "records point-in-time allow, deny, or step-up result with inputs and policy version"),
        record("APFCapabilityGrant", DomainLayer::ApfCore, Mutability::Mutable, "delegates bounded capability to an agent, plugin, worker, adapter, or service actor"),
        record("LegalDocumentVersion", DomainLayer::ApfCore, Mutability::Immutable, "versions legal text and acknowledgement requirements"),
        record("LegalAcknowledgement", DomainLayer::ApfCore, Mutability::AppendOnly, "proves a user accepted a legal document version in context"),
        record("EvidenceBundle", DomainLayer::ApfCore, Mutability::Immutable, "seals hashes, refs, redaction policy, and proof inputs for meaningful actions"),
        record("HostedEvidenceReceipt", DomainLayer::ApfCore, Mutability::Immutable, "verifies hosted or external durable receipt mirrors without moving hosted custody into core"),
        record("EvidenceRetentionPolicy", DomainLayer::ApfCore, Mutability::Mutable, "controls retention, legal hold, tombstone, export, and deletion behavior"),
        record("EvidenceIntegrityIncident", DomainLayer::ApfCore, Mutability::AppendOnly, "records hash, chain, receipt, or escrow integrity failure"),
        record("APFPolicyBundle", DomainLayer::ApfCore, Mutability::Immutable, "stores versioned policy package, hash, signature, owner, scope, expiry, and failure mode"),
        record("APFPolicyProjection", DomainLayer::ApfCore, Mutability::DerivedProjection, "stores evaluator-specific policy input for local, gateway, sidecar, or hosted enforcement"),
        record("AgentRegistration", DomainLayer::ApfCore, Mutability::Mutable, "registers agent id, version, publisher, signing keys, runtime class, tools, and attestations"),
        record("ToolManifest", DomainLayer::ApfCore, Mutability::Immutable, "declares resource server tools, actions, effects, schemas, risk class, credential, and evidence needs"),
        record("SessionAdmission", DomainLayer::ApfCore, Mutability::AppendOnly, "decides whether a user-agent-application session is admitted and under what autonomy bounds"),
        record("ActionAuthorizationDecision", DomainLayer::ApfCore, Mutability::AppendOnly, "answers a concrete action request against mandate, policy, evidence, context, and PEP coverage"),
        record("CredentialGrant", DomainLayer::ApfCore, Mutability::Mutable, "mints short-lived non-exportable downstream credential authority after allow decision"),
        record("DetectorVerdict", DomainLayer::ApfCore, Mutability::AppendOnly, "normalizes risk, security, fraud, anomaly, or policy signals with TTL and failure mode"),
        record("APFEvidenceEvent", DomainLayer::ApfCore, Mutability::AppendOnly, "stores hash-chained APF event evidence with actor, session, action, resource, and policy digests"),
        record("FinanceActionDescriptor", DomainLayer::ApfFinanceProfile, Mutability::Immutable, "maps raw calls into canonical finance action, resource, context, purpose, and evidence fields"),
        record("APFMandate", DomainLayer::ApfCore, Mutability::Mutable, "stores durable delegated authority that can narrow but never widen parent policy"),
        record("APFReceipt", DomainLayer::ApfCore, Mutability::Immutable, "proves decisions, approvals, credential grants, action attempts, outcomes, and integrity hashes"),
        record("APFContextSnapshot", DomainLayer::ApfCore, Mutability::Immutable, "stores redacted, hashed, encrypted, or escrow-referenced decision context such as Sightline refs"),
        record("APFExtensionSchema", DomainLayer::ApfCore, Mutability::Mutable, "versions typed customer fields for users, access, mandates, receipts, hooks, and policy inputs"),
        record("PolicyIntegrationHook", DomainLayer::ApfCore, Mutability::Mutable, "registers external policy, context, detector, evidence, approval, or risk hooks behind a typed interface"),
        record("PEPRegistration", DomainLayer::ApfCore, Mutability::Mutable, "declares enforcement point coverage class and block capability"),
        record("FederatedCapabilityGrant", DomainLayer::ApfFederation, Mutability::Immutable, "stores signed cross-system grant with issuer, audience, subject, capabilities, denials, expiry, and signature"),
        record("ClientContext", DomainLayer::ApfCore, Mutability::Immutable, "represents the client tuple field, including local personal client=self semantics"),
        record("PurposeTaxonomy", DomainLayer::ApfCore, Mutability::Mutable, "versions controlled purpose ids and customer purpose packs"),
        record("ConfigurableCheckPack", DomainLayer::ApfCore, Mutability::Mutable, "groups customer-defined checks with schemas, digests, failure mode, and conformance fixtures"),
        record("FinanceProfileResource", DomainLayer::ApfFinanceProfile, Mutability::Immutable, "names finance resources such as account, portfolio, instrument, strategy, order, or evidence"),
        record("FinanceActionClass", DomainLayer::ApfFinanceProfile, Mutability::Immutable, "names governed finance action classes and no-action boundaries for TradeAssembly"),
        record("FinanceRiskGate", DomainLayer::ApfFinanceProfile, Mutability::Mutable, "defines configurable risk checks such as notional, account, mode, symbol, approval, or kill switch"),
        record("FinanceComplianceHook", DomainLayer::ApfFinanceProfile, Mutability::Mutable, "defines configurable supervision, books-and-records, customer-data, and retention hooks"),
    ]
}

fn record(
    name: &'static str,
    layer: DomainLayer,
    mutability: Mutability,
    lifecycle_reason: &'static str,
) -> RecordContract {
    RecordContract {
        name,
        layer,
        mutability,
        status: RecordContractStatus::Canonical,
        lifecycle_reason,
    }
}

fn compatibility_record(
    name: &'static str,
    layer: DomainLayer,
    mutability: Mutability,
    lifecycle_reason: &'static str,
) -> RecordContract {
    RecordContract {
        name,
        layer,
        mutability,
        status: RecordContractStatus::CompatibilityOnly,
        lifecycle_reason,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStatus {
    Draft,
    Published,
    Active,
    Archived,
    Deprecated,
    Blocked,
    Failed,
    Stopped,
}

impl LifecycleStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Active => "active",
            Self::Archived => "archived",
            Self::Deprecated => "deprecated",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRecord {
    pub strategy_id: String,
    pub status: LifecycleStatus,
    pub current_version_id: Option<String>,
    pub created_by_actor_id: String,
    pub workspace_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyDraftRecord {
    pub draft_id: String,
    pub strategy_id: String,
    pub draft_hash: String,
    pub state: StrategyDraftState,
    pub spec_fragment: Value,
    pub base_version_id: Option<String>,
    pub authority_context_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyVersionRecord {
    pub version_id: String,
    pub strategy_id: String,
    pub source_schema_id: String,
    pub original_spec_json: String,
    pub original_spec_hash: String,
    pub canonical_spec_json: String,
    pub canonical_spec_hash: String,
    pub state: StrategyVersionState,
    pub published_from_draft_id: String,
    pub provenance_event_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyExecutionConfigRecord {
    pub config_id: String,
    pub strategy_id: String,
    pub version_id: String,
    pub current_revision_id: Option<String>,
    pub created_by_actor_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyExecutionConfigRevisionRecord {
    pub config_revision_id: String,
    pub config_id: String,
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub revision: u64,
    pub mode_scope: ModeScope,
    pub universe_bindings: Vec<String>,
    pub symbol_bindings: BTreeMap<String, String>,
    pub parameter_overrides: BTreeMap<String, Value>,
    pub risk_budget_ref: String,
    pub risk_scale_ref: String,
    pub capability_resolution_ids: Vec<String>,
    pub plugin_instance_ids: Vec<String>,
    pub account_binding_id: Option<String>,
    pub opaque_credential_handle_refs: Vec<String>,
    pub clock_overrides: BTreeMap<String, Value>,
    pub config_hash: String,
    pub published_event_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[deprecated(
    note = "compatibility-only ExecutionRun API; use ActivationRecord or ExecutionAttemptRecord"
)]
pub struct ExecutionRunRecord {
    pub run_id: String,
    pub config_id: String,
    pub strategy_id: String,
    pub version_id: String,
    pub state: ExecutionRunState,
    pub readiness: ReadinessSnapshot,
    /// Legacy API projection only; ActivationRecord owns its lifecycle identity.
    pub activation_ids: Vec<String>,
    pub report_artifact_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActivationRecord {
    pub activation_id: String,
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub config_revision_id: String,
    pub mandate_id: String,
    pub authority_context_ref: String,
    pub exclusivity_scope: String,
    pub dashboard_report_scope_ref: String,
    pub state: ActivationState,
    pub control_version: u64,
    pub active_lease_ids: Vec<String>,
    pub latest_checkpoint_id: Option<String>,
    pub started_event_id: Option<String>,
    pub stopped_event_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAttemptState {
    Starting,
    Running,
    Completed,
    Failed,
    Replaced,
    LeaseLost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionAttemptCommand {
    Start,
    EvaluateTick,
    Complete,
    Fail,
    Replace,
    LoseLease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAttemptStartCause {
    InitialActivation,
    Restart,
    Recovery,
    LeaseReplacement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAttemptEndCause {
    Completed,
    Failed,
    Replaced,
    LeaseLost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAttemptHealth {
    Starting,
    Healthy,
    Degraded,
    Unhealthy,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionAttemptRecord {
    pub attempt_id: String,
    pub activation_id: String,
    pub worker_id: String,
    pub worker_epoch: u64,
    pub fenced_lease_id: String,
    pub state: ExecutionAttemptState,
    pub start_cause: ExecutionAttemptStartCause,
    pub started_event_id: Option<String>,
    pub end_cause: Option<ExecutionAttemptEndCause>,
    pub ended_event_id: Option<String>,
    pub health: ExecutionAttemptHealth,
    pub diagnostic_refs: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationTriggerSource {
    Scheduled,
    MarketData,
    BrokerEvent,
    PluginEvent,
    Manual,
    Replay,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationTickRecord {
    pub tick_id: String,
    pub execution_attempt_id: String,
    pub activation_id: String,
    pub tick_sequence: u64,
    pub trigger_source: EvaluationTriggerSource,
    pub market_time: String,
    pub event_time: String,
    pub input_cursor: String,
    pub compiled_plan_hash: String,
    pub data_refs: Vec<String>,
    pub decision_refs: Vec<String>,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionSessionRecord {
    pub session_id: String,
    pub activation_id: String,
    pub surface_id: String,
    pub actor_id: String,
    pub state: SessionSurfaceState,
    pub last_observed_event_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BacktestRunRecord {
    pub run_id: String,
    pub strategy_id: String,
    pub version_id: String,
    pub strategy_version_hash: String,
    pub data_snapshot_id: String,
    pub manifest_hash: String,
    pub state: BacktestRunState,
    pub report_artifact_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RobustnessRunRecord {
    pub run_id: String,
    pub strategy_id: String,
    pub version_id: String,
    pub source_backtest_run_ids: Vec<String>,
    pub scenario_manifest_hash: String,
    pub state: RobustnessRunState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapabilityResolutionRecord {
    pub resolution_id: String,
    pub requirement_hash: String,
    pub catalog_hash: String,
    pub state: CapabilityResolutionState,
    pub candidates: Vec<CapabilityCandidate>,
    pub selected_plugin_instance_id: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityCandidate {
    pub capability_ref: String,
    pub plugin_manifest_id: String,
    pub plugin_instance_id: Option<String>,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntitlementGrantRecord {
    pub grant_id: String,
    pub principal_scope: String,
    pub resource_scope: String,
    pub capability_ref: String,
    pub operation_ref: String,
    pub mode_scope: ModeScope,
    pub effect: GrantEffect,
    pub limits: BTreeMap<String, Value>,
    pub precedence: GrantPrecedence,
    pub source_profile: String,
    pub state: EntitlementGrantState,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub approval_id: String,
    pub target_ref: String,
    pub snapshot_hash: String,
    pub state: ApprovalState,
    pub approver_actor_id: Option<String>,
    pub expires_at: String,
    pub consumed_by_event_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderIntentRecord {
    pub intent_id: String,
    pub activation_id: String,
    pub decision_id: String,
    pub account_binding_id: String,
    pub state: OrderIntentState,
    pub quote_ref: String,
    pub risk_reservation_id: String,
    pub approval_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub decision_id: String,
    pub activation_id: String,
    pub evaluation_tick_id: String,
    pub strategy_version_hash: String,
    pub input_refs: Vec<String>,
    pub output_hash: String,
    pub order_intent_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventLedgerRecord {
    pub ledger_id: String,
    pub event_id: String,
    pub event_type: String,
    pub idempotency_key: String,
    pub authority_context_ref: String,
    pub previous_event_hash: Option<String>,
    pub event_hash: String,
    pub payload_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportArtifactRecord {
    pub artifact_id: String,
    pub artifact_type: String,
    pub source_ref: String,
    pub redaction_policy_hash: String,
    pub payload_hash: String,
    pub export_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRecord {
    pub user_id: String,
    pub display_ref: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub identity_id: String,
    pub user_id: String,
    pub identity_kind: IdentityKind,
    pub external_subject_ref: String,
    pub assurance_level: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    Local,
    TradeAssemblyId,
    Oidc,
    SamlBrokered,
    Dev,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganizationRecord {
    pub org_id: String,
    pub org_kind: OrganizationKind,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationKind {
    Personal,
    Team,
    Enterprise,
    TradeAssemblyInternal,
    Local,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipRecord {
    pub membership_id: String,
    pub user_id: String,
    pub org_id: String,
    pub status: String,
    pub source: String,
    pub role_refs: Vec<String>,
    pub external_group_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorRecord {
    pub actor_id: String,
    pub actor_kind: ActorKind,
    pub user_id: Option<String>,
    pub org_id: Option<String>,
    pub agent_id: Option<String>,
    pub plugin_instance_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    Agent,
    Plugin,
    Worker,
    Adapter,
    ServiceAccount,
    SupportSession,
    CiWorker,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleBindingRecord {
    pub binding_id: String,
    pub actor_id: String,
    pub role_id: String,
    pub resource_ref: String,
    pub issued_by_actor_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRelationshipRecord {
    pub relationship_id: String,
    pub subject_ref: String,
    pub relation: String,
    pub resource_ref: String,
    pub evidence_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrgEntitlementSnapshotRecord {
    pub snapshot_id: String,
    pub org_id: String,
    pub profile_id: String,
    pub feature_limits: BTreeMap<String, Value>,
    pub deny_refs: Vec<String>,
    pub policy_version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyDecisionRecord {
    pub decision_id: String,
    pub actor_id: String,
    pub action: String,
    pub resource_ref: String,
    pub decision: PolicyEffect,
    pub policy_version: String,
    pub evidence_refs: Vec<String>,
    pub reason_codes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFCapabilityGrantRecord {
    pub grant_id: String,
    pub subject_actor_id: String,
    pub capability_ref: String,
    pub resource_ref: String,
    pub expires_at: String,
    pub limits: BTreeMap<String, Value>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegalDocumentVersionRecord {
    pub document_id: String,
    pub version_id: String,
    pub locale: String,
    pub content_hash: String,
    pub effective_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegalAcknowledgementRecord {
    pub acknowledgement_id: String,
    pub document_id: String,
    pub version_id: String,
    pub user_id: String,
    pub context_ref: String,
    pub evidence_bundle_id: String,
    pub accepted_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceBundleRecord {
    pub bundle_id: String,
    pub evidence_class: String,
    pub payload_policy: ReceiptPayloadPolicy,
    pub evidence_refs: Vec<String>,
    pub bundle_hash: String,
    pub retention_policy_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedEvidenceReceiptRecord {
    pub receipt_id: String,
    pub bundle_id: String,
    pub issuer: String,
    pub signature: String,
    pub issued_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRetentionPolicyRecord {
    pub retention_policy_id: String,
    pub retention_class: String,
    pub legal_hold: bool,
    pub tombstone_policy: String,
    pub export_policy: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFPolicyBundleRecord {
    pub bundle_id: String,
    pub owner_ref: String,
    pub tenant_ref: String,
    pub policy_version: String,
    pub bundle_hash: String,
    pub signature: String,
    pub failure_mode: FailureMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFPolicyProjectionRecord {
    pub projection_id: String,
    pub bundle_id: String,
    pub target: String,
    pub projection_hash: String,
    pub generated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRegistrationRecord {
    pub agent_id: String,
    pub version: String,
    pub publisher_ref: String,
    pub signing_key_ref: String,
    pub runtime_classes: Vec<String>,
    pub allowed_tool_refs: Vec<String>,
    pub risk_tier: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolManifestRecord {
    pub tool_ref: String,
    pub action: String,
    pub effect: SideEffectClass,
    pub resource_type: String,
    pub risk_class: String,
    pub input_schema_ref: String,
    pub output_schema_ref: String,
    pub credential_needs: Vec<String>,
    pub evidence_requirements: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionAdmissionRecord {
    pub admission_id: String,
    pub session_id: String,
    pub user_id: String,
    pub agent_id: String,
    pub application: String,
    pub decision: PolicyEffect,
    pub max_autonomy: SupervisionMode,
    pub expires_at: String,
    pub evidence_event_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinanceActionDescriptorRecord {
    pub descriptor_id: String,
    pub raw_operation_ref: String,
    pub action_class: String,
    pub actor_id: String,
    pub principal_user_id: String,
    pub client: ClientContext,
    pub application: String,
    pub resource_ref: String,
    pub context_snapshot_id: String,
    pub purpose: PurposeContext,
    pub evidence_refs: Vec<String>,
    pub requested_time_window: TimeWindow,
    pub requested_supervision_mode: SupervisionMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFMandateRecord {
    pub mandate_id: String,
    pub parent_mandate_id: Option<String>,
    pub tuple: MandateDecisionTuple,
    pub status: MandateStatus,
    pub limits: BTreeMap<String, Value>,
    pub issued_at: String,
    pub expires_at: String,
    pub evidence_refs: Vec<String>,
    pub mandate_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionAuthorizationDecisionRecord {
    pub decision_id: String,
    pub descriptor_id: String,
    pub mandate_id: Option<String>,
    pub decision: PolicyEffect,
    pub policy_version: String,
    pub projection_digest: String,
    pub pep_coverage: PepCoverageClass,
    pub required_supervision_mode: SupervisionMode,
    pub evidence_refs: Vec<String>,
    pub reason_codes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CredentialGrantRecord {
    pub credential_grant_id: String,
    pub decision_id: String,
    pub credential_handle_ref: String,
    pub session_id: String,
    pub action: String,
    pub resource_ref: String,
    pub expires_at: String,
    pub non_exportable: bool,
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DetectorVerdictRecord {
    pub verdict_id: String,
    pub source_ref: String,
    pub category: String,
    pub severity: String,
    pub confidence: f64,
    pub ttl_seconds: u64,
    pub action_ref: String,
    pub resource_ref: String,
    pub raw_ref: String,
    pub failure_mode: FailureMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFEvidenceEventRecord {
    pub event_id: String,
    pub actor_id: String,
    pub session_id: String,
    pub action: String,
    pub resource_ref: String,
    pub effect: PolicyEffect,
    pub policy_digest: String,
    pub projection_digest: String,
    pub trace_id: String,
    pub previous_hash: Option<String>,
    pub event_hash: String,
    pub receipt_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFReceiptRecord {
    pub receipt_id: String,
    pub principal_user: String,
    pub client: ClientContext,
    pub agent: String,
    pub application: String,
    pub action: String,
    pub resource: String,
    pub purpose: PurposeContext,
    pub policy_version: String,
    pub mandate_hash: Option<String>,
    pub decision: PolicyEffect,
    pub supervision_mode: SupervisionMode,
    pub evidence_refs: Vec<String>,
    pub context_snapshot_id: String,
    pub limits: BTreeMap<String, Value>,
    pub approver: Option<String>,
    pub credential_grant_id: Option<String>,
    pub downstream_outcome_ref: Option<String>,
    pub payload_policy: ReceiptPayloadPolicy,
    pub receipt_hash: String,
    pub previous_receipt_hash: Option<String>,
    pub timestamp: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFContextSnapshotRecord {
    pub snapshot_id: String,
    pub payload_policy: ReceiptPayloadPolicy,
    pub source_refs: Vec<String>,
    pub context_hash: String,
    pub redaction_policy_hash: String,
    pub encrypted_payload_ref: Option<String>,
    pub external_escrow_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct APFExtensionSchemaRecord {
    pub schema_id: String,
    pub owner_ref: String,
    pub target_record: String,
    pub version: String,
    pub fields: Vec<ExtensionField>,
    pub schema_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionField {
    pub name: String,
    pub field_type: String,
    pub encrypted: bool,
    pub external_source: Option<String>,
    pub decision_critical: bool,
    pub indexed: bool,
    pub digest_required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyIntegrationHookRecord {
    pub hook_id: String,
    pub hook_kind: String,
    pub input_schema_ref: String,
    pub output_schema_ref: String,
    pub timeout_ms: u64,
    pub retry_policy: String,
    pub failure_mode: FailureMode,
    pub redaction_policy_ref: String,
    pub conformance_fixture_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PepRegistrationRecord {
    pub pep_id: String,
    pub pep_kind: String,
    pub application: String,
    pub coverage_class: PepCoverageClass,
    pub can_block: bool,
    pub authority_boundary_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FederatedCapabilityGrantRecord {
    pub grant_id: String,
    pub issuer: String,
    pub audience: String,
    pub subject_agent: String,
    pub tenant: String,
    pub capabilities: Vec<String>,
    pub denials: Vec<String>,
    pub expires_at: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinanceProfileResourceRecord {
    pub resource_id: String,
    pub resource_kind: FinanceResourceKind,
    pub canonical_ref: String,
    pub profile_version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinanceActionClassRecord {
    pub action_class_id: String,
    pub resource_kind: FinanceResourceKind,
    pub side_effect_class: SideEffectClass,
    pub required_pep_coverage: PepCoverageClass,
    pub tradeassembly_public_core_allowed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinanceRiskGateRecord {
    pub gate_id: String,
    pub gate_kind: String,
    pub input_schema_ref: String,
    pub failure_mode: FailureMode,
    pub decision_critical: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinanceComplianceHookRecord {
    pub hook_id: String,
    pub hook_kind: String,
    pub policy_hook_id: String,
    pub retention_policy_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MandateDecisionTuple {
    pub agent: String,
    pub principal_user: String,
    pub client: ClientContext,
    pub application: String,
    pub action: String,
    pub resource: String,
    pub context_snapshot_id: String,
    pub time_window: TimeWindow,
    pub purpose: PurposeContext,
    pub evidence_refs: Vec<String>,
    pub policy_version: String,
    pub supervision_mode: SupervisionMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientContext {
    pub client_id: String,
    pub client_kind: ClientKind,
    pub taxonomy_version: String,
}

impl ClientContext {
    pub fn local_self(user_id: impl Into<String>) -> Self {
        Self {
            client_id: user_id.into(),
            client_kind: ClientKind::SelfClient,
            taxonomy_version: "client_context.v1".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    SelfClient,
    Household,
    AdvisorClient,
    Fund,
    Institution,
    CustomerDefined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurposeContext {
    pub purpose_id: String,
    pub taxonomy_version: String,
    pub explanation_evidence_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub starts_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Approved,
    ApprovedWithHumanConfirmation,
    Denied,
    StepUpRequired,
    Quarantined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MandateStatus {
    Draft,
    Active,
    Suspended,
    Revoked,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptPayloadPolicy {
    HashOnly,
    RedactedPayload,
    EncryptedPayloadRef,
    ExternalEscrowRef,
    NoCapture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    FailClosed,
    FailOpen,
    AdvisoryOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PepCoverageClass {
    C0NoVisibility,
    C1AuditOnly,
    C2AdvisoryOnly,
    C3GatewayCanBlock,
    C4CredentialOrAdapterCanBlock,
    C5DownstreamRequiresAuthority,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisionMode {
    None,
    HumanConfirmation,
    SupervisorApproval,
    BreakGlass,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinanceResourceKind {
    BrokerageAccount,
    Portfolio,
    Instrument,
    OptionContract,
    Strategy,
    Backtest,
    PaperOrder,
    LiveOrder,
    ExecutionReport,
    TradeConfirmation,
    RiskMandate,
    BrokerConnection,
    CredentialHandle,
    AdvisorSupervisionRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeScope {
    Research,
    Shadow,
    Paper,
    ConfirmBeforeSend,
    Live,
    Export,
    AdminConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantEffect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantPrecedence {
    DenyBeatsAllow,
    BreakGlassOverride,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    Read,
    DraftMutation,
    Research,
    Credential,
    Paper,
    Live,
    Order,
    Emergency,
    Export,
    AdminConfig,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadinessSnapshot {
    pub spec_valid: bool,
    pub capability_declared: bool,
    pub workspace_compatible: bool,
    pub research_ready: bool,
    pub paper_ready: bool,
    pub live_ready: bool,
}

impl ReadinessSnapshot {
    pub fn all_blocked() -> Self {
        Self {
            spec_valid: false,
            capability_declared: false,
            workspace_compatible: false,
            research_ready: false,
            paper_ready: false,
            live_ready: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityClass {
    Read,
    User,
    UserOnly,
    UserDraft,
    ResearchEntitlement,
    ExecutionAuthority,
    StrongUserAuthority,
    SystemRunner,
    SystemScheduler,
    SystemReconciler,
    PluginConfigAuthority,
    CredentialAuthority,
    SessionAuthority,
    PolicyFabric,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdempotencyRequirement {
    Required,
    OptionalCache,
    AtomicOneTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StalePolicy {
    RejectExpectedVersion,
    IgnoreLateResult,
    ReconcileBeforeApply,
    CompareAndSet,
    NoMutation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TransitionOutcome<S> {
    pub next_state: S,
    pub emitted_events: Vec<&'static str>,
    pub authority_class: AuthorityClass,
    pub idempotency: IdempotencyRequirement,
    pub stale_policy: StalePolicy,
    pub replay_inputs: Vec<&'static str>,
    pub replay_outputs: Vec<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionError {
    pub from_state: String,
    pub command: String,
    pub reason: &'static str,
}

pub trait LifecycleFsm {
    type State: Copy + std::fmt::Debug + PartialEq + Eq;
    type Command: Copy + std::fmt::Debug + PartialEq + Eq;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError>;
}

fn outcome<S>(
    next_state: S,
    event: &'static str,
    authority_class: AuthorityClass,
    idempotency: IdempotencyRequirement,
    stale_policy: StalePolicy,
    replay_inputs: Vec<&'static str>,
    replay_outputs: Vec<&'static str>,
) -> TransitionOutcome<S> {
    TransitionOutcome {
        next_state,
        emitted_events: vec![event],
        authority_class,
        idempotency,
        stale_policy,
        replay_inputs,
        replay_outputs,
    }
}

fn invalid<S: std::fmt::Debug, C: std::fmt::Debug>(state: S, command: C) -> TransitionError {
    TransitionError {
        from_state: format!("{state:?}"),
        command: format!("{command:?}"),
        reason: "transition_not_allowed",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyDraftState {
    Created,
    Editing,
    Valid,
    Invalid,
    Published,
    Archived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyDraftCommand {
    Patch,
    ValidateValid,
    ValidateInvalid,
    Publish,
    Archive,
    Restore,
    CloneFromVersion,
}

pub struct StrategyDraftFsm;

impl LifecycleFsm for StrategyDraftFsm {
    type State = StrategyDraftState;
    type Command = StrategyDraftCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use StrategyDraftCommand as C;
        use StrategyDraftState as S;
        match (state, command) {
            (S::Created | S::Editing | S::Valid | S::Invalid, C::Patch) => Ok(outcome(
                S::Editing,
                "strategy_patch.applied",
                AuthorityClass::UserDraft,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["base_draft_hash", "patch", "actor"],
                vec!["draft_hash", "validation_hints"],
            )),
            (S::Created | S::Editing | S::Invalid, C::ValidateValid) => Ok(outcome(
                S::Valid,
                "strategy_validation.completed",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["draft_hash", "schema_version", "capability_catalog"],
                vec!["validation_report", "readiness_refs"],
            )),
            (S::Created | S::Editing | S::Valid, C::ValidateInvalid) => Ok(outcome(
                S::Invalid,
                "strategy_validation.completed",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["draft_hash", "schema_version", "capability_catalog"],
                vec!["validation_report", "readiness_refs"],
            )),
            (S::Valid, C::Publish) => Ok(outcome(
                S::Published,
                "strategy_version.published",
                AuthorityClass::UserOnly,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["draft_hash", "validation_report", "provenance"],
                vec!["strategy_version_id", "strategy_version_hash"],
            )),
            (S::Created | S::Editing | S::Valid | S::Invalid, C::Archive) => Ok(outcome(
                S::Archived,
                "strategy_draft.archived",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["draft_ref", "reason"],
                vec!["archived_event_id"],
            )),
            (S::Archived, C::Restore) => Ok(outcome(
                S::Editing,
                "strategy_draft.restored",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["archived_draft_ref"],
                vec!["draft_hash"],
            )),
            (S::Published, C::CloneFromVersion) => Ok(outcome(
                S::Editing,
                "strategy_draft.cloned",
                AuthorityClass::UserDraft,
                IdempotencyRequirement::Required,
                StalePolicy::NoMutation,
                vec!["strategy_version_hash"],
                vec!["draft_id", "draft_hash"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyVersionState {
    Published,
    Deprecated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyVersionCommand {
    Deprecate,
    Restore,
    CreateBacktest,
    ConfigureExecution,
}

pub struct StrategyVersionFsm;

impl LifecycleFsm for StrategyVersionFsm {
    type State = StrategyVersionState;
    type Command = StrategyVersionCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use StrategyVersionCommand as C;
        use StrategyVersionState as S;
        match (state, command) {
            (S::Published, C::Deprecate) => Ok(outcome(
                S::Deprecated,
                "strategy_version.deprecated",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["version_ref", "metadata_hash", "reason"],
                vec!["deprecation_event_id"],
            )),
            (S::Deprecated, C::Restore) => Ok(outcome(
                S::Published,
                "strategy_version.restored",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["version_ref", "metadata_hash"],
                vec!["restore_event_id"],
            )),
            (S::Published, C::CreateBacktest) => Ok(outcome(
                S::Published,
                "backtest_run.created",
                AuthorityClass::ResearchEntitlement,
                IdempotencyRequirement::Required,
                StalePolicy::NoMutation,
                vec!["version_hash", "backtest_manifest"],
                vec!["backtest_run_id"],
            )),
            (S::Published, C::ConfigureExecution) => Ok(outcome(
                S::Published,
                "execution_config.created",
                AuthorityClass::ExecutionAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::NoMutation,
                vec!["version_hash", "target_profile", "capability_refs"],
                vec!["execution_config_id"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktestRunState {
    Draft,
    Ready,
    Blocked,
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BacktestRunCommand {
    ValidateReady,
    ValidateBlocked,
    Execute,
    Start,
    Complete,
    Fail,
    Cancel,
    Retry,
    Replay,
    Export,
}

pub struct BacktestRunFsm;

impl LifecycleFsm for BacktestRunFsm {
    type State = BacktestRunState;
    type Command = BacktestRunCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use BacktestRunCommand as C;
        use BacktestRunState as S;
        match (state, command) {
            (S::Draft, C::ValidateReady) => Ok(outcome(
                S::Ready,
                "backtest_run.validated",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["run_manifest", "plugin_fingerprints", "entitlement_refs"],
                vec!["readiness_report"],
            )),
            (S::Draft, C::ValidateBlocked) => Ok(outcome(
                S::Blocked,
                "backtest_run.validated",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["run_manifest", "plugin_fingerprints", "entitlement_refs"],
                vec!["readiness_report"],
            )),
            (S::Ready, C::Execute) => Ok(outcome(
                S::Queued,
                "backtest_run.queued",
                AuthorityClass::ResearchEntitlement,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["manifest_hash", "runner_profile"],
                vec!["queued_job_ref"],
            )),
            (S::Queued, C::Start) => Ok(outcome(
                S::Running,
                "backtest_run.started",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["queued_job_ref", "attempt_id"],
                vec!["start_event_id"],
            )),
            (S::Running, C::Complete) => Ok(outcome(
                S::Completed,
                "backtest_run.completed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["orders", "fills", "positions", "ledger", "report"],
                vec!["report_artifact_refs", "replay_refs"],
            )),
            (S::Running, C::Fail) => Ok(outcome(
                S::Failed,
                "backtest_run.failed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["failure_diagnostics", "partial_refs"],
                vec!["failure_event_id", "diagnostics_ref"],
            )),
            (S::Draft | S::Ready | S::Queued | S::Running, C::Cancel) => Ok(outcome(
                S::Canceled,
                "backtest_run.canceled",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["run_ref", "reason"],
                vec!["cancel_event_id"],
            )),
            (S::Failed, C::Retry) => Ok(outcome(
                S::Queued,
                "backtest_run.retry_requested",
                AuthorityClass::ResearchEntitlement,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["prior_manifest", "failure_refs"],
                vec!["attempt_ref"],
            )),
            (S::Completed, C::Replay) => Ok(outcome(
                S::Completed,
                "backtest_replay.completed",
                AuthorityClass::ResearchEntitlement,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::RejectExpectedVersion,
                vec![
                    "manifest",
                    "dataset_snapshot",
                    "plugin_fingerprints",
                    "event_refs",
                ],
                vec!["match_report"],
            )),
            (S::Completed, C::Export) => Ok(outcome(
                S::Completed,
                "report_export.created",
                AuthorityClass::ResearchEntitlement,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["report_refs", "redaction_policy", "artifact_type"],
                vec!["artifact_ref"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobustnessRunState {
    Draft,
    Ready,
    Blocked,
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RobustnessRunCommand {
    ValidateReady,
    ValidateBlocked,
    Execute,
    Start,
    Complete,
    Fail,
    Cancel,
    Retry,
}

pub struct RobustnessRunFsm;

impl LifecycleFsm for RobustnessRunFsm {
    type State = RobustnessRunState;
    type Command = RobustnessRunCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use RobustnessRunCommand as C;
        use RobustnessRunState as S;
        match (state, command) {
            (S::Draft, C::ValidateReady) => Ok(outcome(
                S::Ready,
                "robustness_run.validated",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec![
                    "scenario_manifest",
                    "source_run_refs",
                    "plugin_fingerprints",
                ],
                vec!["readiness_report"],
            )),
            (S::Draft, C::ValidateBlocked) => Ok(outcome(
                S::Blocked,
                "robustness_run.validated",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec![
                    "scenario_manifest",
                    "source_run_refs",
                    "plugin_fingerprints",
                ],
                vec!["readiness_report"],
            )),
            (S::Ready, C::Execute) => Ok(outcome(
                S::Queued,
                "robustness_run.queued",
                AuthorityClass::ResearchEntitlement,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["manifest_hash", "runner_profile"],
                vec!["queued_job_ref"],
            )),
            (S::Queued, C::Start) => Ok(outcome(
                S::Running,
                "robustness_run.started",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["queued_job_ref", "attempt_id"],
                vec!["start_event_id"],
            )),
            (S::Running, C::Complete) => Ok(outcome(
                S::Completed,
                "robustness_run.completed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["samples", "distributions", "warnings", "report_refs"],
                vec!["distribution_refs", "report_artifact_refs"],
            )),
            (S::Running, C::Fail) => Ok(outcome(
                S::Failed,
                "robustness_run.failed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["failure_diagnostics", "partial_outputs"],
                vec!["failure_refs"],
            )),
            (S::Failed, C::Retry) => Ok(outcome(
                S::Queued,
                "robustness_run.retry_requested",
                AuthorityClass::ResearchEntitlement,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["prior_manifest", "failure_refs"],
                vec!["attempt_ref"],
            )),
            (S::Draft | S::Ready | S::Queued | S::Running, C::Cancel) => Ok(outcome(
                S::Canceled,
                "robustness_run.canceled",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["run_ref", "reason"],
                vec!["cancel_event_id"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRunState {
    Draft,
    Ready,
    Blocked,
    ActivationRequested,
    Active,
    Stopping,
    Deactivated,
    EmergencyStopped,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionRunCommand {
    ValidateReady,
    ValidateBlocked,
    RequestActivation,
    RecordActivation,
    PauseEntries,
    StopAfterFlat,
    EmergencyStop,
    FlatComplete,
    Fail,
}

#[deprecated(
    note = "compatibility-only ExecutionRun transition API; use ActivationFsm or ExecutionAttemptFsm"
)]
pub struct ExecutionRunFsm;

#[allow(deprecated)]
impl LifecycleFsm for ExecutionRunFsm {
    type State = ExecutionRunState;
    type Command = ExecutionRunCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use ExecutionRunCommand as C;
        use ExecutionRunState as S;
        match (state, command) {
            (S::Draft, C::ValidateReady) => Ok(outcome(
                S::Ready,
                "execution_config.validated",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec![
                    "strategy_version",
                    "capability_resolution",
                    "account_binding",
                    "risk_config",
                ],
                vec!["preflight_report"],
            )),
            (S::Draft, C::ValidateBlocked) => Ok(outcome(
                S::Blocked,
                "execution_config.validated",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec![
                    "strategy_version",
                    "capability_resolution",
                    "account_binding",
                    "risk_config",
                ],
                vec!["preflight_report"],
            )),
            (S::Ready, C::RequestActivation) => Ok(outcome(
                S::ActivationRequested,
                "execution_run.activation_requested",
                AuthorityClass::ExecutionAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["config_hash", "mode", "preflight_refs", "authority_context"],
                vec!["activation_request_ref"],
            )),
            (S::ActivationRequested, C::RecordActivation) => Ok(outcome(
                S::Active,
                "execution_run.activated",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["request_ref", "lease_refs", "preflight_refs"],
                vec!["activation_id", "start_evidence"],
            )),
            (S::Active, C::PauseEntries) => Ok(outcome(
                S::Active,
                "execution_control.pause_entries",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["run_ref", "control_version"],
                vec!["updated_control_state"],
            )),
            (S::Active, C::StopAfterFlat) => Ok(outcome(
                S::Stopping,
                "execution_control.stop_after_flat",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["run_ref", "orders_ref", "positions_ref"],
                vec!["stopping_event_id"],
            )),
            (S::Active, C::EmergencyStop) => Ok(outcome(
                S::EmergencyStopped,
                "execution_control.emergency_stop",
                AuthorityClass::StrongUserAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["run_ref", "account_binding", "open_orders", "positions"],
                vec!["emergency_event_id", "broker_control_refs"],
            )),
            (S::Stopping, C::FlatComplete) => Ok(outcome(
                S::Deactivated,
                "execution_run.deactivated",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["reconciliation_state", "order_refs", "position_refs"],
                vec!["deactivation_event_id"],
            )),
            (S::ActivationRequested | S::Active | S::Stopping, C::Fail) => Ok(outcome(
                S::Failed,
                "execution_run.failed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["failure_diagnostics"],
                vec!["failure_event_id"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationState {
    Pending,
    Active,
    Draining,
    Degraded,
    Failed,
    Stopped,
    EmergencyStopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationCommand {
    Start,
    LeaseAcquire,
    EvaluateTick,
    Checkpoint,
    ReconcileOk,
    ReconcileDegraded,
    PauseEntries,
    StopAfterFlat,
    FlatComplete,
    EmergencyStop,
    Recover,
    Fail,
}

pub struct ActivationFsm;

impl LifecycleFsm for ActivationFsm {
    type State = ActivationState;
    type Command = ActivationCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use ActivationCommand as C;
        use ActivationState as S;
        match (state, command) {
            (S::Pending, C::Start) => Ok(outcome(
                S::Active,
                "activation.started",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec![
                    "execution_run_request",
                    "preflight_refs",
                    "initial_checkpoint",
                    "lease",
                ],
                vec!["activation_state", "start_event_id"],
            )),
            (S::Pending | S::Active | S::Degraded, C::Fail) => Ok(outcome(
                S::Failed,
                "activation.failed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["failure_diagnostics"],
                vec!["failure_refs"],
            )),
            (S::Active, C::LeaseAcquire) => Ok(outcome(
                S::Active,
                "scheduler_lease.acquired",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::CompareAndSet,
                vec!["activation_id", "lease_key", "worker_id", "epoch"],
                vec!["lease_ref"],
            )),
            (S::Active, C::EvaluateTick) => Ok(outcome(
                S::Active,
                "execution_tick.evaluated",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec![
                    "checkpoint",
                    "market_inputs",
                    "strategy_version_hash",
                    "account_binding",
                    "risk_state",
                ],
                vec!["decision_record", "order_intent_refs", "updated_refs"],
            )),
            (S::Active, C::Checkpoint) => Ok(outcome(
                S::Active,
                "runtime_checkpoint.updated",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["feed_cursor", "plugin_cursor", "broker_cursor"],
                vec!["checkpoint_ref"],
            )),
            (S::Active, C::ReconcileOk) => Ok(outcome(
                S::Active,
                "reconciliation.completed",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec![
                    "local_state",
                    "broker_snapshot",
                    "plugin_snapshot",
                    "inbox_records",
                ],
                vec!["reconciliation_state"],
            )),
            (S::Active, C::ReconcileDegraded) => Ok(outcome(
                S::Degraded,
                "reconciliation.completed",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec![
                    "local_state",
                    "broker_snapshot",
                    "plugin_snapshot",
                    "inbox_records",
                ],
                vec!["reconciliation_state", "warnings"],
            )),
            (S::Active, C::PauseEntries) => Ok(outcome(
                S::Active,
                "activation_control.updated",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["activation_ref", "control_version"],
                vec!["paused_entry_control_state"],
            )),
            (S::Active, C::StopAfterFlat) => Ok(outcome(
                S::Draining,
                "activation.draining",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["activation_ref", "open_orders", "positions"],
                vec!["draining_state"],
            )),
            (S::Draining, C::FlatComplete) => Ok(outcome(
                S::Stopped,
                "activation.stopped",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["flat_proof", "reconciliation_state"],
                vec!["stopped_event_id"],
            )),
            (S::Active | S::Draining | S::Degraded, C::EmergencyStop) => Ok(outcome(
                S::EmergencyStopped,
                "activation.emergency_stopped",
                AuthorityClass::StrongUserAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec![
                    "activation_ref",
                    "account_binding",
                    "open_orders",
                    "positions",
                ],
                vec!["emergency_evidence", "broker_control_refs"],
            )),
            (S::Degraded, C::Recover) => Ok(outcome(
                S::Active,
                "activation.recovered",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["reconciliation_state", "plugin_health_refs"],
                vec!["recovered_state"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

pub struct ExecutionAttemptFsm;

impl LifecycleFsm for ExecutionAttemptFsm {
    type State = ExecutionAttemptState;
    type Command = ExecutionAttemptCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use ExecutionAttemptCommand as C;
        use ExecutionAttemptState as S;
        match (state, command) {
            (S::Starting, C::Start) => Ok(outcome(
                S::Running,
                "execution_attempt.started",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec![
                    "compiled_plan_hash",
                    "config_revision_hash",
                    "capability_bindings",
                    "checkpoint",
                    "fenced_lease",
                ],
                vec!["start_evidence"],
            )),
            (S::Running, C::EvaluateTick) => Ok(outcome(
                S::Running,
                "evaluation_tick.completed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec![
                    "activation_id",
                    "input_cursor",
                    "tick_id",
                    "fenced_lease",
                    "authority_control",
                ],
                vec![
                    "evaluation_tick",
                    "decision_record",
                    "order_intent_refs",
                    "outbox_refs",
                ],
            )),
            (S::Running, C::Complete) => Ok(outcome(
                S::Completed,
                "execution_attempt.ended",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["attempt_id", "lease_epoch", "final_checkpoint"],
                vec!["end_evidence"],
            )),
            (S::Starting | S::Running, C::Fail) => Ok(outcome(
                S::Failed,
                "execution_attempt.ended",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["attempt_id", "lease_epoch", "failure_diagnostics"],
                vec!["end_evidence", "diagnostic_refs"],
            )),
            (S::Starting | S::Running, C::Replace) => Ok(outcome(
                S::Replaced,
                "execution_attempt.ended",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::CompareAndSet,
                vec!["attempt_id", "lease_epoch", "replacement_attempt_id"],
                vec!["end_evidence"],
            )),
            (S::Starting | S::Running, C::LoseLease) => Ok(outcome(
                S::LeaseLost,
                "execution_attempt.ended",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::CompareAndSet,
                vec!["attempt_id", "lease_epoch", "fenced_lease"],
                vec!["end_evidence"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderIntentState {
    Created,
    ApprovalRequested,
    Rejected,
    Expired,
    Canceled,
    Approved,
    Submitted,
    PartialFilled,
    Filled,
    CancelRequested,
    BrokerRejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderIntentCommand {
    RequestApproval,
    Reject,
    Expire,
    Cancel,
    Approve,
    Submit,
    FillPartial,
    FillComplete,
    RejectByBroker,
}

pub struct OrderIntentFsm;

impl LifecycleFsm for OrderIntentFsm {
    type State = OrderIntentState;
    type Command = OrderIntentCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use OrderIntentCommand as C;
        use OrderIntentState as S;
        match (state, command) {
            (S::Created, C::RequestApproval) => Ok(outcome(
                S::ApprovalRequested,
                "approval.requested",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["order_intent", "quote", "risk_reservation"],
                vec!["approval_id"],
            )),
            (S::Created | S::ApprovalRequested, C::Reject) => Ok(outcome(
                S::Rejected,
                "order_intent.rejected",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["intent_ref", "reason"],
                vec!["rejection_event_id"],
            )),
            (S::Created | S::ApprovalRequested, C::Expire) => Ok(outcome(
                S::Expired,
                "order_intent.expired",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["expiry_timestamp", "reservation_ref"],
                vec!["expired_event_id"],
            )),
            (S::Created | S::ApprovalRequested, C::Cancel) => Ok(outcome(
                S::Canceled,
                "order_intent.canceled",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["intent_ref", "control_ref"],
                vec!["cancel_event_id"],
            )),
            (S::ApprovalRequested, C::Approve) => Ok(outcome(
                S::Approved,
                "order_intent.approved",
                AuthorityClass::UserOnly,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["approval_snapshot", "account_decision", "quote_freshness"],
                vec!["approved_intent"],
            )),
            (S::Approved, C::Submit) => Ok(outcome(
                S::Submitted,
                "order.submitted",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["approved_intent", "credential_handle", "account_binding"],
                vec!["order_record"],
            )),
            (S::Submitted, C::FillPartial) => Ok(outcome(
                S::PartialFilled,
                "fill.received",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["broker_fill_event", "order_ref"],
                vec!["fill_ref", "position_ref"],
            )),
            (S::Submitted | S::PartialFilled, C::FillComplete) => Ok(outcome(
                S::Filled,
                "fill.received",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["broker_fill_event", "order_ref"],
                vec!["fill_ref", "position_ref"],
            )),
            (S::Submitted | S::PartialFilled, C::Cancel) => Ok(outcome(
                S::CancelRequested,
                "order.cancel_requested",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::ReconcileBeforeApply,
                vec!["order_ref", "control_ref"],
                vec!["cancel_evidence"],
            )),
            (S::Submitted, C::RejectByBroker) => Ok(outcome(
                S::BrokerRejected,
                "order.broker_rejected",
                AuthorityClass::SystemReconciler,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["rejection_payload", "order_ref"],
                vec!["rejection_evidence"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Requested,
    Accepted,
    Rejected,
    Expired,
    Consumed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalCommand {
    Accept,
    Reject,
    Expire,
    Consume,
}

pub struct ApprovalFsm;

impl LifecycleFsm for ApprovalFsm {
    type State = ApprovalState;
    type Command = ApprovalCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use ApprovalCommand as C;
        use ApprovalState as S;
        match (state, command) {
            (S::Requested, C::Accept) => Ok(outcome(
                S::Accepted,
                "approval.accepted",
                AuthorityClass::UserOnly,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["approval_snapshot", "authority_context"],
                vec!["accepted_event_id"],
            )),
            (S::Requested, C::Reject) => Ok(outcome(
                S::Rejected,
                "approval.rejected",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["approval_ref", "reason"],
                vec!["rejected_event_id"],
            )),
            (S::Requested, C::Expire) => Ok(outcome(
                S::Expired,
                "approval.expired",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["clock", "approval_ref"],
                vec!["expired_event_id"],
            )),
            (S::Accepted, C::Consume) => Ok(outcome(
                S::Consumed,
                "approval.consumed",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::AtomicOneTime,
                StalePolicy::RejectExpectedVersion,
                vec!["accepted_approval", "target_command"],
                vec!["consumed_ref", "target_result_ref"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityResolutionState {
    Unresolved,
    Candidates,
    Blocked,
    Selected,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityResolutionCommand {
    ResolveCandidates,
    ResolveBlocked,
    Select,
    RefreshSelected,
    RefreshStale,
    RefreshBlocked,
    Invalidate,
}

pub struct CapabilityResolutionFsm;

impl LifecycleFsm for CapabilityResolutionFsm {
    type State = CapabilityResolutionState;
    type Command = CapabilityResolutionCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use CapabilityResolutionCommand as C;
        use CapabilityResolutionState as S;
        match (state, command) {
            (S::Unresolved | S::Stale, C::ResolveCandidates) => Ok(outcome(
                S::Candidates,
                "capability_resolution.completed",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::RejectExpectedVersion,
                vec!["capability_requirement", "plugin_manifests", "entitlements"],
                vec!["candidate_list", "blockers"],
            )),
            (S::Unresolved | S::Stale, C::ResolveBlocked) => Ok(outcome(
                S::Blocked,
                "capability_resolution.completed",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::RejectExpectedVersion,
                vec!["capability_requirement", "plugin_manifests", "entitlements"],
                vec!["candidate_list", "blockers"],
            )),
            (S::Candidates, C::Select) => Ok(outcome(
                S::Selected,
                "capability_resolution.selected",
                AuthorityClass::PluginConfigAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["candidate", "plugin_instance", "account_binding"],
                vec!["selected_resolution"],
            )),
            (S::Selected, C::RefreshSelected) => Ok(outcome(
                S::Selected,
                "capability_resolution.refreshed",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["selected_resolution", "current_plugin_health"],
                vec!["updated_status"],
            )),
            (S::Selected, C::RefreshStale | C::Invalidate) => Ok(outcome(
                S::Stale,
                "capability_resolution.invalidated",
                AuthorityClass::SystemRunner,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["manifest_or_instance_change_ref"],
                vec!["stale_event_id"],
            )),
            (S::Selected, C::RefreshBlocked) => Ok(outcome(
                S::Blocked,
                "capability_resolution.refreshed",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["selected_resolution", "current_plugin_health"],
                vec!["blockers"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginConnectionState {
    Unconfigured,
    Configured,
    Connected,
    Degraded,
    Expired,
    Revoked,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginConnectionCommand {
    Configure,
    StoreCredential,
    Connect,
    Test,
    HealthConnected,
    HealthDegraded,
    HealthExpired,
    Revoke,
    Disable,
}

pub struct PluginConnectionFsm;

impl LifecycleFsm for PluginConnectionFsm {
    type State = PluginConnectionState;
    type Command = PluginConnectionCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use PluginConnectionCommand as C;
        use PluginConnectionState as S;
        match (state, command) {
            (S::Unconfigured, C::Configure) => Ok(outcome(
                S::Configured,
                "plugin_instance.configured",
                AuthorityClass::PluginConfigAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["config_schema_values_redacted"],
                vec!["instance_config_hash"],
            )),
            (S::Configured, C::StoreCredential) => Ok(outcome(
                S::Configured,
                "credential_handle.stored",
                AuthorityClass::CredentialAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::NoMutation,
                vec!["secret_write_only_payload", "plugin_ref"],
                vec!["credential_handle_status"],
            )),
            (S::Configured, C::Connect) => Ok(outcome(
                S::Connected,
                "plugin_instance.connected",
                AuthorityClass::PluginConfigAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["credential_handle", "account_selection"],
                vec!["account_binding_status"],
            )),
            (S::Configured | S::Connected | S::Degraded, C::Test) => Ok(outcome(
                state,
                "plugin_connection.tested",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["plugin_instance_ref"],
                vec!["redacted_test_result"],
            )),
            (S::Connected | S::Degraded | S::Expired, C::HealthConnected) => Ok(outcome(
                S::Connected,
                "plugin_health.checked",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["plugin_instance", "account_binding"],
                vec!["health_status"],
            )),
            (S::Connected, C::HealthDegraded) => Ok(outcome(
                S::Degraded,
                "plugin_health.checked",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["plugin_instance", "account_binding"],
                vec!["health_status"],
            )),
            (S::Connected | S::Degraded, C::HealthExpired) => Ok(outcome(
                S::Expired,
                "plugin_health.checked",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["plugin_instance", "account_binding"],
                vec!["health_status"],
            )),
            (S::Configured | S::Connected | S::Degraded | S::Expired, C::Revoke) => Ok(outcome(
                S::Revoked,
                "credential_handle.revoked",
                AuthorityClass::CredentialAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::NoMutation,
                vec!["handle_ref", "plugin_ref"],
                vec!["revoked_event_id"],
            )),
            (S::Configured | S::Connected | S::Degraded | S::Expired, C::Disable) => Ok(outcome(
                S::Disabled,
                "plugin_instance.disabled",
                AuthorityClass::PluginConfigAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::NoMutation,
                vec!["instance_ref", "reason"],
                vec!["disabled_event_id"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementGrantState {
    Proposed,
    Active,
    Denied,
    Revoked,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntitlementGrantCommand {
    Grant,
    Deny,
    Check,
    Revoke,
    Expire,
    Override,
}

pub struct EntitlementGrantFsm;

impl LifecycleFsm for EntitlementGrantFsm {
    type State = EntitlementGrantState;
    type Command = EntitlementGrantCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use EntitlementGrantCommand as C;
        use EntitlementGrantState as S;
        match (state, command) {
            (S::Proposed, C::Grant) => Ok(outcome(
                S::Active,
                "entitlement.granted",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["principal", "resource", "capability", "mode", "limits"],
                vec!["grant_decision_id"],
            )),
            (S::Proposed, C::Deny) => Ok(outcome(
                S::Denied,
                "entitlement.denied",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["scope", "effect", "reason"],
                vec!["denial_decision_id"],
            )),
            (S::Active, C::Check) => Ok(outcome(
                S::Active,
                "entitlement.checked",
                AuthorityClass::Read,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::RejectExpectedVersion,
                vec!["command", "resource", "capability", "mode"],
                vec!["allow_deny_limit_decision"],
            )),
            (S::Active, C::Revoke) => Ok(outcome(
                S::Revoked,
                "entitlement.revoked",
                AuthorityClass::User,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["grant_id", "reason"],
                vec!["revoked_event_id"],
            )),
            (S::Active, C::Expire) => Ok(outcome(
                S::Expired,
                "entitlement.expired",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["time", "grant_id"],
                vec!["expired_event_id"],
            )),
            (S::Denied, C::Override) => Ok(outcome(
                S::Active,
                "entitlement.override_granted",
                AuthorityClass::StrongUserAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["denial_id", "override_scope", "rationale"],
                vec!["audited_allow"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSurfaceState {
    Registered,
    Focused,
    Selected,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSurfaceCommand {
    Focus,
    Select,
    Navigate,
    Proposal,
    Close,
}

pub struct SessionSurfaceFsm;

impl LifecycleFsm for SessionSurfaceFsm {
    type State = SessionSurfaceState;
    type Command = SessionSurfaceCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use SessionSurfaceCommand as C;
        use SessionSurfaceState as S;
        match (state, command) {
            (S::Registered | S::Focused, C::Focus) => Ok(outcome(
                S::Focused,
                "studio_node.focus_requested",
                AuthorityClass::SessionAuthority,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::RejectExpectedVersion,
                vec!["node_ref", "surface_ref"],
                vec!["focus_request_event"],
            )),
            (S::Registered | S::Focused | S::Selected, C::Select) => Ok(outcome(
                S::Selected,
                "studio_node.selected",
                AuthorityClass::SessionAuthority,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::IgnoreLateResult,
                vec!["semantic_node_ref", "redaction_policy"],
                vec!["shared_selection_snapshot"],
            )),
            (S::Registered | S::Focused | S::Selected, C::Navigate) => Ok(outcome(
                S::Focused,
                "studio_navigation.requested",
                AuthorityClass::SessionAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["route", "surface", "object_refs"],
                vec!["navigation_event"],
            )),
            (S::Selected, C::Proposal) => Ok(outcome(
                S::Selected,
                "strategy_patch.proposed",
                AuthorityClass::SessionAuthority,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["selected_node", "patch", "base_draft_hash"],
                vec!["proposal_ref"],
            )),
            (S::Registered | S::Focused | S::Selected, C::Close) => Ok(outcome(
                S::Closed,
                "sightline_surface.closed",
                AuthorityClass::SessionAuthority,
                IdempotencyRequirement::OptionalCache,
                StalePolicy::NoMutation,
                vec!["surface_ref"],
                vec!["close_event"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerLeaseState {
    Available,
    Held,
    Released,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerLeaseCommand {
    Acquire,
    Renew,
    Release,
    Expire,
}

pub struct SchedulerLeaseFsm;

impl LifecycleFsm for SchedulerLeaseFsm {
    type State = SchedulerLeaseState;
    type Command = SchedulerLeaseCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use SchedulerLeaseCommand as C;
        use SchedulerLeaseState as S;
        match (state, command) {
            (S::Available | S::Expired | S::Released, C::Acquire) => Ok(outcome(
                S::Held,
                "scheduler_lease.acquired",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::CompareAndSet,
                vec!["activation_id", "lease_key", "worker_id", "ttl"],
                vec!["lease_ref", "epoch"],
            )),
            (S::Held, C::Renew) => Ok(outcome(
                S::Held,
                "scheduler_lease.renewed",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::CompareAndSet,
                vec!["lease_ref", "holder_id", "epoch"],
                vec!["renewed_ttl"],
            )),
            (S::Held, C::Release) => Ok(outcome(
                S::Released,
                "scheduler_lease.released",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::CompareAndSet,
                vec!["lease_ref", "holder_id", "reason"],
                vec!["release_event"],
            )),
            (S::Held, C::Expire) => Ok(outcome(
                S::Expired,
                "scheduler_lease.expired",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["lease_ref", "clock", "epoch"],
                vec!["expired_event"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegatedAuthorityTokenState {
    Requested,
    Active,
    Denied,
    Consumed,
    Revoked,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelegatedAuthorityTokenCommand {
    Issue,
    Deny,
    Consume,
    Revoke,
    Expire,
}

pub struct DelegatedAuthorityTokenFsm;

impl LifecycleFsm for DelegatedAuthorityTokenFsm {
    type State = DelegatedAuthorityTokenState;
    type Command = DelegatedAuthorityTokenCommand;

    fn transition(
        state: Self::State,
        command: Self::Command,
    ) -> Result<TransitionOutcome<Self::State>, TransitionError> {
        use DelegatedAuthorityTokenCommand as C;
        use DelegatedAuthorityTokenState as S;
        match (state, command) {
            (S::Requested, C::Issue) => Ok(outcome(
                S::Active,
                "delegated_authority.issued",
                AuthorityClass::PolicyFabric,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec![
                    "request_scope",
                    "user_principal",
                    "agent_principal",
                    "command_bounds",
                ],
                vec!["token_id", "decision_event_ref"],
            )),
            (S::Requested, C::Deny) => Ok(outcome(
                S::Denied,
                "delegated_authority.denied",
                AuthorityClass::PolicyFabric,
                IdempotencyRequirement::Required,
                StalePolicy::RejectExpectedVersion,
                vec!["request_scope", "reason"],
                vec!["denial_event_ref"],
            )),
            (S::Active, C::Consume) => Ok(outcome(
                S::Consumed,
                "delegated_authority.consumed",
                AuthorityClass::PolicyFabric,
                IdempotencyRequirement::AtomicOneTime,
                StalePolicy::RejectExpectedVersion,
                vec!["token_id", "target_command_envelope"],
                vec!["consumption_event", "target_command_authority_decision"],
            )),
            (S::Active, C::Revoke) => Ok(outcome(
                S::Revoked,
                "delegated_authority.revoked",
                AuthorityClass::PolicyFabric,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["token_id", "reason"],
                vec!["revoked_event"],
            )),
            (S::Active, C::Expire) => Ok(outcome(
                S::Expired,
                "delegated_authority.expired",
                AuthorityClass::SystemScheduler,
                IdempotencyRequirement::Required,
                StalePolicy::IgnoreLateResult,
                vec!["token_id", "clock"],
                vec!["expired_event"],
            )),
            _ => Err(invalid(state, command)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn canonical_contracts_cover_phase_1_records_and_keep_apf_generic() {
        let contracts = canonical_record_contracts();
        let names = contracts
            .iter()
            .map(|contract| contract.name)
            .collect::<BTreeSet<_>>();
        for required in [
            "Strategy",
            "StrategyDraft",
            "StrategyVersion",
            "StrategySpec",
            "StrategyExecutionConfig",
            "StrategyExecutionConfigRevision",
            "ExecutionRun",
            "Activation",
            "ExecutionAttempt",
            "ExecutionSession",
            "BacktestRun",
            "RobustnessRun",
            "Comparison",
            "DataSnapshot",
            "PluginManifest",
            "PluginInstance",
            "CapabilityResolution",
            "EntitlementGrant",
            "Approval",
            "OrderIntent",
            "DecisionRecord",
            "EventLedger",
            "ReportArtifact",
            "User",
            "Identity",
            "Organization",
            "Membership",
            "Actor",
            "RoleBinding",
            "ResourceRelationship",
            "OrgEntitlementSnapshot",
            "PolicyDecision",
            "APFCapabilityGrant",
            "LegalDocumentVersion",
            "LegalAcknowledgement",
            "EvidenceBundle",
            "HostedEvidenceReceipt",
            "EvidenceRetentionPolicy",
            "APFPolicyBundle",
            "APFPolicyProjection",
            "AgentRegistration",
            "ToolManifest",
            "SessionAdmission",
            "ActionAuthorizationDecision",
            "CredentialGrant",
            "DetectorVerdict",
            "APFEvidenceEvent",
            "FinanceActionDescriptor",
            "APFMandate",
            "APFReceipt",
            "APFContextSnapshot",
            "APFExtensionSchema",
            "PolicyIntegrationHook",
            "PEPRegistration",
            "FederatedCapabilityGrant",
            "ClientContext",
            "FinanceProfileResource",
            "FinanceActionClass",
            "FinanceRiskGate",
            "FinanceComplianceHook",
        ] {
            assert!(names.contains(required), "missing {required}");
        }

        let apf_core_names = contracts
            .iter()
            .filter(|contract| contract.layer == DomainLayer::ApfCore)
            .map(|contract| contract.name)
            .collect::<BTreeSet<_>>();
        assert!(apf_core_names.contains("APFMandate"));
        assert!(apf_core_names.contains("APFReceipt"));
        assert!(!apf_core_names.contains("FinanceProfileResource"));

        let execution_run = contracts
            .iter()
            .find(|contract| contract.name == "ExecutionRun")
            .expect("ExecutionRun compatibility contract");
        assert_eq!(
            execution_run.status,
            RecordContractStatus::CompatibilityOnly
        );
    }

    #[test]
    fn readiness_flags_are_not_collapsed() {
        let readiness = ReadinessSnapshot {
            spec_valid: true,
            capability_declared: true,
            workspace_compatible: false,
            research_ready: true,
            paper_ready: false,
            live_ready: false,
        };

        assert!(readiness.spec_valid);
        assert!(readiness.capability_declared);
        assert!(!readiness.workspace_compatible);
        assert!(readiness.research_ready);
        assert!(!readiness.paper_ready);
        assert!(!readiness.live_ready);
    }

    #[test]
    fn strategy_draft_fsm_requires_valid_before_publish() {
        let invalid_publish = StrategyDraftFsm::transition(
            StrategyDraftState::Editing,
            StrategyDraftCommand::Publish,
        );
        let valid_publish =
            StrategyDraftFsm::transition(StrategyDraftState::Valid, StrategyDraftCommand::Publish)
                .expect("valid publish transition");

        assert!(invalid_publish.is_err());
        assert_eq!(valid_publish.next_state, StrategyDraftState::Published);
        assert_eq!(valid_publish.authority_class, AuthorityClass::UserOnly);
        assert_eq!(valid_publish.idempotency, IdempotencyRequirement::Required);
        assert_eq!(
            valid_publish.emitted_events,
            vec!["strategy_version.published"]
        );
        assert!(!valid_publish.replay_inputs.is_empty());
        assert!(!valid_publish.replay_outputs.is_empty());
    }

    #[test]
    fn canonical_strategy_lifecycle_records_preserve_identity_and_cardinality() {
        let draft = StrategyDraftRecord {
            draft_id: "draft_1".to_string(),
            strategy_id: "strategy_1".to_string(),
            draft_hash: "sha256:draft".to_string(),
            state: StrategyDraftState::Valid,
            spec_fragment: serde_json::json!({"spec_version": "3.0"}),
            base_version_id: None,
            authority_context_ref: "authority:user_1".to_string(),
        };
        let version = StrategyVersionRecord {
            version_id: "version_1".to_string(),
            strategy_id: draft.strategy_id.clone(),
            source_schema_id: "https://tradeassembly.local/schemas/strategy-spec-v3.schema.json"
                .to_string(),
            original_spec_json: "{ \"spec_version\": \"3.0\" }".to_string(),
            original_spec_hash: "sha256:original".to_string(),
            canonical_spec_json: "{\"spec_version\":\"3.0\"}".to_string(),
            canonical_spec_hash: "sha256:canonical".to_string(),
            state: StrategyVersionState::Published,
            published_from_draft_id: draft.draft_id.clone(),
            provenance_event_id: "event:published".to_string(),
        };
        let config = StrategyExecutionConfigRecord {
            config_id: "config_1".to_string(),
            strategy_id: version.strategy_id.clone(),
            version_id: version.version_id.clone(),
            current_revision_id: Some("config_revision_2".to_string()),
            created_by_actor_id: "user_1".to_string(),
        };
        let revisions = [1_u64, 2_u64].map(|revision| StrategyExecutionConfigRevisionRecord {
            config_revision_id: format!("config_revision_{revision}"),
            config_id: config.config_id.clone(),
            strategy_id: config.strategy_id.clone(),
            strategy_version_id: config.version_id.clone(),
            revision,
            mode_scope: ModeScope::Paper,
            universe_bindings: vec!["universe:sp500".to_string()],
            symbol_bindings: BTreeMap::from([("underlying".to_string(), "symbol:SPY".to_string())]),
            parameter_overrides: BTreeMap::new(),
            risk_budget_ref: "risk_budget:paper".to_string(),
            risk_scale_ref: "risk_scale:1".to_string(),
            capability_resolution_ids: vec!["capability_resolution_1".to_string()],
            plugin_instance_ids: vec!["plugin_instance_1".to_string()],
            account_binding_id: Some("account_binding_1".to_string()),
            opaque_credential_handle_refs: vec!["credential_handle:paper".to_string()],
            clock_overrides: BTreeMap::new(),
            config_hash: format!("sha256:config_{revision}"),
            published_event_id: format!("event:config_revision_{revision}"),
        });
        let activations = [
            ("activation_1", "strategy_1:account_binding_1:paper"),
            ("activation_2", "strategy_1:account_binding_2:paper"),
        ]
        .map(|(activation_id, exclusivity_scope)| ActivationRecord {
            activation_id: activation_id.to_string(),
            strategy_id: version.strategy_id.clone(),
            strategy_version_id: version.version_id.clone(),
            config_revision_id: revisions[1].config_revision_id.clone(),
            mandate_id: "mandate_1".to_string(),
            authority_context_ref: "authority:user_1".to_string(),
            exclusivity_scope: exclusivity_scope.to_string(),
            dashboard_report_scope_ref: "report_scope:activation".to_string(),
            state: ActivationState::Pending,
            control_version: 0,
            active_lease_ids: Vec::new(),
            latest_checkpoint_id: None,
            started_event_id: None,
            stopped_event_id: None,
        });
        let attempts =
            [("attempt_1", 1_u64), ("attempt_2", 2_u64)].map(|(attempt_id, worker_epoch)| {
                ExecutionAttemptRecord {
                    attempt_id: attempt_id.to_string(),
                    activation_id: activations[0].activation_id.clone(),
                    worker_id: format!("worker_{worker_epoch}"),
                    worker_epoch,
                    fenced_lease_id: format!("lease_{worker_epoch}"),
                    state: ExecutionAttemptState::Starting,
                    start_cause: if worker_epoch == 1 {
                        ExecutionAttemptStartCause::InitialActivation
                    } else {
                        ExecutionAttemptStartCause::Restart
                    },
                    started_event_id: None,
                    end_cause: None,
                    ended_event_id: None,
                    health: ExecutionAttemptHealth::Starting,
                    diagnostic_refs: Vec::new(),
                }
            });
        let ticks = [1_u64, 2_u64].map(|tick_sequence| EvaluationTickRecord {
            tick_id: format!("tick_{tick_sequence}"),
            execution_attempt_id: attempts[0].attempt_id.clone(),
            activation_id: attempts[0].activation_id.clone(),
            tick_sequence,
            trigger_source: EvaluationTriggerSource::MarketData,
            market_time: format!("2026-07-19T14:30:0{tick_sequence}Z"),
            event_time: format!("2026-07-19T14:30:0{tick_sequence}Z"),
            input_cursor: format!("cursor:{tick_sequence}"),
            compiled_plan_hash: "sha256:plan".to_string(),
            data_refs: vec![format!("data:{tick_sequence}")],
            decision_refs: vec![format!("decision:{tick_sequence}")],
            idempotency_key: format!("activation_1:cursor:{tick_sequence}"),
        });

        assert_eq!(version.published_from_draft_id, draft.draft_id);
        assert_ne!(version.original_spec_json, version.canonical_spec_json);
        assert_ne!(version.original_spec_hash, version.canonical_spec_hash);
        assert_eq!(config.version_id, version.version_id);
        assert_eq!(
            revisions
                .iter()
                .filter(|revision| revision.config_id == config.config_id)
                .count(),
            2
        );
        assert_ne!(
            activations[0].exclusivity_scope,
            activations[1].exclusivity_scope
        );
        assert_eq!(
            activations
                .iter()
                .filter(|activation| {
                    activation.strategy_version_id == version.version_id
                        && activation.config_revision_id == revisions[1].config_revision_id
                })
                .count(),
            2
        );
        assert_eq!(
            attempts
                .iter()
                .filter(|attempt| attempt.activation_id == activations[0].activation_id)
                .count(),
            2
        );
        assert_eq!(
            ticks
                .iter()
                .filter(|tick| tick.execution_attempt_id == attempts[0].attempt_id)
                .count(),
            2
        );

        let contracts = canonical_record_contracts();
        for immutable_record in ["StrategyVersion", "StrategyExecutionConfigRevision"] {
            let contract = contracts
                .iter()
                .find(|contract| contract.name == immutable_record)
                .expect("immutable lifecycle contract");
            assert_eq!(contract.mutability, Mutability::Immutable);
            assert_eq!(contract.status, RecordContractStatus::Canonical);
        }
    }

    #[test]
    fn publishing_a_draft_creates_a_version_without_activating_it() {
        let publication =
            StrategyDraftFsm::transition(StrategyDraftState::Valid, StrategyDraftCommand::Publish)
                .expect("publish valid draft");

        assert_eq!(publication.next_state, StrategyDraftState::Published);
        assert_eq!(
            publication.replay_outputs,
            vec!["strategy_version_id", "strategy_version_hash"]
        );
        assert!(publication
            .emitted_events
            .iter()
            .all(|event| !event.contains("activation")));
        assert!(publication
            .replay_outputs
            .iter()
            .all(|output| !output.contains("activation")));
        assert!(ActivationFsm::transition(
            ActivationState::Pending,
            ActivationCommand::EvaluateTick
        )
        .is_err());
    }

    #[test]
    fn execution_attempt_fsm_enforces_worker_tenure_boundaries() {
        let running = ExecutionAttemptFsm::transition(
            ExecutionAttemptState::Starting,
            ExecutionAttemptCommand::Start,
        )
        .expect("start attempt");
        let evaluated = ExecutionAttemptFsm::transition(
            running.next_state,
            ExecutionAttemptCommand::EvaluateTick,
        )
        .expect("evaluate tick");
        let completed = ExecutionAttemptFsm::transition(
            evaluated.next_state,
            ExecutionAttemptCommand::Complete,
        )
        .expect("complete attempt");

        assert_eq!(running.next_state, ExecutionAttemptState::Running);
        assert_eq!(evaluated.next_state, ExecutionAttemptState::Running);
        assert_eq!(evaluated.emitted_events, vec!["evaluation_tick.completed"]);
        assert_eq!(completed.next_state, ExecutionAttemptState::Completed);
        assert!(ExecutionAttemptFsm::transition(
            ExecutionAttemptState::Starting,
            ExecutionAttemptCommand::EvaluateTick,
        )
        .is_err());
        assert!(ExecutionAttemptFsm::transition(
            ExecutionAttemptState::Completed,
            ExecutionAttemptCommand::Start,
        )
        .is_err());
        assert_eq!(
            ExecutionAttemptFsm::transition(
                ExecutionAttemptState::Running,
                ExecutionAttemptCommand::LoseLease,
            )
            .expect("lose lease")
            .next_state,
            ExecutionAttemptState::LeaseLost
        );
    }

    #[test]
    fn backtest_fsm_preserves_queue_runner_and_replay_boundaries() {
        let queued =
            BacktestRunFsm::transition(BacktestRunState::Ready, BacktestRunCommand::Execute)
                .expect("queue");
        let running =
            BacktestRunFsm::transition(queued.next_state, BacktestRunCommand::Start).expect("run");
        let completed =
            BacktestRunFsm::transition(running.next_state, BacktestRunCommand::Complete)
                .expect("complete");
        let replay = BacktestRunFsm::transition(completed.next_state, BacktestRunCommand::Replay)
            .expect("replay");

        assert_eq!(queued.next_state, BacktestRunState::Queued);
        assert_eq!(queued.idempotency, IdempotencyRequirement::Required);
        assert_eq!(running.authority_class, AuthorityClass::SystemRunner);
        assert_eq!(completed.next_state, BacktestRunState::Completed);
        assert_eq!(replay.next_state, BacktestRunState::Completed);
        assert!(BacktestRunFsm::transition(
            BacktestRunState::Canceled,
            BacktestRunCommand::Complete
        )
        .is_err());
    }

    #[test]
    fn activation_fsm_keeps_continuous_execution_and_emergency_paths_distinct() {
        let active = ActivationFsm::transition(ActivationState::Pending, ActivationCommand::Start)
            .expect("start");
        let tick = ActivationFsm::transition(active.next_state, ActivationCommand::EvaluateTick)
            .expect("tick");
        let draining = ActivationFsm::transition(tick.next_state, ActivationCommand::StopAfterFlat)
            .expect("drain");
        let stopped =
            ActivationFsm::transition(draining.next_state, ActivationCommand::FlatComplete)
                .expect("stop");
        let emergency =
            ActivationFsm::transition(ActivationState::Active, ActivationCommand::EmergencyStop)
                .expect("emergency");

        assert_eq!(active.next_state, ActivationState::Active);
        assert_eq!(tick.next_state, ActivationState::Active);
        assert_eq!(draining.next_state, ActivationState::Draining);
        assert_eq!(stopped.next_state, ActivationState::Stopped);
        assert_eq!(emergency.next_state, ActivationState::EmergencyStopped);
        assert_eq!(
            emergency.authority_class,
            AuthorityClass::StrongUserAuthority
        );
        assert!(ActivationFsm::transition(
            ActivationState::Stopped,
            ActivationCommand::EvaluateTick
        )
        .is_err());
    }

    #[test]
    fn approval_and_order_intent_require_user_approval_before_submit() {
        let requested = OrderIntentFsm::transition(
            OrderIntentState::Created,
            OrderIntentCommand::RequestApproval,
        )
        .expect("request approval");
        let approved =
            OrderIntentFsm::transition(requested.next_state, OrderIntentCommand::Approve)
                .expect("approve");
        let submitted = OrderIntentFsm::transition(approved.next_state, OrderIntentCommand::Submit)
            .expect("submit");

        assert_eq!(requested.next_state, OrderIntentState::ApprovalRequested);
        assert_eq!(approved.authority_class, AuthorityClass::UserOnly);
        assert_eq!(submitted.next_state, OrderIntentState::Submitted);
        assert!(
            OrderIntentFsm::transition(OrderIntentState::Created, OrderIntentCommand::Submit)
                .is_err()
        );
    }

    #[test]
    fn capability_plugin_and_entitlement_fsms_fail_closed_on_invalid_order() {
        assert!(CapabilityResolutionFsm::transition(
            CapabilityResolutionState::Unresolved,
            CapabilityResolutionCommand::Select
        )
        .is_err());
        assert!(PluginConnectionFsm::transition(
            PluginConnectionState::Unconfigured,
            PluginConnectionCommand::Connect
        )
        .is_err());
        assert!(EntitlementGrantFsm::transition(
            EntitlementGrantState::Proposed,
            EntitlementGrantCommand::Check
        )
        .is_err());
    }

    #[test]
    fn mandate_tuple_uses_non_nullable_local_self_client() {
        let tuple = MandateDecisionTuple {
            agent: "agent_research_v1".to_string(),
            principal_user: "user_123".to_string(),
            client: ClientContext::local_self("user_123"),
            application: "tradeassembly_studio".to_string(),
            action: "create_order_proposal".to_string(),
            resource: "brokerage_account_789".to_string(),
            context_snapshot_id: "ctx_1".to_string(),
            time_window: TimeWindow {
                starts_at: "2026-07-08T00:00:00Z".to_string(),
                expires_at: "2026-07-08T20:00:00Z".to_string(),
            },
            purpose: PurposeContext {
                purpose_id: "client_request".to_string(),
                taxonomy_version: "purpose.v1".to_string(),
                explanation_evidence_ref: Some("hash:user_instruction".to_string()),
            },
            evidence_refs: vec!["hash:risk_profile".to_string()],
            policy_version: "mandate_policy_2026_07_07".to_string(),
            supervision_mode: SupervisionMode::HumanConfirmation,
        };

        assert_eq!(tuple.client.client_id, "user_123");
        assert_eq!(tuple.client.client_kind, ClientKind::SelfClient);
        assert_eq!(tuple.purpose.purpose_id, "client_request");
        assert_eq!(tuple.supervision_mode, SupervisionMode::HumanConfirmation);
    }

    #[test]
    fn apf_receipt_context_payload_policy_is_explicit() {
        let receipt = APFReceiptRecord {
            receipt_id: "rcpt_1".to_string(),
            principal_user: "user_123".to_string(),
            client: ClientContext::local_self("user_123"),
            agent: "research_agent_v1".to_string(),
            application: "tradeassembly_studio".to_string(),
            action: "create_order_proposal".to_string(),
            resource: "brokerage_account_789".to_string(),
            purpose: PurposeContext {
                purpose_id: "client_request".to_string(),
                taxonomy_version: "purpose.v1".to_string(),
                explanation_evidence_ref: None,
            },
            policy_version: "mandate_policy_2026_07_07".to_string(),
            mandate_hash: Some("hash:mandate".to_string()),
            decision: PolicyEffect::ApprovedWithHumanConfirmation,
            supervision_mode: SupervisionMode::HumanConfirmation,
            evidence_refs: vec!["hash:user_instruction".to_string()],
            context_snapshot_id: "ctx_1".to_string(),
            limits: BTreeMap::new(),
            approver: Some("user_123".to_string()),
            credential_grant_id: None,
            downstream_outcome_ref: None,
            payload_policy: ReceiptPayloadPolicy::HashOnly,
            receipt_hash: "hash:receipt".to_string(),
            previous_receipt_hash: None,
            timestamp: "2026-07-08T20:00:00Z".to_string(),
        };

        assert_eq!(receipt.payload_policy, ReceiptPayloadPolicy::HashOnly);
        assert_eq!(
            receipt.decision,
            PolicyEffect::ApprovedWithHumanConfirmation
        );
        assert_eq!(receipt.context_snapshot_id, "ctx_1");
    }
}
