// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Source-only fault proof. No HTTP, WorkOS, provider or packaged-binary proof.
//! Successful custody and receipt operations use real local adapters. The sink
//! independently reopens them before consuming ACK. All values are fixtures.

use super::*;
use crate::adapters::local::{credentials::LocalCredentialStore, sqlite::LocalSqliteStorage};
use crate::ports::{
    BrokerCredentials, CredentialPort, CredentialStatus, PortDescriptor, StoragePort, VersionedPort,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const ACTOR: &str = "oauth-fault-fixture-owner";
const INSTANCE: &str = "oauth-fault-instance";
const PLUGIN: &str = "oauth-fault-plugin";
const TOKEN: &str = "oauth-fault-fixture-identity-token";
const CREDENTIAL: &str = "oauth-fault-fixture-provider-token";

struct Sink {
    db: String,
    lose_response: bool,
}

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

pub(super) fn active() -> bool {
    SINK.with(|sink| sink.borrow().is_some())
}

pub(super) fn actor_token(actor: &str) -> Result<String, &'static str> {
    assert_eq!(actor, ACTOR);
    Ok(TOKEN.into())
}

fn sink_path(db: &str) -> PathBuf {
    PathBuf::from(format!("{db}.sink.json"))
}

fn observations(db: &str) -> Value {
    serde_json::from_slice(&std::fs::read(sink_path(db)).unwrap()).unwrap()
}

pub(super) fn post_json(url: &Url, token: &str, body: Value) -> Result<Value, &'static str> {
    assert_eq!(url.origin().ascii_serialization(), "https://relay.example");
    assert_eq!(token, TOKEN);
    SINK.with(|sink| {
        let sink = sink.borrow();
        let sink = sink.as_ref().expect("installed fixture sink");
        let mut evidence = observations(&sink.db);
        assert_eq!(body["instanceRef"], INSTANCE);
        let response = match url.path() {
            "/v1/connections/start" => {
                assert_eq!(body["mode"], "paper");
                assert_eq!(body["scopes"], json!(["data"]));
                evidence["startCount"] = json!(evidence["startCount"].as_u64().unwrap() + 1);
                json!({"connectionId":"fixture-connection", "authorizationUrl":"https://broker.example/authorize", "expiresAtMs":i64::MAX})
            }
            "/v1/connections/status" => {
                assert_eq!(body["connectionId"], "fixture-connection");
                evidence["statusCount"] = json!(evidence["statusCount"].as_u64().unwrap() + 1);
                json!({"status":"ready", "mode":"paper", "scopes":["data"], "accountId":"fixture-account", "accessToken":CREDENTIAL})
            }
            "/v1/connections/ack" => {
                assert_eq!(body["connectionId"], "fixture-connection");
                // These are new adapter instances, not reads from the service
                // objects or the response that is about to be acknowledged.
                let credentials = LocalCredentialStore::new(&sink.db);
                let fields = credentials.resolve_fields(INSTANCE).unwrap().unwrap();
                assert_eq!(fields.get("access_token").map(String::as_str), Some(CREDENTIAL));
                let storage = LocalSqliteStorage::new(&sink.db);
                let instance = storage.get_json("plugin_instances_v2", INSTANCE).unwrap().unwrap();
                let attempts = storage.list_json("plugin_oauth_attempts").unwrap();
                assert_eq!(attempts.len(), 1);
                let attempt = &attempts[0].1;
                assert_eq!(attempt["receipt"]["status"], "ready");
                assert_eq!(attempt["storedCredentialRevision"], instance["credentialRevision"]);
                assert_eq!(instance["credentialRevision"], 1);
                let handoff = credentials.resolve_fields(attempt["handoffRef"].as_str().unwrap()).unwrap().unwrap();
                assert_eq!(body["verifier"].as_str(), handoff.get("verifier").map(String::as_str));
                let acks = evidence["ackRequests"].as_array_mut().unwrap();
                if let Some(first) = acks.first() {
                    assert_eq!(first, &body, "recovery must retry identical ACK");
                }
                acks.push(body);
                evidence["consumedCount"] = json!(evidence["consumedCount"].as_u64().unwrap() + 1);
                // Consumption is durable before the response is lost.
                std::fs::write(sink_path(&sink.db), serde_json::to_vec(&evidence).unwrap()).unwrap();
                if sink.lose_response {
                    return Err("plugin_oauth_relay_unavailable");
                }
                json!({"acknowledged":true})
            }
            _ => panic!("unexpected fixture transport request"),
        };
        std::fs::write(sink_path(&sink.db), serde_json::to_vec(&evidence).unwrap()).unwrap();
        Ok(response)
    })
}

struct CountedCredentials {
    inner: Arc<dyn CredentialPort>,
    writes: Arc<AtomicUsize>,
    fail: bool,
}

impl VersionedPort for CountedCredentials {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        self.inner.descriptors()
    }
}

impl CredentialPort for CountedCredentials {
    fn status(&self, reference: &str) -> CredentialStatus {
        self.inner.status(reference)
    }
    fn store(
        &self,
        reference: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<CredentialStatus, String> {
        if reference == INSTANCE {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err("fixture credential failure".into());
            }
        }
        self.inner.store(reference, fields)
    }
    fn revoke(&self, reference: &str) -> Result<bool, String> {
        self.inner.revoke(reference)
    }
    fn resolve_fields(&self, reference: &str) -> Result<Option<BTreeMap<String, String>>, String> {
        self.inner.resolve_fields(reference)
    }
    fn resolve_broker_credentials(
        &self,
        reference: &str,
    ) -> Result<Option<BrokerCredentials>, String> {
        self.inner.resolve_broker_credentials(reference)
    }
}

fn decorate(
    mut service: TradeAssemblyService,
    fail: bool,
) -> (TradeAssemblyService, Arc<AtomicUsize>) {
    service.connection_profile = Some(serde_json::from_value(json!({
        "environment":"production", "issuer":"https://identity.example", "clientId":"fixture",
        "organizationId":null, "hubBaseUrl":"https://hub.example", "redirectUri":"http://127.0.0.1:8976/callback",
        "relayOrigins":["https://relay.example"], "authorizationOrigins":["https://broker.example"],
        "entitlementChecks":[], "checkoutUrl":null
    })).unwrap());
    let mut runtime = (*service.runtime()).clone();
    let writes = Arc::new(AtomicUsize::new(0));
    runtime.credentials = Arc::new(CountedCredentials {
        inner: runtime.credentials,
        writes: writes.clone(),
        fail,
    });
    (
        service
            .with_test_runtime(runtime)
            .for_authenticated_invocation("oidc", ACTOR, None, None),
        writes,
    )
}

struct Fixture {
    _directory: tempfile::TempDir,
    service: TradeAssemblyService,
    writes: Arc<AtomicUsize>,
}

impl Fixture {
    fn new(fail: bool, lose_response: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let db = directory
            .path()
            .join("oauth.db")
            .to_string_lossy()
            .to_string();
        let (service, writes) = decorate(TradeAssemblyService::test_local(&db), fail);
        std::fs::write(
            sink_path(&db),
            serde_json::to_vec(
                &json!({"startCount":0,"statusCount":0,"ackRequests":[],"consumedCount":0}),
            )
            .unwrap(),
        )
        .unwrap();
        SINK.with(|sink| *sink.borrow_mut() = Some(Sink { db, lose_response }));
        let mut manifest = plugin_lifecycle::get_manifest(&service, "tradeassembly.simbroker").body
            ["manifest"]
            .clone();
        manifest["metadata"]["id"] = json!(PLUGIN);
        manifest["configuration"]["fields"] = json!([
            {"id":"access_token", "label":"Fixture token", "description":"Fixture", "inputType":"password", "required":false, "storageClass":"credential"}
        ]);
        manifest["configuration"]["oauth"] = json!({
            "label":"Fixture connection", "provider":"fixture",
            "credentialField":"access_token", "scopes":[{"id":"data", "label":"Fixture data"}],
            "environments":[{"id":"production", "label":"Fixture production", "relayBaseUrl":"https://relay.example"}]
        });
        manifest["health"]["requiredCredentials"] = json!(["access_token"]);
        manifest["health"]
            .as_object_mut()
            .unwrap()
            .remove("credentialAlternatives");
        let digest = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json_canonicalizer::to_vec(&manifest).unwrap())
        );
        let installed = plugin_lifecycle::install_manifest(
            &service,
            json!({
                "manifest":manifest, "source":{"type":"package","locator":"fixture.json","package":{"type":"file","locator":"fixture.json"}},
                "integrity":{"manifestSha256":digest}
            }),
        );
        assert_eq!(installed.status, 200, "{installed:?}");
        let created = plugin_lifecycle::create_instance(
            &service,
            json!({"instanceRef":INSTANCE,"pluginRef":PLUGIN,"accountMode":"paper","configuration":{}}),
        );
        assert_eq!(created.status, 200, "{created:?}");
        let started = start_blocking(
            &service,
            json!({"instanceRef":INSTANCE,"environment":"production","scopes":["data"],"idempotency_key":"fixture-start"}),
        );
        assert_eq!(started.status, 200, "{started:?}");
        Self {
            _directory: directory,
            service,
            writes,
        }
    }
    fn poll(&self) -> ServiceResponse {
        status_blocking(&self.service, poll_body())
    }
    fn attempt(&self) -> Value {
        self.service
            .runtime()
            .storage
            .list_json("plugin_oauth_attempts")
            .unwrap()[0]
            .1
            .clone()
    }
    fn evidence(&self) -> Value {
        observations(self.service.db())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        SINK.with(|sink| *sink.borrow_mut() = None);
    }
}

fn poll_body() -> Value {
    json!({"instanceRef":INSTANCE,"connectionId":"fixture-connection"})
}

#[test]
fn custody_and_durable_receipt_precede_ack() {
    let fixture = Fixture::new(false, false);
    let response = fixture.poll();
    assert_eq!(response.status, 200);
    assert_eq!(response.body["status"], "ready");
    assert_eq!(fixture.evidence()["consumedCount"], 1);
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.attempt()["acknowledged"], true);
}

#[test]
fn credential_failure_never_sends_ack() {
    let fixture = Fixture::new(true, false);
    let response = fixture.poll();
    assert_eq!(response.status, 409);
    assert_eq!(
        response.body["error"]["code"],
        "plugin_oauth_credential_store_failed"
    );
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.evidence()["consumedCount"], 0);
    assert!(fixture.attempt()["receipt"].is_null());
    assert!(LocalCredentialStore::new(fixture.service.db())
        .resolve_fields(INSTANCE)
        .unwrap()
        .is_none());
}

#[test]
fn receipt_failure_never_sends_ack() {
    let fixture = Fixture::new(false, false);
    rusqlite::Connection::open(fixture.service.db()).unwrap().execute_batch(
        "CREATE TRIGGER fault_receipt_insert BEFORE INSERT ON tradeassembly_kv
         WHEN NEW.namespace='plugin_oauth_attempts' AND json_type(NEW.value_json,'$.receipt')='object'
         BEGIN SELECT RAISE(ABORT, 'fixture receipt failure'); END;"
    ).unwrap();
    assert_eq!(fixture.poll().status, 500);
    assert_eq!(fixture.evidence()["consumedCount"], 0);
    assert!(fixture.attempt()["receipt"].is_null());
    // Custody already happened; do not claim a rollback the code does not do.
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    assert!(LocalCredentialStore::new(fixture.service.db())
        .resolve_fields(INSTANCE)
        .unwrap()
        .is_some());
}

#[test]
fn consumed_ack_lost_response_recovers_in_distinct_process_without_rewrite() {
    let fixture = Fixture::new(false, true);
    let response = fixture.poll();
    assert_eq!(response.status, 200);
    assert_eq!(response.body["ackPending"], true);
    assert_eq!(fixture.evidence()["consumedCount"], 1);
    let before = fixture.attempt();
    assert_ne!(before["acknowledged"], true);
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    let token = TradeAssemblyService::test_local_handoff(fixture.service.db()).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TRADEASSEMBLY_OAUTH_FAULT_DB", fixture.service.db())
        .env("TRADEASSEMBLY_OAUTH_FAULT_HANDOFF", token)
        .args([
            "--exact",
            "service::plugin_oauth::fault_tests::restart_child",
            "--ignored",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert_ne!(child.id(), std::process::id());
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "restart child failed");
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("restart child timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fixture.evidence()["statusCount"], 1);
    assert_eq!(fixture.evidence()["consumedCount"], 2);
    assert_eq!(fixture.attempt()["acknowledged"], true);
    assert!(LocalCredentialStore::new(fixture.service.db())
        .resolve_fields(before["handoffRef"].as_str().unwrap())
        .unwrap()
        .is_none());
    let evidence = fixture.evidence();
    assert_eq!(fixture.poll().status, 200);
    assert_eq!(
        fixture.evidence(),
        evidence,
        "completed replay must perform no network request"
    );
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
}

#[test]
#[ignore = "invoked only by the bounded parent over an owned fixture"]
fn restart_child() {
    let db = std::env::var("TRADEASSEMBLY_OAUTH_FAULT_DB").unwrap();
    let token = std::env::var("TRADEASSEMBLY_OAUTH_FAULT_HANDOFF").unwrap();
    let (service, writes) = decorate(
        TradeAssemblyService::test_local_with_handoff(&db, token).unwrap(),
        false,
    );
    SINK.with(|sink| {
        *sink.borrow_mut() = Some(Sink {
            db,
            lose_response: false,
        })
    });
    let response = status_blocking(&service, poll_body());
    assert_eq!(response.status, 200);
    assert_eq!(response.body["status"], "ready");
    assert!(response.body["ackPending"].is_null());
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[test]
fn changed_credential_revision_blocks_ack_recovery() {
    let fixture = Fixture::new(false, true);
    assert_eq!(fixture.poll().body["ackPending"], true);
    let changed = plugin_lifecycle::store_credentials(
        &fixture.service,
        INSTANCE,
        json!({"credentials":{"access_token":"replacement-fixture"}}),
    );
    assert_eq!(changed.status, 200);
    let before = fixture.evidence();
    let response = fixture.poll();
    assert_eq!(response.status, 409);
    assert_eq!(
        response.body["error"]["code"],
        "plugin_oauth_credentials_changed"
    );
    assert_eq!(fixture.evidence(), before);
}
