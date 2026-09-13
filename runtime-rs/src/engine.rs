// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

pub fn btc_exit_demo_spec() -> Value {
    json!({
        "strategy_id": "strat_local_btc_demo",
        "symbol": "BTC/USD",
        "asset_class": "crypto",
        "market_requirements": {"asset_class": "crypto", "instrument_family": "crypto_asset"}
    })
}

pub fn apply_dynamic_sizing(
    legs: &[Value],
    sizing: &Value,
    snapshot: &Value,
    quantity_precision: u32,
    metadata: Option<&Value>,
) -> Vec<Value> {
    legs.iter()
        .map(|leg| {
            let mut sized = leg.clone();
            if sized.get("qty").is_some() {
                return sized;
            }
            let symbol = sized["symbol"].as_str().unwrap_or("");
            let Some(price) = latest_price(symbol, snapshot) else {
                return sized;
            };
            let notional = sizing["notional"].as_f64().or_else(|| {
                let pct = sizing["notional_pct"].as_f64()?;
                let basis = sizing["basis"].as_str().unwrap_or("equity");
                let account = metadata?["account"][basis].as_f64()?;
                Some(account * pct / 100.0)
            });
            if let Some(notional) = notional {
                let raw_qty = notional / price;
                let qty = if let Some(step) = sizing["qty_step"].as_f64() {
                    (raw_qty / step).floor() * step
                } else {
                    let factor = 10_f64.powi(quantity_precision as i32);
                    (raw_qty * factor).floor() / factor
                };
                sized["qty"] = json!(qty);
            }
            sized
        })
        .collect()
}

pub fn decision_engine_handle_trigger(trigger: &Value) -> Value {
    prepare_order_intent(
        trigger["activation_id"].as_str().unwrap_or_default(),
        trigger["idempotency_key"].as_str(),
    )
}

pub fn decision_runner_handle_trigger(trigger: &Value) -> Value {
    prepare_order_intent(
        trigger["activation_id"].as_str().unwrap_or_default(),
        trigger["idempotency_key"].as_str(),
    )
}

pub fn prepare_order_intent(activation_id: &str, idempotency_key: Option<&str>) -> Value {
    json!({
        "activation_id": activation_id,
        "idempotency_key": idempotency_key,
        "prepared": true
    })
}

pub fn run_once_runner(
    activation_id: &str,
    submit_exit: bool,
    submit_orders: bool,
    idempotency_key: Option<&str>,
) -> Value {
    json!({
        "activation_id": activation_id,
        "submit_exit": submit_exit,
        "submit_orders": submit_orders,
        "idempotency_key": idempotency_key,
        "status": "completed"
    })
}

pub fn run_once(activation_id: &str, submit_orders: bool) -> Value {
    run_once_runner(activation_id, false, submit_orders, None)
}

fn latest_price(symbol: &str, snapshot: &Value) -> Option<f64> {
    let quote = &snapshot["quotes"][symbol];
    quote["last"]
        .as_f64()
        .or_else(|| Some((quote["bid"].as_f64()? + quote["ask"].as_f64()?) / 2.0))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn engine_top_level_demo_spec_and_dynamic_sizing_match_source_shape() {
        let spec = super::btc_exit_demo_spec();
        let sized = super::apply_dynamic_sizing(
            &[json!({"symbol": "BTC/USD", "side": "buy"})],
            &json!({"notional": 25, "rounding": "floor"}),
            &json!({"quotes": {"BTC/USD": {"last": 43100}}}),
            4,
            None,
        );

        assert_eq!(spec["symbol"], "BTC/USD");
        assert_eq!(sized[0]["qty"], 0.0005);
        assert!(sized[0]["side"] == "buy");
    }

    #[test]
    fn dynamic_sizing_preserves_existing_quantity_and_uses_account_percent() {
        let preserved = super::apply_dynamic_sizing(
            &[json!({"symbol": "SPY", "qty": 3})],
            &json!({"qty": 10}),
            &json!({}),
            4,
            None,
        );
        let percent = super::apply_dynamic_sizing(
            &[json!({"symbol": "SPY"})],
            &json!({"notional_pct": 10, "basis": "equity", "qty_step": 0.01}),
            &json!({"quotes": {"SPY": {"bid": 499, "ask": 501}}}),
            4,
            Some(&json!({"account": {"equity": 10000, "buying_power": 5000}})),
        );

        assert_eq!(preserved, vec![json!({"symbol": "SPY", "qty": 3})]);
        assert_eq!(percent[0]["qty"], 2.0);
    }

    #[test]
    fn decision_and_run_once_helpers_delegate_to_runtime_service_shape() {
        assert_eq!(
            super::decision_engine_handle_trigger(&json!({"activation_id": "act-0"}))["prepared"],
            true
        );
        assert_eq!(
            super::decision_runner_handle_trigger(
                &json!({"activation_id": "act-1", "idempotency_key": "run-1"})
            ),
            json!({"activation_id": "act-1", "idempotency_key": "run-1", "prepared": true})
        );
        assert_eq!(super::prepare_order_intent("act-2", None)["prepared"], true);
        assert_eq!(
            super::run_once_runner("act-1", true, true, Some("run-1")),
            json!({
                "activation_id": "act-1",
                "submit_exit": true,
                "submit_orders": true,
                "idempotency_key": "run-1",
                "status": "completed"
            })
        );
        assert_eq!(super::run_once("act-2", false)["submit_orders"], false);
    }
}
