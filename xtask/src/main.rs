// Copyright (c) 2026 OptionLab LLC. All rights reserved.
mod architecture_check;
mod bundle_local;
mod check_whitelist;
mod core_verify;
mod foss_core_boundary;
mod plugin_contract;
mod public_scan;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::Path::new(".");
    let rest = args.get(1..).unwrap_or_default();
    let status = match args.first().map(String::as_str).unwrap_or("help") {
        "verify" if rest.is_empty() => core_verify::verify(),
        "setup" if rest.is_empty() => core_verify::setup(),
        "onboarding-verify" if rest.is_empty() => core_verify::onboarding_verify(),
        "architecture-core" if rest.is_empty() => architecture_check::run_core_architecture(root),
        "architecture-check" => architecture_check::run_architecture_check_command(rest, root),
        "contract" => architecture_check::run_contract_check_command(rest, root),
        "bundle-local" => bundle_local::run(rest, root),
        "distribution-pack"
        | "distribution-verify"
        | "distribution-capture-registry"
        | "distribution-freeze-native"
        | "distribution-describe-native-inputs"
        | "distribution-qualify"
        | "distribution-describe-candidate"
        | "distribution-extract-github-builds" => {
            let subcommand = match args[0].as_str() {
                "distribution-pack" => "pack",
                "distribution-freeze-native" => "freeze-native",
                "distribution-describe-native-inputs" => "describe-native-inputs",
                "distribution-qualify" => "qualify",
                "distribution-describe-candidate" => "describe-candidate",
                "distribution-extract-github-builds" => "extract-github-builds",
                "distribution-capture-registry" => "capture-registry",
                _ => "verify",
            };
            std::process::Command::new("cargo")
                .args([
                    "run",
                    "--locked",
                    "-p",
                    "tradeassembly-distribution",
                    "--",
                    subcommand,
                ])
                .args(rest)
                .status()
                .map(|status| status.code().unwrap_or(1))
                .unwrap_or(1)
        }
        "check-whitelist" => check_whitelist::run_check_whitelist(rest, root),
        "plugin-contract" => plugin_contract::run_plugin_contract(rest, root),
        "scan-public" => public_scan::run_public_scan(rest, root),
        "foss-core-boundary" => foss_core_boundary::run(rest, root),
        "help" | "--help" => {
            println!("Core tasks: setup, verify, onboarding-verify, architecture-core, check-whitelist, plugin-contract, scan-public, foss-core-boundary, bundle-local, distribution-pack, distribution-describe-native-inputs, distribution-freeze-native, distribution-describe-candidate, distribution-extract-github-builds, distribution-qualify, distribution-verify, distribution-capture-registry");
            0
        }
        _ => {
            eprintln!("Unknown Core task or unsupported arguments");
            2
        }
    };
    std::process::exit(status);
}
