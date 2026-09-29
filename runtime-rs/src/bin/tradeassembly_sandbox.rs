//! Launch the bundled Sandbox Runtime without consulting the host Node install.
//!
//! The release bundle is deliberately self-contained.  Keep this launcher small:
//! it resolves the two files from its own bundle, scrubs Node/dynamic-loader
//! injection variables, and replaces itself with the bundled Node process.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::ExitCode;

#[cfg(not(target_os = "windows"))]
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
#[cfg(not(target_os = "windows"))]
const NODE_RELATIVE: &str = "runtime/node/bin/node";
#[cfg(target_os = "windows")]
const NODE_RELATIVE: &str = "runtime/node/bin/node.exe";
const CLI_RELATIVE: &str = "runtime/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js";

#[derive(Debug, Clone, PartialEq, Eq)]
struct BundlePaths {
    node: PathBuf,
    cli: PathBuf,
}

fn resolve_bundle_paths(root: &Path) -> Result<BundlePaths, ()> {
    let root = root.canonicalize().map_err(|_| ())?;
    let node = contained_file(&root, &root.join(NODE_RELATIVE))?;
    let cli = contained_file(&root, &root.join(CLI_RELATIVE))?;
    Ok(BundlePaths { node, cli })
}

fn contained_file(root: &Path, candidate: &Path) -> Result<PathBuf, ()> {
    let resolved = candidate.canonicalize().map_err(|_| ())?;
    if !resolved.starts_with(root) {
        return Err(());
    }
    if !resolved.is_file() {
        return Err(());
    }
    Ok(resolved)
}

fn bundle_root(executable: &Path) -> Result<PathBuf, ()> {
    let executable = executable.canonicalize().map_err(|_| ())?;
    let bin = executable.parent().ok_or(())?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err(());
    }
    bin.parent().map(Path::to_path_buf).ok_or(())
}

fn forwarded_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    args.into_iter().collect()
}

fn scrubbed_environment(command: &mut Command) -> Result<(), ()> {
    command.env_clear();
    for name in [
        "HOME",
        "TMPDIR",
        "SSL_CERT",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(not(target_os = "windows"))]
    command.env("PATH", SYSTEM_PATH);
    #[cfg(target_os = "windows")]
    {
        // SRT's dedicated-account broker needs Windows system/profile paths,
        // not the host PATH or arbitrary Node/process-injection variables.
        let system = PathBuf::from(std::env::var_os("SystemRoot").ok_or(())?);
        if !system.is_absolute() || !system.join("System32/cmd.exe").is_file() {
            return Err(());
        }
        for name in [
            "SystemRoot",
            "WINDIR",
            "ProgramData",
            "LOCALAPPDATA",
            "APPDATA",
            "USERPROFILE",
            "TEMP",
            "TMP",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env("ComSpec", system.join("System32/cmd.exe"));
        command.env(
            "PATH",
            std::env::join_paths([
                system.join("System32"),
                system.clone(),
                system.join("System32/Wbem"),
                system.join("System32/WindowsPowerShell/v1.0"),
            ])
            .map_err(|_| ())?,
        );
    }
    Ok(())
}

fn bundled_command(args: Vec<OsString>) -> Result<Command, ()> {
    let executable = std::env::current_exe().map_err(|_| ())?;
    let root = bundle_root(&executable)?;
    let paths = resolve_bundle_paths(&root)?;
    let mut command = Command::new(paths.node);
    command.arg(paths.cli);
    command.args(forwarded_args(args.into_iter().skip(1)));
    scrubbed_environment(&mut command)?;
    Ok(command)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn exec_bundled(args: Vec<OsString>) -> Result<(), ()> {
    use std::os::unix::process::CommandExt;
    let mut command = bundled_command(args)?;
    let error = command.exec();
    let _ = error;
    Err(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() -> ExitCode {
    if exec_bundled(std::env::args_os().collect()).is_err() {
        eprintln!("tradeassembly-sandbox: bundled runtime unavailable");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    match bundled_command(std::env::args_os().collect())
        .and_then(|mut command| command.status().map_err(|_| ()))
    {
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1),
        ),
        Err(()) => {
            eprintln!("tradeassembly-sandbox: bundled runtime unavailable");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn main() -> ExitCode {
    eprintln!("tradeassembly-sandbox: unsupported runtime platform");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_bundle() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().expect("temporary bundle");
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("bin")).expect("bin");
        fs::create_dir_all(root.join("runtime/node/bin")).expect("node");
        fs::create_dir_all(root.join("runtime/node_modules/@anthropic-ai/sandbox-runtime/dist"))
            .expect("cli");
        fs::write(root.join("bin/tradeassembly-sandbox"), b"launcher").expect("launcher");
        fs::write(root.join(NODE_RELATIVE), b"node").expect("node file");
        fs::write(root.join(CLI_RELATIVE), b"cli").expect("cli file");
        (dir, root)
    }

    #[test]
    fn bundle_paths_resolve_from_launcher_root() {
        let (_dir, root) = make_bundle();
        let paths = resolve_bundle_paths(&root).expect("valid bundle");
        assert_eq!(paths.node, root.join(NODE_RELATIVE).canonicalize().unwrap());
        assert_eq!(paths.cli, root.join(CLI_RELATIVE).canonicalize().unwrap());
    }

    #[test]
    fn missing_dependency_fails_closed() {
        let (dir, root) = make_bundle();
        fs::remove_file(root.join(CLI_RELATIVE)).expect("remove cli");
        assert!(resolve_bundle_paths(&root).is_err());
        drop(dir);
    }

    #[cfg(unix)]
    #[test]
    fn dependency_symlink_escape_fails_closed() {
        let (_dir, root) = make_bundle();
        let outside = tempdir().expect("outside");
        let outside_cli = outside.path().join("cli.js");
        fs::write(&outside_cli, b"cli").expect("outside cli");
        fs::remove_file(root.join(CLI_RELATIVE)).expect("remove cli");
        std::os::unix::fs::symlink(&outside_cli, root.join(CLI_RELATIVE)).expect("symlink");
        assert!(resolve_bundle_paths(&root).is_err());
    }

    #[test]
    fn arguments_preserve_spaces_and_metacharacters() {
        let args = vec![
            OsString::from("--label"),
            OsString::from("two words; $HOME && echo unsafe"),
            OsString::from("quotes='\" and \\slashes"),
        ];
        assert_eq!(forwarded_args(args.clone()), args);
    }

    #[test]
    fn launcher_drops_injection_and_credential_environment() {
        let mut command = Command::new("not-executed");
        command.env("NODE_OPTIONS", "fixture-injection");
        command.env("AWS_SECRET_ACCESS_KEY", "fixture-not-a-secret");
        command.env("PATH", "fixture-untrusted-path");
        scrubbed_environment(&mut command).expect("native system environment");
        let names: Vec<_> = command.get_envs().map(|(name, _)| name).collect();
        assert!(!names.contains(&OsStr::new("NODE_OPTIONS")));
        assert!(!names.contains(&OsStr::new("AWS_SECRET_ACCESS_KEY")));
        assert!(command
            .get_envs()
            .any(|(name, value)| name == "PATH"
                && value != Some(OsStr::new("fixture-untrusted-path"))));
    }
}
