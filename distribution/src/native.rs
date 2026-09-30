// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Freeze explicitly staged native build inputs. This is not qualification.

use crate::{
    atomic_json, digest, executable, portable_name, read_json, verify_bundle, Release, Result,
    PARENT_SHA,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, fs, io::Read, path::Path};

const NODE_VERSION: &str = "v22.23.2";
const SANDBOX_VERSION: &str = "0.0.67";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeInputs {
    pub schema_version: u32,
    pub target: String,
    pub core_revision: String,
    pub warden_revision: String,
    pub alpaca_revision: String,
    pub core_sha256: String,
    pub warden_sha256: String,
    pub launcher_sha256: String,
    pub node_sha256: String,
    pub alpaca_package: String,
    pub alpaca_package_sha256: String,
    pub alpaca_manifest_sha256: String,
}

fn binary_target(path: &Path, target: &str) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path).map_err(|_| "native_binary_unavailable")?;
    let mut header = [0; 64];
    file.read_exact(&mut header)
        .map_err(|_| "native_binary_invalid")?;
    let valid = match target {
        "aarch64-apple-darwin" => header[..8] == [0xcf, 0xfa, 0xed, 0xfe, 12, 0, 0, 1],
        "x86_64-apple-darwin" => header[..8] == [0xcf, 0xfa, 0xed, 0xfe, 7, 0, 0, 1],
        "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu" => {
            let machine = if target.starts_with("x86_64") {
                62
            } else {
                183
            };
            header[..6] == [0x7f, b'E', b'L', b'F', 2, 1]
                && u16::from_le_bytes([header[18], header[19]]) == machine
        }
        "x86_64-pc-windows-msvc" if header[..2] == *b"MZ" => {
            let offset = u32::from_le_bytes(header[60..64].try_into().unwrap());
            if !(64..=65536).contains(&offset) {
                return Err("native_binary_invalid".into());
            }
            file.seek(SeekFrom::Start(u64::from(offset)))
                .map_err(|_| "native_binary_invalid")?;
            let mut pe = [0; 6];
            file.read_exact(&mut pe)
                .map_err(|_| "native_binary_invalid")?;
            pe == [b'P', b'E', 0, 0, 0x64, 0x86]
        }
        _ => false,
    };
    if !valid {
        return Err("native_binary_target_mismatch".into());
    }
    Ok(())
}

fn sandbox_pin(root: &Path) -> Result<()> {
    let lock: serde_json::Value = read_json(&root.join("runtime/package-lock.json"))?;
    let installed: serde_json::Value =
        read_json(&root.join("runtime/node_modules/@anthropic-ai/sandbox-runtime/package.json"))?;
    if lock["packages"]["node_modules/@anthropic-ai/sandbox-runtime"]["version"] != SANDBOX_VERSION
        || installed["version"] != SANDBOX_VERSION
    {
        return Err("native_sandbox_pin_invalid".into());
    }
    Ok(())
}

fn plugin_pin(root: &Path, spec: &NativeInputs) -> Result<()> {
    let lock: serde_json::Value = read_json(&root.join("plugins/alpaca.json"))?;
    if lock["target"] != spec.target
        || lock["version"] != "0.1.17"
        || lock["sourceRevision"] != spec.alpaca_revision
        || lock["packageFile"] != spec.alpaca_package
        || lock["packageSha256"] != spec.alpaca_package_sha256
        || lock["manifestSha256"] != spec.alpaca_manifest_sha256
        || !crate::is_digest(&spec.alpaca_package_sha256)
        || digest(&root.join("plugins").join(&spec.alpaca_package))? != spec.alpaca_package_sha256
    {
        return Err("native_plugin_lock_binding_failed".into());
    }
    Ok(())
}

fn node_pin(binary: &Path) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut output = tempfile::tempfile().map_err(|_| "native_node_version_unavailable")?;
    let mut child = Command::new(binary)
        .arg("--version")
        .env_remove("NODE_OPTIONS")
        .env_remove("NODE_PATH")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(
            output
                .try_clone()
                .map_err(|_| "native_node_version_unavailable")?,
        )
        .spawn()
        .map_err(|_| "native_node_version_unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("native_node_version_timeout".into());
            }
        }
    };
    output
        .seek(SeekFrom::Start(0))
        .map_err(|_| "native_node_version_unavailable")?;
    let mut bytes = Vec::new();
    output
        .take(129)
        .read_to_end(&mut bytes)
        .map_err(|_| "native_node_version_unavailable")?;
    if !status.success()
        || bytes.len() > 128
        || std::str::from_utf8(&bytes).map(str::trim) != Ok(NODE_VERSION)
    {
        return Err("native_node_pin_invalid".into());
    }
    Ok(())
}

pub fn freeze(input: &Path, metadata: &Path, parent: &Path, out: &Path) -> Result<()> {
    let spec: NativeInputs = read_json(metadata)?;
    if digest(parent)? != PARENT_SHA || spec.schema_version != 1 {
        return Err("native_input_contract_invalid".into());
    }
    if spec.target != crate::install::host_target()? {
        return Err("native_host_required".into());
    }
    for revision in [
        &spec.core_revision,
        &spec.warden_revision,
        &spec.alpaca_revision,
    ] {
        if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("native_source_revision_invalid".into());
        }
    }
    crate::relative(Path::new(&spec.alpaca_package))?;
    if Path::new(&spec.alpaca_package).components().count() != 1
        || !spec.alpaca_package.ends_with(".tar.gz")
        || !crate::is_digest(&spec.alpaca_manifest_sha256)
    {
        return Err("native_plugin_metadata_invalid".into());
    }
    if out.exists() || input.join("bundle.json").exists() {
        return Err("native_staging_or_output_already_frozen".into());
    }
    let parent_out = out.parent().ok_or("output_path_invalid")?;
    fs::create_dir_all(parent_out).map_err(|_| "output_create_failed")?;
    let stage = tempfile::tempdir_in(parent_out).map_err(|_| "output_create_failed")?;
    let source = input
        .canonicalize()
        .map_err(|_| "native_staging_unavailable")?;
    for entry in walkdir::WalkDir::new(&source)
        .min_depth(1)
        .follow_links(false)
    {
        let entry = entry.map_err(|_| "native_staging_invalid")?;
        let relative = entry
            .path()
            .strip_prefix(&source)
            .map_err(|_| "native_staging_invalid")?;
        let name = portable_name(relative)?;
        let top = name.split('/').next().unwrap_or_default();
        if !matches!(
            top,
            "bin" | "runtime" | "plugins" | "LICENSE" | "NOTICE" | "SETUP.md"
        ) {
            return Err("native_staging_unexpected_surface".into());
        }
        let destination = stage.path().join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination).map_err(|_| "native_copy_failed")?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|_| "native_copy_failed")?;
            }
            fs::copy(entry.path(), &destination).map_err(|_| "native_copy_failed")?;
        } else if entry.file_type().is_symlink() {
            if !entry
                .path()
                .canonicalize()
                .map_err(|_| "native_link_invalid")?
                .starts_with(&source)
            {
                return Err("native_link_escape".into());
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(
                fs::read_link(entry.path()).map_err(|_| "native_link_invalid")?,
                &destination,
            )
            .map_err(|_| "native_copy_failed")?;
            #[cfg(not(unix))]
            return Err("native_windows_staging_links_forbidden".into());
        } else {
            return Err("native_special_file_forbidden".into());
        }
    }
    let node = if cfg!(windows) {
        "runtime/node/bin/node.exe"
    } else {
        "runtime/node/bin/node"
    };
    for (name, expected) in [
        (executable("bin/tradeassembly"), &spec.core_sha256),
        (executable("bin/warden"), &spec.warden_sha256),
        (
            executable("bin/tradeassembly-sandbox"),
            &spec.launcher_sha256,
        ),
        (node.to_string(), &spec.node_sha256),
    ] {
        if !crate::is_digest(expected) || digest(&stage.path().join(&name))? != *expected {
            return Err("native_input_digest_mismatch".into());
        }
        binary_target(&stage.path().join(&name), &spec.target)?;
    }
    node_pin(&stage.path().join(node))?;
    plugin_pin(stage.path(), &spec)?;
    for name in ["LICENSE", "NOTICE", "SETUP.md", "runtime/node/LICENSE"] {
        if fs::metadata(stage.path().join(name))
            .map_err(|_| "native_notice_missing")?
            .len()
            == 0
        {
            return Err("native_notice_empty".into());
        }
    }
    sandbox_pin(stage.path())?;
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(stage.path())
        .min_depth(1)
        .follow_links(false)
    {
        let entry = entry.map_err(|_| "native_staging_invalid")?;
        if !entry.file_type().is_dir() {
            let name = portable_name(
                entry
                    .path()
                    .strip_prefix(stage.path())
                    .map_err(|_| "native_staging_invalid")?,
            )?;
            files.insert(name, json!({"sha256":digest(entry.path())?}));
        }
    }
    atomic_json(
        &stage.path().join("bundle.json"),
        &json!({
            "schemaVersion":"tradeassembly.local-bundle.v1", "target":spec.target,
            "releaseReady":false, "signed":false, "notarized":false,
            "sandboxVersion":"0.0.67", "sourceBuildRequiredForCustomer":false,
            "nativeInputs":spec, "files":files
        }),
    )?;
    let release = Release {
        schema_version: 1,
        version: "0.1.0-beta.1".into(),
        target: spec.target,
        parent_lock_sha256: PARENT_SHA.into(),
        bundle_manifest_sha256: digest(&stage.path().join("bundle.json"))?,
        archive_sha256: "0".repeat(64),
        warden_sha256: spec.warden_sha256,
        state_compatibility: "f2-local-v1".into(),
        candidate_descriptor_sha256: None,
    };
    verify_bundle(stage.path(), &release)?;
    fs::rename(stage.path(), out).map_err(|_| "native_freeze_failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TARGETS;

    #[test]
    fn native_plugin_lock_must_bind_every_input_not_a_copied_mac_lock() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("plugins")).unwrap();
        let package = "alpaca.tar.gz";
        fs::write(
            temp.path().join("plugins").join(package),
            b"package fixture",
        )
        .unwrap();
        let spec = NativeInputs {
            schema_version: 1,
            target: TARGETS[4].into(),
            core_revision: "a".repeat(40),
            warden_revision: "b".repeat(40),
            alpaca_revision: "c".repeat(40),
            core_sha256: "d".repeat(64),
            warden_sha256: "e".repeat(64),
            launcher_sha256: "f".repeat(64),
            node_sha256: "0".repeat(64),
            alpaca_package: package.into(),
            alpaca_package_sha256: digest(&temp.path().join("plugins").join(package)).unwrap(),
            alpaca_manifest_sha256: "1".repeat(64),
        };
        let lock = json!({"version":"0.1.17","target":spec.target,
            "sourceRevision":spec.alpaca_revision,"packageFile":spec.alpaca_package,
            "packageSha256":spec.alpaca_package_sha256,
            "manifestSha256":spec.alpaca_manifest_sha256});
        let path = temp.path().join("plugins/alpaca.json");
        atomic_json(&path, &lock).unwrap();
        assert!(plugin_pin(temp.path(), &spec).is_ok());
        for field in [
            "target",
            "version",
            "sourceRevision",
            "packageFile",
            "packageSha256",
            "manifestSha256",
        ] {
            let mut wrong = lock.clone();
            wrong[field] = json!("stale");
            atomic_json(&path, &wrong).unwrap();
            assert_eq!(
                plugin_pin(temp.path(), &spec).unwrap_err(),
                "native_plugin_lock_binding_failed"
            );
        }
        atomic_json(&path, &lock).unwrap();
        fs::write(
            temp.path().join("plugins").join(package),
            b"changed package",
        )
        .unwrap();
        assert_eq!(
            plugin_pin(temp.path(), &spec).unwrap_err(),
            "native_plugin_lock_binding_failed"
        );
    }

    #[test]
    fn binary_headers_reject_cross_target_and_truncation() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("native-binary");
        let mut header = [0_u8; 64];
        header[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        header[18..20].copy_from_slice(&62_u16.to_le_bytes());
        fs::write(&file, header).unwrap();
        assert!(binary_target(&file, TARGETS[2]).is_ok());
        assert_eq!(
            binary_target(&file, TARGETS[3]).unwrap_err(),
            "native_binary_target_mismatch"
        );
        assert!(binary_target(&file, TARGETS[1]).is_err());
        let mut arm_mach_o = [0_u8; 64];
        arm_mach_o[..8].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe, 12, 0, 0, 1]);
        fs::write(&file, arm_mach_o).unwrap();
        assert!(binary_target(&file, TARGETS[0]).is_ok());
        assert_eq!(
            binary_target(&file, TARGETS[1]).unwrap_err(),
            "native_binary_target_mismatch"
        );
        fs::write(&file, b"MZ").unwrap();
        assert_eq!(
            binary_target(&file, TARGETS[4]).unwrap_err(),
            "native_binary_invalid"
        );
    }

    #[test]
    fn windows_header_checks_machine_and_bounded_offset() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("binary.exe");
        let mut header = [0_u8; 70];
        header[..2].copy_from_slice(b"MZ");
        header[60..64].copy_from_slice(&64_u32.to_le_bytes());
        header[64..70].copy_from_slice(&[b'P', b'E', 0, 0, 0x64, 0x86]);
        fs::write(&file, header).unwrap();
        assert!(binary_target(&file, TARGETS[4]).is_ok());
        header[68] = 0;
        fs::write(&file, header).unwrap();
        assert!(binary_target(&file, TARGETS[4]).is_err());
        header[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&file, header).unwrap();
        assert_eq!(
            binary_target(&file, TARGETS[4]).unwrap_err(),
            "native_binary_invalid"
        );
    }

    #[test]
    fn installed_sandbox_must_match_locked_version() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp
            .path()
            .join("runtime/node_modules/@anthropic-ai/sandbox-runtime/package.json");
        fs::create_dir_all(package.parent().unwrap()).unwrap();
        atomic_json(&temp.path().join("runtime/package-lock.json"), &json!({
            "packages":{"node_modules/@anthropic-ai/sandbox-runtime":{"version":SANDBOX_VERSION}}
        })).unwrap();
        atomic_json(&package, &json!({"version":SANDBOX_VERSION})).unwrap();
        assert!(sandbox_pin(temp.path()).is_ok());
        atomic_json(&package, &json!({"version":"0.0.66"})).unwrap();
        assert_eq!(
            sandbox_pin(temp.path()).unwrap_err(),
            "native_sandbox_pin_invalid"
        );
    }
}
