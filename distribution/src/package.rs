// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::{
    archive, atomic_json, digest, read_json, verify_bundle, Release, Result, PARENT_SHA, TARGETS,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn evidence_file(root: &Path, proof: &Value) -> Result<PathBuf> {
    let name = proof["artifact"]
        .as_str()
        .ok_or("artifact_binding_missing")?;
    crate::relative(Path::new(name))?;
    let file = root.join(name);
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "evidence_root_unavailable")?;
    if !file
        .canonicalize()
        .map_err(|_| "evidence_unavailable")?
        .starts_with(canonical_root)
        || !file.is_file()
        || fs::metadata(&file)
            .map_err(|_| "evidence_unavailable")?
            .len()
            == 0
        || proof["sha256"] != digest(&file)?
    {
        return Err("artifact_binding_failed".into());
    }
    Ok(file)
}

pub(crate) fn verify_delivery_artifacts(
    root: &Path,
    receipt: &Value,
    release: &Release,
) -> Result<()> {
    let descriptor = evidence_file(root, &receipt["artifacts"]["installerDescriptor"])?;
    let installer = evidence_file(root, &receipt["artifacts"]["installer"])?;
    let launcher = evidence_file(root, &receipt["artifacts"]["npmLauncher"])?;
    let platform = evidence_file(root, &receipt["artifacts"]["npmPlatform"])?;
    let filename = if release.target == TARGETS[4] {
        "tradeassembly-distribution.exe"
    } else {
        "tradeassembly-distribution"
    };
    let installer_spec: Value = read_json(&descriptor)?;
    if installer_spec["filename"] != filename || installer_spec["sha256"] != digest(&installer)? {
        return Err("qualified_installer_binding_failed".into());
    }
    let platform_stage = tempfile::tempdir().map_err(|_| "qualification_stage_failed")?;
    crate::extract(&platform, platform_stage.path())?;
    let payload = platform_stage.path().join("package");
    let index = TARGETS
        .iter()
        .position(|t| *t == release.target)
        .ok_or("bundle_target_unsupported")?;
    let package: Value = read_json(&payload.join("package.json"))?;
    let packaged_release: Release = read_json(&payload.join("release.json"))?;
    if packaged_release != *release
        || package["name"] != NPM_NAMES[index]
        || package["version"] != release.version
        || package.get("scripts").is_some()
        || digest(&payload.join("installer.json"))? != digest(&descriptor)?
        || digest(&payload.join(filename))? != digest(&installer)?
        || digest(&payload.join("bundle.tar.gz"))? != release.archive_sha256
    {
        return Err("qualified_platform_package_binding_failed".into());
    }
    if release.schema_version == 2 {
        let candidate_stage = tempfile::tempdir().map_err(|_| "qualification_stage_failed")?;
        crate::extract(&payload.join("bundle.tar.gz"), candidate_stage.path())?;
        crate::candidate::verify(&payload, candidate_stage.path(), release)?;
        let candidate_descriptor = payload.join(crate::candidate::NAME);
        let described: Value = read_json(&candidate_descriptor)?;
        if receipt["candidateDescriptorSha256"] != digest(&candidate_descriptor)?
            || receipt["sourceRevisions"]["core"] != described["nativeInputs"]["coreRevision"]
            || receipt["sourceRevisions"]["warden"] != described["nativeInputs"]["wardenRevision"]
            || receipt["sourceRevisions"]["alpaca"] != described["nativeInputs"]["alpacaRevision"]
        {
            return Err("qualified_candidate_source_binding_failed".into());
        }
    } else {
        crate::candidate::verify(&payload, &payload, release)?;
    }
    let launcher_stage = tempfile::tempdir().map_err(|_| "qualification_stage_failed")?;
    crate::extract(&launcher, launcher_stage.path())?;
    let payload = launcher_stage.path().join("package");
    let package: Value = read_json(&payload.join("package.json"))?;
    if package["name"] != "tradeassembly"
        || package["version"] != release.version
        || package.get("scripts").is_some()
        || package["bin"]["tradeassembly"] != "cli.cjs"
        || receipt["launcherSha256"] != digest(&payload.join("cli.cjs"))?
        || NPM_NAMES
            .iter()
            .any(|name| package["optionalDependencies"][name] != release.version)
    {
        return Err("qualified_launcher_package_binding_failed".into());
    }
    Ok(())
}

pub const NPM_NAMES: [&str; 5] = [
    "tradeassembly-darwin-arm64",
    "tradeassembly-darwin-x64",
    "tradeassembly-linux-x64",
    "tradeassembly-linux-arm64",
    "tradeassembly-win32-x64",
];

pub fn pack(
    bundle: &Path,
    parent: &Path,
    version: &str,
    installer: &Path,
    descriptor: Option<&Path>,
    out: &Path,
) -> Result<Value> {
    if digest(parent)? != PARENT_SHA {
        return Err("parent_release_lock_changed".into());
    }
    let metadata: Value = read_json(&bundle.join("bundle.json"))?;
    let target = metadata["target"].as_str().ok_or("bundle_target_missing")?;
    // Packaging a cross-target payload with a host installer is forbidden.
    if target != crate::install::host_target()? {
        return Err("pack_on_native_target_required".into());
    }
    let index = TARGETS
        .iter()
        .position(|t| *t == target)
        .ok_or("bundle_target_unsupported")?;
    if descriptor.is_some_and(|path| {
        path.file_name().and_then(|name| name.to_str()) != Some(crate::candidate::NAME)
    }) {
        return Err("candidate_descriptor_filename_invalid".into());
    }
    let descriptor_digest = descriptor.map(digest).transpose()?;
    if descriptor.is_none() && version != "0.1.0-beta.1" {
        return Err("replacement_candidate_descriptor_required".into());
    }
    let mut release = Release {
        schema_version: if descriptor.is_some() { 2 } else { 1 },
        version: version.into(),
        target: target.into(),
        parent_lock_sha256: PARENT_SHA.into(),
        bundle_manifest_sha256: digest(&bundle.join("bundle.json"))?,
        archive_sha256: "0".repeat(64),
        warden_sha256: digest(&bundle.join(crate::executable("bin/warden")))?,
        state_compatibility: "f2-local-v1".into(),
        candidate_descriptor_sha256: descriptor_digest,
    };
    verify_bundle(bundle, &release)?;
    release.validate_candidate()?;
    if let Some(descriptor) = descriptor {
        crate::candidate::verify(
            descriptor
                .parent()
                .ok_or("candidate_descriptor_path_invalid")?,
            bundle,
            &release,
        )?;
    }
    if out.exists() {
        return Err("output_exists_use_new_candidate_directory".into());
    }
    let parent_out = out.parent().ok_or("output_path_invalid")?;
    fs::create_dir_all(parent_out).map_err(|_| "output_create_failed")?;
    let stage = tempfile::Builder::new()
        .prefix("distribution-")
        .tempdir_in(parent_out)
        .map_err(|_| "output_create_failed")?;
    let platform = stage.path().join(NPM_NAMES[index]);
    fs::create_dir(&platform).map_err(|_| "output_create_failed")?;
    if let Some(descriptor) = descriptor {
        fs::copy(descriptor, platform.join(crate::candidate::NAME))
            .map_err(|_| "candidate_descriptor_copy_failed")?;
    }
    archive(bundle, &platform.join("bundle.tar.gz"))?;
    release.archive_sha256 = digest(&platform.join("bundle.tar.gz"))?;
    atomic_json(&platform.join("release.json"), &release)?;
    let installer_name = crate::executable("tradeassembly-distribution");
    fs::copy(installer, platform.join(&installer_name)).map_err(|_| "installer_copy_failed")?;
    atomic_json(
        &platform.join("installer.json"),
        &json!({"filename":installer_name,"sha256":digest(installer)?}),
    )?;
    let (os, cpu) = match index {
        0 => ("darwin", "arm64"),
        1 => ("darwin", "x64"),
        2 => ("linux", "x64"),
        3 => ("linux", "arm64"),
        _ => ("win32", "x64"),
    };
    let mut package_files = vec!["bundle.tar.gz", "release.json", "installer.json"];
    if descriptor.is_some() {
        package_files.push(crate::candidate::NAME);
    }
    package_files.extend([installer_name.as_str(), "LICENSE", "NOTICE"]);
    atomic_json(
        &platform.join("package.json"),
        &json!({
            "name":NPM_NAMES[index],"version":version,"description":"TradeAssembly native F2 prerelease payload",
            "license":"Apache-2.0","os":[os],"cpu":[cpu],
            "files":package_files,
            "engines":{"node":">=22"},"publishConfig":{"access":"public","tag":"beta"}
        }),
    )?;
    let launcher = stage.path().join("tradeassembly");
    fs::create_dir(&launcher).map_err(|_| "output_create_failed")?;
    fs::write(
        launcher.join("cli.cjs"),
        include_bytes!("../../packaging/npm/cli.cjs"),
    )
    .map_err(|_| "launcher_write_failed")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(launcher.join("cli.cjs"), fs::Permissions::from_mode(0o755))
            .map_err(|_| "launcher_permission_failed")?;
    }
    let deps: BTreeMap<_, _> = NPM_NAMES.into_iter().map(|name| (name, version)).collect();
    atomic_json(
        &launcher.join("package.json"),
        &json!({
            "name":"tradeassembly","version":version,"description":"Local-first trading tooling. Users supply and authorize all strategies.",
            "license":"Apache-2.0","bin":{"tradeassembly":"cli.cjs"},"files":["cli.cjs","LICENSE","NOTICE","README.md"],
            "optionalDependencies":deps,"engines":{"node":">=22"},"publishConfig":{"access":"public","tag":"beta"}
        }),
    )?;
    fs::write(
        launcher.join("README.md"),
        include_bytes!("../../packaging/npm/README.md"),
    )
    .map_err(|_| "launcher_write_failed")?;
    for directory in [&launcher, &platform] {
        for name in ["LICENSE", "NOTICE"] {
            fs::copy(bundle.join(name), directory.join(name)).map_err(|_| "notice_copy_failed")?;
        }
    }
    atomic_json(
        &stage.path().join("candidate.json"),
        &json!({"schemaVersion":1,"release":release,"platformPackage":NPM_NAMES[index],"launcherPackage":"tradeassembly","publishable":false,"reason":"native_matrix_and_registry_installation_not_yet_qualified"}),
    )?;
    fs::rename(stage.path(), out).map_err(|_| "candidate_commit_failed")?;
    Ok(json!({"candidate":out,"release":release,"publishable":false}))
}

/// Checks release evidence, never substitutes packaging tests for runtime gates.
/// Each receipt references nonempty hashed artifacts produced on its own target.
pub fn verify_matrix(root: &Path, published: bool) -> Result<Value> {
    let matrix: Value = read_json(&root.join("matrix.json"))?;
    if matrix["schemaVersion"] != 1
        || matrix["parentLockSha256"] != PARENT_SHA
        || matrix["npmTag"] != "beta"
    {
        return Err("distribution_matrix_invalid".into());
    }
    let version = matrix["version"]
        .as_str()
        .ok_or("distribution_version_missing")?;
    let mut qualified = Vec::new();
    for target in TARGETS {
        let (host_os, host_arch) = match target {
            "aarch64-apple-darwin" => ("macos", "aarch64"),
            "x86_64-apple-darwin" => ("macos", "x86_64"),
            "x86_64-unknown-linux-gnu" => ("linux", "x86_64"),
            "aarch64-unknown-linux-gnu" => ("linux", "aarch64"),
            _ => ("windows", "x86_64"),
        };
        let receipt_path = root.join(target).join("receipt.json");
        let receipt: Value = read_json(&receipt_path)
            .map_err(|_| format!("native_qualification_missing:{target}"))?;
        let release: Release = read_json(&root.join(target).join("release.json"))?;
        release.validate_candidate()?;
        if release.target != target
            || release.version != version
            || receipt["schemaVersion"] != "tradeassembly.distribution-native.v2"
            || receipt["target"] != target
            || receipt["version"] != version
            || receipt["releaseManifestSha256"] != digest(&root.join(target).join("release.json"))?
            || receipt["native"] != true
            || receipt["host"]["os"] != host_os
            || receipt["host"]["arch"] != host_arch
            || receipt["host"]["target"] != target
            || receipt["realWarden"] != true
            || receipt["actualBrokerOrders"] != false
            || receipt["liveActivated"] != false
            || receipt["qualificationSourceRevision"]
                .as_str()
                .is_none_or(|value| {
                    value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        {
            return Err(format!("native_qualification_binding_failed:{target}"));
        }
        evidence_file(&root.join(target), &receipt["controlledBroker"])
            .map_err(|code| format!("native_controlled_broker_binding_failed:{target}:{code}"))?;
        verify_delivery_artifacts(&root.join(target), &receipt, &release)
            .map_err(|code| format!("native_delivery_binding_failed:{target}:{code}"))?;
        let mut checks = vec![
            "frozenPayload",
            "npmLocalIgnoreScripts",
            "pnpmLocalIgnoreScripts",
            "sourceFreeSetup",
            "offlineAlpaca",
            "stoppedUpgradeRollback",
            "activeUpgradeDenied",
            "interruptedRecovery",
            "preservedConfigurationIdentityState",
            "duplicateRecovery",
            "ambiguousOutcomeRecovery",
            "wardenControlledSink",
            "sandboxDeniedEgress",
            "sandboxDeniedFilesystem",
        ];
        if target == TARGETS[4] {
            checks.extend([
                "windowsPrivateAcl",
                "windowsSandboxAccount",
                "windowsSandboxElevation",
                "windowsSandboxWfp",
            ]);
        }
        if published {
            checks.extend(["npmPublishedIgnoreScripts", "pnpmPublishedIgnoreScripts"]);
        }
        let evidence_root = root
            .join(target)
            .canonicalize()
            .map_err(|_| "evidence_root_unavailable")?;
        for check in checks {
            let proof = &receipt["checks"][check];
            let name = proof["artifact"]
                .as_str()
                .ok_or_else(|| format!("native_check_missing:{target}:{check}"))?;
            crate::relative(Path::new(name))?;
            let artifact = root.join(target).join(name);
            if !artifact
                .canonicalize()
                .map_err(|_| "evidence_unavailable")?
                .starts_with(&evidence_root)
                || proof["exitCode"] != 0
                || !artifact.is_file()
                || fs::metadata(&artifact)
                    .map_err(|_| "evidence_unavailable")?
                    .len()
                    == 0
                || proof["sha256"] != digest(&artifact)?
            {
                return Err(format!("native_check_failed:{target}:{check}"));
            }
        }
        if target == TARGETS[4] {
            if receipt["checks"]["windowsSandboxElevation"]["artifact"] != "installed-package.log"
                || receipt["checks"]["windowsPrivateAcl"]["artifact"] != "windows-private-acl.json"
                || receipt["checks"]["windowsSandboxAccount"]["artifact"] != "sandbox.log"
                || receipt["checks"]["windowsSandboxWfp"]["artifact"] != "sandbox.log"
            {
                return Err("native_windows_proof_source_invalid".into());
            }
            let installed: Value = read_json(&root.join(target).join("installed-package.log"))?;
            let acl: Value = read_json(&root.join(target).join("windows-private-acl.json"))?;
            if installed["installed"] != true
                || installed["windowsSandboxProvisionedNow"] != true
                || acl["schemaVersion"] != "tradeassembly.windows-private-acl.v1"
                || acl["ownerOnlyState"] != true
                || acl["ownerOnlyAuthorityFiles"] != true
            {
                return Err("native_windows_proof_semantics_invalid".into());
            }
        }
        qualified.push(target);
    }
    Ok(
        json!({"schemaVersion":1,"qualified":true,"phase":if published {"published"} else {"candidate"},"releaseReady":published,"commercialReleaseReady":false,"m7":false,"m8":false,"version":version,"targets":qualified,"npmTag":"beta","appleNotarized":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof(root: &Path, name: &str) -> Value {
        json!({"artifact":name,"sha256":digest(&root.join(name)).unwrap()})
    }

    fn delivery_fixture(root: &Path) -> (Value, Release) {
        fs::write(root.join("installer"), b"original installer fixture").unwrap();
        atomic_json(
            &root.join("installer.json"),
            &json!({
                "filename":"tradeassembly-distribution",
                "sha256":digest(&root.join("installer")).unwrap()
            }),
        )
        .unwrap();
        let stage = tempfile::tempdir().unwrap();
        let package = stage.path().join("package");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("bundle.tar.gz"), b"frozen archive fixture").unwrap();
        let release = Release {
            schema_version: 1,
            version: "0.1.0-beta.1".into(),
            target: TARGETS[2].into(),
            parent_lock_sha256: PARENT_SHA.into(),
            bundle_manifest_sha256: "a".repeat(64),
            archive_sha256: digest(&package.join("bundle.tar.gz")).unwrap(),
            warden_sha256: "b".repeat(64),
            state_compatibility: "f2-local-v1".into(),
            candidate_descriptor_sha256: None,
        };
        atomic_json(&package.join("release.json"), &release).unwrap();
        atomic_json(
            &package.join("package.json"),
            &json!({
                "name":NPM_NAMES[2],"version":release.version
            }),
        )
        .unwrap();
        fs::copy(
            root.join("installer"),
            package.join("tradeassembly-distribution"),
        )
        .unwrap();
        fs::copy(root.join("installer.json"), package.join("installer.json")).unwrap();
        archive(stage.path(), &root.join("platform.tgz")).unwrap();
        let stage = tempfile::tempdir().unwrap();
        let package = stage.path().join("package");
        fs::create_dir(&package).unwrap();
        fs::write(package.join("cli.cjs"), b"launcher fixture").unwrap();
        let deps: BTreeMap<_, _> = NPM_NAMES
            .into_iter()
            .map(|name| (name, &release.version))
            .collect();
        atomic_json(
            &package.join("package.json"),
            &json!({
                "name":"tradeassembly","version":release.version,
                "bin":{"tradeassembly":"cli.cjs"},"optionalDependencies":deps
            }),
        )
        .unwrap();
        let launcher_sha = digest(&package.join("cli.cjs")).unwrap();
        archive(stage.path(), &root.join("launcher.tgz")).unwrap();
        (
            json!({"launcherSha256":launcher_sha,"artifacts":{
                "installerDescriptor":proof(root,"installer.json"),"installer":proof(root,"installer"),
                "npmLauncher":proof(root,"launcher.tgz"),"npmPlatform":proof(root,"platform.tgz")
            }}),
            release,
        )
    }

    #[test]
    fn delivery_binding_inspects_actual_tarballs_and_rejects_changed_installer() {
        let root = tempfile::tempdir().unwrap();
        let (mut receipt, release) = delivery_fixture(root.path());
        verify_delivery_artifacts(root.path(), &receipt, &release).unwrap();
        fs::write(root.path().join("installer"), b"different installer").unwrap();
        assert_eq!(
            verify_delivery_artifacts(root.path(), &receipt, &release).unwrap_err(),
            "artifact_binding_failed"
        );
        receipt["artifacts"]["installer"] = proof(root.path(), "installer");
        assert_eq!(
            verify_delivery_artifacts(root.path(), &receipt, &release).unwrap_err(),
            "qualified_installer_binding_failed"
        );
        atomic_json(&root.path().join("installer.json"), &json!({
            "filename":"tradeassembly-distribution","sha256":digest(&root.path().join("installer")).unwrap()
        })).unwrap();
        receipt["artifacts"]["installerDescriptor"] = proof(root.path(), "installer.json");
        assert_eq!(
            verify_delivery_artifacts(root.path(), &receipt, &release).unwrap_err(),
            "qualified_platform_package_binding_failed"
        );
    }

    #[test]
    fn delivery_binding_rejects_changed_launcher_and_path_escape() {
        let root = tempfile::tempdir().unwrap();
        let (mut receipt, release) = delivery_fixture(root.path());
        receipt["launcherSha256"] = json!("0".repeat(64));
        assert_eq!(
            verify_delivery_artifacts(root.path(), &receipt, &release).unwrap_err(),
            "qualified_launcher_package_binding_failed"
        );
        receipt["artifacts"]["npmPlatform"]["artifact"] = json!("../platform.tgz");
        assert_eq!(
            verify_delivery_artifacts(root.path(), &receipt, &release).unwrap_err(),
            "unsafe_payload_path"
        );
    }

    #[test]
    fn manifest_only_v1_receipts_cannot_qualify() {
        let root = tempfile::tempdir().unwrap();
        atomic_json(&root.path().join("matrix.json"), &json!({"schemaVersion":1,"parentLockSha256":PARENT_SHA,"version":"0.1.0-beta.1","npmTag":"beta"})).unwrap();
        let target = root.path().join(TARGETS[0]);
        fs::create_dir(&target).unwrap();
        let release = Release {
            schema_version: 1,
            version: "0.1.0-beta.1".into(),
            target: TARGETS[0].into(),
            parent_lock_sha256: PARENT_SHA.into(),
            bundle_manifest_sha256: crate::FROZEN_MAC_SHA.into(),
            archive_sha256: "a".repeat(64),
            warden_sha256: "b".repeat(64),
            state_compatibility: "f2-local-v1".into(),
            candidate_descriptor_sha256: None,
        };
        atomic_json(&target.join("release.json"), &release).unwrap();
        atomic_json(&target.join("receipt.json"), &json!({
            "schemaVersion":"tradeassembly.distribution-native.v1","target":release.target,
            "version":release.version,"releaseManifestSha256":digest(&target.join("release.json")).unwrap(),
            "native":true,"realWarden":true,"actualBrokerOrders":false,"liveActivated":false
        })).unwrap();
        assert_eq!(
            verify_matrix(root.path(), false).unwrap_err(),
            format!("native_qualification_binding_failed:{}", TARGETS[0])
        );
    }

    #[test]
    fn baseline_receipt_cannot_qualify_replacement_candidate() {
        let root = tempfile::tempdir().unwrap();
        atomic_json(
            &root.path().join("matrix.json"),
            &json!({
                "schemaVersion":1,"parentLockSha256":PARENT_SHA,
                "version":"0.1.0-beta.2","npmTag":"beta"
            }),
        )
        .unwrap();
        let target = root.path().join(TARGETS[0]);
        fs::create_dir(&target).unwrap();
        let release = Release {
            schema_version: 2,
            version: "0.1.0-beta.2".into(),
            target: TARGETS[0].into(),
            parent_lock_sha256: PARENT_SHA.into(),
            bundle_manifest_sha256: "a".repeat(64),
            archive_sha256: "b".repeat(64),
            warden_sha256: "c".repeat(64),
            state_compatibility: "f2-local-v1".into(),
            candidate_descriptor_sha256: Some("d".repeat(64)),
        };
        atomic_json(&target.join("release.json"), &release).unwrap();
        atomic_json(
            &target.join("receipt.json"),
            &json!({
                "schemaVersion":"tradeassembly.distribution-native.v2",
                "target":TARGETS[0],"version":release.version,
                "releaseManifestSha256":crate::FROZEN_MAC_SHA,
                "native":true,"host":{"os":"macos","arch":"aarch64","target":TARGETS[0]},
                "realWarden":true,"actualBrokerOrders":false,"liveActivated":false
            }),
        )
        .unwrap();
        assert_eq!(
            verify_matrix(root.path(), false).unwrap_err(),
            format!("native_qualification_binding_failed:{}", TARGETS[0])
        );
    }
    #[test]
    fn missing_native_proofs_never_pass_distribution_gate() {
        let temp = tempfile::tempdir().unwrap();
        atomic_json(&temp.path().join("matrix.json"), &json!({"schemaVersion":1,"parentLockSha256":PARENT_SHA,"version":"0.1.0-beta.1","npmTag":"beta"})).unwrap();
        assert_eq!(
            verify_matrix(temp.path(), true).unwrap_err(),
            format!("native_qualification_missing:{}", TARGETS[0])
        );
    }
}
