//! Bootstrap checksum-pinned, released consumer sources with reviewed consumer patches.
//! No credentials; network only with explicit --fetch. Never overwrite existing sources.
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
const CRATES: &[(&str, &str)] = &[(
    "openidconnect-4.0.1",
    "0d8c6709ba2ea764bbed26bce1adf3c10517113ddea6f2d4196e4851757ef2b2",
)];
const PATCH_SHA256: &str = "97c1d611054d8e0ba5823f4f2c55effd6dc8f923aa3e90e2c166533439f12eab";

fn run(program: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    let result = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|_| format!("cannot execute {program}"))?;
    if !result.status.success() {
        return Err(format!("{program} failed; preparation directory retained"));
    }
    Ok(())
}
fn checksum(path: &Path) -> Result<String, String> {
    if !path
        .symlink_metadata()
        .map_err(|_| "missing archive")?
        .file_type()
        .is_file()
    {
        return Err("archive must be a regular file".into());
    }
    let result = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .map_err(|_| "checksum tool unavailable")?;
    let digest = String::from_utf8(result.stdout).map_err(|_| "invalid checksum output")?;
    if !result.status.success() {
        return Err("checksum failed".into());
    }
    Ok(digest
        .split_whitespace()
        .next()
        .ok_or("checksum missing")?
        .into())
}
fn files(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(root.join(relative)).map_err(|_| "cannot inspect sources")? {
        let entry = entry.map_err(|_| "cannot inspect entry")?;
        let kind = entry.file_type().map_err(|_| "cannot inspect file type")?;
        let path = relative.join(entry.file_name());
        if kind.is_dir() {
            files(root, &path, out)?;
        } else if kind.is_file() {
            if !matches!(
                entry.file_name().to_str(),
                Some("Cargo.lock" | ".cargo-ok" | ".cargo-checksum.json")
            ) {
                out.push(path);
            }
        } else {
            return Err("source symlink or special file rejected".into());
        }
    }
    out.sort();
    Ok(())
}
fn same_sources(expected: &Path, actual: &Path) -> Result<(), String> {
    if !actual
        .symlink_metadata()
        .map_err(|_| "missing source directory")?
        .file_type()
        .is_dir()
    {
        return Err("source directory must not be a symlink".into());
    }
    let (mut left, mut right) = (Vec::new(), Vec::new());
    files(expected, Path::new(""), &mut left)?;
    files(actual, Path::new(""), &mut right)?;
    if left != right {
        return Err("existing source inventory differs; refusing overwrite".into());
    }
    for path in left {
        if fs::read(expected.join(&path)).map_err(|_| "cannot read expected source")?
            != fs::read(actual.join(path)).map_err(|_| "cannot read actual source")?
        {
            return Err("existing source changed; refusing overwrite".into());
        }
    }
    Ok(())
}
fn prepare() -> Result<(), String> {
    let args: Vec<_> = env::args().collect();
    if !(args.len() == 3 || args.len() == 4 && args[3] == "--fetch") {
        return Err("usage: prepare-crypto-consumers CACHE OUTPUT [--fetch]".into());
    }
    let fetch = args.len() == 4;
    let root = env::current_dir().map_err(|_| "working directory unavailable")?;
    let cache = root.join(&args[1]);
    if fetch {
        fs::create_dir_all(&cache).map_err(|_| "cannot create cache")?;
    }
    let cache = fs::canonicalize(cache).map_err(|_| "registry cache unavailable")?;
    let out = root.join(&args[2]);
    for (name, expected) in CRATES {
        let archive = cache.join(format!("{name}.crate"));
        if archive.symlink_metadata().is_err() && fetch {
            let (crate_name, version) = name.rsplit_once('-').ok_or("invalid pinned name")?;
            let url = format!(
                "https://static.crates.io/crates/{crate_name}/{crate_name}-{version}.crate"
            );
            let download = cache.join(format!("{name}.download-{}", std::process::id()));
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&download)
                .map_err(|_| "download path exists")?;
            run(
                "curl",
                &[
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--proto",
                    "=https",
                    "--max-time",
                    "30",
                    "--output",
                    download.to_str().ok_or("invalid download path")?,
                    &url,
                ],
                &root,
            )?;
            if checksum(&download)? != *expected {
                return Err(format!("download checksum mismatch: {name}"));
            }
            // Hard link is atomic and never replaces an existing archive.
            fs::hard_link(&download, &archive)
                .map_err(|_| "cache destination appeared; refusing overwrite")?;
            fs::remove_file(download).map_err(|_| "temporary download cleanup failed")?;
        }
        if checksum(&archive)? != *expected {
            return Err(format!("registry checksum mismatch: {name}"));
        }
    }
    let stage = out.with_extension(format!("prepare-{}", std::process::id()));
    fs::create_dir(&stage).map_err(|_| "fresh staging directory unavailable")?;
    for (name, _) in CRATES {
        let archive = cache.join(format!("{name}.crate"));
        let patch = root.join(format!("patches/crypto-consumers/{name}.patch"));
        if checksum(&patch)? != PATCH_SHA256 {
            return Err("OIDC patch checksum mismatch".into());
        }
        run(
            "tar",
            &["-xf", archive.to_str().ok_or("invalid archive path")?],
            &stage,
        )?;
        run(
            "git",
            &[
                "apply",
                "--unidiff-zero",
                "--check",
                patch.to_str().ok_or("invalid patch path")?,
            ],
            &stage,
        )?;
        run(
            "git",
            &[
                "apply",
                "--unidiff-zero",
                patch.to_str().ok_or("invalid patch path")?,
            ],
            &stage,
        )?;
    }
    if out.symlink_metadata().is_ok() {
        if !out
            .symlink_metadata()
            .map_err(|_| "output unavailable")?
            .file_type()
            .is_dir()
        {
            return Err("output is not a directory".into());
        }
        for (name, _) in CRATES {
            same_sources(&stage.join(name), &out.join(name))?;
        }
        fs::remove_dir_all(stage).map_err(|_| "temporary staging cleanup failed")?;
    } else {
        fs::rename(stage, out).map_err(|_| "cannot install prepared sources")?;
    }
    println!("verified 1 pinned consumer source and reviewed consumer patches");
    Ok(())
}
fn main() {
    if let Err(error) = prepare() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "ta-consumer-source-test-{}-{nanos}-{}",
                std::process::id(),
                FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir(&path).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn changed_source_is_rejected_without_overwrite() {
        let f = Fixture::new();
        let a = f.dir("expected");
        let b = f.dir("actual");
        fs::write(a.join("Cargo.toml"), b"original").unwrap();
        fs::write(b.join("Cargo.toml"), b"original").unwrap();
        assert!(same_sources(&a, &b).is_ok());
        fs::write(b.join("Cargo.toml"), b"changed").unwrap();
        assert!(same_sources(&a, &b).is_err());
        assert_eq!(fs::read(b.join("Cargo.toml")).unwrap(), b"changed");
    }
    #[test]
    fn extra_source_rejected_generated_lock_allowed() {
        let f = Fixture::new();
        let a = f.dir("expected");
        let b = f.dir("actual");
        fs::write(b.join("Cargo.lock"), b"test-only lock").unwrap();
        assert!(same_sources(&a, &b).is_ok());
        fs::write(b.join("build.rs"), b"unexpected").unwrap();
        assert!(same_sources(&a, &b).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn source_symlink_is_rejected() {
        let f = Fixture::new();
        let a = f.dir("expected");
        let b = f.dir("actual");
        fs::write(a.join("lib.rs"), b"source").unwrap();
        std::os::unix::fs::symlink(a.join("lib.rs"), b.join("lib.rs")).unwrap();
        assert!(same_sources(&a, &b).is_err());
        assert!(checksum(&b.join("lib.rs")).is_err());
    }
    #[test]
    fn checksum_uses_standard_sha256() {
        let f = Fixture::new();
        let p = f.0.join("archive");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(
            checksum(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
