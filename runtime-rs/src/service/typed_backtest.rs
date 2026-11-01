//! Canonical owned typed research. Pure publisher evaluation drives research-only
//! simulation; this module never constructs provider fills or broker authority.
use super::{data_captures, data_evaluation, data_plans, data_strategy, TradeAssemblyService};
use crate::backtest_contracts::*;
use crate::historical_data::{DatasetSnapshot, HistoricalObservationData};
use crate::ports::{
    ComparePutOutcome, IdempotencyKey, SideEffectContext, StorageExpectation, StorageWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow;
const PROGRESS: &str = "typed_backtest_progress_v1";
const REPLAYS: &str = "typed_backtest_replays_v1";
const MAX_BYTES: usize = 2 * 1024 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(0);
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(value)
        .map_err(|_| "typed_backtest_encoding_invalid".into())
}
fn nonce(s: &TradeAssemblyService) -> Result<String, String> {
    Ok(format!(
        "{}-{}-{}",
        s.runtime().clock.trusted_now_ms()?,
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ))
}
fn context(manifest: &BacktestRunManifest, key: &str) -> Result<SideEffectContext, String> {
    Ok(SideEffectContext::new(
        manifest.content.authority.clone(),
        IdempotencyKey::new(key)?,
    ))
}
fn actual_owner(s: &TradeAssemblyService) -> Result<crate::ports::ObjectOwner, String> {
    if s.agent_mcp_execution_context().is_some() || s.invocation_actor_kind != "user" {
        return Err("typed_backtest_delegation_unsupported".into());
    }
    s.invocation_owner()
        .ok_or("typed_backtest_current_owner_required".into())
}
fn owner(s: &TradeAssemblyService, manifest: &BacktestRunManifest) -> Result<String, String> {
    let current = actual_owner(s)?;
    let scope = s
        .runtime()
        .object_authorization
        .scope("backtest_run", &manifest.content.run_id)?
        .ok_or("typed_backtest_run_not_owned")?;
    if scope.owner != current {
        return Err("typed_backtest_run_not_owned".into());
    }
    s.require_object("backtest_run", &manifest.content.run_id)
        .map_err(|_| "typed_backtest_run_not_owned")?;
    if manifest
        .content
        .authority
        .account_mode
        .eq_ignore_ascii_case("live")
    {
        return Err("typed_backtest_live_authority_unsupported".into());
    }
    hash(&current)
}
/// Initial fixed-quantity signal simulation supports only the bare portable
/// stages below. Required custom sizing/risk/screens/stops/session triggers are
/// rejected rather than being silently approximated by a research assumption.
fn validate_strategy(
    s: &TradeAssemblyService,
    c: &BacktestConfiguration,
) -> Result<crate::spec::StrategySpec, String> {
    let version = s
        .runtime()
        .storage
        .list_json("strategy_versions")?
        .into_iter()
        .map(|(_, v)| v)
        .find(|v| {
            v["id"] == c.strategy.strategy_version_id
                && (v["strategyId"] == c.strategy.strategy_id
                    || v["strategy_id"] == c.strategy.strategy_id)
        })
        .ok_or("typed_backtest_strategy_missing")?;
    if canonical_hash(&version["spec"], "typed_backtest_strategy_hash_invalid")?
        != c.strategy.strategy_spec_hash
    {
        return Err("typed_backtest_strategy_changed".into());
    }
    let spec = crate::spec::StrategySpec::model_validate(version["spec"].clone())?;
    use crate::spec::{
        AllocationMode, EndOfDataBehavior, ExitMonitoringMode, ExitScope, ExitTriggerKind,
        MultiLegMode, OrderType, StageName,
    };
    if spec.allocation.mode != AllocationMode::None
        || !spec.variables.is_empty()
        || spec
            .signal_market_requirements
            .iter()
            .flat_map(|m| &m.selectors)
            .any(|v| v.screen.is_some())
    {
        return Err("typed_backtest_required_stage_unsupported".into());
    }
    for (stage, operation) in [
        (StageName::Inputs, "market_data.bars.read@1"),
        (StageName::Universe, "universe.static.select@1"),
        (StageName::Intent, "intent.trade.build@1"),
        (StageName::Sizing, "sizing.relative.apply@1"),
        (StageName::Risk, "risk.strategy.guard@1"),
        (StageName::ExitPolicy, "exit.policy.evaluate@1"),
        (StageName::OrderStrategy, "order.intent.build@1"),
        (StageName::PricePolicy, "price.snapshot.select@1"),
        (StageName::MarketClock, "calendar.session.resolve@1"),
    ] {
        let view = spec.stages.view(&stage);
        if !view.enabled || !view.config.is_empty() || view.substeps.len() != 1 {
            return Err("typed_backtest_required_stage_unsupported".into());
        }
        let step = &view.substeps[0];
        if step.operation_ref != operation
            || !step.config.is_empty()
            || step.expression.is_some()
            || step.state.is_some()
            || step.option_selection.is_some()
            || step.optional
        {
            return Err("typed_backtest_required_stage_unsupported".into());
        }
    }
    let evaluation = spec.stages.view(&StageName::Evaluate);
    if !evaluation.enabled || !evaluation.config.is_empty() || evaluation.substeps.len() != 1 {
        return Err("typed_backtest_required_stage_unsupported".into());
    }
    let step = &evaluation.substeps[0];
    if step.operation_ref != "signal.rule.evaluate@1"
        || step.config.len() != 1
        || !step.config.contains_key(crate::data_strategy::CONFIG_KEY)
        || step.expression.is_some()
        || step.state.is_some()
        || step.optional
        || step.option_selection.is_some()
    {
        return Err("typed_backtest_required_stage_unsupported".into());
    }
    let exit = &spec.stages.exit_policy.policy;
    validate_signal_exit_and_tif(
        exit,
        &spec.stages.order_strategy.policy,
        &c.execution.time_in_force,
    )?;
    if exit.monitoring != [ExitMonitoringMode::Synthetic]
        || exit.triggers.len() != 1
        || exit.triggers[0].kind != ExitTriggerKind::Indicator
        || exit.triggers[0].scope != ExitScope::FullPosition
        || !matches!(
            (&exit.end_of_data, &c.end_of_data),
            (
                EndOfDataBehavior::PreserveOpenPositions,
                EndOfDataPolicy::MarkToMarket
            ) | (
                EndOfDataBehavior::CloseSyntheticPositions,
                EndOfDataPolicy::ForceClose
            )
        )
    {
        return Err("typed_backtest_exit_stage_unsupported".into());
    }
    let order = &spec.stages.order_strategy.policy;
    if order.allowed_order_types != [OrderType::Market]
        || order.multi_leg_mode != MultiLegMode::NotApplicable
        || order.conditional_orders_allowed
        || spec.stages.price_policy.policy.expression.is_some()
        || spec.stages.price_policy.policy.tick_size.is_some()
        || !spec.stages.market_clock.policy.session_triggers.is_empty()
        || spec
            .stages
            .market_clock
            .policy
            .eligibility_expression
            .is_some()
    {
        return Err("typed_backtest_required_stage_unsupported".into());
    }
    Ok(spec)
}
fn validate_signal_exit_and_tif(
    exit: &crate::spec::ExitPolicy,
    order: &crate::spec::OrderPolicy,
    time_in_force: &str,
) -> Result<(), String> {
    if exit
        .triggers
        .iter()
        .any(|trigger| trigger.expression.is_some() || trigger.capability_requirement_ref.is_some())
    {
        return Err("typed_backtest_exit_stage_unsupported".into());
    }
    if !order
        .time_in_force
        .iter()
        .any(|published| published == time_in_force)
    {
        return Err("typed_backtest_order_tif_unsupported".into());
    }
    Ok(())
}
fn validate_calendar(
    c: &BacktestConfiguration,
    snapshot: &DatasetSnapshot,
    spec: &crate::spec::StrategySpec,
) -> Result<(), String> {
    use chrono::{Datelike, Timelike};
    let clock = &spec.stages.market_clock.policy;
    let zone: chrono_tz::Tz = clock
        .timezone
        .parse()
        .map_err(|_| "typed_backtest_timezone_unsupported")?;
    if c.dataset.timezone != clock.timezone
        || clock.calendar_refs != [c.dataset.calendar.clone()]
        || clock.holiday_behavior != crate::spec::HolidayBehavior::FollowCalendar
        || clock.early_close_behavior != crate::spec::EarlyCloseBehavior::FollowCalendar
    {
        return Err("typed_backtest_calendar_policy_unsupported".into());
    }
    for row in &snapshot.content.observations {
        let HistoricalObservationData::Bar(bar) = &row.data else {
            return Err("typed_backtest_fill_kind_invalid".into());
        };
        let session = bar
            .session
            .as_ref()
            .ok_or("typed_backtest_session_evidence_required")?;
        let parse = |value: &str| {
            chrono::DateTime::parse_from_rfc3339(value)
                .map_err(|_| "typed_backtest_session_invalid")
        };
        let raw = parse(&row.timestamp)?.timestamp_millis();
        let binding = c
            .typed_evaluation
            .as_ref()
            .ok_or("typed_backtest_binding_required")?;
        let (start, end) = binding.fill_bounds(raw)?;
        let at = chrono::DateTime::from_timestamp_millis(start)
            .ok_or("typed_backtest_fill_time_invalid")?;
        if session.source_ref.is_empty()
            || session.calendar != c.dataset.calendar
            || start < parse(&session.start)?.timestamp_millis()
            || end > parse(&session.end)?.timestamp_millis()
        {
            return Err("typed_backtest_outside_provider_session".into());
        }
        let civil = at.with_timezone(&zone);
        let day = civil.weekday().to_string().to_ascii_lowercase();
        let weekday = match day.as_str() {
            "mon" => "monday",
            "tue" => "tuesday",
            "wed" => "wednesday",
            "thu" => "thursday",
            "fri" => "friday",
            "sat" => "saturday",
            "sun" => "sunday",
            _ => return Err("typed_backtest_clock_invalid".into()),
        };
        let minute = civil.hour() * 60 + civil.minute();
        let parse_minute = |v: &str| -> Result<u32, String> {
            if v == "24:00" {
                return Ok(24 * 60);
            }
            let t = chrono::NaiveTime::parse_from_str(v, "%H:%M")
                .map_err(|_| "typed_backtest_clock_invalid")?;
            Ok(t.hour() * 60 + t.minute())
        };
        let mut in_window = false;
        for window in &clock.windows {
            if minute >= parse_minute(&window.start)? && minute < parse_minute(&window.end)? {
                in_window = true;
            }
        }
        if !in_window || !clock.days.iter().any(|d| d == weekday) {
            return Err("typed_backtest_outside_published_window".into());
        }
    }
    Ok(())
}
/// Validate admission using actual owned immutable plan/capture documents. No
/// acquisition, plan mutation, substitute dataset or unverified source is used.
pub(crate) fn validate_configuration(
    s: &TradeAssemblyService,
    c: &BacktestConfiguration,
) -> Result<(), String> {
    actual_owner(s)?;
    let b = c
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_binding_required")?;
    if !b.validate() || !c.validate().ready || b.as_of_ms > s.runtime().clock.trusted_now_ms()? {
        return Err("typed_backtest_config_invalid".into());
    }
    crate::backtest_engine::validate_typed_model(c)?;
    validate_strategy(s, c)?;
    let plan = data_plans::current(s, &b.plan_revision_id)?;
    if plan.revision_sha256 != b.plan_revision_sha256
        || plan.content.scope.strategy_id != c.strategy.strategy_id
        || plan.content.scope.strategy_version_id != c.strategy.strategy_version_id
        || plan.content.scope.strategy_spec_hash != c.strategy.strategy_spec_hash
        || plan.content.plan.request.mode != "backtest"
        || plan.content.plan.request.capability_graph_revision != c.capability_graph.revision_id
        || plan.content.plan.capability_graph_fingerprint != c.capability_graph.graph_fingerprint
    {
        return Err("typed_backtest_plan_binding_changed".into());
    }
    if plan.content.selection_source.is_some() {
        return Err("typed_backtest_historical_selection_unsupported".into());
    }
    let program = data_strategy::program_for_content(s, &plan.content)?
        .ok_or("typed_backtest_signal_program_required")?;
    program.validate(&plan.content.plan)?;
    let roots = plan
        .content
        .plan
        .contracts
        .iter()
        .filter(|n| n.contract.inputs.is_empty())
        .map(|n| n.graph_node_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if roots.iter().ne(b.source_captures.keys()) {
        return Err("typed_backtest_capture_roots_mismatch".into());
    }
    if plan.content.plan.request.trade_instruments.len() != 1
        || plan.content.plan.request.trade_instruments[0].instrument_id
            != c.instruments[0].instrument_id
        || plan.content.plan.request.trade_instruments[0].instrument_family
            != c.instruments[0].instrument_family
        || (c.instruments[0].instrument_family == "crypto_spot"
            && plan.content.plan.request.trade_instruments[0].asset_class != "crypto")
    {
        return Err("typed_backtest_trade_subject_mismatch".into());
    }
    if plan
        .content
        .plan
        .contracts
        .iter()
        .any(|node| node.contract.state.is_some() && node.contract.v3().is_none())
    {
        return Err("typed_backtest_state_reconstruction_contract_required".into());
    }
    for node in &plan.content.plan.request.nodes {
        if node.context.interval_ms != Some(b.interval_ms) || node.context.instruments.len() != 1 {
            return Err("typed_backtest_context_unsupported".into());
        }
        if !plan
            .content
            .package_sha256
            .contains_key(&node.graph_node_id)
            || !plan
                .content
                .binding_sha256
                .contains_key(&node.graph_node_id)
        {
            return Err("typed_backtest_plan_binding_changed".into());
        }
    }
    for (node, pin) in &b.source_captures {
        let receipt = data_captures::current(s, &pin.capture_id)?;
        if receipt.receipt_sha256 != pin.receipt_sha256
            || !matches!(plan.content.source_authentication.get(node),Some(data_captures::SourceAuthentication::HostAcquisition{capture_id,receipt_sha256,..}) if capture_id==&pin.capture_id && receipt_sha256==&pin.receipt_sha256)
            || receipt.policy.scope != plan.content.scope
            || receipt.policy.mode != "backtest"
            || receipt.policy.replay_node != *node
            || receipt.policy.context.interval_ms != Some(b.interval_ms)
            || receipt.policy.event_start_ms != b.event_start_ms
            || receipt.policy.event_end_ms != b.event_end_ms
            || receipt.acquisition_response.observed_at_ms > b.as_of_ms
            || receipt.replay_observed_at_ms > b.as_of_ms
        {
            return Err("typed_backtest_capture_binding_changed".into());
        }
    }
    data_captures::validate_window(
        s,
        &plan,
        &EvaluationWindow {
            event_start_ms: b.event_start_ms,
            event_end_ms: b.event_end_ms,
            as_of_ms: b.as_of_ms,
        },
    )?;
    Ok(())
}
pub(crate) fn validate_dataset(
    service: &TradeAssemblyService,
    c: &BacktestConfiguration,
    snapshot: &DatasetSnapshot,
) -> Result<(), String> {
    let b = c
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_binding_required")?;
    snapshot.verify()?;
    crate::backtest_engine::verify_dataset_configuration(snapshot, c)?;
    crate::backtest_engine::validate_typed_crypto_precision(c, snapshot)?;
    crate::backtest_engine::validate_typed_precision(snapshot)?;
    validate_calendar(c, snapshot, &validate_strategy(service, c)?)?;
    if c.instruments.len() != 1
        || snapshot.content.instruments != vec![c.instruments[0].instrument_id.clone()]
        || snapshot.content.observations.len() as u64
            != (b.event_end_ms - b.event_start_ms) as u64 / b.interval_ms
    {
        return Err("typed_backtest_fill_series_alignment_invalid".into());
    }
    for (index, row) in snapshot.content.observations.iter().enumerate() {
        let expected_start = b.event_start_ms + index as i64 * b.interval_ms as i64;
        let expected_end = expected_start + b.interval_ms as i64;
        if row.instrument_id != c.instruments[0].instrument_id
            || !matches!(row.data, HistoricalObservationData::Bar(_))
            || b.fill_bounds(
                chrono::DateTime::parse_from_rfc3339(&row.timestamp)
                    .map_err(|_| "typed_backtest_fill_time_invalid")?
                    .timestamp_millis(),
            )? != (expected_start, expected_end)
        {
            return Err("typed_backtest_fill_series_alignment_invalid".into());
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Progress {
    schema_version: String,
    owner_sha256: String,
    manifest_hash: String,
    evaluation_run_id: String,
    cursor_ms: i64,
    checkpoint_sha256: Option<String>,
    pending: Option<data_evaluation::EvaluationStep>,
    steps: Vec<data_evaluation::EvaluationStep>,
    receipts: Vec<String>,
    state_sha256: String,
}
fn seal(state: &mut Progress) -> Result<(), String> {
    state.state_sha256.clear();
    state.state_sha256 = hash(state)?;
    Ok(())
}
fn state_key(manifest: &BacktestRunManifest) -> Result<String, String> {
    hash(&json!({"run":manifest.content.run_id,"manifest":manifest.manifest_hash}))
}
fn save(
    s: &TradeAssemblyService,
    key: &str,
    old: Option<Value>,
    state: &mut Progress,
    ctx: &SideEffectContext,
) -> Result<(), String> {
    seal(state)?;
    let value = serde_json::to_value(state).map_err(|_| "typed_backtest_state_invalid")?;
    if serde_json::to_vec(&value)
        .map_err(|_| "typed_backtest_state_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("typed_backtest_state_budget_exceeded".into());
    }
    if s.runtime().storage.put_json_batch(
        &[StorageWrite::new(PROGRESS, key, value, ctx.clone())],
        &[StorageExpectation::new(PROGRESS, key, old)],
    )? != ComparePutOutcome::Updated
    {
        return Err("typed_backtest_progress_conflict".into());
    }
    Ok(())
}
fn step_context(
    manifest: &BacktestRunManifest,
    request: &data_evaluation::EvaluationStep,
) -> Result<SideEffectContext, String> {
    context(manifest, &format!("typed-backtest:{}", hash(request)?))
}
fn next_step(b: &TypedBacktestBinding, state: &Progress) -> data_evaluation::EvaluationStep {
    data_evaluation::EvaluationStep {
        run_id: state.evaluation_run_id.clone(),
        plan_revision_id: b.plan_revision_id.clone(),
        window: EvaluationWindow {
            event_start_ms: state.cursor_ms,
            event_end_ms: (state.cursor_ms + b.max_frames_per_step as i64 * b.interval_ms as i64)
                .min(b.event_end_ms),
            as_of_ms: b.as_of_ms,
        },
        expected_checkpoint_sha256: state.checkpoint_sha256.clone(),
    }
}
fn admit_receipt(
    c: &BacktestConfiguration,
    state: &mut Progress,
    request: data_evaluation::EvaluationStep,
    receipt: &data_evaluation::EvaluationReceipt,
) -> Result<(), String> {
    let b = c
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_binding_required")?;
    let count =
        (request.window.event_end_ms - request.window.event_start_ms) as u64 / b.interval_ms;
    if count == 0
        || receipt.frames_processed != count
        || receipt.signals.len() as u64 != count
        || receipt.checkpoint.run_id != state.evaluation_run_id
        || receipt.checkpoint.plan_revision_id != b.plan_revision_id
        || receipt.checkpoint.cursors.is_empty()
        || receipt
            .checkpoint
            .cursors
            .values()
            .any(|end| *end != request.window.event_end_ms)
        || state.receipts.len() >= b.maximum_frames as usize
    {
        return Err("typed_backtest_evaluation_coverage_invalid".into());
    }
    for (index, frame) in receipt.signals.iter().enumerate() {
        if frame.event_end_ms
            != request.window.event_start_ms + (index as i64 + 1) * b.interval_ms as i64
            || frame.known_at_ms > b.as_of_ms
            || frame.arrived_at_ms > b.as_of_ms
            || frame.trade_instruments.len() != 1
            || frame.trade_instruments[0].instrument_id != c.instruments[0].instrument_id
        {
            return Err("typed_backtest_signal_binding_invalid".into());
        }
    }
    state.cursor_ms = request.window.event_end_ms;
    state.checkpoint_sha256 = Some(receipt.checkpoint.checkpoint_sha256.clone());
    state.steps.push(request);
    state.receipts.push(receipt.receipt_sha256.clone());
    state.pending = None;
    Ok(())
}
fn evidence(
    s: &TradeAssemblyService,
    m: &BacktestRunManifest,
    state: &Progress,
) -> Result<
    (
        TypedBacktestEvidence,
        Vec<crate::strategy_kernel::BooleanSignal>,
    ),
    String,
> {
    let b = m
        .content
        .configuration
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_binding_required")?;
    let mut semantic = Vec::new();
    let mut semantic_bytes = 0_usize;
    let mut signals = Vec::new();
    let mut values = Vec::new();
    let mut warming = 0;
    for (request, digest) in state.steps.iter().zip(&state.receipts) {
        let receipt = data_evaluation::inspect_receipt(s, request, &step_context(m, request)?)?;
        if receipt.receipt_sha256 != *digest {
            return Err("typed_backtest_original_receipt_changed".into());
        }
        let mut checkpoint = serde_json::to_value(&receipt.checkpoint)
            .map_err(|_| "typed_backtest_evidence_invalid")?;
        for key in ["runId", "checkpointSha256"] {
            checkpoint.as_object_mut().unwrap().remove(key);
        }
        let observed =
            json!({"outputs":receipt.outputs,"signals":receipt.signals,"checkpoint":checkpoint});
        semantic_bytes = semantic_bytes
            .checked_add(
                serde_json::to_vec(&observed)
                    .map_err(|_| "typed_backtest_encoding_invalid")?
                    .len(),
            )
            .ok_or("typed_backtest_evidence_budget_exceeded")?;
        if semantic_bytes > 8 * 1024 * 1024 {
            return Err("typed_backtest_evidence_budget_exceeded".into());
        }
        semantic.push(observed);
        for frame in &receipt.signals {
            let boolean = |v: &crate::data_strategy::SignalValue| match v {
                crate::data_strategy::SignalValue::Ready { value } => Some(*value),
                crate::data_strategy::SignalValue::WarmingUp => None,
            };
            let entry = boolean(&frame.entry);
            let exit = boolean(&frame.exit);
            if entry.is_none() || exit.is_none() {
                warming += 1;
            }
            signals.push(crate::strategy_kernel::BooleanSignal { entry, exit });
            values.push(serde_json::to_value(frame).map_err(|_| "typed_backtest_signal_invalid")?);
        }
    }
    Ok((
        TypedBacktestEvidence {
            schema_version: "tradeassembly.typed-backtest-evidence.v1".into(),
            plan_revision_id: b.plan_revision_id.clone(),
            plan_revision_sha256: b.plan_revision_sha256.clone(),
            evaluation_receipts: state.receipts.clone(),
            final_checkpoint_sha256: state
                .checkpoint_sha256
                .clone()
                .ok_or("typed_backtest_checkpoint_missing")?,
            semantic_sha256: hash(&semantic)?,
            frames_processed: signals.len() as u64,
            warming_frames: warming,
            signals: values,
            equity: Vec::new(),
            max_drawdown_bps: 0,
            knowledge_policy: b.knowledge_policy.clone(),
            point_in_time_qualified: false,
        },
        signals,
    ))
}
fn acquire_progress_lease(
    s: &TradeAssemblyService,
    resource: &str,
) -> Result<Option<crate::ports::LeaseClaim>, String> {
    s.runtime().leases.acquire(
        resource,
        &nonce(s)?,
        s.runtime().clock.trusted_now_ms()?,
        120000,
    )
}
/// One retained evaluator step per canonical queue delivery; crash recovery uses
/// the original immutable inner key before moving the outer progress cursor.
pub(crate) fn advance(
    s: &TradeAssemblyService,
    m: &BacktestRunManifest,
    version: &Value,
    snapshot: &DatasetSnapshot,
) -> Result<Option<BacktestResult>, String> {
    let who = owner(s, m)?;
    validate_configuration(s, &m.content.configuration)?;
    validate_dataset(s, &m.content.configuration, snapshot)?;
    let b = m
        .content
        .configuration
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_binding_required")?;
    let key = state_key(m)?;
    let ctx = context(m, &format!("typed-backtest-progress:{key}"))?;
    let resource = format!("typed-backtest:{key}");
    let Some(lease) = acquire_progress_lease(s, &resource)? else {
        // Another legitimate turn owns the retained command. Defer without
        // changing progress, invoking a plugin, or failing the run.
        return Ok(None);
    };
    let result = (|| {
        let old = s.runtime().storage.get_json(PROGRESS, &key)?;
        let mut state: Progress = match &old {
            Some(raw) => {
                serde_json::from_value(raw.clone()).map_err(|_| "typed_backtest_state_invalid")?
            }
            None => Progress {
                schema_version: "tradeassembly.typed-backtest-progress.v1".into(),
                owner_sha256: who.clone(),
                manifest_hash: m.manifest_hash.clone(),
                evaluation_run_id: format!("typed-backtest-{}", hash(&m.content.run_id)?),
                cursor_ms: b.event_start_ms,
                checkpoint_sha256: None,
                pending: None,
                steps: Vec::new(),
                receipts: Vec::new(),
                state_sha256: String::new(),
            },
        };
        if old.is_some() {
            let mut check = state.clone();
            seal(&mut check)?;
            if check != state {
                return Err("typed_backtest_state_invalid".into());
            }
        }
        if state.schema_version != "tradeassembly.typed-backtest-progress.v1"
            || state.evaluation_run_id != format!("typed-backtest-{}", hash(&m.content.run_id)?)
            || state.owner_sha256 != who
            || state.manifest_hash != m.manifest_hash
            || state.steps.len() != state.receipts.len()
            || state.receipts.len() > 256
            || state.cursor_ms < b.event_start_ms
            || state.cursor_ms > b.event_end_ms
        {
            return Err("typed_backtest_state_invalid".into());
        }
        if state.cursor_ms < b.event_end_ms {
            let mut expected = old;
            if state.pending.is_none() {
                state.pending = Some(next_step(b, &state));
                save(s, &key, expected.take(), &mut state, &ctx)?;
                expected =
                    Some(serde_json::to_value(&state).map_err(|_| "typed_backtest_state_invalid")?);
            }
            let request = state
                .pending
                .clone()
                .ok_or("typed_backtest_pending_step_missing")?;
            if request != next_step(b, &state) {
                return Err("typed_backtest_retained_command_changed".into());
            }
            owner(s, m)?;
            validate_configuration(s, &m.content.configuration)?;
            let receipt = data_evaluation::step(s, request.clone(), &step_context(m, &request)?)?;
            admit_receipt(&m.content.configuration, &mut state, request, &receipt)?;
            if s.runtime().leases.current(&resource)?.as_ref() != Some(&lease)
                || s.runtime().clock.trusted_now_ms()? >= lease.expires_at_ms
            {
                return Err("typed_backtest_lease_lost".into());
            }
            owner(s, m)?;
            save(s, &key, expected, &mut state, &ctx)?;
        }
        if state.cursor_ms < b.event_end_ms {
            return Ok(None);
        }
        let (evidence, signals) = evidence(s, m, &state)?;
        crate::backtest_engine::execute_typed(
            m,
            version,
            snapshot,
            &signals,
            evidence,
            s.runtime().clock.now_ms(),
        )
        .map(Some)
    })();
    let _ = s.runtime().leases.release(&lease);
    result
}
/// Recompute in a fresh normal evaluator run so no original receipt/operation
/// key can satisfy the computation. Only identity fields are excluded from the
/// semantic comparison; original immutable result evidence remains unchanged.
pub(crate) fn replay(
    s: &TradeAssemblyService,
    m: &BacktestRunManifest,
    version: &Value,
    snapshot: &DatasetSnapshot,
    expected: &BacktestResult,
) -> Result<BacktestResult, String> {
    let who = owner(s, m)?;
    expected.verify()?;
    validate_configuration(s, &m.content.configuration)?;
    validate_dataset(s, &m.content.configuration, snapshot)?;
    let b = m
        .content
        .configuration
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_binding_required")?;
    let mut state = Progress {
        schema_version: "tradeassembly.typed-backtest-progress.v1".into(),
        owner_sha256: who,
        manifest_hash: m.manifest_hash.clone(),
        evaluation_run_id: format!("typed-replay-{}", hash(&nonce(s)?)?),
        cursor_ms: b.event_start_ms,
        checkpoint_sha256: None,
        pending: None,
        steps: Vec::new(),
        receipts: Vec::new(),
        state_sha256: String::new(),
    };
    while state.cursor_ms < b.event_end_ms {
        owner(s, m)?;
        let request = next_step(b, &state);
        let receipt = data_evaluation::step(s, request.clone(), &step_context(m, &request)?)?;
        admit_receipt(&m.content.configuration, &mut state, request, &receipt)?;
    }
    let (evidence, signals) = evidence(s, m, &state)?;
    let mut recomputed = crate::backtest_engine::execute_typed(
        m,
        version,
        snapshot,
        &signals,
        evidence,
        expected.created_at_ms,
    )?;
    let original = expected
        .content
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_original_evidence_missing")?;
    let actual = recomputed
        .content
        .typed_evaluation
        .as_ref()
        .ok_or("typed_backtest_replay_evidence_missing")?;
    if original.semantic_sha256 != actual.semantic_sha256 {
        return Err("typed_backtest_replay_semantic_mismatch".into());
    }
    // All recalculated economic outputs and semantic signal evidence must match.
    // Original receipt/checkpoint identities remain the original run's lineage.
    let actual_receipts = actual.evaluation_receipts.clone();
    let actual_checkpoint = actual.final_checkpoint_sha256.clone();
    recomputed
        .content
        .typed_evaluation
        .as_mut()
        .unwrap()
        .evaluation_receipts = original.evaluation_receipts.clone();
    recomputed
        .content
        .typed_evaluation
        .as_mut()
        .unwrap()
        .final_checkpoint_sha256 = original.final_checkpoint_sha256.clone();
    if recomputed.content != expected.content {
        return Err("typed_backtest_replay_mismatch".into());
    }
    let ctx = context(
        m,
        &format!("typed-backtest-replay:{}", hash(&state.evaluation_run_id)?),
    )?;
    s.runtime().storage.put_json_if_absent(REPLAYS,&state.evaluation_run_id,json!({"schemaVersion":"tradeassembly.typed-backtest-replay.v1","originalResultHash":expected.result_hash,"recomputed":true,"replayRunId":state.evaluation_run_id,"evaluationReceipts":actual_receipts,"finalCheckpointSha256":actual_checkpoint,"semanticSha256":original.semantic_sha256}),&ctx)?;
    Ok(expected.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_backtest_binding_is_closed_bounded_and_retrospective_only() {
        let raw = json!({"schemaVersion":"tradeassembly.typed-backtest-binding.v1","planRevisionId":"dataplan_original","planRevisionSha256":"a".repeat(64),"sourceCaptures":{"source":{"captureId":"capture_original","receiptSha256":"b".repeat(64)}},"eventStartMs":1000,"eventEndMs":5000,"asOfMs":6000,"intervalMs":1000,"maxFramesPerStep":2,"maximumFrames":4,"quantityMicros":1000000,"initialPosition":"flat","knowledgePolicy":"retrospective_capture_cutoff","fillTimestampBasis":"event_end"});
        let good: TypedBacktestBinding = serde_json::from_value(raw.clone()).unwrap();
        assert!(good.validate());
        assert_eq!(serde_json::to_value(good).unwrap(), raw);
        for (field, value) in [
            ("knowledgePolicy", json!("point_in_time")),
            ("initialPosition", json!("long")),
            ("maxFramesPerStep", json!(65)),
            ("maximumFrames", json!(257)),
            ("quantityMicros", json!(0)),
            ("eventEndMs", json!(5001)),
            ("asOfMs", json!(4999)),
        ] {
            let mut bad = raw.clone();
            bad[field] = value;
            let bad: TypedBacktestBinding = serde_json::from_value(bad).unwrap();
            assert!(!bad.validate(), "{field}");
        }
        let mut unknown = raw;
        unknown["callerSignals"] = json!([true]);
        assert!(serde_json::from_value::<TypedBacktestBinding>(unknown).is_err());
    }
    #[test]
    fn typed_backtest_progress_contention_survives_service_recreation_and_defers() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("progress.sqlite");
        let db = db.to_str().unwrap();
        let s = TradeAssemblyService::from_runtime(db, crate::adapters::local::test_runtime(db));
        let lease = acquire_progress_lease(&s, "typed-backtest:test")
            .unwrap()
            .unwrap();
        let recreated =
            TradeAssemblyService::from_runtime(db, crate::adapters::local::test_runtime(db));
        assert!(acquire_progress_lease(&recreated, "typed-backtest:test")
            .unwrap()
            .is_none());
        assert_eq!(
            recreated
                .runtime()
                .leases
                .current("typed-backtest:test")
                .unwrap(),
            Some(lease.clone())
        );
        assert!(recreated
            .runtime()
            .storage
            .list_json(PROGRESS)
            .unwrap()
            .is_empty());
        s.runtime().leases.release(&lease).unwrap();
        let replacement = acquire_progress_lease(&recreated, "typed-backtest:test")
            .unwrap()
            .unwrap();
        assert!(replacement.fencing_token > lease.fencing_token);
    }
    #[test]
    fn typed_backtest_fill_bases_are_explicit_and_share_end_and_next_open_grid() {
        let raw = json!({"schemaVersion":"tradeassembly.typed-backtest-binding.v1","planRevisionId":"dataplan_original","planRevisionSha256":"a".repeat(64),"sourceCaptures":{"source":{"captureId":"capture_original","receiptSha256":"b".repeat(64)}},"eventStartMs":1000,"eventEndMs":5000,"asOfMs":6000,"intervalMs":1000,"maxFramesPerStep":2,"maximumFrames":4,"quantityMicros":1000000,"initialPosition":"flat","knowledgePolicy":"retrospective_capture_cutoff","fillTimestampBasis":"event_end"});
        let mut binding: TypedBacktestBinding = serde_json::from_value(raw).unwrap();
        assert_eq!(binding.fill_bounds(2000).unwrap(), (1000, 2000));
        assert_eq!(binding.fill_bounds(3000).unwrap().0, 2000);
        binding.fill_timestamp_basis = "event_start".into();
        assert!(binding.validate());
        assert_eq!(binding.fill_bounds(1000).unwrap(), (1000, 2000));
        assert_eq!(binding.fill_bounds(2000).unwrap().0, 2000);
        assert_ne!(
            binding.fill_bounds(2000).unwrap(),
            (1000, 2000),
            "wrong source basis must not align"
        );
        binding.fill_timestamp_basis = "infer".into();
        assert!(!binding.validate());
        assert!(binding.fill_bounds(1000).is_err());
    }
    #[test]
    fn typed_backtest_refuses_unexecuted_trigger_expression_capability_and_tif() {
        let raw = json!({"monitoring":["synthetic"],"triggers":[{"id":"exit","kind":"indicator","scope":"full_position"}],"composition":"any","precedence":[],"end_of_data":"preserve_open_positions"});
        let exit: crate::spec::ExitPolicy = serde_json::from_value(raw.clone()).unwrap();
        let order: crate::spec::OrderPolicy = serde_json::from_value(json!({"allowed_order_types":["market"],"multi_leg_mode":"not_applicable","time_in_force":["day"],"conditional_orders_allowed":false})).unwrap();
        validate_signal_exit_and_tif(&exit, &order, "day").unwrap();
        assert_eq!(
            validate_signal_exit_and_tif(&exit, &order, "gtc").unwrap_err(),
            "typed_backtest_order_tif_unsupported"
        );
        for (field, value) in [
            (
                "expression",
                json!({"language":"cel@1","source":"false","result_type":"bool","max_cost":100}),
            ),
            (
                "capability_requirement_ref",
                json!("required_exit_operation"),
            ),
        ] {
            let mut bad = raw.clone();
            bad["triggers"][0][field] = value;
            let bad: crate::spec::ExitPolicy = serde_json::from_value(bad).unwrap();
            assert_eq!(
                validate_signal_exit_and_tif(&bad, &order, "day").unwrap_err(),
                "typed_backtest_exit_stage_unsupported"
            );
        }
    }
    #[test]
    fn typed_backtest_never_synthesizes_current_human_authority_from_actor_or_storage() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("identity.sqlite");
        let db = db.to_str().unwrap();
        let service =
            TradeAssemblyService::from_runtime(db, crate::adapters::local::test_runtime(db));
        assert_eq!(
            actual_owner(&service).unwrap_err(),
            "typed_backtest_current_owner_required"
        );
        let authenticated =
            service.for_authenticated_invocation("fixture", "actual-human", None, None);
        assert_eq!(
            actual_owner(&authenticated).unwrap().subject,
            "actual-human"
        );
    }
}
