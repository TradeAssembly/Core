use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tempfile::TempDir;
use tradeassembly_runtime::finance_authority::FinanceAuthorityPort;
use tradeassembly_runtime::ports::{
    ClockPort, ComparePutOutcome, EvidencePort, EvidenceRecord, ImmutablePutOutcome,
    PortDescriptor, PortKind, ScheduledWork, SchedulerPort, SideEffectContext, StorageExpectation,
    StoragePort, StorageWrite, VersionedPort,
};
use tradeassembly_runtime::service::TradeAssemblyService;

struct FailingBatchStorage {
    delegate: Arc<dyn StoragePort>,
}

impl VersionedPort for FailingBatchStorage {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl StoragePort for FailingBatchStorage {
    fn adapter_name(&self) -> &'static str {
        "injected-failing-batch"
    }

    fn put_json(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        self.delegate.put_json(namespace, key, value, context)
    }

    fn put_json_if_absent(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
    ) -> Result<ImmutablePutOutcome, String> {
        self.delegate
            .put_json_if_absent(namespace, key, value, context)
    }

    fn compare_and_put_json(
        &self,
        namespace: &str,
        key: &str,
        expected: Value,
        replacement: Value,
        context: &SideEffectContext,
    ) -> Result<ComparePutOutcome, String> {
        self.delegate
            .compare_and_put_json(namespace, key, expected, replacement, context)
    }

    fn put_json_batch(
        &self,
        _writes: &[StorageWrite],
        _expectations: &[StorageExpectation],
    ) -> Result<ComparePutOutcome, String> {
        Err("injected secret path /tmp/tradeassembly.db token=hidden".to_string())
    }

    fn get_json(&self, namespace: &str, key: &str) -> Result<Option<Value>, String> {
        self.delegate.get_json(namespace, key)
    }

    fn list_json(&self, namespace: &str) -> Result<Vec<(String, Value)>, String> {
        self.delegate.list_json(namespace)
    }

    fn clear_namespace(&self, namespace: &str) -> Result<(), String> {
        self.delegate.clear_namespace(namespace)
    }
}

struct FailingEvidence {
    delegate: Arc<dyn EvidencePort>,
}

struct ObservedScheduler {
    delegate: Arc<dyn SchedulerPort>,
    schedules: Arc<AtomicUsize>,
}

impl VersionedPort for ObservedScheduler {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl SchedulerPort for ObservedScheduler {
    fn schedule(&self, work: ScheduledWork) -> Result<(), String> {
        self.schedules.fetch_add(1, Ordering::SeqCst);
        self.delegate.schedule(work)
    }

    fn cancel(&self, _schedule_key: &str) -> Result<bool, String> {
        Err("injected cancellation failure".to_string())
    }

    fn claim_key(
        &self,
        schedule_key: &str,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
    ) -> Result<Option<ScheduledWork>, String> {
        self.delegate
            .claim_key(schedule_key, owner, now_ms, lease_ms)
    }

    fn claim_due(
        &self,
        owner: &str,
        now_ms: i64,
        lease_ms: i64,
        limit: usize,
    ) -> Result<Vec<ScheduledWork>, String> {
        self.delegate.claim_due(owner, now_ms, lease_ms, limit)
    }

    fn complete(
        &self,
        schedule_key: &str,
        owner: &str,
        fencing_token: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        self.delegate
            .complete(schedule_key, owner, fencing_token, now_ms)
    }
}

impl VersionedPort for FailingEvidence {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.delegate.descriptors()
    }
}

impl EvidencePort for FailingEvidence {
    fn append(&self, _record: EvidenceRecord) -> Result<(), String> {
        Err("injected raw evidence secret=hidden".to_string())
    }

    fn list(&self, aggregate_id: &str) -> Result<Vec<EvidenceRecord>, String> {
        self.delegate.list(aggregate_id)
    }
}

struct InjectedClock {
    now_ms: i64,
    error: bool,
}

impl VersionedPort for InjectedClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![PortDescriptor::new(PortKind::Clock, "test.injected-clock")
            .for_profiles(&["local"])
            .with_capabilities(&["clock.utc_ms"])]
    }
}

impl ClockPort for InjectedClock {
    fn now_ms(&self) -> i64 {
        self.now_ms
    }

    fn trusted_now_ms(&self) -> Result<i64, String> {
        if self.error {
            Err("injected clock credential=hidden".to_string())
        } else {
            Ok(self.now_ms)
        }
    }
}

struct UnavailableAuthority;

impl FinanceAuthorityPort for UnavailableAuthority {
    fn prepare_command(
        &self,
        _storage: &dyn StoragePort,
        _envelope: &tradeassembly_runtime::control_plane::ControlPlaneCommandEnvelope,
    ) -> Result<(), String> {
        Err("warden_transport_timeout bearer=hidden".to_string())
    }

    fn complete_command(
        &self,
        _storage: &dyn StoragePort,
        _envelope: &tradeassembly_runtime::control_plane::ControlPlaneCommandEnvelope,
        _status: u16,
    ) -> Result<(), String> {
        Ok(())
    }

    fn audit(&self, _storage: &dyn StoragePort) -> Value {
        json!({})
    }
}

struct Fixture {
    _directory: TempDir,
    db: String,
    config_id: String,
    runtime: tradeassembly_runtime::ports::ServiceRuntime,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let directory = tempfile::tempdir().expect("temp directory");
        let db = directory
            .path()
            .join(format!("{name}.db"))
            .to_string_lossy()
            .to_string();
        let service = TradeAssemblyService::test_local(&db);
        let saved = service.handle_http(
            "POST",
            "/product/strategy-execution-configs/save",
            json!({
                "strategyId": "strat_local_btc_demo",
                "mode": "paper",
                "providerRef": "sim",
                "idempotencyKey": format!("{name}-config"),
            }),
        );
        assert_eq!(saved.status, 200, "{:#}", saved.body);
        let config_id = saved.body["body"]["configId"]
            .as_str()
            .expect("config id")
            .to_string();
        let runtime = (*service.runtime()).clone();
        Self {
            _directory: directory,
            db,
            config_id,
            runtime,
        }
    }

    fn request(
        &self,
        service: &TradeAssemblyService,
        key: &str,
    ) -> tradeassembly_runtime::service::ServiceResponse {
        let body = self.request_body(key);
        service.handle_http(
            "POST",
            "/product/strategy-execution-activations/activate",
            body,
        )
    }

    fn request_body(&self, key: &str) -> Value {
        json!({
            "activationId": format!("activation_{key}"),
            "configId": self.config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": key,
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        })
    }

    fn recovered_service(&self) -> TradeAssemblyService {
        TradeAssemblyService::from_runtime(self.db.clone(), self.runtime.clone())
    }
}

fn assert_no_activation_state(service: &TradeAssemblyService) {
    for namespace in [
        "execution_activations",
        "execution_runs",
        "execution_events",
        "execution_checkpoints",
        "execution_watches",
        "execution_reconciliation",
        "execution_controls",
        "execution_health",
    ] {
        assert!(
            service
                .runtime()
                .storage
                .list_json(namespace)
                .expect("list activation namespace")
                .is_empty(),
            "{namespace} must remain empty"
        );
    }
}

fn assert_redacted_failure(response: &tradeassembly_runtime::service::ServiceResponse, code: &str) {
    assert_eq!(response.status, 503, "{:#}", response.body);
    assert_eq!(response.body["error"]["code"], code);
    let encoded = response.body.to_string();
    for forbidden in ["/tmp/", "secret", "token", "credential", "raw evidence"] {
        assert!(
            !encoded.to_ascii_lowercase().contains(forbidden),
            "failure leaked {forbidden}: {encoded}"
        );
    }
}

fn assert_execution_config_required(response: &tradeassembly_runtime::service::ServiceResponse) {
    assert_eq!(response.status, 400, "{:#}", response.body);
    assert_eq!(
        response.body["error"]["details"]["error"]["code"],
        "execution_config_required"
    );
}

fn config_and_revision_counts(service: &TradeAssemblyService) -> (usize, usize) {
    (
        service
            .runtime()
            .storage
            .list_json("execution_configs")
            .expect("execution configs")
            .len(),
        service
            .runtime()
            .storage
            .list_json("capability_graph_revisions")
            .expect("capability revisions")
            .len(),
    )
}

fn inline_request(key: &str) -> Value {
    json!({
        "strategyId": "strat_local_btc_demo",
        "providerRef": "sim",
        "mode": "paper",
        "schedulerIntervalSeconds": 17,
        "idempotencyKey": key,
        "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
    })
}

#[test]
fn activation_storage_batch_failure_is_atomic_redacted_and_retryable() {
    let fixture = Fixture::new("activation-storage-failure");
    let mut failing_runtime = fixture.runtime.clone();
    let schedule_calls = Arc::new(AtomicUsize::new(0));
    failing_runtime.scheduler = Arc::new(ObservedScheduler {
        delegate: Arc::clone(&fixture.runtime.scheduler),
        schedules: Arc::clone(&schedule_calls),
    });
    failing_runtime.storage = Arc::new(FailingBatchStorage {
        delegate: Arc::clone(&fixture.runtime.storage),
    });
    let failing = TradeAssemblyService::from_runtime(fixture.db.clone(), failing_runtime);

    let failed = fixture.request(&failing, "activation-storage-retry");
    assert_redacted_failure(&failed, "activation_storage_unavailable");
    assert_no_activation_state(&failing);
    assert!(failing
        .runtime()
        .scheduler
        .claim_key(
            "execution:activation_activation-storage-retry:cycle:1",
            "failure-check",
            i64::MAX / 2,
            1_000,
        )
        .expect("failed activation schedule lookup")
        .is_none());
    assert_eq!(schedule_calls.load(Ordering::SeqCst), 0);

    let before = config_and_revision_counts(&failing);
    let inline_failed = failing.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        inline_request("activation-storage-inline-retry"),
    );
    assert_execution_config_required(&inline_failed);
    assert_eq!(config_and_revision_counts(&failing), before);
    assert_eq!(schedule_calls.load(Ordering::SeqCst), 0);

    let recovered = fixture.recovered_service();
    let accepted = fixture.request(&recovered, "activation-storage-retry");
    assert_eq!(accepted.status, 200, "{:#}", accepted.body);
    let duplicate = fixture.request(&recovered, "activation-storage-retry");
    assert_eq!(duplicate.status, 200, "{:#}", duplicate.body);
    assert_eq!(
        recovered
            .runtime()
            .storage
            .list_json("execution_runs")
            .expect("execution runs")
            .len(),
        1
    );
}

#[test]
fn activation_evidence_failure_precedes_all_domain_mutation_and_recovers() {
    let fixture = Fixture::new("activation-evidence-failure");
    let mut failing_runtime = fixture.runtime.clone();
    failing_runtime.evidence = Arc::new(FailingEvidence {
        delegate: Arc::clone(&fixture.runtime.evidence),
    });
    let failing = TradeAssemblyService::from_runtime(fixture.db.clone(), failing_runtime);

    let failed = fixture.request(&failing, "activation-evidence-retry");
    assert_redacted_failure(&failed, "activation_evidence_unavailable");
    assert_no_activation_state(&failing);
    let before = config_and_revision_counts(&failing);
    let inline_failed = failing.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        inline_request("activation-evidence-inline-retry"),
    );
    assert_execution_config_required(&inline_failed);
    assert_eq!(config_and_revision_counts(&failing), before);

    let recovered = fixture.recovered_service();
    let accepted = fixture.request(&recovered, "activation-evidence-retry");
    assert_eq!(accepted.status, 200, "{:#}", accepted.body);
    assert_eq!(
        recovered
            .runtime()
            .evidence
            .list(
                accepted.body["body"]["activationId"]
                    .as_str()
                    .expect("activation id")
            )
            .expect("activation evidence")
            .len(),
        1
    );
}

#[test]
fn unavailable_and_regressing_trusted_clocks_fail_before_mutation() {
    let fixture = Fixture::new("activation-clock-failure");
    let mut unavailable_runtime = fixture.runtime.clone();
    unavailable_runtime.clock = Arc::new(InjectedClock {
        now_ms: 0,
        error: true,
    });
    let unavailable = TradeAssemblyService::from_runtime(fixture.db.clone(), unavailable_runtime);
    let failed = fixture.request(&unavailable, "activation-clock-unavailable");
    assert_redacted_failure(&failed, "trusted_clock_unavailable");
    assert_no_activation_state(&unavailable);
    let configs_before = unavailable
        .runtime()
        .storage
        .list_json("execution_configs")
        .expect("execution configs")
        .len();
    let revisions_before = unavailable
        .runtime()
        .storage
        .list_json("capability_graph_revisions")
        .expect("capability revisions")
        .len();
    let inline_failed = unavailable.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "strategyId": "strat_local_btc_demo",
            "providerRef": "sim",
            "mode": "paper",
            "idempotencyKey": "activation-clock-inline-unavailable",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        }),
    );
    assert_execution_config_required(&inline_failed);
    assert_eq!(
        unavailable
            .runtime()
            .storage
            .list_json("execution_configs")
            .expect("execution configs")
            .len(),
        configs_before
    );
    assert_eq!(
        unavailable
            .runtime()
            .storage
            .list_json("capability_graph_revisions")
            .expect("capability revisions")
            .len(),
        revisions_before
    );

    let context = SideEffectContext::new(
        tradeassembly_runtime::ports::AuthorityContext::local_cli(),
        tradeassembly_runtime::ports::IdempotencyKey::new("clock-watermark")
            .expect("idempotency key"),
    );
    fixture
        .runtime
        .storage
        .put_json(
            "trusted_clock_watermarks",
            "execution.activation",
            json!({"lastTrustedAtMs": 200}),
            &context,
        )
        .expect("clock watermark");
    let mut regressing_runtime = fixture.runtime.clone();
    regressing_runtime.clock = Arc::new(InjectedClock {
        now_ms: 199,
        error: false,
    });
    let regressing = TradeAssemblyService::from_runtime(fixture.db.clone(), regressing_runtime);
    let regressed = fixture.request(&regressing, "activation-clock-regressed");
    assert_redacted_failure(&regressed, "trusted_clock_regressed");
    assert_no_activation_state(&regressing);

    fixture
        .runtime
        .storage
        .put_json(
            "trusted_clock_watermarks",
            "execution.activation",
            json!({"lastTrustedAtMs": "invalid"}),
            &context,
        )
        .expect("malformed clock watermark");
    let mut malformed_runtime = fixture.runtime.clone();
    malformed_runtime.clock = Arc::new(InjectedClock {
        now_ms: 201,
        error: false,
    });
    let malformed = TradeAssemblyService::from_runtime(fixture.db.clone(), malformed_runtime);
    let invalid = fixture.request(&malformed, "activation-clock-invalid");
    assert_redacted_failure(&invalid, "trusted_clock_state_invalid");
    assert_no_activation_state(&malformed);
}

#[test]
fn unavailable_warden_fails_closed_without_leaking_transport_detail() {
    let fixture = Fixture::new("activation-warden-failure");
    let mut runtime = fixture.runtime.clone();
    runtime.finance_authority = Arc::new(UnavailableAuthority);
    let service = TradeAssemblyService::from_runtime(fixture.db.clone(), runtime);

    let failed = fixture.request(&service, "activation-warden-unavailable");
    assert_eq!(failed.status, 403, "{:#}", failed.body);
    assert_eq!(failed.body["error"]["code"], "finance_authority_denied");
    assert_no_activation_state(&service);
    let encoded = failed.body.to_string().to_ascii_lowercase();
    assert!(!encoded.contains("bearer"));
    assert!(!encoded.contains("timeout"));
    let before = config_and_revision_counts(&service);
    let inline_failed = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        inline_request("activation-warden-inline-unavailable"),
    );
    assert_eq!(inline_failed.status, 403, "{:#}", inline_failed.body);
    assert_eq!(config_and_revision_counts(&service), before);

    let recovered = fixture.recovered_service();
    let accepted = fixture.request(&recovered, "activation-warden-unavailable");
    assert_eq!(accepted.status, 200, "{:#}", accepted.body);
}

#[test]
fn graphql_and_mcp_dependency_failures_remain_retryable() {
    let graphql_fixture = Fixture::new("activation-graphql-evidence-failure");
    let mut graphql_runtime = graphql_fixture.runtime.clone();
    graphql_runtime.evidence = Arc::new(FailingEvidence {
        delegate: Arc::clone(&graphql_fixture.runtime.evidence),
    });
    let graphql_failing =
        TradeAssemblyService::from_runtime(graphql_fixture.db.clone(), graphql_runtime);
    let graphql_request = json!({
        "operationName": "ActivateExecution",
        "query": "mutation ActivateExecution { activateExecution }",
        "variables": {
            "configId": graphql_fixture.config_id,
            "strategyId": "strat_local_btc_demo",
            "idempotencyKey": "activation-graphql-evidence-retry",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
        },
    });
    let graphql_failed = graphql_failing.execute_graphql(graphql_request.clone());
    assert_eq!(
        graphql_failed["errors"][0]["message"],
        "activation_evidence_unavailable"
    );
    assert_no_activation_state(&graphql_failing);
    let graphql_recovered = graphql_fixture.recovered_service();
    let graphql_accepted = graphql_recovered.execute_graphql(graphql_request);
    assert!(
        graphql_accepted.get("errors").is_none(),
        "{graphql_accepted:#}"
    );

    let mcp_fixture = Fixture::new("activation-mcp-evidence-failure");
    let mut mcp_runtime = mcp_fixture.runtime.clone();
    mcp_runtime.evidence = Arc::new(FailingEvidence {
        delegate: Arc::clone(&mcp_fixture.runtime.evidence),
    });
    let mcp_failing = TradeAssemblyService::from_runtime(mcp_fixture.db.clone(), mcp_runtime);
    let mcp_request = json!({
        "configId": mcp_fixture.config_id,
        "strategyId": "strat_local_btc_demo",
        "idempotencyKey": "activation-mcp-evidence-retry",
        "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
    });
    let mcp_failed =
        mcp_failing.call_mcp_tool("tradeassembly.execution.activate", mcp_request.clone());
    assert_eq!(mcp_failed["isError"], true);
    assert_eq!(
        mcp_failed["structuredContent"]["error"]["code"],
        "activation_evidence_unavailable"
    );
    assert_no_activation_state(&mcp_failing);
    let mcp_recovered = mcp_fixture.recovered_service();
    let mcp_accepted = mcp_recovered.call_mcp_tool("tradeassembly.execution.activate", mcp_request);
    assert_eq!(mcp_accepted["isError"], false, "{mcp_accepted:#}");
}
