#[path = "common/live_plugin_fixture.rs"]
mod live_plugin_fixture;

use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;
use tradeassembly_runtime as runtime_crate;
use tradeassembly_runtime::broker_submission::{
    build_order_intent, load_current_state, validate_prepared_binding, BrokerExecutionProvenance,
    BrokerSubmissionDependencies, LocalBrokerSubmissionBoundary,
};
use tradeassembly_runtime::local_owner_identity::LocalOwnerIdentity;
use tradeassembly_runtime::ports::{
    AuthorityContext, BrokerSubmissionPort, IdempotencyKey, PluginOperationRequest,
    SideEffectContext,
};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn loads_positive_deterministic_state_and_rejects_fenced_mutation() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let db = root.join("runtime.db");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    let owner = LocalOwnerIdentity::for_database(&db).unwrap();
    let service = base.for_authenticated_invocation(&owner.issuer, &owner.subject, None, None);
    live_plugin_fixture::install_live_metadata_fixture(&service, &root);
    let saved = service.handle_http("POST", "/product/strategy-execution-configs/save", json!({"strategyId":"strat_local_btc_demo","mode":"live","providerRef":"mandate-live","accountRef":"account://mandate-live/controlled","dataProviderRef":"mandate-live","dataAccountRef":"account://mandate-live/controlled"}));
    assert_eq!(saved.status, 200, "{saved:#?}");
    let config_id = saved.body["body"]["item"]["configId"].as_str().unwrap();
    let issued = service.handle_http("POST", "/product/live-mandates/issue", json!({"configId":config_id,"expiresAtMs":service.runtime().clock.now_ms()+600_000,"idempotencyKey":"loader-mandate"}));
    assert_eq!(issued.status, 201, "{issued:#?}");
    let mandate = issued.body["mandate"].clone();
    let activation_id = "loader-activation";
    let mut run = service
        .runtime()
        .storage
        .get_json("execution_configs", config_id)
        .unwrap()
        .unwrap();
    run["kind"] = json!("ExecutionRun");
    run["state"] = json!("active");
    run["activationId"] = json!(activation_id);
    run["localLiveAuthority"] = json!({"schemaVersion":"tradeassembly.local_live_activation_authority.v1","mandateId":mandate["mandateId"],"mandateDigest":mandate["digest"],"actor":{"actorKind":"user","issuer":owner.issuer,"subject":owner.subject}});
    run["correlationId"] = json!("run-correlation");
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new("loader-state").unwrap(),
    );
    service
        .runtime()
        .storage
        .put_json(
            "execution_runs",
            &format!("run_{activation_id}"),
            run.clone(),
            &context,
        )
        .unwrap();
    service
        .runtime()
        .storage
        .put_json(
            "execution_activations",
            activation_id,
            run.clone(),
            &context,
        )
        .unwrap();
    service.runtime().storage.put_json("execution_controls", activation_id, json!({"activationId":activation_id,"pauseEntries":"available","stop":"available","emergencyStop":"available","newEntriesPaused":false}), &context).unwrap();
    service
        .runtime()
        .storage
        .put_json(
            "execution_reconciliation",
            activation_id,
            json!({"activationId":activation_id,"state":"clear","newEntriesPaused":false}),
            &context,
        )
        .unwrap();
    let revision = service
        .runtime()
        .storage
        .get_json(
            "capability_graph_revisions",
            run["capabilityGraphRevisionId"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
    let node = revision["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["selected"].is_object()
                && n["requirement"]["capability"]
                    .as_str()
                    .unwrap()
                    .starts_with("broker.order_submit")
        })
        .unwrap();
    let selected = &node["selected"];
    let quote_node = revision["graph"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["requirement"]["requirementId"] == "execution.risk.quote")
        .expect("live execution quote requirement");
    assert!(quote_node["blockers"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(quote_node["selected"]["pluginInstanceRef"], "mandate-live");
    assert_eq!(
        quote_node["selected"]["operationId"],
        "marketdata.quote.read"
    );
    assert_eq!(
        quote_node["selected"]["accountRef"],
        "account://mandate-live/controlled"
    );
    let resource = format!("loader-resource:{activation_id}");
    let lease = service
        .runtime()
        .leases
        .acquire(
            &resource,
            "loader-worker",
            service.runtime().clock.now_ms(),
            60_000,
        )
        .unwrap()
        .unwrap();
    let attempt_id = "loader-attempt";
    service.runtime().storage.put_json("execution_attempts", attempt_id, json!({"attemptId":attempt_id,"activationId":activation_id,"state":"running","owner":"loader-worker","fencingToken":lease.fencing_token,"leaseResource":resource}), &context).unwrap();
    let request = PluginOperationRequest {
        correlation_id: "request-correlation".into(),
        plugin_instance_ref: selected["pluginInstanceRef"].as_str().unwrap().into(),
        plugin_ref: selected["pluginRef"].as_str().unwrap().into(),
        manifest_fingerprint: selected["manifestFingerprint"].as_str().unwrap().into(),
        operation_id: selected["operationId"].as_str().unwrap().into(),
        capability: node["requirement"]["capability"].as_str().unwrap().into(),
        capability_graph_revision_id: run["capabilityGraphRevisionId"].as_str().unwrap().into(),
        capability_graph_fingerprint: run["capabilityGraphFingerprint"].as_str().unwrap().into(),
        strategy_id: run["strategyId"].as_str().unwrap().into(),
        strategy_version_id: run["strategyVersionId"].as_str().unwrap().into(),
        strategy_spec_hash: run["strategySpecHash"].as_str().unwrap().into(),
        activation_id: activation_id.into(),
        attempt_id: attempt_id.into(),
        evaluation_tick_id: "tick-1".into(),
        mode: "live".into(),
        purpose: "live_order_submission".into(),
        account_ref: selected["accountRef"].as_str().map(str::to_string),
        timeout_ms: 1000,
        fencing_token: Some(lease.fencing_token),
        input: json!({}),
        evidence_refs: vec![],
    };
    let deps = BrokerSubmissionDependencies {
        storage: Arc::clone(&service.runtime().storage),
        plugins: Arc::clone(&service.runtime().plugins),
        plugin_packages: Arc::clone(&service.runtime().plugin_packages),
        credentials: Arc::clone(&service.runtime().credentials),
        capability_resolver: Arc::clone(&service.runtime().capability_resolver),
        clock: Arc::clone(&service.runtime().clock),
        leases: Arc::clone(&service.runtime().leases),
        owner: owner.clone(),
    };
    let state = load_current_state(&deps, &request, &context).unwrap();
    assert!(matches!(
        state.provenance,
        BrokerExecutionProvenance::Deterministic { .. }
    ));
    assert_eq!(state.actor.subject, owner.subject);
    let prepared = tradeassembly_runtime::ports::broker_submission::BrokerPreparedBinding {
        plugin_instance_ref: request.plugin_instance_ref.clone(),
        plugin_ref: request.plugin_ref.clone(),
        package_sha256: state.package.package_sha256.clone(),
        manifest_fingerprint: request.manifest_fingerprint.clone(),
        configuration_digest: tradeassembly_runtime::spec::canonical_hash(
            &state.instance["configuration"],
        )
        .unwrap(),
        credential_ref: state.instance["credentialRef"].as_str().unwrap().into(),
        credential_generation: 1, // Explicit installed fixture credential revision.
    };
    validate_prepared_binding(&state, &prepared).unwrap();
    let mut order_request = request.clone();
    order_request.input = json!({"symbol":"BTC/USD","side":"buy","orderType":"market","timeInForce":"gtc","clientOrderId":"controlled-order","quantity":"0.000003","quantityMicros":999});
    let (intent, digest) = build_order_intent(&state, &order_request, &context, &prepared).unwrap();
    assert_eq!(intent["order"]["quantityMicros"], 3);
    assert_eq!(intent["dispatchQuantity"], "0.000003");
    assert_eq!(intent["actor"]["subject"], owner.subject);
    assert_eq!(intent["leaseFence"], lease.fencing_token);
    assert_eq!(intent["mandateDigest"], mandate["digest"]);
    let boundary = LocalBrokerSubmissionBoundary::new(
        deps.clone(),
        service.runtime().finance_authority.clone(),
    );
    assert_eq!(
        boundary
            .admit(&order_request, &context, &prepared)
            .err()
            .as_deref(),
        Some("broker_price_receipt_required")
    );
    let mut forged_price_request = order_request.clone();
    forged_price_request.evidence_refs = vec!["plugin-receipt:nonexistent".into()];
    assert!(boundary
        .admit(&forged_price_request, &context, &prepared)
        .is_err());
    assert!(service
        .runtime()
        .storage
        .list_json("broker_order_intents")
        .unwrap()
        .is_empty());
    assert_eq!(
        digest,
        tradeassembly_runtime::spec::canonical_hash(&intent).unwrap()
    );
    assert_eq!(
        build_order_intent(&state, &order_request, &context, &prepared)
            .unwrap()
            .1,
        digest
    );
    order_request.input["quantity"] = json!("0.000004");
    assert_ne!(
        build_order_intent(&state, &order_request, &context, &prepared)
            .unwrap()
            .1,
        digest
    );
    for field in 0..7 {
        let mut changed = prepared.clone();
        match field {
            0 => changed.plugin_instance_ref.push_str("-changed"),
            1 => changed.plugin_ref.push_str("-changed"),
            2 => changed.package_sha256.push_str("-changed"),
            3 => changed.manifest_fingerprint.push_str("-changed"),
            4 => changed.configuration_digest.push_str("-changed"),
            5 => changed.credential_ref.push_str("-changed"),
            6 => changed.credential_generation += 1,
            _ => unreachable!(),
        }
        assert_eq!(
            validate_prepared_binding(&state, &changed).unwrap_err(),
            "prepared_invocation_binding_stale"
        );
    }
    let mut stale = request.clone();
    stale.fencing_token = Some(lease.fencing_token + 1);
    assert_eq!(
        load_current_state(&deps, &stale, &context).unwrap_err(),
        "request_fencing_token_mismatch"
    );
}
