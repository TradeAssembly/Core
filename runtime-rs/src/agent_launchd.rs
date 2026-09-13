// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! User-scoped macOS launchd lifecycle for the local agent runner.
//!
//! This module deliberately owns only process supervision. It does not store
//! credentials, configure brokers, or invoke agent/broker work itself.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL_PREFIX: &str = "ai.tradeassembly.agent.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchdProfile {
    pub executable: PathBuf,
    /// Absolute local Codex executable captured at install time. `launchd`
    /// does not inherit an interactive shell PATH, so the background runner
    /// must not depend on a best-effort command lookup.
    pub codex_executable: PathBuf,
    pub database_path: PathBuf,
    pub runtime_config_path: PathBuf,
    pub runner_id: String,
    pub log_directory: PathBuf,
}

impl LaunchdProfile {
    pub fn label(&self) -> Result<String, String> {
        self.validate()?;
        let digest = Sha256::digest(
            self.runtime_config_path
                .as_os_str()
                .to_string_lossy()
                .as_bytes(),
        );
        let suffix = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(format!("{LABEL_PREFIX}{suffix}"))
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, path) in [
            ("tradeassembly_binary", &self.executable),
            ("codex_binary", &self.codex_executable),
            ("database_path", &self.database_path),
            ("runtime_config_path", &self.runtime_config_path),
            ("log_directory", &self.log_directory),
        ] {
            if !path.is_absolute() {
                return Err(format!("launchd_{name}_must_be_absolute"));
            }
        }
        if self.runner_id.trim().is_empty() || contains_control(&self.runner_id) {
            return Err("launchd_runner_id_invalid".to_string());
        }
        Ok(())
    }

    pub fn program_arguments(&self) -> Result<Vec<String>, String> {
        self.validate()?;
        Ok(vec![
            self.executable.display().to_string(),
            "--config".to_string(),
            self.runtime_config_path.display().to_string(),
            "--db".to_string(),
            self.database_path.display().to_string(),
            "agent".to_string(),
            "run".to_string(),
            "--runner-id".to_string(),
            self.runner_id.clone(),
        ])
    }

    pub fn plist(&self) -> Result<String, String> {
        let label = self.label()?;
        let args = self.program_arguments()?;
        let program_arguments = args
            .iter()
            .map(|value| format!("    <string>{}</string>", xml_escape(value)))
            .collect::<Vec<_>>()
            .join("\n");
        let stdout = self.log_directory.join("agent-runner.out.log");
        let stderr = self.log_directory.join("agent-runner.err.log");
        Ok(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{label}</string>\n  <key>ProgramArguments</key>\n  <array>\n{program_arguments}\n  </array>\n  <key>EnvironmentVariables</key>\n  <dict>\n    <key>TRADEASSEMBLY_CODEX_BIN</key>\n    <string>{codex_executable}</string>\n  </dict>\n  <key>WorkingDirectory</key>\n  <string>{working_directory}</string>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n  <key>ThrottleInterval</key>\n  <integer>10</integer>\n  <key>StandardOutPath</key>\n  <string>{stdout}</string>\n  <key>StandardErrorPath</key>\n  <string>{stderr}</string>\n</dict>\n</plist>\n",
            label = xml_escape(&label),
            codex_executable = xml_escape(&self.codex_executable.display().to_string()),
            working_directory = xml_escape(&self.log_directory.display().to_string()),
            stdout = xml_escape(&stdout.display().to_string()),
            stderr = xml_escape(&stderr.display().to_string()),
        ))
    }
}

pub trait Launchctl {
    fn bootstrap(&self, domain: &str, plist_path: &Path) -> Result<(), String>;
    fn bootout(&self, service_target: &str) -> Result<(), String>;
    fn status(&self, service_target: &str) -> Result<LaunchdServiceState, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchdServiceState {
    Loaded,
    NotLoaded,
}

pub struct SystemLaunchctl;

impl Launchctl for SystemLaunchctl {
    fn bootstrap(&self, domain: &str, plist_path: &Path) -> Result<(), String> {
        run_launchctl(&["bootstrap", domain, &plist_path.display().to_string()])
    }

    fn bootout(&self, service_target: &str) -> Result<(), String> {
        run_launchctl(&["bootout", service_target])
    }

    fn status(&self, service_target: &str) -> Result<LaunchdServiceState, String> {
        if std::env::consts::OS != "macos" {
            return Err("launchd_unsupported_platform".to_string());
        }
        let output = Command::new("/bin/launchctl")
            .args(["print", service_target])
            .output()
            .map_err(|_| "launchctl_unavailable".to_string())?;
        if output.status.success() {
            return Ok(LaunchdServiceState::Loaded);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("could not find service") || stderr.contains("no service") {
            Ok(LaunchdServiceState::NotLoaded)
        } else {
            Err("launchctl_status_failed".to_string())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdLifecycleStatus {
    pub label: String,
    pub plist_path: String,
    pub loaded: bool,
    pub profile_matches: Option<bool>,
}

pub fn install(
    launchctl: &dyn Launchctl,
    profile: &LaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let plist = profile.plist()?;
    let label = profile.label()?;
    let plist_path = plist_path(launch_agents_dir, &label);
    let domain = domain(uid);
    let target = service_target(uid, &label);
    let previous = read_owned_profile(&plist_path)?;
    if previous.as_ref().is_some_and(|previous| previous != &plist) {
        return Err("launchd_existing_profile_conflict".into());
    }
    let loaded = launchctl.status(&target)? == LaunchdServiceState::Loaded;
    if loaded && previous.is_none() {
        return Err("launchd_existing_service_unmanaged".into());
    }
    if loaded {
        return Ok(LaunchdLifecycleStatus {
            label,
            plist_path: plist_path.display().to_string(),
            loaded: true,
            profile_matches: Some(true),
        });
    }
    fs::create_dir_all(launch_agents_dir).map_err(|_| "launchd_directory_create_failed")?;
    fs::create_dir_all(&profile.log_directory)
        .map_err(|_| "launchd_log_directory_create_failed")?;
    validate_log_paths(&profile.log_directory)?;
    if read_owned_profile(&plist_path)?.is_none() {
        create_profile(&plist_path, plist.as_bytes())?;
    }
    if read_owned_profile(&plist_path)?.as_deref() != Some(plist.as_str()) {
        return Err("launchd_existing_profile_conflict".into());
    }
    launchctl.bootstrap(&domain, &plist_path)?;
    Ok(LaunchdLifecycleStatus {
        label,
        plist_path: plist_path.display().to_string(),
        loaded: true,
        profile_matches: Some(true),
    })
}

pub fn verify(
    launchctl: &dyn Launchctl,
    profile: &LaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let expected = profile.plist()?;
    let label = profile.label()?;
    let plist_path = plist_path(launch_agents_dir, &label);
    let profile_matches = read_owned_profile(&plist_path)?.is_some_and(|actual| actual == expected);
    let loaded = launchctl.status(&service_target(uid, &label))? == LaunchdServiceState::Loaded;
    Ok(LaunchdLifecycleStatus {
        label,
        plist_path: plist_path.display().to_string(),
        loaded,
        profile_matches: Some(profile_matches),
    })
}

pub fn status(
    launchctl: &dyn Launchctl,
    profile: &LaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let label = profile.label()?;
    let plist_path = plist_path(launch_agents_dir, &label);
    let expected = profile.plist()?;
    let profile_matches = read_owned_profile(&plist_path)?.is_some_and(|actual| actual == expected);
    let loaded = launchctl.status(&service_target(uid, &label))? == LaunchdServiceState::Loaded;
    Ok(LaunchdLifecycleStatus {
        label,
        plist_path: plist_path.display().to_string(),
        loaded,
        profile_matches: Some(profile_matches),
    })
}

pub fn uninstall(
    launchctl: &dyn Launchctl,
    profile: &LaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let label = profile.label()?;
    let expected = profile.plist()?;
    let plist_path = plist_path(launch_agents_dir, &label);
    let target = service_target(uid, &label);
    let loaded = launchctl.status(&target)? == LaunchdServiceState::Loaded;
    match read_owned_profile(&plist_path)? {
        Some(actual) if actual != expected => {
            return Err("launchd_existing_profile_conflict".into())
        }
        Some(_) => {
            if loaded {
                launchctl.bootout(&target)?;
            }
            fs::remove_file(&plist_path).map_err(|_| "launchd_plist_remove_failed")?;
        }
        None if loaded => return Err("launchd_existing_service_unmanaged".into()),
        None => {}
    }
    Ok(LaunchdLifecycleStatus {
        label,
        plist_path: plist_path.display().to_string(),
        loaded: false,
        profile_matches: None,
    })
}

pub fn default_launch_agents_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "launchd_home_unavailable".to_string())?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents"))
}

pub fn current_uid() -> Result<u32, String> {
    if let Ok(uid) = std::env::var("UID").unwrap_or_default().parse::<u32>() {
        return Ok(uid);
    }
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(|_| "launchd_uid_unavailable".to_string())?;
    if !output.status.success() {
        return Err("launchd_uid_unavailable".to_string());
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|_| "launchd_uid_unavailable".to_string())
}

fn plist_path(launch_agents_dir: &Path, label: &str) -> PathBuf {
    launch_agents_dir.join(format!("{label}.plist"))
}

fn domain(uid: u32) -> String {
    format!("gui/{uid}")
}

fn service_target(uid: u32, label: &str) -> String {
    format!("{}/{}", domain(uid), label)
}

fn validate_launch_agents_dir(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("launchd_launch_agents_dir_must_be_absolute".into());
    }
    reject_symlink_components(path)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("launchd_launch_agents_dir_invalid".into());
        }
    }
    Ok(())
}

fn validate_log_paths(directory: &Path) -> Result<(), String> {
    reject_symlink_components(directory)?;
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("launchd_log_directory_invalid".into())
        }
        Ok(_) | Err(_) => {}
    }
    for name in ["agent-runner.out.log", "agent-runner.err.log"] {
        let path = directory.join(name);
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err("launchd_log_path_invalid".into());
            }
        }
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            if metadata.file_type().is_symlink() {
                return Err("launchd_parent_path_symlink".into());
            }
        }
    }
    Ok(())
}

fn read_owned_profile(path: &Path) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("launchd_existing_profile_invalid".into())
        }
        Ok(_) => fs::read_to_string(path)
            .map(Some)
            .map_err(|_| "launchd_existing_profile_unreadable".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("launchd_existing_profile_unreadable".into()),
    }
}

fn create_profile(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::AlreadyExists => "launchd_existing_profile_conflict",
        _ => "launchd_plist_write_failed",
    })?;
    file.write_all(contents)
        .map_err(|_| "launchd_plist_write_failed".into())
}

fn run_launchctl(args: &[&str]) -> Result<(), String> {
    if std::env::consts::OS != "macos" {
        return Err("launchd_unsupported_platform".to_string());
    }
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|_| "launchctl_unavailable".to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err("launchctl_command_failed".to_string())
    }
}

fn contains_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
