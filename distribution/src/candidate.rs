// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Exact input binding for a replacement native candidate.

use crate::{
    atomic_json, digest, native::NativeInputs, read_json, verify_bundle, Release, Result,
    FROZEN_MAC_SHA, PARENT_SHA, TARGETS,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, path::Path};

pub const NAME: &str = "candidate-descriptor.json";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CandidateDescriptor {
    pub schema_version: u32,
    pub version: String,
    pub target: String,
    pub parent_lock_sha256: String,
    pub baseline_mac_manifest_sha256: String,
    pub bundle_manifest_sha256: String,
    pub native_inputs: NativeInputs,
}

pub fn describe(bundle: &Path, parent: &Path, version: &str, out: &Path) -> Result<Value> {
    if digest(parent)? != PARENT_SHA {
        return Err("parent_release_lock_changed".into());
    }
    if out.file_name().and_then(|name| name.to_str()) != Some(NAME) || out.exists() {
        return Err("candidate_descriptor_output_invalid".into());
    }
    let manifest: Value = read_json(&bundle.join("bundle.json"))?;
    let target = manifest["target"]
        .as_str()
        .ok_or("candidate_target_missing")?;
    let inputs: NativeInputs = serde_json::from_value(manifest["nativeInputs"].clone())
        .map_err(|_| "candidate_native_inputs_missing")?;
    let descriptor = CandidateDescriptor {
        schema_version: 1,
        version: version.into(),
        target: target.into(),
        parent_lock_sha256: PARENT_SHA.into(),
        baseline_mac_manifest_sha256: FROZEN_MAC_SHA.into(),
        bundle_manifest_sha256: digest(&bundle.join("bundle.json"))?,
        native_inputs: inputs,
    };
    let parent = out.parent().ok_or("candidate_descriptor_output_invalid")?;
    fs::create_dir_all(parent).map_err(|_| "candidate_descriptor_output_invalid")?;
    let stage = tempfile::tempdir_in(parent).map_err(|_| "candidate_descriptor_output_invalid")?;
    let staged = stage.path().join(NAME);
    atomic_json(&staged, &descriptor)?;
    let release = Release {
        schema_version: 2,
        version: version.into(),
        target: target.into(),
        parent_lock_sha256: PARENT_SHA.into(),
        bundle_manifest_sha256: descriptor.bundle_manifest_sha256.clone(),
        archive_sha256: "0".repeat(64),
        warden_sha256: descriptor.native_inputs.warden_sha256.clone(),
        state_compatibility: "f2-local-v1".into(),
        candidate_descriptor_sha256: Some(digest(&staged)?),
    };
    verify_bundle(bundle, &release)?;
    verify(stage.path(), bundle, &release)?;
    fs::hard_link(&staged, out).map_err(|_| "candidate_descriptor_commit_failed")?;
    Ok(
        json!({"descriptor":out,"sha256":release.candidate_descriptor_sha256,
        "version":version,"target":target,"qualified":false}),
    )
}

pub fn verify(package: &Path, bundle: &Path, release: &Release) -> Result<()> {
    release.validate_candidate()?;
    let descriptor_path = package.join(NAME);
    if release.schema_version == 1 {
        if descriptor_path.exists() {
            return Err("baseline_candidate_descriptor_unexpected".into());
        }
        return Ok(());
    }
    if digest(&descriptor_path)?
        != release
            .candidate_descriptor_sha256
            .as_deref()
            .ok_or("candidate_descriptor_missing")?
    {
        return Err("candidate_descriptor_digest_mismatch".into());
    }
    let descriptor: CandidateDescriptor = read_json(&descriptor_path)?;
    if descriptor.schema_version != 1
        || descriptor.version != release.version
        || descriptor.target != release.target
        || descriptor.parent_lock_sha256 != release.parent_lock_sha256
        || descriptor.parent_lock_sha256 != PARENT_SHA
        || descriptor.baseline_mac_manifest_sha256 != FROZEN_MAC_SHA
        || descriptor.bundle_manifest_sha256 != release.bundle_manifest_sha256
        || digest(&bundle.join("bundle.json"))? != descriptor.bundle_manifest_sha256
        || descriptor.native_inputs.target != descriptor.target
        || descriptor.native_inputs.warden_sha256 != release.warden_sha256
    {
        return Err("candidate_descriptor_binding_invalid".into());
    }
    let manifest: Value = read_json(&bundle.join("bundle.json"))?;
    let manifest_inputs: NativeInputs = serde_json::from_value(manifest["nativeInputs"].clone())
        .map_err(|_| "candidate_native_inputs_missing")?;
    if descriptor.native_inputs != manifest_inputs || manifest_inputs.schema_version != 1 {
        return Err("candidate_native_inputs_mismatch".into());
    }
    for revision in [
        &manifest_inputs.core_revision,
        &manifest_inputs.warden_revision,
        &manifest_inputs.alpaca_revision,
    ] {
        if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("candidate_source_revision_invalid".into());
        }
    }
    let binary = |name: &str| -> String {
        if release.target == TARGETS[4] {
            format!("{name}.exe")
        } else {
            name.to_owned()
        }
    };
    let node = if release.target == TARGETS[4] {
        "runtime/node/bin/node.exe"
    } else {
        "runtime/node/bin/node"
    };
    for (name, expected) in [
        (binary("bin/tradeassembly"), &manifest_inputs.core_sha256),
        (binary("bin/warden"), &manifest_inputs.warden_sha256),
        (
            binary("bin/tradeassembly-sandbox"),
            &manifest_inputs.launcher_sha256,
        ),
        (node.to_owned(), &manifest_inputs.node_sha256),
    ] {
        if !crate::is_digest(expected) || digest(&bundle.join(name))? != *expected {
            return Err("candidate_native_binary_mismatch".into());
        }
    }
    crate::relative(Path::new(&manifest_inputs.alpaca_package))?;
    if Path::new(&manifest_inputs.alpaca_package)
        .components()
        .count()
        != 1
        || !manifest_inputs.alpaca_package.ends_with(".tar.gz")
        || !crate::is_digest(&manifest_inputs.alpaca_package_sha256)
        || !crate::is_digest(&manifest_inputs.alpaca_manifest_sha256)
        || digest(&bundle.join("plugins").join(&manifest_inputs.alpaca_package))?
            != manifest_inputs.alpaca_package_sha256
    {
        return Err("candidate_plugin_input_mismatch".into());
    }
    let alpaca_lock: Value = read_json(&bundle.join("plugins/alpaca.json"))?;
    if alpaca_lock["target"] != release.target
        || alpaca_lock["version"] != "0.1.17"
        || alpaca_lock["sourceRevision"] != manifest_inputs.alpaca_revision
        || alpaca_lock["packageFile"] != manifest_inputs.alpaca_package
        || alpaca_lock["packageSha256"] != manifest_inputs.alpaca_package_sha256
        || alpaca_lock["manifestSha256"] != manifest_inputs.alpaca_manifest_sha256
    {
        return Err("candidate_plugin_lock_mismatch".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic_json;
    use std::fs;

    #[test]
    fn replacement_descriptor_binds_baseline_version_source_and_payload() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for directory in ["bin", "runtime/node/bin", "plugins"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for (name, bytes) in [
            ("bin/tradeassembly", b"core".as_slice()),
            ("bin/warden", b"warden".as_slice()),
            ("bin/tradeassembly-sandbox", b"sandbox".as_slice()),
            ("runtime/node/bin/node", b"node".as_slice()),
            ("plugins/alpaca.tar.gz", b"alpaca".as_slice()),
        ] {
            fs::write(root.join(name), bytes).unwrap();
        }
        let inputs = NativeInputs {
            schema_version: 1,
            target: TARGETS[0].into(),
            core_revision: "a".repeat(40),
            warden_revision: "b".repeat(40),
            alpaca_revision: "c".repeat(40),
            core_sha256: digest(&root.join("bin/tradeassembly")).unwrap(),
            warden_sha256: digest(&root.join("bin/warden")).unwrap(),
            launcher_sha256: digest(&root.join("bin/tradeassembly-sandbox")).unwrap(),
            node_sha256: digest(&root.join("runtime/node/bin/node")).unwrap(),
            alpaca_package: "alpaca.tar.gz".into(),
            alpaca_package_sha256: digest(&root.join("plugins/alpaca.tar.gz")).unwrap(),
            alpaca_manifest_sha256: "d".repeat(64),
        };
        atomic_json(
            &root.join("plugins/alpaca.json"),
            &serde_json::json!({"target":inputs.target,
                "version":"0.1.17","packageFile":inputs.alpaca_package,
                "sourceRevision":inputs.alpaca_revision,
                "packageSha256":inputs.alpaca_package_sha256,
                "manifestSha256":inputs.alpaca_manifest_sha256}),
        )
        .unwrap();
        atomic_json(
            &root.join("bundle.json"),
            &serde_json::json!({"target":TARGETS[0],"nativeInputs":inputs}),
        )
        .unwrap();
        let mut descriptor = CandidateDescriptor {
            schema_version: 1,
            version: "0.1.0-beta.2".into(),
            target: TARGETS[0].into(),
            parent_lock_sha256: PARENT_SHA.into(),
            baseline_mac_manifest_sha256: FROZEN_MAC_SHA.into(),
            bundle_manifest_sha256: digest(&root.join("bundle.json")).unwrap(),
            native_inputs: inputs,
        };
        atomic_json(&root.join(NAME), &descriptor).unwrap();
        let mut release = Release {
            schema_version: 2,
            version: descriptor.version.clone(),
            target: descriptor.target.clone(),
            parent_lock_sha256: PARENT_SHA.into(),
            bundle_manifest_sha256: descriptor.bundle_manifest_sha256.clone(),
            archive_sha256: "e".repeat(64),
            warden_sha256: descriptor.native_inputs.warden_sha256.clone(),
            state_compatibility: "f2-local-v1".into(),
            candidate_descriptor_sha256: Some(digest(&root.join(NAME)).unwrap()),
        };
        verify(root, root, &release).unwrap();
        fs::write(root.join("bin/tradeassembly"), b"changed core").unwrap();
        assert_eq!(
            verify(root, root, &release).unwrap_err(),
            "candidate_native_binary_mismatch"
        );
        fs::write(root.join("bin/tradeassembly"), b"core").unwrap();
        descriptor.version = "0.1.0-beta.3".into();
        atomic_json(&root.join(NAME), &descriptor).unwrap();
        assert_eq!(
            verify(root, root, &release).unwrap_err(),
            "candidate_descriptor_digest_mismatch"
        );
        release.candidate_descriptor_sha256 = Some(digest(&root.join(NAME)).unwrap());
        assert_eq!(
            verify(root, root, &release).unwrap_err(),
            "candidate_descriptor_binding_invalid"
        );
        descriptor.version = release.version.clone();
        descriptor.baseline_mac_manifest_sha256 = "0".repeat(64);
        atomic_json(&root.join(NAME), &descriptor).unwrap();
        release.candidate_descriptor_sha256 = Some(digest(&root.join(NAME)).unwrap());
        assert_eq!(
            verify(root, root, &release).unwrap_err(),
            "candidate_descriptor_binding_invalid"
        );
    }
}
