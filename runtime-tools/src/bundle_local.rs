//! Assemble an unsigned local candidate exclusively from explicit binary inputs.
use flate2::read::GzDecoder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tradeassembly_runtime::adapters::plugin_packages::LocalPluginPackageStore;
use tradeassembly_runtime::ports::{PluginPackageInstallRequest, PluginPackagePort};

const HELP: &str = "cargo xtask bundle-local --core PATH --warden PATH --sandbox-launcher PATH --alpaca-package PATH --node-archive PATH --output NEW_DIRECTORY [--connection-profile PATH]\nCreates an unsigned macOS arm64 candidate. Uses npm ci on the build host only. Never publishes or signs. Connection profiles contain public deployment configuration only.";

pub(crate) fn run(args: &[String], root: &Path) -> i32 {
    if args.iter().any(|arg| arg == "--help") {
        println!("{HELP}");
        return 0;
    }
    match assemble(args, root) {
        Ok(path) => {
            println!("Unsigned candidate assembled: {}", path.display());
            0
        }
        Err(error) => {
            eprintln!("Local bundle failed: {error}");
            1
        }
    }
}

fn assemble(args: &[String], root: &Path) -> Result<PathBuf, String> {
    let options = options(args)?;
    let output = &options["--output"];
    if output.exists() {
        return Err("output_already_exists".into());
    }
    let parent = output.parent().ok_or("output_parent_required")?;
    if !parent.is_dir() {
        return Err("output_parent_missing".into());
    }
    let stage = tempfile::tempdir_in(parent).map_err(|_| "bundle_stage_failed")?;
    let node: Value = read_json(&root.join("packaging/sandbox/node.json"))?;
    let alpaca: Value = read_json(&root.join("packaging/alpaca.json"))?;
    verify_digest(&options["--node-archive"], string(&node, "sha256")?)?;
    verify_digest(
        &options["--alpaca-package"],
        string(&alpaca, "packageSha256")?,
    )?;
    // Exercise the production installer before shipping pins. Its manifest hash
    // is canonical JSON, not the raw YAML digest in the package descriptor.
    let qualification = tempfile::tempdir().map_err(|_| "bundle_plugin_stage_failed")?;
    let installed = LocalPluginPackageStore::new(qualification.path())?.install(
        &PluginPackageInstallRequest {
            source_type: "file".into(),
            locator: options["--alpaca-package"].to_string_lossy().into_owned(),
            package_sha256: string(&alpaca, "packageSha256")?.into(),
            manifest_sha256: string(&alpaca, "manifestSha256")?.into(),
            offline: true,
        },
    )?;
    if installed.plugin_ref != "tradeassembly.alpaca"
        || installed.version != string(&alpaca, "version")?
    {
        return Err("bundle_plugin_identity_mismatch".into());
    }
    verify_alpaca_package(
        &options["--alpaca-package"],
        string(&alpaca, "target")?,
        root,
    )?;
    let destination = stage.path();
    fs::create_dir(destination.join("bin")).map_err(|_| "bundle_directory_failed")?;
    for (flag, name) in [
        ("--core", "tradeassembly"),
        ("--warden", "warden"),
        ("--sandbox-launcher", "tradeassembly-sandbox"),
    ] {
        verify_release_arm64(&options[flag], root)?;
        fs::copy(&options[flag], destination.join("bin").join(name))
            .map_err(|_| "bundle_binary_copy_failed")?;
    }
    let bitwarden_helper = root.join("scripts/bitwarden-session-exec");
    if !bitwarden_helper.is_file() {
        return Err("bitwarden_helper_missing".into());
    }
    fs::copy(
        &bitwarden_helper,
        destination.join("bin/tradeassembly-bitwarden"),
    )
    .map_err(|_| "bitwarden_helper_copy_failed")?;
    #[cfg(unix)]
    fs::set_permissions(
        destination.join("bin/tradeassembly-bitwarden"),
        fs::Permissions::from_mode(0o755),
    )
    .map_err(|_| "bitwarden_helper_permission_failed")?;
    unpack_node(
        &options["--node-archive"],
        &destination.join("runtime/node"),
        string(&node, "version")?,
    )?;
    let runtime = destination.join("runtime");
    for name in ["package.json", "package-lock.json"] {
        fs::copy(
            root.join("packaging/sandbox").join(name),
            runtime.join(name),
        )
        .map_err(|_| "sandbox_lock_missing")?;
    }
    let lock = read_json(&runtime.join("package-lock.json"))?;
    if lock["packages"]["node_modules/@anthropic-ai/sandbox-runtime"]["version"] != "0.0.67" {
        return Err("sandbox_version_not_pinned".into());
    }
    npm_ci(&runtime)?;
    let plugins = destination.join("plugins");
    fs::create_dir(&plugins).map_err(|_| "bundle_directory_failed")?;
    let package_name = string(&alpaca, "packageFile")?;
    if Path::new(package_name).components().count() != 1 || !package_name.ends_with(".tar.gz") {
        return Err("bundle_plugin_name_invalid".into());
    }
    fs::copy(&options["--alpaca-package"], plugins.join(package_name))
        .map_err(|_| "bundle_plugin_copy_failed")?;
    fs::copy(
        root.join("packaging/alpaca.json"),
        plugins.join("alpaca.json"),
    )
    .map_err(|_| "bundle_plugin_copy_failed")?;
    copy_core_notices(root, destination)?;
    if let Some(path) = options.get("--connection-profile") {
        let profile = tradeassembly_runtime::connection_profile::ConnectionProfile::read(path)?;
        fs::write(
            destination.join("bin/connection-profile.json"),
            serde_json::to_vec_pretty(&profile).map_err(|_| "connection_profile_invalid")?,
        )
        .map_err(|_| "connection_profile_copy_failed")?;
    }
    fs::copy(
        root.join("docs/reference/local-binary-setup.md"),
        destination.join("SETUP.md"),
    )
    .map_err(|_| "bundle_guide_missing")?;
    let files = inventory(destination)?;
    let manifest = json!({
        "schemaVersion":"tradeassembly.local-bundle.v1", "target":"aarch64-apple-darwin",
        "releaseReady":false, "signed":false, "notarized":false,
        "node":node, "alpaca":alpaca, "sandboxVersion":"0.0.67", "files":files,
        "sourceBuildRequiredForCustomer":false,
        "remainingAcceptance":["bundle runtime smoke", "signing and notarization", "public download", "third-party distribution notices", "full release gates"]
    });
    fs::write(
        destination.join("bundle.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|_| "bundle_manifest_invalid")?,
    )
    .map_err(|_| "bundle_manifest_write_failed")?;
    if output.exists() {
        return Err("output_already_exists".into());
    }
    fs::rename(destination, output).map_err(|_| "bundle_publish_local_failed")?;
    Ok(output.clone())
}

fn options(args: &[String]) -> Result<BTreeMap<String, PathBuf>, String> {
    let names = [
        "--core",
        "--warden",
        "--sandbox-launcher",
        "--alpaca-package",
        "--node-archive",
        "--output",
    ];
    if args.len() != names.len() * 2 && args.len() != (names.len() + 1) * 2 {
        return Err(HELP.into());
    }
    let mut result = BTreeMap::new();
    for pair in args.as_chunks::<2>().0 {
        if (!names.contains(&pair[0].as_str()) && pair[0] != "--connection-profile")
            || result.contains_key(&pair[0])
        {
            return Err("bundle_option_invalid".into());
        }
        let path = PathBuf::from(&pair[1]);
        if !path.is_absolute() {
            return Err("bundle_paths_must_be_absolute".into());
        }
        result.insert(pair[0].clone(), path);
    }
    if !names.iter().all(|name| result.contains_key(*name)) {
        return Err(HELP.into());
    }
    Ok(result)
}

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|_| "bundle_input_unavailable")?)
        .map_err(|_| "bundle_input_invalid".into())
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| "bundle_metadata_invalid".into())
}
fn digest(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|_| "bundle_input_unavailable")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "bundle_input_unavailable")?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn verify_digest(path: &Path, expected: &str) -> Result<(), String> {
    if expected.len() != 64 || digest(path)? != expected {
        return Err("bundle_digest_mismatch".into());
    }
    Ok(())
}
fn verify_arm64(path: &Path) -> Result<(), String> {
    let mut header = [0u8; 8];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|_| "bundle_binary_invalid")?;
    if header != [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1] {
        return Err("bundle_binary_not_macos_arm64".into());
    }
    Ok(())
}

fn verify_release_arm64(path: &Path, root: &Path) -> Result<(), String> {
    verify_arm64(path)?;
    verify_no_private_build_paths(&fs::read(path).map_err(|_| "bundle_binary_invalid")?, root)
}

fn verify_no_private_build_paths(bytes: &[u8], root: &Path) -> Result<(), String> {
    let canonical = root
        .canonicalize()
        .map_err(|_| "bundle_source_root_invalid")?;
    let root_bytes = canonical.as_os_str().as_encoded_bytes();
    let private_prefix = canonical.components().take(3).collect::<PathBuf>();
    let private_bytes = private_prefix.as_os_str().as_encoded_bytes();
    if [root_bytes, private_bytes]
        .into_iter()
        .filter(|needle| needle.len() > 1)
        .any(|needle| bytes.windows(needle.len()).any(|window| window == needle))
    {
        return Err("bundle_binary_contains_private_build_path".into());
    }
    Ok(())
}

fn verify_alpaca_package(archive: &Path, target: &str, root: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|_| "bundle_plugin_unavailable")?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let expected = format!("bin/{target}/tradeassembly-plugin-alpaca");
    let mut found = false;
    for entry in archive.entries().map_err(|_| "bundle_plugin_invalid")? {
        let mut entry = entry.map_err(|_| "bundle_plugin_invalid")?;
        let path = entry.path().map_err(|_| "bundle_plugin_invalid")?;
        if path != Path::new(&expected) {
            continue;
        }
        if found || !entry.header().entry_type().is_file() || entry.size() > 64 * 1024 * 1024 {
            return Err("bundle_plugin_binary_invalid".into());
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|_| "bundle_plugin_binary_invalid")?;
        verify_no_private_build_paths(&bytes, root)?;
        found = true;
    }
    found
        .then_some(())
        .ok_or_else(|| "bundle_plugin_binary_missing".into())
}

fn unpack_node(archive: &Path, output: &Path, version: &str) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|_| "node_archive_missing")?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let prefix = format!("node-v{version}-darwin-arm64/");
    let mut binary = false;
    let mut license = false;
    for entry in archive.entries().map_err(|_| "node_archive_invalid")? {
        let mut entry = entry.map_err(|_| "node_archive_invalid")?;
        let path = entry
            .path()
            .map_err(|_| "node_archive_invalid")?
            .into_owned();
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err("node_archive_path_invalid".into());
        }
        let path = path.to_str().ok_or("node_archive_path_invalid")?;
        let Some(relative) = path.strip_prefix(&prefix) else {
            return Err("node_archive_root_invalid".into());
        };
        if !matches!(relative, "bin/node" | "LICENSE") {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err("node_archive_link_rejected".into());
        }
        if (relative == "bin/node" && binary) || (relative == "LICENSE" && license) {
            return Err("node_archive_duplicate".into());
        }
        let target = output.join(relative);
        fs::create_dir_all(target.parent().ok_or("node_archive_path_invalid")?)
            .map_err(|_| "bundle_directory_failed")?;
        entry
            .unpack(&target)
            .map_err(|_| "node_archive_unpack_failed")?;
        binary |= relative == "bin/node";
        license |= relative == "LICENSE";
    }
    if !binary || !license {
        return Err("node_archive_incomplete".into());
    }
    verify_arm64(&output.join("bin/node"))
}

fn npm_ci(directory: &Path) -> Result<(), String> {
    let mut child = Command::new("npm")
        .args([
            "ci",
            "--omit=dev",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ])
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "build_host_npm_unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err("sandbox_dependency_install_failed".into())
                }
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("sandbox_dependency_install_timeout".into());
            }
        }
    }
}

fn copy_core_notices(root: &Path, destination: &Path) -> Result<(), String> {
    for name in ["LICENSE", "NOTICE"] {
        fs::copy(root.join(name), destination.join(name))
            .map_err(|_| format!("bundle_required_notice_missing:{name}"))?;
    }
    Ok(())
}

fn inventory(root: &Path) -> Result<BTreeMap<String, Value>, String> {
    let canonical = root.canonicalize().map_err(|_| "bundle_inventory_failed")?;
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|_| "bundle_inventory_failed")?;
        if entry.file_type().is_dir() {
            continue;
        }
        let resolved = entry
            .path()
            .canonicalize()
            .map_err(|_| "bundle_inventory_failed")?;
        if !resolved.starts_with(&canonical) {
            return Err("bundle_link_escape".into());
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| "bundle_inventory_failed")?
            .to_str()
            .ok_or("bundle_inventory_failed")?
            .to_string();
        files.insert(relative, json!({"sha256":digest(entry.path())?}));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_notices_are_required_and_inventory_binds_their_bytes() {
        let root = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        assert!(copy_core_notices(root.path(), destination.path()).is_err());
        fs::write(root.path().join("LICENSE"), b"license fixture").unwrap();
        assert!(copy_core_notices(root.path(), destination.path()).is_err());
        fs::write(root.path().join("NOTICE"), b"notice fixture").unwrap();
        copy_core_notices(root.path(), destination.path()).unwrap();
        let files = inventory(destination.path()).unwrap();
        for name in ["LICENSE", "NOTICE"] {
            assert_eq!(
                fs::read(root.path().join(name)).unwrap(),
                fs::read(destination.path().join(name)).unwrap()
            );
            assert_eq!(
                files[name]["sha256"],
                digest(&root.path().join(name)).unwrap()
            );
        }
    }
    #[test]
    fn bundle_local_rejects_node_archive_links() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("node.tar.gz");
        let stream = flate2::write::GzEncoder::new(
            fs::File::create(&file).unwrap(),
            flate2::Compression::default(),
        );
        let mut archive = tar::Builder::new(stream);
        let mut header = tar::Header::new_gnu();
        header
            .set_path("node-v22.23.2-darwin-arm64/bin/node")
            .unwrap();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name("/outside-bundle").unwrap();
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append(&header, std::io::empty()).unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        assert_eq!(
            unpack_node(&file, &root.path().join("out"), "22.23.2").unwrap_err(),
            "node_archive_link_rejected"
        );
        assert!(!root.path().join("out/bin/node").exists());
    }

    #[cfg(unix)]
    #[test]
    fn bundle_local_inventory_rejects_escaping_links() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("bundle");
        fs::create_dir(&inside).unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, b"fixture").unwrap();
        std::os::unix::fs::symlink(&outside, inside.join("escape")).unwrap();
        assert_eq!(inventory(&inside).unwrap_err(), "bundle_link_escape");
    }
    #[test]
    fn bundle_local_rejects_missing_duplicate_or_relative_inputs() {
        assert!(options(&[]).is_err());
        let args: Vec<String> = [
            "--core",
            "--warden",
            "--sandbox-launcher",
            "--alpaca-package",
            "--node-archive",
            "--output",
        ]
        .into_iter()
        .flat_map(|key| [key.into(), "/tmp/input".into()])
        .collect();
        assert_eq!(options(&args).unwrap().len(), 6);
        let mut duplicate = args.clone();
        duplicate[2] = "--core".into();
        assert!(options(&duplicate).is_err());
        let mut relative = args;
        relative[1] = "relative".into();
        assert!(options(&relative).is_err());
    }
    #[test]
    fn bundle_local_digest_and_target_checks_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        fs::write(&file, b"not a binary").unwrap();
        assert!(verify_arm64(&file).is_err());
        assert!(verify_digest(&file, &"0".repeat(64)).is_err());
        verify_digest(&file, &digest(&file).unwrap()).unwrap();
    }

    #[test]
    fn release_binary_rejects_source_and_builder_private_paths() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let source = root.as_os_str().as_encoded_bytes();
        assert_eq!(
            verify_no_private_build_paths(source, &root).unwrap_err(),
            "bundle_binary_contains_private_build_path"
        );
        let builder = root.components().take(3).collect::<PathBuf>();
        assert_eq!(
            verify_no_private_build_paths(builder.as_os_str().as_encoded_bytes(), &root)
                .unwrap_err(),
            "bundle_binary_contains_private_build_path"
        );
        verify_no_private_build_paths(b"/build/tradeassembly/runtime-rs", &root).unwrap();
    }
}
