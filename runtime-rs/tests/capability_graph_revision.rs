use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tar::{Builder, Header};
use tempfile::TempDir;
use tradeassembly_runtime::finance_authority::TestFinanceAuthority;
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::runtime_config::{RuntimeBuilder, RuntimeConfig};
use tradeassembly_runtime::service::TradeAssemblyService;

const EPOCH: &str = "2026-07-22T12:00:00Z";

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3/valid/static-equity.json")
}

fn test_service(name: &str) -> (TempDir, String, TradeAssemblyService) {
    let database = tempfile::Builder::new()
        .prefix(name)
        .tempdir()
        .expect("temporary database directory");
    let db = database
        .path()
        .join("tradeassembly.db")
        .to_string_lossy()
        .into_owned();
    let service = configured_test_service(&db);
    (database, db, service)
}

fn configured_test_service(db: &str) -> TradeAssemblyService {
    let mut config = RuntimeConfig::local(db.to_string());
    config.oidc_issuer = "https://identity.example/user_management/client_test".to_string();
    config.oidc_client_id = "client_test".to_string();
    let (runtime, _) = RuntimeBuilder::new(config)
        .with_finance_authority(Arc::new(TestFinanceAuthority))
        .build()
        .expect("configured test runtime");
    TradeAssemblyService::from_runtime(db.to_string(), runtime)
}

fn publish_static_strategy(service: &TradeAssemblyService) -> (String, String, String) {
    let mut spec: Value =
        serde_json::from_slice(&fs::read(fixture_path()).expect("strategy fixture"))
            .expect("strategy fixture JSON");
    spec["stages"]["evaluate"]["substeps"][0]["config"] = json!({
        "tradeassembly.expression": "bar.close > 0",
        "tradeassembly.quantity": 1_000_000
    });
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "Capability graph fixture", "spec": spec}),
    );
    assert_eq!(created.status, 201, "{:#}", created.body);
    let strategy_id = created.body["body"]["strategy"]["id"]
        .as_str()
        .expect("strategy id")
        .to_string();
    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": created.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(published.status, 200, "{:#}", published.body);
    assert_eq!(published.body["body"]["published"], true);
    (
        strategy_id,
        published.body["body"]["version"]["id"]
            .as_str()
            .expect("version id")
            .to_string(),
        published.body["body"]["version"]["specHash"]
            .as_str()
            .expect("spec hash")
            .to_string(),
    )
}

fn graph_body(strategy_id: &str, version_id: &str, spec_hash: &str) -> Value {
    json!({
        "configId": "cfg_capability_graph_fixture",
        "strategyId": strategy_id,
        "strategyVersionId": version_id,
        "strategySpecHash": spec_hash,
        "mode": "backtest",
        "evaluationEpoch": EPOCH,
        "idempotencyKey": "capability-graph-fixture-v1",
        "bindings": {
            "req_bars": {
                "pluginInstanceRef": "local-data",
                "pluginRef": "tradeassembly.local-data",
                "operationId": "marketdata.bars.read_v1"
            },
            "req_logic": {
                "pluginInstanceRef": "core-runtime",
                "pluginRef": "tradeassembly.core-runtime",
                "operationId": "expression.cel.evaluate_v1"
            },
            "req_calendar": {
                "pluginInstanceRef": "core-runtime",
                "pluginRef": "tradeassembly.core-runtime",
                "operationId": "calendar.session.resolve_v1"
            },
            "backtest_dataset": {
                "pluginInstanceRef": "local-data",
                "pluginRef": "tradeassembly.local-data",
                "operationId": "marketdata.bars.read_v1"
            }
        },
        "additionalRequirements": [{
            "requirementId": "backtest_dataset",
            "capability": "market_data.bars.read@1",
            "purpose": "Read immutable backtest dataset bars",
            "requiredFor": ["backtest"],
            "stageRefs": ["inputs"],
            "substepRefs": ["load_bars"],
            "constraints": {
                "instrumentFamilies": ["equity"],
                "dataShapes": ["bars"],
                "operations": ["read"],
                "fields": ["open", "high", "low", "close", "volume"],
                "inputPaths": ["market_data.source_bars"],
                "schemaRefs": ["schema://market-data/bars@1"],
                "timeframe": "1h",
                "maxFreshness": "PT5M",
                "deterministic": true,
                "replayable": true
            },
            "dependencyRefs": [],
            "fallbackPolicy": "none",
            "policyTags": ["immutable_dataset"]
        }]
    })
}

fn response_body(response: &tradeassembly_runtime::service::ServiceResponse) -> Value {
    response
        .body
        .get("body")
        .cloned()
        .unwrap_or_else(|| response.body.clone())
}

fn node<'a>(graph: &'a Value, requirement_id: &str) -> &'a Value {
    graph["nodes"]
        .as_array()
        .expect("graph nodes")
        .iter()
        .find(|node| node["nodeId"] == requirement_id)
        .unwrap_or_else(|| panic!("missing graph node {requirement_id}: {graph:#}"))
}

fn save_graph(service: &TradeAssemblyService, body: Value) -> Value {
    let response = service.handle_http("POST", "/capability-graph-revisions", body);
    assert_eq!(response.status, 200, "{:#}", response.body);
    let saved = response_body(&response);
    assert_eq!(saved["ok"], true, "{saved:#}");
    saved["revision"].clone()
}

fn currentness(service: &TradeAssemblyService, revision_id: &str) -> Value {
    response_body(&service.handle_http(
        "POST",
        "/capability-graph-revisions/currentness",
        json!({"revisionId": revision_id, "evaluationEpoch": EPOCH}),
    ))
}

fn manifest_digest(manifest: &Value) -> String {
    let canonical = serde_json_canonicalizer::to_vec(manifest).expect("canonical manifest");
    format!("sha256:{:x}", Sha256::digest(canonical))
}

fn append_package_file(
    archive: &mut Builder<GzEncoder<Vec<u8>>>,
    path: &str,
    bytes: &[u8],
    mode: u32,
) {
    let mut header = Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_cksum();
    archive
        .append_data(&mut header, path, bytes)
        .expect("append package fixture");
}

#[derive(Clone)]
struct PluginPackageFixture {
    package_sha256: String,
    manifest_sha256: String,
    locator: String,
}

impl PluginPackageFixture {
    fn install_body(&self) -> Value {
        json!({
            "source": {
                "type": "package",
                "locator": self.locator,
                "package": {"type": "file", "locator": self.locator}
            },
            "integrity": {
                "packageSha256": self.package_sha256,
                "manifestSha256": self.manifest_sha256
            },
            "trust": {"level": "local-test"}
        })
    }
}

fn install_credential_data_plugin(
    service: &TradeAssemblyService,
    directory: &Path,
) -> PluginPackageFixture {
    let manifests = service.handle_http("GET", "/plugins/manifests", json!({}));
    let mut manifest = manifests.body["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.local-data")
        .expect("local-data manifest")["manifest"]
        .clone();
    let paper_submit = manifests.body["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
        .expect("simbroker manifest")["manifest"]["operations"]
        .as_array()
        .expect("simbroker operations")
        .iter()
        .find(|operation| operation["id"] == "broker.paper_order_submit")
        .expect("paper submit operation")
        .clone();
    let paper_submit_capability = manifests.body["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
        .expect("simbroker manifest")["manifest"]["capabilities"]
        .as_array()
        .expect("simbroker capabilities")
        .iter()
        .find(|capability| capability["id"] == paper_submit["capability"])
        .expect("paper submit capability")
        .clone();
    manifest["operations"]
        .as_array_mut()
        .expect("fixture operations")
        .push(paper_submit);
    manifest["capabilities"]
        .as_array_mut()
        .expect("fixture capabilities")
        .push(paper_submit_capability);
    manifest["metadata"]["id"] = json!("example.credential-data");
    manifest["metadata"]["name"] = json!("Credential Data Fixture");
    manifest["metadata"]["provider"] = json!({"id": "example", "name": "Example"});
    manifest["configuration"] = json!({
        "fields": [
            {
                "id": "api_key",
                "label": "API key",
                "description": "Fixture key.",
                "inputType": "password",
                "required": true,
                "storageClass": "credential"
            },
            {
                "id": "api_secret",
                "label": "API secret",
                "description": "Fixture secret.",
                "inputType": "password",
                "required": true,
                "storageClass": "credential"
            }
        ]
    });
    manifest["health"] = json!({
        "requiredConfiguration": [],
        "requiredCredentials": ["api_key", "api_secret"],
        "connectionCheckOperation": null
    });
    for operation in manifest["operations"]
        .as_array_mut()
        .expect("manifest operations")
    {
        operation["credentialGrantRequired"] = json!(true);
        operation["accountBindingRequired"] = json!(true);
    }
    manifest["runtime"] = json!({
        "protocol": "stdio",
        "entrypoint": "fixture-plugin",
        "timeoutSeconds": 5
    });
    let manifest_bytes = serde_yaml::to_string(&manifest)
        .expect("serialize package manifest")
        .into_bytes();
    let manifest_file_sha = format!("{:x}", Sha256::digest(&manifest_bytes));
    let manifest_sha = manifest_digest(&manifest)
        .strip_prefix("sha256:")
        .expect("prefixed manifest digest")
        .to_string();
    let executable = b"credential-data-package-fixture";
    let executable_sha = format!("{:x}", Sha256::digest(executable));
    let target = tradeassembly_plugin_sdk::host_target();
    let executable_path = format!("bin/{target}/fixture-plugin");
    let descriptor = serde_json::to_vec_pretty(&json!({
        "packageContractVersion": "1",
        "plugin": {"id": "example.credential-data", "version": "0.1.0"},
        "manifest": {"path": "manifest.yaml", "sha256": manifest_file_sha},
        "targets": [{
            "target": target,
            "binary": {"path": executable_path, "sha256": executable_sha}
        }],
        "responseSchemas": [],
        "compatibility": {
            "hostContract": {
                "minimum": tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION,
                "maximum": tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION
            },
            "sdkVersion": tradeassembly_plugin_sdk::SDK_VERSION
        }
    }))
    .expect("serialize package descriptor");
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = Builder::new(encoder);
    append_package_file(
        &mut archive,
        "tradeassembly-plugin.json",
        &descriptor,
        0o644,
    );
    append_package_file(&mut archive, "manifest.yaml", &manifest_bytes, 0o644);
    append_package_file(&mut archive, &executable_path, executable, 0o755);
    archive.finish().expect("finish package archive");
    let package_bytes = archive
        .into_inner()
        .expect("package encoder")
        .finish()
        .expect("compress package archive");
    let package_sha = format!("{:x}", Sha256::digest(&package_bytes));
    let package_path = directory.join("credential-data-package.tar.gz");
    fs::File::create(&package_path)
        .expect("create package fixture")
        .write_all(&package_bytes)
        .expect("write package fixture");
    let fixture = PluginPackageFixture {
        package_sha256: package_sha,
        manifest_sha256: manifest_sha,
        locator: package_path.to_string_lossy().into_owned(),
    };
    let installed = service.handle_http("POST", "/plugins/packages", fixture.install_body());
    assert_eq!(installed.status, 200, "{:#}", installed.body);
    let created = service.handle_http(
        "POST",
        "/plugins/instances",
        json!({
            "pluginRef": "example.credential-data",
            "instanceRef": "credential-data",
            "providerRef": "credential-data",
            "accountRef": "account://credential-data/paper",
            "enabled": true
        }),
    );
    assert_eq!(created.status, 200, "{:#}", created.body);
    fixture
}

#[test]
fn published_strategy_graph_preserves_typed_requirements_and_transport_parity() {
    let (_database, _db, service) = test_service("capability-graph-normalization");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let body = graph_body(&strategy_id, &version_id, &spec_hash);

    let http = service.handle_http("POST", "/capabilities/graph/resolve", body.clone());
    assert_eq!(http.status, 200, "{:#}", http.body);
    let graph = response_body(&http);
    assert_eq!(graph["ok"], true, "{graph:#}");
    let ids = graph["nodes"]
        .as_array()
        .expect("graph nodes")
        .iter()
        .map(|node| node["nodeId"].as_str().expect("node id"))
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["backtest_dataset", "req_calendar", "req_logic"]);
    assert!(!ids.contains(&"req_bars"), "live source requirement leaked");
    assert!(!ids.contains(&"req_order"), "paper/live requirement leaked");

    let dataset = &node(&graph, "backtest_dataset")["requirement"];
    assert_eq!(dataset["policyTags"], json!(["immutable_dataset"]));
    assert_eq!(dataset["purpose"], "Read immutable backtest dataset bars");
    assert_eq!(dataset["stageRefs"], json!(["inputs"]));

    let graphql = service.execute_graphql(json!({
        "operationName": "ResolveCapabilityGraph",
        "query": "query ResolveCapabilityGraph { capabilityGraphResolution }",
        "variables": body
    }));
    assert_eq!(
        graphql["data"]["capabilityGraphResolution"], graph,
        "{graphql:#}"
    );

    let mcp = service.call_mcp_tool(
        "tradeassembly.plugin.capability_graph_resolve",
        graph_body(&strategy_id, &version_id, &spec_hash),
    );
    assert_eq!(mcp["structuredContent"], graph, "{mcp:#}");
}

#[test]
fn immutable_revision_survives_restart_and_rejects_idempotency_conflict() {
    let (_database, db, service) = test_service("capability-graph-revision");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let mut body = graph_body(&strategy_id, &version_id, &spec_hash);
    body["authority"] = json!({
        "actor": "untrusted-authority",
        "api_secret": "AUTHORITY_SECRET_MUST_NOT_LEAK",
        "nested": {"token": "AUTHORITY_TOKEN_MUST_NOT_LEAK"}
    });
    body["authorityContext"] = json!({
        "actor": "user.local",
        "surface": "capability-revision-test",
        "accountMode": "backtest",
        "apiSecret": "CONTEXT_SECRET_MUST_NOT_LEAK",
        "nested": {"privateKey": "CONTEXT_KEY_MUST_NOT_LEAK"}
    });

    let saved = service.handle_http("POST", "/capability-graph-revisions", body.clone());
    assert_eq!(saved.status, 200, "{:#}", saved.body);
    let first = response_body(&saved);
    assert_eq!(first["ok"], true, "{first:#}");
    assert_eq!(first["duplicate"], false);
    assert_eq!(first["revision"]["immutable"], true);
    assert_eq!(first["revision"]["graph"]["ok"], true);
    assert_eq!(
        first["revision"]["authority"],
        json!({
            "actor": "user.local",
            "surface": "capability-revision-test",
            "accountMode": "backtest"
        })
    );
    let revision_id = first["revision"]["revisionId"]
        .as_str()
        .expect("revision id")
        .to_string();

    let duplicate =
        response_body(&service.handle_http("POST", "/capability-graph-revisions", body.clone()));
    assert_eq!(duplicate["ok"], true);
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(duplicate["revision"]["revisionId"], revision_id);
    assert_eq!(duplicate["revision"]["graph"], first["revision"]["graph"]);

    let mut changed = body;
    changed["mode"] = json!("research");
    let conflict = service.handle_http("POST", "/capability-graph-revisions", changed);
    assert_eq!(conflict.status, 409, "{:#}", conflict.body);
    assert_eq!(conflict.body["error"]["code"], "idempotency_conflict");

    drop(service);
    let reopened = configured_test_service(&db);
    let read = response_body(&reopened.handle_http(
        "GET",
        &format!("/capability-graph-revisions/{revision_id}"),
        json!({}),
    ));
    assert_eq!(read["ok"], true, "{read:#}");
    assert_eq!(read["revision"], first["revision"]);

    let current = response_body(&reopened.handle_http(
        "POST",
        "/capability-graph-revisions/currentness",
        json!({"revisionId": revision_id, "evaluationEpoch": EPOCH}),
    ));
    assert_eq!(current["current"], true, "{current:#}");

    let graphql = reopened.execute_graphql(json!({
        "operationName": "CapabilityGraphRevision",
        "query": "query CapabilityGraphRevision { capabilityGraphRevision }",
        "variables": {"revisionId": revision_id}
    }));
    assert_eq!(
        graphql["data"]["capabilityGraphRevision"], read,
        "{graphql:#}"
    );

    let mcp = reopened.call_mcp_tool(
        "tradeassembly.plugin.capability_revision_get",
        json!({"revision_id": revision_id}),
    );
    assert_eq!(mcp["structuredContent"], read, "{mcp:#}");

    let serialized = serde_json::to_string(&read).expect("serialized revision");
    assert!(!serialized.contains("api_key"));
    assert!(!serialized.contains("apiSecret"));
    assert!(!serialized.contains("secret-value"));
    assert!(!serialized.contains("AUTHORITY_SECRET_MUST_NOT_LEAK"));
    assert!(!serialized.contains("AUTHORITY_TOKEN_MUST_NOT_LEAK"));
    assert!(!serialized.contains("CONTEXT_SECRET_MUST_NOT_LEAK"));
    assert!(!serialized.contains("CONTEXT_KEY_MUST_NOT_LEAK"));
    assert!(!serialized.contains("untrusted-authority"));
}

#[test]
fn demo_reset_clears_capability_and_execution_replay_state() {
    let (_database, _db, service) = test_service("capability-graph-demo-reset");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let body = graph_body(&strategy_id, &version_id, &spec_hash);
    let revision = save_graph(&service, body.clone());
    let revision_id = revision["revisionId"]
        .as_str()
        .expect("revision id")
        .to_string();
    let saved_config = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": strategy_id,
            "mode": "paper",
            "providerRef": "sim"
        }),
    );
    assert_eq!(saved_config.status, 200, "{:#}", saved_config.body);
    assert!(!service
        .runtime()
        .storage
        .list_json("execution_configs")
        .expect("stored execution configs")
        .is_empty());
    let plugin_context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("demo-reset-plugin-operation").expect("valid idempotency key"),
    );
    for namespace in ["plugin_operation_requests", "plugin_operation_receipts"] {
        service
            .runtime()
            .storage
            .put_json_if_absent(
                namespace,
                "demo-reset-plugin-operation",
                json!({"record": namespace}),
                &plugin_context,
            )
            .expect("stored plugin operation replay record");
    }
    let mut disabled = service
        .handle_http("GET", "/plugins/instances/sim", json!({}))
        .body;
    disabled["enabled"] = json!(false);
    disabled["health"]["state"] = json!("disabled");
    service
        .runtime()
        .storage
        .put_json("plugin_instances_v2", "sim", disabled, &plugin_context)
        .expect("stored mutated plugin instance");
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json("plugin_instances_v2", "sim")
            .expect("stored plugin instance")
            .expect("mutated plugin instance")["enabled"],
        false
    );

    let reset = service.handle_http("POST", "/demo/reset-btc", json!({}));
    assert_eq!(reset.status, 200, "{:#}", reset.body);
    assert_eq!(reset.body["resetApplied"], true);
    let namespaces = reset.body["namespaces"]
        .as_array()
        .expect("reset namespaces");
    assert!(namespaces
        .iter()
        .any(|namespace| namespace == "capability_graph_revisions"));
    assert!(namespaces
        .iter()
        .any(|namespace| namespace == "capability_graph_idempotency"));
    assert!(namespaces
        .iter()
        .any(|namespace| namespace == "execution_configs"));
    for namespace in ["plugin_operation_requests", "plugin_operation_receipts"] {
        assert!(namespaces.iter().any(|candidate| candidate == namespace));
    }
    assert!(namespaces
        .iter()
        .any(|namespace| namespace == "plugin_instances_v2"));

    let missing = response_body(&service.handle_http(
        "GET",
        &format!("/capability-graph-revisions/{revision_id}"),
        json!({}),
    ));
    assert_eq!(missing["ok"], false, "{missing:#}");
    assert_eq!(
        missing["blockers"][0]["code"],
        "capability_revision_not_found"
    );

    let recreated =
        response_body(&service.handle_http("POST", "/capability-graph-revisions", body));
    assert_eq!(recreated["ok"], true, "{recreated:#}");
    assert_eq!(recreated["duplicate"], false, "{recreated:#}");

    assert!(service
        .runtime()
        .storage
        .list_json("execution_configs")
        .expect("stored execution configs")
        .is_empty());
    for namespace in ["plugin_operation_requests", "plugin_operation_receipts"] {
        assert!(service
            .runtime()
            .storage
            .list_json(namespace)
            .expect("stored plugin operation replay records")
            .is_empty());
    }
    assert!(service
        .runtime()
        .storage
        .list_json("plugin_instances_v2")
        .expect("stored plugin instances")
        .is_empty());
    let restored = service.handle_http("GET", "/plugins/instances/sim", json!({}));
    assert_eq!(restored.status, 200, "{:#}", restored.body);
    assert_eq!(restored.body["enabled"], true);
}

#[test]
fn currentness_fails_closed_for_manifest_health_entitlement_and_strategy_changes() {
    let (_database, _db, service) = test_service("capability-graph-staleness");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);

    let manifest_revision = save_graph(&service, graph_body(&strategy_id, &version_id, &spec_hash));
    let installed = service.handle_http(
        "GET",
        "/plugins/manifests/tradeassembly.local-data",
        json!({}),
    );
    assert_eq!(installed.status, 200, "{:#}", installed.body);
    let mut replacement = installed.body["manifest"].clone();
    replacement["metadata"]["version"] = json!("0.1.1");
    let digest = manifest_digest(&replacement);
    let install = service.handle_http(
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": replacement,
            "integrity": {"manifestSha256": digest},
            "source": {
                "type": "git",
                "commit": "1111111111111111111111111111111111111111",
                "locator": "git+https://example.invalid/local-data?ref=1111111111111111111111111111111111111111#plugin"
            }
        }),
    );
    assert_eq!(install.status, 200, "{:#}", install.body);
    let stale_manifest = currentness(
        &service,
        manifest_revision["revisionId"]
            .as_str()
            .expect("revision id"),
    );
    assert_eq!(stale_manifest["current"], false, "{stale_manifest:#}");
    assert_eq!(stale_manifest["blockers"][0]["code"], "resolution_stale");

    let (_database, _db, service) = test_service("capability-graph-disabled");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let disabled_revision = save_graph(&service, graph_body(&strategy_id, &version_id, &spec_hash));
    let disabled = service.handle_http("POST", "/plugins/instances/local-data:disable", json!({}));
    assert_eq!(disabled.status, 200, "{:#}", disabled.body);
    let stale_health = currentness(
        &service,
        disabled_revision["revisionId"]
            .as_str()
            .expect("revision id"),
    );
    assert_eq!(stale_health["current"], false, "{stale_health:#}");

    let (_database, _db, service) = test_service("capability-graph-entitlement");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let entitlement_revision =
        save_graph(&service, graph_body(&strategy_id, &version_id, &spec_hash));
    let denied = service.handle_http(
        "POST",
        "/entitlements/deny",
        json!({"capability": "market_data.bars.read@1", "reason": "stale test"}),
    );
    assert_eq!(denied.status, 200, "{:#}", denied.body);
    let stale_entitlement = currentness(
        &service,
        entitlement_revision["revisionId"]
            .as_str()
            .expect("revision id"),
    );
    assert_eq!(stale_entitlement["current"], false, "{stale_entitlement:#}");

    let (_database, _db, service) = test_service("capability-graph-strategy-hash");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let source_revision = save_graph(&service, graph_body(&strategy_id, &version_id, &spec_hash));
    let forged = json!({
        "configId": "cfg_forged_strategy_hash",
        "strategyId": strategy_id,
        "strategyVersionId": version_id,
        "strategySpecHash": "sha256:forged",
        "mode": "backtest",
        "evaluationEpoch": EPOCH,
        "idempotencyKey": "capability-graph-forged-hash",
        "graphRequest": source_revision["graphRequest"]
    });
    let forged_revision = save_graph(&service, forged);
    let stale_strategy = currentness(
        &service,
        forged_revision["revisionId"].as_str().expect("revision id"),
    );
    assert_eq!(stale_strategy["current"], false, "{stale_strategy:#}");
    assert!(stale_strategy["blockers"]
        .as_array()
        .expect("blockers")
        .iter()
        .any(|blocker| blocker["code"] == "strategy_version_mismatch"));
}

#[test]
fn credential_revocation_invalidates_a_bound_revision_without_secret_output() {
    let (database, _db, service) = test_service("capability-graph-credential");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let _package = install_credential_data_plugin(&service, database.path());
    let stored = service.handle_http(
        "POST",
        "/plugins/instances/credential-data/credentials",
        json!({"credentials": {"api_key": "KEY_MUST_NOT_LEAK", "api_secret": "SECRET_MUST_NOT_LEAK"}}),
    );
    assert_eq!(stored.status, 200, "{:#}", stored.body);
    let health = service.handle_http(
        "POST",
        "/plugins/instances/credential-data/health:refresh",
        json!({}),
    );
    assert_eq!(health.status, 200, "{:#}", health.body);
    assert_eq!(health.body["instance"]["health"]["state"], "ready");

    let body = json!({
        "configId": "cfg_credential_data",
        "strategyId": strategy_id,
        "strategyVersionId": version_id,
        "strategySpecHash": spec_hash,
        "mode": "paper",
        "evaluationEpoch": EPOCH,
        "idempotencyKey": "capability-graph-credential-data",
        "graphRequest": {
            "requirements": [{
                "requirementId": "credential.data.bars",
                "capability": "market_data.bars.read@1",
                "mode": "paper",
                "declaration": "required",
                "requiredFor": ["paper"],
                "constraints": {
                    "instrumentFamilies": ["equity"],
                    "dataShapes": ["bars"],
                    "operations": ["read"],
                    "inputPaths": ["market_data.source_bars"],
                    "deterministic": false,
                    "replayable": false
                },
                "fallbackPolicy": "none",
                "accountRef": "account://credential-data/paper",
                "purpose": "paper_market_observation"
            }],
            "bindings": {
                "credential.data.bars": {
                    "pluginInstanceRef": "credential-data",
                    "pluginRef": "example.credential-data",
                    "operationId": "marketdata.bars.read_v1",
                    "accountRef": "account://credential-data/paper"
                }
            },
            "configuredFallbacks": {},
            "evaluationEpoch": EPOCH
        }
    });
    let revision = save_graph(&service, body.clone());
    assert_eq!(revision["graph"]["ok"], true, "{revision:#}");

    let replaced = service.handle_http(
        "POST",
        "/plugins/instances/credential-data/credentials",
        json!({"credentials": {
            "api_key": "REPLACEMENT_KEY_MUST_NOT_LEAK",
            "api_secret": "REPLACEMENT_SECRET_MUST_NOT_LEAK"
        }}),
    );
    assert_eq!(replaced.status, 200, "{:#}", replaced.body);
    let stale_replacement = currentness(
        &service,
        revision["revisionId"].as_str().expect("revision id"),
    );
    assert_eq!(stale_replacement["current"], false, "{stale_replacement:#}");
    assert!(stale_replacement["blockers"]
        .as_array()
        .expect("blockers")
        .iter()
        .any(|blocker| blocker["code"] == "resolution_stale"));
    let replacement_serialized = stale_replacement.to_string();
    assert!(!replacement_serialized.contains("REPLACEMENT_KEY_MUST_NOT_LEAK"));
    assert!(!replacement_serialized.contains("REPLACEMENT_SECRET_MUST_NOT_LEAK"));

    let mut replacement_body = body;
    replacement_body["configId"] = json!("cfg_credential_data_after_replacement");
    replacement_body["idempotencyKey"] =
        json!("capability-graph-credential-data-after-replacement");
    let replacement_revision = save_graph(&service, replacement_body);
    assert_eq!(replacement_revision["graph"]["ok"], true);

    let revoked = service.handle_http(
        "DELETE",
        "/plugins/instances/credential-data/credentials",
        json!({}),
    );
    assert_eq!(revoked.status, 200, "{:#}", revoked.body);
    let stale = currentness(
        &service,
        revision["revisionId"].as_str().expect("revision id"),
    );
    assert_eq!(stale["current"], false, "{stale:#}");
    let serialized = stale.to_string();
    assert!(!serialized.contains("KEY_MUST_NOT_LEAK"));
    assert!(!serialized.contains("SECRET_MUST_NOT_LEAK"));
    assert!(!serialized.contains("REPLACEMENT_KEY_MUST_NOT_LEAK"));
    assert!(!serialized.contains("REPLACEMENT_SECRET_MUST_NOT_LEAK"));
}

#[test]
fn external_plugin_reconnect_restores_exact_research_and_paper_readiness() {
    let (database, db, service) = test_service("capability-graph-plugin-reconnect");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let package = install_credential_data_plugin(&service, database.path());
    let api_key = ["RECOVERY", "KEY", "MUST", "NOT", "LEAK"].join("_");
    let api_secret = ["RECOVERY", "SECRET", "MUST", "NOT", "LEAK"].join("_");
    let stored = service.handle_http(
        "POST",
        "/plugins/instances/credential-data/credentials",
        json!({"credentials": {"api_key": api_key, "api_secret": api_secret}}),
    );
    assert_eq!(stored.status, 200, "{:#}", stored.body);
    let credential_revision = stored.body["credentialStatus"]["revisionRef"].clone();
    let health = service.handle_http(
        "POST",
        "/plugins/instances/credential-data/health:refresh",
        json!({}),
    );
    assert_eq!(health.status, 200, "{:#}", health.body);
    assert_eq!(health.body["instance"]["health"]["state"], "ready");

    let research_revision = save_graph(
        &service,
        json!({
            "configId": "cfg_reconnect_research",
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "strategySpecHash": spec_hash,
            "mode": "research",
            "evaluationEpoch": EPOCH,
            "idempotencyKey": "capability-graph-reconnect-research",
            "graphRequest": {
                "requirements": [{
                    "requirementId": "reconnect.data.bars",
                    "capability": "market_data.bars.read@1",
                    "mode": "research",
                    "declaration": "required",
                    "requiredFor": ["research"],
                    "constraints": {
                        "instrumentFamilies": ["equity"],
                        "dataShapes": ["bars"],
                        "operations": ["read"],
                        "inputPaths": ["market_data.source_bars"],
                        "deterministic": false,
                        "replayable": false
                    },
                    "fallbackPolicy": "none",
                    "accountRef": "account://credential-data/research",
                    "purpose": "research_market_observation"
                }],
                "bindings": {
                    "reconnect.data.bars": {
                        "pluginInstanceRef": "credential-data",
                        "pluginRef": "example.credential-data",
                        "operationId": "marketdata.bars.read_v1",
                        "accountRef": "account://credential-data/research"
                    }
                },
                "configuredFallbacks": {},
                "evaluationEpoch": EPOCH
            }
        }),
    );
    assert_eq!(
        research_revision["graph"]["ok"], true,
        "{research_revision:#}"
    );

    let configured = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "configId": "cfg_reconnect_paper",
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "mode": "paper",
            "providerRef": "credential-data",
            "accountRef": "account://credential-data/paper",
            "dataProviderRef": "local-data",
            "evaluationEpoch": EPOCH
        }),
    );
    assert_eq!(configured.status, 200, "{:#}", configured.body);
    let config = configured.body["body"]["item"].clone();
    assert_eq!(config["status"], "configured", "{config:#}");
    assert_eq!(config["providerRef"], "credential-data");
    assert_eq!(config["dataProviderRef"], "local-data");
    let paper_revision_id = config["capabilityGraphRevisionId"]
        .as_str()
        .expect("paper capability revision")
        .to_string();
    let paper_revision = response_body(&service.handle_http(
        "GET",
        &format!("/capability-graph-revisions/{paper_revision_id}"),
        json!({}),
    ))["revision"]
        .clone();
    let nodes = paper_revision["graph"]["nodes"]
        .as_array()
        .expect("paper capability graph nodes");
    assert!(nodes.iter().any(|node| {
        node["nodeId"] == "execution.broker.submit"
            && node["selected"]["pluginInstanceRef"] == "credential-data"
    }));
    assert!(nodes.iter().any(|node| {
        node["nodeId"] == "req_bars" && node["selected"]["pluginInstanceRef"] == "local-data"
    }));
    let research_revision_id = research_revision["revisionId"]
        .as_str()
        .expect("research capability revision")
        .to_string();
    let research_fingerprint = research_revision["graphFingerprint"].clone();
    let paper_fingerprint = config["capabilityGraphFingerprint"].clone();

    drop(service);
    let restarted = configured_test_service(&db);
    let instance = restarted.handle_http("GET", "/plugins/instances/credential-data", json!({}));
    assert_eq!(instance.status, 200, "{:#}", instance.body);
    assert_eq!(instance.body["configuration"], json!({}));
    assert_eq!(instance.body["credentialStatus"]["configured"], true);
    assert_eq!(
        instance.body["credentialStatus"]["revisionRef"],
        credential_revision
    );
    assert_eq!(instance.body["health"]["state"], "ready");
    assert_eq!(
        currentness(&restarted, &research_revision_id)["current"],
        true
    );
    assert_eq!(currentness(&restarted, &paper_revision_id)["current"], true);

    assert_eq!(
        restarted
            .runtime()
            .plugin_packages
            .remove(&package.package_sha256),
        Ok(true)
    );
    let unavailable = restarted.handle_http("GET", "/plugins/instances/credential-data", json!({}));
    assert_eq!(unavailable.status, 200, "{:#}", unavailable.body);
    assert_eq!(unavailable.body["health"]["state"], "host_unavailable");
    assert_eq!(unavailable.body["credentialStatus"]["configured"], true);
    assert_eq!(
        unavailable.body["credentialStatus"]["revisionRef"],
        credential_revision
    );
    for revision_id in [&research_revision_id, &paper_revision_id] {
        let stale = currentness(&restarted, revision_id);
        assert_eq!(stale["current"], false, "{stale:#}");
        assert!(stale["blockers"]
            .as_array()
            .expect("stale blockers")
            .iter()
            .any(|blocker| blocker["code"] == "resolution_stale"));
    }
    let blocked = restarted.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "activationId": "activation-package-unavailable",
            "configId": config["configId"],
            "strategyId": strategy_id,
            "idempotencyKey": "activation-package-unavailable",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(blocked.status, 400, "{:#}", blocked.body);
    assert!(blocked.body["error"]["details"]["blockedReasons"]
        .as_array()
        .expect("blocked reasons")
        .iter()
        .any(|reason| reason == "resolution_stale"));

    let mut reconnect_install = package.install_body();
    reconnect_install["idempotencyKey"] = json!("plugin-package-reconnect");
    let reinstalled = restarted.handle_http("POST", "/plugins/packages", reconnect_install);
    assert_eq!(reinstalled.status, 200, "{:#}", reinstalled.body);
    assert_eq!(
        reinstalled.body["idempotent"], true,
        "{:#}",
        reinstalled.body
    );
    let restored_health = restarted.handle_http(
        "POST",
        "/plugins/instances/credential-data/health:refresh",
        json!({}),
    );
    assert_eq!(restored_health.status, 200, "{:#}", restored_health.body);
    assert_eq!(restored_health.body["instance"]["health"]["state"], "ready");
    assert_eq!(
        restored_health.body["instance"]["credentialStatus"]["revisionRef"],
        credential_revision
    );

    let restored_research = currentness(&restarted, &research_revision_id);
    let restored_paper = currentness(&restarted, &paper_revision_id);
    assert_eq!(restored_research["current"], true, "{restored_research:#}");
    assert_eq!(restored_paper["current"], true, "{restored_paper:#}");
    assert_eq!(
        restored_research["currentGraphFingerprint"],
        research_fingerprint
    );
    assert_eq!(restored_paper["currentGraphFingerprint"], paper_fingerprint);

    let activated = restarted.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "activationId": "activation-package-restored",
            "configId": config["configId"],
            "strategyId": strategy_id,
            "idempotencyKey": "activation-package-restored",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activated.status, 200, "{:#}", activated.body);
    assert_eq!(activated.body["body"]["status"], "active");

    let public_evidence = json!({
        "stored": stored.body,
        "health": health.body,
        "instance": instance.body,
        "unavailable": unavailable.body,
        "blocked": blocked.body,
        "reinstalled": reinstalled.body,
        "restoredHealth": restored_health.body,
        "restoredResearch": restored_research,
        "restoredPaper": restored_paper,
        "activated": activated.body,
    })
    .to_string();
    assert!(!public_evidence.contains(&api_key));
    assert!(!public_evidence.contains(&api_secret));
}

#[test]
fn execution_rejects_wrong_context_revision_and_copies_valid_snapshot_into_run() {
    let (_database, _db, service) = test_service("capability-graph-execution");
    let (strategy_id, version_id, spec_hash) = publish_static_strategy(&service);
    let backtest_revision = save_graph(&service, graph_body(&strategy_id, &version_id, &spec_hash));
    let wrong_mode = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "mode": "paper",
            "providerRef": "sim",
            "capabilityGraphRevisionId": backtest_revision["revisionId"]
        }),
    );
    assert_eq!(wrong_mode.status, 200, "{:#}", wrong_mode.body);
    assert_eq!(wrong_mode.body["body"]["item"]["status"], "blocked");
    assert_eq!(
        wrong_mode.body["body"]["item"]["capabilityGraphError"]["blockers"][0]["code"],
        "execution_mode_mismatch"
    );

    let configured = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "configId": "cfg_valid_execution_consumer",
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "mode": "paper",
            "providerRef": "sim"
        }),
    );
    assert_eq!(configured.status, 200, "{:#}", configured.body);
    let item = &configured.body["body"]["item"];
    assert_eq!(item["status"], "configured", "{item:#}");

    let source_revision = response_body(&service.handle_http(
        "GET",
        &format!(
            "/capability-graph-revisions/{}",
            item["capabilityGraphRevisionId"]
                .as_str()
                .expect("capability graph revision id")
        ),
        json!({}),
    ))["revision"]
        .clone();
    let mut incomplete_request = source_revision["graphRequest"].clone();
    incomplete_request["requirements"] = json!(source_revision["graphRequest"]["requirements"]
        .as_array()
        .expect("requirements")
        .iter()
        .filter(|requirement| requirement["requirementId"] == "execution.broker.submit")
        .cloned()
        .collect::<Vec<_>>());
    let incomplete_revision = save_graph(
        &service,
        json!({
            "configId": "cfg_incomplete_execution_graph",
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "strategySpecHash": spec_hash,
            "mode": "paper",
            "evaluationEpoch": EPOCH,
            "idempotencyKey": "capability-graph-incomplete-execution",
            "graphRequest": incomplete_request
        }),
    );
    assert_eq!(incomplete_revision["graph"]["ok"], true);
    let incomplete_config = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "configId": "cfg_incomplete_execution_consumer",
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "mode": "paper",
            "providerRef": "sim",
            "capabilityGraphRevisionId": incomplete_revision["revisionId"]
        }),
    );
    assert_eq!(
        incomplete_config.status, 200,
        "{:#}",
        incomplete_config.body
    );
    assert_eq!(
        incomplete_config.body["body"]["item"]["status"], "blocked",
        "{:#}",
        incomplete_config.body
    );
    assert_eq!(
        incomplete_config.body["body"]["item"]["capabilityGraphError"]["blockers"][0]["code"],
        "capability_revision_mismatch"
    );

    let activated = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "configId": item["configId"],
            "strategyId": strategy_id,
            "idempotencyKey": "capability-graph-execution-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(activated.status, 200, "{:#}", activated.body);
    assert_eq!(activated.body["body"]["status"], "active");
    let run = &activated.body["body"]["run"];
    assert_eq!(
        run["immutableInput"]["capabilityGraphRevisionId"],
        item["capabilityGraphRevisionId"]
    );
    assert_eq!(
        run["immutableInput"]["capabilityGraphFingerprint"],
        item["capabilityGraphFingerprint"]
    );
    assert_eq!(run["immutableInput"]["capabilityGraphSnapshot"]["ok"], true);

    let wrong_strategy = service.handle_http(
        "POST",
        "/product/strategy-execution-configs/save",
        json!({
            "strategyId": "strat_local_btc_demo",
            "mode": "paper",
            "providerRef": "sim",
            "capabilityGraphRevisionId": item["capabilityGraphRevisionId"]
        }),
    );
    assert_eq!(wrong_strategy.status, 200, "{:#}", wrong_strategy.body);
    assert_eq!(wrong_strategy.body["body"]["item"]["status"], "blocked");
    assert_eq!(
        wrong_strategy.body["body"]["item"]["capabilityGraphError"]["blockers"][0]["code"],
        "capability_revision_mismatch"
    );

    let stopped = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/control",
        json!({
            "activationId": activated.body["body"]["activationId"],
            "action": "stop"
        }),
    );
    assert_eq!(stopped.status, 200, "{:#}", stopped.body);
    assert_eq!(stopped.body["body"]["state"], "stopped");

    let disabled = service.handle_http("POST", "/plugins/instances/sim:disable", json!({}));
    assert_eq!(disabled.status, 200, "{:#}", disabled.body);
    let stale_activation = service.handle_http(
        "POST",
        "/product/strategy-execution-activations/activate",
        json!({
            "activationId": "activation-stale-capability-graph",
            "configId": item["configId"],
            "strategyId": strategy_id,
            "idempotencyKey": "capability-graph-stale-activation",
            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"]
        }),
    );
    assert_eq!(stale_activation.status, 400, "{:#}", stale_activation.body);
    assert!(stale_activation.body["error"]["details"]["blockedReasons"]
        .as_array()
        .expect("blocked reasons")
        .iter()
        .any(|reason| reason == "resolution_stale"));
}

#[test]
fn backtest_requires_the_supplied_revision_and_rejects_it_after_binding_stales() {
    let (_database, _db, service) = test_service("capability-graph-backtest-consumer");
    let (strategy_id, version_id, _) = publish_static_strategy(&service);
    let dataset = service.handle_http(
        "POST",
        "/dataset-ingestions",
        json!({
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "pluginInstanceRef": "local-data",
            "operationId": "marketdata.bars.read_v1",
            "assetClass": "equity",
            "instruments": ["SPY"],
            "dataKind": "bars",
            "granularity": "1m",
            "timeSlice": {
                "start": "2026-01-02T00:00:00Z",
                "end": "2026-01-02T00:02:00Z"
            },
            "calendar": "XNYS",
            "timezone": "UTC",
            "normalizationPolicy": {
                "timestampUnit": "rfc3339",
                "timezone": "UTC",
                "duplicatePolicy": "reject",
                "priceAdjustment": "raw"
            },
            "qualityPolicy": {
                "missingIntervals": "allow",
                "staleObservations": "allow",
                "outliers": "allow",
                "invalidMarkets": "allow",
                "calendarMismatch": "allow",
                "corporateActionGaps": "allow",
                "missingDerivativeFields": "allow"
            },
            "maxRows": 100,
            "idempotencyKey": "capability-graph-backtest-dataset",
            "authorityContext": {
                "actor": "user.local",
                "surface": "capability-graph-backtest-test"
            }
        }),
    );
    assert_eq!(dataset.status, 201, "{:#}", dataset.body);
    assert_eq!(dataset.body["status"], "completed", "{:#}", dataset.body);
    let dataset_id = dataset.body["snapshot"]["datasetId"]
        .as_str()
        .expect("dataset id")
        .to_string();

    let missing = service.handle_http(
        "POST",
        "/product/strategies/backtests/run",
        json!({
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "datasetId": dataset_id,
            "capabilityGraphRevisionId": "caprev_missing",
            "idempotencyKey": "backtest-missing-capability-revision"
        }),
    );
    assert_eq!(missing.status, 400, "{:#}", missing.body);
    assert_eq!(
        missing.body["error"]["code"],
        "backtest_capability_revision_stale"
    );

    let queued = service.handle_http(
        "POST",
        "/product/strategies/backtests/run",
        json!({
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "datasetId": dataset_id,
            "idempotencyKey": "backtest-capability-revision-source"
        }),
    );
    assert_eq!(queued.status, 202, "{:#}", queued.body);
    assert_eq!(queued.body["status"], "queued", "{:#}", queued.body);
    let run_id = queued.body["runId"].as_str().expect("run id");
    let detailed = service.handle_http("GET", &format!("/backtests/{run_id}"), json!({}));
    assert_eq!(detailed.status, 200, "{:#}", detailed.body);
    let revision_id = detailed.body["manifest"]["content"]["configuration"]["capabilityGraph"]
        ["revisionId"]
        .as_str()
        .expect("capability revision id")
        .to_string();
    assert_eq!(
        detailed.body["manifest"]["content"]["configuration"]["capabilityGraph"]
            ["graphFingerprint"],
        response_body(&service.handle_http(
            "GET",
            &format!("/capability-graph-revisions/{revision_id}"),
            json!({}),
        ))["revision"]["graphFingerprint"]
    );

    let source_revision = response_body(&service.handle_http(
        "GET",
        &format!("/capability-graph-revisions/{revision_id}"),
        json!({}),
    ))["revision"]
        .clone();
    let mut incomplete_request = source_revision["graphRequest"].clone();
    incomplete_request["requirements"] = json!(source_revision["graphRequest"]["requirements"]
        .as_array()
        .expect("requirements")
        .iter()
        .filter(|requirement| requirement["requirementId"]
            .as_str()
            .is_some_and(|id| id.starts_with("backtest.dataset:")))
        .cloned()
        .collect::<Vec<_>>());
    let incomplete_revision = save_graph(
        &service,
        json!({
            "configId": "cfg_incomplete_backtest_graph",
            "strategyId": source_revision["strategyId"],
            "strategyVersionId": source_revision["strategyVersionId"],
            "strategySpecHash": source_revision["strategySpecHash"],
            "mode": "backtest",
            "evaluationEpoch": EPOCH,
            "idempotencyKey": "capability-graph-incomplete-backtest",
            "graphRequest": incomplete_request
        }),
    );
    assert_eq!(incomplete_revision["graph"]["ok"], true);
    let incomplete = service.handle_http(
        "POST",
        "/product/strategies/backtests/run",
        json!({
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "datasetId": dataset_id,
            "capabilityGraphRevisionId": incomplete_revision["revisionId"],
            "idempotencyKey": "backtest-incomplete-capability-revision"
        }),
    );
    assert_eq!(incomplete.status, 400, "{:#}", incomplete.body);
    assert_eq!(
        incomplete.body["error"]["code"],
        "backtest_capability_revision_stale"
    );

    let source_disabled =
        service.handle_http("POST", "/plugins/instances/local-data:disable", json!({}));
    assert_eq!(source_disabled.status, 200, "{:#}", source_disabled.body);
    let offline_replay = service.handle_http(
        "POST",
        "/product/strategies/backtests/run",
        json!({
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "datasetId": dataset_id,
            "capabilityGraphRevisionId": revision_id,
            "idempotencyKey": "backtest-offline-capability-revision"
        }),
    );
    assert_eq!(offline_replay.status, 202, "{:#}", offline_replay.body);

    let runtime_disabled =
        service.handle_http("POST", "/plugins/instances/core-runtime:disable", json!({}));
    assert_eq!(runtime_disabled.status, 200, "{:#}", runtime_disabled.body);
    let stale = service.handle_http(
        "POST",
        "/product/strategies/backtests/run",
        json!({
            "strategyId": strategy_id,
            "strategyVersionId": version_id,
            "datasetId": dataset_id,
            "capabilityGraphRevisionId": revision_id,
            "idempotencyKey": "backtest-stale-capability-revision"
        }),
    );
    assert_eq!(stale.status, 400, "{:#}", stale.body);
    assert_eq!(
        stale.body["error"]["code"],
        "backtest_capability_revision_stale"
    );
}
