//! Opted-in, activation-scoped management through ordinary authorized HTTP children.
//! A retained request is persisted before invocation, and its exact response is
//! journaled atomically with progress. Existing submission state only permits
//! lookup/settlement, never another submit. Calendar rollover and automatic
//! reversal-signal evaluation are deliberately not qualified here.
use super::*;
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
    pub cadence_ms: i64,
    pub max_positions_per_round: usize,
    pub source_inputs: BTreeMap<String, BTreeMap<String, typed_cycle::InputValue>>,
}
impl Config {
    pub(crate) fn validate(&self) -> Result<(), String> {
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
}
impl Work {
    fn new(kind: Kind, id: String) -> Self {
        Self {
            kind,
            id,
            collections: BTreeMap::new(),
            submission: None,
            reconciled: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
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
struct State {
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
        let child_id = hash(&child.key)?;
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
pub(crate) fn has_work(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<bool, String> {
    let Some(b) = binding(s, job)? else {
        return Ok(false);
    };
    if load_state(s, &b)?
        .is_some_and(|(_, state)| state.pending.is_some() || state.cursor < state.work.len())
    {
        return Ok(true);
    }
    Ok(!entry_work(s, &b, job)?.is_empty() || !position_ids(s, &b, job)?.is_empty())
}
pub(crate) fn next_due(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<i64, String> {
    let now = s.runtime().clock.trusted_now_ms()?;
    let Some(b) = binding(s, job)? else {
        return Ok(now.saturating_add(60_000));
    };
    Ok(load_state(s, &b)?.map_or(now, |(_, state)| state.next_due_ms))
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
) -> Result<Child, String> {
    child(
        state,
        ADVANCE,
        json!({"scope":plan.command.scope,"advance":{
        "executionOccurrenceId":occurrence,"positionId":position.position_id,
        "accountCollectionId":collections[0],"metadataCollectionId":collections[1],
        "quoteCollectionId":collections[2],"reversalSignalReceiptSha256":null}}),
        Action::Advance {},
    )
}
fn next_child(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
    state: &mut State,
) -> Result<Option<Child>, String> {
    let Some(work) = state.work.get(state.cursor).cloned() else {
        return Ok(None);
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
                return plan_child(state, &plan, SETTLE, Action::Settle {}).map(Some);
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
                .map(Some);
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
            return plan_child(state, &plan, RECONCILE, Action::Reconcile {}).map(Some);
        }
        return Ok(None);
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
                        return plan_child(state, &plan, SETTLE, Action::Settle {}).map(Some);
                    }
                    return Ok(None);
                }
                submission::Phase::Denied => {}
                _ => {
                    return if work.reconciled {
                        Ok(None)
                    } else {
                        plan_child(state, &plan, RECONCILE, Action::Reconcile {}).map(Some)
                    };
                }
            }
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
            .map(Some);
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
        )
        .map(Some);
    }
    if !position
        .quantity
        .checked_subtract(position.closed_quantity)?
        .positive()
    {
        return Ok(None);
    }
    if position.policy.rules.reversal_signal.is_some() {
        return Err("management_automation_reversal_evidence_unsupported".into());
    }
    // A completed occurrence is no permission for the next market session.
    execution_window(s, &entry, &state.occurrence_id)?;
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
                "input":b.config.input(role, &entry.order.instrument.instrument_id)?}}), Action::Collect { role:role.into() }).map(Some);
        }
    }
    let occurrence = state.occurrence_id.clone();
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
    )
    .map(Some)
}
fn complete_work(state: &mut State) {
    state.cursor = (state.cursor + 1).min(state.work.len());
    state.pending = None;
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
fn apply_response(
    s: &TradeAssemblyService,
    b: &Binding,
    state: &mut State,
    child: &Child,
    response: &ServiceResponse,
) -> Result<(), String> {
    let work = state
        .work
        .get_mut(state.cursor)
        .ok_or("management_automation_work_missing")?;
    match &child.action {
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
            state.work = entry_work(s, &b, job)?;
            let selected = rotate(
                &position_ids(s, &b, job)?,
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
            state.occurrence_id = job.request.occurrence_id.clone();
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
            }
        }
        if state.pending.is_none() {
            match next_child(s, &b, job, &mut state) {
                Ok(Some(next)) => state.pending = Some(next),
                Ok(None) => complete_work(&mut state),
                Err(error) => fail_work(&mut state, now, b.config.cadence_ms, error),
            }
            if state.cursor == state.work.len() {
                state.next_due_ms = state
                    .next_due_ms
                    .max(now.saturating_add(b.config.cadence_ms));
            }
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
        let response = s.handle_http("POST", &pending.path, pending.body.clone());
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
                Err(error) => fail_work(&mut state, after, b.config.cadence_ms, error),
            }
        } else {
            let error = crate::service::service_error_code(&response.body)
                .unwrap_or_else(|| "management_automation_child_failed".into());
            // A failed finite child is retained in the journal. The next round
            // gets fresh lookup/read keys; no failed child fabricates evidence.
            fail_work(&mut state, after, b.config.cadence_ms, error);
        }
        if state.cursor == state.work.len() {
            state.next_due_ms = state
                .next_due_ms
                .max(after.saturating_add(b.config.cadence_ms));
        }
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
}
