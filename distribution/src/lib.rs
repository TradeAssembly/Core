// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Source-free distribution. Runtime qualification is separate from packaging.

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(not(windows))]
use std::io::Write;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};
use walkdir::WalkDir;

pub mod candidate;
pub mod install;
pub mod native;
pub mod package;
pub mod qualify;

#[cfg(windows)]
#[path = "../../platform/windows_private.rs"]
pub mod windows_private;

pub type Result<T> = std::result::Result<T, String>;
pub const TARGETS: [&str; 5] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];
pub const PARENT_SHA: &str = "222b1f205edb39bc5e233333c210213354e024d0fa47ac6a5c7588354bf9b639";
pub const FROZEN_MAC_SHA: &str = "6740c7d7f4e8a692ff005dd18b6dc2656883eb8e366253e4e8e6aa054108f5dc";
const MAX_BYTES: u64 = 1_073_741_824;
const MAX_ENTRIES: usize = 10_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Release {
    pub schema_version: u32,
    pub version: String,
    pub target: String,
    pub parent_lock_sha256: String,
    pub bundle_manifest_sha256: String,
    pub archive_sha256: String,
    pub warden_sha256: String,
    pub state_compatibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_descriptor_sha256: Option<String>,
}

impl Release {
    pub fn validate(&self) -> Result<()> {
        let version = semver::Version::parse(&self.version).map_err(|_| "version_invalid")?;
        if version.pre.is_empty()
            || !matches!(self.schema_version, 1 | 2)
            || !TARGETS.contains(&self.target.as_str())
            || self.state_compatibility != "f2-local-v1"
            || [
                &self.parent_lock_sha256,
                &self.bundle_manifest_sha256,
                &self.archive_sha256,
                &self.warden_sha256,
            ]
            .iter()
            .any(|s| !is_digest(s))
        {
            return Err("release_contract_invalid".into());
        }
        if (self.schema_version == 1 && self.candidate_descriptor_sha256.is_some())
            || (self.schema_version == 2
                && self
                    .candidate_descriptor_sha256
                    .as_deref()
                    .is_none_or(|sha| !is_digest(sha)))
        {
            return Err("release_candidate_descriptor_invalid".into());
        }
        Ok(())
    }

    pub fn validate_candidate(&self) -> Result<()> {
        self.validate()?;
        if self.parent_lock_sha256 != PARENT_SHA {
            return Err("candidate_parent_release_lock_changed".into());
        }
        if self.schema_version == 1
            && self.target == TARGETS[0]
            && self.bundle_manifest_sha256 != FROZEN_MAC_SHA
        {
            return Err("frozen_mac_payload_changed".into());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct Bundle {
    target: String,
    files: BTreeMap<String, FileHash>,
}
#[derive(Debug, Deserialize)]
struct FileHash {
    sha256: String,
}

pub fn is_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|_| "file_unavailable")?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = file.read(&mut buf).map_err(|_| "file_read_failed")?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    if fs::metadata(path)
        .map_err(|_| "metadata_unavailable")?
        .len()
        > 4_194_304
    {
        return Err("metadata_too_large".into());
    }
    let bytes = fs::read(path).map_err(|_| "metadata_unavailable")?;
    if bytes.len() > 4_194_304 {
        return Err("metadata_too_large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "metadata_invalid".into())
}

pub fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path
            .to_str()
            .is_none_or(|s| s.contains('\\') || s.contains(':'))
    {
        return Err("unsafe_payload_path".into());
    }
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err("unsafe_payload_path".into());
        };
        let name = name.to_str().ok_or("unsafe_payload_path")?;
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if name.ends_with(['.', ' '])
            || name
                .chars()
                .any(|c| c.is_control() || "<>\"|?*".contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
                })
            })
        {
            return Err("unsafe_payload_path".into());
        }
    }
    Ok(())
}

/// Convert filesystem-native separators to canonical archive/manifest names.
/// Serialized payload paths still reject literal backslashes on every host.
pub fn portable_name(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err("unsafe_payload_path".into());
        };
        parts.push(name.to_str().ok_or("unsafe_payload_path")?);
    }
    let name = parts.join("/");
    relative(Path::new(&name))?;
    Ok(name)
}

pub fn verify_bundle(root: &Path, release: &Release) -> Result<()> {
    release.validate()?;
    if digest(&root.join("bundle.json"))? != release.bundle_manifest_sha256 {
        return Err("bundle_manifest_digest_mismatch".into());
    }
    let bundle: Bundle = read_json(&root.join("bundle.json"))?;
    if bundle.target != release.target
        || bundle.files.is_empty()
        || bundle.files.len() > MAX_ENTRIES
    {
        return Err("bundle_target_or_inventory_invalid".into());
    }
    let canonical = root.canonicalize().map_err(|_| "payload_unavailable")?;
    let mut expected = BTreeSet::from([PathBuf::from("bundle.json")]);
    for (name, entry) in bundle.files {
        relative(Path::new(&name))?;
        if name == "bundle.json" || !is_digest(&entry.sha256) {
            return Err("bundle_inventory_invalid".into());
        }
        let file = root.join(&name);
        if !file
            .canonicalize()
            .map_err(|_| "payload_file_unavailable")?
            .starts_with(&canonical)
            || !file.is_file()
            || digest(&file)? != entry.sha256
        {
            return Err("payload_digest_or_containment_failed".into());
        }
        expected.insert(PathBuf::from(name));
    }
    let mut actual = BTreeSet::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|_| "payload_inventory_failed")?;
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() && !entry.file_type().is_symlink() {
            return Err("payload_special_file_forbidden".into());
        }
        actual.insert(
            entry
                .path()
                .strip_prefix(root)
                .map_err(|_| "payload_inventory_failed")?
                .to_owned(),
        );
    }
    if expected != actual {
        return Err("payload_inventory_mismatch".into());
    }
    if digest(&root.join(executable("bin/warden")))? != release.warden_sha256 {
        return Err("authority_digest_mismatch".into());
    }
    for name in [
        "bin/tradeassembly",
        "bin/warden",
        "bin/tradeassembly-sandbox",
    ] {
        executable_file(&root.join(executable(name)))?;
    }
    Ok(())
}

pub fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn executable_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "executable_unavailable")?;
    if !metadata.is_file() {
        return Err("executable_must_be_regular".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("executable_permission_missing".into());
        }
    }
    Ok(())
}

pub fn archive(root: &Path, destination: &Path) -> Result<()> {
    let file = File::create(destination).map_err(|_| "archive_create_failed")?;
    let encoder = GzEncoder::new(file, Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    // Sorted traversal yields deterministic headers apart from source metadata.
    for entry in WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .min_depth(1)
    {
        let entry = entry.map_err(|_| "archive_inventory_failed")?;
        let name = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| "archive_inventory_failed")?;
        let name = portable_name(name)?;
        builder
            .append_path_with_name(entry.path(), &name)
            .map_err(|_| "archive_write_failed")?;
    }
    builder
        .into_inner()
        .map_err(|_| "archive_write_failed")?
        .finish()
        .map_err(|_| "archive_write_failed")?
        .sync_all()
        .map_err(|_| "archive_sync_failed")?;
    Ok(())
}

/// Extract regular files first, links last. No archive member may traverse a link.
pub fn extract(archive: &Path, destination: &Path) -> Result<()> {
    if fs::read_dir(destination)
        .map_err(|_| "extraction_root_unavailable")?
        .next()
        .is_some()
    {
        return Err("extraction_root_not_empty".into());
    }
    let mut archive = tar::Archive::new(GzDecoder::new(
        File::open(archive).map_err(|_| "archive_unavailable")?,
    ));
    let mut names = BTreeSet::new();
    let mut links = Vec::new();
    let mut total: u64 = 0;
    for entry in archive.entries().map_err(|_| "archive_invalid")? {
        let mut entry = entry.map_err(|_| "archive_invalid")?;
        let path = entry
            .path()
            .map_err(|_| "archive_path_invalid")?
            .into_owned();
        relative(&path)?;
        if !names.insert(path.clone()) || names.len() > MAX_ENTRIES {
            return Err("archive_duplicate_or_limit".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("archive_size_limit")?;
        if total > MAX_BYTES {
            return Err("archive_size_limit".into());
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|_| "archive_link_invalid")?
                .ok_or("archive_link_invalid")?
                .into_owned();
            safe_link(&path, &link)?;
            links.push((path, link));
        } else if kind.is_file() || kind.is_dir() {
            if !entry
                .unpack_in(destination)
                .map_err(|_| "archive_unpack_failed")?
            {
                return Err("archive_escape".into());
            }
        } else {
            return Err("archive_special_member_forbidden".into());
        }
    }
    for (path, link) in links {
        let file = destination.join(&path);
        fs::create_dir_all(file.parent().ok_or("archive_path_invalid")?)
            .map_err(|_| "archive_unpack_failed")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(link, file).map_err(|_| "archive_link_failed")?;
        #[cfg(windows)]
        {
            // Do not require Developer Mode or elevated symlink creation on Windows.
            let target = file
                .parent()
                .ok_or("archive_link_invalid")?
                .join(link)
                .canonicalize()
                .map_err(|_| "archive_link_invalid")?;
            let root = destination
                .canonicalize()
                .map_err(|_| "archive_link_invalid")?;
            if !target.starts_with(root) || !target.is_file() {
                return Err("archive_link_invalid".into());
            }
            fs::copy(target, file).map_err(|_| "archive_link_failed")?;
        }
    }
    Ok(())
}

fn safe_link(path: &Path, link: &Path) -> Result<()> {
    if link.is_absolute()
        || link
            .to_str()
            .is_none_or(|s| s.contains('\\') || s.contains(':'))
    {
        return Err("archive_link_escape".into());
    }
    let mut depth = path
        .parent()
        .ok_or("archive_link_invalid")?
        .components()
        .count();
    for c in link.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => (),
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return Err("archive_link_escape".into()),
        }
    }
    Ok(())
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    #[cfg(windows)]
    {
        let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| "metadata_write_failed")?;
        bytes.push(b'\n');
        windows_private::write_atomic(path, &bytes).map_err(|_| "metadata_replace_failed".into())
    }
    #[cfg(not(windows))]
    {
        let parent = path.parent().ok_or("metadata_path_invalid")?;
        let mut temp =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "metadata_create_failed")?;
        serde_json::to_writer_pretty(temp.as_file_mut(), value)
            .map_err(|_| "metadata_write_failed")?;
        temp.write_all(b"\n").map_err(|_| "metadata_write_failed")?;
        temp.as_file()
            .sync_all()
            .map_err(|_| "metadata_sync_failed")?;
        temp.persist(path).map_err(|_| "metadata_replace_failed")?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| "metadata_sync_failed")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_paths_and_links() {
        for path in [
            "../escape",
            "/absolute",
            "C:\\escape",
            "a/../b",
            "a:b",
            "bin/NUL",
            "bin/COM1.txt",
            "bin/LPT9",
            "bin/trailing.",
            "bin/trailing ",
            "bin/wild*card",
        ] {
            assert!(relative(Path::new(path)).is_err());
        }
        assert!(safe_link(Path::new("bin/tool"), Path::new("../../escape")).is_err());
        assert!(safe_link(Path::new("bin/tool"), Path::new("../lib/tool")).is_ok());
    }

    #[test]
    fn native_names_are_serialized_with_portable_separators() {
        let native = PathBuf::from("runtime")
            .join("node")
            .join("bin")
            .join("node.exe");
        assert_eq!(portable_name(&native).unwrap(), "runtime/node/bin/node.exe");
        assert!(portable_name(Path::new("../escape")).is_err());
        assert!(relative(Path::new("bin/good name.exe")).is_ok());
    }

    #[test]
    fn archive_roundtrip_preserves_files_and_links() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("output");
        fs::create_dir_all(source.join("bin")).unwrap();
        fs::create_dir(&output).unwrap();
        fs::write(source.join("bin/tool"), b"immutable bytes").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("tool", source.join("bin/link")).unwrap();
        let archive_path = temp.path().join("bundle.tar.gz");
        archive(&source, &archive_path).unwrap();
        extract(&archive_path, &output).unwrap();
        assert_eq!(
            digest(&source.join("bin/tool")).unwrap(),
            digest(&output.join("bin/tool")).unwrap()
        );
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(output.join("bin/link")).unwrap(),
            Path::new("tool")
        );
        assert!(extract(&archive_path, &output).is_err());
    }

    #[test]
    fn duplicate_archive_paths_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("duplicate.tar.gz");
        let encoder = GzEncoder::new(File::create(&archive_path).unwrap(), Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        for _ in 0..2 {
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o600);
            header.set_cksum();
            tar.append_data(&mut header, "same", &b"x"[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        let output = temp.path().join("output");
        fs::create_dir(&output).unwrap();
        assert_eq!(
            extract(&archive_path, &output).unwrap_err(),
            "archive_duplicate_or_limit"
        );
    }

    #[test]
    fn synthetic_inventory_rejects_corruption_extras_and_missing_execution_mode() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("bin")).unwrap();
        let mut files = BTreeMap::new();
        for name in [
            "bin/tradeassembly",
            "bin/warden",
            "bin/tradeassembly-sandbox",
        ] {
            let name = executable(name);
            fs::write(
                root.join(&name),
                b"synthetic parser fixture, not native qualification",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(root.join(&name), fs::Permissions::from_mode(0o755)).unwrap();
            }
            files.insert(
                name.clone(),
                serde_json::json!({"sha256":digest(&root.join(name)).unwrap()}),
            );
        }
        let target = install::host_target().unwrap();
        atomic_json(
            &root.join("bundle.json"),
            &serde_json::json!({"target":target,"files":files}),
        )
        .unwrap();
        let release = Release {
            schema_version: 1,
            version: "0.1.0-beta.1".into(),
            target: target.into(),
            parent_lock_sha256: PARENT_SHA.into(),
            bundle_manifest_sha256: digest(&root.join("bundle.json")).unwrap(),
            archive_sha256: "0".repeat(64),
            warden_sha256: digest(&root.join(executable("bin/warden"))).unwrap(),
            state_compatibility: "f2-local-v1".into(),
            candidate_descriptor_sha256: None,
        };
        verify_bundle(root, &release).unwrap();
        let core = root.join(executable("bin/tradeassembly"));
        fs::write(&core, b"corrupted").unwrap();
        assert_eq!(
            verify_bundle(root, &release).unwrap_err(),
            "payload_digest_or_containment_failed"
        );
        fs::write(&core, b"synthetic parser fixture, not native qualification").unwrap();
        fs::write(root.join("extra"), b"unlisted").unwrap();
        assert_eq!(
            verify_bundle(root, &release).unwrap_err(),
            "payload_inventory_mismatch"
        );
        fs::remove_file(root.join("extra")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(core, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(
                verify_bundle(root, &release).unwrap_err(),
                "executable_permission_missing"
            );
        }
    }
}
