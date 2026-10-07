use serde_json::Value;
use std::process::Command;
#[test]
fn compatibility_query_is_read_only_and_identity_shape_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_tradeassembly-core-runner");
    let query = |flag| {
        let out = Command::new(bin)
            .arg(flag)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let identity = query("--build-identity");
    assert_eq!(identity.as_object().unwrap().len(), 2);
    let descriptor = query("--compatibility");
    assert_eq!(descriptor["component"], "core");
    assert!(descriptor["provides"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "core.runner.paper"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(!Command::new(bin)
        .args(["--compatibility", "--db", "ignored"])
        .status()
        .unwrap()
        .success());
}
