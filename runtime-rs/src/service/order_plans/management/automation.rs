//! Opted-in, activation-scoped management through ordinary authorized HTTP children.
//! A retained request is persisted before invocation, and its exact response is
//! journaled atomically with progress. Existing submission state only permits
//! lookup/settlement, never another submit. Calendar rollover and automatic
//! reversal-signal evaluation are deliberately not qualified here.
use super::*;
pub(crate) mod reversal;
use crate::service::{execution::typed_cycle, session_dispatch};

const STATE: &str = "execution_management_automation_v1";
const CHILDREN: &str = "execution_management_automation_children_v1";
const LEASE_MS: i64 = 120_000;
const MAX_BYTES: usize = 1_048_576;
const MAX_ENTRIES: usize = 4096;
const ROLES: [&str; 3] = ["account_data", "instrument_metadata", "pricing_quote"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reversal_evaluation: Option<reversal::Config>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_rollover: Option<CalendarRollover>,
    pub cadence_ms: i64,
    pub max_positions_per_round: usize,
    pub source_inputs: BTreeMap<String, BTreeMap<String, typed_cycle::InputValue>>,
}
impl Config {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(reversal) = &self.reversal_evaluation {
            reversal.validate()?;
        }
        if let Some(rollover) = &self.calendar_rollover {
            rollover.validate()?;
        }
        if !(1000..=60_000).contains(&self.cadence_ms)
            || !(1..=32).contains(&self.max_positions_per_round)
            || self
                .source_inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                != ROLES
            || serde_json::to_vec(self)
                .map_err(|_| "management_automation_config_invalid")?
                .len()
                > 16_384
        {
            return Err("management_automation_config_invalid".into());
        }
        for fields in self.source_inputs.values() {
            if fields.len() > 32 {
                return Err("management_automation_config_invalid".into());
            }
            for (name, input) in fields {
                if name.is_empty()
                    || name.len() > 128
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || matches!(
                        name.as_str(),
                        "config" | "credentials" | "authority" | "accountRef" | "account_ref"
                    )
                {
                    return Err("management_automation_input_binding_invalid".into());
                }
                if let typed_cycle::InputValue::Literal { value } = input {
                    crate::service::data_captures::capturable_payload(value)?;
                }
            }
        }
        Ok(())
    }
    fn input(&self, role: &str, instrument: &str) -> Result<Value, String> {
        Ok(Value::Object(
            self.source_inputs
                .get(role)
                .ok_or("management_automation_input_missing")?
                .iter()
                .map(|(name, input)| {
                    (
                        name.clone(),
                        match input {
                            typed_cycle::InputValue::Literal { value } => value.clone(),
                            typed_cycle::InputValue::TradeInstrumentId {} => json!(instrument),
                        },
                    )
                })
                .collect(),
        ))
    }
}

/// Explicit future-session opt-in. No after-hours protection is claimed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CalendarRollover {
    pub schema_version: String,
    pub trigger_ref: String,
    pub refresh_interval_ms: i64,
    pub observation_policy: ObservationPolicy,
    pub source_inputs: BTreeMap<String, CalendarInput>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ObservationPolicy {
    ExecutionSessionOnly,
    ContinuousObservation,
}
/// Existing input variants retain their exact serde contract. Only calendar
/// acquisition may request a zone-derived civil date; it is never a market fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum CalendarInput {
    Existing(typed_cycle::InputValue),
    Date(CalendarDate),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum CalendarDate {
    CalendarDate { offset_days: i32 },
}
impl CalendarRollover {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != "tradeassembly.management_calendar_rollover.v1"
            || self.trigger_ref.is_empty()
            || self.trigger_ref.len() > 128
            || self.trigger_ref.trim() != self.trigger_ref
            || self.trigger_ref.chars().any(char::is_control)
            || !(1000..=3_600_000).contains(&self.refresh_interval_ms)
            || self.source_inputs.len() > 32
        {
            return Err("management_automation_calendar_config_invalid".into());
        }
        if self.observation_policy != ObservationPolicy::ExecutionSessionOnly {
            return Err("management_automation_continuous_observation_unsupported".into());
        }
        for (name, input) in &self.source_inputs {
            if name.is_empty()
                || name.len() > 128
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || matches!(
                    name.as_str(),
                    "config" | "credentials" | "authority" | "accountRef" | "account_ref"
                )
            {
                return Err("management_automation_calendar_input_invalid".into());
            }
            match input {
                CalendarInput::Existing(typed_cycle::InputValue::Literal { value }) => {
                    crate::service::data_captures::capturable_payload(value)?
                }
                CalendarInput::Date(CalendarDate::CalendarDate { offset_days })
                    if !(-366..=366).contains(offset_days) =>
                {
                    return Err("management_automation_calendar_date_offset_invalid".into())
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn input(&self, instrument: &str, timezone: &str, now: i64) -> Result<Value, String> {
        self.validate()?;
        if now <= 0 || now > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64 {
            return Err("management_automation_calendar_clock_invalid".into());
        }
        let zone: chrono_tz::Tz = timezone
            .parse()
            .map_err(|_| "management_automation_calendar_timezone_unsupported")?;
        let today = chrono::DateTime::from_timestamp_millis(now)
            .ok_or("management_automation_calendar_clock_invalid")?
            .with_timezone(&zone)
            .date_naive();
        let mut result = serde_json::Map::new();
        for (name, input) in &self.source_inputs {
            let value = match input {
                CalendarInput::Existing(typed_cycle::InputValue::Literal { value }) => {
                    value.clone()
                }
                CalendarInput::Existing(typed_cycle::InputValue::TradeInstrumentId {}) => {
                    json!(instrument)
                }
                CalendarInput::Date(CalendarDate::CalendarDate { offset_days }) => {
                    let date = today
                        .checked_add_signed(chrono::Duration::days(i64::from(*offset_days)))
                        .ok_or("management_automation_calendar_date_invalid")?
                        .to_string();
                    tradeassembly_plugin_sdk::session_calendar::date_ordinal(&date)?;
                    json!(date)
                }
            };
            result.insert(name.clone(), value);
        }
        Ok(Value::Object(result))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Entry,
    Position,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Work {
    kind: Kind,
    id: String,
    collections: BTreeMap<String, String>,
    submission: Option<Value>,
    reconciled: bool,
    // One completed calendar preparation may serve this observation round even
    // after its periodic poll is due. This is not a calendar freshness permit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_id: Option<String>,
}
impl Work {
    fn new(kind: Kind, id: String) -> Self {
        Self {
            kind,
            id,
            collections: BTreeMap::new(),
            submission: None,
            reconciled: false,
            window_id: None,
        }
    }
    fn admitted_window_matches(&self, heads: &BTreeMap<String, WindowHead>) -> bool {
        self.kind == Kind::Position
            && self.window_id.as_ref().is_some_and(|id| {
                id.strip_prefix("management_window_").is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                }) && heads.get(&self.id).and_then(|head| head.window_id.as_ref()) == Some(id)
            })
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Reversal { step: reversal::Action },
    CollectCalendar {},
    PrepareWindow {},
    Collect { role: String },
    Advance {},
    Submit { plan_id: String },
    Reconcile {},
    Settle {},
    Register {},
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Child {
    key: String,
    path: String,
    body: Value,
    action: Action,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WindowHead {
    window_id: Option<String>,
    refresh_at_ms: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deferred_children: Vec<Child>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    // Earliest expected observation wake for this round. It is scheduler
    // metadata, not market evidence, and is cleared only on a new round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    round_observation_wake_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    reversal_heads: BTreeMap<String, reversal::Progress>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    window_heads: BTreeMap<String, WindowHead>,
    schema_version: String,
    activation_id: String,
    owner_sha256: String,
    config_sha256: String,
    core_revision: String,
    occurrence_id: String,
    round: u64,
    generation: u64,
    child_sequence: u64,
    next_due_ms: i64,
    position_cursor: Option<String>,
    work: Vec<Work>,
    cursor: usize,
    pending: Option<Child>,
    error: Option<String>,
    failures: u32,
    state_sha256: String,
}
struct Binding {
    activation: String,
    config: Config,
    config_hash: String,
    owner: String,
    revision: String,
}

pub(crate) fn validate_reversal_config(
    s: &TradeAssemblyService,
    scope: &data_plans::PlanScope,
    graph: &str,
    policy: Option<&ExitPolicy>,
    config: &Config,
) -> Result<(), String> {
    let Some(reversal) = &config.reversal_evaluation else {
        return Ok(());
    };
    let policy = policy
        .filter(|p| p.rules.reversal_signal.as_deref() == Some("typed.exit"))
        .ok_or("management_reversal_policy_binding_invalid")?;
    reversal.validate_reversal_age(policy.max_reversal_age_ms)?;
    reversal::validate_bindings(s, scope, graph, reversal)
}
pub(crate) fn validate_entry_reversal_sources(
    s: &TradeAssemblyService,
    plan: &data_plans::StoredDataPlan,
    receipt: &data_evaluation::EvaluationReceipt,
    config: &Config,
) -> Result<(), String> {
    let Some(reversal) = &config.reversal_evaluation else {
        return Ok(());
    };
    reversal::validate_entry_sources(s, plan, reversal)?;
    if receipt
        .signals
        .last()
        .is_none_or(|frame| !matches!(frame.exit, crate::data_strategy::SignalValue::Ready { .. }))
    {
        return Err("management_reversal_exit_warmup_required_before_entry".into());
    }
    Ok(())
}
fn retain_reversal_child(state: &mut State, now: i64, cadence: i64, error: String) {
    state.error = Some(error);
    state.failures = state.failures.saturating_add(1).min(16);
    state.next_due_ms = now.saturating_add(
        cadence
            .saturating_mul(1_i64 << state.failures.min(6))
            .min(60_000),
    );
    // Keep cursor, all source/checkpoint progress, and the exact original child.
}
fn current_owner(s: &TradeAssemblyService) -> Result<String, String> {
    let (issuer, session) = s
        .responsible_human_identity()
        .ok_or("management_automation_owner_required")?;
    let now = s.runtime().clock.trusted_now_ms()?;
    let claims = s
        .runtime()
        .identity
        .resolve(session, now)
        .map_err(|_| "management_automation_identity_unavailable")?;
    if claims.issuer != issuer || claims.expires_at_ms <= now {
        return Err("management_automation_identity_unavailable".into());
    }
    owner(s)
}
fn binding(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<Option<Binding>, String> {
    let _timing = crate::service::order_plans::financial_timing("manager.binding");
    let Some(activation) = job.request.execution_activation_id.as_deref() else {
        return Ok(None);
    };
    let who = current_owner(s)?;
    let revision = crate::build_identity::current()
        .ok_or("management_automation_build_identity_required")?
        .revision()
        .to_owned();
    if job.owner_sha256 != who || job.core_revision != revision || job.request.mode != "paper" {
        return Err("management_automation_job_binding_invalid".into());
    }
    let (_, config) = management_run(s, activation)?;
    let Some(automatic) = config["typedExecution"]["automaticExecution"].as_object() else {
        return Ok(None);
    };
    let Some(raw) = automatic.get("management").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let management: Config =
        serde_json::from_value(raw.clone()).map_err(|_| "management_automation_config_invalid")?;
    management.validate()?;
    // Validate the complete opted-in automatic contract, including its parent.
    typed_cycle::automatic(&config)?;
    if config["strategyId"] != job.request.scope.strategy_id
        || config["strategyVersionId"] != job.request.scope.strategy_version_id
        || config["strategySpecHash"] != job.request.scope.strategy_spec_hash
        || config["capabilityGraphRevisionId"] != job.request.capability_graph_revision
        || policy(s, &job.request.scope)?.is_none()
    {
        return Err("management_automation_scope_mismatch".into());
    }
    Ok(Some(Binding {
        activation: activation.into(),
        config: management,
        config_hash: hash(&config)?,
        owner: who,
        revision,
    }))
}
fn state_id(b: &Binding) -> Result<String, String> {
    hash(&(&b.owner, &b.activation))
}
fn state_hash(state: &State) -> Result<String, String> {
    let mut unsigned = state.clone();
    unsigned.state_sha256.clear();
    hash(&unsigned)
}
fn load_state(s: &TradeAssemblyService, b: &Binding) -> Result<Option<(Value, State)>, String> {
    let Some(raw) = s.runtime().storage.get_json(STATE, &state_id(b)?)? else {
        return Ok(None);
    };
    if serde_json::to_vec(&raw)
        .map_err(|_| "management_automation_state_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("management_automation_state_invalid".into());
    }
    let state: State =
        serde_json::from_value(raw.clone()).map_err(|_| "management_automation_state_invalid")?;
    if state.schema_version != "tradeassembly.management_automation.v1"
        || state.owner_sha256 != b.owner
        || state.activation_id != b.activation
        || state.config_sha256 != b.config_hash
        || state.core_revision != b.revision
        || state.state_sha256 != state_hash(&state)?
        || state.round_observation_wake_at_ms.is_some_and(|wake| {
            wake <= 0 || wake > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64
        })
        || state.reversal_heads.len() > MAX_ENTRIES
        || state
            .reversal_heads
            .values()
            .any(|progress| progress.validate().is_err())
        || state.window_heads.len() > MAX_ENTRIES
        || state.window_heads.values().any(|head| {
            head.refresh_at_ms <= 0
                || head
                    .window_id
                    .as_ref()
                    .is_some_and(|id| !id.starts_with("management_window_") || id.len() != 82)
                || head.deferred_children.len() > 4
                || head.deferred_children.iter().any(|child| {
                    !calendar_action(&child.action) && !observation_action(&child.action)
                })
        })
        || state.work.iter().any(|work| {
            work.window_id.is_some() && !work.admitted_window_matches(&state.window_heads)
        })
        || state.cursor > state.work.len()
        || state.work.len() > MAX_ENTRIES + 32
        || state.failures > 16
        || state.child_sequence > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
        || state.generation > tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER
    {
        return Err("management_automation_state_invalid".into());
    }
    Ok(Some((raw, state)))
}
fn context(key: &str) -> Result<SideEffectContext, String> {
    Ok(SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key)?,
    ))
}
fn save(
    s: &TradeAssemblyService,
    b: &Binding,
    old: Option<Value>,
    state: &mut State,
    outcome: Option<(&Child, Value)>,
) -> Result<(), String> {
    state.generation = state
        .generation
        .checked_add(1)
        .filter(|n| *n <= tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER)
        .ok_or("management_automation_generation_exhausted")?;
    state.state_sha256 = state_hash(state)?;
    let raw = value(state)?;
    if serde_json::to_vec(&raw)
        .map_err(|_| "management_automation_state_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("management_automation_state_too_large".into());
    }
    let id = state_id(b)?;
    let ctx = context(&format!("management-state:{id}:{}", state.generation))?;
    let mut writes = vec![StorageWrite::new(STATE, &id, raw, ctx.clone())];
    let mut expectations = vec![StorageExpectation::new(STATE, &id, old)];
    if let Some((child, response)) = outcome {
        let child_id = if response["retryRetainedOriginalChild"] == true {
            hash(&(&child.key, state.generation))?
        } else {
            hash(&child.key)?
        };
        let record = json!({"schemaVersion":"tradeassembly.management_automation_child.v1",
            "activationId":b.activation,"ownerSha256":b.owner,"coreRevision":b.revision,
            "configSha256":b.config_hash,"child":child,"response":response});
        writes.push(StorageWrite::new(CHILDREN, &child_id, record, ctx));
        expectations.push(StorageExpectation::new(CHILDREN, &child_id, None));
    }
    if s.runtime().storage.put_json_batch(&writes, &expectations)? != ComparePutOutcome::Updated {
        return Err("management_automation_conflict".into());
    }
    Ok(())
}
fn registered_id(s: &TradeAssemblyService, plan: &OrderPlan) -> Result<String, String> {
    Ok(format!("position_{}", hash(&(owner(s)?, &plan.plan_id))?))
}
fn check_plan(plan: &OrderPlan, b: &Binding, job: &session_dispatch::Job) -> Result<(), String> {
    if plan.owner_sha256 != b.owner
        || plan.command.scope != job.request.scope
        || plan.command.capability_graph_revision != job.request.capability_graph_revision
    {
        return Err("management_automation_plan_binding_invalid".into());
    }
    Ok(())
}
fn entry_work(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
) -> Result<Vec<Work>, String> {
    let mut tasks = Vec::new();
    for (id, raw) in s.runtime().storage.list_json(submission::STATE)? {
        if raw["ownerSha256"] != b.owner
            || raw["intent"]["originalRequest"]["activationId"] != b.activation
        {
            continue;
        }
        let plan = read_owned(s, &id)?;
        check_plan(&plan, b, job)?;
        if plan.management.is_some() {
            continue;
        }
        let (_, record) =
            submission::record(s, &plan)?.ok_or("management_automation_entry_missing")?;
        let pending = matches!(
            record.phase,
            submission::Phase::Admitting
                | submission::Phase::Admitted
                | submission::Phase::Sending
                | submission::Phase::Unknown
                | submission::Phase::Open
                | submission::Phase::PartiallyFilled
        );
        let unregistered = record.phase == submission::Phase::Consumed
            && record.filled_quantity.positive()
            && s.runtime()
                .storage
                .get_json(POSITIONS, &registered_id(s, &plan)?)?
                .is_none();
        if pending || unregistered {
            tasks.push(Work::new(Kind::Entry, id));
        }
        if tasks.len() > MAX_ENTRIES {
            return Err("management_automation_entry_limit".into());
        }
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tasks)
}
fn position_ids(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for (id, raw) in s.runtime().storage.list_json(POSITIONS)? {
        if raw["ownerSha256"] != b.owner || raw["activationId"] != b.activation {
            continue;
        }
        let (_, position) = load(s, &id)?;
        check_plan(&read_owned(s, &position.entry_plan_id)?, b, job)?;
        if position
            .quantity
            .checked_subtract(position.closed_quantity)?
            .positive()
            || !position.reservations.is_empty()
        {
            ids.push(id);
        }
        if ids.len() > MAX_ENTRIES {
            return Err("management_automation_position_limit".into());
        }
    }
    ids.sort();
    Ok(ids)
}
fn rotate(ids: &[String], after: Option<&str>, max: usize) -> Vec<String> {
    let split = after.map_or(0, |after| ids.partition_point(|id| id.as_str() <= after));
    ids[split..]
        .iter()
        .chain(ids[..split].iter())
        .take(max)
        .cloned()
        .collect()
}
/// Read-only wake eligibility for the separately verified health-read binding.
/// Financial management still calls binding()/typed::current normally.
pub(crate) fn has_owned_work_for_health(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
    config: &Value,
) -> Result<bool, String> {
    let Some(activation) = job.request.execution_activation_id.as_deref() else {
        return Ok(false);
    };
    let Some(raw) = config["typedExecution"]["automaticExecution"]
        .get("management")
        .filter(|v| !v.is_null())
    else {
        return Ok(false);
    };
    let management: Config =
        serde_json::from_value(raw.clone()).map_err(|_| "management_automation_config_invalid")?;
    management.validate()?;
    let who = current_owner(s)?;
    let revision = crate::build_identity::current()
        .ok_or("management_automation_build_identity_required")?
        .revision()
        .to_owned();
    if who != job.owner_sha256
        || revision != job.core_revision
        || job.request.mode != "paper"
        || config["strategyId"] != job.request.scope.strategy_id
        || config["strategyVersionId"] != job.request.scope.strategy_version_id
        || config["strategySpecHash"] != job.request.scope.strategy_spec_hash
        || config["capabilityGraphRevisionId"] != job.request.capability_graph_revision
    {
        return Err("management_automation_scope_mismatch".into());
    }
    let b = Binding {
        activation: activation.into(),
        config: management,
        config_hash: hash(config)?,
        owner: who,
        revision,
    };
    has_bound_work(s, &b, job)
}
fn has_loaded_work(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
    state: Option<&State>,
) -> Result<bool, String> {
    if state.is_some_and(|state| state.pending.is_some() || state.cursor < state.work.len()) {
        return Ok(true);
    }
    Ok(!entry_work(s, b, job)?.is_empty() || !position_ids(s, b, job)?.is_empty())
}
fn has_bound_work(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
) -> Result<bool, String> {
    let loaded = load_state(s, b)?;
    has_loaded_work(s, b, job, loaded.as_ref().map(|(_, state)| state))
}
pub(crate) fn has_work(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<bool, String> {
    let Some(b) = binding(s, job)? else {
        return Ok(false);
    };
    has_bound_work(s, &b, job)
}
/// Advisory wake/selection data for one worker scheduling decision only. This
/// carries no binding, grant, child body or permission into an effect handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SchedulingState {
    pub has_work: bool,
    pub next_due_ms: i64,
}
fn bound_scheduling_state(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
    now: i64,
) -> Result<SchedulingState, String> {
    let loaded = load_state(s, b)?;
    let state = loaded.as_ref().map(|(_, state)| state);
    Ok(SchedulingState {
        has_work: has_loaded_work(s, b, job, state)?,
        next_due_ms: state.map_or(now, |state| state.next_due_ms),
    })
}
pub(crate) fn scheduling_state(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<SchedulingState, String> {
    let _timing = crate::service::order_plans::financial_timing("manager.next_due");
    let now = s.runtime().clock.trusted_now_ms()?;
    let Some(b) = binding(s, job)? else {
        return Ok(SchedulingState {
            has_work: false,
            next_due_ms: now.saturating_add(60_000),
        });
    };
    bound_scheduling_state(s, &b, job, now)
}
fn summary(state: &State) -> Value {
    let status = if state.error.is_some() {
        "blocked"
    } else if state.cursor < state.work.len() {
        "running"
    } else {
        "waiting"
    };
    json!({"activationId":state.activation_id,"status":status,"round":state.round,"nextAttemptAtMs":state.next_due_ms,
        "workRemaining":state.work.len().saturating_sub(state.cursor),"error":state.error,
        "pendingChildKey":state.pending.as_ref().map(|c| &c.key),"executionAuthorityGranted":false,
        "calendarRolloverQualified":false,"automaticReversalQualified":false})
}
fn child(state: &mut State, path: &str, mut body: Value, action: Action) -> Result<Child, String> {
    state.child_sequence = state
        .child_sequence
        .checked_add(1)
        .filter(|n| *n <= tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER)
        .ok_or("management_automation_child_sequence_exhausted")?;
    let key = format!(
        "managed-auto:{}:{}:{}",
        hash(&(&state.owner_sha256, &state.activation_id))?,
        state.round,
        state.child_sequence
    );
    body["accountMode"] = json!("paper");
    body["idempotencyKey"] = json!(key);
    if serde_json::to_vec(&body)
        .map_err(|_| "management_automation_child_invalid")?
        .len()
        > 32_768
    {
        return Err("management_automation_child_too_large".into());
    }
    Ok(Child {
        key,
        path: path.into(),
        body,
        action,
    })
}
fn plan_child(
    state: &mut State,
    plan: &OrderPlan,
    path: &str,
    action: Action,
) -> Result<Child, String> {
    child(
        state,
        path,
        json!({"scope":plan.command.scope,"planId":plan.plan_id}),
        action,
    )
}
fn advance_child(
    state: &mut State,
    plan: &OrderPlan,
    position: &Position,
    occurrence: &str,
    collections: [&str; 3],
    reversal_receipt: Option<&str>,
) -> Result<Child, String> {
    child(
        state,
        ADVANCE,
        json!({"scope":plan.command.scope,"advance":{
        "executionOccurrenceId":occurrence,"positionId":position.position_id,
        "accountCollectionId":collections[0],"metadataCollectionId":collections[1],
        "quoteCollectionId":collections[2],"reversalSignalReceiptSha256":reversal_receipt}}),
        Action::Advance {},
    )
}
fn calendar_action(action: &Action) -> bool {
    matches!(
        action,
        Action::CollectCalendar {} | Action::PrepareWindow {}
    )
}
fn observation_action(action: &Action) -> bool {
    matches!(
        action,
        Action::Collect { .. } | Action::Advance {} | Action::Reversal { .. }
    )
}
fn head_current(
    s: &TradeAssemblyService,
    state: &State,
    position_id: &str,
    now: i64,
) -> Result<Option<crate::service::management_windows::Current>, String> {
    let Some(head) = state.window_heads.get(position_id) else {
        return Ok(None);
    };
    if now >= head.refresh_at_ms {
        return Ok(None);
    }
    let Some(id) = &head.window_id else {
        return Ok(None);
    };
    match crate::service::management_windows::current(s, id) {
        Ok(current) => Ok(Some(current)),
        Err(error) if error == "management_window_calendar_stale" => Ok(None),
        Err(error) => Err(error),
    }
}
fn effect_window(
    s: &TradeAssemblyService,
    state: &State,
    position_id: &str,
) -> Result<Option<crate::service::management_windows::Current>, String> {
    let Some(id) = state
        .window_heads
        .get(position_id)
        .and_then(|head| head.window_id.as_ref())
    else {
        return Ok(None);
    };
    match crate::service::management_windows::current(s, id) {
        Ok(current) => Ok(Some(current)),
        Err(error) if error == "management_window_calendar_stale" => Ok(None),
        Err(error) => Err(error),
    }
}
// A successful PrepareWindow is useful evidence for this bounded Work round;
// do not let its next periodic poll starve the first account/metadata/quote read.
// Both paths retain the same current source, provider TTL and session checks.
fn work_window_current(
    s: &TradeAssemblyService,
    state: &State,
    work: &Work,
    now: i64,
) -> Result<Option<crate::service::management_windows::Current>, String> {
    let observation_started = work.admitted_window_matches(&state.window_heads)
        || ROLES
            .iter()
            .any(|role| work.collections.contains_key(*role));
    if observation_started {
        effect_window(s, state, &work.id)
    } else {
        head_current(s, state, &work.id, now)
    }
}
fn refresh_deadline(now: i64, interval: i64, fresh_until: i64, close: Option<i64>) -> i64 {
    now.saturating_add(interval)
        .min(fresh_until)
        .min(close.unwrap_or(i64::MAX))
}
fn refresh_at(
    receipt: &crate::service::management_windows::Receipt,
    rollover: &CalendarRollover,
    now: i64,
) -> i64 {
    refresh_deadline(
        now,
        rollover.refresh_interval_ms,
        receipt.fresh_until_ms,
        receipt.selection.close_at_ms,
    )
}
/// A retained read/evidence request never changes body or key on restart.
/// Deferred requests remain in the owned head even if its lot becomes flat;
/// cache pruning does not silently discard an unknown original read outcome.
fn defer_pending(state: &mut State, position_id: &str) -> Result<(), String> {
    let pending = state
        .pending
        .as_ref()
        .ok_or("management_automation_child_missing")?
        .clone();
    if !calendar_action(&pending.action) && !observation_action(&pending.action) {
        return Err("management_automation_financial_child_cannot_be_deferred".into());
    }
    if !state.window_heads.contains_key(position_id) && state.window_heads.len() >= MAX_ENTRIES {
        return Err("management_automation_window_head_limit".into());
    }
    let head = state
        .window_heads
        .entry(position_id.into())
        .or_insert(WindowHead {
            window_id: None,
            refresh_at_ms: 1,
            deferred_children: Vec::new(),
        });
    if head.deferred_children.len() >= 4 {
        return Err("management_automation_deferred_child_limit".into());
    }
    if head
        .deferred_children
        .iter()
        .any(|child| child.key == pending.key)
    {
        return Err("management_automation_deferred_child_conflict".into());
    }
    head.deferred_children.push(pending);
    state.pending = None;
    Ok(())
}
/// Only free positions reach this function; pending close recovery precedes it.
fn rollover_child(
    s: &TradeAssemblyService,
    rollover: &CalendarRollover,
    state: &mut State,
    work: &Work,
    entry: &OrderPlan,
    position: &Position,
) -> Result<WindowStep, String> {
    let now = s.runtime().clock.trusted_now_ms()?;
    if let Some(retained) = state
        .window_heads
        .get_mut(&position.position_id)
        .and_then(|head| {
            let index = head
                .deferred_children
                .iter()
                .position(|child| calendar_action(&child.action))?;
            Some(head.deferred_children.remove(index))
        })
    {
        return Ok(WindowStep::Child(retained));
    }
    if let Some(collection) = work.collections.get("execution_calendar") {
        return child(state, crate::service::management_windows::PREPARE, json!({"managementWindow":{
            "scope":entry.command.scope,"positionId":position.position_id,"triggerRef":rollover.trigger_ref,
            "executionCalendarCollectionId":collection}}), Action::PrepareWindow {}).map(WindowStep::Child);
    }
    let current = work_window_current(s, state, work, now)?;
    if let Some(current) = current {
        if current.status != crate::service::management_windows::Status::Active {
            return Ok(WindowStep::Wait);
        }
        let id = current.receipt.window_id;
        if let Some(retained) = state
            .window_heads
            .get_mut(&position.position_id)
            .and_then(|head| {
                if head.deferred_children.is_empty() {
                    None
                } else {
                    Some(head.deferred_children.remove(0))
                }
            })
        {
            return Ok(WindowStep::Child(retained));
        }
        return Ok(WindowStep::Ready(id));
    }
    if !state.window_heads.contains_key(&position.position_id)
        && state.window_heads.len() >= MAX_ENTRIES
    {
        return Err("management_automation_window_head_limit".into());
    }
    let target = crate::service::management_windows::acquisition(
        s,
        &position.position_id,
        &rollover.trigger_ref,
    )?;
    if target.scope != entry.command.scope
        || target.capability_graph_revision != entry.command.capability_graph_revision
    {
        return Err("management_automation_calendar_scope_mismatch".into());
    }
    let input = rollover.input(
        &entry.order.instrument.instrument_id,
        &target.calendar.timezone,
        now,
    )?;
    child(state, "/product/strategies/data-collections/create", json!({"collection":{
        "scope":target.scope,"capabilityGraphRevision":target.capability_graph_revision,
        "acquisitionNode":target.calendar.capability_requirement_ref,"mode":"paper",
        "responseSchemaRef":tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA,"input":input}}),
        Action::CollectCalendar {}).map(WindowStep::Child)
}
enum WindowStep {
    Child(Child),
    Ready(String),
    Wait,
}

enum PlannedChild {
    Child(Child),
    Complete,
    Wait(reversal::ObservationWait),
}

fn next_child(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
    state: &mut State,
) -> Result<PlannedChild, String> {
    let Some(work) = state.work.get(state.cursor).cloned() else {
        return Ok(PlannedChild::Complete);
    };
    if work.kind == Kind::Entry {
        let plan = read_owned(s, &work.id)?;
        check_plan(&plan, b, job)?;
        let (_, record) =
            submission::record(s, &plan)?.ok_or("management_automation_entry_missing")?;
        if record.intent["originalRequest"]["activationId"] != b.activation
            || plan.management.is_some()
        {
            return Err("management_automation_entry_binding_invalid".into());
        }
        if record.phase == submission::Phase::Consumed && record.filled_quantity.positive() {
            if record.settlement_receipt_sha256.is_none() {
                return plan_child(state, &plan, SETTLE, Action::Settle {})
                    .map(PlannedChild::Child);
            }
            if s.runtime()
                .storage
                .get_json(POSITIONS, &registered_id(s, &plan)?)?
                .is_none()
            {
                return child(
                    state,
                    REGISTER,
                    json!({"scope":plan.command.scope,"register":{
                    "entryPlanId":plan.plan_id,"activationId":b.activation}}),
                    Action::Register {},
                )
                .map(PlannedChild::Child);
            }
        } else if !work.reconciled
            && matches!(
                record.phase,
                submission::Phase::Admitting
                    | submission::Phase::Admitted
                    | submission::Phase::Sending
                    | submission::Phase::Unknown
                    | submission::Phase::Open
                    | submission::Phase::PartiallyFilled
            )
        {
            return plan_child(state, &plan, RECONCILE, Action::Reconcile {})
                .map(PlannedChild::Child);
        }
        return Ok(PlannedChild::Complete);
    }
    let (_, position) = load(s, &work.id)?;
    if position.activation_id != b.activation {
        return Err("management_automation_position_binding_invalid".into());
    }
    let entry = read_owned(s, &position.entry_plan_id)?;
    check_plan(&entry, b, job)?;
    if !position.reservations.is_empty() {
        if position.reservations.len() != 1 {
            return Err("management_automation_multiple_closes_invalid".into());
        }
        let id = position
            .reservations
            .keys()
            .next()
            .ok_or("management_automation_close_missing")?;
        let plan = read_owned(s, id)?;
        check_plan(&plan, b, job)?;
        let close = plan
            .management
            .as_ref()
            .ok_or("management_automation_close_binding_missing")?;
        if close.position_id != position.position_id
            || close.original_entry_plan_id != entry.plan_id
        {
            return Err("management_automation_close_binding_invalid".into());
        }
        if let Some((_, record)) = submission::record(s, &plan)? {
            if record.intent["originalRequest"]["activationId"] != b.activation {
                return Err("management_automation_close_activation_invalid".into());
            }
            match record.phase {
                submission::Phase::Consumed
                | submission::Phase::Rejected
                | submission::Phase::Cancelled => {
                    if record.settlement_receipt_sha256.is_none() {
                        return plan_child(state, &plan, SETTLE, Action::Settle {})
                            .map(PlannedChild::Child);
                    }
                    return Ok(PlannedChild::Complete);
                }
                submission::Phase::Denied => {}
                _ => {
                    return if work.reconciled {
                        Ok(PlannedChild::Complete)
                    } else {
                        plan_child(state, &plan, RECONCILE, Action::Reconcile {})
                            .map(PlannedChild::Child)
                    };
                }
            }
        } else if s.runtime().clock.trusted_now_ms()? >= plan.valid_until_ms {
            // An unsent expired reservation is released through the original
            // normal Advance before any calendar refresh or new proposal.
            return advance_child(
                state,
                &plan,
                &position,
                &close.execution_occurrence_id,
                [
                    &plan.command.account_collection_id,
                    &plan.command.metadata_collection_id,
                    &plan.command.quote_collection_id,
                ],
                None,
            )
            .map(PlannedChild::Child);
        } else if let Some(submission) = &work.submission {
            if submission["planId"] != plan.plan_id {
                return Err("management_automation_submission_binding_invalid".into());
            }
            // This is the only submit construction: no submission record exists.
            return child(
                state,
                SUBMIT,
                json!({"scope":plan.command.scope,"submission":submission}),
                Action::Submit {
                    plan_id: plan.plan_id.clone(),
                },
            )
            .map(PlannedChild::Child);
        }
        // Resume the retained close using its original collections and window.
        return advance_child(
            state,
            &plan,
            &position,
            &close.execution_occurrence_id,
            [
                &plan.command.account_collection_id,
                &plan.command.metadata_collection_id,
                &plan.command.quote_collection_id,
            ],
            None,
        )
        .map(PlannedChild::Child);
    }
    if !position
        .quantity
        .checked_subtract(position.closed_quantity)?
        .positive()
    {
        return Ok(PlannedChild::Complete);
    }
    if position.policy.rules.reversal_signal.is_some() && b.config.reversal_evaluation.is_none() {
        return Err("management_automation_reversal_evidence_unsupported".into());
    }
    let occurrence = if let Some(rollover) = &b.config.calendar_rollover {
        match rollover_child(s, rollover, state, &work, &entry, &position)? {
            WindowStep::Child(child) => return Ok(PlannedChild::Child(child)),
            WindowStep::Wait => return Ok(PlannedChild::Complete),
            WindowStep::Ready(id) => id,
        }
    } else {
        // Legacy opt-in remains limited to the exact original occurrence.
        execution_window(s, &entry, &state.occurrence_id)?;
        state.occurrence_id.clone()
    };
    if b.config.calendar_rollover.is_none() {
        if let Some(retained) = state
            .window_heads
            .get_mut(&position.position_id)
            .and_then(|head| {
                if head.deferred_children.is_empty() {
                    None
                } else {
                    Some(head.deferred_children.remove(0))
                }
            })
        {
            return Ok(PlannedChild::Child(retained));
        }
    }
    let reversal_receipt = if let Some(config) = &b.config.reversal_evaluation {
        match reversal::next_child(s, config, state, &entry, &position)? {
            reversal::Step::Child(child) => return Ok(PlannedChild::Child(child)),
            reversal::Step::Wait(wait) => return Ok(PlannedChild::Wait(wait)),
            reversal::Step::Ready(receipt) => Some(receipt),
        }
    } else {
        None
    };
    for (role, schema) in [
        ("account_data", ACCOUNT_SCHEMA),
        ("instrument_metadata", METADATA_SCHEMA),
        ("pricing_quote", QUOTE_SCHEMA),
    ] {
        if !work.collections.contains_key(role) {
            let node = entry
                .policy
                .source_nodes
                .get(role)
                .ok_or("management_automation_source_missing")?;
            return child(state, "/product/strategies/data-collections/create", json!({"collection":{
                "scope":entry.command.scope,"capabilityGraphRevision":entry.command.capability_graph_revision,
                "acquisitionNode":node,"mode":"paper","responseSchemaRef":schema,
                "input":b.config.input(role, &entry.order.instrument.instrument_id)?}}), Action::Collect { role:role.into() }).map(PlannedChild::Child);
        }
    }
    advance_child(
        state,
        &entry,
        &position,
        &occurrence,
        [
            &work.collections["account_data"],
            &work.collections["instrument_metadata"],
            &work.collections["pricing_quote"],
        ],
        reversal_receipt.as_deref(),
    )
    .map(PlannedChild::Child)
}
fn complete_work(state: &mut State) {
    state.cursor = (state.cursor + 1).min(state.work.len());
    state.pending = None;
}
// Expected observation waits are successful planning outcomes, not failed
// provider calls. They retain the evaluator cursor/checkpoint and never grant
// permission to collect financial facts or advance a close.
fn apply_planned_child(
    state: &mut State,
    now: i64,
    cadence: i64,
    planned: Result<PlannedChild, String>,
) {
    match planned {
        Ok(PlannedChild::Child(next)) => state.pending = Some(next),
        Ok(PlannedChild::Complete) => complete_work(state),
        Ok(PlannedChild::Wait(wait)) => {
            state.error = None;
            state.failures = 0;
            complete_work(state);
            let due = wait.next_due_ms(now, cadence);
            state.round_observation_wake_at_ms = Some(
                state
                    .round_observation_wake_at_ms
                    .map_or(due, |earlier| earlier.min(due)),
            );
            // Finish this position without delaying the remaining round.
            // The retained wake is applied when the whole round completes.
            state.next_due_ms = now;
        }
        Err(error) => fail_work(state, now, cadence, error),
    }
    schedule_round_end(state, now, cadence);
}

fn schedule_round_end(state: &mut State, now: i64, cadence: i64) {
    if state.cursor == state.work.len() {
        state.next_due_ms = state.next_due_ms.max(now.saturating_add(cadence));
    }
    // A later work item or successful child must not erase an earlier
    // observation wake. Actual refusal/unknown-child backoff remains binding;
    // never apply a global minimum to an error deadline.
    if state.error.is_some() {
        return;
    }
    if let Some(wake) = state.round_observation_wake_at_ms {
        state.next_due_ms = state.next_due_ms.min(wake.max(now));
    }
}

fn fail_work(state: &mut State, now: i64, cadence: i64, error: String) {
    state.error = Some(error);
    state.failures = state.failures.saturating_add(1).min(16);
    let backoff = cadence
        .saturating_mul(1_i64 << state.failures.min(6))
        .min(60_000);
    state.next_due_ms = now.saturating_add(backoff);
    complete_work(state);
}
fn validate_retained_window_command(
    child: &Child,
    command: &crate::service::management_windows::Command,
) -> Result<(), String> {
    let requested: crate::service::management_windows::Command =
        serde_json::from_value(child.body["managementWindow"].clone())
            .map_err(|_| "management_automation_window_request_invalid")?;
    if requested != *command {
        return Err("management_automation_window_result_binding_invalid".into());
    }
    Ok(())
}
fn apply_response(
    s: &TradeAssemblyService,
    b: &Binding,
    state: &mut State,
    child: &Child,
    response: &ServiceResponse,
) -> Result<(), String> {
    if let Action::Reversal { step } = &child.action {
        reversal::apply_response(s, b, state, child, step, response)?;
        state.pending = None;
        return Ok(());
    }
    let work = state
        .work
        .get_mut(state.cursor)
        .ok_or("management_automation_work_missing")?;
    match &child.action {
        Action::Reversal { .. } => return Err("management_reversal_action_invalid".into()),
        Action::CollectCalendar {} => {
            let id = response.body["collection"]["collectionId"]
                .as_str()
                .ok_or("management_automation_calendar_result_invalid")?;
            if id.is_empty() || id.len() > 256 {
                return Err("management_automation_calendar_result_invalid".into());
            }
            work.collections
                .insert("execution_calendar".into(), id.into());
        }
        Action::PrepareWindow {} => {
            let returned: crate::service::management_windows::Receipt =
                serde_json::from_value(response.body["managementWindow"].clone())
                    .map_err(|_| "management_automation_window_result_invalid")?;
            let receipt = crate::service::management_windows::historical(s, &returned.window_id)?;
            let rollover = b
                .config
                .calendar_rollover
                .as_ref()
                .ok_or("management_automation_rollover_disabled")?;
            validate_retained_window_command(child, &receipt.command)?;
            if returned != receipt
                || receipt.command.position_id != work.id
                || work.kind != Kind::Position
                || receipt.command.trigger_ref != rollover.trigger_ref
                || receipt.subject.lineage.activation_id != b.activation
                || receipt.subject.config_sha256 != b.config_hash
            {
                return Err("management_automation_window_result_binding_invalid".into());
            }
            let deferred_children = state
                .window_heads
                .get_mut(&work.id)
                .map(|head| std::mem::take(&mut head.deferred_children))
                .unwrap_or_default();
            state.window_heads.insert(
                work.id.clone(),
                WindowHead {
                    window_id: Some(receipt.window_id.clone()),
                    refresh_at_ms: refresh_at(
                        &receipt,
                        rollover,
                        s.runtime().clock.trusted_now_ms()?,
                    ),
                    deferred_children,
                },
            );
            work.window_id = Some(receipt.window_id.clone());
            work.collections.remove("execution_calendar");
        }
        Action::Collect { role } => {
            let id = response.body["collection"]["collectionId"]
                .as_str()
                .ok_or("management_automation_collection_result_invalid")?;
            if id.len() > 256 || id.is_empty() {
                return Err("management_automation_collection_result_invalid".into());
            }
            work.collections.insert(role.clone(), id.into());
        }
        Action::Advance {} => {
            if response.body["decision"].is_null() {
                complete_work(state);
            } else {
                let command: transport::SubmissionCommand =
                    serde_json::from_value(response.body["decision"]["submission"].clone())
                        .map_err(|_| "management_automation_decision_result_invalid")?;
                command.validate()?;
                let plan = read_owned(s, &command.plan_id)?;
                let close = plan
                    .management
                    .as_ref()
                    .ok_or("management_automation_decision_close_missing")?;
                if command.activation_id != b.activation
                    || close.position_id != work.id
                    || work.kind != Kind::Position
                {
                    return Err("management_automation_decision_binding_invalid".into());
                }
                work.submission = Some(response.body["decision"]["submission"].clone());
            }
        }
        Action::Reconcile {} => {
            work.reconciled = true;
            work.submission = None;
        }
        Action::Submit { .. } => {
            work.submission = None;
        }
        Action::Settle {} => {
            if work.kind == Kind::Position {
                complete_work(state);
            }
        }
        Action::Register {} => complete_work(state),
    }
    state.pending = None;
    Ok(())
}

pub(crate) fn advance(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<Value, String> {
    let _timing = crate::service::order_plans::financial_timing("manager.advance");
    let Some(b) = binding(s, job)? else {
        return Ok(json!({"enabled":false,"executionAuthorityGranted":false}));
    };
    let now = s.runtime().clock.trusted_now_ms()?;
    let resource = format!("managed-automation:{}", state_id(&b)?);
    let Some(lease) = s.runtime().leases.acquire(
        &resource,
        &format!("managed-worker:{}", job.job_id),
        now,
        LEASE_MS,
    )?
    else {
        return Ok(
            json!({"activationId":b.activation,"status":"lease_busy","nextAttemptAtMs":now.saturating_add(1000),"executionAuthorityGranted":false}),
        );
    };
    let result = (|| {
        // Only the activation lease holder may initialize or advance a round.
        let loaded = load_state(s, &b)?;
        let (mut old, mut state) = match loaded {
            Some((raw, state)) => (Some(raw), state),
            None => (
                None,
                State {
                    round_observation_wake_at_ms: None,
                    reversal_heads: BTreeMap::new(),
                    window_heads: BTreeMap::new(),
                    schema_version: "tradeassembly.management_automation.v1".into(),
                    activation_id: b.activation.clone(),
                    owner_sha256: b.owner.clone(),
                    config_sha256: b.config_hash.clone(),
                    core_revision: b.revision.clone(),
                    occurrence_id: job.request.occurrence_id.clone(),
                    round: 0,
                    generation: 0,
                    child_sequence: 0,
                    next_due_ms: now,
                    position_cursor: None,
                    work: Vec::new(),
                    cursor: 0,
                    pending: None,
                    error: None,
                    failures: 0,
                    state_sha256: String::new(),
                },
            ),
        };
        if now < state.next_due_ms {
            return Ok(summary(&state));
        }
        if state.cursor == state.work.len() && state.pending.is_none() {
            state.round_observation_wake_at_ms = None;
            state.work = entry_work(s, &b, job)?;
            let owned_positions = position_ids(s, &b, job)?;
            let live = owned_positions
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            state
                .window_heads
                .retain(|id, head| live.contains(id) || !head.deferred_children.is_empty());
            state.reversal_heads.retain(|id, _| live.contains(id));
            let selected = rotate(
                &owned_positions,
                state.position_cursor.as_deref(),
                b.config.max_positions_per_round,
            );
            state.position_cursor = selected.last().cloned().or(state.position_cursor);
            state
                .work
                .extend(selected.into_iter().map(|id| Work::new(Kind::Position, id)));
            state.cursor = 0;
            state.round = state
                .round
                .checked_add(1)
                .filter(|n| *n <= tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER)
                .ok_or("management_automation_round_exhausted")?;
            if b.config.calendar_rollover.is_none() {
                state.occurrence_id = job.request.occurrence_id.clone();
            }
            if state.work.is_empty() {
                state.next_due_ms = now.saturating_add(b.config.cadence_ms);
                save(s, &b, old, &mut state, None)?;
                return Ok(summary(&state));
            }
        }
        // An interrupted submit may already have reached durable broker state.
        // Preserve the original child, then continue by lookup, never resend.
        if let Some(pending) = state.pending.clone() {
            if let Action::Submit { plan_id } = &pending.action {
                let plan = read_owned(s, plan_id)?;
                if submission::record(s, &plan)?.is_some() {
                    state.pending = None;
                    if let Some(work) = state.work.get_mut(state.cursor) {
                        work.submission = None;
                    }
                    save(
                        s,
                        &b,
                        old,
                        &mut state,
                        Some((
                            &pending,
                            json!({"status":"submission_state_exists_response_unknown","lookupRequired":true}),
                        )),
                    )?;
                    return Ok(summary(&state));
                }
                if s.runtime().clock.trusted_now_ms()? >= plan.valid_until_ms {
                    state.pending = None;
                    if let Some(work) = state.work.get_mut(state.cursor) {
                        work.submission = None;
                    }
                    save(
                        s,
                        &b,
                        old,
                        &mut state,
                        Some((
                            &pending,
                            json!({"status":"original_unsent_submit_expired","submissionStateAbsent":true,"normalAbortRequired":true}),
                        )),
                    )?;
                    return Ok(summary(&state));
                }
            }
        }
        if b.config.calendar_rollover.is_some() || b.config.reversal_evaluation.is_some() {
            if let Some(pending) = state.pending.clone() {
                let work = state
                    .work
                    .get(state.cursor)
                    .cloned()
                    .ok_or("management_automation_work_missing")?;
                if work.kind == Kind::Position
                    && (calendar_action(&pending.action) || observation_action(&pending.action))
                {
                    let (_, position) = load(s, &work.id)?;
                    let financial_pending = !position.reservations.is_empty();
                    let outside_session = if b.config.calendar_rollover.is_some()
                        && observation_action(&pending.action)
                        && !financial_pending
                    {
                        effect_window(s, &state, &work.id)?.is_none_or(|window| {
                            window.status != crate::service::management_windows::Status::Active
                        })
                    } else {
                        false
                    };
                    if (financial_pending && !matches!(&pending.action, Action::Advance {}))
                        || outside_session
                    {
                        defer_pending(&mut state, &work.id)?;
                        save(s, &b, old, &mut state, None)?;
                        return Ok(summary(&state));
                    }
                }
            }
        }
        if state.pending.is_none() {
            let planned = measured("manager.next_child", || next_child(s, &b, job, &mut state));
            apply_planned_child(&mut state, now, b.config.cadence_ms, planned);
            save(s, &b, old, &mut state, None)?;
            old = Some(value(&state)?);
            if state.pending.is_none() {
                return Ok(summary(&state));
            }
        }
        let pending = state
            .pending
            .clone()
            .ok_or("management_automation_child_missing")?;
        // All materialized body bytes/key are durable before this sole child.
        let fresh = binding(s, job)?.ok_or("management_automation_disabled")?;
        if fresh.config_hash != b.config_hash
            || current_owner(s)? != b.owner
            || s.runtime().leases.current(&lease.resource)?.as_ref() != Some(&lease)
            || lease.expires_at_ms <= s.runtime().clock.trusted_now_ms()?
        {
            return Err("management_automation_lease_or_binding_stale".into());
        }
        if let Action::Submit { plan_id } = &pending.action {
            // A manual recovery may have established state after the pending
            // child was retained. Leave it pending for the lookup branch above.
            if submission::record(s, &read_owned(s, plan_id)?)?.is_some() {
                return Err("management_automation_existing_submission_lookup_required".into());
            }
        }
        if (b.config.calendar_rollover.is_some() || b.config.reversal_evaluation.is_some())
            && (calendar_action(&pending.action) || observation_action(&pending.action))
        {
            let work = state
                .work
                .get(state.cursor)
                .cloned()
                .ok_or("management_automation_work_missing")?;
            if work.kind == Kind::Position {
                let (_, position) = load(s, &work.id)?;
                let financial_pending = !position.reservations.is_empty();
                let outside_session = if b.config.calendar_rollover.is_some()
                    && observation_action(&pending.action)
                    && !financial_pending
                {
                    effect_window(s, &state, &work.id)?.is_none_or(|window| {
                        window.status != crate::service::management_windows::Status::Active
                    })
                } else {
                    false
                };
                if (financial_pending && !matches!(&pending.action, Action::Advance {}))
                    || outside_session
                {
                    defer_pending(&mut state, &work.id)?;
                    save(s, &b, old, &mut state, None)?;
                    return Ok(summary(&state));
                }
            }
        }
        if matches!(pending.action, Action::Reversal { .. }) && b.config.calendar_rollover.is_none()
        {
            let work = state
                .work
                .get(state.cursor)
                .ok_or("management_automation_work_missing")?;
            let (_, position) = load(s, &work.id)?;
            execution_window(
                s,
                &read_owned(s, &position.entry_plan_id)?,
                &state.occurrence_id,
            )?;
        }
        let phase = match &pending.action {
            Action::Advance {} => "manager.child.advance",
            Action::Submit { .. } => "manager.child.submit",
            Action::Collect { .. } => "manager.child.collect",
            Action::CollectCalendar {} | Action::PrepareWindow {} => "manager.child.calendar",
            _ => "manager.child.other",
        };
        let response = measured(phase, || {
            s.handle_http("POST", &pending.path, pending.body.clone())
        });
        // Losing the fence leaves the original pending child for safe recovery.
        if current_owner(s)? != b.owner
            || s.runtime().leases.current(&lease.resource)?.as_ref() != Some(&lease)
            || lease.expires_at_ms <= s.runtime().clock.trusted_now_ms()?
        {
            return Err("management_automation_lease_stale".into());
        }
        let response_value = json!({"status":response.status,"body":response.body});
        if serde_json::to_vec(&response_value)
            .map_err(|_| "management_automation_response_invalid")?
            .len()
            > MAX_BYTES
        {
            return Err("management_automation_response_too_large".into());
        }
        let after = s.runtime().clock.trusted_now_ms()?;
        if response.status == 200 {
            match apply_response(s, &b, &mut state, &pending, &response) {
                Ok(()) => {
                    state.failures = 0;
                    state.error = None;
                    state.next_due_ms = after;
                }
                Err(error) if matches!(pending.action, Action::Reversal { .. }) => {
                    retain_reversal_child(
                        &mut state,
                        after,
                        b.config.cadence_ms,
                        format!("management_reversal_outcome_seal_required:{error}"),
                    );
                    let observed =
                        json!({"retryRetainedOriginalChild":true,"observed":response_value});
                    save(s, &b, old, &mut state, Some((&pending, observed)))?;
                    return Ok(summary(&state));
                }
                Err(error) => fail_work(&mut state, after, b.config.cadence_ms, error),
            }
        } else {
            let error = crate::service::service_error_code(&response.body)
                .unwrap_or_else(|| "management_automation_child_failed".into());
            if matches!(pending.action, Action::Reversal { .. })
                && reversal::unknown_response(&response)
            {
                retain_reversal_child(
                    &mut state,
                    after,
                    b.config.cadence_ms,
                    format!("management_reversal_child_outcome_unknown:{error}"),
                );
                let observed = json!({"retryRetainedOriginalChild":true,"observed":response_value});
                save(s, &b, old, &mut state, Some((&pending, observed)))?;
                return Ok(summary(&state));
            }
            // Known terminal refusals are journaled; unknown new observations
            // above retain the original key instead of reacquiring evidence.
            fail_work(&mut state, after, b.config.cadence_ms, error);
        }
        schedule_round_end(&mut state, after, b.config.cadence_ms);
        save(s, &b, old, &mut state, Some((&pending, response_value)))?;
        Ok(summary(&state))
    })();
    let _ = s.runtime().leases.release(&lease);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    fn config() -> Config {
        Config {
            reversal_evaluation: None,
            calendar_rollover: None,
            cadence_ms: 1000,
            max_positions_per_round: 2,
            source_inputs: ROLES
                .into_iter()
                .map(|role| {
                    (
                        role.into(),
                        BTreeMap::from([(
                            "instrument_id".into(),
                            typed_cycle::InputValue::TradeInstrumentId {},
                        )]),
                    )
                })
                .collect(),
        }
    }
    #[test]
    fn management_inputs_are_explicit_closed_and_materialized_for_the_owned_instrument() {
        let c = config();
        c.validate().unwrap();
        assert_eq!(
            c.input("pricing_quote", "owned.position").unwrap(),
            json!({"instrument_id":"owned.position"})
        );
        let mut v = value(&c).unwrap();
        v["unknown"] = json!(true);
        assert!(serde_json::from_value::<Config>(v).is_err());
        let mut c = config();
        c.cadence_ms = 999;
        assert!(c.validate().is_err());
        c = config();
        c.max_positions_per_round = 33;
        assert!(c.validate().is_err());
        c = config();
        c.source_inputs.remove("account_data");
        assert!(c.validate().is_err());
        c = config();
        c.source_inputs.get_mut("pricing_quote").unwrap().insert(
            "credentials".into(),
            typed_cycle::InputValue::Literal {
                value: json!("forbidden"),
            },
        );
        assert!(c.validate().is_err());
    }
    #[test]
    fn bounded_rounds_rotate_across_every_owned_position() {
        let ids = vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()];
        let mut after = None;
        let mut seen = BTreeSet::new();
        for _ in 0..3 {
            let batch = rotate(&ids, after.as_deref(), 2);
            assert_eq!(batch.len(), 2);
            after = batch.last().cloned();
            seen.extend(batch);
        }
        assert_eq!(seen, ids.into_iter().collect());
    }
    fn rollover() -> CalendarRollover {
        serde_json::from_value(json!({"schemaVersion":"tradeassembly.management_calendar_rollover.v1","triggerRef":"owned_execution","refreshIntervalMs":60000,"observationPolicy":"execution_session_only","sourceInputs":{
            "symbol":{"kind":"trade_instrument_id"},"from":{"kind":"calendar_date","offsetDays":-1},"to":{"kind":"calendar_date","offsetDays":1},"feed":{"kind":"literal","value":"explicit_primary_sessions"}
        }})).unwrap()
    }
    fn ms(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .timestamp_millis()
    }
    fn legacy_state() -> State {
        serde_json::from_value(json!({"schemaVersion":"tradeassembly.management_automation.v1","activationId":"activation","ownerSha256":"owner","configSha256":"config","coreRevision":"revision","occurrenceId":"occurrence_original","round":1,"generation":0,"childSequence":0,"nextDueMs":1,"positionCursor":null,"work":[],"cursor":0,"pending":null,"error":null,"failures":0,"stateSha256":""})).unwrap()
    }
    #[test]
    fn expected_reversal_waits_do_not_backoff_or_change_retained_evidence() {
        let mut state = legacy_state();
        state.error = Some("previous_real_failure".into());
        state.failures = 6;
        // Authored scheduling state only, not an evaluation/provider receipt.
        state.reversal_heads.insert(
            "owned-position".into(),
            serde_json::from_value(json!({"streams":{},"pendingCaptures":{},
                "planRevisionId":"retained-plan","runId":"retained-run",
                "cursorMs":1_700_000_000_123_i64,"checkpointSha256":"a".repeat(64),
                "signalReceiptSha256":null,"waitingForInput":false}))
            .unwrap(),
        );
        let retained_heads = state.reversal_heads.clone();
        let mut now = 1_700_000_000_123;
        let boundary = now + 60_000;
        for _ in 0..10 {
            // The prior item finished the round. advance clears this marker
            // when constructing the next round, before rescanning positions.
            state.round_observation_wake_at_ms = None;
            state.work = vec![Work::new(Kind::Position, "owned-position".into())];
            state.cursor = 0;
            apply_planned_child(
                &mut state,
                now,
                1000,
                Ok(PlannedChild::Wait(reversal::ObservationWait::WarmingUp {
                    next_completed_at_ms: boundary,
                })),
            );
            assert_eq!(state.failures, 0);
            assert!(state.error.is_none());
            assert!(state.pending.is_none());
            assert_eq!(state.cursor, 1);
            assert_eq!(state.next_due_ms, now + 1000);
            assert_eq!(state.reversal_heads, retained_heads);
            now = state.next_due_ms;
        }
        // A large configured cadence must not postpone a known imminent bar
        // by a whole round after the last position waits.
        state.round_observation_wake_at_ms = None;
        apply_planned_child(
            &mut state,
            boundary - 1,
            60_000,
            Ok(PlannedChild::Wait(reversal::ObservationWait::Input {
                next_completed_at_ms: boundary,
            })),
        );
        assert_eq!(state.next_due_ms, boundary);
        let restored: State = serde_json::from_value(value(&state).unwrap()).unwrap();
        assert_eq!(restored.next_due_ms, boundary);
        assert_eq!(restored.reversal_heads, retained_heads);
        assert_eq!(restored.failures, 0);
    }
    #[test]
    fn real_reversal_errors_keep_failure_backoff_including_legacy_wait_strings() {
        for code in [
            "managed_reversal_not_current",
            "management_reversal_stream_input_template_changed",
            "management_reversal_grid_invalid",
            "management_reversal_warming_up",
            "management_reversal_waiting_for_input",
        ] {
            let mut state = legacy_state();
            state.work = vec![Work::new(Kind::Position, "owned-position".into())];
            apply_planned_child(&mut state, 1000, 1000, Err(code.into()));
            assert_eq!(state.error.as_deref(), Some(code));
            assert_eq!(state.failures, 1);
            assert_eq!(state.next_due_ms, 3000);
            assert!(state.pending.is_none());
        }
    }

    #[test]
    fn earliest_reversal_wait_survives_later_work_and_service_recreation() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory
            .path()
            .join("multi-work-wake.sqlite")
            .to_string_lossy()
            .into_owned();
        let service = TradeAssemblyService::test_local(&db);
        let binding = Binding {
            activation: "activation".into(),
            config: config(),
            config_hash: "config".into(),
            owner: "owner".into(),
            revision: "revision".into(),
        };
        let mut state = legacy_state();
        state.work = vec![
            Work::new(Kind::Position, "waiting-position".into()),
            Work::new(Kind::Position, "later-position".into()),
        ];
        apply_planned_child(
            &mut state,
            1000,
            60_000,
            Ok(PlannedChild::Wait(reversal::ObservationWait::WarmingUp {
                next_completed_at_ms: 1500,
            })),
        );
        assert_eq!(state.cursor, 1);
        assert!(
            state.next_due_ms <= 1000,
            "remaining work must be immediately due"
        );
        save(&service, &binding, None, &mut state, None).unwrap();
        let restarted = TradeAssemblyService::test_local(&db);
        let (raw, mut restored) = load_state(&restarted, &binding).unwrap().unwrap();
        assert!(restored.next_due_ms <= 1000);
        // Completing the remaining position before the boundary must retain
        // that future wake instead of waiting another full cadence.
        apply_planned_child(&mut restored, 1100, 60_000, Ok(PlannedChild::Complete));
        assert_eq!(restored.cursor, 2);
        assert_eq!(restored.next_due_ms, 1500);
        assert_eq!(value(&restored).unwrap()["roundObservationWakeAtMs"], 1500);
        assert!(restored.error.is_none());
        assert_eq!(restored.failures, 0);
        assert!(restored.pending.is_none());
        save(&restarted, &binding, Some(raw), &mut restored, None).unwrap();
        let second_restart = TradeAssemblyService::test_local(&db);
        let (_, restored) = load_state(&second_restart, &binding).unwrap().unwrap();
        assert_eq!(restored.next_due_ms, 1500);
        assert_eq!(value(&restored).unwrap()["roundObservationWakeAtMs"], 1500);
        assert!(second_restart
            .runtime()
            .storage
            .list_json(CHILDREN)
            .unwrap()
            .is_empty());
        assert!(second_restart
            .runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn later_successful_child_and_later_wait_keep_the_earliest_round_wake() {
        let mut state = legacy_state();
        state.work = (0..3)
            .map(|i| Work::new(Kind::Position, format!("position-{i}")))
            .collect();
        apply_planned_child(
            &mut state,
            1000,
            60_000,
            Ok(PlannedChild::Wait(reversal::ObservationWait::WarmingUp {
                next_completed_at_ms: 1500,
            })),
        );
        apply_planned_child(
            &mut state,
            1100,
            60_000,
            Ok(PlannedChild::Wait(reversal::ObservationWait::Input {
                next_completed_at_ms: 2000,
            })),
        );
        assert_eq!(state.round_observation_wake_at_ms, Some(1500));
        assert!(
            state.next_due_ms <= 1100,
            "a second wait must not delay the last item"
        );
        // Model exactly the successful-child assignments in advance: after
        // apply_response completes the last item, success clears the error and
        // writes next_due_ms = after before the shared round-end scheduling.
        complete_work(&mut state);
        state.error = None;
        state.failures = 0;
        state.next_due_ms = 1200;
        schedule_round_end(&mut state, 1200, 60_000);
        assert_eq!(state.next_due_ms, 1500);
        state.next_due_ms = 1800;
        schedule_round_end(&mut state, 1800, 60_000);
        assert_eq!(
            state.next_due_ms, 1800,
            "an overdue wake is immediately due"
        );
    }

    #[test]
    fn earlier_round_wake_never_shortens_real_error_or_unknown_child_backoff() {
        let mut state = legacy_state();
        state.work = vec![
            Work::new(Kind::Position, "waiting-position".into()),
            Work::new(Kind::Position, "failed-position".into()),
        ];
        apply_planned_child(
            &mut state,
            1000,
            60_000,
            Ok(PlannedChild::Wait(reversal::ObservationWait::WarmingUp {
                next_completed_at_ms: 1500,
            })),
        );
        apply_planned_child(
            &mut state,
            1500,
            60_000,
            Err("managed_reversal_not_current".into()),
        );
        assert_eq!(state.next_due_ms, 61_500);
        assert_eq!(state.round_observation_wake_at_ms, Some(1500));
        assert_eq!(state.error.as_deref(), Some("managed_reversal_not_current"));
        assert_eq!(state.failures, 1);
        // Unknown outcomes retain their exact original child and backoff too.
        state.cursor = 1;
        let original = child(
            &mut state,
            "/retained-test-child",
            json!({"original":true}),
            Action::Reversal {
                step: reversal::Action::Evaluate {},
            },
        )
        .unwrap();
        state.pending = Some(original.clone());
        retain_reversal_child(
            &mut state,
            1600,
            1000,
            "original_child_outcome_unknown".into(),
        );
        let deadline = state.next_due_ms;
        schedule_round_end(&mut state, 1600, 1000);
        assert_eq!(state.next_due_ms, deadline);
        assert_eq!(state.pending.as_ref(), Some(&original));
        assert_eq!(state.round_observation_wake_at_ms, Some(1500));
    }

    #[test]
    fn absent_rollover_preserves_legacy_config_bytes_and_state_hash() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Legacy<'a> {
            cadence_ms: i64,
            max_positions_per_round: usize,
            source_inputs: &'a BTreeMap<String, BTreeMap<String, typed_cycle::InputValue>>,
        }
        let c = config();
        let old = Legacy {
            cadence_ms: c.cadence_ms,
            max_positions_per_round: c.max_positions_per_round,
            source_inputs: &c.source_inputs,
        };
        assert_eq!(
            serde_json::to_vec(&c).unwrap(),
            serde_json::to_vec(&old).unwrap()
        );
        let original = serde_json::to_value(&old).unwrap();
        let decoded: Config = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(hash(&decoded).unwrap(), hash(&original).unwrap());
        assert!(decoded.calendar_rollover.is_none());
        assert!(decoded.reversal_evaluation.is_none());
        let state = legacy_state();
        let mut old = serde_json::to_value(&state).unwrap();
        assert!(old.get("windowHeads").is_none());
        assert!(old.get("reversalHeads").is_none());
        assert!(old.get("roundObservationWakeAtMs").is_none());
        assert!(state.round_observation_wake_at_ms.is_none());
        old["stateSha256"] = json!("");
        assert_eq!(state_hash(&state).unwrap(), hash(&old).unwrap());
        let decoded: State = serde_json::from_value(old.clone()).unwrap();
        assert!(decoded.window_heads.is_empty());
        assert_eq!(serde_json::to_value(&decoded).unwrap(), old);
    }
    #[test]
    fn scheduling_state_refreshes_retained_work_due_and_rejects_changed_config_or_window_head() {
        let (_dir, s, _, _) = crate::service::data_plans::tests::fixture_service(None);
        // Authored scheduler state only; no synthetic positions or financial
        // receipts. Pending recovery must win without scanning for new lots.
        let job: session_dispatch::Job = serde_json::from_value(json!({
            "schemaVersion":"tradeassembly.session_dispatch.v1","jobId":"advisory-job",
            "request":{"occurrenceId":"original","scope":{"strategyId":"s","strategyVersionId":"v","strategySpecHash":"h"},
                "capabilityGraphRevision":"g","mode":"paper","subjects":{"kind":"all_selected","maxMembers":1},
                "eventStartMs":86400000,"eventEndMs":172800000,"intervalMs":86400000,"maxFramesPerStep":1,
                "sourceCapturesBySubject":{}},
            "coreRevision":"revision","ownerSha256":"owner","scheduleSha256":"schedule",
            "createdAtMs":1,"startedAtMs":null,"status":"complete","paused":false,"generation":0,
            "attempts":0,"nextAttemptAtMs":0,"error":null,"recordSha256":"","fencingToken":0,
            "queueMessageId":null,"pending":null,"results":{},"subjects":null
        })).unwrap();
        let mut binding = Binding {
            activation: "activation".into(),
            config: config(),
            config_hash: "config".into(),
            owner: "owner".into(),
            revision: "revision".into(),
        };
        let mut state = legacy_state();
        state
            .work
            .push(Work::new(Kind::Position, "retained-position".into()));
        let retained = child(
            &mut state,
            RECONCILE,
            json!({"original":"unchanged"}),
            Action::Reconcile {},
        )
        .unwrap();
        state.pending = Some(retained);
        state.next_due_ms = 1234;
        save(&s, &binding, None, &mut state, None).unwrap();
        let first = bound_scheduling_state(&s, &binding, &job, 1000).unwrap();
        assert_eq!(
            first,
            SchedulingState {
                has_work: true,
                next_due_ms: 1234
            }
        );
        let (old, _) = load_state(&s, &binding).unwrap().unwrap();
        state.pending = None;
        state.cursor = state.work.len();
        state.next_due_ms = 5678;
        save(&s, &binding, Some(old), &mut state, None).unwrap();
        assert_eq!(
            bound_scheduling_state(&s, &binding, &job, 2000).unwrap(),
            SchedulingState {
                has_work: false,
                next_due_ms: 5678
            }
        );
        assert_eq!(first.next_due_ms, 1234);
        binding.config_hash = "changed".into();
        assert_eq!(
            bound_scheduling_state(&s, &binding, &job, 2000).unwrap_err(),
            "management_automation_state_invalid"
        );
        binding.config_hash = "config".into();
        let (old, _) = load_state(&s, &binding).unwrap().unwrap();
        state.cursor = 0;
        state.work[0].window_id = Some(format!("management_window_{}", "a".repeat(64)));
        save(&s, &binding, Some(old), &mut state, None).unwrap();
        assert_eq!(
            bound_scheduling_state(&s, &binding, &job, 2000).unwrap_err(),
            "management_automation_state_invalid"
        );
        assert!(s.runtime().storage.list_json(CHILDREN).unwrap().is_empty());
        assert!(s
            .runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reversal_unknown_child_restarts_with_exact_original_bytes_key_and_no_cursor_advance() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory
            .path()
            .join("reversal-retained.sqlite")
            .to_string_lossy()
            .into_owned();
        let service = TradeAssemblyService::test_local(&db);
        let binding = Binding {
            activation: "activation".into(),
            config: config(),
            config_hash: "config".into(),
            owner: "owner".into(),
            revision: "revision".into(),
        };
        let mut state = legacy_state();
        state.work = vec![Work::new(Kind::Position, "owned-position".into())];
        let pending = child(
            &mut state,
            "/product/strategies/managed-data-runs/step",
            json!({"scope":{"strategyId":"s"},"managedStep":{"step":{"runId":"retained-run",
                "window":{"eventStartMs":10,"eventEndMs":20,"asOfMs":21},
                "expectedCheckpointSha256":"a".repeat(64)}}}),
            Action::Reversal {
                step: reversal::Action::Evaluate {},
            },
        )
        .unwrap();
        state.pending = Some(pending.clone());
        save(&service, &binding, None, &mut state, None).unwrap();
        let restarted = TradeAssemblyService::test_local(&db);
        let (raw, mut loaded) = load_state(&restarted, &binding).unwrap().unwrap();
        assert_eq!(loaded.pending.as_ref(), Some(&pending));
        assert_eq!(loaded.cursor, 0);
        retain_reversal_child(
            &mut loaded,
            100,
            1000,
            "management_reversal_child_outcome_unknown".into(),
        );
        save(&restarted,&binding,Some(raw),&mut loaded,Some((&pending,
            json!({"retryRetainedOriginalChild":true,"observed":{"status":503,"body":{"error":{"code":"outcome_uncertain"}}}})))).unwrap();
        let restarted = TradeAssemblyService::test_local(&db);
        let (_, again) = load_state(&restarted, &binding).unwrap().unwrap();
        assert_eq!(again.pending.as_ref(), Some(&pending));
        assert_eq!(again.cursor, 0);
        assert_eq!(again.child_sequence, state.child_sequence);
        assert_eq!(
            again.pending.unwrap().body["managedStep"]["step"]["expectedCheckpointSha256"],
            "a".repeat(64)
        );
        let observed = restarted.runtime().storage.list_json(CHILDREN).unwrap();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].1["child"], value(&pending).unwrap());
        assert_eq!(observed[0].1["response"]["observed"]["status"], 503);
    }

    #[test]
    fn rollover_config_is_closed_bounded_and_session_only() {
        let good = rollover();
        good.validate().unwrap();
        let mut c = good.clone();
        c.observation_policy = ObservationPolicy::ContinuousObservation;
        assert_eq!(
            c.validate().unwrap_err(),
            "management_automation_continuous_observation_unsupported"
        );
        for interval in [999, 3600001] {
            let mut c = good.clone();
            c.refresh_interval_ms = interval;
            assert!(c.validate().is_err());
        }
        let mut c = good.clone();
        c.schema_version = "tradeassembly.management_calendar_rollover.v2".into();
        assert!(c.validate().is_err());
        let mut c = good.clone();
        c.source_inputs.insert(
            "credentials".into(),
            CalendarInput::Existing(typed_cycle::InputValue::Literal { value: json!("no") }),
        );
        assert!(c.validate().is_err());
        let mut c = good.clone();
        c.source_inputs.insert(
            "from".into(),
            CalendarInput::Date(CalendarDate::CalendarDate { offset_days: 367 }),
        );
        assert!(c.validate().is_err());
        let mut body = serde_json::to_value(&good).unwrap();
        body["marketOpen"] = json!("09:30");
        assert!(serde_json::from_value::<CalendarRollover>(body).is_err());
        assert!(serde_json::from_value::<CalendarInput>(
            json!({"kind":"calendar_date","offsetDays":0,"timezone":"UTC"})
        )
        .is_err());
        // Date materialization is calendar-only; old account/metadata/quote inputs stay closed.
        assert!(serde_json::from_value::<typed_cycle::InputValue>(
            json!({"kind":"calendar_date","offsetDays":0})
        )
        .is_err());
    }
    #[test]
    fn calendar_dates_use_published_zone_and_no_market_time_assumptions() {
        let c = rollover();
        let at = ms("2026-03-09T01:00:00Z");
        let new_york = c.input("OWNED", "America/New_York", at).unwrap();
        assert_eq!(
            new_york,
            json!({"symbol":"OWNED","from":"2026-03-07","to":"2026-03-09","feed":"explicit_primary_sessions"})
        );
        let utc = c.input("OWNED", "Etc/UTC", at).unwrap();
        assert_eq!(utc["from"], "2026-03-08");
        assert_eq!(utc["to"], "2026-03-10");
        let after_dst = c
            .input("OWNED", "America/New_York", ms("2026-03-09T05:00:00Z"))
            .unwrap();
        assert_eq!(after_dst["from"], "2026-03-08");
        assert_eq!(
            c.input("OWNED", "unsupported-provider-label", at)
                .unwrap_err(),
            "management_automation_calendar_timezone_unsupported"
        );
        assert!(c.input("OWNED", "Etc/UTC", 0).is_err());
    }
    #[test]
    fn retained_calendar_child_survives_durable_reload_without_new_key_or_new_date() {
        let (_dir, s, _, _) = crate::service::data_plans::tests::fixture_service(None);
        let config = config();
        let binding = Binding {
            activation: "activation".into(),
            config,
            config_hash: "config".into(),
            owner: "owner".into(),
            revision: "revision".into(),
        };
        let mut state = legacy_state();
        state
            .work
            .push(Work::new(Kind::Position, "position_owned".into()));
        let input = rollover()
            .input("OWNED", "America/New_York", ms("2026-03-09T01:00:00Z"))
            .unwrap();
        let retained = child(
            &mut state,
            "/product/strategies/data-collections/create",
            json!({"collection":{"input":input,"acquisitionNode":"published_execution_calendar"}}),
            Action::CollectCalendar {},
        )
        .unwrap();
        state.pending = Some(retained.clone());
        let before = s
            .runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap();
        save(&s, &binding, None, &mut state, None).unwrap();
        let restored = TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone());
        let (_, loaded) = load_state(&restored, &binding).unwrap().unwrap();
        assert_eq!(loaded.pending, Some(retained.clone()));
        assert_eq!(loaded.child_sequence, state.child_sequence);
        assert_ne!(
            rollover()
                .input("OWNED", "America/New_York", ms("2026-03-10T05:00:00Z"))
                .unwrap(),
            retained.body["collection"]["input"]
        );
        assert_eq!(
            s.runtime()
                .storage
                .list_json("plugin_operation_receipts")
                .unwrap(),
            before
        );
    }
    #[test]
    fn deferred_observation_and_calendar_requests_survive_reload_without_financial_retargeting() {
        let mut state = legacy_state();
        state
            .work
            .push(Work::new(Kind::Position, "position_owned".into()));
        let first = child(
            &mut state,
            "/product/strategies/data-collections/create",
            json!({"collection":{"input":{"symbol":"OWNED"}}}),
            Action::Collect {
                role: "pricing_quote".into(),
            },
        )
        .unwrap();
        state.pending = Some(first.clone());
        defer_pending(&mut state, "position_owned").unwrap();
        let second=child(&mut state,crate::service::management_windows::PREPARE,json!({"managementWindow":{"positionId":"position_owned","executionCalendarCollectionId":"original_collection"}}),Action::PrepareWindow{}).unwrap();
        state.pending = Some(second.clone());
        defer_pending(&mut state, "position_owned").unwrap();
        let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(
            restored.window_heads["position_owned"].deferred_children,
            vec![first, second]
        );
        assert!(restored.pending.is_none());
        assert_eq!(restored.occurrence_id, "occurrence_original");
        let financial = child(
            &mut state,
            SUBMIT,
            json!({"submission":{"planId":"original_close"}}),
            Action::Submit {
                plan_id: "original_close".into(),
            },
        )
        .unwrap();
        state.pending = Some(financial.clone());
        assert_eq!(
            defer_pending(&mut state, "position_owned").unwrap_err(),
            "management_automation_financial_child_cannot_be_deferred"
        );
        assert_eq!(state.pending, Some(financial));
        assert!(!calendar_action(&Action::Reconcile {}));
        assert!(!observation_action(&Action::Settle {}));
    }
    #[test]
    fn refresh_deadline_never_outlives_explicit_policy_freshness_or_execution_close() {
        assert_eq!(refresh_deadline(1000, 60000, 100000, Some(100000)), 61000);
        assert_eq!(refresh_deadline(1000, 60000, 2000, Some(100000)), 2000);
        assert_eq!(refresh_deadline(1000, 60000, 100000, Some(3000)), 3000);
        assert_eq!(refresh_deadline(1000, 60000, 2000, None), 2000);
    }
    #[test]
    fn retained_prepare_window_binds_original_body_after_transient_work_is_recreated() {
        let mut state = legacy_state();
        let command = crate::service::management_windows::Command {
            scope: crate::service::data_plans::PlanScope {
                strategy_id: "s".into(),
                strategy_version_id: "v".into(),
                strategy_spec_hash: "a".repeat(64),
            },
            position_id: "position_owned".into(),
            trigger_ref: "owned_execution".into(),
            execution_calendar_collection_id: "original_calendar_collection".into(),
        };
        let retained = child(
            &mut state,
            crate::service::management_windows::PREPARE,
            json!({"managementWindow":command}),
            Action::PrepareWindow {},
        )
        .unwrap();
        let restored: Child =
            serde_json::from_slice(&serde_json::to_vec(&retained).unwrap()).unwrap();
        let new_work = Work::new(Kind::Position, "position_owned".into());
        assert!(new_work.collections.is_empty());
        validate_retained_window_command(&restored, &command).unwrap();
        let mut wrong = command;
        wrong.execution_calendar_collection_id = "reacquired_different_collection".into();
        assert_eq!(
            validate_retained_window_command(&restored, &wrong).unwrap_err(),
            "management_automation_window_result_binding_invalid"
        );
        assert_eq!(restored.key, retained.key);
        assert_eq!(restored.body, retained.body);
    }

    #[test]
    fn absent_round_window_preserves_legacy_work_bytes_and_state_hash() {
        let old = json!({"kind":"position","id":"position_owned","collections":{},
            "submission":null,"reconciled":false});
        let decoded: Work = serde_json::from_value(old.clone()).unwrap();
        assert!(decoded.window_id.is_none());
        assert_eq!(value(&decoded).unwrap(), old);
        assert_eq!(hash(&decoded).unwrap(), hash(&old).unwrap());
        let mut state = legacy_state();
        state.work.push(decoded);
        let mut original = value(&state).unwrap();
        original["stateSha256"] = json!("");
        assert!(original["work"][0].get("windowId").is_none());
        assert_eq!(state_hash(&state).unwrap(), hash(&original).unwrap());
    }
    fn admitted_round_state() -> State {
        let mut state = legacy_state();
        let id = format!("management_window_{}", "a".repeat(64));
        let mut work = Work::new(Kind::Position, "position_owned".into());
        work.window_id = Some(id.clone());
        state.work.push(work);
        state.window_heads.insert(
            "position_owned".into(),
            WindowHead {
                window_id: Some(id),
                refresh_at_ms: 1001,
                deferred_children: Vec::new(),
            },
        );
        state
    }
    #[test]
    fn prepared_round_survives_restart_and_due_poll_still_runs_actual_window_currentness() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("round-window.sqlite");
        let db = db.to_str().unwrap();
        let service = TradeAssemblyService::test_local(db);
        let binding = Binding {
            activation: "activation".into(),
            config: config(),
            config_hash: "config".into(),
            owner: "owner".into(),
            revision: "revision".into(),
        };
        let mut state = admitted_round_state();
        let retained = child(
            &mut state,
            "/product/strategies/data-collections/create",
            json!({"collection":{"input":{"symbol":"OWNED"}}}),
            Action::Collect {
                role: "account_data".into(),
            },
        )
        .unwrap();
        state.pending = Some(retained.clone());
        save(&service, &binding, None, &mut state, None).unwrap();
        let restarted = TradeAssemblyService::test_local(db);
        let (_, mut loaded) = load_state(&restarted, &binding).unwrap().unwrap();
        assert_eq!(loaded.pending.as_ref(), Some(&retained));
        assert!(loaded.work[0].collections.is_empty());
        assert!(loaded.work[0].admitted_window_matches(&loaded.window_heads));
        // The periodic poll expired during preparation. The admitted round must
        // still call the actual window validator; an unbacked receipt cannot pass.
        assert!(head_current(&restarted, &loaded, "position_owned", 2000)
            .unwrap()
            .is_none());
        assert_eq!(
            work_window_current(&restarted, &loaded, &loaded.work[0], 2000).unwrap_err(),
            "management_window_not_available"
        );
        let next = Work::new(Kind::Position, "position_owned".into());
        assert!(next.window_id.is_none());
        assert!(work_window_current(&restarted, &loaded, &next, 2000)
            .unwrap()
            .is_none());
        // Unknown/deferred observation bytes survive this admission; they are
        // still replayed through the original effect-window checks.
        defer_pending(&mut loaded, "position_owned").unwrap();
        assert_eq!(
            loaded.window_heads["position_owned"].deferred_children,
            vec![retained]
        );
        assert!(loaded.work[0].admitted_window_matches(&loaded.window_heads));
        assert_eq!(
            work_window_current(&restarted, &loaded, &loaded.work[0], 2000).unwrap_err(),
            "management_window_not_available"
        );
    }
    #[test]
    fn persisted_round_window_rejects_entry_wrong_head_and_invalid_ids() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("round-window-invalid.sqlite");
        let service = TradeAssemblyService::test_local(db.to_str().unwrap());
        let binding = Binding {
            activation: "activation".into(),
            config: config(),
            config_hash: "config".into(),
            owner: "owner".into(),
            revision: "revision".into(),
        };
        let mut state = admitted_round_state();
        save(&service, &binding, None, &mut state, None).unwrap();
        let (mut old, _) = load_state(&service, &binding).unwrap().unwrap();
        for (index, changed) in [
            "entry",
            "mismatch",
            "invalid",
            "missing_head",
            "wrong_position",
        ]
        .into_iter()
        .enumerate()
        {
            let mut invalid = state.clone();
            invalid.generation += index as u64;
            match changed {
                "entry" => invalid.work[0].kind = Kind::Entry,
                "mismatch" => {
                    invalid.work[0].window_id =
                        Some(format!("management_window_{}", "b".repeat(64)))
                }
                "invalid" => {
                    let id = format!("management_window_{}", "!".repeat(64));
                    invalid.work[0].window_id = Some(id.clone());
                    invalid
                        .window_heads
                        .get_mut("position_owned")
                        .unwrap()
                        .window_id = Some(id);
                }
                "missing_head" => {
                    invalid.window_heads.clear();
                }
                _ => invalid.work[0].id = "position_other".into(),
            }
            save(&service, &binding, Some(old), &mut invalid, None).unwrap();
            old = value(&invalid).unwrap();
            assert_eq!(
                load_state(&service, &binding).err().unwrap(),
                "management_automation_state_invalid",
                "{changed}"
            );
        }
    }
}
