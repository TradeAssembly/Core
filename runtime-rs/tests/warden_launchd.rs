use std::path::Path;
use std::sync::Mutex;
use tradeassembly_runtime::agent_launchd::{Launchctl, LaunchdServiceState};
use tradeassembly_runtime::warden_launchd::{
    install, status, uninstall, verify, PolicyLaunchdProfile,
};

#[derive(Default)]
struct FakeLaunchctl {
    loaded: Mutex<bool>,
    calls: Mutex<Vec<String>>,
}

impl Launchctl for FakeLaunchctl {
    fn bootstrap(&self, domain: &str, path: &Path) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("bootstrap:{domain}:{}", path.display()));
        *self.loaded.lock().unwrap() = true;
        Ok(())
    }

    fn bootout(&self, target: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("bootout:{target}"));
        *self.loaded.lock().unwrap() = false;
        Ok(())
    }

    fn status(&self, target: &str) -> Result<LaunchdServiceState, String> {
        self.calls.lock().unwrap().push(format!("status:{target}"));
        Ok(if *self.loaded.lock().unwrap() {
            LaunchdServiceState::Loaded
        } else {
            LaunchdServiceState::NotLoaded
        })
    }
}

fn profile(root: &Path) -> PolicyLaunchdProfile {
    let root = root.canonicalize().unwrap();
    PolicyLaunchdProfile {
        executable: root.join("bin/tradeassembly"),
        state_dir: root.join("state & policy"),
    }
}

fn private_dir(path: &Path) {
    std::fs::create_dir(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn profile_is_stable_escaped_and_distinct_from_agent() {
    let root = tempfile::tempdir().unwrap();
    private_dir(&root.path().join("state & policy"));
    let profile = profile(root.path());
    let label = profile.label().unwrap();
    assert!(label.starts_with("ai.tradeassembly.policy."));
    assert_eq!(label, profile.label().unwrap());
    assert!(!label.contains("agent"));
    let plist = profile.plist().unwrap();
    assert_eq!(plist, profile.plist().unwrap());
    assert!(plist.contains("policy"));
    assert!(plist.contains("state &amp; policy"));
    assert!(plist.contains("policy.stdout.log"));
    assert!(plist.contains("policy.stderr.log"));
    assert!(plist.contains("<true/>"));
    assert!(!plist.contains("token"));
}

#[test]
fn lifecycle_is_idempotent_and_preserves_state_and_logs() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    private_dir(&state);
    let launch_agents = root.path().canonicalize().unwrap().join("LaunchAgents");
    let canonical_root = root.path().canonicalize().unwrap();
    let canonical_state = state.canonicalize().unwrap();
    let profile = PolicyLaunchdProfile {
        executable: canonical_root.join("bin/tradeassembly"),
        state_dir: canonical_state,
    };
    let fake = FakeLaunchctl::default();
    let first = install(&fake, &profile, &launch_agents, 501).unwrap();
    assert!(first.loaded);
    let call_count = fake.calls.lock().unwrap().len();
    let second = install(&fake, &profile, &launch_agents, 501).unwrap();
    assert!(second.loaded);
    assert_eq!(fake.calls.lock().unwrap().len(), call_count + 1);
    assert!(!fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.starts_with("bootout:")));
    assert_eq!(
        verify(&fake, &profile, &launch_agents, 501)
            .unwrap()
            .profile_matches,
        Some(true)
    );
    assert!(status(&fake, &profile, &launch_agents, 501).unwrap().loaded);
    std::fs::write(state.join("policy.stdout.log"), b"keep").unwrap();
    let removed = uninstall(&fake, &profile, &launch_agents, 501).unwrap();
    assert!(!removed.loaded);
    assert!(state.join("policy.stdout.log").is_file());
    assert!(!Path::new(&removed.plist_path).exists());
}

#[test]
fn conflicts_symlinks_and_unmanaged_services_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state & policy");
    private_dir(&state);
    let launch_agents = root.path().canonicalize().unwrap().join("LaunchAgents");
    let profile = profile(root.path());
    let fake = FakeLaunchctl::default();
    std::fs::create_dir_all(&launch_agents).unwrap();
    let label = profile.label().unwrap();
    let path = launch_agents.join(format!("{label}.plist"));
    std::fs::write(&path, "different").unwrap();
    assert_eq!(
        install(&fake, &profile, &launch_agents, 501).unwrap_err(),
        "policy_launchd_existing_profile_conflict"
    );
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(root.path().join("other"), &path).unwrap();
    assert_eq!(
        status(&fake, &profile, &launch_agents, 501).unwrap_err(),
        "policy_launchd_existing_profile_invalid"
    );
    std::fs::remove_file(&path).unwrap();
    *fake.loaded.lock().unwrap() = true;
    assert_eq!(
        uninstall(&fake, &profile, &launch_agents, 501).unwrap_err(),
        "policy_launchd_existing_service_unmanaged"
    );
    assert!(!fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.starts_with("bootout:")));
}

#[test]
fn log_symlink_is_rejected_before_lifecycle_changes() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state & policy");
    private_dir(&state);
    std::os::unix::fs::symlink(root.path().join("other"), state.join("policy.stdout.log")).unwrap();
    let profile = profile(root.path());
    assert_eq!(
        profile.plist().unwrap_err(),
        "policy_launchd_log_path_invalid"
    );
}

#[test]
fn profile_change_during_status_check_cannot_be_bootstrapped() {
    struct ChangedProfile(std::path::PathBuf);
    impl Launchctl for ChangedProfile {
        fn bootstrap(&self, _: &str, _: &Path) -> Result<(), String> {
            panic!("unowned bootstrap")
        }
        fn bootout(&self, _: &str) -> Result<(), String> {
            panic!("unowned bootout")
        }
        fn status(&self, _: &str) -> Result<LaunchdServiceState, String> {
            std::fs::write(&self.0, "changed profile").unwrap();
            Ok(LaunchdServiceState::NotLoaded)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    private_dir(&base.join("state & policy"));
    let profile = profile(&base);
    let agents = base.join("LaunchAgents");
    private_dir(&agents);
    let path = agents.join(format!("{}.plist", profile.label().unwrap()));
    std::fs::write(&path, profile.plist().unwrap()).unwrap();
    assert_eq!(
        install(&ChangedProfile(path.clone()), &profile, &agents, 501).unwrap_err(),
        "policy_launchd_existing_profile_conflict"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "changed profile");
}
