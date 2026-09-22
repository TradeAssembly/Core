//! Bounded verification and setup gates for the extracted public Core.
//!
//! The command lists in this module are deliberately fixed.  Keeping the
//! registry here makes the gate auditable and prevents `verify` or `setup`
//! from silently growing webapp, service, or plugin-activation side effects.

use std::process::Command;

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
        args: &["test", "--workspace"],
    },
    Check {
        program: "cargo",
        args: &["nextest", "run", "--workspace"],
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
        args: &["build", "--workspace", "--locked"],
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

/// Run the complete, ordered Core verification gate.
pub(crate) fn verify() -> i32 {
    run_checks(VERIFY_CHECKS, run_command)
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
                    "tradeassembly-runtime",
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
                "cargo test --workspace",
                "cargo nextest run --workspace",
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
    }

    #[test]
    fn setup_is_locked_and_standalone_without_service_or_plugin_activation() {
        let commands: Vec<_> = SETUP_CHECKS.iter().map(command).collect();
        assert_eq!(
            commands,
            vec![
                "cargo build --workspace --locked",
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
