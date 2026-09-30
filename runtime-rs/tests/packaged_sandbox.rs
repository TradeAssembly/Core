//! Explicit real-bundle proof; no credentials, broker orders or user files.
use serde_json::json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn bounded(command: Command) -> Output {
    bounded_input(command, None)
}

fn bounded_input(mut command: Command, input: Option<&[u8]>) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    // Drain concurrently: capability discovery can exceed the OS pipe buffer.
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let read_pipe = |pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(1_048_577).read_to_end(&mut bytes).unwrap();
            assert!(bytes.len() <= 1_048_576, "fixture output exceeded limit");
            bytes
        })
    };
    let stdout = read_pipe(Box::new(stdout));
    let stderr = read_pipe(Box::new(stderr));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Output {
                status,
                stdout: stdout.join().unwrap(),
                stderr: stderr.join().unwrap(),
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("bundle command timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sandbox(bundle: &Path, settings: &Path) -> Command {
    let mut command = Command::new(bundle.join(executable("bin/tradeassembly-sandbox")));
    command
        .env_clear()
        .current_dir(bundle)
        .arg("--settings")
        .arg(settings);
    command
}

fn executable(path: &str) -> String {
    if cfg!(windows) {
        format!("{path}.exe")
    } else {
        path.into()
    }
}

fn node_probe(bundle: &Path, settings: &Path, script: &str) -> Command {
    let mut command = sandbox(bundle, settings);
    command
        .arg(bundle.join(executable("runtime/node/bin/node")))
        .args(["--eval", script]);
    command
}

#[test]
#[ignore = "requires explicit F2_TEST_BUNDLE_PATH containing real packaged binaries"]
fn packaged_sandbox_runs_and_denies_files_and_network_without_host_node() {
    let bundle = PathBuf::from(std::env::var_os("F2_TEST_BUNDLE_PATH").expect("select bundle"))
        .canonicalize()
        .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    let target = tradeassembly_plugin_sdk::host_target();
    assert_eq!(manifest["target"], target, "native host required");
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let restricted = root.join("restricted");
    let forbidden_write = root.join("forbidden-write");
    std::fs::write(&restricted, "private fixture, not a credential").unwrap();
    let settings = root.join("settings.json");
    std::fs::write(&settings, serde_json::to_vec(&json!({
        "network":{"allowedDomains":[],"deniedDomains":[],"allowUnixSockets":[],"allowLocalBinding":false},
        "filesystem":{"denyRead":[restricted],"allowRead":[],"allowWrite":[],"denyWrite":[forbidden_write]},
        "enableWeakerNestedSandbox":false,"enableWeakerNetworkIsolation":false,"allowAppleEvents":false
    })).unwrap()).unwrap();
    let mut allowed = node_probe(&bundle, &settings, "process.stdout.write(process.argv[1])");
    allowed.arg("argument with spaces & ; intact");
    let output = bounded(allowed);
    assert!(
        output.status.success(),
        "sandbox launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "argument with spaces & ; intact"
    );
    // The outside-sandbox control proves the target exists and is readable.
    // A missing executable/file or arbitrary failure cannot prove denial.
    let read_script = "try { process.stdout.write(require('fs').readFileSync(process.argv[1])); } catch (e) { process.exit(['EACCES','EPERM'].includes(e.code) ? 77 : 78); }";
    let mut control = Command::new(bundle.join(executable("runtime/node/bin/node")));
    control
        .env_clear()
        .args(["--eval", read_script])
        .arg(&restricted);
    assert!(bounded(control).status.success(), "outside read control");
    let mut read = node_probe(&bundle, &settings, read_script);
    read.arg(&restricted);
    let output = bounded(read);
    assert_eq!(output.status.code(), Some(77), "OS read denial required");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private fixture"));
    let mut write = node_probe(&bundle, &settings, "try { require('fs').writeFileSync(process.argv[1], 'fixture'); } catch (e) { process.exit(['EACCES','EPERM'].includes(e.code) ? 77 : 78); }");
    write.arg(&forbidden_write);
    assert_eq!(
        bounded(write).status.code(),
        Some(77),
        "OS write denial required"
    );
    assert!(!forbidden_write.exists());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // Prove the destination is reachable outside the sandbox, then drain it.
    let control = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let _ = listener.accept().unwrap();
    drop(control);
    listener.set_nonblocking(true).unwrap();
    let mut network = node_probe(&bundle, &settings, "const socket = require('net').connect(Number(process.argv[1]), '127.0.0.1'); const timer = setTimeout(() => { socket.destroy(); process.exit(77); }, 3000); socket.on('connect', () => { clearTimeout(timer); socket.destroy(); process.exit(0); }); socket.on('error', e => { clearTimeout(timer); process.exit(['EACCES','EPERM','ECONNREFUSED','ECONNRESET','ENETUNREACH','EHOSTUNREACH','ETIMEDOUT'].includes(e.code) ? 77 : 78); });");
    network.arg(listener.local_addr().unwrap().port().to_string());
    assert_eq!(
        bounded(network).status.code(),
        Some(77),
        "network denial required"
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );

    // Execute the real pinned plugin's local discovery operation through the
    // bundled sandbox. No broker credentials or network access are needed.
    use sha2::{Digest, Sha256};
    let pin_bytes = std::fs::read(bundle.join("plugins/alpaca.json")).unwrap();
    let pin: serde_json::Value = serde_json::from_slice(&pin_bytes).unwrap();
    assert_eq!(pin["target"], target);
    assert_eq!(
        manifest["files"]["plugins/alpaca.json"]["sha256"],
        format!("{:x}", Sha256::digest(&pin_bytes)),
        "bundle pin binding required"
    );
    let package = std::fs::read(
        bundle
            .join("plugins")
            .join(pin["packageFile"].as_str().unwrap()),
    )
    .unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&package)),
        pin["packageSha256"]
    );
    assert_eq!(
        manifest["files"][format!("plugins/{}", pin["packageFile"].as_str().unwrap())]["sha256"],
        pin["packageSha256"],
        "bundle package binding required"
    );
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(package.as_slice()));
    let plugin = root.join(executable("tradeassembly-plugin-alpaca"));
    let binary_path = executable(&format!("bin/{target}/tradeassembly-plugin-alpaca"));
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == Path::new(&binary_path) {
            assert!(entry.header().entry_type().is_file());
            assert!(!found);
            entry.unpack(&plugin).unwrap();
            found = true;
        }
    }
    assert!(found);
    let request = json!({
        "schemaVersion":"1", "hostContractVersion":"1", "sdkVersion":"0.1.0",
        "requestId":"packaged-capability-probe",
        "metadata":{
            "operationId":"plugin.capability_matrix", "pluginId":"tradeassembly.alpaca",
            "pluginInstanceId":"fixture", "packageDigest":pin["packageSha256"],
            "manifestDigest":pin["manifestSha256"], "capabilityBindingId":"fixture",
            "capabilityFingerprint":"fixture"
        },
        "context":{
            "authorityId":"fixture", "purpose":"capability_discovery", "mode":"paper",
            "fencingToken":"fixture", "timeoutMs":1000, "idempotencyKey":"fixture-capability",
            "evidenceReferences":[]
        },
        "payload":{"kind":"operation","payload":{}}
    });
    let mut discover = sandbox(&bundle, &settings);
    discover.arg(plugin);
    let output = bounded_input(discover, Some(format!("{request}\n").as_bytes()));
    assert!(
        output.status.success(),
        "plugin launch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["requestId"], "packaged-capability-probe");
    assert_eq!(response["status"], "succeeded");
    assert_eq!(response["payload"]["pluginId"], "tradeassembly.alpaca");
    assert!(response["payload"]["operations"]
        .as_array()
        .is_some_and(|ops| !ops.is_empty()));
}
