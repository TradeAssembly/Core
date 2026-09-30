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
    let parsed = semver::Version::parse(&original.version).unwrap();
    next.version = format!(
        "{}.{}.{}-{}.next",
        parsed.major, parsed.minor, parsed.patch, parsed.pre
    );
    if original.schema_version == 2 {
        let mut descriptor: Value =
            read_json(&package.join(tradeassembly_distribution::candidate::NAME)).unwrap();
        descriptor["version"] = json!(next.version);
        let path = next_package.join(tradeassembly_distribution::candidate::NAME);
        atomic_json(&path, &descriptor).unwrap();
        next.candidate_descriptor_sha256 = Some(digest(&path).unwrap());
    }
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

    let config_path = root.join("state/local/runtime.json");
    let mut config: Value = read_json(&config_path).unwrap();
    let owner_scopes = json!("openid profile cohort_custom_scope");
    config["oidcScopes"] = owner_scopes.clone();
    atomic_json(&config_path, &config).unwrap();

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
    let preserved: Value = read_json(&config_path).unwrap();
    assert_eq!(
        preserved["oidcScopes"], owner_scopes,
        "owner connection settings survive upgrade and rollback"
    );
    let mut external = preserved.clone();
    external["postgresUrlRef"] = json!("file:///unavailable-isolated-test-reference");
    atomic_json(&config_path, &external).unwrap();
    let (success, result) = invoke(&next_package, &root, "upgrade", port);
    assert!(!success);
    assert_eq!(
        result["code"],
        "upgrade_external_state_requires_qualified_migration"
    );
    atomic_json(&config_path, &preserved).unwrap();
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

#[test]
#[ignore = "requires explicit frozen baseline and replacement Mac packages"]
fn frozen_baseline_upgrades_to_replacement_and_rolls_back_without_state_loss() {
    let baseline = std::env::var_os("TRADEASSEMBLY_DISTRIBUTION_BASELINE_PACKAGE")
        .expect("explicit frozen baseline package required");
    let replacement = std::env::var_os("TRADEASSEMBLY_DISTRIBUTION_REPLACEMENT_PACKAGE")
        .expect("explicit replacement package required");
    let baseline = Path::new(&baseline);
    let replacement = Path::new(&replacement);
    let old: Release = read_json(&baseline.join("release.json")).unwrap();
    let new: Release = read_json(&replacement.join("release.json")).unwrap();
    assert_eq!(old.schema_version, 1);
    assert_eq!(new.schema_version, 2);
    assert_ne!(old.archive_sha256, new.archive_sha256);
    assert_eq!(old.warden_sha256, new.warden_sha256);
    assert_eq!(old.parent_lock_sha256, new.parent_lock_sha256);

    let temp = tempfile::Builder::new()
        .prefix("tradeassembly-baseline-upgrade-")
        .tempdir()
        .unwrap();
    let root = temp.path().join("rig with spaces");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (success, result) = invoke(baseline, &root, "install", port);
    assert!(success, "baseline install: {result}");
    let state = root.join("state/local");
    let secrets = ["signing.seed", "warden.token", "runtime.db.local-owner"];
    let fingerprints: Vec<_> = secrets
        .iter()
        .filter_map(|name| {
            let path = state.join(name);
            path.is_file().then(|| (name, digest(&path).unwrap()))
        })
        .collect();
    assert!(fingerprints.len() >= 2);
    let config_path = state.join("runtime.json");
    let mut config: Value = read_json(&config_path).unwrap();
    config["oidcScopes"] = json!("openid profile cohort_scope");
    atomic_json(&config_path, &config).unwrap();
    let db = rusqlite::Connection::open(state.join("runtime.db")).unwrap();
    db.execute("INSERT INTO tradeassembly_kv(namespace,item_key,value_json,idempotency_key,authority_actor,updated_at_ms) VALUES('distribution_upgrade_probe','preserved',?1,'upgrade-probe','local-user',1)", [json!({"value":"retained"}).to_string()]).unwrap();

    let (success, result) = invoke(replacement, &root, "upgrade", port);
    assert!(success, "replacement upgrade: {result}");
    assert_eq!(result["version"], new.version);
    assert_eq!(result["payloadSha256"], new.archive_sha256);
    assert_eq!(
        read_json::<Value>(&config_path).unwrap()["oidcScopes"],
        config["oidcScopes"]
    );
    assert!(Command::new(root.join(executable("bin/tradeassembly")))
        .arg("--help")
        .output()
        .unwrap()
        .status
        .success());

    let (success, result) = invoke(replacement, &root, "rollback", port);
    assert!(success, "baseline rollback: {result}");
    assert_eq!(result["version"], old.version);
    assert_eq!(result["payloadSha256"], old.archive_sha256);
    assert_eq!(
        read_json::<Value>(&config_path).unwrap()["oidcScopes"],
        config["oidcScopes"]
    );
    for (name, fingerprint) in fingerprints {
        assert_eq!(
            digest(&state.join(name)).unwrap(),
            fingerprint,
            "{name} changed"
        );
    }
    assert_eq!(
        digest(&root.join(executable("authority/bin/warden"))).unwrap(),
        old.warden_sha256
    );
    assert_eq!(db.query_row("SELECT value_json FROM tradeassembly_kv WHERE namespace='distribution_upgrade_probe' AND item_key='preserved'", [], |row| row.get::<_, String>(0)).unwrap(), json!({"value":"retained"}).to_string());
    assert!(Command::new(root.join(executable("bin/tradeassembly")))
        .arg("--help")
        .output()
        .unwrap()
        .status
        .success());
    assert!(!root.join("pending.json").exists());
}
