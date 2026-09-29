//! Private local Windows files. No secret is written before owner/ACL validation.
//! Callers retain their existing content formats, integrity checks and locks.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf, Prefix},
};
use windows_permissions::{
    constants::{AceType, SeObjectType, SecurityInformation},
    utilities::current_process_sid,
    wrappers::{GetSecurityInfo, SetSecurityInfo},
    LocalBox, SecurityDescriptor, Sid,
};

const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;
const REPARSE_POINT: u32 = 0x400;
const READ_CONTROL: u32 = 0x0002_0000;
const CHANGE_SECURITY: u32 = 0x000e_0000;
const READ_WRITE: u32 = 0xc000_0000;
const SHARE_READ_WRITE: u32 = 3; // Never FILE_SHARE_DELETE.

fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "private local file validation failed",
    )
}

fn local_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut components = absolute.components();
    if !matches!(components.next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || components.next() != Some(Component::RootDir)
    {
        return Err(denied());
    }
    for component in components {
        let Component::Normal(name) = component else {
            return Err(denied());
        };
        let name = name.to_str().ok_or_else(denied)?;
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if name.ends_with(['.', ' '])
            || name
                .chars()
                .any(|c| c.is_control() || ":<>\"|?*".contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
                })
            })
        {
            return Err(denied());
        }
    }
    Ok(absolute)
}

fn directory_handle(path: &Path, change_security: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .access_mode(if change_security {
            CHANGE_SECURITY
        } else {
            READ_CONTROL
        })
        .share_mode(SHARE_READ_WRITE)
        .custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(denied());
    }
    Ok(file)
}

fn validate_acl(file: &File, allow_privileged: bool) -> io::Result<()> {
    let user = current_process_sid()?;
    let sd = GetSecurityInfo(
        file,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner | SecurityInformation::Dacl,
    )?;
    let system: LocalBox<Sid> = "S-1-5-18".parse()?;
    let administrators: LocalBox<Sid> = "S-1-5-32-544".parse()?;
    if sd.owner() != Some(user.as_ref())
        && (!allow_privileged
            || (sd.owner() != Some(system.as_ref()) && sd.owner() != Some(administrators.as_ref())))
    {
        return Err(denied());
    }
    let acl = sd.dacl().ok_or_else(denied)?; // Null DACL grants everyone.
    let mut owner_allowed = false;
    for index in 0..acl.len() {
        let ace = acl.get_ace(index).ok_or_else(denied)?;
        // Reject conditional/object/unknown ACEs rather than approximate them.
        if ace.ace_type() != AceType::ACCESS_ALLOWED_ACE_TYPE {
            return Err(denied());
        }
        let sid = ace.sid().ok_or_else(denied)?;
        if sid == user.as_ref() {
            owner_allowed = true;
        } else if !allow_privileged || (sid != system.as_ref() && sid != administrators.as_ref()) {
            return Err(denied());
        }
    }
    if !owner_allowed {
        return Err(denied());
    }
    Ok(())
}

fn protect(file: &mut File, directory: bool) -> io::Result<()> {
    let user = current_process_sid()?;
    let inheritance = if directory { "OICI" } else { "" };
    let sd: LocalBox<SecurityDescriptor> =
        format!("O:{user}D:P(A;{inheritance};FA;;;{user})").parse()?;
    SetSecurityInfo(
        file,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner | SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
        Some(user.as_ref()),
        None,
        sd.dacl(),
        None,
    )?;
    validate_acl(file, false)
}

/// Held directory handles prevent ancestor replacement while resolving a child.
struct Parents(Vec<File>);

fn private_parent(path: &Path) -> io::Result<Parents> {
    let parent = path.parent().ok_or_else(denied)?;
    let mut current = PathBuf::new();
    let mut handles = Vec::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        if current.has_root() {
            handles.push(directory_handle(&current, false)?);
        }
    }
    let last = handles.last().ok_or_else(denied)?;
    validate_acl(last, true)?;
    Ok(Parents(handles))
}

/// Create an empty private directory inside an already private local ancestor.
pub fn create_directory(path: &Path) -> io::Result<()> {
    let path = local_path(path)?;
    let parents = private_parent(&path)?;
    fs::create_dir(&path)?;
    let mut file = directory_handle(&path, true)?;
    protect(&mut file, true)?;
    drop(parents);
    Ok(())
}

pub fn validate_directory(path: &Path) -> io::Result<()> {
    let path = local_path(path)?;
    let parents = private_parent(&path)?;
    let directory = directory_handle(&path, false)?;
    validate_acl(&directory, false)?;
    drop(parents);
    Ok(())
}

/// Create only missing directories. Never tighten an existing non-private root.
pub fn ensure_directory(path: &Path) -> io::Result<()> {
    let path = local_path(path)?;
    let mut missing = Vec::new();
    let mut ancestor = path.as_path();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor.to_owned());
                ancestor = ancestor.parent().ok_or_else(denied)?;
            }
            Err(error) => return Err(error),
        }
    }
    if missing.is_empty() {
        return validate_directory(&path);
    }
    for directory in missing.into_iter().rev() {
        match create_directory(&directory) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                validate_directory(&directory)?
            }
            Err(error) => return Err(error),
        }
    }
    validate_directory(&path)
}

/// Create exclusively, protect and validate before the caller writes any bytes.
pub fn create_new(path: &Path) -> io::Result<File> {
    let path = local_path(path)?;
    let parents = private_parent(&path)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .access_mode(READ_WRITE | CHANGE_SECURITY)
        .share_mode(SHARE_READ_WRITE)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(&path)?;
    protect(&mut file, false)?;
    drop(parents);
    Ok(file)
}

fn open_existing(path: &Path, write: bool) -> io::Result<File> {
    let path = local_path(path)?;
    let parents = private_parent(&path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .share_mode(SHARE_READ_WRITE)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(denied());
    }
    validate_acl(&file, false)?;
    drop(parents);
    Ok(file)
}

pub fn open_read(path: &Path) -> io::Result<File> {
    open_existing(path, false)
}

pub fn open_lock(path: &Path) -> io::Result<File> {
    match write_new(path, &[]) {
        Ok(()) => open_existing(path, true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => open_existing(path, true),
        Err(error) => Err(error),
    }
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_read(path)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn temporary_path(path: &Path) -> io::Result<PathBuf> {
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce).map_err(|_| io::Error::other("private file nonce unavailable"))?;
    Ok(path.parent().ok_or_else(denied)?.join(format!(
        ".tradeassembly-local-write-{}.tmp",
        hex::encode(nonce)
    )))
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write(path, contents, true)
}

/// Copy public executable bytes into protected state without copying source ACLs.
pub fn copy_private(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    let destination = local_path(destination)?;
    let parents = private_parent(&destination)?;
    if fs::symlink_metadata(&destination).is_ok() {
        drop(open_read(&destination)?);
        if !replace {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "private file exists",
            ));
        }
    }
    let temporary = temporary_path(&destination)?;
    let result = (|| {
        let mut source = File::open(source)?;
        let mut output = create_new(&temporary)?;
        io::copy(&mut source, &mut output)?;
        output.sync_all()?;
        drop(output);
        if replace {
            atomicwrites::replace_atomic(&temporary, &destination)
        } else {
            atomicwrites::move_atomic(&temporary, &destination)
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    drop(parents);
    if !replace && result.is_err() && open_existing(&destination, false).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "private file exists",
        ));
    }
    result
}

pub fn write_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    write(path, contents, false)
}

fn write(path: &Path, contents: &[u8], replace: bool) -> io::Result<()> {
    let path = local_path(path)?;
    let parents = private_parent(&path)?;
    // Never replace an existing unsafe/reparse-backed secret silently.
    match fs::symlink_metadata(&path) {
        Ok(_) if replace => {
            drop(open_read(&path)?);
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "private file exists",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let temporary = temporary_path(&path)?;
    let result = (|| {
        let mut file = create_new(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if replace {
            atomicwrites::replace_atomic(&temporary, &path)
        } else {
            atomicwrites::move_atomic(&temporary, &path)
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    drop(parents);
    if !replace && result.is_err() && open_existing(&path, false).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "private file exists",
        ));
    }
    result
}

pub fn remove_durable(path: &Path) -> io::Result<()> {
    let path = local_path(path)?;
    let parents = private_parent(&path)?;
    match open_read(&path) {
        Ok(file) => drop(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let tombstone = temporary_path(&path)?;
    atomicwrites::move_atomic(&path, &tombstone)?;
    // The original name is durably absent. A leftover private tombstone is safe.
    let _ = fs::remove_file(tombstone);
    drop(parents);
    Ok(())
}

/// Publish an already-synced export directory without replacing any destination.
pub fn publish_directory(temporary: &Path, destination: &Path) -> io::Result<()> {
    let temporary = local_path(temporary)?;
    let destination = local_path(destination)?;
    if temporary.parent() != destination.parent() {
        return Err(denied());
    }
    let parents = private_parent(&temporary)?;
    let directory = directory_handle(&temporary, false)?;
    validate_acl(&directory, false)?;
    drop(directory);
    atomicwrites::move_atomic(&temporary, &destination)?;
    drop(parents);
    Ok(())
}

/// Only for a freshly allocated, empty staging directory in a private parent.
pub fn protect_empty_staging_directory(path: &Path) -> io::Result<()> {
    let path = local_path(path)?;
    let parents = private_parent(&path)?;
    let mut directory = directory_handle(&path, true)?;
    validate_acl(&directory, true)?;
    if fs::read_dir(&path)?.next().is_some() {
        return Err(denied());
    }
    protect(&mut directory, true)?;
    drop(parents);
    Ok(())
}

impl Drop for Parents {
    fn drop(&mut self) {
        // Explicitly retain handles until the enclosing operation is finished.
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;

    fn root() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("authority");
        create_directory(&root).unwrap();
        (temporary, root)
    }

    #[test]
    fn private_files_are_atomic_reopenable_and_durably_removable() {
        let (_temporary, root) = root();
        let path = root.join("key");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(read(&path).unwrap(), b"first");
        write_atomic(&path, b"second").unwrap();
        assert_eq!(read(&path).unwrap(), b"second");
        assert_eq!(
            create_new(&path).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        remove_durable(&path).unwrap();
        assert!(!path.exists());
        remove_durable(&path).unwrap();
    }

    #[test]
    fn private_roots_are_recursive_and_publication_never_clobbers_identity() {
        let (_temporary, root) = root();
        let nested = root.join("state/local");
        ensure_directory(&nested).unwrap();
        ensure_directory(&nested).unwrap();
        let owner = nested.join("owner.json");
        write_new(&owner, b"original identity").unwrap();
        assert_eq!(
            write_new(&owner, b"replacement identity")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(read(&owner).unwrap(), b"original identity");
    }

    #[test]
    fn private_lock_contends_between_independent_handles() {
        let (_temporary, root) = root();
        let path = root.join("lock");
        let first = open_lock(&path).unwrap();
        let second = open_lock(&path).unwrap();
        FileExt::lock_exclusive(&first).unwrap();
        assert!(FileExt::try_lock_exclusive(&second).is_err());
        FileExt::unlock(&first).unwrap();
        FileExt::try_lock_exclusive(&second).unwrap();
        FileExt::unlock(&second).unwrap();
    }

    #[test]
    fn broad_acl_is_rejected_for_reads_and_replacement() {
        let (_temporary, root) = root();
        let path = root.join("key");
        let mut file = create_new(&path).unwrap();
        file.write_all(b"original").unwrap();
        let user = current_process_sid().unwrap();
        let sd: LocalBox<SecurityDescriptor> =
            format!("D:P(A;;FA;;;{user})(A;;FR;;;WD)").parse().unwrap();
        SetSecurityInfo(
            &mut file,
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            sd.dacl(),
            None,
        )
        .unwrap();
        drop(file);
        assert_eq!(
            read(&path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            write_atomic(&path, b"replacement").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }

    #[test]
    fn reparse_parents_and_remote_or_stream_paths_are_rejected() {
        let (_temporary, root) = root();
        let other = root.join("other");
        create_directory(&other).unwrap();
        let link = root.join("link");
        // Native runner must support the real security fixture; never skip it.
        std::os::windows::fs::symlink_dir(&other, &link).unwrap();
        assert_eq!(
            create_new(&link.join("key")).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(local_path(Path::new(r"\\server\share\key")).is_err());
        assert!(local_path(&root.join("key:stream")).is_err());
    }
}
