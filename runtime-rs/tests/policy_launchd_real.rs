//! Explicit macOS integration: only a temporary installation's policy process.
#![cfg(target_os = "macos")]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

struct Rig {
    root: Option<tempfile::TempDir>,
    binary: PathBuf,
    state: PathBuf,
    agents: PathBuf,
}

impl Rig {
    fn policy(&self, action: &str) -> Output {
        Command::new(&self.binary)
            .env_clear()
            .args(["policy", "launchd", action, "--state-dir"])
            .arg(&self.state)
            .arg("--launch-agents-dir")
            .arg(&self.agents)
            .output()
            .unwrap()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let stopped = self.policy("uninstall");
        if !stopped.status.success() {
            // Never erase a fixture while its owned service may still be live.
            if let Some(root) = self.root.take() {
                eprintln!(
                    "policy cleanup failed; fixture preserved at {}",
                    root.keep().display()
                );
            }
        }
    }
}

fn selected(name: &str) -> PathBuf {
    Path::new(&std::env::var_os(name).expect("select explicit real binary"))
        .canonicalize()
        .unwrap()
}

fn process_id(target: &str) -> Option<u32> {
    let output = Command::new("/bin/launchctl")
        .args(["print", target])
        .output()
        .unwrap();
    assert!(output.status.success(), "owned service must remain loaded");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid = ")?.parse().ok())
}

fn wait_running(target: &str, previous: Option<u32>, port: u16) -> u32 {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(pid) = process_id(target).filter(|pid| Some(*pid) != previous) {
            if let Ok(response) = client.get(format!("http://127.0.0.1:{port}/health")).send() {
                if response.status().is_success() {
                    let health: Value = response.json().unwrap();
                    assert_eq!(health["service"], "warden-sidecar");
                    return pid;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "owned policy restart/health timed out"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires explicit real binaries and an available macOS GUI launchd domain"]
fn owned_policy_survives_process_loss_and_uninstalls_without_losing_state() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let binary = selected("F2_TEST_TRADEASSEMBLY_BINARY");
    let warden = selected("F2_TEST_WARDEN_BINARY");
    let state = base.join("installation");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let setup = Command::new(&binary)
        .env_clear()
        .args(["setup", "--state-dir"])
        .arg(&state)
        .arg("--warden-binary")
        .arg(warden)
        .arg("--warden-port")
        .arg(port.to_string())
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stdout)
    );
    let before = std::fs::read(state.join("warden.token")).unwrap();
    let rig = Rig {
        root: Some(root),
        binary,
        state,
        agents: base.join("LaunchAgents"),
    };
    drop(listener);
    let install = rig.policy("install");
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stdout)
    );
    let body: Value = serde_json::from_slice(&install.stdout).unwrap();
    let label = body["supervision"]["label"].as_str().unwrap();
    assert!(label.starts_with("ai.tradeassembly.policy."));
    let uid = tradeassembly_runtime::agent_launchd::current_uid().unwrap();
    let target = format!("gui/{uid}/{label}");
    let first = wait_running(&target, None, port);
    assert!(rig.policy("install").status.success());
    assert_eq!(
        process_id(&target),
        Some(first),
        "repeat install must not restart policy"
    );
    assert!(Command::new("/bin/launchctl")
        .args(["kill", "SIGKILL", &target])
        .status()
        .unwrap()
        .success());
    let restarted = wait_running(&target, Some(first), port);
    assert_ne!(first, restarted);
    assert_eq!(
        std::fs::read(rig.state.join("warden.token")).unwrap(),
        before
    );
    assert!(rig.policy("verify").status.success());
    assert!(rig.policy("uninstall").status.success());
    let status = rig.policy("status");
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["supervision"]["loaded"], false);
    assert!(
        !rig.policy("verify").status.success(),
        "verification must fail after removal"
    );
    assert!(rig.state.join("runtime.json").is_file());
    assert!(rig.state.join("warden.sqlite").is_file());
    assert_eq!(
        std::fs::read(rig.state.join("warden.token")).unwrap(),
        before
    );
}
