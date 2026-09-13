use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use tradeassembly_runtime::adapters::plugin_operations::LocalPluginOperations;
use tradeassembly_runtime::ports::{
    AuthorityContext, IdempotencyKey, PluginOperationPort, PluginOperationRequest,
    PluginOperationResponse, PortDescriptor, SideEffectContext, StoragePort, VersionedPort,
};
use tradeassembly_runtime::service::TradeAssemblyService;

const EVALUATE_TICK_PATH: &str =
    "/product/strategy-execution-activations/{activation_id}/ticks/evaluate";

#[derive(Clone, Copy)]
enum BrokerFailurePoint {
    BeforeSubmission,
    AfterSubmission,
}

#[derive(Clone, Copy)]
enum MarketEvidenceCorruption {
    Schema,
    Freshness,
    Instrument,
    Evidence,
}

struct FailOncePluginOperations {
    delegate: LocalPluginOperations,
    failure_point: BrokerFailurePoint,
    failed: AtomicBool,
    broker_calls: Arc<AtomicUsize>,
}

struct InvalidMarketEvidencePluginOperations {
    delegate: LocalPluginOperations,
    corruption: MarketEvidenceCorruption,
}

struct RiskOrderingPluginOperations {
    delegate: LocalPluginOperations,
    storage: Arc<dyn StoragePort>,
    observed_pre_submission_reservation: Arc<AtomicBool>,
}

struct ReconciliationRequiredPluginOperations {
    delegate: LocalPluginOperations,
}

struct LegacyReceiptUntilRekeyPluginOperations {
    delegate: LocalPluginOperations,
    original_key: Mutex<Option<String>>,
}

impl VersionedPort for RiskOrderingPluginOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl PluginOperationPort for RiskOrderingPluginOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        if request.operation_id == "broker.paper_order_submit" {
            let reserved = !self.storage.list_json("risk_reservations")?.is_empty();
            self.observed_pre_submission_reservation
                .store(reserved, Ordering::SeqCst);
            if !reserved {
                return Err("risk_reservation_missing_before_submission".to_string());
            }
        }
        self.delegate.invoke(request, context)
    }
}

impl VersionedPort for ReconciliationRequiredPluginOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl PluginOperationPort for ReconciliationRequiredPluginOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        let mut response = self.delegate.invoke(request, context)?;
        if request.operation_id == "broker.paper_order_submit" {
            response.reconciliation_required = true;
            response.freshness_state = "unknown".to_string();
            response.payload = json!({"status": "unknown"});
        }
        Ok(response)
    }
}

impl VersionedPort for LegacyReceiptUntilRekeyPluginOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl PluginOperationPort for LegacyReceiptUntilRekeyPluginOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        let mut response = self.delegate.invoke(request, context)?;
        if request.operation_id != "broker.paper_order_submit" {
            return Ok(response);
        }
        let key = context.idempotency_key.as_str();
        let mut original = self.original_key.lock().expect("legacy receipt key lock");
        let original_key = original.get_or_insert_with(|| key.to_string());
        if original_key == key {
            response.schema_ref = "schema://legacy-plugin/order-response@1".to_string();
            let payload = response.payload.as_object_mut().expect("order payload");
            payload.remove("fillPrice");
            payload.remove("fillPriceMicros");
        }
        Ok(response)
    }
}

impl VersionedPort for InvalidMarketEvidencePluginOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl PluginOperationPort for InvalidMarketEvidencePluginOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        let mut response = self.delegate.invoke(request, context)?;
        if request.operation_id == "marketdata.bars.read_v1" {
            match self.corruption {
                MarketEvidenceCorruption::Schema => {
                    response.schema_ref = "schema://market-data/unknown@1".to_string();
                }
                MarketEvidenceCorruption::Freshness => {
                    response.freshness_state = "stale".to_string();
                }
                MarketEvidenceCorruption::Instrument => {
                    response.payload["symbol"] = json!("OTHER/USD");
                }
                MarketEvidenceCorruption::Evidence => {
                    response.evidence_refs.clear();
                }
            }
        }
        Ok(response)
    }
}

impl VersionedPort for FailOncePluginOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl PluginOperationPort for FailOncePluginOperations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        if request.operation_id != "broker.paper_order_submit" {
            return self.delegate.invoke(request, context);
        }
        self.broker_calls.fetch_add(1, Ordering::SeqCst);
        if !self.failed.swap(true, Ordering::SeqCst) {
            if matches!(self.failure_point, BrokerFailurePoint::AfterSubmission) {
                self.delegate.invoke(request, context)?;
            }
            return Err(
                "injected_broker_transport_failure token=must-never-be-persisted".to_string(),
            );
        }
        self.delegate.invoke(request, context)
    }
}

fn test_db(name: &str) -> String {
    format!(
        ".tradeassembly/test-continuous-paper-{name}-{}.db",
        std::process::id()
    )
}

fn activate(service: &TradeAssemblyService, suffix: &str) -> Value {
    activate_with_risk(service, suffix, 25.0, 0.00017)
}

fn activate_with_risk(
    service: &TradeAssemblyService,
    suffix: &str,
    max_notional: f64,
    max_order_quantity: f64,
) -> Value {
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "riskLimits": {
                "max_notional": max_notional,
                "max_order_quantity": max_order_quantity,
            },
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
    assert_eq!(saved.body["body"]["item"]["assetClass"], "crypto");
    for check_id in [
        "strategy_version_locked",
        "strategy_spec_executable",
        "capability_graph_revision",
        "data_available",
        "calendar_available",
        "broker_operation_available",
        "data_freshness_policy",
        "warmup_available",
        "risk_limits",
        "entitlement",
        "account_bindings",
        "local_identity",
    ] {
        let check = saved.body["body"]["readiness"]["checks"]
            .as_array()
            .expect("readiness checks")
            .iter()
            .find(|check| check["id"] == check_id)
            .unwrap_or_else(|| panic!("missing readiness check {check_id}"));
        assert_eq!(
            check["status"], "pass",
            "readiness check {check_id} did not pass: {check:#}"
        );
    }

    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": saved.body["body"]["configId"],
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": format!("continuous-paper-activation-{suffix}"),
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activated.status, 200, "{:#}", activated.body);
    assert_eq!(activated.body["body"]["run"]["assetClass"], "crypto");
    activated.body["body"].clone()
}

fn storage_context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("test idempotency key"),
    )
}

fn namespace_values(service: &TradeAssemblyService, namespace: &str) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .expect("list execution namespace")
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

fn service_with_one_broker_failure(
    db: &str,
    failure_point: BrokerFailurePoint,
) -> (TradeAssemblyService, Arc<AtomicUsize>) {
    let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.to_string());
    let broker_calls = Arc::new(AtomicUsize::new(0));
    runtime.plugin_operations = Arc::new(FailOncePluginOperations {
        delegate: LocalPluginOperations::new(Arc::clone(&runtime.storage)),
        failure_point,
        failed: AtomicBool::new(false),
        broker_calls: Arc::clone(&broker_calls),
    });
    (
        TradeAssemblyService::from_runtime(db.to_string(), runtime),
        broker_calls,
    )
}

fn service_with_market_corruption(
    db: &str,
    corruption: MarketEvidenceCorruption,
) -> TradeAssemblyService {
    let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.to_string());
    runtime.plugin_operations = Arc::new(InvalidMarketEvidencePluginOperations {
        delegate: LocalPluginOperations::new(Arc::clone(&runtime.storage)),
        corruption,
    });
    TradeAssemblyService::from_runtime(db.to_string(), runtime)
}

fn service_with_risk_ordering_assertion(db: &str) -> (TradeAssemblyService, Arc<AtomicBool>) {
    let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.to_string());
    let observed = Arc::new(AtomicBool::new(false));
    runtime.plugin_operations = Arc::new(RiskOrderingPluginOperations {
        delegate: LocalPluginOperations::new(Arc::clone(&runtime.storage)),
        storage: Arc::clone(&runtime.storage),
        observed_pre_submission_reservation: Arc::clone(&observed),
    });
    (
        TradeAssemblyService::from_runtime(db.to_string(), runtime),
        observed,
    )
}

fn service_with_reconciliation_required_order(db: &str) -> TradeAssemblyService {
    let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.to_string());
    runtime.plugin_operations = Arc::new(ReconciliationRequiredPluginOperations {
        delegate: LocalPluginOperations::new(Arc::clone(&runtime.storage)),
    });
    TradeAssemblyService::from_runtime(db.to_string(), runtime)
}

fn service_with_legacy_receipt_until_rekey(db: &str) -> TradeAssemblyService {
    let mut runtime = tradeassembly_runtime::adapters::local::test_runtime(db.to_string());
    runtime.plugin_operations = Arc::new(LegacyReceiptUntilRekeyPluginOperations {
        delegate: LocalPluginOperations::new(Arc::clone(&runtime.storage)),
        original_key: Mutex::new(None),
    });
    TradeAssemblyService::from_runtime(db.to_string(), runtime)
}

fn execution_event_hash(event: &Value) -> String {
    let value = json!({
        "activationId": event["activationId"],
        "correlationId": event["correlationId"],
        "sequence": event["sequence"],
        "eventType": event["eventType"],
        "payload": event["payload"],
        "previousHash": event["previousHash"],
    });
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&value).expect("serialize event hash input"))
    )
}

fn source_bound_sim_bar(
    activation: &Value,
    event_id: &str,
    close: f64,
    observed_at_ms: i64,
) -> Value {
    source_bound_sim_bar_for_symbol(activation, event_id, "BTC/USD", close, observed_at_ms)
}

fn source_bound_sim_bar_for_symbol(
    activation: &Value,
    event_id: &str,
    symbol: &str,
    close: f64,
    observed_at_ms: i64,
) -> Value {
    let binding = activation["run"]["immutableInput"]["capabilityGraphSnapshot"]["nodes"]
        .as_array()
        .expect("capability graph nodes")
        .iter()
        .filter_map(|node| node.get("selected"))
        .find(|selected| selected["operationId"] == "marketdata.bars.read_v1")
        .expect("market-data plugin binding");
    json!({
        "schemaVersion": "tradeassembly.plugin_observation.v1",
        "eventId": event_id,
        "observedAtMs": observed_at_ms,
        "freshness": {"state": "fresh", "maxAgeMs": 60_000},
        "source": {
            "pluginInstanceRef": binding["pluginInstanceRef"],
            "pluginRef": binding["pluginRef"],
            "manifestFingerprint": binding["manifestFingerprint"],
            "operationId": binding["operationId"],
            "capability": binding["operation"]["capability"],
        },
        "bar": {
            "symbol": symbol,
            "timeframe": "1m",
            "open": close,
            "high": close,
            "low": close,
            "close": close,
            "volume": 10.0,
        },
        "evidenceRefs": [format!("sim://bars/{event_id}")],
    })
}

fn replace_json_string(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(text) if text == from => *text = to.to_string(),
        Value::Array(items) => {
            for item in items {
                replace_json_string(item, from, to);
            }
        }
        Value::Object(fields) => {
            for field in fields.values_mut() {
                replace_json_string(field, from, to);
            }
        }
        _ => {}
    }
}

fn duplicate_published_strategy(
    service: &TradeAssemblyService,
    name: &str,
    symbol: &str,
) -> String {
    duplicate_published_strategy_with_calendar(service, name, symbol, None)
}

fn duplicate_published_strategy_with_calendar(
    service: &TradeAssemblyService,
    name: &str,
    symbol: &str,
    calendar_ref: Option<&str>,
) -> String {
    let duplicated = service.handle_http(
        "POST",
        "/product/strategies/duplicate",
        json!({"strategyId": "strat_local_btc_demo", "name": name}),
    );
    assert_eq!(duplicated.status, 200, "{:#}", duplicated.body);
    let strategy_id = duplicated.body["body"]["strategy"]["id"]
        .as_str()
        .expect("duplicated strategy id")
        .to_string();
    let mut spec = duplicated.body["body"]["draft"]["spec"].clone();
    replace_json_string(&mut spec, "BTC/USD", symbol);
    if let Some(calendar_ref) = calendar_ref {
        replace_json_string(&mut spec, "CRYPTO_24X7", calendar_ref);
    }
    let saved = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId": strategy_id, "name": name, "spec": spec}),
    );
    assert_eq!(saved.status, 200, "{:#}", saved.body);
    assert_eq!(saved.body["body"]["validation"]["ok"], true);
    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"},
        }),
    );
    assert_eq!(published.status, 200, "{:#}", published.body);
    assert_eq!(published.body["body"]["published"], true);
    strategy_id
}

fn activate_strategy(
    service: &TradeAssemblyService,
    strategy_id: &str,
    symbol: &str,
    suffix: &str,
) -> Value {
    let saved = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": strategy_id,
            "mode": "paper",
            "providerRef": "sim",
            "symbol": symbol,
            "riskLimits": {
                "max_notional": 25.0,
                "max_order_quantity": 0.00017,
            },
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
    assert_eq!(saved.body["body"]["readiness"]["ready"], true);
    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": saved.body["body"]["configId"],
            "strategyId": strategy_id,
            "idempotencyKey": format!("continuous-paper-{suffix}"),
            "correlationId": "caller-forged-correlation",
            "controlPlaneCorrelationId": "caller-forged-internal-correlation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_eq!(activated.status, 200, "{:#}", activated.body);
    activated.body["body"].clone()
}

fn evaluate_tick(
    service: &TradeAssemblyService,
    activation: &Value,
    tick_id: &str,
    sequence: u64,
    mut observation: Value,
) -> tradeassembly_runtime::service::ServiceResponse {
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let correlation_id = activation["run"]["correlationId"]
        .as_str()
        .expect("correlation id");
    assert_ne!(correlation_id, "caller-forged-correlation");
    assert_ne!(correlation_id, "caller-forged-internal-correlation");
    observation["freshness"]["maxAgeMs"] =
        activation["run"]["dataFreshnessPolicy"]["maxAgeMs"].clone();
    persist_market_receipt(
        service,
        activation_id,
        activation["run"]["correlationId"]
            .as_str()
            .expect("activation correlation"),
        sequence,
        &mut observation,
    );
    service.handle_http_from_source(
        "scheduler",
        "POST",
        &EVALUATE_TICK_PATH.replace("{activation_id}", activation_id),
        json!({
            "tickId": tick_id,
            "sequence": sequence,
            "strategyVersionId": activation["run"]["strategyVersionId"],
            "observation": observation,
            "idempotencyKey": format!("{activation_id}:{tick_id}"),
            "accountMode": "paper",
            "authorityContext": {
                "actor": "local-user",
                "surface": "scheduler",
                "accountMode": "paper",
            },
        }),
    )
}

fn persist_market_receipt(
    service: &TradeAssemblyService,
    activation_id: &str,
    correlation_id: &str,
    sequence: u64,
    observation: &mut Value,
) {
    let event_id = observation["eventId"]
        .as_str()
        .unwrap_or("invalid-event")
        .to_string();
    let content_hash = format!(
        "sha256:{:x}",
        Sha256::digest(
            serde_json::to_vec(&observation["bar"]).expect("serialize test market observation")
        )
    );
    let mut source_evidence = observation["evidenceRefs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    source_evidence.retain(|reference| {
        reference
            .as_str()
            .is_some_and(|value| !value.starts_with("plugin-"))
    });
    let mut durable_evidence = source_evidence.clone();
    durable_evidence.push(json!(format!("plugin-event://{event_id}")));
    durable_evidence.push(json!(format!("plugin-content://{content_hash}")));
    observation["evidenceRefs"] = Value::Array(durable_evidence);
    observation["deterministic"] = json!(true);
    observation["replayable"] = json!(true);
    let micros = |field: &str| {
        observation["bar"][field]
            .as_f64()
            .map(|value| (value * 1_000_000.0).round() as i64)
            .unwrap_or_default()
    };
    let receipt = PluginOperationResponse {
        correlation_id: correlation_id.to_string(),
        schema_ref: "schema://market-data/bar@1".to_string(),
        payload: json!({
            "symbol": observation["bar"]["symbol"],
            "barIndex": sequence,
            "openMicros": micros("open"),
            "highMicros": micros("high"),
            "lowMicros": micros("low"),
            "closeMicros": micros("close"),
            "volumeMicros": micros("volume"),
            "timeframe": observation["bar"]["timeframe"],
            "sourceClass": "test_fixture",
        }),
        observed_at_ms: observation["observedAtMs"].as_i64().unwrap_or_default(),
        source_event_id: event_id,
        content_hash,
        freshness_state: "fresh".to_string(),
        evidence_refs: source_evidence
            .into_iter()
            .filter_map(|reference| reference.as_str().map(str::to_string))
            .collect(),
        deterministic: true,
        replayable: true,
        provider_outcome_id: None,
        reconciliation_required: false,
    };
    service
        .runtime()
        .storage
        .put_json(
            "plugin_operation_receipts",
            &format!("execution:{activation_id}:tick:{sequence}:market"),
            serde_json::to_value(receipt).expect("serialize test market receipt"),
            &storage_context(&format!("test:market-receipt:{activation_id}:{sequence}")),
        )
        .expect("persist test market receipt");
}

fn orders_for_activation(service: &TradeAssemblyService, activation_id: &str) -> Vec<Value> {
    service
        .handle_http("GET", "/orders", json!({}))
        .body
        .as_array()
        .expect("orders response")
        .iter()
        .filter(|order| order["activation_id"] == activation_id)
        .cloned()
        .collect()
}

#[test]
fn source_bound_sim_bars_compile_the_published_spec_and_record_factual_decisions() {
    let db = test_db("source-bound-decisions");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "source-bound-decisions");

    let no_signal = evaluate_tick(
        &service,
        &activation,
        "tick-false-entry",
        1,
        source_bound_sim_bar(&activation, "sim-bar-false-entry", 100.0, 1_784_000_000_000),
    );
    assert_eq!(no_signal.status, 200, "{:#}", no_signal.body);
    assert_eq!(no_signal.body["body"]["decision"]["kind"], "no_signal");
    assert_eq!(
        no_signal.body["body"]["decision"]["sourceObservationIds"],
        json!(["sim-bar-false-entry"])
    );
    assert_eq!(
        no_signal.body["body"]["compiledStrategy"]["strategyVersionId"],
        activation["run"]["strategyVersionId"]
    );
    assert_eq!(
        no_signal.body["body"]["compiledStrategy"]["strategySpecHash"],
        activation["run"]["strategySpecHash"]
    );
    assert!(no_signal.body["body"]["decision"]["facts"].is_object());
    assert!(
        orders_for_activation(
            &service,
            activation["activationId"].as_str().expect("activation id")
        )
        .is_empty(),
        "a factual no-signal decision must not submit a paper order"
    );
}

#[test]
fn configured_quantity_and_tick_identity_control_the_single_sim_order() {
    let db = test_db("quantity-and-idempotency");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "quantity-and-idempotency");
    let observation =
        source_bound_sim_bar(&activation, "sim-bar-true-entry", 101.0, 1_784_000_060_000);

    let first = evaluate_tick(
        &service,
        &activation,
        "tick-true-entry",
        1,
        observation.clone(),
    );
    assert_eq!(first.status, 200, "{:#}", first.body);
    assert_eq!(first.body["body"]["decision"]["kind"], "order_intent");
    assert_eq!(first.body["body"]["orderIntent"]["quantity"], 0.00017);
    assert_eq!(
        first.body["body"]["orderIntent"]["sourceObservationId"],
        "sim-bar-true-entry"
    );
    assert_eq!(
        first.body["body"]["orderIntent"]["plugin"]["operationId"],
        "broker.paper_order_submit"
    );

    let duplicate = evaluate_tick(&service, &activation, "tick-true-entry", 1, observation);
    assert_eq!(duplicate.status, 200, "{:#}", duplicate.body);
    assert_eq!(duplicate.body["duplicate"], true);
    assert_eq!(
        duplicate.body["body"]["orderIntent"]["clientOrderId"],
        first.body["body"]["orderIntent"]["clientOrderId"]
    );

    let orders = orders_for_activation(
        &service,
        activation["activationId"].as_str().expect("activation id"),
    );
    assert_eq!(
        orders.len(),
        1,
        "duplicate tick submitted another order: {orders:#?}"
    );
    assert_eq!(orders[0]["qty"], 0.00017);

    let duplicate_event = evaluate_tick(
        &service,
        &activation,
        "tick-duplicate-event",
        2,
        source_bound_sim_bar(&activation, "sim-bar-true-entry", 102.0, 1_784_000_120_000),
    );
    assert_eq!(duplicate_event.status, 422, "{:#}", duplicate_event.body);
    assert_eq!(
        duplicate_event.body["error"]["details"]["reason"],
        "observation_event_duplicate"
    );

    let regressing_event_time = evaluate_tick(
        &service,
        &activation,
        "tick-regressing-event-time",
        2,
        source_bound_sim_bar(
            &activation,
            "sim-bar-regressing-event-time",
            102.0,
            1_784_000_000_000,
        ),
    );
    assert_eq!(
        regressing_event_time.status, 422,
        "{:#}",
        regressing_event_time.body
    );
    assert_eq!(
        regressing_event_time.body["error"]["details"]["reason"],
        "observation_event_time_not_monotonic"
    );

    let stale = evaluate_tick(
        &service,
        &activation,
        "tick-stale",
        0,
        source_bound_sim_bar(&activation, "sim-bar-stale", 102.0, 1_784_000_030_000),
    );
    assert_eq!(stale.status, 409, "{:#}", stale.body);
    assert_eq!(stale.body["error"]["code"], "stale_tick");
    assert_eq!(
        orders_for_activation(
            &service,
            activation["activationId"].as_str().expect("activation id")
        )
        .len(),
        1,
        "a stale tick submitted another order"
    );

    let future = evaluate_tick(
        &service,
        &activation,
        "tick-future",
        3,
        source_bound_sim_bar(&activation, "sim-bar-future", 102.0, 1_784_000_180_000),
    );
    assert_eq!(future.status, 409, "{:#}", future.body);
    assert_eq!(future.body["error"]["code"], "out_of_order_tick");
    assert_eq!(future.body["error"]["details"]["expectedSequence"], 2);
    assert_eq!(
        orders_for_activation(
            &service,
            activation["activationId"].as_str().expect("activation id")
        )
        .len(),
        1,
        "an out-of-order future tick submitted another order"
    );
}

#[test]
fn caller_supplied_market_observation_without_durable_plugin_receipt_fails_closed() {
    let db = test_db("forged-observation");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "forged-observation");
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let forged = source_bound_sim_bar(&activation, "forged-market-event", 101.0, 1_784_000_060_000);

    let response = service.handle_http_from_source(
        "scheduler",
        "POST",
        &EVALUATE_TICK_PATH.replace("{activation_id}", activation_id),
        json!({
            "tickId": "tick-forged-observation",
            "sequence": 1,
            "strategyVersionId": activation["run"]["strategyVersionId"],
            "observation": forged,
            "idempotencyKey": format!("{activation_id}:forged"),
            "accountMode": "paper",
            "authorityContext": {
                "actor": "local-user",
                "surface": "scheduler",
                "accountMode": "paper",
            },
        }),
    );

    assert_eq!(response.status, 422, "{:#}", response.body);
    assert_eq!(
        response.body["error"]["details"]["reason"],
        "observation_receipt_missing"
    );
    assert!(orders_for_activation(&service, activation_id).is_empty());
}

#[test]
fn risk_reservation_is_durable_before_broker_plugin_submission() {
    let db = test_db("risk-before-submission");
    let _ = std::fs::remove_file(&db);
    let (service, observed_reservation) = service_with_risk_ordering_assertion(&db);
    let activation = activate(&service, "risk-before-submission");

    let response = evaluate_tick(
        &service,
        &activation,
        "tick-risk-before-submission",
        1,
        source_bound_sim_bar(
            &activation,
            "sim-bar-risk-before-submission",
            101.0,
            1_784_000_060_000,
        ),
    );

    assert_eq!(response.status, 200, "{:#}", response.body);
    assert_eq!(
        response.body["body"]["orderResult"]["status"], "completed",
        "{:#}",
        response.body
    );
    assert!(observed_reservation.load(Ordering::SeqCst));
    assert_eq!(
        namespace_values(&service, "risk_reservations").len(),
        1,
        "the order must have exactly one durable reservation"
    );
}

#[test]
fn plugin_reconciliation_required_outcome_never_becomes_a_fill() {
    let db = test_db("plugin-reconciliation-required");
    let _ = std::fs::remove_file(&db);
    let service = service_with_reconciliation_required_order(&db);
    let activation = activate(&service, "plugin-reconciliation-required");
    let activation_id = activation["activationId"].as_str().expect("activation id");

    let response = evaluate_tick(
        &service,
        &activation,
        "tick-plugin-reconciliation-required",
        1,
        source_bound_sim_bar(
            &activation,
            "sim-bar-plugin-reconciliation-required",
            101.0,
            1_784_000_060_000,
        ),
    );

    assert_eq!(response.status, 200, "{:#}", response.body);
    assert_eq!(
        response.body["body"]["orderResult"]["error"],
        "plugin_order_reconciliation_required"
    );
    let orders = orders_for_activation(&service, activation_id);
    assert_eq!(orders.len(), 1, "{orders:#?}");
    assert_eq!(orders[0]["status"], "reconciliation_required");
    assert!(namespace_values(&service, "positions").is_empty());
    let reconciliation = service
        .runtime()
        .storage
        .get_json("execution_reconciliation", activation_id)
        .expect("read reconciliation")
        .expect("reconciliation required");
    assert_eq!(reconciliation["state"], "required");
    assert_eq!(reconciliation["newEntriesPaused"], true);
}

#[test]
fn restart_resumes_the_activation_from_its_durable_checkpoint() {
    let db = test_db("restart-checkpoint");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "restart-checkpoint");
    let first = evaluate_tick(
        &service,
        &activation,
        "tick-before-restart",
        1,
        source_bound_sim_bar(
            &activation,
            "sim-bar-before-restart",
            100.0,
            1_784_000_120_000,
        ),
    );
    assert_eq!(first.status, 200, "{:#}", first.body);
    let checkpoint_id = first.body["body"]["checkpoint"]["checkpointId"].clone();
    let checkpoint_hash = first.body["body"]["checkpoint"]["checkpointHash"].clone();

    let restarted = TradeAssemblyService::test_local(&db);
    let resumed = evaluate_tick(
        &restarted,
        &activation,
        "tick-after-restart",
        2,
        source_bound_sim_bar(
            &activation,
            "sim-bar-after-restart",
            100.0,
            1_784_000_180_000,
        ),
    );
    assert_eq!(resumed.status, 200, "{:#}", resumed.body);
    assert_eq!(
        resumed.body["body"]["resumedFrom"]["checkpointId"],
        checkpoint_id
    );
    assert_eq!(
        resumed.body["body"]["resumedFrom"]["checkpointHash"],
        checkpoint_hash
    );
    assert_eq!(resumed.body["body"]["checkpoint"]["sequence"], 2);
}

#[test]
fn resume_restarts_a_deliberately_stopped_activation_and_its_scheduler() {
    let db = test_db("resume-stopped-activation");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "resume-stopped-activation");
    let activation_id = activation["activationId"].as_str().expect("activation id");

    let stopped = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "stop"}),
    );
    assert_eq!(stopped.status, 200, "{:#}", stopped.body);
    let resumed = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "resume_entries"}),
    );
    assert_eq!(resumed.status, 200, "{:#}", resumed.body);

    let run = service
        .runtime()
        .storage
        .get_json("execution_runs", &format!("run_{activation_id}"))
        .expect("read run")
        .expect("execution run");
    assert_eq!(run["state"], "active");
    assert_eq!(run["health"]["status"], "running");
    let health = service
        .runtime()
        .storage
        .get_json("execution_health", activation_id)
        .expect("read health")
        .expect("execution health");
    assert_eq!(health["status"], "healthy");
    let scheduler = service.handle_http("GET", "/scheduler/status", json!({}));
    assert_eq!(scheduler.status, 200, "{:#}", scheduler.body);
    assert_eq!(scheduler.body["running"], true);
    assert_eq!(scheduler.body["activationId"], activation_id);
}

#[test]
fn malformed_or_unbound_plugin_evidence_fails_closed_before_any_order_intent() {
    let db = test_db("plugin-evidence-fails-closed");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "plugin-evidence-fails-closed");
    let activation_id = activation["activationId"].as_str().expect("activation id");

    let mut malformed =
        source_bound_sim_bar(&activation, "sim-bar-malformed", 101.0, 1_784_000_240_000);
    malformed["source"]["manifestFingerprint"] = json!("forged-manifest-fingerprint");
    malformed["bar"]["close"] = Value::Null;
    let response = evaluate_tick(&service, &activation, "tick-malformed", 1, malformed);

    assert_eq!(response.status, 422, "{:#}", response.body);
    assert_eq!(response.body["error"]["code"], "plugin_evidence_invalid");
    assert!(
        response.body["error"]["details"]["failClosed"].as_bool() == Some(true),
        "malformed or unbound evidence must report an explicit fail-closed outcome"
    );
    assert!(
        orders_for_activation(&service, activation_id).is_empty(),
        "malformed or unbound plugin evidence must not reach the order port"
    );
}

#[test]
fn entry_controls_do_not_suppress_exits_and_stop_after_flat_is_durable() {
    let db = test_db("entry-controls");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "entry-controls");
    let activation_id = activation["activationId"].as_str().expect("activation id");

    let paused = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "pause_entries"}),
    );
    assert_eq!(paused.status, 200, "{:#}", paused.body);
    let blocked = evaluate_tick(
        &service,
        &activation,
        "tick-entry-paused",
        1,
        source_bound_sim_bar(
            &activation,
            "sim-bar-entry-paused",
            101.0,
            1_784_000_300_000,
        ),
    );
    assert_eq!(blocked.status, 200, "{:#}", blocked.body);
    assert_eq!(blocked.body["body"]["decision"]["kind"], "entry_blocked");
    assert!(orders_for_activation(&service, activation_id).is_empty());

    let resumed = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "resume_entries"}),
    );
    assert_eq!(resumed.status, 200, "{:#}", resumed.body);
    let entered = evaluate_tick(
        &service,
        &activation,
        "tick-entry-resumed",
        2,
        source_bound_sim_bar(
            &activation,
            "sim-bar-entry-resumed",
            101.0,
            1_784_000_360_000,
        ),
    );
    assert_eq!(entered.status, 200, "{:#}", entered.body);
    assert_eq!(entered.body["body"]["decision"]["kind"], "order_intent");
    assert_eq!(orders_for_activation(&service, activation_id).len(), 1);

    let armed = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({"activationId": activation_id, "action": "stop_after_flat"}),
    );
    assert_eq!(armed.status, 200, "{:#}", armed.body);
    let exited = evaluate_tick(
        &service,
        &activation,
        "tick-exit-while-entry-blocked",
        3,
        source_bound_sim_bar(
            &activation,
            "sim-bar-exit-while-entry-blocked",
            98.0,
            1_784_000_420_000,
        ),
    );
    assert_eq!(exited.status, 200, "{:#}", exited.body);
    assert_eq!(exited.body["body"]["decision"]["kind"], "order_intent");
    assert_eq!(exited.body["body"]["orderIntent"]["side"], "sell");
    assert_eq!(orders_for_activation(&service, activation_id).len(), 2);
    let orders = orders_for_activation(&service, activation_id);
    assert!(orders
        .iter()
        .all(|order| order["fill_price"].as_f64().is_some()));
    let positions = namespace_values(&service, "positions")
        .into_iter()
        .filter(|position| position["activation_id"] == activation_id)
        .collect::<Vec<_>>();
    assert_eq!(positions.len(), 1, "{positions:#?}");
    assert_eq!(positions[0]["status"], "closed");
    assert_eq!(positions[0]["qty"], 0.0);
    assert_eq!(positions[0]["average_price"], 101.0);
    assert_eq!(positions[0]["mark_price"], 98.0);
    assert_eq!(positions[0]["realized_pnl"], -0.00051);

    let dashboard = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo", "activationId": activation_id}),
    );
    assert_eq!(dashboard.status, 200, "{:#}", dashboard.body);
    assert_eq!(
        dashboard.body["execution"]["positions"]["summary"]["openPositions"],
        0
    );
    assert_eq!(
        dashboard.body["positionLifecycle"]["summary"]["status"],
        "flat"
    );
    assert_eq!(dashboard.body["performanceSummary"]["active_positions"], 0);
    assert_eq!(
        dashboard.body["execution"]["executionAnalytics"]["capacityLiquidity"]["activePositions"],
        0
    );
    assert_eq!(
        dashboard.body["execution"]["ledger"]["totals"]["openTrades"],
        0
    );
    assert_eq!(
        dashboard.body["execution"]["ledger"]["totals"]["realizedPnl"],
        -0.00051
    );

    let durable_positions = positions.clone();
    drop(service);
    let service = TradeAssemblyService::test_local(&db);
    let recovered = namespace_values(&service, "positions")
        .into_iter()
        .filter(|position| position["activation_id"] == activation_id)
        .collect::<Vec<_>>();
    assert_eq!(recovered, durable_positions);

    let workspace = service.handle_http(
        "POST",
        "/product/strategies/execution-workspace",
        json!({"strategyId": "strat_local_btc_demo"}),
    );
    let stopped = workspace.body["activeRuns"]
        .as_array()
        .expect("execution runs")
        .iter()
        .find(|run| run["activationId"] == activation_id)
        .expect("controlled activation");
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(stopped["stateReason"], "stop_after_flat_completed");
}

#[test]
fn activation_tick_watch_reconciliation_and_hash_chain_are_durable() {
    let db = test_db("durable-records-and-hash-chain");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "durable-records-and-hash-chain");
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let correlation_id = activation["run"]["correlationId"]
        .as_str()
        .expect("correlation id");

    let evaluated = evaluate_tick(
        &service,
        &activation,
        "tick-durable-records",
        1,
        source_bound_sim_bar(
            &activation,
            "sim-bar-durable-records",
            100.0,
            1_784_000_480_000,
        ),
    );
    assert_eq!(evaluated.status, 200, "{:#}", evaluated.body);

    let activation_records = namespace_values(&service, "execution_activations");
    assert_eq!(activation_records.len(), 1);
    assert_eq!(activation_records[0]["kind"], "ActivationRecord");
    assert_eq!(activation_records[0]["correlationId"], correlation_id);
    assert_eq!(activation_records[0]["lastCompletedSequence"], 1);
    assert_eq!(
        activation_records[0]["checkpointHash"],
        evaluated.body["body"]["checkpoint"]["checkpointHash"]
    );

    let watches = namespace_values(&service, "execution_watches");
    assert_eq!(watches.len(), 1);
    assert_eq!(watches[0]["kind"], "RuntimeWatch");
    assert_eq!(watches[0]["activationId"], activation_id);
    assert_eq!(watches[0]["correlationId"], correlation_id);
    assert_eq!(watches[0]["operationId"], "marketdata.bars.read_v1");

    let reconciliation = namespace_values(&service, "execution_reconciliation");
    assert_eq!(reconciliation.len(), 1);
    assert_eq!(reconciliation[0]["kind"], "ReconciliationState");
    assert_eq!(reconciliation[0]["state"], "clear");
    assert_eq!(reconciliation[0]["correlationId"], correlation_id);

    let ticks = namespace_values(&service, "execution_ticks");
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0]["kind"], "EvaluationTickRecord");
    assert_eq!(ticks[0]["status"], "completed");
    assert_eq!(ticks[0]["correlationId"], correlation_id);
    assert_eq!(
        ticks[0]["sourceObservationIds"],
        json!(["sim-bar-durable-records"])
    );

    let mut events = namespace_values(&service, "execution_events")
        .into_iter()
        .filter(|event| event["activationId"] == activation_id)
        .collect::<Vec<_>>();
    events.sort_by_key(|event| event["sequence"].as_u64().unwrap_or_default());
    assert!(events.len() >= 2, "{events:#?}");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["correlationId"], correlation_id);
        assert_eq!(event["eventHash"], execution_event_hash(event));
        if index == 0 {
            assert!(event["previousHash"].is_null());
        } else {
            assert_eq!(event["previousHash"], events[index - 1]["eventHash"]);
        }
    }
    for namespace in [
        "execution_checkpoints",
        "plugin_operation_requests",
        "plugin_operation_receipts",
    ] {
        let records = namespace_values(&service, namespace);
        assert!(!records.is_empty(), "{namespace} has no evidence");
        assert!(
            records
                .iter()
                .all(|record| record["correlationId"] == correlation_id),
            "{namespace} lost the activation correlation: {records:#?}"
        );
    }
    let activation_descriptor = namespace_values(&service, "finance_action_descriptors")
        .into_iter()
        .find(|descriptor| descriptor["action_id"] == "execution.activate.paper")
        .expect("activation Warden descriptor");
    assert_eq!(activation_descriptor["correlation_id"], correlation_id);
    let serialized =
        serde_json::to_string(&[activation_records, watches, reconciliation, ticks, events])
            .expect("serialize correlated evidence");
    assert!(!serialized.contains("caller-forged"));
}

#[test]
fn concurrent_execution_events_append_without_overwrite_or_hash_chain_break() {
    let db = test_db("concurrent-event-append");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "concurrent-event-append");
    let activation_id = activation["activationId"]
        .as_str()
        .expect("activation id")
        .to_string();
    let before = namespace_values(&service, "execution_events")
        .into_iter()
        .filter(|event| event["activationId"] == activation_id)
        .count();
    let barrier = Arc::new(Barrier::new(3));
    let left_service = service.clone();
    let left_activation = activation_id.clone();
    let left_barrier = Arc::clone(&barrier);
    let left = std::thread::spawn(move || {
        left_barrier.wait();
        left_service.handle_http(
            "POST",
            "/product/strategy-execution-activations/control",
            json!({"activationId": left_activation, "action": "pause_entries"}),
        )
    });
    let right_service = service.clone();
    let right_activation = activation_id.clone();
    let right_barrier = Arc::clone(&barrier);
    let right = std::thread::spawn(move || {
        right_barrier.wait();
        right_service.handle_http(
            "POST",
            "/product/strategy-execution-activations/control",
            json!({"activationId": right_activation, "action": "stop_after_flat"}),
        )
    });
    barrier.wait();
    assert_eq!(left.join().expect("left control").status, 200);
    assert_eq!(right.join().expect("right control").status, 200);

    let mut events = namespace_values(&service, "execution_events")
        .into_iter()
        .filter(|event| event["activationId"] == activation_id)
        .collect::<Vec<_>>();
    events.sort_by_key(|event| event["sequence"].as_u64().unwrap_or_default());
    assert_eq!(events.len(), before + 2, "{events:#?}");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["sequence"], u64::try_from(index + 1).unwrap());
        assert_eq!(event["eventHash"], execution_event_hash(event));
        if index == 0 {
            assert!(event["previousHash"].is_null());
        } else {
            assert_eq!(event["previousHash"], events[index - 1]["eventHash"]);
        }
    }
}

#[test]
fn capability_drift_pauses_before_order_submission_and_records_the_blocked_tick() {
    let db = test_db("capability-drift");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "capability-drift");
    let activation_id = activation["activationId"].as_str().expect("activation id");

    for (method, path) in [
        ("POST", "/plugins/instances/sim:disable"),
        ("POST", "/plugins/instances/sim:upgrade"),
        ("POST", "/plugins/instances/sim:rollback"),
        ("DELETE", "/plugins/instances/sim"),
    ] {
        let blocked_transition = service.handle_http(method, path, json!({}));
        assert_eq!(
            blocked_transition.status, 409,
            "{method} {path}: {:#}",
            blocked_transition.body
        );
        assert_eq!(
            blocked_transition.body["error"]["code"], "plugin_instance_in_use",
            "{method} {path}: {:#}",
            blocked_transition.body
        );
        assert_eq!(
            blocked_transition.body["error"]["details"]["dependencies"][0]["activationId"],
            activation_id
        );
    }
    let current = service.handle_http("GET", "/plugins/instances/sim", json!({}));
    assert_eq!(current.status, 200, "{:#}", current.body);
    assert_eq!(current.body["enabled"], true);

    let run_id = activation["executionRunId"]
        .as_str()
        .expect("execution run id");
    let mut run = service
        .runtime()
        .storage
        .get_json("execution_runs", run_id)
        .expect("read execution run")
        .expect("execution run");
    run["state"] = json!("paused");
    service
        .runtime()
        .storage
        .put_json(
            "execution_runs",
            run_id,
            run.clone(),
            &storage_context("capability-drift-pause"),
        )
        .expect("pause execution run");
    let paused_transition = service.handle_http(
        "POST",
        "/plugins/instances/sim:disable",
        json!({"idempotencyKey": "capability-drift-paused-disable"}),
    );
    assert_eq!(
        paused_transition.status, 409,
        "{:#}",
        paused_transition.body
    );
    assert_eq!(
        paused_transition.body["error"]["details"]["dependencies"][0]["state"],
        "paused"
    );

    run["state"] = json!("stopped");
    service
        .runtime()
        .storage
        .put_json(
            "execution_runs",
            run_id,
            run.clone(),
            &storage_context("capability-drift-stop"),
        )
        .expect("stop execution run");
    let terminal_transition = service.handle_http(
        "POST",
        "/plugins/instances/sim:disable",
        json!({"idempotencyKey": "capability-drift-stopped-disable"}),
    );
    assert_eq!(
        terminal_transition.status, 200,
        "{:#}",
        terminal_transition.body
    );
    let reenabled = service.handle_http(
        "POST",
        "/plugins/instances/sim:enable",
        json!({"idempotencyKey": "capability-drift-reenable"}),
    );
    assert_eq!(reenabled.status, 200, "{:#}", reenabled.body);

    run["state"] = json!("active");
    service
        .runtime()
        .storage
        .put_json(
            "execution_runs",
            run_id,
            run,
            &storage_context("capability-drift-reactivate"),
        )
        .expect("reactivate execution run fixture");

    let mut disabled = reenabled.body["instance"].clone();
    disabled["enabled"] = json!(false);
    disabled["health"]["state"] = json!("disabled");
    service
        .runtime()
        .storage
        .put_json(
            "plugin_instances_v2",
            "sim",
            disabled,
            &storage_context("capability-drift-direct-mutation"),
        )
        .expect("simulate out-of-band capability drift");
    let blocked = evaluate_tick(
        &service,
        &activation,
        "tick-capability-drift",
        1,
        source_bound_sim_bar(
            &activation,
            "sim-bar-capability-drift",
            101.0,
            1_784_000_540_000,
        ),
    );
    assert_eq!(blocked.status, 409, "{:#}", blocked.body);
    assert_eq!(blocked.body["error"]["code"], "capability_revision_stale");
    assert!(orders_for_activation(&service, activation_id).is_empty());

    let reconciliation = service
        .runtime()
        .storage
        .get_json("execution_reconciliation", activation_id)
        .expect("read reconciliation")
        .expect("reconciliation state");
    assert_eq!(reconciliation["state"], "required");
    assert_eq!(reconciliation["newEntriesPaused"], true);
    assert_eq!(reconciliation["reason"], "capability_revision_stale");

    let tick = namespace_values(&service, "execution_ticks")
        .into_iter()
        .find(|tick| tick["tickId"] == "tick-capability-drift")
        .expect("blocked tick record");
    assert_eq!(tick["status"], "capability_blocked");
    assert_eq!(tick["decision"]["code"], "capability_revision_stale");
}

#[test]
fn scheduler_fail_closed_cycle_advances_without_effects_and_recovers_next_cycle() {
    let db = test_db("capability-drift-scheduler-recovery");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "capability-drift-scheduler-recovery");
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let run_id = activation["executionRunId"]
        .as_str()
        .expect("execution run id");

    let current_instance = service.handle_http("GET", "/plugins/instances/sim", json!({}));
    assert_eq!(current_instance.status, 200, "{:#}", current_instance.body);
    let original_instance = current_instance.body;
    let mut disabled = original_instance.clone();
    disabled["enabled"] = json!(false);
    disabled["health"]["state"] = json!("disabled");
    service
        .runtime()
        .storage
        .put_json(
            "plugin_instances_v2",
            "sim",
            disabled,
            &storage_context("capability-drift-scheduler-disable"),
        )
        .expect("simulate transient capability drift");

    let blocked = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-capability-drift-blocked",
            "maxCycles": 1,
            "leaseTtlSeconds": 30,
        }),
    );
    assert_eq!(blocked.status, 200, "{:#}", blocked.body);
    assert_eq!(blocked.body["body"]["ticks"][0]["status"], "blocked");
    assert_eq!(
        blocked.body["body"]["ticks"][0]["reason"],
        "capability_revision_stale"
    );
    assert_eq!(blocked.body["body"]["ticks"][0]["effectsApplied"], false);
    assert_eq!(blocked.body["body"]["ticks"][0]["willRetryNextCycle"], true);
    assert!(orders_for_activation(&service, activation_id).is_empty());

    let blocked_run = service
        .runtime()
        .storage
        .get_json("execution_runs", run_id)
        .expect("read blocked run")
        .expect("blocked run");
    assert_eq!(blocked_run["state"], "active");
    assert_eq!(blocked_run["cycle"], 1);
    assert_eq!(
        blocked_run["checkpoint"]["reason"],
        "capability_revision_stale"
    );
    assert_eq!(
        blocked_run["checkpoint"]["state"]["failClosed"]["effectsApplied"],
        false
    );
    let blocked_attempt = namespace_values(&service, "execution_attempts")
        .into_iter()
        .find(|attempt| attempt["cycle"] == 1)
        .expect("blocked attempt");
    assert_eq!(blocked_attempt["state"], "blocked");
    assert_eq!(blocked_attempt["effectsApplied"], false);
    let blocked_scheduler = service.handle_http("GET", "/scheduler/status", json!({}));
    assert_eq!(
        blocked_scheduler.status, 200,
        "{:#}",
        blocked_scheduler.body
    );
    assert_eq!(blocked_scheduler.body["nextCycle"], 2);

    service
        .runtime()
        .storage
        .put_json(
            "plugin_instances_v2",
            "sim",
            original_instance,
            &storage_context("capability-drift-scheduler-restore"),
        )
        .expect("restore plugin capability");
    let recovered = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-capability-drift-recovered",
            "maxCycles": 2,
            "leaseTtlSeconds": 30,
        }),
    );
    assert_eq!(recovered.status, 200, "{:#}", recovered.body);
    assert!(
        recovered.body["body"]["ticks"]
            .as_array()
            .expect("scheduler ticks")
            .iter()
            .any(|tick| tick["status"] == "complete" && tick["sequence"] == 2),
        "{:#}",
        recovered.body
    );

    let recovered_run = service
        .runtime()
        .storage
        .get_json("execution_runs", run_id)
        .expect("read recovered run")
        .expect("recovered run");
    assert_eq!(recovered_run["state"], "active");
    assert_eq!(recovered_run["cycle"], 2);
    assert_eq!(recovered_run["stateReason"], "evaluation_completed");
    let reconciliation = service
        .runtime()
        .storage
        .get_json("execution_reconciliation", activation_id)
        .expect("read reconciliation")
        .expect("reconciliation");
    assert_eq!(reconciliation["state"], "clear");
    assert_eq!(reconciliation["newEntriesPaused"], false);
    assert!(orders_for_activation(&service, activation_id).is_empty());
}

#[test]
fn saving_execution_config_materializes_the_seed_strategy_version() {
    let db = test_db("materialized-seed-version");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    assert!(namespace_values(&service, "strategy_versions").is_empty());

    let activation = activate(&service, "materialized-seed-version");
    let versions = namespace_values(&service, "strategy_versions");
    assert_eq!(versions.len(), 1, "{versions:#?}");
    assert_eq!(versions[0]["id"], activation["run"]["strategyVersionId"]);
    assert_eq!(
        versions[0]["specHash"],
        activation["run"]["strategySpecHash"]
    );
    assert_eq!(versions[0]["immutable"], true);
}

#[test]
fn risk_and_reconciliation_blocks_are_factual_terminal_decisions() {
    let db = test_db("risk-and-reconciliation-blocks");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate_with_risk(&service, "risk-and-reconciliation-blocks", 0.001, 0.00017);
    let activation_id = activation["activationId"].as_str().expect("activation id");

    let risk_blocked = evaluate_tick(
        &service,
        &activation,
        "tick-risk-blocked",
        1,
        source_bound_sim_bar(
            &activation,
            "sim-bar-risk-blocked",
            101.0,
            1_784_000_600_000,
        ),
    );
    assert_eq!(risk_blocked.status, 200, "{:#}", risk_blocked.body);
    assert_eq!(
        risk_blocked.body["body"]["decision"]["kind"],
        "risk_blocked"
    );
    assert_eq!(
        risk_blocked.body["body"]["orderResult"]["reason"],
        "execution_risk_limit_blocked"
    );
    assert!(orders_for_activation(&service, activation_id).is_empty());

    service
        .runtime()
        .storage
        .put_json(
            "execution_reconciliation",
            activation_id,
            json!({
                "schemaVersion": "tradeassembly.execution_reconciliation.v1",
                "kind": "ReconciliationState",
                "activationId": activation_id,
                "state": "required",
                "pendingActions": [],
                "ambiguousActions": [{"clientOrderId": "unknown-outcome"}],
                "newEntriesPaused": true,
            }),
            &storage_context("test:reconciliation-uncertainty"),
        )
        .expect("inject reconciliation uncertainty");

    let reconciliation_blocked = evaluate_tick(
        &service,
        &activation,
        "tick-reconciliation-blocked",
        2,
        source_bound_sim_bar(
            &activation,
            "sim-bar-reconciliation-blocked",
            101.0,
            1_784_000_660_000,
        ),
    );
    assert_eq!(
        reconciliation_blocked.status, 200,
        "{:#}",
        reconciliation_blocked.body
    );
    assert_eq!(
        reconciliation_blocked.body["body"]["decision"]["kind"],
        "reconciliation_blocked"
    );
    assert_eq!(
        reconciliation_blocked.body["body"]["orderResult"]["reason"],
        "reconciliation_required"
    );
    assert!(orders_for_activation(&service, activation_id).is_empty());

    let ticks = namespace_values(&service, "execution_ticks");
    assert!(ticks.iter().any(|tick| {
        tick["tickId"] == "tick-risk-blocked"
            && tick["decision"]["kind"] == "risk_blocked"
            && tick["status"] == "completed"
    }));
    assert!(ticks.iter().any(|tick| {
        tick["tickId"] == "tick-reconciliation-blocked"
            && tick["decision"]["kind"] == "reconciliation_blocked"
            && tick["status"] == "completed"
    }));
}

#[test]
fn distinct_strategies_symbols_checkpoints_and_order_identities_never_share_state() {
    let db = test_db("strategy-symbol-isolation");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let btc = activate(&service, "strategy-symbol-isolation-btc");
    let eth_strategy = duplicate_published_strategy(&service, "ETH continuous paper", "ETH/USD");
    let eth = activate_strategy(
        &service,
        &eth_strategy,
        "ETH/USD",
        "strategy-symbol-isolation-eth",
    );
    let btc_activation = btc["activationId"].as_str().expect("BTC activation");
    let eth_activation = eth["activationId"].as_str().expect("ETH activation");
    assert_ne!(btc_activation, eth_activation);

    let btc_tick = evaluate_tick(
        &service,
        &btc,
        "tick-btc-isolated",
        1,
        source_bound_sim_bar_for_symbol(
            &btc,
            "sim-bar-btc-isolated",
            "BTC/USD",
            101.0,
            1_784_000_720_000,
        ),
    );
    assert_eq!(btc_tick.status, 200, "{:#}", btc_tick.body);
    assert_eq!(btc_tick.body["body"]["decision"]["kind"], "order_intent");

    let eth_tick = evaluate_tick(
        &service,
        &eth,
        "tick-eth-isolated",
        1,
        source_bound_sim_bar_for_symbol(
            &eth,
            "sim-bar-eth-isolated",
            "ETH/USD",
            100.0,
            1_784_000_720_000,
        ),
    );
    assert_eq!(eth_tick.status, 200, "{:#}", eth_tick.body);
    assert_eq!(eth_tick.body["body"]["decision"]["kind"], "no_signal");

    let btc_orders = orders_for_activation(&service, btc_activation);
    let eth_orders = orders_for_activation(&service, eth_activation);
    assert_eq!(btc_orders.len(), 1);
    assert_eq!(btc_orders[0]["symbol"], "BTC/USD");
    assert!(eth_orders.is_empty());

    let ticks = namespace_values(&service, "execution_ticks");
    let btc_record = ticks
        .iter()
        .find(|tick| tick["activationId"] == btc_activation)
        .expect("BTC tick");
    let eth_record = ticks
        .iter()
        .find(|tick| tick["activationId"] == eth_activation)
        .expect("ETH tick");
    assert_eq!(
        btc_record["sourceObservationIds"],
        json!(["sim-bar-btc-isolated"])
    );
    assert_eq!(
        eth_record["sourceObservationIds"],
        json!(["sim-bar-eth-isolated"])
    );
    assert_ne!(btc_record["checkpointId"], eth_record["checkpointId"]);

    let activations = namespace_values(&service, "execution_activations");
    let btc_root = activations
        .iter()
        .find(|record| record["activationId"] == btc_activation)
        .expect("BTC activation root");
    let eth_root = activations
        .iter()
        .find(|record| record["activationId"] == eth_activation)
        .expect("ETH activation root");
    assert_eq!(btc_root["symbol"], "BTC/USD");
    assert_eq!(eth_root["symbol"], "ETH/USD");
    assert_ne!(btc_root["strategyId"], eth_root["strategyId"]);
}

#[test]
fn bounded_virtual_time_soak_survives_restart_without_duplicate_side_effects() {
    let db = test_db("virtual-time-soak");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "virtual-time-soak");
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let correlation_id = activation["run"]["correlationId"]
        .as_str()
        .expect("correlation id")
        .to_string();

    let first_half = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-soak-a",
            "maxCycles": 32,
            "leaseTtlSeconds": 30,
        }),
    );
    assert_eq!(first_half.status, 200, "{:#}", first_half.body);
    assert_eq!(first_half.body["body"]["iterations"], 32);
    assert_eq!(
        first_half.body["body"]["ticks"].as_array().map(Vec::len),
        Some(32)
    );
    drop(service);

    let restarted = TradeAssemblyService::test_local(&db);
    let second_half = restarted.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-soak-b",
            "maxCycles": 64,
            "leaseTtlSeconds": 30,
        }),
    );
    assert_eq!(second_half.status, 200, "{:#}", second_half.body);
    assert_eq!(second_half.body["body"]["iterations"], 64);
    assert_eq!(
        second_half.body["body"]["ticks"].as_array().map(Vec::len),
        Some(64)
    );
    assert_eq!(
        second_half.body["body"]["ticks"][0]["status"], "idle",
        "{:#}",
        second_half.body
    );
    assert_eq!(
        second_half.body["body"]["ticks"][31]["status"], "idle",
        "{:#}",
        second_half.body
    );
    assert_eq!(
        second_half.body["body"]["ticks"][32]["status"], "complete",
        "{:#}",
        second_half.body
    );

    let activation_record = restarted
        .runtime()
        .storage
        .get_json("execution_activations", activation_id)
        .expect("read activation")
        .expect("activation root");
    assert_eq!(activation_record["lastCompletedSequence"], 64);

    let ticks = namespace_values(&restarted, "execution_ticks")
        .into_iter()
        .filter(|tick| tick["activationId"] == activation_id)
        .collect::<Vec<_>>();
    assert_eq!(ticks.len(), 64);
    assert!(ticks.iter().all(|tick| tick["status"] == "completed"));
    assert!(ticks
        .iter()
        .all(|tick| tick["correlationId"] == correlation_id));

    let attempts = namespace_values(&restarted, "execution_attempts")
        .into_iter()
        .filter(|attempt| attempt["activationId"] == activation_id)
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 64);
    assert!(attempts
        .iter()
        .all(|attempt| attempt["state"] == "completed"));
    assert!(attempts
        .iter()
        .all(|attempt| attempt["correlationId"] == correlation_id));
    assert!(attempts
        .iter()
        .any(|attempt| attempt["owner"] == "worker-soak-a"));
    assert!(attempts
        .iter()
        .any(|attempt| attempt["owner"] == "worker-soak-b"));

    let orders = orders_for_activation(&restarted, activation_id);
    assert!(!orders.is_empty(), "{orders:#?}");
    assert!(orders.len() < 64, "{orders:#?}");
    assert!(orders
        .iter()
        .all(|order| order["correlationId"] == correlation_id));
    let client_ids = orders
        .iter()
        .map(|order| order["client_order_id"].as_str().expect("client order id"))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(client_ids.len(), orders.len());
    assert!(orders.iter().any(|order| order["side"] == "buy"));
    assert!(orders.iter().any(|order| order["side"] == "sell"));
    let plugin_receipts = namespace_values(&restarted, "plugin_operation_receipts");
    assert!(!plugin_receipts.is_empty());
    assert!(plugin_receipts
        .iter()
        .all(|receipt| receipt["correlationId"] == correlation_id));
}

#[test]
fn continuous_and_session_calendars_gate_evaluation_from_bound_event_time() {
    let db = test_db("calendar-gating");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);

    let continuous = activate(&service, "calendar-continuous");
    let weekend_ms = 1_784_991_600_000_i64;
    let continuous_tick = evaluate_tick(
        &service,
        &continuous,
        "tick-continuous-weekend",
        1,
        source_bound_sim_bar(&continuous, "sim-bar-continuous-weekend", 101.0, weekend_ms),
    );
    assert_eq!(continuous_tick.status, 200, "{:#}", continuous_tick.body);
    assert_eq!(
        continuous_tick.body["body"]["decision"]["facts"]["calendar"]["calendarRef"],
        "CRYPTO_24X7"
    );
    assert_eq!(
        continuous_tick.body["body"]["decision"]["facts"]["calendar"]["eligible"],
        true
    );
    assert_eq!(
        continuous_tick.body["body"]["decision"]["kind"],
        "order_intent"
    );

    let session_strategy = duplicate_published_strategy_with_calendar(
        &service,
        "Session calendar strategy",
        "SESSION/USD",
        Some("XNYS"),
    );
    let session = activate_strategy(
        &service,
        &session_strategy,
        "SESSION/USD",
        "calendar-session",
    );
    assert_eq!(session["run"]["calendarRef"], "XNYS");

    let closed = evaluate_tick(
        &service,
        &session,
        "tick-session-closed",
        1,
        source_bound_sim_bar_for_symbol(
            &session,
            "sim-bar-session-closed",
            "SESSION/USD",
            101.0,
            weekend_ms,
        ),
    );
    assert_eq!(closed.status, 200, "{:#}", closed.body);
    assert_eq!(closed.body["body"]["decision"]["kind"], "market_closed");
    assert_eq!(
        closed.body["body"]["decision"]["facts"]["calendar"]["sessionState"],
        "closed"
    );
    assert_eq!(
        closed.body["body"]["decision"]["facts"]["calendar"]["calendarId"],
        "XNYS"
    );
    assert_eq!(
        closed.body["body"]["decision"]["facts"]["calendar"]["timezone"],
        "America/New_York"
    );
    assert_eq!(
        closed.body["body"]["decision"]["facts"]["calendar"]["nextTransitionKind"],
        "open"
    );
    assert!(
        closed.body["body"]["decision"]["facts"]["calendar"]["nextTransitionAtMs"]
            .as_i64()
            .is_some()
    );
    assert_eq!(
        closed.body["body"]["orderResult"]["reason"],
        "market_closed"
    );
    assert!(orders_for_activation(
        &service,
        session["activationId"].as_str().expect("activation id")
    )
    .is_empty());

    // 2026-07-27 09:45 America/New_York is 13:45 UTC during daylight time.
    let weekday_open_ms = 1_785_159_900_000_i64;
    let open = evaluate_tick(
        &service,
        &session,
        "tick-session-open",
        2,
        source_bound_sim_bar_for_symbol(
            &session,
            "sim-bar-session-open",
            "SESSION/USD",
            101.0,
            weekday_open_ms,
        ),
    );
    assert_eq!(open.status, 200, "{:#}", open.body);
    assert_eq!(open.body["body"]["decision"]["kind"], "order_intent");
    assert_eq!(
        open.body["body"]["decision"]["facts"]["calendar"]["sessionState"],
        "open"
    );
    assert_eq!(
        open.body["body"]["decision"]["facts"]["calendar"]["nextTransitionKind"],
        "close"
    );
    assert_eq!(
        orders_for_activation(
            &service,
            session["activationId"].as_str().expect("activation id")
        )
        .len(),
        1
    );
}

#[test]
fn scheduler_rejects_invalid_plugin_market_evidence_before_any_order() {
    for (label, corruption) in [
        ("schema", MarketEvidenceCorruption::Schema),
        ("freshness", MarketEvidenceCorruption::Freshness),
        ("instrument", MarketEvidenceCorruption::Instrument),
        ("evidence", MarketEvidenceCorruption::Evidence),
    ] {
        let db = test_db(&format!("invalid-market-{label}"));
        let _ = std::fs::remove_file(&db);
        let service = service_with_market_corruption(&db, corruption);
        let activation = activate(&service, &format!("invalid-market-{label}"));
        let activation_id = activation["activationId"].as_str().expect("activation id");

        let result = service.handle_http_from_source(
            "scheduler",
            "POST",
            "/scheduler/run",
            json!({
                "activationId": activation_id,
                "workerId": format!("worker-invalid-market-{label}"),
                "maxCycles": 1,
                "leaseTtlSeconds": 30,
            }),
        );
        assert_eq!(result.status, 200, "{:#}", result.body);
        assert_eq!(result.body["body"]["ticks"][0]["status"], "blocked");
        assert_eq!(
            result.body["body"]["ticks"][0]["reason"],
            "market_data_plugin_evidence_invalid"
        );
        assert_eq!(result.body["body"]["ticks"][0]["effectsApplied"], false);
        assert_eq!(result.body["body"]["ticks"][0]["willRetryNextCycle"], true);
        assert!(orders_for_activation(&service, activation_id).is_empty());

        let attempts = namespace_values(&service, "execution_attempts")
            .into_iter()
            .filter(|attempt| attempt["activationId"] == activation_id)
            .collect::<Vec<_>>();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0]["state"], "blocked");
        assert_eq!(
            attempts[0]["blockedReason"],
            "market_data_plugin_evidence_invalid"
        );
        assert_eq!(attempts[0]["effectsApplied"], false);

        let run = service
            .runtime()
            .storage
            .get_json("execution_runs", &format!("run_{activation_id}"))
            .expect("read active run")
            .expect("active run");
        assert_eq!(run["state"], "active");
        assert_eq!(run["cycle"], 1);

        let ticks = namespace_values(&service, "execution_ticks")
            .into_iter()
            .filter(|tick| tick["activationId"] == activation_id)
            .collect::<Vec<_>>();
        assert_eq!(ticks.len(), 1, "{ticks:#?}");
        assert_eq!(ticks[0]["status"], "plugin_evidence_blocked");
        assert_eq!(
            ticks[0]["decision"]["code"],
            "market_data_plugin_evidence_invalid"
        );
    }
}

#[test]
fn broker_failures_before_and_after_submission_recover_the_same_order_by_reconciliation() {
    for (label, failure_point) in [
        ("before", BrokerFailurePoint::BeforeSubmission),
        ("after", BrokerFailurePoint::AfterSubmission),
    ] {
        let db = test_db(&format!("broker-recovery-{label}"));
        let _ = std::fs::remove_file(&db);
        let (service, broker_calls) = service_with_one_broker_failure(&db, failure_point);
        let activation = activate(&service, &format!("broker-recovery-{label}"));
        let activation_id = activation["activationId"].as_str().expect("activation id");

        let failed_attempt = service.handle_http_from_source(
            "scheduler",
            "POST",
            "/scheduler/run",
            json!({
                "activationId": activation_id,
                "workerId": format!("worker-{label}-first"),
                "maxCycles": 1,
                "leaseTtlSeconds": 30,
            }),
        );
        assert_eq!(failed_attempt.status, 200, "{:#}", failed_attempt.body);
        assert_eq!(
            failed_attempt.body["body"]["ticks"][0]["decision"]["kind"], "error",
            "{:#}",
            failed_attempt.body
        );
        assert!(!failed_attempt
            .body
            .to_string()
            .contains("must-never-be-persisted"));
        let required = service
            .runtime()
            .storage
            .get_json("execution_reconciliation", activation_id)
            .expect("read required reconciliation")
            .expect("required reconciliation state");
        assert_eq!(required["state"], "required");
        assert_eq!(
            required["ambiguousActions"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            required["ambiguousActions"][0]["orderRequest"]["pluginOperationRequest"]["input"]
                ["assetClass"],
            "crypto"
        );

        let recovered = service.handle_http_from_source(
            "scheduler",
            "POST",
            "/scheduler/run",
            json!({
                "activationId": activation_id,
                "workerId": format!("worker-{label}-recovery"),
                "maxCycles": 2,
                "leaseTtlSeconds": 30,
            }),
        );
        assert_eq!(recovered.status, 200, "{:#}", recovered.body);
        assert_eq!(recovered.body["body"]["ticks"][0]["status"], "idle");
        assert_eq!(recovered.body["body"]["ticks"][1]["status"], "complete");

        let reconciliation = service
            .runtime()
            .storage
            .get_json("execution_reconciliation", activation_id)
            .expect("read resolved reconciliation")
            .expect("resolved reconciliation state");
        assert_eq!(reconciliation["state"], "clear");
        assert_eq!(reconciliation["newEntriesPaused"], false);
        assert_eq!(
            reconciliation["resolvedActions"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(broker_calls.load(Ordering::SeqCst), 2);

        let orders = orders_for_activation(&service, activation_id);
        assert_eq!(orders.len(), 1, "{orders:#?}");
        assert_eq!(orders[0]["status"], "filled");
        let receipts = namespace_values(&service, "plugin_operation_receipts")
            .into_iter()
            .filter(|receipt| receipt["schemaRef"] == "schema://broker/order-receipt@1")
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 1, "{receipts:#?}");

        let run = service
            .runtime()
            .storage
            .get_json("execution_runs", &format!("run_{activation_id}"))
            .expect("read reconciled run")
            .expect("reconciled run");
        assert_eq!(run["paperPositionMicros"], 170);
        assert_eq!(run["cycle"], 2);
        for namespace in [
            "execution_orders",
            "execution_order_attempts",
            "execution_runs",
            "execution_ticks",
            "execution_reconciliation",
            "execution_events",
        ] {
            assert!(
                !serde_json::to_string(&namespace_values(&service, namespace))
                    .expect("serialize execution evidence")
                    .contains("must-never-be-persisted"),
                "plugin failure detail leaked through {namespace}"
            );
        }
    }
}

#[test]
fn active_activation_reconciles_a_legacy_receipt_without_operator_intervention() {
    let db = test_db("legacy-receipt-reconciliation");
    let _ = std::fs::remove_file(&db);
    let service = service_with_legacy_receipt_until_rekey(&db);
    let activation = activate(&service, "legacy-receipt-reconciliation");
    let activation_id = activation["activationId"].as_str().expect("activation id");

    let failed = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-legacy-receipt",
            "maxCycles": 1,
            "leaseTtlSeconds": 30,
        }),
    );
    assert_eq!(failed.status, 200, "{:#}", failed.body);
    let required = service
        .runtime()
        .storage
        .get_json("execution_reconciliation", activation_id)
        .expect("read required reconciliation")
        .expect("required reconciliation state");
    assert_eq!(required["state"], "required");

    let recovered = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-legacy-receipt-recovery",
            "maxCycles": 2,
            "leaseTtlSeconds": 30,
        }),
    );
    assert_eq!(recovered.status, 200, "{:#}", recovered.body);
    assert_eq!(recovered.body["body"]["ticks"][0]["status"], "idle");
    assert_eq!(recovered.body["body"]["ticks"][1]["status"], "complete");

    let reconciliation = service
        .runtime()
        .storage
        .get_json("execution_reconciliation", activation_id)
        .expect("read reconciliation")
        .expect("reconciliation state");
    assert_eq!(reconciliation["state"], "clear");
    assert_eq!(reconciliation["newEntriesPaused"], false);
    let run = service
        .runtime()
        .storage
        .get_json("execution_runs", &format!("run_{activation_id}"))
        .expect("read run")
        .expect("execution run");
    assert_eq!(run["state"], "active");
    assert_eq!(run["paperPositionMicros"], 170);
    assert!(namespace_values(&service, "plugin_operation_receipts")
        .iter()
        .any(|receipt| receipt["schemaRef"] == "schema://broker/order-receipt@1"));
}

#[test]
fn expired_worker_lease_transfers_one_tick_and_one_side_effect_to_the_new_owner() {
    let db = test_db("lease-transfer");
    let _ = std::fs::remove_file(&db);
    let service = TradeAssemblyService::test_local(&db);
    let activation = activate(&service, "lease-transfer");
    let activation_id = activation["activationId"].as_str().expect("activation id");
    let now_ms = service.runtime().clock.now_ms();
    let held = service
        .runtime()
        .leases
        .acquire(
            &format!("execution-tick:{activation_id}:1"),
            "worker-original",
            now_ms,
            1_000,
        )
        .expect("acquire original worker lease")
        .expect("original worker owns lease");

    let contended = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-contender",
            "maxCycles": 1,
            "leaseTtlSeconds": 1,
        }),
    );
    assert_eq!(contended.status, 200, "{:#}", contended.body);
    assert_eq!(contended.body["body"]["ticks"][0]["status"], "contended");
    assert!(orders_for_activation(&service, activation_id).is_empty());

    let transferred = service.handle_http_from_source(
        "scheduler",
        "POST",
        "/scheduler/run",
        json!({
            "activationId": activation_id,
            "workerId": "worker-recovery",
            "maxCycles": 2,
            "leaseTtlSeconds": 1,
        }),
    );
    assert_eq!(transferred.status, 200, "{:#}", transferred.body);
    assert_eq!(transferred.body["body"]["ticks"][0]["status"], "idle");
    assert_eq!(transferred.body["body"]["ticks"][1]["status"], "complete");
    assert!(
        transferred.body["body"]["ticks"][1]["lease"]["fencingToken"]
            .as_i64()
            .expect("recovery fencing token")
            > held.fencing_token
    );

    let attempts = namespace_values(&service, "execution_attempts")
        .into_iter()
        .filter(|attempt| attempt["activationId"] == activation_id)
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 1, "{attempts:#?}");
    assert_eq!(attempts[0]["owner"], "worker-recovery");
    assert_eq!(attempts[0]["state"], "completed");
    assert_eq!(orders_for_activation(&service, activation_id).len(), 1);
}
