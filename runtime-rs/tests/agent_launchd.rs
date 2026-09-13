use clap::Parser;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use tradeassembly_runtime::agent_launchd::{
    install, status, uninstall, verify, Launchctl, LaunchdProfile, LaunchdServiceState,
};
use tradeassembly_runtime::cli::{Cli, Command};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

#[derive(Default)]
struct FakeLaunchctl {
    loaded: Mutex<HashSet<String>>,
    calls: Mutex<Vec<String>>,
}

impl Launchctl for FakeLaunchctl {
    fn bootstrap(&self, domain: &str, plist_path: &Path) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("bootstrap:{domain}:{}", plist_path.display()));
        let label = plist_path.file_stem().unwrap().to_string_lossy();
        self.loaded
            .lock()
            .unwrap()
            .insert(format!("{domain}/{label}"));
        Ok(())
    }

    fn bootout(&self, service_target: &str) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("bootout:{service_target}"));
        self.loaded.lock().unwrap().remove(service_target);
        Ok(())
    }

    fn status(&self, service_target: &str) -> Result<LaunchdServiceState, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("status:{service_target}"));
        Ok(if self.loaded.lock().unwrap().contains(service_target) {
            LaunchdServiceState::Loaded
        } else {
            LaunchdServiceState::NotLoaded
        })
    }
}

fn profile(root: &Path) -> LaunchdProfile {
    LaunchdProfile {
        executable: root.join("bin/tradeassembly"),
        codex_executable: root.join("bin/codex"),
        database_path: root.join("state/tradeassembly.db"),
        runtime_config_path: root.join("state/runtime.json"),
        runner_id: "local-launchd".to_string(),
        log_directory: root.join("state"),
    }
}

#[test]
fn launchd_profile_is_deterministic_and_credential_free() {
    let directory = tempdir();
    let profile = profile(directory.path());
    let first = profile.plist().unwrap();
    let second = profile.plist().unwrap();
    assert_eq!(first, second);
    let label = profile.label().unwrap();
    assert!(label.starts_with("ai.tradeassembly.agent."));
    assert_eq!(label, profile.label().unwrap());
    assert!(first.contains(&format!("<string>{label}</string>")));
    assert!(first.contains("<string>--db</string>"));
    assert!(first.contains("<string>--config</string>"));
    assert!(first.contains(&format!(
        "<string>{}</string>",
        profile.runtime_config_path.display()
    )));
    assert!(first.contains("<string>agent</string>"));
    assert!(first.contains("<string>run</string>"));
    assert!(first.contains("<key>TRADEASSEMBLY_CODEX_BIN</key>"));
    assert!(first.contains(&format!(
        "<string>{}</string>",
        profile.codex_executable.display()
    )));
    assert!(first.contains("<key>KeepAlive</key>"));
    assert!(!first.contains("token"));
    let mut invalid = profile;
    invalid.database_path = "relative.db".into();
    assert_eq!(
        invalid.plist().unwrap_err(),
        "launchd_database_path_must_be_absolute"
    );
}

#[test]
fn launchd_config_path_is_escaped_and_must_be_absolute() {
    let directory = tempdir();
    let mut profile = profile(directory.path());
    profile.runtime_config_path = directory.path().join("space & more/runtime.json");
    let plist = profile.plist().unwrap();
    assert!(plist.contains("space &amp; more/runtime.json</string>"));
    profile.runtime_config_path = "relative.json".into();
    assert_eq!(
        profile.plist().unwrap_err(),
        "launchd_runtime_config_path_must_be_absolute"
    );
}

#[test]
fn fake_launchctl_proves_user_scoped_install_verify_status_and_uninstall() {
    let directory = tempdir();
    let launch_agents = directory.path().join("LaunchAgents");
    let fake = FakeLaunchctl::default();
    let profile = profile(directory.path());

    let installed = install(&fake, &profile, &launch_agents, 501).unwrap();
    assert!(installed.loaded);
    assert_eq!(installed.profile_matches, Some(true));
    assert!(installed
        .plist_path
        .ends_with(&format!("{}.plist", profile.label().unwrap())));
    assert!(Path::new(&installed.plist_path).is_file());
    assert!(fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.starts_with("bootstrap:gui/501:")));

    let verified = verify(&fake, &profile, &launch_agents, 501).unwrap();
    assert!(verified.loaded);
    assert_eq!(verified.profile_matches, Some(true));
    std::fs::write(&verified.plist_path, "drift").unwrap();
    assert_eq!(
        verify(&fake, &profile, &launch_agents, 501)
            .unwrap()
            .profile_matches,
        Some(false)
    );
    assert!(status(&fake, &profile, &launch_agents, 501).unwrap().loaded);

    assert_eq!(
        uninstall(&fake, &profile, &launch_agents, 501).unwrap_err(),
        "launchd_existing_profile_conflict"
    );
    std::fs::write(&verified.plist_path, profile.plist().unwrap()).unwrap();
    let removed = uninstall(&fake, &profile, &launch_agents, 501).unwrap();
    assert!(!removed.loaded);
    assert!(!Path::new(&removed.plist_path).exists());
    assert!(fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call == &format!("bootout:gui/501/{}", profile.label().unwrap())));
    assert!(!status(&fake, &profile, &launch_agents, 501).unwrap().loaded);
}

#[test]
fn agent_launchd_commands_parse_without_mutating_a_service() {
    for arguments in [
        vec![
            "agent",
            "install",
            "--tradeassembly-bin",
            "/tmp/tradeassembly",
        ],
        vec![
            "agent",
            "install",
            "--tradeassembly-bin",
            "/tmp/tradeassembly",
            "--codex-bin",
            "/tmp/codex",
        ],
        vec![
            "agent",
            "verify",
            "--launch-agents-dir",
            "/tmp/LaunchAgents",
        ],
        vec!["agent", "status"],
        vec!["agent", "uninstall"],
    ] {
        let mut argv = vec!["tradeassembly"];
        argv.extend(arguments);
        let parsed = Cli::try_parse_from(argv).expect("agent lifecycle command parses");
        assert!(matches!(parsed.command, Command::Agent { .. }));
    }
}

#[test]
fn install_does_not_replace_another_installation_or_unmanaged_service() {
    let root = tempdir();
    let directory = root.path().join("LaunchAgents");
    let fake = FakeLaunchctl::default();
    let original = profile(root.path());
    let installed = install(&fake, &original, &directory, 501).unwrap();
    let before = std::fs::read(&installed.plist_path).unwrap();
    fake.calls.lock().unwrap().clear();
    let mut other = original;
    other.runtime_config_path = root.path().join("other/runtime.json");
    let other_installed = install(&fake, &other, &directory, 501).unwrap();
    assert_ne!(installed.plist_path, other_installed.plist_path);
    assert_eq!(std::fs::read(&installed.plist_path).unwrap(), before);
    fake.calls.lock().unwrap().clear();
    assert_eq!(
        install(&fake, &other, &root.path().join("another-directory"), 501).unwrap_err(),
        "launchd_existing_service_unmanaged"
    );
    assert!(!fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.starts_with("bootout:") || call.starts_with("bootstrap:")));
}

#[test]
fn profiles_are_isolated_and_uninstall_preserves_logs() {
    let root = tempdir();
    let directory = root.path().join("LaunchAgents");
    let fake = FakeLaunchctl::default();
    let first = profile(root.path());
    let mut second = first.clone();
    second.runtime_config_path = root.path().join("other/runtime.json");

    assert_ne!(first.label().unwrap(), second.label().unwrap());
    let first_installed = install(&fake, &first, &directory, 501).unwrap();
    let log = first.log_directory.join("agent-runner.out.log");
    std::fs::create_dir_all(&first.log_directory).unwrap();
    std::fs::write(&log, "keep").unwrap();
    let second_installed = install(&fake, &second, &directory, 501).unwrap();
    assert_ne!(first_installed.plist_path, second_installed.plist_path);
    let removed = uninstall(&fake, &first, &directory, 501).unwrap();
    assert!(!Path::new(&removed.plist_path).exists());
    assert_eq!(std::fs::read_to_string(log).unwrap(), "keep");
    assert!(Path::new(&second_installed.plist_path).exists());
    assert!(status(&fake, &second, &directory, 501).unwrap().loaded);
}

#[test]
fn symlinked_plist_and_launch_agents_directory_are_rejected() {
    let root = tempdir();
    let fake = FakeLaunchctl::default();
    let profile = profile(root.path());
    let directory = root.path().join("LaunchAgents");
    std::fs::create_dir_all(&directory).unwrap();
    let target = root.path().join("elsewhere.plist");
    std::fs::write(&target, "foreign").unwrap();
    std::os::unix::fs::symlink(
        &target,
        directory.join(format!("{}.plist", profile.label().unwrap())),
    )
    .unwrap();
    assert_eq!(
        install(&fake, &profile, &directory, 501).unwrap_err(),
        "launchd_existing_profile_invalid"
    );
    let linked_dir = root.path().join("linked");
    std::os::unix::fs::symlink(&directory, &linked_dir).unwrap();
    assert_eq!(
        status(&fake, &profile, &linked_dir, 501).unwrap_err(),
        "launchd_parent_path_symlink"
    );
}

#[test]
fn uninstall_is_idempotent_when_profile_absent_and_service_unloaded() {
    let root = tempdir();
    let fake = FakeLaunchctl::default();
    let profile = profile(root.path());
    let directory = root.path().join("LaunchAgents");
    let first = uninstall(&fake, &profile, &directory, 501).unwrap();
    let second = uninstall(&fake, &profile, &directory, 501).unwrap();
    assert_eq!(first, second);
    assert!(!first.loaded);
}
