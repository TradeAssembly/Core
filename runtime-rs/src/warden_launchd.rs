// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! User-scoped macOS launchd lifecycle for the local Warden sidecar.
//!
//! This module owns only the sidecar process. It never manages the agent
//! runner, broker credentials, positions, or trading activation.

use crate::agent_launchd::{Launchctl, LaunchdLifecycleStatus, LaunchdServiceState};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

const LABEL_PREFIX: &str = "ai.tradeassembly.policy.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyLaunchdProfile {
    pub executable: PathBuf,
    pub state_dir: PathBuf,
}

impl PolicyLaunchdProfile {
    pub fn label(&self) -> Result<String, String> {
        let state = canonical_state_dir(&self.state_dir)?;
        let digest = Sha256::digest(state.as_os_str().to_string_lossy().as_bytes());
        let suffix = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(format!("{LABEL_PREFIX}{suffix}"))
    }

    pub fn validate(&self) -> Result<PathBuf, String> {
        if !self.executable.is_absolute() {
            return Err("policy_launchd_executable_must_be_absolute".into());
        }
        let state = canonical_state_dir(&self.state_dir)?;
        for name in ["policy.stdout.log", "policy.stderr.log"] {
            let path = state.join(name);
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err("policy_launchd_log_path_invalid".into());
                }
            }
        }
        Ok(state)
    }

    pub fn program_arguments(&self) -> Result<Vec<String>, String> {
        let state = self.validate()?;
        Ok(vec![
            self.executable.display().to_string(),
            "policy".into(),
            "serve".into(),
            "--state-dir".into(),
            state.display().to_string(),
        ])
    }

    pub fn plist(&self) -> Result<String, String> {
        let state = self.validate()?;
        let label = self.label()?;
        let args = self
            .program_arguments()?
            .into_iter()
            .map(|value| format!("    <string>{}</string>", xml_escape(&value)))
            .collect::<Vec<_>>()
            .join("\n");
        let stdout = state.join("policy.stdout.log");
        let stderr = state.join("policy.stderr.log");
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{label}</string>\n  <key>ProgramArguments</key>\n  <array>\n{args}\n  </array>\n  <key>WorkingDirectory</key>\n  <string>{state}</string>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n  <key>StandardOutPath</key>\n  <string>{stdout}</string>\n  <key>StandardErrorPath</key>\n  <string>{stderr}</string>\n</dict>\n</plist>\n",
            label = xml_escape(&label),
            state = xml_escape(&state.display().to_string()),
            stdout = xml_escape(&stdout.display().to_string()),
            stderr = xml_escape(&stderr.display().to_string()),
        );
        Ok(plist)
    }
}

pub fn install(
    launchctl: &dyn Launchctl,
    profile: &PolicyLaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let expected = profile.plist()?;
    let label = profile.label()?;
    let path = plist_path(launch_agents_dir, &label);
    let existing = read_owned_profile(&path)?;
    let target = service_target(uid, &label);
    let loaded = launchctl.status(&target)? == LaunchdServiceState::Loaded;
    if let Some(actual) = existing {
        if actual != expected {
            return Err("policy_launchd_existing_profile_conflict".into());
        }
        if loaded {
            return status_value(&path, &label, true, true);
        }
    } else if loaded {
        return Err("policy_launchd_existing_service_unmanaged".into());
    }

    fs::create_dir_all(launch_agents_dir).map_err(|_| "policy_launchd_directory_create_failed")?;
    if read_owned_profile(&path)?.is_none() {
        match create_private_file(&path, expected.as_bytes()) {
            Ok(()) => {}
            Err(error) if error == "policy_launchd_existing_profile_conflict" => {
                if read_owned_profile(&path)?.as_deref() != Some(expected.as_str()) {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
    if read_owned_profile(&path)?.as_deref() != Some(expected.as_str()) {
        return Err("policy_launchd_existing_profile_conflict".into());
    }
    launchctl.bootstrap(&domain(uid), &path)?;
    status_value(&path, &label, true, true)
}

pub fn verify(
    launchctl: &dyn Launchctl,
    profile: &PolicyLaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let expected = profile.plist()?;
    let label = profile.label()?;
    let path = plist_path(launch_agents_dir, &label);
    let matches = read_owned_profile(&path)?.is_some_and(|actual| actual == expected);
    let loaded = launchctl.status(&service_target(uid, &label))? == LaunchdServiceState::Loaded;
    status_value(&path, &label, loaded, matches)
}

pub fn status(
    launchctl: &dyn Launchctl,
    profile: &PolicyLaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let expected = profile.plist()?;
    let label = profile.label()?;
    let path = plist_path(launch_agents_dir, &label);
    let matches = read_owned_profile(&path)?.is_some_and(|actual| actual == expected);
    let loaded = launchctl.status(&service_target(uid, &label))? == LaunchdServiceState::Loaded;
    status_value(&path, &label, loaded, matches)
}

pub fn uninstall(
    launchctl: &dyn Launchctl,
    profile: &PolicyLaunchdProfile,
    launch_agents_dir: &Path,
    uid: u32,
) -> Result<LaunchdLifecycleStatus, String> {
    validate_launch_agents_dir(launch_agents_dir)?;
    let expected = profile.plist()?;
    let label = profile.label()?;
    let path = plist_path(launch_agents_dir, &label);
    let existing = read_owned_profile(&path)?;
    let target = service_target(uid, &label);
    let loaded = launchctl.status(&target)? == LaunchdServiceState::Loaded;
    match existing {
        Some(actual) if actual != expected => {
            return Err("policy_launchd_existing_profile_conflict".into())
        }
        Some(_) => {
            if loaded {
                launchctl.bootout(&target)?;
            }
            fs::remove_file(&path).map_err(|_| "policy_launchd_plist_remove_failed")?;
        }
        None if loaded => return Err("policy_launchd_existing_service_unmanaged".into()),
        None => {}
    }
    status_value(&path, &label, false, false)
}

fn canonical_state_dir(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("policy_launchd_state_dir_must_be_absolute".into());
    }
    reject_symlink_components(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "policy_launchd_state_dir_unavailable")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("policy_launchd_state_dir_invalid".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("policy_launchd_state_dir_insecure".into());
        }
    }
    path.canonicalize()
        .map_err(|_| "policy_launchd_state_dir_unavailable".into())
}

fn validate_launch_agents_dir(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("policy_launchd_launch_agents_dir_must_be_absolute".into());
    }
    reject_symlink_components(path)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("policy_launchd_launch_agents_dir_invalid".into());
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
                return Err("policy_launchd_state_dir_symlink".into());
            }
        }
    }
    Ok(())
}

fn create_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| match error.kind() {
        ErrorKind::AlreadyExists => "policy_launchd_existing_profile_conflict",
        _ => "policy_launchd_plist_write_failed",
    })?;
    use std::io::Write;
    file.write_all(contents)
        .map_err(|_| "policy_launchd_plist_write_failed".into())
}

fn read_owned_profile(path: &Path) -> Result<Option<String>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("policy_launchd_existing_profile_invalid".into())
        }
        Ok(_) => fs::read_to_string(path)
            .map(Some)
            .map_err(|_| "policy_launchd_existing_profile_unreadable".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err("policy_launchd_existing_profile_unreadable".into()),
    }
}

fn status_value(
    path: &Path,
    label: &str,
    loaded: bool,
    profile_matches: bool,
) -> Result<LaunchdLifecycleStatus, String> {
    Ok(LaunchdLifecycleStatus {
        label: label.to_string(),
        plist_path: path.display().to_string(),
        loaded,
        profile_matches: Some(profile_matches),
    })
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

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
