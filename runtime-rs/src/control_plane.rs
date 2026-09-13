// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::finance_authority::FinanceAuthorityPort;
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext, StoragePort};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

pub const COMMAND_NS: &str = "control_plane_commands";
pub const SEQUENCE_NS: &str = "control_plane_sequences";
const SCHEMA_VERSION: &str = "tradeassembly.control_plane.command.v1";

const SENSITIVE_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "api_secret",
    "authorization",
    "bearer",
    "client_secret",
    "code_verifier",
    "password",
    "private_key",
    "refresh_token",
    "secret",
    "token",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlPlaneCommandEnvelope {
    pub schema_version: String,
    pub command_id: String,
    #[serde(default)]
    pub correlation_id: String,
    pub command_name: String,
    pub command_group: String,
    pub source_interface: String,
    pub target_object: Option<String>,
    pub side_effect_class: String,
    pub authority: AuthorityContext,
    pub idempotency_key: IdempotencyKey,
    pub idempotency_requirement: String,
    pub expected_sequence: Option<u64>,
    pub evidence_refs: Vec<String>,
    pub payload_hash: String,
    pub payload_preview: Value,
}

impl ControlPlaneCommandEnvelope {
    pub fn effective_correlation_id(&self) -> &str {
        if self.correlation_id.is_empty() {
            &self.command_id
        } else {
            &self.correlation_id
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlPlaneCommandRecord {
    pub envelope: ControlPlaneCommandEnvelope,
    pub duplicate: bool,
    pub response_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<Value>,
}

pub fn http_envelope(
    source_interface: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> Result<ControlPlaneCommandEnvelope, String> {
    let method = method.to_ascii_uppercase();
    let route_path = path.split('?').next().unwrap_or(path);
    let command_name = http_command_name(&method, route_path);
    let side_effect_class = classify_side_effect(&method, route_path, body);
    envelope_from_parts(
        source_interface,
        &command_name,
        &side_effect_class,
        target_object(route_path, body),
        body,
    )
}

pub fn graphql_envelope(request: &Value) -> Result<ControlPlaneCommandEnvelope, String> {
    let operation = graphql_operation_name(request);
    let operation = if operation.is_empty() {
        "unknown".to_string()
    } else {
        operation
    };
    let command_name = graphql_command_name(&operation);
    let route_path = graphql_operation_route(&operation);
    let effective_payload = graphql_effective_payload(request, &operation);
    let side_effect_class = if graphql_is_mutation(request) {
        classify_side_effect("POST", route_path, &effective_payload)
    } else {
        "read".to_string()
    };
    envelope_from_parts(
        "graphql",
        &command_name,
        &side_effect_class,
        target_object(route_path, &effective_payload),
        &effective_payload,
    )
}

pub fn mcp_envelope(name: &str, arguments: &Value) -> Result<ControlPlaneCommandEnvelope, String> {
    let command_name = mcp_command_name(name);
    let route_path = mcp_route_path(name, &command_name);
    let side_effect_class = if mcp_is_read(name) {
        "read".to_string()
    } else if mcp_uses_route_classification(name, &command_name) {
        classify_side_effect("POST", &route_path, arguments)
    } else {
        "mutation".to_string()
    };
    envelope_from_parts(
        "mcp",
        &command_name,
        &side_effect_class,
        target_object(&route_path, arguments),
        arguments,
    )
}

pub fn prepare_command(
    storage: &dyn StoragePort,
    finance_authority: &dyn FinanceAuthorityPort,
    envelope: ControlPlaneCommandEnvelope,
) -> Result<ControlPlaneCommandRecord, String> {
    let existing = storage.get_json(COMMAND_NS, &envelope.command_id)?;
    let (duplicate, response_status, response_body) = match existing {
        Some(value) => {
            let mut record: ControlPlaneCommandRecord =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            normalize_historical_correlation(&mut record);
            if record.envelope.payload_hash != envelope.payload_hash {
                return Err("idempotency_conflict".to_string());
            }
            if envelope.side_effect_class == "read" {
                return persist_prepared_record(
                    storage,
                    finance_authority,
                    envelope,
                    false,
                    None,
                    None,
                );
            }
            let completed = record.response_status.is_some();
            (completed, record.response_status, record.response_body)
        }
        None => (false, None, None),
    };
    persist_prepared_record(
        storage,
        finance_authority,
        envelope,
        duplicate,
        response_status,
        response_body,
    )
}

fn persist_prepared_record(
    storage: &dyn StoragePort,
    finance_authority: &dyn FinanceAuthorityPort,
    envelope: ControlPlaneCommandEnvelope,
    duplicate: bool,
    response_status: Option<u16>,
    response_body: Option<Value>,
) -> Result<ControlPlaneCommandRecord, String> {
    let record = ControlPlaneCommandRecord {
        envelope: envelope.clone(),
        duplicate,
        response_status,
        response_body,
    };
    storage.put_json(
        COMMAND_NS,
        &envelope.command_id,
        serde_json::to_value(&record).map_err(|error| error.to_string())?,
        &SideEffectContext::new(envelope.authority.clone(), envelope.idempotency_key.clone()),
    )?;
    if duplicate {
        return Ok(record);
    }
    if let Err(error) = finance_authority.prepare_command(storage, &envelope) {
        if error.starts_with("denied:") {
            let denied = ControlPlaneCommandRecord {
                envelope: envelope.clone(),
                duplicate,
                response_status: Some(403),
                response_body: None,
            };
            storage.put_json(
                COMMAND_NS,
                &envelope.command_id,
                serde_json::to_value(&denied).map_err(|error| error.to_string())?,
                &SideEffectContext::new(
                    envelope.authority.clone(),
                    envelope.idempotency_key.clone(),
                ),
            )?;
        }
        return Err(format!("finance_authority:{error}"));
    }
    if let Err(error) = validate_expected_sequence(storage, &envelope) {
        complete_command(storage, finance_authority, &envelope, 409)?;
        return Err(error);
    }
    Ok(record)
}

pub fn complete_command(
    storage: &dyn StoragePort,
    finance_authority: &dyn FinanceAuthorityPort,
    envelope: &ControlPlaneCommandEnvelope,
    status: u16,
) -> Result<(), String> {
    complete_command_with_body(storage, finance_authority, envelope, status, None)
}

pub fn complete_command_with_body(
    storage: &dyn StoragePort,
    finance_authority: &dyn FinanceAuthorityPort,
    envelope: &ControlPlaneCommandEnvelope,
    status: u16,
    response_body: Option<Value>,
) -> Result<(), String> {
    let mut record = storage
        .get_json(COMMAND_NS, &envelope.command_id)?
        .map(serde_json::from_value::<ControlPlaneCommandRecord>)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| ControlPlaneCommandRecord {
            envelope: envelope.clone(),
            duplicate: false,
            response_status: None,
            response_body: None,
        });
    normalize_historical_correlation(&mut record);
    record.response_status = Some(status);
    if let Some(response_body) = response_body {
        record.response_body = Some(redact_sensitive_payload(response_body));
    }
    finance_authority
        .complete_command(storage, envelope, status)
        .map_err(|error| format!("finance_authority:{error}"))?;
    storage.put_json(
        COMMAND_NS,
        &envelope.command_id,
        serde_json::to_value(&record).map_err(|error| error.to_string())?,
        &SideEffectContext::new(envelope.authority.clone(), envelope.idempotency_key.clone()),
    )
}

pub fn list_commands(storage: &dyn StoragePort) -> Result<Vec<ControlPlaneCommandRecord>, String> {
    let mut records = storage
        .list_json(COMMAND_NS)?
        .into_iter()
        .map(|(_, value)| serde_json::from_value::<ControlPlaneCommandRecord>(value))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    records
        .iter_mut()
        .for_each(normalize_historical_correlation);
    records.sort_by(|left, right| {
        left.envelope
            .command_id
            .cmp(&right.envelope.command_id)
            .then(
                left.envelope
                    .source_interface
                    .cmp(&right.envelope.source_interface),
            )
    });
    Ok(records)
}

fn normalize_historical_correlation(record: &mut ControlPlaneCommandRecord) {
    if record.envelope.correlation_id.is_empty() {
        record.envelope.correlation_id = record.envelope.command_id.clone();
    }
}

fn envelope_from_parts(
    source_interface: &str,
    command_name: &str,
    side_effect_class: &str,
    target_object: Option<String>,
    body: &Value,
) -> Result<ControlPlaneCommandEnvelope, String> {
    validate_key_part("source_interface", source_interface)?;
    validate_key_part("command_name", command_name)?;
    let payload_hash = command_payload_hash(command_name, body)?;
    let payload_preview = redact_sensitive_payload(body.clone());
    let authority = authority_from_body(source_interface, body);
    let idempotency_key = idempotency_key_from_body(
        source_interface,
        command_name,
        side_effect_class,
        body,
        &payload_hash,
    )?;
    let command_id = command_id(source_interface, command_name, idempotency_key.as_str());
    let correlation_id = command_id.clone();
    Ok(ControlPlaneCommandEnvelope {
        schema_version: SCHEMA_VERSION.to_string(),
        command_id,
        correlation_id,
        command_name: command_name.to_string(),
        command_group: command_group(command_name).to_string(),
        source_interface: source_interface.to_string(),
        target_object,
        side_effect_class: side_effect_class.to_string(),
        authority,
        idempotency_key,
        idempotency_requirement: if side_effect_class == "read" {
            "optional".to_string()
        } else {
            "required".to_string()
        },
        expected_sequence: expected_sequence(body),
        evidence_refs: evidence_refs(body),
        payload_hash,
        payload_preview,
    })
}

fn command_payload_hash(command_name: &str, body: &Value) -> Result<String, String> {
    if command_name == "plugin.package.install" {
        let integrity = nested_value(body, "integrity");
        let package_sha256 = integrity
            .and_then(|value| value.get("packageSha256"))
            .and_then(Value::as_str);
        let manifest_sha256 = integrity
            .and_then(|value| value.get("manifestSha256"))
            .and_then(Value::as_str);
        if let (Some(package_sha256), Some(manifest_sha256)) = (package_sha256, manifest_sha256) {
            return hash_json(&json!({
                "packageSha256": package_sha256,
                "manifestSha256": manifest_sha256,
            }));
        }
    }
    hash_json(body)
}

fn http_command_name(method: &str, path: &str) -> String {
    match (method, path) {
        ("GET", "/workspace/shell") => "workspace.shell.get".to_string(),
        ("GET", "/workspace") => "workspace.get".to_string(),
        ("GET", "/strategies") => "strategy.list".to_string(),
        ("POST", "/strategies") | ("POST", "/product/strategies/create") => {
            "strategy.create".to_string()
        }
        ("POST", "/strategies/draft") | ("POST", "/product/strategies/save-draft") => {
            "strategy.draft.save".to_string()
        }
        ("POST", "/product/strategies/semantic-selection") => {
            "strategy.semantic.select".to_string()
        }
        ("POST", "/product/strategies/proposals/create") => {
            "strategy.draft.patch.propose".to_string()
        }
        ("POST", "/product/strategies/proposals/review")
        | ("POST", "/product/strategies/proposals/apply") => {
            "strategy.draft.patch.review".to_string()
        }
        ("GET", "/product/sightline/session-state") => "sightline.session.get".to_string(),
        ("POST", "/product/sightline/surfaces/register") => {
            "sightline.surface.register".to_string()
        }
        ("POST", "/product/sightline/strategy-editor/publish") => {
            "sightline.strategy_editor.publish".to_string()
        }
        ("POST", "/product/sightline/studio-surface/publish") => {
            "sightline.studio_surface.publish".to_string()
        }
        ("POST", "/product/sightline/selections/set") => "sightline.selection.set".to_string(),
        ("POST", "/product/sightline/selections/share") => "sightline.selection.share".to_string(),
        ("POST", "/product/sightline/context/get") => "sightline.context.get".to_string(),
        ("POST", "/product/sightline/agents/connect") => "sightline.agent.connect".to_string(),
        ("POST", "/product/sightline/presence/update") => "sightline.presence.update".to_string(),
        ("POST", "/product/sightline/navigation/request") => {
            "sightline.navigation.request".to_string()
        }
        ("POST", "/product/sightline/nodes/focus") => "sightline.node.focus".to_string(),
        ("POST", "/product/sightline/nodes/highlight") => "sightline.node.highlight".to_string(),
        ("POST", "/product/sightline/proposals/create") => "sightline.proposal.create".to_string(),
        ("POST", "/product/sightline/approvals/request") => {
            "sightline.approval.request".to_string()
        }
        ("GET", "/product/sightline/events") => "sightline.events.list".to_string(),
        ("POST", "/product/strategies/publish") => "strategy.version.publish".to_string(),
        ("POST", "/capabilities/graph/resolve") => "capability.graph.resolve".to_string(),
        ("POST", "/capability-graph-revisions") => "capability.binding.revision.save".to_string(),
        ("POST", "/capability-graph-revisions/currentness") => {
            "capability.graph.currentness.check".to_string()
        }
        ("POST", "/product/strategies/duplicate") => "strategy.duplicate".to_string(),
        ("POST", "/product/strategies/archive") => "strategy.archive".to_string(),
        ("POST", "/product/strategies/restore") => "strategy.restore".to_string(),
        ("POST", "/product/run-center/run-once") | ("POST", "/run") => {
            "execution.run_once".to_string()
        }
        ("POST", "/demo/btc-exit") | ("POST", "/exit-watches/process") => {
            "execution.run".to_string()
        }
        ("POST", "/product/strategy-execution-configs/save") => "execution.config.save".to_string(),
        ("POST", "/product/live-mandates/issue") => "execution.live_mandate.issue".to_string(),
        ("POST", "/product/live-mandates/revoke") => "execution.live_mandate.revoke".to_string(),
        ("POST", "/product/live-mandates/status") => "execution.live_mandate.status".to_string(),
        ("POST", "/product/strategy-execution-configs/activation-readiness") => {
            "execution.activation.preflight".to_string()
        }
        ("POST", "/product/strategy-execution-activations/activate") => {
            "execution.activate".to_string()
        }
        ("POST", "/product/strategy-execution-activations/deactivate") => {
            "execution.deactivate".to_string()
        }
        ("POST", "/product/strategy-execution-activations/control") => {
            "execution.control".to_string()
        }
        ("POST", path)
            if path.starts_with("/product/strategy-execution-activations/")
                && path.ends_with("/ticks/evaluate") =>
        {
            "execution.evaluate".to_string()
        }
        ("POST", "/scheduler/run") | ("POST", "/scheduler/tick") => "scheduler.run".to_string(),
        ("POST", "/product/scheduler/start") | ("POST", "/scheduler/start") => {
            "scheduler.start".to_string()
        }
        ("POST", "/product/scheduler/stop") | ("POST", "/scheduler/stop") => {
            "scheduler.stop".to_string()
        }
        ("POST", "/orders/reconcile") => "orders.reconcile".to_string(),
        ("POST", "/orders/broker-recovery") => "orders.broker_recovery".to_string(),
        ("POST", "/product/strategies/research-runs/create") => "backtest.run.execute".to_string(),
        ("POST", "/robustness-runs") | ("POST", "/robustness-runs:process") => {
            "research.robustness.run".to_string()
        }
        ("POST", path)
            if path.starts_with("/robustness-runs/")
                && matches!(
                    path.rsplit('/').next(),
                    Some("cancel" | "retry" | "replay" | "export")
                ) =>
        {
            "research.robustness.run".to_string()
        }
        ("GET", "/robustness-runs") => "research.robustness.read".to_string(),
        ("GET", path) if path.starts_with("/robustness-runs/") => {
            "research.robustness.read".to_string()
        }
        ("POST", "/derivatives-analyses") => "research.derivatives.run".to_string(),
        ("GET", "/derivatives-analyses") => "research.derivatives.read".to_string(),
        ("GET", path) if path.starts_with("/derivatives-analyses/") => {
            "research.derivatives.read".to_string()
        }
        ("POST", "/research-comparisons") => "research.comparison.run".to_string(),
        ("GET", "/research-comparisons") => "research.comparison.read".to_string(),
        ("GET", path) if path.starts_with("/research-comparisons/") => {
            "research.comparison.read".to_string()
        }
        ("POST", "/dataset-ingestions") => "research.dataset.create".to_string(),
        ("POST", path) if path.starts_with("/dataset-ingestions/") && path.ends_with("/cancel") => {
            "research.dataset.cancel".to_string()
        }
        ("POST", "/product/strategies/backtests/run")
        | ("POST", "/backtests")
        | ("POST", "/backtests:process")
        | ("POST", "/strategies/{strategy_id}/backtests") => "backtest.run.execute".to_string(),
        ("POST", path)
            if path.starts_with("/backtests/")
                && matches!(path.rsplit('/').next(), Some("cancel" | "retry")) =>
        {
            "backtest.run.execute".to_string()
        }
        ("POST", "/product/strategies/backtests/export") => "backtest.report.export".to_string(),
        ("POST", path) if path.starts_with("/backtests/") && path.ends_with("/export") => {
            "backtest.report.export".to_string()
        }
        ("POST", path) if path.starts_with("/strategies/") && path.ends_with("/backtests") => {
            "backtest.run.execute".to_string()
        }
        ("POST", path) if path.starts_with("/product/strategies/scenario-valuation") => {
            "research.scenario_valuation.run".to_string()
        }
        ("POST", path) if path.starts_with("/product/strategies/monte-carlo") => {
            "research.monte_carlo.run".to_string()
        }
        ("POST", path) if path.starts_with("/product/strategies/portfolio-risk") => {
            "research.portfolio_risk.run".to_string()
        }
        ("POST", "/execution/fill-quality") => "research.fill_quality.analyze".to_string(),
        ("POST", path) if path.starts_with("/product/strategies/fill-quality") => {
            "research.fill_quality.analyze".to_string()
        }
        ("POST", "/execution/lifecycle-calendar") => {
            "research.lifecycle_calendar.build".to_string()
        }
        ("POST", path) if path.starts_with("/product/strategies/lifecycle-calendar") => {
            "research.lifecycle_calendar.build".to_string()
        }
        ("POST", "/journal/attribution-review") => {
            "research.attribution_journal.review".to_string()
        }
        ("POST", path) if path.starts_with("/product/strategies/attribution-journal") => {
            "research.attribution_journal.review".to_string()
        }
        ("POST", "/research/notebook") => "research.notebook.update".to_string(),
        ("POST", path) if path.starts_with("/product/strategies/research-notebook") => {
            "research.notebook.update".to_string()
        }
        ("POST", "/product/reports/envelope") => "research.report.generate".to_string(),
        ("POST", path) if path.starts_with("/execution/position-lifecycle/") => {
            "execution.position_lifecycle.manage".to_string()
        }
        ("POST", "/product/providers/credentials/test") => "credential.test".to_string(),
        ("POST", "/product/strategies/ai-draft") => "strategy.draft.patch.propose".to_string(),
        ("PUT", "/providers/policy") => "provider.policy.update".to_string(),
        ("POST", "/entitlements/grant") => "entitlement.grant".to_string(),
        ("POST", "/entitlements/deny") => "entitlement.deny".to_string(),
        ("DELETE", path) if path.starts_with("/entitlements/") => "entitlement.revoke".to_string(),
        ("POST", "/plugins/manifests") => "plugin.manifest.install".to_string(),
        ("POST", "/plugins/packages") => "plugin.package.install".to_string(),
        ("POST", "/plugins/instances") => "plugin.instance.create".to_string(),
        ("POST", "/plugins/oauth/start" | "/plugins/oauth/status") => {
            "credential.store".to_string()
        }
        ("POST", "/plugins/oauth/disconnect") => "credential.revoke".to_string(),
        ("PUT", path)
            if path.starts_with("/plugins/instances/") && path.ends_with("/configuration") =>
        {
            "plugin.instance.update".to_string()
        }
        ("DELETE", path)
            if path.starts_with("/plugins/instances/")
                && path.trim_matches('/').split('/').count() == 3 =>
        {
            "plugin.instance.remove".to_string()
        }
        ("POST", path) if path.starts_with("/plugins/instances/") && path.ends_with(":enable") => {
            "plugin.instance.enable".to_string()
        }
        ("POST", path) if path.starts_with("/plugins/instances/") && path.ends_with(":disable") => {
            "plugin.instance.disable".to_string()
        }
        ("POST", path) if path.starts_with("/plugins/instances/") && path.ends_with(":upgrade") => {
            "plugin.instance.upgrade".to_string()
        }
        ("POST", path)
            if path.starts_with("/plugins/instances/") && path.ends_with(":rollback") =>
        {
            "plugin.instance.rollback".to_string()
        }
        ("POST", path)
            if path.starts_with("/plugins/instances/") && path.ends_with("/health:refresh") =>
        {
            "plugin.instance.health.refresh".to_string()
        }
        (_, path) if path.starts_with("/plugins/instances/") && path.ends_with("/credentials") => {
            match method {
                "GET" => "credential.status".to_string(),
                "DELETE" => "credential.revoke".to_string(),
                _ => "credential.store".to_string(),
            }
        }
        ("POST", "/product/providers/update-instance") => "plugin.instance.update".to_string(),
        ("POST", "/product/providers/enable-instance") => "plugin.instance.enable".to_string(),
        ("POST", "/product/providers/disable-instance") => "plugin.instance.disable".to_string(),
        ("POST", "/research/datasets")
        | ("POST", "/product/strategies/research/datasets/create") => {
            "research.dataset.create".to_string()
        }
        ("POST", "/research/universes") => "research.universe.create".to_string(),
        ("POST", "/research/jobs") | ("POST", "/product/strategies/research/jobs/create") => {
            "research.job.create".to_string()
        }
        ("POST", "/product/strategies/research/jobs/status") => "research.job.status".to_string(),
        ("POST", "/research/sweeps") | ("POST", "/product/strategies/research-sweeps/create") => {
            "research.sweep.create".to_string()
        }
        ("POST", "/product/strategies/research/promote-to-paper") => {
            "research.promote.paper".to_string()
        }
        ("POST", "/product/alerts/acknowledge") => "alert.acknowledge".to_string(),
        ("POST", "/product/strategies/share-snapshots/create") => {
            "strategy.share.create".to_string()
        }
        ("POST", "/product/strategies/share-snapshots/revoke") => {
            "strategy.share.revoke".to_string()
        }
        ("POST", "/product/share-snapshots/export") => "strategy.share.export".to_string(),
        ("POST", "/marketdata/instrument-packs/install")
        | ("POST", "/instruments/packs/install") => "instrument.pack.install".to_string(),
        ("POST", "/portfolio/positions/sync") => "portfolio.positions.sync".to_string(),
        _ if path.starts_with("/providers/") && path.ends_with("/credentials") => match method {
            "GET" => "credential.status".to_string(),
            "DELETE" => "credential.revoke".to_string(),
            _ => "credential.store".to_string(),
        },
        _ if path.starts_with("/providers/") && path.ends_with("/credentials/test") => {
            "credential.test".to_string()
        }
        _ if path.starts_with("/strategies/") => {
            format!("strategy.{}", method.to_ascii_lowercase())
        }
        _ => fallback_command_name(method, path),
    }
}

fn fallback_command_name(method: &str, path: &str) -> String {
    let route = path
        .trim_matches('/')
        .replace("{", "")
        .replace("}", "")
        .replace(['/', ':'], ".")
        .replace('-', "_");
    if route.is_empty() {
        method.to_ascii_lowercase()
    } else {
        format!("{}.{}", route, method.to_ascii_lowercase())
    }
}

fn mcp_command_name(name: &str) -> String {
    match name {
        "tradeassembly.plugin.oauth.start" | "tradeassembly.plugin.oauth.status" => {
            return "credential.store".to_string();
        }
        "tradeassembly.plugin.oauth.disconnect" => return "credential.revoke".to_string(),
        "tradeassembly.broker.connect" => return "plugin.instance.create".to_string(),
        "tradeassembly.broker.verify" => return "plugin.instance.health.refresh".to_string(),
        "studio.deployment.create" => return "agent.deployment.create".to_string(),
        "studio.deployment.start" => return "agent.deployment.start".to_string(),
        "studio.deployment.pause" => return "agent.deployment.pause".to_string(),
        "studio.deployment.stop" => return "agent.deployment.stop".to_string(),
        "studio.deployment.inspect" => return "agent.deployment.read".to_string(),
        "studio.agent_run.inspect" | "studio.agent_run.events" | "studio.agent_run.health" => {
            return "agent.run.read".to_string()
        }
        "studio.agent_run.recover" => return "agent.run.recover".to_string(),
        "studio.execution.external_receipt.append" => {
            return "external_broker_receipt.append".to_string();
        }
        "studio.execution.external_receipt.inspect" => {
            return "external_broker_receipt.inspect".to_string();
        }
        "studio.execution.external_receipt.reconcile" => {
            return "external_broker_receipt.reconcile".to_string();
        }
        "tradeassembly.strategy.save_draft" | "tradeassembly.strategy.draft.save" => {
            return "strategy.draft.save".to_string();
        }
        "tradeassembly.strategy.draft.select_node" => {
            return "strategy.semantic.select".to_string();
        }
        "tradeassembly.strategy.draft.propose_patch" => {
            return "strategy.draft.patch.propose".to_string();
        }
        "tradeassembly.strategy.draft.review_patch"
        | "tradeassembly.strategy.draft.apply_patch" => {
            return "strategy.draft.patch.review".to_string();
        }
        "tradeassembly.strategy.version.publish" => {
            return "strategy.version.publish".to_string();
        }
        "tradeassembly.plugin.capability_graph_resolve" => {
            return "capability.graph.resolve".to_string();
        }
        "tradeassembly.plugin.capability_revision_save" => {
            return "capability.binding.revision.save".to_string();
        }
        "tradeassembly.plugin.capability_revision_get" => {
            return "capability.graph.revision.get".to_string();
        }
        "tradeassembly.plugin.capability_revision_check" => {
            return "capability.graph.currentness.check".to_string();
        }
        "tradeassembly.sightline.session_state" => {
            return "sightline.session.get".to_string();
        }
        "tradeassembly.sightline.get_current_selection" => {
            return "sightline.selection.get".to_string();
        }
        "tradeassembly.sightline.get_context" => {
            return "sightline.context.get".to_string();
        }
        "tradeassembly.sightline.create_proposal" => {
            return "sightline.proposal.create".to_string();
        }
        "tradeassembly.sightline.request_approval" => {
            return "sightline.approval.request".to_string();
        }
        "tradeassembly.sightline.navigate_surface" => {
            return "sightline.navigation.request".to_string();
        }
        "tradeassembly.sightline.list_events" => {
            return "sightline.events.list".to_string();
        }
        "tradeassembly.backtest.run" => {
            return "backtest.run.execute".to_string();
        }
        "tradeassembly.backtest.cancel"
        | "tradeassembly.backtest.retry"
        | "tradeassembly.backtest.process" => {
            return "backtest.run.execute".to_string();
        }
        "tradeassembly.robustness.run"
        | "tradeassembly.robustness.process"
        | "tradeassembly.robustness.cancel"
        | "tradeassembly.robustness.retry" => {
            return "research.robustness.run".to_string();
        }
        "tradeassembly.robustness.list" | "tradeassembly.robustness.get" => {
            return "research.robustness.read".to_string();
        }
        "tradeassembly.derivatives.create" => {
            return "research.derivatives.run".to_string();
        }
        "tradeassembly.derivatives.list"
        | "tradeassembly.derivatives.get"
        | "tradeassembly.derivatives.export" => {
            return "research.derivatives.read".to_string();
        }
        "tradeassembly.comparison.create" => {
            return "research.comparison.run".to_string();
        }
        "tradeassembly.comparison.list"
        | "tradeassembly.comparison.get"
        | "tradeassembly.comparison.export" => {
            return "research.comparison.read".to_string();
        }
        "tradeassembly.dataset_ingestion.create" => {
            return "research.dataset.create".to_string();
        }
        "tradeassembly.dataset_ingestion.cancel" => {
            return "research.dataset.cancel".to_string();
        }
        "tradeassembly.backtest.export" => {
            return "backtest.report.export".to_string();
        }
        "tradeassembly.execution.live_mandate.issue" => {
            return "execution.live_mandate.issue".to_string();
        }
        "tradeassembly.execution.live_mandate.revoke" => {
            return "execution.live_mandate.revoke".to_string();
        }
        "tradeassembly.execution.live_mandate.status" => {
            return "execution.live_mandate.status".to_string();
        }
        "tradeassembly.monte_carlo.export" => {
            return "research.monte_carlo.run".to_string();
        }
        "tradeassembly.portfolio_risk.export" => {
            return "research.portfolio_risk.run".to_string();
        }
        "tradeassembly.fill_quality.export" => {
            return "research.fill_quality.analyze".to_string();
        }
        "tradeassembly.attribution_journal.export" => {
            return "research.attribution_journal.review".to_string();
        }
        "tradeassembly.lifecycle_calendar.export" => {
            return "research.lifecycle_calendar.build".to_string();
        }
        "tradeassembly.research_notebook.export" => {
            return "research.notebook.update".to_string();
        }
        _ => {}
    }
    name.strip_prefix("tradeassembly.")
        .unwrap_or(name)
        .replace('_', "-")
        .replace('-', "_")
}

fn mcp_uses_route_classification(name: &str, command_name: &str) -> bool {
    name.starts_with("tradeassembly.plugin.oauth.")
        || name.starts_with("tradeassembly.execution.")
        || name.starts_with("studio.deployment.")
        || name.starts_with("studio.agent_run.")
        || name.starts_with("studio.execution.external_receipt.")
        || name.starts_with("tradeassembly.sightline.")
        || command_name.contains("backtest")
        || command_name.contains("research")
        || name.starts_with("tradeassembly.dataset_ingestion.")
}

fn mcp_route_path(name: &str, command_name: &str) -> String {
    match name {
        "tradeassembly.plugin.oauth.start" => "/plugins/oauth/start".to_string(),
        "tradeassembly.plugin.oauth.status" => "/plugins/oauth/status".to_string(),
        "tradeassembly.plugin.oauth.disconnect" => "/plugins/oauth/disconnect".to_string(),
        "tradeassembly.broker.connect" => "/plugins/instances".to_string(),
        "tradeassembly.broker.verify" => {
            "/plugins/instances/{instance_ref}/health:refresh".to_string()
        }
        "studio.deployment.create" => "/studio/agent-deployments".to_string(),
        "studio.deployment.start" => "/studio/agent-deployments/{deployment_id}/start".to_string(),
        "studio.deployment.pause" => "/studio/agent-deployments/{deployment_id}/pause".to_string(),
        "studio.deployment.stop" => "/studio/agent-deployments/{deployment_id}/stop".to_string(),
        "studio.deployment.inspect" => "/studio/agent-deployments/{deployment_id}".to_string(),
        "studio.agent_run.inspect" => "/studio/agent-runs/{run_id}".to_string(),
        "studio.agent_run.events" => "/studio/agent-deployments/{deployment_id}/events".to_string(),
        "studio.agent_run.health" => "/studio/agent-runs/health".to_string(),
        "studio.agent_run.recover" => {
            "/studio/agent-deployments/{deployment_id}/recover".to_string()
        }
        "studio.execution.external_receipt.append" => {
            "/studio/external-broker-receipts".to_string()
        }
        "studio.execution.external_receipt.inspect" => {
            "/studio/external-broker-receipts/{client_order_id}".to_string()
        }
        "studio.execution.external_receipt.reconcile" => {
            "/studio/external-broker-receipts/{client_order_id}/reconcile".to_string()
        }
        "tradeassembly.sightline.session_state" => "/product/sightline/session-state".to_string(),
        "tradeassembly.sightline.get_current_selection" => {
            "/product/sightline/context/get".to_string()
        }
        "tradeassembly.sightline.get_context" => "/product/sightline/context/get".to_string(),
        "tradeassembly.sightline.create_proposal" => {
            "/product/sightline/proposals/create".to_string()
        }
        "tradeassembly.sightline.request_approval" => {
            "/product/sightline/approvals/request".to_string()
        }
        "tradeassembly.sightline.navigate_surface" => {
            "/product/sightline/navigation/request".to_string()
        }
        "tradeassembly.sightline.list_events" => "/product/sightline/events".to_string(),
        "tradeassembly.execution.config.save" => {
            "/product/strategy-execution-configs/save".to_string()
        }
        "tradeassembly.execution.live_mandate.issue" => "/product/live-mandates/issue".to_string(),
        "tradeassembly.execution.live_mandate.revoke" => {
            "/product/live-mandates/revoke".to_string()
        }
        "tradeassembly.execution.live_mandate.status" => {
            "/product/live-mandates/status".to_string()
        }
        "tradeassembly.execution.readiness" => {
            "/product/strategy-execution-configs/activation-readiness".to_string()
        }
        "tradeassembly.execution.activate" => {
            "/product/strategy-execution-activations/activate".to_string()
        }
        "tradeassembly.execution.control" => {
            "/product/strategy-execution-activations/control".to_string()
        }
        "tradeassembly.execution.deactivate" => {
            "/product/strategy-execution-activations/deactivate".to_string()
        }
        "tradeassembly.execution.run" => "/product/run-center/run-once".to_string(),
        "tradeassembly.execution.scheduler.start" => "/product/scheduler/start".to_string(),
        "tradeassembly.execution.scheduler.run" => "/scheduler/run".to_string(),
        "tradeassembly.execution.scheduler.stop" => "/product/scheduler/stop".to_string(),
        "tradeassembly.backtest.run" => "/product/strategies/backtests/run".to_string(),
        "tradeassembly.backtest.get" => "/backtests/{run_id}".to_string(),
        "tradeassembly.backtest.list" => "/backtests".to_string(),
        "tradeassembly.backtest.cancel" => "/backtests/{run_id}/cancel".to_string(),
        "tradeassembly.backtest.retry" => "/backtests/{run_id}/retry".to_string(),
        "tradeassembly.backtest.process" => "/backtests:process".to_string(),
        "tradeassembly.backtest.replay" => "/backtests/{run_id}/replay".to_string(),
        "tradeassembly.backtest.export" => "/product/strategies/backtests/export".to_string(),
        "tradeassembly.robustness.run" => "/robustness-runs".to_string(),
        "tradeassembly.robustness.list" => "/robustness-runs".to_string(),
        "tradeassembly.robustness.get" => "/robustness-runs/{run_id}".to_string(),
        "tradeassembly.robustness.process" => "/robustness-runs:process".to_string(),
        "tradeassembly.robustness.cancel" => "/robustness-runs/{run_id}/cancel".to_string(),
        "tradeassembly.robustness.retry" => "/robustness-runs/{run_id}/retry".to_string(),
        "tradeassembly.derivatives.create" => "/derivatives-analyses".to_string(),
        "tradeassembly.derivatives.list" => "/derivatives-analyses".to_string(),
        "tradeassembly.derivatives.get" => "/derivatives-analyses/{analysis_id}".to_string(),
        "tradeassembly.derivatives.export" => {
            "/derivatives-analyses/{analysis_id}/exports/{format}".to_string()
        }
        "tradeassembly.comparison.create" => "/research-comparisons".to_string(),
        "tradeassembly.comparison.list" => "/research-comparisons".to_string(),
        "tradeassembly.comparison.get" => "/research-comparisons/{comparison_id}".to_string(),
        "tradeassembly.comparison.export" => {
            "/research-comparisons/{comparison_id}/exports/{format}".to_string()
        }
        "tradeassembly.dataset_ingestion.create" => "/dataset-ingestions".to_string(),
        "tradeassembly.dataset_ingestion.cancel" => {
            "/dataset-ingestions/{ingestion_id}/cancel".to_string()
        }
        "tradeassembly.dataset_ingestion.get" | "tradeassembly.dataset_ingestion.status" => {
            "/dataset-ingestions/{ingestion_id}".to_string()
        }
        "tradeassembly.dataset_ingestion.list" => "/dataset-ingestions".to_string(),
        "tradeassembly.dataset_ingestion.verify" => {
            "/dataset-ingestions/{ingestion_id}/verify".to_string()
        }
        _ => format!("/mcp/{command_name}"),
    }
}

fn graphql_command_name(operation: &str) -> String {
    match operation {
        "CreateStrategy" => "strategy.create".to_string(),
        "CreateStrategyAiDraft" => "strategy.draft.patch.propose".to_string(),
        "SaveBuilderDraft" => "strategy.draft.save".to_string(),
        "SelectStrategySemanticNode" => "strategy.semantic.select".to_string(),
        "ProposeStrategyPatch" => "strategy.draft.patch.propose".to_string(),
        "ReviewStrategyPatch" | "ApplyStrategyPatch" => "strategy.draft.patch.review".to_string(),
        "ResolveCapabilityGraph" => "capability.graph.resolve".to_string(),
        "SaveCapabilityGraphRevision" => "capability.binding.revision.save".to_string(),
        "CapabilityGraphRevision" => "capability.graph.revision.get".to_string(),
        "CheckCapabilityGraphRevision" => "capability.graph.currentness.check".to_string(),
        "SightlineSessionState" => "sightline.session.get".to_string(),
        "PublishSightlineStrategyEditorSurface" => "sightline.strategy_editor.publish".to_string(),
        "PublishSightlineStudioSurface" => "sightline.studio_surface.publish".to_string(),
        "SetSightlineSelection" => "sightline.selection.set".to_string(),
        "ShareSightlineSelection" => "sightline.selection.share".to_string(),
        "SightlineGetContext" => "sightline.context.get".to_string(),
        "ConnectSightlineAgent" => "sightline.agent.connect".to_string(),
        "SightlineUpdatePresence" => "sightline.presence.update".to_string(),
        "SightlineCreateProposal" => "sightline.proposal.create".to_string(),
        "SightlineRequestApproval" => "sightline.approval.request".to_string(),
        "SightlineNavigateSurface" => "sightline.navigation.request".to_string(),
        "SightlineFocusNode" => "sightline.node.focus".to_string(),
        "SightlineHighlightNode" => "sightline.node.highlight".to_string(),
        "SightlineEvents" => "sightline.events.list".to_string(),
        "PublishStrategy" => "strategy.version.publish".to_string(),
        "DuplicateStrategy" => "strategy.duplicate".to_string(),
        "ArchiveStrategy" => "strategy.archive".to_string(),
        "RestoreStrategy" => "strategy.restore".to_string(),
        "RunStrategyOnce" => "execution.run_once".to_string(),
        "SchedulerRun" => "scheduler.run".to_string(),
        "SchedulerStart" => "scheduler.start".to_string(),
        "SchedulerStop" => "scheduler.stop".to_string(),
        "SaveExecutionConfig" => "execution.config.save".to_string(),
        "ActivationReadiness" => "execution.activation.preflight".to_string(),
        "ActivateExecution" => "execution.activate".to_string(),
        "ControlStrategyExecution" => "execution.control".to_string(),
        "DeactivateStrategyExecution" => "execution.deactivate".to_string(),
        "ReconcileOrders" => "orders.reconcile".to_string(),
        "StoreCredentials" => "credential.store".to_string(),
        "TestCredentials" => "credential.test".to_string(),
        "InstallPluginManifest" => "plugin.manifest.install".to_string(),
        "InstallPluginPackage" => "plugin.package.install".to_string(),
        "CreatePluginInstance" => "plugin.instance.create".to_string(),
        "ConfigurePluginInstance" => "plugin.instance.update".to_string(),
        "EnablePluginInstance" => "plugin.instance.enable".to_string(),
        "DisablePluginInstance" => "plugin.instance.disable".to_string(),
        "UpgradePluginInstance" => "plugin.instance.upgrade".to_string(),
        "RollbackPluginInstance" => "plugin.instance.rollback".to_string(),
        "RemovePluginInstance" => "plugin.instance.remove".to_string(),
        "StorePluginCredentials" => "credential.store".to_string(),
        "StartPluginOAuth" | "PluginOAuthStatus" => "credential.store".to_string(),
        "DisconnectPluginOAuth" => "credential.revoke".to_string(),
        "RevokePluginCredentials" => "credential.revoke".to_string(),
        "RefreshPluginHealth" => "plugin.instance.health.refresh".to_string(),
        "RevokeEntitlementOverride" => "entitlement.revoke".to_string(),
        "RunBacktest" => "backtest.run.execute".to_string(),
        "CancelBacktest" | "RetryBacktest" | "ProcessBacktest" => {
            "backtest.run.execute".to_string()
        }
        "BacktestRuns" | "BacktestRun" | "ReplayBacktest" => "backtest.run.read".to_string(),
        "BacktestReportExport" => "backtest.report.export".to_string(),
        "CreateRobustnessRun"
        | "ProcessRobustnessRun"
        | "CancelRobustnessRun"
        | "RetryRobustnessRun"
        | "ReplayRobustnessRun"
        | "ExportRobustnessRun" => "research.robustness.run".to_string(),
        "ObserveStudioSetup" => "studio.setup.observe".to_string(),
        "RobustnessRuns" | "RobustnessRun" | "RobustnessReport" => {
            "research.robustness.read".to_string()
        }
        "CreateDerivativesAnalysis" => "research.derivatives.run".to_string(),
        "DerivativesAnalyses" | "DerivativesAnalysis" | "DerivativesAnalysisExport" => {
            "research.derivatives.read".to_string()
        }
        "CreateResearchComparison" => "research.comparison.run".to_string(),
        "ResearchComparisons" | "ResearchComparison" | "ResearchComparisonExport" => {
            "research.comparison.read".to_string()
        }
        "RunStrategyResearch" => "backtest.run.execute".to_string(),
        "CreateDatasetIngestion" => "research.dataset.create".to_string(),
        "DatasetIngestionList" => "dataset.ingestion.list".to_string(),
        "DatasetIngestionGet" | "DatasetIngestionStatus" => "dataset.ingestion.get".to_string(),
        "CancelDatasetIngestion" => "research.dataset.cancel".to_string(),
        "VerifyDatasetIngestion" => "dataset.ingestion.verify".to_string(),
        "RunScenarioValuation" => "research.scenario_valuation.run".to_string(),
        "RunMonteCarlo" => "research.monte_carlo.run".to_string(),
        "MonteCarloStatus" | "MonteCarloReport" => "research.monte_carlo.run".to_string(),
        "RunPortfolioRisk" | "PortfolioRiskReport" => "research.portfolio_risk.run".to_string(),
        "FillQualityInspect" | "FillQualityReport" | "FillQualityReplay" | "FillQualityExport" => {
            "research.fill_quality.analyze".to_string()
        }
        "AttributionJournalReview"
        | "AttributionJournalReport"
        | "AttributionJournalInspect"
        | "AttributionJournalReplay"
        | "AttributionJournalExport" => "research.attribution_journal.review".to_string(),
        "LifecycleCalendarList"
        | "LifecycleCalendarInspect"
        | "LifecycleCalendarExport"
        | "LifecycleCalendarReplay" => "research.lifecycle_calendar.build".to_string(),
        "ResearchNotebookCreate"
        | "ResearchNotebookCompose"
        | "ResearchNotebookAttach"
        | "ResearchNotebookList"
        | "ResearchNotebookInspect"
        | "ResearchNotebookExport"
        | "ResearchNotebookReplay" => "research.notebook.update".to_string(),
        "CreateResearchDataset" => "research.dataset.create".to_string(),
        "QueueStrategyResearchJob" => "research.job.create".to_string(),
        "StrategyResearchJobStatus" => "research.job.status".to_string(),
        "PromoteStrategyResearchToPaper" => "research.promote.paper".to_string(),
        "RunResearchSweep" => "research.sweep.create".to_string(),
        "CreateResearchUniverse" => "research.universe.create".to_string(),
        "UpdateProviderInstance" => "plugin.instance.update".to_string(),
        "EnableProviderInstance" => "plugin.instance.enable".to_string(),
        "DisableProviderInstance" => "plugin.instance.disable".to_string(),
        "AcknowledgeAlert" => "alert.acknowledge".to_string(),
        "CreateStrategyShareSnapshot" => "strategy.share.create".to_string(),
        "RevokeStrategyShareSnapshot" => "strategy.share.revoke".to_string(),
        "LocalWorkspace" => "workspace.get".to_string(),
        "StrategyLibrary" => "strategy.list".to_string(),
        other => format!("graphql.{other}"),
    }
}

fn graphql_operation_route(operation: &str) -> &str {
    match operation {
        "CreateStrategy" => "/product/strategies/create",
        "CreateStrategyAiDraft" => "/product/strategies/ai-draft",
        "SaveBuilderDraft" => "/product/strategies/save-draft",
        "SelectStrategySemanticNode" => "/product/strategies/semantic-selection",
        "ProposeStrategyPatch" => "/product/strategies/proposals/create",
        "ReviewStrategyPatch" => "/product/strategies/proposals/review",
        "ApplyStrategyPatch" => "/product/strategies/proposals/apply",
        "ValidateBuilderDraft" => "/product/strategies/validate-draft",
        "ValidateStrategyExpression" => "/product/strategies/validate-expression",
        "CompileStrategyPreview" => "/strategy/contracts/envelope-preview",
        "PublishStrategy" => "/product/strategies/publish",
        "DuplicateStrategy" => "/product/strategies/duplicate",
        "ArchiveStrategy" => "/product/strategies/archive",
        "RestoreStrategy" => "/product/strategies/restore",
        "RunStrategyOnce" => "/product/run-center/run-once",
        "SchedulerRun" => "/scheduler/run",
        "SchedulerStart" => "/scheduler/start",
        "SchedulerStop" => "/scheduler/stop",
        "ReconcileOrders" => "/orders/reconcile",
        "StoreCredentials" => "/providers/{provider_ref}/credentials",
        "TestCredentials" => "/providers/{provider_ref}/credentials/test",
        "InstallPluginManifest" => "/plugins/manifests",
        "InstallPluginPackage" => "/plugins/packages",
        "CreatePluginInstance" => "/plugins/instances",
        "ConfigurePluginInstance" => "/plugins/instances/{instance_ref}/configuration",
        "EnablePluginInstance" => "/plugins/instances/{instance_ref}:enable",
        "DisablePluginInstance" => "/plugins/instances/{instance_ref}:disable",
        "UpgradePluginInstance" => "/plugins/instances/{instance_ref}:upgrade",
        "RollbackPluginInstance" => "/plugins/instances/{instance_ref}:rollback",
        "RemovePluginInstance" => "/plugins/instances/{instance_ref}",
        "StorePluginCredentials" => "/plugins/instances/{instance_ref}/credentials",
        "StartPluginOAuth" => "/plugins/oauth/start",
        "PluginOAuthStatus" => "/plugins/oauth/status",
        "DisconnectPluginOAuth" => "/plugins/oauth/disconnect",
        "RevokePluginCredentials" => "/plugins/instances/{instance_ref}/credentials",
        "RefreshPluginHealth" => "/plugins/instances/{instance_ref}/health:refresh",
        "RevokeEntitlementOverride" => "/entitlements/{grant_id}",
        "SightlineSessionState" => "/product/sightline/session-state",
        "PublishSightlineStrategyEditorSurface" => "/product/sightline/strategy-editor/publish",
        "PublishSightlineStudioSurface" => "/product/sightline/studio-surface/publish",
        "SetSightlineSelection" => "/product/sightline/selections/set",
        "ShareSightlineSelection" => "/product/sightline/selections/share",
        "SightlineGetContext" => "/product/sightline/context/get",
        "ConnectSightlineAgent" => "/product/sightline/agents/connect",
        "SightlineUpdatePresence" => "/product/sightline/presence/update",
        "SightlineCreateProposal" => "/product/sightline/proposals/create",
        "SightlineRequestApproval" => "/product/sightline/approvals/request",
        "SightlineNavigateSurface" => "/product/sightline/navigation/request",
        "SightlineFocusNode" => "/product/sightline/nodes/focus",
        "SightlineHighlightNode" => "/product/sightline/nodes/highlight",
        "SightlineEvents" => "/product/sightline/events",
        "RunBacktest" => "/product/strategies/backtests/run",
        "BacktestRuns" => "/backtests",
        "BacktestRun" => "/backtests/{run_id}",
        "CancelBacktest" => "/backtests/{run_id}/cancel",
        "RetryBacktest" => "/backtests/{run_id}/retry",
        "ProcessBacktest" => "/backtests:process",
        "ReplayBacktest" => "/backtests/{run_id}/replay",
        "BacktestReportExport" => "/product/strategies/backtests/export",
        "CreateRobustnessRun" => "/robustness-runs",
        "RobustnessRuns" => "/robustness-runs",
        "RobustnessRun" => "/robustness-runs/{run_id}",
        "RobustnessReport" => "/robustness-runs/{run_id}/report",
        "ProcessRobustnessRun" => "/robustness-runs:process",
        "CancelRobustnessRun" => "/robustness-runs/{run_id}/cancel",
        "RetryRobustnessRun" => "/robustness-runs/{run_id}/retry",
        "ReplayRobustnessRun" => "/robustness-runs/{run_id}/replay",
        "ExportRobustnessRun" => "/robustness-runs/{run_id}/export",
        "ObserveStudioSetup" => "/studio/setup/observe",
        "CreateDerivativesAnalysis" => "/derivatives-analyses",
        "DerivativesAnalyses" => "/derivatives-analyses",
        "DerivativesAnalysis" => "/derivatives-analyses/{analysis_id}",
        "DerivativesAnalysisExport" => "/derivatives-analyses/{analysis_id}/exports/{format}",
        "CreateResearchComparison" => "/research-comparisons",
        "ResearchComparisons" => "/research-comparisons",
        "ResearchComparison" => "/research-comparisons/{comparison_id}",
        "ResearchComparisonExport" => "/research-comparisons/{comparison_id}/exports/{format}",
        "RunStrategyResearch" => "/product/strategies/research-runs/create",
        "CreateDatasetIngestion" => "/dataset-ingestions",
        "DatasetIngestionList" => "/dataset-ingestions",
        "DatasetIngestionGet" | "DatasetIngestionStatus" => "/dataset-ingestions/{ingestion_id}",
        "CancelDatasetIngestion" => "/dataset-ingestions/{ingestion_id}/cancel",
        "VerifyDatasetIngestion" => "/dataset-ingestions/{ingestion_id}/verify",
        "RunScenarioValuation" => "/product/strategies/scenario-valuation",
        "RunMonteCarlo" => "/product/strategies/monte-carlo",
        "MonteCarloStatus" => "/product/strategies/monte-carlo/status",
        "MonteCarloReport" => "/product/strategies/monte-carlo/report",
        "RunPortfolioRisk" => "/product/strategies/portfolio-risk",
        "PortfolioRiskReport" => "/product/strategies/portfolio-risk/report",
        "FillQualityInspect" => "/product/strategies/fill-quality/inspect",
        "FillQualityReport" => "/product/strategies/fill-quality/report",
        "FillQualityReplay" => "/product/strategies/fill-quality/replay",
        "FillQualityExport" => "/product/strategies/fill-quality/export",
        "AttributionJournalReview" => "/product/strategies/attribution-journal/review",
        "AttributionJournalReport" => "/product/strategies/attribution-journal/report",
        "AttributionJournalInspect" => "/product/strategies/attribution-journal/inspect",
        "AttributionJournalReplay" => "/product/strategies/attribution-journal/replay",
        "AttributionJournalExport" => "/product/strategies/attribution-journal/export",
        "LifecycleCalendarList" => "/product/strategies/lifecycle-calendar/list",
        "LifecycleCalendarInspect" => "/product/strategies/lifecycle-calendar/inspect",
        "LifecycleCalendarExport" => "/product/strategies/lifecycle-calendar/export",
        "LifecycleCalendarReplay" => "/product/strategies/lifecycle-calendar/replay",
        "ResearchNotebookCreate" => "/product/strategies/research-notebook/create",
        "ResearchNotebookCompose" => "/product/strategies/research-notebook/compose",
        "ResearchNotebookAttach" => "/product/strategies/research-notebook/attach",
        "ResearchNotebookList" => "/product/strategies/research-notebook/list",
        "ResearchNotebookInspect" => "/product/strategies/research-notebook/inspect",
        "ResearchNotebookExport" => "/product/strategies/research-notebook/export",
        "ResearchNotebookReplay" => "/product/strategies/research-notebook/replay",
        "CreateResearchDataset" => "/product/strategies/research/datasets/create",
        "ResolveCapabilityGraph" => "/capabilities/graph/resolve",
        "SaveCapabilityGraphRevision" => "/capability-graph-revisions",
        "CapabilityGraphRevision" => "/capability-graph-revisions/{revision_id}",
        "CheckCapabilityGraphRevision" => "/capability-graph-revisions/currentness",
        "QueueStrategyResearchJob" => "/product/strategies/research/jobs/create",
        "StrategyResearchJobStatus" => "/product/strategies/research/jobs/status",
        "PromoteStrategyResearchToPaper" => "/product/strategies/research/promote-to-paper",
        "RunResearchSweep" => "/product/strategies/research-sweeps/create",
        "CreateResearchUniverse" => "/research/universes",
        "SaveExecutionConfig" => "/product/strategy-execution-configs/save",
        "ActivationReadiness" => "/product/strategy-execution-configs/activation-readiness",
        "ActivateExecution" => "/product/strategy-execution-activations/activate",
        "ControlStrategyExecution" => "/product/strategy-execution-activations/control",
        "DeactivateStrategyExecution" => "/product/strategy-execution-activations/deactivate",
        "UpdateProviderInstance" => "/product/providers/update-instance",
        "EnableProviderInstance" => "/product/providers/enable-instance",
        "DisableProviderInstance" => "/product/providers/disable-instance",
        "AcknowledgeAlert" => "/product/alerts/acknowledge",
        "CreateStrategyShareSnapshot" => "/product/strategies/share-snapshots/create",
        "RevokeStrategyShareSnapshot" => "/product/strategies/share-snapshots/revoke",
        _ => "/graphql",
    }
}

fn mcp_is_read(name: &str) -> bool {
    // Emits owner-scoped JSON, without publishing or writing an artifact.
    if name == "tradeassembly.journal.export" {
        return true;
    }
    // Polling OAuth can redeem the handoff and install credentials.
    if name.ends_with(".export") || name == "tradeassembly.plugin.oauth.status" {
        return false;
    }
    if matches!(
        name,
        "tradeassembly.sightline.session_state"
            | "tradeassembly.sightline.get_current_selection"
            | "tradeassembly.sightline.get_context"
            | "tradeassembly.sightline.list_events"
            | "tradeassembly.plugin.capability_resolve"
            | "tradeassembly.plugin.capability_graph_resolve"
            | "tradeassembly.plugin.capability_revision_get"
            | "tradeassembly.plugin.capability_revision_check"
            | "tradeassembly.plugin.capability_matrix"
            | "tradeassembly.provider.capability_matrix"
            | "tradeassembly.provider.pack_compatibility"
            | "tradeassembly.dataset_ingestion.verify"
            | "studio.execution.external_receipt.inspect"
            | "studio.deployment.inspect"
            | "studio.agent_run.inspect"
            | "studio.agent_run.events"
            | "studio.agent_run.health"
    ) {
        return true;
    }
    name.ends_with(".list")
        || name.ends_with(".get")
        || name.ends_with(".status")
        || name.ends_with(".inspect")
        || name.ends_with(".validate")
        || name.ends_with(".resolve")
        || name.ends_with(".evaluate")
        || name.ends_with(".valuation")
        || name.ends_with(".explain")
        || name.ends_with(".report")
        || name.ends_with(".replay")
        || name == "tradeassembly.health"
        || name == "tradeassembly.setup.status"
        || name == "tradeassembly.studio.link"
}

fn classify_side_effect(method: &str, path: &str, body: &Value) -> String {
    if is_read_http_route(method, path) {
        return "read".to_string();
    }
    if path == "/product/sightline/context/get" {
        return "read".to_string();
    }
    if path.starts_with("/product/sightline/navigation")
        || path.starts_with("/product/sightline/nodes/")
        || path.starts_with("/product/sightline/surfaces/")
        || path.starts_with("/product/sightline/studio-surface/")
        || path.starts_with("/product/sightline/selections/")
        || path.starts_with("/product/sightline/agents/")
        || path.starts_with("/product/sightline/presence/")
    {
        return "session".to_string();
    }
    if path.starts_with("/product/sightline/proposals") {
        return "draft_mutation".to_string();
    }
    if path.starts_with("/product/sightline/approvals") {
        return "approval".to_string();
    }
    if path.contains("credentials") || path.contains("oauth") {
        return "credential".to_string();
    }
    if path.contains("strategy-execution-activations/activate")
        || path.contains("strategy-execution-activations/control")
        || path.contains("strategy-execution-activations/deactivate")
    {
        return if account_mode(body) == "live" {
            "live_order".to_string()
        } else {
            "paper".to_string()
        };
    }
    if path.contains("strategy-execution-configs") {
        return "mutation".to_string();
    }
    if path.contains("backtest")
        || path.contains("research")
        || path.contains("dataset-ingestion")
        || path.contains("monte-carlo")
        || path.contains("portfolio-risk")
    {
        return "research".to_string();
    }
    if path.contains("order") || path.contains("run-center") || path == "/run" {
        if nested_value(body, "submitOrders")
            .or_else(|| nested_value(body, "submit_orders"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return if account_mode(body) == "live" {
                "live_order".to_string()
            } else {
                "paper_order".to_string()
            };
        }
        return "paper".to_string();
    }
    if path.contains("/strategies/proposals/create") {
        return "draft_proposal".to_string();
    }
    if path.contains("/strategies/proposals/review") || path.contains("/strategies/proposals/apply")
    {
        return "draft_proposal_review".to_string();
    }
    if path.contains("/strategies/publish") {
        return "draft_publish".to_string();
    }
    if path.contains("strategy") || path.contains("strategies") {
        return "draft_mutation".to_string();
    }
    if path.contains("scheduler") || path.contains("worker") {
        return "scheduler".to_string();
    }
    "mutation".to_string()
}

fn is_read_http_route(method: &str, path: &str) -> bool {
    if method == "GET" {
        return true;
    }
    if method != "POST" {
        return false;
    }
    if path.starts_with("/dataset-ingestions/") && path.ends_with("/verify") {
        return true;
    }
    if (path.starts_with("/backtests/") && path.ends_with("/replay"))
        || (path.starts_with("/execution/position-lifecycle/")
            && matches!(
                path.rsplit('/').next(),
                Some("inspect" | "status" | "replay")
            ))
    {
        return true;
    }
    matches!(
        path,
        "/capabilities/resolve"
            | "/product/live-mandates/status"
            | "/capabilities/graph/resolve"
            | "/capability-graph-revisions/currentness"
            | "/journal/replay"
            | "/journal/replay-harness"
            | "/journal/replay-report"
            | "/marketdata/conformance"
            | "/marketdata/indicators"
            | "/marketdata/instrument-packs/validate"
            | "/marketdata/instruments/resolve"
            | "/marketdata/options/select"
            | "/marketdata/selectors/evaluate"
            | "/marketdata/selectors/explain"
            | "/orders/status-stream"
            | "/orders/broker-recovery"
            | "/orders/status-stream/watch"
            | "/plugins/capabilities/resolve"
            | "/product/share-snapshots/view"
            | "/product/strategies/validate-draft"
            | "/product/strategies/validate-expression"
            | "/providers/broker-conformance"
            | "/strategy/contracts/capability:resolve"
            | "/strategy/contracts/compatibility:resolve"
            | "/strategy/contracts/compile-preview"
            | "/strategy/contracts/envelope-preview"
            | "/strategy/contracts/validate"
            | "/product/viewer"
            | "/product/strategies/home"
            | "/product/strategies/get"
            | "/product/strategies/instrument-context"
            | "/product/strategies/version-history"
            | "/product/strategies/builder-state"
            | "/product/strategies/research-workspace"
            | "/product/strategies/execution-workspace"
            | "/product/strategy-execution-configs/activation-readiness"
            | "/product/run-center"
            | "/product/research/rollup"
            | "/product/alerts/center"
            | "/product/discover-workspace"
            | "/product/strategies/share-workspace"
            | "/product/connect-workspace"
            | "/product/plugins/providers"
            | "/product/providers/list"
            | "/plugins/instances"
            | "/plugins/manifests"
    )
}

fn command_group(command_name: &str) -> &str {
    command_name.split('.').next().unwrap_or("unknown")
}

fn target_object(path: &str, body: &Value) -> Option<String> {
    if path.starts_with("/plugins/oauth/") {
        if let Some(instance) = nested_value(body, "instanceRef")
            .or_else(|| nested_value(body, "instance_ref"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return Some(instance.to_string());
        }
    }
    for key in [
        "strategyId",
        "strategy_id",
        "activationId",
        "activation_id",
        "providerRef",
        "provider_ref",
        "ref",
        "orderId",
        "order_id",
        "backtestId",
        "backtest_id",
        "deploymentId",
        "deployment_id",
        "runId",
        "run_id",
    ] {
        if let Some(value) = nested_value(body, key).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                return Some(value.to_string());
            }
        }
    }
    if path.starts_with("/providers/") {
        return path
            .trim_matches('/')
            .split('/')
            .nth(1)
            .map(ToOwned::to_owned);
    }
    if path.starts_with("/strategies/") {
        return path
            .trim_matches('/')
            .split('/')
            .nth(1)
            .map(ToOwned::to_owned);
    }
    None
}

fn authority_from_body(source_interface: &str, body: &Value) -> AuthorityContext {
    let context =
        nested_value(body, "authorityContext").or_else(|| nested_value(body, "authority_context"));
    let actor = context
        .and_then(|value| value.get("actor").or_else(|| value.get("principalUser")))
        .and_then(Value::as_str)
        .unwrap_or("local-user");
    let surface = context
        .and_then(|value| value.get("surface"))
        .and_then(Value::as_str)
        .unwrap_or(source_interface);
    AuthorityContext {
        actor: actor.to_string(),
        surface: surface.to_string(),
        account_mode: account_mode(body),
    }
}

fn account_mode(body: &Value) -> String {
    nested_value(body, "accountMode")
        .or_else(|| nested_value(body, "account_mode"))
        .and_then(Value::as_str)
        .unwrap_or("paper")
        .to_string()
}

fn idempotency_key_from_body(
    source_interface: &str,
    command_name: &str,
    side_effect_class: &str,
    body: &Value,
    payload_hash: &str,
) -> Result<IdempotencyKey, String> {
    let explicit = body
        .get("idempotencyKey")
        .or_else(|| nested_value(body, "idempotencyKey"))
        .or_else(|| nested_value(body, "idempotency_key"))
        .or_else(|| nested_value(body, "commandId"))
        .or_else(|| nested_value(body, "command_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            let digest = &payload_hash[..16.min(payload_hash.len())];
            if side_effect_class == "read" {
                format!("auto:{source_interface}:{command_name}:{digest}")
            } else if implicit_rerunnable_command(command_name) {
                format!(
                    "auto:{source_interface}:{command_name}:{digest}:{}",
                    auto_mutation_nonce()
                )
            } else {
                format!("auto:{source_interface}:{command_name}:{digest}")
            }
        });
    IdempotencyKey::new(explicit)
}

fn implicit_rerunnable_command(command_name: &str) -> bool {
    matches!(
        command_name,
        "credential.test" | "orders.reconcile" | "plugin.instance.health.refresh"
    )
}

fn auto_mutation_nonce() -> u64 {
    static NEXT_AUTO_MUTATION_NONCE: AtomicU64 = AtomicU64::new(1);
    NEXT_AUTO_MUTATION_NONCE.fetch_add(1, Ordering::Relaxed)
}

fn command_id(source_interface: &str, command_name: &str, idempotency_key: &str) -> String {
    format!(
        "cmd_{}",
        hash_bytes(format!("{source_interface}:{command_name}:{idempotency_key}").as_bytes())
    )
}

fn expected_sequence(body: &Value) -> Option<u64> {
    nested_value(body, "expectedSequence")
        .or_else(|| nested_value(body, "expected_sequence"))
        .or_else(|| nested_value(body, "expectedVersion"))
        .or_else(|| nested_value(body, "expected_version"))
        .and_then(Value::as_u64)
}

fn evidence_refs(body: &Value) -> Vec<String> {
    nested_value(body, "evidenceRefs")
        .or_else(|| nested_value(body, "evidence_refs"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.trim().is_empty())
                .map(|item| {
                    if is_sensitive_text(item) {
                        "[REDACTED]".to_string()
                    } else {
                        item.to_string()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn nested_value<'a>(body: &'a Value, key: &str) -> Option<&'a Value> {
    body.get(key).or_else(|| {
        body.get("variables").and_then(|variables| {
            variables.get(key).or_else(|| {
                variables
                    .get("request")
                    .and_then(|request| request.get(key))
            })
        })
    })
}

fn validate_expected_sequence(
    storage: &dyn StoragePort,
    envelope: &ControlPlaneCommandEnvelope,
) -> Result<(), String> {
    let Some(expected) = envelope.expected_sequence else {
        return Ok(());
    };
    let stored_current = envelope
        .target_object
        .as_ref()
        .and_then(|target| storage.get_json(SEQUENCE_NS, target).ok().flatten())
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.get("sequence").and_then(Value::as_u64))
                .or_else(|| value.get("version").and_then(Value::as_u64))
        });
    match stored_current {
        Some(current) if current == expected => Ok(()),
        Some(_) => Err("stale_sequence".to_string()),
        None => Err("stale_sequence_unavailable".to_string()),
    }
}

fn graphql_operation_name(request: &Value) -> String {
    if let Some(operation) = request.get("operationName").and_then(Value::as_str) {
        if !operation.trim().is_empty() {
            return operation.to_string();
        }
    }
    let query = request.get("query").and_then(Value::as_str).unwrap_or("");
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|window| {
            if matches!(window[0], "query" | "mutation") {
                Some(window[1].trim_matches('{').to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn graphql_effective_payload(request: &Value, operation: &str) -> Value {
    let variables = request
        .get("variables")
        .cloned()
        .unwrap_or_else(|| request.clone());
    json!({
        "_graphqlOperation": operation,
        "variables": variables
    })
}

fn graphql_is_mutation(request: &Value) -> bool {
    request
        .get("query")
        .and_then(Value::as_str)
        .map(|query| query.trim_start().starts_with("mutation"))
        .unwrap_or(false)
}

fn redact_sensitive_payload(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if is_sensitive_key(&key) {
                        (key, Value::String("[REDACTED]".to_string()))
                    } else {
                        (key, redact_sensitive_payload(value))
                    }
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(redact_sensitive_payload).collect())
        }
        Value::String(text) if is_sensitive_text(&text) => Value::String("[REDACTED]".to_string()),
        other => other,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.replace('-', "_").to_ascii_lowercase();
    let compact = normalized.replace('_', "");
    SENSITIVE_KEYS.contains(&normalized.as_str())
        || normalized.ends_with("_token")
        || normalized.ends_with("_secret")
        || compact.contains("token")
        || compact.contains("secret")
        || compact == "apikey"
        || compact == "privatekey"
        || compact == "authorizationurl"
        || compact == "handoffverifier"
        || compact == "verifier"
}

fn is_sensitive_text(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("api_secret")
        || normalized.contains("apisecret")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("access_key")
        || normalized.contains("private_key")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("bearer ")
}

fn hash_json(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn validate_key_part(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} is required"));
    }
    if value.contains("..") || value.contains('/') || value.contains('\\') {
        return Err(format!("{label} contains unsafe characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::local::sqlite::LocalSqliteStorage;
    use crate::finance_authority::TestFinanceAuthority;
    use serde_json::json;
    use tempfile::tempdir;

    fn plugin_package_body(locator: &str, offline: bool) -> Value {
        json!({
            "idempotencyKey": "plugin-package:package-digest:manifest-digest",
            "source": {
                "type": "package",
                "locator": locator,
                "package": {
                    "type": if locator.starts_with("https://") { "https" } else { "file" },
                    "locator": locator,
                },
            },
            "integrity": {
                "packageSha256": "package-digest",
                "manifestSha256": "manifest-digest",
            },
            "offline": offline,
        })
    }

    #[test]
    fn oauth_handoff_material_is_not_journaled() {
        let redacted = redact_sensitive_payload(json!({
            "authorizationUrl": "https://provider.test/authorize?state=private-state",
            "verifier": "private-proof",
            "accessToken": "private-token",
            "connectionId": "opaque-id",
            "mode": "live"
        }));
        assert_eq!(redacted["authorizationUrl"], "[REDACTED]");
        assert_eq!(redacted["verifier"], "[REDACTED]");
        assert_eq!(redacted["accessToken"], "[REDACTED]");
        assert_eq!(redacted["mode"], "live");
    }

    #[test]
    fn plugin_package_replay_uses_immutable_identity_across_acquisition_changes() {
        let directory = tempdir().expect("temporary storage");
        let database = directory.path().join("tradeassembly.db");
        let storage = LocalSqliteStorage::new(database.to_string_lossy());
        let authority = TestFinanceAuthority;
        let first = http_envelope(
            "cli",
            "POST",
            "/plugins/packages",
            &plugin_package_body("/tmp/tradeassembly-plugin.tar.gz", true),
        )
        .expect("first envelope");
        let second = http_envelope(
            "cli",
            "POST",
            "/plugins/packages",
            &plugin_package_body("https://example.invalid/tradeassembly-plugin.tar.gz", false),
        )
        .expect("second envelope");

        assert_eq!(first.command_id, second.command_id);
        assert_eq!(first.payload_hash, second.payload_hash);
        prepare_command(&storage, &authority, first.clone()).expect("prepare first");
        complete_command_with_body(&storage, &authority, &first, 200, Some(json!({"ok": true})))
            .expect("complete first");

        let replay = prepare_command(&storage, &authority, second).expect("prepare replay");
        assert!(replay.duplicate);
        assert_eq!(replay.response_status, Some(200));
        assert_eq!(replay.response_body, Some(json!({"ok": true})));
    }

    #[test]
    fn redacts_sensitive_payload_before_hash_preview_storage() {
        let envelope = http_envelope(
            "http",
            "POST",
            "/providers/alpaca-paper/credentials",
            &json!({"api_secret": "SHOULD_NOT_LEAK", "providerRef": "alpaca-paper"}),
        )
        .expect("envelope");

        assert_eq!(envelope.command_name, "credential.store");
        assert_eq!(envelope.payload_preview["api_secret"], "[REDACTED]");
        assert!(!serde_json::to_string(&envelope)
            .unwrap()
            .contains("SHOULD_NOT_LEAK"));
    }

    #[test]
    fn new_commands_root_correlation_at_their_trusted_command_id() {
        let envelope = http_envelope(
            "http",
            "POST",
            "/product/strategies/create",
            &json!({
                "strategyId": "strategy-correlation",
                "idempotencyKey": "strategy-correlation-create",
                "correlationId": "caller-forged",
                "controlPlaneCorrelationId": "caller-forged-internal"
            }),
        )
        .expect("envelope");

        assert_eq!(envelope.correlation_id, envelope.command_id);
        assert_eq!(envelope.effective_correlation_id(), envelope.command_id);
        assert_ne!(envelope.correlation_id, "caller-forged");
        assert_ne!(envelope.correlation_id, "caller-forged-internal");
    }

    #[test]
    fn historical_commands_without_correlation_use_their_command_id() {
        let envelope = http_envelope(
            "http",
            "POST",
            "/product/strategies/create",
            &json!({
                "strategyId": "strategy-legacy-correlation",
                "idempotencyKey": "strategy-legacy-correlation-create"
            }),
        )
        .expect("envelope");
        let mut stored = serde_json::to_value(&envelope).expect("serialize envelope");
        stored
            .as_object_mut()
            .expect("envelope object")
            .remove("correlation_id");

        let restored: ControlPlaneCommandEnvelope =
            serde_json::from_value(stored).expect("deserialize historical envelope");
        assert!(restored.correlation_id.is_empty());
        assert_eq!(restored.effective_correlation_id(), restored.command_id);
    }

    #[test]
    fn maps_common_surfaces_to_canonical_commands() {
        let http = http_envelope(
            "cli",
            "POST",
            "/product/strategies/create",
            &json!({"strategyId": "strat_local_btc_demo"}),
        )
        .expect("http envelope");
        let mcp = mcp_envelope(
            "tradeassembly.strategy.create",
            &json!({"strategy_id": "strat_local_btc_demo"}),
        )
        .expect("mcp envelope");

        assert_eq!(http.command_name, "strategy.create");
        assert_eq!(mcp.command_name, "strategy.create");
        assert_eq!(http.command_group, "strategy");
        assert_eq!(mcp.target_object.as_deref(), Some("strat_local_btc_demo"));
    }

    #[test]
    fn oauth_mcp_uses_the_same_credential_authority_as_http() {
        for (action, command, authority_action) in [
            ("start", "credential.store", "credential.configure"),
            ("status", "credential.store", "credential.configure"),
            ("disconnect", "credential.revoke", "credential.revoke"),
        ] {
            let arguments = json!({"instanceRef": "alpaca-paper"});
            let http = http_envelope(
                "http",
                "POST",
                &format!("/plugins/oauth/{action}"),
                &arguments,
            )
            .expect("http envelope");
            let mcp = mcp_envelope(&format!("tradeassembly.plugin.oauth.{action}"), &arguments)
                .expect("mcp envelope");
            assert_eq!(mcp.command_name, command);
            assert_eq!(mcp.command_name, http.command_name);
            assert_eq!(mcp.side_effect_class, "credential");
            assert_eq!(mcp.side_effect_class, http.side_effect_class);
            assert_eq!(mcp.target_object.as_deref(), Some("alpaca-paper"));
            assert_eq!(mcp.target_object, http.target_object);
            let definition = crate::finance_authority::authority_definition(&mcp)
                .expect("registered credential authority");
            assert_eq!(definition.action, authority_action);
        }
    }

    #[test]
    fn maps_report_producing_research_posts_to_registered_commands() {
        for (path, expected) in [
            (
                "/product/strategies/scenario-valuation",
                "research.scenario_valuation.run",
            ),
            (
                "/product/strategies/monte-carlo/report",
                "research.monte_carlo.run",
            ),
            (
                "/product/strategies/portfolio-risk/export",
                "research.portfolio_risk.run",
            ),
            (
                "/product/strategies/fill-quality/inspect",
                "research.fill_quality.analyze",
            ),
            (
                "/product/strategies/lifecycle-calendar/list",
                "research.lifecycle_calendar.build",
            ),
            (
                "/product/strategies/attribution-journal/review",
                "research.attribution_journal.review",
            ),
            (
                "/product/strategies/research-notebook/compose",
                "research.notebook.update",
            ),
        ] {
            let envelope = http_envelope(
                "cli",
                "POST",
                path,
                &json!({"strategyId": "strat_local_btc_demo"}),
            )
            .expect("research envelope");
            assert_eq!(envelope.command_name, expected, "{path}");
            assert_ne!(envelope.side_effect_class, "read", "{path}");
        }
    }

    #[test]
    fn maps_every_mcp_export_to_a_protected_canonical_action() {
        for (name, expected) in [
            ("tradeassembly.backtest.export", "backtest.report.export"),
            (
                "tradeassembly.monte_carlo.export",
                "research.monte_carlo.run",
            ),
            (
                "tradeassembly.portfolio_risk.export",
                "research.portfolio_risk.run",
            ),
            (
                "tradeassembly.fill_quality.export",
                "research.fill_quality.analyze",
            ),
            (
                "tradeassembly.attribution_journal.export",
                "research.attribution_journal.review",
            ),
            (
                "tradeassembly.lifecycle_calendar.export",
                "research.lifecycle_calendar.build",
            ),
            (
                "tradeassembly.research_notebook.export",
                "research.notebook.update",
            ),
        ] {
            let envelope = mcp_envelope(
                name,
                &json!({
                    "strategy_id": "strat_local_btc_demo",
                    "idempotencyKey": format!("export-{name}")
                }),
            )
            .expect("export envelope");
            assert_eq!(envelope.command_name, expected, "{name}");
            assert_ne!(envelope.side_effect_class, "read", "{name}");
        }
    }

    #[test]
    fn research_http_alias_uses_durable_backtest_authority() {
        let body = json!({"idempotencyKey": "explicit-research-request"});
        let canonical = http_envelope("cli", "POST", "/backtests", &body).unwrap();
        let alias = http_envelope(
            "cli",
            "POST",
            "/product/strategies/research-runs/create",
            &body,
        )
        .unwrap();
        assert_eq!(alias.command_name, canonical.command_name);
        assert_eq!(alias.command_name, "backtest.run.execute");
        assert_eq!(alias.side_effect_class, canonical.side_effect_class);
        assert_ne!(alias.side_effect_class, "read");
    }

    #[test]
    fn graphql_aliases_keep_canonical_command_but_distinct_auto_idempotency() {
        let variables = json!({
            "strategyId": "strat_local_btc_demo",
            "datasetId": "demo",
            "_graphqlOperation": "caller-spoofed-operation"
        });
        let backtest = graphql_envelope(&json!({
            "operationName": "RunBacktest",
            "query": "mutation RunBacktest { runBacktest }",
            "variables": variables.clone()
        }))
        .expect("backtest envelope");
        let research = graphql_envelope(&json!({
            "operationName": "RunStrategyResearch",
            "query": "mutation RunStrategyResearch { runStrategyResearch }",
            "variables": variables
        }))
        .expect("strategy research envelope");

        assert_eq!(backtest.command_name, "backtest.run.execute");
        assert_eq!(research.command_name, "backtest.run.execute");
        assert_ne!(backtest.command_id, research.command_id);
        assert_eq!(backtest.payload_preview["_graphqlOperation"], "RunBacktest");
        assert_eq!(
            research.payload_preview["_graphqlOperation"],
            "RunStrategyResearch"
        );
        assert_eq!(
            research.payload_preview["variables"]["_graphqlOperation"],
            "caller-spoofed-operation"
        );

        let null_backtest = graphql_envelope(&json!({
            "operationName": "RunBacktest",
            "query": "mutation RunBacktest { runBacktest }",
            "variables": null
        }))
        .expect("null backtest envelope");
        let null_research = graphql_envelope(&json!({
            "operationName": "RunStrategyResearch",
            "query": "mutation RunStrategyResearch { runStrategyResearch }",
            "variables": null
        }))
        .expect("null strategy research envelope");
        assert_eq!(null_backtest.command_name, "backtest.run.execute");
        assert_eq!(null_research.command_name, "backtest.run.execute");
        assert_ne!(null_backtest.command_id, null_research.command_id);
        assert_eq!(
            null_backtest.payload_preview["_graphqlOperation"],
            "RunBacktest"
        );
        assert_eq!(
            null_research.payload_preview["_graphqlOperation"],
            "RunStrategyResearch"
        );
        assert!(null_research.payload_preview["variables"].is_null());
    }

    #[test]
    fn implicit_keys_are_retry_safe_by_default_and_unique_only_for_rerunnable_commands() {
        let first_rerunnable = http_envelope("http", "POST", "/orders/reconcile", &json!({}))
            .expect("first rerunnable envelope");
        let second_rerunnable = http_envelope("http", "POST", "/orders/reconcile", &json!({}))
            .expect("second rerunnable envelope");
        assert_ne!(first_rerunnable.command_id, second_rerunnable.command_id);
        assert_ne!(
            first_rerunnable.idempotency_key.as_str(),
            second_rerunnable.idempotency_key.as_str()
        );
        let first_credential_test = http_envelope(
            "http",
            "POST",
            "/providers/alpaca-paper/credentials/test",
            &json!({}),
        )
        .expect("first credential test envelope");
        let second_credential_test = http_envelope(
            "http",
            "POST",
            "/providers/alpaca-paper/credentials/test",
            &json!({}),
        )
        .expect("second credential test envelope");
        assert_ne!(
            first_credential_test.command_id,
            second_credential_test.command_id
        );
        let first_health_refresh = http_envelope(
            "http",
            "POST",
            "/plugins/instances/example/health:refresh",
            &json!({}),
        )
        .expect("first health refresh envelope");
        let second_health_refresh = http_envelope(
            "http",
            "POST",
            "/plugins/instances/example/health:refresh",
            &json!({}),
        )
        .expect("second health refresh envelope");
        assert_ne!(
            first_health_refresh.command_id,
            second_health_refresh.command_id
        );

        let first_mutation = http_envelope(
            "http",
            "POST",
            "/product/strategies/create",
            &json!({"strategyId": "strat_local_btc_demo"}),
        )
        .expect("first mutation envelope");
        let second_mutation = http_envelope(
            "http",
            "POST",
            "/product/strategies/create",
            &json!({"strategyId": "strat_local_btc_demo"}),
        )
        .expect("second mutation envelope");
        assert_eq!(first_mutation.command_id, second_mutation.command_id);
        assert_eq!(
            first_mutation.idempotency_key.as_str(),
            second_mutation.idempotency_key.as_str()
        );

        let first_read =
            http_envelope("http", "GET", "/orders", &json!({})).expect("first read envelope");
        let second_read =
            http_envelope("http", "GET", "/orders", &json!({})).expect("second read envelope");
        assert_eq!(first_read.command_id, second_read.command_id);
        assert_eq!(
            first_read.idempotency_key.as_str(),
            second_read.idempotency_key.as_str()
        );
    }
}
