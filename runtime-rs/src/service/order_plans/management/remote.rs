//! Finite V6 managed-position commands. Observations and decisions never grant C5.
//! Snapshot replay reads immutable owner-bound outcomes, not current broker facts.
use super::*;

pub const INSPECT: &str = "/product/strategies/positions/inspect";
pub const RESOURCE_KIND: &str = "strategy_managed_position";
const SNAPSHOTS: &str = "management_runner_snapshots_v1";
const MAX_JOURNAL: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManagementOperation {
    Register {
        scope: data_plans::PlanScope,
        entry_plan_id: String,
        activation_id: String,
    },
    Advance {
        scope: data_plans::PlanScope,
        advance: AdvanceCommand,
    },
    SubmitClose {
        scope: data_plans::PlanScope,
        submission: transport::SubmissionCommand,
    },
    ReconcileClose {
        scope: data_plans::PlanScope,
        plan_id: String,
    },
    SettleClose {
        scope: data_plans::PlanScope,
        plan_id: String,
    },
    /// Interim finite full-journal read: no cursor/paging is supported.
    Inspect {
        scope: data_plans::PlanScope,
        position_id: String,
    },
}
impl ManagementOperation {
    pub fn scope(&self) -> &data_plans::PlanScope {
        match self {
            Self::Register { scope, .. }
            | Self::Advance { scope, .. }
            | Self::SubmitClose { scope, .. }
            | Self::ReconcileClose { scope, .. }
            | Self::SettleClose { scope, .. }
            | Self::Inspect { scope, .. } => scope,
        }
    }
    pub fn action(&self) -> &'static str {
        match self {
            Self::Register { .. } => "strategy.position.register",
            Self::Advance { .. } => "strategy.position.advance",
            Self::SubmitClose { .. } => "strategy.order_plan.submit",
            Self::ReconcileClose { .. } => "strategy.order_plan.reconcile",
            Self::SettleClose { .. } => "strategy.order_plan.settle",
            Self::Inspect { .. } => "strategy.position.inspect",
        }
    }
    pub fn body(&self) -> (&'static str, Value) {
        match self {
            Self::Register {
                scope,
                entry_plan_id,
                activation_id,
            } => (
                REGISTER,
                json!({"scope":scope,"register":{"entryPlanId":entry_plan_id,"activationId":activation_id}}),
            ),
            Self::Advance { scope, advance } => (ADVANCE, json!({"scope":scope,"advance":advance})),
            Self::SubmitClose { scope, submission } => {
                (SUBMIT, json!({"scope":scope,"submission":submission}))
            }
            Self::ReconcileClose { scope, plan_id } => {
                (RECONCILE, json!({"scope":scope,"planId":plan_id}))
            }
            Self::SettleClose { scope, plan_id } => {
                (SETTLE, json!({"scope":scope,"planId":plan_id}))
            }
            Self::Inspect { scope, position_id } => {
                (INSPECT, json!({"scope":scope,"positionId":position_id}))
            }
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        let label =
            |v: &str| !v.is_empty() && v.len() <= 256 && v.bytes().all(|b| b.is_ascii_graphic());
        if ![
            &self.scope().strategy_id,
            &self.scope().strategy_version_id,
            &self.scope().strategy_spec_hash,
        ]
        .into_iter()
        .all(|s| label(s))
        {
            return Err("management_runner_scope_invalid".into());
        }
        let plan = |id: &str| {
            id.strip_prefix("orderplan_").is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        match self {
            Self::Register {
                entry_plan_id,
                activation_id,
                ..
            } if !plan(entry_plan_id) || !label(activation_id) => {
                Err("management_runner_command_invalid".into())
            }
            Self::SubmitClose { submission, .. } => {
                submission.validate()?;
                if !plan(&submission.plan_id) {
                    return Err("management_runner_command_invalid".into());
                }
                Ok(())
            }
            Self::ReconcileClose { plan_id, .. } | Self::SettleClose { plan_id, .. }
                if !plan(plan_id) =>
            {
                Err("management_runner_command_invalid".into())
            }
            Self::Advance { advance, .. } => {
                if ![
                    &advance.execution_occurrence_id,
                    &advance.position_id,
                    &advance.account_collection_id,
                    &advance.metadata_collection_id,
                    &advance.quote_collection_id,
                ]
                .into_iter()
                .all(|s| label(s))
                    || advance
                        .reversal_signal_receipt_sha256
                        .as_deref()
                        .is_some_and(|s| !label(s))
                {
                    return Err("management_runner_command_invalid".into());
                }
                Ok(())
            }
            Self::Inspect { position_id, .. } if !label(position_id) => {
                Err("management_runner_command_invalid".into())
            }
            _ => Ok(()),
        }
    }
}

fn original_entry(
    s: &TradeAssemblyService,
    op: &ManagementOperation,
) -> Result<(OrderPlan, String, Option<Position>, Option<OrderPlan>), String> {
    match op {
        ManagementOperation::Register {
            entry_plan_id,
            activation_id,
            ..
        } => Ok((
            read_owned(s, entry_plan_id)?,
            activation_id.clone(),
            None,
            None,
        )),
        ManagementOperation::Advance { advance, .. } => {
            let (_, pos) = load(s, &advance.position_id)?;
            Ok((
                read_owned(s, &pos.entry_plan_id)?,
                pos.activation_id.clone(),
                Some(pos),
                None,
            ))
        }
        ManagementOperation::Inspect { position_id, .. } => {
            let (_, pos) = load(s, position_id)?;
            Ok((
                read_owned(s, &pos.entry_plan_id)?,
                pos.activation_id.clone(),
                Some(pos),
                None,
            ))
        }
        ManagementOperation::SubmitClose {
            submission: command,
            ..
        } => close_entry(s, &command.plan_id),
        ManagementOperation::ReconcileClose { plan_id, .. }
        | ManagementOperation::SettleClose { plan_id, .. } => close_entry(s, plan_id),
    }
}
fn close_entry(
    s: &TradeAssemblyService,
    id: &str,
) -> Result<(OrderPlan, String, Option<Position>, Option<OrderPlan>), String> {
    let close = read_owned(s, id)?;
    let binding = close
        .management
        .as_ref()
        .ok_or("management_runner_close_required")?;
    let (_, pos) = load(s, &binding.position_id)?;
    let entry = read_owned(s, &pos.entry_plan_id)?;
    if binding.original_entry_plan_id != entry.plan_id
        || close.order.position_action != PositionAction::SellToClose
        || close.command.scope != entry.command.scope
        || close.account_key != entry.account_key
        || close.canonical_account != entry.canonical_account
        || binding.exit_policy_sha256 != hash(&pos.policy)?
    {
        return Err("management_runner_close_binding_invalid".into());
    }
    Ok((entry, pos.activation_id.clone(), Some(pos), Some(close)))
}
/// Only immutable entry/position/close lineage enters the domain. Live quotes,
/// balances, reservations, controls, leases and source generations are rechecked
/// independently at preparation, after acceptance and on normal C3/C5 dispatch.
pub(crate) fn binding(
    s: &TradeAssemblyService,
    op: &ManagementOperation,
) -> Result<(data_plans::PlanScope, String, String), String> {
    op.validate()?;
    s.require_object("strategy", &op.scope().strategy_id)
        .map_err(|_| "management_runner_not_available")?;
    let (entry, activation, position, close) = original_entry(s, op)?;
    if entry.management.is_some()
        || entry.command.scope != *op.scope()
        || entry.command.mode != "paper"
    {
        return Err("management_runner_entry_scope_invalid".into());
    }
    // Historical owner-bound reads do not acquire current trading authority.
    if let ManagementOperation::Inspect { .. } = op {
        let pos = position
            .as_ref()
            .ok_or("management_runner_position_missing")?;
        let (_, original) =
            submission::record(s, &entry)?.ok_or("management_runner_original_entry_missing")?;
        settlement::verify_historical_entry(s, &entry, &pos.entry_settlement_sha256)?;
        if pos.entry_plan_id != entry.plan_id
            || original.intent["originalRequest"]["activationId"] != activation
        {
            return Err("management_runner_historical_lineage_invalid".into());
        }
        bounded_journal(s, pos)?;
        let canonical = entry
            .canonical_account
            .as_ref()
            .ok_or("management_runner_account_missing")?;
        return Ok((
            op.scope().clone(),
            entry.command.capability_graph_revision.clone(),
            hash(
                &json!({"schemaVersion":"tradeassembly.management_domain.v1","operation":op,"ownerSha256":owner(s)?,"scope":op.scope(),"graph":entry.command.capability_graph_revision,"canonicalAccountSha256":canonical.identity_sha256,"immutableProof":{"entryPlanSha256":hash(&entry)?,"positionId":pos.position_id,"activationId":activation,"entrySettlementSha256":pos.entry_settlement_sha256,"originalIntentSha256":original.intent_sha256,"exitPolicySha256":hash(&pos.policy)?}}),
            )?,
        ));
    }
    let (run, config) = management_run(s, &activation)?;
    let canonical = entry
        .canonical_account
        .as_ref()
        .ok_or("management_runner_account_missing")?;
    if config["strategyId"] != op.scope().strategy_id
        || config["strategyVersionId"] != op.scope().strategy_version_id
        || config["strategySpecHash"] != op.scope().strategy_spec_hash
        || config["capabilityGraphRevisionId"] != entry.command.capability_graph_revision
        || config["canonicalAccount"] != value(canonical)?
    {
        return Err("management_runner_activation_scope_invalid".into());
    }
    let (_, original) =
        submission::record(s, &entry)?.ok_or("management_runner_original_entry_missing")?;
    if original.intent["originalRequest"]["activationId"] != activation {
        return Err("management_runner_entry_activation_invalid".into());
    }
    settlement::entry_basis(s, &entry)?;
    let source = settlement::current_source(s, &entry, &original)?;
    let policy = policy(s, op.scope())?.ok_or("management_runner_policy_required")?;
    if position.as_ref().is_some_and(|p| p.policy != policy) {
        return Err("management_runner_policy_changed".into());
    }
    let mut proof = json!({"entryPlanSha256":hash(&entry)?,"originalIntentSha256":original.intent_sha256,
        "entrySettlementSha256":original.settlement_receipt_sha256,"activationId":activation,
        "exitPolicySha256":hash(&policy)?,"configSha256":hash(&config)?,"sourceBindingSha256":hash(&source)?});
    if let Some(pos) = &position {
        proof["positionId"] = json!(pos.position_id);
        proof["entrySettlementSha256"] = json!(pos.entry_settlement_sha256);
    }
    match op {
        ManagementOperation::Advance { advance, .. } => {
            let pos = position
                .as_ref()
                .ok_or("management_runner_position_missing")?;
            if !pos
                .quantity
                .checked_subtract(pos.closed_quantity)?
                .positive()
            {
                return Err("management_runner_position_flat".into());
            }
            if pos.reservations.is_empty() {
                let mut command = entry.command.clone();
                command.account_collection_id = advance.account_collection_id.clone();
                command.metadata_collection_id = advance.metadata_collection_id.clone();
                command.quote_collection_id = advance.quote_collection_id.clone();
                let current = observations(s, pos, &entry, &command)?;
                execution_window(s, &entry, &advance.execution_occurrence_id)?;
                reversal(
                    s,
                    pos,
                    &entry,
                    advance.reversal_signal_receipt_sha256.as_deref(),
                    s.runtime().clock.trusted_now_ms()?,
                )?;
                monitor_observation(
                    &pos.entry_time,
                    pos.last_quote_observed_at_ms,
                    pos.last_quote_known_at_ms,
                    pos.last_quote_receipt.as_deref(),
                    pos.last_reversal_receipt.as_deref(),
                    &current.quote.time,
                    &current.receipts["quote"],
                    advance.reversal_signal_receipt_sha256.as_deref(),
                )?;
                proof["sourceReceiptSha256"] = json!(current.receipts);
            } else {
                if pos.reservations.len() != 1 {
                    return Err("management_runner_multiple_closes_invalid".into());
                }
                let pending = read_owned(
                    s,
                    pos.reservations
                        .keys()
                        .next()
                        .ok_or("management_runner_close_missing")?,
                )?;
                let b = pending
                    .management
                    .as_ref()
                    .ok_or("management_runner_close_required")?;
                if b.execution_occurrence_id != advance.execution_occurrence_id
                    || pending.command.account_collection_id != advance.account_collection_id
                    || pending.command.metadata_collection_id != advance.metadata_collection_id
                    || pending.command.quote_collection_id != advance.quote_collection_id
                    || advance.reversal_signal_receipt_sha256.is_some()
                {
                    return Err("management_runner_retained_close_command_mismatch".into());
                }
                if submission::record(s, &pending)?.is_some() {
                    return Err("management_runner_close_reconciliation_required".into());
                }
                snapshot(s, &pending)?;
                proof["retainedClosePlanSha256"] = json!(hash(&pending)?);
            }
        }
        ManagementOperation::SubmitClose {
            submission: command,
            ..
        } => {
            let close = close.as_ref().ok_or("management_runner_close_missing")?;
            if command.activation_id != activation || submission::record(s, close)?.is_some() {
                return Err("management_runner_close_already_submitted_reconcile_required".into());
            }
            let (_, _, _) = transport::request(s, command)?;
            validate_approval(s, close, &run, &config)?;
        }
        ManagementOperation::ReconcileClose { .. } | ManagementOperation::SettleClose { .. } => {
            let close = close.as_ref().ok_or("management_runner_close_missing")?;
            let (_, record) = submission::record(s, close)?.ok_or("broker_v2_state_missing")?;
            if record.intent["originalRequest"]["activationId"] != activation {
                return Err("management_runner_close_activation_invalid".into());
            }
            let source = settlement::current_source(s, close, &record)?;
            let (operation, capability, purpose) =
                if matches!(op, ManagementOperation::SettleClose { .. }) {
                    (
                        tradeassembly_plugin_sdk::close_settlement::OPERATION,
                        tradeassembly_plugin_sdk::close_settlement::CAPABILITY,
                        "settlement",
                    )
                } else {
                    let package = s
                        .runtime()
                        .plugin_packages
                        .get(&source.package_sha256)?
                        .ok_or("management_runner_broker_package_missing")?;
                    let (operation, capability) =
                        crate::broker_submission::v2::recovery_lookup_binding(
                            &package.manifest,
                            crate::broker_submission::v2::SUBMIT_OPERATION,
                        )?;
                    (operation, capability, "recovery")
                };
            require_read_capability(
                s,
                close,
                &source,
                &record.intent["accountRef"],
                operation,
                capability,
                purpose,
            )?;
            proof["closeOriginalIntentSha256"] = json!(record.intent_sha256);
            proof["closeOriginalKeySha256"] = json!(hash(&record.original_key)?);
        }
        ManagementOperation::Inspect { .. } => {
            bounded_journal(
                s,
                position
                    .as_ref()
                    .ok_or("management_runner_position_missing")?,
            )?;
        }
        ManagementOperation::Register { .. } => {}
    }
    if let Some(close) = &close {
        proof["closePlanSha256"] = json!(hash(close)?);
        proof["closeBinding"] = json!(close.management);
    }
    Ok((
        op.scope().clone(),
        entry.command.capability_graph_revision.clone(),
        hash(&json!({
        "schemaVersion":"tradeassembly.management_domain.v1","operation":op,"ownerSha256":owner(s)?,
        "scope":op.scope(),"graph":entry.command.capability_graph_revision,"canonicalAccountSha256":canonical.identity_sha256,
        "immutableProof":proof}))?,
    ))
}

fn require_read_capability(
    s: &TradeAssemblyService,
    plan: &OrderPlan,
    binding: &crate::ports::broker_submission::BrokerPreparedBinding,
    account_ref: &Value,
    operation: &str,
    capability: &str,
    purpose: &str,
) -> Result<(), String> {
    let manifest = s
        .runtime()
        .plugins
        .get_manifest(&binding.plugin_ref)?
        .ok_or("management_runner_read_operation_missing")?;
    if !manifest["manifest"]["operations"]
        .as_array()
        .is_some_and(|ops| {
            ops.iter().any(|op| {
                op["id"] == operation
                    && op["capability"] == capability
                    && op["effect"] == "read"
                    && op["purpose"] == purpose
            })
        })
    {
        return Err("management_runner_read_operation_invalid".into());
    }
    let resolved = crate::service::providers::resolve_capability(
        s,
        crate::service::providers::requirement_from_body(&json!({
            "capability":capability,"pluginRef":binding.plugin_ref,"pluginInstanceRef":binding.plugin_instance_ref,
            "operation":operation,"mode":"paper","purpose":purpose,"accountRef":account_ref,
            "assetClass":plan.order.instrument.asset_class,"instrumentFamily":plan.order.instrument.instrument_family
        })),
    );
    if !resolved["candidates"].as_array().is_some_and(|xs| {
        xs.iter().any(|x| {
            x["pluginInstanceRef"] == binding.plugin_instance_ref
                && x["operation"]["id"] == operation
                && x["blockers"].as_array().is_some_and(Vec::is_empty)
        })
    }) {
        return Err("management_runner_read_permission_changed".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PositionProjection {
    pub position_id: String,
    pub entry_plan_id: String,
    pub activation_id: String,
    pub entry_settlement_sha256: String,
    pub exit_policy_sha256: String,
    pub quantity: Decimal,
    pub closed_quantity: Decimal,
    pub basis: Decimal,
    pub high_water: Option<Decimal>,
    pub generation: u64,
    pub state_sha256: String,
    pub reservations: BTreeMap<String, Decimal>,
}
impl PositionProjection {
    fn from_position(p: &Position) -> Result<Self, String> {
        Ok(Self {
            position_id: p.position_id.clone(),
            entry_plan_id: p.entry_plan_id.clone(),
            activation_id: p.activation_id.clone(),
            entry_settlement_sha256: p.entry_settlement_sha256.clone(),
            exit_policy_sha256: hash(&p.policy)?,
            quantity: p.quantity,
            closed_quantity: p.closed_quantity,
            basis: p.basis,
            high_water: p.high_water,
            generation: p.generation,
            state_sha256: p.state_sha256.clone(),
            reservations: p.reservations.clone(),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalEntry {
    pub schema_version: String,
    pub position_id: String,
    pub entry_plan_id: String,
    pub event: String,
    pub receipt_sha256: String,
    pub quantity: Decimal,
    pub closed_quantity: Decimal,
    pub high_water: Option<Decimal>,
    pub generation: u64,
    pub historical_outcome: bool,
    pub execution_authority_granted: bool,
}
fn bounded_journal(
    s: &TradeAssemblyService,
    position: &Position,
) -> Result<Vec<JournalEntry>, String> {
    let mut entries = Vec::new();
    for (key, raw) in s.runtime().storage.list_json(JOURNAL)? {
        if !key.starts_with(&format!("{}:", position.position_id)) {
            continue;
        }
        let entry: JournalEntry =
            serde_json::from_value(raw).map_err(|_| "management_runner_journal_invalid")?;
        if entry.schema_version != "tradeassembly.position_journal.v1"
            || entry.position_id != position.position_id
            || entry.entry_plan_id != position.entry_plan_id
            || entry.generation > position.generation
            || !entry.historical_outcome
            || entry.execution_authority_granted
        {
            return Err("management_runner_journal_invalid".into());
        }
        entries.push(entry);
        if entries.len() > MAX_JOURNAL {
            return Err("management_runner_journal_paging_unsupported".into());
        }
    }
    entries.sort_by_key(|e| e.generation);
    if entries
        .windows(2)
        .any(|w| w[0].generation >= w[1].generation)
    {
        return Err("management_runner_journal_invalid".into());
    }
    Ok(entries)
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloseSettlementProjection {
    pub receipt_sha256: String,
    pub gross_proceeds: Decimal,
    pub fee_cash: Decimal,
    pub filled_quantity: Decimal,
    pub released_hold: Decimal,
    pub posting_id: String,
    pub ledger_epoch: String,
    pub posting_sequence: u64,
    pub checkpoint_sequence: u64,
    pub provider_response_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagementSnapshot {
    pub schema_version: String,
    pub command_sha256: String,
    pub operation: String,
    pub scope: data_plans::PlanScope,
    pub canonical_account_sha256: String,
    pub position: PositionProjection,
    pub close_plan_id: Option<String>,
    pub close_order: Option<BrokerOrderV2>,
    pub phase: Option<submission::Phase>,
    pub retained_cash: Option<Decimal>,
    pub filled_quantity: Option<Decimal>,
    pub submission: Option<transport::SubmissionCommand>,
    pub settlement: Option<CloseSettlementProjection>,
    pub journal: Vec<JournalEntry>,
    pub journal_paging_supported: bool,
    pub journal_included: bool,
    pub journal_completeness: String,
    pub observed_at_ms: i64,
    pub historical_outcome: bool,
    pub execution_authority_granted: bool,
    pub snapshot_sha256: String,
}
fn snapshot_hash(v: &ManagementSnapshot) -> Result<String, String> {
    let mut v = v.clone();
    v.snapshot_sha256.clear();
    hash(&v)
}
pub(crate) fn stored(
    s: &TradeAssemblyService,
    op: &ManagementOperation,
    command: &str,
) -> Result<Option<ManagementSnapshot>, String> {
    let Some(raw) = s.runtime().storage.get_json(SNAPSHOTS, command)? else {
        return Ok(None);
    };
    let v: ManagementSnapshot = serde_json::from_value(raw["snapshot"].clone())
        .map_err(|_| "management_runner_snapshot_invalid")?;
    validate_snapshot(s, op, command, &v)?;
    Ok(Some(v))
}
pub(crate) fn validate_snapshot(
    s: &TradeAssemblyService,
    op: &ManagementOperation,
    command: &str,
    v: &ManagementSnapshot,
) -> Result<(), String> {
    s.require_object("strategy", &op.scope().strategy_id)
        .map_err(|_| "management_runner_not_available")?;
    if v.schema_version != "tradeassembly.management_snapshot.v1"
        || v.command_sha256 != command
        || v.operation != op.action()
        || v.scope != *op.scope()
        || !v.historical_outcome
        || v.execution_authority_granted
        || v.journal_paging_supported
        || v.journal.len() > MAX_JOURNAL
        || v.journal_included != matches!(op, ManagementOperation::Inspect { .. })
        || v.journal_completeness
            != if v.journal_included {
                "managed_position_namespace_only"
            } else {
                "not_included"
            }
        || v.observed_at_ms <= 0
        || snapshot_hash(v)? != v.snapshot_sha256
    {
        return Err("management_runner_snapshot_invalid".into());
    }
    let raw = s
        .runtime()
        .storage
        .get_json(SNAPSHOTS, command)?
        .ok_or("management_runner_snapshot_missing")?;
    if raw["ownerSha256"] != owner(s)? || raw["snapshot"] != value(v)? {
        return Err("management_runner_snapshot_invalid".into());
    }
    Ok(())
}
pub(crate) fn project(
    s: &TradeAssemblyService,
    op: &ManagementOperation,
    command: &str,
    body: Value,
) -> Result<ManagementSnapshot, String> {
    if let Some(v) = stored(s, op, command)? {
        return Ok(v);
    }
    let (entry, activation, current_position, requested_close) = original_entry(s, op)?;
    let position = if !body["position"].is_null() {
        let p: Position = serde_json::from_value(body["position"].clone())
            .map_err(|_| "management_runner_position_result_invalid")?;
        let mut check = p.clone();
        seal(&mut check)?;
        if check != p
            || p.owner_sha256 != owner(s)?
            || p.entry_plan_id != entry.plan_id
            || p.activation_id != activation
        {
            return Err("management_runner_position_result_invalid".into());
        }
        p
    } else {
        current_position.ok_or("management_runner_position_result_missing")?
    };
    let mut v = ManagementSnapshot {
        schema_version: "tradeassembly.management_snapshot.v1".into(),
        command_sha256: command.into(),
        operation: op.action().into(),
        scope: op.scope().clone(),
        canonical_account_sha256: entry
            .canonical_account
            .as_ref()
            .ok_or("management_runner_account_missing")?
            .identity_sha256
            .clone(),
        position: PositionProjection::from_position(&position)?,
        close_plan_id: None,
        close_order: None,
        phase: None,
        retained_cash: None,
        filled_quantity: None,
        submission: None,
        settlement: None,
        journal: if matches!(op, ManagementOperation::Inspect { .. }) {
            bounded_journal(s, &position)?
        } else {
            Vec::new()
        },
        journal_paging_supported: false,
        journal_included: matches!(op, ManagementOperation::Inspect { .. }),
        journal_completeness: if matches!(op, ManagementOperation::Inspect { .. }) {
            "managed_position_namespace_only"
        } else {
            "not_included"
        }
        .into(),
        observed_at_ms: s.runtime().clock.trusted_now_ms()?,
        historical_outcome: true,
        execution_authority_granted: false,
        snapshot_sha256: String::new(),
    };
    let mut close = requested_close;
    if let ManagementOperation::Advance { .. } = op {
        if !body["decision"].is_null() {
            let command: transport::SubmissionCommand =
                serde_json::from_value(body["decision"]["submission"].clone())
                    .map_err(|_| "management_runner_decision_result_invalid")?;
            command.validate()?;
            if command.activation_id != activation {
                return Err("management_runner_decision_result_invalid".into());
            }
            close = Some(read_owned(s, &command.plan_id)?);
            v.submission = Some(command);
        }
    }
    if let Some(close) = close {
        let b = close
            .management
            .as_ref()
            .ok_or("management_runner_close_required")?;
        if b.position_id != position.position_id || b.original_entry_plan_id != entry.plan_id {
            return Err("management_runner_close_binding_invalid".into());
        }
        v.close_plan_id = Some(close.plan_id.clone());
        v.close_order = Some(close.order.clone());
        if let Some((_, record)) = submission::record(s, &close)? {
            if matches!(
                op,
                ManagementOperation::SubmitClose { .. }
                    | ManagementOperation::ReconcileClose { .. }
            ) {
                if body["planId"] != close.plan_id {
                    return Err("management_runner_submission_result_invalid".into());
                }
                let phase: submission::Phase =
                    serde_json::from_value(body["submission"]["phase"].clone())
                        .map_err(|_| "management_runner_submission_result_invalid")?;
                let cash: Decimal =
                    serde_json::from_value(body["submission"]["retainedCash"].clone())
                        .map_err(|_| "management_runner_submission_result_invalid")?;
                let filled: Decimal =
                    serde_json::from_value(body["submission"]["filledQuantity"].clone())
                        .map_err(|_| "management_runner_submission_result_invalid")?;
                if cash.negative()
                    || (cash != Decimal::ZERO && cash != close.purchase_cash)
                    || filled.negative()
                    || filled.compare(close.order.quantity)?.is_gt()
                {
                    return Err("management_runner_submission_result_invalid".into());
                }
                v.phase = Some(phase);
                v.retained_cash = Some(cash);
                v.filled_quantity = Some(filled);
            } else {
                v.phase = Some(record.phase);
                v.retained_cash = Some(record.retained_cash);
                v.filled_quantity = Some(record.filled_quantity);
            }
            if let ManagementOperation::SettleClose { .. } = op {
                let id = record
                    .settlement_receipt_sha256
                    .as_deref()
                    .ok_or("management_runner_settlement_missing")?;
                if body["receiptSha256"] != id {
                    return Err("management_runner_settlement_result_invalid".into());
                }
                let receipt = read_close(s, id)?;
                v.settlement = Some(CloseSettlementProjection {
                    receipt_sha256: id.into(),
                    gross_proceeds: receipt.observation.gross_proceeds,
                    fee_cash: receipt.observation.fee_cash,
                    filled_quantity: *receipt.observation.order.filled_quantity.required()?,
                    released_hold: receipt.released_hold,
                    posting_id: receipt.observation.posting_id,
                    ledger_epoch: receipt.observation.checkpoint.ledger_epoch,
                    posting_sequence: receipt.observation.posting_sequence,
                    checkpoint_sequence: receipt.observation.checkpoint.sequence,
                    provider_response_sha256: receipt.provider_response_sha256,
                });
                // Settlement atomically advanced the position before projection.
                let (_, position) = load(s, &b.position_id)?;
                v.position = PositionProjection::from_position(&position)?;
            }
        }
    }
    v.snapshot_sha256 = snapshot_hash(&v)?;
    let ctx = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("management-snapshot:{command}"))?,
    );
    s.runtime().storage.put_json_if_absent(
        SNAPSHOTS,
        command,
        json!({"ownerSha256":owner(s)?,"snapshot":v}),
        &ctx,
    )?;
    let stored = stored(s, op, command)?.ok_or("management_runner_snapshot_missing")?;
    if stored != v {
        return Err("management_runner_snapshot_conflict".into());
    }
    Ok(v)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InspectCommand {
    scope: data_plans::PlanScope,
    position_id: String,
}
pub(crate) fn inspect_response(s: &TradeAssemblyService, body: &Value) -> ServiceResponse {
    let result = (|| {
        // Control-plane metadata is host-owned; only the nested finite DTO is
        // parsed from caller fields, matching existing manager command patterns.
        let command: InspectCommand =
            serde_json::from_value(json!({"scope":body["scope"],"positionId":body["positionId"]}))
                .map_err(|_| "management_runner_inspect_invalid")?;
        let op = ManagementOperation::Inspect {
            scope: command.scope,
            position_id: command.position_id.clone(),
        };
        binding(s, &op)?;
        let (_, position) = load(s, &command.position_id)?;
        let journal = bounded_journal(s, &position)?;
        Ok::<_, String>(
            json!({"position":position,"journal":journal,"journalPagingSupported":false,"executionAuthorityGranted":false}),
        )
    })();
    match result {
        Ok(v) => ServiceResponse::ok(v),
        Err(e) => ServiceResponse::conflict(&e),
    }
}
