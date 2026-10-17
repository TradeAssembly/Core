//! Deterministic, integrity-bound read model and exports for one completed backtest.

use crate::backtest_accounting::{AccountingResult, Position};
use crate::backtest_contracts::{
    canonical_hash, BacktestAttempt, BacktestAttemptState, BacktestDiagnostic,
    BacktestLifecycleEvent, BacktestResult, BacktestRunManifest, BacktestRunRecord,
    DiagnosticSeverity,
};
use crate::domain::BacktestRunState;
use crate::historical_data::{
    AssetClass, DatasetSnapshot, DatasetSnapshotContent, HistoricalObservationData,
    OptionContractObservation,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};

pub const BACKTEST_REPORT_SCHEMA: &str = "tradeassembly.backtest-report.v2";
pub const BACKTEST_EXPORT_SCHEMA: &str = "tradeassembly.backtest-export.v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReport {
    pub schema: String,
    pub report_hash: String,
    pub run: BacktestReportRun,
    pub assumptions: BacktestReportAssumptions,
    pub overview: BacktestReportOverview,
    pub chart: BacktestReportChart,
    pub trades: Vec<BacktestTrade>,
    pub journal: Vec<BacktestJournalEvent>,
    pub orders: Vec<Value>,
    pub fills: Vec<Value>,
    pub positions: Vec<Value>,
    pub ledger: Vec<Value>,
    pub costs_and_risk: BacktestCostsAndRisk,
    pub diagnostics: Vec<BacktestReportDiagnostic>,
    pub metadata: BacktestReportMetadata,
    pub provenance: BacktestReportProvenance,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReportProvenance {
    pub strategy_version_id: String,
    pub strategy_spec_hash: String,
    pub strategy_execution_summary: Value,
    pub dataset_id: String,
    pub dataset_hash: String,
    pub source_class: String,
    pub source_plugin_ref: String,
    pub source_plugin_instance_ref: String,
    pub source_operation_id: String,
    pub source_manifest_fingerprint: String,
    pub time_coverage: crate::historical_data::DatasetTimeSlice,
    pub granularity: String,
}

impl BacktestReportProvenance {
    pub fn from_snapshot(
        snapshot: &DatasetSnapshot,
        strategy_version_id: impl Into<String>,
        strategy_spec_hash: impl Into<String>,
        strategy_execution_summary: Value,
    ) -> Self {
        let source = &snapshot.content.source;
        Self {
            strategy_version_id: strategy_version_id.into(),
            strategy_spec_hash: strategy_spec_hash.into(),
            strategy_execution_summary,
            dataset_id: snapshot.dataset_id.clone(),
            dataset_hash: snapshot.content_hash.clone(),
            source_class: serde_json::to_value(&source.source_class)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unavailable".to_string()),
            source_plugin_ref: source.plugin_ref.clone(),
            source_plugin_instance_ref: source.plugin_instance_ref.clone(),
            source_operation_id: source.operation_id.clone(),
            source_manifest_fingerprint: source.plugin_manifest_fingerprint.clone(),
            time_coverage: snapshot.content.time_slice.clone(),
            granularity: snapshot.content.granularity.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReportRun {
    pub run_id: String,
    pub state: BacktestRunState,
    pub strategy_id: String,
    pub strategy_version_id: String,
    pub strategy_spec_hash: String,
    pub created_at_ms: i64,
    pub completed_at_ms: i64,
    pub dataset_id: String,
    pub dataset_time_slice: Value,
    pub instruments: Vec<String>,
    pub source_plugin_ref: String,
    pub source_operation_id: String,
    pub source_manifest_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReportAssumptions {
    pub starting_cash_micros: i64,
    pub reporting_currency: String,
    pub account_model: Value,
    pub execution: Value,
    pub costs: Value,
    pub liquidity: Value,
    pub risk: Value,
    pub end_of_data: Value,
    pub evaluator_version: String,
    pub compiler_version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReportOverview {
    pub starting_equity_micros: i64,
    pub ending_cash_micros: i64,
    pub market_value_micros: i64,
    pub ending_equity_micros: i64,
    pub gross_pnl_micros: i64,
    pub net_pnl_micros: i64,
    pub realized_pnl_micros: i64,
    pub unrealized_pnl_micros: i64,
    pub return_bps: Option<i64>,
    pub trade_count: u64,
    pub win_count: u64,
    pub loss_count: u64,
    pub wash_count: u64,
    pub open_count: u64,
    pub fees_micros: i64,
    pub cost_drag_micros: i64,
    pub reconciliation_status: String,
    pub reconciliation_residual_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReportChart {
    pub range: Value,
    pub bars: Vec<Value>,
    pub markers: Vec<Value>,
    pub connectors: Vec<Value>,
    pub warnings: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestTrade {
    pub trade_id: String,
    pub symbol: String,
    pub instrument_type: String,
    pub instrument_details: BacktestInstrumentDetails,
    pub side: String,
    pub quantity_micros: i64,
    pub entry: Value,
    pub exit: Option<Value>,
    pub terminal_valuation: Option<Value>,
    pub realized: bool,
    pub pnl_micros: Option<i64>,
    pub fee_micros: i64,
    pub holding_interval_ms: Option<i64>,
    pub order_refs: Vec<String>,
    pub fill_refs: Vec<String>,
    pub ledger_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestInstrumentDetails {
    pub schema: String,
    pub instrument_id: String,
    pub asset_class: Option<AssetClass>,
    pub observed_at: Option<String>,
    pub fields: Vec<BacktestInstrumentField>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestInstrumentField {
    pub key: String,
    pub label: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestJournalEvent {
    pub event_id: String,
    pub event_type: String,
    pub timestamp_ms: i64,
    pub detail: String,
    pub from_state: BacktestRunState,
    pub to_state: BacktestRunState,
    pub attempt_id: String,
    pub evidence_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestCostsAndRisk {
    pub fees_micros: i64,
    pub cost_drag_micros: i64,
    pub maximum_loss_micros: i64,
    pub maximum_position_notional_micros: i64,
    pub maximum_order_quantity_micros: i64,
    pub unavailable: Vec<UnavailableMetric>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnavailableMetric {
    pub metric: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReportDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub detail: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestReportMetadata {
    pub manifest_hash: String,
    pub result_hash: String,
    pub dataset_hash: String,
    pub lifecycle_event_hashes: Vec<String>,
    pub current_attempt_id: String,
    pub replay_ref: String,
    pub available_export_formats: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BacktestExportArtifact {
    pub schema: String,
    pub export_kind: String,
    pub media_type: String,
    pub filename: String,
    pub byte_length: u64,
    pub content_hash: String,
    pub source_run_hash: String,
    pub source_manifest_hash: String,
    pub source_result_hash: String,
    pub source_dataset_hash: String,
    pub source_report_hash: String,
    pub content: String,
}

pub fn project(
    run: &BacktestRunRecord,
    manifest: &BacktestRunManifest,
    result: &BacktestResult,
    snapshot: &DatasetSnapshot,
    attempts: &[BacktestAttempt],
    events: &[BacktestLifecycleEvent],
) -> Result<BacktestReport, String> {
    project_with_strategy_spec(run, manifest, result, snapshot, attempts, events, None)
}

pub fn project_with_strategy_spec(
    run: &BacktestRunRecord,
    manifest: &BacktestRunManifest,
    result: &BacktestResult,
    snapshot: &DatasetSnapshot,
    attempts: &[BacktestAttempt],
    events: &[BacktestLifecycleEvent],
    strategy_spec: Option<&Value>,
) -> Result<BacktestReport, String> {
    verify_inputs(run, manifest, result, snapshot, attempts, events)?;
    let accounting: AccountingResult = serde_json::from_value(result.content.accounting.clone())
        .map_err(|_| "backtest_report_accounting_invalid".to_string())?;
    let trades = project_trades(&result.content.fills, &accounting.positions, snapshot)?;
    let overview = overview(manifest, &accounting, &trades)?;
    let chart = chart(snapshot, &trades, &manifest.content.evidence_refs);
    let diagnostics = diagnostics(
        &manifest.content.validation.diagnostics,
        &result.content.diagnostics,
        snapshot,
        events,
    );
    let mut report = BacktestReport {
        schema: BACKTEST_REPORT_SCHEMA.to_string(),
        report_hash: String::new(),
        run: BacktestReportRun {
            run_id: run.run_id.clone(),
            state: run.state,
            strategy_id: manifest.content.configuration.strategy.strategy_id.clone(),
            strategy_version_id: manifest
                .content
                .configuration
                .strategy
                .strategy_version_id
                .clone(),
            strategy_spec_hash: manifest
                .content
                .configuration
                .strategy
                .strategy_spec_hash
                .clone(),
            created_at_ms: run.created_at_ms,
            completed_at_ms: run.updated_at_ms,
            dataset_id: snapshot.dataset_id.clone(),
            dataset_time_slice: serde_json::to_value(&snapshot.content.time_slice)
                .map_err(|_| "backtest_report_serialization_failed".to_string())?,
            instruments: snapshot.content.instruments.clone(),
            source_plugin_ref: snapshot.content.source.plugin_ref.clone(),
            source_operation_id: snapshot.content.source.operation_id.clone(),
            source_manifest_fingerprint: snapshot
                .content
                .source
                .plugin_manifest_fingerprint
                .clone(),
        },
        assumptions: BacktestReportAssumptions {
            starting_cash_micros: manifest.content.configuration.capital.starting_cash_micros,
            reporting_currency: manifest
                .content
                .configuration
                .capital
                .reporting_currency
                .clone(),
            account_model: serde_json::to_value(
                &manifest.content.configuration.capital.account_model,
            )
            .map_err(|_| "backtest_report_serialization_failed".to_string())?,
            execution: serde_json::to_value(&manifest.content.configuration.execution)
                .map_err(|_| "backtest_report_serialization_failed".to_string())?,
            costs: serde_json::to_value(&manifest.content.configuration.costs)
                .map_err(|_| "backtest_report_serialization_failed".to_string())?,
            liquidity: serde_json::to_value(&manifest.content.configuration.liquidity)
                .map_err(|_| "backtest_report_serialization_failed".to_string())?,
            risk: serde_json::to_value(&manifest.content.configuration.risk)
                .map_err(|_| "backtest_report_serialization_failed".to_string())?,
            end_of_data: serde_json::to_value(&manifest.content.configuration.end_of_data)
                .map_err(|_| "backtest_report_serialization_failed".to_string())?,
            evaluator_version: result.content.evaluator_version.clone(),
            compiler_version: result.content.compiler_version.clone(),
        },
        overview,
        chart,
        trades,
        journal: journal(events),
        orders: result.content.orders.clone(),
        fills: result.content.fills.clone(),
        positions: result.content.positions.clone(),
        ledger: result.content.ledger.clone(),
        costs_and_risk: BacktestCostsAndRisk {
            fees_micros: accounting.costs_micros,
            cost_drag_micros: accounting.costs_micros,
            maximum_loss_micros: manifest.content.configuration.risk.maximum_loss_micros,
            maximum_position_notional_micros: manifest
                .content
                .configuration
                .risk
                .maximum_position_notional_micros,
            maximum_order_quantity_micros: manifest
                .content
                .configuration
                .risk
                .maximum_order_quantity_micros,
            unavailable: vec![
                unavailable(
                    "max_drawdown_bps",
                    "equity series is not emitted by the accepted engine",
                ),
                unavailable(
                    "sharpe_ratio",
                    "return series is not emitted by the accepted engine",
                ),
                unavailable(
                    "trade_latency",
                    "execution latency is not emitted by the accepted engine",
                ),
            ],
        },
        diagnostics,
        metadata: BacktestReportMetadata {
            manifest_hash: manifest.manifest_hash.clone(),
            result_hash: result.result_hash.clone(),
            dataset_hash: snapshot.content_hash.clone(),
            lifecycle_event_hashes: events
                .iter()
                .map(|event| event.event_hash.clone())
                .collect(),
            current_attempt_id: run.current_attempt_id.clone(),
            replay_ref: format!("tradeassembly://backtests/{}/replay", run.run_id),
            available_export_formats: export_kinds()
                .iter()
                .map(|kind| (*kind).to_string())
                .collect(),
            warnings: Vec::new(),
        },
        provenance: BacktestReportProvenance::from_snapshot(
            snapshot,
            manifest
                .content
                .configuration
                .strategy
                .strategy_version_id
                .clone(),
            manifest
                .content
                .configuration
                .strategy
                .strategy_spec_hash
                .clone(),
            strategy_execution_summary(
                strategy_spec,
                &manifest.content.configuration.strategy.strategy_spec_hash,
            ),
        ),
    };
    report.report_hash =
        canonical_hash(&report_without_hash(&report), "backtest_report_hash_failed")?;
    Ok(report)
}

pub fn export(
    report: &BacktestReport,
    manifest: &BacktestRunManifest,
    kind: &str,
) -> Result<BacktestExportArtifact, String> {
    if !export_kinds().contains(&kind) {
        return Err("backtest_export_kind_invalid".to_string());
    }
    let (media_type, filename, content) = match kind {
        "manifestJson" => (
            "application/json",
            format!("{}-manifest.json", report.run.run_id),
            canonical_json(manifest)?,
        ),
        "reportJson" => (
            "application/json",
            format!("{}-report.json", report.run.run_id),
            canonical_json(report)?,
        ),
        "tradesCsv" => (
            "text/csv",
            format!("{}-trades.csv", report.run.run_id),
            trades_csv(report),
        ),
        "ordersFillsCsv" => (
            "text/csv",
            format!("{}-orders-fills.csv", report.run.run_id),
            orders_fills_csv(report),
        ),
        "positionsLedgerCsv" => (
            "text/csv",
            format!("{}-positions-ledger.csv", report.run.run_id),
            positions_ledger_csv(report),
        ),
        "equityCsv" => (
            "text/csv",
            format!("{}-equity.csv", report.run.run_id),
            equity_csv(report),
        ),
        _ => return Err("backtest_export_kind_invalid".to_string()),
    };
    let content_hash = sha256(&content);
    Ok(BacktestExportArtifact {
        schema: BACKTEST_EXPORT_SCHEMA.to_string(),
        export_kind: kind.to_string(),
        media_type: media_type.to_string(),
        filename,
        byte_length: content.len() as u64,
        content_hash,
        source_run_hash: canonical_hash(&report.run, "backtest_export_hash_failed")?,
        source_manifest_hash: report.metadata.manifest_hash.clone(),
        source_result_hash: report.metadata.result_hash.clone(),
        source_dataset_hash: report.metadata.dataset_hash.clone(),
        source_report_hash: report.report_hash.clone(),
        content,
    })
}

fn strategy_execution_summary(strategy_spec: Option<&Value>, expected_hash: &str) -> Value {
    let Some(spec) = strategy_spec else {
        return serde_json::json!({
            "status": "unavailable",
            "reason": "verified immutable strategy spec was not available",
            "indicators": [],
            "rules": [],
        });
    };
    let Ok(actual_hash) = canonical_hash(spec, "backtest_strategy_hash_failed") else {
        return serde_json::json!({
            "status": "unavailable",
            "reason": "immutable strategy spec could not be canonically hashed",
            "indicators": [],
            "rules": [],
        });
    };
    if actual_hash != expected_hash {
        return serde_json::json!({
            "status": "unavailable",
            "reason": "immutable strategy spec hash did not match the manifest",
            "indicators": [],
            "rules": [],
        });
    }
    serde_json::json!({
        "status": "verified",
        "source": "immutable_strategy_version_spec",
        "indicators": spec.get("indicators").cloned().unwrap_or_else(|| serde_json::json!([])),
        "rules": spec.get("rules").cloned().unwrap_or_else(|| serde_json::json!([])),
        "stages": spec.get("stages").cloned().unwrap_or(Value::Null),
        "capabilityRequirements": spec.get("capability_requirements").cloned().unwrap_or(Value::Null),
    })
}

pub fn export_kinds() -> &'static [&'static str] {
    &[
        "manifestJson",
        "reportJson",
        "tradesCsv",
        "ordersFillsCsv",
        "positionsLedgerCsv",
        "equityCsv",
    ]
}

fn verify_inputs(
    run: &BacktestRunRecord,
    manifest: &BacktestRunManifest,
    result: &BacktestResult,
    snapshot: &DatasetSnapshot,
    attempts: &[BacktestAttempt],
    events: &[BacktestLifecycleEvent],
) -> Result<(), String> {
    manifest.verify()?;
    result.verify()?;
    snapshot.verify()?;
    if run.state != BacktestRunState::Completed
        || run.run_id != manifest.content.run_id
        || run.manifest_hash != manifest.manifest_hash
        || run.result_hash.as_deref() != Some(result.result_hash.as_str())
        || run.result.as_ref() != Some(result)
        || result.content.run_id != run.run_id
        || result.content.manifest_hash != manifest.manifest_hash
        || snapshot.dataset_id != manifest.content.configuration.dataset.dataset_id
        || snapshot.snapshot_ref != manifest.content.configuration.dataset.snapshot_ref
        || snapshot.content_hash != manifest.content.configuration.dataset.content_hash
        || snapshot.content.strategy_id != manifest.content.configuration.strategy.strategy_id
    {
        return Err("backtest_report_integrity_failed".to_string());
    }
    let current_attempt = attempts
        .iter()
        .find(|attempt| attempt.attempt_id == run.current_attempt_id);
    if current_attempt.is_none_or(|attempt| {
        attempt.run_id != run.run_id || attempt.state != BacktestAttemptState::Completed
    }) {
        return Err("backtest_report_attempt_integrity_failed".to_string());
    }
    if events != run.events
        || events.is_empty()
        || events
            .last()
            .is_none_or(|event| event.to_state != BacktestRunState::Completed)
    {
        return Err("backtest_report_lifecycle_integrity_failed".to_string());
    }
    let mut previous = None;
    for (index, event) in events.iter().enumerate() {
        let expected = canonical_hash(
            &json!({"runId":event.run_id,"sequence":event.sequence,"from":event.from_state,"to":event.to_state,"attemptId":event.attempt_id,"previous":event.previous_event_hash,"detail":event.detail}),
            "backtest_event_hash_failed",
        )?;
        if event.run_id != run.run_id
            || event.sequence != index as i64 + 1
            || event.previous_event_hash != previous
            || event.event_hash != expected
        {
            return Err("backtest_report_lifecycle_integrity_failed".to_string());
        }
        previous = Some(event.event_hash.clone());
    }
    Ok(())
}

fn project_trades(
    fills: &[Value],
    positions: &BTreeMap<String, Position>,
    snapshot: &DatasetSnapshot,
) -> Result<Vec<BacktestTrade>, String> {
    let mut open = BTreeMap::<String, VecDeque<OpenLot>>::new();
    let mut trades = Vec::new();
    for fill in fills {
        let symbol = required_text(fill, "symbol")?;
        let side = required_text(fill, "side")?;
        let quantity = required_i64(fill, "quantityMicros")?;
        let price = required_i64(fill, "priceMicros")?;
        let fee = required_i64(fill, "feeMicros")?;
        let fill_id = required_text(fill, "fillId")?;
        let timestamp = fill
            .get("executionTimestamp")
            .and_then(Value::as_str)
            .map(str::to_string);
        if side == "buy" {
            open.entry(symbol).or_default().push_back(OpenLot {
                fill_id,
                quantity,
                price,
                fee,
                timestamp,
            });
            continue;
        }
        if side != "sell" {
            return Err("backtest_report_fill_invalid".to_string());
        }
        let mut remaining = quantity;
        while remaining > 0 {
            let lot = open
                .get_mut(&symbol)
                .and_then(VecDeque::front_mut)
                .ok_or_else(|| "backtest_report_fill_invalid".to_string())?;
            let matched = lot.quantity.min(remaining);
            let entry = marker(
                &lot.fill_id,
                &symbol,
                "entry",
                matched,
                lot.price,
                lot.timestamp.as_deref(),
                false,
            );
            let exit = marker(
                &fill_id,
                &symbol,
                "exit",
                matched,
                price,
                timestamp.as_deref(),
                fill.get("endOfData")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            );
            let entry_fee = prorate(lot.fee, matched, lot.quantity)?;
            let exit_fee = prorate(fee, matched, quantity)?;
            let pnl =
                notional(matched, price)? - notional(matched, lot.price)? - entry_fee - exit_fee;
            let instrument_details =
                instrument_details(snapshot, &symbol, lot.timestamp.as_deref())?;
            let instrument_type = instrument_details
                .asset_class
                .as_ref()
                .map(asset_class_name)
                .unwrap_or("unspecified")
                .to_string();
            trades.push(BacktestTrade {
                trade_id: format!("{}:{}:{}", symbol, lot.fill_id, fill_id),
                instrument_type,
                instrument_details,
                symbol: symbol.clone(),
                side: "long".to_string(),
                quantity_micros: matched,
                entry,
                exit: Some(exit),
                terminal_valuation: None,
                realized: true,
                pnl_micros: Some(pnl),
                fee_micros: entry_fee + exit_fee,
                holding_interval_ms: None,
                order_refs: Vec::new(),
                fill_refs: vec![lot.fill_id.clone(), fill_id.clone()],
                ledger_refs: ledger_refs(&[lot.fill_id.clone(), fill_id.clone()]),
            });
            lot.quantity -= matched;
            remaining -= matched;
            if lot.quantity == 0 {
                open.get_mut(&symbol).expect("open lot exists").pop_front();
            }
        }
    }
    for (symbol, lots) in open {
        if lots.is_empty() {
            continue;
        }
        let Some(position) = positions.get(&symbol) else {
            return Err("backtest_report_position_integrity_failed".to_string());
        };
        for lot in lots {
            let value = marker(
                &format!("{}:valuation", lot.fill_id),
                &symbol,
                "terminal_valuation",
                lot.quantity,
                position.market_price_micros,
                Some(&snapshot.last_timestamp),
                true,
            );
            let pnl = notional(lot.quantity, position.market_price_micros)?
                - notional(lot.quantity, lot.price)?
                - lot.fee;
            let instrument_details =
                instrument_details(snapshot, &symbol, lot.timestamp.as_deref())?;
            let instrument_type = instrument_details
                .asset_class
                .as_ref()
                .map(asset_class_name)
                .unwrap_or("unspecified")
                .to_string();
            trades.push(BacktestTrade {
                trade_id: format!("{}:{}:open", symbol, lot.fill_id),
                instrument_type,
                instrument_details,
                symbol: symbol.clone(),
                side: "long".to_string(),
                quantity_micros: lot.quantity,
                entry: marker(
                    &lot.fill_id,
                    &symbol,
                    "entry",
                    lot.quantity,
                    lot.price,
                    lot.timestamp.as_deref(),
                    false,
                ),
                exit: None,
                terminal_valuation: Some(value),
                realized: false,
                pnl_micros: Some(pnl),
                fee_micros: lot.fee,
                holding_interval_ms: None,
                order_refs: Vec::new(),
                fill_refs: vec![lot.fill_id.clone()],
                ledger_refs: ledger_refs(&[lot.fill_id]),
            });
        }
    }
    trades.sort_by(|left, right| left.trade_id.cmp(&right.trade_id));
    Ok(trades)
}

fn instrument_details(
    snapshot: &DatasetSnapshot,
    symbol: &str,
    observed_at: Option<&str>,
) -> Result<BacktestInstrumentDetails, String> {
    instrument_details_from_content(&snapshot.content, symbol, observed_at)
}

fn instrument_details_from_content(
    content: &DatasetSnapshotContent,
    symbol: &str,
    observed_at: Option<&str>,
) -> Result<BacktestInstrumentDetails, String> {
    let matching = content
        .observations
        .iter()
        .filter(|observation| observation.instrument_id == symbol)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Err("backtest_report_instrument_integrity_failed".to_string());
    }

    match &matching[0].data {
        HistoricalObservationData::Bar(_) => {
            if matching
                .iter()
                .any(|observation| !matches!(observation.data, HistoricalObservationData::Bar(_)))
            {
                return Err("backtest_report_instrument_integrity_failed".to_string());
            }
            let asset_class = unambiguous_bar_asset_class(content);
            let mut fields = vec![instrument_field("instrument_id", "Instrument", symbol)];
            if let Some(class) = asset_class.as_ref() {
                fields.push(instrument_field(
                    "asset_class",
                    "Asset class",
                    asset_class_name(class),
                ));
            }
            Ok(BacktestInstrumentDetails {
                schema: "tradeassembly.instrument.bar.v1".to_string(),
                instrument_id: symbol.to_string(),
                asset_class,
                observed_at: None,
                fields,
            })
        }
        HistoricalObservationData::OptionContract(_) => {
            let timestamp = observed_at
                .ok_or_else(|| "backtest_report_instrument_integrity_failed".to_string())?;
            let observation = matching
                .into_iter()
                .find(|observation| observation.timestamp == timestamp)
                .ok_or_else(|| "backtest_report_instrument_integrity_failed".to_string())?;
            let HistoricalObservationData::OptionContract(contract) = &observation.data else {
                return Err("backtest_report_instrument_integrity_failed".to_string());
            };
            if contract.contract_symbol != symbol {
                return Err("backtest_report_instrument_integrity_failed".to_string());
            }
            Ok(option_instrument_details(contract, timestamp))
        }
    }
}

fn unambiguous_bar_asset_class(content: &DatasetSnapshotContent) -> Option<AssetClass> {
    let classes = content
        .asset_classes
        .iter()
        .filter(|class| **class != AssetClass::Option)
        .collect::<Vec<_>>();
    (classes.len() == 1).then(|| classes[0].clone())
}

fn option_instrument_details(
    contract: &OptionContractObservation,
    observed_at: &str,
) -> BacktestInstrumentDetails {
    let mut fields = vec![
        instrument_field(
            "contract_symbol",
            "Contract",
            contract.contract_symbol.clone(),
        ),
        instrument_field(
            "underlying_instrument_id",
            "Underlying",
            contract.underlying_instrument_id.clone(),
        ),
        instrument_field("expiration", "Expiration", contract.expiration.clone()),
        instrument_field("strike", "Strike", contract.strike),
        instrument_field(
            "right",
            "Right",
            serde_json::to_value(&contract.right).expect("option right serializable"),
        ),
    ];
    push_optional_field(&mut fields, "bid", "Bid", contract.bid);
    push_optional_field(&mut fields, "ask", "Ask", contract.ask);
    push_optional_field(&mut fields, "last", "Last", contract.last);
    push_optional_field(&mut fields, "volume", "Volume", contract.volume);
    push_optional_field(
        &mut fields,
        "open_interest",
        "Open interest",
        contract.open_interest,
    );
    push_optional_field(
        &mut fields,
        "implied_volatility",
        "Implied volatility",
        contract.implied_volatility,
    );
    if let Some(greeks) = &contract.greeks {
        push_optional_field(&mut fields, "delta", "Delta", greeks.delta);
        push_optional_field(&mut fields, "gamma", "Gamma", greeks.gamma);
        push_optional_field(&mut fields, "theta", "Theta", greeks.theta);
        push_optional_field(&mut fields, "vega", "Vega", greeks.vega);
        push_optional_field(&mut fields, "rho", "Rho", greeks.rho);
    }
    BacktestInstrumentDetails {
        schema: "tradeassembly.instrument.option-contract.v1".to_string(),
        instrument_id: contract.contract_symbol.clone(),
        asset_class: Some(AssetClass::Option),
        observed_at: Some(observed_at.to_string()),
        fields,
    }
}

fn instrument_field(
    key: impl Into<String>,
    label: impl Into<String>,
    value: impl Into<Value>,
) -> BacktestInstrumentField {
    BacktestInstrumentField {
        key: key.into(),
        label: label.into(),
        value: value.into(),
    }
}

fn push_optional_field(
    fields: &mut Vec<BacktestInstrumentField>,
    key: &str,
    label: &str,
    value: Option<f64>,
) {
    if let Some(value) = value {
        fields.push(instrument_field(key, label, value));
    }
}

fn asset_class_name(asset_class: &AssetClass) -> &'static str {
    match asset_class {
        AssetClass::Equity => "equity",
        AssetClass::Etf => "etf",
        AssetClass::CryptoSpot => "crypto_spot",
        AssetClass::Option => "option",
    }
}

#[derive(Clone)]
struct OpenLot {
    fill_id: String,
    quantity: i64,
    price: i64,
    fee: i64,
    timestamp: Option<String>,
}

fn overview(
    manifest: &BacktestRunManifest,
    accounting: &AccountingResult,
    trades: &[BacktestTrade],
) -> Result<BacktestReportOverview, String> {
    let start = manifest.content.configuration.capital.starting_cash_micros;
    let net = accounting
        .ending_equity_micros
        .checked_sub(start)
        .ok_or_else(|| "backtest_report_accounting_invalid".to_string())?;
    let gross = net
        .checked_add(accounting.costs_micros)
        .ok_or_else(|| "backtest_report_accounting_invalid".to_string())?;
    let (mut wins, mut losses, mut washes, mut opens) = (0, 0, 0, 0);
    for trade in trades {
        match (trade.realized, trade.pnl_micros) {
            (false, _) => opens += 1,
            (true, Some(value)) if value > 0 => wins += 1,
            (true, Some(value)) if value < 0 => losses += 1,
            (true, Some(_)) => washes += 1,
            _ => {}
        }
    }
    Ok(BacktestReportOverview {
        starting_equity_micros: start,
        ending_cash_micros: accounting.ending_cash_micros,
        market_value_micros: accounting.market_value_micros,
        ending_equity_micros: accounting.ending_equity_micros,
        gross_pnl_micros: gross,
        net_pnl_micros: net,
        realized_pnl_micros: accounting.realized_pnl_micros,
        unrealized_pnl_micros: accounting.unrealized_pnl_micros,
        return_bps: if start == 0 {
            None
        } else {
            Some(
                i64::try_from(i128::from(net) * 10_000 / i128::from(start))
                    .map_err(|_| "backtest_report_accounting_invalid".to_string())?,
            )
        },
        trade_count: trades.len() as u64,
        win_count: wins,
        loss_count: losses,
        wash_count: washes,
        open_count: opens,
        fees_micros: accounting.costs_micros,
        cost_drag_micros: accounting.costs_micros,
        reconciliation_status: if accounting.reconciliation_residual_micros == 0 {
            "reconciled".to_string()
        } else {
            "unreconciled".to_string()
        },
        reconciliation_residual_micros: accounting.reconciliation_residual_micros,
    })
}

fn chart(
    snapshot: &DatasetSnapshot,
    trades: &[BacktestTrade],
    evidence_refs: &[String],
) -> BacktestReportChart {
    let bars = snapshot.content.observations.iter().filter_map(|row| match &row.data { HistoricalObservationData::Bar(bar) => Some(json!({"timestamp":row.timestamp,"symbol":row.instrument_id,"open":bar.open,"high":bar.high,"low":bar.low,"close":bar.close,"volume":bar.volume})), _ => None }).collect();
    let mut markers = Vec::new();
    let mut connectors = Vec::new();
    for trade in trades {
        let entry = chart_marker(&trade.entry, snapshot);
        markers.push(entry.clone());
        if let Some(exit) = &trade.exit {
            let exit = chart_marker(exit, snapshot);
            markers.push(exit.clone());
            connectors.push(json!({
                "tradeId":trade.trade_id,
                "entryFillId":entry["fillId"],
                "exitFillId":exit["fillId"],
                "from":chart_coordinate(&entry),
                "to":chart_coordinate(&exit),
                "realized":true,
                "pnlMicros":trade.pnl_micros,
                "tone":tone(trade.pnl_micros)
            }));
        }
        if let Some(value) = &trade.terminal_valuation {
            let value = chart_marker(value, snapshot);
            markers.push(value.clone());
            connectors.push(json!({
                "tradeId":trade.trade_id,
                "entryFillId":entry["fillId"],
                "valuationFillId":value["fillId"],
                "from":chart_coordinate(&entry),
                "to":chart_coordinate(&value),
                "realized":false,
                "pnlMicros":trade.pnl_micros,
                "tone":"unrealized"
            }));
        }
    }
    BacktestReportChart {
        range: json!({"start":snapshot.first_timestamp,"end":snapshot.last_timestamp}),
        bars,
        markers,
        connectors,
        warnings: snapshot
            .content
            .quality_findings
            .iter()
            .filter(|finding| {
                !matches!(
                    finding.severity,
                    crate::historical_data::DatasetQualitySeverity::Info
                )
            })
            .map(|finding| finding.code.clone())
            .collect(),
        evidence_refs: evidence_refs.to_vec(),
    }
}

fn chart_marker(marker: &Value, snapshot: &DatasetSnapshot) -> Value {
    let mut marker = marker.clone();
    let price = marker["priceMicros"].as_i64().unwrap_or_default() as f64 / 1_000_000.0;
    marker["price"] = json!(price);
    if marker["timestamp"].is_null() {
        marker["timestamp"] = json!(snapshot.last_timestamp);
    }
    marker
}

fn chart_coordinate(marker: &Value) -> Value {
    json!({
        "timestamp": marker["timestamp"],
        "price": marker["price"],
        "fillId": marker["fillId"],
    })
}

fn diagnostics(
    validation: &[BacktestDiagnostic],
    result: &[BacktestDiagnostic],
    snapshot: &DatasetSnapshot,
    events: &[BacktestLifecycleEvent],
) -> Vec<BacktestReportDiagnostic> {
    let mut diagnostics = Vec::new();
    for diagnostic in validation.iter().chain(result) {
        diagnostics.push(BacktestReportDiagnostic {
            severity: diagnostic.severity.clone(),
            code: diagnostic.code.clone(),
            detail: diagnostic.detail.clone(),
            evidence_refs: Vec::new(),
        });
    }
    for finding in &snapshot.content.quality_findings {
        diagnostics.push(BacktestReportDiagnostic {
            severity: match finding.severity {
                crate::historical_data::DatasetQualitySeverity::Info => DiagnosticSeverity::Info,
                crate::historical_data::DatasetQualitySeverity::Warning => {
                    DiagnosticSeverity::Warning
                }
                crate::historical_data::DatasetQualitySeverity::Error => DiagnosticSeverity::Error,
            },
            code: finding.code.clone(),
            detail: finding.detail.clone(),
            evidence_refs: Vec::new(),
        });
    }
    diagnostics.push(BacktestReportDiagnostic {
        severity: DiagnosticSeverity::Info,
        code: "backtest_lifecycle_verified".to_string(),
        detail: format!("{} immutable lifecycle events verified", events.len()),
        evidence_refs: events
            .iter()
            .map(|event| event.event_hash.clone())
            .collect(),
    });
    diagnostics
        .sort_by(|left, right| (&left.code, &left.detail).cmp(&(&right.code, &right.detail)));
    diagnostics
}

fn journal(events: &[BacktestLifecycleEvent]) -> Vec<BacktestJournalEvent> {
    events
        .iter()
        .map(|event| BacktestJournalEvent {
            event_id: event.event_id.clone(),
            event_type: event.event_type.clone(),
            timestamp_ms: event.occurred_at_ms,
            detail: event.detail.clone(),
            from_state: event.from_state,
            to_state: event.to_state,
            attempt_id: event.attempt_id.clone(),
            evidence_hash: event.event_hash.clone(),
        })
        .collect()
}

fn report_without_hash(report: &BacktestReport) -> Value {
    let mut value = serde_json::to_value(report).expect("report serializable");
    value["reportHash"] = Value::String(String::new());
    value
}
fn canonical_json<T: Serialize>(value: &T) -> Result<String, String> {
    String::from_utf8(
        serde_json_canonicalizer::to_vec(value)
            .map_err(|_| "backtest_export_serialization_failed".to_string())?,
    )
    .map_err(|_| "backtest_export_serialization_failed".to_string())
}
fn sha256(content: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}
fn unavailable(metric: &str, reason: &str) -> UnavailableMetric {
    UnavailableMetric {
        metric: metric.to_string(),
        reason: reason.to_string(),
    }
}
fn required_text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "backtest_report_fill_invalid".to_string())
}
fn required_i64(value: &Value, key: &str) -> Result<i64, String> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| "backtest_report_fill_invalid".to_string())
}
fn notional(quantity: i64, price: i64) -> Result<i64, String> {
    i64::try_from(
        i128::from(quantity)
            .checked_mul(i128::from(price))
            .ok_or_else(|| "backtest_report_accounting_invalid".to_string())?
            / 1_000_000,
    )
    .map_err(|_| "backtest_report_accounting_invalid".to_string())
}
fn prorate(total: i64, quantity: i64, whole: i64) -> Result<i64, String> {
    if whole <= 0 {
        return Err("backtest_report_fill_invalid".to_string());
    }
    i64::try_from(i128::from(total) * i128::from(quantity) / i128::from(whole))
        .map_err(|_| "backtest_report_accounting_invalid".to_string())
}
fn marker(
    fill_id: &str,
    symbol: &str,
    kind: &str,
    quantity: i64,
    price: i64,
    timestamp: Option<&str>,
    terminal: bool,
) -> Value {
    let lifecycle_state = match kind {
        "exit" => "closed",
        "terminal_valuation" => "terminal_valued",
        _ => "open",
    };
    let occurred_at_ms = timestamp
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis());
    json!({
        "fillId":fill_id,
        "symbol":symbol,
        "kind":kind,
        "quantityMicros":quantity,
        "priceMicros":price,
        "timestamp":timestamp,
        "terminal":terminal,
        "lifecycleState":lifecycle_state,
        "occurredAtMs":occurred_at_ms,
    })
}
fn tone(pnl: Option<i64>) -> &'static str {
    match pnl.unwrap_or_default().cmp(&0) {
        std::cmp::Ordering::Greater => "profit",
        std::cmp::Ordering::Less => "loss",
        std::cmp::Ordering::Equal => "wash",
    }
}

fn ledger_refs(fill_ids: &[String]) -> Vec<String> {
    fill_ids
        .iter()
        .flat_map(|fill_id| [format!("{fill_id}:notional"), format!("{fill_id}:fee")])
        .collect()
}
fn csv_field(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    let prefixed = if value.starts_with(['=', '+', '-', '@']) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    format!("\"{}\"", prefixed.replace('"', "\"\""))
}
fn csv_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}
fn csv_row(values: &[String]) -> String {
    format!(
        "{}\n",
        values.iter().map(csv_field).collect::<Vec<_>>().join(",")
    )
}
fn trades_csv(report: &BacktestReport) -> String {
    let mut output = "trade_id,symbol,realized,quantity_micros,entry_fill_id,exit_fill_id,terminal_valuation_fill_id,pnl_micros,fee_micros\n".to_string();
    for trade in &report.trades {
        output.push_str(&csv_row(&[
            trade.trade_id.clone(),
            trade.symbol.clone(),
            trade.realized.to_string(),
            trade.quantity_micros.to_string(),
            csv_value(&trade.entry["fillId"]),
            trade
                .exit
                .as_ref()
                .map(|value| csv_value(&value["fillId"]))
                .unwrap_or_default(),
            trade
                .terminal_valuation
                .as_ref()
                .map(|value| csv_value(&value["fillId"]))
                .unwrap_or_default(),
            trade
                .pnl_micros
                .map(|value| value.to_string())
                .unwrap_or_default(),
            trade.fee_micros.to_string(),
        ]));
    }
    output
}
fn orders_fills_csv(report: &BacktestReport) -> String {
    let has_notional = report
        .orders
        .iter()
        .any(|order| order.get("notionalMicros").is_some());
    let mut output = "record_type,fill_id,symbol,side,quantity_micros,price_micros,fee_micros,execution_timestamp".to_string();
    if has_notional {
        output.push_str(",notional_micros");
    }
    output.push('\n');
    for order in &report.orders {
        let mut row = vec![
            "order".to_string(),
            String::new(),
            csv_value(&order["symbol"]),
            csv_value(&order["side"]),
            csv_value(&order["quantityMicros"]),
            String::new(),
            String::new(),
            csv_value(&order["executionTimestamp"]),
        ];
        if has_notional {
            row.push(csv_value(&order["notionalMicros"]));
        }
        output.push_str(&csv_row(&row));
    }
    for fill in &report.fills {
        let mut row = vec![
            "fill".to_string(),
            csv_value(&fill["fillId"]),
            csv_value(&fill["symbol"]),
            csv_value(&fill["side"]),
            csv_value(&fill["quantityMicros"]),
            csv_value(&fill["priceMicros"]),
            csv_value(&fill["feeMicros"]),
            csv_value(&fill["executionTimestamp"]),
        ];
        if has_notional {
            row.push(String::new());
        }
        output.push_str(&csv_row(&row));
    }
    output
}
fn positions_ledger_csv(report: &BacktestReport) -> String {
    let mut output = "record_type,symbol,quantity_micros,average_cost_micros,market_price_micros,market_value_micros,unrealized_pnl_micros,entry_id,kind,cash_delta_micros\n".to_string();
    for position in &report.positions {
        output.push_str(&csv_row(&[
            "position".to_string(),
            csv_value(&position["symbol"]),
            csv_value(&position["quantity"]),
            csv_value(&position["averageCostMicros"]),
            csv_value(&position["marketPriceMicros"]),
            csv_value(&position["marketValueMicros"]),
            csv_value(&position["unrealizedPnlMicros"]),
            String::new(),
            String::new(),
            String::new(),
        ]));
    }
    for ledger in &report.ledger {
        output.push_str(&csv_row(&[
            "ledger".to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            csv_value(&ledger["entryId"]),
            csv_value(&ledger["kind"]),
            csv_value(&ledger["cashDeltaMicros"]),
        ]));
    }
    output
}
fn equity_csv(report: &BacktestReport) -> String {
    let mut output = "timestamp,ending_cash_micros,market_value_micros,ending_equity_micros,reconciliation_residual_micros\n".to_string();
    output.push_str(&csv_row(&[
        report.chart.range["end"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        report.overview.ending_cash_micros.to_string(),
        report.overview.market_value_micros.to_string(),
        report.overview.ending_equity_micros.to_string(),
        report.overview.reconciliation_residual_micros.to_string(),
    ]));
    output
}

#[cfg(test)]
mod tests {
    use super::{csv_field, instrument_details_from_content, strategy_execution_summary};
    use crate::backtest_contracts::canonical_hash;
    use crate::historical_data::{
        AssetClass, BarObservation, DatasetNormalizationPolicy, DatasetQualityPolicy,
        DatasetSourceClass, DatasetSourceManifest, DatasetTimeSlice, HistoricalDataKind,
        HistoricalObservation, HistoricalObservationData, OptionContractObservation, OptionGreeks,
        OptionRight, QualityDisposition, DATASET_SNAPSHOT_SCHEMA,
    };
    use std::collections::BTreeMap;

    #[test]
    fn execution_summary_is_hash_bound_and_does_not_infer_indicator_names() {
        let spec = serde_json::json!({
            "name": "price-threshold",
            "rules": [{"field": "close", "operator": ">", "value": 100}],
            "indicators": ["SMA"]
        });
        let hash = canonical_hash(&spec, "test_hash").expect("hash");
        let summary = strategy_execution_summary(Some(&spec), &hash);
        assert_eq!(summary["status"], "verified");
        assert_eq!(summary["indicators"], serde_json::json!(["SMA"]));
        assert!(!summary["indicators"].to_string().contains("VWAP"));
        assert!(!summary["indicators"].to_string().contains("RSI"));

        let mut tampered = spec.clone();
        tampered["indicators"] = serde_json::json!(["RSI"]);
        let unavailable = strategy_execution_summary(Some(&tampered), &hash);
        assert_eq!(unavailable["status"], "unavailable");
        assert!(unavailable["reason"]
            .as_str()
            .expect("reason")
            .contains("hash"));
    }

    #[test]
    fn csv_escapes_formula_and_quotes() {
        assert_eq!(csv_field("=SUM(A1:A2)"), "\"'=SUM(A1:A2)\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn bar_instrument_details_use_only_unambiguous_snapshot_class() {
        let content = content(
            vec![AssetClass::CryptoSpot],
            HistoricalDataKind::Bars,
            vec![HistoricalObservation {
                instrument_id: "BTC/USD".to_string(),
                timestamp: "2026-07-01T00:00:00Z".to_string(),
                data: HistoricalObservationData::Bar(BarObservation {
                    open: 100.0,
                    high: 101.0,
                    low: 99.0,
                    close: 100.5,
                    volume: 10.0,
                    vwap: None,
                    session: None,
                }),
            }],
        );
        let details = instrument_details_from_content(&content, "BTC/USD", None).expect("details");
        assert_eq!(details.schema, "tradeassembly.instrument.bar.v1");
        assert_eq!(details.asset_class, Some(AssetClass::CryptoSpot));
        assert_eq!(details.fields[1].value, "crypto_spot");

        let mut ambiguous = content;
        ambiguous.asset_classes.push(AssetClass::Equity);
        let details =
            instrument_details_from_content(&ambiguous, "BTC/USD", None).expect("details");
        assert_eq!(details.asset_class, None);
        assert_eq!(details.fields.len(), 1);
    }

    #[test]
    fn option_details_are_timestamp_bound_and_omit_absent_fields() {
        let timestamp = "2026-07-01T14:30:00Z";
        let content = content(
            vec![AssetClass::Equity, AssetClass::Option],
            HistoricalDataKind::OptionChain,
            vec![HistoricalObservation {
                instrument_id: "SPY260717C00600000".to_string(),
                timestamp: timestamp.to_string(),
                data: HistoricalObservationData::OptionContract(Box::new(
                    OptionContractObservation {
                        contract_symbol: "SPY260717C00600000".to_string(),
                        underlying_instrument_id: "SPY".to_string(),
                        expiration: "2026-07-17".to_string(),
                        strike: 600.0,
                        right: OptionRight::Call,
                        bid: Some(2.1),
                        ask: Some(2.2),
                        last: None,
                        volume: None,
                        open_interest: Some(1200.0),
                        implied_volatility: None,
                        greeks: Some(OptionGreeks {
                            delta: Some(0.51),
                            gamma: None,
                            theta: Some(-0.08),
                            vega: None,
                            rho: None,
                        }),
                    },
                )),
            }],
        );
        let details =
            instrument_details_from_content(&content, "SPY260717C00600000", Some(timestamp))
                .expect("option details");
        assert_eq!(
            details.schema,
            "tradeassembly.instrument.option-contract.v1"
        );
        assert_eq!(details.asset_class, Some(AssetClass::Option));
        assert_eq!(details.observed_at.as_deref(), Some(timestamp));
        let keys = details
            .fields
            .iter()
            .map(|field| field.key.as_str())
            .collect::<Vec<_>>();
        assert!(keys.contains(&"bid"));
        assert!(keys.contains(&"open_interest"));
        assert!(keys.contains(&"delta"));
        assert!(keys.contains(&"theta"));
        assert!(!keys.contains(&"last"));
        assert!(!keys.contains(&"gamma"));
    }

    #[test]
    fn instrument_details_fail_closed_for_missing_or_conflicting_evidence() {
        let mut content = content(
            vec![AssetClass::Option],
            HistoricalDataKind::OptionChain,
            vec![HistoricalObservation {
                instrument_id: "SPY260717C00600000".to_string(),
                timestamp: "2026-07-01T14:30:00Z".to_string(),
                data: HistoricalObservationData::OptionContract(Box::new(
                    OptionContractObservation {
                        contract_symbol: "SPY260717C00600000".to_string(),
                        underlying_instrument_id: "SPY".to_string(),
                        expiration: "2026-07-17".to_string(),
                        strike: 600.0,
                        right: OptionRight::Call,
                        bid: Some(2.1),
                        ask: None,
                        last: None,
                        volume: None,
                        open_interest: None,
                        implied_volatility: None,
                        greeks: None,
                    },
                )),
            }],
        );
        assert_eq!(
            instrument_details_from_content(&content, "missing", None),
            Err("backtest_report_instrument_integrity_failed".to_string())
        );
        assert_eq!(
            instrument_details_from_content(
                &content,
                "SPY260717C00600000",
                Some("2026-07-01T15:30:00Z")
            ),
            Err("backtest_report_instrument_integrity_failed".to_string())
        );
        if let HistoricalObservationData::OptionContract(contract) =
            &mut content.observations[0].data
        {
            contract.contract_symbol = "SPY260717P00600000".to_string();
        }
        assert_eq!(
            instrument_details_from_content(
                &content,
                "SPY260717C00600000",
                Some("2026-07-01T14:30:00Z")
            ),
            Err("backtest_report_instrument_integrity_failed".to_string())
        );
    }

    fn content(
        asset_classes: Vec<AssetClass>,
        data_kind: HistoricalDataKind,
        observations: Vec<HistoricalObservation>,
    ) -> crate::historical_data::DatasetSnapshotContent {
        crate::historical_data::DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            strategy_id: "strategy-1".to_string(),
            strategy_version_id: Some("version-1".to_string()),
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: "local-data:test".to_string(),
                plugin_ref: "local-data@1".to_string(),
                operation_id: "market_data.history".to_string(),
                plugin_manifest_fingerprint: "sha256:manifest".to_string(),
                capability_graph_revision_id: "revision-1".to_string(),
                capability_graph_fingerprint: "sha256:graph".to_string(),
                source_request_hash: "sha256:request".to_string(),
                upstream_refs: BTreeMap::new(),
            },
            asset_classes,
            instruments: vec!["BTC/USD".to_string(), "SPY".to_string()],
            data_kind,
            granularity: "1m".to_string(),
            time_slice: DatasetTimeSlice {
                start: "2026-07-01T00:00:00Z".to_string(),
                end: "2026-07-02T00:00:00Z".to_string(),
            },
            calendar: "24x7".to_string(),
            timezone: "UTC".to_string(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "milliseconds".to_string(),
                timezone: "UTC".to_string(),
                duplicate_policy: "reject".to_string(),
                price_adjustment: "raw".to_string(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: QualityDisposition::Reject,
                stale_observations: QualityDisposition::Reject,
                outliers: QualityDisposition::Warn,
                invalid_markets: QualityDisposition::Reject,
                calendar_mismatch: QualityDisposition::Reject,
                corporate_action_gaps: QualityDisposition::Warn,
                missing_derivative_fields: QualityDisposition::Reject,
            },
            quality_findings: Vec::new(),
            observations,
        }
    }
}
