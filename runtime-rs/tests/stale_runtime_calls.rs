use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn tracked_sources_do_not_call_stale_python_or_make_runtime_targets() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let fixture = stale_call_fixture();
    let mut offenders = Vec::new();
    visit(&root, &mut |path| {
        if should_skip(path, &fixture.skip_path_contains) || !is_text_surface(path) {
            return;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        for needle in &fixture.needles {
            if text.contains(needle) {
                offenders.push(format!("{} contains {needle}", path.display()));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "stale runtime calls:\n{}",
        offenders.join("\n")
    );
}

fn visit(path: &Path, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink()
            || path.file_name().and_then(|value| value.to_str()) == Some(".worktrees")
        {
            continue;
        }
        if metadata.is_dir() {
            visit(&path, f);
        } else {
            f(&path);
        }
    }
}

fn should_skip(path: &Path, skip_path_contains: &[String]) -> bool {
    let text = path.to_string_lossy();
    skip_path_contains
        .iter()
        .any(|needle| text.contains(needle))
}

fn is_text_surface(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("md")
            | Some("rs")
            | Some("ts")
            | Some("tsx")
            | Some("toml")
            | Some("yaml")
            | Some("yml")
            | Some("json")
            | Some("sh")
    ) || path.file_name().and_then(|value| value.to_str()) == Some("Makefile")
        || path.file_name().and_then(|value| value.to_str()) == Some("Justfile")
        || path.file_name().and_then(|value| value.to_str()) == Some("Dockerfile")
}

#[derive(serde::Deserialize)]
struct StaleCallFixture {
    needles: Vec<String>,
    skip_path_contains: Vec<String>,
}

fn stale_call_fixture() -> StaleCallFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("parity")
        .join("stale_runtime_calls.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("read stale-call fixture {}: {error}", path.display());
    });
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("parse stale-call fixture {}: {error}", path.display());
    })
}
