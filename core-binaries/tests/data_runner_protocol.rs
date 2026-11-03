//! Process wire compatibility only; no market data, policy bypass or orders.
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};
#[test]
fn data_only_process_has_bounded_wire_and_refuses_legacy_execution() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".tradeassembly");
    std::fs::create_dir(&state).unwrap();
    let token = state.join("warden.token");
    std::fs::write(&token, "fake-data-runner-token-at-least-thirty-two-bytes").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .args([
            "--data-only",
            "--db",
            root.path().join("runner.db").to_str().unwrap(),
            "--core-release",
            "fixture.release",
        ])
        .current_dir(root.path())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let command = json!({"schemaVersion":"tradeassembly.core-data-runner.v1","coreRelease":"fixture.release","workspaceId":"fixture.workspace","commandId":"fixture.command","authorityRef":"provenance.only","idempotencyKey":"fixture.command","mode":"paper","submittedAtMs":1000,"operation":{"kind":"prepare_session","schedule":{"scope":{"strategyId":"fixture.missing","strategyVersionId":"fixture.missing","strategySpecHash":"sha256:fixture"},"capabilityGraphRevision":"fixture.graph","mode":"paper","triggerRef":"preopen","sessionDate":"2026-10-31","calendarCollectionId":"fixture.calendar","executionCalendarCollectionId":"fixture.calendar","knowledgeCutoffMs":1000}}});
    let mut live = command.clone();
    live["mode"] = json!("live");
    let mut secret = command.clone();
    secret["credentials"] = json!({"api_key":"fake-key-must-not-echo"});
    let mut large = command.clone();
    large["authorityRef"] = json!("X".repeat(270000));
    let messages = [
        json!({"operation":"dispatch_data","command":live}),
        json!({"operation":"dispatch_data","command":secret}),
        json!({"operation":"dispatch_data","command":large}),
        json!({"operation":"dispatch_data","command":command}),
        json!({"operation":"data_receipt","workspace_id":"fixture.workspace","command_id":"missing"}),
        json!({"operation":"dispatch_paper","command":{"schema":"tradeassembly.core-paper-runner.v1","bridgeVersion":"1","coreRelease":"fixture.release","workspaceId":"fixture.workspace","commandId":"paper.fixture","authorityRef":"provenance.only","idempotencyKey":"paper.fixture","submittedAtMs":1000,"operation":{"kind":"evaluate_strategy","step":{}}}}),
    ];
    let mut input = child.stdin.take().unwrap();
    for m in messages {
        writeln!(input, "{m}").unwrap();
    }
    drop(input);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let raw = String::from_utf8(out.stdout).unwrap();
    assert!(!raw.contains("fake-key-must-not-echo"));
    assert!(raw.len() < 16384);
    let lines = raw
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 6, "{raw}");
    for i in [0, 1, 2, 3, 5] {
        assert_eq!(lines[i]["ok"], false, "{raw}");
        assert!(lines[i].get("receipt").is_none(), "{raw}");
    }
    assert_eq!(lines[0]["error"]["code"], "data_runner_command_invalid");
    assert_eq!(
        lines[1]["error"]["code"],
        "data_runner_credential_or_execution_material_forbidden"
    );
    assert_eq!(
        lines[2]["error"]["code"],
        "data_runner_command_bounds_invalid"
    );
    assert_eq!(lines[4], json!({"ok":true,"receipt":null}));
}
