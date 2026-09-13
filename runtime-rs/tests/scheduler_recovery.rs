use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tradeassembly_runtime::ports::{
    AuthorityContext, EventAppend, IdempotencyKey, OutboxRecord, ScheduledWork, SideEffectContext,
};
use tradeassembly_runtime::service::TradeAssemblyService;

fn db(name: &str) -> String {
    format!(".tradeassembly/test-gc14-{name}-{}.db", std::process::id())
}

fn activate(service: &TradeAssemblyService, suffix: &str) -> (String, Value) {
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "schedulerIntervalSeconds": 300,
        }),
    );
    assert_eq!(saved.status, 200, "{:#}", saved.body);
    let config_id = saved.body["body"]["configId"].as_str().expect("config id");
    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": format!("gc14-activation-{suffix}"),
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activated.status, 200, "{:#}", activated.body);
    (
        activated.body["body"]["activationId"]
            .as_str()
            .expect("activation id")
            .to_string(),
        activated.body["body"]["run"].clone(),
    )
}

fn storage_context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("idempotency key"),
    )
}

#[test]
fn checkpoint_before_ack_recovers_without_repeating_the_tick() {
    let db = db("checkpoint-recovery");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let (activation_id, run) = activate(&service, "checkpoint");
    let checkpoint_id = format!("checkpoint_{}_1", activation_id.replace('-', "_"));
    let checkpoint_state = json!({
        "activeWatches": ["BTC/USD"],
        "pendingOrders": [],
        "riskReservations": [],
    });
    let checkpoint_hash = format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({
                "activationId": activation_id,
                "cycle": 1,
                "previousHash": run["checkpointHash"],
                "state": checkpoint_state,
            }))
            .expect("checkpoint hash input")
        )
    );
    let checkpoint = json!({
        "schemaVersion": "tradeassembly.execution_checkpoint.v1",
        "checkpointId": checkpoint_id,
        "activationId": activation_id,
        "cycle": 1,
        "reason": "tick_completed",
        "state": checkpoint_state,
        "previousHash": run["checkpointHash"],
        "checkpointHash": checkpoint_hash,
    });
    service
        .runtime()
        .storage
        .put_json(
            "execution_checkpoints",
            &checkpoint_id,
            checkpoint,
            &storage_context("gc14-checkpoint-crash"),
        )
        .expect("persist crash checkpoint");
    let due_at_ms = service.runtime().clock.now_ms();
    let schedule_key = format!("execution:{activation_id}:cycle:1");
    service
        .runtime()
        .scheduler
        .schedule(ScheduledWork {
            schedule_key: schedule_key.clone(),
            queue: "execution-ticks".to_string(),
            payload: json!({"activationId": activation_id, "cycle": 1}),
            due_at_ms,
            idempotency_key: IdempotencyKey::new(format!("schedule:{schedule_key}"))
                .expect("schedule idempotency key"),
            fencing_token: 0,
        })
        .expect("persist crash schedule");
    let first_claim = service
        .runtime()
        .scheduler
        .claim_key(&schedule_key, "crashed-worker", due_at_ms, 0)
        .expect("claim crash schedule")
        .expect("crashed worker owns schedule");
    service
        .runtime()
        .events
        .append(EventAppend {
            stream: format!("execution-scheduler:{activation_id}"),
            event_type: "execution.tick.completed".to_string(),
            aggregate_id: activation_id.clone(),
            payload: json!({
                "activationId": first_claim.payload["activationId"],
                "cycle": first_claim.payload["cycle"],
                "scheduleKey": first_claim.schedule_key,
                "checkpointId": checkpoint_id,
                "checkpointHash": checkpoint_hash,
            }),
            idempotency_key: IdempotencyKey::new(format!(
                "execution-tick:{activation_id}:1:complete"
            ))
            .expect("completion idempotency key"),
            occurred_at_ms: first_claim.due_at_ms,
            retention_until_ms: None,
        })
        .expect("persist crash finalization event");

    let recovered = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "gc14-recovery-worker",
            "maxCycles": 1,
            "leaseTtlSeconds": 1,
            "idempotencyKey": "gc14-recover-checkpoint",
        }),
    );
    assert_eq!(recovered.status, 200, "{:#}", recovered.body);
    assert_eq!(
        recovered.body["body"]["ticks"][0]["status"], "recovered",
        "{:#}",
        recovered.body
    );
    assert_eq!(
        recovered.body["body"]["ticks"][0]["reason"],
        "checkpoint_already_completed"
    );

    let reopened = TradeAssemblyService::test_local(&db);
    let workspace = reopened.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    assert_eq!(workspace.body["activeRun"]["cycle"], 1);
    let decision_count = workspace.body["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| {
            event["eventType"] == "decision.order_intent"
                || event["eventType"] == "decision.no_trade"
        })
        .count();
    assert_eq!(decision_count, 0);

    let stream = format!("execution-scheduler:{activation_id}");
    let events = reopened
        .runtime()
        .events
        .replay(&stream, 0, 10)
        .expect("replay scheduler events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "execution.tick.completed");
    assert!(!reopened
        .runtime()
        .inbox
        .record_once(
            "execution-scheduler",
            &format!("execution:{activation_id}:cycle:1"),
            None,
            reopened.runtime().clock.now_ms(),
        )
        .expect("completed inbox record"));

    let now = reopened.runtime().clock.now_ms();
    let first_outbox = reopened
        .runtime()
        .outbox
        .claim_pending("publisher-a", now, 10)
        .expect("claim recovered tick outbox")
        .into_iter()
        .find(|record| record.topic == "execution.tick.completed")
        .expect("recovered tick outbox");
    drop(reopened);
    let restarted = TradeAssemblyService::test_local(&db);
    let replacement = restarted
        .runtime()
        .outbox
        .claim_pending("publisher-b", now + 30_001, 10)
        .expect("reclaim recovered tick outbox")
        .into_iter()
        .find(|record| record.outbox_id == first_outbox.outbox_id)
        .expect("replacement outbox claim");
    assert!(replacement.fencing_token > first_outbox.fencing_token);
    assert!(restarted
        .runtime()
        .outbox
        .mark_published(
            &first_outbox.outbox_id,
            "publisher-a",
            first_outbox.fencing_token,
        )
        .is_err());
    restarted
        .runtime()
        .outbox
        .mark_published(
            &replacement.outbox_id,
            "publisher-b",
            replacement.fencing_token,
        )
        .expect("publish replacement outbox claim");
    let _ = std::fs::remove_file(db);
}

#[test]
fn exact_schedule_and_logical_lease_claims_are_fenced() {
    let db = db("fencing");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let now = service.runtime().clock.now_ms();
    let schedule_key = "execution:activation-fence:cycle:1";
    service
        .runtime()
        .scheduler
        .schedule(ScheduledWork {
            schedule_key: schedule_key.to_string(),
            queue: "execution-ticks".to_string(),
            payload: json!({"activationId": "activation-fence", "cycle": 1}),
            due_at_ms: now,
            idempotency_key: IdempotencyKey::new("schedule-fence").expect("schedule key"),
            fencing_token: 0,
        })
        .expect("schedule work");
    let first = service
        .runtime()
        .scheduler
        .claim_key(schedule_key, "worker-a", now, 10)
        .expect("first claim")
        .expect("first owner");
    assert!(service
        .runtime()
        .scheduler
        .claim_key(schedule_key, "worker-b", now + 9, 10)
        .expect("contended claim")
        .is_none());
    let replacement = service
        .runtime()
        .scheduler
        .claim_key(schedule_key, "worker-b", now + 11, 10)
        .expect("replacement claim")
        .expect("replacement owner");
    assert!(replacement.fencing_token > first.fencing_token);
    assert!(service
        .runtime()
        .scheduler
        .complete(schedule_key, "worker-a", first.fencing_token, now + 11)
        .is_err());
    service
        .runtime()
        .scheduler
        .complete(
            schedule_key,
            "worker-b",
            replacement.fencing_token,
            now + 11,
        )
        .expect("replacement completion");

    let resource = "execution-tick:activation-fence:1";
    let first_lease = service
        .runtime()
        .leases
        .acquire(resource, "worker-a", now, 10)
        .expect("first lease")
        .expect("first lease owner");
    let replacement_lease = service
        .runtime()
        .leases
        .acquire(resource, "worker-b", now + 11, 10)
        .expect("replacement lease")
        .expect("replacement lease owner");
    assert!(replacement_lease.fencing_token > first_lease.fencing_token);
    assert!(service.runtime().leases.release(&first_lease).is_err());
    service
        .runtime()
        .leases
        .release(&replacement_lease)
        .expect("release replacement lease");
    let _ = std::fs::remove_file(db);
}

#[test]
fn invalid_tick_delivery_remains_retryable_and_unacknowledged() {
    let db = db("retry");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let now = service.runtime().clock.now_ms();
    let schedule_key = "invalid-execution-tick";
    service
        .runtime()
        .scheduler
        .schedule(ScheduledWork {
            schedule_key: schedule_key.to_string(),
            queue: "unexpected-queue".to_string(),
            payload: json!({"activationId": "missing", "cycle": 1}),
            due_at_ms: now,
            idempotency_key: IdempotencyKey::new("invalid-tick").expect("schedule key"),
            fencing_token: 0,
        })
        .expect("schedule invalid work");
    let response = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "workerId": "invalid-worker",
            "maxCycles": 1,
            "leaseTtlSeconds": 1,
            "idempotencyKey": "gc14-invalid-delivery", // gitleaks:allow -- public invalid-delivery test key, not a credential
        }),
    );
    assert_eq!(response.status, 200, "{:#}", response.body);
    assert_eq!(response.body["body"]["ticks"][0]["status"], "failed");
    assert!(service
        .runtime()
        .inbox
        .record_once(
            "execution-scheduler",
            schedule_key,
            None,
            service.runtime().clock.now_ms(),
        )
        .expect("no completion inbox record"));
    let retry = service
        .runtime()
        .scheduler
        .claim_key(
            schedule_key,
            "retry-worker",
            service.runtime().clock.now_ms() + 2_000,
            1_000,
        )
        .expect("retry claim")
        .expect("retryable work");
    assert!(retry.fencing_token > 1);
    let _ = std::fs::remove_file(db);
}

#[test]
fn outbox_append_is_idempotent_by_tick_identity() {
    let db = db("outbox-idempotency");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let record = OutboxRecord {
        outbox_id: "outbox-execution-tick-1".to_string(),
        topic: "execution.tick.completed".to_string(),
        payload: json!({"activationId": "activation-1", "cycle": 1}),
        idempotency_key: IdempotencyKey::new("execution-tick-1").expect("outbox key"),
        created_at_ms: service.runtime().clock.now_ms(),
        fencing_token: 0,
    };
    service
        .runtime()
        .outbox
        .append(record.clone())
        .expect("first append");
    service
        .runtime()
        .outbox
        .append(record)
        .expect("duplicate append");
    assert_eq!(
        service
            .runtime()
            .outbox
            .claim_pending("publisher", service.runtime().clock.now_ms(), 10)
            .expect("claim outbox")
            .len(),
        1
    );
    let _ = std::fs::remove_file(db);
}
