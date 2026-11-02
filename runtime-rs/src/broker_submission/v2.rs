//! Versioned V2 admission through the same C5 authority and trusted run loader.
use super::*;
use crate::ports::broker_submission::{BrokerPreparedBinding, BrokerSubmissionPermit};
use crate::{control_plane::ControlPlaneCommandEnvelope, ports::*, service::order_plans};
pub use order_plans::submission::OrderPlanSource;
use serde_json::json;
use sha2::{Digest, Sha256};
use tradeassembly_plugin_sdk::broker_lookup_v3 as lookup_v3;
use tradeassembly_plugin_sdk::broker_v2::{self as wire, OrderObservation, OrderStatus};
pub const SUBMIT_OPERATION: &str = "broker.paper_order_submit_v2";
pub const SUBMIT_CAPABILITY: &str = "broker.order_submit.paper@2";
pub const LOOKUP_OPERATION: &str = "broker.order_lookup.paper_v2";
pub const LOOKUP_CAPABILITY: &str = "broker.order_lookup.paper@2";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryLookup {
    LegacyV2,
    OriginalOrderV3,
}
impl RecoveryLookup {
    fn operation(self) -> &'static str {
        match self {
            Self::LegacyV2 => LOOKUP_OPERATION,
            Self::OriginalOrderV3 => lookup_v3::OPERATION_ID,
        }
    }
    fn capability(self) -> &'static str {
        match self {
            Self::LegacyV2 => LOOKUP_CAPABILITY,
            Self::OriginalOrderV3 => lookup_v3::CAPABILITY,
        }
    }
}
// Selection is an explicit dependency of the immutable submit operation, never
// an availability-based fallback. Existing V2 packages retain their exact wire.
fn recovery_lookup(manifest: &Value, submit_operation: &str) -> Result<RecoveryLookup, String> {
    let operations = manifest["operations"]
        .as_array()
        .ok_or("broker_v2_lookup_operation_missing")?;
    let mut submits = operations
        .iter()
        .filter(|op| op["id"] == submit_operation && op["capability"] == SUBMIT_CAPABILITY);
    let submit = submits
        .next()
        .ok_or("broker_v2_lookup_submit_binding_invalid")?;
    if submits.next().is_some() {
        return Err("broker_v2_lookup_submit_binding_invalid".into());
    }
    let dependencies = submit["dependencies"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let declared = dependencies
        .iter()
        .filter(|dependency| dependency["dependencyId"] == lookup_v3::DEPENDENCY_ID)
        .collect::<Vec<_>>();
    let selected = match declared.as_slice() {
        [] => RecoveryLookup::LegacyV2,
        [dependency]
            if dependency["required"] == true
                && dependency["capability"] == lookup_v3::CAPABILITY
                && dependency["operationId"] == lookup_v3::OPERATION_ID =>
        {
            RecoveryLookup::OriginalOrderV3
        }
        [dependency]
            if dependency["required"] == true
                && dependency["capability"] == LOOKUP_CAPABILITY
                && dependency["operationId"] == LOOKUP_OPERATION =>
        {
            RecoveryLookup::LegacyV2
        }
        _ => return Err("broker_v2_lookup_dependency_invalid".into()),
    };
    if operations
        .iter()
        .filter(|op| {
            op["id"] == selected.operation()
                && op["capability"] == selected.capability()
                && op["effect"] == "read"
        })
        .count()
        != 1
    {
        return Err("broker_v2_lookup_operation_missing".into());
    }
    Ok(selected)
}
pub(crate) fn recovery_lookup_binding(
    manifest: &Value,
    submit_operation: &str,
) -> Result<(&'static str, &'static str), String> {
    let selected = recovery_lookup(manifest, submit_operation)?;
    Ok((selected.operation(), selected.capability()))
}
fn recovery_input(
    contract: RecoveryLookup,
    original: &wire::SubmitRequest,
    account: &account_identity::CanonicalAccountIdentity,
) -> Result<Value, String> {
    original.validate()?;
    if account.identity_sha256 != original.canonical_account_sha256
        || account.environment != "paper"
    {
        return Err("broker_v2_canonical_account_mismatch".into());
    }
    match contract {
        RecoveryLookup::LegacyV2 => Ok(
            json!({"schemaVersion":"tradeassembly.broker_lookup.v2","canonicalAccountSha256":original.canonical_account_sha256,"clientOrderId":original.order.client_order_id}),
        ),
        RecoveryLookup::OriginalOrderV3 => {
            let request = lookup_v3::LookupRequestV3 {
                schema_version: lookup_v3::VERSION.into(),
                original_submit: original.clone(),
                expected_account: serde_json::from_value(json!({
                    "brokerNamespace":account.broker_namespace,
                    "environment":account.environment,
                    "providerAccountId":account.provider_account_id,
                }))
                .map_err(|_| "broker_v3_expected_account_invalid")?,
            };
            request.validate()?;
            serde_json::to_value(request).map_err(|_| "broker_v3_lookup_encoding_invalid".into())
        }
    }
}
pub(crate) fn key_hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
pub(crate) fn is_submission(r: &PluginOperationRequest) -> bool {
    r.operation_id == SUBMIT_OPERATION
        && r.capability == SUBMIT_CAPABILITY
        && r.mode == "paper"
        && r.purpose == "paper_trading"
}
fn validate(
    deps: &BrokerSubmissionDependencies,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
    prepared: &BrokerPreparedBinding,
    source: &OrderPlanSource,
) -> Result<(BrokerCurrentState, order_plans::OrderPlan, Value), String> {
    let _timing = order_plans::financial_timing("broker.v2.validate");
    if !is_submission(request) {
        return Err("broker_v2_paper_context_required".into());
    }
    source.with_validation_round(|round| validate_in_round(deps, request, context, prepared, round))
}
fn validate_in_round(
    deps: &BrokerSubmissionDependencies,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
    prepared: &BrokerPreparedBinding,
    source: &OrderPlanSource,
) -> Result<(BrokerCurrentState, order_plans::OrderPlan, Value), String> {
    if !is_submission(request) {
        return Err("broker_v2_paper_context_required".into());
    }
    let _timing = order_plans::financial_timing("broker.v2.validate_body");
    let owner = source
        .service()
        .invocation_owner()
        .ok_or("broker_v2_owner_required")?;
    if owner.issuer != deps.owner.issuer || owner.subject != deps.owner.subject {
        return Err("broker_v2_owner_mismatch".into());
    }
    let p = source.historical()?;
    let input: wire::SubmitRequest =
        serde_json::from_value(request.input.clone()).map_err(|_| "broker_v2_request_invalid")?;
    input.validate()?;
    if serde_json::to_value(wire::SubmitRequest {
        schema_version: "tradeassembly.broker_submit.v2".into(),
        plan_id: p.plan_id.clone(),
        canonical_account_sha256: p
            .canonical_account
            .as_ref()
            .ok_or("broker_v2_canonical_plan_required")?
            .identity_sha256
            .clone(),
        order: p.order.clone(),
    })
    .map_err(|_| "broker_v2_request_invalid")?
        != request.input
    {
        return Err("broker_v2_plan_input_changed".into());
    }
    if request.strategy_id != p.command.scope.strategy_id
        || request.strategy_version_id != p.command.scope.strategy_version_id
        || request.strategy_spec_hash != p.command.scope.strategy_spec_hash
        || request.capability_graph_revision_id != p.command.capability_graph_revision
    {
        return Err("broker_v2_strategy_binding_mismatch".into());
    }
    let managing = p.management.is_some();
    let state = order_plans::measured("broker.load_current_state", || {
        if managing {
            load_management_state(deps, request, context)
        } else {
            load_current_state(deps, request, context)
        }
    })?;
    validate_prepared_binding(&state, prepared)?;
    let signal = if managing {
        None
    } else {
        Some(source.verified_signal()?)
    };
    if managing {
        order_plans::management::validate_approval(
            source.service(),
            &p,
            &state.run,
            &state.config,
        )?;
    } else if state.config["orchestrator"] == "typed_pipeline" {
        let selection = order_plans::decision::check_plan_with_signal(
            source.service(),
            &p,
            &state.run,
            &state.config,
            signal.as_ref().ok_or("broker_v2_signal_missing")?,
        )?;
        if state.run["orderPlanApprovals"][&p.plan_id]["selectionBindingSha256"] != selection {
            return Err("broker_v2_selection_changed".into());
        }
    } else {
        validate_order_symbol(&state.config, &p.order.instrument.provider_symbol)?;
    }
    if state.config["executionContractVersion"] != "tradeassembly.typed_execution.v2" {
        return Err("broker_v2_typed_activation_required".into());
    }
    let approval = &state.run["orderPlanApprovals"][&p.plan_id];
    if approval["planId"] != p.plan_id
        || approval["entryId"] != p.entry_id
        || approval["signalReceiptSha256"] != p.command.signal_receipt_sha256
        || approval["canonicalAccountSha256"] != input.canonical_account_sha256
        || approval["leaseResource"] != state.current_lease.resource
        || approval["leaseOwner"] != state.current_lease.owner
        || approval["leaseFence"] != state.current_lease.fencing_token
    {
        return Err("broker_v2_plan_decision_missing".into());
    }

    if !managing
        && state.config["orderPlanPolicySha256"]
            != crate::data_pipeline::canonical_sha256(&p.policy)
                .map_err(|_| "broker_v2_policy_invalid")?
    {
        return Err("broker_v2_risk_policy_not_activated".into());
    }
    let account = account_identity::from_state(&state)?;
    if Some(&account.identity) != p.canonical_account.as_ref() {
        return Err("broker_v2_canonical_account_mismatch".into());
    }
    // Recovery is an independently declared read on the exact same package.
    let recovery = recovery_lookup(&state.package.manifest, &request.operation_id)?;
    let status = deps
        .credentials
        .status(required_value(&state.instance, "credentialRef")?);
    let entry = crate::plugin_catalog::stored_plugin_entry(
        &state.instance,
        &state.manifest,
        &status,
        state.instance["health"]["state"]
            .as_str()
            .unwrap_or_default()
            .into(),
    )
    .ok_or("broker_v2_lookup_permission_unavailable")?;
    let lookup = deps.capability_resolver.resolve(
        CapabilityRequirement {
            requirement_id: "broker-v2-admission-recovery".into(),
            capability: recovery.capability().into(),
            mode: "paper".into(),
            plugin_instance_ref: Some(request.plugin_instance_ref.clone()),
            plugin_ref: Some(request.plugin_ref.clone()),
            operation_id: Some(recovery.operation().into()),
            account_ref: request.account_ref.clone(),
            purpose: Some("recovery".into()),
            asset_class: Some(p.order.instrument.asset_class.clone()),
            instrument_family: Some(p.order.instrument.instrument_family.clone()),
            fallback_policy: "deny".into(),
            ..Default::default()
        },
        CapabilityResolverCatalog {
            plugins: vec![entry],
            entitlements: stored_entitlements(deps.storage.as_ref())?,
        },
    );
    if !lookup.ok {
        return Err("broker_v2_lookup_permission_unavailable".into());
    }
    if order_plans::measured("broker.source.snapshot", || {
        if let Some(signal) = &signal {
            source.snapshot_with_signal(signal)
        } else {
            source.snapshot()
        }
    })? != p
    {
        return Err("broker_v2_plan_changed".into());
    }
    let (mut intent, _) = bound_intent(
        &state,
        request,
        context,
        prepared,
        serde_json::to_value(&p.order).map_err(|_| "broker_v2_order_invalid")?,
        p.order.quantity.to_string(),
        crate::spec::canonical_hash(&request.input["order"]["instrument"])?,
        "tradeassembly.broker_order_intent.v2",
    )?;
    let node = state.config["orderPlanBrokerNode"]
        .as_str()
        .ok_or("broker_v2_broker_node_not_activated")?;
    if !state.capability_revision["graph"]["nodes"]
        .as_array()
        .is_some_and(|nodes| {
            nodes
                .iter()
                .any(|n| n["nodeId"] == node && n["selected"] == state.selected)
        })
        || request
            .evidence_refs
            .iter()
            .filter(|s| s.starts_with("broker-node:"))
            .collect::<Vec<_>>()
            != vec![&format!("broker-node:{node}")]
    {
        return Err("broker_v2_broker_node_not_activated".into());
    }
    intent["brokerRequirementRef"] = json!(node);
    intent["orderPlanApprovalSha256"] = json!(crate::spec::canonical_hash(
        &state.run["orderPlanApprovals"][&p.plan_id]
    )?);
    intent["planId"] = json!(p.plan_id);
    intent["plan"] = serde_json::to_value(&p).map_err(|_| "broker_v2_plan_invalid")?;
    intent["canonicalAccount"] =
        serde_json::to_value(account.identity).map_err(|_| "broker_v2_account_invalid")?;
    intent["brokerAccountBindingSha256"] = json!(account.binding_sha256);
    intent["originalRequest"] =
        serde_json::to_value(request).map_err(|_| "broker_v2_request_invalid")?;
    intent["requestBinding"]["schemaVersion"] = json!("tradeassembly.plugin_request_binding.v2");
    let binding = intent["requestBinding"]
        .as_object_mut()
        .ok_or("broker_v2_binding_invalid")?;
    let instrument = binding
        .remove("symbolHash")
        .ok_or("broker_v2_binding_invalid")?;
    binding.insert("instrumentSha256".into(), instrument);
    // Hashing/authority work can itself cross a short quote or lease deadline.
    // Check the final trusted time, not only the loader's initial timestamp.
    let final_now = deps.clock.trusted_now_ms()?;
    if final_now >= p.valid_until_ms
        || final_now >= state.current_lease.expires_at_ms
        || !account_observation_matches(&state.instance, &state.selected, final_now, "paper")
    {
        return Err("broker_v2_authority_or_data_expired".into());
    }
    validate_execution_window(
        &state.run["orderPlanApprovals"][&p.plan_id]["executionWindow"],
        &p.command.scope,
        final_now,
        p.management.as_ref().map(|b| {
            (
                &p.order.position_action,
                b,
                p.command.capability_graph_revision.as_str(),
            )
        }),
    )?;
    Ok((state, p, intent))
}
/// Read-only native comparison; never persists an intent or obtains a permit.
#[cfg(test)]
pub(crate) fn profile_validation_round(
    source: &OrderPlanSource,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
) -> Result<Value, String> {
    let s = source.service();
    let runtime = s.runtime();
    let deps = BrokerSubmissionDependencies {
        storage: runtime.storage.clone(),
        plugins: runtime.plugins.clone(),
        plugin_packages: runtime.plugin_packages.clone(),
        credentials: runtime.credentials.clone(),
        capability_resolver: runtime.capability_resolver.clone(),
        clock: runtime.clock.clone(),
        leases: runtime.leases.clone(),
        owner: crate::local_owner_identity::LocalOwnerIdentity::existing_for_database(
            std::fs::canonicalize(s.db()).map_err(|_| "c5_profile_database_path_invalid")?,
        )?,
    };
    let state = load_management_state(&deps, request, context)?;
    let prepared = BrokerPreparedBinding {
        plugin_instance_ref: required_value(&state.selected, "pluginInstanceRef")?.into(),
        plugin_ref: required_value(&state.selected, "pluginRef")?.into(),
        package_sha256: state.package.package_sha256.clone(),
        manifest_fingerprint: required_value(&state.selected, "manifestFingerprint")?.into(),
        configuration_digest: crate::spec::canonical_hash(&state.instance["configuration"])?,
        credential_ref: required_value(&state.instance, "credentialRef")?.into(),
        credential_generation: crate::live_execution_checks::credential_revision(&state.instance),
    };
    let start = std::time::Instant::now();
    let (_, reference_plan, reference_intent) =
        crate::service::management_windows::with_reference_subject_check(|| {
            validate_in_round(&deps, request, context, &prepared, source)
        })?;
    let reference_us = start.elapsed().as_micros();
    let start = std::time::Instant::now();
    let (_, current_plan, current_intent) = validate(&deps, request, context, &prepared, source)?;
    let current_us = start.elapsed().as_micros();
    if reference_plan != current_plan || reference_intent != current_intent {
        return Err("c5_validation_profile_changed_inputs".into());
    }
    Ok(
        json!({"referenceUs":reference_us,"scopedUs":current_us,"samePlanAndIntent":true,
        "validUntilMs":current_plan.valid_until_ms,"finishedAtMs":deps.clock.trusted_now_ms()?,
        "durableIntentWritten":false,"brokerCalls":0,"scope":"single pure V2validate; independent C5 revalidations remainfresh"}),
    )
}

fn envelope(
    state: &BrokerCurrentState,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
    intent: Value,
) -> Result<ControlPlaneCommandEnvelope, String> {
    let digest = crate::spec::canonical_hash(&intent)?;
    Ok(ControlPlaneCommandEnvelope {
        schema_version: "tradeassembly.control_plane.command.v1".into(),
        command_id: format!("cmd_{}", key_hash(context.idempotency_key.as_str())),
        correlation_id: request.correlation_id.clone(),
        command_name: "order.submit.paper".into(),
        command_group: "order".into(),
        source_interface: "broker_boundary".into(),
        target_object: Some(required_value(&state.selected, "accountRef")?.into()),
        side_effect_class: "paper_order".into(),
        authority: AuthorityContext {
            actor: state.actor.subject.clone(),
            surface: "broker_boundary".into(),
            account_mode: "paper".into(),
        },
        idempotency_key: context.idempotency_key.clone(),
        idempotency_requirement: "required".into(),
        expected_sequence: None,
        evidence_refs: vec![
            format!(
                "order-plan:{}",
                intent["planId"].as_str().ok_or("broker_v2_plan_missing")?
            ),
            format!("broker-intent:{digest}"),
        ],
        payload_hash: digest,
        payload_preview: intent,
    })
}
pub(crate) fn admit(
    deps: &BrokerSubmissionDependencies,
    authority: &dyn crate::finance_authority::FinanceAuthorityPort,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
    prepared: &BrokerPreparedBinding,
    source: &OrderPlanSource,
) -> Result<BrokerSubmissionPermit, String> {
    let (state, p, intent) = validate(deps, request, context, prepared, source)?;
    source.begin(&p, intent.clone(), context)?;
    let e = envelope(&state, request, context, intent)?;
    authority.prepare_broker_submission(deps.storage.as_ref(), &e)?;
    revalidate(
        deps,
        request,
        context,
        prepared,
        &BrokerSubmissionPermit {
            envelope: e.clone(),
        },
        source,
    )?;
    source.admitted(context)?;
    Ok(BrokerSubmissionPermit { envelope: e })
}
pub(crate) fn revalidate(
    deps: &BrokerSubmissionDependencies,
    request: &PluginOperationRequest,
    context: &SideEffectContext,
    prepared: &BrokerPreparedBinding,
    permit: &BrokerSubmissionPermit,
    source: &OrderPlanSource,
) -> Result<(), String> {
    let (_, _, fresh) = validate(deps, request, context, prepared, source)?;
    if !super::admission::same_authority_during_renewal(
        &permit.envelope.payload_preview,
        &fresh,
        deps.clock.trusted_now_ms()?,
    ) {
        return Err("broker_v2_state_changed".into());
    }
    crate::finance_authority::validate_stored_broker_authorization(
        deps.storage.as_ref(),
        &permit.envelope,
    )
}
pub(crate) fn complete(
    deps: &BrokerSubmissionDependencies,
    authority: &dyn crate::finance_authority::FinanceAuthorityPort,
    permit: &BrokerSubmissionPermit,
    response: &PluginOperationResponse,
    source: &OrderPlanSource,
) -> Result<(), String> {
    crate::finance_authority::validate_stored_broker_authorization(
        deps.storage.as_ref(),
        &permit.envelope,
    )?;
    if response.correlation_id != permit.envelope.correlation_id
        || response.schema_ref != wire::OBSERVATION_SCHEMA
    {
        return Err("broker_v2_response_binding_invalid".into());
    }
    let mut o: OrderObservation = serde_json::from_value(response.payload.clone())
        .map_err(|_| "broker_v2_observation_invalid")?;
    o.validate(deps.clock.trusted_now_ms()?)?;
    let unknown = o.status == OrderStatus::Unknown || response.reconciliation_required;
    if response.reconciliation_required {
        o.status = OrderStatus::Unknown;
        o.filled_quantity = tradeassembly_plugin_sdk::execution_snapshot::Observed::Unavailable {
            reason: "provider_reconciliation_required".into(),
        };
        o.cash_spent = tradeassembly_plugin_sdk::execution_snapshot::Observed::Unavailable {
            reason: "provider_reconciliation_required".into(),
        };
    }
    let context = SideEffectContext::new(
        permit.envelope.authority.clone(),
        permit.envelope.idempotency_key.clone(),
    );
    let record = source.observe(o, &context)?;
    if unknown {
        return Err("plugin_order_reconciliation_required".into());
    }
    authority.complete_broker_submission(
        deps.storage.as_ref(),
        &permit.envelope,
        if record.phase == order_plans::submission::Phase::Rejected {
            409
        } else {
            202
        },
    )
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    KnownNotSent,
    Historical,
    Observed,
    StillUnknown,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryResult {
    pub schema_version: String,
    pub plan_id: String,
    pub kind: RecoveryKind,
    pub record_sha256: String,
    pub receipt: Option<PluginOperationResponse>,
}
pub struct RecoveryPlan {
    pub(crate) lookup: Option<PluginOperationRequest>,
    pub(crate) original: PluginOperationRequest,
    pub(crate) binding: BrokerPreparedBinding,
    pub(crate) original_key: String,
    pub(crate) envelope: ControlPlaneCommandEnvelope,
    pub(crate) kind: RecoveryKind,
}
pub(crate) fn original_envelope(
    intent: Value,
    request: &PluginOperationRequest,
    key: &str,
) -> Result<ControlPlaneCommandEnvelope, String> {
    Ok(ControlPlaneCommandEnvelope {
        schema_version: "tradeassembly.control_plane.command.v1".into(),
        command_id: format!("cmd_{}", key_hash(key)),
        correlation_id: request.correlation_id.clone(),
        command_name: "order.submit.paper".into(),
        command_group: "order".into(),
        source_interface: "broker_boundary".into(),
        target_object: Some(required_value(&intent, "accountRef")?.into()),
        side_effect_class: "paper_order".into(),
        authority: AuthorityContext {
            actor: required_value(&intent["actor"], "subject")?.into(),
            surface: "broker_boundary".into(),
            account_mode: "paper".into(),
        },
        idempotency_key: IdempotencyKey::new(key)?,
        idempotency_requirement: "required".into(),
        expected_sequence: None,
        evidence_refs: vec![
            format!("order-plan:{}", required_value(&intent, "planId")?),
            format!("broker-intent:{}", crate::spec::canonical_hash(&intent)?),
        ],
        payload_hash: crate::spec::canonical_hash(&intent)?,
        payload_preview: intent,
    })
}
pub(crate) fn prepare_recovery(
    deps: &BrokerSubmissionDependencies,
    source: &OrderPlanSource,
    context: &SideEffectContext,
) -> Result<RecoveryPlan, String> {
    let owner = source
        .service()
        .invocation_owner()
        .ok_or("broker_v2_owner_required")?;
    if owner.issuer != deps.owner.issuer
        || owner.subject != deps.owner.subject
        || context.authority.account_mode != "paper"
    {
        return Err("broker_v2_owner_mismatch".into());
    }
    let state = source.state()?.ok_or("broker_v2_state_missing")?;
    if state.original_key == context.idempotency_key.as_str() {
        return Err("broker_v2_distinct_recovery_key_required".into());
    }
    let original: PluginOperationRequest =
        serde_json::from_value(state.intent["originalRequest"].clone())
            .map_err(|_| "broker_v2_original_request_invalid")?;
    let binding: BrokerPreparedBinding =
        serde_json::from_value(state.intent["preparedBinding"].clone())
            .map_err(|_| "broker_v2_original_binding_invalid")?;
    let envelope = original_envelope(state.intent.clone(), &original, &state.original_key)?;
    let original_context =
        SideEffectContext::new(envelope.authority.clone(), envelope.idempotency_key.clone());
    use order_plans::submission::Phase;
    if matches!(state.phase, Phase::Admitting | Phase::Admitted) {
        source.abort_unsent(&original_context)?;
        return Ok(RecoveryPlan {
            lookup: None,
            original,
            binding,
            original_key: state.original_key,
            envelope,
            kind: RecoveryKind::KnownNotSent,
        });
    }
    if state.phase == Phase::Denied {
        return Ok(RecoveryPlan {
            lookup: None,
            original,
            binding,
            original_key: state.original_key,
            envelope,
            kind: RecoveryKind::KnownNotSent,
        });
    }
    crate::finance_authority::validate_stored_broker_authorization(
        deps.storage.as_ref(),
        &envelope,
    )?;
    if matches!(
        state.phase,
        Phase::Rejected | Phase::Cancelled | Phase::Consumed
    ) {
        return Ok(RecoveryPlan {
            lookup: None,
            original,
            binding,
            original_key: state.original_key,
            envelope,
            kind: RecoveryKind::Historical,
        });
    }
    source
        .service()
        .require_object("plugin_instance", &original.plugin_instance_ref)
        .map_err(|_| "broker_v2_observer_source_not_owned")?;
    let selected = json!({"pluginInstanceRef":original.plugin_instance_ref,"pluginRef":original.plugin_ref,"manifestFingerprint":original.manifest_fingerprint,"accountRef":original.account_ref});
    let account = account_identity::observer(
        deps.plugins.as_ref(),
        deps.plugin_packages.as_ref(),
        deps.credentials.as_ref(),
        &selected,
        deps.clock.trusted_now_ms()?,
        "paper",
    )?;
    if serde_json::to_value(&account.identity).map_err(|_| "broker_v2_account_invalid")?
        != state.intent["canonicalAccount"]
    {
        return Err("broker_v2_canonical_account_mismatch".into());
    }
    let instance = deps
        .plugins
        .get_instance(&original.plugin_instance_ref)?
        .ok_or("broker_v2_observer_source_missing")?;
    let current = BrokerPreparedBinding {
        plugin_instance_ref: original.plugin_instance_ref.clone(),
        plugin_ref: original.plugin_ref.clone(),
        package_sha256: instance["activePackageSha256"]
            .as_str()
            .ok_or("broker_v2_observer_binding_invalid")?
            .into(),
        manifest_fingerprint: original.manifest_fingerprint.clone(),
        configuration_digest: crate::spec::canonical_hash(&instance["configuration"])?,
        credential_ref: instance["credentialRef"]
            .as_str()
            .ok_or("broker_v2_observer_binding_invalid")?
            .into(),
        credential_generation: crate::live_execution_checks::credential_revision(&instance),
    };
    if current != binding {
        return Err("broker_v2_observer_binding_changed".into());
    }
    let status = deps.credentials.status(&current.credential_ref);
    let instance = crate::live_execution_checks::apply_current_credential_status(instance, &status);
    let manifest = deps
        .plugins
        .get_manifest(&original.plugin_ref)?
        .ok_or("broker_v2_observer_source_missing")?;
    let recovery = recovery_lookup(&manifest["manifest"], &original.operation_id)?;
    let entry =
        crate::plugin_catalog::stored_plugin_entry(&instance, &manifest, &status, "ready".into())
            .ok_or("broker_v2_observer_binding_invalid")?;
    let order: wire::SubmitRequest = serde_json::from_value(original.input.clone())
        .map_err(|_| "broker_v2_original_request_invalid")?;
    let resolution = deps.capability_resolver.resolve(
        CapabilityRequirement {
            requirement_id: "broker-v2-recovery".into(),
            capability: recovery.capability().into(),
            mode: "paper".into(),
            plugin_instance_ref: Some(original.plugin_instance_ref.clone()),
            plugin_ref: Some(original.plugin_ref.clone()),
            operation_id: Some(recovery.operation().into()),
            account_ref: original.account_ref.clone(),
            purpose: Some("recovery".into()),
            asset_class: Some(order.order.instrument.asset_class.clone()),
            instrument_family: Some(order.order.instrument.instrument_family.clone()),
            fallback_policy: "deny".into(),
            ..Default::default()
        },
        CapabilityResolverCatalog {
            plugins: vec![entry],
            entitlements: stored_entitlements(deps.storage.as_ref())?,
        },
    );
    if !resolution.ok {
        return Err("broker_v2_lookup_permission_unavailable".into());
    }
    let mut lookup = original.clone();
    lookup.correlation_id = format!(
        "broker-recovery-v2:{}",
        key_hash(context.idempotency_key.as_str())
    );
    lookup.operation_id = recovery.operation().into();
    lookup.capability = recovery.capability().into();
    lookup.purpose = "recovery".into();
    lookup.input = recovery_input(recovery, &order, &account.identity)?;
    lookup.evidence_refs = vec![format!("original-broker-intent:{}", state.intent_sha256)];
    Ok(RecoveryPlan {
        lookup: Some(lookup),
        original,
        binding,
        original_key: state.original_key,
        envelope,
        kind: RecoveryKind::Observed,
    })
}
pub(crate) fn observation_from_response(
    response: &PluginOperationResponse,
    source: &OrderPlanSource,
    now: i64,
) -> Result<OrderObservation, String> {
    if response.schema_ref != wire::OBSERVATION_SCHEMA || response.reconciliation_required {
        return Err("broker_v2_lookup_inconclusive".into());
    }
    let o: OrderObservation = serde_json::from_value(response.payload.clone())
        .map_err(|_| "broker_v2_observation_invalid")?;
    o.validate(now)?;
    let p = source.historical()?;
    if o.order != p.order
        || Some(o.canonical_account_sha256.as_str())
            != p.canonical_account
                .as_ref()
                .map(|a| a.identity_sha256.as_str())
    {
        return Err("broker_v2_observation_binding_mismatch".into());
    }
    Ok(o)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutionWindowConstraint {
    schema_version: String,
    scope: crate::service::data_plans::PlanScope,
    occurrence_id: String,
    schedule_receipt_sha256: String,
    open_at_ms: i64,
    close_at_ms: i64,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManagementWindowConstraint {
    schema_version: String,
    scope: crate::service::data_plans::PlanScope,
    window_id: String,
    window_receipt_sha256: String,
    position_id: String,
    original_entry_plan_id: String,
    capability_graph_revision: String,
    open_at_ms: i64,
    close_at_ms: i64,
}
fn validate_management_window_constraint(
    value: &Value,
    scope: &crate::service::data_plans::PlanScope,
    now: i64,
    managed: Option<(
        &tradeassembly_plugin_sdk::execution::PositionAction,
        &crate::service::order_plans::management::CloseBinding,
        &str,
    )>,
) -> Result<(), String> {
    let (action, binding, graph) = managed.ok_or("broker_v2_execution_window_invalid")?;
    let w: ManagementWindowConstraint =
        serde_json::from_value(value.clone()).map_err(|_| "broker_v2_execution_window_invalid")?;
    if w.schema_version != "tradeassembly.management_window_constraint.v1"
        || action != &tradeassembly_plugin_sdk::execution::PositionAction::SellToClose
        || w.scope != *scope
        || w.window_id != format!("management_window_{}", w.window_receipt_sha256)
        || w.window_id != binding.execution_occurrence_id
        || w.position_id != binding.position_id
        || w.original_entry_plan_id != binding.original_entry_plan_id
        || w.capability_graph_revision != graph
        || w.window_receipt_sha256.len() != 64
        || !w
            .window_receipt_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || w.open_at_ms <= 0
        || w.close_at_ms <= w.open_at_ms
        || w.close_at_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
    {
        return Err("broker_v2_execution_window_invalid".into());
    }
    if now < w.open_at_ms || now >= w.close_at_ms {
        return Err("broker_v2_outside_execution_window".into());
    }
    Ok(())
}
fn validate_execution_window(
    value: &Value,
    scope: &crate::service::data_plans::PlanScope,
    now: i64,
    managed: Option<(
        &tradeassembly_plugin_sdk::execution::PositionAction,
        &crate::service::order_plans::management::CloseBinding,
        &str,
    )>,
) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    if value["schemaVersion"] == "tradeassembly.management_window_constraint.v1" {
        return validate_management_window_constraint(value, scope, now, managed);
    }
    let window: ExecutionWindowConstraint =
        serde_json::from_value(value.clone()).map_err(|_| "broker_v2_execution_window_invalid")?;
    if window.schema_version != "tradeassembly.execution_window_constraint.v1"
        || window.scope != *scope
        || !window.occurrence_id.starts_with("occurrence_")
        || window.schedule_receipt_sha256.len() != 64
        || !window
            .schedule_receipt_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || window.open_at_ms <= 0
        || window.close_at_ms <= window.open_at_ms
        || window.close_at_ms > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
    {
        return Err("broker_v2_execution_window_invalid".into());
    }
    if now < window.open_at_ms || now >= window.close_at_ms {
        return Err("broker_v2_outside_execution_window".into());
    }
    Ok(())
}
#[cfg(test)]
mod lookup_contract_tests {
    use super::*;

    fn manifest(dependencies: Value) -> Value {
        json!({"operations":[
            {"id":SUBMIT_OPERATION,"capability":SUBMIT_CAPABILITY,"effect":"write","dependencies":dependencies},
            {"id":LOOKUP_OPERATION,"capability":LOOKUP_CAPABILITY,"effect":"read"},
            {"id":lookup_v3::OPERATION_ID,"capability":lookup_v3::CAPABILITY,"effect":"read"}
        ]})
    }
    fn original() -> (
        wire::SubmitRequest,
        account_identity::CanonicalAccountIdentity,
    ) {
        let expected = lookup_v3::ExpectedPaperAccount {
            broker_namespace: "fixture.broker".into(),
            environment: lookup_v3::PaperEnvironment::Paper,
            provider_account_id: "fixture.account".into(),
        };
        let digest = expected.canonical_account_sha256().unwrap();
        let account = account_identity::CanonicalAccountIdentity {
            schema_version: "tradeassembly.canonical_account.v1".into(),
            broker_namespace: expected.broker_namespace,
            environment: "paper".into(),
            provider_account_id: expected.provider_account_id,
            identity_sha256: digest.clone(),
        };
        let request = serde_json::from_value(json!({"schemaVersion":"tradeassembly.broker_submit.v2","planId":format!("orderplan_{}","a".repeat(64)),"canonicalAccountSha256":digest,"order":{
            "schemaVersion":"tradeassembly.broker_order.v2","instrument":{"instrumentId":"host.opaque.instrument","providerSymbol":"SPY","assetClass":"equity","instrumentFamily":"equity","quantityUnit":"shares","multiplier":"1","quoteCurrency":"USD"},
            "positionAction":"buy_to_open","quantity":"0.4","pricing":{"kind":"limit","limitPrice":"100"},"timeInForce":"day","clientOrderId":"fixture.client"
        }})).unwrap();
        (request, account)
    }
    #[test]
    fn lookup_v3_selection_is_explicit_and_never_falls_back_by_availability() {
        assert_eq!(
            recovery_lookup(&manifest(json!([])), SUBMIT_OPERATION).unwrap(),
            RecoveryLookup::LegacyV2
        );
        let declaration = json!({"dependencyId":lookup_v3::DEPENDENCY_ID,"capability":lookup_v3::CAPABILITY,"operationId":lookup_v3::OPERATION_ID,"required":true,"traits":{}});
        assert_eq!(
            recovery_lookup(&manifest(json!([declaration.clone()])), SUBMIT_OPERATION).unwrap(),
            RecoveryLookup::OriginalOrderV3
        );
        let mut missing = manifest(json!([declaration.clone()]));
        missing["operations"].as_array_mut().unwrap().pop();
        assert!(
            recovery_lookup(&missing, SUBMIT_OPERATION).is_err(),
            "declared V3 must not fall back to existing V2"
        );
        for field in ["required", "capability", "operationId"] {
            let mut bad = declaration.clone();
            bad[field] = if field == "required" {
                json!(false)
            } else {
                json!("unknown")
            };
            assert!(recovery_lookup(&manifest(json!([bad])), SUBMIT_OPERATION).is_err());
        }
        assert!(recovery_lookup(
            &manifest(json!([declaration.clone(), declaration])),
            SUBMIT_OPERATION
        )
        .is_err());
    }
    #[test]
    fn lookup_v3_joins_exact_original_host_order_and_account_and_preserves_v2_wire() {
        let (original, account) = original();
        let legacy = recovery_input(RecoveryLookup::LegacyV2, &original, &account).unwrap();
        assert_eq!(
            legacy,
            json!({"schemaVersion":"tradeassembly.broker_lookup.v2","canonicalAccountSha256":original.canonical_account_sha256,"clientOrderId":original.order.client_order_id})
        );
        let payload = recovery_input(RecoveryLookup::OriginalOrderV3, &original, &account).unwrap();
        let decoded =
            lookup_v3::decode_lookup_json(&serde_json::to_vec(&payload).unwrap()).unwrap();
        assert_eq!(decoded.original_submit, original);
        assert_eq!(
            decoded.expected_account.provider_account_id,
            account.provider_account_id
        );
        let mut changed = account.clone();
        changed.provider_account_id = "another-account".into();
        assert!(recovery_input(RecoveryLookup::OriginalOrderV3, &original, &changed).is_err());
        changed = account;
        changed.environment = "live".into();
        assert!(recovery_input(RecoveryLookup::OriginalOrderV3, &original, &changed).is_err());
    }
    #[test]
    fn lookup_v3_actual_schema_accepts_bound_request_and_refuses_control_labels() {
        let schema = lookup_v3::request_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let (original, account) = original();
        let good = recovery_input(RecoveryLookup::OriginalOrderV3, &original, &account).unwrap();
        assert!(validator.is_valid(&good));
        let label = jsonschema::validator_for(
            &schema["properties"]["expectedAccount"]["properties"]["providerAccountId"],
        )
        .unwrap();
        assert!(label.is_valid(&json!("normal.account-id")));
        for bad in ["line\nbreak", "c1\u{0085}control", "null\0byte"] {
            assert!(!label.is_valid(&json!(bad)));
        }
        let mut extra = good;
        extra["submittedAtMs"] = json!(1);
        assert!(!validator.is_valid(&extra));
    }
}

#[cfg(test)]
mod settlement_observation_contract_tests {
    use serde_json::Value;
    use tradeassembly_plugin_sdk::{broker_lookup_v3, settlement_observation_v2 as wire};

    #[test]
    fn settlement_observation_v2_actual_schema_matches_shared_cases() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../plugin-sdk/tests/fixtures/settlement_observation_v2");
        let cases: Vec<Value> =
            serde_json::from_slice(&std::fs::read(fixtures.join("cases.json")).unwrap()).unwrap();
        let request = wire::request_schema();
        let response = wire::response_schema();
        let request_validator = jsonschema::validator_for(&request).unwrap();
        let response_validator = jsonschema::validator_for(&response).unwrap();
        for case in &cases {
            let document: Value = serde_json::from_slice(
                &std::fs::read(fixtures.join(case["file"].as_str().unwrap())).unwrap(),
            )
            .unwrap();
            let validator = if case["schema"] == "request" {
                &request_validator
            } else {
                &response_validator
            };
            let errors = validator
                .iter_errors(&document)
                .map(|e| e.to_string())
                .collect::<Vec<_>>();
            assert_eq!(
                errors.is_empty(),
                case["schemaValid"].as_bool().unwrap(),
                "case {} structural-schema mismatch: {errors:?}",
                case["file"]
            );
        }
        if let Some(directory) = std::env::var_os("CORE_PROVIDER_CONTRACT_EXPORT") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir(&directory).unwrap();
            for (name, schema) in [
                ("lookup-v3-request.json", broker_lookup_v3::request_schema()),
                ("settlement-v2-request.json", request),
                ("settlement-v2-response.json", response),
            ] {
                let digest =
                    tradeassembly_plugin_sdk::canonical::canonical_sha256(&schema).unwrap();
                std::fs::write(
                    directory.join(name),
                    serde_json::to_vec_pretty(&schema).unwrap(),
                )
                .unwrap();
                eprintln!("provider-contract {name} canonicalSha256={digest}");
            }
        }
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    #[test]
    fn scheduled_window_is_checked_at_final_c5_time_and_legacy_stays_optional() {
        let scope = crate::service::data_plans::PlanScope {
            strategy_id: "s".into(),
            strategy_version_id: "v".into(),
            strategy_spec_hash: "h".into(),
        };
        let window = json!({"schemaVersion":"tradeassembly.execution_window_constraint.v1","scope":scope,"occurrenceId":"occurrence_fixture","scheduleReceiptSha256":"a".repeat(64),"openAtMs":1000,"closeAtMs":2000});
        for now in [999, 2000] {
            assert_eq!(
                validate_execution_window(&window, &scope, now, None).unwrap_err(),
                "broker_v2_outside_execution_window"
            );
        }
        assert!(validate_execution_window(&window, &scope, 1000, None).is_ok());
        assert!(validate_execution_window(&Value::Null, &scope, 2000, None).is_ok());
    }
    #[test]
    fn management_window_is_only_for_exact_owned_sell_close_binding() {
        use tradeassembly_plugin_sdk::execution::PositionAction;
        let scope = crate::service::data_plans::PlanScope {
            strategy_id: "s".into(),
            strategy_version_id: "v".into(),
            strategy_spec_hash: "h".into(),
        };
        let binding = crate::service::order_plans::management::CloseBinding {
            execution_occurrence_id: format!("management_window_{}", "a".repeat(64)),
            position_id: "position_1".into(),
            trigger_sha256: "c".repeat(64),
            exit_policy_sha256: "d".repeat(64),
            original_entry_plan_id: "entry_1".into(),
        };
        let w = json!({"schemaVersion":"tradeassembly.management_window_constraint.v1","scope":scope,"windowId":binding.execution_occurrence_id,"windowReceiptSha256":"a".repeat(64),"positionId":"position_1","originalEntryPlanId":"entry_1","capabilityGraphRevision":"graph_1","openAtMs":1000,"closeAtMs":2000});
        let close = Some((&PositionAction::SellToClose, &binding, "graph_1"));
        assert!(validate_execution_window(&w, &scope, 1000, close).is_ok());
        for now in [999, 2000] {
            assert_eq!(
                validate_execution_window(&w, &scope, now, close).unwrap_err(),
                "broker_v2_outside_execution_window"
            );
        }
        assert_eq!(
            validate_execution_window(&w, &scope, 1000, None).unwrap_err(),
            "broker_v2_execution_window_invalid"
        );
        assert_eq!(
            validate_execution_window(
                &w,
                &scope,
                1000,
                Some((&PositionAction::BuyToOpen, &binding, "graph_1"))
            )
            .unwrap_err(),
            "broker_v2_execution_window_invalid"
        );
        for field in ["occurrenceId", "scheduleReceiptSha256", "quantity"] {
            let mut bad = w.clone();
            bad[field] = json!(1);
            assert_eq!(
                validate_execution_window(&bad, &scope, 1000, close).unwrap_err(),
                "broker_v2_execution_window_invalid"
            );
        }
        for (field, value) in [
            ("positionId", json!("position_2")),
            ("originalEntryPlanId", json!("entry_2")),
            ("capabilityGraphRevision", json!("graph_2")),
        ] {
            let mut bad = w.clone();
            bad[field] = value;
            assert_eq!(
                validate_execution_window(&bad, &scope, 1000, close).unwrap_err(),
                "broker_v2_execution_window_invalid"
            );
        }
        let mut wrong = w.clone();
        wrong["windowId"] = json!(format!("management_window_{}", "b".repeat(64)));
        wrong["windowReceiptSha256"] = json!("b".repeat(64));
        assert!(validate_execution_window(&wrong, &scope, 1000, close).is_err());
        let mut old = w;
        old["schemaVersion"] = json!("tradeassembly.execution_window_constraint.v1");
        assert!(validate_execution_window(&old, &scope, 1000, close).is_err());
    }
}
