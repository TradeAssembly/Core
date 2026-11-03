// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Observe-only continuation of an immutable pre-dispatch cycle.
use super::{broker_recovery, execution, orders, positions, ServiceResponse, TradeAssemblyService};
use crate::ports::{
    AuthorityContext, ComparePutOutcome, IdempotencyKey, PluginOperationResponse,
    SideEffectContext, StorageExpectation, StorageWrite,
};
use serde_json::{json, Value};

const PENDING: &str = "execution_pending_original_cycles";
const OUTCOMES: &str = "execution_original_cycle_recoveries";

pub(super) fn recover(service: &TradeAssemblyService, body: Value) -> ServiceResponse {
    // Existing observer authentication revalidates owner/account/package/credential
    // generation, immutable intent, prepared request and C5 receipt before lookup.
    let observation = broker_recovery::recover(service, body.clone());
    if observation.status != 200 {
        return observation;
    }
    let key = body["originalIdempotencyKey"].as_str().unwrap_or_default();
    let recovery_key = body["recoveryIdempotencyKey"].as_str().unwrap_or_default();
    let receipt = match serde_json::from_value(observation.body["receipt"].clone()) {
        Ok(receipt) => receipt,
        Err(_) => return ServiceResponse::error(409, "original_cycle_receipt_invalid", false),
    };
    match finish(service, key, recovery_key, &receipt) {
        Ok(result) => ServiceResponse::ok(result),
        Err(_) => ServiceResponse::error(409, "original_cycle_recovery_unresolved", false),
    }
}

fn finish(
    service: &TradeAssemblyService,
    key: &str,
    recovery_key: &str,
    receipt: &PluginOperationResponse,
) -> Result<Value, String> {
    let storage = &service.runtime().storage;
    let pending = storage
        .get_json(PENDING, key)?
        .ok_or("pending_original_cycle_missing")?;
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: "authenticated_broker_observer".into(),
            surface: "original_cycle_recovery".into(),
            account_mode: required(&pending["run"], "mode")?.into(),
        },
        IdempotencyKey::new(format!("original-cycle-recovery:{recovery_key}"))?,
    );
    let command_ns = "execution_original_cycle_recovery_commands";
    let command = json!({"originalIdempotencyKey":key});
    if storage.put_json_if_absent(command_ns, recovery_key, command.clone(), &context)?
        == crate::ports::ImmutablePutOutcome::AlreadyPresent
        && storage.get_json(command_ns, recovery_key)? != Some(command)
    {
        return Err("original_cycle_recovery_command_conflict".into());
    }
    // A successful retry still passed the authenticated original observer above.
    if let Some(result) = storage.get_json(OUTCOMES, key)? {
        return Ok(result);
    }
    if pending["schemaVersion"] != "tradeassembly.pending_original_cycle.v1"
        || pending["originalIdempotencyKey"] != key
    {
        return Err("pending_original_cycle_invalid".into());
    }
    // The financial intent must bind this exact original state. Old snapshots
    // without this durable commitment cannot be upgraded into recoverable cycles.
    use sha2::{Digest, Sha256};
    let intent_key = format!("intent_{:x}", Sha256::digest(key.as_bytes()));
    let intent = storage
        .get_json("broker_order_intents", &intent_key)?
        .ok_or("original_intent_missing")?;
    if intent["pendingOriginalCycleDigest"] != crate::spec::canonical_hash(&pending)? {
        return Err("original_cycle_not_bound".into());
    }
    let run_id = required(&pending["run"], "executionRunId")?;
    let activation_id = required(&pending["run"], "activationId")?;
    let sequence = pending["body"]["sequence"]
        .as_u64()
        .ok_or("original_sequence_missing")?;
    let tick_id = required(&pending["body"], "tickId")?;
    let old_run = storage
        .get_json("execution_runs", run_id)?
        .ok_or("original_run_missing")?;
    let old_activation = storage
        .get_json("execution_activations", activation_id)?
        .ok_or("original_activation_missing")?;
    if old_run["cycle"].as_u64().unwrap_or(0).checked_add(1) != Some(sequence)
        || old_run["checkpoint"] != pending["run"]["checkpoint"]
        || old_run["strategySpecHash"] != pending["run"]["strategySpecHash"]
        || old_run["strategyVersionId"] != pending["run"]["strategyVersionId"]
        || old_run["capabilityGraphRevisionId"] != pending["run"]["capabilityGraphRevisionId"]
        || old_run["configId"] != pending["run"]["configId"]
        || old_run["mode"] != pending["run"]["mode"]
        || old_activation["strategySpecHash"] != pending["activation"]["strategySpecHash"]
        || intent["activationId"] != activation_id
    {
        return Err("original_cycle_context_changed".into());
    }
    let order_id = format!("order_{}", orders::slug(key));
    let mut order = storage
        .get_json("execution_orders", &order_id)?
        .ok_or("original_order_missing")?;
    let old_order = order.clone();
    if order["activation_id"] != activation_id
        || order["client_order_id"] != pending["orderIntent"]["clientOrderId"]
        || order["symbol"] != pending["orderIntent"]["symbol"]
        || order["side"] != pending["orderIntent"]["side"]
        || order["qty"] != pending["orderIntent"]["quantity"]
        || order["account_ref"] != intent["accountRef"]
        || order["strategy_id"] != pending["run"]["strategyId"]
    {
        return Err("original_order_context_changed".into());
    }
    let canonical = orders::canonical_order_receipt(
        receipt,
        required(&pending["orderIntent"], "clientOrderId")?,
        required(&pending["orderIntent"], "symbol")?,
        required(&pending["orderIntent"], "side")?,
        pending["orderIntent"]["quantity"]
            .as_f64()
            .ok_or("original_quantity_missing")?,
        false,
        None,
    )?;
    if receipt.reconciliation_required {
        return Err("original_receipt_unresolved".into());
    }
    order["status"] = json!(canonical.status);
    order["provider_order_id"] = json!(canonical.provider_order_id);
    order["filled_qty"] = json!(canonical.filled_quantity);
    order["fill_price"] = json!(canonical.fill_price);
    order["pluginReceipt"] = serde_json::to_value(receipt).map_err(|_| "receipt_invalid")?;
    let provenance = json!({"kind":"observed_original_cycle","originalIdempotencyKey":key,
        "recoveryIdempotencyKey":recovery_key,"receiptContentHash":receipt.content_hash,
        "evidenceRefs":receipt.evidence_refs,"schedulerConfirmed":false});
    order["recovery"] = provenance.clone();
    let order_attempt_key = format!("attempt_{order_id}");
    let old_order_attempt = storage
        .get_json("execution_order_attempts", &order_attempt_key)?
        .ok_or("original_order_attempt_missing")?;
    if old_order_attempt["order_id"] != order_id {
        return Err("original_order_attempt_changed".into());
    }
    let mut order_attempt = old_order_attempt.clone();
    order_attempt["status"] = json!("completed");
    order_attempt["recovery"] = provenance.clone();
    let mut writes = Vec::new();
    let mut expectations = vec![
        StorageExpectation::new(OUTCOMES, key, None),
        StorageExpectation::new(PENDING, key, Some(pending.clone())),
        StorageExpectation::new("execution_runs", run_id, Some(old_run.clone())),
        StorageExpectation::new(
            "execution_activations",
            activation_id,
            Some(old_activation.clone()),
        ),
        StorageExpectation::new("execution_orders", &order_id, Some(old_order)),
        StorageExpectation::new(
            "execution_order_attempts",
            &order_attempt_key,
            Some(old_order_attempt),
        ),
        StorageExpectation::new("broker_order_intents", &intent_key, Some(intent.clone())),
    ];
    if canonical.filled_quantity > 0.0 {
        let position_key = positions::position_key(&order)?;
        let existing = storage.get_json("positions", &position_key)?;
        if existing.as_ref().is_some_and(|value| {
            value["appliedOrderRefs"]
                .as_array()
                .is_some_and(|refs| refs.contains(&json!(order_id)))
        }) {
            return Err("original_position_already_applied_without_cycle".into());
        }
        let next = positions::next_position(
            existing.as_ref(),
            &order,
            canonical.filled_quantity,
            canonical.fill_price.ok_or("original_fill_price_missing")?,
            &position_key,
        )?;
        expectations.push(StorageExpectation::new(
            "positions",
            &position_key,
            existing,
        ));
        writes.push(StorageWrite::new(
            "positions",
            position_key,
            next,
            context.clone(),
        ));
    }
    let order_run_id = format!("run_{}", orders::slug(&order_id));
    let order_result = json!({"id":order_run_id,"run_id":order_run_id,"status":"completed","order_id":order_id,"entry_order_id":order_id,
        "activation_id":activation_id,"correlationId":pending["run"]["correlationId"],
        "submit_orders":true,"submit_exit":false,"duplicate":false,"order":order,"recovery":provenance});
    let signed_quantity = (canonical.filled_quantity * 1_000_000.0).round() as i64
        * if pending["orderIntent"]["side"] == "sell" {
            -1
        } else {
            1
        };
    let position = pending["run"]["paperPositionMicros"]
        .as_i64()
        .unwrap_or(0)
        .checked_add(signed_quantity)
        .ok_or("position_overflow")?;
    let checkpoint = execution::checkpoint(
        activation_id,
        pending["run"]["correlationId"].as_str().unwrap_or_default(),
        sequence,
        pending["run"]["checkpointHash"].as_str(),
        "original_cycle_observed_recovery",
        json!({"activeWatches":[],"pendingOrders":[],"paperPositionMicros":position,
            "sourceObservationId":pending["sourceObservationId"],"sourceObservedAtMs":pending["observedAtMs"],"recovery":provenance}),
    );
    let mut run = old_run;
    run["cycle"] = json!(sequence);
    run["state"] = json!("stopped");
    run["stateReason"] = json!("original_cycle_observed_recovery");
    run["lastDecision"] = pending["decision"].clone();
    run["lastOrderResult"] = order_result.clone();
    run["paperPositionMicros"] = json!(position);
    run["checkpoint"] = checkpoint.clone();
    run["checkpointId"] = checkpoint["checkpointId"].clone();
    run["checkpointHash"] = checkpoint["checkpointHash"].clone();
    run["recovery"] = provenance.clone();
    let mut activation = old_activation;
    activation["state"] = json!("stopped");
    activation["stateReason"] = run["stateReason"].clone();
    activation["lastCompletedSequence"] = json!(sequence);
    activation["checkpointId"] = checkpoint["checkpointId"].clone();
    activation["checkpointHash"] = checkpoint["checkpointHash"].clone();
    activation["recovery"] = provenance.clone();
    let tick = json!({"schemaVersion":"tradeassembly.evaluation_tick.v1","kind":"EvaluationTickRecord",
        "activationId":activation_id,"tickId":tick_id,"sequence":sequence,"attemptId":pending["body"]["attemptId"],
        "strategyVersionId":run["strategyVersionId"],"strategySpecHash":run["strategySpecHash"],
        "sourceObservationIds":[pending["sourceObservationId"]],"decision":pending["decision"],"orderIntent":pending["orderIntent"],
        "orderResult":order_result,"priorCheckpointId":pending["run"]["checkpointId"],
        "checkpointId":checkpoint["checkpointId"],"status":"completed","recovery":provenance});
    let result = json!({"state":"recovered","activationState":"stopped","activationId":activation_id,
        "tickId":tick_id,"sequence":sequence,"originalIdempotencyKey":key,
        "cycleRecovery":"observed_original_cycle","schedulerConfirmed":false,"receipt":receipt,
        "checkpoint":checkpoint,"orderResult":order_result,"recovery":provenance});
    let mut add =
        |ns: &str, k: &str, v: Value| writes.push(StorageWrite::new(ns, k, v, context.clone()));
    add("execution_orders", &order_id, order);
    add(
        "execution_order_attempts",
        &order_attempt_key,
        order_attempt,
    );
    add("execution_runs", &order_run_id, order_result.clone());
    add("execution_idempotency", key, order_result);
    add("execution_runs", run_id, run);
    add("execution_activations", activation_id, activation);
    add(
        "execution_checkpoints",
        required(&checkpoint, "checkpointId")?,
        checkpoint.clone(),
    );
    add(
        "execution_ticks",
        &format!("{}:{sequence}", orders::slug(activation_id)),
        tick,
    );
    if let Some(attempt_id) = pending["attempt"]["attemptId"].as_str() {
        let old = storage
            .get_json("execution_attempts", attempt_id)?
            .ok_or("original_attempt_missing")?;
        if old["fencingToken"] != pending["attempt"]["fencingToken"]
            || old["cycle"] != json!(sequence)
        {
            return Err("original_attempt_changed".into());
        }
        let mut attempt = old.clone();
        attempt["state"] = json!("completed");
        attempt["completedAtMs"] = json!(receipt.observed_at_ms);
        attempt["recovery"] = provenance.clone();
        expectations.push(StorageExpectation::new(
            "execution_attempts",
            attempt_id,
            Some(old),
        ));
        add("execution_attempts", attempt_id, attempt);
    }
    let scheduler = storage.get_json("scheduler_state", "current")?;
    if let Some(mut value) = scheduler.clone().filter(|value| {
        value["activationId"] == activation_id || value["activation_id"] == activation_id
    }) {
        value["state"] = json!("stopped");
        value["running"] = json!(false);
        value["enabled"] = json!(false);
        value["lastObservedRecoveredCycle"] = json!(sequence);
        value["recovery"] = provenance;
        expectations.push(StorageExpectation::new(
            "scheduler_state",
            "current",
            scheduler,
        ));
        add("scheduler_state", "current", value);
    }
    add(OUTCOMES, key, result.clone());
    if storage.put_json_batch(&writes, &expectations)? != ComparePutOutcome::Updated {
        return Err("original_cycle_concurrent_change".into());
    }
    Ok(result)
}
fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "original_cycle_field_missing".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn fixture(service: &TradeAssemblyService) -> PluginOperationResponse {
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("fixture").unwrap(),
        );
        let run = json!({"executionRunId":"run_original","activationId":"original","cycle":0,
            "state":"active","mode":"paper","strategyId":"strategy","strategyVersionId":"version",
            "strategySpecHash":"strategy-hash","capabilityGraphRevisionId":"graph","configId":"config",
            "checkpoint":{"checkpointId":"prior","checkpointHash":"prior-hash"},"checkpointId":"prior","checkpointHash":"prior-hash","paperPositionMicros":0});
        let activation = run.clone();
        let order = json!({"id":"order_original_order","order_id":"order_original_order","activation_id":"original",
            "strategy_id":"strategy","client_order_id":"client-original","symbol":"BTC/USD","side":"buy","qty":1.0,
            "account_ref":"account","status":"submission_pending"});
        let pending = json!({"schemaVersion":"tradeassembly.pending_original_cycle.v1","originalIdempotencyKey":"original-order",
            "run":run,"activation":activation,"body":{"sequence":1,"tickId":"original-tick","attemptId":"original-attempt","fencingToken":7},
            "decision":{"kind":"order_intent"},"orderIntent":{"clientOrderId":"client-original","symbol":"BTC/USD","side":"buy","quantity":1.0,"quantityMicros":1000000},
            "sourceObservationId":"original-observation","observedAtMs":1,"attempt":{"attemptId":"original-attempt","cycle":1,"fencingToken":7}});
        let intent = json!({"activationId":"original","accountRef":"account","pendingOriginalCycleDigest":crate::spec::canonical_hash(&pending).unwrap()});
        for (ns, key, value) in [
            (PENDING, "original-order".into(), pending),
            (
                "broker_order_intents",
                format!("intent_{:x}", Sha256::digest(b"original-order")),
                intent,
            ),
            ("execution_runs", "run_original".into(), run),
            ("execution_activations", "original".into(), activation),
            ("execution_orders", "order_original_order".into(), order),
            (
                "execution_order_attempts",
                "attempt_order_original_order".into(),
                json!({"id":"attempt_order_original_order","order_id":"order_original_order","activation_id":"original","status":"submission_pending"}),
            ),
            (
                "execution_attempts",
                "original-attempt".into(),
                json!({"attemptId":"original-attempt","cycle":1,"fencingToken":7,"state":"running"}),
            ),
            (
                "scheduler_state",
                "current".into(),
                json!({"activationId":"original","state":"running","lastCompletedCycle":0}),
            ),
        ] {
            service
                .runtime()
                .storage
                .put_json(ns, &key, value, &context)
                .unwrap();
        }
        serde_json::from_value(json!({"correlationId":"original","schemaRef":"schema://broker/order-receipt@1",
            "payload":{"providerOrderId":"provider-original","clientOrderId":"client-original","symbol":"BTC/USD","side":"buy","quantity":1.0,"filledQuantity":1.0,"fillPrice":12.5,"status":"filled"},
            "observedAtMs":2,"sourceEventId":"lookup-original","contentHash":"lookup-hash","freshnessState":"fresh",
            "evidenceRefs":["broker-recovery:recovery"],"deterministic":false,"replayable":true,
            "providerOutcomeId":"provider-original","reconciliationRequired":false})).unwrap()
    }
    #[test]
    fn original_cycle_atomic_failure_restart_and_repeat_apply_accounting_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let db = root.join("runtime.db");
        let service = TradeAssemblyService::test_local(db.to_str().unwrap());
        let receipt = fixture(&service);
        let snapshot = root.join("admitted.sqlite");
        service
            .runtime()
            .storage
            .execution_checkpoint_snapshot(&snapshot)
            .unwrap();
        let restored = root.join("restored.db");
        drop(TradeAssemblyService::test_local(restored.to_str().unwrap()));
        std::fs::copy(snapshot, &restored).unwrap();
        let service = TradeAssemblyService::test_local(restored.to_str().unwrap());
        let conn = rusqlite::Connection::open(&restored).unwrap();
        conn.execute_batch("CREATE TRIGGER recovery_fault BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='execution_original_cycle_recoveries' BEGIN SELECT RAISE(ABORT,'controlled interruption'); END;").unwrap();
        assert!(finish(&service, "original-order", "recovery", &receipt).is_err());
        assert!(service
            .runtime()
            .storage
            .list_json("positions")
            .unwrap()
            .is_empty());
        assert_eq!(
            service
                .runtime()
                .storage
                .get_json("execution_runs", "run_original")
                .unwrap()
                .unwrap()["cycle"],
            0
        );
        conn.execute_batch("DROP TRIGGER recovery_fault").unwrap();
        let first = finish(&service, "original-order", "recovery", &receipt).unwrap();
        assert_eq!(first["state"], "recovered");
        assert_eq!(first["schedulerConfirmed"], false);
        assert_eq!(
            first,
            finish(&service, "original-order", "recovery", &receipt).unwrap()
        );
        assert_eq!(
            first,
            finish(&service, "original-order", "another-recovery", &receipt).unwrap()
        );
        let positions = service.runtime().storage.list_json("positions").unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].1["qty"], 1.0);
        assert_eq!(
            positions[0].1["appliedOrderRefs"].as_array().unwrap().len(),
            1
        );
        let run = service
            .runtime()
            .storage
            .get_json("execution_runs", "run_original")
            .unwrap()
            .unwrap();
        assert_eq!(run["cycle"], 1);
        assert_eq!(run["state"], "stopped");
        assert_eq!(run["paperPositionMicros"], 1000000);
        assert_eq!(
            service
                .runtime()
                .storage
                .list_json("execution_ticks")
                .unwrap()
                .len(),
            1
        );
        let state = service
            .runtime()
            .storage
            .get_json("scheduler_state", "current")
            .unwrap()
            .unwrap();
        assert_eq!(state["state"], "stopped");
        assert_eq!(state["lastCompletedCycle"], 0);
        assert_eq!(state["lastObservedRecoveredCycle"], 1);
    }
    #[test]
    fn original_cycle_rejects_changed_state_and_receipt_without_writes() {
        for field in ["clientOrderId", "symbol", "side", "quantity"] {
            let temp = tempfile::tempdir().unwrap();
            let db = temp.path().join("runtime.db");
            let service = TradeAssemblyService::test_local(db.to_str().unwrap());
            let mut receipt = fixture(&service);
            receipt.payload[field] = if field == "quantity" {
                json!(2.0)
            } else {
                json!("wrong")
            };
            assert!(
                finish(&service, "original-order", "recovery", &receipt).is_err(),
                "{field}"
            );
            assert!(service
                .runtime()
                .storage
                .list_json("positions")
                .unwrap()
                .is_empty());
            assert!(service
                .runtime()
                .storage
                .list_json(OUTCOMES)
                .unwrap()
                .is_empty());
        }
        for (namespace, key, field) in [
            (PENDING, "original-order", "decision"),
            ("execution_orders", "order_original_order", "account_ref"),
            ("execution_attempts", "original-attempt", "fencingToken"),
            ("execution_runs", "run_original", "strategySpecHash"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let db = temp.path().join("runtime.db");
            let service = TradeAssemblyService::test_local(db.to_str().unwrap());
            let receipt = fixture(&service);
            let storage = &service.runtime().storage;
            let mut value = storage.get_json(namespace, key).unwrap().unwrap();
            value[field] = json!("changed");
            storage
                .put_json(
                    namespace,
                    key,
                    value,
                    &SideEffectContext::new(
                        AuthorityContext::local_cli(),
                        IdempotencyKey::new("corrupt-fixture").unwrap(),
                    ),
                )
                .unwrap();
            assert!(
                finish(&service, "original-order", "recovery", &receipt).is_err(),
                "{namespace}"
            );
            assert!(storage.list_json("positions").unwrap().is_empty());
        }
    }
}
