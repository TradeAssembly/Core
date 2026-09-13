// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MANIFEST_PATH: &str = "foss-core-publication.yaml";
const MANIFEST_SCHEMA: &str = "tradeassembly.foss_core_publication.v1";
const REPOSITORY: &str = "TradeAssembly/Core";
const PUBLICATION_CLASS: &str = "public_core";
const MAX_SCANNED_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub(crate) const APACHE_2_LICENSE_SHA256: &str =
    "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30";
const REQUIRED_NOTICE_MARKERS: &[&str] = &[
    "TradeAssembly Core",
    "Copyright 2026 OptionLab LLC",
    "Apache-2.0",
    "Third-party components retain their own licenses and attribution notices",
];

const REQUIRED_FORBIDDEN_PATHS: &[&str] =
    &["apps/www/", "packages/hosted-", "packages/hub-", "site/"];

const REQUIRED_FORBIDDEN_MARKERS: &[&str] = &[
    "WORKOS_API_KEY",
    "WORKOS_COOKIE_PASSWORD",
    "WORKOS_CLIENT_ID",
    "WORKOS_REDIRECT_URI",
    "WORKOS_AUTHKIT_DOMAIN",
    "POSTHOG_PROJECT_API_KEY",
    "POSTHOG_HOST",
    "TRADEASSEMBLY_TELEMETRY_RELAY_SECRET",
    "TRADEASSEMBLY_TELEMETRY_RELAY_URL",
    "@workos-inc/",
    "posthog-js",
    "api.workos.com",
    "i.posthog.com",
];

const REQUIRED_FORBIDDEN_DEPENDENCY_FRAGMENTS: &[&str] = &[
    "../TradeAssembly-Product",
    "/TradeAssembly-Product/",
    "TRADEASSEMBLY_PRODUCT_PATH",
];

const ARCHIVE_RUNTIME_TIMEOUT: Duration = Duration::from_secs(30);
const ARCHIVE_BUILD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationManifest {
    schema_version: String,
    repository: String,
    publication_class: String,
    public_path_prefixes: Vec<String>,
    public_files: Vec<String>,
    metadata_only_path_prefixes: Vec<String>,
    metadata_only_files: Vec<String>,
    forbidden_path_prefixes: Vec<String>,
    forbidden_implementation_markers: Vec<String>,
    forbidden_dependency_path_fragments: Vec<String>,
    license: LicensePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LicensePolicy {
    current_status: String,
    publication_ready: bool,
    decision_owner: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BoundaryReport {
    schema_version: &'static str,
    status: &'static str,
    repository: &'static str,
    publication_class: &'static str,
    scanned_files: usize,
    archive_smoke: &'static str,
    public_license_ready: bool,
    license_status: String,
    failures: Vec<String>,
}

pub fn run(args: &[String], root: &Path) -> i32 {
    let archive_smoke = match parse_args(args) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let (mut report, manifest) = scan(root);
    if report.failures.is_empty() && archive_smoke {
        if let Err(error) = run_archive_smoke(root) {
            report.failures.push(error);
        }
    }
    finalize_report(&mut report, manifest.as_ref(), archive_smoke);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serialize FOSS Core boundary report")
    );
    if report.failures.is_empty() {
        0
    } else {
        1
    }
}

fn finalize_report(
    report: &mut BoundaryReport,
    manifest: Option<&PublicationManifest>,
    archive_smoke: bool,
) {
    if let Some(manifest) = manifest {
        report.license_status = manifest.license.current_status.clone();
    }
    report.archive_smoke = if archive_smoke {
        if report.failures.is_empty() {
            "passed"
        } else {
            "failed"
        }
    } else {
        "not_requested"
    };
    report.status = if report.failures.is_empty() {
        "passed"
    } else {
        "failed"
    };
    report.public_license_ready = report.failures.is_empty()
        && manifest
            .as_ref()
            .is_some_and(|value| value.license.publication_ready);
}

fn parse_args(args: &[String]) -> Result<bool, String> {
    let mut archive_smoke = false;
    for arg in args {
        match arg.as_str() {
            "--archive-smoke" => archive_smoke = true,
            "--format" | "json" | "--json" => {}
            other => return Err(format!("unknown foss-core-boundary option: {other}")),
        }
    }
    Ok(archive_smoke)
}

fn scan(root: &Path) -> (BoundaryReport, Option<PublicationManifest>) {
    let mut report = BoundaryReport {
        schema_version: MANIFEST_SCHEMA,
        status: "failed",
        repository: REPOSITORY,
        publication_class: PUBLICATION_CLASS,
        scanned_files: 0,
        archive_smoke: "not_requested",
        public_license_ready: false,
        license_status: "manifest_unavailable".to_string(),
        failures: Vec::new(),
    };
    let manifest = match load_manifest(root) {
        Ok(value) => value,
        Err(error) => {
            report.failures.push(error);
            return (report, None);
        }
    };
    validate_manifest(&manifest, &mut report.failures);
    validate_license_files(root, &mut report.failures);
    let tracked = match tracked_files(root) {
        Ok(value) => value,
        Err(error) => {
            report.failures.push(error);
            return (report, Some(manifest));
        }
    };
    report.scanned_files = tracked.len();
    for relative in tracked {
        scan_path(root, &relative, &manifest, &mut report.failures);
    }
    validate_cargo_metadata(root, &manifest, &mut report.failures);
    (report, Some(manifest))
}

fn load_manifest(root: &Path) -> Result<PublicationManifest, String> {
    let path = root.join(MANIFEST_PATH);
    let bytes = fs::read(&path)
        .map_err(|error| format!("read publication manifest {}: {error}", path.display()))?;
    serde_yaml::from_slice(&bytes)
        .map_err(|error| format!("parse publication manifest {}: {error}", path.display()))
}

fn validate_manifest(manifest: &PublicationManifest, failures: &mut Vec<String>) {
    require_equal(
        &manifest.schema_version,
        MANIFEST_SCHEMA,
        "manifest schemaVersion",
        failures,
    );
    require_equal(
        &manifest.repository,
        REPOSITORY,
        "manifest repository",
        failures,
    );
    require_equal(
        &manifest.publication_class,
        PUBLICATION_CLASS,
        "manifest publicationClass",
        failures,
    );
    require_contains(
        &manifest.forbidden_path_prefixes,
        REQUIRED_FORBIDDEN_PATHS,
        "forbiddenPathPrefixes",
        failures,
    );
    require_contains(
        &manifest.forbidden_implementation_markers,
        REQUIRED_FORBIDDEN_MARKERS,
        "forbiddenImplementationMarkers",
        failures,
    );
    require_contains(
        &manifest.forbidden_dependency_path_fragments,
        REQUIRED_FORBIDDEN_DEPENDENCY_FRAGMENTS,
        "forbiddenDependencyPathFragments",
        failures,
    );
    require_equal(
        &manifest.license.current_status,
        "Apache-2.0",
        "license currentStatus",
        failures,
    );
    if !manifest.license.publication_ready {
        failures.push("license publicationReady must be true".to_string());
    }
    require_equal(
        &manifest.license.decision_owner,
        "human",
        "license decisionOwner",
        failures,
    );
}

fn validate_license_files(root: &Path, failures: &mut Vec<String>) {
    let license_path = root.join("LICENSE");
    match fs::read(&license_path) {
        Ok(bytes) => {
            let actual = format!("{:x}", Sha256::digest(&bytes));
            if actual != APACHE_2_LICENSE_SHA256 {
                failures.push(format!(
                    "LICENSE sha256 {actual} does not match canonical Apache-2.0 hash"
                ));
            }
        }
        Err(error) => failures.push(format!("read LICENSE {}: {error}", license_path.display())),
    }

    let notice_path = root.join("NOTICE");
    match fs::read_to_string(&notice_path) {
        Ok(notice) => {
            let notice = notice.to_ascii_lowercase();
            for marker in REQUIRED_NOTICE_MARKERS {
                if !notice.contains(&marker.to_ascii_lowercase()) {
                    failures.push(format!("NOTICE is missing required text {marker:?}"));
                }
            }
        }
        Err(error) => failures.push(format!("read NOTICE {}: {error}", notice_path.display())),
    }
}

fn require_equal(actual: &str, expected: &str, label: &str, failures: &mut Vec<String>) {
    if actual != expected {
        failures.push(format!("{label} must be {expected:?}, got {actual:?}"));
    }
}

fn require_contains(actual: &[String], required: &[&str], label: &str, failures: &mut Vec<String>) {
    for required_value in required {
        if !actual.iter().any(|value| value == required_value) {
            failures.push(format!(
                "{label} is missing required value {required_value:?}"
            ));
        }
    }
}

fn tracked_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .map_err(|error| format!("run git ls-files: {error}"))?;
    if !output.status.success() {
        return Err(command_failure("git ls-files", &output));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
        .collect())
}

fn scan_path(
    root: &Path,
    relative: &Path,
    manifest: &PublicationManifest,
    failures: &mut Vec<String>,
) {
    let normalized = normalize(relative);
    if manifest
        .forbidden_path_prefixes
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        failures.push(format!(
            "private Product path is forbidden in Core: {normalized}"
        ));
        return;
    }
    let listed = manifest.public_files.iter().any(|path| path == &normalized)
        || manifest
            .public_path_prefixes
            .iter()
            .any(|prefix| normalized.starts_with(prefix));
    if !listed {
        failures.push(format!(
            "tracked path is not classified by the publication manifest: {normalized}"
        ));
        return;
    }
    if is_metadata_only(&normalized, manifest) || !is_scannable_text_path(relative) {
        return;
    }
    let path = root.join(relative);
    let Ok(metadata) = fs::metadata(&path) else {
        failures.push(format!("cannot stat classified path: {normalized}"));
        return;
    };
    if !metadata.is_file() {
        return;
    }
    if metadata.len() > MAX_SCANNED_FILE_BYTES {
        failures.push(format!(
            "scannable publication file exceeds {} bytes: {normalized}",
            MAX_SCANNED_FILE_BYTES
        ));
        return;
    }
    let Ok(bytes) = fs::read(&path) else {
        failures.push(format!("cannot read classified path: {normalized}"));
        return;
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return;
    };
    for marker in &manifest.forbidden_implementation_markers {
        if text.contains(marker) {
            failures.push(format!(
                "Product-only implementation marker {marker:?} found in {normalized}"
            ));
        }
    }
    for fragment in &manifest.forbidden_dependency_path_fragments {
        if text.contains(fragment) {
            failures.push(format!(
                "Product dependency fragment {fragment:?} found in {normalized}"
            ));
        }
    }
}

fn is_metadata_only(relative: &str, manifest: &PublicationManifest) -> bool {
    if manifest
        .metadata_only_files
        .iter()
        .any(|path| path == relative)
    {
        return true;
    }
    let in_metadata_tree = manifest
        .metadata_only_path_prefixes
        .iter()
        .any(|prefix| relative.starts_with(prefix));
    in_metadata_tree && is_descriptive_metadata_path(Path::new(relative))
}

fn is_descriptive_metadata_path(path: &Path) -> bool {
    const DESCRIPTIVE_EXTENSIONS: &[&str] = &["csv", "md", "svg", "txt"];
    const IMMUTABLE_EVIDENCE_EXTENSIONS: &[&str] =
        &["csv", "json", "jsonl", "md", "svg", "txt", "yaml", "yml"];
    let extension = path.extension().and_then(OsStr::to_str);
    let normalized = normalize(path);
    if normalized.starts_with(".sdlc/evidence/") || normalized.starts_with(".sdlc/imports/") {
        extension.is_some_and(|value| IMMUTABLE_EVIDENCE_EXTENSIONS.contains(&value))
    } else {
        extension.is_some_and(|value| DESCRIPTIVE_EXTENSIONS.contains(&value))
    }
}

fn is_scannable_text_path(path: &Path) -> bool {
    const FILENAMES: &[&str] = &[
        "Cargo.lock",
        "Cargo.toml",
        "Dockerfile",
        "Justfile",
        "package-lock.json",
        "package.json",
        "render.yaml",
    ];
    const EXTENSIONS: &[&str] = &[
        "conf", "css", "csv", "env", "graphql", "hcl", "html", "ini", "js", "json", "jsx", "md",
        "mjs", "rs", "sh", "sql", "svg", "toml", "ts", "tsx", "txt", "yaml", "yml",
    ];
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| FILENAMES.contains(&name))
        || path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| EXTENSIONS.contains(&extension))
}

fn validate_cargo_metadata(
    root: &Path,
    manifest: &PublicationManifest,
    failures: &mut Vec<String>,
) {
    let output = match Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .env_remove("TRADEASSEMBLY_CORE_PATH")
        .env_remove("TRADEASSEMBLY_PRODUCT_PATH")
        .output()
    {
        Ok(value) => value,
        Err(error) => {
            failures.push(format!("run cargo metadata: {error}"));
            return;
        }
    };
    if !output.status.success() {
        failures.push(command_failure("cargo metadata", &output));
        return;
    }
    let value: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(error) => {
            failures.push(format!("parse cargo metadata: {error}"));
            return;
        }
    };
    let canonical_root = match fs::canonicalize(root) {
        Ok(value) => value,
        Err(error) => {
            failures.push(format!("canonicalize repository root: {error}"));
            return;
        }
    };
    let mut checked = BTreeSet::new();
    let Some(packages) = value.get("packages").and_then(|value| value.as_array()) else {
        failures.push("cargo metadata omitted packages".to_string());
        return;
    };
    for package in packages {
        if let Some(path) = package
            .get("manifest_path")
            .and_then(|value| value.as_str())
        {
            validate_metadata_path(path, &canonical_root, &mut checked, failures);
        }
        if let Some(dependencies) = package
            .get("dependencies")
            .and_then(|value| value.as_array())
        {
            for dependency in dependencies {
                if let Some(path) = dependency.get("path").and_then(|value| value.as_str()) {
                    validate_metadata_path(path, &canonical_root, &mut checked, failures);
                    for fragment in &manifest.forbidden_dependency_path_fragments {
                        if path.contains(fragment) {
                            failures.push(format!(
                                "cargo dependency uses Product path fragment {fragment:?}: {path}"
                            ));
                        }
                    }
                }
            }
        }
    }
}

fn validate_metadata_path(
    raw: &str,
    canonical_root: &Path,
    checked: &mut BTreeSet<String>,
    failures: &mut Vec<String>,
) {
    if !checked.insert(raw.to_string()) {
        return;
    }
    let path = Path::new(raw);
    let canonical = if path.is_file() {
        fs::canonicalize(path)
    } else {
        fs::canonicalize(path.parent().unwrap_or(path))
    };
    match canonical {
        Ok(path) if path.starts_with(canonical_root) => {}
        Ok(path) => failures.push(format!(
            "Cargo path dependency escapes Core repository: {}",
            path.display()
        )),
        Err(error) => failures.push(format!("canonicalize Cargo path {raw}: {error}")),
    }
}

fn run_archive_smoke(root: &Path) -> Result<(), String> {
    ensure_clean_repository(root)?;
    let revision = git_output(root, &["rev-parse", "HEAD"])?;
    let temp = tempfile::tempdir().map_err(|error| format!("create archive workspace: {error}"))?;
    let archive_path = temp.path().join("tradeassembly.tar");
    let source_path = temp.path().join("source");
    fs::create_dir(&source_path)
        .map_err(|error| format!("create archive source directory: {error}"))?;
    let output = Command::new("git")
        .args([
            "archive",
            "--format=tar",
            "-o",
            archive_path
                .to_str()
                .ok_or_else(|| "archive path is not UTF-8".to_string())?,
            "--prefix=core/",
            &revision,
        ])
        .current_dir(root)
        .output()
        .map_err(|error| format!("run git archive: {error}"))?;
    if !output.status.success() {
        return Err(command_failure("git archive", &output));
    }
    let bytes = fs::read(&archive_path)
        .map_err(|error| format!("read generated source archive: {error}"))?;
    tar::Archive::new(Cursor::new(bytes))
        .unpack(&source_path)
        .map_err(|error| format!("extract generated source archive: {error}"))?;
    let source_root = source_path.join("core");
    if !source_root.join("Cargo.toml").is_file() {
        return Err("archived Core source has no Cargo.toml at archive root".to_string());
    }
    validate_archive_paths(&source_root)?;

    run_isolated(
        &source_root,
        temp.path(),
        "cargo metadata",
        "cargo",
        &["metadata", "--locked", "--no-deps", "--format-version", "1"],
        ARCHIVE_BUILD_TIMEOUT,
        &revision,
    )?;
    run_isolated(
        &source_root,
        temp.path(),
        "cargo build --workspace --all-targets",
        "cargo",
        &["build", "--locked", "--workspace", "--all-targets"],
        ARCHIVE_BUILD_TIMEOUT,
        &revision,
    )?;
    run_archive_runtime_smoke(&source_root, temp.path(), &revision)
}

fn run_archive_runtime_smoke(source: &Path, temp: &Path, revision: &str) -> Result<(), String> {
    let binary = temp.join("target/debug/tradeassembly");
    run_runtime_command(&binary, source, temp, revision, &["--help"])?;
    let state = temp.join("runtime-state");
    fs::create_dir_all(&state).map_err(|error| format!("create private runtime state: {error}"))?;
    let db = state.join("tradeassembly.db");
    let db = db
        .to_str()
        .ok_or_else(|| "archive database path is not UTF-8".to_string())?;
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"tradeassembly.setup.status\",\"arguments\":{\"surface\":\"agent\"}}}\n"
    );
    let output = run_runtime_protocol(&binary, source, temp, revision, db, input)?;
    let lines = output.lines().collect::<Vec<_>>();
    if lines.len() != 3 {
        return Err(format!(
            "Core MCP protocol returned {} lines, expected 3",
            lines.len()
        ));
    }
    for (index, line) in lines.iter().enumerate() {
        let response: serde_json::Value =
            serde_json::from_str(line).map_err(|_| "Core MCP returned invalid JSON".to_string())?;
        if response["jsonrpc"] != "2.0"
            || response["id"] != (index + 1)
            || response.get("error").is_some()
            || response["result"]["isError"] == true
        {
            return Err("Core MCP response binding/error validation failed".to_string());
        }
    }
    let initialize: serde_json::Value = serde_json::from_str(lines[0])
        .map_err(|error| format!("parse initialize response: {error}"))?;
    if initialize["result"]["protocolVersion"] != "2025-11-25" {
        return Err("Core MCP initialize did not negotiate protocol 2025-11-25".to_string());
    }
    let tools: serde_json::Value = serde_json::from_str(lines[1])
        .map_err(|error| format!("parse tools/list response: {error}"))?;
    let has_setup = tools["result"]["tools"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["name"] == "tradeassembly.setup.status")
    });
    if !has_setup {
        return Err("Core MCP tools/list omitted tradeassembly.setup.status".to_string());
    }
    let setup: serde_json::Value = serde_json::from_str(lines[2])
        .map_err(|error| format!("parse setup discovery response: {error}"))?;
    if setup["result"]["structuredContent"]["acceptance"]["requestedSurface"] != "agent" {
        return Err("Core setup discovery did not preserve agent surface".to_string());
    }
    Ok(())
}

struct ProcessGuard {
    child: Child,
    log_path: PathBuf,
    stderr_path: PathBuf,
}

impl ProcessGuard {
    fn spawn(mut command: Command, log_path: &Path, label: &str) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let stdout = File::create(log_path)
            .map_err(|error| format!("create {label} archive log: {error}"))?;
        let stderr = stdout
            .try_clone()
            .map_err(|error| format!("clone {label} archive log: {error}"))?;
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|error| format!("start archived {label}: {error}"))?;
        Ok(Self {
            child,
            log_path: log_path.to_path_buf(),
            stderr_path: log_path.to_path_buf(),
        })
    }

    fn spawn_with_stdin(
        mut command: Command,
        log_path: &Path,
        label: &str,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let stdout = File::create(log_path)
            .map_err(|error| format!("create {label} archive log: {error}"))?;
        let stderr_path = log_path.with_extension("stderr.log");
        let stderr = File::create(&stderr_path)
            .map_err(|error| format!("create {label} stderr log: {error}"))?;
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|error| format!("start archived {label}: {error}"))?;
        Ok(Self {
            child,
            log_path: log_path.to_path_buf(),
            stderr_path,
        })
    }

    fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    fn wait(&mut self, label: &str, timeout: Duration) -> Result<Output, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if [&self.log_path, &self.stderr_path]
                .iter()
                .any(|path| fs::metadata(path).is_ok_and(|metadata| metadata.len() > 1024 * 1024))
            {
                return Err(format!("{label} exceeded 1 MiB output cap"));
            }
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| format!("inspect {label}: {error}"))?
            {
                if status.success() {
                    return Ok(Output {
                        status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    });
                }
                return Err(format!(
                    "{label} failed with {status}: {}",
                    log_tail(&self.stderr_path)
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{label} timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // Each child owns a fresh process group; terminate build grandchildren too.
        #[cfg(unix)]
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &format!("-{}", self.child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn run_isolated(
    source: &Path,
    temp: &Path,
    label: &str,
    program: &str,
    args: &[&str],
    timeout: Duration,
    revision: &str,
) -> Result<(), String> {
    let mut command = Command::new(program);
    configure_isolated_command(&mut command, source, temp, args, revision);
    let mut process = ProcessGuard::spawn(command, &temp.join("archive-command.log"), label)?;
    process.wait(label, timeout).map(|_| ())
}

fn run_runtime_command(
    binary: &Path,
    source: &Path,
    temp: &Path,
    revision: &str,
    args: &[&str],
) -> Result<(), String> {
    let mut command = Command::new(binary);
    configure_runtime_environment(&mut command, source, temp, revision);
    command.args(args);
    let mut process =
        ProcessGuard::spawn(command, &temp.join("archive-runtime.log"), "Core --help")?;
    process
        .wait("Core --help", ARCHIVE_RUNTIME_TIMEOUT)
        .map(|_| ())
}

fn run_runtime_protocol(
    binary: &Path,
    source: &Path,
    temp: &Path,
    revision: &str,
    db: &str,
    input: &str,
) -> Result<String, String> {
    let mut command = Command::new(binary);
    configure_runtime_environment(&mut command, source, temp, revision);
    command.args(["--db", db, "mcp", "serve", "--transport", "stdio"]);
    let log_path = temp.join("archive-mcp.log");
    let mut process = ProcessGuard::spawn_with_stdin(command, &log_path, "Core MCP")?;
    process
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| "Core MCP stdin unavailable".to_string())?
        .write_all(input.as_bytes())
        .map_err(|error| format!("write Core MCP protocol: {error}"))?;
    process.wait("Core MCP", ARCHIVE_RUNTIME_TIMEOUT)?;
    let mut bytes =
        fs::read(&log_path).map_err(|error| format!("read Core MCP output: {error}"))?;
    if bytes.len() > 1024 * 1024 {
        return Err("Core MCP output exceeded 1 MiB cap".to_string());
    }
    if let Some(last) = bytes.last() {
        if *last == b'\n' {
            bytes.pop();
        }
    }
    String::from_utf8(bytes).map_err(|error| format!("decode Core MCP output: {error}"))
}

fn configure_isolated_command(
    command: &mut Command,
    source: &Path,
    temp: &Path,
    args: &[&str],
    revision: &str,
) {
    command
        .args(args)
        .current_dir(source)
        .env("CARGO_TARGET_DIR", temp.join("target"));
    configure_isolated_environment(command, source, temp);
    command.env("TRADEASSEMBLY_CORE_REVISION", revision);
}

fn log_tail(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else {
        return "no diagnostic output".to_string();
    };
    let start = bytes.len().saturating_sub(4096);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

fn configure_isolated_environment(command: &mut Command, source: &Path, temp: &Path) {
    command
        .env_clear()
        .current_dir(source)
        .env("CARGO_TARGET_DIR", temp.join("target"))
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        );
    for name in ["CARGO_HOME", "RUSTUP_HOME"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn configure_runtime_environment(
    command: &mut Command,
    source: &Path,
    temp: &Path,
    revision: &str,
) {
    command
        .env_clear()
        .current_dir(source)
        .env("HOME", temp.join("home"))
        .env("TMPDIR", temp)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("TRADEASSEMBLY_CORE_REVISION", revision);
}

fn ensure_clean_repository(root: &Path) -> Result<(), String> {
    let output = git_output(root, &["status", "--porcelain", "--untracked-files=all"])?;
    if output.is_empty() {
        Ok(())
    } else {
        Err(
            "archive smoke requires a clean repository, including nonignored untracked files"
                .to_string(),
        )
    }
}

fn validate_archive_paths(source: &Path) -> Result<(), String> {
    let canonical_root =
        fs::canonicalize(source).map_err(|error| format!("canonicalize archive root: {error}"))?;
    fn visit(path: &Path, root: &Path) -> Result<(), String> {
        let canonical = fs::canonicalize(path)
            .map_err(|error| format!("canonicalize archived path: {error}"))?;
        if !canonical.starts_with(root) {
            return Err(format!(
                "archived path escapes Core repository: {}",
                canonical.display()
            ));
        }
        if canonical.is_dir() {
            for child in
                fs::read_dir(path).map_err(|error| format!("read archived directory: {error}"))?
            {
                visit(
                    &child
                        .map_err(|error| format!("read archived entry: {error}"))?
                        .path(),
                    root,
                )?;
            }
        }
        Ok(())
    }
    visit(source, &canonical_root)?;
    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|error| format!("run git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(command_failure(&format!("git {}", args.join(" ")), &output));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("decode git {} output: {error}", args.join(" ")))
}

fn command_failure(label: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail = stderr
        .lines()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    format!("{label} failed with {}:\n{tail}", output.status)
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn manifest() -> PublicationManifest {
        let mut manifest: PublicationManifest =
            serde_yaml::from_str(include_str!("../../foss-core-publication.yaml")).unwrap();
        manifest.license.current_status = "Apache-2.0".to_string();
        manifest.license.publication_ready = true;
        manifest
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::File::create(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn manifest_keeps_required_fail_closed_policy() {
        let manifest = manifest();
        let mut failures = Vec::new();
        validate_manifest(&manifest, &mut failures);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn rejects_private_product_path() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "apps/www/account.js",
            "export const account = true;",
        );
        let mut failures = Vec::new();
        scan_path(
            root.path(),
            Path::new("apps/www/account.js"),
            &manifest(),
            &mut failures,
        );
        assert!(failures
            .iter()
            .any(|failure| failure.contains("private Product path")));
    }

    #[test]
    fn rejects_product_only_marker_in_tests_and_assets() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "runtime-rs/tests/hosted_fixture.rs",
            "const secret = process.env.WORKOS_API_KEY;",
        );
        let mut failures = Vec::new();
        scan_path(
            root.path(),
            Path::new("runtime-rs/tests/hosted_fixture.rs"),
            &manifest(),
            &mut failures,
        );
        assert!(failures
            .iter()
            .any(|failure| failure.contains("WORKOS_API_KEY")));
    }

    #[test]
    fn rejects_product_checkout_dependency() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "scripts/check.sh",
            "cargo test --manifest-path ../TradeAssembly-Product/Cargo.toml",
        );
        let mut failures = Vec::new();
        scan_path(
            root.path(),
            Path::new("scripts/check.sh"),
            &manifest(),
            &mut failures,
        );
        assert!(failures
            .iter()
            .any(|failure| failure.contains("Product dependency fragment")));
    }

    #[test]
    fn permits_boundary_description_in_metadata() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "docs/repo-boundaries.md",
            "Product uses WORKOS_API_KEY; Core does not.",
        );
        let mut failures = Vec::new();
        scan_path(
            root.path(),
            Path::new("docs/repo-boundaries.md"),
            &manifest(),
            &mut failures,
        );
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn scans_active_config_and_executable_assets_inside_metadata_trees() {
        let root = tempfile::tempdir().unwrap();
        for relative in [
            ".sdlc/checks.yaml",
            "docs/planning/assets/generate.mjs",
            ".codex/config.toml",
            ".codex/workflows/routing.md",
            "docs/runtime-config.yaml",
            "packaging/release-config.json",
        ] {
            write(root.path(), relative, "const key = 'WORKOS_API_KEY';");
            let mut failures = Vec::new();
            scan_path(root.path(), Path::new(relative), &manifest(), &mut failures);
            assert!(
                failures
                    .iter()
                    .any(|failure| failure.contains("WORKOS_API_KEY")),
                "{relative}: {failures:#?}"
            );
        }
    }

    #[test]
    fn permits_descriptive_immutable_evidence() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            ".sdlc/evidence/roadmap/GC-X/receipt.json",
            r#"{"forbiddenExample":"WORKOS_API_KEY"}"#,
        );
        let mut failures = Vec::new();
        scan_path(
            root.path(),
            Path::new(".sdlc/evidence/roadmap/GC-X/receipt.json"),
            &manifest(),
            &mut failures,
        );
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn runtime_commands_use_private_minimal_environment() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let mut command = Command::new("true");
        configure_runtime_environment(&mut command, source.path(), target.path(), "abc123");
        let envs = command.get_envs().collect::<BTreeSet<_>>();
        assert!(envs
            .iter()
            .any(|(name, value)| *name == OsStr::new("HOME") && value.is_some()));
        assert!(envs.iter().any(|(name, value)| *name
            == OsStr::new("TRADEASSEMBLY_CORE_REVISION")
            && value.is_some_and(|v| v == OsStr::new("abc123"))));
        assert!(!envs
            .iter()
            .any(|(name, _)| *name == OsStr::new("WORKOS_API_KEY")));
    }

    #[cfg(unix)]
    #[test]
    fn archive_process_wait_reports_early_exit() {
        let temp = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "exit 7"]);
        let mut guard =
            ProcessGuard::spawn(command, &temp.path().join("early.log"), "fixture").unwrap();
        let error = guard.wait("fixture", Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("failed with"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn archive_guard_rejects_timeout_and_oversized_completed_output() {
        let temp = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let mut guard =
            ProcessGuard::spawn(command, &temp.path().join("timeout.log"), "fixture").unwrap();
        assert!(guard
            .wait("fixture", Duration::ZERO)
            .unwrap_err()
            .contains("timed out"));
        drop(guard);
        let log = temp.path().join("oversize.log");
        let mut command = Command::new("sh");
        command.args(["-c", "exit 0"]);
        let mut guard = ProcessGuard::spawn(command, &log, "fixture").unwrap();
        guard.child.wait().unwrap();
        fs::write(&log, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(guard
            .wait("fixture", Duration::from_secs(2))
            .unwrap_err()
            .contains("output cap"));
    }

    #[cfg(unix)]
    #[test]
    fn archive_process_guard_terminates_and_reaps_child() {
        let temp = tempfile::tempdir().unwrap();
        let pid = {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30"]);
            let guard = ProcessGuard::spawn(command, &temp.path().join("child.log"), "fixture")
                .expect("start guarded child");
            let pid = guard.child.id();
            assert_eq!(unsafe { libc::kill(pid as i32, 0) }, 0);
            pid
        };
        assert_ne!(unsafe { libc::kill(pid as i32, 0) }, 0);
    }

    #[test]
    fn clean_archive_requires_no_untracked_files() {
        let root = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        write(root.path(), "untracked.txt", "x");
        let error = ensure_clean_repository(root.path()).unwrap_err();
        assert!(error.contains("nonignored untracked files"));
    }

    #[test]
    fn accepts_public_core_source() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "runtime-rs/src/lib.rs",
            "pub trait IdentityProvider {}",
        );
        let mut failures = Vec::new();
        scan_path(
            root.path(),
            Path::new("runtime-rs/src/lib.rs"),
            &manifest(),
            &mut failures,
        );
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn accepts_canonical_license_and_notice() {
        let root = tempfile::tempdir().unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../LICENSE"),
            root.path().join("LICENSE"),
        )
        .unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../NOTICE"),
            root.path().join("NOTICE"),
        )
        .unwrap();
        let mut failures = Vec::new();
        validate_license_files(root.path(), &mut failures);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn rejects_missing_or_altered_license_files() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "LICENSE", "not Apache-2.0");
        let mut failures = Vec::new();
        validate_license_files(root.path(), &mut failures);
        assert!(failures
            .iter()
            .any(|failure| failure.contains("LICENSE sha256")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("read NOTICE")));

        let missing = tempfile::tempdir().unwrap();
        let mut failures = Vec::new();
        validate_license_files(missing.path(), &mut failures);
        assert!(failures
            .iter()
            .any(|failure| failure.contains("read LICENSE")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("read NOTICE")));
    }

    #[test]
    fn rejects_invalid_notice_markers() {
        let root = tempfile::tempdir().unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../LICENSE"),
            root.path().join("LICENSE"),
        )
        .unwrap();
        write(root.path(), "NOTICE", "TradeAssembly Core\n");
        let mut failures = Vec::new();
        validate_license_files(root.path(), &mut failures);
        assert_eq!(failures.len(), 3);
        assert!(failures.iter().all(|failure| failure.contains("NOTICE")));
    }

    #[test]
    fn rejects_license_metadata_mismatch_and_unready_status() {
        let mut manifest = manifest();
        manifest.license.current_status = "MIT".to_string();
        manifest.license.publication_ready = false;
        let mut failures = Vec::new();
        validate_manifest(&manifest, &mut failures);
        assert!(failures
            .iter()
            .any(|failure| failure.contains("currentStatus")));
        assert!(failures
            .iter()
            .any(|failure| failure.contains("publicationReady")));
    }

    #[test]
    fn public_license_readiness_is_false_for_scan_or_archive_failures() {
        let manifest = manifest();
        let mut report = BoundaryReport {
            schema_version: MANIFEST_SCHEMA,
            status: "failed",
            repository: REPOSITORY,
            publication_class: PUBLICATION_CLASS,
            scanned_files: 0,
            archive_smoke: "not_requested",
            public_license_ready: false,
            license_status: "manifest_unavailable".to_string(),
            failures: Vec::new(),
        };

        finalize_report(&mut report, Some(&manifest), false);
        assert!(report.public_license_ready);

        report.failures.push("scan failed".to_string());
        finalize_report(&mut report, Some(&manifest), false);
        assert!(!report.public_license_ready);

        report.failures.clear();
        finalize_report(&mut report, Some(&manifest), true);
        report.failures.push("archive smoke failed".to_string());
        finalize_report(&mut report, Some(&manifest), true);
        assert!(!report.public_license_ready);
    }
}
