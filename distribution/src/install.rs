// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::{atomic_json, digest, executable, extract, read_json, verify_bundle, Release, Result};
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct OwnedPolicy(Child);
impl Drop for OwnedPolicy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bounded_output(command: &mut Command) -> Result<(bool, Value)> {
    let output = tempfile::tempfile().map_err(|_| "command_capture_unavailable")?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            output
                .try_clone()
                .map_err(|_| "command_capture_unavailable")?,
        ))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "command_launch_failed")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("command_timeout_or_wait_failed".into());
            }
        }
    };
    if output
        .metadata()
        .map_err(|_| "command_capture_failed")?
        .len()
        > 1_048_576
    {
        return Err("command_output_limit".into());
    }
    use std::io::{Read, Seek};
    let mut output = output;
    output.rewind().map_err(|_| "command_capture_failed")?;
    let mut bytes = Vec::new();
    output
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| "command_capture_failed")?;
    let value = serde_json::from_slice(&bytes).map_err(|_| "command_result_invalid")?;
    Ok((status.success(), value))
}

#[cfg(windows)]
fn bounded_status(command: &mut Command, timeout: std::time::Duration) -> Result<bool> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "windows_sandbox_setup_launch_failed")?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("windows_sandbox_setup_timeout_or_wait_failed".into());
            }
        }
    }
}

#[cfg(any(windows, test))]
fn windows_srt_install_needed(status: &Value) -> Result<bool> {
    let exists = status["user"]["user"]["exists"]
        .as_bool()
        .ok_or("windows_sandbox_status_invalid")?;
    let credential = status["user"]["cred_present"]
        .as_bool()
        .ok_or("windows_sandbox_status_invalid")?;
    let wfp = status["wfp"]["state"]
        .as_str()
        .ok_or("windows_sandbox_status_invalid")?;
    if !exists {
        // A non-elevated caller may not inspect WFP at all.
        return if !credential && matches!(wfp, "absent" | "cannot-read") {
            Ok(true)
        } else {
            Err("windows_sandbox_partial_install_requires_repair".into())
        };
    }
    if credential && matches!(wfp, "installed" | "cannot-read") {
        // A shared machine account already exists. Never rotate its password
        // during an ordinary TradeAssembly install or upgrade.
        return Ok(false);
    }
    Err("windows_sandbox_existing_install_requires_repair".into())
}

#[cfg(any(windows, test))]
fn windows_srt_port_range(status: &Value) -> Result<Option<[u16; 2]>> {
    let raw = &status["wfp"]["port_range"];
    if raw.is_null() {
        return Ok(None);
    }
    let ports = raw.as_array().ok_or("windows_sandbox_port_range_invalid")?;
    if ports.len() != 2 {
        return Err("windows_sandbox_port_range_invalid".into());
    }
    let lo = u16::try_from(
        ports[0]
            .as_u64()
            .ok_or("windows_sandbox_port_range_invalid")?,
    )
    .map_err(|_| "windows_sandbox_port_range_invalid")?;
    let hi = u16::try_from(
        ports[1]
            .as_u64()
            .ok_or("windows_sandbox_port_range_invalid")?,
    )
    .map_err(|_| "windows_sandbox_port_range_invalid")?;
    if lo == 0 || hi < lo || hi - lo > 64 {
        return Err("windows_sandbox_port_range_invalid".into());
    }
    Ok(Some([lo, hi]))
}

#[cfg(windows)]
fn windows_srt_range_bindable(range: [u16; 2]) -> bool {
    let mut reserved = Vec::new();
    for port in range[0]..=range[1] {
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => reserved.push(listener),
            Err(_) => return false,
        }
    }
    reserved.len() == usize::from(range[1] - range[0] + 1)
}

#[cfg(windows)]
fn choose_windows_srt_port_range() -> Result<[u16; 2]> {
    for range in [
        [60080, 60089],
        [40080, 40089],
        [40090, 40099],
        [40100, 40109],
    ] {
        if windows_srt_range_bindable(range) {
            return Ok(range);
        }
    }
    Err("windows_sandbox_no_available_proxy_port_range".into())
}

#[cfg(any(windows, test))]
fn stored_windows_srt_port_range(root: &Path) -> Result<Option<[u16; 2]>> {
    let record_path = root.join("windows-srt.json");
    let record = if record_path.exists() {
        let record: Value = read_json(&record_path)?;
        if record["schemaVersion"] != 1 {
            return Err("windows_sandbox_port_range_record_invalid".into());
        }
        Some(
            windows_srt_port_range(&json!({"wfp":{"port_range":record["portRange"]}}))?
                .ok_or("windows_sandbox_port_range_record_invalid")?,
        )
    } else {
        None
    };
    let config_path = root.join("state/local/runtime.json");
    let config = if config_path.exists() {
        let config: Value = read_json(&config_path)?;
        windows_srt_port_range(
            &json!({"wfp":{"port_range":config["pluginSandboxWindowsProxyPortRange"]}}),
        )?
    } else {
        None
    };
    if record.is_some() && config.is_some() && record != config {
        return Err("windows_sandbox_port_range_changed".into());
    }
    Ok(record.or(config))
}

#[cfg(windows)]
struct WindowsSrtReady {
    provisioned_now: bool,
    port_range: [u16; 2],
}

#[cfg(windows)]
fn ensure_windows_srt(bundle: &Path, root: &Path) -> Result<WindowsSrtReady> {
    let srt = bundle
        .join("runtime/node_modules/@anthropic-ai/sandbox-runtime/vendor/srt-win/x64/srt-win.exe");
    let read_status = || -> Result<Value> {
        let (success, status) = bounded_output(Command::new(&srt).arg("status"))?;
        if !success {
            return Err("windows_sandbox_status_unavailable".into());
        }
        Ok(status)
    };
    let before = read_status()?;
    let provisioned_now = windows_srt_install_needed(&before)?;
    let stored = stored_windows_srt_port_range(root)?;
    let observed = windows_srt_port_range(&before)?;
    if stored.is_some() && observed.is_some() && stored != observed {
        return Err("windows_sandbox_port_range_changed".into());
    }
    let port_range = if provisioned_now {
        if let Some(stored) = stored {
            if !windows_srt_range_bindable(stored) {
                return Err("windows_sandbox_recorded_proxy_range_unavailable".into());
            }
            stored
        } else {
            choose_windows_srt_port_range()?
        }
    } else {
        observed.or(stored).unwrap_or([60080, 60089])
    };
    if provisioned_now {
        // Persist before UAC: retry after cancellation or a process crash
        // cannot silently select another machine-level WFP range.
        if stored.is_none() {
            atomic_json(
                &root.join("windows-srt.json"),
                &json!({"schemaVersion":1,"portRange":port_range}),
            )?;
        }
        let node = bundle.join(executable("runtime/node/bin/node"));
        let cli = bundle.join("runtime/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js");
        let installed = bounded_status(
            Command::new(node)
                .arg(cli)
                .arg("windows-install")
                .arg("--proxy-port-range")
                .arg(format!("{}-{}", port_range[0], port_range[1])),
            std::time::Duration::from_secs(180),
        )?;
        if !installed {
            return Err("windows_sandbox_setup_failed_or_cancelled".into());
        }
    }
    let after = read_status()?;
    if windows_srt_install_needed(&after)? {
        return Err("windows_sandbox_setup_unverified".into());
    }
    if let Some(actual) = windows_srt_port_range(&after)? {
        if actual != port_range {
            return Err("windows_sandbox_port_range_changed".into());
        }
    }
    // A non-elevated status check may report WFP as `cannot-read`. SRT's
    // initialize() makes a behavioral egress-fence probe, so exercise the
    // actual bundled launcher before recording a runnable installation.
    let mut settings = tempfile::NamedTempFile::new_in(root)
        .map_err(|_| "windows_sandbox_probe_config_unavailable")?;
    let config = json!({
        "network":{"allowedDomains":[],"deniedDomains":[],"strictAllowlist":true,"allowUnixSockets":[],"allowLocalBinding":false},
        "filesystem":{"denyRead":[],"allowRead":[bundle],"allowWrite":[],"denyWrite":[]},
        "windows":{"proxyPortRange":port_range},
        "enableWeakerNestedSandbox":false,"enableWeakerNetworkIsolation":false,"allowAppleEvents":false
    });
    use std::io::Write;
    settings
        .write_all(
            &serde_json::to_vec(&config).map_err(|_| "windows_sandbox_probe_config_invalid")?,
        )
        .and_then(|_| settings.flush())
        .map_err(|_| "windows_sandbox_probe_config_unavailable")?;
    let launcher = bundle.join(executable("bin/tradeassembly-sandbox"));
    let node = bundle.join(executable("runtime/node/bin/node"));
    let ready = bounded_status(
        Command::new(launcher)
            .current_dir(bundle)
            .arg("--settings")
            .arg(settings.path())
            .arg(node)
            .args(["--eval", "process.exit(0)"]),
        std::time::Duration::from_secs(45),
    )?;
    if !ready {
        return Err("windows_sandbox_readiness_failed".into());
    }
    if stored.is_none() && !provisioned_now {
        atomic_json(
            &root.join("windows-srt.json"),
            &json!({"schemaVersion":1,"portRange":port_range}),
        )?;
    }
    Ok(WindowsSrtReady {
        provisioned_now,
        port_range,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Installed {
    pub schema_version: u32,
    pub current: Release,
    pub previous: Option<Release>,
    pub warden_port: u16,
}

pub fn host_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok(crate::TARGETS[0]),
        ("macos", "x86_64") => Ok(crate::TARGETS[1]),
        ("linux", "x86_64") => Ok(crate::TARGETS[2]),
        ("linux", "aarch64") => Ok(crate::TARGETS[3]),
        ("windows", "x86_64") => Ok(crate::TARGETS[4]),
        _ => Err("platform_unsupported".into()),
    }
}

pub fn default_root() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(
            PathBuf::from(std::env::var_os("HOME").ok_or("home_unavailable")?)
                .join("Library/Application Support/TradeAssembly"),
        )
    }
    #[cfg(target_os = "linux")]
    {
        Ok(std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or(
                PathBuf::from(std::env::var_os("HOME").ok_or("home_unavailable")?)
                    .join(".local/share"),
            )
            .join("tradeassembly"))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(
            PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("home_unavailable")?)
                .join("TradeAssembly"),
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err("platform_unsupported".into())
    }
}

fn private_dir(path: &Path) -> Result<()> {
    #[cfg(windows)]
    return crate::windows_private::ensure_directory(path)
        .map_err(|_| "install_permissions_failed".into());
    #[cfg(not(windows))]
    {
        fs::create_dir_all(path).map_err(|_| "install_directory_failed")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|_| "install_permissions_failed")?;
        }
        Ok(())
    }
}

fn root_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("installation_root_must_be_absolute".into());
    }
    // Canonicalize the existing parent (macOS /var is a system link); the owned
    // root itself and its children must not be links.
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir()) {
        return Err("installation_root_unsafe".into());
    }
    if path.exists() {
        let marker = path.join("distribution-owner.json");
        if !marker.exists()
            && fs::read_dir(path)
                .map_err(|_| "installation_inspection_failed")?
                .next()
                .is_some()
        {
            return Err("installation_root_not_owned_choose_empty_directory".into());
        }
        if marker.exists() {
            let owner: Value = read_json(&marker)?;
            if owner != json!({"schemaVersion":"tradeassembly.distribution-owner.v1"}) {
                return Err("installation_owner_invalid".into());
            }
        }
    }
    private_dir(path)?;
    let root = path
        .canonicalize()
        .map_err(|_| "installation_root_unavailable")?;
    if !root.join("distribution-owner.json").exists() {
        atomic_json(
            &root.join("distribution-owner.json"),
            &json!({"schemaVersion":"tradeassembly.distribution-owner.v1"}),
        )?;
    }
    for entry in walkdir::WalkDir::new(&root)
        .follow_links(false)
        .max_depth(3)
    {
        let entry = entry.map_err(|_| "installation_inspection_failed")?;
        // Links are permitted only in immutable, verified payloads.
        if entry.file_type().is_symlink()
            && (entry.depth() <= 2
                || !entry.path().is_file()
                || !entry.path().starts_with(root.join("versions")))
        {
            return Err("installation_link_forbidden".into());
        }
    }
    Ok(root)
}

fn lock(root: &Path, exclusive: bool) -> Result<File> {
    let path = root.join("distribution.lock");
    if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
        return Err("installation_lock_unsafe".into());
    }
    #[cfg(not(windows))]
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| "installation_lock_unavailable")?;
    #[cfg(windows)]
    let file =
        crate::windows_private::open_lock(&path).map_err(|_| "installation_lock_unavailable")?;
    let outcome = if exclusive {
        file.try_lock_exclusive()
    } else {
        FileExt::try_lock_shared(&file)
    };
    outcome.map_err(|_| "rig_running_or_installation_busy")?;
    Ok(file)
}

pub fn stopped_database(path: &Path, now_ms: i64) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "state_database_unavailable")?;
    db.busy_timeout(std::time::Duration::from_millis(100))
        .map_err(|_| "state_database_busy")?;
    let exists = |name: &str| -> Result<bool> {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [name],
            |r| r.get(0),
        )
        .map_err(|_| "state_schema_invalid".into())
    };
    if !exists("tradeassembly_kv")? {
        return Err("state_schema_unknown".into());
    }
    let mut statement = db.prepare("SELECT namespace, value_json FROM tradeassembly_kv WHERE namespace IN ('execution_activations','execution_runs','agent_deployments','agent_runs','agent_active_runs')").map_err(|_| "state_schema_invalid")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "state_read_failed")?;
    for row in rows {
        let (namespace, encoded) = row.map_err(|_| "state_read_failed")?;
        let value: Value = serde_json::from_str(&encoded).map_err(|_| "state_record_invalid")?;
        let state = match namespace.as_str() {
            "agent_deployments" => value.pointer("/deployment/desiredState"),
            "agent_runs" => value.pointer("/run/state"),
            _ => value.get("state"),
        }
        .and_then(Value::as_str)
        .ok_or("state_record_unknown")?;
        let inactive = match namespace.as_str() {
            "agent_deployments" => matches!(state, "stopped" | "paused" | "draft"),
            "agent_runs" | "agent_active_runs" => matches!(
                state,
                "completed"
                    | "failed"
                    | "stopped"
                    | "paused"
                    | "expired"
                    | "cancelled"
                    | "interrupted"
                    | "reconciled"
            ),
            _ => matches!(
                state,
                "stopped" | "paused" | "completed" | "failed" | "cancelled" | "draft"
            ),
        };
        if !inactive {
            return Err("rig_has_active_or_unknown_execution_state".into());
        }
    }
    if exists("runtime_leases")? {
        let leases: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM runtime_leases WHERE expires_at_ms > ?1",
                [now_ms],
                |r| r.get(0),
            )
            .map_err(|_| "lease_schema_invalid")?;
        if leases > 0 {
            return Err("rig_has_unexpired_lease".into());
        }
    }
    Ok(())
}

fn stopped(root: &Path) -> Result<()> {
    let config_path = root.join("state/local/runtime.json");
    if config_path.exists() {
        let config: Value = read_json(&config_path)?;
        if ["postgresUrlRef", "natsUrlRef"]
            .iter()
            .any(|key| config.get(*key).is_some_and(|value| !value.is_null()))
        {
            return Err("upgrade_external_state_requires_qualified_migration".into());
        }
    }
    // Inspect only executable paths. Never read/emit command lines or environments.
    let system = sysinfo::System::new_all();
    if system
        .processes()
        .values()
        .filter_map(|p| p.exe())
        .any(|path| {
            path.starts_with(root.join("versions")) || path.starts_with(root.join("authority"))
        })
    {
        return Err("rig_process_running_stop_owned_services_first".into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock_unavailable")?
        .as_millis();
    let now = i64::try_from(now).map_err(|_| "clock_unavailable")?;
    let state = root.join("state/local");
    // Discover actual SQLite files without assuming a developer's filename.
    for entry in walkdir::WalkDir::new(&state)
        .follow_links(false)
        .max_depth(2)
    {
        let entry = entry.map_err(|_| "state_inspection_failed")?;
        if entry.file_type().is_symlink() {
            return Err("state_link_forbidden".into());
        }
        if entry.file_type().is_file() {
            let mut f = File::open(entry.path()).map_err(|_| "state_read_failed")?;
            let mut header = [0; 16];
            if std::io::Read::read_exact(&mut f, &mut header).is_ok()
                && &header == b"SQLite format 3\0"
            {
                // Policy database has a different schema; validate it separately.
                let db =
                    Connection::open_with_flags(entry.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
                        .map_err(|_| "state_database_unavailable")?;
                let core: bool = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='tradeassembly_kv')",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|_| "state_schema_invalid")?;
                if core {
                    stopped_database(entry.path(), now)?;
                } else if entry.file_name() != "warden.sqlite" {
                    return Err("state_database_schema_unknown".into());
                }
            }
        }
    }
    Ok(())
}

fn installed(root: &Path) -> Result<Option<Installed>> {
    let path = root.join("installed.json");
    if !path.exists() {
        return Ok(None);
    }
    let state: Installed = read_json(&path)?;
    state.current.validate()?;
    if state.schema_version != 1 || state.warden_port == 0 {
        return Err("installation_record_invalid".into());
    }
    if let Some(previous) = &state.previous {
        previous.validate()?;
    }
    Ok(Some(state))
}

fn compatible(old: &Release, new: &Release) -> Result<()> {
    if old.target != new.target
        || old.warden_sha256 != new.warden_sha256
        || old.state_compatibility != new.state_compatibility
    {
        return Err("upgrade_requires_qualified_authority_or_state_migration".into());
    }
    Ok(())
}

fn prepare(
    root: &Path,
    release: &Release,
    port: u16,
    windows_range: Option<[u16; 2]>,
) -> Result<()> {
    let payload = root.join("versions").join(&release.archive_sha256);
    let runtime = payload.join(executable("bin/tradeassembly"));
    let state = root.join("state/local");
    let config_path = state.join("runtime.json");
    let authority = root.join(executable("authority/bin/warden"));
    let port = port.to_string();
    #[cfg(windows)]
    if config_path.exists() {
        let existing: Value = read_json(&config_path)?;
        if let Some(range) = existing.get("pluginSandboxWindowsProxyPortRange") {
            if range != &json!(windows_range.ok_or("windows_sandbox_port_range_missing")?) {
                return Err("windows_sandbox_port_range_changed".into());
            }
        }
    }
    #[cfg(not(windows))]
    let _ = windows_range;
    if config_path.exists() {
        // Frozen Core setup verifies its original bootstrap JSON byte-for-byte;
        // it is not a user-config migration API. Validate existing bindings and
        // update only the versioned sandbox path, leaving user settings intact.
        let mut prior = installed(root)?.unwrap_or(Installed {
            schema_version: 1,
            current: release.clone(),
            previous: None,
            warden_port: port.parse().map_err(|_| "warden_port_invalid")?,
        });
        let mut config: Value = read_json(&config_path)?;
        let sandbox = payload.join(executable("bin/tradeassembly-sandbox"));
        let next = Value::String(sandbox.to_str().ok_or("installation_path_invalid")?.into());
        // A retry may already have switched this binding before registration
        // failed. Only the verified prior or this staged release is admissible.
        if config["pluginSandboxCommand"] == next {
            prior.current = release.clone();
        }
        verify_prepared(root, &prior)?;
        if config["pluginSandboxCommand"] != next {
            config["pluginSandboxCommand"] = next;
            atomic_json(&config_path, &config)?;
        }
    } else {
        let mut setup = Command::new(&runtime);
        setup
            .args(["setup", "--state-dir"])
            .arg(&state)
            .arg("--warden-binary")
            .arg(&authority)
            .args(["--warden-port", &port]);
        let (success, result) = bounded_output(&mut setup)?;
        if !success {
            return Err("runtime_setup_failed_no_pointer_changed".into());
        }
        // Structured setup may include local identity details; don't print its stdout.
        if result.get("prepared").and_then(Value::as_bool) != Some(true)
            || result.get("automationReady").and_then(Value::as_bool) != Some(false)
        {
            return Err("runtime_setup_result_invalid".into());
        }
    }
    #[cfg(windows)]
    {
        let mut config: Value = read_json(&config_path)?;
        config["pluginSandboxWindowsProxyPortRange"] =
            json!(windows_range.ok_or("windows_sandbox_port_range_missing")?);
        atomic_json(&config_path, &config)?;
    }
    // Offline plugin registration is protected by real Warden. Run only the
    // freshly prepared, digest-bound local authority for this operation. It
    // does not evaluate a strategy and is always reaped, including on failure.
    let port_number: u16 = port.parse().map_err(|_| "warden_port_invalid")?;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port_number));
    let reservation = std::net::TcpListener::bind(address).map_err(|_| "warden_port_in_use")?;
    drop(reservation);
    let mut policy = OwnedPolicy(
        Command::new(&authority)
            .arg("--database")
            .arg(state.join("warden.sqlite"))
            .arg("--key-file")
            .arg(state.join("signing.seed"))
            .args(["--key-id", "tradeassembly-local-key"])
            .arg("--peps")
            .arg(state.join("peps.json"))
            .args(["serve", "--bind", &address.to_string()])
            .arg("--token-file")
            .arg(state.join("warden.token"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "setup_policy_start_failed")?,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if policy
            .0
            .try_wait()
            .map_err(|_| "setup_policy_wait_failed")?
            .is_some()
        {
            return Err("setup_policy_exited".into());
        }
        if std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(50))
            .is_ok()
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("setup_policy_ready_timeout".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let mut plugin = Command::new(runtime);
    plugin
        .arg("--config")
        .arg(state.join("runtime.json"))
        .args(["plugins", "install-default", "--offline"]);
    let (success, response) = bounded_output(&mut plugin)?;
    if !success || response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err("offline_plugin_install_failed_no_pointer_changed".into());
    }
    Ok(())
}

pub fn install(package: &Path, root: &Path, upgrade: bool, port: u16) -> Result<Value> {
    let release: Release = read_json(&package.join("release.json"))?;
    release.validate_candidate()?;
    if release.target != host_target()? {
        return Err("package_host_target_mismatch".into());
    }
    if port == 0 {
        return Err("warden_port_invalid".into());
    }
    if digest(&package.join("bundle.tar.gz"))? != release.archive_sha256 {
        return Err("archive_digest_mismatch".into());
    }
    let root = root_path(root)?;
    let _lock = lock(&root, true)?;
    let current = installed(&root)?;
    if let Some(current) = &current {
        compatible(&current.current, &release)?;
        if current.warden_port != port {
            return Err("warden_port_change_requires_migration".into());
        }
        if current.current != release && !upgrade {
            return Err("existing_installation_use_upgrade".into());
        }
    } else if root.join("state").exists() && !root.join("pending.json").exists() {
        return Err("unrecognized_existing_state".into());
    }
    if let Some(current) = &current {
        if current.current == release && !root.join("pending.json").exists() {
            verify_installed(&root, current)?;
            #[cfg(windows)]
            ensure_windows_srt(&root.join("versions").join(&release.archive_sha256), &root)?;
            return status(&root);
        }
    }
    if root.join("state/local").exists() {
        stopped(&root)?;
    }
    for dir in ["versions", "authority/bin", "state", "bin"] {
        private_dir(&root.join(dir))?;
    }
    let destination = root.join("versions").join(&release.archive_sha256);
    if !destination.exists() {
        let stage = tempfile::Builder::new()
            .prefix("stage-")
            .tempdir_in(root.join("versions"))
            .map_err(|_| "stage_create_failed")?;
        #[cfg(windows)]
        crate::windows_private::protect_empty_staging_directory(stage.path())
            .map_err(|_| "stage_create_failed")?;
        extract(&package.join("bundle.tar.gz"), stage.path())?;
        verify_bundle(stage.path(), &release)?;
        crate::candidate::verify(package, stage.path(), &release)?;
        #[cfg(not(windows))]
        fs::rename(stage.path(), &destination).map_err(|_| "payload_commit_failed")?;
        #[cfg(windows)]
        crate::windows_private::publish_directory(stage.path(), &destination)
            .map_err(|_| "payload_commit_failed")?;
    }
    verify_bundle(&destination, &release)?;
    crate::candidate::verify(package, &destination, &release)?;
    // Native Windows SRT is machine-scoped. Provision once before any
    // installed pointer can make this version runnable. A cancelled UAC
    // prompt leaves an inert staged payload and can be retried explicitly.
    #[cfg(windows)]
    let windows_sandbox = ensure_windows_srt(&destination, &root)?;
    let authority = root.join(executable("authority/bin/warden"));
    if !authority.exists() {
        #[cfg(not(windows))]
        fs::copy(destination.join(executable("bin/warden")), &authority)
            .map_err(|_| "authority_install_failed")?;
        #[cfg(windows)]
        crate::windows_private::copy_private(
            &destination.join(executable("bin/warden")),
            &authority,
            false,
        )
        .map_err(|_| "authority_install_failed")?;
    }
    if digest(&authority)? != release.warden_sha256 {
        return Err("installed_authority_changed".into());
    }
    let next = Installed {
        schema_version: 1,
        current: release,
        previous: current.as_ref().map(|s| s.current.clone()),
        warden_port: port,
    };
    atomic_json(&root.join("pending.json"), &next)?;
    #[cfg(windows)]
    let windows_range = Some(windows_sandbox.port_range);
    #[cfg(not(windows))]
    let windows_range = None;
    prepare(&root, &next.current, port, windows_range)?;
    // Stable launcher copied outside npm cache. It finds its installation from
    // its own executable location, not caller-controlled runtime configuration.
    let facade = root.join(executable("bin/tradeassembly"));
    let source = std::env::current_exe().map_err(|_| "installer_path_unavailable")?;
    #[cfg(not(windows))]
    {
        let temp = root.join(executable("bin/tradeassembly-next"));
        fs::copy(source, &temp).map_err(|_| "facade_install_failed")?;
        fs::rename(temp, facade).map_err(|_| "facade_install_failed")?;
    }
    #[cfg(windows)]
    crate::windows_private::copy_private(&source, &facade, true)
        .map_err(|_| "facade_install_failed")?;
    atomic_json(&root.join("installed.json"), &next)?;
    remove_pending(&root)?;
    #[cfg(not(windows))]
    return status(&root);
    #[cfg(windows)]
    {
        let mut result = status(&root)?;
        result["windowsSandboxProvisionedNow"] = json!(windows_sandbox.provisioned_now);
        result["windowsSandboxBehavioralReady"] = json!(true);
        result["windowsSandboxProxyPortRange"] = json!(windows_sandbox.port_range);
        Ok(result)
    }
}

fn verify_installed(root: &Path, record: &Installed) -> Result<()> {
    verify_bundle(
        &root.join("versions").join(&record.current.archive_sha256),
        &record.current,
    )?;
    if digest(&root.join(executable("authority/bin/warden")))? != record.current.warden_sha256 {
        return Err("installed_authority_changed".into());
    }
    verify_prepared(root, record)?;
    Ok(())
}

fn verify_prepared(root: &Path, record: &Installed) -> Result<()> {
    let state = root.join("state/local");
    let authority = root.join(executable("authority/bin/warden"));
    let local: Value = read_json(&state.join("installation.json"))?;
    if local["schemaVersion"] != "tradeassembly.local-installation.v1"
        || local["executable"] != authority.to_str().ok_or("installation_path_invalid")?
        || local["sha256"] != record.current.warden_sha256
        || local["port"] != record.warden_port
    {
        return Err("local_authority_binding_changed".into());
    }
    let config: Value = read_json(&state.join("runtime.json"))?;
    let sandbox = root
        .join("versions")
        .join(&record.current.archive_sha256)
        .join(executable("bin/tradeassembly-sandbox"));
    if config["databasePath"]
        != state
            .join("runtime.db")
            .to_str()
            .ok_or("installation_path_invalid")?
        || config["pluginSandboxCommand"] != sandbox.to_str().ok_or("installation_path_invalid")?
        || config["wardenSidecarUrl"] != format!("http://127.0.0.1:{}", record.warden_port)
        || config["wardenTokenRef"] != format!("file://{}", state.join("warden.token").display())
    {
        return Err("local_runtime_binding_changed".into());
    }
    for (name, length) in [("signing.seed", 32), ("warden.token", 64)] {
        let metadata = fs::symlink_metadata(state.join(name))
            .map_err(|_| "local_authority_material_unavailable")?;
        if !metadata.is_file() || metadata.len() != length {
            return Err("local_authority_material_invalid".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err("local_authority_material_insecure".into());
            }
        }
    }
    Ok(())
}

pub fn status(root: &Path) -> Result<Value> {
    let root = root
        .canonicalize()
        .map_err(|_| "installation_unavailable")?;
    let state = installed(&root)?.ok_or("installation_not_prepared")?;
    if root.join("pending.json").exists() {
        return Err("installation_transaction_pending_rerun_install".into());
    }
    verify_installed(&root, &state)?;
    Ok(
        json!({"schemaVersion":1,"installed":true,"version":state.current.version,"target":state.current.target,"payloadSha256":state.current.archive_sha256,"command":root.join(executable("bin/tradeassembly")),"stateDirectory":root.join("state/local"),"nextCommands":[{"command":root.join(executable("bin/tradeassembly")),"args":["policy","serve","--state-dir",root.join("state/local")]}],"mcp":{"command":root.join(executable("bin/tradeassembly")),"args":["--config",root.join("state/local/runtime.json"),"mcp","serve"]},"installationActivatedStrategy":false,"brokerConnectionStatus":"not_checked"}),
    )
}

pub fn rollback(root: &Path) -> Result<Value> {
    let root = root_path(root)?;
    let _lock = lock(&root, true)?;
    let state = installed(&root)?.ok_or("installation_not_prepared")?;
    let previous = state.previous.clone().ok_or("rollback_unavailable")?;
    compatible(&state.current, &previous)?;
    stopped(&root)?;
    let next = Installed {
        schema_version: 1,
        current: previous,
        previous: Some(state.current),
        warden_port: state.warden_port,
    };
    if root.join("pending.json").exists() {
        let pending: Installed = read_json(&root.join("pending.json"))?;
        if pending != next {
            return Err("different_installation_transaction_pending".into());
        }
    }
    verify_bundle(
        &root.join("versions").join(&next.current.archive_sha256),
        &next.current,
    )?;
    if digest(&root.join(executable("authority/bin/warden")))? != next.current.warden_sha256 {
        return Err("installed_authority_changed".into());
    }
    atomic_json(&root.join("pending.json"), &next)?;
    #[cfg(windows)]
    let windows_range = Some(
        ensure_windows_srt(
            &root.join("versions").join(&next.current.archive_sha256),
            &root,
        )?
        .port_range,
    );
    #[cfg(not(windows))]
    let windows_range = None;
    prepare(&root, &next.current, next.warden_port, windows_range)?;
    atomic_json(&root.join("installed.json"), &next)?;
    remove_pending(&root)?;
    status(&root)
}

fn remove_pending(root: &Path) -> Result<()> {
    #[cfg(windows)]
    return crate::windows_private::remove_durable(&root.join("pending.json"))
        .map_err(|_| "transaction_cleanup_failed".into());
    #[cfg(not(windows))]
    fs::remove_file(root.join("pending.json")).map_err(|_| "transaction_cleanup_failed".into())
}

pub fn run(root: &Path, args: &[String]) -> Result<i32> {
    let root = root
        .canonicalize()
        .map_err(|_| "installation_unavailable")?;
    let _lock = lock(&root, false)?;
    if root.join("pending.json").exists() {
        return Err("installation_transaction_pending_rerun_install".into());
    }
    let state = installed(&root)?.ok_or("installation_not_prepared")?;
    verify_prepared(&root, &state)?;
    // Validate the executable on every start, full inventory in status/install.
    let payload = root.join("versions").join(&state.current.archive_sha256);
    let bundle: Value = read_json(&payload.join("bundle.json"))?;
    if digest(&payload.join("bundle.json"))? != state.current.bundle_manifest_sha256 {
        return Err("bundle_manifest_digest_mismatch".into());
    }
    let runtime = payload.join(executable("bin/tradeassembly"));
    if digest(&runtime)?
        != bundle["files"][executable("bin/tradeassembly")]["sha256"]
            .as_str()
            .ok_or("runtime_digest_unavailable")?
    {
        return Err("runtime_digest_mismatch".into());
    }
    let status = Command::new(runtime)
        .args(args)
        .status()
        .map_err(|_| "runtime_launch_failed")?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_srt_setup_is_one_time_and_partial_state_fails_closed() {
        let status = |exists, credential, wfp| json!({"user":{"user":{"exists":exists},"cred_present":credential},"wfp":{"state":wfp}});
        assert!(windows_srt_install_needed(&status(false, false, "absent")).unwrap());
        assert!(windows_srt_install_needed(&status(false, false, "cannot-read")).unwrap());
        for wfp in ["installed", "cannot-read"] {
            assert!(!windows_srt_install_needed(&status(true, true, wfp)).unwrap());
        }
        for partial in [
            status(true, false, "installed"),
            status(true, true, "absent"),
            status(false, true, "absent"),
            status(false, false, "installed"),
        ] {
            assert!(windows_srt_install_needed(&partial).is_err());
        }
        assert!(windows_srt_install_needed(&json!({})).is_err());
    }
    #[test]
    fn windows_srt_range_requires_a_bounded_observed_binding() {
        assert_eq!(
            windows_srt_port_range(&json!({"wfp":{"port_range":[40080,40089]}})).unwrap(),
            Some([40080, 40089])
        );
        assert_eq!(windows_srt_port_range(&json!({"wfp":{}})).unwrap(), None);
        for invalid in [
            json!([0, 9]),
            json!([40089, 40080]),
            json!([40080, 40180]),
            json!([40080]),
            json!([40080, 70000]),
        ] {
            assert!(windows_srt_port_range(&json!({"wfp":{"port_range":invalid}})).is_err());
        }
    }
    #[test]
    fn windows_srt_retry_range_record_is_stable_and_rejects_config_drift() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("windows-srt.json"),
            r#"{"schemaVersion":1,"portRange":[40080,40089]}"#,
        )
        .unwrap();
        assert_eq!(
            stored_windows_srt_port_range(root.path()).unwrap(),
            Some([40080, 40089])
        );
        fs::create_dir_all(root.path().join("state/local")).unwrap();
        fs::write(
            root.path().join("state/local/runtime.json"),
            r#"{"pluginSandboxWindowsProxyPortRange":[60080,60089]}"#,
        )
        .unwrap();
        assert_eq!(
            stored_windows_srt_port_range(root.path()).unwrap_err(),
            "windows_sandbox_port_range_changed"
        );
    }
    #[test]
    fn actual_sqlite_state_blocks_active_execution_and_lease() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE tradeassembly_kv(namespace TEXT,value_json TEXT); CREATE TABLE runtime_leases(expires_at_ms INTEGER);").unwrap();
        assert!(stopped_database(&path, 100).is_ok());
        db.execute(
            "INSERT INTO tradeassembly_kv VALUES('agent_deployments',?1)",
            [r#"{"deployment":{"desiredState":"active"}}"#],
        )
        .unwrap();
        assert_eq!(
            stopped_database(&path, 100).unwrap_err(),
            "rig_has_active_or_unknown_execution_state"
        );
        db.execute(
            "UPDATE tradeassembly_kv SET value_json=?1",
            [r#"{"deployment":{"desiredState":"paused"}}"#],
        )
        .unwrap();
        assert!(stopped_database(&path, 100).is_ok());
        db.execute("INSERT INTO runtime_leases VALUES(101)", [])
            .unwrap();
        assert_eq!(
            stopped_database(&path, 100).unwrap_err(),
            "rig_has_unexpired_lease"
        );
        assert!(stopped_database(&path, 102).is_ok());
        db.execute("UPDATE tradeassembly_kv SET value_json='{}'", [])
            .unwrap();
        assert_eq!(
            stopped_database(&path, 102).unwrap_err(),
            "state_record_unknown"
        );
    }
    #[test]
    fn concurrent_facade_prevents_upgrade_lock() {
        let temp = tempfile::tempdir().unwrap();
        let running = lock(temp.path(), false).unwrap();
        assert!(lock(temp.path(), true).is_err());
        drop(running);
        assert!(lock(temp.path(), true).is_ok());
    }
}
