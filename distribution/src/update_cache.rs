// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Private, durable authenticated download cache. Does not execute downloaded code.
//! Native activation belongs to the owning single/combined delivery installer.

use crate::updates::{
    authenticate, plan, BinaryRelease, Installation, UpdateDecision, UpdateProfile,
};
use crate::{atomic_json, digest, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use url::Url;

const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrustIdentity {
    pub installation_root: PathBuf,
    pub product: String,
    pub target: String,
    pub profile: UpdateProfile,
    pub public_key: [u8; 32],
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema: u32,
    identity: TrustIdentity,
    highest_sequence: u64,
    metadata_sha256: Option<String>,
    staged_archive_sha256: Option<String>,
}

/// Constructed only after signature, replay and downloaded-byte verification.
pub struct ReadyArchive {
    archive: PathBuf,
    release: BinaryRelease,
}
impl ReadyArchive {
    pub fn path(&self) -> &Path {
        &self.archive
    }
    pub fn release(&self) -> &BinaryRelease {
        &self.release
    }
}

pub struct UpdateCache {
    root: PathBuf,
    identity: TrustIdentity,
    _lock: File,
}

fn regular(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|_| "update_file_unavailable")?;
    let meta = file.metadata().map_err(|_| "update_file_unavailable")?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
        return Err("update_file_insecure".into());
    }
    Ok(file)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let mut bytes = Vec::new();
    regular(path)?
        .take(131_073)
        .read_to_end(&mut bytes)
        .map_err(|_| "update_receipt_read_failed")?;
    if bytes.len() > 131_072 {
        return Err("update_receipt_limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "update_receipt_invalid".into())
}

impl UpdateCache {
    /// Fetch the stable product/platform feed, then authenticate and stage it.
    /// Pinned/development profiles never make a metadata or artifact request.
    pub fn check(&self, installed: &Installation, now: u64) -> Result<UpdateDecision> {
        self.check_with(installed, now, &HttpsTransport)
    }

    fn check_with(
        &self,
        installed: &Installation,
        now: u64,
        transport: &dyn Transport,
    ) -> Result<UpdateDecision> {
        self.check_installation(installed)?;
        if !matches!(installed.profile, UpdateProfile::Stable {}) {
            return plan(installed, &[], &self.identity.public_key, now);
        }
        let repo = match self.identity.product.as_str() {
            "core-bundle" => "Core",
            "studio-local" => "Studio",
            _ => return Err("update_feed_product_invalid".into()),
        };
        if !crate::TARGETS.contains(&self.identity.target.as_str()) {
            return Err("update_feed_target_invalid".into());
        }
        let url = format!("https://github.com/TradeAssembly/{repo}/releases/latest/download/tradeassembly-update-{}.json",self.identity.target);
        let response = transport.get(&url)?;
        if response.size.is_some_and(|n| n > 65_536) {
            return Err("update_metadata_limit".into());
        }
        let mut envelope = Vec::new();
        response
            .body
            .take(65_537)
            .read_to_end(&mut envelope)
            .map_err(|_| "update_metadata_download_failed")?;
        if envelope.len() > 65_536 || response.size.is_some_and(|n| n != envelope.len() as u64) {
            return Err("update_metadata_limit_or_truncation".into());
        }
        self.download_with(installed, &envelope, now, transport)
    }

    /// Explicit enrollment into an empty private directory; never replaces trust.
    /// The owning installer must supply the authenticated application identity.
    pub fn enroll(root: &Path, identity: TrustIdentity) -> Result<Self> {
        let cache = Self::lock(root, identity)?;
        if fs::read_dir(&cache.root)
            .map_err(|_| "update_cache_unavailable")?
            .any(|entry| entry.is_err() || entry.is_ok_and(|e| e.file_name() != "update.lock"))
        {
            return Err("update_cache_not_empty".into());
        }
        cache.save(&State {
            schema: 1,
            identity: cache.identity.clone(),
            highest_sequence: 0,
            metadata_sha256: None,
            staged_archive_sha256: None,
        })?;
        Ok(cache)
    }

    pub fn open(root: &Path, identity: TrustIdentity) -> Result<Self> {
        let cache = Self::lock(root, identity)?;
        cache.state()?;
        Ok(cache)
    }

    fn lock(root: &Path, identity: TrustIdentity) -> Result<Self> {
        if !root.is_absolute()
            || !identity.installation_root.is_absolute()
            || identity
                .installation_root
                .canonicalize()
                .map_err(|_| "update_installation_unavailable")?
                != identity.installation_root
            || root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("update_root_invalid".into());
        }
        // Resolve only the existing parent (macOS /var is a system link).
        let parent = root
            .parent()
            .ok_or("update_root_invalid")?
            .canonicalize()
            .map_err(|_| "update_cache_parent_unavailable")?;
        let root = parent.join(root.file_name().ok_or("update_root_invalid")?);
        if !root.exists() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .map_err(|_| "update_cache_create_failed")?;
        }
        let meta = fs::symlink_metadata(&root).map_err(|_| "update_cache_unavailable")?;
        if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
            return Err("update_cache_insecure".into());
        }
        let lock_path = root.join("update.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&lock_path)
            .map_err(|_| "update_lock_unavailable")?;
        let meta = lock.metadata().map_err(|_| "update_lock_unavailable")?;
        if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
            return Err("update_lock_insecure".into());
        }
        lock.try_lock_exclusive().map_err(|_| "update_cache_busy")?;
        Ok(Self {
            root,
            identity,
            _lock: lock,
        })
    }

    fn state(&self) -> Result<State> {
        let state: State = read_json(&self.root.join("trust.json"))?;
        if state.schema != 1
            || state.identity != self.identity
            || (state.highest_sequence == 0) != state.metadata_sha256.is_none()
            || state
                .metadata_sha256
                .as_deref()
                .is_some_and(|d| !crate::is_digest(d))
            || state
                .staged_archive_sha256
                .as_deref()
                .is_some_and(|d| !crate::is_digest(d))
        {
            return Err("update_trust_identity_changed".into());
        }
        Ok(state)
    }
    fn save(&self, state: &State) -> Result<()> {
        atomic_json(&self.root.join("trust.json"), state)
    }

    fn check_installation(&self, installed: &Installation) -> Result<()> {
        if installed.profile != self.identity.profile
            || installed.product != self.identity.product
            || installed.target != self.identity.target
        {
            return Err("update_installation_identity_mismatch".into());
        }
        Ok(())
    }

    pub fn download(
        &self,
        installed: &Installation,
        envelope: &[u8],
        now: u64,
    ) -> Result<UpdateDecision> {
        self.download_with(installed, envelope, now, &HttpsTransport)
    }

    /// Explicit offline import uses identical signature, freshness and digest gates.
    /// A local path is never installation authority by itself.
    pub fn import(
        &self,
        installed: &Installation,
        envelope: &[u8],
        now: u64,
        archive: &Path,
    ) -> Result<UpdateDecision> {
        struct LocalArchive<'a>(&'a Path);
        impl Transport for LocalArchive<'_> {
            fn get(&self, _: &str) -> Result<Response> {
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                    .open(self.0)
                    .map_err(|_| "update_import_unavailable")?;
                let metadata = file.metadata().map_err(|_| "update_import_unavailable")?;
                if !metadata.is_file() {
                    return Err("update_import_invalid".into());
                }
                Ok(Response {
                    size: Some(metadata.len()),
                    body: Box::new(file),
                })
            }
        }
        self.download_with(installed, envelope, now, &LocalArchive(archive))
    }

    fn download_with(
        &self,
        installed: &Installation,
        envelope: &[u8],
        now: u64,
        transport: &dyn Transport,
    ) -> Result<UpdateDecision> {
        self.check_installation(installed)?;
        let decision = plan(installed, envelope, &self.identity.public_key, now)?;
        if matches!(decision, UpdateDecision::Disabled { .. }) {
            return Ok(decision);
        }
        let release = authenticate(envelope, &self.identity.public_key, now)?;
        let metadata_sha = format!("{:x}", Sha256::digest(envelope));
        let mut state = self.state()?;
        if release.sequence < state.highest_sequence
            || (release.sequence == state.highest_sequence
                && state.metadata_sha256.as_deref() != Some(&metadata_sha))
        {
            return Err("update_metadata_replayed_or_rebound".into());
        }
        // Persist freshness before I/O, allowing only exact-metadata retry after failure.
        state.highest_sequence = release.sequence;
        state.metadata_sha256 = Some(metadata_sha);
        state.staged_archive_sha256 = None;
        self.save(&state)?;
        if matches!(decision, UpdateDecision::Current { .. }) {
            return Ok(decision);
        }
        let destination = self.root.join(format!("{}.tar.gz", release.archive_sha256));
        if destination.exists() {
            regular(&destination)?;
            if digest(&destination)? != release.archive_sha256 {
                return Err("update_cached_archive_changed".into());
            }
        } else {
            let mut staged = tempfile::NamedTempFile::new_in(&self.root)
                .map_err(|_| "update_stage_unavailable")?;
            let deadline = Instant::now() + DEADLINE;
            let mut response = transport.get(&release.archive_url)?;
            transfer(
                &mut response,
                staged.as_file_mut(),
                &release.archive_sha256,
                MAX_ARCHIVE,
                deadline,
            )?;
            staged
                .as_file()
                .sync_all()
                .map_err(|_| "update_archive_sync_failed")?;
            staged
                .persist_noclobber(&destination)
                .map_err(|_| "update_archive_commit_failed")?;
            File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(|_| "update_archive_sync_failed")?;
        }
        // Retain the exact signed envelope, not a reserialized release.
        atomic_json(
            &self.root.join("staged.json"),
            &serde_json::json!({"envelope": STANDARD.encode(envelope)}),
        )?;
        state.staged_archive_sha256 = Some(release.archive_sha256);
        self.save(&state)?;
        Ok(decision)
    }

    /// Holds the update lock throughout verification and owning installer activation.
    /// No caller-provided path or executable is accepted as the staged artifact.
    pub fn with_ready<T>(
        &self,
        installed: &Installation,
        now: u64,
        activate: impl FnOnce(ReadyArchive) -> Result<T>,
    ) -> Result<T> {
        self.check_installation(installed)?;
        if !matches!(installed.profile, UpdateProfile::Stable {}) {
            return Err("update_profile_disabled".into());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Staged {
            envelope: String,
        }
        let saved: Staged = read_json(&self.root.join("staged.json"))?;
        let envelope = STANDARD
            .decode(&saved.envelope)
            .map_err(|_| "update_staged_metadata_changed")?;
        let state = self.state()?;
        if state.metadata_sha256.as_deref() != Some(&format!("{:x}", Sha256::digest(&envelope))) {
            return Err("update_staged_metadata_changed".into());
        }
        let release = authenticate(&envelope, &self.identity.public_key, now)?;
        if !matches!(
            plan(installed, &envelope, &self.identity.public_key, now)?,
            UpdateDecision::Stage { .. } | UpdateDecision::Current { .. }
        ) || state.staged_archive_sha256.as_deref() != Some(&release.archive_sha256)
            || state.highest_sequence != release.sequence
        {
            return Err("update_stage_not_ready".into());
        }
        let archive = self.root.join(format!("{}.tar.gz", release.archive_sha256));
        regular(&archive)?;
        if digest(&archive)? != release.archive_sha256 {
            return Err("update_cached_archive_changed".into());
        }
        activate(ReadyArchive { archive, release })
    }
}

trait Transport {
    fn get(&self, url: &str) -> Result<Response>;
}
struct Response {
    size: Option<u64>,
    body: Box<dyn Read>,
}
struct HttpsTransport;

fn redirect_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

impl Transport for HttpsTransport {
    fn get(&self, url: &str) -> Result<Response> {
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(DEADLINE)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() > 5 || !redirect_allowed(attempt.url()) {
                    attempt.error("update redirect denied")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| "update_transport_unavailable")?;
        let response = client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .map_err(|_| "update_download_failed")?;
        if response.status() != reqwest::StatusCode::OK
            || response
                .headers()
                .get(reqwest::header::CONTENT_ENCODING)
                .is_some_and(|v| v != "identity")
        {
            return Err("update_download_response_invalid".into());
        }
        Ok(Response {
            size: response.content_length(),
            body: Box::new(response),
        })
    }
}

fn transfer(
    response: &mut Response,
    output: &mut File,
    expected: &str,
    max: u64,
    deadline: Instant,
) -> Result<()> {
    if response.size.is_some_and(|size| size > max) {
        return Err("update_archive_limit".into());
    }
    let mut bytes = [0u8; 65_536];
    let mut total = 0u64;
    let mut hash = Sha256::new();
    loop {
        if Instant::now() >= deadline {
            return Err("update_download_deadline".into());
        }
        let count = response
            .body
            .read(&mut bytes)
            .map_err(|_| "update_download_interrupted")?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or("update_archive_limit")?;
        if total > max {
            return Err("update_archive_limit".into());
        }
        hash.update(&bytes[..count]);
        output
            .write_all(&bytes[..count])
            .map_err(|_| "update_archive_write_failed")?;
    }
    if response.size.is_some_and(|size| size != total)
        || format!("{:x}", hash.finalize()) != expected
    {
        return Err("update_archive_integrity_failed".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::SignedBinaryRelease;
    use ed25519_dalek::{Signer, SigningKey};
    use std::{cell::Cell, collections::BTreeMap, io::Cursor};

    struct MemoryTransport {
        bytes: Vec<u8>,
        calls: Cell<usize>,
        fail: bool,
    }
    impl Transport for MemoryTransport {
        fn get(&self, _: &str) -> Result<Response> {
            self.calls.set(self.calls.get() + 1);
            if self.fail {
                return Err("fixture_offline".into());
            }
            Ok(Response {
                size: Some(self.bytes.len() as u64),
                body: Box::new(Cursor::new(self.bytes.clone())),
            })
        }
    }
    fn transport(bytes: &[u8]) -> MemoryTransport {
        MemoryTransport {
            bytes: bytes.to_vec(),
            calls: Cell::new(0),
            fail: false,
        }
    }
    fn fixture(parent: &Path) -> (TrustIdentity, Installation, BinaryRelease, SigningKey) {
        let key = SigningKey::from_bytes(&[7; 32]);
        let profile = UpdateProfile::Stable {};
        let identity = TrustIdentity {
            installation_root: parent.canonicalize().unwrap(),
            product: "core-bundle".into(),
            target: "aarch64-apple-darwin".into(),
            profile: profile.clone(),
            public_key: key.verifying_key().to_bytes(),
        };
        let installed = Installation {
            profile,
            product: identity.product.clone(),
            target: identity.target.clone(),
            version: "0.2.0".into(),
            state_compatibility: "f2-local-v1".into(),
            accepted_sequence: 1,
            archive_sha256: "e".repeat(64),
            bundle_manifest_sha256: "f".repeat(64),
        };
        let release = BinaryRelease {
            schema_version: 1,
            product: identity.product.clone(),
            target: identity.target.clone(),
            channel: "stable".into(),
            version: "0.3.0".into(),
            sequence: 2,
            issued_at: 100,
            expires_at: 200,
            state_compatibility: "f2-local-v1".into(),
            archive_url:
                "https://github.com/TradeAssembly/Core/releases/download/v0.3.0/core.tar.gz".into(),
            archive_sha256: format!("{:x}", Sha256::digest(b"archive")),
            bundle_manifest_sha256: "b".repeat(64),
            components: BTreeMap::from([("core".into(), "c".repeat(64))]),
        };
        (identity, installed, release, key)
    }
    fn signed(release: &BinaryRelease, key: &SigningKey) -> Vec<u8> {
        let payload = serde_json::to_vec(release).unwrap();
        let message = [
            b"tradeassembly.binary-update.v1\0".as_slice(),
            payload.as_slice(),
        ]
        .concat();
        serde_json::to_vec(&SignedBinaryRelease {
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(key.sign(&message).to_bytes()),
        })
        .unwrap()
    }
    #[test]
    fn scheduled_check_fetches_only_the_bound_feed_and_authenticated_archive() {
        struct Feed {
            envelope: Vec<u8>,
            calls: Cell<usize>,
        }
        impl Transport for Feed {
            fn get(&self, url: &str) -> Result<Response> {
                self.calls.set(self.calls.get() + 1);
                let bytes = if url.ends_with(".json") {
                    assert_eq!(url,"https://github.com/TradeAssembly/Core/releases/latest/download/tradeassembly-update-aarch64-apple-darwin.json");
                    self.envelope.clone()
                } else {
                    b"archive".to_vec()
                };
                Ok(Response {
                    size: Some(bytes.len() as u64),
                    body: Box::new(Cursor::new(bytes)),
                })
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let (identity, installed, release, key) = fixture(temp.path());
        let cache = UpdateCache::enroll(&temp.path().join("updates"), identity).unwrap();
        let feed = Feed {
            envelope: signed(&release, &key),
            calls: Cell::new(0),
        };
        assert!(matches!(
            cache.check_with(&installed, 150, &feed).unwrap(),
            UpdateDecision::Stage { .. }
        ));
        assert_eq!(feed.calls.get(), 2);
        let bad = Feed {
            envelope: b"untrusted metadata".to_vec(),
            calls: Cell::new(0),
        };
        assert!(cache.check_with(&installed, 150, &bad).is_err());
        assert_eq!(
            bad.calls.get(),
            1,
            "unsigned metadata never triggers artifact download"
        );
    }

    #[test]
    fn restart_preserves_authenticated_download_and_exact_retry() {
        let temp = tempfile::tempdir().unwrap();
        let (identity, installed, release, key) = fixture(temp.path());
        let path = temp.path().join("updates");
        let bytes = signed(&release, &key);
        let network = transport(b"archive");
        let cache = UpdateCache::enroll(&path, identity.clone()).unwrap();
        assert!(
            UpdateCache::open(&path, identity.clone()).is_err(),
            "one updater owns the cache"
        );
        cache
            .download_with(&installed, &bytes, 150, &network)
            .unwrap();
        assert_eq!(network.calls.get(), 1);
        drop(cache);
        let cache = UpdateCache::open(&path, identity.clone()).unwrap();
        cache
            .download_with(&installed, &bytes, 150, &network)
            .unwrap();
        assert_eq!(network.calls.get(), 1, "verified cache retry is offline");
        cache
            .with_ready(&installed, 150, |ready| {
                assert_eq!(fs::read(ready.path()).unwrap(), b"archive");
                assert_eq!(ready.release().components, release.components);
                Ok(())
            })
            .unwrap();
        drop(cache);
        let mut wrong = identity;
        wrong.public_key = [9; 32];
        assert!(
            UpdateCache::open(&path, wrong).is_err(),
            "cannot replace persisted trust"
        );
    }
    #[test]
    fn offline_attempt_records_freshness_and_rejects_rebinding() {
        let temp = tempfile::tempdir().unwrap();
        let (identity, installed, mut release, key) = fixture(temp.path());
        let path = temp.path().join("updates");
        let cache = UpdateCache::enroll(&path, identity.clone()).unwrap();
        let mut network = transport(b"archive");
        network.fail = true;
        let original = signed(&release, &key);
        assert!(cache
            .download_with(&installed, &original, 150, &network)
            .is_err());
        drop(cache);
        let cache = UpdateCache::open(&path, identity).unwrap();
        release.archive_sha256 = "a".repeat(64);
        assert!(cache
            .download_with(
                &installed,
                &signed(&release, &key),
                150,
                &transport(b"archive")
            )
            .is_err());
        release.sequence = 1;
        assert!(cache
            .download_with(
                &installed,
                &signed(&release, &key),
                150,
                &transport(b"archive")
            )
            .is_err());
        cache
            .download_with(&installed, &original, 150, &transport(b"archive"))
            .unwrap();
    }
    #[test]
    fn corruption_expiry_and_dev_profile_never_reach_activation() {
        let temp = tempfile::tempdir().unwrap();
        let (identity, mut installed, release, key) = fixture(temp.path());
        let cache = UpdateCache::enroll(&temp.path().join("updates"), identity).unwrap();
        cache
            .download_with(
                &installed,
                &signed(&release, &key),
                150,
                &transport(b"archive"),
            )
            .unwrap();
        assert!(cache
            .with_ready(&installed, 200, |_| -> Result<()> {
                panic!("expired metadata activated")
            })
            .is_err());
        fs::write(
            cache
                .root
                .join(format!("{}.tar.gz", release.archive_sha256)),
            b"corrupt",
        )
        .unwrap();
        assert!(cache
            .with_ready(&installed, 150, |_| -> Result<()> {
                panic!("corrupt bytes activated")
            })
            .is_err());
        installed.profile = UpdateProfile::Development {
            version: installed.version.clone(),
        };
        assert!(cache
            .download_with(&installed, b"", 150, &transport(b"archive"))
            .is_err());
        assert!(cache
            .with_ready(&installed, 150, |_| -> Result<()> {
                panic!("dev activated")
            })
            .is_err());
    }
    #[test]
    fn pinned_cache_never_downloads_and_unsafe_files_fail() {
        let temp = tempfile::tempdir().unwrap();
        let (mut identity, mut installed, _, _) = fixture(temp.path());
        installed.profile = UpdateProfile::Pinned {
            version: installed.version.clone(),
        };
        identity.profile = installed.profile.clone();
        let path = temp.path().join("updates");
        let cache = UpdateCache::enroll(&path, identity.clone()).unwrap();
        let network = transport(b"archive");
        assert!(matches!(
            cache.download_with(&installed, b"", 0, &network).unwrap(),
            UpdateDecision::Disabled { .. }
        ));
        assert_eq!(network.calls.get(), 0);
        assert!(matches!(
            cache.check_with(&installed, 0, &network).unwrap(),
            UpdateDecision::Disabled { .. }
        ));
        assert_eq!(network.calls.get(), 0);
        drop(cache);
        fs::remove_file(path.join("trust.json")).unwrap();
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, b"untouched").unwrap();
        std::os::unix::fs::symlink(&sentinel, path.join("trust.json")).unwrap();
        assert!(UpdateCache::open(&path, identity).is_err());
        assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
    }
    #[test]
    fn streaming_bounds_and_redirect_policy_fail_closed() {
        for (declared, bytes, max, expected) in [
            (Some(999), b"archive".as_slice(), 8, "update_archive_limit"),
            (None, b"archive".as_slice(), 3, "update_archive_limit"),
            (
                Some(10),
                b"archive".as_slice(),
                20,
                "update_archive_integrity_failed",
            ),
            (
                Some(7),
                b"corrupt".as_slice(),
                20,
                "update_archive_integrity_failed",
            ),
        ] {
            let mut response = Response {
                size: declared,
                body: Box::new(Cursor::new(bytes.to_vec())),
            };
            assert_eq!(
                transfer(
                    &mut response,
                    &mut tempfile::tempfile().unwrap(),
                    &format!("{:x}", Sha256::digest(b"archive")),
                    max,
                    Instant::now() + Duration::from_secs(1)
                )
                .unwrap_err(),
                expected
            );
        }
        for url in [
            "http://github.com/a",
            "https://evil.example/a",
            "https://github.com.evil.example/a",
            "https://user:password@github.com/a",
            "https://github.com/a#fragment",
            "https://127.0.0.1/a",
        ] {
            assert!(!redirect_allowed(&Url::parse(url).unwrap()), "{url}");
        }
        assert!(redirect_allowed(&Url::parse("https://release-assets.githubusercontent.com/github-production-release-asset/abc?signature=test").unwrap()));
    }
}
