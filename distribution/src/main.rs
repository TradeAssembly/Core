// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use std::path::{Path, PathBuf};
use tradeassembly_distribution::{install, package, Result};

fn option(args: &[String], name: &str) -> Result<Option<String>> {
    let matches: Vec<_> = args
        .iter()
        .enumerate()
        .filter(|(_, v)| v.as_str() == name)
        .collect();
    if matches.len() > 1 {
        return Err("duplicate_option".into());
    }
    matches
        .first()
        .map(|(i, _)| {
            args.get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| "option_value_required".into())
        })
        .transpose()
}
fn required(args: &[String], name: &str) -> Result<String> {
    option(args, name)?.ok_or_else(|| format!("required_option:{name}"))
}
fn execute(args: &[String]) -> Result<i32> {
    let exe = std::env::current_exe().map_err(|_| "installer_path_unavailable")?;
    let facade = exe.file_stem().and_then(|s| s.to_str()) == Some("tradeassembly");
    let root = if facade {
        exe.parent()
            .and_then(Path::parent)
            .ok_or("facade_path_invalid")?
            .to_owned()
    } else {
        option(args, "--root")?
            .map(|path| Ok(PathBuf::from(path)))
            .unwrap_or_else(install::default_root)?
    };
    if facade && !matches!(args.first().map(String::as_str), Some("distribution")) {
        return install::run(&root, args);
    }
    let args = if facade { &args[1..] } else { args };
    let command = args.first().map(String::as_str).unwrap_or("help");
    let value = match command {
        "install" | "upgrade" => {
            let package = exe.parent().ok_or("package_path_invalid")?;
            let port = option(args, "--warden-port")?
                .unwrap_or_else(|| "8181".into())
                .parse()
                .map_err(|_| "warden_port_invalid")?;
            install::install(
                package,
                &root,
                command == "upgrade" || args.iter().any(|s| s == "--upgrade"),
                port,
            )?
        }
        "status" => install::status(&root)?,
        "rollback" => install::rollback(&root)?,
        "run" => return install::run(&root, &args[1..]),
        "freeze-native" => {
            tradeassembly_distribution::native::freeze(
                Path::new(&required(args, "--input")?),
                Path::new(&required(args, "--metadata")?),
                Path::new(&required(args, "--parent")?),
                Path::new(&required(args, "--out")?),
            )?;
            serde_json::json!({"frozen":true,"qualified":false})
        }
        "describe-native-inputs" => tradeassembly_distribution::native::describe_inputs(
            Path::new(&required(args, "--input")?),
            &required(args, "--core-revision")?,
            &required(args, "--warden-revision")?,
            Path::new(&required(args, "--out")?),
        )?,
        "describe-candidate" => tradeassembly_distribution::candidate::describe(
            Path::new(&required(args, "--bundle")?),
            Path::new(&required(args, "--parent")?),
            &required(args, "--version")?,
            Path::new(&required(args, "--out")?),
        )?,
        "pack" => package::pack(
            Path::new(&required(args, "--bundle")?),
            Path::new(&required(args, "--parent")?),
            &required(args, "--version")?,
            Path::new(&required(args, "--installer")?),
            option(args, "--candidate-descriptor")?
                .as_deref()
                .map(Path::new),
            Path::new(&required(args, "--out")?),
        )?,
        "qualify" => tradeassembly_distribution::qualify::qualify(
            Path::new(&required(args, "--candidate")?),
            option(args, "--baseline-package")?
                .as_deref()
                .map(Path::new),
            Path::new(&required(args, "--controlled-broker")?),
            Path::new(&required(args, "--source")?),
            Path::new(&required(args, "--out")?),
        )?,
        "verify" => package::verify_matrix(
            Path::new(&required(args, "--evidence")?),
            !args.iter().any(|arg| arg == "--candidate"),
        )?,
        "capture-registry" => package::capture_registry(Path::new(&required(args, "--evidence")?))?,
        "help" | "--help" => {
            println!("TradeAssembly distribution: install [--root ABSOLUTE_PATH] [--warden-port PORT], upgrade, status, rollback, run CORE_ARGS. No strategy is created or activated. Maintainers: describe-native-inputs --input STAGING --core-revision SHA --warden-revision SHA --out NEW_NATIVE_INPUTS_JSON; freeze-native --input STAGING --metadata JSON --parent LOCK --out NEW_PATH (native host required; not qualification); describe-candidate --bundle FROZEN --parent LOCK --version PRERELEASE --out NEW_DIRECTORY/candidate-descriptor.json; pack --bundle PATH --parent LOCK --version PRERELEASE --installer BINARY [--candidate-descriptor FILE] --out NEW_PATH; qualify --candidate DIR --baseline-package MAC_BASELINE_DIR --controlled-broker BINARY --source CORE_CHECKOUT --out NEW_EVIDENCE_DIR; verify --evidence DIRECTORY [--candidate]; capture-registry --evidence DIRECTORY (read-only npm capture after candidate qualification).");
            return Ok(0);
        }
        _ => return install::run(&root, args),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|_| "output_encode_failed")?
    );
    Ok(0)
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = match execute(&args) {
        Ok(code) => code,
        Err(code) => {
            eprintln!("{}", serde_json::json!({"ok":false,"code":code}));
            1
        }
    };
    std::process::exit(status);
}
