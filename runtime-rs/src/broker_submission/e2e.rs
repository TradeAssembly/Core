//! Opt-in, real-process proof for the named-agent broker boundary.
//!
//! This deliberately lives behind an ignored test: it needs an explicitly
//! built Warden, controlled broker, Node, and SRT executable.

#[path = "../../tests/common/controlled_broker_package.rs"]
mod controlled_broker_package;
#[path = "../../tests/common/controlled_warden.rs"]
mod controlled_warden;

use crate as runtime_crate;
use crate::agent_runner::{
    self, AgentDeployment, AgentMcpExecutionCapability, AgentRun, AgentRuntimePort,
};
use crate::local_live_authorization::deployment_actor;
use crate::local_owner_identity::LocalOwnerIdentity;
use crate::ports::{
    AuthorityContext, IdempotencyKey, PluginOperationPort, PluginOperationRequest,
    PluginOperationResponse, PluginProcessSandboxPort, PluginSandboxRequest,
    SandboxedPluginProcess, SideEffectContext,
};
use crate::service::TradeAssemblyService;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static SANDBOX_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct ControlledSandbox {
    srt: PathBuf,
    node: PathBuf,
    settings_root: PathBuf,
    launches: AtomicU64,
}

impl ControlledSandbox {
    fn new(srt: PathBuf, node: PathBuf, settings_root: &Path) -> Self {
        std::fs::create_dir_all(settings_root).unwrap();
        Self {
            srt,
            node,
            settings_root: settings_root.to_owned(),
            launches: AtomicU64::new(0),
        }
    }
}

impl crate::ports::VersionedPort for ControlledSandbox {
    fn descriptors(&self) -> Vec<crate::ports::PortDescriptor> {
        vec![]
    }
}

impl PluginProcessSandboxPort for ControlledSandbox {
    fn spawn(&self, request: &PluginSandboxRequest) -> Result<SandboxedPluginProcess, String> {
        assert!(
            request.allowed_domains.is_empty(),
            "controlled fixture must not receive network access"
        );
        let root = request
            .install_root
            .canonicalize()
            .map_err(|_| "fixture root invalid")?;
        let executable = request
            .executable
            .canonicalize()
            .map_err(|_| "fixture executable invalid")?;
        assert!(executable.starts_with(&root));
        let settings_path = self.settings_root.join(format!(
            "controlled-sandbox-{}-{}.json",
            std::process::id(),
            SANDBOX_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let denied_reads: Vec<String> = std::env::var("HOME").into_iter().collect();
        let settings = json!({
            "network": {"allowedDomains": [], "deniedDomains": [], "strictAllowlist": true, "allowUnixSockets": [], "allowLocalBinding": false},
            "filesystem": {"denyRead": denied_reads, "allowRead": [root], "allowWrite": [root], "denyWrite": []},
            "enableWeakerNestedSandbox": false, "enableWeakerNetworkIsolation": false, "allowAppleEvents": false
        });
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&settings_path)
            .map_err(|_| "sandbox settings unavailable")?;
        file.write_all(serde_json::to_string(&settings).unwrap().as_bytes())
            .map_err(|_| "sandbox settings unavailable")?;
        file.flush().map_err(|_| "sandbox settings unavailable")?;
        let mut command = Command::new(&self.node);
        command
            .arg(&self.srt)
            .args(["--settings", settings_path.to_str().unwrap()])
            .arg(&executable)
            .current_dir(root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        self.launches.fetch_add(1, Ordering::SeqCst);
        match command.spawn() {
            Ok(child) => Ok(SandboxedPluginProcess::new(child, Some(settings_path))),
            Err(_) => {
                let _ = std::fs::remove_file(settings_path);
                Err("sandbox process unavailable".into())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Accepted,
    LostResponse,
    RestartRecovery,
    AgentRecovery,
    GenericAccepted,
    GenericMissingActor,
    MissingActor,
    Paused,
    StoppedAgent,
    WrongAccount,
    ExcessQuantity,
    MissingPrice,
    WardenDenied,
    WardenOutage,
    PreEffectStorageFailure,
    ReceiptStorageFailure,
    RevokedMandate,
    ExpiredMandate,
    InvalidQuantity,
    ExcessNotional,
    StalePrice,
    ChangedConfig,
    ChangedPackage,
    ChangedCredential,
    ReplacedLease,
}

struct NamedAgent {
    scenario: Scenario,
    runtime: TradeAssemblyService,
    operations: Arc<dyn PluginOperationPort>,
    quote_request: PluginOperationRequest,
    order_request: PluginOperationRequest,
    quote: Mutex<Option<PluginOperationResponse>>,
    order: Mutex<Option<PluginOperationResponse>>,
    captured_agent: Mutex<Option<agent_runner::VerifiedAgentMcpExecutionContext>>,
    deps: super::BrokerSubmissionDependencies,
}

impl AgentRuntimePort for NamedAgent {
    fn start_or_resume(&self, run: &AgentRun, _: &Value) -> Result<Option<String>, String> {
        Ok(Some(format!("controlled-session:{}", run.run_id)))
    }

    fn start_or_resume_with_mcp_execution_context(
        &self,
        run: &AgentRun,
        _: &Value,
        capability: &AgentMcpExecutionCapability,
    ) -> Result<Option<String>, String> {
        let verified = capability.verified_context(&self.runtime.runtime())?;
        *self.captured_agent.lock().unwrap() = Some(verified.clone());
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("fixture-quote").unwrap(),
        )
        .with_agent_execution(Some(&verified));
        let quote_response = self.operations.invoke(&self.quote_request, &context)?;
        *self.quote.lock().unwrap() = Some(quote_response.clone());
        let mut order = self.order_request.clone();
        order.evidence_refs = quote_response.evidence_refs.clone();
        let mut order_context = SideEffectContext::new(
            context.authority.clone(),
            IdempotencyKey::new("fixture-order").unwrap(),
        )
        .with_agent_execution(Some(&verified));
        match self.scenario {
            Scenario::MissingActor => order_context = order_context.with_agent_execution(None),
            Scenario::StoppedAgent => {
                agent_runner::set_desired_state(
                    &self.runtime.runtime(),
                    "fixture-agent",
                    "stopped",
                )?;
            }
            Scenario::Paused => self.runtime.runtime().storage.put_json(
                "execution_controls",
                "fixture-activation",
                json!({"pauseEntries":"paused"}),
                &context,
            )?,
            Scenario::WrongAccount => order.account_ref = Some("account://wrong/fixture".into()),
            Scenario::ExcessQuantity => order.input["quantity"] = json!("3"),
            Scenario::MissingPrice => order.evidence_refs.clear(),
            Scenario::InvalidQuantity => order.input["quantity"] = json!("0"),
            Scenario::ChangedConfig => {
                let run = self
                    .deps
                    .storage
                    .get_json("execution_runs", "run_fixture-activation")?
                    .unwrap();
                let id = run["configId"].as_str().unwrap();
                let mut config = self
                    .deps
                    .storage
                    .get_json("execution_configs", id)?
                    .unwrap();
                config["riskLimits"]["max_notional"] = json!(11);
                self.deps
                    .storage
                    .put_json("execution_configs", id, config, &context)?;
            }
            Scenario::ChangedPackage | Scenario::ChangedCredential => {
                let mut instance = self
                    .deps
                    .storage
                    .get_json("plugin_instances_v2", "mandate-live")?
                    .unwrap();
                if self.scenario == Scenario::ChangedPackage {
                    instance["activePackageSha256"] = json!("unavailable-package");
                } else {
                    instance["credentialRevision"] =
                        json!(instance["credentialRevision"].as_u64().unwrap_or(0) + 1);
                }
                self.deps.storage.put_json(
                    "plugin_instances_v2",
                    "mandate-live",
                    instance,
                    &context,
                )?;
            }
            Scenario::ReplacedLease => {
                let lease = verified.current_lease(
                    self.deps.storage.as_ref(),
                    self.deps.clock.as_ref(),
                    self.deps.leases.as_ref(),
                )?;
                self.deps.leases.release(&lease)?;
                let replacement = self
                    .deps
                    .leases
                    .acquire(
                        &lease.resource,
                        &lease.owner,
                        self.deps.clock.now_ms(),
                        600_000,
                    )?
                    .unwrap();
                assert!(replacement.fencing_token > lease.fencing_token);
            }
            Scenario::StalePrice => {
                let mut receipt = self
                    .runtime
                    .runtime()
                    .storage
                    .get_json("plugin_operation_receipts", "fixture-quote")?
                    .unwrap();
                receipt["payload"]["quote"]["timestamp"] = json!("2000-01-01T00:00:00Z");
                self.runtime.runtime().storage.put_json(
                    "plugin_operation_receipts",
                    "fixture-quote",
                    receipt,
                    &context,
                )?;
            }
            Scenario::RevokedMandate | Scenario::ExpiredMandate => {
                let run = self
                    .runtime
                    .runtime()
                    .storage
                    .get_json("execution_runs", "run_fixture-activation")?
                    .unwrap();
                let id = run["localLiveAuthority"]["mandateId"].as_str().unwrap();
                if self.scenario == Scenario::RevokedMandate {
                    let revoked = self.runtime.handle_http(
                        "POST",
                        "/product/live-mandates/revoke",
                        json!({"mandateId":id,"idempotencyKey":"before-submit-revoke"}),
                    );
                    assert_eq!(revoked.status, 200, "{revoked:#?}");
                } else {
                    let expires = self
                        .runtime
                        .runtime()
                        .storage
                        .get_json("local_live_mandates", id)?
                        .unwrap()["expiresAtMs"]
                        .as_i64()
                        .unwrap();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
                    while self.runtime.runtime().clock.now_ms() <= expires {
                        assert!(std::time::Instant::now() < deadline, "bounded expiry wait");
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
                let error = super::load_current_state(&self.deps, &order, &order_context)
                    .unwrap_err()
                    .to_lowercase();
                assert!(
                    error.contains(if self.scenario == Scenario::RevokedMandate {
                        "revoked"
                    } else {
                        "expired"
                    }),
                    "{error}"
                );
            }
            Scenario::Accepted
            | Scenario::LostResponse
            | Scenario::RestartRecovery
            | Scenario::AgentRecovery
            | Scenario::GenericAccepted
            | Scenario::GenericMissingActor
            | Scenario::WardenOutage
            | Scenario::PreEffectStorageFailure
            | Scenario::ReceiptStorageFailure
            | Scenario::ExcessNotional
            | Scenario::WardenDenied => {}
        }
        if matches!(
            self.scenario,
            Scenario::GenericAccepted | Scenario::GenericMissingActor
        ) {
            let mut runtime = self.runtime.runtime().as_ref().clone();
            runtime.plugin_operations = self.operations.clone();
            let mut service = TradeAssemblyService::from_runtime(self.runtime.db(), runtime);
            if self.scenario == Scenario::GenericAccepted {
                service = service.with_verified_agent_mcp_execution_context(verified);
            }
            let mut body = serde_json::to_value(&order).unwrap();
            body["idempotencyKey"] = json!("fixture-order");
            body["instrumentFamily"] = json!("crypto_spot");
            let response = service.handle_http(
                "POST",
                "/plugins/instances/mandate-live/operations/broker.live_order_submit:invoke",
                body,
            );
            if self.scenario == Scenario::GenericAccepted {
                assert_eq!(response.status, 200, "{response:#?}");
                *self.order.lock().unwrap() =
                    Some(serde_json::from_value(response.body["response"].clone()).unwrap());
            } else {
                assert_eq!(response.status, 400, "{response:#?}");
            }
            return Ok(Some(format!("controlled-session:{}", run.run_id)));
        }
        if matches!(
            self.scenario,
            Scenario::LostResponse
                | Scenario::RestartRecovery
                | Scenario::AgentRecovery
                | Scenario::ReceiptStorageFailure
        ) {
            for _ in 0..2 {
                assert_eq!(
                    self.operations.invoke(&order, &order_context).unwrap_err(),
                    "plugin_order_reconciliation_required"
                );
            }
            if self.scenario == Scenario::AgentRecovery {
                let intent = self
                    .runtime
                    .runtime()
                    .storage
                    .list_json("broker_order_intents")?
                    .pop()
                    .unwrap()
                    .1;
                let revoked = self.runtime.handle_http(
                    "POST",
                    "/product/live-mandates/revoke",
                    json!({"mandateId":intent["mandateId"],"idempotencyKey":"active-agent-revoke"}),
                );
                assert_eq!(revoked.status, 200, "{revoked:#?}");
                let mut runtime = self.runtime.runtime().as_ref().clone();
                runtime.plugin_operations = self.operations.clone();
                let service = TradeAssemblyService::from_runtime(self.runtime.db(), runtime)
                    .with_verified_agent_mcp_execution_context(verified);
                let recovered = service.handle_http("POST","/orders/broker-recovery",json!({"originalIdempotencyKey":"fixture-order","recoveryIdempotencyKey":"active-agent-recovery"}));
                assert_eq!(recovered.status, 200, "{recovered:#?}");
                *self.order.lock().unwrap() =
                    Some(serde_json::from_value(recovered.body["receipt"].clone()).unwrap());
            }
            return Ok(Some(format!("controlled-session:{}", run.run_id)));
        }
        if self.scenario != Scenario::Accepted {
            for _ in 0..2 {
                assert_eq!(
                    self.operations.invoke(&order, &order_context).unwrap_err(),
                    "plugin_order_submission_denied",
                    "{:?}",
                    self.scenario
                );
            }
            return Ok(Some(format!("controlled-session:{}", run.run_id)));
        }
        let (first, concurrent) = std::thread::scope(|scope| {
            let first = scope.spawn(|| self.operations.invoke(&order, &order_context));
            let concurrent = scope.spawn(|| self.operations.invoke(&order, &order_context));
            (first.join().unwrap(), concurrent.join().unwrap())
        });
        assert!(
            first.is_ok() || concurrent.is_ok(),
            "no accepted controlled submission"
        );
        let recovered = self.operations.invoke(&order, &order_context)?;
        for result in [first, concurrent] {
            match result {
                Ok(response) => assert_eq!(response, recovered),
                Err(error) => assert_eq!(error, "plugin_order_reconciliation_required"),
            }
        }
        *self.order.lock().unwrap() = Some(recovered);
        Ok(Some(format!("controlled-session:{}", run.run_id)))
    }
}

#[test]
#[ignore = "requires explicit F2_TEST_WARDEN_BINARY, F2_TEST_CONTROLLED_BROKER_BINARY, F2_TEST_NODE_BINARY, and F2_TEST_SRT_CLI"]
fn named_agent_real_plugin_quote_and_c5_order_are_duplicate_safe() {
    run_scenario(Scenario::Accepted);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn owner_recovers_lost_response_after_stop_and_revocation_without_resubmit() {
    run_scenario(Scenario::LostResponse);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn fresh_runtime_process_recovers_without_warden_or_original_agent_memory() {
    run_scenario(Scenario::RestartRecovery);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn current_agent_observes_after_mandate_revocation_but_expired_callback_cannot() {
    run_scenario(Scenario::AgentRecovery);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn generic_plugin_route_uses_the_same_real_broker_boundary() {
    run_scenario(Scenario::GenericAccepted);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn generic_plugin_route_cannot_use_copied_agent_or_scheduler_fields() {
    run_scenario(Scenario::GenericMissingActor);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn stopped_warden_fails_closed_before_broker_dispatch() {
    run_scenario(Scenario::WardenOutage);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn real_sqlite_intent_write_failure_prevents_dispatch() {
    run_scenario(Scenario::PreEffectStorageFailure);
}

#[test]
#[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
fn real_sqlite_receipt_write_failure_recovers_without_resubmission() {
    run_scenario(Scenario::ReceiptStorageFailure);
}

macro_rules! risk_authority_cases {
    ($($name:ident => $case:ident),* $(,)?) => {$(
        #[test]
        #[ignore = "requires explicit real Warden, controlled broker, Node and SRT"]
        fn $name() { run_scenario(Scenario::$case); }
    )*};
}
risk_authority_cases! {
    revoked_mandate_prevents_real_dispatch => RevokedMandate,
    expired_mandate_prevents_real_dispatch => ExpiredMandate,
    invalid_quantity_prevents_real_dispatch => InvalidQuantity,
    excess_notional_prevents_real_dispatch => ExcessNotional,
    stale_quote_prevents_real_dispatch => StalePrice,
    changed_config_prevents_real_dispatch => ChangedConfig,
    changed_package_prevents_real_dispatch => ChangedPackage,
    changed_credential_generation_prevents_real_dispatch => ChangedCredential,
    replaced_agent_lease_prevents_real_dispatch => ReplacedLease,
    stopped_agent_prevents_real_dispatch => StoppedAgent,
}

#[test]
fn restart_recovery_child() {
    let Some(db) = std::env::var_os("F2_RECOVERY_CHILD_DB") else {
        return;
    };
    let db = PathBuf::from(db);
    let handoff = std::env::var("F2_RECOVERY_CHILD_HANDOFF").expect("owned fixture handoff");
    let base =
        TradeAssemblyService::test_local_with_handoff(db.to_string_lossy(), handoff).unwrap();
    let owner = LocalOwnerIdentity::for_database(&db).unwrap();
    let root = db.parent().unwrap();
    let sandbox = Arc::new(ControlledSandbox::new(
        PathBuf::from(std::env::var_os("F2_TEST_SRT_CLI").unwrap())
            .canonicalize()
            .unwrap(),
        PathBuf::from(std::env::var_os("F2_TEST_NODE_BINARY").unwrap())
            .canonicalize()
            .unwrap(),
        &root.join("child-sandbox"),
    ));
    let mut runtime = base.runtime().as_ref().clone();
    runtime.plugin_operations = Arc::new(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            runtime.storage.clone(),
            runtime.plugins.clone(),
            runtime.credentials.clone(),
            runtime.plugin_packages.clone(),
            runtime.clock.clone(),
            sandbox.clone(),
        ),
    ); // Deliberately no broker admission boundary or running Warden.
    let service = TradeAssemblyService::from_runtime(db.to_string_lossy(), runtime)
        .for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
    assert!(service
        .runtime()
        .storage
        .get_json("plugin_operation_receipts", "fixture-order")
        .unwrap()
        .is_none());
    let result = service.handle_http("POST","/orders/broker-recovery",json!({"originalIdempotencyKey":"fixture-order","recoveryIdempotencyKey":"fixture-recovery"}));
    assert_eq!(result.status, 200, "{result:#?}");
    assert_eq!(result.body["receipt"]["payload"]["submissionCount"], 1);
    assert_eq!(sandbox.launches.load(Ordering::SeqCst), 1);
}

macro_rules! rejected_cases {
    ($($name:ident => $scenario:ident),* $(,)?) => { $(
        #[test]
        #[ignore = "requires real Warden, controlled broker, Node and SRT environment inputs"]
        fn $name() { run_scenario(Scenario::$scenario); }
    )* };
}
rejected_cases! {
    missing_agent_authority_never_reaches_sink => MissingActor,
    paused_activation_never_reaches_sink => Paused,
    wrong_account_never_reaches_sink => WrongAccount,
    excess_quantity_never_reaches_sink => ExcessQuantity,
    missing_price_never_reaches_sink => MissingPrice,
    real_warden_denial_never_reaches_sink => WardenDenied,
}

fn run_scenario(scenario: Scenario) {
    for name in [
        "F2_TEST_WARDEN_BINARY",
        "F2_TEST_CONTROLLED_BROKER_BINARY",
        "F2_TEST_NODE_BINARY",
        "F2_TEST_SRT_CLI",
    ] {
        assert!(std::env::var_os(name).is_some(), "{name} is required");
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let db = root.join("runtime.db");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    let owner = LocalOwnerIdentity::for_database(&db).unwrap();
    let service = base.for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
    let mut warden = controlled_warden::ControlledWarden::start(&root);
    if scenario != Scenario::WardenDenied {
        warden.allow_controlled_submission();
    }
    let package = controlled_broker_package::install_controlled_package(
        &service,
        &root,
        &PathBuf::from(std::env::var_os("F2_TEST_CONTROLLED_BROKER_BINARY").unwrap()),
    );
    if matches!(
        scenario,
        Scenario::LostResponse | Scenario::RestartRecovery | Scenario::AgentRecovery
    ) {
        std::fs::write(
            package
                .install_root
                .join(".f2-controlled-broker-lose-response"),
            b"fixture",
        )
        .unwrap();
    }
    let saved = service.handle_http("POST", "/product/strategy-execution-configs/save", json!({"strategyId":"strat_local_btc_demo","mode":"live","providerRef":"mandate-live","accountRef":"account://mandate-live/controlled","dataProviderRef":"mandate-live","dataAccountRef":"account://mandate-live/controlled","riskLimits":{"max_notional":if scenario==Scenario::ExcessNotional {0.5} else {10.0},"max_order_quantity":2}}));
    assert_eq!(saved.status, 200, "{saved:#?}");
    let config_id = saved.body["body"]["item"]["configId"].as_str().unwrap();
    let deployment = AgentDeployment {
        executor: Default::default(),
        deployment_id: "fixture-agent".into(),
        system_project_id: "system-1".into(),
        agent_definition_version_id: "agent-v1".into(),
        execution_config_version_id: config_id.into(),
        studio_tool_allowlist: vec!["studio.deployment.inspect".into()],
        desired_state: "active".into(),
        interval_seconds: 60,
        cron_utc: None,
        mode: "live".into(),
        prompt: "controlled fixture".into(),
        workspace: root.display().to_string(),
        runtime_profile: "local-read-only".into(),
    };
    agent_runner::put_deployment(&service.runtime(), &deployment).unwrap();
    let issued = service.handle_http("POST", "/product/live-mandates/issue", json!({"configId":config_id,"expiresAtMs":service.runtime().clock.now_ms()+if scenario==Scenario::ExpiredMandate {6000} else {600_000},"delegateDeploymentId":deployment.deployment_id,"idempotencyKey":"fixture-mandate"}));
    assert_eq!(issued.status, 201, "{issued:#?}");
    let mut run = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .unwrap()
        .unwrap();
    run["kind"] = json!("ExecutionRun");
    run["state"] = json!("active");
    run["activationId"] = json!("fixture-activation");
    run["correlationId"] = json!("fixture-run-correlation");
    let mandate = issued.body["mandate"].clone();
    run["localLiveAuthority"] = json!({"schemaVersion":"tradeassembly.local_live_activation_authority.v1","mandateId":mandate["mandateId"],"mandateDigest":mandate["digest"],"actor":deployment_actor(&owner.stable_identity_id, &deployment.deployment_id, &agent_runner::deployment_binding_digest(&deployment))});
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("fixture-state").unwrap(),
    );
    for (namespace, key) in [
        ("execution_runs", "run_fixture-activation"),
        ("execution_activations", "fixture-activation"),
    ] {
        service
            .runtime()
            .storage
            .put_json(namespace, key, run.clone(), &context)
            .unwrap();
    }
    service.runtime().storage.put_json("execution_controls", "fixture-activation", json!({"activationId":"fixture-activation","pauseEntries":"available","stop":"available","emergencyStop":"available","newEntriesPaused":false}), &context).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            "execution_reconciliation",
            "fixture-activation",
            json!({"activationId":"fixture-activation","state":"clear","newEntriesPaused":false}),
            &context,
        )
        .unwrap();
    let revision = service
        .runtime()
        .storage
        .get_json(
            "capability_graph_revisions",
            run["capabilityGraphRevisionId"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
    let nodes = revision["graph"]["nodes"].as_array().unwrap();
    let quote_node = nodes
        .iter()
        .find(|n| n["requirement"]["requirementId"] == "execution.risk.quote")
        .expect("live execution quote requirement");
    let order_node = nodes
        .iter()
        .find(|n| {
            n["selected"].is_object()
                && n["requirement"]["capability"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("broker.order_submit")
        })
        .expect("live order requirement");
    let make_request = |node: &Value,
                        operation: &str,
                        capability: &str,
                        purpose: &str,
                        correlation: &str,
                        input: Value|
     -> PluginOperationRequest {
        let selected = &node["selected"];
        PluginOperationRequest {
            correlation_id: correlation.into(),
            plugin_instance_ref: selected["pluginInstanceRef"].as_str().unwrap().into(),
            plugin_ref: selected["pluginRef"].as_str().unwrap().into(),
            manifest_fingerprint: selected["manifestFingerprint"].as_str().unwrap().into(),
            operation_id: operation.into(),
            capability: capability.into(),
            capability_graph_revision_id: run["capabilityGraphRevisionId"].as_str().unwrap().into(),
            capability_graph_fingerprint: run["capabilityGraphFingerprint"]
                .as_str()
                .unwrap()
                .into(),
            strategy_id: run["strategyId"].as_str().unwrap().into(),
            strategy_version_id: run["strategyVersionId"].as_str().unwrap().into(),
            strategy_spec_hash: run["strategySpecHash"].as_str().unwrap().into(),
            activation_id: "fixture-activation".into(),
            attempt_id: "fixture-agent-attempt".into(),
            evaluation_tick_id: "fixture-agent-tick".into(),
            mode: "live".into(),
            purpose: purpose.into(),
            account_ref: selected["accountRef"].as_str().map(str::to_owned),
            timeout_ms: 10_000,
            fencing_token: None,
            input,
            evidence_refs: vec![],
        }
    };
    let quote_request = make_request(
        quote_node,
        "marketdata.quote.read",
        quote_node["requirement"]["capability"].as_str().unwrap(),
        "market_data_research",
        "fixture-quote-correlation",
        json!({"symbol":"BTC/USD"}),
    );
    let order_request = make_request(
        order_node,
        "broker.live_order_submit",
        order_node["requirement"]["capability"].as_str().unwrap(),
        "live_order_submission",
        "fixture-order-correlation",
        json!({"symbol":"BTC/USD","side":"buy","orderType":"market","timeInForce":"gtc","clientOrderId":"fixture-order","quantity":"1"}),
    );
    let sandbox = Arc::new(ControlledSandbox::new(
        PathBuf::from(std::env::var_os("F2_TEST_SRT_CLI").unwrap())
            .canonicalize()
            .unwrap(),
        PathBuf::from(std::env::var_os("F2_TEST_NODE_BINARY").unwrap())
            .canonicalize()
            .unwrap(),
        &root.join("sandbox"),
    ));
    let deps = super::BrokerSubmissionDependencies {
        storage: service.runtime().storage.clone(),
        plugins: service.runtime().plugins.clone(),
        plugin_packages: service.runtime().plugin_packages.clone(),
        credentials: service.runtime().credentials.clone(),
        capability_resolver: service.runtime().capability_resolver.clone(),
        clock: service.runtime().clock.clone(),
        leases: service.runtime().leases.clone(),
        owner: owner.clone(),
    };
    let boundary =
        super::LocalBrokerSubmissionBoundary::new(deps.clone(), Arc::new(warden.authority.clone()));
    let operations = Arc::new(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            service.runtime().storage.clone(),
            service.runtime().plugins.clone(),
            service.runtime().credentials.clone(),
            service.runtime().plugin_packages.clone(),
            service.runtime().clock.clone(),
            sandbox.clone(),
        )
        .with_broker_boundary(Arc::new(boundary)),
    );
    if scenario == Scenario::WardenOutage {
        warden.stop();
    }
    if matches!(
        scenario,
        Scenario::PreEffectStorageFailure | Scenario::ReceiptStorageFailure
    ) {
        let connection = rusqlite::Connection::open(&db).unwrap();
        let sql = if scenario == Scenario::PreEffectStorageFailure {
            "CREATE TRIGGER f2_storage_fault BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='broker_order_intents' BEGIN SELECT RAISE(ABORT,'controlled storage failure'); END;"
        } else {
            "CREATE TRIGGER f2_storage_fault BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='plugin_operation_receipts' AND NEW.item_key='fixture-order' BEGIN SELECT RAISE(ABORT,'controlled storage failure'); END;"
        };
        connection.execute_batch(sql).unwrap();
    }
    let adapter = NamedAgent {
        scenario,
        runtime: service.clone(),
        operations: operations.clone(),
        quote_request,
        order_request,
        quote: Mutex::new(None),
        order: Mutex::new(None),
        captured_agent: Mutex::new(None),
        deps,
    };
    let result = agent_runner::supervise_once(
        &service.runtime(),
        "fixture-runner",
        service.runtime().clock.now_ms(),
        &adapter,
    )
    .unwrap();
    assert_eq!(
        result[0]["outcome"],
        if scenario == Scenario::ReplacedLease {
            "pending_reconcile"
        } else {
            "completed"
        },
        "{result:#?}"
    );
    if scenario == Scenario::ReplacedLease {
        assert_eq!(result[0]["errorCode"], "agent_run_pending_reconcile");
    }
    if scenario == Scenario::AgentRecovery {
        let launches = sandbox.launches.load(Ordering::SeqCst);
        assert_eq!(launches, 3);
        let mut runtime = service.runtime().as_ref().clone();
        runtime.plugin_operations = operations.clone();
        let expired = TradeAssemblyService::from_runtime(db.to_string_lossy(), runtime)
            .with_verified_agent_mcp_execution_context(
                adapter.captured_agent.lock().unwrap().clone().unwrap(),
            );
        assert_eq!(expired.handle_http("POST","/orders/broker-recovery",json!({"originalIdempotencyKey":"fixture-order","recoveryIdempotencyKey":"expired-agent"})).status,403);
        assert_eq!(sandbox.launches.load(Ordering::SeqCst), launches);
    }
    if scenario == Scenario::ReceiptStorageFailure {
        assert!(service
            .runtime()
            .storage
            .get_json("plugin_operation_receipts", "fixture-order")
            .unwrap()
            .is_none());
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch("DROP TRIGGER f2_storage_fault;")
            .unwrap();
    }
    if matches!(
        scenario,
        Scenario::LostResponse | Scenario::RestartRecovery | Scenario::ReceiptStorageFailure
    ) {
        agent_runner::set_desired_state(&service.runtime(), "fixture-agent", "stopped").unwrap();
        let revoked = service.handle_http(
            "POST",
            "/product/live-mandates/revoke",
            json!({"mandateId":mandate["mandateId"],"idempotencyKey":"fixture-revoke"}),
        );
        assert_eq!(revoked.status, 200, "{revoked:#?}");
        service
            .runtime()
            .storage
            .put_json(
                "execution_controls",
                "fixture-activation",
                json!({"stopRequested":true}),
                &context,
            )
            .unwrap();
        let mut recovery_runtime = service.runtime().as_ref().clone();
        recovery_runtime.plugin_operations = operations.clone();
        let recovery_service =
            TradeAssemblyService::from_runtime(db.to_string_lossy(), recovery_runtime)
                .for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
        let recovery_body = json!({"originalIdempotencyKey":"fixture-order","recoveryIdempotencyKey":"fixture-recovery"});
        let launches_before = sandbox.launches.load(Ordering::SeqCst);
        let wrong_owner =
            recovery_service.for_authenticated_invocation(&owner.issuer, "wrong-owner", None, None);
        assert_eq!(
            wrong_owner
                .handle_http("POST", "/orders/broker-recovery", recovery_body.clone())
                .status,
            403
        );
        let mut injected = recovery_body.clone();
        injected["accountRef"] = json!("account://wrong");
        assert_eq!(
            recovery_service
                .handle_http("POST", "/orders/broker-recovery", injected)
                .status,
            400
        );
        // Deliberately corrupt only disposable fixture storage, then restore it.
        // Every denied binding must fail before any actual sandbox subprocess.
        for (namespace, key, field, replacement) in [
            ("execution_configs", config_id, "strategyId", json!("wrong")),
            (
                "plugin_instances_v2",
                "mandate-live",
                "accountRef",
                json!("account://wrong"),
            ),
            (
                "plugin_instances_v2",
                "mandate-live",
                "credentialRef",
                json!("credential://wrong"),
            ),
            (
                "plugin_instances_v2",
                "mandate-live",
                "activePackageSha256",
                json!("wrong"),
            ),
            (
                "plugin_operation_prepared_bindings",
                "fixture-order",
                "credentialGeneration",
                json!(999),
            ),
            (
                "plugin_broker_dispatch_claims",
                "fixture-order",
                "requestHash",
                json!("wrong"),
            ),
        ] {
            let original = service
                .runtime()
                .storage
                .get_json(namespace, key)
                .unwrap()
                .unwrap();
            let mut changed = original.clone();
            changed[field] = replacement;
            service
                .runtime()
                .storage
                .put_json(namespace, key, changed, &context)
                .unwrap();
            assert_eq!(
                recovery_service
                    .handle_http("POST", "/orders/broker-recovery", recovery_body.clone())
                    .status,
                403,
                "{namespace}/{field}"
            );
            service
                .runtime()
                .storage
                .put_json(namespace, key, original, &context)
                .unwrap();
        }
        assert_eq!(sandbox.launches.load(Ordering::SeqCst), launches_before);
        // The controlled provider temporarily cannot find the original order.
        // Lookup may retry with a NEW recovery key, never by releasing submit.
        let sink_path = package.install_root.join(".f2-controlled-broker.sqlite");
        let held_path = package
            .install_root
            .join(".f2-controlled-broker-held.sqlite");
        std::fs::rename(&sink_path, &held_path).unwrap();
        let not_found_body = json!({"originalIdempotencyKey":"fixture-order","recoveryIdempotencyKey":"fixture-not-found"});
        assert_eq!(
            recovery_service
                .handle_http("POST", "/orders/broker-recovery", not_found_body.clone())
                .status,
            409
        );
        assert!(!sink_path.exists());
        let launches_after_missing = sandbox.launches.load(Ordering::SeqCst);
        assert_eq!(launches_after_missing, launches_before + 1);
        assert_eq!(
            recovery_service
                .handle_http("POST", "/orders/broker-recovery", not_found_body)
                .status,
            409
        );
        assert_eq!(
            sandbox.launches.load(Ordering::SeqCst),
            launches_after_missing
        );
        assert!(service
            .runtime()
            .storage
            .get_json("plugin_operation_receipts", "fixture-order")
            .unwrap()
            .is_none());
        std::fs::rename(&held_path, &sink_path).unwrap();
        let receipt: PluginOperationResponse = if scenario == Scenario::RestartRecovery {
            drop(warden);
            let handoff = TradeAssemblyService::test_local_handoff(db.to_string_lossy()).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "broker_submission::e2e::restart_recovery_child",
                    "--nocapture",
                ])
                .env("F2_RECOVERY_CHILD_DB", &db)
                .env("F2_RECOVERY_CHILD_HANDOFF", handoff)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                "child probe did not execute"
            );
            serde_json::from_value(
                service
                    .runtime()
                    .storage
                    .get_json("plugin_operation_receipts", "fixture-order")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap()
        } else {
            let recovered = recovery_service.handle_http("POST", "/orders/broker-recovery", json!({"originalIdempotencyKey":"fixture-order","recoveryIdempotencyKey":"fixture-recovery"}));
            assert_eq!(recovered.status, 200, "{recovered:#?}");
            serde_json::from_value(recovered.body["receipt"].clone()).unwrap()
        };
        assert_eq!(receipt.payload["status"], "accepted");
        assert_eq!(receipt.payload["submissionCount"], 1);
        let expected_parent_launches = launches_before
            + if scenario == Scenario::RestartRecovery {
                1
            } else {
                2
            };
        assert_eq!(
            sandbox.launches.load(Ordering::SeqCst),
            expected_parent_launches
        );
        let mut replay = adapter.order_request.clone();
        replay.evidence_refs = adapter
            .quote
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .evidence_refs
            .clone();
        let replay_context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("fixture-order").unwrap(),
        );
        assert_eq!(
            operations.invoke(&replay, &replay_context).unwrap(),
            receipt
        );
        assert_eq!(
            sandbox.launches.load(Ordering::SeqCst),
            expected_parent_launches
        );
        *adapter.order.lock().unwrap() = Some(receipt);
        assert_eq!(
            service
                .runtime()
                .storage
                .list_json("plugin_broker_recovery_outcomes")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            service
                .runtime()
                .storage
                .list_json("plugin_broker_dispatch_claims")
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        adapter.quote.lock().unwrap().as_ref().unwrap().payload["quote"]["symbol"],
        "BTC/USD"
    );
    if !matches!(
        scenario,
        Scenario::Accepted
            | Scenario::GenericAccepted
            | Scenario::ReceiptStorageFailure
            | Scenario::LostResponse
            | Scenario::RestartRecovery
            | Scenario::AgentRecovery
    ) {
        assert!(adapter.order.lock().unwrap().is_none());
        assert!(
            !package
                .install_root
                .join(".f2-controlled-broker.sqlite")
                .exists(),
            "denied order reached sink: {scenario:?}"
        );
        assert_eq!(
            service
                .runtime()
                .storage
                .list_json("plugin_broker_submission_denials")
                .unwrap()
                .len(),
            1
        );
        let decisions = service
            .runtime()
            .storage
            .list_json(crate::finance_authority::DECISION_NS)
            .unwrap();
        let c5: Vec<_> = decisions
            .iter()
            .filter(|(_, value)| {
                value["request"]["pep_id"] == "pep-tradeassembly-broker-submission"
            })
            .collect();
        if scenario == Scenario::WardenDenied {
            assert_eq!(c5.len(), 1);
            assert_eq!(c5[0].1["decision"]["decision"], "deny");
        } else {
            assert!(c5.is_empty());
        }
        return;
    }
    let order = adapter.order.lock().unwrap().clone().unwrap();
    assert_eq!(order.payload["submissionCount"], 1);
    let sink = rusqlite::Connection::open_with_flags(
        package.install_root.join(".f2-controlled-broker.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (count, submissions): (i64, i64) = sink
        .query_row("SELECT COUNT(*), SUM(submissions) FROM orders", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!((count, submissions), (1, 1));
    let intents = service
        .runtime()
        .storage
        .list_json("broker_order_intents")
        .unwrap();
    assert_eq!(intents.len(), 1);
    let provenance = &intents[0].1["executionProvenance"];
    assert_eq!(provenance["kind"], "agent");
    assert_eq!(provenance["deploymentId"], "fixture-agent");
    assert!(provenance["runId"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(provenance["bindingDigest"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(service
        .runtime()
        .storage
        .list_json("execution_attempts")
        .unwrap()
        .is_empty());
    let decisions = service
        .runtime()
        .storage
        .list_json(crate::finance_authority::DECISION_NS)
        .unwrap();
    let c5: Vec<_> = decisions
        .iter()
        .filter(|(_, value)| value["request"]["pep_id"] == "pep-tradeassembly-broker-submission")
        .collect();
    assert_eq!(c5.len(), 1);
    let (command_id, decision) = c5[0];
    assert_eq!(decision["decision"]["decision"], "allow");
    assert_eq!(decision["request"]["idempotency_key"], "fixture-order");
    for namespace in [
        crate::finance_authority::RECEIPT_NS,
        crate::finance_authority::OUTCOME_NS,
    ] {
        assert_eq!(
            service
                .runtime()
                .storage
                .get_json(namespace, command_id)
                .unwrap()
                .is_some(),
            !(matches!(
                scenario,
                Scenario::LostResponse | Scenario::RestartRecovery | Scenario::AgentRecovery
            ) && namespace == crate::finance_authority::OUTCOME_NS),
            "missing {namespace}"
        );
    }
}
