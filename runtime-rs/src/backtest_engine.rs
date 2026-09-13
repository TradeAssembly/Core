//! Deterministic backtest orchestration over immutable strategy and dataset inputs.

use crate::backtest_accounting::{
    account, AccountingFill, AccountingInput, EndOfDataPolicy as AccountingEndPolicy, FillSide,
};
use crate::backtest_contracts::{
    canonical_hash, AccountModel, BacktestDiagnostic, BacktestResult, BacktestResultContent,
    BacktestRunManifest, DiagnosticSeverity, EndOfDataPolicy, FillPolicy, FillTiming, PriceSource,
    BACKTEST_RESULT_SCHEMA,
};
use crate::historical_data::{DatasetSnapshot, HistoricalDataKind, HistoricalObservationData};
use crate::strategy_kernel::{
    calculate_fee_micros, run, CompiledStrategy, ExecutionTiming, KernelMode, MarketObservation,
    OrderSide, PartialFillPolicy, SimulationConfig,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const UNIT: i64 = 1_000_000;

pub fn execute(
    manifest: &BacktestRunManifest,
    strategy_version: &Value,
    snapshot: &DatasetSnapshot,
    created_at_ms: i64,
) -> Result<BacktestResult, String> {
    manifest.verify()?;
    snapshot.verify()?;
    let configuration = &manifest.content.configuration;
    verify_strategy_version(strategy_version, manifest)?;
    verify_dataset(snapshot, manifest)?;
    if configuration.capital.account_model != AccountModel::Cash
        || configuration.capital.maximum_leverage_micros != UNIT
        || !configuration.execution.reject_on_insufficient_capital
        || !configuration.execution.reject_on_insufficient_liquidity
        || configuration.instruments.len() != 1
        || configuration.risk.maximum_open_positions != 1
        || configuration.risk.maximum_loss_micros < configuration.capital.starting_cash_micros
        || configuration.instruments[0].contract_multiplier_micros != UNIT
    {
        return Err("backtest_account_model_unsupported".to_string());
    }

    let spec = strategy_version
        .get("spec")
        .ok_or_else(|| "backtest_strategy_version_required".to_string())?;
    let compiled = CompiledStrategy::compile_json(spec).map_err(kernel_error)?;
    let observations = normalized_bars(snapshot, configuration.liquidity.minimum_volume_micros)?;
    let simulation = simulation_config(manifest)?;
    let kernel =
        run(KernelMode::Backtest, &compiled, &observations, simulation).map_err(kernel_error)?;

    let timestamps = snapshot
        .content
        .observations
        .iter()
        .map(|row| row.timestamp.as_str())
        .collect::<Vec<_>>();
    let mut fills = kernel
        .fills
        .iter()
        .enumerate()
        .map(|(index, fill)| AccountingFill {
            fill_id: format!(
                "{}:fill:{}:{}",
                manifest.content.run_id, fill.order.execution_bar_index, index
            ),
            symbol: fill.order.intent.symbol.clone(),
            side: match fill.order.intent.side {
                OrderSide::Buy => FillSide::Buy,
                OrderSide::Sell => FillSide::Sell,
            },
            quantity: fill.quantity,
            price_micros: fill.price_micros,
            fee_micros: fill.fee_micros,
        })
        .collect::<Vec<_>>();
    let marks = last_marks(&observations);
    if configuration.end_of_data == EndOfDataPolicy::ForceClose {
        append_force_closes(&mut fills, &marks, simulation, &manifest.content.run_id)?;
    }
    let accounting = account(&AccountingInput {
        starting_cash_micros: configuration.capital.starting_cash_micros,
        fills: fills.clone(),
        cash_entries: Vec::new(),
        marks_micros: marks,
        end_of_data: AccountingEndPolicy::MarkToMarket,
        reconciliation_tolerance_micros: 0,
    })
    .map_err(accounting_error)?;

    let decisions = kernel
        .decisions
        .iter()
        .map(|decision| match decision {
            crate::strategy_kernel::Decision::NoSignal { bar_index } => json!({
                "kind": "no_signal",
                "barIndex": bar_index,
                "timestamp": timestamp(&timestamps, *bar_index),
            }),
            crate::strategy_kernel::Decision::OrderIntent(intent) => json!({
                "kind": "order_intent",
                "barIndex": intent.decision_bar_index,
                "timestamp": timestamp(&timestamps, intent.decision_bar_index),
                "symbol": intent.symbol,
                "side": side_name(intent.side),
                "quantityMicros": intent.quantity,
            }),
        })
        .collect();
    let orders = kernel
        .orders
        .iter()
        .map(|order| {
            json!({
                "decisionBarIndex": order.intent.decision_bar_index,
                "decisionTimestamp": timestamp(&timestamps, order.intent.decision_bar_index),
                "executionBarIndex": order.execution_bar_index,
                "executionTimestamp": timestamp(&timestamps, order.execution_bar_index),
                "symbol": order.intent.symbol,
                "side": side_name(order.intent.side),
                "quantityMicros": order.intent.quantity,
            })
        })
        .collect();
    let result_fills = kernel
        .fills
        .iter()
        .enumerate()
        .map(|(index, fill)| {
            json!({
                "fillId": fills[index].fill_id,
                "decisionBarIndex": fill.order.intent.decision_bar_index,
                "decisionTimestamp": timestamp(&timestamps, fill.order.intent.decision_bar_index),
                "executionBarIndex": fill.order.execution_bar_index,
                "executionTimestamp": timestamp(&timestamps, fill.order.execution_bar_index),
                "symbol": fill.order.intent.symbol,
                "side": side_name(fill.order.intent.side),
                "quantityMicros": fill.quantity,
                "priceMicros": fill.price_micros,
                "feeMicros": fill.fee_micros,
            })
        })
        .chain(fills.iter().skip(kernel.fills.len()).map(|fill| {
            json!({
                "fillId": fill.fill_id,
                "symbol": fill.symbol,
                "side": "sell",
                "quantityMicros": fill.quantity,
                "priceMicros": fill.price_micros,
                "feeMicros": fill.fee_micros,
                "endOfData": true,
            })
        }))
        .collect();
    let diagnostics = kernel
        .rejects
        .iter()
        .map(|reject| BacktestDiagnostic {
            code: "backtest_order_rejected".to_string(),
            field: None,
            severity: DiagnosticSeverity::Warning,
            detail: format!("{reject:?}"),
        })
        .collect();
    BacktestResult::build(
        BacktestResultContent {
            schema: BACKTEST_RESULT_SCHEMA.to_string(),
            run_id: manifest.content.run_id.clone(),
            manifest_hash: manifest.manifest_hash.clone(),
            evaluator_version: configuration.evaluator_version.clone(),
            compiler_version: configuration.compiler_version.clone(),
            decisions,
            orders,
            fills: result_fills,
            positions: accounting
                .positions
                .values()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "backtest_result_serialization_failed".to_string())?,
            ledger: accounting
                .ledger
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "backtest_result_serialization_failed".to_string())?,
            accounting: serde_json::to_value(accounting)
                .map_err(|_| "backtest_result_serialization_failed".to_string())?,
            diagnostics,
        },
        created_at_ms,
    )
}

pub fn replay(
    manifest: &BacktestRunManifest,
    strategy_version: &Value,
    snapshot: &DatasetSnapshot,
    expected: &BacktestResult,
) -> Result<BacktestResult, String> {
    expected.verify()?;
    let recomputed = execute(manifest, strategy_version, snapshot, expected.created_at_ms)?;
    if recomputed.result_hash != expected.result_hash {
        return Err("backtest_replay_mismatch".to_string());
    }
    Ok(recomputed)
}

fn verify_strategy_version(version: &Value, manifest: &BacktestRunManifest) -> Result<(), String> {
    let binding = &manifest.content.configuration.strategy;
    let spec = version
        .get("spec")
        .ok_or_else(|| "backtest_strategy_version_required".to_string())?;
    let spec_hash = canonical_hash(spec, "backtest_strategy_hash_failed")?;
    if version.get("id").and_then(Value::as_str) != Some(binding.strategy_version_id.as_str())
        || version
            .get("strategyId")
            .or_else(|| version.get("strategy_id"))
            .and_then(Value::as_str)
            != Some(binding.strategy_id.as_str())
        || version.get("specHash").and_then(Value::as_str)
            != Some(binding.strategy_spec_hash.as_str())
        || spec_hash != binding.strategy_spec_hash
    {
        return Err("backtest_strategy_integrity_failed".to_string());
    }
    Ok(())
}

fn verify_dataset(
    snapshot: &DatasetSnapshot,
    manifest: &BacktestRunManifest,
) -> Result<(), String> {
    let binding = &manifest.content.configuration.dataset;
    let source = &snapshot.content.source;
    let source_class = serde_json::to_value(&source.source_class)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string));
    if snapshot.dataset_id != binding.dataset_id
        || snapshot.snapshot_ref != binding.snapshot_ref
        || snapshot.content_hash != binding.content_hash
        || snapshot.content.schema != binding.schema
        || snapshot.content.strategy_id != manifest.content.configuration.strategy.strategy_id
        || source.plugin_ref != binding.source_plugin_ref
        || source.plugin_instance_ref != binding.source_plugin_instance_ref
        || source.operation_id != binding.source_operation_id
        || source.plugin_manifest_fingerprint != binding.source_manifest_fingerprint
        || source.capability_graph_revision_id != binding.capability_graph_revision_id
        || source.capability_graph_fingerprint != binding.capability_graph_fingerprint
        || source_class.as_deref() != Some(binding.source_class.as_str())
        || snapshot.content.instruments != binding.instruments
        || snapshot.content.time_slice != binding.selected_time_slice
        || snapshot.content.granularity != binding.granularity
        || snapshot.content.calendar != binding.calendar
        || snapshot.content.timezone != binding.timezone
        || snapshot.content.normalization_policy.price_adjustment != binding.corporate_action_policy
        || snapshot.content.data_kind != HistoricalDataKind::Bars
    {
        return Err("backtest_dataset_integrity_failed".to_string());
    }
    Ok(())
}

fn normalized_bars(
    snapshot: &DatasetSnapshot,
    minimum_volume_micros: i64,
) -> Result<Vec<MarketObservation>, String> {
    snapshot
        .content
        .observations
        .iter()
        .enumerate()
        .map(|(bar_index, row)| {
            let HistoricalObservationData::Bar(bar) = &row.data else {
                return Err("backtest_data_kind_unsupported".to_string());
            };
            let volume = micros(bar.volume, "backtest_data_invalid")?;
            Ok(MarketObservation {
                bar_index,
                symbol: row.instrument_id.clone(),
                open: micros(bar.open, "backtest_data_invalid")?,
                close: micros(bar.close, "backtest_data_invalid")?,
                volume: if volume < minimum_volume_micros {
                    0
                } else {
                    volume
                },
            })
        })
        .collect()
}

fn simulation_config(manifest: &BacktestRunManifest) -> Result<SimulationConfig, String> {
    let configuration = &manifest.content.configuration;
    let timing = match (
        &configuration.execution.fill_timing,
        &configuration.execution.fill_price_source,
    ) {
        (FillTiming::SameBar, PriceSource::Open) => ExecutionTiming::SameBarOpen,
        (FillTiming::SameBar, PriceSource::Close) => ExecutionTiming::SameBarClose,
        (FillTiming::NextBar, PriceSource::Open) => ExecutionTiming::NextBarOpen,
        (FillTiming::NextBar, PriceSource::Close) => ExecutionTiming::NextBarClose,
    };
    Ok(SimulationConfig {
        timing,
        fee_per_order_micros: configuration.costs.fixed_fee_micros,
        fee_per_unit_micros: configuration.costs.per_unit_fee_micros,
        notional_fee_bps: i64::from(configuration.costs.notional_fee_bps),
        spread_bps: i64::from(configuration.costs.spread_bps),
        slippage_bps: i64::from(configuration.costs.slippage_bps),
        participation_bps: i64::from(configuration.liquidity.maximum_participation_bps),
        partial_fill_policy: match configuration.execution.fill_policy {
            FillPolicy::Full => PartialFillPolicy::Reject,
            FillPolicy::ParticipationCappedPartial => PartialFillPolicy::Allow,
        },
        starting_cash_micros: configuration.capital.starting_cash_micros,
        maximum_order_quantity_micros: configuration.risk.maximum_order_quantity_micros,
        maximum_position_notional_micros: configuration.risk.maximum_position_notional_micros,
    })
}

fn append_force_closes(
    fills: &mut Vec<AccountingFill>,
    marks: &BTreeMap<String, i64>,
    simulation: SimulationConfig,
    run_id: &str,
) -> Result<(), String> {
    let mut quantities = BTreeMap::<String, i64>::new();
    for fill in fills.iter() {
        let signed = match fill.side {
            FillSide::Buy => fill.quantity,
            FillSide::Sell => -fill.quantity,
        };
        let current = quantities.entry(fill.symbol.clone()).or_default();
        *current = current
            .checked_add(signed)
            .ok_or_else(|| "backtest_accounting_overflow".to_string())?;
    }
    for (symbol, quantity) in quantities {
        if quantity <= 0 {
            continue;
        }
        let price = *marks
            .get(&symbol)
            .ok_or_else(|| "backtest_mark_missing".to_string())?;
        let gross = scaled_notional(quantity, price)?;
        let fee = calculate_fee_micros(quantity, gross, simulation).map_err(kernel_error)?;
        fills.push(AccountingFill {
            fill_id: format!("{run_id}:force_close:{symbol}"),
            symbol,
            side: FillSide::Sell,
            quantity,
            price_micros: price,
            fee_micros: fee,
        });
    }
    Ok(())
}

fn last_marks(observations: &[MarketObservation]) -> BTreeMap<String, i64> {
    observations
        .iter()
        .map(|row| (row.symbol.clone(), row.close))
        .collect()
}

fn micros(value: f64, code: &str) -> Result<i64, String> {
    if !value.is_finite() || value < 0.0 || value > i64::MAX as f64 / UNIT as f64 {
        return Err(code.to_string());
    }
    Ok((value * UNIT as f64).round() as i64)
}

fn scaled_notional(quantity: i64, price: i64) -> Result<i64, String> {
    i64::try_from(
        i128::from(quantity)
            .checked_mul(i128::from(price))
            .ok_or_else(|| "backtest_accounting_overflow".to_string())?
            / i128::from(UNIT),
    )
    .map_err(|_| "backtest_accounting_overflow".to_string())
}

fn timestamp<'a>(timestamps: &'a [&str], index: usize) -> Option<&'a str> {
    timestamps.get(index).copied()
}

fn side_name(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    }
}

fn kernel_error(error: crate::strategy_kernel::KernelError) -> String {
    format!("backtest_kernel_{error:?}").to_lowercase()
}

fn accounting_error(error: crate::backtest_accounting::AccountingError) -> String {
    format!("backtest_accounting_{error:?}").to_lowercase()
}
