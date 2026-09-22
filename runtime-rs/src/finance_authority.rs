// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::control_plane::ControlPlaneCommandEnvelope;
use crate::ports::{SideEffectContext, StoragePort};
use reqwest::blocking::{Client, Response};
#[cfg(test)]
use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DESCRIPTOR_NS: &str = "finance_action_descriptors";
pub const DECISION_NS: &str = "warden_decisions";
pub const RECEIPT_NS: &str = "finance_receipts";
pub const OUTCOME_NS: &str = "warden_downstream_outcomes";
pub const REQUIRED_WARDEN_SERVICE_VERSION: &str = "0.1.0";
pub const TRADEASSEMBLY_POLICY_BUNDLE: &str =
    include_str!("../config/warden/tradeassembly-core-policy-bundle.json");
pub const TRADEASSEMBLY_PEP_MANIFEST: &str =
    include_str!("../config/warden/tradeassembly-core-peps.json");

pub trait FinanceAuthorityPort: Send + Sync {
    /// Availability only; callers must not treat this as an order permit.
    fn verify_broker_boundary(&self) -> Result<(), String> {
        Err("broker_submission_authority_unavailable".into())
    }

    /// Separate from control-plane authorization. Implementations that do not
    /// support a broker-boundary PEP must never inherit a control-plane allow.
    fn prepare_broker_submission(
        &self,
        _storage: &dyn StoragePort,
        _envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        Err("broker_submission_authority_unavailable".into())
    }

    fn complete_broker_submission(
        &self,
        _storage: &dyn StoragePort,
        _envelope: &ControlPlaneCommandEnvelope,
        _status: u16,
    ) -> Result<(), String> {
        Err("broker_submission_authority_unavailable".into())
    }

    fn prepare_command(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<(), String>;

    fn complete_command(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
        status: u16,
    ) -> Result<(), String>;

    fn audit(&self, storage: &dyn StoragePort) -> Value;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WardenSideEffectClass {
    Write,
    Credential,
    Export,
    Admin,
}

impl WardenSideEffectClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Credential => "credential",
            Self::Export => "export",
            Self::Admin => "admin",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AuthorityActionDefinition {
    pub action: &'static str,
    pub resource_kind: &'static str,
    pub purpose: &'static str,
    pub side_effect_class: WardenSideEffectClass,
    pub baseline_decision: &'static str,
}

macro_rules! action {
    ($name:ident, $action:literal, $resource:literal, $purpose:literal, $effect:ident) => {
        pub const $name: AuthorityActionDefinition = AuthorityActionDefinition {
            action: $action,
            resource_kind: $resource,
            purpose: $purpose,
            side_effect_class: WardenSideEffectClass::$effect,
            baseline_decision: "allow",
        };
    };
    ($name:ident, $action:literal, $resource:literal, $purpose:literal, $effect:ident, $decision:literal) => {
        pub const $name: AuthorityActionDefinition = AuthorityActionDefinition {
            action: $action,
            resource_kind: $resource,
            purpose: $purpose,
            side_effect_class: WardenSideEffectClass::$effect,
            baseline_decision: $decision,
        };
    };
}

action!(
    STRATEGY_CREATE,
    "strategy.create",
    "strategy",
    "client_request",
    Write
);
action!(
    STRATEGY_EDIT,
    "strategy.draft.update",
    "strategy",
    "client_request",
    Write
);
action!(
    STRATEGY_PROPOSE,
    "strategy.draft.patch.propose",
    "strategy",
    "client_request",
    Write
);
action!(
    STRATEGY_REVIEW,
    "strategy.draft.patch.review",
    "strategy",
    "client_request",
    Write
);
action!(
    STRATEGY_PUBLISH,
    "strategy.version.publish",
    "strategy",
    "client_request",
    Write
);
action!(
    STRATEGY_LIFECYCLE,
    "strategy.lifecycle.manage",
    "strategy",
    "client_request",
    Write
);
action!(
    STUDIO_SETUP_OBSERVE,
    "studio.setup.observe",
    "plugin",
    "client_request",
    Write
);
action!(
    PLUGIN_CONFIGURE,
    "plugin.configure",
    "plugin",
    "credential_configuration",
    Admin
);
action!(
    PLUGIN_OPERATION,
    "plugin.operation.invoke",
    "plugin_operation",
    "client_request",
    Write
);
action!(
    CAPABILITY_BINDING_CONFIGURE,
    "capability.binding.revision.save",
    "execution_config",
    "strategy_configuration",
    Write
);
action!(
    ENTITLEMENT_MANAGE,
    "entitlement.manage",
    "entitlement",
    "policy_administration",
    Admin
);
action!(
    CREDENTIAL_CONFIGURE,
    "credential.configure",
    "plugin",
    "credential_configuration",
    Credential
);
action!(
    CREDENTIAL_TEST,
    "credential.test",
    "plugin",
    "credential_configuration",
    Credential
);
action!(
    CREDENTIAL_REVOKE,
    "credential.revoke",
    "plugin",
    "credential_configuration",
    Credential
);
action!(
    RESEARCH_RUN,
    "research.run.execute",
    "research_run",
    "strategy_research",
    Write
);
action!(
    RESEARCH_DATASET,
    "research.dataset.create",
    "research_dataset",
    "strategy_research",
    Write
);
action!(
    RESEARCH_DATASET_CANCEL,
    "research.dataset.cancel",
    "research_dataset",
    "strategy_research",
    Write
);
action!(
    RESEARCH_UNIVERSE,
    "research.universe.create",
    "research_universe",
    "strategy_research",
    Write
);
action!(
    RESEARCH_JOB,
    "research.job.manage",
    "research_job",
    "strategy_research",
    Write
);
action!(
    RESEARCH_NOTEBOOK,
    "research.notebook.update",
    "research_notebook",
    "strategy_research",
    Write
);
action!(
    BACKTEST_RUN,
    "backtest.run.execute",
    "backtest_run",
    "backtest",
    Write
);
action!(
    BACKTEST_EXPORT,
    "backtest.report.export",
    "backtest_report",
    "backtest",
    Export
);
action!(
    MONTE_CARLO_RUN,
    "research.monte_carlo.run",
    "research_run",
    "strategy_research",
    Write
);
action!(
    ROBUSTNESS_RUN,
    "research.robustness.run",
    "robustness_run",
    "strategy_research",
    Write
);
action!(
    ROBUSTNESS_READ,
    "research.robustness.read",
    "robustness_run",
    "strategy_research",
    Write
);
action!(
    DERIVATIVES_RUN,
    "research.derivatives.run",
    "derivatives_analysis",
    "strategy_research",
    Write
);
action!(
    DERIVATIVES_READ,
    "research.derivatives.read",
    "derivatives_analysis",
    "strategy_research",
    Write
);
action!(
    COMPARISON_RUN,
    "research.comparison.run",
    "research_comparison",
    "strategy_research",
    Write
);
action!(
    COMPARISON_READ,
    "research.comparison.read",
    "research_comparison",
    "strategy_research",
    Write
);
action!(
    SCENARIO_VALUATION_RUN,
    "research.scenario_valuation.run",
    "research_run",
    "strategy_research",
    Write
);
action!(
    FILL_QUALITY_ANALYZE,
    "research.fill_quality.analyze",
    "research_run",
    "strategy_research",
    Write
);
action!(
    LIFECYCLE_CALENDAR_BUILD,
    "research.lifecycle_calendar.build",
    "research_run",
    "strategy_research",
    Write
);
action!(
    ATTRIBUTION_JOURNAL_REVIEW,
    "research.attribution_journal.review",
    "research_run",
    "strategy_research",
    Write
);
action!(
    RESEARCH_REPORT_GENERATE,
    "research.report.generate",
    "research_run",
    "strategy_research",
    Write
);
action!(
    PORTFOLIO_RISK_RUN,
    "research.portfolio_risk.run",
    "research_run",
    "strategy_research",
    Write
);
action!(
    EXECUTION_CONFIG,
    "execution.config.save",
    "execution_config",
    "paper_trading",
    Write
);
action!(
    LIVE_MANDATE_MANAGE,
    "execution.live_mandate.manage",
    "execution_config",
    "policy_administration",
    Admin
);
action!(
    EXECUTION_ACTIVATE_PAPER,
    "execution.activate.paper",
    "activation",
    "paper_trading",
    Write
);
action!(
    EXECUTION_ACTIVATE_LIVE,
    "execution.activate.live",
    "activation",
    "live_order_submission",
    Write,
    "deny"
);
action!(
    EXECUTION_CONTROL,
    "execution.control",
    "activation",
    "paper_trading",
    Write
);
action!(
    EXECUTION_CANCEL_ORDERS,
    "execution.cancel_orders",
    "activation",
    "paper_trading",
    Admin
);
action!(
    EXECUTION_LIQUIDATE,
    "execution.liquidate",
    "activation",
    "paper_trading",
    Admin
);
action!(
    EXECUTION_DEACTIVATE,
    "execution.deactivate",
    "activation",
    "paper_trading",
    Write
);
action!(
    EXECUTION_EVALUATE,
    "execution.evaluate.paper",
    "activation",
    "paper_trading",
    Write
);
action!(
    POSITION_LIFECYCLE_MANAGE,
    "execution.position_lifecycle.manage",
    "position_lifecycle",
    "paper_trading",
    Write
);
action!(
    ORDER_SUBMIT_PAPER,
    "order.submit.paper",
    "brokerage_account",
    "paper_trading",
    Write
);
action!(
    ORDER_SUBMIT_LIVE,
    "order.submit.live",
    "brokerage_account",
    "live_order_submission",
    Write,
    "deny"
);
action!(
    ORDER_MANAGE,
    "order.manage",
    "order",
    "paper_trading",
    Write
);
action!(
    EXTERNAL_BROKER_RECEIPT_RECORD,
    "external_broker_receipt.record",
    "broker_receipt",
    "broker_reconciliation",
    Write
);
action!(
    SCHEDULER_MANAGE,
    "scheduler.manage",
    "scheduler",
    "policy_administration",
    Admin
);
action!(
    AGENT_DEPLOYMENT_MANAGE,
    "agent.deployment.manage",
    "agent_deployment",
    "agent_orchestration",
    Admin
);
action!(
    EMERGENCY_STOP,
    "execution.emergency_stop",
    "activation",
    "paper_trading",
    Admin
);
action!(
    RISK_MANAGE,
    "risk.manage",
    "risk_envelope",
    "paper_trading",
    Write
);
action!(
    POSITION_SYNC,
    "portfolio.positions.sync",
    "portfolio",
    "paper_trading",
    Write
);
action!(
    SIGHTLINE_SESSION,
    "sightline.session.mutate",
    "sightline_session",
    "client_request",
    Write
);
action!(
    SIGHTLINE_PROPOSAL,
    "sightline.proposal.create",
    "sightline_proposal",
    "client_request",
    Write
);
action!(
    SIGHTLINE_APPROVAL,
    "sightline.approval.request",
    "approval_request",
    "client_request",
    Write
);
action!(
    SHARE_MANAGE,
    "strategy.share.manage",
    "strategy_share",
    "client_request",
    Export
);
action!(
    ALERT_ACKNOWLEDGE,
    "alert.acknowledge",
    "alert",
    "client_request",
    Write
);
action!(
    INSTRUMENT_PACK_INSTALL,
    "instrument.pack.install",
    "instrument_pack",
    "policy_administration",
    Admin
);
action!(
    POLICY_MANAGE,
    "tradeassembly.policy.manage",
    "policy",
    "policy_administration",
    Admin
);
action!(
    INSTALL_CLAIM,
    "tradeassembly.install.claim",
    "installation",
    "client_request",
    Admin
);
action!(
    TELEMETRY_MANAGE,
    "tradeassembly.telemetry.manage",
    "telemetry_preference",
    "client_request",
    Admin
);

pub const AUTHORITY_ACTIONS: &[AuthorityActionDefinition] = &[
    STRATEGY_CREATE,
    STRATEGY_EDIT,
    STRATEGY_PROPOSE,
    STRATEGY_REVIEW,
    STRATEGY_PUBLISH,
    STRATEGY_LIFECYCLE,
    PLUGIN_CONFIGURE,
    PLUGIN_OPERATION,
    STUDIO_SETUP_OBSERVE,
    CAPABILITY_BINDING_CONFIGURE,
    ENTITLEMENT_MANAGE,
    CREDENTIAL_CONFIGURE,
    CREDENTIAL_TEST,
    CREDENTIAL_REVOKE,
    RESEARCH_RUN,
    RESEARCH_DATASET,
    RESEARCH_DATASET_CANCEL,
    RESEARCH_UNIVERSE,
    RESEARCH_JOB,
    RESEARCH_NOTEBOOK,
    BACKTEST_RUN,
    BACKTEST_EXPORT,
    MONTE_CARLO_RUN,
    SCENARIO_VALUATION_RUN,
    FILL_QUALITY_ANALYZE,
    LIFECYCLE_CALENDAR_BUILD,
    ATTRIBUTION_JOURNAL_REVIEW,
    RESEARCH_REPORT_GENERATE,
    PORTFOLIO_RISK_RUN,
    EXECUTION_CONFIG,
    LIVE_MANDATE_MANAGE,
    EXECUTION_ACTIVATE_PAPER,
    EXECUTION_ACTIVATE_LIVE,
    EXECUTION_CONTROL,
    EXECUTION_CANCEL_ORDERS,
    EXECUTION_LIQUIDATE,
    EXECUTION_DEACTIVATE,
    EXECUTION_EVALUATE,
    POSITION_LIFECYCLE_MANAGE,
    ORDER_SUBMIT_PAPER,
    ORDER_SUBMIT_LIVE,
    ORDER_MANAGE,
    EXTERNAL_BROKER_RECEIPT_RECORD,
    SCHEDULER_MANAGE,
    AGENT_DEPLOYMENT_MANAGE,
    EMERGENCY_STOP,
    RISK_MANAGE,
    POSITION_SYNC,
    SIGHTLINE_SESSION,
    SIGHTLINE_PROPOSAL,
    SIGHTLINE_APPROVAL,
    SHARE_MANAGE,
    ALERT_ACKNOWLEDGE,
    INSTRUMENT_PACK_INSTALL,
    POLICY_MANAGE,
    INSTALL_CLAIM,
    TELEMETRY_MANAGE,
];

pub fn authority_definition(
    envelope: &ControlPlaneCommandEnvelope,
) -> Result<&'static AuthorityActionDefinition, String> {
    if envelope.side_effect_class == "read" {
        return Err("read_action_not_protected".to_string());
    }
    let command = envelope.command_name.as_str();
    let payload = &envelope.payload_preview;
    if envelope.side_effect_class == "live_order" {
        return Ok(&ORDER_SUBMIT_LIVE);
    }
    if envelope.side_effect_class == "paper_order" {
        return Ok(&ORDER_SUBMIT_PAPER);
    }
    if command == "execution.activate" {
        return Ok(if account_mode(envelope) == "live" {
            &EXECUTION_ACTIVATE_LIVE
        } else {
            &EXECUTION_ACTIVATE_PAPER
        });
    }
    if command == "execution.control" {
        return Ok(match string_field(payload, &["action"]).as_deref() {
            Some("emergency_stop") => &EMERGENCY_STOP,
            Some("cancel_orders") => &EXECUTION_CANCEL_ORDERS,
            Some("liquidate") => &EXECUTION_LIQUIDATE,
            _ => &EXECUTION_CONTROL,
        });
    }
    let definition = match command {
        "studio.setup.observe" => &STUDIO_SETUP_OBSERVE,
        "strategy.create" => &STRATEGY_CREATE,
        "strategy.draft.save" | "strategy.semantic.select" => &STRATEGY_EDIT,
        "strategy.draft.patch.propose" => &STRATEGY_PROPOSE,
        "strategy.draft.patch.review" => &STRATEGY_REVIEW,
        "strategy.version.publish" => &STRATEGY_PUBLISH,
        "strategy.duplicate" | "strategy.archive" | "strategy.restore" => &STRATEGY_LIFECYCLE,
        "capability.binding.revision.save" => &CAPABILITY_BINDING_CONFIGURE,
        "plugin.manifest.install"
        | "plugin.package.install"
        | "plugin.instance.create"
        | "plugin.instance.update"
        | "plugin.instance.enable"
        | "plugin.instance.disable"
        | "plugin.instance.upgrade"
        | "plugin.instance.rollback"
        | "plugin.instance.remove"
        | "plugin.instance.health.refresh" => &PLUGIN_CONFIGURE,
        "entitlement.grant" | "entitlement.deny" | "entitlement.revoke" => &ENTITLEMENT_MANAGE,
        "credential.store" => &CREDENTIAL_CONFIGURE,
        "credential.test" => &CREDENTIAL_TEST,
        "credential.revoke" => &CREDENTIAL_REVOKE,
        "research.run.create" | "research.sweep.create" | "research.promote.paper" => &RESEARCH_RUN,
        "research.dataset.create" => &RESEARCH_DATASET,
        "research.dataset.cancel" => &RESEARCH_DATASET_CANCEL,
        "research.universe.create" => &RESEARCH_UNIVERSE,
        "research.job.create" | "research.job.status" => &RESEARCH_JOB,
        "research.notebook.update"
        | "research_notebook.create"
        | "research_notebook.compose"
        | "research_notebook.attach" => &RESEARCH_NOTEBOOK,
        "backtest.run.execute" => &BACKTEST_RUN,
        "backtest.report.export" => &BACKTEST_EXPORT,
        "research.monte_carlo.run" | "monte_carlo.run" => &MONTE_CARLO_RUN,
        "research.robustness.run" => &ROBUSTNESS_RUN,
        "research.robustness.read" => &ROBUSTNESS_READ,
        "research.derivatives.run" => &DERIVATIVES_RUN,
        "research.derivatives.read" => &DERIVATIVES_READ,
        "research.comparison.run" => &COMPARISON_RUN,
        "research.comparison.read" => &COMPARISON_READ,
        "research.scenario_valuation.run" => &SCENARIO_VALUATION_RUN,
        "research.fill_quality.analyze" => &FILL_QUALITY_ANALYZE,
        "research.lifecycle_calendar.build" => &LIFECYCLE_CALENDAR_BUILD,
        "research.attribution_journal.review" => &ATTRIBUTION_JOURNAL_REVIEW,
        "research.report.generate" => &RESEARCH_REPORT_GENERATE,
        "research.portfolio_risk.run" | "portfolio_risk.run" => &PORTFOLIO_RISK_RUN,
        "execution.config.save" => &EXECUTION_CONFIG,
        "execution.live_mandate.issue" | "execution.live_mandate.revoke" => &LIVE_MANDATE_MANAGE,
        "execution.control" => &EXECUTION_CONTROL,
        "execution.deactivate" => &EXECUTION_DEACTIVATE,
        "execution.evaluate" | "execution.run" | "execution.run_once" => &EXECUTION_EVALUATE,
        "execution.position_lifecycle.manage" => &POSITION_LIFECYCLE_MANAGE,
        "orders.reconcile" => &ORDER_MANAGE,
        "external_broker_receipt.append" | "external_broker_receipt.reconcile" => {
            &EXTERNAL_BROKER_RECEIPT_RECORD
        }
        "scheduler.start"
        | "scheduler.run"
        | "scheduler.stop"
        | "execution.scheduler.start"
        | "execution.scheduler.run"
        | "execution.scheduler.stop" => &SCHEDULER_MANAGE,
        "agent.deployment.create"
        | "agent.deployment.start"
        | "agent.deployment.pause"
        | "agent.deployment.stop"
        | "agent.run.recover" => &AGENT_DEPLOYMENT_MANAGE,
        "sightline.surface.register"
        | "sightline.strategy_editor.publish"
        | "sightline.studio_surface.publish"
        | "sightline.selection.set"
        | "sightline.selection.share"
        | "sightline.agent.connect"
        | "sightline.presence.update"
        | "sightline.navigation.request"
        | "sightline.node.focus"
        | "sightline.node.highlight" => &SIGHTLINE_SESSION,
        "sightline.proposal.create" => &SIGHTLINE_PROPOSAL,
        "sightline.approval.request" => &SIGHTLINE_APPROVAL,
        "strategy.share.create" | "strategy.share.revoke" | "strategy.share.export" => {
            &SHARE_MANAGE
        }
        "alert.acknowledge" => &ALERT_ACKNOWLEDGE,
        "instrument.pack.install" => &INSTRUMENT_PACK_INSTALL,
        "provider.policy.update" => &POLICY_MANAGE,
        "install.claim" => &INSTALL_CLAIM,
        "telemetry.emit" | "telemetry.opt_out" => &TELEMETRY_MANAGE,
        _ if command.starts_with("plugins.instances.") && command.ends_with(".invoke.post") => {
            &PLUGIN_OPERATION
        }
        _ if command.starts_with("orders.") && command.ends_with(".status_events.post") => {
            &ORDER_MANAGE
        }
        _ if command.starts_with("risk.") => &RISK_MANAGE,
        _ if command == "portfolio.positions.sync" => &POSITION_SYNC,
        _ => return Err(format!("unregistered_protected_action:{command}")),
    };
    Ok(definition)
}

#[derive(Clone, Debug)]
pub struct WardenSidecarAuthority {
    client: Client,
    base_url: String,
    bearer_token: String,
    required_version: String,
    policy_bundle: Value,
}

impl WardenSidecarAuthority {
    pub fn new(
        base_url: impl Into<String>,
        bearer_token: impl Into<String>,
        required_version: impl Into<String>,
    ) -> Result<Self, String> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err("Warden sidecar URL must be absolute HTTP(S)".to_string());
        }
        let bearer_token = bearer_token.into();
        if bearer_token.len() < 32 {
            return Err("Warden sidecar token must contain at least 32 bytes".to_string());
        }
        let required_version = required_version.into();
        if required_version.trim().is_empty() {
            return Err("required Warden service version is empty".to_string());
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| format!("Warden HTTP client failed: {error}"))?;
        Ok(Self {
            client,
            base_url,
            bearer_token,
            required_version,
            policy_bundle: serde_json::from_str(TRADEASSEMBLY_POLICY_BUNDLE)
                .map_err(|_| "tradeassembly_policy_bundle_invalid".to_string())?,
        })
    }

    /// Install-time dependency configuration, never an operation argument.
    /// Warden still validates and signs every decision under this exact bundle.
    /// The default constructor retains the embedded shipping policy.
    pub fn with_policy_bundle(mut self, bundle: Value) -> Result<Self, String> {
        if bundle["schema_version"] != "apf.policy_bundle.v1"
            || !bundle["rules"].is_array()
            || serde_json::to_vec(&bundle)
                .map_err(|_| "warden_policy_invalid")?
                .len()
                > 1_048_576
        {
            return Err("warden_policy_invalid".into());
        }
        self.policy_bundle = bundle;
        Ok(self)
    }

    pub fn preflight(&self) -> Result<(), String> {
        let response = self
            .client
            .get(format!("{}/health", self.base_url))
            .send()
            .map_err(|_| "warden_unavailable".to_string())?;
        let body = response_json(response, "warden_health_failed")?;
        if body["status"] != "ok" || body["service"] != "warden-sidecar" {
            return Err("warden_health_invalid".to_string());
        }
        if body["version"].as_str() != Some(self.required_version.as_str()) {
            return Err(format!(
                "warden_version_mismatch:required={}:actual={}",
                self.required_version,
                body["version"].as_str().unwrap_or("unknown")
            ));
        }
        Ok(())
    }

    fn protected_post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .bearer_auth(&self.bearer_token)
            .json(body)
            .send()
            .map_err(|_| "warden_unavailable".to_string())?;
        response_json(response, "warden_request_failed")
    }

    fn protected_get(&self, path: &str) -> Result<Value, String> {
        let response = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .bearer_auth(&self.bearer_token)
            .send()
            .map_err(|_| "warden_unavailable".to_string())?;
        response_json(response, "warden_request_failed")
    }

    fn authorize(
        &self,
        envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<AuthorizationEvidence, String> {
        self.authorize_at(envelope, EnforcementPoint::ControlPlane)
    }

    fn authorize_at(
        &self,
        envelope: &ControlPlaneCommandEnvelope,
        pep: EnforcementPoint,
    ) -> Result<AuthorizationEvidence, String> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| self.authorize_blocking(envelope, pep))
                .join()
                .map_err(|_| "warden_transport_worker_failed".to_string())?
        })
    }

    fn authorize_blocking(
        &self,
        envelope: &ControlPlaneCommandEnvelope,
        pep: EnforcementPoint,
    ) -> Result<AuthorizationEvidence, String> {
        self.preflight()?;
        let installed = self.protected_post("/v1/policies", &self.policy_bundle)?;
        let policy_version = installed
            .get("policy_version")
            .cloned()
            .ok_or_else(|| "warden_policy_version_missing".to_string())?;
        let definition =
            authority_definition(envelope).map_err(|error| format!("denied:{error}"))?;
        let request = authorization_request_at(envelope, definition, policy_version, pep);
        let decision = self.protected_post("/v1/actions/authorize", &request)?;
        validate_decision(envelope, definition, &decision, pep)?;
        let receipt_id = decision["receipt"]["receipt_id"]
            .as_str()
            .ok_or_else(|| "warden_receipt_id_missing".to_string())?;
        let signed_receipt = self.protected_get(&format!("/v1/receipts/{receipt_id}"))?;
        validate_signed_receipt(&decision, &signed_receipt)?;
        Ok(AuthorizationEvidence {
            definition: *definition,
            request,
            decision,
            signed_receipt,
        })
    }
}

impl FinanceAuthorityPort for WardenSidecarAuthority {
    fn verify_broker_boundary(&self) -> Result<(), String> {
        self.preflight()
    }

    fn prepare_broker_submission(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        validate_broker_envelope(envelope)?;
        let evidence = self.authorize_at(envelope, EnforcementPoint::BrokerSubmission)?;
        persist_authorization(storage, envelope, &evidence)?;
        if evidence.decision["decision"] != "allow" {
            return Err("denied:broker_submission_policy".into());
        }
        Ok(())
    }

    fn complete_broker_submission(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
        status: u16,
    ) -> Result<(), String> {
        validate_stored_broker_authorization(storage, envelope)?;
        persist_outcome(storage, envelope, status)
    }

    fn prepare_command(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        if envelope.side_effect_class == "read" {
            return Ok(());
        }
        let evidence = self.authorize(envelope)?;
        persist_authorization(storage, envelope, &evidence)?;
        if evidence.decision["decision"] != "allow" {
            let reasons = evidence.decision["reasons"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_else(|| "unspecified".to_string());
            return Err(format!(
                "denied:{}:{reasons}",
                evidence.decision["decision"]
                    .as_str()
                    .unwrap_or("invalid_decision")
            ));
        }
        Ok(())
    }

    fn complete_command(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
        status: u16,
    ) -> Result<(), String> {
        if envelope.side_effect_class == "read" {
            return Ok(());
        }
        if storage
            .get_json(DECISION_NS, &envelope.command_id)?
            .is_none()
        {
            return Err("warden_decision_missing".to_string());
        }
        persist_outcome(storage, envelope, status)
    }

    fn audit(&self, storage: &dyn StoragePort) -> Value {
        authority_audit(storage)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TestFinanceAuthority;

impl FinanceAuthorityPort for TestFinanceAuthority {
    fn prepare_command(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        if envelope.side_effect_class == "read" {
            return Ok(());
        }
        let definition =
            *authority_definition(envelope).map_err(|error| format!("denied:{error}"))?;
        let decision_kind = definition.baseline_decision;
        let receipt = json!({
            "schema_version": "warden.signed_receipt.v2",
            "receipt": {
                "schema_version": "apf.receipt_base.v1",
                "receipt_id": format!("test-receipt:{}", envelope.command_id),
                "receipt_type": "action_authorization",
                "action": definition.action,
                "resource": resource_ref(envelope, &definition),
                "decision": decision_kind,
                "pep_id": "pep-tradeassembly-control-plane",
                "request_digest": envelope.payload_hash,
                "receipt_hash": hash_text(&format!("{}:{}", envelope.command_id, decision_kind)),
            },
            "signature": {
                "schema_version": "warden.receipt_signature.v2",
                "algorithm": "ed25519",
                "key_id": "test-only-authority",
                "signed_at_epoch_ms": now_epoch_ms(),
                "receipt_hash": hash_text(&format!("{}:{}", envelope.command_id, decision_kind)),
                "signature": "test-only-signature"
            }
        });
        let request = authorization_request(
            envelope,
            &definition,
            json!({
                "bundle_id": "tradeassembly-core",
                "bundle_version": "test",
                "bundle_digest": hash_text("test-policy"),
                "projection_digest": hash_text("test-projection")
            }),
        );
        let decision = json!({
            "schema_version": "apf.action_authorization_decision.v1",
            "decision_id": format!("test-decision:{}", envelope.command_id),
            "request_id": envelope.command_id,
            "idempotency_key": envelope.idempotency_key.as_str(),
            "decision": decision_kind,
            "reasons": if decision_kind == "allow" { json!([]) } else { json!(["baseline_live_disabled"]) },
            "pep_id": "pep-tradeassembly-control-plane",
            "receipt": receipt["receipt"].clone()
        });
        persist_authorization(
            storage,
            envelope,
            &AuthorizationEvidence {
                definition,
                request,
                decision,
                signed_receipt: receipt,
            },
        )?;
        if decision_kind != "allow" {
            return Err(format!("denied:{decision_kind}:baseline_live_disabled"));
        }
        Ok(())
    }

    fn complete_command(
        &self,
        storage: &dyn StoragePort,
        envelope: &ControlPlaneCommandEnvelope,
        status: u16,
    ) -> Result<(), String> {
        if envelope.side_effect_class == "read" {
            return Ok(());
        }
        persist_outcome(storage, envelope, status)
    }

    fn audit(&self, storage: &dyn StoragePort) -> Value {
        authority_audit(storage)
    }
}

#[derive(Clone, Debug)]
struct AuthorizationEvidence {
    definition: AuthorityActionDefinition,
    request: Value,
    decision: Value,
    signed_receipt: Value,
}

#[derive(Clone, Copy)]
enum EnforcementPoint {
    ControlPlane,
    BrokerSubmission,
}

impl EnforcementPoint {
    fn id(self) -> &'static str {
        match self {
            Self::ControlPlane => "pep-tradeassembly-control-plane",
            Self::BrokerSubmission => "pep-tradeassembly-broker-submission",
        }
    }

    fn coverage(self) -> &'static str {
        match self {
            Self::ControlPlane => "c3",
            Self::BrokerSubmission => "c5",
        }
    }
}

fn validate_broker_envelope(envelope: &ControlPlaneCommandEnvelope) -> Result<(), String> {
    if envelope.command_name != "order.submit.live"
        || envelope.side_effect_class != "live_order"
        || envelope.authority.account_mode != "live"
        || envelope.target_object.as_deref().is_none_or(str::is_empty)
        || envelope.payload_hash.is_empty()
    {
        return Err("broker_submission_envelope_invalid".into());
    }
    Ok(())
}

/// Validate original durable C5 evidence without contacting Warden or writing
/// an outcome. This is historical evidence, never a fresh dispatch permission.
pub(crate) fn validate_stored_broker_authorization(
    storage: &dyn StoragePort,
    envelope: &ControlPlaneCommandEnvelope,
) -> Result<(), String> {
    validate_broker_envelope(envelope)?;
    let stored = storage
        .get_json(DECISION_NS, &envelope.command_id)?
        .ok_or("broker_authorization_evidence_missing")?;
    if stored["request"]["pep_id"] != EnforcementPoint::BrokerSubmission.id()
        || stored["request"]["action"] != ORDER_SUBMIT_LIVE.action
        || stored["request"]["resource"] != resource_ref(envelope, &ORDER_SUBMIT_LIVE)
        || stored["request"]["idempotency_key"] != envelope.idempotency_key.as_str()
        || stored["decision"]["decision"] != "allow"
        || stored["request"]["context"]["context_digest"]
            != hash_text(&format!(
                "{}:{}:{}",
                envelope.source_interface, envelope.command_name, envelope.payload_hash
            ))
    {
        return Err("broker_authorization_evidence_mismatch".into());
    }
    validate_decision(
        envelope,
        &ORDER_SUBMIT_LIVE,
        &stored["decision"],
        EnforcementPoint::BrokerSubmission,
    )?;
    let receipt = storage
        .get_json(RECEIPT_NS, &envelope.command_id)?
        .ok_or("broker_authorization_evidence_missing")?;
    validate_signed_receipt(&stored["decision"], &receipt)
}

fn persist_authorization(
    storage: &dyn StoragePort,
    envelope: &ControlPlaneCommandEnvelope,
    evidence: &AuthorizationEvidence,
) -> Result<(), String> {
    let context =
        SideEffectContext::new(envelope.authority.clone(), envelope.idempotency_key.clone());
    let descriptor = json!({
        "schema_version": "tradeassembly.warden_action_descriptor.v1",
        "command_id": envelope.command_id,
        "correlation_id": envelope.effective_correlation_id(),
        "command_name": envelope.command_name,
        "action_id": evidence.definition.action,
        "resource": resource_ref(envelope, &evidence.definition),
        "purpose": evidence.definition.purpose,
        "side_effect_class": evidence.definition.side_effect_class.as_str(),
        "source_interface": {
            "interface_kind": envelope.source_interface,
            "pep_id": evidence.request["pep_id"],
            "pep_coverage": evidence.request["pep_coverage"],
            "can_block": true,
            "receipt_sink_writable": true
        },
        "request_digest": envelope.payload_hash,
        "sightline_refs": redacted_context_refs(envelope),
    });
    storage.put_json(
        DECISION_NS,
        &envelope.command_id,
        json!({
            "schema_version": "tradeassembly.warden_decision_evidence.v1",
            "correlation_id": envelope.effective_correlation_id(),
            "request": evidence.request,
            "decision": evidence.decision,
        }),
        &context,
    )?;
    storage.put_json(
        RECEIPT_NS,
        &envelope.command_id,
        evidence.signed_receipt.clone(),
        &context,
    )?;
    if evidence.request["pep_id"] == EnforcementPoint::BrokerSubmission.id()
        && (storage.get_json(RECEIPT_NS, &envelope.command_id)?
            != Some(evidence.signed_receipt.clone())
            || storage
                .get_json(DECISION_NS, &envelope.command_id)?
                .as_ref()
                .map(|value| &value["request"])
                != Some(&evidence.request))
    {
        return Err("broker_authorization_evidence_not_durable".into());
    }
    // Do not persist a writable-sink assertion before its receipt write/readback.
    storage.put_json(DESCRIPTOR_NS, &envelope.command_id, descriptor, &context)
}

fn persist_outcome(
    storage: &dyn StoragePort,
    envelope: &ControlPlaneCommandEnvelope,
    status: u16,
) -> Result<(), String> {
    storage.put_json(
        OUTCOME_NS,
        &envelope.command_id,
        json!({
            "schema_version": "tradeassembly.warden_downstream_outcome.v1",
            "command_id": envelope.command_id,
            "correlation_id": envelope.effective_correlation_id(),
            "idempotency_key": envelope.idempotency_key.as_str(),
            "status": status,
            "outcome": if status < 400 { "succeeded" } else { "failed" },
            "recorded_at_epoch_ms": now_epoch_ms(),
        }),
        &SideEffectContext::new(envelope.authority.clone(), envelope.idempotency_key.clone()),
    )
}

pub fn authority_audit(storage: &dyn StoragePort) -> Value {
    let decisions = list_namespace(storage, DECISION_NS);
    json!({
        "schemaVersion": "tradeassembly.warden_authority_audit.v1",
        "descriptors": list_namespace(storage, DESCRIPTOR_NS),
        "decisions": decisions,
        "receipts": list_namespace(storage, RECEIPT_NS),
        "outcomes": list_namespace(storage, OUTCOME_NS),
    })
}

fn authorization_request(
    envelope: &ControlPlaneCommandEnvelope,
    definition: &AuthorityActionDefinition,
    policy_version: Value,
) -> Value {
    authorization_request_at(
        envelope,
        definition,
        policy_version,
        EnforcementPoint::ControlPlane,
    )
}

fn authorization_request_at(
    envelope: &ControlPlaneCommandEnvelope,
    definition: &AuthorityActionDefinition,
    policy_version: Value,
    pep: EnforcementPoint,
) -> Value {
    let now = now_epoch_ms();
    let expires = now.saturating_add(300_000);
    let actor = sanitize_id(&envelope.authority.actor);
    let principal_kind = if envelope.source_interface == "local_setup"
        && string_field(&envelope.payload_preview, &["actorKind", "actor_kind"]).as_deref()
            == Some("application")
    {
        "application"
    } else {
        "user"
    };
    let agent = string_field(&envelope.payload_preview, &["agent", "agentId", "agent_id"])
        .unwrap_or_else(|| format!("{}-surface", envelope.source_interface));
    let resource = resource_ref(envelope, definition);
    let context_digest = hash_text(&format!(
        "{}:{}:{}",
        envelope.source_interface, envelope.command_name, envelope.payload_hash
    ));
    let evidence = authority_evidence(envelope);
    json!({
        "schema_version": "apf.action_authorization_request.v1",
        "request_id": envelope.command_id,
        "idempotency_key": envelope.idempotency_key.as_str(),
        "session_id": string_field(&envelope.payload_preview, &["sessionId", "session_id"])
            .unwrap_or_else(|| format!("tradeassembly-session:{}", envelope.source_interface)),
        "session_expires_at_epoch_ms": expires,
        "agent": entity("agent", sanitize_id(&agent)),
        "principal_user": entity(principal_kind, actor),
        "client": entity("workspace", "self"),
        "application": entity("application", "tradeassembly-studio-core"),
        "action": definition.action,
        "resource": resource.clone(),
        "context": {
            "schema_version": "apf.context_snapshot.v1",
            "snapshot_id": format!("context:{}", envelope.command_id),
            "actor_refs": [entity("agent", sanitize_id(&agent))],
            "resource_refs": [resource],
            "capture_policy": "hash_only",
            "context_digest": context_digest,
            "extensions": {
                "tradeassembly.control_plane.context.v1:correlation_id": {
                    "schema_id": "tradeassembly.control_plane.context.v1",
                    "field": "correlation_id",
                    "decision_critical": false,
                    "value_digest": hash_text(envelope.effective_correlation_id())
                }
            }
        },
        "time_window": {
            "valid_from_epoch_ms": now.saturating_sub(1_000),
            "valid_until_epoch_ms": expires
        },
        "purpose": {
            "purpose_id": definition.purpose,
            "taxonomy_version": "warden.finance.baseline.v1",
            "explanation_evidence_ref": "evidence:user-intent"
        },
        "evidence": evidence,
        "policy_version": policy_version,
        "approval_mode": "none",
        "supervision_mode": "observe",
        "side_effect_class": definition.side_effect_class.as_str(),
        "pep_coverage": pep.coverage(),
        "pep_id": pep.id(),
        "requested_at_epoch_ms": now,
        "payload_policy": "hash_only"
    })
}

fn validate_decision(
    envelope: &ControlPlaneCommandEnvelope,
    definition: &AuthorityActionDefinition,
    decision: &Value,
    pep: EnforcementPoint,
) -> Result<(), String> {
    if decision["schema_version"] != "apf.action_authorization_decision.v1"
        || decision["request_id"] != envelope.command_id
        || decision["idempotency_key"] != envelope.idempotency_key.as_str()
        || decision["pep_id"] != pep.id()
        || decision["receipt"]["action"] != definition.action
        || decision["receipt"]["resource"]["kind"] != definition.resource_kind
        || decision["receipt"]["pep_id"] != pep.id()
        || decision["receipt"]["decision"] != decision["decision"]
    {
        return Err("warden_decision_mismatch".to_string());
    }
    if !matches!(decision["decision"].as_str(), Some("allow" | "deny")) {
        return Err("warden_decision_invalid".to_string());
    }
    Ok(())
}

fn validate_signed_receipt(decision: &Value, signed: &Value) -> Result<(), String> {
    if signed["schema_version"] != "warden.signed_receipt.v2"
        || signed["receipt"] != decision["receipt"]
        || signed["signature"]["schema_version"] != "warden.receipt_signature.v2"
        || signed["signature"]["algorithm"] != "ed25519"
        || signed["signature"]["receipt_hash"] != signed["receipt"]["receipt_hash"]
        || signed["signature"]["key_id"]
            .as_str()
            .is_none_or(str::is_empty)
        || signed["signature"]["signature"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err("warden_signed_receipt_invalid".to_string());
    }
    Ok(())
}

fn response_json(response: Response, code: &str) -> Result<Value, String> {
    let status = response.status();
    let body: Value = response
        .json()
        .map_err(|_| format!("{code}:invalid_json"))?;
    if !status.is_success() {
        return Err(format!(
            "{code}:{}:{}",
            status.as_u16(),
            body["code"].as_str().unwrap_or("unknown")
        ));
    }
    Ok(body)
}

fn authority_evidence(envelope: &ControlPlaneCommandEnvelope) -> Vec<Value> {
    let mut evidence = vec![json!({
        "evidence_id": "user-intent",
        "digest": hash_text(&format!("{}:{}", envelope.command_name, envelope.payload_hash)),
        "capture_policy": "hash_only"
    })];
    evidence.extend(
        envelope
            .evidence_refs
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                json!({
                    "evidence_id": format!("external-reference-{index}"),
                    "digest": hash_text(reference),
                    "capture_policy": "hash_only"
                })
            }),
    );
    evidence
}

fn resource_ref(
    envelope: &ControlPlaneCommandEnvelope,
    definition: &AuthorityActionDefinition,
) -> Value {
    let id = envelope
        .target_object
        .clone()
        .or_else(|| {
            string_field(
                &envelope.payload_preview,
                &[
                    "strategyId",
                    "strategy_id",
                    "activationId",
                    "activation_id",
                    "pluginRef",
                    "plugin_ref",
                    "providerRef",
                    "provider_ref",
                    "backtestId",
                    "backtest_id",
                    "orderId",
                    "order_id",
                ],
            )
        })
        .unwrap_or_else(|| envelope.command_id.clone());
    entity(definition.resource_kind, sanitize_id(&id))
}

fn redacted_context_refs(envelope: &ControlPlaneCommandEnvelope) -> Vec<Value> {
    [
        "sightlineRefs",
        "sightline_refs",
        "contextRefs",
        "context_refs",
    ]
    .iter()
    .find_map(|key| envelope.payload_preview.get(key).and_then(Value::as_array))
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(|reference| {
        json!({
            "kind": "sightline_context_digest",
            "id": hash_text(reference)
        })
    })
    .collect()
}

fn account_mode(envelope: &ControlPlaneCommandEnvelope) -> String {
    string_field(
        &envelope.payload_preview,
        &["accountMode", "account_mode", "mode"],
    )
    .unwrap_or_else(|| envelope.authority.account_mode.clone())
    .to_ascii_lowercase()
}

fn string_field(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| payload.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn entity(kind: &str, id: impl Into<String>) -> Value {
    json!({"kind": kind, "id": id.into()})
}

fn sanitize_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn hash_text(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn list_namespace(storage: &dyn StoragePort, namespace: &str) -> Vec<Value> {
    let mut items = storage.list_json(namespace).unwrap_or_default();
    items.sort_by(|left, right| left.0.cmp(&right.0));
    items.into_iter().map(|(_, value)| value).collect()
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct PolicyPack {
    rules: Vec<PolicyRule>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct PolicyRule {
    action: String,
    resource_kind: String,
    side_effect_class: String,
    required_decision: String,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct PepPackEntry {
    actions: Vec<String>,
    resources: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::local::sqlite::LocalSqliteStorage;
    use crate::control_plane::{graphql_envelope, http_envelope, mcp_envelope};
    use axum::{
        extract::{Path as AxumPath, State},
        http::StatusCode,
        routing::{get, post},
        Json, Router,
    };
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    const TEST_TOKEN: &str = "test-warden-token-with-at-least-32-bytes";

    #[derive(Clone, Copy, Debug)]
    enum MockBehavior {
        Allow,
        Deny,
        VersionMismatch,
        DecisionReceiptMismatch,
        ReceiptMismatch,
        Unauthorized,
    }

    #[derive(Debug)]
    struct MockState {
        behavior: MockBehavior,
        request: Mutex<Option<Value>>,
        receipt: Mutex<Option<Value>>,
    }

    struct MockServer {
        base_url: String,
        state: Arc<MockState>,
        shutdown: Option<oneshot::Sender<()>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn spawn_mock_warden(behavior: MockBehavior) -> MockServer {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("mock listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("mock address");
        let state = Arc::new(MockState {
            behavior,
            request: Mutex::new(None),
            receipt: Mutex::new(None),
        });
        let server_state = Arc::clone(&state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("mock runtime");
            runtime.block_on(async move {
                let listener =
                    tokio::net::TcpListener::from_std(listener).expect("tokio mock listener");
                let router = Router::new()
                    .route("/health", get(mock_health))
                    .route("/v1/policies", post(mock_policy))
                    .route("/v1/actions/authorize", post(mock_authorize))
                    .route("/v1/receipts/{receipt_id}", get(mock_receipt))
                    .with_state(server_state);
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("mock Warden serves");
            });
        });
        MockServer {
            base_url: format!("http://{address}"),
            state,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        }
    }

    async fn mock_health(State(state): State<Arc<MockState>>) -> Json<Value> {
        Json(json!({
            "status": "ok",
            "service": "warden-sidecar",
            "version": if matches!(state.behavior, MockBehavior::VersionMismatch) {
                "9.9.9"
            } else {
                REQUIRED_WARDEN_SERVICE_VERSION
            }
        }))
    }

    async fn mock_policy(
        State(state): State<Arc<MockState>>,
        Json(_bundle): Json<Value>,
    ) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
        if matches!(state.behavior, MockBehavior::Unauthorized) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"code": "unauthorized"})),
            ));
        }
        Ok(Json(json!({
            "policy_version": {
                "bundle_id": "tradeassembly-studio-core",
                "bundle_version": "2026-07-21.1",
                "bundle_digest": hash_text("bundle"),
                "projection_digest": hash_text("projection")
            }
        })))
    }

    async fn mock_authorize(
        State(state): State<Arc<MockState>>,
        Json(request): Json<Value>,
    ) -> Json<Value> {
        *state.request.lock().expect("request lock") = Some(request.clone());
        let decision_kind = if matches!(state.behavior, MockBehavior::Deny) {
            "deny"
        } else {
            "allow"
        };
        let receipt_hash = hash_text("mock-receipt");
        let receipt_decision = if matches!(state.behavior, MockBehavior::DecisionReceiptMismatch) {
            "deny"
        } else {
            decision_kind
        };
        let receipt = json!({
            "schema_version": "apf.receipt_base.v1",
            "receipt_id": format!("receipt:{}", request["request_id"].as_str().unwrap_or("missing")),
            "receipt_type": "action_authorization",
            "action": request["action"],
            "resource": request["resource"],
            "decision": receipt_decision,
            "pep_id": request["pep_id"],
            "request_digest": request["context"]["context_digest"],
            "receipt_hash": receipt_hash,
        });
        let signed = json!({
            "schema_version": "warden.signed_receipt.v2",
            "receipt": receipt,
            "signature": {
                "schema_version": "warden.receipt_signature.v2",
                "algorithm": "ed25519",
                "key_id": "mock-key",
                "signed_at_epoch_ms": now_epoch_ms(),
                "receipt_hash": receipt_hash,
                "signature": "mock-signature"
            }
        });
        *state.receipt.lock().expect("receipt lock") = Some(signed.clone());
        Json(json!({
            "schema_version": "apf.action_authorization_decision.v1",
            "decision_id": format!("decision:{}", request["request_id"].as_str().unwrap_or("missing")),
            "request_id": request["request_id"],
            "idempotency_key": request["idempotency_key"],
            "decision": decision_kind,
            "reasons": if decision_kind == "deny" { json!(["policy_denied"]) } else { json!([]) },
            "pep_id": request["pep_id"],
            "receipt": signed["receipt"]
        }))
    }

    async fn mock_receipt(
        State(state): State<Arc<MockState>>,
        AxumPath(_receipt_id): AxumPath<String>,
    ) -> Json<Value> {
        let mut receipt = state
            .receipt
            .lock()
            .expect("receipt lock")
            .clone()
            .expect("authorization receipt");
        if matches!(state.behavior, MockBehavior::ReceiptMismatch) {
            receipt["receipt"]["action"] = json!("forged.action");
        }
        Json(receipt)
    }

    fn strategy_envelope(idempotency_key: &str) -> ControlPlaneCommandEnvelope {
        http_envelope(
            "http",
            "POST",
            "/product/strategies/create",
            &json!({
                "name": "Warden contract test",
                "idempotencyKey": idempotency_key,
                "apiSecret": "must-not-survive-redaction",
                "sightlineRefs": ["sightline://private/context"]
            }),
        )
        .expect("strategy envelope")
    }

    #[test]
    fn only_local_setup_can_bind_an_application_principal() {
        let body = json!({
            "idempotencyKey": "plugin-package:package-digest:manifest-digest",
            "actor": {
                "kind": "application",
                "id": "tradeassembly.local_setup",
                "principalUser": "tradeassembly.local_setup"
            },
            "actorKind": "application",
            "principalUser": "tradeassembly.local_setup",
            "authorityContext": {
                "actor": "tradeassembly.local_setup",
                "surface": "local_setup",
                "accountMode": "paper"
            }
        });
        let local_setup =
            http_envelope("local_setup", "POST", "/plugins/packages", &body).expect("envelope");
        let definition = authority_definition(&local_setup).expect("authority definition");
        let request = authorization_request(&local_setup, definition, json!({"version": "test"}));
        assert_eq!(request["principal_user"]["kind"], "application");
        assert_eq!(request["principal_user"]["id"], "tradeassembly.local_setup");

        let spoofed = http_envelope("http", "POST", "/plugins/packages", &body).expect("envelope");
        let definition = authority_definition(&spoofed).expect("authority definition");
        let request = authorization_request(&spoofed, definition, json!({"version": "test"}));
        assert_eq!(request["principal_user"]["kind"], "user");
    }

    fn test_storage(name: &str) -> (tempfile::TempDir, LocalSqliteStorage) {
        let directory = tempfile::tempdir().expect("temporary storage");
        let path = directory.path().join(format!("{name}.sqlite"));
        (
            directory,
            LocalSqliteStorage::new(path.display().to_string()),
        )
    }

    #[test]
    fn policy_configuration_is_explicit_bounded_and_leaves_default_unchanged() {
        let authority = WardenSidecarAuthority::new(
            "http://127.0.0.1:1",
            "fixture-only-token".repeat(4),
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .unwrap();
        let original: Value = serde_json::from_str(TRADEASSEMBLY_POLICY_BUNDLE).unwrap();
        assert_eq!(authority.policy_bundle, original);
        for invalid in [
            Value::Null,
            json!({"schema_version":"other","rules":[]}),
            json!({"schema_version":"apf.policy_bundle.v1","rules":null}),
            json!({"schema_version":"apf.policy_bundle.v1","rules":[],"padding":"x".repeat(1_048_576)}),
        ] {
            assert!(authority.clone().with_policy_bundle(invalid).is_err());
        }
        let mut changed = original.clone();
        changed["version"] = json!("isolated-version");
        assert_eq!(
            authority
                .clone()
                .with_policy_bundle(changed.clone())
                .unwrap()
                .policy_bundle,
            changed
        );
        assert_eq!(authority.policy_bundle, original);
    }

    #[test]
    fn policy_pep_and_registry_are_exactly_aligned() {
        let policy: PolicyPack = serde_json::from_str(TRADEASSEMBLY_POLICY_BUNDLE).expect("policy");
        let peps: Vec<PepPackEntry> =
            serde_json::from_str(TRADEASSEMBLY_PEP_MANIFEST).expect("peps");
        assert_eq!(peps.len(), 2);
        assert_eq!(peps[1].actions, vec!["order.submit.live"]);
        assert_eq!(peps[1].resources, vec!["brokerage_account"]);
        let registrations: Value = serde_json::from_str(TRADEASSEMBLY_PEP_MANIFEST).unwrap();
        assert_eq!(registrations[0]["coverage_class"], "c3");
        assert_eq!(registrations[1]["coverage_class"], "c5");
        assert_eq!(
            registrations[1]["pep_id"],
            EnforcementPoint::BrokerSubmission.id()
        );
        let bundle: Value = serde_json::from_str(TRADEASSEMBLY_POLICY_BUNDLE).unwrap();
        let live_rule = bundle["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["action"] == "order.submit.live")
            .unwrap();
        assert_eq!(live_rule["min_pep_coverage"], "c5");
        assert_eq!(
            live_rule["required_decision"], "deny",
            "admission cannot open before the broker-boundary mandate guard is installed"
        );
        let registry = AUTHORITY_ACTIONS
            .iter()
            .map(|entry| {
                (
                    entry.action.to_string(),
                    entry.resource_kind.to_string(),
                    entry.side_effect_class.as_str().to_string(),
                    entry.baseline_decision.to_string(),
                )
            })
            .collect::<BTreeSet<_>>();
        let rules = policy
            .rules
            .into_iter()
            .map(|rule| {
                (
                    rule.action,
                    rule.resource_kind,
                    rule.side_effect_class,
                    rule.required_decision,
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            registry.len(),
            AUTHORITY_ACTIONS.len(),
            "duplicate registry action"
        );
        assert_eq!(rules, registry);
        assert_eq!(
            peps[0].actions.iter().cloned().collect::<BTreeSet<_>>(),
            registry.iter().map(|entry| entry.0.clone()).collect()
        );
        assert_eq!(
            peps[0].resources.iter().cloned().collect::<BTreeSet<_>>(),
            registry.iter().map(|entry| entry.1.clone()).collect()
        );
    }

    #[test]
    fn every_documented_http_mutation_resolves_to_a_registered_action() {
        let mut unresolved = Vec::new();
        for route in crate::http::documented_routes() {
            if matches!(
                route,
                "POST /graphql" | "POST /demo/reset" | "POST /demo/reset-btc"
            ) {
                continue;
            }
            let (method, template) = route.split_once(' ').expect("documented route shape");
            let path = template
                .replace("{strategy_id}", "strat_local_btc_demo")
                .replace("{backtest_id}", "backtest-local")
                .replace("{order_id}", "order-local")
                .replace("{operation_id}", "status")
                .replace("{operation}", "inspect")
                .replace("{ref}", "alpaca-paper");
            let envelope = http_envelope(
                "http",
                method,
                &path,
                &json!({
                    "strategyId": "strat_local_btc_demo",
                    "accountMode": "paper",
                    "action": "pause"
                }),
            )
            .expect("documented route envelope");
            if envelope.side_effect_class != "read" {
                if let Err(error) = authority_definition(&envelope) {
                    unresolved.push(format!("{route} => {} ({error})", envelope.command_name));
                }
            }
        }
        assert!(
            unresolved.is_empty(),
            "documented mutations missing Warden actions:\n{}",
            unresolved.join("\n")
        );
    }

    #[test]
    fn interface_worker_plugin_and_emergency_paths_map_exact_actions() {
        let http = http_envelope(
            "http",
            "POST",
            "/product/strategies/backtests/run",
            &json!({"idempotencyKey": "http-backtest"}),
        )
        .expect("HTTP envelope");
        let graphql = graphql_envelope(&json!({
            "operationName": "RunBacktest",
            "query": "mutation RunBacktest { runBacktest }",
            "variables": {"idempotencyKey": "graphql-backtest"}
        }))
        .expect("GraphQL envelope");
        let mcp = mcp_envelope(
            "tradeassembly.backtest.run",
            &json!({"idempotencyKey": "mcp-backtest"}),
        )
        .expect("MCP envelope");
        for envelope in [&http, &graphql, &mcp] {
            assert_eq!(
                authority_definition(envelope).expect("backtest").action,
                BACKTEST_RUN.action
            );
        }

        let scheduler = http_envelope(
            "worker",
            "POST",
            "/scheduler/run",
            &json!({"idempotencyKey": "scheduler"}),
        )
        .expect("scheduler envelope");
        assert_eq!(
            authority_definition(&scheduler).expect("scheduler").action,
            SCHEDULER_MANAGE.action
        );

        let plugin = http_envelope(
            "plugin",
            "POST",
            "/plugins/instances/alpaca/invoke",
            &json!({"idempotencyKey": "plugin-operation"}),
        )
        .expect("plugin envelope");
        assert_eq!(
            authority_definition(&plugin)
                .expect("plugin operation")
                .action,
            PLUGIN_OPERATION.action
        );

        let emergency = http_envelope(
            "studio",
            "POST",
            "/product/strategy-execution-activations/control",
            &json!({
                "idempotencyKey": "emergency",
                "action": "emergency_stop",
                "accountMode": "paper"
            }),
        )
        .expect("emergency envelope");
        assert_eq!(
            authority_definition(&emergency)
                .expect("emergency stop")
                .action,
            EMERGENCY_STOP.action
        );

        let sightline_surface = http_envelope(
            "studio",
            "POST",
            "/product/sightline/surfaces/register",
            &json!({"idempotencyKey": "sightline-surface"}),
        )
        .expect("Sightline envelope");
        assert_eq!(
            authority_definition(&sightline_surface)
                .expect("Sightline surface")
                .action,
            SIGHTLINE_SESSION.action
        );
    }

    fn broker_envelope(key: &str) -> ControlPlaneCommandEnvelope {
        let mut envelope = strategy_envelope(key);
        envelope.command_name = "order.submit.live".into();
        envelope.side_effect_class = "live_order".into();
        envelope.authority.account_mode = "live".into();
        envelope.source_interface = "plugin_operation_boundary".into();
        envelope.target_object = Some("account://controlled-test".into());
        envelope
    }

    #[test]
    fn broker_authority_uses_distinct_pep_and_bound_durable_evidence() {
        let server = spawn_mock_warden(MockBehavior::Allow);
        let authority = WardenSidecarAuthority::new(
            &server.base_url,
            TEST_TOKEN,
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .unwrap();
        let (_directory, storage) = test_storage("broker-pep");
        let envelope = broker_envelope("broker-pep");
        authority
            .prepare_broker_submission(&storage, &envelope)
            .unwrap();
        let request = server.state.request.lock().unwrap().clone().unwrap();
        assert_eq!(request["pep_id"], "pep-tradeassembly-broker-submission");
        assert_eq!(request["pep_coverage"], "c5");
        assert_eq!(request["action"], "order.submit.live");
        let descriptor = storage
            .get_json(DESCRIPTOR_NS, &envelope.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(descriptor["source_interface"]["pep_id"], request["pep_id"]);
        assert_eq!(descriptor["source_interface"]["pep_coverage"], "c5");
        let original_decisions = storage.list_json(DECISION_NS).unwrap();
        let original_receipts = storage.list_json(RECEIPT_NS).unwrap();
        validate_stored_broker_authorization(&storage, &envelope).unwrap();
        assert_eq!(storage.list_json(DECISION_NS).unwrap(), original_decisions);
        assert_eq!(storage.list_json(RECEIPT_NS).unwrap(), original_receipts);
        assert!(storage
            .get_json(OUTCOME_NS, &envelope.command_id)
            .unwrap()
            .is_none());
        authority
            .complete_broker_submission(&storage, &envelope, 202)
            .unwrap();
        let outcome = storage
            .get_json(OUTCOME_NS, &envelope.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            outcome["correlation_id"],
            envelope.effective_correlation_id()
        );

        let mut changed = envelope.clone();
        changed.payload_hash = "changed-payload".into();
        assert_eq!(
            authority
                .complete_broker_submission(&storage, &changed, 202)
                .unwrap_err(),
            "broker_authorization_evidence_mismatch"
        );
        changed = envelope.clone();
        changed.target_object = Some("account://different-account".into());
        assert_eq!(
            authority
                .complete_broker_submission(&storage, &changed, 202)
                .unwrap_err(),
            "broker_authorization_evidence_mismatch"
        );

        // A normal control-plane receipt must not be presented as C5 evidence.
        authority.prepare_command(&storage, &envelope).unwrap();
        assert_eq!(
            authority
                .complete_broker_submission(&storage, &envelope, 202)
                .unwrap_err(),
            "broker_authorization_evidence_mismatch"
        );
    }

    #[test]
    fn broker_authority_rejects_wrong_envelope_policy_and_storage_failure() {
        let server = spawn_mock_warden(MockBehavior::Allow);
        let authority = WardenSidecarAuthority::new(
            &server.base_url,
            TEST_TOKEN,
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .unwrap();
        let (directory, storage) = test_storage("broker-errors");
        let envelope = broker_envelope("broker-errors");
        assert_eq!(
            authority
                .prepare_broker_submission(&storage, &strategy_envelope("not-broker"))
                .unwrap_err(),
            "broker_submission_envelope_invalid"
        );
        assert!(server.state.request.lock().unwrap().is_none());
        assert_eq!(
            TestFinanceAuthority
                .prepare_broker_submission(&storage, &envelope)
                .unwrap_err(),
            "broker_submission_authority_unavailable"
        );
        let unavailable = LocalSqliteStorage::new(directory.path().display().to_string());
        assert!(authority
            .prepare_broker_submission(&unavailable, &envelope)
            .is_err());
        let denied = spawn_mock_warden(MockBehavior::Deny);
        let authority = WardenSidecarAuthority::new(
            &denied.base_url,
            TEST_TOKEN,
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .unwrap();
        assert_eq!(
            authority
                .prepare_broker_submission(&storage, &envelope)
                .unwrap_err(),
            "denied:broker_submission_policy"
        );
        assert!(authority
            .complete_broker_submission(&storage, &envelope, 202)
            .is_err());
        assert!(storage
            .get_json(OUTCOME_NS, &envelope.command_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn warden_sidecar_allow_persists_bound_redacted_evidence() {
        let server = spawn_mock_warden(MockBehavior::Allow);
        let authority = WardenSidecarAuthority::new(
            &server.base_url,
            TEST_TOKEN,
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .expect("authority");
        let (_directory, storage) = test_storage("allow");
        let envelope = strategy_envelope("warden-allow");

        authority
            .prepare_command(&storage, &envelope)
            .expect("Warden allows command");
        authority
            .complete_command(&storage, &envelope, 201)
            .expect("outcome persists");

        let request = server
            .state
            .request
            .lock()
            .expect("request lock")
            .clone()
            .expect("authorization request");
        assert_eq!(request["action"], "strategy.create");
        assert_eq!(request["resource"]["kind"], "strategy");
        assert_eq!(request["purpose"]["purpose_id"], "client_request");
        assert_eq!(request["idempotency_key"], "warden-allow");
        assert_eq!(
            request["context"]["extensions"]
                ["tradeassembly.control_plane.context.v1:correlation_id"]["schema_id"],
            "tradeassembly.control_plane.context.v1"
        );
        assert_eq!(
            request["context"]["extensions"]
                ["tradeassembly.control_plane.context.v1:correlation_id"]["field"],
            "correlation_id"
        );
        assert_eq!(
            request["context"]["extensions"]
                ["tradeassembly.control_plane.context.v1:correlation_id"]["decision_critical"],
            false
        );
        assert_eq!(
            request["context"]["extensions"]
                ["tradeassembly.control_plane.context.v1:correlation_id"]["value_digest"],
            hash_text(envelope.effective_correlation_id())
        );
        let audit = authority.audit(&storage);
        for (key, extension) in request["context"]["extensions"].as_object().unwrap() {
            assert_eq!(
                key,
                &format!(
                    "{}:{}",
                    extension["schema_id"].as_str().unwrap(),
                    extension["field"].as_str().unwrap()
                )
            );
        }
        let serialized = serde_json::to_string(&audit).expect("audit serializes");
        assert!(!serialized.contains("must-not-survive-redaction"));
        assert!(!serialized.contains("sightline://private/context"));
        assert_eq!(audit["receipts"][0]["signature"]["algorithm"], "ed25519");
        assert_eq!(audit["outcomes"][0]["status"], 201);
    }

    #[test]
    fn warden_sidecar_failures_are_closed_before_dispatch() {
        for (behavior, expected) in [
            (MockBehavior::Deny, "denied:deny:policy_denied"),
            (MockBehavior::VersionMismatch, "warden_version_mismatch"),
            (
                MockBehavior::DecisionReceiptMismatch,
                "warden_decision_mismatch",
            ),
            (
                MockBehavior::ReceiptMismatch,
                "warden_signed_receipt_invalid",
            ),
            (
                MockBehavior::Unauthorized,
                "warden_request_failed:401:unauthorized",
            ),
        ] {
            let server = spawn_mock_warden(behavior);
            let authority = WardenSidecarAuthority::new(
                &server.base_url,
                TEST_TOKEN,
                REQUIRED_WARDEN_SERVICE_VERSION,
            )
            .expect("authority");
            let (_directory, storage) = test_storage("failure");
            let error = authority
                .prepare_command(&storage, &strategy_envelope("warden-failure"))
                .expect_err("failure must deny dispatch");
            assert!(error.contains(expected), "{behavior:?}: {error}");
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("unused port");
        let unavailable_url = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        let authority = WardenSidecarAuthority::new(
            unavailable_url,
            TEST_TOKEN,
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .expect("authority");
        let (_directory, storage) = test_storage("unavailable");
        assert_eq!(
            authority
                .prepare_command(&storage, &strategy_envelope("warden-unavailable"))
                .expect_err("outage fails closed"),
            "warden_unavailable"
        );
    }
}
