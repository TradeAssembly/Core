//! Source-bound non-atomic observations. This ledger never credits cash or
//! releases a reservation; coherent V1 reconciliation remains a separate proof.
use super::*;
use crate::ports::broker_submission::BrokerPreparedBinding;
use tradeassembly_plugin_sdk::{broker_lookup_v3, broker_v2, settlement_observation_v2 as wire};
#[cfg(test)]
mod managed_entry_guard_tests;
mod reducer;
const HEADS: &str = "execution_provider_observation_heads_v2";
const RECEIPTS: &str = "execution_provider_observation_receipts_v2";
const KEYS: &str = "execution_provider_observation_keys_v2";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Head {
    schema_version: String,
    plan_id: String,
    owner_sha256: String,
    receipt_sha256: String,
    projection: Option<reducer::Projection>,
    blocked_reason: Option<String>,
    head_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    schema_version: String,
    plan_id: String,
    owner_sha256: String,
    account_key: String,
    original_intent_sha256: String,
    request: wire::Request,
    source_binding: BrokerPreparedBinding,
    response_sha256: String,
    observation: wire::Observation,
    previous_receipt_sha256: Option<String>,
    refused_reason: Option<String>,
    projection: Option<reducer::Projection>,
    blockers: Vec<reducer::Blocker>,
    acquired_at_ms: i64,
    receipt_sha256: String,
}
fn seal_head(h: &mut Head) -> Result<(), String> {
    h.head_sha256.clear();
    h.head_sha256 = hash(h)?;
    Ok(())
}
fn seal_receipt(r: &mut Receipt) -> Result<(), String> {
    r.receipt_sha256.clear();
    r.receipt_sha256 = hash(r)?;
    Ok(())
}
fn role(p: &OrderPlan) -> wire::Role {
    if p.management.is_some() {
        wire::Role::Close
    } else {
        wire::Role::Entry
    }
}
fn dependency(role: wire::Role) -> &'static str {
    match role {
        wire::Role::Entry => "broker.cash_settlement",
        wire::Role::Close => "broker.close_settlement",
    }
}
pub(super) fn selected_manifest(
    manifest: &Value,
    operation: &str,
    role: wire::Role,
) -> Result<bool, String> {
    let operations = manifest["operations"]
        .as_array()
        .ok_or("provider_observation_operations_missing")?;
    let submits = operations
        .iter()
        .filter(|o| o["id"] == operation)
        .collect::<Vec<_>>();
    let [submit] = submits.as_slice() else {
        return Err("provider_observation_submit_binding_invalid".into());
    };
    if submit["capability"] != crate::broker_submission::v2::SUBMIT_CAPABILITY {
        return Err("provider_observation_submit_binding_invalid".into());
    }
    let deps = submit["dependencies"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let declared = deps
        .iter()
        .filter(|d| d["dependencyId"] == dependency(role))
        .collect::<Vec<_>>();
    if declared.is_empty() {
        return Ok(false);
    }
    if declared.len() != 1
        || declared[0]["required"] != true
        || declared[0]["operationId"] != role.operation_id()
        || declared[0]["capability"] != role.capability()
    {
        return Err("provider_observation_dependency_invalid".into());
    }
    if operations
        .iter()
        .filter(|o| {
            o["id"] == role.operation_id()
                && o["capability"] == role.capability()
                && o["effect"] == "read"
                && o["purpose"] == "recovery"
        })
        .count()
        != 1
    {
        return Err("provider_observation_operation_missing".into());
    }
    Ok(true)
}
pub(super) fn selected(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    r: &submission::SubmissionRecord,
) -> Result<bool, String> {
    let binding: BrokerPreparedBinding =
        serde_json::from_value(r.intent["preparedBinding"].clone())
            .map_err(|_| "provider_observation_source_invalid")?;
    let package = s
        .runtime()
        .plugin_packages
        .get(&binding.package_sha256)?
        .ok_or("provider_observation_package_missing")?;
    selected_manifest(
        &package.manifest,
        r.intent["originalRequest"]["operationId"]
            .as_str()
            .ok_or("provider_observation_submit_missing")?,
        role(p),
    )
}
fn observable_phase(phase: &submission::Phase) -> Result<(), String> {
    if matches!(
        phase,
        submission::Phase::Sending
            | submission::Phase::Unknown
            | submission::Phase::Open
            | submission::Phase::PartiallyFilled
            | submission::Phase::Consumed
            | submission::Phase::Rejected
            | submission::Phase::Cancelled
    ) {
        Ok(())
    } else {
        Err("provider_observation_dispatch_not_established".into())
    }
}
fn read_receipt(s: &TradeAssemblyService, p: &OrderPlan, id: &str) -> Result<Receipt, String> {
    let mut r: Receipt = serde_json::from_value(
        s.runtime()
            .storage
            .get_json(RECEIPTS, id)?
            .ok_or("provider_observation_receipt_missing")?,
    )
    .map_err(|_| "provider_observation_receipt_invalid")?;
    let expected = r.receipt_sha256.clone();
    seal_receipt(&mut r)?;
    if expected != id
        || r.receipt_sha256 != id
        || r.schema_version != "tradeassembly.provider_observation_receipt.v2"
        || r.owner_sha256 != owner(s)?
        || r.plan_id != p.plan_id
        || r.account_key != p.account_key
    {
        return Err("provider_observation_receipt_invalid".into());
    }
    r.observation.validate(&r.request, r.acquired_at_ms)?;
    let (_, record) = submission::record(s, p)?.ok_or("broker_v2_state_missing")?;
    observable_phase(&record.phase)?;
    if r.original_intent_sha256 != record.intent_sha256
        || value(&r.source_binding)? != record.intent["preparedBinding"]
        || r.observation.order.order != p.order
        || r.request != request(p, &record)?
    {
        return Err("provider_observation_original_intent_mismatch".into());
    }
    Ok(r)
}
fn head(s: &TradeAssemblyService, p: &OrderPlan) -> Result<Option<(Value, Head)>, String> {
    let Some(raw) = s.runtime().storage.get_json(HEADS, &p.plan_id)? else {
        return Ok(None);
    };
    let mut h: Head =
        serde_json::from_value(raw.clone()).map_err(|_| "provider_observation_head_invalid")?;
    let expected = h.head_sha256.clone();
    seal_head(&mut h)?;
    if h.head_sha256 != expected
        || h.schema_version != "tradeassembly.provider_observation_head.v2"
        || h.plan_id != p.plan_id
        || h.owner_sha256 != owner(s)?
    {
        return Err("provider_observation_head_invalid".into());
    }
    read_receipt(s, p, &h.receipt_sha256)?;
    Ok(Some((raw, h)))
}
fn request(p: &OrderPlan, r: &submission::SubmissionRecord) -> Result<wire::Request, String> {
    let original: broker_v2::SubmitRequest =
        serde_json::from_value(r.intent["originalRequest"]["input"].clone())
            .map_err(|_| "provider_observation_original_order_invalid")?;
    if original.plan_id != p.plan_id
        || original.order != p.order
        || Some(original.canonical_account_sha256.as_str())
            != p.canonical_account
                .as_ref()
                .map(|a| a.identity_sha256.as_str())
    {
        return Err("provider_observation_original_order_invalid".into());
    }

    let identity = p
        .canonical_account
        .as_ref()
        .ok_or("provider_observation_account_missing")?;
    let q=wire::Request{schema_version:wire::REQUEST_VERSION.into(),role:role(p),lookup:broker_lookup_v3::LookupRequestV3{schema_version:broker_lookup_v3::VERSION.into(),original_submit:original,expected_account:serde_json::from_value(json!({"brokerNamespace":identity.broker_namespace,"environment":identity.environment,"providerAccountId":identity.provider_account_id})).map_err(|_|"provider_observation_account_invalid")?},host_account_ref:r.intent["accountRef"].as_str().ok_or("provider_observation_account_missing")?.into()};
    q.validate()?;
    Ok(q)
}
fn require_read(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    r: &submission::SubmissionRecord,
    b: &BrokerPreparedBinding,
) -> Result<(), String> {
    if !selected(s, p, r)? {
        return Err("provider_observation_explicit_binding_required".into());
    }
    settlement::require_read_operation(
        s,
        p,
        b,
        &r.intent["accountRef"],
        role(p).operation_id(),
        role(p).capability(),
        "recovery",
    )
}
fn summary(r: &Receipt) -> Value {
    json!({"schemaVersion":r.schema_version,"planId":r.plan_id,"role":r.request.role,"receiptSha256":r.receipt_sha256,"status":if r.refused_reason.is_some(){"blocked"}else{match r.observation.status{wire::ProviderObservationStatus::Pending=>"pending",wire::ProviderObservationStatus::Provisional=>"provisional"}},"blockedReason":r.refused_reason,"observation":r.observation,"projection":r.projection,"blockers":r.blockers,"observedAtMs":r.acquired_at_ms,"historicalOutcome":true,"cashReconciled":false,"releasedHold":"0","executionAuthorityGranted":false})
}
/// Positive selection and current read authority, without invoking a provider.
pub(super) fn preflight(
    s: &TradeAssemblyService,
    p: &OrderPlan,
) -> Result<(submission::SubmissionRecord, BrokerPreparedBinding), String> {
    let (_, record) = submission::record(s, p)?.ok_or("broker_v2_state_missing")?;
    observable_phase(&record.phase)?;
    if !selected(s, p, &record)? {
        return Err("provider_observation_explicit_selection_required".into());
    }
    request(p, &record)?.validate()?;
    let binding = settlement::current_source(s, p, &record)?;
    require_read(s, p, &record, &binding)?;
    Ok((record, binding))
}
pub(super) fn historical_entry_basis(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    id: &str,
) -> Result<(Decimal, Decimal), String> {
    let r = read_receipt(s, p, id)?;
    if r.refused_reason.is_some()
        || r.request.role != wire::Role::Entry
        || !matches!(
            r.observation.order.status,
            broker_v2::OrderStatus::Filled
                | broker_v2::OrderStatus::Cancelled
                | broker_v2::OrderStatus::Expired
        )
    {
        return Err("provider_observation_terminal_entry_required".into());
    }
    let quantity = *r.observation.order.filled_quantity.required()?;
    let basis = r.observation.average_fill_price.required()?.price;
    if !quantity.positive() {
        return Err("provider_observation_entry_basis_changed".into());
    }
    Ok((quantity, basis))
}
fn project_receipts(
    r: Receipt,
    h: Receipt,
) -> Result<management::provider_remote::ObservationProjection, String> {
    use management::provider_remote::{
        AcceptedObservationProjection, ReportedMoney, ReportedObservation,
    };
    let accepted_projection = r
        .projection
        .as_ref()
        .map(|p| AcceptedObservationProjection {
            generation: p.generation,
            projection_sha256: p.projection_sha256.clone(),
            observation_sha256: p.observation_sha256.clone(),
            filled_quantity: p.filled_quantity,
            cash_debit: p.cash_debit,
            average_fill_price: p.average_fill_price.as_ref().map(|m| ReportedMoney {
                amount: m.amount,
                currency: m.currency.clone(),
            }),
            fee_cash: p.fee_cash.as_ref().map(|m| ReportedMoney {
                amount: m.amount,
                currency: m.currency.clone(),
            }),
            fee_finality: p.fee_finality,
        });
    let blockers = serde_json::to_value(&r.blockers)
        .map_err(|_| "provider_observation_projection_invalid")?
        .as_array()
        .ok_or("provider_observation_projection_invalid")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "provider_observation_projection_invalid".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(management::provider_remote::ObservationProjection {
        receipt_sha256: r.receipt_sha256,
        plan_id: r.plan_id,
        role: r.request.role,
        original_intent_sha256: r.original_intent_sha256,
        source_binding_sha256: hash(&r.source_binding)?,
        provider_response_sha256: r.response_sha256,
        observation: ReportedObservation::from_observation(&r.observation)?,
        accepted_projection,
        blockers,
        blocked_reason: r.refused_reason,
        head_receipt_sha256: h.receipt_sha256,
        head_blocked_reason: h.refused_reason,
        observed_at_ms: r.acquired_at_ms,
        cash_reconciled: false,
        released_hold: Decimal::ZERO,
    })
}
pub(super) fn project_original_receipt(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    id: &str,
) -> Result<management::provider_remote::ObservationProjection, String> {
    let r = read_receipt(s, p, id)?;
    project_receipts(r.clone(), r)
}
pub(super) fn verify_projection(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    v: &management::provider_remote::ObservationProjection,
) -> Result<(), String> {
    let expected = project_receipts(
        read_receipt(s, p, &v.receipt_sha256)?,
        read_receipt(s, p, &v.head_receipt_sha256)?,
    )?;
    if expected != *v {
        return Err("provider_observation_projection_invalid".into());
    }
    Ok(())
}
pub(super) fn project_latest(
    s: &TradeAssemblyService,
    p: &OrderPlan,
) -> Result<Option<management::provider_remote::ObservationProjection>, String> {
    head(s, p)?
        .map(|(_, h)| project_original_receipt(s, p, &h.receipt_sha256))
        .transpose()
}

pub(super) fn observe(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    if ctx.authority.account_mode != "paper" {
        return Err("provider_observation_paper_required".into());
    }
    let (record, binding) = preflight(s, p)?;
    let key = hash(&(owner(s)?, p.plan_id.clone(), ctx.idempotency_key.as_str()))?;
    if let Some(prior) = s.runtime().storage.get_json(KEYS, &key)? {
        let r = read_receipt(
            s,
            p,
            prior.as_str().ok_or("provider_observation_key_invalid")?,
        )?;
        return Ok(summary(&r));
    }
    let q = request(p, &record)?;
    let original: PluginOperationRequest =
        serde_json::from_value(record.intent["originalRequest"].clone())
            .map_err(|_| "provider_observation_original_request_invalid")?;
    let correlation = format!("provider-observation:{key}");
    let response = crate::service::providers::invoke_operation(
        s,
        &format!(
            "/plugins/instances/{}/operations/{}:invoke",
            binding.plugin_instance_ref,
            q.role.operation_id()
        ),
        json!({"strategyId":p.command.scope.strategy_id,"strategyVersionId":p.command.scope.strategy_version_id,"strategySpecHash":p.command.scope.strategy_spec_hash,"activationId":original.activation_id,"attemptId":original.attempt_id,"evaluationTickId":original.evaluation_tick_id,"fencingToken":original.fencing_token,"capabilityGraphRevisionId":original.capability_graph_revision_id,"capabilityGraphFingerprint":original.capability_graph_fingerprint,"mode":"paper","purpose":"recovery","accountRef":record.intent["accountRef"],"assetClass":p.order.instrument.asset_class,"instrumentFamily":p.order.instrument.instrument_family,"input":q,"idempotencyKey":correlation}),
    );
    if response.status != 200 {
        return Err("provider_observation_provider_unavailable".into());
    }
    let response: PluginOperationResponse =
        serde_json::from_value(response.body["response"].clone())
            .map_err(|_| "provider_observation_response_invalid")?;
    if response.correlation_id != correlation
        || response.schema_ref != wire::RESPONSE_SCHEMA
        || response.reconciliation_required
        || response.freshness_state != "fresh"
        || !jsonschema::validator_for(&wire::response_schema())
            .map_err(|_| "provider_observation_schema_invalid")?
            .is_valid(&response.payload)
    {
        return Err("provider_observation_response_invalid".into());
    }
    let o = wire::decode_observation_json(
        &serde_json::to_vec(&response.payload)
            .map_err(|_| "provider_observation_encoding_invalid")?,
        &q,
        s.runtime().clock.trusted_now_ms()?,
    )?;
    let now = s.runtime().clock.trusted_now_ms()?;
    if o.evidence
        .iter()
        .any(|e| now.saturating_sub(e.completed_at_ms) > p.policy.max_snapshot_age_ms)
    {
        return Err("provider_observation_read_stale".into());
    }

    if settlement::current_source(s, p, &record)? != binding {
        return Err("provider_observation_source_changed".into());
    }
    require_read(s, p, &record, &binding)?;
    commit(
        s,
        p,
        &record,
        Acquired {
            binding,
            request: q,
            response_hash: hash(&response)?,
            observation: o,
        },
        &key,
        ctx,
    )
}
struct Acquired {
    binding: BrokerPreparedBinding,
    request: wire::Request,
    response_hash: String,
    observation: wire::Observation,
}
fn commit(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    record: &submission::SubmissionRecord,
    acquired: Acquired,
    key: &str,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let Acquired {
        binding,
        request: q,
        response_hash,
        observation: o,
    } = acquired;
    observable_phase(&record.phase)?;
    let now = s.runtime().clock.trusted_now_ms()?;
    o.validate(&q, now)?;
    let prior = head(s, p)?;
    let next = reducer::reduce(
        prior.as_ref().and_then(|(_, h)| h.projection.as_ref()),
        &q,
        &o,
        now,
    );
    let next=next.and_then(|outcome|{
        if let Some(old)=&record.observation {
            let known_fill=matches!(&o.order.filled_quantity,Observed::Available{value} if value.compare(record.filled_quantity).is_ok_and(|c|!c.is_lt()));
            let debit_ok=match (&old.cash_spent,&o.order.cash_spent){(Observed::Available{value:before},Observed::Available{value:after})=>!after.compare(*before)?.is_lt(),(Observed::Available{value},Observed::Unavailable{..})=>!value.positive(),_=>true};
            let was_terminal=matches!(old.status,broker_v2::OrderStatus::Filled|broker_v2::OrderStatus::Cancelled|broker_v2::OrderStatus::Expired|broker_v2::OrderStatus::Rejected);
            if (record.filled_quantity.positive()&&!known_fill)||!debit_ok
                ||old.provider_order_id.as_ref().is_some_and(|id|o.order.provider_order_id.as_ref()!=Some(id))
                ||o.order.observed_at_ms<old.observed_at_ms||o.order.known_at_ms<old.known_at_ms
                ||(was_terminal&&old.status!=o.order.status){return Err("provider_observation_submission_facts_regressed".into())}
        }
        Ok(outcome)
    });
    let (projection, blocked_reason, blockers) = match next {
        Ok(next) => {
            if next.disposition == reducer::Disposition::Unchanged {
                let (raw, h) = prior.as_ref().ok_or("provider_observation_head_missing")?;
                let receipt = read_receipt(s, p, &h.receipt_sha256)?;
                let cash = s
                    .runtime()
                    .storage
                    .get_json(CASH, &p.account_key)?
                    .ok_or("provider_observation_reservation_missing")?;
                ledger_owned(s, cash.clone())?;
                if s.runtime().storage.put_json_batch(
                    &[StorageWrite::new(
                        KEYS,
                        key,
                        json!(receipt.receipt_sha256),
                        ctx.clone(),
                    )],
                    &[
                        StorageExpectation::new(KEYS, key, None),
                        StorageExpectation::new(HEADS, &p.plan_id, Some(raw.clone())),
                        StorageExpectation::new(CASH, &p.account_key, Some(cash)),
                        StorageExpectation::new(
                            submission::STATE,
                            &p.plan_id,
                            Some(value(record)?),
                        ),
                    ],
                )? != ComparePutOutcome::Updated
                {
                    return Err("provider_observation_concurrent_change".into());
                }
                return Ok(summary(&receipt));
            }
            (Some(next.projection), None, next.blockers)
        }
        Err(error) => (
            prior.as_ref().and_then(|(_, h)| h.projection.clone()),
            Some(error),
            vec![reducer::Blocker::CoherentPostingUnavailable],
        ),
    };
    let mut receipt = Receipt {
        schema_version: "tradeassembly.provider_observation_receipt.v2".into(),
        plan_id: p.plan_id.clone(),
        owner_sha256: owner(s)?,
        account_key: p.account_key.clone(),
        original_intent_sha256: record.intent_sha256.clone(),
        request: q,
        source_binding: binding,
        response_sha256: response_hash,
        observation: o,
        previous_receipt_sha256: prior.as_ref().map(|(_, h)| h.receipt_sha256.clone()),
        refused_reason: blocked_reason.clone(),
        projection: projection.clone(),
        blockers,
        acquired_at_ms: now,
        receipt_sha256: String::new(),
    };
    seal_receipt(&mut receipt)?;
    let mut h = Head {
        schema_version: "tradeassembly.provider_observation_head.v2".into(),
        plan_id: p.plan_id.clone(),
        owner_sha256: owner(s)?,
        receipt_sha256: receipt.receipt_sha256.clone(),
        projection,
        blocked_reason,
        head_sha256: String::new(),
    };
    seal_head(&mut h)?;
    let old_cash = s
        .runtime()
        .storage
        .get_json(CASH, &p.account_key)?
        .ok_or("provider_observation_reservation_missing")?;
    let ledger = ledger_owned(s, old_cash.clone())?;
    if record.retained_cash.positive()
        && ledger.reservations.get(&p.plan_id) != Some(&record.retained_cash)
    {
        return Err("provider_observation_reservation_mismatch".into());
    }
    let mut writes = vec![
        StorageWrite::new(
            RECEIPTS,
            &receipt.receipt_sha256,
            value(&receipt)?,
            ctx.clone(),
        ),
        StorageWrite::new(HEADS, &p.plan_id, value(&h)?, ctx.clone()),
        StorageWrite::new(KEYS, key, json!(receipt.receipt_sha256), ctx.clone()),
    ];
    let mut expects = vec![
        StorageExpectation::new(RECEIPTS, &receipt.receipt_sha256, None),
        StorageExpectation::new(HEADS, &p.plan_id, prior.map(|(raw, _)| raw)),
        StorageExpectation::new(KEYS, key, None),
        StorageExpectation::new(CASH, &p.account_key, Some(old_cash)),
        StorageExpectation::new(submission::STATE, &p.plan_id, Some(value(record)?)),
    ];
    if h.blocked_reason.is_none() && p.management.is_some() {
        management::provisional_close_writes(
            s,
            p,
            &receipt.observation,
            &receipt.receipt_sha256,
            ctx,
            &mut writes,
            &mut expects,
        )?;
    }
    if s.runtime().storage.put_json_batch(&writes, &expects)? != ComparePutOutcome::Updated {
        return Err("provider_observation_concurrent_change".into());
    }
    Ok(summary(&receipt))
}
pub(super) fn entry_basis(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    pinned: Option<&str>,
) -> Result<(Decimal, Decimal, String, ObservationTime), String> {
    let (_, h) = head(s, p)?.ok_or("provider_observation_required")?;
    if h.blocked_reason.is_some() {
        return Err("provider_observation_reconciliation_blocked".into());
    }
    let current = read_receipt(s, p, &h.receipt_sha256)?;
    let r = if let Some(id) = pinned {
        read_receipt(s, p, id)?
    } else {
        current.clone()
    };
    if r.refused_reason.is_some()
        || r.request.role != wire::Role::Entry
        || !matches!(
            r.observation.order.status,
            broker_v2::OrderStatus::Filled
                | broker_v2::OrderStatus::Cancelled
                | broker_v2::OrderStatus::Expired
        )
    {
        return Err("provider_observation_terminal_entry_required".into());
    }
    let qty = *r.observation.order.filled_quantity.required()?;
    let price = r.observation.average_fill_price.required()?.price;
    if !qty.positive()
        || current.observation.order.filled_quantity != r.observation.order.filled_quantity
        || current.observation.average_fill_price != r.observation.average_fill_price
    {
        return Err("provider_observation_entry_basis_changed".into());
    }
    Ok((
        qty,
        price,
        r.receipt_sha256,
        ObservationTime {
            observed_at_ms: r.observation.order.observed_at_ms,
            known_at_ms: r.observation.order.known_at_ms,
            basis: TimeBasis::ProviderObserved,
        },
    ))
}
pub(super) fn ready(s: &TradeAssemblyService, p: &OrderPlan) -> Result<bool, String> {
    match entry_basis(s, p, None) {
        Ok(_) => Ok(true),
        Err(e)
            if matches!(
                e.as_str(),
                "provider_observation_required"
                    | "provider_observation_terminal_entry_required"
                    | "execution_observation_unavailable"
                    | "provider_observation_reconciliation_blocked"
            ) =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

pub(super) fn verify_applied_close(
    s: &TradeAssemblyService,
    id: &str,
    position: &str,
    quantity: Decimal,
    receipt: &str,
) -> Result<(), String> {
    let p = read_owned(s, id)?;
    if p.management
        .as_ref()
        .is_none_or(|b| b.position_id != position)
    {
        return Err("provider_observation_close_binding_invalid".into());
    }
    let r = read_receipt(s, &p, receipt)?;
    let (_, h) = head(s, &p)?.ok_or("provider_observation_head_missing")?;
    if h.blocked_reason.is_some() {
        return Err("provider_observation_reconciliation_blocked".into());
    }
    let latest = read_receipt(s, &p, &h.receipt_sha256)?;
    if r.refused_reason.is_some()
        || r.request.role != wire::Role::Close
        || r.observation.order.filled_quantity != (Observed::Available { value: quantity })
        || latest.observation.order.filled_quantity != r.observation.order.filled_quantity
    {
        return Err("provider_observation_close_fill_changed".into());
    }
    Ok(())
}

// Private authored storage regression seam; no provider or authority claim.
#[cfg(test)]
pub(super) fn commit_authored_close_test_observation(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    observation: wire::Observation,
    key: &str,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let (_, record) = submission::record(s, p)?.ok_or("broker_v2_state_missing")?;
    let binding = serde_json::from_value(record.intent["preparedBinding"].clone())
        .map_err(|_| "provider_observation_original_binding_invalid")?;
    let request = request(p, &record)?;
    let response_hash = hash(&observation)?;
    commit(
        s,
        p,
        &record,
        Acquired {
            binding,
            request,
            response_hash,
            observation,
        },
        key,
        ctx,
    )
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    #[test]
    fn known_unsent_phases_cannot_observe_or_consume_provisional_fills() {
        use submission::Phase::*;
        for phase in [Admitting, Admitted, Denied] {
            assert_eq!(
                observable_phase(&phase).unwrap_err(),
                "provider_observation_dispatch_not_established"
            );
        }
        for phase in [
            Sending,
            Unknown,
            Open,
            PartiallyFilled,
            Consumed,
            Rejected,
            Cancelled,
        ] {
            observable_phase(&phase).unwrap();
        }
    }
    #[test]
    fn non_atomic_observation_selection_requires_exact_declared_read_role() {
        let role = wire::Role::Entry;
        let make = |deps: Value| json!({"operations":[{"id":crate::broker_submission::v2::SUBMIT_OPERATION,"capability":crate::broker_submission::v2::SUBMIT_CAPABILITY,"dependencies":deps},{"id":role.operation_id(),"capability":role.capability(),"effect":"read","purpose":"recovery"}]});
        let op = crate::broker_submission::v2::SUBMIT_OPERATION;
        assert!(
            !selected_manifest(&make(json!([])), op, role).unwrap(),
            "presence is not opt-in"
        );
        let dep = json!({"dependencyId":"broker.cash_settlement","capability":role.capability(),"operationId":role.operation_id(),"required":true});
        assert!(selected_manifest(&make(json!([dep.clone()])), op, role).unwrap());
        assert!(!selected_manifest(&make(json!([dep.clone()])), op, wire::Role::Close).unwrap());
        for field in ["required", "capability", "operationId"] {
            let mut bad = dep.clone();
            bad[field] = if field == "required" {
                json!(false)
            } else {
                json!("wrong")
            };
            assert!(selected_manifest(&make(json!([bad])), op, role).is_err());
        }
        assert!(selected_manifest(&make(json!([dep.clone(), dep])), op, role).is_err());
    }
}
