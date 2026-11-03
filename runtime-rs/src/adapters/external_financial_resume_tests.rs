// Restore only the mock authority; Core and broker state remain the original.
#[test]
#[ignore = "explicit preserved synthetic root and read-only reconciliation gate"]
fn financial_v4_resume_preserved_lookup_only() {
    let file = std::env::var("FINANCIAL_V4_LOOKUP_CONFIG").unwrap();
    let mut request: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    let root = std::path::Path::new(request["root"].as_str().unwrap());
    assert_eq!(fs::read_to_string(root.join("SYNTHETIC_FINANCIAL_REMOTE_FIXTURE")).unwrap(), "synthetic only\n");
    let warden = crate::finance_authority::tests::spawn_mock_warden(crate::finance_authority::tests::MockBehavior::Allow);
    request["wardenOrigin"] = json!(warden.base_url);
    let mut child = std::process::Command::new("python3")
        .arg(std::env::var("FINANCIAL_V4_LOOKUP_DRIVER").unwrap())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped()).spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "preserved lookup driver: {}", String::from_utf8_lossy(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["syntheticSubmits"], 1);
    assert_eq!(result["realBrokerCalls"], 0);
    write_screen_interface_evidence("preserved_lookup", "checkpoint.json", &result);
}
