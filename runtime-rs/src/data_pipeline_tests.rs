use super::*;
use crate::ports::*;
use serde_json::json;

fn operation(id: &str) -> PluginOperationContract {
    serde_json::from_value(json!({
        "id":id,"capability":format!("test.{id}"),"protocol":"local-rust",
        "resourceType":"data","financeResourceType":"data","effect":"read","risk":"low",
        "credentialGrantRequired":false,"accountBindingRequired":false,"apfActionId":"test.read",
        "mandateRequired":false,"purpose":"test","evidence":[],"pepCoverageClass":"c1",
        "checkPacks":[],"receiptClass":"test","redaction":"hash_or_ref","noAdvice":true,"detectors":["no_advice"],
        "traits":{"modes":["paper"],"instrumentFamilies":["crypto_asset"],
            "inputSchemaRefs":["schema://test/series@1"],"outputSchemaRefs":["schema://test/series@1"]},
        "supportedProtocols":["local-rust"]
    })).unwrap()
}
fn port() -> DataPort {
    DataPort {
        schema: SchemaIdentity {
            schema_ref: "schema://test/series@1".into(),
            sha256: "a".repeat(64),
        },
        shape: DataShape::Series,
        time: TimeContract {
            interval_ms: Some(86_400_000),
            completed_only: true,
        },
    }
}
fn context() -> DataContext {
    DataContext {
        instruments: vec![InstrumentContext {
            instrument_id: "BTC/USD".into(),
            asset_class: "crypto".into(),
            instrument_family: "crypto_asset".into(),
        }],
        interval_ms: Some(86_400_000),
    }
}
fn contract(id: &str, input: bool) -> OperationDataContract {
    OperationDataContract {
        schema_version: DataContractVersion::V1,
        operation_id: id.into(),
        semantics_version: "test.1".into(),
        inputs: if input {
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
        outputs: BTreeMap::from([("value".into(), port())]),
        parameter_schema: if input {
            json!({"type":"object","properties":{"window":{"type":"integer","minimum":1}},"required":["window"],"additionalProperties":false})
        } else {
            json!({"type":"object","additionalProperties":false})
        },
        applicability: DataApplicability {
            asset_classes: vec!["crypto".into()],
            instrument_families: vec!["crypto_asset".into()],
            modes: vec!["paper".into()],
        },
        readiness: ReadinessPolicy::AllRequiredInputs,
        warmup: WarmupContract {
            minimum_observations: 1,
            observations_parameter: input.then(|| "window".into()),
            additional_observations: 0,
        },
        state: None,
    }
}
fn seal(contracts: &mut [VerifiedDataContract]) {
    for c in contracts {
        c.contract_sha256 = canonical_sha256(&c.contract).unwrap();
    }
}
// The fixture selects two separate plugins through the existing real resolver.
fn make_fixture(
    typed: bool,
    derived_count: usize,
) -> (
    CapabilityGraphResolution,
    PipelineRequest,
    Vec<VerifiedDataContract>,
) {
    let provider = operation("provide");
    let mut derive = operation("derive");
    if !typed {
        derive.dependencies.push(PluginOperationDependency {
            dependency_id: "source".into(),
            capability: provider.capability.clone(),
            operation_id: Some(provider.id.clone()),
            required: true,
            traits: Default::default(),
        });
    }
    let plugins = [provider, derive]
        .into_iter()
        .map(|op| PluginCatalogEntry {
            instance_ref: format!("instance.{}", op.id),
            plugin_ref: format!("plugin.{}", op.id),
            provider_ref: format!("provider.{}", op.id),
            account_ref: None,
            name: op.id.clone(),
            enabled: true,
            trust_level: "test".into(),
            manifest_fingerprint: format!("manifest.{}", op.id),
            capabilities: vec![op.capability.clone()],
            operations: vec![op],
            credential_status: CredentialResolutionStatus {
                configured: true,
                custody: "test".into(),
                status: "stored".into(),
                revision_ref: None,
            },
            health: "ok".into(),
        })
        .collect();
    let entitlements = ["test.provide", "test.derive"]
        .into_iter()
        .map(|cap| EntitlementGrant {
            grant_id: cap.into(),
            profile: "local_full".into(),
            source: "test".into(),
            capability: cap.into(),
            effect: "allow".into(),
            state: "active".into(),
            scope: "global".into(),
            source_type: "test".into(),
            source_ref: None,
            precedence: 10,
            limits: Default::default(),
            issued_at: None,
            expires_at: None,
            revoked_at: None,
            reason: None,
        })
        .collect();
    let graph = crate::capability::resolve_graph(
        CapabilityGraphRequest {
            requirements: {
                let mut requirements = (0..derived_count)
                    .map(|index| CapabilityRequirement {
                        requirement_id: if index == 0 {
                            "result".into()
                        } else {
                            format!("result_{index}")
                        },
                        capability: "test.derive".into(),
                        mode: "paper".into(),
                        declaration: "required".into(),
                        fallback_policy: "none".into(),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                if typed {
                    requirements.push(CapabilityRequirement {
                        requirement_id: "source".into(),
                        capability: "test.provide".into(),
                        mode: "paper".into(),
                        declaration: "required".into(),
                        fallback_policy: "none".into(),
                        ..Default::default()
                    });
                }
                requirements
            },
            evaluation_epoch: "test-epoch".into(),
            ..Default::default()
        },
        CapabilityResolverCatalog {
            plugins,
            entitlements,
        },
    );
    assert!(graph.ok, "{graph:#?}");
    let source = graph
        .nodes
        .iter()
        .find(|n| n.selected.as_ref().unwrap().operation_id == "provide")
        .unwrap()
        .node_id
        .clone();
    let target = graph
        .nodes
        .iter()
        .find(|n| n.selected.as_ref().unwrap().operation_id == "derive")
        .unwrap()
        .node_id
        .clone();
    let nodes = graph
        .nodes
        .iter()
        .map(|n| {
            let input = n.selected.as_ref().unwrap().operation_id == "derive";
            PipelineNode {
                graph_node_id: n.node_id.clone(),
                parameters: if input {
                    json!({"window":5})
                } else {
                    json!({})
                },
                context: context(),
                input_contexts: BTreeMap::new(),
            }
        })
        .collect();
    let contracts = graph
        .nodes
        .iter()
        .map(|n| {
            let s = n.selected.as_ref().unwrap();
            let mut c = contract(&s.operation_id, s.operation_id == "derive");
            if typed {
                c.schema_version = DataContractVersion::V2;
            }
            VerifiedDataContract {
                graph_node_id: n.node_id.clone(),
                manifest_fingerprint: s.manifest_fingerprint.clone(),
                contract_sha256: canonical_sha256(&c).unwrap(),
                contract: c.into(),
            }
        })
        .collect();
    let request = PipelineRequest {
        selected_outputs: vec![],
        source_captures: BTreeMap::new(),
        capability_graph_revision: "revision.test".into(),
        mode: "paper".into(),
        trade_instruments: vec![],
        nodes,
        bindings: if typed {
            graph
                .nodes
                .iter()
                .filter(|n| n.selected.as_ref().unwrap().operation_id == "derive")
                .map(|n| PortBinding {
                    producer_node: if n.node_id == "result_2" {
                        "result_1".into()
                    } else {
                        source.clone()
                    },
                    output_port: "value".into(),
                    consumer_node: n.node_id.clone(),
                    input_port: "source".into(),
                })
                .collect()
        } else {
            vec![PortBinding {
                producer_node: source,
                output_port: "value".into(),
                consumer_node: target,
                input_port: "source".into(),
            }]
        },
    };
    (graph, request, contracts)
}
pub(crate) fn fixture() -> (
    CapabilityGraphResolution,
    PipelineRequest,
    Vec<VerifiedDataContract>,
) {
    make_fixture(false, 1)
}

fn has(errors: &[PlanDiagnostic], code: PlanDiagnosticCode) -> bool {
    errors.iter().any(|e| e.code == code)
}
#[test]
fn data_pipeline_compiles_cross_plugin_dependencies_with_stable_identity() {
    let (graph, request, contracts) = fixture();
    let plan = compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap();
    assert_eq!(plan.execution_order[0], request.bindings[0].producer_node);
    assert_eq!(
        plan.source_warmup_observations[&request.bindings[0].consumer_node],
        5
    );
    let mut reordered = request.clone();
    reordered.nodes.reverse();
    let mut reversed = contracts.clone();
    reversed.reverse();
    assert_eq!(
        plan,
        compile_data_plan(&graph, reordered, reversed).unwrap()
    );
    let mut changed = request;
    changed
        .nodes
        .iter_mut()
        .find(|n| n.parameters.get("window").is_some())
        .unwrap()
        .parameters = json!({"window":6});
    assert_ne!(
        plan.plan_sha256,
        compile_data_plan(&graph, changed, contracts)
            .unwrap()
            .plan_sha256
    );
}
#[test]
fn data_pipeline_missing_input_is_actionable() {
    let (graph, mut request, contracts) = fixture();
    request.bindings.clear();
    let errors = compile_data_plan(&graph, request, contracts).unwrap_err();
    let missing = errors
        .iter()
        .find(|e| e.code == PlanDiagnosticCode::MissingInput)
        .unwrap();
    assert_eq!(missing.port.as_deref(), Some("source"));
    assert!(missing.message.contains("schema://test/series@1"));
}
#[test]
fn data_pipeline_rejects_schema_drift_incomplete_data_and_symbol_substitution() {
    let (graph, request, mut contracts) = fixture();
    let original = contracts.clone();
    contracts
        .iter_mut()
        .find(|c| c.contract.operation_id == "provide")
        .unwrap()
        .contract
        .outputs
        .get_mut("value")
        .unwrap()
        .schema
        .sha256 = "b".repeat(64);
    seal(&mut contracts);
    assert!(has(
        &compile_data_plan(&graph, request.clone(), contracts).unwrap_err(),
        PlanDiagnosticCode::IncompatibleSchema
    ));
    let mut contracts = original.clone();
    contracts
        .iter_mut()
        .find(|c| c.contract.operation_id == "provide")
        .unwrap()
        .contract
        .outputs
        .get_mut("value")
        .unwrap()
        .time
        .completed_only = false;
    seal(&mut contracts);
    assert!(has(
        &compile_data_plan(&graph, request.clone(), contracts).unwrap_err(),
        PlanDiagnosticCode::IncompatibleTimeContext
    ));
    let mut request = request;
    let source = request.bindings[0].producer_node.clone();
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == source)
        .unwrap()
        .context
        .instruments[0]
        .instrument_id = "ETH/USD".into();
    assert!(has(
        &compile_data_plan(&graph, request, original).unwrap_err(),
        PlanDiagnosticCode::IncompatibleTimeContext
    ));
}
#[test]
fn data_pipeline_rejects_unpinned_contracts_invalid_parameters_and_blocked_graphs() {
    let (graph, request, contracts) = fixture();
    let mut bad = contracts.clone();
    bad[0].manifest_fingerprint = "other".into();
    assert!(has(
        &compile_data_plan(&graph, request.clone(), bad).unwrap_err(),
        PlanDiagnosticCode::InvalidContract
    ));
    let mut bad = contracts.clone();
    bad[0].contract.semantics_version = "changed".into();
    assert!(has(
        &compile_data_plan(&graph, request.clone(), bad).unwrap_err(),
        PlanDiagnosticCode::InvalidContract
    ));
    let mut params = request.clone();
    params
        .nodes
        .iter_mut()
        .find(|n| n.parameters.get("window").is_some())
        .unwrap()
        .parameters = json!({"window":0});
    assert!(has(
        &compile_data_plan(&graph, params, contracts.clone()).unwrap_err(),
        PlanDiagnosticCode::InvalidParameters
    ));
    let mut blocked = graph.clone();
    blocked.ok = false;
    assert!(has(
        &compile_data_plan(&blocked, request.clone(), contracts.clone()).unwrap_err(),
        PlanDiagnosticCode::GraphBlocked
    ));
    let mut mode = request;
    mode.mode = "live".into();
    assert!(has(
        &compile_data_plan(&graph, mode, contracts).unwrap_err(),
        PlanDiagnosticCode::UnsupportedContext
    ));
}
#[test]
fn data_pipeline_data_context_can_differ_from_traded_instruments() {
    let (graph, mut request, contracts) = fixture();
    request.trade_instruments = vec![InstrumentContext {
        instrument_id: "broker-unsupported".into(),
        asset_class: "future".into(),
        instrument_family: "future_contract".into(),
    }];
    assert!(compile_data_plan(&graph, request.clone(), contracts.clone()).is_ok());
    let source = request.bindings[0].producer_node.clone();
    let target = request.bindings[0].consumer_node.clone();
    let mut external = context();
    external.instruments[0].instrument_id = "ETH/USD".into();
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == source)
        .unwrap()
        .context = external.clone();
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == target)
        .unwrap()
        .input_contexts
        .insert("source".into(), external);
    assert!(compile_data_plan(&graph, request, contracts).is_ok());
}
#[test]
fn data_pipeline_rejects_undeclared_edges_duplicate_inputs_and_cycles() {
    let (graph, request, contracts) = fixture();
    let mut missing = graph.clone();
    missing.edges.clear();
    assert!(has(
        &compile_data_plan(&missing, request.clone(), contracts.clone()).unwrap_err(),
        PlanDiagnosticCode::UndeclaredDependency
    ));
    let mut duplicate = request.clone();
    duplicate.bindings.push(duplicate.bindings[0].clone());
    assert!(has(
        &compile_data_plan(&graph, duplicate, contracts.clone()).unwrap_err(),
        PlanDiagnosticCode::DuplicateInput
    ));
    let mut graph = graph;
    let mut request = request;
    let mut contracts = contracts;
    let forward = request.bindings[0].clone();
    contracts
        .iter_mut()
        .find(|c| c.contract.operation_id == "provide")
        .unwrap()
        .contract
        .inputs
        .insert(
            "source".into(),
            InputPort {
                data: port(),
                required: true,
                dependency_id: "source".into(),
            },
        );
    seal(&mut contracts);
    graph.edges.push(CapabilityGraphEdge {
        from_node_id: forward.producer_node.clone(),
        to_node_id: forward.consumer_node.clone(),
        dependency_id: "source".into(),
        origin: "test".into(),
        required: true,
    });
    request.bindings.push(PortBinding {
        producer_node: forward.consumer_node,
        output_port: "value".into(),
        consumer_node: forward.producer_node,
        input_port: "source".into(),
    });
    assert!(has(
        &compile_data_plan(&graph, request, contracts).unwrap_err(),
        PlanDiagnosticCode::Cycle
    ));
}
#[test]
fn data_pipeline_versions_and_unknown_fields_fail_closed() {
    let (_, request, contracts) = fixture();
    let mut value = serde_json::to_value(&contracts[0].contract).unwrap();
    value["schemaVersion"] = json!("tradeassembly.operation_data.v99");
    assert!(serde_json::from_value::<OperationDataContract>(value).is_err());
    let mut value = serde_json::to_value(request).unwrap();
    value["autoFallback"] = json!(true);
    assert!(serde_json::from_value::<PipelineRequest>(value).is_err());
    assert_eq!(tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION, "1");
}

#[test]
fn data_pipeline_rejects_implicit_resampling_and_external_parameter_schemas() {
    let (graph, mut request, mut contracts) = fixture();
    let source = request.bindings[0].producer_node.clone();
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == source)
        .unwrap()
        .context
        .interval_ms = Some(3_600_000);
    contracts
        .iter_mut()
        .find(|c| c.contract.operation_id == "provide")
        .unwrap()
        .contract
        .outputs
        .get_mut("value")
        .unwrap()
        .time
        .interval_ms = Some(3_600_000);
    seal(&mut contracts);
    assert!(has(
        &compile_data_plan(&graph, request, contracts).unwrap_err(),
        PlanDiagnosticCode::IncompatibleTimeContext
    ));
    let (graph, request, mut contracts) = fixture();
    contracts[0].contract.parameter_schema = json!({"$ref":"https://invalid.example/schema.json"});
    seal(&mut contracts);
    assert!(has(
        &compile_data_plan(&graph, request, contracts).unwrap_err(),
        PlanDiagnosticCode::InvalidContract
    ));
}

#[test]
fn data_pipeline_state_identity_is_pinned_and_history_overflow_fails_closed() {
    let (graph, request, mut contracts) = fixture();
    let original = compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap();
    contracts[0].contract.state = Some(OperationStateContract {
        schema: SchemaIdentity {
            schema_ref: "schema://test/state@1".into(),
            sha256: "c".repeat(64),
        },
        version: "1".into(),
        partition: StatePartition::Instrument,
    });
    seal(&mut contracts);
    assert_ne!(
        original.plan_sha256,
        compile_data_plan(&graph, request.clone(), contracts.clone())
            .unwrap()
            .plan_sha256
    );
    contracts
        .iter_mut()
        .find(|c| c.contract.operation_id == "provide")
        .unwrap()
        .contract
        .warmup
        .minimum_observations = MAX_EXACT_INTEGER;
    seal(&mut contracts);
    assert!(has(
        &compile_data_plan(&graph, request, contracts).unwrap_err(),
        PlanDiagnosticCode::WarmupOverflow
    ));
}

#[test]
fn data_pipeline_rejects_numbers_outside_exact_supported_range() {
    assert_ne!(
        canonical_sha256(&json!(9_007_199_254_740_990_u64)).unwrap(),
        canonical_sha256(&json!(9_007_199_254_740_991_u64)).unwrap()
    );
    assert!(canonical_sha256(&json!({"nested":[-9_007_199_254_740_992_i64]})).is_err());
    for window in [9_007_199_254_740_992_u64, 9_007_199_254_740_993_u64] {
        let (graph, mut request, contracts) = fixture();
        request
            .nodes
            .iter_mut()
            .find(|n| n.parameters.get("window").is_some())
            .unwrap()
            .parameters = json!({"window":window});
        assert!(has(
            &compile_data_plan(&graph, request, contracts).unwrap_err(),
            PlanDiagnosticCode::InvalidParameters
        ));
        assert!(canonical_sha256(&json!({"window":window})).is_err());
    }
}

#[test]
fn data_pipeline_normalizes_schema_integer_float_before_validation_warmup_and_hash() {
    let (graph, request, contracts) = fixture();
    let integer = compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap();
    let mut float = request;
    float
        .nodes
        .iter_mut()
        .find(|n| n.parameters.get("window").is_some())
        .unwrap()
        .parameters = json!({"window":5.0});
    let normalized = compile_data_plan(&graph, float, contracts).unwrap();
    assert_eq!(integer, normalized);
}

#[test]
fn data_pipeline_strategy_edges_cannot_impersonate_operation_dependencies() {
    let (mut graph, request, contracts) = fixture();
    let original = graph.clone();
    let consumer = request.bindings[0].consumer_node.clone();
    graph
        .nodes
        .iter_mut()
        .find(|n| n.node_id == consumer)
        .unwrap()
        .selected
        .as_mut()
        .unwrap()
        .operation
        .dependencies
        .clear();
    assert!(has(
        &compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap_err(),
        PlanDiagnosticCode::UndeclaredDependency
    ));
    // A matching operation declaration is insufficient when the edge has a
    // different meaning. Check this independently of the missing-declaration case.
    let mut graph = original;
    graph.edges[0].origin = "strategy_dependency".into();
    assert!(has(
        &compile_data_plan(&graph, request, contracts).unwrap_err(),
        PlanDiagnosticCode::UndeclaredDependency
    ));
}

#[test]
fn data_pipeline_rejects_hourly_aggregation_before_daily_window() {
    let (mut graph, mut request, mut contracts) = fixture();
    let aggregate_id = request.bindings[0].producer_node.clone();
    let raw_id = format!("{aggregate_id}::raw");
    let mut raw_node = graph
        .nodes
        .iter()
        .find(|n| n.node_id == aggregate_id)
        .unwrap()
        .clone();
    raw_node.node_id = raw_id.clone();
    raw_node.parent_node_id = Some(aggregate_id.clone());
    raw_node.dependency_id = Some("raw".into());
    raw_node.requirement.capability = "test.raw".into();
    let selected = raw_node.selected.as_mut().unwrap();
    selected.operation_id = "raw".into();
    selected.operation = operation("raw");
    let raw_manifest = selected.manifest_fingerprint.clone();
    graph.nodes.push(raw_node);
    graph
        .nodes
        .iter_mut()
        .find(|n| n.node_id == aggregate_id)
        .unwrap()
        .selected
        .as_mut()
        .unwrap()
        .operation
        .dependencies
        .push(PluginOperationDependency {
            dependency_id: "raw".into(),
            capability: "test.raw".into(),
            operation_id: Some("raw".into()),
            required: true,
            traits: Default::default(),
        });
    graph.edges.push(CapabilityGraphEdge {
        from_node_id: aggregate_id.clone(),
        to_node_id: raw_id.clone(),
        dependency_id: "raw".into(),
        origin: "plugin_dependency".into(),
        required: true,
    });
    let mut hourly = context();
    hourly.interval_ms = Some(3_600_000);
    let aggregate = contracts
        .iter_mut()
        .find(|c| c.graph_node_id == aggregate_id)
        .unwrap();
    let mut input = port();
    input.time.interval_ms = Some(3_600_000);
    aggregate.contract.inputs.insert(
        "raw".into(),
        InputPort {
            data: input,
            required: true,
            dependency_id: "raw".into(),
        },
    );
    aggregate.contract.warmup.minimum_observations = 24;
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == aggregate_id)
        .unwrap()
        .input_contexts
        .insert("raw".into(), hourly.clone());
    let mut raw_contract = contract("raw", false);
    raw_contract
        .outputs
        .get_mut("value")
        .unwrap()
        .time
        .interval_ms = Some(3_600_000);
    contracts.push(VerifiedDataContract {
        graph_node_id: raw_id.clone(),
        manifest_fingerprint: raw_manifest,
        contract_sha256: String::new(),
        contract: raw_contract.into(),
    });
    seal(&mut contracts);
    request.nodes.push(PipelineNode {
        graph_node_id: raw_id.clone(),
        parameters: json!({}),
        context: hourly,
        input_contexts: Default::default(),
    });
    request.bindings.push(PortBinding {
        producer_node: raw_id,
        output_port: "value".into(),
        consumer_node: aggregate_id,
        input_port: "raw".into(),
    });
    assert!(has(
        &compile_data_plan(&graph, request, contracts).unwrap_err(),
        PlanDiagnosticCode::IncompatibleTimeContext
    ));
}

#[test]
fn data_pipeline_v2_composes_repeated_operations_and_shared_outputs() {
    let (graph, request, contracts) = make_fixture(true, 3);
    assert!(graph.edges.is_empty());
    let plan = compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap();
    assert_eq!(plan.schema_version, DataPlanVersion::V2);
    assert_eq!(plan.source_warmup_observations["result"], 5);
    assert_eq!(plan.source_warmup_observations["result_1"], 5);
    assert_eq!(plan.source_warmup_observations["result_2"], 9);
    assert_eq!(plan.execution_order.len(), 4);
    let mut bad = contracts.clone();
    bad.iter_mut()
        .find(|c| c.graph_node_id == "source")
        .unwrap()
        .contract
        .outputs
        .get_mut("value")
        .unwrap()
        .schema
        .sha256 = "b".repeat(64);
    seal(&mut bad);
    assert!(has(
        &compile_data_plan(&graph, request.clone(), bad).unwrap_err(),
        PlanDiagnosticCode::IncompatibleSchema
    ));
    let mut cyclic = contracts;
    cyclic
        .iter_mut()
        .find(|c| c.graph_node_id == "source")
        .unwrap()
        .contract
        .inputs
        .insert(
            "feedback".into(),
            InputPort {
                data: port(),
                required: true,
                dependency_id: "feedback".into(),
            },
        );
    seal(&mut cyclic);
    let mut request = request;
    request.bindings.push(PortBinding {
        producer_node: "result_2".into(),
        output_port: "value".into(),
        consumer_node: "source".into(),
        input_port: "feedback".into(),
    });
    assert!(has(
        &compile_data_plan(&graph, request, cyclic).unwrap_err(),
        PlanDiagnosticCode::Cycle
    ));
}

#[test]
fn data_pipeline_v3_composes_only_the_selected_output_history() {
    use tradeassembly_plugin_sdk::data_history::*;
    let (graph, mut request, mut contracts) = make_fixture(true, 2);
    let first = contracts
        .iter_mut()
        .find(|c| c.graph_node_id == "result")
        .unwrap();
    let mut base = first.contract.clone().into_legacy().unwrap();
    let port = base.outputs.remove("value").unwrap();
    base.outputs = BTreeMap::from([("early".into(), port.clone()), ("late".into(), port)]);
    base.parameter_schema["properties"]["tail"] = json!({"type":"integer","minimum":1});
    base.parameter_schema["required"] = json!(["window", "tail"]);
    let param = || Observations::Parameter {
        name: "window".into(),
    };
    let late = Observations::Subtract {
        left: Box::new(Observations::Add {
            terms: vec![
                param(),
                Observations::Parameter {
                    name: "tail".into(),
                },
            ],
        }),
        right: Box::new(Observations::Literal { value: 1 }),
    };
    first.contract = VersionedOperationData::V3(OperationDataV3 {
        schema_version: HistoryVersion::V3,
        operation: base,
        history: HistoryContract {
            first_ready: BTreeMap::from([
                ("early".into(), BTreeMap::from([("source".into(), param())])),
                ("late".into(), BTreeMap::from([("source".into(), late)])),
            ]),
            partitioning: Partitioning::InstrumentIndependent,
            required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
            reconstruction: Reconstruction::CheckpointOrExplicitOrigin,
        },
    });
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "result")
        .unwrap()
        .parameters = json!({"window":5,"tail":3});
    let consumer = request
        .bindings
        .iter_mut()
        .find(|b| b.consumer_node == "result_1")
        .unwrap();
    consumer.producer_node = "result".into();
    consumer.output_port = "early".into();
    seal(&mut contracts);
    let early = compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap();
    assert_eq!(early.schema_version, DataPlanVersion::V3);
    assert!(serde_json::to_value(&request)
        .unwrap()
        .get("selectedOutputs")
        .is_none());
    let mut explicit = request.clone();
    explicit.selected_outputs = vec![PipelineOutput {
        node: "result".into(),
        port: "early".into(),
    }];
    let selected_early = compile_data_plan(&graph, explicit.clone(), contracts.clone()).unwrap();
    assert_eq!(selected_early.schema_version, DataPlanVersion::V5);
    assert_eq!(selected_early.selected_source_history["source"], 5);
    explicit.selected_outputs[0].port = "late".into();
    let selected_late = compile_data_plan(&graph, explicit.clone(), contracts.clone()).unwrap();
    assert_eq!(selected_late.selected_source_history["source"], 7);
    assert_ne!(selected_early.plan_sha256, selected_late.plan_sha256);
    explicit
        .selected_outputs
        .push(explicit.selected_outputs[0].clone());
    assert!(has(
        &compile_data_plan(&graph, explicit, contracts.clone()).unwrap_err(),
        PlanDiagnosticCode::UnknownPort
    ));

    assert_eq!(early.source_warmup_observations["result"], 7);
    assert_eq!(early.source_warmup_observations["result_1"], 9);
    assert_eq!(
        early.source_history_by_output["result"]["early"]["source"],
        5
    );
    assert_eq!(
        early.source_history_by_output["result"]["late"]["source"],
        7
    );
    request
        .bindings
        .iter_mut()
        .find(|b| b.consumer_node == "result_1")
        .unwrap()
        .output_port = "late".into();
    let late = compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap();
    assert_eq!(late.source_warmup_observations["result_1"], 11);
    assert_ne!(early.plan_sha256, late.plan_sha256);
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "result")
        .unwrap()
        .parameters["tail"] = json!(-1);
    let errors = compile_data_plan(&graph, request, contracts).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| e.node_id.as_deref() == Some("result") && e.port.as_deref() == Some("late")));
}

#[test]
fn data_pipeline_v3_stateful_source_is_rejected_during_preparation() {
    use tradeassembly_plugin_sdk::data_history::*;
    let (graph, request, mut contracts) = make_fixture(true, 1);
    let source = contracts
        .iter_mut()
        .find(|c| c.graph_node_id == "source")
        .unwrap();
    let mut operation = source.contract.clone().into_legacy().unwrap();
    operation.schema_version = DataContractVersion::V2;
    operation.state = Some(OperationStateContract {
        schema: operation.outputs["value"].schema.clone(),
        version: "1".into(),
        partition: StatePartition::Instrument,
    });
    source.contract = VersionedOperationData::V3(OperationDataV3 {
        schema_version: HistoryVersion::V3,
        operation,
        history: HistoryContract {
            first_ready: BTreeMap::from([("value".into(), BTreeMap::new())]),
            partitioning: Partitioning::InstrumentIndependent,
            required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
            reconstruction: Reconstruction::CheckpointOrExplicitOrigin,
        },
    });
    seal(&mut contracts);
    let errors = compile_data_plan(&graph, request, contracts).unwrap_err();
    assert!(errors
        .iter()
        .any(|e| e.code == PlanDiagnosticCode::InvalidContract
            && e.message.contains("Stateful V3 source acquisition")));
}

#[test]
fn data_pipeline_v3_zero_parameter_can_resolve_to_positive_history() {
    use tradeassembly_plugin_sdk::data_history::*;
    let (graph, mut request, mut contracts) = make_fixture(true, 1);
    let binding = contracts
        .iter_mut()
        .find(|c| c.graph_node_id == "result")
        .unwrap();
    let mut operation = binding.contract.clone().into_legacy().unwrap();
    operation.schema_version = DataContractVersion::V2;
    operation.parameter_schema = json!({"type":"object","properties":{"period":{"type":"integer","minimum":0}},"required":["period"],"additionalProperties":false});
    operation.warmup = WarmupContract {
        minimum_observations: 1,
        observations_parameter: None,
        additional_observations: 0,
    };
    binding.contract = VersionedOperationData::V3(OperationDataV3 {
        schema_version: HistoryVersion::V3,
        operation,
        history: HistoryContract {
            first_ready: BTreeMap::from([(
                "value".into(),
                BTreeMap::from([(
                    "source".into(),
                    Observations::Add {
                        terms: vec![
                            Observations::Parameter {
                                name: "period".into(),
                            },
                            Observations::Literal { value: 1 },
                        ],
                    },
                )]),
            )]),
            partitioning: Partitioning::InstrumentIndependent,
            required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
            reconstruction: Reconstruction::FiniteWindow,
        },
    });
    seal(&mut contracts);
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "result")
        .unwrap()
        .parameters = json!({"period":0});
    let zero = compile_data_plan(&graph, request.clone(), contracts.clone()).unwrap();
    assert_eq!(zero.source_warmup_observations["result"], 1);
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "result")
        .unwrap()
        .parameters = json!({"period":0.0});
    assert_eq!(
        compile_data_plan(&graph, request.clone(), contracts.clone())
            .unwrap()
            .plan_sha256,
        zero.plan_sha256
    );
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "result")
        .unwrap()
        .parameters = json!({"period":2});
    assert_eq!(
        compile_data_plan(&graph, request.clone(), contracts.clone())
            .unwrap()
            .source_warmup_observations["result"],
        3
    );
    request
        .nodes
        .iter_mut()
        .find(|n| n.graph_node_id == "result")
        .unwrap()
        .parameters = json!({"period":-1});
    assert!(compile_data_plan(&graph, request, contracts).is_err());
}
