// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{strategy_id_from, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::attribution_journal::{
    attribution_journal_hash, ArtifactHashRef, AttributionEdge, AttributionJournalContract,
    AttributionRefKind, AttributionSourceProvenance, AttributionStableRef, AttributionWarning,
    AttributionWarningCode, AttributionWarningSeverity, CodeRef, DataSnapshotRef, DrawdownInput,
    HoldingPeriodSummary, JournalAnalyticsOutput, JournalGroupingDimension, PnlAttributionInput,
    ReplayRef, ReplayReviewContract, SkipRejectSummary, UserNoteRef, WinLossSummary,
    ATTRIBUTION_JOURNAL_SCHEMA_VERSION,
};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const REVIEW_NS: &str = "attribution_journal_reviews";
const EXPORT_NS: &str = "attribution_journal_exports";
const DEFAULT_NOW_UNIX_MS: u64 = 1_785_000_000_000;

#[derive(Clone, Debug)]
struct ExecutionRow {
    event_id: String,
    ref_uri: String,
    content_hash: String,
    underlying: String,
    instrument_id: String,
    leg_id: String,
    entry_at_unix_ms: u64,
    exit_at_unix_ms: u64,
    gross_pnl: f64,
    fees: f64,
    net_pnl: f64,
    quantity: f64,
    outcome: String,
    fill_quality_ref: String,
}

pub(crate) fn analyze(service: &TradeAssemblyService, request: Value) -> Value {
    let strategy_id = strategy_id_from(&request);
    let review_id = string_field(&request, &["reviewId", "review_id"])
        .unwrap_or_else(|| "attribution_journal_latest".to_string());
    let generated_at_unix_ms = u64_field(
        &request,
        &["generatedAtUnixMs", "generated_at_unix_ms", "asOfUnixMs"],
    )
    .unwrap_or(DEFAULT_NOW_UNIX_MS);
    let studio_base_url = string_field(&request, &["studioBaseUrl", "studio_base_url"])
        .unwrap_or_else(|| "http://127.0.0.1:3001".to_string());
    let executions = execution_rows(&request);
    let mut service_warnings = Vec::new();
    let mut refs = stable_refs(
        &request,
        &strategy_id,
        generated_at_unix_ms,
        &mut service_warnings,
    );
    refs.extend(
        executions
            .iter()
            .map(|row| execution_ref(row, generated_at_unix_ms)),
    );
    let user_notes = user_notes(&request, generated_at_unix_ms, &mut service_warnings);
    refs.extend(
        user_notes
            .iter()
            .map(|note| user_note_stable_ref(note, generated_at_unix_ms)),
    );
    let analytics = analytics_output(&request, &executions, generated_at_unix_ms);
    let replay_review = replay_review(
        &request,
        &review_id,
        &refs,
        generated_at_unix_ms,
        &mut service_warnings,
    );
    let mut metadata = sanitized_metadata(&request, &mut service_warnings);
    metadata.insert(
        "computedBy".to_string(),
        json!("tradeassembly.attribution_journal.service"),
    );
    for row in &executions {
        metadata.insert(
            format!("{}.underlying", row.event_id),
            json!(row.underlying.clone()),
        );
        metadata.insert(
            format!("{}.instrumentId", row.event_id),
            json!(row.instrument_id.clone()),
        );
        metadata.insert(format!("{}.legId", row.event_id), json!(row.leg_id.clone()));
    }
    let mut contract = AttributionJournalContract {
        schema_version: ATTRIBUTION_JOURNAL_SCHEMA_VERSION.to_string(),
        review_id: review_id.clone(),
        strategy_id: strategy_id.clone(),
        generated_at_unix_ms,
        refs,
        attribution_edges: Vec::new(),
        user_notes,
        analytics,
        replay_review,
        warnings: service_warnings,
        replay_refs: vec![ReplayRef {
            replay_ref: format!("tradeassembly://attribution-journal/{review_id}/replay"),
            deterministic: true,
        }],
        metadata,
        no_advice_notice: LEGAL_BOUNDARY.to_string(),
    };
    contract.attribution_edges = attribution_edges(&contract.refs);
    let validation = contract.validate();
    let contract_hash = attribution_journal_hash(&contract);
    let export_refs = export_refs(&review_id);
    let deep_links = json!({
        "strategy": {"href": format!("{}/app/strategies/{strategy_id}", studio_base_url.trim_end_matches('/'))},
        "execution": {"href": format!("{}/app/strategies/{strategy_id}/execution?attributionJournal={review_id}", studio_base_url.trim_end_matches('/'))},
        "journal": {"href": format!("{}/app/reviews/{review_id}/journal?strategyId={strategy_id}&attributionJournal={review_id}", studio_base_url.trim_end_matches('/'))},
        "replay": {"href": format!("{}/app/reviews/{review_id}/evidence?strategyId={strategy_id}&attributionJournal={review_id}", studio_base_url.trim_end_matches('/'))},
        "research": {"href": format!("{}/app/strategies/{strategy_id}/research?attributionJournal={review_id}", studio_base_url.trim_end_matches('/'))}
    });
    let mut result = json!({
        "schemaVersion": "tradeassembly.attribution_journal.service.v1",
        "kind": "attribution_journal",
        "ok": validation.ok,
        "reviewId": review_id,
        "strategyId": strategy_id,
        "contractHash": contract_hash,
        "summary": summary(&contract, validation.warnings.len()),
        "analytics": contract.analytics,
        "attributionTable": attribution_table(&contract),
        "attribution": {"tables": attribution_table(&contract), "edges": contract.attribution_edges},
        "journal": {"tradeRows": attribution_table(&contract), "totals": pnl_input_summary(&contract.analytics.pnl_inputs)},
        "skipRejectSummary": contract.analytics.skip_reject_summaries,
        "pnlInputSummary": pnl_input_summary(&contract.analytics.pnl_inputs),
        "holdingPeriodSummary": contract.analytics.holding_period_summaries,
        "replayReview": contract.replay_review,
        "warningSummary": warning_summary(&validation.warnings),
        "warnings": validation.warnings,
        "contract": contract,
        "exportRefs": export_refs,
        "replayRefs": {
            "commands": [
                format!("tradeassembly attribution-journal review --review-id {} --db {}", result_placeholder(), service.db()),
                format!("tradeassembly attribution-journal replay --review-id {} --db {}", result_placeholder(), service.db())
            ],
            "refs": [{"replayRef": format!("tradeassembly://attribution-journal/{}/replay", result_placeholder()), "deterministic": true}]
        },
        "deepLinks": deep_links,
        "agentSummary": {
            "scope": "Local attribution and journal analytics review",
            "results": {
                "closedCount": result_count_placeholder(),
                "netPnl": result_count_placeholder(),
                "warningCount": result_count_placeholder()
            },
            "studioDeepLinks": {
                "execution": deep_links["execution"]["href"]
            }
        },
        "sideEffects": {
            "strategyLogicChanged": false,
            "credentialsTouched": false,
            "brokerStateChanged": false,
            "activationChanged": false
        },
        "noAdvice": LEGAL_BOUNDARY,
        "noAdviceNotice": LEGAL_BOUNDARY
    });
    let review_id = result["reviewId"].as_str().unwrap_or("review").to_string();
    result["replayRefs"] = json!({
        "commands": [
            format!("tradeassembly attribution-journal review --review-id {review_id} --db {}", service.db()),
            format!("tradeassembly attribution-journal replay --review-id {review_id} --db {}", service.db())
        ],
        "refs": [{"replayRef": format!("tradeassembly://attribution-journal/{review_id}/replay"), "deterministic": true}]
    });
    result["agentSummary"]["results"]["closedCount"] = result["summary"]["closedCount"].clone();
    result["agentSummary"]["results"]["netPnl"] = result["summary"]["netPnl"].clone();
    result["agentSummary"]["results"]["warningCount"] = result["summary"]["warningCount"].clone();
    result["agentDisplaySummary"] = json!(format!(
        "Reviewed attribution journal {review_id} from {} refs, {} closed executions, {} skip/reject reasons, {} warnings, and deterministic replay/export refs.",
        result["summary"]["refCount"].as_u64().unwrap_or(0),
        result["summary"]["closedCount"].as_u64().unwrap_or(0),
        result["summary"]["skipRejectCount"].as_u64().unwrap_or(0),
        result["summary"]["warningCount"].as_u64().unwrap_or(0)
    ));
    persist(service, result)
}

pub(crate) fn latest(service: &TradeAssemblyService) -> Option<Value> {
    service
        .runtime()
        .storage
        .list_json(REVIEW_NS)
        .ok()
        .and_then(|mut values| values.pop().map(|(_, value)| value))
}

fn persist(service: &TradeAssemblyService, mut result: Value) -> Value {
    let review_id = result["reviewId"]
        .as_str()
        .unwrap_or("attribution_journal_latest")
        .to_string();
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("attribution_journal.review:{review_id}"))
            .expect("valid attribution journal idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(REVIEW_NS, &review_id, result.clone(), &context)
        .expect("persist attribution journal review");
    service
        .runtime()
        .storage
        .put_json(
            EXPORT_NS,
            &review_id,
            result["exportRefs"].clone(),
            &context,
        )
        .expect("persist attribution journal exports");
    let journal_id = service
        .runtime()
        .record_side_effect(
            "attribution_journal.review.completed",
            result.clone(),
            &context,
        )
        .expect("record attribution journal evidence");
    result["journalEvidence"] = json!({
        "eventType": "attribution_journal.review.completed",
        "journalId": journal_id,
        "storageNamespace": REVIEW_NS,
        "exportStorageNamespace": EXPORT_NS,
        "reviewId": review_id
    });
    service
        .runtime()
        .storage
        .put_json(REVIEW_NS, &review_id, result.clone(), &context)
        .expect("persist attribution journal review with journal evidence");
    result
}

fn stable_refs(
    request: &Value,
    strategy_id: &str,
    generated_at_unix_ms: u64,
    warnings: &mut Vec<AttributionWarning>,
) -> Vec<AttributionStableRef> {
    let mut refs = Vec::new();
    refs.push(stable_ref_from_value(
        request.get("strategyVersion"),
        AttributionRefKind::StrategyVersion,
        "strategy-version",
        &format!("tradeassembly://strategy/{strategy_id}/versions/latest"),
        "sha256:strategy",
        generated_at_unix_ms,
    ));
    for (key, kind, default_id, default_uri, default_hash) in [
        (
            "selectorRuns",
            AttributionRefKind::SelectorRun,
            "selector-local",
            "tradeassembly://selector/local",
            "sha256:selector",
        ),
        (
            "backtestReports",
            AttributionRefKind::BacktestRun,
            "backtest-local",
            "tradeassembly://report/backtest-local",
            "sha256:backtest",
        ),
        (
            "ledgerCheckpoints",
            AttributionRefKind::LedgerCheckpoint,
            "ledger-local",
            "tradeassembly://ledger/local",
            "sha256:ledger",
        ),
        (
            "valuationSnapshots",
            AttributionRefKind::ValuationSnapshot,
            "valuation-local",
            "tradeassembly://valuation/local",
            "sha256:valuation",
        ),
        (
            "fillQualityReports",
            AttributionRefKind::FillQualityReport,
            "fill-quality-local",
            "tradeassembly://report/fill-quality-local",
            "sha256:fillquality",
        ),
        (
            "artifactRefs",
            AttributionRefKind::ReportArtifact,
            "report-local",
            "tradeassembly://report/local",
            "sha256:report",
        ),
    ] {
        let values = array_field(request, &[key, &snake_case_key(key)]);
        if values.is_empty() {
            refs.push(stable_ref_from_value(
                None,
                kind,
                default_id,
                default_uri,
                default_hash,
                generated_at_unix_ms,
            ));
        } else {
            refs.extend(values.iter().map(|value| {
                stable_ref_from_value(
                    Some(value),
                    kind.clone(),
                    default_id,
                    default_uri,
                    default_hash,
                    generated_at_unix_ms,
                )
            }));
        }
    }
    if refs.iter().any(|stable_ref| {
        stable_ref
            .observed_hash
            .as_ref()
            .is_some_and(|observed| observed != &stable_ref.content_hash)
    }) {
        warnings.push(service_warning(
            AttributionWarningCode::HashMismatch,
            "A referenced artifact observed hash does not match its recorded content hash.",
            "tradeassembly://attribution/hash-mismatch",
        ));
    }
    refs
}

fn stable_ref_from_value(
    value: Option<&Value>,
    ref_kind: AttributionRefKind,
    default_id: &str,
    default_uri: &str,
    default_hash: &str,
    generated_at_unix_ms: u64,
) -> AttributionStableRef {
    let value = value.unwrap_or(&Value::Null);
    let ref_id = string_field(value, &["refId", "ref_id", "id"]).unwrap_or(default_id.to_string());
    let ref_uri =
        string_field(value, &["refUri", "ref_uri", "uri"]).unwrap_or(default_uri.to_string());
    let content_hash =
        string_field(value, &["contentHash", "content_hash"]).unwrap_or(default_hash.to_string());
    let as_of_unix_ms =
        u64_field(value, &["asOfUnixMs", "as_of_unix_ms"]).unwrap_or(generated_at_unix_ms);
    AttributionStableRef {
        ref_id: ref_id.clone(),
        ref_kind,
        ref_uri,
        content_hash,
        observed_hash: string_field(value, &["observedHash", "observed_hash"]),
        as_of_unix_ms,
        stale_after_unix_ms: u64_field(value, &["staleAfterUnixMs", "stale_after_unix_ms"]),
        verified_data_source: bool_field(value, &["verifiedDataSource", "verified_data_source"])
            .unwrap_or(true),
        provenance: provenance(
            "local_journal",
            &format!("journal://{ref_id}"),
            as_of_unix_ms,
        ),
    }
}

fn execution_rows(request: &Value) -> Vec<ExecutionRow> {
    array_field(request, &["executionEvents", "execution_events", "events"])
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let event_id = string_field(value, &["eventId", "event_id", "id"])
                .unwrap_or_else(|| format!("execution-{index}"));
            ExecutionRow {
                ref_uri: string_field(value, &["refUri", "ref_uri", "uri"])
                    .unwrap_or_else(|| format!("tradeassembly://execution/{event_id}")),
                content_hash: string_field(value, &["contentHash", "content_hash"])
                    .unwrap_or_else(|| hash_string(&event_id)),
                underlying: string_field(value, &["underlying", "symbol"])
                    .unwrap_or_else(|| "UNKNOWN".to_string()),
                instrument_id: string_field(value, &["instrumentId", "instrument_id"])
                    .unwrap_or_else(|| "instrument-unknown".to_string()),
                leg_id: string_field(value, &["legId", "leg_id"])
                    .unwrap_or_else(|| "leg".to_string()),
                entry_at_unix_ms: u64_field(value, &["entryAtUnixMs", "entry_at_unix_ms"])
                    .unwrap_or(DEFAULT_NOW_UNIX_MS),
                exit_at_unix_ms: u64_field(value, &["exitAtUnixMs", "exit_at_unix_ms"])
                    .unwrap_or(DEFAULT_NOW_UNIX_MS),
                gross_pnl: f64_field(value, &["grossPnl", "gross_pnl"]).unwrap_or_default(),
                fees: f64_field(value, &["fees", "feeAmount", "fee_amount"]).unwrap_or_default(),
                net_pnl: f64_field(value, &["netPnl", "net_pnl"]).unwrap_or_else(|| {
                    f64_field(value, &["grossPnl", "gross_pnl"]).unwrap_or_default()
                        - f64_field(value, &["fees", "feeAmount", "fee_amount"]).unwrap_or_default()
                }),
                quantity: f64_field(value, &["quantity", "filledQuantity", "filled_quantity"])
                    .unwrap_or(1.0),
                outcome: string_field(value, &["outcome"]).unwrap_or_else(|| "flat".to_string()),
                fill_quality_ref: string_field(value, &["fillQualityRef", "fill_quality_ref"])
                    .unwrap_or_else(|| "fill-quality-local".to_string()),
                event_id,
            }
        })
        .collect()
}

fn execution_ref(row: &ExecutionRow, generated_at_unix_ms: u64) -> AttributionStableRef {
    AttributionStableRef {
        ref_id: row.event_id.clone(),
        ref_kind: AttributionRefKind::ExecutionEvent,
        ref_uri: row.ref_uri.clone(),
        content_hash: row.content_hash.clone(),
        observed_hash: None,
        as_of_unix_ms: generated_at_unix_ms,
        stale_after_unix_ms: None,
        verified_data_source: true,
        provenance: provenance(
            "local_journal",
            &format!("journal://{}", row.event_id),
            generated_at_unix_ms,
        ),
    }
}

fn user_notes(
    request: &Value,
    generated_at_unix_ms: u64,
    warnings: &mut Vec<AttributionWarning>,
) -> Vec<UserNoteRef> {
    let notes = array_field(request, &["userNotes", "user_notes"]);
    let values = if notes.is_empty() {
        vec![json!({
            "noteId": "note-local",
            "noteRef": "tradeassembly://notes/local",
            "contentHash": "sha256:note",
            "explicitlySaved": true,
            "createdAtUnixMs": generated_at_unix_ms
        })]
    } else {
        notes.into_iter().cloned().collect()
    };
    values
        .iter()
        .map(|value| {
            let note_id = string_field(value, &["noteId", "note_id", "id"])
                .unwrap_or_else(|| "note-local".to_string());
            let explicitly_saved =
                bool_field(value, &["explicitlySaved", "explicitly_saved"]).unwrap_or(false);
            if !explicitly_saved {
                warnings.push(service_warning(
                    AttributionWarningCode::IncompleteJournalState,
                    "Unsaved user notes are excluded from trusted journal attribution.",
                    "tradeassembly://attribution/unsaved-note",
                ));
            }
            UserNoteRef {
                note_ref: string_field(value, &["noteRef", "note_ref", "refUri"])
                    .unwrap_or_else(|| format!("tradeassembly://notes/{note_id}")),
                content_hash: string_field(value, &["contentHash", "content_hash"])
                    .unwrap_or_else(|| hash_string(&note_id)),
                explicitly_saved,
                created_at_unix_ms: u64_field(
                    value,
                    &["createdAtUnixMs", "created_at_unix_ms", "asOfUnixMs"],
                )
                .unwrap_or(generated_at_unix_ms),
                provenance: provenance(
                    "local_journal",
                    &format!("journal://{note_id}"),
                    generated_at_unix_ms,
                ),
                note_id,
            }
        })
        .collect()
}

fn user_note_stable_ref(note: &UserNoteRef, generated_at_unix_ms: u64) -> AttributionStableRef {
    AttributionStableRef {
        ref_id: note.note_id.clone(),
        ref_kind: AttributionRefKind::UserNote,
        ref_uri: note.note_ref.clone(),
        content_hash: note.content_hash.clone(),
        observed_hash: None,
        as_of_unix_ms: generated_at_unix_ms,
        stale_after_unix_ms: None,
        verified_data_source: true,
        provenance: note.provenance.clone(),
    }
}

fn analytics_output(
    request: &Value,
    executions: &[ExecutionRow],
    generated_at_unix_ms: u64,
) -> JournalAnalyticsOutput {
    let grouping_dimensions = grouping_dimensions(request);
    let pnl_inputs = executions
        .iter()
        .map(|row| PnlAttributionInput {
            input_id: format!("pnl-{}", row.event_id),
            basis: "closed_execution".to_string(),
            currency: "USD".to_string(),
            gross_pnl: round6(row.gross_pnl),
            fees: round6(row.fees),
            net_pnl: round6(row.net_pnl),
            quantity: round6(row.quantity),
            strategy_ref: "strategy-version".to_string(),
            execution_ref: row.event_id.clone(),
            valuation_ref: Some("valuation-local".to_string()),
        })
        .collect::<Vec<_>>();
    JournalAnalyticsOutput {
        grouping_dimensions,
        pnl_inputs,
        win_loss_summary: win_loss_summary(executions),
        drawdown_inputs: vec![drawdown_input(executions)],
        holding_period_summaries: holding_period_summaries(executions),
        skip_reject_summaries: skip_reject_summaries(request),
        fill_quality_refs: fill_quality_refs(executions),
        provenance: provenance(
            "local_journal",
            "journal://attribution-journal/analytics",
            generated_at_unix_ms,
        ),
    }
}

fn grouping_dimensions(request: &Value) -> Vec<JournalGroupingDimension> {
    let requested = array_field(request, &["groupingDimensions", "grouping_dimensions"]);
    if requested.is_empty() {
        return vec![
            JournalGroupingDimension::StrategyVersion,
            JournalGroupingDimension::Underlying,
            JournalGroupingDimension::Outcome,
        ];
    }
    requested
        .iter()
        .filter_map(|value| value.as_str())
        .filter_map(|value| match value {
            "strategy_version" => Some(JournalGroupingDimension::StrategyVersion),
            "underlying" => Some(JournalGroupingDimension::Underlying),
            "instrument" => Some(JournalGroupingDimension::Instrument),
            "leg" => Some(JournalGroupingDimension::Leg),
            "entry_date" => Some(JournalGroupingDimension::EntryDate),
            "exit_date" => Some(JournalGroupingDimension::ExitDate),
            "holding_period_bucket" => Some(JournalGroupingDimension::HoldingPeriodBucket),
            "outcome" => Some(JournalGroupingDimension::Outcome),
            "skip_reason" => Some(JournalGroupingDimension::SkipReason),
            "reject_reason" => Some(JournalGroupingDimension::RejectReason),
            "fill_quality_bucket" => Some(JournalGroupingDimension::FillQualityBucket),
            "user_tag" => Some(JournalGroupingDimension::UserTag),
            _ => None,
        })
        .collect()
}

fn win_loss_summary(executions: &[ExecutionRow]) -> WinLossSummary {
    let wins = executions
        .iter()
        .filter(|row| row.outcome == "win" || row.net_pnl > 0.0)
        .count() as u64;
    let losses = executions
        .iter()
        .filter(|row| row.outcome == "loss" || row.net_pnl < 0.0)
        .count() as u64;
    let total_closed = executions.len() as u64;
    WinLossSummary {
        total_closed,
        wins,
        losses,
        flat: total_closed.saturating_sub(wins + losses),
    }
}

fn drawdown_input(executions: &[ExecutionRow]) -> DrawdownInput {
    let mut equity = 0.0;
    let mut peak = 0.0;
    let mut trough = 0.0;
    for row in executions {
        equity += row.net_pnl;
        if equity > peak {
            peak = equity;
        }
        if equity < trough {
            trough = equity;
        }
    }
    DrawdownInput {
        input_id: "drawdown-local".to_string(),
        equity_curve_ref: "artifact://equity-curve".to_string(),
        content_hash: hash_string(&format!("{peak}:{trough}:{}", executions.len())),
        peak_value: round6(peak),
        trough_value: round6(trough),
    }
}

fn holding_period_summaries(executions: &[ExecutionRow]) -> Vec<HoldingPeriodSummary> {
    let mut buckets: BTreeMap<String, (u64, u64, f64)> = BTreeMap::new();
    for row in executions {
        let holding_ms = row.exit_at_unix_ms.saturating_sub(row.entry_at_unix_ms);
        let bucket = if holding_ms <= 86_400_000 {
            "0-1d"
        } else if holding_ms <= 604_800_000 {
            "1-7d"
        } else {
            "7d+"
        }
        .to_string();
        let entry = buckets.entry(bucket).or_insert((0, 0, 0.0));
        entry.0 += 1;
        entry.1 += holding_ms;
        entry.2 += row.net_pnl;
    }
    buckets
        .into_iter()
        .map(
            |(bucket, (trade_count, total_ms, net_pnl))| HoldingPeriodSummary {
                bucket,
                trade_count,
                average_holding_ms: total_ms.checked_div(trade_count).unwrap_or(0),
                net_pnl: round6(net_pnl),
            },
        )
        .collect()
}

fn skip_reject_summaries(request: &Value) -> Vec<SkipRejectSummary> {
    let mut summaries = Vec::new();
    summaries.extend(reason_summaries(request, &["skips", "skipEvents"], "skip"));
    summaries.extend(reason_summaries(
        request,
        &["rejects", "rejectEvents"],
        "reject",
    ));
    if summaries.is_empty() {
        summaries.push(SkipRejectSummary {
            reason_code: "none_recorded".to_string(),
            reason_kind: "none".to_string(),
            count: 0,
            source_refs: vec!["journal://attribution-journal".to_string()],
        });
    }
    summaries
}

fn reason_summaries(request: &Value, keys: &[&str], kind: &str) -> Vec<SkipRejectSummary> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for value in array_field(request, keys) {
        let reason = string_field(value, &["reasonCode", "reason_code", "reason"])
            .unwrap_or_else(|| "unspecified".to_string());
        let source = string_field(value, &["sourceRef", "source_ref"])
            .unwrap_or_else(|| "journal://unknown".to_string());
        grouped.entry(reason).or_default().push(source);
    }
    grouped
        .into_iter()
        .map(|(reason_code, source_refs)| SkipRejectSummary {
            reason_code,
            reason_kind: kind.to_string(),
            count: source_refs.len() as u64,
            source_refs,
        })
        .collect()
}

fn fill_quality_refs(executions: &[ExecutionRow]) -> Vec<String> {
    let refs = executions
        .iter()
        .map(|row| row.fill_quality_ref.clone())
        .collect::<BTreeSet<_>>();
    if refs.is_empty() {
        vec!["fill-quality-local".to_string()]
    } else {
        refs.into_iter().collect()
    }
}

fn replay_review(
    request: &Value,
    review_id: &str,
    refs: &[AttributionStableRef],
    generated_at_unix_ms: u64,
    warnings: &mut Vec<AttributionWarning>,
) -> ReplayReviewContract {
    let data_snapshots = data_snapshots(request, generated_at_unix_ms);
    if data_snapshots
        .iter()
        .any(|snapshot| !snapshot.verified_data_source)
    {
        warnings.push(service_warning(
            AttributionWarningCode::UnverifiedDataSource,
            "Replay review contains an unverified data snapshot ref.",
            "tradeassembly://attribution/unverified-data-source",
        ));
    }
    if data_snapshots.iter().any(|snapshot| {
        snapshot
            .stale_after_unix_ms
            .is_some_and(|stale_after| snapshot.as_of_unix_ms > stale_after)
    }) {
        warnings.push(service_warning(
            AttributionWarningCode::StaleArtifact,
            "Replay review contains a stale data snapshot ref.",
            "tradeassembly://attribution/stale-data",
        ));
    }
    ReplayReviewContract {
        replay_id: format!("replay-{review_id}"),
        reproduced_result_ref: format!("tradeassembly://attribution-journal/{review_id}"),
        input_refs: refs
            .iter()
            .map(|stable_ref| stable_ref.ref_id.clone())
            .collect(),
        code_refs: code_refs(request),
        artifact_hashes: refs
            .iter()
            .map(|stable_ref| ArtifactHashRef {
                artifact_ref: stable_ref.ref_uri.clone(),
                content_hash: stable_ref.content_hash.clone(),
            })
            .collect(),
        data_snapshots,
        user_settings_hash: string_field(request, &["settingsHash", "settings_hash"])
            .unwrap_or_else(|| "sha256:settings".to_string()),
        deterministic: true,
        reproduced_at_unix_ms: generated_at_unix_ms,
    }
}

fn code_refs(request: &Value) -> Vec<CodeRef> {
    let values = array_field(request, &["codeRefs", "code_refs"]);
    if values.is_empty() {
        return vec![CodeRef {
            code_ref: "git://TradeAssembly/runtime-rs/src/service/attribution_journal.rs"
                .to_string(),
            commit_sha: "local".to_string(),
            content_hash: "sha256:code".to_string(),
        }];
    }
    values
        .iter()
        .map(|value| CodeRef {
            code_ref: string_field(value, &["codeRef", "code_ref"]).unwrap_or_else(|| {
                "git://TradeAssembly/runtime-rs/src/service/attribution_journal.rs".to_string()
            }),
            commit_sha: string_field(value, &["commitSha", "commit_sha"])
                .unwrap_or_else(|| "local".to_string()),
            content_hash: string_field(value, &["contentHash", "content_hash"])
                .unwrap_or_else(|| "sha256:code".to_string()),
        })
        .collect()
}

fn data_snapshots(request: &Value, generated_at_unix_ms: u64) -> Vec<DataSnapshotRef> {
    let values = array_field(request, &["dataSnapshots", "data_snapshots"]);
    if values.is_empty() {
        return vec![DataSnapshotRef {
            snapshot_ref: "snapshot://local".to_string(),
            content_hash: "sha256:snapshot".to_string(),
            as_of_unix_ms: generated_at_unix_ms,
            stale_after_unix_ms: None,
            verified_data_source: true,
            provenance: provenance(
                "local_journal",
                "journal://snapshot/local",
                generated_at_unix_ms,
            ),
        }];
    }
    values
        .iter()
        .map(|value| {
            let snapshot_ref = string_field(value, &["snapshotRef", "snapshot_ref"])
                .unwrap_or_else(|| "snapshot://local".to_string());
            let as_of_unix_ms =
                u64_field(value, &["asOfUnixMs", "as_of_unix_ms"]).unwrap_or(generated_at_unix_ms);
            DataSnapshotRef {
                snapshot_ref: snapshot_ref.clone(),
                content_hash: string_field(value, &["contentHash", "content_hash"])
                    .unwrap_or_else(|| "sha256:snapshot".to_string()),
                as_of_unix_ms,
                stale_after_unix_ms: u64_field(value, &["staleAfterUnixMs", "stale_after_unix_ms"]),
                verified_data_source: bool_field(
                    value,
                    &["verifiedDataSource", "verified_data_source"],
                )
                .unwrap_or(true),
                provenance: provenance("local_journal", &snapshot_ref, as_of_unix_ms),
            }
        })
        .collect()
}

fn attribution_edges(refs: &[AttributionStableRef]) -> Vec<AttributionEdge> {
    let root = refs
        .iter()
        .find(|stable_ref| stable_ref.ref_kind == AttributionRefKind::StrategyVersion)
        .map(|stable_ref| stable_ref.ref_id.clone())
        .or_else(|| refs.first().map(|stable_ref| stable_ref.ref_id.clone()))
        .unwrap_or_else(|| "strategy-version".to_string());
    refs.iter()
        .filter(|stable_ref| stable_ref.ref_id != root)
        .map(|stable_ref| {
            let edge_id = format!("edge-{root}-{}", stable_ref.ref_id);
            AttributionEdge {
                edge_id: edge_id.clone(),
                from_ref: root.clone(),
                to_ref: stable_ref.ref_id.clone(),
                relation: "descriptive_evidence_link".to_string(),
                evidence_hash: hash_string(&edge_id),
            }
        })
        .collect()
}

fn attribution_table(contract: &AttributionJournalContract) -> Vec<Value> {
    contract
        .analytics
        .pnl_inputs
        .iter()
        .map(|input| {
            let execution = contract
                .refs
                .iter()
                .find(|stable_ref| stable_ref.ref_id == input.execution_ref);
            json!({
                "inputId": input.input_id,
                "strategyRef": input.strategy_ref,
                "executionRef": input.execution_ref,
                "instrumentRef": execution.map(|stable_ref| stable_ref.ref_uri.clone()),
                "underlying": contract.metadata.get(&format!("{}.underlying", input.execution_ref)),
                "instrumentId": contract.metadata.get(&format!("{}.instrumentId", input.execution_ref)),
                "legId": contract.metadata.get(&format!("{}.legId", input.execution_ref)),
                "grossPnl": input.gross_pnl,
                "fees": input.fees,
                "netPnl": input.net_pnl,
                "quantity": input.quantity
            })
        })
        .collect()
}

fn summary(contract: &AttributionJournalContract, warning_count: usize) -> Value {
    let analytics = &contract.analytics;
    let net_pnl = analytics
        .pnl_inputs
        .iter()
        .map(|input| input.net_pnl)
        .sum::<f64>();
    json!({
        "refCount": contract.refs.len(),
        "closedCount": analytics.win_loss_summary.total_closed,
        "wins": analytics.win_loss_summary.wins,
        "losses": analytics.win_loss_summary.losses,
        "flat": analytics.win_loss_summary.flat,
        "netPnl": round6(net_pnl),
        "skipRejectCount": analytics.skip_reject_summaries.iter().map(|summary| summary.count).sum::<u64>(),
        "warningCount": warning_count,
        "groupingDimensions": analytics.grouping_dimensions,
        "replayDeterministic": contract.replay_review.deterministic,
    })
}

fn pnl_input_summary(inputs: &[PnlAttributionInput]) -> Value {
    json!({
        "inputCount": inputs.len(),
        "grossPnl": round6(inputs.iter().map(|input| input.gross_pnl).sum::<f64>()),
        "fees": round6(inputs.iter().map(|input| input.fees).sum::<f64>()),
        "netPnl": round6(inputs.iter().map(|input| input.net_pnl).sum::<f64>()),
        "quantity": round6(inputs.iter().map(|input| input.quantity).sum::<f64>()),
    })
}

fn warning_summary(warnings: &[AttributionWarning]) -> Value {
    let mut by_code = BTreeMap::new();
    for warning in warnings {
        *by_code
            .entry(format!("{:?}", warning.code).to_ascii_lowercase())
            .or_insert(0u64) += 1;
    }
    json!({
        "warningCount": warnings.iter().filter(|warning| warning.severity == AttributionWarningSeverity::Warning).count(),
        "errorCount": warnings.iter().filter(|warning| warning.severity == AttributionWarningSeverity::Error).count(),
        "byCode": by_code
    })
}

fn export_refs(review_id: &str) -> Value {
    json!({
        "json": {"uri": format!("tradeassembly://artifact/attribution-journal/{review_id}/review.json")},
        "csv": {"uri": format!("tradeassembly://artifact/attribution-journal/{review_id}/journal.csv")},
        "replay": {"uri": format!("tradeassembly://artifact/attribution-journal/{review_id}/replay.json")}
    })
}

fn sanitized_metadata(
    request: &Value,
    warnings: &mut Vec<AttributionWarning>,
) -> BTreeMap<String, Value> {
    let mut metadata = BTreeMap::new();
    if let Some(object) = request.get("metadata").and_then(Value::as_object) {
        for (key, value) in object {
            if secret_like(key) || value_contains_secret(value) {
                warnings.push(service_warning(
                    AttributionWarningCode::RawSecretRejected,
                    "Secret-shaped attribution metadata was rejected before persistence.",
                    "tradeassembly://attribution/metadata-redacted",
                ));
            } else {
                metadata.insert(key.clone(), value.clone());
            }
        }
    }
    metadata
}

fn service_warning(
    code: AttributionWarningCode,
    message: &str,
    replay_ref: &str,
) -> AttributionWarning {
    AttributionWarning {
        code,
        severity: AttributionWarningSeverity::Error,
        message: message.to_string(),
        replay_ref: replay_ref.to_string(),
    }
}

fn provenance(
    source: &str,
    source_ref: &str,
    collected_at_unix_ms: u64,
) -> AttributionSourceProvenance {
    AttributionSourceProvenance {
        source: source.to_string(),
        source_ref: source_ref.to_string(),
        collected_at_unix_ms,
        raw_secrets_exposed: false,
    }
}

fn array_field<'a>(value: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_array))
        .map(|values| values.iter().collect())
        .unwrap_or_default()
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(ToString::to_string)
}

fn u64_field(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
}

fn f64_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_f64))
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn snake_case_key(key: &str) -> String {
    let mut output = String::new();
    for (index, ch) in key.chars().enumerate() {
        if ch.is_uppercase() {
            if index > 0 {
                output.push('_');
            }
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push(ch);
        }
    }
    output
}

fn hash_string(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn result_placeholder() -> &'static str {
    "__result__"
}

fn result_count_placeholder() -> u64 {
    0
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

fn value_contains_secret(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| secret_like(key) || value_contains_secret(value)),
        Value::Array(values) => values.iter().any(value_contains_secret),
        Value::String(value) => secret_like(value),
        _ => false,
    }
}
