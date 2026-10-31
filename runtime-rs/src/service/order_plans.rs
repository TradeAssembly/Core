//! Host-witnessed sizing proposals and shared cash reservations. Neither is C5
//! admission; the existing enforcing broker boundary remains mandatory.
use super::{
    capability_graph, data_binding, data_collections, data_evaluation, data_plans, strategy,
    ServiceResponse, TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, ports::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{cmp::Ordering, collections::BTreeMap};
use tradeassembly_plugin_sdk::{
    execution::*, execution_calculation as calc, execution_snapshot::*,
};
pub mod decision;
pub mod remote;
pub mod submission;
pub mod transport;
const CONFIG: &str = "tradeassembly.order_plan";
const PLANS: &str = "execution_order_plans_v1";
const COMMANDS: &str = "execution_order_plan_commands_v1";
const ENTRIES: &str = "execution_entry_plans_v1";
const KEYS: &str = "execution_order_plan_keys_v1";
const CASH: &str = "execution_cash_reservations_v1";
const RELEASES: &str = "execution_cash_releases_v1";
pub(super) const PREPARE: &str = "/product/strategies/order-plans/prepare";
pub(super) const RESERVE: &str = "/product/strategies/order-plans/reserve";
pub(super) const RECONCILE: &str = "/product/strategies/order-plans/reconcile";
pub(super) const DECIDE: &str = "/product/strategies/order-plans/decide";
pub(super) const SELECT: &str = "/product/strategies/order-plans/select";
pub(super) const SUBMIT: &str = "/product/strategies/order-plans/submit";
pub(super) const RELEASE: &str = "/product/strategies/order-plans/release";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub schema_version: String,
    pub calculation_node: String,
    pub semantics_version: String,
    pub source_nodes: BTreeMap<String, String>,
    pub parameters: Value,
    pub position_action: PositionAction,
    pub time_in_force: String,
    pub fee_cash_cap: Decimal,
    pub max_purchase_cash: Decimal,
    pub max_premium_gross: Decimal,
    pub max_quantity: Decimal,
    pub maximum_ask_multiple: Decimal,
    pub max_signal_age_ms: i64,
    pub max_snapshot_age_ms: i64,
    pub quote_profiles: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareCommand {
    pub scope: data_plans::PlanScope,
    pub capability_graph_revision: String,
    pub mode: String,
    pub reserve_cash: bool,
    pub signal_receipt_sha256: String,
    pub account_collection_id: String,
    pub metadata_collection_id: String,
    pub quote_collection_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrderPlan {
    pub plan_id: String,
    pub entry_id: String,
    pub command: PrepareCommand,
    pub policy: Policy,
    pub calculation: data_binding::PinnedOperation,
    pub source_receipts: BTreeMap<String, String>,
    pub account_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_account:
        Option<crate::broker_submission::account_identity::CanonicalAccountIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_binding_sha256: Option<String>,
    pub owner_sha256: String,
    pub prepared_at_ms: i64,
    pub valid_until_ms: i64,
    pub available_cash: Decimal,
    pub order: BrokerOrderV2,
    pub purchase_cash: Decimal,
    pub calculation_response_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ledger {
    owner_sha256: String,
    reservations: BTreeMap<String, Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gross_limit: Option<Decimal>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseReceipt {
    schema_version: String,
    plan_id: String,
    account_key: String,
    owner_sha256: String,
    released_cash: Decimal,
    released_at_ms: i64,
    receipt_sha256: String,
}
fn release_hash(r: &ReleaseReceipt) -> Result<String, String> {
    let mut unsigned = r.clone();
    unsigned.receipt_sha256.clear();
    hash(&unsigned)
}
fn ledger_owned(s: &TradeAssemblyService, raw: Value) -> Result<Ledger, String> {
    let ledger: Ledger =
        serde_json::from_value(raw).map_err(|_| "order_plan_reservations_invalid")?;
    if ledger.owner_sha256 != owner(s)?
        || ledger.reservations.len() > 256
        || ledger.reservations.values().any(|v| !v.positive())
    {
        return Err("order_plan_reservations_unavailable".into());
    }
    Ok(ledger)
}
// A durable terminal fence, never a broker cancellation or an execution permit.
fn released(s: &TradeAssemblyService, r: &OrderPlan) -> Result<Option<ReleaseReceipt>, String> {
    let Some(raw) = s.runtime().storage.get_json(RELEASES, &r.plan_id)? else {
        return Ok(None);
    };
    let receipt: ReleaseReceipt =
        serde_json::from_value(raw).map_err(|_| "order_plan_release_invalid")?;
    if receipt.schema_version != "tradeassembly.order_plan_release.v1"
        || receipt.plan_id != r.plan_id
        || receipt.account_key != r.account_key
        || receipt.owner_sha256 != r.owner_sha256
        || receipt.receipt_sha256 != release_hash(&receipt)?
        || receipt.released_at_ms <= 0
        || receipt.released_at_ms > s.runtime().clock.now_ms()
        || !(receipt.released_cash == Decimal::ZERO || receipt.released_cash == r.purchase_cash)
    {
        return Err("order_plan_release_invalid".into());
    }
    let ledger = ledger_owned(
        s,
        s.runtime()
            .storage
            .get_json(CASH, &r.account_key)?
            .ok_or("order_plan_release_invalid")?,
    )?;
    if ledger.reservations.contains_key(&r.plan_id) {
        return Err("order_plan_release_inconsistent".into());
    }
    Ok(Some(receipt))
}
fn release_summary(r: &ReleaseReceipt) -> Result<Value, String> {
    Ok(
        json!({"planId":r.plan_id,"reservationStatus":"released","releasedCash":r.released_cash,"releaseReceipt":r,"executionAuthorityGranted":false,"brokerDispatchAllowed":false}),
    )
}
pub fn release(
    s: &TradeAssemblyService,
    id: &str,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    if ctx.authority.account_mode != "paper" {
        return Err("order_plan_mode_unsupported".into());
    }
    let original = read_owned(s, id)?;
    for _ in 0..8 {
        if let Some(r) = released(s, &original)? {
            return release_summary(&r);
        }
        let submission_state = submission::release_guard(s, &original)?;
        let old = s.runtime().storage.get_json(CASH, &original.account_key)?;
        let mut ledger = match old.clone() {
            Some(raw) => ledger_owned(s, raw)?,
            None => Ledger {
                owner_sha256: owner(s)?,
                reservations: BTreeMap::new(),
                gross_limit: None,
            },
        };
        let amount = ledger.reservations.remove(id).unwrap_or(Decimal::ZERO);
        if amount != Decimal::ZERO && amount != original.purchase_cash {
            return Err("order_plan_reservation_conflict".into());
        }
        let mut receipt = ReleaseReceipt {
            schema_version: "tradeassembly.order_plan_release.v1".into(),
            plan_id: id.into(),
            account_key: original.account_key.clone(),
            owner_sha256: owner(s)?,
            released_cash: amount,
            released_at_ms: s.runtime().clock.now_ms(),
            receipt_sha256: String::new(),
        };
        receipt.receipt_sha256 = release_hash(&receipt)?;
        let outcome = s
            .runtime()
            .storage
            .put_json_batch(
                &[
                    StorageWrite::new(CASH, &original.account_key, value(&ledger)?, ctx.clone()),
                    StorageWrite::new(RELEASES, id, value(&receipt)?, ctx.clone()),
                ],
                &[
                    StorageExpectation::new(CASH, &original.account_key, old),
                    StorageExpectation::new(RELEASES, id, None),
                    StorageExpectation::new(PLANS, id, Some(value(&original)?)),
                    StorageExpectation::new(submission::STATE, id, submission_state),
                ],
            )
            .map_err(|_| "order_plan_release_commit_failed")?;
        if outcome == ComparePutOutcome::Updated {
            return release_summary(&receipt);
        }
    }
    Err("order_plan_release_contention".into())
}
struct Facts {
    entry_id: String,
    account: AccountSnapshot,
    metadata: InstrumentMetadata,
    quote: Quote,
    receipts: BTreeMap<String, String>,
    account_key: String,
    canonical_account: Option<crate::broker_submission::account_identity::CanonicalAccountIdentity>,
    account_binding_sha256: Option<String>,
    available: Decimal,
    valid_until: i64,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "order_plan_encoding_invalid".into())
}
fn value<T: Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|_| "order_plan_encoding_invalid".into())
}
fn owner(s: &TradeAssemblyService) -> Result<String, String> {
    hash(&s.invocation_owner().ok_or("order_plan_human_required")?)
}
fn minimum(a: Decimal, b: Decimal) -> Result<Decimal, String> {
    Ok(if a.compare(b)? == Ordering::Greater {
        b
    } else {
        a
    })
}
fn within(a: Decimal, b: Decimal) -> Result<(), String> {
    if a.negative() || a.compare(b)? == Ordering::Greater {
        Err("order_plan_risk_limit_exceeded".into())
    } else {
        Ok(())
    }
}
fn policy(s: &TradeAssemblyService, c: &PrepareCommand) -> Result<Policy, String> {
    published_policy(s, &c.scope)
}
pub(super) fn published_policy(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
) -> Result<Policy, String> {
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| "order_plan_not_available")?;
    let v = strategy::strategy_versions(s, &scope.strategy_id)
        .into_iter()
        .find(|v| v["id"] == scope.strategy_version_id && v["specHash"] == scope.strategy_spec_hash)
        .ok_or("order_plan_version_missing")?;
    if crate::spec::canonical_hash(&v["spec"])? != scope.strategy_spec_hash
        || !tradeassembly_strategy::spec::validate(&v["spec"]).valid
    {
        return Err("order_plan_spec_invalid".into());
    }
    let stage = &v["spec"]["stages"]["sizing"];
    let steps = stage["substeps"]
        .as_array()
        .ok_or("order_plan_policy_missing")?;
    if stage["enabled"] != true
        || steps.len() != 1
        || steps[0]["optional"] == true
        || steps[0]["config"].as_object().is_none_or(|v| v.len() != 1)
    {
        return Err("order_plan_stage_unsupported".into());
    }
    let p: Policy = serde_json::from_value(steps[0]["config"][CONFIG].clone())
        .map_err(|_| "order_plan_policy_invalid")?;
    tradeassembly_plugin_sdk::data_package::metadata_budget(&p.parameters)?;
    if p.schema_version != "tradeassembly.order_plan_policy.v1"
        || !p.parameters.is_object()
        || p.calculation_node.is_empty()
        || p.calculation_node.len() > 128
        || p.semantics_version.is_empty()
        || p.semantics_version.len() > 128
        || p.source_nodes
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != vec!["account_data", "instrument_metadata", "pricing_quote"]
        || p.source_nodes
            .values()
            .any(|v| v.is_empty() || v.len() > 128)
        || p.position_action != PositionAction::BuyToOpen
        || !matches!(p.time_in_force.as_str(), "day" | "gtc")
        || p.fee_cash_cap.negative()
        || !p.max_purchase_cash.positive()
        || !p.max_premium_gross.positive()
        || !p.max_quantity.positive()
        || p.maximum_ask_multiple.compare("1".parse()?)? == Ordering::Less
        || p.maximum_ask_multiple.compare("2".parse()?)? == Ordering::Greater
        || !(1..=604800000).contains(&p.max_signal_age_ms)
        || !(1..=30000).contains(&p.max_snapshot_age_ms)
        || p.quote_profiles.is_empty()
        || p.quote_profiles.len() > 16
        || p.quote_profiles
            .iter()
            .any(|x| x.is_empty() || x.len() > 128)
    {
        return Err("order_plan_policy_unsupported".into());
    }
    let typed = tradeassembly_strategy::spec::StrategySpec::model_validate(v["spec"].clone())
        .map_err(|_| "order_plan_spec_invalid")?;
    if !typed.capability_requirements.required.iter().any(|r| {
        r.requirement_id == p.calculation_node
            && r.stage_refs
                .contains(&tradeassembly_strategy::spec::StageName::Sizing)
    }) {
        return Err("order_plan_sizing_requirement_missing".into());
    }
    Ok(p)
}
fn observation(now: i64, time: &ObservationTime, host: i64, age: i64) -> Result<i64, String> {
    time.validate()?;
    if host > now
        || time.known_at_ms > host
        || time.observed_at_ms > now
        || now - time.observed_at_ms >= age
    {
        return Err("order_plan_observation_stale".into());
    }
    time.observed_at_ms
        .checked_add(age)
        .ok_or("order_plan_time_invalid".into())
}
#[inline]
pub(crate) fn measured<T>(stage: &'static str, f: impl FnOnce() -> T) -> T {
    #[cfg(not(test))]
    {
        let _ = stage;
        f()
    }
    #[cfg(test)]
    {
        let start = std::time::Instant::now();
        let result = f();
        if std::env::var_os("FINANCIAL_PROFILE").is_some() {
            eprintln!("FIN_PROFILE {stage} {}us", start.elapsed().as_micros());
        }
        result
    }
}
fn collection(
    s: &TradeAssemblyService,
    c: &PrepareCommand,
    id: &str,
    schema_ref: &str,
    schema: Value,
) -> Result<data_collections::CollectionReceipt, String> {
    let r = measured("collection.current", || data_collections::current(s, id))?;
    if r.command.scope != c.scope
        || r.command.capability_graph_revision != c.capability_graph_revision
        || r.command.mode != c.mode
        || r.command.response_schema_ref != schema_ref
        || r.response_schema_sha256 != hash(&schema)?
    {
        return Err("order_plan_source_mismatch".into());
    }
    Ok(r)
}
fn funding_capacity(account: &AccountSnapshot) -> Result<Decimal, String> {
    if *account.borrowing_debt.required()? != Decimal::ZERO
        || account.cash_balance.required()?.negative()
        || account.non_marginable_buying_power.required()?.negative()
    {
        return Err("order_plan_borrowing_unsupported".into());
    }
    minimum(
        *account.cash_balance.required()?,
        *account.non_marginable_buying_power.required()?,
    )
}
// Call-local evidence only: no Clone/serde/storage and no public constructor.
// Every approval/admission revalidation round loads a fresh instance.
pub(crate) struct VerifiedSignal {
    owner_sha256: String,
    database: String,
    command: PrepareCommand,
    plan: data_plans::StoredDataPlan,
    receipt: data_evaluation::EvaluationReceipt,
}
impl VerifiedSignal {
    pub(crate) fn matches(
        &self,
        s: &TradeAssemblyService,
        c: &PrepareCommand,
    ) -> Result<(), String> {
        if self.owner_sha256 != owner(s)? || self.database != s.db() || self.command != *c {
            return Err("order_plan_signal_witness_mismatch".into());
        }
        Ok(())
    }
}
pub(crate) fn verify_signal(
    s: &TradeAssemblyService,
    c: &PrepareCommand,
) -> Result<VerifiedSignal, String> {
    let (plan, receipt) = measured("signal.verify_round", || {
        data_evaluation::indexed_receipt(s, &c.signal_receipt_sha256)
    })?;
    if plan.content.scope != c.scope
        || plan.content.plan.request.capability_graph_revision != c.capability_graph_revision
        || plan.content.plan.request.mode != c.mode
        || receipt.receipt_sha256 != c.signal_receipt_sha256
    {
        return Err("order_plan_signal_binding_mismatch".into());
    }
    Ok(VerifiedSignal {
        owner_sha256: owner(s)?,
        database: s.db().into(),
        command: c.clone(),
        plan,
        receipt,
    })
}
fn facts(s: &TradeAssemblyService, c: &PrepareCommand, p: &Policy) -> Result<Facts, String> {
    let signal = verify_signal(s, c)?;
    facts_with_signal(s, c, p, &signal)
}
fn facts_with_signal(
    s: &TradeAssemblyService,
    c: &PrepareCommand,
    p: &Policy,
    verified: &VerifiedSignal,
) -> Result<Facts, String> {
    verified.matches(s, c)?;
    if c.mode != "paper" {
        return Err("order_plan_mode_unsupported".into());
    }
    let now = s.runtime().clock.now_ms();
    let signal = &verified.receipt;
    let frame = signal
        .signals
        .last()
        .ok_or("order_plan_signal_unavailable")?;
    if frame.entry != (crate::data_strategy::SignalValue::Ready { value: true })
        || frame.trade_instruments.len() != 1
        || frame.known_at_ms > now
        || frame.arrived_at_ms > now
        || frame.event_end_ms > now
        || now - frame.event_end_ms >= p.max_signal_age_ms
    {
        return Err("order_plan_signal_not_current_ready_entry".into());
    }
    let subject = &frame.trade_instruments[0];
    let ar = collection(
        s,
        c,
        &c.account_collection_id,
        ACCOUNT_SCHEMA,
        account_schema(),
    )?;
    let mr = collection(
        s,
        c,
        &c.metadata_collection_id,
        METADATA_SCHEMA,
        metadata_schema(),
    )?;
    let qr = collection(s, c, &c.quote_collection_id, QUOTE_SCHEMA, quote_schema())?;
    for (name, r) in [
        ("account_data", &ar),
        ("instrument_metadata", &mr),
        ("pricing_quote", &qr),
    ] {
        if p.source_nodes.get(name) != Some(&r.command.acquisition_node) {
            return Err("order_plan_provider_selection_mismatch".into());
        }
    }
    let account: AccountSnapshot = serde_json::from_value(ar.response.payload.clone())
        .map_err(|_| "order_plan_account_invalid")?;
    let metadata: InstrumentMetadata = serde_json::from_value(mr.response.payload.clone())
        .map_err(|_| "order_plan_metadata_invalid")?;
    let quote: Quote = serde_json::from_value(qr.response.payload.clone())
        .map_err(|_| "order_plan_quote_invalid")?;
    account.validate()?;
    metadata.validate()?;
    quote.validate()?;
    let mut valid_until = frame
        .event_end_ms
        .checked_add(p.max_signal_age_ms)
        .ok_or("order_plan_time_invalid")?;
    for (time, r) in [
        (&account.time, &ar),
        (&metadata.time, &mr),
        (&quote.time, &qr),
    ] {
        valid_until = valid_until.min(observation(
            now,
            time,
            r.host_known_at_ms,
            p.max_snapshot_age_ms,
        )?);
    }
    if account.mode != TradingMode::Paper
        || ar.acquisition.account_ref.as_deref() != Some(account.account_ref.as_str())
        || !account.coverage.account_complete
        || !account.coverage.positions_complete
        || !account.coverage.open_orders_complete
    {
        return Err("order_plan_account_coverage_or_binding_invalid".into());
    }
    account.positions.required()?;
    account.open_orders.required()?;
    funding_capacity(&account)?;
    let permission = account
        .instrument_permissions
        .iter()
        .find(|x| {
            x.asset_class == subject.asset_class && x.instrument_family == subject.instrument_family
        })
        .ok_or("order_plan_trading_permission_missing")?;
    if !permission.actions.required()?.contains(&p.position_action) {
        return Err("order_plan_trading_permission_missing".into());
    }
    if metadata.instrument_id != subject.instrument_id
        || metadata.asset_class != subject.asset_class
        || metadata.instrument_family != subject.instrument_family
        || quote.instrument_id != subject.instrument_id
        || metadata.quote_currency.required()? != &account.currency
        || quote.currency != account.currency
        || metadata.tradable.required()? != &true
        || quote.indicative.required()? != &false
        || quote.origin != QuoteOrigin::Provider
        || !p.quote_profiles.contains(&quote.source_profile)
    {
        return Err("order_plan_instrument_or_quote_ineligible".into());
    }
    if metadata
        .expires_at_ms
        .required()?
        .is_some_and(|expiry| expiry <= now)
    {
        return Err("order_plan_instrument_expired".into());
    }
    if metadata.quantity_unit.required()? == &QuantityUnit::Contracts
        && metadata.standard_deliverable.required()? != &true
    {
        return Err("order_plan_deliverable_unsupported".into());
    }
    quote.bid.required()?;
    quote.ask.required()?;
    metadata.minimum_quantity.required()?;
    metadata.quantity_increment.required()?;
    metadata.price_increment.required()?;
    metadata.multiplier.required()?;
    let legacy_key = hash(&(
        "tradeassembly.cash_account.v1",
        &ar.acquisition.plugin_ref,
        &account.provider_account_id,
        &account.currency,
        &account.mode,
    ))?;
    let runtime = s.runtime();
    let witness = crate::broker_submission::account_identity::current(
        runtime.plugins.as_ref(),
        runtime.plugin_packages.as_ref(),
        runtime.credentials.as_ref(),
        &json!({"pluginInstanceRef":ar.acquisition.plugin_instance_ref,"pluginRef":ar.acquisition.plugin_ref,"manifestFingerprint":ar.acquisition.manifest_sha256,"accountRef":ar.acquisition.account_ref}),
        now,
        "paper",
    )?;
    if witness.identity.provider_account_id != account.provider_account_id
        || witness.account_ref != account.account_ref
    {
        return Err("order_plan_canonical_account_mismatch".into());
    }
    let account_key = hash(&(
        "tradeassembly.cash_account.v2",
        &witness.identity.identity_sha256,
        &account.currency,
    ))?;
    // Old proposal/checkpoint bytes are untouched. An outstanding legacy hold
    // cannot silently disappear into a newly keyed ledger.
    if let Some(raw) = runtime.storage.get_json(CASH, &legacy_key)? {
        if !ledger_owned(s, raw)?.reservations.is_empty() {
            return Err("order_plan_legacy_reservations_require_release".into());
        }
    }
    let available = funding_capacity(&account)?;
    let receipts = BTreeMap::from([
        ("signal".into(), signal.receipt_sha256.clone()),
        ("account".into(), ar.receipt_sha256),
        ("metadata".into(), mr.receipt_sha256),
        ("quote".into(), qr.receipt_sha256),
    ]);
    let entry_id = hash(&(
        "tradeassembly.entry_intent.v1",
        owner(s)?,
        &c.scope,
        &c.mode,
        &subject.instrument_id,
        frame.event_end_ms,
    ))?;
    Ok(Facts {
        entry_id,
        account,
        metadata,
        quote,
        receipts,
        account_key,
        canonical_account: Some(witness.identity),
        account_binding_sha256: Some(witness.binding_sha256),
        available,
        valid_until,
    })
}
fn calculation(
    s: &TradeAssemblyService,
    c: &PrepareCommand,
    p: &Policy,
    f: &Facts,
) -> Result<
    (
        CapabilityGraphResolution,
        CapabilitySelection,
        data_binding::PinnedOperation,
    ),
    String,
> {
    let epoch = chrono::DateTime::from_timestamp_millis(s.runtime().clock.now_ms())
        .ok_or("order_plan_clock_invalid")?
        .to_rfc3339();
    let revision = capability_graph::revision_for_context(
        s,
        &c.capability_graph_revision,
        &c.scope.strategy_id,
        &c.scope.strategy_version_id,
        &c.mode,
        &epoch,
    )
    .map_err(|_| "order_plan_graph_stale")?;
    if revision["strategySpecHash"] != c.scope.strategy_spec_hash {
        return Err("order_plan_graph_mismatch".into());
    }
    let graph: CapabilityGraphResolution = serde_json::from_value(revision["graph"].clone())
        .map_err(|_| "order_plan_graph_invalid")?;
    let selected = graph
        .nodes
        .iter()
        .find(|n| n.node_id == p.calculation_node)
        .and_then(|n| n.selected.clone())
        .ok_or("order_plan_calculation_missing")?;
    let pin = data_binding::pin(s, &selected)?;
    if s.runtime()
        .plugins
        .get_instance(&selected.plugin_instance_ref)?
        .and_then(|v| v.get("configuration").cloned())
        .unwrap_or_else(|| json!({}))
        != json!({})
    {
        return Err("order_plan_hidden_calculation_configuration_unsupported".into());
    }

    let runtime = s.runtime();
    let package = runtime
        .plugin_packages
        .get(&pin.package_sha256)?
        .ok_or("order_plan_package_missing")?;
    data_evaluation::allowed_data_operation(&selected, true, &package.manifest)?;
    let contract = runtime
        .plugin_packages
        .operation_data(&pin.package_sha256, &pin.operation_id)?
        .ok_or("order_plan_contract_missing")?;
    if contract.state.is_some()
        || contract.semantics_version != p.semantics_version
        || !selected.operation.traits.deterministic
        || !selected.operation.traits.replayable
        || !selected.operation.traits.modes.contains(&c.mode)
        || !contract.applicability.modes.contains(&c.mode)
        || !contract
            .applicability
            .asset_classes
            .contains(&f.metadata.asset_class)
        || !contract
            .applicability
            .instrument_families
            .contains(&f.metadata.instrument_family)
        || !selected
            .operation
            .traits
            .instrument_families
            .contains(&f.metadata.instrument_family)
        || contract.warmup.minimum_observations != 1
        || contract.warmup.observations_parameter.is_some()
        || contract.warmup.additional_observations != 0
    {
        return Err("order_plan_calculation_contract_unsupported".into());
    }
    if !selected
        .operation
        .traits
        .input_schema_refs
        .contains(&calc::INPUT_SCHEMA.into())
        || !selected
            .operation
            .traits
            .output_schema_refs
            .contains(&calc::OUTPUT_SCHEMA.into())
    {
        return Err("order_plan_calculation_wire_unsupported".into());
    }
    for (id, expected) in [
        (calc::INPUT_SCHEMA, calc::input_schema()),
        (calc::OUTPUT_SCHEMA, calc::output_schema()),
    ] {
        let actual = runtime
            .plugin_packages
            .data_schema(&pin.package_sha256, id)?
            .ok_or("order_plan_schema_missing")?;
        if hash(&actual)? != hash(&expected)? {
            return Err("order_plan_schema_mismatch".into());
        }
    }
    let ports = [
        ("account", ACCOUNT_SCHEMA, account_schema()),
        ("metadata", METADATA_SCHEMA, metadata_schema()),
        ("quote", QUOTE_SCHEMA, quote_schema()),
        (
            "availableCash",
            "schema://tradeassembly/execution-decimal@1",
            decimal_schema(),
        ),
    ];
    if contract.inputs.len() != ports.len() || contract.outputs.len() != 1 {
        return Err("order_plan_calculation_ports_invalid".into());
    }
    for (name, id, schema) in ports {
        let input = contract
            .inputs
            .get(name)
            .ok_or("order_plan_calculation_ports_invalid")?;
        if !input.required
            || input.data.schema.schema_ref != id
            || input.data.schema.sha256 != hash(&schema)?
            || input.data.shape != tradeassembly_plugin_sdk::data_contract::DataShape::Scalar
            || input.data.time.interval_ms.is_some()
            || !input.data.time.completed_only
        {
            return Err("order_plan_calculation_ports_invalid".into());
        }
    }
    let output = contract
        .outputs
        .get("proposal")
        .ok_or("order_plan_calculation_ports_invalid")?;
    if output.schema.schema_ref != calc::OUTPUT_SCHEMA
        || output.schema.sha256 != hash(&calc::output_schema())?
        || output.shape != tradeassembly_plugin_sdk::data_contract::DataShape::Scalar
        || output.time.interval_ms.is_some()
        || !output.time.completed_only
    {
        return Err("order_plan_calculation_ports_invalid".into());
    }
    if !jsonschema::validator_for(&contract.parameter_schema)
        .map_err(|_| "order_plan_parameter_schema_invalid")?
        .is_valid(&p.parameters)
    {
        return Err("order_plan_parameters_invalid".into());
    }
    Ok((graph, selected, pin))
}
fn cash(s: &TradeAssemblyService, f: &Facts) -> Result<(Option<Value>, Ledger, Decimal), String> {
    let old = s.runtime().storage.get_json(CASH, &f.account_key)?;
    let ledger: Ledger = match &old {
        Some(v) => {
            serde_json::from_value(v.clone()).map_err(|_| "order_plan_reservations_invalid")?
        }
        None => Ledger {
            owner_sha256: owner(s)?,
            reservations: BTreeMap::new(),
            gross_limit: None,
        },
    };
    if ledger.owner_sha256 != owner(s)? || ledger.reservations.len() > 256 {
        return Err("order_plan_reservations_unavailable".into());
    }
    let mut reserved = Decimal::ZERO;
    for amount in ledger.reservations.values() {
        if !amount.positive() {
            return Err("order_plan_reservations_invalid".into());
        }
        reserved = reserved.checked_add(*amount)?;
    }
    if reserved.compare(f.available)? == Ordering::Greater {
        return Err("order_plan_cash_unavailable".into());
    }
    Ok((old, ledger, f.available.checked_subtract(reserved)?))
}
fn ledger_policy(ledger: &mut Ledger, p: &Policy) -> Result<(), String> {
    if ledger.reservations.is_empty() {
        ledger.gross_limit = Some(p.max_premium_gross);
        return Ok(());
    }
    if ledger.gross_limit != Some(p.max_premium_gross) {
        return Err("order_plan_account_gross_policy_conflict".into());
    }
    Ok(())
}
fn risk(
    p: &Policy,
    f: &Facts,
    order: &BrokerOrderV2,
    available: Decimal,
) -> Result<Decimal, String> {
    order.validate()?;
    if order.position_action != p.position_action
        || order.time_in_force != p.time_in_force
        || order.instrument.instrument_id != f.metadata.instrument_id
        || order.instrument.provider_symbol != f.metadata.provider_symbol
        || order.instrument.asset_class != f.metadata.asset_class
        || order.instrument.instrument_family != f.metadata.instrument_family
        || order.instrument.quantity_unit != *f.metadata.quantity_unit.required()?
        || order.instrument.multiplier != *f.metadata.multiplier.required()?
        || order.instrument.quote_currency != f.account.currency.code
    {
        return Err("order_plan_proposal_identity_invalid".into());
    }
    if !order
        .quantity
        .is_multiple(*f.metadata.quantity_increment.required()?)?
        || order
            .quantity
            .compare(*f.metadata.minimum_quantity.required()?)?
            == Ordering::Less
    {
        return Err("order_plan_quantity_increment_invalid".into());
    }
    within(order.quantity, p.max_quantity)?;
    let PriceInstruction::Limit { limit_price } = order.pricing else {
        return Err("order_plan_market_pricing_unsupported".into());
    };
    if !limit_price.is_multiple(*f.metadata.price_increment.required()?)? {
        return Err("order_plan_price_increment_invalid".into());
    }
    let ceiling = f
        .quote
        .ask
        .required()?
        .multiply(p.maximum_ask_multiple)?
        .round(12, Rounding::Floor)?;
    within(limit_price, ceiling)?;
    let purchase = order
        .limit_notional(f.account.currency.decimals)?
        .checked_add(p.fee_cash_cap)?;
    within(purchase, p.max_purchase_cash)?;
    within(purchase, available)?;
    let already_reserved = f.available.checked_subtract(available)?;
    let mut gross = *f.account.gross_exposure.required()?;
    if gross.negative() {
        return Err("order_plan_gross_exposure_invalid".into());
    }
    p.fee_cash_cap
        .multiply("1".parse()?)?
        .round(f.account.currency.decimals, Rounding::Exact)?;
    for position in f.account.positions.required()? {
        if position.side == HoldingSide::Short {
            return Err("order_plan_existing_short_risk_unsupported".into());
        }
    }
    for order in f.account.open_orders.required()? {
        if order.position_action == PositionAction::SellToOpen {
            return Err("order_plan_existing_short_risk_unsupported".into());
        }
        let cost = *order.worst_case_cash.required()?;
        if cost.negative() {
            return Err("order_plan_open_order_cost_invalid".into());
        }
        gross = gross.checked_add(cost)?;
    }
    within(
        gross.checked_add(already_reserved)?.checked_add(purchase)?,
        p.max_premium_gross,
    )?;
    Ok(purchase)
}
pub fn prepare(
    s: &TradeAssemblyService,
    c: PrepareCommand,
    ctx: &SideEffectContext,
) -> Result<OrderPlan, String> {
    if ctx.authority.account_mode != "paper" || c.mode != "paper" {
        return Err("order_plan_mode_unsupported".into());
    }
    let p = policy(s, &c)?;
    let key = hash(&(
        owner(s)?,
        &c.scope,
        &ctx.authority,
        ctx.idempotency_key.as_str(),
    ))?;
    if let Some(prior) = s.runtime().storage.get_json(KEYS, &key)? {
        let prior: OrderPlan =
            serde_json::from_value(prior).map_err(|_| "order_plan_record_invalid")?;
        if prior.command != c {
            return Err("order_plan_idempotency_conflict".into());
        }
        let stored = read_owned(s, &prior.plan_id)?;
        if stored != prior || stored.command != c {
            return Err("order_plan_record_invalid".into());
        }
        return Ok(stored);
    }
    if let Some(prior) = original_command(s, &c)? {
        let raw = value(&prior)?;
        s.runtime()
            .storage
            .put_json_if_absent(KEYS, &key, raw.clone(), ctx)?;
        if s.runtime().storage.get_json(KEYS, &key)?.as_ref() != Some(&raw) {
            return Err("order_plan_idempotency_conflict".into());
        }
        return Ok(prior);
    }
    let f = facts(s, &c, &p)?;
    let (ledger_old, ledger, available) = cash(s, &f)?;
    if c.reserve_cash && ledger.reservations.len() >= 256 {
        return Err("order_plan_reservation_capacity".into());
    }
    let (graph, selected, pin) = measured("calculation.pin", || calculation(s, &c, &p, &f))?;
    if s.runtime()
        .storage
        .get_json(ENTRIES, &f.entry_id)?
        .is_some()
    {
        return Err("order_plan_entry_already_prepared".into());
    }
    let input = calc::CalculationInput {
        schema_version: calc::InputVersion::V1,
        account: f.account.clone(),
        metadata: f.metadata.clone(),
        quote: f.quote.clone(),
        available_cash: available,
        parameters: p.parameters.clone(),
    };
    let correlation = hash(&(
        "tradeassembly.order_plan.calculation.v1",
        &key,
        &c,
        &p,
        &pin,
        &input,
    ))?;
    let request = PluginOperationRequest {
        correlation_id: correlation.clone(),
        plugin_instance_ref: selected.plugin_instance_ref.clone(),
        plugin_ref: selected.plugin_ref.clone(),
        manifest_fingerprint: selected.manifest_fingerprint.clone(),
        operation_id: selected.operation_id.clone(),
        capability: selected.operation.capability.clone(),
        capability_graph_revision_id: c.capability_graph_revision.clone(),
        capability_graph_fingerprint: graph.graph_fingerprint,
        strategy_id: c.scope.strategy_id.clone(),
        strategy_version_id: c.scope.strategy_version_id.clone(),
        strategy_spec_hash: c.scope.strategy_spec_hash.clone(),
        activation_id: String::new(),
        attempt_id: correlation.clone(),
        evaluation_tick_id: correlation.clone(),
        mode: c.mode.clone(),
        purpose: selected.operation.purpose.clone(),
        account_ref: None,
        timeout_ms: 15000,
        fencing_token: None,
        input: value(&input)?,
        evidence_refs: f.receipts.values().cloned().collect(),
    };
    let mut call = ctx.clone();
    call.idempotency_key = IdempotencyKey::new(format!("order-plan-calculation:{correlation}"))?;
    let response = s.runtime().plugin_operations.invoke(&request, &call)?;
    if response.correlation_id != correlation
        || response.schema_ref != calc::OUTPUT_SCHEMA
        || response.reconciliation_required
        || response.freshness_state != "fresh"
        || !jsonschema::validator_for(&calc::output_schema())
            .map_err(|_| "order_plan_schema_invalid")?
            .is_valid(&response.payload)
    {
        return Err("order_plan_calculation_response_invalid".into());
    }
    let proposal: calc::CalculationOutput = serde_json::from_value(response.payload.clone())
        .map_err(|_| "order_plan_calculation_response_invalid")?;
    let order = BrokerOrderV2 {
        schema_version: OrderVersion::V2,
        instrument: InstrumentIdentity {
            instrument_id: f.metadata.instrument_id.clone(),
            provider_symbol: f.metadata.provider_symbol.clone(),
            asset_class: f.metadata.asset_class.clone(),
            instrument_family: f.metadata.instrument_family.clone(),
            quantity_unit: f.metadata.quantity_unit.required()?.clone(),
            multiplier: *f.metadata.multiplier.required()?,
            quote_currency: f.account.currency.code.clone(),
        },
        position_action: p.position_action.clone(),
        quantity: proposal.quantity,
        pricing: PriceInstruction::Limit {
            limit_price: proposal.limit_price,
        },
        time_in_force: p.time_in_force.clone(),
        client_order_id: format!("ta-{}", &f.entry_id[..40]),
    };
    let purchase = risk(&p, &f, &order, available)?;
    let fresh = facts(s, &c, &p)?;
    if fresh.receipts != f.receipts
        || fresh.canonical_account != f.canonical_account
        || fresh.account_binding_sha256 != f.account_binding_sha256
        || measured("calculation.pin", || calculation(s, &c, &p, &fresh))?.2 != pin
    {
        return Err("order_plan_sources_changed".into());
    }
    let mut result = OrderPlan {
        plan_id: String::new(),
        entry_id: f.entry_id.clone(),
        command: c,
        policy: p,
        calculation: pin,
        source_receipts: f.receipts,
        account_key: f.account_key,
        canonical_account: f.canonical_account,
        account_binding_sha256: f.account_binding_sha256,
        owner_sha256: owner(s)?,
        prepared_at_ms: s.runtime().clock.now_ms(),
        valid_until_ms: f.valid_until,
        available_cash: available,
        order,
        purchase_cash: purchase,
        calculation_response_sha256: hash(&response)?,
    };
    result.plan_id = format!("orderplan_{}", hash(&result)?);
    persist_prepared(s, &result, &key, ledger_old, ledger, ctx)?;
    Ok(result)
}
fn persist_prepared(
    s: &TradeAssemblyService,
    result: &OrderPlan,
    key: &str,
    ledger_old: Option<Value>,
    mut ledger: Ledger,
    ctx: &SideEffectContext,
) -> Result<(), String> {
    ledger_policy(&mut ledger, &result.policy)?;
    if result.owner_sha256 != owner(s)?
        || result.command.mode != "paper"
        || ctx.authority.account_mode != "paper"
    {
        return Err("order_plan_commit_authority_invalid".into());
    }
    s.require_object("strategy", &result.command.scope.strategy_id)
        .map_err(|_| "order_plan_not_available")?;
    let raw = value(result)?;
    let mut writes = vec![
        StorageWrite::new(PLANS, &result.plan_id, raw.clone(), ctx.clone()),
        StorageWrite::new(KEYS, key, raw, ctx.clone()),
        StorageWrite::new(
            ENTRIES,
            &result.entry_id,
            json!({"planId":result.plan_id}),
            ctx.clone(),
        ),
        StorageWrite::new(
            COMMANDS,
            &hash(&(owner(s)?, &result.command))?,
            json!({"planId":result.plan_id}),
            ctx.clone(),
        ),
    ];
    let mut expectations = vec![
        StorageExpectation::new(PLANS, &result.plan_id, None),
        StorageExpectation::new(RELEASES, &result.plan_id, None),
        StorageExpectation::new(submission::STATE, &result.plan_id, None),
        StorageExpectation::new(KEYS, key, None),
        StorageExpectation::new(ENTRIES, &result.entry_id, None),
        StorageExpectation::new(COMMANDS, &hash(&(owner(s)?, &result.command))?, None),
    ];
    if result.command.reserve_cash {
        ledger
            .reservations
            .insert(result.plan_id.clone(), result.purchase_cash);
        writes.push(StorageWrite::new(
            CASH,
            &result.account_key,
            value(&ledger)?,
            ctx.clone(),
        ));
        expectations.push(StorageExpectation::new(
            CASH,
            &result.account_key,
            ledger_old,
        ));
    }
    if s.runtime().clock.now_ms() >= result.valid_until_ms {
        return Err("order_plan_observation_stale".into());
    }
    if s.runtime()
        .storage
        .put_json_batch(&writes, &expectations)
        .map_err(|_| "order_plan_commit_failed")?
        != ComparePutOutcome::Updated
    {
        return Err("order_plan_commit_conflict".into());
    }
    Ok(())
}
fn original_command(
    s: &TradeAssemblyService,
    c: &PrepareCommand,
) -> Result<Option<OrderPlan>, String> {
    let key = hash(&(owner(s)?, c))?;
    let Some(v) = s.runtime().storage.get_json(COMMANDS, &key)? else {
        return Ok(None);
    };
    let r = read_owned(s, v["planId"].as_str().ok_or("order_plan_record_invalid")?)?;
    if r.command != *c {
        return Err("order_plan_idempotency_conflict".into());
    }
    Ok(Some(r))
}
fn held(s: &TradeAssemblyService, r: &OrderPlan) -> Result<bool, String> {
    if released(s, r)?.is_some() {
        return Ok(false);
    }
    let Some(v) = s.runtime().storage.get_json(CASH, &r.account_key)? else {
        return Ok(false);
    };
    let ledger: Ledger =
        serde_json::from_value(v).map_err(|_| "order_plan_reservations_invalid")?;
    if ledger.owner_sha256 != owner(s)? || ledger.reservations.len() > 256 {
        return Err("order_plan_reservations_unavailable".into());
    }
    match ledger.reservations.get(&r.plan_id) {
        None => Ok(false),
        Some(v) if *v == r.purchase_cash => Ok(true),
        _ => Err("order_plan_reservation_conflict".into()),
    }
}
fn reservation_summary(r: &OrderPlan) -> Value {
    json!({"planId":r.plan_id,"cashReserved":r.purchase_cash,"currentness":"not_checked","alreadyReserved":true,"executionAuthorityGranted":false,"brokerDispatchAllowed":false})
}
fn read_owned(s: &TradeAssemblyService, id: &str) -> Result<OrderPlan, String> {
    if !id.starts_with("orderplan_") || id.len() != 74 {
        return Err("order_plan_not_available".into());
    }
    let r: OrderPlan = serde_json::from_value(
        s.runtime()
            .storage
            .get_json(PLANS, id)?
            .ok_or("order_plan_not_available")?,
    )
    .map_err(|_| "order_plan_record_invalid")?;
    let mut unsigned = r.clone();
    unsigned.plan_id.clear();
    if format!("orderplan_{}", hash(&unsigned)?) != id
        || r.plan_id != id
        || r.owner_sha256 != owner(s)?
    {
        return Err("order_plan_record_invalid".into());
    }
    s.require_object("strategy", &r.command.scope.strategy_id)
        .map_err(|_| "order_plan_not_available")?;
    Ok(r)
}
pub fn current(s: &TradeAssemblyService, id: &str) -> Result<OrderPlan, String> {
    checked_current(s, id).map(|(r, _)| r)
}
fn checked_current(s: &TradeAssemblyService, id: &str) -> Result<(OrderPlan, Facts), String> {
    let p = read_owned(s, id)?;
    // Terminal release and expiry refuse before loading external data lineage.
    // Preserve the legacy refusal ordering even when a signal is unavailable.
    current_record(s, &p)?;
    let signal = verify_signal(s, &p.command)?;
    checked_current_with_signal(s, id, &signal)
}
fn current_record(s: &TradeAssemblyService, r: &OrderPlan) -> Result<Policy, String> {
    if released(s, r)?.is_some() {
        return Err("order_plan_reservation_released".into());
    }
    let p = policy(s, &r.command)?;
    if p != r.policy || s.runtime().clock.now_ms() >= r.valid_until_ms {
        return Err("order_plan_stale".into());
    }
    Ok(p)
}
fn checked_current_with_signal(
    s: &TradeAssemblyService,
    id: &str,
    signal: &VerifiedSignal,
) -> Result<(OrderPlan, Facts), String> {
    let r = read_owned(s, id)?;
    signal.matches(s, &r.command)?;
    let p = current_record(s, &r)?;
    let f = facts_with_signal(s, &r.command, &p, signal)?;
    if r.entry_id != f.entry_id
        || r.source_receipts != f.receipts
        || r.account_key != f.account_key
        || r.canonical_account != f.canonical_account
        || r.account_binding_sha256 != f.account_binding_sha256
        || measured("calculation.pin", || calculation(s, &r.command, &p, &f))?.2 != r.calculation
        || risk(&p, &f, &r.order, r.available_cash)? != r.purchase_cash
    {
        return Err("order_plan_binding_stale".into());
    }
    Ok((r, f))
}
pub fn reserve(
    s: &TradeAssemblyService,
    id: &str,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    if ctx.authority.account_mode != "paper" {
        return Err("order_plan_mode_unsupported".into());
    }
    let original = read_owned(s, id)?;
    submission::reserve_guard(s, &original)?;
    if released(s, &original)?.is_some() {
        return Err("order_plan_reservation_released".into());
    }
    if held(s, &original)? {
        return Ok(reservation_summary(&original));
    }
    for _ in 0..8 {
        let r = current(s, id)?;
        let f = facts(s, &r.command, &r.policy)?;
        let (old, mut ledger, available) = cash(s, &f)?;
        ledger_policy(&mut ledger, &r.policy)?;
        if let Some(amount) = ledger.reservations.get(id) {
            if *amount != r.purchase_cash {
                return Err("order_plan_reservation_conflict".into());
            }
            return Ok(
                json!({"planId":id,"cashReserved":amount,"executionAuthorityGranted":false,"brokerDispatchAllowed":false}),
            );
        }
        if ledger.reservations.len() >= 256 {
            return Err("order_plan_reservation_capacity".into());
        }
        risk(&r.policy, &f, &r.order, available)?;
        ledger.reservations.insert(id.into(), r.purchase_cash);
        if commit_reservation(s, &r, old, &ledger, ctx)? == ComparePutOutcome::Updated {
            return Ok(
                json!({"planId":id,"cashReserved":r.purchase_cash,"executionAuthorityGranted":false,"brokerDispatchAllowed":false}),
            );
        }
    }
    Err("order_plan_reservation_contention".into())
}
fn commit_reservation(
    s: &TradeAssemblyService,
    r: &OrderPlan,
    old: Option<Value>,
    ledger: &Ledger,
    ctx: &SideEffectContext,
) -> Result<ComparePutOutcome, String> {
    s.runtime().storage.put_json_batch(
        &[StorageWrite::new(
            CASH,
            &r.account_key,
            value(ledger)?,
            ctx.clone(),
        )],
        &[
            StorageExpectation::new(CASH, &r.account_key, old),
            StorageExpectation::new(RELEASES, &r.plan_id, None),
            StorageExpectation::new(submission::STATE, &r.plan_id, None),
            StorageExpectation::new(PLANS, &r.plan_id, Some(value(r)?)),
        ],
    )
}
fn command(body: &Value) -> Result<PrepareCommand, String> {
    if serde_json::to_vec(body)
        .map_err(|_| "order_plan_command_invalid")?
        .len()
        > 8192
    {
        return Err("order_plan_command_too_large".into());
    }
    serde_json::from_value(body["orderPlan"].clone())
        .map_err(|_| "order_plan_command_invalid".into())
}
pub(super) fn preflight(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    if path != PREPARE
        && path != RESERVE
        && path != RELEASE
        && path != SUBMIT
        && path != RECONCILE
        && path != DECIDE
        && path != SELECT
    {
        return None;
    }
    let result = (|| {
        let e = crate::control_plane::http_envelope("http", "POST", path, body)?;
        if e.authority.account_mode != "paper" {
            return Err("order_plan_mode_unsupported".into());
        }
        if path == PREPARE {
            let c = command(body)?;
            if ["strategyId", "strategy_id"].iter().any(|k| {
                body.get(*k)
                    .is_some_and(|v| v != &json!(c.scope.strategy_id))
            }) || body.get("scope").is_some_and(|v| v != &json!(c.scope))
                || e.target_object.as_deref() != Some(c.scope.strategy_id.as_str())
            {
                return Err("order_plan_scope_mismatch".into());
            }
            // C3 authorizes the owned planning request. Full current data,
            // graph and artifact checks remain inside prepare before invocation
            // and immediately before its atomic plan/cash commit.
            policy(s, &c)?;
        } else if path == SELECT {
            let c: decision::SelectionCommand = serde_json::from_value(body["selection"].clone())
                .map_err(|_| "typed_selection_command_invalid")?;
            s.require_object("execution_activation", &c.activation_id)
                .map_err(|_| "typed_selection_not_available")?;
            let plan = data_plans::current(s, &c.plan_revision_id)?;
            if body["scope"] != value(&plan.content.scope)?
                || e.target_object.as_deref() != Some(plan.content.scope.strategy_id.as_str())
            {
                return Err("order_plan_scope_mismatch".into());
            }
        } else {
            let plan_id = if path == SUBMIT {
                body["submission"]["planId"].as_str()
            } else if path == DECIDE {
                body["decision"]["planId"].as_str()
            } else {
                body["planId"].as_str()
            }
            .ok_or("order_plan_id_required")?;
            let r = read_owned(s, plan_id)?;
            if e.target_object.as_deref() != Some(r.command.scope.strategy_id.as_str())
                || body["scope"] != value(&r.command.scope)?
            {
                return Err("order_plan_scope_mismatch".into());
            }
        }
        Ok::<_, String>(())
    })();
    result.err().map(|e| ServiceResponse::conflict(&e))
}
pub(super) fn response(s: &TradeAssemblyService, path: &str, body: Value) -> ServiceResponse {
    let result = (|| {
        let e = crate::control_plane::http_envelope("http", "POST", path, &body)?;
        if e.authority.account_mode != "paper" {
            return Err("order_plan_mode_unsupported".into());
        }
        let ctx = SideEffectContext::new(
            e.authority,
            IdempotencyKey::new(
                body["idempotencyKey"]
                    .as_str()
                    .ok_or("order_plan_idempotency_required")?,
            )?,
        )
        .with_agent_execution(s.agent_mcp_execution_context());
        if path == PREPARE {
            let r = prepare(s, command(&body)?, &ctx)?;
            Ok(
                json!({"planId":r.plan_id,"order":r.order,"purchaseCash":r.purchase_cash,"validUntilMs":r.valid_until_ms,"cashReservedAtPreparation":r.command.reserve_cash,"currentness":"not_checked","executionAuthorityGranted":false,"brokerDispatchAllowed":false}),
            )
        } else if path == DECIDE {
            decision::decide(
                s,
                serde_json::from_value(body["decision"].clone())
                    .map_err(|_| "typed_entry_command_invalid")?,
                &ctx,
            )
        } else if path == SELECT {
            decision::select(
                s,
                serde_json::from_value(body["selection"].clone())
                    .map_err(|_| "typed_selection_command_invalid")?,
                &ctx,
            )
        } else if path == RECONCILE {
            transport::reconcile(
                s,
                body["planId"].as_str().ok_or("order_plan_id_required")?,
                body["idempotencyKey"]
                    .as_str()
                    .ok_or("order_plan_idempotency_required")?,
            )
        } else if path == SUBMIT {
            transport::submit(
                s,
                serde_json::from_value(body["submission"].clone())
                    .map_err(|_| "broker_v2_command_invalid")?,
            )
        } else if path == RELEASE {
            release(
                s,
                body["planId"].as_str().ok_or("order_plan_id_required")?,
                &ctx,
            )
        } else {
            reserve(
                s,
                body["planId"].as_str().ok_or("order_plan_id_required")?,
                &ctx,
            )
        }
    })();
    match result {
        Ok(v) => ServiceResponse::ok(v),
        Err(e) => ServiceResponse::conflict(&e),
    }
}

#[cfg(test)]
mod tests;
