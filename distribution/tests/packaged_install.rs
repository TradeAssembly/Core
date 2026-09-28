// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Real-binary acceptance, opt-in because it needs the frozen package artifact.
//! No strategy activation, brokerage traffic, or customer state is permitted.

use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tradeassembly_distribution::{atomic_json, digest, executable, read_json, Release};

struct Policy(Child);
impl Drop for Policy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn invoke(package: &Path, root: &Path, command: &str, port: u16) -> (bool, Value) {
    let output = Command::new(package.join(executable("tradeassembly-distribution")))
        .args([command, "--root"])
        .arg(root)
        .args(["--warden-port", &port.to_string()])
        .output()
        .expect("installer starts");
    let bytes = if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    };
    let value = serde_json::from_slice(bytes).expect("structured installer result");
    (output.status.success(), value)
}

#[test]
#[ignore = "requires explicit frozen native package; invokes actual Core and Warden"]
fn frozen_package_setup_reinstall_upgrade_rollback_and_running_denial() {
    let package = std::env::var_os("TRADEASSEMBLY_DISTRIBUTION_PACKAGE")
        .expect("explicit native package required");
    let package = Path::new(&package);
    let original: Release = read_json(&package.join("release.json")).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("tradeassembly-distribution-acceptance-")
        .tempdir()
        .unwrap();
    let root = temp.path().join("owned rig with spaces");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (success, result) = invoke(package, &root, "install", port);
    assert!(success, "install: {result}");
    let root = root.canonicalize().unwrap();
    assert_eq!(result["payloadSha256"], original.archive_sha256);
    assert!(root.join("state/local/runtime.db").is_file());
    let secrets = ["signing.seed", "warden.token", "runtime.db.local-owner"];
    let fingerprints: Vec<_> = secrets
        .iter()
        .filter_map(|name| {
            let path = root.join("state/local").join(name);
            path.is_file()
                .then(|| (path.clone(), digest(&path).unwrap()))
        })
        .collect();
    assert!(fingerprints.len() >= 2);
    let (success, result) = invoke(package, &root, "install", port);
    assert!(success, "idempotent reinstall: {result}");

    // New npm release, same frozen runtime and schema. Not proof of arbitrary
    // future state migration; those releases need separate qualification.
    let next_package = temp.path().join("next-package");
    fs::create_dir(&next_package).unwrap();
    for name in [
        executable("tradeassembly-distribution"),
        "bundle.tar.gz".into(),
    ] {
        fs::hard_link(package.join(&name), next_package.join(name)).unwrap();
    }
    let mut next = original.clone();
    next.version = "0.1.0-beta.2".into();
    atomic_json(&next_package.join("release.json"), &next).unwrap();

    let payload = root.join("versions").join(&original.archive_sha256);
    let process = Command::new(payload.join(executable("bin/tradeassembly")))
        .args(["policy", "serve", "--state-dir"])
        .arg(root.join("state/local"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut policy = Policy(process);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            policy.0.try_wait().unwrap().is_none(),
            "owned policy stays running"
        );
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "owned policy startup");
        std::thread::sleep(Duration::from_millis(50));
    }
    let (success, result) = invoke(&next_package, &root, "upgrade", port);
    assert!(!success);
    assert_eq!(
        result["code"],
        "rig_process_running_stop_owned_services_first"
    );
    drop(policy);

    // Isolated persisted active state must block even with no live worker.
    let db = rusqlite::Connection::open(root.join("state/local/runtime.db")).unwrap();
    db.execute("INSERT INTO tradeassembly_kv(namespace,item_key,value_json,idempotency_key,authority_actor,updated_at_ms) VALUES('execution_activations','distribution-negative-test',?1,'distribution-negative-test','local-user',1)", [json!({"state":"active"}).to_string()]).unwrap();
    let (success, result) = invoke(&next_package, &root, "upgrade", port);
    assert!(!success);
    assert_eq!(result["code"], "rig_has_active_or_unknown_execution_state");
    db.execute(
        "UPDATE tradeassembly_kv SET value_json=?1 WHERE item_key='distribution-negative-test'",
        [json!({"state":"stopped"}).to_string()],
    )
    .unwrap();
    // Real partial transaction: configuration preparation completes, but an
    // unrelated listener prevents the authorized registration sidecar starting.
    let occupied = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let (success, result) = invoke(&next_package, &root, "upgrade", port);
    assert!(!success);
    assert_eq!(result["code"], "warden_port_in_use");
    let still_current: Value = read_json(&root.join("installed.json")).unwrap();
    assert_eq!(still_current["current"]["version"], original.version);
    assert!(root.join("pending.json").exists());
    let dispatch = Command::new(root.join(executable("bin/tradeassembly")))
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        !dispatch.status.success(),
        "pending transaction denies runtime dispatch"
    );
    drop(occupied);
    let (success, result) = invoke(&next_package, &root, "upgrade", port);
    assert!(success, "compatible upgrade recovery: {result}");
    assert_eq!(result["version"], next.version);
    let occupied = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let (success, result) = invoke(&next_package, &root, "rollback", port);
    assert!(!success);
    assert_eq!(result["code"], "warden_port_in_use");
    drop(occupied);
    let (success, result) = invoke(&next_package, &root, "rollback", port);
    assert!(success, "rollback: {result}");
    assert_eq!(result["version"], original.version);
    for (path, hash) in fingerprints {
        assert_eq!(digest(&path).unwrap(), hash, "credential binding preserved");
    }
    assert_eq!(
        digest(&root.join(executable("authority/bin/warden"))).unwrap(),
        original.warden_sha256
    );
    assert_eq!(
        db.query_row(
            "SELECT value_json FROM tradeassembly_kv WHERE item_key='distribution-negative-test'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        json!({"state":"stopped"}).to_string()
    );
    let output = Command::new(root.join(executable("bin/tradeassembly")))
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stable facade works outside npm cache"
    );
    assert!(!root.join("pending.json").exists());
}
