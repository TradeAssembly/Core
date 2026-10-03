// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Local and public-registry delivery are separate opt-in, real-binary checks.

use std::{fs, path::Path, process::Command};
use tradeassembly_distribution::{atomic_json, executable};

fn package_manager(name: &str) -> Command {
    // Windows installs these CLIs as batch launchers. Rust's Command handles
    // batch-file argument escaping; do not concatenate paths into a shell line.
    #[cfg(windows)]
    let name = format!("{name}.cmd");
    Command::new(name)
}

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
            package_manager("npm")
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
        package_manager("npm")
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
        package_manager("npm")
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
        package_manager("pnpm")
            .arg("--dir")
            .arg(&pnpm_client)
            .args(["add", "--offline", "--ignore-scripts"])
            .arg(&launcher)
            .arg(&native),
    );
    succeeds(
        package_manager("pnpm")
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

#[test]
#[ignore = "requires published exact registry packages and a native Mac ARM64 host"]
fn npm_and_pnpm_registry_packages_work_without_scripts_or_source() {
    use serde_json::{json, Value};
    use tradeassembly_distribution::{digest, read_json, Release, TARGETS};
    assert_eq!(
        tradeassembly_distribution::install::host_target().unwrap(),
        TARGETS[0]
    );
    let evidence = std::env::var_os("TRADEASSEMBLY_REGISTRY_EVIDENCE")
        .expect("explicit qualified registry evidence required");
    let evidence = Path::new(&evidence);
    // This check cannot replace candidate qualification or registry-byte capture.
    tradeassembly_distribution::package::verify_matrix(evidence, false).unwrap();
    let release: Release = read_json(&evidence.join(TARGETS[0]).join("release.json")).unwrap();
    let native: Value = read_json(&evidence.join(TARGETS[0]).join("receipt.json")).unwrap();
    let registry: Value = read_json(&evidence.join("registry/registry.json")).unwrap();
    assert_eq!(registry["version"], release.version);
    let temp = tempfile::Builder::new()
        .prefix("tradeassembly-registry-delivery-")
        .tempdir()
        .unwrap();
    // Public packages need no operator npm credentials. Never read the user's
    // npmrc, global configuration or shared cache in this isolated test.
    let empty_config = temp.path().join("user.npmrc");
    let global_config = temp.path().join("global.npmrc");
    for config in [&empty_config, &global_config] {
        fs::write(config, b"registry=https://registry.npmjs.org\n").unwrap();
    }
    let search_path = std::env::var_os("PATH").expect("package-manager search path required");
    for manager in ["npm", "pnpm"] {
        let client = temp.path().join(format!("{manager}-client"));
        fs::create_dir(&client).unwrap();
        atomic_json(
            &client.join("package.json"),
            &json!({
                "name":"isolated-registry-acceptance","version":"0.0.0","private":true
            }),
        )
        .unwrap();
        let cache = temp.path().join(format!("{manager}-cache"));
        let clean = |command: &mut Command| {
            command
                .env_clear()
                .env("PATH", &search_path)
                .env("LANG", "C")
                .env("NPM_CONFIG_USERCONFIG", &empty_config)
                .env("NPM_CONFIG_GLOBALCONFIG", &global_config)
                .env("NPM_CONFIG_CACHE", &cache)
                .env("NPM_CONFIG_FETCH_RETRIES", "0")
                .env("NPM_CONFIG_FETCH_TIMEOUT", "30000")
                .env_remove("NODE_AUTH_TOKEN")
                .env_remove("NPM_TOKEN");
        };
        let mut install = package_manager(manager);
        if manager == "npm" {
            install.args(["install", "--prefix"]).arg(&client).args([
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
            ]);
        } else {
            install
                .arg("--dir")
                .arg(&client)
                .args(["add", "--ignore-scripts"]);
        }
        install
            .arg("--registry=https://registry.npmjs.org")
            .arg(format!("tradeassembly@{}", release.version));
        clean(&mut install);
        succeeds(&mut install);
        let modules = client.join("node_modules");
        let platform = modules.join("tradeassembly-darwin-arm64");
        let launcher = modules.join("tradeassembly");
        for (name, directory) in [
            ("tradeassembly", &launcher),
            ("tradeassembly-darwin-arm64", &platform),
        ] {
            let metadata: Value = read_json(&directory.join("package.json")).unwrap();
            assert_eq!(metadata["name"], name);
            assert_eq!(metadata["version"], release.version);
            assert!(
                metadata.get("scripts").is_none(),
                "lifecycle scripts prohibited"
            );
        }
        let fetched: Release = read_json(&platform.join("release.json")).unwrap();
        assert_eq!(
            fetched, release,
            "registry installs the qualified native release"
        );
        assert_eq!(
            digest(&platform.join("bundle.tar.gz")).unwrap(),
            release.archive_sha256
        );
        assert_eq!(
            digest(&launcher.join("cli.cjs")).unwrap(),
            native["launcherSha256"]
        );
        assert_eq!(
            digest(&platform.join(executable("tradeassembly-distribution"))).unwrap(),
            native["artifacts"]["installer"]["sha256"]
        );
        let rig = temp.path().join(format!("{manager} owned rig with spaces"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        drop(listener);
        let mut execute = package_manager(manager);
        if manager == "npm" {
            execute.args(["exec", "--prefix"]).arg(&client).args([
                "--offline",
                "--no",
                "--",
                "tradeassembly",
            ]);
        } else {
            execute
                .arg("--dir")
                .arg(&client)
                .args(["exec", "tradeassembly"]);
        }
        execute
            .args(["install", "--root"])
            .arg(&rig)
            .args(["--warden-port", &port]);
        clean(&mut execute);
        succeeds(&mut execute);
        let installed: Value = read_json(&rig.join("installed.json")).unwrap();
        assert_eq!(installed["current"]["version"], release.version);
        assert_eq!(
            digest(&rig.join(executable("authority/bin/warden"))).unwrap(),
            release.warden_sha256
        );
        assert!(rig.join("state/local/runtime.db").is_file());
        // Prove the stable install outlives both package-manager source and cache.
        fs::remove_dir_all(&client).unwrap();
        if cache.exists() {
            fs::remove_dir_all(&cache).unwrap();
        }
        let stable = rig.join(executable("bin/tradeassembly"));
        succeeds(Command::new(&stable).arg("--help"));
        succeeds(Command::new(&stable).args(["distribution", "status"]));
    }
}
