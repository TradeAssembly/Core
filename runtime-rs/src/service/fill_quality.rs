// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{TradeAssemblyService, LEGAL_BOUNDARY};
use crate::execution_contracts::{
    analyze_fill_quality, ExecutionEventContract, ExecutionEventKind, FillQualityBenchmark,
    FillQualityReport, OrderSide, WarningSeverity,
};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};

const REPORT_NS: &str = "fill_quality_reports";

pub(crate) fn analyze(service: &TradeAssemblyService, request: Value) -> Value {
    let analysis_id = string_field(&request, &["analysisId", "analysis_id"])
        .unwrap_or_else(|| "fill_quality_latest".to_string());
    let strategy_id = string_field(&request, &["strategyId", "strategy_id"])
        .unwrap_or_else(|| "strat_local_btc_demo".to_string());
    let benchmark_selection = user_benchmark_selection(&request);
    let benchmark = benchmark_selection
        .as_ref()
        .map(|selection| selection.benchmark_ref.clone())
        .or_else(|| string_field(&request, &["benchmark", "benchmarkRef", "benchmark_ref"]))
        .unwrap_or_else(|| "decision_quote_midpoint".to_string());
    let studio_base_url = string_field(&request, &["studioBaseUrl", "studio_base_url"])
        .unwrap_or_else(|| "http://127.0.0.1:3001".to_string());
    let execution_href = format!(
        "{}/app/strategies/{strategy_id}/execution?fillQuality={analysis_id}",
        studio_base_url.trim_end_matches('/')
    );
    let events = execution_events(&request);
    let mut reports = events.iter().map(analyze_fill_quality).collect::<Vec<_>>();
    if let Some(selection) = benchmark_selection.as_ref() {
        for (event, report) in events.iter().zip(reports.iter_mut()) {
            apply_user_benchmark(event, report, selection);
        }
    }
    let aggregate = aggregate_summary(&events, &reports);
    let warnings = reports
        .iter()
        .flat_map(|report| report.warnings.clone())
        .collect::<Vec<_>>();
    let provider_caveats = provider_capability_caveats(&request, &events);
    let export_refs = json!({
        "json": {"uri": format!("tradeassembly://artifact/fill-quality/{analysis_id}/report.json")},
        "csv": {"uri": format!("tradeassembly://artifact/fill-quality/{analysis_id}/fills.csv")},
    });
    let replay_refs = json!({
        "commands": [
            format!("tradeassembly fill-quality analyze --analysis-id {analysis_id} --db {}", service.db()),
            format!("tradeassembly report-envelope --kind fill_quality --strategy-id {strategy_id} --analysis-id {analysis_id}")
        ],
        "refs": reports
            .iter()
            .flat_map(|report| report.replay_refs.clone())
            .map(|replay| json!({"replayRef": replay.replay_ref, "deterministic": replay.deterministic}))
            .collect::<Vec<_>>()
    });
    let artifact_refs = json!([
        {"id": format!("fill-quality-{analysis_id}-json"), "uri": export_refs["json"]["uri"]},
        {"id": format!("fill-quality-{analysis_id}-csv"), "uri": export_refs["csv"]["uri"]}
    ]);
    let mut result = json!({
        "schemaVersion": "tradeassembly.fill_quality.service.v1",
        "kind": "fill_quality",
        "analysisId": analysis_id,
        "strategyId": strategy_id,
        "ok": warnings.iter().all(|warning| warning.severity != WarningSeverity::Error),
        "benchmarkSelection": benchmark.clone(),
        "summary": aggregate,
        "agentSummary": {
            "scope": format!("Fill quality analysis for strategy {strategy_id}"),
            "benchmark": benchmark.clone(),
            "results": {
                "eventCount": aggregate["eventCount"],
                "fillCount": aggregate["fillCount"],
                "averageSlippagePerUnit": aggregate["averageSlippagePerUnit"],
                "aggregateFees": aggregate["aggregateFees"],
                "warningCount": aggregate["warningCount"],
            },
            "warnings": warnings.iter().map(|warning| json!({"code": warning.code, "severity": warning.severity, "message": warning.message})).collect::<Vec<_>>(),
            "studioDeepLinks": {
                "execution": execution_href.clone()
            }
        },
        "perFill": per_fill_rows(&events, &reports),
        "rejectCancelSummary": reject_cancel_summary(&reports),
        "providerCapabilityCaveats": provider_caveats,
        "warnings": warnings,
        "artifactRefs": artifact_refs,
        "exportRefs": export_refs,
        "replayRefs": replay_refs,
        "reportEnvelope": {
            "schemaVersion": "tradeassembly.report.v1",
            "kind": "fill_quality",
            "strategyId": strategy_id,
            "summary": aggregate,
            "renderContract": {
                "preferred": "tradeassembly_ui_deep_links",
                "userVisible": ["summary", "perFill", "warnings", "deepLinks"],
                "agentVisible": ["artifactRefs", "exportRefs", "replayRefs"]
            },
            "deepLinks": {
                "strategy": {"href": format!("{}/app/strategies/{strategy_id}", studio_base_url.trim_end_matches('/'))},
                "execution": {"href": execution_href}
            }
        },
        "noAdvice": LEGAL_BOUNDARY
    });

    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("fill_quality.analysis:{analysis_id}"))
            .expect("valid fill quality idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(REPORT_NS, &analysis_id, result.clone(), &context)
        .expect("persist fill quality report");
    let journal_id = service
        .runtime()
        .record_side_effect("fill_quality.analysis.completed", result.clone(), &context)
        .expect("record fill quality journal evidence");
    result["journalEvidence"] = json!({
        "eventType": "fill_quality.analysis.completed",
        "journalId": journal_id,
        "storageNamespace": REPORT_NS,
        "analysisId": result["analysisId"],
    });
    service
        .runtime()
        .storage
        .put_json(
            REPORT_NS,
            result["analysisId"]
                .as_str()
                .unwrap_or("fill_quality_latest"),
            result.clone(),
            &context,
        )
        .expect("persist fill quality report with journal evidence");
    result
}

fn execution_events(request: &Value) -> Vec<ExecutionEventContract> {
    request
        .get("executionEvents")
        .or_else(|| request.get("execution_events"))
        .or_else(|| request.get("events"))
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|event| {
                    serde_json::from_value::<ExecutionEventContract>(event.clone()).ok()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn aggregate_summary(events: &[ExecutionEventContract], reports: &[FillQualityReport]) -> Value {
    let mut fill_count = 0usize;
    let mut aggregate_quantity = 0.0;
    let mut aggregate_fees = 0.0;
    let mut aggregate_slippage = 0.0;
    let mut aggregate_spread_capture = 0.0;
    let mut slippage_weight = 0.0;
    let mut time_total = 0u64;
    let mut time_count = 0u64;

    for (event, report) in events.iter().zip(reports) {
        if let Some(fill) = event.fill.as_ref() {
            if fill.filled_quantity.is_some() || fill.fill_price.is_some() {
                fill_count += 1;
            }
            let quantity = fill.filled_quantity.unwrap_or_default();
            aggregate_quantity += quantity;
            aggregate_fees += fill.fee_amount.unwrap_or_default();
            if let Some(slippage) = report.slippage_amount {
                aggregate_slippage += slippage * quantity;
                slippage_weight += quantity;
            }
            if let Some(spread_capture) = report.spread_capture {
                aggregate_spread_capture += spread_capture * quantity;
            }
        }
        if let Some(time_to_fill_ms) = report.time_to_fill_ms {
            time_total += time_to_fill_ms;
            time_count += 1;
        }
    }

    json!({
        "eventCount": reports.len(),
        "fillCount": fill_count,
        "aggregateQuantity": round6(aggregate_quantity),
        "aggregateFees": round6(aggregate_fees),
        "aggregateSlippageAmount": round6(aggregate_slippage),
        "averageSlippagePerUnit": if slippage_weight > 0.0 { json!(round6(aggregate_slippage / slippage_weight)) } else { Value::Null },
        "aggregateSpreadCapture": round6(aggregate_spread_capture),
        "averageTimeToFillMs": time_total.checked_div(time_count).map(|average| json!(average)).unwrap_or(Value::Null),
        "rejectCount": reports.iter().filter(|report| report.reject_reason.is_some()).count(),
        "cancelCount": reports.iter().filter(|report| report.cancel_reason.is_some()).count(),
        "warningCount": reports.iter().map(|report| report.warnings.len()).sum::<usize>(),
        "errorCount": reports.iter().flat_map(|report| report.warnings.iter()).filter(|warning| warning.severity == WarningSeverity::Error).count(),
    })
}

fn per_fill_rows(events: &[ExecutionEventContract], reports: &[FillQualityReport]) -> Vec<Value> {
    events
        .iter()
        .zip(reports)
        .map(|(event, report)| {
            let fill = event.fill.as_ref();
            json!({
                "eventId": event.event_id,
                "eventKind": event.event_kind,
                "mode": event.mode,
                "strategyId": event.order_intent.strategy_id,
                "instrumentId": event.order_intent.instrument_id,
                "providerRef": event.order_intent.provider_ref,
                "fillId": fill.map(|fill| fill.fill_id.clone()),
                "filledQuantity": fill.and_then(|fill| fill.filled_quantity),
                "fillPrice": fill.and_then(|fill| fill.fill_price),
                "feeAmount": fill.and_then(|fill| fill.fee_amount),
                "feeCurrency": fill.and_then(|fill| fill.fee_currency.clone()),
                "benchmarkReference": report.benchmark_reference,
                "slippageAmount": report.slippage_amount,
                "spreadCapture": report.spread_capture,
                "timeToFillMs": report.time_to_fill_ms,
                "rejectReason": report.reject_reason,
                "cancelReason": report.cancel_reason,
                "warningCount": report.warnings.len(),
                "replayRefs": report.replay_refs,
            })
        })
        .collect()
}

fn reject_cancel_summary(reports: &[FillQualityReport]) -> Value {
    let rejects = reports
        .iter()
        .filter_map(|report| report.reject_reason.as_ref())
        .map(|reason| json!({"reason": reason, "count": 1}))
        .collect::<Vec<_>>();
    let cancels = reports
        .iter()
        .filter_map(|report| report.cancel_reason.as_ref())
        .map(|reason| json!({"reason": reason, "count": 1}))
        .collect::<Vec<_>>();
    json!({"rejects": rejects, "cancels": cancels})
}

fn provider_capability_caveats(request: &Value, events: &[ExecutionEventContract]) -> Vec<Value> {
    let mut caveats = request
        .get("providerCapabilityCaveats")
        .or_else(|| request.get("provider_capability_caveats"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if events.iter().any(|event| {
        matches!(
            event.event_kind,
            ExecutionEventKind::ManualImport | ExecutionEventKind::SimulatedFill
        )
    }) {
        caveats.push(json!({
            "code": "execution_source_mode",
            "message": "Some execution evidence came from simulated or manually imported events; provider-native fields may be unavailable.",
        }));
    }
    caveats
}

#[derive(Clone, Debug)]
struct UserBenchmarkSelection {
    benchmark_ref: String,
    benchmark_price: f64,
    benchmark_source: String,
}

fn user_benchmark_selection(request: &Value) -> Option<UserBenchmarkSelection> {
    let benchmark = request
        .get("benchmark")
        .or_else(|| request.get("benchmarkSelection"));
    let benchmark_ref = benchmark
        .and_then(|value| {
            value
                .get("benchmarkRef")
                .or_else(|| value.get("benchmark_ref"))
        })
        .and_then(Value::as_str)
        .or_else(|| {
            request
                .get("benchmarkRef")
                .or_else(|| request.get("benchmark_ref"))
                .and_then(Value::as_str)
        })
        .unwrap_or("user_selected_benchmark")
        .to_string();
    let benchmark_price = benchmark
        .and_then(|value| {
            value
                .get("benchmarkPrice")
                .or_else(|| value.get("benchmark_price"))
                .or_else(|| value.get("price"))
        })
        .and_then(Value::as_f64)
        .or_else(|| {
            request
                .get("benchmarkPrice")
                .or_else(|| request.get("benchmark_price"))
                .and_then(Value::as_f64)
        })?;
    let benchmark_source = benchmark
        .and_then(|value| {
            value
                .get("benchmarkSource")
                .or_else(|| value.get("benchmark_source"))
                .or_else(|| value.get("source"))
        })
        .and_then(Value::as_str)
        .unwrap_or("user_selected")
        .to_string();

    Some(UserBenchmarkSelection {
        benchmark_ref,
        benchmark_price,
        benchmark_source,
    })
}

fn apply_user_benchmark(
    event: &ExecutionEventContract,
    report: &mut FillQualityReport,
    selection: &UserBenchmarkSelection,
) {
    report.benchmark_reference = FillQualityBenchmark {
        benchmark_ref: selection.benchmark_ref.clone(),
        benchmark_price: Some(selection.benchmark_price),
        benchmark_source: selection.benchmark_source.clone(),
    };
    report.slippage_amount =
        event
            .fill
            .as_ref()
            .and_then(|fill| fill.fill_price)
            .map(|fill_price| match event.order_intent.side {
                OrderSide::Buy => fill_price - selection.benchmark_price,
                OrderSide::Sell => selection.benchmark_price - fill_price,
            });
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_contracts::{
        ExecutionAccountScope, ExecutionFill, ExecutionMode, ExecutionOrderIntent, FillStatus,
        OrderSide, OrderType, QuoteSnapshot, ReplayRef, SourceProvenance,
        EXECUTION_EVENT_SCHEMA_VERSION,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn fill_quality_service_persists_report_exports_replay_and_journal_evidence() {
        let service = TradeAssemblyService::test_local(temp_db("happy"));
        let result = service.fill_quality_analysis(json!({
            "analysisId": "analysis-happy",
            "strategyId": "strategy-1",
            "executionEvents": [event("fill-1", 2.0, 10.10, 0.02, 1_400), event("fill-2", 1.0, 10.20, 0.01, 1_500)]
        }));

        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["summary"]["fillCount"], json!(2));
        assert_eq!(result["summary"]["aggregateQuantity"], json!(3.0));
        assert_eq!(result["summary"]["aggregateFees"], json!(0.03));
        assert_eq!(result["summary"]["aggregateSlippageAmount"], json!(0.4));
        assert_eq!(result["summary"]["averageSlippagePerUnit"], json!(0.133333));
        assert_eq!(
            result["exportRefs"]["csv"]["uri"],
            json!("tradeassembly://artifact/fill-quality/analysis-happy/fills.csv")
        );
        assert_eq!(
            result["journalEvidence"]["eventType"],
            json!("fill_quality.analysis.completed")
        );
        assert!(service
            .runtime()
            .storage
            .get_json(REPORT_NS, "analysis-happy")
            .expect("read report")
            .is_some());
    }

    #[test]
    fn fill_quality_service_reports_partial_fills_missing_and_stale_quotes() {
        let service = TradeAssemblyService::test_local(temp_db("warnings"));
        let mut partial = event("fill-partial", 0.5, 10.25, 0.0, 2_500);
        partial["eventKind"] = json!("partial_fill");
        partial["fill"]["status"] = json!("partially_filled");
        partial["decisionQuote"] = Value::Null;
        partial["fillQuote"]["staleAfterUnixMs"] = json!(2_000);

        let result = service.fill_quality_analysis(json!({
            "analysisId": "analysis-warnings",
            "executionEvents": [partial]
        }));

        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["summary"]["fillCount"], json!(1));
        assert_eq!(result["summary"]["warningCount"], json!(2));
        assert_eq!(result["summary"]["errorCount"], json!(2));
        assert_eq!(result["perFill"][0]["slippageAmount"], Value::Null);
        let rendered = serde_json::to_string(&result).expect("serialize result");
        assert!(rendered.contains("missing_decision_quote"));
        assert!(rendered.contains("stale_fill_quote"));
    }

    #[test]
    fn fill_quality_service_applies_user_selected_benchmark() {
        let service = TradeAssemblyService::test_local(temp_db("benchmark"));
        let result = service.fill_quality_analysis(json!({
            "analysisId": "analysis-benchmark",
            "strategyId": "strategy-1",
            "benchmark": {
                "benchmarkRef": "user-benchmark-vwap-1m",
                "benchmarkPrice": 10.05,
                "benchmarkSource": "user_selected_vwap"
            },
            "executionEvents": [event("fill-1", 2.0, 10.10, 0.02, 1_400), event("fill-2", 1.0, 10.20, 0.01, 1_500)]
        }));

        assert_eq!(
            result["benchmarkSelection"],
            json!("user-benchmark-vwap-1m")
        );
        assert_eq!(result["summary"]["aggregateSlippageAmount"], json!(0.25));
        assert_eq!(result["summary"]["averageSlippagePerUnit"], json!(0.083333));
        assert_eq!(
            result["perFill"][0]["benchmarkReference"]["benchmarkSource"],
            json!("user_selected_vwap")
        );
        assert_eq!(
            result["perFill"][0]["benchmarkReference"]["benchmarkPrice"],
            json!(10.05)
        );
    }

    #[test]
    fn fill_quality_service_summarizes_rejects_cancels_and_provider_caveats() {
        let service = TradeAssemblyService::test_local(temp_db("rejects"));
        let mut reject = event("reject-1", 0.0, 0.0, 0.0, 1_400);
        reject["eventKind"] = json!("reject");
        reject["fill"]["status"] = json!("rejected");
        reject["fill"]["filledQuantity"] = Value::Null;
        reject["fill"]["fillPrice"] = Value::Null;
        reject["fill"]["filledAtUnixMs"] = Value::Null;
        reject["fill"]["rejectReason"] = json!("provider_rejected_buying_power");
        let mut manual = event("manual-1", 1.0, 10.0, 0.0, 1_200);
        manual["eventKind"] = json!("manual_import");
        manual["mode"] = json!("manual_import");

        let result = service.fill_quality_analysis(json!({
            "analysisId": "analysis-rejects",
            "executionEvents": [reject, manual],
            "providerCapabilityCaveats": [{"code": "missing_venue", "message": "Venue metadata was not supplied by provider."}]
        }));

        assert_eq!(result["summary"]["rejectCount"], json!(1));
        assert_eq!(
            result["rejectCancelSummary"]["rejects"][0]["reason"],
            json!("provider_rejected_buying_power")
        );
        assert_eq!(
            result["providerCapabilityCaveats"]
                .as_array()
                .expect("caveats")
                .len(),
            2
        );
        assert_eq!(result["perFill"].as_array().expect("fills").len(), 2);
    }

    #[test]
    fn fill_quality_service_is_deterministic_and_does_not_emit_advice_or_raw_secrets() {
        let service = TradeAssemblyService::test_local(temp_db("deterministic"));
        let mut raw_secret = event("secret-1", 1.0, 10.1, 0.0, 1_400);
        raw_secret["orderIntent"]["venueMetadata"]["api_secret"] = json!("SHOULD_NOT_LEAK");
        let request =
            json!({"analysisId": "analysis-deterministic", "executionEvents": [raw_secret]});

        let first = service.fill_quality_analysis(request.clone());
        let second = service.fill_quality_analysis(request);
        let rendered = serde_json::to_string(&first).expect("serialize result");

        assert_eq!(first["perFill"], second["perFill"]);
        assert_eq!(first["summary"], second["summary"]);
        assert!(rendered.contains("raw_secret_rejected"));
        assert!(!rendered.contains("SHOULD_NOT_LEAK"));
        for forbidden in [
            "you should",
            "we recommend",
            "best broker",
            "buy now",
            "sell now",
            "increase size",
        ] {
            assert!(
                !rendered.to_ascii_lowercase().contains(forbidden),
                "fill quality service emitted advice phrase: {forbidden}"
            );
        }
    }

    fn event(
        fill_id: &str,
        quantity: f64,
        fill_price: f64,
        fee_amount: f64,
        filled_at_unix_ms: u64,
    ) -> Value {
        serde_json::to_value(ExecutionEventContract {
            schema_version: EXECUTION_EVENT_SCHEMA_VERSION.to_string(),
            event_id: format!("event-{fill_id}"),
            event_kind: ExecutionEventKind::PaperFill,
            mode: ExecutionMode::Paper,
            order_intent: ExecutionOrderIntent {
                order_intent_id: format!("intent-{fill_id}"),
                strategy_id: "strategy-1".to_string(),
                instrument_id: "ul:equity:SPY".to_string(),
                side: OrderSide::Buy,
                order_type: OrderType::Limit,
                quantity,
                limit_price: Some(10.25),
                created_at_unix_ms: 1_000,
                provider_ref: Some("alpaca-paper".to_string()),
                user_activation_ref: Some("activation-user-approved".to_string()),
                account_scope: ExecutionAccountScope {
                    mode: ExecutionMode::Paper,
                    account_scope_ref: "acct_scope_redacted".to_string(),
                    broker_account_id: None,
                    raw_account_identifier_exposed: false,
                },
                venue_metadata: BTreeMap::from([("venue".to_string(), json!("paper"))]),
                source_provenance: provenance("order_intent"),
            },
            fill: Some(ExecutionFill {
                fill_id: fill_id.to_string(),
                status: FillStatus::Filled,
                filled_quantity: Some(quantity),
                fill_price: Some(fill_price),
                filled_at_unix_ms: Some(filled_at_unix_ms),
                fee_amount: Some(fee_amount),
                fee_currency: Some("USD".to_string()),
                reject_reason: None,
                cancel_reason: None,
                provider_venue_metadata: BTreeMap::from([("venue".to_string(), json!("paper"))]),
                source_provenance: provenance("fill"),
            }),
            decision_quote: Some(quote("quote-decision", 9.95, 10.05, 1_000)),
            fill_quote: Some(quote("quote-fill", 10.00, 10.10, filled_at_unix_ms)),
            replay_ref: ReplayRef {
                replay_ref: format!("replay://fills/{fill_id}"),
                deterministic: true,
            },
        })
        .expect("event serializes")
    }

    fn quote(ref_id: &str, bid: f64, ask: f64, captured_at_unix_ms: u64) -> QuoteSnapshot {
        QuoteSnapshot {
            quote_ref: ref_id.to_string(),
            instrument_id: "ul:equity:SPY".to_string(),
            bid: Some(bid),
            ask: Some(ask),
            last: None,
            captured_at_unix_ms,
            stale_after_unix_ms: Some(captured_at_unix_ms + 1_000),
            provider_ref: "alpaca-paper".to_string(),
            venue_metadata: BTreeMap::from([("feed".to_string(), json!("iex"))]),
            source_provenance: provenance(ref_id),
        }
    }

    fn provenance(source_ref: &str) -> SourceProvenance {
        SourceProvenance {
            source: "fixture".to_string(),
            source_ref: source_ref.to_string(),
            collected_at_unix_ms: 1_000,
            raw_secrets_exposed: false,
        }
    }

    fn temp_db(name: &str) -> String {
        format!(
            "{}/tradeassembly-fill-quality-{name}-{}.db",
            std::env::temp_dir().display(),
            std::process::id()
        )
    }
}
