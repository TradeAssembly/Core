// Canonical typed-backtest native process fixture: declared synthetic source,
// diagnostic sandbox/test principal, no SRT, real provider or broker claim.
const TYPED_RESEARCH_PLUGIN: &str = "fixture.typed_research";
fn typed_research_executable() -> &'static [u8] {
    static BINARY: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BINARY.get_or_init(|| {
        let deps = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("typed-research-fixture");
        let mut command = std::process::Command::new("rustc");
        command
            .args(["--edition=2021", "--crate-name", "typed_research_fixture"])
            .arg(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("src/adapters/fixtures/typed_research_plugin.rs"),
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
fn install_typed_research_package(
    s: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
) -> InstalledPluginPackage {
    use crate::adapters::plugin_packages::data_package_tests::{archive_files, materials};
    let (mut descriptor, contract, series_bytes, _) = materials();
    let mut source: Value = serde_json::from_slice(&contract).unwrap();
    source["schemaVersion"] = json!("tradeassembly.operation_data.v2");
    source["operationId"] = json!("research_replay");
    source["semanticsVersion"] = json!("synthetic_typed_research_v1");
    source["outputs"]["value"]["time"] = json!({"intervalMs":1000,"completedOnly":true});
    source["inputs"] = json!({});
    source["state"] = Value::Null;
    source["warmup"] =
        json!({"minimumObservations":1,"observationsParameter":null,"additionalObservations":0});
    source["applicability"] = json!({"assetClasses":["equity"],"instrumentFamilies":["equity"],"modes":["paper","backtest"]});
    source["parameterSchema"] = json!({"type":"object","properties":{"snapshot":{"type":"object"},"digest":{"type":"string"}},"required":["snapshot","digest"],"additionalProperties":false});
    let state_schema = json!({"type":"object","properties":{"seen":{"type":"integer","minimum":0}},"required":["seen"],"additionalProperties":false});
    let mut counter = source.clone();
    counter["operationId"] = json!("research_counter");
    counter["inputs"] = json!({"source":{"data":source["outputs"]["value"],"required":true,"dependencyId":"source"}});
    counter["parameterSchema"] = json!({"type":"object","additionalProperties":false});
    counter["warmup"]["minimumObservations"] = json!(2);
    counter["state"] = json!({"schema":{"schemaRef":"schema://fixture-typed-research/state@1","sha256":canonical_sha256(&state_schema).unwrap()},"version":"synthetic_research_counter_v1","partition":"instrument"});
    let history = json!({"schemaVersion":"tradeassembly.operation_data.v3","operation":counter,
        "history":{"firstReady":{"value":{"source":{"op":"literal","value":2}}},"partitioning":"instrument_independent",
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
            "research_acquire",
            "test.typed_research.acquire@1",
            false,
            vec!["schema://fixture-typed-research/acquisition@1"],
        ),
        (
            "research_replay",
            "test.typed_research.replay@1",
            true,
            vec!["schema://test/series@1", EVALUATION_RESPONSE_SCHEMA],
        ),
        (
            "research_counter",
            "test.typed_research.counter@1",
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
        op["traits"] = json!({"modes":["paper","backtest"],"instrumentFamilies":["equity"],"inputSchemaRefs":if id=="research_counter"{vec!["schema://test/series@1"]}else{vec![]},
            "outputSchemaRefs":schemas,"deterministic":pure,"replayable":pure});
        operations.push(op);
    }
    let manifest = json!({"metadata":{"id":TYPED_RESEARCH_PLUGIN,"version":"1","provider":{"id":TYPED_RESEARCH_PLUGIN,"name":"Synthetic typed research state counter"}},
        "capabilities":operations.iter().map(|op|json!({"id":op["capability"]})).collect::<Vec<_>>(),"operations":operations,"permissions":[],
        "configuration":{"fields":[]},"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let binary = typed_research_executable();
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    descriptor["packageContractVersion"] = json!("3");
    descriptor["recipes"] = json!([]);
    descriptor["package"]["plugin"]["id"] = json!(TYPED_RESEARCH_PLUGIN);
    descriptor["package"]["plugin"]["version"] = json!("1");
    descriptor["package"]["manifest"]["sha256"] = json!(sha256(&manifest_bytes));
    descriptor["package"]["targets"][0]["binary"]["sha256"] = json!(sha256(binary));
    descriptor["dataContracts"] = json!([]);
    descriptor["dataSchemas"] = json!([]);
    descriptor["package"]["responseSchemas"] = json!([]);
    let mut files = vec![
        ("manifest.json".to_string(), manifest_bytes),
        ("bin/plugin".into(), binary.to_vec()),
    ];
    for (name, value) in [("research_replay", source), ("research_counter", history)] {
        let path = format!("contract-{name}.json");
        let bytes = serde_json::to_vec(&value).unwrap();
        descriptor["dataContracts"].as_array_mut().unwrap().push(json!({"operationId":name,"document":{"path":path,"sha256":sha256(&bytes)},"canonicalSha256":canonical_sha256(&value).unwrap()}));
        files.push((path, bytes));
    }
    let series: Value = serde_json::from_slice(&series_bytes).unwrap();
    for (name, value) in [
        ("schema://test/series@1", series),
        ("schema://fixture-typed-research/state@1", state_schema),
        (EVALUATION_RESPONSE_SCHEMA, json!({"type":"object"})),
        (
            "schema://fixture-typed-research/acquisition@1",
            json!({"type":"object"}),
        ),
    ] {
        let path = format!("schema-{}.json", files.len());
        let bytes = serde_json::to_vec(&value).unwrap();
        descriptor["dataSchemas"].as_array_mut().unwrap().push(json!({"schemaRef":name,"document":{"path":path,"sha256":sha256(&bytes)},"canonicalSha256":canonical_sha256(&value).unwrap()}));
        if matches!(
            name,
            EVALUATION_RESPONSE_SCHEMA | "schema://fixture-typed-research/acquisition@1"
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
    s.runtime().plugins.install_manifest(TYPED_RESEARCH_PLUGIN,json!({"pluginRef":TYPED_RESEARCH_PLUGIN,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx("stream-fixture-install")).unwrap();
    s.runtime().plugins.put_instance(TYPED_RESEARCH_PLUGIN,json!({"instanceRef":TYPED_RESEARCH_PLUGIN,"pluginRef":TYPED_RESEARCH_PLUGIN,"providerRef":TYPED_RESEARCH_PLUGIN,"enabled":true,"activePackageSha256":package.package_sha256,"configuration":{}}),&ctx("stream-fixture-instance")).unwrap();
    package
}
fn typed_research_scope(
    s: &crate::service::TradeAssemblyService,
    p: &InstalledPluginPackage,
) -> (data_plans::PlanScope, Value) {
    let context = DataContext {
        instruments: vec![InstrumentContext {
            instrument_id: "SPY".into(),
            asset_class: "equity".into(),
            instrument_family: "equity".into(),
        }],
        interval_ms: Some(1000),
    };
    let value = crate::data_strategy::Operand::Output {
        node: "research_counter".into(),
        port: "value".into(),
        instrument: "SPY".into(),
        sample: crate::data_strategy::Sample::Current,
    };
    let program = crate::data_strategy::DataStrategy {
        version: 1,
        nodes: vec![
            PipelineNode {
                graph_node_id: "research_source".into(),
                parameters: json!({}),
                context: context.clone(),
                input_contexts: BTreeMap::new(),
            },
            PipelineNode {
                graph_node_id: "research_counter".into(),
                parameters: json!({}),
                context: context.clone(),
                input_contexts: BTreeMap::new(),
            },
        ],
        bindings: vec![PortBinding {
            producer_node: "research_source".into(),
            output_port: "value".into(),
            consumer_node: "research_counter".into(),
            input_port: "source".into(),
        }],
        trade_instruments: context.instruments,
        entry: crate::data_strategy::Rule::Compare {
            left: value.clone(),
            op: crate::data_strategy::Comparison::Greater,
            right: crate::data_strategy::Operand::Number { value: 0.0 },
        },
        exit: crate::data_strategy::Rule::Compare {
            left: value,
            op: crate::data_strategy::Comparison::Less,
            right: crate::data_strategy::Operand::Number { value: 0.0 },
        },
    };
    let mut spec: Value = serde_json::from_str(include_str!(
        "../../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .unwrap();
    spec["strategy_id"] = json!("synthetic-typed-research");
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({(crate::data_strategy::CONFIG_KEY):program});
    spec["stages"]["exit_policy"]["policy"] = json!({"monitoring":["synthetic"],"triggers":[{"id":"typed_exit","kind":"indicator","scope":"full_position"}],"composition":"any","precedence":[],"end_of_data":"preserve_open_positions"});
    spec["stages"]["order_strategy"]["policy"]["allowed_order_types"] = json!(["market"]);
    spec["stages"]["price_policy"]["policy"]["tick_size"] = Value::Null;
    spec["stages"]["market_clock"]["policy"]["timezone"] = json!("Etc/UTC");
    spec["stages"]["market_clock"]["policy"]["calendar_refs"] = json!(["SYNTHETIC_RESEARCH"]);
    spec["stages"]["market_clock"]["policy"]["windows"] = json!([{"start":"00:00","end":"24:00"}]);
    spec["stages"]["market_clock"]["policy"]["days"] = json!([
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday"
    ]);
    let mut bindings = json!({"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}});
    for (node, operation, capability) in [
        (
            "research_source",
            "research_replay",
            "test.typed_research.replay@1",
        ),
        (
            "research_counter",
            "research_counter",
            "test.typed_research.counter@1",
        ),
        (
            "research_acquisition",
            "research_acquire",
            "test.typed_research.acquire@1",
        ),
    ] {
        let mut requirement = spec["capability_requirements"]["required"][0].clone();
        requirement["requirement_id"] = json!(node);
        requirement["capability"] = json!(capability);
        requirement["required_for"] = json!(["research", "backtest", "paper"]);
        requirement["stage_refs"] = json!(["evaluate"]);
        requirement["substep_refs"] = json!([]);
        requirement["dependency_refs"] = json!([]);
        requirement["constraints"] = json!({"schema_refs":[],"data_shapes":[],"fields":[],"input_paths":[],"operations":[],"deterministic":false,"replayable":false});
        spec["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap()
            .push(requirement);
        bindings[node] = json!({"pluginInstanceRef":p.plugin_ref,"pluginRef":p.plugin_ref,"operationId":operation});
        s.runtime().storage.put_json("entitlement_grants",capability,json!({"grantId":capability,"profile":"local_full","source":"test","capability":capability,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx("typed-research-entitlement")).unwrap();
    }
    let created=s.handle_http("POST","/product/strategies/create",json!({"id":"synthetic-typed-research","name":"Synthetic typed research","spec":spec,"idempotencyKey":"typed-research-create"}));
    assert_eq!(created.status, 201, "{}", created.body);
    let strategy = created.body["body"]["strategy"]["id"].as_str().unwrap();
    let published=s.handle_http("POST","/product/strategies/publish",json!({"strategyId":strategy,"expectedDraftHash":created.body["body"]["draft"]["draftHash"],"idempotencyKey":"typed-research-publish"}));
    assert_eq!(published.status, 200, "{}", published.body);
    assert_eq!(
        published.body["body"]["published"], true,
        "{}",
        published.body
    );
    (
        data_plans::PlanScope {
            strategy_id: strategy.into(),
            strategy_version_id: published.body["body"]["version"]["id"]
                .as_str()
                .unwrap()
                .into(),
            strategy_spec_hash: published.body["body"]["version"]["specHash"]
                .as_str()
                .unwrap()
                .into(),
        },
        bindings,
    )
}
fn typed_research_capture_plan(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    bindings: &Value,
    mode: &str,
    start: i64,
    end: i64,
) -> (
    data_plans::StoredDataPlan,
    crate::service::data_captures::CaptureReceipt,
) {
    let response=s.handle_http("POST","/capability-graph-revisions",
        json!({"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":mode,"bindings":bindings,"idempotencyKey":format!("typed-graph-{mode}")}),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let graph = response.body;
    assert_eq!(graph["revision"]["graph"]["ok"], true, "{graph}");
    let revision = graph["revision"]["revisionId"].as_str().unwrap();
    let nodes = graph["revision"]["graph"]["nodes"].as_array().unwrap();
    let resolved_node = |operation: &str| {
        let matched = nodes
            .iter()
            .filter(|node| node["selected"]["operationId"] == operation)
            .collect::<Vec<_>>();
        assert_eq!(
            matched.len(),
            1,
            "exact published operation must resolve once: {graph}"
        );
        matched[0]["nodeId"].as_str().unwrap().to_string()
    };
    let acquisition_node = resolved_node("research_acquire");
    let source_node = resolved_node("research_replay");
    assert_eq!(acquisition_node, "research_acquisition");
    assert_eq!(
        source_node, "research_source",
        "normal graph ID must match the published typed program"
    );
    assert_eq!(resolved_node("research_counter"), "research_counter");
    let captured=s.handle_http("POST","/product/strategies/data-captures/create",json!({"capture":{"receiptVersion":"tradeassembly.data_capture_receipt.v2","policy":{"scope":scope,"capabilityGraphRevision":revision,"mode":mode,"acquisitionNode":acquisition_node,"replayNode":source_node,"parameterExtracts":{"snapshot":"/snapshot","digest":"/digest"},"context":{"instruments":[{"instrumentId":"SPY","assetClass":"equity","instrumentFamily":"equity"}],"intervalMs":1000},"eventStartMs":start,"eventEndMs":end,"gapPolicy":"strict_fixed_interval"},"input":{"instrument":"SPY","start":start,"end":end,"origin":start,"cutoff":s.runtime().clock.trusted_now_ms().unwrap(),"interval":1000,"feed":"synthetic_typed_research_v1"}},"accountMode":"paper","idempotencyKey":format!("typed-capture-{mode}")}));
    assert_eq!(captured.status, 200, "{}", captured.body);
    let capture = crate::service::data_captures::current(
        s,
        captured.body["capture"]["captureId"].as_str().unwrap(),
    )
    .unwrap();
    assert!(capture.input_witness.is_some());
    assert_eq!(capture.replay_response.outputs["value"].points.len(), 6);
    let prepared=s.handle_http("POST","/product/strategies/data-plans/prepare",json!({"scope":scope,"capabilityGraphRevisionId":revision,"mode":mode,"sourceCaptures":{(source_node):capture.capture_id},"idempotencyKey":format!("typed-prepare-{mode}")}));
    assert_eq!(prepared.status, 200, "{}", prepared.body);
    (
        serde_json::from_value(prepared.body["plan"].clone()).unwrap(),
        capture,
    )
}
fn typed_research_fill_series(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    plan: &data_plans::StoredDataPlan,
    start: i64,
    end: i64,
) -> crate::historical_data::DatasetSnapshot {
    use crate::historical_data::*;
    let stamp = |time| {
        chrono::DateTime::from_timestamp_millis(time)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    let observations = [100., 110., 90., 95., 95., 96.]
        .into_iter()
        .enumerate()
        .map(|(index, price)| HistoricalObservation {
            instrument_id: "SPY".into(),
            timestamp: stamp(start + (index as i64 + 1) * 1000),
            data: HistoricalObservationData::Bar(BarObservation {
                open: price,
                high: price,
                low: price,
                close: price,
                volume: 100.,
                vwap: None,
                session: Some(BarSession {
                    calendar: "SYNTHETIC_RESEARCH".into(),
                    start: stamp(start - 1000),
                    end: stamp(end + 1000),
                    source_ref: "fixture:explicit-synthetic-session".into(),
                }),
            }),
        })
        .collect();
    let source = DatasetSourceManifest {
        source_class: DatasetSourceClass::Test,
        plugin_instance_ref: "explicit-synthetic-fill-series".into(),
        plugin_ref: "fixture.synthetic_fill_assumptions".into(),
        operation_id: "test.explicit_fill_series".into(),
        plugin_manifest_fingerprint: canonical_sha256(&json!({"schema":"fixture.fill-series.v1"}))
            .unwrap(),
        capability_graph_revision_id: plan.content.plan.request.capability_graph_revision.clone(),
        capability_graph_fingerprint: plan.content.plan.capability_graph_fingerprint.clone(),
        source_request_hash: canonical_sha256(&json!({"start":start,"end":end,"fixture":true}))
            .unwrap(),
        upstream_refs: BTreeMap::new(),
    };
    let snapshot = DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.into(),
            strategy_id: scope.strategy_id.clone(),
            strategy_version_id: Some(scope.strategy_version_id.clone()),
            source,
            asset_classes: vec![AssetClass::Equity],
            instruments: vec!["SPY".into()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1s".into(),
            time_slice: DatasetTimeSlice {
                start: stamp(start + 1000),
                end: stamp(end),
            },
            calendar: "SYNTHETIC_RESEARCH".into(),
            timezone: "Etc/UTC".into(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".into(),
                timezone: "Etc/UTC".into(),
                duplicate_policy: "reject".into(),
                price_adjustment: "none".into(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: QualityDisposition::Reject,
                stale_observations: QualityDisposition::Reject,
                outliers: QualityDisposition::Allow,
                invalid_markets: QualityDisposition::Reject,
                calendar_mismatch: QualityDisposition::Reject,
                corporate_action_gaps: QualityDisposition::Reject,
                missing_derivative_fields: QualityDisposition::Allow,
            },
            quality_findings: vec![],
            observations,
        },
        s.runtime().clock.now_ms(),
    )
    .unwrap();
    s.runtime()
        .dataset_snapshots
        .put(&snapshot, &ctx("typed-fill-series"))
        .unwrap();
    s.bind_inherited_object(
        "dataset_snapshot",
        &snapshot.dataset_id,
        "strategy",
        &scope.strategy_id,
    )
    .unwrap();
    snapshot
}
fn typed_research_configuration(
    s: &crate::service::TradeAssemblyService,
    scope: &data_plans::PlanScope,
    plan: &data_plans::StoredDataPlan,
    capture: &crate::service::data_captures::CaptureReceipt,
    snapshot: &crate::historical_data::DatasetSnapshot,
) -> crate::backtest_contracts::BacktestConfiguration {
    let source = &snapshot.content.source;
    serde_json::from_value(json!({"schema":"tradeassembly.backtest-configuration.v2","typedEvaluation":{"schemaVersion":"tradeassembly.typed-backtest-binding.v1","planRevisionId":plan.revision_id,"planRevisionSha256":plan.revision_sha256,"sourceCaptures":{"research_source":{"captureId":capture.capture_id,"receiptSha256":capture.receipt_sha256}},"eventStartMs":capture.policy.event_start_ms,"eventEndMs":capture.policy.event_end_ms,"asOfMs":s.runtime().clock.trusted_now_ms().unwrap(),"intervalMs":1000,"maxFramesPerStep":2,"maximumFrames":6,"quantityMicros":1000000,"initialPosition":"flat","knowledgePolicy":"retrospective_capture_cutoff","fillTimestampBasis":"event_end"},"strategy":{"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash},"dataset":{"datasetId":snapshot.dataset_id,"snapshotRef":snapshot.snapshot_ref,"contentHash":snapshot.content_hash,"schema":snapshot.content.schema,"sourcePluginRef":source.plugin_ref,"sourcePluginInstanceRef":source.plugin_instance_ref,"sourceOperationId":source.operation_id,"sourceManifestFingerprint":source.plugin_manifest_fingerprint,"sourceClass":"test","capabilityGraphRevisionId":source.capability_graph_revision_id,"capabilityGraphFingerprint":source.capability_graph_fingerprint,"instruments":["SPY"],"selectedTimeSlice":snapshot.content.time_slice,"granularity":"1s","calendar":"SYNTHETIC_RESEARCH","timezone":"Etc/UTC","corporateActionPolicy":"none","benchmark":null},"capabilityGraph":{"revisionId":plan.content.plan.request.capability_graph_revision,"graphFingerprint":plan.content.plan.capability_graph_fingerprint},"capital":{"startingCashMicros":1000000000,"reportingCurrency":"USD","accountModel":"cash","maximumLeverageMicros":1000000,"marginPolicy":"none"},"execution":{"evaluationTiming":"bar_close","fillTiming":"next_bar","fillPriceSource":"open","orderType":"market","timeInForce":"day","fillPolicy":"full","rejectOnInsufficientCapital":true,"rejectOnInsufficientLiquidity":true,"staleDataPolicy":"reject","missingDataPolicy":"reject"},"costs":{"fixedFeeMicros":10000,"perUnitFeeMicros":0,"notionalFeeBps":0,"spreadBps":10,"slippageBps":20},"liquidity":{"maximumParticipationBps":10000,"minimumVolumeMicros":0},"instruments":[{"instrumentId":"SPY","instrumentFamily":"equity","quantityUnit":"shares","quantityScale":6,"priceScale":6,"contractMultiplierMicros":1000000,"settlement":"cash","metadata":{}}],"risk":{"maximumPositionNotionalMicros":1000000000,"maximumOrderQuantityMicros":10000000,"maximumLossMicros":1000000000,"maximumOpenPositions":1},"endOfData":"mark_to_market","evaluatorVersion":"tradeassembly.typed-backtest.v1","compilerVersion":"tradeassembly.typed-backtest.v1","pluginFingerprints":{},"variableOverrides":{}})).unwrap()
}
#[test]
#[ignore = "explicit generic native pure-plugin capture/typed backtest/queue/restart/recompute fixture; diagnostic sandbox/test principal, not SRT"]
fn native_typed_backtest_uses_real_publisher_state_and_shared_simulation_after_restart() {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    let operations = Arc::new(ForwardNative::default());
    let (dir, base, _, _) = data_plans::tests::fixture_service(Some(operations.clone()));
    let service = base.for_authenticated_invocation("fixture", "typed-research-owner", None, None);
    let package = install_typed_research_package(&service, dir.path());
    for instance in service.runtime().plugins.list_instances().unwrap() {
        service
            .bind_owned_object("plugin_instance", instance["instanceRef"].as_str().unwrap())
            .unwrap();
    }
    *operations.0.lock().unwrap() = Some(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            service.runtime().storage.clone(),
            service.runtime().plugins.clone(),
            service.runtime().credentials.clone(),
            service.runtime().plugin_packages.clone(),
            service.runtime().clock.clone(),
            Arc::new(TracedFixtureSandbox),
        ),
    );
    let (scope, bindings) = typed_research_scope(&service, &package);
    let end = service.runtime().clock.trusted_now_ms().unwrap();
    let start = end - 6000;
    let (plan, capture) =
        typed_research_capture_plan(&service, &scope, &bindings, "backtest", start, end);
    let snapshot = typed_research_fill_series(&service, &scope, &plan, start, end);
    let config = typed_research_configuration(&service, &scope, &plan, &capture, &snapshot);
    assert!(config.validate().ready);
    let created = service.handle_http(
        "POST",
        "/backtests",
        json!({"configuration":config,"idempotencyKey":"typed-canonical-backtest"}),
    );
    assert_eq!(created.status, 202, "{}", created.body);
    let id = created.body["runId"].as_str().unwrap();
    let first = service.handle_http(
        "POST",
        "/backtests:process",
        json!({"runId":id,"worker":"typed-native","idempotencyKey":"typed-step-1"}),
    );
    assert_eq!(first.status, 202, "{}", first.body);
    let before = service
        .runtime()
        .storage
        .list_json("typed_backtest_progress_v1")
        .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].1["cursorMs"], start + 2000);
    let restored = crate::service::TradeAssemblyService::from_runtime(
        service.db(),
        (*service.runtime()).clone(),
    )
    .for_authenticated_invocation("fixture", "typed-research-owner", None, None);
    assert_eq!(
        restored
            .runtime()
            .storage
            .list_json("typed_backtest_progress_v1")
            .unwrap(),
        before
    );
    let mut completed = Value::Null;
    for index in 2..=5 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let response=restored.handle_http("POST","/backtests:process",json!({"runId":id,"worker":"typed-native","idempotencyKey":format!("typed-step-{index}")}));
        assert!(matches!(response.status, 200 | 202), "{}", response.body);
        if response.body["status"] == "completed" {
            completed = response.body;
            break;
        }
    }
    assert_eq!(completed["status"], "completed", "{completed}");
    assert_eq!(
        completed["result"]["content"]["schema"],
        "tradeassembly.backtest-result.v2"
    );
    let result = restored
        .runtime()
        .backtests
        .get_result(id)
        .unwrap()
        .unwrap();
    let evidence = result.content.typed_evaluation.as_ref().unwrap();
    assert_eq!(evidence.frames_processed, 6);
    assert_eq!(evidence.warming_frames, 1);
    assert_eq!(evidence.evaluation_receipts.len(), 3);
    assert_eq!(evidence.equity.len(), 6);
    assert_eq!(result.content.fills.len(), 2);
    assert_eq!(
        result.content.accounting["endingEquityMicros"],
        1004425000_i64
    );
    assert_eq!(evidence.max_drawdown_bps, 2);
    assert_eq!(result.content.fills[0]["executionBarIndex"], 2);
    assert_eq!(
        result.content.fills[0]["executionSourceBarTimestamp"],
        snapshot.content.observations[2].timestamp
    );
    let actual_execution = chrono::DateTime::parse_from_rfc3339(
        result.content.fills[0]["executionTimestamp"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
    .timestamp_millis();
    assert_eq!(
        actual_execution,
        start + 2000,
        "next bar OPEN is prior bar end boundary"
    );
    // A genuinely distinct immutable fill series may label bars by their open.
    // Its explicit basis must map to the same evaluator end grid; using the
    // wrong basis refuses instead of borrowing a future row or shifting signals.
    let mut start_content = snapshot.content.clone();
    for row in &mut start_content.observations {
        let raw = chrono::DateTime::parse_from_rfc3339(&row.timestamp)
            .unwrap()
            .timestamp_millis();
        row.timestamp = chrono::DateTime::from_timestamp_millis(raw - 1000)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    }
    start_content.time_slice.start = start_content
        .observations
        .first()
        .unwrap()
        .timestamp
        .clone();
    start_content.time_slice.end = start_content.observations.last().unwrap().timestamp.clone();
    let start_snapshot = crate::historical_data::DatasetSnapshot::build(
        start_content,
        restored.runtime().clock.now_ms(),
    )
    .unwrap();
    let mut start_config =
        typed_research_configuration(&restored, &scope, &plan, &capture, &start_snapshot);
    assert!(crate::service::typed_backtest::validate_dataset(
        &restored,
        &start_config,
        &start_snapshot
    )
    .is_err());
    start_config
        .typed_evaluation
        .as_mut()
        .unwrap()
        .fill_timestamp_basis = "event_start".into();
    crate::service::typed_backtest::validate_dataset(&restored, &start_config, &start_snapshot)
        .unwrap();
    let first_decision_ms = chrono::DateTime::parse_from_rfc3339(
        result.content.fills[0]["decisionTimestamp"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
    .timestamp_millis();
    assert_eq!(
        first_decision_ms, actual_execution,
        "end-of-bar decision uses the following bar's open, never that bar's close"
    );
    assert_eq!(result.content.decisions[0]["kind"], "signal_unavailable");
    let report = restored.handle_http("GET", &format!("/backtests/{id}/report"), json!({}));
    assert_eq!(report.status, 200, "{}", report.body);
    assert_eq!(
        report.body["report"]["schema"],
        "tradeassembly.backtest-report.v3"
    );
    assert_eq!(report.body["report"]["overview"]["tradeCount"], 1);
    assert_eq!(
        report.body["report"]["typedEvaluation"]["pointInTimeQualified"],
        false
    );
    let decoded: crate::backtest_report::BacktestReport =
        serde_json::from_value(report.body["report"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        report.body["report"]
    );
    let calls_before = restored
        .runtime()
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap()
        .len();
    let replay = restored.handle_http(
        "POST",
        &format!("/backtests/{id}/replay"),
        json!({"idempotencyKey":"typed-recompute"}),
    );
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(replay.body["status"], "verified");
    assert!(
        restored
            .runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap()
            .len()
            > calls_before,
        "fresh evaluator run forces actual pure-plugin calls"
    );
    assert_eq!(
        restored
            .runtime()
            .backtests
            .get_result(id)
            .unwrap()
            .unwrap(),
        result
    );
    let (paper_plan, paper_capture) =
        typed_research_capture_plan(&restored, &scope, &bindings, "paper", start, end);
    let cutoff = restored.runtime().clock.trusted_now_ms().unwrap();
    assert!(paper_capture.acquisition_response.observed_at_ms <= cutoff);
    let mut checkpoint = None;
    let mut paper_signals = Vec::new();
    let mut paper_final = None;
    for index in 0..3 {
        let step = data_evaluation::EvaluationStep {
            run_id: "typed-paper-parity".into(),
            plan_revision_id: paper_plan.revision_id.clone(),
            window: EvaluationWindow {
                event_start_ms: start + index * 2000,
                event_end_ms: start + (index + 1) * 2000,
                as_of_ms: cutoff,
            },
            expected_checkpoint_sha256: checkpoint,
        };
        let response=restored.handle_http("POST","/product/strategies/data-runs/step",json!({"scope":scope,"step":step,"accountMode":"paper","idempotencyKey":format!("typed-paper-{index}")}));
        assert_eq!(response.status, 200, "{}", response.body);
        let receipt: crate::service::data_evaluation::EvaluationReceipt =
            serde_json::from_value(response.body["receipt"].clone()).unwrap();
        checkpoint = Some(receipt.checkpoint.checkpoint_sha256.clone());
        paper_signals.extend(receipt.signals.clone());
        paper_final = Some(receipt.checkpoint);
    }
    assert_eq!(
        paper_signals.len(),
        6,
        "paper evaluator must cover all six frames"
    );
    assert_eq!(
        evidence.signals.len(),
        6,
        "research must cover all six frames"
    );
    assert_eq!(
        paper_signals.len(),
        evidence.signals.len(),
        "parity cannot compare only a common prefix"
    );
    for (paper, research) in paper_signals.iter().zip(&evidence.signals) {
        assert_eq!(paper.event_end_ms, research["eventEndMs"].as_i64().unwrap());
        assert_eq!(
            serde_json::to_value(&paper.entry).unwrap(),
            research["entry"]
        );
        assert_eq!(serde_json::to_value(&paper.exit).unwrap(), research["exit"]);
    }
    let final_receipt = crate::service::data_evaluation::indexed_receipt(
        &restored,
        evidence.evaluation_receipts.last().unwrap(),
    )
    .unwrap()
    .1;
    assert_eq!(paper_final.unwrap().states, final_receipt.checkpoint.states);
    assert!(restored
        .runtime()
        .storage
        .list_json("execution_order_plan_submissions_v2")
        .unwrap()
        .is_empty());
    write_screen_interface_evidence(
        "typed_research",
        "canonical-native-parity.json",
        &json!({"configuration":config,"backtest":completed,"report":report.body,"replay":replay.body,"paperSignals":paper_signals,"qualification":"actual native synthetic pure plugin subprocesses and canonical typed research queue/state/replay with service recreation in the same Core process; diagnostic sandbox/test identity; no Core OS-process restart, SRT, normal local-owner bootstrap, live provider/history entitlement, full sizing/risk/stop/screen parity or actual pack formula qualification"}),
    );
}
