use super::TradeAssemblyService;
use crate::ports::{
    AuthorityContext, ComparePutOutcome, IdempotencyKey, ImmutablePutOutcome, SideEffectContext,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const POSITIONS_NS: &str = "positions";
const MAX_WRITE_ATTEMPTS: usize = 8;
const QUANTITY_EPSILON: f64 = 1e-10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PositionApplyOutcome {
    Applied,
    Duplicate,
}

pub(crate) fn validate_order_transition(
    service: &TradeAssemblyService,
    order: &Value,
) -> Result<(), String> {
    if order["side"].as_str() != Some("sell") {
        return Ok(());
    }
    let quantity = positive_number(order, &["qty", "quantity"])?;
    let key = position_key(order)?;
    let available = service
        .runtime()
        .storage
        .get_json(POSITIONS_NS, &key)?
        .and_then(|position| position["qty"].as_f64())
        .unwrap_or(0.0);
    if quantity > available + QUANTITY_EPSILON {
        return Err("paper_position_insufficient_quantity".to_string());
    }
    Ok(())
}

pub(crate) fn apply_filled_order(
    service: &TradeAssemblyService,
    order: &Value,
) -> Result<PositionApplyOutcome, String> {
    let order_id = required_string(order, &["id", "order_id", "orderId"])?;
    let quantity = positive_number(order, &["filled_qty", "filledQuantity"])?;
    let fill_price = positive_number(order, &["fill_price", "fillPrice"])?;
    let side = required_string(order, &["side"])?;
    if !matches!(side.as_str(), "buy" | "sell") {
        return Err("paper_position_side_invalid".to_string());
    }
    let key = position_key(order)?;
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("position.apply:{order_id}"))
            .map_err(|_| "paper_position_idempotency_invalid".to_string())?,
    );

    for _ in 0..MAX_WRITE_ATTEMPTS {
        let existing = service.runtime().storage.get_json(POSITIONS_NS, &key)?;
        if existing.as_ref().is_some_and(|position| {
            position["appliedOrderRefs"]
                .as_array()
                .is_some_and(|refs| refs.iter().any(|value| value.as_str() == Some(&order_id)))
        }) {
            return Ok(PositionApplyOutcome::Duplicate);
        }
        let replacement = next_position(existing.as_ref(), order, quantity, fill_price, &key)?;
        let outcome = match existing {
            Some(expected) => service.runtime().storage.compare_and_put_json(
                POSITIONS_NS,
                &key,
                expected,
                replacement,
                &context,
            )?,
            None => match service.runtime().storage.put_json_if_absent(
                POSITIONS_NS,
                &key,
                replacement,
                &context,
            )? {
                ImmutablePutOutcome::Created => ComparePutOutcome::Updated,
                ImmutablePutOutcome::AlreadyPresent => ComparePutOutcome::Conflict,
            },
        };
        if outcome == ComparePutOutcome::Updated {
            return Ok(PositionApplyOutcome::Applied);
        }
    }
    Err("paper_position_update_conflict".to_string())
}

fn next_position(
    existing: Option<&Value>,
    order: &Value,
    quantity: f64,
    fill_price: f64,
    key: &str,
) -> Result<Value, String> {
    let order_id = required_string(order, &["id", "order_id", "orderId"])?;
    let mut applied_refs = existing
        .and_then(|value| value["appliedOrderRefs"].as_array())
        .cloned()
        .unwrap_or_default();
    let prior_quantity = existing
        .and_then(|value| value["qty"].as_f64())
        .unwrap_or(0.0);
    let prior_average = existing
        .and_then(|value| value["average_price"].as_f64())
        .unwrap_or(0.0);
    let prior_realized = existing
        .and_then(|value| value["realized_pnl"].as_f64())
        .unwrap_or(0.0);
    let side = order["side"].as_str().unwrap_or_default();
    let (next_quantity, average_price, realized_delta) = match side {
        "buy" => {
            let next_quantity = round10(prior_quantity + quantity);
            let average = round10(
                ((prior_quantity * prior_average) + (quantity * fill_price)) / next_quantity,
            );
            (next_quantity, average, 0.0)
        }
        "sell" if quantity <= prior_quantity + QUANTITY_EPSILON => {
            let remaining = round10((prior_quantity - quantity).max(0.0));
            (
                remaining,
                prior_average,
                round10((fill_price - prior_average) * quantity),
            )
        }
        "sell" => return Err("paper_position_insufficient_quantity".to_string()),
        _ => return Err("paper_position_side_invalid".to_string()),
    };
    applied_refs.push(json!(order_id));
    let mut entry_refs = existing
        .and_then(|value| value["entryOrderRefs"].as_array())
        .cloned()
        .unwrap_or_default();
    let mut exit_refs = existing
        .and_then(|value| value["exitOrderRefs"].as_array())
        .cloned()
        .unwrap_or_default();
    if side == "buy" {
        entry_refs.push(json!(order_id));
    } else {
        exit_refs.push(json!(order_id));
    }
    let status = if next_quantity <= QUANTITY_EPSILON {
        "closed"
    } else {
        "open"
    };
    let realized_pnl = round10(prior_realized + realized_delta);
    let mut position = json!({
        "schemaVersion": "tradeassembly.paper_position.v1",
        "id": format!("position_{key}"),
        "position_id": format!("position_{key}"),
        "activation_id": field(order, &["activation_id", "activationId"]),
        "strategy_id": field(order, &["strategy_id", "strategyId"]),
        "account_ref": account_scope(order),
        "symbol": field(order, &["symbol"]),
        "status": status,
        "side": "long",
        "qty": next_quantity,
        "average_price": average_price,
        "mark_price": fill_price,
        "realized_pnl": realized_pnl,
        "last_order_id": order_id,
        "sourceObservationRefs": merged_source_observation_refs(existing, order),
        "appliedOrderRefs": applied_refs,
        "entryOrderRefs": entry_refs,
        "exitOrderRefs": exit_refs,
    });
    if status == "closed" {
        position["closed_by_order_id"] = json!(order_id);
    }
    Ok(position)
}

fn position_key(order: &Value) -> Result<String, String> {
    let activation_id = required_string(order, &["activation_id", "activationId"])?;
    let symbol = required_string(order, &["symbol"])?;
    let account = account_scope(order);
    let digest = Sha256::digest(format!("{activation_id}\0{account}\0{symbol}").as_bytes());
    Ok(format!("{:x}", digest)[..24].to_string())
}

fn account_scope(order: &Value) -> String {
    order
        .get("account_ref")
        .or_else(|| order.get("accountRef"))
        .or_else(|| order.get("accountMode"))
        .and_then(Value::as_str)
        .unwrap_or("paper")
        .to_string()
}

fn merged_source_observation_refs(existing: Option<&Value>, order: &Value) -> Vec<Value> {
    let mut refs = existing
        .and_then(|value| value["sourceObservationRefs"].as_array())
        .cloned()
        .unwrap_or_default();
    if let Some(reference) = order
        .get("source_observation_id")
        .or_else(|| order.get("sourceObservationId"))
        .and_then(Value::as_str)
    {
        refs.push(json!(reference));
    }
    refs.extend(
        order
            .get("evidenceRefs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    );
    refs.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    refs.dedup();
    refs
}

fn positive_number(value: &Value, names: &[&str]) -> Result<f64, String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_f64))
        .filter(|number| number.is_finite() && *number > 0.0)
        .ok_or_else(|| "paper_position_number_invalid".to_string())
}

fn required_string(value: &Value, names: &[&str]) -> Result<String, String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "paper_position_scope_invalid".to_string())
}

fn field(value: &Value, names: &[&str]) -> Value {
    names
        .iter()
        .find_map(|name| value.get(*name))
        .cloned()
        .unwrap_or(Value::Null)
}

fn round10(value: f64) -> f64 {
    (value * 10_000_000_000.0).round() / 10_000_000_000.0
}

pub(crate) fn list(service: &TradeAssemblyService) -> Vec<Value> {
    let values = stored(service);
    if values.is_empty() {
        vec![sample_position()]
    } else {
        values
    }
}

pub(crate) fn stored(service: &TradeAssemblyService) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json(POSITIONS_NS)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

pub(crate) fn sync(service: &TradeAssemblyService) -> Vec<Value> {
    list(service)
}

fn sample_position() -> Value {
    json!({
        "id": "position-local-btc",
        "strategy_id": "strat_local_btc_demo",
        "symbol": "BTC/USD",
        "instrumentId": "BTC/USD",
        "status": "open",
        "qty": 0.0002,
        "notional": 18.5,
        "sector": "Crypto",
        "assetClass": "crypto",
        "providerRef": "sim",
        "approvedEvidenceRef": "journal://positions/position-local-btc"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(name: &str) -> String {
        format!(
            ".tradeassembly/test-paper-position-{name}-{}.db",
            std::process::id()
        )
    }

    fn order(
        id: &str,
        activation: &str,
        account: &str,
        symbol: &str,
        side: &str,
        quantity: f64,
        price: f64,
    ) -> Value {
        json!({
            "id": id,
            "order_id": id,
            "activation_id": activation,
            "strategy_id": "strategy-test",
            "account_ref": account,
            "symbol": symbol,
            "side": side,
            "qty": quantity,
            "filled_qty": quantity,
            "fill_price": price,
            "source_observation_id": format!("observation-{id}"),
            "evidenceRefs": [format!("evidence://{id}")],
        })
    }

    #[test]
    fn weighted_buys_partial_sell_and_duplicate_are_deterministic() {
        let path = db("weighted");
        let _ = std::fs::remove_file(&path);
        let service = TradeAssemblyService::test_local(&path);
        let first = order(
            "buy-1",
            "activation-a",
            "paper-a",
            "BTC/USD",
            "buy",
            2.0,
            100.0,
        );
        let second = order(
            "buy-2",
            "activation-a",
            "paper-a",
            "BTC/USD",
            "buy",
            1.0,
            130.0,
        );
        let sell = order(
            "sell-1",
            "activation-a",
            "paper-a",
            "BTC/USD",
            "sell",
            1.0,
            120.0,
        );

        assert_eq!(
            apply_filled_order(&service, &first),
            Ok(PositionApplyOutcome::Applied)
        );
        assert_eq!(
            apply_filled_order(&service, &second),
            Ok(PositionApplyOutcome::Applied)
        );
        assert_eq!(
            apply_filled_order(&service, &sell),
            Ok(PositionApplyOutcome::Applied)
        );
        assert_eq!(
            apply_filled_order(&service, &sell),
            Ok(PositionApplyOutcome::Duplicate)
        );

        let values = stored(&service);
        assert_eq!(values.len(), 1, "{values:#?}");
        let position = &values[0];
        assert_eq!(position["status"], "open");
        assert_eq!(position["qty"], 2.0);
        assert_eq!(position["average_price"], 110.0);
        assert_eq!(position["mark_price"], 120.0);
        assert_eq!(position["realized_pnl"], 10.0);
        assert_eq!(
            position["appliedOrderRefs"].as_array().map(Vec::len),
            Some(3)
        );
    }

    #[test]
    fn equal_sell_closes_and_restart_preserves_accounting() {
        let path = db("restart");
        let _ = std::fs::remove_file(&path);
        let service = TradeAssemblyService::test_local(&path);
        let buy = order(
            "buy",
            "activation-a",
            "paper-a",
            "BTC/USD",
            "buy",
            2.0,
            100.0,
        );
        let sell = order(
            "sell",
            "activation-a",
            "paper-a",
            "BTC/USD",
            "sell",
            2.0,
            125.0,
        );
        assert_eq!(
            apply_filled_order(&service, &buy),
            Ok(PositionApplyOutcome::Applied)
        );
        assert_eq!(
            apply_filled_order(&service, &sell),
            Ok(PositionApplyOutcome::Applied)
        );
        let before = stored(&service);
        assert_eq!(before[0]["status"], "closed");
        assert_eq!(before[0]["qty"], 0.0);
        assert_eq!(before[0]["realized_pnl"], 50.0);
        drop(service);

        let restarted = TradeAssemblyService::test_local(&path);
        assert_eq!(stored(&restarted), before);
    }

    #[test]
    fn scope_isolation_and_oversell_fail_closed() {
        let path = db("scope");
        let _ = std::fs::remove_file(&path);
        let service = TradeAssemblyService::test_local(&path);
        for value in [
            order("a", "activation-a", "paper-a", "BTC/USD", "buy", 1.0, 100.0),
            order("b", "activation-b", "paper-a", "BTC/USD", "buy", 2.0, 100.0),
            order("c", "activation-a", "paper-b", "BTC/USD", "buy", 3.0, 100.0),
            order("d", "activation-a", "paper-a", "ETH/USD", "buy", 4.0, 100.0),
        ] {
            assert_eq!(
                apply_filled_order(&service, &value),
                Ok(PositionApplyOutcome::Applied)
            );
        }
        assert_eq!(stored(&service).len(), 4);

        let oversell = order(
            "oversell",
            "activation-a",
            "paper-a",
            "BTC/USD",
            "sell",
            1.1,
            120.0,
        );
        assert_eq!(
            validate_order_transition(&service, &oversell),
            Err("paper_position_insufficient_quantity".to_string())
        );
        let before = stored(&service);
        assert_eq!(
            apply_filled_order(&service, &oversell),
            Err("paper_position_insufficient_quantity".to_string())
        );
        assert_eq!(stored(&service), before);
    }
}
