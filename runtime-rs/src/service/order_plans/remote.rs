//! Finite paper financial operations for the versioned installed-node channel.
use super::*;
use crate::service::data_runner::DataRunnerMode;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FinancialOperation {
    Prepare {
        order_plan: PrepareCommand,
    },
    Reserve {
        scope: data_plans::PlanScope,
        plan_id: String,
    },
    Release {
        scope: data_plans::PlanScope,
        plan_id: String,
    },
    Decide {
        scope: data_plans::PlanScope,
        decision: decision::DecisionCommand,
    },
    Select {
        scope: data_plans::PlanScope,
        selection: decision::SelectionCommand,
    },
    Submit {
        scope: data_plans::PlanScope,
        submission: transport::SubmissionCommand,
    },
    Reconcile {
        scope: data_plans::PlanScope,
        plan_id: String,
    },
    Settle {
        scope: data_plans::PlanScope,
        plan_id: String,
    },
}
impl FinancialOperation {
    pub fn settlement(&self) -> bool {
        matches!(self, Self::Settle { .. })
    }
    pub fn snapshot_schema(&self) -> &'static str {
        if self.settlement() {
            "tradeassembly.financial_snapshot.v2"
        } else {
            "tradeassembly.financial_snapshot.v1"
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        let label = |v: &str| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control);
        let scope = self.scope();
        if ![
            scope.strategy_id.as_str(),
            scope.strategy_version_id.as_str(),
            scope.strategy_spec_hash.as_str(),
        ]
        .iter()
        .all(|s| label(s))
        {
            return Err("financial_runner_scope_invalid".into());
        }
        match self {
            Self::Submit { submission, .. } => submission.validate(),
            Self::Prepare { order_plan } if order_plan.mode != "paper" => {
                Err("financial_runner_paper_required".into())
            }
            Self::Select { selection, .. }
                if !label(&selection.activation_id) || !label(&selection.plan_revision_id) =>
            {
                Err("financial_runner_command_invalid".into())
            }
            Self::Decide { decision, .. }
                if !label(&decision.activation_id) || !label(&decision.plan_id) =>
            {
                Err("financial_runner_command_invalid".into())
            }
            _ if self.plan_id().is_some_and(|p| !label(p)) => {
                Err("financial_runner_command_invalid".into())
            }
            _ => Ok(()),
        }
    }

    pub fn body(&self) -> (&'static str, Value) {
        match self {
            Self::Prepare { order_plan } => (PREPARE, json!({"orderPlan":order_plan})),
            Self::Reserve { scope, plan_id } => (RESERVE, json!({"scope":scope,"planId":plan_id})),
            Self::Release { scope, plan_id } => (RELEASE, json!({"scope":scope,"planId":plan_id})),
            Self::Decide { scope, decision } => {
                (DECIDE, json!({"scope":scope,"decision":decision}))
            }
            Self::Select { scope, selection } => {
                (SELECT, json!({"scope":scope,"selection":selection}))
            }
            Self::Submit { scope, submission } => {
                (SUBMIT, json!({"scope":scope,"submission":submission}))
            }
            Self::Settle { scope, plan_id } => (SETTLE, json!({"scope":scope,"planId":plan_id})),
            Self::Reconcile { scope, plan_id } => {
                (RECONCILE, json!({"scope":scope,"planId":plan_id}))
            }
        }
    }
    pub fn action(&self) -> &'static str {
        match self {
            Self::Prepare { .. } => "strategy.order_plan.prepare",
            Self::Reserve { .. } => "strategy.order_plan.reserve",
            Self::Release { .. } => "strategy.order_plan.release",
            Self::Decide { .. } | Self::Select { .. } | Self::Submit { .. } => {
                "strategy.order_plan.submit"
            }
            Self::Reconcile { .. } => "strategy.order_plan.reconcile",
            Self::Settle { .. } => "strategy.order_plan.settle",
        }
    }
    pub(crate) fn scope(&self) -> &data_plans::PlanScope {
        match self {
            Self::Prepare { order_plan } => &order_plan.scope,
            Self::Reserve { scope, .. }
            | Self::Release { scope, .. }
            | Self::Decide { scope, .. }
            | Self::Select { scope, .. }
            | Self::Submit { scope, .. }
            | Self::Reconcile { scope, .. }
            | Self::Settle { scope, .. } => scope,
        }
    }
    fn plan_id(&self) -> Option<&str> {
        match self {
            Self::Reserve { plan_id, .. }
            | Self::Release { plan_id, .. }
            | Self::Reconcile { plan_id, .. }
            | Self::Settle { plan_id, .. } => Some(plan_id),
            Self::Decide { decision, .. } => Some(&decision.plan_id),
            Self::Submit { submission, .. } => Some(&submission.plan_id),
            _ => None,
        }
    }
}
/// Domain is immutable. Mutable cash, lease, controls, source credentials and
/// quote age are rechecked for actual dispatch, never hashed as reusable trust.
pub(crate) fn binding(
    s: &TradeAssemblyService,
    op: &FinancialOperation,
    historical: bool,
) -> Result<(data_plans::PlanScope, String, String), String> {
    s.require_object("strategy", &op.scope().strategy_id)
        .map_err(|_| "financial_runner_not_available")?;
    let (graph, canonical, proof) = match op {
        FinancialOperation::Prepare { order_plan: c } => {
            if c.mode != "paper" {
                return Err("financial_runner_paper_required".into());
            }
            let policy = policy(s, c)?;
            let canonical = if historical {
                original_command(s, c)?
                    .and_then(|p| p.canonical_account)
                    .ok_or("financial_runner_original_plan_missing")?
            } else {
                let f = facts(s, c, &policy)?;
                let _ = calculation(s, c, &policy, &f)?;
                let _ = cash(s, &f)?;
                f.canonical_account
                    .ok_or("financial_runner_account_missing")?
            };
            (
                c.capability_graph_revision.clone(),
                Some(canonical),
                json!({"command":c,"policy":policy}),
            )
        }
        FinancialOperation::Select { selection, .. } => {
            let p = if historical {
                data_plans::read_owned(s, &selection.plan_revision_id)?
            } else {
                data_plans::current(s, &selection.plan_revision_id)?
            };
            if p.content.scope != *op.scope() {
                return Err("financial_runner_scope_mismatch".into());
            }
            s.require_object("execution_activation", &selection.activation_id)
                .map_err(|_| "financial_runner_not_available")?;
            let r = s
                .runtime()
                .storage
                .get_json(
                    "execution_runs",
                    &format!("run_{}", selection.activation_id),
                )?
                .ok_or("financial_runner_run_missing")?;
            let c = s
                .runtime()
                .storage
                .get_json(
                    "execution_configs",
                    r["configId"]
                        .as_str()
                        .ok_or("financial_runner_config_missing")?,
                )?
                .ok_or("financial_runner_config_missing")?;
            if !historical {
                crate::service::execution::typed::current(s, &c)?;
            }
            if c["strategySpecHash"] != op.scope().strategy_spec_hash
                || c["capabilityGraphRevisionId"]
                    != p.content.plan.request.capability_graph_revision
            {
                return Err("financial_runner_scope_mismatch".into());
            }
            let account = serde_json::from_value(c["canonicalAccount"].clone())
                .map_err(|_| "financial_runner_account_missing")?;
            (
                p.content.plan.request.capability_graph_revision.clone(),
                Some(account),
                json!({"config":c,"selectedPlanSha256":p.revision_sha256}),
            )
        }
        _ => {
            let p = read_owned(s, op.plan_id().ok_or("financial_runner_plan_required")?)?;
            if p.command.scope != *op.scope() {
                return Err("financial_runner_scope_mismatch".into());
            }
            if !historical {
                match op {
                    FinancialOperation::Reserve { .. } => {
                        let (p, f) = checked_current(s, &p.plan_id)?;
                        if released(s, &p)?.is_some() {
                            return Err("order_plan_released".into());
                        }
                        submission::reserve_guard(s, &p)?;
                        let (_, _, available) = cash(s, &f)?;
                        risk(
                            &p.policy,
                            &f,
                            &p.order,
                            if held(s, &p)? {
                                available.checked_add(p.purchase_cash)?
                            } else {
                                available
                            },
                        )?;
                    }
                    FinancialOperation::Release { .. } => {
                        submission::release_guard(s, &p)?;
                    }
                    FinancialOperation::Decide { decision, .. } => {
                        let source = submission::source(s, &p.plan_id)?;
                        let signal = source.verified_signal()?;
                        source.snapshot_with_signal(&signal)?;
                        let (r, c) = decision::run(s, &decision.activation_id)?;
                        decision::check_plan_with_signal(s, &p, &r, &c, &signal)?;
                    }
                    FinancialOperation::Submit { submission: c, .. } => {
                        let (source, _, _) = transport::request(s, c)?;
                        let signal = source.verified_signal()?;
                        source.snapshot_with_signal(&signal)?;
                        let (r, cfg) = decision::run(s, &c.activation_id)?;
                        decision::check_plan_with_signal(s, &p, &r, &cfg, &signal)?;
                    }
                    FinancialOperation::Settle { .. } => {
                        settlement::preflight(s, &p)?;
                    }
                    FinancialOperation::Reconcile { .. }
                        if submission::source(s, &p.plan_id)?.state()?.is_none() =>
                    {
                        return Err("broker_v2_state_missing".into());
                    }
                    _ => {}
                }
            }
            let mut immutable_proof = json!({"planSha256":hash(&p)?,"entryId":p.entry_id,"signalReceiptSha256":p.command.signal_receipt_sha256});
            if op.settlement() {
                let (_, original) = submission::record(s, &p)?.ok_or("broker_v2_state_missing")?;
                immutable_proof["originalIntentSha256"] = json!(original.intent_sha256);
                immutable_proof["originalBrokerBindingSha256"] =
                    json!(hash(&original.intent["preparedBinding"])?);
                immutable_proof["originalCommandKeySha256"] = json!(hash(&original.original_key)?);
            }
            (
                p.command.capability_graph_revision.clone(),
                p.canonical_account.clone(),
                immutable_proof,
            )
        }
    };
    let canonical = canonical.ok_or("financial_runner_account_missing")?;
    Ok((
        op.scope().clone(),
        graph.clone(),
        hash(
            &json!({"schemaVersion":"tradeassembly.financial_domain.v1","operation":op,"owner":owner(s)?,"scope":op.scope(),"graph":graph,"canonicalAccount":canonical,"immutableProof":proof}),
        )?,
    ))
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinancialSnapshot {
    pub schema_version: String,
    pub command_sha256: String,
    pub operation: String,
    pub scope: data_plans::PlanScope,
    pub plan_id: Option<String>,
    pub canonical_account_sha256: String,
    pub order: Option<BrokerOrderV2>,
    pub cash_amount: Option<Decimal>,
    pub cash_reserved: bool,
    pub valid_until_ms: Option<i64>,
    pub phase: Option<submission::Phase>,
    pub observation: Option<tradeassembly_plugin_sdk::broker_v2::OrderObservation>,
    pub submission: Option<transport::SubmissionCommand>,
    pub selection_binding_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement: Option<settlement::Projection>,
    pub observed_at_ms: i64,
    pub execution_authority_granted: bool,
    pub snapshot_sha256: String,
}
const SNAPSHOTS: &str = "financial_runner_snapshots_v1";
fn snapshot_hash(v: &FinancialSnapshot) -> Result<String, String> {
    let mut v = v.clone();
    v.snapshot_sha256.clear();
    hash(&v)
}
pub(crate) fn validate_snapshot(
    s: &TradeAssemblyService,
    op: &FinancialOperation,
    command: &str,
    v: &FinancialSnapshot,
) -> Result<(), String> {
    s.require_object("strategy", &op.scope().strategy_id)
        .map_err(|_| "financial_runner_not_available")?;
    if v.schema_version != op.snapshot_schema()
        || v.settlement.is_some() != op.settlement()
        || v.command_sha256 != command
        || v.scope != *op.scope()
        || v.execution_authority_granted
        || snapshot_hash(v)? != v.snapshot_sha256
    {
        return Err("financial_runner_snapshot_invalid".into());
    }
    let stored = s
        .runtime()
        .storage
        .get_json(SNAPSHOTS, command)?
        .ok_or("financial_runner_snapshot_missing")?;
    if stored["ownerSha256"] != owner(s)? || stored["snapshot"] != value(v)? {
        return Err("financial_runner_snapshot_invalid".into());
    }
    Ok(())
}
pub(crate) fn project(
    s: &TradeAssemblyService,
    op: &FinancialOperation,
    command: &str,
    body: Value,
) -> Result<FinancialSnapshot, String> {
    if let Some(old) = s.runtime().storage.get_json(SNAPSHOTS, command)? {
        let v: FinancialSnapshot = serde_json::from_value(old["snapshot"].clone())
            .map_err(|_| "financial_runner_snapshot_invalid")?;
        validate_snapshot(s, op, command, &v)?;
        return Ok(v);
    }
    let mut v = FinancialSnapshot {
        schema_version: op.snapshot_schema().into(),
        command_sha256: command.into(),
        operation: op.action().into(),
        scope: op.scope().clone(),
        plan_id: None,
        canonical_account_sha256: String::new(),
        order: None,
        cash_amount: None,
        cash_reserved: false,
        valid_until_ms: None,
        phase: None,
        observation: None,
        submission: None,
        selection_binding_sha256: None,
        settlement: None,
        observed_at_ms: s.runtime().clock.trusted_now_ms()?,
        execution_authority_granted: false,
        snapshot_sha256: String::new(),
    };
    if let FinancialOperation::Select { selection, .. } = op {
        let (_, _, _) = binding(s, op, true)?;
        let run = s
            .runtime()
            .storage
            .get_json(
                "execution_runs",
                &format!("run_{}", selection.activation_id),
            )?
            .ok_or("financial_runner_run_missing")?;
        let cfg = s
            .runtime()
            .storage
            .get_json(
                "execution_configs",
                run["configId"]
                    .as_str()
                    .ok_or("financial_runner_config_missing")?,
            )?
            .ok_or("financial_runner_config_missing")?;
        if body["selection"]["bindingSha256"] != run["typedSelectionHead"]["bindingSha256"] {
            return Err("financial_runner_result_mismatch".into());
        }
        v.canonical_account_sha256 = cfg["canonicalAccount"]["identitySha256"]
            .as_str()
            .ok_or("financial_runner_account_missing")?
            .into();
        v.selection_binding_sha256 = body["selection"]["bindingSha256"]
            .as_str()
            .map(str::to_owned);
    } else {
        let id = body["planId"]
            .as_str()
            .ok_or("financial_runner_plan_required")?;
        let p = read_owned(s, id)?;
        if p.command.scope != *op.scope() || op.plan_id().is_some_and(|wanted| wanted != id) {
            return Err("financial_runner_result_mismatch".into());
        }
        v.plan_id = Some(id.into());
        v.canonical_account_sha256 = p
            .canonical_account
            .as_ref()
            .ok_or("financial_runner_account_missing")?
            .identity_sha256
            .clone();
        v.order = Some(p.order.clone());
        v.valid_until_ms = Some(p.valid_until_ms);
        v.cash_reserved = false;
        match op {
            FinancialOperation::Prepare { order_plan } => {
                if p.command != *order_plan {
                    return Err("financial_runner_result_mismatch".into());
                }
                v.cash_amount = Some(p.purchase_cash);
                v.cash_reserved = body["cashReservedAtPreparation"] == true;
            }
            FinancialOperation::Reserve { .. } => {
                if body["cashReserved"] != p.purchase_cash.to_string() {
                    return Err("financial_runner_result_mismatch".into());
                }
                v.cash_amount = Some(p.purchase_cash);
                v.cash_reserved = true;
            }
            FinancialOperation::Release { .. } => {
                v.cash_amount = Some(
                    released(s, &p)?
                        .ok_or("financial_runner_release_missing")?
                        .released_cash,
                );
            }
            FinancialOperation::Decide { decision, .. } => {
                let cmd: transport::SubmissionCommand =
                    serde_json::from_value(body["submission"].clone())
                        .map_err(|_| "financial_runner_decision_invalid")?;
                if cmd.plan_id != id || cmd.activation_id != decision.activation_id {
                    return Err("financial_runner_result_mismatch".into());
                }
                v.cash_reserved = true;
                v.cash_amount = Some(p.purchase_cash);
                v.submission = Some(cmd);
                v.selection_binding_sha256 =
                    body["selectionBindingSha256"].as_str().map(str::to_owned);
            }
            FinancialOperation::Settle { .. } => {
                let projection = settlement::projection(
                    s,
                    &p,
                    body["receiptSha256"]
                        .as_str()
                        .ok_or("financial_runner_settlement_missing")?,
                )?;
                let (_, record) = submission::record(s, &p)?.ok_or("broker_v2_state_missing")?;
                v.valid_until_ms = None;
                v.cash_amount = Some(record.retained_cash);
                v.cash_reserved = record.retained_cash.positive();
                v.phase = Some(record.phase);
                v.observation = record.observation;
                v.settlement = Some(projection);
            }
            FinancialOperation::Submit { .. } | FinancialOperation::Reconcile { .. } => {
                let phase: submission::Phase =
                    serde_json::from_value(body["submission"]["phase"].clone())
                        .map_err(|_| "financial_runner_submission_missing")?;
                let amount: Decimal =
                    serde_json::from_value(body["submission"]["retainedCash"].clone())
                        .map_err(|_| "financial_runner_submission_missing")?;
                if amount != Decimal::ZERO && amount != p.purchase_cash {
                    return Err("financial_runner_result_mismatch".into());
                }
                v.phase = Some(phase);
                v.cash_amount = Some(amount);
                v.cash_reserved = amount.positive();
                let payload = if matches!(op, FinancialOperation::Submit { .. }) {
                    &body["receipt"]["payload"]
                } else {
                    &body["recovery"]["receipt"]["payload"]
                };
                if !payload.is_null() {
                    let o: tradeassembly_plugin_sdk::broker_v2::OrderObservation =
                        serde_json::from_value(payload.clone())
                            .map_err(|_| "financial_runner_observation_invalid")?;
                    o.validate(v.observed_at_ms)?;
                    if o.order != p.order
                        || o.canonical_account_sha256 != v.canonical_account_sha256
                    {
                        return Err("financial_runner_result_mismatch".into());
                    }
                    v.observation = Some(o);
                }
            }
            _ => {}
        }
    }
    v.snapshot_sha256 = snapshot_hash(&v)?;
    let ctx = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("financial-snapshot:{command}"))?,
    );
    s.runtime().storage.put_json_if_absent(
        SNAPSHOTS,
        command,
        json!({"ownerSha256":owner(s)?,"snapshot":v}),
        &ctx,
    )?;
    validate_snapshot(s, op, command, &v)?;
    Ok(v)
}
pub(crate) fn require_paper(mode: &DataRunnerMode) -> Result<(), String> {
    if *mode != DataRunnerMode::Paper {
        Err("financial_runner_paper_required".into())
    } else {
        Ok(())
    }
}
