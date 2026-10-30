// Opt-in integration against an independently built data calculation package.
// NativeTestSandbox is a process fixture, not production sandbox qualification.
use super::*;
use crate::{
    data_pipeline::canonical_sha256,
    ports::{AuthorityContext, IdempotencyKey, PluginPackageInstallRequest},
};
use tradeassembly_plugin_sdk::market_data::{
    bar_batch, ohlcv_identity, ohlcv_series_schema, OhlcvBar, ProviderBar, OHLCV_SERIES_SCHEMA,
};
use tradeassembly_plugin_sdk::{data_contract::*, data_evaluation::*};

#[test]
#[ignore = "requires DATA_CALCULATION_TEST_PACKAGE built for this host"]
fn real_data_calculation_package_installs_prepares_and_executes_without_credentials() {
    let path = std::path::PathBuf::from(
        std::env::var_os("DATA_CALCULATION_TEST_PACKAGE").expect("built data package required"),
    );
    let archive = fs::read(&path).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = dir
        .path()
        .join("data-host.db")
        .to_string_lossy()
        .into_owned();
    let runtime = crate::adapters::local::test_runtime(&db);
    // Manifest bytes are read from the supplied local archive, then verified by
    // the actual installer against the supplied archive and canonical manifest.
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()));
    let manifest: Value = tar
        .entries()
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().unwrap().as_ref() == std::path::Path::new("manifest.json"))
        .map(|mut entry| {
            let mut bytes = vec![];
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            serde_json::from_slice(&bytes).unwrap()
        })
        .expect("manifest required");
    let package = runtime
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: format!("{:x}", Sha256::digest(&archive)),
            manifest_sha256: canonical_sha256(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    assert!(runtime
        .plugin_packages
        .data_contract(&package.package_sha256, "indicator.ema")
        .unwrap()
        .is_some());
    let ctx = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("calculation-integration").unwrap(),
    );
    runtime.plugins.install_manifest(&package.plugin_ref,json!({"pluginRef":package.plugin_ref,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx).unwrap();
    runtime.plugins.put_instance("calculation",json!({"instanceRef":"calculation","pluginRef":package.plugin_ref,"enabled":true,"configuration":{},"activePackageSha256":package.package_sha256}),&ctx).unwrap();
    let schema = SchemaIdentity {
        schema_ref: "schema://tradeassembly/number-series@1".into(),
        sha256: canonical_sha256(&json!({"type":"array","items":{"type":"number"}})).unwrap(),
    };
    let host = ExternalPluginHost::new(
        runtime.plugins.clone(),
        runtime.credentials.clone(),
        runtime.plugin_packages.clone(),
        runtime.clock.clone(),
        Arc::new(NativeTestSandbox),
    );
    let mut states = std::collections::BTreeMap::new();
    let mut results = vec![];
    for (index, value) in [1, 4, 2, 8].into_iter().enumerate() {
        let start = index as i64 * 86_400_000;
        let mut point = DataPoint {
            instrument_id: "BTC/USD".into(),
            source_event_id: format!("fixture:{index}"),
            provenance: BatchProvenance {
                content_sha256: String::new(),
                upstream_content_sha256: vec![],
                event_start_ms: start,
                event_end_ms: start + 86_400_000,
                known_at_ms: start + 86_400_000,
                arrived_at_ms: start + 86_400_000,
                completed: true,
                source_revision: "fixture.1".into(),
            },
            result: PointValue::Ready(json!(value)),
        };
        point.provenance.content_sha256 = point_sha256(&point).unwrap();
        let evaluation = DataEvaluationRequest {
            schema_version: EvaluationVersion::V1,
            plan_sha256: "test-plan".into(),
            node_id: "ema".into(),
            semantics_version: "tradeassembly.ema.sma-seed.v1".into(),
            instruments: vec!["BTC/USD".into()],
            window: EvaluationWindow {
                event_start_ms: start,
                event_end_ms: start + 86_400_000,
                as_of_ms: start + 86_400_000,
            },
            parameters: json!({"period":3}),
            inputs: std::collections::BTreeMap::from([(
                "source".into(),
                DataBatch {
                    schema: schema.clone(),
                    shape: DataShape::Series,
                    points: vec![point],
                },
            )]),
            prior_state: states,
            max_points: 1,
        };
        let mut request = operation_request();
        request.plugin_instance_ref = "calculation".into();
        request.plugin_ref = package.plugin_ref.clone();
        request.manifest_fingerprint = package.manifest_sha256.clone();
        request.operation_id = "indicator.ema".into();
        request.capability = "indicator.ema".into();
        request.account_ref = None;
        request.purpose = "data_calculation".into();
        request.input = serde_json::to_value(evaluation).unwrap();
        request.evaluation_tick_id = format!("frame-{index}");
        let mut context = ctx.clone();
        context.idempotency_key =
            IdempotencyKey::new(format!("calculation-frame-{index}")).unwrap();
        let result = host.invoke(&request, &context).unwrap();
        assert!(result.deterministic && result.replayable);
        let decoded: DataEvaluationResponse = serde_json::from_value(result.payload).unwrap();
        results.push(decoded.outputs["value"].points[0].result.clone());
        states = decoded.state;
    }
    assert_eq!(results[..2], [PointValue::WarmingUp, PointValue::WarmingUp]);
    let PointValue::Ready(value) = &results[3] else {
        panic!("EMA not ready")
    };
    assert!((value.as_f64().unwrap() - 31.0 / 6.0).abs() < 1e-12);
    assert!(runtime
        .plugins
        .get_instance("calculation")
        .unwrap()
        .unwrap()
        .get("credentialRef")
        .is_none());
}

#[derive(Default)]
struct FixtureSourceAndRealCalculations {
    calculations:
        std::sync::Mutex<Option<crate::adapters::plugin_operations::LocalPluginOperations>>,
}
impl VersionedPort for FixtureSourceAndRealCalculations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![]
    }
}
impl crate::ports::PluginOperationPort for FixtureSourceAndRealCalculations {
    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        if request.operation_id != "provide.ohlcv" {
            assert!(matches!(
                request.operation_id.as_str(),
                "indicator.ema" | "data.ohlcv_field"
            ));
            return crate::ports::PluginOperationPort::invoke(
                self.calculations.lock().unwrap().as_ref().unwrap(),
                request,
                context,
            );
        }
        // Explicit synthetic acquisition fixture; never represented as provider data.
        let payload: DataEvaluationRequest = serde_json::from_value(request.input.clone()).unwrap();
        let mut rows = vec![];
        let day = 86_400_000;
        let mut start = payload.window.event_start_ms;
        while start < payload.window.event_end_ms {
            let index = (start / day - 10) as usize;
            let value = [1, 4, 2, 8][index];
            rows.push(ProviderBar {
                instrument_id: "BTC/USD".into(),
                source_event_id: format!("synthetic:{index}"),
                provenance: BatchProvenance {
                    content_sha256: String::new(),
                    upstream_content_sha256: vec![],
                    event_start_ms: start,
                    event_end_ms: start + day,
                    known_at_ms: start + day,
                    arrived_at_ms: payload.window.as_of_ms,
                    completed: true,
                    source_revision: "synthetic.1".into(),
                },
                value: OhlcvBar {
                    open: value as f64,
                    high: value as f64 + 1.0,
                    low: value as f64 - 1.0,
                    close: value as f64,
                    volume: 1.0,
                    vwap: None,
                    trade_count: None,
                    session: None,
                },
            });
            start += day;
        }
        let response = DataEvaluationResponse {
            schema_version: EvaluationVersion::V1,
            outputs: std::collections::BTreeMap::from([(
                "value".into(),
                bar_batch(&payload, day as u64, rows)?,
            )]),
            state: Default::default(),
        };
        Ok(PluginOperationResponse {
            correlation_id: request.correlation_id.clone(),
            schema_ref: EVALUATION_RESPONSE_SCHEMA.into(),
            payload: serde_json::to_value(response).unwrap(),
            observed_at_ms: payload.window.as_of_ms,
            source_event_id: "synthetic-source".into(),
            content_hash: "synthetic-source".into(),
            freshness_state: "fresh".into(),
            evidence_refs: vec![],
            deterministic: true,
            replayable: true,
            provider_outcome_id: None,
            reconciliation_required: false,
        })
    }
}

#[test]
#[ignore = "requires DATA_CALCULATION_TEST_PACKAGE built for this host"]
fn saved_plan_evaluator_composes_real_repeated_operations_with_batch_incremental_parity() {
    use crate::{
        data_pipeline::*,
        ports::*,
        service::{data_evaluation as host, data_plans},
    };
    let mixed = Arc::new(FixtureSourceAndRealCalculations::default());
    let (_dir, service, scope, _) = data_plans::tests::fixture_service(Some(mixed.clone()));
    let runtime = service.runtime();
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("pipeline-process-setup").unwrap(),
    );
    let path = std::path::PathBuf::from(std::env::var_os("DATA_CALCULATION_TEST_PACKAGE").unwrap());
    let archive = fs::read(&path).unwrap();
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive.as_slice()));
    let manifest: Value = tar
        .entries()
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().unwrap().as_ref() == std::path::Path::new("manifest.json"))
        .map(|mut entry| {
            let mut b = vec![];
            std::io::Read::read_to_end(&mut entry, &mut b).unwrap();
            serde_json::from_slice(&b).unwrap()
        })
        .unwrap();
    let calculation = runtime
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: path.to_string_lossy().into(),
            package_sha256: format!("{:x}", Sha256::digest(&archive)),
            manifest_sha256: canonical_sha256(&manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    let register = |plugin: &str,
                    instance: &str,
                    manifest: Value,
                    package: &InstalledPluginPackage| {
        runtime.plugins.install_manifest(plugin,json!({"pluginRef":plugin,"manifest":manifest,"manifestDigest":package.manifest_sha256,"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&context).unwrap();
        runtime.plugins.put_instance(instance,json!({"instanceRef":instance,"pluginRef":plugin,"providerRef":instance,"enabled":true,"activePackageSha256":package.package_sha256,"configuration":{}}),&context).unwrap();
    };
    register(
        &calculation.plugin_ref,
        "default-calculations",
        manifest,
        &calculation,
    );
    let (mut descriptor, contract_bytes, _, _) =
        crate::adapters::plugin_packages::data_package_tests::materials();
    let schema_bytes = serde_json::to_vec(&ohlcv_series_schema()).unwrap();
    let mut contract: Value = serde_json::from_slice(&contract_bytes).unwrap();
    contract["operationId"] = json!("provide.ohlcv");
    contract["outputs"]["value"]["schema"] = serde_json::to_value(ohlcv_identity()).unwrap();
    let template = runtime
        .plugins
        .get_manifest("example.provide")
        .unwrap()
        .unwrap();
    let mut operation = template["manifest"]["operations"][0].clone();
    operation["id"] = json!("provide.ohlcv");
    operation["capability"] = json!("test.ohlcv-source");
    operation["traits"]["instrumentFamilies"] = json!(["crypto_asset"]);
    operation["traits"]["outputSchemaRefs"] =
        json!([OHLCV_SERIES_SCHEMA, EVALUATION_RESPONSE_SCHEMA]);
    let source_manifest = json!({"metadata":{"id":"example.synthetic-source","version":"1","provider":{"id":"synthetic","name":"Synthetic fixture"}},"capabilities":[{"id":"test.ohlcv-source"}],"operations":[operation],"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
    let source_bytes = serde_json::to_vec(&source_manifest).unwrap();
    let contract_bytes = serde_json::to_vec(&contract).unwrap();
    descriptor["package"]["plugin"]["id"] = json!("example.synthetic-source");
    descriptor["package"]["manifest"]["sha256"] =
        json!(format!("{:x}", Sha256::digest(&source_bytes)));
    descriptor["dataContracts"][0]["operationId"] = json!("provide.ohlcv");
    descriptor["dataContracts"][0]["document"]["sha256"] =
        json!(format!("{:x}", Sha256::digest(&contract_bytes)));
    descriptor["dataContracts"][0]["canonicalSha256"] = json!(canonical_sha256(&contract).unwrap());
    descriptor["dataSchemas"][0]["schemaRef"] = json!(OHLCV_SERIES_SCHEMA);
    descriptor["dataSchemas"][0]["document"]["sha256"] =
        json!(format!("{:x}", Sha256::digest(&schema_bytes)));
    descriptor["dataSchemas"][0]["canonicalSha256"] = json!(ohlcv_identity().sha256);
    let source_archive = crate::adapters::plugin_packages::data_package_tests::archive_files(vec![
        (
            "tradeassembly-plugin.json",
            serde_json::to_vec(&descriptor).unwrap(),
        ),
        ("manifest.json", source_bytes),
        ("contract.json", contract_bytes),
        ("schema.json", schema_bytes),
        ("bin/plugin", b"not-executed".to_vec()),
    ]);
    let source_path = _dir.path().join("synthetic-source.tar.gz");
    fs::write(&source_path, &source_archive).unwrap();
    let source = runtime
        .plugin_packages
        .install(&PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: source_path.to_string_lossy().into(),
            package_sha256: format!("{:x}", Sha256::digest(&source_archive)),
            manifest_sha256: canonical_sha256(&source_manifest).unwrap(),
            offline: true,
        })
        .unwrap();
    register(
        &source.plugin_ref,
        "synthetic-source",
        source_manifest,
        &source,
    );
    for capability in ["test.ohlcv-source", "indicator.ema", "data.ohlcv_field"] {
        let template = runtime
            .storage
            .get_json("entitlement_grants", "test.provide")
            .unwrap()
            .unwrap();
        let mut grant: EntitlementGrant = serde_json::from_value(template).unwrap();
        grant.grant_id = format!("test.{capability}");
        grant.capability = capability.into();
        runtime
            .storage
            .put_json(
                "entitlement_grants",
                &grant.grant_id,
                serde_json::to_value(&grant).unwrap(),
                &context,
            )
            .unwrap();
    }
    *mixed.calculations.lock().unwrap() = Some(
        crate::adapters::plugin_operations::LocalPluginOperations::with_external_host(
            runtime.storage.clone(),
            runtime.plugins.clone(),
            runtime.credentials.clone(),
            runtime.plugin_packages.clone(),
            runtime.clock.clone(),
            Arc::new(NativeTestSandbox),
        ),
    );
    let saved_revision=service.handle_http("POST", "/capability-graph-revisions",json!({"configId":"cfg_process_proof","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":"paper","additionalRequirements":[{"requirementId":"source","capability":"test.ohlcv-source","mode":"paper","declaration":"required","fallbackPolicy":"none"},{"requirementId":"close","capability":"data.ohlcv_field","mode":"paper","declaration":"required","fallbackPolicy":"none"},{"requirementId":"ema1","capability":"indicator.ema","mode":"paper","declaration":"required","fallbackPolicy":"none"},{"requirementId":"ema2","capability":"indicator.ema","mode":"paper","declaration":"required","fallbackPolicy":"none"}]}));
    assert_eq!(saved_revision.status, 200, "{}", saved_revision.body);
    let revision = &saved_revision.body;
    assert_eq!(revision["revision"]["graph"]["ok"], true, "{revision:#}");
    let data_context = DataContext {
        instruments: vec![InstrumentContext {
            instrument_id: "BTC/USD".into(),
            asset_class: "crypto".into(),
            instrument_family: "crypto_asset".into(),
        }],
        interval_ms: Some(86_400_000),
    };
    let pipeline = PipelineRequest {
        capability_graph_revision: revision["revision"]["revisionId"].as_str().unwrap().into(),
        mode: "paper".into(),
        trade_instruments: vec![],
        nodes: [
            ("source", json!({})),
            ("close", json!({"field":"close"})),
            ("ema1", json!({"period":3})),
            ("ema2", json!({"period":2})),
        ]
        .into_iter()
        .map(|(id, parameters)| PipelineNode {
            graph_node_id: id.into(),
            parameters,
            context: data_context.clone(),
            input_contexts: Default::default(),
        })
        .collect(),
        bindings: vec![
            PortBinding {
                producer_node: "source".into(),
                output_port: "value".into(),
                consumer_node: "close".into(),
                input_port: "source".into(),
            },
            PortBinding {
                producer_node: "close".into(),
                output_port: "value".into(),
                consumer_node: "ema1".into(),
                input_port: "source".into(),
            },
            PortBinding {
                producer_node: "ema1".into(),
                output_port: "value".into(),
                consumer_node: "ema2".into(),
                input_port: "source".into(),
            },
        ],
    };
    let plan = data_plans::save(&service, scope, pipeline, &context).unwrap();
    assert_eq!(plan.content.plan.source_warmup_observations["ema2"], 4);
    let step = |run: &str, index: i64, end: i64, prior: Option<String>, key: &str| {
        let mut ctx = context.clone();
        ctx.idempotency_key = IdempotencyKey::new(key).unwrap();
        host::step(
            &service,
            host::EvaluationStep {
                run_id: run.into(),
                plan_revision_id: plan.revision_id.clone(),
                window: EvaluationWindow {
                    event_start_ms: index * 86_400_000,
                    event_end_ms: end * 86_400_000,
                    as_of_ms: end * 86_400_000,
                },
                expected_checkpoint_sha256: prior,
            },
            &ctx,
        )
        .unwrap()
    };
    let batch_first = step("batch", 10, 12, None, "batch-a");
    assert_eq!(batch_first.status, host::EvaluationStatus::WarmingUp);
    let batch = step(
        "batch",
        12,
        14,
        Some(batch_first.checkpoint.checkpoint_sha256.clone()),
        "batch-b",
    );
    assert_eq!(batch.status, host::EvaluationStatus::Ready);
    let mut streamed = vec![];
    let mut prior = None;
    let mut last = None;
    for index in 10..14 {
        let receipt = step(
            "stream",
            index,
            index + 1,
            prior,
            &format!("stream-{index}"),
        );
        streamed.extend(
            receipt.outputs["ema2"]
                .iter()
                .flat_map(|r| r.outputs["value"].points.clone()),
        );
        prior = Some(receipt.checkpoint.checkpoint_sha256.clone());
        last = Some(receipt);
    }
    let batch_points = batch_first.outputs["ema2"]
        .iter()
        .chain(batch.outputs["ema2"].iter())
        .flat_map(|r| r.outputs["value"].points.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        batch_points
            .iter()
            .map(|p| (&p.result, &p.provenance.content_sha256))
            .collect::<Vec<_>>(),
        streamed
            .iter()
            .map(|p| (&p.result, &p.provenance.content_sha256))
            .collect::<Vec<_>>()
    );
    assert_eq!(batch.checkpoint.states, last.unwrap().checkpoint.states);
    let PointValue::Ready(value) = &batch_points[3].result else {
        panic!("composite not ready")
    };
    assert!((value.as_f64().unwrap() - 3.75).abs() < 1e-12);
    assert!(runtime
        .storage
        .list_json("plugin_broker_dispatch_claims")
        .unwrap()
        .is_empty());
    assert!(!runtime
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap()
        .is_empty());
}
