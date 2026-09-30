// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Native, process-derived qualification. An incomplete run has no receipt.

use crate::{
    atomic_json, digest, executable, install, package::NPM_NAMES, read_json, Release, Result,
    TARGETS,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn committed_source_revision(source: &Path, core_revision: &str) -> Result<String> {
    let status = Command::new("git")
        .args([
            "-C",
            source.to_str().ok_or("qualification_source_invalid")?,
            "status",
            "--porcelain",
        ])
        .output()
        .map_err(|_| "qualification_source_status_unavailable")?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err("qualification_source_not_clean".into());
    }
    let ancestor = Command::new("git")
        .args([
            "-C",
            source.to_str().ok_or("qualification_source_invalid")?,
            "merge-base",
            "--is-ancestor",
            core_revision,
            "HEAD",
        ])
        .status()
        .map_err(|_| "qualification_source_revision_unavailable")?;
    if !ancestor.success() {
        return Err("qualification_core_source_not_ancestor".into());
    }
    let head = Command::new("git")
        .args([
            "-C",
            source.to_str().ok_or("qualification_source_invalid")?,
            "rev-parse",
            "HEAD",
        ])
        .output()
        .map_err(|_| "qualification_source_revision_unavailable")?;
    if !head.status.success() {
        return Err("qualification_source_revision_unavailable".into());
    }
    String::from_utf8(head.stdout)
        .map(|text| text.trim().into())
        .map_err(|_| "qualification_source_revision_invalid".into())
}

fn proof(root: &Path, name: &str) -> Result<Value> {
    Ok(json!({"artifact":name,"sha256":digest(&root.join(name))?}))
}

fn run(mut command: Command, log: &Path, expected: &str) -> Result<()> {
    let output = command
        .output()
        .map_err(|_| format!("qualification_command_unavailable:{expected}"))?;
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    fs::write(log, &bytes).map_err(|_| "qualification_log_write_failed")?;
    if !output.status.success() || !String::from_utf8_lossy(&bytes).contains(expected) {
        return Err(format!("qualification_check_failed:{expected}"));
    }
    Ok(())
}

fn cargo_test(
    source: &Path,
    package: &str,
    test: &str,
    filter: &str,
    expected: &str,
    env: &BTreeMap<&str, String>,
    log: &Path,
) -> Result<()> {
    let mut command = Command::new("cargo");
    command.current_dir(source).args([
        "test",
        "--locked",
        "-p",
        package,
        "--test",
        test,
        filter,
        "--",
        "--ignored",
        "--test-threads=1",
    ]);
    for (name, value) in env {
        command.env(name, value);
    }
    run(command, log, expected)
}

fn copy(source: &Path, out: &Path, name: &str) -> Result<()> {
    fs::copy(source, out.join(name))
        .map_err(|_| format!("qualification_artifact_copy_failed:{name}"))?;
    Ok(())
}

pub fn qualify(
    candidate: &Path,
    baseline: Option<&Path>,
    controlled_broker: &Path,
    source: &Path,
    out: &Path,
) -> Result<Value> {
    if out.exists()
        || !source.join("Cargo.toml").is_file()
        || !source.join("runtime-rs/tests/agent_order_mcp.rs").is_file()
    {
        return Err("qualification_output_or_source_invalid".into());
    }
    let candidate = candidate
        .canonicalize()
        .map_err(|_| "qualification_candidate_unavailable")?;
    let controlled_broker = controlled_broker
        .canonicalize()
        .map_err(|_| "qualification_controlled_broker_unavailable")?;
    let source = source
        .canonicalize()
        .map_err(|_| "qualification_source_unavailable")?;
    let metadata: Value = read_json(&candidate.join("candidate.json"))?;
    let release: Release = serde_json::from_value(metadata["release"].clone())
        .map_err(|_| "qualification_release_invalid")?;
    release.validate_candidate()?;
    let host = install::host_target()?;
    if release.target != host || release.schema_version != 2 || metadata["publishable"] != false {
        return Err("qualification_native_candidate_required".into());
    }
    let index = TARGETS
        .iter()
        .position(|target| *target == host)
        .ok_or("qualification_target_invalid")?;
    let platform = candidate.join(NPM_NAMES[index]);
    let packaged: Release = read_json(&platform.join("release.json"))?;
    if packaged != release || digest(&platform.join("bundle.tar.gz"))? != release.archive_sha256 {
        return Err("qualification_package_binding_failed".into());
    }
    let descriptor: Value = read_json(&platform.join(crate::candidate::NAME))?;
    let core_revision = descriptor["nativeInputs"]["coreRevision"]
        .as_str()
        .ok_or("qualification_core_revision_missing")?;
    let qualification_source_revision = committed_source_revision(&source, core_revision)?;
    if host == TARGETS[0] && baseline.is_none() {
        return Err("qualification_frozen_baseline_required".into());
    }
    fs::create_dir_all(out.parent().ok_or("qualification_output_parent_invalid")?)
        .map_err(|_| "qualification_output_create_failed")?;
    fs::create_dir(out).map_err(|_| "qualification_output_create_failed")?;
    copy(&platform.join("release.json"), out, "release.json")?;
    copy(&platform.join("installer.json"), out, "installer.json")?;
    let installer_name = executable("tradeassembly-distribution");
    copy(&platform.join(&installer_name), out, &installer_name)?;
    let controlled_name = executable("f2-controlled-broker");
    copy(&controlled_broker, out, &controlled_name)?;

    for (name, directory) in [
        ("launcher", candidate.join("tradeassembly")),
        ("platform", platform.clone()),
    ] {
        let mut command = Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" });
        command
            .current_dir(&directory)
            .args(["pack", "--ignore-scripts", "--json", "--pack-destination"])
            .arg(out);
        run(command, &out.join(format!("{name}-pack.log")), "filename")?;
        let package_name = if name == "launcher" {
            "tradeassembly"
        } else {
            NPM_NAMES[index]
        };
        let archive = out.join(format!("{package_name}-{}.tgz", release.version));
        fs::rename(archive, out.join(format!("{name}.tgz")))
            .map_err(|_| "qualification_npm_archive_missing")?;
    }

    // User-like identity material is created only under this private,
    // automatically removed rig, never inside the publishable evidence tree.
    let private_rig = tempfile::Builder::new()
        .prefix("tradeassembly-native-qualification-")
        .tempdir()
        .map_err(|_| "qualification_private_rig_unavailable")?;
    let rig = private_rig.path().join("isolated-rig");
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| "qualification_port_unavailable")?;
    let port = listener
        .local_addr()
        .map_err(|_| "qualification_port_unavailable")?
        .port();
    drop(listener);
    let mut installer = Command::new(platform.join(&installer_name));
    installer
        .args(["install", "--root"])
        .arg(&rig)
        .args(["--warden-port", &port.to_string()]);
    run(
        installer,
        &out.join("installed-package.log"),
        "\"installed\": true",
    )?;
    #[cfg(windows)]
    {
        // This target must prove a fresh one-command install, including the
        // elevated SRT provisioner. A pre-existing machine setup is useful to
        // customers but cannot qualify that release behavior.
        let installed: Value = read_json(&out.join("installed-package.log"))?;
        if installed["windowsSandboxProvisionedNow"] != true
            || installed["windowsSandboxBehavioralReady"] != true
        {
            return Err("qualification_windows_fresh_sandbox_setup_required".into());
        }
        for directory in [&rig, &rig.join("state"), &rig.join("state/local")] {
            crate::windows_private::validate_directory(directory)
                .map_err(|_| "qualification_windows_private_acl_failed")?;
        }
        for name in ["signing.seed", "warden.token"] {
            crate::windows_private::open_read(&rig.join("state/local").join(name))
                .map_err(|_| "qualification_windows_private_acl_failed")?;
        }
        atomic_json(
            &out.join("windows-private-acl.json"),
            &json!({"schemaVersion":"tradeassembly.windows-private-acl.v1","ownerOnlyState":true,"ownerOnlyAuthorityFiles":true}),
        )?;
    }
    let payload = rig.join("versions").join(&release.archive_sha256);
    if digest(&rig.join(executable("authority/bin/warden")))? != release.warden_sha256 {
        return Err("qualification_real_warden_binding_failed".into());
    }
    let mut env = BTreeMap::new();
    env.insert(
        "TRADEASSEMBLY_DISTRIBUTION_PACKAGE",
        platform.display().to_string(),
    );
    cargo_test(
        &source,
        "tradeassembly-distribution",
        "packaged_install",
        "frozen_package_setup_reinstall_upgrade_rollback_and_running_denial",
        "1 passed",
        &env,
        &out.join("packaged-install.log"),
    )?;
    if let Some(baseline) = baseline {
        env.insert(
            "TRADEASSEMBLY_DISTRIBUTION_BASELINE_PACKAGE",
            baseline.display().to_string(),
        );
        env.insert(
            "TRADEASSEMBLY_DISTRIBUTION_REPLACEMENT_PACKAGE",
            platform.display().to_string(),
        );
        cargo_test(
            &source,
            "tradeassembly-distribution",
            "packaged_install",
            "frozen_baseline_upgrades_to_replacement_and_rolls_back_without_state_loss",
            "1 passed",
            &env,
            &out.join("baseline-upgrade.log"),
        )?;
    }
    env.clear();
    env.insert(
        "TRADEASSEMBLY_DISTRIBUTION_CANDIDATE",
        candidate.display().to_string(),
    );
    cargo_test(
        &source,
        "tradeassembly-distribution",
        "package_managers",
        "npm_and_pnpm_local_tarballs_work_without_lifecycle_scripts_or_source",
        "1 passed",
        &env,
        &out.join("package-managers.log"),
    )?;
    env.clear();
    env.insert("F2_TEST_BUNDLE_PATH", payload.display().to_string());
    #[cfg(windows)]
    env.insert(
        "F2_TEST_PRIVATE_STATE_ROOT",
        rig.join("state/local").display().to_string(),
    );
    cargo_test(
        &source,
        "tradeassembly-runtime",
        "packaged_sandbox",
        "packaged_sandbox_runs_and_denies_files_and_network_without_host_node",
        "1 passed",
        &env,
        &out.join("sandbox.log"),
    )?;
    env.clear();
    env.insert(
        "F2_TEST_RUNTIME_BINARY",
        payload
            .join(executable("bin/tradeassembly"))
            .display()
            .to_string(),
    );
    env.insert(
        "F2_TEST_WARDEN_BINARY",
        rig.join(executable("authority/bin/warden"))
            .display()
            .to_string(),
    );
    env.insert(
        "F2_TEST_CONTROLLED_BROKER_BINARY",
        controlled_broker.display().to_string(),
    );
    env.insert(
        "F2_TEST_NODE_BINARY",
        payload
            .join(executable("runtime/node/bin/node"))
            .display()
            .to_string(),
    );
    env.insert(
        "F2_TEST_SRT_CLI",
        payload
            .join("runtime/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js")
            .display()
            .to_string(),
    );
    cargo_test(
        &source,
        "tradeassembly-runtime",
        "agent_order_mcp",
        "",
        "4 passed",
        &env,
        &out.join("agent-order-mcp.log"),
    )?;

    let (os, arch) = match host {
        "aarch64-apple-darwin" => ("macos", "aarch64"),
        "x86_64-apple-darwin" => ("macos", "x86_64"),
        "x86_64-unknown-linux-gnu" => ("linux", "x86_64"),
        "aarch64-unknown-linux-gnu" => ("linux", "aarch64"),
        "x86_64-pc-windows-msvc" => ("windows", "x86_64"),
        _ => return Err("qualification_host_unsupported".into()),
    };
    let mut checks = serde_json::Map::new();
    for (name, log) in [
        ("frozenPayload", "installed-package.log"),
        ("npmLocalIgnoreScripts", "package-managers.log"),
        ("pnpmLocalIgnoreScripts", "package-managers.log"),
        ("sourceFreeSetup", "packaged-install.log"),
        ("offlineAlpaca", "sandbox.log"),
        (
            "stoppedUpgradeRollback",
            if baseline.is_some() {
                "baseline-upgrade.log"
            } else {
                "packaged-install.log"
            },
        ),
        ("activeUpgradeDenied", "packaged-install.log"),
        ("interruptedRecovery", "packaged-install.log"),
        (
            "preservedConfigurationIdentityState",
            if baseline.is_some() {
                "baseline-upgrade.log"
            } else {
                "packaged-install.log"
            },
        ),
        ("duplicateRecovery", "agent-order-mcp.log"),
        ("ambiguousOutcomeRecovery", "agent-order-mcp.log"),
        ("wardenControlledSink", "agent-order-mcp.log"),
        ("sandboxDeniedEgress", "sandbox.log"),
        ("sandboxDeniedFilesystem", "sandbox.log"),
    ] {
        let mut value = proof(out, log)?;
        value["exitCode"] = json!(0);
        checks.insert(name.into(), value);
    }
    #[cfg(windows)]
    for (name, log) in [
        ("windowsPrivateAcl", "windows-private-acl.json"),
        ("windowsSandboxAccount", "sandbox.log"),
        ("windowsSandboxElevation", "installed-package.log"),
        ("windowsSandboxWfp", "sandbox.log"),
        ("windowsSandboxDeniedOwnerSecret", "sandbox.log"),
    ] {
        let mut value = proof(out, log)?;
        value["exitCode"] = json!(0);
        checks.insert(name.into(), value);
    }
    let receipt = json!({
        "schemaVersion":"tradeassembly.distribution-native.v2",
        "target":host,"version":release.version,
        "releaseManifestSha256":digest(&out.join("release.json"))?,
        "native":true,"host":{"os":os,"arch":arch,"target":host},
        "realWarden":true,"actualBrokerOrders":false,"liveActivated":false,
        "qualificationSourceRevision":qualification_source_revision,
        "candidateDescriptorSha256":digest(&platform.join(crate::candidate::NAME))?,
        "sourceRevisions":{
            "core":descriptor["nativeInputs"]["coreRevision"],
            "warden":descriptor["nativeInputs"]["wardenRevision"],
            "alpaca":descriptor["nativeInputs"]["alpacaRevision"],
        },
        "controlledBroker":proof(out,&controlled_name)?,
        "launcherSha256":digest(&candidate.join("tradeassembly/cli.cjs"))?,
        "artifacts":{
            "installerDescriptor":proof(out,"installer.json")?,
            "installer":proof(out,&installer_name)?,
            "npmLauncher":proof(out,"launcher.tgz")?,
            "npmPlatform":proof(out,"platform.tgz")?,
        },
        "checks":checks,
    });
    crate::package::verify_delivery_artifacts(out, &receipt, &release)?;
    // A receipt is the final write. Failed or interrupted commands leave
    // inspectable logs but cannot be mistaken for qualification.
    atomic_json(&out.join("receipt.json"), &receipt)?;
    Ok(json!({"qualified":true,"target":host,"receipt":out.join("receipt.json")}))
}
