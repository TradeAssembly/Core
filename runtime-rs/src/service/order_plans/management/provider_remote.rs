//! Finite V7 transport for non-atomic provider observations and owned positions.
//! Historical projections never grant execution or certify coherent cash posting.
use super::*;
use tradeassembly_plugin_sdk::settlement_observation_v2 as wire;

pub const INSPECT: &str = "/product/strategies/provider-positions/inspect";
const SNAPSHOTS: &str = "provider_management_snapshots_v7";
const MAX_JOURNAL: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderManagementOperation {
    ObserveSettlement {
        scope: data_plans::PlanScope,
        plan_id: String,
    },
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
    Inspect {
        scope: data_plans::PlanScope,
        position_id: String,
    },
}
impl ProviderManagementOperation {
    pub fn scope(&self) -> &data_plans::PlanScope {
        match self {
            Self::ObserveSettlement { scope, .. }
            | Self::Register { scope, .. }
            | Self::Advance { scope, .. }
            | Self::SubmitClose { scope, .. }
            | Self::ReconcileClose { scope, .. }
            | Self::SettleClose { scope, .. }
            | Self::Inspect { scope, .. } => scope,
        }
    }
    pub fn resource_kind(&self) -> &'static str {
        if matches!(self, Self::ObserveSettlement { .. }) {
            "strategy_order_plan"
        } else {
            remote::RESOURCE_KIND
        }
    }
    fn legacy(&self) -> Option<remote::ManagementOperation> {
        use remote::ManagementOperation as M;
        Some(match self {
            Self::ObserveSettlement { .. } => return None,
            Self::Register {
                scope,
                entry_plan_id,
                activation_id,
            } => M::Register {
                scope: scope.clone(),
                entry_plan_id: entry_plan_id.clone(),
                activation_id: activation_id.clone(),
            },
            Self::Advance { scope, advance } => M::Advance {
                scope: scope.clone(),
                advance: advance.clone(),
            },
            Self::SubmitClose { scope, submission } => M::SubmitClose {
                scope: scope.clone(),
                submission: submission.clone(),
            },
            Self::ReconcileClose { scope, plan_id } => M::ReconcileClose {
                scope: scope.clone(),
                plan_id: plan_id.clone(),
            },
            Self::SettleClose { scope, plan_id } => M::SettleClose {
                scope: scope.clone(),
                plan_id: plan_id.clone(),
            },
            Self::Inspect { scope, position_id } => M::Inspect {
                scope: scope.clone(),
                position_id: position_id.clone(),
            },
        })
    }
    pub fn action(&self) -> &'static str {
        match self {
            Self::ObserveSettlement { .. } | Self::SettleClose { .. } => {
                "strategy.order_plan.settle"
            }
            Self::Register { .. } => "strategy.position.register",
            Self::Advance { .. } => "strategy.position.advance",
            Self::SubmitClose { .. } => "strategy.order_plan.submit",
            Self::ReconcileClose { .. } => "strategy.order_plan.reconcile",
            Self::Inspect { .. } => "strategy.position.inspect",
        }
    }
    pub fn body(&self) -> (&'static str, Value) {
        match self {
            Self::ObserveSettlement { scope, plan_id } => {
                (SETTLE, json!({"scope":scope,"planId":plan_id}))
            }
            Self::Inspect { scope, position_id } => {
                (INSPECT, json!({"scope":scope,"positionId":position_id}))
            }
            _ => self.legacy().expect("closed V7 intent").body(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if let Self::ObserveSettlement { scope, plan_id } = self {
            let plan_ok = plan_id.strip_prefix("orderplan_").is_some_and(|id| {
                id.len() == 64
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
            if !plan_ok
                || [
                    &scope.strategy_id,
                    &scope.strategy_version_id,
                    &scope.strategy_spec_hash,
                ]
                .iter()
                .any(|s| {
                    s.is_empty()
                        || s.len() > 256
                        || s.trim() != s.as_str()
                        || s.chars().any(char::is_control)
                })
            {
                return Err("provider_management_command_invalid".into());
            }
            Ok(())
        } else {
            self.legacy().expect("closed V7 intent").validate()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum EntryEvidence {
    ObservedV2 {
        observation_receipt_sha256: String,
        basis: String,
        coherence: String,
        cash_reconciled: bool,
    },
}
impl EntryEvidence {
    fn observed(id: &str) -> Self {
        Self::ObservedV2 {
            observation_receipt_sha256: id.into(),
            basis: "provider_reported_average_fill".into(),
            coherence: "non_atomic".into(),
            cash_reconciled: false,
        }
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::ObservedV2 {
                observation_receipt_sha256,
                basis,
                coherence,
                cash_reconciled,
            } if observation_receipt_sha256.len() == 64
                && observation_receipt_sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && basis == "provider_reported_average_fill"
                && coherence == "non_atomic"
                && !cash_reconciled =>
            {
                Ok(())
            }
            _ => Err("provider_management_entry_evidence_invalid".into()),
        }
    }
}
pub(super) fn historical_entry_evidence(
    s: &TradeAssemblyService,
    entry: &OrderPlan,
    pos: &Position,
) -> Result<EntryEvidence, String> {
    let id = pos
        .entry_observation_sha256
        .as_deref()
        .ok_or("provider_management_observed_entry_required")?;
    if pos.policy.entry_basis != "provider_reported_average_fill"
        || pos.entry_cash_reconciled != Some(false)
        || !pos.entry_settlement_sha256.is_empty()
    {
        return Err("provider_management_observed_entry_required".into());
    }
    let (quantity, basis) = super::super::provisional::historical_entry_basis(s, entry, id)?;
    if quantity != pos.quantity || basis != pos.basis {
        return Err("provider_management_entry_pin_invalid".into());
    }
    Ok(EntryEvidence::observed(id))
}
pub(super) fn entry_evidence_for_binding(
    s: &TradeAssemblyService,
    entry: &OrderPlan,
    pos: Option<&Position>,
    policy: &ExitPolicy,
) -> Result<EntryEvidence, String> {
    if policy.entry_basis != "provider_reported_average_fill" {
        return Err("provider_management_observed_entry_required".into());
    }
    if let Some(pos) = pos {
        historical_entry_evidence(s, entry, pos)
    } else {
        let (_, _, id, _) = entry_basis(s, entry, policy, None)?;
        Ok(EntryEvidence::observed(&id))
    }
}
fn original(
    s: &TradeAssemblyService,
    op: &ProviderManagementOperation,
) -> Result<(OrderPlan, String, Option<Position>, Option<OrderPlan>), String> {
    if let ProviderManagementOperation::ObserveSettlement { plan_id, .. } = op {
        let plan = read_owned(s, plan_id)?;
        if plan.management.is_some() {
            return remote::close_entry(s, plan_id);
        }
        let (_, r) = submission::record(s, &plan)?.ok_or("broker_v2_state_missing")?;
        let activation = r.intent["originalRequest"]["activationId"]
            .as_str()
            .ok_or("provider_management_activation_missing")?
            .into();
        Ok((plan, activation, None, None))
    } else {
        remote::original_entry(s, &op.legacy().expect("closed V7 intent"))
    }
}
pub(crate) fn binding(
    s: &TradeAssemblyService,
    op: &ProviderManagementOperation,
) -> Result<(data_plans::PlanScope, String, String), String> {
    op.validate()?;
    s.require_object("strategy", &op.scope().strategy_id)
        .map_err(|_| "provider_management_not_available")?;
    let (entry, _, _, close) = original(s, op)?;
    if entry.command.scope != *op.scope() || entry.command.mode != "paper" {
        return Err("provider_management_scope_invalid".into());
    }
    if let Some(close) = &close {
        let b = close.management.as_ref().ok_or("managed_close_required")?;
        let (_, pos) = load(s, &b.position_id)?;
        historical_entry_evidence(s, &entry, &pos)?;
    }
    let canonical = entry
        .canonical_account
        .as_ref()
        .ok_or("provider_management_account_missing")?;
    let proof = if matches!(
        op,
        ProviderManagementOperation::ObserveSettlement { .. }
            | ProviderManagementOperation::SettleClose { .. }
            | ProviderManagementOperation::ReconcileClose { .. }
    ) {
        let plan = close.as_ref().unwrap_or(&entry);
        let (r, source) = if matches!(op, ProviderManagementOperation::ReconcileClose { .. }) {
            let (_, r) = submission::record(s, plan)?.ok_or("broker_v2_state_missing")?;
            if !super::super::provisional::selected(s, plan, &r)? {
                return Err("provider_observation_explicit_selection_required".into());
            }
            let source = settlement::current_source(s, plan, &r)?;
            let package = s
                .runtime()
                .plugin_packages
                .get(&source.package_sha256)?
                .ok_or("provider_management_package_missing")?;
            let (operation, capability) = crate::broker_submission::v2::recovery_lookup_binding(
                &package.manifest,
                crate::broker_submission::v2::SUBMIT_OPERATION,
            )?;
            remote::require_read_capability(
                s,
                plan,
                &source,
                &r.intent["accountRef"],
                operation,
                capability,
                "recovery",
            )?;
            (r, source)
        } else {
            super::super::provisional::preflight(s, plan)?
        };
        json!({"planId":plan.plan_id,"planSha256":hash(plan)?,"originalIntentSha256":r.intent_sha256,"originalKeySha256":hash(&r.original_key)?,"sourceBindingSha256":hash(&source)?,"canonicalAccountSha256":canonical.identity_sha256})
    } else {
        let legacy = op.legacy().expect("closed V7 intent");
        // V7 is explicitly for observed entries. V6 retains coherent legacy semantics.
        let (_, _, position, _) = original(s, op)?;
        let policy = if matches!(op, ProviderManagementOperation::Inspect { .. }) {
            position
                .as_ref()
                .ok_or("provider_management_position_missing")?
                .policy
                .clone()
        } else {
            policy(s, op.scope())?.ok_or("provider_management_policy_required")?
        };
        if policy.entry_basis != "provider_reported_average_fill" {
            return Err("provider_management_observed_entry_required".into());
        }
        if let Some(pos) = position.as_ref() {
            historical_entry_evidence(s, &entry, pos)?;
        }
        let (_, graph, domain) = remote::binding_for_provider_v7(s, &legacy)?;
        if graph != entry.command.capability_graph_revision {
            return Err("provider_management_graph_mismatch".into());
        }
        json!({"managementDomainSha256":domain})
    };
    Ok((
        op.scope().clone(),
        entry.command.capability_graph_revision.clone(),
        hash(
            &json!({"schemaVersion":"tradeassembly.provider-management-domain.v7","operation":op,"resourceKind":op.resource_kind(),"ownerSha256":owner(s)?,"scope":op.scope(),"graph":entry.command.capability_graph_revision,"canonicalAccountSha256":canonical.identity_sha256,"immutableProof":proof}),
        )?,
    ))
}

mod observation;
pub use observation::{
    AcceptedObservationProjection, ObservationProjection, ReportedMoney, ReportedObservation,
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPositionProjection {
    pub position_id: String,
    pub entry_plan_id: String,
    pub activation_id: String,
    pub entry_evidence: EntryEvidence,
    pub exit_policy_sha256: String,
    pub quantity: Decimal,
    pub closed_quantity: Decimal,
    pub basis: Decimal,
    pub high_water: Option<Decimal>,
    pub generation: u64,
    pub state_sha256: String,
    pub reservations: BTreeMap<String, Decimal>,
}
impl ProviderPositionProjection {
    fn from_position(
        s: &TradeAssemblyService,
        entry: &OrderPlan,
        p: &Position,
    ) -> Result<Self, String> {
        Ok(Self {
            position_id: p.position_id.clone(),
            entry_plan_id: p.entry_plan_id.clone(),
            activation_id: p.activation_id.clone(),
            entry_evidence: historical_entry_evidence(s, entry, p)?,
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
pub struct ProviderJournalEntry {
    pub schema_version: String,
    pub source_journal_sha256: String,
    pub position_id: String,
    pub entry_plan_id: String,
    pub entry_evidence: EntryEvidence,
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
    entry: &OrderPlan,
    pos: &Position,
) -> Result<Vec<ProviderJournalEntry>, String> {
    let evidence = historical_entry_evidence(s, entry, pos)?;
    let mut rows = Vec::new();
    for (key, raw) in s.runtime().storage.list_json(JOURNAL)? {
        if !key.starts_with(&format!("{}:", pos.position_id)) {
            continue;
        }
        let mut base = raw.clone();
        let pin = base
            .as_object_mut()
            .ok_or("provider_management_journal_invalid")?
            .remove("entryObservationSha256");
        let basis = base.as_object_mut().unwrap().remove("entryBasis");
        let reconciled = base.as_object_mut().unwrap().remove("entryCashReconciled");
        if pin != Some(json!(pos.entry_observation_sha256))
            || basis != Some(json!("provider_reported_average_fill"))
            || reconciled != Some(json!(false))
        {
            return Err("provider_management_journal_invalid".into());
        }
        let row: remote::JournalEntry =
            serde_json::from_value(base).map_err(|_| "provider_management_journal_invalid")?;
        if row.schema_version != "tradeassembly.position_journal.v1"
            || row.position_id != pos.position_id
            || row.entry_plan_id != entry.plan_id
            || row.generation > pos.generation
            || !row.historical_outcome
            || row.execution_authority_granted
        {
            return Err("provider_management_journal_invalid".into());
        }
        rows.push(ProviderJournalEntry {
            schema_version: "tradeassembly.provider-management-journal.v7".into(),
            source_journal_sha256: hash(&raw)?,
            position_id: row.position_id,
            entry_plan_id: row.entry_plan_id,
            entry_evidence: evidence.clone(),
            event: row.event,
            receipt_sha256: row.receipt_sha256,
            quantity: row.quantity,
            closed_quantity: row.closed_quantity,
            high_water: row.high_water,
            generation: row.generation,
            historical_outcome: true,
            execution_authority_granted: false,
        });
        if rows.len() > MAX_JOURNAL {
            return Err("provider_management_journal_paging_unsupported".into());
        }
    }
    rows.sort_by_key(|r| r.generation);
    if rows.windows(2).any(|w| w[0].generation >= w[1].generation) {
        return Err("provider_management_journal_invalid".into());
    }
    Ok(rows)
}
pub(super) fn validate_historical_journal(
    s: &TradeAssemblyService,
    entry: &OrderPlan,
    pos: &Position,
) -> Result<(), String> {
    bounded_journal(s, entry, pos).map(|_| ())
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderManagementSnapshot {
    pub schema_version: String,
    pub command_sha256: String,
    pub operation: String,
    pub scope: data_plans::PlanScope,
    pub canonical_account_sha256: String,
    pub entry_plan_id: String,
    pub position: Option<ProviderPositionProjection>,
    pub observation: Option<ObservationProjection>,
    pub close_plan_id: Option<String>,
    pub close_order: Option<BrokerOrderV2>,
    pub phase: Option<submission::Phase>,
    pub retained_cash: Option<Decimal>,
    pub filled_quantity: Option<Decimal>,
    pub submission: Option<transport::SubmissionCommand>,
    pub journal: Vec<ProviderJournalEntry>,
    pub journal_paging_supported: bool,
    pub journal_included: bool,
    pub journal_completeness: String,
    pub observed_at_ms: i64,
    pub historical_outcome: bool,
    pub execution_authority_granted: bool,
    pub snapshot_sha256: String,
}
fn snapshot_hash(v: &ProviderManagementSnapshot) -> Result<String, String> {
    let mut v = v.clone();
    v.snapshot_sha256.clear();
    hash(&v)
}
pub(crate) fn stored(
    s: &TradeAssemblyService,
    op: &ProviderManagementOperation,
    command: &str,
) -> Result<Option<ProviderManagementSnapshot>, String> {
    let Some(raw) = s.runtime().storage.get_json(SNAPSHOTS, command)? else {
        return Ok(None);
    };
    let v = serde_json::from_value(raw["snapshot"].clone())
        .map_err(|_| "provider_management_snapshot_invalid")?;
    validate_snapshot(s, op, command, &v)?;
    Ok(Some(v))
}
pub(crate) fn validate_snapshot(
    s: &TradeAssemblyService,
    op: &ProviderManagementOperation,
    command: &str,
    v: &ProviderManagementSnapshot,
) -> Result<(), String> {
    s.require_object("strategy", &op.scope().strategy_id)
        .map_err(|_| "provider_management_not_available")?;
    if v.schema_version != "tradeassembly.provider-management-snapshot.v7"
        || v.command_sha256 != command
        || v.operation != op.action()
        || v.scope != *op.scope()
        || !v.historical_outcome
        || v.execution_authority_granted
        || v.observed_at_ms <= 0
        || v.journal_paging_supported
        || v.journal.len() > MAX_JOURNAL
        || v.journal_included != matches!(op, ProviderManagementOperation::Inspect { .. })
        || v.journal_completeness
            != if v.journal_included {
                "managed_position_namespace_only"
            } else {
                "not_included"
            }
        || snapshot_hash(v)? != v.snapshot_sha256
    {
        return Err("provider_management_snapshot_invalid".into());
    }
    let carries_position = matches!(
        op,
        ProviderManagementOperation::Register { .. }
            | ProviderManagementOperation::Advance { .. }
            | ProviderManagementOperation::Inspect { .. }
    );
    let carries_submission = matches!(
        op,
        ProviderManagementOperation::SubmitClose { .. }
            | ProviderManagementOperation::ReconcileClose { .. }
    );
    if carries_position != v.position.is_some()
        || carries_submission
            != (v.phase.is_some() && v.retained_cash.is_some() && v.filled_quantity.is_some())
        || (!carries_submission
            && (v.phase.is_some() || v.retained_cash.is_some() || v.filled_quantity.is_some()))
        || (matches!(
            op,
            ProviderManagementOperation::ObserveSettlement { .. }
                | ProviderManagementOperation::SettleClose { .. }
        ) && v.observation.is_none())
    {
        return Err("provider_management_snapshot_invalid".into());
    }
    if let Some(pos) = &v.position {
        pos.entry_evidence.validate()?;
    }
    for row in &v.journal {
        row.entry_evidence.validate()?;
        if row.schema_version != "tradeassembly.provider-management-journal.v7"
            || !row.historical_outcome
            || row.execution_authority_granted
        {
            return Err("provider_management_snapshot_invalid".into());
        }
    }
    if let Some(o) = &v.observation {
        if o.cash_reconciled || o.released_hold != Decimal::ZERO {
            return Err("provider_management_snapshot_invalid".into());
        }
    }
    let raw = s
        .runtime()
        .storage
        .get_json(SNAPSHOTS, command)?
        .ok_or("provider_management_snapshot_missing")?;
    if raw["ownerSha256"] != owner(s)? || raw["snapshot"] != value(v)? {
        return Err("provider_management_snapshot_invalid".into());
    }
    Ok(())
}
pub(crate) fn project(
    s: &TradeAssemblyService,
    op: &ProviderManagementOperation,
    command: &str,
    body: Value,
) -> Result<ProviderManagementSnapshot, String> {
    if let Some(v) = stored(s, op, command)? {
        return Ok(v);
    }
    let (entry, activation, lineage_position, mut close) = original(s, op)?;
    let carries_position = matches!(
        op,
        ProviderManagementOperation::Register { .. }
            | ProviderManagementOperation::Advance { .. }
            | ProviderManagementOperation::Inspect { .. }
    );
    let position = if carries_position && !body["position"].is_null() {
        let p: Position = serde_json::from_value(body["position"].clone())
            .map_err(|_| "provider_management_position_result_invalid")?;
        let mut checked = p.clone();
        seal(&mut checked)?;
        if checked != p
            || p.owner_sha256 != owner(s)?
            || p.entry_plan_id != entry.plan_id
            || p.activation_id != activation
        {
            return Err("provider_management_position_result_invalid".into());
        }
        Some(p)
    } else {
        None
    };
    if carries_position && position.is_none() {
        return Err("provider_management_position_result_missing".into());
    }
    let mut v = ProviderManagementSnapshot {
        schema_version: "tradeassembly.provider-management-snapshot.v7".into(),
        command_sha256: command.into(),
        operation: op.action().into(),
        scope: op.scope().clone(),
        canonical_account_sha256: entry
            .canonical_account
            .as_ref()
            .ok_or("provider_management_account_missing")?
            .identity_sha256
            .clone(),
        entry_plan_id: entry.plan_id.clone(),
        position: position
            .as_ref()
            .map(|p| ProviderPositionProjection::from_position(s, &entry, p))
            .transpose()?,
        observation: None,
        close_plan_id: None,
        close_order: None,
        phase: None,
        retained_cash: None,
        filled_quantity: None,
        submission: None,
        journal: if matches!(op, ProviderManagementOperation::Inspect { .. }) {
            bounded_journal(
                s,
                &entry,
                position
                    .as_ref()
                    .ok_or("provider_management_position_missing")?,
            )?
        } else {
            Vec::new()
        },
        journal_paging_supported: false,
        journal_included: matches!(op, ProviderManagementOperation::Inspect { .. }),
        journal_completeness: if matches!(op, ProviderManagementOperation::Inspect { .. }) {
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
    if let ProviderManagementOperation::Advance { .. } = op {
        if !body["decision"].is_null() {
            let c: transport::SubmissionCommand =
                serde_json::from_value(body["decision"]["submission"].clone())
                    .map_err(|_| "provider_management_decision_invalid")?;
            c.validate()?;
            if c.activation_id != activation {
                return Err("provider_management_decision_invalid".into());
            }
            close = Some(read_owned(s, &c.plan_id)?);
            v.submission = Some(c);
        }
    }
    let observed_plan = close.as_ref().unwrap_or(&entry);
    if let Some(close) = &close {
        let b = close.management.as_ref().ok_or("managed_close_required")?;
        if b.original_entry_plan_id != entry.plan_id
            || lineage_position
                .as_ref()
                .or(position.as_ref())
                .is_none_or(|p| b.position_id != p.position_id)
        {
            return Err("provider_management_close_binding_invalid".into());
        }
        v.close_plan_id = Some(close.plan_id.clone());
        v.close_order = Some(close.order.clone());
    }
    if matches!(
        op,
        ProviderManagementOperation::ObserveSettlement { .. }
            | ProviderManagementOperation::SettleClose { .. }
    ) {
        let id = body["receiptSha256"]
            .as_str()
            .ok_or("provider_management_observation_result_missing")?;
        let projection = super::super::provisional::project_original_receipt(s, observed_plan, id)?;
        if body["planId"] != observed_plan.plan_id
            || body["cashReconciled"] != false
            || body["releasedHold"] != "0"
            || body["role"] != value(&projection.role)?
        {
            return Err("provider_management_observation_result_invalid".into());
        }
        v.observation = Some(projection);
    } else if matches!(op, ProviderManagementOperation::Inspect { .. })
        && !body["observation"].is_null()
    {
        let o: ObservationProjection = serde_json::from_value(body["observation"].clone())
            .map_err(|_| "provider_management_observation_result_invalid")?;
        super::super::provisional::verify_projection(s, &entry, &o)?;
        v.observation = Some(o);
    }
    if matches!(
        op,
        ProviderManagementOperation::SubmitClose { .. }
            | ProviderManagementOperation::ReconcileClose { .. }
    ) && body["planId"] != observed_plan.plan_id
    {
        return Err("provider_management_submission_result_invalid".into());
    }
    if matches!(
        op,
        ProviderManagementOperation::SubmitClose { .. }
            | ProviderManagementOperation::ReconcileClose { .. }
    ) {
        let phase: submission::Phase = serde_json::from_value(body["submission"]["phase"].clone())
            .map_err(|_| "provider_management_submission_result_invalid")?;
        let retained: Decimal = serde_json::from_value(body["submission"]["retainedCash"].clone())
            .map_err(|_| "provider_management_submission_result_invalid")?;
        let filled: Decimal = serde_json::from_value(body["submission"]["filledQuantity"].clone())
            .map_err(|_| "provider_management_submission_result_invalid")?;
        if retained.negative()
            || (retained != Decimal::ZERO && retained != observed_plan.purchase_cash)
            || filled.negative()
            || filled.compare(observed_plan.order.quantity)?.is_gt()
        {
            return Err("provider_management_submission_result_invalid".into());
        }
        v.phase = Some(phase);
        v.retained_cash = Some(retained);
        v.filled_quantity = Some(filled);
    }
    v.snapshot_sha256 = snapshot_hash(&v)?;
    let ctx = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("provider-management-snapshot:{command}"))?,
    );
    s.runtime().storage.put_json_if_absent(
        SNAPSHOTS,
        command,
        json!({"ownerSha256":owner(s)?,"snapshot":v}),
        &ctx,
    )?;
    let saved = stored(s, op, command)?.ok_or("provider_management_snapshot_missing")?;
    if saved != v {
        return Err("provider_management_snapshot_conflict".into());
    }
    Ok(v)
}
pub(crate) fn inspect_response(s: &TradeAssemblyService, body: &Value) -> ServiceResponse {
    let result = (|| {
        let op = ProviderManagementOperation::Inspect {
            scope: serde_json::from_value(body["scope"].clone())
                .map_err(|_| "provider_management_scope_invalid")?,
            position_id: body["positionId"]
                .as_str()
                .ok_or("provider_management_position_missing")?
                .into(),
        };
        binding(s, &op)?;
        let (_, _, position, _) = original(s, &op)?;
        let p = position.ok_or("provider_management_position_missing")?;
        Ok::<_, String>(
            json!({"position":p,"observation":super::super::provisional::project_latest(s,&read_owned(s,&p.entry_plan_id)?)?,"executionAuthorityGranted":false}),
        )
    })();
    match result {
        Ok(v) => ServiceResponse::ok(v),
        Err(e) => ServiceResponse::conflict(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_management_v7_snapshot_wire_preserves_unavailable_and_tagged_evidence() {
        let observation:wire::Observation=serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../plugin-sdk/tests/fixtures/settlement_observation_v2/response-filled-unavailable-cash-fees.json"))).unwrap();
        let evidence = EntryEvidence::observed(&"a".repeat(64));
        evidence.validate().unwrap();
        let mut bad = evidence.clone();
        let EntryEvidence::ObservedV2 {
            cash_reconciled, ..
        } = &mut bad;
        *cash_reconciled = true;
        assert!(bad.validate().is_err());
        let position = ProviderPositionProjection {
            position_id: "fixture.position".into(),
            entry_plan_id: format!("orderplan_{}", "b".repeat(64)),
            activation_id: "fixture.activation".into(),
            entry_evidence: evidence.clone(),
            exit_policy_sha256: "c".repeat(64),
            quantity: "1".parse().unwrap(),
            closed_quantity: Decimal::ZERO,
            basis: "100".parse().unwrap(),
            high_water: None,
            generation: 1,
            state_sha256: "d".repeat(64),
            reservations: BTreeMap::new(),
        };
        let mut snapshot = ProviderManagementSnapshot {
            schema_version: "tradeassembly.provider-management-snapshot.v7".into(),
            command_sha256: "e".repeat(64),
            operation: "strategy.position.inspect".into(),
            scope: data_plans::PlanScope {
                strategy_id: "strategy".into(),
                strategy_version_id: "version".into(),
                strategy_spec_hash: "spec".into(),
            },
            canonical_account_sha256: "f".repeat(64),
            entry_plan_id: position.entry_plan_id.clone(),
            position: Some(position),
            observation: Some(ObservationProjection {
                receipt_sha256: "a".repeat(64),
                plan_id: format!("orderplan_{}", "b".repeat(64)),
                role: wire::Role::Entry,
                original_intent_sha256: "1".repeat(64),
                source_binding_sha256: "2".repeat(64),
                provider_response_sha256: "3".repeat(64),
                observation: ReportedObservation::from_observation(&observation).unwrap(),
                accepted_projection: None,
                blockers: Vec::new(),
                blocked_reason: None,
                head_receipt_sha256: "a".repeat(64),
                head_blocked_reason: None,
                observed_at_ms: 1700000005000,
                cash_reconciled: false,
                released_hold: Decimal::ZERO,
            }),
            close_plan_id: None,
            close_order: None,
            phase: None,
            retained_cash: None,
            filled_quantity: None,
            submission: None,
            journal: vec![ProviderJournalEntry {
                schema_version: "tradeassembly.provider-management-journal.v7".into(),
                source_journal_sha256: "4".repeat(64),
                position_id: "fixture.position".into(),
                entry_plan_id: format!("orderplan_{}", "b".repeat(64)),
                entry_evidence: evidence,
                event: "entry_registered".into(),
                receipt_sha256: "a".repeat(64),
                quantity: "1".parse().unwrap(),
                closed_quantity: Decimal::ZERO,
                high_water: None,
                generation: 1,
                historical_outcome: true,
                execution_authority_granted: false,
            }],
            journal_paging_supported: false,
            journal_included: true,
            journal_completeness: "managed_position_namespace_only".into(),
            observed_at_ms: 1700000005000,
            historical_outcome: true,
            execution_authority_granted: false,
            snapshot_sha256: String::new(),
        };
        snapshot.snapshot_sha256 = snapshot_hash(&snapshot).unwrap();
        let bytes = serde_json::to_vec_pretty(&snapshot).unwrap();
        let decoded: ProviderManagementSnapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, snapshot);
        let value = value(&snapshot).unwrap();
        assert_eq!(value["position"]["entryEvidence"]["kind"], "observed_v2");
        assert!(value["position"].get("entrySettlementSha256").is_none());
        assert_eq!(value["observation"]["cashReconciled"], false);
        assert_eq!(
            value["observation"]["observation"]["feeCash"]["status"],
            "unavailable"
        );
        assert!(value["closeOrder"].is_null());
        let mut extra = value.clone();
        extra["cashReleased"] = json!(true);
        assert!(serde_json::from_value::<ProviderManagementSnapshot>(extra).is_err());
        if let Some(dir) = std::env::var_os("PROVIDER_MANAGEMENT_V7_WIRE_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("snapshot-inspect-authored-shape-only.json"), bytes).unwrap();
        }
    }
    #[test]
    fn provider_management_v7_available_account_projection_omits_raw_identity() {
        let mut raw: wire::Observation = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugin-sdk/tests/fixtures/settlement_observation_v2/response-partial-account.json"
        )))
        .unwrap();
        let Observed::Available { value: sample } = &mut raw.account else {
            panic!("account fixture must be available")
        };
        sample.snapshot.account_ref = "account://DO_NOT_TRANSMIT_ACCOUNT_ALIAS".into();
        sample.snapshot.provider_account_id = "DO_NOT_TRANSMIT_PROVIDER_ACCOUNT".into();
        for e in &mut raw.evidence {
            if e.kind == wire::EvidenceKind::Account {
                e.provider_record_id = Observed::Available {
                    value: "DO_NOT_TRANSMIT_ACCOUNT_RECORD".into(),
                };
            }
        }
        let projected = ReportedObservation::from_observation(&raw).unwrap();
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("DO_NOT_TRANSMIT"));
        assert!(!encoded.contains("accountRef"));
        assert!(!encoded.contains("providerAccountId"));
        assert!(!encoded.contains("providerRecordId"));
        assert_eq!(
            value(&projected).unwrap()["accountCoverage"]["status"],
            "available"
        );
    }
}
