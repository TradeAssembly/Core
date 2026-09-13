//! Read-only recovery of an already admitted broker operation.
//!
//! Recovery deliberately reconstructs the original operation from durable
//! records.  Nothing supplied by a caller is used as an authority or as an
//! input to the lookup request.

use super::{BrokerSubmissionDependencies, CONFIGS_NS};
use crate::agent_runner::VerifiedAgentMcpExecutionContext;
use crate::control_plane::ControlPlaneCommandEnvelope;
use crate::finance_authority::validate_stored_broker_authorization;
use crate::ports::broker_submission::BrokerPreparedBinding;
use crate::ports::{AuthorityContext, IdempotencyKey, PluginOperationRequest};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const INTENTS_NS: &str = "broker_order_intents";
const REQUESTS_NS: &str = "plugin_operation_requests";
const PREPARED_NS: &str = "plugin_operation_prepared_bindings";
const CLAIMS_NS: &str = "plugin_broker_dispatch_claims";
const DENIALS_NS: &str = "plugin_broker_submission_denials";

pub struct BrokerOrderRecoveryPlan {
    pub(crate) original_key: String,
    pub(crate) intent: Value,
    pub(crate) lookup: PluginOperationRequest,
    pub(crate) binding: BrokerPreparedBinding,
    deps: BrokerSubmissionDependencies,
    observer: RecoveryObserver,
    original_records: Value,
}

pub(crate) enum RecoveryObserver {
    Owner { issuer: String, subject: String },
    Agent(VerifiedAgentMcpExecutionContext),
}

pub(crate) fn prepare_plan(
    deps: BrokerSubmissionDependencies,
    original_key: &str,
    observer: RecoveryObserver,
) -> Result<BrokerOrderRecoveryPlan, String> {
    if original_key.trim().is_empty() {
        return Err("recovery_key_missing".into());
    }
    let records = load_records(&deps, original_key)?;
    validate_observer(&deps, &observer, &records["intent"])?;
    let intent = records["intent"].clone();
    let binding: BrokerPreparedBinding = serde_json::from_value(records["prepared"].clone())
        .map_err(|_| "recovery_prepared_binding_invalid".to_string())?;
    validate_records(&deps, original_key, &records, &intent, &binding)?;
    let lookup = build_lookup(
        original_key,
        &intent,
        &records["request"],
        &records["config"],
        &binding,
    )?;
    let envelope = envelope(&intent)?;
    validate_stored_broker_authorization(deps.storage.as_ref(), &envelope)?;
    Ok(BrokerOrderRecoveryPlan {
        original_key: original_key.into(),
        intent,
        lookup,
        binding,
        deps,
        observer,
        original_records: records,
    })
}

impl BrokerOrderRecoveryPlan {
    pub(crate) fn revalidate(&self) -> Result<(), String> {
        let records = load_records(&self.deps, &self.original_key)?;
        validate_observer(&self.deps, &self.observer, &records["intent"])?;
        validate_records(
            &self.deps,
            &self.original_key,
            &records,
            &self.intent,
            &self.binding,
        )?;
        let envelope = envelope(&self.intent)?;
        validate_stored_broker_authorization(self.deps.storage.as_ref(), &envelope)?;
        if records != self.original_records {
            return Err("recovery_evidence_changed".into());
        }
        Ok(())
    }
}

fn load_records(deps: &BrokerSubmissionDependencies, key: &str) -> Result<Value, String> {
    let intent = get(deps, INTENTS_NS, &format!("intent_{}", hash(key)))?;
    let request = get(deps, REQUESTS_NS, key)?;
    let prepared = get(deps, PREPARED_NS, key)?;
    let claim = get(deps, CLAIMS_NS, key)?;
    if get_optional(deps, DENIALS_NS, key)?.is_some() {
        return Err("recovery_terminal_denial".into());
    }
    Ok(
        json!({"intent":intent,"request":request,"prepared":prepared,"claim":claim,
        "config": get(deps, CONFIGS_NS, intent_field(&intent,"configId")?)?,
        "decision":get(deps,crate::finance_authority::DECISION_NS,&format!("cmd_{}",hash(key)))?,
        "receipt":get(deps,crate::finance_authority::RECEIPT_NS,&format!("cmd_{}",hash(key)))?}),
    )
}

fn validate_records(
    deps: &BrokerSubmissionDependencies,
    key: &str,
    records: &Value,
    intent: &Value,
    binding: &BrokerPreparedBinding,
) -> Result<(), String> {
    if intent["schemaVersion"] != "tradeassembly.broker_order_intent.v1"
        || intent["idempotencyKey"] != key
    {
        return Err("recovery_intent_invalid".into());
    }
    let request = &records["request"];
    if request["correlationId"] != intent["correlationId"]
        || request["requestHash"].as_str().is_none()
    {
        return Err("recovery_request_mismatch".into());
    }
    if records["claim"]["requestHash"] != request["requestHash"]
        || records["claim"]["correlationId"] != request["correlationId"]
    {
        return Err("recovery_claim_mismatch".into());
    }
    if intent["packageSha256"] != binding.package_sha256
        || intent["manifestFingerprint"] != binding.manifest_fingerprint
        || intent["configurationDigest"] != binding.configuration_digest
        || intent["credentialGeneration"] != binding.credential_generation
    {
        return Err("recovery_binding_mismatch".into());
    }
    let config = &records["config"];
    if config["configId"] != intent["configId"]
        || crate::spec::canonical_hash(config).map_err(|_| "recovery_config_invalid".to_string())?
            != intent["configDigest"]
    {
        return Err("recovery_config_mismatch".into());
    }
    let selected = config
        .get("selectedPluginInstanceRef")
        .or_else(|| config.get("pluginInstanceRef"));
    if selected.is_some() && selected != Some(&Value::String(binding.plugin_instance_ref.clone())) {
        return Err("recovery_plugin_instance_mismatch".into());
    }
    // The immutable request binding is the only source for graph identity and
    // operation metadata; do not accept those fields from a recovery caller.
    let rb = request
        .get("binding")
        .ok_or("recovery_request_binding_missing")?;
    if intent["preparedBinding"] != records["prepared"] || &intent["requestBinding"] != rb {
        return Err("recovery_authorized_binding_mismatch".into());
    }
    for (field, expected) in [
        ("pluginInstanceRef", &binding.plugin_instance_ref),
        ("pluginRef", &binding.plugin_ref),
        ("manifestFingerprint", &binding.manifest_fingerprint),
    ] {
        if rb[field].as_str() != Some(expected) {
            return Err("recovery_request_binding_mismatch".into());
        }
    }
    if rb["schemaVersion"] != "tradeassembly.plugin_request_binding.v1"
        || rb["mode"] != "live"
        || rb["accountRef"] != intent["accountRef"]
        || rb["operationId"] != "broker.live_order_submit"
        || rb["capability"] != "broker.order_submit.live"
        || intent["operationId"] != "broker.live_order_submit"
        || records["claim"]["state"] != "claimed"
        || rb["symbolHash"] != crate::spec::canonical_hash(&intent["order"]["symbol"])?
    {
        return Err("recovery_original_operation_mismatch".into());
    }
    let instance = deps
        .plugins
        .get_instance(&binding.plugin_instance_ref)?
        .ok_or("recovery_instance_missing")?;
    if instance["enabled"] != true
        || instance.get("removedAtMs").is_some_and(|v| !v.is_null())
        || instance["instanceRef"] != binding.plugin_instance_ref
        || instance["pluginRef"] != binding.plugin_ref
        || instance["accountRef"] != intent["accountRef"]
        || instance["accountMode"] != "live"
        || instance["activePackageSha256"] != binding.package_sha256
        || crate::spec::canonical_hash(&instance["configuration"])? != binding.configuration_digest
        || instance["credentialRef"] != binding.credential_ref
        || crate::live_execution_checks::credential_revision(&instance)
            != binding.credential_generation
        || !deps.credentials.status(&binding.credential_ref).configured
    {
        return Err("recovery_current_binding_changed".into());
    }
    let manifest = deps
        .plugins
        .get_manifest(&binding.plugin_ref)?
        .ok_or("recovery_manifest_missing")?;
    let package = deps
        .plugin_packages
        .get(&binding.package_sha256)?
        .ok_or("recovery_package_missing")?;
    if package.plugin_ref != binding.plugin_ref
        || package.package_sha256 != binding.package_sha256
        || package.manifest_sha256 != manifest["manifestDigest"]
        || manifest["package"]["packageSha256"] != binding.package_sha256
        || manifest["manifestDigest"] != binding.manifest_fingerprint
        || crate::spec::canonical_hash(&manifest["manifest"])?.trim_start_matches("sha256:")
            != binding.manifest_fingerprint.trim_start_matches("sha256:")
    {
        return Err("recovery_current_package_changed".into());
    }
    Ok(())
}

fn build_lookup(
    key: &str,
    intent: &Value,
    request_record: &Value,
    config: &Value,
    binding: &BrokerPreparedBinding,
) -> Result<PluginOperationRequest, String> {
    let input = intent["order"]
        .get("clientOrderId")
        .cloned()
        .map(|v| json!({"clientOrderId":v}))
        .ok_or_else(|| "recovery_client_order_id_missing".to_string())?;
    Ok(PluginOperationRequest {
        correlation_id: intent_field(intent, "correlationId")?.into(),
        plugin_instance_ref: binding.plugin_instance_ref.clone(),
        plugin_ref: binding.plugin_ref.clone(),
        manifest_fingerprint: binding.manifest_fingerprint.clone(),
        operation_id: "broker.order_lookup".into(),
        capability: "broker.order_lookup.live".into(),
        capability_graph_revision_id: intent_field(
            &request_record["binding"],
            "capabilityGraphRevisionId",
        )?
        .into(),
        capability_graph_fingerprint: request_record["binding"]["capabilityGraphFingerprint"]
            .as_str()
            .unwrap_or_default()
            .into(),
        strategy_id: intent_field(config, "strategyId")?.into(),
        strategy_version_id: intent_field(intent, "strategyVersion")?.into(),
        strategy_spec_hash: intent_field(intent, "strategyHash")?.into(),
        activation_id: intent_field(intent, "activationId")?.into(),
        attempt_id: format!("recovery:{key}"),
        evaluation_tick_id: format!("recovery:{key}"),
        mode: "live".into(),
        purpose: "recovery".into(),
        account_ref: intent["accountRef"].as_str().map(str::to_string),
        timeout_ms: 5000,
        fencing_token: None,
        input,
        evidence_refs: vec![],
    })
}

fn envelope(intent: &Value) -> Result<ControlPlaneCommandEnvelope, String> {
    let digest = crate::spec::canonical_hash(intent)
        .map_err(|_| "recovery_intent_digest_invalid".to_string())?;
    Ok(ControlPlaneCommandEnvelope {
        schema_version: "tradeassembly.control_plane.command.v1".into(),
        command_id: format!("cmd_{}", hash(intent_field(intent, "idempotencyKey")?)),
        correlation_id: intent_field(intent, "correlationId")?.into(),
        command_name: "order.submit.live".into(),
        command_group: "order".into(),
        source_interface: "broker_boundary".into(),
        target_object: intent["accountRef"].as_str().map(str::to_string),
        side_effect_class: "live_order".into(),
        authority: AuthorityContext {
            actor: intent["actor"]["subject"]
                .as_str()
                .unwrap_or_default()
                .into(),
            surface: "broker_boundary".into(),
            account_mode: "live".into(),
        },
        idempotency_key: IdempotencyKey::new(intent_field(intent, "idempotencyKey")?)?,
        idempotency_requirement: "required".into(),
        expected_sequence: None,
        evidence_refs: vec![format!("broker-intent:{digest}")],
        payload_hash: digest,
        payload_preview: intent.clone(),
    })
}

fn validate_observer(
    deps: &BrokerSubmissionDependencies,
    observer: &RecoveryObserver,
    intent: &Value,
) -> Result<(), String> {
    match observer {
        RecoveryObserver::Owner { issuer, subject } => {
            if issuer != &deps.owner.issuer || subject != &deps.owner.subject {
                return Err("recovery_owner_mismatch".into());
            }
        }
        RecoveryObserver::Agent(agent) => {
            let _ = agent.current_lease(
                deps.storage.as_ref(),
                deps.clock.as_ref(),
                deps.leases.as_ref(),
            )?;
            let p = &intent["executionProvenance"];
            if p["kind"] != "agent"
                || p["deploymentId"] != agent.deployment_id()
                || p["bindingDigest"] != agent.binding_digest()
                || intent["actor"]
                    != json!(crate::local_live_authorization::deployment_actor(
                        &deps.owner.stable_identity_id,
                        agent.deployment_id(),
                        agent.binding_digest()
                    ))
            {
                return Err("recovery_agent_mismatch".into());
            }
        }
    }
    Ok(())
}

fn intent_field<'a>(v: &'a Value, field: &str) -> Result<&'a str, String> {
    v[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("recovery_{field}_missing"))
}
fn get(deps: &BrokerSubmissionDependencies, ns: &str, key: &str) -> Result<Value, String> {
    get_optional(deps, ns, key)?.ok_or_else(|| format!("{ns}_record_missing"))
}
fn get_optional(
    deps: &BrokerSubmissionDependencies,
    ns: &str,
    key: &str,
) -> Result<Option<Value>, String> {
    deps.storage.get_json(ns, key)
}
fn hash(value: &str) -> String {
    let mut h = Sha256::new();
    h.update(value.as_bytes());
    format!("{:x}", h.finalize())
}
