//! Explicit, owned account-health reads. This module grants no trading authority.
//! Every provider effect crosses the existing health:refresh HTTP/C3 boundary.
use super::{typed, TradeAssemblyService};
use crate::broker_submission::account_identity::{self, CanonicalAccountIdentity};
use crate::ports::*;
use crate::service::{capability_graph, data_binding, data_plans, order_plans, session_dispatch};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const NS: &str = "automatic_account_health_v1";
const OUTCOMES: &str = "automatic_account_health_children_v1";
const LEASE_MS: i64 = 120_000;
const MAX_BYTES: usize = 1_048_576;
const MAX_COUNTER: u64 = tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER;
pub(crate) const REQUIRED_ACTION: &str = "plugin.instance.health.refresh";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub schema_version: String,
    pub refresh_interval_ms: i64,
}
impl Config {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != "tradeassembly.automatic_account_health_refresh.v1"
            || !(1000..=60_000).contains(&self.refresh_interval_ms)
        {
            return Err("automatic_health_config_invalid".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderPin {
    role: String,
    selected: CapabilitySelection,
    source: data_binding::PinnedOperation,
    identity: CanonicalAccountIdentity,
    account_binding_sha256: String,
    health_operation_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Pins {
    schema_version: String,
    core_revision: String,
    graph_record_sha256: String,
    providers: Vec<ProviderPin>,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(v)
        .map_err(|_| "automatic_health_encoding_invalid".into())
}
fn val<T: Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|_| "automatic_health_encoding_invalid".into())
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
        .ok_or_else(|| format!("automatic_health_{key}_required"))
}
fn core() -> Result<String, String> {
    Ok(crate::build_identity::current()
        .ok_or("automatic_health_core_identity_required")?
        .revision()
        .into())
}
fn owner(s: &TradeAssemblyService) -> Result<String, String> {
    let (issuer, subject) = s
        .responsible_human_identity()
        .ok_or("automatic_health_owner_required")?;
    let now = s.runtime().clock.trusted_now_ms()?;
    let claims = s.runtime().identity.resolve(subject, now)?;
    if claims.issuer != issuer || claims.expires_at_ms <= now {
        return Err("automatic_health_identity_stale".into());
    }
    hash(
        &s.invocation_owner()
            .ok_or("automatic_health_owner_required")?,
    )
}
fn selected(graph: &Value, node: &str) -> Result<CapabilitySelection, String> {
    let raw = graph["graph"]["nodes"]
        .as_array()
        .and_then(|nodes| nodes.iter().find(|v| v["nodeId"] == node))
        .ok_or("automatic_health_source_node_missing")?;
    serde_json::from_value(raw["selected"].clone())
        .map_err(|_| "automatic_health_source_unresolved".into())
}
fn operation(
    s: &TradeAssemblyService,
    source: &data_binding::PinnedOperation,
) -> Result<Value, String> {
    let package = s
        .runtime()
        .plugin_packages
        .get(&source.package_sha256)?
        .ok_or("automatic_health_package_missing")?;
    if package.plugin_ref != source.plugin_ref || package.manifest_sha256 != source.manifest_sha256
    {
        return Err("automatic_health_package_changed".into());
    }
    let ops = package.manifest["operations"]
        .as_array()
        .ok_or("automatic_health_operation_required")?;
    let matches = ops
        .iter()
        .filter(|op| op["id"] == "account.health")
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err("automatic_health_operation_required".into());
    }
    let raw = matches[0];
    let op: PluginOperationContract =
        serde_json::from_value(raw.clone()).map_err(|_| "automatic_health_operation_invalid")?;
    if op.effect != "read"
        || op.purpose != "account_health"
        || op.capability != "account.health"
        || !op.traits.modes.iter().any(|mode| mode == "paper")
        || op.traits.input_schema_refs.is_empty()
        || op.traits.output_schema_refs.is_empty()
        || op.traits.input_schema_refs.len() > 4
        || op.traits.output_schema_refs.len() > 4
    {
        return Err("automatic_health_operation_invalid".into());
    }
    require_health_schema_documents(&package, &op)?;
    Ok(raw.clone())
}
fn require_health_schema_documents(
    verified: &InstalledPluginPackage,
    op: &PluginOperationContract,
) -> Result<(), String> {
    // This package came from the normal verified get() above. V1 get() verifies
    // every responseSchemas document; V2/V3 get() verifies both document sets.
    // data_schema() indexes data metadata only and intentionally has no V1 view.
    // Decode the SDK envelope so references must bind actual digested documents,
    // including request schemas explicitly packaged under responseSchemas in V1.
    let bytes = serde_json::to_vec(&verified.descriptor)
        .map_err(|_| "automatic_health_descriptor_invalid")?;
    let envelope =
        tradeassembly_plugin_sdk::data_package::VersionedPackageDescriptor::decode(&bytes)?;
    for schema in op
        .traits
        .input_schema_refs
        .iter()
        .chain(&op.traits.output_schema_refs)
    {
        let response_document = envelope
            .base()
            .response_schemas
            .iter()
            .any(|doc| &doc.schema_ref == schema);
        let data_document = envelope.data().is_some_and(|data| {
            data.data_schemas
                .iter()
                .any(|doc| &doc.schema_ref == schema)
        });
        if schema.is_empty() || schema.len() > 256 || (!response_document && !data_document) {
            return Err(format!(
                "automatic_health_schema_document_required:{schema}"
            ));
        }
    }
    Ok(())
}
fn raw_record(
    s: &TradeAssemblyService,
    source: &data_binding::PinnedOperation,
) -> Result<Value, String> {
    s.require_object("plugin_instance", &source.plugin_instance_ref)
        .map_err(|_| "automatic_health_provider_not_available")?;
    let record = s
        .runtime()
        .plugins
        .get_instance(&source.plugin_instance_ref)?
        .ok_or("automatic_health_instance_missing")?;
    let status = s
        .runtime()
        .credentials
        .status(text(&record, "credentialRef")?);
    if !status.configured
        || record["credentialStatus"]["revisionRef"]
            != format!(
                "credential-generation:{}",
                crate::live_execution_checks::credential_revision(&record)
            )
    {
        return Err("automatic_health_genuine_bootstrap_required".into());
    }
    Ok(crate::live_execution_checks::apply_current_credential_status(record, &status))
}
fn same_source(
    expected: &data_binding::PinnedOperation,
    actual: &data_binding::PinnedOperation,
) -> Result<(), String> {
    if actual != expected {
        return Err("automatic_health_source_changed".into());
    }
    Ok(())
}
fn observed_identity(record: &Value, expected: &CanonicalAccountIdentity) -> Result<(), String> {
    // A failed normal health read clears account evidence. That is retryable READ
    // state, not a trading witness. A contradictory observation is never repaired.
    if record["accountMode"] != expected.environment
        || record["health"]["account"]["mode"]
            .as_str()
            .is_some_and(|mode| mode != expected.environment)
        || record["health"]["account"]["id"]
            .as_str()
            .is_some_and(|id| id != expected.provider_account_id)
    {
        return Err("automatic_health_observed_account_changed".into());
    }
    Ok(())
}
fn read_provider(s: &TradeAssemblyService, pin: &ProviderPin) -> Result<Value, String> {
    // Deliberately does NOT call canonical current/observer or graph currentness.
    // It permits only the same read after degraded/null or expired health evidence.
    same_source(&pin.source, &data_binding::pin(s, &pin.selected)?)?;
    if hash(&operation(s, &pin.source)?)? != pin.health_operation_sha256 {
        return Err("automatic_health_operation_changed".into());
    }
    let record = raw_record(s, &pin.source)?;
    observed_identity(&record, &pin.identity)?;
    let binding = hash(&json!({"identity":pin.identity.identity_sha256,
        "pluginInstanceRef":pin.selected.plugin_instance_ref,"pluginRef":pin.selected.plugin_ref,
        "packageSha256":pin.source.package_sha256,"manifestSha256":pin.source.manifest_sha256,
        "healthBinding":crate::live_execution_checks::live_health_evidence_binding(&record)}))?;
    if binding != pin.account_binding_sha256 {
        return Err("automatic_health_account_binding_changed".into());
    }
    Ok(record)
}
fn fresh_at_interval(
    s: &TradeAssemblyService,
    pin: &ProviderPin,
    now: i64,
    interval: i64,
) -> Result<i64, String> {
    let record = read_provider(s, pin)?;
    if account_identity::current(
        s.runtime().plugins.as_ref(),
        s.runtime().plugin_packages.as_ref(),
        s.runtime().credentials.as_ref(),
        &val(&pin.selected)?,
        now,
        "paper",
    )
    .is_ok_and(|w| w.identity == pin.identity && w.binding_sha256 == pin.account_binding_sha256)
    {
        let checked = record["health"]["checkedAtMs"]
            .as_i64()
            .ok_or("automatic_health_observation_invalid")?;
        return checked
            .checked_add(interval)
            .ok_or("automatic_health_clock_invalid".into());
    }
    Ok(now)
}
/// Called only by normal config build with a genuinely current graph and account.
/// Omitting the opt-in returns None and changes no existing config bytes/hash.
pub(super) fn bootstrap(
    s: &TradeAssemblyService,
    command: &typed::ConfigCommand,
    graph: &Value,
) -> Result<Option<Pins>, String> {
    let Some(config) = command
        .automatic_execution
        .as_ref()
        .and_then(|v| v.account_health_refresh.as_ref())
    else {
        return Ok(None);
    };
    config.validate()?;
    let policy = order_plans::published_policy(s, &command.scope)?;
    let facts = policy
        .source_nodes
        .get("account_data")
        .ok_or("automatic_health_facts_node_required")?;
    let now = s.runtime().clock.trusted_now_ms()?;
    let mut providers = Vec::new();
    for (role, node) in [
        ("broker", command.broker_requirement_ref.as_str()),
        ("account_data", facts.as_str()),
    ] {
        let chosen = selected(graph, node)?;
        let source = data_binding::pin(s, &chosen)?;
        let _ = raw_record(s, &source)?; // Persisted genuine bootstrap metadata required.
        let witness = account_identity::current(
            s.runtime().plugins.as_ref(),
            s.runtime().plugin_packages.as_ref(),
            s.runtime().credentials.as_ref(),
            &val(&chosen)?,
            now,
            "paper",
        )?;
        let health_operation_sha256 = hash(&operation(s, &source)?)?;
        providers.push(ProviderPin {
            role: role.into(),
            selected: chosen,
            source,
            identity: witness.identity,
            account_binding_sha256: witness.binding_sha256,
            health_operation_sha256,
        });
    }
    if providers[0].identity != providers[1].identity {
        return Err("automatic_health_provider_accounts_mismatch".into());
    }
    Ok(Some(Pins {
        schema_version: "tradeassembly.automatic_account_health_pins.v1".into(),
        core_revision: core()?,
        graph_record_sha256: hash(graph)?,
        providers,
    }))
}
struct Binding {
    activation: String,
    owner: String,
    core: String,
    config_sha256: String,
    config: Value,
    command: typed::ConfigCommand,
    refresh: Config,
    pins: Pins,
}
fn read_binding(s: &TradeAssemblyService, activation: &str) -> Result<Option<Binding>, String> {
    s.require_object("execution_activation", activation)
        .map_err(|_| "automatic_health_activation_not_available")?;
    let run = s
        .runtime()
        .storage
        .get_json("execution_runs", &format!("run_{activation}"))?
        .ok_or("automatic_health_run_missing")?;
    let a = s
        .runtime()
        .storage
        .get_json("execution_activations", activation)?
        .ok_or("automatic_health_activation_missing")?;
    let config = s
        .runtime()
        .storage
        .get_json("execution_configs", text(&run, "configId")?)?
        .ok_or("automatic_health_config_missing")?;
    // Legacy configs continue through their original path, without new health IO.
    let command: typed::ConfigCommand = serde_json::from_value(config["typedExecution"].clone())
        .map_err(|_| "automatic_health_config_invalid")?;
    let Some(refresh) = command
        .automatic_execution
        .as_ref()
        .and_then(|v| v.account_health_refresh.clone())
    else {
        return Ok(None);
    };
    command
        .automatic_execution
        .as_ref()
        .ok_or("automatic_health_config_invalid")?
        .validate()?;
    let who = owner(s)?;
    let revision = core()?;
    s.require_object("execution_config", text(&config, "configId")?)
        .map_err(|_| "automatic_health_config_not_available")?;
    s.require_object("strategy", &command.scope.strategy_id)
        .map_err(|_| "automatic_health_strategy_not_available")?;
    if !typed::is_typed(&config)
        || !typed::is_typed(&run)
        || !typed::is_typed(&a)
        || run["state"] != "active"
        || a["state"] != "active"
        || config["mode"] != "paper"
        || run["mode"] != "paper"
        || a["mode"] != "paper"
        || run["accountRef"] != config["accountRef"]
        || a["accountRef"] != config["accountRef"]
        || run["activationId"] != activation
        || a["activationId"] != activation
        || run["configId"] != a["configId"]
        || run["configId"] != config["configId"]
    {
        return Err("automatic_health_activation_stale".into());
    }
    for row in [&run, &a, &config] {
        if row["strategyId"] != command.scope.strategy_id
            || row["strategyVersionId"] != command.scope.strategy_version_id
            || row["strategySpecHash"] != command.scope.strategy_spec_hash
            || row["capabilityGraphRevisionId"] != command.capability_graph_revision
        {
            return Err("automatic_health_scope_changed".into());
        }
    }
    for ns in ["execution_controls", "execution_reconciliation"] {
        let control = s
            .runtime()
            .storage
            .get_json(ns, activation)?
            .ok_or("automatic_health_controls_missing")?;
        // New-entry pause/membership do not orphan an existing owned position.
        if control["activationId"] != activation
            || crate::broker_submission::management_controls_blocked(&control)
        {
            return Err("automatic_health_controls_blocked".into());
        }
    }
    let pins: Pins = serde_json::from_value(config["accountHealthProviderPins"].clone())
        .map_err(|_| "automatic_health_pins_missing")?;
    if pins.schema_version != "tradeassembly.automatic_account_health_pins.v1"
        || pins.core_revision != revision
        || pins.providers.len() != 2
        || pins.providers[0].role != "broker"
        || pins.providers[1].role != "account_data"
        || pins.providers[0].identity != pins.providers[1].identity
        || val(&pins.providers[0].identity)? != config["canonicalAccount"]
        || pins.providers[0].account_binding_sha256 != text(&config, "brokerAccountBindingSha256")?
    {
        return Err("automatic_health_pins_invalid".into());
    }
    let graph = s
        .runtime()
        .storage
        .get_json(
            capability_graph::REVISIONS_NS,
            &command.capability_graph_revision,
        )?
        .ok_or("automatic_health_graph_missing")?;
    if graph["immutable"] != true
        || graph["mode"] != "paper"
        || graph["graph"]["ok"] != true
        || graph["strategyId"] != command.scope.strategy_id
        || graph["strategyVersionId"] != command.scope.strategy_version_id
        || graph["strategySpecHash"] != command.scope.strategy_spec_hash
        || graph["revisionId"] != command.capability_graph_revision
        || graph["graphFingerprint"] != config["capabilityGraphFingerprint"]
        || hash(&graph)? != pins.graph_record_sha256
    {
        return Err("automatic_health_saved_graph_changed".into());
    }
    let policy = order_plans::published_policy(s, &command.scope)?;
    if hash(&policy)? != config["orderPlanPolicySha256"]
        || typed::config_id(
            s,
            &command,
            &config["canonicalAccount"],
            text(&config, "brokerAccountBindingSha256")?,
            &policy,
            Some(&pins),
        )? != text(&config, "configId")?
    {
        return Err("automatic_health_config_changed".into());
    }
    let managed = order_plans::management::policy(s, &command.scope)?;
    if managed
        .as_ref()
        .map(hash)
        .transpose()?
        .map(Value::String)
        .unwrap_or(Value::Null)
        != config["managedExitPolicySha256"]
    {
        return Err("automatic_health_policy_changed".into());
    }
    let facts = policy
        .source_nodes
        .get("account_data")
        .ok_or("automatic_health_facts_node_required")?;
    if selected(&graph, &command.broker_requirement_ref)? != pins.providers[0].selected
        || selected(&graph, facts)? != pins.providers[1].selected
    {
        return Err("automatic_health_saved_source_changed".into());
    }
    for pin in &pins.providers {
        read_provider(s, pin)?;
    }
    Ok(Some(Binding {
        activation: activation.into(),
        owner: who,
        core: revision,
        config_sha256: hash(&config)?,
        config,
        command,
        refresh,
        pins,
    }))
}
fn job_binding(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<Option<Binding>, String> {
    if job.paused {
        return Err("session_dispatch_job_paused".into());
    }
    let Some(activation) = job.request.execution_activation_id.as_deref() else {
        return Ok(None);
    };
    let Some(b) = read_binding(s, activation)? else {
        return Ok(None);
    };
    if job.owner_sha256 != b.owner
        || job.core_revision != b.core
        || job.request.mode != "paper"
        || job.request.scope != b.command.scope
        || job.request.capability_graph_revision != b.command.capability_graph_revision
    {
        return Err("automatic_health_job_binding_invalid".into());
    }
    if job.status.terminal()
        && !order_plans::management::automation::has_owned_work_for_health(s, job, &b.config)?
    {
        return Ok(None);
    }
    Ok(Some(b))
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Child {
    key: String,
    path: String,
    body: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Outcome {
    record_id: String,
    status: u16,
    classification: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Target {
    instance_ref: String,
    next_attempt_at_ms: i64,
    failures: u32,
    pending: Option<Child>,
    last_outcome: Option<Outcome>,
    error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    schema_version: String,
    activation_id: String,
    owner_sha256: String,
    core_revision: String,
    config_sha256: String,
    pins_sha256: String,
    generation: u64,
    sequence: u64,
    targets: Vec<Target>,
    cursor: usize,
    state_sha256: String,
}
fn state_id(b: &Binding) -> Result<String, String> {
    hash(&(&b.owner, &b.activation))
}
fn seal(state: &mut State) -> Result<(), String> {
    state.state_sha256.clear();
    state.state_sha256 = hash(state)?;
    Ok(())
}
fn targets(b: &Binding) -> Vec<Target> {
    let mut ids = b
        .pins
        .providers
        .iter()
        .map(|p| p.source.plugin_instance_ref.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|id| Target {
            instance_ref: id,
            next_attempt_at_ms: 0,
            failures: 0,
            pending: None,
            last_outcome: None,
            error: None,
        })
        .collect()
}
fn checked_increment(n: u64) -> Result<u64, String> {
    n.checked_add(1)
        .filter(|n| *n <= MAX_COUNTER)
        .ok_or("automatic_health_counter_exhausted".into())
}
fn child(b: &Binding, sequence: u64, instance: &str) -> Result<Child, String> {
    if instance.is_empty()
        || instance.len() > 256
        || !instance
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':'))
    {
        return Err("automatic_health_instance_path_invalid".into());
    }
    let key = format!("automatic-health:{}:{sequence}", state_id(b)?);
    Ok(Child {
        path: format!("/plugins/instances/{instance}/health:refresh"),
        body: json!({"accountMode":"paper","idempotencyKey":key}),
        key,
    })
}
fn load(s: &TradeAssemblyService, b: &Binding) -> Result<Option<(Value, State)>, String> {
    let Some(raw) = s.runtime().storage.get_json(NS, &state_id(b)?)? else {
        return Ok(None);
    };
    if serde_json::to_vec(&raw)
        .map_err(|_| "automatic_health_state_invalid")?
        .len()
        > MAX_BYTES
    {
        return Err("automatic_health_state_invalid".into());
    }
    let state: State =
        serde_json::from_value(raw.clone()).map_err(|_| "automatic_health_state_invalid")?;
    let mut signed = state.clone();
    seal(&mut signed)?;
    let expected = targets(b);
    if state.schema_version != "tradeassembly.automatic_account_health_state.v1"
        || state.activation_id != b.activation
        || state.owner_sha256 != b.owner
        || state.core_revision != b.core
        || state.config_sha256 != b.config_sha256
        || state.pins_sha256 != hash(&b.pins)?
        || state.state_sha256 != signed.state_sha256
        || state.generation > MAX_COUNTER
        || state.sequence > MAX_COUNTER
        || state.targets.len() != expected.len()
        || state.cursor >= expected.len()
        || state.targets.iter().zip(&expected).any(|(a, b)| {
            a.instance_ref != b.instance_ref || a.failures > 16 || a.next_attempt_at_ms < 0
        })
    {
        return Err("automatic_health_state_invalid".into());
    }
    for target in &state.targets {
        if let Some(retained) = &target.pending {
            let suffix = retained
                .key
                .rsplit(':')
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or("automatic_health_child_invalid")?;
            if suffix == 0
                || suffix > state.sequence
                || child(b, suffix, &target.instance_ref)? != *retained
            {
                return Err("automatic_health_child_invalid".into());
            }
        }
    }
    Ok(Some((raw, state)))
}
fn save(
    s: &TradeAssemblyService,
    b: &Binding,
    old: Option<Value>,
    state: &mut State,
    outcome: Option<(&Child, &super::ServiceResponse, &str)>,
) -> Result<(), String> {
    state.generation = checked_increment(state.generation)?;
    seal(state)?;
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!(
            "automatic-health-state:{}:{}",
            state_id(b)?,
            state.generation
        ))?,
    );
    let mut writes = vec![StorageWrite::new(
        NS,
        &state_id(b)?,
        val(state)?,
        context.clone(),
    )];
    let mut expectations = vec![StorageExpectation::new(NS, &state_id(b)?, old)];
    if let Some((child, response, record_id)) = outcome {
        let raw = json!({"schemaVersion":"tradeassembly.automatic_account_health_child.v1","activationId":b.activation,
            "ownerSha256":b.owner,"coreRevision":b.core,"configSha256":b.config_sha256,
            "child":child,"response":{"status":response.status,"body":response.body}});
        if serde_json::to_vec(&raw)
            .map_err(|_| "automatic_health_outcome_invalid")?
            .len()
            > MAX_BYTES
        {
            return Err("automatic_health_outcome_too_large".into());
        }
        writes.push(StorageWrite::new(OUTCOMES, record_id, raw, context));
        expectations.push(StorageExpectation::new(OUTCOMES, record_id, None));
    }
    if s.runtime().storage.put_json_batch(&writes, &expectations)? != ComparePutOutcome::Updated {
        return Err("automatic_health_state_conflict".into());
    }
    Ok(())
}
fn due(
    s: &TradeAssemblyService,
    b: &Binding,
    state: Option<&State>,
    now: i64,
) -> Result<i64, String> {
    let mut next = i64::MAX;
    for target in state
        .map(|v| v.targets.clone())
        .unwrap_or_else(|| targets(b))
    {
        let mut at = i64::MAX;
        for pin in b
            .pins
            .providers
            .iter()
            .filter(|p| p.source.plugin_instance_ref == target.instance_ref)
        {
            at = at.min(fresh_at(s, b, pin, now)?);
        }
        if target.pending.is_some() || target.error.is_some() || at <= now {
            at = target.next_attempt_at_ms.max(now);
        }
        next = next.min(at);
    }
    Ok(next)
}
#[derive(Clone, Debug)]
pub(crate) struct Gate {
    pub ready: bool,
    pub next_attempt_at_ms: i64,
    pub projection: Value,
}
fn projection(b: &Binding, state: Option<&State>, ready: bool, next: i64) -> Value {
    json!({"schemaVersion":"tradeassembly.automatic_account_health_status.v1","enabled":true,"status":if ready{"ready"}else if state.is_some_and(|st|st.targets.iter().any(|t|t.error.is_some())){"blocked"}else{"waiting"},
        "requiredAction":REQUIRED_ACTION,"nextAttemptAtMs":next,
        "pendingChildKeys":state.map(|s| s.targets.iter().filter_map(|t| t.pending.as_ref().map(|c| c.key.clone())).collect::<Vec<_>>()).unwrap_or_default(),
        "errors":state.map(|s|s.targets.iter().filter_map(|t|t.error.clone()).collect::<Vec<_>>()).unwrap_or_default(),
        "configId":b.config["configId"],"executionAuthorityGranted":false,"financialEffects":false})
}
fn no_gate(now: i64) -> Gate {
    Gate {
        ready: true,
        next_attempt_at_ms: now.saturating_add(60_000),
        projection: json!({"enabled":false,"executionAuthorityGranted":false}),
    }
}
pub(crate) fn needs_job_turn(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<bool, String> {
    let Some(b) = job_binding(s, job)? else {
        return Ok(false);
    };
    let state = load(s, &b)?.map(|(_, state)| state);
    let now = s.runtime().clock.trusted_now_ms()?;
    Ok(state.as_ref().is_some_and(|st| {
        st.targets
            .iter()
            .any(|t| t.pending.is_some() || t.error.is_some())
    }) || due(s, &b, state.as_ref(), now)? <= now)
}
pub(crate) fn status_job(s: &TradeAssemblyService, job: &session_dispatch::Job) -> Value {
    let result = (|| {
        let Some(b) = job_binding(s, job)? else {
            return Ok(no_gate(s.runtime().clock.now_ms()).projection);
        };
        let state = load(s, &b)?.map(|(_, state)| state);
        let now = s.runtime().clock.trusted_now_ms()?;
        let next = due(s, &b, state.as_ref(), now)?;
        let ready = ready(s, &b, state.as_ref(), now)?;
        Ok::<Value, String>(projection(&b, state.as_ref(), ready, next))
    })();
    result.unwrap_or_else(|error| json!({"status":"blocked","error":error,"requiredAction":REQUIRED_ACTION,"executionAuthorityGranted":false}))
}
fn backoff(failures: u32) -> i64 {
    1000i64
        .saturating_mul(1i64 << failures.saturating_sub(1).min(6))
        .min(60_000)
}
fn unknown(response: &super::ServiceResponse) -> bool {
    response.status >= 500
        || response.body["error"]["code"] == "idempotency_in_progress"
        || response.body["error"]["code"] == "idempotency_conflict"
}
fn sealed_failure(
    s: &TradeAssemblyService,
    child: &Child,
    response: &super::ServiceResponse,
) -> Result<bool, String> {
    if response.status < 500 {
        return Ok(false);
    }
    let expected = crate::control_plane::http_envelope("http", "POST", &child.path, &child.body)?;
    let Some(raw) = s
        .runtime()
        .storage
        .get_json(crate::control_plane::COMMAND_NS, &expected.command_id)?
    else {
        return Ok(false);
    };
    let recorded: crate::control_plane::ControlPlaneCommandRecord =
        serde_json::from_value(raw).map_err(|_| "automatic_health_control_record_invalid")?;
    if recorded.envelope.command_name != REQUIRED_ACTION
        || recorded.envelope.idempotency_key != expected.idempotency_key
        || recorded.envelope.target_object != expected.target_object
    {
        return Err("automatic_health_control_record_mismatch".into());
    }
    Ok(recorded.response_status == Some(response.status))
}
fn finish_target(
    target: &mut Target,
    response: &super::ServiceResponse,
    valid: bool,
    now: i64,
    record_id: String,
    sealed_failure: bool,
) {
    let uncertain = unknown(response) && !sealed_failure;
    let success = response.status == 200 && valid;
    target.last_outcome = Some(Outcome {
        record_id,
        status: response.status,
        classification: if uncertain {
            "unknown"
        } else if success {
            "fresh"
        } else {
            "failed"
        }
        .into(),
    });
    if success {
        target.pending = None;
        target.failures = 0;
        target.next_attempt_at_ms = now;
        target.error = None;
    } else {
        if !uncertain {
            target.pending = None;
        }
        target.failures = target.failures.saturating_add(1).min(16);
        target.next_attempt_at_ms = now.saturating_add(backoff(target.failures));
        target.error = Some(if uncertain {
            "automatic_health_response_unknown".into()
        } else if response.status == 403 {
            "automatic_health_refresh_authority_denied".into()
        } else {
            "automatic_health_refresh_not_current".into()
        });
    }
}
fn guarded_call<T>(
    check: impl FnOnce() -> Result<(), String>,
    call: impl FnOnce() -> T,
) -> Result<T, String> {
    check()?;
    Ok(call())
}
// Bound the optional scheduling hint. Oversized historical approval maps fall
// back to ordinary refresh; they never disable health or grant a deferral.
const MAX_PRIORITY_APPROVALS: usize = 128;
fn unresolved_financial_phase(
    phase: Option<&order_plans::submission::Phase>,
    finalized: bool,
    retained_cash: bool,
    filled: bool,
    managed: bool,
    live_approval: bool,
) -> bool {
    use order_plans::submission::Phase;
    if finalized {
        return false;
    }
    match phase {
        None => live_approval,
        Some(
            Phase::Admitting
            | Phase::Admitted
            | Phase::Sending
            | Phase::Unknown
            | Phase::Open
            | Phase::PartiallyFilled,
        ) => true,
        Some(Phase::Consumed) => retained_cash || filled,
        // A terminal close can still need authoritative final posting for an
        // actual retained hold/fill. Historical approvals alone do not count.
        Some(Phase::Rejected | Phase::Cancelled) => managed && (retained_cash || filled),
        Some(Phase::Denied) => false,
    }
}
// A C5 admission denial can precede Source.begin and therefore have no
// submission lifecycle row. Presence under the original fixed V2 key only
// removes this scheduling hint; even an unqualified record grants no deferral.
fn pre_admission_denial_present(s: &TradeAssemblyService, plan_id: &str) -> Result<bool, String> {
    Ok(s.runtime()
        .storage
        .get_json(
            "plugin_broker_submission_denials",
            &format!("broker-v2:{plan_id}"),
        )?
        .is_some())
}
/// Metadata-only scheduling witness, never an order permit or a new submit.
fn protected_financial_approval(
    s: &TradeAssemblyService,
    b: &Binding,
    now: i64,
) -> Result<bool, String> {
    let run = s
        .runtime()
        .storage
        .get_json("execution_runs", &format!("run_{}", b.activation))?
        .ok_or("automatic_health_run_missing")?;
    if run["activationId"] != b.activation
        || run["configId"] != b.config["configId"]
        || run["state"] != "active"
        || run["strategyId"] != b.command.scope.strategy_id
        || run["strategyVersionId"] != b.command.scope.strategy_version_id
        || run["strategySpecHash"] != b.command.scope.strategy_spec_hash
        || run["capabilityGraphRevisionId"] != b.command.capability_graph_revision
    {
        return Err("automatic_health_financial_binding_changed".into());
    }
    let Some(approvals) = run["orderPlanApprovals"].as_object() else {
        return Ok(false);
    };
    if approvals.len() > MAX_PRIORITY_APPROVALS {
        return Ok(false);
    }
    for (id, approval) in approvals {
        if approval["planId"] != *id {
            return Ok(false);
        }
        let source = order_plans::submission::source(s, id)?;
        let plan = source.historical()?;
        if plan.owner_sha256 != b.owner
            || plan.command.scope != b.command.scope
            || plan.command.mode != "paper"
            || plan.command.capability_graph_revision != b.command.capability_graph_revision
            || plan.canonical_account.as_ref() != Some(&b.pins.providers[0].identity)
            || plan.account_binding_sha256.as_ref()
                != Some(&b.pins.providers[1].account_binding_sha256)
            || approval["entryId"] != plan.entry_id
            || approval["signalReceiptSha256"] != plan.command.signal_receipt_sha256
            || approval["canonicalAccountSha256"] != b.pins.providers[0].identity.identity_sha256
        {
            return Err("automatic_health_financial_binding_changed".into());
        }
        if let Some(record) = source.state()? {
            if record.intent["originalRequest"]["activationId"] != b.activation {
                return Err("automatic_health_financial_binding_changed".into());
            }
            if unresolved_financial_phase(
                Some(&record.phase),
                record.settlement_receipt_sha256.is_some(),
                record.retained_cash.positive(),
                record.filled_quantity.positive(),
                plan.management.is_some(),
                false,
            ) {
                return Ok(true);
            }
        } else if plan.valid_until_ms > now && !pre_admission_denial_present(s, id)? {
            // Use the ordinary owned status reader for a release fence. A
            // retained approval cannot revive a released, unsubmitted plan.
            if !order_plans::transport::status(s, id)?["release"].is_null() {
                continue;
            }
            let Some(resource) = approval["leaseResource"].as_str() else {
                return Ok(false);
            };
            let lease = s.runtime().leases.current(resource)?;
            let live = lease.is_some_and(|lease| {
                lease.expires_at_ms > now
                    && approval["leaseOwner"] == lease.owner
                    && approval["leaseFence"] == lease.fencing_token
            });
            if live {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn protected_cycle_plan(
    s: &TradeAssemblyService,
    b: &Binding,
    c: &super::typed_cycle::Command,
) -> Result<bool, String> {
    if c.activation_id != b.activation || c.scope != b.command.scope {
        return Ok(false);
    }
    let Some(plan) = super::typed_cycle::prepared_plan_for_health(s, c, &b.config_sha256)? else {
        return Ok(false);
    };
    // Once approval exists, only the existing approved-plan/live-lease path may
    // grant this scheduling hint, including a crash before cycle phase sealing.
    let run = s
        .runtime()
        .storage
        .get_json("execution_runs", &format!("run_{}", b.activation))?
        .ok_or("automatic_health_run_missing")?;
    let Some(approvals) = run["orderPlanApprovals"].as_object() else {
        return Ok(false);
    };
    if run["activationId"] != b.activation
        || run["configId"] != b.config["configId"]
        || run["state"] != "active"
        || approvals.contains_key(&plan.plan_id)
    {
        return Ok(false);
    }
    Ok(plan.owner_sha256 == b.owner
        && plan.command.mode == "paper"
        && plan.command.capability_graph_revision == b.command.capability_graph_revision
        && plan.canonical_account.as_ref() == Some(&b.pins.providers[0].identity)
        && plan.account_binding_sha256.as_ref()
            == Some(&b.pins.providers[1].account_binding_sha256)
        && !pre_admission_denial_present(s, &plan.plan_id)?
        && plan.valid_until_ms > s.runtime().clock.trusted_now_ms()?)
}
fn protected_job_cycle(
    s: &TradeAssemblyService,
    b: &Binding,
    job: &session_dispatch::Job,
) -> Result<bool, String> {
    for command in session_dispatch::prepared_cycle_commands(s, job)? {
        if protected_cycle_plan(s, b, &command)? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn periodic_refresh_may_defer(
    state: &State,
    protected: impl FnOnce() -> Result<bool, String>,
    current: impl FnOnce() -> Result<bool, String>,
) -> Result<bool, String> {
    if state
        .targets
        .iter()
        .any(|target| target.pending.is_some() || target.error.is_some())
    {
        return Ok(false);
    }
    if !protected()? {
        return Ok(false);
    }
    current()
}
fn advance(
    s: &TradeAssemblyService,
    b: Binding,
    before_effect: impl Fn() -> Result<(), String>,
    prepared_cycle: impl Fn(&Binding) -> Result<bool, String>,
) -> Result<Gate, String> {
    let now = s.runtime().clock.trusted_now_ms()?;
    let resource = format!("automatic-health:{}", state_id(&b)?);
    let mut nonce = [0u8; 16];
    use rand::RngCore;
    rand::rngs::OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| "automatic_health_lease_nonce_unavailable")?;
    let lease_owner = format!("automatic-health-owner:{}", hash(&nonce)?);
    let Some(lease) = s
        .runtime()
        .leases
        .acquire(&resource, &lease_owner, now, LEASE_MS)?
    else {
        let next = now.saturating_add(1000);
        return Ok(Gate {
            ready: false,
            next_attempt_at_ms: next,
            projection: projection(&b, None, false, next),
        });
    };
    let result = (|| {
        let loaded = load(s, &b)?;
        let (mut old, mut state) = loaded.map(|(v, s)| (Some(v), s)).unwrap_or((
            None,
            State {
                schema_version: "tradeassembly.automatic_account_health_state.v1".into(),
                activation_id: b.activation.clone(),
                owner_sha256: b.owner.clone(),
                core_revision: b.core.clone(),
                config_sha256: b.config_sha256.clone(),
                pins_sha256: hash(&b.pins)?,
                generation: 0,
                sequence: 0,
                targets: targets(&b),
                cursor: 0,
                state_sha256: String::new(),
            },
        ));
        let mut selected_index = None;
        for offset in 0..state.targets.len() {
            let index = (state.cursor + offset) % state.targets.len();
            let target = &state.targets[index];
            let mut at = i64::MAX;
            for pin in b
                .pins
                .providers
                .iter()
                .filter(|p| p.source.plugin_instance_ref == target.instance_ref)
            {
                at = at.min(fresh_at(s, &b, pin, now)?);
            }
            if (target.pending.is_some() || target.error.is_some() || at <= now)
                && now >= target.next_attempt_at_ms
            {
                selected_index = Some(index);
                break;
            }
        }
        if let Some(index) = selected_index {
            // A not-yet-started periodic read must not spend the final freshness
            // budget of an existing approved plan or original lookup/settlement.
            // No pending/error health child is skipped, and exact normal account
            // currentness remains mandatory. The financial child still performs
            // its own ordinary config/source/account/C3/C5 checks afterward.
            if periodic_refresh_may_defer(
                &state,
                || {
                    Ok(
                        protected_financial_approval(s, &b, s.runtime().clock.trusted_now_ms()?)?
                            || prepared_cycle(&b)?,
                    )
                },
                || ready(s, &b, Some(&state), s.runtime().clock.trusted_now_ms()?),
            )? {
                let next = s.runtime().clock.trusted_now_ms()?;
                return Ok(Gate {
                    ready: true,
                    next_attempt_at_ms: next,
                    projection: projection(&b, Some(&state), true, next),
                });
            }
            if state.targets[index].pending.is_none() {
                state.sequence = checked_increment(state.sequence)?;
                state.targets[index].pending = Some(child(
                    &b,
                    state.sequence,
                    &state.targets[index].instance_ref,
                )?);
                save(s, &b, old.take(), &mut state, None)?;
                old = Some(val(&state)?);
            }
            let retained = state.targets[index]
                .pending
                .clone()
                .ok_or("automatic_health_child_missing")?;
            let response = guarded_call(
                || {
                    before_effect()?;
                    let fresh =
                        read_binding(s, &b.activation)?.ok_or("automatic_health_opt_in_removed")?;
                    if fresh.config_sha256 != b.config_sha256
                        || fresh.owner != b.owner
                        || fresh.core != b.core
                        || s.runtime().leases.current(&lease.resource)?.as_ref() != Some(&lease)
                        || lease.expires_at_ms <= s.runtime().clock.trusted_now_ms()?
                    {
                        return Err("automatic_health_pre_effect_binding_stale".into());
                    }
                    Ok(())
                },
                || s.handle_http("POST", &retained.path, retained.body.clone()),
            )?;
            // Preserve exact observed child response before interpreting it. The
            // normal handler may have committed health even when C3 sealing failed.
            if owner(s)? != b.owner
                || s.runtime().leases.current(&lease.resource)?.as_ref() != Some(&lease)
                || lease.expires_at_ms <= s.runtime().clock.trusted_now_ms()?
            {
                return Err("automatic_health_post_effect_lease_stale".into());
            }
            let after = s.runtime().clock.trusted_now_ms()?;
            let instance = &state.targets[index].instance_ref;
            let valid = b
                .pins
                .providers
                .iter()
                .filter(|p| &p.source.plugin_instance_ref == instance)
                .all(|pin| {
                    account_identity::current(
                        s.runtime().plugins.as_ref(),
                        s.runtime().plugin_packages.as_ref(),
                        s.runtime().credentials.as_ref(),
                        &serde_json::to_value(&pin.selected).unwrap_or(Value::Null),
                        after,
                        "paper",
                    )
                    .is_ok_and(|w| {
                        w.identity == pin.identity && w.binding_sha256 == pin.account_binding_sha256
                    })
                });
            let record_id = hash(&(
                &b.activation,
                &b.owner,
                state.generation,
                &retained.key,
                response.status,
                &response.body,
            ))?;
            finish_target(
                &mut state.targets[index],
                &response,
                valid,
                after,
                record_id.clone(),
                sealed_failure(s, &retained, &response)?,
            );
            state.cursor = (index + 1) % state.targets.len();
            save(
                s,
                &b,
                old.take(),
                &mut state,
                Some((&retained, &response, &record_id)),
            )?;
        }
        let now = s.runtime().clock.trusted_now_ms()?;
        let next = due(s, &b, Some(&state), now)?;
        // due() can be in backoff while health remains stale; neither a backoff
        // deadline nor an unknown response is permission to proceed.
        let ready = ready(s, &b, Some(&state), now)?;
        Ok(Gate {
            ready,
            next_attempt_at_ms: next,
            projection: projection(&b, Some(&state), ready, next),
        })
    })();
    let _ = s.runtime().leases.release(&lease);
    result
}
// Keep one freshness calculation used by lease-protected selection and readiness.
fn fresh_at(
    s: &TradeAssemblyService,
    b: &Binding,
    pin: &ProviderPin,
    now: i64,
) -> Result<i64, String> {
    fresh_at_interval(s, pin, now, b.refresh.refresh_interval_ms)
}

fn ready(
    s: &TradeAssemblyService,
    b: &Binding,
    state: Option<&State>,
    now: i64,
) -> Result<bool, String> {
    if state.is_some_and(|st| {
        st.targets
            .iter()
            .any(|t| t.pending.is_some() || t.error.is_some())
    }) {
        return Ok(false);
    }
    for pin in &b.pins.providers {
        if fresh_at_interval(
            s,
            pin,
            now,
            crate::live_execution_checks::LIVE_ACCOUNT_OBSERVATION_MAX_AGE_MS + 1,
        )? <= now
        {
            return Ok(false);
        }
    }
    Ok(true)
}
pub(crate) fn advance_job(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<Gate, String> {
    let Some(binding) = job_binding(s, job)? else {
        return Ok(no_gate(s.runtime().clock.now_ms()));
    };
    advance(
        s,
        binding,
        || session_dispatch::health_job_current(s, &job.job_id),
        |binding| protected_job_cycle(s, binding, job),
    )
}
pub(crate) fn advance_worker_job(
    s: &TradeAssemblyService,
    job: &session_dispatch::Job,
) -> Result<Gate, String> {
    let Some(binding) = job_binding(s, job)? else {
        return Ok(no_gate(s.runtime().clock.now_ms()));
    };
    advance(
        s,
        binding,
        || session_dispatch::health_worker_intent_current(s, &job.job_id),
        |binding| protected_job_cycle(s, binding, job),
    )
}
pub(crate) fn advance_activation(
    s: &TradeAssemblyService,
    activation: &str,
    scope: &data_plans::PlanScope,
    prepared_cycle: Option<&super::typed_cycle::Command>,
) -> Result<Gate, String> {
    let Some(binding) = read_binding(s, activation)? else {
        return Ok(no_gate(s.runtime().clock.now_ms()));
    };
    if binding.command.scope != *scope {
        return Err("automatic_health_scope_changed".into());
    }
    advance(
        s,
        binding,
        || Ok(()),
        |binding| {
            prepared_cycle.map_or(Ok(false), |command| {
                protected_cycle_plan(s, binding, command)
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> CanonicalAccountIdentity {
        CanonicalAccountIdentity {
            schema_version: "tradeassembly.canonical_account.v1".into(),
            broker_namespace: "synthetic".into(),
            environment: "paper".into(),
            provider_account_id: "owned-account".into(),
            identity_sha256: "a".repeat(64),
        }
    }
    fn chosen() -> CapabilitySelection {
        serde_json::from_value(json!({"pluginInstanceRef":"fixture","pluginRef":"fixture.plugin","providerRef":"synthetic",
            "operationId":"account.read","accountRef":"account://fixture/owned-account","manifestFingerprint":"manifest",
            "entitlementDecisionRef":null,"selectionReason":"explicit",
            "operation":{"id":"account.read","capability":"account.read","protocol":"fixture","resourceType":"account",
                "financeResourceType":"account","effect":"read","risk":"read","credentialGrantRequired":true,"accountBindingRequired":true,
                "apfActionId":"account.read","mandateRequired":false,"purpose":"account_data","evidence":[],"pepCoverageClass":"fixture",
                "checkPacks":[],"receiptClass":"fixture","redaction":"strict","noAdvice":true,"supportedProtocols":["fixture"]}})).unwrap()
    }
    fn source() -> data_binding::PinnedOperation {
        data_binding::PinnedOperation {
            plugin_instance_ref: "fixture".into(),
            plugin_ref: "fixture.plugin".into(),
            operation_id: "account.read".into(),
            package_sha256: "package".into(),
            manifest_sha256: "manifest".into(),
            context_sha256: "context".into(),
            account_ref: Some("account://fixture/owned-account".into()),
        }
    }
    fn binding() -> Binding {
        let pin = ProviderPin {
            role: "broker".into(),
            selected: chosen(),
            source: source(),
            identity: identity(),
            account_binding_sha256: "binding".into(),
            health_operation_sha256: "operation".into(),
        };
        let mut facts = pin.clone();
        facts.role = "account_data".into();
        Binding {
            activation: "activation-fixture".into(),
            owner: "owner-fixture".into(),
            core: "core-fixture".into(),
            config_sha256: "config-fixture".into(),
            config: json!({"configId":"config-fixture"}),
            command: typed::ConfigCommand {
                scope: data_plans::PlanScope {
                    strategy_id: "s".into(),
                    strategy_version_id: "v".into(),
                    strategy_spec_hash: "h".into(),
                },
                capability_graph_revision: "g".into(),
                broker_requirement_ref: "b".into(),
                automatic_execution: None,
            },
            refresh: Config {
                schema_version: "tradeassembly.automatic_account_health_refresh.v1".into(),
                refresh_interval_ms: 60_000,
            },
            pins: Pins {
                schema_version: "tradeassembly.automatic_account_health_pins.v1".into(),
                core_revision: "core-fixture".into(),
                graph_record_sha256: "graph".into(),
                providers: vec![pin, facts],
            },
        }
    }
    fn initial(b: &Binding) -> State {
        State {
            schema_version: "tradeassembly.automatic_account_health_state.v1".into(),
            activation_id: b.activation.clone(),
            owner_sha256: b.owner.clone(),
            core_revision: b.core.clone(),
            config_sha256: b.config_sha256.clone(),
            pins_sha256: hash(&b.pins).unwrap(),
            generation: 0,
            sequence: 1,
            targets: targets(b),
            cursor: 0,
            state_sha256: String::new(),
        }
    }
    fn service(db: &str) -> TradeAssemblyService {
        TradeAssemblyService::from_runtime(db, crate::adapters::local::test_runtime(db))
    }
    #[test]
    fn no_opt_in_preserves_original_config_id_and_facts_pins_bind_new_id() {
        let s = service(":memory:").for_authenticated_invocation("fixture", "owned", None, None);
        let mut b = binding();
        let policy = order_plans::Policy {
            schema_version: "fixture".into(),
            calculation_node: "calc".into(),
            semantics_version: "fixture".into(),
            source_nodes: std::collections::BTreeMap::new(),
            parameters: json!({}),
            position_action: tradeassembly_plugin_sdk::execution::PositionAction::BuyToOpen,
            time_in_force: "day".into(),
            fee_cash_cap: tradeassembly_plugin_sdk::execution::Decimal::ZERO,
            max_purchase_cash: tradeassembly_plugin_sdk::execution::Decimal::ZERO,
            max_premium_gross: tradeassembly_plugin_sdk::execution::Decimal::ZERO,
            max_quantity: tradeassembly_plugin_sdk::execution::Decimal::ZERO,
            maximum_ask_multiple: tradeassembly_plugin_sdk::execution::Decimal::ZERO,
            max_signal_age_ms: 1,
            max_snapshot_age_ms: 1,
            quote_profiles: vec![],
        };
        let account = val(&identity()).unwrap();
        let who = s.invocation_owner().unwrap();
        let old=format!("cfg_typed_{}",hash(&json!({"command":b.command,"owner":who,"account":account,"brokerBinding":"binding","policy":policy})).unwrap());
        assert_eq!(
            typed::config_id(&s, &b.command, &account, "binding", &policy, None).unwrap(),
            old
        );
        let first =
            typed::config_id(&s, &b.command, &account, "binding", &policy, Some(&b.pins)).unwrap();
        assert_ne!(first, old);
        b.pins.providers[1].source.context_sha256 = "changed-facts-config".into();
        assert_ne!(
            typed::config_id(&s, &b.command, &account, "binding", &policy, Some(&b.pins)).unwrap(),
            first
        );
    }
    #[test]
    fn activation_lease_excludes_an_independent_concurrent_read_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("service.db");
        let s = service(db.to_str().unwrap());
        let resource = "automatic-health:fixture-activation";
        let now = s.runtime().clock.trusted_now_ms().unwrap();
        let first = s
            .runtime()
            .leases
            .acquire(resource, "attempt-one", now, LEASE_MS)
            .unwrap()
            .unwrap();
        assert!(s
            .runtime()
            .leases
            .acquire(resource, "attempt-two", now + 1, LEASE_MS)
            .unwrap()
            .is_none());
        assert_eq!(
            s.runtime().leases.current(resource).unwrap(),
            Some(first.clone())
        );
        s.runtime().leases.release(&first).unwrap();
        let second = s
            .runtime()
            .leases
            .acquire(resource, "attempt-two", 1002, LEASE_MS)
            .unwrap()
            .unwrap();
        assert!(second.fencing_token > first.fencing_token);
    }
    #[test]
    fn closed_config_and_legacy_automatic_bytes_are_preserved() {
        let old = json!({"sourceInputs":{"account_data":{},"instrument_metadata":{},"pricing_quote":{}},"maxSubjectsPerOccurrence":1});
        let parsed: super::super::typed_cycle::AutomaticConfig =
            serde_json::from_value(old.clone()).unwrap();
        parsed.validate().unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), old);
        for interval in [0, 999, 60_001, i64::MAX] {
            assert!(Config {
                schema_version: "tradeassembly.automatic_account_health_refresh.v1".into(),
                refresh_interval_ms: interval
            }
            .validate()
            .is_err());
        }
        assert!(serde_json::from_value::<Config>(json!({"schemaVersion":"tradeassembly.automatic_account_health_refresh.v1","refreshIntervalMs":1000,"provider":"caller"})).is_err());
        assert!(Config {
            schema_version: "future".into(),
            refresh_interval_ms: 1000
        }
        .validate()
        .is_err());
    }
    #[test]
    fn failure_null_health_allows_only_read_identity_check_then_retry() {
        let mut record =
            json!({"accountMode":"paper","health":{"state":"degraded","account":null}});
        assert!(observed_identity(&record, &identity()).is_ok());
        let mut target = targets(&binding()).remove(0);
        target.pending = Some(child(&binding(), 1, "fixture").unwrap());
        finish_target(
            &mut target,
            &super::super::ServiceResponse::bad_request("plugin_health_failed"),
            false,
            1000,
            "outcome1".into(),
            false,
        );
        assert!(target.pending.is_none());
        assert_eq!(target.next_attempt_at_ms, 2000);
        record["health"] = json!({"state":"ready","account":{"id":"owned-account","mode":"paper"}});
        assert!(observed_identity(&record, &identity()).is_ok());
        target.pending = Some(child(&binding(), 2, "fixture").unwrap());
        finish_target(
            &mut target,
            &super::super::ServiceResponse::ok(json!({})),
            true,
            2000,
            "outcome2".into(),
            false,
        );
        assert!(target.error.is_none());
        assert_eq!(target.failures, 0);
        record["health"]["account"]["id"] = json!("another-account");
        assert!(observed_identity(&record, &identity()).is_err());
        record["health"]["account"] = json!({"id":"owned-account","mode":"live"});
        assert!(observed_identity(&record, &identity()).is_err());
    }
    #[test]
    fn facts_configuration_drift_refuses_before_http_invocation() {
        let expected = source();
        let mut changed = expected.clone();
        changed.context_sha256 = "changed-config".into();
        let called = std::cell::Cell::new(false);
        let result = guarded_call(|| same_source(&expected, &changed), || called.set(true));
        assert_eq!(result.unwrap_err(), "automatic_health_source_changed");
        assert!(!called.get());
        for field in ["package", "manifest", "instance", "account"] {
            let mut changed = expected.clone();
            match field {
                "package" => changed.package_sha256 = "other".into(),
                "manifest" => changed.manifest_sha256 = "other".into(),
                "instance" => changed.plugin_instance_ref = "other".into(),
                _ => changed.account_ref = Some("other".into()),
            }
            assert!(same_source(&expected, &changed).is_err());
        }
    }
    #[test]
    fn unknown_child_survives_actual_storage_recreation_with_exact_key() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("health.sqlite");
        let db = db.to_str().unwrap();
        let b = binding();
        let mut state = initial(&b);
        state.targets[0].pending = Some(child(&b, 1, "fixture").unwrap());
        let retained = state.targets[0].pending.clone().unwrap();
        {
            let s = service(db);
            save(&s, &b, None, &mut state, None).unwrap();
            let old = val(&state).unwrap();
            let response = super::super::ServiceResponse::internal_error("C3 outcome seal failed");
            finish_target(
                &mut state.targets[0],
                &response,
                false,
                1000,
                "unknown-outcome".into(),
                false,
            );
            save(
                &s,
                &b,
                Some(old),
                &mut state,
                Some((&retained, &response, "unknown-outcome")),
            )
            .unwrap();
        }
        let s = service(db);
        let (_, reloaded) = load(&s, &b).unwrap().unwrap();
        assert_eq!(reloaded.targets[0].pending.as_ref(), Some(&retained));
        assert_eq!(
            reloaded.targets[0].last_outcome.as_ref().unwrap().status,
            500
        );
        assert_eq!(
            reloaded.targets[0]
                .last_outcome
                .as_ref()
                .unwrap()
                .classification,
            "unknown"
        );
        let outcome = s
            .runtime()
            .storage
            .get_json(OUTCOMES, "unknown-outcome")
            .unwrap()
            .unwrap();
        assert_eq!(outcome["response"]["status"], 500);
        assert_eq!(outcome["child"]["key"], retained.key);
    }
    #[test]
    fn http_200_without_fresh_actual_account_is_a_failed_read() {
        let b = binding();
        let mut target = targets(&b).remove(0);
        target.pending = Some(child(&b, 1, "fixture").unwrap());
        finish_target(
            &mut target,
            &super::super::ServiceResponse::ok(json!({"instance":{"health":{"account":null}}})),
            false,
            1000,
            "bad-evidence".into(),
            false,
        );
        assert_eq!(
            target.last_outcome.as_ref().unwrap().classification,
            "failed"
        );
        assert_eq!(
            target.error.as_deref(),
            Some("automatic_health_refresh_not_current")
        );
        assert_eq!(target.next_attempt_at_ms, 2000);
    }
    #[test]
    fn targets_deduplicate_without_dropping_either_provider_pin() {
        let mut b = binding();
        assert_eq!(b.pins.providers.len(), 2);
        assert_eq!(targets(&b).len(), 1);
        b.pins.providers[1].source.plugin_instance_ref = "facts".into();
        assert_eq!(targets(&b).len(), 2);
        assert_eq!(backoff(16), 60_000);
        assert!(checked_increment(MAX_COUNTER).is_err());
    }
    #[test]
    fn ttl_endpoint_is_inclusive_and_future_health_is_rejected() {
        let mut r = json!({"instanceRef":"fixture","pluginRef":"fixture.plugin","accountRef":"account://fixture/owned-account","accountMode":"paper",
            "credentialStatus":{"configured":true},"health":{"checkedAtMs":1000,"evidenceCurrent":true,"account":{"id":"owned-account","mode":"paper",
                "status":"ACTIVE","tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false}}});
        let selected = serde_json::to_value(chosen()).unwrap();
        assert!(crate::live_execution_checks::account_observation_matches(
            &r, &selected, 121000, "paper"
        ));
        assert!(!crate::live_execution_checks::account_observation_matches(
            &r, &selected, 121001, "paper"
        ));
        r["health"]["checkedAtMs"] = json!(2000);
        assert!(!crate::live_execution_checks::account_observation_matches(
            &r, &selected, 1000, "paper"
        ));
    }
    fn packaged_health(
        directory: &std::path::Path,
        version: u8,
        include_request: bool,
        health_in_data_schemas: bool,
    ) -> (
        crate::adapters::plugin_packages::LocalPluginPackageStore,
        InstalledPluginPackage,
    ) {
        use sha2::{Digest, Sha256};
        let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        let request_ref = "schema://fixture/health-request@1";
        let response_ref = "schema://fixture/health-response@1";
        let request = json!({"type":"object","additionalProperties":false,"properties":{}});
        let response = json!({"type":"object","required":["account"],"properties":{"account":{"type":"object"}}});
        let mut health = chosen().operation;
        health.id = "account.health".into();
        health.capability = "account.health".into();
        health.purpose = "account_health".into();
        health.traits.modes = vec!["paper".into()];
        health.traits.input_schema_refs = vec![request_ref.into()];
        health.traits.output_schema_refs = vec![response_ref.into()];
        let mut manifest = json!({"metadata":{"id":"fixture.plugin","version":"1","provider":{"id":"synthetic"}},"operations":[health]});
        let mut schemas = vec![(response_ref, "health-response.json", response)];
        if include_request {
            schemas.push((request_ref, "health-request.json", request));
        }
        let mut extra_files = Vec::new();
        let mut data_contracts = Vec::new();
        let mut data_schemas = Vec::new();
        if version == 2 {
            // Use the same real synthetic V2 contract used by adapter package tests.
            let (_, _, contracts) = crate::data_pipeline::tests::fixture();
            let mut contract = contracts
                .into_iter()
                .find(|c| c.contract.operation_id == "provide")
                .unwrap()
                .contract;
            let series = json!({"type":"array","items":{"type":"number"}});
            let series_bytes = serde_json::to_vec(&series).unwrap();
            contract.outputs.get_mut("value").unwrap().schema.sha256 = hash(&series).unwrap();
            let contract_bytes = serde_json::to_vec(&contract).unwrap();
            data_contracts.push(json!({"operationId":"provide","document":{"path":"contract.json","sha256":digest(&contract_bytes)},"canonicalSha256":hash(&contract).unwrap()}));
            data_schemas.push(json!({"schemaRef":"schema://test/series@1","document":{"path":"series.json","sha256":digest(&series_bytes)},"canonicalSha256":hash(&series).unwrap()}));
            extra_files.push(("contract.json".to_string(), contract_bytes));
            extra_files.push(("series.json".to_string(), series_bytes));
            manifest["operations"].as_array_mut().unwrap().push(json!({"id":"provide","traits":{"inputSchemaRefs":[],"outputSchemaRefs":["schema://test/series@1"]}}));
        }
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let executable = b"not-executed-health-schema-fixture";
        let mut base = json!({"packageContractVersion":"1","plugin":{"id":"fixture.plugin","version":"1"},
            "manifest":{"path":"manifest.json","sha256":digest(&manifest_bytes)},
            "targets":[{"target":tradeassembly_plugin_sdk::host_target(),"binary":{"path":"bin/plugin","sha256":digest(executable)}}],
            "responseSchemas":[],"compatibility":{"hostContract":{"minimum":"1","maximum":"1"},"sdkVersion":tradeassembly_plugin_sdk::SDK_VERSION}});
        for (schema_ref, path, value) in schemas {
            let bytes = serde_json::to_vec(&value).unwrap();
            let document = json!({"path":path,"sha256":digest(&bytes)});
            if health_in_data_schemas {
                assert_eq!(version, 2);
                data_schemas.push(json!({"schemaRef":schema_ref,"document":document,"canonicalSha256":hash(&value).unwrap()}));
            } else {
                base["responseSchemas"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"schemaRef":schema_ref,"document":document}));
            }
            extra_files.push((path.to_string(), bytes));
        }
        let descriptor = if version == 1 {
            base
        } else {
            json!({"packageContractVersion":"2","package":base,"dataContracts":data_contracts,"dataSchemas":data_schemas})
        };
        let mut files = vec![
            (
                "tradeassembly-plugin.json".to_string(),
                serde_json::to_vec(&descriptor).unwrap(),
            ),
            ("manifest.json".to_string(), manifest_bytes),
            ("bin/plugin".to_string(), executable.to_vec()),
        ];
        files.extend(extra_files);
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(gzip);
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(&mut header, path, bytes.as_slice())
                .unwrap();
        }
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        let path = directory.join("health.tar.gz");
        std::fs::write(&path, &bytes).unwrap();
        let store =
            crate::adapters::plugin_packages::LocalPluginPackageStore::new(directory.join("store"))
                .unwrap();
        let package = store
            .install(&PluginPackageInstallRequest {
                source_type: "file".into(),
                locator: path.to_string_lossy().into(),
                package_sha256: digest(&bytes),
                manifest_sha256: hash(&manifest).unwrap(),
                offline: true,
            })
            .unwrap();
        (store, package)
    }
    fn health_service(
        store: crate::adapters::plugin_packages::LocalPluginPackageStore,
        package: &InstalledPluginPackage,
    ) -> (TradeAssemblyService, data_binding::PinnedOperation) {
        let mut runtime = crate::adapters::local::test_runtime(":memory:");
        runtime.plugin_packages = std::sync::Arc::new(store);
        let mut pin = source();
        pin.package_sha256 = package.package_sha256.clone();
        pin.manifest_sha256 = package.manifest_sha256.clone();
        pin.plugin_ref = package.plugin_ref.clone();
        (TradeAssemblyService::from_runtime(":memory:", runtime), pin)
    }
    #[test]
    fn actual_legacy_package_health_accepts_verified_request_and_response_documents() {
        let dir = tempfile::tempdir().unwrap();
        let (store, package) = packaged_health(dir.path(), 1, true, false);
        assert!(store
            .data_schema(&package.package_sha256, "schema://fixture/health-request@1")
            .unwrap()
            .is_none());
        let (s, pin) = health_service(store, &package);
        assert_eq!(operation(&s, &pin).unwrap()["id"], "account.health");
        std::fs::write(package.install_root.join("health-request.json"), b"{}").unwrap();
        assert!(operation(&s, &pin)
            .unwrap_err()
            .contains("schema_digest_mismatch"));
    }
    #[test]
    fn actual_legacy_package_health_refuses_an_unpackaged_request_schema() {
        let dir = tempfile::tempdir().unwrap();
        let (store, package) = packaged_health(dir.path(), 1, false, false);
        let (s, pin) = health_service(store, &package);
        assert_eq!(
            operation(&s, &pin).unwrap_err(),
            "automatic_health_schema_document_required:schema://fixture/health-request@1"
        );
    }
    #[test]
    fn actual_v2_health_accepts_response_only_or_data_schema_documents_and_rechecks_bytes() {
        for data_schemas in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (store, package) = packaged_health(dir.path(), 2, true, data_schemas);
            let (s, pin) = health_service(store, &package);
            assert_eq!(operation(&s, &pin).unwrap()["id"], "account.health");
            std::fs::write(package.install_root.join("health-response.json"), b"{}").unwrap();
            assert!(operation(&s, &pin).is_err());
        }
    }

    struct Authority {
        deny: bool,
    }
    impl crate::finance_authority::FinanceAuthorityPort for Authority {
        fn prepare_command(
            &self,
            _: &dyn StoragePort,
            e: &crate::control_plane::ControlPlaneCommandEnvelope,
        ) -> Result<(), String> {
            assert_eq!(e.command_name, REQUIRED_ACTION);
            if self.deny {
                Err("denied:fixture-explicit-health-authority".into())
            } else {
                Ok(())
            }
        }
        fn complete_command(
            &self,
            _: &dyn StoragePort,
            _: &crate::control_plane::ControlPlaneCommandEnvelope,
            _: u16,
        ) -> Result<(), String> {
            Ok(())
        }
        fn audit(&self, _: &dyn StoragePort) -> Value {
            json!({})
        }
    }
    #[test]
    fn normal_c3_health_authority_denial_stops_dispatch_before_provider() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("service.db");
        let s = service(db.to_str().unwrap());
        let body = json!({"accountMode":"paper","idempotencyKey":"health-denied"});
        let e = crate::control_plane::http_envelope(
            "http",
            "POST",
            "/plugins/instances/fixture/health:refresh",
            &body,
        )
        .unwrap();
        let called = std::cell::Cell::new(false);
        let result = guarded_call(
            || {
                crate::control_plane::prepare_command(
                    s.runtime().storage.as_ref(),
                    &Authority { deny: true },
                    e.clone(),
                )
                .map(|_| ())
            },
            || called.set(true),
        );
        assert!(result.unwrap_err().starts_with("finance_authority:denied:"));
        assert!(!called.get());
        let stored = s
            .runtime()
            .storage
            .get_json(crate::control_plane::COMMAND_NS, &e.command_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored["response_status"], 403);
    }
    #[test]
    fn sealed_c3_500_is_a_completed_failed_read_and_allows_a_new_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("service.db");
        let s = service(db.to_str().unwrap());
        let b = binding();
        let retained = child(&b, 1, "fixture").unwrap();
        let e = crate::control_plane::http_envelope("http", "POST", &retained.path, &retained.body)
            .unwrap();
        let authority = Authority { deny: false };
        crate::control_plane::prepare_command(s.runtime().storage.as_ref(), &authority, e.clone())
            .unwrap();
        let response = super::super::ServiceResponse::internal_error("health persistence failed");
        assert!(!sealed_failure(&s, &retained, &response).unwrap());
        crate::control_plane::complete_command_with_body(
            s.runtime().storage.as_ref(),
            &authority,
            &e,
            response.status,
            Some(response.body.clone()),
        )
        .unwrap();
        assert!(sealed_failure(&s, &retained, &response).unwrap());
        let mut target = targets(&b).remove(0);
        target.pending = Some(retained.clone());
        finish_target(
            &mut target,
            &response,
            false,
            1000,
            "completed-500".into(),
            true,
        );
        assert!(target.pending.is_none());
        assert_eq!(target.last_outcome.unwrap().classification, "failed");
        assert_ne!(retained.key, child(&b, 2, "fixture").unwrap().key);
    }
    #[test]
    fn normal_c3_unknown_retries_original_key_then_recovers_exact_completed_body() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("service.db");
        let s = service(db.to_str().unwrap());
        let body = json!({"accountMode":"paper","idempotencyKey":"health-retained"});
        let e = crate::control_plane::http_envelope(
            "http",
            "POST",
            "/plugins/instances/fixture/health:refresh",
            &body,
        )
        .unwrap();
        let authority = Authority { deny: false };
        let first = crate::control_plane::prepare_command(
            s.runtime().storage.as_ref(),
            &authority,
            e.clone(),
        )
        .unwrap();
        assert!(!first.duplicate);
        // Simulated lost response before completion, no invented HTTP outcome.
        let retried = crate::control_plane::prepare_command(
            s.runtime().storage.as_ref(),
            &authority,
            e.clone(),
        )
        .unwrap();
        assert!(!retried.duplicate);
        assert_eq!(
            first.envelope.idempotency_key,
            retried.envelope.idempotency_key
        );
        let actual = json!({"actual":"sealed-observation"});
        crate::control_plane::complete_command_with_body(
            s.runtime().storage.as_ref(),
            &authority,
            &e,
            200,
            Some(actual.clone()),
        )
        .unwrap();
        let recovered =
            crate::control_plane::prepare_command(s.runtime().storage.as_ref(), &authority, e)
                .unwrap();
        assert!(recovered.duplicate);
        assert_eq!(recovered.response_status, Some(200));
        assert_eq!(recovered.response_body, Some(actual));
    }
    fn priority_current_fixture(
        directory: &std::path::Path,
    ) -> (TradeAssemblyService, Binding, Vec<Value>, i64) {
        let directory = directory.canonicalize().unwrap();
        let (store, package) = packaged_health(&directory, 1, true, false);
        let db = directory.join("priority.sqlite");
        let mut runtime = crate::adapters::local::test_runtime(db.to_str().unwrap());
        runtime.plugin_packages = std::sync::Arc::new(store);
        let identity = crate::local_owner_identity::LocalOwnerIdentity::for_database(&db).unwrap();
        runtime.identity = std::sync::Arc::new(
            crate::adapters::local::system::LocalOwnerIdentityPort::new(db.to_str().unwrap())
                .unwrap(),
        );
        let s = TradeAssemblyService::from_runtime(db.to_str().unwrap(), runtime)
            .for_authenticated_invocation(
                &identity.issuer,
                &identity.stable_identity_id,
                None,
                None,
            );
        let ctx = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("priority-fixture").unwrap(),
        );
        s.runtime().plugins.install_manifest(&package.plugin_ref,json!({"pluginRef":package.plugin_ref,"manifest":package.manifest,
            "manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256}}),&ctx).unwrap();
        s.runtime()
            .credentials
            .store(
                "priority-fixture-only",
                &std::collections::BTreeMap::from([
                    ("api_key".into(), "synthetic-unit-key".into()),
                    ("api_secret".into(), "synthetic-unit-secret".into()),
                ]),
            )
            .unwrap();
        let now = s.runtime().clock.trusted_now_ms().unwrap();
        let mut b = binding();
        b.owner = owner(&s).unwrap();
        b.pins.providers.clear();
        let mut records = Vec::new();
        for (instance, role) in [("fixture", "broker"), ("facts", "account_data")] {
            let mut record = json!({"instanceRef":instance,"pluginRef":package.plugin_ref,"providerRef":"synthetic",
                "enabled":true,"activePackageSha256":package.package_sha256,"configuration":{},
                "credentialRef":"priority-fixture-only","credentialRevision":1,"credentialStatus":{"configured":true,"revisionRef":"credential-generation:1"},
                "accountRef":format!("account://{instance}/owned-account"),"accountMode":"paper",
                "health":{"state":"ready","checkedAtMs":now-60001,"connectivityChecked":true,"evidenceCurrent":true,
                    "account":{"id":"owned-account","mode":"paper","status":"ACTIVE","tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false}}});
            record["health"]["binding"] =
                crate::live_execution_checks::live_health_evidence_binding(&record);
            s.runtime()
                .plugins
                .put_instance(instance, record.clone(), &ctx)
                .unwrap();
            s.bind_owned_object("plugin_instance", instance).unwrap();
            records.push(record);
            let mut selected = chosen();
            selected.plugin_instance_ref = instance.into();
            selected.plugin_ref = package.plugin_ref.clone();
            selected.account_ref = Some(format!("account://{instance}/owned-account"));
            selected.manifest_fingerprint = package.manifest_sha256.clone();
            selected.operation_id = "account.health".into();
            selected.operation =
                serde_json::from_value(package.manifest["operations"][0].clone()).unwrap();
            let source = data_binding::pin(&s, &selected).unwrap();
            let witness = account_identity::current(
                s.runtime().plugins.as_ref(),
                s.runtime().plugin_packages.as_ref(),
                s.runtime().credentials.as_ref(),
                &val(&selected).unwrap(),
                now,
                "paper",
            )
            .unwrap();
            b.pins.providers.push(ProviderPin {
                role: role.into(),
                selected,
                source: source.clone(),
                identity: witness.identity,
                account_binding_sha256: witness.binding_sha256,
                health_operation_sha256: hash(&operation(&s, &source).unwrap()).unwrap(),
            });
        }
        (s, b, records, now)
    }
    #[test]
    fn financial_priority_defers_only_periodic_reads_with_two_exact_current_pins() {
        let directory = tempfile::tempdir().unwrap();
        let (s, b, records, now) = priority_current_fixture(directory.path());
        let state = initial(&b);
        assert_eq!(b.pins.providers.len(), 2);
        assert_eq!(state.targets.len(), 2);
        assert!(
            due(&s, &b, Some(&state), now).unwrap() <= now,
            "60s periodic refresh is due"
        );
        assert!(
            ready(&s, &b, Some(&state), now).unwrap(),
            "actual120s account evidence is still current"
        );
        assert!(periodic_refresh_may_defer(
            &state,
            || Ok(true),
            || ready(&s, &b, Some(&state), now)
        )
        .unwrap());
        assert!(!periodic_refresh_may_defer(
            &state,
            || Ok(false),
            || panic!("no financial work cannot defer")
        )
        .unwrap());
        let endpoint = records[0]["health"]["checkedAtMs"].as_i64().unwrap()
            + crate::live_execution_checks::LIVE_ACCOUNT_OBSERVATION_MAX_AGE_MS;
        assert!(periodic_refresh_may_defer(
            &state,
            || Ok(true),
            || ready(&s, &b, Some(&state), endpoint)
        )
        .unwrap());
        assert!(
            !periodic_refresh_may_defer(
                &state,
                || Ok(true),
                || ready(&s, &b, Some(&state), endpoint + 1)
            )
            .unwrap(),
            "actual TTL expiry cannot be deferred"
        );
        let ctx = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("priority-stale-facts").unwrap(),
        );
        let mut facts = records[1].clone();
        facts["health"]["checkedAtMs"] = json!(now + 1);
        s.runtime()
            .plugins
            .put_instance("facts", facts, &ctx)
            .unwrap();
        assert!(
            !periodic_refresh_may_defer(&state, || Ok(true), || ready(&s, &b, Some(&state), now))
                .unwrap(),
            "future evidence for one exact pin is not current"
        );
        let mut facts = records[1].clone();
        facts["health"] = json!({"state":"degraded","account":null});
        s.runtime()
            .plugins
            .put_instance("facts", facts, &ctx)
            .unwrap();
        assert!(
            !periodic_refresh_may_defer(&state, || Ok(true), || ready(&s, &b, Some(&state), now))
                .unwrap(),
            "a failed read with null account must recover through normal health rather than defer"
        );
    }
    #[test]
    fn financial_priority_never_skips_pending_unknown_or_failed_health_child() {
        let b = binding();
        let mut state = initial(&b);
        let retained = child(&b, 1, "fixture").unwrap();
        state.targets[0].pending = Some(retained.clone());
        let before = state.clone();
        assert!(!periodic_refresh_may_defer(
            &state,
            || panic!("pending original health must drain"),
            || panic!("pending health cannot become ready")
        )
        .unwrap());
        assert_eq!(state, before);
        assert_eq!(state.targets[0].pending.as_ref(), Some(&retained));
        state.targets[0].pending = None;
        state.targets[0].error = Some("automatic_health_refresh_not_current".into());
        assert!(!periodic_refresh_may_defer(
            &state,
            || panic!("failed health must recover"),
            || panic!("failed health cannot defer")
        )
        .unwrap());
    }
    #[test]
    fn financial_priority_refuses_changed_credentials_account_and_package_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let (s, b, records, now) = priority_current_fixture(directory.path());
        let state = initial(&b);
        let ctx = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("priority-source-drift").unwrap(),
        );
        for change in ["credentials", "account", "package", "configuration"] {
            let mut facts = records[1].clone();
            match change {
                "credentials" => facts["credentialRevision"] = json!(2),
                "account" => facts["health"]["account"]["id"] = json!("other-observed-account"),
                "package" => facts["activePackageSha256"] = json!("e".repeat(64)),
                _ => facts["configuration"] = json!({"changed":true}),
            }
            s.runtime()
                .plugins
                .put_instance("facts", facts, &ctx)
                .unwrap();
            assert!(
                periodic_refresh_may_defer(
                    &state,
                    || Ok(true),
                    || ready(&s, &b, Some(&state), now)
                )
                .is_err(),
                "{change} must not grant deferral"
            );
            s.runtime()
                .plugins
                .put_instance("facts", records[1].clone(), &ctx)
                .unwrap();
        }
        s.runtime()
            .credentials
            .revoke("priority-fixture-only")
            .unwrap();
        assert!(
            periodic_refresh_may_defer(&state, || Ok(true), || ready(&s, &b, Some(&state), now))
                .is_err(),
            "revoked actual credential posture cannot defer"
        );
    }
    #[test]
    fn pre_source_terminal_denial_never_qualifies_historical_approval_for_deferral() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("terminal-denial.sqlite");
        let s = service(db.to_str().unwrap());
        let id = "orderplan_original";
        assert!(!pre_admission_denial_present(&s, id).unwrap());
        let ctx = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("denied-original").unwrap(),
        );
        s.runtime().storage.put_json_if_absent("plugin_broker_submission_denials", &format!("broker-v2:{id}"),
            json!({"state":"denied","requestHash":"original","correlationId":"original","reason":"admission_denied"}), &ctx).unwrap();
        assert!(pre_admission_denial_present(&s, id).unwrap());
        let recreated = service(db.to_str().unwrap());
        assert!(
            pre_admission_denial_present(&recreated, id).unwrap(),
            "original denial survives service recreation"
        );
        assert!(!pre_admission_denial_present(&recreated, "orderplan_other").unwrap());
        // Malformed/unqualified metadata also cannot grant a scheduling hint.
        recreated
            .runtime()
            .storage
            .put_json_if_absent(
                "plugin_broker_submission_denials",
                "broker-v2:orderplan_unqualified",
                json!({}),
                &ctx,
            )
            .unwrap();
        assert!(pre_admission_denial_present(&recreated, "orderplan_unqualified").unwrap());
    }
    #[test]
    fn only_unresolved_financial_lifecycle_or_live_approval_retains_priority() {
        use order_plans::submission::Phase;
        assert!(unresolved_financial_phase(
            None, false, false, false, false, true
        ));
        assert!(!unresolved_financial_phase(
            None, false, false, false, false, false
        ));
        for phase in [
            Phase::Admitting,
            Phase::Admitted,
            Phase::Sending,
            Phase::Unknown,
            Phase::Open,
            Phase::PartiallyFilled,
        ] {
            assert!(
                unresolved_financial_phase(Some(&phase), false, false, false, false, false),
                "original lifecycle must drain even after plan expiry"
            );
            assert!(
                !unresolved_financial_phase(Some(&phase), true, true, true, true, true),
                "final posting resumes periodic reads"
            );
        }
        for phase in [Phase::Denied, Phase::Rejected, Phase::Cancelled] {
            assert!(
                !unresolved_financial_phase(Some(&phase), false, false, false, false, true),
                "historical terminal approval is not pending work"
            );
        }
        assert!(unresolved_financial_phase(
            Some(&Phase::Consumed),
            false,
            false,
            true,
            false,
            false
        ));
        assert!(!unresolved_financial_phase(
            Some(&Phase::Consumed),
            true,
            true,
            true,
            true,
            true
        ));
        assert!(
            unresolved_financial_phase(Some(&Phase::Cancelled), false, true, true, true, false),
            "unposted actual close hold/fill may settle"
        );
        assert!(!unresolved_financial_phase(
            Some(&Phase::Cancelled),
            true,
            true,
            true,
            true,
            false
        ));
        assert!(!unresolved_financial_phase(
            Some(&Phase::Denied),
            false,
            true,
            true,
            true,
            true
        ));
    }
}
