//! Host-owned entry approvals; selection refresh never deletes managed state.
use super::*;
use crate::service::{data_plans, execution::typed};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionCommand {
    pub plan_id: String,
    pub activation_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionCommand {
    pub activation_id: String,
    pub plan_revision_id: String,
}
pub(in crate::service) fn run(
    s: &TradeAssemblyService,
    id: &str,
) -> Result<(Value, Value), String> {
    s.require_object("execution_activation", id)
        .map_err(|_| "typed_entry_activation_not_available")?;
    let r = s
        .runtime()
        .storage
        .get_json("execution_runs", &format!("run_{id}"))?
        .ok_or("typed_entry_run_missing")?;
    let a = s
        .runtime()
        .storage
        .get_json("execution_activations", id)?
        .ok_or("typed_entry_activation_missing")?;
    let c = s
        .runtime()
        .storage
        .get_json(
            "execution_configs",
            r["configId"].as_str().ok_or("typed_entry_config_missing")?,
        )?
        .ok_or("typed_entry_config_missing")?;
    measured("decision.typed.current", || typed::current(s, &c))?;
    if r["state"] != "active"
        || a["state"] != "active"
        || !typed::is_typed(&r)
        || !typed::is_typed(&a)
        || r["configId"] != a["configId"]
        || r["strategySpecHash"] != c["strategySpecHash"]
        || r["capabilityGraphRevisionId"] != c["capabilityGraphRevisionId"]
    {
        return Err("typed_entry_activation_stale".into());
    }
    for ns in ["execution_controls", "execution_reconciliation"] {
        let control = s
            .runtime()
            .storage
            .get_json(ns, id)?
            .ok_or("typed_entry_controls_missing")?;
        if control["activationId"] != id
            || crate::broker_submission::controls_blocked(&control)
            || (ns == "execution_reconciliation"
                && (control["state"] != "clear" || control["newEntriesPaused"] != false))
        {
            return Err("typed_entry_controls_blocked".into());
        }
    }
    Ok((r, c))
}
fn plan_scope(c: &Value) -> Result<data_plans::PlanScope, String> {
    Ok(data_plans::PlanScope {
        strategy_id: c["strategyId"]
            .as_str()
            .ok_or("typed_entry_scope_invalid")?
            .into(),
        strategy_version_id: c["strategyVersionId"]
            .as_str()
            .ok_or("typed_entry_scope_invalid")?
            .into(),
        strategy_spec_hash: c["strategySpecHash"]
            .as_str()
            .ok_or("typed_entry_scope_invalid")?
            .into(),
    })
}
fn selection_binding(p: &data_plans::StoredDataPlan) -> Result<String, String> {
    let sources = p
        .content
        .selection_source
        .as_ref()
        .map(|s| s.selection_sha256.clone())
        .unwrap_or_default();
    hash(&(
        "tradeassembly.typed_selection_binding.v1",
        &p.content.scope,
        &p.content.plan.request.capability_graph_revision,
        sources,
    ))
}
fn head(s: &TradeAssemblyService, p: &data_plans::StoredDataPlan) -> Result<Value, String> {
    let mut sources = BTreeMap::new();
    if let Some(source) = &p.content.selection_source {
        for (name, id) in &source.command.selection_receipts {
            let r = crate::service::instrument_screens::current(s, id)?;
            if source.selection_sha256.get(name) != Some(&r.receipt_sha256) {
                return Err("typed_selection_lineage_invalid".into());
            }
            sources.insert(
                name.clone(),
                json!({"sha256":r.receipt_sha256,"knownAtMs":r.instrument_set.known_at_ms}),
            );
        }
    }
    Ok(
        json!({"scope":p.content.scope,"graph":p.content.plan.request.capability_graph_revision,"sources":sources,"bindingSha256":selection_binding(p)?}),
    )
}
#[cfg(test)]
pub(crate) fn check_plan(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    r: &Value,
    c: &Value,
) -> Result<String, String> {
    let signal = verify_signal(s, &p.command)?;
    check_plan_with_signal(s, p, r, c, &signal)
}
pub(crate) fn check_plan_with_signal(
    s: &TradeAssemblyService,
    p: &OrderPlan,
    r: &Value,
    c: &Value,
    verified: &VerifiedSignal,
) -> Result<String, String> {
    verified.matches(s, &p.command)?;
    if p.command.scope != plan_scope(c)?
        || p.command.capability_graph_revision != c["capabilityGraphRevisionId"]
        || Some(&c["canonicalAccount"])
            != p.canonical_account
                .as_ref()
                .map(|a| serde_json::to_value(a).unwrap())
                .as_ref()
        || hash(&p.policy)? != c["orderPlanPolicySha256"]
    {
        return Err("typed_entry_policy_or_account_mismatch".into());
    }
    let signal = &verified.receipt;
    let binding = selection_binding(&verified.plan)?;
    if c["selectionPolicy"] == "selected_receipt_head"
        && r["typedSelectionHead"]["bindingSha256"] != binding
    {
        return Err("typed_entry_selection_not_current".into());
    }
    let frame = signal.signals.last().ok_or("typed_entry_signal_missing")?;
    if frame.trade_instruments.len() != 1
        || frame.trade_instruments[0].instrument_id != p.order.instrument.instrument_id
    {
        return Err("typed_entry_subject_mismatch".into());
    }
    Ok(binding)
}

pub(crate) fn select(
    s: &TradeAssemblyService,
    c: SelectionCommand,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let (mut r, config) = run(s, &c.activation_id)?;
    let old = r.clone();
    let p = data_plans::current(s, &c.plan_revision_id)?;
    if p.content.scope != plan_scope(&config)?
        || p.content.plan.request.capability_graph_revision != config["capabilityGraphRevisionId"]
        || p.content.selection_source.is_none()
        || config["selectionPolicy"] != "selected_receipt_head"
    {
        return Err("typed_selection_binding_mismatch".into());
    }
    let h = head(s, &p)?;
    if !r["typedSelectionHead"].is_null() {
        let previous = r["typedSelectionHead"]["sources"]
            .as_object()
            .ok_or("typed_selection_state_invalid")?;
        let fresh = h["sources"]
            .as_object()
            .ok_or("typed_selection_state_invalid")?;
        if previous.keys().collect::<Vec<_>>() != fresh.keys().collect::<Vec<_>>() {
            return Err("typed_selection_source_changed".into());
        }
        for (k, v) in fresh {
            let old = &previous[k];
            if v["sha256"] != old["sha256"]
                && v["knownAtMs"]
                    .as_i64()
                    .ok_or("typed_selection_time_invalid")?
                    <= old["knownAtMs"]
                        .as_i64()
                        .ok_or("typed_selection_time_invalid")?
            {
                return Err("typed_selection_revision_not_newer".into());
            }
        }
    }
    r["typedSelectionHead"] = h.clone();
    if s.runtime().storage.put_json_batch(
        &[StorageWrite::new(
            "execution_runs",
            format!("run_{}", c.activation_id),
            r,
            ctx.clone(),
        )],
        &[StorageExpectation::new(
            "execution_runs",
            format!("run_{}", c.activation_id),
            Some(old),
        )],
    )? != ComparePutOutcome::Updated
    {
        return Err("typed_selection_update_conflict".into());
    }
    Ok(
        json!({"activationId":c.activation_id,"selection":h,"managedEntriesPreserved":true,"executionAuthorityGranted":false}),
    )
}
pub(crate) fn decide(
    s: &TradeAssemblyService,
    c: DecisionCommand,
    ctx: &SideEffectContext,
) -> Result<Value, String> {
    let source = submission::source(s, &c.plan_id)?;
    let signal = source.verified_signal()?;
    let p = source.snapshot_with_signal(&signal)?;
    if source.state()?.is_some() {
        return Err("order_plan_already_submitted".into());
    }
    let (mut r, config) = run(s, &c.activation_id)?;
    let old = r.clone();
    let selection = check_plan_with_signal(s, &p, &r, &config, &signal)?;
    let attempt = format!(
        "attempt_typed_{}",
        hash(&(c.activation_id.clone(), p.entry_id.clone()))?
    );
    let resource = format!(
        "typed-entry:{}",
        hash(&(c.activation_id.clone(), p.entry_id.clone()))?
    );
    let worker = format!("typed-worker:{}", hash(&(owner(s)?, &attempt))?);
    let now = s.runtime().clock.trusted_now_ms()?;
    let (lease, owns_lease) = if let Some(agent) = s.agent_mcp_execution_context() {
        let deployment = s
            .runtime()
            .storage
            .get_json(crate::agent_runner::DEPLOYMENTS_NS, agent.deployment_id())?
            .ok_or("typed_entry_deployment_missing")?;
        if deployment["deployment"]["executionConfigVersionId"] != config["configId"]
            || deployment["deployment"]["mode"] != "paper"
        {
            return Err("typed_entry_agent_config_mismatch".into());
        }
        (
            agent.current_lease(
                s.runtime().storage.as_ref(),
                s.runtime().clock.as_ref(),
                s.runtime().leases.as_ref(),
            )?,
            false,
        )
    } else {
        (
            s.runtime()
                .leases
                .acquire(&resource, &worker, now, 60000)?
                .ok_or("typed_entry_lease_busy")?,
            true,
        )
    };
    let resource = lease.resource.clone();
    let worker = lease.owner.clone();
    let result = (|| {
        let old_attempt = s
            .runtime()
            .storage
            .get_json("execution_attempts", &attempt)?;
        let mut approval = json!({"planId":p.plan_id,"entryId":p.entry_id,"signalReceiptSha256":p.command.signal_receipt_sha256,"canonicalAccountSha256":p.canonical_account.as_ref().ok_or("typed_entry_account_required")?.identity_sha256,"leaseResource":lease.resource,"leaseOwner":lease.owner,"leaseFence":lease.fencing_token,"selectionBindingSha256":selection,"attemptId":attempt});
        if !r["orderPlanApprovals"][&p.plan_id]["executionWindow"].is_null() {
            approval["executionWindow"] =
                r["orderPlanApprovals"][&p.plan_id]["executionWindow"].clone();
        }
        if r["orderPlanApprovals"]
            .as_object()
            .is_none_or(|a| a.len() >= 256 && !a.contains_key(&p.plan_id))
        {
            return Err("typed_entry_active_plan_limit".into());
        }
        r["orderPlanApprovals"][&p.plan_id] = approval;
        r["managedEntries"][&p.entry_id] = json!({"planId":p.plan_id,"instrument":p.order.instrument,"state":"reserved","selectionBindingSha256":selection});
        let attempt_record = json!({"attemptId":attempt,"activationId":c.activation_id,"state":"running","owner":worker,"leaseResource":resource,"fencingToken":lease.fencing_token,"executionContractVersion":typed::VERSION});
        source.snapshot()?;
        let (fresh_run, fresh_config) = run(s, &c.activation_id)?;
        if fresh_run != old || fresh_config != config {
            return Err("typed_entry_state_changed".into());
        }
        let state_key = format!("run_{}", c.activation_id);
        if s.runtime().storage.put_json_batch(
            &[
                StorageWrite::new("execution_runs", &state_key, r, ctx.clone()),
                StorageWrite::new("execution_attempts", &attempt, attempt_record, ctx.clone()),
            ],
            &[
                StorageExpectation::new("execution_runs", &state_key, Some(old)),
                StorageExpectation::new("execution_attempts", &attempt, old_attempt),
                StorageExpectation::new(submission::STATE, &p.plan_id, None),
                StorageExpectation::new(RELEASES, &p.plan_id, None),
            ],
        )? != ComparePutOutcome::Updated
        {
            return Err("typed_entry_decision_conflict".into());
        }
        Ok(
            json!({"planId":p.plan_id,"submission":{"planId":p.plan_id,"activationId":c.activation_id,"attemptId":attempt,"brokerRequirementRef":config["orderPlanBrokerNode"]},"leaseExpiresAtMs":lease.expires_at_ms,"selectionBindingSha256":selection,"executionAuthorityGranted":false,"c5StillRequired":true}),
        )
    })();
    if result.is_err() && owns_lease {
        let _ = s.runtime().leases.release(&lease);
    }
    result
}
