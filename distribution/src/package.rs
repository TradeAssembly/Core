// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::{
    archive, atomic_json, digest, read_json, verify_bundle, Release, Result, PARENT_SHA, TARGETS,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

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
    let mut release = Release {
        schema_version: 1,
        version: version.into(),
        target: target.into(),
        parent_lock_sha256: PARENT_SHA.into(),
        bundle_manifest_sha256: digest(&bundle.join("bundle.json"))?,
        archive_sha256: "0".repeat(64),
        warden_sha256: digest(&bundle.join(crate::executable("bin/warden")))?,
        state_compatibility: "f2-local-v1".into(),
    };
    verify_bundle(bundle, &release)?;
    release.validate_candidate()?;
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
    atomic_json(
        &platform.join("package.json"),
        &json!({
            "name":NPM_NAMES[index],"version":version,"description":"TradeAssembly native F2 prerelease payload",
            "license":"Apache-2.0","os":[os],"cpu":[cpu],
            "files":["bundle.tar.gz","release.json","installer.json",installer_name,"LICENSE","NOTICE"],
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
        let receipt_path = root.join(target).join("receipt.json");
        let receipt: Value = read_json(&receipt_path)
            .map_err(|_| format!("native_qualification_missing:{target}"))?;
        let release: Release = read_json(&root.join(target).join("release.json"))?;
        release.validate_candidate()?;
        if release.target != target
            || release.version != version
            || receipt["schemaVersion"] != "tradeassembly.distribution-native.v1"
            || receipt["target"] != target
            || receipt["version"] != version
            || receipt["releaseManifestSha256"] != digest(&root.join(target).join("release.json"))?
            || receipt["native"] != true
            || receipt["realWarden"] != true
            || receipt["actualBrokerOrders"] != false
            || receipt["liveActivated"] != false
        {
            return Err(format!("native_qualification_binding_failed:{target}"));
        }
        let mut checks = vec![
            "frozenPayload",
            "npmLocalIgnoreScripts",
            "pnpmLocalIgnoreScripts",
            "sourceFreeSetup",
            "offlineAlpaca",
            "stoppedUpgradeRollback",
            "activeUpgradeDenied",
            "duplicateRecovery",
            "ambiguousOutcomeRecovery",
            "wardenControlledSink",
            "sandboxDeniedEgress",
            "sandboxDeniedFilesystem",
        ];
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
        qualified.push(target);
    }
    Ok(
        json!({"schemaVersion":1,"qualified":true,"phase":if published {"published"} else {"candidate"},"releaseReady":published,"version":version,"targets":qualified,"npmTag":"beta","appleNotarized":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
