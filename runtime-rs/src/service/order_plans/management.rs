//! Owned long positions and immutable synthetic exit proposals. Never a broker permit.
use super::*;
pub(crate) mod automation;
pub mod provider_remote;
pub mod remote;
#[path = "../position_management/rules.rs"]
mod rules;
use rules::ExitRules;
const POSITIONS: &str = "execution_managed_positions_v1";
const TRIGGERS: &str = "execution_managed_exit_triggers_v1";
const JOURNAL: &str = "execution_managed_position_journal_v1";
pub const REGISTER: &str = "/product/strategies/positions/register";
pub const ADVANCE: &str = "/product/strategies/positions/advance";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseBinding {
    pub execution_occurrence_id: String,
    pub position_id: String,
    pub trigger_sha256: String,
    pub exit_policy_sha256: String,
    pub original_entry_plan_id: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExitPolicy {
    entry_basis: String,
    rules: ExitRules,
    fee_cash_cap: Decimal,
    max_limit_slippage_ratio: Decimal,
    time_in_force: String,
    max_reversal_age_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Position {
    schema_version: String,
    position_id: String,
    owner_sha256: String,
    entry_plan_id: String,
    activation_id: String,
    entry_settlement_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry_observation_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry_cash_reconciled: Option<bool>,
    policy: ExitPolicy,
    quantity: Decimal,
    closed_quantity: Decimal,
    basis: Decimal,
    entry_time: ObservationTime,
    high_water: Option<Decimal>,
    last_quote_known_at_ms: i64,
    last_quote_observed_at_ms: i64,
    last_reversal_receipt: Option<String>,
    reversal_watermark: Option<ReversalWatermark>,
    last_quote_receipt: Option<String>,
    reservations: BTreeMap<String, Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provisional_closes: Option<BTreeMap<String, AppliedClose>>,
    generation: u64,
    state_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppliedClose {
    quantity: Decimal,
    receipt_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Trigger {
    schema_version: String,
    position_id: String,
    policy_sha256: String,
    quote_receipt_sha256: String,
    reversal_receipt_sha256: Option<String>,
    price: Decimal,
    high_water: Decimal,
    reason: rules::Trigger,
    known_at_ms: i64,
    trigger_sha256: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterCommand {
    pub entry_plan_id: String,
    pub activation_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvanceCommand {
    pub execution_occurrence_id: String,
    pub position_id: String,
    pub account_collection_id: String,
    pub metadata_collection_id: String,
    pub quote_collection_id: String,
    pub reversal_signal_receipt_sha256: Option<String>,
}
fn seal(p: &mut Position) -> Result<(), String> {
    p.state_sha256.clear();
    p.state_sha256 = hash(p)?;
    Ok(())
}
fn journal(p: &Position, event: &str, receipt: &str) -> Value {
    let mut event = json!({"schemaVersion":"tradeassembly.position_journal.v1","positionId":p.position_id,"entryPlanId":p.entry_plan_id,"event":event,"receiptSha256":receipt,"quantity":p.quantity,"closedQuantity":p.closed_quantity,"highWater":p.high_water,"generation":p.generation,"historicalOutcome":true,"executionAuthorityGranted":false});
    if p.entry_observation_sha256.is_some() {
        event["entryObservationSha256"] = json!(p.entry_observation_sha256);
        event["entryCashReconciled"] = json!(false);
        event["entryBasis"] = json!(p.policy.entry_basis);
    }
    event
}
fn load(s: &TradeAssemblyService, id: &str) -> Result<(Value, Position), String> {
    let raw = s
        .runtime()
        .storage
        .get_json(POSITIONS, id)?
        .ok_or("managed_position_not_available")?;
    let p: Position =
        serde_json::from_value(raw.clone()).map_err(|_| "managed_position_invalid")?;
    let mut check = p.clone();
    seal(&mut check)?;
    let observed = p.policy.entry_basis == "provider_reported_average_fill";
    let entry_type_valid = if observed {
        p.schema_version == "tradeassembly.managed_position.v2"
            && p.entry_settlement_sha256.is_empty()
            && p.entry_observation_sha256
                .as_ref()
                .is_some_and(|v| v.len() == 64)
            && p.entry_cash_reconciled == Some(false)
    } else {
        p.schema_version == "tradeassembly.managed_position.v1"
            && p.entry_observation_sha256.is_none()
            && p.entry_cash_reconciled.is_none()
    };
    if !entry_type_valid
        || p.position_id != id
        || p.owner_sha256 != owner(s)?
        || check != p
        || p.generation > 10000
        || p.closed_quantity.negative()
        || p.closed_quantity.compare(p.quantity)?.is_gt()
        || !p.basis.positive()
        || p.reservations.len() > 256
        || p.provisional_closes.as_ref().is_some_and(|m| {
            m.len() > 256
                || m.values()
                    .any(|q| q.quantity.negative() || q.receipt_sha256.len() != 64)
        })
    {
        return Err("managed_position_invalid".into());
    }
    let entry = read_owned(s, &p.entry_plan_id)?;
    if entry.management.is_some() {
        return Err("managed_position_entry_invalid".into());
    }
    s.require_object("execution_activation", &p.activation_id)
        .map_err(|_| "managed_position_not_available")?;
    Ok((raw, p))
}
pub(crate) fn policy(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
) -> Result<Option<ExitPolicy>, String> {
    s.require_object("strategy", &scope.strategy_id)
        .map_err(|_| "managed_exit_not_available")?;
    let v = strategy::strategy_versions(s, &scope.strategy_id)
        .into_iter()
        .find(|v| {
            v["id"] == scope.strategy_version_id
                && v["specHash"] == scope.strategy_spec_hash
                && v["immutable"] == true
        })
        .ok_or("managed_exit_published_required")?;
    let config = &v["spec"]["stages"]["exit_policy"]["config"]["tradeassembly.managed_exit.v1"];
    if config.is_null() {
        return Ok(None);
    }
    let p: ExitPolicy =
        serde_json::from_value(config.clone()).map_err(|_| "managed_exit_policy_invalid")?;
    p.rules.validate()?;
    if !matches!(
        p.entry_basis.as_str(),
        "provider_final_gross_excluding_fees" | "provider_reported_average_fill"
    ) || v["spec"]["stages"]["exit_policy"]["enabled"] != true
        || v["spec"]["stages"]["exit_policy"]["policy"]["monitoring"] != json!(["synthetic"])
        || !p.fee_cash_cap.positive()
        || p.max_limit_slippage_ratio.negative()
        || p.max_limit_slippage_ratio.compare("1".parse()?)?.is_ge()
        || !matches!(p.time_in_force.as_str(), "day" | "gtc")
        || !(1..=30000).contains(&p.max_reversal_age_ms)
        || p.rules
            .reversal_signal
            .as_deref()
            .is_some_and(|v| v != "typed.exit")
    {
        return Err("managed_exit_policy_unsupported".into());
    }
    let declared = &v["spec"]["stages"]["exit_policy"]["policy"];
    let precedence =
        serde_json::to_value(&p.rules.precedence).map_err(|_| "managed_exit_policy_invalid")?;
    let mut ids = declared["triggers"]
        .as_array()
        .ok_or("managed_exit_triggers_missing")?
        .iter()
        .map(|t| {
            if t["scope"] != "full_position"
                || t.get("expression").is_some()
                || t.get("capability_requirement_ref").is_some()
            {
                return Err("managed_exit_trigger_unsupported".to_string());
            }
            let id = t["id"].as_str().ok_or("managed_exit_trigger_invalid")?;
            let kind = if id == "reversal" {
                "indicator"
            } else {
                "price"
            };
            if t["kind"] != kind {
                return Err("managed_exit_trigger_unsupported".into());
            }
            Ok(id.to_owned())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut expected = p
        .rules
        .precedence
        .iter()
        .map(|x| {
            serde_json::to_value(x)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    ids.sort();
    expected.sort();
    if declared["composition"] != "priority"
        || declared["precedence"] != precedence
        || ids != expected
    {
        return Err("managed_exit_declaration_mismatch".into());
    }
    Ok(Some(p))
}
pub(crate) fn validate_unsettled_provider(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    manifest: &Value,
) -> Result<(), String> {
    if let Some(p) = policy(s, scope)? {
        let entry = super::provisional::selected_manifest(
            manifest,
            crate::broker_submission::v2::SUBMIT_OPERATION,
            tradeassembly_plugin_sdk::settlement_observation_v2::Role::Entry,
        )?;
        if p.entry_basis == "provider_reported_average_fill" {
            if !entry
                || !super::provisional::selected_manifest(
                    manifest,
                    crate::broker_submission::v2::SUBMIT_OPERATION,
                    tradeassembly_plugin_sdk::settlement_observation_v2::Role::Close,
                )?
            {
                return Err("managed_unsettled_provider_contract_required".into());
            }
        } else if entry {
            return Err("managed_unsettled_basis_opt_in_required".into());
        }
    }
    Ok(())
}
// Reported gross fills cannot establish crypto inventory after received-asset
// fees. No net-ownership producer/reducer is admitted by this contract yet.
pub(crate) fn require_reported_inventory_family(family: &str) -> Result<(), String> {
    if family == "crypto_spot" {
        return Err("managed_order_attributed_net_inventory_required".into());
    }
    Ok(())
}
pub(crate) fn validate_inventory_readiness(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    family: &str,
) -> Result<(), String> {
    if policy(s, scope)?.is_some_and(|p| p.entry_basis == "provider_reported_average_fill") {
        require_reported_inventory_family(family)?;
    }
    Ok(())
}
pub(crate) fn validate_automatic_policy(policy: &ExitPolicy) -> Result<(), String> {
    if policy.rules.reversal_signal.is_some() {
        return Err("management_automation_reversal_evidence_unsupported".into());
    }
    Ok(())
}
pub(crate) fn management_run(s: &TradeAssemblyService, id: &str) -> Result<(Value, Value), String> {
    s.require_object("execution_activation", id)
        .map_err(|_| "managed_activation_not_available")?;
    let r = s
        .runtime()
        .storage
        .get_json("execution_runs", &format!("run_{id}"))?
        .ok_or("managed_run_missing")?;
    let a = s
        .runtime()
        .storage
        .get_json("execution_activations", id)?
        .ok_or("managed_activation_missing")?;
    let c = s
        .runtime()
        .storage
        .get_json(
            "execution_configs",
            r["configId"].as_str().ok_or("managed_config_missing")?,
        )?
        .ok_or("managed_config_missing")?;
    crate::service::execution::typed::current(s, &c)?;
    if r["state"] != "active"
        || a["state"] != "active"
        || r["configId"] != a["configId"]
        || r["strategySpecHash"] != c["strategySpecHash"]
        || r["capabilityGraphRevisionId"] != c["capabilityGraphRevisionId"]
    {
        return Err("managed_activation_stale".into());
    }
    for ns in ["execution_controls", "execution_reconciliation"] {
        let v = s
            .runtime()
            .storage
            .get_json(ns, id)?
            .ok_or("managed_controls_missing")?;
        if v["activationId"] != id
            || crate::broker_submission::management_controls_blocked(&v)
            || (ns == "execution_reconciliation" && v["state"] != "clear")
        {
            return Err("managed_controls_blocked".into());
        }
    }
    Ok((r, c))
}
pub fn register(
    s: &TradeAssemblyService,
    c: RegisterCommand,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    if ctx.authority.account_mode != "paper" {
        return Err("managed_paper_required".into());
    }
    let entry = read_owned(s, &c.entry_plan_id)?;
    let (_, original) = submission::record(s, &entry)?.ok_or("managed_entry_submission_missing")?;
    if original.intent["originalRequest"]["activationId"] != c.activation_id {
        return Err("managed_activation_scope_mismatch".into());
    }
    let (_, config) = management_run(s, &c.activation_id)?;
    if config["strategyId"] != entry.command.scope.strategy_id
        || config["strategyVersionId"] != entry.command.scope.strategy_version_id
        || config["capabilityGraphRevisionId"] != entry.command.capability_graph_revision
        || config["canonicalAccount"]
            != value(
                entry
                    .canonical_account
                    .as_ref()
                    .ok_or("managed_account_required")?,
            )?
        || config["strategySpecHash"] != entry.command.scope.strategy_spec_hash
    {
        return Err("managed_activation_scope_mismatch".into());
    }
    let policy = policy(s, &entry.command.scope)?.ok_or("managed_exit_policy_required")?;
    let (quantity, basis, receipt, entry_time) = entry_basis(s, &entry, &policy, None)?;
    let id = format!("position_{}", hash(&(owner(s)?, &entry.plan_id))?);
    if s.runtime().storage.get_json(POSITIONS, &id)?.is_some() {
        return inspect(s, &id);
    }
    let observed = policy.entry_basis == "provider_reported_average_fill";
    let mut p = Position {
        schema_version: if observed {
            "tradeassembly.managed_position.v2"
        } else {
            "tradeassembly.managed_position.v1"
        }
        .into(),
        position_id: id.clone(),
        owner_sha256: owner(s)?,
        entry_plan_id: entry.plan_id.clone(),
        activation_id: c.activation_id,
        entry_settlement_sha256: if observed {
            String::new()
        } else {
            receipt.clone()
        },
        entry_observation_sha256: observed.then(|| receipt.clone()),
        entry_cash_reconciled: observed.then_some(false),
        policy,
        quantity,
        closed_quantity: Decimal::ZERO,
        basis,
        entry_time,
        high_water: None,
        last_quote_known_at_ms: 0,
        last_quote_observed_at_ms: 0,
        last_reversal_receipt: None,
        reversal_watermark: None,
        last_quote_receipt: None,
        reservations: BTreeMap::new(),
        provisional_closes: None,
        generation: 0,
        state_sha256: String::new(),
    };
    seal(&mut p)?;
    if s.runtime().storage.put_json_batch(
        &[
            StorageWrite::new(POSITIONS, &id, value(&p)?, ctx.clone()),
            StorageWrite::new(
                JOURNAL,
                format!("{id}:0"),
                journal(&p, "entry_fill_registered", &receipt),
                ctx.clone(),
            ),
        ],
        &[StorageExpectation::new(POSITIONS, &id, None)],
    )? != ComparePutOutcome::Updated
    {
        return Err("managed_registration_conflict".into());
    }
    inspect(s, &id)
}
/// Current host witness for a computation-only managed exit plan. No caller
/// supplies the traded instrument, account, activation or executable authority.
pub(in crate::service) fn exit_plan_subject(
    s: &TradeAssemblyService,
    id: &str,
    scope: &data_plans::PlanScope,
) -> Result<(crate::data_pipeline::InstrumentContext, String, String), String> {
    let (_, pos) = load(s, id)?;
    let entry = read_owned(s, &pos.entry_plan_id)?;
    if entry.command.scope != *scope
        || entry.command.mode != "paper"
        || !pos
            .quantity
            .checked_subtract(pos.closed_quantity)?
            .positive()
    {
        return Err("managed_plan_scope_or_remaining_lot_invalid".into());
    }
    let (_, config) = management_run(s, &pos.activation_id)?;
    if config["strategyId"] != scope.strategy_id
        || config["strategyVersionId"] != scope.strategy_version_id
        || config["strategySpecHash"] != scope.strategy_spec_hash
        || config["capabilityGraphRevisionId"] != entry.command.capability_graph_revision
        || config["canonicalAccount"]
            != value(
                entry
                    .canonical_account
                    .as_ref()
                    .ok_or("managed_account_required")?,
            )?
        || policy(s, scope)?.as_ref() != Some(&pos.policy)
        || pos.policy.rules.reversal_signal.as_deref() != Some("typed.exit")
    {
        return Err("managed_plan_activation_or_policy_invalid".into());
    }
    let (_, original) = submission::record(s, &entry)?.ok_or("managed_entry_submission_missing")?;
    if original.intent["originalRequest"]["activationId"] != pos.activation_id {
        return Err("managed_plan_activation_invalid".into());
    }
    verify_entry(s, &entry, &pos)?;
    let (quantity, basis, _, _) = entry_basis(s, &entry, &pos.policy, Some(entry_evidence(&pos)))?;
    if quantity != pos.quantity || basis != pos.basis {
        return Err("managed_plan_entry_basis_changed".into());
    }
    let source = settlement::current_source(s, &entry, &original)?;
    let instrument = crate::data_pipeline::InstrumentContext {
        instrument_id: entry.order.instrument.instrument_id.clone(),
        asset_class: entry.order.instrument.asset_class.clone(),
        instrument_family: entry.order.instrument.instrument_family.clone(),
    };
    // High water, closed quantity, reservations and leases are mutable checks,
    // not source identity. Settled entry, account, policy and source are pinned.
    let mut source_proof = json!({"schemaVersion":"tradeassembly.managed_exit_plan_source.v1",
        "owner":owner(s)?,"positionId":pos.position_id,"entryPlanSha256":hash(&entry)?,
        "entrySettlementSha256":pos.entry_settlement_sha256,"activationId":pos.activation_id,
        "policySha256":hash(&pos.policy)?,"configSha256":hash(&config)?,"source":source});
    if let Some(observation) = &pos.entry_observation_sha256 {
        source_proof
            .as_object_mut()
            .unwrap()
            .remove("entrySettlementSha256");
        source_proof["schemaVersion"] = json!("tradeassembly.managed_exit_plan_source.v2");
        source_proof["entryObservationSha256"] = json!(observation);
        source_proof["entryBasis"] = json!(pos.policy.entry_basis);
    }
    let proof = hash(&source_proof)?;
    Ok((instrument, entry.command.capability_graph_revision, proof))
}
/// Resolve the immutable entry computation only after proving current ownership
/// of its remaining settled position. This is computation lineage, never entry
/// eligibility, selector freshness, or an executable permission.
pub(in crate::service) fn entry_signal_source(
    s: &TradeAssemblyService,
    id: &str,
    scope: &data_plans::PlanScope,
) -> Result<
    (
        data_plans::StoredDataPlan,
        data_evaluation::EvaluationReceipt,
        String,
    ),
    String,
> {
    let (instrument, graph, proof) = exit_plan_subject(s, id, scope)?;
    let (_, pos) = load(s, id)?;
    let entry = read_owned(s, &pos.entry_plan_id)?;
    let (plan, receipt) =
        data_evaluation::historical_indexed_receipt(s, &entry.command.signal_receipt_sha256)?;
    let signal = receipt
        .signals
        .last()
        .ok_or("managed_stream_entry_signal_missing")?;
    if plan.content.scope != *scope
        || plan.content.plan.request.mode != "paper"
        || plan.content.plan.request.capability_graph_revision != graph
        || receipt.receipt_sha256 != entry.command.signal_receipt_sha256
        || signal.trade_instruments != vec![instrument]
        || signal.entry != (crate::data_strategy::SignalValue::Ready { value: true })
        || plan
            .content
            .selection_source
            .as_ref()
            .is_some_and(|source| source.managed_position_id.is_some())
    {
        return Err("managed_stream_entry_signal_binding_invalid".into());
    }
    Ok((plan, receipt, proof))
}

/// Stable owned position/entry lineage; no active-control or calendar authority.
pub(in crate::service) fn window_lineage(
    s: &TradeAssemblyService,
    id: &str,
) -> Result<crate::service::management_windows::Lineage, String> {
    let (_, pos) = load(s, id)?;
    let entry = read_owned(s, &pos.entry_plan_id)?;
    if entry.management.is_some() || entry.command.mode != "paper" {
        return Err("management_window_entry_invalid".into());
    }
    let (_, original) = submission::record(s, &entry)?.ok_or("managed_entry_submission_missing")?;
    if original.intent["originalRequest"]["activationId"] != pos.activation_id {
        return Err("management_window_activation_mismatch".into());
    }
    verify_entry(s, &entry, &pos)?;
    let (quantity, basis, _, _) = entry_basis(s, &entry, &pos.policy, Some(entry_evidence(&pos)))?;
    if quantity != pos.quantity || basis != pos.basis {
        return Err("management_window_entry_basis_changed".into());
    }
    let entry_evidence_sha256 = entry_evidence(&pos).to_owned();
    Ok(crate::service::management_windows::Lineage {
        owner_sha256: owner(s)?,
        position_id: pos.position_id,
        original_entry_plan_id: entry.plan_id,
        activation_id: pos.activation_id,
        scope: entry.command.scope,
        capability_graph_revision: entry.command.capability_graph_revision,
        exit_policy_sha256: hash(&pos.policy)?,
        entry_settlement_sha256: entry_evidence_sha256,
    })
}
/// Fresh owner/activation/config/source witness for new management windows.
pub(in crate::service) fn window_subject(
    s: &TradeAssemblyService,
    id: &str,
) -> Result<crate::service::management_windows::Subject, String> {
    let lineage = window_lineage(s, id)?;
    let (_, pos) = load(s, id)?;
    if !pos
        .quantity
        .checked_subtract(pos.closed_quantity)?
        .positive()
    {
        return Err("management_window_remaining_lot_required".into());
    }
    let entry = read_owned(s, &lineage.original_entry_plan_id)?;
    let (_, config) = management_run(s, &lineage.activation_id)?;
    if config["strategyId"] != lineage.scope.strategy_id
        || config["strategyVersionId"] != lineage.scope.strategy_version_id
        || config["strategySpecHash"] != lineage.scope.strategy_spec_hash
        || config["capabilityGraphRevisionId"] != lineage.capability_graph_revision
        || config["canonicalAccount"]
            != value(
                entry
                    .canonical_account
                    .as_ref()
                    .ok_or("managed_account_required")?,
            )?
        || policy(s, &lineage.scope)?.as_ref() != Some(&pos.policy)
    {
        return Err("management_window_activation_or_policy_invalid".into());
    }
    let (_, original) = submission::record(s, &entry)?.ok_or("managed_entry_submission_missing")?;
    let source = settlement::current_source(s, &entry, &original)?;
    Ok(crate::service::management_windows::Subject {
        lineage,
        config_sha256: hash(&config)?,
        source_binding_sha256: hash(&source)?,
    })
}

pub fn inspect(s: &TradeAssemblyService, id: &str) -> Result<Value, String> {
    let (_, p) = load(s, id)?;
    Ok(
        json!({"position":p,"journal":s.runtime().storage.list_json(JOURNAL)?.into_iter().filter(|(k,_)|k.starts_with(&format!("{id}:"))).map(|(_,v)|v).collect::<Vec<_>>(),"executionAuthorityGranted":false}),
    )
}
struct Current {
    account: AccountSnapshot,
    metadata: InstrumentMetadata,
    quote: Quote,
    receipts: BTreeMap<String, String>,
    valid_until: i64,
}
fn observations(
    s: &TradeAssemblyService,
    p: &Position,
    entry: &OrderPlan,
    c: &PrepareCommand,
) -> Result<Current, String> {
    let _timing = crate::service::order_plans::financial_timing("management.observations");
    crate::service::package_validation::with_round(s, |s| observations_in_round(s, p, entry, c))
}
fn observations_in_round(
    s: &TradeAssemblyService,
    p: &Position,
    entry: &OrderPlan,
    c: &PrepareCommand,
) -> Result<Current, String> {
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
        if entry.policy.source_nodes.get(name) != Some(&r.command.acquisition_node) {
            return Err("managed_source_selection_mismatch".into());
        }
    }
    let a: AccountSnapshot = serde_json::from_value(ar.response.payload.clone())
        .map_err(|_| "managed_account_invalid")?;
    let m: InstrumentMetadata = serde_json::from_value(mr.response.payload.clone())
        .map_err(|_| "managed_metadata_invalid")?;
    let q: Quote =
        serde_json::from_value(qr.response.payload.clone()).map_err(|_| "managed_quote_invalid")?;
    a.validate()?;
    m.validate()?;
    q.validate()?;
    let now = s.runtime().clock.trusted_now_ms()?;
    let mut until = i64::MAX;
    for (t, r) in [(&a.time, &ar), (&m.time, &mr), (&q.time, &qr)] {
        until = until.min(observation(
            now,
            t,
            r.host_known_at_ms,
            p.policy.rules.max_quote_age_ms,
        )?)
    }
    let runtime = s.runtime();
    let witness = crate::broker_submission::account_identity::current(
        runtime.plugins.as_ref(),
        runtime.plugin_packages.as_ref(),
        runtime.credentials.as_ref(),
        &json!({"pluginInstanceRef":ar.acquisition.plugin_instance_ref,"pluginRef":ar.acquisition.plugin_ref,"manifestFingerprint":ar.acquisition.manifest_sha256,"accountRef":ar.acquisition.account_ref}),
        now,
        "paper",
    )?;
    if Some(&witness.identity) != entry.canonical_account.as_ref()
        || witness.identity.provider_account_id != a.provider_account_id
        || witness.account_ref != a.account_ref
        || a.mode != TradingMode::Paper
        || !a.coverage.account_complete
        || !a.coverage.positions_complete
        || !a.coverage.open_orders_complete
        || a.currency != q.currency
        || Some(&a.currency) != entry.account_currency.as_ref()
        || q.origin != QuoteOrigin::Provider
        || *q.indicative.required()?
        || !entry.policy.quote_profiles.contains(&q.source_profile)
        || m.instrument_id != entry.order.instrument.instrument_id
        || q.instrument_id != m.instrument_id
        || m.provider_symbol != entry.order.instrument.provider_symbol
        || m.asset_class != entry.order.instrument.asset_class
        || m.instrument_family != entry.order.instrument.instrument_family
        || m.multiplier.required()? != &entry.order.instrument.multiplier
        || m.quantity_unit.required()? != &entry.order.instrument.quantity_unit
        || m.quote_currency.required()? != &a.currency
        || !*m.tradable.required()?
    {
        return Err("managed_evidence_binding_invalid".into());
    }
    if m.expires_at_ms.required()?.is_some_and(|e| e <= now)
        || m.quantity_unit.required()? == &QuantityUnit::Contracts
            && !*m.standard_deliverable.required()?
    {
        return Err("managed_instrument_unavailable".into());
    }
    if !a.instrument_permissions.iter().any(|x| {
        x.asset_class == m.asset_class
            && x.instrument_family == m.instrument_family
            && x.actions
                .required()
                .is_ok_and(|xs| xs.contains(&PositionAction::SellToClose))
    }) {
        return Err("managed_close_permission_missing".into());
    }
    let mut held = Decimal::ZERO;
    for pos in a.positions.required()? {
        if pos.instrument_id == m.instrument_id {
            if pos.side != HoldingSide::Long {
                return Err("managed_short_position_unsupported".into());
            }
            held = held.checked_add(pos.quantity)?
        }
    }
    let remaining = p.quantity.checked_subtract(p.closed_quantity)?;
    if held.compare(remaining)?.is_lt() {
        return Err("managed_position_quantity_unavailable".into());
    }
    if a.time.observed_at_ms < p.entry_time.observed_at_ms
        || a.time.known_at_ms < p.entry_time.known_at_ms
    {
        return Err("managed_account_predates_entry".into());
    }
    let mut attributed = remaining;
    for (id, raw) in s.runtime().storage.list_json(POSITIONS)? {
        if id != p.position_id && raw["ownerSha256"] == p.owner_sha256 {
            let (_, other) = load(s, &id)?;
            let original = read_owned(s, &other.entry_plan_id)?;
            if original.account_key == entry.account_key
                && original.order.instrument.instrument_id == m.instrument_id
            {
                attributed = attributed
                    .checked_add(other.quantity.checked_subtract(other.closed_quantity)?)?;
            }
        }
    }
    if attributed.compare(held)?.is_gt() {
        return Err("managed_owned_lots_exceed_account".into());
    }
    // Unknown or unrelated pending closes cannot be assumed safe to overlap.
    for o in a.open_orders.required()? {
        if o.instrument_id == m.instrument_id && o.position_action == PositionAction::SellToClose {
            return Err("managed_existing_close_requires_reconciliation".into());
        }
    }
    funding_capacity(&a)?;
    Ok(Current {
        account: a,
        metadata: m,
        quote: q,
        receipts: BTreeMap::from([
            ("account".into(), ar.receipt_sha256),
            ("metadata".into(), mr.receipt_sha256),
            ("quote".into(), qr.receipt_sha256),
        ]),
        valid_until: until,
    })
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReversalWatermark {
    receipt_sha256: String,
    event_end_ms: i64,
    known_at_ms: i64,
    arrived_at_ms: i64,
}
struct ReadyReversal {
    value: bool,
    watermark: ReversalWatermark,
}
fn causal_reversal(
    entry: &ObservationTime,
    previous: Option<&ReversalWatermark>,
    next: &ReversalWatermark,
) -> Result<(), String> {
    if next.event_end_ms < entry.observed_at_ms
        || next.known_at_ms < entry.known_at_ms
        || next.arrived_at_ms < entry.known_at_ms
    {
        return Err("managed_reversal_predates_entry".into());
    }
    if previous.is_some_and(|old| {
        next.event_end_ms < old.event_end_ms
            || next.known_at_ms < old.known_at_ms
            || next.arrived_at_ms < old.arrived_at_ms
            || next.event_end_ms == old.event_end_ms && next.receipt_sha256 != old.receipt_sha256
    }) {
        return Err("managed_reversal_revision_requires_replay".into());
    }
    Ok(())
}
// Shared by strict execution validation and read-only idle classification.
// Current indexed receipt integrity/ownership/source, exact scope/lineage and
// actual-now subject/clock checks are mandatory even for a no-action wait.
fn reversal_frame(
    s: &TradeAssemblyService,
    p: &Position,
    entry: &OrderPlan,
    id: &str,
    now: i64,
) -> Result<crate::data_strategy::SignalFrame, String> {
    let (plan, r) = data_evaluation::indexed_receipt(s, id)?;
    if plan.content.scope != entry.command.scope || r.receipt_sha256 != id {
        return Err("managed_reversal_scope_mismatch".into());
    }
    if plan
        .content
        .selection_source
        .as_ref()
        .and_then(|source| source.managed_position_id.as_deref())
        .is_some_and(|id| id != p.position_id)
    {
        return Err("managed_reversal_position_lineage_mismatch".into());
    }
    let frame = r.signals.last().ok_or("managed_reversal_not_ready")?;
    validate_reversal_subject(frame, &entry.order.instrument.instrument_id, now)?;
    Ok(frame.clone())
}
fn validate_reversal_subject(
    frame: &crate::data_strategy::SignalFrame,
    instrument: &str,
    now: i64,
) -> Result<(), String> {
    if frame.trade_instruments.len() != 1
        || frame.trade_instruments[0].instrument_id != instrument
        || frame.known_at_ms > now
        || frame.arrived_at_ms > now
        || frame.event_end_ms > now
    {
        return Err("managed_reversal_not_current".into());
    }
    Ok(())
}
fn reversal_watermark(id: &str, frame: &crate::data_strategy::SignalFrame) -> ReversalWatermark {
    ReversalWatermark {
        receipt_sha256: id.into(),
        event_end_ms: frame.event_end_ms,
        known_at_ms: frame.known_at_ms,
        arrived_at_ms: frame.arrived_at_ms,
    }
}
fn ready_reversal(
    frame: &crate::data_strategy::SignalFrame,
    id: &str,
    entry: &ObservationTime,
    previous: Option<&ReversalWatermark>,
    now: i64,
    max_age_ms: i64,
) -> Result<ReadyReversal, String> {
    if now - frame.event_end_ms >= max_age_ms {
        return Err("managed_reversal_not_current".into());
    }
    let watermark = reversal_watermark(id, frame);
    causal_reversal(entry, previous, &watermark)?;
    match frame.exit {
        crate::data_strategy::SignalValue::Ready { value } => {
            Ok(ReadyReversal { value, watermark })
        }
        crate::data_strategy::SignalValue::WarmingUp => Err("managed_reversal_not_ready".into()),
    }
}
fn reversal(
    s: &TradeAssemblyService,
    p: &Position,
    entry: &OrderPlan,
    id: Option<&str>,
    now: i64,
) -> Result<Option<ReadyReversal>, String> {
    if p.policy.rules.reversal_signal.is_none() {
        if id.is_some() {
            return Err("managed_unexpected_reversal".into());
        }
        return Ok(None);
    }
    let id = id.ok_or("managed_reversal_required")?;
    let frame = reversal_frame(s, p, entry, id, now)?;
    ready_reversal(
        &frame,
        id,
        &p.entry_time,
        p.reversal_watermark.as_ref(),
        now,
        p.policy.max_reversal_age_ms,
    )
    .map(Some)
}

// This classification can only suppress work, never return a trading signal.
// TTL is irrelevant only to a consumed false reversal-only observation or initial
// bootstrap before entry. With any prior management watermark/consumption,
// pre-entry evidence remains a causal refusal, not another bootstrap.
// True retries, independent price work and unconsumed post-entry evidence still
// use strict reversal().
fn idle_reversal_frame(
    frame: &crate::data_strategy::SignalFrame,
    id: &str,
    entry: &ObservationTime,
    previous: Option<&ReversalWatermark>,
    last_consumed: Option<&str>,
    price_exits: bool,
) -> Result<bool, String> {
    if !matches!(frame.exit, crate::data_strategy::SignalValue::Ready { .. }) {
        return Err("managed_reversal_not_ready".into());
    }
    match causal_reversal(entry, previous, &reversal_watermark(id, frame)) {
        // A consumed false signal needs no further reversal-only monitoring.
        // A still-true signal can be needed after an unfilled close, and price
        // rules require ordinary fresh quote/Advance work even when false.
        Ok(()) => Ok(last_consumed == Some(id)
            && matches!(
                frame.exit,
                crate::data_strategy::SignalValue::Ready { value: false }
            )
            && !price_exits),
        Err(code)
            if code == "managed_reversal_predates_entry"
                && previous.is_none()
                && last_consumed.is_none() =>
        {
            Ok(true)
        }
        Err(code) => Err(code),
    }
}
fn idle_reversal_observation(
    s: &TradeAssemblyService,
    p: &Position,
    entry: &OrderPlan,
    id: &str,
    now: i64,
) -> Result<bool, String> {
    let frame = reversal_frame(s, p, entry, id, now)?;
    idle_reversal_frame(
        &frame,
        id,
        &p.entry_time,
        p.reversal_watermark.as_ref(),
        p.last_reversal_receipt.as_deref(),
        p.policy.rules.stop_loss_ratio.is_some() || p.policy.rules.trailing_ratio.is_some(),
    )
}

pub fn advance(
    s: &TradeAssemblyService,
    c: AdvanceCommand,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let _timing = crate::service::order_plans::financial_timing("management.advance");
    if ctx.authority.account_mode != "paper" {
        return Err("managed_paper_required".into());
    }
    let (old, mut pos) = load(s, &c.position_id)?;
    assert_provisional_close_evidence(s, &pos)?;
    let entry = read_owned(s, &pos.entry_plan_id)?;
    verify_entry(s, &entry, &pos)?;
    let (_, config) = management_run(s, &pos.activation_id)?;
    if policy(s, &entry.command.scope)?.as_ref() != Some(&pos.policy) {
        return Err("managed_policy_changed".into());
    }
    if !pos.reservations.is_empty() {
        if pos.reservations.len() != 1 {
            return Err("managed_multiple_pending_closes_invalid".into());
        }
        let id = pos
            .reservations
            .keys()
            .next()
            .ok_or("managed_pending_close_missing")?;
        let plan = read_owned(s, id)?;
        let state = submission::record(s, &plan)?;
        if state.is_none() {
            if s.runtime().clock.trusted_now_ms()? >= plan.valid_until_ms {
                return abort_unsent(s, &plan, &old, &pos, None, ctx);
            }
            let decision = decide(s, &plan, &pos, &config, ctx)?;
            return Ok(
                json!({"position":pos,"decision":decision,"resumedReservedClose":true,"executionAuthorityGranted":false}),
            );
        }
        if let Some((raw, record)) = state {
            if record.phase == submission::Phase::Denied {
                return abort_unsent(s, &plan, &old, &pos, Some(raw), ctx);
            }
            return Ok(
                json!({"position":pos,"status":"pending_close_requires_reconciliation","submission":submission::summary(record),"executionAuthorityGranted":false}),
            );
        }
    }
    let quantity = pos.quantity.checked_subtract(pos.closed_quantity)?;
    if !quantity.positive() {
        return inspect(s, &pos.position_id);
    }
    let mut command = entry.command.clone();
    command.account_collection_id = c.account_collection_id;
    command.metadata_collection_id = c.metadata_collection_id;
    command.quote_collection_id = c.quote_collection_id;
    command.reserve_cash = true;
    let f = observations(s, &pos, &entry, &command)?;
    let now = s.runtime().clock.trusted_now_ms()?;
    let quote_id = f.receipts["quote"].clone();
    let duplicate = monitor_observation(
        &pos.entry_time,
        pos.last_quote_observed_at_ms,
        pos.last_quote_known_at_ms,
        pos.last_quote_receipt.as_deref(),
        pos.last_reversal_receipt.as_deref(),
        &f.quote.time,
        &quote_id,
        c.reversal_signal_receipt_sha256.as_deref(),
    )?;
    if duplicate {
        return inspect(s, &pos.position_id);
    }
    let rev = reversal(
        s,
        &pos,
        &entry,
        c.reversal_signal_receipt_sha256.as_deref(),
        now,
    )?;
    let price = pos.policy.rules.price(&f.quote, now)?;
    let outcome = pos.policy.rules.evaluate(
        pos.basis,
        pos.high_water,
        price,
        rev.as_ref().map(|r| r.value),
    )?;
    pos.high_water = Some(outcome.next_high_water);
    pos.last_quote_known_at_ms = f.quote.time.known_at_ms;
    pos.last_quote_observed_at_ms = f.quote.time.observed_at_ms;
    pos.last_reversal_receipt = c.reversal_signal_receipt_sha256.clone();
    pos.reversal_watermark = rev.as_ref().map(|r| r.watermark.clone());
    pos.last_quote_receipt = Some(quote_id.clone());
    pos.generation = pos
        .generation
        .checked_add(1)
        .filter(|v| *v <= 10000)
        .ok_or("managed_generation_exhausted")?;
    let mut writes = Vec::new();
    let mut expectations = vec![StorageExpectation::new(
        POSITIONS,
        &pos.position_id,
        Some(old),
    )];
    let mut close = None;
    if let Some(reason) = outcome.trigger {
        let (_, execution_close) = execution_window(s, &entry, &c.execution_occurrence_id)?;
        let mut trigger = Trigger {
            schema_version: "tradeassembly.managed_exit_trigger.v1".into(),
            position_id: pos.position_id.clone(),
            policy_sha256: hash(&pos.policy)?,
            quote_receipt_sha256: quote_id,
            reversal_receipt_sha256: c.reversal_signal_receipt_sha256,
            price,
            high_water: outcome.next_high_water,
            reason,
            known_at_ms: now,
            trigger_sha256: String::new(),
        };
        trigger.trigger_sha256 = hash(&trigger)?;
        let available =
            settlement::funding_floor(s, &entry.account_key, funding_capacity(&f.account)?)?.0;
        let old_cash = s.runtime().storage.get_json(CASH, &entry.account_key)?;
        let mut ledger = ledger_owned(s, old_cash.clone().ok_or("managed_cash_ledger_missing")?)?;
        let held = ledger
            .reservations
            .values()
            .try_fold(Decimal::ZERO, |a, x| a.checked_add(*x))?;
        if available
            .checked_subtract(held)?
            .compare(pos.policy.fee_cash_cap)?
            .is_lt()
        {
            return Err("managed_fee_cash_unavailable".into());
        }
        let m = &f.metadata;
        if quantity.compare(*m.minimum_quantity.required()?)?.is_lt()
            || !quantity.is_multiple(*m.quantity_increment.required()?)?
        {
            return Err("managed_close_lot_invalid".into());
        }
        let raw = f
            .quote
            .bid
            .required()?
            .multiply(
                "1".parse::<Decimal>()?
                    .checked_subtract(pos.policy.max_limit_slippage_ratio)?,
            )?
            .round(12, Rounding::Floor)?;
        let tick = *m.price_increment.required()?;
        let limit = raw
            .checked_divide(tick, 0, Rounding::Floor)?
            .multiply(tick)?
            .round(12, Rounding::Exact)?;
        if !limit.positive() {
            return Err("managed_close_limit_invalid".into());
        }
        let mut plan = entry.clone();
        plan.management = Some(CloseBinding {
            execution_occurrence_id: c.execution_occurrence_id.clone(),
            position_id: pos.position_id.clone(),
            trigger_sha256: trigger.trigger_sha256.clone(),
            exit_policy_sha256: hash(&pos.policy)?,
            original_entry_plan_id: entry.plan_id.clone(),
        });
        plan.plan_id.clear();
        plan.entry_id = trigger.trigger_sha256.clone();
        plan.command = command;
        plan.policy.position_action = PositionAction::SellToClose;
        plan.policy.fee_cash_cap = pos.policy.fee_cash_cap;
        plan.order.position_action = PositionAction::SellToClose;
        plan.order.quantity = quantity;
        plan.order.pricing = PriceInstruction::Limit { limit_price: limit };
        plan.order.time_in_force = pos.policy.time_in_force.clone();
        plan.order.client_order_id = format!("ta-close-{}", &trigger.trigger_sha256[..40]);
        plan.order.validate()?;
        plan.prepared_at_ms = now;
        plan.valid_until_ms = f.valid_until.min(execution_close);
        if let Some(r) = &rev {
            plan.valid_until_ms = plan.valid_until_ms.min(
                r.watermark
                    .event_end_ms
                    .checked_add(pos.policy.max_reversal_age_ms)
                    .ok_or("managed_reversal_time_invalid")?,
            );
        }
        plan.available_cash = available;
        plan.purchase_cash = pos.policy.fee_cash_cap;
        plan.source_receipts = f.receipts;
        plan.calculation_response_sha256 = trigger.trigger_sha256.clone();
        plan.plan_id = format!("orderplan_{}", hash(&plan)?);
        if ledger.reservations.len() >= 256 {
            return Err("managed_cash_capacity".into());
        }
        ledger
            .reservations
            .insert(plan.plan_id.clone(), plan.purchase_cash);
        pos.reservations.insert(plan.plan_id.clone(), quantity);
        expectations.extend([
            StorageExpectation::new(CASH, &entry.account_key, old_cash),
            StorageExpectation::new(PLANS, &plan.plan_id, None),
            StorageExpectation::new(TRIGGERS, &trigger.trigger_sha256, None),
        ]);
        writes.extend([
            StorageWrite::new(CASH, &entry.account_key, value(&ledger)?, ctx.clone()),
            StorageWrite::new(PLANS, &plan.plan_id, value(&plan)?, ctx.clone()),
            StorageWrite::new(
                TRIGGERS,
                &trigger.trigger_sha256,
                value(&trigger)?,
                ctx.clone(),
            ),
        ]);
        close = Some(plan);
    }
    seal(&mut pos)?;
    writes.push(StorageWrite::new(
        POSITIONS,
        &pos.position_id,
        value(&pos)?,
        ctx.clone(),
    ));
    writes.push(StorageWrite::new(
        JOURNAL,
        format!("{}:{}", pos.position_id, pos.generation),
        journal(
            &pos,
            if close.is_some() {
                "exit_triggered"
            } else {
                "price_observed"
            },
            pos.last_quote_receipt.as_deref().unwrap_or_default(),
        ),
        ctx.clone(),
    ));
    if measured("management.advance.commit", || {
        s.runtime().storage.put_json_batch(&writes, &expectations)
    })? != ComparePutOutcome::Updated
    {
        return Err("managed_advance_conflict".into());
    }
    #[cfg(test)]
    if close.is_some() && ctx.idempotency_key.as_str() == "managed-fixture-fail-after-prepare" {
        return Err("managed_fixture_prepared_before_decision".into());
    }
    let decision = match close {
        Some(p) => Some(decide(s, &p, &pos, &config, ctx)?),
        None => None,
    };
    Ok(json!({"position":pos,"decision":decision,"executionAuthorityGranted":false}))
}
fn abort_unsent(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    old_pos: &Value,
    pos: &Position,
    state: Option<Value>,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let old_cash = s
        .runtime()
        .storage
        .get_json(CASH, &p.account_key)?
        .ok_or("managed_cash_missing")?;
    let mut ledger = ledger_owned(s, old_cash.clone())?;
    let mut pos = pos.clone();
    if ledger.reservations.remove(&p.plan_id) != Some(p.purchase_cash)
        || pos.reservations.remove(&p.plan_id) != Some(p.order.quantity)
    {
        return Err("managed_release_reservation_invalid".into());
    }
    let mut receipt = ReleaseReceipt {
        schema_version: "tradeassembly.order_plan_release.v1".into(),
        plan_id: p.plan_id.clone(),
        account_key: p.account_key.clone(),
        owner_sha256: owner(s)?,
        released_cash: p.purchase_cash,
        released_at_ms: s.runtime().clock.trusted_now_ms()?,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = release_hash(&receipt)?;
    pos.generation = pos
        .generation
        .checked_add(1)
        .filter(|v| *v <= 10000)
        .ok_or("managed_generation_exhausted")?;
    seal(&mut pos)?;
    if s.runtime().storage.put_json_batch(
        &[
            StorageWrite::new(CASH, &p.account_key, value(&ledger)?, ctx.clone()),
            StorageWrite::new(POSITIONS, &pos.position_id, value(&pos)?, ctx.clone()),
            StorageWrite::new(RELEASES, &p.plan_id, value(&receipt)?, ctx.clone()),
            StorageWrite::new(
                JOURNAL,
                format!("{}:{}", pos.position_id, pos.generation),
                journal(&pos, "unsent_close_released", &receipt.receipt_sha256),
                ctx.clone(),
            ),
        ],
        &[
            StorageExpectation::new(CASH, &p.account_key, Some(old_cash)),
            StorageExpectation::new(POSITIONS, &pos.position_id, Some(old_pos.clone())),
            StorageExpectation::new(RELEASES, &p.plan_id, None),
            StorageExpectation::new(submission::STATE, &p.plan_id, state),
        ],
    )? != ComparePutOutcome::Updated
    {
        return Err("managed_release_conflict".into());
    }
    Ok(
        json!({"position":pos,"status":"unsent_close_released","release":receipt,"executionAuthorityGranted":false}),
    )
}
// Keep the independent causal and idempotency inputs explicit at this boundary.
#[allow(clippy::too_many_arguments)]
fn monitor_observation(
    entry: &ObservationTime,
    last_observed: i64,
    last_known: i64,
    last_quote: Option<&str>,
    last_reversal: Option<&str>,
    time: &ObservationTime,
    quote: &str,
    reversal: Option<&str>,
) -> Result<bool, String> {
    if time.observed_at_ms < entry.observed_at_ms || time.known_at_ms < entry.known_at_ms {
        return Err("managed_quote_predates_entry".into());
    }
    if time.observed_at_ms < last_observed
        || time.known_at_ms < last_known
        || (time.observed_at_ms == last_observed
            && last_quote.is_some()
            && last_quote != Some(quote))
    {
        return Err("managed_quote_revision_requires_replay".into());
    }
    Ok(last_quote == Some(quote) && last_reversal == reversal)
}
fn execution_window(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    id: &str,
) -> Result<(Value, i64), String> {
    if id.starts_with("management_window_") {
        let current = crate::service::management_windows::current(s, id)?;
        let r = current.receipt;
        let original_entry = match p.management.as_ref() {
            Some(binding) => read_owned(s, &binding.original_entry_plan_id)?,
            None => p.clone(),
        };
        if original_entry.management.is_some()
            || original_entry.order.position_action != PositionAction::BuyToOpen
            || original_entry.owner_sha256 != p.owner_sha256
            || original_entry.command.scope != p.command.scope
            || r.command.scope != p.command.scope
            || r.subject.lineage.original_entry_plan_id != original_entry.plan_id
            || p.management
                .as_ref()
                .is_some_and(|b| b.position_id != r.command.position_id)
            || r.subject.lineage.capability_graph_revision != p.command.capability_graph_revision
        {
            return Err("managed_execution_window_scope_mismatch".into());
        }
        if current.status != crate::service::management_windows::Status::Active {
            return Err("managed_execution_window_closed".into());
        }
        let open = r
            .selection
            .open_at_ms
            .ok_or("managed_execution_window_missing")?;
        let provider_close = r
            .selection
            .close_at_ms
            .ok_or("managed_execution_window_missing")?;
        // Calendar freshness is also an exclusive final admission deadline.
        // The immutable receipt retains the provider's actual interval bounds.
        let close = provider_close.min(r.fresh_until_ms);
        let now = s.runtime().clock.trusted_now_ms()?;
        if now < open || now >= close {
            return Err("managed_execution_window_closed".into());
        }
        return Ok((
            json!({"schemaVersion":"tradeassembly.management_window_constraint.v1", "scope":p.command.scope,
            "windowId":id,"windowReceiptSha256":r.receipt_sha256,"positionId":r.command.position_id,
            "originalEntryPlanId":original_entry.plan_id,"capabilityGraphRevision":p.command.capability_graph_revision,"openAtMs":open,"closeAtMs":close}),
            close,
        ));
    }
    let r = crate::service::session_schedules::current(s, id)?;
    let open = r
        .occurrence
        .execution_open_at_ms
        .ok_or("managed_execution_window_missing")?;
    let close = r
        .occurrence
        .execution_close_at_ms
        .ok_or("managed_execution_window_missing")?;
    if r.command.scope != p.command.scope
        || r.command.capability_graph_revision != p.command.capability_graph_revision
        || r.command.mode != "paper"
    {
        return Err("managed_execution_window_scope_mismatch".into());
    }
    let now = s.runtime().clock.trusted_now_ms()?;
    if now < open || now >= close {
        return Err("managed_execution_window_closed".into());
    }
    Ok((
        json!({"schemaVersion":"tradeassembly.execution_window_constraint.v1","scope":p.command.scope,"occurrenceId":id,"scheduleReceiptSha256":r.receipt_sha256,"openAtMs":open,"closeAtMs":close}),
        close,
    ))
}
pub(crate) fn snapshot(s: &TradeAssemblyService, p: &OrderPlan) -> Result<OrderPlan, String> {
    let _timing = crate::service::order_plans::financial_timing("management.snapshot");
    // A fresh round for every approval/admission/revalidation. Nested pure
    // validators can share this round; no service escapes to broker dispatch.
    crate::service::package_validation::with_round(s, |s| snapshot_in_round(s, p))
}
fn snapshot_in_round(s: &TradeAssemblyService, p: &OrderPlan) -> Result<OrderPlan, String> {
    let b = p.management.as_ref().ok_or("managed_close_required")?;
    let (_, pos) = load(s, &b.position_id)?;
    assert_provisional_close_evidence(s, &pos)?;
    let entry = read_owned(s, &b.original_entry_plan_id)?;
    verify_entry(s, &entry, &pos)?;
    let (_, execution_close) = measured("management.snapshot.window", || {
        execution_window(s, &entry, &b.execution_occurrence_id)
    })?;
    if p.valid_until_ms > execution_close {
        return Err("managed_execution_window_invalid".into());
    }
    if pos.entry_plan_id != entry.plan_id
        || b.exit_policy_sha256 != hash(&pos.policy)?
        || policy(s, &entry.command.scope)?.as_ref() != Some(&pos.policy)
        || p.owner_sha256 != pos.owner_sha256
        || p.order.position_action != PositionAction::SellToClose
        || pos.reservations.get(&p.plan_id) != Some(&p.order.quantity)
        || s.runtime().clock.trusted_now_ms()? >= p.valid_until_ms
    {
        return Err("managed_close_stale".into());
    }
    let raw = s
        .runtime()
        .storage
        .get_json(TRIGGERS, &b.trigger_sha256)?
        .ok_or("managed_trigger_missing")?;
    let t: Trigger = serde_json::from_value(raw).map_err(|_| "managed_trigger_invalid")?;
    let mut check = t.clone();
    check.trigger_sha256.clear();
    if t.trigger_sha256 != hash(&check)?
        || t.position_id != pos.position_id
        || t.policy_sha256 != b.exit_policy_sha256
        || p.entry_id != t.trigger_sha256
        || p.calculation_response_sha256 != t.trigger_sha256
    {
        return Err("managed_trigger_binding_invalid".into());
    }
    let _ = measured("management.snapshot.run", || {
        management_run(s, &pos.activation_id)
    })?;
    let (_, original) = submission::record(s, &entry)?.ok_or("managed_entry_submission_missing")?;
    let _ = measured("management.snapshot.source", || {
        settlement::current_source(s, &entry, &original)
    })?;
    let _current_reversal = measured("management.snapshot.reversal", || {
        reversal(
            s,
            &pos,
            &entry,
            t.reversal_receipt_sha256.as_deref(),
            s.runtime().clock.trusted_now_ms()?,
        )
    })?;
    let f = measured("management.snapshot.observations", || {
        observations_in_round(s, &pos, &entry, &p.command)
    })?;
    if f.receipts != p.source_receipts
        || p.order.instrument != entry.order.instrument
        || p.order
            .quantity
            .compare(pos.quantity.checked_subtract(pos.closed_quantity)?)?
            .is_gt()
        || p.purchase_cash != pos.policy.fee_cash_cap
        || p.order.time_in_force != pos.policy.time_in_force
        || p.command.scope != entry.command.scope
        || p.command.capability_graph_revision != entry.command.capability_graph_revision
    {
        return Err("managed_close_binding_invalid".into());
    }
    let ledger = ledger_owned(
        s,
        s.runtime()
            .storage
            .get_json(CASH, &p.account_key)?
            .ok_or("managed_fee_reservation_missing")?,
    )?;
    let tick = *f.metadata.price_increment.required()?;
    let raw = f
        .quote
        .bid
        .required()?
        .multiply(
            "1".parse::<Decimal>()?
                .checked_subtract(pos.policy.max_limit_slippage_ratio)?,
        )?
        .round(12, Rounding::Floor)?;
    let limit = raw
        .checked_divide(tick, 0, Rounding::Floor)?
        .multiply(tick)?
        .round(12, Rounding::Exact)?;
    let mut expected_policy = entry.policy.clone();
    expected_policy.position_action = PositionAction::SellToClose;
    expected_policy.fee_cash_cap = pos.policy.fee_cash_cap;
    if p.policy != expected_policy
        || p.order.pricing != (PriceInstruction::Limit { limit_price: limit })
        || !p
            .order
            .quantity
            .is_multiple(*f.metadata.quantity_increment.required()?)?
        || p.order
            .quantity
            .compare(*f.metadata.minimum_quantity.required()?)?
            .is_lt()
        || p.order.client_order_id != format!("ta-close-{}", &b.trigger_sha256[..40])
    {
        return Err("managed_close_lot_or_price_changed".into());
    }
    if ledger.reservations.get(&p.plan_id) != Some(&p.purchase_cash) {
        return Err("managed_fee_reservation_missing".into());
    }
    Ok(p.clone())
}
fn decide(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    pos: &Position,
    config: &Value,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let _timing = crate::service::order_plans::financial_timing("management.decide");
    measured("management.decide.first_snapshot", || snapshot(s, p))?;
    let (mut run, fresh_config) = management_run(s, &pos.activation_id)?;
    if &fresh_config != config {
        return Err("managed_config_changed".into());
    }
    let old = run.clone();
    let attempt = format!("attempt_close_{}", hash(&(owner(s)?, &p.plan_id))?);
    let resource = format!("typed-close:{}", p.plan_id);
    let worker = format!("managed-worker:{}", hash(&(owner(s)?, &attempt))?);
    let now = s.runtime().clock.trusted_now_ms()?;
    let lease = measured("management.decide.lease", || {
        s.runtime().leases.acquire(&resource, &worker, now, 60000)
    })?
    .ok_or("managed_close_lease_busy")?;
    let result = (|| {
        let old_attempt = s
            .runtime()
            .storage
            .get_json("execution_attempts", &attempt)?;
        run["orderPlanApprovals"][&p.plan_id] = json!({"planId":p.plan_id,"entryId":p.entry_id,"signalReceiptSha256":p.command.signal_receipt_sha256,"canonicalAccountSha256":p.canonical_account.as_ref().ok_or("managed_account_required")?.identity_sha256,"leaseResource":lease.resource,"leaseOwner":lease.owner,"leaseFence":lease.fencing_token,"attemptId":attempt,"managedPositionId":pos.position_id,"managedTriggerSha256":p.entry_id,"managedExitPolicySha256":hash(&pos.policy)?});
        run["orderPlanApprovals"][&p.plan_id]["executionWindow"] = execution_window(
            s,
            p,
            &p.management
                .as_ref()
                .ok_or("managed_close_required")?
                .execution_occurrence_id,
        )?
        .0;
        measured("management.decide.second_snapshot", || snapshot(s, p))?;
        let a = json!({"attemptId":attempt,"activationId":pos.activation_id,"state":"running","owner":worker,"leaseResource":resource,"fencingToken":lease.fencing_token,"executionContractVersion":crate::service::execution::typed::VERSION});
        let key = format!("run_{}", pos.activation_id);
        if measured("management.decide.commit", || {
            s.runtime().storage.put_json_batch(
                &[
                    StorageWrite::new("execution_runs", &key, run, ctx.clone()),
                    StorageWrite::new("execution_attempts", &attempt, a, ctx.clone()),
                ],
                &[
                    StorageExpectation::new("execution_runs", &key, Some(old)),
                    StorageExpectation::new("execution_attempts", &attempt, old_attempt),
                    StorageExpectation::new(submission::STATE, &p.plan_id, None),
                ],
            )
        })? != ComparePutOutcome::Updated
        {
            return Err("managed_decision_conflict".into());
        }
        Ok(
            json!({"planId":p.plan_id,"submission":{"planId":p.plan_id,"activationId":pos.activation_id,"attemptId":attempt,"brokerRequirementRef":config["orderPlanBrokerNode"]},"executionAuthorityGranted":false,"c5StillRequired":true}),
        )
    })();
    if result.is_err() {
        let _ = s.runtime().leases.release(&lease);
    }
    result
}
pub(crate) fn validate_approval(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    run: &Value,
    config: &Value,
) -> Result<(), String> {
    let b = p.management.as_ref().ok_or("managed_close_required")?;
    let (_, pos) = load(s, &b.position_id)?;
    let a = &run["orderPlanApprovals"][&p.plan_id];
    if a["managedPositionId"] != b.position_id
        || a["managedTriggerSha256"] != b.trigger_sha256
        || a["managedExitPolicySha256"] != b.exit_policy_sha256
        || config["managedExitPolicySha256"] != b.exit_policy_sha256
        || run["activationId"] != pos.activation_id
    {
        return Err("managed_approval_invalid".into());
    }
    snapshot(s, p).map(|_| ())
}
const CLOSE_RECEIPTS: &str = "execution_close_settlement_receipts_v1";
const CLOSE_KEYS: &str = "execution_close_settlement_keys_v1";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CloseReceipt {
    schema_version: String,
    plan_id: String,
    position_id: String,
    account_key: String,
    owner_sha256: String,
    source_binding: crate::ports::broker_submission::BrokerPreparedBinding,
    provider_response_sha256: String,
    observation: tradeassembly_plugin_sdk::close_settlement::Observation,
    released_hold: Decimal,
    receipt_sha256: String,
}
fn close_hash(r: &CloseReceipt) -> Result<String, String> {
    let mut c = r.clone();
    c.receipt_sha256.clear();
    hash(&c)
}
fn read_close(s: &TradeAssemblyService, id: &str) -> Result<CloseReceipt, String> {
    let r: CloseReceipt = serde_json::from_value(
        s.runtime()
            .storage
            .get_json(CLOSE_RECEIPTS, id)?
            .ok_or("close_settlement_receipt_missing")?,
    )
    .map_err(|_| "close_settlement_receipt_invalid")?;
    if r.schema_version != "tradeassembly.close_settlement_receipt.v1"
        || r.owner_sha256 != owner(s)?
        || r.receipt_sha256 != id
        || close_hash(&r)? != id
    {
        return Err("close_settlement_receipt_invalid".into());
    }
    Ok(r)
}
pub(super) fn verify_close_record(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    r: &submission::SubmissionRecord,
) -> Result<(), String> {
    if let Some(id) = &r.settlement_receipt_sha256 {
        let receipt = read_close(s, id)?;
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
            return Err("close_settlement_record_invalid".into());
        }
    }
    Ok(())
}
pub(super) fn verify_capital(
    s: &TradeAssemblyService,
    c: &settlement::CapitalCheckpoint,
) -> Result<(), String> {
    let r = read_close(s, &c.receipt_sha256)?;
    let a = &r.observation.checkpoint.account;
    if c.account_key != r.account_key
        || c.ledger_epoch != r.observation.checkpoint.ledger_epoch
        || c.sequence != r.observation.checkpoint.sequence
        || c.available_cash != funding_capacity(a)?
        || c.gross_exposure != settlement::gross_floor(a)?
        || c.account_semantics_sha256 != settlement::account_semantics(a)?
    {
        return Err("close_capital_checkpoint_invalid".into());
    }
    Ok(())
}
pub(super) fn capital_time(
    s: &TradeAssemblyService,
    c: &settlement::CapitalCheckpoint,
) -> Result<ObservationTime, String> {
    Ok(read_close(s, &c.receipt_sha256)?
        .observation
        .checkpoint
        .account
        .time)
}
fn assert_provisional_close_evidence(
    s: &TradeAssemblyService,
    pos: &Position,
) -> Result<(), String> {
    if let Some(applied) = &pos.provisional_closes {
        for (id, close) in applied {
            let plan = read_owned(s, id)?;
            let (_, record) =
                submission::record(s, &plan)?.ok_or("managed_close_submission_missing")?;
            if let Some(receipt_id) = &record.settlement_receipt_sha256 {
                // The immutable final receipt supersedes the provisional read
                // head as quantity evidence. Keep AppliedClose: commit_close
                // uses it to avoid applying the same fill a second time.
                let receipt = read_close(s, receipt_id)?;
                verify_final_applied_close(s, pos, close, &plan, &record, &receipt)?;
                continue;
            }
            super::provisional::verify_applied_close(
                s,
                id,
                &pos.position_id,
                close.quantity,
                &close.receipt_sha256,
            )?;
        }
    }
    Ok(())
}
// Historical host-committed evidence only: this does not reauthorize an
// order, make an old account checkpoint current, or accept provider V2 finality.
fn verify_final_applied_close(
    s: &TradeAssemblyService,
    pos: &Position,
    applied: &AppliedClose,
    plan: &OrderPlan,
    record: &submission::SubmissionRecord,
    receipt: &CloseReceipt,
) -> Result<(), String> {
    let binding = plan.management.as_ref().ok_or("managed_close_required")?;
    let prepared: crate::ports::broker_submission::BrokerPreparedBinding =
        serde_json::from_value(record.intent["preparedBinding"].clone())
            .map_err(|_| "close_settlement_record_invalid")?;
    let posting = hash(&(
        &plan.account_key,
        &receipt.observation.checkpoint.ledger_epoch,
        &receipt.observation.posting_id,
    ))?;
    if binding.position_id != pos.position_id
        || binding.original_entry_plan_id != pos.entry_plan_id
        || receipt.position_id != pos.position_id
        || receipt.plan_id != plan.plan_id
        || receipt.account_key != plan.account_key
        || receipt.source_binding != prepared
        || plan.order.position_action != PositionAction::SellToClose
        || receipt.observation.order.order != plan.order
        || Some(receipt.observation.canonical_account_sha256.as_str())
            != plan
                .canonical_account
                .as_ref()
                .map(|a| a.identity_sha256.as_str())
        || receipt.observation.order.filled_quantity
            != (Observed::Available {
                value: applied.quantity,
            })
        || record.filled_quantity != applied.quantity
        || record.observation.as_ref() != Some(&receipt.observation.order)
        || record.provider_order_id != receipt.observation.order.provider_order_id
        || record.retained_cash != Decimal::ZERO
        || !matches!(
            record.phase,
            submission::Phase::Consumed
                | submission::Phase::Cancelled
                | submission::Phase::Rejected
        )
        || s.runtime()
            .storage
            .get_json("execution_close_postings_v1", &posting)?
            != Some(json!(plan.plan_id))
    {
        return Err("managed_final_close_evidence_invalid".into());
    }
    Ok(())
}

fn entry_evidence(p: &Position) -> &str {
    p.entry_observation_sha256
        .as_deref()
        .unwrap_or(&p.entry_settlement_sha256)
}
fn entry_basis(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    policy: &ExitPolicy,
    pinned: Option<&str>,
) -> Result<(Decimal, Decimal, String, ObservationTime), String> {
    if policy.entry_basis == "provider_reported_average_fill" {
        super::provisional::entry_basis(s, p, pinned)
    } else {
        settlement::entry_basis(s, p)
    }
}
fn verify_entry(s: &TradeAssemblyService, entry: &OrderPlan, pos: &Position) -> Result<(), String> {
    if pos.policy.entry_basis == "provider_reported_average_fill" {
        let (q, b, _, _) = entry_basis(s, entry, &pos.policy, Some(entry_evidence(pos)))?;
        if q != pos.quantity || b != pos.basis {
            return Err("managed_entry_basis_changed".into());
        }
        Ok(())
    } else {
        settlement::verify_historical_entry(s, entry, &pos.entry_settlement_sha256)
    }
}
pub(super) fn provisional_close_writes(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    o: &tradeassembly_plugin_sdk::settlement_observation_v2::Observation,
    receipt: &str,
    ctx: &SideEffectContext,
    writes: &mut Vec<StorageWrite>,
    expects: &mut Vec<StorageExpectation>,
) -> Result<(), String> {
    use tradeassembly_plugin_sdk::broker_v2::OrderStatus;
    if !matches!(
        o.order.status,
        OrderStatus::Filled | OrderStatus::Cancelled | OrderStatus::Expired | OrderStatus::Rejected
    ) {
        return Ok(());
    }
    let binding = p.management.as_ref().ok_or("managed_close_required")?;
    let (raw, mut pos) = load(s, &binding.position_id)?;
    if binding.original_entry_plan_id != pos.entry_plan_id || o.order.order != p.order {
        return Err("managed_provisional_close_binding_invalid".into());
    }
    let filled = *o.order.filled_quantity.required()?;
    if filled.negative()
        || filled.compare(p.order.quantity)?.is_gt()
        || o.order.status == OrderStatus::Filled && filled != p.order.quantity
    {
        return Err("managed_provisional_close_quantity_invalid".into());
    }
    let previous = pos
        .provisional_closes
        .as_ref()
        .and_then(|m| m.get(&p.plan_id))
        .map(|x| x.quantity);
    if let Some(prior) = previous {
        if prior != filled {
            return Err("managed_provisional_close_revision_requires_reconciliation".into());
        }
        return Ok(());
    }
    if pos.reservations.get(&p.plan_id) != Some(&p.order.quantity) {
        return Err("managed_provisional_close_reservation_missing".into());
    }
    let next = pos.closed_quantity.checked_add(filled)?;
    if next.compare(pos.quantity)?.is_gt() {
        return Err("managed_provisional_close_exceeds_owned".into());
    }
    let applied = pos.provisional_closes.get_or_insert_with(BTreeMap::new);
    if applied.len() >= 256 {
        return Err("managed_provisional_close_capacity".into());
    }
    applied.insert(
        p.plan_id.clone(),
        AppliedClose {
            quantity: filled,
            receipt_sha256: receipt.into(),
        },
    );
    pos.closed_quantity = next;
    pos.reservations.remove(&p.plan_id);
    pos.generation = pos
        .generation
        .checked_add(1)
        .filter(|g| *g <= 10000)
        .ok_or("managed_generation_exhausted")?;
    seal(&mut pos)?;
    writes.push(StorageWrite::new(
        POSITIONS,
        &pos.position_id,
        value(&pos)?,
        ctx.clone(),
    ));
    writes.push(StorageWrite::new(
        JOURNAL,
        format!("{}:{}", pos.position_id, pos.generation),
        journal(&pos, "close_fill_observed_cash_unreconciled", receipt),
        ctx.clone(),
    ));
    expects.push(StorageExpectation::new(
        POSITIONS,
        &pos.position_id,
        Some(raw),
    ));
    Ok(())
}

fn close_summary(r: &CloseReceipt) -> Value {
    json!({"schemaVersion":r.schema_version,"planId":r.plan_id,"positionId":r.position_id,"receiptSha256":r.receipt_sha256,"grossProceeds":r.observation.gross_proceeds,"feeCash":r.observation.fee_cash,"filledQuantity":r.observation.order.filled_quantity,"releasedHold":r.released_hold,"checkpointEpoch":r.observation.checkpoint.ledger_epoch,"checkpointSequence":r.observation.checkpoint.sequence,"accountCheckpointSha256":hash(&r.observation.checkpoint).ok(),"availableCashCeiling":funding_capacity(&r.observation.checkpoint.account).ok(),"historicalOutcome":true,"executionAuthorityGranted":false})
}
pub(super) fn settle(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    use tradeassembly_plugin_sdk::close_settlement as wire;
    let b = p.management.as_ref().ok_or("managed_close_required")?;
    let key = hash(&(owner(s)?, &p.plan_id, ctx.idempotency_key.as_str()))?;
    if let Some(v) = s.runtime().storage.get_json(CLOSE_KEYS, &key)? {
        return Ok(close_summary(&read_close(
            s,
            v.as_str().ok_or("close_settlement_key_invalid")?,
        )?));
    }
    let (_, record) = submission::record(s, p)?.ok_or("broker_v2_state_missing")?;
    if record.settlement_receipt_sha256.is_some() {
        return Ok(close_summary(&read_close(
            s,
            record
                .settlement_receipt_sha256
                .as_deref()
                .unwrap_or_default(),
        )?));
    }
    if !matches!(
        record.phase,
        submission::Phase::Sending
            | submission::Phase::Unknown
            | submission::Phase::Open
            | submission::Phase::PartiallyFilled
            | submission::Phase::Consumed
            | submission::Phase::Rejected
            | submission::Phase::Cancelled
    ) {
        return Err("close_settlement_state_unsupported".into());
    }
    let binding = settlement::current_source(s, p, &record)?;
    let input = wire::Request {
        schema_version: "tradeassembly.close_settlement_request.v1".into(),
        plan_id: p.plan_id.clone(),
        original_entry_plan_id: b.original_entry_plan_id.clone(),
        canonical_account_sha256: p
            .canonical_account
            .as_ref()
            .ok_or("managed_account_required")?
            .identity_sha256
            .clone(),
        client_order_id: p.order.client_order_id.clone(),
    };
    input.validate()?;
    let manifest = s
        .runtime()
        .plugins
        .get_manifest(&binding.plugin_ref)?
        .ok_or("close_settlement_operation_unavailable")?;
    if !manifest["manifest"]["operations"]
        .as_array()
        .is_some_and(|xs| {
            xs.iter().any(|o| {
                o["id"] == wire::OPERATION
                    && o["capability"] == wire::CAPABILITY
                    && o["effect"] == "read"
                    && o["purpose"] == "settlement"
            })
        })
    {
        return Err("close_settlement_operation_unavailable".into());
    }
    let original: PluginOperationRequest =
        serde_json::from_value(record.intent["originalRequest"].clone())
            .map_err(|_| "close_original_request_invalid")?;
    let response = crate::service::providers::invoke_operation(
        s,
        &format!(
            "/plugins/instances/{}/operations/{}:invoke",
            binding.plugin_instance_ref,
            wire::OPERATION
        ),
        json!({"strategyId":p.command.scope.strategy_id,"strategyVersionId":p.command.scope.strategy_version_id,"strategySpecHash":p.command.scope.strategy_spec_hash,"activationId":original.activation_id,"attemptId":original.attempt_id,"evaluationTickId":original.evaluation_tick_id,"fencingToken":original.fencing_token,"capabilityGraphRevisionId":original.capability_graph_revision_id,"capabilityGraphFingerprint":original.capability_graph_fingerprint,"assetClass":p.order.instrument.asset_class,"instrumentFamily":p.order.instrument.instrument_family,"accountRef":record.intent["accountRef"],"mode":"paper","purpose":"settlement","accountMode":"paper","input":input,"idempotencyKey":format!("close-settlement-read:{key}")}),
    );
    if response.status != 200 {
        return Err("close_settlement_provider_unavailable".into());
    }
    let response: PluginOperationResponse =
        serde_json::from_value(response.body["response"].clone())
            .map_err(|_| "close_settlement_response_invalid")?;
    if response.correlation_id != format!("close-settlement-read:{key}")
        || response.freshness_state != "fresh"
        || response.schema_ref != wire::RESPONSE_SCHEMA
        || response.reconciliation_required
        || !jsonschema::validator_for(&wire::response_schema())
            .map_err(|_| "close_settlement_schema_invalid")?
            .is_valid(&response.payload)
    {
        return Err("close_settlement_response_invalid".into());
    }
    let o: wire::Observation = serde_json::from_value(response.payload.clone())
        .map_err(|_| "close_settlement_response_invalid")?;
    o.validate_for(
        &input,
        &p.order,
        s.runtime().clock.trusted_now_ms()?,
        p.policy.max_snapshot_age_ms,
    )?;
    if settlement::current_source(s, p, &record)? != binding {
        return Err("close_settlement_source_changed".into());
    }
    commit_close(s, p, binding, hash(&response)?, o, &key, ctx)
}
fn commit_close(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    binding: crate::ports::broker_submission::BrokerPreparedBinding,
    response_hash: String,
    o: tradeassembly_plugin_sdk::close_settlement::Observation,
    key: &str,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let b = p.management.as_ref().ok_or("managed_close_required")?;
    let (old_pos, mut pos) = load(s, &b.position_id)?;
    let (old_state, mut state) = submission::record(s, p)?.ok_or("broker_v2_state_missing")?;
    if state.settlement_receipt_sha256.is_some() {
        return Err("close_settlement_already_committed".into());
    }
    let old_cash = s
        .runtime()
        .storage
        .get_json(CASH, &p.account_key)?
        .ok_or("close_settlement_ledger_missing")?;
    let mut ledger = ledger_owned(s, old_cash.clone())?;
    let a = &o.checkpoint.account;
    let filled = *o.order.filled_quantity.required()?;
    let prior_applied = pos
        .provisional_closes
        .as_ref()
        .and_then(|m| m.get(&p.plan_id))
        .map(|x| x.quantity);
    let applied = prior_applied.unwrap_or(Decimal::ZERO);
    if prior_applied.is_some_and(|q| q != filled) {
        return Err("close_settlement_provisional_fill_changed".into());
    }
    let additional = filled.checked_subtract(applied)?;

    if (pos
        .provisional_closes
        .as_ref()
        .is_none_or(|m| !m.contains_key(&p.plan_id))
        && pos.reservations.get(&p.plan_id) != Some(&p.order.quantity))
        || ledger.reservations.get(&p.plan_id) != Some(&p.purchase_cash)
        || state.retained_cash != p.purchase_cash
        || o.fee_cash.compare(p.purchase_cash)?.is_gt()
        || filled.compare(state.filled_quantity)?.is_lt()
        || state
            .provider_order_id
            .as_ref()
            .is_some_and(|id| o.order.provider_order_id.as_ref() != Some(id))
        || state.observation.as_ref().is_some_and(|old| {
            o.order.observed_at_ms < old.observed_at_ms || o.order.known_at_ms < old.known_at_ms
        })
        || a.provider_account_id
            != p.canonical_account
                .as_ref()
                .ok_or("managed_account_required")?
                .provider_account_id
        || Some(&a.currency) != p.account_currency.as_ref()
        || Some(a.account_ref.as_str()) != state.intent["accountRef"].as_str()
    {
        return Err("close_settlement_identity_or_reservation_invalid".into());
    }
    if a.open_orders.required()?.iter().any(|x| {
        Some(&x.provider_order_id) == o.order.provider_order_id.as_ref()
            || x.client_order_id.as_deref() == Some(p.order.client_order_id.as_str())
    }) {
        return Err("close_settlement_order_still_open".into());
    }
    let PriceInstruction::Limit { limit_price } = p.order.pricing else {
        return Err("close_settlement_limit_required".into());
    };
    let minimum_proceeds = filled
        .multiply(limit_price)?
        .multiply(p.order.instrument.multiplier)?
        .round(a.currency.decimals, Rounding::Floor)?;
    if o.gross_proceeds.compare(minimum_proceeds)?.is_lt() {
        return Err("close_settlement_below_limit".into());
    }
    let available = funding_capacity(a)?;
    if let Some(previous) = &ledger.capital_checkpoint {
        let t = settlement::capital_time(s, previous)?;
        if previous.ledger_epoch != o.checkpoint.ledger_epoch
            || o.checkpoint.sequence <= previous.sequence
            || a.time.observed_at_ms < t.observed_at_ms
            || a.time.known_at_ms < t.known_at_ms
            || available
                .compare(
                    previous
                        .available_cash
                        .checked_add(o.gross_proceeds)?
                        .checked_subtract(o.fee_cash)?,
                )?
                .is_gt()
        {
            return Err("close_settlement_checkpoint_invalid".into());
        }
    }
    let remaining = pos
        .quantity
        .checked_subtract(pos.closed_quantity)?
        .checked_subtract(additional)?;
    let mut total_owned = remaining;
    for (_, raw) in s.runtime().storage.list_json(POSITIONS)? {
        let other: Position =
            serde_json::from_value(raw).map_err(|_| "managed_position_invalid")?;
        if other.position_id != pos.position_id && other.owner_sha256 == owner(s)? {
            let original = read_owned(s, &other.entry_plan_id)?;
            if original.account_key == p.account_key
                && original.order.instrument.instrument_id == p.order.instrument.instrument_id
            {
                total_owned = total_owned
                    .checked_add(other.quantity.checked_subtract(other.closed_quantity)?)?
            }
        }
    }
    let mut held = Decimal::ZERO;
    for h in a.positions.required()? {
        if h.instrument_id == p.order.instrument.instrument_id {
            if h.side != HoldingSide::Long {
                return Err("close_settlement_short_unsupported".into());
            }
            held = held.checked_add(h.quantity)?
        }
    }
    if held.compare(total_owned)?.is_lt() {
        return Err("close_settlement_position_coverage_invalid".into());
    }
    let mut receipt = CloseReceipt {
        schema_version: "tradeassembly.close_settlement_receipt.v1".into(),
        plan_id: p.plan_id.clone(),
        position_id: pos.position_id.clone(),
        account_key: p.account_key.clone(),
        owner_sha256: owner(s)?,
        source_binding: binding,
        provider_response_sha256: response_hash,
        observation: o.clone(),
        released_hold: state.retained_cash,
        receipt_sha256: String::new(),
    };
    receipt.receipt_sha256 = close_hash(&receipt)?;
    pos.closed_quantity = pos.closed_quantity.checked_add(additional)?;
    pos.reservations.remove(&p.plan_id);
    pos.generation = pos
        .generation
        .checked_add(1)
        .filter(|n| *n <= 10000)
        .ok_or("managed_generation_exhausted")?;
    seal(&mut pos)?;
    ledger.reservations.remove(&p.plan_id);
    ledger.capital_checkpoint = Some(settlement::CapitalCheckpoint {
        receipt_kind: Some("close_settlement".into()),
        account_key: p.account_key.clone(),
        ledger_epoch: o.checkpoint.ledger_epoch.clone(),
        sequence: o.checkpoint.sequence,
        available_cash: available,
        gross_exposure: settlement::gross_floor(a)?,
        account_semantics_sha256: settlement::account_semantics(a)?,
        receipt_sha256: receipt.receipt_sha256.clone(),
    });
    state.phase = if filled.positive() {
        submission::Phase::Consumed
    } else if o.order.status == tradeassembly_plugin_sdk::broker_v2::OrderStatus::Rejected {
        submission::Phase::Rejected
    } else {
        submission::Phase::Cancelled
    };
    state.retained_cash = Decimal::ZERO;
    state.filled_quantity = filled;
    state.observation = Some(o.order.clone());
    state.provider_order_id = o.order.provider_order_id.clone();
    state.generation = state
        .generation
        .checked_add(1)
        .filter(|v| *v <= 10000)
        .ok_or("broker_v2_generation_invalid")?;
    state.settlement_receipt_sha256 = Some(receipt.receipt_sha256.clone());
    state.record_sha256 = submission::record_hash(&state)?;
    let posting = hash(&(&p.account_key, &o.checkpoint.ledger_epoch, &o.posting_id))?;
    if s.runtime().storage.put_json_batch(
        &[
            StorageWrite::new(
                CLOSE_RECEIPTS,
                &receipt.receipt_sha256,
                value(&receipt)?,
                ctx.clone(),
            ),
            StorageWrite::new(CLOSE_KEYS, key, json!(receipt.receipt_sha256), ctx.clone()),
            StorageWrite::new(
                "execution_close_postings_v1",
                &posting,
                json!(p.plan_id),
                ctx.clone(),
            ),
            StorageWrite::new(CASH, &p.account_key, value(&ledger)?, ctx.clone()),
            StorageWrite::new(submission::STATE, &p.plan_id, value(&state)?, ctx.clone()),
            StorageWrite::new(POSITIONS, &pos.position_id, value(&pos)?, ctx.clone()),
            StorageWrite::new(
                JOURNAL,
                format!("{}:{}", pos.position_id, pos.generation),
                journal(&pos, "close_final_posting", &receipt.receipt_sha256),
                ctx.clone(),
            ),
        ],
        &[
            StorageExpectation::new(CLOSE_RECEIPTS, &receipt.receipt_sha256, None),
            StorageExpectation::new(CLOSE_KEYS, key, None),
            StorageExpectation::new("execution_close_postings_v1", &posting, None),
            StorageExpectation::new(CASH, &p.account_key, Some(old_cash)),
            StorageExpectation::new(submission::STATE, &p.plan_id, Some(old_state)),
            StorageExpectation::new(POSITIONS, &pos.position_id, Some(old_pos)),
        ],
    )? != ComparePutOutcome::Updated
    {
        return Err("close_settlement_atomic_conflict".into());
    }
    Ok(close_summary(&receipt))
}

pub(crate) fn preflight(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    if path != REGISTER && path != ADVANCE {
        return None;
    }
    let result = (|| {
        if serde_json::to_vec(body)
            .map_err(|_| "managed_command_invalid")?
            .len()
            > 8192
        {
            return Err("managed_command_too_large".into());
        }
        let entry = if path == REGISTER {
            let c: RegisterCommand = serde_json::from_value(body["register"].clone())
                .map_err(|_| "managed_register_invalid")?;
            s.require_object("execution_activation", &c.activation_id)
                .map_err(|_| "managed_not_available")?;
            read_owned(s, &c.entry_plan_id)?
        } else {
            let c: AdvanceCommand = serde_json::from_value(body["advance"].clone())
                .map_err(|_| "managed_advance_invalid")?;
            let (_, p) = load(s, &c.position_id)?;
            read_owned(s, &p.entry_plan_id)?
        };
        let e = crate::control_plane::http_envelope("http", "POST", path, body)?;
        if e.authority.account_mode != "paper"
            || body["scope"] != json!(entry.command.scope)
            || e.target_object.as_deref() != Some(entry.command.scope.strategy_id.as_str())
        {
            return Err("managed_scope_or_mode_mismatch".into());
        }
        Ok::<_, String>(())
    })();
    result.err().map(|e| ServiceResponse::conflict(&e))
}
pub(crate) fn response(s: &TradeAssemblyService, path: &str, body: Value) -> ServiceResponse {
    let result = (|| {
        let e = crate::control_plane::http_envelope("http", "POST", path, &body)?;
        let ctx = SideEffectContext::new(
            e.authority,
            IdempotencyKey::new(
                body["idempotencyKey"]
                    .as_str()
                    .ok_or("managed_key_required")?,
            )?,
        );
        if path == REGISTER {
            register(
                s,
                serde_json::from_value(body["register"].clone())
                    .map_err(|_| "managed_register_invalid")?,
                &ctx,
            )
        } else {
            advance(
                s,
                serde_json::from_value(body["advance"].clone())
                    .map_err(|_| "managed_advance_invalid")?,
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
mod tests {
    use super::*;
    fn time(obs: i64, known: i64) -> ObservationTime {
        ObservationTime {
            observed_at_ms: obs,
            known_at_ms: known,
            basis: TimeBasis::ProviderObserved,
        }
    }
    #[test]
    fn reversal_on_same_quote_is_new_work_and_prefill_or_revised_quotes_refuse() {
        let entry = time(2000, 2100);
        assert_eq!(
            monitor_observation(&entry, 0, 0, None, None, &time(1999, 2200), "q", None)
                .unwrap_err(),
            "managed_quote_predates_entry"
        );
        assert!(!monitor_observation(
            &entry,
            2200,
            2300,
            Some("q"),
            Some("false"),
            &time(2200, 2300),
            "q",
            Some("true")
        )
        .unwrap());
        assert!(monitor_observation(
            &entry,
            2200,
            2300,
            Some("q"),
            Some("true"),
            &time(2200, 2300),
            "q",
            Some("true")
        )
        .unwrap());
        assert_eq!(
            monitor_observation(
                &entry,
                2200,
                2300,
                Some("q"),
                None,
                &time(2200, 2400),
                "q-revised",
                None
            )
            .unwrap_err(),
            "managed_quote_revision_requires_replay"
        );
        assert!(monitor_observation(
            &entry,
            2200,
            2300,
            Some("q"),
            None,
            &time(2100, 2400),
            "older",
            None
        )
        .is_err());
    }
    #[test]
    fn reversed_or_prefill_reversal_evidence_cannot_trigger_a_close() {
        let entry = time(2000, 2100);
        let first = ReversalWatermark {
            receipt_sha256: "first".into(),
            event_end_ms: 2200,
            known_at_ms: 2300,
            arrived_at_ms: 2400,
        };
        assert!(causal_reversal(&entry, None, &first).is_ok());
        let older = ReversalWatermark {
            receipt_sha256: "older".into(),
            event_end_ms: 2199,
            known_at_ms: 2301,
            arrived_at_ms: 2401,
        };
        assert!(causal_reversal(&entry, Some(&first), &older).is_err());
        let revised = ReversalWatermark {
            event_end_ms: 2200,
            ..older.clone()
        };
        assert!(causal_reversal(&entry, Some(&first), &revised).is_err());
        let prefill = ReversalWatermark {
            event_end_ms: 1999,
            ..older
        };
        assert_eq!(
            causal_reversal(&entry, None, &prefill).unwrap_err(),
            "managed_reversal_predates_entry"
        );
        assert!(causal_reversal(&entry, Some(&first), &first).is_ok());
    }
    fn idle_frame(end: i64, known: i64, arrived: i64) -> crate::data_strategy::SignalFrame {
        crate::data_strategy::SignalFrame {
            event_end_ms: end,
            known_at_ms: known,
            arrived_at_ms: arrived,
            trade_instruments: vec![crate::data_pipeline::InstrumentContext {
                instrument_id: "owned".into(),
                asset_class: "equity".into(),
                instrument_family: "equity".into(),
            }],
            entry: crate::data_strategy::SignalValue::Ready { value: false },
            exit: crate::data_strategy::SignalValue::Ready { value: false },
            upstream_content_sha256: vec![],
        }
    }
    #[test]
    fn consumed_false_and_verified_preentry_are_idle_without_manufacturing_a_signal() {
        let now = 100_000;
        let consumed = idle_frame(60_000, 60_001, 60_002);
        validate_reversal_subject(&consumed, "owned", now).unwrap();
        let watermark = reversal_watermark("consumed", &consumed);
        assert!(idle_reversal_frame(
            &consumed,
            "consumed",
            &time(50_000, 50_001),
            Some(&watermark),
            Some("consumed"),
            false
        )
        .unwrap());
        // The same stale receipt is still rejected for manual execution.
        assert_eq!(
            ready_reversal(
                &consumed,
                "consumed",
                &time(50_000, 50_001),
                Some(&watermark),
                now,
                30_000
            )
            .err()
            .unwrap(),
            "managed_reversal_not_current"
        );
        assert!(!idle_reversal_frame(
            &consumed,
            "unconsumed",
            &time(50_000, 50_001),
            None,
            None,
            false
        )
        .unwrap());
        let bootstrap = idle_frame(60_000, 60_001, 60_002);
        validate_reversal_subject(&bootstrap, "owned", now).unwrap();
        assert!(idle_reversal_frame(
            &bootstrap,
            "bootstrap",
            &time(70_000, 70_001),
            None,
            None,
            false
        )
        .unwrap());
        assert_eq!(
            ready_reversal(
                &bootstrap,
                "bootstrap",
                &time(70_000, 70_001),
                None,
                now,
                30_000
            )
            .err()
            .unwrap(),
            "managed_reversal_not_current"
        );
        let fresh_bootstrap = idle_frame(90_000, 90_001, 90_002);
        assert_eq!(
            ready_reversal(
                &fresh_bootstrap,
                "fresh-bootstrap",
                &time(95_000, 95_001),
                None,
                now,
                30_000
            )
            .err()
            .unwrap(),
            "managed_reversal_predates_entry"
        );
    }
    #[test]
    fn idle_wait_preserves_fresh_true_close_retry_and_independent_price_monitoring() {
        let frame = idle_frame(90_000, 90_001, 90_002);
        let entry = time(50_000, 50_001);
        let prior = reversal_watermark("false-observation", &frame);
        assert!(idle_reversal_frame(
            &frame,
            "false-observation",
            &entry,
            Some(&prior),
            Some("false-observation"),
            false
        )
        .unwrap());
        assert!(!idle_reversal_frame(
            &frame,
            "false-observation",
            &entry,
            Some(&prior),
            Some("false-observation"),
            true
        )
        .unwrap());
        let mut exit = frame;
        exit.exit = crate::data_strategy::SignalValue::Ready { value: true };
        let prior = reversal_watermark("true-observation", &exit);
        assert!(!idle_reversal_frame(
            &exit,
            "true-observation",
            &entry,
            Some(&prior),
            Some("true-observation"),
            false
        )
        .unwrap());
        assert!(
            ready_reversal(
                &exit,
                "true-observation",
                &entry,
                Some(&prior),
                100_000,
                30_000
            )
            .unwrap()
            .value
        );
    }

    #[test]
    fn idle_classification_still_requires_actual_now_subject_clocks_and_ready_exit() {
        let now = 100_000;
        let frame = idle_frame(60_000, 60_001, 60_002);
        let mut foreign = frame.clone();
        foreign.trade_instruments[0].instrument_id = "foreign".into();
        assert_eq!(
            validate_reversal_subject(&foreign, "owned", now).unwrap_err(),
            "managed_reversal_not_current"
        );
        let mut multiple = frame.clone();
        multiple
            .trade_instruments
            .push(multiple.trade_instruments[0].clone());
        assert_eq!(
            validate_reversal_subject(&multiple, "owned", now).unwrap_err(),
            "managed_reversal_not_current"
        );
        for clock in ["known", "arrived", "event"] {
            let mut bad = frame.clone();
            match clock {
                "known" => bad.known_at_ms = now + 1,
                "arrived" => bad.arrived_at_ms = now + 1,
                "event" => bad.event_end_ms = now + 1,
                _ => unreachable!(),
            }
            assert_eq!(
                validate_reversal_subject(&bad, "owned", now).unwrap_err(),
                "managed_reversal_not_current"
            );
        }
        let mut unready = frame;
        unready.exit = crate::data_strategy::SignalValue::WarmingUp;
        assert_eq!(
            idle_reversal_frame(
                &unready,
                "consumed",
                &time(70_000, 70_001),
                None,
                Some("consumed"),
                false
            )
            .unwrap_err(),
            "managed_reversal_not_ready"
        );
    }
    #[test]
    fn unconsumed_stale_and_revised_postentry_observations_remain_errors() {
        let now = 100_000;
        let frame = idle_frame(60_000, 60_001, 60_002);
        assert!(!idle_reversal_frame(
            &frame,
            "unconsumed",
            &time(50_000, 50_001),
            None,
            None,
            false
        )
        .unwrap());
        assert_eq!(
            ready_reversal(
                &frame,
                "unconsumed",
                &time(50_000, 50_001),
                None,
                now,
                30_000
            )
            .err()
            .unwrap(),
            "managed_reversal_not_current"
        );
        let prior = reversal_watermark("original", &frame);
        let preentry = idle_frame(40_000, 40_001, 40_002);
        validate_reversal_subject(&preentry, "owned", now).unwrap();
        assert_eq!(
            idle_reversal_frame(
                &preentry,
                "regressed",
                &time(50_000, 50_001),
                Some(&prior),
                Some("original"),
                false
            )
            .unwrap_err(),
            "managed_reversal_predates_entry",
        );
        assert_eq!(
            idle_reversal_frame(
                &preentry,
                "regressed",
                &time(50_000, 50_001),
                Some(&prior),
                None,
                false
            )
            .unwrap_err(),
            "managed_reversal_predates_entry",
        );
        assert_eq!(
            idle_reversal_frame(
                &preentry,
                "regressed",
                &time(50_000, 50_001),
                None,
                Some("original"),
                false
            )
            .unwrap_err(),
            "managed_reversal_predates_entry",
        );
        assert_eq!(
            idle_reversal_frame(
                &frame,
                "revised",
                &time(50_000, 50_001),
                Some(&prior),
                Some("revised"),
                false
            )
            .unwrap_err(),
            "managed_reversal_revision_requires_replay"
        );
    }

    #[test]
    fn unsupported_automatic_reversal_is_rejected_before_entry() {
        let mut p:ExitPolicy=serde_json::from_value(json!({"entryBasis":"provider_final_gross_excluding_fees","rules":{"schemaVersion":"tradeassembly.position_exit_rules.v1","priceBasis":"bid","stopLossRatio":"0.1","trailingRatio":"0.2","trailingStart":{"kind":"immediate"},"reversalSignal":null,"precedence":["stop_loss","trailing"],"maxQuoteAgeMs":30000},"feeCashCap":"1","maxLimitSlippageRatio":"0.01","timeInForce":"day","maxReversalAgeMs":30000})).unwrap();
        assert!(validate_automatic_policy(&p).is_ok());
        p.rules.reversal_signal = Some("typed.exit".into());
        assert_eq!(
            validate_automatic_policy(&p).unwrap_err(),
            "management_automation_reversal_evidence_unsupported"
        );
    }
}

#[cfg(test)]
#[path = "management/final_close_evidence_tests.rs"]
mod final_close_evidence_tests;
