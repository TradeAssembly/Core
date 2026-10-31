//! Explicit data-computation runner protocol. No activation or broker operation.
//! Remote installation authentication is owned by its transport, not authorityRef.
use super::{
    data_evaluation, data_plans, data_recipes, instrument_screens, session_schedules,
    TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, ports::*, session_schedule::SessionOccurrence};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tradeassembly_plugin_sdk::instrument_set::InstrumentSet;
pub const DATA_RUNNER_SCHEMA: &str = "tradeassembly.core-data-runner.v1";
pub const MAX_DATA_COMMAND_BYTES: usize = 262144;
pub const MAX_DATA_RECEIPT_BYTES: usize = 307200;
const COMMANDS: &str = "data_runner_commands_v1";
const BINDINGS: &str = "data_runner_idempotency_v1";
const RECEIPTS: &str = "data_runner_receipts_v1";
const HISTORY: &str = "data_runner_receipt_revisions_v1";
const FENCES: &str = "data_runner_dispatch_fences_v1";
const COMMAND_LEASE_MS: i64 = 120_000;

struct DispatchLease {
    repository: std::sync::Arc<dyn LeaseRepository>,
    claim: LeaseClaim,
}
impl DispatchLease {
    fn current(&self, now: i64) -> Result<(), String> {
        let current = self.repository.current(&self.claim.resource)?;
        if now <= 0 || current.as_ref() != Some(&self.claim) || self.claim.expires_at_ms <= now {
            return Err("data_runner_dispatch_lease_stale".into());
        }
        Ok(())
    }
}
impl Drop for DispatchLease {
    fn drop(&mut self) {
        // A superseded owner cannot release its replacement's fence.
        let _ = self.repository.release(&self.claim);
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataRunnerMode {
    Paper,
    Backtest,
}
impl DataRunnerMode {
    fn name(&self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Backtest => "backtest",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DataRunnerOperation {
    PrepareStrategy {
        scope: data_plans::PlanScope,
        capability_graph_revision: String,
        source_captures: BTreeMap<String, String>,
    },
    PrepareRecipe {
        scope: data_plans::PlanScope,
        program: data_recipes::RecipeProgram,
    },
    EvaluateStrategy {
        step: data_evaluation::EvaluationStep,
    },
    EvaluateRecipe {
        step: data_evaluation::EvaluationStep,
    },
    EvaluateScreen {
        screen: instrument_screens::ScreenCommand,
    },
    PrepareSession {
        schedule: session_schedules::ScheduleCommand,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataRunnerCommand {
    pub schema_version: String,
    pub core_release: String,
    pub workspace_id: String,
    pub command_id: String,
    /// Provenance only. It does not select a principal or grant a permission.
    pub authority_ref: String,
    pub idempotency_key: IdempotencyKey,
    pub mode: DataRunnerMode,
    pub submitted_at_ms: i64,
    pub operation: DataRunnerOperation,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "data_runner_encoding_invalid".into())
}
fn label(v: &str) -> bool {
    !v.is_empty() && v.len() <= 256 && v.trim() == v && !v.chars().any(char::is_control)
}
fn budget(v: &Value, depth: usize, nodes: &mut usize) -> Result<(), String> {
    *nodes += 1;
    if depth > 24 || *nodes > 16384 {
        return Err("data_runner_command_bounds_invalid".into());
    }
    match v {
        Value::Object(m) => {
            if m.len() > 128 {
                return Err("data_runner_command_bounds_invalid".into());
            }
            for (k, v) in m {
                let key = k
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase();
                if [
                    "apikey",
                    "apisecret",
                    "accesstoken",
                    "refreshtoken",
                    "privatekey",
                    "password",
                    "credentials",
                    "authorization",
                    "headers",
                    "config",
                    "db",
                    "shell",
                    "sql",
                ]
                .contains(&key.as_str())
                {
                    return Err("data_runner_credential_or_execution_material_forbidden".into());
                }
                budget(v, depth + 1, nodes)?;
            }
        }
        Value::Array(a) => {
            if a.len() > 4096 {
                return Err("data_runner_command_bounds_invalid".into());
            }
            for v in a {
                budget(v, depth + 1, nodes)?;
            }
        }
        Value::String(s) if s.len() > 16384 => {
            return Err("data_runner_command_bounds_invalid".into())
        }
        _ => {}
    }
    Ok(())
}
impl DataRunnerCommand {
    pub fn parse(bytes: &[u8], expected_release: &str) -> Result<Self, String> {
        if bytes.len() > MAX_DATA_COMMAND_BYTES {
            return Err("data_runner_command_bounds_invalid".into());
        }
        let mut v: Value =
            serde_json::from_slice(bytes).map_err(|_| "data_runner_command_invalid")?;
        budget(&v, 0, &mut 0)?;
        tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut v)
            .map_err(|_| "data_runner_number_invalid")?;
        let c: Self = serde_json::from_value(v).map_err(|_| "data_runner_command_invalid")?;
        c.validate(expected_release)?;
        Ok(c)
    }
    pub fn validate(&self, expected_release: &str) -> Result<(), String> {
        let v = serde_json::to_value(self).map_err(|_| "data_runner_command_invalid")?;
        if serde_json::to_vec(&v)
            .map_err(|_| "data_runner_encoding_invalid")?
            .len()
            > MAX_DATA_COMMAND_BYTES
        {
            return Err("data_runner_command_bounds_invalid".into());
        }
        budget(&v, 0, &mut 0)?;
        if self.schema_version != DATA_RUNNER_SCHEMA {
            return Err("data_runner_version_unsupported".into());
        }
        if !label(expected_release) || self.core_release != expected_release {
            return Err("data_runner_release_mismatch".into());
        }
        if [&self.workspace_id, &self.command_id, &self.authority_ref]
            .iter()
            .any(|s| !label(s))
            || self.submitted_at_ms <= 0
        {
            return Err("data_runner_identity_invalid".into());
        }
        if !label(self.idempotency_key.as_str()) {
            return Err("data_runner_identity_invalid".into());
        }
        hash(self)?;
        Ok(())
    }
    pub fn canonical_hash(&self) -> Result<String, String> {
        hash(self)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DataRunnerResult {
    Plan {
        scope: data_plans::PlanScope,
        revision_id: String,
        revision_sha256: String,
        plan_sha256: String,
        capability_graph_revision: String,
        mode: String,
    },
    Evaluation {
        run_id: String,
        plan_revision_id: String,
        mode: String,
        receipt_sha256: String,
        checkpoint_sha256: String,
        generation: u64,
        status: data_evaluation::EvaluationStatus,
        frames_processed: u64,
        signal_frames: usize,
    },
    Selection {
        scope: data_plans::PlanScope,
        capability_graph_revision: String,
        mode: String,
        selection_id: String,
        receipt_sha256: String,
        plugin_ref: String,
        operation_id: String,
        package_sha256: String,
        manifest_sha256: String,
        contract_sha256: String,
        policy_sha256: String,
        instrument_set: InstrumentSet,
    },
    Session {
        scope: data_plans::PlanScope,
        capability_graph_revision: String,
        mode: String,
        receipt_sha256: String,
        prepared_at_ms: i64,
        occurrence: SessionOccurrence,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataRunnerState {
    Accepted,
    Uncertain,
    Completed,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataRunnerReceipt {
    pub schema_version: String,
    pub workspace_id: String,
    pub command_id: String,
    pub command_sha256: String,
    pub core_release: String,
    pub core_revision: Option<String>,
    pub state: DataRunnerState,
    pub observed_at_ms: i64,
    pub result: Option<DataRunnerResult>,
    pub error: Option<String>,
    pub execution_authority_granted: bool,
    pub receipt_sha256: String,
    pub previous_receipt_sha256: Option<String>,
}
fn receipt_hash(r: &DataRunnerReceipt) -> Result<String, String> {
    let mut r = r.clone();
    r.receipt_sha256.clear();
    hash(&r)
}
fn command_value(c: &DataRunnerCommand) -> Result<Value, String> {
    let mut v = serde_json::to_value(c).map_err(|_| "data_runner_encoding_invalid")?;
    tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut v)
        .map_err(|_| "data_runner_number_invalid")?;
    Ok(v)
}
fn key(workspace: &str, id: &str) -> Result<String, String> {
    hash(&(workspace, id))
}
pub struct DurableCoreDataRunner {
    service: TradeAssemblyService,
    expected_release: String,
}
impl DurableCoreDataRunner {
    pub fn new(service: TradeAssemblyService, expected_release: String) -> Result<Self, String> {
        if !label(&expected_release) {
            return Err("data_runner_release_mismatch".into());
        }
        Ok(Self {
            service,
            expected_release,
        })
    }
    fn body(&self, c: &DataRunnerCommand) -> Result<(&'static str, Value), String> {
        let now = self.service.runtime().clock.now_ms();
        if c.submitted_at_ms > now.saturating_add(5000) {
            return Err("data_runner_submission_future".into());
        }
        let mode = c.mode.name();
        let (path, mut body) = match &c.operation {
            DataRunnerOperation::PrepareStrategy {
                scope,
                capability_graph_revision,
                source_captures,
            } => (
                "/product/strategies/data-plans/prepare",
                json!({"scope":scope,"capabilityGraphRevisionId":capability_graph_revision,"mode":mode,"sourceCaptures":source_captures}),
            ),
            DataRunnerOperation::PrepareRecipe { scope, program } => {
                if program.base_request.mode != mode {
                    return Err("data_runner_mode_mismatch".into());
                }
                (
                    "/product/strategies/data-recipes/prepare",
                    json!({"scope":scope,"program":program}),
                )
            }
            DataRunnerOperation::EvaluateStrategy { step }
            | DataRunnerOperation::EvaluateRecipe { step } => {
                let plan = data_plans::current(&self.service, &step.plan_revision_id)?;
                if plan.content.plan.request.mode != mode {
                    return Err("data_runner_mode_mismatch".into());
                }
                if step.window.as_of_ms > now {
                    return Err("data_runner_window_future".into());
                }
                (
                    if matches!(&c.operation, DataRunnerOperation::EvaluateRecipe { .. }) {
                        "/product/strategies/data-recipes/step"
                    } else {
                        "/product/strategies/data-runs/step"
                    },
                    json!({"step":step}),
                )
            }
            DataRunnerOperation::EvaluateScreen { screen } => {
                if screen.mode != mode {
                    return Err("data_runner_mode_mismatch".into());
                }
                (
                    "/product/strategies/instrument-screens/create",
                    json!({"screen":screen}),
                )
            }
            DataRunnerOperation::PrepareSession { schedule } => {
                if schedule.mode != mode {
                    return Err("data_runner_mode_mismatch".into());
                }
                (
                    "/product/strategies/session-schedules/prepare",
                    json!({"schedule":schedule}),
                )
            }
        };
        // Execution authority and object scope remain those of the service invocation.
        // authorityRef is deliberately absent from this authorization context.
        body["idempotencyKey"] = json!(format!("data-runner:{}", c.canonical_hash()?));
        body["accountMode"] = json!("paper");
        Ok((path, body))
    }
    fn preflight(&self, c: &DataRunnerCommand) -> Result<(&'static str, Value), String> {
        let (path, body) = self.body(c)?;
        if let Some(response) = super::object_authorization::authorize_private_request(
            &self.service,
            "POST",
            path,
            &body,
        ) {
            return Err(super::service_error_code(&response.body)
                .unwrap_or_else(|| "data_runner_owner_unavailable".into()));
        }
        let rejection = super::data_strategy::preflight(&self.service, path, &body)
            .or_else(|| data_recipes::preflight(&self.service, path, &body))
            .or_else(|| instrument_screens::preflight(&self.service, path, &body))
            .or_else(|| session_schedules::preflight(&self.service, path, &body));
        if let Some(response) = rejection {
            return Err(super::service_error_code(&response.body)
                .unwrap_or_else(|| "data_runner_preflight_failed".into()));
        }
        Ok((path, body))
    }
    fn context(&self, c: &DataRunnerCommand) -> Result<SideEffectContext, String> {
        Ok(SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new(format!("data-runner-storage:{}", c.canonical_hash()?))?,
        ))
    }
    fn validate_receipt(&self, c: &DataRunnerCommand, r: &DataRunnerReceipt) -> Result<(), String> {
        if r.schema_version != "tradeassembly.core-data-runner.receipt.v1"
            || r.workspace_id != c.workspace_id
            || r.command_id != c.command_id
            || r.command_sha256 != c.canonical_hash()?
            || r.core_release != c.core_release
            || r.observed_at_ms <= 0
            || r.core_revision.as_ref().is_some_and(|v| {
                v.len() != 40
                    || !v
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
            || r.execution_authority_granted
            || receipt_hash(r)? != r.receipt_sha256
        {
            return Err("data_runner_receipt_invalid".into());
        }
        if !matches!(
            (&r.state, &r.result, &r.error),
            (DataRunnerState::Accepted, None, None)
                | (DataRunnerState::Uncertain, None, Some(_))
                | (DataRunnerState::Completed, Some(_), None)
                | (DataRunnerState::Failed, None, Some(_))
        ) {
            return Err("data_runner_receipt_invalid".into());
        }
        if (r.state == DataRunnerState::Accepted) != r.previous_receipt_sha256.is_none() {
            return Err("data_runner_receipt_invalid".into());
        }
        Ok(())
    }
    fn project(&self, c: &DataRunnerCommand, body: Value) -> Result<DataRunnerResult, String> {
        match &c.operation {
            DataRunnerOperation::PrepareStrategy { .. }
            | DataRunnerOperation::PrepareRecipe { .. } => {
                let p: data_plans::StoredDataPlan = serde_json::from_value(body["plan"].clone())
                    .map_err(|_| "data_runner_plan_projection_invalid")?;
                let current = data_plans::current(&self.service, &p.revision_id)?;
                if current != p {
                    return Err("data_runner_plan_projection_invalid".into());
                }
                let expected = match &c.operation {
                    DataRunnerOperation::PrepareStrategy {
                        scope,
                        capability_graph_revision,
                        source_captures,
                    } => {
                        p.content.scope == *scope
                            && p.content.plan.request.capability_graph_revision
                                == *capability_graph_revision
                            && p.content.plan.request.mode == c.mode.name()
                            && p.content.plan.request.source_captures == *source_captures
                            && p.content.plan.request.selected_outputs.is_empty()
                            && p.content.recipe_source.is_none()
                    }
                    DataRunnerOperation::PrepareRecipe { scope, program } => {
                        data_recipes::prepare(&self.service, scope.clone(), program.clone())?
                            == p.content
                    }
                    _ => false,
                };
                if !expected {
                    return Err("data_runner_plan_projection_invalid".into());
                }

                Ok(DataRunnerResult::Plan {
                    scope: p.content.scope,
                    revision_id: p.revision_id,
                    revision_sha256: p.revision_sha256,
                    plan_sha256: p.content.plan.plan_sha256,
                    capability_graph_revision: p.content.plan.request.capability_graph_revision,
                    mode: p.content.plan.request.mode,
                })
            }
            DataRunnerOperation::EvaluateStrategy { .. }
            | DataRunnerOperation::EvaluateRecipe { .. } => {
                let r: data_evaluation::EvaluationReceipt =
                    serde_json::from_value(body["receipt"].clone())
                        .map_err(|_| "data_runner_evaluation_projection_invalid")?;
                Ok(DataRunnerResult::Evaluation {
                    run_id: r.checkpoint.run_id,
                    plan_revision_id: r.checkpoint.plan_revision_id,
                    mode: c.mode.name().into(),
                    receipt_sha256: r.receipt_sha256,
                    checkpoint_sha256: r.checkpoint.checkpoint_sha256,
                    generation: r.checkpoint.generation,
                    status: r.status,
                    frames_processed: r.frames_processed,
                    signal_frames: r.signals.len(),
                })
            }
            DataRunnerOperation::EvaluateScreen { .. } => {
                let r = instrument_screens::current(
                    &self.service,
                    body["selection"]["selectionId"]
                        .as_str()
                        .ok_or("data_runner_selection_projection_invalid")?,
                )?;
                if !matches!(&c.operation,DataRunnerOperation::EvaluateScreen{screen} if *screen==r.command)
                {
                    return Err("data_runner_selection_projection_invalid".into());
                }
                Ok(DataRunnerResult::Selection {
                    scope: r.command.scope,
                    capability_graph_revision: r.command.capability_graph_revision,
                    mode: r.command.mode,
                    selection_id: r.selection_id,
                    receipt_sha256: r.receipt_sha256,
                    plugin_ref: r.operation.operation.plugin_ref,
                    operation_id: r.operation.operation.operation_id,
                    package_sha256: r.operation.operation.package_sha256,
                    manifest_sha256: r.operation.operation.manifest_sha256,
                    contract_sha256: r.operation.contract_sha256,
                    policy_sha256: r.operation.policy_sha256,
                    instrument_set: r.instrument_set,
                })
            }
            DataRunnerOperation::PrepareSession { .. } => {
                let r = session_schedules::current(
                    &self.service,
                    body["schedule"]["occurrence"]["occurrenceId"]
                        .as_str()
                        .ok_or("data_runner_session_projection_invalid")?,
                )?;
                if !matches!(&c.operation,DataRunnerOperation::PrepareSession{schedule} if *schedule==r.command)
                {
                    return Err("data_runner_session_projection_invalid".into());
                }
                Ok(DataRunnerResult::Session {
                    scope: r.command.scope,
                    capability_graph_revision: r.command.capability_graph_revision,
                    mode: r.command.mode,
                    receipt_sha256: r.receipt_sha256,
                    prepared_at_ms: r.prepared_at_ms,
                    occurrence: r.occurrence,
                })
            }
        }
    }
    fn validate_source_result(
        &self,
        c: &DataRunnerCommand,
        result: &DataRunnerResult,
    ) -> Result<(), String> {
        let body = match result {
            DataRunnerResult::Plan { revision_id, .. } => {
                json!({"plan":data_plans::current(&self.service,revision_id)?})
            }
            DataRunnerResult::Evaluation { .. } => {
                let step = match &c.operation {
                    DataRunnerOperation::EvaluateStrategy { step }
                    | DataRunnerOperation::EvaluateRecipe { step } => step,
                    _ => return Err("data_runner_result_mismatch".into()),
                };
                let (path, body) = self.body(c)?;
                let e = crate::control_plane::http_envelope("http", "POST", path, &body)?;
                let ctx = SideEffectContext::new(
                    e.authority,
                    IdempotencyKey::new(
                        body["idempotencyKey"]
                            .as_str()
                            .ok_or("data_runner_identity_invalid")?,
                    )?,
                );
                json!({"receipt":data_evaluation::inspect_receipt(&self.service,step,&ctx)?})
            }
            DataRunnerResult::Selection { selection_id, .. } => {
                json!({"selection":{"selectionId":selection_id}})
            }
            DataRunnerResult::Session { occurrence, .. } => {
                json!({"schedule":{"occurrence":{"occurrenceId":occurrence.occurrence_id}}})
            }
        };
        if self.project(c, body)? != *result {
            return Err("data_runner_result_mismatch".into());
        }
        Ok(())
    }
    fn load_bound_receipt(&self, c: &DataRunnerCommand) -> Result<DataRunnerReceipt, String> {
        let storage = self.service.runtime().storage.clone();
        let k = key(&c.workspace_id, &c.command_id)?;
        let binding = json!({"commandId":c.command_id,"commandSha256":c.canonical_hash()?});
        if storage.get_json(COMMANDS, &k)? != Some(command_value(c)?)
            || storage.get_json(BINDINGS, &key(&c.workspace_id, c.idempotency_key.as_str())?)?
                != Some(binding)
        {
            return Err("data_runner_idempotency_conflict".into());
        }
        let value = storage
            .get_json(RECEIPTS, &k)?
            .ok_or("data_runner_receipt_missing")?;
        if serde_json::to_vec(&value)
            .map_err(|_| "data_runner_encoding_invalid")?
            .len()
            > MAX_DATA_RECEIPT_BYTES
        {
            return Err("data_runner_receipt_bounds_invalid".into());
        }
        let r: DataRunnerReceipt =
            serde_json::from_value(value.clone()).map_err(|_| "data_runner_receipt_invalid")?;
        self.validate_receipt(c, &r)?;
        if storage.get_json(HISTORY, &r.receipt_sha256)? != Some(value) {
            return Err("data_runner_receipt_revision_missing".into());
        }
        let mut child = r.clone();
        for _ in 0..2 {
            let Some(previous) = child.previous_receipt_sha256.clone() else {
                break;
            };
            let v = storage
                .get_json(HISTORY, &previous)?
                .ok_or("data_runner_receipt_revision_missing")?;
            let parent: DataRunnerReceipt =
                serde_json::from_value(v).map_err(|_| "data_runner_receipt_invalid")?;
            self.validate_receipt(c, &parent)?;
            if parent.receipt_sha256 != previous
                || parent.core_revision != child.core_revision
                || parent.observed_at_ms > child.observed_at_ms
                || !matches!(
                    parent.state,
                    DataRunnerState::Accepted | DataRunnerState::Uncertain
                )
                || child.state == DataRunnerState::Uncertain
                    && parent.state != DataRunnerState::Accepted
            {
                return Err("data_runner_receipt_invalid".into());
            }
            child = parent;
        }
        if child.state != DataRunnerState::Accepted || child.previous_receipt_sha256.is_some() {
            return Err("data_runner_receipt_invalid".into());
        }
        if let Some(result) = &r.result {
            self.validate_source_result(c, result)?;
        }
        Ok(r)
    }
    pub fn dispatch(&self, c: &DataRunnerCommand) -> Result<DataRunnerReceipt, String> {
        c.validate(&self.expected_release)?;
        let (path, body) = self.preflight(c)?;
        let command_hash = c.canonical_hash()?;
        let command_key = key(&c.workspace_id, &c.command_id)?;
        let binding_key = key(&c.workspace_id, c.idempotency_key.as_str())?;
        let ctx = self.context(c)?;
        let storage = self.service.runtime().storage.clone();
        if storage.get_json(COMMANDS, &command_key)?.is_none() {
            let mut accepted = DataRunnerReceipt {
                schema_version: "tradeassembly.core-data-runner.receipt.v1".into(),
                workspace_id: c.workspace_id.clone(),
                command_id: c.command_id.clone(),
                command_sha256: command_hash.clone(),
                core_release: c.core_release.clone(),
                core_revision: crate::build_identity::current().map(|v| v.revision().to_string()),
                state: DataRunnerState::Accepted,
                observed_at_ms: self.service.runtime().clock.now_ms(),
                result: None,
                error: None,
                execution_authority_granted: false,
                receipt_sha256: String::new(),
                previous_receipt_sha256: None,
            };
            accepted.receipt_sha256 = receipt_hash(&accepted)?;
            let value =
                serde_json::to_value(&accepted).map_err(|_| "data_runner_encoding_invalid")?;
            storage
                .put_json_batch(
                    &[
                        StorageWrite::new(COMMANDS, &command_key, command_value(c)?, ctx.clone()),
                        StorageWrite::new(
                            BINDINGS,
                            &binding_key,
                            json!({"commandId":c.command_id,"commandSha256":command_hash}),
                            ctx.clone(),
                        ),
                        StorageWrite::new(RECEIPTS, &command_key, value.clone(), ctx.clone()),
                        StorageWrite::new(HISTORY, &accepted.receipt_sha256, value, ctx.clone()),
                    ],
                    &[
                        StorageExpectation::new(COMMANDS, &command_key, None),
                        StorageExpectation::new(BINDINGS, &binding_key, None),
                        StorageExpectation::new(RECEIPTS, &command_key, None),
                        StorageExpectation::new(HISTORY, &accepted.receipt_sha256, None),
                    ],
                )
                .map_err(|_| "data_runner_accept_commit_failed")?;
            // A concurrent exact admission may win. Verify its whole binding;
            // never interpret a storage conflict as permission to dispatch.
        }
        let accepted = self.load_bound_receipt(c)?;
        if matches!(
            accepted.state,
            DataRunnerState::Completed | DataRunnerState::Failed
        ) {
            return Ok(accepted);
        }
        if accepted.core_revision
            != crate::build_identity::current().map(|v| v.revision().to_string())
        {
            return Err("data_runner_resume_revision_mismatch".into());
        }
        let repository = self.service.runtime().leases.clone();
        let Some(claim) = repository.acquire(
            &format!("data-runner:{command_hash}"),
            &format!("dispatch:{:032x}", rand::random::<u128>()),
            self.service.runtime().clock.now_ms(),
            COMMAND_LEASE_MS,
        )?
        else {
            // Another exact invocation owns the effect. Pending is not failure
            // and must not replace that invocation's eventual result.
            return self.load_bound_receipt(c);
        };
        let lease = DispatchLease { repository, claim };
        lease.current(self.service.runtime().clock.now_ms())?;
        let accepted = self.load_bound_receipt(c)?;
        if matches!(
            accepted.state,
            DataRunnerState::Completed | DataRunnerState::Failed
        ) {
            return Ok(accepted);
        }
        let prior_fence = storage.get_json(FENCES, &command_key)?;
        if prior_fence.as_ref().is_some_and(|v| {
            v["fencingToken"]
                .as_i64()
                .is_none_or(|token| token >= lease.claim.fencing_token)
        }) {
            return Err("data_runner_dispatch_fence_invalid".into());
        }
        let fence =
            serde_json::to_value(&lease.claim).map_err(|_| "data_runner_encoding_invalid")?;
        if storage.put_json_batch(
            &[StorageWrite::new(
                FENCES,
                &command_key,
                fence.clone(),
                ctx.clone(),
            )],
            &[StorageExpectation::new(FENCES, &command_key, prior_fence)],
        )? != ComparePutOutcome::Updated
        {
            return Err("data_runner_dispatch_fence_conflict".into());
        }
        let response = self
            .service
            .handle_http_from_source("core-data-runner", "POST", path, body);
        let (state, result, error) = if response.status >= 500
            || response.body["error"]["code"] == "idempotency_in_progress"
            || response.status < 400 && response.body["executionAuthorityGranted"] != false
        {
            (
                DataRunnerState::Uncertain,
                None,
                Some("data_runner_upstream_outcome_uncertain".to_string()),
            )
        } else {
            let result = if response.status >= 400 {
                Err(super::service_error_code(&response.body)
                    .unwrap_or_else(|| "data_runner_upstream_failed".into()))
            } else {
                self.project(c, response.body)
            };
            match result {
                Ok(result) => (DataRunnerState::Completed, Some(result), None),
                Err(error) => (DataRunnerState::Failed, None, Some(error)),
            }
        };
        if state == DataRunnerState::Uncertain && accepted.state == DataRunnerState::Uncertain {
            lease.current(self.service.runtime().clock.now_ms())?;
            return Ok(accepted);
        }
        let mut r = DataRunnerReceipt {
            schema_version: "tradeassembly.core-data-runner.receipt.v1".into(),
            workspace_id: c.workspace_id.clone(),
            command_id: c.command_id.clone(),
            command_sha256: command_hash,
            core_release: c.core_release.clone(),
            core_revision: crate::build_identity::current().map(|v| v.revision().to_string()),
            state,
            observed_at_ms: self.service.runtime().clock.now_ms(),
            result,
            error,
            execution_authority_granted: false,
            receipt_sha256: String::new(),
            previous_receipt_sha256: Some(accepted.receipt_sha256.clone()),
        };
        r.receipt_sha256 = receipt_hash(&r)?;
        let value = serde_json::to_value(&r).map_err(|_| "data_runner_encoding_invalid")?;
        if serde_json::to_vec(&value)
            .map_err(|_| "data_runner_encoding_invalid")?
            .len()
            > MAX_DATA_RECEIPT_BYTES
        {
            return Err("data_runner_receipt_bounds_invalid".into());
        }
        lease.current(self.service.runtime().clock.now_ms())?;
        match storage
            .put_json_batch(
                &[
                    StorageWrite::new(RECEIPTS, &command_key, value.clone(), ctx.clone()),
                    StorageWrite::new(HISTORY, &r.receipt_sha256, value, ctx),
                ],
                &[
                    StorageExpectation::new(FENCES, &command_key, Some(fence)),
                    StorageExpectation::new(
                        RECEIPTS,
                        &command_key,
                        Some(
                            serde_json::to_value(&accepted)
                                .map_err(|_| "data_runner_encoding_invalid")?,
                        ),
                    ),
                    StorageExpectation::new(HISTORY, &r.receipt_sha256, None),
                ],
            )
            .map_err(|_| "data_runner_complete_commit_failed")?
        {
            ComparePutOutcome::Updated => Ok(r),
            ComparePutOutcome::Conflict => {
                let prior = self.load_bound_receipt(c)?;
                if r.state == DataRunnerState::Uncertain
                    && matches!(
                        prior.state,
                        DataRunnerState::Completed | DataRunnerState::Failed
                    )
                {
                    return Ok(prior);
                }
                if prior.state == DataRunnerState::Accepted
                    || prior.result != r.result
                    || prior.error != r.error
                {
                    return Err("data_runner_receipt_conflict".into());
                }
                Ok(prior)
            }
        }
    }
    pub fn receipt(
        &self,
        workspace: &str,
        command_id: &str,
    ) -> Result<Option<DataRunnerReceipt>, String> {
        if !label(workspace) || !label(command_id) {
            return Err("data_runner_identity_invalid".into());
        }
        let k = key(workspace, command_id)?;
        let Some(v) = self.service.runtime().storage.get_json(COMMANDS, &k)? else {
            return Ok(None);
        };
        let c: DataRunnerCommand =
            serde_json::from_value(v).map_err(|_| "data_runner_command_invalid")?;
        if c.workspace_id != workspace || c.command_id != command_id {
            return Err("data_runner_receipt_lookup_mismatch".into());
        }
        c.validate(&self.expected_release)?;
        self.preflight(&c)?;
        Ok(Some(self.load_bound_receipt(&c)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dispatch_lease_expiry_and_replacement_fences_old_owner() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("lease.db").to_string_lossy().into_owned();
        let runtime = crate::adapters::local::test_runtime(&db);
        let repository = runtime.leases.clone();
        let claim = repository
            .acquire("command", "first", 1000, 100)
            .unwrap()
            .unwrap();
        let old = DispatchLease {
            repository: repository.clone(),
            claim,
        };
        old.current(1001).unwrap();
        assert_eq!(
            old.current(1100).unwrap_err(),
            "data_runner_dispatch_lease_stale"
        );
        assert!(repository
            .acquire("command", "second", 1001, 100)
            .unwrap()
            .is_none());
        let replacement = repository
            .acquire("command", "second", 1101, 100)
            .unwrap()
            .unwrap();
        assert!(replacement.fencing_token > old.claim.fencing_token);
        assert!(old.current(1102).is_err());
        drop(old);
        assert_eq!(repository.current("command").unwrap(), Some(replacement));
    }
    fn command() -> Value {
        json!({"schemaVersion":DATA_RUNNER_SCHEMA,"coreRelease":"fixture.release","workspaceId":"fixture.workspace","commandId":"fixture.command","authorityRef":"provenance.only","idempotencyKey":"fixture.key","mode":"paper","submittedAtMs":1000,"operation":{"kind":"prepare_session","schedule":{"scope":{"strategyId":"fixture.strategy","strategyVersionId":"fixture.version","strategySpecHash":"sha256:fixture"},"capabilityGraphRevision":"fixture.graph","mode":"paper","triggerRef":"preopen","sessionDate":"2026-10-31","calendarCollectionId":"fixture.calendar","executionCalendarCollectionId":"fixture.calendar","knowledgeCutoffMs":1000}}})
    }
    #[test]
    fn explicit_data_wire_rejects_live_broker_unknown_and_material_fields() {
        let good = command();
        assert!(
            DataRunnerCommand::parse(&serde_json::to_vec(&good).unwrap(), "fixture.release")
                .is_ok()
        );
        for (pointer, value) in [
            ("/mode", json!("live")),
            ("/operation/kind", json!("submit_order")),
            ("/schemaVersion", json!("tradeassembly.core-data-runner.v2")),
            ("/idempotencyKey", json!("")),
            ("/coreRelease", json!("other")),
        ] {
            let mut v = good.clone();
            *v.pointer_mut(pointer).unwrap() = value;
            assert!(
                DataRunnerCommand::parse(&serde_json::to_vec(&v).unwrap(), "fixture.release")
                    .is_err()
            );
        }
        let mut v = good.clone();
        v["sql"] = json!("forbidden fixture material");
        assert!(
            DataRunnerCommand::parse(&serde_json::to_vec(&v).unwrap(), "fixture.release").is_err()
        );
        let mut v = good;
        v["operation"]["api_key"] = json!("fake-only");
        assert!(
            DataRunnerCommand::parse(&serde_json::to_vec(&v).unwrap(), "fixture.release").is_err()
        );
    }
    #[test]
    fn normalized_integral_command_forms_share_identity_and_bounds_are_explicit() {
        let a = command();
        let mut b = a.clone();
        b["submittedAtMs"] = json!(1000.0);
        let a =
            DataRunnerCommand::parse(&serde_json::to_vec(&a).unwrap(), "fixture.release").unwrap();
        let b =
            DataRunnerCommand::parse(&serde_json::to_vec(&b).unwrap(), "fixture.release").unwrap();
        assert_eq!(a.canonical_hash().unwrap(), b.canonical_hash().unwrap());
        assert!(DataRunnerCommand::parse(
            &vec![b' '; MAX_DATA_COMMAND_BYTES + 1],
            "fixture.release"
        )
        .is_err());
    }
}
