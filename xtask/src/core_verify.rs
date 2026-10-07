//! Bounded verification and setup gates for the extracted public Core.
//!
//! The command lists in this module are deliberately fixed.  Keeping the
//! registry here makes the gate auditable and prevents `verify` or `setup`
//! from silently growing webapp, service, or plugin-activation side effects.

use std::{path::Path, process::Command};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Check {
    program: &'static str,
    args: &'static [&'static str],
}

const VERIFY_CHECKS: &[Check] = &[
    Check {
        program: "cargo",
        args: &["fmt", "--check"],
    },
    Check {
        program: "cargo",
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    },
    Check {
        program: "npm",
        args: &[
            "ci",
            "--prefix",
            "packaging/sandbox",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ],
    },
    Check {
        program: "cargo",
        args: &["test", "--workspace", "--doc", "--locked"],
    },
    Check {
        program: "cargo",
        args: &["nextest", "run", "--workspace", "--locked"],
    },
    Check {
        program: "cargo",
        args: &["deny", "check"],
    },
    Check {
        program: "cargo",
        args: &["audit", "--ignore", "RUSTSEC-2023-0071"],
    },
    Check {
        program: "cargo-machete",
        args: &["."],
    },
    Check {
        program: "cargo",
        args: &["xtask", "check-whitelist", "--repo", "."],
    },
    Check {
        program: "cargo",
        args: &["xtask", "plugin-contract"],
    },
    Check {
        program: "cargo",
        args: &["xtask", "architecture-core"],
    },
    Check {
        program: "cargo",
        args: &["xtask", "scan-public"],
    },
    Check {
        program: "cargo",
        args: &["xtask", "foss-core-boundary"],
    },
];

const SETUP_CHECKS: &[Check] = &[
    Check {
        program: "cargo",
        args: &["fetch", "--locked"],
    },
    Check {
        program: "npm",
        args: &[
            "ci",
            "--prefix",
            "packaging/sandbox",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ],
    },
];

/// Fast component feedback; does not claim release qualification.
pub(crate) fn check() -> i32 {
    run_checks(
        &[
            Check {
                program: "cargo",
                args: &["fmt", "--check"],
            },
            Check {
                program: "cargo",
                args: &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            },
            Check {
                program: "cargo",
                args: &["test", "--workspace", "--lib", "--locked"],
            },
            Check {
                program: "cargo",
                args: &["xtask", "architecture-core"],
            },
        ],
        run_command,
    )
}

/// Build native local executables. Distribution qualification remains separate.
pub(crate) fn build() -> i32 {
    run_checks(
        &[Check {
            program: "cargo",
            args: &["build", "--workspace", "--bins", "--locked"],
        }],
        run_command,
    )
}

/// Run the complete, ordered Core verification gate.
pub(crate) fn verify() -> i32 {
    verify_mode(false)
}

/// Full coverage on a dirty development checkout, never release qualification.
pub(crate) fn verify_dev() -> i32 {
    verify_mode(true)
}

fn verify_mode(development: bool) -> i32 {
    if let Err(reason) = prerequisites(Path::new("."), development) {
        eprintln!("Core verification preflight: {reason}");
        return 1;
    }
    let mut checks = VERIFY_CHECKS.to_vec();
    if development {
        let whitelist = checks
            .iter_mut()
            .find(|check| {
                check.args.first() == Some(&"xtask")
                    && check.args.get(1) == Some(&"check-whitelist")
            })
            .expect("fixed whitelist check");
        whitelist.args = &["xtask", "check-whitelist", "--repo", ".", "--allow-dirty"];
    }
    run_checks(&checks, run_command)
}

fn prerequisites(root: &Path, development: bool) -> Result<(), String> {
    for input in [
        "Cargo.toml",
        "Cargo.lock",
        "packaging/sandbox/package-lock.json",
        "docs/rust-source-whitelist.yaml",
        "foss-core-publication.yaml",
    ] {
        if !root.join(input).is_file() {
            return Err(format!("required input missing: {input}"));
        }
    }
    for (program, args) in [
        ("git", vec!["rev-parse", "--verify", "HEAD"]),
        ("cargo", vec!["--version"]),
        ("npm", vec!["--version"]),
        ("cargo", vec!["nextest", "--version"]),
        ("cargo", vec!["deny", "--version"]),
        ("cargo", vec!["audit", "--version"]),
        ("cargo-machete", vec!["--version"]),
    ] {
        let output = Command::new(program)
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|_| format!("required tool unavailable: {program}"))?;
        if !output.status.success() {
            return Err(format!(
                "required tool or committed revision unavailable: {program}"
            ));
        }
    }
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(root)
        .output()
        .map_err(|_| "git status unavailable")?;
    if !status.status.success() {
        return Err("git status failed".into());
    }
    require_release_clean(&status.stdout, development)?;
    #[cfg(unix)]
    {
        // A warm low-debug owning gate completed with about 3 GiB available.
        // This 2 GiB reserve catches known disk pressure; it is not a cold-build guarantee.
        let output = Command::new("df")
            .args(["-Pk", "."])
            .env("LC_ALL", "C")
            .current_dir(root)
            .output()
            .map_err(|_| "storage headroom unavailable")?;
        if !output.status.success() {
            return Err("storage headroom check failed".into());
        }
        require_storage_headroom(&String::from_utf8_lossy(&output.stdout))?;
    }
    Ok(())
}

fn require_release_clean(status: &[u8], development: bool) -> Result<(), String> {
    if !development && !status.is_empty() {
        return Err("release verification requires clean committed source; use verify --dev for development".into());
    }
    Ok(())
}

#[cfg(unix)]
fn require_storage_headroom(df: &str) -> Result<(), String> {
    let available = df
        .lines()
        .last()
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or("storage headroom invalid")?;
    if available < 2 * 1024 * 1024 {
        return Err(
            "less than 2 GiB free; preserve evidence and resolve storage before building".into(),
        );
    }
    Ok(())
}

/// Prepare the Core workspace and its pinned standalone sandbox prerequisite.
/// This intentionally does not start Warden or install/activate plugins.
pub(crate) fn setup() -> i32 {
    run_checks(SETUP_CHECKS, run_command)
}

/// Local acceptance only. Provider OAuth and paid Relay proof remain release gates.
pub(crate) fn onboarding_verify() -> i32 {
    run_checks(
        &[
            Check {
                program: "cargo",
                args: &[
                    "test",
                    "-p",
                    "tradeassembly-runtime",
                    "--lib",
                    "control_plane::tests::oauth_",
                ],
            },
            Check {
                program: "cargo",
                args: &[
                    "test",
                    "-p",
                    "tradeassembly-runtime",
                    "--lib",
                    "service::plugin_oauth",
                ],
            },
            Check {
                program: "cargo",
                args: &[
                    "test",
                    "-p",
                    "tradeassembly-runtime",
                    "--lib",
                    "connection_profile",
                ],
            },
            Check {
                program: "cargo",
                args: &[
                    "test",
                    "-p",
                    "tradeassembly-runtime",
                    "--lib",
                    "browser_onboarding",
                ],
            },
            Check {
                program: "cargo",
                args: &[
                    "test",
                    "-p",
                    "tradeassembly-runtime",
                    "--lib",
                    "identity_bootstrap",
                ],
            },
            Check {
                program: "cargo",
                args: &[
                    "test",
                    "-p",
                    "tradeassembly-core-binaries",
                    "--test",
                    "browser_onboarding_stdio",
                ],
            },
        ],
        run_command,
    )
}

fn run_checks<F>(checks: &[Check], mut runner: F) -> i32
where
    F: FnMut(&Check) -> Result<i32, i32>,
{
    for check in checks {
        match runner(check) {
            Ok(code) => {
                if code != 0 {
                    return code;
                }
            }
            Err(code) => return code,
        }
    }
    0
}

fn run_command(check: &Check) -> Result<i32, i32> {
    let status = Command::new(check.program)
        .args(check.args)
        .status()
        .map_err(|error| {
            eprintln!("Failed to run {}: {error}", format_command(check));
            1
        })?;
    if status.success() {
        Ok(0)
    } else {
        Err(status.code().unwrap_or(1))
    }
}

fn format_command(check: &Check) -> String {
    std::iter::once(check.program)
        .chain(check.args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{run_checks, Check, SETUP_CHECKS, VERIFY_CHECKS};

    fn command(check: &Check) -> String {
        std::iter::once(check.program)
            .chain(check.args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn verify_registry_contains_exact_ordered_core_checks() {
        let commands: Vec<_> = VERIFY_CHECKS.iter().map(command).collect();
        assert_eq!(
            commands,
            vec![
                "cargo fmt --check",
                "cargo clippy --workspace --all-targets -- -D warnings",
                "npm ci --prefix packaging/sandbox --ignore-scripts --no-audit --no-fund",
                "cargo test --workspace --doc --locked",
                "cargo nextest run --workspace --locked",
                "cargo deny check",
                "cargo audit --ignore RUSTSEC-2023-0071",
                "cargo-machete .",
                "cargo xtask check-whitelist --repo .",
                "cargo xtask plugin-contract",
                "cargo xtask architecture-core",
                "cargo xtask scan-public",
                "cargo xtask foss-core-boundary",
            ]
        );
        assert!(commands.iter().all(|command| !command.contains("webapp")));
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.contains("nextest run"))
                .count(),
            1
        );
        assert!(!commands
            .iter()
            .any(|command| command == "cargo test --workspace"));
    }

    #[test]
    fn dirty_development_is_not_clean_release_qualification() {
        assert!(super::require_release_clean(b" M source.rs\n", false).is_err());
        assert!(super::require_release_clean(b"?? new.rs\n", false).is_err());
        assert!(super::require_release_clean(b" M source.rs\n", true).is_ok());
        assert!(super::require_release_clean(b"", false).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn headroom_fails_closed_before_expensive_checks() {
        assert!(super::require_storage_headroom("Filesystem 1024-blocks Used Available Capacity Mounted\n/dev/test 4000000 1902848 2097152 48% /\n").is_ok());
        assert!(
            super::require_storage_headroom("/dev/test 4000000 1902849 2097151 48% /\n").is_err()
        );
        assert!(super::require_storage_headroom("invalid\n").is_err());
    }

    #[test]
    fn setup_is_locked_and_standalone_without_service_or_plugin_activation() {
        let commands: Vec<_> = SETUP_CHECKS.iter().map(command).collect();
        assert_eq!(
            commands,
            vec![
                "cargo fetch --locked",
                "npm ci --prefix packaging/sandbox --ignore-scripts --no-audit --no-fund",
            ]
        );
        assert!(commands.iter().all(|command| {
            !command.contains("warden")
                && !command.contains("plugin")
                && !command.contains("webapp")
        }));
    }

    #[test]
    fn runner_stops_at_first_failure() {
        let checks = &[
            Check {
                program: "first",
                args: &[],
            },
            Check {
                program: "second",
                args: &[],
            },
            Check {
                program: "third",
                args: &[],
            },
        ];
        let mut seen = Vec::new();
        let result = run_checks(checks, |check| {
            seen.push(check.program);
            if check.program == "second" {
                Err(17)
            } else {
                Ok(0)
            }
        });
        assert_eq!(result, 17);
        assert_eq!(seen, ["first", "second"]);
    }

    #[test]
    fn runner_preserves_nonzero_exit_code() {
        let checks = &[Check {
            program: "failed",
            args: &[],
        }];
        assert_eq!(run_checks(checks, |_| Ok(23)), 23);
        assert_eq!(run_checks(checks, |_| Err(23)), 23);
    }
}
