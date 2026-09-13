use serde_json::json;
use tempfile::NamedTempFile;
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn canonical_robustness_routes_expose_durable_state_and_reject_legacy_fixtures() {
    let file = NamedTempFile::new().expect("db");
    let service = TradeAssemblyService::test_local(file.path().to_string_lossy());

    let listed = service.handle_http("GET", "/robustness-runs", json!({}));
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["runs"], json!([]));

    let missing_source = service.handle_http(
        "POST",
        "/robustness-runs",
        json!({
            "studyKind": "monte_carlo",
            "assumptions": {"study":"monte_carlo"},
            "budget": {
                "maximumSamples": 100,
                "maximumGridPoints": 10,
                "maximumWindows": 10,
                "maximumScenarios": 10,
                "maximumOutputBytes": 100000,
                "maximumAttempts": 2
            },
            "deterministicSeed": 7,
            "idempotencyKey": "missing-source"
        }),
    );
    assert_eq!(missing_source.status, 400);
    assert_eq!(
        missing_source.body["error"]["code"],
        "robustness_request_invalid"
    );

    let legacy_fixture_request = service.handle_http(
        "POST",
        "/product/strategies/monte-carlo",
        json!({"strategyId":"strategy-1","assumptionHash":"fixture"}),
    );
    assert_eq!(legacy_fixture_request.status, 400);
    assert!(legacy_fixture_request.body.get("reportEnvelope").is_none());

    let absent = service.handle_http("GET", "/robustness-runs/absent", json!({}));
    assert_eq!(absent.status, 400);
    assert_eq!(absent.body["error"]["code"], "robustness_not_found");
}

#[test]
fn studio_origin_is_trusted_once_and_rejects_unsafe_forms() {
    let file = NamedTempFile::new().expect("db");
    let service = TradeAssemblyService::test_local(file.path().to_string_lossy())
        .with_studio_base_url("http://127.0.0.1:3002/")
        .expect("loopback origin");
    let unavailable = service.handle_http(
        "GET",
        "/robustness-runs/absent/report",
        json!({
            "studioBaseUrl": "https://attacker.invalid"
        }),
    );
    assert_eq!(unavailable.status, 400);
    assert_eq!(
        TradeAssemblyService::test_local(
            NamedTempFile::new().expect("db").path().to_string_lossy()
        )
        .with_studio_base_url("https://example.invalid/path")
        .expect_err("path must be rejected"),
        "studio_origin_invalid"
    );
    assert!(TradeAssemblyService::test_local(
        NamedTempFile::new().expect("db").path().to_string_lossy()
    )
    .with_studio_base_url("http://example.invalid:3001")
    .is_err());
}
