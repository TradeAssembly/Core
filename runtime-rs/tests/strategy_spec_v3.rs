use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tradeassembly_runtime::service::TradeAssemblyService;
use tradeassembly_runtime::spec::{
    canonical_bytes, canonical_hash, detect_source_family, migrate_bytes, raw_hash,
    validate_cli_file, validate_strategy_spec_report, StageName, StrategySpec,
    StrategySpecSourceFamily,
};

fn corpus_path(section: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/strategy-spec/v3")
        .join(section)
        .join(name)
}

fn load(section: &str, name: &str) -> Value {
    serde_json::from_slice(&fs::read(corpus_path(section, name)).expect("fixture bytes"))
        .expect("fixture JSON")
}

#[test]
fn valid_corpus_round_trips_through_the_closed_model() {
    assert_eq!(
        StageName::TRANSFORMATION_ORDER.last(),
        Some(&StageName::PricePolicy)
    );
    assert!(!StageName::TRANSFORMATION_ORDER.contains(&StageName::MarketClock));
    assert_eq!(
        StageName::VALIDATION_ORDER.first(),
        Some(&StageName::MarketClock)
    );
    let cases = [
        "static-equity.json",
        "dynamic-screener-equity.json",
        "crypto-spot-24x7.json",
        "underlying-single-leg-option.json",
        "multi-leg-option-spread.json",
        "mixed-instruments-multiple-symbols.json",
        "transitive-stateful-plugins.json",
    ];
    for name in cases {
        let value = load("valid", name);
        let report = validate_strategy_spec_report(&value);
        assert!(report.valid, "{name}: {:#?}", report.diagnostics);
        let model = StrategySpec::model_validate(value.clone()).expect("closed V3 model");
        let round_trip = serde_json::to_value(model).expect("serialize V3 model");
        assert!(
            validate_strategy_spec_report(&round_trip).valid,
            "round trip failed for {name}"
        );
    }

    let dynamic = load("valid", "dynamic-screener-equity.json");
    assert_eq!(
        dynamic.pointer("/signal_market_requirements/0/selectors/0/type"),
        Some(&json!("dynamic"))
    );
    let crypto = load("valid", "crypto-spot-24x7.json");
    assert_eq!(
        crypto.pointer("/stages/market_clock/policy/calendar_refs/0"),
        Some(&json!("CRYPTO_24X7"))
    );
    let multi_leg = load("valid", "multi-leg-option-spread.json");
    assert_eq!(
        multi_leg.pointer("/stages/order_strategy/policy/multi_leg_mode"),
        Some(&json!("atomic_required"))
    );
    let mixed = load("valid", "mixed-instruments-multiple-symbols.json");
    assert_eq!(
        mixed.pointer("/trade_market_requirements/0/supports_mixed_instruments"),
        Some(&json!(true))
    );
    let stateful = load("valid", "transitive-stateful-plugins.json");
    assert_eq!(
        stateful.pointer("/stages/evaluate/substeps/0/state/sharing_scope"),
        Some(&json!("instrument"))
    );
}

#[test]
fn invalid_corpus_has_stable_codes_and_json_pointers() {
    let cases = [
        (
            "unresolved-external-read.json",
            "SPEC_UNRESOLVED_READ",
            "/stages/evaluate/substeps/0/reads/0",
        ),
        ("dataflow-cycle.json", "SPEC_DATAFLOW_CYCLE", "/stages"),
        (
            "schema-ref-mismatch.json",
            "SPEC_SCHEMA_REF_MISMATCH",
            "/stages/universe/substeps/0/input_schema_ref",
        ),
        (
            "wrong-policy-schema.json",
            "SPEC_STRUCTURE_INVALID",
            "/stages/order_strategy/policy/clock",
        ),
        (
            "malformed-expression.json",
            "SPEC_EXPRESSION_PARSE_INVALID",
            "/signal_market_requirements/0/selectors/0/expression",
        ),
        (
            "expression-type-mismatch.json",
            "SPEC_EXPRESSION_TYPE_MISMATCH",
            "/signal_market_requirements/0/selectors/0/expression/result_type",
        ),
        (
            "expression-cost-exceeded.json",
            "SPEC_EXPRESSION_COST_EXCEEDED",
            "/signal_market_requirements/0/selectors/0/expression/max_cost",
        ),
        (
            "missing-capability-ref.json",
            "SPEC_CAPABILITY_REF_MISSING",
            "/stages/risk/substeps/0/capability_requirement_refs/0",
        ),
        (
            "reserved-authority-recursive.json",
            "SPEC_AUTHORITY_RESERVED",
            "/metadata/nested/config/provider_ref",
        ),
    ];
    for (name, code, pointer) in cases {
        let path = corpus_path("invalid", name);
        let value = load("invalid", name);
        let report = validate_strategy_spec_report(&value);
        assert!(!report.valid, "{name} unexpectedly valid");
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code && diagnostic.pointer == pointer),
            "{name}: expected {code} {pointer}, got {:#?}",
            report.diagnostics
        );
        let cli = validate_cli_file(&path).expect_err("invalid fixture must fail CLI validation");
        let expected = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.display_line())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(cli, expected, "CLI diagnostic drift for {name}");
    }
}

#[test]
fn migration_corpus_detects_exact_families_and_preserves_source_evidence() {
    let predecessor_path = corpus_path("migrations", "predecessor-strategy-contract-v2.json");
    let predecessor_bytes = fs::read(&predecessor_path).unwrap();
    let predecessor_value: Value = serde_json::from_slice(&predecessor_bytes).unwrap();
    assert_eq!(
        detect_source_family(&predecessor_value),
        StrategySpecSourceFamily::PredecessorV2
    );
    let predecessor = migrate_bytes(&predecessor_bytes).unwrap();
    assert_eq!(
        predecessor.source_representation,
        tradeassembly_runtime::spec::MigrationSourceRepresentation::IngressBytes
    );
    assert_eq!(predecessor.original_document.as_bytes(), predecessor_bytes);
    assert_eq!(predecessor.original_hash, raw_hash(&predecessor_bytes));
    assert!(predecessor.proposed_v3_draft.is_none());
    assert!(predecessor.requires_review);
    assert!(!predecessor.questions.is_empty());
    assert!(!predecessor.diff.is_empty());
    assert!(predecessor
        .execution_config_proposal
        .contains_key("/runtime/provider_ref"));

    let skeletal_bytes = fs::read(corpus_path(
        "migrations",
        "skeletal-equity-sma-cross-v2.json",
    ))
    .unwrap();
    let skeletal_value: Value = serde_json::from_slice(&skeletal_bytes).unwrap();
    assert_eq!(
        detect_source_family(&skeletal_value),
        StrategySpecSourceFamily::SkeletalTradeAssemblyV2
    );
    let skeletal = migrate_bytes(&skeletal_bytes).unwrap();
    assert!(skeletal.proposed_v3_draft.is_none());
    assert!(skeletal.questions.iter().all(|question| question.blocking));

    let ambiguous_bytes = fs::read(corpus_path("migrations", "ambiguous-v2.json")).unwrap();
    let ambiguous = migrate_bytes(&ambiguous_bytes).unwrap();
    assert_eq!(
        ambiguous.source_family,
        StrategySpecSourceFamily::AmbiguousV2
    );
    assert!(ambiguous
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "MIGRATION_SOURCE_AMBIGUOUS"));
}

#[test]
fn service_publication_path_is_strict_v3_and_preserves_canonical_identity() {
    let db = format!(
        ".tradeassembly/test-strategy-spec-v3-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let value = load("valid", "static-equity.json");

    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "V3 service fixture", "spec": value}),
    );
    assert_eq!(created.status, 201, "{:#?}", created.body);
    assert_eq!(
        created.body["body"]["validation"]["readiness"]["specValid"],
        true
    );
    assert_eq!(
        created.body["body"]["validation"]["readiness"]["researchReady"],
        true
    );
    assert_eq!(
        created.body["body"]["validation"]["readiness"]["workspaceCompatible"],
        false
    );
    let strategy_id = created.body["body"]["strategy"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut value = created.body["body"]["strategy"]["latestSpec"].clone();
    assert_eq!(value["strategy_id"], strategy_id);
    assert_eq!(value["name"], "V3 service fixture");

    value["name"] = json!("V3 service fixture saved");
    let saved = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId": strategy_id, "spec": value}),
    );
    assert_eq!(saved.status, 200);
    assert_eq!(saved.body["body"]["draftSaved"], true, "{:#?}", saved.body);
    let expected_hash = canonical_hash(&value).unwrap();
    assert_eq!(saved.body["body"]["draft"]["specHash"], expected_hash);

    let validated = service.handle_http(
        "POST",
        "/product/strategies/validate-draft",
        json!({"strategyId": strategy_id, "spec": value}),
    );
    assert_eq!(
        validated.body["body"]["validation"]["ok"], true,
        "{:#?}",
        validated.body
    );

    let contract = service.handle_http("POST", "/strategy/contracts/validate", value.clone());
    assert_eq!(contract.status, 200, "{:#?}", contract.body);
    assert_eq!(contract.body["body"]["report"]["spec_hash"], expected_hash);

    let published = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(
        published.body["body"]["published"], true,
        "{:#?}",
        published.body
    );
    let version = &published.body["body"]["version"];
    assert_eq!(version["specHash"], expected_hash);
    assert_eq!(version["canonicalSpecHash"], expected_hash);
    assert_eq!(version["identityAlgorithm"], "RFC8785+SHA-256");
    assert_eq!(
        version["sourceRepresentation"],
        "normalized_parsed_json_value"
    );
    let normalized = version["normalizedSourceSpecBytes"]
        .as_str()
        .unwrap()
        .as_bytes();
    assert_eq!(version["normalizedSourceSpecHash"], raw_hash(normalized));
    assert_eq!(serde_json::from_slice::<Value>(normalized).unwrap(), value);
    let canonical = version["canonicalSpecBytes"].as_str().unwrap().as_bytes();
    assert_eq!(canonical, canonical_bytes(&value).unwrap());
    assert_eq!(version["canonicalSpecHash"], raw_hash(canonical));

    let mut mismatched = value.clone();
    mismatched["strategy_id"] = json!("different_strategy_family");
    let rejected_mismatch = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId": strategy_id, "spec": mismatched}),
    );
    assert_eq!(rejected_mismatch.body["ok"], false);
    assert_eq!(
        rejected_mismatch.body["error"]["code"],
        "strategy_draft_invalid"
    );
    assert!(rejected_mismatch.body["body"]["validation"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["label"] == "strategy_family_identity" && check["ok"] == false));

    let skeletal = json!({"schemaVersion": "tradeassembly.strategy.v1", "name": "not V3"});
    let rejected_create = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "not V3", "spec": skeletal}),
    );
    assert_eq!(rejected_create.status, 400);
    assert_eq!(
        rejected_create.body["error"]["code"],
        "strategy_draft_invalid"
    );
    let rejected_contract = service.handle_http(
        "POST",
        "/strategy/contracts/validate",
        json!({"schemaVersion": "tradeassembly.strategy.v1", "name": "not V3"}),
    );
    assert_eq!(rejected_contract.status, 400);
    assert_eq!(
        rejected_contract.body["error"]["details"]["report"]["diagnostics"][0]["code"],
        "SPEC_VERSION_UNSUPPORTED"
    );
}

#[test]
fn gc07_safe_invalid_drafts_persist_but_cannot_publish() {
    let db = format!(
        ".tradeassembly/test-gc07-invalid-draft-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "GC-07 invalid draft", "spec": load("valid", "static-equity.json")}),
    );
    assert_eq!(created.status, 201, "{:#?}", created.body);
    let strategy_id = created.body["body"]["strategy"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut invalid_spec = created.body["body"]["draft"]["spec"].clone();
    invalid_spec["trade_market_requirements"] = json!([]);

    let saved = service.handle_http(
        "POST",
        "/product/strategies/save-draft",
        json!({"strategyId": strategy_id, "spec": invalid_spec}),
    );
    assert_eq!(saved.status, 200, "{:#?}", saved.body);
    assert_eq!(saved.body["ok"], true);
    assert_eq!(saved.body["body"]["draftSaved"], true);
    assert_eq!(saved.body["body"]["draft"]["status"], "invalid");
    assert_eq!(saved.body["body"]["validation"]["ok"], false);

    let reloaded = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": strategy_id}),
    );
    assert_eq!(reloaded.body["draft"]["status"], "invalid");
    assert_eq!(
        reloaded.body["editableSpec"]["trade_market_requirements"],
        json!([])
    );
    assert_eq!(reloaded.body["validation"]["ok"], false);

    let publish = service.handle_http(
        "POST",
        "/product/strategies/publish",
        json!({
            "strategyId": strategy_id,
            "expectedDraftHash": saved.body["body"]["draft"]["draftHash"],
            "actor": {"kind": "user", "id": "user.local"}
        }),
    );
    assert_eq!(publish.body["ok"], false);
    assert_eq!(publish.body["error"]["code"], "strategy_draft_invalid");
}

#[test]
fn gc07_json_import_is_explicit_and_preserves_safe_invalid_content() {
    let db = format!(
        ".tradeassembly/test-gc07-json-import-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let mut invalid_spec = load("valid", "crypto-spot-24x7.json");
    invalid_spec["capability_requirements"]["required"] = json!([]);

    let ordinary_create = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "Rejected invalid create", "spec": invalid_spec.clone()}),
    );
    assert_eq!(ordinary_create.status, 400);
    assert_eq!(
        ordinary_create.body["error"]["code"],
        "strategy_draft_invalid"
    );

    let imported = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"mode": "import", "name": "Imported invalid draft", "spec": invalid_spec}),
    );
    assert_eq!(imported.status, 201, "{:#?}", imported.body);
    assert_eq!(imported.body["body"]["draft"]["status"], "invalid");
    assert_eq!(imported.body["body"]["validation"]["ok"], false);
    assert_eq!(
        imported.body["body"]["draft"]["spec"]["capability_requirements"]["required"],
        json!([])
    );

    let mut unsafe_spec = load("valid", "static-equity.json");
    unsafe_spec["credential_handle"] = json!("secret-handle");
    let unsafe_import = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"mode": "import", "name": "Unsafe import", "spec": unsafe_spec}),
    );
    assert_eq!(unsafe_import.status, 400);
    assert_eq!(
        unsafe_import.body["error"]["code"],
        "strategy_draft_invalid"
    );
    assert!(
        unsafe_import.body["error"]["details"]["persistenceViolations"]
            .as_array()
            .is_some_and(|violations| !violations.is_empty())
    );
}

#[test]
fn graphql_json_import_preserves_safe_invalid_content() {
    let db = format!(
        ".tradeassembly/test-graphql-json-import-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let mut invalid_spec = load("valid", "static-equity.json");
    invalid_spec["name"] = json!("Imported invalid StrategySpec");
    invalid_spec["trade_market_requirements"] = json!([]);

    let imported = service.execute_graphql(json!({
        "operationName": "CreateStrategy",
        "query": "mutation CreateStrategy { createStrategy }",
        "variables": {
            "mode": "import",
            "name": "Imported invalid StrategySpec",
            "spec": invalid_spec,
        },
    }));

    assert!(
        imported.get("errors").is_none(),
        "safe invalid imports must persist through GraphQL: {imported:#}"
    );
    assert_eq!(
        imported["data"]["createStrategy"]["body"]["draft"]["status"],
        "invalid"
    );
    assert_eq!(
        imported["data"]["createStrategy"]["body"]["draft"]["spec"]["trade_market_requirements"],
        json!([])
    );
}

#[test]
fn gc07_compile_preview_is_spec_derived_and_never_executable() {
    let db = format!(
        ".tradeassembly/test-gc07-compile-preview-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let spec = load("valid", "static-equity.json");
    let preview = service.handle_http("POST", "/strategy/contracts/envelope-preview", spec.clone());
    assert_eq!(preview.status, 200, "{:#?}", preview.body);
    let plan = &preview.body["body"]["orderPlanPreview"];
    assert_eq!(plan["strategyId"], spec["strategy_id"]);
    assert_eq!(plan["symbols"], json!(["SPY"]));
    assert_eq!(
        plan["orderPolicy"]["allowedOrderTypes"],
        json!(["market", "limit"])
    );
    assert_eq!(plan["submit"], false);
    assert_eq!(plan["executable"], false);
    assert!(
        plan.get("side").is_none(),
        "preview must not invent direction"
    );
    assert!(
        plan.get("quantity").is_none(),
        "preview must not invent size"
    );

    let mut invalid = spec;
    invalid["trade_market_requirements"] = json!([]);
    let rejected = service.handle_http("POST", "/strategy/contracts/envelope-preview", invalid);
    assert_eq!(rejected.status, 400);
    assert_eq!(rejected.body["error"]["code"], "strategy_preview_invalid");
    assert!(rejected.body.get("orderPlanPreview").is_none());
}

#[test]
fn gc07_builder_state_resolves_each_declared_capability_requirement() {
    let db = format!(
        ".tradeassembly/test-gc07-builder-resolver-{}.db",
        std::process::id()
    );
    let service = TradeAssemblyService::test_local(&db);
    let mut spec = load("valid", "crypto-spot-24x7.json");
    let mut optional = spec["capability_requirements"]["required"][0].clone();
    optional["requirement_id"] = json!("req_optional_bars");
    optional["purpose"] = json!("Optional alternate normalized bars");
    spec["capability_requirements"]["optional"] = json!([optional]);
    let created = service.handle_http(
        "POST",
        "/product/strategies/create",
        json!({"name": "GC-07 resolver", "spec": spec}),
    );
    let strategy_id = created.body["body"]["strategy"]["id"].as_str().unwrap();
    let state = service.handle_http(
        "POST",
        "/product/strategies/builder-state",
        json!({"strategyId": strategy_id}),
    );
    let required = state.body["editableSpec"]["capability_requirements"]["required"]
        .as_array()
        .unwrap();
    let optional = state.body["editableSpec"]["capability_requirements"]["optional"]
        .as_array()
        .unwrap();
    let requirements = required.iter().chain(optional).collect::<Vec<_>>();
    let resolutions = state.body["capabilityResolutions"].as_array().unwrap();
    assert_eq!(resolutions.len(), requirements.len());
    for (requirement, resolution) in requirements.iter().zip(resolutions) {
        assert_eq!(
            resolution["requirement"]["requirementId"],
            requirement["requirement_id"]
        );
        assert_eq!(
            resolution["requirement"]["capability"],
            requirement["capability"]
        );
    }
    assert_eq!(resolutions[0]["declaration"], "required");
    assert_eq!(resolutions.last().unwrap()["declaration"], "optional");

    let graph = &state.body["capabilityGraphResolution"];
    assert_eq!(graph["schemaVersion"], "tradeassembly.capability_graph.v1");
    let nodes = graph["nodes"].as_array().expect("capability graph nodes");
    for requirement in requirements {
        let requirement_id = requirement["requirement_id"].as_str().unwrap();
        let node = nodes
            .iter()
            .find(|node| node["nodeId"].as_str() == Some(requirement_id))
            .unwrap_or_else(|| panic!("missing graph node {requirement_id}: {graph:#}"));
        assert_eq!(node["origin"], "strategy_requirement");
        assert_eq!(node["requirement"]["capability"], requirement["capability"]);
    }
    assert!(graph["edges"].as_array().is_some_and(|edges| edges
        .iter()
        .any(|edge| edge["origin"] == "strategy_dependency")));
    let serialized = serde_json::to_string(graph).unwrap();
    for forbidden in ["api_key", "api_secret", "credentialStoreHandle"] {
        assert!(
            !serialized.contains(forbidden),
            "builder graph leaked {forbidden}"
        );
    }
}

#[test]
fn canonical_hash_is_equal_across_key_order_but_original_evidence_is_not() {
    let left_bytes = br#"{"b":2,"a":1}"#;
    let right_bytes = br#"{"a":1,"b":2}"#;
    let left: Value = serde_json::from_slice(left_bytes).unwrap();
    let right: Value = serde_json::from_slice(right_bytes).unwrap();
    assert_eq!(
        canonical_hash(&left).unwrap(),
        canonical_hash(&right).unwrap()
    );
    assert_ne!(raw_hash(left_bytes), raw_hash(right_bytes));
}

#[test]
fn state_sharing_scopes_preserve_account_authorization_semantics() {
    let base = load("valid", "transitive-stateful-plugins.json");
    for scope in ["instrument", "activation", "research_run"] {
        let mut value = base.clone();
        value["stages"]["evaluate"]["substeps"][0]["state"]["sharing_scope"] = json!(scope);
        assert!(
            validate_strategy_spec_report(&value).valid,
            "portable state scope {scope} should validate"
        );
    }

    let mut account = base;
    account["portfolio_scope"] = json!("account");
    account["capability_requirements"]["required"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "requirement_id": "req_account_state",
            "capability": "portfolio.account.read@1",
            "purpose": "Authorize account-scoped state declaration",
            "required_for": ["research", "backtest", "paper", "live"],
            "stage_refs": ["evaluate"],
            "substep_refs": ["evaluate_signal"],
            "constraints": {
                "instrument_families": ["equity"],
                "data_shapes": ["account_portfolio"],
                "operations": ["read"],
                "fields": ["positions"],
                "input_paths": [],
                "schema_refs": ["schema://portfolio/account@1"],
                "deterministic": true,
                "replayable": true
            },
            "dependency_refs": [],
            "fallback_policy": "none",
            "policy_tags": []
        }));
    account["stages"]["evaluate"]["substeps"][0]["capability_requirement_refs"]
        .as_array_mut()
        .unwrap()
        .push(json!("req_account_state"));
    account["stages"]["evaluate"]["substeps"][0]["state"]["sharing_scope"] = json!("account");
    account["stages"]["evaluate"]["substeps"][0]["state"]["sharing_authorization_capability_ref"] =
        json!("req_account_state");
    assert!(validate_strategy_spec_report(&account).valid);

    account["stages"]["evaluate"]["substeps"][0]["state"]["sharing_authorization_capability_ref"] =
        Value::Null;
    let rejected = validate_strategy_spec_report(&account);
    assert!(rejected
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "SPEC_ACCOUNT_STATE_CAPABILITY_REQUIRED"));
}
