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
    let mut command = Command::new(bundle.join("bin/tradeassembly-sandbox"));
    command
        .env_clear()
        .current_dir(bundle)
        .arg("--settings")
        .arg(settings);
    command
}

#[test]
#[ignore = "requires explicit F2_TEST_BUNDLE_PATH containing real packaged binaries"]
fn packaged_sandbox_runs_and_denies_files_and_network_without_host_node() {
    let bundle = PathBuf::from(std::env::var_os("F2_TEST_BUNDLE_PATH").expect("select bundle"))
        .canonicalize()
        .unwrap();
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
    let mut allowed = sandbox(&bundle, &settings);
    allowed
        .arg("/usr/bin/printf")
        .arg("%s")
        .arg("argument with spaces & ; intact");
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
    let mut read = sandbox(&bundle, &settings);
    read.arg("/bin/cat").arg(&restricted);
    let output = bounded(read);
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private fixture"));
    let mut write = sandbox(&bundle, &settings);
    write.arg("/usr/bin/touch").arg(&forbidden_write);
    assert!(!bounded(write).status.success());
    assert!(!forbidden_write.exists());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // Prove the destination is reachable outside the sandbox, then drain it.
    let control = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let _ = listener.accept().unwrap();
    drop(control);
    listener.set_nonblocking(true).unwrap();
    let mut network = sandbox(&bundle, &settings);
    network
        .args(["/usr/bin/curl", "--fail", "--silent", "--max-time", "3"])
        .arg(format!("http://{}", listener.local_addr().unwrap()));
    assert!(!bounded(network).status.success());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );

    // Execute the real pinned plugin's local discovery operation through the
    // bundled sandbox. No broker credentials or network access are needed.
    use sha2::{Digest, Sha256};
    let pin: serde_json::Value =
        serde_json::from_str(include_str!("../../packaging/alpaca.json")).unwrap();
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
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(package.as_slice()));
    let plugin = root.join("tradeassembly-plugin-alpaca");
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap()
            == Path::new("bin/aarch64-apple-darwin/tradeassembly-plugin-alpaca")
        {
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
