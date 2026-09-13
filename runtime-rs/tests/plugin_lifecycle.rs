// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use flate2::{write::GzEncoder, Compression};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;
use tar::{Builder, Header};
use tradeassembly_runtime::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use tradeassembly_runtime::service::TradeAssemblyService;

#[test]
fn two_plugin_instances_persist_configuration_credentials_and_health_independently() {
    let directory = tempfile::tempdir().expect("temporary plugin lifecycle directory");
    let database = directory.path().join("runtime.db");
    let service = TradeAssemblyService::test_local(database.to_string_lossy().to_string());
    install_fixture_manifest(&service);

    for (instance_ref, endpoint) in [
        ("alpaca-one", "https://paper-api.alpaca.markets"),
        ("alpaca-two", "https://sandbox.example.invalid"),
    ] {
        let created = request(
            &service,
            "POST",
            "/plugins/instances",
            json!({
                "instanceRef": instance_ref,
                "pluginRef": "example.lifecycle-plugin",
                "enabled": true,
                "configuration": {"base_url": endpoint, "account_mode": "paper"},
            }),
        );
        assert_eq!(created["ok"], true, "{created:#}");
    }

    let bound = request(
        &service,
        "PUT",
        "/plugins/instances/alpaca-one/configuration",
        json!({
            "configuration": {},
            "accountRef": "account://alpaca/paper"
        }),
    );
    assert_eq!(
        bound["instance"]["accountRef"], "account://alpaca/paper",
        "{bound:#}"
    );
    let workspace = service.handle_http("GET", "/workspace", json!({}));
    let listed = workspace.body["providers"]
        .as_array()
        .expect("workspace providers")
        .iter()
        .find(|provider| provider["instanceRef"] == "alpaca-one")
        .expect("configured plugin instance");
    assert_eq!(listed["accountRef"], "account://alpaca/paper");

    let rejected = request_response(
        &service,
        "PUT",
        "/plugins/instances/alpaca-one/configuration",
        json!({
            "configuration": {},
            "accountRef": "not an account"
        }),
    );
    assert_eq!(rejected.status, 400);
    assert_eq!(rejected.body["error"]["code"], "plugin_account_ref_invalid");

    let first_secret = "secret-first-instance";
    let second_secret = "secret-second-instance";
    let first = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-one/credentials",
        json!({"credentials": {"api_key": "key-first-instance", "api_secret": first_secret}}),
    );
    let second = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-two/credentials",
        json!({"credentials": {"api_key": "key-second-instance", "api_secret": second_secret}}),
    );
    assert_eq!(first["credentialStatus"]["configured"], true, "{first:#}");
    assert_eq!(second["credentialStatus"]["configured"], true, "{second:#}");
    assert_no_secret(&first, &[first_secret, second_secret]);
    assert_no_secret(&second, &[first_secret, second_secret]);

    let health = request(
        &service,
        "POST",
        "/plugins/instances/alpaca-one/health:refresh",
        json!({}),
    );
    assert_eq!(health["instance"]["health"]["state"], "host_unavailable");
    assert_eq!(health["instance"]["health"]["connectivityChecked"], false);

    drop(service);
    let restarted = TradeAssemblyService::test_local(database.to_string_lossy().to_string());
    let first_after = request(
        &restarted,
        "GET",
        "/plugins/instances/alpaca-one",
        json!({}),
    );
    let second_after = request(
        &restarted,
        "GET",
        "/plugins/instances/alpaca-two",
        json!({}),
    );
    assert_eq!(
        first_after["configuration"]["base_url"],
        "https://paper-api.alpaca.markets"
    );
    assert_eq!(first_after["accountRef"], "account://alpaca/paper");
    assert_eq!(
        second_after["configuration"]["base_url"],
        "https://sandbox.example.invalid"
    );
    assert_eq!(first_after["credentialStatus"]["configured"], true);
    assert_eq!(second_after["credentialStatus"]["configured"], true);
    assert_no_secret(&first_after, &[first_secret, second_secret]);
    assert_no_secret(&second_after, &[first_secret, second_secret]);

    let revoked = request(
        &restarted,
        "DELETE",
        "/plugins/instances/alpaca-one/credentials",
        json!({}),
    );
    assert_eq!(revoked["revoked"], true, "{revoked:#}");
    let first_status = request(
        &restarted,
        "GET",
        "/plugins/instances/alpaca-one/credentials",
        json!({}),
    );
    let second_status = request(
        &restarted,
        "GET",
        "/plugins/instances/alpaca-two/credentials",
        json!({}),
    );
    assert_eq!(first_status["credentialStatus"]["configured"], false);
    assert_eq!(second_status["credentialStatus"]["configured"], true);

    let database_bytes = fs::read(&database).expect("runtime database");
    let database_text = String::from_utf8_lossy(&database_bytes);
    assert!(!database_text.contains(first_secret));
    assert!(!database_text.contains(second_secret));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.path().join("credentials/alpaca-two.json");
        let mode = fs::metadata(path)
            .expect("credential metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn configuration_validation_and_disabled_or_removed_states_fail_closed() {
    let directory = tempfile::tempdir().expect("temporary plugin lifecycle directory");
    let service = TradeAssemblyService::test_local(
        directory
            .path()
            .join("runtime.db")
            .to_string_lossy()
            .to_string(),
    );
    install_fixture_manifest(&service);
    let unknown = request_response(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "bad-config",
            "pluginRef": "example.lifecycle-plugin",
            "configuration": {"not_declared": true},
        }),
    );
    assert_eq!(unknown.status, 400);
    assert_eq!(
        unknown.body["error"]["code"],
        "plugin_configuration_invalid"
    );

    let credential_in_config = request_response(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "secret-in-config",
            "pluginRef": "example.lifecycle-plugin",
            "configuration": {"api_key": "must-not-persist"},
        }),
    );
    assert_eq!(credential_in_config.status, 400);
    assert!(!credential_in_config
        .body
        .to_string()
        .contains("must-not-persist"));

    let created = request(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "removable",
            "pluginRef": "example.lifecycle-plugin",
            "configuration": {"base_url": "https://paper-api.alpaca.markets", "account_mode": "paper"},
        }),
    );
    assert_eq!(created["instance"]["enabled"], false);
    assert_eq!(created["instance"]["health"]["state"], "disabled");

    let removed = request(
        &service,
        "DELETE",
        "/plugins/instances/removable",
        json!({}),
    );
    assert_eq!(removed["instance"]["enabled"], false);
    let get_removed = request_response(&service, "GET", "/plugins/instances/removable", json!({}));
    assert_eq!(get_removed.status, 400);
    assert_eq!(
        get_removed.body["error"]["code"],
        "plugin_instance_not_found"
    );
    for (method, path, body) in [
        (
            "POST",
            "/plugins/instances/removable/credentials",
            json!({"credentials": {"api_key": "orphan-key", "api_secret": "orphan-secret"}}),
        ),
        ("GET", "/plugins/instances/removable/credentials", json!({})),
        (
            "DELETE",
            "/plugins/instances/removable/credentials",
            json!({}),
        ),
        (
            "POST",
            "/plugins/instances/removable/health:refresh",
            json!({}),
        ),
    ] {
        let response = request_response(&service, method, path, body);
        assert_eq!(response.status, 400, "{method} {path}: {:#}", response.body);
        assert_eq!(
            response.body["error"]["code"], "plugin_instance_removed",
            "{method} {path}: {:#}",
            response.body
        );
    }
    assert!(!directory.path().join("credentials/removable.json").exists());

    let enabled = request(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "enabled-instance",
            "pluginRef": "example.lifecycle-plugin",
            "enabled": true,
            "configuration": {"base_url": "https://paper-api.alpaca.markets", "account_mode": "paper"},
        }),
    );
    assert_eq!(enabled["instance"]["enabled"], true);
    let enabled_remove = request_response(
        &service,
        "DELETE",
        "/plugins/instances/enabled-instance",
        json!({}),
    );
    assert_eq!(enabled_remove.status, 400, "{:#}", enabled_remove.body);
    assert_eq!(
        enabled_remove.body["error"]["code"],
        "plugin_instance_must_be_disabled"
    );

    let alias_directory = tempfile::tempdir().expect("temporary alias lifecycle directory");
    let alias_service = TradeAssemblyService::test_local(
        alias_directory
            .path()
            .join("runtime.db")
            .to_string_lossy()
            .to_string(),
    );
    install_fixture_manifest(&alias_service);
    request(
        &alias_service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "alpaca-paper",
            "pluginRef": "example.lifecycle-plugin",
            "configuration": {"base_url": "https://paper-api.alpaca.markets", "account_mode": "paper"},
        }),
    );
    let removed_default = request(
        &alias_service,
        "DELETE",
        "/plugins/instances/alpaca-paper",
        json!({}),
    );
    assert_eq!(removed_default["instance"]["enabled"], false);
    assert!(
        !removed_default["instance"]["removedAtMs"].is_null(),
        "{removed_default:#}"
    );
    let stored_removed_default = alias_service
        .runtime()
        .plugins
        .get_instance("alpaca-paper")
        .expect("plugin registry read")
        .expect("removed default tombstone");
    assert!(
        !stored_removed_default["removedAtMs"].is_null(),
        "{stored_removed_default:#}"
    );
    let removed_default_get = request_response(
        &alias_service,
        "GET",
        "/plugins/instances/alpaca-paper",
        json!({}),
    );
    assert_eq!(
        removed_default_get.status, 400,
        "{:#}",
        removed_default_get.body
    );
    for (method, body) in [
        (
            "POST",
            json!({"api_key": "orphan-key", "api_secret": "orphan-secret"}),
        ),
        ("GET", json!({})),
        ("DELETE", json!({})),
    ] {
        let response = request_response(
            &alias_service,
            method,
            "/providers/alpaca-paper/credentials",
            body,
        );
        assert_eq!(response.status, 400, "{method}: {:#}", response.body);
        assert_eq!(
            response.body["error"]["code"], "plugin_instance_removed",
            "{method}: {:#}",
            response.body
        );
    }
    assert!(!alias_directory
        .path()
        .join("credentials/alpaca-paper.json")
        .exists());
}

#[test]
fn manifest_install_is_digest_backed_idempotent_and_atomic_on_rejection() {
    let directory = tempfile::tempdir().expect("temporary plugin lifecycle directory");
    let service = TradeAssemblyService::test_local(
        directory
            .path()
            .join("runtime.db")
            .to_string_lossy()
            .to_string(),
    );
    let manifests = request(&service, "GET", "/plugins/manifests", json!({}));
    let mut manifest = manifests["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
        .expect("simbroker manifest")["manifest"]
        .clone();
    manifest["metadata"]["id"] = json!("example.fixture-plugin");
    manifest["metadata"]["name"] = json!("Fixture Plugin");
    manifest["metadata"]["provider"] = json!({"id": "example", "name": "Example"});
    let dependency_traits = manifest["operations"][2]["traits"].clone();
    manifest["operations"][0]["dependencies"] = json!([{
        "dependencyId": "historical-bars",
        "capability": "market_data.bars.read@1",
        "operationId": "marketdata.bars.read",
        "required": true,
        "traits": dependency_traits,
    }]);
    let manifest_digest = digest(&manifest);
    let commit = "0123456789abcdef0123456789abcdef01234567";
    let source = json!({
        "type": "git",
        "commit": commit,
        "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#example.fixture-plugin")
    });
    let body = json!({
        "manifest": manifest,
        "source": source,
        "integrity": {"manifestSha256": manifest_digest},
        "trust": {"level": "local-test"},
    });
    let installed = request(&service, "POST", "/plugins/manifests", body.clone());
    assert_eq!(installed["ok"], true, "{installed:#}");
    assert_eq!(
        installed["manifest"]["manifest"]["operations"][0]["dependencies"][0]["capability"],
        "market_data.bars.read@1"
    );
    let repeated = request(&service, "POST", "/plugins/manifests", body);
    assert_eq!(repeated["ok"], true, "{repeated:#}");

    let mut invalid = installed["manifest"]["manifest"].clone();
    invalid["metadata"]["id"] = json!("example.rejected-plugin");
    invalid["runtime"]["entrypoint"] = json!("../../escape");
    let rejected = request_response(
        &service,
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": invalid,
            "source": {
                "type": "git",
                "commit": commit,
                "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#example.rejected-plugin")
            },
            "integrity": {"manifestSha256": "sha256:not-the-digest"},
        }),
    );
    assert_eq!(rejected.status, 400);
    let after = request(&service, "GET", "/plugins/manifests", json!({}));
    assert!(after["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .all(|record| record["pluginRef"] != "example.rejected-plugin"));

    let mut schema_invalid = installed["manifest"]["manifest"].clone();
    schema_invalid["metadata"]["id"] = json!("example.schema-invalid");
    schema_invalid
        .as_object_mut()
        .expect("manifest object")
        .remove("permissions");
    let schema_invalid_digest = digest(&schema_invalid);
    let schema_invalid_response = request_response(
        &service,
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": schema_invalid,
            "source": {
                "type": "git",
                "commit": commit,
                "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#example.schema-invalid")
            },
            "integrity": {"manifestSha256": schema_invalid_digest},
        }),
    );
    assert_eq!(schema_invalid_response.status, 400);
    assert_eq!(
        schema_invalid_response.body["error"]["code"],
        "plugin_manifest_invalid"
    );

    let mut fake_core = installed["manifest"]["manifest"].clone();
    fake_core["metadata"]["id"] = json!("example.fake-core");
    fake_core["runtime"]["protocol"] = json!("rust-native");
    fake_core["runtime"]["entrypoint"] = json!("tradeassembly-core");
    let fake_core_digest = digest(&fake_core);
    let fake_core_response = request_response(
        &service,
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": fake_core,
            "source": {"type": "core", "locator": "core://plugins/example.fake-core"},
            "integrity": {"manifestSha256": fake_core_digest},
        }),
    );
    assert_eq!(fake_core_response.status, 400);
    assert_eq!(
        fake_core_response.body["error"]["code"],
        "plugin_manifest_invalid"
    );

    let mut external_native = installed["manifest"]["manifest"].clone();
    external_native["metadata"]["id"] = json!("example.external-native");
    external_native["runtime"]["protocol"] = json!("rust-native");
    external_native["runtime"]["entrypoint"] = json!("tradeassembly-core");
    let external_native_digest = digest(&external_native);
    request(
        &service,
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": external_native,
            "source": {
                "type": "git",
                "commit": commit,
                "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#example.external-native")
            },
            "integrity": {"manifestSha256": external_native_digest},
        }),
    );
    request(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "external-native",
            "pluginRef": "example.external-native",
            "enabled": true,
            "configuration": {},
        }),
    );
    let external_health = request(
        &service,
        "POST",
        "/plugins/instances/external-native/health:refresh",
        json!({}),
    );
    assert_eq!(
        external_health["instance"]["health"]["state"],
        "host_unavailable"
    );
    assert_eq!(
        external_health["instance"]["health"]["connectivityChecked"],
        false
    );
    let unhealthy_invoke = request_response(
        &service,
        "POST",
        "/plugins/instances/external-native/operations/marketdata.bars.read:invoke",
        json!({"capability": "marketdata.bars", "mode": "paper"}),
    );
    assert_eq!(unhealthy_invoke.status, 400);
    assert_eq!(
        unhealthy_invoke.body["error"]["code"],
        "plugin_invocation_blocked"
    );
}

#[test]
fn compatibility_routes_share_instance_state_and_entitlement_overrides_are_inspectable() {
    let directory = tempfile::tempdir().expect("temporary plugin lifecycle directory");
    let service = TradeAssemblyService::test_local(
        directory
            .path()
            .join("runtime.db")
            .to_string_lossy()
            .to_string(),
    );
    install_fixture_manifest(&service);
    request(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "alpaca-paper",
            "pluginRef": "example.lifecycle-plugin",
            "enabled": true,
            "configuration": {"base_url": "https://paper-api.alpaca.markets", "account_mode": "paper"},
        }),
    );

    let disabled = request(
        &service,
        "POST",
        "/product/providers/disable-instance",
        json!({"pluginRef": "example.lifecycle-plugin"}),
    );
    assert_eq!(disabled["body"]["pluginRef"], "example.lifecycle-plugin");
    assert_eq!(disabled["body"]["enabled"], false);
    let canonical = request(
        &service,
        "GET",
        "/plugins/instances/alpaca-paper",
        json!({}),
    );
    assert_eq!(canonical["enabled"], false);
    let invoke = request_response(
        &service,
        "POST",
        "/plugins/instances/alpaca-paper/operations/marketdata.bars.read:invoke",
        json!({"capability": "marketdata.bars", "mode": "paper"}),
    );
    assert_eq!(invoke.status, 400);
    assert_eq!(invoke.body["error"]["code"], "plugin_invocation_blocked");

    let denied = request(
        &service,
        "POST",
        "/entitlements/deny",
        json!({
            "capability": "marketdata.bars",
            "scope": "global",
            "precedence": 250,
            "limits": {"maxRows": 10},
            "reason": "local lifecycle test",
        }),
    );
    let grant_id = denied["grant"]["grantId"].as_str().expect("deny grant id");
    let listed = request(&service, "GET", "/entitlements", json!({}));
    let stored = listed["grants"]
        .as_array()
        .expect("entitlement grants")
        .iter()
        .find(|grant| grant["grantId"] == grant_id)
        .expect("durable deny grant");
    assert_eq!(stored["precedence"], 250);
    assert_eq!(stored["limits"]["maxRows"], 10);

    let revoked = request(
        &service,
        "DELETE",
        &format!("/entitlements/{grant_id}"),
        json!({}),
    );
    assert_eq!(revoked["grant"]["state"], "revoked");
    assert!(revoked["grant"]["revokedAt"].is_string());
}

#[test]
fn package_upgrade_rollback_restart_and_remove_preserve_historical_evidence() {
    let directory = tempfile::tempdir().expect("temporary package lifecycle directory");
    let database = directory.path().join("runtime.db");
    let service = TradeAssemblyService::test_local(database.to_string_lossy().to_string());
    let first = package_fixture(&service, directory.path(), "0.1.0", "first");
    let second = package_fixture(&service, directory.path(), "0.2.0", "second");

    let installed = request(
        &service,
        "POST",
        "/plugins/packages",
        first.install_body("install-first"),
    );
    assert_eq!(installed["package"]["packageSha256"], first.package_sha256);
    let created = request(
        &service,
        "POST",
        "/plugins/instances",
        json!({
            "instanceRef": "package-lifecycle",
            "pluginRef": "example.package-lifecycle",
            "enabled": false,
            "configuration": {},
            "idempotencyKey": "create-package-lifecycle",
        }),
    );
    assert_eq!(
        created["instance"]["activePackageSha256"],
        first.package_sha256
    );

    let historical = json!({
        "kind": "ExecutionRun",
        "executionRunId": "run_package_history",
        "state": "stopped",
        "immutableInput": {
            "capabilityGraphSnapshot": {"nodes": [{"selected": {
                "pluginInstanceRef": "package-lifecycle",
                "manifestFingerprint": first.manifest_sha256,
            }}]},
        },
    });
    service
        .runtime()
        .storage
        .put_json(
            "execution_runs",
            "run_package_history",
            historical.clone(),
            &test_context("package-history"),
        )
        .expect("store historical run");
    let historical_digest = digest(&historical);

    let upgraded = request(
        &service,
        "POST",
        "/plugins/instances/package-lifecycle:upgrade",
        second.install_body("upgrade-second"),
    );
    assert_eq!(
        upgraded["instance"]["activePackageSha256"],
        second.package_sha256
    );
    assert_eq!(
        upgraded["instance"]["previousPackageSha256"],
        first.package_sha256
    );
    assert_eq!(digest(&stored_run(&service)), historical_digest);

    drop(service);
    let restarted = TradeAssemblyService::test_local(database.to_string_lossy().to_string());
    let after_restart = request(
        &restarted,
        "GET",
        "/plugins/instances/package-lifecycle",
        json!({}),
    );
    assert_eq!(after_restart["activePackageSha256"], second.package_sha256);

    let rolled_back = request(
        &restarted,
        "POST",
        "/plugins/instances/package-lifecycle:rollback",
        json!({"idempotencyKey": "rollback-first"}),
    );
    assert_eq!(
        rolled_back["instance"]["activePackageSha256"],
        first.package_sha256
    );
    assert_eq!(
        rolled_back["instance"]["previousPackageSha256"],
        second.package_sha256
    );
    let active_manifest = request(
        &restarted,
        "GET",
        "/plugins/manifests/example.package-lifecycle",
        json!({}),
    );
    assert_eq!(active_manifest["manifestDigest"], first.manifest_sha256);

    let failed_upgrade = request_response(
        &restarted,
        "POST",
        "/plugins/instances/package-lifecycle:upgrade",
        json!({"idempotencyKey": "failed-upgrade"}),
    );
    assert_eq!(failed_upgrade.status, 400, "{:#}", failed_upgrade.body);
    let after_failure = request(
        &restarted,
        "GET",
        "/plugins/instances/package-lifecycle",
        json!({}),
    );
    assert_eq!(after_failure["activePackageSha256"], first.package_sha256);
    assert_eq!(digest(&stored_run(&restarted)), historical_digest);

    let enabled = request(
        &restarted,
        "POST",
        "/plugins/instances/package-lifecycle:enable",
        json!({"idempotencyKey": "enable-package-lifecycle"}),
    );
    assert_eq!(enabled["instance"]["enabled"], true);
    let disabled = request(
        &restarted,
        "POST",
        "/plugins/instances/package-lifecycle:disable",
        json!({"idempotencyKey": "disable-package-lifecycle"}),
    );
    assert_eq!(disabled["instance"]["enabled"], false);
    let removed = request(
        &restarted,
        "DELETE",
        "/plugins/instances/package-lifecycle",
        json!({"idempotencyKey": "remove-package-lifecycle"}),
    );
    assert!(!removed["instance"]["removedAtMs"].is_null());
    let tombstone = restarted
        .runtime()
        .plugins
        .get_instance("package-lifecycle")
        .expect("removed instance read")
        .expect("removed instance tombstone");
    let removed_again = request(
        &restarted,
        "DELETE",
        "/plugins/instances/package-lifecycle",
        json!({"idempotencyKey": "remove-package-lifecycle-again"}),
    );
    assert_eq!(removed_again["idempotent"], true);
    assert_eq!(
        removed_again["instance"]["removedAtMs"],
        tombstone["removedAtMs"]
    );
    assert_eq!(
        restarted
            .runtime()
            .plugins
            .get_instance("package-lifecycle")
            .expect("repeated removal read")
            .expect("retained instance tombstone"),
        tombstone
    );
    assert!(restarted
        .runtime()
        .plugin_packages
        .get(&first.package_sha256)
        .expect("first package read")
        .is_some());
    assert!(restarted
        .runtime()
        .plugin_packages
        .get(&second.package_sha256)
        .expect("second package read")
        .is_some());
    assert_eq!(digest(&stored_run(&restarted)), historical_digest);
}

fn request(service: &TradeAssemblyService, method: &str, path: &str, body: Value) -> Value {
    let response = request_response(service, method, path, body);
    assert_eq!(response.status, 200, "{method} {path}: {:#}", response.body);
    response.body
}

fn request_response(
    service: &TradeAssemblyService,
    method: &str,
    path: &str,
    body: Value,
) -> tradeassembly_runtime::service::ServiceResponse {
    service.handle_http(method, path, body)
}

fn digest(manifest: &Value) -> String {
    let bytes = serde_json_canonicalizer::to_vec(manifest).expect("canonical manifest");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

struct PackageFixture {
    package_sha256: String,
    manifest_sha256: String,
    locator: String,
}

impl PackageFixture {
    fn install_body(&self, idempotency_key: &str) -> Value {
        json!({
            "source": {
                "type": "package",
                "locator": self.locator,
                "package": {"type": "file", "locator": self.locator},
            },
            "integrity": {
                "packageSha256": self.package_sha256,
                "manifestSha256": self.manifest_sha256,
            },
            "trust": {"level": "local-test"},
            "idempotencyKey": idempotency_key,
        })
    }
}

fn package_fixture(
    service: &TradeAssemblyService,
    directory: &Path,
    version: &str,
    marker: &str,
) -> PackageFixture {
    let manifests = request(service, "GET", "/plugins/manifests", json!({}));
    let mut manifest = manifests["manifests"]
        .as_array()
        .expect("manifest records")
        .iter()
        .find(|record| record["pluginRef"] == "tradeassembly.simbroker")
        .expect("simbroker manifest")["manifest"]
        .clone();
    manifest["metadata"]["id"] = json!("example.package-lifecycle");
    manifest["metadata"]["name"] = json!("Package Lifecycle Fixture");
    manifest["metadata"]["version"] = json!(version);
    manifest["metadata"]["provider"] = json!({"id": "example", "name": "Example"});
    manifest["runtime"] = json!({
        "protocol": "stdio",
        "entrypoint": "fixture-plugin",
        "timeoutSeconds": 5,
    });
    let manifest_bytes = serde_yaml::to_string(&manifest)
        .expect("serialize package manifest")
        .into_bytes();
    let manifest_file_sha = format!("{:x}", Sha256::digest(&manifest_bytes));
    let manifest_sha256 = digest(&manifest)
        .strip_prefix("sha256:")
        .expect("manifest digest prefix")
        .to_string();
    let executable = format!("package-lifecycle-{marker}").into_bytes();
    let executable_sha = format!("{:x}", Sha256::digest(&executable));
    let target = tradeassembly_plugin_sdk::host_target();
    let executable_path = format!("bin/{target}/fixture-plugin");
    let descriptor = serde_json::to_vec_pretty(&json!({
        "packageContractVersion": "1",
        "plugin": {"id": "example.package-lifecycle", "version": version},
        "manifest": {"path": "manifest.yaml", "sha256": manifest_file_sha},
        "targets": [{
            "target": target,
            "binary": {"path": executable_path, "sha256": executable_sha},
        }],
        "responseSchemas": [],
        "compatibility": {
            "hostContract": {
                "minimum": tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION,
                "maximum": tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION,
            },
            "sdkVersion": tradeassembly_plugin_sdk::SDK_VERSION,
        },
    }))
    .expect("serialize package descriptor");
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = Builder::new(encoder);
    append_package_file(
        &mut archive,
        "tradeassembly-plugin.json",
        &descriptor,
        0o644,
    );
    append_package_file(&mut archive, "manifest.yaml", &manifest_bytes, 0o644);
    append_package_file(&mut archive, &executable_path, &executable, 0o755);
    archive.finish().expect("finish package archive");
    let package_bytes = archive
        .into_inner()
        .expect("package encoder")
        .finish()
        .expect("compress package archive");
    let package_sha256 = format!("{:x}", Sha256::digest(&package_bytes));
    let package_path = directory.join(format!("package-lifecycle-{version}.tar.gz"));
    fs::File::create(&package_path)
        .expect("create package fixture")
        .write_all(&package_bytes)
        .expect("write package fixture");
    PackageFixture {
        package_sha256,
        manifest_sha256,
        locator: package_path.to_string_lossy().into_owned(),
    }
}

fn append_package_file(
    archive: &mut Builder<GzEncoder<Vec<u8>>>,
    path: &str,
    bytes: &[u8],
    mode: u32,
) {
    let mut header = Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_cksum();
    archive
        .append_data(&mut header, path, bytes)
        .expect("append package fixture");
}

fn stored_run(service: &TradeAssemblyService) -> Value {
    service
        .runtime()
        .storage
        .get_json("execution_runs", "run_package_history")
        .expect("read historical run")
        .expect("historical run")
}

fn test_context(key: &str) -> SideEffectContext {
    SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(key).expect("valid test idempotency key"),
    )
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
    manifest["metadata"]["id"] = json!("example.lifecycle-plugin");
    manifest["metadata"]["name"] = json!("Lifecycle Fixture Plugin");
    manifest["metadata"]["provider"] = json!({"id": "example", "name": "Example"});
    manifest["configuration"] = json!({
        "fields": [
            {
                "id": "api_key",
                "label": "API key",
                "description": "Fixture key.",
                "inputType": "password",
                "required": true,
                "storageClass": "credential"
            },
            {
                "id": "api_secret",
                "label": "API secret",
                "description": "Fixture secret.",
                "inputType": "password",
                "required": true,
                "storageClass": "credential"
            },
            {
                "id": "base_url",
                "label": "API URL",
                "description": "Fixture endpoint.",
                "inputType": "url",
                "required": true,
                "storageClass": "configuration"
            },
            {
                "id": "account_mode",
                "label": "Account mode",
                "description": "Fixture mode.",
                "inputType": "select",
                "required": true,
                "storageClass": "configuration",
                "options": ["paper"]
            }
        ]
    });
    manifest["health"] = json!({
        "requiredConfiguration": ["base_url", "account_mode"],
        "requiredCredentials": ["api_key", "api_secret"],
        "connectionCheckOperation": null
    });
    let manifest_digest = digest(&manifest);
    let commit = "0123456789abcdef0123456789abcdef01234567";
    request(
        service,
        "POST",
        "/plugins/manifests",
        json!({
            "manifest": manifest,
            "source": {
                "type": "git",
                "commit": commit,
                "locator": format!("git+https://example.invalid/plugins.git?ref={commit}#example.lifecycle-plugin")
            },
            "integrity": {"manifestSha256": manifest_digest},
            "trust": {"level": "local-test"}
        }),
    );
}

fn assert_no_secret(value: &Value, secrets: &[&str]) {
    let rendered = value.to_string();
    for secret in secrets {
        assert!(
            !rendered.contains(secret),
            "secret appeared in public response"
        );
    }
}
