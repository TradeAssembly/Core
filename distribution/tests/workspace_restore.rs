//! Native installed-workspace recovery, with no hosted services or credentials.
use serde_json::Value;
use std::{
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tradeassembly_distribution::executable;

struct Policy(Child);
impl Drop for Policy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn command(binary: &Path, home: &Path) -> Command {
    let mut command = Command::new(binary);
    command.env_clear().env("HOME", home).env(
        "PATH",
        std::env::var_os("PATH").expect("native platform path"),
    );
    command
}
fn invoke(command: &mut Command, input: Option<&[u8]>) -> (bool, Value) {
    // File capture prevents pipe deadlocks and never prints backup contents.
    let mut output = tempfile::tempfile().unwrap();
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(output.try_clone().unwrap()))
        .stderr(Stdio::null())
        .spawn()
        .expect("native command starts");
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native recovery command exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(output.metadata().unwrap().len() <= 1024 * 1024);
    output.rewind().unwrap();
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes).unwrap();
    let value = serde_json::from_slice(&bytes).expect("structured native command result");
    (status.success(), value)
}
struct Installation {
    binary: PathBuf,
    runtime: PathBuf,
    database: PathBuf,
    _policy: Policy,
}
impl Installation {
    fn new(package: &Path, root: &Path, home: &Path) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let (success, receipt) = invoke(
            command(
                &package.join(executable("tradeassembly-distribution")),
                home,
            )
            .args(["install", "--root"])
            .arg(root)
            .args(["--warden-port", &port.to_string()]),
            None,
        );
        assert!(
            success && receipt["installed"] == true,
            "fresh native install"
        );
        let binary = root
            .join("versions")
            .join(receipt["payloadSha256"].as_str().unwrap())
            .join(executable("bin/tradeassembly"));
        let state = root.join("state/local");
        let mut policy = Policy(
            command(&binary, home)
                .args(["policy", "serve", "--state-dir"])
                .arg(&state)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                policy.0.try_wait().unwrap().is_none(),
                "owned Warden stays alive"
            );
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "owned Warden startup");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self {
            binary,
            runtime: state.join("runtime.json"),
            database: state.join("runtime.db"),
            _policy: policy,
        }
    }
    fn query(&self, home: &Path, args: &[&str], input: Option<&[u8]>) -> (bool, Value) {
        invoke(
            command(&self.binary, home)
                .arg("--config")
                .arg(&self.runtime)
                .args(args),
            input,
        )
    }
    fn import(&self, home: &Path, bundle: &[u8]) -> (bool, Value) {
        self.query(
            home,
            &[
                "--db",
                self.database.to_str().unwrap(),
                "workspace",
                "import",
                "--stdin",
            ],
            Some(bundle),
        )
    }
}

#[test]
#[ignore = "explicit frozen native package; real installer, local-owner CLI and Warden only"]
fn installed_default_plugin_bootstrap_allows_restore_and_exact_replay_only() {
    let package = PathBuf::from(
        std::env::var_os("TRADEASSEMBLY_DISTRIBUTION_PACKAGE")
            .expect("explicit frozen native package required"),
    );
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().canonicalize().unwrap();
    let source = Installation::new(&package, &home.join("source"), &home);
    let target = Installation::new(&package, &home.join("target"), &home);
    let mutated = Installation::new(&package, &home.join("mutated"), &home);
    for installation in [&source, &target, &mutated] {
        assert!(installation.query(&home, &["auth", "status"], None).0);
    }
    assert!(
        source
            .query(
                &home,
                &[
                    "strategy",
                    "create",
                    "--mode",
                    "blank",
                    "--name",
                    "Restore regression"
                ],
                None
            )
            .0
    );
    let (success, envelope) = source.query(&home, &["workspace", "export", "--stdout"], None);
    assert!(success && envelope["ok"] == true);
    assert_eq!(envelope["data"]["entries"].as_array().unwrap().len(), 2);
    let bundle = serde_json::to_vec(&envelope["data"]).unwrap();
    let (success, restored) = target.import(&home, &bundle);
    assert!(
        success,
        "untouched installed bootstrap must permit restore; code={}",
        restored["data"]["error"]["code"]
    );
    assert_eq!(restored["data"]["restoredState"], "stopped");
    let (success, replay) = target.import(&home, &bundle);
    assert!(success, "exact restore retry must succeed");
    assert_eq!(replay["data"]["duplicate"], true);

    // Changing one installed value must not be adopted as a new baseline.
    let connection = rusqlite::Connection::open(&mutated.database).unwrap();
    connection.execute(
        "UPDATE tradeassembly_kv SET value_json=json_set(value_json,'$.installedAtMs',0) WHERE namespace='plugin_manifests'",
        [],
    ).unwrap();
    let (success, rejected) = mutated.import(&home, &bundle);
    assert!(!success);
    assert_eq!(rejected["data"]["error"]["code"], "workspace_not_empty");
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM tradeassembly_kv WHERE namespace='public_workspace_imports_v1'", [], |row| row.get::<_, usize>(0)).unwrap(), 0);

    // A customer write after restoration also makes replay non-exact.
    assert!(
        target
            .query(
                &home,
                &[
                    "strategy",
                    "create",
                    "--mode",
                    "blank",
                    "--name",
                    "Additional customer data"
                ],
                None
            )
            .0
    );
    let (success, rejected) = target.import(&home, &bundle);
    assert!(!success);
    assert_eq!(rejected["data"]["error"]["code"], "workspace_not_empty");
}
