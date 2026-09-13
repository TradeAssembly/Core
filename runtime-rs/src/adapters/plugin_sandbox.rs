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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static SETTINGS_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub struct SandboxRuntimePluginSandbox {
    command: PathBuf,
    settings_root: PathBuf,
    allow_local_egress: bool,
}

impl SandboxRuntimePluginSandbox {
    pub fn new(
        command: impl Into<PathBuf>,
        settings_root: impl Into<PathBuf>,
        allow_local_egress: bool,
    ) -> Result<Self, String> {
        let settings_root = settings_root.into();
        std::fs::create_dir_all(&settings_root)
            .map_err(|_| "plugin_sandbox_settings_unavailable".to_string())?;
        let settings_root = std::fs::canonicalize(settings_root)
            .map_err(|_| "plugin_sandbox_settings_unavailable".to_string())?;
        Ok(Self {
            command: command.into(),
            settings_root,
            allow_local_egress,
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
        let allowed_domains =
            normalized_domains(&request.allowed_domains, self.allow_local_egress)?;
        let home = std::env::var("HOME").ok().filter(|value| !value.is_empty());
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
        let bytes = serde_json::to_vec(&settings)
            .map_err(|_| "plugin_sandbox_policy_invalid".to_string())?;
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
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in ["HOME", "PATH", "TMPDIR", "SSL_CERT_FILE", "SSL_CERT_DIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        match command.spawn() {
            Ok(child) => Ok(SandboxedPluginProcess::new(child, Some(settings_path))),
            Err(_) => {
                let _ = std::fs::remove_file(settings_path);
                Err("plugin_sandbox_unavailable".to_string())
            }
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
    let source_tree =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/sandbox/node_modules/.bin/srt");
    if source_tree.is_file() {
        source_tree
    } else {
        PathBuf::from(configured)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        normalized_domains, resolve_executable, resolve_sandbox_command, PluginProcessSandboxPort,
        PluginSandboxRequest, SandboxRuntimePluginSandbox,
    };
    use serde_json::Value;
    use std::fs;
    use std::net::TcpListener;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

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
