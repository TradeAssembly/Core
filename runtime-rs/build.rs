use std::{env, path::Path, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=TRADEASSEMBLY_CORE_REVISION");
    println!("cargo:rerun-if-changed=../.git/HEAD");

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
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}
