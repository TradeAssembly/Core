//! Portable state validation and policy admission for broker submission.
//!
//! The loader returns a snapshot, not permission. The concrete boundary adds
//! durable intent, trusted-price risk checks and C5 authorization; dispatch
//! remains the responsibility of the duplicate-safe plugin operation adapter.

use crate::capability::local_full_entitlements;
use crate::live_execution_checks::apply_current_credential_status;
use crate::live_execution_checks::live_account_observation_matches;
use crate::local_live_authorization::{
    binding_from_observed_state, check_local_live_mandate, check_persisted_run_authority,
    deployment_actor,
};
use crate::local_owner_identity::LocalOwnerIdentity;
use crate::plugin_catalog::stored_plugin_entry;
use crate::ports::{
    CapabilityRequirement, CapabilityResolverCatalog, CapabilityResolverPort, ClockPort,
    CredentialPort, LeaseClaim, LeaseRepository, PluginOperationRequest, PluginPackagePort,
    PluginRegistryPort, SideEffectContext, StoragePort,
};
use serde_json::Value;
use std::sync::Arc;

mod admission;
mod observation;
pub(crate) use observation::validate_order_identity;
mod recovery;
pub use recovery::BrokerOrderRecoveryPlan;
pub(crate) use recovery::{prepare_plan as prepare_recovery_plan, RecoveryObserver};
mod prices;
mod risk;
pub use admission::LocalBrokerSubmissionBoundary;
pub(crate) use risk::validate_order_limits;

const ACTIVATIONS_NS: &str = "execution_activations";
const RUNS_NS: &str = "execution_runs";
const CONFIGS_NS: &str = "execution_configs";
const CONTROLS_NS: &str = "execution_controls";
const RECONCILIATION_NS: &str = "execution_reconciliation";
const ATTEMPTS_NS: &str = "execution_attempts";
const REVISION_NS: &str = "capability_graph_revisions";
const ENTITLEMENT_NS: &str = "entitlement_grants";

/// Dependencies for the local current-state loader.  All observations are
/// supplied by trusted ports; callers cannot provide an owner identity in the
/// operation request.
#[derive(Clone)]
pub struct BrokerSubmissionDependencies {
    pub storage: Arc<dyn StoragePort>,
    pub plugins: Arc<dyn PluginRegistryPort>,
    pub plugin_packages: Arc<dyn PluginPackagePort>,
    pub credentials: Arc<dyn CredentialPort>,
    pub capability_resolver: Arc<dyn CapabilityResolverPort>,
    pub clock: Arc<dyn ClockPort>,
    pub leases: Arc<dyn LeaseRepository>,
    pub owner: LocalOwnerIdentity,
}

/// State passed to the later intent/risk/C5 implementation.  This is not a
/// permit and cannot be used to dispatch without the final authorization step.
#[derive(Clone, Debug)]
pub struct BrokerCurrentState {
    pub run: Value,
    pub activation: Value,
    pub config: Value,
    pub capability_revision: Value,
    pub controls: Value,
    pub reconciliation: Value,
    pub provenance: BrokerExecutionProvenance,
    pub current_lease: LeaseClaim,
    pub selected: Value,
    pub instance: Value,
    pub manifest: Value,
    pub package: crate::ports::InstalledPluginPackage,
    pub mandate: crate::local_live_authorization::LocalLiveMandate,
    pub actor: crate::local_live_authorization::VerifiedLiveActor,
    pub now_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrokerExecutionProvenance {
    Deterministic {
        attempt: Value,
    },
    Agent {
        run_id: String,
        deployment_id: String,
        binding_digest: String,
    },
}

/// Check the pinned invocation against the freshly loaded state. This does not
/// authorize an order: intent/risk checks and C5 authorization are still required.
pub fn validate_prepared_binding(
    state: &BrokerCurrentState,
    prepared: &crate::ports::broker_submission::BrokerPreparedBinding,
) -> Result<(), String> {
    let expected = crate::ports::broker_submission::BrokerPreparedBinding {
        plugin_instance_ref: required_value(&state.selected, "pluginInstanceRef")?.into(),
        plugin_ref: required_value(&state.selected, "pluginRef")?.into(),
        package_sha256: state.package.package_sha256.clone(),
        manifest_fingerprint: required_value(&state.selected, "manifestFingerprint")?.into(),
        configuration_digest: crate::spec::canonical_hash(&state.instance["configuration"])
            .map_err(|_| "prepared_configuration_digest_invalid".to_string())?,
        credential_ref: required_value(&state.instance, "credentialRef")?.into(),
        credential_generation: crate::live_execution_checks::credential_revision(&state.instance),
    };
    if prepared != &expected {
        return Err("prepared_invocation_binding_stale".into());
    }
    Ok(())
}

/// Build recoverable, non-secret intent from current state and exact order input.
/// This is NOT an authorization or durable receipt. The admission boundary must
/// enforce risk, persist it immutably and obtain C5 permission before dispatch.
pub fn build_order_intent(
    state: &BrokerCurrentState,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
    prepared: &crate::ports::broker_submission::BrokerPreparedBinding,
) -> Result<(Value, String), String> {
    validate_prepared_binding(state, prepared)?;
    let order = crate::broker_order_intent::CanonicalBrokerOrder::from_input(&request.input)?;
    validate_order_symbol(&state.config, &order.symbol)?;
    let provenance = match &state.provenance {
        BrokerExecutionProvenance::Agent {
            run_id,
            deployment_id,
            binding_digest,
        } => {
            serde_json::json!({"kind":"agent", "runId":run_id, "deploymentId":deployment_id, "bindingDigest":binding_digest})
        }
        BrokerExecutionProvenance::Deterministic { .. } => {
            serde_json::json!({"kind":"deterministic", "attemptId":request.attempt_id})
        }
    };
    let intent = serde_json::json!({
        "schemaVersion": "tradeassembly.broker_order_intent.v1",
        "actor": state.actor,
        "executionProvenance": provenance,
        "accountRef": state.mandate.binding.account_ref,
        "configId": state.mandate.binding.execution_config_id,
        "configDigest": state.mandate.binding.execution_config_digest,
        "mandateId": state.mandate.mandate_id,
        "mandateDigest": state.mandate.digest,
        "leaseResource": state.current_lease.resource,
        "leaseOwner": state.current_lease.owner,
        "leaseFence": state.current_lease.fencing_token,
        "leaseExpiresAtMs": state.current_lease.expires_at_ms,
        "strategyVersion": state.mandate.binding.strategy_version,
        "strategyHash": state.mandate.binding.strategy_hash,
        "activationId": request.activation_id,
        "correlationId": request.correlation_id,
        "idempotencyKey": context.idempotency_key.as_str(),
        "operationId": request.operation_id,
        "packageSha256": prepared.package_sha256,
        "manifestFingerprint": prepared.manifest_fingerprint,
        "configurationDigest": prepared.configuration_digest,
        "credentialGeneration": prepared.credential_generation,
        "preparedBinding": prepared,
        "requestBinding": {
            "schemaVersion":"tradeassembly.plugin_request_binding.v1",
            "operationId":request.operation_id, "capability":request.capability,
            "pluginInstanceRef":request.plugin_instance_ref, "pluginRef":request.plugin_ref,
            "manifestFingerprint":request.manifest_fingerprint,
            "capabilityGraphRevisionId":request.capability_graph_revision_id,
            "capabilityGraphFingerprint":request.capability_graph_fingerprint,
            "accountRef":request.account_ref, "mode":request.mode,
            "symbolHash":crate::spec::canonical_hash(&request.input["symbol"])?
        },
        "order": order,
        "dispatchQuantity": order.canonical_quantity(),
    });
    let digest = crate::spec::canonical_hash(&intent)
        .map_err(|_| "broker_order_intent_digest_invalid".to_string())?;
    Ok((intent, digest))
}

pub fn load_current_state(
    deps: &BrokerSubmissionDependencies,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
) -> Result<BrokerCurrentState, String> {
    let now_ms = deps.clock.trusted_now_ms()?;
    let activation_id = required(&request.activation_id, "activation_id_missing")?;
    let run_id = format!("run_{activation_id}");
    let run = read(deps.storage.as_ref(), RUNS_NS, &run_id)?;
    let activation = read(deps.storage.as_ref(), ACTIVATIONS_NS, activation_id)?;
    let config_id = required_value(&run, "configId")?;
    let config = read(deps.storage.as_ref(), CONFIGS_NS, config_id)?;
    let controls = read(deps.storage.as_ref(), CONTROLS_NS, activation_id)?;
    let reconciliation = read(deps.storage.as_ref(), RECONCILIATION_NS, activation_id)?;
    let attempt = if context.agent_execution().is_none() {
        Some(
            deps.storage
                .get_json(ATTEMPTS_NS, &request.attempt_id)?
                .ok_or_else(|| "execution_attempts_record_missing".to_string())?,
        )
    } else {
        None
    };

    if run["activationId"] != activation_id {
        return Err("activation_id_mismatch".into());
    }
    exact(request, &run)?;
    if run["state"] != "active" || activation["state"] != "active" {
        return Err("execution_not_active".into());
    }
    if activation["activationId"] != activation_id
        || activation["orchestrator"] != run["orchestrator"]
        || config["orchestrator"].as_str().unwrap_or("deterministic")
            != run["orchestrator"].as_str().unwrap_or("deterministic")
        || activation["mode"] != run["mode"]
        || activation["localLiveAuthority"] != run["localLiveAuthority"]
        || activation["capabilityGraphRevisionId"] != run["capabilityGraphRevisionId"]
        || activation["strategyVersionId"] != run["strategyVersionId"]
        || activation["strategySpecHash"] != run["strategySpecHash"]
    {
        return Err("activation_binding_mismatch".into());
    }
    if controls_blocked(&controls)
        || controls["activationId"] != activation_id
        || reconciliation["activationId"] != activation_id
        || reconciliation["state"] != "clear"
        || reconciliation["newEntriesPaused"] != false
    {
        return Err("execution_controls_blocked".into());
    }
    if let Some(attempt) = &attempt {
        if attempt["state"] != "running"
            || attempt["activationId"] != activation_id
            || attempt["attemptId"] != request.attempt_id
        {
            return Err("execution_attempt_invalid".into());
        }
    }

    let revision_id = required_value(&run, "capabilityGraphRevisionId")?;
    let revision = read(deps.storage.as_ref(), REVISION_NS, revision_id)?;
    if revision["revisionId"] != revision_id
        || revision["graphFingerprint"] != run["capabilityGraphFingerprint"]
    {
        return Err("capability_revision_stale".into());
    }
    let selected = selected_binding(&revision, request)?;
    let instance_ref = required_value(&selected, "pluginInstanceRef")?;
    let plugin_ref = required_value(&selected, "pluginRef")?;
    let stored_instance = deps
        .plugins
        .get_instance(instance_ref)?
        .ok_or_else(|| "plugin_instance_missing".to_string())?;
    let manifest = deps
        .plugins
        .get_manifest(plugin_ref)?
        .ok_or_else(|| "plugin_manifest_missing".to_string())?;
    let package_digest = required_value(&stored_instance, "activePackageSha256")?;
    let package = deps
        .plugin_packages
        .get(package_digest)?
        .ok_or_else(|| "plugin_package_missing".to_string())?;
    if package.plugin_ref != plugin_ref
        || package.manifest_sha256 != manifest["manifestDigest"]
        || stored_instance["activePackageSha256"] != manifest["package"]["packageSha256"]
        || stored_instance["enabled"] != true
    {
        return Err("plugin_package_binding_mismatch".into());
    }
    let credential_ref = required_value(&stored_instance, "credentialRef")?;
    let credential_status = deps.credentials.status(credential_ref);
    if !credential_status.configured {
        return Err("credential_not_configured".into());
    }
    let instance = apply_current_credential_status(stored_instance, &credential_status);
    if !live_account_observation_matches(&instance, &selected, now_ms) {
        return Err("live_account_observation_invalid".into());
    }

    let health = instance["health"]["state"].as_str().unwrap_or_default();
    let entry = stored_plugin_entry(&instance, &manifest, &credential_status, health.to_string())
        .ok_or_else(|| "plugin_catalog_invalid".to_string())?;
    let grants = stored_entitlements(deps.storage.as_ref())?;
    let requirement: CapabilityRequirement = serde_json::from_value(
        selected
            .get("requirement")
            .cloned()
            .or_else(|| {
                revision["graph"]["nodes"].as_array().and_then(|nodes| {
                    nodes
                        .iter()
                        .find(|node| node["selected"] == selected)
                        .map(|node| node["requirement"].clone())
                })
            })
            .ok_or_else(|| "capability_requirement_missing".to_string())?,
    )
    .map_err(|_| "capability_requirement_invalid".to_string())?;
    let fresh = deps.capability_resolver.resolve(
        requirement,
        CapabilityResolverCatalog {
            plugins: vec![entry],
            entitlements: grants,
        },
    );
    if !fresh.ok
        || !fresh.candidates.iter().any(|candidate| {
            candidate.plugin_instance_ref == selected["pluginInstanceRef"]
                && candidate.plugin_ref == selected["pluginRef"]
                && candidate.operation.id == selected["operationId"]
                && candidate.manifest_fingerprint == selected["manifestFingerprint"]
                && candidate.operation.capability == request.capability
                && candidate.operation.purpose == request.purpose
        })
    {
        return Err("capability_selection_stale".into());
    }

    let current =
        binding_from_observed_state(&config, &revision, &selected, &instance, &deps.owner)
            .map_err(str::to_string)?;
    let mandate =
        check_persisted_run_authority(deps.storage.as_ref(), &run, &config, &current, now_ms)
            .map_err(|error| format!("live_authority_invalid:{error:?}"))?;
    let actor = serde_json::from_value(run["localLiveAuthority"]["actor"].clone())
        .map_err(|_| "live_authority_actor_invalid".to_string())?;

    let (provenance, lease, actor) = if let Some(attempt) = attempt {
        if !attempt_fencing_matches(request, &attempt) {
            return Err("request_fencing_token_mismatch".into());
        }
        let resource = required_value(&attempt, "leaseResource")?;
        let lease = deps
            .leases
            .current(resource)?
            .ok_or_else(|| "execution_lease_missing".to_string())?;
        if lease.resource != resource
            || lease.owner != attempt["owner"].as_str().unwrap_or_default()
            || lease.fencing_token != attempt["fencingToken"].as_i64().unwrap_or_default()
            || lease.expires_at_ms <= now_ms
        {
            return Err("execution_lease_invalid".into());
        }
        (
            BrokerExecutionProvenance::Deterministic {
                attempt: attempt.clone(),
            },
            lease,
            actor,
        )
    } else {
        let agent = context
            .agent_execution()
            .ok_or_else(|| "agent_execution_context_missing".to_string())?;
        let lease = agent.current_lease(
            deps.storage.as_ref(),
            deps.clock.as_ref(),
            deps.leases.as_ref(),
        )?;
        if request.fencing_token.is_some() && request.fencing_token != Some(lease.fencing_token) {
            return Err("request_fencing_token_mismatch".into());
        }
        let deployment_record = read(
            deps.storage.as_ref(),
            crate::agent_runner::DEPLOYMENTS_NS,
            agent.deployment_id(),
        )?;
        if deployment_record["deployment"]["desiredState"] != "active"
            || deployment_record["deployment"]["executionConfigVersionId"] != config["configId"]
            || deployment_record["deployment"]["mode"] != "live"
        {
            return Err("agent_deployment_binding_invalid".into());
        }
        let agent_actor = deployment_actor(
            &deps.owner.stable_identity_id,
            agent.deployment_id(),
            agent.binding_digest(),
        );
        let mandate_id = run["localLiveAuthority"]["mandateId"]
            .as_str()
            .ok_or_else(|| "live_mandate_id_missing".to_string())?;
        let mandate = check_local_live_mandate(
            deps.storage.as_ref(),
            mandate_id,
            &current,
            &agent_actor,
            now_ms,
        )
        .map_err(|error| format!("agent_live_authority_invalid:{error:?}"))?;
        if mandate.digest != run["localLiveAuthority"]["mandateDigest"] {
            return Err("live_authority_invalid".into());
        }
        let provenance = BrokerExecutionProvenance::Agent {
            run_id: agent.run_id().to_string(),
            deployment_id: agent.deployment_id().to_string(),
            binding_digest: agent.binding_digest().to_string(),
        };
        (provenance, lease, agent_actor)
    };
    Ok(BrokerCurrentState {
        run,
        activation,
        config,
        capability_revision: revision,
        controls,
        reconciliation,
        provenance,
        current_lease: lease,
        selected,
        instance,
        manifest,
        package,
        mandate,
        actor,
        now_ms,
    })
}

fn validate_order_symbol(config: &Value, symbol: &str) -> Result<(), String> {
    let configured = config["symbol"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "execution_symbol_missing".to_string())?;
    // Do not normalize or infer broker instrument aliases at the authority boundary.
    if configured != symbol {
        return Err("order_symbol_outside_execution_config".into());
    }
    Ok(())
}

fn exact(request: &PluginOperationRequest, run: &Value) -> Result<(), String> {
    for (name, actual, expected) in [
        (
            "strategy_id",
            request.strategy_id.as_str(),
            run["strategyId"].as_str(),
        ),
        (
            "strategy_version_id",
            request.strategy_version_id.as_str(),
            run["strategyVersionId"].as_str(),
        ),
        (
            "strategy_spec_hash",
            request.strategy_spec_hash.as_str(),
            run["strategySpecHash"].as_str(),
        ),
        ("mode", request.mode.as_str(), run["mode"].as_str()),
        (
            "capability_graph_revision_id",
            request.capability_graph_revision_id.as_str(),
            run["capabilityGraphRevisionId"].as_str(),
        ),
        (
            "capability_graph_fingerprint",
            request.capability_graph_fingerprint.as_str(),
            run["capabilityGraphFingerprint"].as_str(),
        ),
    ] {
        if expected != Some(actual) {
            return Err(format!("request_{name}_mismatch"));
        }
    }
    Ok(())
}

fn selected_binding(revision: &Value, request: &PluginOperationRequest) -> Result<Value, String> {
    revision["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["requirement"]["capability"] == request.capability)
        .filter(|node| node["blockers"].as_array().is_some_and(Vec::is_empty))
        .filter_map(|node| node.get("selected"))
        .find(|selected| {
            selected["operationId"] == request.operation_id
                && selected["pluginInstanceRef"] == request.plugin_instance_ref
                && selected["pluginRef"] == request.plugin_ref
                && selected["manifestFingerprint"] == request.manifest_fingerprint
                && selected["accountRef"]
                    == request
                        .account_ref
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null)
        })
        .cloned()
        .ok_or_else(|| "selected_capability_missing".into())
}

fn stored_entitlements(
    storage: &dyn StoragePort,
) -> Result<Vec<crate::ports::EntitlementGrant>, String> {
    let mut grants = local_full_entitlements();
    for (_, value) in storage.list_json(ENTITLEMENT_NS)? {
        grants.push(
            serde_json::from_value(value).map_err(|_| "entitlement_grant_invalid".to_string())?,
        );
    }
    Ok(grants)
}

fn controls_blocked(controls: &Value) -> bool {
    controls["paused"] == true
        || controls["stopRequested"] == true
        || controls["emergencyStop"] == true
        || controls["newEntriesPaused"] == true
        || controls["pauseEntries"] == "paused"
        || controls["stop"] == "requested"
        || controls["stop"] == "stopped"
        || controls["emergencyStop"] == "requested"
        || controls["emergencyStop"] == "engaged"
        || controls["lastControl"]["action"] == "pause_entries"
        || controls["lastControl"]["action"] == "stop"
        || controls["lastControl"]["action"] == "emergency_stop"
}

fn attempt_fencing_matches(request: &PluginOperationRequest, attempt: &Value) -> bool {
    request.fencing_token.is_some() && request.fencing_token == attempt["fencingToken"].as_i64()
}

fn read(storage: &dyn StoragePort, namespace: &str, key: &str) -> Result<Value, String> {
    storage
        .get_json(namespace, key)?
        .ok_or_else(|| format!("{namespace}_record_missing"))
}

fn required<'a>(value: &'a str, error: &'static str) -> Result<&'a str, String> {
    (!value.trim().is_empty())
        .then_some(value)
        .ok_or_else(|| error.into())
}

fn required_value<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{field}_missing"))
}

#[cfg(test)]
#[path = "broker_submission/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "broker_submission/e2e.rs"]
mod e2e;
