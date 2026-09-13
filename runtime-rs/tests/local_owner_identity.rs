use std::fs;
use std::sync::Arc;
use std::thread;
use tempfile::tempdir;
use tradeassembly_runtime::local_owner_identity::LocalOwnerIdentity;

fn private_root(directory: &tempfile::TempDir) -> std::path::PathBuf {
    let path = directory.path().join("state");
    fs::create_dir(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
    }
    path.canonicalize().unwrap()
}

#[test]
fn identity_is_stable_across_restart_and_distinct_between_installs() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let first_root = private_root(&first);
    let second_root = private_root(&second);
    let a = LocalOwnerIdentity::load_or_create(&first_root).unwrap();
    let reopened = LocalOwnerIdentity::load_or_create(&first_root).unwrap();
    let b = LocalOwnerIdentity::load_or_create(&second_root).unwrap();

    assert_eq!(a, reopened);
    assert_ne!(a.subject, b.subject);
    assert_eq!(a.issuer, "local-owner");
    assert_eq!(a.audience, ["tradeassembly-local"]);
}

#[test]
fn concurrent_initialization_converges_on_one_identity() {
    let directory = Arc::new(tempdir().unwrap());
    let state_root = Arc::new(private_root(&directory));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let state_root = Arc::clone(&state_root);
        workers.push(thread::spawn(move || {
            LocalOwnerIdentity::load_or_create(state_root.as_path()).unwrap()
        }));
    }
    let identities = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert!(identities.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn malformed_oversized_and_nonregular_state_fail_closed() {
    let directory = tempdir().unwrap();
    let state_root = private_root(&directory);
    let state = state_root.join("local-owner.json");
    fs::write(&state, br#"{"version":1,"ownerId":"forged"}"#).unwrap();
    assert!(LocalOwnerIdentity::load_or_create(&state_root).is_err());

    fs::write(&state, vec![b'x'; 1025]).unwrap();
    assert_eq!(
        LocalOwnerIdentity::load_or_create(&state_root).unwrap_err(),
        "local_owner_state_oversized"
    );

    fs::remove_file(&state).unwrap();
    fs::create_dir(&state).unwrap();
    assert_eq!(
        LocalOwnerIdentity::load_or_create(&state_root).unwrap_err(),
        "local_owner_state_file_invalid"
    );
}

#[test]
fn stale_temporary_state_does_not_block_initialization() {
    let directory = tempdir().unwrap();
    let state_root = private_root(&directory);
    fs::write(state_root.join(".local-owner.stale.tmp"), b"partial").unwrap();
    assert!(LocalOwnerIdentity::load_or_create(&state_root).is_ok());
}

#[test]
fn database_derivation_rejects_memory_and_appends_state_directory() {
    let directory = tempdir().unwrap();
    let database = directory.path().canonicalize().unwrap().join("runtime.db");
    let identity = LocalOwnerIdentity::for_database(&database).unwrap();
    assert!(directory
        .path()
        .join("runtime.db.local-owner/local-owner.json")
        .exists());
    assert_eq!(
        identity,
        LocalOwnerIdentity::for_database(&database).unwrap()
    );
    assert_eq!(
        LocalOwnerIdentity::for_database(":memory:").unwrap_err(),
        "local_owner_database_path_invalid"
    );
}

#[cfg(unix)]
#[test]
fn insecure_state_permissions_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempdir().unwrap();
    let state_root = private_root(&directory);
    let identity = LocalOwnerIdentity::load_or_create(&state_root).unwrap();
    let state = state_root.join("local-owner.json");
    let mut permissions = fs::metadata(&state).unwrap().permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&state, permissions).unwrap();
    assert_eq!(
        LocalOwnerIdentity::load_or_create(&state_root).unwrap_err(),
        "local_owner_state_file_insecure"
    );
    assert!(!identity.subject.is_empty());
}

#[cfg(unix)]
#[test]
fn insecure_state_directory_permissions_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempdir().unwrap();
    let state_dir = directory.path().join("state");
    fs::create_dir(&state_dir).unwrap();
    let state_dir = state_dir.canonicalize().unwrap();
    let mut permissions = fs::metadata(&state_dir).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&state_dir, permissions).unwrap();
    assert_eq!(
        LocalOwnerIdentity::load_or_create(&state_dir).unwrap_err(),
        "local_owner_state_directory_insecure"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_state_parent_is_rejected_before_creation() {
    use std::os::unix::fs::symlink;
    let directory = tempdir().unwrap();
    let target = private_root(&directory);
    let link = directory.path().join("link");
    symlink(&target, &link).unwrap();
    let state = link.join("nested");
    assert_eq!(
        LocalOwnerIdentity::load_or_create(&state).unwrap_err(),
        "local_owner_state_directory_invalid"
    );
    assert!(!target.join("nested").exists());
}
