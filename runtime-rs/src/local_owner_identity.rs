//! Filesystem-backed identity for account-free local operation.
//!
//! The containing directory is the installation's trust root. This module does
//! not provide cryptographic tamper detection; it only refuses unsafe or
//! malformed state and never accepts an identity from a caller.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

const STATE_VERSION: u8 = 1;
const STATE_FILE: &str = "local-owner.json";
const MAX_STATE_BYTES: u64 = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalOwnerIdentity {
    pub stable_identity_id: String,
    pub issuer: String,
    pub subject: String,
    pub audience: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredIdentity {
    version: u8,
    owner_id: String,
}

impl LocalOwnerIdentity {
    /// Derive the installation state directory from a real database path.
    pub fn for_database(database_path: impl AsRef<Path>) -> Result<Self, String> {
        let database_path = database_path.as_ref();
        if database_path.as_os_str().is_empty() || database_path == Path::new(":memory:") {
            return Err("local_owner_database_path_invalid".to_string());
        }
        let mut state_dir = database_path.as_os_str().to_os_string();
        state_dir.push(".local-owner");
        let state_dir = PathBuf::from(state_dir);
        Self::load_or_create(state_dir)
    }

    /// Load the installation owner, creating it once under `state_dir`.
    pub fn load_or_create(state_dir: impl AsRef<Path>) -> Result<Self, String> {
        let state_dir = state_dir.as_ref();
        ensure_private_directory(state_dir)?;
        let state_path = state_dir.join(STATE_FILE);
        let stored = match read_state(&state_path) {
            Ok(stored) => stored,
            Err(error) if error == "local_owner_state_missing" => create_state(&state_path)?,
            Err(error) => return Err(error),
        };
        validate_stored(&stored)?;
        Ok(Self::from_stored(stored))
    }

    fn from_stored(stored: StoredIdentity) -> Self {
        let subject = format!("local:{}", stored.owner_id);
        Self {
            stable_identity_id: subject.clone(),
            issuer: "local-owner".to_string(),
            subject,
            audience: vec!["tradeassembly-local".to_string()],
        }
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    reject_symlink_components(path)?;
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .map_err(|_| "local_owner_state_unavailable".to_string())?;
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "local_owner_state_unavailable".to_string())?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("local_owner_state_directory_invalid".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("local_owner_state_directory_insecure".to_string());
        }
    }
    Ok(())
}

fn reject_symlink_components(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component),
            Component::CurDir => {}
            Component::ParentDir => current.push(component),
            Component::Normal(_) => {
                current.push(component);
                if fs::symlink_metadata(&current)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    return Err("local_owner_state_directory_invalid".to_string());
                }
            }
        }
    }
    Ok(())
}

fn create_state(path: &Path) -> Result<StoredIdentity, String> {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let stored = StoredIdentity {
        version: STATE_VERSION,
        owner_id: URL_SAFE_NO_PAD.encode(bytes),
    };
    validate_stored(&stored)?;
    let encoded =
        serde_json::to_vec(&stored).map_err(|_| "local_owner_state_unavailable".to_string())?;
    let temp_path = path.with_file_name(format!(".local-owner.{}.tmp", stored.owner_id));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp_path)
            .map_err(|_| "local_owner_state_unavailable".to_string())?;
        std::io::Write::write_all(&mut file, &encoded)
            .map_err(|_| "local_owner_state_unavailable".to_string())?;
        file.sync_all()
            .map_err(|_| "local_owner_state_unavailable".to_string())?;
        match fs::hard_link(&temp_path, path) {
            Ok(()) => Ok(stored.clone()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => read_state(path),
            Err(_) => Err("local_owner_state_unavailable".to_string()),
        }
    })();
    let _ = fs::remove_file(&temp_path);
    result
}

fn read_state(path: &Path) -> Result<StoredIdentity, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err("local_owner_state_missing".to_string())
        }
        Err(_) => return Err("local_owner_state_unavailable".to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("local_owner_state_file_invalid".to_string());
    }
    if metadata.len() > MAX_STATE_BYTES {
        return Err("local_owner_state_oversized".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("local_owner_state_file_insecure".to_string());
        }
    }
    let bytes = fs::read(path).map_err(|_| "local_owner_state_unavailable".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "local_owner_state_invalid".to_string())
}

fn validate_stored(stored: &StoredIdentity) -> Result<(), String> {
    if stored.version != STATE_VERSION {
        return Err("local_owner_state_version_unsupported".to_string());
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(&stored.owner_id)
        .map_err(|_| "local_owner_state_id_invalid".to_string())?;
    if decoded.len() != 32 || stored.owner_id.trim() != stored.owner_id {
        return Err("local_owner_state_id_invalid".to_string());
    }
    Ok(())
}
