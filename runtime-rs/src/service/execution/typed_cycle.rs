//! Local, opted-in scheduled financial phases. Every child uses normal C3/C5.
//! This is not exposed through the computation-only remote scheduler contract.
use super::*;
use crate::service::{data_evaluation, data_plans, order_plans, session_schedules};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
const NS: &str = "typed_execution_cycles_v1";
pub(crate) const INTERNAL: &str = "internal:typed-financial-cycle";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InputValue {
    Literal { value: Value },
    TradeInstrumentId {},
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomaticConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_health_refresh: Option<super::automatic_health::Config>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management: Option<crate::service::order_plans::management::automation::Config>,
    pub source_inputs: BTreeMap<String, BTreeMap<String, InputValue>>,
    pub max_subjects_per_occurrence: usize,
}
impl AutomaticConfig {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(health) = &self.account_health_refresh {
            health.validate()?;
        }
        if let Some(management) = &self.management {
            management.validate()?;
        }
        if self.max_subjects_per_occurrence == 0
            || self.max_subjects_per_occurrence > 32
            || self
                .source_inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != vec!["account_data", "instrument_metadata", "pricing_quote"]
            || serde_json::to_vec(self)
                .map_err(|_| "typed_cycle_config_invalid")?
                .len()
                > 16384
        {
            return Err("typed_cycle_config_invalid".into());
        }
        for input in self.source_inputs.values() {
            if input.len() > 32 {
                return Err("typed_cycle_config_invalid".into());
            }
            for (name, value) in input {
                if name.is_empty()
                    || name.len() > 128
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || matches!(
                        name.as_str(),
                        "config" | "credentials" | "authority" | "accountRef" | "account_ref"
                    )
                {
                    return Err("typed_cycle_input_binding_invalid".into());
                }
                if let InputValue::Literal { value } = value {
                    crate::service::data_captures::capturable_payload(value)?;
                }
            }
        }
        Ok(())
    }
    fn input(&self, role: &str, instrument: &str) -> Result<Value, String> {
        let fields = self
            .source_inputs
            .get(role)
            .ok_or("typed_cycle_input_binding_missing")?;
        Ok(Value::Object(
            fields
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        match v {
                            InputValue::Literal { value } => value.clone(),
                            InputValue::TradeInstrumentId {} => json!(instrument),
                        },
                    )
                })
                .collect(),
        ))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Command {
    pub scope: data_plans::PlanScope,
    pub activation_id: String,
    pub occurrence_id: String,
    pub plan_revision_id: String,
    pub signal_receipt_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    schema_version: String,
    owner_sha256: String,
    command: Command,
    config_sha256: String,
    phase: u8,
    status: String,
    collections: BTreeMap<String, String>,
    plan_id: Option<String>,
    submission: Option<Value>,
    error: Option<String>,
    state_sha256: String,
}
fn digest<T: Serialize>(v: &T) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(v).map_err(|_| "typed_cycle_encoding_invalid".into())
}
fn seal(s: &mut State) -> Result<(), String> {
    s.state_sha256.clear();
    s.state_sha256 = digest(s)?;
    Ok(())
}
fn owner(s: &TradeAssemblyService) -> Result<String, String> {
    digest(&s.invocation_owner().ok_or("typed_cycle_owner_required")?)
}
fn load(s: &TradeAssemblyService, id: &str) -> Result<Option<(Value, State)>, String> {
    let Some(raw) = s.runtime().storage.get_json(NS, id)? else {
        return Ok(None);
    };
    let state: State =
        serde_json::from_value(raw.clone()).map_err(|_| "typed_cycle_state_invalid")?;
    let mut check = state.clone();
    seal(&mut check)?;
    if state.schema_version != "tradeassembly.typed_execution_cycle.v1"
        || state.owner_sha256 != owner(s)?
        || state.phase > 7
        || check.state_sha256 != state.state_sha256
    {
        return Err("typed_cycle_state_invalid".into());
    }
    Ok(Some((raw, state)))
}
fn summary(id: &str, state: &State, next: Option<i64>) -> Value {
    json!({"cycleId":id,"status":state.status,"phase":state.phase,"planId":state.plan_id,"error":state.error,"nextAttemptAtMs":next,"executionAuthorityGranted":false})
}
fn persist(
    s: &TradeAssemblyService,
    id: &str,
    old: Value,
    state: &mut State,
) -> Result<(), String> {
    seal(state)?;
    let ctx = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("typed-cycle:{id}:{}", state.phase))?,
    );
    if s.runtime().storage.put_json_batch(
        &[StorageWrite::new(
            NS,
            id,
            serde_json::to_value(state).map_err(|_| "typed_cycle_encoding_invalid")?,
            ctx,
        )],
        &[StorageExpectation::new(NS, id, Some(old))],
    )? != ComparePutOutcome::Updated
    {
        return Err("typed_cycle_conflict".into());
    }
    Ok(())
}
pub(crate) fn automatic(config: &Value) -> Result<AutomaticConfig, String> {
    let c: typed::ConfigCommand = serde_json::from_value(config["typedExecution"].clone())
        .map_err(|_| "typed_cycle_config_invalid")?;
    let a = c.automatic_execution.ok_or("typed_cycle_not_enabled")?;
    a.validate()?;
    Ok(a)
}
/// Exactly one retained financial phase per call; request/child keys are stable
/// across service/process recreation. A child error never becomes a false signal.
pub(crate) fn advance(s: &TradeAssemblyService, c: Command) -> Result<Value, String> {
    s.require_object("execution_activation", &c.activation_id)
        .map_err(|_| "typed_cycle_not_available")?;
    let id = digest(&c)?;
    if let Some((_, old)) = load(s, &id)? {
        if old.command != c {
            return Err("typed_cycle_identity_conflict".into());
        }
        if matches!(
            old.status.as_str(),
            "complete" | "skipped" | "blocked" | "uncertain"
        ) {
            return Ok(summary(&id, &old, None));
        }
    }
    let health = super::automatic_health::advance_activation(s, &c.activation_id, &c.scope)?;
    if !health.ready {
        // No financial phase has advanced. The scheduler retains this same child
        // until this future deadline rather than recording a synthetic result.
        return Ok(json!({"cycleId":id,"status":"running","phase":0,
            "nextAttemptAtMs":health.next_attempt_at_ms.max(s.runtime().clock.now_ms().saturating_add(1)),
            "accountHealthRefresh":health.projection,"executionAuthorityGranted":false}));
    }
    // Health IO may have outlived a background stop/pause. This private
    // context restricts only the worker; finite manual phases remain valid.
    s.require_background_worker_current()?;
    let (_run, config) = order_plans::decision::run(s, &c.activation_id)?;
    let automatic = automatic(&config)?;
    let (plan, receipt) = data_evaluation::indexed_receipt(s, &c.signal_receipt_sha256)?;
    if plan.revision_id != c.plan_revision_id
        || plan.content.scope != c.scope
        || config["strategyId"] != c.scope.strategy_id
        || config["strategyVersionId"] != c.scope.strategy_version_id
        || config["strategySpecHash"] != c.scope.strategy_spec_hash
        || config["capabilityGraphRevisionId"]
            != plan.content.plan.request.capability_graph_revision
    {
        return Err("typed_cycle_binding_mismatch".into());
    }
    if let Some(management) = &automatic.management {
        order_plans::management::automation::validate_entry_reversal_sources(
            s, &plan, &receipt, management,
        )?;
    }
    let occurrence = session_schedules::current(s, &c.occurrence_id)?;
    if occurrence.command.scope != c.scope
        || occurrence.command.mode != "paper"
        || occurrence.command.capability_graph_revision
            != plan.content.plan.request.capability_graph_revision
    {
        return Err("typed_cycle_occurrence_mismatch".into());
    }
    let now = s.runtime().clock.trusted_now_ms()?;
    let open = occurrence
        .occurrence
        .execution_open_at_ms
        .ok_or("typed_cycle_execution_window_missing")?;
    let close = occurrence
        .occurrence
        .execution_close_at_ms
        .ok_or("typed_cycle_execution_window_missing")?;
    let frame = receipt.signals.last().ok_or("typed_cycle_signal_missing")?;
    if frame.trade_instruments.len() != 1 {
        return Err("typed_cycle_single_subject_required".into());
    }
    let instrument = &frame.trade_instruments[0].instrument_id;
    let source_config = digest(&config)?;
    let ctx = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("typed-cycle-create:{id}"))?,
    );
    if load(s, &id)?.is_none() {
        let mut state = State {
            schema_version: "tradeassembly.typed_execution_cycle.v1".into(),
            owner_sha256: owner(s)?,
            command: c.clone(),
            config_sha256: source_config.clone(),
            phase: 0,
            status: "running".into(),
            collections: BTreeMap::new(),
            plan_id: None,
            submission: None,
            error: None,
            state_sha256: String::new(),
        };
        seal(&mut state)?;
        s.runtime().storage.put_json_if_absent(
            NS,
            &id,
            serde_json::to_value(state).map_err(|_| "typed_cycle_encoding_invalid")?,
            &ctx,
        )?;
    }
    let (old, mut state) = load(s, &id)?.ok_or("typed_cycle_state_missing")?;
    if state.config_sha256 != source_config {
        return Err("typed_cycle_config_changed".into());
    }
    if now < open {
        return Ok(summary(&id, &state, Some(open)));
    }
    if now >= close {
        state.status = "blocked".into();
        state.error = Some("typed_cycle_execution_window_closed".into());
        persist(s, &id, old, &mut state)?;
        return Ok(summary(&id, &state, None));
    }
    match &frame.entry {
        crate::data_strategy::SignalValue::Ready { value: false } => {
            state.status = "skipped".into();
            persist(s, &id, old, &mut state)?;
            return Ok(summary(&id, &state, None));
        }
        crate::data_strategy::SignalValue::Ready { value: true } => {}
        _ => return Err("typed_cycle_signal_unavailable".into()),
    }
    let policy = order_plans::published_policy(s, &c.scope)?;
    let phase = state.phase;
    let (path, mut body) = match phase {
        0 if plan.content.selection_source.is_some() => (
            order_plans::SELECT,
            json!({"scope":c.scope,"selection":{"activationId":c.activation_id,"planRevisionId":c.plan_revision_id}}),
        ),
        0 => {
            state.phase = 1;
            persist(s, &id, old, &mut state)?;
            return Ok(summary(&id, &state, None));
        }
        1..=3 => {
            let (role, schema) = match phase {
                1 => (
                    "account_data",
                    tradeassembly_plugin_sdk::execution_snapshot::ACCOUNT_SCHEMA,
                ),
                2 => (
                    "instrument_metadata",
                    tradeassembly_plugin_sdk::execution_snapshot::METADATA_SCHEMA,
                ),
                _ => (
                    "pricing_quote",
                    tradeassembly_plugin_sdk::execution_snapshot::QUOTE_SCHEMA,
                ),
            };
            let node = policy
                .source_nodes
                .get(role)
                .ok_or("typed_cycle_source_missing")?;
            (
                "/product/strategies/data-collections/create",
                json!({"collection":{"scope":c.scope,"capabilityGraphRevision":plan.content.plan.request.capability_graph_revision,"acquisitionNode":node,"mode":"paper","responseSchemaRef":schema,"input":automatic.input(role,instrument)?}}),
            )
        }
        4 => (
            order_plans::PREPARE,
            json!({"orderPlan":{"scope":c.scope,"capabilityGraphRevision":plan.content.plan.request.capability_graph_revision,"mode":"paper","reserveCash":true,"signalReceiptSha256":c.signal_receipt_sha256,"accountCollectionId":state.collections.get("account_data").ok_or("typed_cycle_collection_missing")?,"metadataCollectionId":state.collections.get("instrument_metadata").ok_or("typed_cycle_collection_missing")?,"quoteCollectionId":state.collections.get("pricing_quote").ok_or("typed_cycle_collection_missing")?}}),
        ),
        5 => (
            order_plans::DECIDE,
            json!({"scope":c.scope,"decision":{"planId":state.plan_id.as_ref().ok_or("typed_cycle_plan_missing")?,"activationId":c.activation_id}}),
        ),
        6 => (
            order_plans::SUBMIT,
            json!({"scope":c.scope,"submission":state.submission.as_ref().ok_or("typed_cycle_submission_missing")?}),
        ),
        _ => return Err("typed_cycle_phase_invalid".into()),
    };
    // The retained phase reconstructs this exact separately authorized child.
    // Repeating the phase repeats its original idempotency key, never a new order.
    let key = format!("typed-cycle:{id}:{phase}");
    body["accountMode"] = json!("paper");
    body["idempotencyKey"] = json!(key);
    let response = s.handle_http("POST", path, body);
    if response.status != 200 {
        state.error = Some(
            crate::service::service_error_code(&response.body)
                .unwrap_or_else(|| "typed_cycle_child_failed".into()),
        );
        let uncertain = if phase == 6 {
            if let Some(id) = &state.plan_id {
                order_plans::submission::source(s, id)?
                    .state()?
                    .is_some_and(|record| {
                        matches!(
                            record.phase,
                            order_plans::submission::Phase::Sending
                                | order_plans::submission::Phase::Unknown
                                | order_plans::submission::Phase::Open
                                | order_plans::submission::Phase::PartiallyFilled
                                | order_plans::submission::Phase::Consumed
                        )
                    })
            } else {
                false
            }
        } else {
            false
        };
        state.status = if uncertain { "uncertain" } else { "blocked" }.into();
    } else {
        match phase {
            1..=3 => {
                let role = match phase {
                    1 => "account_data",
                    2 => "instrument_metadata",
                    _ => "pricing_quote",
                };
                state.collections.insert(
                    role.into(),
                    response.body["collection"]["collectionId"]
                        .as_str()
                        .ok_or("typed_cycle_collection_result_invalid")?
                        .into(),
                );
            }
            4 => {
                state.plan_id = Some(
                    response.body["planId"]
                        .as_str()
                        .ok_or("typed_cycle_plan_result_invalid")?
                        .into(),
                )
            }
            5 => {
                let plan_id = state.plan_id.as_ref().ok_or("typed_cycle_plan_missing")?;
                let key = format!("run_{}", c.activation_id);
                let old_run = s
                    .runtime()
                    .storage
                    .get_json("execution_runs", &key)?
                    .ok_or("typed_cycle_run_missing")?;
                let mut fresh_run = old_run.clone();
                if fresh_run["orderPlanApprovals"][plan_id]["planId"] != *plan_id {
                    return Err("typed_cycle_approval_missing".into());
                }
                let window = json!({"schemaVersion":"tradeassembly.execution_window_constraint.v1","scope":c.scope,"occurrenceId":c.occurrence_id,"scheduleReceiptSha256":occurrence.receipt_sha256,"openAtMs":open,"closeAtMs":close});
                if !fresh_run["orderPlanApprovals"][plan_id]["executionWindow"].is_null()
                    && fresh_run["orderPlanApprovals"][plan_id]["executionWindow"] != window
                {
                    return Err("typed_cycle_window_changed".into());
                }
                fresh_run["orderPlanApprovals"][plan_id]["executionWindow"] = window;
                if s.runtime().storage.put_json_batch(
                    &[StorageWrite::new(
                        "execution_runs",
                        &key,
                        fresh_run,
                        ctx.clone(),
                    )],
                    &[
                        StorageExpectation::new("execution_runs", &key, Some(old_run)),
                        StorageExpectation::new(order_plans::submission::STATE, plan_id, None),
                    ],
                )? != ComparePutOutcome::Updated
                {
                    return Err("typed_cycle_approval_conflict".into());
                }
                state.submission = Some(response.body["submission"].clone());
            }
            6 => state.status = "complete".into(),
            _ => {}
        }
        state.phase += 1;
    }
    persist(s, &id, old, &mut state)?;
    Ok(summary(&id, &state, None))
}
pub(crate) fn response(s: &TradeAssemblyService, body: &Value) -> ServiceResponse {
    let result = serde_json::from_value(body["cycle"].clone())
        .map_err(|_| "typed_cycle_command_invalid".to_string())
        .and_then(|c| advance(s, c));
    match result {
        Ok(v) => ServiceResponse::ok(v),
        Err(e) => ServiceResponse::conflict(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_cycle_inputs_are_explicit_bounded_and_non_executable() {
        let value = json!({"sourceInputs":{"account_data":{},"instrument_metadata":{"instrumentId":{"kind":"trade_instrument_id"}},"pricing_quote":{"instrumentId":{"kind":"trade_instrument_id"}}},"maxSubjectsPerOccurrence":1});
        let config: AutomaticConfig = serde_json::from_value(value.clone()).unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.input("pricing_quote", "SPY").unwrap(),
            json!({"instrumentId":"SPY"})
        );
        for fault in ["missing", "bound", "authority"] {
            let mut bad = value.clone();
            match fault {
                "missing" => {
                    bad["sourceInputs"]
                        .as_object_mut()
                        .unwrap()
                        .remove("account_data");
                }
                "bound" => bad["maxSubjectsPerOccurrence"] = json!(0),
                _ => {
                    bad["sourceInputs"]["account_data"]["authority"] =
                        json!({"kind":"literal","value":true})
                }
            };
            let bad: AutomaticConfig = serde_json::from_value(bad).unwrap();
            assert!(bad.validate().is_err());
        }
    }
    #[test]
    fn legacy_typed_config_preserves_absent_automatic_field() {
        let value = json!({"scope":{"strategyId":"s","strategyVersionId":"v","strategySpecHash":"h"},"capabilityGraphRevision":"g","brokerRequirementRef":"b"});
        let config: typed::ConfigCommand = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(config).unwrap(), value);
    }
}
