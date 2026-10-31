use serde_json::{json, Value};
use std::sync::Arc;
use tradeassembly_runtime::adapters::plugin_operations::LocalPluginOperations;
use tradeassembly_runtime::ports::{
    PluginOperationPort, PluginOperationRequest, PluginOperationResponse, PortDescriptor,
    SideEffectContext, StoragePort, VersionedPort,
};
use tradeassembly_runtime::service::TradeAssemblyService;

const PRIVATE_CANARY: &str = "sk_live_private_receipt_canary_48291";
const RECEIPTS: &str = "plugin_operation_receipts";

#[derive(Clone, Copy)]
enum ReceiptFault {
    Missing,
    Invalid,
    Mismatch,
    PayloadMismatch,
}

struct InvalidReceiptOperations {
    delegate: LocalPluginOperations,
    storage: Arc<dyn StoragePort>,
    fault: ReceiptFault,
}

impl VersionedPort for InvalidReceiptOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl PluginOperationPort for InvalidReceiptOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        let response = self.delegate.invoke(request, context)?;
        if request.operation_id != "marketdata.bars.read_v1" {
            return Ok(response);
        }
        // Return valid evidence while corrupting its durable receipt. This
        // reaches the real evaluator, after the scheduler's envelope checks.
        if matches!(self.fault, ReceiptFault::Missing) {
            self.storage.clear_namespace(RECEIPTS)?;
        } else {
            let mut receipt = serde_json::to_value(&response).expect("serialize test receipt");
            match self.fault {
                ReceiptFault::Invalid => receipt = json!({"private": PRIVATE_CANARY}),
                ReceiptFault::Mismatch => receipt["payload"]["closeMicros"] = json!(1),
                ReceiptFault::PayloadMismatch => {
                    receipt["payload"]["highMicros"] =
                        json!(receipt["payload"]["highMicros"].as_i64().expect("bar high") + 1);
                }
                ReceiptFault::Missing => unreachable!(),
            }
            receipt["payload"]["private"] = json!(PRIVATE_CANARY);
            self.storage
                .put_json(RECEIPTS, context.idempotency_key.as_str(), receipt, context)?;
        }
        Ok(response)
    }
}

fn values(service: &TradeAssemblyService, namespace: &str) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .expect("read durable test records")
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

fn activate(service: &TradeAssemblyService) -> Value {
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {"max_notional": 25.0, "max_order_quantity": 0.00017},
            "capabilityBindings": {
                "execution.market.bars": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "marketdata.bars.read_v1",
                },
                "execution.broker.submit": {
                    "pluginInstanceRef": "sim",
                    "pluginRef": "tradeassembly.simbroker",
                    "operationId": "broker.paper_order_submit",
                },
            },
        }),
    );
    assert_eq!(saved.status, 200, "{:#}", saved.body);
    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": saved.body["body"]["configId"],
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "receipt-diagnostics-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activated.status, 200, "{:#}", activated.body);
    activated.body["body"].clone()
}

#[test]
fn scheduler_retains_owned_evaluation_causes_without_receipt_payloads() {
    for (fault, reason) in [
        (ReceiptFault::Missing, "observation_receipt_missing"),
        (ReceiptFault::Invalid, "observation_receipt_invalid"),
        (ReceiptFault::Mismatch, "observation_receipt_mismatch"),
        (
            ReceiptFault::PayloadMismatch,
            "observation_receipt_payload_mismatch",
        ),
    ] {
        let directory = tempfile::tempdir().expect("test directory");
        let db = directory
            .path()
            .join("runtime.db")
            .to_string_lossy()
            .into_owned();
        let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.clone());
        runtime.plugin_operations = Arc::new(InvalidReceiptOperations {
            delegate: LocalPluginOperations::new(Arc::clone(&runtime.storage)),
            storage: Arc::clone(&runtime.storage),
            fault,
        });
        let service = TradeAssemblyService::from_runtime(db.clone(), runtime);
        let activation = activate(&service);
        let activation_id = activation["activationId"].as_str().expect("activation id");
        let request = json!({
            "activationId": activation_id,
            "workerId": "diagnostic-worker",
            "maxCycles": 1,
            "leaseTtlSeconds": 30,
        });
        let result =
            service.handle_http_from_source("scheduler", "POST", "/scheduler/run", request.clone());
        assert_eq!(result.status, 200, "{:#}", result.body);
        let tick = &result.body["body"]["ticks"][0];
        let classification = json!({"code": "plugin_evidence_invalid", "reason": reason});
        assert_eq!(tick["status"], "blocked");
        assert_eq!(tick["reason"], "evaluation_failed");
        assert_eq!(tick["evaluationFailure"], classification);
        assert_eq!(tick["effectsApplied"], false);
        assert_eq!(tick["willRetryNextCycle"], true);
        assert_eq!(tick["cycle"], 1);
        assert!(tick.get("error").is_none());

        let attempts = values(&service, "execution_attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0]["state"], "blocked");
        assert_eq!(attempts[0]["blockedReason"], "evaluation_failed");
        assert_eq!(attempts[0]["evaluationFailure"], classification);
        assert_eq!(attempts[0]["effectsApplied"], false);
        let events = values(&service, "execution_events");
        let event = events
            .iter()
            .find(|event| event["eventType"] == "execution.cycle_fail_closed")
            .expect("durable fail-closed event");
        assert_eq!(
            event["payload"]["detail"]["evaluationFailure"],
            classification
        );
        let checkpoints = values(&service, "execution_checkpoints");
        let run = service
            .runtime()
            .storage
            .get_json("execution_runs", &format!("run_{activation_id}"))
            .expect("read run")
            .expect("run");
        assert_eq!(run["state"], "active");
        assert_eq!(run["cycle"], 1);
        assert_eq!(
            tick["checkpoint"]["state"]["failClosed"]["effectsApplied"],
            false
        );
        for namespace in [
            "execution_orders",
            "execution_pending_original_cycles",
            "execution_order_attempts",
            "risk_reservations",
        ] {
            assert!(
                values(&service, namespace).is_empty(),
                "unexpected effect in {namespace}"
            );
        }
        assert!(!service
            .runtime()
            .outbox
            .claim_pending(
                "diagnostic-inspection",
                service.runtime().clock.now_ms(),
                100,
            )
            .expect("inspect pending effects")
            .iter()
            .any(|record| record.topic == "execution.paper_order.intent"));
        for diagnostic in [
            &result.body,
            &json!(attempts),
            &json!(events),
            &json!(checkpoints),
        ] {
            assert!(!diagnostic.to_string().contains(PRIVATE_CANARY));
        }
        // The injected value really reached storage; the negative assertion
        // above is not passing because the test never introduced private text.
        if !matches!(fault, ReceiptFault::Missing) {
            assert!(json!(values(&service, RECEIPTS))
                .to_string()
                .contains(PRIVATE_CANARY));
        }

        drop(service);
        let restarted = TradeAssemblyService::from_runtime(
            db.clone(),
            tradeassembly_runtime::adapters::local::test_runtime(db),
        );
        assert_eq!(values(&restarted, "execution_attempts"), attempts);
        // An untargeted poll respects the already-completed cycle's next due
        // time. Targeted /scheduler/run intentionally forces a new cycle.
        let replay = restarted.handle_http_from_source(
            "scheduler",
            "POST",
            "/scheduler/run",
            json!({"maxCycles": 1}),
        );
        assert_eq!(replay.status, 200);
        assert_eq!(replay.body["body"]["ticks"][0]["status"], "idle");
        assert_eq!(values(&restarted, "execution_attempts"), attempts);
        assert_eq!(values(&restarted, "execution_events"), events);
        assert_eq!(values(&restarted, "execution_checkpoints"), checkpoints);
    }
}
