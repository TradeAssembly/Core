// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tradeassembly_runtime::service::TradeAssemblyService;

fn service_pair() -> (TradeAssemblyService, TradeAssemblyService) {
    let db = tempfile::Builder::new()
        .prefix("tradeassembly-plugin-object-authorization-")
        .suffix(".db")
        .tempfile()
        .expect("temporary database")
        .into_temp_path()
        .keep()
        .expect("keep database");
    let base = TradeAssemblyService::test_local(db.to_string_lossy());
    let alice = base.for_authenticated_invocation(
        "https://issuer-a.example",
        "alice",
        None,
        Some("Alice".to_string()),
    );
    let bob = base.for_authenticated_invocation(
        "https://issuer-b.example",
        "bob",
        None,
        Some("Bob".to_string()),
    );
    (alice, bob)
}

#[test]
fn plugin_instances_and_credentials_are_owner_scoped_without_secret_disclosure() {
    let (alice, bob) = service_pair();
    install_fixture_manifest(&alice);

    let created = request(
        &alice,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "alice-private-plugin",
            "pluginRef": "example.object-auth-plugin",
            "configuration": {"base_url": "https://paper-api.example.invalid"},
        }),
    );
    assert_eq!(created["instance"]["instanceRef"], "alice-private-plugin");

    let owner_list = request(&alice, "GET", "/plugins/instances", json!({}));
    assert!(owner_list["instances"]
        .as_array()
        .expect("instance list")
        .iter()
        .any(|record| record["instanceRef"] == "alice-private-plugin"));
    let foreign_list = request(&bob, "GET", "/plugins/instances", json!({}));
    assert!(!foreign_list.to_string().contains("alice-private-plugin"));

    let missing = bob.handle_http("GET", "/plugins/instances/missing-plugin", json!({}));
    let foreign = bob.handle_http("GET", "/plugins/instances/alice-private-plugin", json!({}));
    assert_eq!(foreign.status, 404);
    assert_eq!(foreign.body, missing.body);
    assert!(!foreign.body.to_string().contains("alice-private-plugin"));

    let seeded_foreign = bob.handle_http("GET", "/plugins/instances/sim", json!({}));
    assert_eq!(seeded_foreign.status, 404);
    assert_eq!(seeded_foreign.body, missing.body);

    let legacy_foreign = bob.handle_http(
        "POST",
        "/providers/alice-private-plugin/credentials/test",
        json!({}),
    );
    let legacy_missing = bob.handle_http(
        "POST",
        "/providers/missing-plugin/credentials/test",
        json!({}),
    );
    assert_eq!(legacy_foreign.status, 404);
    assert_eq!(legacy_foreign.body, legacy_missing.body);

    for (method, path, body) in [
        (
            "PUT",
            "/plugins/instances/alice-private-plugin/configuration",
            json!({"configuration": {"base_url": "https://attacker.invalid"}}),
        ),
        (
            "POST",
            "/plugins/instances/alice-private-plugin:enable",
            json!({}),
        ),
        (
            "POST",
            "/plugins/instances/alice-private-plugin/credentials",
            json!({"credentials": {"api_key": "attacker-key", "api_secret": "attacker-secret"}}),
        ),
        (
            "GET",
            "/plugins/instances/alice-private-plugin/credentials",
            json!({}),
        ),
        (
            "DELETE",
            "/plugins/instances/alice-private-plugin/credentials",
            json!({}),
        ),
        (
            "POST",
            "/plugins/instances/alice-private-plugin/health:refresh",
            json!({}),
        ),
    ] {
        let response = bob.handle_http(method, path, body);
        assert_eq!(response.status, 404, "{method} {path}: {:#}", response.body);
        assert_eq!(
            response.body, missing.body,
            "{method} {path}: {:#}",
            response.body
        );
    }

    let stored = request(
        &alice,
        "POST",
        "/plugins/instances/alice-private-plugin/credentials",
        json!({"credentials": {"api_key": "owner-key", "api_secret": "owner-secret"}}),
    );
    assert_eq!(stored["credentialStatus"]["configured"], true);
    assert!(!stored.to_string().contains("owner-secret"));

    let owner = request(
        &alice,
        "GET",
        "/plugins/instances/alice-private-plugin",
        json!({}),
    );
    assert_eq!(
        owner["configuration"]["base_url"],
        "https://paper-api.example.invalid"
    );
    assert_eq!(owner["enabled"], false);
    assert_eq!(owner["credentialStatus"]["configured"], true);
    assert!(!owner.to_string().contains("owner-secret"));
}

fn request(service: &TradeAssemblyService, method: &str, path: &str, body: Value) -> Value {
    let response = service.handle_http(method, path, body);
    assert_eq!(response.status, 200, "{method} {path}: {:#}", response.body);
    response.body
}

fn install_fixture_manifest(service: &TradeAssemblyService) {
    let manifests = request(service, "GET", "/plugins/manifests", json!({}));
    let mut manifest = manifests["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
        .expect("simbroker manifest")["manifest"]
        .clone();
    manifest["metadata"]["id"] = json!("example.object-auth-plugin");
    manifest["metadata"]["name"] = json!("Object Authorization Fixture Plugin");
    manifest["metadata"]["provider"] = json!({"id": "example", "name": "Example"});
    manifest["configuration"] = json!({
        "fields": [
            {"id": "api_key", "label": "API key", "description": "Fixture key.", "inputType": "password", "required": true, "storageClass": "credential"},
            {"id": "api_secret", "label": "API secret", "description": "Fixture secret.", "inputType": "password", "required": true, "storageClass": "credential"},
            {"id": "base_url", "label": "API URL", "description": "Fixture endpoint.", "inputType": "url", "required": true, "storageClass": "configuration"}
        ]
    });
    manifest["health"] = json!({
        "requiredConfiguration": ["base_url"],
        "requiredCredentials": ["api_key", "api_secret"],
        "connectionCheckOperation": null
    });
    let digest = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json_canonicalizer::to_vec(&manifest).expect("canonical manifest"))
    );
    request(
        service,
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": manifest,
            "source": {"type": "git", "commit": "0123456789abcdef0123456789abcdef01234567", "locator": "git+https://example.invalid/plugins.git?ref=0123456789abcdef0123456789abcdef01234567#example.object-auth-plugin"},
            "integrity": {"manifestSha256": digest},
            "trust": {"level": "local-test"}
        }),
    );
}
