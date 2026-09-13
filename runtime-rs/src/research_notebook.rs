// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const RESEARCH_NOTEBOOK_SCHEMA_VERSION: &str = "tradeassembly.research_notebook.v1";
pub const RESEARCH_NOTEBOOK_EXPORT_SCHEMA_VERSION: &str =
    "tradeassembly.research_notebook.export.v1";
pub const RESEARCH_NOTEBOOK_NOTICE: &str = "Descriptive research evidence only. TradeAssembly did not recommend trades, timing, sizing, allocation, activation, or strategy changes.";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchNotebookBlockKind {
    Observation,
    Assumption,
    DeterministicCalculation,
    ModelDerivedOutput,
    Limitation,
    UserAuthoredDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchNotebookToolKind {
    DataQuery,
    Backtest,
    MonteCarlo,
    ScenarioValuation,
    SelectorExplain,
    FillQuality,
    LifecycleCalendar,
    NotebookExport,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchNotebookRedactionStatus {
    Clean,
    Redacted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchNotebookWarningSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchNotebookWarningCode {
    MissingStrategyScope,
    UnsavedPromptRejected,
    MissingProvenance,
    MissingReplayRef,
    InvalidDataWindow,
    RawSecretRejected,
    BrokerAccountIdentifierRejected,
    HostedControlRejected,
    SensitivePayloadRejected,
    BrokenArtifactRef,
    PredictiveOrAdviceCopyRejected,
    UnsupportedSchema,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookSourceProvenance {
    pub source: String,
    pub source_ref: String,
    pub collected_at_unix_ms: u64,
    pub raw_secrets_exposed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookPromptRef {
    pub prompt_ref: String,
    pub role: String,
    pub content_hash: String,
    pub explicitly_saved: bool,
    pub redaction_status: ResearchNotebookRedactionStatus,
    pub provenance: ResearchNotebookSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub tool_kind: ResearchNotebookToolKind,
    pub arguments_hash: String,
    pub output_ref: String,
    pub output_hash: String,
    pub redaction_status: ResearchNotebookRedactionStatus,
    pub provenance: ResearchNotebookSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookInputRef {
    pub input_id: String,
    pub input_kind: String,
    pub ref_uri: String,
    pub content_hash: String,
    pub redaction_status: ResearchNotebookRedactionStatus,
    pub provenance: ResearchNotebookSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookDataWindow {
    pub window_id: String,
    pub starts_at_unix_ms: u64,
    pub ends_at_unix_ms: u64,
    pub timezone: String,
    pub as_of_unix_ms: u64,
    pub stale_after_unix_ms: Option<u64>,
    #[serde(default)]
    pub dataset_refs: Vec<String>,
    pub provenance: ResearchNotebookSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookAssumption {
    pub assumption_id: String,
    pub statement: String,
    pub source_ref: String,
    pub user_supplied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookReportRef {
    pub report_id: String,
    pub report_kind: String,
    pub report_ref: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookChartRef {
    pub chart_id: String,
    pub title: String,
    pub artifact_ref: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookTableRef {
    pub table_id: String,
    pub title: String,
    pub artifact_ref: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookExplainableBlock {
    pub block_id: String,
    pub kind: ResearchNotebookBlockKind,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub source_refs: Vec<String>,
    pub calculation_ref: Option<String>,
    pub model_ref: Option<String>,
    pub user_decision_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookReplayRef {
    pub replay_ref: String,
    pub deterministic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookWarning {
    pub code: ResearchNotebookWarningCode,
    pub severity: ResearchNotebookWarningSeverity,
    pub message: String,
    pub replay_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookContract {
    pub schema_version: String,
    pub notebook_id: String,
    pub strategy_id: String,
    pub run_id: String,
    pub title: String,
    pub created_at_unix_ms: u64,
    #[serde(default)]
    pub prompts: Vec<ResearchNotebookPromptRef>,
    #[serde(default)]
    pub tool_calls: Vec<ResearchNotebookToolCall>,
    #[serde(default)]
    pub inputs: Vec<ResearchNotebookInputRef>,
    #[serde(default)]
    pub data_windows: Vec<ResearchNotebookDataWindow>,
    #[serde(default)]
    pub assumptions: Vec<ResearchNotebookAssumption>,
    #[serde(default)]
    pub explainable_blocks: Vec<ResearchNotebookExplainableBlock>,
    #[serde(default)]
    pub report_refs: Vec<ResearchNotebookReportRef>,
    #[serde(default)]
    pub chart_refs: Vec<ResearchNotebookChartRef>,
    #[serde(default)]
    pub table_refs: Vec<ResearchNotebookTableRef>,
    #[serde(default)]
    pub warnings: Vec<ResearchNotebookWarning>,
    pub provenance: ResearchNotebookSourceProvenance,
    #[serde(default)]
    pub replay_refs: Vec<ResearchNotebookReplayRef>,
    pub redaction_status: ResearchNotebookRedactionStatus,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub no_advice_notice: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookValidationReport {
    pub ok: bool,
    #[serde(default)]
    pub warnings: Vec<ResearchNotebookWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchNotebookExportBundle {
    pub schema_version: String,
    pub notebook_id: String,
    pub content_hash: String,
    pub json: String,
    pub markdown: String,
    pub html: String,
}

impl ResearchNotebookContract {
    pub fn validate(&self) -> ResearchNotebookValidationReport {
        validate_research_notebook(self)
    }
}

pub fn validate_research_notebook(
    notebook: &ResearchNotebookContract,
) -> ResearchNotebookValidationReport {
    let mut warnings = notebook.warnings.clone();
    validate_required_fields(notebook, &mut warnings);
    validate_saved_prompts(notebook, &mut warnings);
    validate_windows(notebook, &mut warnings);
    validate_provenance(notebook, &mut warnings);
    validate_secret_surfaces(notebook, &mut warnings);
    validate_copy_boundary(notebook, &mut warnings);
    let ok = warnings
        .iter()
        .all(|warning| warning.severity != ResearchNotebookWarningSeverity::Error);
    ResearchNotebookValidationReport { ok, warnings }
}

pub fn research_notebook_hash(notebook: &ResearchNotebookContract) -> String {
    let bytes = serde_json::to_vec(notebook).expect("research notebook serializes");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub fn export_research_notebook(
    notebook: &ResearchNotebookContract,
) -> Result<ResearchNotebookExportBundle, ResearchNotebookValidationReport> {
    let validation = notebook.validate();
    if !validation.ok {
        return Err(validation);
    }
    let json = serde_json::to_string_pretty(notebook).expect("research notebook json export");
    let markdown = export_markdown(notebook);
    let html = export_html(notebook);
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    hasher.update(markdown.as_bytes());
    hasher.update(html.as_bytes());
    Ok(ResearchNotebookExportBundle {
        schema_version: RESEARCH_NOTEBOOK_EXPORT_SCHEMA_VERSION.to_string(),
        notebook_id: notebook.notebook_id.clone(),
        content_hash: format!("sha256:{:x}", hasher.finalize()),
        json,
        markdown,
        html,
    })
}

pub fn required_explainable_block_kinds() -> BTreeSet<ResearchNotebookBlockKind> {
    BTreeSet::from([
        ResearchNotebookBlockKind::Observation,
        ResearchNotebookBlockKind::Assumption,
        ResearchNotebookBlockKind::DeterministicCalculation,
        ResearchNotebookBlockKind::ModelDerivedOutput,
        ResearchNotebookBlockKind::Limitation,
        ResearchNotebookBlockKind::UserAuthoredDecision,
    ])
}

fn validate_required_fields(
    notebook: &ResearchNotebookContract,
    warnings: &mut Vec<ResearchNotebookWarning>,
) {
    if notebook.schema_version != RESEARCH_NOTEBOOK_SCHEMA_VERSION {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::UnsupportedSchema,
            "Research notebook schema version is unsupported.",
        ));
    }
    if notebook.strategy_id.trim().is_empty()
        || notebook.run_id.trim().is_empty()
        || notebook.notebook_id.trim().is_empty()
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::MissingStrategyScope,
            "Research notebook must be scoped to one strategy research run.",
        ));
    }
    if notebook.replay_refs.is_empty()
        || notebook
            .replay_refs
            .iter()
            .all(|replay| !replay.deterministic)
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::MissingReplayRef,
            "Research notebook must include at least one deterministic replay ref.",
        ));
    }
}

fn validate_saved_prompts(
    notebook: &ResearchNotebookContract,
    warnings: &mut Vec<ResearchNotebookWarning>,
) {
    if notebook
        .prompts
        .iter()
        .any(|prompt| !prompt.explicitly_saved)
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::UnsavedPromptRejected,
            "Research notebook prompt refs may be stored only when explicitly saved.",
        ));
    }
}

fn validate_windows(
    notebook: &ResearchNotebookContract,
    warnings: &mut Vec<ResearchNotebookWarning>,
) {
    if notebook.data_windows.iter().any(|window| {
        window.ends_at_unix_ms < window.starts_at_unix_ms
            || window.timezone.trim().is_empty()
            || window
                .stale_after_unix_ms
                .is_some_and(|stale_after| window.as_of_unix_ms > stale_after)
    }) {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::InvalidDataWindow,
            "Research notebook contains an invalid or stale data window.",
        ));
    }
}

fn validate_provenance(
    notebook: &ResearchNotebookContract,
    warnings: &mut Vec<ResearchNotebookWarning>,
) {
    let missing_root = provenance_missing(&notebook.provenance);
    let missing_prompts = notebook
        .prompts
        .iter()
        .any(|prompt| provenance_missing(&prompt.provenance));
    let missing_tools = notebook
        .tool_calls
        .iter()
        .any(|call| provenance_missing(&call.provenance));
    let missing_inputs = notebook
        .inputs
        .iter()
        .any(|input| provenance_missing(&input.provenance));
    let missing_windows = notebook
        .data_windows
        .iter()
        .any(|window| provenance_missing(&window.provenance));
    if missing_root || missing_prompts || missing_tools || missing_inputs || missing_windows {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::MissingProvenance,
            "Research notebook provenance is missing source or source ref data.",
        ));
    }
}

fn validate_secret_surfaces(
    notebook: &ResearchNotebookContract,
    warnings: &mut Vec<ResearchNotebookWarning>,
) {
    if notebook.provenance.raw_secrets_exposed
        || notebook
            .prompts
            .iter()
            .any(|prompt| prompt.provenance.raw_secrets_exposed)
        || notebook
            .tool_calls
            .iter()
            .any(|call| call.provenance.raw_secrets_exposed)
        || notebook
            .inputs
            .iter()
            .any(|input| input.provenance.raw_secrets_exposed)
        || notebook
            .data_windows
            .iter()
            .any(|window| window.provenance.raw_secrets_exposed)
        || contains_secret_key(&notebook.metadata)
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::RawSecretRejected,
            "Research notebook contains raw secret-shaped metadata.",
        ));
    }
    if metadata_has_key_or_string(&notebook.metadata, broker_account_indicator)
        || refs_have_string(notebook, broker_account_indicator)
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::BrokerAccountIdentifierRejected,
            "Research notebook must use redacted account scope refs, not broker account identifiers.",
        ));
    }
    if metadata_has_key_or_string(&notebook.metadata, hosted_control_indicator)
        || refs_have_string(notebook, hosted_control_indicator)
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::HostedControlRejected,
            "Research notebook cannot depend on private hosted controls.",
        ));
    }
    if metadata_has_key_or_string(&notebook.metadata, sensitive_payload_indicator)
        || refs_have_string(notebook, sensitive_payload_indicator)
        || notebook.redaction_status == ResearchNotebookRedactionStatus::Rejected
        || notebook
            .prompts
            .iter()
            .any(|prompt| prompt.redaction_status == ResearchNotebookRedactionStatus::Rejected)
        || notebook
            .tool_calls
            .iter()
            .any(|call| call.redaction_status == ResearchNotebookRedactionStatus::Rejected)
        || notebook
            .inputs
            .iter()
            .any(|input| input.redaction_status == ResearchNotebookRedactionStatus::Rejected)
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::SensitivePayloadRejected,
            "Research notebook contains rejected or sensitive payload refs.",
        ));
    }
}

fn validate_copy_boundary(
    notebook: &ResearchNotebookContract,
    warnings: &mut Vec<ResearchNotebookWarning>,
) {
    let mut values = vec![notebook.title.as_str()];
    values.extend(notebook.assumptions.iter().flat_map(|assumption| {
        [
            assumption.statement.as_str(),
            assumption.source_ref.as_str(),
        ]
    }));
    values.extend(notebook.explainable_blocks.iter().flat_map(|block| {
        [
            block.title.as_str(),
            block.summary.as_str(),
            block.calculation_ref.as_deref().unwrap_or_default(),
            block.model_ref.as_deref().unwrap_or_default(),
            block.user_decision_ref.as_deref().unwrap_or_default(),
        ]
    }));
    values.extend(
        notebook
            .warnings
            .iter()
            .map(|warning| warning.message.as_str()),
    );
    if values
        .iter()
        .any(|value| contains_advice_or_prediction(value))
    {
        warnings.push(warning(
            notebook,
            ResearchNotebookWarningCode::PredictiveOrAdviceCopyRejected,
            "Research notebook copy must stay descriptive and non-predictive.",
        ));
    }
}

fn export_markdown(notebook: &ResearchNotebookContract) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", notebook.title));
    out.push_str(&format!("- Strategy: `{}`\n", notebook.strategy_id));
    out.push_str(&format!("- Run: `{}`\n", notebook.run_id));
    out.push_str(&format!("- Notebook: `{}`\n\n", notebook.notebook_id));
    out.push_str("## Explainable Blocks\n\n");
    for block in &notebook.explainable_blocks {
        out.push_str(&format!(
            "### {} ({:?})\n\n{}\n\n",
            block.title, block.kind, block.summary
        ));
    }
    out.push_str("## Artifacts\n\n");
    for report in &notebook.report_refs {
        out.push_str(&format!(
            "- Report `{}`: `{}` (`{}`)\n",
            report.report_kind, report.report_ref, report.content_hash
        ));
    }
    for chart in &notebook.chart_refs {
        out.push_str(&format!(
            "- Chart `{}`: `{}` (`{}`)\n",
            chart.title, chart.artifact_ref, chart.content_hash
        ));
    }
    for table in &notebook.table_refs {
        out.push_str(&format!(
            "- Table `{}`: `{}` (`{}`)\n",
            table.title, table.artifact_ref, table.content_hash
        ));
    }
    out.push_str("\n## Replay\n\n");
    for replay in &notebook.replay_refs {
        out.push_str(&format!(
            "- `{}` deterministic={}\n",
            replay.replay_ref, replay.deterministic
        ));
    }
    out.push_str(&format!("\n{}\n", notebook.no_advice_notice));
    out
}

fn export_html(notebook: &ResearchNotebookContract) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html><html><head><meta charset=\"utf-8\"><title>");
    out.push_str(&escape_html(&notebook.title));
    out.push_str("</title></head><body>");
    out.push_str(&format!("<h1>{}</h1>", escape_html(&notebook.title)));
    out.push_str(&format!(
        "<p><strong>Strategy</strong> <code>{}</code> <strong>Run</strong> <code>{}</code></p>",
        escape_html(&notebook.strategy_id),
        escape_html(&notebook.run_id)
    ));
    out.push_str("<h2>Explainable Blocks</h2>");
    for block in &notebook.explainable_blocks {
        out.push_str(&format!(
            "<section><h3>{}</h3><p><code>{:?}</code></p><p>{}</p></section>",
            escape_html(&block.title),
            block.kind,
            escape_html(&block.summary)
        ));
    }
    out.push_str("<h2>Artifacts</h2><ul>");
    for report in &notebook.report_refs {
        out.push_str(&format!(
            "<li>Report <code>{}</code>: <code>{}</code></li>",
            escape_html(&report.report_kind),
            escape_html(&report.report_ref)
        ));
    }
    for chart in &notebook.chart_refs {
        out.push_str(&format!(
            "<li>Chart <code>{}</code>: <code>{}</code></li>",
            escape_html(&chart.title),
            escape_html(&chart.artifact_ref)
        ));
    }
    for table in &notebook.table_refs {
        out.push_str(&format!(
            "<li>Table <code>{}</code>: <code>{}</code></li>",
            escape_html(&table.title),
            escape_html(&table.artifact_ref)
        ));
    }
    out.push_str("</ul><h2>Replay</h2><ul>");
    for replay in &notebook.replay_refs {
        out.push_str(&format!(
            "<li><code>{}</code> deterministic={}</li>",
            escape_html(&replay.replay_ref),
            replay.deterministic
        ));
    }
    out.push_str("</ul><p>");
    out.push_str(&escape_html(&notebook.no_advice_notice));
    out.push_str("</p></body></html>");
    out
}

fn warning(
    notebook: &ResearchNotebookContract,
    code: ResearchNotebookWarningCode,
    message: &str,
) -> ResearchNotebookWarning {
    ResearchNotebookWarning {
        code,
        severity: ResearchNotebookWarningSeverity::Error,
        message: message.to_string(),
        replay_ref: notebook
            .replay_refs
            .first()
            .map(|replay| replay.replay_ref.clone())
            .unwrap_or_else(|| {
                format!(
                    "tradeassembly://research-notebook/{}/replay",
                    notebook.notebook_id
                )
            }),
    }
}

fn provenance_missing(provenance: &ResearchNotebookSourceProvenance) -> bool {
    provenance.source.trim().is_empty() || provenance.source_ref.trim().is_empty()
}

fn refs_have_string(notebook: &ResearchNotebookContract, predicate: fn(&str) -> bool) -> bool {
    notebook
        .prompts
        .iter()
        .any(|prompt| predicate(&prompt.prompt_ref) || predicate(&prompt.content_hash))
        || notebook.tool_calls.iter().any(|call| {
            predicate(&call.tool_name)
                || predicate(&call.arguments_hash)
                || predicate(&call.output_ref)
                || predicate(&call.output_hash)
        })
        || notebook
            .inputs
            .iter()
            .any(|input| predicate(&input.ref_uri) || predicate(&input.content_hash))
        || notebook.data_windows.iter().any(|window| {
            predicate(&window.window_id) || window.dataset_refs.iter().any(|value| predicate(value))
        })
        || notebook
            .report_refs
            .iter()
            .any(|report| predicate(&report.report_ref) || predicate(&report.content_hash))
        || notebook
            .chart_refs
            .iter()
            .any(|chart| predicate(&chart.artifact_ref) || predicate(&chart.content_hash))
        || notebook
            .table_refs
            .iter()
            .any(|table| predicate(&table.artifact_ref) || predicate(&table.content_hash))
        || notebook
            .replay_refs
            .iter()
            .any(|replay| predicate(&replay.replay_ref))
}

fn contains_secret_key(metadata: &BTreeMap<String, Value>) -> bool {
    metadata
        .iter()
        .any(|(key, value)| secret_key(key) || value_contains_key_or_string(value, secret_key))
}

fn metadata_has_key_or_string(
    metadata: &BTreeMap<String, Value>,
    predicate: fn(&str) -> bool,
) -> bool {
    metadata
        .iter()
        .any(|(key, value)| predicate(key) || value_contains_key_or_string(value, predicate))
}

fn value_contains_key_or_string(value: &Value, predicate: fn(&str) -> bool) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| predicate(key) || value_contains_key_or_string(value, predicate)),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_key_or_string(value, predicate)),
        Value::String(value) => predicate(value),
        _ => false,
    }
}

fn secret_key(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("api_key")
        || normalized.contains("api_secret")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("token")
        || normalized.contains("oauth")
}

fn broker_account_indicator(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("broker_account_id")
        || normalized.contains("brokeraccountid")
        || normalized.contains("account_number")
        || normalized.contains("raw_account")
}

fn hosted_control_indicator(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("hosted://")
        || normalized.contains("hosted-control")
        || normalized.contains("private-hosted")
        || normalized.contains("production-kms")
        || normalized.contains("billing")
}

fn sensitive_payload_indicator(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("sensitive_payload")
        || normalized.contains("raw_payload")
        || normalized.contains("unredacted")
        || normalized.contains("credential")
}

fn contains_advice_or_prediction(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "should buy",
        "should sell",
        "must buy",
        "must sell",
        "you should",
        "we recommend",
        "activate now",
        "increase size",
        "decrease size",
        "increase allocation",
        "decrease allocation",
        "predicts profit",
        "guaranteed profit",
        "next winner",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notebook_contract_serializes_hashes_and_validates_clean_payload() {
        let notebook = sample_notebook();
        let serialized = serde_json::to_string(&notebook).expect("serialize notebook");
        let hash = research_notebook_hash(&notebook);

        assert!(notebook.validate().ok);
        assert!(serialized.contains("\"schemaVersion\":\"tradeassembly.research_notebook.v1\""));
        assert!(serialized.contains("\"strategyId\":\"strat_local_btc_demo\""));
        assert!(serialized.contains("\"kind\":\"deterministic_calculation\""));
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash, research_notebook_hash(&notebook));
        assert!(!serialized.contains("api_secret"));
        assert!(!serialized.contains("brokerAccountId"));
    }

    #[test]
    fn required_blocks_distinguish_explainability_kinds() {
        let kinds = required_explainable_block_kinds();

        for kind in [
            ResearchNotebookBlockKind::Observation,
            ResearchNotebookBlockKind::Assumption,
            ResearchNotebookBlockKind::DeterministicCalculation,
            ResearchNotebookBlockKind::ModelDerivedOutput,
            ResearchNotebookBlockKind::Limitation,
            ResearchNotebookBlockKind::UserAuthoredDecision,
        ] {
            assert!(kinds.contains(&kind), "{kind:?}");
        }
    }

    #[test]
    fn export_bundle_is_deterministic_and_public_safe() {
        let notebook = sample_notebook();

        let export = export_research_notebook(&notebook).expect("export clean notebook");
        let export_again =
            export_research_notebook(&notebook).expect("export clean notebook again");

        assert_eq!(export, export_again);
        assert_eq!(
            export.schema_version,
            RESEARCH_NOTEBOOK_EXPORT_SCHEMA_VERSION
        );
        assert!(export.json.contains("\"chartRefs\""));
        assert!(export.markdown.contains("## Explainable Blocks"));
        assert!(export.html.contains("<h2>Artifacts</h2>"));
        for blocked in [
            "api_secret",
            "brokerAccountId",
            "hosted-control",
            "raw_payload",
            "should buy",
            "should sell",
        ] {
            assert!(!export.json.contains(blocked), "{blocked}");
            assert!(!export.markdown.contains(blocked), "{blocked}");
            assert!(!export.html.contains(blocked), "{blocked}");
        }
    }

    #[test]
    fn validation_rejects_unsaved_prompts_secrets_hosted_controls_and_bad_windows() {
        let mut notebook = sample_notebook();
        notebook.prompts[0].explicitly_saved = false;
        notebook.provenance.raw_secrets_exposed = true;
        notebook.data_windows[0].ends_at_unix_ms = notebook.data_windows[0].starts_at_unix_ms - 1;
        notebook.replay_refs.clear();
        notebook
            .metadata
            .insert("api_secret".to_string(), json!("SHOULD_NOT_LEAK"));
        notebook
            .metadata
            .insert("broker_account_id".to_string(), json!("ACCOUNT-123"));
        notebook
            .metadata
            .insert("hosted-control".to_string(), json!("private-hosted-admin"));
        notebook
            .metadata
            .insert("raw_payload".to_string(), json!({"credential": "abc"}));

        let report = notebook.validate();
        let codes = report
            .warnings
            .iter()
            .map(|warning| warning.code.clone())
            .collect::<BTreeSet<_>>();
        let rendered = serde_json::to_string(&report).expect("serialize report");

        assert!(!report.ok);
        assert!(codes.contains(&ResearchNotebookWarningCode::UnsavedPromptRejected));
        assert!(codes.contains(&ResearchNotebookWarningCode::RawSecretRejected));
        assert!(codes.contains(&ResearchNotebookWarningCode::InvalidDataWindow));
        assert!(codes.contains(&ResearchNotebookWarningCode::MissingReplayRef));
        assert!(codes.contains(&ResearchNotebookWarningCode::BrokerAccountIdentifierRejected));
        assert!(codes.contains(&ResearchNotebookWarningCode::HostedControlRejected));
        assert!(codes.contains(&ResearchNotebookWarningCode::SensitivePayloadRejected));
        assert!(!rendered.contains("SHOULD_NOT_LEAK"));
        assert!(export_research_notebook(&notebook).is_err());
    }

    #[test]
    fn validation_rejects_predictive_or_advice_copy() {
        let mut notebook = sample_notebook();
        notebook.explainable_blocks[0].summary =
            "This predicts profit and you should buy now.".to_string();

        let report = notebook.validate();

        assert!(!report.ok);
        assert!(report.warnings.iter().any(|warning| {
            warning.code == ResearchNotebookWarningCode::PredictiveOrAdviceCopyRejected
        }));
    }

    #[test]
    fn notebook_notice_contains_no_action_advice_copy() {
        let notebook = sample_notebook();
        let serialized = serde_json::to_string(&notebook).expect("serialize notebook");

        assert!(serialized.contains("did not recommend trades"));
        for forbidden in [
            "should buy",
            "should sell",
            "activate now",
            "increase allocation",
        ] {
            assert!(
                !serialized.to_ascii_lowercase().contains(forbidden),
                "notebook emitted advice phrase: {forbidden}"
            );
        }
    }

    fn sample_notebook() -> ResearchNotebookContract {
        ResearchNotebookContract {
            schema_version: RESEARCH_NOTEBOOK_SCHEMA_VERSION.to_string(),
            notebook_id: "notebook_research_001".to_string(),
            strategy_id: "strat_local_btc_demo".to_string(),
            run_id: "research_run_001".to_string(),
            title: "BTC mean reversion research notebook".to_string(),
            created_at_unix_ms: 1_785_000_000_000,
            prompts: vec![ResearchNotebookPromptRef {
                prompt_ref: "tradeassembly://prompt/research_run_001/prompt_001".to_string(),
                role: "user".to_string(),
                content_hash: "sha256:prompt001".to_string(),
                explicitly_saved: true,
                redaction_status: ResearchNotebookRedactionStatus::Redacted,
                provenance: provenance("codex-session", "thread://local/research_run_001"),
            }],
            tool_calls: vec![
                ResearchNotebookToolCall {
                    call_id: "tool_backtest_001".to_string(),
                    tool_name: "tradeassembly.backtest.run".to_string(),
                    tool_kind: ResearchNotebookToolKind::Backtest,
                    arguments_hash: "sha256:args001".to_string(),
                    output_ref: "tradeassembly://artifact/backtest_001".to_string(),
                    output_hash: "sha256:backtest001".to_string(),
                    redaction_status: ResearchNotebookRedactionStatus::Clean,
                    provenance: provenance("tradeassembly-runtime", "journal://tool/backtest_001"),
                },
                ResearchNotebookToolCall {
                    call_id: "tool_chart_001".to_string(),
                    tool_name: "tradeassembly.chart.render".to_string(),
                    tool_kind: ResearchNotebookToolKind::Other,
                    arguments_hash: "sha256:chartargs001".to_string(),
                    output_ref: "tradeassembly://artifact/chart_001".to_string(),
                    output_hash: "sha256:chart001".to_string(),
                    redaction_status: ResearchNotebookRedactionStatus::Clean,
                    provenance: provenance("tradeassembly-runtime", "journal://tool/chart_001"),
                },
            ],
            inputs: vec![ResearchNotebookInputRef {
                input_id: "input_dataset_001".to_string(),
                input_kind: "market_data_window".to_string(),
                ref_uri: "tradeassembly://dataset/btcusd/local-bars".to_string(),
                content_hash: "sha256:dataset001".to_string(),
                redaction_status: ResearchNotebookRedactionStatus::Clean,
                provenance: provenance("local-marketdata", "dataset://btcusd/local-bars"),
            }],
            data_windows: vec![ResearchNotebookDataWindow {
                window_id: "window_30d".to_string(),
                starts_at_unix_ms: 1_782_408_000_000,
                ends_at_unix_ms: 1_785_000_000_000,
                timezone: "UTC".to_string(),
                as_of_unix_ms: 1_785_000_000_000,
                stale_after_unix_ms: Some(1_785_086_400_000),
                dataset_refs: vec!["tradeassembly://dataset/btcusd/local-bars".to_string()],
                provenance: provenance("local-marketdata", "dataset://btcusd/local-bars"),
            }],
            assumptions: vec![ResearchNotebookAssumption {
                assumption_id: "assumption_001".to_string(),
                statement: "The research run used the user-selected 30 day data window."
                    .to_string(),
                source_ref: "tradeassembly://research/runs/research_run_001".to_string(),
                user_supplied: false,
            }],
            explainable_blocks: vec![
                ResearchNotebookExplainableBlock {
                    block_id: "block_observation_001".to_string(),
                    kind: ResearchNotebookBlockKind::Observation,
                    title: "Observed drawdown cluster".to_string(),
                    summary: "The run recorded clustered drawdowns during high-volatility bars."
                        .to_string(),
                    source_refs: vec!["tradeassembly://artifact/backtest_001".to_string()],
                    calculation_ref: None,
                    model_ref: None,
                    user_decision_ref: None,
                },
                ResearchNotebookExplainableBlock {
                    block_id: "block_calc_001".to_string(),
                    kind: ResearchNotebookBlockKind::DeterministicCalculation,
                    title: "Win rate calculation".to_string(),
                    summary:
                        "The win rate was calculated from completed simulated fills in the journal."
                            .to_string(),
                    source_refs: vec!["journal://research_run_001/fills".to_string()],
                    calculation_ref: Some("tradeassembly://calculation/win_rate_001".to_string()),
                    model_ref: None,
                    user_decision_ref: None,
                },
                ResearchNotebookExplainableBlock {
                    block_id: "block_model_001".to_string(),
                    kind: ResearchNotebookBlockKind::ModelDerivedOutput,
                    title: "Agent summary".to_string(),
                    summary: "The model summarized deterministic artifacts and listed limitations."
                        .to_string(),
                    source_refs: vec!["tradeassembly://artifact/backtest_001".to_string()],
                    calculation_ref: None,
                    model_ref: Some("model://active-thread".to_string()),
                    user_decision_ref: None,
                },
                ResearchNotebookExplainableBlock {
                    block_id: "block_user_001".to_string(),
                    kind: ResearchNotebookBlockKind::UserAuthoredDecision,
                    title: "User note".to_string(),
                    summary: "The user saved a note to inspect drawdown behavior later."
                        .to_string(),
                    source_refs: vec!["tradeassembly://note/user_note_001".to_string()],
                    calculation_ref: None,
                    model_ref: None,
                    user_decision_ref: Some("tradeassembly://note/user_note_001".to_string()),
                },
            ],
            report_refs: vec![ResearchNotebookReportRef {
                report_id: "report_backtest_001".to_string(),
                report_kind: "backtest".to_string(),
                report_ref: "tradeassembly://report/backtest_001".to_string(),
                content_hash: "sha256:report001".to_string(),
            }],
            chart_refs: vec![ResearchNotebookChartRef {
                chart_id: "chart_equity_001".to_string(),
                title: "Equity curve".to_string(),
                artifact_ref: "tradeassembly://artifact/chart_equity_001.svg".to_string(),
                content_hash: "sha256:chart001".to_string(),
            }],
            table_refs: vec![ResearchNotebookTableRef {
                table_id: "table_trades_001".to_string(),
                title: "Trade journal rows".to_string(),
                artifact_ref: "tradeassembly://artifact/trades_001.csv".to_string(),
                content_hash: "sha256:table001".to_string(),
            }],
            warnings: vec![],
            provenance: provenance(
                "tradeassembly-runtime",
                "journal://research_run_001/notebook",
            ),
            replay_refs: vec![ResearchNotebookReplayRef {
                replay_ref: "tradeassembly://research-notebook/notebook_research_001/replay"
                    .to_string(),
                deterministic: true,
            }],
            redaction_status: ResearchNotebookRedactionStatus::Redacted,
            metadata: BTreeMap::new(),
            no_advice_notice: RESEARCH_NOTEBOOK_NOTICE.to_string(),
        }
    }

    fn provenance(source: &str, source_ref: &str) -> ResearchNotebookSourceProvenance {
        ResearchNotebookSourceProvenance {
            source: source.to_string(),
            source_ref: source_ref.to_string(),
            collected_at_unix_ms: 1_785_000_000_000,
            raw_secrets_exposed: false,
        }
    }
}
