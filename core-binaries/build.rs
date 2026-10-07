use std::{env, path::Path, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=TRADEASSEMBLY_CORE_REVISION");
    let root = Path::new("..");
    for name in [
        "HEAD".to_owned(),
        git_output(root, &["symbolic-ref", "-q", "HEAD"]).unwrap_or_else(|| "HEAD".into()),
        "packed-refs".to_owned(),
    ] {
        if let Some(path) = git_output(root, &["rev-parse", "--git-path", &name]) {
            let path = Path::new(&path);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }

    let revision = env::var("TRADEASSEMBLY_CORE_REVISION")
        .ok()
        .or_else(|| git_revision(Path::new("..")))
        .expect("TRADEASSEMBLY_CORE_REVISION or a Git checkout is required");
    assert!(
        revision.len() == 40
            && revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "TradeAssembly Core revision must be a full lowercase Git commit"
    );
    println!("cargo:rustc-env=TRADEASSEMBLY_CORE_REVISION={revision}");
}

fn git_revision(root: &Path) -> Option<String> {
    git_output(root, &["rev-parse", "HEAD"])
}
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
