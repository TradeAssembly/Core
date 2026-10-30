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
fn fixture_executable() -> &'static [u8] {
    static BINARY: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        // Compile the Rust-only fixture against this test build's locked
        // dependency artifacts; no nested Cargo command or registry access.
        let deps = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("capture-fixture");
        let mut command = std::process::Command::new("rustc");
        command
            .args(["--edition=2021", "--crate-name", "capture_fixture"])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/adapters/fixtures/capture_plugin.rs"),
            )
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .arg("-o")
            .arg(&output);
        for name in ["serde_json", "sha2", "serde_json_canonicalizer"] {
            let mut libraries = std::fs::read_dir(&deps)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("lib{name}-"))
                        && e.path().extension().is_some_and(|e| e == "rlib")
                })
                .map(|e| e.path())
                .collect::<Vec<_>>();
            libraries.sort_by_key(|p| std::fs::metadata(p).unwrap().modified().unwrap());
            let library = libraries
                .last()
                .expect("locked Rust fixture dependency artifact");
            command
                .arg("--extern")
                .arg(format!("{name}={}", library.display()));
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Rust fixture compile failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::fs::read(output).unwrap()
    })
}
fn install_fixture(
    service: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
    calculator: bool,
    interval_ms: u64,
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
    contract["outputs"]["value"]["time"]["intervalMs"] = json!(interval_ms);
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
        operations.push(operation.clone());
        operation["id"] = json!("chain");
        operation["capability"] = json!("test.option.chain@1");
        operation["traits"]["dataShapes"] = json!(["option_chain"]);
        operation["traits"]["outputSchemaRefs"] =
            json!(["schema://capture-fixture/option-chain@1"]);
        operations.push(operation);
    }
    let manifest = json!({"metadata":{"id":id,"version":"1","provider":{"id":id,"name":"Local capture fixture"}},"operations":operations,
        "capabilities":[{"id":"test.capture.acquire"},{"id":"test.capture.replay"},{"id":"test.capture.derive"},{"id":"test.option.chain@1"}],"permissions":if calculator {vec![]}else{vec!["network.outbound"]},
        "configuration":{"fields":[{"id":"dataApiUrl","inputType":"url","storageClass":"configuration"}]},"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let contract_bytes = serde_json::to_vec(&contract).unwrap();
    let script = fixture_executable();
    let response = serde_json::to_vec(&json!({"type":"object"})).unwrap();
    descriptor["package"]["plugin"]["id"] = json!(id);
    descriptor["package"]["manifest"]["sha256"] = json!(sha256(&manifest_bytes));
    descriptor["package"]["targets"][0]["binary"]["sha256"] = json!(sha256(script));
    descriptor["package"]["responseSchemas"] = json!([{ "schemaRef":"schema://capture-fixture/option-chain@1","document":{"path":"response.json","sha256":sha256(&response)}},{"schemaRef":EVALUATION_RESPONSE_SCHEMA,"document":{"path":"response.json","sha256":sha256(&response)}},{"schemaRef":"schema://capture-fixture/response@1","document":{"path":"response.json","sha256":sha256(&response)}}]);
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
struct AdvancedClock {
    base: Arc<dyn ClockPort>,
    advance: Arc<std::sync::atomic::AtomicI64>,
}
impl VersionedPort for AdvancedClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl ClockPort for AdvancedClock {
    fn now_ms(&self) -> i64 {
        self.base.now_ms() + self.advance.load(std::sync::atomic::Ordering::SeqCst)
    }
}
struct Rig {
    dir: tempfile::TempDir,
    service: crate::service::TradeAssemblyService,
    command: captures::CaptureCommand,
    operations: Arc<ForwardNative>,
    server: std::thread::JoinHandle<()>,
}
fn rig() -> Rig {
    rig_with_options(false)
}
fn rig_with_options(options: bool) -> Rig {
    let operations = Arc::new(ForwardNative::default());
    let (dir, service, scope, _) = data_plans::tests::fixture_service(Some(operations.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for index in 0..if options { 4 } else { 1 } {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 8192];
            let n = stream.read(&mut buf).unwrap();
            let path = if index == 0 {
                "/observations"
            } else {
                "/options"
            };
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with(&format!("GET {path} ")));
            let day = 86400000_i64;
            let now = chrono::Utc::now();
            let start = if options {
                now.timestamp_millis() / 60000 * 60000 - 3 * 60000
            } else {
                10 * day
            };
            let body = if index == 0 {
                json!({"values":[10,20,30],"start":start,"interval":if options {60000}else{day},"instrument":"TEST"})
            } else {
                let expiry = (now.date_naive() + chrono::Days::new(21))
                    .format("%Y-%m-%d")
                    .to_string();
                let quote_time = if index == 1 {
                    now - chrono::Duration::milliseconds(86400000 - 120000)
                } else {
                    now
                };
                let rows=["B","A"].iter().map(|id|json!({"instrumentId":id,"timestamp":quote_time.to_rfc3339(),"data":{"kind":"option_contract","data":{"contractSymbol":id,"underlyingInstrumentId":"TEST","expiration":expiry,"strike":500.0,"right":"call","bid":4.0,"ask":4.2,"volume":100,"openInterest":50,"greeks":{"delta":if index==1 {0.5}else{0.1}}}}})).collect::<Vec<_>>();
                // Synthetic HTTP response exercises the production wire profile;
                // this fixture is never authenticated market-data evidence.
                json!({"sourceClass":"production","observations":rows,"nextPageToken":if index==3 {json!("more")}else{Value::Null},"upstreamRefs":["fixture:options"]})
            };
            let body = serde_json::to_vec(&body).unwrap();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    for calculator in [false, true] {
        let package = install_fixture(
            &service,
            dir.path(),
            calculator,
            if options { 60000 } else { 86400000 },
        );
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
        "test.option.chain@1",
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
                interval_ms: Some(if options { 60000 } else { 86400000 }),
            },
            event_start_ms: if options {
                service.runtime().clock.now_ms() / 60000 * 60000 - 3 * 60000
            } else {
                10 * 86400000
            },
            event_end_ms: if options {
                service.runtime().clock.now_ms() / 60000 * 60000
            } else {
                13 * 86400000
            },
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
    let missing = r.service.call_mcp_tool(
        "tradeassembly.strategy.data_capture.get",
        json!({"captureId":"capture_missing"}),
    );
    assert_eq!(missing["isError"], true, "{missing}");

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
    assert_eq!(mcp["isError"], true, "{mcp}");
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

#[test]
fn option_selection_real_process_causal_signal_selection_restart_and_no_match() {
    use crate::data_strategy::{Comparison, DataStrategy, Operand, Rule, Sample};
    use crate::service::option_selections::{SelectionCommand, SelectionReceipt};
    let r = rig_with_options(true);
    let response = r.service.handle_http(
        "POST",
        "/product/strategies/data-captures/create",
        json!({"capture":r.command,"idempotencyKey":"options-underlying-capture"}),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let capture: captures::CaptureReceipt =
        serde_json::from_value(response.body["capture"].clone()).unwrap();
    let program = DataStrategy {
        version: 1,
        nodes: vec![PipelineNode {
            graph_node_id: "source".into(),
            parameters: json!({}),
            context: r.command.policy.context.clone(),
            input_contexts: BTreeMap::new(),
        }],
        bindings: vec![],
        trade_instruments: vec![InstrumentContext {
            instrument_id: "STATIC_NOT_SELECTED".into(),
            asset_class: "option".into(),
            instrument_family: "option_contract".into(),
        }],
        entry: Rule::Compare {
            left: Operand::Output {
                node: "source".into(),
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
    let mut policy = crate::option_selection::tests::policy();
    policy.signal_requirement_ref = "signal_primary".into();
    policy.trade_requirement_ref = "trade_primary".into();
    policy.underlying_selector_ref = "primary_symbol".into();
    policy.max_age_ms = 86400000;
    policy.max_pages = 1;
    let mut spec: Value = serde_json::from_str(include_str!(
        "../../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .unwrap();
    spec["strategy_id"] = json!(r.command.policy.scope.strategy_id);
    spec["signal_market_requirements"][0]["selectors"][0]["normalized_id"] = json!("TEST");
    spec["trade_market_requirements"][0]["asset_classes"] = json!(["option"]);
    spec["trade_market_requirements"][0]["instrument_families"] = json!(["option_contract"]);
    spec["trade_market_requirements"][0]["required_capability_refs"] = json!([]);
    spec["trade_market_requirements"][0]["quantity_units"] = json!(["contract"]);
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.data_strategy":program});
    let mut chain_req = spec["capability_requirements"]["required"][0].clone();
    chain_req["requirement_id"] = json!("chain");
    chain_req["capability"] = json!("test.option.chain@1");
    chain_req["purpose"] = json!("Acquire normalized option snapshots");
    chain_req["stage_refs"] = json!(["intent"]);
    chain_req["substep_refs"] = json!(["select_options"]);
    chain_req["constraints"]["data_shapes"] = json!(["option_chain"]);
    chain_req["constraints"]["input_paths"] = json!([]);
    chain_req["constraints"]["schema_refs"] = json!(["schema://capture-fixture/option-chain@1"]);
    chain_req["constraints"]["fields"] = json!([]);
    chain_req["constraints"]["deterministic"] = json!(false);
    chain_req["constraints"]["replayable"] = json!(false);
    chain_req["constraints"]["operations"] = json!([]);
    chain_req["constraints"]
        .as_object_mut()
        .unwrap()
        .remove("max_freshness");
    chain_req["constraints"]
        .as_object_mut()
        .unwrap()
        .remove("timeframe");
    spec["capability_requirements"]["required"]
        .as_array_mut()
        .unwrap()
        .push(chain_req);
    let mut step = spec["stages"]["universe"]["substeps"][0].clone();
    step["id"] = json!("select_options");
    step["operation_ref"] = json!("option.contract.select@1");
    step["capability_requirement_refs"] = json!(["chain"]);
    step["reads"] = json!(["signals.primary"]);
    step["writes"] = json!(["assets.selected_option"]);
    step["option_selection"] = json!(policy);
    spec["stages"]["intent"]["substeps"]
        .as_array_mut()
        .unwrap()
        .insert(0, step);
    spec["stages"]["intent"]["substeps"][1]["reads"] =
        json!(["signals.primary", "assets.selected_option"]);
    let report = crate::spec::validate_strategy_spec_report(&spec);
    assert!(
        report.valid,
        "{}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    assert!(crate::strategy_kernel::CompiledStrategy::compile_json(&spec).is_err());
    let saved = r.service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId":r.command.policy.scope.strategy_id,"spec":spec}),
    );
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
    let graph=r.service.handle_http("POST","/capability-graph-revisions",json!({"configId":"option-journey","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"},"chain":{"pluginInstanceRef":"capture.source","pluginRef":"capture.source","operationId":"chain"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper","additionalRequirements":[{"requirementId":"source","capability":"test.capture.replay","mode":"paper","declaration":"required","fallbackPolicy":"none"}]}));
    assert_eq!(
        graph.body["revision"]["graph"]["ok"], true,
        "{}",
        graph.body
    );
    let revision = graph.body["revision"]["revisionId"].as_str().unwrap();
    let prepared=r.service.handle_http("POST","/product/strategies/data-plans/prepare",json!({"scope":scope,"capabilityGraphRevisionId":revision,"mode":"paper","sourceCaptures":{"source":capture.capture_id},"idempotencyKey":"option-plan"}));
    assert_eq!(prepared.status, 200, "{}", prepared.body);
    let signal_step = data_evaluation::EvaluationStep {
        run_id: "options-signal".into(),
        plan_revision_id: prepared.body["plan"]["revisionId"].as_str().unwrap().into(),
        window: EvaluationWindow {
            event_start_ms: r.command.policy.event_start_ms,
            event_end_ms: r.command.policy.event_end_ms,
            as_of_ms: r.service.runtime().clock.now_ms(),
        },
        expected_checkpoint_sha256: None,
    };
    let evaluated = r.service.handle_http(
        "POST",
        "/product/strategies/data-runs/step",
        json!({"step":signal_step,"idempotencyKey":"option-signal-step"}),
    );
    assert_eq!(evaluated.status, 200, "{}", evaluated.body);
    assert_eq!(
        evaluated.body["receipt"]["signals"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["entry"]["value"],
        true
    );
    let command = SelectionCommand {
        scope,
        capability_graph_revision: revision.into(),
        acquisition_node: "chain".into(),
        mode: "paper".into(),
        stage_substep_id: "select_options".into(),
        signal_step,
        signal_idempotency_key: "option-signal-step".into(),
        signal_event_end_ms: r.command.policy.event_end_ms,
    };
    let body = json!({"selection":command,"idempotencyKey":"first-option-selection"});
    let selected = r.service.handle_http(
        "POST",
        "/product/strategies/option-selections/create",
        body.clone(),
    );
    assert_eq!(selected.status, 200, "{}", selected.body);
    let stored = r
        .service
        .runtime()
        .storage
        .get_json(
            "option_selection_receipts_v1",
            selected.body["selection"]["receiptId"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
    let receipt: SelectionReceipt = serde_json::from_value(stored).unwrap();
    assert!(selected.body["selection"].get("responses").is_none());
    assert!(!selected.body.to_string().contains("nextPageToken"));
    assert!(
        matches!(receipt.result.outcome,crate::option_selection::OptionOutcome::Selected{ref contract} if contract.contract_id=="A" && contract.underlying=="TEST")
    );
    assert!(!receipt.result.stage_failed);
    assert!(!receipt.signal_receipt_sha256.is_empty());
    assert!(!serde_json::to_string(&receipt)
        .unwrap()
        .contains("fake-capture-canary"));
    assert_eq!(selected.body["executionAuthorityGranted"], false);
    let db = r
        .dir
        .path()
        .join("service.db")
        .to_string_lossy()
        .into_owned();
    drop(r.service);
    drop(r.operations);
    let clock_advance = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.clock = Arc::new(AdvancedClock {
        base: runtime.clock.clone(),
        advance: clock_advance.clone(),
    });
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
    let service = crate::service::TradeAssemblyService::from_runtime(&db, runtime);
    let retry = service.handle_http(
        "POST",
        "/product/strategies/option-selections/create",
        body.clone(),
    );
    assert_eq!(
        retry.body["selection"], selected.body["selection"],
        "{}",
        retry.body
    );
    let fresh_mcp = service.call_mcp_tool(
        "tradeassembly.strategy.option_selection.create",
        body.clone(),
    );
    assert_eq!(fresh_mcp["isError"], false, "{fresh_mcp}");
    assert_eq!(
        fresh_mcp["structuredContent"]["selection"]["receiptId"],
        selected.body["selection"]["receiptId"],
        "{fresh_mcp}"
    );
    let indexes = service
        .runtime()
        .storage
        .list_json("option_selection_keys_v1")
        .unwrap();
    assert_eq!(
        indexes.len(),
        1,
        "HTTP and MCP must share one host decision"
    );
    let legacy_authority = crate::control_plane::http_envelope(
        "http",
        "POST",
        "/product/strategies/option-selections/create",
        &body,
    )
    .unwrap()
    .authority;
    let legacy_key = canonical_sha256(&(
        &command.scope.strategy_id,
        &legacy_authority,
        "first-option-selection",
    ))
    .unwrap();
    service
        .runtime()
        .storage
        .clear_namespace("option_selection_keys_v1")
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            "option_selection_keys_v1",
            &legacy_key,
            indexes[0].1.clone(),
            &ctx("legacy-selection-index"),
        )
        .unwrap();
    let legacy_retry = service.call_mcp_tool(
        "tradeassembly.strategy.option_selection.create",
        body.clone(),
    );
    assert_eq!(legacy_retry["isError"], false, "{legacy_retry}");
    assert_eq!(
        legacy_retry["structuredContent"]["selection"]["receiptId"],
        selected.body["selection"]["receiptId"]
    );
    let original = service
        .runtime()
        .storage
        .get_json("option_selection_receipts_v1", &receipt.receipt_id)
        .unwrap()
        .unwrap();
    let mut forged = original.clone();
    forged["result"]["outcome"]["contract"]["contractId"] = json!("FORGED");
    forged["receiptId"] = json!("");
    forged["receiptId"] = json!(canonical_sha256(&forged).unwrap());
    service
        .runtime()
        .storage
        .put_json(
            "option_selection_receipts_v1",
            &receipt.receipt_id,
            forged,
            &ctx("tamper-option-receipt"),
        )
        .unwrap();
    assert_ne!(
        service
            .handle_http(
                "POST",
                "/product/strategies/option-selections/create",
                body.clone()
            )
            .status,
        200,
        "a rehashed row under the wrong receipt ID must reject"
    );
    service
        .runtime()
        .storage
        .put_json(
            "option_selection_receipts_v1",
            &receipt.receipt_id,
            original,
            &ctx("restore-option-receipt"),
        )
        .unwrap();
    let no_match = service.handle_http(
        "POST",
        "/product/strategies/option-selections/create",
        json!({"selection":command,"idempotencyKey":"second-option-selection"}),
    );
    assert_eq!(no_match.status, 200, "{}", no_match.body);
    assert_eq!(
        no_match.body["selection"]["result"]["outcome"]["status"],
        "no_eligible_contract"
    );
    assert_eq!(
        no_match.body["selection"]["result"]["rejected"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let incomplete = service.handle_http(
        "POST",
        "/product/strategies/option-selections/create",
        json!({"selection":command,"idempotencyKey":"incomplete-option-selection"}),
    );
    assert_eq!(incomplete.status, 409, "{}", incomplete.body);
    assert_eq!(incomplete.body["error"]["code"], "option_chain_incomplete");
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("option_selection_receipts_v1")
            .unwrap()
            .len(),
        2
    );
    let original_before_expiry = service
        .runtime()
        .storage
        .get_json("option_selection_receipts_v1", &receipt.receipt_id)
        .unwrap()
        .unwrap();
    let operation_count = service
        .runtime()
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap()
        .len();
    clock_advance.store(180000, std::sync::atomic::Ordering::SeqCst);
    assert!(
        service.runtime().clock.now_ms() - command.signal_event_end_ms < 86400000,
        "signal must remain fresh while the quote expires"
    );
    let expired_http = service.handle_http(
        "POST",
        "/product/strategies/option-selections/create",
        body.clone(),
    );
    let expired_mcp = service.call_mcp_tool(
        "tradeassembly.strategy.option_selection.create",
        body.clone(),
    );
    assert_eq!(
        (expired_http.status, expired_mcp["isError"].as_bool()),
        (409, Some(true)),
        "HTTP:{} MCP:{}",
        expired_http.body,
        expired_mcp
    );
    assert_eq!(
        expired_http.body["error"]["code"],
        "option_selection_receipt_stale"
    );
    assert_eq!(
        expired_mcp["structuredContent"]["error"]["code"], "option_selection_receipt_stale",
        "{expired_mcp}"
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .get_json("option_selection_receipts_v1", &receipt.receipt_id)
            .unwrap()
            .unwrap(),
        original_before_expiry
    );
    assert_eq!(
        service
            .runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap()
            .len(),
        operation_count,
        "stale retries must not reacquire"
    );
    let mut pin = service
        .runtime()
        .plugins
        .get_instance("capture.source")
        .unwrap()
        .unwrap();
    pin["enabled"] = json!(false);
    service
        .runtime()
        .plugins
        .put_instance("capture.source", pin, &ctx("revoke-option-source"))
        .unwrap();
    assert_ne!(
        service
            .handle_http("POST", "/product/strategies/option-selections/create", body)
            .status,
        200
    );
    let alice = service.for_authenticated_invocation("test", "alice", None, None);
    alice
        .bind_owned_object("strategy", &command.scope.strategy_id)
        .unwrap();
    let bob = service.for_authenticated_invocation("test", "bob", None, None);
    let denied = bob.call_mcp_tool(
        "tradeassembly.strategy.option_selection.create",
        json!({"selection":command,"idempotencyKey":"foreign-owner-option"}),
    );
    assert_eq!(denied["isError"], true, "{denied}");
    assert!(denied.get("selection").is_none());
    assert!(service
        .runtime()
        .storage
        .list_json("orders")
        .unwrap()
        .is_empty());
    r.server.join().unwrap();
}
