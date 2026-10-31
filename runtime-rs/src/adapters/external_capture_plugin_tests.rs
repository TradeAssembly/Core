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
// Diagnostic boundary for this authored fake-data fixture only. Production
// plugins keep stderr suppressed by the normal host sandbox.
struct TracedFixtureSandbox;
impl VersionedPort for TracedFixtureSandbox {fn descriptors(&self)->Vec<PortDescriptor>{vec![]}}
impl PluginProcessSandboxPort for TracedFixtureSandbox {
 fn spawn(&self,r:&PluginSandboxRequest)->Result<SandboxedPluginProcess,String>{
  let child=std::process::Command::new(&r.executable).current_dir(&r.install_root).env_clear().stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::inherit()).spawn().map_err(|_|"fixture_spawn_failed")?;
  eprintln!("authored fixture child pid={}",child.id());Ok(SandboxedPluginProcess::new(child,None))
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
fn install_fixture_with_recipe(
    service: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
    calculator: bool,
    interval_ms: u64,
    recipe: bool,
) -> InstalledPluginPackage {
    install_fixture_variant(service,root,calculator,interval_ms,recipe,false)
}
fn install_fixture_variant(
    service:&crate::service::TradeAssemblyService,root:&std::path::Path,
    calculator:bool,interval_ms:u64,recipe:bool,versioned_acquisition:bool,
)->InstalledPluginPackage {
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
        operation["capability"] = json!(if versioned_acquisition {"test.screen.source@1"} else {"test.capture.acquire"});
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
        operations.push(operation.clone());
        operation["id"]=json!("calendar");
        operation["capability"]=json!("test.session.calendar@1");
        operation["traits"]["dataShapes"]=json!(["session_calendar"]);
        operation["traits"]["outputSchemaRefs"]=json!([tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA]);
        operations.push(operation);
    }
    let manifest = json!({"metadata":{"id":id,"version":"1","provider":{"id":id,"name":"Local capture fixture"}},"operations":operations,
        "capabilities":[{"id":if versioned_acquisition {"test.screen.source@1"}else{"test.capture.acquire"}},{"id":"test.capture.replay"},{"id":"test.capture.derive"},{"id":"test.option.chain@1"},{"id":"test.session.calendar@1"}],"permissions":if calculator {vec![]}else{vec!["network.outbound"]},
        "configuration":{"fields":[{"id":"dataApiUrl","inputType":"url","storageClass":"configuration"}]},"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let contract_bytes = serde_json::to_vec(&contract).unwrap();
    let script = fixture_executable();
    let response = serde_json::to_vec(&json!({"type":"object"})).unwrap();
    descriptor["package"]["plugin"]["id"] = json!(id);
    descriptor["package"]["manifest"]["sha256"] = json!(sha256(&manifest_bytes));
    descriptor["package"]["targets"][0]["binary"]["sha256"] = json!(sha256(script));
    descriptor["package"]["responseSchemas"] = json!([{ "schemaRef":"schema://capture-fixture/option-chain@1","document":{"path":"response.json","sha256":sha256(&response)}},{"schemaRef":EVALUATION_RESPONSE_SCHEMA,"document":{"path":"response.json","sha256":sha256(&response)}},{"schemaRef":"schema://capture-fixture/response@1","document":{"path":"response.json","sha256":sha256(&response)}}]);
    let calendar_schema=serde_json::to_vec(&tradeassembly_plugin_sdk::session_calendar::schema()).unwrap();
    descriptor["package"]["responseSchemas"].as_array_mut().unwrap().push(json!({"schemaRef":tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA,"document":{"path":"calendar.json","sha256":sha256(&calendar_schema)}}));
    descriptor["dataContracts"][0]["operationId"] = json!(opid);
    descriptor["dataContracts"][0]["document"]["sha256"] = json!(sha256(&contract_bytes));
    descriptor["dataContracts"][0]["canonicalSha256"] = json!(canonical_sha256(&contract).unwrap());
    let recipe_bytes = if recipe {
        assert!(calculator);
        let definition=json!({"schemaVersion":"tradeassembly.data_recipe.v1","recipeId":"source-projection","semanticsVersion":"1","parameterSchema":{"type":"object","additionalProperties":false},"inputs":{"source":contract["inputs"]["source"]["data"]},"nodes":{"projection":{"operation":{"pluginRef":"self","operationId":"derive","semanticsVersion":contract["semanticsVersion"]},"parameters":{}}},"bindings":[{"producer":{"kind":"input","port":"source"},"consumerNode":"projection","inputPort":"source"}],"outputs":{"value":{"producer":{"kind":"output","node":"projection","port":"value"},"data":contract["outputs"]["value"]}}});
        let bytes=serde_json::to_vec_pretty(&definition).unwrap();
        descriptor["packageContractVersion"]=json!("3");
        descriptor["recipes"]=json!([{"recipeId":"source-projection","document":{"path":"recipe.json","sha256":sha256(&bytes)},"canonicalSha256":canonical_sha256(&definition).unwrap()}]);
        Some(bytes)
    } else {None};
    let mut files = vec![
        (
            "tradeassembly-plugin.json",
            serde_json::to_vec(&descriptor).unwrap(),
        ),
        ("manifest.json", manifest_bytes),
        ("contract.json", contract_bytes),
        ("schema.json", schema_bytes),
        ("response.json", response),
        ("calendar.json",calendar_schema),
        ("bin/plugin", script.to_vec()),
    ];
    if let Some(bytes)=recipe_bytes {files.push(("recipe.json",bytes));}
    let archive=archive_files(files);
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

fn install_screen_package(service:&crate::service::TradeAssemblyService,root:&std::path::Path)->InstalledPluginPackage {
    use crate::adapters::plugin_packages::data_package_tests::{archive_files,materials};
    use tradeassembly_plugin_sdk::instrument_screen as wire;
    let (mut descriptor,_,_,_)=materials();
    let template=crate::data_pipeline::tests::fixture().0.nodes.into_iter().find_map(|n|n.selected).unwrap();
    let mut op=serde_json::to_value(template.operation).unwrap();
    op["id"]=json!("screen");op["capability"]=json!("test.instrument.screen@1");op["protocol"]=json!("stdio");op["dependencies"]=json!([]);op["credentialGrantRequired"]=json!(false);op["accountBindingRequired"]=json!(false);
    op["traits"]=json!({"modes":["paper"],"instrumentFamilies":["equity"],"dataShapes":["instrument_set"],"operations":["read"],"fields":[],"inputSchemaRefs":[wire::SCREEN_REQUEST_SCHEMA,wire::SCREEN_INPUT_SCHEMA],"outputSchemaRefs":[wire::SCREEN_RESULT_SCHEMA],"timeframes":[],"deterministic":true,"replayable":true});
    let manifest=json!({"metadata":{"id":"test.instrument-screen","version":"1","provider":{"id":"test.instrument-screen","name":"Local screening fixture"}},"operations":[op],"capabilities":[{"id":"test.instrument.screen@1"}],"permissions":[],"configuration":{"fields":[]},"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let schema_doc=|id:tradeassembly_plugin_sdk::data_contract::SchemaIdentity|json!({"schema":id,"shape":"table","time":{"intervalMs":null,"completedOnly":true}});
    let contract=json!({"schemaVersion":"tradeassembly.operation_data.v2","operationId":"screen","semanticsVersion":"fixture.screen.v1","inputs":{"contracts":{"data":schema_doc(wire::input_identity()),"required":true,"dependencyId":"contracts"}},"outputs":{"instruments":schema_doc(wire::result_identity())},"parameterSchema":{"type":"object","properties":{"minDte":{"type":"integer","minimum":0},"maxDte":{"type":"integer","minimum":0},"maxOiAgeDays":{"type":"integer","minimum":0},"fault":{"enum":["omit_rejections","foreign_member","undeclared_context"]}},"required":["minDte","maxDte","maxOiAgeDays"],"additionalProperties":false},"applicability":{"assetClasses":["equity"],"instrumentFamilies":["equity"],"modes":["paper"]},"readiness":"all_required_inputs","warmup":{"minimumObservations":1,"observationsParameter":null,"additionalObservations":0},"state":null});
    let manifest_bytes=serde_json::to_vec(&manifest).unwrap();let contract_bytes=serde_json::to_vec(&contract).unwrap();let binary=fixture_executable();
    descriptor["package"]["plugin"]["id"]=json!("test.instrument-screen");descriptor["package"]["manifest"]["sha256"]=json!(sha256(&manifest_bytes));descriptor["package"]["targets"][0]["binary"]["sha256"]=json!(sha256(binary));
    let docs=[("request.json",wire::request_identity(),wire::request_schema()),("input.json",wire::input_identity(),wire::input_schema()),("result.json",wire::result_identity(),wire::result_schema())];
    descriptor["dataSchemas"]=json!(docs.iter().map(|(path,id,v)|json!({"schemaRef":id.schema_ref,"document":{"path":path,"sha256":sha256(&serde_json::to_vec(v).unwrap())},"canonicalSha256":id.sha256})).collect::<Vec<_>>());
    descriptor["package"]["responseSchemas"]=json!([{"schemaRef":wire::SCREEN_RESULT_SCHEMA,"document":{"path":"result.json","sha256":sha256(&serde_json::to_vec(&wire::result_schema()).unwrap())}}]);
    descriptor["dataContracts"]=json!([{"operationId":"screen","document":{"path":"data.json","sha256":sha256(&contract_bytes)},"canonicalSha256":canonical_sha256(&contract).unwrap()}]);
    let mut files=vec![("manifest.json",manifest_bytes),("data.json",contract_bytes),("bin/plugin",binary.to_vec())];
    for (path,_,value) in &docs {files.push((path,serde_json::to_vec(value).unwrap()));}
    files.push(("tradeassembly-plugin.json",serde_json::to_vec(&descriptor).unwrap()));
    let archive=archive_files(files);let path=root.join("screen-package.tgz");fs::write(&path,&archive).unwrap();
    let package=service.runtime().plugin_packages.install(&PluginPackageInstallRequest{source_type:"file".into(),locator:path.to_string_lossy().into(),package_sha256:sha256(&archive),manifest_sha256:canonical_sha256(&manifest).unwrap(),offline:true}).unwrap();
    service.runtime().plugins.install_manifest(&package.plugin_ref,json!({"pluginRef":package.plugin_ref,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx("screen-install")).unwrap();package
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
fn rig_with_options(options: bool) -> Rig { rig_configuration(options,false) }
fn rig_configuration(options:bool,recipes:bool)->Rig {
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
        let package = install_fixture_with_recipe(
            &service,
            dir.path(),
            calculator,
            if options { 60000 } else { 86400000 },
            recipes && calculator,
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
fn instrument_screen_real_process_strategy_bound_selection_restart_and_expiry() { screen_process_fixture("selected");screen_process_fixture("integral"); }
#[test]
fn instrument_screen_process_no_match_incomplete_foreign_and_unaccounted_cases() {
    for case in ["no_match","incomplete","foreign","unaccounted","undeclared_context"] {screen_process_fixture(case);}
}
#[test]
fn instrument_screen_atomic_commit_failure_reuses_response_without_new_invocation() {screen_process_fixture("atomic");}
fn write_screen_interface_evidence(case:&str,name:&str,value:&Value) {
    let Some(root)=std::env::var_os("SCREEN_INTERFACE_EVIDENCE_DIR") else {return;};
    let directory=std::path::PathBuf::from(root).join(case);fs::create_dir_all(&directory).unwrap();
    let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(directory.join(name)).unwrap();
    file.write_all(&serde_json::to_vec_pretty(value).unwrap()).unwrap();file.write_all(b"\n").unwrap();
}
fn screen_process_fixture(case:&str) {
    use crate::service::{data_collections as collections,instrument_screens as screens};
    use tradeassembly_plugin_sdk::instrument_screen as wire;
    let operations=Arc::new(ForwardNative::default());
    let (dir,service,original_scope,_)=data_plans::tests::fixture_service(Some(operations.clone()));
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
    let complete=case!="incomplete";
    let server=std::thread::spawn(move|| {
        let (mut stream,_)=listener.accept().unwrap();let mut buffer=[0;8192];let n=stream.read(&mut buffer).unwrap();assert!(String::from_utf8_lossy(&buffer[..n]).starts_with("GET /observations "));
        let rows=json!([
            {"id":"B","dte":45,"oi":10,"oiAgeDays":1},
            {"id":"A","dte":60,"oi":10,"oiAgeDays":0},
            {"id":"ZERO","dte":50,"oi":0,"oiAgeDays":1},
            {"id":"MISSING","dte":50,"oi":null,"oiAgeDays":null},
            {"id":"EARLY","dte":44,"oi":100,"oiAgeDays":0},
            {"id":"LATE","dte":61,"oi":100,"oiAgeDays":0},
            {"id":"STALE","dte":50,"oi":100,"oiAgeDays":2}]);
        let bytes=serde_json::to_vec(&json!({"rows":rows,"coverage":{"complete":complete,"scope":"fixture_candidates"}})).unwrap();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();stream.write_all(&bytes).unwrap();
    });
    let source=install_fixture_variant(&service,dir.path(),false,86400000,false,true);
    let calculator=install_screen_package(&service,dir.path());
    for package in [&source,&calculator] {
        let mut instance=json!({"instanceRef":package.plugin_ref,"pluginRef":package.plugin_ref,"providerRef":package.plugin_ref,"enabled":true,"activePackageSha256":package.package_sha256,"configuration":if package.plugin_ref==source.plugin_ref {json!({"dataApiUrl":format!("http://{address}")})}else{json!({})}});
        if package.plugin_ref==source.plugin_ref {instance["credentialRef"]=json!("screen-fixture-source");}
        service.runtime().plugins.put_instance(&package.plugin_ref,instance,&ctx("screen-instance")).unwrap();
    }
    service.runtime().credentials.store("screen-fixture-source",&BTreeMap::from([("api_key".into(),"fake-screen-source-key".into()),("api_secret".into(),"fake-screen-source-secret".into())])).unwrap();
    *operations.0.lock().unwrap()=Some(crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(service.runtime().storage.clone(),service.runtime().plugins.clone(),service.runtime().credentials.clone(),service.runtime().plugin_packages.clone(),service.runtime().clock.clone(),Arc::new(TracedFixtureSandbox)));
    let mut spec:Value=serde_json::from_str(include_str!("../../../examples/strategy-spec/v3/valid/static-equity.json")).unwrap();spec["strategy_id"]=json!(original_scope.strategy_id);
    let mut screen_policy=json!({"version":"tradeassembly.instrument_screen.v1","capability_requirement_ref":"screen","semantics_version":"fixture.screen.v1","inputs":{"contracts":{"capability_requirement_ref":"screen_source","response_schema_ref":"schema://capture-fixture/response@1","query":{"literal":{},"date_bindings":{}},"max_age_ms":300000,"coverage":{"complete_pointer":"/snapshot/coverage/complete","scope_pointer":"/snapshot/coverage/scope","scope_value":"fixture_candidates"}}},"candidate_input":"contracts","candidate_records_pointer":"/snapshot/rows","candidate_id_pointer":"/id","parameters":{"minDte":45,"maxDte":60,"maxOiAgeDays":1},"coverage":"complete_configured_inputs","max_candidates":7,"top_n":1,"ttl_ms":120000});
    if case=="integral" {screen_policy["top_n"]=json!(1.0);}
    if case=="no_match" {screen_policy["parameters"]["minDte"]=json!(100);screen_policy["parameters"]["maxDte"]=json!(200);}
    if case=="foreign" {screen_policy["parameters"]["fault"]=json!("foreign_member");}
    if case=="unaccounted" {screen_policy["parameters"]["fault"]=json!("omit_rejections");}
    if case=="undeclared_context" {screen_policy["parameters"]["fault"]=json!("undeclared_context");}
    spec["signal_market_requirements"][0]["selectors"].as_array_mut().unwrap().push(json!({"selector_id":"screened","type":"screened","screen":screen_policy}));
    for (id,capability,schemas,shapes,deps,deterministic) in [
        ("screen_source","test.screen.source@1",vec!["schema://capture-fixture/response@1"],vec![],vec![],false),
        ("screen","test.instrument.screen@1",vec![wire::SCREEN_REQUEST_SCHEMA,wire::SCREEN_RESULT_SCHEMA],vec!["instrument_set"],vec!["screen_source"],true),
    ] {
        let mut requirement=spec["capability_requirements"]["required"][0].clone();
        requirement["requirement_id"]=json!(id);requirement["capability"]=json!(capability);requirement["stage_refs"]=json!(["universe"]);requirement["substep_refs"]=json!([]);requirement["dependency_refs"]=json!(deps);
        requirement["constraints"]["schema_refs"]=json!(schemas);requirement["constraints"]["data_shapes"]=json!(shapes);requirement["constraints"]["fields"]=json!([]);requirement["constraints"]["input_paths"]=json!([]);requirement["constraints"]["operations"]=json!([]);requirement["constraints"]["deterministic"]=json!(deterministic);requirement["constraints"]["replayable"]=json!(deterministic);requirement["constraints"].as_object_mut().unwrap().remove("timeframe");requirement["constraints"].as_object_mut().unwrap().remove("max_freshness");
        spec["capability_requirements"]["required"].as_array_mut().unwrap().push(requirement);
        service.runtime().storage.put_json("entitlement_grants",capability,json!({"grantId":capability,"profile":"local_full","source":"test","capability":capability,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("screen-grant")).unwrap();
    }
    let report=crate::spec::validate_strategy_spec_report(&spec);assert!(report.valid,"{:?}",report.diagnostics);
    assert!(crate::strategy_kernel::CompiledStrategy::compile_json(&spec).is_err());
    let saved=service.handle_http("POST","/product/strategies/save-draft",json!({"strategyId":original_scope.strategy_id,"spec":spec}));assert_eq!(saved.body["body"]["draftSaved"],true,"{}",saved.body);
    let published=service.handle_http("POST","/product/strategies/publish",json!({"strategyId":original_scope.strategy_id,"expectedDraftHash":saved.body["body"]["draft"]["draftHash"],"actor":{"kind":"user","id":"user.local"}}));assert_eq!(published.body["body"]["published"],true,"{}",published.body);
    let version=&published.body["body"]["version"];let scope=data_plans::PlanScope{strategy_id:original_scope.strategy_id,strategy_version_id:version["id"].as_str().unwrap().into(),strategy_spec_hash:version["specHash"].as_str().unwrap().into()};
    let graph=service.handle_http("POST","/capability-graph-revisions",json!({"configId":"screen-graph","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"},"screen_source":{"pluginInstanceRef":source.plugin_ref,"pluginRef":source.plugin_ref,"operationId":"acquire"},"screen":{"pluginInstanceRef":calculator.plugin_ref,"pluginRef":calculator.plugin_ref,"operationId":"screen"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper"}));assert_eq!(graph.body["revision"]["graph"]["ok"],true,"{}",graph.body);
    let revision=graph.body["revision"]["revisionId"].as_str().unwrap().to_string();
    let collection=collections::acquire(&service,collections::CollectionCommand{scope:scope.clone(),capability_graph_revision:revision.clone(),acquisition_node:"screen_source".into(),mode:"paper".into(),response_schema_ref:"schema://capture-fixture/response@1".into(),input:json!({})},&ctx("screen-source-acquire")).unwrap();server.join().unwrap();
    let command=screens::ScreenCommand{scope,capability_graph_revision:revision,mode:"paper".into(),signal_requirement_ref:"signal_primary".into(),selector_ref:"screened".into(),collections:BTreeMap::from([("contracts".into(),collection.collection_id)]),knowledge_cutoff_ms:service.runtime().clock.now_ms()};
    let body=json!({"screen":command,"idempotencyKey":"screen-select"});
    write_screen_interface_evidence(case,"request.json",&body);

    let mut wrong_mode=body.clone();wrong_mode["accountMode"]=json!("live");
    assert_eq!(service.handle_http("POST","/product/strategies/instrument-screens/create",wrong_mode).status,409);
    if case=="atomic" {
        rusqlite::Connection::open(dir.path().join("service.db")).unwrap().execute_batch("CREATE TRIGGER fail_screen BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='instrument_screen_receipts_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    }
    if matches!(case,"incomplete"|"foreign"|"unaccounted"|"undeclared_context"|"atomic") {
        let before=service.runtime().storage.list_json("plugin_operation_receipts").unwrap().len();
        let response=service.handle_http("POST","/product/strategies/instrument-screens/create",body.clone());
        write_screen_interface_evidence(case,"failure-http.json",&json!({"status":response.status,"body":response.body}));
        assert_eq!(response.status,409,"{}",response.body);
        if case!="atomic" {assert_eq!(response.body["error"]["code"],if case=="incomplete" {"instrument_screen_input_incomplete"}else if case=="undeclared_context" {"instrument_screen_context_undeclared"}else{"instrument_screen_candidate_accounting_invalid"});}
        assert_eq!(service.call_mcp_tool("tradeassembly.strategy.instrument_screen.create",body.clone())["isError"],true);
        if case=="incomplete" {assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap().len(),before);}
        for namespace in ["instrument_screen_receipts_v1","instrument_screen_keys_v1"] {assert!(service.runtime().storage.list_json(namespace).unwrap().is_empty());}
        if case=="atomic" {
            rusqlite::Connection::open(dir.path().join("service.db")).unwrap().execute_batch("DROP TRIGGER fail_screen;").unwrap();
            let operations_before=service.runtime().storage.list_json("plugin_operation_receipts").unwrap();
            let authority=crate::control_plane::http_envelope("http","POST","/product/strategies/instrument-screens/create",&body).unwrap().authority;
            let recovered=screens::acquire(&service,command,&SideEffectContext::new(authority,IdempotencyKey::new("screen-select").unwrap())).unwrap();
            assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap(),operations_before);
            assert_eq!(recovered.instrument_set.known_at_ms,recovered.operation.response.observed_at_ms);
            assert_eq!(recovered.instrument_set.expires_at_ms,recovered.operation.response.observed_at_ms+120000);
        }
        return;
    }
    let response=service.handle_http("POST","/product/strategies/instrument-screens/create",body.clone());
    assert_eq!(response.status,200,"{}",response.body);assert_eq!(response.body["executionAuthorityGranted"],false);
    write_screen_interface_evidence(case,"response.json",&response.body);
    write_screen_interface_evidence(case,"discovery.json",&crate::data_strategy::discovery());
    write_screen_interface_evidence(case,"strategy-schema.json",&crate::spec::strategy_spec_schema());
    write_screen_interface_evidence(case,"screen-request-schema.json",&wire::request_schema());
    write_screen_interface_evidence(case,"screen-input-schema.json",&wire::input_schema());
    write_screen_interface_evidence(case,"screen-result-schema.json",&wire::result_schema());

    assert!(response.body["selection"].get("response").is_none());
    let selection=screens::current(&service,response.body["selection"]["selectionId"].as_str().unwrap()).unwrap();
    assert_eq!(response.body["instrumentSet"],serde_json::to_value(&selection.instrument_set).unwrap());
    if case!="no_match" {
    assert_eq!(selection.instrument_set.members[0].instrument_id,"A");assert_eq!(selection.instrument_set.candidate_count,7);assert_eq!(selection.instrument_set.members[0].context_instruments,["SPY"]);
    assert!(selection.instrument_set.rejection_summary.iter().any(|r|r.reason_code=="below_rank_cutoff" && r.count==2));
    assert!(selection.instrument_set.rejection_summary.iter().any(|r|r.reason_code=="missing_open_interest"));
    assert!(selection.instrument_set.rejection_summary.iter().any(|r|r.reason_code=="expiry_out_of_range" && r.count==2));
    assert!(selection.instrument_set.rejection_summary.iter().any(|r|r.reason_code=="stale_open_interest"));
    } else {
        assert_eq!(selection.instrument_set.status,tradeassembly_plugin_sdk::instrument_set::SelectionStatus::CompleteEmpty);
        assert!(selection.instrument_set.members.is_empty());
        assert_eq!(selection.instrument_set.candidate_count,7);
        assert!(selection.instrument_set.rejection_summary.iter().any(|r|r.reason_code=="expiry_out_of_range" && r.count==7));
    }
    let duplicate=service.handle_http("POST","/product/strategies/instrument-screens/create",body.clone());
    assert_eq!(duplicate.body["selection"],response.body["selection"]);assert_eq!(duplicate.body["instrumentSet"],response.body["instrumentSet"]);
    let mcp=service.call_mcp_tool("tradeassembly.strategy.instrument_screen.get",json!({"selectionId":selection.selection_id}));assert_eq!(mcp["structuredContent"]["instrumentSet"],response.body["instrumentSet"]);
    write_screen_interface_evidence(case,"get-mcp.json",&mcp);

    assert_eq!(service.call_mcp_tool("tradeassembly.strategy.instrument_screen.get",json!({"selectionId":"missing"}))["isError"],true);
    assert!(!serde_json::to_string(&selection).unwrap().contains("fake-screen-source"));
    let db=dir.path().join("service.db").to_string_lossy().into_owned();drop(service);drop(operations);
    let mut runtime=crate::adapters::local::test_runtime(&db);runtime.plugin_packages=Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(dir.path().join("package-store")).unwrap());
    let shift=Arc::new(std::sync::atomic::AtomicI64::new(0));runtime.clock=Arc::new(AdvancedClock{base:runtime.clock.clone(),advance:shift.clone()});
    let reopened=crate::service::TradeAssemblyService::from_runtime(&db,runtime);
    assert_eq!(screens::current(&reopened,&selection.selection_id).unwrap(),selection);
    let duplicate=reopened.handle_http("POST","/product/strategies/instrument-screens/create",body.clone());
    assert_eq!(duplicate.body["selection"],response.body["selection"]);assert_eq!(duplicate.body["instrumentSet"],response.body["instrumentSet"]);
    let original=reopened.runtime().storage.get_json("instrument_screen_receipts_v1",&selection.selection_id).unwrap().unwrap();
    let operation_count=reopened.runtime().storage.list_json("plugin_operation_receipts").unwrap().len();
    shift.store(120001,std::sync::atomic::Ordering::SeqCst);assert_eq!(screens::current(&reopened,&selection.selection_id).unwrap_err(),"instrument_screen_stale");
    let expired_http=reopened.handle_http("POST","/product/strategies/instrument-screens/create",body.clone());
    assert_eq!(expired_http.body["error"]["code"],"instrument_screen_stale");
    let expired_mcp=reopened.call_mcp_tool("tradeassembly.strategy.instrument_screen.create",body.clone());assert_eq!(expired_mcp["isError"],true);
    write_screen_interface_evidence(case,"expired-http.json",&json!({"status":expired_http.status,"body":expired_http.body}));
    write_screen_interface_evidence(case,"expired-mcp.json",&expired_mcp);
    assert_eq!(reopened.runtime().storage.list_json("plugin_operation_receipts").unwrap().len(),operation_count);
    assert_eq!(reopened.runtime().storage.get_json("instrument_screen_receipts_v1",&selection.selection_id).unwrap().unwrap(),original);
    shift.store(0,std::sync::atomic::Ordering::SeqCst);
    let alice=reopened.for_authenticated_invocation("test","alice",None,None);alice.bind_owned_object("strategy",&selection.command.scope.strategy_id).unwrap();
    for instance in reopened.runtime().plugins.list_instances().unwrap() {alice.bind_owned_object("plugin_instance",instance["instanceRef"].as_str().unwrap()).unwrap();}
    let bob=reopened.for_authenticated_invocation("test","bob",None,None);
    assert_eq!(bob.call_mcp_tool("tradeassembly.strategy.instrument_screen.get",json!({"selectionId":selection.selection_id}))["isError"],true);
    let mut instance=reopened.runtime().plugins.get_instance(&calculator.plugin_ref).unwrap().unwrap();instance["enabled"]=json!(false);reopened.runtime().plugins.put_instance(&calculator.plugin_ref,instance,&ctx("screen-revoke")).unwrap();
    assert_ne!(reopened.handle_http("POST","/product/strategies/instrument-screens/create",body).status,200);

}

#[test]
fn collection_real_process_admission_idempotency_recreation_and_revocation() {
    use crate::service::data_collections as collections;
    let r = rig();
    let command = collections::CollectionCommand {
        scope: r.command.policy.scope.clone(),
        capability_graph_revision: r.command.policy.capability_graph_revision.clone(),
        acquisition_node: "acquire".into(), mode: "paper".into(),
        response_schema_ref: "schema://capture-fixture/response@1".into(), input: json!({}),
    };
    let body=json!({"collection":command,"idempotencyKey":"collection-process"});
    let response=r.service.handle_http("POST","/product/strategies/data-collections/create",body.clone());
    assert_eq!(response.status,200,"{}",response.body);
    assert_eq!(response.body["executionAuthorityGranted"],false);
    let receipt=collections::current(&r.service,response.body["collection"]["collectionId"].as_str().unwrap()).unwrap();
    assert_eq!(receipt.response.payload["snapshot"]["values"],json!([10,20,30]));
    assert!(receipt.host_known_at_ms>=receipt.response.observed_at_ms);
    assert!(!serde_json::to_string(&receipt).unwrap().contains("fake-capture-canary"));
    assert_eq!(r.service.handle_http("POST","/product/strategies/data-collections/create",body.clone()).body["collection"],response.body["collection"]);
    let read=r.service.call_mcp_tool("tradeassembly.strategy.data_collection.get",json!({"collectionId":receipt.collection_id}));
    assert_eq!(read["structuredContent"]["collection"],response.body["collection"],"{}",read);
    assert_eq!(r.service.call_mcp_tool("tradeassembly.strategy.data_collection.get",json!({"collectionId":"missing"}))["isError"],true);
    let mut changed=body.clone(); changed["collection"]["input"]=json!({"changed":true});
    assert_eq!(r.service.handle_http("POST","/product/strategies/data-collections/create",changed).status,409);
    let mut wrong_mode=body.clone(); wrong_mode["accountMode"]=json!("live");
    assert_eq!(r.service.handle_http("POST","/product/strategies/data-collections/create",wrong_mode).status,409);
    r.server.join().unwrap();
    let db=r.dir.path().join("service.db").to_string_lossy().into_owned();
    drop(r.service); drop(r.operations);
    let mut runtime=crate::adapters::local::test_runtime(&db);
    runtime.plugin_packages=Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(r.dir.path().join("package-store")).unwrap());
    // No operation invoker is restored: reopening/retrying must use original
    // evidence and must not silently reacquire another provider snapshot.
    let reopened=crate::service::TradeAssemblyService::from_runtime(&db,runtime);
    assert_eq!(collections::current(&reopened,&receipt.collection_id).unwrap(),receipt);
    assert_eq!(reopened.handle_http("POST","/product/strategies/data-collections/create",body.clone()).body["collection"],response.body["collection"]);
    let alice=reopened.for_authenticated_invocation("test","alice",None,None);
    alice.bind_owned_object("strategy",&receipt.command.scope.strategy_id).unwrap();
    for instance in reopened.runtime().plugins.list_instances().unwrap() {
        alice.bind_owned_object("plugin_instance",instance["instanceRef"].as_str().unwrap()).unwrap();
    }
    let bob=reopened.for_authenticated_invocation("test","bob",None,None);
    assert_eq!(bob.call_mcp_tool("tradeassembly.strategy.data_collection.get",json!({"collectionId":receipt.collection_id}))["isError"],true);
    let storage=reopened.runtime().storage.clone();
    let mut forged=serde_json::to_value(&receipt).unwrap(); forged["response"]["payload"]["snapshot"]["values"]=json!([999]);
    storage.put_json("data_collection_receipts_v1",&receipt.collection_id,forged,&ctx("tamper")).unwrap();
    assert_eq!(collections::current(&reopened,&receipt.collection_id).unwrap_err(),"data_collection_receipt_invalid");
    storage.put_json("data_collection_receipts_v1",&receipt.collection_id,serde_json::to_value(&receipt).unwrap(),&ctx("restore")).unwrap();
    let mut instance=reopened.runtime().plugins.get_instance(&receipt.acquisition.plugin_instance_ref).unwrap().unwrap(); instance["enabled"]=json!(false);
    reopened.runtime().plugins.put_instance(&receipt.acquisition.plugin_instance_ref,instance,&ctx("revoke")).unwrap();
    assert!(collections::current(&reopened,&receipt.collection_id).is_err());
    assert_eq!(reopened.call_mcp_tool("tradeassembly.strategy.data_collection.get",json!({"collectionId":receipt.collection_id}))["isError"],true);
    assert_ne!(reopened.handle_http("POST","/product/strategies/data-collections/create",body).status,200);
}

#[test]
fn collection_process_atomic_failure_leaves_no_receipt_or_key() {
    let r=rig();
    let connection=rusqlite::Connection::open(r.dir.path().join("service.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_collection BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='data_collection_receipts_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    let response=r.service.handle_http("POST","/product/strategies/data-collections/create",json!({"collection":{"scope":r.command.policy.scope,"capabilityGraphRevision":r.command.policy.capability_graph_revision,"acquisitionNode":"acquire","mode":"paper","responseSchemaRef":"schema://capture-fixture/response@1","input":{}},"idempotencyKey":"collection-atomic"}));
    assert_eq!(response.status,409,"{}",response.body);
    for namespace in ["data_collection_receipts_v1","data_collection_keys_v1"] { assert!(r.service.runtime().storage.list_json(namespace).unwrap().is_empty()); }
    r.server.join().unwrap();
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
        selected_outputs: vec![],
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
    runtime.plugin_packages = Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(r.dir.path().join("package-store")).unwrap());
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
    external_provider_admission_fixture("DATA_CAPTURE_PROVIDER_CONFIG",false);
}
#[test]
#[ignore = "requires explicitly pinned DATA_COLLECTION_PROVIDER_CONFIG; loopback mock and fake credentials only"]
fn collection_external_provider_package_admits_reopens_and_preserves_evidence_without_live_services() {
    external_provider_admission_fixture("DATA_COLLECTION_PROVIDER_CONFIG",true);
}
fn external_provider_admission_fixture(config_env:&str,collection:bool) {
    let config: Value = serde_json::from_slice(
        &fs::read(
            std::env::var_os(config_env)
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
            if let Some(expected)=config["manifestDocumentSha256"].as_str() { assert_eq!(sha256(&bytes),expected); }
            serde_yaml::from_slice(&bytes).unwrap()
        })
        .unwrap();
    if let Some(expected)=config["manifestSha256"].as_str() { assert_eq!(canonical_sha256(&manifest).unwrap(),expected); }
    assert!(config["manifestSha256"].is_string() || config["manifestDocumentSha256"].is_string());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let responses=config["httpResponses"].as_array().cloned().unwrap_or_else(||vec![config["httpResponse"].clone()]);
    let query_fragments=config["httpQueryFragments"].as_array().cloned().unwrap_or_default();
    let prefix = config["httpRequestPrefix"].as_str().unwrap().to_string();
    let server = std::thread::spawn(move || {
      for (index,response) in responses.iter().enumerate() {
        let mock_body=serde_json::to_vec(response).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 8192];
        let n = stream.read(&mut buf).unwrap();
        let request=String::from_utf8_lossy(&buf[..n]);
        assert!(request.starts_with(&prefix));
        for fragment in &query_fragments { assert!(request.lines().next().unwrap().contains(fragment.as_str().unwrap())); }
        if index>0 { assert!(request.lines().next().unwrap().contains("page_token=")); }
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",mock_body.len()).unwrap();
        stream.write_all(&mock_body).unwrap();
      }
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
        if collection && node=="source" {continue;}
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
    if collection {
        use crate::service::data_collections as collections;
        let command=collections::CollectionCommand {scope,capability_graph_revision:graph.body["revision"]["revisionId"].as_str().unwrap().into(),acquisition_node:"acquire".into(),mode:"paper".into(),response_schema_ref:config["responseSchemaRef"].as_str().unwrap().into(),input:config["input"].clone()};
        let body=json!({"collection":command,"idempotencyKey":"external-native-collection"});
        let response=service.handle_http("POST","/product/strategies/data-collections/create",body.clone());
        assert_eq!(response.status,200,"{}",response.body);
        let receipt=collections::current(&service,response.body["collection"]["collectionId"].as_str().unwrap()).unwrap();
        assert_eq!(receipt.acquisition.package_sha256,sha256(&archive));
        assert!(response.body["collection"].get("response").is_none());
        assert!(response.body["collection"].get("input").is_none());
        assert!(!serde_json::to_string(&response.body).unwrap().contains("nextPageToken"));
        for secret in fields.values() { assert!(!serde_json::to_string(&receipt).unwrap().contains(secret)); }
        for (pointer,expected) in config["expectedPayload"].as_object().unwrap() { assert_eq!(receipt.response.payload.pointer(pointer),Some(expected)); }
        assert_eq!(service.handle_http("POST","/product/strategies/data-collections/create",body.clone()).body["collection"],response.body["collection"]);
        server.join().unwrap();
        let db=dir.path().join("service.db").to_string_lossy().into_owned(); drop(service);drop(operations);
        let mut runtime=crate::adapters::local::test_runtime(&db);
        runtime.plugin_packages=Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(dir.path().join("package-store")).unwrap());
        let reopened=crate::service::TradeAssemblyService::from_runtime(&db,runtime);
        assert_eq!(collections::current(&reopened,&receipt.collection_id).unwrap(),receipt);
        assert_eq!(reopened.handle_http("POST","/product/strategies/data-collections/create",body).body["collection"],response.body["collection"]);
        return;
    }
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
            selected_outputs: vec![],
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
    runtime.plugin_packages = Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(dir.path().join("package-store")).unwrap());
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
    runtime.plugin_packages = Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(r.dir.path().join("package-store")).unwrap());
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

#[test]
fn recipe_real_process_public_preparation_evaluation_recreation_and_tamper() {
    use crate::service::data_recipes::*;
    let r=rig_configuration(false,true);
    let capture=r.service.handle_http("POST","/product/strategies/data-captures/create",json!({"capture":r.command,"idempotencyKey":"recipe-capture"}));
    assert_eq!(capture.status,200,"{}",capture.body);
    let receipt:captures::CaptureReceipt=serde_json::from_value(capture.body["capture"].clone()).unwrap();
    let package_sha=r.service.runtime().plugins.get_instance("capture.calculation").unwrap().unwrap()["activePackageSha256"].as_str().unwrap().to_owned();
    let program=RecipeProgram {
        schema_version:RecipeProgramVersion::V1,
        base_request:PipelineRequest {selected_outputs:vec![],source_captures:BTreeMap::from([("source".into(),receipt.capture_id.clone())]),capability_graph_revision:r.command.policy.capability_graph_revision.clone(),mode:"paper".into(),trade_instruments:vec![],nodes:vec![PipelineNode {graph_node_id:"source".into(),parameters:json!({}),context:r.command.policy.context.clone(),input_contexts:BTreeMap::new()}],bindings:vec![]},
        instances:vec![RecipeInvocation {instance_id:"projection".into(),plugin_instance_ref:"capture.calculation".into(),package_sha256:package_sha.clone(),recipe_id:"source-projection".into(),semantics_version:"1".into(),parameters:json!({}),nodes:BTreeMap::from([("projection".into(),RecipeNodeBinding {graph_node_id:"derived".into(),context:r.command.policy.context.clone(),input_contexts:BTreeMap::new()})]),inputs:BTreeMap::from([("source".into(),RecipeInput::Pipeline {node:"source".into(),port:"value".into()})]),selected_outputs:vec!["value".into()]}],
        outputs:vec![RecipeInput::Recipe {instance:"projection".into(),output:"value".into()}]
    };
    let body=json!({"scope":r.command.policy.scope,"program":program,"idempotencyKey":"public-recipe"});
    let prepared=r.service.handle_http("POST","/product/strategies/data-recipes/prepare",body.clone());
    assert_eq!(prepared.status,200,"{}",prepared.body);
    assert_eq!(prepared.body["executionAuthorityGranted"],false);
    let plan_count=r.service.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap().len();
    for case in ["parameter","cycle","context","mode"] {
        let mut invalid=body.clone(); invalid["idempotencyKey"]=json!(format!("recipe-invalid-{case}"));
        match case {
            "parameter"=>invalid["program"]["instances"][0]["parameters"]=json!({"unexpected":true}),
            "cycle"=>invalid["program"]["instances"][0]["inputs"]["source"]=json!({"kind":"recipe","instance":"projection","output":"value"}),
            "mode"=>invalid["program"]["baseRequest"]["mode"]=json!("live"),
            _=>invalid["program"]["instances"][0]["nodes"]["projection"]["context"]["instruments"][0]["instrumentFamily"]=json!("crypto_spot"),
        }
        let error=r.service.handle_http("POST","/product/strategies/data-recipes/prepare",invalid.clone());
        assert_eq!(error.status,409,"{case}: {}",error.body);
        let expected=match case {"parameter"=>"data_recipe_parameters_invalid","cycle"=>"data_recipe_instance_cycle","mode"=>"data_plan_graph_stale",_=>"data_recipe_plan_compile_failed"};
        assert_eq!(error.body["error"]["code"],expected,"{case}: {}",error.body);
        let mcp=r.service.call_mcp_tool("tradeassembly.strategy.data_recipe.prepare",invalid);
        assert_eq!(mcp["isError"],true,"{case}: {mcp}");
        if case=="context" {assert!(error.body["diagnostics"].as_array().is_some_and(|rows|!rows.is_empty()));}
        assert_eq!(r.service.runtime().storage.list_json("compiled_data_plan_revisions_v1").unwrap().len(),plan_count);
    }
    let plan:data_plans::StoredDataPlan=serde_json::from_value(prepared.body["plan"].clone()).unwrap();
    assert_eq!(plan.content.plan.schema_version,DataPlanVersion::V5);
    assert_eq!(plan.content.plan.selected_source_history["source"],1);
    let source=plan.content.recipe_source.as_ref().unwrap();
    assert_eq!(source.pins.len(),1);
    assert_ne!(source.pins[0].recipe_document_sha256,source.pins[0].recipe_canonical_sha256);
    let step_body=json!({"step":data_evaluation::EvaluationStep {run_id:"recipe-run".into(),plan_revision_id:plan.revision_id.clone(),window:EvaluationWindow {event_start_ms:10*86400000,event_end_ms:13*86400000,as_of_ms:r.service.runtime().clock.now_ms()},expected_checkpoint_sha256:None},"idempotencyKey":"recipe-evaluate"});
    let response=r.service.handle_http("POST","/product/strategies/data-recipes/step",step_body.clone());
    assert_eq!(response.status,200,"{}",response.body);
    let evaluated:data_evaluation::EvaluationReceipt=serde_json::from_value(response.body["receipt"].clone()).unwrap();
    let values=evaluated.outputs["derived"].iter().flat_map(|response|response.outputs["value"].points.iter()).filter_map(|point|match &point.result {PointValue::Ready(value)=>value.as_i64(),_=>None}).collect::<Vec<_>>();
    assert_eq!(values,vec![10,20,30]);
    let db=r.dir.path().join("service.db").to_string_lossy().into_owned();
    drop(r.service);drop(r.operations);
    let mut runtime=crate::adapters::local::test_runtime(&db);
    runtime.plugin_packages=Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(r.dir.path().join("package-store")).unwrap());
    runtime.plugin_operations=Arc::new(crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(runtime.storage.clone(),runtime.plugins.clone(),runtime.credentials.clone(),runtime.plugin_packages.clone(),runtime.clock.clone(),Arc::new(NativeTestSandbox)));
    let reopened=crate::service::TradeAssemblyService::from_runtime(&db,runtime);
    assert_eq!(data_plans::current(&reopened,&plan.revision_id).unwrap(),plan);
    let retry=reopened.handle_http("POST","/product/strategies/data-recipes/step",step_body.clone());
    assert_eq!(retry.body["receipt"],response.body["receipt"]);
    let mut missing=step_body.clone(); missing["step"]["planRevisionId"]=json!("missing"); missing["idempotencyKey"]=json!("missing-recipe-step");
    assert_eq!(reopened.call_mcp_tool("tradeassembly.strategy.data_recipe.step",missing)["isError"],true);
    let mcp=reopened.call_mcp_tool("tradeassembly.strategy.data_recipe.prepare",body.clone());
    assert_ne!(mcp["isError"],true,"{mcp}");
    assert_eq!(mcp["structuredContent"]["plan"],prepared.body["plan"]);
    let saved=reopened.runtime().storage.list_json("data_evaluation_receipts_v1").unwrap();
    let package=reopened.runtime().plugin_packages.get(&package_sha).unwrap().unwrap();
    fs::write(package.install_root.join("recipe.json"),b"{}").unwrap();
    let rejected=reopened.call_mcp_tool("tradeassembly.strategy.data_recipe.prepare",body);
    assert_eq!(rejected["isError"],true,"{rejected}");
    assert_eq!(reopened.runtime().storage.list_json("data_evaluation_receipts_v1").unwrap(),saved);
    assert!(data_plans::current(&reopened,&plan.revision_id).is_err());
    let strategy_id=r.command.policy.scope.strategy_id.clone();
    let alice=reopened.for_authenticated_invocation("test","alice",None,None);
    alice.bind_owned_object("strategy",&strategy_id).unwrap();
    let bob=reopened.for_authenticated_invocation("test","bob",None,None);
    let foreign=bob.call_mcp_tool("tradeassembly.strategy.data_recipe.step",step_body);
    assert_eq!(foreign["isError"],true,"{foreign}");
    r.server.join().unwrap();
}

#[test]
fn session_calendar_real_process_public_plan_restart_expiry_and_closed_day(){
 for closed in [false,true]{session_calendar_process_fixture(closed);}
}
fn session_calendar_process_fixture(closed:bool){
 eprintln!("calendar fixture case closed={closed}: start");
 use crate::service::{data_collections as collections,session_schedules as schedules};
 use tradeassembly_plugin_sdk::session_calendar as wire;
 let operations=Arc::new(ForwardNative::default());
 let (dir,service,original_scope,_)=data_plans::tests::fixture_service(Some(operations.clone()));
 let now=service.runtime().clock.now_ms();
 let date=chrono::DateTime::from_timestamp_millis(now).unwrap().date_naive().to_string();
 let session=if closed{json!({"status":"closed","reason":"explicit_fixture_holiday"})}else{json!({"status":"open","openAtMs":now+600000,"closeAtMs":now+3600000,"earlyClose":true})};
 let document=json!({"schemaVersion":"tradeassembly.session_calendar.v1","calendarRef":"fixture.exchange","timeZone":"Etc/UTC","sessionScope":"fixture_primary","revision":"fixture.v1","rangeStart":date,"rangeEnd":date,"knownAtMs":now,"days":[{"sessionDate":date,"session":session}]});
 let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
 let server=std::thread::spawn(move||{let (mut stream,_)=listener.accept().unwrap();eprintln!("calendar fixture closed={closed}: accepted");let mut buffer=[0;8192];let n=stream.read(&mut buffer).unwrap();assert!(String::from_utf8_lossy(&buffer[..n]).starts_with("GET /options "));let bytes=serde_json::to_vec(&document).unwrap();write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();stream.write_all(&bytes).unwrap();stream.shutdown(std::net::Shutdown::Both).unwrap();eprintln!("calendar fixture closed={closed}: response sent");});
 let source=install_fixture_variant(&service,dir.path(),false,86400000,false,true);
 service.runtime().plugins.put_instance(&source.plugin_ref,json!({"instanceRef":source.plugin_ref,"pluginRef":source.plugin_ref,"providerRef":source.plugin_ref,"enabled":true,"activePackageSha256":source.package_sha256,"credentialRef":"calendar-fixture","configuration":{"dataApiUrl":format!("http://{address}")}}),&ctx("calendar-instance")).unwrap();
 service.runtime().credentials.store("calendar-fixture",&BTreeMap::from([("api_key".into(),"fake-calendar-key".into()),("api_secret".into(),"fake-calendar-secret".into())])).unwrap();
 *operations.0.lock().unwrap()=Some(crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(service.runtime().storage.clone(),service.runtime().plugins.clone(),service.runtime().credentials.clone(),service.runtime().plugin_packages.clone(),service.runtime().clock.clone(),Arc::new(TracedFixtureSandbox)));
 let mut spec:Value=serde_json::from_str(include_str!("../../../examples/strategy-spec/v3/valid/static-equity.json")).unwrap();spec["strategy_id"]=json!(original_scope.strategy_id);
 let selector=spec["signal_market_requirements"][0]["selectors"][0]["selector_id"].clone();
 let binding=json!({"capability_requirement_ref":"calendar","calendar_ref":"fixture.exchange","timezone":"Etc/UTC","session_scope":"fixture_primary","max_age_ms":86400000});
 spec["stages"]["market_clock"]["policy"]["session_triggers"]=json!([{"version":"tradeassembly.session_trigger.v1","trigger_id":"preopen","calendar":binding,"anchor":"open","offset_ms":-300000,"missed_run":{"policy":"catch_up","max_lateness_ms":60000},"retry":{"max_attempts":3,"backoff_ms":1000},"execution_window":{"calendar":binding,"open_offset_ms":0,"close_offset_ms":0},"selections":[{"signal_requirement_ref":"signal_primary","selector_ref":selector}]}]);
 let mut requirement=spec["capability_requirements"]["required"][0].clone();
 requirement["requirement_id"]=json!("calendar");requirement["capability"]=json!("test.session.calendar@1");requirement["stage_refs"]=json!(["market_clock"]);requirement["substep_refs"]=json!([]);requirement["dependency_refs"]=json!([]);
 for (key,value) in [("schema_refs",json!([wire::SESSION_CALENDAR_SCHEMA])),("data_shapes",json!([])),("fields",json!([])),("input_paths",json!([])),("operations",json!([])),("deterministic",json!(false)),("replayable",json!(false))]{requirement["constraints"][key]=value;}
 requirement["constraints"].as_object_mut().unwrap().remove("timeframe");requirement["constraints"].as_object_mut().unwrap().remove("max_freshness");
 spec["capability_requirements"]["required"].as_array_mut().unwrap().push(requirement);
 service.runtime().storage.put_json("entitlement_grants","test.session.calendar@1",json!({"grantId":"test.session.calendar@1","profile":"local_full","source":"test","capability":"test.session.calendar@1","effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("calendar-grant")).unwrap();
 let report=crate::spec::validate_strategy_spec_report(&spec);assert!(report.valid,"{:?}",report.diagnostics);
 let saved=service.handle_http("POST","/product/strategies/save-draft",json!({"strategyId":original_scope.strategy_id,"spec":spec}));assert_eq!(saved.body["body"]["draftSaved"],true,"{}",saved.body);
 let published=service.handle_http("POST","/product/strategies/publish",json!({"strategyId":original_scope.strategy_id,"expectedDraftHash":saved.body["body"]["draft"]["draftHash"],"actor":{"kind":"user","id":"user.local"}}));assert_eq!(published.body["body"]["published"],true,"{}",published.body);
 let version=&published.body["body"]["version"];let scope=data_plans::PlanScope{strategy_id:original_scope.strategy_id,strategy_version_id:version["id"].as_str().unwrap().into(),strategy_spec_hash:version["specHash"].as_str().unwrap().into()};
 let graph=service.handle_http("POST","/capability-graph-revisions",json!({"configId":"calendar-graph","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"},"calendar":{"pluginInstanceRef":source.plugin_ref,"pluginRef":source.plugin_ref,"operationId":"calendar"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper"}));assert_eq!(graph.body["revision"]["graph"]["ok"],true,"{}",graph.body);
 let revision=graph.body["revision"]["revisionId"].as_str().unwrap().to_string();
 eprintln!("calendar fixture closed={closed}: acquiring");
 let collection=collections::acquire(&service,collections::CollectionCommand{scope:scope.clone(),capability_graph_revision:revision.clone(),acquisition_node:"calendar".into(),mode:"paper".into(),response_schema_ref:wire::SESSION_CALENDAR_SCHEMA.into(),input:json!({})},&ctx("calendar-source")).unwrap();server.join().unwrap();eprintln!("calendar fixture closed={closed}: acquisition complete");
 let command=schedules::ScheduleCommand{scope,capability_graph_revision:revision,mode:"paper".into(),trigger_ref:"preopen".into(),session_date:date,calendar_collection_id:collection.collection_id.clone(),execution_calendar_collection_id:collection.collection_id,knowledge_cutoff_ms:service.runtime().clock.now_ms()};
 let body=json!({"schedule":command,"idempotencyKey":"calendar-plan"});
 let case=if closed{"calendar_closed"}else{"calendar_open"};write_screen_interface_evidence(case,"request.json",&body);
 let mut wrong=body.clone();wrong["accountMode"]=json!("live");assert_eq!(service.handle_http("POST","/product/strategies/session-schedules/prepare",wrong).status,409);
 if !closed {
  let connection=rusqlite::Connection::open(dir.path().join("service.db")).unwrap();connection.execute_batch("CREATE TRIGGER fail_session_plan BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='session_occurrence_plans_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
  let mut fault=body.clone();fault["idempotencyKey"]=json!("calendar-fault");let response=service.handle_http("POST","/product/strategies/session-schedules/prepare",fault.clone());assert_eq!(response.status,409,"{}",response.body);assert_eq!(response.body["error"]["code"],"session_schedule_commit_failed");assert_eq!(service.call_mcp_tool("tradeassembly.strategy.session_schedule.prepare",fault)["isError"],true);assert!(service.runtime().storage.list_json("session_occurrence_plans_v1").unwrap().is_empty());connection.execute_batch("DROP TRIGGER fail_session_plan;").unwrap();
 }
 let response=service.handle_http("POST","/product/strategies/session-schedules/prepare",body.clone());assert_eq!(response.status,200,"{}",response.body);assert_eq!(response.body["queued"],false);assert_eq!(response.body["executionAuthorityGranted"],false);
 write_screen_interface_evidence(case,"response.json",&response.body);write_screen_interface_evidence(case,"calendar-schema.json",&wire::schema());
 let occurrence=response.body["schedule"]["occurrence"]["occurrenceId"].as_str().unwrap();
 let receipt=schedules::current(&service,occurrence).unwrap();assert_eq!(receipt.occurrence.status,if closed{crate::session_schedule::SessionOccurrenceStatus::Closed}else{crate::session_schedule::SessionOccurrenceStatus::Scheduled});
 let retry=service.handle_http("POST","/product/strategies/session-schedules/prepare",body.clone());assert_eq!(retry.status,200,"{}",retry.body);assert_eq!(retry.body["schedule"],response.body["schedule"]);
 let mcp=service.call_mcp_tool("tradeassembly.strategy.session_schedule.get",json!({"occurrenceId":occurrence}));assert_ne!(mcp["isError"],true,"{mcp}");assert_eq!(mcp["structuredContent"]["schedule"],response.body["schedule"]);write_screen_interface_evidence(case,"get-mcp.json",&mcp);
 use crate::service::data_runner::{DataRunnerCommand,DataRunnerMode,DataRunnerOperation,DataRunnerState,DurableCoreDataRunner};
 let runner_command=DataRunnerCommand{schema_version:crate::service::data_runner::DATA_RUNNER_SCHEMA.into(),core_release:"fixture.release".into(),workspace_id:"fixture.workspace".into(),command_id:"fixture.command".into(),authority_ref:"provenance.only".into(),idempotency_key:IdempotencyKey::new("fixture.command").unwrap(),mode:DataRunnerMode::Paper,submitted_at_ms:service.runtime().clock.now_ms(),operation:DataRunnerOperation::PrepareSession{schedule:command.clone()}};
 let runner=DurableCoreDataRunner::new(service.clone(),"fixture.release".into()).unwrap();
 if !closed {
  let connection=rusqlite::Connection::open(dir.path().join("service.db")).unwrap();connection.execute_batch("CREATE TRIGGER fail_data_runner_complete BEFORE UPDATE ON tradeassembly_kv WHEN NEW.namespace='data_runner_receipts_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
  assert_eq!(runner.dispatch(&runner_command).unwrap_err(),"data_runner_complete_commit_failed");let accepted=runner.receipt("fixture.workspace","fixture.command").unwrap().unwrap();assert_eq!(accepted.state,DataRunnerState::Accepted);assert_eq!(service.runtime().storage.list_json("data_runner_receipt_revisions_v1").unwrap().len(),1);connection.execute_batch("DROP TRIGGER fail_data_runner_complete;").unwrap();
 }
 let runner_receipt=runner.dispatch(&runner_command).unwrap();assert_eq!(runner_receipt.state,DataRunnerState::Completed);assert!(!runner_receipt.execution_authority_granted);assert!(runner_receipt.previous_receipt_sha256.is_some());assert_eq!(runner.dispatch(&runner_command).unwrap(),runner_receipt);
 assert_eq!(service.runtime().storage.list_json("data_runner_receipt_revisions_v1").unwrap().len(),2);
 let mut changed=runner_command.clone();changed.authority_ref="changed.provenance".into();assert_eq!(runner.dispatch(&changed).unwrap_err(),"data_runner_idempotency_conflict");let mut changed=runner_command.clone();changed.command_id="different.command".into();assert_eq!(runner.dispatch(&changed).unwrap_err(),"data_runner_idempotency_conflict");
 if !closed {
  let connection=rusqlite::Connection::open(dir.path().join("service.db")).unwrap();connection.execute_batch("CREATE TRIGGER fail_upstream_complete BEFORE UPDATE ON tradeassembly_kv WHEN NEW.namespace='control_plane_commands' AND json_extract(NEW.value_json,'$.response_status') IS NOT NULL BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
  let mut uncertain=runner_command.clone();uncertain.command_id.push_str(".uncertain");uncertain.idempotency_key=IdempotencyKey::new("runner-uncertain").unwrap();
  let result=runner.dispatch(&uncertain).unwrap();assert_eq!(result.state,DataRunnerState::Uncertain);assert!(result.result.is_none());assert_eq!(runner.receipt(&uncertain.workspace_id,&uncertain.command_id).unwrap().unwrap(),result);write_screen_interface_evidence(case,"data-runner-uncertain.json",&serde_json::to_value(&result).unwrap());
  connection.execute_batch("DROP TRIGGER fail_upstream_complete;").unwrap();let recovered=runner.dispatch(&uncertain).unwrap();assert_eq!(recovered.state,DataRunnerState::Completed);assert_eq!(recovered.previous_receipt_sha256,Some(result.receipt_sha256));assert_eq!(runner.dispatch(&uncertain).unwrap(),recovered);write_screen_interface_evidence(case,"data-runner-uncertain-recovered.json",&serde_json::to_value(&recovered).unwrap());
 }
 maybe_run_data_runner_process(dir.path(),&runner_command,case);
 write_screen_interface_evidence(case,"data-runner-command.json",&serde_json::to_value(&runner_command).unwrap());write_screen_interface_evidence(case,"data-runner-receipt.json",&serde_json::to_value(&runner_receipt).unwrap());
 drop(runner);
 let db=dir.path().join("service.db").to_string_lossy().into_owned();drop(service);drop(operations);
 let mut runtime=crate::adapters::local::test_runtime(&db);runtime.plugin_packages=Arc::new(crate::adapters::plugin_packages::LocalPluginPackageStore::new(dir.path().join("package-store")).unwrap());
 let service=crate::service::TradeAssemblyService::from_runtime(&db,runtime);assert_eq!(schedules::current(&service,occurrence).unwrap(),receipt);let runner=DurableCoreDataRunner::new(service.clone(),"fixture.release".into()).unwrap();assert_eq!(runner.dispatch(&runner_command).unwrap(),runner_receipt);

 let mut runtime=(*service.runtime()).clone();runtime.clock=Arc::new(AdvancedClock{base:service.runtime().clock.clone(),advance:Arc::new(std::sync::atomic::AtomicI64::new(86400001))});let expired=crate::service::TradeAssemblyService::from_runtime(dir.path().join("service.db").to_str().unwrap(),runtime);
 let expired_http=expired.handle_http("POST","/product/strategies/session-schedules/prepare",body.clone());assert_eq!(expired_http.status,409);let expired_mcp=expired.call_mcp_tool("tradeassembly.strategy.session_schedule.get",json!({"occurrenceId":occurrence}));assert_eq!(expired_mcp["isError"],true);write_screen_interface_evidence(case,"expired-http.json",&json!({"status":expired_http.status,"body":expired_http.body}));write_screen_interface_evidence(case,"expired-mcp.json",&expired_mcp);

 assert_eq!(service.runtime().storage.list_json("session_occurrence_plans_v1").unwrap().len(),1);
 assert_eq!(service.runtime().storage.list_json("plugin_operation_receipts").unwrap().len(),1);
 let mut instance=service.runtime().plugins.get_instance(&source.plugin_ref).unwrap().unwrap();instance["enabled"]=json!(false);service.runtime().plugins.put_instance(&source.plugin_ref,instance,&ctx("calendar-revoke")).unwrap();assert_eq!(service.call_mcp_tool("tradeassembly.strategy.session_schedule.get",json!({"occurrenceId":occurrence}))["isError"],true);

}

// Opt-in finite cross-process acceptance. The normal production runner and its
// real HTTP authority adapter are used, with only the authority service mocked.
fn maybe_run_data_runner_process(root:&std::path::Path,command:&crate::service::data_runner::DataRunnerCommand,case:&str){
 let Some(binary)=std::env::var_os("DATA_RUNNER_PROCESS_BINARY") else{return;};
 let identity=std::process::Command::new(&binary).arg("--build-identity").output().unwrap();assert!(identity.status.success());let identity:Value=serde_json::from_slice(&identity.stdout).unwrap();let revision=identity["coreRevision"].as_str().unwrap();
 if let Ok(expected)=std::env::var("DATA_RUNNER_EXPECT_REVISION"){assert_eq!(revision,expected,"use the exact clean candidate executable");}
 fn copy_tree(source:&std::path::Path,target:&std::path::Path){std::fs::create_dir_all(target).unwrap();for e in std::fs::read_dir(source).unwrap(){let e=e.unwrap();let meta=e.file_type().unwrap();assert!(!meta.is_symlink());let dest=target.join(e.file_name());if meta.is_dir(){copy_tree(&e.path(),&dest);}else{std::fs::copy(e.path(),dest).unwrap();}}}
 let artifacts=root.join("process-artifacts");copy_tree(&root.join("package-store"),&artifacts.join("plugins"));
 let state=root.join(".tradeassembly");std::fs::create_dir_all(&state).unwrap();let token=state.join("warden.token");std::fs::write(&token,"fake-data-runner-authority-token-at-least-thirty-two-bytes").unwrap();
 #[cfg(unix)]{use std::os::unix::fs::PermissionsExt;std::fs::set_permissions(&state,std::fs::Permissions::from_mode(0o700)).unwrap();std::fs::set_permissions(&token,std::fs::Permissions::from_mode(0o600)).unwrap();}
 use crate::finance_authority::tests::{spawn_mock_warden,MockBehavior};
 for deny in [false,true]{
  let authority=spawn_mock_warden(if deny{MockBehavior::Deny}else{MockBehavior::Allow});
  let mut command=command.clone();command.command_id.push_str(if deny{".process.deny"}else{".process"});command.idempotency_key=IdempotencyKey::new(command.command_id.clone()).unwrap();
  let requests=[json!({"operation":"dispatch_data","command":command}),json!({"operation":"dispatch_data","command":command}),json!({"operation":"data_receipt","workspace_id":command.workspace_id,"command_id":command.command_id})];
  let mut child=std::process::Command::new(&binary).args(["--db",root.join("service.db").to_str().unwrap(),"--core-release","fixture.release"]).current_dir(root).env_clear().env("TRADEASSEMBLY_ARTIFACT_ROOT",&artifacts).env("TRADEASSEMBLY_WARDEN_SIDECAR_URL",&authority.base_url).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
  let mut stdin=child.stdin.take().unwrap();for r in requests{writeln!(stdin,"{r}").unwrap();}drop(stdin);let deadline=std::time::Instant::now()+std::time::Duration::from_secs(180);loop{if child.try_wait().unwrap().is_some(){break;}if std::time::Instant::now()>=deadline{child.kill().unwrap();child.wait().unwrap();panic!("owned data-runner fixture exceeded180s; preserve original command for recovery");}std::thread::sleep(std::time::Duration::from_millis(10));}let output=child.wait_with_output().unwrap();assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
  let responses=String::from_utf8(output.stdout).unwrap().lines().map(|l|serde_json::from_str::<Value>(l).unwrap()).collect::<Vec<_>>();assert_eq!(responses.len(),3);
  for r in &responses{assert_eq!(r["ok"],true,"{r}");assert_eq!(r["receipt"]["executionAuthorityGranted"],false,"{r}");assert_eq!(r["receipt"]["coreRevision"],revision);assert_eq!(r["receipt"]["state"],if deny{"failed"}else{"completed"},"{r}");if deny{assert!(r["receipt"]["result"].is_null());}else{assert_eq!(r["receipt"]["result"]["kind"],"session");}}
  assert_eq!(responses[0],responses[1]);assert_eq!(responses[0],responses[2]);write_screen_interface_evidence(case,if deny{"process-deny.json"}else{"process-allow.json"},&serde_json::to_value(responses).unwrap());
 }
}
