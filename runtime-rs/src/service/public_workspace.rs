//! Versioned, bounded portability of public research state. No execution authority.
//! Entries are JSON records, never filesystem members or database images.
use super::{backtest_lifecycle, execution, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::backtest_contracts::{canonical_hash, BacktestConfiguration};
use crate::backtest_report::BacktestReport;
use crate::ports::{
    AuthorityContext, IdempotencyKey, ImmutablePutOutcome, ObjectScope, SideEffectContext,
    StorageExpectation, StorageWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
#[cfg(unix)]
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::path::Component;
use std::path::Path;

pub const WORKSPACE_SCHEMA: &str = "tradeassembly.public-workspace.v1";
pub const RESULT_SUMMARY_SCHEMA: &str = "tradeassembly.strategy-result-summary.v1";
pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const INPUT_NAMESPACES: &[&str] = &[
    "backtests",
    "strategies",
    "strategy_versions",
    "strategy_drafts",
    "research_notebooks",
    "research_universes",
    "research_datasets",
    "dataset_snapshots_v1",
    "backtest_manifests_v1",
    "backtest_runs_v2",
    "public_dataset_references_v1",
    "public_backtest_configurations_v1",
    "public_backtest_reports_v1",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicWorkspaceEntry {
    pub namespace: String,
    pub key: String,
    pub content_hash: String,
    pub value: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicWorkspaceBundle {
    pub schema: String,
    pub entries: Vec<PublicWorkspaceEntry>,
    pub settings: String,
    pub legacy_backtests_excluded: usize,
    pub bundle_hash: String,
}

fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    canonical_hash(value, "workspace_hash_failed")
}
fn code(error: &str) -> String {
    error.to_string()
}
fn select(value: &Value, fields: &[&str]) -> Value {
    Value::Object(
        fields
            .iter()
            .filter_map(|key| value.get(*key).map(|v| ((*key).to_string(), v.clone())))
            .collect(),
    )
}
fn fields(namespace: &str) -> Option<&'static [&'static str]> {
    match namespace {
        "strategies" => Some(&[
            "id",
            "name",
            "symbol",
            "asset_class",
            "assetClass",
            "status",
            "version",
            "draft_revision",
            "draftRevision",
            "current_version_id",
            "currentVersionId",
            "provider_ref",
            "providerRef",
            "spec",
            "latestSpec",
            "noAdvice",
        ]),
        "strategy_versions" => Some(&[
            "id",
            "strategy_id",
            "strategyId",
            "version",
            "spec",
            "specHash",
            "identityAlgorithm",
            "sourceRepresentation",
            "normalizedSourceSpecBytes",
            "normalizedSourceSpecHash",
            "canonicalSpecBytes",
            "canonicalSpecHash",
            "immutable",
            "publishedAt",
        ]),
        "strategy_drafts" => Some(&[
            "id",
            "strategyId",
            "strategy_id",
            "name",
            "status",
            "revision",
            "baseVersionId",
            "draftHash",
            "specHash",
            "identityAlgorithm",
            "sourceRepresentation",
            "normalizedSourceSpecHash",
            "canonicalSpecHash",
            "spec",
            "updatedAt",
        ]),
        "research_universes" => Some(&[
            "id",
            "strategyId",
            "strategy_id",
            "name",
            "symbols",
            "filters",
            "provider_ref",
            "metrics",
            "createdAt",
            "noAdvice",
        ]),
        "public_dataset_references_v1" => Some(&[
            "datasetId",
            "strategyId",
            "snapshotRef",
            "contentHash",
            "rowCount",
            "timeSlice",
            "source",
            "instruments",
            "granularity",
            "timezone",
            "calendar",
            "normalizationPolicy",
            "qualityPolicy",
            "sourcePluginRef",
            "configuration",
            "rawDataIncluded",
        ]),
        "public_backtest_configurations_v1" => Some(&["runId", "configuration"]),
        "public_backtest_reports_v1" => Some(&[
            "schema",
            "reportHash",
            "run",
            "assumptions",
            "overview",
            "trades",
            "journal",
            "orders",
            "fills",
            "positions",
            "ledger",
            "costsAndRisk",
            "diagnostics",
            "metadata",
            "provenance",
            "rawDataIncluded",
        ]),
        "research_notebooks" => Some(&[
            "schemaVersion",
            "kind",
            "ok",
            "strategyId",
            "runId",
            "notebookId",
            "notebookHash",
            "summary",
            "notebook",
            "warnings",
            "artifactRefs",
            "exportRefs",
            "replayRefs",
            "noAdvice",
            "recordHash",
        ]),
        _ => None,
    }
}
fn entry(namespace: &str, key: String, value: Value) -> Result<PublicWorkspaceEntry, String> {
    Ok(PublicWorkspaceEntry {
        namespace: namespace.to_string(),
        key,
        content_hash: hash(&value)?,
        value,
    })
}
fn local_process(service: &TradeAssemblyService) -> Result<(), String> {
    local_sqlite(service)?;
    if service
        .invocation_owner()
        .is_some_and(|owner| owner.issuer != "local-owner")
    {
        return Err(code("workspace_local_sqlite_owner_required"));
    }
    Ok(())
}

fn local_sqlite(service: &TradeAssemblyService) -> Result<(), String> {
    if service.runtime().storage.adapter_name() != "sqlite-wal"
        || !service
            .runtime_manifest()
            .is_some_and(|manifest| manifest.profile == "local")
    {
        return Err(code("workspace_local_sqlite_owner_required"));
    }
    Ok(())
}

/// A local-process capability minted only from a verified CLI session. Hosted
/// identity remains the object owner; this never links it to a local owner.
pub(crate) struct VerifiedLocalWorkspace {
    service: TradeAssemblyService,
}

impl VerifiedLocalWorkspace {
    pub(crate) fn from_verified_session(
        service: &TradeAssemblyService,
        session: crate::cli_identity::VerifiedCliSession,
    ) -> Result<Self, String> {
        local_sqlite(service)?;
        let service = session.bind_service(service)?;
        if service.invocation_owner().is_none() {
            return Err(code("workspace_local_sqlite_owner_required"));
        }
        Ok(Self { service })
    }

    pub(crate) fn export(&self) -> Result<PublicWorkspaceBundle, String> {
        export_owned(&self.service)
    }

    pub(crate) fn import(&self, raw: &[u8]) -> Result<Value, String> {
        import_owned(&self.service, raw)
    }

    pub(crate) fn export_file(&self, path: &Path) -> Result<Value, String> {
        write_bundle_file(self.export()?, path)
    }
}
fn strategy_id(value: &Value) -> Option<&str> {
    value["strategyId"]
        .as_str()
        .or(value["strategy_id"].as_str())
}

pub fn export(service: &TradeAssemblyService) -> Result<PublicWorkspaceBundle, String> {
    local_process(service)?;
    export_owned(service)
}

fn export_owned(service: &TradeAssemblyService) -> Result<PublicWorkspaceBundle, String> {
    let rows = service.runtime().storage.public_workspace_snapshot(
        INPUT_NAMESPACES,
        MAX_ENTRIES,
        MAX_BUNDLE_BYTES,
    )?;
    let mut entries = Vec::new();
    let legacy_backtests_excluded = rows.iter().filter(|(ns, _, _)| ns == "backtests").count();
    let strategy_ids = rows
        .iter()
        .filter(|(ns, _, _)| ns == "strategies")
        .filter(|(_, key, _)| service.object_is_visible("strategy", key))
        .map(|(_, key, _)| key.clone())
        .collect::<BTreeSet<_>>();
    for (ns, key, value) in rows {
        if ns == "backtests" {
            continue;
        }
        if ns == "strategies" {
            if !strategy_ids.contains(&key) {
                continue;
            }
        } else if ns == "backtest_runs_v2" || ns == "backtest_manifests_v1" {
            // Handled below through the integrity-checking completed report owner.
            continue;
        } else {
            let sid = if ns == "dataset_snapshots_v1" {
                value["content"]["strategyId"].as_str()
            } else if ns == "public_backtest_configurations_v1" {
                value["configuration"]["strategy"]["strategyId"].as_str()
            } else if ns == "public_backtest_reports_v1" {
                value["run"]["strategyId"].as_str()
            } else {
                strategy_id(&value)
            };
            if !sid.is_some_and(|sid| strategy_ids.contains(sid)) {
                continue;
            }
        }
        let (target, mut projected) = match ns.as_str() {
            "dataset_snapshots_v1" => (
                "public_dataset_references_v1",
                json!({"datasetId":value["datasetId"],"strategyId":value["content"]["strategyId"],"snapshotRef":value["snapshotRef"],"contentHash":value["contentHash"],"rowCount":value["rowCount"],"timeSlice":value["content"]["timeSlice"],"source":value["content"]["source"],"instruments":value["content"]["instruments"],"granularity":value["content"]["granularity"],"timezone":value["content"]["timezone"],"calendar":value["content"]["calendar"],"normalizationPolicy":value["content"]["normalizationPolicy"],"qualityPolicy":value["content"]["qualityPolicy"],"rawDataIncluded":false}),
            ),
            "research_datasets" => (
                "public_dataset_references_v1",
                json!({"datasetId":value["datasetId"],"strategyId":strategy_id(&value),"snapshotRef":value["snapshotRef"],"contentHash":value["contentHash"],"rowCount":value["rowCount"],"timeSlice":value["timeSlice"],"sourcePluginRef":value["sourcePluginRef"],"configuration":select(&value,&["symbol","normalization","timezone","calendar","session","capital","slippage","spread","fillModel","feeModel","orderPolicy"]),"rawDataIncluded":false}),
            ),
            _ => (
                ns.as_str(),
                select(&value, fields(&ns).ok_or("workspace_namespace_invalid")?),
            ),
        };
        if target == "strategies" {
            projected["status"] = json!("stopped");
        }
        if target == "research_notebooks" {
            // Presentation-only selectors and CLI paths are excluded. The typed
            // notebook and its original content hash remain intact.
            projected
                .as_object_mut()
                .expect("object")
                .remove("recordHash");
            projected["replayRefs"] =
                json!({"available":false,"reason":"source_workspace_artifacts_not_restored"});
            projected["recordHash"] = json!(hash(&projected)?);
        }
        entries.push(entry(target, key, projected)?);
    }
    let runs = service.runtime().backtests.list_runs()?;
    if runs.len() > MAX_ENTRIES {
        return Err(code("workspace_size_limit"));
    }
    for run in runs {
        if run.state != crate::domain::BacktestRunState::Completed
            || !service.object_is_visible("backtest_run", &run.run_id)
        {
            continue;
        }
        let report = backtest_lifecycle::report_projection(service, &run.run_id)?;
        if !strategy_ids.contains(&report.run.strategy_id) {
            continue;
        }
        let manifest = service
            .runtime()
            .backtests
            .get_manifest(&run.run_id)?
            .ok_or("backtest_report_integrity_failed")?;
        entries.push(entry(
            "public_backtest_configurations_v1",
            run.run_id.clone(),
            json!({"runId":run.run_id,"configuration":manifest.content.configuration}),
        )?);
        entries.push(entry(
            "public_backtest_reports_v1",
            run.run_id.clone(),
            portable_report(&report)?,
        )?);
    }
    entries.sort_by(|a, b| (&a.namespace, &a.key).cmp(&(&b.namespace, &b.key)));
    let mut bundle = PublicWorkspaceBundle {
        schema: WORKSPACE_SCHEMA.to_string(),
        bundle_hash: String::new(),
        entries,
        settings: "not_supported_no_durable_settings_namespace".to_string(),
        legacy_backtests_excluded,
    };
    bundle.bundle_hash = bundle_hash(&bundle)?;
    validate(&bundle)?;
    Ok(bundle)
}
fn portable_report(report: &BacktestReport) -> Result<Value, String> {
    let value = serde_json::to_value(report).map_err(|_| code("workspace_report_invalid"))?;
    let mut value = select(
        &value,
        fields("public_backtest_reports_v1").expect("known fields"),
    );
    value["rawDataIncluded"] = json!(false);
    Ok(value)
}

fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 256
        && !key.contains("..")
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
}
/// Defense in depth over typed and allowlisted payloads, including free text.
fn reject_sensitive(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 48 {
        return Err(code("workspace_depth_limit"));
    }
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
                let harmless_flag =
                    normalized == "rawsecretsexposed" && value == &Value::Bool(false);
                let credential_key = [
                    "password",
                    "secret",
                    "token",
                    "privatekey",
                    "apikey",
                    "brokeraccount",
                    "accountid",
                    "accountref",
                    "credential",
                ]
                .iter()
                .any(|part| normalized.contains(part));
                if (!harmless_flag && credential_key)
                    || [
                        "password",
                        "secret",
                        "apikey",
                        "accesstoken",
                        "refreshtoken",
                        "privatekey",
                        "brokeraccount",
                        "accountid",
                        "authoritycontext",
                        "localliveauthority",
                        "activationid",
                        "credentials",
                        "authorization",
                        "rawdata",
                        "observations",
                        "records",
                    ]
                    .iter()
                    .any(|k| {
                        normalized == *k
                            || (normalized.ends_with(k)
                                && !(normalized == "staleobservations" && value.is_string()))
                    })
                {
                    return Err(code("workspace_sensitive_data_rejected"));
                }
                reject_sensitive(value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_sensitive(value, depth + 1)?;
            }
        }
        Value::String(s) => {
            let lower = s.to_ascii_lowercase();
            if [
                "-----begin",
                "bearer ",
                "sk_live_",
                "sk_test_",
                "rk_live_",
                "ghp_",
                "xoxb-",
                "akia",
                "aws_secret_access_key",
                "api_key=",
                "password=",
                "access_token=",
            ]
            .iter()
            .any(|needle| lower.contains(needle))
                || (s.split('.').count() == 3 && s.starts_with("eyJ"))
            {
                return Err(code("workspace_sensitive_data_rejected"));
            }
            if lower.starts_with("account:") || lower.starts_with("credential:") {
                return Err(code("workspace_sensitive_reference_rejected"));
            }
            if lower.starts_with("file:")
                || lower.starts_with('/')
                || lower.starts_with("../")
                || lower.contains("/../")
                || lower.contains('\\')
                || (lower.len() > 2
                    && lower.as_bytes()[1] == b':'
                    && lower.as_bytes()[0].is_ascii_alphabetic())
            {
                return Err(code("workspace_path_rejected"));
            }
            if let Ok(url) = url::Url::parse(s) {
                if !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(code("workspace_sensitive_reference_rejected"));
                }
            }
        }
        _ => {}
    }
    Ok(())
}
fn bundle_hash(bundle: &PublicWorkspaceBundle) -> Result<String, String> {
    hash(
        &json!({"schema":bundle.schema,"entries":bundle.entries,"settings":bundle.settings,"legacyBacktestsExcluded":bundle.legacy_backtests_excluded}),
    )
}
fn validate(bundle: &PublicWorkspaceBundle) -> Result<(), String> {
    if bundle.schema != WORKSPACE_SCHEMA
        || bundle.settings != "not_supported_no_durable_settings_namespace"
    {
        return Err(code("workspace_schema_invalid"));
    }
    if bundle.entries.len() > MAX_ENTRIES
        || serde_json::to_vec(bundle)
            .map_err(|_| code("workspace_invalid"))?
            .len()
            > MAX_BUNDLE_BYTES
    {
        return Err(code("workspace_size_limit"));
    }
    if bundle.bundle_hash != bundle_hash(bundle)? {
        return Err(code("workspace_bundle_checksum_failed"));
    }
    let mut seen = BTreeSet::new();
    let strategies = bundle
        .entries
        .iter()
        .filter(|entry| entry.namespace == "strategies")
        .map(|entry| entry.key.as_str())
        .collect::<BTreeSet<_>>();
    for entry in &bundle.entries {
        if !safe_key(&entry.key) || !seen.insert((&entry.namespace, &entry.key)) {
            return Err(code("workspace_key_invalid"));
        }
        let allowed = fields(&entry.namespace).ok_or("workspace_namespace_invalid")?;
        let object = entry.value.as_object().ok_or("workspace_record_invalid")?;
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(code("workspace_field_invalid"));
        }
        if serde_json::to_vec(&entry.value)
            .map_err(|_| code("workspace_invalid"))?
            .len()
            > MAX_ENTRY_BYTES
        {
            return Err(code("workspace_entry_size_limit"));
        }
        reject_sensitive(&entry.value, 0)?;
        if entry.content_hash != hash(&entry.value)? {
            return Err(code("workspace_entry_checksum_failed"));
        }
        let sid = match entry.namespace.as_str() {
            "strategies" => {
                if entry.value["id"] != entry.key || entry.value["status"] != "stopped" {
                    return Err(code("workspace_strategy_invalid"));
                }
                Some(entry.key.as_str())
            }
            "public_backtest_reports_v1" => {
                validate_report(&entry.value)?;
                if entry.value["run"]["runId"] != entry.key {
                    return Err(code("workspace_report_invalid"));
                }
                entry.value["run"]["strategyId"].as_str()
            }
            "public_backtest_configurations_v1" => {
                let config: BacktestConfiguration =
                    serde_json::from_value(entry.value["configuration"].clone())
                        .map_err(|_| code("workspace_configuration_invalid"))?;
                if !config.validate().ready || entry.value["runId"] != entry.key {
                    return Err(code("workspace_configuration_invalid"));
                }
                entry.value["configuration"]["strategy"]["strategyId"].as_str()
            }
            "strategy_versions" | "strategy_drafts" => {
                let spec = entry
                    .value
                    .get("spec")
                    .filter(|v| v.is_object())
                    .ok_or("workspace_strategy_spec_required")?;
                let expected = hash(spec)?;
                if entry.value["specHash"].as_str() != Some(expected.as_str()) {
                    return Err(code("workspace_strategy_hash_failed"));
                }
                for field in ["canonicalSpecBytes", "normalizedSourceSpecBytes"] {
                    if let Some(bytes) = entry.value[field].as_str() {
                        let parsed: Value = serde_json::from_str(bytes)
                            .map_err(|_| code("workspace_strategy_source_invalid"))?;
                        if &parsed != spec {
                            return Err(code("workspace_strategy_source_invalid"));
                        }
                    }
                }
                strategy_id(&entry.value)
            }
            "research_notebooks" => {
                let notebook: crate::research_notebook::ResearchNotebookContract =
                    serde_json::from_value(entry.value["notebook"].clone())
                        .map_err(|_| code("workspace_notebook_invalid"))?;
                if !notebook.validate().ok
                    || notebook.notebook_id != entry.key
                    || entry.value["notebookId"] != entry.key
                    || entry.value["strategyId"] != notebook.strategy_id
                    || entry.value["notebookHash"]
                        != crate::research_notebook::research_notebook_hash(&notebook)
                {
                    return Err(code("workspace_notebook_invalid"));
                }
                strategy_id(&entry.value)
            }
            _ => strategy_id(&entry.value),
        };
        if !sid.is_some_and(|sid| strategies.contains(sid)) {
            return Err(code("workspace_strategy_reference_invalid"));
        }
        if entry.namespace == "public_dataset_references_v1"
            && (entry.value["datasetId"] != entry.key || entry.value["rawDataIncluded"] != false)
        {
            return Err(code("workspace_dataset_invalid"));
        }
    }
    let versions = bundle
        .entries
        .iter()
        .filter(|entry| entry.namespace == "strategy_versions")
        .collect::<Vec<_>>();
    for entry in &bundle.entries {
        if entry.namespace == "strategies" {
            if let Some(version_id) = entry.value["currentVersionId"]
                .as_str()
                .or(entry.value["current_version_id"].as_str())
            {
                if !versions.iter().any(|version| {
                    version.value["id"] == version_id
                        && strategy_id(&version.value) == Some(entry.key.as_str())
                }) {
                    return Err(code("workspace_version_reference_invalid"));
                }
            }
        }
        if entry.namespace == "public_backtest_reports_v1" {
            let config = bundle
                .entries
                .iter()
                .find(|config| {
                    config.namespace == "public_backtest_configurations_v1"
                        && config.key == entry.key
                })
                .ok_or("workspace_report_configuration_missing")?;
            let run = &entry.value["run"];
            let binding = &config.value["configuration"]["strategy"];
            if run["strategyId"] != binding["strategyId"]
                || run["strategyVersionId"] != binding["strategyVersionId"]
                || run["strategySpecHash"] != binding["strategySpecHash"]
                || entry.value["provenance"]["datasetHash"]
                    != config.value["configuration"]["dataset"]["contentHash"]
            {
                return Err(code("workspace_report_binding_invalid"));
            }
            if !versions.iter().any(|v| {
                v.value["id"] == binding["strategyVersionId"]
                    && v.value["specHash"] == binding["strategySpecHash"]
            }) {
                return Err(code("workspace_report_version_missing"));
            }
            if !bundle.entries.iter().any(|v| {
                v.namespace == "public_dataset_references_v1"
                    && v.value["datasetId"] == run["datasetId"]
                    && v.value["contentHash"] == entry.value["provenance"]["datasetHash"]
            }) {
                return Err(code("workspace_report_dataset_missing"));
            }
        }
    }
    Ok(())
}
fn validate_report(value: &Value) -> Result<(), String> {
    if value["schema"] != crate::backtest_report::BACKTEST_REPORT_SCHEMA
        || value["rawDataIncluded"] != false
        || value["run"]["state"] != "completed"
    {
        return Err(code("workspace_report_invalid"));
    }
    // Round-trip through the typed report enforces every nested contract's
    // closed fields; chart rows are deliberately absent from portable artifacts.
    let mut typed = value.clone();
    typed
        .as_object_mut()
        .ok_or("workspace_report_invalid")?
        .remove("rawDataIncluded");
    typed["chart"] = json!({"range":null,"bars":[],"markers":[],"connectors":[],"warnings":[],"evidenceRefs":[]});
    let _: BacktestReport =
        serde_json::from_value(typed).map_err(|_| code("workspace_report_invalid"))?;
    let overview: crate::backtest_report::BacktestReportOverview =
        serde_json::from_value(value["overview"].clone())
            .map_err(|_| code("workspace_report_invalid"))?;
    if overview
        .ending_equity_micros
        .checked_sub(overview.starting_equity_micros)
        != Some(overview.net_pnl_micros)
        || overview.gross_pnl_micros.checked_sub(overview.fees_micros)
            != Some(overview.net_pnl_micros)
        || overview
            .ending_cash_micros
            .checked_add(overview.market_value_micros)
            != Some(overview.ending_equity_micros)
    {
        return Err(code("workspace_report_reconciliation_failed"));
    }
    Ok(())
}

// Parse untrusted bundles without silently discarding duplicate JSON fields.
// serde_json::Value otherwise permits a later value to overwrite an earlier one.
struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("unique-key JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<UniqueJson, E> {
                serde_json::Number::from_f64(v)
                    .map(|v| UniqueJson(Value::Number(v)))
                    .ok_or_else(|| E::custom("nonfinite JSON"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson(json!(v)))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::String(v)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueJson>()? {
                    values.push(value.0);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    values.insert(key, map.next_value::<UniqueJson>()?.0);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

pub fn import(service: &TradeAssemblyService, raw: &[u8]) -> Result<Value, String> {
    local_process(service)?;
    import_owned(service, raw)
}

fn import_owned(service: &TradeAssemblyService, raw: &[u8]) -> Result<Value, String> {
    if raw.len() > MAX_BUNDLE_BYTES {
        return Err(code("workspace_size_limit"));
    }
    let parsed: UniqueJson =
        serde_json::from_slice(raw).map_err(|_| code("workspace_bundle_invalid"))?;
    let bundle: PublicWorkspaceBundle =
        serde_json::from_value(parsed.0).map_err(|_| code("workspace_bundle_invalid"))?;
    validate(&bundle)?;
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("workspace.import:{}", bundle.bundle_hash))?,
    );
    let mut writes = Vec::new();
    for entry in &bundle.entries {
        let mut value = entry.value.clone();
        if entry.namespace == "strategy_drafts" {
            value["capabilityBindings"] = json!({});
        }
        if entry.namespace == "research_notebooks" {
            let notebook: crate::research_notebook::ResearchNotebookContract =
                serde_json::from_value(value["notebook"].clone())
                    .map_err(|_| code("workspace_notebook_invalid"))?;
            let exported = crate::research_notebook::export_research_notebook(&notebook)
                .map_err(|_| code("workspace_notebook_invalid"))?;
            writes.push(StorageWrite::new(
                "research_notebook_exports",
                &entry.key,
                serde_json::to_value(exported).map_err(|_| code("workspace_notebook_invalid"))?,
                context.clone(),
            ));
        }
        writes.push(StorageWrite::new(
            &entry.namespace,
            &entry.key,
            value,
            context.clone(),
        ));
        if let Some(owner) = service.invocation_owner() {
            let object_type = match entry.namespace.as_str() {
                "strategies" => "strategy",
                "research_notebooks" => "research_notebook",
                "research_universes" => "research_universe",
                _ => continue,
            };
            let parent_id = if object_type == "strategy" {
                None
            } else {
                strategy_id(&entry.value).map(str::to_string)
            };
            let scope = ObjectScope {
                object_type: object_type.to_string(),
                object_id: entry.key.clone(),
                owner,
                parent_type: parent_id.as_ref().map(|_| "strategy".to_string()),
                parent_id,
            };
            writes.push(StorageWrite::new(
                "object_authorization_scopes_v1",
                format!("{object_type}:{}", entry.key),
                serde_json::to_value(scope).map_err(|_| code("workspace_scope_invalid"))?,
                context.clone(),
            ));
        }
    }
    let mut bootstrap = Vec::new();
    if let Some(owner) = service.invocation_owner() {
        for (kind, id) in [
            ("strategy", "strat_local_btc_demo"),
            ("strategy", "strat_local_crypto_replay"),
            ("plugin_instance", "sim"),
            ("plugin_instance", "local-data"),
            ("plugin_instance", "alpaca-paper"),
            ("credential_reference", "sim"),
            ("credential_reference", "local-data"),
            ("credential_reference", "alpaca-paper"),
        ] {
            let scope = ObjectScope {
                object_type: kind.to_string(),
                object_id: id.to_string(),
                owner: owner.clone(),
                parent_type: (kind == "credential_reference")
                    .then(|| "plugin_instance".to_string()),
                parent_id: (kind == "credential_reference").then(|| id.to_string()),
            };
            bootstrap.push(StorageExpectation::new(
                "object_authorization_scopes_v1",
                format!("{kind}:{id}"),
                Some(serde_json::to_value(scope).map_err(|_| code("workspace_scope_invalid"))?),
            ));
        }
    }
    let outcome = service.runtime().storage.import_public_workspace(
        &writes,
        &bundle.bundle_hash,
        &bootstrap,
    )?;
    Ok(
        json!({"ok":true,"schema":WORKSPACE_SCHEMA,"bundleHash":bundle.bundle_hash,"entryCount":bundle.entries.len(),"restoredState":"stopped","duplicate":outcome==ImmutablePutOutcome::AlreadyPresent,"rawDataIncluded":false,"noAdvice":LEGAL_BOUNDARY}),
    )
}

pub fn read_stdin() -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_BUNDLE_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| code("workspace_input_unreadable"))?;
    if raw.len() > MAX_BUNDLE_BYTES {
        return Err(code("workspace_size_limit"));
    }
    Ok(raw)
}
#[cfg(unix)]
fn parent_directory(path: &Path) -> Result<(std::os::fd::OwnedFd, &std::ffi::OsStr), String> {
    use rustix::fs::{open, openat, Mode, OFlags};
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(code("workspace_path_rejected"));
    }
    let filename = path.file_name().ok_or("workspace_path_rejected")?;
    let flags = OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::RDONLY;
    let mut directory = open(
        if path.is_absolute() { "/" } else { "." },
        flags,
        Mode::empty(),
    )
    .map_err(|_| code("workspace_path_unavailable"))?;
    for component in path.parent().unwrap_or(Path::new(".")).components() {
        if let Component::Normal(segment) = component {
            directory = openat(&directory, segment, flags, Mode::empty())
                .map_err(|_| code("workspace_symlink_or_parent_rejected"))?;
        }
    }
    Ok((directory, filename))
}
#[cfg(unix)]
pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    use rustix::fs::{openat, Mode, OFlags};
    let (directory, filename) = parent_directory(path)?;
    let fd = openat(
        &directory,
        filename,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|_| code("workspace_file_unreadable"))?;
    let file = fs::File::from(fd);
    if !file
        .metadata()
        .map_err(|_| code("workspace_file_unreadable"))?
        .is_file()
    {
        return Err(code("workspace_file_invalid"));
    }
    let mut raw = Vec::new();
    file.take((MAX_BUNDLE_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| code("workspace_file_unreadable"))?;
    if raw.len() > MAX_BUNDLE_BYTES {
        return Err(code("workspace_size_limit"));
    }
    Ok(raw)
}
#[cfg(not(unix))]
pub fn read_file(_: &Path) -> Result<Vec<u8>, String> {
    Err(code("workspace_file_io_unsupported"))
}
#[cfg(unix)]
pub fn export_file(service: &TradeAssemblyService, path: &Path) -> Result<Value, String> {
    write_bundle_file(export(service)?, path)
}
#[cfg(unix)]
fn write_bundle_file(bundle: PublicWorkspaceBundle, path: &Path) -> Result<Value, String> {
    use rustix::fs::{linkat, openat, unlinkat, AtFlags, Mode, OFlags};
    let (directory, filename) = parent_directory(path)?;
    let raw = serde_json::to_vec(&bundle).map_err(|_| code("workspace_bundle_invalid"))?;
    let temporary = format!(".workspace-{:032x}.tmp", rand::random::<u128>());
    let mut temporary_created = false;
    let result = (|| {
        let fd = openat(
            &directory,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| code("workspace_export_write_failed"))?;
        temporary_created = true;
        let mut file = fs::File::from(fd);
        file.write_all(&raw)
            .and_then(|_| file.sync_all())
            .map_err(|_| code("workspace_export_write_failed"))?;
        // linkat never replaces a destination. All operations are anchored to
        // opened directory descriptors, so concurrent ancestor swaps cannot
        // redirect writes through symlinks. A crash exposes complete bytes only.
        linkat(
            &directory,
            temporary.as_str(),
            &directory,
            filename,
            AtFlags::empty(),
        )
        .map_err(|_| code("workspace_export_destination_exists_or_unavailable"))?;
        rustix::fs::fsync(&directory).map_err(|_| code("workspace_export_sync_failed"))?;
        Ok(
            json!({"ok":true,"schema":WORKSPACE_SCHEMA,"bundleHash":bundle.bundle_hash,"entryCount":bundle.entries.len()}),
        )
    })();
    if temporary_created {
        let _ = unlinkat(&directory, temporary.as_str(), AtFlags::empty());
    }
    result
}
#[cfg(not(unix))]
pub fn export_file(_: &TradeAssemblyService, _: &Path) -> Result<Value, String> {
    Err(code("workspace_file_io_unsupported"))
}
#[cfg(not(unix))]
fn write_bundle_file(_: PublicWorkspaceBundle, _: &Path) -> Result<Value, String> {
    Err(code("workspace_file_io_unsupported"))
}

pub fn result_summary(service: &TradeAssemblyService, id: &str) -> Result<Value, String> {
    service
        .require_object("strategy", id)
        .map_err(|_| code("object_not_available"))?;
    if !safe_key(id) {
        return Err(code("strategy_id_invalid"));
    }
    let rows = service.runtime().storage.public_workspace_snapshot(
        &["execution_runs", "public_backtest_reports_v1"],
        MAX_ENTRIES,
        MAX_BUNDLE_BYTES,
    )?;
    let mut runs = Vec::new();
    let mut backtests = Vec::new();
    let mut paper = Vec::new();
    for (ns, key, value) in rows {
        if ns == "public_backtest_reports_v1" {
            if value["run"]["strategyId"] == id {
                validate_report(&value)?;
                backtests.push(summarize_report(&value));
            }
        } else if strategy_id(&value) == Some(id) {
            let activation = value["activationId"].as_str().unwrap_or_default();
            let projection = execution::workspace(service, id, &json!({"activationId":activation}));
            // The wire field retains its v1 name, but the execution ledger owns
            // a reconciliation state (clear/required/unavailable), not a status.
            let reconciliation =
                projection["execution"]["ledger"]["reconciliation"]["state"].clone();
            runs.push(json!({"runId":key,"mode":value["mode"],"state":value["state"],"period":{"start":timestamp(&value,"startedAtMs"),"end":timestamp(&value,"stoppedAtMs")},"reconciliationStatus":reconciliation}));
            if value["mode"] == "paper" {
                let totals = &projection["execution"]["ledger"]["totals"];
                paper.push(json!({"runId":key,"period":{"start":timestamp(&value,"startedAtMs"),"end":timestamp(&value,"stoppedAtMs")},"provenance":{"snapshotHash":projection["snapshot"]["snapshotHash"],"source":"tradeassembly.execution_ledger.v2","scope":"since_run_start"},"metrics":{"realizedPnl":totals["realizedPnl"],"unrealizedPnl":totals["unrealizedPnl"],"totalPnl":totals["totalPnl"],"currency":totals["currency"],"fees":null},"reconciliationStatus":reconciliation,"completeness":if totals["available"]==true {"partial"}else{"unavailable"},"reason":if totals["available"]==true {json!("fee_accounting_unavailable")}else{totals["unavailableReason"].clone()}}));
            }
        }
    }
    let native = service.runtime().backtests.list_runs()?;
    if native.len() > MAX_ENTRIES {
        return Err(code("workspace_size_limit"));
    }
    for run in native {
        if run.state != crate::domain::BacktestRunState::Completed
            || !service.object_is_visible("backtest_run", &run.run_id)
        {
            continue;
        }
        let report = backtest_lifecycle::report_projection(service, &run.run_id)?;
        if report.run.strategy_id == id {
            backtests.push(summarize_report(&portable_report(&report)?));
        }
    }
    runs.sort_by(|a, b| a["runId"].as_str().cmp(&b["runId"].as_str()));
    paper.sort_by(|a, b| a["runId"].as_str().cmp(&b["runId"].as_str()));
    backtests.sort_by(|a, b| a["runId"].as_str().cmp(&b["runId"].as_str()));
    backtests.dedup_by(|a, b| a["runId"] == b["runId"]);
    if runs.len() > MAX_ENTRIES || paper.len() > MAX_ENTRIES || backtests.len() > MAX_ENTRIES {
        return Err(code("result_summary_size_limit"));
    }
    let cursor = service
        .invocation_owner()
        .map(|owner| service.runtime().journal.try_owner_cursor(&owner))
        .transpose()?
        .flatten();
    let source_hash = hash(
        &json!({"operational":runs,"paper":paper,"backtests":backtests,"localArchiveCursor":cursor}),
    )?;
    let summary = json!({"schema":RESULT_SUMMARY_SCHEMA,"strategyId":id,"source":{"kind":"core_durable_state","schema":RESULT_SUMMARY_SCHEMA,"contentHash":source_hash,"localArchiveCursor":cursor,"cursorCompleteness":if cursor.is_some(){"available"}else{"unavailable"}},"operational":{"runs":runs,"completeness":"observed_state"},"paper":{"runs":paper,"completeness":if paper.is_empty(){"unavailable"}else{"partial"},"reason":if paper.is_empty(){json!("paper_execution_evidence_unavailable")}else{Value::Null}},"backtests":backtests,"noAdvice":LEGAL_BOUNDARY});
    if serde_json::to_vec(&summary)
        .map_err(|_| code("result_summary_invalid"))?
        .len()
        > MAX_BUNDLE_BYTES
    {
        return Err(code("result_summary_size_limit"));
    }
    Ok(summary)
}
fn timestamp(value: &Value, key: &str) -> Value {
    value[key]
        .as_i64()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|t| json!(t.to_rfc3339()))
        .unwrap_or(Value::Null)
}
fn summarize_report(report: &Value) -> Value {
    json!({"runId":report["run"]["runId"],"period":report["provenance"]["timeCoverage"],"provenance":{"reportHash":report["reportHash"],"strategyVersionId":report["provenance"]["strategyVersionId"],"strategySpecHash":report["provenance"]["strategySpecHash"],"datasetId":report["provenance"]["datasetId"],"datasetHash":report["provenance"]["datasetHash"]},"metrics":select(&report["overview"],&["grossPnlMicros","netPnlMicros","feesMicros","realizedPnlMicros","unrealizedPnlMicros","returnBps"]),"reconciliation":{"status":report["overview"]["reconciliationStatus"],"residualMicros":report["overview"]["reconciliationResidualMicros"]},"completeness":"completed_report"})
}

#[cfg(test)]
mod authority_tests {
    use super::*;
    use crate::cli::{run_with_service, Cli};
    use crate::cli_identity::verify_cli_session;
    use crate::runtime_config::{RuntimeBuilder, RuntimeConfig};
    use clap::Parser;

    #[tokio::test]
    async fn local_workspace_cli_remains_account_free_and_rejects_server_profiles() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let mut config = RuntimeConfig::local(root.join("source.db").display().to_string());
        config.oidc_profile = "local_owner".into();
        config.artifact_root = Some(root.join("exports").display().to_string());
        let local_service = |config: &RuntimeConfig| {
            let (runtime, manifest) = RuntimeBuilder::new(config.clone())
                .with_finance_authority(std::sync::Arc::new(
                    crate::finance_authority::TestFinanceAuthority,
                ))
                .build()
                .unwrap();
            let mut service =
                TradeAssemblyService::from_runtime(config.database_path.clone().unwrap(), runtime);
            service.runtime_manifest = Some(manifest);
            service.oauth_config = Some(config.clone());
            service
        };
        let source = local_service(&config);
        let session = verify_cli_session(&config).await.unwrap();
        let workspace = VerifiedLocalWorkspace::from_verified_session(&source, session).unwrap();
        let bundle_path = root.join("bundle.json");
        let cli = Cli::try_parse_from([
            "tradeassembly",
            "workspace",
            "export",
            "--out",
            bundle_path.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(run_with_service(cli, &source, &config).await, 0);
        assert_eq!(
            read_file(&bundle_path).unwrap(),
            serde_json::to_vec(&workspace.export().unwrap()).unwrap()
        );
        for profile in ["self_hosted", "serverless"] {
            // Even a server profile with SQLite injected cannot mint the local
            // process capability. Check before any export or restore mutation.
            let mut server = source.clone();
            server.runtime_manifest.as_mut().unwrap().profile = profile.into();
            let session = verify_cli_session(&config).await.unwrap();
            assert!(VerifiedLocalWorkspace::from_verified_session(&server, session).is_err());
            assert_eq!(
                export(&server).unwrap_err(),
                "workspace_local_sqlite_owner_required"
            );
            assert_eq!(
                import(&server, &read_file(&bundle_path).unwrap()).unwrap_err(),
                "workspace_local_sqlite_owner_required"
            );
        }
        let mut target_config = config.clone();
        target_config.database_path = Some(root.join("target.db").display().to_string());
        let target = local_service(&target_config);
        let cli = Cli::try_parse_from([
            "tradeassembly",
            "workspace",
            "import",
            "--from",
            bundle_path.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(run_with_service(cli, &target, &target_config).await, 0);
        assert!(target
            .runtime()
            .storage
            .list_json("execution_activations")
            .unwrap()
            .is_empty());
    }
}
