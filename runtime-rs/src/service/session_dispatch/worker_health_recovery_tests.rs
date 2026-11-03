use super::*;
use crate::ports::{
    IdentityClaims, IdentityPort, InstalledPluginPackage, PluginOperationPort,
    PluginOperationRequest, PluginOperationResponse, PluginPackageInstallRequest,
    PluginPackagePort, PortDescriptor, VersionedPort,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

struct FixtureIdentity;
impl VersionedPort for FixtureIdentity {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl IdentityPort for FixtureIdentity {
    fn resolve(&self, subject: &str, _: i64) -> Result<IdentityClaims, String> {
        Ok(IdentityClaims {
            issuer: "worker.fixture".into(),
            subject: subject.into(),
            audience: vec![],
            assurance: None,
            expires_at_ms: i64::MAX,
        })
    }
}
fn fixture() -> (tempfile::TempDir, TradeAssemblyService) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("worker.db").to_string_lossy().into_owned();
    let _ = crate::build_identity::install("1111111111111111111111111111111111111111");
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.identity = Arc::new(FixtureIdentity);
    let s = TradeAssemblyService::from_runtime(&db, runtime).for_authenticated_invocation(
        "worker.fixture",
        "owner",
        None,
        None,
    );
    s.bind_owned_object("strategy", "worker.owned").unwrap();
    (dir, s)
}
// Authored retained scheduler records, never financial/position/fill records.
// The intent itself is persisted by the actual normal worker set() path.
fn retain(s: &TradeAssemblyService, occurrence: &str) -> String {
    let request = DispatchRequest {
        occurrence_id: occurrence.into(),
        scope: data_plans::PlanScope {
            strategy_id: "worker.owned".into(),
            strategy_version_id: "version".into(),
            strategy_spec_hash: "hash".into(),
        },
        capability_graph_revision: "graph".into(),
        mode: "paper".into(),
        subjects: SubjectPolicy::AllSelected { max_members: 1 },
        event_start_ms: 86400000,
        event_end_ms: 172800000,
        interval_ms: 86400000,
        max_frames_per_step: 1,
        source_captures_by_subject: BTreeMap::new(),
        execution_activation_id: None,
    };
    let who = owner(s).unwrap();
    let id = job_id(&request, &who).unwrap();
    let mut job = Job {
        schema_version: "tradeassembly.session_dispatch.v1".into(),
        job_id: id.clone(),
        request,
        core_revision: revision().unwrap(),
        owner_sha256: who,
        schedule_sha256: "a".repeat(64),
        created_at_ms: s.runtime().clock.now_ms(),
        started_at_ms: None,
        status: Status::Pending,
        paused: false,
        generation: 0,
        attempts: 0,
        next_attempt_at_ms: 0,
        error: None,
        record_sha256: String::new(),
        fencing_token: 0,
        queue_message_id: None,
        pending: None,
        results: BTreeMap::new(),
        subjects: None,
    };
    job.record_sha256 = record_hash(&job).unwrap();
    s.runtime()
        .storage
        .put_json(
            JOBS,
            &id,
            value(&job).unwrap(),
            &context("retain-worker-fixture").unwrap(),
        )
        .unwrap();
    set(s, &id, true).unwrap();
    id
}
fn update(s: &TradeAssemblyService, id: &str, change: impl FnOnce(&mut Job)) {
    let (mut job, old) = read(s, id).unwrap();
    change(&mut job);
    persist(s, old, &mut job).unwrap();
}

// Mock package/operation ports isolate this ordering regression. They exercise
// actual normal Core HTTP/C3/registry/health persistence and worker intent;
// they do not qualify executable integrity, native providers or automatic trades.
struct FixturePackage(InstalledPluginPackage);
impl VersionedPort for FixturePackage {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginPackagePort for FixturePackage {
    fn get(&self, d: &str) -> Result<Option<InstalledPluginPackage>, String> {
        Ok((d == self.0.package_sha256).then(|| self.0.clone()))
    }
    fn install(&self, _: &PluginPackageInstallRequest) -> Result<InstalledPluginPackage, String> {
        panic!("unexpected install")
    }
    fn remove(&self, _: &str) -> Result<bool, String> {
        panic!("unexpected remove")
    }
}
struct HeldHealth {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    health_reads: AtomicUsize,
    next_children: AtomicUsize,
    clock: Arc<dyn crate::ports::ClockPort>,
}
impl VersionedPort for HeldHealth {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginOperationPort for HeldHealth {
    fn invoke(
        &self,
        r: &PluginOperationRequest,
        _: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        let payload = if r.operation_id == "account.health" {
            self.health_reads.fetch_add(1, Ordering::SeqCst);
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(()).unwrap();
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }
            json!({"account":{"id":"synthetic-owned-paper","mode":"paper","status":"ACTIVE","tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false}})
        } else {
            assert_eq!(r.operation_id, "next");
            self.next_children.fetch_add(1, Ordering::SeqCst);
            json!({"value":true})
        };
        Ok(PluginOperationResponse {
            correlation_id: r.correlation_id.clone(),
            schema_ref: "schema://worker/health@1".into(),
            content_hash: hash(&payload)?,
            payload,
            observed_at_ms: self.clock.trusted_now_ms()?,
            source_event_id: "mock-worker-read".into(),
            freshness_state: "fresh".into(),
            evidence_refs: vec![],
            deterministic: true,
            replayable: false,
            provider_outcome_id: None,
            reconciliation_required: false,
        })
    }
}
fn install_held_health(s: &TradeAssemblyService, port: Arc<HeldHealth>) -> TradeAssemblyService {
    let (graph, _, _) = crate::data_pipeline::tests::fixture();
    let mut health = graph
        .nodes
        .iter()
        .find_map(|n| n.selected.as_ref())
        .unwrap()
        .operation
        .clone();
    health.id = "account.health".into();
    health.capability = "account.health".into();
    health.effect = "read".into();
    health.risk = "read".into();
    health.purpose = "account_health".into();
    health.credential_grant_required = false;
    health.account_binding_required = false;
    health.traits = crate::ports::PluginOperationTraits {
        modes: vec!["paper".into()],
        output_schema_refs: vec!["schema://worker/health@1".into()],
        ..Default::default()
    };
    let mut next = health.clone();
    next.id = "next".into();
    next.capability = "test.worker.next".into();
    let manifest = json!({"metadata":{"id":"fixture.worker.health","version":"1","provider":{"id":"fixture.worker.health"}},
        "capabilities":[{"id":"account.health"},{"id":"test.worker.next"}],"operations":[health,next]});
    let package = InstalledPluginPackage {
        plugin_ref: "fixture.worker.health".into(),
        version: "1".into(),
        package_sha256: "b".repeat(64),
        manifest_sha256: hash(&manifest).unwrap(),
        manifest: manifest.clone(),
        descriptor: json!({}),
        install_root: "/mock/not-executed".into(),
        executable_path: "/mock/not-executed/plugin".into(),
        host_contract: "1".into(),
        sdk_version: "fixture".into(),
    };
    let mut runtime = (*s.runtime()).clone();
    runtime.plugin_packages = Arc::new(FixturePackage(package.clone()));
    runtime.plugin_operations = port;
    let mut service = s.clone();
    service.runtime = Arc::new(runtime);
    let runtime = service.runtime();
    let ctx = context("held-health-install").unwrap();
    runtime.plugins.install_manifest("fixture.worker.health",json!({"pluginRef":"fixture.worker.health","manifest":manifest,
        "manifestDigest":package.manifest_sha256,"source":{"type":"file"},"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx).unwrap();
    runtime.plugins.put_instance("worker.health",json!({"instanceRef":"worker.health","pluginRef":"fixture.worker.health","providerRef":"fixture.worker.health",
        "enabled":true,"activePackageSha256":package.package_sha256,"configuration":{"mode":"paper"},"accountMode":"paper",
        "health":{"revision":0,"state":"ready"}}),&ctx).unwrap();
    service
        .bind_owned_object("plugin_instance", "worker.health")
        .unwrap();
    let mut grant = crate::capability::local_full_entitlements()
        .into_iter()
        .next()
        .unwrap();
    grant.grant_id = "worker-next".into();
    grant.capability = "test.worker.next".into();
    runtime
        .storage
        .put_json(
            "entitlement_grants",
            "worker-next",
            serde_json::to_value(grant).unwrap(),
            &ctx,
        )
        .unwrap();
    service
}
#[test]
fn stop_or_pause_during_actual_health_http_blocks_next_nested_child_but_preserves_completed_read() {
    for pause in [false, true] {
        let (_dir, s) = fixture();
        let id = retain(
            &s,
            if pause {
                "pause-held-health"
            } else {
                "stop-held-health"
            },
        );
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let port = Arc::new(HeldHealth {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(release_rx),
            health_reads: AtomicUsize::new(0),
            next_children: AtomicUsize::new(0),
            clock: s.runtime().clock.clone(),
        });
        let s = install_held_health(&s, port.clone());
        let background = background_service(&s, &id);
        let thread_id = id.clone();
        let joined = std::thread::spawn(move || {
            let health = background.handle_http(
                "POST",
                "/plugins/instances/worker.health/health:refresh",
                json!({"accountMode":"paper","idempotencyKey":"held-health-http"}),
            );
            // Completed health evidence may be journaled despite a later stop.
            assert_eq!(health.status, 200, "{}", health.body);
            assert_eq!(
                health.body["instance"]["health"]["state"], "ready",
                "{}",
                health.body
            );
            assert!(after_health(&background, &thread_id).unwrap().is_none());
            // Both nested data-step and pure-validation clones keep provenance.
            let nested = crate::service::data_evaluation::request_service(&background);
            crate::service::package_validation::with_round(&nested,|nested|Ok(nested.handle_http("POST",
                "/plugins/instances/worker.health/operations/next:invoke",json!({"mode":"paper","purpose":"account_health","input":{},"idempotencyKey":"must-not-run"})))).unwrap()
        });
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let control = s.handle_http(
            "POST",
            "/scheduler/run",
            json!({"accountMode":"paper","sessionDispatch":if pause {
            json!({"kind":"pause","jobId":id,"paused":true})
        } else {json!({"kind":"stop_worker","jobId":id})},"idempotencyKey":"stop-during-health"}),
        );
        assert_eq!(control.status, 200, "{}", control.body);
        assert_eq!(control.body["ok"], true, "{}", control.body);
        release_tx.send(()).unwrap();
        let denied = joined.join().unwrap();
        assert_eq!(denied.status, 409, "{}", denied.body);
        assert_eq!(
            denied.body["error"]["code"],
            if pause {
                "automatic_health_job_intent_stale"
            } else {
                "automatic_health_worker_intent_disabled"
            }
        );
        assert_eq!(port.health_reads.load(Ordering::SeqCst), 1);
        assert_eq!(port.next_children.load(Ordering::SeqCst), 0);
        let record = s
            .runtime()
            .plugins
            .get_instance("worker.health")
            .unwrap()
            .unwrap();
        assert_eq!(record["health"]["account"]["id"], "synthetic-owned-paper");
        // Finite manual commands do not inherit the background worker restriction.
        let manual=s.handle_http("POST","/plugins/instances/worker.health/operations/next:invoke",json!({"mode":"paper","purpose":"account_health","input":{},"idempotencyKey":"manual-after-stop"}));
        assert_eq!(manual.status, 200, "{}", manual.body);
        assert_eq!(port.next_children.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn resume_isolates_owned_eligibility_and_intent_failures_and_continues_other_jobs() {
    for invalid_intent in [false, true] {
        let (_dir, s) = fixture();
        let a = retain(&s, "resume-A");
        let b = retain(&s, "resume-B");
        let mut ids = [a, b];
        ids.sort();
        let bad = &ids[0];
        let good = &ids[1];
        if invalid_intent {
            let mut raw = s.runtime().storage.get_json(INTENTS, bad).unwrap().unwrap();
            raw["recordSha256"] = json!("tampered");
            s.runtime()
                .storage
                .put_json(INTENTS, bad, raw, &context("malformed-intent").unwrap())
                .unwrap();
        } else {
            // A retained terminal job with revoked/missing owned activation is
            // refused by the actual automatic health eligibility path.
            update(&s, bad, |j| {
                j.status = Status::Complete;
                j.request.execution_activation_id = Some("unavailable-owned-activation".into());
            });
        }
        let mut started = Vec::new();
        let count = resume_owned_with(&s, |_, id| {
            started.push(id.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(started, vec![good.clone()]);
        let blocked = status(&s, bad);
        assert_eq!(blocked["running"], false);
        assert_eq!(
            blocked["lastError"],
            if invalid_intent {
                "session_dispatch_worker_intent_invalid"
            } else {
                "automatic_health_activation_not_available"
            }
        );
        assert!(enabled(&s, good).unwrap());
        assert!(read(&s, good).is_ok());
    }
}
#[test]
fn resume_still_rejects_missing_global_owner_and_never_starts_foreign_intent() {
    let (_dir, s) = fixture();
    let id = retain(&s, "foreign-intent");
    let foreign = s.for_authenticated_invocation("worker.fixture", "foreign", None, None);
    assert_eq!(
        resume_owned_with(&foreign, |_, _| panic!("foreign job started")).unwrap(),
        0
    );
    let anonymous = TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone());
    assert_eq!(
        resume_owned_with(&anonymous, |_, _| panic!("anonymous worker started")).unwrap_err(),
        "session_dispatch_human_required"
    );
    assert!(enabled(&s, &id).unwrap());
}
