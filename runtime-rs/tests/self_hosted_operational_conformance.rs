// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use postgres::{Client, NoTls};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use tradeassembly_runtime::ports::{
    verify_operational_ports, EventAppend, IdempotencyKey, OperationalPortSet, OutboxRecord,
    QueueRequest, ScheduledWork,
};
use tradeassembly_runtime::runtime_config::{
    RuntimeBuilder, RuntimeConfig, RuntimeConfigLayer, SecretResolver,
};

struct TestSecretResolver {
    postgres_url: String,
    nats_url: String,
}

impl SecretResolver for TestSecretResolver {
    fn resolve(&self, reference: &str) -> Result<String, String> {
        match reference {
            "env://TRADEASSEMBLY_TEST_POSTGRES_URL" => Ok(self.postgres_url.clone()),
            "env://TRADEASSEMBLY_TEST_NATS_URL" => Ok(self.nats_url.clone()),
            "env://TRADEASSEMBLY_TEST_WARDEN_TOKEN" => {
                Ok("self-hosted-conformance-token-reference-value".to_string())
            }
            _ => Err("test secret reference is unsupported".to_string()),
        }
    }
}

#[test]
#[ignore = "requires Docker-backed Postgres and NATS JetStream"]
fn self_hosted_operational_conformance_against_docker() {
    let config = RuntimeConfig::resolve(
        None,
        RuntimeConfigLayer::default(),
        RuntimeConfigLayer {
            profile: Some("self_hosted".to_string()),
            oidc_profile: Some("configured_oidc".to_string()),
            oidc_issuer: Some("https://identity.example".to_string()),
            oidc_audience: Some("tradeassembly".to_string()),
            oidc_client_id: Some("tradeassembly".to_string()),
            oidc_redirect_uri: Some("http://127.0.0.1:8976/callback".to_string()),
            warden_token_ref: Some("env://TRADEASSEMBLY_TEST_WARDEN_TOKEN".to_string()),
            postgres_url_ref: Some("env://TRADEASSEMBLY_TEST_POSTGRES_URL".to_string()),
            nats_url_ref: Some("env://TRADEASSEMBLY_TEST_NATS_URL".to_string()),
            object_store_endpoint: Some(
                std::env::var("TRADEASSEMBLY_TEST_ARTIFACT_ROOT").unwrap_or_else(|_| {
                    "file:///tmp/tradeassembly-self-hosted-artifacts".to_string()
                }),
            ),
            ..RuntimeConfigLayer::default()
        },
    )
    .expect("self-hosted config");
    let (runtime, manifest) = RuntimeBuilder::new(config.clone())
        .build()
        .expect("self-hosted runtime");
    assert_eq!(manifest.profile, "self_hosted");
    let report = verify_operational_ports(OperationalPortSet {
        queue: runtime.queue.as_ref(),
        events: runtime.events.as_ref(),
        scheduler: runtime.scheduler.as_ref(),
        outbox: runtime.outbox.as_ref(),
        inbox: runtime.inbox.as_ref(),
        leases: runtime.leases.as_ref(),
        evidence: runtime.evidence.as_ref(),
        telemetry: runtime.telemetry.as_ref(),
    })
    .expect("self-hosted conformance");
    assert_eq!(report.checks.len(), 8);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_millis() as i64;
    let fixture = format!("restart-{now}");
    let queue = format!("{fixture}-queue");
    let message_id = runtime
        .queue
        .enqueue(QueueRequest {
            queue: queue.clone(),
            payload: json!({"fixtureId": fixture}),
            idempotency_key: IdempotencyKey::new(format!("{fixture}-queue-key"))
                .expect("queue key"),
            partition_key: Some(fixture.clone()),
            priority: 1,
            available_at_ms: now,
            retention_until_ms: now + 60_000,
            max_attempts: 3,
        })
        .expect("enqueue restart fixture");
    let stream = format!("{fixture}-stream");
    let event = runtime
        .events
        .append(EventAppend {
            stream: stream.clone(),
            event_type: "conformance.restart".to_string(),
            aggregate_id: fixture.clone(),
            payload: json!({"fixtureId": fixture}),
            idempotency_key: IdempotencyKey::new(format!("{fixture}-event-key"))
                .expect("event key"),
            occurred_at_ms: now,
            retention_until_ms: Some(now + 60_000),
        })
        .expect("append restart fixture");
    drop(runtime);

    let (restarted, restarted_manifest) = RuntimeBuilder::new(config.clone())
        .build()
        .expect("restart self-hosted runtime");
    assert_eq!(restarted_manifest.profile, "self_hosted");
    let claimed = restarted
        .queue
        .claim(&queue, "restart-worker", now + 1, 1_000, 1)
        .expect("claim persisted queue message");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].message_id, message_id);
    restarted
        .queue
        .acknowledge(&message_id, claimed[0].fencing_token)
        .expect("ack persisted queue message");
    assert_eq!(
        restarted
            .events
            .replay(&stream, 0, 10)
            .expect("replay persisted event"),
        vec![event.clone()]
    );
    force_event_publication_pending(&event.event_id);
    let (event_recovery_runtime, _) = RuntimeBuilder::new(config.clone())
        .build()
        .expect("event publication recovery runtime");
    assert_event_published(&event.event_id);
    drop(event_recovery_runtime);

    let colliding_suffix = format!("collision-{now}");
    let queue_with_dash = format!("a-b-{colliding_suffix}");
    let queue_with_underscore = format!("a_b-{colliding_suffix}");
    for (queue_name, marker) in [
        (&queue_with_underscore, "underscore"),
        (&queue_with_dash, "dash"),
    ] {
        restarted
            .queue
            .enqueue(QueueRequest {
                queue: queue_name.clone(),
                payload: json!({"marker": marker}),
                idempotency_key: IdempotencyKey::new(format!("{colliding_suffix}-{marker}"))
                    .expect("collision queue key"),
                partition_key: None,
                priority: 1,
                available_at_ms: now,
                retention_until_ms: now + 60_000,
                max_attempts: 3,
            })
            .expect("enqueue collision fixture");
    }
    let dash_delivery = restarted
        .queue
        .claim(&queue_with_dash, "dash-worker", now + 2, 1_000, 1)
        .expect("claim dash queue");
    assert_eq!(dash_delivery.len(), 1);
    assert_eq!(dash_delivery[0].queue, queue_with_dash);
    assert_eq!(dash_delivery[0].payload, json!({"marker": "dash"}));
    restarted
        .queue
        .acknowledge(&dash_delivery[0].message_id, dash_delivery[0].fencing_token)
        .expect("ack dash queue");
    let underscore_delivery = restarted
        .queue
        .claim(
            &queue_with_underscore,
            "underscore-worker",
            now + 2,
            1_000,
            1,
        )
        .expect("claim underscore queue");
    assert_eq!(underscore_delivery.len(), 1);
    assert_eq!(underscore_delivery[0].queue, queue_with_underscore);
    assert_eq!(
        underscore_delivery[0].payload,
        json!({"marker": "underscore"})
    );
    restarted
        .queue
        .acknowledge(
            &underscore_delivery[0].message_id,
            underscore_delivery[0].fencing_token,
        )
        .expect("ack underscore queue");

    let (peer, _) = RuntimeBuilder::new(config.clone())
        .build()
        .expect("peer self-hosted runtime");

    let duplicate_queue = format!("duplicate-delivery-{now}");
    let duplicate_key = IdempotencyKey::new(format!("duplicate-delivery-{now}-key"))
        .expect("duplicate delivery key");
    let duplicate_message_id = restarted
        .queue
        .enqueue(QueueRequest {
            queue: duplicate_queue.clone(),
            payload: json!({"fixtureId": fixture, "kind": "duplicate-delivery"}),
            idempotency_key: duplicate_key.clone(),
            partition_key: None,
            priority: 1,
            available_at_ms: now,
            retention_until_ms: now + 60_000,
            max_attempts: 3,
        })
        .expect("enqueue duplicate delivery fixture");
    inject_physical_queue_duplicate(&duplicate_queue, &duplicate_message_id);
    let duplicate_claims = restarted
        .queue
        .claim(&duplicate_queue, "duplicate-worker", now + 3, 10_000, 2)
        .expect("claim duplicate physical deliveries");
    assert_eq!(duplicate_claims.len(), 1);
    assert_eq!(duplicate_claims[0].attempt, 1);
    assert_eq!(duplicate_claims[0].fencing_token, 1);
    assert!(peer
        .queue
        .claim(&duplicate_queue, "duplicate-peer", now + 3, 10_000, 1)
        .expect("peer duplicate claim")
        .is_empty());
    restarted
        .queue
        .acknowledge(&duplicate_message_id, duplicate_claims[0].fencing_token)
        .expect("ack logical duplicate once");
    let mismatched_duplicate = restarted.queue.enqueue(QueueRequest {
        queue: duplicate_queue.clone(),
        payload: json!({"fixtureId": fixture, "kind": "changed-payload"}),
        idempotency_key: duplicate_key,
        partition_key: None,
        priority: 1,
        available_at_ms: now,
        retention_until_ms: now + 60_000,
        max_attempts: 3,
    });
    assert!(matches!(
        mismatched_duplicate,
        Err(ref error) if error == "queue idempotency key was reused with a different request"
    ));

    let retry_recovery_queue = format!("retry-recovery-{now}");
    let retry_message_id = restarted
        .queue
        .enqueue(QueueRequest {
            queue: retry_recovery_queue.clone(),
            payload: json!({"fixtureId": fixture, "kind": "retry-recovery"}),
            idempotency_key: IdempotencyKey::new(format!("{retry_recovery_queue}-key"))
                .expect("retry recovery key"),
            partition_key: None,
            priority: 1,
            available_at_ms: now,
            retention_until_ms: now + 60_000,
            max_attempts: 3,
        })
        .expect("enqueue retry recovery fixture");
    let retry_claim = restarted
        .queue
        .claim(&retry_recovery_queue, "retry-recovery", now + 4, 10_000, 1)
        .expect("claim retry recovery fixture")
        .pop()
        .expect("retry recovery delivery");
    force_queue_transition(
        &retry_message_id,
        retry_claim.fencing_token,
        "retrying",
        now,
    );
    let retry_recovered = restarted
        .queue
        .claim(&retry_recovery_queue, "retry-recovered", now + 5, 10_000, 1)
        .expect("recover retry transport transition")
        .pop()
        .expect("recovered retry delivery");
    assert!(retry_recovered.fencing_token > retry_claim.fencing_token);
    restarted
        .queue
        .acknowledge(&retry_message_id, retry_recovered.fencing_token)
        .expect("ack recovered retry delivery");

    let ack_recovery_queue = format!("ack-recovery-{now}");
    let ack_message_id = restarted
        .queue
        .enqueue(QueueRequest {
            queue: ack_recovery_queue.clone(),
            payload: json!({"fixtureId": fixture, "kind": "ack-recovery"}),
            idempotency_key: IdempotencyKey::new(format!("{ack_recovery_queue}-key"))
                .expect("ack recovery key"),
            partition_key: None,
            priority: 1,
            available_at_ms: now,
            retention_until_ms: now + 60_000,
            max_attempts: 3,
        })
        .expect("enqueue acknowledgement recovery fixture");
    let ack_claim = restarted
        .queue
        .claim(&ack_recovery_queue, "ack-recovery", now + 6, 10_000, 1)
        .expect("claim acknowledgement recovery fixture")
        .pop()
        .expect("acknowledgement recovery delivery");
    force_queue_transition(
        &ack_message_id,
        ack_claim.fencing_token,
        "acknowledging",
        now,
    );
    assert!(restarted
        .queue
        .claim(&ack_recovery_queue, "ack-recovered", now + 7, 10_000, 1)
        .expect("recover acknowledgement transport transition")
        .is_empty());
    assert_queue_state(&ack_message_id, "acked");

    let schedule_key = format!("race-schedule-{now}");
    restarted
        .scheduler
        .schedule(ScheduledWork {
            schedule_key: schedule_key.clone(),
            queue: "race".to_string(),
            payload: json!({"fixtureId": fixture}),
            due_at_ms: now,
            idempotency_key: IdempotencyKey::new(format!("{schedule_key}-key"))
                .expect("schedule key"),
            fencing_token: 0,
        })
        .expect("schedule race fixture");
    let barrier = Arc::new(Barrier::new(3));
    let left_barrier = Arc::clone(&barrier);
    let right_barrier = Arc::clone(&barrier);
    let left_scheduler = Arc::clone(&restarted.scheduler);
    let right_scheduler = Arc::clone(&peer.scheduler);
    let left = thread::spawn(move || {
        left_barrier.wait();
        left_scheduler.claim_due("scheduler-left", now, 1_000, 1)
    });
    let right = thread::spawn(move || {
        right_barrier.wait();
        right_scheduler.claim_due("scheduler-right", now, 1_000, 1)
    });
    barrier.wait();
    let left_claims = left
        .join()
        .expect("left scheduler thread")
        .expect("left claim");
    let right_claims = right
        .join()
        .expect("right scheduler thread")
        .expect("right claim");
    assert_eq!(left_claims.len() + right_claims.len(), 1);
    let (winning_schedule, owner) = if let Some(claim) = left_claims.first() {
        (claim, "scheduler-left")
    } else {
        (
            right_claims.first().expect("one scheduler claimant wins"),
            "scheduler-right",
        )
    };
    restarted
        .scheduler
        .complete(&schedule_key, owner, winning_schedule.fencing_token, now)
        .expect("complete raced schedule fixture");

    let outbox_id = format!("race-outbox-{now}");
    restarted
        .outbox
        .append(OutboxRecord {
            outbox_id: outbox_id.clone(),
            topic: "race.recorded".to_string(),
            payload: json!({"fixtureId": fixture}),
            idempotency_key: IdempotencyKey::new(format!("{outbox_id}-key")).expect("outbox key"),
            created_at_ms: now,
            fencing_token: 0,
        })
        .expect("append outbox race fixture");
    let barrier = Arc::new(Barrier::new(3));
    let left_barrier = Arc::clone(&barrier);
    let right_barrier = Arc::clone(&barrier);
    let left_outbox = Arc::clone(&restarted.outbox);
    let right_outbox = Arc::clone(&peer.outbox);
    let left = thread::spawn(move || {
        left_barrier.wait();
        left_outbox.claim_pending("outbox-left", now, 1)
    });
    let right = thread::spawn(move || {
        right_barrier.wait();
        right_outbox.claim_pending("outbox-right", now, 1)
    });
    barrier.wait();
    let left_claims = left
        .join()
        .expect("left outbox thread")
        .expect("left claim");
    let right_claims = right
        .join()
        .expect("right outbox thread")
        .expect("right claim");
    assert_eq!(left_claims.len() + right_claims.len(), 1);
    let (winning_claim, owner) = if let Some(claim) = left_claims.first() {
        (claim, "outbox-left")
    } else {
        (
            right_claims.first().expect("one outbox claimant wins"),
            "outbox-right",
        )
    };
    restarted
        .outbox
        .mark_published(&outbox_id, owner, winning_claim.fencing_token)
        .expect("publish raced outbox fixture");

    let lease_resource = format!("race-lease-{now}");
    let barrier = Arc::new(Barrier::new(3));
    let left_barrier = Arc::clone(&barrier);
    let right_barrier = Arc::clone(&barrier);
    let left_leases = Arc::clone(&restarted.leases);
    let right_leases = Arc::clone(&peer.leases);
    let left_resource = lease_resource.clone();
    let right_resource = lease_resource.clone();
    let left = thread::spawn(move || {
        left_barrier.wait();
        left_leases.acquire(&left_resource, "lease-left", now, 1_000)
    });
    let right = thread::spawn(move || {
        right_barrier.wait();
        right_leases.acquire(&right_resource, "lease-right", now, 1_000)
    });
    barrier.wait();
    let left_claim = left.join().expect("left lease thread").expect("left claim");
    let right_claim = right
        .join()
        .expect("right lease thread")
        .expect("right claim");
    assert_eq!(
        usize::from(left_claim.is_some()) + usize::from(right_claim.is_some()),
        1
    );
    restarted
        .leases
        .release(
            left_claim
                .as_ref()
                .or(right_claim.as_ref())
                .expect("one lease claimant wins"),
        )
        .expect("release raced lease fixture");

    let failing_resolver = Arc::new(TestSecretResolver {
        postgres_url: std::env::var("TRADEASSEMBLY_TEST_POSTGRES_URL").expect("test postgres URL"),
        nats_url: "nats://127.0.0.1:1".to_string(),
    });
    let failure = RuntimeBuilder::new(config)
        .with_secret_resolver(failing_resolver)
        .build();
    assert!(
        matches!(failure, Err(ref error) if error == "JetStream connection failed"),
        "self-hosted composition must fail closed when JetStream is unavailable"
    );
}

fn inject_physical_queue_duplicate(queue: &str, message_id: &str) {
    let nats_url = std::env::var("TRADEASSEMBLY_TEST_NATS_URL").expect("test NATS URL");
    let subject = format!("queue.v1_{:x}", Sha256::digest(queue.as_bytes()));
    let payload = json!({"messageId": message_id}).to_string();
    tokio::runtime::Runtime::new()
        .expect("duplicate injector runtime")
        .block_on(async move {
            let client = async_nats::connect(nats_url)
                .await
                .expect("connect duplicate injector");
            client
                .publish(subject, payload.into())
                .await
                .expect("publish duplicate queue record");
            client.flush().await.expect("flush duplicate queue record");
        });
}

fn postgres_client() -> Client {
    Client::connect(
        &std::env::var("TRADEASSEMBLY_TEST_POSTGRES_URL").expect("test Postgres URL"),
        NoTls,
    )
    .expect("connect test Postgres")
}

fn force_event_publication_pending(event_id: &str) {
    assert_eq!(
        postgres_client()
            .execute(
                "UPDATE runtime_events SET published_at_ms=NULL WHERE event_id=$1",
                &[&event_id],
            )
            .expect("force pending event publication"),
        1
    );
}

fn assert_event_published(event_id: &str) {
    let published_at_ms: Option<i64> = postgres_client()
        .query_one(
            "SELECT published_at_ms FROM runtime_events WHERE event_id=$1",
            &[&event_id],
        )
        .expect("read event publication state")
        .get(0);
    assert!(published_at_ms.is_some());
}

fn force_queue_transition(message_id: &str, fencing_token: i64, state: &str, available_at_ms: i64) {
    assert_eq!(
        postgres_client()
            .execute(
                "UPDATE runtime_queue_messages SET state=$3, available_at_ms=$4 WHERE message_id=$1 AND fencing_token=$2",
                &[&message_id, &fencing_token, &state, &available_at_ms],
            )
            .expect("force pending queue transport transition"),
        1
    );
}

fn assert_queue_state(message_id: &str, expected: &str) {
    let state: String = postgres_client()
        .query_one(
            "SELECT state FROM runtime_queue_messages WHERE message_id=$1",
            &[&message_id],
        )
        .expect("read queue state")
        .get(0);
    assert_eq!(state, expected);
}
