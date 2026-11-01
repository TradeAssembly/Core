// Generic host-flow qualification with a separately declared synthetic 1-second
// counter package. This does not qualify the real pack's daily EMA/TSI formulas.
const AUTOMATIC_STREAM_CASE: &str = "financial_screen_auto_stream";
const STREAM_PLUGIN: &str = "fixture.managed_stream.synthetic";
const STREAM_SOURCE: &str = "exec_stream_source";
const STREAM_ACQUIRE: &str = "exec_stream_acquire";
fn automatic_stream_executable() -> &'static [u8] {
    static BINARY: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let deps = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("managed-stream-fixture");
        let mut command = std::process::Command::new("rustc");
        command
            .args(["--edition=2021", "--crate-name", "managed_stream_fixture"])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/adapters/fixtures/managed_stream_plugin.rs"),
            )
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .arg("-o")
            .arg(&output);
        for name in ["serde_json", "tradeassembly_plugin_sdk"] {
            let library = owning_fingerprint_dependency(name);
            command
                .arg("--extern")
                .arg(format!("{name}={}", library.display()));
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        fs::read(output).unwrap()
    })
}
fn install_automatic_stream_package(
    s: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
) -> InstalledPluginPackage {
    use crate::adapters::plugin_packages::data_package_tests::{archive_files, materials};
    let (mut descriptor, contract, series_bytes, _) = materials();
    let mut source: Value = serde_json::from_slice(&contract).unwrap();
    source["schemaVersion"] = json!("tradeassembly.operation_data.v2");
    source["operationId"] = json!("stream_replay");
    source["semanticsVersion"] = json!("synthetic_stream_batch_v2");
    source["outputs"]["value"]["time"] = json!({"intervalMs":1000,"completedOnly":true});
    source["inputs"] = json!({});
    source["state"] = Value::Null;
    source["warmup"] =
        json!({"minimumObservations":1,"observationsParameter":null,"additionalObservations":0});
    source["applicability"] =
        json!({"assetClasses":["equity"],"instrumentFamilies":["equity"],"modes":["paper"]});
    source["parameterSchema"] = json!({"type":"object","properties":{"snapshot":{"type":"object"},"digest":{"type":"string"}},"required":["snapshot","digest"],"additionalProperties":false});
    let state_schema = json!({"type":"object","properties":{"seen":{"type":"integer","minimum":0}},"required":["seen"],"additionalProperties":false});
    let mut counter = source.clone();
    counter["operationId"] = json!("stream_counter");
    counter["inputs"] = json!({"source":{"data":source["outputs"]["value"],"required":true,"dependencyId":"source"}});
    counter["parameterSchema"] = json!({"type":"object","additionalProperties":false});
    counter["state"] = json!({"schema":{"schemaRef":"schema://fixture-managed-stream/state@1","sha256":canonical_sha256(&state_schema).unwrap()},"version":"synthetic_counter_batch_v2","partition":"instrument"});
    let history = json!({"schemaVersion":"tradeassembly.operation_data.v3","operation":counter,
        "history":{"firstReady":{"value":{"source":{"op":"literal","value":1}}},"partitioning":"instrument_independent",
            "requiredInputUnavailable":"invoke_complete_frame","reconstruction":"checkpoint_or_explicit_origin"}});
    let template = crate::data_pipeline::tests::fixture()
        .0
        .nodes
        .into_iter()
        .find_map(|n| n.selected)
        .unwrap();
    let mut operations = Vec::new();
    for (id, cap, pure, schemas) in [
        (
            "stream_acquire",
            "test.managed_stream.acquire@1",
            false,
            vec!["schema://fixture-managed-stream/acquisition@1"],
        ),
        (
            "stream_replay",
            "test.managed_stream.replay@1",
            true,
            vec!["schema://test/series@1", EVALUATION_RESPONSE_SCHEMA],
        ),
        (
            "stream_counter",
            "test.managed_stream.counter@1",
            true,
            vec!["schema://test/series@1", EVALUATION_RESPONSE_SCHEMA],
        ),
    ] {
        let mut op = serde_json::to_value(&template.operation).unwrap();
        op["id"] = json!(id);
        op["capability"] = json!(cap);
        op["protocol"] = json!("stdio");
        op["effect"] = json!("read");
        op["dependencies"] = json!([]);
        op["credentialGrantRequired"] = json!(false);
        op["accountBindingRequired"] = json!(false);
        op["traits"] = json!({"modes":["paper"],"instrumentFamilies":["equity"],"inputSchemaRefs":if id=="stream_counter"{vec!["schema://test/series@1"]}else{vec![]},
            "outputSchemaRefs":schemas,"deterministic":pure,"replayable":pure});
        operations.push(op);
    }
    let manifest = json!({"metadata":{"id":STREAM_PLUGIN,"version":"2","provider":{"id":STREAM_PLUGIN,"name":"Synthetic one-second captured stream counter"}},
        "capabilities":operations.iter().map(|op|json!({"id":op["capability"]})).collect::<Vec<_>>(),"operations":operations,"permissions":[],
        "configuration":{"fields":[]},"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let binary = automatic_stream_executable();
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    descriptor["packageContractVersion"] = json!("3");
    descriptor["recipes"] = json!([]);
    descriptor["package"]["plugin"]["id"] = json!(STREAM_PLUGIN);
    descriptor["package"]["plugin"]["version"] = json!("2");
    descriptor["package"]["manifest"]["sha256"] = json!(sha256(&manifest_bytes));
    descriptor["package"]["targets"][0]["binary"]["sha256"] = json!(sha256(binary));
    descriptor["dataContracts"] = json!([]);
    descriptor["dataSchemas"] = json!([]);
    descriptor["package"]["responseSchemas"] = json!([]);
    let mut files = vec![
        ("manifest.json".to_string(), manifest_bytes),
        ("bin/plugin".into(), binary.to_vec()),
    ];
    for (name, value) in [("stream_replay", source), ("stream_counter", history)] {
        let path = format!("contract-{name}.json");
        let bytes = serde_json::to_vec(&value).unwrap();
        descriptor["dataContracts"].as_array_mut().unwrap().push(json!({"operationId":name,"document":{"path":path,"sha256":sha256(&bytes)},"canonicalSha256":canonical_sha256(&value).unwrap()}));
        files.push((path, bytes));
    }
    let series: Value = serde_json::from_slice(&series_bytes).unwrap();
    for (name, value) in [
        ("schema://test/series@1", series),
        ("schema://fixture-managed-stream/state@1", state_schema),
        (EVALUATION_RESPONSE_SCHEMA, json!({"type":"object"})),
        (
            "schema://fixture-managed-stream/acquisition@1",
            json!({"type":"object"}),
        ),
    ] {
        let path = format!("schema-{}.json", files.len());
        let bytes = serde_json::to_vec(&value).unwrap();
        descriptor["dataSchemas"].as_array_mut().unwrap().push(json!({"schemaRef":name,"document":{"path":path,"sha256":sha256(&bytes)},"canonicalSha256":canonical_sha256(&value).unwrap()}));
        if matches!(
            name,
            EVALUATION_RESPONSE_SCHEMA | "schema://fixture-managed-stream/acquisition@1"
        ) {
            descriptor["package"]["responseSchemas"]
                .as_array_mut()
                .unwrap()
                .push(json!({"schemaRef":name,"document":{"path":path,"sha256":sha256(&bytes)}}));
        }
        files.push((path, bytes));
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
    let path = root.join("synthetic-managed-stream.tgz");
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
    s.runtime().plugins.install_manifest(STREAM_PLUGIN,json!({"pluginRef":STREAM_PLUGIN,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx("stream-fixture-install")).unwrap();
    s.runtime().plugins.put_instance(STREAM_PLUGIN,json!({"instanceRef":STREAM_PLUGIN,"pluginRef":STREAM_PLUGIN,"providerRef":STREAM_PLUGIN,"enabled":true,"activePackageSha256":package.package_sha256,"configuration":{}}),&ctx("stream-fixture-instance")).unwrap();
    package
}
fn automatic_stream_source_config() -> Value {
    json!({"acquisitionNode":STREAM_ACQUIRE,"intervalMs":1000,"origin":{"kind":"admitted_capture_start"},
        "parameterExtracts":{"snapshot":"/snapshot","digest":"/digest"},"input":{
            "instrument":{"kind":"trade_instrument_id"},"start":{"kind":"event_start_ms"},"end":{"kind":"event_end_ms"},
            "origin":{"kind":"origin_start_ms"},"cutoff":{"kind":"as_of_ms"},"interval":{"kind":"interval_ms"},
            "feed":{"kind":"literal","value":"synthetic_stream_batch_v2"}}})
}
fn configure_automatic_stream_spec(
    s: &crate::service::TradeAssemblyService,
    spec: &mut Value,
    bindings: &mut Value,
    replay: &InstalledPluginPackage,
    stream: &InstalledPluginPackage,
) {
    use crate::selection_strategy::*;
    configure_managed_plan_native_spec(s, spec, bindings, replay);
    let config = &mut spec["stages"]["evaluate"]["substeps"][0]["config"][CONFIG_KEY];
    let mut template: SelectionStrategy = serde_json::from_value(config.clone()).unwrap();
    template.nodes = vec![
        NodeTemplate {
            graph_node_id: STREAM_SOURCE.into(),
            parameters: json!({}),
            context: ContextTemplate::Subject {
                interval_ms: Some(1000),
            },
            input_contexts: BTreeMap::new(),
        },
        NodeTemplate {
            graph_node_id: "exec_signal".into(),
            parameters: json!({}),
            context: ContextTemplate::Subject {
                interval_ms: Some(1000),
            },
            input_contexts: BTreeMap::new(),
        },
    ];
    template.bindings = vec![PortBinding {
        producer_node: STREAM_SOURCE.into(),
        output_port: "value".into(),
        consumer_node: "exec_signal".into(),
        input_port: "source".into(),
    }];
    template.exit = RuleTemplate::Compare {
        left: OperandTemplate::Output {
            node: "exec_signal".into(),
            port: "value".into(),
            instrument: InstrumentRef::Subject,
            sample: crate::data_strategy::Sample::Current,
        },
        op: crate::data_strategy::Comparison::Less,
        right: OperandTemplate::Number { value: 0.0 },
    };
    *config = serde_json::to_value(template).unwrap();
    for (node, op, cap, schemas) in [
        (
            STREAM_SOURCE,
            "stream_replay",
            "test.managed_stream.replay@1",
            vec!["schema://test/series@1"],
        ),
        (
            STREAM_ACQUIRE,
            "stream_acquire",
            "test.managed_stream.acquire@1",
            vec!["schema://fixture-managed-stream/acquisition@1"],
        ),
        (
            "exec_signal",
            "stream_counter",
            "test.managed_stream.counter@1",
            vec!["schema://test/series@1"],
        ),
    ] {
        let requirements = spec["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap();
        let mut requirement = requirements[0].clone();
        requirement["requirement_id"] = json!(node);
        requirement["capability"] = json!(cap);
        requirement["stage_refs"] = json!(["evaluate"]);
        requirement["substep_refs"] = json!([]);
        requirement["dependency_refs"] = json!([]);
        requirement["constraints"] = json!({"schema_refs":schemas,"data_shapes":[],"fields":[],"input_paths":[],"operations":[],"deterministic":false,"replayable":false});
        if let Some(existing) = requirements
            .iter_mut()
            .find(|r| r["requirement_id"] == node)
        {
            *existing = requirement;
        } else {
            requirements.push(requirement);
        }
        bindings[node] = json!({"pluginInstanceRef":stream.plugin_ref,"pluginRef":stream.plugin_ref,"operationId":op});
        s.runtime().storage.put_json("entitlement_grants",cap,json!({"grantId":cap,"profile":"local_full","source":"test","capability":cap,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("stream-fixture-entitlement")).unwrap();
    }
}
fn prepare_automatic_stream_entry_plan(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    revision: &str,
    start: i64,
    end: i64,
    subject: &str,
    key: &str,
) -> Value {
    use crate::service::{
        data_collections as collections, instrument_screens as screens, selection_plans,
    };
    let source: crate::service::order_plans::management::automation::reversal::Source =
        serde_json::from_value(automatic_stream_source_config()).unwrap();
    let input = source
        .materialize_input(
            subject,
            &tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow {
                event_start_ms: start,
                event_end_ms: end,
                as_of_ms: s.runtime().clock.trusted_now_ms().unwrap(),
            },
            start,
        )
        .unwrap();
    let capture=s.handle_http("POST","/product/strategies/data-captures/create",json!({"capture":{
        "receiptVersion":"tradeassembly.data_capture_receipt.v2","policy":{"scope":scope,"capabilityGraphRevision":revision,"mode":"paper",
        "acquisitionNode":STREAM_ACQUIRE,"replayNode":STREAM_SOURCE,"parameterExtracts":{"snapshot":"/snapshot","digest":"/digest"},
        "context":{"instruments":[{"instrumentId":subject,"assetClass":"equity","instrumentFamily":"equity"}],"intervalMs":1000},
        "eventStartMs":start,"eventEndMs":end,"gapPolicy":"strict_fixed_interval"},"input":input},"accountMode":"paper","idempotencyKey":format!("{key}-seed-capture")}));
    assert_eq!(capture.status, 200, "{}", capture.body);
    assert_eq!(capture.body["capture"]["inputWitness"]["input"], input);
    if key == "stream-initial" {
        // The automatic scheduler owns screening, selected-plan preparation and
        // selection. Supply only the actual fresh admitted seed here; do not
        // run the entire entry selection journey once before running it again.
        write_screen_interface_evidence(
            "automatic_stream",
            "initial-seed-capture.json",
            &capture.body,
        );
        return capture.body;
    }
    let collection = collections::acquire(
        s,
        collections::CollectionCommand {
            scope: scope.clone(),
            capability_graph_revision: revision.into(),
            acquisition_node: "screen_source".into(),
            mode: "paper".into(),
            response_schema_ref: "schema://capture-fixture/response@1".into(),
            input: json!({}),
        },
        &ctx(&format!("{key}-collection")),
    )
    .unwrap();
    let screen = screens::acquire(
        s,
        screens::ScreenCommand {
            scope: scope.clone(),
            capability_graph_revision: revision.into(),
            mode: "paper".into(),
            signal_requirement_ref: "signal_primary".into(),
            selector_ref: "screened".into(),
            collections: BTreeMap::from([("contracts".into(), collection.collection_id)]),
            knowledge_cutoff_ms: s.runtime().clock.now_ms(),
        },
        &ctx(&format!("{key}-screen")),
    )
    .unwrap();
    assert_eq!(screen.instrument_set.members[0].instrument_id, subject);
    let command = selection_plans::SelectionPlanCommand {
        scope: scope.clone(),
        capability_graph_revision: revision.into(),
        mode: "paper".into(),
        subject_instrument_id: subject.into(),
        selection_receipts: BTreeMap::from([("selected".into(), screen.selection_id)]),
        source_captures: BTreeMap::from([(
            STREAM_SOURCE.into(),
            capture.body["capture"]["captureId"]
                .as_str()
                .unwrap()
                .into(),
        )]),
    };
    let plan=s.handle_http("POST","/product/strategies/selection-plans/prepare",json!({"selectionPlan":command,"accountMode":"paper","idempotencyKey":format!("{key}-prepare")}));
    assert_eq!(plan.status, 200, "{}", plan.body);
    let selected = financial_native_call(
        s,
        crate::service::order_plans::remote::FinancialOperation::Select {
            scope: scope.clone(),
            selection: crate::service::order_plans::decision::SelectionCommand {
                activation_id: "fixture-v2-activation".into(),
                plan_revision_id: plan.body["plan"]["revisionId"].as_str().unwrap().into(),
            },
        },
        &format!("{key}-select"),
    );
    assert_eq!(selected.status, 200, "{}", selected.body);
    write_screen_interface_evidence(
        "automatic_stream",
        &format!("{key}-seed.json"),
        &json!({"capture":capture.body,"plan":plan.body,"selected":selected.body,"syntheticIntervalMs":1000}),
    );
    plan.body
}
fn automatic_stream_cursor(receipt: &crate::service::data_evaluation::EvaluationReceipt) -> i64 {
    assert_eq!(
        receipt.checkpoint.cursors.len(),
        1,
        "one typed source/port/instrument"
    );
    *receipt.checkpoint.cursors.values().next().unwrap()
}
fn automatic_stream_receipts(
    s: &crate::service::TradeAssemblyService,
) -> Vec<crate::service::data_evaluation::EvaluationReceipt> {
    let mut receipts = s
        .runtime()
        .storage
        .list_json("data_evaluation_receipts_v1")
        .unwrap()
        .into_iter()
        .map(|(_, value)| {
            serde_json::from_value::<crate::service::data_evaluation::EvaluationReceipt>(value)
                .unwrap()
        })
        .filter(|r| r.source_snapshot_sha256.is_some())
        .collect::<Vec<_>>();
    receipts.sort_by_key(automatic_stream_cursor);
    receipts
}
fn exercise_native_automatic_stream(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    revision: &str,
    prepared: &Value,
    root: &std::path::Path,
) {
    use crate::service::{
        data_collections as collections, session_dispatch as dispatch,
        session_schedules as schedules,
    };
    let seed = prepared["capture"]["captureId"].as_str().unwrap();
    let seed = crate::service::data_captures::current(s, seed).unwrap();
    let calendar = collections::acquire(
        s,
        collections::CollectionCommand {
            scope: scope.clone(),
            capability_graph_revision: revision.into(),
            acquisition_node: "calendar".into(),
            mode: "paper".into(),
            response_schema_ref:
                tradeassembly_plugin_sdk::session_calendar::SESSION_CALENDAR_SCHEMA.into(),
            input: json!({}),
        },
        &ctx("stream-worker-calendar"),
    )
    .unwrap();
    let now = s.runtime().clock.now_ms();
    let date = chrono::DateTime::from_timestamp_millis(now)
        .unwrap()
        .date_naive()
        .to_string();
    let schedule=s.handle_http("POST","/product/strategies/session-schedules/prepare",json!({"schedule":schedules::ScheduleCommand {scope:scope.clone(),capability_graph_revision:revision.into(),mode:"paper".into(),trigger_ref:"preopen".into(),session_date:date,calendar_collection_id:calendar.collection_id.clone(),execution_calendar_collection_id:calendar.collection_id,knowledge_cutoff_ms:now},"accountMode":"paper","idempotencyKey":"stream-worker-schedule"}));
    assert_eq!(schedule.status, 200, "{}", schedule.body);
    let request = dispatch::DispatchRequest {
        occurrence_id: schedule.body["schedule"]["occurrence"]["occurrenceId"]
            .as_str()
            .unwrap()
            .into(),
        scope: scope.clone(),
        capability_graph_revision: revision.into(),
        mode: "paper".into(),
        subjects: dispatch::SubjectPolicy::AllSelected { max_members: 1 },
        event_start_ms: seed.policy.event_start_ms,
        event_end_ms: seed.policy.event_end_ms,
        interval_ms: 1000,
        max_frames_per_step: 4,
        source_captures_by_subject: BTreeMap::from([(
            "SPY".into(),
            BTreeMap::from([(STREAM_SOURCE.into(), seed.capture_id.clone())]),
        )]),
        execution_activation_id: Some("fixture-v2-activation".into()),
    };
    let enqueue=s.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"enqueue","job":request},"accountMode":"paper","idempotencyKey":"stream-worker-enqueue"}));
    assert_eq!(enqueue.status, 200, "{}", enqueue.body);
    assert_eq!(enqueue.body["ok"], true, "{}", enqueue.body);
    let id = enqueue.body["body"]["job"]["jobId"].as_str().unwrap();
    let start=s.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"start_worker","jobId":id},"accountMode":"paper","idempotencyKey":"stream-worker-start"}));
    assert_eq!(start.body["ok"], true, "{}", start.body);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
    let mut removed = false;
    let mut checkpoint_restart = false;
    let mut retained_checkpoint = Value::Null;
    let final_position = loop {
        let persisted_job = s
            .runtime()
            .storage
            .get_json("session_dispatch_jobs_v1", id)
            .unwrap()
            .unwrap();
        assert_ne!(
            persisted_job["status"], "failed",
            "automatic job refused: {}",
            persisted_job
        );
        assert!(
            std::time::Instant::now() < deadline,
            "automatic stream deadline; worker={} manager={:?}",
            dispatch::worker_status_for_test(s, id),
            s.runtime()
                .storage
                .list_json("execution_management_automation_v1")
        );
        let positions = s
            .runtime()
            .storage
            .list_json("execution_managed_positions_v1")
            .unwrap();
        if !positions.is_empty() && !removed {
            dispatch::halt_worker_for_test(s, id);
            native_health_wait_worker_stopped(s, id);
            // Make batch coverage deterministic with genuine elapsed time, not
            // a clock override: the next new source query has at least eight
            // completed one-second observations available on its original grid.
            std::thread::sleep(std::time::Duration::from_millis(8250));
            // A real new screened selection, genuine B capture and normal select
            // replace the entry membership. No broker/position state is authored.
            let end = s.runtime().clock.trusted_now_ms().unwrap();
            prepare_automatic_stream_entry_plan(
                s,
                scope,
                revision,
                end - 1000,
                end,
                "B",
                "stream-removal",
            );
            let pause=s.handle_http("POST","/product/strategy-execution-activations/control",json!({"activationId":"fixture-v2-activation","action":"pause_entries","accountMode":"paper","idempotencyKey":"stream-pause-entries"}));
            assert_eq!(pause.status, 200, "{}", pause.body);
            let owner = s.invocation_owner().unwrap();
            let restored =
                crate::service::TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone())
                    .for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
            let resumed=restored.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"resume_workers"},"accountMode":"paper","idempotencyKey":"stream-resume-after-removal"}));
            assert_eq!(resumed.body["ok"], true, "{}", resumed.body);
            assert_eq!(
                resumed.body["body"]["workersResumed"], 1,
                "{}",
                resumed.body
            );
            removed = true;
        }
        let receipts = automatic_stream_receipts(s);
        if receipts.len() >= 2 && !checkpoint_restart {
            dispatch::halt_worker_for_test(s, id);
            native_health_wait_worker_stopped(s, id);
            let receipts = automatic_stream_receipts(s);
            let prior = receipts.last().unwrap();
            retained_checkpoint = serde_json::to_value(&prior.checkpoint).unwrap();
            let owner = s.invocation_owner().unwrap();
            let restored =
                crate::service::TradeAssemblyService::from_runtime(s.db(), (*s.runtime()).clone())
                    .for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
            assert_eq!(automatic_stream_receipts(&restored), receipts);
            let resumed=restored.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"resume_workers"},"accountMode":"paper","idempotencyKey":"stream-resume-checkpoint"}));
            assert_eq!(resumed.body["ok"], true, "{}", resumed.body);
            assert_eq!(
                resumed.body["body"]["workersResumed"], 1,
                "{}",
                resumed.body
            );
            checkpoint_restart = true;
        }
        if let Some((key, position)) = positions.first() {
            if position["closedQuantity"] == "0.4" && position["reservations"] == json!({}) {
                let inspected = s.handle_http(
                    "GET",
                    &format!("/product/strategies/positions/{key}"),
                    json!({}),
                );
                assert_eq!(inspected.status, 200, "{}", inspected.body);
                break inspected.body;
            }
        }
        let status = dispatch::worker_status_for_test(s, id);
        if status["running"] == false {
            assert!(status["lastError"].is_null(), "worker stopped: {}", status);
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    let stop=s.handle_http("POST","/scheduler/run",json!({"sessionDispatch":{"kind":"stop_worker","jobId":id},"accountMode":"paper","idempotencyKey":"stream-worker-stop"}));
    assert_eq!(stop.body["ok"], true, "{}", stop.body);
    assert!(removed && checkpoint_restart);
    let receipts = automatic_stream_receipts(s);
    assert!(
        receipts.len() >= 3,
        "seed + at least two acquired successors required"
    );
    let plan = &receipts[0].checkpoint.plan_revision_id;
    let run = &receipts[0].checkpoint.run_id;
    let mut frames = 0_u64;
    for (index, receipt) in receipts.iter().enumerate() {
        assert_eq!(&receipt.checkpoint.plan_revision_id, plan);
        assert_eq!(&receipt.checkpoint.run_id, run);
        assert!((1..=4).contains(&receipt.frames_processed));
        frames += receipt.frames_processed;
        assert_eq!(
            receipt.checkpoint.states["exec_signal"]["SPY"]["seen"],
            json!(frames)
        );
        assert_eq!(
            receipt.signals.last().unwrap().exit,
            crate::data_strategy::SignalValue::Ready {
                value: frames >= 18
            }
        );
        if index > 0 {
            assert_eq!(
                automatic_stream_cursor(receipt),
                automatic_stream_cursor(&receipts[index - 1])
                    + receipt.frames_processed as i64 * 1000
            );
            assert_ne!(
                receipt.checkpoint.checkpoint_sha256,
                receipts[index - 1].checkpoint.checkpoint_sha256
            );
        }
    }
    assert_eq!(
        automatic_stream_cursor(&receipts[0]),
        seed.policy.event_end_ms
    );
    let captures = s
        .runtime()
        .storage
        .list_json("data_acquisition_receipts_v1")
        .unwrap()
        .into_iter()
        .map(|(_, v)| {
            serde_json::from_value::<crate::service::data_captures::CaptureReceipt>(v).unwrap()
        })
        .filter(|c| {
            c.policy.replay_node == STREAM_SOURCE
                && c.policy.context.instruments[0].instrument_id == "SPY"
        })
        .collect::<Vec<_>>();
    assert!(
        captures.len() >= 4,
        "seed plus at least three bounded successor segments before counter18"
    );
    for capture in &captures {
        let witness = crate::service::data_captures::verified_input_witness(capture).unwrap();
        assert_eq!(witness.input["feed"], "synthetic_stream_batch_v2");
        assert_eq!(witness.input["instrument"], "SPY");
        assert_eq!(witness.input["origin"], json!(seed.policy.event_start_ms));
        assert_eq!(witness.input["start"], json!(capture.policy.event_start_ms));
        assert_eq!(witness.input["end"], json!(capture.policy.event_end_ms));
        assert_eq!(witness.input["interval"], 1000);
        assert!(
            (1000..=8000).contains(&(capture.policy.event_end_ms - capture.policy.event_start_ms))
        );
        let values = capture.replay_parameters["snapshot"]["values"]
            .as_array()
            .unwrap();
        assert_eq!(
            values.len() as i64 * 1000,
            capture.policy.event_end_ms - capture.policy.event_start_ms
        );
        for (index, value) in values.iter().enumerate() {
            assert_eq!(
                *value,
                json!(
                    100 + (capture.policy.event_start_ms - seed.policy.event_start_ms) / 1000
                        + index as i64
                )
            );
        }
    }
    let mut heads = s
        .runtime()
        .storage
        .list_json("managed_capture_stream_heads_v1")
        .unwrap();
    assert!(heads.len() >= 3);
    heads.sort_by_key(|(_, head)| head["sequence"].as_u64().unwrap());
    let snapshots = s
        .runtime()
        .storage
        .list_json("managed_stream_step_snapshots_v1")
        .unwrap();
    for (index, (_, head)) in heads.iter().enumerate() {
        assert_eq!(head["sequence"], json!(index));
        let span = head["eventEndMs"].as_i64().unwrap() - head["eventStartMs"].as_i64().unwrap();
        assert!((1000..=8000).contains(&span));
        assert_eq!(span % 1000, 0);
        if index > 0 {
            assert_eq!(head["eventStartMs"], heads[index - 1].1["eventEndMs"]);
            assert_eq!(head["previousHeadSha256"], heads[index - 1].1["headSha256"]);
        }
    }
    for receipt in &receipts {
        let snapshot = snapshots
            .iter()
            .map(|(_, v)| &v["snapshot"])
            .find(|v| v["snapshotSha256"] == receipt.source_snapshot_sha256.as_deref().unwrap())
            .unwrap();
        assert_eq!(
            snapshot["window"]["eventEndMs"],
            json!(automatic_stream_cursor(receipt))
        );
        assert_eq!(
            snapshot["window"]["eventEndMs"].as_i64().unwrap()
                - snapshot["window"]["eventStartMs"].as_i64().unwrap(),
            receipt.frames_processed as i64 * 1000
        );
        let segments = snapshot["sources"][STREAM_SOURCE]["segments"]
            .as_array()
            .unwrap();
        assert!(!segments.is_empty());
        let from = snapshot["window"]["eventStartMs"].as_i64().unwrap();
        let to = snapshot["window"]["eventEndMs"].as_i64().unwrap();
        let covered = segments
            .iter()
            .map(|segment| {
                let start = segment["eventStartMs"].as_i64().unwrap().max(from);
                let end = segment["eventEndMs"].as_i64().unwrap().min(to);
                (end - start).max(0)
            })
            .sum::<i64>();
        assert_eq!(
            covered,
            to - from,
            "exact admitted segment intersections cover every requested frame"
        );
    }
    assert!(
        receipts.iter().any(|r| r.frames_processed > 1),
        "actual batched evaluation required"
    );
    assert!(
        captures
            .iter()
            .any(|c| c.policy.event_end_ms - c.policy.event_start_ms > 4000),
        "a capture larger than one evaluation window is required"
    );
    let position_id = final_position["position"]["positionId"].as_str().unwrap();
    let rules = &final_position["position"]["policy"]["rules"];
    assert!(
        rules["stopLossRatio"].is_null()
            && rules["trailingRatio"].is_null()
            && rules["trailingStart"].is_null()
    );
    assert_eq!(rules["precedence"], json!(["reversal"]));
    assert_eq!(rules["reversalSignal"], "typed.exit");
    let triggers = s
        .runtime()
        .storage
        .list_json("execution_managed_exit_triggers_v1")
        .unwrap()
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| v["positionId"] == position_id)
        .collect::<Vec<_>>();
    assert_eq!(triggers.len(), 1, "exactly one original reversal trigger");
    assert_eq!(triggers[0]["reason"], "reversal");
    assert_eq!(
        triggers[0]["price"], "90",
        "the normal synthetic managed quote remains explicit"
    );
    let reversal_id = triggers[0]["reversalReceiptSha256"].as_str().unwrap();
    let reversal = receipts
        .iter()
        .find(|r| r.receipt_sha256 == reversal_id)
        .unwrap();
    assert!(
        reversal.checkpoint.states["exec_signal"]["SPY"]["seen"]
            .as_u64()
            .unwrap()
            >= 18
    );
    assert_eq!(
        reversal.signals.last().unwrap().exit,
        crate::data_strategy::SignalValue::Ready { value: true }
    );
    let frame_end = reversal.signals.last().unwrap().event_end_ms;
    assert!(
        frame_end
            > final_position["position"]["entryTime"]["observedAtMs"]
                .as_i64()
                .unwrap()
    );
    let trigger_time = triggers[0]["knownAtMs"].as_i64().unwrap();
    assert!(
        frame_end <= trigger_time && trigger_time - frame_end < 30000,
        "actual signal freshness guard retained"
    );
    let close_plans = s
        .runtime()
        .storage
        .list_json("execution_order_plans_v1")
        .unwrap()
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| v["management"]["positionId"] == position_id)
        .collect::<Vec<_>>();
    assert_eq!(close_plans.len(), 1);
    assert_eq!(
        close_plans[0]["management"]["triggerSha256"],
        triggers[0]["triggerSha256"]
    );
    assert_eq!(close_plans[0]["order"]["positionAction"], "sell_to_close");
    let calls = fs::read_to_string(root.join("synthetic-broker/calls.log")).unwrap();
    assert_eq!(calls.lines().filter(|v| *v == "submit").count(), 2);
    assert_eq!(calls.lines().filter(|v| *v == "lookup").count(), 2);
    assert_eq!(
        fs::read_to_string(root.join("synthetic-broker/close-settlement.log"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    write_screen_interface_evidence(
        "automatic_stream",
        "checkpointed-background-reversal.json",
        &json!({"position":final_position,"receipts":receipts,"captures":captures,"heads":heads,"retainedCheckpointAtRecreation":retained_checkpoint,"entryMembershipReplacedBy":"B","entriesPaused":true,"serviceRecreatedAfterRegistrationAndCheckpoint":true,"actualCoreOsProcessRestartQualified":false,"enforcingSrtQualified":false,"actualPackMathQualified":false,"realProviderCalls":0,"syntheticIntervalMs":1000,"maxCaptureBars":8,"maxEvaluationFrames":4,"counterTrueAt":18,"unchangedMaxReversalAgeMs":30000,"unchangedC5ExpiryMs":45000,"qualification":"native Core service plus actual separately declared synthetic plugin subprocesses; diagnostic sandbox and mock Warden; normal automatic C3 capture/stream/step and ordinary C5 broker/settlement; requires separate frozen Core/SRT process gate"}),
    );
}
#[test]
#[ignore = "explicit native synthetic one-second captured stream/checkpoint/automatic C5 fixture; diagnostic sandbox, not SRT"]
fn native_automatic_batched_captured_stream_reversal_preserves_checkpoint_after_membership_removal_and_restart(
) {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    native_execution_plan_fixture(Some(AUTOMATIC_STREAM_CASE));
}
