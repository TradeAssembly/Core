//! Fixture transport proof only; not Warden or authorization-boundary proof.
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const ISOLATION_MARKER: &str = ".f2-controlled-broker-fixture";
const LOSE_RESPONSE_MARKER: &str = ".f2-controlled-broker-lose-response";

fn invoke(root: &std::path::Path, operation: &str, quantity: &str) -> Output {
    let binary = std::env::var("F2_TEST_CONTROLLED_BROKER_BINARY")
        .expect("explicit built controlled-broker binary required");
    let binary = std::fs::canonicalize(binary).expect("fixture binary must exist");
    let mut child = Command::new(binary)
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn actual fixture process");
    let request = json!({
        "schemaVersion":"1", "hostContractVersion":"1", "sdkVersion":"0.1.0",
        "requestId":"fixture-request", "metadata":{
            "operationId":operation,"pluginId":"tradeassembly.f2-controlled-broker",
            "pluginInstanceId":"fixture","packageDigest":"fixture-package",
            "manifestDigest":"fixture-manifest","capabilityBindingId":"fixture-binding",
            "capabilityFingerprint":"fixture-fingerprint"
        },
        "context":{"authorityId":"fixture-only","purpose":"controlled-proof","mode":"live",
            "fencingToken":"1","timeoutMs":1000,"idempotencyKey":"fixture-order"},
        "payload":{"kind":"brokerOrder","payload":{
            "symbol":"FIXTURE","side":"buy","orderType":"market",
            "timeInForce":"day","quantity":quantity,"clientOrderId":"fixture-order"
        }}
    });
    let mut stdin = child.stdin.take().unwrap();
    // Missing-marker test may exit before consuming input.
    let _ = writeln!(stdin, "{request}");
    drop(stdin);
    child.wait_with_output().expect("wait fixture")
}

#[test]
#[ignore = "requires explicitly built F2_TEST_CONTROLLED_BROKER_BINARY"]
fn lost_response_reopens_durable_sink_without_resubmission() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(ISOLATION_MARKER), b"test-only").unwrap();
    std::fs::write(root.path().join(LOSE_RESPONSE_MARKER), b"test-only").unwrap();
    let lost = invoke(root.path(), "broker.order_submit", "1");
    assert_eq!(lost.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(lost.stderr).unwrap().trim(),
        "controlled_response_lost"
    );
    assert!(lost.stdout.is_empty());
    std::fs::remove_file(root.path().join(LOSE_RESPONSE_MARKER)).unwrap();
    let recovered = invoke(root.path(), "broker.order_lookup", "999.000000");
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let response: Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert_eq!(response["payload"]["submissionCount"], 1);
    assert_eq!(response["payload"]["clientOrderId"], "fixture-order");
    assert_eq!(response["payload"]["providerOrderId"], "fixture-order");
    assert_eq!(
        response["payload"]["accountRef"],
        "account://mandate-live/controlled"
    );
    assert_eq!(response["payload"]["symbol"], "FIXTURE");
    assert_eq!(response["payload"]["side"], "buy");
    assert_eq!(response["payload"]["quantity"], "1");
    assert_eq!(response["payload"]["orderType"], "market");
    assert_eq!(response["payload"]["timeInForce"], "day");
    assert_eq!(response["reconciliation"], "reconciled");
}

#[test]
#[ignore = "requires explicitly built F2_TEST_CONTROLLED_BROKER_BINARY"]
fn sink_counts_duplicate_and_conflicting_submissions() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(ISOLATION_MARKER), b"test-only").unwrap();
    assert!(invoke(root.path(), "broker.order_submit", "1")
        .status
        .success());
    assert!(invoke(root.path(), "broker.order_submit", "1")
        .status
        .success());
    let conflict = invoke(root.path(), "broker.order_submit", "2");
    assert_eq!(conflict.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(conflict.stderr).unwrap().trim(),
        "controlled_idempotency_conflict"
    );
    let found = invoke(root.path(), "broker.order_lookup", "1");
    assert!(found.status.success());
    let response: Value = serde_json::from_slice(&found.stdout).unwrap();
    assert_eq!(response["payload"]["submissionCount"], 3);
}

#[test]
#[ignore = "requires explicitly built F2_TEST_CONTROLLED_BROKER_BINARY"]
fn unknown_lookup_does_not_create_state() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(ISOLATION_MARKER), b"test-only").unwrap();
    let output = invoke(root.path(), "broker.order_lookup", "1");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "controlled_order_not_found"
    );
    assert!(!root.path().join(".f2-controlled-broker.sqlite").exists());
}

#[test]
#[ignore = "requires explicitly built F2_TEST_CONTROLLED_BROKER_BINARY"]
fn missing_isolation_marker_refuses_to_create_state() {
    let root = tempfile::tempdir().unwrap();
    let output = invoke(root.path(), "broker.order_submit", "1");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "controlled_fixture_marker_required"
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
