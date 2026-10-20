//! Versioned, bounded JSON Lines types for TradeAssembly external plugins.
//!
//! A host writes exactly one [`PluginRequest`] and closes stdin. A plugin writes
//! exactly one [`PluginResponse`] and exits. Credential grants are ephemeral
//! request transport data and are redacted from debug output.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt,
    io::{self, BufRead, Write},
};

/// The initial public host and SDK wire-contract version.
pub const WIRE_CONTRACT_VERSION: &str = "1";

/// The exact SDK crate version compiled into a host or plugin.
pub const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The canonical Rust target triple for the host process.
pub const fn host_target() -> &'static str {
    if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(
        target_arch = "aarch64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "musl"
    )) {
        "x86_64-unknown-linux-musl"
    } else if cfg!(all(
        target_arch = "aarch64",
        target_os = "linux",
        target_env = "musl"
    )) {
        "aarch64-unknown-linux-musl"
    } else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_arch = "aarch64", target_os = "windows")) {
        "aarch64-pc-windows-msvc"
    } else {
        "unsupported-target"
    }
}

/// Maximum permitted bytes for a single public JSONL envelope.
pub const DEFAULT_MAX_ENVELOPE_BYTES: usize = 1_048_576;

/// A generic public request delivered to one plugin process.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRequest {
    pub schema_version: String,
    pub host_contract_version: String,
    pub sdk_version: String,
    pub request_id: String,
    pub metadata: RequestMetadata,
    pub context: RequestContext,
    pub payload: RequestPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral_credential_grant: Option<EphemeralCredentialGrant>,
}

impl fmt::Debug for PluginRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginRequest")
            .field("schema_version", &self.schema_version)
            .field("host_contract_version", &self.host_contract_version)
            .field("sdk_version", &self.sdk_version)
            .field("request_id", &self.request_id)
            .field("metadata", &self.metadata)
            .field("context", &self.context)
            .field("payload", &self.payload)
            .field(
                "ephemeral_credential_grant",
                &self
                    .ephemeral_credential_grant
                    .as_ref()
                    .map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// Non-secret identifiers and fingerprints that bind a request to its package.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestMetadata {
    pub operation_id: String,
    pub plugin_id: String,
    pub plugin_instance_id: String,
    pub package_digest: String,
    pub manifest_digest: String,
    pub capability_binding_id: String,
    pub capability_fingerprint: String,
}

/// Non-secret authority and replay context resolved by the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestContext {
    pub authority_id: String,
    pub purpose: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub fencing_token: String,
    pub timeout_ms: u64,
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_references: Vec<String>,
}

/// One request kind and its operation-specific JSON payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "camelCase")]
pub enum RequestPayload {
    Operation(Value),
    HistoricalData(Value),
    BrokerOrder(Value),
    Health(Value),
    CapabilityMatrix(Value),
}

/// Request-scoped credentials keyed by host-defined grant names.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EphemeralCredentialGrant(BTreeMap<String, String>);

impl EphemeralCredentialGrant {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self(values)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for EphemeralCredentialGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// A generic public response emitted by one plugin process.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginResponse {
    pub schema_version: String,
    pub request_id: String,
    pub status: ResponseStatus,
    pub response_schema: String,
    pub payload: Value,
    pub provider_outcome: ProviderOutcome,
    pub reconciliation: Reconciliation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_references: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_diagnostics: Vec<RedactedDiagnostic>,
}

/// Stable host-visible response status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Succeeded,
    Failed,
    ReconciliationRequired,
}

/// Factual provider result details; values must be non-secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderOutcome {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_reference: Option<String>,
}

/// Whether an effectful request has a final provider outcome.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconciliation {
    NotRequired,
    Reconciled,
    Required,
}

/// A bounded, redacted diagnostic suitable for host display or journaling.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedactedDiagnostic {
    pub code: String,
    pub message: String,
}

/// Immutable descriptor stored as `tradeassembly-plugin.json` in a plugin package.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPackageDescriptor {
    pub package_contract_version: String,
    pub plugin: PackagePlugin,
    pub manifest: DigestedPackagePath,
    pub targets: Vec<PackageTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_schemas: Vec<PackageResponseSchema>,
    pub compatibility: PackageCompatibility,
}

impl PluginPackageDescriptor {
    /// Validates fields that must be safe before package extraction or execution.
    pub fn validate(&self) -> Result<(), PackageDescriptorError> {
        if self.package_contract_version != WIRE_CONTRACT_VERSION {
            return Err(PackageDescriptorError::UnsupportedContractVersion);
        }
        if self.plugin.id.is_empty() || self.plugin.version.is_empty() {
            return Err(PackageDescriptorError::MissingPluginIdentity);
        }
        self.manifest.validate()?;
        if self.targets.is_empty() {
            return Err(PackageDescriptorError::MissingTargets);
        }
        for target in &self.targets {
            if target.target.is_empty() {
                return Err(PackageDescriptorError::MissingTarget);
            }
            target.binary.validate()?;
        }
        let mut schema_refs = std::collections::BTreeSet::new();
        for schema in &self.response_schemas {
            if schema.schema_ref.is_empty() || !schema_refs.insert(&schema.schema_ref) {
                return Err(PackageDescriptorError::InvalidResponseSchema);
            }
            schema.document.validate()?;
        }
        if self.compatibility.host_contract.minimum.is_empty()
            || self.compatibility.host_contract.maximum.is_empty()
            || self.compatibility.sdk_version.is_empty()
        {
            return Err(PackageDescriptorError::MissingCompatibility);
        }
        Ok(())
    }
}

/// Plugin identity embedded in an immutable package descriptor.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackagePlugin {
    pub id: String,
    pub version: String,
}

/// A package-relative path and its lowercase SHA-256 digest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DigestedPackagePath {
    pub path: String,
    pub sha256: String,
}

impl DigestedPackagePath {
    fn validate(&self) -> Result<(), PackageDescriptorError> {
        if !is_safe_package_path(&self.path) {
            return Err(PackageDescriptorError::UnsafePackagePath);
        }
        if !is_sha256(&self.sha256) {
            return Err(PackageDescriptorError::InvalidSha256);
        }
        Ok(())
    }
}

/// Target-specific executable declared by a package.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageTarget {
    pub target: String,
    pub binary: DigestedPackagePath,
}

/// One response schema bound to a package-relative, digested JSON Schema file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageResponseSchema {
    pub schema_ref: String,
    pub document: DigestedPackagePath,
}

/// Host and SDK compatibility declared by a package.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageCompatibility {
    pub host_contract: HostContractCompatibility,
    pub sdk_version: String,
}

/// Inclusive host-contract range declared by a package.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostContractCompatibility {
    pub minimum: String,
    pub maximum: String,
}

/// Validation failures for a package descriptor. Errors do not include input values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageDescriptorError {
    UnsupportedContractVersion,
    MissingPluginIdentity,
    UnsafePackagePath,
    InvalidSha256,
    MissingTargets,
    MissingTarget,
    MissingCompatibility,
    InvalidResponseSchema,
}

impl fmt::Display for PackageDescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedContractVersion => "unsupported package contract version",
            Self::MissingPluginIdentity => "missing package plugin identity",
            Self::UnsafePackagePath => "unsafe package-relative path",
            Self::InvalidSha256 => "invalid package SHA-256 digest",
            Self::MissingTargets => "package has no target binaries",
            Self::MissingTarget => "package target is missing",
            Self::MissingCompatibility => "package compatibility is incomplete",
            Self::InvalidResponseSchema => "package response schema mapping is invalid",
        })
    }
}

impl std::error::Error for PackageDescriptorError {}

/// Returns whether `path` is a non-absolute, traversal-free package path.
pub fn is_safe_package_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Reads exactly one newline-terminated JSON envelope within `max_bytes`.
pub fn read_request<R: BufRead>(reader: R, max_bytes: usize) -> Result<PluginRequest, WireError> {
    read_envelope(reader, max_bytes)
}

/// Reads exactly one newline-terminated JSON response envelope within `max_bytes`.
pub fn read_response<R: BufRead>(reader: R, max_bytes: usize) -> Result<PluginResponse, WireError> {
    read_envelope(reader, max_bytes)
}

/// Serializes one request envelope as a bounded JSON Lines message.
pub fn write_request<W: Write>(
    writer: W,
    request: &PluginRequest,
    max_bytes: usize,
) -> Result<(), WireError> {
    write_envelope(writer, request, max_bytes)
}

/// Serializes one response envelope as a bounded JSON Lines message.
pub fn write_response<W: Write>(
    writer: W,
    response: &PluginResponse,
    max_bytes: usize,
) -> Result<(), WireError> {
    write_envelope(writer, response, max_bytes)
}

fn read_envelope<R: BufRead, T: for<'de> Deserialize<'de>>(
    mut reader: R,
    max_bytes: usize,
) -> Result<T, WireError> {
    if max_bytes == 0 {
        return Err(WireError::InvalidLimit);
    }

    let read_limit = max_bytes.checked_add(1).ok_or(WireError::InvalidLimit)?;
    let mut bytes = Vec::with_capacity(read_limit.min(DEFAULT_MAX_ENVELOPE_BYTES));
    let read = {
        let mut limited = std::io::Read::take(&mut reader, read_limit as u64);
        limited.read_until(b'\n', &mut bytes)?
    };
    if read == 0 {
        return Err(WireError::MissingEnvelope);
    }
    if bytes.len() > max_bytes {
        return Err(WireError::EnvelopeTooLarge { max_bytes });
    }
    if bytes.pop() != Some(b'\n') {
        return Err(WireError::MissingNewline);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(WireError::MissingEnvelope);
    }
    let mut trailing = [0_u8; 1];
    if std::io::Read::read(&mut reader, &mut trailing)? != 0 {
        return Err(WireError::ExtraEnvelopeData);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_envelope<W: Write, T: Serialize>(
    mut writer: W,
    envelope: &T,
    max_bytes: usize,
) -> Result<(), WireError> {
    if max_bytes == 0 {
        return Err(WireError::InvalidLimit);
    }
    let mut bytes = serde_json::to_vec(envelope)?;
    if bytes.len() > max_bytes {
        return Err(WireError::EnvelopeTooLarge { max_bytes });
    }
    bytes.push(b'\n');
    write_frame(&mut writer, &bytes, std::time::Duration::from_secs(5))?;
    Ok(())
}

// A sandbox launcher can pass a nonblocking pipe to the plugin. Preserve partial
// writes under backpressure without spinning forever or repeating frame bytes.
fn write_frame<W: Write>(
    writer: &mut W,
    bytes: &[u8],
    budget: std::time::Duration,
) -> io::Result<()> {
    let deadline = std::time::Instant::now() + budget;
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let count = retry_transport(deadline, || writer.write(remaining))?;
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        remaining = &remaining[count..];
    }
    retry_transport(deadline, || writer.flush())
}

fn retry_transport<T>(
    deadline: std::time::Instant,
    mut operation: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    loop {
        match operation() {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                if std::time::Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            result => return result,
        }
    }
}

/// Public framing failures. Error messages do not include envelope contents.
#[derive(Debug)]
pub enum WireError {
    InvalidLimit,
    MissingEnvelope,
    MissingNewline,
    ExtraEnvelopeData,
    EnvelopeTooLarge { max_bytes: usize },
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimit => formatter.write_str("invalid envelope byte limit"),
            Self::MissingEnvelope => formatter.write_str("missing JSONL envelope"),
            Self::MissingNewline => formatter.write_str("JSONL envelope must end with a newline"),
            Self::ExtraEnvelopeData => formatter.write_str("expected exactly one JSONL envelope"),
            Self::EnvelopeTooLarge { max_bytes } => {
                write!(formatter, "JSONL envelope exceeds {max_bytes} bytes")
            }
            Self::Io(_) => formatter.write_str("JSONL transport I/O failure"),
            Self::Json(_) => formatter.write_str("invalid JSONL envelope"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<io::Error> for WireError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for WireError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn request() -> PluginRequest {
        PluginRequest {
            schema_version: WIRE_CONTRACT_VERSION.to_string(),
            host_contract_version: WIRE_CONTRACT_VERSION.to_string(),
            sdk_version: "0.1.0".to_string(),
            request_id: "request-1".to_string(),
            metadata: RequestMetadata {
                operation_id: "health".to_string(),
                plugin_id: "tradeassembly.example".to_string(),
                plugin_instance_id: "example-1".to_string(),
                package_digest: "a".repeat(64),
                manifest_digest: "b".repeat(64),
                capability_binding_id: "binding-1".to_string(),
                capability_fingerprint: "fingerprint-1".to_string(),
            },
            context: RequestContext {
                authority_id: "authority-1".to_string(),
                purpose: "connection_check".to_string(),
                mode: "paper".to_string(),
                account_id: None,
                fencing_token: "fence-1".to_string(),
                timeout_ms: 1_000,
                idempotency_key: "idempotency-1".to_string(), // gitleaks:allow -- public protocol test key, not a credential
                strategy_id: None,
                activation_id: None,
                attempt_id: None,
                tick_id: None,
                evidence_references: vec!["evidence-1".to_string()],
            },
            payload: RequestPayload::Health(json!({"probe": "connection"})),
            ephemeral_credential_grant: Some(EphemeralCredentialGrant::new(BTreeMap::from([(
                "api_secret".to_string(),
                "not-for-debug-output".to_string(),
            )]))),
        }
    }

    #[test]
    fn request_round_trips_as_one_closed_envelope() {
        let original = request();
        let mut output = Vec::new();
        write_request(&mut output, &original, DEFAULT_MAX_ENVELOPE_BYTES).unwrap();

        let decoded = read_request(Cursor::new(output), DEFAULT_MAX_ENVELOPE_BYTES).unwrap();
        assert_eq!(decoded.request_id, original.request_id);
        assert_eq!(decoded.payload, original.payload);
        assert_eq!(
            decoded
                .ephemeral_credential_grant
                .unwrap()
                .get("api_secret"),
            Some("not-for-debug-output")
        );
    }

    #[test]
    fn frame_writer_preserves_partial_output_under_backpressure() {
        #[derive(Default)]
        struct Backpressure {
            bytes: Vec<u8>,
            writes: usize,
            flushes: usize,
        }
        impl Write for Backpressure {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.writes += 1;
                if self.writes % 2 == 1 {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let count = bytes.len().min(3);
                self.bytes.extend_from_slice(&bytes[..count]);
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                if self.flushes == 1 {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            }
        }
        let mut writer = Backpressure::default();
        write_frame(
            &mut writer,
            b"exact frame\n",
            std::time::Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(writer.bytes, b"exact frame\n");
        assert_eq!(writer.flushes, 2);
    }

    #[test]
    fn transport_backpressure_has_a_deadline_and_preserves_permanent_errors() {
        let deadline = std::time::Instant::now();
        let stalled = retry_transport::<()>(deadline, || Err(io::ErrorKind::WouldBlock.into()));
        assert_eq!(stalled.unwrap_err().kind(), io::ErrorKind::TimedOut);
        let broken = retry_transport::<()>(deadline, || Err(io::ErrorKind::BrokenPipe.into()));
        assert_eq!(broken.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[cfg(unix)]
    #[test]
    fn frame_survives_a_real_full_nonblocking_socket() {
        use std::io::Read;
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        let (mut writer, mut reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let mut prefix_len = 0;
        loop {
            match writer.write(&[b'p'; 4096]) {
                Ok(count) => prefix_len += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                other => panic!("unexpected socket result: {other:?}"),
            }
            assert!(prefix_len < 16 * 1024 * 1024);
        }
        let receiver = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let payload = vec![b'x'; 512 * 1024];
        write_frame(&mut writer, &payload, std::time::Duration::from_secs(5)).unwrap();
        writer.shutdown(Shutdown::Write).unwrap();
        let bytes = receiver.join().unwrap();
        assert!(bytes[..prefix_len].iter().all(|byte| *byte == b'p'));
        assert_eq!(&bytes[prefix_len..], payload);
    }

    #[test]
    fn request_debug_redacts_credential_grants() {
        let debug = format!("{:?}", request());
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("not-for-debug-output"));
    }

    #[test]
    fn reader_rejects_unknown_fields_extra_data_and_oversized_input() {
        let mut unknown = serde_json::to_value(request()).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_string(), Value::Bool(true));
        let unknown = format!("{}\n", serde_json::to_string(&unknown).unwrap());
        assert!(matches!(
            read_request(Cursor::new(unknown), DEFAULT_MAX_ENVELOPE_BYTES),
            Err(WireError::Json(_))
        ));
        assert!(matches!(
            read_request(Cursor::new(b"{}\n{}\n"), DEFAULT_MAX_ENVELOPE_BYTES),
            Err(WireError::ExtraEnvelopeData)
        ));
        assert!(matches!(
            read_request(Cursor::new(b"{}\n"), 1),
            Err(WireError::EnvelopeTooLarge { .. })
        ));
    }

    #[test]
    fn writer_enforces_the_selected_bound() {
        assert!(matches!(
            write_request(Vec::new(), &request(), 1),
            Err(WireError::EnvelopeTooLarge { .. })
        ));
    }

    #[test]
    fn package_descriptor_rejects_unsafe_paths_and_invalid_digests() {
        let descriptor = PluginPackageDescriptor {
            package_contract_version: WIRE_CONTRACT_VERSION.to_string(),
            plugin: PackagePlugin {
                id: "tradeassembly.example".to_string(),
                version: "1.0.0".to_string(),
            },
            manifest: DigestedPackagePath {
                path: "manifest.yaml".to_string(),
                sha256: "a".repeat(64),
            },
            targets: vec![PackageTarget {
                target: "x86_64-unknown-linux-gnu".to_string(),
                binary: DigestedPackagePath {
                    path: "bin/x86_64-unknown-linux-gnu/plugin".to_string(),
                    sha256: "b".repeat(64),
                },
            }],
            response_schemas: vec![PackageResponseSchema {
                schema_ref: "schema://tradeassembly.example/response@1".to_string(),
                document: DigestedPackagePath {
                    path: "schemas/response.json".to_string(),
                    sha256: "c".repeat(64),
                },
            }],
            compatibility: PackageCompatibility {
                host_contract: HostContractCompatibility {
                    minimum: "1".to_string(),
                    maximum: "1".to_string(),
                },
                sdk_version: "0.1.0".to_string(),
            },
        };
        assert!(descriptor.validate().is_ok());
        assert!(!is_safe_package_path("../plugin"));

        let mut invalid = descriptor;
        invalid.manifest.sha256 = "invalid".to_string();
        assert_eq!(
            invalid.validate(),
            Err(PackageDescriptorError::InvalidSha256)
        );
    }

    #[test]
    fn host_target_is_supported() {
        assert_ne!(super::host_target(), "unsupported-target");
    }
}
