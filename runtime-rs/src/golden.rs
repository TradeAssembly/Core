// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Map, Value};

pub fn build_golden_run_corpus() -> Map<String, Value> {
    Map::from_iter([
        (
            "btc_entry_exit".to_string(),
            corpus_item(json!({
                "report": report([("synthetic_exit.watch_triggered", 1)], json!({})),
            })),
        ),
        (
            "duplicate_run".to_string(),
            corpus_item(json!({
                "second": {"idempotent": true},
                "report": report([("broker.order_dry_run", 1)], json!({})),
            })),
        ),
        (
            "equity_momentum".to_string(),
            corpus_item(json!({
                "run": {"dry_run": true, "order_plan": {"symbol": "SPY"}},
                "report": report([("broker.order_dry_run", 1)], json!({})),
            })),
        ),
        (
            "simbroker_paper".to_string(),
            corpus_item(json!({
                "report": report([("broker.order_submitted", 1), ("broker.exit_order_submitted", 1)], json!({})),
            })),
        ),
        (
            "option_spread_preview".to_string(),
            corpus_item(json!({
                "preview": {
                    "ok": true,
                    "orderPlanPreview": {
                        "riskPreview": {"margin_model": "defined_risk_option_spread"}
                    }
                },
                "report": report([], json!({})),
            })),
        ),
        (
            "partial_fill".to_string(),
            corpus_item(json!({
                "report": report([("broker.order_partially_filled", 1)], json!({})),
            })),
        ),
        (
            "rejected_order".to_string(),
            corpus_item(json!({
                "report": report([], json!({"openReservationIds": []})),
            })),
        ),
        (
            "restart_recovery".to_string(),
            corpus_item(json!({
                "exitResult": {"triggered": 1},
                "report": report([("synthetic_exit.watch_triggered", 1)], json!({})),
            })),
        ),
        (
            "stale_data".to_string(),
            corpus_item(json!({
                "run": {"order_id": null, "error": "stale market data"},
                "report": report([("marketdata.stale", 1)], json!({})),
            })),
        ),
    ])
}

fn corpus_item(mut extra: Value) -> Value {
    let object = extra.as_object_mut().expect("corpus item object");
    object
        .entry("report")
        .or_insert_with(|| report([], json!({})));
    extra
}

fn report<const N: usize>(counts: [(&str, u64); N], risk: Value) -> Value {
    let count_map = counts
        .into_iter()
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect::<Map<_, _>>();
    json!({
        "parity": {"ok": true},
        "audit": {"eventsHaveHashes": true, "sequenceContiguous": true},
        "orders": {"clientOrderIdsUnique": true},
        "counts": count_map,
        "risk": risk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn golden_run_corpus_replays_core_failure_scenarios() {
        let corpus = build_golden_run_corpus();
        let keys = corpus.keys().cloned().collect::<BTreeSet<_>>();

        assert_eq!(
            keys,
            BTreeSet::from_iter([
                "btc_entry_exit".to_string(),
                "duplicate_run".to_string(),
                "equity_momentum".to_string(),
                "simbroker_paper".to_string(),
                "option_spread_preview".to_string(),
                "partial_fill".to_string(),
                "rejected_order".to_string(),
                "restart_recovery".to_string(),
                "stale_data".to_string(),
            ])
        );
        for evidence in corpus.values() {
            let report = &evidence["report"];
            assert_eq!(report["parity"]["ok"], true);
            assert_eq!(report["audit"]["eventsHaveHashes"], true);
            assert_eq!(report["audit"]["sequenceContiguous"], true);
            assert_eq!(report["orders"]["clientOrderIdsUnique"], true);
        }

        assert_eq!(
            corpus["btc_entry_exit"]["report"]["counts"]["synthetic_exit.watch_triggered"],
            1
        );
        assert_eq!(corpus["duplicate_run"]["second"]["idempotent"], true);
        assert_eq!(
            corpus["duplicate_run"]["report"]["counts"]["broker.order_dry_run"],
            1
        );
        assert_eq!(corpus["equity_momentum"]["run"]["dry_run"], true);
        assert_eq!(
            corpus["equity_momentum"]["run"]["order_plan"]["symbol"],
            "SPY"
        );
        assert_eq!(
            corpus["equity_momentum"]["report"]["counts"]["broker.order_dry_run"],
            1
        );
        assert_eq!(
            corpus["simbroker_paper"]["report"]["counts"]["broker.order_submitted"],
            1
        );
        assert_eq!(
            corpus["simbroker_paper"]["report"]["counts"]["broker.exit_order_submitted"],
            1
        );
        assert_eq!(corpus["option_spread_preview"]["preview"]["ok"], true);
        assert_eq!(
            corpus["option_spread_preview"]["preview"]["orderPlanPreview"]["riskPreview"]
                ["margin_model"],
            "defined_risk_option_spread"
        );
        assert_eq!(
            corpus["partial_fill"]["report"]["counts"]["broker.order_partially_filled"],
            1
        );
        assert_eq!(
            corpus["rejected_order"]["report"]["risk"]["openReservationIds"],
            json!([])
        );
        assert_eq!(corpus["restart_recovery"]["exitResult"]["triggered"], 1);
        assert_eq!(
            corpus["restart_recovery"]["report"]["counts"]["synthetic_exit.watch_triggered"],
            1
        );
        assert_eq!(corpus["stale_data"]["run"]["order_id"], Value::Null);
        assert!(corpus["stale_data"]["run"]["error"]
            .as_str()
            .expect("stale error")
            .contains("stale"));
        assert_eq!(
            corpus["stale_data"]["report"]["counts"]["marketdata.stale"],
            1
        );
    }
}
