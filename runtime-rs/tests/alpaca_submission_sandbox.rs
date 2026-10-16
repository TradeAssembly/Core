// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Optional package acceptance: the real Core host, shipping sandbox launcher,
//! bundled Node and packaged plugin talk only to a loopback provider fixture.
#![cfg(unix)]
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tradeassembly_runtime::{
    adapters::{
        external_plugin_host::ExternalPluginHost, plugin_sandbox::SandboxRuntimePluginSandbox,
    },
    ports::*,
};
struct Fixture {
    package: InstalledPluginPackage,
    manifest: Value,
    instance: Value,
}
impl VersionedPort for Fixture {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl ClockPort for Fixture {
    fn now_ms(&self) -> i64 {
        42
    }
}
impl CredentialPort for Fixture {
    fn status(&self, _: &str) -> CredentialStatus {
        unimplemented!()
    }
    fn store(&self, _: &str, _: &BTreeMap<String, String>) -> Result<CredentialStatus, String> {
        unimplemented!()
    }
    fn revoke(&self, _: &str) -> Result<bool, String> {
        unimplemented!()
    }
    fn resolve_fields(&self, _: &str) -> Result<Option<BTreeMap<String, String>>, String> {
        Ok(Some(BTreeMap::from([
            ("api_key".into(), "synthetic-key".into()),
            ("api_secret".into(), "synthetic-secret".into()),
        ])))
    }
    fn resolve_broker_credentials(&self, _: &str) -> Result<Option<BrokerCredentials>, String> {
        unimplemented!()
    }
}
impl PluginPackagePort for Fixture {
    fn install(&self, _: &PluginPackageInstallRequest) -> Result<InstalledPluginPackage, String> {
        unimplemented!()
    }
    fn get(&self, _: &str) -> Result<Option<InstalledPluginPackage>, String> {
        Ok(Some(self.package.clone()))
    }
    fn remove(&self, _: &str) -> Result<bool, String> {
        unimplemented!()
    }
}
impl PluginRegistryPort for Fixture {
    fn list_manifests(&self) -> Result<Vec<Value>, String> {
        unimplemented!()
    }
    fn get_manifest(&self, _: &str) -> Result<Option<Value>, String> {
        Ok(Some(self.manifest.clone()))
    }
    fn install_manifest(&self, _: &str, _: Value, _: &SideEffectContext) -> Result<Value, String> {
        unimplemented!()
    }
    fn get_manifest_revision(&self, _: &str, _: &str) -> Result<Option<Value>, String> {
        unimplemented!()
    }
    fn activate_manifest_revision(
        &self,
        _: &str,
        _: &str,
        _: &SideEffectContext,
    ) -> Result<Value, String> {
        unimplemented!()
    }
    fn list_instances(&self) -> Result<Vec<Value>, String> {
        unimplemented!()
    }
    fn get_instance(&self, _: &str) -> Result<Option<Value>, String> {
        Ok(Some(self.instance.clone()))
    }
    fn put_instance(&self, _: &str, _: Value, _: &SideEffectContext) -> Result<Value, String> {
        unimplemented!()
    }
}

struct Step {
    path: String,
    status: &'static str,
    body: Value,
}
fn step(path: impl Into<String>, status: &'static str, body: Value) -> Step {
    Step {
        path: path.into(),
        status,
        body,
    }
}
fn receipt() -> Value {
    json!({"id":"provider-fixture", "client_order_id":"f".repeat(51), "symbol":"BTC/USD", "side":"buy", "type":"market", "time_in_force":"gtc", "qty":"0.0001", "filled_qty":"0.0001", "filled_avg_price":"67000", "status":"filled", "private":"provider-canary"})
}
fn invoke(steps: Vec<Step>) -> Result<PluginOperationResponse, String> {
    let root = PathBuf::from(
        std::env::var_os("ALPACA_TEST_PACKAGE_ROOT").expect("extracted native package required"),
    )
    .canonicalize()
    .unwrap();
    let launcher = PathBuf::from(
        std::env::var_os("ALPACA_TEST_SANDBOX_COMMAND")
            .expect("shipping sandbox launcher required"),
    )
    .canonicalize()
    .unwrap();
    let descriptor: Value =
        serde_json::from_slice(&std::fs::read(root.join("tradeassembly-plugin.json")).unwrap())
            .unwrap();
    let target = descriptor["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|target| target["target"] == tradeassembly_plugin_sdk::host_target())
        .expect("native package target");
    let binary = root.join(target["binary"]["path"].as_str().unwrap());
    assert_eq!(
        format!("{:x}", Sha256::digest(std::fs::read(&binary).unwrap())),
        target["binary"]["sha256"].as_str().unwrap()
    );
    let manifest_bytes =
        std::fs::read(root.join(descriptor["manifest"]["path"].as_str().unwrap())).unwrap();
    let digest = descriptor["manifest"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(format!("{:x}", Sha256::digest(&manifest_bytes)), digest);
    let manifest: Value = serde_yaml::from_slice(&manifest_bytes).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        for expected in steps {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "missing controlled provider request"
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("controlled provider accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut input = BufReader::new(&stream);
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            assert!(
                line.starts_with(&expected.path),
                "unexpected provider method/path"
            );
            let mut content_length = 0;
            loop {
                line.clear();
                assert!(input.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse::<usize>().unwrap();
                }
            }
            if expected.path.starts_with("POST ") {
                let mut bytes = vec![0; content_length];
                input.read_exact(&mut bytes).unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(
                    body,
                    json!({"symbol":"BTC/USD","qty":"0.0001","side":"buy","type":"market","time_in_force":"gtc","client_order_id":"f".repeat(51)})
                );
            }
            let body = expected.body.to_string();
            write!(stream, "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", expected.status, body.len(), body).unwrap();
        }
        // The finite fixture consumes every expected request. A duplicate POST
        // cannot be accepted after the listener closes.
    });
    let package = InstalledPluginPackage {
        plugin_ref: "tradeassembly.alpaca".into(),
        version: descriptor["plugin"]["version"].as_str().unwrap().into(),
        package_sha256: "a".repeat(64),
        manifest_sha256: digest.clone(),
        manifest: manifest.clone(),
        executable_path: binary,
        descriptor,
        install_root: root,
        host_contract: "1".into(),
        sdk_version: "0.1.0".into(),
    };
    let fixture = Arc::new(Fixture {
        package,
        manifest: json!({"manifest":manifest,"manifestDigest":digest,"package":{"packageSha256":"a".repeat(64)}}),
        instance: json!({"enabled":true,"instanceRef":"fixture","pluginRef":"tradeassembly.alpaca","activePackageSha256":"a".repeat(64),"credentialRef":"fixture-credential","configuration":{"mode":"paper","tradingApiUrl":url,"dataApiUrl":url}}),
    });
    let settings = tempfile::tempdir().unwrap();
    let sandbox =
        Arc::new(SandboxRuntimePluginSandbox::new(launcher, settings.path(), true, None).unwrap());
    let host = ExternalPluginHost::new(
        fixture.clone(),
        fixture.clone(),
        fixture.clone(),
        fixture,
        sandbox,
    );
    let request: PluginOperationRequest = serde_json::from_value(json!({"correlationId":"fixture","pluginInstanceRef":"fixture","pluginRef":"tradeassembly.alpaca","manifestFingerprint":digest,"operationId":"broker.paper_order_submit","capability":"broker.order_submit.paper","capabilityGraphRevisionId":"fixture","capabilityGraphFingerprint":"fixture","strategyId":"fixture","strategyVersionId":"fixture","strategySpecHash":"fixture","activationId":"fixture","attemptId":"fixture","evaluationTickId":"fixture","mode":"paper","purpose":"paper","accountRef":"fixture-account","timeoutMs":10000,"input":{"symbol":"BTC/USD","side":"buy","quantity":"0.0001","orderType":"market","timeInForce":"gtc","clientOrderId":"f".repeat(51)},"evidenceRefs":[]})).unwrap();
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("fixture-client").unwrap(),
    );
    let response = host.invoke(&request, &context);
    server.join().unwrap();
    response
}

#[test]
#[ignore = "requires ALPACA_TEST_PACKAGE_ROOT and ALPACA_TEST_SANDBOX_COMMAND (shipping native launcher)"]
fn packaged_submission_through_core_and_shipping_sandbox() {
    let response = invoke(vec![step("POST /v2/orders ", "200 OK", receipt())]).unwrap();
    assert_eq!(response.schema_ref, "schema://broker/order-receipt@1");
    assert_eq!(response.payload["accountRef"], "fixture-account");
    assert_eq!(response.payload["quantity"], "0.0001");
    assert_eq!(response.payload["clientOrderId"], "f".repeat(51));
    assert_eq!(response.payload["timeInForce"], "gtc");
    assert_eq!(response.freshness_state, "fresh");
    assert!(!response.reconciliation_required);
    assert!(!serde_json::to_string(&response).unwrap().contains("canary"));
}

#[test]
#[ignore = "requires ALPACA_TEST_PACKAGE_ROOT and ALPACA_TEST_SANDBOX_COMMAND (shipping native launcher)"]
fn packaged_provider_failures_survive_core_policy_without_resubmission() {
    for (status, code) in [
        ("401 Unauthorized", "provider_auth_failed"),
        ("403 Forbidden", "provider_auth_failed"),
        ("400 Bad Request", "provider_request_failed"),
        ("429 Too Many Requests", "provider_request_failed"),
        ("500 Internal Server Error", "provider_request_failed"),
    ] {
        let error = invoke(vec![step(
            "POST /v2/orders ", status,
            json!({"code":40310000,"message":"crypto trading is not enabled","private":"provider-canary"})
        )]).unwrap_err();
        let (primary, detail) = error
            .split_once("|provider|")
            .expect("bounded provider diagnostic");
        assert_eq!(primary, format!("plugin_operation_failed:{code}"));
        let detail: Value = serde_json::from_str(detail).unwrap();
        assert_eq!(detail["httpStatus"], status[..3].parse::<u16>().unwrap());
        assert_eq!(detail["providerCode"], 40310000);
        assert!(!error.contains("canary"));
    }
    let mut invalid = receipt();
    invalid.as_object_mut().unwrap().remove("qty");
    assert_eq!(
        invoke(vec![step("POST /v2/orders ", "200 OK", invalid)]).unwrap_err(),
        "plugin_operation_failed:provider_schema_invalid"
    );
    let error = invoke(vec![
        step(
            "POST /v2/orders ",
            "422 Unprocessable Entity",
            json!({"private":"provider-canary"}),
        ),
        step(
            format!(
                "GET /v2/orders:by_client_order_id?client_order_id={} ",
                "f".repeat(51)
            ),
            "404 Not Found",
            json!({"private":"provider-canary"}),
        ),
    ])
    .unwrap_err();
    let (primary, detail) = error
        .split_once("|provider|")
        .expect("rejected submission diagnostic");
    assert_eq!(primary, "plugin_operation_failed:provider_order_absent");
    let detail: Value = serde_json::from_str(detail).unwrap();
    assert_eq!(detail["httpStatus"], 422);
    assert!(!error.contains("canary"));
}
