//! Integrity-bound projection from a completed backtest into a robustness result.

use crate::backtest_contracts::{canonical_hash, BacktestResult, BacktestRunManifest};
use crate::backtest_report::{BacktestReport, BACKTEST_REPORT_SCHEMA};
use crate::domain::BacktestRunState;
use crate::historical_data::{DatasetSnapshot, HistoricalObservationData};
use crate::robustness_contracts::{
    RobustnessPublicEvidence, RobustnessResult, RobustnessResultContent, RobustnessRunManifest,
    RobustnessStudyKind, ROBUSTNESS_PUBLIC_EVIDENCE_SCHEMA, ROBUSTNESS_RESULT_SCHEMA,
};
use crate::robustness_engine::{
    self, MonteCarloMode, ObservedEquityPoint, ObservedTrade, RobustnessRequest, SourceBindings,
    SourceUnits, StressKind, StudyBudget, StudyInput, Unit, VerificationStatus, VerifiedBacktest,
    ROBUSTNESS_ENGINE_VERSION, ROBUSTNESS_REQUEST_SCHEMA,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct RobustnessSourceArtifacts {
    pub binding: crate::robustness_contracts::RobustnessSourceBinding,
    pub manifest: BacktestRunManifest,
    pub result: BacktestResult,
    pub report: BacktestReport,
    pub dataset: DatasetSnapshot,
    pub strategy_version: Value,
}

/// Projects the exact typed study declaration and durable source hashes into
/// the public report boundary. The text is a fixed label per enum variant;
/// neither engine output nor caller-provided prose can change it.
pub fn public_evidence(
    manifest: &RobustnessRunManifest,
    result: &RobustnessResult,
) -> Result<RobustnessPublicEvidence, String> {
    manifest.verify()?;
    result.verify()?;
    if result.content.run_id != manifest.content.run_id
        || result.content.manifest_hash != manifest.manifest_hash
        || result.content.source != manifest.content.source
    {
        return Err("robustness_evidence_integrity_failed".to_string());
    }
    let study = parse_study(manifest)?;
    let (measures, mut limitations) = match study {
        StudyInput::MonteCarlo(_) => (
            "distribution of simulated outcomes under the declared resampling, execution-randomization, parameter-perturbation, or synthetic-path assumptions",
            vec![
                "Monte Carlo, path resampling, and ruin estimates are not overfit validation",
            ],
        ),
        StudyInput::ParameterSensitivity(_) => (
            "outcome sensitivity across the declared parameter cases",
            Vec::new(),
        ),
        StudyInput::FeeSensitivity(_) => (
            "outcome sensitivity to the declared fee and cost cases",
            Vec::new(),
        ),
        StudyInput::SlippageSensitivity(_) => (
            "outcome sensitivity to the declared slippage cases",
            Vec::new(),
        ),
        StudyInput::ExecutionSensitivity(_) => (
            "outcome sensitivity to the declared execution-model cases",
            Vec::new(),
        ),
        StudyInput::WalkForward(_) => (
            "stability across the declared rolling walk-forward windows",
            Vec::new(),
        ),
        StudyInput::Regime(_) => (
            "outcome variation across the declared market-regime partitions",
            Vec::new(),
        ),
        StudyInput::Stress(_) => (
            "outcome response to the declared stress scenarios",
            Vec::new(),
        ),
        StudyInput::CapacityLiquidity(_) => (
            "outcome response across the declared capacity and liquidity cases",
            Vec::new(),
        ),
    };
    limitations.push(
        "No unavailable holdout, optimizer, or out-of-sample proof is claimed by this report",
    );
    Ok(RobustnessPublicEvidence {
        schema: ROBUSTNESS_PUBLIC_EVIDENCE_SCHEMA.to_string(),
        study_kind: manifest.content.study_kind.clone(),
        measures: measures.to_string(),
        limitations: limitations.into_iter().map(str::to_string).collect(),
        manifest_hash: manifest.manifest_hash.clone(),
        result_hash: result.result_hash.clone(),
        source: manifest.content.source.clone(),
        supporting_sources: manifest.content.supporting_sources.clone(),
    })
}

/// Executes the typed robustness study against one exact, completed backtest evidence set.
///
/// The entrypoint deliberately accepts only durable artifacts. It does not fetch data, infer
/// missing market facts, or substitute source artifacts with matching-looking values.
pub fn execute(
    manifest: &RobustnessRunManifest,
    source_manifest: &BacktestRunManifest,
    source_result: &BacktestResult,
    source_report: &BacktestReport,
    dataset: &DatasetSnapshot,
    strategy_version: &Value,
    now_ms: i64,
) -> Result<RobustnessResult, String> {
    if !manifest.content.supporting_sources.is_empty() {
        return Err("robustness_supporting_sources_missing".to_string());
    }
    execute_with_sources(
        manifest,
        &[RobustnessSourceArtifacts {
            binding: manifest.content.source.clone(),
            manifest: source_manifest.clone(),
            result: source_result.clone(),
            report: source_report.clone(),
            dataset: dataset.clone(),
            strategy_version: strategy_version.clone(),
        }],
        now_ms,
    )
}

pub fn execute_with_sources(
    manifest: &RobustnessRunManifest,
    sources: &[RobustnessSourceArtifacts],
    now_ms: i64,
) -> Result<RobustnessResult, String> {
    let verified_sources = verify_and_map_sources(manifest, sources)?;
    let study = compile_study(manifest, sources, &verified_sources)?;
    let request = RobustnessRequest {
        schema: ROBUSTNESS_REQUEST_SCHEMA.to_string(),
        engine_version: ROBUSTNESS_ENGINE_VERSION.to_string(),
        seed: manifest.content.deterministic_seed,
        source: verified_sources
            .first()
            .cloned()
            .ok_or_else(|| "robustness_source_integrity_failed".to_string())?,
        supporting_sources: verified_sources.iter().skip(1).cloned().collect(),
        budgets: map_budget(manifest)?,
        study,
    };
    let engine_result = robustness_engine::run(&request)
        .map_err(|error| format!("robustness_engine_{}", error.code))?;
    let output = serde_json::to_value(&engine_result)
        .map_err(|_| "robustness_projection_output_serialization_failed".to_string())?;
    let diagnostics = engine_result
        .diagnostics
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "robustness_projection_diagnostics_serialization_failed".to_string())?;
    RobustnessResult::build(
        RobustnessResultContent {
            schema: ROBUSTNESS_RESULT_SCHEMA.to_string(),
            run_id: manifest.content.run_id.clone(),
            manifest_hash: manifest.manifest_hash.clone(),
            source: manifest.content.source.clone(),
            engine_version: manifest.content.engine_version.clone(),
            output,
            diagnostics,
        },
        now_ms,
    )
}

fn verify_and_map_sources(
    manifest: &RobustnessRunManifest,
    sources: &[RobustnessSourceArtifacts],
) -> Result<Vec<VerifiedBacktest>, String> {
    manifest.verify()?;
    let expected = std::iter::once(&manifest.content.source)
        .chain(manifest.content.supporting_sources.iter())
        .collect::<Vec<_>>();
    if sources.len() != expected.len() {
        return Err("robustness_source_set_mismatch".to_string());
    }
    let verified = expected
        .into_iter()
        .map(|binding| {
            let artifacts = sources
                .iter()
                .find(|source| source.binding.source_run_id == binding.source_run_id)
                .ok_or_else(|| "robustness_source_set_mismatch".to_string())?;
            if &artifacts.binding != binding {
                return Err("robustness_source_binding_mismatch".to_string());
            }
            verify_artifacts(
                binding,
                &artifacts.manifest,
                &artifacts.result,
                &artifacts.report,
                &artifacts.dataset,
                &artifacts.strategy_version,
            )?;
            map_source(&artifacts.manifest, &artifacts.report, &artifacts.dataset)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let strategy_id = sources
        .first()
        .map(|source| {
            source
                .manifest
                .content
                .configuration
                .strategy
                .strategy_id
                .as_str()
        })
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "robustness_source_strategy_invalid".to_string())?;
    if sources
        .iter()
        .any(|source| source.manifest.content.configuration.strategy.strategy_id != strategy_id)
    {
        return Err("robustness_source_strategy_incompatible".to_string());
    }
    Ok(verified)
}

fn verify_artifacts(
    binding: &crate::robustness_contracts::RobustnessSourceBinding,
    source_manifest: &BacktestRunManifest,
    source_result: &BacktestResult,
    source_report: &BacktestReport,
    dataset: &DatasetSnapshot,
    strategy_version: &Value,
) -> Result<(), String> {
    source_manifest.verify()?;
    source_result.verify()?;
    dataset.verify()?;
    verify_report(source_report)?;

    if source_report.run.state != BacktestRunState::Completed
        || binding.source_run_id != source_manifest.content.run_id
        || source_result.content.run_id != source_manifest.content.run_id
        || source_result.content.manifest_hash != source_manifest.manifest_hash
        || source_report.run.run_id != source_manifest.content.run_id
        || source_report.metadata.manifest_hash != source_manifest.manifest_hash
        || source_report.metadata.result_hash != source_result.result_hash
        || source_report.metadata.dataset_hash != dataset.content_hash
    {
        return Err("robustness_source_backtest_binding_invalid".to_string());
    }
    if binding.source_manifest_hash != source_manifest.manifest_hash
        || binding.source_result_hash != source_result.result_hash
        || binding.source_report_hash != source_report.report_hash
        || binding.source_dataset_id != dataset.dataset_id
        || binding.source_dataset_hash != dataset.content_hash
    {
        return Err("robustness_source_binding_mismatch".to_string());
    }
    let configured_dataset = &source_manifest.content.configuration.dataset;
    let configured_strategy = &source_manifest.content.configuration.strategy;
    let strategy_spec = strategy_version
        .get("spec")
        .ok_or_else(|| "robustness_source_strategy_invalid".to_string())?;
    let strategy_spec_hash =
        canonical_hash(strategy_spec, "robustness_source_strategy_hash_failed")?;
    if configured_dataset.dataset_id != dataset.dataset_id
        || configured_dataset.snapshot_ref != dataset.snapshot_ref
        || configured_dataset.content_hash != dataset.content_hash
        || source_report.run.strategy_id != configured_strategy.strategy_id
        || source_report.run.strategy_version_id != configured_strategy.strategy_version_id
        || source_report.run.strategy_spec_hash != configured_strategy.strategy_spec_hash
        || strategy_version.get("id").and_then(Value::as_str)
            != Some(configured_strategy.strategy_version_id.as_str())
        || strategy_version
            .get("strategyId")
            .or_else(|| strategy_version.get("strategy_id"))
            .and_then(Value::as_str)
            != Some(configured_strategy.strategy_id.as_str())
        || strategy_spec_hash != configured_strategy.strategy_spec_hash
    {
        return Err("robustness_source_substitution_detected".to_string());
    }
    Ok(())
}

fn verify_report(report: &BacktestReport) -> Result<(), String> {
    let mut value = serde_json::to_value(report)
        .map_err(|_| "robustness_source_report_serialization_failed".to_string())?;
    value["reportHash"] = Value::String(String::new());
    if report.schema != BACKTEST_REPORT_SCHEMA
        || report.report_hash != canonical_hash(&value, "robustness_source_report_hash_failed")?
    {
        return Err("robustness_source_report_integrity_failed".to_string());
    }
    Ok(())
}

pub(crate) fn parse_study(manifest: &RobustnessRunManifest) -> Result<StudyInput, String> {
    let study: StudyInput = serde_json::from_value(manifest.content.assumptions.clone())
        .map_err(|_| "robustness_study_assumptions_invalid".to_string())?;
    if study_kind(&study) != manifest.content.study_kind {
        return Err("robustness_study_kind_mismatch".to_string());
    }
    Ok(study)
}

fn compile_study(
    manifest: &RobustnessRunManifest,
    artifacts: &[RobustnessSourceArtifacts],
    sources: &[VerifiedBacktest],
) -> Result<StudyInput, String> {
    let mut study = parse_study(manifest)?;
    let catalog = sources
        .iter()
        .map(|source| (source.bindings.run_id.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let artifact_catalog = artifacts
        .iter()
        .map(|source| (source.binding.source_run_id.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    match &mut study {
        StudyInput::MonteCarlo(input) => {
            let primary = sources
                .first()
                .ok_or_else(|| "robustness_source_integrity_failed".to_string())?;
            match &mut input.mode {
                MonteCarloMode::ParameterPerturbation { model } => {
                    model.evidence_refs = primary.bindings.evidence_refs.clone();
                }
                MonteCarloMode::SyntheticPath { model } => {
                    model.evidence_refs = primary.bindings.evidence_refs.clone();
                }
                _ => {}
            }
        }
        StudyInput::ParameterSensitivity(input) => {
            for case in &mut input.cases {
                let source = required_source(&catalog, &case.source_run_id)?;
                let source_artifacts = required_artifacts(&artifact_catalog, &case.source_run_id)?;
                case.parameters = numeric_overrides(
                    &source_artifacts.strategy_version,
                    &source_artifacts
                        .manifest
                        .content
                        .configuration
                        .variable_overrides,
                )?;
                case.trade_returns = source_returns(source);
                case.evidence_refs = source.bindings.evidence_refs.clone();
            }
        }
        StudyInput::FeeSensitivity(input)
        | StudyInput::SlippageSensitivity(input)
        | StudyInput::ExecutionSensitivity(input) => {
            for case in &mut input.cases {
                let source = required_source(&catalog, &case.source_run_id)?;
                let source_artifacts = required_artifacts(&artifact_catalog, &case.source_run_id)?;
                let configuration = &source_artifacts.manifest.content.configuration;
                case.fee_bps = configuration.costs.notional_fee_bps as f64;
                case.slippage_bps = configuration.costs.slippage_bps as f64;
                case.execution_model = configuration.evaluator_version.clone();
                case.trade_returns = source_returns(source);
                case.evidence_refs = source.bindings.evidence_refs.clone();
            }
        }
        StudyInput::WalkForward(input) => {
            for window in &mut input.windows {
                let in_sample = required_source(&catalog, &window.in_sample_source_run_id)?;
                let out_of_sample = required_source(&catalog, &window.out_of_sample_source_run_id)?;
                let in_artifacts =
                    required_artifacts(&artifact_catalog, &window.in_sample_source_run_id)?;
                window.in_sample_returns = source_returns(in_sample);
                window.out_of_sample_returns = source_returns(out_of_sample);
                window.selected_parameters = numeric_overrides(
                    &in_artifacts.strategy_version,
                    &in_artifacts
                        .manifest
                        .content
                        .configuration
                        .variable_overrides,
                )?;
                window.evidence_refs = merged_refs(&[
                    &in_sample.bindings.evidence_refs,
                    &out_of_sample.bindings.evidence_refs,
                ]);
            }
        }
        StudyInput::Regime(input) => {
            for segment in &mut input.segments {
                let source = required_source(&catalog, &segment.source_run_id)?;
                segment.trade_returns = source_returns(source);
                segment.costs_micros = source.trades.iter().map(|trade| trade.fee_micros).sum();
                segment.exposure_ratio = None;
                segment.evidence_refs = source.bindings.evidence_refs.clone();
            }
        }
        StudyInput::Stress(input) => {
            for scenario in &mut input.scenarios {
                match scenario.kind {
                    StressKind::HistoricalObserved => {
                        let run_id = scenario
                            .source_run_id
                            .as_deref()
                            .ok_or_else(|| "robustness_stress_source_missing".to_string())?;
                        let source = required_source(&catalog, run_id)?;
                        scenario.shocked_trade_returns = source_returns(source);
                        scenario.exposure_ratio = None;
                        scenario.contribution_micros = source
                            .trades
                            .iter()
                            .map(|trade| (trade.trade_id.clone(), trade.pnl_micros))
                            .collect();
                        scenario.evidence_refs = source.bindings.evidence_refs.clone();
                    }
                    StressKind::HypotheticalDeclared => {
                        scenario.source_run_id = None;
                        scenario.evidence_refs = sources
                            .first()
                            .map(|source| source.bindings.evidence_refs.clone())
                            .unwrap_or_default();
                    }
                }
            }
        }
        StudyInput::CapacityLiquidity(input) => {
            for observation in &mut input.observations {
                let source = required_source(&catalog, &observation.source_run_id)?;
                let source_artifacts =
                    required_artifacts(&artifact_catalog, &observation.source_run_id)?;
                let trade = source
                    .trades
                    .iter()
                    .find(|trade| trade.trade_id == observation.trade_id)
                    .ok_or_else(|| "robustness_liquidity_trade_not_found".to_string())?;
                let report_trade = source_artifacts
                    .report
                    .trades
                    .iter()
                    .find(|candidate| candidate.trade_id == observation.trade_id)
                    .ok_or_else(|| "robustness_liquidity_trade_not_found".to_string())?;
                observation.return_ratio = trade.return_ratio;
                observation.quantity = trade.quantity;
                let liquidity = observed_liquidity(report_trade, &source_artifacts.dataset);
                observation.observed_volume = liquidity.volume;
                observation.observed_spread_bps = liquidity.spread_bps;
                observation.open_interest = liquidity.open_interest;
                observation.displayed_depth = None;
                observation.evidence_refs =
                    merged_refs(&[&trade.evidence_refs, &source.bindings.evidence_refs]);
            }
        }
    }
    Ok(study)
}

fn required_source<'a>(
    catalog: &BTreeMap<&str, &'a VerifiedBacktest>,
    run_id: &str,
) -> Result<&'a VerifiedBacktest, String> {
    catalog
        .get(run_id)
        .copied()
        .ok_or_else(|| "robustness_observed_source_not_bound".to_string())
}

fn required_artifacts<'a>(
    catalog: &BTreeMap<&str, &'a RobustnessSourceArtifacts>,
    run_id: &str,
) -> Result<&'a RobustnessSourceArtifacts, String> {
    catalog
        .get(run_id)
        .copied()
        .ok_or_else(|| "robustness_observed_source_not_bound".to_string())
}

fn source_returns(source: &VerifiedBacktest) -> Vec<f64> {
    source
        .trades
        .iter()
        .map(|trade| trade.return_ratio)
        .collect()
}

fn numeric_overrides(
    strategy_version: &Value,
    overrides: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, f64>, String> {
    let variables = strategy_version
        .get("spec")
        .and_then(|spec| spec.get("variables"))
        .and_then(Value::as_object)
        .ok_or_else(|| "robustness_source_variables_invalid".to_string())?;
    let mut values = variables
        .iter()
        .filter_map(|(name, definition)| {
            definition
                .get("default")
                .and_then(Value::as_f64)
                .map(|value| (name.clone(), value))
        })
        .collect::<BTreeMap<_, _>>();
    for (name, value) in overrides {
        if !variables.contains_key(name) {
            return Err("robustness_source_variable_override_invalid".to_string());
        }
        let value = value
            .as_f64()
            .ok_or_else(|| "robustness_source_variable_override_invalid".to_string())?;
        values.insert(name.clone(), value);
    }
    Ok(values)
}

fn merged_refs(groups: &[&Vec<String>]) -> Vec<String> {
    groups
        .iter()
        .flat_map(|refs| refs.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Default)]
struct ObservedLiquidity {
    volume: Option<f64>,
    spread_bps: Option<f64>,
    open_interest: Option<f64>,
}

fn observed_liquidity(
    trade: &crate::backtest_report::BacktestTrade,
    dataset: &DatasetSnapshot,
) -> ObservedLiquidity {
    let timestamp = trade.entry.get("timestamp").and_then(Value::as_str);
    let observation = dataset.content.observations.iter().find(|observation| {
        observation.instrument_id == trade.symbol
            && timestamp.is_some_and(|value| value == observation.timestamp)
    });
    match observation.map(|observation| &observation.data) {
        Some(HistoricalObservationData::Bar(bar)) => ObservedLiquidity {
            volume: Some(bar.volume),
            ..ObservedLiquidity::default()
        },
        Some(HistoricalObservationData::OptionContract(option)) => {
            let spread_bps = option.bid.zip(option.ask).and_then(|(bid, ask)| {
                let midpoint = (bid + ask) / 2.0;
                (midpoint > 0.0).then_some((ask - bid) / midpoint * 10_000.0)
            });
            ObservedLiquidity {
                volume: option.volume,
                spread_bps,
                open_interest: option.open_interest,
            }
        }
        None => ObservedLiquidity::default(),
    }
}

fn study_kind(study: &StudyInput) -> RobustnessStudyKind {
    match study {
        StudyInput::MonteCarlo(_) => RobustnessStudyKind::MonteCarlo,
        StudyInput::ParameterSensitivity(_) => RobustnessStudyKind::ParameterSensitivity,
        StudyInput::FeeSensitivity(_) => RobustnessStudyKind::FeeSensitivity,
        StudyInput::SlippageSensitivity(_) => RobustnessStudyKind::SlippageSensitivity,
        StudyInput::ExecutionSensitivity(_) => RobustnessStudyKind::ExecutionModelSensitivity,
        StudyInput::WalkForward(_) => RobustnessStudyKind::WalkForward,
        StudyInput::Regime(_) => RobustnessStudyKind::Regime,
        StudyInput::Stress(_) => RobustnessStudyKind::Stress,
        StudyInput::CapacityLiquidity(_) => RobustnessStudyKind::CapacityLiquidity,
    }
}

pub(crate) fn map_budget(manifest: &RobustnessRunManifest) -> Result<StudyBudget, String> {
    let budget = &manifest.content.budget;
    let max_path_points = usize::try_from(
        u64::from(budget.maximum_samples)
            .checked_mul(u64::from(budget.maximum_windows))
            .ok_or_else(|| "robustness_budget_conversion_overflow".to_string())?,
    )
    .map_err(|_| "robustness_budget_conversion_overflow".to_string())?;
    let max_output_points = usize::try_from(budget.maximum_output_bytes / 64)
        .map_err(|_| "robustness_budget_conversion_overflow".to_string())?;
    let max_memory_bytes = usize::try_from(budget.maximum_output_bytes)
        .map_err(|_| "robustness_budget_conversion_overflow".to_string())?;
    if max_path_points == 0 || max_output_points == 0 || max_memory_bytes == 0 {
        return Err("robustness_budget_projection_invalid".to_string());
    }
    Ok(StudyBudget {
        max_samples: budget.maximum_samples as usize,
        max_grid_points: budget.maximum_grid_points as usize,
        max_windows: budget.maximum_windows as usize,
        max_scenarios: budget.maximum_scenarios as usize,
        max_path_points,
        max_output_points,
        max_memory_bytes,
    })
}

fn map_source(
    source_manifest: &BacktestRunManifest,
    report: &BacktestReport,
    dataset: &DatasetSnapshot,
) -> Result<VerifiedBacktest, String> {
    let refs = source_evidence_refs(source_manifest, report);
    if refs.is_empty() {
        return Err("robustness_source_evidence_unavailable".to_string());
    }
    let starting_equity = report.overview.starting_equity_micros;
    if starting_equity <= 0 || report.overview.ending_equity_micros < 0 {
        return Err("robustness_source_equity_unavailable".to_string());
    }
    let slippage_bps = source_manifest.content.configuration.costs.slippage_bps as f64;
    let trades = report
        .trades
        .iter()
        .enumerate()
        .map(|(index, trade)| {
            let pnl = trade
                .pnl_micros
                .ok_or_else(|| "robustness_source_trade_pnl_unavailable".to_string())?;
            let trade_refs = trade_evidence_refs(trade);
            if trade_refs.is_empty() {
                return Err("robustness_source_trade_evidence_unavailable".to_string());
            }
            Ok(ObservedTrade {
                trade_id: trade.trade_id.clone(),
                sequence: (index + 1) as u64,
                return_ratio: pnl as f64 / starting_equity as f64,
                pnl_micros: pnl,
                fee_micros: trade.fee_micros,
                slippage_bps,
                quantity: trade.quantity_micros as f64 / 1_000_000.0,
                evidence_refs: trade_refs,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if trades.is_empty() {
        return Err("robustness_source_trades_unavailable".to_string());
    }
    Ok(VerifiedBacktest {
        verification: VerificationStatus::IntegrityVerifiedCompleted,
        bindings: SourceBindings {
            run_id: source_manifest.content.run_id.clone(),
            manifest_hash: source_manifest.manifest_hash.clone(),
            result_hash: report.metadata.result_hash.clone(),
            report_hash: report.report_hash.clone(),
            dataset_hash: dataset.content_hash.clone(),
            strategy_spec_hash: source_manifest
                .content
                .configuration
                .strategy
                .strategy_spec_hash
                .clone(),
            evidence_refs: refs.clone(),
            replay_ref: report.metadata.replay_ref.clone(),
        },
        units: SourceUnits {
            reporting_currency: report.assumptions.reporting_currency.clone(),
            equity: Unit::CurrencyMicros,
            trade_return: Unit::Ratio,
            cost: Unit::CurrencyMicros,
            quantity: Unit::Quantity,
        },
        starting_equity_micros: starting_equity,
        trades,
        equity: vec![
            ObservedEquityPoint {
                sequence: 1,
                equity_micros: starting_equity,
                evidence_refs: refs.clone(),
            },
            ObservedEquityPoint {
                sequence: 2,
                equity_micros: report.overview.ending_equity_micros,
                evidence_refs: refs,
            },
        ],
    })
}

fn source_evidence_refs(manifest: &BacktestRunManifest, report: &BacktestReport) -> Vec<String> {
    let mut refs = BTreeSet::new();
    refs.extend(manifest.content.evidence_refs.iter().cloned());
    refs.extend(report.chart.evidence_refs.iter().cloned());
    refs.extend(
        report
            .journal
            .iter()
            .map(|event| event.evidence_hash.clone()),
    );
    refs.into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect()
}

fn trade_evidence_refs(trade: &crate::backtest_report::BacktestTrade) -> Vec<String> {
    let mut refs = BTreeSet::new();
    refs.extend(trade.order_refs.iter().cloned());
    refs.extend(trade.fill_refs.iter().cloned());
    refs.extend(trade.ledger_refs.iter().cloned());
    refs.into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect()
}
