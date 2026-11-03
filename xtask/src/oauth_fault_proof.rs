// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Finite source-only proof against the frozen Mac candidate's exact Core tree.
//! Never rebuilds/replaces shipping artifacts or emits production readiness.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REVISION: &str = "3abf3e052b932dff5c0a0c18bf5673691842e58b";
const DESCRIPTOR: &str = "51da2114d1e8ec86de4089734b8f165d76fc80e98fefbd660f7586c150fa7fea";
const NATIVE: &str = "5e4a2a5c09717e1ea74256a7b2f3eef23c1cf32ff040ef2658b7cae7c346676a";
const OAUTH: &str = "runtime-rs/src/service/plugin_oauth.rs";
const TESTS: &str = "runtime-rs/src/service/plugin_oauth_fault_tests.rs";
const PREFIX: &str = "service::plugin_oauth::fault_tests::";
const CASES: &[&str] = &[
    "custody_and_durable_receipt_precede_ack",
    "credential_failure_never_sends_ack",
    "receipt_failure_never_sends_ack",
    "consumed_ack_lost_response_recovers_in_distinct_process_without_rewrite",
    "changed_credential_revision_blocks_ack_recovery",
];
const HOOKS: &[&str] = &[
    "    #[cfg(test)]\n    if fault_tests::active() {\n        return fault_tests::post_json(url, token, body);\n    }\n",
    "    #[cfg(test)]\n    if fault_tests::active() {\n        return fault_tests::actor_token(actor);\n    }\n",
    "#[cfg(test)]\n#[path = \"plugin_oauth_fault_tests.rs\"]\nmod fault_tests;\n\n",
];

pub(crate) fn run(args: &[String], root: &Path) -> i32 {
    match capture(args, root) {
        Ok(report) => {
            println!("{}", report.display());
            0
        }
        Err(error) => {
            eprintln!("OAuth source fault proof failed: {error}");
            1
        }
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|_| "git unavailable")?;
    if !output.status.success() || output.stdout.len() > 128 * 1024 * 1024 {
        return Err("frozen source lookup failed".into());
    }
    Ok(output.stdout)
}

fn remove_hooks(source: &str) -> Result<String, String> {
    let mut restored = source.to_string();
    for hook in HOOKS {
        if restored.matches(hook).count() != 1 {
            return Err("test overlay is not the exact allowlisted patch".into());
        }
        restored = restored.replacen(hook, "", 1);
    }
    Ok(restored)
}

fn bounded(command: &mut Command, output: &Path) -> Result<(), String> {
    let log = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|_| "new log unavailable")?;
    let mut child = command
        .stdout(Stdio::from(log.try_clone().map_err(|_| "log unavailable")?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|_| "proof process unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| "proof process status unavailable")?
        {
            return if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "proof process failed; inspect {}",
                    output.display()
                ))
            };
        }
        if Instant::now() >= deadline
            || fs::metadata(output)
                .map(|m| m.len() > 16 * 1024 * 1024)
                .unwrap_or(true)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("proof time/output bound exceeded".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn capture(args: &[String], root: &Path) -> Result<std::path::PathBuf, String> {
    if args.len() != 4 || args[0] != "--descriptor" || args[2] != "--evidence" {
        return Err("usage: cargo xtask oauth-fault-proof --descriptor ABSOLUTE_DESCRIPTOR --evidence ABSOLUTE_NEW_DIRECTORY".into());
    }
    let descriptor_path = Path::new(&args[1]);
    let evidence = Path::new(&args[3]);
    if !descriptor_path.is_absolute() || !evidence.is_absolute() || evidence.exists() {
        return Err("absolute descriptor and new evidence directory required".into());
    }
    let descriptor_bytes = fs::read(descriptor_path).map_err(|_| "descriptor unavailable")?;
    let descriptor: Value =
        serde_json::from_slice(&descriptor_bytes).map_err(|_| "descriptor invalid")?;
    if sha(&descriptor_bytes) != DESCRIPTOR
        || descriptor["nativeInputs"]["coreRevision"] != REVISION
        || descriptor["nativeInputs"]["coreSha256"] != NATIVE
        || descriptor["target"] != "aarch64-apple-darwin"
    {
        return Err("frozen candidate binding differs".into());
    }
    let base = git(root, &["show", &format!("{REVISION}:{OAUTH}")])?;
    let overlay = fs::read(root.join(OAUTH)).map_err(|_| "overlay unavailable")?;
    let restored = remove_hooks(std::str::from_utf8(&overlay).map_err(|_| "overlay invalid")?)?;
    if restored.as_bytes() != base {
        return Err("overlay changed frozen OAuth control flow".into());
    }
    let tests = fs::read(root.join(TESTS)).map_err(|_| "test support unavailable")?;
    let archive = git(root, &["archive", "--format=tar", REVISION])?;
    fs::create_dir(evidence).map_err(|_| "evidence directory creation failed")?;
    let source = evidence.join("frozen-source");
    fs::create_dir(&source).map_err(|_| "source directory creation failed")?;
    tar::Archive::new(archive.as_slice())
        .unpack(&source)
        .map_err(|_| "frozen archive extraction failed")?;
    if fs::read(source.join(OAUTH)).map_err(|_| "archived OAuth unavailable")? != base {
        return Err("archive/source mismatch".into());
    }
    fs::write(source.join(OAUTH), &overlay).map_err(|_| "overlay write failed")?;
    fs::write(source.join(TESTS), &tests).map_err(|_| "test support write failed")?;
    // Compile the archived frozen dependency tree, NOT current-tree repairs.
    // Reuse only Cargo's cache. This does not write any qualified payload path.
    let target = root
        .canonicalize()
        .map_err(|_| "root unavailable")?
        .join("target");
    let build_log = evidence.join("build.log");
    let mut build = Command::new("cargo");
    build
        .current_dir(&source)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .env("CARGO_INCREMENTAL", "0")
        .args([
            "test",
            "--locked",
            "--offline",
            "-p",
            "tradeassembly-runtime",
            "--lib",
            "--no-run",
            "--message-format=json",
            "--target-dir",
        ])
        .arg(&target);
    bounded(&mut build, &build_log)?;
    let build_bytes = fs::read(&build_log).map_err(|_| "build log unavailable")?;
    let executables: Vec<String> = String::from_utf8_lossy(&build_bytes)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| {
            v["reason"] == "compiler-artifact"
                && v["target"]["name"] == "tradeassembly_runtime"
                && v["profile"]["test"] == true
                && v["features"] == json!([])
                && v["profile"]["debug_assertions"] == true
                && v["profile"]["overflow_checks"] == true
        })
        .filter_map(|v| v["executable"].as_str().map(str::to_string))
        .collect();
    if executables.len() != 1 {
        return Err("exact frozen-source test executable missing".into());
    }
    let executable = Path::new(&executables[0]);
    if !executable.starts_with(&target) {
        return Err("test executable outside Cargo target".into());
    }
    let executable_bytes = fs::read(executable).map_err(|_| "test executable unavailable")?;
    if executable_bytes.get(..8) != Some(&[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1]) {
        return Err("source proof test executable is not ARM64 Mach-O".into());
    }
    let executable_sha = sha(&executable_bytes);
    let mut results = Vec::new();
    for case in CASES {
        let name = format!("{PREFIX}{case}");
        let log_path = evidence.join(format!("{case}.log"));
        let mut test = Command::new(executable);
        test.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .args(["--exact", &name, "--test-threads=1"]);
        bounded(&mut test, &log_path)?;
        let bytes = fs::read(&log_path).map_err(|_| "test log unavailable")?;
        let text = std::str::from_utf8(&bytes).map_err(|_| "test log invalid")?;
        validate_result(text, &name)?;
        results.push(
            json!({"name":name,"log":format!("{case}.log"),"sha256":sha(&bytes),"exitCode":0}),
        );
    }
    let rustc = Command::new("rustc")
        .arg("-Vv")
        .output()
        .map_err(|_| "toolchain unavailable")?;
    if !rustc.status.success()
        || !String::from_utf8_lossy(&rustc.stdout).contains("host: aarch64-apple-darwin")
    {
        return Err("frozen Mac source proof requires ARM64 Mac host".into());
    }
    let report = json!({"schemaVersion":1,"kind":"frozen-core-oauth-source-fault-proof",
        "producerSha256":sha(include_bytes!("oauth_fault_proof.rs")),
        "coreRevision":REVISION,"descriptorSha256":DESCRIPTOR,"nativeSha256":NATIVE,
        "sourceArchiveSha256":sha(&archive),"baseOauthSha256":sha(&base),"overlayOauthSha256":sha(&overlay),
        "testSupportSha256":sha(&tests),"cargoLockSha256":sha(&fs::read(source.join("Cargo.lock")).map_err(|_| "frozen lock unavailable")?),
        "testExecutableSha256":executable_sha,"buildLogSha256":sha(&build_bytes),
        "toolchain":String::from_utf8(rustc.stdout).map_err(|_| "toolchain invalid")?,"target":"aarch64-apple-darwin",
        "features":"default","cases":results,"sourceFaultProofVerified":true,
        "productionHttpVerified":false,"productionIdentityVerified":false,"packagedFaultExecutionVerified":false,
        "fullOnboardingVerified":false,"releaseReady":false});
    let report_path = evidence.join("proof.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).map_err(|_| "report encoding failed")?,
    )
    .map_err(|_| "report write failed")?;
    Ok(report_path)
}

fn validate_result(text: &str, name: &str) -> Result<(), String> {
    if !text
        .lines()
        .any(|line| line == format!("test {name} ... ok"))
        || !text.contains("1 passed; 0 failed; 0 ignored;")
    {
        return Err("named behavioral assertion did not execute successfully".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_skipped_wrong_or_missing_assertions() {
        assert!(validate_result("test x ... ok\n1 passed; 0 failed; 0 ignored;", "x").is_ok());
        for text in [
            "test x ... ignored\n0 passed; 0 failed; 1 ignored;",
            "test y ... ok\n1 passed; 0 failed; 0 ignored;",
            "1 passed; 0 failed; 0 ignored;",
        ] {
            assert!(validate_result(text, "x").is_err());
        }
    }
    #[test]
    fn reconstruction_rejects_missing_and_duplicate_hooks() {
        let overlay = HOOKS.join("");
        assert_eq!(remove_hooks(&overlay).unwrap(), "");
        assert!(remove_hooks("").is_err());
        assert!(remove_hooks(&(overlay.clone() + HOOKS[0])).is_err());
    }
}
