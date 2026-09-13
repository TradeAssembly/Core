//! Pure, immutable comparison contracts for completed research runs.
//!
//! This module deliberately has no persistence or control-plane dependency.

use crate::backtest_contracts::canonical_hash;
use crate::backtest_report::BacktestReport;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const COMPARISON_SCHEMA: &str = "tradeassembly.research-comparison.v1";
pub const COMPARISON_EXPORT_SCHEMA: &str = "tradeassembly.research-comparison-export.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ComparisonPolicy {
    Block,
    Warn,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompatibilityPolicy {
    pub currency: ComparisonPolicy,
    pub instrument_scope: ComparisonPolicy,
    pub dataset_range: ComparisonPolicy,
    pub strategy_version: ComparisonPolicy,
    pub accounting: ComparisonPolicy,
    pub fill: ComparisonPolicy,
    pub cost: ComparisonPolicy,
    pub study_semantics: ComparisonPolicy,
}

impl Default for CompatibilityPolicy {
    fn default() -> Self {
        Self {
            currency: ComparisonPolicy::Block,
            instrument_scope: ComparisonPolicy::Warn,
            dataset_range: ComparisonPolicy::Warn,
            strategy_version: ComparisonPolicy::Warn,
            accounting: ComparisonPolicy::Block,
            fill: ComparisonPolicy::Block,
            cost: ComparisonPolicy::Block,
            study_semantics: ComparisonPolicy::Block,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonScope {
    pub strategy_id: String,
    pub allow_cross_version: bool,
    pub compatible_strategy_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonManifest {
    pub schema: String,
    pub selected_run_hashes: Vec<String>,
    pub scope: ComparisonScope,
    pub policy: CompatibilityPolicy,
    pub manifest_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EquityPoint {
    pub timestamp: String,
    pub absolute_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DrawdownPoint {
    pub timestamp: String,
    pub drawdown_bps: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RobustnessComparisonResult {
    pub run_hash: String,
    pub report_hash: String,
    pub result_hash: String,
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub reporting_currency: String,
    pub dataset_hash: String,
    pub dataset_range: Value,
    pub instruments: Vec<String>,
    pub accounting: Value,
    pub execution: Value,
    pub costs: Value,
    pub study_semantics: Value,
    pub metrics: BTreeMap<String, MetricValue>,
    pub unavailable: BTreeMap<String, String>,
    pub equity: Vec<EquityPoint>,
    pub drawdown: Vec<DrawdownPoint>,
    pub trade_count: Option<u64>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MetricValue {
    pub value: i64,
    pub unit: MetricUnit,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum MetricUnit {
    CurrencyMicros,
    BasisPoints,
    Count,
    QuantityMicros,
    RatioMicros,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum ComparisonInput {
    Backtest(Box<BacktestReport>),
    Robustness(Box<RobustnessComparisonResult>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum CompatibilityDisposition {
    Compatible,
    Warning,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssumptionDiff {
    pub field: String,
    pub values_by_run: BTreeMap<String, Value>,
    pub disposition: CompatibilityDisposition,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlignedMetric {
    pub metric: String,
    pub unit: MetricUnit,
    pub values_by_run: BTreeMap<String, Option<i64>>,
    pub unavailable: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overlay<T> {
    pub values_by_run: BTreeMap<String, Vec<T>>,
    pub unavailable: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonArtifact {
    pub schema: String,
    pub comparison_hash: String,
    pub manifest_hash: String,
    pub source_hashes: BTreeMap<String, SourceHashes>,
    pub assumptions_diff: Vec<AssumptionDiff>,
    pub aligned_metrics: Vec<AlignedMetric>,
    pub absolute_equity: Overlay<EquityPoint>,
    pub normalized_equity_bps: Overlay<EquityPoint>,
    pub drawdown: Overlay<DrawdownPoint>,
    pub trade_summary: Vec<AlignedMetric>,
    pub risk_summary: Vec<AlignedMetric>,
    pub cost_summary: Vec<AlignedMetric>,
    pub diagnostics: BTreeMap<String, Vec<String>>,
    pub blocked: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceHashes {
    pub run_hash: String,
    pub report_hash: String,
    pub result_hash: String,
    pub dataset_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComparisonExport {
    pub schema: String,
    pub content_hash: String,
    pub content: String,
}

impl ComparisonManifest {
    pub fn build(
        mut selected_run_hashes: Vec<String>,
        scope: ComparisonScope,
        policy: CompatibilityPolicy,
    ) -> Result<Self, String> {
        validate_run_hashes(&selected_run_hashes)?;
        selected_run_hashes.sort();
        let mut manifest = Self {
            schema: COMPARISON_SCHEMA.to_string(),
            selected_run_hashes,
            scope,
            policy,
            manifest_hash: String::new(),
        };
        manifest.manifest_hash = canonical_hash(
            &manifest_without_hash(&manifest),
            "comparison_manifest_hash_failed",
        )?;
        Ok(manifest)
    }
}

pub fn compare(
    manifest: &ComparisonManifest,
    inputs: &[ComparisonInput],
) -> Result<ComparisonArtifact, String> {
    validate_manifest(manifest)?;
    if inputs.len() != manifest.selected_run_hashes.len() {
        return Err("comparison_selected_runs_mismatch".to_string());
    }
    let mut runs: BTreeMap<String, RunView> = BTreeMap::new();
    for input in inputs {
        let view = RunView::from_input(input)?;
        if runs.insert(view.run_hash.clone(), view).is_some() {
            return Err("comparison_duplicate_run".to_string());
        }
    }
    if runs.keys().cloned().collect::<Vec<_>>() != manifest.selected_run_hashes {
        return Err("comparison_selected_runs_mismatch".to_string());
    }
    validate_scope(&manifest.scope, runs.values())?;
    let diffs = assumption_diffs(&runs, &manifest.policy);
    let blocked = diffs
        .iter()
        .any(|diff| diff.disposition == CompatibilityDisposition::Blocked);
    let warnings = diffs
        .iter()
        .filter(|diff| diff.disposition == CompatibilityDisposition::Warning)
        .map(|diff| format!("comparison_warning:{}", diff.field))
        .collect();
    let source_hashes = runs
        .iter()
        .map(|(id, run)| (id.clone(), run.source_hashes()))
        .collect();
    let aligned_metrics = align_metrics(&runs, blocked);
    let (absolute_equity, normalized_equity_bps, drawdown) = overlays(&runs, blocked);
    let mut artifact = ComparisonArtifact {
        schema: COMPARISON_SCHEMA.to_string(),
        comparison_hash: String::new(),
        manifest_hash: manifest.manifest_hash.clone(),
        source_hashes,
        assumptions_diff: diffs,
        aligned_metrics: aligned_metrics.clone(),
        absolute_equity,
        normalized_equity_bps,
        drawdown,
        trade_summary: select_metrics(
            &aligned_metrics,
            &["trade_count", "win_count", "loss_count", "open_count"],
        ),
        risk_summary: select_metrics(
            &aligned_metrics,
            &[
                "maximum_loss",
                "maximum_position_notional",
                "maximum_order_quantity",
                "max_drawdown",
            ],
        ),
        cost_summary: select_metrics(&aligned_metrics, &["fees", "cost_drag"]),
        diagnostics: runs
            .iter()
            .map(|(id, run)| (id.clone(), run.diagnostics.clone()))
            .collect(),
        blocked,
        warnings,
    };
    artifact.comparison_hash = canonical_hash(
        &artifact_without_hash(&artifact),
        "comparison_artifact_hash_failed",
    )?;
    Ok(artifact)
}

pub fn export_json(artifact: &ComparisonArtifact) -> Result<ComparisonExport, String> {
    let content = String::from_utf8(
        serde_json_canonicalizer::to_vec(artifact)
            .map_err(|_| "comparison_export_serialization_failed".to_string())?,
    )
    .map_err(|_| "comparison_export_serialization_failed".to_string())?;
    Ok(ComparisonExport {
        schema: COMPARISON_EXPORT_SCHEMA.to_string(),
        content_hash: format!("sha256:{:x}", Sha256::digest(content.as_bytes())),
        content,
    })
}

/// Durable application service for immutable comparisons.
///
/// The pure contracts above intentionally do not know where a completed run is
/// stored. This adapter binds only completed, integrity-verified backtests or
/// robustness studies,
/// persists the immutable manifest, result, and exports through `StoragePort`,
/// and leaves control-plane routing to the caller.
pub mod durable {
    use super::{
        canonical_hash, compare, export_json, ComparisonArtifact, ComparisonInput,
        ComparisonManifest, ComparisonScope, CompatibilityPolicy, MetricUnit, MetricValue,
        RobustnessComparisonResult, COMPARISON_EXPORT_SCHEMA,
    };
    use crate::backtest_contracts::{BacktestRunManifest, BacktestRunRecord};
    use crate::backtest_report::{self, BacktestReport};
    use crate::domain::{BacktestRunState, RobustnessRunState};
    use crate::historical_data::DatasetSnapshot;
    use crate::ports::{
        BacktestRunRepository, DatasetSnapshotRepository, IdempotencyKey, ImmutablePutOutcome,
        SideEffectContext, StoragePort,
    };
    use crate::robustness_contracts::{
        RobustnessResult, RobustnessRunManifest, RobustnessRunRecord, ROBUSTNESS_RESULT_SCHEMA,
        ROBUSTNESS_RUN_SCHEMA,
    };
    use crate::robustness_engine::{
        self, Metric as RobustnessMetric, RobustnessResult as EngineRobustnessResult, StudyOutput,
        Unit,
    };
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};

    const MANIFESTS_NS: &str = "research_comparison_manifests_v1";
    const ARTIFACTS_NS: &str = "research_comparison_artifacts_v1";
    const EXPORTS_NS: &str = "research_comparison_exports_v1";
    const IDEMPOTENCY_NS: &str = "research_comparison_idempotency_v1";
    const ROBUSTNESS_MANIFESTS_NS: &str = "robustness_manifests_v1";
    const ROBUSTNESS_RESULTS_NS: &str = "robustness_results_v1";
    const ROBUSTNESS_RUNS_NS: &str = "robustness_runs_v1";
    const DURABLE_MANIFEST_SCHEMA: &str = "tradeassembly.research-comparison-manifest.v1";
    const DURABLE_ARTIFACT_SCHEMA: &str = "tradeassembly.research-comparison-artifact.v1";

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ComparisonSourceKind {
        Backtest,
        Robustness,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct ComparisonSourceBinding {
        pub kind: ComparisonSourceKind,
        pub source_run_id: String,
        pub run_hash: String,
        pub manifest_hash: String,
        pub result_hash: String,
        pub report_hash: String,
        pub dataset_hash: String,
        pub strategy_id: String,
        pub strategy_version_id: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct ComparisonManifestContent {
        pub schema: String,
        pub comparison_id: String,
        pub request_hash: String,
        pub idempotency_key: String,
        pub sources: Vec<ComparisonSourceBinding>,
        pub scope: ComparisonScope,
        pub policy: CompatibilityPolicy,
        pub engine_manifest: ComparisonManifest,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct DurableComparisonManifest {
        pub manifest_hash: String,
        pub manifest_ref: String,
        pub byte_length: u64,
        pub created_at_ms: i64,
        pub content: ComparisonManifestContent,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct DurableComparisonArtifact {
        pub schema: String,
        pub comparison_id: String,
        pub manifest_hash: String,
        pub artifact_hash: String,
        pub artifact_ref: String,
        pub byte_length: u64,
        pub created_at_ms: i64,
        pub artifact: ComparisonArtifact,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct DurableComparisonExport {
        pub schema: String,
        pub comparison_id: String,
        pub export_kind: String,
        pub media_type: String,
        pub content_hash: String,
        pub content_ref: String,
        pub source_artifact_hash: String,
        pub byte_length: u64,
        pub content: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    pub struct ComparisonRecord {
        pub comparison_id: String,
        pub request_hash: String,
        pub manifest_hash: String,
        pub artifact_hash: String,
        pub source_run_ids: Vec<String>,
        pub exports: BTreeMap<String, String>,
        pub created_at_ms: i64,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CreateComparisonRequest {
        pub source_run_ids: Vec<String>,
        pub scope: ComparisonScope,
        pub policy: CompatibilityPolicy,
        pub idempotency_key: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct IdempotencyClaim {
        request_hash: String,
        comparison_id: String,
    }

    pub struct ComparisonService<'a> {
        storage: &'a dyn StoragePort,
        backtests: &'a dyn BacktestRunRepository,
        datasets: &'a dyn DatasetSnapshotRepository,
    }

    impl<'a> ComparisonService<'a> {
        pub fn new(
            storage: &'a dyn StoragePort,
            backtests: &'a dyn BacktestRunRepository,
            datasets: &'a dyn DatasetSnapshotRepository,
        ) -> Self {
            Self {
                storage,
                backtests,
                datasets,
            }
        }

        pub fn create(
            &self,
            request: CreateComparisonRequest,
            now_ms: i64,
            context: &SideEffectContext,
        ) -> Result<ComparisonRecord, String> {
            self.create_with_prewrite(request, now_ms, context, |_| Ok(()))
        }

        pub fn create_with_prewrite<F>(
            &self,
            request: CreateComparisonRequest,
            now_ms: i64,
            context: &SideEffectContext,
            before_write: F,
        ) -> Result<ComparisonRecord, String>
        where
            F: Fn(&str) -> Result<(), String>,
        {
            if context.idempotency_key.as_str() != request.idempotency_key
                || IdempotencyKey::new(request.idempotency_key.clone()).is_err()
            {
                return Err("comparison_request_conflict".to_string());
            }
            validate_requested_runs(&request.source_run_ids)?;
            let sources = self.load_sources(&request.source_run_ids)?;
            let request_hash = request_hash(&sources, &request.scope, &request.policy)?;
            if let Some(existing) = self.get_idempotency(&request.idempotency_key)? {
                if existing.request_hash != request_hash {
                    return Err("comparison_request_conflict".to_string());
                }
                before_write(&existing.comparison_id)?;
                return self
                    .get(&existing.comparison_id)?
                    .ok_or_else(|| "comparison_idempotency_integrity_failed".to_string());
            }

            let comparison_id = format!("comparison_{}", hash_suffix(&request_hash));
            let selected_run_hashes = sources
                .iter()
                .map(|source| source.binding().run_hash.clone())
                .collect();
            let engine_manifest = ComparisonManifest::build(
                selected_run_hashes,
                request.scope.clone(),
                request.policy.clone(),
            )?;
            let manifest = DurableComparisonManifest::build(
                ComparisonManifestContent {
                    schema: DURABLE_MANIFEST_SCHEMA.to_string(),
                    comparison_id: comparison_id.clone(),
                    request_hash: request_hash.clone(),
                    idempotency_key: request.idempotency_key.clone(),
                    sources: sources
                        .iter()
                        .map(|source| source.binding().clone())
                        .collect(),
                    scope: request.scope,
                    policy: request.policy,
                    engine_manifest,
                },
                now_ms,
            )?;
            let artifact = compare(
                &manifest.content.engine_manifest,
                &sources
                    .iter()
                    .map(VerifiedSource::input)
                    .collect::<Vec<_>>(),
            )?;
            let durable_artifact = DurableComparisonArtifact::build(
                &comparison_id,
                &manifest.manifest_hash,
                artifact,
                now_ms,
            )?;
            // Compute every immutable value before the first durable write so
            // an incompatible source set cannot leave an orphan manifest.
            let exports = build_exports(&comparison_id, &durable_artifact)?;
            before_write(&comparison_id)?;
            put_immutable(
                self.storage,
                MANIFESTS_NS,
                &comparison_id,
                &manifest,
                context,
                "comparison_manifest_write_conflict",
            )?;
            put_immutable(
                self.storage,
                ARTIFACTS_NS,
                &comparison_id,
                &durable_artifact,
                context,
                "comparison_artifact_write_conflict",
            )?;
            let export_hashes = self.persist_exports(exports, context)?;
            let record = ComparisonRecord {
                comparison_id: comparison_id.clone(),
                request_hash: request_hash.clone(),
                manifest_hash: manifest.manifest_hash,
                artifact_hash: durable_artifact.artifact_hash,
                source_run_ids: sources
                    .iter()
                    .map(|source| source.binding().source_run_id.clone())
                    .collect(),
                exports: export_hashes,
                created_at_ms: now_ms,
            };
            put_immutable(
                self.storage,
                IDEMPOTENCY_NS,
                &request.idempotency_key,
                &IdempotencyClaim {
                    request_hash,
                    comparison_id,
                },
                context,
                "comparison_request_conflict",
            )?;
            Ok(record)
        }

        pub fn get(&self, comparison_id: &str) -> Result<Option<ComparisonRecord>, String> {
            let Some(manifest) = get_typed::<DurableComparisonManifest>(
                self.storage,
                MANIFESTS_NS,
                comparison_id,
                "comparison_manifest_integrity_failed",
            )?
            else {
                return Ok(None);
            };
            manifest.verify()?;
            let artifact = get_typed::<DurableComparisonArtifact>(
                self.storage,
                ARTIFACTS_NS,
                comparison_id,
                "comparison_artifact_integrity_failed",
            )?
            .ok_or_else(|| "comparison_artifact_integrity_failed".to_string())?;
            artifact.verify()?;
            if artifact.comparison_id != manifest.content.comparison_id
                || artifact.manifest_hash != manifest.manifest_hash
                || artifact.artifact.manifest_hash != manifest.content.engine_manifest.manifest_hash
            {
                return Err("comparison_artifact_integrity_failed".to_string());
            }
            let exports = ["json", "csv"]
                .into_iter()
                .map(|kind| {
                    let export = self
                        .get_export(comparison_id, kind)?
                        .ok_or_else(|| "comparison_export_integrity_failed".to_string())?;
                    if export.source_artifact_hash != artifact.artifact_hash {
                        return Err("comparison_export_integrity_failed".to_string());
                    }
                    Ok((kind.to_string(), export.content_hash))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            Ok(Some(ComparisonRecord {
                comparison_id: manifest.content.comparison_id.clone(),
                request_hash: manifest.content.request_hash.clone(),
                manifest_hash: manifest.manifest_hash,
                artifact_hash: artifact.artifact_hash,
                source_run_ids: manifest
                    .content
                    .sources
                    .iter()
                    .map(|source| source.source_run_id.clone())
                    .collect(),
                exports,
                created_at_ms: manifest.created_at_ms,
            }))
        }

        pub fn list(&self) -> Result<Vec<ComparisonRecord>, String> {
            let mut records = self
                .storage
                .list_json(MANIFESTS_NS)?
                .into_iter()
                .map(|(comparison_id, _)| {
                    self.get(&comparison_id)?
                        .ok_or_else(|| "comparison_manifest_integrity_failed".to_string())
                })
                .collect::<Result<Vec<_>, String>>()?;
            records.sort_by(|left, right| left.comparison_id.cmp(&right.comparison_id));
            Ok(records)
        }

        pub fn get_manifest(
            &self,
            comparison_id: &str,
        ) -> Result<Option<DurableComparisonManifest>, String> {
            let manifest: Option<DurableComparisonManifest> = get_typed(
                self.storage,
                MANIFESTS_NS,
                comparison_id,
                "comparison_manifest_integrity_failed",
            )?;
            if let Some(value) = &manifest {
                value.verify()?;
            }
            Ok(manifest)
        }

        pub fn get_artifact(
            &self,
            comparison_id: &str,
        ) -> Result<Option<DurableComparisonArtifact>, String> {
            let artifact: Option<DurableComparisonArtifact> = get_typed(
                self.storage,
                ARTIFACTS_NS,
                comparison_id,
                "comparison_artifact_integrity_failed",
            )?;
            if let Some(value) = &artifact {
                value.verify()?;
            }
            Ok(artifact)
        }

        pub fn get_export(
            &self,
            comparison_id: &str,
            export_kind: &str,
        ) -> Result<Option<DurableComparisonExport>, String> {
            if !matches!(export_kind, "json" | "csv") {
                return Err("comparison_export_kind_invalid".to_string());
            }
            let export: Option<DurableComparisonExport> = get_typed(
                self.storage,
                EXPORTS_NS,
                &export_key(comparison_id, export_kind),
                "comparison_export_integrity_failed",
            )?;
            if let Some(value) = &export {
                value.verify()?;
            }
            Ok(export)
        }

        fn get_idempotency(&self, key: &str) -> Result<Option<IdempotencyClaim>, String> {
            get_typed(
                self.storage,
                IDEMPOTENCY_NS,
                key,
                "comparison_idempotency_integrity_failed",
            )
        }

        fn load_sources(
            &self,
            requested_run_ids: &[String],
        ) -> Result<Vec<VerifiedSource>, String> {
            let mut sources = requested_run_ids
                .iter()
                .map(|run_id| self.load_source(run_id))
                .collect::<Result<Vec<_>, _>>()?;
            sources.sort_by(|left, right| left.binding().run_hash.cmp(&right.binding().run_hash));
            if sources
                .iter()
                .map(|source| &source.binding().run_hash)
                .collect::<BTreeSet<_>>()
                .len()
                != sources.len()
            {
                return Err("comparison_duplicate_run".to_string());
            }
            Ok(sources)
        }

        fn load_source(&self, run_id: &str) -> Result<VerifiedSource, String> {
            let backtest = load_backtest(self.storage, self.backtests, self.datasets, run_id);
            let robustness = load_robustness(self.storage, self.backtests, self.datasets, run_id);
            match (backtest, robustness) {
                (Ok(_), Ok(_)) => Err("comparison_source_kind_ambiguous".to_string()),
                (Ok(source), Err(error)) if error == "comparison_source_not_found" => {
                    Ok(VerifiedSource::Backtest(Box::new(source)))
                }
                (Ok(_), Err(error)) => Err(error),
                (Err(error), Ok(source)) if error == "comparison_source_not_found" => {
                    Ok(VerifiedSource::Robustness(Box::new(source)))
                }
                (Err(error), Ok(_)) => Err(error),
                (Err(backtest_error), Err(robustness_error)) => {
                    if robustness_error != "comparison_source_not_found" {
                        Err(robustness_error)
                    } else {
                        Err(backtest_error)
                    }
                }
            }
        }

        fn persist_exports(
            &self,
            exports: Vec<DurableComparisonExport>,
            context: &SideEffectContext,
        ) -> Result<BTreeMap<String, String>, String> {
            let mut hashes = BTreeMap::new();
            for export in exports {
                put_immutable(
                    self.storage,
                    EXPORTS_NS,
                    &export_key(&export.comparison_id, &export.export_kind),
                    &export,
                    context,
                    "comparison_export_write_conflict",
                )?;
                hashes.insert(export.export_kind.clone(), export.content_hash.clone());
            }
            Ok(hashes)
        }
    }

    #[derive(Clone)]
    struct VerifiedBacktest {
        binding: ComparisonSourceBinding,
        report: BacktestReport,
    }

    #[derive(Clone)]
    struct VerifiedRobustness {
        binding: ComparisonSourceBinding,
        comparison: super::RobustnessComparisonResult,
    }

    #[derive(Clone)]
    enum VerifiedSource {
        Backtest(Box<VerifiedBacktest>),
        Robustness(Box<VerifiedRobustness>),
    }

    impl VerifiedSource {
        fn binding(&self) -> &ComparisonSourceBinding {
            match self {
                Self::Backtest(source) => &source.binding,
                Self::Robustness(source) => &source.binding,
            }
        }

        fn input(&self) -> ComparisonInput {
            match self {
                Self::Backtest(source) => {
                    ComparisonInput::Backtest(Box::new(source.report.clone()))
                }
                Self::Robustness(source) => {
                    ComparisonInput::Robustness(Box::new(source.comparison.clone()))
                }
            }
        }
    }

    impl DurableComparisonManifest {
        fn build(content: ComparisonManifestContent, created_at_ms: i64) -> Result<Self, String> {
            validate_manifest_content(&content)?;
            let bytes = canonical_bytes(&content, "comparison_manifest_serialization_failed")?;
            Ok(Self {
                manifest_hash: hash_bytes(&bytes),
                manifest_ref: format!(
                    "tradeassembly://comparisons/{}/manifest",
                    content.comparison_id
                ),
                byte_length: bytes.len() as u64,
                created_at_ms,
                content,
            })
        }

        pub fn verify(&self) -> Result<(), String> {
            let rebuilt = Self::build(self.content.clone(), self.created_at_ms)
                .map_err(|_| "comparison_manifest_integrity_failed".to_string())?;
            if rebuilt.manifest_hash != self.manifest_hash
                || rebuilt.manifest_ref != self.manifest_ref
                || rebuilt.byte_length != self.byte_length
            {
                return Err("comparison_manifest_integrity_failed".to_string());
            }
            Ok(())
        }
    }

    impl DurableComparisonArtifact {
        fn build(
            comparison_id: &str,
            manifest_hash: &str,
            artifact: ComparisonArtifact,
            created_at_ms: i64,
        ) -> Result<Self, String> {
            if comparison_id.trim().is_empty() || manifest_hash.trim().is_empty() {
                return Err("comparison_artifact_invalid".to_string());
            }
            let value = json!({
                "comparisonId": comparison_id,
                "manifestHash": manifest_hash,
                "artifact": artifact,
            });
            let bytes = canonical_bytes(&value, "comparison_artifact_serialization_failed")?;
            Ok(Self {
                schema: DURABLE_ARTIFACT_SCHEMA.to_string(),
                comparison_id: comparison_id.to_string(),
                manifest_hash: manifest_hash.to_string(),
                artifact_hash: hash_bytes(&bytes),
                artifact_ref: format!("tradeassembly://comparisons/{comparison_id}/artifact"),
                byte_length: bytes.len() as u64,
                created_at_ms,
                artifact,
            })
        }

        pub fn verify(&self) -> Result<(), String> {
            if self.schema != DURABLE_ARTIFACT_SCHEMA {
                return Err("comparison_artifact_integrity_failed".to_string());
            }
            let rebuilt = Self::build(
                &self.comparison_id,
                &self.manifest_hash,
                self.artifact.clone(),
                self.created_at_ms,
            )
            .map_err(|_| "comparison_artifact_integrity_failed".to_string())?;
            if rebuilt.artifact_hash != self.artifact_hash
                || rebuilt.artifact_ref != self.artifact_ref
                || rebuilt.byte_length != self.byte_length
            {
                return Err("comparison_artifact_integrity_failed".to_string());
            }
            Ok(())
        }
    }

    impl DurableComparisonExport {
        fn build(
            comparison_id: &str,
            export_kind: &str,
            media_type: &str,
            source_artifact_hash: &str,
            content: String,
        ) -> Self {
            let content_hash = hash_bytes(content.as_bytes());
            Self {
                schema: COMPARISON_EXPORT_SCHEMA.to_string(),
                comparison_id: comparison_id.to_string(),
                export_kind: export_kind.to_string(),
                media_type: media_type.to_string(),
                content_hash,
                content_ref: format!(
                    "tradeassembly://comparisons/{comparison_id}/exports/{export_kind}"
                ),
                source_artifact_hash: source_artifact_hash.to_string(),
                byte_length: content.len() as u64,
                content,
            }
        }

        pub fn verify(&self) -> Result<(), String> {
            if self.schema != COMPARISON_EXPORT_SCHEMA
                || !matches!(self.export_kind.as_str(), "json" | "csv")
                || self.source_artifact_hash.trim().is_empty()
                || self.byte_length != self.content.len() as u64
                || self.content_hash != hash_bytes(self.content.as_bytes())
                || self.content_ref
                    != format!(
                        "tradeassembly://comparisons/{}/exports/{}",
                        self.comparison_id, self.export_kind
                    )
            {
                return Err("comparison_export_integrity_failed".to_string());
            }
            Ok(())
        }
    }

    fn load_backtest(
        storage: &dyn StoragePort,
        backtests: &dyn BacktestRunRepository,
        datasets: &dyn DatasetSnapshotRepository,
        run_id: &str,
    ) -> Result<VerifiedBacktest, String> {
        let run = backtests
            .get_run(run_id)?
            .ok_or_else(|| "comparison_source_not_found".to_string())?;
        if run.state != BacktestRunState::Completed {
            return Err("comparison_source_not_completed".to_string());
        }
        let manifest = backtests
            .get_manifest(run_id)?
            .ok_or_else(|| "comparison_source_integrity_failed".to_string())?;
        let result = backtests
            .get_result(run_id)?
            .ok_or_else(|| "comparison_source_integrity_failed".to_string())?;
        let snapshot = datasets
            .get(&manifest.content.configuration.dataset.dataset_id)?
            .ok_or_else(|| "comparison_source_integrity_failed".to_string())?;
        let attempts = backtests.list_attempts(run_id)?;
        let events = backtests.list_events(run_id)?;
        verify_source_records(&run, &manifest, &result, &snapshot)?;
        let strategy = &manifest.content.configuration.strategy;
        let strategy_spec = storage
            .list_json("strategy_versions")
            .ok()
            .into_iter()
            .flatten()
            .map(|(_, value)| value)
            .find(|value| {
                value.get("id").and_then(Value::as_str)
                    == Some(strategy.strategy_version_id.as_str())
                    && value
                        .get("strategyId")
                        .or_else(|| value.get("strategy_id"))
                        .and_then(Value::as_str)
                        == Some(strategy.strategy_id.as_str())
                    && value.get("spec").and_then(|spec| {
                        canonical_hash(spec, "comparison_strategy_hash_failed").ok()
                    }) == Some(strategy.strategy_spec_hash.clone())
            })
            .and_then(|value| value.get("spec").cloned())
            .ok_or_else(|| "comparison_source_integrity_failed".to_string())?;
        let report = backtest_report::project_with_strategy_spec(
            &run,
            &manifest,
            &result,
            &snapshot,
            &attempts,
            &events,
            Some(&strategy_spec),
        )
        .map_err(|_| "comparison_source_integrity_failed".to_string())?;
        let run_hash = canonical_hash(&report.run, "comparison_run_hash_failed")?;
        Ok(VerifiedBacktest {
            binding: ComparisonSourceBinding {
                kind: ComparisonSourceKind::Backtest,
                source_run_id: run_id.to_string(),
                run_hash,
                manifest_hash: manifest.manifest_hash.clone(),
                result_hash: result.result_hash.clone(),
                report_hash: report.report_hash.clone(),
                dataset_hash: snapshot.content_hash.clone(),
                strategy_id: report.run.strategy_id.clone(),
                strategy_version_id: report.run.strategy_version_id.clone(),
            },
            report,
        })
    }

    fn load_robustness(
        storage: &dyn StoragePort,
        backtests: &dyn BacktestRunRepository,
        datasets: &dyn DatasetSnapshotRepository,
        run_id: &str,
    ) -> Result<VerifiedRobustness, String> {
        let Some(run) = get_typed::<RobustnessRunRecord>(
            storage,
            ROBUSTNESS_RUNS_NS,
            run_id,
            "comparison_source_integrity_failed",
        )?
        else {
            return Err("comparison_source_not_found".to_string());
        };
        if run.schema != ROBUSTNESS_RUN_SCHEMA {
            return Err("comparison_source_integrity_failed".to_string());
        }
        if run.state != RobustnessRunState::Completed {
            return Err("comparison_source_not_completed".to_string());
        }
        let manifest = get_typed::<RobustnessRunManifest>(
            storage,
            ROBUSTNESS_MANIFESTS_NS,
            run_id,
            "comparison_source_integrity_failed",
        )?
        .ok_or_else(|| "comparison_source_integrity_failed".to_string())?;
        let result = get_typed::<RobustnessResult>(
            storage,
            ROBUSTNESS_RESULTS_NS,
            run_id,
            "comparison_source_integrity_failed",
        )?
        .ok_or_else(|| "comparison_source_integrity_failed".to_string())?;
        manifest
            .verify()
            .map_err(|_| "comparison_source_integrity_failed".to_string())?;
        result
            .verify()
            .map_err(|_| "comparison_source_integrity_failed".to_string())?;
        if result.content.schema != ROBUSTNESS_RESULT_SCHEMA
            || run.manifest_hash != manifest.manifest_hash
            || run.result_hash.as_deref() != Some(result.result_hash.as_str())
            || run.result.as_ref() != Some(&result)
            || manifest.content.run_id != run.run_id
            || result.content.run_id != run.run_id
            || result.content.manifest_hash != manifest.manifest_hash
            || result.content.source != manifest.content.source
        {
            return Err("comparison_source_substitution_detected".to_string());
        }

        let source = load_backtest(
            storage,
            backtests,
            datasets,
            &manifest.content.source.source_run_id,
        )?;
        let binding = &manifest.content.source;
        if binding.source_manifest_hash != source.binding.manifest_hash
            || binding.source_result_hash != source.binding.result_hash
            || binding.source_report_hash != source.binding.report_hash
            || binding.source_dataset_hash != source.binding.dataset_hash
            || binding.source_run_id != source.binding.source_run_id
        {
            return Err("comparison_source_substitution_detected".to_string());
        }
        let output: EngineRobustnessResult = serde_json::from_value(result.content.output.clone())
            .map_err(|_| "comparison_robustness_output_invalid".to_string())?;
        if !robustness_engine::verify_result_hash(&output)
            .map_err(|_| "comparison_robustness_output_invalid".to_string())?
            || output.engine_version != result.content.engine_version
            || output.source_bindings.run_id != binding.source_run_id
            || output.source_bindings.manifest_hash != binding.source_manifest_hash
            || output.source_bindings.result_hash != binding.source_result_hash
            || output.source_bindings.report_hash != binding.source_report_hash
            || output.source_bindings.dataset_hash != binding.source_dataset_hash
            || output.source_bindings.strategy_spec_hash != source.report.run.strategy_spec_hash
        {
            return Err("comparison_source_substitution_detected".to_string());
        }
        let run_hash = canonical_hash(&run, "comparison_robustness_run_hash_failed")?;
        let comparison = robustness_comparison_result(
            &run_hash,
            &result,
            &source.report,
            &source.binding,
            &output,
        )?;
        Ok(VerifiedRobustness {
            binding: ComparisonSourceBinding {
                kind: ComparisonSourceKind::Robustness,
                source_run_id: run_id.to_string(),
                run_hash,
                manifest_hash: manifest.manifest_hash,
                result_hash: result.result_hash,
                report_hash: source.binding.report_hash.clone(),
                dataset_hash: source.binding.dataset_hash.clone(),
                strategy_id: source.binding.strategy_id.clone(),
                strategy_version_id: source.binding.strategy_version_id.clone(),
            },
            comparison,
        })
    }

    fn robustness_comparison_result(
        run_hash: &str,
        result: &RobustnessResult,
        report: &BacktestReport,
        source: &ComparisonSourceBinding,
        output: &EngineRobustnessResult,
    ) -> Result<RobustnessComparisonResult, String> {
        let mut metrics = BTreeMap::new();
        let mut unavailable = BTreeMap::new();
        for key in [
            "starting_equity",
            "ending_equity",
            "net_pnl",
            "return",
            "trade_count",
            "win_count",
            "loss_count",
            "open_count",
            "fees",
            "cost_drag",
            "maximum_loss",
            "maximum_position_notional",
            "maximum_order_quantity",
        ] {
            unavailable.insert(
                key.to_string(),
                "metric_not_derivable_from_robustness_study".to_string(),
            );
        }
        if let StudyOutput::MonteCarlo(value) = &output.output {
            add_robustness_metric(
                &mut metrics,
                &mut unavailable,
                "robustness.monte_carlo.loss_probability",
                &value.loss_probability,
            );
            add_robustness_metric(
                &mut metrics,
                &mut unavailable,
                "robustness.monte_carlo.ruin_probability",
                &value.ruin_probability,
            );
            add_robustness_metric(
                &mut metrics,
                &mut unavailable,
                "robustness.monte_carlo.terminal_interval_lower",
                &value.terminal_confidence_interval.lower,
            );
            add_robustness_metric(
                &mut metrics,
                &mut unavailable,
                "robustness.monte_carlo.terminal_interval_upper",
                &value.terminal_confidence_interval.upper,
            );
        }
        for diagnostic in &output.diagnostics {
            unavailable.insert(
                format!("diagnostic.{}", diagnostic.code),
                diagnostic.detail.clone(),
            );
        }
        Ok(RobustnessComparisonResult {
            run_hash: run_hash.to_string(),
            report_hash: source.report_hash.clone(),
            result_hash: result.result_hash.clone(),
            strategy_id: source.strategy_id.clone(),
            strategy_version_id: source.strategy_version_id.clone(),
            reporting_currency: report.assumptions.reporting_currency.clone(),
            dataset_hash: source.dataset_hash.clone(),
            dataset_range: report.run.dataset_time_slice.clone(),
            instruments: report.run.instruments.clone(),
            accounting: report.assumptions.account_model.clone(),
            execution: report.assumptions.execution.clone(),
            costs: report.assumptions.costs.clone(),
            study_semantics: json!({
                "kind": "robustness",
                "study": output.study,
                "engineVersion": output.engine_version,
            }),
            metrics,
            equity: Vec::new(),
            drawdown: Vec::new(),
            trade_count: None,
            diagnostics: output
                .diagnostics
                .iter()
                .map(|diagnostic| format!("{}:{}", diagnostic.code, diagnostic.detail))
                .collect(),
            unavailable,
        })
    }

    fn add_robustness_metric(
        metrics: &mut BTreeMap<String, MetricValue>,
        unavailable: &mut BTreeMap<String, String>,
        name: &str,
        metric: &RobustnessMetric,
    ) {
        let Some(value) = metric_to_comparison(metric) else {
            unavailable.insert(
                name.to_string(),
                "robustness_metric_not_representable_as_fixed_point".to_string(),
            );
            return;
        };
        metrics.insert(name.to_string(), value);
    }

    fn metric_to_comparison(metric: &RobustnessMetric) -> Option<MetricValue> {
        if !metric.value.is_finite() {
            return None;
        }
        let (value, unit) = match metric.unit {
            Unit::CurrencyMicros => (metric.value, MetricUnit::CurrencyMicros),
            Unit::BasisPoints => (metric.value, MetricUnit::BasisPoints),
            Unit::Ratio | Unit::Probability => {
                (metric.value * 1_000_000.0, MetricUnit::RatioMicros)
            }
            Unit::Quantity => (metric.value * 1_000_000.0, MetricUnit::QuantityMicros),
            Unit::Count | Unit::Steps => (metric.value, MetricUnit::Count),
        };
        if value < i64::MIN as f64 || value > i64::MAX as f64 {
            return None;
        }
        Some(MetricValue {
            value: value.round() as i64,
            unit,
        })
    }

    fn verify_source_records(
        run: &BacktestRunRecord,
        manifest: &BacktestRunManifest,
        result: &crate::backtest_contracts::BacktestResult,
        snapshot: &DatasetSnapshot,
    ) -> Result<(), String> {
        manifest
            .verify()
            .map_err(|_| "comparison_source_integrity_failed".to_string())?;
        result
            .verify()
            .map_err(|_| "comparison_source_integrity_failed".to_string())?;
        snapshot
            .verify()
            .map_err(|_| "comparison_source_integrity_failed".to_string())?;
        if run.manifest_hash != manifest.manifest_hash
            || run.result_hash.as_deref() != Some(result.result_hash.as_str())
            || run.result.as_ref() != Some(result)
            || result.content.run_id != run.run_id
            || result.content.manifest_hash != manifest.manifest_hash
            || manifest.content.run_id != run.run_id
            || manifest.content.configuration.dataset.dataset_id != snapshot.dataset_id
            || manifest.content.configuration.dataset.content_hash != snapshot.content_hash
            || manifest.content.configuration.dataset.snapshot_ref != snapshot.snapshot_ref
        {
            return Err("comparison_source_substitution_detected".to_string());
        }
        Ok(())
    }

    fn request_hash(
        sources: &[VerifiedSource],
        scope: &ComparisonScope,
        policy: &CompatibilityPolicy,
    ) -> Result<String, String> {
        canonical_hash(
            &json!({
                "sources": sources.iter().map(VerifiedSource::binding).collect::<Vec<_>>(),
                "scope": scope,
                "policy": policy,
            }),
            "comparison_request_hash_failed",
        )
    }

    fn validate_requested_runs(run_ids: &[String]) -> Result<(), String> {
        if !(2..=5).contains(&run_ids.len())
            || run_ids.iter().any(|run_id| run_id.trim().is_empty())
            || run_ids.iter().collect::<BTreeSet<_>>().len() != run_ids.len()
        {
            return Err("comparison_runs_invalid".to_string());
        }
        Ok(())
    }

    fn validate_manifest_content(content: &ComparisonManifestContent) -> Result<(), String> {
        if content.schema != DURABLE_MANIFEST_SCHEMA
            || content.comparison_id.trim().is_empty()
            || content.request_hash.trim().is_empty()
            || content.idempotency_key.trim().is_empty()
            || !(2..=5).contains(&content.sources.len())
            || content
                .sources
                .iter()
                .any(|source| !source_binding_valid(source))
        {
            return Err("comparison_manifest_invalid".to_string());
        }
        let source_hashes = content
            .sources
            .iter()
            .map(|source| source.run_hash.clone())
            .collect::<Vec<_>>();
        if source_hashes != content.engine_manifest.selected_run_hashes {
            return Err("comparison_manifest_invalid".to_string());
        }
        if content
            .sources
            .iter()
            .map(|source| &source.source_run_id)
            .collect::<BTreeSet<_>>()
            .len()
            != content.sources.len()
        {
            return Err("comparison_manifest_invalid".to_string());
        }
        let rebuilt = ComparisonManifest::build(
            source_hashes,
            content.scope.clone(),
            content.policy.clone(),
        )?;
        if rebuilt != content.engine_manifest {
            return Err("comparison_manifest_invalid".to_string());
        }
        Ok(())
    }

    fn source_binding_valid(source: &ComparisonSourceBinding) -> bool {
        [
            &source.source_run_id,
            &source.run_hash,
            &source.manifest_hash,
            &source.result_hash,
            &source.report_hash,
            &source.dataset_hash,
            &source.strategy_id,
            &source.strategy_version_id,
        ]
        .iter()
        .all(|value| !value.trim().is_empty())
    }

    fn put_immutable<T: Serialize>(
        storage: &dyn StoragePort,
        namespace: &str,
        key: &str,
        value: &T,
        context: &SideEffectContext,
        conflict: &str,
    ) -> Result<(), String> {
        let value = serde_json::to_value(value)
            .map_err(|_| "comparison_storage_serialization_failed".to_string())?;
        match storage.put_json_if_absent(namespace, key, value, context) {
            Ok(ImmutablePutOutcome::Created | ImmutablePutOutcome::AlreadyPresent) => Ok(()),
            Err(error) if error == "immutable_storage_conflict" => Err(conflict.to_string()),
            Err(error) => Err(error),
        }
    }

    fn get_typed<T: for<'de> Deserialize<'de>>(
        storage: &dyn StoragePort,
        namespace: &str,
        key: &str,
        error: &str,
    ) -> Result<Option<T>, String> {
        storage
            .get_json(namespace, key)?
            .map(|value| serde_json::from_value(value).map_err(|_| error.to_string()))
            .transpose()
    }

    fn export_csv(artifact: &ComparisonArtifact) -> String {
        let mut lines = vec!["run_hash,metric,unit,value,unavailable_reason".to_string()];
        for metric in &artifact.aligned_metrics {
            for run_hash in metric.values_by_run.keys() {
                let value = metric.values_by_run[run_hash]
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let reason = metric
                    .unavailable
                    .get(run_hash)
                    .cloned()
                    .unwrap_or_default();
                lines.push(csv_row(&[
                    run_hash,
                    &metric.metric,
                    &serde_json::to_string(&metric.unit).unwrap_or_default(),
                    &value,
                    &reason,
                ]));
            }
        }
        format!("{}\n", lines.join("\n"))
    }

    fn build_exports(
        comparison_id: &str,
        artifact: &DurableComparisonArtifact,
    ) -> Result<Vec<DurableComparisonExport>, String> {
        let json = export_json(&artifact.artifact)?;
        Ok(vec![
            DurableComparisonExport::build(
                comparison_id,
                "json",
                "application/json",
                &artifact.artifact_hash,
                json.content,
            ),
            DurableComparisonExport::build(
                comparison_id,
                "csv",
                "text/csv",
                &artifact.artifact_hash,
                export_csv(&artifact.artifact),
            ),
        ])
    }

    fn csv_row(values: &[&str]) -> String {
        values
            .iter()
            .map(|value| csv_field(value))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn csv_field(value: &str) -> String {
        let escaped = value.replace('"', "\"\"");
        let prefixed = if matches!(escaped.chars().next(), Some('=' | '+' | '-' | '@')) {
            format!("'{escaped}")
        } else {
            escaped
        };
        format!("\"{prefixed}\"")
    }

    fn canonical_bytes<T: Serialize>(value: &T, error: &str) -> Result<Vec<u8>, String> {
        serde_json_canonicalizer::to_vec(value).map_err(|_| error.to_string())
    }

    fn hash_bytes(bytes: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(bytes))
    }

    fn hash_suffix(hash: &str) -> String {
        hash.strip_prefix("sha256:")
            .unwrap_or(hash)
            .chars()
            .take(24)
            .collect()
    }

    fn export_key(comparison_id: &str, export_kind: &str) -> String {
        format!("{comparison_id}:{export_kind}")
    }
}

#[derive(Clone)]
struct RunView {
    run_hash: String,
    report_hash: String,
    result_hash: String,
    dataset_hash: String,
    strategy_id: String,
    strategy_version_id: String,
    currency: String,
    instruments: Vec<String>,
    dataset_range: Value,
    accounting: Value,
    execution: Value,
    costs: Value,
    study_semantics: Value,
    metrics: BTreeMap<String, MetricValue>,
    equity: Vec<EquityPoint>,
    drawdown: Vec<DrawdownPoint>,
    diagnostics: Vec<String>,
    unavailable: BTreeMap<String, String>,
}

impl RunView {
    fn from_input(input: &ComparisonInput) -> Result<Self, String> {
        match input {
            ComparisonInput::Backtest(report) => {
                verify_report(report)?;
                let mut unavailable: BTreeMap<_, _> = report
                    .costs_and_risk
                    .unavailable
                    .iter()
                    .map(|item| (item.metric.clone(), item.reason.clone()))
                    .collect();
                let mut metrics = BTreeMap::new();
                insert(
                    &mut metrics,
                    "starting_equity",
                    report.overview.starting_equity_micros,
                    MetricUnit::CurrencyMicros,
                );
                insert(
                    &mut metrics,
                    "ending_equity",
                    report.overview.ending_equity_micros,
                    MetricUnit::CurrencyMicros,
                );
                insert(
                    &mut metrics,
                    "net_pnl",
                    report.overview.net_pnl_micros,
                    MetricUnit::CurrencyMicros,
                );
                if let Some(return_bps) = report.overview.return_bps {
                    insert(&mut metrics, "return", return_bps, MetricUnit::BasisPoints);
                } else {
                    unavailable.insert(
                        "return".to_string(),
                        "return_requires_nonzero_starting_equity".to_string(),
                    );
                }
                insert(
                    &mut metrics,
                    "trade_count",
                    report.overview.trade_count as i64,
                    MetricUnit::Count,
                );
                insert(
                    &mut metrics,
                    "win_count",
                    report.overview.win_count as i64,
                    MetricUnit::Count,
                );
                insert(
                    &mut metrics,
                    "loss_count",
                    report.overview.loss_count as i64,
                    MetricUnit::Count,
                );
                insert(
                    &mut metrics,
                    "open_count",
                    report.overview.open_count as i64,
                    MetricUnit::Count,
                );
                insert(
                    &mut metrics,
                    "fees",
                    report.overview.fees_micros,
                    MetricUnit::CurrencyMicros,
                );
                insert(
                    &mut metrics,
                    "cost_drag",
                    report.overview.cost_drag_micros,
                    MetricUnit::CurrencyMicros,
                );
                insert(
                    &mut metrics,
                    "maximum_loss",
                    report.costs_and_risk.maximum_loss_micros,
                    MetricUnit::CurrencyMicros,
                );
                insert(
                    &mut metrics,
                    "maximum_position_notional",
                    report.costs_and_risk.maximum_position_notional_micros,
                    MetricUnit::CurrencyMicros,
                );
                insert(
                    &mut metrics,
                    "maximum_order_quantity",
                    report.costs_and_risk.maximum_order_quantity_micros,
                    MetricUnit::QuantityMicros,
                );
                for key in unavailable.keys() {
                    metrics.remove(key);
                }
                Ok(Self {
                    run_hash: canonical_hash(&report.run, "comparison_run_hash_failed")?,
                    report_hash: report.report_hash.clone(),
                    result_hash: report.metadata.result_hash.clone(),
                    dataset_hash: report.metadata.dataset_hash.clone(),
                    strategy_id: report.run.strategy_id.clone(),
                    strategy_version_id: report.run.strategy_version_id.clone(),
                    currency: report.assumptions.reporting_currency.clone(),
                    instruments: sorted(&report.run.instruments),
                    dataset_range: report.run.dataset_time_slice.clone(),
                    accounting: report.assumptions.account_model.clone(),
                    execution: report.assumptions.execution.clone(),
                    costs: report.assumptions.costs.clone(),
                    study_semantics: json!({"kind":"backtest","evaluatorVersion":report.assumptions.evaluator_version,"compilerVersion":report.assumptions.compiler_version}),
                    metrics,
                    equity: Vec::new(),
                    drawdown: Vec::new(),
                    diagnostics: report
                        .diagnostics
                        .iter()
                        .map(|item| format!("{}:{}", item.code, item.detail))
                        .chain(unavailable.values().cloned())
                        .collect(),
                    unavailable,
                })
            }
            ComparisonInput::Robustness(run) => Ok(Self {
                run_hash: run.run_hash.clone(),
                report_hash: run.report_hash.clone(),
                result_hash: run.result_hash.clone(),
                dataset_hash: run.dataset_hash.clone(),
                strategy_id: run.strategy_id.clone(),
                strategy_version_id: run.strategy_version_id.clone(),
                currency: run.reporting_currency.clone(),
                instruments: sorted(&run.instruments),
                dataset_range: run.dataset_range.clone(),
                accounting: run.accounting.clone(),
                execution: run.execution.clone(),
                costs: run.costs.clone(),
                study_semantics: run.study_semantics.clone(),
                metrics: run.metrics.clone(),
                equity: sorted_equity(&run.equity)?,
                drawdown: sorted_drawdown(&run.drawdown)?,
                diagnostics: run.diagnostics.clone(),
                unavailable: run.unavailable.clone(),
            }),
        }
    }
    fn source_hashes(&self) -> SourceHashes {
        SourceHashes {
            run_hash: self.run_hash.clone(),
            report_hash: self.report_hash.clone(),
            result_hash: self.result_hash.clone(),
            dataset_hash: self.dataset_hash.clone(),
        }
    }
}

fn validate_run_hashes(hashes: &[String]) -> Result<(), String> {
    if !(2..=5).contains(&hashes.len())
        || hashes.iter().any(|hash| hash.trim().is_empty())
        || hashes.iter().collect::<BTreeSet<_>>().len() != hashes.len()
    {
        Err("comparison_runs_invalid".to_string())
    } else {
        Ok(())
    }
}
fn validate_manifest(manifest: &ComparisonManifest) -> Result<(), String> {
    validate_run_hashes(&manifest.selected_run_hashes)?;
    if manifest.schema != COMPARISON_SCHEMA
        || manifest.manifest_hash
            != canonical_hash(
                &manifest_without_hash(manifest),
                "comparison_manifest_hash_failed",
            )?
    {
        Err("comparison_manifest_integrity_failed".to_string())
    } else {
        Ok(())
    }
}
fn validate_scope<'a>(
    scope: &ComparisonScope,
    runs: impl Iterator<Item = &'a RunView>,
) -> Result<(), String> {
    if scope.strategy_id.trim().is_empty() {
        return Err("comparison_scope_invalid".to_string());
    }
    let runs = runs.collect::<Vec<_>>();
    for run in &runs {
        if run.strategy_id != scope.strategy_id
            && !scope
                .compatible_strategy_ids
                .iter()
                .any(|id| id == &run.strategy_id)
        {
            return Err("comparison_cross_strategy_scope_required".to_string());
        }
    }
    if !scope.allow_cross_version
        && runs
            .iter()
            .map(|run| &run.strategy_version_id)
            .collect::<BTreeSet<_>>()
            .len()
            > 1
    {
        return Err("comparison_strategy_version_scope_required".to_string());
    }
    Ok(())
}
fn verify_report(report: &BacktestReport) -> Result<(), String> {
    if report.schema != crate::backtest_report::BACKTEST_REPORT_SCHEMA
        || report.report_hash
            != canonical_hash(
                &report_without_hash(report),
                "comparison_report_hash_failed",
            )?
        || report.metadata.manifest_hash.trim().is_empty()
        || report.metadata.result_hash.trim().is_empty()
        || report.metadata.dataset_hash.trim().is_empty()
    {
        Err("comparison_report_integrity_failed".to_string())
    } else {
        Ok(())
    }
}
fn assumption_diffs(
    runs: &BTreeMap<String, RunView>,
    policy: &CompatibilityPolicy,
) -> Vec<AssumptionDiff> {
    [
        (
            "currency",
            &policy.currency,
            runs.iter()
                .map(|(id, r)| (id.clone(), json!(r.currency)))
                .collect::<BTreeMap<String, Value>>(),
        ),
        (
            "instrumentScope",
            &policy.instrument_scope,
            runs.iter()
                .map(|(id, r)| (id.clone(), json!(r.instruments)))
                .collect::<BTreeMap<String, Value>>(),
        ),
        (
            "datasetRange",
            &policy.dataset_range,
            runs.iter()
                .map(|(id, r)| (id.clone(), r.dataset_range.clone()))
                .collect::<BTreeMap<String, Value>>(),
        ),
        (
            "strategyVersion",
            &policy.strategy_version,
            runs.iter()
                .map(|(id, r)| (id.clone(), json!(r.strategy_version_id)))
                .collect::<BTreeMap<String, Value>>(),
        ),
        (
            "accounting",
            &policy.accounting,
            runs.iter()
                .map(|(id, r)| (id.clone(), r.accounting.clone()))
                .collect::<BTreeMap<String, Value>>(),
        ),
        (
            "fill",
            &policy.fill,
            runs.iter()
                .map(|(id, r)| (id.clone(), r.execution.clone()))
                .collect::<BTreeMap<String, Value>>(),
        ),
        (
            "cost",
            &policy.cost,
            runs.iter()
                .map(|(id, r)| (id.clone(), r.costs.clone()))
                .collect::<BTreeMap<String, Value>>(),
        ),
        (
            "studySemantics",
            &policy.study_semantics,
            runs.iter()
                .map(|(id, r)| (id.clone(), r.study_semantics.clone()))
                .collect::<BTreeMap<String, Value>>(),
        ),
    ]
    .into_iter()
    .map(|(field, policy, values_by_run)| {
        let same = values_by_run
            .values()
            .next()
            .is_some_and(|first| values_by_run.values().all(|value| value == first));
        let disposition = if same {
            CompatibilityDisposition::Compatible
        } else {
            match policy {
                ComparisonPolicy::Block => CompatibilityDisposition::Blocked,
                ComparisonPolicy::Warn => CompatibilityDisposition::Warning,
            }
        };
        AssumptionDiff {
            field: field.to_string(),
            values_by_run,
            reason: (!same).then(|| format!("comparison_{}_mismatch", field)),
            disposition,
        }
    })
    .collect()
}
fn align_metrics(runs: &BTreeMap<String, RunView>, blocked: bool) -> Vec<AlignedMetric> {
    let mut keys = BTreeSet::new();
    for run in runs.values() {
        keys.extend(run.metrics.keys().cloned());
    }
    keys.into_iter()
        .map(|metric| {
            let units: BTreeSet<_> = runs
                .values()
                .filter_map(|run| run.metrics.get(&metric).map(|value| value.unit.clone()))
                .collect();
            let unit = units.iter().next().cloned().unwrap_or(MetricUnit::Count);
            let mixed = units.len() > 1;
            let mut values_by_run = BTreeMap::new();
            let mut unavailable = BTreeMap::new();
            for (id, run) in runs {
                let value = run.metrics.get(&metric);
                if blocked {
                    unavailable.insert(id.clone(), "comparison_blocked_by_assumptions".to_string());
                    values_by_run.insert(id.clone(), None);
                } else if mixed {
                    unavailable.insert(id.clone(), "metric_unit_mismatch".to_string());
                    values_by_run.insert(id.clone(), None);
                } else {
                    values_by_run.insert(id.clone(), value.map(|value| value.value));
                    if value.is_none() {
                        unavailable.insert(
                            id.clone(),
                            run.unavailable
                                .get(&metric)
                                .cloned()
                                .unwrap_or_else(|| "metric_not_emitted_by_source".to_string()),
                        );
                    }
                }
            }
            AlignedMetric {
                metric,
                unit,
                values_by_run,
                unavailable,
            }
        })
        .collect()
}
fn overlays(
    runs: &BTreeMap<String, RunView>,
    blocked: bool,
) -> (
    Overlay<EquityPoint>,
    Overlay<EquityPoint>,
    Overlay<DrawdownPoint>,
) {
    let mut absolute: Overlay<EquityPoint> = empty_overlay();
    let mut normalized: Overlay<EquityPoint> = empty_overlay();
    let mut drawdown: Overlay<DrawdownPoint> = empty_overlay();
    for (id, run) in runs {
        if blocked {
            absolute
                .unavailable
                .insert(id.clone(), "comparison_blocked_by_assumptions".to_string());
            normalized
                .unavailable
                .insert(id.clone(), "comparison_blocked_by_assumptions".to_string());
            drawdown
                .unavailable
                .insert(id.clone(), "comparison_blocked_by_assumptions".to_string());
        } else {
            if run.equity.is_empty() {
                absolute.unavailable.insert(
                    id.clone(),
                    "equity_series_not_emitted_by_source".to_string(),
                );
                normalized.unavailable.insert(
                    id.clone(),
                    "equity_series_not_emitted_by_source".to_string(),
                );
            } else {
                absolute
                    .values_by_run
                    .insert(id.clone(), run.equity.clone());
                let start = run.equity[0].absolute_micros;
                if start == 0 {
                    normalized.unavailable.insert(
                        id.clone(),
                        "normalized_equity_requires_nonzero_start".to_string(),
                    );
                } else {
                    normalized.values_by_run.insert(
                        id.clone(),
                        run.equity
                            .iter()
                            .map(|point| EquityPoint {
                                timestamp: point.timestamp.clone(),
                                absolute_micros: i64::try_from(
                                    i128::from(point.absolute_micros) * 10_000 / i128::from(start),
                                )
                                .unwrap_or_default(),
                            })
                            .collect(),
                    );
                }
            }
            if run.drawdown.is_empty() {
                drawdown.unavailable.insert(
                    id.clone(),
                    "drawdown_series_not_emitted_by_source".to_string(),
                );
            } else {
                drawdown
                    .values_by_run
                    .insert(id.clone(), run.drawdown.clone());
            }
        }
    }
    (absolute, normalized, drawdown)
}
fn empty_overlay<T>() -> Overlay<T> {
    Overlay {
        values_by_run: BTreeMap::new(),
        unavailable: BTreeMap::new(),
    }
}
fn select_metrics(metrics: &[AlignedMetric], names: &[&str]) -> Vec<AlignedMetric> {
    metrics
        .iter()
        .filter(|metric| names.contains(&metric.metric.as_str()))
        .cloned()
        .collect()
}
fn insert(metrics: &mut BTreeMap<String, MetricValue>, name: &str, value: i64, unit: MetricUnit) {
    metrics.insert(name.to_string(), MetricValue { value, unit });
}
fn sorted(items: &[String]) -> Vec<String> {
    let mut items = items.to_vec();
    items.sort();
    items.dedup();
    items
}
fn sorted_equity(points: &[EquityPoint]) -> Result<Vec<EquityPoint>, String> {
    let mut points = points.to_vec();
    points.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    if points
        .windows(2)
        .any(|pair| pair[0].timestamp == pair[1].timestamp)
    {
        Err("comparison_equity_timestamp_duplicate".to_string())
    } else {
        Ok(points)
    }
}
fn sorted_drawdown(points: &[DrawdownPoint]) -> Result<Vec<DrawdownPoint>, String> {
    let mut points = points.to_vec();
    points.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    if points
        .windows(2)
        .any(|pair| pair[0].timestamp == pair[1].timestamp)
    {
        Err("comparison_drawdown_timestamp_duplicate".to_string())
    } else {
        Ok(points)
    }
}
fn manifest_without_hash(manifest: &ComparisonManifest) -> Value {
    let mut value = serde_json::to_value(manifest).expect("comparison manifest serializable");
    value["manifestHash"] = json!("");
    value
}
fn artifact_without_hash(artifact: &ComparisonArtifact) -> Value {
    let mut value = serde_json::to_value(artifact).expect("comparison artifact serializable");
    value["comparisonHash"] = json!("");
    value
}
fn report_without_hash(report: &BacktestReport) -> Value {
    let mut value = serde_json::to_value(report).expect("backtest report serializable");
    value["reportHash"] = json!("");
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    fn robustness(id: &str, strategy: &str, version: &str, currency: &str) -> ComparisonInput {
        ComparisonInput::Robustness(Box::new(RobustnessComparisonResult {
            run_hash: id.to_string(),
            report_hash: format!("report-{id}"),
            result_hash: format!("result-{id}"),
            strategy_id: strategy.to_string(),
            strategy_version_id: version.to_string(),
            reporting_currency: currency.to_string(),
            dataset_hash: "dataset-a".to_string(),
            dataset_range: json!({"start":"2026-01-01","end":"2026-01-31"}),
            instruments: vec!["BTC/USD".to_string()],
            accounting: json!({"model":"cash"}),
            execution: json!({"fill":"close"}),
            costs: json!({"feeBps":10}),
            study_semantics: json!({"kind":"monteCarlo","seed":7}),
            metrics: BTreeMap::from([
                (
                    "return".to_string(),
                    MetricValue {
                        value: 10,
                        unit: MetricUnit::BasisPoints,
                    },
                ),
                (
                    "fees".to_string(),
                    MetricValue {
                        value: 4,
                        unit: MetricUnit::CurrencyMicros,
                    },
                ),
            ]),
            unavailable: BTreeMap::new(),
            equity: vec![
                EquityPoint {
                    timestamp: "2026-01-01".to_string(),
                    absolute_micros: 100,
                },
                EquityPoint {
                    timestamp: "2026-01-02".to_string(),
                    absolute_micros: 110,
                },
            ],
            drawdown: vec![DrawdownPoint {
                timestamp: "2026-01-01".to_string(),
                drawdown_bps: 0,
            }],
            trade_count: Some(1),
            diagnostics: vec!["source_verified".to_string()],
        }))
    }
    fn manifest(ids: Vec<&str>) -> ComparisonManifest {
        ComparisonManifest::build(
            ids.into_iter().map(str::to_string).collect(),
            ComparisonScope {
                strategy_id: "strategy-a".to_string(),
                allow_cross_version: true,
                compatible_strategy_ids: vec![],
            },
            CompatibilityPolicy::default(),
        )
        .unwrap()
    }
    #[test]
    fn selection_identity_and_artifact_ignore_input_order() {
        let manifest = manifest(vec!["a", "b"]);
        let left = compare(
            &manifest,
            &[
                robustness("b", "strategy-a", "v2", "USD"),
                robustness("a", "strategy-a", "v1", "USD"),
            ],
        )
        .unwrap();
        let right = compare(
            &manifest,
            &[
                robustness("a", "strategy-a", "v1", "USD"),
                robustness("b", "strategy-a", "v2", "USD"),
            ],
        )
        .unwrap();
        assert_eq!(left.comparison_hash, right.comparison_hash);
        assert_eq!(manifest.selected_run_hashes, vec!["a", "b"]);
    }
    #[test]
    fn exact_bounds_duplicates_and_selected_set_fail_closed() {
        assert!(ComparisonManifest::build(
            vec!["a".into()],
            ComparisonScope {
                strategy_id: "x".into(),
                allow_cross_version: true,
                compatible_strategy_ids: vec![]
            },
            CompatibilityPolicy::default()
        )
        .is_err());
        assert!(ComparisonManifest::build(
            vec!["a".into(), "a".into()],
            ComparisonScope {
                strategy_id: "x".into(),
                allow_cross_version: true,
                compatible_strategy_ids: vec![]
            },
            CompatibilityPolicy::default()
        )
        .is_err());
        let manifest = manifest(vec!["a", "b"]);
        assert_eq!(
            compare(&manifest, &[robustness("a", "strategy-a", "v1", "USD")]).unwrap_err(),
            "comparison_selected_runs_mismatch"
        );
    }
    #[test]
    fn incompatible_currency_blocks_metrics_and_overlays() {
        let manifest = manifest(vec!["a", "b"]);
        let result = compare(
            &manifest,
            &[
                robustness("a", "strategy-a", "v1", "USD"),
                robustness("b", "strategy-a", "v2", "EUR"),
            ],
        )
        .unwrap();
        assert!(result.blocked);
        assert!(result
            .aligned_metrics
            .iter()
            .all(|metric| metric.values_by_run.values().all(Option::is_none)));
        assert_eq!(
            result.absolute_equity.unavailable["a"],
            "comparison_blocked_by_assumptions"
        );
    }
    #[test]
    fn warnings_are_explicit_and_do_not_block() {
        let policy = CompatibilityPolicy {
            dataset_range: ComparisonPolicy::Warn,
            ..CompatibilityPolicy::default()
        };
        let mut manifest = manifest(vec!["a", "b"]);
        manifest.policy = policy;
        manifest.manifest_hash = canonical_hash(&manifest_without_hash(&manifest), "x").unwrap();
        let mut second = robustness("b", "strategy-a", "v2", "USD");
        if let ComparisonInput::Robustness(value) = &mut second {
            value.dataset_range = json!({"start":"2026-02-01","end":"2026-02-28"});
        }
        let result = compare(
            &manifest,
            &[robustness("a", "strategy-a", "v1", "USD"), second],
        )
        .unwrap();
        assert!(!result.blocked);
        assert!(result
            .warnings
            .contains(&"comparison_warning:datasetRange".to_string()));
    }
    #[test]
    fn cross_strategy_requires_declared_scope_but_versions_are_allowed() {
        let comparison_manifest = manifest(vec!["a", "b"]);
        assert_eq!(
            compare(
                &comparison_manifest,
                &[
                    robustness("a", "strategy-a", "v1", "USD"),
                    robustness("b", "strategy-b", "v1", "USD")
                ]
            )
            .unwrap_err(),
            "comparison_cross_strategy_scope_required"
        );
        let mut scoped = manifest(vec!["a", "b"]);
        scoped
            .scope
            .compatible_strategy_ids
            .push("strategy-b".to_string());
        scoped.manifest_hash = canonical_hash(&manifest_without_hash(&scoped), "x").unwrap();
        assert!(compare(
            &scoped,
            &[
                robustness("a", "strategy-a", "v1", "USD"),
                robustness("b", "strategy-b", "v1", "USD")
            ]
        )
        .is_ok());
    }
    #[test]
    fn unit_mismatch_and_missing_series_are_unavailable_not_fabricated() {
        let manifest = manifest(vec!["a", "b"]);
        let mut second = robustness("b", "strategy-a", "v2", "USD");
        if let ComparisonInput::Robustness(value) = &mut second {
            value.metrics.insert(
                "return".into(),
                MetricValue {
                    value: 1,
                    unit: MetricUnit::CurrencyMicros,
                },
            );
            value.equity.clear();
            value.drawdown.clear();
        }
        let result = compare(
            &manifest,
            &[robustness("a", "strategy-a", "v1", "USD"), second],
        )
        .unwrap();
        let metric = result
            .aligned_metrics
            .iter()
            .find(|metric| metric.metric == "return")
            .unwrap();
        assert!(metric.values_by_run.values().all(Option::is_none));
        assert_eq!(metric.unavailable["a"], "metric_unit_mismatch");
        assert_eq!(
            result.absolute_equity.unavailable["b"],
            "equity_series_not_emitted_by_source"
        );
        assert_eq!(
            result.drawdown.unavailable["b"],
            "drawdown_series_not_emitted_by_source"
        );
    }
    #[test]
    fn exports_are_deterministic_and_hash_bound() {
        let artifact = compare(
            &manifest(vec!["a", "b"]),
            &[
                robustness("a", "strategy-a", "v1", "USD"),
                robustness("b", "strategy-a", "v2", "USD"),
            ],
        )
        .unwrap();
        let first = export_json(&artifact).unwrap();
        let second = export_json(&artifact).unwrap();
        assert_eq!(first, second);
        assert!(first.content_hash.starts_with("sha256:"));
        assert!(first.content.contains(&artifact.comparison_hash));
    }
}
