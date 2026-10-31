//! Versioned V2 admission through the same C5 authority and trusted run loader.
use super::*;
use crate::ports::broker_submission::{BrokerPreparedBinding, BrokerSubmissionPermit};
use crate::{control_plane::ControlPlaneCommandEnvelope, ports::*, service::order_plans};
pub use order_plans::submission::OrderPlanSource;
use serde_json::json;
use sha2::{Digest, Sha256};
use tradeassembly_plugin_sdk::broker_v2::{self as wire, OrderObservation, OrderStatus};
pub const SUBMIT_OPERATION: &str = "broker.paper_order_submit_v2";
pub const SUBMIT_CAPABILITY: &str = "broker.order_submit.paper@2";
pub const LOOKUP_OPERATION: &str = "broker.order_lookup.paper_v2";
pub const LOOKUP_CAPABILITY: &str = "broker.order_lookup.paper@2";
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
    if !is_submission(request) {
        return Err("broker_v2_paper_context_required".into());
    }
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
    let state = order_plans::measured("broker.load_current_state", || {
        load_current_state(deps, request, context)
    })?;
    validate_prepared_binding(&state, prepared)?;
    let signal = source.verified_signal()?;
    if state.config["orchestrator"] == "typed_pipeline" {
        let selection = order_plans::decision::check_plan_with_signal(
            source.service(),
            &p,
            &state.run,
            &state.config,
            &signal,
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

    if state.config["orderPlanPolicySha256"]
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
    if !state.package.manifest["operations"]
        .as_array()
        .is_some_and(|ops| {
            ops.iter().any(|o| {
                o["id"] == LOOKUP_OPERATION
                    && o["capability"] == LOOKUP_CAPABILITY
                    && o["effect"] == "read"
            })
        })
    {
        return Err("broker_v2_lookup_operation_missing".into());
    }
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
            capability: LOOKUP_CAPABILITY.into(),
            mode: "paper".into(),
            plugin_instance_ref: Some(request.plugin_instance_ref.clone()),
            plugin_ref: Some(request.plugin_ref.clone()),
            operation_id: Some(LOOKUP_OPERATION.into()),
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
        source.snapshot_with_signal(&signal)
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
    Ok((state, p, intent))
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
fn original_envelope(
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
    if serde_json::to_value(account.identity).map_err(|_| "broker_v2_account_invalid")?
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
    let entry =
        crate::plugin_catalog::stored_plugin_entry(&instance, &manifest, &status, "ready".into())
            .ok_or("broker_v2_observer_binding_invalid")?;
    let order: wire::SubmitRequest = serde_json::from_value(original.input.clone())
        .map_err(|_| "broker_v2_original_request_invalid")?;
    let resolution = deps.capability_resolver.resolve(
        CapabilityRequirement {
            requirement_id: "broker-v2-recovery".into(),
            capability: LOOKUP_CAPABILITY.into(),
            mode: "paper".into(),
            plugin_instance_ref: Some(original.plugin_instance_ref.clone()),
            plugin_ref: Some(original.plugin_ref.clone()),
            operation_id: Some(LOOKUP_OPERATION.into()),
            account_ref: original.account_ref.clone(),
            purpose: Some("recovery".into()),
            asset_class: Some(order.order.instrument.asset_class),
            instrument_family: Some(order.order.instrument.instrument_family),
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
    lookup.operation_id = LOOKUP_OPERATION.into();
    lookup.capability = LOOKUP_CAPABILITY.into();
    lookup.purpose = "recovery".into();
    lookup.input = json!({"schemaVersion":"tradeassembly.broker_lookup.v2","canonicalAccountSha256":state.intent["canonicalAccount"]["identitySha256"],"clientOrderId":state.intent["order"]["clientOrderId"]});
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
