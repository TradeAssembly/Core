// Explicit joint gate: real Core CLI + separately owned actual Relay fixture.
// All identity/key/provider facts are synthetic in this private temp fixture.
fn maybe_run_joint_reach_data_process(
    root: &std::path::Path,
    service: &crate::service::TradeAssemblyService,
    command: &crate::service::data_runner::DataRunnerCommand,
    case: &str,
) {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let Some(binary) = std::env::var_os("REACH_DATA_TEST_CORE_BINARY") else {
        return;
    };
    let origin = std::env::var("REACH_DATA_TEST_RELAY_ORIGIN")
        .expect("explicit actual Relay fixture origin");
    let url = url::Url::parse(&origin).unwrap();
    assert_eq!(url.scheme(), "http");
    assert!(
        url.host_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("localhost")
                || s.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback())),
        "never use a real service for this synthetic gate"
    );
    let auth = std::env::var("REACH_DATA_TEST_HUMAN_AUTH").expect("fixture human header");
    assert!(
        auth.starts_with("Bearer fixture"),
        "synthetic authenticator only"
    );
    let expected =
        std::env::var("REACH_DATA_TEST_CORE_REVISION").expect("exact selected Core revision");
    assert_eq!(expected.len(), 40);
    let root = root.canonicalize().unwrap();
    let db = root.join("service.db");
    let owner = crate::local_owner_identity::LocalOwnerIdentity::for_database(&db).unwrap();
    let scoped =
        service.for_authenticated_invocation(&owner.issuer, &owner.stable_identity_id, None, None);
    let scope = match &command.operation {
        crate::service::data_runner::DataRunnerOperation::PrepareSession { schedule } => {
            &schedule.scope
        }
        _ => panic!("use authored calendar command"),
    };
    scoped
        .bind_owned_object("strategy", &scope.strategy_id)
        .unwrap();
    for i in scoped.runtime().plugins.list_instances().unwrap() {
        scoped
            .bind_owned_object("plugin_instance", i["instanceRef"].as_str().unwrap())
            .unwrap();
    }
    fn copy(source: &std::path::Path, dest: &std::path::Path) {
        fs::create_dir_all(dest).unwrap();
        for e in fs::read_dir(source).unwrap() {
            let e = e.unwrap();
            assert!(!e.file_type().unwrap().is_symlink());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &dest.join(e.file_name()));
            } else {
                fs::copy(e.path(), dest.join(e.file_name())).unwrap();
            }
        }
    }
    let artifacts = root.join("reach-artifacts");
    copy(&root.join("package-store"), &artifacts.join("plugins"));
    let state = root.join(".tradeassembly");
    fs::create_dir_all(&state).unwrap();
    let token = state.join("warden.token");
    fs::write(
        &token,
        "fake-reach-authority-token-at-least-thirty-two-bytes",
    )
    .unwrap();
    let key = root.join("fixture-node.seed");
    fs::write(&key, URL_SAFE_NO_PAD.encode([7u8; 32])).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        for f in [&key, &token] {
            fs::set_permissions(f, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let warden = crate::finance_authority::tests::spawn_mock_warden(
        crate::finance_authority::tests::MockBehavior::Allow,
    );
    let run = |flag: Option<&str>, input: Option<Value>| -> std::process::Output {
        let mut child = std::process::Command::new(&binary)
            .args([
                "--data-channel",
                "--core-release",
                "fixture.release",
                "--db",
                db.to_str().unwrap(),
                "--relay-url",
                &origin,
                "--tenant-id",
                "fixture.tenant",
                "--workspace-id",
                "fixture.workspace",
                "--node-id",
                "fixture.node",
                "--actor-ref",
                "fixture.actor",
                "--node-key-file",
                key.to_str().unwrap(),
                "--once",
            ])
            .args(flag)
            .current_dir(&root)
            .env_clear()
            .env("TRADEASSEMBLY_AUTH_PROFILE", "local_owner")
            .env("TRADEASSEMBLY_ARTIFACT_ROOT", &artifacts)
            .env("TRADEASSEMBLY_WARDEN_SIDECAR_URL", &warden.base_url)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = input {
            writeln!(stdin, "{input}").unwrap();
        }
        drop(stdin);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("owned joint Core child exceeded180s; outcome must be reconciled");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    };
    let selected = run(
        Some("--select-source"),
        Some(json!({"requestId":"fixture.source.selection","expectedRevision":0})),
    );
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected: Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(selected["selection"]["coreRevision"], expected);
    assert_eq!(selected["executionAuthorityGranted"], false);
    assert_eq!(
        selected["selection"]["ownerBindingSha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let http = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let response = http
        .post(format!("{origin}/v1/reach/data/source/select"))
        .header("Authorization", &auth)
        .json(&selected["selection"])
        .send()
        .unwrap();
    let status = response.status();
    let selection: Value = response.json().unwrap();
    assert!(
        status.is_success(),
        "source selection {status}: {selection}"
    );
    let revision = selection["revision"].as_u64().unwrap();
    assert!(revision > 0);
    write_screen_interface_evidence(case, "joint-source-selection.json", &selection);
    let mut command = command.clone();
    command.command_id.push_str(".joint");
    command.idempotency_key = IdempotencyKey::new(command.command_id.clone()).unwrap();
    let prepared = run(
        Some("--prepare-command"),
        Some(json!({"kind":"prepare","sourceRevision":revision,"command":command,"ttlMs":300000})),
    );
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(prepared["executionAuthorityGranted"], false);
    assert_eq!(
        prepared["preparation"]["command"]["resource"]["ownerBindingSha256"],
        selected["selection"]["ownerBindingSha256"]
    );
    let before = scoped
        .runtime()
        .storage
        .list_json("data_runner_commands_v1")
        .unwrap()
        .len();
    let operations = scoped
        .runtime()
        .storage
        .list_json("plugin_operation_receipts")
        .unwrap();
    let orders = scoped.runtime().storage.list_json("orders").unwrap();
    let denied = http
        .post(format!("{origin}/v1/reach/data/commands"))
        .header("Authorization", "Bearer fixture-denied")
        .json(&prepared["preparation"])
        .send()
        .unwrap();
    assert_eq!(denied.status().as_u16(), 403);
    assert_eq!(
        scoped
            .runtime()
            .storage
            .list_json("data_runner_commands_v1")
            .unwrap()
            .len(),
        before
    );
    let response = http
        .post(format!("{origin}/v1/reach/data/commands"))
        .header("Authorization", &auth)
        .json(&prepared["preparation"])
        .send()
        .unwrap();
    let status = response.status();
    let admitted: Value = response.json().unwrap();
    assert!(status.is_success(), "admit {status}: {admitted}");
    assert_eq!(admitted["state"], "queued");
    write_screen_interface_evidence(case, "joint-admitted.json", &admitted);
    let executed = run(None, None);
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    let result: Value = serde_json::from_slice(&executed.stdout).unwrap();
    assert_eq!(result["status"], "recorded", "{result}");
    assert_eq!(result["state"], "completed", "{result}");
    write_screen_interface_evidence(case, "joint-core-result.json", &result);
    let status = http
        .get(format!(
            "{origin}/v1/reach/data/commands/fixture.node/{}",
            command.command_id
        ))
        .header("Authorization", &auth)
        .send()
        .unwrap();
    assert!(status.status().is_success());
    let status: Value = status.json().unwrap();
    assert_eq!(status["delivery"]["state"], "completed", "{status}");
    assert_eq!(status["currentness"], "not_checked");
    assert_eq!(status["executionAuthorityGranted"], false);
    write_screen_interface_evidence(case, "joint-historical-status.json", &status);
    let commands = scoped
        .runtime()
        .storage
        .list_json("control_plane_commands")
        .unwrap();
    let second = run(None, None);
    assert!(second.status.success());
    let second: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second["status"], "idle");
    assert_eq!(
        scoped
            .runtime()
            .storage
            .list_json("control_plane_commands")
            .unwrap(),
        commands
    );
    assert_eq!(
        scoped
            .runtime()
            .storage
            .list_json("plugin_operation_receipts")
            .unwrap(),
        operations
    );
    assert_eq!(
        scoped.runtime().storage.list_json("orders").unwrap(),
        orders
    );
    let owner_path =
        std::path::PathBuf::from(format!("{}.local-owner", db.display())).join("local-owner.json");
    let backup = owner_path.with_extension("fixture-preserved");
    fs::rename(&owner_path, &backup).unwrap();
    let refusal = run(Some("--describe-source"), None);
    assert!(!refusal.status.success());
    assert!(
        !owner_path.exists(),
        "consumer must not regenerate a missing identity"
    );
    fs::rename(&backup, &owner_path).unwrap();
    let refusal: Value = serde_json::from_slice(&refusal.stderr).unwrap();
    assert_eq!(refusal["code"], "reach_data_current_identity_unavailable");
    write_screen_interface_evidence(case, "joint-missing-identity-refusal.json", &refusal);
    write_screen_interface_evidence(
        case,
        "joint-proof-scope.json",
        &json!({"coreRevision":expected,"sourceRegistration":"actual Core existing synthetic identity + existing fixture key","relay":"actual separate local Relay fixture; human/installation authority is synthetic","provider":"authored native calendar HTTP fixture","warden":"mocked existing HTTP authority adapter","brokerOrders":false,"extraProviderAcquisition":false,"wholeResponseLossRestart":"not exercised by this positive case"}),
    );
}
