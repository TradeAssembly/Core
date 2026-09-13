// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{positions, strategy_id_from, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::lifecycle_calendar::{
    lifecycle_calendar_event_hash, required_lifecycle_calendar_event_types,
    LifecycleCalendarAffectedRef, LifecycleCalendarEventContract, LifecycleCalendarEventType,
    LifecycleCalendarReplayRef, LifecycleCalendarSourceContract, LifecycleCalendarSourceKind,
    LifecycleCalendarSourceProvenance, LifecycleCalendarWarning, LifecycleCalendarWarningCode,
    LifecycleCalendarWarningSeverity, LifecycleCalendarWindow, LIFECYCLE_CALENDAR_NOTICE,
    LIFECYCLE_CALENDAR_SCHEMA_VERSION,
};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const CALENDAR_NS: &str = "lifecycle_calendar_timelines";
const DEFAULT_NOW_UNIX_MS: u64 = 1_785_000_000_000;

pub(crate) fn build_timeline(service: &TradeAssemblyService, request: Value) -> Value {
    let scope = scope_from(&request);
    let strategy_id = strategy_id_from(&request);
    let now_unix_ms = u64_field(&request, &["nowUnixMs", "now_unix_ms", "asOfUnixMs"])
        .unwrap_or(DEFAULT_NOW_UNIX_MS);
    let studio_base_url = string_field(&request, &["studioBaseUrl", "studio_base_url"])
        .unwrap_or_else(|| "http://127.0.0.1:3001".to_string());
    let mut events = collect_events(service, &request, &strategy_id, now_unix_ms);
    let mut service_warnings = service_warnings(&request, &events);
    let events = dedupe_events(&mut events);
    let (upcoming, past) = split_events(events, now_unix_ms);
    let affected_positions = affected_positions(&upcoming, &past);
    let validation_warning_count = upcoming
        .iter()
        .chain(past.iter())
        .map(|event| event.warnings.len())
        .sum::<usize>();
    let error_count = upcoming
        .iter()
        .chain(past.iter())
        .flat_map(|event| event.warnings.iter())
        .filter(|warning| warning.severity == LifecycleCalendarWarningSeverity::Error)
        .count()
        + service_warnings
            .iter()
            .filter(|warning| warning["severity"] == "error")
            .count();
    service_warnings.sort_by_key(|warning| warning["code"].as_str().unwrap_or("").to_string());
    let timeline_id = timeline_id(&scope, &upcoming, &past);
    let export_refs = json!({
        "json": {"uri": format!("tradeassembly://artifact/lifecycle-calendar/{timeline_id}/timeline.json")},
        "csv": {"uri": format!("tradeassembly://artifact/lifecycle-calendar/{timeline_id}/events.csv")},
        "ics": {"uri": format!("tradeassembly://artifact/lifecycle-calendar/{timeline_id}/calendar.ics")},
    });
    let replay_refs = json!({
        "commands": [
            format!("tradeassembly lifecycle-calendar build --timeline-id {timeline_id} --db {}", service.db()),
            format!("tradeassembly lifecycle-calendar replay --timeline-id {timeline_id} --db {}", service.db())
        ],
        "refs": upcoming.iter()
            .chain(past.iter())
            .flat_map(|event| event.replay_refs.clone())
            .map(|replay| json!({"replayRef": replay.replay_ref, "deterministic": replay.deterministic}))
            .collect::<Vec<_>>()
    });
    let mut result = json!({
        "schemaVersion": "tradeassembly.lifecycle_calendar.service.v1",
        "timelineId": timeline_id,
        "scope": scope,
        "strategyId": strategy_id,
        "asOfUnixMs": now_unix_ms,
        "ok": error_count == 0,
        "summary": {
            "upcomingCount": upcoming.len(),
            "pastReplayRelevantCount": past.len(),
            "affectedPositionCount": affected_positions.len(),
            "warningCount": validation_warning_count + service_warnings.len(),
            "errorCount": error_count,
        },
        "upcomingEvents": upcoming,
        "pastEvents": past,
        "pastReplayRelevantEvents": past,
        "affectedPositions": affected_positions,
        "warnings": service_warnings,
        "exportRefs": export_refs,
        "replayRefs": replay_refs,
        "deepLinks": {
            "strategy": {"href": format!("{}/app/strategies/{strategy_id}", studio_base_url.trim_end_matches('/'))},
            "calendar": {"href": format!("{}/app/strategies/{strategy_id}/execution?lifecycleCalendar={timeline_id}", studio_base_url.trim_end_matches('/'))}
        },
        "sideEffects": {
            "brokerStateChanged": false,
            "strategyLogicChanged": false,
            "credentialsTouched": false,
            "exerciseDecisionChanged": false,
            "activationChanged": false,
        },
        "agentSummary": {
            "scope": "Local lifecycle calendar evidence",
            "results": {
                "upcomingEvents": result_count_placeholder(),
                "pastReplayRelevantEvents": result_count_placeholder(),
                "warnings": result_count_placeholder(),
            },
            "studioDeepLinks": {
                "calendar": format!("{}/app/strategies/{strategy_id}/execution?lifecycleCalendar={timeline_id}", studio_base_url.trim_end_matches('/'))
            }
        },
        "noAdvice": LEGAL_BOUNDARY,
        "noAdviceNotice": LIFECYCLE_CALENDAR_NOTICE,
    });
    result["agentSummary"]["results"]["upcomingEvents"] =
        result["summary"]["upcomingCount"].clone();
    result["agentSummary"]["results"]["pastReplayRelevantEvents"] =
        result["summary"]["pastReplayRelevantCount"].clone();
    result["agentSummary"]["results"]["warnings"] = result["summary"]["warningCount"].clone();

    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!(
            "lifecycle_calendar.timeline:{}",
            result["timelineId"].as_str().unwrap_or("latest")
        ))
        .expect("valid lifecycle calendar idempotency key"),
    );
    let timeline_id = result["timelineId"]
        .as_str()
        .unwrap_or("lifecycle_calendar_latest")
        .to_string();
    service
        .runtime()
        .storage
        .put_json(CALENDAR_NS, &timeline_id, result.clone(), &context)
        .expect("persist lifecycle calendar timeline");
    let journal_id = service
        .runtime()
        .record_side_effect(
            "lifecycle_calendar.timeline.completed",
            result.clone(),
            &context,
        )
        .expect("record lifecycle calendar journal evidence");
    result["journalEvidence"] = json!({
        "eventType": "lifecycle_calendar.timeline.completed",
        "journalId": journal_id,
        "storageNamespace": CALENDAR_NS,
        "timelineId": timeline_id,
    });
    service
        .runtime()
        .storage
        .put_json(CALENDAR_NS, &timeline_id, result.clone(), &context)
        .expect("persist lifecycle calendar timeline with journal evidence");
    result
}

pub(crate) fn latest_timeline(service: &TradeAssemblyService) -> Option<Value> {
    service
        .runtime()
        .storage
        .list_json(CALENDAR_NS)
        .ok()
        .and_then(|mut values| values.pop().map(|(_, value)| value))
}

fn collect_events(
    service: &TradeAssemblyService,
    request: &Value,
    strategy_id: &str,
    now_unix_ms: u64,
) -> Vec<LifecycleCalendarEventContract> {
    let mut events = Vec::new();
    collect_event_arrays(request, &mut events, strategy_id, now_unix_ms, "request");
    for pack in array_field(request, &["instrumentPacks", "instrument_packs", "packs"]) {
        collect_event_arrays(
            &pack,
            &mut events,
            strategy_id,
            now_unix_ms,
            "instrument_pack",
        );
    }
    for provider in array_field(
        request,
        &["providerMetadata", "provider_metadata", "providers"],
    ) {
        collect_event_arrays(
            &provider,
            &mut events,
            strategy_id,
            now_unix_ms,
            "provider_metadata",
        );
        collect_typed_provider_events(&provider, &mut events, strategy_id, now_unix_ms);
    }
    for position in ledger_positions(service, request) {
        collect_position_events(&position, &mut events, strategy_id, now_unix_ms);
    }
    for reminder in array_field(request, &["userReminders", "user_reminders", "reminders"]) {
        events.push(loose_event(
            &reminder,
            LifecycleCalendarEventType::UserReminder,
            strategy_id,
            now_unix_ms,
            "user_reminder",
        ));
    }
    events
}

fn collect_event_arrays(
    value: &Value,
    events: &mut Vec<LifecycleCalendarEventContract>,
    strategy_id: &str,
    now_unix_ms: u64,
    source_hint: &str,
) {
    for key in [
        "events",
        "lifecycleEvents",
        "lifecycle_events",
        "calendarEvents",
    ] {
        for event in value
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            events.push(event_from_value(
                event,
                strategy_id,
                now_unix_ms,
                source_hint,
            ));
        }
    }
}

fn collect_typed_provider_events(
    provider: &Value,
    events: &mut Vec<LifecycleCalendarEventContract>,
    strategy_id: &str,
    now_unix_ms: u64,
) {
    for (key, event_type) in [
        ("earnings", LifecycleCalendarEventType::EarningsDate),
        ("dividends", LifecycleCalendarEventType::ExDividendDate),
        (
            "corporateActions",
            LifecycleCalendarEventType::CorporateAction,
        ),
        (
            "corporate_actions",
            LifecycleCalendarEventType::CorporateAction,
        ),
    ] {
        for item in provider
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            events.push(loose_event(
                item,
                event_type.clone(),
                strategy_id,
                now_unix_ms,
                "provider_metadata",
            ));
        }
    }
}

fn collect_position_events(
    position: &Value,
    events: &mut Vec<LifecycleCalendarEventContract>,
    strategy_id: &str,
    now_unix_ms: u64,
) {
    for (keys, event_type) in [
        (
            &["expirationUnixMs", "expiration_unix_ms", "expiresAtUnixMs"][..],
            LifecycleCalendarEventType::Expiration,
        ),
        (
            &[
                "lastTradeUnixMs",
                "last_trade_unix_ms",
                "lastTradeDateUnixMs",
            ][..],
            LifecycleCalendarEventType::LastTradeDate,
        ),
        (
            &[
                "exDividendUnixMs",
                "ex_dividend_unix_ms",
                "exDividendDateUnixMs",
            ][..],
            LifecycleCalendarEventType::ExDividendDate,
        ),
        (
            &["earningsUnixMs", "earnings_unix_ms", "earningsDateUnixMs"][..],
            LifecycleCalendarEventType::EarningsDate,
        ),
        (
            &["corporateActionUnixMs", "corporate_action_unix_ms"][..],
            LifecycleCalendarEventType::CorporateAction,
        ),
        (
            &["marginEventUnixMs", "margin_event_unix_ms"][..],
            LifecycleCalendarEventType::MarginEvent,
        ),
    ] {
        if u64_field(position, keys).is_some() {
            let mut value = position.clone();
            value["eventType"] = json!(event_type_name(&event_type));
            events.push(loose_event(
                &value,
                event_type,
                strategy_id,
                now_unix_ms,
                "local_ledger",
            ));
        }
    }
}

fn event_from_value(
    value: &Value,
    strategy_id: &str,
    now_unix_ms: u64,
    source_hint: &str,
) -> LifecycleCalendarEventContract {
    if let Ok(mut event) = serde_json::from_value::<LifecycleCalendarEventContract>(value.clone()) {
        normalize_event(&mut event, strategy_id, now_unix_ms);
        return event;
    }
    let event_type = event_type_from(value).unwrap_or(LifecycleCalendarEventType::Expiration);
    loose_event(value, event_type, strategy_id, now_unix_ms, source_hint)
}

fn loose_event(
    value: &Value,
    event_type: LifecycleCalendarEventType,
    strategy_id: &str,
    now_unix_ms: u64,
    source_hint: &str,
) -> LifecycleCalendarEventContract {
    let event_at = event_timestamp(value, &event_type).unwrap_or(now_unix_ms);
    let starts_at = u64_field(value, &["startsAtUnixMs", "starts_at_unix_ms"]).unwrap_or(event_at);
    let ends_at = u64_field(value, &["endsAtUnixMs", "ends_at_unix_ms"]).unwrap_or(event_at);
    let instrument_id = string_field(
        value,
        &["instrumentId", "instrument_id", "canonicalInstrumentId"],
    )
    .or_else(|| string_field(value, &["symbol"]).map(|symbol| format!("symbol:{symbol}")))
    .unwrap_or_else(|| "unsupported:missing-instrument".to_string());
    let symbol = string_field(value, &["symbol", "underlyingSymbol", "underlying_symbol"])
        .unwrap_or_else(|| instrument_id.clone());
    let position_id = string_field(value, &["positionId", "position_id"]);
    let mut event = LifecycleCalendarEventContract {
        schema_version: LIFECYCLE_CALENDAR_SCHEMA_VERSION.to_string(),
        event_id: string_field(value, &["eventId", "event_id", "id"]).unwrap_or_else(|| {
            stable_id(
                "lc_evt",
                &[
                    event_type_name(&event_type),
                    &instrument_id,
                    &starts_at.to_string(),
                ],
            )
        }),
        event_type: event_type.clone(),
        affected: vec![LifecycleCalendarAffectedRef {
            instrument_id,
            symbol,
            asset_class: string_field(value, &["assetClass", "asset_class"])
                .unwrap_or_else(|| "option".to_string()),
            position_id,
            strategy_id: Some(
                string_field(value, &["strategyId", "strategy_id"])
                    .unwrap_or_else(|| strategy_id.to_string()),
            ),
            account_scope_ref: string_field(value, &["accountScopeRef", "account_scope_ref"]),
        }],
        window: LifecycleCalendarWindow {
            starts_at_unix_ms: starts_at,
            ends_at_unix_ms: ends_at,
            timezone: string_field(value, &["timezone"])
                .unwrap_or_else(|| "America/New_York".to_string()),
        },
        as_of_unix_ms: u64_field(value, &["asOfUnixMs", "as_of_unix_ms"]).unwrap_or(now_unix_ms),
        stale_after_unix_ms: u64_field(value, &["staleAfterUnixMs", "stale_after_unix_ms"]),
        confidence: f64_field(value, &["confidence"]).unwrap_or_else(|| {
            if event_type == LifecycleCalendarEventType::UserReminder {
                1.0
            } else {
                0.8
            }
        }),
        source: source_from(value, source_hint),
        unsupported_reason: string_field(value, &["unsupportedReason", "unsupported_reason"]),
        warnings: vec![],
        deep_links: BTreeMap::from([(
            "studio".to_string(),
            format!("/app/strategies/{strategy_id}/execution?calendarEvent=pending"),
        )]),
        replay_refs: vec![],
        metadata: BTreeMap::new(),
        no_advice_notice: LIFECYCLE_CALENDAR_NOTICE.to_string(),
    };
    normalize_event(&mut event, strategy_id, now_unix_ms);
    event
}

fn normalize_event(
    event: &mut LifecycleCalendarEventContract,
    strategy_id: &str,
    now_unix_ms: u64,
) {
    if event.event_id.trim().is_empty() {
        let first = event
            .affected
            .first()
            .map(|affected| affected.instrument_id.as_str())
            .unwrap_or("missing");
        event.event_id = stable_id(
            "lc_evt",
            &[
                event_type_name(&event.event_type),
                first,
                &event.window.starts_at_unix_ms.to_string(),
            ],
        );
    }
    for affected in &mut event.affected {
        if affected.strategy_id.is_none() {
            affected.strategy_id = Some(strategy_id.to_string());
        }
    }
    event
        .deep_links
        .entry("studio".to_string())
        .or_insert_with(|| {
            format!(
                "/app/strategies/{strategy_id}/execution?calendarEvent={}",
                event.event_id
            )
        });
    if event.replay_refs.is_empty() {
        event.replay_refs.push(LifecycleCalendarReplayRef {
            replay_ref: format!(
                "tradeassembly://lifecycle-calendar/{}/replay",
                event.event_id
            ),
            deterministic: true,
        });
    }
    if event.source.supported_event_types.is_empty() {
        event.source.supported_event_types = required_lifecycle_calendar_event_types()
            .into_iter()
            .collect();
    }
    if event.as_of_unix_ms == 0 {
        event.as_of_unix_ms = now_unix_ms;
    }
    let report = event.validate();
    event.warnings = report.warnings;
}

fn dedupe_events(
    events: &mut [LifecycleCalendarEventContract],
) -> Vec<LifecycleCalendarEventContract> {
    let mut deduped = BTreeMap::<String, LifecycleCalendarEventContract>::new();
    for event in events.iter().cloned() {
        let key = dedupe_key(&event);
        if let Some(existing) = deduped.get_mut(&key) {
            merge_duplicate(existing, &event);
        } else {
            deduped.insert(key, event);
        }
    }
    let mut values = deduped.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.window
            .starts_at_unix_ms
            .cmp(&right.window.starts_at_unix_ms)
            .then(left.event_id.cmp(&right.event_id))
    });
    values
}

fn merge_duplicate(
    existing: &mut LifecycleCalendarEventContract,
    duplicate: &LifecycleCalendarEventContract,
) {
    let mut seen = existing
        .warnings
        .iter()
        .map(|warning| {
            format!(
                "{:?}:{:?}:{}",
                warning.code, warning.severity, warning.message
            )
        })
        .collect::<BTreeSet<_>>();
    for warning in &duplicate.warnings {
        let key = format!(
            "{:?}:{:?}:{}",
            warning.code, warning.severity, warning.message
        );
        if seen.insert(key) {
            existing.warnings.push(warning.clone());
        }
    }
    existing.confidence = existing.confidence.min(duplicate.confidence);
    existing.metadata.insert(
        "duplicateProvenance".to_string(),
        json!({
            "source": duplicate.source.provenance.source,
            "sourceRef": duplicate.source.provenance.source_ref,
            "confidence": duplicate.confidence,
        }),
    );
}

fn split_events(
    events: Vec<LifecycleCalendarEventContract>,
    now_unix_ms: u64,
) -> (
    Vec<LifecycleCalendarEventContract>,
    Vec<LifecycleCalendarEventContract>,
) {
    let (past, upcoming): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|event| event.window.ends_at_unix_ms < now_unix_ms);
    (upcoming, past)
}

fn affected_positions(
    upcoming: &[LifecycleCalendarEventContract],
    past: &[LifecycleCalendarEventContract],
) -> Vec<Value> {
    let mut positions = BTreeMap::new();
    for event in upcoming.iter().chain(past.iter()) {
        for affected in &event.affected {
            if let Some(position_id) = affected.position_id.as_ref() {
                positions.insert(
                    position_id.clone(),
                    json!({
                        "positionId": position_id,
                        "instrumentId": affected.instrument_id,
                        "symbol": affected.symbol,
                        "strategyId": affected.strategy_id,
                    }),
                );
            }
        }
    }
    positions.into_values().collect()
}

fn service_warnings(request: &Value, events: &[LifecycleCalendarEventContract]) -> Vec<Value> {
    let mut warnings = Vec::new();
    if events.is_empty() {
        warnings.push(service_warning(
            LifecycleCalendarWarningCode::MissingSourceData,
            LifecycleCalendarWarningSeverity::Error,
            "No local lifecycle calendar source data was available.",
        ));
    }
    if contains_secret_shape(request) {
        warnings.push(service_warning(
            LifecycleCalendarWarningCode::RawSecretRejected,
            LifecycleCalendarWarningSeverity::Error,
            "Lifecycle calendar input contained secret-shaped fields; raw values were not echoed.",
        ));
    }
    warnings
}

fn service_warning(
    code: LifecycleCalendarWarningCode,
    severity: LifecycleCalendarWarningSeverity,
    message: &str,
) -> Value {
    serde_json::to_value(LifecycleCalendarWarning {
        code,
        severity,
        message: message.to_string(),
        replay_ref: "tradeassembly://lifecycle-calendar/latest/replay".to_string(),
    })
    .expect("service warning serializes")
}

fn ledger_positions(service: &TradeAssemblyService, request: &Value) -> Vec<Value> {
    request
        .get("ledgerState")
        .or_else(|| request.get("ledger_state"))
        .and_then(|ledger| ledger.get("positions"))
        .or_else(|| request.get("positions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| positions::list(service))
}

fn source_from(value: &Value, source_hint: &str) -> LifecycleCalendarSourceContract {
    if let Some(source) = value.get("source") {
        if let Ok(source) =
            serde_json::from_value::<LifecycleCalendarSourceContract>(source.clone())
        {
            return source;
        }
    }
    let source_id = string_field(
        value,
        &["sourceId", "source_id", "providerRef", "provider_ref"],
    )
    .unwrap_or_else(|| source_hint.to_string());
    let source_kind = if source_hint == "user_reminder" {
        LifecycleCalendarSourceKind::UserReminder
    } else if source_hint == "local_ledger" {
        LifecycleCalendarSourceKind::LocalDerived
    } else {
        LifecycleCalendarSourceKind::ProviderAdapter
    };
    LifecycleCalendarSourceContract {
        source_id: source_id.clone(),
        source_kind,
        adapter_ref: string_field(value, &["adapterRef", "adapter_ref"])
            .unwrap_or_else(|| format!("adapter://local/{source_hint}")),
        provider_ref: string_field(value, &["providerRef", "provider_ref"]),
        local_first: bool_field(value, &["localFirst", "local_first"]).unwrap_or(true),
        hosted_integration: bool_field(value, &["hostedIntegration", "hosted_integration"])
            .unwrap_or(false),
        account_specific: bool_field(value, &["accountSpecific", "account_specific"])
            .unwrap_or(source_hint == "local_ledger"),
        supported_event_types: required_lifecycle_calendar_event_types()
            .into_iter()
            .collect(),
        provenance: LifecycleCalendarSourceProvenance {
            source: source_id,
            source_ref: string_field(value, &["sourceRef", "source_ref"])
                .unwrap_or_else(|| format!("local://{source_hint}")),
            collected_at_unix_ms: u64_field(value, &["collectedAtUnixMs", "collected_at_unix_ms"])
                .unwrap_or(DEFAULT_NOW_UNIX_MS),
            raw_secrets_exposed: false,
        },
    }
}

fn event_type_from(value: &Value) -> Option<LifecycleCalendarEventType> {
    let raw = string_field(value, &["eventType", "event_type", "type"])?;
    Some(match raw.as_str() {
        "expiration" => LifecycleCalendarEventType::Expiration,
        "last_trade_date" | "lastTradeDate" => LifecycleCalendarEventType::LastTradeDate,
        "exercise_window" | "exerciseWindow" => LifecycleCalendarEventType::ExerciseWindow,
        "assignment_window" | "assignmentWindow" => LifecycleCalendarEventType::AssignmentWindow,
        "ex_dividend_date" | "exDividendDate" | "dividend" => {
            LifecycleCalendarEventType::ExDividendDate
        }
        "earnings_date" | "earningsDate" | "earnings" => LifecycleCalendarEventType::EarningsDate,
        "corporate_action" | "corporateAction" => LifecycleCalendarEventType::CorporateAction,
        "margin_event" | "marginEvent" => LifecycleCalendarEventType::MarginEvent,
        "user_reminder" | "userReminder" | "reminder" => LifecycleCalendarEventType::UserReminder,
        _ => LifecycleCalendarEventType::Expiration,
    })
}

fn event_timestamp(value: &Value, event_type: &LifecycleCalendarEventType) -> Option<u64> {
    let generic = u64_field(
        value,
        &[
            "atUnixMs",
            "at_unix_ms",
            "timestampUnixMs",
            "timestamp_unix_ms",
            "eventUnixMs",
        ],
    );
    generic.or_else(|| match event_type {
        LifecycleCalendarEventType::Expiration => u64_field(
            value,
            &["expirationUnixMs", "expiration_unix_ms", "expiresAtUnixMs"],
        ),
        LifecycleCalendarEventType::LastTradeDate => u64_field(
            value,
            &[
                "lastTradeUnixMs",
                "last_trade_unix_ms",
                "lastTradeDateUnixMs",
            ],
        ),
        LifecycleCalendarEventType::ExDividendDate => u64_field(
            value,
            &[
                "exDividendUnixMs",
                "ex_dividend_unix_ms",
                "exDividendDateUnixMs",
            ],
        ),
        LifecycleCalendarEventType::EarningsDate => u64_field(
            value,
            &["earningsUnixMs", "earnings_unix_ms", "earningsDateUnixMs"],
        ),
        LifecycleCalendarEventType::CorporateAction => u64_field(
            value,
            &["corporateActionUnixMs", "corporate_action_unix_ms"],
        ),
        LifecycleCalendarEventType::MarginEvent => {
            u64_field(value, &["marginEventUnixMs", "margin_event_unix_ms"])
        }
        _ => None,
    })
}

fn scope_from(request: &Value) -> Value {
    json!({
        "kind": string_field(request, &["scopeKind", "scope_kind"]).unwrap_or_else(|| "strategy".to_string()),
        "id": string_field(request, &["scopeId", "scope_id", "strategyId", "strategy_id"]).unwrap_or_else(|| "strat_local_btc_demo".to_string()),
    })
}

fn timeline_id(
    scope: &Value,
    upcoming: &[LifecycleCalendarEventContract],
    past: &[LifecycleCalendarEventContract],
) -> String {
    let event_hashes = upcoming
        .iter()
        .chain(past.iter())
        .map(lifecycle_calendar_event_hash)
        .collect::<Vec<_>>();
    stable_id(
        "lifecycle_calendar",
        &[
            scope["kind"].as_str().unwrap_or("strategy"),
            scope["id"].as_str().unwrap_or("local"),
            &event_hashes.join("|"),
        ],
    )
}

fn dedupe_key(event: &LifecycleCalendarEventContract) -> String {
    let affected = event.affected.first();
    format!(
        "{}|{}|{}|{}|{}|{}",
        event_type_name(&event.event_type),
        event.source.source_id,
        event.window.starts_at_unix_ms,
        event.window.ends_at_unix_ms,
        affected
            .map(|affected| affected.instrument_id.as_str())
            .unwrap_or("missing"),
        affected
            .and_then(|affected| affected.position_id.as_deref())
            .unwrap_or("none")
    )
}

fn event_type_name(event_type: &LifecycleCalendarEventType) -> &'static str {
    match event_type {
        LifecycleCalendarEventType::Expiration => "expiration",
        LifecycleCalendarEventType::LastTradeDate => "last_trade_date",
        LifecycleCalendarEventType::ExerciseWindow => "exercise_window",
        LifecycleCalendarEventType::AssignmentWindow => "assignment_window",
        LifecycleCalendarEventType::ExDividendDate => "ex_dividend_date",
        LifecycleCalendarEventType::EarningsDate => "earnings_date",
        LifecycleCalendarEventType::CorporateAction => "corporate_action",
        LifecycleCalendarEventType::MarginEvent => "margin_event",
        LifecycleCalendarEventType::UserReminder => "user_reminder",
    }
}

fn array_field(value: &Value, keys: &[&str]) -> Vec<Value> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn u64_field(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_u64)
            .or_else(|| value.get(*key).and_then(Value::as_str)?.parse::<u64>().ok())
    })
}

fn f64_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_f64)
            .or_else(|| value.get(*key).and_then(Value::as_str)?.parse::<f64>().ok())
    })
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"|");
    }
    format!("{prefix}_{:x}", hasher.finalize())[..prefix.len() + 17].to_string()
}

fn contains_secret_shape(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| secret_key(key) || contains_secret_shape(value)),
        Value::Array(values) => values.iter().any(contains_secret_shape),
        _ => false,
    }
}

fn secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    normalized.contains("api_key")
        || normalized.contains("api_secret")
        || normalized.contains("secret")
        || normalized.contains("password")
        || normalized.contains("token")
}

fn result_count_placeholder() -> Value {
    json!(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_calendar_service_builds_strategy_timeline_from_all_local_sources() {
        let service = TradeAssemblyService::test_local(test_database("timeline"));
        let result = service.lifecycle_calendar(sample_request());

        assert_eq!(
            result["schemaVersion"],
            "tradeassembly.lifecycle_calendar.service.v1"
        );
        assert!(result["ok"].as_bool().unwrap());
        assert_eq!(result["summary"]["upcomingCount"], 5);
        assert_eq!(result["summary"]["pastReplayRelevantCount"], 1);
        assert_eq!(result["summary"]["affectedPositionCount"], 1);
        assert_eq!(
            result["sideEffects"]["brokerStateChanged"],
            json!(false),
            "calendar service must not touch broker state"
        );
        assert!(result["exportRefs"]["ics"]["uri"]
            .as_str()
            .unwrap()
            .contains("lifecycle-calendar"));
        assert!(result["replayRefs"]["commands"][0]
            .as_str()
            .unwrap()
            .contains("lifecycle-calendar build"));
        assert_eq!(
            result["journalEvidence"]["eventType"],
            "lifecycle_calendar.timeline.completed"
        );
        assert!(service.latest_lifecycle_calendar().is_some());
    }

    #[test]
    fn lifecycle_calendar_service_dedupes_events_and_preserves_lowest_confidence() {
        let service = TradeAssemblyService::test_local(test_database("dedupe"));
        let request = json!({
            "strategyId": "strat_local_btc_demo",
            "nowUnixMs": 1_785_000_000_000u64,
            "providerMetadata": [{
                "events": [
            event("expiration", 1_785_801_600_000, 0.91),
            event("expiration", 1_785_801_600_000, 0.62)
                ]
            }]
        });

        let result = service.lifecycle_calendar(request);
        let expirations = result["upcomingEvents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["eventType"] == "expiration")
            .collect::<Vec<_>>();

        assert_eq!(expirations.len(), 1);
        assert_eq!(expirations[0]["confidence"], json!(0.62));
        assert!(expirations[0]["metadata"]["duplicateProvenance"].is_object());
    }

    #[test]
    fn lifecycle_calendar_service_fails_closed_for_stale_sources_without_secret_echo() {
        let service = TradeAssemblyService::test_local(test_database("stale"));
        let result = service.lifecycle_calendar(json!({
            "strategyId": "strat_local_btc_demo",
            "nowUnixMs": 1_785_000_000_000u64,
            "providerMetadata": [{
                "api_secret": "do-not-echo",
                "events": [{
                    "eventType": "earnings_date",
                    "instrumentId": "eq:SPY",
                    "symbol": "SPY",
                    "atUnixMs": 1_785_801_600_000u64,
                    "staleAfterUnixMs": 1_000u64,
                    "confidence": 0.7
                }]
            }]
        }));
        let serialized = serde_json::to_string(&result).unwrap();

        assert!(!result["ok"].as_bool().unwrap());
        assert!(serialized.contains("stale_source_data"));
        assert!(serialized.contains("raw_secret_rejected"));
        assert!(!serialized.contains("do-not-echo"));
    }

    #[test]
    fn lifecycle_calendar_service_output_contains_no_action_advice() {
        let service = TradeAssemblyService::test_local(test_database("copy"));
        let result = service.lifecycle_calendar(sample_request());
        let serialized = serde_json::to_string(&result).unwrap().to_lowercase();

        assert!(serialized.contains("tradeassembly never tells users what to trade"));
        assert!(!serialized.contains("should exercise"));
        assert!(!serialized.contains("should roll"));
        assert!(!serialized.contains("should close"));
    }

    fn test_database(label: &str) -> String {
        std::env::temp_dir()
            .join(format!(
                "tradeassembly-lifecycle-calendar-{label}-{}.db",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned()
    }

    fn sample_request() -> Value {
        json!({
            "strategyId": "strat_local_btc_demo",
            "scopeKind": "strategy",
            "nowUnixMs": 1_785_000_000_000u64,
            "ledgerState": {
                "positions": [{
                    "id": "position-spy-call",
                    "strategy_id": "strat_local_btc_demo",
                    "instrumentId": "occ:SPY260717C00550000",
                    "symbol": "SPY",
                    "assetClass": "option",
                    "accountScopeRef": "acct_scope_redacted",
                    "expirationUnixMs": 1_785_801_600_000u64
                }]
            },
            "providerMetadata": [{
                "providerRef": "calendar-data",
                "earnings": [{
                    "instrumentId": "eq:SPY",
                    "symbol": "SPY",
                    "atUnixMs": 1_785_715_200_000u64,
                    "confidence": 0.82
                }],
                "dividends": [{
                    "instrumentId": "eq:SPY",
                    "symbol": "SPY",
                    "atUnixMs": 1_785_628_800_000u64,
                    "confidence": 0.78
                }],
                "corporateActions": [{
                    "instrumentId": "eq:SPY",
                    "symbol": "SPY",
                    "atUnixMs": 1_784_900_000_000u64,
                    "confidence": 0.74
                }]
            }],
            "instrumentPacks": [{
                "events": [event("last_trade_date", 1_785_715_200_000u64, 0.95)]
            }],
            "userReminders": [{
                "id": "reminder-review-risk",
                "instrumentId": "occ:SPY260717C00550000",
                "symbol": "SPY",
                "atUnixMs": 1_785_196_800_000u64,
                "message": "Review position notes"
            }]
        })
    }

    fn event(event_type: &str, at: u64, confidence: f64) -> Value {
        json!({
            "eventType": event_type,
            "instrumentId": "occ:SPY260717C00550000",
            "symbol": "SPY",
            "positionId": "position-spy-call",
            "accountScopeRef": "acct_scope_redacted",
            "atUnixMs": at,
            "confidence": confidence
        })
    }
}
