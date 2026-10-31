//! Synthetic in-process provider fixture through real Core capture/recipe/evaluator
//! APIs and actual verified packages/SQLite. Not OS-process/provider qualification.
use super::*;
use crate::ports::*;
use std::sync::{
    atomic::{AtomicI64, AtomicUsize, Ordering},
    Arc,
};
use tradeassembly_plugin_sdk::{data_contract::*, data_evaluation::*, data_history::*};
const NOW: i64 = 1_700_000_000_000;
const START: i64 = NOW - 10_000;
const SERIES: &str = "schema://example.context/series@1";
const ACQUIRED: &str = "schema://example.context/acquired@1";
struct Clock;
impl VersionedPort for Clock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl ClockPort for Clock {
    fn now_ms(&self) -> i64 {
        NOW
    }
}
struct Identity;
impl VersionedPort for Identity {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl IdentityPort for Identity {
    fn resolve(&self, subject: &str, _: i64) -> Result<IdentityClaims, String> {
        Ok(IdentityClaims {
            issuer: "context-fixture".into(),
            subject: subject.into(),
            audience: vec![],
            assurance: Some("synthetic-test".into()),
            expires_at_ms: NOW + 1_000_000,
        })
    }
}
struct Operations {
    calls: AtomicUsize,
    origin: &'static str,
    anchor: Option<i64>,
    late: bool,
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (schema, payload) = if q.operation_id == "acquire" {
            let mut payload = json!({"origin":self.origin,"values":[10,20,30],"eventStartMs":START,"intervalMs":1000});
            if let Some(anchor) = self.anchor {
                payload["anchorAtMs"] = json!(anchor);
            }
            (ACQUIRED, payload)
        } else {
            let request: DataEvaluationRequest =
                serde_json::from_value(q.input.clone()).map_err(|_| "fixture_request_invalid")?;
            let mut points = Vec::new();
            if q.operation_id == "provide" {
                let start = request.parameters["originStartMs"]
                    .as_i64()
                    .ok_or("fixture_origin_missing")?;
                let interval = request.parameters["originIntervalMs"]
                    .as_i64()
                    .ok_or("fixture_origin_missing")?;
                if interval != 1000 {
                    return Err("fixture_interval_mismatch".into());
                }
                for (index, value) in request.parameters["values"]
                    .as_array()
                    .ok_or("fixture_values_missing")?
                    .iter()
                    .enumerate()
                {
                    let event_start = start + index as i64 * interval;
                    let end = event_start + interval;
                    if event_start < request.window.event_start_ms
                        || end > request.window.event_end_ms
                    {
                        continue;
                    }
                    let mut point = DataPoint {
                        instrument_id: "TEST".into(),
                        source_event_id: format!("synthetic-source:{end}"),
                        provenance: BatchProvenance {
                            content_sha256: String::new(),
                            upstream_content_sha256: vec![],
                            event_start_ms: event_start,
                            event_end_ms: end,
                            known_at_ms: end + 1,
                            arrived_at_ms: end + 1,
                            completed: true,
                            source_revision: "fixture-source-v1".into(),
                        },
                        result: PointValue::Ready(value.clone()),
                    };
                    point.provenance.content_sha256 = point_sha256(&point).unwrap();
                    points.push(point);
                }
            } else if q.operation_id == "derive" {
                for source in &request.inputs["source"].points {
                    let mut point = source.clone();
                    point.source_event_id =
                        format!("synthetic-derived:{}", source.provenance.event_end_ms);
                    point.provenance.source_revision = "fixture-calculation-v1".into();
                    point.provenance.upstream_content_sha256 =
                        vec![source.provenance.content_sha256.clone()];
                    point.result = match &source.result {
                        PointValue::Ready(v) => PointValue::Ready(json!(v.as_f64().unwrap() + 1.0)),
                        _ => PointValue::WarmingUp,
                    };
                    point.provenance.content_sha256 = point_sha256(&point).unwrap();
                    points.push(point);
                }
            } else {
                return Err("fixture_operation_unexpected".into());
            }
            let mut outputs: BTreeMap<String, DataBatch> = BTreeMap::from([(
                "value".into(),
                DataBatch {
                    schema: tradeassembly_plugin_sdk::data_contract::SchemaIdentity {
                        schema_ref: SERIES.into(),
                        sha256: hash(&json!({"type":"array","items":{"type":"number"}}))?,
                    },
                    shape: DataShape::Series,
                    points,
                },
            )]);
            if q.operation_id == "derive" && self.late {
                let mut late = outputs["value"].clone();
                for p in &mut late.points {
                    p.result = PointValue::WarmingUp;
                    p.provenance.content_sha256 = point_sha256(p).unwrap();
                }
                outputs.insert("late".into(), late);
            }
            (
                EVALUATION_RESPONSE_SCHEMA,
                serde_json::to_value(DataEvaluationResponse {
                    schema_version: EvaluationVersion::V1,
                    outputs,
                    state: BTreeMap::new(),
                })
                .unwrap(),
            )
        };
        Ok(PluginOperationResponse {
            correlation_id: q.correlation_id.clone(),
            schema_ref: schema.into(),
            content_hash: hash(&payload)?,
            payload,
            observed_at_ms: NOW,
            source_event_id: format!("fixture:{}", q.operation_id),
            freshness_state: "fresh".into(),
            evidence_refs: vec![],
            deterministic: true,
            replayable: true,
            provider_outcome_id: None,
            reconciliation_required: false,
        })
    }
}
fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).unwrap(),
    )
}
#[derive(Default)]
struct Options {
    estimate: bool,
    anchor: bool,
    missing_anchor: bool,
    late: bool,
    planned: bool,
    no_binding: bool,
}
struct Rig {
    _dir: tempfile::TempDir,
    service: TradeAssemblyService,
    request: Value,
    operations: Arc<Operations>,
}
fn port() -> DataPort {
    DataPort {
        schema: tradeassembly_plugin_sdk::data_contract::SchemaIdentity {
            schema_ref: SERIES.into(),
            sha256: hash(&json!({"type":"array","items":{"type":"number"}})).unwrap(),
        },
        shape: DataShape::Series,
        time: TimeContract {
            interval_ms: Some(1000),
            completed_only: true,
        },
    }
}
fn op(id: &str, mode: &str) -> Value {
    json!({"id":id,"capability":format!("example.context.{id}"),"protocol":"local-rust","resourceType":"data","financeResourceType":"data","effect":"read","risk":"low","credentialGrantRequired":false,"accountBindingRequired":false,"apfActionId":"example.context.read","mandateRequired":false,"purpose":"data","evidence":[],"pepCoverageClass":"fixture","checkPacks":[],"receiptClass":"fixture","redaction":"hash_or_ref","noAdvice":true,"detectors":["no_advice"],"traits":{"modes":[mode],"instrumentFamilies":["equity"],"deterministic":true,"replayable":true,"inputSchemaRefs":if id=="derive"{vec![SERIES]}else{vec![]},"outputSchemaRefs":if id=="acquire"{vec![ACQUIRED]}else{vec![SERIES,EVALUATION_RESPONSE_SCHEMA]}},"supportedProtocols":["local-rust"]})
}
fn rig(options: Options) -> Rig {
    rig_in_mode(options, "paper")
}
fn rig_in_mode(options: Options, mode: &str) -> Rig {
    crate::build_identity::install("1111111111111111111111111111111111111111").unwrap();
    use sha2::{Digest, Sha256};
    let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("context.sqlite")
        .to_string_lossy()
        .into_owned();
    let operations = Arc::new(Operations {
        calls: AtomicUsize::new(0),
        origin: if options.estimate {
            "estimated"
        } else {
            "observed"
        },
        anchor: (!options.missing_anchor).then_some(START),
        late: options.late,
    });
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.clock = Arc::new(Clock);
    runtime.identity = Arc::new(Identity);
    runtime.plugin_operations = operations.clone();
    runtime.plugin_packages = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(dir.path().join("packages"))
            .unwrap(),
    );
    let service = TradeAssemblyService::from_runtime(&db, runtime).for_authenticated_invocation(
        "context-fixture",
        "owner",
        None,
        None,
    );
    let source_params = json!({"type":"object","properties":{"values":{"type":"array","minItems":1,"maxItems":256,"items":{"type":"number"}},"originStartMs":{"type":"integer","minimum":1},"originIntervalMs":{"const":1000}},"required":["values","originStartMs","originIntervalMs"],"additionalProperties":false});
    let mut contracts = Vec::new();
    for id in ["provide", "derive"] {
        let mut outputs: BTreeMap<String, DataPort> = BTreeMap::from([("value".into(), port())]);
        if id == "derive" && options.late {
            outputs.insert("late".into(), port());
        }
        let first = outputs
            .keys()
            .map(|out| {
                (
                    out.clone(),
                    if id == "provide" {
                        BTreeMap::new()
                    } else {
                        BTreeMap::from([(
                            "source".into(),
                            Observations::Literal {
                                value: if out == "late" { 5 } else { 1 },
                            },
                        )])
                    },
                )
            })
            .collect();
        contracts.push(OperationDataV3 {
            schema_version: HistoryVersion::V3,
            operation: OperationDataContract {
                schema_version: DataContractVersion::V2,
                operation_id: id.into(),
                semantics_version: "example.1".into(),
                inputs: if id == "derive" {
                    BTreeMap::from([(
                        "source".into(),
                        InputPort {
                            data: port(),
                            required: true,
                            dependency_id: "source".into(),
                        },
                    )])
                } else {
                    BTreeMap::new()
                },
                outputs,
                parameter_schema: if id == "provide" {
                    source_params.clone()
                } else {
                    json!({"type":"object","additionalProperties":false})
                },
                applicability: DataApplicability {
                    asset_classes: vec!["equity".into()],
                    instrument_families: vec!["equity".into()],
                    modes: vec![mode.into()],
                },
                readiness: ReadinessPolicy::AllRequiredInputs,
                warmup: WarmupContract {
                    minimum_observations: 1,
                    observations_parameter: None,
                    additional_observations: 0,
                },
                state: None,
            },
            history: HistoryContract {
                first_ready: first,
                partitioning: Partitioning::InstrumentIndependent,
                required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
                reconstruction: Reconstruction::FiniteWindow,
            },
        });
    }
    let recipe:DataRecipe=serde_json::from_value(json!({"schemaVersion":"tradeassembly.data_recipe.v1","recipeId":"example.offset","semanticsVersion":"example.1","parameterSchema":{"type":"object","additionalProperties":false},"inputs":{"source":port()},"nodes":{"offset":{"operation":{"pluginRef":"self","operationId":"derive","semanticsVersion":"example.1"},"parameters":{}}},"bindings":[{"producer":{"kind":"input","port":"source"},"consumerNode":"offset","inputPort":"source"}],"outputs":if options.late{json!({"value":{"producer":{"kind":"output","node":"offset","port":"value"},"data":port()},"late":{"producer":{"kind":"output","node":"offset","port":"late"},"data":port()}})}else{json!({"value":{"producer":{"kind":"output","node":"offset","port":"value"},"data":port()}})}})).unwrap();
    let recipe_bytes = serde_json::to_vec(&recipe).unwrap();
    let recipe_identity = RecipeIdentity {
        recipe_id: recipe.recipe_id.clone(),
        semantics_version: recipe.semantics_version.clone(),
        canonical_sha256: hash(&recipe).unwrap(),
        document_sha256: digest(&recipe_bytes),
    };
    let series = json!({"type":"array","items":{"type":"number"}});
    let acquired = json!({"type":"object","additionalProperties":false,"properties":{"origin":{"enum":["observed","estimated","simulation","synthetic"]},"values":{"type":"array","items":{"type":"number"}},"eventStartMs":{"type":"integer"},"intervalMs":{"const":1000},"anchorAtMs":{"type":"integer"}},"required":["origin","values","eventStartMs","intervalMs"]});
    let catalog_doc = json!({"schemaVersion":"example.catalog.v1","entries":["example.offset"]});
    let catalog_bytes = serde_json::to_vec(&catalog_doc).unwrap();
    let catalog = Catalog {
        schema_version: "example.catalog.v1".into(),
        byte_sha256: digest(&catalog_bytes),
        canonical_sha256: hash(&catalog_doc).unwrap(),
        canonicalization: "tradeassembly.canonical-json.v1".into(),
    };
    let mut gates = vec![
        json!({"kind":"host_capture","input":"source","acquisitionSchema":{"schemaRef":ACQUIRED,"canonicalSha256":hash(&acquired).unwrap()},"originPointer":"/origin"}),
        json!({"kind":"strict_fixed_interval_history"}),
    ];
    if options.anchor {
        gates.push(json!({"kind":"capture_anchor","input":"source","anchorPointer":"/anchorAtMs","boundary":"window_start"}));
    }
    let output = |name: &str| json!({"name":name,"implementationStatus":if options.planned{"planned_only"}else{"implemented"},"schema":{"schemaRef":SERIES,"canonicalSha256":hash(&series).unwrap()},"origin":"derived","modelId":"example.offset","modelVersion":"1","units":"price","estimate":{"kind":"never"},"gates":gates});
    let mut bound_outputs = vec![output("value")];
    if options.late {
        bound_outputs.push(output("late"));
    }
    let profile_recipe = if options.planned {
        None
    } else {
        Some(recipe_identity.clone())
    };
    let binding = json!({"schemaVersion":"tradeassembly.indicator_profile_bindings.v1","catalog":catalog,"profiles":[{"conceptId":"example.context","profileId":"example.offset.fixed","profileVersion":"1","recipe":profile_recipe,"outputs":bound_outputs}]});
    let mut manifest = json!({"metadata":{"id":"example.context","version":"1","provider":{"id":"example.context","name":"Synthetic fixture"}},"capabilities":[{"id":"example.context.acquire"},{"id":"example.context.provide"},{"id":"example.context.derive"}],"operations":[op("acquire",mode),op("provide",mode),op("derive",mode)],"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    if !options.no_binding {
        manifest["metadata"]["indicatorProfileBindings"] = binding;
    }
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let binary = b"synthetic-port-test-not-executed";
    let mut files = vec![
        ("manifest.json".to_string(), manifest_bytes.clone()),
        ("bin/plugin".to_string(), binary.to_vec()),
        ("recipe.json".to_string(), recipe_bytes.clone()),
    ];
    let mut descriptor = json!({"packageContractVersion":"3","package":{"packageContractVersion":"1","plugin":{"id":"example.context","version":"1"},"manifest":{"path":"manifest.json","sha256":digest(&manifest_bytes)},"targets":[{"target":tradeassembly_plugin_sdk::host_target(),"binary":{"path":"bin/plugin","sha256":digest(binary)}}],"compatibility":{"hostContract":{"minimum":"1","maximum":"1"},"sdkVersion":tradeassembly_plugin_sdk::SDK_VERSION}},"dataContracts":[],"dataSchemas":[],"recipes":[{"recipeId":"example.offset","document":{"path":"recipe.json","sha256":digest(&recipe_bytes)},"canonicalSha256":hash(&recipe).unwrap()}]});
    for (id, value) in [
        (SERIES, series),
        (ACQUIRED, acquired),
        (EVALUATION_RESPONSE_SCHEMA, json!({"type":"object"})),
    ] {
        let bytes = serde_json::to_vec(&value).unwrap();
        let path = format!("schema-{}.json", files.len());
        descriptor["dataSchemas"].as_array_mut().unwrap().push(json!({"schemaRef":id,"document":{"path":path,"sha256":digest(&bytes)},"canonicalSha256":hash(&value).unwrap()}));
        files.push((path, bytes));
    }
    for contract in contracts {
        let bytes = serde_json::to_vec(&contract).unwrap();
        let path = format!("contract-{}.json", contract.operation.operation_id);
        descriptor["dataContracts"].as_array_mut().unwrap().push(json!({"operationId":contract.operation.operation_id,"document":{"path":path,"sha256":digest(&bytes)},"canonicalSha256":hash(&contract).unwrap()}));
        files.push((path, bytes));
    }
    files.push((
        "tradeassembly-plugin.json".into(),
        serde_json::to_vec(&descriptor).unwrap(),
    ));
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(gzip);
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, path, bytes.as_slice())
            .unwrap();
    }
    let archive = archive.into_inner().unwrap().finish().unwrap();
    let path = dir.path().join("package.tar.gz");
    std::fs::write(&path, &archive).unwrap();
    let package = service
        .runtime()
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: digest(&archive),
            manifest_sha256: hash(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    service.runtime().plugins.install_manifest("example.context",json!({"pluginRef":"example.context","manifest":manifest,"manifestDigest":package.manifest_sha256,"source":{"type":"file"},"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&context("publisher-install")).unwrap();
    let instance=service.handle_http("POST","/plugins/instances",json!({"instanceRef":"example-context","pluginRef":"example.context","enabled":true,"configuration":{},"idempotencyKey":"fixture-instance"}));
    assert_eq!(instance.status, 200, "{}", instance.body);
    for capability in ["acquire", "provide", "derive"] {
        let name = format!("example.context.{capability}");
        let grant = EntitlementGrant {
            grant_id: name.clone(),
            profile: "local_full".into(),
            source: "synthetic-test".into(),
            capability: name.clone(),
            effect: "allow".into(),
            state: "active".into(),
            scope: "global".into(),
            source_type: "test".into(),
            source_ref: None,
            precedence: 10,
            limits: BTreeMap::new(),
            issued_at: None,
            expires_at: None,
            revoked_at: None,
            reason: None,
        };
        service
            .runtime()
            .storage
            .put_json(
                "entitlement_grants",
                &name,
                serde_json::to_value(grant).unwrap(),
                &context("fixture-explicit-grants"),
            )
            .unwrap();
    }
    let spec: Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/strategy-spec/v3/valid/static-equity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name":"Generic contextual support fixture","spec":spec}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let id = created.body["body"]["strategy"]["id"].as_str().unwrap();
    let publish=service.handle_http("POST","/product/strategies/publish",json!({"strategyId":id,"expectedDraftHash":created.body["body"]["draft"]["draftHash"],"actor":{"kind":"user","id":"owner"},"idempotencyKey":"fixture-publish"}));
    assert_eq!(publish.body["body"]["published"], true, "{}", publish.body);
    let scope = data_plans::PlanScope {
        strategy_id: id.into(),
        strategy_version_id: publish.body["body"]["version"]["id"]
            .as_str()
            .unwrap()
            .into(),
        strategy_spec_hash: publish.body["body"]["version"]["specHash"]
            .as_str()
            .unwrap()
            .into(),
    };
    let revision = capability_graph::save_revision(
        &service,
        json!({"configId":"fixture-context","strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":mode,"bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}},"additionalRequirements":[{"requirementId":"source","capability":"example.context.provide","mode":mode,"declaration":"required","fallbackPolicy":"none"},{"requirementId":"calc","capability":"example.context.derive","mode":mode,"declaration":"required","fallbackPolicy":"none"},{"requirementId":"acquisition","capability":"example.context.acquire","mode":mode,"declaration":"required","fallbackPolicy":"none"}]}),
    );
    assert_eq!(revision["revision"]["graph"]["ok"], true, "{revision}");
    let graph = revision["revision"]["revisionId"]
        .as_str()
        .unwrap()
        .to_string();
    let data_context = crate::data_pipeline::DataContext {
        instruments: vec![InstrumentContext {
            instrument_id: "TEST".into(),
            asset_class: "equity".into(),
            instrument_family: "equity".into(),
        }],
        interval_ms: Some(1000),
    };
    let capture = data_captures::acquire(
        &service,
        data_captures::CaptureCommand {
            policy: data_captures::CapturePolicy {
                scope: scope.clone(),
                capability_graph_revision: graph.clone(),
                mode: mode.into(),
                acquisition_node: "acquisition".into(),
                replay_node: "source".into(),
                parameter_extracts: BTreeMap::from([
                    ("values".into(), "/values".into()),
                    ("originStartMs".into(), "/eventStartMs".into()),
                    ("originIntervalMs".into(), "/intervalMs".into()),
                ]),
                context: data_context.clone(),
                event_start_ms: START,
                event_end_ms: START + 3000,
                gap_policy: data_captures::GapPolicy::StrictFixedInterval,
            },
            input: json!({}),
        },
        &context("fixture-capture"),
    )
    .unwrap();
    let selected = if options.late {
        vec!["value".to_string(), "late".to_string()]
    } else {
        vec!["value".to_string()]
    };
    let program = data_recipes::RecipeProgram {
        schema_version: data_recipes::RecipeProgramVersion::V1,
        base_request: crate::data_pipeline::PipelineRequest {
            selected_outputs: vec![],
            source_captures: BTreeMap::from([("source".into(), capture.capture_id.clone())]),
            capability_graph_revision: graph.clone(),
            mode: mode.into(),
            trade_instruments: vec![],
            nodes: vec![crate::data_pipeline::PipelineNode {
                graph_node_id: "source".into(),
                parameters: capture.replay_parameters.clone(),
                context: data_context.clone(),
                input_contexts: BTreeMap::new(),
            }],
            bindings: vec![],
        },
        instances: vec![data_recipes::RecipeInvocation {
            instance_id: "selected".into(),
            plugin_instance_ref: "example-context".into(),
            package_sha256: package.package_sha256.clone(),
            recipe_id: "example.offset".into(),
            semantics_version: "example.1".into(),
            parameters: json!({}),
            nodes: BTreeMap::from([(
                "offset".into(),
                data_recipes::RecipeNodeBinding {
                    graph_node_id: "calc".into(),
                    context: data_context,
                    input_contexts: BTreeMap::new(),
                },
            )]),
            inputs: BTreeMap::from([(
                "source".into(),
                data_recipes::RecipeInput::Pipeline {
                    node: "source".into(),
                    port: "value".into(),
                },
            )]),
            selected_outputs: selected.clone(),
        }],
        outputs: selected
            .iter()
            .map(|out| data_recipes::RecipeInput::Recipe {
                instance: "selected".into(),
                output: out.clone(),
            })
            .collect(),
    };
    let plan = data_recipes::save(
        &service,
        scope.clone(),
        program,
        &context("fixture-real-plan"),
    )
    .unwrap();
    let step = data_evaluation::EvaluationStep {
        run_id: "fixture-context-evaluation".into(),
        plan_revision_id: plan.revision_id.clone(),
        window: EvaluationWindow {
            event_start_ms: START,
            event_end_ms: START + 3000,
            as_of_ms: NOW,
        },
        expected_checkpoint_sha256: None,
    };
    let receipt =
        data_evaluation::step(&service, step.clone(), &context("fixture-real-evaluation")).unwrap();
    let request = json!({"schemaVersion":"tradeassembly.indicator_context_request.v1","scope":scope,"planRevisionId":plan.revision_id,"recipeInstanceId":"selected","selection":{"pluginInstanceRef":"example-context","packageSha256":package.package_sha256,"catalog":catalog,"conceptId":"example.context","profileId":"example.offset.fixed","profileVersion":"1","recipe":profile_recipe,"selectedOutputs":selected},"subjectInstrumentId":"TEST","evaluation":{"receiptSha256":receipt.receipt_sha256,"step":step},"allowEstimates":false});
    Rig {
        _dir: dir,
        service,
        request,
        operations,
    }
}
type NamespaceSnapshot = (String, String, Vec<(String, Value)>);
fn snapshot(s: &TradeAssemblyService) -> Vec<NamespaceSnapshot> {
    [
        "compiled_data_plan_revisions_v1",
        "compiled_data_plan_idempotency_v1",
        "data_evaluation_checkpoints_v1",
        "data_evaluation_receipts_v1",
        "data_evaluation_receipt_hash_index_v1",
        "data_acquisition_receipts_v1",
        "plugin_operation_receipts",
        "entitlement_grants",
        "control_plane_commands",
        "orders",
    ]
    .into_iter()
    .map(|ns| {
        (
            ns.into(),
            "owned-read-snapshot".into(),
            s.runtime().storage.list_json(ns).unwrap(),
        )
    })
    .collect()
}
struct AgentFixtureClock(AtomicI64);
impl VersionedPort for AgentFixtureClock {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl ClockPort for AgentFixtureClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
fn contextual_verified_paper_agent_refuses_live_plan_and_expired_real_lease() {
    for (mode, allowed_tool) in [
        ("live", TOOL),
        ("paper", TOOL),
        ("paper", "tradeassembly.health"),
    ] {
        // All mode-bearing package, contract, graph, capture, and compiled plan
        // documents are declared in the selected mode before normal preparation.
        // These are synthetic read operations; no live trading is activated.
        let rig = rig_in_mode(Options::default(), mode);
        let clock = Arc::new(AgentFixtureClock(AtomicI64::new(NOW)));
        let mut runtime = rig.service.runtime().as_ref().clone();
        runtime.clock = clock.clone();
        let service = rig.service.clone().with_test_runtime(runtime);
        let ordinary = service.handle_http("POST", PATH, rig.request.clone());
        assert_eq!(ordinary.status, 200, "{}", ordinary.body);
        assert_eq!(ordinary.body["subject"]["mode"], mode);
        let (context, lease) = crate::agent_runner::tests::resolved_report_context_fixture(
            &service,
            "paper",
            allowed_tool,
        );
        let agent = service.with_verified_agent_mcp_execution_context(context.clone());
        let before = snapshot(&service);
        let calls = rig.operations.calls.load(Ordering::SeqCst);
        if allowed_tool != TOOL {
            let http = agent.handle_http("POST", PATH, rig.request.clone());
            assert_eq!(http.status, 400, "{}", http.body);
            assert_eq!(http.body["error"]["code"], "agent_mcp_tool_not_allowed");
            let mcp = agent.call_mcp_tool(TOOL, json!({"request":rig.request}));
            assert_eq!(mcp["isError"], true, "{mcp}");
            assert!(mcp.to_string().contains("agent_mcp_tool_not_allowed"));
        } else if mode == "live" {
            let http = agent.handle_http("POST", PATH, rig.request.clone());
            assert_eq!(http.status, 400, "{}", http.body);
            assert_eq!(
                http.body["error"]["code"],
                "agent_mcp_execution_context_mismatch"
            );
            let mcp = agent.call_mcp_tool(TOOL, json!({"request":rig.request}));
            assert_eq!(mcp["isError"], true, "{mcp}");
            assert!(mcp
                .to_string()
                .contains("agent_mcp_execution_context_mismatch"));
        } else {
            let admitted = agent.handle_http("POST", PATH, rig.request.clone());
            assert_eq!(admitted.status, 200, "{}", admitted.body);
            let admitted_mcp = agent.call_mcp_tool(TOOL, json!({"request":rig.request.clone()}));
            assert_eq!(admitted_mcp["isError"], false, "{admitted_mcp}");
            clock.0.store(lease.expires_at_ms + 1, Ordering::SeqCst);
            // The opaque capability and active pointer still resolve. Only the
            // authoritative supervisor lease is expired; no storage is rewritten.
            context
                .revalidate(service.runtime().storage.as_ref(), clock.as_ref())
                .unwrap();
            assert_eq!(
                context
                    .current_lease(
                        service.runtime().storage.as_ref(),
                        clock.as_ref(),
                        service.runtime().leases.as_ref(),
                    )
                    .unwrap_err(),
                "agent_run_lease_invalid"
            );
            let http = agent.handle_http("POST", PATH, rig.request.clone());
            assert_eq!(http.status, 400, "{}", http.body);
            assert_eq!(http.body["error"]["code"], "agent_run_lease_invalid");
        }
        assert_eq!(snapshot(&service), before);
        assert_eq!(rig.operations.calls.load(Ordering::SeqCst), calls);
    }
}

#[test]
fn contextual_available_uses_normal_owned_capture_plan_and_original_evaluation_without_effects() {
    let rig = rig(Options::default());
    let before = snapshot(&rig.service);
    let calls = rig.operations.calls.load(Ordering::SeqCst);
    let response = report(&rig.service, &rig.request).unwrap();
    assert_eq!(
        response["outputs"][0]["availability"], "available",
        "{response}"
    );
    assert_eq!(response["axes"]["sourceAuthority"], "host_acquisition");
    assert_eq!(response["axes"]["vendorEquivalence"], "not_checked");
    assert_eq!(response["executionAuthorityGranted"], false);
    assert_eq!(response["providerCalls"], 0);
    assert_eq!(response["persistentWrites"], 0);
    assert_eq!(snapshot(&rig.service), before);
    assert_eq!(rig.operations.calls.load(Ordering::SeqCst), calls);
    let http = rig.service.handle_http("POST", PATH, rig.request.clone());
    assert_eq!(http.status, 200, "{}", http.body);
    assert_eq!(http.body, response);
    let mcp = rig
        .service
        .call_mcp_tool(TOOL, json!({"request":rig.request}));
    assert_eq!(mcp["isError"], false, "{mcp}");
    let text: Value = serde_json::from_str(mcp["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, mcp["structuredContent"]);
    assert_eq!(text, response);
    if let Ok(path) = std::env::var("INDICATOR_CONTEXT_EVIDENCE_DIR") {
        let path = std::path::Path::new(&path);
        assert!(path.is_absolute());
        std::fs::create_dir_all(path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        for (name, value) in [
            ("actual-request.json", rig.request.clone()),
            (
                "actual-http-response.json",
                json!({"status":http.status,"body":http.body}),
            ),
            ("actual-mcp-response.json", mcp.clone()),
        ] {
            let file = path.join(name);
            assert!(
                !file.exists(),
                "fixture evidence must use a fresh directory"
            );
            std::fs::write(&file, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }
    assert_eq!(snapshot(&rig.service), before);
    assert_eq!(rig.operations.calls.load(Ordering::SeqCst), calls);
}
#[test]
fn contextual_estimate_consent_clears_only_consent_and_keeps_missing_anchor() {
    let rig = rig(Options {
        estimate: true,
        anchor: true,
        missing_anchor: true,
        ..Default::default()
    });
    let denied = report(&rig.service, &rig.request).unwrap();
    let blockers = denied["outputs"][0]["blockers"].as_array().unwrap();
    assert!(blockers
        .iter()
        .any(|b| b["availability"] == "estimate_opt_in_required"));
    assert!(blockers
        .iter()
        .any(|b| b["availability"] == "missing_anchor"));
    let mut allowed = rig.request.clone();
    allowed["allowEstimates"] = json!(true);
    let report = report(&rig.service, &allowed).unwrap();
    assert_ne!(report["outputs"][0]["availability"], "available");
    let blockers = report["outputs"][0]["blockers"].as_array().unwrap();
    assert!(!blockers
        .iter()
        .any(|b| b["availability"] == "estimate_opt_in_required"));
    assert!(blockers
        .iter()
        .any(|b| b["availability"] == "missing_anchor"));
}
#[test]
fn contextual_partial_outputs_do_not_promote_warming_output() {
    let rig = rig(Options {
        late: true,
        ..Default::default()
    });
    let report = report(&rig.service, &rig.request).unwrap();
    assert_eq!(
        report["outputs"][0]["availability"], "available",
        "{report}"
    );
    assert_ne!(report["outputs"][1]["availability"], "available");
    assert!(report["outputs"][1]["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["availability"] == "missing_history"));
}
#[test]
fn contextual_planned_profile_does_not_use_ready_related_calculation() {
    let rig = rig(Options {
        planned: true,
        ..Default::default()
    });
    let report = report(&rig.service, &rig.request).unwrap();
    assert_eq!(report["outputs"][0]["availability"], "planned_only");
    assert_eq!(report["validUntilMs"], report["inspectedAtMs"]);
}
#[test]
fn contextual_legacy_package_without_binding_is_unknown_not_available() {
    let rig = rig(Options {
        no_binding: true,
        ..Default::default()
    });
    let report = report(&rig.service, &rig.request).unwrap();
    assert_eq!(report["outputs"][0]["availability"], "not_checked");
    assert_eq!(
        report["outputs"][0]["blockers"][0]["code"],
        "catalog_not_admitted"
    );
}
#[test]
fn contextual_owner_source_and_original_window_are_independently_bound() {
    let rig = rig(Options::default());
    let before = rig.operations.calls.load(Ordering::SeqCst);
    let foreign =
        rig.service
            .for_authenticated_invocation("context-fixture", "other-owner", None, None);
    assert!(report(&foreign, &rig.request).is_err());
    let stored = snapshot(&rig.service);
    let http = foreign.handle_http("POST", PATH, rig.request.clone());
    assert_eq!(http.status, 404, "{}", http.body);
    let mcp = foreign.call_mcp_tool(TOOL, json!({"request":rig.request}));
    assert_eq!(mcp["isError"], true, "{mcp}");
    assert!(mcp["structuredContent"]["outputs"].is_null());
    assert_eq!(snapshot(&rig.service), stored);
    let mut changed = rig.request.clone();
    changed["evaluation"]["step"]["window"]["asOfMs"] = json!(NOW - 1);
    let report = report(&rig.service, &changed).unwrap();
    assert_ne!(report["outputs"][0]["availability"], "available");
    let mut instance = rig
        .service
        .runtime()
        .plugins
        .get_instance("example-context")
        .unwrap()
        .unwrap();
    instance["configuration"] = json!({"changed":true});
    rig.service
        .runtime()
        .plugins
        .put_instance(
            "example-context",
            instance,
            &context("fixture-change-config"),
        )
        .unwrap();
    let report = super::report(&rig.service, &rig.request).unwrap();
    assert_ne!(report["outputs"][0]["availability"], "available");
    assert_eq!(rig.operations.calls.load(Ordering::SeqCst), before);
}
#[test]
fn contextual_checked_entitlement_denial_is_distinct_from_unchecked() {
    let rig = rig(Options::default());
    let mut grant = rig
        .service
        .runtime()
        .storage
        .get_json("entitlement_grants", "example.context.derive")
        .unwrap()
        .unwrap();
    grant["effect"] = json!("deny");
    grant["precedence"] = json!(100);
    rig.service
        .runtime()
        .storage
        .put_json(
            "entitlement_grants",
            "example.context.derive",
            grant,
            &context("fixture-revoke-calculation"),
        )
        .unwrap();
    let report = report(&rig.service, &rig.request).unwrap();
    assert_ne!(report["outputs"][0]["availability"], "available");
    assert!(
        report["outputs"][0]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["availability"] == "missing_entitlement"),
        "{report}"
    );
}
#[test]
fn contextual_public_contracts_reject_unknown_versions_fields_and_gates() {
    let request: Value = serde_json::from_str(include_str!("synthetic-request.json")).unwrap();
    assert!(validate(&request, &request_schema(), 65_536).is_ok());
    let mut bare = request.clone();
    bare["scope"]["strategySpecHash"] = json!("7".repeat(64));
    assert!(validate(&bare, &request_schema(), 65_536).is_err());
    for field in ["schemaVersion", "authority", "unknown"] {
        let mut bad = request.clone();
        bad[field] = json!("future-or-forged");
        assert!(validate(&bad, &request_schema(), 65_536).is_err());
    }
    let mut binding: Value = serde_json::from_str(include_str!("synthetic-binding.json")).unwrap();
    assert!(validate(&binding, &binding_schema(), 262_144).is_ok());
    binding["profiles"][0]["outputs"][0]["gates"][0]["kind"] = json!("unadopted_calendar_guess");
    assert!(validate(&binding, &binding_schema(), 262_144).is_err());
}
#[test]
fn contextual_service_recreation_reads_the_same_actual_owned_evidence() {
    let rig = rig(Options::default());
    let first = report(&rig.service, &rig.request).unwrap();
    let calls = rig.operations.calls.load(Ordering::SeqCst);
    let db = rig.service.db().to_string();
    let mut runtime = crate::adapters::local::test_runtime(&db);
    runtime.clock = Arc::new(Clock);
    runtime.identity = Arc::new(Identity);
    runtime.plugin_operations = rig.operations.clone();
    runtime.plugin_packages = Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            rig._dir.path().join("packages"),
        )
        .unwrap(),
    );
    let restarted = TradeAssemblyService::from_runtime(&db, runtime).for_authenticated_invocation(
        "context-fixture",
        "owner",
        None,
        None,
    );
    let before = snapshot(&restarted);
    let second = report(&restarted, &rig.request).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        second["outputs"][0]["availability"], "available",
        "{second}"
    );
    assert_eq!(before, snapshot(&restarted));
    assert_eq!(calls, rig.operations.calls.load(Ordering::SeqCst));
}
