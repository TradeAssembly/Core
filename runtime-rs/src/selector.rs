// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SELECTOR_SCHEMA_VERSION: &str = "tradeassembly.selector.v1";
const NO_ADVICE_SELECTOR_NOTICE: &str = "Selectors are deterministic filters over user-defined criteria. TradeAssembly did not recommend what to trade, when to trade, or how much to trade.";

pub fn evaluate_selector(
    selector: &Value,
    snapshots: &[Value],
    run_id: &str,
    snapshot_ref: &str,
    evaluated_at_unix: i64,
) -> Value {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for snapshot in snapshots {
        let candidate = selector_candidate(selector, snapshot, run_id, evaluated_at_unix);
        if candidate["status"] == "accepted" {
            accepted.push(candidate);
        } else {
            rejected.push(candidate);
        }
    }
    accepted.sort_by(|left, right| {
        sort_tuple(selector, &left["contract"]).cmp(&sort_tuple(selector, &right["contract"]))
    });
    rejected.sort_by(|left, right| {
        let left_key = format!(
            "{}:{}:{}",
            str_at(&left["contract"], &["canonical_contract_id"]),
            str_at(&left["contract"], &["source_provider"]),
            str_at(left, &["candidate_id"])
        );
        let right_key = format!(
            "{}:{}:{}",
            str_at(&right["contract"], &["canonical_contract_id"]),
            str_at(&right["contract"], &["source_provider"]),
            str_at(right, &["candidate_id"])
        );
        left_key.cmp(&right_key)
    });
    let mut artifact = json!({
        "schema_version": SELECTOR_SCHEMA_VERSION,
        "run_id": run_id,
        "selector": selector,
        "snapshot_ref": snapshot_ref,
        "accepted_candidates": accepted,
        "rejected_candidates": rejected,
        "provider_data_assumptions": [
            "Selection used local snapshot fields and provider capability metadata only.",
            "Missing liquidity, Greeks, identity, freshness, or provider support failed closed."
        ],
        "warnings": [NO_ADVICE_SELECTOR_NOTICE],
        "replay_refs": {
            "commands": [format!("tradeassembly selector replay --run-id {run_id} --snapshot {snapshot_ref}")],
            "snapshotRef": snapshot_ref,
            "deterministic": true
        },
        "export_refs": {
            "json": {"kind": "json", "path": format!("tradeassembly://selector-runs/{run_id}/report.json")},
            "csv": {"kind": "csv", "path": format!("tradeassembly://selector-runs/{run_id}/candidates.csv")}
        },
        "notice": NO_ADVICE_SELECTOR_NOTICE
    });
    let reason_summary = reason_code_summary(&artifact);
    artifact["reasonCodeSummary"] = reason_summary;
    artifact
}

pub fn selector_evaluated_event(artifact: &Value) -> Value {
    json!({"eventType": "marketdata.selector.evaluated", "payload": artifact})
}

pub fn canonical_selector_payload(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}

pub fn validate_selector_spec_payload(selector: &Value) -> Value {
    match normalize_selector_spec(selector) {
        Ok(spec) => json!({
            "ok": true,
            "selector": spec,
            "reasonCodes": [],
            "notice": NO_ADVICE_SELECTOR_NOTICE
        }),
        Err(error) => json!({
            "ok": false,
            "errors": [error],
            "reasonCodes": ["invalid_selector_input"],
            "notice": NO_ADVICE_SELECTOR_NOTICE
        }),
    }
}

pub fn normalize_selector_spec(selector: &Value) -> Result<Value, String> {
    if selector
        .get("user_defined_criteria")
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Err("selector criteria must be user-defined".to_string());
    }
    for field in ["dte", "delta", "strike", "spread_width", "iv"] {
        ensure_min_le_max(selector, field)?;
    }
    reject_secret_material(selector)?;
    let mut spec = selector.as_object().cloned().unwrap_or_default();
    spec.entry("schema_version".to_string())
        .or_insert_with(|| json!(SELECTOR_SCHEMA_VERSION));
    let mut capabilities = spec
        .get("required_provider_capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(Value::String)
        .collect::<Vec<_>>();
    capabilities.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    spec.insert(
        "required_provider_capabilities".to_string(),
        Value::Array(capabilities),
    );
    spec.entry("sort".to_string())
        .or_insert_with(|| json!([{"key": "delta_distance"}]));
    spec.entry("tie_breaks".to_string()).or_insert_with(|| {
        json!([
            {"key": "dte"},
            {"key": "strike"},
            {"key": "canonical_contract_id"},
            {"key": "provider_ref"}
        ])
    });
    spec.entry("user_defined_criteria".to_string())
        .or_insert(Value::Bool(true));
    Ok(Value::Object(spec))
}

pub fn reject_secret_material(value: &Value) -> Result<(), String> {
    reject_secret_material_at(value, "$")
}

fn selector_candidate(
    selector: &Value,
    contract: &Value,
    run_id: &str,
    evaluated_at_unix: i64,
) -> Value {
    let mut reasons = std::collections::BTreeSet::<String>::new();
    let mut explanations = Vec::<String>::new();

    if let Some(right) = selector.get("right").and_then(Value::as_str) {
        if right != str_at(contract, &["right"]) {
            reasons.insert("invalid_selector_input".to_string());
            explanations.push(format!(
                "Contract right {} did not match user selector right {right}.",
                str_at(contract, &["right"])
            ));
        }
    }
    match str_at(contract, &["support_status"]).as_str() {
        "ambiguous" => {
            reasons.insert("ambiguous_identity".to_string());
            explanations
                .push("Contract identity was ambiguous in the provider snapshot.".to_string());
        }
        "unsupported" => {
            reasons.insert("unsupported_provider_capability".to_string());
            explanations.push("Provider marked this contract unsupported.".to_string());
        }
        _ => {}
    }
    let missing_capabilities = missing_capabilities(selector, contract);
    if !missing_capabilities.is_empty() {
        reasons.insert("unsupported_provider_capability".to_string());
        explanations.push(format!(
            "Provider snapshot lacked required capabilities: {}.",
            missing_capabilities.join(", ")
        ));
    }
    for (field, code) in [
        ("bid", "missing_bid"),
        ("ask", "missing_ask"),
        ("mid", "missing_mid"),
    ] {
        if contract.get(field).is_none_or(Value::is_null) {
            reasons.insert(code.to_string());
            explanations.push(format!("{} was missing.", title(field)));
        }
    }
    if selector.get("min_open_interest").is_some()
        && contract.get("open_interest").is_none_or(Value::is_null)
    {
        reasons.insert("missing_open_interest".to_string());
        explanations.push("Open interest was required but missing.".to_string());
    }
    if selector.get("min_volume").is_some() && contract.get("volume").is_none_or(Value::is_null) {
        reasons.insert("missing_volume".to_string());
        explanations.push("Volume was required but missing.".to_string());
    }
    if (selector.get("min_iv").is_some() || selector.get("max_iv").is_some())
        && contract.get("iv").is_none_or(Value::is_null)
    {
        reasons.insert("missing_iv".to_string());
        explanations.push("IV was required but missing.".to_string());
    }
    if (selector.get("target_delta").is_some()
        || selector.get("min_delta").is_some()
        || selector.get("max_delta").is_some())
        && contract.get("delta").is_none_or(Value::is_null)
    {
        reasons.insert("missing_greeks".to_string());
        explanations.push("Delta was required but missing.".to_string());
    }
    if let Some(max_age) = int_at(selector, "max_snapshot_age_seconds") {
        let age = evaluated_at_unix - int_at(contract, "as_of_unix").unwrap_or(evaluated_at_unix);
        if age > max_age {
            reasons.insert("stale_snapshot".to_string());
            explanations.push(format!("Snapshot age {age}s exceeded max {max_age}s."));
        }
    }
    range_checks(selector, contract, &mut reasons, &mut explanations);

    let status = if reasons.is_empty() {
        reasons.insert("accepted".to_string());
        explanations = vec!["Candidate satisfied the user-defined selector filters.".to_string()];
        "accepted"
    } else {
        "rejected"
    };
    let reason_codes = reasons.into_iter().map(Value::String).collect::<Vec<_>>();
    let candidate_id = candidate_id(&str_at(selector, &["selector_id"]), contract);
    json!({
        "candidate_id": candidate_id,
        "status": status,
        "contract": contract,
        "reason_codes": reason_codes,
        "explanations": explanations,
        "data_assumptions": [
            format!("Snapshot provider: {}.", str_at(contract, &["source_provider"])),
            format!("Snapshot timestamp: {}.", int_at(contract, "as_of_unix").unwrap_or_default()),
            "No live provider call is required to replay this selector artifact."
        ],
        "warnings": if status == "rejected" { json!(["Rejected fail-closed; TradeAssembly did not substitute another contract."]) } else { json!([]) },
        "provenance": {"source": "local_selector", "rawSecretsExposed": false},
        "replay_ref": format!("tradeassembly://selector-runs/{run_id}/candidates/{candidate_id}")
    })
}

fn range_checks(
    selector: &Value,
    contract: &Value,
    reasons: &mut std::collections::BTreeSet<String>,
    explanations: &mut Vec<String>,
) {
    check_min_max_i64(
        selector,
        contract,
        "dte",
        "outside_dte",
        reasons,
        explanations,
    );
    check_min_max_f64(
        selector,
        contract,
        "delta",
        "outside_delta",
        reasons,
        explanations,
    );
    check_min_max_f64(
        selector,
        contract,
        "strike",
        "outside_strike_distance",
        reasons,
        explanations,
    );
    if let (Some(target), Some(max_distance), Some(strike)) = (
        float_at(selector, "target_strike"),
        float_at(selector, "max_strike_distance"),
        float_at(contract, "strike"),
    ) {
        let distance = (strike - target).abs();
        if distance > max_distance {
            reasons.insert("outside_strike_distance".to_string());
            explanations.push(format!(
                "Strike distance {distance} exceeded max {max_distance}."
            ));
        }
    }
    let spread_width = float_at(contract, "spread_width").or_else(|| {
        Some((float_at(contract, "ask")? - float_at(contract, "bid")? * 1.0).round_to_10())
    });
    if let (Some(max_width), Some(width)) = (float_at(selector, "max_bid_ask_width"), spread_width)
    {
        if width > max_width {
            reasons.insert("outside_bid_ask_width".to_string());
            explanations.push(format!("Bid/ask width {width} exceeded max {max_width}."));
        }
    }
    if let (Some(max_pct), Some(width), Some(mid)) = (
        float_at(selector, "max_bid_ask_width_pct_of_mid"),
        spread_width,
        float_at(contract, "mid"),
    ) {
        if mid != 0.0 && width / mid * 100.0 > max_pct {
            reasons.insert("outside_bid_ask_width".to_string());
            explanations.push(format!("Bid/ask width exceeded max {max_pct}% of mid."));
        }
    }
    check_floor_i64(
        selector,
        contract,
        "open_interest",
        "outside_open_interest",
        reasons,
        explanations,
    );
    check_floor_i64(
        selector,
        contract,
        "volume",
        "outside_volume",
        reasons,
        explanations,
    );
    check_min_max_f64(
        selector,
        contract,
        "iv",
        "outside_iv",
        reasons,
        explanations,
    );
}

fn check_min_max_i64(
    selector: &Value,
    contract: &Value,
    field: &str,
    code: &str,
    reasons: &mut std::collections::BTreeSet<String>,
    explanations: &mut Vec<String>,
) {
    if let (Some(minimum), Some(value)) = (
        int_at(selector, &format!("min_{field}")),
        int_at(contract, field),
    ) {
        if value < minimum {
            reasons.insert(code.to_string());
            explanations.push(format!("{} {value} was below min {minimum}.", title(field)));
        }
    }
    if let (Some(maximum), Some(value)) = (
        int_at(selector, &format!("max_{field}")),
        int_at(contract, field),
    ) {
        if value > maximum {
            reasons.insert(code.to_string());
            explanations.push(format!("{} {value} was above max {maximum}.", title(field)));
        }
    }
}

fn check_min_max_f64(
    selector: &Value,
    contract: &Value,
    field: &str,
    code: &str,
    reasons: &mut std::collections::BTreeSet<String>,
    explanations: &mut Vec<String>,
) {
    if let (Some(minimum), Some(value)) = (
        float_at(selector, &format!("min_{field}")),
        float_at(contract, field),
    ) {
        if value < minimum {
            reasons.insert(code.to_string());
            explanations.push(format!("{} {value} was below min {minimum}.", title(field)));
        }
    }
    if let (Some(maximum), Some(value)) = (
        float_at(selector, &format!("max_{field}")),
        float_at(contract, field),
    ) {
        if value > maximum {
            reasons.insert(code.to_string());
            explanations.push(format!("{} {value} was above max {maximum}.", title(field)));
        }
    }
}

fn check_floor_i64(
    selector: &Value,
    contract: &Value,
    field: &str,
    code: &str,
    reasons: &mut std::collections::BTreeSet<String>,
    explanations: &mut Vec<String>,
) {
    if let (Some(minimum), Some(value)) = (
        int_at(selector, &format!("min_{field}")),
        int_at(contract, field),
    ) {
        if value < minimum {
            reasons.insert(code.to_string());
            explanations.push(format!("{} {value} was below min {minimum}.", title(field)));
        }
    }
}

fn sort_tuple(selector: &Value, contract: &Value) -> Vec<String> {
    let mut keys = selector
        .get("sort")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            json!([{"key": "delta_distance"}])
                .as_array()
                .unwrap()
                .clone()
        });
    keys.extend(
        json!([
            {"key": "dte"},
            {"key": "strike"},
            {"key": "canonical_contract_id"},
            {"key": "provider_ref"}
        ])
        .as_array()
        .unwrap()
        .clone(),
    );
    keys.into_iter()
        .map(|sort| sort_value(selector, contract, sort["key"].as_str().unwrap_or("")))
        .collect()
}

fn sort_value(selector: &Value, contract: &Value, key: &str) -> String {
    match key {
        "canonical_contract_id" => str_at(contract, &["canonical_contract_id"]),
        "provider_ref" => str_at(contract, &["source_provider"]),
        "expiration" => str_at(contract, &["expiration"]),
        "dte" => numeric_sort(float_at(contract, "dte").unwrap_or(f64::MAX)),
        "strike" => numeric_sort(float_at(contract, "strike").unwrap_or(f64::MAX)),
        "delta_distance" => numeric_sort(
            (float_at(contract, "delta").unwrap_or(0.0)
                - float_at(selector, "target_delta").unwrap_or(0.0))
            .abs(),
        ),
        "strike_distance" => numeric_sort(
            (float_at(contract, "strike").unwrap_or(0.0)
                - float_at(selector, "target_strike")
                    .unwrap_or(float_at(contract, "strike").unwrap_or(0.0)))
            .abs(),
        ),
        "bid_ask_width" => numeric_sort(bid_ask_width(contract).unwrap_or(f64::MAX)),
        "bid_ask_width_pct_of_mid" => {
            let pct = bid_ask_width(contract).unwrap_or(f64::MAX)
                / float_at(contract, "mid").unwrap_or(1.0)
                * 100.0;
            numeric_sort(pct)
        }
        "open_interest" => numeric_sort(float_at(contract, "open_interest").unwrap_or(f64::MAX)),
        "volume" => numeric_sort(float_at(contract, "volume").unwrap_or(f64::MAX)),
        "iv" => numeric_sort(float_at(contract, "iv").unwrap_or(f64::MAX)),
        _ => str_at(contract, &["canonical_contract_id"]),
    }
}

fn reason_code_summary(artifact: &Value) -> Value {
    let mut counts = std::collections::BTreeMap::<String, i64>::new();
    for bucket in ["accepted_candidates", "rejected_candidates"] {
        if let Some(candidates) = artifact.get(bucket).and_then(Value::as_array) {
            for candidate in candidates {
                if let Some(codes) = candidate.get("reason_codes").and_then(Value::as_array) {
                    for code in codes {
                        if let Some(code) = code.as_str() {
                            *counts.entry(code.to_string()).or_default() += 1;
                        }
                    }
                }
            }
        }
    }
    json!(counts)
}

fn missing_capabilities(selector: &Value, contract: &Value) -> Vec<String> {
    let supported = contract
        .get("supported_capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut missing = selector
        .get("required_provider_capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|capability| !supported.contains(capability))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    missing.sort();
    missing
}

fn ensure_min_le_max(selector: &Value, field: &str) -> Result<(), String> {
    let minimum = selector.get(format!("min_{field}")).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|item| item as f64))
    });
    let maximum = selector.get(format!("max_{field}")).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|item| item as f64))
    });
    if let (Some(minimum), Some(maximum)) = (minimum, maximum) {
        if minimum > maximum {
            return Err(format!("min_{field} must be <= max_{field}"));
        }
    }
    Ok(())
}

fn reject_secret_material_at(value: &Value, path: &str) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let lowered = key.to_lowercase().replace('-', "_");
                let normalized = lowered.replace('_', "");
                let safe_secret_metadata = normalized == "rawsecretsexposed";
                let sensitive = [
                    "api_key",
                    "apikey",
                    "password",
                    "private_key",
                    "secret",
                    "token",
                ]
                .iter()
                .any(|fragment| lowered.contains(fragment));
                if sensitive && !safe_secret_metadata {
                    return Err(format!(
                        "selector artifacts must not contain secret material at {path}.{key}"
                    ));
                }
                reject_secret_material_at(child, &format!("{path}.{key}"))?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                reject_secret_material_at(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn candidate_id(selector_id: &str, contract: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "{selector_id}:{}:{}",
        str_at(contract, &["canonical_contract_id"]),
        str_at(contract, &["source_provider"])
    ));
    format!("candidate_{:x}", hasher.finalize())
        .chars()
        .take("candidate_".len() + 12)
        .collect()
}

fn bid_ask_width(contract: &Value) -> Option<f64> {
    float_at(contract, "spread_width")
        .or_else(|| Some((float_at(contract, "ask")? - float_at(contract, "bid")?).round_to_10()))
}

fn numeric_sort(value: f64) -> String {
    format!("{value:020.10}")
}

fn str_at(value: &Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn int_at(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn float_at(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn title(field: &str) -> String {
    let mut chars = field.replace('_', " ").chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    chars.into_iter().collect()
}

trait RoundToTen {
    fn round_to_10(self) -> f64;
}

impl RoundToTen for f64 {
    fn round_to_10(self) -> f64 {
        (self * 10_000_000_000.0).round() / 10_000_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    const NOW: i64 = 1_782_231_200;
    const NOTICE: &str = "Selectors are deterministic filters over user-defined criteria. TradeAssembly did not recommend what to trade, when to trade, or how much to trade.";

    #[test]
    fn selector_evaluation_is_deterministic_and_orders_by_user_sort_then_tie_breaks() {
        let spec = selector(json!({}));
        let farther = snapshot(
            "occ:SPY260717P00521000",
            json!({"strike": 521.0, "delta": -0.34, "bid": 4.0, "ask": 4.2, "mid": 4.1, "spread_width": 0.2}),
        );
        let closer_left = snapshot(
            "occ:SPY260717P00519000",
            json!({"strike": 519.0, "delta": -0.305, "bid": 4.1, "ask": 4.2, "mid": 4.15, "spread_width": 0.1}),
        );
        let closer_right = snapshot(
            "occ:SPY260717P00520000",
            json!({"strike": 520.0, "delta": -0.305, "bid": 4.1, "ask": 4.2, "mid": 4.15, "spread_width": 0.1}),
        );

        let first = super::evaluate_selector(
            &spec,
            &[farther.clone(), closer_right.clone(), closer_left.clone()],
            "selector_run_1",
            "snapshot://local/snap_001",
            NOW,
        );
        let second = super::evaluate_selector(
            &spec,
            &[closer_left, farther, closer_right],
            "selector_run_1",
            "snapshot://local/snap_001",
            NOW,
        );

        assert_eq!(
            first["accepted_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["contract"]["canonical_contract_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "occ:SPY260717P00519000",
                "occ:SPY260717P00520000",
                "occ:SPY260717P00521000",
            ]
        );
        assert_eq!(
            super::canonical_selector_payload(&first),
            super::canonical_selector_payload(&second)
        );
        assert_eq!(
            first["accepted_candidates"][0]["reason_codes"],
            json!(["accepted"])
        );
    }

    #[test]
    fn selector_explain_rejects_missing_stale_unsupported_and_out_of_bounds_filters() {
        let bad = snapshot(
            "occ:SPY260717P00480000",
            json!({
                "strike": 480.0,
                "delta": null,
                "dte": 60,
                "bid": null,
                "ask": null,
                "mid": null,
                "open_interest": null,
                "volume": null,
                "iv": null,
                "gamma": null,
                "theta": null,
                "vega": null,
                "rho": null,
                "spread_width": null,
                "as_of_unix": NOW - 1800,
                "supported_capabilities": [],
                "support_status": "ambiguous"
            }),
        );

        let payload = super::evaluate_selector(
            &selector(json!({})),
            &[bad],
            "selector_run_1",
            "snapshot://local/snap_001",
            NOW,
        );

        let codes = payload["rejected_candidates"][0]["reason_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        for code in [
            "ambiguous_identity",
            "missing_ask",
            "missing_bid",
            "missing_greeks",
            "missing_iv",
            "missing_mid",
            "missing_open_interest",
            "missing_volume",
            "outside_dte",
            "outside_strike_distance",
            "stale_snapshot",
            "unsupported_provider_capability",
        ] {
            assert!(codes.contains(code), "missing {code}");
        }
        assert_eq!(
            payload["rejected_candidates"][0]["warnings"],
            json!(["Rejected fail-closed; TradeAssembly did not substitute another contract."])
        );
        assert_eq!(payload["notice"], NOTICE);
        assert!(payload["notice"].as_str().unwrap().contains("recommend"));
    }

    #[test]
    fn selector_records_journal_evidence_and_does_not_emit_secret_material() {
        let artifact = super::evaluate_selector(
            &selector(json!({})),
            &[snapshot("occ:SPY260717P00520000", json!({}))],
            "selector_run_1",
            "snapshot://local/snap_001",
            NOW,
        );
        let event = super::selector_evaluated_event(&artifact);

        assert_eq!(event["eventType"], "marketdata.selector.evaluated");
        assert_eq!(event["payload"], artifact);
        assert_eq!(
            artifact["replay_refs"]["commands"],
            json!(["tradeassembly selector replay --run-id selector_run_1 --snapshot snapshot://local/snap_001"])
        );
        assert_eq!(
            artifact["export_refs"]["json"]["path"],
            "tradeassembly://selector-runs/selector_run_1/report.json"
        );
        assert_eq!(
            artifact["export_refs"]["csv"]["path"],
            "tradeassembly://selector-runs/selector_run_1/candidates.csv"
        );
        assert!(!super::canonical_selector_payload(&artifact).contains("SUPER-SECRET-VALUE"));
    }

    #[test]
    fn selector_rejects_right_mismatch_without_substituting_another_trade() {
        let artifact = super::evaluate_selector(
            &selector(json!({"right": "call"})),
            &[snapshot("occ:SPY260717P00520000", json!({}))],
            "selector_run_1",
            "snapshot://local/snap_001",
            NOW,
        );

        assert_eq!(artifact["accepted_candidates"], json!([]));
        assert_eq!(artifact["rejected_candidates"][0]["status"], "rejected");
        assert!(artifact["rejected_candidates"][0]["reason_codes"]
            .as_array()
            .unwrap()
            .contains(&json!("invalid_selector_input")));
        assert!(artifact["rejected_candidates"][0]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("did not substitute another contract"));
    }

    #[test]
    fn selector_spec_validation_normalizes_defaults_and_fails_closed() {
        let spec = super::normalize_selector_spec(&selector(json!({
            "required_provider_capabilities": ["marketdata.option_chain", "marketdata.quotes", "marketdata.option_chain"]
        })))
        .unwrap();

        assert_eq!(spec["schema_version"], "tradeassembly.selector.v1");
        assert_eq!(
            spec["required_provider_capabilities"],
            json!(["marketdata.option_chain", "marketdata.quotes"])
        );
        assert_eq!(spec["tie_breaks"][2]["key"], "canonical_contract_id");
        assert_eq!(spec["tie_breaks"][3]["key"], "provider_ref");

        let result =
            super::validate_selector_spec_payload(&selector(json!({"min_dte": 60, "max_dte": 30})));
        assert_eq!(result["ok"], false);
        assert_eq!(result["reasonCodes"], json!(["invalid_selector_input"]));
        assert_eq!(result["notice"], NOTICE);
    }

    #[test]
    fn selector_contract_payloads_reject_secret_material() {
        let bad_snapshot = snapshot(
            "occ:SPY260717P00520000",
            json!({"provenance": {"api_key": "SUPER-SECRET-VALUE"}}),
        );
        assert!(super::reject_secret_material(&bad_snapshot).is_err());

        let bad_run = json!({
            "run_id": "selector_run_1",
            "replay_refs": {"token": "SUPER-SECRET-VALUE"}
        });
        assert!(super::reject_secret_material(&bad_run).is_err());
    }

    #[test]
    fn selector_serialization_is_deterministic_for_replay_refs() {
        let first = super::evaluate_selector(
            &selector(json!({})),
            &[snapshot("occ:SPY260717P00520000", json!({}))],
            "selector_run_1",
            "snapshot://local/snap_001",
            NOW,
        );
        let second: serde_json::Value =
            serde_json::from_str(&super::canonical_selector_payload(&first)).unwrap();

        assert_eq!(
            super::canonical_selector_payload(&first),
            super::canonical_selector_payload(&second)
        );
    }

    fn selector(overrides: serde_json::Value) -> serde_json::Value {
        merge(
            json!({
                "schema_version": "tradeassembly.selector.v1",
                "selector_id": "sel_spy_put_30d",
                "underlying_symbol": "SPY",
                "right": "put",
                "min_dte": 30,
                "max_dte": 45,
                "target_delta": -0.30,
                "min_delta": -0.40,
                "max_delta": -0.20,
                "target_strike": 520.0,
                "min_strike": 500.0,
                "max_strike": 540.0,
                "max_strike_distance": 25.0,
                "max_bid_ask_width": 0.25,
                "max_bid_ask_width_pct_of_mid": 10.0,
                "min_open_interest": 500,
                "min_volume": 100,
                "min_iv": 0.1,
                "max_iv": 0.7,
                "max_snapshot_age_seconds": 120,
                "required_provider_capabilities": ["marketdata.option_chain", "marketdata.quotes"],
                "sort": [{"key": "delta_distance"}, {"key": "bid_ask_width_pct_of_mid"}],
                "user_defined_criteria": true
            }),
            overrides,
        )
    }

    fn snapshot(contract_id: &str, overrides: serde_json::Value) -> serde_json::Value {
        merge(
            json!({
                "canonical_contract_id": contract_id,
                "provider_aliases": {"local-data": contract_id.trim_start_matches("occ:")},
                "underlying_symbol": "SPY",
                "symbol": contract_id.trim_start_matches("occ:"),
                "right": "put",
                "strike": 520.0,
                "expiration": "2026-07-17",
                "dte": 31,
                "bid": 4.1,
                "ask": 4.2,
                "mid": 4.15,
                "last": 4.12,
                "open_interest": 1800,
                "volume": 420,
                "iv": 0.24,
                "delta": -0.31,
                "gamma": 0.02,
                "theta": -0.03,
                "vega": 0.11,
                "rho": -0.04,
                "spread_width": 0.1,
                "as_of_unix": NOW - 30,
                "source_provider": "local-data",
                "supported_capabilities": ["marketdata.option_chain", "marketdata.quotes"],
                "support_status": "supported",
                "provenance": {"snapshotId": format!("snap_{contract_id}"), "rawSecretsExposed": false}
            }),
            overrides,
        )
    }

    fn merge(mut base: serde_json::Value, overrides: serde_json::Value) -> serde_json::Value {
        let base = base.as_object_mut().unwrap();
        for (key, value) in overrides.as_object().unwrap() {
            base.insert(key.clone(), value.clone());
        }
        serde_json::Value::Object(base.clone())
    }
}
