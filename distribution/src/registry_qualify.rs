// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Separate, process-derived registry delivery proof. Never edits native receipts.
use crate::{digest, read_json, Release, Result, TARGETS};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, io::Write, path::Path, process::Command, time::SystemTime};

const SCHEMA: &str = "tradeassembly.registry-native.v1";
const PM_TEST: &str = "npm_and_pnpm_registry_packages_work_without_scripts_or_source";
const UPGRADE_TEST: &str =
    "frozen_baseline_upgrades_to_replacement_and_rolls_back_without_state_loss";
const RECEIPT: &str = "registry/qualification.json";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Check {
    artifact: String,
    sha256: String,
    exit_code: i32,
    command: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Receipt {
    schema_version: String,
    target: String,
    version: String,
    native_receipt_sha256: String,
    registry_manifest_sha256: String,
    baseline: Release,
    qualification_source_revision: String,
    package_manager_test_sha256: String,
    upgrade_test_sha256: String,
    started_at_ms: u64,
    finished_at_ms: u64,
    actual_broker_orders: bool,
    live_activated: bool,
    package_managers: Check,
    upgrade_rollback: Check,
}
fn pm_source() -> String {
    format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(include_bytes!("../tests/package_managers.rs"))
    )
}
fn upgrade_source() -> String {
    format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(include_bytes!("../tests/packaged_install.rs"))
    )
}
fn arguments(suite: &str, test: &str) -> Vec<String> {
    [
        "cargo",
        "test",
        "--locked",
        "-p",
        "tradeassembly-distribution",
        "--test",
        suite,
        test,
        "--",
        "--ignored",
        "--exact",
        "--test-threads=1",
    ]
    .map(String::from)
    .to_vec()
}
fn successful_test(log: &[u8], test: &str) -> bool {
    let log = String::from_utf8_lossy(log);
    log.lines()
        .any(|line| line == format!("test {test} ... ok"))
        && log
            .lines()
            .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"))
}
fn timestamp() -> Result<u64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|time| time.as_millis().try_into().ok())
        .ok_or_else(|| "registry_qualification_clock_invalid".into())
}
fn check(root: &Path, proof: &Check, suite: &str, test: &str) -> Result<()> {
    if proof.exit_code != 0 || proof.command != arguments(suite, test) {
        return Err("registry_qualification_command_binding_failed".into());
    }
    let value = serde_json::to_value(proof).map_err(|_| "registry_proof_encode_failed")?;
    let path = crate::package::evidence_file(root, &value)?;
    if !successful_test(
        &fs::read(path).map_err(|_| "registry_qualification_log_unavailable")?,
        test,
    ) {
        return Err("registry_qualification_test_not_passed".into());
    }
    Ok(())
}
pub(crate) fn verify(root: &Path, version: &str) -> Result<()> {
    let receipt: Receipt = read_json(&root.join(RECEIPT))
        .map_err(|_| "registry_native_qualification_missing_or_invalid")?;
    receipt.baseline.validate_candidate()?;
    if receipt.schema_version != SCHEMA
        || receipt.target != TARGETS[0]
        || receipt.version != version
        || receipt.native_receipt_sha256 != digest(&root.join(TARGETS[0]).join("receipt.json"))?
        || receipt.registry_manifest_sha256 != digest(&root.join("registry/registry.json"))?
        || receipt.baseline.schema_version != 1
        || receipt.baseline.target != TARGETS[0]
        || receipt.baseline.bundle_manifest_sha256 != crate::FROZEN_MAC_SHA
        || receipt.baseline.version == version
        || receipt.qualification_source_revision.len() != 40
        || !receipt
            .qualification_source_revision
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
        || receipt.package_manager_test_sha256 != pm_source()
        || receipt.upgrade_test_sha256 != upgrade_source()
        || receipt.started_at_ms == 0
        || receipt.finished_at_ms < receipt.started_at_ms
        || receipt.actual_broker_orders
        || receipt.live_activated
    {
        return Err("registry_qualification_binding_failed".into());
    }
    check(root, &receipt.package_managers, "package_managers", PM_TEST)?;
    check(
        root,
        &receipt.upgrade_rollback,
        "packaged_install",
        UPGRADE_TEST,
    )
}
fn run(
    root: &Path,
    source: &Path,
    suite: &str,
    test: &str,
    environment: &[(&str, &Path)],
    name: &str,
) -> Result<Check> {
    let args = arguments(suite, test);
    let mut command = Command::new(&args[0]);
    command.current_dir(source).args(&args[1..]);
    for (key, path) in environment {
        command.env(key, path);
    }
    let output = command
        .output()
        .map_err(|_| "registry_qualification_command_unavailable")?;
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    let path = root.join(name);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|_| "registry_qualification_new_log_required")?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| "registry_qualification_log_write_failed")?;
    if !output.status.success() || !successful_test(&bytes, test) {
        return Err(format!("registry_qualification_test_failed:{test}"));
    }
    Ok(Check {
        artifact: name.into(),
        sha256: digest(&path)?,
        exit_code: 0,
        command: args,
    })
}
pub fn capture(root: &Path, baseline: &Path, source: &Path) -> Result<Value> {
    if crate::install::host_target()? != TARGETS[0]
        || [root, baseline, source].iter().any(|p| !p.is_absolute())
    {
        return Err("registry_qualification_native_mac_and_absolute_paths_required".into());
    }
    let candidate = crate::package::verify_matrix(root, false)?;
    if candidate["schemaVersion"] != 2
        || !fs::symlink_metadata(root.join("registry")).is_ok_and(|metadata| metadata.is_dir())
    {
        return Err("registry_qualification_first_release_and_regular_registry_required".into());
    }
    let version = candidate["version"]
        .as_str()
        .ok_or("registry_version_missing")?;
    crate::package::verify_first_release_registry(root, version)?;
    if fs::symlink_metadata(root.join(RECEIPT)).is_ok() {
        verify(root, version)?;
        return Ok(
            json!({"qualified":true,"reused":true,"receipt":root.join(RECEIPT),"commercialReleaseReady":false}),
        );
    }
    let mac = root.join(TARGETS[0]);
    let native: Value = read_json(&mac.join("receipt.json"))?;
    let core_revision = native["sourceRevisions"]["core"]
        .as_str()
        .ok_or("registry_core_revision_missing")?;
    let revision = crate::qualify::committed_source_revision(source, core_revision)?;
    // Reject a clean but wrong test checkout before execution.
    if digest(&source.join("distribution/tests/package_managers.rs"))? != pm_source()
        || digest(&source.join("distribution/tests/packaged_install.rs"))? != upgrade_source()
    {
        return Err("registry_qualification_source_binding_failed".into());
    }
    let old: Release = read_json(&baseline.join("release.json"))?;
    old.validate_candidate()?;
    if old.schema_version != 1
        || old.target != TARGETS[0]
        || old.version == version
        || digest(&baseline.join("bundle.tar.gz"))? != old.archive_sha256
    {
        return Err("registry_qualification_frozen_baseline_required".into());
    }
    let started = timestamp()?;
    let output = root.join("registry/mac-qualification");
    fs::create_dir(&output).map_err(|_| "registry_qualification_new_output_required")?;
    let registry: Value = read_json(&root.join("registry/registry.json"))?;
    let tarball = crate::package::evidence_file(
        &root.join("registry"),
        &registry["packages"]["tradeassembly-darwin-arm64"]["tarball"],
    )?;
    // Upgrade from the untouched frozen baseline to actual captured registry bytes.
    let staging = tempfile::tempdir().map_err(|_| "registry_qualification_stage_failed")?;
    crate::extract(&tarball, staging.path())?;
    let replacement = staging.path().join("package");
    let package_managers = run(
        root,
        source,
        "package_managers",
        PM_TEST,
        &[("TRADEASSEMBLY_REGISTRY_EVIDENCE", root)],
        "registry/mac-qualification/package-managers.log",
    )?;
    let upgrade_rollback = run(
        root,
        source,
        "packaged_install",
        UPGRADE_TEST,
        &[
            ("TRADEASSEMBLY_DISTRIBUTION_BASELINE_PACKAGE", baseline),
            (
                "TRADEASSEMBLY_DISTRIBUTION_REPLACEMENT_PACKAGE",
                &replacement,
            ),
        ],
        "registry/mac-qualification/upgrade-rollback.log",
    )?;
    let receipt = Receipt {
        schema_version: SCHEMA.into(),
        target: TARGETS[0].into(),
        version: version.into(),
        native_receipt_sha256: digest(&mac.join("receipt.json"))?,
        registry_manifest_sha256: digest(&root.join("registry/registry.json"))?,
        baseline: old,
        qualification_source_revision: revision,
        package_manager_test_sha256: pm_source(),
        upgrade_test_sha256: upgrade_source(),
        started_at_ms: started,
        finished_at_ms: timestamp()?,
        actual_broker_orders: false,
        live_activated: false,
        package_managers,
        upgrade_rollback,
    };
    // Receipt is the final write; failed tests leave logs but no positive qualification.
    let mut file = tempfile::NamedTempFile::new_in(root.join("registry"))
        .map_err(|_| "registry_qualification_receipt_stage_failed")?;
    serde_json::to_writer_pretty(file.as_file_mut(), &receipt)
        .map_err(|_| "registry_qualification_receipt_write_failed")?;
    file.write_all(b"\n")
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| "registry_qualification_receipt_write_failed")?;
    file.persist_noclobber(root.join(RECEIPT))
        .map_err(|_| "registry_qualification_new_receipt_required")?;
    verify(root, version)?;
    Ok(
        json!({"qualified":true,"reused":false,"receipt":root.join(RECEIPT),"commercialReleaseReady":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_test_success_not_generic_zero_or_skipped_summary() {
        let valid = format!("test {PM_TEST} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 1 filtered out; finished in 1s\n");
        assert!(successful_test(valid.as_bytes(), PM_TEST));
        for invalid in [
            "exitCode: 0",
            "test result: ok. 0 passed; 0 failed; 1 ignored;",
            "test wrong ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;",
        ] {
            assert!(!successful_test(invalid.as_bytes(), PM_TEST));
        }
        assert!(!successful_test(
            valid.replace("0 failed", "1 failed").as_bytes(),
            PM_TEST
        ));
    }
    #[test]
    fn fixed_commands_cannot_execute_manifest_supplied_programs() {
        for (suite, test) in [
            ("package_managers", PM_TEST),
            ("packaged_install", UPGRADE_TEST),
        ] {
            let args = arguments(suite, test);
            assert_eq!(args[0], "cargo");
            assert_eq!(
                &args[8..],
                ["--", "--ignored", "--exact", "--test-threads=1"]
            );
            assert!(args.contains(&"--locked".into()));
        }
    }
    #[test]
    fn missing_receipt_cannot_qualify_published_delivery() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            verify(root.path(), "0.1.0-beta.3").unwrap_err(),
            "registry_native_qualification_missing_or_invalid"
        );
    }
    fn fixture(root: &Path) -> Value {
        fs::create_dir(root.join(TARGETS[0])).unwrap();
        fs::create_dir(root.join("registry")).unwrap();
        fs::write(
            root.join(TARGETS[0]).join("receipt.json"),
            b"unit native binding",
        )
        .unwrap();
        fs::write(
            root.join("registry/registry.json"),
            b"unit registry binding",
        )
        .unwrap();
        let mut checks = Vec::new();
        for (suite, test) in [
            ("package_managers", PM_TEST),
            ("packaged_install", UPGRADE_TEST),
        ] {
            let artifact = format!("registry/{suite}.log");
            fs::write(root.join(&artifact), format!("test {test} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 1 filtered out;\n")).unwrap();
            checks.push(Check {
                sha256: digest(&root.join(&artifact)).unwrap(),
                artifact,
                exit_code: 0,
                command: arguments(suite, test),
            });
        }
        let receipt = Receipt {
            schema_version: SCHEMA.into(),
            target: TARGETS[0].into(),
            version: "0.1.0-beta.3".into(),
            native_receipt_sha256: digest(&root.join(TARGETS[0]).join("receipt.json")).unwrap(),
            registry_manifest_sha256: digest(&root.join("registry/registry.json")).unwrap(),
            baseline: Release {
                schema_version: 1,
                version: "0.1.0-beta.1".into(),
                target: TARGETS[0].into(),
                parent_lock_sha256: crate::PARENT_SHA.into(),
                bundle_manifest_sha256: crate::FROZEN_MAC_SHA.into(),
                archive_sha256: "a".repeat(64),
                warden_sha256: "b".repeat(64),
                state_compatibility: "f2-local-v1".into(),
                candidate_descriptor_sha256: None,
                deployment_environment: None,
                connection_profile_sha256: None,
            },
            qualification_source_revision: "c".repeat(40),
            package_manager_test_sha256: pm_source(),
            upgrade_test_sha256: upgrade_source(),
            started_at_ms: 1,
            finished_at_ms: 2,
            actual_broker_orders: false,
            live_activated: false,
            package_managers: checks.remove(0),
            upgrade_rollback: checks.remove(0),
        };
        serde_json::to_value(receipt).unwrap()
    }
    #[test]
    fn receipt_rejects_changed_inputs_commands_source_host_and_unknown_fields() {
        let root = tempfile::tempdir().unwrap();
        let receipt = fixture(root.path());
        crate::atomic_json(&root.path().join(RECEIPT), &receipt).unwrap();
        verify(root.path(), "0.1.0-beta.3").unwrap(); // Leaf validation, not release proof.
        for pointer in [
            "/target",
            "/version",
            "/nativeReceiptSha256",
            "/registryManifestSha256",
            "/qualificationSourceRevision",
            "/packageManagerTestSha256",
            "/upgradeTestSha256",
            "/packageManagers/sha256",
            "/packageManagers/command/0",
            "/upgradeRollback/sha256",
            "/baseline/parentLockSha256",
        ] {
            let mut changed = receipt.clone();
            *changed.pointer_mut(pointer).unwrap() = json!("changed");
            crate::atomic_json(&root.path().join(RECEIPT), &changed).unwrap();
            assert!(verify(root.path(), "0.1.0-beta.3").is_err(), "{pointer}");
        }
        for pointer in ["/actualBrokerOrders", "/liveActivated"] {
            let mut changed = receipt.clone();
            *changed.pointer_mut(pointer).unwrap() = json!(true);
            crate::atomic_json(&root.path().join(RECEIPT), &changed).unwrap();
            assert!(verify(root.path(), "0.1.0-beta.3").is_err());
        }
        let mut changed = receipt.clone();
        changed["ready"] = json!(true);
        crate::atomic_json(&root.path().join(RECEIPT), &changed).unwrap();
        assert!(verify(root.path(), "0.1.0-beta.3").is_err());
        crate::atomic_json(&root.path().join(RECEIPT), &receipt).unwrap();
        fs::write(
            root.path().join("registry/package_managers.log"),
            b"test result: ok. 0 passed; 0 failed; 1 ignored;",
        )
        .unwrap();
        assert!(verify(root.path(), "0.1.0-beta.3").is_err());
    }
}
