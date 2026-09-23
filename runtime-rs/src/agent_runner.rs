// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Local, all-active agent deployment supervisor. It is deliberately adapter-neutral:
//! a future Codex adapter receives only the opaque session reference and redacted trigger.

use crate::ports::{
    AuthorityContext, EventAppend, IdempotencyKey, ImmutablePutOutcome, LeaseClaim, OutboxRecord,
    ServiceRuntime, SideEffectContext,
};
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const DEPLOYMENTS_NS: &str = "agent_deployments";
pub const RUNS_NS: &str = "agent_runs";
pub const ACTIVE_RUNS_NS: &str = "agent_active_runs";
pub const SCHEDULES_NS: &str = "agent_deployment_schedules";
pub const MCP_CAPABILITIES_NS: &str = "agent_mcp_capabilities";
pub const RECOVERY_RECEIPTS_NS: &str = "agent_recovery_receipts";
pub const SCHEMA_VERSION: &str = "tradeassembly.agent_runner.v1";
pub const DEFAULT_LEASE_MS: i64 = 90_000;
pub const DEFAULT_HEARTBEAT_MS: u64 = 30_000;
pub const DEFAULT_RUN_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
const MILLIS_PER_MINUTE: i64 = 60_000;
const MAX_CRON_EXPRESSION_BYTES: usize = 160;
const MAX_CRON_FIELD_TERMS: usize = 32;
const MAX_CRON_LOOKAHEAD_MINUTES: usize = 366 * 24 * 60;
const MAX_COALESCED_TICKS: u64 = 1_024;
const MAX_CRON_MISSED_SCAN_MINUTES: usize = 7 * 24 * 60;
pub const MCP_CAPABILITY_ENV: &str = "TRADEASSEMBLY_AGENT_MCP_CAPABILITY";

pub mod external_session;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentRunnerTiming {
    pub lease_ms: i64,
    pub heartbeat_ms: u64,
}

struct LeaseHeartbeat<'a> {
    adapter: &'a dyn AgentRuntimePort,
    lease: &'a LeaseClaim,
    started_at_ms: i64,
    timing: AgentRunnerTiming,
}

impl Default for AgentRunnerTiming {
    fn default() -> Self {
        Self {
            lease_ms: DEFAULT_LEASE_MS,
            heartbeat_ms: DEFAULT_HEARTBEAT_MS,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutor {
    #[default]
    Supervised,
    ExternalClient,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeployment {
    /// Who owns the agent process, independently of strategy evaluation mode.
    #[serde(default)]
    pub executor: AgentExecutor,
    pub deployment_id: String,
    pub system_project_id: String,
    pub agent_definition_version_id: String,
    pub execution_config_version_id: String,
    /// The exact MCP tools that this deployment may invoke. Broker tools remain
    /// customer-owned and out of the Studio control plane.
    #[serde(default)]
    pub studio_tool_allowlist: Vec<String>,
    pub desired_state: String,
    /// The legacy/default schedule. It must be at least sixty seconds when a
    /// UTC cron schedule is not supplied. A validated `cron_utc` takes
    /// precedence when present so existing deployment JSON remains compatible.
    #[serde(default = "default_interval_seconds")]
    pub interval_seconds: u64,
    /// A bounded, five-field, minute-resolution cron expression interpreted in
    /// UTC. Seconds fields and time-zone overrides are intentionally rejected.
    #[serde(default)]
    pub cron_utc: Option<String>,
    pub mode: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub runtime_profile: String,
}

fn default_interval_seconds() -> u64 {
    60
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentScheduleState {
    #[serde(default)]
    schema_version: String,
    #[serde(default)]
    schedule_kind: String,
    #[serde(default)]
    schedule_digest: String,
    #[serde(default)]
    next_due_at_ms: i64,
    #[serde(default)]
    last_due_at_ms: Option<i64>,
    #[serde(default)]
    missed_tick_count: u64,
    #[serde(default)]
    missed_tick_count_capped: bool,
    #[serde(default)]
    updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DeploymentSchedule {
    Interval { seconds: u64 },
    CronUtc(CronUtcSchedule),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScheduleDue {
    state: AgentScheduleState,
    scheduled_for_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScheduleDecision {
    NotDue {
        state: AgentScheduleState,
        persist: bool,
    },
    Due(ScheduleDue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CronUtcSchedule {
    expression: String,
    minute: CronField,
    hour: CronField,
    day_of_month: CronField,
    month: CronField,
    day_of_week: CronField,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CronField {
    minimum: u8,
    allowed: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub run_id: String,
    pub deployment_id: String,
    pub trigger_id: String,
    pub state: String,
    pub lease_fence: i64,
    #[serde(default)]
    pub deployment_binding_digest: String,
    pub codex_session_ref: Option<String>,
}

/// Verified, runner-issued authority for a single in-flight Codex child. The
/// opaque capability itself is never stored in this structure or returned to
/// an MCP client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAgentMcpExecutionContext {
    deployment_id: String,
    run_id: String,
    binding_digest: String,
    mode: String,
    studio_tool_allowlist: Vec<String>,
    capability_digest: String,
}

impl VerifiedAgentMcpExecutionContext {
    /// Validate the supervisor's actual current lease, not merely the cached
    /// active pointer. A stale child must not act after its lease is replaced.
    pub fn current_lease(
        &self,
        storage: &dyn crate::ports::StoragePort,
        clock: &dyn crate::ports::ClockPort,
        leases: &dyn crate::ports::LeaseRepository,
    ) -> Result<LeaseClaim, String> {
        self.revalidate(storage, clock)?;
        let invalid = || "agent_run_lease_invalid".to_string();
        let run = storage
            .get_json(RUNS_NS, self.run_id())?
            .ok_or_else(invalid)?;
        let active = storage
            .get_json(ACTIVE_RUNS_NS, self.deployment_id())?
            .ok_or_else(invalid)?;
        let deployment = storage
            .get_json(DEPLOYMENTS_NS, self.deployment_id())?
            .ok_or_else(invalid)?;
        if deployment["deployment"]["desiredState"] != "active" {
            return Err("agent_deployment_not_active".into());
        }
        let deployment: AgentDeployment =
            serde_json::from_value(deployment["deployment"].clone()).map_err(|_| invalid())?;
        if deployment_binding_digest(&deployment) != self.binding_digest
            || run["run"]["runId"] != self.run_id
            || run["run"]["deploymentId"] != self.deployment_id
            || run["run"]["state"] != "running"
            || run["mcpCapabilityDigest"] != self.capability_digest
            || active["runId"] != self.run_id
            || active["state"] != "running"
        {
            return Err(invalid());
        }
        let fence = run["run"]["leaseFence"]
            .as_i64()
            .filter(|f| *f > 0)
            .ok_or_else(invalid)?;
        let resource = format!("agent-deployment:{}", self.deployment_id());
        let lease = leases.current(&resource)?.ok_or_else(invalid)?;
        if active["leaseFence"].as_i64() != Some(fence)
            || lease.resource != resource
            || lease.owner.trim().is_empty()
            || lease.fencing_token != fence
            || lease.expires_at_ms <= clock.trusted_now_ms()?
        {
            return Err(invalid());
        }
        Ok(lease)
    }

    /// Rechecks the capability at the side-effect boundary using portable
    /// durable state and a trusted clock. No deterministic scheduler attempt
    /// is required for a supervised agent run.
    pub fn revalidate(
        &self,
        storage: &dyn crate::ports::StoragePort,
        clock: &dyn crate::ports::ClockPort,
    ) -> Result<(), String> {
        let current = verified_mcp_execution_context_from_ports(
            storage,
            clock.trusted_now_ms()?,
            self.capability_digest(),
        )?;
        if current == *self {
            Ok(())
        } else {
            Err("agent_mcp_execution_context_invalid".to_string())
        }
    }

    pub fn allowed_tools(&self) -> &[String] {
        &self.studio_tool_allowlist
    }

    pub(crate) fn deployment_id(&self) -> &str {
        &self.deployment_id
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    pub(crate) fn mode(&self) -> &str {
        &self.mode
    }

    fn capability_digest(&self) -> &str {
        &self.capability_digest
    }
}

/// Ephemeral opaque capability passed only through the runner-spawned Codex
/// environment. Persist only its SHA-256 digest.
pub struct AgentMcpExecutionCapability {
    token: String,
}

impl AgentMcpExecutionCapability {
    /// Resolve this runner-issued capability for an in-process tool adapter.
    /// This exposes no bearer token and still checks the current durable run,
    /// lease and deployment binding on every resolution.
    pub fn verified_context(
        &self,
        runtime: &ServiceRuntime,
    ) -> Result<VerifiedAgentMcpExecutionContext, String> {
        verified_mcp_execution_context_for_digest(runtime, &self.digest())
    }

    fn issue() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self {
            token: bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        }
    }

    fn digest(&self) -> String {
        mcp_capability_digest(&self.token)
    }

    fn token(&self) -> &str {
        &self.token
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMcpCapabilityRecord {
    schema_version: String,
    deployment_id: String,
    run_id: String,
    binding_digest: String,
    mode: String,
    studio_tool_allowlist: Vec<String>,
    capability_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRecoveryReceipt {
    schema_version: String,
    deployment_id: String,
    run_id: String,
    request_digest: String,
    requested_at_ms: i64,
    state: String,
    receipt: Value,
}

/// Runtime adapters may create/resume an opaque session. They never receive broker material.
pub trait AgentRuntimePort: Send + Sync {
    fn start_or_resume(&self, run: &AgentRun, trigger: &Value) -> Result<Option<String>, String>;

    /// Default adapters stay compatible and deliberately receive no raw
    /// capability. The installed Codex adapter overrides this to pass a
    /// runner-issued opaque token to its inherited MCP subprocess.
    fn start_or_resume_with_mcp_execution_context(
        &self,
        run: &AgentRun,
        trigger: &Value,
        _context: &AgentMcpExecutionCapability,
    ) -> Result<Option<String>, String> {
        self.start_or_resume(run, trigger)
    }
}

/// Local adapter for the installed Codex CLI. Its output is reduced to a session id only.
pub struct CodexCliAdapter {
    executable: String,
    config: String,
    database: String,
}

impl AgentRuntimePort for CodexCliAdapter {
    fn start_or_resume(&self, run: &AgentRun, trigger: &Value) -> Result<Option<String>, String> {
        self.start_or_resume_with_optional_mcp_execution_context(run, trigger, None)
    }

    fn start_or_resume_with_mcp_execution_context(
        &self,
        run: &AgentRun,
        trigger: &Value,
        context: &AgentMcpExecutionCapability,
    ) -> Result<Option<String>, String> {
        self.start_or_resume_with_optional_mcp_execution_context(run, trigger, Some(context))
    }
}

impl CodexCliAdapter {
    pub fn new(executable: &Path, config: &Path, database: &Path) -> Result<Self, String> {
        fn checked(path: &Path) -> Result<String, String> {
            if !path.is_absolute() || !path.is_file() {
                return Err("agent_mcp_binding_path_invalid".into());
            }
            path.canonicalize()
                .ok()
                .and_then(|path| path.to_str().map(str::to_owned))
                .ok_or_else(|| "agent_mcp_binding_path_invalid".into())
        }
        Ok(Self {
            executable: checked(executable)?,
            config: checked(config)?,
            database: checked(database)?,
        })
    }

    /// A fresh private server name avoids Codex's recursive table merge retaining
    /// an old transport or environment. Other user-configured servers stay intact.
    /// Only the capability variable's NAME is exposed here, never its value.
    pub fn command_args(&self, run: &AgentRun, trigger: &Value) -> Result<Vec<String>, String> {
        let mut nonce = [0_u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let server = format!("tradeassembly_runner_{:032x}", u128::from_be_bytes(nonce));
        let mut trigger = trigger.clone();
        let object = trigger
            .as_object_mut()
            .ok_or_else(|| "agent_trigger_invalid".to_string())?;
        object.insert("tradeAssemblyMcpServer".into(), json!(server));
        object.insert("tradeAssemblyMcpInstruction".into(), json!(
            "Use tradeAssemblyMcpServer for this deployment's TradeAssembly tools. Other server bindings are not this installation."
        ));
        let mcp_args = json!([
            "--config",
            self.config,
            "--db",
            self.database,
            "mcp",
            "serve",
            "--transport",
            "stdio"
        ]);
        // JSON string/array encoding is also valid TOML basic-string/array syntax.
        let setting = format!(
            "mcp_servers.{server}={{command={},args={},env_vars=[\"{}\"],enabled=true,required=true}}",
            json!(self.executable), mcp_args, MCP_CAPABILITY_ENV
        );
        let mut args = vec!["-c".into(), setting];
        args.extend(codex_command_args(run, &trigger)?);
        Ok(args)
    }

    fn start_or_resume_with_optional_mcp_execution_context(
        &self,
        run: &AgentRun,
        trigger: &Value,
        context: Option<&AgentMcpExecutionCapability>,
    ) -> Result<Option<String>, String> {
        let body = run_codex_command(self.command_args(run, trigger)?, context)?;
        if body.is_none() {
            return Err("codex_cli_failed".to_string());
        }
        if let Some(session) = run.codex_session_ref.clone() {
            return Ok(Some(session));
        }
        extract_session_ref(body.as_deref().unwrap_or_default())
            .map(Some)
            .ok_or_else(|| "codex_session_reference_missing".to_string())
    }
}

/// Runs the local Codex process with a bounded wait. Stderr is intentionally
/// discarded because providers may include user-controlled material in errors.
/// The caller maintains the durable deployment lease while this child runs.
fn run_codex_command(
    args: Vec<String>,
    context: Option<&AgentMcpExecutionCapability>,
) -> Result<Option<String>, String> {
    let codex_bin = match std::env::var_os("TRADEASSEMBLY_CODEX_BIN") {
        Some(path) if Path::new(&path).is_absolute() => {
            resolve_codex_executable(Path::new(&path))?.into_os_string()
        }
        Some(_) => return Err("codex_cli_unavailable".to_string()),
        None => "codex".into(),
    };
    let mut command = Command::new(codex_bin);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(context) = context {
        command.env(MCP_CAPABILITY_ENV, context.token());
    } else {
        command.env_remove(MCP_CAPABILITY_ENV);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "codex_cli_unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex_cli_output_invalid".to_string())?;
    let reader = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut body = Vec::new();
        let mut chunk = [0_u8; 8_192];
        loop {
            let read = stdout
                .read(&mut chunk)
                .map_err(|_| "codex_cli_output_invalid".to_string())?;
            if read == 0 {
                break;
            }
            let remaining = DEFAULT_CODEX_OUTPUT_BYTES.saturating_sub(body.len() as u64) as usize;
            body.extend_from_slice(&chunk[..read.min(remaining)]);
        }
        String::from_utf8(body).map_err(|_| "codex_cli_output_invalid".to_string())
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= Duration::from_millis(DEFAULT_RUN_TIMEOUT_MS) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("codex_cli_timeout".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("codex_cli_failed".to_string());
            }
        }
    };
    let body = reader
        .join()
        .map_err(|_| "codex_cli_output_invalid".to_string())??;
    if status.success() {
        Ok(Some(body))
    } else {
        Ok(None)
    }
}

const DEFAULT_CODEX_OUTPUT_BYTES: u64 = 1_000_000;

/// npm's Codex entrypoint uses `/usr/bin/env node`, which is not durable in
/// launchd's environment. Resolve only the recognized official package layout;
/// do not execute a shell or search the machine for a replacement runtime.
pub fn resolve_codex_executable(path: &Path) -> Result<std::path::PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|_| "codex_cli_unavailable".to_string())?;
    if canonical.file_name().is_none_or(|name| name != "codex.js") {
        return Ok(canonical);
    }
    let package = canonical
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "codex_cli_unavailable".to_string())?;
    let metadata: Value = std::fs::read(package.join("package.json"))
        .ok()
        .and_then(|body| serde_json::from_slice(&body).ok())
        .ok_or_else(|| "codex_cli_unavailable".to_string())?;
    if metadata["name"] != "@openai/codex" {
        return Err("codex_cli_unavailable".into());
    }
    let (platform, target) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => ("darwin-arm64", "aarch64-apple-darwin"),
        ("macos", "x86_64") => ("darwin-x64", "x86_64-apple-darwin"),
        ("linux", "aarch64") => ("linux-arm64", "aarch64-unknown-linux-musl"),
        ("linux", "x86_64") => ("linux-x64", "x86_64-unknown-linux-musl"),
        _ => return Err("codex_cli_unavailable".into()),
    };
    for vendor in [
        package.join(format!("node_modules/@openai/codex-{platform}/vendor")),
        package.join("vendor"),
    ] {
        let binary = vendor.join(target).join("bin/codex");
        if binary.is_file() {
            return binary
                .canonicalize()
                .map_err(|_| "codex_cli_unavailable".into());
        }
    }
    Err("codex_cli_unavailable".into())
}

#[derive(Clone, Debug)]
pub struct AgentEntitlementDecision {
    pub allowed: bool,
    pub reference: String,
}
pub trait AgentEntitlementPort: Send + Sync {
    fn authorize(&self, deployment: &AgentDeployment) -> AgentEntitlementDecision;
}
pub struct LocalEntitlement;
impl AgentEntitlementPort for LocalEntitlement {
    fn authorize(&self, deployment: &AgentDeployment) -> AgentEntitlementDecision {
        AgentEntitlementDecision {
            allowed: true,
            reference: format!("local-noop:{}", deployment.deployment_id),
        }
    }
}

pub fn codex_command_args(run: &AgentRun, trigger: &Value) -> Result<Vec<String>, String> {
    let prompt =
        serde_json::to_string(trigger).map_err(|_| "agent_trigger_encode_failed".to_string())?;
    let workspace = trigger["workspace"].as_str().unwrap_or_default();
    let profile = trigger["runtimeProfile"].as_str().unwrap_or_default();
    if let Some(session) = run.codex_session_ref.as_deref() {
        // A local workspace may contain data without a Git repository.
        Ok(vec![
            "exec".into(),
            "resume".into(),
            "--skip-git-repo-check".into(),
            session.into(),
            prompt,
        ])
    } else {
        let mut args = vec!["exec".into(), "--json".into()];
        if !workspace.is_empty() {
            args.extend(["-C".into(), workspace.into()]);
        }
        if !profile.is_empty() {
            args.extend(["-p".into(), profile.into()]);
        }
        args.push("--skip-git-repo-check".into());
        args.push(prompt);
        Ok(args)
    }
}

pub fn put_deployment(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
) -> Result<(), String> {
    put_deployment_with_context(
        runtime,
        deployment,
        &context(&format!("deployment:{}", deployment.deployment_id)),
    )
}

/// Persists a deployment through the caller's durable authority context.
/// Versioned strategy/tool bindings may not be changed in place while the
/// deployment is active or has an in-flight/quarantined run; callers must
/// create a new deployment revision instead.
pub fn put_deployment_with_context(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
    side_effect_context: &SideEffectContext,
) -> Result<(), String> {
    let schedule = if deployment.executor == AgentExecutor::Supervised {
        Some(deployment_schedule(deployment)?)
    } else {
        None
    };
    if deployment.deployment_id.trim().is_empty()
        || deployment.system_project_id.trim().is_empty()
        || deployment.agent_definition_version_id.trim().is_empty()
        || deployment.execution_config_version_id.trim().is_empty()
        || deployment.studio_tool_allowlist.is_empty()
        || deployment
            .studio_tool_allowlist
            .iter()
            .any(|tool| !valid_agent_tool_id(tool))
        || !matches!(deployment.mode.as_str(), "paper" | "live")
        || (deployment.executor == AgentExecutor::Supervised
            && (deployment.workspace.trim().is_empty()
                || !Path::new(&deployment.workspace).is_dir()
                || deployment.prompt.trim().is_empty()))
        || secret_shaped(&deployment.prompt)
        || !matches!(
            deployment.desired_state.as_str(),
            "active" | "paused" | "stopped"
        )
    {
        return Err("agent_deployment_invalid".to_string());
    }
    let binding_digest = deployment_binding_digest(deployment);
    if let Some(existing) = runtime
        .storage
        .get_json(DEPLOYMENTS_NS, &deployment.deployment_id)?
    {
        let existing_deployment = existing
            .get("deployment")
            .cloned()
            .and_then(|value| serde_json::from_value::<AgentDeployment>(value).ok())
            .ok_or_else(|| "agent_deployment_invalid".to_string())?;
        let existing_binding = existing
            .get("bindingDigest")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| deployment_binding_digest(&existing_deployment));
        if existing_binding != binding_digest {
            let has_active_run = runtime
                .storage
                .get_json(ACTIVE_RUNS_NS, &deployment.deployment_id)?
                .is_some_and(|active| {
                    matches!(
                        active.get("state").and_then(Value::as_str),
                        Some("running" | "pending_reconcile")
                    )
                });
            if existing_deployment.desired_state == "active" || has_active_run {
                return Err("agent_deployment_binding_immutable".to_string());
            }
        }
    }
    runtime.storage.put_json(
        DEPLOYMENTS_NS,
        &deployment.deployment_id,
        json!({
            "schemaVersion": SCHEMA_VERSION,
            "kind": "AgentDeployment",
            "orchestrator": "agent",
            "host": "local",
            "deployment": deployment,
            "bindingDigest": binding_digest,
            "credentialPosture": "customer-managed; no credential material is persisted in agent records",
        }),
        side_effect_context,
    )?;
    if let Some(schedule) = schedule {
        if runtime
            .storage
            .get_json(SCHEDULES_NS, &deployment.deployment_id)?
            .is_some()
        {
            return Ok(());
        }
        runtime.storage.put_json(
            SCHEDULES_NS,
            &deployment.deployment_id,
            schedule_state_value(&initial_schedule_state(&schedule)),
            side_effect_context,
        )?;
    }
    Ok(())
}

/// Stable identity for the versioned inputs and Studio-side tool authority that
/// must remain unchanged while a run is active or resumed.
pub fn deployment_binding_digest(deployment: &AgentDeployment) -> String {
    let mut tool_ids = deployment.studio_tool_allowlist.clone();
    tool_ids.sort();
    tool_ids.dedup();
    let mut binding = json!({
        "systemProjectId": deployment.system_project_id,
        "agentDefinitionVersionId": deployment.agent_definition_version_id,
        "executionConfigVersionId": deployment.execution_config_version_id,
        "studioToolAllowlist": tool_ids,
    });
    // Preserve legacy supervised digests. External ownership is a new binding.
    if deployment.executor == AgentExecutor::ExternalClient {
        binding["executor"] = json!("external_client");
    }
    short_hash(&binding.to_string())
}

fn valid_agent_tool_id(tool: &str) -> bool {
    tool.len() <= 160
        && tool.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
        && crate::mcp::tool_names()
            .into_iter()
            .any(|known| known == tool)
}

fn canonical_allowlist(allowlist: &[String]) -> Vec<String> {
    let mut allowlist = allowlist.to_vec();
    allowlist.sort();
    allowlist.dedup();
    allowlist
}

/// Resolves the opaque capability inherited by a runner-spawned Codex process.
/// A missing capability deliberately means generic local MCP behavior; a
/// supplied but invalid capability fails closed.
pub fn resolve_mcp_execution_context(
    runtime: &ServiceRuntime,
    token: Option<&str>,
) -> Result<Option<VerifiedAgentMcpExecutionContext>, String> {
    let Some(token) = token else {
        return Ok(None);
    };
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("agent_mcp_execution_context_invalid".to_string());
    }
    verified_mcp_execution_context_for_digest(runtime, &mcp_capability_digest(token)).map(Some)
}

fn verified_mcp_execution_context_for_digest(
    runtime: &ServiceRuntime,
    capability_digest: &str,
) -> Result<VerifiedAgentMcpExecutionContext, String> {
    verified_mcp_execution_context_from_ports(
        runtime.storage.as_ref(),
        runtime.clock.trusted_now_ms()?,
        capability_digest,
    )
}

fn verified_mcp_execution_context_from_ports(
    storage: &dyn crate::ports::StoragePort,
    now_ms: i64,
    capability_digest: &str,
) -> Result<VerifiedAgentMcpExecutionContext, String> {
    if now_ms < 0 {
        return Err("agent_mcp_execution_clock_invalid".to_string());
    }
    let record = storage
        .get_json(MCP_CAPABILITIES_NS, capability_digest)?
        .ok_or_else(|| "agent_mcp_execution_context_invalid".to_string())?;
    let record = serde_json::from_value::<AgentMcpCapabilityRecord>(record)
        .map_err(|_| "agent_mcp_execution_context_invalid".to_string())?;
    if record.schema_version != SCHEMA_VERSION
        || record.capability_digest != capability_digest
        || record.deployment_id.trim().is_empty()
        || record.run_id.trim().is_empty()
        || record.binding_digest.trim().is_empty()
        || !matches!(record.mode.as_str(), "paper" | "live")
        || record.studio_tool_allowlist.is_empty()
        || record
            .studio_tool_allowlist
            .iter()
            .any(|tool| !valid_agent_tool_id(tool))
    {
        return Err("agent_mcp_execution_context_invalid".to_string());
    }
    let run = storage
        .get_json(RUNS_NS, &record.run_id)?
        .ok_or_else(|| "agent_mcp_execution_context_invalid".to_string())?;
    if run.pointer("/run/runId").and_then(Value::as_str) != Some(record.run_id.as_str())
        || run.pointer("/run/deploymentId").and_then(Value::as_str)
            != Some(record.deployment_id.as_str())
        || run.pointer("/run/state").and_then(Value::as_str) != Some("running")
        || run.get("bindingDigest").and_then(Value::as_str) != Some(record.binding_digest.as_str())
        || run.get("mcpCapabilityDigest").and_then(Value::as_str) != Some(capability_digest)
    {
        return Err("agent_mcp_execution_context_invalid".to_string());
    }
    let active = storage
        .get_json(ACTIVE_RUNS_NS, &record.deployment_id)?
        .ok_or_else(|| "agent_mcp_execution_context_invalid".to_string())?;
    if active.get("state").and_then(Value::as_str) != Some("running")
        || active.get("runId").and_then(Value::as_str) != Some(record.run_id.as_str())
        || active
            .get("leaseExpiresAtMs")
            .and_then(Value::as_i64)
            .is_none_or(|expires_at_ms| expires_at_ms <= now_ms)
    {
        return Err("agent_mcp_execution_context_invalid".to_string());
    }
    let deployment = storage
        .get_json(DEPLOYMENTS_NS, &record.deployment_id)?
        .and_then(|value| value.get("deployment").cloned())
        .and_then(|value| serde_json::from_value::<AgentDeployment>(value).ok())
        .ok_or_else(|| "agent_mcp_execution_context_invalid".to_string())?;
    if deployment_binding_digest(&deployment) != record.binding_digest
        || deployment.mode != record.mode
        || canonical_allowlist(&deployment.studio_tool_allowlist)
            != canonical_allowlist(&record.studio_tool_allowlist)
    {
        return Err("agent_mcp_execution_context_invalid".to_string());
    }
    Ok(VerifiedAgentMcpExecutionContext {
        deployment_id: record.deployment_id,
        run_id: record.run_id,
        binding_digest: record.binding_digest,
        mode: record.mode,
        studio_tool_allowlist: canonical_allowlist(&record.studio_tool_allowlist),
        capability_digest: capability_digest.to_string(),
    })
}

/// Applies a verified per-run capability to one MCP call. This is a local
/// process boundary, not a hostile-local-machine sandbox: only a runner-issued
/// token that still maps to the active run can reach this path.
pub fn bind_mcp_execution_context(
    runtime: &ServiceRuntime,
    execution_context: &VerifiedAgentMcpExecutionContext,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let current =
        verified_mcp_execution_context_for_digest(runtime, execution_context.capability_digest())?;
    if current != *execution_context {
        return Err("agent_mcp_execution_context_invalid".to_string());
    }
    execution_context.current_lease(
        runtime.storage.as_ref(),
        runtime.clock.as_ref(),
        runtime.leases.as_ref(),
    )?;
    if !execution_context
        .studio_tool_allowlist
        .iter()
        .any(|tool| tool == name)
    {
        return Err("agent_mcp_tool_not_allowed".to_string());
    }
    if matches!(
        name,
        "studio.deployment.create"
            | "studio.deployment.start"
            | "studio.deployment.pause"
            | "studio.deployment.stop"
            | "studio.agent_run.recover"
    ) {
        return Err("agent_mcp_operator_tool_forbidden".to_string());
    }
    let mut arguments = arguments;
    let map = arguments
        .as_object_mut()
        .ok_or_else(|| "agent_mcp_execution_context_invalid".to_string())?;
    match name {
        "studio.deployment.inspect" | "studio.agent_run.events" | "studio.agent_run.health" => {
            bind_scoped_identifier(
                map,
                "deployment_id",
                "deploymentId",
                execution_context.deployment_id(),
            )?;
        }
        "studio.agent_run.inspect" => {
            bind_scoped_identifier(map, "run_id", "runId", execution_context.run_id())?;
        }
        "studio.execution.external_receipt.append" => {
            bind_scoped_identifier(
                map,
                "deployment_id",
                "deploymentId",
                execution_context.deployment_id(),
            )?;
            bind_scoped_identifier(map, "run_id", "runId", execution_context.run_id())?;
            bind_scoped_identifier(map, "environment", "environment", execution_context.mode())?;
        }
        "studio.execution.external_receipt.inspect"
        | "studio.execution.external_receipt.reconcile" => {
            bind_scoped_identifier(
                map,
                "deployment_id",
                "deploymentId",
                execution_context.deployment_id(),
            )?;
        }
        // Non-operator tools are still bounded by the durable allowlist and
        // authenticated identity/mode binding below. Only the lifecycle and
        // evidence surfaces above carry deployment/run identifiers that must
        // be forcibly narrowed here.
        _ => {}
    }
    bind_scoped_account_mode(map, execution_context.mode())?;
    Ok(arguments)
}

pub(crate) fn revalidate_mcp_execution_context(
    runtime: &ServiceRuntime,
    context: &VerifiedAgentMcpExecutionContext,
) -> Result<(), String> {
    context
        .current_lease(
            runtime.storage.as_ref(),
            runtime.clock.as_ref(),
            runtime.leases.as_ref(),
        )
        .map(|_| ())
}

fn bind_scoped_identifier(
    arguments: &mut serde_json::Map<String, Value>,
    snake_case: &str,
    camel_case: &str,
    expected: &str,
) -> Result<(), String> {
    for key in [snake_case, camel_case] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str) {
            if value != expected {
                return Err("agent_mcp_execution_context_mismatch".to_string());
            }
        } else if arguments.contains_key(key) {
            return Err("agent_mcp_execution_context_mismatch".to_string());
        }
    }
    if snake_case != camel_case {
        arguments.remove(camel_case);
    }
    arguments.insert(snake_case.to_string(), json!(expected));
    Ok(())
}

fn bind_scoped_account_mode(
    arguments: &mut serde_json::Map<String, Value>,
    expected: &str,
) -> Result<(), String> {
    for key in ["mode", "accountMode", "account_mode"] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str) {
            if value != expected {
                return Err("agent_mcp_execution_context_mismatch".to_string());
            }
        } else if arguments.contains_key(key) {
            return Err("agent_mcp_execution_context_mismatch".to_string());
        }
        arguments.insert(key.to_string(), json!(expected));
    }
    let mut authority_bound = false;
    for key in ["authorityContext", "authority_context"] {
        let Some(authority) = arguments.get_mut(key).and_then(Value::as_object_mut) else {
            continue;
        };
        if authority
            .get("actor")
            .and_then(Value::as_str)
            .is_none_or(|actor| actor.trim().is_empty())
        {
            return Err("agent_mcp_identity_required".to_string());
        }
        authority.insert("accountMode".to_string(), json!(expected));
        authority.remove("account_mode");
        authority_bound = true;
    }
    authority_bound
        .then_some(())
        .ok_or_else(|| "agent_mcp_identity_required".to_string())
}

fn deployment_schedule(deployment: &AgentDeployment) -> Result<DeploymentSchedule, String> {
    if let Some(expression) = deployment.cron_utc.as_deref() {
        return CronUtcSchedule::parse(expression).map(DeploymentSchedule::CronUtc);
    }
    if deployment.interval_seconds < 60 || interval_millis(deployment.interval_seconds).is_none() {
        return Err("agent_deployment_invalid".to_string());
    }
    Ok(DeploymentSchedule::Interval {
        seconds: deployment.interval_seconds,
    })
}

impl DeploymentSchedule {
    fn kind(&self) -> &'static str {
        match self {
            Self::Interval { .. } => "interval",
            Self::CronUtc(_) => "cron_utc",
        }
    }

    fn digest(&self) -> String {
        let value = match self {
            Self::Interval { seconds } => json!({
                "kind": self.kind(),
                "intervalSeconds": seconds,
            }),
            Self::CronUtc(schedule) => json!({
                "kind": self.kind(),
                "expression": schedule.expression,
            }),
        };
        short_hash(&value.to_string())
    }
}

fn initial_schedule_state(schedule: &DeploymentSchedule) -> AgentScheduleState {
    AgentScheduleState {
        schema_version: SCHEMA_VERSION.to_string(),
        schedule_kind: schedule.kind().to_string(),
        schedule_digest: schedule.digest(),
        next_due_at_ms: 0,
        last_due_at_ms: None,
        missed_tick_count: 0,
        missed_tick_count_capped: false,
        updated_at_ms: 0,
    }
}

fn schedule_state_value(state: &AgentScheduleState) -> Value {
    json!({
        "schemaVersion": state.schema_version,
        "scheduleKind": state.schedule_kind,
        "scheduleDigest": state.schedule_digest,
        "nextDueAtMs": state.next_due_at_ms,
        "lastDueAtMs": state.last_due_at_ms,
        "missedTickCount": state.missed_tick_count,
        "missedTickCountCapped": state.missed_tick_count_capped,
        "updatedAtMs": state.updated_at_ms,
    })
}

fn schedule_state_matches(state: &AgentScheduleState, schedule: &DeploymentSchedule) -> bool {
    let matches_current = state.schedule_kind == schedule.kind()
        && state.schedule_digest == schedule.digest()
        && state.schema_version == SCHEMA_VERSION;
    // Existing interval records predate the explicit schedule metadata. Their
    // durable next-due timestamp remains valid; cron records always initialize
    // against their own UTC expression instead of inheriting that timestamp.
    matches_current
        || (state.schedule_kind.is_empty()
            && matches!(schedule, DeploymentSchedule::Interval { .. }))
}

fn normalized_schedule_state(
    schedule: &DeploymentSchedule,
    state: AgentScheduleState,
) -> (AgentScheduleState, bool) {
    if state.schedule_kind.is_empty() && matches!(schedule, DeploymentSchedule::Interval { .. }) {
        let mut upgraded = state;
        upgraded.schema_version = SCHEMA_VERSION.to_string();
        upgraded.schedule_kind = schedule.kind().to_string();
        upgraded.schedule_digest = schedule.digest();
        return (upgraded, true);
    }
    if schedule_state_matches(&state, schedule) {
        return (state, false);
    }
    (initial_schedule_state(schedule), true)
}

fn schedule_decision(
    schedule: &DeploymentSchedule,
    stored: Option<Value>,
    now_ms: i64,
) -> Result<ScheduleDecision, String> {
    let stored = match stored {
        Some(value) => {
            serde_json::from_value(value).map_err(|_| "agent_schedule_state_invalid".to_string())?
        }
        None => initial_schedule_state(schedule),
    };
    let (state, was_normalized) = normalized_schedule_state(schedule, stored);
    match schedule {
        DeploymentSchedule::Interval { seconds } => {
            let interval_ms = interval_millis(*seconds)
                .ok_or_else(|| "agent_schedule_state_invalid".to_string())?;
            if state.next_due_at_ms > now_ms {
                return Ok(ScheduleDecision::NotDue {
                    state,
                    persist: was_normalized,
                });
            }
            let due = interval_due(state, interval_ms, now_ms)?;
            Ok(ScheduleDecision::Due(due))
        }
        DeploymentSchedule::CronUtc(cron) => {
            cron_schedule_decision(cron, state, now_ms, was_normalized)
        }
    }
}

fn interval_millis(seconds: u64) -> Option<i64> {
    i64::try_from(seconds).ok()?.checked_mul(1_000)
}

fn interval_due(
    mut state: AgentScheduleState,
    interval_ms: i64,
    now_ms: i64,
) -> Result<ScheduleDue, String> {
    if state.last_due_at_ms.is_none() && state.next_due_at_ms == 0 {
        state.last_due_at_ms = Some(now_ms);
        state.next_due_at_ms = add_interval(now_ms, interval_ms, 1)?;
        state.missed_tick_count = 0;
        state.missed_tick_count_capped = false;
        state.updated_at_ms = now_ms;
        return Ok(ScheduleDue {
            state,
            scheduled_for_ms: now_ms,
        });
    }
    let scheduled_for_ms = state.next_due_at_ms;
    let elapsed_ms = now_ms.saturating_sub(scheduled_for_ms);
    let missed_tick_count = (elapsed_ms / interval_ms) as u64;
    state.last_due_at_ms = Some(scheduled_for_ms);
    state.next_due_at_ms = add_interval(
        scheduled_for_ms,
        interval_ms,
        missed_tick_count.saturating_add(1),
    )?;
    state.missed_tick_count = missed_tick_count;
    state.missed_tick_count_capped = false;
    state.updated_at_ms = now_ms;
    Ok(ScheduleDue {
        state,
        scheduled_for_ms,
    })
}

fn add_interval(start_ms: i64, interval_ms: i64, count: u64) -> Result<i64, String> {
    let result = i128::from(start_ms) + i128::from(interval_ms) * i128::from(count);
    i64::try_from(result).map_err(|_| "agent_schedule_state_invalid".to_string())
}

fn cron_schedule_decision(
    cron: &CronUtcSchedule,
    mut state: AgentScheduleState,
    now_ms: i64,
    was_normalized: bool,
) -> Result<ScheduleDecision, String> {
    if state.last_due_at_ms.is_none() && state.next_due_at_ms == 0 {
        let first_due_at_ms = cron_due_at_or_after(cron, now_ms)?;
        if first_due_at_ms > now_ms {
            state.next_due_at_ms = first_due_at_ms;
            state.updated_at_ms = now_ms;
            return Ok(ScheduleDecision::NotDue {
                state,
                persist: true,
            });
        }
    } else if state.next_due_at_ms > now_ms {
        return Ok(ScheduleDecision::NotDue {
            state,
            persist: was_normalized,
        });
    }
    cron_due(cron, state, now_ms).map(ScheduleDecision::Due)
}

fn cron_due(
    cron: &CronUtcSchedule,
    mut state: AgentScheduleState,
    now_ms: i64,
) -> Result<ScheduleDue, String> {
    let scheduled_for_ms = if state.last_due_at_ms.is_none() && state.next_due_at_ms == 0 {
        cron_due_at_or_after(cron, now_ms)?
    } else {
        state.next_due_at_ms
    };
    if scheduled_for_ms > now_ms {
        return Err("agent_schedule_state_invalid".to_string());
    }
    let next_due_at_ms = next_cron_due_after(cron, now_ms)?;
    let (missed_tick_count, missed_tick_count_capped) =
        cron_missed_tick_count(cron, scheduled_for_ms, now_ms)?;
    state.last_due_at_ms = Some(scheduled_for_ms);
    state.next_due_at_ms = next_due_at_ms;
    state.missed_tick_count = missed_tick_count;
    state.missed_tick_count_capped = missed_tick_count_capped;
    state.updated_at_ms = now_ms;
    Ok(ScheduleDue {
        state,
        scheduled_for_ms,
    })
}

fn cron_due_at_or_after(cron: &CronUtcSchedule, now_ms: i64) -> Result<i64, String> {
    let candidate = ceil_to_minute(now_ms);
    if cron.matches(utc_timestamp(candidate)?) {
        return Ok(candidate);
    }
    next_cron_due_after(cron, candidate)
}

fn next_cron_due_after(cron: &CronUtcSchedule, after_ms: i64) -> Result<i64, String> {
    let mut candidate = floor_to_minute(after_ms)
        .checked_add(MILLIS_PER_MINUTE)
        .ok_or_else(|| "agent_schedule_state_invalid".to_string())?;
    for _ in 0..MAX_CRON_LOOKAHEAD_MINUTES {
        if cron.matches(utc_timestamp(candidate)?) {
            return Ok(candidate);
        }
        candidate = candidate
            .checked_add(MILLIS_PER_MINUTE)
            .ok_or_else(|| "agent_schedule_state_invalid".to_string())?;
    }
    Err("agent_cron_next_due_unavailable".to_string())
}

fn cron_missed_tick_count(
    cron: &CronUtcSchedule,
    scheduled_for_ms: i64,
    now_ms: i64,
) -> Result<(u64, bool), String> {
    let mut candidate = scheduled_for_ms
        .checked_add(MILLIS_PER_MINUTE)
        .ok_or_else(|| "agent_schedule_state_invalid".to_string())?;
    let mut missed_tick_count = 0_u64;
    for _ in 0..MAX_CRON_MISSED_SCAN_MINUTES {
        if candidate > now_ms {
            return Ok((missed_tick_count, false));
        }
        if cron.matches(utc_timestamp(candidate)?) {
            missed_tick_count = missed_tick_count.saturating_add(1);
        }
        if missed_tick_count >= MAX_COALESCED_TICKS {
            return Ok((missed_tick_count, true));
        }
        candidate = candidate
            .checked_add(MILLIS_PER_MINUTE)
            .ok_or_else(|| "agent_schedule_state_invalid".to_string())?;
    }
    // This is intentionally a lower bound after the fixed scan budget. The
    // durable cap flag tells callers not to treat it as an exact loss count.
    Ok((missed_tick_count, true))
}

fn floor_to_minute(value: i64) -> i64 {
    value
        .div_euclid(MILLIS_PER_MINUTE)
        .saturating_mul(MILLIS_PER_MINUTE)
}

fn ceil_to_minute(value: i64) -> i64 {
    let floor = floor_to_minute(value);
    if floor == value {
        floor
    } else {
        floor.saturating_add(MILLIS_PER_MINUTE)
    }
}

fn utc_timestamp(timestamp_ms: i64) -> Result<DateTime<Utc>, String> {
    Utc.timestamp_millis_opt(timestamp_ms)
        .single()
        .ok_or_else(|| "agent_schedule_state_invalid".to_string())
}

impl CronUtcSchedule {
    fn parse(expression: &str) -> Result<Self, String> {
        if expression.is_empty() || expression.len() > MAX_CRON_EXPRESSION_BYTES {
            return Err("agent_deployment_invalid".to_string());
        }
        let fields = expression.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err("agent_deployment_invalid".to_string());
        }
        Ok(Self {
            expression: fields.join(" "),
            minute: CronField::parse(fields[0], 0, 59, false)?,
            hour: CronField::parse(fields[1], 0, 23, false)?,
            day_of_month: CronField::parse(fields[2], 1, 31, false)?,
            month: CronField::parse(fields[3], 1, 12, false)?,
            day_of_week: CronField::parse(fields[4], 0, 7, true)?,
        })
    }

    fn matches(&self, timestamp: DateTime<Utc>) -> bool {
        if !self.minute.matches(timestamp.minute() as u8)
            || !self.hour.matches(timestamp.hour() as u8)
            || !self.month.matches(timestamp.month() as u8)
        {
            return false;
        }
        let day_of_month_matches = self.day_of_month.matches(timestamp.day() as u8);
        let day_of_week_matches = self
            .day_of_week
            .matches(timestamp.weekday().num_days_from_sunday() as u8);
        match (self.day_of_month.is_all(), self.day_of_week.is_all()) {
            (true, true) => true,
            (true, false) => day_of_week_matches,
            (false, true) => day_of_month_matches,
            (false, false) => day_of_month_matches || day_of_week_matches,
        }
    }
}

impl CronField {
    fn parse(value: &str, minimum: u8, maximum: u8, sunday_alias: bool) -> Result<Self, String> {
        let terms = value.split(',').collect::<Vec<_>>();
        if value.is_empty()
            || terms.len() > MAX_CRON_FIELD_TERMS
            || terms.iter().any(|term| term.is_empty())
        {
            return Err("agent_deployment_invalid".to_string());
        }
        let mut field = Self {
            minimum,
            allowed: vec![false; usize::from(maximum - minimum + 1 - u8::from(sunday_alias))],
        };
        for term in terms {
            field.add_term(term, minimum, maximum, sunday_alias)?;
        }
        if field.allowed.iter().all(|allowed| !allowed) {
            return Err("agent_deployment_invalid".to_string());
        }
        Ok(field)
    }

    fn add_term(
        &mut self,
        term: &str,
        minimum: u8,
        maximum: u8,
        sunday_alias: bool,
    ) -> Result<(), String> {
        let mut parts = term.split('/');
        let base = parts.next().unwrap_or_default();
        let step = match parts.next() {
            Some(raw) => {
                if parts.next().is_some() {
                    return Err("agent_deployment_invalid".to_string());
                }
                if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err("agent_deployment_invalid".to_string());
                }
                let value = raw
                    .parse::<usize>()
                    .map_err(|_| "agent_deployment_invalid".to_string())?;
                if value == 0 {
                    return Err("agent_deployment_invalid".to_string());
                }
                Some(value)
            }
            None => None,
        };
        let (start, end) = if base == "*" {
            (minimum, maximum)
        } else if let Some((start, end)) = base.split_once('-') {
            if end.contains('-') {
                return Err("agent_deployment_invalid".to_string());
            }
            let start = parse_cron_bound(start, minimum, maximum)?;
            let end = parse_cron_bound(end, minimum, maximum)?;
            if start > end {
                return Err("agent_deployment_invalid".to_string());
            }
            (start, end)
        } else {
            let start = parse_cron_bound(base, minimum, maximum)?;
            (start, step.map_or(start, |_| maximum))
        };
        let step = step.unwrap_or(1);
        for candidate in (start..=end).step_by(step) {
            self.mark(candidate, sunday_alias);
        }
        Ok(())
    }

    fn mark(&mut self, value: u8, sunday_alias: bool) {
        let value = if sunday_alias && value == 7 { 0 } else { value };
        let index = usize::from(value.saturating_sub(self.minimum));
        if let Some(slot) = self.allowed.get_mut(index) {
            *slot = true;
        }
    }

    fn matches(&self, value: u8) -> bool {
        self.allowed
            .get(usize::from(value.saturating_sub(self.minimum)))
            .copied()
            .unwrap_or(false)
    }

    fn is_all(&self) -> bool {
        self.allowed.iter().all(|allowed| *allowed)
    }
}

fn parse_cron_bound(value: &str, minimum: u8, maximum: u8) -> Result<u8, String> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("agent_deployment_invalid".to_string());
    }
    let value = value
        .parse::<u8>()
        .map_err(|_| "agent_deployment_invalid".to_string())?;
    if value < minimum || value > maximum {
        return Err("agent_deployment_invalid".to_string());
    }
    Ok(value)
}

fn persist_schedule_state(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
    state: &AgentScheduleState,
) -> Result<(), String> {
    persist_schedule_state_with_context(
        runtime,
        deployment,
        state,
        &context(&format!("schedule:{}", deployment.deployment_id)),
    )
}

fn persist_schedule_state_with_context(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
    state: &AgentScheduleState,
    side_effect_context: &SideEffectContext,
) -> Result<(), String> {
    let stored_deployment = runtime
        .storage
        .get_json(DEPLOYMENTS_NS, &deployment.deployment_id)?
        .and_then(|record| record.get("deployment").cloned())
        .ok_or_else(|| "agent_deployment_not_found".to_string())?;
    let stored_deployment = serde_json::from_value::<AgentDeployment>(stored_deployment)
        .map_err(|_| "agent_deployment_invalid".to_string())?;
    let current_schedule = deployment_schedule(&stored_deployment)?;
    if state.schedule_kind != current_schedule.kind()
        || state.schedule_digest != current_schedule.digest()
    {
        // A prior run may complete after the deployment has been reconfigured.
        // It must never reinstate the old cadence over the current record.
        return Ok(());
    }
    if let Some(existing) = runtime
        .storage
        .get_json(SCHEDULES_NS, &deployment.deployment_id)?
    {
        let existing = serde_json::from_value::<AgentScheduleState>(existing)
            .map_err(|_| "agent_schedule_state_invalid".to_string())?;
        if existing.schedule_kind == state.schedule_kind
            && existing.schedule_digest == state.schedule_digest
            && existing.next_due_at_ms >= state.next_due_at_ms
        {
            // An overlapping active run already consumed a later due tick. Do
            // not let this run's earlier completion roll that state backward.
            return Ok(());
        }
    }
    runtime.storage.put_json(
        SCHEDULES_NS,
        &deployment.deployment_id,
        schedule_state_value(state),
        side_effect_context,
    )
}

fn schedule_after_operator_recovery(
    deployment: &AgentDeployment,
    now_ms: i64,
) -> Result<AgentScheduleState, String> {
    let schedule = deployment_schedule(deployment)?;
    let mut state = initial_schedule_state(&schedule);
    state.next_due_at_ms = match schedule {
        DeploymentSchedule::Interval { .. } => now_ms,
        DeploymentSchedule::CronUtc(cron) => next_cron_due_after(&cron, now_ms)?,
    };
    state.updated_at_ms = now_ms;
    Ok(state)
}

pub fn set_desired_state(
    runtime: &ServiceRuntime,
    deployment_id: &str,
    desired_state: &str,
) -> Result<(), String> {
    set_desired_state_with_context(
        runtime,
        deployment_id,
        desired_state,
        &context(&format!("deployment:{deployment_id}")),
    )
}

pub fn set_desired_state_with_context(
    runtime: &ServiceRuntime,
    deployment_id: &str,
    desired_state: &str,
    side_effect_context: &SideEffectContext,
) -> Result<(), String> {
    let mut deployment = deployments(runtime)?
        .into_iter()
        .find(|item| item.deployment_id == deployment_id)
        .ok_or_else(|| "agent_deployment_not_found".to_string())?;
    deployment.desired_state = desired_state.to_string();
    put_deployment_with_context(runtime, &deployment, side_effect_context)
}

pub fn deployments(runtime: &ServiceRuntime) -> Result<Vec<AgentDeployment>, String> {
    runtime.storage.list_json(DEPLOYMENTS_NS).map(|items| {
        items
            .into_iter()
            .filter_map(|(_, value)| serde_json::from_value(value.get("deployment")?.clone()).ok())
            .collect()
    })
}

/// One foreground supervisor pass. A duplicate or overlapping tick is coalesced, not run twice.
pub fn supervise_once(
    runtime: &ServiceRuntime,
    runner_id: &str,
    now_ms: i64,
    adapter: &dyn AgentRuntimePort,
) -> Result<Vec<Value>, String> {
    supervise_once_with_timing_and_entitlement(
        runtime,
        runner_id,
        now_ms,
        adapter,
        &LocalEntitlement,
        AgentRunnerTiming::default(),
    )
}

pub fn supervise_once_with_entitlement(
    runtime: &ServiceRuntime,
    runner_id: &str,
    now_ms: i64,
    adapter: &dyn AgentRuntimePort,
    entitlement: &dyn AgentEntitlementPort,
) -> Result<Vec<Value>, String> {
    supervise_once_with_timing_and_entitlement(
        runtime,
        runner_id,
        now_ms,
        adapter,
        entitlement,
        AgentRunnerTiming::default(),
    )
}

/// Testable variant of [`supervise_once`] with a bounded lease/heartbeat cadence.
/// Production callers must use the default 90-second lease and 30-second heartbeat.
pub fn supervise_once_with_timing_and_entitlement(
    runtime: &ServiceRuntime,
    runner_id: &str,
    now_ms: i64,
    adapter: &dyn AgentRuntimePort,
    entitlement: &dyn AgentEntitlementPort,
    timing: AgentRunnerTiming,
) -> Result<Vec<Value>, String> {
    if timing.lease_ms <= 0
        || timing.heartbeat_ms == 0
        || timing.heartbeat_ms as i64 >= timing.lease_ms
    {
        return Err("agent_runner_timing_invalid".to_string());
    }
    let mut receipts = Vec::new();
    for deployment in deployments(runtime)?
        .into_iter()
        .filter(|d| d.desired_state == "active" && d.executor == AgentExecutor::Supervised)
    {
        match supervise_deployment(
            runtime,
            runner_id,
            now_ms,
            &deployment,
            adapter,
            entitlement,
            timing,
        ) {
            Ok(receipt) => receipts.push(receipt),
            // A deployment fault is a durable per-deployment outcome. It must
            // not take down other active deployments or the foreground runner.
            Err(error) => receipts.push(failure_receipt(&deployment, &error)),
        }
    }
    Ok(receipts)
}

/// Resolves a quarantined run after the local operator has reconciled it using
/// the deterministic execution evidence. This never submits or retries an order.
pub fn recover_pending_run(
    runtime: &ServiceRuntime,
    deployment_id: &str,
    now_ms: i64,
) -> Result<Value, String> {
    recover_pending_run_with_context(
        runtime,
        deployment_id,
        now_ms,
        &context(&format!("agent-recovery:{deployment_id}")),
    )
}

/// Resolves a quarantined run with a durable request receipt. If a process
/// fails after changing the run state but before returning, replaying the same
/// idempotency context finalizes (or returns) this receipt instead of treating
/// the already-reconciled run as an ambiguous new request.
pub fn recover_pending_run_with_context(
    runtime: &ServiceRuntime,
    deployment_id: &str,
    now_ms: i64,
    side_effect_context: &SideEffectContext,
) -> Result<Value, String> {
    let deployment = deployments(runtime)?
        .into_iter()
        .find(|item| item.deployment_id == deployment_id)
        .ok_or_else(|| "agent_deployment_not_found".to_string())?;
    let request_digest = recovery_request_digest(&deployment, side_effect_context);
    let receipt_key = format!("recovery_{request_digest}");
    let Some(lease) = runtime.leases.acquire(
        &format!("agent-deployment:{}", deployment.deployment_id),
        "local-agent-recovery",
        now_ms,
        DEFAULT_LEASE_MS,
    )?
    else {
        return Err("agent_recovery_lease_held".to_string());
    };
    let mut transition_started = false;
    let result = (|| {
        if deployment.executor == AgentExecutor::ExternalClient {
            external_session::quarantine_abandoned(runtime, deployment_id, side_effect_context)?;
        }
        let receipt = match runtime
            .storage
            .get_json(RECOVERY_RECEIPTS_NS, &receipt_key)?
        {
            Some(value) => serde_json::from_value::<AgentRecoveryReceipt>(value)
                .map_err(|_| "agent_recovery_receipt_invalid".to_string())?,
            None => {
                let active = runtime
                    .storage
                    .get_json(ACTIVE_RUNS_NS, &deployment.deployment_id)?
                    .ok_or_else(|| "agent_run_not_pending_reconcile".to_string())?;
                if active["state"] != "pending_reconcile" {
                    return Err("agent_run_not_pending_reconcile".to_string());
                }
                let run_id = active["runId"]
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| "agent_run_not_pending_reconcile".to_string())?;
                let candidate = AgentRecoveryReceipt {
                    schema_version: SCHEMA_VERSION.to_string(),
                    deployment_id: deployment.deployment_id.clone(),
                    run_id: run_id.to_string(),
                    request_digest: request_digest.clone(),
                    requested_at_ms: now_ms,
                    state: "in_progress".to_string(),
                    receipt: receipt(&deployment, "reconciled", Some(run_id)),
                };
                match runtime.storage.put_json_if_absent(
                    RECOVERY_RECEIPTS_NS,
                    &receipt_key,
                    serde_json::to_value(&candidate)
                        .map_err(|_| "agent_recovery_receipt_invalid".to_string())?,
                    side_effect_context,
                ) {
                    Ok(ImmutablePutOutcome::Created | ImmutablePutOutcome::AlreadyPresent) => {
                        runtime
                            .storage
                            .get_json(RECOVERY_RECEIPTS_NS, &receipt_key)?
                            .and_then(|value| serde_json::from_value(value).ok())
                            .ok_or_else(|| "agent_recovery_receipt_invalid".to_string())?
                    }
                    Err(error) if error == "immutable_storage_conflict" => {
                        return Err("agent_recovery_idempotency_conflict".to_string());
                    }
                    Err(_) => return Err("agent_recovery_receipt_persist_failed".to_string()),
                }
            }
        };
        if receipt.schema_version != SCHEMA_VERSION
            || receipt.deployment_id != deployment.deployment_id
            || receipt.request_digest != request_digest
            || !matches!(receipt.state.as_str(), "in_progress" | "completed")
        {
            return Err("agent_recovery_receipt_invalid".to_string());
        }
        if receipt.state == "completed" {
            return Ok(receipt.receipt);
        }
        let recovery_at_ms = receipt.requested_at_ms;
        let run_id = receipt.run_id.as_str();
        let mut record = runtime
            .storage
            .get_json(RUNS_NS, run_id)?
            .ok_or_else(|| "agent_run_not_pending_reconcile".to_string())?;
        let state = record
            .pointer("/run/state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if state == "pending_reconcile" {
            record["run"]["state"] = json!("reconciled");
            runtime
                .storage
                .put_json(RUNS_NS, run_id, record.clone(), side_effect_context)?;
            transition_started = true;
        } else if state != "reconciled" {
            return Err("agent_run_not_pending_reconcile".to_string());
        } else {
            transition_started = true;
        }
        let active = runtime
            .storage
            .get_json(ACTIVE_RUNS_NS, &deployment.deployment_id)?
            .ok_or_else(|| "agent_run_not_pending_reconcile".to_string())?;
        let active_matches = (active["state"] == "pending_reconcile"
            && active["runId"].as_str() == Some(run_id))
            || (active["state"] == "reconciled"
                && active["resolvedRunId"].as_str() == Some(run_id));
        if !active_matches {
            return Err("agent_run_not_pending_reconcile".to_string());
        }
        // Do not retain an ambiguous session reference as the predecessor of a
        // newly authorized run. The prior run remains in its own evidence record.
        runtime.storage.put_json(
            ACTIVE_RUNS_NS,
            &deployment.deployment_id,
            json!({"state": "reconciled", "resolvedRunId": run_id, "resolvedAtMs": recovery_at_ms}),
            side_effect_context,
        )?;
        if deployment.executor == AgentExecutor::Supervised {
            let next_schedule = schedule_after_operator_recovery(&deployment, recovery_at_ms)?;
            persist_schedule_state_with_context(
                runtime,
                &deployment,
                &next_schedule,
                side_effect_context,
            )?;
        }
        append_evidence_with_context(
            runtime,
            &deployment,
            "agent_run.operator_reconciled",
            &record,
            recovery_at_ms,
            side_effect_context,
        )?;
        let completed = AgentRecoveryReceipt {
            state: "completed".to_string(),
            ..receipt
        };
        runtime.storage.put_json(
            RECOVERY_RECEIPTS_NS,
            &receipt_key,
            serde_json::to_value(&completed)
                .map_err(|_| "agent_recovery_receipt_invalid".to_string())?,
            side_effect_context,
        )?;
        Ok(completed.receipt)
    })();
    let _ = runtime.leases.release(&lease);
    if transition_started && result.is_err() {
        Err("agent_recovery_retryable".to_string())
    } else {
        result
    }
}

fn supervise_deployment(
    runtime: &ServiceRuntime,
    runner_id: &str,
    now_ms: i64,
    deployment: &AgentDeployment,
    adapter: &dyn AgentRuntimePort,
    entitlement: &dyn AgentEntitlementPort,
    timing: AgentRunnerTiming,
) -> Result<Value, String> {
    let Some(lease) = runtime.leases.acquire(
        &format!("agent-deployment:{}", deployment.deployment_id),
        runner_id,
        now_ms,
        timing.lease_ms,
    )?
    else {
        return Ok(receipt(deployment, "lease_held", None));
    };
    let mut lease = lease;
    let schedule = deployment_schedule(deployment)?;
    let due = match schedule_decision(
        &schedule,
        runtime
            .storage
            .get_json(SCHEDULES_NS, &deployment.deployment_id)?,
        now_ms,
    )? {
        ScheduleDecision::NotDue { state, persist } => {
            if persist {
                persist_schedule_state(runtime, deployment, &state)?;
            }
            runtime.leases.release(&lease)?;
            return Ok(receipt(deployment, "not_due", None));
        }
        ScheduleDecision::Due(due) => due,
    };
    let scheduled_for_ms = due.scheduled_for_ms;
    let schedule_state = due.state;
    let active_key = &deployment.deployment_id;
    let mut recovered = None;
    if let Some(active) = runtime.storage.get_json(ACTIVE_RUNS_NS, active_key)? {
        if active["state"] == "pending_reconcile" {
            runtime.leases.release(&lease)?;
            return Ok(receipt(
                deployment,
                "pending_reconcile",
                active["runId"].as_str(),
            ));
        }
        if active["state"] == "running"
            && active["leaseExpiresAtMs"].as_i64().unwrap_or(i64::MIN) > now_ms
        {
            let active_run_id = active["runId"].as_str().map(str::to_string);
            persist_schedule_state(runtime, deployment, &schedule_state)?;
            append_evidence(
                runtime,
                deployment,
                "agent_run.coalesced",
                &json!({
                    "activeRun": active,
                    "schedule": schedule_state_value(&schedule_state),
                }),
                now_ms,
            )?;
            runtime.leases.release(&lease)?;
            return Ok(receipt(deployment, "coalesced", active_run_id.as_deref()));
        }
        if active["state"] == "running" {
            append_evidence(
                runtime,
                deployment,
                "agent_run.recovered",
                &json!({
                    "activeRun": active,
                    "schedule": schedule_state_value(&schedule_state),
                }),
                now_ms,
            )?;
            let old_id = active["runId"]
                .as_str()
                .ok_or_else(|| "agent_run_pending_reconcile".to_string())?;
            let mut old = runtime
                .storage
                .get_json(RUNS_NS, old_id)?
                .ok_or_else(|| "agent_run_pending_reconcile".to_string())?;
            let session = old
                .pointer("/run/codexSessionRef")
                .and_then(Value::as_str)
                .map(str::to_string);
            if session.is_none() {
                old["run"]["state"] = json!("pending_reconcile");
                runtime.storage.put_json(
                    RUNS_NS,
                    old_id,
                    old.clone(),
                    &context(&format!("run:{old_id}")),
                )?;
                runtime.storage.put_json(
                    ACTIVE_RUNS_NS,
                    active_key,
                    json!({"runId": old_id, "state": "pending_reconcile"}),
                    &context(&format!("active:{active_key}")),
                )?;
                append_evidence(
                    runtime,
                    deployment,
                    "agent_run.pending_reconcile",
                    &old,
                    now_ms,
                )?;
                runtime.leases.release(&lease)?;
                return Err("agent_run_pending_reconcile".to_string());
            }
            recovered = Some((old_id.to_string(), session));
        }
    }
    let trigger_id = format!("tick:{}:{scheduled_for_ms}", deployment.deployment_id);
    let binding_digest = deployment_binding_digest(deployment);
    let run_id = recovered
        .as_ref()
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| format!("run_{}", short_hash(&trigger_id)));
    let previous_session = if let Some((_, session)) = recovered {
        session
    } else {
        runtime
            .storage
            .get_json(ACTIVE_RUNS_NS, active_key)?
            .and_then(|active| active["runId"].as_str().map(str::to_string))
            .and_then(|run_id| runtime.storage.get_json(RUNS_NS, &run_id).ok().flatten())
            .and_then(|record| {
                (record["bindingDigest"].as_str() == Some(binding_digest.as_str()))
                    .then(|| {
                        record
                            .pointer("/run/codexSessionRef")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .flatten()
            })
    };
    let run = AgentRun {
        run_id: run_id.clone(),
        deployment_id: deployment.deployment_id.clone(),
        trigger_id,
        state: "running".to_string(),
        lease_fence: lease.fencing_token,
        deployment_binding_digest: binding_digest.clone(),
        codex_session_ref: previous_session,
    };
    if let Some(existing) = runtime.storage.get_json(RUNS_NS, &run_id)? {
        let state = existing
            .pointer("/run/state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if state == "completed" {
            persist_schedule_state(runtime, deployment, &schedule_state)?;
            runtime.leases.release(&lease)?;
            return Ok(receipt(deployment, "idempotent", Some(&run_id)));
        }
        if existing["bindingDigest"].as_str() != Some(binding_digest.as_str()) {
            let mut pending = existing;
            pending["run"]["state"] = json!("pending_reconcile");
            runtime.storage.put_json(
                RUNS_NS,
                &run_id,
                pending.clone(),
                &context(&format!("run:{run_id}")),
            )?;
            runtime.storage.put_json(
                ACTIVE_RUNS_NS,
                active_key,
                json!({"runId": run_id, "state": "pending_reconcile", "bindingDigest": binding_digest}),
                &context(&format!("active:{active_key}")),
            )?;
            append_evidence(
                runtime,
                deployment,
                "agent_run.binding_mismatch",
                &pending,
                now_ms,
            )?;
            runtime.leases.release(&lease)?;
            return Err("agent_run_pending_reconcile".to_string());
        }
        if existing
            .pointer("/run/codexSessionRef")
            .and_then(Value::as_str)
            .is_none()
        {
            let mut pending = existing;
            pending["run"]["state"] = json!("pending_reconcile");
            runtime.storage.put_json(
                RUNS_NS,
                &run_id,
                pending.clone(),
                &context(&format!("run:{run_id}")),
            )?;
            runtime.storage.put_json(
                ACTIVE_RUNS_NS,
                active_key,
                json!({"runId": run_id, "state": "pending_reconcile"}),
                &context(&format!("active:{active_key}")),
            )?;
            append_evidence(
                runtime,
                deployment,
                "agent_run.pending_reconcile",
                &pending,
                now_ms,
            )?;
            runtime.leases.release(&lease)?;
            return Err("agent_run_pending_reconcile".to_string());
        }
    }
    let trigger = json!({
        "schemaVersion": SCHEMA_VERSION,
        "systemProjectId": deployment.system_project_id,
        "agentDefinitionVersionId": deployment.agent_definition_version_id,
        "executionConfigVersionId": deployment.execution_config_version_id,
        "deploymentBindingDigest": binding_digest.clone(),
        "studioToolAllowlist": deployment.studio_tool_allowlist,
        "deploymentId": deployment.deployment_id,
        "runId": run_id,
        "mode": deployment.mode,
        "schedule": {
            "kind": schedule.kind(),
            "scheduledForMs": scheduled_for_ms,
            "missedTickCount": schedule_state.missed_tick_count,
            "missedTickCountCapped": schedule_state.missed_tick_count_capped,
            "nextDueAtMs": schedule_state.next_due_at_ms,
        },
        "prompt": deployment.prompt,
        "workspace": deployment.workspace,
        "runtimeProfile": deployment.runtime_profile,
        "sideEffectPolicy": "Use Studio MCP tools for Studio mutations, analysis, and durable evidence. Broker execution remains user-configured and broker-off-platform; a separately configured local broker interface may be used by the agent. Never put credentials in this prompt or persist them in Studio. Retain broker client order IDs for deterministic reconciliation.",
        "redactedFacts": {},
    });
    let entitlement = entitlement.authorize(deployment);
    if !entitlement.allowed {
        // A denied start has no agent side effect, but it should not hammer a
        // future Hub entitlement endpoint once per foreground scan.
        persist_schedule_state(runtime, deployment, &schedule_state)?;
        runtime.leases.release(&lease)?;
        return Err("agent_entitlement_denied".to_string());
    }
    let entitlement_ref = entitlement.reference;
    let mcp_capability = AgentMcpExecutionCapability::issue();
    let capability_digest = mcp_capability.digest();
    let capability_record = AgentMcpCapabilityRecord {
        schema_version: SCHEMA_VERSION.to_string(),
        deployment_id: deployment.deployment_id.clone(),
        run_id: run_id.clone(),
        binding_digest: binding_digest.clone(),
        mode: deployment.mode.clone(),
        studio_tool_allowlist: canonical_allowlist(&deployment.studio_tool_allowlist),
        capability_digest: capability_digest.clone(),
    };
    let running_record = json!({
        "schemaVersion": SCHEMA_VERSION,
        "kind": "AgentRun",
        "run": run,
        "bindingDigest": binding_digest,
        "mcpCapabilityDigest": capability_digest,
        "entitlementRef": entitlement_ref,
        "schedule": schedule_state_value(&schedule_state),
    });
    runtime.storage.put_json(
        MCP_CAPABILITIES_NS,
        &capability_digest,
        serde_json::to_value(&capability_record)
            .map_err(|_| "agent_mcp_execution_context_invalid".to_string())?,
        &context(&format!("mcp-capability:{run_id}")),
    )?;
    runtime.storage.put_json(
        RUNS_NS,
        &run_id,
        running_record,
        &context(&format!("run:{run_id}")),
    )?;
    runtime.storage.put_json(
        ACTIVE_RUNS_NS,
        active_key,
        active_run_record(&run_id, "running", &lease, now_ms),
        &context(&format!("active:{active_key}")),
    )?;
    let session_ref = match run_adapter_with_lease_heartbeat(
        runtime,
        deployment,
        &run,
        &trigger,
        &mcp_capability,
        LeaseHeartbeat {
            adapter,
            lease: &lease,
            started_at_ms: now_ms,
            timing,
        },
    ) {
        Ok((session_ref, renewed_lease)) => {
            lease = renewed_lease;
            session_ref
        }
        Err(_) => {
            // A non-successful agent process can have exited after an MCP side
            // effect. Quarantine the run; never schedule a blind retry.
            let pending = json!({
                "schemaVersion": SCHEMA_VERSION,
                "kind": "AgentRun",
                "run": {
                    "runId": run_id,
                    "deploymentId": deployment.deployment_id,
                    "state": "pending_reconcile",
                    "errorCode": "agent_runtime_failed",
                },
                "mcpCapabilityDigest": capability_digest,
                "entitlementRef": entitlement_ref,
                "schedule": schedule_state_value(&schedule_state),
            });
            let _ = runtime.storage.put_json(
                RUNS_NS,
                &run_id,
                pending.clone(),
                &context(&format!("run:{run_id}")),
            );
            let _ = runtime.storage.put_json(
                ACTIVE_RUNS_NS,
                active_key,
                json!({"runId": run_id, "state": "pending_reconcile"}),
                &context(&format!("active:{active_key}")),
            );
            let _ = append_evidence(
                runtime,
                deployment,
                "agent_run.pending_reconcile",
                &pending,
                now_ms,
            );
            let _ = runtime.leases.release(&lease);
            return Err("agent_run_pending_reconcile".to_string());
        }
    };
    let completed = AgentRun {
        state: "completed".to_string(),
        codex_session_ref: session_ref,
        ..run
    };
    let record = json!({
        "schemaVersion": SCHEMA_VERSION,
        "kind": "AgentRun",
        "run": completed,
        "bindingDigest": deployment_binding_digest(deployment),
        "mcpCapabilityDigest": capability_digest,
        "entitlementRef": entitlement_ref,
        "schedule": schedule_state_value(&schedule_state),
    });
    runtime.storage.put_json(
        RUNS_NS,
        &run_id,
        record.clone(),
        &context(&format!("run:{run_id}")),
    )?;
    runtime.storage.put_json(
        ACTIVE_RUNS_NS,
        active_key,
        active_run_record(&run_id, "completed", &lease, now_ms),
        &context(&format!("active:{active_key}")),
    )?;
    persist_schedule_state(runtime, deployment, &schedule_state)?;
    append_evidence(runtime, deployment, "agent_run.completed", &record, now_ms)?;
    runtime.leases.release(&lease)?;
    Ok(receipt(deployment, "completed", Some(&run_id)))
}

/// Keeps the deployment lease valid while a synchronous runtime adapter is
/// running. Without this watchdog, a long Codex turn could outlive its lease
/// and a second runner could resume the same opaque session concurrently.
fn run_adapter_with_lease_heartbeat(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
    run: &AgentRun,
    trigger: &Value,
    mcp_execution_context: &AgentMcpExecutionCapability,
    heartbeat: LeaseHeartbeat<'_>,
) -> Result<(Option<String>, LeaseClaim), String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let started = Instant::now();
    let mut renewed = heartbeat.lease.clone();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = sender.send(
                heartbeat
                    .adapter
                    .start_or_resume_with_mcp_execution_context(
                        run,
                        trigger,
                        mcp_execution_context,
                    ),
            );
        });
        loop {
            match receiver.recv_timeout(Duration::from_millis(heartbeat.timing.heartbeat_ms)) {
                Ok(result) => {
                    let heartbeat_at_ms = elapsed_now_ms(heartbeat.started_at_ms, started);
                    renewed = runtime
                        .leases
                        .renew(&renewed, heartbeat_at_ms, heartbeat.timing.lease_ms)
                        .map_err(|_| "agent_run_lease_lost".to_string())?;
                    persist_active_heartbeat(runtime, deployment, run, &renewed, heartbeat_at_ms)?;
                    return result.map(|session_ref| (session_ref, renewed));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let heartbeat_at_ms = elapsed_now_ms(heartbeat.started_at_ms, started);
                    renewed = runtime
                        .leases
                        .renew(&renewed, heartbeat_at_ms, heartbeat.timing.lease_ms)
                        .map_err(|_| "agent_run_lease_lost".to_string())?;
                    persist_active_heartbeat(runtime, deployment, run, &renewed, heartbeat_at_ms)?;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("agent_runtime_channel_closed".to_string());
                }
            }
        }
    })
}

fn elapsed_now_ms(started_at_ms: i64, started: Instant) -> i64 {
    started_at_ms.saturating_add(started.elapsed().as_millis().try_into().unwrap_or(i64::MAX))
}

fn persist_active_heartbeat(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
    run: &AgentRun,
    lease: &LeaseClaim,
    heartbeat_at_ms: i64,
) -> Result<(), String> {
    runtime.storage.put_json(
        ACTIVE_RUNS_NS,
        &deployment.deployment_id,
        active_run_record(&run.run_id, "running", lease, heartbeat_at_ms),
        &context(&format!("active:{}", deployment.deployment_id)),
    )
}

fn active_run_record(run_id: &str, state: &str, lease: &LeaseClaim, heartbeat_at_ms: i64) -> Value {
    json!({
        "runId": run_id,
        "state": state,
        "leaseExpiresAtMs": lease.expires_at_ms,
        "leaseFence": lease.fencing_token,
        "heartbeatAtMs": heartbeat_at_ms,
    })
}

fn append_evidence(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
    event_type: &str,
    payload: &Value,
    now_ms: i64,
) -> Result<(), String> {
    append_evidence_with_context(
        runtime,
        deployment,
        event_type,
        payload,
        now_ms,
        &context(&format!(
            "evidence:{event_type}:{}",
            deployment.deployment_id
        )),
    )
}

fn append_evidence_with_context(
    runtime: &ServiceRuntime,
    deployment: &AgentDeployment,
    event_type: &str,
    payload: &Value,
    now_ms: i64,
    side_effect_context: &SideEffectContext,
) -> Result<(), String> {
    let key = format!(
        "{event_type}:{}:{}",
        deployment.deployment_id,
        short_hash(&payload.to_string())
    );
    let redacted = json!({
        "schemaVersion": SCHEMA_VERSION,
        "deploymentId": deployment.deployment_id,
        "eventType": event_type,
        "authority": {
            "actor": side_effect_context.authority.actor,
            "surface": side_effect_context.authority.surface,
            "accountMode": side_effect_context.authority.account_mode,
        },
        "payload": redact(payload),
    });
    runtime.events.append(EventAppend {
        stream: format!("agent-deployment:{}", deployment.deployment_id),
        event_type: event_type.to_string(),
        aggregate_id: deployment.deployment_id.clone(),
        payload: redacted.clone(),
        idempotency_key: IdempotencyKey::new(key.clone())?,
        occurred_at_ms: now_ms,
        retention_until_ms: None,
    })?;
    runtime.outbox.append(OutboxRecord {
        outbox_id: format!("outbox_{}", short_hash(&key)),
        topic: event_type.to_string(),
        payload: redacted,
        idempotency_key: IdempotencyKey::new(format!("outbox:{key}"))?,
        created_at_ms: now_ms,
        fencing_token: 0,
    })
}

fn receipt(deployment: &AgentDeployment, outcome: &str, run_id: Option<&str>) -> Value {
    json!({"schemaVersion": SCHEMA_VERSION, "deploymentId": deployment.deployment_id, "outcome": outcome, "runId": run_id, "redacted": true})
}

fn failure_receipt(deployment: &AgentDeployment, error: &str) -> Value {
    match error {
        "agent_entitlement_denied" => json!({
            "schemaVersion": SCHEMA_VERSION,
            "deploymentId": deployment.deployment_id,
            "outcome": "entitlement_denied",
            "errorCode": "agent_entitlement_denied",
            "redacted": true,
        }),
        "agent_run_pending_reconcile" => json!({
            "schemaVersion": SCHEMA_VERSION,
            "deploymentId": deployment.deployment_id,
            "outcome": "pending_reconcile",
            "errorCode": "agent_run_pending_reconcile",
            "redacted": true,
        }),
        _ => json!({
            "schemaVersion": SCHEMA_VERSION,
            "deploymentId": deployment.deployment_id,
            "outcome": "failed",
            "errorCode": "agent_runner_failed",
            "redacted": true,
        }),
    }
}

fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("fixed agent runner key"),
    )
}

fn short_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..24].to_string()
}

/// Stable, non-reversible lookup key for a runner-issued MCP capability.
/// The raw token is process-local and is never persisted or surfaced in
/// evidence, command receipts, or MCP responses.
pub fn mcp_capability_digest(token: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(token.as_bytes()))
}

fn recovery_request_digest(
    deployment: &AgentDeployment,
    side_effect_context: &SideEffectContext,
) -> String {
    short_hash(
        &json!({
            "deploymentId": deployment.deployment_id,
            "actor": side_effect_context.authority.actor,
            "surface": side_effect_context.authority.surface,
            "accountMode": side_effect_context.authority.account_mode,
            "idempotencyKey": side_effect_context.idempotency_key.as_str(),
        })
        .to_string(),
    )
}

fn redact(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        for key in ["apiKey", "apiSecret", "token", "secret", "authorization"] {
            object.remove(key);
        }
    }
    value
}

pub fn extract_session_ref(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|value| {
            value
                .pointer("/thread_id")
                .or_else(|| value.pointer("/session_id"))
                .or_else(|| value.pointer("/threadId"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn secret_shaped(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "api secret",
        "apisecret",
        "access_token",
        "access token",
        "refresh_token",
        "refresh token",
        "client_secret",
        "client secret",
        "private_key",
        "private key",
        "authorization:",
        "bearer ",
        "password=",
        "password:",
        "secret=",
        "secret:",
    ]
    .into_iter()
    .any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::TradeAssemblyService;

    pub(super) fn deployment() -> AgentDeployment {
        AgentDeployment {
            executor: AgentExecutor::default(),
            deployment_id: "schedule-state-test".to_string(),
            system_project_id: "system-test".to_string(),
            agent_definition_version_id: "agent-v1".to_string(),
            execution_config_version_id: "config-v1".to_string(),
            studio_tool_allowlist: vec!["studio.deployment.inspect".to_string()],
            desired_state: "active".to_string(),
            interval_seconds: 60,
            cron_utc: None,
            mode: "paper".to_string(),
            prompt: "Use redacted Studio evidence.".to_string(),
            workspace: std::env::temp_dir().display().to_string(),
            runtime_profile: "local-read-only".to_string(),
        }
    }

    #[test]
    fn later_coalesced_schedule_state_cannot_be_rolled_back_by_a_completed_run() {
        let db = format!(
            ".tradeassembly/test-agent-schedule-state-{}.db",
            std::process::id()
        );
        let _ = std::fs::remove_file(&db);
        let service = TradeAssemblyService::test_local(&db);
        let deployment = deployment();
        put_deployment(&service.runtime(), &deployment).expect("test deployment persists");
        let schedule = deployment_schedule(&deployment).expect("test schedule is valid");
        let mut earlier = initial_schedule_state(&schedule);
        earlier.next_due_at_ms = 120_000;
        earlier.last_due_at_ms = Some(60_000);
        earlier.updated_at_ms = 60_000;
        let mut later = earlier.clone();
        later.next_due_at_ms = 180_000;
        later.last_due_at_ms = Some(120_000);
        later.updated_at_ms = 120_000;

        persist_schedule_state(&service.runtime(), &deployment, &later)
            .expect("coalesced state persists");
        persist_schedule_state(&service.runtime(), &deployment, &earlier)
            .expect("completed state is safely ignored");

        let state = service
            .runtime()
            .storage
            .get_json(SCHEDULES_NS, &deployment.deployment_id)
            .expect("schedule reads")
            .expect("schedule exists");
        assert_eq!(state["nextDueAtMs"], 180_000);
        let _ = std::fs::remove_file(db);
    }

    #[test]
    fn runner_mcp_capability_is_active_run_scoped_and_fail_closed() {
        let db = format!(
            ".tradeassembly/test-agent-mcp-capability-{}.db",
            std::process::id()
        );
        let _ = std::fs::remove_file(&db);
        let service = TradeAssemblyService::test_local(&db);
        let mut deployment = deployment();
        deployment.mode = "live".to_string();
        deployment.studio_tool_allowlist = vec![
            "studio.deployment.inspect".to_string(),
            "studio.execution.external_receipt.append".to_string(),
            "tradeassembly.health".to_string(),
        ];
        put_deployment(&service.runtime(), &deployment).expect("deployment persists");
        let binding_digest = deployment_binding_digest(&deployment);
        let capability = AgentMcpExecutionCapability::issue();
        let capability_digest = capability.digest();
        let capability_record = AgentMcpCapabilityRecord {
            schema_version: SCHEMA_VERSION.to_string(),
            deployment_id: deployment.deployment_id.clone(),
            run_id: "run-capability".to_string(),
            binding_digest: binding_digest.clone(),
            mode: deployment.mode.clone(),
            studio_tool_allowlist: deployment.studio_tool_allowlist.clone(),
            capability_digest: capability_digest.clone(),
        };
        let supervisor_lease = service
            .runtime()
            .leases
            .acquire(
                &format!("agent-deployment:{}", deployment.deployment_id),
                "controlled-supervisor",
                service.runtime().clock.trusted_now_ms().unwrap(),
                600_000,
            )
            .unwrap()
            .unwrap();
        service
            .runtime()
            .storage
            .put_json(
                RUNS_NS,
                "run-capability",
                json!({
                    "run": {
                        "runId": "run-capability",
                        "deploymentId": deployment.deployment_id,
                        "state": "running",
                        "leaseFence": supervisor_lease.fencing_token,
                    },
                    "bindingDigest": binding_digest,
                    "mcpCapabilityDigest": capability_digest,
                }),
                &context("capability-run"),
            )
            .expect("run persists");
        service
            .runtime()
            .storage
            .put_json(
                ACTIVE_RUNS_NS,
                &deployment.deployment_id,
                json!({
                    "runId": "run-capability",
                    "state": "running",
                    "leaseExpiresAtMs": i64::MAX,
                    "leaseFence": supervisor_lease.fencing_token,
                }),
                &context("capability-active"),
            )
            .expect("active pointer persists");
        service
            .runtime()
            .storage
            .put_json(
                MCP_CAPABILITIES_NS,
                &capability_digest,
                serde_json::to_value(&capability_record).expect("capability serializes"),
                &context("capability-record"),
            )
            .expect("capability persists");

        let resolved = resolve_mcp_execution_context(&service.runtime(), Some(capability.token()))
            .expect("capability resolves")
            .expect("runner-scoped capability");
        // The portable side-effect context preserves typed identity. It is
        // not reconstructed from authority text and needs no scheduler tick.
        let ordinary_context = context("ordinary-plugin-call");
        assert!(ordinary_context.agent_execution().is_none());
        let plugin_context = ordinary_context.with_agent_execution(Some(&resolved));
        let carried = plugin_context.agent_execution().unwrap();
        assert_eq!(carried, &resolved);
        assert_eq!(
            carried
                .current_lease(
                    service.runtime().storage.as_ref(),
                    service.runtime().clock.as_ref(),
                    service.runtime().leases.as_ref()
                )
                .unwrap(),
            supervisor_lease
        );
        let deployment_record = service
            .runtime()
            .storage
            .get_json(DEPLOYMENTS_NS, &deployment.deployment_id)
            .unwrap()
            .unwrap();
        for desired in ["paused", "stopped"] {
            let mut paused = deployment_record.clone();
            paused["deployment"]["desiredState"] = json!(desired);
            service
                .runtime()
                .storage
                .put_json(
                    DEPLOYMENTS_NS,
                    &deployment.deployment_id,
                    paused,
                    &context("controlled-desired-state"),
                )
                .unwrap();
            assert!(carried
                .current_lease(
                    service.runtime().storage.as_ref(),
                    service.runtime().clock.as_ref(),
                    service.runtime().leases.as_ref()
                )
                .is_err());
        }
        service
            .runtime()
            .storage
            .put_json(
                DEPLOYMENTS_NS,
                &deployment.deployment_id,
                deployment_record,
                &context("restore-controlled-desired-state"),
            )
            .unwrap();
        carried
            .revalidate(
                service.runtime().storage.as_ref(),
                service.runtime().clock.as_ref(),
            )
            .expect("current agent identity remains valid at the port boundary");
        assert!(service
            .runtime()
            .storage
            .list_json("execution_attempts")
            .unwrap()
            .is_empty());
        assert_service_paths_carry_agent_identity(&service, &resolved);
        assert_eq!(
            resolved.allowed_tools(),
            [
                "studio.deployment.inspect",
                "studio.execution.external_receipt.append",
                "tradeassembly.health",
            ]
        );
        let bound = bind_mcp_execution_context(
            &service.runtime(),
            &resolved,
            "studio.deployment.inspect",
            json!({
                "authority_context": {"actor": "verified-user", "surface": "mcp", "accountMode": "live"}
            }),
        )
        .expect("allowed tool is bound");
        assert_eq!(bound["deployment_id"], deployment.deployment_id);
        let authenticated = service
            .for_authenticated_invocation(
                "test-issuer",
                "verified-user",
                None,
                Some("Verified User".to_string()),
            )
            .with_verified_agent_mcp_execution_context(resolved.clone());
        let ordinary = authenticated.call_mcp_tool("tradeassembly.health", json!({}));
        assert_eq!(ordinary["isError"], false, "{ordinary:#}");
        // A previously authenticated MCP process must lose authority when the
        // supervisor's persisted lease expires, without needing to reconnect.
        let active_before = service
            .runtime()
            .storage
            .get_json(ACTIVE_RUNS_NS, &deployment.deployment_id)
            .unwrap()
            .unwrap();
        let mut expired = active_before.clone();
        expired["leaseExpiresAtMs"] = json!(i64::MIN);
        service
            .runtime()
            .storage
            .put_json(
                ACTIVE_RUNS_NS,
                &deployment.deployment_id,
                expired,
                &context("capability-expired"),
            )
            .unwrap();
        assert!(
            resolve_mcp_execution_context(&service.runtime(), Some(capability.token())).is_err()
        );
        assert!(
            carried
                .revalidate(
                    service.runtime().storage.as_ref(),
                    service.runtime().clock.as_ref(),
                )
                .is_err(),
            "cloned side-effect identity cannot outlive its run lease"
        );
        let expired_call = authenticated.call_mcp_tool("tradeassembly.health", json!({}));
        assert_eq!(expired_call["isError"], true);
        assert!(expired_call
            .to_string()
            .contains("agent_mcp_execution_context_invalid"));
        service
            .runtime()
            .storage
            .put_json(
                ACTIVE_RUNS_NS,
                &deployment.deployment_id,
                active_before,
                &context("capability-test-lease-restored"),
            )
            .unwrap();
        let receipt = authenticated.call_mcp_tool(
            "studio.execution.external_receipt.append",
            json!({
                "broker": "alpaca",
                "client_order_id": "client-order-live",
                "event_type": "order_accepted",
                "receipt": {"status": "accepted"},
                "idempotency_key": "runner-live-external-receipt",
                "authority_context": {
                    "actor": "forged-user",
                    "surface": "mcp",
                    "accountMode": "paper"
                },
            }),
        );
        assert_eq!(receipt["isError"], false, "{receipt:#}");
        assert_eq!(
            receipt["structuredContent"]["receipt"]["environment"],
            "live"
        );
        assert_eq!(
            bind_mcp_execution_context(
                &service.runtime(),
                &resolved,
                "studio.agent_run.health",
                json!({"authority_context": {"actor": "verified-user", "surface": "mcp", "accountMode": "live"}}),
            )
            .unwrap_err(),
            "agent_mcp_tool_not_allowed"
        );
        assert_eq!(
            bind_mcp_execution_context(
                &service.runtime(),
                &resolved,
                "studio.deployment.inspect",
                json!({
                    "deployment_id": "other-deployment",
                    "authority_context": {"actor": "verified-user", "surface": "mcp", "accountMode": "live"}
                }),
            )
            .unwrap_err(),
            "agent_mcp_execution_context_mismatch"
        );
        // The cached active pointer remains unexpired. Replacing the real
        // lease must still invalidate the old child, including same-owner reuse.
        service.runtime().leases.release(&supervisor_lease).unwrap();
        let replacement = service
            .runtime()
            .leases
            .acquire(
                &supervisor_lease.resource,
                &supervisor_lease.owner,
                service.runtime().clock.trusted_now_ms().unwrap(),
                600_000,
            )
            .unwrap()
            .unwrap();
        assert!(replacement.fencing_token > supervisor_lease.fencing_token);
        assert_eq!(
            bind_mcp_execution_context(
                &service.runtime(),
                &resolved,
                "tradeassembly.health",
                json!({}),
            )
            .unwrap_err(),
            "agent_run_lease_invalid"
        );
        assert_eq!(
            revalidate_mcp_execution_context(&service.runtime(), &resolved).unwrap_err(),
            "agent_run_lease_invalid"
        );
        assert_eq!(
            authenticated.call_mcp_tool("tradeassembly.health", json!({}))["isError"],
            true,
            "an unexpired cached active pointer must not authorize a fenced-out MCP session"
        );
        assert!(carried
            .revalidate(
                service.runtime().storage.as_ref(),
                service.runtime().clock.as_ref()
            )
            .is_ok());
        assert_eq!(
            carried
                .current_lease(
                    service.runtime().storage.as_ref(),
                    service.runtime().clock.as_ref(),
                    service.runtime().leases.as_ref()
                )
                .unwrap_err(),
            "agent_run_lease_invalid"
        );
        service
            .runtime()
            .storage
            .put_json(
                RUNS_NS,
                "run-capability",
                json!({
                    "run": {
                        "runId": "run-capability",
                        "deploymentId": deployment.deployment_id,
                        "state": "completed",
                    },
                    "bindingDigest": binding_digest,
                    "mcpCapabilityDigest": capability_digest,
                }),
                &context("capability-completed"),
            )
            .expect("run completes");
        assert_eq!(
            bind_mcp_execution_context(
                &service.runtime(),
                &resolved,
                "studio.deployment.inspect",
                json!({"authority_context": {"actor": "verified-user", "surface": "mcp", "accountMode": "live"}}),
            )
            .unwrap_err(),
            "agent_mcp_execution_context_invalid"
        );
        let _ = std::fs::remove_file(db);
    }

    // Controlled sink: this proves identity propagation, not broker admission.
    // It performs no provider calls and deliberately returns no execution receipt.
    fn assert_service_paths_carry_agent_identity(
        base: &TradeAssemblyService,
        identity: &VerifiedAgentMcpExecutionContext,
    ) {
        use crate::ports::{
            PluginOperationPort, PluginOperationRequest, PluginOperationResponse, PortDescriptor,
            VersionedPort,
        };
        use std::sync::{Arc, Mutex};
        struct Capture(Arc<Mutex<Vec<SideEffectContext>>>);
        impl VersionedPort for Capture {
            fn descriptors(&self) -> Vec<PortDescriptor> {
                vec![]
            }
        }
        impl PluginOperationPort for Capture {
            fn invoke(
                &self,
                _: &PluginOperationRequest,
                context: &SideEffectContext,
            ) -> Result<PluginOperationResponse, String> {
                self.0.lock().unwrap().push(context.clone());
                Err("controlled_no_dispatch".into())
            }
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = (*base.runtime()).clone();
        runtime.plugin_operations = Arc::new(Capture(calls.clone()));
        let service = TradeAssemblyService::from_runtime(base.db(), runtime)
            .with_verified_agent_mcp_execution_context(identity.clone());
        let response = service.handle_http(
            "POST",
            "/plugins/instances/sim/operations/marketdata.bars.read_v1:invoke",
            json!({"idempotencyKey":"identity-provider", "mode":"paper", "assetClass":"crypto",
                "input":{"symbol":"BTC/USD","timeframe":"1m"}}),
        );
        assert_eq!(calls.lock().unwrap().len(), 1, "{response:?}");
        let request = json!({
            "pluginInstanceRef":"tradeassembly.simbroker.local", "pluginRef":"tradeassembly.simbroker",
            "manifestFingerprint":"test", "operationId":"broker.paper_order_submit",
            "capability":"broker.order_submit.paper", "capabilityGraphRevisionId":"test",
            "capabilityGraphFingerprint":"test", "strategyId":"test", "strategyVersionId":"test",
            "strategySpecHash":"test", "activationId":"test", "attemptId":"test",
            "evaluationTickId":"test", "mode":"paper", "purpose":"paper_trading",
            "timeoutMs":1000, "input":{"clientOrderId":"identity-order"}
        });
        let response = service.handle_http(
            "POST",
            "/product/run-center/run-once",
            json!({"idempotencyKey":"identity-order", "submitOrders":true,
                "symbol":"TEST", "qty":1, "side":"buy", "price":1,
                "authorityContext":{"actor":"caller-text", "surface":"mcp", "accountMode":"paper"},
                "pluginOperationRequest":request}),
        );
        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2, "{response:?}");
        for context in captured.iter() {
            assert_eq!(context.agent_execution(), Some(identity));
        }
    }

    #[test]
    fn recovery_replay_finalizes_a_receipt_after_the_run_transition() {
        let db = format!(
            ".tradeassembly/test-agent-recovery-replay-{}.db",
            std::process::id()
        );
        let _ = std::fs::remove_file(&db);
        let service = TradeAssemblyService::test_local(&db);
        let mut deployment = deployment();
        deployment.desired_state = "paused".to_string();
        put_deployment(&service.runtime(), &deployment).expect("deployment persists");
        let side_effect_context = SideEffectContext::new(
            AuthorityContext {
                actor: "verified-user".to_string(),
                surface: "mcp".to_string(),
                account_mode: "paper".to_string(),
            },
            IdempotencyKey::new("recover-after-transition").expect("idempotency"),
        );
        let request_digest = recovery_request_digest(&deployment, &side_effect_context);
        let receipt_key = format!("recovery_{request_digest}");
        let receipt = receipt(&deployment, "reconciled", Some("run-recovery"));
        service
            .runtime()
            .storage
            .put_json(
                RUNS_NS,
                "run-recovery",
                json!({
                    "run": {
                        "runId": "run-recovery",
                        "deploymentId": deployment.deployment_id,
                        "state": "reconciled",
                    }
                }),
                &side_effect_context,
            )
            .expect("transitioned run persists");
        service
            .runtime()
            .storage
            .put_json(
                ACTIVE_RUNS_NS,
                &deployment.deployment_id,
                json!({"state": "reconciled", "resolvedRunId": "run-recovery", "resolvedAtMs": 100}),
                &side_effect_context,
            )
            .expect("transitioned pointer persists");
        let in_progress = AgentRecoveryReceipt {
            schema_version: SCHEMA_VERSION.to_string(),
            deployment_id: deployment.deployment_id.clone(),
            run_id: "run-recovery".to_string(),
            request_digest,
            requested_at_ms: 100,
            state: "in_progress".to_string(),
            receipt: receipt.clone(),
        };
        service
            .runtime()
            .storage
            .put_json(
                RECOVERY_RECEIPTS_NS,
                &receipt_key,
                serde_json::to_value(&in_progress).expect("receipt serializes"),
                &side_effect_context,
            )
            .expect("in-progress receipt persists");

        let replay = recover_pending_run_with_context(
            &service.runtime(),
            &deployment.deployment_id,
            101,
            &side_effect_context,
        )
        .expect("replay finalizes recovery");
        assert_eq!(replay, receipt);
        let stored = service
            .runtime()
            .storage
            .get_json(RECOVERY_RECEIPTS_NS, &receipt_key)
            .expect("receipt reads")
            .expect("receipt exists");
        assert_eq!(stored["state"], "completed");
        assert_eq!(
            recover_pending_run_with_context(
                &service.runtime(),
                &deployment.deployment_id,
                102,
                &side_effect_context,
            )
            .expect("completed receipt replays"),
            receipt
        );
        let _ = std::fs::remove_file(db);
    }
}
