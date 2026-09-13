// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    FailureMode, InstalledPluginPackage, PluginPackageInstallRequest, PluginPackagePort,
    PortDescriptor, PortKind, VersionedPort,
};
use flate2::read::GzDecoder;
use reqwest::{blocking::Client, redirect::Policy, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tar::Archive;
use tradeassembly_plugin_sdk::PluginPackageDescriptor;
use url::Url;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

const MAX_PACKAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PACKAGE_URL_BYTES: usize = 4 * 1024;
const MAX_PACKAGE_PATH_BYTES: usize = 2 * 1024;
const MAX_PACKAGE_REDIRECTS: usize = 5;
const PACKAGE_DESCRIPTOR: &str = "tradeassembly-plugin.json";
const INSTALLED_RECORD: &str = ".tradeassembly-installed.json";

trait PackageAddressResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String>;
}

struct SystemPackageAddressResolver;

impl PackageAddressResolver for SystemPackageAddressResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|_| "plugin_package_source_unavailable".to_string())?
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err("plugin_package_source_unavailable".to_string());
        }
        Ok(addresses)
    }
}

pub struct LocalPluginPackageStore {
    root: PathBuf,
}

impl LocalPluginPackageStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        fs::create_dir_all(root.join("packages"))
            .map_err(|_| "plugin_package_store_unavailable".to_string())?;
        fs::create_dir_all(root.join("staging"))
            .map_err(|_| "plugin_package_store_unavailable".to_string())?;
        let root =
            fs::canonicalize(root).map_err(|_| "plugin_package_store_unavailable".to_string())?;
        Ok(Self { root })
    }

    fn package_root(&self, digest: &str) -> PathBuf {
        self.root.join("packages").join(digest)
    }

    fn staging_path(&self, package_sha256: &str) -> PathBuf {
        self.root.join("staging").join(format!(
            "{}-{}-{}",
            package_sha256,
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn load_record(&self, package_root: &Path) -> Result<InstalledPluginPackage, String> {
        let bytes = fs::read(package_root.join(INSTALLED_RECORD))
            .map_err(|_| "plugin_package_record_missing".to_string())?;
        let mut record: InstalledPluginPackage = serde_json::from_slice(&bytes)
            .map_err(|_| "plugin_package_record_invalid".to_string())?;
        let descriptor_bytes = fs::read(package_root.join(PACKAGE_DESCRIPTOR))
            .map_err(|_| "plugin_package_descriptor_missing".to_string())?;
        let descriptor: PluginPackageDescriptor = serde_json::from_slice(&descriptor_bytes)
            .map_err(|_| "plugin_package_descriptor_invalid".to_string())?;
        descriptor
            .validate()
            .map_err(|_| "plugin_package_descriptor_invalid".to_string())?;
        let target = target_record(&descriptor)?;
        let manifest_path = package_root.join(&descriptor.manifest.path);
        ensure_within(package_root, &manifest_path)?;
        let manifest_bytes =
            fs::read(&manifest_path).map_err(|_| "plugin_package_manifest_missing".to_string())?;
        let manifest: Value = serde_yaml::from_slice(&manifest_bytes)
            .map_err(|_| "plugin_package_manifest_invalid".to_string())?;
        let executable_path = package_root.join(&target.binary.path);
        ensure_within(package_root, &executable_path)?;
        let executable_bytes = fs::read(&executable_path)
            .map_err(|_| "plugin_package_executable_missing".to_string())?;
        let canonical_manifest_sha256 = canonical_sha256(&manifest)?;
        let descriptor_value: Value = serde_json::from_slice(&descriptor_bytes)
            .map_err(|_| "plugin_package_descriptor_invalid".to_string())?;
        if descriptor.manifest.sha256 != sha256(&manifest_bytes)
            || target.binary.sha256 != sha256(&executable_bytes)
            || descriptor.plugin.id != record.plugin_ref
            || descriptor.plugin.version != record.version
            || manifest["metadata"]["id"] != record.plugin_ref
            || manifest["metadata"]["version"] != record.version
            || canonical_manifest_sha256 != record.manifest_sha256
            || descriptor.compatibility.host_contract.minimum != record.host_contract
            || descriptor.compatibility.host_contract.maximum != record.host_contract
            || descriptor.compatibility.sdk_version != record.sdk_version
            || descriptor_value != record.descriptor
            || manifest != record.manifest
        {
            return Err("plugin_package_integrity_invalid".to_string());
        }
        verify_response_schemas(&descriptor, package_root)?;
        record.install_root = package_root.to_path_buf();
        record.executable_path = executable_path;
        Ok(record)
    }

    fn read_source(&self, request: &PluginPackageInstallRequest) -> Result<Vec<u8>, String> {
        match request.source_type.as_str() {
            "file" => {
                let metadata = fs::metadata(&request.locator)
                    .map_err(|_| "plugin_package_source_unavailable".to_string())?;
                if metadata.len() > MAX_PACKAGE_BYTES as u64 {
                    return Err("plugin_package_too_large".to_string());
                }
                fs::read(&request.locator)
                    .map_err(|_| "plugin_package_source_unavailable".to_string())
            }
            "https" if !request.locator.starts_with("https://") => {
                Err("plugin_package_source_invalid".to_string())
            }
            "https" if !request.offline => {
                let locator = request.locator.clone();
                std::thread::spawn(move || fetch_https_package(&locator))
                    .join()
                    .map_err(|_| "plugin_package_source_unavailable".to_string())?
            }
            "https" => Err("plugin_package_offline_cache_miss".to_string()),
            _ => Err("plugin_package_source_invalid".to_string()),
        }
    }

    fn extract(
        &self,
        bytes: &[u8],
        package_sha256: &str,
    ) -> Result<InstalledPluginPackage, String> {
        let destination = self.package_root(package_sha256);
        if destination.is_dir() {
            return self.load_record(&destination);
        }
        let staging = self.staging_path(package_sha256);
        if staging.exists() {
            fs::remove_dir_all(&staging)
                .map_err(|_| "plugin_package_staging_failed".to_string())?;
        }
        fs::create_dir(&staging).map_err(|_| "plugin_package_staging_failed".to_string())?;

        let result = (|| {
            let decoder = GzDecoder::new(Cursor::new(bytes));
            let mut archive = Archive::new(decoder);
            let mut expanded = 0_u64;
            let mut paths = std::collections::BTreeSet::new();
            for entry in archive
                .entries()
                .map_err(|_| "plugin_package_archive_invalid".to_string())?
            {
                let mut entry = entry.map_err(|_| "plugin_package_archive_invalid".to_string())?;
                if !entry.header().entry_type().is_file() {
                    return Err("plugin_package_archive_entry_forbidden".to_string());
                }
                let path = entry
                    .path()
                    .map_err(|_| "plugin_package_archive_invalid".to_string())?
                    .into_owned();
                validate_relative_path(&path)?;
                if !paths.insert(path.clone()) {
                    return Err("plugin_package_archive_duplicate_path".to_string());
                }
                expanded = expanded
                    .checked_add(entry.size())
                    .ok_or_else(|| "plugin_package_too_large".to_string())?;
                if expanded > MAX_EXPANDED_BYTES {
                    return Err("plugin_package_too_large".to_string());
                }
                let output = staging.join(&path);
                ensure_within(&staging, &output)?;
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|_| "plugin_package_staging_failed".to_string())?;
                }
                let mut file = File::create(&output)
                    .map_err(|_| "plugin_package_staging_failed".to_string())?;
                std::io::copy(&mut entry, &mut file)
                    .map_err(|_| "plugin_package_staging_failed".to_string())?;
                file.flush()
                    .map_err(|_| "plugin_package_staging_failed".to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
                    fs::set_permissions(&output, fs::Permissions::from_mode(mode))
                        .map_err(|_| "plugin_package_staging_failed".to_string())?;
                }
            }

            let descriptor_bytes = fs::read(staging.join(PACKAGE_DESCRIPTOR))
                .map_err(|_| "plugin_package_descriptor_missing".to_string())?;
            let descriptor_contract: PluginPackageDescriptor =
                serde_json::from_slice(&descriptor_bytes)
                    .map_err(|_| "plugin_package_descriptor_invalid".to_string())?;
            descriptor_contract
                .validate()
                .map_err(|_| "plugin_package_descriptor_invalid".to_string())?;
            let descriptor: Value = serde_json::from_slice(&descriptor_bytes)
                .map_err(|_| "plugin_package_descriptor_invalid".to_string())?;
            let plugin_ref = descriptor_contract.plugin.id.clone();
            let version = descriptor_contract.plugin.version.clone();
            let manifest_path = descriptor_contract.manifest.path.clone();
            let manifest_sha256 = descriptor_contract.manifest.sha256.clone();
            let host_contract = descriptor_contract
                .compatibility
                .host_contract
                .minimum
                .clone();
            if descriptor_contract.compatibility.host_contract.minimum
                != descriptor_contract.compatibility.host_contract.maximum
                || host_contract != tradeassembly_plugin_sdk::WIRE_CONTRACT_VERSION
            {
                return Err("plugin_package_host_contract_unsupported".to_string());
            }
            let sdk_version = descriptor_contract.compatibility.sdk_version.clone();
            if sdk_version != tradeassembly_plugin_sdk::SDK_VERSION {
                return Err("plugin_package_sdk_version_unsupported".to_string());
            }
            let target = target_record(&descriptor_contract)?;
            let executable_path = target.binary.path.clone();
            let manifest_file = staging.join(&manifest_path);
            ensure_within(&staging, &manifest_file)?;
            let manifest_bytes = fs::read(&manifest_file)
                .map_err(|_| "plugin_package_manifest_missing".to_string())?;
            if sha256(&manifest_bytes) != manifest_sha256 {
                return Err("plugin_package_manifest_digest_mismatch".to_string());
            }
            let manifest: Value = serde_yaml::from_slice(&manifest_bytes)
                .map_err(|_| "plugin_package_manifest_invalid".to_string())?;
            if manifest["metadata"]["id"] != plugin_ref
                || manifest["metadata"]["version"] != version
            {
                return Err("plugin_package_identity_mismatch".to_string());
            }
            let executable = staging.join(&executable_path);
            ensure_within(&staging, &executable)?;
            let executable_bytes = fs::read(&executable)
                .map_err(|_| "plugin_package_executable_missing".to_string())?;
            let expected_executable_sha = target.binary.sha256.clone();
            if sha256(&executable_bytes) != expected_executable_sha {
                return Err("plugin_package_executable_digest_mismatch".to_string());
            }
            verify_response_schemas(&descriptor_contract, &staging)?;

            let record = InstalledPluginPackage {
                plugin_ref,
                version,
                package_sha256: package_sha256.to_string(),
                manifest_sha256: canonical_sha256(&manifest)?,
                manifest,
                descriptor,
                install_root: PathBuf::new(),
                executable_path: PathBuf::from(executable_path),
                host_contract,
                sdk_version,
            };
            let record_bytes = serde_json::to_vec(&record)
                .map_err(|_| "plugin_package_record_invalid".to_string())?;
            fs::write(staging.join(INSTALLED_RECORD), record_bytes)
                .map_err(|_| "plugin_package_staging_failed".to_string())?;
            match fs::rename(&staging, &destination) {
                Ok(()) => {}
                Err(_) if destination.is_dir() => {
                    fs::remove_dir_all(&staging)
                        .map_err(|_| "plugin_package_staging_failed".to_string())?;
                }
                Err(_) => return Err("plugin_package_install_failed".to_string()),
            }
            self.load_record(&destination)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }
}

fn verify_response_schemas(
    descriptor: &PluginPackageDescriptor,
    package_root: &Path,
) -> Result<(), String> {
    for schema in &descriptor.response_schemas {
        let path = package_root.join(&schema.document.path);
        ensure_within(package_root, &path)?;
        let bytes =
            fs::read(path).map_err(|_| "plugin_package_response_schema_missing".to_string())?;
        if sha256(&bytes) != schema.document.sha256 {
            return Err("plugin_package_response_schema_digest_mismatch".to_string());
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "plugin_package_response_schema_invalid".to_string())?;
        jsonschema::validator_for(&value)
            .map_err(|_| "plugin_package_response_schema_invalid".to_string())?;
    }
    Ok(())
}

fn fetch_https_package(locator: &str) -> Result<Vec<u8>, String> {
    fetch_https_package_with_resolver(locator, &SystemPackageAddressResolver)
}

fn fetch_https_package_with_resolver(
    locator: &str,
    resolver: &dyn PackageAddressResolver,
) -> Result<Vec<u8>, String> {
    let mut url = parse_package_url(locator)?;
    for redirect_count in 0..=MAX_PACKAGE_REDIRECTS {
        let (host, addresses) = resolve_package_destination(&url, resolver)?;
        let client = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(60))
            .resolve_to_addrs(&host, &addresses)
            .build()
            .map_err(|_| "plugin_package_source_unavailable".to_string())?;
        let mut response = client
            .get(url.clone())
            .send()
            .map_err(|_| "plugin_package_source_unavailable".to_string())?;
        if response.status().is_redirection() {
            url = next_redirect(
                &url,
                response.status(),
                response.headers().get("location"),
                redirect_count,
            )?;
            continue;
        }
        if !response.status().is_success() {
            return Err("plugin_package_source_unavailable".to_string());
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_PACKAGE_BYTES as u64)
        {
            return Err("plugin_package_too_large".to_string());
        }
        return read_package_body(&mut response, MAX_PACKAGE_BYTES);
    }
    Err("plugin_package_redirect_limit_exceeded".to_string())
}

fn parse_package_url(locator: &str) -> Result<Url, String> {
    if locator.len() > MAX_PACKAGE_URL_BYTES {
        return Err("plugin_package_source_invalid".to_string());
    }
    let url = Url::parse(locator).map_err(|_| "plugin_package_source_invalid".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
        || url.path().len() > MAX_PACKAGE_PATH_BYTES
    {
        return Err("plugin_package_source_invalid".to_string());
    }
    Ok(url)
}

fn resolve_package_destination(
    url: &Url,
    resolver: &dyn PackageAddressResolver,
) -> Result<(String, Vec<SocketAddr>), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "plugin_package_source_invalid".to_string())?;
    let addresses = resolver.resolve(host, 443)?;
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_public_address(address.ip()))
    {
        return Err("plugin_package_source_invalid".to_string());
    }
    Ok((host.to_string(), addresses))
}

fn redirect_target(
    current: &Url,
    status: StatusCode,
    location: Option<&reqwest::header::HeaderValue>,
) -> Result<Url, String> {
    if !status.is_redirection() {
        return Err("plugin_package_source_unavailable".to_string());
    }
    let location = location
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "plugin_package_source_invalid".to_string())?;
    let target = current
        .join(location)
        .map_err(|_| "plugin_package_source_invalid".to_string())?;
    parse_package_url(target.as_str())
}

fn next_redirect(
    current: &Url,
    status: StatusCode,
    location: Option<&reqwest::header::HeaderValue>,
    redirect_count: usize,
) -> Result<Url, String> {
    if redirect_count >= MAX_PACKAGE_REDIRECTS {
        return Err("plugin_package_redirect_limit_exceeded".to_string());
    }
    redirect_target(current, status, location)
}

fn read_package_body(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "plugin_package_source_unavailable".to_string())?;
    if bytes.len() > limit {
        return Err("plugin_package_too_large".to_string());
    }
    Ok(bytes)
}

fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    !address.is_loopback()
        && !address.is_private()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_unspecified()
        && !matches!(
            octets,
            [0, ..]
                | [100, 64..=127, ..]
                | [192, 0, 0, ..]
                | [192, 0, 2, ..]
                | [192, 88, 99, ..]
                | [198, 18..=19, ..]
                | [198, 51, 100, ..]
                | [203, 0, 113, ..]
                | [224..=255, ..]
        )
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(address) = address.to_ipv4_mapped() {
        return is_public_ipv4(address);
    }
    let segments = address.segments();
    !(address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || segments[0] == 0x2001 && segments[1] == 0x0db8
        || segments[0] == 0x0100 && segments[1..].iter().all(|segment| *segment == 0))
}

impl VersionedPort for LocalPluginPackageStore {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Plugins, "local.plugin-package-store")
            .for_profiles(&["local", "self_hosted"])
            .with_capabilities(&[
                "plugin.package.install",
                "plugin.package.load",
                "plugin.package.remove",
            ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl PluginPackagePort for LocalPluginPackageStore {
    fn install(
        &self,
        request: &PluginPackageInstallRequest,
    ) -> Result<InstalledPluginPackage, String> {
        validate_digest(&request.package_sha256)?;
        validate_digest(&request.manifest_sha256)?;
        if let Some(installed) = self.get(&request.package_sha256)? {
            if installed.manifest_sha256 != request.manifest_sha256 {
                return Err("plugin_package_manifest_digest_mismatch".to_string());
            }
            return Ok(installed);
        }
        let bytes = self.read_source(request)?;
        if sha256(&bytes) != request.package_sha256 {
            return Err("plugin_package_digest_mismatch".to_string());
        }
        let installed = self.extract(&bytes, &request.package_sha256)?;
        if installed.manifest_sha256 != request.manifest_sha256 {
            return Err("plugin_package_manifest_digest_mismatch".to_string());
        }
        Ok(installed)
    }

    fn get(&self, package_sha256: &str) -> Result<Option<InstalledPluginPackage>, String> {
        validate_digest(package_sha256)?;
        let root = self.package_root(package_sha256);
        if !root.is_dir() {
            return Ok(None);
        }
        self.load_record(&root).map(Some)
    }

    fn remove(&self, package_sha256: &str) -> Result<bool, String> {
        validate_digest(package_sha256)?;
        let root = self.package_root(package_sha256);
        if !root.exists() {
            return Ok(false);
        }
        fs::remove_dir_all(root).map_err(|_| "plugin_package_remove_failed".to_string())?;
        Ok(true)
    }
}

fn validate_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("plugin_package_archive_path_forbidden".to_string());
    }
    Ok(())
}

fn ensure_within(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "plugin_package_archive_path_forbidden".to_string())?;
    validate_relative_path(relative)
}

fn target_record(
    descriptor: &PluginPackageDescriptor,
) -> Result<&tradeassembly_plugin_sdk::PackageTarget, String> {
    let target = tradeassembly_plugin_sdk::host_target();
    descriptor
        .targets
        .iter()
        .find(|record| record.target == target)
        .ok_or_else(|| "plugin_package_target_unsupported".to_string())
}

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err("plugin_package_digest_invalid".to_string())
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_sha256(value: &Value) -> Result<String, String> {
    serde_json_canonicalizer::to_vec(value)
        .map(|bytes| sha256(&bytes))
        .map_err(|_| "plugin_package_manifest_invalid".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_sha256, is_public_address, next_redirect, parse_package_url, read_package_body,
        redirect_target, resolve_package_destination, sha256, validate_digest,
        validate_relative_path, LocalPluginPackageStore, PackageAddressResolver,
    };
    use crate::ports::{PluginPackageInstallRequest, PluginPackagePort};
    use flate2::{write::GzEncoder, Compression};
    use reqwest::{header::HeaderValue, StatusCode};
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::path::Path;
    use tar::{Builder, Header};

    struct FixtureResolver(Vec<SocketAddr>);

    impl PackageAddressResolver for FixtureResolver {
        fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<SocketAddr>, String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn rejects_archive_traversal_and_noncanonical_digests() {
        assert!(validate_relative_path(Path::new("bin/a")).is_ok());
        assert!(validate_relative_path(Path::new("../escape")).is_err());
        assert!(validate_relative_path(Path::new("/absolute")).is_err());
        assert!(validate_digest(&"a".repeat(64)).is_ok());
        assert!(validate_digest(&"A".repeat(64)).is_err());
        assert!(validate_digest("sha256:abc").is_err());
    }

    #[test]
    fn same_process_installs_use_distinct_staging_paths() {
        let directory = tempfile::tempdir().expect("package test directory");
        let store =
            LocalPluginPackageStore::new(directory.path().join("store")).expect("package store");
        let digest = "a".repeat(64);

        assert_ne!(store.staging_path(&digest), store.staging_path(&digest));
    }

    #[test]
    fn installs_digest_addressed_package_idempotently_and_offline() {
        let directory = tempfile::tempdir().expect("package test directory");
        let manifest = br#"apiVersion: tradeassembly.org/v1alpha1
kind: TradeAssemblyPluginManifest
manifestVersion: "0.1"
metadata:
  id: example.external
  name: Example
  version: 0.1.0
  provider:
    id: example
    name: Example
"#;
        let executable = b"fixture-executable";
        let response_schema =
            br#"{"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}}}"#;
        let target = tradeassembly_plugin_sdk::host_target();
        let descriptor = serde_json::to_vec(&json!({
            "packageContractVersion": "1",
            "plugin": {"id": "example.external", "version": "0.1.0"},
            "manifest": {"path": "manifest.yaml", "sha256": sha256(manifest)},
            "targets": [{
                "target": target,
                "binary": {"path": "bin/example", "sha256": sha256(executable)},
            }],
            "responseSchemas": [{
                "schemaRef": "schema://example.external/response@1",
                "document": {
                    "path": "schemas/response.json",
                    "sha256": sha256(response_schema)
                }
            }],
            "compatibility": {
                "hostContract": {"minimum": "1", "maximum": "1"},
                "sdkVersion": "0.1.0",
            },
        }))
        .expect("serialize descriptor");
        let archive = archive(&[
            ("tradeassembly-plugin.json", &descriptor, 0o644),
            ("manifest.yaml", manifest, 0o644),
            ("bin/example", executable, 0o755),
            ("schemas/response.json", response_schema, 0o644),
        ]);
        let archive_path = directory.path().join("example.tar.gz");
        fs::write(&archive_path, &archive).expect("write package archive");
        let package_sha256 = sha256(&archive);
        let request = PluginPackageInstallRequest {
            source_type: "file".to_string(),
            locator: archive_path.to_string_lossy().to_string(),
            package_sha256: package_sha256.clone(),
            manifest_sha256: canonical_sha256(
                &serde_yaml::from_slice(manifest).expect("parse manifest"),
            )
            .expect("canonical manifest digest"),
            offline: false,
        };
        let store =
            LocalPluginPackageStore::new(directory.path().join("store")).expect("package store");
        let installed = store.install(&request).expect("install package");
        assert_eq!(installed.plugin_ref, "example.external");
        assert_eq!(installed.package_sha256, package_sha256);
        assert!(installed.executable_path.is_file());
        assert_eq!(
            store.install(&request).expect("idempotent install"),
            installed
        );
        fs::remove_file(&archive_path).expect("remove source archive");
        assert_eq!(
            store
                .install(&request)
                .expect("online setup reuses digest cache"),
            installed
        );
        let mut offline = request;
        offline.offline = true;
        assert_eq!(
            store.install(&offline).expect("offline cached install"),
            installed
        );
    }

    #[test]
    fn rejects_post_install_executable_tampering() {
        let directory = tempfile::tempdir().expect("package test directory");
        let manifest = br#"apiVersion: tradeassembly.org/v1alpha1
kind: TradeAssemblyPluginManifest
manifestVersion: "0.1"
metadata:
  id: example.external
  name: Example
  version: 0.1.0
  provider:
    id: example
    name: Example
"#;
        let executable = b"fixture-executable";
        let target = tradeassembly_plugin_sdk::host_target();
        let descriptor = serde_json::to_vec(&json!({
            "packageContractVersion": "1",
            "plugin": {"id": "example.external", "version": "0.1.0"},
            "manifest": {"path": "manifest.yaml", "sha256": sha256(manifest)},
            "targets": [{
                "target": target,
                "binary": {"path": "bin/example", "sha256": sha256(executable)},
            }],
            "responseSchemas": [],
            "compatibility": {
                "hostContract": {"minimum": "1", "maximum": "1"},
                "sdkVersion": "0.1.0",
            },
        }))
        .expect("serialize descriptor");
        let archive = archive(&[
            ("tradeassembly-plugin.json", &descriptor, 0o644),
            ("manifest.yaml", manifest, 0o644),
            ("bin/example", executable, 0o755),
        ]);
        let archive_path = directory.path().join("example.tar.gz");
        fs::write(&archive_path, &archive).expect("write package archive");
        let package_sha256 = sha256(&archive);
        let store =
            LocalPluginPackageStore::new(directory.path().join("store")).expect("package store");
        let installed = store
            .install(&PluginPackageInstallRequest {
                source_type: "file".to_string(),
                locator: archive_path.to_string_lossy().to_string(),
                package_sha256: package_sha256.clone(),
                manifest_sha256: canonical_sha256(
                    &serde_yaml::from_slice(manifest).expect("parse manifest"),
                )
                .expect("canonical manifest digest"),
                offline: false,
            })
            .expect("install package");

        fs::write(&installed.executable_path, b"tampered").expect("tamper executable");
        assert_eq!(
            store
                .get(&package_sha256)
                .expect_err("tampered package must fail closed"),
            "plugin_package_integrity_invalid"
        );
    }

    #[test]
    fn rejects_package_digest_mismatch_without_publishing() {
        let directory = tempfile::tempdir().expect("package test directory");
        let archive_path = directory.path().join("invalid.tar.gz");
        fs::write(&archive_path, b"not an archive").expect("write invalid package");
        let store =
            LocalPluginPackageStore::new(directory.path().join("store")).expect("package store");
        let error = store
            .install(&PluginPackageInstallRequest {
                source_type: "file".to_string(),
                locator: archive_path.to_string_lossy().to_string(),
                package_sha256: "a".repeat(64),
                manifest_sha256: "b".repeat(64),
                offline: false,
            })
            .expect_err("digest mismatch must fail");
        assert_eq!(error, "plugin_package_digest_mismatch");
        assert!(store.get(&"a".repeat(64)).expect("lookup").is_none());
    }

    #[test]
    fn rejects_unencrypted_remote_source_before_fetch() {
        let directory = tempfile::tempdir().expect("package test directory");
        let store =
            LocalPluginPackageStore::new(directory.path().join("store")).expect("package store");
        let error = store
            .install(&PluginPackageInstallRequest {
                source_type: "https".to_string(),
                locator: "http://example.invalid/plugin.tar.gz".to_string(),
                package_sha256: "a".repeat(64),
                manifest_sha256: "b".repeat(64),
                offline: false,
            })
            .expect_err("unencrypted package transport must fail");
        assert_eq!(error, "plugin_package_source_invalid");
        assert!(store.get(&"a".repeat(64)).expect("lookup").is_none());
    }

    #[test]
    fn package_urls_are_strict_and_credential_free() {
        for valid in [
            "https://packages.example/plugin.tar.gz",
            "https://packages.example/dir/plugin.tar.gz",
        ] {
            assert!(parse_package_url(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "http://packages.example/plugin.tar.gz",
            "https://user@packages.example/plugin.tar.gz",
            "https://packages.example:8443/plugin.tar.gz",
            "https:",
            "https://packages.example/plugin.tar.gz?token=secret",
            "https://packages.example/plugin.tar.gz#fragment",
            &format!("https://packages.example/{}", "a".repeat(2049)),
        ] {
            assert_eq!(
                parse_package_url(invalid).expect_err("invalid package URL"),
                "plugin_package_source_invalid",
                "{invalid}"
            );
        }
    }

    #[test]
    fn package_destination_rejects_any_non_public_answer() {
        let url = parse_package_url("https://packages.example/plugin.tar.gz").expect("package URL");
        let public = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 443);
        assert!(resolve_package_destination(&url, &FixtureResolver(vec![public])).is_ok());

        for forbidden in [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)),
        ] {
            assert!(!is_public_address(forbidden), "{forbidden}");
            assert_eq!(
                resolve_package_destination(
                    &url,
                    &FixtureResolver(vec![public, SocketAddr::new(forbidden, 443)]),
                )
                .expect_err("mixed DNS answers must fail"),
                "plugin_package_source_invalid"
            );
        }
    }

    #[test]
    fn redirects_are_reparsed_and_revalidated_before_following() {
        let current = parse_package_url("https://packages.example/first.tar.gz").expect("URL");
        let target = redirect_target(
            &current,
            StatusCode::FOUND,
            Some(&HeaderValue::from_static(
                "https://metadata.example/next.tar.gz",
            )),
        )
        .expect("redirect URL");
        assert_eq!(target.host_str(), Some("metadata.example"));
        assert_eq!(
            resolve_package_destination(
                &target,
                &FixtureResolver(vec![SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
                    443,
                )]),
            )
            .expect_err("unsafe redirect must fail before a target request"),
            "plugin_package_source_invalid"
        );
        assert_eq!(
            redirect_target(
                &current,
                StatusCode::FOUND,
                Some(&HeaderValue::from_static(
                    "http://packages.example/next.tar.gz"
                )),
            )
            .expect_err("redirect scheme must be HTTPS"),
            "plugin_package_source_invalid"
        );
        assert_eq!(
            next_redirect(
                &current,
                StatusCode::FOUND,
                Some(&HeaderValue::from_static("/next.tar.gz")),
                5,
            )
            .expect_err("redirect limit must apply per hop"),
            "plugin_package_redirect_limit_exceeded"
        );
    }

    #[test]
    fn package_body_is_streamed_with_a_hard_limit() {
        let mut within_limit = std::io::Cursor::new(vec![7_u8; 8]);
        assert_eq!(
            read_package_body(&mut within_limit, 8).expect("body"),
            vec![7; 8]
        );
        let mut oversized = std::io::Cursor::new(vec![7_u8; 9]);
        assert_eq!(
            read_package_body(&mut oversized, 8).expect_err("oversized body"),
            "plugin_package_too_large"
        );
    }

    fn archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut builder = Builder::new(&mut encoder);
            for (path, bytes, mode) in entries {
                let mut header = Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(*mode);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, *bytes)
                    .expect("append package entry");
            }
            builder.finish().expect("finish package archive");
        }
        encoder.flush().expect("flush package archive");
        encoder.finish().expect("finish gzip")
    }
}
