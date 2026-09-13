//! Nontrading process-supervision acceptance using only an isolated owned label.
#![cfg(target_os = "macos")]
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

struct Rig {
    root: Option<tempfile::TempDir>,
    binary: PathBuf,
    codex: PathBuf,
    state: PathBuf,
    agents: PathBuf,
}
impl Rig {
    fn agent(&self, action: &str) -> Output {
        Command::new(&self.binary)
            .env_clear()
            .arg("--config")
            .arg(self.state.join("runtime.json"))
            .args(["agent", action, "--codex-bin"])
            .arg(&self.codex)
            .arg("--launch-agents-dir")
            .arg(&self.agents)
            .output()
            .unwrap()
    }
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
        let agent = self.agent("uninstall");
        let policy = self.policy("uninstall");
        if !agent.status.success() || !policy.status.success() {
            if let Some(root) = self.root.take() {
                eprintln!(
                    "cleanup incomplete; preserve owned rig {}",
                    root.keep().display()
                );
            }
        }
    }
}
fn selected(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).expect("explicit binary required"))
        .canonicalize()
        .unwrap()
}
fn pid(target: &str) -> Option<u32> {
    let output = Command::new("/bin/launchctl")
        .args(["print", target])
        .output()
        .unwrap();
    assert!(output.status.success(), "owned label must be loaded");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid = ")?.parse().ok())
}
fn wait_stable(target: &str, old: Option<u32>) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(candidate) = pid(target).filter(|p| Some(*p) != old) {
            std::thread::sleep(Duration::from_secs(1));
            if pid(target) == Some(candidate) {
                return candidate;
            }
        }
        assert!(
            Instant::now() < deadline,
            "owned runner did not become stable"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
#[test]
#[ignore = "requires explicit real binaries and macOS GUI launchd; no model or broker calls"]
fn isolated_agent_restarts_and_uninstalls_without_losing_state() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let state = base.join("installation");
    let binary = selected("F2_TEST_TRADEASSEMBLY_BINARY");
    let codex = selected("F2_TEST_CODEX_BINARY");
    let warden = selected("F2_TEST_WARDEN_BINARY");
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
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
    assert!(setup.status.success());
    drop(reservation);
    let rig = Rig {
        root: Some(root),
        binary,
        codex,
        state,
        agents: base.join("LaunchAgents"),
    };
    assert!(rig.policy("install").status.success());
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .is_ok_and(|r| r.status().is_success())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "owned policy did not become healthy"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let config_before = std::fs::read(rig.state.join("runtime.json")).unwrap();
    let installed = rig.agent("install");
    assert!(installed.status.success(), "isolated agent install failed");
    let result: serde_json::Value = serde_json::from_slice(&installed.stdout).unwrap();
    let label = result["data"]["launchd"]["label"].as_str().unwrap();
    assert!(label.starts_with("ai.tradeassembly.agent."));
    let target = format!(
        "gui/{}/{}",
        tradeassembly_runtime::agent_launchd::current_uid().unwrap(),
        label
    );
    let first = wait_stable(&target, None);
    assert!(rig.agent("verify").status.success());
    assert!(rig.agent("install").status.success());
    assert_eq!(
        pid(&target),
        Some(first),
        "repeat install must not restart the agent"
    );
    assert!(Command::new("/bin/launchctl")
        .args(["kill", "SIGKILL", &target])
        .status()
        .unwrap()
        .success());
    let replacement = wait_stable(&target, Some(first));
    assert_ne!(first, replacement);
    for name in ["agent-runner.out.log", "agent-runner.err.log"] {
        assert!(
            std::fs::read(rig.state.join(name)).unwrap().is_empty(),
            "empty long-running supervisor should not emit a startup failure"
        );
    }
    assert!(rig.agent("verify").status.success());
    assert!(rig.agent("uninstall").status.success());
    assert!(!Command::new("/bin/launchctl")
        .args(["print", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success());
    assert!(!rig.agents.join(format!("{label}.plist")).exists());
    assert_eq!(
        std::fs::read(rig.state.join("runtime.json")).unwrap(),
        config_before
    );
    assert!(rig.state.join("runtime.db").is_file());
    assert!(
        !rig.agent("verify").status.success(),
        "verify must reject an unloaded service"
    );
    assert!(rig.agent("uninstall").status.success());
}
