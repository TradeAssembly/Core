// Opt-in qualification of exact frozen Core executables, synthetic state only.
fn exercise_compiled_session_process(
    service: &crate::service::TradeAssemblyService,
    job: &crate::service::session_dispatch::DispatchRequest,
    case: &str,
) {
    fn pin(name: &str, sha_name: &str) -> (std::path::PathBuf, String) {
        let supplied =
            std::path::PathBuf::from(std::env::var(name).expect("explicit frozen fixture path"));
        assert!(supplied.is_absolute());
        let path = supplied.canonicalize().unwrap();
        assert!(path.is_file());
        let sha = std::env::var(sha_name).expect("explicit frozen fixture SHA256");
        assert!(
            sha.len() == 64
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(
            sha256(&fs::read(&path).unwrap()),
            sha,
            "frozen bytes changed: {name}"
        );
        (path, sha)
    }
    fn protected_file(path: &std::path::Path, bytes: &[u8]) -> std::fs::File {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        file
    }
    fn copy(source: &std::path::Path, dest: &std::path::Path) {
        fs::create_dir_all(dest).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            assert!(!entry.file_type().unwrap().is_symlink());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &dest.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), dest.join(entry.file_name())).unwrap();
            }
        }
    }
    let (binary, binary_sha) = pin(
        "SESSION_PROCESS_TEST_BINARY",
        "SESSION_PROCESS_TEST_BINARY_SHA256",
    );
    let (driver, driver_sha) = pin(
        "SESSION_PROCESS_TEST_DRIVER",
        "SESSION_PROCESS_TEST_DRIVER_SHA256",
    );
    let (srt, srt_sha) = pin(
        "SESSION_PROCESS_TEST_SRT",
        "SESSION_PROCESS_TEST_SRT_SHA256",
    );
    let revision =
        std::env::var("SESSION_PROCESS_TEST_CORE_REVISION").expect("exact frozen Core revision");
    assert!(
        revision.len() == 40
            && revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(job.mode, "paper");
    assert!(job.execution_activation_id.is_none());
    let root = std::path::Path::new(service.db())
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    drop(protected_file(
        &root.join("SYNTHETIC_SESSION_PROCESS_FIXTURE"),
        b"synthetic only\n",
    ));
    copy(
        &root.join("package-store"),
        &root.join("process-artifacts/plugins"),
    );
    let state = root.join(".tradeassembly");
    fs::create_dir_all(&state).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    }
    drop(protected_file(
        &state.join("warden.token"),
        b"fake-reach-authority-token-at-least-thirty-two-bytes",
    ));
    let warden = crate::finance_authority::tests::spawn_mock_warden(
        crate::finance_authority::tests::MockBehavior::Allow,
    );
    let evidence_root = std::path::PathBuf::from(
        std::env::var("SCREEN_INTERFACE_EVIDENCE_DIR").expect("private evidence root"),
    );
    fs::create_dir_all(&evidence_root).unwrap();
    let evidence = evidence_root
        .canonicalize()
        .unwrap()
        .join(case)
        .join("compiled-process");
    let input = json!({"root":root,"binary":binary,"binarySha256":binary_sha,"revision":revision,"job":job,"evidence":evidence,"wardenOrigin":warden.base_url,"srt":srt,"srtSha256":srt_sha});
    write_screen_interface_evidence(case, "compiled-process-request.json", &input);
    let bytes = serde_json::to_vec(&input).unwrap();
    assert!(bytes.len() <= 1_048_576);
    let request_path = state.join("session-process-driver-request.json");
    drop(protected_file(&request_path, &bytes));
    let stdout_path = evidence
        .parent()
        .unwrap()
        .join("compiled-process-driver.stdout.json");
    let stderr_path = evidence
        .parent()
        .unwrap()
        .join("compiled-process-driver.stderr.log");
    let mut child = std::process::Command::new(driver)
        .current_dir(&root)
        .stdin(std::process::Stdio::from(
            std::fs::File::open(request_path).unwrap(),
        ))
        .stdout(std::process::Stdio::from(protected_file(&stdout_path, b"")))
        .stderr(std::process::Stdio::from(protected_file(&stderr_path, b"")))
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let cleanup_deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while child.try_wait().unwrap().is_none() {
                assert!(
                    std::time::Instant::now() < cleanup_deadline,
                    "frozen driver kill/join uncertain; inspect retained synthetic state"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("bounded frozen session driver timed out; inspect retained synthetic state");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert!(fs::metadata(&stdout_path).unwrap().len() <= 1_048_576);
    assert!(fs::metadata(&stderr_path).unwrap().len() <= 1_048_576);
    assert!(
        status.success(),
        "compiled process gate failed: {}",
        String::from_utf8_lossy(&fs::read(&stderr_path).unwrap())
    );
    let result: Value = serde_json::from_slice(&fs::read(stdout_path).unwrap()).unwrap();
    assert_eq!(result["status"], "PASS", "{result}");
    write_screen_interface_evidence(
        case,
        "compiled-process-driver-result.json",
        &json!({"driverSha256":driver_sha,"result":result}),
    );
}
