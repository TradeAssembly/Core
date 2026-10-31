// Opt-in bootstrap only. Financial effects must run through frozen Core/SRT.
fn prepare_financial_joint_driver(
    s: &crate::service::TradeAssemblyService,
    root: &std::path::Path,
    scope: &data_plans::PlanScope,
    graph: &str,
    prepared: &Value,
    signal: &Value,
    warden_origin: &str,
) {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    fn copy(source: &std::path::Path, target: &std::path::Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            assert!(!entry.file_type().unwrap().is_symlink());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
    fs::write(root.join("SYNTHETIC_FINANCIAL_REMOTE_FIXTURE"), "synthetic only\n").unwrap();
    copy(&root.join("package-store"), &root.join("reach-artifacts/plugins"));
    let state = root.join(".tradeassembly");
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("warden.token"), "fake-financial-fixture-token-at-least-thirty-two-bytes").unwrap();
    let key = root.join("fixture-node.seed");
    fs::write(&key, URL_SAFE_NO_PAD.encode([7u8; 32])).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        for path in [&key, &state.join("warden.token")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let request = json!({"root":root,"scope":scope,"graph":graph,"prepared":prepared,"signal":signal,"wardenOrigin":warden_origin,"brokerOrigin":std::env::var("FINANCIAL_V4_BROKER_ORIGIN").unwrap(),"qualification":"authored bootstrap only; driver must use frozen Core + enforcing SRT"});
    write_screen_interface_evidence("financial_joint", "bootstrap.json", &request);
    let script = std::env::var("FINANCIAL_V4_JOINT_DRIVER").unwrap();
    assert!(std::path::Path::new(&script).is_absolute());
    let mut child = std::process::Command::new("python3").arg(script).arg("--drive")
        .current_dir(root).stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    fs::write(root.join("financial-driver.stdout.log"), &output.stdout).unwrap();
    fs::write(root.join("financial-driver.stderr.log"), &output.stderr).unwrap();
    assert!(output.status.success(), "financial joint failed, retained root {}: {}", root.display(), String::from_utf8_lossy(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "PASS");
    assert_eq!(s.db(), root.join("service.db").to_str().unwrap());
    write_screen_interface_evidence("financial_joint", "result.json", &result);
}

#[test]
#[ignore = "explicit both-ready financial Relay fixture and enforcing SRT gate"]
fn financial_v4_actual_core_relay_http_broker_joint() {
    for key in ["FINANCIAL_V4_JOINT_DRIVER", "FINANCIAL_V4_BROKER_ORIGIN", "FINANCIAL_V4_JOINT_CONFIG", "FINANCIAL_V4_JOINT_READY"] {
        assert!(std::env::var_os(key).is_some(), "missing joint input {key}");
    }
    assert_eq!(std::env::var("FINANCIAL_V4_JOINT_READY").unwrap(), "yes");
    crate::build_identity::install("23f6418908e4059b9ae0f8dd29955bfeecb02046").unwrap();
    native_execution_plan_fixture(Some("financial_joint"));
}
