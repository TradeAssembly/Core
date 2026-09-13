use serde_json::json;
use tradeassembly_runtime::ports::{
    AuthorityContext, IdempotencyKey, PluginOperationRequest, SideEffectContext,
};
use tradeassembly_runtime::service::TradeAssemblyService;

const RECEIPTS_NS: &str = "plugin_operation_receipts";
const REQUESTS_NS: &str = "plugin_operation_requests";

#[test]
fn read_receipt_retains_account_and_instrument_binding_without_arbitrary_input() {
    let temp = tempfile::tempdir().unwrap();
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let mut request = bar_request();
    request
        .evidence_refs
        .push("plugin-receipt:caller-forged".into());
    request.input["privateExtension"] = json!("fixture-sensitive-not-for-provenance");
    let runtime = service.runtime();
    let response = runtime
        .plugin_operations
        .invoke(&request, &context("bound-read"))
        .unwrap();
    assert_eq!(
        response
            .evidence_refs
            .iter()
            .filter(|reference| reference.starts_with("plugin-receipt:"))
            .collect::<Vec<_>>(),
        vec!["plugin-receipt:bound-read"]
    );
    let stored = runtime
        .storage
        .get_json(REQUESTS_NS, "bound-read")
        .unwrap()
        .unwrap();
    assert_eq!(stored["binding"]["operationId"], request.operation_id);
    assert_eq!(stored["binding"]["accountRef"], json!(request.account_ref));
    assert_eq!(
        stored["binding"]["pluginInstanceRef"],
        request.plugin_instance_ref
    );
    assert_eq!(
        stored["binding"]["manifestFingerprint"],
        request.manifest_fingerprint
    );
    assert_eq!(
        stored["binding"]["symbolHash"],
        crate_symbol_hash(&request.input["symbol"])
    );
    assert!(!stored
        .to_string()
        .contains("fixture-sensitive-not-for-provenance"));
    assert!(stored.get("input").is_none());
    let receipt = runtime
        .storage
        .get_json(RECEIPTS_NS, "bound-read")
        .unwrap()
        .unwrap();
    assert_eq!(receipt["evidenceRefs"], json!(response.evidence_refs));
    assert_eq!(
        runtime
            .plugin_operations
            .invoke(&request, &context("bound-read"))
            .unwrap(),
        response
    );
}

fn crate_symbol_hash(value: &serde_json::Value) -> String {
    tradeassembly_runtime::spec::canonical_hash(value).unwrap()
}

#[test]
fn broker_receipt_write_failure_is_ambiguous_on_first_call_and_restart() {
    use std::sync::Arc;
    use tradeassembly_runtime::ports::{
        ComparePutOutcome, ImmutablePutOutcome, PluginOperationPort, PortDescriptor, StoragePort,
        VersionedPort,
    };
    struct FailReceipt(Arc<dyn StoragePort>);
    impl VersionedPort for FailReceipt {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            self.0.descriptors()
        }
    }
    impl StoragePort for FailReceipt {
        fn adapter_name(&self) -> &'static str {
            "controlled-receipt-failure"
        }
        fn put_json(
            &self,
            ns: &str,
            key: &str,
            value: serde_json::Value,
            ctx: &SideEffectContext,
        ) -> Result<(), String> {
            self.0.put_json(ns, key, value, ctx)
        }
        fn put_json_if_absent(
            &self,
            ns: &str,
            key: &str,
            value: serde_json::Value,
            ctx: &SideEffectContext,
        ) -> Result<ImmutablePutOutcome, String> {
            if ns == RECEIPTS_NS {
                return Err("controlled_receipt_write_failure".into());
            }
            self.0.put_json_if_absent(ns, key, value, ctx)
        }
        fn compare_and_put_json(
            &self,
            ns: &str,
            key: &str,
            expected: serde_json::Value,
            replacement: serde_json::Value,
            ctx: &SideEffectContext,
        ) -> Result<ComparePutOutcome, String> {
            self.0
                .compare_and_put_json(ns, key, expected, replacement, ctx)
        }
        fn get_json(&self, ns: &str, key: &str) -> Result<Option<serde_json::Value>, String> {
            self.0.get_json(ns, key)
        }
        fn list_json(&self, ns: &str) -> Result<Vec<(String, serde_json::Value)>, String> {
            self.0.list_json(ns)
        }
        fn clear_namespace(&self, ns: &str) -> Result<(), String> {
            self.0.clear_namespace(ns)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let storage = service.runtime().storage.clone();
    let adapter = tradeassembly_runtime::adapters::plugin_operations::LocalPluginOperations::new(
        Arc::new(FailReceipt(storage.clone())),
    );
    let mut request = bar_request();
    request.operation_id = "broker.paper_order_submit".into();
    request.capability = "broker.order_submit.paper".into();
    request.input = json!({"symbol":"BTC/USD", "side":"buy", "quantityMicros":1,
        "fillPriceMicros":1,"sequence":1,"clientOrderId":"controlled-receipt-fault"});
    let ctx = context("controlled-receipt-fault");
    assert_eq!(
        adapter.invoke(&request, &ctx).unwrap_err(),
        "plugin_order_reconciliation_required"
    );
    // Storage is healthy again, but the prior claim prevents another dispatch.
    let restarted = tradeassembly_runtime::adapters::plugin_operations::LocalPluginOperations::new(
        storage.clone(),
    );
    assert_eq!(
        restarted.invoke(&request, &ctx).unwrap_err(),
        "plugin_order_reconciliation_required"
    );
    assert!(storage.list_json(RECEIPTS_NS).unwrap().is_empty());
    assert_eq!(
        storage
            .list_json("plugin_broker_dispatch_claims")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn plugin_order_requires_explicit_submission_without_dispatch_or_order_records() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tradeassembly_runtime::ports::{
        PluginOperationPort, PluginOperationResponse, PortDescriptor, VersionedPort,
    };
    struct CountingBroker(Arc<AtomicUsize>);
    impl VersionedPort for CountingBroker {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            vec![]
        }
    }
    impl PluginOperationPort for CountingBroker {
        fn invoke(
            &self,
            _: &PluginOperationRequest,
            _: &SideEffectContext,
        ) -> Result<PluginOperationResponse, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err("unexpected_dispatch".into())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let path = db_path(&temp);
    let base = TradeAssemblyService::test_local(&path);
    let mut runtime = (*base.runtime()).clone();
    let calls = Arc::new(AtomicUsize::new(0));
    runtime.plugin_operations = Arc::new(CountingBroker(calls.clone()));
    let service = TradeAssemblyService::from_runtime(&path, runtime);
    let mut request = bar_request();
    request.operation_id = "broker.paper_order_submit".into();
    request.capability = "broker.order_submit.paper".into();
    for field in [
        None,
        Some(json!(false)),
        Some(json!("true")),
        Some(json!(null)),
    ] {
        let mut body = json!({"pluginOperationRequest":request});
        if let Some(field) = field {
            body["submitOrders"] = field;
        }
        let response = service.handle_http("POST", "/product/run-center/run-once", body);
        assert_eq!(response.status, 400, "{response:#?}");
        assert!(response
            .body
            .to_string()
            .contains("plugin_order_submission_not_requested"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(service
        .runtime()
        .storage
        .list_json("execution_orders")
        .unwrap()
        .is_empty());
    assert!(service
        .runtime()
        .storage
        .list_json("execution_order_attempts")
        .unwrap()
        .is_empty());
}

#[test]
fn order_service_preserves_unfinished_dispatch_as_reconciliation_required() {
    use tradeassembly_runtime::ports::{
        PluginOperationPort, PluginOperationResponse, PortDescriptor, VersionedPort,
    };
    struct PendingBroker;
    impl VersionedPort for PendingBroker {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            vec![]
        }
    }
    impl PluginOperationPort for PendingBroker {
        fn invoke(
            &self,
            _: &PluginOperationRequest,
            _: &SideEffectContext,
        ) -> Result<PluginOperationResponse, String> {
            Err("plugin_order_reconciliation_required".into())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let path = db_path(&temp);
    let base = TradeAssemblyService::test_local(&path);
    let mut runtime = (*base.runtime()).clone();
    runtime.plugin_operations = std::sync::Arc::new(PendingBroker);
    let service = TradeAssemblyService::from_runtime(&path, runtime);
    let mut request = bar_request();
    request.operation_id = "broker.paper_order_submit".into();
    request.capability = "broker.order_submit.paper".into();
    let response = service.handle_http(
        "POST",
        "/product/run-center/run-once",
        json!({
            "submitOrders":true, "idempotencyKey":"service-pending-broker", "accountMode":"paper",
            "symbol":"BTC/USD", "qty":0.0002, "side":"buy", "pluginOperationRequest":request,
            "authorityContext":{"actor":"controlled-owner","surface":"test","accountMode":"paper"}
        }),
    );
    assert_eq!(
        response.body["body"]["order"]["status"], "reconciliation_required",
        "{response:#?}"
    );
}

#[test]
fn unfinished_broker_dispatch_survives_adapter_restart_without_resubmission() {
    use tradeassembly_runtime::ports::PluginOperationPort;
    let temp = tempfile::tempdir().unwrap();
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();
    let mut request = bar_request();
    request.operation_id = "broker.paper_order_submit".into();
    request.capability = "broker.order_submit.paper".into();
    request.input = json!({"symbol":"BTC/USD","side":"buy","quantityMicros":1,
        "fillPriceMicros":1,"sequence":1,"clientOrderId":"controlled-id"});
    let context = context("controlled-broker-claim");
    let first = runtime
        .plugin_operations
        .invoke(&request, &context)
        .unwrap();
    let completed_retry = runtime
        .plugin_operations
        .invoke(&request, &context)
        .unwrap();
    assert_eq!(first, completed_retry);
    assert_eq!(
        runtime
            .storage
            .list_json("plugin_broker_dispatch_claims")
            .unwrap()
            .len(),
        1
    );
    // Fault injection in an isolated fixture: model a crash after dispatch but
    // before its response becomes durable. Preserve the dispatch claim.
    runtime.storage.clear_namespace(RECEIPTS_NS).unwrap();
    let restarted = tradeassembly_runtime::adapters::plugin_operations::LocalPluginOperations::new(
        runtime.storage.clone(),
    );
    assert_eq!(
        restarted.invoke(&request, &context).unwrap_err(),
        "plugin_order_reconciliation_required"
    );
    assert!(runtime.storage.list_json(RECEIPTS_NS).unwrap().is_empty());
}

#[test]
fn sim_bar_invocation_is_exact_and_duplicate_retry_returns_the_durable_receipt() {
    let temp = tempfile::tempdir().expect("create isolated test directory");
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();
    let request = bar_request();
    let context = context("plugin-bar-idempotency");

    let first = runtime
        .plugin_operations
        .invoke(&request, &context)
        .expect("valid sim bar invocation");
    assert_eq!(first.schema_ref, "schema://market-data/bar@1");
    assert_eq!(first.observed_at_ms, 420_000);
    assert_eq!(
        first.payload,
        json!({
            "symbol": "BTC/USD",
            "barIndex": 7,
            "openMicros": 94_990_000,
            "highMicros": 95_025_000,
            "lowMicros": 94_975_000,
            "closeMicros": 95_000_000,
            "volumeMicros": 100_000_000_000_i64,
            "timeframe": "1m",
            "sourceClass": "deterministic_fixture",
        })
    );
    assert!(first.deterministic);
    assert!(first.replayable);

    let durable = runtime
        .storage
        .get_json(RECEIPTS_NS, context.idempotency_key.as_str())
        .expect("read durable receipt")
        .expect("durable receipt exists");
    let duplicate = runtime
        .plugin_operations
        .invoke(&request, &context)
        .expect("duplicate identical invocation");

    assert_eq!(
        serde_json::to_vec(&first).expect("serialize first response"),
        serde_json::to_vec(&duplicate).expect("serialize duplicate response")
    );
    assert_eq!(
        serde_json::to_vec(&duplicate).expect("serialize duplicate response"),
        serde_json::to_vec(&durable).expect("serialize durable receipt")
    );
}

#[test]
fn changed_request_cannot_reuse_an_existing_plugin_operation_idempotency_key() {
    let temp = tempfile::tempdir().expect("create isolated test directory");
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();
    let context = context("plugin-changed-request-idempotency");

    runtime
        .plugin_operations
        .invoke(&bar_request(), &context)
        .expect("first request succeeds");

    let mut changed = bar_request();
    changed.input = json!({"symbol": "BTC/USD", "sequence": 8, "timeframe": "1m"});
    assert!(
        runtime
            .plugin_operations
            .invoke(&changed, &context)
            .is_err(),
        "a changed request must not receive the original durable receipt"
    );
}

#[test]
fn historical_receipts_inherit_the_trusted_request_root_and_conflicts_fail_closed() {
    let temp = tempfile::tempdir().expect("create isolated test directory");
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();
    let request = bar_request();
    let context = context("plugin-historical-correlation");

    let response = runtime
        .plugin_operations
        .invoke(&request, &context)
        .expect("create durable plugin receipt");
    let mut historical = serde_json::to_value(response).expect("serialize response");
    historical
        .as_object_mut()
        .expect("response is an object")
        .remove("correlationId");
    runtime
        .storage
        .put_json(
            RECEIPTS_NS,
            context.idempotency_key.as_str(),
            historical,
            &context,
        )
        .expect("replace with historical receipt");

    let restored = runtime
        .plugin_operations
        .invoke(&request, &context)
        .expect("historical receipt remains readable");
    assert_eq!(restored.correlation_id, request.correlation_id);

    let mut conflicting = serde_json::to_value(restored).expect("serialize restored response");
    conflicting["correlationId"] = json!("cmd-other-root");
    runtime
        .storage
        .put_json(
            RECEIPTS_NS,
            context.idempotency_key.as_str(),
            conflicting,
            &context,
        )
        .expect("replace with conflicting receipt");
    assert_eq!(
        runtime
            .plugin_operations
            .invoke(&request, &context)
            .expect_err("a mismatched durable receipt must fail closed"),
        "plugin_operation_correlation_conflict"
    );

    let request_record = runtime
        .storage
        .get_json(REQUESTS_NS, context.idempotency_key.as_str())
        .expect("read request record")
        .expect("request record exists");
    assert_eq!(request_record["correlationId"], request.correlation_id);
}

#[test]
fn unsupported_plugin_operation_and_unknown_mode_fail_closed() {
    let temp = tempfile::tempdir().expect("create isolated test directory");
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();

    let mut unknown = bar_request();
    unknown.plugin_ref = "example.unknown-plugin".to_string();
    assert_eq!(
        runtime
            .plugin_operations
            .invoke(&unknown, &context("plugin-unknown"))
            .expect_err("unknown plugin must fail closed"),
        "plugin_instance_not_found"
    );

    let mut non_paper = bar_request();
    non_paper.mode = "unknown".to_string();
    assert_eq!(
        runtime
            .plugin_operations
            .invoke(&non_paper, &context("plugin-unknown-mode"))
            .expect_err("unknown mode must fail closed"),
        "plugin_operation_mode_unsupported"
    );
}

#[test]
fn live_read_crosses_port_but_broker_capability_cannot_disguise_itself_as_a_read() {
    let temp = tempfile::tempdir().expect("isolated storage");
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();
    let mut read = bar_request();
    read.mode = "live".into();
    let response = runtime
        .plugin_operations
        .invoke(&read, &context("live-read"))
        .expect("Live strategy may obtain market data before admission");
    assert_eq!(response.schema_ref, "schema://market-data/bar@1");
    assert!(runtime
        .storage
        .get_json(RECEIPTS_NS, "live-read")
        .unwrap()
        .is_some());

    for mode in ["live", "paper", "research", "backtest"] {
        let mut disguised = read.clone();
        disguised.mode = mode.into();
        disguised.capability = "broker.order_submit.live".into();
        let key = format!("disguised-live-{mode}");
        assert_eq!(
            runtime
                .plugin_operations
                .invoke(&disguised, &context(&key))
                .unwrap_err(),
            "plugin_operation_mode_unsupported"
        );
        assert!(runtime
            .storage
            .get_json(REQUESTS_NS, &key)
            .unwrap()
            .is_none());
        assert!(runtime
            .storage
            .get_json(RECEIPTS_NS, &key)
            .unwrap()
            .is_none());
    }
}

#[test]
fn default_live_submission_is_denied_and_records_no_dispatch() {
    let temp = tempfile::tempdir().expect("isolated storage");
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();
    let mut request = bar_request();
    request.plugin_ref = "external.test-broker".into();
    request.operation_id = "broker.live_order_submit".into();
    request.capability = "broker.order_submit.live".into();
    request.mode = "live".into();
    request.purpose = "live_order_submission".into();
    let context = context("default-live-denial");
    for _ in 0..2 {
        assert_eq!(
            runtime
                .plugin_operations
                .invoke(&request, &context)
                .unwrap_err(),
            "plugin_operation_mode_unsupported"
        );
    }
    assert!(runtime
        .storage
        .list_json("plugin_broker_dispatch_claims")
        .unwrap()
        .is_empty());
    assert!(runtime
        .storage
        .get_json(
            "plugin_broker_submission_denials",
            context.idempotency_key.as_str()
        )
        .unwrap()
        .is_none());
}

#[test]
fn paper_order_returns_provider_identity_and_rejects_secret_shaped_input() {
    let temp = tempfile::tempdir().expect("create isolated test directory");
    let service = TradeAssemblyService::test_local(db_path(&temp));
    let runtime = service.runtime();
    let api_key = "pk_test_do_not_echo";
    let raw_credential = "local-test-value";
    let mut request = bar_request();
    request.operation_id = "broker.paper_order_submit".to_string();
    request.capability = "broker.order_submit.paper".to_string();
    request.input = json!({
        "symbol": "BTC/USD",
        "side": "buy",
        "quantityMicros": 170_000_000_i64,
        "fillPriceMicros": 95_000_000_i64,
        "sequence": 7,
        "clientOrderId": "client-order-7",
    });

    let response = runtime
        .plugin_operations
        .invoke(&request, &context("plugin-paper-order"))
        .expect("paper order invocation");
    assert_eq!(response.schema_ref, "schema://broker/order-receipt@1");
    assert!(response.payload["providerOrderId"]
        .as_str()
        .is_some_and(|value| value.starts_with("sim-order-")));
    assert_eq!(response.payload["clientOrderId"], "client-order-7");
    assert_eq!(response.payload["status"], "filled");
    assert_eq!(response.payload["fillPriceMicros"], 95_000_000_i64);
    assert!(response
        .provider_outcome_id
        .as_deref()
        .is_some_and(|value| value.starts_with("sim-order-")));

    let mut second_activation = request.clone();
    second_activation.activation_id = "activation-second".to_string();
    second_activation.correlation_id = "cmd-second-correlation".to_string();
    second_activation.input["clientOrderId"] = json!("client-order-second");
    let second_response = runtime
        .plugin_operations
        .invoke(
            &second_activation,
            &context("plugin-paper-order-second-activation"),
        )
        .expect("second activation paper order invocation");
    assert_ne!(
        response.provider_outcome_id, second_response.provider_outcome_id,
        "provider outcomes must be namespaced by activation even when tick IDs match"
    );

    request.input["apiKey"] = json!(api_key);
    request.input["apiSecret"] = json!(raw_credential);
    assert_eq!(
        runtime
            .plugin_operations
            .invoke(&request, &context("plugin-paper-order-with-secret"))
            .expect_err("raw credentials must not cross the operation request boundary"),
        "plugin_operation_raw_credential_forbidden"
    );
}

fn bar_request() -> PluginOperationRequest {
    PluginOperationRequest {
        correlation_id: "cmd-test-correlation".to_string(),
        plugin_instance_ref: "tradeassembly.simbroker.local".to_string(),
        plugin_ref: "tradeassembly.simbroker".to_string(),
        manifest_fingerprint: "sha256:simbroker-manifest".to_string(),
        operation_id: "marketdata.bars.read_v1".to_string(),
        capability: "market_data.bars.read@1".to_string(),
        capability_graph_revision_id: "capability-graph-revision-1".to_string(),
        capability_graph_fingerprint: "sha256:capability-graph".to_string(),
        strategy_id: "strategy-btc-paper".to_string(),
        strategy_version_id: "strategy-btc-paper-v1".to_string(),
        strategy_spec_hash: "sha256:strategy-spec".to_string(),
        activation_id: "activation-btc-paper".to_string(),
        attempt_id: "attempt-7".to_string(),
        evaluation_tick_id: "tick-7".to_string(),
        mode: "paper".to_string(),
        purpose: "paper_trading".to_string(),
        account_ref: None,
        timeout_ms: 10_000,
        fencing_token: Some(7),
        input: json!({"symbol": "BTC/USD", "sequence": 7, "timeframe": "1m"}),
        evidence_refs: vec!["evidence://fixture/7".to_string()],
    }
}

fn context(idempotency_key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(idempotency_key).expect("valid idempotency key"),
    )
}

fn db_path(temp: &tempfile::TempDir) -> String {
    temp.path()
        .join("plugin-operations.db")
        .to_string_lossy()
        .to_string()
}
