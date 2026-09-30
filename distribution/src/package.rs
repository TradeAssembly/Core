// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::{
    archive, atomic_json, digest, read_json, verify_bundle, Release, Result, PARENT_SHA, TARGETS,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::Command,
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

fn first_release_platform_metadata_valid(package: &Value, index: usize) -> bool {
    FIRST_RELEASE_INDICES.contains(&index)
        && package["tradeassemblyReleasePolicy"] == FIRST_RELEASE_POLICY
        && package["tradeassemblyPlatformQualification"] == platform_qualification(index)
}

fn first_release_launcher_metadata_valid(package: &Value) -> bool {
    package["tradeassemblyReleasePolicy"] == FIRST_RELEASE_POLICY
        && package["optionalDependencies"]
            .as_object()
            .is_some_and(|dependencies| {
                dependencies.len() == FIRST_RELEASE_INDICES.len()
                    && FIRST_RELEASE_INDICES
                        .iter()
                        .all(|index| dependencies[NPM_NAMES[*index]] == package["version"])
            })
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
        || (release.schema_version == 2 && !first_release_platform_metadata_valid(&package, index))
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
        let native_binary = |name: &str| {
            if release.target == TARGETS[4] {
                format!("{name}.exe")
            } else {
                name.to_owned()
            }
        };
        for name in [
            "bin/tradeassembly",
            "bin/warden",
            "bin/tradeassembly-sandbox",
        ] {
            crate::native::binary_target(
                &candidate_stage.path().join(native_binary(name)),
                &release.target,
            )?;
        }
        let node = if release.target == TARGETS[4] {
            "runtime/node/bin/node.exe"
        } else {
            "runtime/node/bin/node"
        };
        crate::native::binary_target(&candidate_stage.path().join(node), &release.target)?;
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
        || (release.schema_version == 2 && !first_release_launcher_metadata_valid(&package))
        || (release.schema_version == 1
            && (package["optionalDependencies"]
                .as_object()
                .is_none_or(|dependencies| dependencies.len() != NPM_NAMES.len())
                || NPM_NAMES
                    .iter()
                    .any(|name| package["optionalDependencies"][name] != release.version)))
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

// The supported first-release matrix deliberately excludes Intel macOS. The
// full TARGETS/NPM_NAMES inventory remains available to read older artifacts.
pub const FIRST_RELEASE_INDICES: [usize; 4] = [0, 2, 3, 4];
pub const FIRST_RELEASE_POLICY: &str = "macos-arm64-qualified-experimental-v1";

fn platform_qualification(index: usize) -> &'static str {
    if index == 0 {
        "required"
    } else {
        "deferred-experimental"
    }
}

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
    if descriptor.is_some() && !FIRST_RELEASE_INDICES.contains(&index) {
        return Err("first_release_target_excluded".into());
    }
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
            "tradeassemblyReleasePolicy":FIRST_RELEASE_POLICY,
            "tradeassemblyPlatformQualification":platform_qualification(index),
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
    let release_indices: &[usize] = if descriptor.is_some() {
        &FIRST_RELEASE_INDICES
    } else {
        &[0, 1, 2, 3, 4]
    };
    let deps: BTreeMap<_, _> = release_indices
        .iter()
        .map(|index| (NPM_NAMES[*index], version))
        .collect();
    atomic_json(
        &launcher.join("package.json"),
        &json!({
            "name":"tradeassembly","version":version,"description":"Local-first trading tooling. Users supply and authorize all strategies.",
            "license":"Apache-2.0","bin":{"tradeassembly":"cli.cjs"},"files":["cli.cjs","LICENSE","NOTICE","README.md"],
            "tradeassemblyReleasePolicy":FIRST_RELEASE_POLICY,
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
fn verify_windows_native_proof(root: &Path, receipt: &Value) -> Result<()> {
    if receipt["checks"]["windowsSandboxElevation"]["artifact"] != "installed-package.log"
        || receipt["checks"]["windowsPrivateAcl"]["artifact"] != "windows-private-acl.json"
        || receipt["checks"]["windowsSandboxAccount"]["artifact"] != "sandbox.log"
        || receipt["checks"]["windowsSandboxWfp"]["artifact"] != "sandbox.log"
    {
        return Err("native_windows_proof_source_invalid".into());
    }
    let installed: Value = read_json(&root.join("installed-package.log"))?;
    let acl: Value = read_json(&root.join("windows-private-acl.json"))?;
    let sandbox = fs::read_to_string(root.join("sandbox.log"))
        .map_err(|_| "native_windows_sandbox_log_unavailable")?;
    if installed["installed"] != true
        || installed["windowsSandboxProvisionedNow"] != true
        || installed["windowsSandboxBehavioralReady"] != true
        || acl["schemaVersion"] != "tradeassembly.windows-private-acl.v1"
        || acl["ownerOnlyState"] != true
        || acl["ownerOnlyAuthorityFiles"] != true
        || !sandbox
            .contains("packaged_sandbox_runs_and_denies_files_and_network_without_host_node ... ok")
        || !sandbox.contains("1 passed")
    {
        return Err("native_windows_proof_semantics_invalid".into());
    }
    Ok(())
}

pub fn verify_matrix(root: &Path, published: bool) -> Result<Value> {
    let matrix: Value = read_json(&root.join("matrix.json"))?;
    if matrix["parentLockSha256"] != PARENT_SHA || matrix["npmTag"] != "beta" {
        return Err("distribution_matrix_invalid".into());
    }
    let version = matrix["version"]
        .as_str()
        .ok_or("distribution_version_missing")?;
    if matrix["schemaVersion"] == 2 {
        return verify_first_release_matrix(root, &matrix, version, published);
    }
    if matrix["schemaVersion"] != 1 {
        return Err("distribution_matrix_invalid".into());
    }
    let qualified = verify_native_targets(root, &TARGETS, version, published)?;
    Ok(
        json!({"schemaVersion":1,"qualified":true,"phase":if published {"published"} else {"candidate"},"releaseReady":published,"commercialReleaseReady":false,"m7":false,"m8":false,"version":version,"targets":qualified,"npmTag":"beta","appleNotarized":false}),
    )
}

fn verify_native_targets<'a>(
    root: &Path,
    targets: &[&'a str],
    version: &str,
    published: bool,
) -> Result<Vec<&'a str>> {
    let mut qualified = Vec::new();
    for &target in targets {
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
            verify_windows_native_proof(&root.join(target), &receipt)?;
        }
        qualified.push(target);
    }
    Ok(qualified)
}

fn first_release_matrix_valid(matrix: &Value) -> bool {
    if matrix["releasePolicy"] != FIRST_RELEASE_POLICY {
        return false;
    }
    matrix["targetQualification"]
        .as_object()
        .is_some_and(|qualifications| {
            qualifications.len() == FIRST_RELEASE_INDICES.len()
                && FIRST_RELEASE_INDICES
                    .iter()
                    .all(|index| qualifications[TARGETS[*index]] == platform_qualification(*index))
        })
}

fn verify_codebuild_readbacks(root: &Path, receipt: &Value) -> Result<()> {
    for owner in ["core", "warden", "alpaca"] {
        let proof = &receipt["buildReadbacks"][owner];
        let file = evidence_file(root, proof)?;
        let readback: Value = read_json(&file)?;
        if readback["schemaVersion"] != "tradeassembly.codebuild-sanitized.v1"
            || readback.as_object().is_none_or(|fields| fields.len() != 2)
        {
            return Err("experimental_codebuild_readback_invalid".into());
        }
        let builds = readback["builds"]
            .as_array()
            .ok_or("experimental_codebuild_readback_invalid")?;
        if builds.len() != 1 {
            return Err("experimental_codebuild_readback_invalid".into());
        }
        let build = &builds[0];
        if build.as_object().is_none_or(|fields| fields.len() != 3) {
            return Err("experimental_codebuild_readback_invalid".into());
        }
        let revision = receipt["sourceRevisions"][owner]
            .as_str()
            .ok_or("experimental_source_revision_missing")?;
        let arn = build["arn"]
            .as_str()
            .ok_or("experimental_codebuild_readback_invalid")?;
        if revision.len() != 40
            || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !arn.starts_with("arn:aws:codebuild:")
            || build["buildStatus"] != "SUCCEEDED"
            || build["resolvedSourceVersion"] != revision
            || receipt["buildArns"][owner] != arn
        {
            return Err("experimental_codebuild_readback_invalid".into());
        }
    }
    Ok(())
}

fn verify_experimental_target(root: &Path, target: &str, version: &str) -> Result<()> {
    let target_root = root.join(target);
    let receipt: Value = read_json(&target_root.join("receipt.json"))
        .map_err(|_| format!("experimental_build_missing:{target}"))?;
    let release_path = target_root.join("release.json");
    let release: Release = read_json(&release_path)?;
    release.validate_candidate()?;
    if release.schema_version != 2
        || release.target != target
        || release.version != version
        || receipt["schemaVersion"] != "tradeassembly.distribution-experimental-build.v1"
        || receipt["qualification"] != "deferred-experimental"
        || receipt["target"] != target
        || receipt["version"] != version
        || receipt["releaseManifestSha256"] != digest(&release_path)?
    {
        return Err(format!("experimental_build_binding_failed:{target}"));
    }
    verify_codebuild_readbacks(&target_root, &receipt)
        .map_err(|code| format!("experimental_build_provenance_failed:{target}:{code}"))?;
    verify_delivery_artifacts(&target_root, &receipt, &release)
        .map_err(|code| format!("experimental_delivery_binding_failed:{target}:{code}"))?;
    let platform = evidence_file(&target_root, &receipt["artifacts"]["npmPlatform"])?;
    let unpacked = tempfile::tempdir().map_err(|_| "experimental_output_stage_failed")?;
    crate::extract(&platform, unpacked.path())?;
    let descriptor: Value =
        read_json(&unpacked.path().join("package").join(crate::candidate::NAME))?;
    for (name, field) in [
        ("core", "coreSha256"),
        ("warden", "wardenSha256"),
        ("sandbox", "launcherSha256"),
        ("node", "nodeSha256"),
        ("alpaca", "alpacaPackageSha256"),
    ] {
        let output = evidence_file(&target_root, &receipt["buildOutputs"][name])?;
        if descriptor["nativeInputs"][field] != digest(&output)? {
            return Err(format!(
                "experimental_build_output_mismatch:{target}:{name}"
            ));
        }
    }
    Ok(())
}

fn sha512_integrity(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|_| "registry_tarball_unavailable")?;
    let mut hash = Sha512::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "registry_tarball_read_failed")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("sha512-{}", STANDARD.encode(hash.finalize())))
}

fn verify_registry_package(
    root: &Path,
    registry: &Value,
    name: &str,
    version: &str,
    qualified_tarball: &Path,
) -> Result<()> {
    let record = &registry["packages"][name];
    let metadata = &record["metadata"];
    let tarball = evidence_file(&root.join("registry"), &record["tarball"])?;
    let expected_url = format!("https://registry.npmjs.org/{name}/-/{name}-{version}.tgz");
    if metadata["name"] != name
        || metadata["version"] != version
        || metadata["dist"]["tarball"] != expected_url
        || metadata["dist"]["integrity"] != sha512_integrity(&tarball)?
        || record["betaTagVersion"] != version
        || digest(&tarball)? != digest(qualified_tarball)?
    {
        return Err(format!("registry_package_binding_failed:{name}"));
    }
    Ok(())
}

fn verify_first_release_registry(root: &Path, version: &str) -> Result<()> {
    let registry: Value =
        read_json(&root.join("registry/registry.json")).map_err(|_| "registry_evidence_missing")?;
    if registry["schemaVersion"] != "tradeassembly.npm-registry.v1"
        || registry["registryBase"] != "https://registry.npmjs.org"
        || registry["version"] != version
        || registry["packages"]
            .as_object()
            .is_none_or(|packages| packages.len() != FIRST_RELEASE_INDICES.len() + 1)
    {
        return Err("registry_evidence_invalid".into());
    }
    let mut launcher_sha: Option<String> = None;
    for index in FIRST_RELEASE_INDICES {
        let target_root = root.join(TARGETS[index]);
        let receipt: Value = read_json(&target_root.join("receipt.json"))?;
        let platform = evidence_file(&target_root, &receipt["artifacts"]["npmPlatform"])?;
        let launcher = evidence_file(&target_root, &receipt["artifacts"]["npmLauncher"])?;
        verify_registry_package(root, &registry, NPM_NAMES[index], version, &platform)?;
        let current_launcher_sha = digest(&launcher)?;
        if launcher_sha
            .as_ref()
            .is_some_and(|expected| expected != &current_launcher_sha)
        {
            return Err("registry_candidate_launcher_mismatch".into());
        }
        launcher_sha = Some(current_launcher_sha);
        if index == 0 {
            verify_registry_package(root, &registry, "tradeassembly", version, &launcher)?;
        }
    }
    Ok(())
}

fn npm_json(arguments: &[&str]) -> Result<Value> {
    let output = Command::new("npm")
        .args(arguments)
        .args(["--registry=https://registry.npmjs.org", "--json"])
        .output()
        .map_err(|_| "npm_registry_command_unavailable")?;
    if !output.status.success() || output.stdout.len() > 4_194_304 {
        return Err("npm_registry_command_failed".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "npm_registry_response_invalid".into())
}

fn capture_registry_package(
    stage: &Path,
    name: &str,
    version: &str,
    qualified_tarball: &Path,
) -> Result<Value> {
    let spec = format!("{name}@{version}");
    let metadata = npm_json(&["view", &spec])?;
    let beta = npm_json(&["view", &format!("{name}@beta"), "version"])?;
    if beta != version {
        return Err(format!("registry_beta_tag_mismatch:{name}"));
    }
    let packed = npm_json(&[
        "pack",
        &spec,
        "--ignore-scripts",
        "--pack-destination",
        stage.to_str().ok_or("registry_stage_path_invalid")?,
    ])?;
    let entries = packed.as_array().ok_or("npm_pack_response_invalid")?;
    if entries.len() != 1 {
        return Err("npm_pack_response_invalid".into());
    }
    let filename = entries[0]["filename"]
        .as_str()
        .ok_or("npm_pack_response_invalid")?;
    let expected_filename = format!("{name}-{version}.tgz");
    if filename != expected_filename {
        return Err("npm_pack_filename_invalid".into());
    }
    let downloaded = stage.join(filename);
    if !downloaded.is_file() || digest(&downloaded)? != digest(qualified_tarball)? {
        return Err(format!("registry_candidate_bytes_mismatch:{name}"));
    }
    let record = json!({
        "tarball":{"artifact":filename,"sha256":digest(&downloaded)?},
        "betaTagVersion":version,
        "metadata":{"name":metadata["name"],"version":metadata["version"],
                    "dist":{"tarball":metadata["dist"]["tarball"],
                            "integrity":metadata["dist"]["integrity"]}}
    });
    // Validate the downloaded bytes against npm's own integrity metadata now;
    // the final offline verifier repeats this against retained evidence.
    let proof = &record["metadata"];
    if proof["name"] != name
        || proof["version"] != version
        || proof["dist"]["integrity"] != sha512_integrity(&downloaded)?
        || proof["dist"]["tarball"]
            != format!("https://registry.npmjs.org/{name}/-/{name}-{version}.tgz")
    {
        return Err(format!("registry_metadata_mismatch:{name}"));
    }
    Ok(record)
}

/// Read-only npm capture. Publication itself remains a separately authorized
/// R5 action; this command never publishes or mutates the npm account.
pub fn capture_registry(root: &Path) -> Result<Value> {
    let matrix: Value = read_json(&root.join("matrix.json"))?;
    if matrix["schemaVersion"] != 2 || !first_release_matrix_valid(&matrix) {
        return Err("first_release_matrix_policy_invalid".into());
    }
    let version = matrix["version"]
        .as_str()
        .ok_or("distribution_version_missing")?;
    verify_first_release_matrix(root, &matrix, version, false)?;
    if root.join("registry").exists() {
        return Err("registry_evidence_already_exists".into());
    }
    let stage = tempfile::Builder::new()
        .prefix("registry-capture-")
        .tempdir_in(root)
        .map_err(|_| "registry_stage_failed")?;
    let mut packages = serde_json::Map::new();
    for index in FIRST_RELEASE_INDICES {
        let target_root = root.join(TARGETS[index]);
        let receipt: Value = read_json(&target_root.join("receipt.json"))?;
        let platform = evidence_file(&target_root, &receipt["artifacts"]["npmPlatform"])?;
        let launcher = evidence_file(&target_root, &receipt["artifacts"]["npmLauncher"])?;
        packages.insert(
            NPM_NAMES[index].into(),
            capture_registry_package(stage.path(), NPM_NAMES[index], version, &platform)?,
        );
        if index == 0 {
            packages.insert(
                "tradeassembly".into(),
                capture_registry_package(stage.path(), "tradeassembly", version, &launcher)?,
            );
        }
    }
    atomic_json(
        &stage.path().join("registry.json"),
        &json!({"schemaVersion":"tradeassembly.npm-registry.v1",
                "registryBase":"https://registry.npmjs.org",
                "version":version,"packages":packages}),
    )?;
    fs::rename(stage.path(), root.join("registry"))
        .map_err(|_| "registry_capture_commit_failed")?;
    verify_first_release_registry(root, version)?;
    Ok(json!({"captured":true,"version":version,"packages":FIRST_RELEASE_INDICES.len()+1}))
}

fn verify_first_release_matrix(
    root: &Path,
    matrix: &Value,
    version: &str,
    published: bool,
) -> Result<Value> {
    if !first_release_matrix_valid(matrix) {
        return Err("first_release_matrix_policy_invalid".into());
    }
    let qualified = verify_native_targets(root, &[TARGETS[0]], version, published)?;
    let mut experimental = Vec::new();
    for index in [2, 3, 4] {
        let target = TARGETS[index];
        verify_experimental_target(root, target, version)?;
        experimental.push(target);
    }
    if published {
        verify_first_release_registry(root, version)?;
        let mac = root.join(TARGETS[0]);
        let receipt: Value = read_json(&mac.join("receipt.json"))?;
        let proof = &receipt["checks"]["registryUpgradeRollback"];
        if proof["exitCode"] != 0 {
            return Err("registry_mac_upgrade_rollback_missing".into());
        }
        evidence_file(&mac, proof)
            .map_err(|code| format!("registry_mac_upgrade_rollback_invalid:{code}"))?;
    }
    Ok(json!({
        "schemaVersion":2,"qualified":true,"phase":if published {"published"} else {"candidate"},
        "releaseReady":published,
        "commercialReleaseReady":false,"m7":false,"m8":false,"version":version,
        "qualifiedTargets":qualified,"experimentalUnqualifiedTargets":experimental,
        "npmTag":"beta","appleNotarized":false,"releasePolicy":FIRST_RELEASE_POLICY
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_tarball_requires_candidate_bytes_integrity_url_and_beta_tag() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("registry")).unwrap();
        let candidate = root.path().join("qualified.tgz");
        let published = root.path().join("registry/published.tgz");
        fs::write(&candidate, b"qualified npm tarball fixture").unwrap();
        fs::copy(&candidate, &published).unwrap();
        let name = "tradeassembly-linux-x64";
        let version = "0.1.0-beta.3";
        let mut registry = json!({"packages":{name:{
            "tarball":proof(&root.path().join("registry"),"published.tgz"),
            "betaTagVersion":version,
            "metadata":{"name":name,"version":version,"dist":{
                "tarball":format!("https://registry.npmjs.org/{name}/-/{name}-{version}.tgz"),
                "integrity":sha512_integrity(&published).unwrap()
            }}
        }}});
        assert!(verify_registry_package(root.path(), &registry, name, version, &candidate).is_ok());
        registry["packages"][name]["metadata"]["dist"]["integrity"] = json!("sha512-wrong");
        assert_eq!(
            verify_registry_package(root.path(), &registry, name, version, &candidate).unwrap_err(),
            format!("registry_package_binding_failed:{name}")
        );
        registry["packages"][name]["metadata"]["dist"]["integrity"] =
            json!(sha512_integrity(&published).unwrap());
        registry["packages"][name]["betaTagVersion"] = json!("0.1.0-beta.2");
        assert!(
            verify_registry_package(root.path(), &registry, name, version, &candidate).is_err()
        );
        registry["packages"][name]["betaTagVersion"] = json!(version);
        fs::write(&published, b"changed registry tarball fixture").unwrap();
        registry["packages"][name]["tarball"] =
            proof(&root.path().join("registry"), "published.tgz");
        registry["packages"][name]["metadata"]["dist"]["integrity"] =
            json!(sha512_integrity(&published).unwrap());
        assert_eq!(
            verify_registry_package(root.path(), &registry, name, version, &candidate).unwrap_err(),
            format!("registry_package_binding_failed:{name}")
        );
    }

    #[test]
    fn first_release_policy_excludes_intel_mac_and_never_marks_experimental_qualified() {
        let launcher = json!({
            "version":"0.1.0-beta.3",
            "tradeassemblyReleasePolicy":FIRST_RELEASE_POLICY,
            "optionalDependencies":{
                NPM_NAMES[0]:"0.1.0-beta.3",
                NPM_NAMES[2]:"0.1.0-beta.3",
                NPM_NAMES[3]:"0.1.0-beta.3",
                NPM_NAMES[4]:"0.1.0-beta.3"
            }
        });
        assert!(first_release_launcher_metadata_valid(&launcher));
        let mut with_intel_mac = launcher.clone();
        with_intel_mac["optionalDependencies"][NPM_NAMES[1]] = json!("0.1.0-beta.3");
        assert!(!first_release_launcher_metadata_valid(&with_intel_mac));
        let mut missing_linux = launcher.clone();
        missing_linux["optionalDependencies"]
            .as_object_mut()
            .unwrap()
            .remove(NPM_NAMES[2]);
        assert!(!first_release_launcher_metadata_valid(&missing_linux));

        for index in FIRST_RELEASE_INDICES {
            let metadata = json!({
                "tradeassemblyReleasePolicy":FIRST_RELEASE_POLICY,
                "tradeassemblyPlatformQualification":platform_qualification(index)
            });
            assert!(first_release_platform_metadata_valid(&metadata, index));
            if index != 0 {
                let mut falsely_qualified = metadata;
                falsely_qualified["tradeassemblyPlatformQualification"] = json!("required");
                assert!(!first_release_platform_metadata_valid(
                    &falsely_qualified,
                    index
                ));
            }
        }
        assert!(!first_release_platform_metadata_valid(
            &json!({"tradeassemblyReleasePolicy":FIRST_RELEASE_POLICY,
                    "tradeassemblyPlatformQualification":"required"}),
            1
        ));
    }

    #[test]
    fn first_release_matrix_requires_exact_statuses_and_mac_evidence() {
        let root = tempfile::tempdir().unwrap();
        let mut matrix = json!({
            "schemaVersion":2,"parentLockSha256":PARENT_SHA,
            "npmTag":"beta","version":"0.1.0-beta.3",
            "releasePolicy":FIRST_RELEASE_POLICY,
            "targetQualification":{
                TARGETS[0]:"required",
                TARGETS[2]:"deferred-experimental",
                TARGETS[3]:"deferred-experimental",
                TARGETS[4]:"deferred-experimental"
            }
        });
        assert!(first_release_matrix_valid(&matrix));
        atomic_json(&root.path().join("matrix.json"), &matrix).unwrap();
        assert_eq!(
            verify_matrix(root.path(), false).unwrap_err(),
            format!("native_qualification_missing:{}", TARGETS[0])
        );
        assert_eq!(
            capture_registry(root.path()).unwrap_err(),
            format!("native_qualification_missing:{}", TARGETS[0])
        );
        assert!(!root.path().join("registry").exists());
        matrix["targetQualification"][TARGETS[2]] = json!("required");
        assert!(!first_release_matrix_valid(&matrix));
        atomic_json(&root.path().join("matrix.json"), &matrix).unwrap();
        assert_eq!(
            verify_matrix(root.path(), false).unwrap_err(),
            "first_release_matrix_policy_invalid"
        );
        matrix["targetQualification"][TARGETS[2]] = json!("deferred-experimental");
        matrix["targetQualification"][TARGETS[1]] = json!("required");
        assert!(!first_release_matrix_valid(&matrix));
    }

    #[test]
    fn codebuild_readbacks_bind_all_three_producer_revisions_and_sanitize_fields() {
        let root = tempfile::tempdir().unwrap();
        let mut receipt = json!({"sourceRevisions":{},"buildArns":{},"buildReadbacks":{}});
        for (owner, revision) in [("core", "a"), ("warden", "b"), ("alpaca", "c")] {
            let revision = revision.repeat(40);
            let arn = format!("arn:aws:codebuild:us-east-1:123456789012:build/{owner}:fixture");
            let file = format!("{owner}-readback.json");
            atomic_json(
                &root.path().join(&file),
                &json!({"schemaVersion":"tradeassembly.codebuild-sanitized.v1",
                    "builds":[{"arn":arn,"buildStatus":"SUCCEEDED",
                               "resolvedSourceVersion":revision}]}),
            )
            .unwrap();
            receipt["sourceRevisions"][owner] = json!(revision);
            receipt["buildArns"][owner] = json!(arn);
            receipt["buildReadbacks"][owner] = proof(root.path(), &file);
        }
        assert!(verify_codebuild_readbacks(root.path(), &receipt).is_ok());
        receipt["sourceRevisions"]["warden"] = json!("d".repeat(40));
        assert_eq!(
            verify_codebuild_readbacks(root.path(), &receipt).unwrap_err(),
            "experimental_codebuild_readback_invalid"
        );
        receipt["sourceRevisions"]["warden"] = json!("b".repeat(40));
        let file = root.path().join("core-readback.json");
        atomic_json(
            &file,
            &json!({"schemaVersion":"tradeassembly.codebuild-sanitized.v1",
                "builds":[{"arn":receipt["buildArns"]["core"],"buildStatus":"SUCCEEDED",
                           "resolvedSourceVersion":"a".repeat(40),"environment":{"secret":"must-reject"}}]}),
        )
        .unwrap();
        receipt["buildReadbacks"]["core"] = proof(root.path(), "core-readback.json");
        assert_eq!(
            verify_codebuild_readbacks(root.path(), &receipt).unwrap_err(),
            "experimental_codebuild_readback_invalid"
        );
    }

    #[test]
    fn windows_native_proof_rejects_missing_setup_behavior_and_rebound_logs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        atomic_json(
            &root.join("installed-package.log"),
            &json!({"installed":true,"windowsSandboxProvisionedNow":true,"windowsSandboxBehavioralReady":true}),
        )
        .unwrap();
        atomic_json(
            &root.join("windows-private-acl.json"),
            &json!({"schemaVersion":"tradeassembly.windows-private-acl.v1","ownerOnlyState":true,"ownerOnlyAuthorityFiles":true}),
        )
        .unwrap();
        fs::write(root.join("sandbox.log"), "test packaged_sandbox_runs_and_denies_files_and_network_without_host_node ... ok\ntest result: ok. 1 passed; 0 failed;").unwrap();
        let receipt = json!({"checks":{
            "windowsSandboxElevation":{"artifact":"installed-package.log"},
            "windowsPrivateAcl":{"artifact":"windows-private-acl.json"},
            "windowsSandboxAccount":{"artifact":"sandbox.log"},
            "windowsSandboxWfp":{"artifact":"sandbox.log"}
        }});
        assert!(verify_windows_native_proof(root, &receipt).is_ok());
        let mut wrong = receipt.clone();
        wrong["checks"]["windowsSandboxWfp"]["artifact"] = json!("unrelated.log");
        assert_eq!(
            verify_windows_native_proof(root, &wrong).unwrap_err(),
            "native_windows_proof_source_invalid"
        );
        atomic_json(
            &root.join("installed-package.log"),
            &json!({"installed":true,"windowsSandboxProvisionedNow":true,"windowsSandboxBehavioralReady":false}),
        )
        .unwrap();
        assert_eq!(
            verify_windows_native_proof(root, &receipt).unwrap_err(),
            "native_windows_proof_semantics_invalid"
        );
    }

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
