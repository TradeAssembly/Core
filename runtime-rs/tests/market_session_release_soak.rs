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

const TEST_NAME: &str = "market_session_release_soak";
const REQUIRED_PHASES: [&str; 3] = ["closed_before", "open", "closed_after"];
const DIAGNOSTIC_MINIMUM_CYCLES: u64 = 3;
const CHILD_TIMEOUT_SECONDS: u64 = 120;
const PHASES: [&str; 3] = ["closed_before", "open", "closed_after"];
const EVALUATE_TICK_PATH: &str =
    "/product/strategy-execution-activations/{activation_id}/ticks/evaluate";

// This is a declared standard-close equity option fixture. It is deliberately
// not an ETF or index option, whose session rules can differ from XNYS cash
// regular hours. The fixture does not generalize a 16:00 ET close to options.
const EQUITY_FIXTURE: &str = "ACME";
const EQUITY_OPTION_FIXTURE: &str = "ACME260821C00100000";

fn instrument_fixtures() -> Value {
    json!([
        {"strategyKind":"equity","assetClass":"equity","instrumentFamily":"equity","symbol":EQUITY_FIXTURE},
        {"strategyKind":"listed_option","assetClass":"option","instrumentFamily":"option_contract","underlying":EQUITY_FIXTURE,"contract":EQUITY_OPTION_FIXTURE,"calendar":"XNYS","sessionScope":"declared_standard_close_equity_option"}
    ])
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SoakState {
    version: u32,
    config: SoakConfig,
    genesis_sha256: String,
    database: String,
    activation_id: String,
    correlation_id: String,
    activations: Vec<ActivationBinding>,
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
    activations: Vec<ActivationBinding>,
    started_at_ms: u64,
    run_nonce: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivationBinding {
    label: String,
    activation_id: String,
    correlation_id: String,
    strategy_id: String,
    strategy_version_id: String,
    strategy_spec_hash: String,
    capability_graph_revision_id: String,
    capability_graph_fingerprint: String,
    symbol: String,
    asset_class: String,
    instrument_family: String,
    underlying: Option<String>,
    contract: Option<String>,
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
        .unwrap_or(if diagnostic { 3 } else { 1 });
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
    if config.diagnostic
        && config.duration_seconds / config.interval_seconds != DIAGNOSTIC_MINIMUM_CYCLES
    {
        return Err("diagnostic_requires_exactly_three_fixture_phases".to_string());
    }
    if schedule(config).len() != DIAGNOSTIC_MINIMUM_CYCLES as usize {
        return Err("insufficient_cycles_for_complete_fault_schedule".to_string());
    }
    Ok(())
}

fn schedule(_config: &SoakConfig) -> Vec<String> {
    PHASES.map(str::to_string).to_vec()
}

fn state_dir() -> PathBuf {
    env::var_os("TRADEASSEMBLY_SOAK_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".tradeassembly/market-session-soak"))
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

fn append_event(
    dir: &Path,
    state: &mut SoakState,
    phase: &str,
    status: &str,
    detail: Value,
) -> Result<(), String> {
    state.event_sequence += 1;
    let mut event = json!({
        "sequence": state.event_sequence,
        "phase": phase,
        "status": status,
        "atMs": now_ms(),
        "detail": detail,
        "previousHash": state.last_event_hash,
    });
    let event_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&event).map_err(|error| error.to_string())?)
    );
    event["eventHash"] = json!(event_hash);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("events.jsonl"))
        .map_err(|error| error.to_string())?;
    file.write_all(
        serde_json::to_string(&event)
            .map_err(|error| error.to_string())?
            .as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    state.last_event_hash = event["eventHash"].as_str().map(str::to_string);
    Ok(())
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

fn replace_json_string(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(text) if text == from => *text = to.to_string(),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| replace_json_string(value, from, to)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| replace_json_string(value, from, to)),
        _ => {}
    }
}

fn activate_strategy(
    service: &TradeAssemblyService,
    label: &str,
    symbol: &str,
    asset_class: &str,
    instrument_family: &str,
    underlying: Option<&str>,
    contract: Option<&str>,
) -> (Value, ActivationBinding) {
    let duplicated = service.handle_http(
        "POST",
        "/product/strategies/duplicate",
        json!({"strategyId":"strat_local_btc_demo","name":label}),
    );
    assert_eq!(duplicated.status, 200, "{:#}", duplicated.body);
    let strategy_id = duplicated.body["body"]["strategy"]["id"]
        .as_str()
        .expect("strategy id")
        .to_string();
    let mut spec = duplicated.body["body"]["draft"]["spec"].clone();
    replace_json_string(&mut spec, "BTC/USD", symbol);
    replace_json_string(&mut spec, "CRYPTO_24X7", "XNYS");
    replace_json_string(&mut spec, "crypto_spot", instrument_family);
    replace_json_string(&mut spec, "crypto", asset_class);
    let saved_draft = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId":strategy_id,"name":label,"spec":spec}),
    );
    assert_eq!(saved_draft.status, 200, "{:#}", saved_draft.body);
    assert_eq!(
        saved_draft.body["body"]["validation"]["ok"], true,
        "{:#}",
        saved_draft.body
    );
    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({"strategyId":strategy_id,"expectedDraftHash":saved_draft.body["body"]["draft"]["draftHash"],"actor":{"kind":"user","id":"user.local"}}),
    );
    assert_eq!(published.status, 200, "{:#}", published.body);
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": strategy_id, "mode": "paper", "providerRef": "sim", "symbol":symbol,
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
            "configId": saved.body["body"]["configId"], "strategyId": strategy_id,
            "idempotencyKey": format!("market-session-release-soak-{label}"),
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activation.status, 200, "{:#}", activation.body);
    let activation = activation.body["body"].clone();
    let run = &activation["run"];
    let binding = ActivationBinding {
        label: label.to_string(),
        activation_id: activation["activationId"]
            .as_str()
            .expect("activation id")
            .to_string(),
        correlation_id: run["correlationId"]
            .as_str()
            .expect("correlation id")
            .to_string(),
        strategy_id,
        strategy_version_id: run["strategyVersionId"]
            .as_str()
            .expect("strategy version")
            .to_string(),
        strategy_spec_hash: run["strategySpecHash"]
            .as_str()
            .expect("strategy hash")
            .to_string(),
        capability_graph_revision_id: run["capabilityGraphRevisionId"]
            .as_str()
            .expect("capability revision")
            .to_string(),
        capability_graph_fingerprint: run["capabilityGraphFingerprint"]
            .as_str()
            .expect("capability fingerprint")
            .to_string(),
        symbol: symbol.to_string(),
        asset_class: asset_class.to_string(),
        instrument_family: instrument_family.to_string(),
        underlying: underlying.map(str::to_string),
        contract: contract.map(str::to_string),
    };
    (activation, binding)
}

fn activate_pair(service: &TradeAssemblyService) -> (Vec<Value>, Vec<ActivationBinding>) {
    let equity = activate_strategy(
        service,
        "static-equity",
        EQUITY_FIXTURE,
        "equity",
        "equity",
        None,
        None,
    );
    let option = activate_strategy(
        service,
        "underlying-single-leg-option",
        EQUITY_OPTION_FIXTURE,
        "option",
        "option_contract",
        Some(EQUITY_FIXTURE),
        Some(EQUITY_OPTION_FIXTURE),
    );
    (vec![equity.0, option.0], vec![equity.1, option.1])
}

fn persist_market_receipt(
    service: &TradeAssemblyService,
    activation: &Value,
    sequence: u64,
    close: f64,
    symbol: &str,
    observed_at_ms: i64,
) {
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let correlation_id = activation["run"]["correlationId"]
        .as_str()
        .expect("correlation id");
    let bar = json!({"symbol":symbol, "timeframe":"1m", "open":close, "high":close, "low":close, "close":close, "volume":10.0});
    let content_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&bar).expect("bar"))
    );
    let receipt = PluginOperationResponse {
        correlation_id: correlation_id.to_string(),
        schema_ref: "schema://market-data/bar@1".to_string(),
        payload: json!({"symbol":symbol, "barIndex":sequence, "openMicros":(close * 1_000_000.0) as i64, "highMicros":(close * 1_000_000.0) as i64, "lowMicros":(close * 1_000_000.0) as i64, "closeMicros":(close * 1_000_000.0) as i64, "volumeMicros":10_000_000i64, "timeframe":"1m", "sourceClass":"test_fixture"}),
        observed_at_ms,
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
    symbol: &str,
    observed_at_ms: i64,
) -> tradeassembly_runtime::service::ServiceResponse {
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let close = if sequence == 1 { 100.0 } else { 101.0 };
    let content_hash = format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({"symbol":symbol, "timeframe":"1m", "open":close, "high":close, "low":close, "close":close, "volume":10.0}))
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
        persist_market_receipt(service, activation, sequence, close, symbol, observed_at_ms);
    }
    service.handle_http_from_source("scheduler", "POST", &EVALUATE_TICK_PATH.replace("{activation_id}", activation_id), json!({
        "tickId": format!("soak-tick-{sequence}-{attempt}"), "sequence": sequence,
        "strategyVersionId": activation["run"]["strategyVersionId"],
        "idempotencyKey": format!("{activation_id}:soak-tick-{sequence}-{attempt}"), "accountMode":"paper",
        "authorityContext":{"actor":"local-user", "surface":"scheduler", "accountMode":"paper"},
        "observation": {"schemaVersion":"tradeassembly.plugin_observation.v1", "eventId":format!("soak-bar-{sequence}"), "observedAtMs":observed_at_ms,
          "freshness":{"state":"fresh", "maxAgeMs":activation["run"]["dataFreshnessPolicy"]["maxAgeMs"]},
          "source":{"pluginInstanceRef":binding["pluginInstanceRef"], "pluginRef":binding["pluginRef"], "manifestFingerprint":binding["manifestFingerprint"], "operationId":binding["operationId"], "capability":binding["operation"]["capability"]},
          "bar":{"symbol":symbol, "timeframe":"1m", "open":close, "high":close, "low":close, "close":close, "volume":10.0},
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

fn phase_observed_at_ms(config: &SoakConfig, phase: &str) -> Result<i64, String> {
    if !config.diagnostic {
        return i64::try_from(now_ms()).map_err(|_| "wall_clock_out_of_range".to_string());
    }
    match phase {
        // 2026-07-27 09:29, 09:31, and 16:01 America/New_York.
        "closed_before" => Ok(1_785_158_540_000),
        "open" => Ok(1_785_159_060_000),
        "closed_after" => Ok(1_785_182_460_000),
        _ => Err("market_session_phase_invalid".to_string()),
    }
}

fn resolve_xnys_session(
    service: &TradeAssemblyService,
    activation: &Value,
    phase: &str,
    observed_at_ms: i64,
) -> Result<Value, String> {
    let run = &activation["run"];
    let request = PluginOperationRequest {
        correlation_id: run["correlationId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        plugin_instance_ref: "core-runtime".to_string(),
        plugin_ref: "tradeassembly.core-runtime".to_string(),
        manifest_fingerprint: "local-core-runtime-v1".to_string(),
        operation_id: "calendar.session.resolve_v1".to_string(),
        capability: "calendar.session.resolve@1".to_string(),
        capability_graph_revision_id: run["capabilityGraphRevisionId"]
            .as_str()
            .unwrap_or("local")
            .to_string(),
        capability_graph_fingerprint: run["capabilityGraphFingerprint"]
            .as_str()
            .unwrap_or("local")
            .to_string(),
        strategy_id: run["strategyId"].as_str().unwrap_or_default().to_string(),
        strategy_version_id: run["strategyVersionId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        strategy_spec_hash: run["strategySpecHash"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        activation_id: activation["activationId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        attempt_id: format!("market-session-{phase}"),
        evaluation_tick_id: format!("market-session-{phase}"),
        mode: "paper".to_string(),
        purpose: "paper_trading".to_string(),
        account_ref: None,
        timeout_ms: 10_000,
        fencing_token: None,
        input: json!({"calendarRef":"XNYS","observedAtMs":observed_at_ms,"sequence":1}),
        evidence_refs: vec![format!("fixture://xnys/{phase}")],
    };
    let response = service.runtime().plugin_operations.invoke(
        &request,
        &storage_context(&format!("market-session-calendar:{phase}:{observed_at_ms}")),
    )?;
    if response.schema_ref != "schema://calendar/session-resolution@1" {
        return Err("calendar_schema_mismatch".to_string());
    }
    Ok(response.payload)
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

fn broker_side_effect_count(service: &TradeAssemblyService) -> Result<usize, String> {
    Ok(namespace_values(service, "plugin_operation_receipts")?
        .iter()
        .filter(|receipt| receipt["schemaRef"] == "schema://broker/order-receipt@1")
        .count())
}

fn evaluate_closed_phase(
    service: &TradeAssemblyService,
    reconstructed: &[Value],
    bindings: &[ActivationBinding],
    phase: &str,
    observed_at_ms: i64,
) -> Result<Value, String> {
    let broker_effects_before = broker_side_effect_count(service)?;
    let results = reconstructed
        .iter()
        .zip(bindings)
        .map(|(activation, binding)| {
            let sequence = activation_sequence(service, &binding.activation_id)? + 1;
            let response = evaluate(
                service,
                activation,
                sequence,
                true,
                phase,
                &binding.symbol,
                observed_at_ms,
            );
            let duplicate = evaluate(
                service,
                activation,
                sequence,
                true,
                phase,
                &binding.symbol,
                observed_at_ms,
            );
            if response.status != 200
                || response.body["body"]["decision"]["kind"] != "market_closed"
                || response.body["body"]["decision"]["facts"]["calendar"]["eligible"] != false
                || response.body["body"]["decision"]["facts"]["calendar"]["sessionState"]
                    != "closed"
                || response.body["body"]["orderResult"]["reason"] != "market_closed"
                || duplicate.status != 200
                || duplicate.body["duplicate"] != true
            {
                return Err(format!(
                    "closed_market_evaluation_not_suppressed:{phase}:{}:{:#}",
                    binding.activation_id, response.body
                ));
            }
            Ok(json!({
                "activationId": binding.activation_id,
                "strategySpecHash": binding.strategy_spec_hash,
                "capabilityGraphRevisionId": binding.capability_graph_revision_id,
                "symbol": binding.symbol,
                "decision": response.body["body"]["decision"]["kind"],
                "orderResult": response.body["body"]["orderResult"],
                "duplicate": duplicate.body["duplicate"],
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let broker_effects_after = broker_side_effect_count(service)?;
    if broker_effects_after != broker_effects_before {
        return Err(format!(
            "closed_market_created_broker_side_effect:{broker_effects_before}:{broker_effects_after}"
        ));
    }
    Ok(json!({
        "evaluations": results,
        "brokerSideEffectsBefore": broker_effects_before,
        "brokerSideEffectsAfter": broker_effects_after,
    }))
}

fn child_cycle(dir: &Path) -> Result<(), String> {
    let config = soak_config();
    let state_path = dir.join("state.json");
    let mut state = read_state(&state_path)?;
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
    let reconstructed = state
        .activations
        .iter()
        .map(|binding| {
            service
                .runtime()
                .storage
                .list_json("execution_runs")?
                .into_iter()
                .map(|(_, value)| value)
                .find(|value| value["activationId"] == binding.activation_id)
                .map(|run| json!({"activationId":binding.activation_id,"run":run}))
                .ok_or_else(|| "activation_reconstruction_failed".to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    if reconstructed.len() != 2
        || state.activations.iter().any(|binding| {
            binding.strategy_spec_hash.is_empty()
                || binding.capability_graph_revision_id.is_empty()
                || binding.capability_graph_fingerprint.is_empty()
        })
    {
        return Err("durable_activation_bindings_missing".to_string());
    }
    let before_sequence = activation_sequence(&service, &state.activation_id)?;
    let observed_at_ms = phase_observed_at_ms(&config, &phase)?;
    let calendar = resolve_xnys_session(&service, &activation, &phase, observed_at_ms)?;
    let expected_open = phase == "open";
    if calendar["calendarId"] != "XNYS" {
        return Err("calendar_transition_observation_invalid".to_string());
    }
    if calendar["sessionState"] != if expected_open { "open" } else { "closed" }
        || calendar["eligible"] != expected_open
        || calendar["nextTransitionAtMs"].as_i64().is_none()
    {
        return Err(if config.diagnostic {
            "calendar_transition_observation_invalid".to_string()
        } else {
            "market_phase_not_due".to_string()
        });
    }
    let detail = match phase.as_str() {
        "closed_before" => {
            if run["state"] != "active"
                || run["correlationId"] != state.correlation_id
                || before_sequence != 0
                || reconstructed
                    .iter()
                    .any(|activation| activation["run"]["state"] != "active")
            {
                return Err("initial_reconstruction_failed".to_string());
            }
            let suppression = evaluate_closed_phase(
                &service,
                &reconstructed,
                &state.activations,
                &phase,
                observed_at_ms,
            )?;
            json!({"reconstructed":true,"state":"active","decision":"market_closed","calendar":calendar,"suppression":suppression})
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
        "open" => {
            let results = reconstructed
                .iter()
                .zip(&state.activations)
                .map(|(activation, binding)| {
                    let sequence = activation_sequence(&service, &binding.activation_id)? + 1;
                    let response = evaluate(&service, activation, sequence, true, "first", &binding.symbol, observed_at_ms);
                    let duplicate = evaluate(&service, activation, sequence, true, "first", &binding.symbol, observed_at_ms);
                    if response.status != 200 || duplicate.status != 200 || duplicate.body["duplicate"] != true {
                        return Err("open_evaluation_or_idempotency_failed".to_string());
                    }
                    Ok(json!({"activationId":binding.activation_id,"strategySpecHash":binding.strategy_spec_hash,"capabilityGraphRevisionId":binding.capability_graph_revision_id,"symbol":binding.symbol,"firstStatus":response.status,"duplicateStatus":duplicate.status}))
                })
                .collect::<Result<Vec<_>, String>>()?;
            json!({"evaluations":results,"calendar":calendar,"instrumentFixtures":instrument_fixtures()})
        }
        "closed_after" => {
            if before_sequence == 0 {
                return Err("open_evaluation_missing".to_string());
            }
            let suppression = evaluate_closed_phase(
                &service,
                &reconstructed,
                &state.activations,
                &phase,
                observed_at_ms,
            )?;
            json!({"state":"active","decision":"market_closed","calendar":calendar,"suppression":suppression})
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
    state.completed_cycles += 1;
    state.completed_phases.push(phase.clone());
    let process_result_path =
        dir.join(format!("process-{:04}-result.json", state.completed_cycles));
    let process_result = json!({
        "cycle":state.completed_cycles,
        "phase":phase,
        "pid":std::process::id(),
        "committedAtMs":now_ms(),
        "beforeSequence":before_sequence,
        "afterSequence":after_sequence,
        "outcome":detail,
    });
    atomic_json(&process_result_path, &process_result)?;
    state.process_invocations.push(json!({
        "cycle":state.completed_cycles,
        "phase":phase,
        "pid":std::process::id(),
        "program":env::current_exe().ok().map(|path| path.display().to_string()),
        "args":["--exact",TEST_NAME,"--ignored"],
        "exitCode":0,
        "result":process_result_path.file_name().and_then(|name| name.to_str()),
        "resultSha256":sha256_file(&process_result_path)?,
        "status":"cycle_committed",
    }));
    append_event(
        dir,
        &mut state,
        &phase,
        "complete",
        json!({"beforeSequence":before_sequence,"afterSequence":after_sequence,"outcome":process_result["outcome"]}),
    )?;
    atomic_json(&state_path, &state)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
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
    let reopened = TradeAssemblyService::test_local(&state.database);
    let activations = reopened
        .runtime()
        .storage
        .list_json("execution_activations")
        .map_err(|error| error.to_string())?;
    if activations.len() != 2 || state.activations.len() != 2 {
        return Err("activation_identity_not_unique".to_string());
    }
    for binding in &state.activations {
        let activation = activations
            .iter()
            .find(|(_, activation)| {
                activation["id"] == binding.activation_id
                    || activation["activationId"] == binding.activation_id
            })
            .ok_or_else(|| "activation_binding_missing_after_restart".to_string())?;
        if activation.1["correlationId"] != binding.correlation_id {
            return Err("activation_correlation_mismatch".to_string());
        }
        if binding.label == "underlying-single-leg-option"
            && (binding.asset_class != "option"
                || binding.instrument_family != "option_contract"
                || binding.underlying.as_deref() != Some(EQUITY_FIXTURE)
                || binding.contract.as_deref() != Some(EQUITY_OPTION_FIXTURE))
        {
            return Err("option_activation_binding_invalid".to_string());
        }
    }
    let orders = reopened.handle_http("GET", "/orders", json!({})).body;
    let order_values = orders
        .as_array()
        .ok_or_else(|| "orders_malformed".to_string())?;
    let order_values = order_values
        .iter()
        .filter(|order| {
            state.activations.iter().any(|binding| {
                order["activation_id"] == binding.activation_id
                    || order["activationId"] == binding.activation_id
            })
        })
        .collect::<Vec<_>>();
    let mut identities = BTreeSet::new();
    for order in &order_values {
        for field in ["id", "client_order_id"] {
            let Some(identity) = order[field].as_str() else {
                continue;
            };
            if !identities.insert(format!("{field}:{identity}")) {
                return Err(format!("duplicate_order_{field}"));
            }
        }
        if !state
            .activations
            .iter()
            .any(|binding| order["correlationId"] == binding.correlation_id)
        {
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
    let all_execution_events = namespace_values(&reopened, "execution_events")?;
    for binding in &state.activations {
        let mut checkpoint_sequences = checkpoints
            .iter()
            .filter(|checkpoint| checkpoint["activationId"] == binding.activation_id)
            .filter_map(|checkpoint| checkpoint["sequence"].as_u64())
            .collect::<Vec<_>>();
        checkpoint_sequences.sort_unstable();
        if checkpoint_sequences.is_empty()
            || checkpoint_sequences
                .windows(2)
                .any(|pair| pair[1] != pair[0] + 1)
        {
            return Err(format!(
                "checkpoint_sequence_invalid:{}",
                binding.activation_id
            ));
        }
        let mut activation_events = all_execution_events
            .iter()
            .filter(|event| event["activationId"] == binding.activation_id)
            .cloned()
            .collect::<Vec<_>>();
        verify_execution_event_chain(&mut activation_events, &binding.correlation_id)?;
        verify_side_effect_idempotency(&state.database, &binding.activation_id)?;
    }
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
    let db_digest = sha256_file(Path::new(&state.database))?;
    Ok(json!({
        "supervisorEvents":events.len(),
        "executionEvents":all_execution_events.len(),
        "orders":order_values.len(),
        "positions":namespace_values(&reopened, "positions")?.len(),
        "checkpoints":checkpoints.len(),
        "attempts":attempts.len(),
        "brokerProviderOutcomes":provider_outcomes.len(),
        "activations":activations.len(),
        "verifiedProcessElapsedSeconds":verified_elapsed_seconds,
        "databaseSha256":db_digest,
        "eventsSha256":sha256_file(&events_path)?,
    }))
}

fn transition_observations(dir: &Path, diagnostic: bool) -> Result<Vec<Value>, String> {
    let events = fs::read_to_string(dir.join("events.jsonl")).map_err(|error| error.to_string())?;
    PHASES
        .iter()
        .map(|phase| {
            let event = events
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .find(|event| event["phase"] == *phase)
                .ok_or_else(|| "transition_event_missing".to_string())?;
            let calendar = &event["detail"]["outcome"]["calendar"];
            Ok(json!({
                "phase":phase,
                "sessionState":calendar["sessionState"],
                "observedAtMs":calendar["observedAtMs"],
                "calendarId":calendar["calendarId"],
                "calendarModel":calendar["calendarModel"],
                "nextTransitionKind":calendar["nextTransitionKind"],
                "nextTransitionAtMs":calendar["nextTransitionAtMs"],
                "supervisorObservedAtMs":event["atMs"],
                "clockSource":if diagnostic { "fixture_timestamp" } else { "system_wall_clock" },
                "fixture":diagnostic,
            }))
        })
        .collect()
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

fn child_is_waiting_for_market_phase(
    success: bool,
    diagnostic: bool,
    stdout: &str,
    stderr: &str,
) -> bool {
    !success
        && !diagnostic
        && (stdout.contains("market_phase_not_due") || stderr.contains("market_phase_not_due"))
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
#[ignore = "requires an explicit local state directory; release mode waits for real XNYS transitions"]
fn market_session_release_soak() {
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
        let state = read_state(&state_path).expect("read resume state");
        ensure_resume(&dir, &state, &config).expect("resume state binding");
        verify_completed_process_evidence(&dir, &state).expect("resume process evidence");
        state
    } else {
        let db = dir.join("runtime.sqlite");
        let _ = fs::remove_file(&db);
        let fault = Arc::new(AtomicU8::new(Fault::None as u8));
        let service = service(db.to_str().expect("utf8 db"), fault);
        let (activations, bindings) = activate_pair(&service);
        let stopped = service.handle_http("POST", "/scheduler/stop", json!({}));
        assert_eq!(stopped.status, 200, "{:#}", stopped.body);
        let started_at_ms = now_ms();
        let genesis = SoakGenesis {
            version: 1,
            config: config.clone(),
            database: db.display().to_string(),
            activation_id: activations[0]["activationId"]
                .as_str()
                .expect("activation id")
                .to_string(),
            correlation_id: activations[0]["run"]["correlationId"]
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
            activations: bindings.clone(),
        };
        let genesis_path = dir.join("genesis.json");
        create_new_json(&genesis_path, &genesis).expect("persist immutable genesis");
        let state = SoakState {
            version: 2,
            config: config.clone(),
            genesis_sha256: sha256_file(&genesis_path).expect("hash genesis"),
            database: db.display().to_string(),
            activation_id: activations[0]["activationId"]
                .as_str()
                .expect("activation id")
                .to_string(),
            correlation_id: activations[0]["run"]["correlationId"]
                .as_str()
                .expect("correlation id")
                .to_string(),
            started_at_ms,
            activations: bindings,
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
        if child_is_waiting_for_market_phase(status.success(), config.diagnostic, &stdout, &stderr)
        {
            std::thread::sleep(Duration::from_secs(config.interval_seconds));
            continue;
        }
        assert!(
            status.success(),
            "soak child failed: {status}; stderr={stderr}"
        );
        state = read_state(&state_path).expect("reload child state");
        ensure_resume(&dir, &state, &config).expect("child state binding");
        validate_child_transition(&before, &state, &phase).expect("child transition");
        let invocation = state
            .process_invocations
            .last_mut()
            .and_then(Value::as_object_mut)
            .expect("child process invocation");
        invocation.insert(
            "supervisorStartedAtMs".to_string(),
            json!(child_started_at_ms),
        );
        invocation.insert("supervisorObservedEndAtMs".to_string(), json!(now_ms()));
        invocation.insert("exitCode".to_string(), json!(status.code()));
        invocation.insert(
            "stdout".to_string(),
            json!(stdout_path.file_name().and_then(|name| name.to_str())),
        );
        invocation.insert(
            "stderr".to_string(),
            json!(stderr_path.file_name().and_then(|name| name.to_str())),
        );
        invocation.insert(
            "stdoutSha256".to_string(),
            json!(sha256_file(&stdout_path).expect("hash stdout")),
        );
        invocation.insert(
            "stderrSha256".to_string(),
            json!(sha256_file(&stderr_path).expect("hash stderr")),
        );
        atomic_json(&state_path, &state).expect("persist process invocation");
    }
    let counts = verify_final(&dir, &state).expect("final invariants");
    let elapsed = counts["verifiedProcessElapsedSeconds"]
        .as_u64()
        .expect("verified process elapsed seconds");
    let state_sha256 = sha256_file(&state_path).expect("hash state");
    let transitions =
        transition_observations(&dir, config.diagnostic).expect("transition observations");
    let receipt = json!({
        "schemaVersion":"tradeassembly.market_session_soak.v1",
        "status":"passed",
        "sourceRevision":config.source_revision,
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
        "instrumentFixtures":instrument_fixtures(),
        "plannedPhases":state.planned_phases,
        "completedPhases":state.completed_phases,
        "processInvocations":state.process_invocations,
        "activations":state.activations,
        "transitionObservations":transitions,
        "counts":counts,
        "artifacts":{
            "genesis":{"path":"genesis.json","sha256":state.genesis_sha256},
            "state":{"path":"state.json","sha256":state_sha256},
            "events":{"path":"events.jsonl","sha256":sha256_file(&dir.join("events.jsonl")).expect("hash events")},
            "database":{"path":"runtime.sqlite","sha256":sha256_file(Path::new(&state.database)).expect("hash database")},
        },
        "releaseQualified": qualifies(&config, elapsed, true)
    });
    atomic_json(&dir.join("receipt.json"), &receipt).expect("persist receipt");
}

fn qualifies(config: &SoakConfig, elapsed_seconds: u64, all_phases_complete: bool) -> bool {
    !config.diagnostic && elapsed_seconds > 0 && all_phases_complete
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
fn soak_qualification_is_never_time_duration_based() {
    let release = SoakConfig {
        duration_seconds: 86_400,
        interval_seconds: 1,
        diagnostic: false,
        source_revision: "r".to_string(),
        rust_toolchain: "t".to_string(),
    };
    assert!(!qualifies(&release, 0, true));
    assert!(qualifies(&release, 1, true));
    let diagnostic = SoakConfig {
        diagnostic: true,
        ..release
    };
    assert!(!qualifies(&diagnostic, 100_000, true));
}

#[test]
fn release_supervisor_accepts_market_wait_from_either_captured_stream_only() {
    assert!(child_is_waiting_for_market_phase(
        false,
        false,
        "soak child cycle: \"market_phase_not_due\"",
        "",
    ));
    assert!(child_is_waiting_for_market_phase(
        false,
        false,
        "",
        "soak child cycle: \"market_phase_not_due\"",
    ));
    assert!(!child_is_waiting_for_market_phase(
        true,
        false,
        "market_phase_not_due",
        "",
    ));
    assert!(!child_is_waiting_for_market_phase(
        false,
        true,
        "market_phase_not_due",
        "",
    ));
    assert!(!child_is_waiting_for_market_phase(
        false,
        false,
        "unrelated child failure",
        "",
    ));
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
    let genesis = SoakGenesis {
        version: 1,
        config: config.clone(),
        database: "db".to_string(),
        activation_id: "a".to_string(),
        correlation_id: "c".to_string(),
        started_at_ms: 1,
        run_nonce: "nonce".to_string(),
        activations: Vec::new(),
    };
    let genesis_path = temp.path().join("genesis.json");
    create_new_json(&genesis_path, &genesis).expect("genesis");
    let state = SoakState {
        version: 2,
        config: config.clone(),
        genesis_sha256: sha256_file(&genesis_path).expect("genesis digest"),
        database: "db".to_string(),
        activation_id: "a".to_string(),
        correlation_id: "c".to_string(),
        activations: Vec::new(),
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
fn soak_accepts_release_without_a_duration_threshold_and_rejects_malformed_event_log() {
    let config = SoakConfig {
        duration_seconds: 7,
        interval_seconds: 1,
        diagnostic: false,
        source_revision: "r".to_string(),
        rust_toolchain: "t".to_string(),
    };
    assert!(validate_config(&config).is_ok());
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
        activations: Vec::new(),
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
