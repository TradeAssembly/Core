// Real process/local HTTP fixture. NativeTestSandbox is NOT production OS-sandbox qualification.
use super::*;
use crate::{
    data_pipeline::*,
    ports::*,
    service::{data_captures as captures, data_evaluation, data_plans},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};
use tradeassembly_plugin_sdk::data_evaluation::*;
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
#[derive(Default)]
struct ForwardNative(Mutex<Option<crate::adapters::plugin_operations::LocalPluginOperations>>);
impl VersionedPort for ForwardNative {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginOperationPort for ForwardNative {
    fn invoke(
        &self,
        r: &PluginOperationRequest,
        c: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        self.0.lock().unwrap().as_ref().unwrap().invoke(r, c)
    }
}
fn ctx(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).unwrap(),
    )
}
fn install_fixture(
    service: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
    calculator: bool,
) -> InstalledPluginPackage {
    use crate::adapters::plugin_packages::data_package_tests::{archive_files, materials};
    let (mut descriptor, bytes, schema_bytes, _) = materials();
    let mut contract: Value = serde_json::from_slice(&bytes).unwrap();
    let id = if calculator {
        "capture.calculation"
    } else {
        "capture.source"
    };
    let opid = if calculator { "derive" } else { "replay" };
    contract["schemaVersion"] = json!("tradeassembly.operation_data.v2");
    contract["operationId"] = json!(opid);
    contract["applicability"] =
        json!({"assetClasses":["equity"],"instrumentFamilies":["equity"],"modes":["paper"]});
    if calculator {
        contract["inputs"] = json!({"source":{"data":contract["outputs"]["value"],"required":true,"dependencyId":"source"}});
    } else {
        contract["parameterSchema"] = json!({"type":"object","properties":{"snapshot":{"type":"object"},"digest":{"type":"string"}},"required":["snapshot","digest"],"additionalProperties":false});
    }
    let (_, _, fixture) = crate::data_pipeline::tests::fixture();
    let template_graph = crate::data_pipeline::tests::fixture().0;
    let template = template_graph
        .nodes
        .iter()
        .filter_map(|n| n.selected.as_ref())
        .find(|s| s.operation_id == if calculator { "derive" } else { "provide" })
        .unwrap();
    let mut operation = serde_json::to_value(&template.operation).unwrap();
    operation["id"] = json!(opid);
    operation["capability"] = json!(format!("test.capture.{opid}"));
    operation["protocol"] = json!("stdio");
    operation["traits"] = json!({"modes":["paper"],"instrumentFamilies":["equity"],"inputSchemaRefs":if calculator {vec!["schema://test/series@1"]} else {vec![]},"outputSchemaRefs":["schema://test/series@1",EVALUATION_RESPONSE_SCHEMA],"deterministic":true,"replayable":true});
    if calculator {
        operation["dependencies"] = json!([{ "dependencyId":"source","capability":"test.capture.replay","mode":"same","required":true,"fallbackPolicy":"none"}]);
    }
    let mut operations = vec![operation.clone()];
    if !calculator {
        operation["id"] = json!("acquire");
        operation["capability"] = json!("test.capture.acquire");
        operation["credentialGrantRequired"] = json!(true);
        operation["traits"]["outputSchemaRefs"] = json!(["schema://capture-fixture/response@1"]);
        operation["traits"]["deterministic"] = json!(false);
        operation["traits"]["replayable"] = json!(false);
        operations.push(operation);
    }
    let manifest = json!({"metadata":{"id":id,"version":"1","provider":{"id":id,"name":"Local capture fixture"}},"operations":operations,
        "capabilities":[{"id":"test.capture.acquire"},{"id":"test.capture.replay"},{"id":"test.capture.derive"}],"permissions":if calculator {vec![]}else{vec!["network.outbound"]},
        "configuration":{"fields":[{"id":"dataApiUrl","inputType":"url","storageClass":"configuration"}]},"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let contract_bytes = serde_json::to_vec(&contract).unwrap();
    let script = include_bytes!("fixtures/capture_plugin.py");
    let response = serde_json::to_vec(&json!({"type":"object"})).unwrap();
    descriptor["package"]["plugin"]["id"] = json!(id);
    descriptor["package"]["manifest"]["sha256"] = json!(sha256(&manifest_bytes));
    descriptor["package"]["targets"][0]["binary"]["sha256"] = json!(sha256(script));
    descriptor["package"]["responseSchemas"] = json!([{"schemaRef":EVALUATION_RESPONSE_SCHEMA,"document":{"path":"response.json","sha256":sha256(&response)}},{"schemaRef":"schema://capture-fixture/response@1","document":{"path":"response.json","sha256":sha256(&response)}}]);
    descriptor["dataContracts"][0]["operationId"] = json!(opid);
    descriptor["dataContracts"][0]["document"]["sha256"] = json!(sha256(&contract_bytes));
    descriptor["dataContracts"][0]["canonicalSha256"] = json!(canonical_sha256(&contract).unwrap());
    let archive = archive_files(vec![
        (
            "tradeassembly-plugin.json",
            serde_json::to_vec(&descriptor).unwrap(),
        ),
        ("manifest.json", manifest_bytes),
        ("contract.json", contract_bytes),
        ("schema.json", schema_bytes),
        ("response.json", response),
        ("bin/plugin", script.to_vec()),
    ]);
    let path = root.join(format!("{id}.tgz"));
    fs::write(&path, &archive).unwrap();
    let package = service
        .runtime()
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: sha256(&archive),
            manifest_sha256: canonical_sha256(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    service.runtime().plugins.install_manifest(id,json!({"pluginRef":id,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx("capture-setup")).unwrap();
    let _ = fixture;
    package
}
struct Rig {
    dir: tempfile::TempDir,
    service: crate::service::TradeAssemblyService,
    command: captures::CaptureCommand,
    operations: Arc<ForwardNative>,
    server: std::thread::JoinHandle<()>,
}
fn rig() -> Rig {
    let operations = Arc::new(ForwardNative::default());
    let (dir, service, scope, _) = data_plans::tests::fixture_service(Some(operations.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 8192];
        let n = stream.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /observations "));
        let body=serde_json::to_vec(&json!({"values":[10,20,30],"start":10*86400000_i64,"interval":86400000,"instrument":"TEST"})).unwrap();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
        stream.write_all(&body).unwrap();
    });
    for calculator in [false, true] {
        let package = install_fixture(&service, dir.path(), calculator);
        service.runtime().plugins.put_instance(&package.plugin_ref,json!({"instanceRef":package.plugin_ref,"pluginRef":package.plugin_ref,"providerRef":package.plugin_ref,"enabled":true,"activePackageSha256":package.package_sha256,"credentialRef":if calculator {"fixture-unused"}else{"capture-fixture"},"configuration":if calculator {json!({})} else {json!({"dataApiUrl":format!("http://{address}")})}}),&ctx("instance")).unwrap();
    }
    service
        .runtime()
        .credentials
        .store(
            "capture-fixture",
            &BTreeMap::from([
                ("api_key".into(), "fake-capture-canary-key".into()),
                ("api_secret".into(), "fake-capture-canary-secret".into()),
            ]),
        )
        .unwrap();
    *operations.0.lock().unwrap() = Some(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            service.runtime().storage.clone(),
            service.runtime().plugins.clone(),
            service.runtime().credentials.clone(),
            service.runtime().plugin_packages.clone(),
            service.runtime().clock.clone(),
            Arc::new(NativeTestSandbox),
        ),
    );
    for capability in [
        "test.capture.acquire",
        "test.capture.replay",
        "test.capture.derive",
    ] {
        let grant = json!({"grantId":capability,"profile":"local_full","source":"test","capability":capability,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null});
        service
            .runtime()
            .storage
            .put_json("entitlement_grants", capability, grant, &ctx("grant"))
            .unwrap();
    }
    let graph=service.handle_http("POST","/capability-graph-revisions",json!({"configId":"capture-graph","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper","additionalRequirements":[{"requirementId":"acquire","capability":"test.capture.acquire","mode":"paper","declaration":"required","fallbackPolicy":"none"},{"requirementId":"source","capability":"test.capture.replay","mode":"paper","declaration":"required","fallbackPolicy":"none"},{"requirementId":"derived","capability":"test.capture.derive","mode":"paper","declaration":"required","fallbackPolicy":"none"}]}));
    assert_eq!(
        graph.body["revision"]["graph"]["ok"], true,
        "{}",
        graph.body
    );
    let command = captures::CaptureCommand {
        policy: captures::CapturePolicy {
            scope,
            capability_graph_revision: graph.body["revision"]["revisionId"]
                .as_str()
                .unwrap()
                .into(),
            mode: "paper".into(),
            acquisition_node: "acquire".into(),
            replay_node: "source".into(),
            parameter_extracts: BTreeMap::from([
                ("snapshot".into(), "/snapshot".into()),
                ("digest".into(), "/digest".into()),
            ]),
            context: DataContext {
                instruments: vec![InstrumentContext {
                    instrument_id: "TEST".into(),
                    asset_class: "equity".into(),
                    instrument_family: "equity".into(),
                }],
                interval_ms: Some(86400000),
            },
            event_start_ms: 10 * 86400000,
            event_end_ms: 13 * 86400000,
            gap_policy: captures::GapPolicy::StrictFixedInterval,
        },
        input: json!({}),
    };
    Rig {
        dir,
        service,
        command,
        operations,
        server,
    }
}
#[test]
fn data_capture_real_process_product_admission_replay_and_service_recreation() {
    let r = rig();
    let response = r.service.handle_http(
        "POST",
        "/product/strategies/data-captures/create",
        json!({"capture":r.command,"idempotencyKey":"process-capture"}),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let receipt: captures::CaptureReceipt =
        serde_json::from_value(response.body["capture"].clone()).unwrap();
    assert!(!serde_json::to_string(&receipt)
        .unwrap()
        .contains("fake-capture-canary"));
    assert_eq!(receipt.replay_response.outputs["value"].points.len(), 3);
    let duplicate = r.service.handle_http(
        "POST",
        "/product/strategies/data-captures/create",
        json!({"capture":r.command,"idempotencyKey":"process-capture"}),
    );
    assert_eq!(duplicate.body["capture"], response.body["capture"]);
    let mut pipeline = PipelineRequest {
        source_captures: BTreeMap::from([("source".into(), receipt.capture_id.clone())]),
        capability_graph_revision: r.command.policy.capability_graph_revision.clone(),
        mode: "paper".into(),
        trade_instruments: vec![],
        nodes: vec![
            PipelineNode {
                graph_node_id: "source".into(),
                parameters: receipt.replay_parameters.clone(),
                context: r.command.policy.context.clone(),
                input_contexts: BTreeMap::new(),
            },
            PipelineNode {
                graph_node_id: "derived".into(),
                parameters: json!({}),
                context: r.command.policy.context.clone(),
                input_contexts: BTreeMap::new(),
            },
        ],
        bindings: vec![PortBinding {
            producer_node: "source".into(),
            output_port: "value".into(),
            consumer_node: "derived".into(),
            input_port: "source".into(),
        }],
    };
    use crate::data_strategy::{Comparison, DataStrategy, Operand, Rule, Sample};
    pipeline.trade_instruments = r.command.policy.context.instruments.clone();
    let program = DataStrategy {
        version: 1,
        nodes: pipeline.nodes.clone(),
        bindings: pipeline.bindings.clone(),
        trade_instruments: pipeline.trade_instruments.clone(),
        entry: Rule::Compare {
            left: Operand::Output {
                node: "derived".into(),
                port: "value".into(),
                instrument: "TEST".into(),
                sample: Sample::Current,
            },
            op: Comparison::Greater,
            right: Operand::Number { value: 0.0 },
        },
        exit: Rule::IsTrue {
            value: Operand::Boolean { value: false },
        },
    };
    let mut program = program;
    program.nodes[0].parameters = json!({});
    let mut spec: Value = serde_json::from_str(include_str!(
        "../../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .unwrap();
    spec["strategy_id"] = json!(r.command.policy.scope.strategy_id);
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.data_strategy":program});
    let saved = r.service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId":r.command.policy.scope.strategy_id,"spec":spec}),
    );
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(saved.body["body"]["draftSaved"], true, "{}", saved.body);
    let published=r.service.handle_http("POST","/product/strategies/publish",json!({"strategyId":r.command.policy.scope.strategy_id,"expectedDraftHash":saved.body["body"]["draft"]["draftHash"],"actor":{"kind":"user","id":"user.local"}}));
    assert_eq!(
        published.body["body"]["published"], true,
        "{}",
        published.body
    );
    let version = &published.body["body"]["version"];
    let scope = data_plans::PlanScope {
        strategy_id: r.command.policy.scope.strategy_id.clone(),
        strategy_version_id: version["id"].as_str().unwrap().into(),
        strategy_spec_hash: version["specHash"].as_str().unwrap().into(),
    };
    let graph=r.service.handle_http("POST","/capability-graph-revisions",json!({"configId":"captured-consumer","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper","additionalRequirements":[{"requirementId":"source","capability":"test.capture.replay","mode":"paper","declaration":"required","fallbackPolicy":"none"},{"requirementId":"derived","capability":"test.capture.derive","mode":"paper","declaration":"required","fallbackPolicy":"none"}]}));
    assert_eq!(
        graph.body["revision"]["graph"]["ok"], true,
        "{}",
        graph.body
    );
    pipeline.capability_graph_revision = graph.body["revision"]["revisionId"]
        .as_str()
        .unwrap()
        .into();
    let prepared=r.service.handle_http("POST","/product/strategies/data-plans/prepare",json!({"scope":scope,"capabilityGraphRevisionId":pipeline.capability_graph_revision,"mode":"paper","sourceCaptures":pipeline.source_captures,"idempotencyKey":"captured-product-plan"}));
    assert_eq!(prepared.status, 200, "{}", prepared.body);
    let plan: data_plans::StoredDataPlan =
        serde_json::from_value(prepared.body["plan"].clone()).unwrap();
    assert_eq!(plan.content.plan.schema_version, DataPlanVersion::V4);
    assert!(matches!(
        plan.content.source_authentication["source"],
        captures::SourceAuthentication::HostAcquisition { .. }
    ));
    let as_of = r.service.runtime().clock.now_ms();
    let make = |run: &str, start, end, prior| data_evaluation::EvaluationStep {
        run_id: run.into(),
        plan_revision_id: plan.revision_id.clone(),
        window: EvaluationWindow {
            event_start_ms: start,
            event_end_ms: end,
            as_of_ms: as_of,
        },
        expected_checkpoint_sha256: prior,
    };
    let mut early = make("early", 10 * 86400000, 13 * 86400000, None);
    early.window.as_of_ms = receipt.replay_observed_at_ms - 1;
    assert_eq!(
        data_evaluation::step(&r.service, early, &ctx("early")).unwrap_err(),
        "data_capture_window_outside_admission"
    );
    assert_eq!(
        data_evaluation::step(
            &r.service,
            make("outside", 9 * 86400000, 13 * 86400000, None),
            &ctx("outside")
        )
        .unwrap_err(),
        "data_capture_window_outside_admission"
    );
    let batch = data_evaluation::step(
        &r.service,
        make("batch", 10 * 86400000, 13 * 86400000, None),
        &ctx("batch"),
    )
    .unwrap();
    let first = data_evaluation::step(
        &r.service,
        make("incremental", 10 * 86400000, 11 * 86400000, None),
        &ctx("first"),
    )
    .unwrap();
    let db = r
        .dir
        .path()
        .join("service.db")
        .to_string_lossy()
        .into_owned();
    drop(r.service);
    drop(r.operations);
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.plugin_operations = Arc::new(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            runtime.storage.clone(),
            runtime.plugins.clone(),
            runtime.credentials.clone(),
            runtime.plugin_packages.clone(),
            runtime.clock.clone(),
            Arc::new(NativeTestSandbox),
        ),
    );
    let reopened = crate::service::TradeAssemblyService::from_runtime(&db, runtime);
    assert_eq!(
        captures::current(&reopened, &receipt.capture_id).unwrap(),
        receipt
    );
    let rest = data_evaluation::step(
        &reopened,
        make(
            "incremental",
            11 * 86400000,
            13 * 86400000,
            Some(first.checkpoint.checkpoint_sha256.clone()),
        ),
        &ctx("rest"),
    )
    .unwrap();
    let mut combined = first.outputs["derived"].clone();
    combined.extend(rest.outputs["derived"].clone());
    assert_eq!(combined, batch.outputs["derived"]);
    assert!(batch.outputs["derived"]
        .iter()
        .flat_map(|v| &v.outputs["value"].points)
        .all(|p| p.provenance.known_at_ms >= receipt.acquisition_response.observed_at_ms));
    pipeline.nodes[0].parameters["snapshot"]["values"][0] = json!(999);
    assert!(
        data_plans::save(&reopened, scope, pipeline, &ctx("modified"))
            .unwrap_err()
            .contains("binding_mismatch")
    );
    let mut instance = reopened
        .runtime()
        .plugins
        .get_instance("capture.source")
        .unwrap()
        .unwrap();
    instance["enabled"] = json!(false);
    reopened
        .runtime()
        .plugins
        .put_instance("capture.source", instance, &ctx("revoke-consumer"))
        .unwrap();
    let cached_prepare=reopened.handle_http("POST","/product/strategies/data-plans/prepare",json!({"scope":plan.content.scope,"capabilityGraphRevisionId":plan.content.plan.request.capability_graph_revision,"mode":"paper","sourceCaptures":plan.content.plan.request.source_captures,"idempotencyKey":"captured-product-plan"}));
    assert_ne!(
        cached_prepare.status, 200,
        "cached planning must revalidate capture admission"
    );
    r.server.join().unwrap();
}

#[test]
fn data_capture_process_rejects_gaps_future_data_and_missing_extraction_without_admission() {
    for case in ["gap", "future", "pointer", "headers"] {
        let mut r = rig();
        if case == "pointer" {
            r.command
                .policy
                .parameter_extracts
                .insert("snapshot".into(), "/absent".into());
        } else {
            r.command.input[case] = json!(true);
        }
        let response = r.service.handle_http(
            "POST",
            "/product/strategies/data-captures/create",
            json!({"capture":r.command,"idempotencyKey":"failed-capture"}),
        );
        assert_eq!(response.status, 409, "{case}: {}", response.body);
        assert!(r
            .service
            .runtime()
            .storage
            .list_json("data_acquisition_receipts_v1")
            .unwrap()
            .is_empty());
        assert!(r
            .service
            .runtime()
            .storage
            .list_json("data_acquisition_keys_v1")
            .unwrap()
            .is_empty());
        r.server.join().unwrap();
    }
}
#[test]
fn data_capture_process_atomic_failure_leaves_no_admitted_receipt_or_key() {
    let r = rig();
    let db = r.dir.path().join("service.db");
    let connection = rusqlite::Connection::open(db).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_capture BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='data_acquisition_receipts_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    let response = r.service.handle_http(
        "POST",
        "/product/strategies/data-captures/create",
        json!({"capture":r.command,"idempotencyKey":"atomic-capture"}),
    );
    assert_eq!(response.status, 409);
    assert!(r
        .service
        .runtime()
        .storage
        .list_json("data_acquisition_receipts_v1")
        .unwrap()
        .is_empty());
    assert!(r
        .service
        .runtime()
        .storage
        .list_json("data_acquisition_keys_v1")
        .unwrap()
        .is_empty());
    r.server.join().unwrap();
}
#[test]
fn data_capture_process_tamper_foreign_owner_and_revoked_cached_retry_fail_closed() {
    let r = rig();
    let body = json!({"capture":r.command,"idempotencyKey":"secure-capture"});
    let response = r.service.handle_http(
        "POST",
        "/product/strategies/data-captures/create",
        body.clone(),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let receipt: captures::CaptureReceipt =
        serde_json::from_value(response.body["capture"].clone()).unwrap();
    let runtime = r.service.runtime();
    let mut forged = body.clone();
    forged["capture"]["response"] = json!(receipt.acquisition_response);
    forged["idempotencyKey"] = json!("caller-supplied-response");
    assert_ne!(
        r.service
            .handle_http("POST", "/product/strategies/data-captures/create", forged)
            .status,
        200
    );
    assert_eq!(
        runtime
            .storage
            .list_json("data_acquisition_receipts_v1")
            .unwrap()
            .len(),
        1
    );
    let original = runtime
        .storage
        .get_json("data_acquisition_receipts_v1", &receipt.capture_id)
        .unwrap()
        .unwrap();
    let mut changed = original.clone();
    changed["replayParameters"]["snapshot"]["values"][0] = json!(999);
    runtime
        .storage
        .put_json(
            "data_acquisition_receipts_v1",
            &receipt.capture_id,
            changed,
            &ctx("tamper"),
        )
        .unwrap();
    assert!(captures::current(&r.service, &receipt.capture_id).is_err());
    assert_ne!(
        r.service
            .handle_http(
                "POST",
                "/product/strategies/data-captures/create",
                body.clone()
            )
            .status,
        200,
        "cached responses must revalidate receipt integrity"
    );
    runtime
        .storage
        .put_json(
            "data_acquisition_receipts_v1",
            &receipt.capture_id,
            original,
            &ctx("restore"),
        )
        .unwrap();
    assert!(captures::current(&r.service, "capture_missing").is_err());
    let alice = r
        .service
        .for_authenticated_invocation("test", "alice", None, None);
    alice
        .bind_owned_object("strategy", &receipt.policy.scope.strategy_id)
        .unwrap();
    for instance in runtime.plugins.list_instances().unwrap() {
        alice
            .bind_owned_object("plugin_instance", instance["instanceRef"].as_str().unwrap())
            .unwrap();
    }
    assert!(captures::current(&alice, &receipt.capture_id).is_ok());
    let bob = r
        .service
        .for_authenticated_invocation("test", "bob", None, None);
    assert!(captures::current(&bob, &receipt.capture_id).is_err());
    let mut instance = runtime
        .plugins
        .get_instance("capture.source")
        .unwrap()
        .unwrap();
    instance["enabled"] = json!(false);
    runtime
        .plugins
        .put_instance("capture.source", instance, &ctx("revoke"))
        .unwrap();
    assert_ne!(
        r.service
            .handle_http("POST", "/product/strategies/data-captures/create", body)
            .status,
        200
    );
    let mcp = r.service.call_mcp_tool(
        "tradeassembly.strategy.data_capture.get",
        json!({"captureId":receipt.capture_id}),
    );
    assert!(mcp.get("capture").is_none());
    r.server.join().unwrap();
}

#[test]
#[ignore = "requires an explicitly pinned DATA_CAPTURE_PROVIDER_CONFIG; loopback mock and fake credentials only"]
fn data_capture_external_provider_package_admits_replays_and_reopens_without_live_services() {
    let config: Value = serde_json::from_slice(
        &fs::read(
            std::env::var_os("DATA_CAPTURE_PROVIDER_CONFIG")
                .expect("explicit fixture config required"),
        )
        .unwrap(),
    )
    .unwrap();
    let operations = Arc::new(ForwardNative::default());
    let (dir, service, scope, _) = data_plans::tests::fixture_service(Some(operations.clone()));
    let archive = fs::read(config["archive"].as_str().unwrap()).unwrap();
    assert_eq!(sha256(&archive), config["archiveSha256"].as_str().unwrap());
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()));
    let manifest: Value = tar
        .entries()
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            matches!(
                e.path().unwrap().to_str(),
                Some("manifest.yaml" | "manifest.json")
            )
        })
        .map(|mut e| {
            let mut bytes = vec![];
            e.read_to_end(&mut bytes).unwrap();
            serde_yaml::from_slice(&bytes).unwrap()
        })
        .unwrap();
    assert_eq!(
        canonical_sha256(&manifest).unwrap(),
        config["manifestSha256"].as_str().unwrap()
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let mock_body = serde_json::to_vec(&config["httpResponse"]).unwrap();
    let prefix = config["httpRequestPrefix"].as_str().unwrap().to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 8192];
        let n = stream.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with(&prefix));
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",mock_body.len()).unwrap();
        stream.write_all(&mock_body).unwrap();
    });
    let package = service
        .runtime()
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: config["archive"].as_str().unwrap().into(),
            package_sha256: sha256(&archive),
            manifest_sha256: canonical_sha256(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    let mut configuration = config["configuration"].clone();
    for value in configuration.as_object_mut().unwrap().values_mut() {
        if value == "__LOCAL_MOCK__" {
            *value = json!(format!("http://{address}"));
        } else if value.as_str().is_some_and(|s| s.contains("://")) {
            panic!("fixture configuration must not name a non-local service");
        }
    }
    let fields: BTreeMap<String, String> =
        serde_json::from_value(config["fakeCredentialFields"].clone()).unwrap();
    assert!(!fields.is_empty() && fields.values().all(|s| s.starts_with("fixture-")));
    service
        .runtime()
        .credentials
        .store("external-capture-fixture", &fields)
        .unwrap();
    service.runtime().plugins.install_manifest(&package.plugin_ref,json!({"pluginRef":package.plugin_ref,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx("external-register")).unwrap();
    service.runtime().plugins.put_instance("external-capture-fixture",json!({"instanceRef":"external-capture-fixture","providerRef":"external-capture-fixture","pluginRef":package.plugin_ref,"enabled":true,"activePackageSha256":package.package_sha256,"credentialRef":"external-capture-fixture","configuration":configuration}),&ctx("external-instance")).unwrap();
    *operations.0.lock().unwrap() = Some(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            service.runtime().storage.clone(),
            service.runtime().plugins.clone(),
            service.runtime().credentials.clone(),
            service.runtime().plugin_packages.clone(),
            service.runtime().clock.clone(),
            Arc::new(NativeTestSandbox),
        ),
    );
    let mut requirements = vec![];
    let mut bindings = json!({"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}});
    for (node, key) in [
        ("acquire", "acquisitionOperation"),
        ("source", "replayOperation"),
    ] {
        let operation = manifest["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|op| op["id"] == config[key])
            .unwrap();
        let capability = operation["capability"].as_str().unwrap();
        service.runtime().storage.put_json("entitlement_grants",capability,json!({"grantId":capability,"profile":"local_full","source":"test","capability":capability,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("external-grant")).unwrap();
        requirements.push(json!({"requirementId":node,"capability":capability,"mode":"paper","declaration":"required","fallbackPolicy":"none"}));
        bindings[node] = json!({"pluginInstanceRef":"external-capture-fixture","pluginRef":package.plugin_ref,"operationId":config[key]});
    }
    let graph=service.handle_http("POST","/capability-graph-revisions",json!({"configId":"external-capture-graph","bindings":bindings,"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper","additionalRequirements":requirements}));
    assert_eq!(
        graph.body["revision"]["graph"]["ok"], true,
        "{}",
        graph.body
    );
    let command = captures::CaptureCommand {
        policy: captures::CapturePolicy {
            scope: scope.clone(),
            capability_graph_revision: graph.body["revision"]["revisionId"]
                .as_str()
                .unwrap()
                .into(),
            mode: "paper".into(),
            acquisition_node: "acquire".into(),
            replay_node: "source".into(),
            parameter_extracts: serde_json::from_value(config["parameterExtracts"].clone())
                .unwrap(),
            context: serde_json::from_value(config["context"].clone()).unwrap(),
            event_start_ms: config["eventStartMs"].as_i64().unwrap(),
            event_end_ms: config["eventEndMs"].as_i64().unwrap(),
            gap_policy: captures::GapPolicy::StrictFixedInterval,
        },
        input: config["input"].clone(),
    };
    let response = service.handle_http(
        "POST",
        "/product/strategies/data-captures/create",
        json!({"capture":command,"idempotencyKey":"external-native-capture"}),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let receipt: captures::CaptureReceipt =
        serde_json::from_value(response.body["capture"].clone()).unwrap();
    for value in fields.values() {
        assert!(!serde_json::to_string(&receipt).unwrap().contains(value));
    }
    let plan = data_plans::save(
        &service,
        scope,
        PipelineRequest {
            source_captures: BTreeMap::from([("source".into(), receipt.capture_id.clone())]),
            capability_graph_revision: command.policy.capability_graph_revision.clone(),
            mode: "paper".into(),
            trade_instruments: vec![],
            nodes: vec![PipelineNode {
                graph_node_id: "source".into(),
                parameters: receipt.replay_parameters.clone(),
                context: command.policy.context.clone(),
                input_contexts: BTreeMap::new(),
            }],
            bindings: vec![],
        },
        &ctx("external-native-plan"),
    )
    .unwrap();
    let as_of = service.runtime().clock.now_ms();
    let step = |run: &str, start, end, prior| data_evaluation::EvaluationStep {
        run_id: run.into(),
        plan_revision_id: plan.revision_id.clone(),
        window: EvaluationWindow {
            event_start_ms: start,
            event_end_ms: end,
            as_of_ms: as_of,
        },
        expected_checkpoint_sha256: prior,
    };
    let start = command.policy.event_start_ms;
    let end = command.policy.event_end_ms;
    let mid = start + command.policy.context.interval_ms.unwrap() as i64;
    let batch = data_evaluation::step(
        &service,
        step("external-batch", start, end, None),
        &ctx("external-batch"),
    )
    .unwrap();
    let first = data_evaluation::step(
        &service,
        step("external-incremental", start, mid, None),
        &ctx("external-first"),
    )
    .unwrap();
    let db = dir.path().join("service.db").to_string_lossy().into_owned();
    drop(service);
    drop(operations);
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.plugin_operations = Arc::new(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            runtime.storage.clone(),
            runtime.plugins.clone(),
            runtime.credentials.clone(),
            runtime.plugin_packages.clone(),
            runtime.clock.clone(),
            Arc::new(NativeTestSandbox),
        ),
    );
    let reopened = crate::service::TradeAssemblyService::from_runtime(&db, runtime);
    assert_eq!(
        captures::current(&reopened, &receipt.capture_id).unwrap(),
        receipt
    );
    let rest = data_evaluation::step(
        &reopened,
        step(
            "external-incremental",
            mid,
            end,
            Some(first.checkpoint.checkpoint_sha256),
        ),
        &ctx("external-rest"),
    )
    .unwrap();
    let mut combined = first.outputs["source"].clone();
    combined.extend(rest.outputs["source"].clone());
    assert_eq!(combined, batch.outputs["source"]);
    server.join().unwrap();
}
