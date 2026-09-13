// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use std::{fs, path::Path, process::Command};
use tradeassembly_runtime::service::TradeAssemblyService;

fn fixture(path: &Path) -> String {
    let db = path.to_string_lossy().into_owned();
    drop(TradeAssemblyService::test_local(&db));
    db
}

#[test]
fn handoff_requires_registered_owner_and_never_overwrites_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let foreign = dir.path().join("foreign.db");
    fs::write(&foreign, b"unregistered fixture sentinel").unwrap();
    assert!(TradeAssemblyService::test_local_handoff(foreign.to_string_lossy()).is_err());
    assert_eq!(
        fs::read(&foreign).unwrap(),
        b"unregistered fixture sentinel"
    );
    let db = fixture(&dir.path().join("owned.db"));
    let token = TradeAssemblyService::test_local_handoff(&db).unwrap();
    let receipt = fs::read(format!("{db}.handoff")).unwrap();
    assert!(TradeAssemblyService::test_local_handoff(&db).is_err());
    assert_eq!(fs::read(format!("{db}.handoff")).unwrap(), receipt);
    assert!(TradeAssemblyService::test_local_with_handoff(&db, token).is_ok());
}

#[test]
fn handoff_rejects_wrong_token_path_and_replaced_database_before_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(&dir.path().join("owned.db"));
    let token = TradeAssemblyService::test_local_handoff(&db).unwrap();
    let before = fs::read(&db).unwrap();
    assert!(TradeAssemblyService::test_local_with_handoff(&db, "wrong-token").is_err());
    assert_eq!(fs::read(&db).unwrap(), before);
    // Same inode and same receipt still cannot authorize a different path.
    let alias = dir.path().join("alias.db");
    fs::hard_link(&db, &alias).unwrap();
    fs::copy(
        format!("{db}.handoff"),
        format!("{}.handoff", alias.display()),
    )
    .unwrap();
    assert!(
        TradeAssemblyService::test_local_with_handoff(alias.to_string_lossy(), &token).is_err()
    );
    fs::rename(&db, dir.path().join("original.db")).unwrap();
    fs::write(&db, b"replacement sentinel").unwrap();
    assert!(TradeAssemblyService::test_local_with_handoff(&db, &token).is_err());
    assert_eq!(fs::read(&db).unwrap(), b"replacement sentinel");
}

#[cfg(unix)]
#[test]
fn handoff_rejects_nonprivate_and_symlinked_receipts() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(&dir.path().join("owned.db"));
    let token = TradeAssemblyService::test_local_handoff(&db).unwrap();
    let receipt = format!("{db}.handoff");
    assert_eq!(
        fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(TradeAssemblyService::test_local_with_handoff(&db, &token).is_err());
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
    let moved = dir.path().join("receipt.json");
    fs::rename(&receipt, &moved).unwrap();
    symlink(&moved, &receipt).unwrap();
    assert!(TradeAssemblyService::test_local_with_handoff(&db, &token).is_err());
    assert!(TradeAssemblyService::test_local_handoff(&db).is_err());
}

#[test]
fn ordinary_child_factory_does_not_infer_permission_from_handoff_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(&dir.path().join("owned.db"));
    let _token = TradeAssemblyService::test_local_handoff(&db).unwrap();
    let before = fs::read(&db).unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "ordinary_factory_child_probe"])
        .env("TRADEASSEMBLY_FIXTURE_GUARD_PROBE_DB", &db)
        .output()
        .unwrap();
    assert!(result.status.success(), "child ownership guard failed");
    assert_eq!(fs::read(&db).unwrap(), before);
}

#[test]
fn ordinary_factory_child_probe() {
    let Ok(db) = std::env::var("TRADEASSEMBLY_FIXTURE_GUARD_PROBE_DB") else {
        return;
    };
    assert!(std::panic::catch_unwind(|| TradeAssemblyService::test_local(db)).is_err());
}
