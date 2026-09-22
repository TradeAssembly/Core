//! Real stdio process and HTTP listener acceptance, without broker submissions.
//! This proves local orchestration only; it is not deployed OAuth evidence.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

struct Mcp {
    child: Child,
    input: ChildStdin,
    output: Receiver<Value>,
}
impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Mcp {
    fn start(root: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tradeassembly"))
            .current_dir(root)
            .env_clear()
            .env("TRADEASSEMBLY_AUTH_PROFILE", "local_owner")
            .env(
                "TRADEASSEMBLY_WARDEN_TOKEN_REF",
                "env://ONBOARDING_FIXTURE_TOKEN",
            )
            .env(
                "ONBOARDING_FIXTURE_TOKEN",
                "onboarding-fixture-token-32-bytes-only",
            )
            .args(["mcp", "serve"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(value) = serde_json::from_str(&line) {
                    if tx.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            child,
            input,
            output,
        }
    }
    fn call(&mut self, name: &str, args: Value) -> Value {
        writeln!(self.input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).unwrap();
        self.input.flush().unwrap();
        let response = self
            .output
            .recv_timeout(Duration::from_secs(20))
            .expect("MCP must respond promptly");
        response["result"]["structuredContent"].clone()
    }
}

#[test]
fn binary_browser_setup_survives_real_process_restart_and_cancel() {
    let root = tempfile::tempdir().unwrap();
    let mut first = Mcp::start(root.path());
    let started = first.call(
        "tradeassembly.onboarding.start",
        json!({"mode":"paper","environment":"staging","idempotency_key":"binary-onboarding"}),
    );
    assert_eq!(started["status"], "browser_action_required", "{started}");
    let id = started["onboardingId"].clone();
    let url = started["browserUrl"].as_str().unwrap();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(client.get(url).send().unwrap().status(), 200);
    let login = first.call("tradeassembly.account.login", json!({}));
    assert_eq!(login["authenticated"], false);
    assert_eq!(login["localRuntimeAvailable"], true);
    drop(first);
    let mut second = Mcp::start(root.path());
    let resumed = second.call(
        "tradeassembly.onboarding.status",
        json!({"onboardingId":id}),
    );
    assert_eq!(resumed["onboardingId"], id, "{resumed}");
    assert_ne!(resumed["browserUrl"], started["browserUrl"]);
    assert_eq!(resumed["ready"], false);
    let cancelled = second.call(
        "tradeassembly.onboarding.cancel",
        json!({"onboardingId":id}),
    );
    assert_eq!(cancelled["status"], "cancelled");
    drop(second);
    let mut third = Mcp::start(root.path());
    assert_eq!(
        third.call(
            "tradeassembly.onboarding.status",
            json!({"onboardingId":id})
        )["status"],
        "cancelled"
    );
}
