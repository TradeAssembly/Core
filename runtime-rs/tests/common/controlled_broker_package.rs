use super::runtime_crate as tradeassembly_runtime;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

pub fn install_controlled_package(
    service: &TradeAssemblyService,
    root: &Path,
    binary: &Path,
) -> tradeassembly_runtime::ports::InstalledPluginPackage {
    fn live(value: &mut Value) {
        match value {
            Value::String(s) => {
                *s = match s.as_str() {
                    "paper" => "live".into(),
                    "broker.paper_order_submit" => "broker.live_order_submit".into(),
                    "broker.paper_order_preview" => "broker.live_order_preview".into(),
                    "broker.order_submit.paper" => "broker.order_submit.live".into(),
                    "paper_trading" => "live_order_submission".into(),
                    _ => return,
                }
            }
            Value::Array(values) => {
                values.iter_mut().for_each(live);
                let mut seen = Vec::new();
                values.retain(|v| {
                    if seen.contains(v) {
                        false
                    } else {
                        seen.push(v.clone());
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
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id": "broker.order_lookup.live",
            "description": "Read a previously submitted controlled order for recovery."
        }));
    for operation in manifest["operations"].as_array_mut().unwrap() {
        operation["protocol"] = json!("stdio");
        match operation["id"].as_str() {
            Some("marketdata.quote.read") => {
                operation["traits"]["outputSchemaRefs"] =
                    json!(["schema://tradeassembly.f2-controlled-broker/quote@1"])
            }
            Some("broker.live_order_submit") => {
                operation["traits"]["outputSchemaRefs"] =
                    json!(["schema://tradeassembly.f2-controlled-broker/order@1"])
            }
            _ => {}
        }
    }
    let submit = manifest["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|operation| operation["id"] == "broker.live_order_submit")
        .cloned()
        .expect("controlled live submit operation");
    let mut lookup = submit;
    lookup["id"] = json!("broker.order_lookup");
    lookup["capability"] = json!("broker.order_lookup.live");
    lookup["effect"] = json!("read");
    lookup["riskClass"] = json!("low");
    lookup["financeAction"] =
        json!({"actionId":"order.lookup.live","resourceType":"brokerage_account"});
    lookup["mandateRequired"] = json!(false);
    lookup["purpose"] = json!("recovery");
    lookup["approval"] = json!({"mode":"policy_configured"});
    lookup["supervision"] = json!({"mode":"automated_policy"});
    lookup["traits"] = json!({
        "modes":["live"], "instrumentFamilies":["crypto_spot","equity"],
        "dataShapes":["order_recovery"],
        "fields":["account_ref","client_order_id","provider_order_id","symbol","side","quantity","order_type","time_in_force"],
        "inputSchemaRefs":["schema://broker/order-lookup-request@1"],
        "outputSchemaRefs":["schema://tradeassembly.f2-controlled-broker/order@1"],
        "timeframes":[], "deterministic":true, "replayable":true
    });
    lookup["checkPacks"] = json!(["recovery_reconciliation"]);
    lookup["receiptClass"] = json!("recovery_evidence");
    manifest["operations"].as_array_mut().unwrap().push(lookup);
    let quote_schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"required":["quote"],"properties":{"quote":{"type":"object","additionalProperties":false,"required":["symbol","bid","ask","timestamp"],"properties":{"symbol":{"type":"string"},"bid":{"type":"string"},"ask":{"type":"string"},"timestamp":{"type":"string"}}}}}"#;
    let order_schema = br#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"required":["accountRef","clientOrderId","providerOrderId","symbol","side","quantity","orderType","timeInForce","status","intentDigest","submissionCount"],"properties":{"accountRef":{"type":"string"},"clientOrderId":{"type":"string"},"providerOrderId":{"type":"string"},"symbol":{"type":"string"},"side":{"type":"string"},"quantity":{"type":"string"},"orderType":{"type":"string"},"timeInForce":{"type":"string"},"status":{"const":"accepted"},"intentDigest":{"type":"string"},"submissionCount":{"type":"integer"}}}"#;
    let executable = std::fs::read(binary).unwrap();
    let canonical = tradeassembly_runtime::spec::canonical_hash(&manifest).unwrap();
    let canonical = canonical.strip_prefix("sha256:").unwrap_or(&canonical);
    let bytes = serde_yaml::to_string(&manifest).unwrap().into_bytes();
    let sha = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let target = tradeassembly_plugin_sdk::host_target();
    let binary_path = format!("bin/{target}/fixture");
    let descriptor = serde_json::to_vec(&json!({"packageContractVersion":"1","plugin":{"id":"example.mandate-live","version":"0.1.0"},"manifest":{"path":"manifest.yaml","sha256":sha(&bytes)},"targets":[{"target":target,"binary":{"path":binary_path,"sha256":sha(&executable)}}],"responseSchemas":[{"schemaRef":"schema://tradeassembly.f2-controlled-broker/quote@1","document":{"path":"schemas/quote.json","sha256":sha(quote_schema)}},{"schemaRef":"schema://tradeassembly.f2-controlled-broker/order@1","document":{"path":"schemas/order.json","sha256":sha(order_schema)}}],"compatibility":{"hostContract":{"minimum":tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION,"maximum":tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION},"sdkVersion":tradeassembly_plugin_sdk::SDK_VERSION}})).unwrap();
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (path, content, mode) in [
        ("tradeassembly-plugin.json", descriptor.as_slice(), 0o644),
        ("manifest.yaml", bytes.as_slice(), 0o644),
        (binary_path.as_str(), executable.as_slice(), 0o755),
        ("schemas/quote.json", quote_schema, 0o644),
        ("schemas/order.json", order_schema, 0o644),
        (
            ".f2-controlled-broker-fixture",
            b"fixture\n".as_slice(),
            0o644,
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        archive.append_data(&mut header, path, content).unwrap();
    }
    archive.finish().unwrap();
    let package = archive.into_inner().unwrap().finish().unwrap();
    let path = root.join("controlled-broker.tar.gz");
    std::fs::write(&path, &package).unwrap();
    let installed = service.handle_http("POST", "/plugins/packages", json!({"source":{"type":"package","locator":path,"package":{"type":"file","locator":path}},"integrity":{"packageSha256":sha(&package),"manifestSha256":canonical},"trust":{"level":"local-test"},"idempotencyKey":"install-controlled-broker"}));
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
    let package_sha = sha(&package);
    runtime
        .plugin_packages
        .get(&package_sha)
        .unwrap()
        .expect("installed controlled package")
}
