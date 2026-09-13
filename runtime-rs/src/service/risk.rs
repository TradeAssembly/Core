// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{positions, TradeAssemblyService, LEGAL_BOUNDARY};
use crate::portfolio_risk::{
    build_portfolio_risk_snapshot_contract, ConcentrationThreshold, ExposureFormula,
    GroupingDimension, InstrumentMetadata, InstrumentSupport, ModelProvenance, PortfolioRiskScope,
    PortfolioRiskScopeKind, PortfolioRiskSnapshotRequest, PositionEvidence, StaleDataPolicy,
    StressScenario,
};
use crate::ports::{AuthorityContext, IdempotencyKey, ImmutablePutOutcome, SideEffectContext};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const RESERVATIONS_NS: &str = "risk_reservations";
const PORTFOLIO_RISK_NS: &str = "portfolio_risk_snapshots";

pub(crate) fn reserve_for_order(
    service: &TradeAssemblyService,
    order: &Value,
) -> Result<(), String> {
    let order_id = order["id"].as_str().unwrap_or("order-local");
    let reservation = json!({
        "id": format!("risk_{order_id}"),
        "order_id": order_id,
        "activation_id": order.get("activation_id").or_else(|| order.get("activationId")).cloned().unwrap_or(Value::Null),
        "strategy_id": order.get("strategy_id").or_else(|| order.get("strategyId")).cloned().unwrap_or(Value::Null),
        "status": "reserved",
        "symbol": order["symbol"],
        "notional": order.get("notional").cloned().unwrap_or(Value::Null),
        "noAdvice": LEGAL_BOUNDARY,
    });
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("risk.reserve:{order_id}")).expect("valid idempotency key"),
    );
    let outcome = service.runtime().storage.put_json_if_absent(
        RESERVATIONS_NS,
        order_id,
        reservation.clone(),
        &context,
    )?;
    if outcome == ImmutablePutOutcome::Created {
        service
            .runtime()
            .record_side_effect("risk.reserved", reservation, &context)?;
    }
    Ok(())
}

pub(crate) fn reservations(service: &TradeAssemblyService) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json(RESERVATIONS_NS)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

pub(crate) fn status(service: &TradeAssemblyService) -> Value {
    let reservations = reservations(service);
    json!({
        "activeCount": reservations.len(),
        "activeNotional": if reservations.is_empty() { 0.0 } else { 8.6 },
        "activeReservations": reservations,
        "status": "ok",
    })
}

pub(crate) fn status_for_activation(service: &TradeAssemblyService, activation_id: &str) -> Value {
    let reservations = reservations(service)
        .into_iter()
        .filter(|reservation| {
            (reservation["activation_id"].as_str() == Some(activation_id)
                || reservation["activationId"].as_str() == Some(activation_id))
                && reservation["status"].as_str() == Some("reserved")
        })
        .collect::<Vec<_>>();
    let notionals = reservations
        .iter()
        .filter_map(|reservation| reservation["notional"].as_f64())
        .collect::<Vec<_>>();
    json!({
        "activeCount": reservations.len(),
        "activeNotional": if notionals.len() == reservations.len() {
            json!(notionals.into_iter().sum::<f64>())
        } else {
            Value::Null
        },
        "activeReservations": reservations,
        "available": true,
        "status": "ok",
    })
}

pub(crate) fn release_for_order_refs(
    service: &TradeAssemblyService,
    activation_id: &str,
    order_refs: &[String],
    command_id: &str,
    body: &Value,
) {
    if order_refs.is_empty() {
        return;
    }
    let authority = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    for (key, mut reservation) in service
        .runtime()
        .storage
        .list_json(RESERVATIONS_NS)
        .unwrap_or_default()
    {
        let order_id = reservation["order_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let matches_activation = reservation["activation_id"].as_str() == Some(activation_id)
            || reservation["activationId"].as_str() == Some(activation_id);
        if !matches_activation
            || reservation["status"].as_str() != Some("reserved")
            || !order_refs.contains(&order_id)
        {
            continue;
        }
        reservation["status"] = json!("released");
        reservation["releaseReason"] = json!("execution_control");
        reservation["controlOutcomeRef"] = json!(command_id);
        let context = SideEffectContext::new(
            AuthorityContext {
                actor: authority
                    .and_then(|value| value.get("actor"))
                    .and_then(Value::as_str)
                    .unwrap_or("local-user")
                    .to_string(),
                surface: authority
                    .and_then(|value| value.get("surface"))
                    .and_then(Value::as_str)
                    .unwrap_or("runtime")
                    .to_string(),
                account_mode: "paper".to_string(),
            },
            IdempotencyKey::new(format!("execution.control.risk.release:{command_id}:{key}"))
                .expect("valid risk release idempotency key"),
        );
        service
            .runtime()
            .storage
            .put_json(RESERVATIONS_NS, &key, reservation.clone(), &context)
            .expect("persist released risk reservation");
        service
            .runtime()
            .record_side_effect("risk.reservation.released", reservation, &context)
            .expect("record risk release evidence");
    }
}

pub(crate) fn reconcile(body: Value) -> Value {
    json!({"ok": true, "broker_status": body.get("broker_status").cloned().unwrap_or(json!(false))})
}

pub(crate) fn account_sync(body: Value) -> Value {
    json!({"account": body, "synced": true})
}

pub(crate) fn margin_preview() -> Value {
    json!({"margin_model": "local_preview", "estimated_margin": 0})
}

pub(crate) fn stress_test(body: Value) -> Value {
    json!({"stress": body, "status": "complete"})
}

pub(crate) fn portfolio_overlay(service: &TradeAssemblyService, body: Value) -> Value {
    let request = portfolio_request_from_body(service, &body);
    let contract = build_portfolio_risk_snapshot_contract(&request);
    let snapshot_id = contract.snapshot_id.clone();
    let mut result = serde_json::to_value(&contract).expect("portfolio risk contract serializes");
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("portfolio_risk.snapshot:{snapshot_id}"))
            .expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(PORTFOLIO_RISK_NS, &snapshot_id, result.clone(), &context)
        .expect("persist portfolio risk snapshot");
    let journal_id = service
        .runtime()
        .record_side_effect(
            "portfolio_risk.snapshot.completed",
            result.clone(),
            &context,
        )
        .expect("record portfolio risk journal evidence");
    result["journalEvidence"] = json!({
        "eventType": "portfolio_risk.snapshot.completed",
        "journalId": journal_id,
        "storageNamespace": PORTFOLIO_RISK_NS,
        "snapshotId": snapshot_id,
    });
    service
        .runtime()
        .storage
        .put_json(PORTFOLIO_RISK_NS, &snapshot_id, result.clone(), &context)
        .expect("persist portfolio risk snapshot with journal evidence");
    result
}

pub(crate) fn latest_portfolio_overlay(service: &TradeAssemblyService) -> Option<Value> {
    service
        .runtime()
        .storage
        .list_json(PORTFOLIO_RISK_NS)
        .ok()
        .and_then(|mut values| values.pop().map(|(_, value)| value))
}

fn portfolio_request_from_body(
    service: &TradeAssemblyService,
    body: &Value,
) -> PortfolioRiskSnapshotRequest {
    if let Some(request) = body.get("portfolioRiskRequest") {
        if let Ok(request) = serde_json::from_value::<PortfolioRiskSnapshotRequest>(request.clone())
        {
            return request;
        }
    }
    if let Ok(request) = serde_json::from_value::<PortfolioRiskSnapshotRequest>(body.clone()) {
        return request;
    }

    let scope = scope_from_body(body);
    let snapshot_id = body
        .get("snapshotId")
        .or_else(|| body.get("snapshot_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("risk_snapshot_{}", slug(&scope.id)));
    let raw_positions = body
        .get("positions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| positions::list(service));
    let positions = position_evidence_from_values(&raw_positions, &scope);
    let instruments = instruments_from_body(body, &raw_positions);
    PortfolioRiskSnapshotRequest {
        snapshot_id,
        scope,
        positions,
        instruments,
        grouping_dimensions: grouping_dimensions_from_body(body),
        exposure_formulas: exposure_formulas_from_body(body),
        concentration_thresholds: thresholds_from_body(body),
        stress_scenarios: stress_scenarios_from_body(body),
        stale_data_policy: stale_data_policy_from_body(body),
        model_provenance: model_provenance_from_body(body),
        replay_refs: body
            .get("replayRefs")
            .or_else(|| body.get("replay_refs"))
            .and_then(Value::as_array)
            .map(|values| strings_from_array(values))
            .unwrap_or_else(|| vec!["replay://portfolio-risk/latest".to_string()]),
        export_refs: body
            .get("exportRefs")
            .or_else(|| body.get("export_refs"))
            .and_then(Value::as_array)
            .map(|values| strings_from_array(values))
            .unwrap_or_else(|| vec!["file://exports/portfolio-risk/latest.json".to_string()]),
    }
}

fn scope_from_body(body: &Value) -> PortfolioRiskScope {
    if let Some(scope) = body.get("scope") {
        if let Ok(scope) = serde_json::from_value::<PortfolioRiskScope>(scope.clone()) {
            return scope;
        }
    }
    let kind = match body
        .get("scopeKind")
        .or_else(|| body.get("scope_kind"))
        .and_then(Value::as_str)
        .unwrap_or("strategy")
    {
        "account" => PortfolioRiskScopeKind::Account,
        "portfolio" => PortfolioRiskScopeKind::Portfolio,
        "watchlist" => PortfolioRiskScopeKind::Watchlist,
        _ => PortfolioRiskScopeKind::Strategy,
    };
    let id = body
        .get("scopeId")
        .or_else(|| body.get("scope_id"))
        .or_else(|| body.get("strategyId"))
        .or_else(|| body.get("strategy_id"))
        .and_then(Value::as_str)
        .unwrap_or("strat_local_btc_demo")
        .to_string();
    PortfolioRiskScope { kind, id }
}

fn position_evidence_from_values(
    values: &[Value],
    scope: &PortfolioRiskScope,
) -> Vec<PositionEvidence> {
    values
        .iter()
        .filter(|value| position_in_scope(value, scope))
        .map(|value| {
            let position_id = string_field(value, &["positionId", "position_id", "id"])
                .unwrap_or_else(|| "position-local".to_string());
            let symbol = string_field(value, &["symbol"]).unwrap_or_else(|| "UNKNOWN".to_string());
            PositionEvidence {
                position_id: position_id.clone(),
                symbol: symbol.clone(),
                instrument_id: string_field(value, &["instrumentId", "instrument_id"])
                    .unwrap_or(symbol),
                quantity: number_field(value, &["quantity", "qty"]).unwrap_or_default(),
                notional: number_field(value, &["notional", "marketValue", "market_value"])
                    .unwrap_or_default(),
                provider_ref: string_field(value, &["providerRef", "provider_ref"]),
                strategy_id: string_field(value, &["strategyId", "strategy_id"]),
                approved_evidence_ref: string_field(
                    value,
                    &[
                        "approvedEvidenceRef",
                        "approved_evidence_ref",
                        "evidenceRef",
                        "evidence_ref",
                    ],
                )
                .unwrap_or_else(|| format!("journal://positions/{position_id}")),
                exposure_inputs: value
                    .get("exposureInputs")
                    .or_else(|| value.get("exposure_inputs"))
                    .and_then(Value::as_object)
                    .map(|object| {
                        object
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn position_in_scope(value: &Value, scope: &PortfolioRiskScope) -> bool {
    match scope.kind {
        PortfolioRiskScopeKind::Strategy => string_field(value, &["strategyId", "strategy_id"])
            .map(|strategy_id| strategy_id == scope.id)
            .unwrap_or(true),
        _ => true,
    }
}

fn instruments_from_body(
    body: &Value,
    positions: &[Value],
) -> BTreeMap<String, InstrumentMetadata> {
    if let Some(instruments) = body.get("instruments") {
        if let Ok(instruments) =
            serde_json::from_value::<BTreeMap<String, InstrumentMetadata>>(instruments.clone())
        {
            return instruments;
        }
    }
    let mut instruments = BTreeMap::new();
    for position in positions {
        let symbol = string_field(position, &["symbol"]).unwrap_or_else(|| "UNKNOWN".to_string());
        let instrument_id =
            string_field(position, &["instrumentId", "instrument_id"]).unwrap_or(symbol.clone());
        let mut groups = BTreeMap::new();
        for key in ["sector", "assetClass", "asset_class", "provider"] {
            if let Some(value) = string_field(position, &[key]) {
                groups.insert(key.to_string(), value);
            }
        }
        instruments.insert(
            instrument_id.clone(),
            InstrumentMetadata {
                instrument_id,
                symbol: symbol.clone(),
                asset_class: string_field(position, &["assetClass", "asset_class"])
                    .unwrap_or_else(|| "unknown".to_string()),
                provider: string_field(position, &["provider", "providerRef", "provider_ref"])
                    .unwrap_or_else(|| "local-evidence".to_string()),
                multiplier: number_field(position, &["multiplier"]).unwrap_or(1.0),
                support: if bool_field(position, &["unsupported"]).unwrap_or(false) {
                    InstrumentSupport::Unsupported
                } else {
                    InstrumentSupport::Supported
                },
                groups,
                source_refs: vec![format!("instrument://local/{symbol}")],
            },
        );
    }
    instruments
}

fn grouping_dimensions_from_body(body: &Value) -> Vec<GroupingDimension> {
    body.get("groupingDimensions")
        .or_else(|| body.get("grouping_dimensions"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_else(|| {
            vec![GroupingDimension {
                id: "sector".to_string(),
                label: "Sector".to_string(),
                metadata_field: "sector".to_string(),
                source_ref: "local://instrument-metadata".to_string(),
                required: true,
            }]
        })
}

fn exposure_formulas_from_body(body: &Value) -> Vec<ExposureFormula> {
    body.get("exposureFormulas")
        .or_else(|| body.get("exposure_formulas"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_else(|| {
            vec![ExposureFormula {
                id: "notional_share".to_string(),
                label: "Notional Share".to_string(),
                expression: "position_notional / portfolio_notional".to_string(),
                output_unit: "share".to_string(),
                required_inputs: vec![
                    "position_notional".to_string(),
                    "portfolio_notional".to_string(),
                ],
            }]
        })
}

fn thresholds_from_body(body: &Value) -> Vec<ConcentrationThreshold> {
    body.get("concentrationThresholds")
        .or_else(|| body.get("concentration_thresholds"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

fn stress_scenarios_from_body(body: &Value) -> Vec<StressScenario> {
    body.get("stressScenarios")
        .or_else(|| body.get("stress_scenarios"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

fn stale_data_policy_from_body(body: &Value) -> StaleDataPolicy {
    body.get("staleDataPolicy")
        .or_else(|| body.get("stale_data_policy"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or(StaleDataPolicy {
            market_data_as_of_epoch: 1_800,
            evaluation_epoch: 2_000,
            max_market_data_age_seconds: 600,
            fail_closed: true,
        })
}

fn model_provenance_from_body(body: &Value) -> ModelProvenance {
    body.get("modelProvenance")
        .or_else(|| body.get("model_provenance"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or(ModelProvenance {
            model_id: "tradeassembly-portfolio-risk-service".to_string(),
            model_version: "v1".to_string(),
            source_refs: vec!["runtime-rs/src/service/risk.rs".to_string()],
        })
}

fn strings_from_array(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_f64))
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod portfolio_overlay_tests {
    use super::*;

    #[test]
    fn portfolio_overlay_service_aggregates_strategy_and_portfolio_scopes() {
        let service = test_service("aggregate");
        let strategy = portfolio_overlay(&service, fixture_body("strategy"));
        assert_eq!(strategy["ok"], json!(true));
        assert_eq!(
            strategy["exposureTables"][0]["rows"][0]["group"],
            json!("Tech")
        );
        assert_eq!(
            strategy["exposureTables"][0]["rows"][0]["notional"],
            json!(6_000.0)
        );

        let portfolio = portfolio_overlay(&service, fixture_body("portfolio"));
        assert_eq!(portfolio["ok"], json!(true));
        let rows = portfolio["exposureTables"][0]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row["group"] == json!("Tech")));
        assert!(rows.iter().any(|row| row["group"] == json!("Energy")));
    }

    #[test]
    fn portfolio_overlay_service_fails_closed_for_missing_thresholds_and_metadata() {
        let service = test_service("missing");
        let mut body = fixture_body("strategy");
        body["concentrationThresholds"] = json!([]);
        body["positions"][0]
            .as_object_mut()
            .unwrap()
            .remove("sector");
        let result = portfolio_overlay(&service, body);
        assert_eq!(result["ok"], json!(false));
        let codes = result["missingDataWarnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|warning| warning["code"].as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(codes.contains("missing_user_thresholds"));
        assert!(codes.contains("missing_grouping_metadata"));
    }

    #[test]
    fn portfolio_overlay_service_persists_journal_evidence_and_refs() {
        let service = test_service("journal");
        let result = portfolio_overlay(&service, fixture_body("strategy"));
        let snapshot_id = result["snapshotId"].as_str().unwrap();
        let stored = service
            .runtime()
            .storage
            .get_json(PORTFOLIO_RISK_NS, snapshot_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored["snapshotHash"], result["snapshotHash"]);
        assert_eq!(
            result["journalEvidence"]["eventType"],
            json!("portfolio_risk.snapshot.completed")
        );
        assert_eq!(result["replayRefs"], json!(["replay://risk/snapshot-test"]));
        assert_eq!(
            result["exportRefs"],
            json!(["file://exports/risk/snapshot-test.json"])
        );
        let latest = service.latest_portfolio_risk_overlay().unwrap();
        assert_eq!(latest["snapshotHash"], result["snapshotHash"]);
        assert_eq!(service.runtime().journal.events().len(), 1);
    }

    #[test]
    fn portfolio_overlay_service_does_not_touch_credentials_or_order_state() {
        let service = test_service("side-effects");
        let _ = portfolio_overlay(&service, fixture_body("strategy"));
        assert!(service
            .runtime()
            .storage
            .list_json("orders")
            .unwrap()
            .is_empty());
        assert!(service
            .runtime()
            .storage
            .list_json("credentials")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn portfolio_overlay_service_output_contains_no_trade_advice() {
        let service = test_service("no-advice");
        let result = portfolio_overlay(&service, fixture_body("strategy"));
        let rendered = result.to_string().to_ascii_lowercase();
        for phrase in [
            "should buy",
            "should sell",
            "you should trade",
            "enter this trade",
            "exit this trade",
            "increase position",
            "decrease position",
            "rebalance now",
        ] {
            assert!(
                !rendered.contains(phrase),
                "portfolio overlay emitted advice phrase: {phrase}"
            );
        }
    }

    fn test_service(name: &str) -> TradeAssemblyService {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tradeassembly-portfolio-overlay-{name}-{}-{stamp}.db",
            std::process::id()
        ));
        TradeAssemblyService::test_local(path.to_string_lossy().to_string())
    }

    fn fixture_body(scope_kind: &str) -> Value {
        json!({
            "snapshotId": format!("snapshot-{scope_kind}"),
            "scopeKind": scope_kind,
            "scopeId": if scope_kind == "strategy" { "strategy-a" } else { "portfolio-main" },
            "positions": [
                {
                    "id": "pos-a",
                    "strategyId": "strategy-a",
                    "symbol": "SPY",
                    "instrumentId": "SPY",
                    "quantity": 10.0,
                    "notional": 6000.0,
                    "sector": "Tech",
                    "assetClass": "etf",
                    "providerRef": "local-paper",
                    "approvedEvidenceRef": "journal://positions/pos-a"
                },
                {
                    "id": "pos-b",
                    "strategyId": "strategy-b",
                    "symbol": "XLE",
                    "instrumentId": "XLE",
                    "quantity": 12.0,
                    "notional": 4000.0,
                    "sector": "Energy",
                    "assetClass": "etf",
                    "providerRef": "local-paper",
                    "approvedEvidenceRef": "journal://positions/pos-b"
                }
            ],
            "concentrationThresholds": [{
                "id": "sector-threshold",
                "metric": "notional_share",
                "groupBy": "sector",
                "maxShare": 0.55,
                "maxNotional": null,
                "severity": "warning",
                "userDefined": true
            }],
            "stressScenarios": [{
                "id": "stress-down-5",
                "label": "Down 5",
                "shocks": {"equityPct": -5.0},
                "sourceRef": "scenario://down-5"
            }],
            "replayRefs": ["replay://risk/snapshot-test"],
            "exportRefs": ["file://exports/risk/snapshot-test.json"]
        })
    }
}
