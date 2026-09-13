// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn installed_binary_explicit_hosted_profile_reports_missing_config_without_source() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("tradeassembly");
    std::fs::copy(env!("CARGO_BIN_EXE_tradeassembly"), &binary).unwrap();
    let mut child = Command::new(&binary)
        .current_dir(root.path())
        .env_clear()
        .env("TRADEASSEMBLY_AUTH_PROFILE", "workos")
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"tradeassembly.account.login","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tradeassembly.strategy.list","arguments":{}}}),
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
    assert_eq!(messages[0]["result"]["serverInfo"]["name"], "tradeassembly");
    assert!(messages[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["inputSchema"]["type"] == "object"));
    assert!(messages[2]
        .to_string()
        .contains("workos_configuration_required"));
    assert!(!messages[2].to_string().contains("\"authenticated\":true"));
    assert!(messages[3].to_string().contains("oidc_session_required"));
    assert!(!root.path().join("runtime-rs").exists());
}
