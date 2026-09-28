// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Local tarball delivery only. Public-registry delivery is separate evidence.

use std::{fs, path::Path, process::Command};
use tradeassembly_distribution::{atomic_json, executable};

fn succeeds(command: &mut Command) {
    let result = command.output().expect("command starts");
    assert!(
        result.status.success(),
        "package-manager command failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "requires explicit native candidate and installed npm/pnpm; runs actual installer"]
fn npm_and_pnpm_local_tarballs_work_without_lifecycle_scripts_or_source() {
    let candidate = std::env::var_os("TRADEASSEMBLY_DISTRIBUTION_CANDIDATE")
        .expect("explicit candidate required");
    let candidate = Path::new(&candidate);
    let package: serde_json::Value =
        tradeassembly_distribution::read_json(&candidate.join("candidate.json")).unwrap();
    let platform = package["platformPackage"].as_str().unwrap();
    let version = package["release"]["version"].as_str().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("tradeassembly-package-managers-")
        .tempdir()
        .unwrap();
    for name in ["tradeassembly", platform] {
        succeeds(
            Command::new("npm")
                .current_dir(candidate.join(name))
                .args(["pack", "--ignore-scripts", "--json", "--pack-destination"])
                .arg(temp.path()),
        );
        let metadata: serde_json::Value =
            tradeassembly_distribution::read_json(&candidate.join(name).join("package.json"))
                .unwrap();
        assert!(
            metadata.get("scripts").is_none(),
            "no lifecycle hook exists"
        );
    }
    let launcher = temp.path().join(format!("tradeassembly-{version}.tgz"));
    let native = temp.path().join(format!("{platform}-{version}.tgz"));
    let npm_client = temp.path().join("npm-client");
    let pnpm_client = temp.path().join("pnpm-client");
    let rig = temp.path().join("isolated owned rig");
    succeeds(
        Command::new("npm")
            .args(["install", "--prefix"])
            .arg(&npm_client)
            .args(["--offline", "--ignore-scripts", "--no-audit", "--no-fund"])
            .arg(&launcher)
            .arg(&native),
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    drop(listener);
    succeeds(
        Command::new("npm")
            .args(["exec", "--prefix"])
            .arg(&npm_client)
            .args([
                "--offline",
                "--no",
                "--",
                "tradeassembly",
                "install",
                "--root",
            ])
            .arg(&rig)
            .args(["--warden-port", &port]),
    );
    let stable = rig.join(executable("bin/tradeassembly"));
    assert!(stable.is_file());
    fs::remove_dir_all(&npm_client).unwrap();
    succeeds(Command::new(&stable).arg("--help"));
    fs::create_dir(&pnpm_client).unwrap();
    atomic_json(
        &pnpm_client.join("package.json"),
        &serde_json::json!({"name":"isolated-cohort-acceptance","private":true}),
    )
    .unwrap();
    succeeds(
        Command::new("pnpm")
            .arg("--dir")
            .arg(&pnpm_client)
            .args(["add", "--offline", "--ignore-scripts"])
            .arg(&launcher)
            .arg(&native),
    );
    succeeds(
        Command::new("pnpm")
            .arg("--dir")
            .arg(&pnpm_client)
            .args(["exec", "tradeassembly", "install", "--root"])
            .arg(&rig)
            .args(["--warden-port", &port]),
    );
    fs::remove_dir_all(pnpm_client).unwrap();
    succeeds(Command::new(&stable).args(["distribution", "status"]));
    assert!(rig.join("state/local/runtime.db").is_file());
}
