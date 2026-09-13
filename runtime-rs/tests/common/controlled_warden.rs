//! Real Warden in disposable state. Never changes the shipping policy or owner rig.
use super::runtime_crate as tradeassembly_runtime;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tradeassembly_runtime::finance_authority::{
    WardenSidecarAuthority, REQUIRED_WARDEN_SERVICE_VERSION, TRADEASSEMBLY_POLICY_BUNDLE,
};

pub struct ControlledWarden {
    child: Child,
    port: u16,
    pub authority: WardenSidecarAuthority,
}

impl ControlledWarden {
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
    pub fn start(root: &Path) -> Self {
        let binary = PathBuf::from(
            std::env::var_os("F2_TEST_WARDEN_BINARY")
                .expect("explicit real Warden binary required"),
        )
        .canonicalize()
        .unwrap();
        let state = root.join("warden");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        tradeassembly_runtime::local_install::prepare(&state, &binary, port).unwrap();
        let token = std::fs::read_to_string(state.join("warden.token")).unwrap();
        let authority = WardenSidecarAuthority::new(
            format!("http://127.0.0.1:{port}"),
            token.trim(),
            REQUIRED_WARDEN_SERVICE_VERSION,
        )
        .unwrap();
        let mut command = base_command(&binary, &state);
        command
            .args([
                "serve",
                "--bind",
                &format!("127.0.0.1:{port}"),
                "--token-file",
            ])
            .arg(state.join("warden.token"));
        drop(listener);
        let child = command.spawn().unwrap();
        let mut owned = Self {
            child,
            port,
            authority,
        };
        owned.wait_ready();
        owned
    }

    fn wait_ready(&mut self) {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "isolated Warden exited"
            );
            if let Ok(response) = client
                .get(format!("http://127.0.0.1:{}/health", self.port))
                .send()
            {
                if response.status().is_success() {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "isolated Warden startup timeout");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The allow policy exists only inside this fixture's private database.
    pub fn allow_controlled_submission(&mut self) {
        let mut policy: serde_json::Value =
            serde_json::from_str(TRADEASSEMBLY_POLICY_BUNDLE).unwrap();
        policy["version"] = serde_json::json!("2099-01-01.fixture");
        let rule = policy["rules"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|rule| rule["action"] == "order.submit.live")
            .unwrap();
        assert_eq!(rule["min_pep_coverage"], "c5");
        rule["required_decision"] = serde_json::json!("allow");
        self.authority = self.authority.clone().with_policy_bundle(policy).unwrap();
    }
}

fn base_command(binary: &Path, state: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("--database")
        .arg(state.join("warden.sqlite"))
        .arg("--key-file")
        .arg(state.join("signing.seed"))
        .args(["--key-id", "tradeassembly-local-key", "--peps"])
        .arg(state.join("peps.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

impl Drop for ControlledWarden {
    fn drop(&mut self) {
        self.stop();
    }
}
