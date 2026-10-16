// Copyright (c) 2026 OptionLab LLC. All rights reserved.

//! Private local launcher handshake. This is not a workspace export API.
use crate::ports::{PluginOperationRequest, StoragePort};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static FLIGHT: Mutex<()> = Mutex::new(());
const ERROR: &str = "execution_checkpoint_not_acknowledged";

pub(crate) fn barrier(
    root: &Path,
    storage: &dyn StoragePort,
    key: &str,
    request_hash: &str,
    request: &PluginOperationRequest,
) -> Result<(), String> {
    handshake(
        root,
        storage,
        key,
        request_hash,
        request,
        Duration::from_secs(30),
    )
}

fn handshake(
    root: &Path,
    storage: &dyn StoragePort,
    key: &str,
    request_hash: &str,
    request: &PluginOperationRequest,
    timeout: Duration,
) -> Result<(), String> {
    let _flight = FLIGHT.try_lock().map_err(|_| ERROR)?;
    private_dir(root)?;
    let _process_flight = process_flight(root)?;
    let checkpoint_id = format!("checkpoint_{}", digest(key.as_bytes()));
    let directory = root.join(&checkpoint_id);
    // A prior flight is never reused, even if its acknowledgement remains.
    fs::create_dir(&directory).map_err(|_| ERROR)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(|_| ERROR)?;
    }
    private_dir(&directory)?;
    let snapshot = directory.join("runtime.sqlite");
    let snapshot_file = new_private_file(&snapshot)?;
    storage.execution_checkpoint_snapshot(&snapshot)?;
    snapshot_file.sync_all().map_err(|_| ERROR)?;
    let snapshot_sha256 = digest(&read_private(&snapshot, 512 * 1024 * 1024)?);
    let manifest = json!({
        "schemaVersion":"tradeassembly.execution_checkpoint.v1",
        "checkpointId":checkpoint_id, "originalIdempotencyKey":key,
        "requestHash":request_hash, "snapshotSha256":snapshot_sha256,
        "snapshotFile":"runtime.sqlite", "activationId":request.activation_id,
        "tickId":request.evaluation_tick_id
    });
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|_| ERROR)?;
    let request_digest = digest(&manifest_bytes);
    let mut pending = new_private_file(&directory.join("request.pending"))?;
    pending.write_all(&manifest_bytes).map_err(|_| ERROR)?;
    pending.sync_all().map_err(|_| ERROR)?;
    fs::rename(
        directory.join("request.pending"),
        directory.join("request.json"),
    )
    .map_err(|_| ERROR)?;
    File::open(&directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| ERROR)?;
    let start = Instant::now();
    loop {
        let ack_path = directory.join("ack.json");
        if ack_path.symlink_metadata().is_ok() {
            let ack: Value =
                serde_json::from_slice(&read_private(&ack_path, 16 * 1024)?).map_err(|_| ERROR)?;
            if ack
                == json!({"schemaVersion":"tradeassembly.execution_checkpoint_ack.v1",
                "checkpointId":checkpoint_id,"requestDigest":request_digest,
                "snapshotSha256":snapshot_sha256,"status":"durable"})
            {
                return Ok(());
            }
            return Err(ERROR.into());
        }
        if start.elapsed() >= timeout {
            return Err(ERROR.into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
fn private_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ERROR.into());
    }
    // Reject symlinks in every component. The leaf is private and owner-bound.
    let mut cursor = std::path::PathBuf::new();
    for component in path.components() {
        cursor.push(component);
        if fs::symlink_metadata(&cursor)
            .map_err(|_| ERROR)?
            .file_type()
            .is_symlink()
        {
            return Err(ERROR.into());
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| ERROR)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(ERROR.into());
    }
    Ok(())
}
#[cfg(not(unix))]
fn private_dir(_: &Path) -> Result<(), String> {
    Err("execution_checkpoint_platform_unsupported".into())
}

fn new_private_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|_| ERROR.into())
}
fn read_private(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| ERROR)?;
    let metadata = file.metadata().map_err(|_| ERROR)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(ERROR.into());
        }
    }
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(ERROR.into());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    if bytes.len() as u64 > maximum {
        return Err(ERROR.into());
    }
    Ok(bytes)
}

#[cfg(unix)]
fn process_flight(root: &Path) -> Result<File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = root.join(".flight.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|_| ERROR)?;
    read_private(&path, 0)?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| ERROR)?;
    Ok(lock)
}
#[cfg(not(unix))]
fn process_flight(_: &Path) -> Result<File, String> {
    Err(ERROR.into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::adapters::local::sqlite::LocalSqliteStorage;
    use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
    use std::os::unix::fs::{symlink, PermissionsExt};
    fn request() -> PluginOperationRequest {
        serde_json::from_value(json!({"correlationId":"correlation","pluginInstanceRef":"broker","pluginRef":"example.broker",
            "manifestFingerprint":"manifest","operationId":"broker.paper_order_submit","capability":"broker.order_submit.paper",
            "capabilityGraphRevisionId":"graph","capabilityGraphFingerprint":"graph-hash","strategyId":"strategy",
            "strategyVersionId":"version","strategySpecHash":"strategy-hash","activationId":"activation",
            "attemptId":"attempt","evaluationTickId":"tick","mode":"paper","purpose":"paper_trading",
            "accountRef":"account","timeoutMs":1000,"fencingToken":1,"input":{"clientOrderId":"original"}})).unwrap()
    }
    #[test]
    fn execution_checkpoint_frozen_protocol_uses_sorted_compact_json_sha256() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/execution-checkpoint-protocol.v1.json"
        ))
        .unwrap();
        assert_eq!(
            fixture["ack"]["requestDigest"],
            digest(&serde_json::to_vec(&fixture["request"]).unwrap())
        );
        assert_eq!(
            fixture["request"]["checkpointId"],
            format!("checkpoint_{}", digest(b"original"))
        );
    }
    #[test]
    fn execution_checkpoint_handshake_requires_exact_private_ack_and_consistent_snapshot() {
        for scenario in [
            "valid",
            "absent",
            "wrong_digest",
            "failed",
            "symlink",
            "public_ack",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let db = root.join("source.sqlite");
            let storage = LocalSqliteStorage::new(db.to_str().unwrap());
            let context = SideEffectContext::new(
                AuthorityContext::local_cli(),
                IdempotencyKey::new("fixture").unwrap(),
            );
            storage
                .put_json(
                    "broker_order_intents",
                    "original",
                    json!({"admitted":true}),
                    &context,
                )
                .unwrap();
            let worker_root = root.clone();
            let worker = std::thread::spawn(move || {
                let directory = worker_root.join(format!("checkpoint_{}", digest(b"original")));
                let manifest_path = directory.join("request.json");
                let deadline = Instant::now() + Duration::from_secs(2);
                while !manifest_path.exists() {
                    assert!(Instant::now() < deadline, "request not emitted");
                    std::thread::sleep(Duration::from_millis(2));
                }
                let bytes = read_private(&manifest_path, 16384).unwrap();
                let manifest: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(manifest["snapshotFile"], "runtime.sqlite");
                let snapshot = directory.join("runtime.sqlite");
                assert_eq!(
                    manifest["snapshotSha256"],
                    digest(&fs::read(&snapshot).unwrap())
                );
                let conn = rusqlite::Connection::open_with_flags(
                    snapshot,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let admitted:String = conn.query_row("SELECT value_json FROM tradeassembly_kv WHERE namespace='broker_order_intents' AND item_key='original'",[],|row|row.get(0)).unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&admitted).unwrap(),
                    json!({"admitted":true})
                );
                let mut ack = json!({"schemaVersion":"tradeassembly.execution_checkpoint_ack.v1",
                    "requestDigest":digest(&bytes),"snapshotSha256":manifest["snapshotSha256"],"checkpointId":manifest["checkpointId"],"status":"durable"});
                if scenario == "absent" {
                    return;
                }
                if scenario == "wrong_digest" {
                    ack["requestDigest"] = json!("wrong");
                }
                if scenario == "failed" {
                    ack["status"] = json!("failed");
                }
                let pending = directory.join("ack.pending");
                let mut file = new_private_file(&pending).unwrap();
                file.write_all(&serde_json::to_vec(&ack).unwrap()).unwrap();
                file.sync_all().unwrap();
                if scenario == "public_ack" {
                    fs::set_permissions(&pending, fs::Permissions::from_mode(0o644)).unwrap();
                }
                if scenario == "symlink" {
                    symlink(&pending, directory.join("ack.json")).unwrap();
                } else {
                    fs::rename(&pending, directory.join("ack.json")).unwrap();
                }
            });
            let result = handshake(
                &root,
                &storage,
                "original",
                "request-hash",
                &request(),
                Duration::from_millis(200),
            );
            worker.join().unwrap();
            assert_eq!(
                result.is_ok(),
                scenario == "valid",
                "{scenario}: {result:?}"
            );
            // No stale request/ack can release a second dispatch flight.
            assert!(handshake(
                &root,
                &storage,
                "original",
                "request-hash",
                &request(),
                Duration::ZERO
            )
            .is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let storage = LocalSqliteStorage::new(root.join("source.sqlite").to_str().unwrap());
        assert!(handshake(
            &root,
            &storage,
            "original",
            "hash",
            &request(),
            Duration::ZERO
        )
        .is_err());
    }
}
