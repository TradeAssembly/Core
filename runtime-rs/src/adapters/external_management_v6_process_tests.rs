// New isolated actual-Core-process/SRT bootstrap. No order executes here.
const MANAGEMENT6_PROCESS_PIN: &str = "15cc88cd373e60a424203f06f8daf4bb0000b7f1";
const MANAGEMENT6_BROKER: &str = "fixture.management.v6.broker";
const MANAGEMENT6_FACTS: &str = "fixture.management.v6.facts";
const MANAGEMENT6_PROVIDER: &str = "fixture.provider.management.v6";
const MANAGEMENT6_ACCOUNT: &str = "fixture.management.v6.paper";

fn management6_grant(s: &crate::service::TradeAssemblyService, capability: &str) {
    s.runtime().storage.put_json("entitlement_grants", capability, json!({
        "grantId":capability,"profile":"local_full","source":"test","capability":capability,
        "effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,
        "precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null
    }), &ctx(&format!("management6-declared-grant:{capability}"))).unwrap();
}

fn install_management6_http_package(
    s: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
    origin: &str,
    broker: bool,
) -> InstalledPluginPackage {
    use crate::adapters::plugin_packages::data_package_tests::{archive_files, materials};
    use tradeassembly_plugin_sdk::{
        broker_v2, cash_settlement, close_settlement, execution_snapshot as obs, session_calendar,
    };
    let id = if broker {
        MANAGEMENT6_BROKER
    } else {
        MANAGEMENT6_FACTS
    };
    let mut schemas = vec![
        (obs::ACCOUNT_SCHEMA, obs::account_schema()),
        (obs::METADATA_SCHEMA, obs::metadata_schema()),
        (obs::QUOTE_SCHEMA, obs::quote_schema()),
        (
            session_calendar::SESSION_CALENDAR_SCHEMA,
            session_calendar::schema(),
        ),
        (
            "schema://fixture/account-health-request@1",
            json!({"type":"object","additionalProperties":false,"properties":{}}),
        ),
        (
            "schema://fixture/account-health-response@1",
            json!({"type":"object","required":["account"],"properties":{"account":{"type":"object"}}}),
        ),
    ];
    let lookup = "schema://tradeassembly/broker-lookup@2";
    if broker {
        schemas.extend([
            (broker_v2::SUBMIT_SCHEMA, broker_v2::submit_schema()),
            (broker_v2::OBSERVATION_SCHEMA, broker_v2::observation_schema()),
            (lookup, json!({"type":"object","additionalProperties":false,"required":["schemaVersion","clientOrderId","canonicalAccountSha256"],"properties":{"schemaVersion":{"const":"tradeassembly.broker_lookup.v2"},"clientOrderId":{"type":"string","minLength":1,"maxLength":128},"canonicalAccountSha256":{"type":"string","pattern":"^[0-9a-f]{64}$"}}})),
            (cash_settlement::REQUEST_SCHEMA, cash_settlement::request_schema()),
            (cash_settlement::RESPONSE_SCHEMA, cash_settlement::response_schema()),
            (close_settlement::REQUEST_SCHEMA, close_settlement::request_schema()),
            (close_settlement::RESPONSE_SCHEMA, close_settlement::response_schema()),
        ]);
    }
    let template = crate::data_pipeline::tests::fixture()
        .0
        .nodes
        .into_iter()
        .find_map(|n| n.selected)
        .unwrap();
    let mut operations = Vec::new();
    let declared = if broker {
        vec![
            (
                crate::broker_submission::v2::SUBMIT_OPERATION,
                crate::broker_submission::v2::SUBMIT_CAPABILITY,
                "paper_trading",
                "write",
                broker_v2::SUBMIT_SCHEMA,
                broker_v2::OBSERVATION_SCHEMA,
            ),
            (
                crate::broker_submission::v2::LOOKUP_OPERATION,
                crate::broker_submission::v2::LOOKUP_CAPABILITY,
                "recovery",
                "read",
                lookup,
                broker_v2::OBSERVATION_SCHEMA,
            ),
            (
                cash_settlement::OPERATION,
                cash_settlement::CAPABILITY,
                "settlement",
                "read",
                cash_settlement::REQUEST_SCHEMA,
                cash_settlement::RESPONSE_SCHEMA,
            ),
            (
                close_settlement::OPERATION,
                close_settlement::CAPABILITY,
                "settlement",
                "read",
                close_settlement::REQUEST_SCHEMA,
                close_settlement::RESPONSE_SCHEMA,
            ),
        ]
    } else {
        vec![
            (
                "execution_account",
                "test.execution.execution_account@1",
                "data_acquisition",
                "read",
                "",
                obs::ACCOUNT_SCHEMA,
            ),
            (
                "execution_metadata",
                "test.execution.execution_metadata@1",
                "data_acquisition",
                "read",
                "",
                obs::METADATA_SCHEMA,
            ),
            (
                "execution_quote",
                "test.execution.execution_quote@1",
                "data_acquisition",
                "read",
                "",
                obs::QUOTE_SCHEMA,
            ),
            (
                "management_v6_calendar",
                "test.session.calendar@1",
                "data_acquisition",
                "read",
                "",
                session_calendar::SESSION_CALENDAR_SCHEMA,
            ),
        ]
    };
    for (opid, capability, purpose, effect, input, output) in
        declared.into_iter().chain(std::iter::once((
            "account.health",
            "account.health",
            "account_health",
            "read",
            "schema://fixture/account-health-request@1",
            "schema://fixture/account-health-response@1",
        )))
    {
        let mut op = serde_json::to_value(&template.operation).unwrap();
        let account_bound = !matches!(opid, "management_v6_calendar" | "account.health");
        op["id"] = json!(opid);
        op["capability"] = json!(capability);
        op["purpose"] = json!(purpose);
        op["effect"] = json!(effect);
        op["protocol"] = json!("stdio");
        op["dependencies"] = json!([]);
        op["apfActionId"] = json!(if effect == "write" {
            "order.submit.paper"
        } else if broker || opid == "account.health" {
            "broker.order.observe"
        } else {
            "calculation.read"
        });
        op["financeResourceType"] = json!(if broker || opid == "account.health" {
            "broker_account"
        } else {
            "plugin_operation"
        });
        op["credentialGrantRequired"] = json!(true);
        op["accountBindingRequired"] = json!(account_bound);
        op["traits"] = json!({"modes":["paper"],"instrumentFamilies":["equity"],"inputSchemaRefs":if input.is_empty(){vec![]}else{vec![input]},"outputSchemaRefs":[output],"deterministic":false,"replayable":false});
        if opid == "management_v6_calendar" {
            op["traits"]["dataShapes"] = json!(["session_calendar"]);
        }
        operations.push(op);
        management6_grant(s, capability);
    }
    let manifest = json!({"metadata":{"id":id,"version":"1","provider":{"id":MANAGEMENT6_PROVIDER,"name":"New synthetic management V6 HTTP only"}},"capabilities":operations.iter().map(|v|json!({"id":v["capability"]})).collect::<Vec<_>>(),"operations":operations,"permissions":["network.outbound"],"configuration":{"fields":[{"id":"dataApiUrl","inputType":"url","storageClass":"configuration"},{"id":"mode","inputType":"text","storageClass":"configuration"}]},"health":{"requiredConfiguration":["dataApiUrl"],"requiredCredentials":["api_key","api_secret"]}});
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let binary = fixture_executable();
    let descriptor = json!({"packageContractVersion":"1","plugin":{"id":id,"version":"1"},"manifest":{"path":"manifest.json","sha256":sha256(&manifest_bytes)},"targets":[{"target":tradeassembly_plugin_sdk::host_target(),"binary":{"path":"bin/plugin","sha256":sha256(binary)}}],"compatibility":{"hostContract":{"minimum":"1","maximum":"1"},"sdkVersion":tradeassembly_plugin_sdk::SDK_VERSION},"responseSchemas":[]});
    let mut descriptor = if broker {
        descriptor
    } else {
        let (mut outer, _, _, _) = materials();
        outer["package"] = descriptor;
        outer["dataContracts"] = json!([]);
        outer["dataSchemas"] = json!([]);
        outer
    };
    let mut files = vec![
        ("manifest.json".to_owned(), manifest_bytes),
        ("bin/plugin".into(), binary.to_vec()),
    ];
    for (index, (schema_ref, schema)) in schemas.into_iter().enumerate() {
        let bytes = serde_json::to_vec(&schema).unwrap();
        let path = format!("schema-{index}.json");
        let record =
            json!({"schemaRef":schema_ref,"document":{"path":path,"sha256":sha256(&bytes)}});
        if broker {
            descriptor["responseSchemas"]
                .as_array_mut()
                .unwrap()
                .push(record);
        } else {
            descriptor["package"]["responseSchemas"]
                .as_array_mut()
                .unwrap()
                .push(record.clone());
            let mut data = record;
            data["canonicalSha256"] = json!(canonical_sha256(&schema).unwrap());
            descriptor["dataSchemas"].as_array_mut().unwrap().push(data);
        }
        files.push((path, bytes));
    }
    if !broker {
        let contract = json!({"schemaVersion":"tradeassembly.operation_data.v2","operationId":"execution_account","semanticsVersion":"fixture.execution.v1","inputs":{},"outputs":{"account":{"schema":{"schemaRef":obs::ACCOUNT_SCHEMA,"sha256":canonical_sha256(&obs::account_schema()).unwrap()},"shape":"scalar","time":{"intervalMs":null,"completedOnly":true}}},"parameterSchema":{"type":"object","additionalProperties":false},"applicability":{"assetClasses":["equity"],"instrumentFamilies":["equity"],"modes":["paper"]},"readiness":"all_required_inputs","warmup":{"minimumObservations":1,"observationsParameter":null,"additionalObservations":0},"state":null});
        let bytes = serde_json::to_vec(&contract).unwrap();
        descriptor["dataContracts"].as_array_mut().unwrap().push(json!({"operationId":"execution_account","document":{"path":"contract.json","sha256":sha256(&bytes)},"canonicalSha256":canonical_sha256(&contract).unwrap()}));
        files.push(("contract.json".into(), bytes));
    }
    files.push((
        "tradeassembly-plugin.json".into(),
        serde_json::to_vec(&descriptor).unwrap(),
    ));
    let archive = archive_files(
        files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.clone()))
            .collect(),
    );
    let path = root.join(format!("{id}.tgz"));
    fs::write(&path, &archive).unwrap();
    let package = s
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
    s.runtime().plugins.install_manifest(id, json!({"pluginRef":id,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}), &ctx("management6-new-http-manifest")).unwrap();
    let credential = format!("{id}.new-synthetic-handle");
    s.runtime()
        .credentials
        .store(
            &credential,
            &BTreeMap::from([
                ("api_key".into(), "fake-management6-no-access".into()),
                (
                    "api_secret".into(),
                    "fake-management6-secret-no-access".into(),
                ),
            ]),
        )
        .unwrap();
    s.runtime().plugins.put_instance(id, json!({"instanceRef":id,"pluginRef":id,"providerRef":MANAGEMENT6_PROVIDER,"accountRef":format!("account://{id}/{MANAGEMENT6_ACCOUNT}"),"enabled":true,"activePackageSha256":package.package_sha256,"configuration":{"dataApiUrl":origin,"mode":"paper"},"credentialRef":credential,"credentialRevision":1,"accountMode":"paper","credentialStatus":{"configured":true},"health":{"state":"ready","connectivityChecked":false,"evidenceCurrent":false}}), &ctx("management6-new-http-instance")).unwrap();
    s.bind_owned_object("plugin_instance", id).unwrap();
    package
}

fn management6_driver_phase(
    script: &std::path::Path,
    mode: &str,
    boot: &Value,
    root: &std::path::Path,
) -> Value {
    let mut child = std::process::Command::new("python3")
        .arg(script)
        .arg(mode)
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{boot}").unwrap();
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(if mode == "--drive" { 600 } else { 180 });
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "bounded management6 phase {mode} timed out; preserve original new fixture {}",
                root.display()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    let label = mode.trim_start_matches('-');
    fs::write(
        root.join(format!("management6-{label}.stdout.log")),
        &output.stdout,
    )
    .unwrap();
    fs::write(
        root.join(format!("management6-{label}.stderr.log")),
        &output.stderr,
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{mode} failed; preserve new root {}: {}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[ignore = "explicit NEW frozen Core15 V6 HTTP-only provider/SRT whole-process bootstrap; no Relay"]
fn management_v6_actual_core_process_http_provider_bootstrap() {
    assert_eq!(std::env::var("MANAGEMENT_V6_PROCESS_READY").unwrap(), "yes");
    let cfg: Value = serde_json::from_slice(
        &fs::read(std::env::var("MANAGEMENT_V6_PROCESS_CONFIG").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(cfg["contract"], "management-v6-core-process-srt");
    assert_eq!(cfg["fixtureReady"], true);
    assert_eq!(cfg["coreRevision"], MANAGEMENT6_PROCESS_PIN);
    assert_eq!(cfg["relayQualified"], false);
    crate::build_identity::install(MANAGEMENT6_PROCESS_PIN).unwrap();
    let script = std::path::PathBuf::from(std::env::var("MANAGEMENT_V6_PROCESS_DRIVER").unwrap());
    assert!(script.is_absolute());
    let origin = std::env::var("MANAGEMENT_V6_HTTP_ORIGIN").unwrap();
    assert!(origin.starts_with("http://127.0.0.1:"));
    let retained = tempfile::Builder::new()
        .prefix("management6-new-process-")
        .tempdir()
        .unwrap()
        .keep();
    let root = retained.canonicalize().unwrap();
    assert!(root.is_absolute());
    let db = root.join("service.db");
    let state = root.join(".tradeassembly");
    fs::create_dir(&state).unwrap();
    let token = state.join("warden.token");
    fs::write(&token, "fake-management6-warden-at-least-thirty-two-bytes").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fs::write(
        root.join("SYNTHETIC_MANAGEMENT_V6_PROCESS_FIXTURE"),
        "new synthetic only\n",
    )
    .unwrap();
    let warden = crate::finance_authority::tests::spawn_mock_warden(
        crate::finance_authority::tests::MockBehavior::Allow,
    );
    let mut config = crate::runtime_config::RuntimeConfig::local(db.to_string_lossy().into_owned());
    config.oidc_profile = "local_owner".into();
    config.artifact_root = Some(root.join("reach-artifacts").to_string_lossy().into_owned());
    config.warden_sidecar_url = warden.base_url.clone();
    config.warden_token_ref = format!("file://{}", token.display());
    config.plugin_sandbox_command = cfg["srt"].as_str().unwrap().into();
    config.plugin_sandbox_allow_local_egress = true;
    config.telemetry_enabled = false;
    config.legal_receipt_root = state.join("legal/receipts").to_string_lossy().into_owned();
    config.legal_trusted_keys_path = state
        .join("legal/trusted-keys.json")
        .to_string_lossy()
        .into_owned();
    config.legal_policy_path = state
        .join("legal/policy.json")
        .to_string_lossy()
        .into_owned();
    config.oidc_session_path = state
        .join("auth/cli-session.bin")
        .to_string_lossy()
        .into_owned();
    config.oidc_session_key_path = state
        .join("auth/cli-session.key")
        .to_string_lossy()
        .into_owned();
    let service = crate::service::TradeAssemblyService::from_config_data_only(config).unwrap();
    let owner = crate::local_owner_identity::LocalOwnerIdentity::for_database(&db).unwrap();
    let service =
        service.for_authenticated_invocation(&owner.issuer, &owner.stable_identity_id, None, None);
    let replay = install_fixture_variant(&service, &root, false, 60000, false, false, true);
    service.runtime().plugins.put_instance(&replay.plugin_ref,json!({"instanceRef":replay.plugin_ref,"pluginRef":replay.plugin_ref,"providerRef":replay.plugin_ref,"enabled":true,"activePackageSha256":replay.package_sha256,"configuration":{}}),&ctx("management6-pure-replay-instance")).unwrap();
    let calculator = install_execution_package(&service, &root, true);
    let facts = install_management6_http_package(&service, &root, &origin, false);
    let broker = install_management6_http_package(&service, &root, &origin, true);
    for instance in service.runtime().plugins.list_instances().unwrap() {
        service
            .bind_owned_object("plugin_instance", instance["instanceRef"].as_str().unwrap())
            .unwrap();
    }
    let mut boot = json!({"root":root,"coreRevision":MANAGEMENT6_PROCESS_PIN,"brokerInstanceRef":MANAGEMENT6_BROKER,"factsInstanceRef":MANAGEMENT6_FACTS,"providerAccountId":MANAGEMENT6_ACCOUNT,"providerId":MANAGEMENT6_PROVIDER,"wardenOrigin":warden.base_url,"brokerOrigin":origin});
    let health = management6_driver_phase(&script, "--health", &boot, &root);
    assert_eq!(health["beforeGraphAndActivation"], true);
    assert!(service
        .runtime()
        .storage
        .list_json("execution_activations")
        .unwrap()
        .is_empty());
    assert!(service
        .runtime()
        .storage
        .list_json("execution_order_plan_submissions_v2")
        .unwrap()
        .is_empty());
    let now = service.runtime().clock.now_ms();
    let snapshot = json!({"instrument":"SPY","start":now-60000,"interval":60000,"observedAtMs":now,"values":[100]});
    let subject = InstrumentContext {
        instrument_id: "SPY".into(),
        asset_class: "equity".into(),
        instrument_family: "equity".into(),
    };
    let program = crate::data_strategy::DataStrategy {
        version: 1,
        nodes: vec![PipelineNode {
            graph_node_id: "exec_signal".into(),
            parameters: json!({"snapshot":snapshot,"digest":canonical_sha256(&snapshot).unwrap()}),
            context: DataContext {
                instruments: vec![subject.clone()],
                interval_ms: Some(60000),
            },
            input_contexts: BTreeMap::new(),
        }],
        bindings: vec![],
        trade_instruments: vec![subject],
        entry: crate::data_strategy::Rule::Compare {
            left: crate::data_strategy::Operand::Output {
                node: "exec_signal".into(),
                port: "value".into(),
                instrument: "SPY".into(),
                sample: crate::data_strategy::Sample::Current,
            },
            op: crate::data_strategy::Comparison::Greater,
            right: crate::data_strategy::Operand::Number { value: 0.0 },
        },
        exit: crate::data_strategy::Rule::IsTrue {
            value: crate::data_strategy::Operand::Boolean { value: false },
        },
    };
    let mut spec: Value = serde_json::from_str(include_str!(
        "../../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .unwrap();
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({(crate::data_strategy::CONFIG_KEY):program});
    spec["stages"]["sizing"]["substeps"][0]["config"] = json!({"tradeassembly.order_plan":{"schemaVersion":"tradeassembly.order_plan_policy.v1","calculationNode":"exec_calc","semanticsVersion":"fixture.execution.v1","sourceNodes":{"account_data":"exec_account","instrument_metadata":"exec_metadata","pricing_quote":"exec_quote"},"parameters":{"quantity":"0.4"},"positionAction":"buy_to_open","timeInForce":"day","feeCashCap":"1","maxPurchaseCash":"50","maxPremiumGross":"100","maxQuantity":"1","maximumAskMultiple":"1","maxSignalAgeMs":600000,"maxSnapshotAgeMs":30000,"quoteProfiles":["fixture-quote.v1"]}});
    spec["stages"]["exit_policy"]["enabled"] = json!(true);
    spec["stages"]["exit_policy"]["policy"] = json!({"monitoring":["synthetic"],"triggers":[{"id":"stop_loss","kind":"price","scope":"full_position"},{"id":"trailing","kind":"price","scope":"full_position"}],"composition":"priority","precedence":["stop_loss","trailing"],"end_of_data":"preserve_open_positions"});
    spec["stages"]["exit_policy"]["config"]["tradeassembly.managed_exit.v1"] = json!({"rules":{"schemaVersion":"tradeassembly.position_exit_rules.v1","priceBasis":"bid","stopLossRatio":"0.1","trailingRatio":"0.2","trailingStart":{"kind":"immediate"},"reversalSignal":null,"precedence":["stop_loss","trailing"],"maxQuoteAgeMs":30000},"entryBasis":"provider_final_gross_excluding_fees","feeCashCap":"1","maxLimitSlippageRatio":"0.01","timeInForce":"day","maxReversalAgeMs":30000});
    configure_dispatch_calendar_fixture(&service, &mut spec);
    spec["stages"]["market_clock"]["policy"]["session_triggers"][0]["offset_ms"] = json!(0);
    spec["stages"]["market_clock"]["policy"]["session_triggers"][0]["missed_run"]
        ["max_lateness_ms"] = json!(600000);
    spec["stages"]["market_clock"]["policy"]["session_triggers"][0]["selections"][0]
        ["selector_ref"] = json!("primary_symbol");
    let mut bindings = json!({"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"},"calendar":{"pluginInstanceRef":facts.plugin_ref,"pluginRef":facts.plugin_ref,"operationId":"management_v6_calendar"}});
    for (id, op, plugin, capability, schemas, stage) in [
        (
            "exec_signal",
            "replay",
            &replay,
            "test.capture.replay@1",
            vec!["schema://test/series@1"],
            "evaluate",
        ),
        (
            "exec_account",
            "execution_account",
            &facts,
            "test.execution.execution_account@1",
            vec![tradeassembly_plugin_sdk::execution_snapshot::ACCOUNT_SCHEMA],
            "sizing",
        ),
        (
            "exec_metadata",
            "execution_metadata",
            &facts,
            "test.execution.execution_metadata@1",
            vec![tradeassembly_plugin_sdk::execution_snapshot::METADATA_SCHEMA],
            "sizing",
        ),
        (
            "exec_quote",
            "execution_quote",
            &facts,
            "test.execution.execution_quote@1",
            vec![tradeassembly_plugin_sdk::execution_snapshot::QUOTE_SCHEMA],
            "sizing",
        ),
        (
            "exec_calc",
            "plan_calculate",
            &calculator,
            "test.execution.plan_calculate@1",
            vec![
                tradeassembly_plugin_sdk::execution_calculation::INPUT_SCHEMA,
                tradeassembly_plugin_sdk::execution_calculation::OUTPUT_SCHEMA,
            ],
            "sizing",
        ),
        (
            "exec_broker",
            crate::broker_submission::v2::SUBMIT_OPERATION,
            &broker,
            crate::broker_submission::v2::SUBMIT_CAPABILITY,
            vec![
                tradeassembly_plugin_sdk::broker_v2::SUBMIT_SCHEMA,
                tradeassembly_plugin_sdk::broker_v2::OBSERVATION_SCHEMA,
            ],
            "order_strategy",
        ),
    ] {
        let mut r = spec["capability_requirements"]["required"][0].clone();
        r["requirement_id"] = json!(id);
        r["capability"] = json!(capability);
        r["stage_refs"] = json!([stage]);
        r["substep_refs"] = json!([]);
        r["dependency_refs"] = json!([]);
        r["constraints"] = json!({"schema_refs":schemas,"data_shapes":[],"fields":[],"input_paths":[],"operations":[],"deterministic":false,"replayable":false});
        spec["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap()
            .push(r);
        bindings[id] = json!({"pluginInstanceRef":plugin.plugin_ref,"pluginRef":plugin.plugin_ref,"operationId":op});
        if plugin.plugin_ref == facts.plugin_ref || plugin.plugin_ref == broker.plugin_ref {
            bindings[id]["accountRef"] = json!(format!(
                "account://{}/{MANAGEMENT6_ACCOUNT}",
                plugin.plugin_ref
            ));
        }
        management6_grant(&service, capability);
    }
    let validation = crate::spec::validate_strategy_spec_report(&spec);
    assert!(validation.valid, "{validation:?}");
    let created=service.handle_http("POST","/product/strategies/create",json!({"name":"NEW management6 HTTP process fixture","spec":spec,"idempotencyKey":"management6-create"}));
    assert_eq!(created.status, 201, "{}", created.body);
    let id = created.body["body"]["strategy"]["id"].as_str().unwrap();
    let published=service.handle_http("POST","/product/strategies/publish",json!({"strategyId":id,"expectedDraftHash":created.body["body"]["draft"]["draftHash"],"idempotencyKey":"management6-publish"}));
    assert_eq!(
        published.body["body"]["published"], true,
        "{}",
        published.body
    );
    let scope = crate::service::data_plans::PlanScope {
        strategy_id: id.into(),
        strategy_version_id: published.body["body"]["version"]["id"]
            .as_str()
            .unwrap()
            .into(),
        strategy_spec_hash: published.body["body"]["version"]["specHash"]
            .as_str()
            .unwrap()
            .into(),
    };
    let graph=service.handle_http("POST","/capability-graph-revisions",json!({"configId":"management6-config","strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper","bindings":bindings,"idempotencyKey":"management6-graph"}));
    assert_eq!(
        graph.body["revision"]["graph"]["ok"], true,
        "{}",
        graph.body
    );
    let revision = graph.body["revision"]["revisionId"].as_str().unwrap();
    activate_native_typed_execution(&service, &scope, revision, "management6_process");
    let prepared=service.handle_http("POST","/product/strategies/data-plans/prepare",json!({"scope":scope,"capabilityGraphRevisionId":revision,"mode":"paper","idempotencyKey":"management6-signal-plan"}));
    assert_eq!(prepared.status, 200, "{}", prepared.body);
    let signal=service.handle_http("POST","/product/strategies/data-runs/step",json!({"step":{"runId":"management6-signal","planRevisionId":prepared.body["plan"]["revisionId"],"window":{"eventStartMs":now-60000,"eventEndMs":now,"asOfMs":service.runtime().clock.now_ms()},"expectedCheckpointSha256":null},"idempotencyKey":"management6-ready-signal"}));
    assert_eq!(signal.status, 200, "{}", signal.body);
    assert_eq!(
        signal.body["receipt"]["signals"][0]["entry"],
        json!({"status":"ready","value":true})
    );
    assert!(
        service
            .runtime()
            .storage
            .list_json("execution_order_plan_submissions_v2")
            .unwrap()
            .is_empty(),
        "no original order in bootstrap"
    );
    assert!(service
        .runtime()
        .storage
        .list_json("execution_managed_positions_v1")
        .unwrap()
        .is_empty());
    boot["scope"] = json!(scope);
    boot["graph"] = json!(revision);
    boot["activationId"] = json!("fixture-v2-activation");
    boot["signalReceiptSha256"] = signal.body["receipt"]["receiptSha256"].clone();
    boot["sourceNodes"] = json!({"account":"exec_account","metadata":"exec_metadata","quote":"exec_quote","calendar":"calendar"});
    let health_proof = std::path::Path::new(cfg["evidence"].as_str().unwrap())
        .join("health-before-graph/proof.json");
    boot["healthProofSha256"] = json!(sha256(&fs::read(health_proof).unwrap()));
    fs::write(
        root.join("management6-bootstrap.json"),
        serde_json::to_vec_pretty(&boot).unwrap(),
    )
    .unwrap();
    let result = management6_driver_phase(&script, "--drive", &boot, &root);
    assert_eq!(result["status"], "PASS");
    assert_eq!(result["relayQualified"], false);
    eprintln!("retained NEW Core process root {}", root.display());
}
