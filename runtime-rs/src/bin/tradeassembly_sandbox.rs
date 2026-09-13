//! Launch the bundled Sandbox Runtime without consulting the host Node install.
//!
//! The release bundle is deliberately self-contained.  Keep this launcher small:
//! it resolves the two files from its own bundle, scrubs Node/dynamic-loader
//! injection variables, and replaces itself with the bundled Node process.

#[cfg(any(target_os = "macos", test))]
use std::ffi::{OsStr, OsString};
#[cfg(any(target_os = "macos", test))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", test))]
use std::process::Command;
use std::process::ExitCode;

#[cfg(any(target_os = "macos", test))]
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
#[cfg(any(target_os = "macos", test))]
const NODE_RELATIVE: &str = "runtime/node/bin/node";
#[cfg(any(target_os = "macos", test))]
const CLI_RELATIVE: &str = "runtime/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js";

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct BundlePaths {
    node: PathBuf,
    cli: PathBuf,
}

#[cfg(any(target_os = "macos", test))]
fn resolve_bundle_paths(root: &Path) -> Result<BundlePaths, ()> {
    let root = root.canonicalize().map_err(|_| ())?;
    let node = contained_file(&root, &root.join(NODE_RELATIVE))?;
    let cli = contained_file(&root, &root.join(CLI_RELATIVE))?;
    Ok(BundlePaths { node, cli })
}

#[cfg(any(target_os = "macos", test))]
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

#[cfg(any(target_os = "macos", test))]
fn bundle_root(executable: &Path) -> Result<PathBuf, ()> {
    let executable = executable.canonicalize().map_err(|_| ())?;
    let bin = executable.parent().ok_or(())?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err(());
    }
    bin.parent().map(Path::to_path_buf).ok_or(())
}

#[cfg(any(target_os = "macos", test))]
fn forwarded_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    args.into_iter().collect()
}

#[cfg(any(target_os = "macos", test))]
fn scrubbed_environment(command: &mut Command) {
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
    command.env("PATH", SYSTEM_PATH);
}

#[cfg(target_os = "macos")]
fn exec_bundled(args: Vec<OsString>) -> Result<(), ()> {
    use std::os::unix::process::CommandExt;

    let executable = std::env::current_exe().map_err(|_| ())?;
    let root = bundle_root(&executable)?;
    let paths = resolve_bundle_paths(&root)?;
    let mut command = Command::new(paths.node);
    command.arg(paths.cli);
    command.args(forwarded_args(args.into_iter().skip(1)));
    scrubbed_environment(&mut command);
    let error = command.exec();
    let _ = error;
    Err(())
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    if exec_bundled(std::env::args_os().collect()).is_err() {
        eprintln!("tradeassembly-sandbox: bundled runtime unavailable");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(not(target_os = "macos"))]
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
}
