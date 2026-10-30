use super::*;
use crate::{
    adapters::local::sqlite::LocalSqliteStorage,
    ports::{AuthorityContext, IdempotencyKey},
};
fn content() -> DataPlanContent {
    let (graph, request, contracts) = crate::data_pipeline::tests::fixture();
    DataPlanContent {
        recipe_source: None,
        source_authentication: BTreeMap::new(),
        scope: PlanScope {
            strategy_id: "test-strategy".into(),
            strategy_version_id: "v1".into(),
            strategy_spec_hash: "test-hash".into(),
        },
        plan: compile_data_plan(&graph, request, contracts).unwrap(),
        package_sha256: BTreeMap::new(),
        binding_sha256: BTreeMap::new(),
    }
}
fn context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).unwrap(),
    )
}
#[test]
fn data_plan_persistence_is_immutable_idempotent_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("plans.db");
    let path = db.to_str().unwrap();
    let storage = LocalSqliteStorage::new(path);
    let input = content();
    let saved = persist(&storage, input.clone(), &context("same")).unwrap();
    let restarted = LocalSqliteStorage::new(path);
    assert_eq!(
        saved,
        persist(&restarted, input.clone(), &context("same")).unwrap()
    );
    assert_eq!(
        saved,
        persist(&restarted, input, &context("other-key")).unwrap()
    );
    assert_eq!(restarted.list_json(PLANS).unwrap().len(), 1);
    let mut changed = content();
    changed
        .package_sha256
        .insert("changed".into(), "a".repeat(64));
    assert_eq!(
        persist(&restarted, changed, &context("same")).unwrap_err(),
        "data_plan_idempotency_conflict"
    );
    assert_eq!(restarted.list_json(PLANS).unwrap().len(), 1);
}
#[test]
fn data_plan_rejects_tampering_and_partial_records() {
    let dir = tempfile::tempdir().unwrap();
    let storage = LocalSqliteStorage::new(dir.path().join("plans.db").to_string_lossy());
    let mut changed = content();
    changed.plan.plan_sha256 = "f".repeat(64);
    assert!(persist(&storage, changed, &context("bad")).is_err());
    assert!(storage.list_json(PLANS).unwrap().is_empty());
    assert!(storage.list_json(KEYS).unwrap().is_empty());
    let saved = persist(&storage, content(), &context("good")).unwrap();
    storage.clear_namespace(PLANS).unwrap();
    assert_eq!(
        persist(&storage, saved.content, &context("good")).unwrap_err(),
        "data_plan_partial_commit"
    );
}
#[test]
fn data_plan_freshness_rejects_missing_graph_before_any_storage_write() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("service.db").to_string_lossy().into_owned();
    let service =
        TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db));
    let input = content();
    assert!(save(
        &service,
        input.scope,
        input.plan.request,
        &context("not-admitted")
    )
    .is_err());
    assert!(service
        .runtime()
        .storage
        .list_json(PLANS)
        .unwrap()
        .is_empty());
    assert!(service
        .runtime()
        .storage
        .list_json(KEYS)
        .unwrap()
        .is_empty());
}

#[test]
fn data_plan_atomic_failure_rolls_back_both_records_and_retry_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plans.db");
    let storage = LocalSqliteStorage::new(path.to_string_lossy());
    storage.get_json(PLANS, "initialize").unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_plan BEFORE INSERT ON tradeassembly_kv WHEN NEW.namespace='compiled_data_plan_revisions_v1' BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(persist(&storage, content(), &context("retry")).is_err());
    assert!(storage.list_json(PLANS).unwrap().is_empty());
    assert!(storage.list_json(KEYS).unwrap().is_empty());
    connection.execute_batch("DROP TRIGGER fail_plan;").unwrap();
    persist(&storage, content(), &context("retry")).unwrap();
    assert_eq!(storage.list_json(PLANS).unwrap().len(), 1);
    assert_eq!(storage.list_json(KEYS).unwrap().len(), 1);
}

#[test]
fn data_plan_cached_record_cannot_bypass_current_graph_admission() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("service.db").to_string_lossy().into_owned();
    let service =
        TradeAssemblyService::from_runtime(&db, crate::adapters::local::test_runtime(&db));
    let input = content();
    let runtime = service.runtime();
    let saved = persist(runtime.storage.as_ref(), input.clone(), &context("cached")).unwrap();
    assert_eq!(
        current(&service, &saved.revision_id).unwrap_err(),
        "data_plan_graph_stale"
    );
    assert_eq!(
        save(
            &service,
            input.scope,
            input.plan.request,
            &context("cached")
        )
        .unwrap_err(),
        "data_plan_graph_stale"
    );
    assert_eq!(runtime.storage.list_json(PLANS).unwrap().len(), 1);
    assert_eq!(runtime.storage.list_json(KEYS).unwrap().len(), 1);
}

// Exercises host preparation with two independently installed fixture packages.
// Their executable bytes are inert and are never invoked by plan preparation.
pub(crate) fn fixture_service(
    plugin_operations: Option<std::sync::Arc<dyn crate::ports::PluginOperationPort>>,
) -> (
    tempfile::TempDir,
    TradeAssemblyService,
    PlanScope,
    PipelineRequest,
) {
    fixture_service_with_state(
        plugin_operations,
        tradeassembly_plugin_sdk::data_contract::StatePartition::Instrument,
        "paper",
    )
}

pub(crate) fn fixture_service_with_state(
    plugin_operations: Option<std::sync::Arc<dyn crate::ports::PluginOperationPort>>,
    state_partition: tradeassembly_plugin_sdk::data_contract::StatePartition,
    mode: &str,
) -> (
    tempfile::TempDir,
    TradeAssemblyService,
    PlanScope,
    PipelineRequest,
) {
    fixture_service_with_history(plugin_operations, state_partition, mode, false)
}

pub(crate) fn fixture_service_with_history(
    plugin_operations: Option<std::sync::Arc<dyn crate::ports::PluginOperationPort>>,
    state_partition: tradeassembly_plugin_sdk::data_contract::StatePartition,
    mode: &str,
    v3: bool,
) -> (
    tempfile::TempDir,
    TradeAssemblyService,
    PlanScope,
    PipelineRequest,
) {
    use crate::{
        adapters::plugin_packages::data_package_tests::{archive_files, materials},
        ports::*,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("service.db").to_string_lossy().into_owned();
    let mut fixture_runtime = crate::adapters::local::test_runtime(&db);
    fixture_runtime.plugin_packages = std::sync::Arc::new(
        crate::adapters::plugin_packages::LocalPluginPackageStore::new(
            dir.path().join("package-store"),
        )
        .unwrap(),
    );
    if let Some(operations) = plugin_operations {
        fixture_runtime.plugin_operations = operations;
    }
    let service = TradeAssemblyService::from_runtime(&db, fixture_runtime);
    let runtime = service.runtime();
    let ctx = context("fixture-setup");
    let (fixture_graph, _, fixture_contracts) = crate::data_pipeline::tests::fixture();
    for id in ["provide", "derive"] {
        let plugin = format!("example.{id}");
        let instance = format!("instance.{id}");
        let (mut descriptor, _, schema_bytes, _) = materials();
        let schema: Value = serde_json::from_slice(&schema_bytes).unwrap();
        let schema_hash = canonical_sha256(&schema).unwrap();
        let selected = fixture_graph
            .nodes
            .iter()
            .filter_map(|n| n.selected.as_ref())
            .find(|s| s.operation_id == id)
            .unwrap();
        let mut op = serde_json::to_value(&selected.operation).unwrap();
        op["traits"]["instrumentFamilies"] = json!(["equity"]);
        op["traits"]["modes"] = json!([mode]);
        op["traits"]["outputSchemaRefs"]
            .as_array_mut()
            .unwrap()
            .push(json!(
                tradeassembly_plugin_sdk::data_evaluation::EVALUATION_RESPONSE_SCHEMA
            ));
        let manifest = json!({"metadata":{"id":plugin,"version":"1","provider":{"id":plugin,"name":"Fixture"}},"capabilities":[{"id":format!("test.{id}")}],"operations":[op],"health":{"requiredConfiguration":[],"requiredCredentials":[]}});
        let mut contract = fixture_contracts
            .iter()
            .find(|c| c.contract.operation_id == id)
            .unwrap()
            .contract
            .clone();
        contract.applicability.asset_classes = vec!["equity".into()];
        contract.applicability.modes = vec![mode.into()];
        contract.applicability.instrument_families = vec!["equity".into()];
        for port in contract.inputs.values_mut() {
            port.data.schema.sha256 = schema_hash.clone();
        }
        for port in contract.outputs.values_mut() {
            port.schema.sha256 = schema_hash.clone();
        }
        let state_schema = json!({"type":"object","properties":{"seen":{"type":"integer","minimum":0}},"required":["seen"],"additionalProperties":false});
        let state_bytes = serde_json::to_vec(&state_schema).unwrap();
        let state_hash = canonical_sha256(&state_schema).unwrap();
        if id == "derive" {
            contract.state = Some(
                tradeassembly_plugin_sdk::data_contract::OperationStateContract {
                    schema: tradeassembly_plugin_sdk::data_contract::SchemaIdentity {
                        schema_ref: "schema://test/state@1".into(),
                        sha256: state_hash.clone(),
                    },
                    version: "1".into(),
                    partition: state_partition.clone(),
                },
            );
        }
        if v3 {
            use tradeassembly_plugin_sdk::data_history::*;
            let mut base = contract.into_legacy().unwrap();
            base.schema_version = tradeassembly_plugin_sdk::data_contract::DataContractVersion::V2;
            contract = VersionedOperationData::V3(OperationDataV3 {
                schema_version: HistoryVersion::V3,
                operation: base,
                history: HistoryContract {
                    first_ready: BTreeMap::from([(
                        "value".into(),
                        if id == "provide" {
                            BTreeMap::new()
                        } else {
                            BTreeMap::from([(
                                "source".into(),
                                Observations::Parameter {
                                    name: "window".into(),
                                },
                            )])
                        },
                    )]),
                    partitioning: Partitioning::InstrumentIndependent,
                    required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
                    reconstruction: Reconstruction::CheckpointOrExplicitOrigin,
                },
            });
        }
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let contract_bytes = serde_json::to_vec(&contract).unwrap();
        let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
        descriptor["package"]["plugin"]["id"] = json!(plugin);
        descriptor["package"]["manifest"]["sha256"] = json!(hash(&manifest_bytes));
        descriptor["dataContracts"][0]["operationId"] = json!(id);
        descriptor["dataContracts"][0]["document"]["sha256"] = json!(hash(&contract_bytes));
        descriptor["dataContracts"][0]["canonicalSha256"] =
            json!(canonical_sha256(&contract).unwrap());
        descriptor["dataSchemas"].as_array_mut().unwrap().push(json!({"schemaRef":"schema://test/state@1","document":{"path":"state.json","sha256":hash(&state_bytes)},"canonicalSha256":state_hash}));
        let archive = archive_files(vec![
            (
                "tradeassembly-plugin.json",
                serde_json::to_vec(&descriptor).unwrap(),
            ),
            ("manifest.json", manifest_bytes),
            ("contract.json", contract_bytes),
            ("schema.json", schema_bytes),
            ("state.json", state_bytes),
            ("bin/plugin", b"not-executed".to_vec()),
        ]);
        let path = dir.path().join(format!("{id}.tar.gz"));
        std::fs::write(&path, &archive).unwrap();
        let package = runtime
            .plugin_packages
            .install(&PluginPackageInstallRequest {
                source_type: "file".into(),
                locator: path.to_string_lossy().into(),
                package_sha256: hash(&archive),
                manifest_sha256: canonical_sha256(&manifest).unwrap(),
                offline: true,
            })
            .unwrap();
        runtime.plugins.install_manifest(&plugin,json!({"pluginRef":plugin,"manifest":manifest,"manifestDigest":package.manifest_sha256,"source":{"type":"file"},"package":{"packageSha256":package.package_sha256},"trust":{"level":"test"}}),&ctx).unwrap();
        runtime.plugins.put_instance(&instance,json!({"instanceRef":instance,"pluginRef":plugin,"providerRef":instance,"enabled":true,"activePackageSha256":package.package_sha256,"configuration":{}}),&ctx).unwrap();
        let grant = EntitlementGrant {
            grant_id: format!("test.{id}"),
            profile: "local_full".into(),
            source: "test".into(),
            capability: format!("test.{id}"),
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
        };
        runtime
            .storage
            .put_json(
                "entitlement_grants",
                &grant.grant_id,
                serde_json::to_value(&grant).unwrap(),
                &ctx,
            )
            .unwrap();
    }
    let mut spec: Value = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../examples/strategy-spec/v3/valid/static-equity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    spec["stages"]["evaluate"]["substeps"][0]["config"] =
        json!({"tradeassembly.expression":"bar.close > 0","tradeassembly.quantity":1_000_000});
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name":"Plan storage fixture","spec":spec}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let strategy = created.body["body"]["strategy"]["id"].as_str().unwrap();
    let published=service.handle_http("POST","/product/strategies/publish",json!({"strategyId":strategy,"expectedDraftHash":created.body["body"]["draft"]["draftHash"],"actor":{"kind":"user","id":"user.local"}}));
    assert_eq!(
        published.body["body"]["published"], true,
        "{}",
        published.body
    );
    let scope = PlanScope {
        strategy_id: strategy.into(),
        strategy_version_id: published.body["body"]["version"]["id"]
            .as_str()
            .unwrap()
            .into(),
        strategy_spec_hash: published.body["body"]["version"]["specHash"]
            .as_str()
            .unwrap()
            .into(),
    };
    let revision = super::capability_graph::save_revision(
        &service,
        json!({"configId":"cfg_plan_fixture","bindings":{"req_bars":{"pluginInstanceRef":"local-data","pluginRef":"tradeassembly.local-data","operationId":"marketdata.bars.read_v1"}},"strategyId":scope.strategy_id,"strategyVersionId":scope.strategy_version_id,"strategySpecHash":scope.strategy_spec_hash,"mode":mode,"additionalRequirements":[{"requirementId":"derived","capability":"test.derive","mode":mode,"declaration":"required","fallbackPolicy":"none"}]}),
    );
    assert_eq!(revision["revision"]["graph"]["ok"], true, "{revision:#}");
    let graph: CapabilityGraphResolution =
        serde_json::from_value(revision["revision"]["graph"].clone()).unwrap();
    let source = graph
        .nodes
        .iter()
        .find(|n| {
            n.selected
                .as_ref()
                .is_some_and(|s| s.operation_id == "provide")
        })
        .unwrap()
        .node_id
        .clone();
    let target = "derived".to_string();
    let context = crate::data_pipeline::DataContext {
        instruments: vec![crate::data_pipeline::InstrumentContext {
            instrument_id: "TEST".into(),
            asset_class: "equity".into(),
            instrument_family: "equity".into(),
        }],
        interval_ms: Some(86_400_000),
    };
    let request = PipelineRequest {
        selected_outputs: vec![],
        source_captures: BTreeMap::new(),
        capability_graph_revision: revision["revision"]["revisionId"].as_str().unwrap().into(),
        mode: mode.into(),
        trade_instruments: vec![],
        nodes: vec![
            crate::data_pipeline::PipelineNode {
                graph_node_id: source.clone(),
                parameters: json!({}),
                context: context.clone(),
                input_contexts: Default::default(),
            },
            crate::data_pipeline::PipelineNode {
                graph_node_id: target.clone(),
                parameters: json!({"window":5}),
                context,
                input_contexts: Default::default(),
            },
        ],
        bindings: vec![crate::data_pipeline::PortBinding {
            producer_node: source,
            output_port: "value".into(),
            consumer_node: target,
            input_port: "source".into(),
        }],
    };
    (dir, service, scope, request)
}

#[test]
fn data_plan_service_loads_packages_and_invalidates_revoked_binding() {
    use serde_json::json;
    let (_dir, service, scope, request) = fixture_service(None);
    let runtime = service.runtime();
    let ctx = context("fixture-setup");
    let saved = save(&service, scope.clone(), request.clone(), &ctx).unwrap();
    assert_eq!(current(&service, &saved.revision_id).unwrap(), saved);
    let mut instance = runtime
        .plugins
        .get_instance("instance.provide")
        .unwrap()
        .unwrap();
    instance["enabled"] = json!(false);
    runtime
        .plugins
        .put_instance("instance.provide", instance, &ctx)
        .unwrap();
    assert_eq!(
        current(&service, &saved.revision_id).unwrap_err(),
        "data_plan_graph_stale"
    );
    assert_eq!(
        save(&service, scope, request, &ctx).unwrap_err(),
        "data_plan_graph_stale"
    );
}

#[test]
fn data_plan_provider_configuration_changes_invalidate_prepared_and_cached_identity() {
    let (_dir, service, scope, request) = fixture_service(None);
    let saved = save(
        &service,
        scope.clone(),
        request.clone(),
        &context("binding-pin"),
    )
    .unwrap();
    let runtime = service.runtime();
    let mut provider = runtime
        .plugins
        .get_instance("instance.provide")
        .unwrap()
        .unwrap();
    provider["configuration"] = json!({"datasetRevision":"different-source"});
    runtime
        .plugins
        .put_instance(
            "instance.provide",
            provider,
            &context("change-provider-config"),
        )
        .unwrap();
    assert_eq!(
        current(&service, &saved.revision_id).unwrap_err(),
        "data_plan_stale"
    );
    let fresh = save(&service, scope, request, &context("fresh-binding")).unwrap();
    assert_ne!(fresh.revision_sha256, saved.revision_sha256);
    assert_ne!(fresh.content.binding_sha256, saved.content.binding_sha256);
}
