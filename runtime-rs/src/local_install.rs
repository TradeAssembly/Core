//! Account-free, foreground local Warden installation.
//!
//! This module deliberately does not supervise a process. It prepares private
//! state and returns the exact command the caller may execute in the foreground.

use crate::finance_authority::{
    REQUIRED_WARDEN_SERVICE_VERSION, TRADEASSEMBLY_PEP_MANIFEST, TRADEASSEMBLY_POLICY_BUNDLE,
};
use crate::local_owner_identity::LocalOwnerIdentity;
use crate::runtime_config::RuntimeConfigLayer;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const KEY_ID: &str = "tradeassembly-local-key";
const SCHEMA: &str = "tradeassembly.local-installation.v1";
const SETUP_SCHEMA: &str = "tradeassembly.local-setup.v1";
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Installation {
    schema_version: String,
    service_version: String,
    executable: String,
    sha256: String,
    port: u16,
}

/// Prepare a private local installation and return non-secret setup metadata.
pub fn prepare(state_dir: &Path, warden_binary: &Path, port: u16) -> Result<Value, String> {
    if port == 0 {
        return Err("local_install_port_invalid".into());
    }
    let state = prepare_state_root(state_dir)?;
    let binary = absolute_binary(warden_binary)?;
    if !binary.is_file() {
        return Err("local_install_binary_unavailable".into());
    }

    let installation_path = state.join("installation.json");
    let digest = sha256_file(&binary)?;
    let installation = if installation_path.exists() {
        let saved = read_installation(&installation_path)?;
        if saved.schema_version != SCHEMA
            || saved.service_version != REQUIRED_WARDEN_SERVICE_VERSION
            || saved.executable != binary.display().to_string()
            || saved.sha256 != digest
            || saved.port != port
        {
            return Err("local_installation_integrity_failed".into());
        }
        verify_version(&binary)?;
        saved
    } else {
        let entries = fs::read_dir(&state)
            .map_err(|_| "local_install_state_unavailable")?
            .count();
        if entries > 0 {
            return Err("local_install_state_unrecognized".into());
        }
        verify_version(&binary)?;
        Installation {
            schema_version: SCHEMA.into(),
            service_version: REQUIRED_WARDEN_SERVICE_VERSION.into(),
            executable: binary.display().to_string(),
            sha256: digest,
            port,
        }
    };
    // Bind partial work before generating keys. A retry must not adopt an
    // unrelated directory merely because it contains similarly named files.
    publish_json(&installation_path, &installation)?;
    let executable = std::env::current_exe().map_err(|_| "local_install_executable_unavailable")?;
    let policy_arguments = if cfg!(target_os = "macos") {
        vec![
            "policy".to_string(),
            "launchd".into(),
            "install".into(),
            "--state-dir".into(),
            absolute(&state)?.display().to_string(),
        ]
    } else {
        vec![
            "policy".to_string(),
            "serve".into(),
            "--state-dir".into(),
            absolute(&state)?.display().to_string(),
        ]
    };

    let database = state.join("runtime.db");
    let warden_database = state.join("warden.sqlite");
    let key = state.join("signing.seed");
    let token = state.join("warden.token");
    let policy = state.join("policy-bundle.json");
    let peps = state.join("peps.json");
    let runtime = state.join("runtime.json");
    private_bytes(&key, 32)?;
    private_token(&token)?;
    publish_or_verify_text(&policy, TRADEASSEMBLY_POLICY_BUNDLE)?;
    publish_or_verify_text(&peps, TRADEASSEMBLY_PEP_MANIFEST)?;

    let mut bootstrap = base_command(&binary, &warden_database, &key, &peps);
    if warden_database.is_file() {
        run_bounded(&mut bootstrap, &[OsString::from("receipt-keys")])?;
    } else {
        run_bounded(&mut bootstrap, &[OsString::from("bootstrap")])?;
    }
    let mut policy_command = base_command(&binary, &warden_database, &key, &peps);
    run_bounded(
        &mut policy_command,
        &[
            OsString::from("install-policy"),
            OsString::from("--bundle"),
            policy.clone().into_os_string(),
        ],
    )?;

    let owner = LocalOwnerIdentity::for_database(&database)?;
    let config = RuntimeConfigLayer {
        profile: Some("local".into()),
        database_path: Some(absolute(&database)?.display().to_string()),
        artifact_root: Some(absolute(&state.join("artifacts"))?.display().to_string()),
        oidc_profile: Some("local_owner".into()),
        legal_receipt_root: Some(
            absolute(&state.join("legal/receipts"))?
                .display()
                .to_string(),
        ),
        legal_trusted_keys_path: Some(
            absolute(&state.join("legal/trusted-keys.json"))?
                .display()
                .to_string(),
        ),
        legal_policy_path: Some(
            absolute(&state.join("legal/policy.json"))?
                .display()
                .to_string(),
        ),
        warden_sidecar_url: Some(format!("http://127.0.0.1:{port}")),
        warden_token_ref: Some(format!("file://{}", absolute(&token)?.display())),
        warden_required_version: Some(REQUIRED_WARDEN_SERVICE_VERSION.into()),
        plugin_sandbox_command: bundled_sandbox_command(),
        ..RuntimeConfigLayer::default()
    };
    publish_json(&runtime, &config)?;
    publish_json(&installation_path, &installation)?;

    Ok(json!({
        "ok": true,
        "schemaVersion": SETUP_SCHEMA,
        "prepared": true,
        "policyRunning": false,
        "automationReady": false,
        "sandboxConfigured": config.plugin_sandbox_command.is_some(),
        "runtimeConfigPath": absolute(&runtime)?.display().to_string(),
        "installationPath": absolute(&installation_path)?.display().to_string(),
        "ownerId": owner.stable_identity_id,
        "port": port,
        "nextCommands": [
            {"command":executable, "arguments":policy_arguments},
            {"command":executable, "arguments":["--config", absolute(&runtime)?.display().to_string(), "mcp", "serve"]}
        ],
        "message":"Local policy state is prepared. Run the policy service and attach MCP. Broker and agent setup are separate; no strategy was created or activated.",
    }))
}

fn bundled_sandbox_command() -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    let bin = executable.parent()?;
    let root = bin.parent()?;
    if bin.file_name()? != "bin" || !root.join("bundle.json").is_file() {
        return None;
    }
    let launcher = bin.join("tradeassembly-sandbox");
    if !launcher.is_file()
        || !root.join("runtime/node/bin/node").is_file()
        || !root
            .join("runtime/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js")
            .is_file()
    {
        return None;
    }
    Some(launcher.display().to_string())
}

/// Build the foreground Warden command after validating the persisted install.
pub fn policy_command(state_dir: &Path) -> Result<Command, String> {
    let state = validate_state_root(state_dir)?;
    let installation = read_installation(&state.join("installation.json"))?;
    if installation.schema_version != SCHEMA
        || installation.service_version != REQUIRED_WARDEN_SERVICE_VERSION
        || installation.port == 0
    {
        return Err("local_installation_record_invalid".into());
    }
    let binary = PathBuf::from(&installation.executable);
    if !binary.is_absolute() || sha256_file(&binary)? != installation.sha256 {
        return Err("local_installation_integrity_failed".into());
    }
    verify_version(&binary)?;
    let database = state.join("warden.sqlite");
    reject_symlinks(&database)?;
    let key = state.join("signing.seed");
    let peps = state.join("peps.json");
    if !key.is_file() || !state.join("warden.token").is_file() || !peps.is_file() {
        return Err("local_installation_incomplete".into());
    }
    validate_private_file(&key, 32)?;
    validate_private_file(&state.join("warden.token"), 64)?;
    publish_or_verify_text(&peps, TRADEASSEMBLY_PEP_MANIFEST)?;
    publish_or_verify_text(
        &state.join("policy-bundle.json"),
        TRADEASSEMBLY_POLICY_BUNDLE,
    )?;
    let mut command = base_command(&binary, &database, &key, &peps);
    command
        .arg("serve")
        .arg("--bind")
        .arg(format!("127.0.0.1:{}", installation.port))
        .arg("--token-file")
        .arg(absolute(&state.join("warden.token"))?);
    Ok(command)
}

fn base_command(binary: &Path, database: &Path, key: &Path, peps: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("--database")
        .arg(database)
        .arg("--key-file")
        .arg(key)
        .arg("--key-id")
        .arg(KEY_ID)
        .arg("--peps")
        .arg(peps)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn run_bounded(command: &mut Command, args: &[OsString]) -> Result<(), String> {
    run_bounded_for(command, args, TIMEOUT)
}

fn run_bounded_for(
    command: &mut Command,
    args: &[OsString],
    timeout: Duration,
) -> Result<(), String> {
    let mut child = command
        .args(args)
        .spawn()
        .map_err(|_| "local_install_command_failed")?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err("local_install_command_failed".into()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("local_install_command_timeout".into());
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("local_install_command_failed".into());
            }
        }
    }
}

fn verify_version(binary: &Path) -> Result<(), String> {
    let mut command = Command::new(binary);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| "local_install_binary_incompatible")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let output = child
                    .stdout
                    .take()
                    .and_then(|mut stream| {
                        let mut bytes = Vec::new();
                        std::io::Read::read_to_end(&mut stream, &mut bytes).ok()?;
                        Some(bytes)
                    })
                    .ok_or("local_install_binary_incompatible")?;
                if String::from_utf8(output).ok().is_some_and(|text| {
                    text.trim() == format!("warden {}", REQUIRED_WARDEN_SERVICE_VERSION)
                }) {
                    return Ok(());
                }
                return Err("local_install_binary_incompatible".into());
            }
            Ok(Some(_)) => return Err("local_install_binary_incompatible".into()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("local_install_binary_incompatible".into());
            }
        }
    }
}

fn prepare_state_root(path: &Path) -> Result<PathBuf, String> {
    let path = system_state_path(path);
    if !path.is_absolute() {
        return Err("local_install_state_must_be_absolute".into());
    }
    reject_symlinks(&path)?;
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| "local_install_state_unavailable")?;
    }
    let state = validate_state_root(&path)?;
    for entry in fs::read_dir(&state).map_err(|_| "local_install_state_unavailable")? {
        let entry = entry.map_err(|_| "local_install_state_unavailable")?;
        let name = entry.file_name();
        if entry
            .file_type()
            .map_err(|_| "local_install_state_unavailable")?
            .is_symlink()
        {
            return Err("local_install_state_symlink".into());
        }
        if !recognized(name.to_string_lossy().as_ref()) {
            return Err("local_install_state_unrecognized".into());
        }
    }
    Ok(state)
}

fn validate_state_root(path: &Path) -> Result<PathBuf, String> {
    let path = system_state_path(path);
    if !path.is_absolute() || symlink_metadata(&path)?.file_type().is_symlink() {
        return Err("local_install_state_invalid".into());
    }
    let meta = symlink_metadata(&path)?;
    if !meta.is_dir() {
        return Err("local_install_state_invalid".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err("local_install_state_insecure".into());
        }
    }
    reject_symlinks(&path)?;
    Ok(path)
}

// macOS exposes /tmp as the OS-owned /private/tmp symlink. Normalize only
// that fixed platform alias before applying the no-user-controlled-symlink
// rule. All other path components are still checked with lstat below.
fn system_state_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Ok(suffix) = path.strip_prefix("/tmp") {
            return Path::new("/private/tmp").join(suffix);
        }
    }
    path.to_path_buf()
}

fn recognized(name: &str) -> bool {
    matches!(
        name,
        "installation.json"
            | "runtime.json"
            | "runtime.db"
            | "warden.sqlite"
            | "warden.sqlite-shm"
            | "warden.sqlite-wal"
            | "warden.sqlite.integrity-witness"
            | "warden.sqlite.integrity-key"
            | "warden.sqlite.integrity-lock"
            | "warden.sqlite.integrity-anchor"
            | "runtime.db-shm"
            | "runtime.db-wal"
            | "signing.seed"
            | "warden.token"
            | "policy-bundle.json"
            | "peps.json"
            | "policy.stdout.log"
            | "policy.stderr.log"
            | "agent-runner.out.log"
            | "agent-runner.err.log"
            | "artifacts"
            | "legal"
            | ".local-owner"
            | "runtime.db.local-owner"
    )
}

fn reject_symlinks(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component),
            Component::CurDir => {}
            Component::ParentDir => return Err("local_install_state_invalid".into()),
            Component::Normal(_) => {
                current.push(component);
                if fs::symlink_metadata(&current)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    return Err("local_install_state_symlink".into());
                }
            }
        }
    }
    Ok(())
}

fn validate_private_file(path: &Path, size: u64) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "local_install_secret_invalid")?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != size {
        return Err("local_install_secret_invalid".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("local_install_secret_insecure".into());
        }
    }
    Ok(())
}

fn private_bytes(path: &Path, size: usize) -> Result<(), String> {
    if path.exists() {
        validate_private_file(path, size as u64)?;
        return Ok(());
    }
    let mut bytes = vec![0_u8; size];
    OsRng.fill_bytes(&mut bytes);
    publish_bytes(path, &bytes)
}

fn private_token(path: &Path) -> Result<(), String> {
    if path.exists() {
        validate_private_file(path, 64)?;
        let bytes = fs::read(path).map_err(|_| "local_install_secret_invalid")?;
        if bytes.len() != 64 || !bytes.iter().all(|b| b.is_ascii_hexdigit()) {
            return Err("local_install_secret_invalid".into());
        }
        return Ok(());
    }
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    publish_bytes(path, hex_encode(&bytes).as_bytes())
}

fn publish_or_verify_text(path: &Path, text: &str) -> Result<(), String> {
    if path.exists() {
        validate_private_file(path, text.len() as u64)?;
        if fs::read(path).map_err(|_| "local_install_state_invalid")? != text.as_bytes() {
            return Err("local_install_policy_integrity_failed".into());
        }
        return Ok(());
    }
    publish_bytes(path, text.as_bytes())
}

fn publish_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "local_install_state_write_failed")?;
    if path.exists() {
        validate_private_file(path, bytes.len() as u64)?;
        if fs::read(path).map_err(|_| "local_install_state_invalid")? != bytes {
            return Err("local_install_state_integrity_failed".into());
        }
        return Ok(());
    }
    publish_bytes(path, &bytes)
}

fn publish_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut nonce = [0_u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        hex_encode(&nonce)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|_| "local_install_state_write_failed")?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| "local_install_state_write_failed")?;
        file.sync_all()
            .map_err(|_| "local_install_state_write_failed")?;
        match fs::hard_link(&temp, path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    File::open(parent)
                        .and_then(|f| f.sync_all())
                        .map_err(|_| "local_install_state_write_failed")?;
                }
                Ok(())
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                validate_private_file(path, bytes.len() as u64)?;
                if fs::read(path).map_err(|_| "local_install_state_write_failed")? == bytes {
                    Ok(())
                } else {
                    Err("local_install_state_integrity_failed".into())
                }
            }
            Err(_) => Err("local_install_state_write_failed".to_string()),
        }
    })();
    let _ = fs::remove_file(temp);
    result
}

fn read_installation(path: &Path) -> Result<Installation, String> {
    let metadata = symlink_metadata(path)?;
    if metadata.len() > 8192 {
        return Err("local_installation_record_invalid".into());
    }
    validate_private_file(path, metadata.len())?;
    let bytes = fs::read(path).map_err(|_| "local_installation_record_invalid")?;
    serde_json::from_slice(&bytes).map_err(|_| "local_installation_record_invalid".into())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|_| "local_install_binary_unavailable")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("local_install_path_not_absolute".into());
    }
    Ok(path.to_path_buf())
}

fn absolute_binary(path: &Path) -> Result<PathBuf, String> {
    absolute(path)?
        .canonicalize()
        .map_err(|_| "local_install_binary_unavailable".into())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn symlink_metadata(path: &Path) -> Result<fs::Metadata, String> {
    fs::symlink_metadata(path).map_err(|_| "local_install_state_unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_install_test_child() {
        if std::env::var_os("TRADEASSEMBLY_INSTALL_TEST_SLEEP").is_some() {
            std::thread::sleep(Duration::from_secs(5));
        }
    }

    #[test]
    fn local_install_child_failures_and_timeouts_are_errors() {
        let executable = std::env::current_exe().unwrap();
        let mut bad = Command::new(&executable);
        bad.stdout(Stdio::null()).stderr(Stdio::null());
        assert_eq!(
            run_bounded_for(
                &mut bad,
                &["--invalid-install-test-option".into()],
                Duration::from_secs(2)
            )
            .unwrap_err(),
            "local_install_command_failed"
        );
        let mut slow = Command::new(executable);
        slow.env("TRADEASSEMBLY_INSTALL_TEST_SLEEP", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        assert_eq!(
            run_bounded_for(
                &mut slow,
                &[
                    "--exact".into(),
                    "local_install::tests::local_install_test_child".into()
                ],
                Duration::from_millis(30)
            )
            .unwrap_err(),
            "local_install_command_timeout"
        );
    }

    #[test]
    fn local_install_does_not_adopt_unmarked_recognized_files() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let state = prepare_state_root(&root.join("state")).unwrap();
        let binary = root.join("warden");
        fs::write(&binary, b"not executed").unwrap();
        private_token(&state.join("warden.token")).unwrap();
        assert_eq!(
            prepare(&state, &binary, 8181).unwrap_err(),
            "local_install_state_unrecognized"
        );
        assert!(!state.join("installation.json").exists());
    }

    #[test]
    fn local_install_private_material_is_stable_and_detects_changes() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().canonicalize().unwrap();
        let key = state.join("key");
        private_bytes(&key, 32).unwrap();
        let before = fs::read(&key).unwrap();
        private_bytes(&key, 32).unwrap();
        assert_eq!(fs::read(&key).unwrap(), before);
        let token = state.join("token");
        private_token(&token).unwrap();
        let before = fs::read(&token).unwrap();
        private_token(&token).unwrap();
        assert_eq!(fs::read(&token).unwrap(), before);
        let asset = state.join("asset");
        publish_or_verify_text(&asset, "original").unwrap();
        assert!(publish_or_verify_text(&asset, "modified").is_err());
        assert_eq!(fs::read(&asset).unwrap(), b"original");
    }

    #[test]
    fn local_install_rejects_relative_paths_and_unknown_state() {
        assert!(prepare_state_root(Path::new("relative")).is_err());
        let root = tempfile::tempdir().unwrap();
        let state = root.path().canonicalize().unwrap();
        fs::write(state.join("unrelated"), b"preserve").unwrap();
        assert!(prepare_state_root(&state).is_err());
        assert_eq!(fs::read(state.join("unrelated")).unwrap(), b"preserve");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_install_normalizes_only_the_system_tmp_alias() {
        assert_eq!(
            system_state_path(Path::new("/tmp/tradeassembly-test")),
            Path::new("/private/tmp/tradeassembly-test")
        );
        assert_eq!(
            system_state_path(Path::new("/Users/example/test")),
            Path::new("/Users/example/test")
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_install_rejects_symlinks_and_insecure_existing_state() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let state = root.path().canonicalize().unwrap();
        let key = state.join("key");
        private_bytes(&key, 32).unwrap();
        let alias = state.join("alias");
        symlink(&key, &alias).unwrap();
        assert!(private_bytes(&alias, 32).is_err());
        fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(private_bytes(&key, 32).is_err());
        let directory = state.join("insecure");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_state_root(&directory).is_err());
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn local_install_rejects_changed_binary_before_execution() {
        let root = tempfile::tempdir().unwrap();
        let state = prepare_state_root(&root.path().canonicalize().unwrap().join("state")).unwrap();
        let binary = state.join("binary");
        fs::write(&binary, b"not an executable").unwrap();
        publish_json(
            &state.join("installation.json"),
            &Installation {
                schema_version: SCHEMA.into(),
                service_version: REQUIRED_WARDEN_SERVICE_VERSION.into(),
                executable: binary.display().to_string(),
                sha256: "0".repeat(64),
                port: 8181,
            },
        )
        .unwrap();
        assert_eq!(
            policy_command(&state).unwrap_err(),
            "local_installation_integrity_failed"
        );
    }
}
