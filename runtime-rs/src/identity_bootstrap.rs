//! Nonblocking local OIDC bootstrap for MCP callers.
//!
//! The authorization callback and all OIDC secrets remain in the worker.  The
//! durable record is deliberately redacted and is only a lifecycle/status
//! record, so an interrupted process cannot be mistaken for a successful
//! login or resume a stale authorization transaction.

use crate::cli_identity::{CliIdentity, CliIdentityManager};
use crate::runtime_config::RuntimeConfig;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const STATE_VERSION: u8 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum BootstrapPhase {
    Pending,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BootstrapRecord {
    version: u8,
    attempt_id: String,
    phase: BootstrapPhase,
    process_id: u32,
    started_at_ms: i64,
    finished_at_ms: Option<i64>,
    identity: Option<Value>,
    error: Option<String>,
    #[serde(default)]
    browser_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct IdentityBootstrap {
    manager: CliIdentityManager,
    state_path: PathBuf,
    customer_auth_unconfigured: bool,
}

impl IdentityBootstrap {
    pub(crate) fn configuration_problem(&self) -> Option<Value> {
        self.customer_auth_unconfigured.then(configuration_required)
    }

    pub fn new(config: RuntimeConfig) -> Self {
        let session_path = PathBuf::from(&config.oidc_session_path);
        let state_path = session_path.with_extension("bootstrap.json");
        Self {
            customer_auth_unconfigured: config.oidc_profile == "workos"
                && (config.oidc_client_id.is_empty() || config.oidc_issuer.is_empty()),
            manager: CliIdentityManager::new(config),
            state_path,
        }
    }

    /// Start login and return immediately. Existing encrypted sessions win.
    pub async fn start(&self) -> Result<Value, String> {
        self.start_with_browser_opener(|authorization_url| {
            webbrowser::open(&authorization_url).map_err(|_| "oidc_browser_open_failed".to_string())
        })
        .await
    }

    pub(crate) async fn start_with_browser_opener<F>(
        &self,
        open_browser: F,
    ) -> Result<Value, String>
    where
        F: FnOnce(String) -> Result<(), String> + Send + 'static,
    {
        if self.customer_auth_unconfigured {
            return Ok(configuration_required());
        }
        if self.manager.uses_local_owner() {
            return Ok(local_owner_result());
        }
        if let Some(record) = self.read_record()? {
            if matches!(record.phase, BootstrapPhase::Pending) && record.process_id == process_id()
            {
                return Ok(record.to_value());
            }
        }
        if let Ok(identity) = self.manager.current_identity().await {
            return Ok(authenticated_result(&identity, true));
        }

        if let Some(record) = self.read_record()? {
            if matches!(record.phase, BootstrapPhase::Pending) {
                if record.process_id == process_id() {
                    return Ok(record.to_value());
                }
                let interrupted = record.interrupted();
                self.write_record(&interrupted)?;
            }
        }

        let attempt_id = new_attempt_id();
        let record = BootstrapRecord {
            version: STATE_VERSION,
            attempt_id: attempt_id.clone(),
            phase: BootstrapPhase::Pending,
            process_id: process_id(),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            identity: None,
            error: None,
            browser_url: None,
        };
        self.write_record(&record)?;

        let manager = self.manager.clone();
        let state_path = self.state_path.clone();
        let worker_attempt_id = attempt_id.clone();
        std::thread::Builder::new()
            .name("tradeassembly-oidc-bootstrap".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = write_failure(
                            &state_path,
                            &worker_attempt_id,
                            "oidc_worker_unavailable",
                        );
                        return;
                    }
                };
                let browser_state_path = state_path.clone();
                let browser_attempt_id = worker_attempt_id.clone();
                let outcome = runtime.block_on(manager.login_with_browser_opener(move |url| {
                    update_record(&browser_state_path, &browser_attempt_id, |record| {
                        record.browser_url = Some(url.clone());
                    })?;
                    // Opening the browser is a convenience. The caller can use the
                    // returned URL when no desktop browser is available.
                    let _ = open_browser(url);
                    Ok(())
                }));
                match outcome {
                    Ok(identity) => {
                        let _ = write_success(&state_path, &worker_attempt_id, &identity);
                    }
                    Err(error) => {
                        let _ = write_failure(&state_path, &worker_attempt_id, &error);
                    }
                }
            })
            .map_err(|_| "oidc_worker_unavailable".to_string())?;

        Ok(json!({
            "authenticated": false,
            "status": "pending",
            "attemptId": attempt_id,
            "browserLaunch": "requested",
            "retryable": true,
        }))
    }

    /// Inspect bootstrap state without exposing authorization material.
    pub async fn status(&self) -> Result<Value, String> {
        if self.customer_auth_unconfigured {
            return Ok(configuration_required());
        }
        if self.manager.uses_local_owner() {
            return Ok(local_owner_result());
        }
        // Polling an active login must not contend with token persistence.
        if let Some(record) = self.read_record()? {
            if matches!(record.phase, BootstrapPhase::Pending) && record.process_id == process_id()
            {
                return Ok(record.to_value());
            }
        }
        if let Ok(identity) = self.manager.current_identity().await {
            return Ok(authenticated_result(&identity, true));
        }
        let Some(record) = self.read_record()? else {
            return Ok(json!({
                "authenticated": false,
                "status": "required",
                "retryable": true,
            }));
        };
        if matches!(record.phase, BootstrapPhase::Pending) && record.process_id != process_id() {
            let interrupted = record.interrupted();
            self.write_record(&interrupted)?;
            return Ok(interrupted.to_value());
        }
        if matches!(record.phase, BootstrapPhase::Succeeded) {
            return Ok(json!({
                "authenticated": false,
                "status": "required",
                "retryable": true,
            }));
        }
        Ok(record.to_value())
    }

    fn read_record(&self) -> Result<Option<BootstrapRecord>, String> {
        let bytes = match fs::read(&self.state_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("oidc_bootstrap_state_unavailable".to_string()),
        };
        let record: BootstrapRecord = serde_json::from_slice(&bytes)
            .map_err(|_| "oidc_bootstrap_state_invalid".to_string())?;
        if record.version != STATE_VERSION || record.attempt_id.trim().is_empty() {
            return Err("oidc_bootstrap_state_invalid".to_string());
        }
        Ok(Some(record))
    }

    fn write_record(&self, record: &BootstrapRecord) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|_| "oidc_bootstrap_state_unavailable".to_string())?;
        let parent = self
            .state_path
            .parent()
            .ok_or_else(|| "oidc_bootstrap_state_unavailable".to_string())?;
        fs::create_dir_all(parent).map_err(|_| "oidc_bootstrap_state_unavailable".to_string())?;
        atomic_state_write(&self.state_path, &bytes)
    }
}

fn configuration_required() -> Value {
    json!({
        "authenticated": false,
        "status": "configuration_required",
        "error": "workos_configuration_required",
        "retryable": false,
        "browserLaunch": "not_started",
        "message": "This installation is missing its registered TradeAssembly sign-in configuration. Repair the installation with its published client configuration; do not install Keycloak or inspect source code.",
        "requiredConfiguration": ["TRADEASSEMBLY_AUTH_CLIENT_ID", "TRADEASSEMBLY_AUTH_ISSUER"],
    })
}

fn local_owner_result() -> Value {
    json!({
        "authenticated": false,
        "hubAuthenticated": false,
        "localRuntimeAvailable": true,
        "status": "configuration_required",
        "error": "hosted_sign_in_not_configured",
        "retryable": false,
        "browserLaunch": "not_started",
        "message": "Local access is ready. Hosted sign-in is not configured in this installation. Use the published connection setup; do not paste credentials or edit configuration.",
    })
}

#[cfg(test)]
mod customer_configuration_tests {
    use super::*;

    #[tokio::test]
    async fn local_owner_is_not_hosted_authentication() {
        let mut config = RuntimeConfig::local(":memory:");
        config.oidc_profile = "local_owner".into();
        let bootstrap = IdentityBootstrap::new(config);
        let result = bootstrap
            .start_with_browser_opener(|_| panic!("local owner must not open a browser"))
            .await
            .unwrap();
        assert_eq!(result["authenticated"], false);
        assert_eq!(result["hubAuthenticated"], false);
        assert_eq!(result["localRuntimeAvailable"], true);
        assert_eq!(bootstrap.status().await.unwrap(), result);
    }

    #[tokio::test]
    async fn missing_customer_configuration_neither_opens_browser_nor_writes_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::local(":memory:");
        config.oidc_session_path = dir
            .path()
            .join("session.bin")
            .to_string_lossy()
            .into_owned();
        let bootstrap = IdentityBootstrap::new(config);
        let outcome = bootstrap
            .start_with_browser_opener(|_| panic!("must not launch"))
            .await
            .unwrap();
        assert_eq!(outcome["status"], "configuration_required");
        assert_eq!(bootstrap.status().await.unwrap(), outcome);
        assert!(!bootstrap.state_path.exists());
    }
}

impl BootstrapRecord {
    fn interrupted(&self) -> Self {
        Self {
            phase: BootstrapPhase::Interrupted,
            process_id: process_id(),
            finished_at_ms: Some(now_ms()),
            error: Some("oidc_bootstrap_interrupted".to_string()),
            browser_url: None,
            ..self.clone()
        }
    }

    fn to_value(&self) -> Value {
        let status = match self.phase {
            BootstrapPhase::Pending => "pending",
            BootstrapPhase::Succeeded => "authenticated",
            BootstrapPhase::Failed => "failed",
            BootstrapPhase::Interrupted => "interrupted",
        };
        json!({
            "authenticated": matches!(self.phase, BootstrapPhase::Succeeded),
            "status": status,
            "attemptId": self.attempt_id,
            "startedAtMs": self.started_at_ms,
            "finishedAtMs": self.finished_at_ms,
            "identity": self.identity,
            "error": self.error,
            "retryable": !matches!(self.phase, BootstrapPhase::Succeeded),
            "browserUrl": if matches!(self.phase, BootstrapPhase::Pending) { self.browser_url.clone() } else { None },
        })
    }
}

fn authenticated_result(identity: &CliIdentity, reused: bool) -> Value {
    json!({
        "authenticated": true,
        "status": "authenticated",
        "sessionReused": reused,
        "identity": identity.redacted_status(),
        "retryable": false,
    })
}

fn write_success(path: &PathBuf, attempt_id: &str, identity: &CliIdentity) -> Result<(), String> {
    update_record(path, attempt_id, |record| {
        record.phase = BootstrapPhase::Succeeded;
        record.finished_at_ms = Some(now_ms());
        record.identity = Some(identity.redacted_status());
        record.error = None;
        record.browser_url = None;
    })
}

fn write_failure(path: &PathBuf, attempt_id: &str, error: &str) -> Result<(), String> {
    let error = allowlisted_error(error);
    update_record(path, attempt_id, |record| {
        record.phase = BootstrapPhase::Failed;
        record.finished_at_ms = Some(now_ms());
        record.error = Some(error.to_string());
        record.browser_url = None;
    })
}

fn update_record<F>(path: &PathBuf, attempt_id: &str, update: F) -> Result<(), String>
where
    F: FnOnce(&mut BootstrapRecord),
{
    let bytes = fs::read(path).map_err(|_| "oidc_bootstrap_state_unavailable".to_string())?;
    let mut record: BootstrapRecord =
        serde_json::from_slice(&bytes).map_err(|_| "oidc_bootstrap_state_invalid".to_string())?;
    if record.attempt_id != attempt_id || !matches!(record.phase, BootstrapPhase::Pending) {
        return Ok(());
    }
    update(&mut record);
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|_| "oidc_bootstrap_state_unavailable".to_string())?;
    atomic_state_write(path, &bytes)
}

fn atomic_state_write(path: &PathBuf, bytes: &[u8]) -> Result<(), String> {
    let mut suffix = [0_u8; 8];
    OsRng.fill_bytes(&mut suffix);
    let temporary = path.with_extension(format!(
        "bootstrap.json.tmp-{}",
        URL_SAFE_NO_PAD.encode(suffix)
    ));
    fs::write(&temporary, bytes).map_err(|_| "oidc_bootstrap_state_unavailable".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&temporary)
            .map_err(|_| "oidc_bootstrap_state_unavailable".to_string())?
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&temporary, permissions)
            .map_err(|_| "oidc_bootstrap_state_unavailable".to_string())?;
    }
    fs::rename(&temporary, path).map_err(|_| {
        let _ = fs::remove_file(&temporary);
        "oidc_bootstrap_state_unavailable".to_string()
    })
}

fn allowlisted_error(error: &str) -> String {
    match error {
        "oidc_browser_open_failed"
        | "oidc_callback_listener_unavailable"
        | "oidc_callback_timeout"
        | "oidc_callback_failed"
        | "oidc_callback_peer_invalid"
        | "oidc_callback_invalid"
        | "oidc_callback_host_invalid"
        | "oidc_callback_path_invalid"
        | "oidc_provider_rejected"
        | "oidc_authorization_failed"
        | "oidc_authorization_start_failed"
        | "oidc_configuration_invalid"
        | "oidc_identity_binding_invalid"
        | "oidc_worker_unavailable"
        | "workos_configuration_required"
        | "workos_configuration_invalid"
        | "workos_browser_open_failed"
        | "workos_callback_listener_unavailable"
        | "workos_state_invalid"
        | "workos_token_exchange_failed"
        | "workos_token_invalid"
        | "bitwarden_session_required"
        | "bitwarden_session_store_unavailable"
        | "bitwarden_session_conflict"
        | "workos_migration_source_invalid"
        | "workos_token_header_invalid"
        | "workos_token_signing_key_unavailable"
        | "workos_token_signing_key_invalid"
        | "workos_token_issuer_mismatch"
        | "workos_token_client_mismatch"
        | "workos_token_claims_invalid"
        | "workos_token_expired"
        | "workos_token_not_yet_valid"
        | "workos_token_signature_invalid"
        | "workos_jwks_unavailable"
        | "workos_identity_binding_invalid"
        | "workos_organization_binding_invalid"
        | "workos_hub_identity_permission_missing"
        | "workos_session_busy"
        | "workos_refresh_token_missing"
        | "oidc_session_store_unavailable"
        | "hub_identity_denied"
        | "hub_identity_unauthorized"
        | "hub_identity_forbidden"
        | "hub_identity_permission_denied"
        | "hub_identity_client_unregistered"
        | "hub_identity_binding_invalid"
        | "hub_response_invalid"
        | "hub_unavailable" => error.to_string(),
        _ => "oidc_authorization_failed".to_string(),
    }
}

#[test]
fn token_diagnostics_are_allowlisted_without_accepting_arbitrary_details() {
    for code in [
        "workos_token_header_invalid",
        "workos_token_signing_key_unavailable",
        "workos_token_signing_key_invalid",
        "workos_token_issuer_mismatch",
        "workos_token_client_mismatch",
        "workos_token_claims_invalid",
        "workos_token_expired",
        "workos_token_not_yet_valid",
        "workos_token_signature_invalid",
    ] {
        assert_eq!(allowlisted_error(code), code);
        assert_eq!(
            allowlisted_error(&format!("{code}: secret-sentinel")),
            "oidc_authorization_failed"
        );
    }
}

fn process_id() -> u32 {
    std::process::id()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn new_attempt_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn manager() -> IdentityBootstrap {
        let root = tempdir().unwrap();
        let mut config = RuntimeConfig::local(root.path().join("runtime.db").display().to_string());
        config.oidc_profile = "oidc_test".to_string();
        config.oidc_session_path = root.path().join("session.bin").display().to_string();
        config.oidc_session_key_path = root.path().join("session.key").display().to_string();
        let bootstrap = IdentityBootstrap::new(config);
        std::mem::forget(root);
        bootstrap
    }

    #[test]
    fn missing_state_is_retryable_and_redacted() {
        let bootstrap = manager();
        let status = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(bootstrap.status())
            .unwrap();
        assert_eq!(status["status"], "required");
        assert_eq!(status["retryable"], true);
        assert!(!status.to_string().contains("verifier"));
    }

    #[test]
    fn prior_process_pending_state_becomes_interrupted() {
        let bootstrap = manager();
        let record = BootstrapRecord {
            version: STATE_VERSION,
            attempt_id: "old-attempt".to_string(),
            phase: BootstrapPhase::Pending,
            process_id: process_id().saturating_add(1),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            identity: None,
            error: None,
            browser_url: Some("https://example.test/authorize?state=old".to_string()),
        };
        bootstrap.write_record(&record).unwrap();
        let status = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(bootstrap.status())
            .unwrap();
        assert_eq!(status["status"], "interrupted");
        assert_eq!(status["error"], "oidc_bootstrap_interrupted");
        assert_eq!(status["authenticated"], false);
        assert!(status["browserUrl"].is_null());
    }

    #[test]
    fn success_record_without_valid_session_cannot_claim_authentication() {
        let bootstrap = manager();
        let record = BootstrapRecord {
            version: STATE_VERSION,
            attempt_id: "ok-attempt".to_string(),
            phase: BootstrapPhase::Succeeded,
            process_id: process_id(),
            started_at_ms: now_ms(),
            finished_at_ms: Some(now_ms()),
            identity: Some(json!({"authenticated": true, "stableIdentityId": "stable"})),
            error: None,
            browser_url: None,
        };
        bootstrap.write_record(&record).unwrap();
        let status = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(bootstrap.status())
            .unwrap();
        assert_eq!(status["status"], "required");
        assert_eq!(status["authenticated"], false);
        assert!(!status.to_string().contains("refresh_token"));
    }
}
