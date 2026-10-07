use serde_json::Value;
use std::process::Command;

#[test]
fn core_runner_reports_exact_build_identity_without_runtime_state() {
    let output = Command::new(env!("CARGO_BIN_EXE_tradeassembly-core-runner"))
        .arg("--build-identity")
        .env_clear()
        .output()
        .expect("run Core build identity probe");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("strict JSON identity");
    assert_eq!(value.as_object().map(|object| object.len()), Some(2));
    assert_eq!(
        value["schemaVersion"],
        "tradeassembly.core-runner.build-identity/v1"
    );
    let revision = value["coreRevision"].as_str().expect("Core revision");
    assert_eq!(revision.len(), 40);
    assert!(revision
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
}
