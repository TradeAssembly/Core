use super::{journal_view, ServiceResponse, TradeAssemblyService};
use serde_json::{json, Map, Value};

pub(crate) fn journal_replay(service: &TradeAssemblyService) -> ServiceResponse {
    let events = match journal_view::events(service) {
        Ok(events) => events,
        Err(error) => return error,
    };
    let mut counts = Map::new();
    for event in &events {
        let kind = event["event_type"]
            .as_str()
            .expect("validated journal type");
        let count = counts.get(kind).and_then(Value::as_u64).unwrap_or(0) + 1;
        counts.insert(kind.to_string(), json!(count));
    }
    ServiceResponse::ok(json!({"events": events.len(), "counts": counts,
        "mode": "stored_journal_inspection", "scope": "authenticated_owner",
        "legacyProjection": "owned_strategy_events_only",
        "strategyEvaluationPerformed": false, "deterministic": null}))
}

pub(crate) fn journal_replay_harness(
    service: &TradeAssemblyService,
    body: Value,
) -> ServiceResponse {
    let Some(expected) = body
        .get("expected")
        .and_then(|value| value.get("counts"))
        .and_then(Value::as_object)
        .filter(|counts| counts.values().all(|count| count.as_u64().is_some()))
    else {
        return ServiceResponse::error(400, "journal_expected_counts_required", false);
    };
    let mut response = journal_replay(service);
    if response.status != 200 {
        return response;
    }
    let matches = response.body["counts"] == json!(expected);
    response.status = if matches { 200 } else { 409 };
    response.body["ok"] = json!(matches);
    response.body["expected"] = json!({"counts": expected});
    response.body["comparison"] = json!("exact_event_type_counts");
    if !matches {
        response.body["error"] = json!({"code": "journal_count_mismatch"});
    }
    response
}
