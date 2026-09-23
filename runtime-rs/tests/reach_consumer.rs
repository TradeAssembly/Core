// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use std::sync::{
    atomic::{AtomicI64, AtomicU64, Ordering},
    Arc, Mutex,
};
use tempfile::NamedTempFile;
use tradeassembly_runtime::adapters::connected_runner::{
    Ed25519InstallationSigner, HttpReachTransport,
};
use tradeassembly_runtime::agent_runner::{self, AgentDeployment};
use tradeassembly_runtime::finance_authority::TestFinanceAuthority;
use tradeassembly_runtime::ports::{
    AuthorityContext, ClockPort, ConnectedRunnerError, ConnectedRunnerErrorCode, IdempotencyKey,
    PortDescriptor, ReachCommand, ReachFinishRequest, ReachNodeRequest, ReachNodeScope,
    ReachOperation, ReachPoll, ReachReceipt, ReachRecord, ReachState, ReachTransitionRequest,
    ReachTransportPort, RunnerEntropyPort, SideEffectContext, VersionedPort, REACH_COMMAND_SCHEMA,
    REACH_RECEIPT_SCHEMA,
};
use tradeassembly_runtime::runtime_config::{RuntimeBuilder, RuntimeConfig};
use tradeassembly_runtime::service::reach::{local_deployment_version, ConnectedReachConsumer};
use tradeassembly_runtime::service::TradeAssemblyService;

const NODE: &str = "reach-node-1";

#[test]
fn reach_transport_is_outbound_https_or_loopback_only() {
    assert!(HttpReachTransport::new("https://relay.tradeassembly.ai").is_ok());
    assert!(HttpReachTransport::new("http://127.0.0.1:3000").is_ok());
    assert!(HttpReachTransport::new("http://relay.tradeassembly.ai").is_err());
    assert!(HttpReachTransport::new("https://user:secret@relay.tradeassembly.ai").is_err());
    assert!(HttpReachTransport::new("https://relay.tradeassembly.ai/reach").is_err());
}

struct Clock(AtomicI64);
impl VersionedPort for Clock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}
impl ClockPort for Clock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Entropy(AtomicU64);
impl VersionedPort for Entropy {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}
impl RunnerEntropyPort for Entropy {
    fn opaque_id(&self, prefix: &str) -> Result<String, ConnectedRunnerError> {
        Ok(format!(
            "{prefix}-{}",
            self.0.fetch_add(1, Ordering::SeqCst)
        ))
    }
}

struct Transport {
    record: Mutex<Option<ReachRecord>>,
    fail_finish: AtomicU64,
    accepted: AtomicU64,
    finished: AtomicU64,
}
impl Transport {
    fn new(record: ReachRecord) -> Self {
        Self {
            record: Mutex::new(Some(record)),
            fail_finish: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
            finished: AtomicU64::new(0),
        }
    }
}
impl VersionedPort for Transport {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        Vec::new()
    }
}
impl ReachTransportPort for Transport {
    fn claim(
        &self,
        _request: &ReachNodeRequest,
    ) -> Result<Option<ReachRecord>, ConnectedRunnerError> {
        Ok(self.record.lock().expect("record").clone())
    }
    fn accept(
        &self,
        request: &ReachTransitionRequest,
    ) -> Result<ReachRecord, ConnectedRunnerError> {
        self.accepted.fetch_add(1, Ordering::SeqCst);
        let mut lock = self.record.lock().expect("record");
        let record = lock.as_mut().expect("record");
        assert_eq!(request.command_digest, record.command_digest);
        record.state = ReachState::Accepted;
        record.accepted_at_ms = Some(request.request.issued_at_ms);
        record.revision += 1;
        Ok(record.clone())
    }
    fn finish(&self, request: &ReachFinishRequest) -> Result<ReachRecord, ConnectedRunnerError> {
        self.finished.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_finish
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            })
            .is_ok()
        {
            return Err(ConnectedRunnerError::transport_unavailable());
        }
        let mut lock = self.record.lock().expect("record");
        let record = lock.as_mut().expect("record");
        record.state = request.state;
        record.revision += 1;
        record.receipt = Some(ReachReceipt {
            schema: REACH_RECEIPT_SCHEMA.into(),
            command_digest: record.command_digest.clone(),
            state: request.state,
            evidence_ref: request.evidence_ref.clone(),
            recorded_at_ms: request.request.issued_at_ms,
            liquidation_performed: false,
        });
        let completed = record.clone();
        *lock = None;
        Ok(completed)
    }
}

fn node() -> ReachNodeScope {
    ReachNodeScope {
        tenant_id: "tenant-1".into(),
        workspace_id: "workspace-1".into(),
        node_id: NODE.into(),
        actor_ref: "actor-1".into(),
    }
}

fn service() -> (NamedTempFile, TradeAssemblyService) {
    let database = NamedTempFile::new().expect("database");
    let db = database.path().to_string_lossy().to_string();
    let (runtime, _) = RuntimeBuilder::new(RuntimeConfig::local(db.clone()))
        .with_finance_authority(Arc::new(TestFinanceAuthority))
        .build()
        .expect("runtime");
    (database, TradeAssemblyService::from_runtime(db, runtime))
}

fn install_deployment(service: &TradeAssemblyService) -> String {
    let deployment = AgentDeployment {
        executor: Default::default(),
        deployment_id: "deployment-1".into(),
        system_project_id: "system-1".into(),
        agent_definition_version_id: "agent-v1".into(),
        execution_config_version_id: "config-v1".into(),
        studio_tool_allowlist: vec!["studio.deployment.inspect".into()],
        desired_state: "stopped".into(),
        interval_seconds: 60,
        cron_utc: None,
        mode: "paper".into(),
        prompt: "use the user supplied strategy".into(),
        workspace: std::env::temp_dir().display().to_string(),
        runtime_profile: "local-read-only".into(),
    };
    agent_runner::put_deployment_with_context(
        &service.runtime(),
        &deployment,
        &SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("create-reach-deployment").unwrap(),
        ),
    )
    .expect("deployment");
    local_deployment_version(service, &deployment.deployment_id).expect("version")
}

fn record(version: &str, operation: ReachOperation) -> ReachRecord {
    record_with_id(version, operation, "request-1")
}

fn record_with_id(version: &str, operation: ReachOperation, request_id: &str) -> ReachRecord {
    let command = ReachCommand {
        schema: REACH_COMMAND_SCHEMA.into(),
        tenant_id: "tenant-1".into(),
        workspace_id: "workspace-1".into(),
        node_id: NODE.into(),
        actor_ref: "actor-1".into(),
        request_id: request_id.into(),
        idempotency_key: format!("reach-{request_id}"),
        activation_id: "activation-1".into(),
        deployment_id: "deployment-1".into(),
        deployment_version: version.into(),
        authority_ref: "authority-1".into(),
        issued_at_ms: 1_000,
        expires_at_ms: 20_000,
        operation,
    };
    ReachRecord {
        command_digest: command.digest().expect("digest"),
        command,
        state: ReachState::Delivered,
        created_at_ms: 1_000,
        delivered_at_ms: Some(2_000),
        accepted_at_ms: None,
        receipt: None,
        revision: 2,
    }
}

#[test]
fn inspect_and_stop_use_the_same_bound_local_lifecycle() {
    let (_database, service) = service();
    let version = install_deployment(&service);
    let inspect = Arc::new(Transport::new(record_with_id(
        &version,
        ReachOperation::Inspect,
        "inspect-1",
    )));
    let inspected = consumer(
        service.clone(),
        inspect,
        Arc::new(Clock(AtomicI64::new(3_000))),
    )
    .poll_once()
    .expect("inspect");
    assert!(matches!(
        inspected,
        ReachPoll::Completed {
            state: ReachState::Completed,
            ..
        }
    ));

    agent_runner::set_desired_state_with_context(
        &service.runtime(),
        "deployment-1",
        "active",
        &SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("activate-before-reach-stop").unwrap(),
        ),
    )
    .expect("active");
    let version = local_deployment_version(&service, "deployment-1").expect("version");
    let stop = Arc::new(Transport::new(record_with_id(
        &version,
        ReachOperation::Stop,
        "stop-1",
    )));
    let stopped = consumer(
        service.clone(),
        stop,
        Arc::new(Clock(AtomicI64::new(3_000))),
    )
    .poll_once()
    .expect("stop");
    assert!(matches!(
        stopped,
        ReachPoll::Completed {
            state: ReachState::Completed,
            ..
        }
    ));
    assert_eq!(
        agent_runner::deployments(&service.runtime())
            .unwrap()
            .pop()
            .unwrap()
            .desired_state,
        "stopped"
    );
}

fn consumer(
    service: TradeAssemblyService,
    transport: Arc<Transport>,
    clock: Arc<Clock>,
) -> ConnectedReachConsumer {
    ConnectedReachConsumer::new(
        transport,
        Arc::new(Ed25519InstallationSigner::from_seed(NODE, [7; 32]).unwrap()),
        service,
        node(),
        clock,
        Arc::new(Entropy::default()),
    )
    .expect("consumer")
}

#[test]
fn activate_uses_local_policy_and_exact_deployment_binding_without_scheduler_tick() {
    let (_database, service) = service();
    let version = install_deployment(&service);
    let transport = Arc::new(Transport::new(record(&version, ReachOperation::Activate)));
    let result = consumer(
        service.clone(),
        transport,
        Arc::new(Clock(AtomicI64::new(3_000))),
    )
    .poll_once()
    .expect("poll");
    assert!(
        matches!(
            result,
            ReachPoll::Completed {
                state: ReachState::Completed,
                ..
            }
        ),
        "{result:?}"
    );
    let deployment = agent_runner::deployments(&service.runtime())
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(deployment.desired_state, "active");
    assert!(service
        .runtime()
        .storage
        .list_json(agent_runner::RUNS_NS)
        .unwrap()
        .is_empty());
}

#[test]
fn lost_terminal_response_redelivers_and_replays_local_idempotency() {
    let (_database, service) = service();
    let version = install_deployment(&service);
    let transport = Arc::new(Transport::new(record(&version, ReachOperation::Activate)));
    transport.fail_finish.store(1, Ordering::SeqCst);
    let clock = Arc::new(Clock(AtomicI64::new(3_000)));
    assert_eq!(
        consumer(service.clone(), transport.clone(), clock.clone())
            .poll_once()
            .expect_err("lost finish")
            .code,
        ConnectedRunnerErrorCode::TransportUnavailable
    );
    let result = consumer(service.clone(), transport.clone(), clock)
        .poll_once()
        .expect("recovered poll");
    assert!(
        matches!(
            result,
            ReachPoll::Completed {
                state: ReachState::Completed,
                ..
            }
        ),
        "{result:?}"
    );
    assert_eq!(transport.accepted.load(Ordering::SeqCst), 1);
    assert_eq!(transport.finished.load(Ordering::SeqCst), 2);
    assert_eq!(
        agent_runner::deployments(&service.runtime())
            .unwrap()
            .pop()
            .unwrap()
            .desired_state,
        "active"
    );
}

#[test]
fn wrong_deployment_version_is_durably_rejected() {
    let (_database, service) = service();
    install_deployment(&service);
    let transport = Arc::new(Transport::new(record(
        "wrong-version",
        ReachOperation::Stop,
    )));
    let result = consumer(
        service.clone(),
        transport,
        Arc::new(Clock(AtomicI64::new(3_000))),
    )
    .poll_once()
    .expect("poll");
    assert!(matches!(
        result,
        ReachPoll::Completed {
            state: ReachState::Rejected,
            ..
        }
    ));
    assert_eq!(
        agent_runner::deployments(&service.runtime())
            .unwrap()
            .pop()
            .unwrap()
            .desired_state,
        "stopped"
    );
}
