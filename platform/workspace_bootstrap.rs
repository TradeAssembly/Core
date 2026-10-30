//! Installation evidence and read-only package selection under the existing
//! local-owner filesystem trust root. Evidence creation never migrates old data.
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const ERROR: &str = "workspace_bootstrap_invalid";
const LIMIT: u64 = 1024 * 1024;
const NAMESPACES: [&str; 7] = [
    "control_plane_commands",
    "finance_action_descriptors",
    "finance_receipts",
    "plugin_manifest_revisions",
    "plugin_manifests",
    "warden_decisions",
    "warden_downstream_outcomes",
];
const SCOPES: [(&str, &str); 8] = [
    ("strategy", "strat_local_btc_demo"),
    ("strategy", "strat_local_crypto_replay"),
    ("plugin_instance", "sim"),
    ("plugin_instance", "local-data"),
    ("plugin_instance", "alpaca-paper"),
    ("credential_reference", "sim"),
    ("credential_reference", "local-data"),
    ("credential_reference", "alpaca-paper"),
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Row {
    pub namespace: String,
    pub key: String,
    pub value: Value,
    pub idempotency_key: String,
    pub authority_actor: String,
    pub updated_at_ms: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Material {
    schema: String,
    database: PathBuf,
    owner: Value,
    payload: PathBuf,
    release: Value,
    pin: Value,
    rows: Vec<Row>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Seal {
    material: Material,
    sha256: String,
}
fn check(condition: bool) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(ERROR.into())
    }
}
fn read(path: &Path) -> Result<Value, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ERROR)?;
    check(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= LIMIT)?;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| ERROR)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    check(bytes.len() <= LIMIT as usize)?;
    serde_json::from_slice(&bytes).map_err(|_| ERROR.into())
}
fn hash(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|_| ERROR)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|_| ERROR)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn material_hash(material: &Material) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(material).map_err(|_| ERROR)?)
    ))
}
fn payload_hash(payload: &Path, name: &str) -> Result<String, String> {
    let path = payload.join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|_| ERROR)?;
    check(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && path.canonicalize().map_err(|_| ERROR)?.starts_with(payload),
    )?;
    hash(&path)
}
fn owner_directory(database: &Path) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(".local-owner");
    PathBuf::from(path)
}
fn private_json(path: &Path) -> Result<Value, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ERROR)?;
    check(metadata.is_file() && !metadata.file_type().is_symlink())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        check(
            metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.permissions().mode() & 0o077 == 0
                && metadata.nlink() == 1,
        )?;
    }
    #[cfg(windows)]
    {
        crate::windows_private::read(path).map_err(|_| ERROR)?;
    }
    read(path)
}
fn seal_path(database: &Path) -> PathBuf {
    owner_directory(database).join("workspace-bootstrap.json")
}
fn binary_name() -> &'static str {
    if cfg!(windows) {
        "bin/tradeassembly.exe"
    } else {
        "bin/tradeassembly"
    }
}
fn verify_payload(payload: &Path, release: &Value) -> Result<Value, String> {
    check(payload.is_absolute() && payload.canonicalize().map_err(|_| ERROR)? == payload)?;
    check(payload.file_name().and_then(|v| v.to_str()) == release["archiveSha256"].as_str())?;
    check(
        hash(&payload.join("bundle.json"))?
            == release["bundleManifestSha256"].as_str().ok_or(ERROR)?,
    )?;
    let bundle = read(&payload.join("bundle.json"))?;
    for file in [binary_name(), "plugins/alpaca.json"] {
        check(
            payload_hash(payload, file)?
                == bundle["files"][file]["sha256"].as_str().ok_or(ERROR)?,
        )?;
    }
    let pin = read(&payload.join("plugins/alpaca.json"))?;
    let package = pin["packageFile"].as_str().ok_or(ERROR)?;
    check(Path::new(package).components().count() == 1 && !package.starts_with('.'))?;
    check(
        payload_hash(payload, &format!("plugins/{package}"))?
            == pin["packageSha256"].as_str().ok_or(ERROR)?,
    )?;
    check(
        bundle["nativeInputs"]["alpacaPackageSha256"] == pin["packageSha256"]
            && bundle["nativeInputs"]["alpacaManifestSha256"] == pin["manifestSha256"],
    )?;
    Ok(pin)
}

/// Verified package hashes and compile identity from one selected installation.
#[derive(Debug)]
pub struct InstalledPluginPin {
    pub source: PathBuf,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub version: String,
    pub source_revision: String,
}

/// Resolve the exact current or pending installed payload, without mutation or
/// network access. Dev bundles return `None`; invalid installed bindings fail.
pub fn installed_plugin_pin(
    executable: &Path,
    target: &str,
) -> Option<Result<InstalledPluginPin, String>> {
    let bin = executable.parent()?;
    let payload = bin.parent()?;
    let versions = payload.parent()?;
    if bin.file_name()? != "bin" || versions.file_name()? != "versions" {
        return None;
    }
    Some(resolve_installed_plugin_pin(executable, target))
}

fn digest_text(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn pin_regular_file(path: &Path, limit: u64) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ERROR)?;
    check(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= limit)?;
    // Canonical payload paths have already resolved system aliases. Refuse links
    // in the package/metadata path, including an otherwise contained plugin dir.
    check(path.canonicalize().map_err(|_| ERROR)? == path)?;
    Ok(())
}

fn pin_record(root: &Path, name: &str) -> Result<Option<Value>, String> {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ERROR.into()),
        Ok(_) => {
            let record = private_json(&path)?;
            check(
                record["schemaVersion"] == 1
                    && record["wardenPort"]
                        .as_u64()
                        .is_some_and(|p| p > 0 && p <= u16::MAX as u64)
                    && record["current"].is_object(),
            )?;
            Ok(Some(record))
        }
    }
}

fn resolve_installed_plugin_pin(
    executable: &Path,
    target: &str,
) -> Result<InstalledPluginPin, String> {
    // Do not resolve an attacker-controlled root/payload link into a seemingly
    // valid selected installation. System ancestors such as macOS /var are fine.
    for path in executable.ancestors().take(5) {
        check(
            !fs::symlink_metadata(path)
                .map_err(|_| ERROR)?
                .file_type()
                .is_symlink(),
        )?;
    }
    let executable = executable.canonicalize().map_err(|_| ERROR)?;
    let payload = executable.parent().and_then(Path::parent).ok_or(ERROR)?;
    let root = payload.parent().and_then(Path::parent).ok_or(ERROR)?;
    check(executable == payload.join(binary_name()))?;
    let installed = pin_record(root, "installed.json")?;
    let pending = pin_record(root, "pending.json")?;
    let selected = match (&installed, &pending) {
        (Some(current), Some(next)) => {
            // Before pointer commit: previous is the exact current release. After
            // commit: the full record is identical, awaiting journal cleanup.
            check(
                next == current
                    || (next["previous"] == current["current"]
                        && next["wardenPort"] == current["wardenPort"]),
            )?;
            &next["current"]
        }
        (None, Some(next)) => {
            check(next["previous"].is_null())?;
            &next["current"]
        }
        (Some(current), None) => &current["current"],
        (None, None) => return Err(ERROR.into()),
    };
    check(
        selected["schemaVersion"] == 2
            && selected["target"] == target
            && selected["stateCompatibility"] == "f2-local-v1"
            && digest_text(&selected["archiveSha256"])
            && digest_text(&selected["bundleManifestSha256"]),
    )?;
    pin_regular_file(&payload.join("bundle.json"), LIMIT)?;
    pin_regular_file(&executable, 512 * LIMIT)?;
    pin_regular_file(&payload.join("plugins/alpaca.json"), LIMIT)?;
    let pin = read(&payload.join("plugins/alpaca.json"))?;
    let version = pin["version"].as_str().ok_or(ERROR)?;
    let filename = pin["packageFile"].as_str().ok_or(ERROR)?;
    check(
        !version.is_empty()
            && version.len() <= 64
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
            && filename == format!("tradeassembly-plugin-alpaca-{version}.tar.gz")
            && pin["target"] == target
            && digest_text(&pin["packageSha256"])
            && digest_text(&pin["manifestSha256"]),
    )?;
    let source = pin["sourceRevision"].as_str().ok_or(ERROR)?;
    check(
        source.len() == 40
            && source
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
    )?;
    pin_regular_file(&payload.join("plugins").join(filename), 512 * LIMIT)?;
    let verified = verify_payload(payload, selected)?;
    let bundle = read(&payload.join("bundle.json"))?;
    check(
        bundle["target"] == target
            && bundle["nativeInputs"]["target"] == target
            && bundle["nativeInputs"]["coreRevision"]
                .as_str()
                .is_some_and(|s| {
                    s.len() == 40
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
            && bundle["nativeInputs"]["coreSha256"] == bundle["files"][binary_name()]["sha256"],
    )?;
    check(bundle["nativeInputs"]["alpacaRevision"] == source)?;
    Ok(InstalledPluginPin {
        source: payload.join("plugins").join(filename),
        package_sha256: verified["packageSha256"].as_str().ok_or(ERROR)?.into(),
        manifest_sha256: verified["manifestSha256"].as_str().ok_or(ERROR)?.into(),
        version: version.into(),
        source_revision: source.into(),
    })
}
fn validate(material: &Material) -> Result<(), String> {
    check(material.schema == "tradeassembly.installed-workspace-bootstrap.v1")?;
    let database = &material.database;
    check(database.canonicalize().map_err(|_| ERROR)? == *database)?;
    let state = database.parent().ok_or(ERROR)?;
    let root = state.parent().and_then(Path::parent).ok_or(ERROR)?;
    check(state == root.join("state/local") && database == &state.join("runtime.db"))?;
    check(material.payload.parent() == Some(root.join("versions").as_path()))?;
    let config = read(&state.join("runtime.json"))?;
    check(
        config["databasePath"].as_str() == database.to_str()
            && config["oidcProfile"] == "local_owner",
    )?;
    check(private_json(&owner_directory(database).join("local-owner.json"))? == material.owner)?;
    check(material.owner["version"] == 1)?;
    let owner = format!("local:{}", material.owner["ownerId"].as_str().ok_or(ERROR)?);
    check(verify_payload(&material.payload, &material.release)? == material.pin)?;
    check(material.rows.len() == 15)?;
    let mut rows = std::collections::BTreeMap::new();
    for row in &material.rows {
        check(
            rows.insert((row.namespace.clone(), row.key.clone()), &row.value)
                .is_none(),
        )?;
    }
    for (kind, id) in SCOPES {
        let key = format!("{kind}:{id}");
        let value = rows
            .remove(&("object_authorization_scopes_v1".into(), key))
            .ok_or(ERROR)?;
        check(
            *value
                == json!({"objectType":kind,"objectId":id,"owner":{"issuer":"local-owner","subject":owner,"tenantRef":format!("personal:local-owner:{owner}")},"parentType":if kind=="credential_reference" {json!("plugin_instance")} else {Value::Null},"parentId":if kind=="credential_reference" {json!(id)} else {Value::Null}}),
        )?;
    }
    check(rows.len() == 7)?;
    let mut by_namespace = std::collections::BTreeMap::new();
    for ((namespace, key), value) in rows {
        check(
            NAMESPACES.contains(&namespace.as_str())
                && by_namespace.insert(namespace, (key, value)).is_none(),
        )?;
    }
    let (command_id, command) = &by_namespace["control_plane_commands"];
    let command_id = command_id.as_str();
    let envelope = &command["envelope"];
    let preview = &envelope["payload_preview"];
    let package = &material.pin["packageSha256"];
    let manifest_digest = &material.pin["manifestSha256"];
    let locator = material
        .payload
        .join("plugins")
        .join(material.pin["packageFile"].as_str().ok_or(ERROR)?);
    check(
        envelope["command_id"] == command_id
            && envelope["correlation_id"] == command_id
            && envelope["command_name"] == "plugin.package.install"
            && envelope["source_interface"] == "cli"
            && envelope["authority"]["actor"] == owner
            && envelope["authority"]["account_mode"] == "paper"
            && preview["integrity"]
                == json!({"packageSha256":package,"manifestSha256":manifest_digest})
            && preview["offline"] == true
            && preview["source"]["locator"].as_str() == locator.to_str()
            && preview["source"]["package"] == json!({"type":"file","locator":locator})
            && command["duplicate"] == false
            && command["response_status"] == 200
            && command["response_body"]["ok"] == true,
    )?;
    let idempotency = envelope["idempotency_key"].as_str().ok_or(ERROR)?;
    check(idempotency.starts_with(&format!(
        "cli-install-default:plugin-package:{}:{}:",
        package.as_str().ok_or(ERROR)?,
        manifest_digest.as_str().ok_or(ERROR)?
    )))?;
    let manifest = &command["response_body"]["manifest"];
    check(
        manifest["pluginRef"] == "tradeassembly.alpaca"
            && manifest["manifestDigest"] == *manifest_digest
            && manifest["package"]["packageSha256"] == *package
            && manifest["package"]["manifestSha256"] == *manifest_digest
            && command["response_body"]["package"] == manifest["package"],
    )?;
    let (stored_key, stored_manifest) = &by_namespace["plugin_manifests"];
    check(
        stored_key == "tradeassembly.alpaca"
            && crate::control_plane_redaction::redact_sensitive_payload((*stored_manifest).clone())
                == *manifest,
    )?;
    check(
        by_namespace["plugin_manifest_revisions"]
            == (
                format!(
                    "tradeassembly.alpaca@{}",
                    manifest_digest.as_str().ok_or(ERROR)?
                ),
                *stored_manifest,
            ),
    )?;
    for namespace in [
        "finance_action_descriptors",
        "finance_receipts",
        "warden_decisions",
        "warden_downstream_outcomes",
    ] {
        check(by_namespace[namespace].0 == command_id)?;
    }
    let descriptor = by_namespace["finance_action_descriptors"].1;
    let decision = by_namespace["warden_decisions"].1;
    let receipt = by_namespace["finance_receipts"].1;
    let outcome = by_namespace["warden_downstream_outcomes"].1;
    check(
        descriptor["command_id"] == command_id
            && descriptor["command_name"] == "plugin.package.install"
            && descriptor["action_id"] == "plugin.configure"
            && descriptor["request_digest"] == envelope["payload_hash"]
            && decision["correlation_id"] == command_id
            && decision["request"]["request_id"] == command_id
            && decision["request"]["idempotency_key"] == idempotency
            && decision["request"]["principal_user"]["id"] == owner
            && decision["decision"]["decision"] == "allow"
            && decision["decision"]["receipt"] == receipt["receipt"]
            && receipt["receipt"]["action"] == "plugin.configure"
            && receipt["receipt"]["principal_user"]["id"] == owner
            && outcome["command_id"] == command_id
            && outcome["idempotency_key"] == idempotency
            && outcome["status"] == 200
            && outcome["outcome"] == "succeeded",
    )?;
    Ok(())
}

/// Only the owning installer calls this, immediately after the fresh command.
/// Never called for existing configurations, reinstall, upgrade, or rollback.
pub fn seal_fresh_install(root: &Path, release: Value, response: &Value) -> Result<(), String> {
    let database = root.join("state/local/runtime.db");
    let connection = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| ERROR)?;
    connection
        .execute_batch("BEGIN DEFERRED")
        .map_err(|_| ERROR)?;
    let mut statement = connection.prepare("SELECT namespace,item_key,value_json,idempotency_key,authority_actor,updated_at_ms FROM tradeassembly_kv ORDER BY namespace,item_key LIMIT 16").map_err(|_| ERROR)?;
    let mut result = statement.query([]).map_err(|_| ERROR)?;
    let mut rows = Vec::new();
    let mut total = 0usize;
    while let Some(row) = result.next().map_err(|_| ERROR)? {
        let raw: String = row.get(2).map_err(|_| ERROR)?;
        total += raw.len();
        check(total <= LIMIT as usize)?;
        rows.push(Row {
            namespace: row.get(0).map_err(|_| ERROR)?,
            key: row.get(1).map_err(|_| ERROR)?,
            value: serde_json::from_str(&raw).map_err(|_| ERROR)?,
            idempotency_key: row.get(3).map_err(|_| ERROR)?,
            authority_actor: row.get(4).map_err(|_| ERROR)?,
            updated_at_ms: row.get(5).map_err(|_| ERROR)?,
        });
    }
    for table in [
        "journal_events",
        "runtime_queue",
        "runtime_events",
        "runtime_schedules",
        "runtime_outbox",
        "runtime_inbox",
        "runtime_leases",
        "runtime_evidence",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|_| ERROR)?;
        if exists {
            let occupied: bool = connection
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table})"),
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| ERROR)?;
            check(!occupied)?;
        }
    }
    let material = Material {
        schema: "tradeassembly.installed-workspace-bootstrap.v1".into(),
        database: database.clone(),
        owner: read(&owner_directory(&database).join("local-owner.json"))?,
        payload: root
            .join("versions")
            .join(release["archiveSha256"].as_str().ok_or(ERROR)?),
        release,
        pin: Value::Null,
        rows,
    };
    let mut material = material;
    material.pin = read(&material.payload.join("plugins/alpaca.json"))?;
    validate(&material)?;
    check(
        material
            .rows
            .iter()
            .find(|r| r.namespace == "control_plane_commands")
            .ok_or(ERROR)?
            .value["response_body"]
            == crate::control_plane_redaction::redact_sensitive_payload(response["data"].clone()),
    )?;
    let seal = Seal {
        sha256: material_hash(&material)?,
        material,
    };
    let bytes = serde_json::to_vec(&seal).map_err(|_| ERROR)?;
    check(bytes.len() <= LIMIT as usize)?;
    let path = seal_path(&database);
    #[cfg(windows)]
    crate::windows_private::write_new(&path, &bytes).map_err(|_| ERROR)?;
    #[cfg(not(windows))]
    {
        use std::io::Write;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|_| ERROR)?;
        let result = (|| {
            file.write_all(&bytes).map_err(|_| ERROR)?;
            file.sync_all().map_err(|_| ERROR)?;
            fs::hard_link(&temporary, &path).map_err(|_| ERROR)?;
            Ok::<_, String>(())
        })();
        let cleanup = fs::remove_file(temporary);
        result?;
        cleanup.map_err(|_| ERROR)?;
        fs::File::open(path.parent().ok_or(ERROR)?)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ERROR)?;
    }
    Ok(())
}

/// Verify both retained original provenance and this process's current install.
pub fn load(database: &Path, executable: &Path) -> Result<Vec<Row>, String> {
    let path = seal_path(database);
    if !path.try_exists().map_err(|_| ERROR)? {
        return Ok(Vec::new());
    }
    let metadata = fs::symlink_metadata(&path).map_err(|_| ERROR)?;
    check(metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= LIMIT)?;
    let directory = owner_directory(database);
    check(directory.canonicalize().map_err(|_| ERROR)? == directory)?;
    let directory_metadata = fs::symlink_metadata(&directory).map_err(|_| ERROR)?;
    check(directory_metadata.is_dir() && !directory_metadata.file_type().is_symlink())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        check(metadata.permissions().mode() & 0o077 == 0 && metadata.nlink() == 1)?;
        check(
            directory_metadata.permissions().mode() & 0o077 == 0
                && directory_metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.uid() == directory_metadata.uid(),
        )?;
    }
    #[cfg(windows)]
    {
        crate::windows_private::read(&path).map_err(|_| ERROR)?;
    }
    let seal: Seal = serde_json::from_value(read(&path)?).map_err(|_| ERROR)?;
    check(seal.sha256 == material_hash(&seal.material)? && seal.material.database == database)?;
    validate(&seal.material)?;
    let root = database
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or(ERROR)?;
    let installed = read(&root.join("installed.json"))?;
    check(installed["schemaVersion"] == 1)?;
    let current = root.join("versions").join(
        installed["current"]["archiveSha256"]
            .as_str()
            .ok_or(ERROR)?,
    );
    check(executable == current.join(binary_name()))?;
    verify_payload(&current, &installed["current"])?;
    let state = database.parent().ok_or(ERROR)?;
    let config = read(&state.join("runtime.json"))?;
    let local = read(&state.join("installation.json"))?;
    let authority = root.join(if cfg!(windows) {
        "authority/bin/warden.exe"
    } else {
        "authority/bin/warden"
    });
    let authority_metadata = fs::symlink_metadata(&authority).map_err(|_| ERROR)?;
    check(
        authority_metadata.is_file()
            && !authority_metadata.file_type().is_symlink()
            && authority.canonicalize().map_err(|_| ERROR)? == authority,
    )?;
    let sandbox = current.join(if cfg!(windows) {
        "bin/tradeassembly-sandbox.exe"
    } else {
        "bin/tradeassembly-sandbox"
    });
    check(
        local["schemaVersion"] == "tradeassembly.local-installation.v1"
            && local["executable"].as_str() == authority.to_str()
            && local["sha256"] == installed["current"]["wardenSha256"]
            && hash(&authority)? == local["sha256"].as_str().ok_or(ERROR)?
            && local["port"] == installed["wardenPort"]
            && config["wardenSidecarUrl"]
                == format!(
                    "http://127.0.0.1:{}",
                    installed["wardenPort"].as_u64().ok_or(ERROR)?
                )
            && config["wardenTokenRef"]
                == format!("file://{}", state.join("warden.token").display())
            && config["pluginSandboxCommand"].as_str() == sandbox.to_str(),
    )?;
    Ok(seal.material.rows)
}

#[cfg(test)]
pub(crate) mod test_fixture {
    use super::*;
    pub struct Fixture {
        pub _temporary: tempfile::TempDir,
        pub root: PathBuf,
        pub database: PathBuf,
        pub binary: PathBuf,
        pub release: Value,
        pub response: Value,
    }
    impl Fixture {
        pub fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let database = root.join("state/local/runtime.db");
            let archive = "a".repeat(64);
            let payload = root.join("versions").join(&archive);
            fs::create_dir_all(payload.join("bin")).unwrap();
            fs::create_dir(payload.join("plugins")).unwrap();
            fs::create_dir_all(database.parent().unwrap()).unwrap();
            fs::create_dir(owner_directory(&database)).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    owner_directory(&database),
                    fs::Permissions::from_mode(0o700),
                )
                .unwrap();
            }
            let binary = payload.join(binary_name());
            fs::write(&binary, b"fixture Core; not executed by unit tests").unwrap();
            let package = payload.join("plugins/package.tar.gz");
            fs::write(
                &package,
                b"fixture package; native qualification is separate",
            )
            .unwrap();
            let package_hash = hash(&package).unwrap();
            let manifest_digest = "b".repeat(64);
            let pin = json!({"packageFile":"package.tar.gz","packageSha256":package_hash,"manifestSha256":manifest_digest});
            fs::write(
                payload.join("plugins/alpaca.json"),
                serde_json::to_vec(&pin).unwrap(),
            )
            .unwrap();
            let bundle = json!({"nativeInputs":{"alpacaPackageSha256":package_hash,"alpacaManifestSha256":manifest_digest},"files":{binary_name():{"sha256":hash(&binary).unwrap()},"plugins/alpaca.json":{"sha256":hash(&payload.join("plugins/alpaca.json")).unwrap()}}});
            fs::write(
                payload.join("bundle.json"),
                serde_json::to_vec(&bundle).unwrap(),
            )
            .unwrap();
            let authority = root.join(if cfg!(windows) {
                "authority/bin/warden.exe"
            } else {
                "authority/bin/warden"
            });
            fs::create_dir_all(authority.parent().unwrap()).unwrap();
            fs::write(&authority, b"fixture authority; not executed").unwrap();
            let release = json!({"archiveSha256":archive,"bundleManifestSha256":hash(&payload.join("bundle.json")).unwrap(),"wardenSha256":hash(&authority).unwrap()});
            fs::write(
                root.join("installed.json"),
                serde_json::to_vec(
                    &json!({"schemaVersion":1,"current":release,"wardenPort":32123}),
                )
                .unwrap(),
            )
            .unwrap();
            fs::write(
                database.parent().unwrap().join("runtime.json"),
                serde_json::to_vec(&json!({"databasePath":database,"oidcProfile":"local_owner","wardenSidecarUrl":"http://127.0.0.1:32123","wardenTokenRef":format!("file://{}",database.parent().unwrap().join("warden.token").display()),"pluginSandboxCommand":payload.join(if cfg!(windows) {"bin/tradeassembly-sandbox.exe"} else {"bin/tradeassembly-sandbox"})}))
                    .unwrap(),
            )
            .unwrap();
            fs::write(database.parent().unwrap().join("installation.json"),serde_json::to_vec(&json!({"schemaVersion":"tradeassembly.local-installation.v1","executable":authority,"sha256":release["wardenSha256"],"port":32123})).unwrap()).unwrap();
            fs::write(
                owner_directory(&database).join("local-owner.json"),
                br#"{"version":1,"ownerId":"fixture-owner"}"#,
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    owner_directory(&database).join("local-owner.json"),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
            let owner = "local:fixture-owner";
            let idempotency = format!(
                "cli-install-default:plugin-package:{package_hash}:{manifest_digest}:fixture"
            );
            let manifest = json!({"pluginRef":"tradeassembly.alpaca","manifestDigest":manifest_digest,"package":{"packageSha256":package_hash,"manifestSha256":manifest_digest},"manifest":{"configuration":{"api_secret":{"type":"string"},"label":"paper fixture"}}});
            let response = json!({"ok":true,"data":{"ok":true,"manifest":manifest,"package":manifest["package"]}});
            let envelope = json!({"command_id":"cmd_fixture","correlation_id":"cmd_fixture","command_name":"plugin.package.install","source_interface":"cli","authority":{"actor":owner,"account_mode":"paper"},"idempotency_key":idempotency,"payload_hash":"fixture-payload","payload_preview":{"integrity":{"packageSha256":package_hash,"manifestSha256":manifest_digest},"offline":true,"source":{"locator":package,"package":{"type":"file","locator":package}}}});
            let receipt =
                json!({"receipt":{"action":"plugin.configure","principal_user":{"id":owner}}});
            let mut rows = vec![
                (
                    "control_plane_commands".to_string(),
                    "cmd_fixture".to_string(),
                    json!({"envelope":envelope,"duplicate":false,"response_status":200,"response_body":crate::control_plane_redaction::redact_sensitive_payload(response["data"].clone())}),
                ),
                (
                    "plugin_manifests".into(),
                    "tradeassembly.alpaca".into(),
                    manifest.clone(),
                ),
                (
                    "plugin_manifest_revisions".into(),
                    format!("tradeassembly.alpaca@{manifest_digest}"),
                    manifest,
                ),
                (
                    "finance_action_descriptors".into(),
                    "cmd_fixture".into(),
                    json!({"command_id":"cmd_fixture","command_name":"plugin.package.install","action_id":"plugin.configure","request_digest":"fixture-payload"}),
                ),
                (
                    "finance_receipts".into(),
                    "cmd_fixture".into(),
                    receipt.clone(),
                ),
                (
                    "warden_decisions".into(),
                    "cmd_fixture".into(),
                    json!({"correlation_id":"cmd_fixture","request":{"request_id":"cmd_fixture","idempotency_key":idempotency,"principal_user":{"id":owner}},"decision":{"decision":"allow","receipt":receipt["receipt"]}}),
                ),
                (
                    "warden_downstream_outcomes".into(),
                    "cmd_fixture".into(),
                    json!({"command_id":"cmd_fixture","idempotency_key":idempotency,"status":200,"outcome":"succeeded"}),
                ),
            ];
            for (kind, id) in SCOPES {
                rows.push(("object_authorization_scopes_v1".into(),format!("{kind}:{id}"),json!({"objectType":kind,"objectId":id,"owner":{"issuer":"local-owner","subject":owner,"tenantRef":format!("personal:local-owner:{owner}")},"parentType":if kind=="credential_reference" {json!("plugin_instance")} else {Value::Null},"parentId":if kind=="credential_reference" {json!(id)} else {Value::Null}})));
            }
            let connection = Connection::open(&database).unwrap();
            connection.execute_batch("CREATE TABLE tradeassembly_kv(namespace TEXT,item_key TEXT,value_json TEXT,idempotency_key TEXT,authority_actor TEXT,updated_at_ms INTEGER, PRIMARY KEY(namespace,item_key))").unwrap();
            for (namespace, key, value) in rows {
                connection
                    .execute(
                        "INSERT INTO tradeassembly_kv VALUES(?1,?2,?3,'fixture','fixture',1)",
                        rusqlite::params![namespace, key, serde_json::to_string(&value).unwrap()],
                    )
                    .unwrap();
            }
            Self {
                _temporary: temporary,
                root,
                database,
                binary,
                release,
                response,
            }
        }
        pub fn seal(&self) {
            seal_fresh_install(&self.root, self.release.clone(), &self.response).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{test_fixture::Fixture, *};

    #[test]
    fn workspace_bootstrap_owning_serializer_is_private_create_only_and_bound() {
        let fixture = Fixture::new();
        fixture.seal();
        assert_eq!(load(&fixture.database, &fixture.binary).unwrap().len(), 15);
        let path = seal_path(&fixture.database);
        let original = fs::read(&path).unwrap();
        assert!(
            seal_fresh_install(&fixture.root, fixture.release.clone(), &fixture.response).is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        for pointer in [
            "/material/database",
            "/material/owner/ownerId",
            "/material/pin/packageSha256",
            "/sha256",
        ] {
            let mut altered: Value = serde_json::from_slice(&original).unwrap();
            *altered.pointer_mut(pointer).unwrap() = json!("changed");
            fs::write(&path, serde_json::to_vec(&altered).unwrap()).unwrap();
            assert_eq!(load(&fixture.database, &fixture.binary).unwrap_err(), ERROR);
        }
        fs::write(&path, &original).unwrap();
        fs::write(&fixture.binary, b"changed executable").unwrap();
        assert_eq!(load(&fixture.database, &fixture.binary).unwrap_err(), ERROR);
    }
    #[test]
    fn workspace_bootstrap_never_seals_customer_data_or_activity() {
        for activity in [false, true] {
            let fixture = Fixture::new();
            let connection = Connection::open(&fixture.database).unwrap();
            if activity {
                connection.execute_batch("CREATE TABLE runtime_events(id TEXT); INSERT INTO runtime_events VALUES('event')").unwrap();
            } else {
                connection.execute("INSERT INTO tradeassembly_kv VALUES('strategies','customer','{}','customer','customer',1)",[]).unwrap();
            }
            assert_eq!(
                seal_fresh_install(&fixture.root, fixture.release, &fixture.response).unwrap_err(),
                ERROR
            );
            assert!(!seal_path(&fixture.database).exists());
        }
    }
}

#[cfg(test)]
pub(crate) mod installed_pin_tests {
    use super::*;
    pub struct Fixture {
        pub _temp: tempfile::TempDir,
        pub root: PathBuf,
        pub payload: PathBuf,
        pub executable: PathBuf,
        pub pin: Value,
        pub release: Value,
    }
    pub fn private_record(path: &Path, value: &Value) {
        let bytes = serde_json::to_vec(value).unwrap();
        #[cfg(windows)]
        {
            crate::windows_private::ensure_directory(path.parent().unwrap()).unwrap();
            crate::windows_private::write_atomic(path, &bytes).unwrap();
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    impl Fixture {
        pub fn new(target: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let payload = root.join("versions").join("a".repeat(64));
            fs::create_dir_all(payload.join("bin")).unwrap();
            fs::create_dir(payload.join("plugins")).unwrap();
            let executable = payload.join(binary_name());
            fs::write(&executable, b"new runtime fixture; never executed").unwrap();
            let filename = "tradeassembly-plugin-alpaca-0.1.19.tar.gz";
            fs::write(
                payload.join("plugins").join(filename),
                b"derived signed-package fixture",
            )
            .unwrap();
            let package_hash = hash(&payload.join("plugins").join(filename)).unwrap();
            let compiled: Value =
                serde_json::from_str(include_str!("../packaging/alpaca.json")).unwrap();
            let pin = json!({"version":"0.1.19","target":target,"packageFile":filename,"sourceRevision":compiled["sourceRevision"],"packageSha256":package_hash,"manifestSha256":"b".repeat(64)});
            fs::write(
                payload.join("plugins/alpaca.json"),
                serde_json::to_vec(&pin).unwrap(),
            )
            .unwrap();
            let core_sha = hash(&executable).unwrap();
            let bundle = json!({"target":target,"nativeInputs":{"target":target,"coreRevision":"c".repeat(40),"coreSha256":core_sha,"alpacaRevision":pin["sourceRevision"],"alpacaPackageSha256":package_hash,"alpacaManifestSha256":pin["manifestSha256"]},"files":{binary_name():{"sha256":core_sha},"plugins/alpaca.json":{"sha256":hash(&payload.join("plugins/alpaca.json")).unwrap()}}});
            fs::write(
                payload.join("bundle.json"),
                serde_json::to_vec(&bundle).unwrap(),
            )
            .unwrap();
            let release = json!({"schemaVersion":2,"target":target,"stateCompatibility":"f2-local-v1","archiveSha256":"a".repeat(64),"bundleManifestSha256":hash(&payload.join("bundle.json")).unwrap()});
            private_record(
                &root.join("installed.json"),
                &json!({"schemaVersion":1,"current":release,"previous":null,"wardenPort":32123}),
            );
            Self {
                _temp: temp,
                root,
                payload,
                executable,
                pin,
                release,
            }
        }
    }
    #[test]
    fn installed_pin_uses_exact_derived_package_and_never_mutates_owner_record() {
        let f = Fixture::new("aarch64-apple-darwin");
        let before = fs::read(f.root.join("installed.json")).unwrap();
        let result = installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
            .unwrap()
            .unwrap();
        assert_eq!(result.package_sha256, f.pin["packageSha256"]);
        assert_eq!(result.manifest_sha256, f.pin["manifestSha256"]);
        assert_eq!(
            result.source,
            f.payload
                .join("plugins")
                .join(f.pin["packageFile"].as_str().unwrap())
        );
        assert_eq!(fs::read(f.root.join("installed.json")).unwrap(), before);
        assert!(
            installed_plugin_pin(&f.executable, "x86_64-unknown-linux-gnu")
                .unwrap()
                .is_err()
        );
    }
    #[test]
    fn installed_pin_substitution_of_package_lock_bundle_or_executing_binary_fails_closed() {
        for relative in [
            "plugins/tradeassembly-plugin-alpaca-0.1.19.tar.gz",
            "plugins/alpaca.json",
            "bundle.json",
            binary_name(),
        ] {
            let f = Fixture::new("aarch64-apple-darwin");
            fs::write(f.payload.join(relative), b"substitution").unwrap();
            assert!(
                installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
                    .unwrap()
                    .is_err(),
                "{relative}"
            );
        }
        let f = Fixture::new("aarch64-apple-darwin");
        fs::remove_file(f.root.join("installed.json")).unwrap();
        assert!(installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
            .unwrap()
            .is_err());
    }
    #[test]
    fn installed_pin_pending_fresh_upgrade_and_committed_recovery_are_exact() {
        let f = Fixture::new("aarch64-apple-darwin");
        let next =
            json!({"schemaVersion":1,"current":f.release,"previous":null,"wardenPort":32123});
        private_record(&f.root.join("pending.json"), &next);
        fs::remove_file(f.root.join("installed.json")).unwrap();
        installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
            .unwrap()
            .unwrap();
        let mut old = f.release.clone();
        old["archiveSha256"] = json!("d".repeat(64));
        let current = json!({"schemaVersion":1,"current":old,"previous":null,"wardenPort":32123});
        private_record(&f.root.join("installed.json"), &current);
        assert!(installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
            .unwrap()
            .is_err());
        let upgrade =
            json!({"schemaVersion":1,"current":f.release,"previous":old,"wardenPort":32123});
        private_record(&f.root.join("pending.json"), &upgrade);
        installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
            .unwrap()
            .unwrap();
        private_record(&f.root.join("installed.json"), &upgrade);
        installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
            .unwrap()
            .unwrap();
        let mut wrong = upgrade;
        wrong["wardenPort"] = json!(32124);
        private_record(&f.root.join("pending.json"), &wrong);
        assert!(installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
            .unwrap()
            .is_err());
    }
    #[test]
    fn installed_pin_does_not_apply_to_dev_bundles_and_rejects_linked_or_nonprivate_records() {
        let f = Fixture::new("aarch64-apple-darwin");
        assert!(installed_plugin_pin(
            &f.root.join("dev/bin/tradeassembly"),
            "aarch64-apple-darwin"
        )
        .is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let record = f.root.join("installed.json");
            fs::set_permissions(&record, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
                .unwrap()
                .is_err());
            fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();
            let pin = f.payload.join("plugins/alpaca.json");
            let saved = f.payload.join("saved-pin.json");
            fs::rename(&pin, &saved).unwrap();
            symlink(saved, &pin).unwrap();
            assert!(installed_plugin_pin(&f.executable, "aarch64-apple-darwin")
                .unwrap()
                .is_err());
        }
    }
}
