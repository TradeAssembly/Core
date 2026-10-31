//! Read-only provider evidence, atomic capital rebasing; never broker actuation.
use super::*;
use crate::ports::broker_submission::BrokerPreparedBinding;
use tradeassembly_plugin_sdk::{broker_v2::OrderStatus, cash_settlement as wire};
const RECEIPTS: &str = "execution_cash_settlement_receipts_v1";
const KEYS: &str = "execution_cash_settlement_keys_v1";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub scope: data_plans::PlanScope,
    pub plan_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CapitalCheckpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_kind: Option<String>,
    pub account_key: String,
    pub ledger_epoch: String,
    pub sequence: u64,
    pub available_cash: Decimal,
    pub gross_exposure: Decimal,
    pub account_semantics_sha256: String,
    pub receipt_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    schema_version: String,
    plan_id: String,
    account_key: String,
    owner_sha256: String,
    source_binding: BrokerPreparedBinding,
    provider_response_sha256: String,
    observation: wire::Observation,
    released_hold: Decimal,
    observed_at_ms: i64,
    receipt_sha256: String,
}
fn receipt_hash(v: &Receipt) -> Result<String, String> {
    let mut x = v.clone();
    x.receipt_sha256.clear();
    hash(&x)
}
fn read_receipt(s: &TradeAssemblyService, id: &str) -> Result<Receipt, String> {
    let v: Receipt = serde_json::from_value(
        s.runtime()
            .storage
            .get_json(RECEIPTS, id)?
            .ok_or("cash_settlement_receipt_missing")?,
    )
    .map_err(|_| "cash_settlement_receipt_invalid")?;
    if v.schema_version != "tradeassembly.cash_settlement_receipt.v1"
        || v.owner_sha256 != owner(s)?
        || v.receipt_sha256 != id
        || receipt_hash(&v)? != id
    {
        return Err("cash_settlement_receipt_invalid".into());
    }
    Ok(v)
}
pub(super) fn entry_basis(
    s: &TradeAssemblyService,
    p: &OrderPlan,
) -> Result<(Decimal, Decimal, String, ObservationTime), String> {
    if p.management.is_some() || p.order.position_action != PositionAction::BuyToOpen {
        return Err("managed_entry_purchase_required".into());
    }
    let (_, record) = submission::record(s, p)?.ok_or("managed_entry_submission_missing")?;
    if record.phase != submission::Phase::Consumed {
        return Err("managed_entry_terminal_required".into());
    }
    let receipt = read_receipt(
        s,
        record
            .settlement_receipt_sha256
            .as_deref()
            .ok_or("managed_entry_final_posting_required")?,
    )?;
    let qty = *receipt.observation.order.filled_quantity.required()?;
    if !qty.positive()
        || qty != record.filled_quantity
        || !receipt.observation.gross_cash.positive()
    {
        return Err("managed_entry_fill_required".into());
    }
    let basis = receipt.observation.gross_cash.checked_divide(
        qty.multiply(p.order.instrument.multiplier)?
            .round(12, Rounding::Exact)?,
        12,
        Rounding::Exact,
    )?;
    if basis
        .multiply(qty)?
        .multiply(p.order.instrument.multiplier)?
        .round(12, Rounding::Exact)?
        != receipt.observation.gross_cash
    {
        return Err("managed_entry_basis_precision_unsupported".into());
    }
    Ok((
        qty,
        basis,
        receipt.receipt_sha256,
        receipt.observation.checkpoint.account.time,
    ))
}
pub(super) fn account_semantics(a: &AccountSnapshot) -> Result<String, String> {
    let mut v = value(a)?;
    v.as_object_mut().unwrap().remove("time");
    hash(&v)
}
pub(super) fn gross_floor(a: &AccountSnapshot) -> Result<Decimal, String> {
    let mut total = *a.gross_exposure.required()?;
    let mut positions = Decimal::ZERO;
    for p in a.positions.required()? {
        if p.side != HoldingSide::Long {
            return Err("cash_settlement_short_account_unsupported".into());
        }
        if p.gross_value.required()?.negative() {
            return Err("cash_settlement_account_invalid".into());
        }
        positions = positions.checked_add(*p.gross_value.required()?)?;
    }
    if positions.compare(total)?.is_gt() {
        return Err("cash_settlement_gross_underreported".into());
    }
    for o in a.open_orders.required()? {
        if o.position_action != PositionAction::BuyToOpen
            || o.worst_case_cash.required()?.negative()
        {
            return Err("cash_settlement_open_order_unsupported".into());
        }
        total = total.checked_add(*o.worst_case_cash.required()?)?;
    }
    Ok(total)
}
pub(super) fn verify_checkpoint(
    s: &TradeAssemblyService,
    c: &CapitalCheckpoint,
) -> Result<(), String> {
    if c.receipt_kind.as_deref() == Some("close_settlement") {
        return super::management::verify_capital(s, c);
    }
    if c.receipt_kind.is_some() {
        return Err("cash_settlement_checkpoint_kind_invalid".into());
    }
    let r = read_receipt(s, &c.receipt_sha256)?;
    let a = &r.observation.checkpoint.account;
    if c.account_key != r.account_key
        || c.ledger_epoch != r.observation.checkpoint.ledger_epoch
        || c.sequence != r.observation.checkpoint.sequence
        || c.available_cash != funding_capacity(a)?
        || c.gross_exposure != gross_floor(a)?
        || c.account_semantics_sha256 != account_semantics(a)?
    {
        return Err("cash_settlement_checkpoint_invalid".into());
    }
    Ok(())
}
pub(super) fn capital_time(
    s: &TradeAssemblyService,
    c: &CapitalCheckpoint,
) -> Result<ObservationTime, String> {
    if c.receipt_kind.as_deref() == Some("close_settlement") {
        return super::management::capital_time(s, c);
    }
    Ok(read_receipt(s, &c.receipt_sha256)?
        .observation
        .checkpoint
        .account
        .time)
}
pub(super) fn funding_floor(
    s: &TradeAssemblyService,
    key: &str,
    available: Decimal,
) -> Result<(Decimal, Decimal, Option<String>), String> {
    let Some(raw) = s.runtime().storage.get_json(CASH, key)? else {
        return Ok((available, Decimal::ZERO, None));
    };
    let ledger = ledger_owned(s, raw)?;
    match ledger.capital_checkpoint {
        None => Ok((available, Decimal::ZERO, None)),
        Some(c) => {
            if c.account_key != key {
                return Err("cash_settlement_checkpoint_account_mismatch".into());
            }
            Ok((
                minimum(available, c.available_cash)?,
                c.gross_exposure,
                Some(c.receipt_sha256),
            ))
        }
    }
}
pub(super) fn current_source(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    r: &submission::SubmissionRecord,
) -> Result<BrokerPreparedBinding, String> {
    let original: PluginOperationRequest =
        serde_json::from_value(r.intent["originalRequest"].clone())
            .map_err(|_| "cash_settlement_original_request_invalid")?;
    let expected: BrokerPreparedBinding =
        serde_json::from_value(r.intent["preparedBinding"].clone())
            .map_err(|_| "cash_settlement_source_invalid")?;
    s.require_object("plugin_instance", &original.plugin_instance_ref)
        .map_err(|_| "cash_settlement_source_not_owned")?;
    let e = crate::broker_submission::v2::original_envelope(
        r.intent.clone(),
        &original,
        &r.original_key,
    )?;
    crate::finance_authority::validate_stored_broker_authorization(
        s.runtime().storage.as_ref(),
        &e,
    )?;
    let runtime = s.runtime();
    let selected = json!({"pluginInstanceRef":original.plugin_instance_ref,"pluginRef":original.plugin_ref,"manifestFingerprint":original.manifest_fingerprint,"accountRef":original.account_ref});
    let witness = crate::broker_submission::account_identity::observer(
        runtime.plugins.as_ref(),
        runtime.plugin_packages.as_ref(),
        runtime.credentials.as_ref(),
        &selected,
        runtime.clock.trusted_now_ms()?,
        "paper",
    )?;
    if Some(&witness.identity) != p.canonical_account.as_ref() {
        return Err("cash_settlement_account_mismatch".into());
    }
    let instance = runtime
        .plugins
        .get_instance(&original.plugin_instance_ref)?
        .ok_or("cash_settlement_source_missing")?;
    let current = BrokerPreparedBinding {
        plugin_instance_ref: original.plugin_instance_ref,
        plugin_ref: original.plugin_ref,
        package_sha256: instance["activePackageSha256"]
            .as_str()
            .ok_or("cash_settlement_source_invalid")?
            .into(),
        manifest_fingerprint: original.manifest_fingerprint,
        configuration_digest: crate::spec::canonical_hash(&instance["configuration"])?,
        credential_ref: instance["credentialRef"]
            .as_str()
            .ok_or("cash_settlement_source_invalid")?
            .into(),
        credential_generation: crate::live_execution_checks::credential_revision(&instance),
    };
    if current != expected {
        return Err("cash_settlement_source_changed".into());
    }
    Ok(current)
}
fn summary(r: &Receipt) -> Value {
    json!({"schemaVersion":r.schema_version,"planId":r.plan_id,"receiptSha256":r.receipt_sha256,"releasedHold":r.released_hold,"postedDebit":r.observation.order.cash_spent,"grossCash":r.observation.gross_cash,"feeCash":r.observation.fee_cash,"checkpointEpoch":r.observation.checkpoint.ledger_epoch,"checkpointSequence":r.observation.checkpoint.sequence,"availableCashCeiling":funding_capacity(&r.observation.checkpoint.account).ok(),"historicalOutcome":true,"executionAuthorityGranted":false})
}
pub(crate) fn settle(
    s: &TradeAssemblyService,
    c: Command,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    if ctx.authority.account_mode != "paper" {
        return Err("cash_settlement_paper_required".into());
    }
    let p = read_owned(s, &c.plan_id)?;
    if c.scope != p.command.scope {
        return Err("cash_settlement_scope_mismatch".into());
    }
    if p.management.is_some() {
        return super::management::settle(s, &p, ctx);
    }
    let key = hash(&(owner(s)?, &c, ctx.idempotency_key.as_str()))?;
    if let Some(prior) = s.runtime().storage.get_json(KEYS, &key)? {
        return Ok(summary(&read_receipt(
            s,
            prior.as_str().ok_or("cash_settlement_key_invalid")?,
        )?));
    }
    let (_, record) = submission::record(s, &p)?.ok_or("broker_v2_state_missing")?;
    if !matches!(
        record.phase,
        submission::Phase::Sending
            | submission::Phase::Unknown
            | submission::Phase::Open
            | submission::Phase::PartiallyFilled
            | submission::Phase::Consumed
    ) && record.settlement_receipt_sha256.is_none()
    {
        return Err("cash_settlement_state_unsupported".into());
    }
    let binding = current_source(s, &p, &record)?;
    require_read_capability(s, &p, &binding, &record.intent["accountRef"])?;
    let input = wire::Request {
        schema_version: "tradeassembly.cash_settlement_request.v1".into(),
        plan_id: p.plan_id.clone(),
        canonical_account_sha256: p
            .canonical_account
            .as_ref()
            .ok_or("cash_settlement_account_missing")?
            .identity_sha256
            .clone(),
        client_order_id: p.order.client_order_id.clone(),
    };
    let original: PluginOperationRequest =
        serde_json::from_value(record.intent["originalRequest"].clone())
            .map_err(|_| "cash_settlement_original_request_invalid")?;
    let response = crate::service::providers::invoke_operation(
        s,
        &format!(
            "/plugins/instances/{}/operations/{}:invoke",
            binding.plugin_instance_ref,
            wire::OPERATION
        ),
        json!({"strategyId":p.command.scope.strategy_id,"strategyVersionId":p.command.scope.strategy_version_id,"strategySpecHash":p.command.scope.strategy_spec_hash,"activationId":original.activation_id,"attemptId":original.attempt_id,"evaluationTickId":original.evaluation_tick_id,"fencingToken":original.fencing_token,"capabilityGraphRevisionId":original.capability_graph_revision_id,"capabilityGraphFingerprint":original.capability_graph_fingerprint,"mode":"paper","purpose":"settlement","accountRef":record.intent["accountRef"],"assetClass":p.order.instrument.asset_class,"instrumentFamily":p.order.instrument.instrument_family,"input":input,"idempotencyKey":format!("settlement-read:{key}")}),
    );
    if response.status != 200 {
        return Err("cash_settlement_provider_unavailable".into());
    }
    let response: PluginOperationResponse =
        serde_json::from_value(response.body["response"].clone())
            .map_err(|_| "cash_settlement_response_invalid")?;
    if response.correlation_id != format!("settlement-read:{key}")
        || response.schema_ref != wire::RESPONSE_SCHEMA
        || response.reconciliation_required
        || response.freshness_state != "fresh"
        || !jsonschema::validator_for(&wire::response_schema())
            .map_err(|_| "cash_settlement_schema_invalid")?
            .is_valid(&response.payload)
    {
        return Err("cash_settlement_response_invalid".into());
    }
    let observation: wire::Observation = serde_json::from_value(response.payload.clone())
        .map_err(|_| "cash_settlement_response_invalid")?;
    if current_source(s, &p, &record)? != binding {
        return Err("cash_settlement_source_changed".into());
    }
    require_read_capability(s, &p, &binding, &record.intent["accountRef"])?;
    commit(s, &p, binding, hash(&response)?, observation, &key, ctx)
}
pub(super) fn commit(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    binding: BrokerPreparedBinding,
    response_hash: String,
    o: wire::Observation,
    key: &str,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let now = s.runtime().clock.trusted_now_ms()?;
    o.validate(now, p.policy.max_snapshot_age_ms)?;
    let a = &o.checkpoint.account;
    let (old_state, mut state) = submission::record(s, p)?.ok_or("broker_v2_state_missing")?;
    if !matches!(
        state.phase,
        submission::Phase::Sending
            | submission::Phase::Unknown
            | submission::Phase::Open
            | submission::Phase::PartiallyFilled
            | submission::Phase::Consumed
    ) && state.settlement_receipt_sha256.is_none()
    {
        return Err("cash_settlement_state_unsupported".into());
    }
    if o.order.order != p.order
        || Some(o.canonical_account_sha256.as_str())
            != p.canonical_account
                .as_ref()
                .map(|x| x.identity_sha256.as_str())
        || o.order.provider_order_id.as_ref().is_some_and(|id| {
            state
                .provider_order_id
                .as_ref()
                .is_some_and(|old| id != old)
        })
        || a.provider_account_id
            != p.canonical_account
                .as_ref()
                .ok_or("cash_settlement_account_missing")?
                .provider_account_id
        || Some(&a.currency) != p.account_currency.as_ref()
        || Some(a.account_ref.as_str()) != state.intent["accountRef"].as_str()
        || a.open_orders.required()?.iter().any(|x| {
            Some(&x.provider_order_id) == o.order.provider_order_id.as_ref()
                || x.client_order_id.as_deref() == Some(p.order.client_order_id.as_str())
        })
    {
        return Err("cash_settlement_identity_or_account_mismatch".into());
    }
    let spent = *o.order.cash_spent.required()?;
    let filled = *o.order.filled_quantity.required()?;
    let PriceInstruction::Limit { limit_price } = p.order.pricing else {
        return Err("cash_settlement_limit_required".into());
    };
    let ceiling = filled
        .multiply(limit_price)?
        .multiply(p.order.instrument.multiplier)?
        .round(a.currency.decimals, Rounding::Ceil)?;
    if spent.compare(p.purchase_cash)?.is_gt()
        || o.fee_cash.compare(p.policy.fee_cash_cap)?.is_gt()
        || o.gross_cash.compare(ceiling)?.is_gt()
        || filled.compare(state.filled_quantity)?.is_lt()
        || state.observation.as_ref().is_some_and(|old| {
            o.order.observed_at_ms < old.observed_at_ms || o.order.known_at_ms < old.known_at_ms
        })
    {
        return Err("cash_settlement_regressed_or_excess_debit".into());
    }
    if let Some(old) = &state.observation {
        if let Observed::Available { value } = old.cash_spent {
            if spent.compare(value)?.is_lt() {
                return Err("cash_settlement_debit_regressed".into());
            }
        }
    }
    let old_cash = s
        .runtime()
        .storage
        .get_json(CASH, &p.account_key)?
        .ok_or("cash_settlement_reservation_missing")?;
    let mut ledger = ledger_owned(s, old_cash.clone())?;
    if let Some(old) = &ledger.capital_checkpoint {
        let previous = capital_time(s, old)?;
        if a.time.observed_at_ms < previous.observed_at_ms
            || a.time.known_at_ms < previous.known_at_ms
        {
            return Err("cash_settlement_checkpoint_time_regressed".into());
        }
        if old.account_key != p.account_key
            || old.ledger_epoch != o.checkpoint.ledger_epoch
            || o.checkpoint.sequence < old.sequence
            || o.checkpoint.sequence == old.sequence
                && old.account_semantics_sha256 != account_semantics(a)?
        {
            return Err("cash_settlement_checkpoint_regressed".into());
        }
    }
    let already = if let Some(id) = &state.settlement_receipt_sha256 {
        Some(read_receipt(s, id)?)
    } else {
        None
    };
    if let Some(old) = &already {
        let x = &old.observation;
        if x.posting_id != o.posting_id
            || x.posting_sequence != o.posting_sequence
            || x.gross_cash != o.gross_cash
            || x.fee_cash != o.fee_cash
            || x.order.status != o.order.status
            || x.order.filled_quantity != o.order.filled_quantity
            || x.order.cash_spent != o.order.cash_spent
            || x.order.provider_order_id != o.order.provider_order_id
        {
            return Err("cash_settlement_final_posting_changed".into());
        }
    }
    let released = if already.is_some() {
        if state.retained_cash != Decimal::ZERO || ledger.reservations.contains_key(&p.plan_id) {
            return Err("cash_settlement_reservation_invalid".into());
        }
        Decimal::ZERO
    } else {
        if ledger.reservations.get(&p.plan_id) != Some(&p.purchase_cash)
            || state.retained_cash != p.purchase_cash
        {
            return Err("cash_settlement_reservation_invalid".into());
        }
        p.purchase_cash
    };
    let mut receipt = Receipt {
        schema_version: "tradeassembly.cash_settlement_receipt.v1".into(),
        plan_id: p.plan_id.clone(),
        account_key: p.account_key.clone(),
        owner_sha256: owner(s)?,
        source_binding: binding,
        provider_response_sha256: response_hash,
        observation: o.clone(),
        released_hold: released,
        observed_at_ms: now,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = receipt_hash(&receipt)?;
    ledger.reservations.remove(&p.plan_id);
    ledger.capital_checkpoint = Some(CapitalCheckpoint {
        receipt_kind: None,
        account_key: p.account_key.clone(),
        ledger_epoch: o.checkpoint.ledger_epoch.clone(),
        sequence: o.checkpoint.sequence,
        available_cash: funding_capacity(a)?,
        gross_exposure: gross_floor(a)?,
        account_semantics_sha256: account_semantics(a)?,
        receipt_sha256: receipt.receipt_sha256.clone(),
    });
    state.phase = if spent == Decimal::ZERO && o.order.status == OrderStatus::Rejected {
        submission::Phase::Rejected
    } else if spent == Decimal::ZERO
        && matches!(
            o.order.status,
            OrderStatus::Cancelled | OrderStatus::Expired
        )
    {
        submission::Phase::Cancelled
    } else {
        submission::Phase::Consumed
    };
    state.retained_cash = Decimal::ZERO;
    state.filled_quantity = filled;
    state.provider_order_id = o.order.provider_order_id.clone();
    state.observation = Some(o.order);
    state.generation = state
        .generation
        .checked_add(1)
        .filter(|generation| *generation <= 10000)
        .ok_or("cash_settlement_generation_invalid")?;
    if state.settlement_receipt_sha256.is_none() {
        state.settlement_receipt_sha256 = Some(receipt.receipt_sha256.clone())
    }
    state.record_sha256 = submission::record_hash(&state)?;
    if s.runtime().storage.put_json_batch(
        &[
            StorageWrite::new(
                RECEIPTS,
                &receipt.receipt_sha256,
                value(&receipt)?,
                ctx.clone(),
            ),
            StorageWrite::new(KEYS, key, json!(receipt.receipt_sha256), ctx.clone()),
            StorageWrite::new(CASH, &p.account_key, value(&ledger)?, ctx.clone()),
            StorageWrite::new(submission::STATE, &p.plan_id, value(&state)?, ctx.clone()),
        ],
        &[
            StorageExpectation::new(RECEIPTS, &receipt.receipt_sha256, None),
            StorageExpectation::new(KEYS, key, None),
            StorageExpectation::new(CASH, &p.account_key, Some(old_cash)),
            StorageExpectation::new(submission::STATE, &p.plan_id, Some(old_state)),
            StorageExpectation::new(RELEASES, &p.plan_id, None),
        ],
    )? != ComparePutOutcome::Updated
    {
        return Err("cash_settlement_commit_conflict".into());
    }
    Ok(summary(&receipt))
}

pub(super) fn verify_record(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    r: &submission::SubmissionRecord,
) -> Result<(), String> {
    if p.management.is_some() {
        return super::management::verify_close_record(s, p, r);
    }
    if let Some(id) = &r.settlement_receipt_sha256 {
        let receipt = read_receipt(s, id)?;
        if receipt.plan_id != p.plan_id
            || receipt.account_key != p.account_key
            || r.retained_cash != Decimal::ZERO
            || !matches!(
                r.phase,
                submission::Phase::Consumed
                    | submission::Phase::Cancelled
                    | submission::Phase::Rejected
            )
        {
            return Err("cash_settlement_record_invalid".into());
        }
    }
    Ok(())
}

/// Closed historical projection; never a broker permission or a current balance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Projection {
    pub schema_version: String,
    pub receipt_sha256: String,
    pub canonical_account_sha256: String,
    pub posting_id: String,
    pub posting_sequence: u64,
    pub provider_response_sha256: String,
    pub source_binding_sha256: String,
    pub account_checkpoint_sha256: String,
    pub currency: Currency,
    pub gross_exposure_floor: Decimal,
    pub historical_outcome: bool,
    pub posted_debit: Decimal,
    pub gross_cash: Decimal,
    pub fee_cash: Decimal,
    pub released_hold: Decimal,
    pub checkpoint_epoch: String,
    pub checkpoint_sequence: u64,
    pub available_cash_ceiling: Decimal,
    pub execution_authority_granted: bool,
}
pub(super) fn projection(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    id: &str,
) -> Result<Projection, String> {
    let r = read_receipt(s, id)?;
    if r.plan_id != p.plan_id || r.account_key != p.account_key {
        return Err("cash_settlement_receipt_invalid".into());
    }
    Ok(Projection {
        schema_version: "tradeassembly.cash_settlement_projection.v1".into(),
        receipt_sha256: r.receipt_sha256,
        canonical_account_sha256: r.observation.canonical_account_sha256,
        posting_id: r.observation.posting_id.clone(),
        posting_sequence: r.observation.posting_sequence,
        provider_response_sha256: r.provider_response_sha256.clone(),
        source_binding_sha256: hash(&r.source_binding)?,
        account_checkpoint_sha256: hash(&r.observation.checkpoint)?,
        currency: r.observation.checkpoint.account.currency.clone(),
        gross_exposure_floor: gross_floor(&r.observation.checkpoint.account)?,
        historical_outcome: true,
        posted_debit: *r.observation.order.cash_spent.required()?,
        gross_cash: r.observation.gross_cash,
        fee_cash: r.observation.fee_cash,
        released_hold: r.released_hold,
        checkpoint_epoch: r.observation.checkpoint.ledger_epoch,
        checkpoint_sequence: r.observation.checkpoint.sequence,
        available_cash_ceiling: funding_capacity(&r.observation.checkpoint.account)?,
        execution_authority_granted: false,
    })
}
pub(super) fn preflight(s: &TradeAssemblyService, p: &OrderPlan) -> Result<(), String> {
    let (_, record) = submission::record(s, p)?.ok_or("broker_v2_state_missing")?;
    if p.account_currency.is_none() {
        return Err("cash_settlement_currency_pin_required".into());
    }
    let binding = current_source(s, p, &record)?;
    require_read_capability(s, p, &binding, &record.intent["accountRef"])?;
    Ok(())
}

fn require_read_capability(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    binding: &BrokerPreparedBinding,
    account_ref: &Value,
) -> Result<(), String> {
    let manifest = s
        .runtime()
        .plugins
        .get_manifest(&binding.plugin_ref)?
        .ok_or("cash_settlement_operation_unavailable")?;
    let operation = manifest["manifest"]["operations"]
        .as_array()
        .and_then(|ops| ops.iter().find(|op| op["id"] == wire::OPERATION))
        .ok_or("cash_settlement_operation_unavailable")?;
    if operation["capability"] != wire::CAPABILITY
        || operation["effect"] != "read"
        || operation["purpose"] != "settlement"
    {
        return Err("cash_settlement_operation_invalid".into());
    }
    let resolution = super::super::providers::resolve_capability(
        s,
        super::super::providers::requirement_from_body(
            &json!({"capability":wire::CAPABILITY,"pluginRef":binding.plugin_ref,"pluginInstanceRef":binding.plugin_instance_ref,"operation":wire::OPERATION,"mode":"paper","purpose":"settlement","accountRef":account_ref,"assetClass":p.order.instrument.asset_class,"instrumentFamily":p.order.instrument.instrument_family}),
        ),
    );
    if !resolution["candidates"].as_array().is_some_and(|xs| {
        xs.iter().any(|x| {
            x["pluginInstanceRef"] == binding.plugin_instance_ref
                && x["operation"]["id"] == wire::OPERATION
                && x["blockers"].as_array().is_some_and(Vec::is_empty)
        })
    }) {
        return Err("cash_settlement_permission_changed".into());
    }
    Ok(())
}
