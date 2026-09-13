// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::service::{
    fill_quality_body_from, lifecycle_calendar_body_from, research_notebook_body_from,
    TradeAssemblyService,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const NO_ADVICE: &str = "TradeAssembly provides deterministic machinery, local journaling, broker/data interfaces, and replay tools. It does not tell users what to trade, when to trade, or how much to trade.";
const POSITION_NO_ADVICE: &str = "Descriptive position lifecycle output only. TradeAssembly did not recommend a trade, position size, or activation decision.";

#[derive(Default)]
pub struct PositionLifecycleJournal {
    snapshots: Vec<Value>,
}

impl PositionLifecycleJournal {
    pub fn import(&mut self, ledger: &Value, studio_base_url: Option<&str>) -> Value {
        let payload = position_lifecycle_surface("import", ledger, studio_base_url, None, None);
        self.snapshots.push(payload.clone());
        payload
    }

    pub fn status(
        &self,
        strategy_id: Option<&str>,
        position_id: Option<&str>,
        studio_base_url: Option<&str>,
    ) -> Value {
        self.latest_or_empty("status", strategy_id, position_id, studio_base_url)
    }

    pub fn replay(
        &self,
        strategy_id: Option<&str>,
        position_id: Option<&str>,
        studio_base_url: Option<&str>,
    ) -> Value {
        self.latest_or_empty("replay", strategy_id, position_id, studio_base_url)
    }

    fn latest_or_empty(
        &self,
        operation: &str,
        strategy_id: Option<&str>,
        position_id: Option<&str>,
        studio_base_url: Option<&str>,
    ) -> Value {
        let latest = self.snapshots.iter().rev().find(|snapshot| {
            optional_matches(&snapshot["summary"]["strategyId"], strategy_id)
                && optional_matches(&snapshot["summary"]["positionId"], position_id)
        });
        match latest {
            Some(snapshot) => {
                let mut payload = snapshot.clone();
                payload["operation"] = json!(operation);
                payload["summary"]["snapshotCount"] = json!(self.snapshots.len());
                payload
            }
            None => empty_position_lifecycle_surface(
                operation,
                strategy_id,
                position_id,
                studio_base_url,
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn selector_surface(
    surface: &str,
    operation: &str,
    selector: &Value,
    snapshots: &Value,
    run_id: &str,
    snapshot_ref: &str,
    strategy_id: Option<&str>,
    studio_base_url: Option<&str>,
) -> Value {
    let snapshots = snapshots.as_array().cloned().unwrap_or_default();
    let min_delta = selector
        .get("min_delta")
        .and_then(Value::as_f64)
        .unwrap_or(f64::NEG_INFINITY);
    let max_delta = selector
        .get("max_delta")
        .and_then(Value::as_f64)
        .unwrap_or(f64::INFINITY);
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for snapshot in snapshots {
        let delta = snapshot.get("delta").and_then(Value::as_f64).unwrap_or(0.0);
        let mut candidate = snapshot.clone();
        if min_delta <= delta && delta <= max_delta {
            candidate["status"] = json!("accepted");
            candidate["reason_codes"] = json!([]);
            accepted.push(candidate);
        } else {
            candidate["status"] = json!("rejected");
            candidate["reason_codes"] = json!(["outside_delta"]);
            rejected.push(candidate);
        }
    }
    accepted.sort_by(|left, right| {
        let left_delta = left
            .get("delta")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .abs();
        let right_delta = right
            .get("delta")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .abs();
        right_delta
            .partial_cmp(&left_delta)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let base = studio_base_url.unwrap_or("http://127.0.0.1:3001");
    let strategy_id = strategy_id.unwrap_or("");
    json!({
        "schemaVersion": "tradeassembly.selector_surface.v1",
        "surface": surface,
        "operation": operation,
        "run_id": run_id,
        "snapshot_ref": snapshot_ref,
        "accepted_candidates": accepted,
        "rejected_candidates": rejected,
        "reasonCodeSummary": {"outside_delta": rejected.len()},
        "reportEnvelope": {
            "deepLinks": {
                "selector": {
                    "href": format!("{}/app/strategies/{}/research?selectorRunId={}", base.trim_end_matches('/'), strategy_id, run_id)
                }
            },
            "renderContract": {
                "agentVisible": ["artifactBundle", "artifactRefs", "exportRefs", "replayRefs", "raw"]
            },
            "noAdvice": NO_ADVICE
        },
        "noAdvice": NO_ADVICE
    })
}

pub fn fill_quality_surface_acp(service: &TradeAssemblyService, request: &Value) -> Value {
    service.fill_quality_analysis(fill_quality_body_from(request))
}

pub fn lifecycle_calendar_surface_acp(service: &TradeAssemblyService, request: &Value) -> Value {
    service.lifecycle_calendar(lifecycle_calendar_body_from(request))
}

pub fn research_notebook_surface_acp(service: &TradeAssemblyService, request: &Value) -> Value {
    let operation = request
        .get("operation")
        .or_else(|| request.get("action"))
        .and_then(Value::as_str)
        .unwrap_or("compose");
    service.research_notebook_operation(operation, research_notebook_body_from(request))
}

pub fn position_lifecycle_surface(
    operation: &str,
    ledger: &Value,
    studio_base_url: Option<&str>,
    strategy_id: Option<&str>,
    position_id: Option<&str>,
) -> Value {
    let strategy_id = strategy_id
        .map(str::to_string)
        .unwrap_or_else(|| string_field(ledger, "strategy_id", ""));
    let position_id = position_id
        .map(str::to_string)
        .unwrap_or_else(|| string_field(ledger, "position_id", ""));
    let events = ledger
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let transactions = ledger
        .get("transactions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let opened = signed_quantity(&transactions, true);
    let closed = signed_quantity(&transactions, false).abs();
    let status = if closed > 0.0 && closed < opened {
        "partially_closed"
    } else if closed >= opened && opened > 0.0 {
        "closed"
    } else {
        "open"
    };
    let checkpoint_hash =
        checkpoint_hash(&strategy_id, &position_id, events.len(), transactions.len());
    let snapshot_id = format!("position_lifecycle_snapshot_{checkpoint_hash}");
    json!({
        "schemaVersion": "tradeassembly.position_lifecycle_surface.v1",
        "operation": operation,
        "summary": {
            "strategyId": strategy_id,
            "positionId": position_id,
            "snapshotCount": 1
        },
        "lifecycle": {
            "snapshot_id": snapshot_id,
            "position_status": status,
            "ledger_checkpoint_hash": checkpoint_hash,
            "side_effects": {
                "broker_orders_created": 0,
                "credentials_changed": 0,
                "strategy_logic_changed": 0,
                "trading_activations_changed": 0
            }
        },
        "deepLinks": {
            "studio": {
                "href": position_href(&strategy_id, &position_id, studio_base_url)
            }
        },
        "exportRefs": {
            "json": {
                "path": format!("tradeassembly://positions/{position_id}/lifecycle.json")
            }
        },
        "replayRefs": {
            "checkpointHash": checkpoint_hash,
            "journalRefs": ledger.pointer("/replay_plan/journal_refs").cloned().unwrap_or_else(|| json!([]))
        },
        "timeline": events,
        "legs": ledger.get("legs").cloned().unwrap_or_else(|| json!([])),
        "noAdvice": NO_ADVICE
    })
}

pub fn position_lifecycle_reduce(contract: &Value) -> Value {
    let mut state = contract
        .get("legs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|leg| {
            let leg_id = string_field(leg, "leg_id", "");
            if leg_id.is_empty() {
                None
            } else {
                Some((leg_id, LegState::from_leg(leg)))
            }
        })
        .collect::<BTreeMap<_, _>>();
    let transactions = transactions_by_event(contract);
    let mut events = contract
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    events.sort_by(|left, right| {
        (
            string_field(left, "event_time", ""),
            string_field(left, "event_id", ""),
        )
            .cmp(&(
                string_field(right, "event_time", ""),
                string_field(right, "event_id", ""),
            ))
    });
    let mut warnings = Vec::new();
    let mut closed_history = Vec::new();
    for event in events {
        let event_id = string_field(&event, "event_id", "");
        let event_type = string_field(&event, "event_type", "");
        let event_transactions = transactions.get(&event_id).cloned().unwrap_or_default();
        match event_type.as_str() {
            "open" | "partial_fill" => apply_open(&event_transactions, &mut state),
            "close" => apply_close(
                &event,
                &event_transactions,
                &mut state,
                &mut warnings,
                &mut closed_history,
            ),
            "roll" => apply_roll(&event, &event_transactions, &mut state, &mut closed_history),
            "assignment" | "exercise" | "expiration" => {
                apply_terminal(&event, &mut state, &mut closed_history)
            }
            "adjustment" | "manual_correction" => apply_adjustment(&event, &mut state),
            _ => {}
        }
    }
    let open_quantity = state.values().map(|leg| leg.open_quantity).sum::<f64>();
    let closed_quantity = state.values().map(|leg| leg.closed_quantity).sum::<f64>();
    let realized = state
        .values()
        .map(|leg| leg.realized_pnl_input)
        .sum::<f64>();
    let gross_exposure = state
        .values()
        .map(|leg| (leg.open_quantity * leg.average_open_price).abs())
        .sum::<f64>();
    let position_status = position_status(&state, &warnings);
    let ok = warnings.is_empty();
    let strategy_id = string_field(contract, "strategy_id", "");
    let position_id = string_field(contract, "position_id", "");
    let checkpoint = checkpoint_hash(
        &strategy_id,
        &position_id,
        contract
            .get("events")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        contract
            .get("transactions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    );
    json!({
        "snapshot_id": format!("plc_{checkpoint}"),
        "strategy_id": strategy_id,
        "position_id": position_id,
        "position_status": position_status,
        "ledger_checkpoint_hash": checkpoint,
        "replay_ref": contract.pointer("/replay_plan/replay_ref").and_then(Value::as_str).unwrap_or(""),
        "replay_journal_refs": contract.pointer("/replay_plan/journal_refs").cloned().unwrap_or_else(|| json!([])),
        "leg_state": state
            .iter()
            .map(|(leg_id, leg)| (leg_id.clone(), leg.to_json()))
            .collect::<serde_json::Map<_, _>>(),
        "summary": {
            "open_quantity": decimal_string(open_quantity),
            "closed_quantity": decimal_string(closed_quantity),
            "realized_pnl_input": decimal_string(realized),
            "gross_exposure_input": decimal_string(gross_exposure)
        },
        "closed_history": closed_history,
        "warnings": warnings,
        "side_effects": {
            "broker_orders_created": 0,
            "credentials_changed": 0,
            "strategy_logic_changed": 0,
            "trading_activations_changed": 0
        },
        "ok": ok,
        "no_advice_notice": POSITION_NO_ADVICE
    })
}

fn empty_position_lifecycle_surface(
    operation: &str,
    strategy_id: Option<&str>,
    position_id: Option<&str>,
    studio_base_url: Option<&str>,
) -> Value {
    let strategy_id = strategy_id.unwrap_or("");
    let position_id = position_id.unwrap_or("");
    json!({
        "schemaVersion": "tradeassembly.position_lifecycle_surface.v1",
        "operation": operation,
        "summary": {"strategyId": strategy_id, "positionId": position_id, "snapshotCount": 0},
        "lifecycle": {"snapshot_id": Value::Null, "position_status": "missing", "ledger_checkpoint_hash": Value::Null},
        "deepLinks": {"studio": {"href": position_href(strategy_id, position_id, studio_base_url)}},
        "exportRefs": {"json": {"path": format!("tradeassembly://positions/{position_id}/lifecycle.json")}},
        "replayRefs": {"checkpointHash": Value::Null, "journalRefs": []},
        "timeline": [],
        "legs": [],
        "noAdvice": NO_ADVICE
    })
}

fn position_href(strategy_id: &str, position_id: &str, studio_base_url: Option<&str>) -> String {
    format!(
        "{}/app/strategies/{}/run?positionId={}",
        studio_base_url
            .unwrap_or("http://127.0.0.1:3001")
            .trim_end_matches('/'),
        strategy_id,
        position_id
    )
}

fn signed_quantity(transactions: &[Value], opening: bool) -> f64 {
    transactions
        .iter()
        .filter(|item| {
            item.get("event_id")
                .and_then(Value::as_str)
                .map(|event_id| event_id.contains(if opening { "open" } else { "close" }))
                .unwrap_or(false)
        })
        .map(|item| {
            item.get("quantity")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0)
        })
        .sum()
}

#[derive(Clone, Debug)]
struct LegState {
    leg_id: String,
    canonical_id: String,
    side: String,
    opened_quantity: f64,
    open_quantity: f64,
    closed_quantity: f64,
    rolled_quantity: f64,
    assigned_quantity: f64,
    exercised_quantity: f64,
    expired_quantity: f64,
    average_open_price: f64,
    realized_pnl_input: f64,
}

impl LegState {
    fn from_leg(leg: &Value) -> Self {
        Self {
            leg_id: string_field(leg, "leg_id", ""),
            canonical_id: leg
                .pointer("/instrument/canonical_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            side: string_field(leg, "side", "long"),
            opened_quantity: 0.0,
            open_quantity: 0.0,
            closed_quantity: 0.0,
            rolled_quantity: 0.0,
            assigned_quantity: 0.0,
            exercised_quantity: 0.0,
            expired_quantity: 0.0,
            average_open_price: 0.0,
            realized_pnl_input: 0.0,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "leg_id": self.leg_id,
            "canonical_id": self.canonical_id,
            "side": self.side,
            "opened_quantity": decimal_string(self.opened_quantity),
            "open_quantity": decimal_string(self.open_quantity),
            "closed_quantity": decimal_string(self.closed_quantity),
            "rolled_quantity": decimal_string(self.rolled_quantity),
            "assigned_quantity": decimal_string(self.assigned_quantity),
            "exercised_quantity": decimal_string(self.exercised_quantity),
            "expired_quantity": decimal_string(self.expired_quantity),
            "average_open_price": decimal_string(self.average_open_price),
            "realized_pnl_input": decimal_string(self.realized_pnl_input),
            "status": leg_status(self)
        })
    }
}

fn transactions_by_event(contract: &Value) -> BTreeMap<String, Vec<Value>> {
    let mut grouped = BTreeMap::<String, Vec<Value>>::new();
    for transaction in contract
        .get("transactions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        grouped
            .entry(string_field(transaction, "event_id", ""))
            .or_default()
            .push(transaction.clone());
    }
    grouped
}

fn apply_open(transactions: &[Value], state: &mut BTreeMap<String, LegState>) {
    for transaction in transactions {
        let leg_id = string_field(transaction, "leg_id", "");
        let Some(leg) = state.get_mut(&leg_id) else {
            continue;
        };
        let quantity = number_field(transaction, "quantity").abs();
        let price = number_field(transaction, "price");
        let total_cost = leg.average_open_price * leg.open_quantity + price * quantity;
        leg.opened_quantity += quantity;
        leg.open_quantity += quantity;
        if leg.open_quantity > 0.0 {
            leg.average_open_price = round6(total_cost / leg.open_quantity);
        }
    }
}

fn apply_close(
    event: &Value,
    transactions: &[Value],
    state: &mut BTreeMap<String, LegState>,
    warnings: &mut Vec<Value>,
    closed_history: &mut Vec<Value>,
) {
    let mut leg_ids = BTreeSet::new();
    for transaction in transactions {
        let leg_id = string_field(transaction, "leg_id", "");
        let Some(leg) = state.get_mut(&leg_id) else {
            continue;
        };
        let quantity = number_field(transaction, "quantity").abs();
        if quantity > leg.open_quantity {
            warnings.push(json!({
                "code": "overclose",
                "message": "Lifecycle reducer saw a close quantity greater than open quantity.",
                "severity": "error",
                "event_id": string_field(event, "event_id", ""),
                "leg_id": leg_id,
                "replay_ref": string_field(event, "replay_ref", "")
            }));
        }
        let close_quantity = quantity.min(leg.open_quantity);
        let price = number_field(transaction, "price");
        let side_sign = if leg.side == "long" { 1.0 } else { -1.0 };
        leg.realized_pnl_input += (price - leg.average_open_price) * close_quantity * side_sign;
        leg.open_quantity -= close_quantity;
        leg.closed_quantity += close_quantity;
        leg_ids.insert(leg.leg_id.clone());
    }
    push_history(event, leg_ids, closed_history);
}

fn apply_roll(
    event: &Value,
    transactions: &[Value],
    state: &mut BTreeMap<String, LegState>,
    closed_history: &mut Vec<Value>,
) {
    let close_leg_id = event
        .pointer("/payload/close_leg_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let quantity = event
        .pointer("/payload/quantity")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    let mut leg_ids = BTreeSet::new();
    if let Some(leg) = state.get_mut(&close_leg_id) {
        let roll_quantity = quantity.min(leg.open_quantity);
        leg.open_quantity -= roll_quantity;
        leg.rolled_quantity += roll_quantity;
        leg_ids.insert(leg.leg_id.clone());
    }
    apply_open(transactions, state);
    for transaction in transactions {
        leg_ids.insert(string_field(transaction, "leg_id", ""));
    }
    push_history(event, leg_ids, closed_history);
}

fn apply_terminal(
    event: &Value,
    state: &mut BTreeMap<String, LegState>,
    closed_history: &mut Vec<Value>,
) {
    let event_type = string_field(event, "event_type", "");
    let mut leg_ids = BTreeSet::new();
    let leg_quantities = event
        .pointer("/payload/leg_quantities")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (leg_id, raw_quantity) in leg_quantities {
        let Some(leg) = state.get_mut(&leg_id) else {
            continue;
        };
        let quantity = value_number(&raw_quantity).min(leg.open_quantity);
        leg.open_quantity -= quantity;
        match event_type.as_str() {
            "assignment" => leg.assigned_quantity += quantity,
            "exercise" => leg.exercised_quantity += quantity,
            "expiration" => leg.expired_quantity += quantity,
            _ => {}
        }
        leg_ids.insert(leg.leg_id.clone());
    }
    push_history(event, leg_ids, closed_history);
}

fn apply_adjustment(event: &Value, state: &mut BTreeMap<String, LegState>) {
    let delta = event
        .pointer("/payload/realized_pnl_delta")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    let leg_id = event
        .pointer("/payload/leg_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Some(leg) = if leg_id.is_empty() {
        state.values_mut().next()
    } else {
        state.get_mut(leg_id)
    } {
        leg.realized_pnl_input += delta;
    }
}

fn push_history(event: &Value, leg_ids: BTreeSet<String>, closed_history: &mut Vec<Value>) {
    if leg_ids.is_empty() {
        return;
    }
    closed_history.push(json!({
        "event_id": string_field(event, "event_id", ""),
        "event_type": string_field(event, "event_type", ""),
        "event_time": string_field(event, "event_time", ""),
        "replay_ref": string_field(event, "replay_ref", ""),
        "leg_ids": leg_ids.into_iter().collect::<Vec<_>>()
    }));
}

fn position_status(state: &BTreeMap<String, LegState>, warnings: &[Value]) -> &'static str {
    if !warnings.is_empty() {
        return "warning";
    }
    if state.values().any(|leg| {
        leg.open_quantity > 0.0
            && (leg.closed_quantity > 0.0
                || leg.assigned_quantity > 0.0
                || leg.exercised_quantity > 0.0
                || leg.rolled_quantity > 0.0)
    }) {
        return "partially_closed";
    }
    if state.values().any(|leg| leg.open_quantity > 0.0) {
        return "open";
    }
    "closed"
}

fn leg_status(leg: &LegState) -> &'static str {
    if leg.open_quantity > 0.0 && leg.closed_quantity > 0.0 {
        "partially_closed"
    } else if leg.open_quantity > 0.0 {
        "open"
    } else if leg.assigned_quantity > 0.0 {
        "assigned"
    } else if leg.exercised_quantity > 0.0 {
        "exercised"
    } else if leg.expired_quantity > 0.0 {
        "expired"
    } else {
        "closed"
    }
}

fn number_field(value: &Value, key: &str) -> f64 {
    value.get(key).map(value_number).unwrap_or(0.0)
}

fn value_number(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|raw| raw.parse::<f64>().ok()))
        .unwrap_or(0.0)
}

fn decimal_string(value: f64) -> String {
    let rounded = format!("{:.6}", round6(value));
    let trimmed = rounded.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn checkpoint_hash(
    strategy_id: &str,
    position_id: &str,
    event_count: usize,
    transaction_count: usize,
) -> String {
    format!("chk_{strategy_id}_{position_id}_{event_count}_{transaction_count}")
        .replace([':', '/', ' '], "_")
}

fn string_field(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn optional_matches(value: &Value, expected: Option<&str>) -> bool {
    expected
        .map(|expected| value.as_str() == Some(expected))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_service(name: &str) -> crate::service::TradeAssemblyService {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let db =
            std::env::temp_dir().join(format!("tradeassembly-agent-surfaces-{name}-{stamp}.db"));
        crate::service::TradeAssemblyService::test_local(db.to_string_lossy().to_string())
    }

    #[test]
    fn selector_evaluate_explain_match_across_cli_http_mcp_and_acp() {
        let selector = selector();
        let snapshots = json!([
            snapshot("occ:SPY260717P00520000", 520.0, -0.34),
            snapshot("occ:SPY260717P00500000", 500.0, -0.12)
        ]);

        let cli = super::selector_surface(
            "cli",
            "evaluate",
            &selector,
            &snapshots,
            "selector_run_1",
            "snapshot://local/snap_001",
            Some("strat_selector"),
            Some("http://127.0.0.1:3001"),
        );
        let http = super::selector_surface(
            "http",
            "explain",
            &selector,
            &snapshots,
            "selector_run_1",
            "snapshot://local/snap_001",
            Some("strat_selector"),
            Some("http://127.0.0.1:3001"),
        );
        let mcp = super::selector_surface(
            "mcp",
            "evaluate",
            &selector,
            &snapshots,
            "selector_run_1",
            "snapshot://local/snap_001",
            Some("strat_selector"),
            Some("http://127.0.0.1:3001"),
        );
        let acp = super::selector_surface(
            "acp",
            "explain",
            &selector,
            &snapshots,
            "selector_run_1",
            "snapshot://local/snap_001",
            Some("strat_selector"),
            Some("http://127.0.0.1:3001"),
        );

        assert_eq!(cli["reasonCodeSummary"], http["reasonCodeSummary"]);
        assert_eq!(cli["reasonCodeSummary"], mcp["reasonCodeSummary"]);
        assert_eq!(cli["reasonCodeSummary"], acp["reasonCodeSummary"]);
        assert_eq!(cli["accepted_candidates"][0]["status"], "accepted");
        assert_eq!(
            cli["rejected_candidates"][0]["reason_codes"],
            json!(["outside_delta"])
        );
        assert!(cli["reportEnvelope"]["deepLinks"]["selector"]["href"]
            .as_str()
            .unwrap()
            .ends_with("/app/strategies/strat_selector/research?selectorRunId=selector_run_1"));
        assert_eq!(
            http["reportEnvelope"]["renderContract"]["agentVisible"],
            json!([
                "artifactBundle",
                "artifactRefs",
                "exportRefs",
                "replayRefs",
                "raw"
            ])
        );
        let serialized = serde_json::to_string(&cli).unwrap();
        assert!(serialized.contains("what to trade, when to trade, or how much to trade"));
        assert!(!serialized.contains("SUPER-SECRET-VALUE"));
    }

    #[test]
    fn fill_quality_acp_uses_shared_service_payload() {
        let service = test_service("fill-quality-acp");
        let request = json!({
            "strategy_id": "strat_local_btc_demo",
            "analysis_id": "fill_quality_latest",
            "studio_base_url": "http://127.0.0.1:3001"
        });

        let acp = super::fill_quality_surface_acp(&service, &request);
        let mcp = service.call_mcp_tool("tradeassembly.fill_quality.report", request);
        let mcp_payload = &mcp["structuredContent"];

        assert_eq!(acp["ok"], true);
        assert_eq!(acp["kind"], "fill_quality");
        assert_eq!(acp["summary"], mcp_payload["summary"]);
        assert_eq!(acp["agentSummary"], mcp_payload["agentSummary"]);
        assert_eq!(acp["reportEnvelope"]["kind"], "fill_quality");
        assert!(acp["agentSummary"]["studioDeepLinks"]["execution"]
            .as_str()
            .unwrap()
            .contains("fillQuality=fill_quality_latest"));
        let serialized = serde_json::to_string(&acp).unwrap();
        assert!(!serialized.contains("api_secret"));
        assert!(!serialized.contains("Bearer "));
        assert!(!serialized.to_lowercase().contains("should buy"));
    }

    #[test]
    fn lifecycle_calendar_acp_uses_shared_service_payload() {
        let service = test_service("lifecycle-calendar-acp");
        let request = json!({
            "strategy_id": "strat_local_btc_demo",
            "timeline_id": "lifecycle_calendar_latest",
            "studio_base_url": "http://127.0.0.1:3001"
        });

        let acp = super::lifecycle_calendar_surface_acp(&service, &request);
        let mcp = service.call_mcp_tool("tradeassembly.lifecycle_calendar.inspect", request);
        let mcp_payload = &mcp["structuredContent"];

        assert_eq!(
            acp["schemaVersion"],
            "tradeassembly.lifecycle_calendar.service.v1"
        );
        assert_eq!(acp["summary"], mcp_payload["summary"]);
        assert_eq!(acp["agentSummary"], mcp_payload["agentSummary"]);
        assert!(acp["deepLinks"]["calendar"]["href"]
            .as_str()
            .unwrap()
            .contains("lifecycleCalendar="));
        assert_eq!(acp["sideEffects"]["brokerStateChanged"], false);
        let serialized = serde_json::to_string(&acp).unwrap();
        assert!(!serialized.contains("api_secret"));
        assert!(!serialized.contains("Bearer "));
        assert!(!serialized.to_lowercase().contains("should buy"));
    }

    #[test]
    fn position_lifecycle_surfaces_share_safe_agent_payload() {
        let ledger = ledger_payload();
        let cli = super::position_lifecycle_surface(
            "inspect",
            &ledger,
            Some("http://127.0.0.1:3001"),
            None,
            None,
        );
        let http = super::position_lifecycle_surface(
            "inspect",
            &ledger,
            Some("http://127.0.0.1:3001"),
            None,
            None,
        );
        let mcp = super::position_lifecycle_surface(
            "inspect",
            &ledger,
            Some("http://127.0.0.1:3001"),
            None,
            None,
        );
        let acp = super::position_lifecycle_surface(
            "inspect",
            &ledger,
            Some("http://127.0.0.1:3001"),
            None,
            None,
        );

        for payload in [&cli, &http, &mcp, &acp] {
            assert_eq!(
                payload["schemaVersion"],
                "tradeassembly.position_lifecycle_surface.v1"
            );
            assert_eq!(payload["operation"], "inspect");
            assert_eq!(payload["summary"]["strategyId"], "strategy-spread");
            assert_eq!(payload["summary"]["positionId"], "position-spread");
            assert_eq!(payload["lifecycle"]["position_status"], "partially_closed");
            assert!(payload["deepLinks"]["studio"]["href"]
                .as_str()
                .unwrap()
                .ends_with("/app/strategies/strategy-spread/run?positionId=position-spread"));
            assert!(payload["exportRefs"]["json"]["path"]
                .as_str()
                .unwrap()
                .ends_with("position-spread/lifecycle.json"));
            assert_eq!(
                payload["replayRefs"]["checkpointHash"],
                payload["lifecycle"]["ledger_checkpoint_hash"]
            );
            assert!(!payload["timeline"].as_array().unwrap().is_empty());
            assert!(!payload["legs"].as_array().unwrap().is_empty());
            assert!(payload["noAdvice"]
                .as_str()
                .unwrap()
                .contains("what to trade, when to trade, or how much to trade"));
            assert!(!serde_json::to_string(payload)
                .unwrap()
                .contains("SUPER-SECRET-VALUE"));
        }
        assert_eq!(cli["summary"], http["summary"]);
        assert_eq!(cli["summary"], mcp["summary"]);
        assert_eq!(cli["summary"], acp["summary"]);
    }

    #[test]
    fn position_lifecycle_import_status_and_replay_use_local_journal() {
        let ledger = ledger_payload();
        let mut journal = super::PositionLifecycleJournal::default();

        let imported = journal.import(&ledger, Some("http://127.0.0.1:3001"));
        let status = journal.status(
            Some("strategy-spread"),
            Some("position-spread"),
            Some("http://127.0.0.1:3001"),
        );
        let replay = journal.replay(
            Some("strategy-spread"),
            Some("position-spread"),
            Some("http://127.0.0.1:3001"),
        );

        assert_eq!(imported["operation"], "import");
        assert_eq!(status["operation"], "status");
        assert_eq!(replay["operation"], "replay");
        assert_eq!(status["summary"]["snapshotCount"], json!(1));
        assert_eq!(
            status["lifecycle"]["snapshot_id"],
            imported["lifecycle"]["snapshot_id"]
        );
        assert_eq!(
            replay["replayRefs"]["journalRefs"],
            ledger["replay_plan"]["journal_refs"]
        );
        assert_eq!(
            imported["lifecycle"]["side_effects"],
            json!({
                "broker_orders_created": 0,
                "credentials_changed": 0,
                "strategy_logic_changed": 0,
                "trading_activations_changed": 0
            })
        );
    }

    #[test]
    fn lifecycle_reducer_matches_multileg_source_contract() {
        let snapshot = super::position_lifecycle_reduce(&spread_contract());

        assert_eq!(snapshot["strategy_id"], "strategy-spread");
        assert_eq!(snapshot["position_id"], "position-spread");
        assert!(snapshot["ledger_checkpoint_hash"]
            .as_str()
            .unwrap()
            .starts_with("chk_"));
        assert_eq!(snapshot["replay_ref"], "journal://local/position-spread");
        assert_eq!(snapshot["position_status"], "partially_closed");
        assert_eq!(snapshot["summary"]["open_quantity"], "2");
        assert_eq!(snapshot["summary"]["closed_quantity"], "1");
        assert_eq!(snapshot["summary"]["realized_pnl_input"], "-0.316667");
        assert_eq!(snapshot["summary"]["gross_exposure_input"], "8.133334");
        assert_eq!(
            snapshot["closed_history"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["event_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["evt-close", "evt-assignment"]
        );
        assert!(snapshot["no_advice_notice"]
            .as_str()
            .unwrap()
            .contains("TradeAssembly did not recommend"));
    }

    #[test]
    fn lifecycle_reducer_tracks_roll_exercise_expiration_and_overclose() {
        let mut contract = spread_contract();
        contract["legs"].as_array_mut().unwrap().push(json!({
            "leg_id": "rolled-call",
            "instrument": {"canonical_id": "option:SPY:510C", "asset_class": "option", "symbol": "SPY260717C00510000"},
            "side": "long",
            "ratio": "1"
        }));
        contract["events"].as_array_mut().unwrap().extend([
            event(
                "roll",
                "evt-roll",
                5,
                json!({"close_leg_id": "long-call", "open_leg_id": "rolled-call", "quantity": "1"}),
            ),
            event(
                "exercise",
                "evt-exercise",
                6,
                json!({"leg_quantities": {"long-call": "1"}}),
            ),
            event(
                "expiration",
                "evt-expiration",
                7,
                json!({"leg_quantities": {"rolled-call": "1"}}),
            ),
        ]);
        contract["transactions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "transaction_id": "txn-roll-open",
                "event_id": "evt-roll",
                "leg_id": "rolled-call",
                "quantity": "1",
                "price": "2.00",
                "currency": "USD"
            }));
        contract["replay_plan"]["journal_refs"] = json!(contract["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["replay_ref"].as_str().unwrap())
            .collect::<Vec<_>>());

        let snapshot = super::position_lifecycle_reduce(&contract);

        assert_eq!(snapshot["leg_state"]["long-call"]["rolled_quantity"], "1");
        assert_eq!(
            snapshot["leg_state"]["long-call"]["exercised_quantity"],
            "1"
        );
        assert_eq!(
            snapshot["leg_state"]["rolled-call"]["expired_quantity"],
            "1"
        );
        let closed_ids = snapshot["closed_history"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["event_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(closed_ids.contains(&"evt-roll"));
        assert!(closed_ids.contains(&"evt-expiration"));

        let mut overclose = spread_contract();
        overclose["events"].as_array_mut().unwrap().push(event(
            "close",
            "evt-overclose",
            8,
            json!({}),
        ));
        overclose["transactions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "transaction_id": "txn-overclose",
                "event_id": "evt-overclose",
                "leg_id": "short-call",
                "quantity": "5",
                "price": "0.10",
                "currency": "USD"
            }));
        let failed = super::position_lifecycle_reduce(&overclose);
        assert_eq!(failed["ok"], json!(false));
        assert!(failed["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "overclose"));
    }

    fn selector() -> Value {
        json!({
            "selector_id": "sel_spy_put_30d",
            "underlying_symbol": "SPY",
            "right": "put",
            "min_dte": 30,
            "max_dte": 45,
            "target_delta": -0.35,
            "min_delta": -0.4,
            "max_delta": -0.25,
            "max_bid_ask_width": 0.2,
            "min_open_interest": 100,
            "min_volume": 10,
            "max_snapshot_age_seconds": 86400,
            "required_provider_capabilities": ["options_chain"]
        })
    }

    fn snapshot(contract_id: &str, strike: f64, delta: f64) -> Value {
        json!({
            "canonical_contract_id": contract_id,
            "provider_aliases": {"alpaca-paper": contract_id},
            "underlying_symbol": "SPY",
            "symbol": contract_id,
            "right": "put",
            "strike": strike,
            "expiration": "2026-07-17",
            "dte": 34,
            "bid": 4.1,
            "ask": 4.2,
            "mid": 4.15,
            "last": 4.15,
            "open_interest": 950,
            "volume": 110,
            "iv": 0.22,
            "delta": delta,
            "gamma": 0.02,
            "theta": -0.03,
            "vega": 0.11,
            "rho": -0.02,
            "spread_width": 0.1,
            "as_of": "2026-06-23T13:59:00+00:00",
            "source_provider": "alpaca-paper",
            "supported_capabilities": ["options_chain"],
            "support_status": "supported",
            "provenance": {"snapshot": "fixture", "rawSecretsExposed": false}
        })
    }

    fn ledger_payload() -> Value {
        json!({
            "strategy_id": "strategy-spread",
            "position_id": "position-spread",
            "legs": [
                {"leg_id": "long-call", "instrument": {"canonical_id": "option:SPY:500C", "asset_class": "option", "symbol": "SPY260717C00500000"}, "side": "long", "ratio": "1"},
                {"leg_id": "short-call", "instrument": {"canonical_id": "option:SPY:505C", "asset_class": "option", "symbol": "SPY260717C00505000"}, "side": "short", "ratio": "1"}
            ],
            "events": [
                {"event_id": "evt-open", "event_type": "open", "event_time": "2026-06-23T15:00:00+00:00", "replay_ref": "journal://local/evt-open"},
                {"event_id": "evt-close", "event_type": "close", "event_time": "2026-06-23T15:01:00+00:00", "replay_ref": "journal://local/evt-close"}
            ],
            "transactions": [
                {"transaction_id": "txn-open-long", "event_id": "evt-open", "leg_id": "long-call", "quantity": "2", "price": "4.00", "currency": "USD"},
                {"transaction_id": "txn-open-short", "event_id": "evt-open", "leg_id": "short-call", "quantity": "1", "price": "1.50", "currency": "USD"},
                {"transaction_id": "txn-close-long", "event_id": "evt-close", "leg_id": "long-call", "quantity": "1", "price": "5.00", "currency": "USD"}
            ],
            "replay_plan": {
                "replay_ref": "journal://local/position-spread",
                "journal_refs": ["journal://local/evt-open", "journal://local/evt-close"],
                "deterministic": true,
                "requires_live_provider_calls": false
            },
            "status": "open",
            "debugSecret": "SUPER-SECRET-VALUE"
        })
    }

    fn spread_contract() -> Value {
        json!({
            "strategy_id": "strategy-spread",
            "position_id": "position-spread",
            "legs": [
                {"leg_id": "long-call", "instrument": {"canonical_id": "option:SPY:500C", "asset_class": "option", "symbol": "SPY260717C00500000"}, "side": "long", "ratio": "1"},
                {"leg_id": "short-call", "instrument": {"canonical_id": "option:SPY:505C", "asset_class": "option", "symbol": "SPY260717C00505000"}, "side": "short", "ratio": "1"}
            ],
            "events": [
                event("open", "evt-open", 0, json!({})),
                event("partial_fill", "evt-partial", 1, json!({})),
                event("close", "evt-close", 2, json!({})),
                event("assignment", "evt-assignment", 3, json!({"leg_quantities": {"short-call": "1"}})),
                event("manual_correction", "evt-correction", 4, json!({"realized_pnl_delta": "-1.25", "reason": "fee true-up"}))
            ],
            "transactions": [
                {"transaction_id": "txn-open-long", "event_id": "evt-open", "leg_id": "long-call", "quantity": "2", "price": "4.00", "currency": "USD"},
                {"transaction_id": "txn-open-short", "event_id": "evt-open", "leg_id": "short-call", "quantity": "1", "price": "1.50", "currency": "USD"},
                {"transaction_id": "txn-partial-long", "event_id": "evt-partial", "leg_id": "long-call", "quantity": "1", "price": "4.20", "currency": "USD"},
                {"transaction_id": "txn-close-long", "event_id": "evt-close", "leg_id": "long-call", "quantity": "1", "price": "5.00", "currency": "USD"}
            ],
            "replay_plan": {
                "replay_ref": "journal://local/position-spread",
                "journal_refs": ["journal://local/evt-open", "journal://local/evt-partial", "journal://local/evt-close", "journal://local/evt-assignment", "journal://local/evt-correction"]
            },
            "status": "open"
        })
    }

    fn event(kind: &str, event_id: &str, offset_minutes: i64, payload: Value) -> Value {
        json!({
            "event_id": event_id,
            "strategy_id": "strategy-spread",
            "position_id": "position-spread",
            "event_type": kind,
            "event_time": format!("2026-06-23T15:{offset_minutes:02}:00+00:00"),
            "replay_ref": format!("journal://local/{event_id}"),
            "payload": payload
        })
    }
}
