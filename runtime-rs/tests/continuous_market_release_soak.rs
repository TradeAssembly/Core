use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tradeassembly_runtime::adapters::plugin_operations::LocalPluginOperations;
use tradeassembly_runtime::ports::{
    AuthorityContext, IdempotencyKey, PluginOperationPort, PluginOperationRequest,
    PluginOperationResponse, PortDescriptor, SideEffectContext, VersionedPort,
};
use tradeassembly_runtime::service::TradeAssemblyService;

const TEST_NAME: &str = "continuous_market_release_soak";
const REQUIRED_PHASES: [&str; 6] = [
    "broker_down",
    "broker_recovery",
    "market_data_down",
    "market_data_recovery",
    "duplicate_delivery",
    "lease_contention_expiry_transfer",
];
const DIAGNOSTIC_MINIMUM_CYCLES: u64 = 10;
const CHILD_TIMEOUT_SECONDS: u64 = 120;
const PHASES: [&str; 10] = [
    "ordinary_before",
    "broker_down",
    "broker_recovery",
    "ordinary_between_providers",
    "market_data_down",
    "market_data_recovery",
    "ordinary_before_delivery",
    "duplicate_delivery",
    "lease_contention_expiry_transfer",
    "ordinary_after",
];
const EVALUATE_TICK_PATH: &str =
    "/product/strategy-execution-activations/{activation_id}/ticks/evaluate";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fault {
    None = 0,
    BrokerDown = 1,
    MarketDataDown = 2,
}

struct SoakPluginOperations {
    delegate: LocalPluginOperations,
    fault: Arc<AtomicU8>,
}

impl VersionedPort for SoakPluginOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl PluginOperationPort for SoakPluginOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        let fault = self.fault.load(Ordering::SeqCst);
        if fault == Fault::BrokerDown as u8 && request.operation_id == "broker.paper_order_submit" {
            return Err("soak_injected_broker_unavailable".to_string());
        }
        if fault == Fault::MarketDataDown as u8 && request.operation_id == "marketdata.bars.read_v1"
        {
            return Err("soak_injected_market_data_unavailable".to_string());
        }
        self.delegate.invoke(request, context)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SoakConfig {
    duration_seconds: u64,
    interval_seconds: u64,
    diagnostic: bool,
    source_revision: String,
    rust_toolchain: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SoakState {
    version: u32,
    config: SoakConfig,
    genesis_sha256: String,
    database: String,
    activation_id: String,
    correlation_id: String,
    started_at_ms: u64,
    planned_phases: Vec<String>,
    completed_phases: Vec<String>,
    completed_cycles: u64,
    event_sequence: u64,
    last_event_hash: Option<String>,
    process_invocations: Vec<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SoakGenesis {
    version: u32,
    config: SoakConfig,
    database: String,
    activation_id: String,
    correlation_id: String,
    started_at_ms: u64,
    run_nonce: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as u64
}

fn env_bool(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

fn soak_config() -> SoakConfig {
    let diagnostic = env_bool("TRADEASSEMBLY_SOAK_DIAGNOSTIC");
    let duration_seconds = env::var("TRADEASSEMBLY_SOAK_DURATION_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(if diagnostic { 8 } else { 86_400 });
    let interval_seconds = env::var("TRADEASSEMBLY_SOAK_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    SoakConfig {
        duration_seconds,
        interval_seconds,
        diagnostic,
        source_revision: env::var("TRADEASSEMBLY_SOAK_SOURCE_REVISION")
            .unwrap_or_else(|_| "unknown".to_string()),
        rust_toolchain: env::var("TRADEASSEMBLY_SOAK_RUST_TOOLCHAIN")
            .unwrap_or_else(|_| "unknown".to_string()),
    }
}

fn validate_config(config: &SoakConfig) -> Result<(), String> {
    if config.interval_seconds == 0 {
        return Err("interval_seconds_must_be_positive".to_string());
    }
    if !config.diagnostic && config.duration_seconds < 86_400 {
        return Err("release_duration_must_be_at_least_86400_seconds".to_string());
    }
    if schedule(config).len() < DIAGNOSTIC_MINIMUM_CYCLES as usize {
        return Err("insufficient_cycles_for_complete_fault_schedule".to_string());
    }
    Ok(())
}

fn schedule(config: &SoakConfig) -> Vec<String> {
    let cycles = config.duration_seconds / config.interval_seconds;
    let mut result = Vec::with_capacity(cycles as usize);
    for cycle in 0..cycles {
        result.push(
            PHASES
                .get(cycle as usize)
                .copied()
                .unwrap_or("ordinary_after")
                .to_string(),
        );
    }
    result
}

fn state_dir() -> PathBuf {
    env::var_os("TRADEASSEMBLY_SOAK_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".tradeassembly/continuous-market-soak"))
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    {
        let mut file = File::create(&tmp).map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|error| error.to_string())?;
    Ok(())
}

fn create_new_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("create_new_failed:{error}"))?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn supervisor_event(state: &SoakState, phase: &str, detail: Value) -> Result<Value, String> {
    let mut event = json!({
        "sequence": state.event_sequence + 1,
        "phase": phase,
        "status": "complete",
        "atMs": now_ms(),
        "detail": detail,
        "previousHash": state.last_event_hash,
    });
    let event_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&event).map_err(|error| error.to_string())?)
    );
    event["eventHash"] = json!(event_hash);
    Ok(event)
}

fn persist_supervisor_events(dir: &Path, state: &SoakState) -> Result<(), String> {
    let events = state
        .process_invocations
        .iter()
        .map(|invocation| {
            invocation["supervisorEvent"]
                .as_object()
                .map(|_| invocation["supervisorEvent"].clone())
                .ok_or_else(|| "process_supervisor_event_missing".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    verify_supervisor_events(&events)?;
    let path = dir.join("events.jsonl");
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|error| error.to_string())?;
    for event in events {
        file.write_all(
            serde_json::to_string(&event)
                .map_err(|error| error.to_string())?
                .as_bytes(),
        )
        .map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
    }
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(tmp, path).map_err(|error| error.to_string())?;
    Ok(())
}

fn repair_supervisor_events(dir: &Path, state: &SoakState) -> Result<(), String> {
    let embedded = state
        .process_invocations
        .iter()
        .filter(|invocation| invocation["supervisorEvent"].is_object())
        .count();
    if embedded == 0 {
        return Ok(());
    }
    if embedded != state.process_invocations.len() {
        return Err("process_supervisor_event_set_incomplete".to_string());
    }
    persist_supervisor_events(dir, state)
}

fn read_state(path: &Path) -> Result<SoakState, String> {
    let bytes = fs::read(path).map_err(|error| format!("state_read_failed:{error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("state_malformed:{error}"))
}

fn ensure_resume(dir: &Path, existing: &SoakState, config: &SoakConfig) -> Result<(), String> {
    if existing.version != 2
        || existing.config != *config
        || existing.planned_phases != schedule(config)
    {
        return Err("resume_state_mismatch".to_string());
    }
    if existing.activation_id.is_empty()
        || existing.correlation_id.is_empty()
        || existing.database.is_empty()
        || existing.completed_cycles != existing.completed_phases.len() as u64
        || existing.event_sequence != existing.completed_cycles
        || existing.process_invocations.len() as u64 != existing.completed_cycles
    {
        return Err("resume_state_malformed".to_string());
    }
    let expected_database = dir
        .join("runtime.sqlite")
        .canonicalize()
        .map_err(|error| format!("resume_database_path_invalid:{error}"))?;
    let actual_database = Path::new(&existing.database)
        .canonicalize()
        .map_err(|error| format!("resume_database_path_invalid:{error}"))?;
    if actual_database != expected_database {
        return Err("resume_database_outside_state_directory".to_string());
    }
    let genesis_path = dir.join("genesis.json");
    if sha256_file(&genesis_path)? != existing.genesis_sha256 {
        return Err("resume_genesis_digest_mismatch".to_string());
    }
    let genesis: SoakGenesis = serde_json::from_slice(
        &fs::read(&genesis_path).map_err(|error| format!("genesis_read_failed:{error}"))?,
    )
    .map_err(|error| format!("genesis_malformed:{error}"))?;
    if genesis.version != 1
        || genesis.config != existing.config
        || genesis.database != existing.database
        || genesis.activation_id != existing.activation_id
        || genesis.correlation_id != existing.correlation_id
        || genesis.started_at_ms != existing.started_at_ms
        || genesis.run_nonce.is_empty()
    {
        return Err("resume_genesis_binding_mismatch".to_string());
    }
    Ok(())
}

fn invocation_artifact<'a>(
    invocation: &'a Value,
    path_key: &str,
    digest_key: &str,
) -> Result<(&'a str, &'a str), String> {
    let path = invocation[path_key]
        .as_str()
        .ok_or_else(|| format!("process_{path_key}_missing"))?;
    let digest = invocation[digest_key]
        .as_str()
        .ok_or_else(|| format!("process_{digest_key}_missing"))?;
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("process_{path_key}_unsafe"));
    }
    Ok((path, digest))
}

fn verify_completed_process_evidence(dir: &Path, state: &SoakState) -> Result<u64, String> {
    if state.process_invocations.len() as u64 != state.completed_cycles {
        return Err("process_count_mismatch".to_string());
    }
    let interval_ms = state.config.interval_seconds.saturating_mul(1_000);
    let mut prior_observed_end = state.started_at_ms;
    for (index, invocation) in state.process_invocations.iter().enumerate() {
        let cycle = (index + 1) as u64;
        if invocation["cycle"].as_u64() != Some(cycle)
            || invocation["phase"].as_str() != state.completed_phases.get(index).map(String::as_str)
            || invocation["exitCode"].as_i64() != Some(0)
            || invocation["status"] != "cycle_committed"
        {
            return Err("process_identity_or_exit_invalid".to_string());
        }
        let started = invocation["supervisorStartedAtMs"]
            .as_u64()
            .ok_or_else(|| "process_supervisor_start_missing".to_string())?;
        let ended = invocation["supervisorObservedEndAtMs"]
            .as_u64()
            .ok_or_else(|| "process_supervisor_end_missing".to_string())?;
        if started < prior_observed_end.saturating_add(interval_ms) || ended < started {
            return Err("process_elapsed_interval_invalid".to_string());
        }
        for (path_key, digest_key) in [
            ("result", "resultSha256"),
            ("stdout", "stdoutSha256"),
            ("stderr", "stderrSha256"),
        ] {
            let (relative, expected) = invocation_artifact(invocation, path_key, digest_key)?;
            if sha256_file(&dir.join(relative))? != expected {
                return Err(format!("process_{path_key}_digest_mismatch"));
            }
        }
        let result_path = invocation["result"]
            .as_str()
            .ok_or_else(|| "process_result_missing".to_string())?;
        let result: Value = serde_json::from_slice(
            &fs::read(dir.join(result_path))
                .map_err(|error| format!("process_result_read_failed:{error}"))?,
        )
        .map_err(|error| format!("process_result_malformed:{error}"))?;
        if result["cycle"].as_u64() != Some(cycle)
            || result["phase"] != invocation["phase"]
            || result["pid"] != invocation["pid"]
            || result["committedAtMs"]
                .as_u64()
                .is_none_or(|committed| committed < started || committed > ended)
        {
            return Err("process_result_binding_invalid".to_string());
        }
        prior_observed_end = ended;
    }
    Ok(prior_observed_end.saturating_sub(state.started_at_ms) / 1_000)
}

fn storage_context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("valid test idempotency key"),
    )
}

fn service(db: &str, fault: Arc<AtomicU8>) -> TradeAssemblyService {
    let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.to_string());
    runtime.plugin_operations = Arc::new(SoakPluginOperations {
        delegate: LocalPluginOperations::new(Arc::clone(&runtime.storage)),
        fault,
    });
    TradeAssemblyService::from_runtime(db.to_string(), runtime)
}

fn activate(service: &TradeAssemblyService) -> Value {
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo", "mode": "paper", "providerRef": "sim",
            "schedulerIntervalSeconds": 1,
            "riskLimits": {"max_notional": 25.0, "max_order_quantity": 0.00017},
            "capabilityBindings": {
                "execution.market.bars": {"pluginInstanceRef":"sim", "pluginRef":"tradeassembly.simbroker", "operationId":"marketdata.bars.read_v1"},
                "execution.broker.submit": {"pluginInstanceRef":"sim", "pluginRef":"tradeassembly.simbroker", "operationId":"broker.paper_order_submit"}
            }
        }),
    );
    assert_eq!(saved.status, 200, "{:#}", saved.body);
    let activation = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": saved.body["body"]["configId"], "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "continuous-market-release-soak-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activation.status, 200, "{:#}", activation.body);
    activation.body["body"].clone()
}

fn persist_market_receipt(
    service: &TradeAssemblyService,
    activation: &Value,
    sequence: u64,
    close: f64,
) {
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let correlation_id = activation["run"]["correlationId"]
        .as_str()
        .expect("correlation id");
    let bar = json!({"symbol":"BTC/USD", "timeframe":"1m", "open":close, "high":close, "low":close, "close":close, "volume":10.0});
    let content_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&bar).expect("bar"))
    );
    let receipt = PluginOperationResponse {
        correlation_id: correlation_id.to_string(),
        schema_ref: "schema://market-data/bar@1".to_string(),
        payload: json!({"symbol":"BTC/USD", "barIndex":sequence, "openMicros":(close * 1_000_000.0) as i64, "highMicros":(close * 1_000_000.0) as i64, "lowMicros":(close * 1_000_000.0) as i64, "closeMicros":(close * 1_000_000.0) as i64, "volumeMicros":10_000_000i64, "timeframe":"1m", "sourceClass":"test_fixture"}),
        observed_at_ms: sequence as i64 * 60_000,
        source_event_id: format!("soak-bar-{sequence}"),
        content_hash: content_hash.clone(),
        freshness_state: "fresh".to_string(),
        evidence_refs: vec![format!("sim://bars/{sequence}")],
        deterministic: true,
        replayable: true,
        provider_outcome_id: None,
        reconciliation_required: false,
    };
    service
        .runtime()
        .storage
        .put_json(
            "plugin_operation_receipts",
            &format!("execution:{activation_id}:tick:{sequence}:market"),
            serde_json::to_value(receipt).expect("receipt"),
            &storage_context(&format!("soak-market-receipt:{activation_id}:{sequence}")),
        )
        .expect("persist market receipt");
}

fn evaluate(
    service: &TradeAssemblyService,
    activation: &Value,
    sequence: u64,
    persist_receipt: bool,
    attempt: &str,
) -> tradeassembly_runtime::service::ServiceResponse {
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let close = if sequence == 1 { 100.0 } else { 101.0 };
    let content_hash = format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({"symbol":"BTC/USD", "timeframe":"1m", "open":close, "high":close, "low":close, "close":close, "volume":10.0}))
                .expect("bar")
        )
    );
    let binding = activation["run"]["immutableInput"]["capabilityGraphSnapshot"]["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node.get("selected"))
        .find(|node| node["operationId"] == "marketdata.bars.read_v1")
        .expect("binding");
    if persist_receipt {
        persist_market_receipt(service, activation, sequence, close);
    }
    service.handle_http_from_source("scheduler", "POST", &EVALUATE_TICK_PATH.replace("{activation_id}", activation_id), json!({
        "tickId": format!("soak-tick-{sequence}-{attempt}"), "sequence": sequence,
        "strategyVersionId": activation["run"]["strategyVersionId"],
        "idempotencyKey": format!("{activation_id}:soak-tick-{sequence}-{attempt}"), "accountMode":"paper",
        "authorityContext":{"actor":"local-user", "surface":"scheduler", "accountMode":"paper"},
        "observation": {"schemaVersion":"tradeassembly.plugin_observation.v1", "eventId":format!("soak-bar-{sequence}"), "observedAtMs":sequence as i64 * 60_000,
          "freshness":{"state":"fresh", "maxAgeMs":activation["run"]["dataFreshnessPolicy"]["maxAgeMs"]},
          "source":{"pluginInstanceRef":binding["pluginInstanceRef"], "pluginRef":binding["pluginRef"], "manifestFingerprint":binding["manifestFingerprint"], "operationId":binding["operationId"], "capability":binding["operation"]["capability"]},
          "bar":{"symbol":"BTC/USD", "timeframe":"1m", "open":close, "high":close, "low":close, "close":close, "volume":10.0},
          "evidenceRefs":[format!("sim://bars/{sequence}"), format!("plugin-event://soak-bar-{sequence}"), format!("plugin-content://{content_hash}")],
          "deterministic":true, "replayable":true}
    }))
}

fn phase_fault(phase: &str) -> Fault {
    match phase {
        "broker_down" => Fault::BrokerDown,
        "market_data_down" => Fault::MarketDataDown,
        _ => Fault::None,
    }
}

fn namespace_values(service: &TradeAssemblyService, namespace: &str) -> Result<Vec<Value>, String> {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .map(|records| records.into_iter().map(|(_, value)| value).collect())
}

fn scheduler_run(
    service: &TradeAssemblyService,
    activation_id: &str,
    worker_id: &str,
    max_cycles: u64,
) -> tradeassembly_runtime::service::ServiceResponse {
    service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": worker_id,
            "maxCycles": max_cycles,
            "leaseTtlSeconds": 1,
        }),
    )
}

fn activation_sequence(service: &TradeAssemblyService, activation_id: &str) -> Result<u64, String> {
    service
        .runtime()
        .storage
        .get_json("execution_activations", activation_id)?
        .and_then(|record| record["lastCompletedSequence"].as_u64())
        .ok_or_else(|| "activation_sequence_missing".to_string())
}

fn tick_statuses(response: &tradeassembly_runtime::service::ServiceResponse) -> Vec<String> {
    response.body["body"]["ticks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tick| tick["status"].as_str().map(str::to_string))
        .collect()
}

fn child_cycle(dir: &Path) -> Result<(), String> {
    let config = soak_config();
    let state_path = dir.join("state.json");
    let state = read_state(&state_path)?;
    ensure_resume(dir, &state, &config)?;
    verify_completed_process_evidence(dir, &state)?;
    let phase = state
        .planned_phases
        .get(state.completed_cycles as usize)
        .ok_or_else(|| "no_cycle_due".to_string())?
        .clone();
    let fault = Arc::new(AtomicU8::new(phase_fault(&phase) as u8));
    let service = service(&state.database, Arc::clone(&fault));
    let run = service
        .runtime()
        .storage
        .list_json("execution_runs")
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|(_, value)| value)
        .find(|value| value["activationId"] == state.activation_id)
        .ok_or_else(|| "activation_reconstruction_failed".to_string())?;
    if !run["immutableInput"].is_object() {
        return Err("activation_payload_malformed".to_string());
    }
    let activation = json!({"activationId":state.activation_id, "run":run});
    let before_sequence = activation_sequence(&service, &state.activation_id)?;
    let detail = match phase.as_str() {
        "ordinary_before" => {
            if run["state"] != "active"
                || run["correlationId"] != state.correlation_id
                || before_sequence != 0
            {
                return Err("initial_reconstruction_failed".to_string());
            }
            json!({"reconstructed":true,"state":"active","studioStarted":false})
        }
        "broker_down" => {
            let response = scheduler_run(&service, &state.activation_id, "soak-broker-down", 2);
            let reconciliation = service
                .runtime()
                .storage
                .get_json("execution_reconciliation", &state.activation_id)?
                .ok_or_else(|| "broker_down_reconciliation_missing".to_string())?;
            let broker_error_visible = response.body["body"]["ticks"]
                .as_array()
                .is_some_and(|ticks| ticks.iter().any(|tick| tick["decision"]["kind"] == "error"));
            if response.status != 200
                || !broker_error_visible
                || reconciliation["state"] != "required"
            {
                return Err(format!("broker_down_not_visible:{:#}", response.body));
            }
            json!({"responseStatus":response.status,"tickStatuses":tick_statuses(&response),"reconciliation":"required"})
        }
        "broker_recovery" => {
            let response = scheduler_run(&service, &state.activation_id, "soak-broker-recovery", 2);
            let reconciliation = service
                .runtime()
                .storage
                .get_json("execution_reconciliation", &state.activation_id)?
                .ok_or_else(|| "broker_recovery_reconciliation_missing".to_string())?;
            if response.status != 200
                || !tick_statuses(&response)
                    .iter()
                    .any(|status| status == "complete")
                || reconciliation["state"] != "clear"
            {
                return Err(format!("broker_recovery_failed:{:#}", response.body));
            }
            json!({"responseStatus":response.status,"tickStatuses":tick_statuses(&response),"reconciliation":"clear"})
        }
        "market_data_down" => {
            let response = scheduler_run(&service, &state.activation_id, "soak-data-down", 2);
            let ticks = response.body["body"]["ticks"]
                .as_array()
                .ok_or_else(|| "market_data_down_ticks_missing".to_string())?;
            if response.status != 200
                || !ticks
                    .iter()
                    .any(|tick| tick["reason"].as_str() == Some("tick_processing_failed"))
            {
                return Err(format!("market_data_down_not_visible:{:#}", response.body));
            }
            json!({"responseStatus":response.status,"tickStatuses":tick_statuses(&response),"failure":"market_data_unavailable"})
        }
        "market_data_recovery" => {
            let response = scheduler_run(&service, &state.activation_id, "soak-data-recovery", 2);
            if response.status != 200
                || !tick_statuses(&response)
                    .iter()
                    .any(|status| status == "complete")
            {
                return Err(format!("market_data_recovery_failed:{:#}", response.body));
            }
            json!({"responseStatus":response.status,"tickStatuses":tick_statuses(&response)})
        }
        "duplicate_delivery" => {
            let sequence = activation_sequence(&service, &state.activation_id)? + 1;
            let response = evaluate(&service, &activation, sequence, true, "first");
            let duplicate = evaluate(&service, &activation, sequence, true, "first");
            if response.status != 200
                || duplicate.status != 200
                || duplicate.body["duplicate"] != true
            {
                return Err(format!(
                    "duplicate_delivery_failed:first={:#}:duplicate={:#}",
                    response.body, duplicate.body
                ));
            }
            service
                .runtime()
                .scheduler
                .cancel(&format!(
                    "execution:{}:cycle:{sequence}",
                    state.activation_id
                ))
                .map_err(|error| format!("duplicate_schedule_cancel_failed:{error}"))?;
            json!({"firstStatus":response.status,"duplicateStatus":duplicate.status,"duplicate":true})
        }
        "lease_contention_expiry_transfer" => {
            let next_sequence = activation_sequence(&service, &state.activation_id)? + 1;
            let held = service
                .runtime()
                .leases
                .acquire(
                    &format!("execution-tick:{}:{next_sequence}", state.activation_id),
                    "soak-lease-original",
                    service.runtime().clock.now_ms(),
                    1_000,
                )?
                .ok_or_else(|| "lease_original_acquire_failed".to_string())?;
            let contended =
                scheduler_run(&service, &state.activation_id, "soak-lease-contender", 1);
            if contended.status != 200
                || contended.body["body"]["ticks"][0]["status"] != "contended"
            {
                return Err(format!("lease_contention_failed:{:#}", contended.body));
            }
            let transferred =
                scheduler_run(&service, &state.activation_id, "soak-lease-recovery", 2);
            let transfer = &transferred.body["body"]["ticks"][1];
            let recovery_token = transfer["lease"]["fencingToken"]
                .as_i64()
                .ok_or_else(|| format!("lease_recovery_token_missing:{:#}", transferred.body))?;
            if transferred.status != 200
                || transfer["status"] != "complete"
                || recovery_token <= held.fencing_token
            {
                return Err(format!("lease_transfer_failed:{:#}", transferred.body));
            }
            let attempts = namespace_values(&service, "execution_attempts")?;
            let owners = attempts
                .iter()
                .filter(|attempt| attempt["cycle"] == next_sequence)
                .filter_map(|attempt| attempt["owner"].as_str())
                .collect::<BTreeSet<_>>();
            if owners != BTreeSet::from(["soak-lease-recovery"]) {
                return Err(format!("lease_owner_ambiguous:{owners:?}"));
            }
            json!({"contended":true,"originalFencingToken":held.fencing_token,"recoveryFencingToken":recovery_token,"acceptedOwner":"soak-lease-recovery"})
        }
        _ => {
            let response = scheduler_run(
                &service,
                &state.activation_id,
                &format!("soak-worker-{}", state.completed_cycles + 1),
                2,
            );
            if response.status != 200
                || !tick_statuses(&response)
                    .iter()
                    .any(|status| status == "complete")
            {
                return Err(format!("ordinary_cycle_failed:{phase}:{:#}", response.body));
            }
            json!({"responseStatus":response.status,"tickStatuses":tick_statuses(&response)})
        }
    };
    let after_sequence = activation_sequence(&service, &state.activation_id)?;
    let process_result_path = dir.join(format!(
        "process-{:04}-result.json",
        state.completed_cycles + 1
    ));
    let process_result = json!({
        "cycle":state.completed_cycles + 1,
        "phase":phase,
        "pid":std::process::id(),
        "program":env::current_exe().ok().map(|path| path.display().to_string()),
        "args":["--exact",TEST_NAME,"--ignored"],
        "supervisorStartedAtMs":env::var("TRADEASSEMBLY_SOAK_SUPERVISOR_STARTED_AT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok()),
        "committedAtMs":now_ms(),
        "beforeSequence":before_sequence,
        "afterSequence":after_sequence,
        "outcome":detail,
    });
    atomic_json(&process_result_path, &process_result)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn checkpoint_database(path: &Path) -> Result<(), String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    let (busy, _, _): (u64, u64, u64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| error.to_string())?;
    if busy != 0 {
        return Err("database_checkpoint_busy".to_string());
    }
    Ok(())
}

fn finalize_database(path: &Path) -> Result<String, String> {
    let mut prior_digest = None;
    for _ in 0..3 {
        checkpoint_database(path)?;
        let digest = sha256_file(path)?;
        if prior_digest.as_deref() == Some(digest.as_str()) {
            return Ok(digest);
        }
        prior_digest = Some(digest);
    }
    Err("database_digest_did_not_stabilize".to_string())
}

fn execution_event_hash(event: &Value) -> Result<String, String> {
    let value = json!({
        "activationId": event["activationId"],
        "correlationId": event["correlationId"],
        "sequence": event["sequence"],
        "eventType": event["eventType"],
        "payload": event["payload"],
        "previousHash": event["previousHash"],
    });
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&value).map_err(|error| error.to_string())?)
    ))
}

fn verify_execution_event_chain(
    execution_events: &mut [Value],
    correlation_id: &str,
) -> Result<(), String> {
    execution_events.sort_by_key(|event| event["sequence"].as_u64().unwrap_or_default());
    let mut previous = None;
    for (index, event) in execution_events.iter().enumerate() {
        if event["sequence"].as_u64() != Some((index + 1) as u64)
            || event["correlationId"] != correlation_id
            || event["previousHash"].as_str() != previous.as_deref()
            || event["eventHash"] != execution_event_hash(event)?
        {
            return Err("execution_event_sequence_or_hash_chain_invalid".to_string());
        }
        previous = event["eventHash"].as_str().map(str::to_string);
    }
    Ok(())
}

fn verify_supervisor_events(events: &[Value]) -> Result<(), String> {
    let mut previous = None;
    for (index, event) in events.iter().enumerate() {
        if event["sequence"].as_u64() != Some((index + 1) as u64)
            || event["previousHash"].as_str() != previous.as_deref()
        {
            return Err("event_sequence_or_previous_hash_invalid".to_string());
        }
        let mut hash_input = event.clone();
        let actual = hash_input
            .as_object_mut()
            .and_then(|object| object.remove("eventHash"))
            .ok_or_else(|| "event_hash_missing".to_string())?;
        let expected = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&hash_input).map_err(|error| error.to_string())?)
        );
        if actual != expected {
            return Err("event_hash_invalid".to_string());
        }
        previous = actual.as_str().map(str::to_string);
    }
    Ok(())
}

fn verify_final(dir: &Path, state: &SoakState) -> Result<Value, String> {
    let verified_elapsed_seconds = verify_completed_process_evidence(dir, state)?;
    let events_path = dir.join("events.jsonl");
    let events: Vec<Value> = fs::read_to_string(&events_path)
        .map_err(|error| error.to_string())?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect::<Result<_, _>>()?;
    if events.len() as u64 != state.completed_cycles {
        return Err("event_count_mismatch".to_string());
    }
    verify_supervisor_events(&events)?;
    if state.completed_phases != state.planned_phases {
        return Err("fault_schedule_incomplete".to_string());
    }
    for required in REQUIRED_PHASES {
        if state
            .completed_phases
            .iter()
            .filter(|phase| phase.as_str() == required)
            .count()
            != 1
        {
            return Err(format!("required_phase_count_invalid:{required}"));
        }
    }
    let reopened = service(&state.database, Arc::new(AtomicU8::new(Fault::None as u8)));
    let activations = reopened
        .runtime()
        .storage
        .list_json("execution_activations")
        .map_err(|error| error.to_string())?;
    if activations.len() != 1 {
        return Err("activation_identity_not_unique".to_string());
    }
    if activations[0].1["correlationId"] != state.correlation_id {
        return Err("activation_correlation_mismatch".to_string());
    }
    let orders = reopened.handle_http("GET", "/orders", json!({})).body;
    let order_values = orders
        .as_array()
        .ok_or_else(|| "orders_malformed".to_string())?;
    let mut identities = BTreeSet::new();
    for order in order_values {
        for field in ["id", "client_order_id"] {
            let identity = order[field]
                .as_str()
                .ok_or_else(|| format!("order_{field}_missing"))?;
            if !identities.insert(format!("{field}:{identity}")) {
                return Err(format!("duplicate_order_{field}"));
            }
        }
        if order["correlationId"] != state.correlation_id {
            return Err("order_correlation_mismatch".to_string());
        }
    }
    let plugin_receipts = namespace_values(&reopened, "plugin_operation_receipts")?;
    let mut provider_outcomes = BTreeSet::new();
    for receipt in plugin_receipts
        .iter()
        .filter(|receipt| receipt["schemaRef"] == "schema://broker/order-receipt@1")
    {
        let outcome = receipt["providerOutcomeId"]
            .as_str()
            .ok_or_else(|| "broker_provider_outcome_missing".to_string())?;
        if !provider_outcomes.insert(outcome.to_string()) {
            return Err("duplicate_broker_provider_outcome".to_string());
        }
    }
    if provider_outcomes.len() != order_values.len() {
        return Err("broker_order_receipt_count_mismatch".to_string());
    }
    let checkpoints = namespace_values(&reopened, "execution_checkpoints")?;
    let mut checkpoint_sequences = checkpoints
        .iter()
        .filter(|checkpoint| checkpoint["activationId"] == state.activation_id)
        .filter_map(|checkpoint| checkpoint["sequence"].as_u64())
        .collect::<Vec<_>>();
    checkpoint_sequences.sort_unstable();
    if checkpoint_sequences.is_empty()
        || checkpoint_sequences
            .windows(2)
            .any(|pair| pair[1] != pair[0] + 1)
    {
        return Err("checkpoint_sequence_invalid".to_string());
    }
    let mut execution_events = namespace_values(&reopened, "execution_events")?
        .into_iter()
        .filter(|event| event["activationId"] == state.activation_id)
        .collect::<Vec<_>>();
    verify_execution_event_chain(&mut execution_events, &state.correlation_id)?;
    verify_side_effect_idempotency(&state.database, &state.activation_id)?;
    let reconciliation = reopened
        .runtime()
        .storage
        .get_json("execution_reconciliation", &state.activation_id)?
        .ok_or_else(|| "reconciliation_missing".to_string())?;
    if reconciliation["state"] != "clear"
        || reconciliation["newEntriesPaused"] != false
        || reconciliation["ambiguousActions"]
            .as_array()
            .is_none_or(|actions| !actions.is_empty())
    {
        return Err("reconciliation_not_clear".to_string());
    }
    if !namespace_values(&reopened, "dead_letters")?.is_empty() {
        return Err("dead_letters_present".to_string());
    }
    let attempts = namespace_values(&reopened, "execution_attempts")?;
    let mut accepted_sequences = BTreeSet::new();
    for attempt in attempts
        .iter()
        .filter(|attempt| attempt["activationId"] == state.activation_id)
        .filter(|attempt| attempt["state"] == "completed")
    {
        let sequence = attempt["cycle"]
            .as_u64()
            .ok_or_else(|| "attempt_sequence_missing".to_string())?;
        if !accepted_sequences.insert(sequence) {
            return Err("duplicate_order_identity".to_string());
        }
    }
    Ok(json!({
        "supervisorEvents":events.len(),
        "executionEvents":execution_events.len(),
        "orders":order_values.len(),
        "positions":namespace_values(&reopened, "positions")?.len(),
        "checkpoints":checkpoint_sequences.len(),
        "attempts":attempts.len(),
        "brokerProviderOutcomes":provider_outcomes.len(),
        "activations":activations.len(),
        "verifiedProcessElapsedSeconds":verified_elapsed_seconds,
        "eventsSha256":sha256_file(&events_path)?,
    }))
}

fn verify_side_effect_idempotency(database: &str, activation_id: &str) -> Result<(), String> {
    let connection = Connection::open(database).map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT item_key, value_json, idempotency_key
             FROM tradeassembly_kv
             WHERE namespace IN ('execution_idempotency', 'execution_events')
             ORDER BY namespace, item_key",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut storage_keys = BTreeSet::new();
    let mut broker_keys = BTreeSet::new();
    for (item_key, value_json, storage_key) in rows {
        if !storage_keys.insert(storage_key) {
            return Err("duplicate_side_effect_storage_idempotency".to_string());
        }
        let value: Value = serde_json::from_str(&value_json).map_err(|error| error.to_string())?;
        if value["activationId"].as_str() == Some(activation_id)
            || value["activation_id"].as_str() == Some(activation_id)
            || item_key.contains(activation_id)
        {
            for entry in value["journal"].as_array().into_iter().flatten() {
                let key = entry["idempotency_key"]
                    .as_str()
                    .ok_or_else(|| "side_effect_journal_idempotency_missing".to_string())?;
                if !broker_keys.insert(key.to_string()) {
                    return Err("duplicate_side_effect_journal_idempotency".to_string());
                }
            }
        }
    }
    Ok(())
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            child.kill().map_err(|error| error.to_string())?;
            let _ = child.wait();
            return Err("soak_child_timeout".to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_child_pipe(pipe: Option<impl Read>) -> Result<String, String> {
    let mut output = String::new();
    let mut pipe = pipe.ok_or_else(|| "soak_child_pipe_missing".to_string())?;
    pipe.read_to_string(&mut output)
        .map_err(|error| error.to_string())?;
    Ok(output)
}

fn validate_child_transition(
    before: &SoakState,
    after: &SoakState,
    expected_phase: &str,
) -> Result<(), String> {
    if after.completed_cycles != before.completed_cycles + 1
        || after.completed_phases.len() != before.completed_phases.len() + 1
        || after.completed_phases[..before.completed_phases.len()] != before.completed_phases
        || after.completed_phases.last().map(String::as_str) != Some(expected_phase)
        || after.event_sequence != before.event_sequence + 1
        || after.last_event_hash.is_none()
        || after.process_invocations.len() != before.process_invocations.len() + 1
    {
        return Err("soak_child_state_transition_invalid".to_string());
    }
    Ok(())
}

fn commit_supervised_cycle(
    dir: &Path,
    state_path: &Path,
    state: &mut SoakState,
    phase: &str,
    mut invocation: Value,
    result: &Value,
) -> Result<(), String> {
    let cycle = state.completed_cycles + 1;
    let started = invocation["supervisorStartedAtMs"]
        .as_u64()
        .ok_or_else(|| "process_supervisor_start_missing".to_string())?;
    let ended = invocation["supervisorObservedEndAtMs"]
        .as_u64()
        .ok_or_else(|| "process_supervisor_end_missing".to_string())?;
    if result["cycle"].as_u64() != Some(cycle)
        || result["phase"].as_str() != Some(phase)
        || result["pid"] != invocation["pid"]
        || result["committedAtMs"]
            .as_u64()
            .is_none_or(|committed| committed < started || committed > ended)
    {
        return Err("process_result_binding_invalid".to_string());
    }
    let event = supervisor_event(
        state,
        phase,
        json!({
            "beforeSequence":result["beforeSequence"],
            "afterSequence":result["afterSequence"],
            "outcome":result["outcome"],
        }),
    )?;
    invocation["supervisorEvent"] = event.clone();
    state.completed_cycles = cycle;
    state.completed_phases.push(phase.to_string());
    state.event_sequence += 1;
    state.last_event_hash = event["eventHash"].as_str().map(str::to_string);
    state.process_invocations.push(invocation);
    atomic_json(state_path, state)?;
    persist_supervisor_events(dir, state)
}

fn recover_completed_child(
    dir: &Path,
    state_path: &Path,
    state: &mut SoakState,
) -> Result<bool, String> {
    if state.completed_cycles >= state.planned_phases.len() as u64 {
        return Ok(false);
    }
    let cycle = state.completed_cycles + 1;
    let result_path = dir.join(format!("process-{cycle:04}-result.json"));
    if !result_path.exists() {
        return Ok(false);
    }
    let result: Value = serde_json::from_slice(
        &fs::read(&result_path).map_err(|error| format!("recovery_result_read_failed:{error}"))?,
    )
    .map_err(|error| format!("recovery_result_malformed:{error}"))?;
    let phase = state.planned_phases[state.completed_cycles as usize].clone();
    let pid = result["pid"]
        .as_u64()
        .ok_or_else(|| "recovery_result_pid_missing".to_string())?;
    let started = result["supervisorStartedAtMs"]
        .as_u64()
        .ok_or_else(|| "recovery_result_supervisor_start_missing".to_string())?;
    let committed = result["committedAtMs"]
        .as_u64()
        .ok_or_else(|| "recovery_result_commit_missing".to_string())?;
    if result["cycle"].as_u64() != Some(cycle)
        || result["phase"].as_str() != Some(phase.as_str())
        || committed < started
    {
        return Err("recovery_result_binding_invalid".to_string());
    }

    let output_base = format!("process-{cycle:04}-{pid}.recovered");
    let stdout_path = dir.join(format!("{output_base}.stdout"));
    let stderr_path = dir.join(format!("{output_base}.stderr"));
    fs::write(
        &stdout_path,
        b"child completion recovered after supervisor interruption\n",
    )
    .map_err(|error| error.to_string())?;
    fs::write(&stderr_path, b"").map_err(|error| error.to_string())?;
    let invocation = json!({
        "cycle":cycle,
        "phase":phase,
        "pid":pid,
        "program":result["program"],
        "args":result["args"],
        "exitCode":0,
        "exitStatusSource":"durable_child_completion_marker",
        "recoveredAfterSupervisorInterruption":true,
        "result":result_path.file_name().and_then(|name| name.to_str()),
        "resultSha256":sha256_file(&result_path)?,
        "status":"cycle_committed",
        "supervisorStartedAtMs":started,
        "supervisorObservedEndAtMs":committed,
        "stdout":stdout_path.file_name().and_then(|name| name.to_str()),
        "stderr":stderr_path.file_name().and_then(|name| name.to_str()),
        "stdoutSha256":sha256_file(&stdout_path)?,
        "stderrSha256":sha256_file(&stderr_path)?,
    });
    commit_supervised_cycle(dir, state_path, state, &phase, invocation, &result)?;
    Ok(true)
}

fn host_name() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[test]
#[ignore = "requires an explicit local state directory; release mode runs for 24 real hours"]
fn continuous_market_release_soak() {
    let dir = state_dir();
    fs::create_dir_all(&dir).expect("create soak state dir");
    if env_bool("TRADEASSEMBLY_SOAK_CHILD") {
        child_cycle(&dir).expect("soak child cycle");
        return;
    }
    let config = soak_config();
    validate_config(&config).expect("valid soak configuration");
    let state_path = dir.join("state.json");
    let mut state = if state_path.exists() {
        let mut state = read_state(&state_path).expect("read resume state");
        ensure_resume(&dir, &state, &config).expect("resume state binding");
        recover_completed_child(&dir, &state_path, &mut state)
            .expect("recover completed child before retry");
        repair_supervisor_events(&dir, &state).expect("repair supervisor event projection");
        verify_completed_process_evidence(&dir, &state).expect("resume process evidence");
        state
    } else {
        let db = dir.join("runtime.sqlite");
        let _ = fs::remove_file(&db);
        let fault = Arc::new(AtomicU8::new(Fault::None as u8));
        let service = service(db.to_str().expect("utf8 db"), fault);
        let activation = activate(&service);
        let stopped = service.handle_http("POST", "/scheduler/stop", json!({}));
        assert_eq!(stopped.status, 200, "{:#}", stopped.body);
        let started_at_ms = now_ms();
        let genesis = SoakGenesis {
            version: 1,
            config: config.clone(),
            database: db.display().to_string(),
            activation_id: activation["activationId"]
                .as_str()
                .expect("activation id")
                .to_string(),
            correlation_id: activation["run"]["correlationId"]
                .as_str()
                .expect("correlation id")
                .to_string(),
            started_at_ms,
            run_nonce: format!(
                "sha256:{:x}",
                Sha256::digest(
                    format!(
                        "{}:{started_at_ms}:{}",
                        config.source_revision,
                        std::process::id()
                    )
                    .as_bytes()
                )
            ),
        };
        let genesis_path = dir.join("genesis.json");
        create_new_json(&genesis_path, &genesis).expect("persist immutable genesis");
        let state = SoakState {
            version: 2,
            config: config.clone(),
            genesis_sha256: sha256_file(&genesis_path).expect("hash genesis"),
            database: db.display().to_string(),
            activation_id: activation["activationId"]
                .as_str()
                .expect("activation id")
                .to_string(),
            correlation_id: activation["run"]["correlationId"]
                .as_str()
                .expect("correlation id")
                .to_string(),
            started_at_ms,
            planned_phases: schedule(&config),
            completed_phases: Vec::new(),
            completed_cycles: 0,
            event_sequence: 0,
            last_event_hash: None,
            process_invocations: Vec::new(),
        };
        atomic_json(&state_path, &state).expect("persist initial state");
        state
    };
    while state.completed_cycles < state.planned_phases.len() as u64 {
        std::thread::sleep(
            Duration::from_secs(config.interval_seconds) + Duration::from_millis(100),
        );
        let before = state.clone();
        let phase = before.planned_phases[before.completed_cycles as usize].clone();
        let child_started_at_ms = now_ms();
        let mut child = Command::new(env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg(TEST_NAME)
            .arg("--ignored")
            .env("TRADEASSEMBLY_SOAK_CHILD", "1")
            .env("TRADEASSEMBLY_SOAK_STATE_DIR", &dir)
            .env(
                "TRADEASSEMBLY_SOAK_SUPERVISOR_STARTED_AT_MS",
                child_started_at_ms.to_string(),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start soak child");
        let child_pid = child.id();
        let status = wait_for_child(&mut child, Duration::from_secs(CHILD_TIMEOUT_SECONDS))
            .expect("wait for soak child");
        let stdout = read_child_pipe(child.stdout.take()).expect("read child stdout");
        let stderr = read_child_pipe(child.stderr.take()).expect("read child stderr");
        let output_base = format!("process-{:04}-{child_pid}", before.completed_cycles + 1);
        let stdout_path = dir.join(format!("{output_base}.stdout"));
        let stderr_path = dir.join(format!("{output_base}.stderr"));
        fs::write(&stdout_path, stdout.as_bytes()).expect("persist child stdout");
        fs::write(&stderr_path, stderr.as_bytes()).expect("persist child stderr");
        assert!(
            status.success(),
            "soak child failed: {status}; stderr={stderr}"
        );
        state = read_state(&state_path).expect("reload unchanged child state");
        ensure_resume(&dir, &state, &config).expect("child state binding");
        assert_eq!(state, before, "child must not advance supervisor state");
        let process_result_path = dir.join(format!(
            "process-{:04}-result.json",
            before.completed_cycles + 1
        ));
        let process_result: Value = serde_json::from_slice(
            &fs::read(&process_result_path).expect("read child process result"),
        )
        .expect("parse child process result");
        let invocation = json!({
            "cycle":before.completed_cycles + 1,
            "phase":phase,
            "pid":child_pid,
            "program":env::current_exe().ok().map(|path| path.display().to_string()),
            "args":["--exact",TEST_NAME,"--ignored"],
            "exitCode":status.code(),
            "result":process_result_path.file_name().and_then(|name| name.to_str()),
            "resultSha256":sha256_file(&process_result_path).expect("hash result"),
            "status":"cycle_committed",
            "supervisorStartedAtMs":child_started_at_ms,
            "supervisorObservedEndAtMs":now_ms(),
            "stdout":stdout_path.file_name().and_then(|name| name.to_str()),
            "stderr":stderr_path.file_name().and_then(|name| name.to_str()),
            "stdoutSha256":sha256_file(&stdout_path).expect("hash stdout"),
            "stderrSha256":sha256_file(&stderr_path).expect("hash stderr"),
        });
        commit_supervised_cycle(
            &dir,
            &state_path,
            &mut state,
            &phase,
            invocation,
            &process_result,
        )
        .expect("commit supervised cycle");
        validate_child_transition(&before, &state, &phase).expect("child transition");
    }
    let mut counts = verify_final(&dir, &state).expect("final invariants");
    let database_path = Path::new(&state.database);
    let database_sha256 = finalize_database(database_path).expect("finalize database");
    counts
        .as_object_mut()
        .expect("counts object")
        .insert("databaseSha256".to_string(), json!(database_sha256));
    let elapsed = counts["verifiedProcessElapsedSeconds"]
        .as_u64()
        .expect("verified process elapsed seconds");
    let state_sha256 = sha256_file(&state_path).expect("hash state");
    let finalizer_revision = env::var("TRADEASSEMBLY_SOAK_FINALIZER_REVISION")
        .unwrap_or_else(|_| config.source_revision.clone());
    let superseded_receipt = env::var("TRADEASSEMBLY_SOAK_SUPERSEDED_RECEIPT_SHA256")
        .ok()
        .map(|sha256| {
            json!({
                "path":"receipt.pre-finalization.json",
                "sha256":sha256
            })
        });
    let receipt = json!({
        "schemaVersion":"tradeassembly.continuous_market_soak.v1",
        "status":"passed",
        "sourceRevision":config.source_revision,
        "finalizerRevision":finalizer_revision,
        "rustToolchain":config.rust_toolchain,
        "host":host_name(),
        "durationSeconds":config.duration_seconds,
        "intervalSeconds":config.interval_seconds,
        "diagnostic":config.diagnostic,
        "startedAtMs":state.started_at_ms,
        "endedAtMs":now_ms(),
        "actualElapsedSeconds":elapsed,
        "activationId":state.activation_id,
        "correlationId":state.correlation_id,
        "plannedPhases":state.planned_phases,
        "completedPhases":state.completed_phases,
        "processInvocations":state.process_invocations,
        "counts":counts,
        "artifacts":{
            "genesis":{"path":"genesis.json","sha256":state.genesis_sha256},
            "state":{"path":"state.json","sha256":state_sha256},
            "events":{"path":"events.jsonl","sha256":sha256_file(&dir.join("events.jsonl")).expect("hash events")},
            "database":{"path":"runtime.sqlite","sha256":database_sha256},
        },
        "supersededReceipt":superseded_receipt,
        "releaseQualified": qualifies(&config, elapsed, true)
    });
    atomic_json(&dir.join("receipt.json"), &receipt).expect("persist receipt");
    assert_eq!(
        sha256_file(database_path).expect("rehash final database"),
        receipt["artifacts"]["database"]["sha256"]
    );
}

fn qualifies(config: &SoakConfig, elapsed_seconds: u64, all_phases_complete: bool) -> bool {
    !config.diagnostic
        && config.duration_seconds >= 86_400
        && elapsed_seconds >= 86_400
        && all_phases_complete
}

#[test]
fn soak_schedule_is_deterministic_and_complete() {
    let config = SoakConfig {
        duration_seconds: 10,
        interval_seconds: 1,
        diagnostic: true,
        source_revision: "r".to_string(),
        rust_toolchain: "t".to_string(),
    };
    assert_eq!(schedule(&config), schedule(&config));
    assert_eq!(
        &schedule(&config)[..PHASES.len()],
        PHASES.map(str::to_string)
    );
}

#[test]
fn soak_qualification_requires_real_release_elapsed_time() {
    let release = SoakConfig {
        duration_seconds: 86_400,
        interval_seconds: 1,
        diagnostic: false,
        source_revision: "r".to_string(),
        rust_toolchain: "t".to_string(),
    };
    assert!(!qualifies(&release, 86_399, true));
    assert!(qualifies(&release, 86_400, true));
    let diagnostic = SoakConfig {
        diagnostic: true,
        ..release
    };
    assert!(!qualifies(&diagnostic, 100_000, true));
}

#[test]
fn finalized_database_digest_is_stable_after_quiescent_reconstruction() {
    let temp = tempfile::tempdir().expect("temp dir");
    let database = temp.path().join("runtime.sqlite");
    let service = service(
        database.to_str().expect("database path"),
        Arc::new(AtomicU8::new(Fault::None as u8)),
    );
    service
        .runtime()
        .storage
        .put_json(
            "soak_finalization",
            "proof",
            json!({"status":"durable"}),
            &storage_context("soak-finalization-proof"),
        )
        .expect("persist finalization proof");
    drop(service);

    let digest = finalize_database(&database).expect("finalize database");
    assert_eq!(sha256_file(&database).expect("rehash database"), digest);
}

#[test]
fn supervisor_commit_is_complete_and_event_projection_is_recoverable() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_path = temp.path().join("state.json");
    let result_path = temp.path().join("process-0001-result.json");
    let stdout_path = temp.path().join("process-0001-42.stdout");
    let stderr_path = temp.path().join("process-0001-42.stderr");
    let mut state = SoakState {
        version: 2,
        config: SoakConfig {
            duration_seconds: 10,
            interval_seconds: 1,
            diagnostic: true,
            source_revision: "r".to_string(),
            rust_toolchain: "t".to_string(),
        },
        genesis_sha256: "genesis".to_string(),
        database: "runtime.sqlite".to_string(),
        activation_id: "activation".to_string(),
        correlation_id: "correlation".to_string(),
        started_at_ms: 0,
        planned_phases: PHASES.map(str::to_string).to_vec(),
        completed_phases: Vec::new(),
        completed_cycles: 0,
        event_sequence: 0,
        last_event_hash: None,
        process_invocations: Vec::new(),
    };
    atomic_json(&state_path, &state).expect("persist initial state");
    let before = state.clone();
    let result = json!({
        "cycle":1,
        "phase":"ordinary_before",
        "pid":42,
        "committedAtMs":1001,
        "beforeSequence":0,
        "afterSequence":1,
        "outcome":{"status":"complete"},
    });
    atomic_json(&result_path, &result).expect("persist result");
    fs::write(&stdout_path, b"complete\n").expect("stdout");
    fs::write(&stderr_path, b"").expect("stderr");
    let invocation = json!({
        "cycle":1,
        "phase":"ordinary_before",
        "pid":42,
        "program":"soak-child",
        "args":["--exact",TEST_NAME,"--ignored"],
        "exitCode":0,
        "result":"process-0001-result.json",
        "resultSha256":sha256_file(&result_path).expect("result digest"),
        "status":"cycle_committed",
        "supervisorStartedAtMs":1000,
        "supervisorObservedEndAtMs":1002,
        "stdout":"process-0001-42.stdout",
        "stderr":"process-0001-42.stderr",
        "stdoutSha256":sha256_file(&stdout_path).expect("stdout digest"),
        "stderrSha256":sha256_file(&stderr_path).expect("stderr digest"),
    });

    commit_supervised_cycle(
        temp.path(),
        &state_path,
        &mut state,
        "ordinary_before",
        invocation,
        &result,
    )
    .expect("commit supervised cycle");
    validate_child_transition(&before, &state, "ordinary_before").expect("valid transition");
    verify_completed_process_evidence(temp.path(), &state).expect("complete process evidence");

    fs::remove_file(temp.path().join("events.jsonl")).expect("simulate projection loss");
    repair_supervisor_events(temp.path(), &state).expect("rebuild event projection");
    let events = fs::read_to_string(temp.path().join("events.jsonl")).expect("events");
    assert_eq!(events.lines().count(), 1);
    assert_eq!(read_state(&state_path).expect("durable state"), state);
}

#[test]
fn completed_child_result_recovers_before_phase_retry() {
    let temp = tempfile::tempdir().expect("temp dir");
    let state_path = temp.path().join("state.json");
    let result_path = temp.path().join("process-0001-result.json");
    let mut state = SoakState {
        version: 2,
        config: SoakConfig {
            duration_seconds: 10,
            interval_seconds: 1,
            diagnostic: true,
            source_revision: "r".to_string(),
            rust_toolchain: "t".to_string(),
        },
        genesis_sha256: "genesis".to_string(),
        database: "runtime.sqlite".to_string(),
        activation_id: "activation".to_string(),
        correlation_id: "correlation".to_string(),
        started_at_ms: 0,
        planned_phases: PHASES.map(str::to_string).to_vec(),
        completed_phases: Vec::new(),
        completed_cycles: 0,
        event_sequence: 0,
        last_event_hash: None,
        process_invocations: Vec::new(),
    };
    atomic_json(&state_path, &state).expect("persist initial state");
    atomic_json(
        &result_path,
        &json!({
            "cycle":1,
            "phase":"ordinary_before",
            "pid":42,
            "program":"soak-child",
            "args":["--exact",TEST_NAME,"--ignored"],
            "supervisorStartedAtMs":1000,
            "committedAtMs":1002,
            "beforeSequence":0,
            "afterSequence":1,
            "outcome":{"status":"complete"},
        }),
    )
    .expect("persist child completion marker");

    assert!(
        recover_completed_child(temp.path(), &state_path, &mut state)
            .expect("recover completed child")
    );
    assert_eq!(state.completed_cycles, 1);
    assert_eq!(state.completed_phases, vec!["ordinary_before"]);
    assert_eq!(
        state.process_invocations[0]["exitStatusSource"],
        "durable_child_completion_marker"
    );
    assert_eq!(
        state.process_invocations[0]["recoveredAfterSupervisorInterruption"],
        true
    );
    verify_completed_process_evidence(temp.path(), &state).expect("recovered process evidence");
    assert!(
        !recover_completed_child(temp.path(), &state_path, &mut state).expect("no second recovery")
    );
}

#[test]
fn soak_resume_rejects_configuration_mismatch_and_malformed_state() {
    let temp = tempfile::tempdir().expect("temp dir");
    let config = SoakConfig {
        duration_seconds: 10,
        interval_seconds: 1,
        diagnostic: true,
        source_revision: "r".to_string(),
        rust_toolchain: "t".to_string(),
    };
    let database = temp.path().join("runtime.sqlite");
    File::create(&database).expect("database");
    let database = database.display().to_string();
    let genesis = SoakGenesis {
        version: 1,
        config: config.clone(),
        database: database.clone(),
        activation_id: "a".to_string(),
        correlation_id: "c".to_string(),
        started_at_ms: 1,
        run_nonce: "nonce".to_string(),
    };
    let genesis_path = temp.path().join("genesis.json");
    create_new_json(&genesis_path, &genesis).expect("genesis");
    let state = SoakState {
        version: 2,
        config: config.clone(),
        genesis_sha256: sha256_file(&genesis_path).expect("genesis digest"),
        database,
        activation_id: "a".to_string(),
        correlation_id: "c".to_string(),
        started_at_ms: 1,
        planned_phases: schedule(&config),
        completed_phases: vec![],
        completed_cycles: 0,
        event_sequence: 0,
        last_event_hash: None,
        process_invocations: Vec::new(),
    };
    assert!(ensure_resume(temp.path(), &state, &config).is_ok());
    let bad = SoakConfig {
        source_revision: "other".to_string(),
        ..config
    };
    assert_eq!(
        ensure_resume(temp.path(), &state, &bad),
        Err("resume_state_mismatch".to_string())
    );
    let malformed = SoakState {
        activation_id: String::new(),
        ..state.clone()
    };
    assert_eq!(
        ensure_resume(temp.path(), &malformed, &malformed.config),
        Err("resume_state_malformed".to_string())
    );
    let forged = SoakState {
        started_at_ms: 0,
        ..state
    };
    assert_eq!(
        ensure_resume(temp.path(), &forged, &forged.config),
        Err("resume_genesis_binding_mismatch".to_string())
    );
}

#[test]
fn soak_rejects_short_release_and_malformed_event_log() {
    let config = SoakConfig {
        duration_seconds: 7,
        interval_seconds: 1,
        diagnostic: false,
        source_revision: "r".to_string(),
        rust_toolchain: "t".to_string(),
    };
    assert_eq!(
        validate_config(&config),
        Err("release_duration_must_be_at_least_86400_seconds".to_string())
    );
    assert!(serde_json::from_str::<SoakState>("{not json}").is_err());
}

#[test]
fn soak_rejects_malformed_child_transition_and_tampered_event() {
    let config = SoakConfig {
        duration_seconds: 10,
        interval_seconds: 1,
        diagnostic: true,
        source_revision: "r".to_string(),
        rust_toolchain: "t".to_string(),
    };
    let before = SoakState {
        version: 2,
        config,
        genesis_sha256: "digest".to_string(),
        database: "db".to_string(),
        activation_id: "a".to_string(),
        correlation_id: "c".to_string(),
        started_at_ms: 1,
        planned_phases: PHASES.map(str::to_string).to_vec(),
        completed_phases: Vec::new(),
        completed_cycles: 0,
        event_sequence: 0,
        last_event_hash: None,
        process_invocations: Vec::new(),
    };
    let malformed = before.clone();
    assert_eq!(
        validate_child_transition(&before, &malformed, "ordinary_before"),
        Err("soak_child_state_transition_invalid".to_string())
    );
    let event = json!({
        "sequence":1,
        "phase":"ordinary_before",
        "status":"complete",
        "atMs":1,
        "detail":{},
        "previousHash":null,
        "eventHash":"sha256:tampered",
    });
    assert_eq!(
        verify_supervisor_events(&[event]),
        Err("event_hash_invalid".to_string())
    );
}

#[test]
fn soak_rejects_duplicate_execution_sequence_and_side_effect_idempotency() {
    let mut first = json!({
        "activationId":"activation",
        "correlationId":"correlation",
        "sequence":1,
        "eventType":"tick",
        "payload":{},
        "previousHash":null,
    });
    first["eventHash"] = json!(execution_event_hash(&first).expect("first hash"));
    let mut duplicate = json!({
        "activationId":"activation",
        "correlationId":"correlation",
        "sequence":1,
        "eventType":"tick",
        "payload":{"duplicate":true},
        "previousHash":first["eventHash"],
    });
    duplicate["eventHash"] = json!(execution_event_hash(&duplicate).expect("duplicate hash"));
    assert_eq!(
        verify_execution_event_chain(&mut [first, duplicate], "correlation"),
        Err("execution_event_sequence_or_hash_chain_invalid".to_string())
    );

    let temp = tempfile::NamedTempFile::new().expect("temp db");
    let connection = Connection::open(temp.path()).expect("open db");
    connection
        .execute_batch(
            "CREATE TABLE tradeassembly_kv (
                namespace TEXT NOT NULL,
                item_key TEXT NOT NULL,
                value_json TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                authority_actor TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY(namespace, item_key)
            );
            INSERT INTO tradeassembly_kv VALUES
              ('execution_idempotency','one','{\"activation_id\":\"activation\"}','duplicate','actor',1),
              ('execution_idempotency','two','{\"activation_id\":\"activation\"}','duplicate','actor',2);",
        )
        .expect("seed duplicate");
    assert_eq!(
        verify_side_effect_idempotency(temp.path().to_str().expect("utf8 temp path"), "activation"),
        Err("duplicate_side_effect_storage_idempotency".to_string())
    );
}
