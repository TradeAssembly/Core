// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{ServiceResponse, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::backtest_accounting::AccountingResult;
use crate::backtest_contracts::{canonical_hash, BacktestRunManifest};
use crate::backtest_report::{self, BacktestReport, BacktestTrade};
use crate::derivatives_analysis::{
    self, DeclaredScenario, DerivativesAnalysis, DerivativesAnalysisRequest, InstrumentKind,
    LifecycleEvidence, LifecycleState, Provenance, ScenarioKind, SensitivityGrid, SourceBindings,
    SourcedPrice, StrategyLeg, VersionedPricingModel, DERIVATIVES_REQUEST_SCHEMA, MAX_SCENARIOS,
    MAX_SENSITIVITY_AXIS_VALUES, MAX_SENSITIVITY_POINTS,
};
use crate::domain::BacktestRunState;
use crate::historical_data::{DatasetSnapshot, HistoricalObservationData};
use crate::ports::{AuthorityContext, IdempotencyKey, ImmutablePutOutcome, SideEffectContext};
use chrono::DateTime;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const ANALYSIS_NS: &str = "derivatives_analyses_v1";
const IDEMPOTENCY_NS: &str = "derivatives_analysis_idempotency_v1";
const ENGINE_VERSION: &str = "tradeassembly.derivatives-analysis-engine.v1";

struct SourceArtifacts {
    manifest: BacktestRunManifest,
    report: BacktestReport,
    snapshot: DatasetSnapshot,
    accounting: AccountingResult,
}

pub fn create(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    match create_inner(service, &body) {
        Ok(value) => ServiceResponse::created(value),
        Err(code) if code == "object_not_available" => ServiceResponse::object_unavailable(),
        Err(code) if code == "derivatives_request_conflict" => ServiceResponse::conflict(&code),
        Err(code) => ServiceResponse::bad_request(&code),
    }
}

fn create_inner(service: &TradeAssemblyService, body: &Value) -> Result<Value, String> {
    let source_run_id = required_string(body, "sourceRunId", "source_run_id")?;
    service
        .require_object("backtest_run", source_run_id)
        .map_err(|_| "object_not_available".to_string())?;
    let idempotency_key =
        required_string(body, "idempotencyKey", "idempotency_key").and_then(|value| {
            IdempotencyKey::new(value).map_err(|_| "derivatives_request_invalid".to_string())
        })?;
    validate_wire_bounds(body)?;
    let source = source_artifacts(service, source_run_id)?;
    let as_of_ms = body
        .get("asOfMs")
        .or_else(|| body.get("as_of_ms"))
        .and_then(Value::as_i64)
        .unwrap_or(latest_observation_ms(&source.snapshot)?);
    let max_mark_age_ms = nonnegative_i64(body, "maxMarkAgeMs", "max_mark_age_ms", 900_000)?;
    let max_greek_age_ms = nonnegative_i64(body, "maxGreekAgeMs", "max_greek_age_ms", 900_000)?;
    let scenarios = declared_scenarios(body, source_run_id)?;
    let sensitivity_grid = optional_typed::<SensitivityGrid>(
        body.get("sensitivityGrid")
            .or_else(|| body.get("sensitivity_grid")),
        "derivatives_sensitivity_grid_invalid",
    )?;
    let pricing_model = declared_pricing_model(body, source_run_id)?;
    let request = build_request(
        &source,
        as_of_ms,
        max_mark_age_ms,
        max_greek_age_ms,
        scenarios,
        sensitivity_grid,
        pricing_model,
    )?;
    let request_hash = canonical_hash(&request, "derivatives_request_hash_failed")?;
    if let Some(existing) = service
        .runtime()
        .storage
        .get_json(IDEMPOTENCY_NS, idempotency_key.as_str())?
    {
        if existing["requestHash"].as_str() != Some(request_hash.as_str()) {
            return Err("derivatives_request_conflict".to_string());
        }
        let analysis_id = existing["analysisId"]
            .as_str()
            .ok_or_else(|| "derivatives_evidence_integrity_failed".to_string())?;
        let record = service
            .runtime()
            .storage
            .get_json(ANALYSIS_NS, analysis_id)?
            .ok_or_else(|| "derivatives_evidence_integrity_failed".to_string())?;
        let analysis_id = record["analysisId"]
            .as_str()
            .ok_or_else(|| "derivatives_evidence_integrity_failed".to_string())?;
        service
            .require_object("derivatives_analysis", analysis_id)
            .map_err(|_| "object_not_available".to_string())?;
        verify_record(&record)?;
        return Ok(record);
    }

    let analysis = derivatives_analysis::analyze(
        &request,
        &source.snapshot,
        &source.manifest.content.configuration.instruments,
        &source.accounting,
        &source.report,
    )
    .map_err(analysis_error)?;
    verify_output_hash(&analysis)?;
    let identity_hash = canonical_hash(
        &json!({"requestHash": request_hash, "outputHash": analysis.output_hash}),
        "derivatives_identity_hash_failed",
    )?;
    let analysis_id = format!(
        "derivatives-{}",
        identity_hash
            .strip_prefix("sha256:")
            .unwrap_or(&identity_hash)
    );
    service
        .bind_inherited_object(
            "derivatives_analysis",
            &analysis_id,
            "backtest_run",
            source_run_id,
        )
        .map_err(|_| "object_not_available".to_string())?;
    let exports = write_exports(service, &analysis_id, &analysis)?;
    let record = json!({
        "schema": "tradeassembly.derivatives-analysis-record.v1",
        "analysisId": analysis_id,
        "requestHash": request_hash,
        "request": request,
        "sourceRunId": source_run_id,
        "sourceReportHash": source.report.report_hash,
        "analysis": analysis,
        "exports": exports,
        "replayRef": format!("tradeassembly://derivatives/{analysis_id}/replay"),
        "createdAtMs": service.runtime().clock.now_ms(),
        "noAdvice": LEGAL_BOUNDARY,
    });
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: safe_token(body, "actor", "local-user"),
            surface: safe_token(body, "surface", "service"),
            account_mode: "research".to_string(),
        },
        idempotency_key.clone(),
    );
    match service.runtime().storage.put_json_if_absent(
        ANALYSIS_NS,
        &analysis_id,
        record.clone(),
        &context,
    )? {
        ImmutablePutOutcome::Created | ImmutablePutOutcome::AlreadyPresent => {}
    }
    let idempotency = json!({
        "requestHash": request_hash,
        "analysisId": analysis_id,
    });
    match service.runtime().storage.put_json_if_absent(
        IDEMPOTENCY_NS,
        idempotency_key.as_str(),
        idempotency,
        &context,
    )? {
        ImmutablePutOutcome::Created => {}
        ImmutablePutOutcome::AlreadyPresent => {
            let existing = service
                .runtime()
                .storage
                .get_json(IDEMPOTENCY_NS, idempotency_key.as_str())?
                .ok_or_else(|| "derivatives_request_conflict".to_string())?;
            if existing["requestHash"].as_str() != Some(request_hash.as_str())
                || existing["analysisId"].as_str() != Some(analysis_id.as_str())
            {
                return Err("derivatives_request_conflict".to_string());
            }
        }
    }
    service.runtime().record_side_effect(
        "derivatives.analysis.created",
        json!({
            "analysisId": analysis_id,
            "sourceRunId": source_run_id,
            "requestHash": request_hash,
            "outputHash": record["analysis"]["outputHash"],
        }),
        &context,
    )?;
    Ok(record)
}

fn validate_wire_bounds(body: &Value) -> Result<(), String> {
    if let Some(scenarios) = body.get("scenarios") {
        let scenarios = scenarios
            .as_array()
            .ok_or_else(|| "derivatives_scenarios_invalid".to_string())?;
        if scenarios.len() > MAX_SCENARIOS {
            return Err("derivatives_scenarios_limit_exceeded".to_string());
        }
    }

    let Some(grid) = body
        .get("sensitivityGrid")
        .or_else(|| body.get("sensitivity_grid"))
    else {
        return Ok(());
    };
    let grid = grid
        .as_object()
        .ok_or_else(|| "derivatives_sensitivity_grid_invalid".to_string())?;
    let axis_length = |camel: &str, snake: &str| -> Result<usize, String> {
        grid.get(camel)
            .or_else(|| grid.get(snake))
            .and_then(Value::as_array)
            .map(Vec::len)
            .ok_or_else(|| "derivatives_sensitivity_grid_invalid".to_string())
    };
    let price_count = axis_length("priceShocksBps", "price_shocks_bps")?;
    let volatility_count = axis_length("volatilityShiftsPpm", "volatility_shifts_ppm")?;
    let elapsed_count = axis_length("elapsedSeconds", "elapsed_seconds")?;
    if price_count > MAX_SENSITIVITY_AXIS_VALUES
        || volatility_count > MAX_SENSITIVITY_AXIS_VALUES
        || elapsed_count > MAX_SENSITIVITY_AXIS_VALUES
    {
        return Err("derivatives_sensitivity_grid_limit_exceeded".to_string());
    }
    let point_count = price_count
        .checked_mul(volatility_count)
        .and_then(|count| count.checked_mul(elapsed_count))
        .ok_or_else(|| "derivatives_sensitivity_grid_limit_exceeded".to_string())?;
    if point_count > MAX_SENSITIVITY_POINTS {
        return Err("derivatives_sensitivity_grid_limit_exceeded".to_string());
    }
    Ok(())
}

pub fn get(service: &TradeAssemblyService, analysis_id: &str) -> ServiceResponse {
    if let Err(response) = service.require_object("derivatives_analysis", analysis_id) {
        return response;
    }
    match service.runtime().storage.get_json(ANALYSIS_NS, analysis_id) {
        Ok(Some(value)) if verify_record(&value).is_ok() => ServiceResponse::ok(value),
        Ok(Some(_)) => ServiceResponse::conflict("derivatives_evidence_integrity_failed"),
        Ok(None) => ServiceResponse::bad_request("derivatives_analysis_not_found"),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub(crate) fn verify_current_source(
    service: &TradeAssemblyService,
    value: &Value,
) -> Result<(), String> {
    verify_record(value)?;
    let analysis: DerivativesAnalysis = serde_json::from_value(value["analysis"].clone())
        .map_err(|_| "derivatives_evidence_integrity_failed".to_string())?;
    let source = source_artifacts(service, &analysis.source_bindings.run_id)?;
    let accounting_hash = canonical_hash(&source.accounting, "derivatives_accounting_hash_failed")?;
    let mut instruments = source.manifest.content.configuration.instruments.clone();
    instruments.sort_by(|left, right| left.instrument_id.cmp(&right.instrument_id));
    let instrument_configurations_hash = canonical_hash(
        &instruments,
        "derivatives_instrument_configurations_hash_failed",
    )?;
    let expected = SourceBindings {
        run_id: source.report.run.run_id.clone(),
        strategy_id: source.report.run.strategy_id.clone(),
        strategy_version_id: source.report.run.strategy_version_id.clone(),
        strategy_spec_hash: source.report.run.strategy_spec_hash.clone(),
        manifest_hash: source.manifest.manifest_hash.clone(),
        result_hash: source.report.metadata.result_hash.clone(),
        report_hash: source.report.report_hash.clone(),
        dataset_hash: source.snapshot.content_hash.clone(),
        accounting_hash,
        instrument_configurations_hash,
    };
    if analysis.source_bindings != expected
        || value["sourceRunId"].as_str() != Some(expected.run_id.as_str())
        || value["sourceReportHash"].as_str() != Some(expected.report_hash.as_str())
    {
        return Err("derivatives_source_integrity_failed".to_string());
    }
    Ok(())
}

pub fn list(service: &TradeAssemblyService) -> ServiceResponse {
    match service.runtime().storage.list_json(ANALYSIS_NS) {
        Ok(values) => {
            let mut analyses = values
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            analyses =
                service.filter_visible_values("derivatives_analysis", analyses, &["analysisId"]);
            if analyses.iter().any(|value| verify_record(value).is_err()) {
                return ServiceResponse::conflict("derivatives_evidence_integrity_failed");
            }
            analyses.sort_by(|left, right| {
                left["analysisId"]
                    .as_str()
                    .cmp(&right["analysisId"].as_str())
            });
            ServiceResponse::ok(json!({"analyses": analyses, "noAdvice": LEGAL_BOUNDARY}))
        }
        Err(code) => ServiceResponse::conflict(&code),
    }
}

pub fn export(service: &TradeAssemblyService, analysis_id: &str, format: &str) -> ServiceResponse {
    if let Err(response) = service.require_object("derivatives_analysis", analysis_id) {
        return response;
    }
    let Some(record) = service
        .runtime()
        .storage
        .get_json(ANALYSIS_NS, analysis_id)
        .ok()
        .flatten()
    else {
        return ServiceResponse::bad_request("derivatives_analysis_not_found");
    };
    if verify_record(&record).is_err() {
        return ServiceResponse::conflict("derivatives_evidence_integrity_failed");
    }
    let Some(artifact_ref) = record["exports"][format]["artifactRef"].as_str() else {
        return ServiceResponse::bad_request("derivatives_export_format_invalid");
    };
    match service.runtime().exports.read(artifact_ref) {
        Ok(bytes) => ServiceResponse::ok(json!({
            "analysisId": analysis_id,
            "format": format,
            "artifactRef": artifact_ref,
            "content": String::from_utf8_lossy(&bytes),
            "contentHash": record["exports"][format]["sha256"],
            "noAdvice": LEGAL_BOUNDARY,
        })),
        Err(code) => ServiceResponse::conflict(&code),
    }
}

fn source_artifacts(
    service: &TradeAssemblyService,
    run_id: &str,
) -> Result<SourceArtifacts, String> {
    service
        .require_object("backtest_run", run_id)
        .map_err(|_| "object_not_available".to_string())?;
    let runtime = service.runtime();
    let run = runtime
        .backtests
        .get_run(run_id)?
        .ok_or_else(|| "derivatives_source_not_found".to_string())?;
    if run.state != BacktestRunState::Completed {
        return Err("derivatives_source_not_completed".to_string());
    }
    let manifest = runtime
        .backtests
        .get_manifest(run_id)?
        .ok_or_else(|| "derivatives_source_integrity_failed".to_string())?;
    let result = runtime
        .backtests
        .get_result(run_id)?
        .ok_or_else(|| "derivatives_source_integrity_failed".to_string())?;
    let snapshot = runtime
        .dataset_snapshots
        .get(&manifest.content.configuration.dataset.dataset_id)?
        .ok_or_else(|| "derivatives_source_integrity_failed".to_string())?;
    let strategy = &manifest.content.configuration.strategy;
    let strategy_version = runtime
        .storage
        .list_json("strategy_versions")?
        .into_iter()
        .map(|(_, value)| value)
        .find(|value| {
            value.get("id").and_then(Value::as_str) == Some(strategy.strategy_version_id.as_str())
                && value
                    .get("strategyId")
                    .or_else(|| value.get("strategy_id"))
                    .and_then(Value::as_str)
                    == Some(strategy.strategy_id.as_str())
                && value
                    .get("spec")
                    .and_then(|spec| canonical_hash(spec, "derivatives_strategy_hash_failed").ok())
                    == Some(strategy.strategy_spec_hash.clone())
        })
        .ok_or_else(|| "derivatives_source_integrity_failed".to_string())?;
    let report = backtest_report::project_with_strategy_spec(
        &run,
        &manifest,
        &result,
        &snapshot,
        &runtime.backtests.list_attempts(run_id)?,
        &runtime.backtests.list_events(run_id)?,
        strategy_version.get("spec"),
    )?;
    let accounting = serde_json::from_value(result.content.accounting.clone())
        .map_err(|_| "derivatives_source_accounting_invalid".to_string())?;
    Ok(SourceArtifacts {
        manifest,
        report,
        snapshot,
        accounting,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_request(
    source: &SourceArtifacts,
    as_of_ms: i64,
    max_mark_age_ms: i64,
    max_greek_age_ms: i64,
    scenarios: Vec<DeclaredScenario>,
    sensitivity_grid: Option<SensitivityGrid>,
    pricing_model: Option<VersionedPricingModel>,
) -> Result<DerivativesAnalysisRequest, String> {
    let accounting_hash = canonical_hash(&source.accounting, "derivatives_accounting_hash_failed")?;
    let mut instruments = source.manifest.content.configuration.instruments.clone();
    instruments.sort_by(|left, right| left.instrument_id.cmp(&right.instrument_id));
    let instrument_configurations_hash = canonical_hash(
        &instruments,
        "derivatives_instrument_configurations_hash_failed",
    )?;
    let legs = source
        .report
        .trades
        .iter()
        .map(|trade| strategy_leg(trade, source))
        .collect::<Result<Vec<_>, _>>()?;
    if legs.is_empty() {
        return Err("derivatives_source_trades_unavailable".to_string());
    }
    let source_marks = source
        .accounting
        .positions
        .iter()
        .map(|(instrument_id, position)| {
            (
                instrument_id.clone(),
                SourcedPrice {
                    price_micros: position.market_price_micros,
                    observed_at_ms: latest_observation_ms(&source.snapshot).unwrap_or(as_of_ms),
                    provenance: Provenance {
                        source_ref: format!(
                            "tradeassembly://backtests/{}/accounting",
                            source.report.run.run_id
                        ),
                        content_hash: accounting_hash.clone(),
                    },
                },
            )
        })
        .collect();
    Ok(DerivativesAnalysisRequest {
        schema: DERIVATIVES_REQUEST_SCHEMA.to_string(),
        engine_version: ENGINE_VERSION.to_string(),
        as_of_ms,
        max_mark_age_ms,
        max_greek_age_ms,
        source_bindings: SourceBindings {
            run_id: source.report.run.run_id.clone(),
            strategy_id: source.report.run.strategy_id.clone(),
            strategy_version_id: source.report.run.strategy_version_id.clone(),
            strategy_spec_hash: source.report.run.strategy_spec_hash.clone(),
            manifest_hash: source.manifest.manifest_hash.clone(),
            result_hash: source.report.metadata.result_hash.clone(),
            report_hash: source.report.report_hash.clone(),
            dataset_hash: source.snapshot.content_hash.clone(),
            accounting_hash,
            instrument_configurations_hash,
        },
        legs,
        source_marks,
        scenarios,
        sensitivity_grid,
        pricing_model,
    })
}

fn strategy_leg(trade: &BacktestTrade, source: &SourceArtifacts) -> Result<StrategyLeg, String> {
    let configuration = source
        .manifest
        .content
        .configuration
        .instruments
        .iter()
        .find(|instrument| instrument.instrument_id == trade.symbol);
    let option_observation = source
        .snapshot
        .content
        .observations
        .iter()
        .filter(|observation| observation.instrument_id == trade.symbol)
        .filter_map(|observation| match &observation.data {
            HistoricalObservationData::OptionContract(option) => Some(option.as_ref()),
            HistoricalObservationData::Bar(_) => None,
        })
        .next_back();
    let configured_as_option = configuration.is_some_and(|instrument| {
        matches!(
            instrument.instrument_family.as_str(),
            "option" | "option_contract" | "equity_option"
        )
    });
    let instrument_kind = if configured_as_option || option_observation.is_some() {
        InstrumentKind::Option
    } else {
        InstrumentKind::Underlying
    };
    let underlying_instrument_id = option_observation
        .map(|option| option.underlying_instrument_id.clone())
        .or_else(|| {
            configuration
                .and_then(|instrument| instrument.metadata.get("underlyingInstrumentId").cloned())
        })
        .unwrap_or_else(|| trade.symbol.clone());
    let position_group_id = configuration
        .and_then(|instrument| {
            instrument
                .metadata
                .get("positionGroupId")
                .map(String::as_str)
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&trade.trade_id)
        .to_string();
    let entry_price_micros = trade.entry.get("priceMicros").and_then(Value::as_i64);
    let report_ref = Provenance {
        source_ref: format!(
            "tradeassembly://backtests/{}/report",
            source.report.run.run_id
        ),
        content_hash: source.report.report_hash.clone(),
    };
    let lifecycle = if let Some(exit) = &trade.exit {
        lifecycle_from_exit(exit, report_ref)?
    } else if let Some(terminal) = &trade.terminal_valuation {
        lifecycle_from_terminal_valuation(terminal, report_ref)?
    } else {
        LifecycleEvidence {
            state: LifecycleState::Open,
            occurred_at_ms: None,
            settlement_price_micros: None,
            settlement_cashflow_micros: None,
            terminal_value_micros: None,
            evidence_refs: Vec::new(),
        }
    };
    if lifecycle.state != LifecycleState::Open && lifecycle.occurred_at_ms.is_none() {
        return Err("derivatives_source_lifecycle_invalid".to_string());
    }
    Ok(StrategyLeg {
        leg_id: trade.trade_id.clone(),
        position_group_id,
        instrument_id: trade.symbol.clone(),
        underlying_instrument_id,
        instrument_kind,
        quantity_micros: trade.quantity_micros,
        entry_price_micros,
        report_trade_ids: vec![trade.trade_id.clone()],
        lifecycle,
    })
}

fn lifecycle_from_exit(exit: &Value, report_ref: Provenance) -> Result<LifecycleEvidence, String> {
    let state = match exit
        .get("lifecycleState")
        .or_else(|| exit.get("lifecycle_state"))
        .and_then(Value::as_str)
    {
        Some("closed") | None => LifecycleState::Closed,
        Some("exercised") => LifecycleState::Exercised,
        Some("assigned") => LifecycleState::Assigned,
        Some("expired") => LifecycleState::Expired,
        Some("settled") => LifecycleState::Settled,
        Some("terminal_valued") => LifecycleState::TerminalValued,
        Some(_) => return Err("derivatives_source_lifecycle_invalid".to_string()),
    };
    let occurred_at_ms = evidence_timestamp(exit)
        .ok_or_else(|| "derivatives_source_lifecycle_invalid".to_string())?;
    Ok(LifecycleEvidence {
        state,
        occurred_at_ms: Some(occurred_at_ms),
        settlement_price_micros: optional_i64(
            exit,
            "settlementPriceMicros",
            "settlement_price_micros",
        ),
        settlement_cashflow_micros: optional_i64(
            exit,
            "settlementCashflowMicros",
            "settlement_cashflow_micros",
        ),
        terminal_value_micros: optional_i64(exit, "terminalValueMicros", "terminal_value_micros"),
        evidence_refs: vec![report_ref],
    })
}

fn lifecycle_from_terminal_valuation(
    terminal: &Value,
    report_ref: Provenance,
) -> Result<LifecycleEvidence, String> {
    match terminal
        .get("lifecycleState")
        .or_else(|| terminal.get("lifecycle_state"))
        .and_then(Value::as_str)
    {
        Some("terminal_valued") | None => {}
        Some(_) => return Err("derivatives_source_lifecycle_invalid".to_string()),
    }
    let quantity = terminal
        .get("quantityMicros")
        .and_then(Value::as_i64)
        .ok_or_else(|| "derivatives_source_lifecycle_invalid".to_string())?;
    let price = terminal
        .get("priceMicros")
        .and_then(Value::as_i64)
        .ok_or_else(|| "derivatives_source_lifecycle_invalid".to_string())?;
    let occurred_at_ms = evidence_timestamp(terminal)
        .ok_or_else(|| "derivatives_source_lifecycle_invalid".to_string())?;
    let terminal_value_micros = i128::from(quantity)
        .checked_mul(i128::from(price))
        .and_then(|value| value.checked_div(1_000_000))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| "derivatives_source_lifecycle_invalid".to_string())?;
    Ok(LifecycleEvidence {
        state: LifecycleState::TerminalValued,
        occurred_at_ms: Some(occurred_at_ms),
        settlement_price_micros: None,
        settlement_cashflow_micros: None,
        terminal_value_micros: Some(terminal_value_micros),
        evidence_refs: vec![report_ref],
    })
}

fn optional_i64(value: &Value, camel: &str, snake: &str) -> Option<i64> {
    value
        .get(camel)
        .or_else(|| value.get(snake))
        .and_then(Value::as_i64)
}

fn declared_scenarios(body: &Value, source_run_id: &str) -> Result<Vec<DeclaredScenario>, String> {
    let Some(values) = body.get("scenarios") else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .ok_or_else(|| "derivatives_scenarios_invalid".to_string())?;
    let mut ids = BTreeSet::new();
    values
        .iter()
        .map(|value| {
            let scenario_id = value
                .get("scenarioId")
                .or_else(|| value.get("scenario_id"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "derivatives_scenarios_invalid".to_string())?;
            if !ids.insert(scenario_id.to_string()) {
                return Err("derivatives_scenarios_invalid".to_string());
            }
            let kind = serde_json::from_value::<ScenarioKind>(
                value
                    .get("kind")
                    .cloned()
                    .ok_or_else(|| "derivatives_scenarios_invalid".to_string())?,
            )
            .map_err(|_| "derivatives_scenarios_invalid".to_string())?;
            let underlying_prices_micros = typed_map(
                value
                    .get("underlyingPricesMicros")
                    .or_else(|| value.get("underlying_prices_micros")),
                "derivatives_scenarios_invalid",
            )?;
            if underlying_prices_micros.values().any(|value| *value < 0) {
                return Err("derivatives_scenarios_invalid".to_string());
            }
            let volatility_shifts_ppm = typed_map(
                value
                    .get("volatilityShiftsPpm")
                    .or_else(|| value.get("volatility_shifts_ppm")),
                "derivatives_scenarios_invalid",
            )?;
            let elapsed_seconds = value
                .get("elapsedSeconds")
                .or_else(|| value.get("elapsed_seconds"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if elapsed_seconds < 0 {
                return Err("derivatives_scenarios_invalid".to_string());
            }
            let declaration_payload = json!({
                "scenarioId": scenario_id,
                "kind": kind,
                "underlyingPricesMicros": underlying_prices_micros,
                "volatilityShiftsPpm": volatility_shifts_ppm,
                "elapsedSeconds": elapsed_seconds,
            });
            Ok(DeclaredScenario {
                scenario_id: scenario_id.to_string(),
                kind,
                underlying_prices_micros,
                volatility_shifts_ppm,
                elapsed_seconds,
                declaration: Provenance {
                    source_ref: format!(
                        "tradeassembly://backtests/{source_run_id}/derivatives/scenarios/{scenario_id}"
                    ),
                    content_hash: canonical_hash(
                        &declaration_payload,
                        "derivatives_scenario_hash_failed",
                    )?,
                },
            })
        })
        .collect()
}

fn declared_pricing_model(
    body: &Value,
    source_run_id: &str,
) -> Result<Option<VersionedPricingModel>, String> {
    let Some(value) = body
        .get("pricingModel")
        .or_else(|| body.get("pricing_model"))
    else {
        return Ok(None);
    };
    let mut payload = value.clone();
    let Some(object) = payload.as_object_mut() else {
        return Err("derivatives_pricing_model_invalid".to_string());
    };
    object.remove("inputProvenance");
    object.remove("input_provenance");
    let content_hash = canonical_hash(&payload, "derivatives_pricing_model_hash_failed")?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "derivatives_pricing_model_invalid".to_string())?;
    object.insert(
        "inputProvenance".to_string(),
        json!([{
            "sourceRef": format!(
                "tradeassembly://backtests/{source_run_id}/derivatives/pricing-model"
            ),
            "contentHash": content_hash,
        }]),
    );
    serde_json::from_value(payload)
        .map(Some)
        .map_err(|_| "derivatives_pricing_model_invalid".to_string())
}

fn write_exports(
    service: &TradeAssemblyService,
    analysis_id: &str,
    analysis: &DerivativesAnalysis,
) -> Result<Value, String> {
    let json_bytes = serde_json_canonicalizer::to_vec(analysis)
        .map_err(|_| "derivatives_export_serialization_failed".to_string())?;
    let csv_bytes = derivatives_csv(analysis).into_bytes();
    let json_artifact = service.runtime().exports.write(
        &format!("derivatives/{analysis_id}/analysis.json"),
        "application/json",
        &json_bytes,
    )?;
    let csv_artifact = service.runtime().exports.write(
        &format!("derivatives/{analysis_id}/analysis.csv"),
        "text/csv",
        &csv_bytes,
    )?;
    Ok(json!({
        "json": json_artifact,
        "csv": csv_artifact,
    }))
}

fn derivatives_csv(analysis: &DerivativesAnalysis) -> String {
    let mut output =
        "row_type,id,instrument_id,instrument_kind,calculation,status,value,reason\n".to_string();
    for leg in &analysis.legs {
        output.push_str(&csv_row(vec![
            "leg".to_string(),
            leg.leg_id.clone(),
            leg.instrument_id.clone(),
            format!("{:?}", leg.instrument_kind).to_ascii_lowercase(),
            "lifecycle".to_string(),
            if leg.lifecycle_complete {
                "available".to_string()
            } else {
                "unavailable".to_string()
            },
            String::new(),
            String::new(),
        ]));
    }
    for unavailable in &analysis.unavailable {
        output.push_str(&csv_row(vec![
            "unavailable".to_string(),
            unavailable.scope.clone(),
            String::new(),
            String::new(),
            unavailable.calculation.clone(),
            "unavailable".to_string(),
            String::new(),
            format!("{:?}", unavailable.reason).to_ascii_lowercase(),
        ]));
    }
    output
}

fn csv_row(values: Vec<String>) -> String {
    format!(
        "{}\n",
        values
            .iter()
            .map(|value| {
                let value = if value.starts_with(['=', '+', '-', '@']) {
                    format!("'{value}")
                } else {
                    value.to_string()
                };
                format!("\"{}\"", value.replace('"', "\"\""))
            })
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn verify_record(value: &Value) -> Result<(), String> {
    let request: DerivativesAnalysisRequest = serde_json::from_value(value["request"].clone())
        .map_err(|_| "derivatives_evidence_integrity_failed".to_string())?;
    let analysis: DerivativesAnalysis = serde_json::from_value(value["analysis"].clone())
        .map_err(|_| "derivatives_evidence_integrity_failed".to_string())?;
    verify_output_hash(&analysis)?;
    let request_hash = canonical_hash(&request, "derivatives_request_hash_failed")?;
    if value["requestHash"].as_str() != Some(request_hash.as_str())
        || request.source_bindings != analysis.source_bindings
    {
        return Err("derivatives_evidence_integrity_failed".to_string());
    }
    let identity_hash = canonical_hash(
        &json!({"requestHash": request_hash, "outputHash": analysis.output_hash}),
        "derivatives_identity_hash_failed",
    )?;
    let expected_id = format!(
        "derivatives-{}",
        identity_hash
            .strip_prefix("sha256:")
            .unwrap_or(&identity_hash)
    );
    if value["analysisId"].as_str() != Some(expected_id.as_str()) {
        return Err("derivatives_evidence_integrity_failed".to_string());
    }
    Ok(())
}

fn verify_output_hash(analysis: &DerivativesAnalysis) -> Result<(), String> {
    let mut unhashed = analysis.clone();
    unhashed.output_hash.clear();
    let expected = canonical_hash(&unhashed, "derivatives_analysis_hash_failed")?;
    if analysis.output_hash != expected {
        return Err("derivatives_evidence_integrity_failed".to_string());
    }
    Ok(())
}

fn latest_observation_ms(snapshot: &DatasetSnapshot) -> Result<i64, String> {
    snapshot
        .content
        .observations
        .iter()
        .map(|observation| {
            DateTime::parse_from_rfc3339(&observation.timestamp)
                .map(|value| value.timestamp_millis())
                .map_err(|_| "derivatives_source_timestamp_invalid".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .ok_or_else(|| "derivatives_source_observations_unavailable".to_string())
}

fn evidence_timestamp(value: &Value) -> Option<i64> {
    value
        .get("occurredAtMs")
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
                .map(|timestamp| timestamp.timestamp_millis())
        })
}

fn analysis_error(error: derivatives_analysis::AnalysisError) -> String {
    match error {
        derivatives_analysis::AnalysisError::InvalidRequest(field) => {
            format!("derivatives_request_invalid_{field}")
        }
        derivatives_analysis::AnalysisError::IntegrityFailure(field) => {
            format!("derivatives_source_integrity_failed_{field}")
        }
        derivatives_analysis::AnalysisError::LimitExceeded(field) => {
            format!("derivatives_limit_exceeded_{field}")
        }
        derivatives_analysis::AnalysisError::ArithmeticOverflow => {
            "derivatives_arithmetic_overflow".to_string()
        }
        derivatives_analysis::AnalysisError::SerializationFailure => {
            "derivatives_serialization_failed".to_string()
        }
    }
}

fn required_string<'a>(body: &'a Value, camel: &str, snake: &str) -> Result<&'a str, String> {
    body.get(camel)
        .or_else(|| body.get(snake))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "derivatives_request_invalid".to_string())
}

fn nonnegative_i64(body: &Value, camel: &str, snake: &str, fallback: i64) -> Result<i64, String> {
    let value = body
        .get(camel)
        .or_else(|| body.get(snake))
        .and_then(Value::as_i64)
        .unwrap_or(fallback);
    (value >= 0)
        .then_some(value)
        .ok_or_else(|| "derivatives_request_invalid".to_string())
}

fn optional_typed<T: serde::de::DeserializeOwned>(
    value: Option<&Value>,
    error: &str,
) -> Result<Option<T>, String> {
    value
        .cloned()
        .map(|value| serde_json::from_value(value).map_err(|_| error.to_string()))
        .transpose()
}

fn typed_map(value: Option<&Value>, error: &str) -> Result<BTreeMap<String, i64>, String> {
    value
        .cloned()
        .map(|value| serde_json::from_value(value).map_err(|_| error.to_string()))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn safe_token(body: &Value, key: &str, fallback: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "_-.@:".contains(character)
                })
        })
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> Provenance {
        Provenance {
            source_ref: "tradeassembly://backtests/run-1/report".to_string(),
            content_hash: "sha256:report".to_string(),
        }
    }

    #[test]
    fn exit_lifecycle_preserves_each_supported_terminal_state() {
        for (value, expected) in [
            ("closed", LifecycleState::Closed),
            ("exercised", LifecycleState::Exercised),
            ("assigned", LifecycleState::Assigned),
            ("expired", LifecycleState::Expired),
            ("settled", LifecycleState::Settled),
            ("terminal_valued", LifecycleState::TerminalValued),
        ] {
            let lifecycle = lifecycle_from_exit(
                &json!({
                    "lifecycleState": value,
                    "occurredAtMs": 42,
                    "settlementPriceMicros": 10,
                    "settlementCashflowMicros": 20,
                    "terminalValueMicros": 30,
                }),
                provenance(),
            )
            .expect("supported lifecycle");
            assert_eq!(lifecycle.state, expected);
            assert_eq!(lifecycle.occurred_at_ms, Some(42));
            assert_eq!(lifecycle.settlement_price_micros, Some(10));
            assert_eq!(lifecycle.settlement_cashflow_micros, Some(20));
            assert_eq!(lifecycle.terminal_value_micros, Some(30));
        }
    }

    #[test]
    fn exit_lifecycle_fails_closed_for_unknown_state_or_missing_timestamp() {
        assert_eq!(
            lifecycle_from_exit(
                &json!({"lifecycleState": "unknown", "occurredAtMs": 42}),
                provenance(),
            ),
            Err("derivatives_source_lifecycle_invalid".to_string())
        );
        assert_eq!(
            lifecycle_from_exit(&json!({"lifecycleState": "assigned"}), provenance()),
            Err("derivatives_source_lifecycle_invalid".to_string())
        );
    }

    #[test]
    fn terminal_valuation_requires_its_canonical_state_and_complete_facts() {
        let lifecycle = lifecycle_from_terminal_valuation(
            &json!({
                "lifecycleState": "terminal_valued",
                "occurredAtMs": 42,
                "quantityMicros": 2_000_000,
                "priceMicros": 3_000_000,
            }),
            provenance(),
        )
        .expect("canonical terminal valuation");
        assert_eq!(lifecycle.state, LifecycleState::TerminalValued);
        assert_eq!(lifecycle.occurred_at_ms, Some(42));
        assert_eq!(lifecycle.terminal_value_micros, Some(6_000_000));

        for invalid in [
            json!({
                "lifecycleState": "unknown",
                "occurredAtMs": 42,
                "quantityMicros": 1,
                "priceMicros": 1,
            }),
            json!({
                "lifecycleState": "closed",
                "occurredAtMs": 42,
                "quantityMicros": 1,
                "priceMicros": 1,
            }),
            json!({
                "lifecycleState": "terminal_valued",
                "quantityMicros": 1,
                "priceMicros": 1,
            }),
        ] {
            assert_eq!(
                lifecycle_from_terminal_valuation(&invalid, provenance()),
                Err("derivatives_source_lifecycle_invalid".to_string())
            );
        }
    }
}
