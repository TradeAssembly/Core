// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Actual local service/storage and installed-byte validation with a controlled
//! pure operation port. No native provider, credentials, sockets or broker calls.
use super::*;
use crate::data_pipeline::{DataContext, InstrumentContext, PipelineNode};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tradeassembly_plugin_sdk::data_contract::*;
const START: i64 = 1767225600000; // 2026-01-01 UTC, explicit synthetic history.
const RAW: &str = "schema://fixture/projection-acquisition@1";
const CLOSE: &str = "schema://fixture/projection-close-series@1";
const PLUGIN: &str = "fixture.capture_projection";
#[derive(Default)]
struct Operations {
    calls: Mutex<Vec<String>>,
    excess_precision: AtomicBool,
}
impl VersionedPort for Operations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl PluginOperationPort for Operations {
    fn invoke(
        &self,
        q: &PluginOperationRequest,
        _: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        assert!(matches!(q.operation_id.as_str(), "acquire" | "replay"));
        self.calls.lock().unwrap().push(q.operation_id.clone());
        let now = chrono::Utc::now().timestamp_millis();
        let (schema, payload) = if q.operation_id == "acquire" {
            let mut points = vec![];
            for index in 0..4 {
                let price = 100. + f64::from(index);
                let start = START + i64::from(index) * DAY;
                let mut p = DataPoint {
                    instrument_id: "BTC/USD".into(),
                    source_event_id: format!("synthetic:{index}"),
                    provenance: BatchProvenance {
                        content_sha256: String::new(),
                        upstream_content_sha256: vec![],
                        event_start_ms: start,
                        event_end_ms: start + DAY,
                        known_at_ms: now,
                        arrived_at_ms: now,
                        completed: true,
                        source_revision: "authored-ohlcv-v1".into(),
                    },
                    result: PointValue::Ready(
                        json!({"open":price,"high":price+1.,"low":price-1.,"close":price,"volume":if self.excess_precision.load(Ordering::SeqCst){10.0000001}else{10.},"vwap":price+0.125,"tradeCount":7}),
                    ),
                };
                p.provenance.content_sha256 = point_sha256(&p).unwrap();
                points.push(p);
            }
            (RAW, json!({"snapshot":{"points":points}}))
        } else {
            let request: DataEvaluationRequest = serde_json::from_value(q.input.clone()).unwrap();
            let points: Vec<DataPoint> =
                serde_json::from_value(request.parameters["snapshot"]["points"].clone()).unwrap();
            let points = points
                .into_iter()
                .filter(|p| {
                    p.provenance.event_start_ms >= request.window.event_start_ms
                        && p.provenance.event_end_ms <= request.window.event_end_ms
                })
                .collect::<Vec<_>>();
            let mut close = points.clone();
            for p in &mut close {
                let PointValue::Ready(bar) = &p.result else {
                    panic!("ready fixture")
                };
                p.result = PointValue::Ready(bar["close"].clone());
                p.provenance.content_sha256 = point_sha256(p).unwrap();
            }
            (
                EVALUATION_RESPONSE_SCHEMA,
                value(&DataEvaluationResponse {
                    schema_version: EvaluationVersion::V1,
                    outputs: BTreeMap::from([
                        (
                            "bars".into(),
                            DataBatch {
                                schema: market_data::ohlcv_identity(),
                                shape: DataShape::Series,
                                points,
                            },
                        ),
                        (
                            "close".into(),
                            DataBatch {
                                schema: SchemaIdentity {
                                    schema_ref: CLOSE.into(),
                                    sha256: hash(
                                        &json!({"type":"array","items":{"type":"number"}}),
                                    )
                                    .unwrap(),
                                },
                                shape: DataShape::Series,
                                points: close,
                            },
                        ),
                    ]),
                    state: BTreeMap::new(),
                })?,
            )
        };
        Ok(PluginOperationResponse {
            correlation_id: q.correlation_id.clone(),
            schema_ref: schema.into(),
            payload: payload.clone(),
            observed_at_ms: now,
            source_event_id: q.correlation_id.clone(),
            content_hash: hash(&payload)?,
            freshness_state: "fresh".into(),
            evidence_refs: vec![],
            deterministic: q.operation_id == "replay",
            replayable: q.operation_id == "replay",
            provider_outcome_id: None,
            reconciliation_required: false,
        })
    }
}
fn ctx() -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("projection-fixture").unwrap(),
    )
}
fn install(s: &TradeAssemblyService, dir: &std::path::Path) {
    use crate::adapters::plugin_packages::data_package_tests::{archive_files, materials};
    use sha2::{Digest, Sha256};
    let byteshash = |v: &[u8]| format!("{:x}", Sha256::digest(v));
    let (mut descriptor, _, _, _) = materials();
    let template = crate::data_pipeline::tests::fixture()
        .0
        .nodes
        .into_iter()
        .find_map(|n| n.selected)
        .unwrap();
    let mut operations = vec![];
    for (op, purpose, outputs) in [
        ("acquire", "data_acquisition", vec![RAW]),
        (
            "replay",
            "calculation",
            vec![
                EVALUATION_RESPONSE_SCHEMA,
                market_data::OHLCV_SERIES_SCHEMA,
                CLOSE,
            ],
        ),
    ] {
        let mut v = serde_json::to_value(&template.operation).unwrap();
        v["id"] = json!(op);
        v["capability"] = json!(format!("test.projection.{op}@1"));
        v["purpose"] = json!(purpose);
        v["effect"] = json!("read");
        v["dependencies"] = json!([]);
        v["protocol"] = json!("stdio");
        v["credentialGrantRequired"] = json!(false);
        v["accountBindingRequired"] = json!(false);
        v["traits"] = json!({"modes":["backtest"],"instrumentFamilies":["crypto_spot"],"outputSchemaRefs":outputs,"inputSchemaRefs":[],"deterministic":op=="replay","replayable":op=="replay"});
        operations.push(v);
        let cap = format!("test.projection.{op}@1");
        s.runtime().storage.put_json("entitlement_grants",&cap,json!({"grantId":cap,"profile":"local_full","source":"test","capability":cap,"effect":"allow","state":"active","scope":"global","sourceType":"test","sourceRef":null,"precedence":10,"limits":{},"issuedAt":null,"expiresAt":null,"revokedAt":null,"reason":null}),&ctx()).unwrap();
    }
    let manifest = json!({"metadata":{"id":PLUGIN,"version":"1","provider":{"id":PLUGIN,"name":"Authored synthetic projection fixture"}},"capabilities":[{"id":"test.projection.acquire@1"},{"id":"test.projection.replay@1"}],"operations":operations,"permissions":[],"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let contract = json!({"schemaVersion":"tradeassembly.operation_data.v2","operationId":"replay","semanticsVersion":"fixture.ohlcv-replay.v1","inputs":{},"outputs":{"bars":market_data::ohlcv_output_port(DAY as u64).unwrap(),"close":{"schema":{"schemaRef":CLOSE,"sha256":hash(&json!({"type":"array","items":{"type":"number"}})).unwrap()},"shape":"series","time":{"intervalMs":DAY,"completedOnly":true}}},"parameterSchema":{"type":"object","additionalProperties":false,"required":["snapshot"],"properties":{"snapshot":{"type":"object"}}},"applicability":{"assetClasses":["crypto"],"instrumentFamilies":["crypto_spot"],"modes":["backtest"]},"readiness":"all_required_inputs","warmup":{"minimumObservations":1,"observationsParameter":null,"additionalObservations":0},"state":null});
    let mb = serde_json::to_vec(&manifest).unwrap();
    let cb = serde_json::to_vec(&contract).unwrap();
    descriptor["package"]["plugin"]["id"] = json!(PLUGIN);
    descriptor["package"]["manifest"]["sha256"] = json!(byteshash(&mb));
    descriptor["dataContracts"] = json!([{"operationId":"replay","document":{"path":"contract.json","sha256":byteshash(&cb)},"canonicalSha256":hash(&contract).unwrap()}]);
    descriptor["dataSchemas"] = json!([]);
    let mut files = vec![
        ("manifest.json".to_owned(), mb),
        ("contract.json".into(), cb),
        ("bin/plugin".into(), b"not-executed".to_vec()),
    ];
    for (index,(id,schema)) in [(RAW,json!({"type":"object","required":["snapshot"],"properties":{"snapshot":{"type":"object"}},"additionalProperties":false})),(EVALUATION_RESPONSE_SCHEMA,json!({"type":"object"})),(market_data::OHLCV_SERIES_SCHEMA,market_data::ohlcv_series_schema()),(CLOSE,json!({"type":"array","items":{"type":"number"}}))].into_iter().enumerate(){
  let document=serde_json::to_vec(&schema).unwrap();let path=format!("schema-{index}.json");descriptor["dataSchemas"].as_array_mut().unwrap().push(json!({"schemaRef":id,"document":{"path":path,"sha256":byteshash(&document)},"canonicalSha256":hash(&schema).unwrap()}));files.push((path,document));
 }
    files.push((
        "tradeassembly-plugin.json".into(),
        serde_json::to_vec(&descriptor).unwrap(),
    ));
    let archive = archive_files(files.iter().map(|(p, b)| (p.as_str(), b.clone())).collect());
    let path = dir.join("ohlcv.tgz");
    std::fs::write(&path, &archive).unwrap();
    let installed = s
        .runtime()
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: byteshash(&archive),
            manifest_sha256: hash(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    s.runtime().plugins.install_manifest(PLUGIN,json!({"pluginRef":PLUGIN,"manifest":manifest,"manifestDigest":installed.manifest_sha256,"package":{"packageSha256":installed.package_sha256},"trust":{"level":"test"}}),&ctx()).unwrap();
    s.runtime().plugins.put_instance(PLUGIN,json!({"instanceRef":PLUGIN,"pluginRef":PLUGIN,"enabled":true,"activePackageSha256":installed.package_sha256,"configuration":{}}),&ctx()).unwrap();
    s.bind_owned_object("plugin_instance", PLUGIN).unwrap();
}
struct Rig {
    _dir: tempfile::TempDir,
    s: TradeAssemblyService,
    operations: Arc<Operations>,
    capture: data_captures::CaptureReceipt,
    command: ProjectionCommand,
    plan: super::super::data_plans::StoredDataPlan,
}
fn rig(precision: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("projection.sqlite")
        .to_string_lossy()
        .into_owned();
    // Register this newly created test-owned database through the normal local
    // test factory before custom pure operation ports write it. The normal
    // workspace-export reader below must retain its real profile/owner guards.
    let _local_profile = TradeAssemblyService::test_local(&db);
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.plugin_packages = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(dir.path().join("packages"))
            .unwrap(),
    );
    let operations = Arc::new(Operations::default());
    operations
        .excess_precision
        .store(precision, Ordering::SeqCst);
    runtime.plugin_operations = operations.clone();
    let s = TradeAssemblyService::from_runtime(&db, runtime).for_authenticated_invocation(
        "local-owner",
        "capture-projection-owner",
        None,
        None,
    );
    install(&s, dir.path());
    for instance in s.runtime().plugins.list_instances().unwrap() {
        s.bind_owned_object("plugin_instance", instance["instanceRef"].as_str().unwrap())
            .unwrap();
    }

    let context = DataContext {
        instruments: vec![InstrumentContext {
            instrument_id: "BTC/USD".into(),
            asset_class: "crypto".into(),
            instrument_family: "crypto_spot".into(),
        }],
        interval_ms: Some(DAY as u64),
    };
    let operand = crate::data_strategy::Operand::Output {
        node: "source".into(),
        port: "close".into(),
        instrument: "BTC/USD".into(),
        sample: crate::data_strategy::Sample::Current,
    };
    let program = crate::data_strategy::DataStrategy {
        version: 1,
        nodes: vec![PipelineNode {
            graph_node_id: "source".into(),
            parameters: json!({}),
            context: context.clone(),
            input_contexts: BTreeMap::new(),
        }],
        bindings: vec![],
        trade_instruments: context.instruments.clone(),
        entry: crate::data_strategy::Rule::Compare {
            left: operand.clone(),
            op: crate::data_strategy::Comparison::Greater,
            right: crate::data_strategy::Operand::Number { value: 100. },
        },
        exit: crate::data_strategy::Rule::Compare {
            left: operand,
            op: crate::data_strategy::Comparison::Less,
            right: crate::data_strategy::Operand::Number { value: 100. },
        },
    };
    let mut spec: Value = serde_json::from_str(include_str!(
        "../../../examples/strategy-spec/v3/valid/static-equity.json"
    ))
    .unwrap();
    for r in spec["capability_requirements"]["required"]
        .as_array_mut()
        .unwrap()
    {
        r["constraints"]["instrument_families"] = json!(["crypto_spot"]);
    }
    let signal = &mut spec["signal_market_requirements"][0];
    signal["asset_class"] = json!("crypto");
    signal["instrument_family"] = json!("crypto_spot");
    signal["selectors"][0]["normalized_id"] = json!("BTC/USD");
    signal["calendar_refs"] = json!(["crypto_24x7"]);
    signal["timeframes"] = json!(["1d"]);
    let trade = &mut spec["trade_market_requirements"][0];
    trade["asset_classes"] = json!(["crypto"]);
    trade["instrument_families"] = json!(["crypto_spot"]);
    trade["quantity_units"] = json!(["base_asset"]);
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({(crate::data_strategy::CONFIG_KEY):program});
    spec["stages"]["exit_policy"]["policy"] = json!({"monitoring":["synthetic"],"triggers":[{"id":"close_threshold","kind":"indicator","scope":"full_position"}],"composition":"any","precedence":[],"end_of_data":"preserve_open_positions"});
    spec["stages"]["order_strategy"]["policy"]["allowed_order_types"] = json!(["market"]);
    spec["stages"]["price_policy"]["policy"]["tick_size"] = Value::Null;
    spec["stages"]["market_clock"]["policy"]["timezone"] = json!("UTC");
    spec["stages"]["market_clock"]["policy"]["calendar_refs"] = json!(["crypto_24x7"]);
    spec["stages"]["market_clock"]["policy"]
        .as_object_mut()
        .unwrap()
        .remove("schedule_capability_requirement_ref");
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
    for (node, op) in [("acquisition", "acquire"), ("source", "replay")] {
        let mut r = spec["capability_requirements"]["required"][0].clone();
        r["requirement_id"] = json!(node);
        r["capability"] = json!(format!("test.projection.{op}@1"));
        r["required_for"] = json!(["backtest"]);
        r["stage_refs"] = json!(["evaluate"]);
        r["substep_refs"] = json!([]);
        r["constraints"] = json!({"instrument_families":["crypto_spot"],"schema_refs":[],"data_shapes":[],"fields":[],"input_paths":[],"operations":[],"deterministic":false,"replayable":false});
        r["dependency_refs"] = json!([]);
        spec["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap()
            .push(r);
        bindings[node] = json!({"pluginInstanceRef":PLUGIN,"pluginRef":PLUGIN,"operationId":op});
    }
    let created=s.handle_http("POST","/product/strategies/create",json!({"name":"Synthetic capture projection research","spec":spec,"idempotencyKey":"projection-strategy"}));
    assert_eq!(created.status, 201, "{}", created.body);
    let strategy = created.body["body"]["strategy"]["id"].as_str().unwrap();
    let p=s.handle_http("POST","/product/strategies/publish",json!({"strategyId":strategy,"expectedDraftHash":created.body["body"]["draft"]["draftHash"],"idempotencyKey":"projection-publish"}));
    assert_eq!(p.body["body"]["published"], true, "{}", p.body);
    let scope = PlanScope {
        strategy_id: strategy.into(),
        strategy_version_id: p.body["body"]["version"]["id"].as_str().unwrap().into(),
        strategy_spec_hash: p.body["body"]["version"]["specHash"]
            .as_str()
            .unwrap()
            .into(),
    };
    let graph=s.handle_http("POST","/capability-graph-revisions",json!({"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"bindings":bindings,"mode":"backtest","idempotencyKey":"projection-graph"}));
    assert_eq!(
        graph.body["revision"]["graph"]["ok"], true,
        "{}",
        graph.body
    );
    let captured=s.handle_http("POST","/product/strategies/data-captures/create",json!({"capture":{"receiptVersion":"tradeassembly.data_capture_receipt.v2","policy":{"scope":scope,"capabilityGraphRevision":graph.body["revision"]["revisionId"],"mode":"backtest","acquisitionNode":"acquisition","replayNode":"source","parameterExtracts":{"snapshot":"/snapshot"},"context":context,"eventStartMs":START,"eventEndMs":START+4*DAY,"gapPolicy":"strict_fixed_interval"},"input":{"instrument":"BTC/USD","start":START,"end":START+4*DAY,"interval":DAY,"origin":START,"feed":"authored_synthetic_ohlcv_projection"}},"accountMode":"paper","idempotencyKey":"projection-capture"}));
    assert_eq!(captured.status, 200, "{}", captured.body);
    let capture: data_captures::CaptureReceipt =
        serde_json::from_value(captured.body["capture"].clone()).unwrap();
    let prepared=s.handle_http("POST","/product/strategies/data-plans/prepare",json!({"scope":scope,"capabilityGraphRevisionId":graph.body["revision"]["revisionId"],"mode":"backtest","sourceCaptures":{"source":capture.capture_id},"idempotencyKey":"projection-plan"}));
    assert_eq!(prepared.status, 200, "{}", prepared.body);
    let plan = serde_json::from_value(prepared.body["plan"].clone()).unwrap();
    let command = ProjectionCommand {
        schema_version: Version::V1,
        projection_key: "projection-original".into(),
        scope,
        capture: CapturePin {
            capture_id: capture.capture_id.clone(),
            receipt_sha256: capture.receipt_sha256.clone(),
        },
        replay_output_port: "bars".into(),
        profile: Profile::Diagnostic,
        timestamp_basis: TimestampBasis::EventStart,
        as_of_ms: s.runtime().clock.trusted_now_ms().unwrap(),
        acknowledge_modeled_calendar: true,
        acknowledge_retrospective_availability: true,
    };
    Rig {
        _dir: dir,
        s,
        operations,
        capture,
        command,
        plan,
    }
}
fn create_request(r: &Rig, key: &str) -> ServiceResponse {
    r.s.handle_http(
        "POST",
        CREATE,
        json!({"projection":r.command,"idempotencyKey":key}),
    )
}
fn advance_request_call(s: &TradeAssemblyService, projection: &str, key: &str) -> ServiceResponse {
    s.handle_http("POST",&format!("{CREATE}/{projection}/advance"),json!({"projection":{"schemaVersion":"tradeassembly.dataset_capture_projection.v1","projectionId":projection},"idempotencyKey":key}))
}
fn config(r: &Rig, snapshot: &DatasetSnapshot) -> Value {
    let source = &snapshot.content.source;
    json!({"schema":"tradeassembly.backtest-configuration.v2","typedEvaluation":{"schemaVersion":"tradeassembly.typed-backtest-binding.v2","planRevisionId":r.plan.revision_id,"planRevisionSha256":r.plan.revision_sha256,"sourceCaptures":{"source":{"captureId":r.capture.capture_id,"receiptSha256":r.capture.receipt_sha256}},"eventStartMs":START,"eventEndMs":START+4*DAY,"asOfMs":r.command.as_of_ms,"intervalMs":DAY,"maxFramesPerStep":2,"maximumFrames":4,"quantityMicros":1000,"initialPosition":"flat","knowledgePolicy":"retrospective_capture_cutoff","fillTimestampBasis":"event_start","warmupObservations":1,"calendarEvidence":{"kind":"declared_dataset_calendar","calendar":"crypto_24x7","timezone":"UTC","purpose":"research_simulation_only","acknowledgeResearchAssumption":true}},"strategy":r.command.scope,"dataset":{"datasetId":snapshot.dataset_id,"snapshotRef":snapshot.snapshot_ref,"contentHash":snapshot.content_hash,"schema":snapshot.content.schema,"sourcePluginRef":source.plugin_ref,"sourcePluginInstanceRef":source.plugin_instance_ref,"sourceOperationId":source.operation_id,"sourceManifestFingerprint":source.plugin_manifest_fingerprint,"sourceClass":"test","capabilityGraphRevisionId":source.capability_graph_revision_id,"capabilityGraphFingerprint":source.capability_graph_fingerprint,"instruments":["BTC/USD"],"selectedTimeSlice":snapshot.content.time_slice,"granularity":"1d","calendar":"crypto_24x7","timezone":"UTC","corporateActionPolicy":"none","benchmark":null},"capabilityGraph":{"revisionId":r.plan.content.plan.request.capability_graph_revision,"graphFingerprint":r.plan.content.plan.capability_graph_fingerprint},"capital":{"startingCashMicros":1000000000,"reportingCurrency":"USD","accountModel":"cash","maximumLeverageMicros":1000000,"marginPolicy":"none"},"execution":{"evaluationTiming":"bar_close","fillTiming":"next_bar","fillPriceSource":"open","orderType":"market","timeInForce":"day","fillPolicy":"full","rejectOnInsufficientCapital":true,"rejectOnInsufficientLiquidity":true,"staleDataPolicy":"reject","missingDataPolicy":"reject"},"costs":{"fixedFeeMicros":0,"perUnitFeeMicros":0,"notionalFeeBps":0,"spreadBps":0,"slippageBps":0},"liquidity":{"maximumParticipationBps":10000,"minimumVolumeMicros":0},"instruments":[{"instrumentId":"BTC/USD","instrumentFamily":"crypto_spot","quantityUnit":"base_asset","quantityScale":6,"priceScale":6,"contractMultiplierMicros":1000000,"settlement":"cash","metadata":{}}],"risk":{"maximumPositionNotionalMicros":1000000000,"maximumOrderQuantityMicros":10000000,"maximumLossMicros":1000000000,"maximumOpenPositions":1},"endOfData":"mark_to_market","evaluatorVersion":"tradeassembly.typed-backtest.v1","compilerVersion":"tradeassembly.typed-backtest.v1","pluginFingerprints":{},"variableOverrides":{}})
}
fn counts(s: &TradeAssemblyService) -> (usize, usize, usize) {
    (
        s.runtime().dataset_snapshots.list().unwrap().len(),
        s.runtime().storage.list_json(STATES).unwrap().len(),
        s.runtime().storage.list_json(PROOFS).unwrap().len(),
    )
}

#[test]
fn normal_projection_publishes_real_ohlcv_provenance_then_canonical_typed_research() {
    let r = rig(false);
    assert_eq!(
        r.operations.calls.lock().unwrap().as_slice(),
        ["acquire", "replay"]
    );
    let created = create_request(&r, "create-original");
    assert_eq!(created.status, 202, "{}", created.body);
    assert_eq!(created.body["dataset"], Value::Null);
    assert_eq!(counts(&r.s), (0, 1, 0));
    let projection = created.body["projectionId"].as_str().unwrap();
    let complete = advance_request_call(&r.s, projection, "publish-original");
    assert_eq!(complete.status, 200, "{}", complete.body);
    assert_eq!(complete.body["published"], true);
    assert_eq!(counts(&r.s), (1, 1, 1));
    let exact_create_retry = create_request(&r, "create-original");
    assert_eq!(exact_create_retry.status, 202);
    assert_eq!(
        exact_create_retry.body["published"], false,
        "C3 keeps the original prepared outcome rather than relabeling later state"
    );
    assert_eq!(
        r.s.handle_http("GET", &format!("{CREATE}/{projection}"), json!({}))
            .body["published"],
        true
    );
    let (_, state) = load(&r.s, projection).unwrap();
    verify_for_research(&r.s, &state.snapshot).unwrap();
    assert_eq!(state.snapshot.row_count, 4);
    assert_eq!(
        state.snapshot.content.source.source_class,
        DatasetSourceClass::Test
    );
    assert_eq!(
        state.proof.evidence.points,
        r.capture.replay_response.outputs["bars"].points
    );
    assert_eq!(
        state.proof.evidence.input_witness,
        r.capture.input_witness.clone().unwrap()
    );
    assert!(!state.proof.evidence.provider_authenticity_established);
    for (index, row) in state.snapshot.content.observations.iter().enumerate() {
        let HistoricalObservationData::Bar(bar) = &row.data else {
            panic!("bar")
        };
        assert_eq!(bar.open, 100. + index as f64);
        assert_eq!(bar.close, bar.open);
        assert_eq!(bar.vwap, Some(bar.open + 0.125));
        assert!(bar.session.is_none());
        assert_eq!(row.timestamp, utc(START + index as i64 * DAY).unwrap());
        assert_eq!(
            state.proof.evidence.points[index].provenance.known_at_ms,
            r.capture.replay_response.outputs["bars"].points[index]
                .provenance
                .known_at_ms
        );
        let PointValue::Ready(value) = &state.proof.evidence.points[index].result else {
            panic!("ready")
        };
        assert_eq!(value["tradeCount"], 7);
    }
    // Projection/retries consume the existing capture only, with no operation call.
    let before = r.operations.calls.lock().unwrap().clone();
    let duplicate = create_request(&r, "create-new-outer-key");
    assert_eq!(duplicate.status, 200, "{}", duplicate.body);
    assert_eq!(
        advance_request_call(&r.s, projection, "publish-new-outer-key").body,
        complete.body
    );
    assert_eq!(*r.operations.calls.lock().unwrap(), before);
    assert_eq!(counts(&r.s), (1, 1, 1));
    let metadata=r.s.handle_http("POST","/research/datasets",json!({"strategyId":r.command.scope.strategy_id,"immutableSnapshotId":state.snapshot.dataset_id,"idempotencyKey":"verified-projection-metadata"}));
    assert_eq!(metadata.body["body"]["rows"], 4, "{}", metadata.body);
    let mut early_config: crate::backtest_contracts::BacktestConfiguration =
        serde_json::from_value(config(&r, &state.snapshot)).unwrap();
    early_config.typed_evaluation.as_mut().unwrap().as_of_ms =
        state.proof.evidence.acquisition_observed_at_ms - 1;
    assert_eq!(
        super::super::typed_backtest::validate_dataset(&r.s, &early_config, &state.snapshot)
            .unwrap_err(),
        "dataset_projection_consumer_cutoff_unavailable"
    );
    let run=r.s.handle_http("POST","/backtests",json!({"configuration":config(&r,&state.snapshot),"idempotencyKey":"projection-actual-typed-run"}));
    assert_eq!(run.status, 202, "{}", run.body);
    let run_id = run.body["runId"].as_str().unwrap();
    let mut done = Value::Null;
    for index in 0..4 {
        let response=r.s.handle_http("POST","/backtests:process",json!({"runId":run_id,"worker":"projection-unit","idempotencyKey":format!("projection-process-{index}")}));
        assert!(matches!(response.status, 200 | 202), "{}", response.body);
        if response.body["status"] == "completed" {
            done = response.body;
            break;
        }
    }
    assert_eq!(done["status"], "completed", "{done}");
    let result = r.s.runtime().backtests.get_result(run_id).unwrap().unwrap();
    assert_eq!(
        result
            .content
            .typed_evaluation
            .as_ref()
            .unwrap()
            .frames_processed,
        4
    );
    assert_eq!(result.content.fills.len(), 1);
    assert_eq!(result.content.fills[0]["executionBarIndex"], 2);
    assert_eq!(result.content.fills[0]["quantityMicros"], 1000);
    assert_eq!(
        r.operations
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|v| *v == "acquire")
            .count(),
        1
    );
    assert!(r
        .s
        .runtime()
        .storage
        .list_json("execution_order_plan_submissions_v2")
        .unwrap()
        .is_empty());
}

#[test]
fn owner_wire_port_cutoff_precision_and_changed_key_refuse_before_projection_writes() {
    let mut r = rig(false);
    let before = counts(&r.s);
    let calls = r.operations.calls.lock().unwrap().clone();
    let foreign =
        r.s.for_authenticated_invocation("fixture", "projection-foreign", None, None);
    let denied = foreign.handle_http(
        "POST",
        CREATE,
        json!({"projection":r.command,"idempotencyKey":"foreign"}),
    );
    assert!(denied.status >= 400);
    assert_eq!(counts(&r.s), before);
    let original = r.command.clone();
    for (index, change) in [
        "wrong_port",
        "close_only",
        "old_cutoff",
        "future_cutoff",
        "model_not_acknowledged",
    ]
    .iter()
    .enumerate()
    {
        r.command = original.clone();
        match *change {
            "wrong_port" => r.command.replay_output_port = "missing".into(),
            "close_only" => r.command.replay_output_port = "close".into(),
            "old_cutoff" => r.command.as_of_ms = START + 4 * DAY,
            "future_cutoff" => r.command.as_of_ms = i64::MAX,
            _ => r.command.acknowledge_modeled_calendar = false,
        }
        assert!(
            create_request(&r, &format!("bad-{index}")).status >= 400,
            "{change}"
        );
        assert_eq!(counts(&r.s), before);
    }
    r.command = original.clone();
    let mut unknown = value(&r.command).unwrap();
    unknown["response"] = json!({"bars":[]});
    assert!(
        r.s.handle_http(
            "POST",
            CREATE,
            json!({"projection":unknown,"idempotencyKey":"closed-wire"})
        )
        .status
            >= 400
    );
    assert_eq!(counts(&r.s), before);
    let created = create_request(&r, "stable-create");
    assert_eq!(created.status, 202, "{}", created.body);
    r.command.timestamp_basis = TimestampBasis::EventEnd;
    let conflict = create_request(&r, "fresh-outer-same-projection-key");
    assert_eq!(
        conflict.body["error"]["code"],
        "dataset_projection_idempotency_conflict"
    );
    assert_eq!(counts(&r.s), (0, 1, 0));
    assert_eq!(*r.operations.calls.lock().unwrap(), calls);
    let precise = rig(true);
    assert!(create_request(&precise, "precision-refusal").status >= 400);
    assert_eq!(counts(&precise.s), (0, 0, 0));
}

fn reseal_capture(
    s: &TradeAssemblyService,
    mut capture: data_captures::CaptureReceipt,
) -> CapturePin {
    capture.replay_payload_sha256 = hash(&capture.replay_response).unwrap();
    capture.capture_id.clear();
    capture.receipt_sha256.clear();
    let digest = hash(&capture).unwrap();
    capture.receipt_sha256 = digest.clone();
    capture.capture_id = format!("capture_{digest}");
    s.runtime()
        .storage
        .put_json(
            "data_acquisition_receipts_v1",
            &capture.capture_id,
            value(&capture).unwrap(),
            &ctx(),
        )
        .unwrap();
    CapturePin {
        capture_id: capture.capture_id,
        receipt_sha256: digest,
    }
}
#[test]
fn current_capture_hash_witness_coverage_readiness_and_arrival_cannot_be_bypassed() {
    let mut r = rig(false);
    let valid = r.capture.clone();
    for (index, case) in [
        "hash_tamper",
        "gap",
        "duplicate",
        "unready",
        "future_arrival",
        "v1",
    ]
    .iter()
    .enumerate()
    {
        let mut capture = valid.clone();
        match *case {
            "gap" => {
                capture
                    .replay_response
                    .outputs
                    .get_mut("bars")
                    .unwrap()
                    .points
                    .remove(1);
            }
            "duplicate" => {
                let batch = capture.replay_response.outputs.get_mut("bars").unwrap();
                batch.points[1] = batch.points[0].clone();
            }
            "unready" => {
                let point = &mut capture
                    .replay_response
                    .outputs
                    .get_mut("bars")
                    .unwrap()
                    .points[0];
                point.result = PointValue::WarmingUp;
                point.provenance.content_sha256 = point_sha256(point).unwrap();
            }
            "future_arrival" => {
                capture
                    .replay_response
                    .outputs
                    .get_mut("bars")
                    .unwrap()
                    .points[0]
                    .provenance
                    .arrived_at_ms = capture.replay_as_of_ms + 1;
            }
            "v1" => {
                capture.schema_version = data_captures::CaptureReceiptVersion::V1;
                capture.input_witness = None;
            }
            _ => {
                capture.payload_sha256 = "f".repeat(64);
            }
        }
        r.command.capture = reseal_capture(&r.s, capture);
        r.command.projection_key = format!("bad-capture-{index}");
        let response = create_request(&r, &format!("bad-receipt-{index}"));
        assert!(response.status >= 400, "{case}: {}", response.body);
        assert_eq!(counts(&r.s), (0, 0, 0));
    }
    assert_eq!(
        r.operations.calls.lock().unwrap().as_slice(),
        ["acquire", "replay"]
    );
}

#[test]
fn partial_repository_proof_and_ownership_publication_resume_without_early_research_use() {
    for failure in ["proof_insert", "published_state"] {
        let r = rig(false);
        let c = create_request(&r, &format!("create-{failure}"));
        assert_eq!(c.status, 202, "{}", c.body);
        let projection = c.body["projectionId"].as_str().unwrap();
        let db = rusqlite::Connection::open(r.s.db()).unwrap();
        let sql = if failure == "proof_insert" {
            "CREATE TRIGGER projection_failure BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='dataset_capture_projection_receipts_v1' BEGIN SELECT RAISE(ABORT,'injected'); END;"
        } else {
            "CREATE TRIGGER projection_failure BEFORE UPDATE ON tradeassembly_kv WHEN NEW.namespace='dataset_capture_projection_states_v1' AND NEW.value_json LIKE '%\"phase\":\"published\"%' BEGIN SELECT RAISE(ABORT,'injected'); END;"
        };
        db.execute_batch(sql).unwrap();
        let interrupted = advance_request_call(&r.s, projection, &format!("interrupted-{failure}"));
        assert!(interrupted.status >= 400, "{}", interrupted.body);
        let (_, state) = load(&r.s, projection).unwrap();
        assert_ne!(state.phase, Phase::Published);
        assert_eq!(r.s.runtime().dataset_snapshots.list().unwrap().len(), 1);
        assert!(verify_for_research(&r.s, &state.snapshot).is_err());
        let attempt=r.s.handle_http("POST","/backtests",json!({"configuration":config(&r,&state.snapshot),"idempotencyKey":format!("partial-run-{failure}")}));
        assert!(attempt.status >= 400, "{}", attempt.body);
        assert!(r
            .s
            .runtime()
            .storage
            .list_json("backtest_manifests_v1")
            .unwrap()
            .is_empty());
        if failure == "proof_insert" {
            assert!(r
                .s
                .runtime()
                .object_authorization
                .scope("dataset_snapshot", &state.snapshot.dataset_id)
                .unwrap()
                .is_none());
            assert_eq!(counts(&r.s), (1, 1, 0));
        } else {
            assert!(r
                .s
                .runtime()
                .object_authorization
                .scope("dataset_snapshot", &state.snapshot.dataset_id)
                .unwrap()
                .is_some());
            assert_eq!(counts(&r.s), (1, 1, 1));
        }
        let hidden=r.s.handle_http("POST","/research/datasets",json!({"strategyId":r.command.scope.strategy_id,"immutableSnapshotId":state.snapshot.dataset_id,"idempotencyKey":format!("partial-meta-{failure}")}));
        assert_ne!(hidden.body["body"]["verified"], true);
        let export_reader = TradeAssemblyService::test_local(r.s.db())
            .for_authenticated_invocation("local-owner", "capture-projection-owner", None, None);
        let export = super::super::public_workspace::export(&export_reader).unwrap();
        assert!(!export
            .entries
            .iter()
            .any(|entry| entry.namespace == "public_dataset_references_v1"
                && entry.value["datasetId"] == state.snapshot.dataset_id));
        db.execute_batch("DROP TRIGGER projection_failure;")
            .unwrap();
        let restored = TradeAssemblyService::from_runtime(r.s.db(), (*r.s.runtime()).clone())
            .for_authenticated_invocation("local-owner", "capture-projection-owner", None, None);
        let resumed = advance_request_call(
            &restored,
            projection,
            &format!("resume-fresh-key-{failure}"),
        );
        assert_eq!(resumed.status, 200, "{}", resumed.body);
        assert_eq!(resumed.body["published"], true);
        assert_eq!(counts(&restored), (1, 1, 1));
        verify_for_research(&restored, &state.snapshot).unwrap();
        let export = super::super::public_workspace::export(&export_reader).unwrap();
        assert!(export
            .entries
            .iter()
            .any(|entry| entry.namespace == "public_dataset_references_v1"
                && entry.value["datasetId"] == state.snapshot.dataset_id));
        assert_eq!(
            r.operations.calls.lock().unwrap().as_slice(),
            ["acquire", "replay"]
        );
    }
}

#[test]
fn current_source_revocation_blocks_publication_and_new_research_but_historical_proof_remains() {
    let r = rig(false);
    let c = create_request(&r, "pre-revocation-create");
    let projection = c.body["projectionId"].as_str().unwrap();
    let original = r.s.runtime().plugins.get_instance(PLUGIN).unwrap().unwrap();
    let mut disabled = original.clone();
    disabled["enabled"] = json!(false);
    r.s.runtime()
        .plugins
        .put_instance(PLUGIN, disabled, &ctx())
        .unwrap();
    let denied = advance_request_call(&r.s, projection, "disabled-publish");
    assert!(denied.status >= 400);
    assert_eq!(counts(&r.s), (0, 1, 0));
    r.s.runtime()
        .plugins
        .put_instance(PLUGIN, original.clone(), &ctx())
        .unwrap();
    assert_eq!(
        advance_request_call(&r.s, projection, "enabled-publish").status,
        200
    );
    let (_, state) = load(&r.s, projection).unwrap();
    let mut changed = original;
    changed["configuration"] = json!({"differentSource":true});
    r.s.runtime()
        .plugins
        .put_instance(PLUGIN, changed, &ctx())
        .unwrap();
    let historical =
        r.s.handle_http("GET", &format!("{CREATE}/{projection}"), json!({}));
    assert_eq!(historical.status, 200, "{}", historical.body);
    let before =
        r.s.runtime()
            .storage
            .list_json("backtest_manifests_v1")
            .unwrap();
    let denied = r.s.handle_http(
        "POST",
        "/backtests",
        json!({"configuration":config(&r,&state.snapshot),"idempotencyKey":"changed-source-run"}),
    );
    assert!(denied.status >= 400, "{}", denied.body);
    assert_eq!(
        r.s.runtime()
            .storage
            .list_json("backtest_manifests_v1")
            .unwrap(),
        before
    );
    assert_eq!(
        r.operations.calls.lock().unwrap().as_slice(),
        ["acquire", "replay"]
    );
}

#[test]
fn durable_provenance_tamper_and_foreign_historical_reads_refuse_without_mutation() {
    let r = rig(false);
    let c = create_request(&r, "proof-create");
    let projection = c.body["projectionId"].as_str().unwrap();
    assert_eq!(
        advance_request_call(&r.s, projection, "proof-publish").status,
        200
    );
    let (_, state) = load(&r.s, projection).unwrap();
    let foreign =
        r.s.for_authenticated_invocation("fixture", "foreign", None, None);
    assert!(
        foreign
            .handle_http("GET", &format!("{CREATE}/{projection}"), json!({}))
            .status
            >= 400
    );
    let raw =
        r.s.runtime()
            .storage
            .get_json(PROOFS, projection)
            .unwrap()
            .unwrap();
    let mut corrupt = raw.clone();
    corrupt["evidence"]["points"][0]["provenance"]["arrivedAtMs"] = json!(1);
    r.s.runtime()
        .storage
        .put_json(PROOFS, projection, corrupt.clone(), &ctx())
        .unwrap();
    assert!(verify_for_research(&r.s, &state.snapshot).is_err());
    assert!(
        r.s.handle_http("GET", &format!("{CREATE}/{projection}"), json!({}))
            .status
            >= 400
    );
    let refused = r.s.handle_http(
        "POST",
        "/backtests",
        json!({"configuration":config(&r,&state.snapshot),"idempotencyKey":"tampered-proof-run"}),
    );
    assert!(refused.status >= 400, "{}", refused.body);
    assert_eq!(
        r.s.runtime().storage.get_json(PROOFS, projection).unwrap(),
        Some(corrupt)
    );
    assert_eq!(counts(&r.s), (1, 1, 1));
    assert_eq!(
        r.operations.calls.lock().unwrap().as_slice(),
        ["acquire", "replay"]
    );
}

#[test]
fn installed_replay_byte_tamper_blocks_the_next_publication_round() {
    let r = rig(false);
    let created = create_request(&r, "bytes-create");
    assert_eq!(created.status, 202, "{}", created.body);
    let projection = created.body["projectionId"].as_str().unwrap();
    let package =
        r.s.runtime()
            .plugin_packages
            .get(&r.capture.replay.package_sha256)
            .unwrap()
            .unwrap();
    std::fs::write(package.install_root.join("contract.json"), b"{}").unwrap();
    let before = counts(&r.s);
    let refused = advance_request_call(&r.s, projection, "tampered-bytes-advance");
    assert!(refused.status >= 400, "{}", refused.body);
    assert_eq!(counts(&r.s), before);
    assert!(r
        .s
        .runtime()
        .object_authorization
        .list_visible(&r.s.invocation_owner().unwrap(), "dataset_snapshot")
        .unwrap()
        .is_empty());
    assert_eq!(
        r.operations.calls.lock().unwrap().as_slice(),
        ["acquire", "replay"]
    );
}

#[test]
fn projection_routes_use_existing_research_creation_authority_and_exact_nested_resource() {
    let r = rig(false);
    let body = json!({"projection":r.command,"idempotencyKey":"classification-only"});
    let envelope = crate::control_plane::http_envelope("http", "POST", CREATE, &body).unwrap();
    assert_eq!(envelope.command_name, "research.dataset.project.create");
    assert_eq!(envelope.side_effect_class, "research");
    assert_eq!(
        envelope.target_object.as_deref(),
        Some(r.command.scope.strategy_id.as_str())
    );
    let definition = crate::finance_authority::authority_definition(&envelope).unwrap();
    assert_eq!(definition.action, "research.dataset.create");
    assert_eq!(definition.resource_kind, "research_dataset");
    let projection = id(&r.s, &r.command).unwrap();
    let path = format!("{CREATE}/{projection}/advance");
    let body = json!({"projection":{"schemaVersion":"tradeassembly.dataset_capture_projection.v1","projectionId":projection},"idempotencyKey":"advance-classification-only"});
    let envelope = crate::control_plane::http_envelope("http", "POST", &path, &body).unwrap();
    assert_eq!(envelope.command_name, "research.dataset.project.advance");
    assert_eq!(envelope.side_effect_class, "research");
    assert_eq!(envelope.target_object.as_deref(), Some(projection.as_str()));
    assert_eq!(
        crate::finance_authority::authority_definition(&envelope)
            .unwrap()
            .action,
        "research.dataset.create"
    );
    let wrong =
        r.s.handle_http("POST", "/dataset-projections/wrong/advance", body);
    assert_eq!(
        wrong.body["error"]["code"],
        "dataset_projection_path_mismatch"
    );
    assert_eq!(counts(&r.s), (0, 0, 0));
    assert!(crate::http::route_is_documented("POST", CREATE));
    assert!(crate::http::route_is_documented(
        "GET",
        &format!("{CREATE}/{projection}")
    ));
}
