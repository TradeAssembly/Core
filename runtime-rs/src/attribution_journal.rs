// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const ATTRIBUTION_JOURNAL_SCHEMA_VERSION: &str = "tradeassembly.attribution_journal.v1";
pub const ATTRIBUTION_JOURNAL_NOTICE: &str = "Descriptive attribution and replay evidence only. TradeAssembly did not recommend trades, timing, sizing, allocation, activation, or strategy changes.";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionRefKind {
    StrategyVersion,
    SelectorRun,
    BacktestRun,
    LedgerCheckpoint,
    ExecutionEvent,
    ValuationSnapshot,
    UserNote,
    ReportArtifact,
    FillQualityReport,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalGroupingDimension {
    StrategyVersion,
    Underlying,
    Instrument,
    Leg,
    EntryDate,
    ExitDate,
    HoldingPeriodBucket,
    Outcome,
    SkipReason,
    RejectReason,
    FillQualityBucket,
    UserTag,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionWarningSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionWarningCode {
    UnsupportedSchema,
    MissingRef,
    MissingRequiredRefKind,
    StaleArtifact,
    HashMismatch,
    UnverifiedDataSource,
    IncompleteJournalState,
    IncompleteReplayState,
    RawSecretRejected,
    PredictiveOrAdviceCopyRejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionSourceProvenance {
    pub source: String,
    pub source_ref: String,
    pub collected_at_unix_ms: u64,
    pub raw_secrets_exposed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionStableRef {
    pub ref_id: String,
    pub ref_kind: AttributionRefKind,
    pub ref_uri: String,
    pub content_hash: String,
    pub observed_hash: Option<String>,
    pub as_of_unix_ms: u64,
    pub stale_after_unix_ms: Option<u64>,
    pub verified_data_source: bool,
    pub provenance: AttributionSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionEdge {
    pub edge_id: String,
    pub from_ref: String,
    pub to_ref: String,
    pub relation: String,
    pub evidence_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserNoteRef {
    pub note_id: String,
    pub note_ref: String,
    pub content_hash: String,
    pub explicitly_saved: bool,
    pub created_at_unix_ms: u64,
    pub provenance: AttributionSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PnlAttributionInput {
    pub input_id: String,
    pub basis: String,
    pub currency: String,
    pub gross_pnl: f64,
    pub fees: f64,
    pub net_pnl: f64,
    pub quantity: f64,
    pub strategy_ref: String,
    pub execution_ref: String,
    pub valuation_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinLossSummary {
    pub total_closed: u64,
    pub wins: u64,
    pub losses: u64,
    pub flat: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrawdownInput {
    pub input_id: String,
    pub equity_curve_ref: String,
    pub content_hash: String,
    pub peak_value: f64,
    pub trough_value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldingPeriodSummary {
    pub bucket: String,
    pub trade_count: u64,
    pub average_holding_ms: u64,
    pub net_pnl: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipRejectSummary {
    pub reason_code: String,
    pub reason_kind: String,
    pub count: u64,
    pub source_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalAnalyticsOutput {
    #[serde(default)]
    pub grouping_dimensions: Vec<JournalGroupingDimension>,
    #[serde(default)]
    pub pnl_inputs: Vec<PnlAttributionInput>,
    pub win_loss_summary: WinLossSummary,
    #[serde(default)]
    pub drawdown_inputs: Vec<DrawdownInput>,
    #[serde(default)]
    pub holding_period_summaries: Vec<HoldingPeriodSummary>,
    #[serde(default)]
    pub skip_reject_summaries: Vec<SkipRejectSummary>,
    #[serde(default)]
    pub fill_quality_refs: Vec<String>,
    pub provenance: AttributionSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRef {
    pub code_ref: String,
    pub commit_sha: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactHashRef {
    pub artifact_ref: String,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSnapshotRef {
    pub snapshot_ref: String,
    pub content_hash: String,
    pub as_of_unix_ms: u64,
    pub stale_after_unix_ms: Option<u64>,
    pub verified_data_source: bool,
    pub provenance: AttributionSourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayRef {
    pub replay_ref: String,
    pub deterministic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayReviewContract {
    pub replay_id: String,
    pub reproduced_result_ref: String,
    #[serde(default)]
    pub input_refs: Vec<String>,
    #[serde(default)]
    pub code_refs: Vec<CodeRef>,
    #[serde(default)]
    pub artifact_hashes: Vec<ArtifactHashRef>,
    #[serde(default)]
    pub data_snapshots: Vec<DataSnapshotRef>,
    pub user_settings_hash: String,
    pub deterministic: bool,
    pub reproduced_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionWarning {
    pub code: AttributionWarningCode,
    pub severity: AttributionWarningSeverity,
    pub message: String,
    pub replay_ref: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionJournalContract {
    pub schema_version: String,
    pub review_id: String,
    pub strategy_id: String,
    pub generated_at_unix_ms: u64,
    #[serde(default)]
    pub refs: Vec<AttributionStableRef>,
    #[serde(default)]
    pub attribution_edges: Vec<AttributionEdge>,
    #[serde(default)]
    pub user_notes: Vec<UserNoteRef>,
    pub analytics: JournalAnalyticsOutput,
    pub replay_review: ReplayReviewContract,
    #[serde(default)]
    pub warnings: Vec<AttributionWarning>,
    #[serde(default)]
    pub replay_refs: Vec<ReplayRef>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    pub no_advice_notice: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionValidationReport {
    pub ok: bool,
    #[serde(default)]
    pub warnings: Vec<AttributionWarning>,
}

impl AttributionJournalContract {
    pub fn validate(&self) -> AttributionValidationReport {
        validate_attribution_journal(self)
    }
}

pub fn validate_attribution_journal(
    contract: &AttributionJournalContract,
) -> AttributionValidationReport {
    let mut warnings = contract.warnings.clone();
    validate_required_fields(contract, &mut warnings);
    validate_ref_graph(contract, &mut warnings);
    validate_artifact_freshness_and_hashes(contract, &mut warnings);
    validate_analytics(contract, &mut warnings);
    validate_replay_review(contract, &mut warnings);
    validate_secret_surfaces(contract, &mut warnings);
    validate_copy_boundary(contract, &mut warnings);
    let ok = warnings
        .iter()
        .all(|warning| warning.severity != AttributionWarningSeverity::Error);
    AttributionValidationReport { ok, warnings }
}

pub fn attribution_journal_hash(contract: &AttributionJournalContract) -> String {
    let bytes = serde_json::to_vec(contract).expect("attribution journal serializes");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub fn required_attribution_ref_kinds() -> BTreeSet<AttributionRefKind> {
    BTreeSet::from([
        AttributionRefKind::StrategyVersion,
        AttributionRefKind::SelectorRun,
        AttributionRefKind::BacktestRun,
        AttributionRefKind::LedgerCheckpoint,
        AttributionRefKind::ExecutionEvent,
        AttributionRefKind::ValuationSnapshot,
        AttributionRefKind::UserNote,
        AttributionRefKind::ReportArtifact,
        AttributionRefKind::FillQualityReport,
    ])
}

fn validate_required_fields(
    contract: &AttributionJournalContract,
    warnings: &mut Vec<AttributionWarning>,
) {
    if contract.schema_version != ATTRIBUTION_JOURNAL_SCHEMA_VERSION {
        warnings.push(warning(
            contract,
            AttributionWarningCode::UnsupportedSchema,
            "Attribution journal schema version is unsupported.",
        ));
    }
    if contract.review_id.trim().is_empty() || contract.strategy_id.trim().is_empty() {
        warnings.push(warning(
            contract,
            AttributionWarningCode::MissingRef,
            "Attribution journal must be scoped to a review and strategy.",
        ));
    }
    if contract.replay_refs.is_empty()
        || contract
            .replay_refs
            .iter()
            .all(|replay| !replay.deterministic)
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::IncompleteReplayState,
            "Attribution journal must include at least one deterministic replay ref.",
        ));
    }
}

fn validate_ref_graph(
    contract: &AttributionJournalContract,
    warnings: &mut Vec<AttributionWarning>,
) {
    let present = contract
        .refs
        .iter()
        .map(|stable_ref| stable_ref.ref_kind.clone())
        .collect::<BTreeSet<_>>();
    for required in required_attribution_ref_kinds() {
        if !present.contains(&required) {
            warnings.push(warning(
                contract,
                AttributionWarningCode::MissingRequiredRefKind,
                "Attribution journal is missing a required strategy, research, ledger, execution, valuation, note, report, or fill-quality ref kind.",
            ));
            break;
        }
    }
    if contract.refs.iter().any(ref_missing) || contract.user_notes.iter().any(note_missing) {
        warnings.push(warning(
            contract,
            AttributionWarningCode::MissingRef,
            "Attribution journal contains an empty ref id, URI, hash, saved note, or provenance.",
        ));
    }
    let ref_ids = contract
        .refs
        .iter()
        .map(|stable_ref| stable_ref.ref_id.as_str())
        .collect::<BTreeSet<_>>();
    if contract.attribution_edges.is_empty()
        || contract.attribution_edges.iter().any(|edge| {
            edge.edge_id.trim().is_empty()
                || edge.from_ref.trim().is_empty()
                || edge.to_ref.trim().is_empty()
                || edge.relation.trim().is_empty()
                || !edge.evidence_hash.starts_with("sha256:")
                || !ref_ids.contains(edge.from_ref.as_str())
                || !ref_ids.contains(edge.to_ref.as_str())
        })
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::MissingRef,
            "Attribution journal edges must connect stable refs with evidence hashes.",
        ));
    }
}

fn validate_artifact_freshness_and_hashes(
    contract: &AttributionJournalContract,
    warnings: &mut Vec<AttributionWarning>,
) {
    if contract.refs.iter().any(|stable_ref| {
        stable_ref
            .stale_after_unix_ms
            .is_some_and(|stale_after| stable_ref.as_of_unix_ms > stale_after)
    }) || contract
        .replay_review
        .data_snapshots
        .iter()
        .any(|snapshot| {
            snapshot
                .stale_after_unix_ms
                .is_some_and(|stale_after| snapshot.as_of_unix_ms > stale_after)
        })
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::StaleArtifact,
            "Attribution journal includes stale artifact or data snapshot refs.",
        ));
    }
    if contract.refs.iter().any(|stable_ref| {
        stable_ref
            .observed_hash
            .as_ref()
            .is_some_and(|observed| observed != &stable_ref.content_hash)
    }) {
        warnings.push(warning(
            contract,
            AttributionWarningCode::HashMismatch,
            "Attribution journal ref hash mismatch prevents replay trust.",
        ));
    }
    if contract
        .refs
        .iter()
        .any(|stable_ref| !stable_ref.verified_data_source)
        || contract
            .replay_review
            .data_snapshots
            .iter()
            .any(|snapshot| !snapshot.verified_data_source)
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::UnverifiedDataSource,
            "Attribution journal contains unverified data source refs.",
        ));
    }
}

fn validate_analytics(
    contract: &AttributionJournalContract,
    warnings: &mut Vec<AttributionWarning>,
) {
    let analytics = &contract.analytics;
    if analytics.grouping_dimensions.is_empty()
        || analytics.pnl_inputs.is_empty()
        || analytics.drawdown_inputs.is_empty()
        || analytics.holding_period_summaries.is_empty()
        || analytics.skip_reject_summaries.is_empty()
        || analytics.fill_quality_refs.is_empty()
        || provenance_missing(&analytics.provenance)
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::IncompleteJournalState,
            "Journal analytics must include grouping dimensions, P&L inputs, drawdown inputs, holding periods, skip/reject summaries, fill-quality refs, and provenance.",
        ));
    }
    if analytics.pnl_inputs.iter().any(|input| {
        !input.gross_pnl.is_finite()
            || !input.net_pnl.is_finite()
            || !input.fees.is_finite()
            || !input.quantity.is_finite()
            || input.strategy_ref.trim().is_empty()
            || input.execution_ref.trim().is_empty()
    }) || analytics.drawdown_inputs.iter().any(|input| {
        !input.peak_value.is_finite()
            || !input.trough_value.is_finite()
            || !input.content_hash.starts_with("sha256:")
    }) || analytics
        .holding_period_summaries
        .iter()
        .any(|summary| !summary.net_pnl.is_finite() || summary.bucket.trim().is_empty())
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::IncompleteJournalState,
            "Journal analytics contains invalid numeric or ref-shaped inputs.",
        ));
    }
    if analytics.win_loss_summary.total_closed
        != analytics.win_loss_summary.wins
            + analytics.win_loss_summary.losses
            + analytics.win_loss_summary.flat
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::IncompleteJournalState,
            "Win/loss summary counts do not reconcile to total closed events.",
        ));
    }
}

fn validate_replay_review(
    contract: &AttributionJournalContract,
    warnings: &mut Vec<AttributionWarning>,
) {
    let replay = &contract.replay_review;
    if replay.replay_id.trim().is_empty()
        || replay.reproduced_result_ref.trim().is_empty()
        || replay.input_refs.is_empty()
        || replay.code_refs.is_empty()
        || replay.artifact_hashes.is_empty()
        || replay.data_snapshots.is_empty()
        || replay.user_settings_hash.trim().is_empty()
        || !replay.deterministic
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::IncompleteReplayState,
            "Replay review must identify inputs, code refs, artifact hashes, data snapshots, user settings, and deterministic reproduction state.",
        ));
    }
    if replay.code_refs.iter().any(|code_ref| {
        code_ref.code_ref.trim().is_empty()
            || code_ref.commit_sha.trim().is_empty()
            || !code_ref.content_hash.starts_with("sha256:")
    }) || replay.artifact_hashes.iter().any(|artifact| {
        artifact.artifact_ref.trim().is_empty() || !artifact.content_hash.starts_with("sha256:")
    }) || replay.data_snapshots.iter().any(|snapshot| {
        snapshot.snapshot_ref.trim().is_empty()
            || !snapshot.content_hash.starts_with("sha256:")
            || provenance_missing(&snapshot.provenance)
    }) {
        warnings.push(warning(
            contract,
            AttributionWarningCode::IncompleteReplayState,
            "Replay review contains incomplete code, artifact, or data snapshot refs.",
        ));
    }
}

fn validate_secret_surfaces(
    contract: &AttributionJournalContract,
    warnings: &mut Vec<AttributionWarning>,
) {
    if contains_secret_key(&contract.metadata)
        || contract.refs.iter().any(|stable_ref| {
            stable_ref.provenance.raw_secrets_exposed
                || secret_like(&stable_ref.ref_uri)
                || secret_like(&stable_ref.provenance.source_ref)
        })
        || contract
            .user_notes
            .iter()
            .any(|note| note.provenance.raw_secrets_exposed || secret_like(&note.note_ref))
        || contract.analytics.provenance.raw_secrets_exposed
        || contract
            .replay_review
            .data_snapshots
            .iter()
            .any(|snapshot| {
                snapshot.provenance.raw_secrets_exposed || secret_like(&snapshot.snapshot_ref)
            })
    {
        warnings.push(warning(
            contract,
            AttributionWarningCode::RawSecretRejected,
            "Attribution journal contains raw secret-shaped refs or metadata.",
        ));
    }
}

fn validate_copy_boundary(
    contract: &AttributionJournalContract,
    warnings: &mut Vec<AttributionWarning>,
) {
    let edge_copy_has_advice = contract
        .attribution_edges
        .iter()
        .any(|edge| contains_advice_or_prediction(&edge.relation));
    let reason_copy_has_advice = contract
        .analytics
        .skip_reject_summaries
        .iter()
        .any(|summary| {
            contains_advice_or_prediction(&summary.reason_code)
                || contains_advice_or_prediction(&summary.reason_kind)
        });
    let warning_copy_has_advice = contract
        .warnings
        .iter()
        .any(|warning| contains_advice_or_prediction(&warning.message));

    if edge_copy_has_advice || reason_copy_has_advice || warning_copy_has_advice {
        warnings.push(warning(
            contract,
            AttributionWarningCode::PredictiveOrAdviceCopyRejected,
            "Attribution and journal analytics copy must stay descriptive and non-predictive.",
        ));
    }
}

fn ref_missing(stable_ref: &AttributionStableRef) -> bool {
    stable_ref.ref_id.trim().is_empty()
        || stable_ref.ref_uri.trim().is_empty()
        || !stable_ref.content_hash.starts_with("sha256:")
        || provenance_missing(&stable_ref.provenance)
}

fn note_missing(note: &UserNoteRef) -> bool {
    note.note_id.trim().is_empty()
        || note.note_ref.trim().is_empty()
        || !note.content_hash.starts_with("sha256:")
        || !note.explicitly_saved
        || provenance_missing(&note.provenance)
}

fn provenance_missing(provenance: &AttributionSourceProvenance) -> bool {
    provenance.source.trim().is_empty() || provenance.source_ref.trim().is_empty()
}

fn warning(
    contract: &AttributionJournalContract,
    code: AttributionWarningCode,
    message: &str,
) -> AttributionWarning {
    AttributionWarning {
        code,
        severity: AttributionWarningSeverity::Error,
        message: message.to_string(),
        replay_ref: contract
            .replay_refs
            .first()
            .map(|replay| replay.replay_ref.clone())
            .unwrap_or_else(|| {
                format!("tradeassembly://attribution/{}/replay", contract.review_id)
            }),
    }
}

fn contains_secret_key(metadata: &BTreeMap<String, Value>) -> bool {
    metadata
        .iter()
        .any(|(key, value)| secret_like(key) || value_contains_secret_key(value))
}

fn value_contains_secret_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| secret_like(key) || value_contains_secret_key(value)),
        Value::Array(values) => values.iter().any(value_contains_secret_key),
        Value::String(value) => secret_like(value),
        _ => false,
    }
}

fn secret_like(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    normalized.contains("api_key")
        || normalized.contains("api_secret")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("token")
        || normalized.contains("broker_account_id")
}

fn contains_advice_or_prediction(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "should buy",
        "should sell",
        "should enter",
        "should exit",
        "increase size",
        "reduce size",
        "must trade",
        "recommend trade",
        "recommend sizing",
        "predicts profit",
        "forecast profit",
        "guaranteed",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attribution_journal_serializes_deterministically_and_links_required_refs() {
        let contract = sample_contract();

        let serialized = serde_json::to_string(&contract).expect("serialize attribution journal");
        let hash = attribution_journal_hash(&contract);
        let ref_kinds = contract
            .refs
            .iter()
            .map(|stable_ref| stable_ref.ref_kind.clone())
            .collect::<BTreeSet<_>>();

        assert!(serialized.contains("\"schemaVersion\":\"tradeassembly.attribution_journal.v1\""));
        assert!(serialized.contains("\"groupingDimensions\""));
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash, attribution_journal_hash(&contract));
        assert!(contract.validate().ok);
        for required in required_attribution_ref_kinds() {
            assert!(ref_kinds.contains(&required), "{required:?}");
        }
    }

    #[test]
    fn analytics_output_contains_expected_trader_review_inputs() {
        let analytics = sample_contract().analytics;

        assert!(analytics
            .grouping_dimensions
            .contains(&JournalGroupingDimension::StrategyVersion));
        assert!(analytics
            .grouping_dimensions
            .contains(&JournalGroupingDimension::SkipReason));
        assert_eq!(analytics.pnl_inputs[0].net_pnl, 74.4);
        assert_eq!(analytics.win_loss_summary.total_closed, 1);
        assert_eq!(
            analytics.drawdown_inputs[0].equity_curve_ref,
            "artifact://equity-curve"
        );
        assert_eq!(analytics.holding_period_summaries[0].bucket, "0-1d");
        assert_eq!(analytics.skip_reject_summaries[0].reason_kind, "skip");
        assert_eq!(analytics.fill_quality_refs[0], "fill_quality_report_001");
    }

    #[test]
    fn replay_review_identifies_reproducible_inputs_code_artifacts_data_and_settings() {
        let replay = sample_contract().replay_review;

        assert!(replay.deterministic);
        assert_eq!(replay.input_refs, vec!["strategy_version_001"]);
        assert_eq!(
            replay.code_refs[0].commit_sha,
            "d9541f47123b31c643b92c6681d743d62d7a0ba4"
        );
        assert_eq!(
            replay.artifact_hashes[0].artifact_ref,
            "report://backtest-001"
        );
        assert_eq!(
            replay.data_snapshots[0].snapshot_ref,
            "snapshot://spy-2026-06-01"
        );
        assert!(replay.user_settings_hash.starts_with("sha256:"));
    }

    #[test]
    fn warning_cases_fail_closed_without_echoing_secret_values() {
        let mut contract = sample_contract();
        contract.schema_version = "old".to_string();
        contract.refs.retain(|stable_ref| {
            stable_ref.ref_kind != AttributionRefKind::ExecutionEvent
                && stable_ref.ref_kind != AttributionRefKind::UserNote
        });
        contract.refs[0].content_hash = "sha256:expected".to_string();
        contract.refs[0].observed_hash = Some("sha256:observed".to_string());
        contract.refs[0].as_of_unix_ms = 2_000;
        contract.refs[0].stale_after_unix_ms = Some(1_999);
        contract.refs[0].verified_data_source = false;
        contract.refs[0].provenance.raw_secrets_exposed = true;
        contract.user_notes[0].explicitly_saved = false;
        contract.analytics.grouping_dimensions = vec![];
        contract.analytics.win_loss_summary.losses = 1;
        contract.replay_review.deterministic = false;
        contract.replay_review.code_refs = vec![];
        contract
            .metadata
            .insert("api_secret".to_string(), json!("dont-print-this"));

        let report = contract.validate();
        let codes = report
            .warnings
            .iter()
            .map(|warning| warning.code.clone())
            .collect::<BTreeSet<_>>();
        let serialized = serde_json::to_string(&report).expect("serialize warning report");

        assert!(!report.ok);
        assert!(codes.contains(&AttributionWarningCode::UnsupportedSchema));
        assert!(codes.contains(&AttributionWarningCode::MissingRequiredRefKind));
        assert!(codes.contains(&AttributionWarningCode::MissingRef));
        assert!(codes.contains(&AttributionWarningCode::StaleArtifact));
        assert!(codes.contains(&AttributionWarningCode::HashMismatch));
        assert!(codes.contains(&AttributionWarningCode::UnverifiedDataSource));
        assert!(codes.contains(&AttributionWarningCode::IncompleteJournalState));
        assert!(codes.contains(&AttributionWarningCode::IncompleteReplayState));
        assert!(codes.contains(&AttributionWarningCode::RawSecretRejected));
        assert!(!serialized.contains("dont-print-this"));
    }

    #[test]
    fn advice_and_prediction_copy_is_rejected() {
        let mut contract = sample_contract();
        contract.attribution_edges[0].relation =
            "this predicts profit and should buy more contracts".to_string();

        let report = contract.validate();

        assert!(!report.ok);
        assert!(report.warnings.iter().any(|warning| {
            warning.code == AttributionWarningCode::PredictiveOrAdviceCopyRejected
        }));
    }

    #[test]
    fn notice_contains_no_action_advice_copy() {
        let serialized =
            serde_json::to_string(&sample_contract()).expect("serialize attribution journal");
        let lower = serialized.to_ascii_lowercase();

        assert!(serialized.contains("did not recommend trades"));
        assert!(!lower.contains("should buy"));
        assert!(!lower.contains("should sell"));
        assert!(!lower.contains("should enter"));
        assert!(!lower.contains("should exit"));
        assert!(!lower.contains("recommend strategy changes"));
    }

    fn sample_contract() -> AttributionJournalContract {
        let refs = vec![
            stable_ref("strategy_version_001", AttributionRefKind::StrategyVersion),
            stable_ref("selector_run_001", AttributionRefKind::SelectorRun),
            stable_ref("backtest_run_001", AttributionRefKind::BacktestRun),
            stable_ref(
                "ledger_checkpoint_001",
                AttributionRefKind::LedgerCheckpoint,
            ),
            stable_ref("execution_event_001", AttributionRefKind::ExecutionEvent),
            stable_ref(
                "valuation_snapshot_001",
                AttributionRefKind::ValuationSnapshot,
            ),
            stable_ref("user_note_001", AttributionRefKind::UserNote),
            stable_ref("report_artifact_001", AttributionRefKind::ReportArtifact),
            stable_ref(
                "fill_quality_report_001",
                AttributionRefKind::FillQualityReport,
            ),
        ];
        AttributionJournalContract {
            schema_version: ATTRIBUTION_JOURNAL_SCHEMA_VERSION.to_string(),
            review_id: "attribution_review_001".to_string(),
            strategy_id: "strat_local_btc_demo".to_string(),
            generated_at_unix_ms: 1_785_900_000_000,
            attribution_edges: vec![
                AttributionEdge {
                    edge_id: "edge_strategy_selector".to_string(),
                    from_ref: "strategy_version_001".to_string(),
                    to_ref: "selector_run_001".to_string(),
                    relation: "strategy version produced selector run".to_string(),
                    evidence_hash: "sha256:edge001".to_string(),
                },
                AttributionEdge {
                    edge_id: "edge_execution_fill_quality".to_string(),
                    from_ref: "execution_event_001".to_string(),
                    to_ref: "fill_quality_report_001".to_string(),
                    relation: "execution event produced fill quality report".to_string(),
                    evidence_hash: "sha256:edge002".to_string(),
                },
            ],
            refs,
            user_notes: vec![UserNoteRef {
                note_id: "user_note_001".to_string(),
                note_ref: "note://local/user-note-001".to_string(),
                content_hash: "sha256:usernote001".to_string(),
                explicitly_saved: true,
                created_at_unix_ms: 1_785_800_000_000,
                provenance: provenance("user_note"),
            }],
            analytics: JournalAnalyticsOutput {
                grouping_dimensions: vec![
                    JournalGroupingDimension::StrategyVersion,
                    JournalGroupingDimension::Underlying,
                    JournalGroupingDimension::SkipReason,
                    JournalGroupingDimension::FillQualityBucket,
                ],
                pnl_inputs: vec![PnlAttributionInput {
                    input_id: "pnl_input_001".to_string(),
                    basis: "realized".to_string(),
                    currency: "USD".to_string(),
                    gross_pnl: 82.0,
                    fees: 7.6,
                    net_pnl: 74.4,
                    quantity: 1.0,
                    strategy_ref: "strategy_version_001".to_string(),
                    execution_ref: "execution_event_001".to_string(),
                    valuation_ref: Some("valuation_snapshot_001".to_string()),
                }],
                win_loss_summary: WinLossSummary {
                    total_closed: 1,
                    wins: 1,
                    losses: 0,
                    flat: 0,
                },
                drawdown_inputs: vec![DrawdownInput {
                    input_id: "drawdown_001".to_string(),
                    equity_curve_ref: "artifact://equity-curve".to_string(),
                    content_hash: "sha256:equitycurve001".to_string(),
                    peak_value: 10_100.0,
                    trough_value: 10_025.0,
                }],
                holding_period_summaries: vec![HoldingPeriodSummary {
                    bucket: "0-1d".to_string(),
                    trade_count: 1,
                    average_holding_ms: 3_600_000,
                    net_pnl: 74.4,
                }],
                skip_reject_summaries: vec![SkipRejectSummary {
                    reason_code: "liquidity_filter".to_string(),
                    reason_kind: "skip".to_string(),
                    count: 2,
                    source_refs: vec!["selector_run_001".to_string()],
                }],
                fill_quality_refs: vec!["fill_quality_report_001".to_string()],
                provenance: provenance("journal_analytics"),
            },
            replay_review: ReplayReviewContract {
                replay_id: "replay_review_001".to_string(),
                reproduced_result_ref: "report_artifact_001".to_string(),
                input_refs: vec!["strategy_version_001".to_string()],
                code_refs: vec![CodeRef {
                    code_ref: "git://TradeAssembly/runtime-rs/src/attribution_journal.rs"
                        .to_string(),
                    commit_sha: "d9541f47123b31c643b92c6681d743d62d7a0ba4".to_string(),
                    content_hash: "sha256:coderef001".to_string(),
                }],
                artifact_hashes: vec![ArtifactHashRef {
                    artifact_ref: "report://backtest-001".to_string(),
                    content_hash: "sha256:backtestreport001".to_string(),
                }],
                data_snapshots: vec![DataSnapshotRef {
                    snapshot_ref: "snapshot://spy-2026-06-01".to_string(),
                    content_hash: "sha256:datasnapshot001".to_string(),
                    as_of_unix_ms: 1_785_000_000_000,
                    stale_after_unix_ms: Some(1_786_000_000_000),
                    verified_data_source: true,
                    provenance: provenance("data_snapshot"),
                }],
                user_settings_hash: "sha256:usersettings001".to_string(),
                deterministic: true,
                reproduced_at_unix_ms: 1_785_900_000_000,
            },
            warnings: vec![],
            replay_refs: vec![ReplayRef {
                replay_ref: "tradeassembly://attribution/attribution_review_001/replay".to_string(),
                deterministic: true,
            }],
            metadata: BTreeMap::new(),
            no_advice_notice: ATTRIBUTION_JOURNAL_NOTICE.to_string(),
        }
    }

    fn stable_ref(ref_id: &str, ref_kind: AttributionRefKind) -> AttributionStableRef {
        AttributionStableRef {
            ref_id: ref_id.to_string(),
            ref_kind,
            ref_uri: format!("tradeassembly://refs/{ref_id}"),
            content_hash: format!("sha256:{ref_id}"),
            observed_hash: Some(format!("sha256:{ref_id}")),
            as_of_unix_ms: 1_785_000_000_000,
            stale_after_unix_ms: Some(1_786_000_000_000),
            verified_data_source: true,
            provenance: provenance(ref_id),
        }
    }

    fn provenance(source_ref: &str) -> AttributionSourceProvenance {
        AttributionSourceProvenance {
            source: "local-tradeassembly".to_string(),
            source_ref: format!("journal://{source_ref}"),
            collected_at_unix_ms: 1_785_000_000_000,
            raw_secrets_exposed: false,
        }
    }
}
