//! Explicit local-user, keyed typed recomputation. No broker or new source
//! acquisition authority. Original results and evaluation generations are immutable.
use super::{
    backtest_lifecycle, data_evaluation, data_plans, typed_backtest, ServiceResponse,
    TradeAssemblyService,
};
use crate::backtest_contracts::{BacktestResult, BacktestRunManifest};
use crate::domain::BacktestRunState;
use crate::ports::{
    ComparePutOutcome, IdempotencyKey, SideEffectContext, StorageExpectation, StorageWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(super) const START_TOOL: &str = "tradeassembly.backtest.recompute.start";
pub(super) const ADVANCE_TOOL: &str = "tradeassembly.backtest.recompute.advance";
pub(super) const INSPECT_TOOL: &str = "tradeassembly.backtest.recompute.inspect";
const SCHEMA: &str = "tradeassembly.typed_backtest_recompute_command.v1";
const STATES: &str = "typed_backtest_recomputations_v1";
const REQUESTS: &str = "typed_backtest_recompute_requests_v1";
const OUTCOMES: &str = "typed_backtest_recompute_outcomes_v1";
const MAX_BYTES: usize = 2 * 1024 * 1024;
static CLAIM_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Command {
    Start {
        schema_version: String,
        scope: data_plans::PlanScope,
        run_id: String,
        expected_manifest_hash: String,
        expected_result_hash: String,
        idempotency_key: String,
    },
    Advance {
        schema_version: String,
        scope: data_plans::PlanScope,
        run_id: String,
        recomputation_id: String,
        expected_state_sha256: String,
        idempotency_key: String,
    },
}
impl Command {
    fn scope(&self) -> &data_plans::PlanScope {
        match self {
            Self::Start { scope, .. } | Self::Advance { scope, .. } => scope,
        }
    }
    fn run_id(&self) -> &str {
        match self {
            Self::Start { run_id, .. } | Self::Advance { run_id, .. } => run_id,
        }
    }
    fn key(&self) -> &str {
        match self {
            Self::Start {
                idempotency_key, ..
            }
            | Self::Advance {
                idempotency_key, ..
            } => idempotency_key,
        }
    }
    fn validate(&self) -> Result<(), String> {
        let schema = match self {
            Self::Start { schema_version, .. } | Self::Advance { schema_version, .. } => {
                schema_version
            }
        };
        let scope = self.scope();
        if schema != SCHEMA
            || !cli_path_segment(self.run_id())
            || !label(self.key(), 200)
            || !label(&scope.strategy_id, 128)
            || !label(&scope.strategy_version_id, 128)
            || !prefixed(&scope.strategy_spec_hash)
            || match self {
                Self::Start {
                    expected_manifest_hash,
                    expected_result_hash,
                    ..
                } => !prefixed(expected_manifest_hash) || !prefixed(expected_result_hash),
                Self::Advance {
                    recomputation_id,
                    expected_state_sha256,
                    ..
                } => !cli_path_segment(recomputation_id) || !digest(expected_state_sha256),
            }
        {
            return Err("typed_recompute_command_invalid".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Proof {
    pub(super) original_result_hash: String,
    pub(super) semantic_sha256: String,
    pub(super) evaluation_receipt_sha256: Vec<String>,
    pub(super) final_checkpoint_sha256: String,
    pub(super) proof_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Queued,
    Running,
    Blocked,
    Completed,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Error {
    code: String,
    retryable: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    schema_version: String,
    owner_sha256: String,
    scope: data_plans::PlanScope,
    run_id: String,
    recomputation_id: String,
    original_manifest_hash: String,
    original_result_hash: String,
    phase: Phase,
    progress: typed_backtest::Progress,
    pending_key: Option<String>,
    proof: Option<Proof>,
    error: Option<Error>,
    state_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Original {
    command: Command,
    step: Option<data_evaluation::EvaluationStep>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Outcome {
    command_sha256: String,
    status: u16,
    body: Value,
}
struct Subject {
    manifest: BacktestRunManifest,
    expected: BacktestResult,
    owner_sha256: String,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(v).map_err(|_| "typed_recompute_encoding_invalid".into())
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn prefixed(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(digest)
}
fn label(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_graphic())
}
/// Pure CLI/router validation only; this grants no invocation authority.
pub(crate) fn cli_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub(crate) fn cli_command_path(expected_kind: &str, request: &Value) -> Result<String, String> {
    let command = parse(&json!({"request":request}))?;
    match (expected_kind, command) {
        ("start", Command::Start { run_id, .. }) => {
            Ok(format!("/backtests/{run_id}/recomputations"))
        }
        (
            "advance",
            Command::Advance {
                run_id,
                recomputation_id,
                ..
            },
        ) => Ok(format!(
            "/backtests/{run_id}/recomputations/{recomputation_id}/advance"
        )),
        _ => Err("typed_recompute_command_kind_mismatch".into()),
    }
}

fn key(owner: &str, k: &str) -> Result<String, String> {
    hash(&json!([owner, k]))
}
fn encode<T: Serialize>(v: &T) -> Result<Value, String> {
    let value = serde_json::to_value(v).map_err(|_| "typed_recompute_encoding_invalid")?;
    if serde_json::to_vec(&value)
        .map_err(|_| "typed_recompute_encoding_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("typed_recompute_state_bounds_invalid".into());
    }
    Ok(value)
}
fn decode<T: for<'a> Deserialize<'a>>(v: Value) -> Result<T, String> {
    if serde_json::to_vec(&v)
        .map_err(|_| "typed_recompute_encoding_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("typed_recompute_state_bounds_invalid".into());
    }
    serde_json::from_value(v).map_err(|_| "typed_recompute_state_invalid".into())
}
fn ctx(subject: &Subject, k: &str) -> Result<SideEffectContext, String> {
    Ok(SideEffectContext::new(
        subject.manifest.content.authority.clone(),
        IdempotencyKey::new(format!("typed-recompute:{k}"))?,
    ))
}
fn seal(s: &mut State) -> Result<(), String> {
    s.state_sha256.clear();
    s.state_sha256 = hash(s)?;
    Ok(())
}
fn subject(
    s: &TradeAssemblyService,
    run: &str,
    scope: Option<&data_plans::PlanScope>,
) -> Result<Subject, String> {
    typed_backtest::actual_owner(s)?;
    s.require_background_worker_current()?;
    s.require_object("backtest_run", run)
        .map_err(|_| "object_not_available")?;
    let runtime = s.runtime();
    let record = runtime
        .backtests
        .get_run(run)?
        .ok_or("object_not_available")?;
    let manifest = runtime
        .backtests
        .get_manifest(run)?
        .ok_or("object_not_available")?;
    let expected = runtime
        .backtests
        .get_result(run)?
        .ok_or("typed_recompute_original_not_completed")?;
    manifest.verify()?;
    expected.verify()?;
    if record.state != BacktestRunState::Completed
        || record.manifest_hash != manifest.manifest_hash
        || record.result_hash.as_deref() != Some(expected.result_hash.as_str())
        || record.result.as_ref() != Some(&expected)
        || manifest.content.run_id != run
        || expected.content.run_id != run
        || expected.content.manifest_hash != manifest.manifest_hash
    {
        return Err("typed_recompute_original_binding_invalid".into());
    }
    if manifest.content.configuration.typed_evaluation.is_none() {
        return Err("typed_recompute_original_v2_required".into());
    }
    let configured = &manifest.content.configuration.strategy;
    let actual = data_plans::PlanScope {
        strategy_id: configured.strategy_id.clone(),
        strategy_version_id: configured.strategy_version_id.clone(),
        strategy_spec_hash: configured.strategy_spec_hash.clone(),
    };
    if scope.is_some_and(|scope| scope != &actual) {
        return Err("object_not_available".into());
    }
    let owner_sha256 = typed_backtest::owner(s, &manifest)?;
    Ok(Subject {
        manifest,
        expected,
        owner_sha256,
    })
}
fn state(s: &TradeAssemblyService, id: &str, sub: &Subject) -> Result<(Value, State), String> {
    s.require_object("backtest_recomputation", id)
        .map_err(|_| "object_not_available")?;
    let raw = s
        .runtime()
        .storage
        .get_json(STATES, id)?
        .ok_or("object_not_available")?;
    let state: State = decode(raw.clone())?;
    let mut check = state.clone();
    seal(&mut check)?;
    let binding = sub
        .manifest
        .content
        .configuration
        .typed_evaluation
        .as_ref()
        .ok_or("typed_recompute_original_v2_required")?;
    if check != state
        || state.schema_version != "tradeassembly.typed-backtest-recomputation.v1"
        || state.owner_sha256 != sub.owner_sha256
        || state.run_id != sub.manifest.content.run_id
        || state.recomputation_id != id
        || state.original_manifest_hash != sub.manifest.manifest_hash
        || state.original_result_hash != sub.expected.result_hash
        || state.progress.manifest_hash != sub.manifest.manifest_hash
        || state.progress.owner_sha256 != sub.owner_sha256
        || state.progress.evaluation_run_id
            != format!(
                "typed-recompute-{}",
                id.strip_prefix("backtest_recompute_")
                    .ok_or("typed_recompute_state_invalid")?
            )
        || state.progress.steps.len() != state.progress.receipts.len()
        || state.progress.receipts.len() > 256
        || state.progress.cursor_ms < binding.event_start_ms
        || state.progress.cursor_ms > binding.event_end_ms
    {
        return Err("typed_recompute_state_invalid".into());
    }
    Ok((raw, state))
}
fn original(s: &TradeAssemblyService, k: &str, c: &Command) -> Result<Option<Original>, String> {
    s.runtime()
        .storage
        .get_json(REQUESTS, k)?
        .map(|raw| {
            let record: Original = decode(raw)?;
            if record.command != *c {
                return Err("typed_recompute_idempotency_conflict".into());
            }
            Ok(record)
        })
        .transpose()
}
fn outcome(
    s: &TradeAssemblyService,
    k: &str,
    c: &Command,
) -> Result<Option<ServiceResponse>, String> {
    s.runtime()
        .storage
        .get_json(OUTCOMES, k)?
        .map(|raw| {
            let r: Outcome = decode(raw)?;
            if r.command_sha256 != hash(c)? || !matches!(r.status, 200 | 202 | 409) {
                return Err("typed_recompute_outcome_invalid".into());
            }
            Ok(ServiceResponse {
                status: r.status,
                body: r.body,
            })
        })
        .transpose()
}
fn view(s: &TradeAssemblyService, state: &State) -> Result<Value, String> {
    let mut v = json!({"schemaVersion":"tradeassembly.typed_backtest_recompute_status.v1","scope":state.scope,"runId":state.run_id,"recomputationId":state.recomputation_id,"originalManifestHash":state.original_manifest_hash,"originalResultHash":state.original_result_hash,"stateSha256":state.state_sha256,"phase":state.phase,"framesProcessed":0,"generation":state.progress.receipts.len()});
    // Frame counts use the original admitted interval, not elapsed milliseconds.
    let sub = subject(s, &state.run_id, Some(&state.scope))?;
    let b = sub
        .manifest
        .content
        .configuration
        .typed_evaluation
        .as_ref()
        .unwrap();
    v["framesProcessed"] =
        json!((state.progress.cursor_ms - b.event_start_ms) as u64 / b.interval_ms);
    v["framesRequired"] = json!((b.event_end_ms - b.event_start_ms) as u64 / b.interval_ms);
    if let Some(proof) = &state.proof {
        v["proof"] = encode(proof)?;
    }
    if let Some(error) = &state.error {
        v["error"] = encode(error)?;
    }
    if let Some(k) = &state.pending_key {
        let pending: Original = decode(
            s.runtime()
                .storage
                .get_json(REQUESTS, k)?
                .ok_or("typed_recompute_original_request_missing")?,
        )?;
        v["pendingCommand"] = encode(&pending.command)?;
    }
    Ok(v)
}
fn current(
    s: &TradeAssemblyService,
    sub: &Subject,
) -> Result<(Value, crate::historical_data::DatasetSnapshot), String> {
    typed_backtest::owner(s, &sub.manifest)?;
    typed_backtest::validate_configuration(s, &sub.manifest.content.configuration)?;
    let version = backtest_lifecycle::exact_strategy_version(s, &sub.manifest)
        .ok_or("backtest_strategy_version_required")?;
    let snapshot = s
        .runtime()
        .dataset_snapshots
        .get(&sub.manifest.content.configuration.dataset.dataset_id)?
        .ok_or("backtest_dataset_unavailable")?;
    typed_backtest::validate_dataset(s, &sub.manifest.content.configuration, &snapshot)?;
    Ok((version, snapshot))
}
fn batch(
    s: &TradeAssemblyService,
    writes: Vec<StorageWrite>,
    expected: Vec<StorageExpectation>,
) -> Result<(), String> {
    if s.runtime().storage.put_json_batch(&writes, &expected)? != ComparePutOutcome::Updated {
        return Err("typed_recompute_state_conflict".into());
    }
    Ok(())
}
fn start(s: &TradeAssemblyService, c: &Command, sub: &Subject) -> Result<ServiceResponse, String> {
    let Command::Start {
        scope,
        run_id,
        expected_manifest_hash,
        expected_result_hash,
        ..
    } = c
    else {
        return Err("typed_recompute_command_invalid".into());
    };
    if expected_manifest_hash != &sub.manifest.manifest_hash
        || expected_result_hash != &sub.expected.result_hash
    {
        return Err("typed_recompute_original_binding_invalid".into());
    }
    let k = key(&sub.owner_sha256, c.key())?;
    original(s, &k, c)?;
    if let Some(response) = outcome(s, &k, c)? {
        return Ok(response);
    }
    current(s, sub)?;
    let id = format!("backtest_recompute_{k}");
    let b = sub
        .manifest
        .content
        .configuration
        .typed_evaluation
        .as_ref()
        .unwrap();
    let mut state = State {
        schema_version: "tradeassembly.typed-backtest-recomputation.v1".into(),
        owner_sha256: sub.owner_sha256.clone(),
        scope: scope.clone(),
        run_id: run_id.clone(),
        recomputation_id: id.clone(),
        original_manifest_hash: sub.manifest.manifest_hash.clone(),
        original_result_hash: sub.expected.result_hash.clone(),
        phase: Phase::Queued,
        progress: typed_backtest::Progress {
            schema_version: "tradeassembly.typed-backtest-progress.v1".into(),
            owner_sha256: sub.owner_sha256.clone(),
            manifest_hash: sub.manifest.manifest_hash.clone(),
            evaluation_run_id: format!("typed-recompute-{k}"),
            cursor_ms: b.event_start_ms,
            checkpoint_sha256: None,
            pending: None,
            steps: Vec::new(),
            receipts: Vec::new(),
            state_sha256: String::new(),
        },
        pending_key: None,
        proof: None,
        error: None,
        state_sha256: String::new(),
    };
    seal(&mut state)?;
    s.bind_inherited_object("backtest_recomputation", &id, "backtest_run", run_id)
        .map_err(|_| "object_not_available")?;
    let context = ctx(sub, &k)?;
    let body = view(s, &state)?;
    let result = Outcome {
        command_sha256: hash(c)?,
        status: 202,
        body: body.clone(),
    };
    let admission = batch(
        s,
        vec![
            StorageWrite::new(
                REQUESTS,
                &k,
                encode(&Original {
                    command: c.clone(),
                    step: None,
                })?,
                context.clone(),
            ),
            StorageWrite::new(STATES, &id, encode(&state)?, context.clone()),
            StorageWrite::new(OUTCOMES, &k, encode(&result)?, context),
        ],
        vec![
            StorageExpectation::new(REQUESTS, &k, None),
            StorageExpectation::new(STATES, &id, None),
            StorageExpectation::new(OUTCOMES, &k, None),
        ],
    );
    if admission.is_err() {
        // A concurrent Start or an uncertain storage response can have sealed
        // the exact original admission. Recheck the current owner before any
        // recovery read and return only its actual immutable outcome.
        let current_subject = subject(s, run_id, Some(scope))?;
        if current_subject.owner_sha256 != sub.owner_sha256
            || current_subject.manifest.manifest_hash != sub.manifest.manifest_hash
            || current_subject.expected.result_hash != sub.expected.result_hash
        {
            return Err("typed_recompute_original_binding_invalid".into());
        }
        if original(s, &k, c)?.is_some() {
            if let Some(response) = outcome(s, &k, c)? {
                return Ok(response);
            }
        }
        return Err("typed_recompute_start_reconciliation_required".into());
    }
    Ok(ServiceResponse { status: 202, body })
}
fn advance(
    s: &TradeAssemblyService,
    c: &Command,
    sub: &Subject,
) -> Result<ServiceResponse, String> {
    let Command::Advance {
        recomputation_id,
        expected_state_sha256,
        ..
    } = c
    else {
        return Err("typed_recompute_command_invalid".into());
    };
    let k = key(&sub.owner_sha256, c.key())?;
    original(s, &k, c)?;
    // Both the original owner and recomputation binding precede any cache read.
    let (_, initial) = state(s, recomputation_id, sub)?;
    if initial.scope != *c.scope() {
        return Err("object_not_available".into());
    }
    if let Some(response) = outcome(s, &k, c)? {
        return Ok(response);
    }
    let resource = format!("typed-recompute:{recomputation_id}");
    let claim_owner = format!(
        "{}:{}:{}",
        std::process::id(),
        hash(c)?,
        CLAIM_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let Some(lease) = s.runtime().leases.acquire(
        &resource,
        &claim_owner,
        s.runtime().clock.trusted_now_ms()?,
        120000,
    )?
    else {
        return Err("typed_recompute_busy".into());
    };
    let result = (|| {
        // Another same-key invocation may have completed after the initial
        // outcome read but before this lease was acquired. Revalidate current
        // ownership and the owned binding before recovering its exact outcome.
        s.require_background_worker_current()?;
        if typed_backtest::owner(s, &sub.manifest)? != sub.owner_sha256 {
            return Err("object_not_available".into());
        }
        let (mut raw, mut state) = state(s, recomputation_id, sub)?;
        if state.scope != *c.scope() {
            return Err("object_not_available".into());
        }
        let recorded = original(s, &k, c)?;
        if let Some(response) = outcome(s, &k, c)? {
            return Ok(response);
        }
        if matches!(state.phase, Phase::Completed | Phase::Failed) {
            return Err("typed_recompute_terminal".into());
        }
        let b = sub
            .manifest
            .content
            .configuration
            .typed_evaluation
            .as_ref()
            .unwrap();
        let request = match recorded {
            Some(record) => {
                if state.pending_key.is_none() {
                    return Err("typed_recompute_state_conflict".into());
                }
                record
                    .step
                    .ok_or("typed_recompute_original_request_invalid")?
            }
            None => {
                if state.state_sha256 != *expected_state_sha256 {
                    return Err("typed_recompute_state_conflict".into());
                }
                // A fresh authorized outer command may continue the exact
                // retained inner step after an old outer denial/cache loss.
                // It never changes that step's generation/window/inner key.
                let request = if let Some(request) = state.progress.pending.clone() {
                    request
                } else {
                    current(s, sub)?;
                    let request = typed_backtest::next_step(b, &state.progress);
                    state.progress.pending = Some(request.clone());
                    state.pending_key = Some(k.clone());
                    request
                };
                state.phase = Phase::Running;
                state.error = None;
                seal(&mut state)?;
                let context = ctx(sub, &k)?;
                batch(
                    s,
                    vec![
                        StorageWrite::new(
                            REQUESTS,
                            &k,
                            encode(&Original {
                                command: c.clone(),
                                step: Some(request.clone()),
                            })?,
                            context.clone(),
                        ),
                        StorageWrite::new(STATES, recomputation_id, encode(&state)?, context),
                    ],
                    vec![
                        StorageExpectation::new(REQUESTS, &k, None),
                        StorageExpectation::new(STATES, recomputation_id, Some(raw)),
                    ],
                )?;
                raw = encode(&state)?;
                request
            }
        };
        if request != typed_backtest::next_step(b, &state.progress)
            || state.progress.pending.as_ref() != Some(&request)
        {
            return Err("typed_recompute_retained_step_changed".into());
        }
        typed_backtest::owner(s, &sub.manifest)?;
        let root_key = state
            .pending_key
            .clone()
            .ok_or("typed_recompute_original_request_missing")?;
        let context = typed_backtest::step_context(&sub.manifest, &request)?;
        let observed = data_evaluation::recover_owned_receipt(s, &request, &context)?;
        let receipt = if let Some(receipt) = observed {
            receipt
        } else {
            match current(s, sub).and_then(|_| data_evaluation::step(s, request.clone(), &context))
            {
                Ok(receipt) => receipt,
                Err(code) => {
                    typed_backtest::owner(s, &sub.manifest)?;
                    if s.runtime().leases.current(&resource)?.as_ref() != Some(&lease)
                        || s.runtime().clock.trusted_now_ms()? >= lease.expires_at_ms
                    {
                        return Err("typed_recompute_lease_lost".into());
                    }
                    let safe = if code.len() <= 128
                        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    {
                        code
                    } else {
                        "typed_recompute_evaluation_refused".into()
                    };
                    state.phase = Phase::Blocked;
                    state.error = Some(Error {
                        code: safe,
                        retryable: true,
                    });
                    seal(&mut state)?;
                    batch(
                        s,
                        vec![StorageWrite::new(
                            STATES,
                            recomputation_id,
                            encode(&state)?,
                            ctx(sub, &k)?,
                        )],
                        vec![StorageExpectation::new(STATES, recomputation_id, Some(raw))],
                    )?;
                    return Err("typed_recompute_step_reconciliation_required".into());
                }
            }
        };
        typed_backtest::admit_receipt(
            &sub.manifest.content.configuration,
            &mut state.progress,
            request,
            &receipt,
        )?;
        state.pending_key = None;
        state.error = None;
        let status = if state.progress.cursor_ms == b.event_end_ms {
            // Finalization is pure over original owned metadata and committed
            // receipts. A revoked source never causes new provider I/O here.
            let version = backtest_lifecycle::exact_strategy_version(s, &sub.manifest)
                .ok_or("backtest_strategy_version_required")?;
            let snapshot = s
                .runtime()
                .dataset_snapshots
                .get(&sub.manifest.content.configuration.dataset.dataset_id)?
                .ok_or("backtest_dataset_unavailable")?;
            match typed_backtest::finish_recompute(
                s,
                &sub.manifest,
                &version,
                &snapshot,
                &sub.expected,
                &state.progress,
            ) {
                Ok(proof) => {
                    state.proof = Some(proof);
                    state.phase = Phase::Completed;
                    200
                }
                Err(code) => {
                    state.phase = Phase::Failed;
                    state.error = Some(Error {
                        code: code.clone(),
                        retryable: false,
                    });
                    409
                }
            }
        } else {
            state.phase = Phase::Running;
            202
        };
        typed_backtest::owner(s, &sub.manifest)?;
        if s.runtime().leases.current(&resource)?.as_ref() != Some(&lease)
            || s.runtime().clock.trusted_now_ms()? >= lease.expires_at_ms
        {
            return Err("typed_recompute_lease_lost".into());
        }
        seal(&mut state)?;
        let body = view(s, &state)?;
        let outcome = Outcome {
            command_sha256: hash(c)?,
            status,
            body: body.clone(),
        };
        let context = ctx(sub, &k)?;
        let mut writes = vec![
            StorageWrite::new(STATES, recomputation_id, encode(&state)?, context.clone()),
            StorageWrite::new(OUTCOMES, &k, encode(&outcome)?, context.clone()),
        ];
        let mut expectations = vec![
            StorageExpectation::new(STATES, recomputation_id, Some(raw)),
            StorageExpectation::new(OUTCOMES, &k, None),
        ];
        if root_key != k {
            let root: Original = decode(
                s.runtime()
                    .storage
                    .get_json(REQUESTS, &root_key)?
                    .ok_or("typed_recompute_original_request_missing")?,
            )?;
            let root_outcome = Outcome {
                command_sha256: hash(&root.command)?,
                status,
                body: body.clone(),
            };
            writes.push(StorageWrite::new(
                OUTCOMES,
                &root_key,
                encode(&root_outcome)?,
                context,
            ));
            expectations.push(StorageExpectation::new(OUTCOMES, &root_key, None));
        }
        batch(s, writes, expectations)
            .map_err(|_| "typed_recompute_state_seal_reconciliation_required")?;
        Ok(ServiceResponse { status, body })
    })();
    let _ = s.runtime().leases.release(&lease);
    result
}
fn response(error: String) -> ServiceResponse {
    let code = if error.len() <= 128
        && error
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        error.as_str()
    } else {
        "typed_recompute_internal_error"
    };
    if code == "object_not_available" {
        ServiceResponse::object_unavailable()
    } else if matches!(
        code,
        "typed_backtest_current_owner_required" | "typed_backtest_delegation_unsupported"
    ) {
        ServiceResponse::forbidden(code)
    } else if code.ends_with("reconciliation_required")
        || matches!(code, "typed_recompute_lease_lost" | "typed_recompute_busy")
    {
        ServiceResponse {
            status: 503,
            body: json!({"detail":code,"error":{"code":code,"message":code,"retryable":true,"details":{}}}),
        }
    } else if code == "typed_recompute_command_invalid" {
        ServiceResponse::bad_request(code)
    } else {
        ServiceResponse::conflict(code)
    }
}
#[derive(Clone, Debug)]
pub(super) enum Route {
    Start(String),
    Advance(String, String),
    Inspect(String, String),
}
pub(super) fn route(method: &str, path: &str) -> Option<Route> {
    let p = path
        .split('?')
        .next()?
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    match (method.to_ascii_uppercase().as_str(), p.as_slice()) {
        ("POST", ["backtests", run, "recomputations"]) => Some(Route::Start((*run).into())),
        ("POST", ["backtests", run, "recomputations", id, "advance"]) => {
            Some(Route::Advance((*run).into(), (*id).into()))
        }
        ("GET", ["backtests", run, "recomputations", id]) => {
            Some(Route::Inspect((*run).into(), (*id).into()))
        }
        _ => None,
    }
}
fn parse(body: &Value) -> Result<Command, String> {
    if serde_json::to_vec(
        body.get("request")
            .ok_or("typed_recompute_command_invalid")?,
    )
    .map_err(|_| "typed_recompute_command_invalid")?
    .len()
        > 16384
    {
        return Err("typed_recompute_command_invalid".into());
    }
    let command: Command = decode(
        body.get("request")
            .cloned()
            .ok_or("typed_recompute_command_invalid")?,
    )
    .map_err(|_| "typed_recompute_command_invalid")?;
    command.validate()?;
    Ok(command)
}
pub(super) fn capture_http(
    method: &str,
    path: &str,
    body: &Value,
) -> Result<Option<Value>, ServiceResponse> {
    let Some(route) = route(method, path) else {
        return Ok(None);
    };
    if matches!(route, Route::Inspect(..)) {
        return Ok(None);
    }
    if body
        .as_object()
        .is_none_or(|m| m.len() != 1 || !m.contains_key("request"))
    {
        return Err(ServiceResponse::bad_request(
            "typed_recompute_command_invalid",
        ));
    }
    parse(body).map_err(response)?;
    Ok(Some(body["request"].clone()))
}
pub(super) fn bind_body(body: &mut Value, request: Value) {
    body["idempotencyKey"] = request["idempotencyKey"].clone();
    body["runId"] = request["runId"].clone();
    body["request"] = request;
}
pub(super) fn preflight(
    s: &TradeAssemblyService,
    method: &str,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    let route = route(method, path)?;
    if matches!(route, Route::Inspect(..)) {
        return None;
    }
    let check = (|| {
        let c = parse(body)?;
        match (&route, &c) {
            (Route::Start(run), Command::Start { run_id, .. }) if run == run_id => {}
            (
                Route::Advance(run, id),
                Command::Advance {
                    run_id,
                    recomputation_id,
                    ..
                },
            ) if run == run_id && id == recomputation_id => {}
            _ => return Err("typed_recompute_command_invalid".into()),
        };
        let sub = subject(s, c.run_id(), Some(c.scope()))?;
        if let Command::Advance {
            recomputation_id, ..
        } = &c
        {
            state(s, recomputation_id, &sub)?;
        }
        Ok(())
    })();
    check.err().map(response)
}
pub(super) fn dispatch(
    s: &TradeAssemblyService,
    method: &str,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    match route(method, path)? {
        Route::Inspect(run, id) => Some(
            (|| {
                let sub = subject(s, &run, None)?;
                let (_, state) = state(s, &id, &sub)?;
                Ok(ServiceResponse::ok(view(s, &state)?))
            })()
            .unwrap_or_else(response),
        ),
        _ => Some(
            (|| {
                let c = parse(body)?;
                let sub = subject(s, c.run_id(), Some(c.scope()))?;
                match c {
                    Command::Start { .. } => start(s, &c, &sub),
                    Command::Advance { .. } => advance(s, &c, &sub),
                }
            })()
            .unwrap_or_else(response),
        ),
    }
}
pub(super) fn is_tool(name: &str) -> bool {
    matches!(name, START_TOOL | ADVANCE_TOOL | INSPECT_TOOL)
}
pub(super) fn capture_mcp(
    s: &TradeAssemblyService,
    name: &str,
    args: &Value,
) -> Result<Option<Value>, ServiceResponse> {
    if !is_tool(name) {
        return Ok(None);
    }
    if args
        .as_object()
        .is_none_or(|m| m.keys().any(|k| !matches!(k.as_str(), "request" | "db")))
    {
        return Err(ServiceResponse::bad_request(
            "typed_recompute_command_invalid",
        ));
    }
    if name == INSPECT_TOOL {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Inspect {
            run_id: String,
            recomputation_id: String,
        }
        let read: Inspect = decode(args["request"].clone()).map_err(response)?;
        if !cli_path_segment(&read.run_id) || !cli_path_segment(&read.recomputation_id) {
            return Err(ServiceResponse::bad_request(
                "typed_recompute_command_invalid",
            ));
        }
    } else {
        let c = parse(args).map_err(response)?;
        if !matches!(
            (name, &c),
            (START_TOOL, Command::Start { .. }) | (ADVANCE_TOOL, Command::Advance { .. })
        ) {
            return Err(ServiceResponse::bad_request(
                "typed_recompute_command_invalid",
            ));
        }
    }
    let _ = s;
    Ok(Some(args["request"].clone()))
}
pub(super) fn mcp_preflight(
    s: &TradeAssemblyService,
    name: &str,
    args: &Value,
) -> Option<ServiceResponse> {
    if !is_tool(name) {
        return None;
    }
    if name == INSPECT_TOOL {
        return None;
    }
    let c = match parse(args) {
        Ok(c) => c,
        Err(code) => return Some(response(code)),
    };
    let path = match &c {
        Command::Start { run_id, .. } => format!("/backtests/{run_id}/recomputations"),
        Command::Advance {
            run_id,
            recomputation_id,
            ..
        } => format!("/backtests/{run_id}/recomputations/{recomputation_id}/advance"),
    };
    preflight(s, "POST", &path, args)
}
pub(super) fn mcp_response(
    s: &TradeAssemblyService,
    name: &str,
    args: &Value,
) -> Option<ServiceResponse> {
    let read = &args["request"];
    if name == INSPECT_TOOL {
        return dispatch(
            s,
            "GET",
            &format!(
                "/backtests/{}/recomputations/{}",
                read["runId"].as_str()?,
                read["recomputationId"].as_str()?
            ),
            &json!({}),
        );
    }
    if !matches!(name, START_TOOL | ADVANCE_TOOL) {
        return None;
    }
    let c = match parse(args) {
        Ok(c) => c,
        Err(code) => return Some(response(code)),
    };
    let path = match &c {
        Command::Start { run_id, .. } => format!("/backtests/{run_id}/recomputations"),
        Command::Advance {
            run_id,
            recomputation_id,
            ..
        } => format!("/backtests/{run_id}/recomputations/{recomputation_id}/advance"),
    };
    if let Some(error) = preflight(s, "POST", &path, args) {
        return Some(error);
    }
    dispatch(s, "POST", &path, args)
}
pub(crate) fn tool_schema(kind: &str) -> Value {
    let label = json!({"type":"string","minLength":1,"maxLength":128});
    let segment =
        json!({"type":"string","minLength":1,"maxLength":128,"pattern":"^[A-Za-z0-9_-]+$"});
    if kind == "inspect" {
        return json!({"type":"object","additionalProperties":false,"properties":{"runId":segment,"recomputationId":segment},"required":["runId","recomputationId"]});
    }
    let scope = json!({"type":"object","additionalProperties":false,"properties":{"strategyId":label,"strategyVersionId":label,"strategySpecHash":{"type":"string","pattern":"^sha256:[0-9a-f]{64}$"}},"required":["strategyId","strategyVersionId","strategySpecHash"]});
    let common = json!({"schemaVersion":{"const":SCHEMA},"kind":{"enum":["start","advance"]},"scope":scope,"runId":segment,"idempotencyKey":{"type":"string","minLength":1,"maxLength":200,"pattern":"^[!-~]+$"}});
    let mut start = common.clone();
    start["kind"] = json!({"const":"start"});
    start["expectedManifestHash"] = json!({"type":"string","pattern":"^sha256:[0-9a-f]{64}$"});
    start["expectedResultHash"] = start["expectedManifestHash"].clone();
    let mut advance = common;
    advance["kind"] = json!({"const":"advance"});
    advance["recomputationId"] = segment;
    advance["expectedStateSha256"] = json!({"type":"string","pattern":"^[0-9a-f]{64}$"});
    if kind == "start" {
        json!({"type":"object","additionalProperties":false,"properties":start,"required":["schemaVersion","kind","scope","runId","idempotencyKey","expectedManifestHash","expectedResultHash"]})
    } else {
        json!({"type":"object","additionalProperties":false,"properties":advance,"required":["schemaVersion","kind","scope","runId","idempotencyKey","recomputationId","expectedStateSha256"]})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn start_request() -> Value {
        json!({"schemaVersion":SCHEMA,"kind":"start","scope":{"strategyId":"synthetic.strategy","strategyVersionId":"synthetic.version","strategySpecHash":format!("sha256:{}","1".repeat(64))},"runId":"backtest_synthetic","expectedManifestHash":format!("sha256:{}","2".repeat(64)),"expectedResultHash":format!("sha256:{}","3".repeat(64)),"idempotencyKey":"original-recompute-key"})
    }
    #[test]
    fn typed_recompute_closed_request_survives_enrichment_without_implicit_key_or_resource() {
        let request = start_request();
        let mut body = json!({"request":request,"authorityContext":{"actor":"current-human","surface":"http","accountMode":"paper"}});
        bind_body(&mut body, request.clone());
        assert_eq!(body["request"], request);
        let http = crate::control_plane::http_envelope(
            "http",
            "POST",
            "/backtests/backtest_synthetic/recomputations",
            &body,
        )
        .unwrap();
        let mcp = crate::control_plane::mcp_envelope(START_TOOL, &body).unwrap();
        for envelope in [http, mcp] {
            assert_eq!(envelope.command_name, "backtest.run.execute");
            assert_eq!(envelope.idempotency_key.as_str(), "original-recompute-key");
            assert_eq!(
                envelope.target_object.as_deref(),
                Some("backtest_synthetic")
            );
            assert_ne!(envelope.side_effect_class, "read");
            assert_eq!(envelope.idempotency_requirement, "required");
            assert_eq!(
                crate::finance_authority::authority_definition(&envelope)
                    .unwrap()
                    .resource_kind,
                "backtest_run"
            );
        }
        for extra in [
            "actor",
            "owner",
            "authorityContext",
            "window",
            "credentials",
        ] {
            let mut wrong = request.clone();
            wrong[extra] = json!("forbidden");
            assert!(parse(&json!({"request":wrong})).is_err());
        }
        let mut bare = request;
        bare["scope"]["strategySpecHash"] = json!("1".repeat(64));
        assert!(parse(&json!({"request":bare})).is_err());
    }
    #[test]
    fn typed_recompute_busy_is_retryable_without_promising_outer_key_reexecution() {
        let busy = response("typed_recompute_busy".into());
        assert_eq!(busy.status, 503);
        assert_eq!(busy.body["error"]["code"], "typed_recompute_busy");
        assert_eq!(busy.body["error"]["retryable"], true);
        let uncertain = response("typed_recompute_start_reconciliation_required".into());
        assert_eq!(uncertain.status, 503);
        assert_eq!(uncertain.body["error"]["retryable"], true);
        assert_eq!(
            response("typed_recompute_idempotency_conflict".into()).status,
            409
        );
    }
    #[test]
    fn typed_recompute_tools_are_finite_mutations_and_true_inspection() {
        let definitions = crate::mcp::tool_definitions();
        let tools = definitions.as_array().unwrap();
        for name in [START_TOOL, ADVANCE_TOOL] {
            let tool = tools.iter().find(|t| t["name"] == name).unwrap();
            assert_eq!(tool["annotations"]["readOnlyHint"], false);
            assert_eq!(tool["annotations"]["idempotentHint"], true);
        }
        let inspect = tools.iter().find(|t| t["name"] == INSPECT_TOOL).unwrap();
        assert_eq!(inspect["annotations"]["readOnlyHint"], true);
        assert!(route("GET", "/backtests/run/replay").is_none());
        assert!(matches!(
            route("GET", "/backtests/run/recomputations/id"),
            Some(Route::Inspect(..))
        ));
    }
}
