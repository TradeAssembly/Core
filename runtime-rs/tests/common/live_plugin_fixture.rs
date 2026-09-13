use super::runtime_crate as tradeassembly_runtime;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

pub fn install_live_metadata_fixture(service: &TradeAssemblyService, root: &std::path::Path) {
    fn live(value: &mut Value) {
        match value {
            Value::String(s) => {
                *s = match s.as_str() {
                    "paper" => "live".into(),
                    "broker.paper_order_submit" => "broker.live_order_submit".into(),
                    "broker.order_submit.paper" => "broker.order_submit.live".into(),
                    "paper_trading" => "live_order_submission".into(),
                    _ => return,
                }
            }
            Value::Array(values) => {
                values.iter_mut().for_each(live);
                let mut seen = Vec::new();
                values.retain(|value| {
                    if seen.contains(value) {
                        false
                    } else {
                        seen.push(value.clone());
                        true
                    }
                });
            }
            Value::Object(values) => values.values_mut().for_each(live),
            _ => {}
        }
    }
    let mut manifest: Value = serde_yaml::from_str(include_str!(
        "../../../plugin-contracts/manifests/simbroker.manifest.yaml"
    ))
    .unwrap();
    live(&mut manifest);
    manifest["metadata"]["id"] = json!("example.mandate-live");
    manifest["metadata"]["version"] = json!("0.1.0");
    manifest["runtime"] = json!({"protocol":"stdio","entrypoint":"fixture","timeoutSeconds":5});
    manifest["configuration"] = json!({"fields":[]});
    manifest["health"] = json!({"requiredConfiguration":[],"requiredCredentials":[],"connectionCheckOperation":null});
    let canonical = tradeassembly_runtime::spec::canonical_hash(&manifest).unwrap();
    let canonical = canonical.strip_prefix("sha256:").unwrap_or(&canonical);
    let bytes = serde_yaml::to_string(&manifest).unwrap().into_bytes();
    let executable = b"non-executed mandate metadata fixture";
    let binary_path = format!("bin/{}/fixture", tradeassembly_plugin_sdk::host_target());
    let sha = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let descriptor = serde_json::to_vec(&json!({"packageContractVersion":"1","plugin":{"id":"example.mandate-live","version":"0.1.0"},"manifest":{"path":"manifest.yaml","sha256":sha(&bytes)},"targets":[{"target":tradeassembly_plugin_sdk::host_target(),"binary":{"path":binary_path,"sha256":sha(executable)}}],"responseSchemas":[],"compatibility":{"hostContract":{"minimum":tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION,"maximum":tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION},"sdkVersion":tradeassembly_plugin_sdk::SDK_VERSION}})).unwrap();
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (path, content, mode) in [
        ("tradeassembly-plugin.json", descriptor.as_slice(), 0o644),
        ("manifest.yaml", bytes.as_slice(), 0o644),
        (binary_path.as_str(), executable.as_slice(), 0o755),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        archive.append_data(&mut header, path, content).unwrap();
    }
    archive.finish().unwrap();
    let package = archive.into_inner().unwrap().finish().unwrap();
    let path = root.join("controlled-live.tar.gz");
    std::fs::write(&path, &package).unwrap();
    let installed = service.handle_http("POST", "/plugins/packages", json!({"source":{"type":"package","locator":path,"package":{"type":"file","locator":path}},"integrity":{"packageSha256":sha(&package),"manifestSha256":canonical},"trust":{"level":"local-test"},"idempotencyKey":"install-live-fixture"}));
    assert!(installed.status < 300, "{installed:#?}");
    let created = service.handle_http("POST", "/plugins/instances", json!({"instanceRef":"mandate-live","pluginRef":"example.mandate-live","enabled":true,"configuration":{},"accountMode":"live","accountRef":"account://mandate-live/controlled"}));
    assert!(created.status < 300, "{created:#?}");
    let runtime = service.runtime();
    let mut instance = runtime
        .plugins
        .get_instance("mandate-live")
        .unwrap()
        .unwrap();
    runtime
        .credentials
        .store(
            instance["credentialRef"].as_str().unwrap(),
            &std::collections::BTreeMap::from([("fixture".into(), "controlled-test-value".into())]),
        )
        .unwrap();
    instance["accountMode"] = json!("live");
    instance["accountRef"] = json!("account://mandate-live/controlled");
    instance["credentialRevision"] = json!(1);
    let configuration_digest =
        tradeassembly_runtime::spec::canonical_hash(&instance["configuration"]).unwrap();
    instance["health"] = json!({"state":"ready","connectivityChecked":true,"checkedAtMs":runtime.clock.now_ms(),"account":{"id":"controlled","mode":"live","status":"ACTIVE","tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false},"binding":{"instanceRef":instance["instanceRef"],"pluginRef":instance["pluginRef"],"packageSha256":instance["activePackageSha256"],"configurationDigest":configuration_digest,"credentialRevision":1,"accountRef":instance["accountRef"],"accountMode":"live"}});
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("controlled-health").unwrap(),
    );
    runtime
        .plugins
        .put_instance("mandate-live", instance, &context)
        .unwrap();
}
