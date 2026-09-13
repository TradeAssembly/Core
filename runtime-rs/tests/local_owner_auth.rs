use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};
use tradeassembly_runtime::cli_identity::CliIdentityManager;
use tradeassembly_runtime::runtime_config::RuntimeConfig;

#[tokio::test]
async fn explicit_local_owner_is_stable_without_hosted_configuration() {
    let root = tempfile::tempdir().unwrap();
    let mut config = RuntimeConfig::local(
        root.path()
            .canonicalize()
            .unwrap()
            .join("runtime.db")
            .display()
            .to_string(),
    );
    config.oidc_profile = "local_owner".to_string();
    let manager = CliIdentityManager::new(config.clone());
    let first = manager.current_identity().await.unwrap();
    manager.logout().unwrap();
    let second = CliIdentityManager::new(config.clone())
        .login()
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.issuer, "local-owner");
    assert_eq!(first.assurance.as_deref(), Some("local-process"));
    config.oidc_client_id = "existing-hosted-client".to_string();
    assert!(
        config.validate().is_err(),
        "never mix local and hosted identity silently"
    );
}

#[test]
fn explicit_local_owner_mcp_can_list_strategies_without_network_signin() {
    let root = tempfile::tempdir().unwrap();
    run_mcp(root.path(), true, true);
}

#[test]
fn fresh_default_and_restart_use_local_owner_without_auth_configuration() {
    let root = tempfile::tempdir().unwrap();
    run_mcp(root.path(), false, true);
    let record = root
        .path()
        .join(".tradeassembly/tradeassembly.db.local-owner/local-owner.json");
    let original = std::fs::read(&record).unwrap();
    run_mcp(root.path(), false, true);
    assert_eq!(std::fs::read(&record).unwrap(), original);
}

#[test]
fn existing_database_without_owner_record_does_not_switch_identity() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".tradeassembly");
    std::fs::create_dir(&state).unwrap();
    // Create a current-schema fixture, not an arbitrary legacy schema that
    // would fail initialization before identity selection can be observed.
    let service = tradeassembly_runtime::service::TradeAssemblyService::test_local(
        state.join("tradeassembly.db").display().to_string(),
    );
    drop(service);
    let output = Command::new(env!("CARGO_BIN_EXE_tradeassembly"))
        .current_dir(root.path())
        .env_clear()
        .env(
            "TRADEASSEMBLY_WARDEN_TOKEN_REF",
            "env://F2_TEST_WARDEN_TOKEN",
        )
        .env(
            "F2_TEST_WARDEN_TOKEN",
            "local-owner-auth-fixture-token-32-bytes",
        )
        .args(["auth", "status"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["authenticated"], false);
    assert!(!state.join("tradeassembly.db.local-owner").exists());
}

#[test]
fn local_missing_runtime_dependency_requests_setup_not_hosted_signin() {
    let root = tempfile::tempdir().unwrap();
    run_mcp(root.path(), false, false);
}

fn run_mcp(root: &std::path::Path, explicit: bool, with_policy_token: bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tradeassembly"));
    command
        .current_dir(root)
        .env_clear()
        // Read-only listing still constructs the policy adapter. Supply a
        // fixture token, not a running policy service or an owner's credential.
        .env(
            "TRADEASSEMBLY_WARDEN_TOKEN_REF",
            "env://F2_TEST_WARDEN_TOKEN",
        )
        .env(
            "F2_TEST_WARDEN_TOKEN",
            "local-owner-auth-fixture-token-32-bytes",
        )
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if explicit {
        command.env("TRADEASSEMBLY_AUTH_PROFILE", "local_owner");
    }
    if !with_policy_token {
        command.env_remove("TRADEASSEMBLY_WARDEN_TOKEN_REF");
        command.env_remove("F2_TEST_WARDEN_TOKEN");
    }
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tradeassembly.strategy.list","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tradeassembly.setup.inspect","arguments":{"surface":"agent"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tradeassembly.setup.inspect","arguments":{"surface":"unknown"}}}),
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let messages: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(messages.len(), 4);
    let setup = &messages[2]["result"]["structuredContent"];
    assert_eq!(setup["acceptance"]["requestedSurface"], "agent", "{setup}");
    assert_eq!(
        setup["acceptance"]["studioParity"]["status"],
        "not_requested"
    );
    assert_eq!(setup["acceptance"]["complete"], false);
    assert_eq!(
        messages[3]["result"]["structuredContent"]["error"]["code"],
        "invalid_surface"
    );
    if !with_policy_token {
        assert_eq!(
            messages[1]["result"]["structuredContent"]["error"]["code"],
            "setup_required"
        );
        assert!(!messages[1].to_string().contains("oidc_session_required"));
        return;
    }
    assert!(messages[1].get("error").is_none(), "{}", messages[1]);
    assert_ne!(messages[1]["result"]["isError"], true, "{}", messages[1]);
    assert_eq!(messages[1]["result"]["structuredContent"]["ok"], true);
    assert!(messages[1]["result"]["structuredContent"]["strategies"].is_array());
    assert!(!messages[1].to_string().contains("oidc_session_required"));
}
