// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    FailureMode, PluginProcessSandboxPort, PluginSandboxRequest, PortDescriptor, PortKind,
    SandboxedPluginProcess, VersionedPort,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static SETTINGS_SEQUENCE: AtomicU64 = AtomicU64::new(1);

// Generous limits bound validation work without changing the sandbox's policy.
pub(crate) const MAX_SANDBOX_DOMAINS: usize = 128;
pub(crate) const MAX_SANDBOX_DOMAIN_BYTES: usize = 1_024;
pub(crate) const MAX_SANDBOX_DOMAINS_BYTES: usize = 32 * 1024;
const MAX_SANDBOX_PATH_BYTES: usize = 8 * 1024;
const MAX_SANDBOX_SETTINGS_BYTES: usize = 64 * 1024;

fn check_domain_bounds(values: &[String]) -> Result<(), String> {
    if values.len() > MAX_SANDBOX_DOMAINS {
        return Err("plugin_sandbox_policy_invalid".into());
    }
    let mut bytes = 0usize;
    for value in values {
        if value.len() > MAX_SANDBOX_DOMAIN_BYTES {
            return Err("plugin_sandbox_policy_invalid".into());
        }
        bytes = bytes
            .checked_add(value.len())
            .ok_or("plugin_sandbox_policy_invalid")?;
        if bytes > MAX_SANDBOX_DOMAINS_BYTES {
            return Err("plugin_sandbox_policy_invalid".into());
        }
    }
    Ok(())
}

pub struct SandboxRuntimePluginSandbox {
    command: PathBuf,
    settings_root: PathBuf,
    allow_local_egress: bool,
    windows_proxy_port_range: Option<[u16; 2]>,
}

impl SandboxRuntimePluginSandbox {
    pub fn new(
        command: impl Into<PathBuf>,
        settings_root: impl Into<PathBuf>,
        allow_local_egress: bool,
        windows_proxy_port_range: Option<[u16; 2]>,
    ) -> Result<Self, String> {
        if let Some([lo, hi]) = windows_proxy_port_range {
            if lo == 0 || hi < lo || hi - lo > 64 {
                return Err("plugin_sandbox_windows_port_range_invalid".to_string());
            }
        }
        let settings_root = settings_root.into();
        std::fs::create_dir_all(&settings_root)
            .map_err(|_| "plugin_sandbox_settings_unavailable".to_string())?;
        let settings_root = std::fs::canonicalize(settings_root)
            .map_err(|_| "plugin_sandbox_settings_unavailable".to_string())?;
        Ok(Self {
            command: command.into(),
            settings_root,
            allow_local_egress,
            windows_proxy_port_range,
        })
    }

    fn settings_path(&self) -> PathBuf {
        self.settings_root.join(format!(
            "plugin-sandbox-{}-{}.json",
            std::process::id(),
            SETTINGS_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn write_settings(&self, request: &PluginSandboxRequest) -> Result<PathBuf, String> {
        check_domain_bounds(&request.allowed_domains)?;
        let allowed_domains =
            normalized_domains(&request.allowed_domains, self.allow_local_egress)?;
        let home = std::env::var("HOME").ok().filter(|value| !value.is_empty());
        if home
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SANDBOX_PATH_BYTES)
            || request.install_root.as_os_str().len() > MAX_SANDBOX_PATH_BYTES
        {
            return Err("plugin_sandbox_policy_invalid".into());
        }
        let deny_read = home.into_iter().collect::<Vec<_>>();
        let settings = json!({
            "network": {
                "allowedDomains": allowed_domains,
                "deniedDomains": [],
                "strictAllowlist": true,
                "allowUnixSockets": [],
                "allowLocalBinding": self.allow_local_egress
            },
            "filesystem": {
                "denyRead": deny_read,
                "allowRead": [request.install_root],
                "allowWrite": [],
                "denyWrite": []
            },
            "enableWeakerNestedSandbox": false,
            "enableWeakerNetworkIsolation": false,
            "allowAppleEvents": false
        });
        #[cfg(windows)]
        let settings = if let Some(range) = self.windows_proxy_port_range {
            let mut settings = settings;
            settings["windows"] = json!({"proxyPortRange": range});
            settings
        } else {
            settings
        };
        #[cfg(not(windows))]
        let _ = self.windows_proxy_port_range;
        let bytes = serde_json::to_vec(&settings)
            .map_err(|_| "plugin_sandbox_policy_invalid".to_string())?;
        if bytes.len() > MAX_SANDBOX_SETTINGS_BYTES {
            return Err("plugin_sandbox_policy_invalid".into());
        }
        let path = self.settings_path();
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|_| "plugin_sandbox_settings_unavailable".to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.flush())
            .map_err(|_| "plugin_sandbox_settings_unavailable".to_string())?;
        Ok(path)
    }
}

impl VersionedPort for SandboxRuntimePluginSandbox {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor =
            PortDescriptor::new(PortKind::Plugins, "sandbox-runtime.external-plugin-process")
                .for_profiles(&["local", "self_hosted"])
                .with_capabilities(&[
                    "plugin.process.spawn",
                    "plugin.network.default-deny",
                    "plugin.network.domain-allowlist",
                ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl PluginProcessSandboxPort for SandboxRuntimePluginSandbox {
    fn spawn(&self, request: &PluginSandboxRequest) -> Result<SandboxedPluginProcess, String> {
        check_domain_bounds(&request.allowed_domains)?;
        let (install_root, executable) = resolve_executable(request)?;
        let resolved_request = PluginSandboxRequest {
            executable: executable.clone(),
            install_root: install_root.clone(),
            allowed_domains: request.allowed_domains.clone(),
        };
        let settings_path = self.write_settings(&resolved_request)?;
        let mut command = Command::new(&self.command);
        command
            .arg("--settings")
            .arg(&settings_path)
            .arg(executable)
            .current_dir(install_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_sandbox_environment(&mut command);
        match command.spawn() {
            Ok(child) => Ok(SandboxedPluginProcess::new(child, Some(settings_path))),
            Err(_) => {
                let _ = std::fs::remove_file(settings_path);
                Err("plugin_sandbox_unavailable".to_string())
            }
        }
    }
}

fn configure_sandbox_environment(command: &mut Command) {
    command.env_clear();
    for name in ["HOME", "PATH", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(windows)]
    for name in [
        "SystemRoot",
        "WINDIR",
        "ProgramData",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "TEMP",
        "TMP",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn resolve_executable(request: &PluginSandboxRequest) -> Result<(PathBuf, PathBuf), String> {
    if !request.executable.is_file() || !request.install_root.is_dir() {
        return Err("plugin_sandbox_target_invalid".to_string());
    }
    let root = std::fs::canonicalize(&request.install_root)
        .map_err(|_| "plugin_sandbox_target_invalid".to_string())?;
    let executable = std::fs::canonicalize(&request.executable)
        .map_err(|_| "plugin_sandbox_target_invalid".to_string())?;
    if !executable.starts_with(&root) {
        return Err("plugin_sandbox_target_invalid".to_string());
    }
    Ok((root, executable))
}

fn normalized_domains(values: &[String], allow_local: bool) -> Result<Vec<String>, String> {
    let mut normalized = BTreeSet::new();
    for value in values {
        let candidate = value.trim().to_ascii_lowercase();
        if candidate.is_empty()
            || candidate.contains('*')
            || candidate.contains(':')
            || candidate.contains('/')
            || candidate.contains('@')
            || candidate.contains(' ')
            || is_forbidden_host(&candidate, allow_local)
        {
            return Err("plugin_sandbox_policy_invalid".to_string());
        }
        normalized.insert(candidate);
    }
    Ok(normalized.into_iter().collect())
}

fn is_forbidden_host(host: &str, allow_local: bool) -> bool {
    if host == "localhost" {
        return !allow_local;
    }
    host.parse::<IpAddr>()
        .is_ok_and(|address| is_forbidden_ip(address, allow_local))
}

fn is_forbidden_ip(address: IpAddr, allow_local: bool) -> bool {
    match address {
        IpAddr::V4(address) => {
            if allow_local && address.is_loopback() {
                return false;
            }
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_documentation()
                || address.is_unspecified()
                || address.is_multicast()
                || address == Ipv4Addr::new(100, 100, 100, 200)
                || address.octets()[0] == 0
                || address.octets()[0] >= 224
        }
        IpAddr::V6(address) => {
            if allow_local && address.is_loopback() {
                return false;
            }
            address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || is_ipv6_unique_local(address)
                || is_ipv6_link_local(address)
        }
    }
}

fn is_ipv6_unique_local(address: Ipv6Addr) -> bool {
    address.segments()[0] & 0xfe00 == 0xfc00
}

fn is_ipv6_link_local(address: Ipv6Addr) -> bool {
    address.segments()[0] & 0xffc0 == 0xfe80
}

pub fn resolve_sandbox_command(configured: &str) -> PathBuf {
    if configured != "srt" {
        return PathBuf::from(configured);
    }
    #[cfg(debug_assertions)]
    {
        let source_tree = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../packaging/sandbox/node_modules/.bin/srt");
        if source_tree.is_file() {
            return source_tree;
        }
    }
    PathBuf::from(configured)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::configure_sandbox_environment;
    #[cfg(unix)]
    use super::PluginProcessSandboxPort;
    use super::{
        check_domain_bounds, normalized_domains, resolve_executable, resolve_sandbox_command,
        PluginSandboxRequest, SandboxRuntimePluginSandbox, MAX_SANDBOX_DOMAINS,
        MAX_SANDBOX_DOMAINS_BYTES, MAX_SANDBOX_DOMAIN_BYTES,
    };
    use serde_json::Value;
    use std::fs;
    #[cfg(unix)]
    use std::net::TcpListener;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(windows)]
    use std::process::Command;
    #[cfg(unix)]
    use std::time::Duration;

    #[cfg(windows)]
    #[test]
    fn native_sandbox_launcher_receives_system_paths_without_node_injection() {
        let mut command = Command::new("unused-sandbox-launcher");
        configure_sandbox_environment(&mut command);
        let supplied: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        let system_root = std::env::var_os("SystemRoot").expect("Windows system root");
        assert_eq!(
            supplied
                .get(std::ffi::OsStr::new("SystemRoot"))
                .copied()
                .flatten(),
            Some(system_root.as_os_str())
        );
        assert!(!supplied.contains_key(std::ffi::OsStr::new("NODE_OPTIONS")));
        assert!(!supplied.contains_key(std::ffi::OsStr::new("NODE_PATH")));
    }

    #[test]
    fn domain_policy_is_exact_and_local_is_host_owned() {
        assert_eq!(
            normalized_domains(
                &[
                    "DATA.ALPACA.MARKETS".to_string(),
                    "paper-api.alpaca.markets".to_string(),
                ],
                false,
            )
            .expect("valid domains"),
            vec![
                "data.alpaca.markets".to_string(),
                "paper-api.alpaca.markets".to_string(),
            ]
        );
        for value in [
            "*.alpaca.markets",
            "https://alpaca.markets",
            "user@alpaca.markets",
            "localhost:8000",
            "127.0.0.1:8000",
            "10.0.0.1",
            "169.254.169.254",
        ] {
            assert!(normalized_domains(&[value.to_string()], false).is_err());
        }
        assert_eq!(
            normalized_domains(&["localhost".to_string()], true).expect("test override"),
            vec!["localhost".to_string()]
        );
        assert!(!resolve_sandbox_command("srt").as_os_str().is_empty());
    }

    #[test]
    fn domain_bounds_cover_raw_duplicates_individual_and_total_bytes() {
        let ordinary = vec![
            "data.alpaca.markets".to_string(),
            "paper-api.alpaca.markets".to_string(),
        ];
        assert!(check_domain_bounds(&ordinary).is_ok());
        assert!(check_domain_bounds(&vec!["example.com".into(); MAX_SANDBOX_DOMAINS]).is_ok());
        assert!(check_domain_bounds(&vec!["example.com".into(); MAX_SANDBOX_DOMAINS + 1]).is_err());
        assert!(check_domain_bounds(&["a".repeat(MAX_SANDBOX_DOMAIN_BYTES)]).is_ok());
        assert!(check_domain_bounds(&["a".repeat(MAX_SANDBOX_DOMAIN_BYTES + 1)]).is_err());
        let mut at_total = vec![
            "a".repeat(MAX_SANDBOX_DOMAIN_BYTES);
            MAX_SANDBOX_DOMAINS_BYTES / MAX_SANDBOX_DOMAIN_BYTES
        ];
        assert!(check_domain_bounds(&at_total).is_ok());
        at_total.push("b".into());
        assert!(check_domain_bounds(&at_total).is_err());
    }

    #[test]
    fn oversized_encoded_settings_and_paths_are_rejected_before_file_creation() {
        let directory = tempfile::tempdir().unwrap();
        let settings = directory.path().join("settings");
        let sandbox =
            SandboxRuntimePluginSandbox::new("unused-srt", &settings, false, None).unwrap();
        // Each raw domain fits the byte budget, but JSON escaping expands the settings.
        let domains = (0..MAX_SANDBOX_DOMAINS)
            .map(|i| format!("{i:03}{}a", "\u{1}".repeat(252)))
            .collect::<Vec<_>>();
        assert!(check_domain_bounds(&domains).is_ok());
        let mut request = PluginSandboxRequest {
            executable: "unused".into(),
            install_root: directory.path().into(),
            allowed_domains: domains,
        };
        assert!(
            matches!(sandbox.write_settings(&request), Err(error) if error == "plugin_sandbox_policy_invalid")
        );
        request.allowed_domains.clear();
        request.install_root = "x".repeat(super::MAX_SANDBOX_PATH_BYTES + 1).into();
        assert!(
            matches!(sandbox.write_settings(&request), Err(error) if error == "plugin_sandbox_policy_invalid")
        );
        assert_eq!(fs::read_dir(settings).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn oversized_direct_request_never_writes_settings_or_launches() {
        let directory = tempfile::tempdir().expect("temporary plugin");
        let executable = executable_script(&directory, "#!/bin/sh\nexit 0\n");
        let marker = directory.path().join("launched");
        let launcher = directory.path().join("srt");
        fs::write(
            &launcher,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&launcher).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&launcher, permissions).unwrap();
        let settings = directory.path().join("settings");
        let sandbox = SandboxRuntimePluginSandbox::new(launcher, &settings, false, None).unwrap();
        let request = PluginSandboxRequest {
            executable,
            install_root: directory.path().into(),
            allowed_domains: vec!["example.com".into(); MAX_SANDBOX_DOMAINS + 1],
        };
        assert!(
            matches!(sandbox.spawn(&request), Err(error) if error == "plugin_sandbox_policy_invalid")
        );
        assert!(!marker.exists());
        assert_eq!(fs::read_dir(settings).unwrap().count(), 0);
    }

    #[test]
    fn local_egress_override_reaches_the_os_sandbox_policy() {
        let directory = tempfile::tempdir().expect("temporary plugin");
        let executable = directory.path().join("plugin");
        fs::write(&executable, b"plugin").expect("write test plugin");
        let request = PluginSandboxRequest {
            executable,
            install_root: directory.path().to_path_buf(),
            allowed_domains: vec!["127.0.0.1".to_string()],
        };
        let sandbox = SandboxRuntimePluginSandbox::new(
            directory.path().join("unused-srt"),
            directory.path().join("settings"),
            true,
            None,
        )
        .expect("sandbox adapter");

        let settings_path = sandbox.write_settings(&request).expect("sandbox settings");
        assert!(settings_path.is_absolute());
        let settings: Value =
            serde_json::from_slice(&fs::read(settings_path).expect("read settings"))
                .expect("parse settings");

        assert_eq!(settings["network"]["allowLocalBinding"], true);
        assert_eq!(
            settings["network"]["allowedDomains"],
            serde_json::json!(["127.0.0.1"])
        );
    }

    #[test]
    fn sandbox_target_paths_are_canonical_before_spawn() {
        let directory = tempfile::tempdir().expect("temporary plugin");
        fs::create_dir(directory.path().join("nested")).expect("nested directory");
        let executable = directory.path().join("plugin");
        fs::write(&executable, b"plugin").expect("write test plugin");

        let (root, target) = resolve_executable(&PluginSandboxRequest {
            executable: directory.path().join("nested/../plugin"),
            install_root: directory.path().join("."),
            allowed_domains: Vec::new(),
        })
        .expect("resolved sandbox target");

        assert!(root.is_absolute());
        assert!(target.is_absolute());
        assert_eq!(
            root,
            fs::canonicalize(directory.path()).expect("canonical root")
        );
        assert_eq!(
            target,
            fs::canonicalize(executable).expect("canonical target")
        );
    }

    #[cfg(unix)]
    fn executable_script(directory: &tempfile::TempDir, contents: &str) -> std::path::PathBuf {
        let path = directory.path().join("plugin");
        fs::write(&path, contents).expect("write test plugin");
        let mut permissions = fs::metadata(&path).expect("plugin metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("make plugin executable");
        path
    }

    #[cfg(unix)]
    #[test]
    fn unavailable_sandbox_fails_closed() {
        let directory = tempfile::tempdir().expect("temporary plugin");
        let executable = executable_script(&directory, "#!/bin/sh\nexit 0\n");
        let sandbox = SandboxRuntimePluginSandbox::new(
            directory.path().join("missing-srt"),
            directory.path().join("settings"),
            false,
            None,
        )
        .expect("sandbox adapter");
        assert!(matches!(
            sandbox.spawn(&PluginSandboxRequest {
                executable,
                install_root: directory.path().to_path_buf(),
                allowed_domains: Vec::new(),
            }),
            Err(error) if error == "plugin_sandbox_unavailable"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn real_sandbox_denies_undeclared_loopback_egress() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("local listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("listener address");
        let directory = tempfile::tempdir().expect("temporary plugin");
        let executable = executable_script(
            &directory,
            &format!(
                "#!/bin/sh\nexec /usr/bin/curl --silent --show-error --max-time 1 http://{address}/\n"
            ),
        );
        let sandbox = SandboxRuntimePluginSandbox::new(
            resolve_sandbox_command("srt"),
            directory.path().join("settings"),
            false,
            None,
        )
        .expect("sandbox adapter");
        let mut child = sandbox
            .spawn(&PluginSandboxRequest {
                executable,
                install_root: directory.path().to_path_buf(),
                allowed_domains: Vec::new(),
            })
            .expect("sandbox process");
        let status = child.wait().expect("sandbox process status");
        assert!(!status.success());
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            listener.accept().is_err(),
            "loopback request escaped sandbox"
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires public network; run for plugin-sandbox acceptance evidence"]
    fn real_sandbox_allows_only_the_declared_public_domain() {
        let directory = tempfile::tempdir().expect("temporary plugin");
        let executable = executable_script(
            &directory,
            "#!/bin/sh\n/usr/bin/curl --fail --silent --show-error --max-time 5 https://example.com/ >/dev/null || exit 10\nif /usr/bin/curl --fail --silent --show-error --max-time 2 https://example.org/ >/dev/null 2>&1; then exit 20; fi\nexit 0\n",
        );
        let sandbox = SandboxRuntimePluginSandbox::new(
            resolve_sandbox_command("srt"),
            directory.path().join("settings"),
            false,
            None,
        )
        .expect("sandbox adapter");
        let mut child = sandbox
            .spawn(&PluginSandboxRequest {
                executable,
                install_root: directory.path().to_path_buf(),
                allowed_domains: vec!["example.com".to_string()],
            })
            .expect("sandbox process");
        assert!(child.wait().expect("sandbox process status").success());
    }
}
